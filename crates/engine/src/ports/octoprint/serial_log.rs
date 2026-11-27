//! OctoPrint's `serial.log`: the logger `SERIAL` (comm.py:504 `self._serialLogger`), its handler
//! `serialFile` and formatter `serial` (octoprint/__init__.py:290, :318-326, :378-382), and the handler
//! class `SerialLogHandler` = `TriggeredRolloverLogHandler` (logging/handlers.py:66-131), of
//! OctoPrint 1.11.8 (42be740).
//!
//! - The logger's level is INFO (`__init__.py:379`); with the setting `serial.log` it is DEBUG
//!   (server/__init__.py:314-316). Every `Send:`/`Recv:` line of comm's `_log` is a DEBUG record,
//!   so the file holds them only with `serial.log` on. The INFO notice of `connect`
//!   (printer/standard.py:409-413) is written either way.
//! - The file is `<logs>/serial.log`, formatted `%(asctime)s - %(message)s`, opened on the first
//!   record (`delay: True`), appended to (`FileHandler` mode `a`), UTF-8. `connect` arms a rollover
//!   (standard.py:407 `SerialLogHandler.arm_rollover()`): the next record first moves the file to
//!   `serial.log.<its mtime as %Y-%m-%d_%H-%M-%S>` and keeps the 3 newest of those (`backupCount: 3`).
//! - As in Python: `arm_rollover` sets the CLASS attribute, `doRollover` the INSTANCE attribute
//!   `do_rollover = False` (handlers.py:112), which hides the class attribute from then on. So the
//!   file rolls over at most once per process, on the first record after the first `connect`.
//!
//! Ours: OctoPrint's base folder is not ported (settings.rs: no basedir), so the agent names the logs
//! folder ([`set_logs_folder`]); without it nothing is written. OctoPrint writes through a one-thread
//! executor (`AsyncLogHandlerMixin`), in the order of the records; ours writes in the caller's thread,
//! in the same order. `handleError` (a write that fails) prints to stderr and goes on, as here.

use super::files::pytime_localtime;
use super::settings::settings;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// `backupCount` of the handler `serialFile` (`__init__.py:322`).
const BACKUP_COUNT: usize = 3;

/// `TriggeredRolloverLogHandler.do_rollover`, the class attribute (handlers.py:69, :73-75).
static CLASS_DO_ROLLOVER: AtomicBool = AtomicBool::new(false);

struct Handler {
    /// `self.baseFilename`; `None` until the agent names the folder.
    base_filename: Option<PathBuf>,
    /// `self.stream`; `None` until the first record (`delay: True`) and after a rollover.
    stream: Option<File>,
    /// The instance attribute `do_rollover`, once `doRollover` has set it (handlers.py:112).
    instance_do_rollover: Option<bool>,
}

static HANDLER: Mutex<Handler> = Mutex::new(Handler {
    base_filename: None,
    stream: None,
    instance_do_rollover: None,
});

fn handler() -> std::sync::MutexGuard<'static, Handler> {
    HANDLER.lock().unwrap_or_else(|e| e.into_inner())
}

/// Ours: the folder OctoPrint calls `settings.getBaseFolder("logs")` (created, as `getBaseFolder`
/// creates it). The handler is made here as `logging.config.dictConfig` makes it: `__init__` runs
/// `cleanupFiles` (handlers.py:77-80).
pub fn set_logs_folder(folder: &Path) {
    if let Err(error) = std::fs::create_dir_all(folder) {
        eprintln!("serial.log: {error}");
        return;
    }
    let mut h = handler();
    h.stream = None;
    h.instance_do_rollover = None;
    h.base_filename = Some(folder.join("serial.log"));
    cleanup_files(&h);
}

/// `SerialLogHandler.arm_rollover()` (handlers.py:73-75).
pub fn arm_rollover() {
    CLASS_DO_ROLLOVER.store(true, Ordering::SeqCst);
}

/// `logging.getLogger("SERIAL").isEnabledFor(logging.DEBUG)`: DEBUG with the setting `serial.log`
/// (server/__init__.py:314-316), INFO otherwise.
pub fn debug_enabled() -> bool {
    settings().get_boolean(&["serial", "log"]) == Some(true)
}

/// `self._serialLogger.debug(message)`.
pub fn debug(message: &str) {
    if debug_enabled() {
        emit(message);
    }
}

/// `logging.getLogger("SERIAL").info(message)`: the logger's level is INFO at least.
pub fn info(message: &str) {
    emit(message);
}

/// `RotatingFileHandler.emit`: `shouldRollover`, `doRollover`, then `FileHandler.emit` (opens the file
/// on the first record, writes the formatted record and the terminator, flushes).
fn emit(message: &str) {
    let mut h = handler();
    let Some(base) = h.base_filename.clone() else {
        return;
    };
    // handlers.py:82-83 `shouldRollover`: `self.do_rollover`, the instance attribute once set
    let should = h
        .instance_do_rollover
        .unwrap_or_else(|| CLASS_DO_ROLLOVER.load(Ordering::SeqCst));
    if should {
        do_rollover(&mut h, &base);
    }
    if h.stream.is_none() {
        match OpenOptions::new().append(true).create(true).open(&base) {
            Ok(file) => h.stream = Some(file),
            Err(error) => {
                eprintln!("serial.log: {error}");
                return;
            }
        }
    }
    let line = format!("{} - {message}\n", asctime(SystemTime::now()));
    if let Some(stream) = h.stream.as_mut()
        && let Err(error) = stream
            .write_all(line.as_bytes())
            .and_then(|()| stream.flush())
    {
        eprintln!("serial.log: {error}");
    }
}

/// handlers.py:111-128 `doRollover` (`delay` is true: the stream is not opened again here).
fn do_rollover(h: &mut Handler, base: &Path) {
    h.instance_do_rollover = Some(false);
    h.stream = None;
    if let Ok(meta) = std::fs::metadata(base) {
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        // time.strftime("%Y-%m-%d_%H-%M-%S", time.localtime(mtime))
        let suffix = match pytime_localtime(mtime) {
            Ok(t) => format!(
                "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
                t.year,
                t.mon + 1,
                t.mday,
                t.hour,
                t.min,
                t.sec
            ),
            Err(error) => {
                eprintln!("serial.log: {error}");
                return;
            }
        };
        let mut dfn = base.as_os_str().to_owned();
        dfn.push(format!(".{suffix}"));
        let dfn = PathBuf::from(dfn);
        if dfn.exists() {
            let _ = std::fs::remove_file(&dfn);
        }
        if let Err(error) = std::fs::rename(base, &dfn) {
            eprintln!("serial.log: {error}");
        }
    }
    cleanup_files(h);
}

/// handlers.py:85-104 `getFilesToDelete` and :106-109 `cleanupFiles`.
fn cleanup_files(h: &Handler) {
    let Some(base) = h.base_filename.as_ref() else {
        return;
    };
    let (Some(dir), Some(name)) = (base.parent(), base.file_name()) else {
        return;
    };
    let prefix = format!("{}.", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut result: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            let suffix = file.strip_prefix(&prefix)?;
            file_pattern(suffix).then(|| dir.join(&file))
        })
        .collect();
    result.sort();
    if result.len() < BACKUP_COUNT {
        return;
    }
    for path in &result[..result.len() - BACKUP_COUNT] {
        let _ = std::fs::remove_file(path);
    }
}

/// handlers.py:71 `file_pattern`: `^\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}$`. Differs: `\d` of a Python
/// `str` pattern is any Unicode decimal digit, here only 0-9 (the names `doRollover` writes are ASCII).
fn file_pattern(suffix: &str) -> bool {
    let shape = "dddd-dd-dd_dd-dd-dd";
    suffix.len() == shape.len()
        && shape.bytes().zip(suffix.bytes()).all(|(s, c)| match s {
            b'd' => c.is_ascii_digit(),
            other => c == other,
        })
}

/// `logging.Formatter.formatTime` without `datefmt`: `time.strftime("%Y-%m-%d %H:%M:%S",
/// time.localtime(record.created))` and `",%03d" % record.msecs`.
fn asctime(now: SystemTime) -> String {
    let d = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let msecs = d.subsec_millis();
    match pytime_localtime(secs) {
        Ok(t) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02},{msecs:03}",
            t.year,
            t.mon + 1,
            t.mday,
            t.hour,
            t.min,
            t.sec
        ),
        Err(_) => format!("{secs},{msecs:03}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// handlers.py:71: the rolled-over names, and nothing else.
    #[test]
    fn the_rollover_names() {
        assert!(file_pattern("2026-11-27_10-55-39"));
        assert!(!file_pattern("2026-11-27_10-55-3"));
        assert!(!file_pattern("2026-11-27 10-55-39"));
        assert!(!file_pattern("1"));
    }
}
