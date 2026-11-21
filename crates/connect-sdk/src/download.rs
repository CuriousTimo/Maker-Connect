//! Download functionality for SDK (`download.py`).
//!
//! Reference: the SDK at `c0d7d233` (branch `websockets`, `docs/design/wire.md`), its
//! `prusa/connect/printer/download.py`, ported function by function.
//!
//! **Ours** (no part of the wire): `to_os_path` reads only `fs.sep` and each storage's
//! `path_storage` of `files.Filesystem` (`download.py:314-334`), and `files.py` is not ported, so
//! [`Filesystem`] here is just those two. The Python exceptions are [`Exception`], with the
//! `repr` that `command.py` sends as `error=repr(err)` and the `str` the loop sends as
//! `reason=str(err)`.
//!
//! Unix only, as [`crate::pyos`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use maker_connect_gcode_metadata::py::repr_str;
use serde_json::Value;

use crate::clock;
use crate::command::Finish;
use crate::consts::{self, Event, Source, TransferType};
use crate::models::{EventCallback, Kwargs};
use crate::pyos;
use crate::util::{RequestError, Session};

pub const DOWNLOAD_TYPES: [TransferType; 3] = [
    TransferType::FromWeb,
    TransferType::FromConnect,
    TransferType::FromPrinter,
];

/// A Python exception raised in this module or by what it calls.
#[derive(Debug, Clone, PartialEq)]
pub enum Exception {
    /// An `OSError` (or a subclass) of an `os` call or of `open`.
    Os(pyos::OsError),
    /// `ValueError(message)`.
    Value(String),
    /// Any other exception with one message: its class name and the message.
    Other(&'static str, String),
}

impl Exception {
    /// `repr(err)`.
    pub fn repr(&self) -> String {
        match self {
            Exception::Os(err) => err.repr(),
            Exception::Value(message) => format!("ValueError({})", repr_str(message)),
            Exception::Other(class, message) => format!("{class}({})", repr_str(message)),
        }
    }

    /// `str(err)`.
    pub fn str(&self) -> String {
        match self {
            Exception::Os(err) => err.str(),
            Exception::Value(message) | Exception::Other(_, message) => message.clone(),
        }
    }

    fn transfer_aborted(message: impl Into<String>) -> Exception {
        Exception::Other("TransferAbortedError", message.into())
    }

    fn is_transfer_stopped(&self) -> bool {
        matches!(self, Exception::Other("TransferStoppedError", _))
    }
}

impl From<pyos::Error> for Exception {
    fn from(err: pyos::Error) -> Exception {
        match err {
            pyos::Error::Os(err) => Exception::Os(err),
            pyos::Error::Value(message) => Exception::Value(message),
        }
    }
}

impl From<RequestError> for Exception {
    /// `requests`' exceptions; their messages are not urllib3's (see the audit).
    fn from(err: RequestError) -> Exception {
        match err {
            RequestError::ConnectionError(message) => Exception::Other("ConnectionError", message),
            RequestError::ReadTimeout(message) => Exception::Other("ReadTimeout", message),
            RequestError::Other(message) => Exception::Other("RequestException", message),
        }
    }
}

/// `ForbiddenCharactersError()`
pub fn forbidden_characters_error() -> Exception {
    Exception::Other(
        "ForbiddenCharactersError",
        "Forbidden characters in filename or foldername".into(),
    )
}

/// `FilenameTooLongError()`
pub fn filename_too_long_error() -> Exception {
    Exception::Other("FilenameTooLongError", "Filename length is too long".into())
}

/// `FoldernameTooLongError()`
pub fn foldername_too_long_error() -> Exception {
    Exception::Other(
        "FoldernameTooLongError",
        "Foldername length is too long".into(),
    )
}

/// Check if path contains any of the forbidden characters e.g. '\'
pub fn forbidden_characters(path: &str) -> bool {
    consts::FORBIDDEN_CHARACTERS
        .iter()
        .any(|character| path.contains(*character))
}

/// Check if filename lenght, including .gcode suffix, is > 248 characters
pub fn filename_too_long(filename: &str) -> bool {
    filename.len() > consts::FILENAME_LENGTH
}

/// Check if any foldername length in path is > 255 characters
pub fn foldername_too_long(path: &str) -> bool {
    path.split('/')
        .any(|folder| folder.chars().count() > consts::MAX_NAME_LENGTH)
}

/// Return transfer ID as 32bit integer
pub fn generate_transfer_id() -> u32 {
    // randint(0, 2**32 - 1)
    let mut bytes = [0u8; 4];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut bytes);
    }
    u32::from_ne_bytes(bytes)
}

/// `float("%.2f" % x)`
fn round2(x: f64) -> f64 {
    format!("{x:.2}").parse().unwrap_or(x)
}

/// Python's `int(s)` of a header value: surrounding whitespace, a sign, digits with single
/// underscores between them; anything else is the `ValueError`.
fn py_int(s: &str) -> Result<i64, Exception> {
    let invalid = || {
        Exception::Value(format!(
            "invalid literal for int() with base 10: {}",
            repr_str(s)
        ))
    };
    let t = s.trim_matches(|c: char| c.is_whitespace());
    let (negative, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c == '_' || c.is_ascii_digit())
    {
        return Err(invalid());
    }
    let value: i64 = digits.replace('_', "").parse().map_err(|_| invalid())?;
    Ok(if negative { -value } else { value })
}

type Callback = Arc<dyn Fn() + Send + Sync>;

/// The attributes of a [`Transfer`].
#[derive(Debug, Clone, PartialEq)]
pub struct TransferData {
    pub url: Option<String>,
    pub to_print: Option<bool>,
    pub to_select: Option<bool>,
    pub start_cmd_id: Option<i64>,
    pub path: Option<String>,
    pub size: Option<i64>,
    pub hash: Option<String>,
    pub team_id: Option<Value>,
    /// Set by `DownloadMgr.start`; unset (`AttributeError` when read) until the first one.
    pub os_path: Option<String>,
    pub transfer_id: Option<u32>,
    pub type_: TransferType,
    transferred: i64,
    /// start_ts is deprecated, because it uses system time, instead of tics
    pub start_ts: f64,
    pub stop_ts: f64,
    pub start_time: Option<f64>,
}

/// File transfer representation object
pub struct Transfer {
    lock: Mutex<()>,
    data: Mutex<TransferData>,
    started_cb: Mutex<Callback>,
    progress_cb: Mutex<Callback>,
    stopped_cb: Mutex<Callback>,
}

impl Default for Transfer {
    fn default() -> Transfer {
        Transfer::new()
    }
}

impl Transfer {
    pub fn new() -> Transfer {
        let noop: Callback = Arc::new(|| {});
        Transfer {
            lock: Mutex::new(()),
            data: Mutex::new(TransferData {
                url: None,
                to_print: None,
                to_select: None,
                start_cmd_id: None,
                path: None,
                size: None,
                hash: None,
                team_id: None,
                os_path: None,
                transfer_id: None,
                type_: TransferType::NoTransfer,
                transferred: 0,
                start_ts: 0.0,
                stop_ts: 0.0,
                start_time: None,
            }),
            started_cb: Mutex::new(noop.clone()),
            progress_cb: Mutex::new(noop.clone()),
            stopped_cb: Mutex::new(noop),
        }
    }

    /// The attributes, to read or set.
    pub fn data(&self) -> MutexGuard<'_, TransferData> {
        self.data.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn set_started_cb(&self, cb: Callback) {
        *self.started_cb.lock().unwrap() = cb;
    }

    pub fn set_progress_cb(&self, cb: Callback) {
        *self.progress_cb.lock().unwrap() = cb;
    }

    pub fn set_stopped_cb(&self, cb: Callback) {
        *self.stopped_cb.lock().unwrap() = cb;
    }

    /// Returns the number of bytes already transferred
    pub fn transferred(&self) -> i64 {
        self.data().transferred
    }

    /// Sets the number of bytes transferred and calls back, so UI can update and whatnot
    pub fn set_transferred(&self, transferred: i64) {
        self.data().transferred = transferred;
        let cb = self.progress_cb.lock().unwrap().clone();
        cb();
    }

    /// Return True if any transfer is in progress
    pub fn in_progress(&self) -> bool {
        self.data().type_ != TransferType::NoTransfer
    }

    /// Set a new transfer type, if no transfer is in progress. `Err(None)` is the
    /// `TransferRunningError`.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        type_: TransferType,
        path: &str,
        url: Option<&str>,
        to_print: Option<bool>,
        to_select: Option<bool>,
        start_cmd_id: Option<i64>,
        hash_: Option<&str>,
        team_id: Option<Value>,
    ) -> Result<Finish, Option<Exception>> {
        let filename = pyos::basename(path);

        if forbidden_characters(filename) {
            return Err(Some(forbidden_characters_error()));
        }

        if filename_too_long(filename) {
            return Err(Some(filename_too_long_error()));
        }

        if foldername_too_long(path) {
            return Err(Some(foldername_too_long_error()));
        }

        let _lock = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        if self.in_progress() {
            return Err(None);
        }
        self.reset();

        {
            let mut d = self.data();
            d.start_cmd_id = start_cmd_id;
            d.transfer_id = Some(generate_transfer_id());
            d.start_time = Some(clock::monotonic());
            d.type_ = type_;
            d.path = Some(path.to_owned());
            d.url = url.map(str::to_owned);
            d.to_print = to_print;
            d.to_select = to_select;
            d.hash = hash_.map(str::to_owned);
            d.team_id = team_id;
        }
        let cb = self.started_cb.lock().unwrap().clone();
        cb();

        let kwargs = self.to_dict().map_err(Some)?;
        Ok(Finish {
            source: Source::Wui,
            event: Some(Event::TransferInfo),
            kwargs,
        })
    }

    /// Stop transfer - set the stop timestamp
    pub fn stop(&self) {
        self.data().stop_ts = clock::time();
        let cb = self.stopped_cb.lock().unwrap().clone();
        cb();
    }

    /// Reset transfer data
    pub fn reset(&self) {
        self.data().size = None;
        self.set_transferred(0);
        let mut d = self.data();
        d.start_ts = 0.0;
        d.stop_ts = 0.0;
    }

    /// Calculate current transfer progress
    pub fn progress(&self) -> Result<f64, Exception> {
        let d = self.data();
        match d.size {
            Some(0) => Err(Exception::Other(
                "ZeroDivisionError",
                "division by zero".into(),
            )),
            Some(size) => Ok(d.transferred as f64 / size as f64 * 100.0),
            None => Ok(0.0),
        }
    }

    /// Return elapsed transferring time as a difference of the start time and current time
    /// using monotonic
    pub fn time_transferring(&self) -> i64 {
        let start_time = self.data().start_time.unwrap_or(0.0);
        (clock::monotonic() - start_time) as i64
    }

    /// Return the estimated time remaining for the transfer in seconds. Returns None if not
    /// computation is not possible.
    pub fn time_remaining(&self) -> Result<Option<i64>, Exception> {
        let d = self.data();
        // finished or aborted
        if d.stop_ts > 0.0 {
            return Ok(Some(0));
        }

        // no content-length specified
        let Some(size) = d.size else {
            return Ok(None);
        };

        if d.start_ts > 0.0 {
            let elapsed = clock::time() - d.start_ts;
            if elapsed == 0.0 || d.transferred == 0 {
                return Ok(None); // stands for Infinity
            }
            return Ok(Some(
                (size as f64 / d.transferred as f64 * elapsed - elapsed) as i64,
            ));
        }
        Ok(None)
    }

    /// Serialize a transfer instance.
    pub fn to_dict(&self) -> Result<Kwargs, Exception> {
        let mut out = Kwargs::new();
        if self.in_progress() {
            let time_remaining = self.time_remaining()?;
            let progress = round2(self.progress()?);
            let d = self.data().clone();
            out.insert("transfer_id".into(), d.transfer_id.into());
            out.insert("start_cmd_id".into(), d.start_cmd_id.into());
            out.insert("type".into(), d.type_.value().into());
            out.insert("path".into(), d.path.into());
            out.insert("url".into(), d.url.into());
            out.insert("size".into(), d.size.into());
            out.insert("progress".into(), progress.into());
            out.insert("transferred".into(), d.transferred.into());
            out.insert("time_remaining".into(), time_remaining.into());
            out.insert("time_transferring".into(), self.time_transferring().into());
            out.insert("to_print".into(), d.to_print.into());
            return Ok(out);
        }
        out.insert("type".into(), self.data().type_.value().into());
        Ok(out)
    }
}

/// What `DownloadMgr` reads of `files.Filesystem` (ours, see the module doc): its `sep` and
/// each storage's `path_storage` by its name.
#[derive(Debug, Clone, Default)]
pub struct Filesystem {
    pub sep: String,
    pub storage_dict: HashMap<String, Storage>,
}

/// What `DownloadMgr` reads of `files.Storage`.
#[derive(Debug, Clone, Default)]
pub struct Storage {
    pub path_storage: String,
}

/// `conn_details_cb()`: `(server, headers)`; a header of value `None` is not sent.
pub type ConnDetailsCallback =
    Arc<dyn Fn() -> (Option<String>, Vec<(String, Option<String>)>) + Send + Sync>;
/// `printed_file_cb()`: the path of the file being printed.
pub type PrintedFileCallback = Arc<dyn Fn() -> Option<String> + Send + Sync>;
/// `download_finished_cb(transfer)`.
pub type DownloadFinishedCallback = Arc<dyn Fn(&Transfer) + Send + Sync>;

/// Download manager.
pub struct DownloadMgr {
    pub buffer_size: usize,
    /// seconds
    pub throttle: f64,
    pub fs: Mutex<Filesystem>,
    conn_details_cb: ConnDetailsCallback,
    event_cb: EventCallback,
    printed_file_cb: Mutex<PrintedFileCallback>,
    running_loop: AtomicBool,
    pub transfer: Arc<Transfer>,
    download_finished_cb: Mutex<DownloadFinishedCallback>,
    session: Session,
}

impl DownloadMgr {
    pub const LOOP_INTERVAL: f64 = 0.1;
    pub const VALID_MIME_TYPES: [&'static str; 4] = [
        "text/plain",
        "text/x.gcode",
        "application/binary",
        "application/octet-stream",
    ];
    pub const SMALL_BUFFER: usize = 1024;
    pub const BIG_BUFFER: usize = 1024 * 100;

    pub fn new(
        fs: Filesystem,
        transfer: Arc<Transfer>,
        conn_details_cb: ConnDetailsCallback,
        event_cb: EventCallback,
        printed_file_cb: PrintedFileCallback,
        download_finished_cb: DownloadFinishedCallback,
    ) -> DownloadMgr {
        DownloadMgr {
            buffer_size: Self::BIG_BUFFER,
            throttle: 0.0,
            fs: Mutex::new(fs),
            conn_details_cb,
            event_cb,
            printed_file_cb: Mutex::new(printed_file_cb),
            running_loop: AtomicBool::new(false),
            transfer,
            download_finished_cb: Mutex::new(download_finished_cb),
            session: Session::new(),
        }
    }

    pub fn set_printed_file_cb(&self, cb: PrintedFileCallback) {
        *self.printed_file_cb.lock().unwrap() = cb;
    }

    pub fn set_download_finished_cb(&self, cb: DownloadFinishedCallback) {
        *self.download_finished_cb.lock().unwrap() = cb;
    }

    fn event(&self, event: Event, source: Source, kwargs: Kwargs) {
        (self.event_cb)(event, source, None, None, kwargs);
    }

    /// Start a download of `url` saving it into the `path`.
    ///
    /// This `path` is the absolute virtual path in `self.fs`
    /// (:class:prusa.connect.printer.files.Filesystem)
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        type_: TransferType,
        path: &str,
        url: Option<&str>,
        to_print: Option<bool>,
        to_select: Option<bool>,
        start_cmd_id: Option<i64>,
        hash_: Option<&str>,
        team_id: Option<Value>,
    ) -> Result<Finish, Exception> {
        // Check if no other transfer is running
        let retval = match self.transfer.start(
            type_,
            path,
            url,
            to_print,
            to_select,
            start_cmd_id,
            hash_,
            team_id,
        ) {
            Ok(retval) => retval,
            Err(None) => {
                let mut kwargs = Kwargs::new();
                kwargs.insert("reason".into(), "Another transfer in progress".into());
                return Ok(Finish {
                    source: Source::Connect,
                    event: Some(Event::Rejected),
                    kwargs,
                });
            }
            Err(Some(err)) => return Err(err),
        };

        log::info!("Starting download: {}", url.unwrap_or("None"));

        // transform destination to OS path and validate
        let os_path = self.to_os_path(path)?;
        self.transfer.data().os_path = Some(os_path.clone());
        // make dir (in case filename contains a subdir)
        let dir_ = pyos::dirname(&os_path);
        match pyos::makedirs(dir_.as_bytes(), false) {
            Err(pyos::Error::Os(err)) if err.errno == Some(libc::EEXIST) => {
                log::debug!("{dir_} already exists");
            }
            other => other?,
        }

        Ok(retval)
    }

    /// Translate virtual `destination` of self.fs to real OS path.
    pub fn to_os_path(&self, path: &str) -> Result<String, Exception> {
        if !pyos::isabs(path) {
            return Err(Exception::Value("Destination must be absolute".into()));
        }
        let fs = self.fs.lock().unwrap();
        let sep = fs.sep.as_str();
        let parts: Vec<&str> = path.splitn(3, sep).collect();
        let [_, storage_name, rest] = parts[..] else {
            return Err(Exception::Value(format!(
                "not enough values to unpack (expected 3, got {})",
                parts.len()
            )));
        };
        let Some(storage) = fs.storage_dict.get(storage_name) else {
            return Err(Exception::Value(format!(
                "Invalid storage: `{storage_name}` in `{path}`"
            )));
        };
        if storage.path_storage.is_empty() {
            return Err(Exception::Value(
                "Storage does not have path_storage.".into(),
            ));
        }
        let path_storage = storage.path_storage.trim_end_matches(|c| sep.contains(c));
        let os_path_ = [path_storage, rest].join(sep);
        let os_path_ = String::from_utf8_lossy(&pyos::normpath(os_path_.as_bytes())).into_owned();
        if !os_path_.starts_with(path_storage) {
            return Err(Exception::Value(format!(
                "Destination is outside of defined path_storage for storage: {storage_name}"
            )));
        }
        Ok(os_path_)
    }

    /// One download of the loop: its `try` block, `Ok` when it ended without an exception.
    fn loop_step(&self) -> Result<(), Exception> {
        self.download()?;
        let os_path = self.os_path()?;
        let abs_fn = String::from_utf8_lossy(&pyos::abspath(os_path.as_bytes())?).into_owned();
        let (stop_ts, path, transfer_id, start_cmd_id) = {
            let d = self.transfer.data();
            (d.stop_ts, d.path.clone(), d.transfer_id, d.start_cmd_id)
        };
        if stop_ts != 0.0 {
            // download was stopped
            let tmp_fn = self.tmp_filename()?;
            if pyos::exists(tmp_fn.as_bytes()) {
                pyos::remove(tmp_fn.as_bytes())?;
            }
        } else {
            let printed = self.printed_file_cb.lock().unwrap().clone();
            if printed().as_deref() != Some(abs_fn.as_str()) {
                pyos::rename(self.tmp_filename()?.as_bytes(), abs_fn.as_bytes())?;
            } else {
                let msg = "Gcode being printed would beoverwritten by downloaded file -> aborting.";
                let mut kwargs = Kwargs::new();
                kwargs.insert("reason".into(), msg.into());
                kwargs.insert("path".into(), path.clone().into());
                kwargs.insert("transfer_id".into(), transfer_id.into());
                kwargs.insert("start_cmd_id".into(), start_cmd_id.into());
                self.event(Event::TransferAborted, Source::Connect, kwargs);
            }
        }

        let mut kwargs = Kwargs::new();
        kwargs.insert("start_cmd_id".into(), start_cmd_id.into());
        kwargs.insert("path".into(), path.into());
        kwargs.insert("transfer_id".into(), transfer_id.into());
        self.event(Event::TransferFinished, Source::Connect, kwargs);
        let finished = self.download_finished_cb.lock().unwrap().clone();
        finished(&self.transfer);
        Ok(())
    }

    /// Infinite download loop
    pub fn run_loop(&self) {
        self.running_loop.store(true, Ordering::SeqCst);
        while self.running_loop.load(Ordering::SeqCst) {
            if DOWNLOAD_TYPES.contains(&self.transfer.data().type_) {
                if let Err(err) = self.loop_step() {
                    let (path, transfer_id, start_cmd_id) = {
                        let d = self.transfer.data();
                        (d.path.clone(), d.transfer_id, d.start_cmd_id)
                    };
                    let mut kwargs = Kwargs::new();
                    if err.is_transfer_stopped() {
                        kwargs.insert("path".into(), path.into());
                        kwargs.insert("transfer_id".into(), transfer_id.into());
                        kwargs.insert("start_cmd_id".into(), start_cmd_id.into());
                        self.event(Event::TransferStopped, Source::Connect, kwargs);
                    } else {
                        log::error!("{}", err.str());
                        kwargs.insert("reason".into(), err.str().into());
                        kwargs.insert("path".into(), path.into());
                        kwargs.insert("transfer_id".into(), transfer_id.into());
                        kwargs.insert("start_cmd_id".into(), start_cmd_id.into());
                        self.event(Event::TransferAborted, Source::Connect, kwargs);
                    }
                }
                // End of transfer - reset transfer data
                self.transfer.data().type_ = TransferType::NoTransfer;
            }

            std::thread::sleep(Duration::from_secs_f64(Self::LOOP_INTERVAL));
        }
    }

    /// Set internal variable to stop the download loop.
    pub fn stop_loop(&self) {
        self.running_loop.store(false, Ordering::SeqCst);
    }

    /// Returns important info of Download Manager
    pub fn info(&self) -> Result<Kwargs, Exception> {
        self.transfer.to_dict()
    }

    /// Execute the download and store the file in `self.tmp_filename()`
    pub fn download(&self) -> Result<(), Exception> {
        self.transfer.data().start_ts = clock::time();
        let (server, mut headers) = (self.conn_details_cb)();
        let url = self.transfer.data().url.clone();
        // `self.transfer.url.lower()` of `None`
        let Some(url) = url else {
            return Err(Exception::Other(
                "AttributeError",
                "'NoneType' object has no attribute 'lower'".into(),
            ));
        };

        // server is not connect server, set token to None
        let is_server = server
            .as_deref()
            .is_some_and(|s| !s.is_empty() && url.to_lowercase().starts_with(&s.to_lowercase()));
        if !is_server {
            let user_agent = headers
                .iter()
                .find(|(k, _)| k == "User-Agent")
                .and_then(|(_, v)| v.clone());
            headers = vec![("User-Agent".to_owned(), user_agent)];
        }
        let headers: Vec<(String, String)> = headers
            .into_iter()
            .filter_map(|(k, v)| Some((k, v?)))
            .collect();

        let mut res = self.session.get_stream(&url, &headers)?;

        if res.status_code != 200 {
            return Err(Exception::transfer_aborted(format!(
                "Invalid status code: {}",
                res.status_code
            )));
        }
        let mime_type = res.header("Content-Type").map(str::to_owned);

        if let Some(mime_type) = mime_type.filter(|m| !m.is_empty())
            && !Self::VALID_MIME_TYPES.contains(&mime_type.to_lowercase().as_str())
        {
            return Err(Exception::transfer_aborted(format!(
                "Invalid content type: {mime_type}"
            )));
        }
        let size = match res.header("Content-Length") {
            Some(size) => Some(py_int(size)?),
            None => None,
        };
        self.transfer.data().size = size;

        let tmp_filename = self.tmp_filename()?;
        log::debug!("Save download to: {tmp_filename} ({url})");

        let mut f = std::fs::File::create(&tmp_filename).map_err(|err| {
            Exception::Os(pyos::OsError::from_io_with_filename(
                &err,
                tmp_filename.as_bytes(),
            ))
        })?;
        let os = |err: std::io::Error| Exception::Os(pyos::OsError::from_io(&err));
        self.event(Event::TransferInfo, Source::Wui, self.info()?);
        while let Some(data) = res.next_chunk(self.buffer_size) {
            let data = data?;
            if self.transfer.data().stop_ts > 0.0 {
                return Err(Exception::Other(
                    "TransferStoppedError",
                    "Transfer was stopped".into(),
                ));
            }
            if !self.running_loop.load(Ordering::SeqCst) {
                return Err(Exception::transfer_aborted("Transfer was aborted"));
            }
            std::io::Write::write_all(&mut f, &data).map_err(os)?;
            if self.throttle != 0.0 {
                std::thread::sleep(Duration::from_secs_f64(self.throttle));
            }
            self.transfer
                .set_transferred(self.transfer.transferred() + data.len() as i64);
        }
        std::io::Write::flush(&mut f).map_err(os)?;
        f.sync_all().map_err(os)?;
        drop(f);

        if self.transfer.transferred() == 0 {
            return Err(Exception::transfer_aborted("Empty response"));
        }
        Ok(())
    }

    /// `self.transfer.os_path`: an `AttributeError` before the first `start` set it.
    fn os_path(&self) -> Result<String, Exception> {
        self.transfer.data().os_path.clone().ok_or_else(|| {
            Exception::Other(
                "AttributeError",
                "'Transfer' object has no attribute 'os_path'".into(),
            )
        })
    }

    /// Generate a temporary filename for download based on `self.destination`
    pub fn tmp_filename(&self) -> Result<String, Exception> {
        let os_path = self.os_path()?;
        let path = self.transfer.data().path.clone().unwrap_or_default();
        let dir_ = pyos::dirname(&os_path);
        let base = pyos::basename(&path);
        let joined = pyos::join(dir_.as_bytes(), &[format!(".{base}.part").as_bytes()]);
        Ok(String::from_utf8_lossy(&pyos::abspath(&joined)?).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `float("%.2f" % x)` on CPython 3.13: exact ties round to even, others to nearest.
    #[test]
    fn round2_as_python() {
        for (x, want) in [
            (0.125, 0.12),
            (0.375, 0.38),
            (2.675, 2.67),
            (1.005, 1.0),
            (33.333333, 33.33),
            (100.0, 100.0),
            (0.0, 0.0),
        ] {
            assert_eq!(round2(x), want, "{x}");
        }
    }

    #[test]
    fn int_as_python() {
        assert_eq!(py_int(" 12 "), Ok(12));
        assert_eq!(py_int("1_000"), Ok(1000));
        assert_eq!(py_int("-3"), Ok(-3));
        assert_eq!(
            py_int("1__0").unwrap_err().str(),
            "invalid literal for int() with base 10: '1__0'"
        );
        assert_eq!(
            py_int("").unwrap_err().str(),
            "invalid literal for int() with base 10: ''"
        );
    }
}
