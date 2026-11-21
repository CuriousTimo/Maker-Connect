//! Port of the parts of OctoPrint 1.11.8 `src/octoprint/printer/standard.py` that drive
//! `util/comm.py`, and of the constants and exceptions of `printer/__init__.py` it uses.
//!
//! Original: Copyright (C) 2014 The OctoPrint Project, AGPL-3.0
//! (`__author__ = "Gina Häußge <osd@foosel.net>"`).
//!
//! # Ported (source line per item)
//!
//! `Printer.__init__` (:52) as far as the ported parts need it (the fields below), `connect` (:380),
//! `disconnect` (:424), `commands` (:463), `script` (:480), `jog` (:510), `home` (:553), `extrude`
//! (:575), `change_tool` (:596), `set_temperature` (:606), `_convert_rate_value` (:668), `feed_rate`
//! (:682), `flow_rate` (:689), `select_file` (:696), `unselect_file` (:744), `start_print` (:765),
//! `pause_print` (:792), `resume_print` (:808), `cancel_print` (:824), the state getters
//! `get_state_string` (:843), `get_state_id` (:849), `get_error` (:855), `get_current_connection`
//! (:891), `is_closed_or_error` ... `is_sd_ready` (:899-940), the SD wrappers `get_sd_files` (:942),
//! `add_sd_file` (:954), `_get_free_remote_name` (:1020), `_add_sd_file` (:1038), `delete_sd_file`
//! (:1052), `init_sd_card` (:1060), `release_sd_card` (:1067), `refresh_sd_files` (:1074), `_setState`
//! (:1097), `_validateJob` (:1236), `_setJobData` (:1257, the `_selectedFile` part), `_updateJobUser`
//! (:1383), `_payload_for_print_job_event` (:1918), `error_info` (:200), `firmware_info` (:196), and
//! the `MachineComPrintCallback` methods that act on the wire or on `Printer`'s own state:
//! `on_comm_position_update` (:1459), `on_comm_state_change` (:1464), `on_comm_error` (:1543),
//! `on_comm_z_change` (:1564), `on_comm_sd_files` (:1585), `on_comm_file_selected` (:1588),
//! `on_comm_print_job_started` (:1626), `_done` (:1658), `_cancelling` (:1723), `_cancelled` (:1730),
//! `_paused` (:1788), `_resumed` (:1823), `on_comm_file_transfer_started` (:1848), `_done` (:1868),
//! `_failed` (:1899), `on_comm_force_disconnect` (:1904), `on_comm_record_fileposition` (:1907),
//! `on_comm_firmware_info` (:1915). From `printer/__init__.py`: `valid_axes` (:43), `valid_tool_regex`
//! (:46), `valid_heater_regex` (:49), `UnknownScript` (:734), `InvalidFileLocation` (:739),
//! `InvalidFileType` (:743).
//!
//! # Not ported, and why
//!
//! * Callbacks left out (UI only, they only feed `StateMonitor`, the log and message buffers or the
//!   temperature history that the web page shows): `on_comm_log` (:1443), `on_comm_temperature_update`
//!   (:1449), `on_comm_message` (:1549), `on_comm_progress` (:1556), `on_comm_sd_state_change` (:1576).
//!   They only reach the port's top edge (`Printer::set_top_edge`, ours, `port.rs`); the other
//!   callbacks that are ported reach it as well, before `Printer`'s own handling.
//! * Not wire: `StateMonitor` (:1984), `DataHistory` (:2143), `_updateProgressData` (:1122),
//!   `_updateProgressDataCallback` (:1140), `_updateResendDataCallback` (:1205), `_addTemperatureData`
//!   (:1214), `_setOffsets` (:1090), `_addLog` (:1114), `_addMessage` (:1118), `_getStateFlags` (:1426),
//!   `_sendInitialStateUpdate` (:1403), the callback registry (`register_callback` :206,
//!   `unregister_callback` :213, `send_initial_callback` :220, `_sendAdd*Callbacks` :224-275,
//!   `_get_additional_plugin_data` :276), `get_current_data` (:861), `get_current_job` (:864),
//!   `get_current_temperatures` (:868), `get_temperature_history` (:888), `get_markings` (:762), the
//!   `_on_event_*` handlers (:305-346: they only refresh the job data for the web page and add chart
//!   markings), `_reportPrintProgressToPlugins` (:350), `_create_estimator` (:183) and the print time
//!   estimators (`printer/estimation.py`), `log_lines` (:837), the analysis queue calls of
//!   `on_comm_state_change`, `get_file_position` (:752), `get_transport` (:435), `job_on_hold` (:447),
//!   `set_job_on_hold` (:452), `fake_ack` (:457) (not in the plan's list), `set_temperature_offset`
//!   (:640, decided: not ported).
//! * `_setJobData`: only what sets `self._selectedFile` (the metadata, estimated times, display name and
//!   date go to `StateMonitor` only). A file's analysis metadata is none (no file manager), so
//!   `estimatedPrintTime` stays `None`, as it does in Python for a file without metadata.
//!
//! # Ours (each also listed in the section J report)
//!
//! 1. Arguments Python takes dynamically are `serde_json::Value` where the Python code does
//!    `isinstance` checks (`jog`, `home`, `extrude`, `set_temperature`, the rates, `commands`), so each
//!    check and its `ValueError` is the Python one; `bool` is an `int` there, as in Python. A Python
//!    exception that a caller catches (`ValueError`, `UnknownScript`, `InvalidFileType`,
//!    `InvalidFileLocation`, `OSError`) is `Err(PrinterError)`; any other exception is a panic
//!    (comm.rs, difference 3).
//! 2. `kwargs.get("tags", set())` is `tags: Option<BTreeSet<String>>`, `None` meaning the key is absent.
//!    `home` has Python's own `kwargs.get("tags", set)` (the type `set`, not an empty set): without
//!    `tags`, `set | {...}` is a `TypeError`, returned as `Err(PrinterError::TypeError)` after the
//!    axes were validated (:571).
//! 3. The file manager is the trait `FileManager`, with `LocalFileManager`: the job's file is a local path
//!    (`docs/plans/agent-engine.md`, "Not wire"), so `path_on_disk` and `path_in_storage` are the path itself,
//!    `split_path` splits at the last `/`, there is no recovery data and `log_print` does nothing.
//! 4. `Printer` is created with `Printer::new(..) -> Arc<Printer>` (a `Weak` of itself lets `connect` and
//!    the finalize threads hand `Arc<Printer>` to the comm layer), and `MachineCom` gets `Arc<Printer>` as
//!    its callback object (Python: `callbackObject=self`).
//! 5. Hooks (`octoprint.printer.handle_connect`, `octoprint.printer.sdcardupload`) are called with JSON
//!    arguments and without the printer instance (comm.rs, difference 31); `sdcardupload` hooks need
//!    callables as arguments, so a registered one is `unimplemented!` (no plugin registers one).
//! 6. `Printer.__init__`'s `StateMonitor`, `DataHistory`, deques, estimator, callback list and
//!    progress plugins are not created (see above); `_selectedFile` is a `SelectedFile` struct, guarded by
//!    `_selectedFileMutex` (an `RLock`) as in Python.
//! 7. `_payload_for_print_job_event` and the events' payloads are `serde_json` objects in Python's key
//!    order; a float `time` or `progress` of `None` is `null`.
//! 8. `on_comm_file_transfer_done(.., failed)` is `on_comm_file_transfer_done_failed`; the trait
//!    method has no `failed` argument (comm.rs trait, comm.py:5749).
//! 9. A threaded `finalize` / `log_print` (daemon threads, :1492, :1699, :1773) is a
//!    `py_threading::Thread` with `daemon = true`.

use super::comm::{MachineCom, MachineComPrintCallback, py_float_repr, py_int, py_truthy, py_try};
use super::comm::{OSError, ValueError};
use super::events::{Events, event_manager};
use super::filemanager::{FileDestinations, valid_file_type};
use super::files::get_dos_filename;
use super::plugin::{Hooks, plugin_manager};
use super::profile::PrinterProfileManager;
use super::py_attr::Attr;
use super::py_collections::PyDict;
use super::py_threading::{RLock, Thread};
use super::settings::settings;
use fancy_regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock, Weak};

// ---------------------------------------------------------------------------------------------
// printer/__init__.py
// ---------------------------------------------------------------------------------------------

/// `printer/__init__.py:43 PrinterInterface.valid_axes`
pub const VALID_AXES: [&str; 4] = ["x", "y", "z", "e"];

/// `printer/__init__.py:46 valid_tool_regex = re.compile(r"^(tool\d+)$")`. `re.match` anchors the start;
/// Python's `$` also matches before a final `\n`, which is `\n?\z` here.
pub static VALID_TOOL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A(tool\d+)\n?\z").expect("valid regex"));

/// `printer/__init__.py:49 valid_heater_regex = re.compile(r"^(tool\d*|bed|chamber)$")`
pub static VALID_HEATER_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A(tool\d*|bed|chamber)\n?\z").expect("valid regex"));

/// What the ported methods raise to their caller (comm.rs, difference 3 for every other exception).
#[derive(Debug, Clone, PartialEq)]
pub enum PrinterError {
    /// `ValueError(message)`
    ValueError(String),
    /// `TypeError(message)`
    TypeError(String),
    /// `OverflowError(message)`
    OverflowError(String),
    /// `printer/__init__.py:734 UnknownScript(name)`
    UnknownScript(String),
    /// `printer/__init__.py:739 InvalidFileLocation`
    InvalidFileLocation(String),
    /// `printer/__init__.py:743 InvalidFileType`
    InvalidFileType(String),
    /// `OSError(message)`
    OSError(String),
}

impl std::fmt::Display for PrinterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrinterError::ValueError(m) => write!(f, "ValueError: {m}"),
            PrinterError::TypeError(m) => write!(f, "TypeError: {m}"),
            PrinterError::OverflowError(m) => write!(f, "OverflowError: {m}"),
            PrinterError::UnknownScript(n) => write!(f, "UnknownScript: {n}"),
            PrinterError::InvalidFileLocation(m) => write!(f, "InvalidFileLocation: {m}"),
            PrinterError::InvalidFileType(m) => write!(f, "InvalidFileType: {m}"),
            PrinterError::OSError(m) => write!(f, "OSError: {m}"),
        }
    }
}

impl From<ValueError> for PrinterError {
    fn from(e: ValueError) -> Self {
        PrinterError::ValueError(e.0.to_string())
    }
}

impl From<OSError> for PrinterError {
    fn from(e: OSError) -> Self {
        PrinterError::OSError(e.0)
    }
}

// ---------------------------------------------------------------------------------------------
// Python value helpers (`str()`, `isinstance(x, (int, float))`, `%d`)
// ---------------------------------------------------------------------------------------------

/// OURS: a Python number: `int` (a `bool` is one too) or `float`
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PyNum {
    Int(i64),
    Float(f64),
}

impl PyNum {
    pub(crate) fn as_f64(self) -> f64 {
        match self {
            PyNum::Int(i) => i as f64,
            PyNum::Float(f) => f,
        }
    }
}

/// `isinstance(v, (int, float))`, with the value
pub(crate) fn py_number(v: &Value) -> Option<PyNum> {
    match v {
        Value::Bool(b) => Some(PyNum::Int(*b as i64)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Some(PyNum::Int(i)),
            None => Some(PyNum::Float(n.as_f64().expect("a JSON number is an f64"))),
        },
        _ => None,
    }
}

/// Python `repr(str)`
fn py_repr_str(s: &str) -> String {
    super::py_format::py_str_repr(s, false)
}

/// Python `repr(value)` of a JSON value
pub(crate) fn py_repr_value(v: &Value) -> String {
    match v {
        Value::String(s) => py_repr_str(s),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(py_repr_value).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", py_repr_str(k), py_repr_value(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => py_str_value(other),
    }
}

/// Python `str(value)` of a JSON value
pub(crate) fn py_str_value(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.to_string(),
            None => match n.as_u64() {
                Some(u) => u.to_string(),
                None => py_float_repr(n.as_f64().expect("a JSON number is an f64")),
            },
        },
        Value::String(s) => s.clone(),
        other => py_repr_value(other),
    }
}

/// `"%d" % value` for an `int`/`float`/`bool`: `int(value)`
pub(crate) fn py_percent_d(n: PyNum) -> i64 {
    match n {
        PyNum::Int(i) => i,
        PyNum::Float(f) => {
            if f.is_nan() {
                panic!("ValueError: cannot convert float NaN to integer")
            }
            if f.is_infinite() || f.abs() >= 9.2e18 {
                panic!("OverflowError: cannot convert float infinity to integer")
            }
            f.trunc() as i64
        }
    }
}

/// `set | {extra}`: `kwargs.get("tags", set()) | {...}`
fn with_trigger(tags: &Option<BTreeSet<String>>, extra: &str) -> BTreeSet<String> {
    let mut t = tags.clone().unwrap_or_default();
    t.insert(extra.to_string());
    t
}

/// Python `min(a, b, ...)` of numbers (the first of equal values); a non-number is the `TypeError` of
/// the comparison
fn py_min_values(values: &[Value]) -> Value {
    let mut best = &values[0];
    for v in &values[1..] {
        let (Some(a), Some(b)) = (py_number(best), py_number(v)) else {
            panic!("TypeError: '<' not supported between instances of {v:?} and {best:?}")
        };
        if b.as_f64() < a.as_f64() {
            best = v;
        }
    }
    best.clone()
}

// ---------------------------------------------------------------------------------------------
// The file manager (OURS, see the module doc, 3)
// ---------------------------------------------------------------------------------------------

/// OURS: what `Printer` asks of `octoprint.filemanager.FileManager` (`path_on_disk`, `path_in_storage`,
/// `split_path`, the recovery data and `log_print`), see the module doc.
pub trait FileManager: Send + Sync {
    /// `FileManager.path_on_disk(destination, path)`
    fn path_on_disk(&self, destination: &str, path: &str) -> String;
    /// `FileManager.path_in_storage(destination, path)`
    fn path_in_storage(&self, destination: &str, path: &str) -> String;
    /// `FileManager.split_path(destination, path)`: `(folder, name)`
    fn split_path(&self, destination: &str, path: &str) -> (String, String);
    /// `FileManager.get_recovery_data()`
    fn get_recovery_data(&self) -> Option<Value>;
    /// `FileManager.delete_recovery_data()`
    fn delete_recovery_data(&self);
    /// `FileManager.save_recovery_data(origin, path, pos)`; `Err(())` is `NoSuchStorage`
    fn save_recovery_data(&self, origin: &str, path: &str, pos: Option<i64>) -> Result<(), ()>;
    /// `FileManager.log_print(destination, path, timestamp, print_time, success, printer_profile)`
    fn log_print(
        &self,
        destination: &str,
        path: &str,
        timestamp: f64,
        print_time: Option<f64>,
        success: bool,
        printer_profile: &str,
    );
}

/// OURS: the file manager for a job whose file is a local path (see the module doc, 3)
pub struct LocalFileManager;

impl FileManager for LocalFileManager {
    fn path_on_disk(&self, _destination: &str, path: &str) -> String {
        path.to_string()
    }
    fn path_in_storage(&self, _destination: &str, path: &str) -> String {
        path.to_string()
    }
    fn split_path(&self, _destination: &str, path: &str) -> (String, String) {
        match path.rsplit_once('/') {
            Some((folder, name)) => (folder.to_string(), name.to_string()),
            None => (String::new(), path.to_string()),
        }
    }
    fn get_recovery_data(&self) -> Option<Value> {
        None
    }
    fn delete_recovery_data(&self) {}
    fn save_recovery_data(&self, _origin: &str, _path: &str, _pos: Option<i64>) -> Result<(), ()> {
        Ok(())
    }
    fn log_print(&self, _d: &str, _p: &str, _t: f64, _pt: Option<f64>, _s: bool, _pr: &str) {}
}

// ---------------------------------------------------------------------------------------------
// printer/standard.py
// ---------------------------------------------------------------------------------------------

/// `standard.py:38 ErrorInformation`
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorInformation {
    pub error: String,
    pub reason: String,
    pub consequence: Option<String>,
    pub faq: Option<String>,
    pub logs: Option<Vec<String>>,
}

impl ErrorInformation {
    /// `model_dump()`, in the model's field order
    pub fn model_dump(&self) -> Value {
        json!({
            "error": self.error,
            "reason": self.reason,
            "consequence": self.consequence,
            "faq": self.faq,
            "logs": self.logs,
        })
    }
}

/// `self._selectedFile` (:1257 `_setJobData`): `{"filename", "filesize", "sd", "estimatedPrintTime",
/// "user"}`; `estimatedPrintTime` is always `None` here (module doc)
#[derive(Debug, Clone, PartialEq)]
pub struct SelectedFile {
    pub filename: String,
    pub filesize: Option<i64>,
    pub sd: bool,
    pub user: Option<String>,
}

/// `self._streamingFinishedCallback` / `_streamingFailedCallback`: `callback(remote, remote, FileDestinations.SDCARD)`
pub type StreamingCallback = Arc<dyn Fn(&str, &str, &str) + Send + Sync>;

/// `standard.py:46 Printer`
pub struct Printer {
    /// OURS: this printer as an `Arc` (module doc, 4)
    weak_self: Weak<Printer>,
    /// `self._fileManager`
    _file_manager: Arc<dyn FileManager>,
    /// `self._printerProfileManager`
    _printer_profile_manager: Arc<PrinterProfileManager>,
    /// `self._state`
    _state: Attr<Option<i64>>,
    /// `self._currentZ`
    _current_z: Attr<Option<f64>>,
    /// `self._printAfterSelect`
    _print_after_select: Attr<bool>,
    /// `self._posAfterSelect`
    _pos_after_select: Attr<Option<i64>>,
    /// `self._firmware_info`
    _firmware_info: Attr<Option<Value>>,
    /// `self._error_info`
    _error_info: Attr<Option<ErrorInformation>>,
    /// `self._sdPrinting`
    _sd_printing: Attr<bool>,
    /// `self._sdStreaming`
    _sd_streaming: Attr<bool>,
    /// `self._streamingFinishedCallback`
    _streaming_finished_callback: Attr<Option<StreamingCallback>>,
    /// `self._streamingFailedCallback`
    _streaming_failed_callback: Attr<Option<StreamingCallback>>,
    /// `self._selectedFileMutex`
    _selected_file_mutex: RLock,
    /// `self._selectedFile`
    _selected_file: Attr<Option<SelectedFile>>,
    /// `self.sd_card_upload_hooks`
    sd_card_upload_hooks: Hooks,
    /// `self._comm`
    _comm: Attr<Option<Arc<MachineCom>>>,
    /// `self._handle_connect_hooks`
    _handle_connect_hooks: Hooks,
    /// OURS (top edge): the port's own listener of the `MachineComPrintCallback` calls; this
    /// `Printer`, as comm's callback object, passes each call on to it (`port.rs`)
    _top_edge: Attr<Option<Arc<dyn MachineComPrintCallback>>>,
}

impl Printer {
    /// `standard.py:52 __init__(fileManager, analysisQueue, printerProfileManager)`; what it does with
    /// the analysis queue, the state monitor, the history and the event subscriptions is not ported
    /// (module doc). The hooks are read from the plugin manager here, as Python does.
    pub fn new(
        file_manager: Arc<dyn FileManager>,
        printer_profile_manager: Arc<PrinterProfileManager>,
    ) -> Arc<Printer> {
        Arc::new_cyclic(|weak| Printer {
            weak_self: weak.clone(),
            _file_manager: file_manager,
            _printer_profile_manager: printer_profile_manager,
            _state: Attr::new(None),
            _current_z: Attr::new(None),
            _print_after_select: Attr::new(false),
            _pos_after_select: Attr::new(None),
            _firmware_info: Attr::new(None),
            _error_info: Attr::new(None),
            _sd_printing: Attr::new(false),
            _sd_streaming: Attr::new(false),
            _streaming_finished_callback: Attr::new(None),
            _streaming_failed_callback: Attr::new(None),
            _selected_file_mutex: RLock::new(),
            _selected_file: Attr::new(None),
            // hook card upload
            sd_card_upload_hooks: plugin_manager().get_hooks("octoprint.printer.sdcardupload"),
            // comm
            _comm: Attr::new(None),
            _handle_connect_hooks: plugin_manager().get_hooks("octoprint.printer.handle_connect"),
            _top_edge: Attr::new(None),
        })
    }

    /// OURS: `self._comm`
    pub fn comm(&self) -> Option<Arc<MachineCom>> {
        self._comm.get()
    }

    /// OURS: `printer._comm = comm` (what the reference's `test_script.py` does)
    pub fn set_comm(&self, comm: Option<Arc<MachineCom>>) {
        self._comm.set(comm);
    }

    /// OURS (top edge, module doc): the listener the port's top edge installs; every
    /// `MachineComPrintCallback` call that reaches this `Printer` is passed to it as well.
    pub fn set_top_edge(&self, edge: Arc<dyn MachineComPrintCallback>) {
        self._top_edge.set(Some(edge));
    }

    /// `standard.py:196 firmware_info`
    pub fn firmware_info(&self) -> Option<Value> {
        // frozendict(self._firmware_info) if self._firmware_info else None
        self._firmware_info.get().filter(py_truthy)
    }

    /// `standard.py:200 error_info`
    pub fn error_info(&self) -> Option<Value> {
        self._error_info.get().map(|e| e.model_dump())
    }

    /// `self._printerProfileManager` (OURS: for the top edge)
    pub fn printer_profile_manager(&self) -> &Arc<PrinterProfileManager> {
        &self._printer_profile_manager
    }

    // ~~ PrinterInterface implementation

    /// `standard.py:380 connect(port=None, baudrate=None, profile=None, *args, **kwargs)`
    ///
    /// Connects to the printer. If port and/or baudrate is provided, uses these settings, otherwise
    /// autodetection will be attempted.
    pub fn connect(&self, port: Option<String>, baudrate: Option<i64>, profile: Option<&str>) {
        if self._comm.get().is_some() {
            return;
        }

        for (name, hook) in self._handle_connect_hooks.iter() {
            // OURS (module doc, 5): `hook(self, *args, port=port, baudrate=baudrate, profile=profile, **kwargs)`
            let args = [
                port.clone().map_or(Value::Null, Value::String),
                baudrate.map_or(Value::Null, Value::from),
                profile.map_or(Value::Null, |p| Value::String(p.to_string())),
            ];
            match py_try(|| hook(&args)) {
                Ok(handled) => {
                    if py_truthy(&handled) {
                        // self._logger.info(f"Connect signalled as handled by plugin {name}")
                        let _ = name;
                        return;
                    }
                }
                Err(_) => {
                    // self._logger.exception(f"Exception while handling connect in plugin {name}", ...)
                }
            }
        }

        self._error_info.set(None);

        event_manager().fire(Events::CONNECTING, None);
        self._printer_profile_manager.select(profile);

        // from octoprint.logging.handlers import SerialLogHandler; SerialLogHandler.arm_rollover()
        // logging.getLogger("SERIAL").info("serial.log is currently not enabled, ...")

        self._firmware_info.set(None);
        let callback: Arc<dyn MachineComPrintCallback> = self
            .weak_self
            .upgrade()
            .expect("a Printer is always inside the Arc that Printer::new made");
        let comm = MachineCom::new(
            port,
            baudrate,
            Some(callback),
            Some(self._printer_profile_manager.clone()),
        );
        self._comm.set(Some(comm.clone()));
        comm.start();
    }

    /// `standard.py:424 disconnect(*args, **kwargs)`: closes the connection to the printer.
    pub fn disconnect(&self) {
        event_manager().fire(Events::DISCONNECTING, None);
        if let Some(comm) = self._comm.get() {
            comm.close(false, true, Some(10.0));
        } else {
            event_manager().fire(Events::DISCONNECTED, None);
        }
        self._firmware_info.set(None);
    }

    /// `standard.py:463 commands(commands, tags=None, force=False, *args, **kwargs)`: sends one or more
    /// gcode commands to the printer. `commands` is a `str` or a list/tuple of them; a command that is not a
    /// `str` is `to_unicode`'s `TypeError` (a panic).
    pub fn commands(&self, commands: &Value, tags: Option<BTreeSet<String>>, force: bool) {
        let Some(comm) = self._comm.get() else {
            return;
        };

        let list: Vec<Value> = match commands {
            Value::Array(a) => a.clone(),
            other => vec![other.clone()],
        };

        let mut tags = tags.unwrap_or_default();
        tags.insert("trigger:printer.commands".to_string());

        for command in list {
            let Value::String(command) = command else {
                panic!(
                    "TypeError: decoding to str: need a bytes-like object, {} found",
                    py_repr_value(&command)
                )
            };
            comm.send_command(
                super::comm::QueuedCommand::Command(command),
                None,
                false,
                false,
                force,
                None,
                Some(tags.clone()),
            );
        }
    }

    /// OURS: `commands` for a list of `str` (the callers below build theirs)
    fn commands_list(&self, commands: &[String], tags: BTreeSet<String>) {
        self.commands(
            &Value::Array(commands.iter().cloned().map(Value::String).collect()),
            Some(tags),
            false,
        );
    }

    /// `standard.py:480 script(name, context=None, must_be_set=True, part_of_job=False, *args, **kwargs)`
    pub fn script(
        &self,
        name: &str,
        context: Option<&Value>,
        must_be_set: bool,
        part_of_job: bool,
        tags: Option<BTreeSet<String>>,
    ) -> Result<(), PrinterError> {
        let Some(comm) = self._comm.get() else {
            return Ok(());
        };

        if name.is_empty() {
            return Err(PrinterError::ValueError("name must be set".to_string()));
        }

        // .capitalize() will lowercase all letters but the first
        // this code preserves existing CamelCase
        let mut chars = name.chars();
        let first: String = chars
            .next()
            .expect("name is not empty")
            .to_uppercase()
            .collect();
        let event_name = format!("{first}{}", chars.as_str());

        let event_start = format!("GcodeScript{event_name}Running");
        let payload: Option<Value> = match context {
            Some(Value::Object(m)) => m.get("event").cloned(),
            _ => None,
        };

        event_manager().fire(&event_start, payload.clone());

        let result = comm.send_gcode_script(
            name,
            context,
            Some(with_trigger(&tags, "trigger:printer.script")),
            part_of_job,
        );
        if result.is_empty() && must_be_set {
            return Err(PrinterError::UnknownScript(name.to_string()));
        }

        let event_end = format!("GcodeScript{event_name}Finished");
        event_manager().fire(&event_end, payload);
        Ok(())
    }

    /// `standard.py:510 jog(axes, relative=True, speed=None, *args, **kwargs)`. `axes` is a `str` (the
    /// legacy format, with the amount in `args[0]`) or an object of axis -> amount; `speed` is the JSON
    /// value of the argument.
    pub fn jog(
        &self,
        axes: &Value,
        relative: bool,
        speed: Option<&Value>,
        args: &[Value],
        tags: Option<BTreeSet<String>>,
    ) -> Result<(), PrinterError> {
        let mut axes = axes.clone();
        if let Value::String(axis) = &axes {
            // legacy parameter format, there should be an amount as first anonymous positional arguments too
            let axis = axis.clone();

            if args.is_empty() {
                return Err(PrinterError::ValueError("amount not set".to_string()));
            }
            let amount = &args[0];
            if py_number(amount).is_none() {
                return Err(PrinterError::ValueError(format!(
                    "amount must be a valid number: {}",
                    py_str_value(amount)
                )));
            }

            let mut m = Map::new();
            m.insert(axis, amount.clone());
            axes = Value::Object(m);
        }

        if !py_truthy(&axes) {
            return Err(PrinterError::ValueError(
                "At least one axis to jog must be provided".to_string(),
            ));
        }

        // `for axis in axes`: the keys of a dict, the items of a list
        let axis_items: Vec<Value> = match &axes {
            Value::Object(o) => o.keys().cloned().map(Value::String).collect(),
            Value::Array(a) => a.clone(),
            other => panic!("TypeError: {} is not iterable", py_repr_value(other)),
        };
        for axis in &axis_items {
            let valid = matches!(axis, Value::String(s) if VALID_AXES.contains(&s.as_str()));
            if !valid {
                return Err(PrinterError::ValueError(format!(
                    "Invalid axis {}, valid axes are {}",
                    py_str_value(axis),
                    VALID_AXES.join(", ")
                )));
            }
        }

        let Value::Object(axes_map) = &axes else {
            panic!("AttributeError: 'list' object has no attribute 'items'")
        };

        let mut command = format!(
            "G0 {}",
            axes_map
                .iter()
                .map(|(axis, amt)| format!("{}{}", axis.to_uppercase(), py_str_value(amt)))
                .collect::<Vec<_>>()
                .join(" ")
        );

        let speed: Value = match speed {
            None | Some(Value::Null) => {
                let printer_profile = self._printer_profile_manager.get_current_or_default();
                let speeds: Vec<Value> = axes_map
                    .keys()
                    .map(|axis| printer_profile["axes"][axis.as_str()]["speed"].clone())
                    .collect();
                py_min_values(&speeds)
            }
            Some(v) => v.clone(),
        };

        if py_truthy(&speed) && !speed.is_boolean() {
            command.push_str(&format!(" F{}", py_str_value(&speed)));
        }

        let commands = if relative {
            vec!["G91".to_string(), command, "G90".to_string()]
        } else {
            vec!["G90".to_string(), command]
        };

        self.commands_list(&commands, with_trigger(&tags, "trigger:printer.jog"));
        Ok(())
    }

    /// `standard.py:553 home(axes, *args, **kwargs)`. `axes` is a list or a `str`. Python's own
    /// `kwargs.get("tags", set)` makes a call without `tags` a `TypeError` (module doc, 2).
    pub fn home(&self, axes: &Value, tags: Option<BTreeSet<String>>) -> Result<(), PrinterError> {
        let axes: Vec<Value> = match axes {
            Value::Array(a) => a.clone(),
            Value::String(_) => vec![axes.clone()],
            other => {
                return Err(PrinterError::ValueError(format!(
                    "axes is neither a list nor a string: {}",
                    py_str_value(other)
                )));
            }
        };

        let validated_axes: Vec<String> = axes
            .iter()
            .map(|x| match x {
                Value::String(s) => s.to_lowercase(),
                other => panic!(
                    "AttributeError: {} object has no attribute 'lower'",
                    py_repr_value(other)
                ),
            })
            .filter(|x| VALID_AXES.contains(&x.as_str()))
            .collect();
        if axes.len() != validated_axes.len() {
            return Err(PrinterError::ValueError(format!(
                "axes contains invalid axes: {}",
                py_str_value(&Value::Array(axes))
            )));
        }

        let commands = vec![
            "G91".to_string(),
            format!(
                "G28 {}",
                validated_axes
                    .iter()
                    .map(|x| format!("{}0", x.to_uppercase()))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            "G90".to_string(),
        ];

        let Some(tags) = tags else {
            return Err(PrinterError::TypeError(
                "unsupported operand type(s) for |: 'type' and 'set'".to_string(),
            ));
        };
        self.commands_list(&commands, with_trigger(&Some(tags), "trigger:printer.home"));
        Ok(())
    }

    /// `standard.py:575 extrude(amount, speed=None, *args, **kwargs)`
    pub fn extrude(
        &self,
        amount: &Value,
        speed: Option<&Value>,
        tags: Option<BTreeSet<String>>,
    ) -> Result<(), PrinterError> {
        let Some(amount_num) = py_number(amount) else {
            return Err(PrinterError::ValueError(format!(
                "amount must be a valid number: {}",
                py_str_value(amount)
            )));
        };

        let printer_profile = self._printer_profile_manager.get_current_or_default();

        // Use specified speed (if any)
        let max_e_speed = printer_profile["axes"]["e"]["speed"].clone();

        let extrusion_speed = match speed {
            // No speed was specified so default to value configured in printer profile
            None | Some(Value::Null) => max_e_speed,
            // Make sure that specified value is not greater than maximum as defined in printer profile
            Some(s) => py_min_values(&[s.clone(), max_e_speed]),
        };
        let extrusion_speed = py_number(&extrusion_speed).unwrap_or_else(|| {
            panic!(
                "TypeError: %d format: a real number is required, not {}",
                py_repr_value(&extrusion_speed)
            )
        });
        let _ = amount_num;

        self.commands_list(
            &[
                "G91".to_string(),
                "M83".to_string(),
                format!(
                    "G1 E{} F{}",
                    py_str_value(amount),
                    py_percent_d(extrusion_speed)
                ),
                "M82".to_string(),
                "G90".to_string(),
            ],
            with_trigger(&tags, "trigger:printer.extrude"),
        );
        Ok(())
    }

    /// `standard.py:596 change_tool(tool, *args, **kwargs)`
    pub fn change_tool(
        &self,
        tool: &Value,
        tags: Option<BTreeSet<String>>,
    ) -> Result<(), PrinterError> {
        let Value::String(tool) = tool else {
            panic!(
                "TypeError: expected string or bytes-like object, got {}",
                py_repr_value(tool)
            )
        };
        if !VALID_TOOL_REGEX.is_match(tool).expect("regex runs") {
            return Err(PrinterError::ValueError(format!(
                "tool must match \"tool[0-9]+\": {tool}"
            )));
        }

        let rest: String = tool.chars().skip("tool".len()).collect();
        let tool_num = py_int(&rest).unwrap_or_else(|| {
            panic!("ValueError: invalid literal for int() with base 10: {rest:?}")
        });
        self.commands(
            &Value::String(format!("T{tool_num}")),
            Some(with_trigger(&tags, "trigger:printer.change_tool")),
            false,
        );
        Ok(())
    }

    /// `standard.py:606 set_temperature(heater, value, *args, **kwargs)`
    pub fn set_temperature(
        &self,
        heater: &Value,
        value: &Value,
        tags: Option<BTreeSet<String>>,
    ) -> Result<(), PrinterError> {
        let Value::String(heater) = heater else {
            panic!(
                "TypeError: expected string or bytes-like object, got {}",
                py_repr_value(heater)
            )
        };
        if !VALID_HEATER_REGEX.is_match(heater).expect("regex runs") {
            return Err(PrinterError::ValueError(format!(
                "heater must match \"tool\", \"tool([0-9])\", \"bed\" or \"chamber\": {heater}"
            )));
        }

        match py_number(value) {
            Some(n) if !(n.as_f64() < 0.0) => {}
            _ => {
                return Err(PrinterError::ValueError(format!(
                    "value must be a valid number >= 0: {}",
                    py_str_value(value)
                )));
            }
        }

        let tags = with_trigger(&tags, "trigger:printer.set_temperature");
        let value = py_str_value(value);

        if heater == "tool" {
            // set current tool, whatever that might be
            self.commands(&Value::String(format!("M104 S{value}")), Some(tags), false);
        } else if heater.starts_with("tool") {
            // set specific tool
            let printer_profile = self._printer_profile_manager.get_current_or_default();
            let extruder_count = printer_profile["extruder"]["count"]
                .as_i64()
                .expect("TypeError: extruder count is not an int");
            let shared_nozzle = py_truthy(&printer_profile["extruder"]["sharedNozzle"]);
            if extruder_count > 1 && !shared_nozzle {
                let rest: String = heater.chars().skip("tool".len()).collect();
                let tool_num = py_int(&rest).ok_or_else(|| {
                    PrinterError::ValueError(format!(
                        "invalid literal for int() with base 10: {}",
                        py_repr_str(&rest)
                    ))
                })?;
                self.commands(
                    &Value::String(format!("M104 T{tool_num} S{value}")),
                    Some(tags),
                    false,
                );
            } else {
                self.commands(&Value::String(format!("M104 S{value}")), Some(tags), false);
            }
        } else if heater == "bed" {
            self.commands(&Value::String(format!("M140 S{value}")), Some(tags), false);
        } else if heater == "chamber" {
            self.commands(&Value::String(format!("M141 S{value}")), Some(tags), false);
        }
        Ok(())
    }

    /// `standard.py:668 _convert_rate_value(factor, min_val=None, max_val=None)`
    pub fn _convert_rate_value(
        &self,
        factor: &Value,
        min_val: Option<i64>,
        max_val: Option<i64>,
    ) -> Result<i64, PrinterError> {
        let Some(n) = py_number(factor) else {
            return Err(PrinterError::ValueError(
                "factor is not a number".to_string(),
            ));
        };

        let factor: i64 = match n {
            PyNum::Float(f) => {
                // int(factor * 100)
                let x = f * 100.0;
                if x.is_nan() {
                    return Err(PrinterError::ValueError(
                        "cannot convert float NaN to integer".to_string(),
                    ));
                }
                if x.is_infinite() {
                    return Err(PrinterError::OverflowError(
                        "cannot convert float infinity to integer".to_string(),
                    ));
                }
                if x.abs() >= 9.2e18 {
                    unimplemented!("standard.py:672 int() of a float beyond the 64-bit range")
                }
                x.trunc() as i64
            }
            PyNum::Int(i) => i,
        };

        // `if min_val and factor < min_val` (a `min_val` of 0 is falsy)
        if let Some(min_val) = min_val.filter(|m| *m != 0) {
            if factor < min_val {
                return Err(PrinterError::ValueError(format!(
                    "factor must be a value >={min_val}"
                )));
            }
        }
        if let Some(max_val) = max_val.filter(|m| *m != 0) {
            if factor > max_val {
                return Err(PrinterError::ValueError(format!(
                    "factor must be a value <={max_val}"
                )));
            }
        }

        Ok(factor)
    }

    /// `standard.py:682 feed_rate(factor, *args, **kwargs)`
    pub fn feed_rate(
        &self,
        factor: &Value,
        tags: Option<BTreeSet<String>>,
    ) -> Result<(), PrinterError> {
        let factor = self._convert_rate_value(factor, Some(1), None)?;
        self.commands(
            &Value::String(format!("M220 S{factor}")),
            Some(with_trigger(&tags, "trigger:printer.feed_rate")),
            false,
        );
        Ok(())
    }

    /// `standard.py:689 flow_rate(factor, *args, **kwargs)`
    pub fn flow_rate(
        &self,
        factor: &Value,
        tags: Option<BTreeSet<String>>,
    ) -> Result<(), PrinterError> {
        let factor = self._convert_rate_value(factor, Some(1), None)?;
        self.commands(
            &Value::String(format!("M221 S{factor}")),
            Some(with_trigger(&tags, "trigger:printer.flow_rate")),
            false,
        );
        Ok(())
    }

    /// `standard.py:696 select_file(path, sd, printAfterSelect=False, user=None, pos=None, *args, **kwargs)`
    pub fn select_file(
        &self,
        path: &str,
        sd: bool,
        print_after_select: bool,
        user: Option<&str>,
        pos: Option<i64>,
        tags: Option<BTreeSet<String>>,
    ) -> Result<(), PrinterError> {
        let comm = match self._comm.get() {
            Some(comm) if !(comm.is_busy() || comm.is_streaming()) => comm,
            _ => {
                // self._logger.info("Cannot load file: printer not connected or currently busy")
                return Ok(());
            }
        };

        self._validate_job(path, sd)?;

        let origin = if sd {
            FileDestinations::SDCARD
        } else {
            FileDestinations::LOCAL
        };
        let (path_on_disk, path_in_storage) = if sd {
            (format!("/{path}"), path.to_string())
        } else {
            let on_disk = self._file_manager.path_on_disk(origin, path);
            let in_storage = self._file_manager.path_in_storage(origin, &on_disk);
            (on_disk, in_storage)
        };

        // except Exception: anything goes wrong with the recovery data, we ignore it
        let _ = py_try(|| {
            if let Some(recovery_data) = self._file_manager.get_recovery_data().filter(py_truthy) {
                // clean up recovery data if we just selected a different file
                let actual_origin = recovery_data.get("origin").and_then(Value::as_str);
                let actual_path = recovery_data.get("path").and_then(Value::as_str);

                if actual_origin.is_none()
                    || actual_path.is_none()
                    || actual_origin != Some(origin)
                    || actual_path != Some(path_in_storage.as_str())
                {
                    self._file_manager.delete_recovery_data();
                }
            }
        });

        self._print_after_select.set(print_after_select);
        self._pos_after_select.set(pos);
        comm.select_file(
            &if sd { format!("/{path}") } else { path_on_disk },
            sd,
            user,
            Some(with_trigger(&tags, "trigger:printer.select_file")),
        )?;
        // self._updateProgressData()
        self._set_current_z(None);
        Ok(())
    }

    /// `standard.py:744 unselect_file(*args, **kwargs)`
    pub fn unselect_file(&self) {
        let comm = self._comm.get();
        if let Some(comm) = &comm {
            if comm.is_busy() || comm.is_streaming() {
                return;
            }
        }

        comm.expect("AttributeError: 'NoneType' object has no attribute 'unselectFile'")
            .unselect_file();
        // self._updateProgressData()
        self._set_current_z(None);
    }

    /// `standard.py:765 start_print(pos=None, user=None, *args, **kwargs)`: starts the currently loaded
    /// print job. Only starts if the printer is connected and operational, not currently printing and a
    /// printjob is loaded
    pub fn start_print(
        &self,
        pos: Option<i64>,
        user: Option<&str>,
        tags: Option<BTreeSet<String>>,
    ) -> Result<(), PrinterError> {
        let comm = match self._comm.get() {
            Some(comm) if comm.is_operational() && !comm.is_printing() => comm,
            _ => return Ok(()),
        };

        {
            let _guard = self._selected_file_mutex.lock();
            if self._selected_file.get().is_none() {
                return Ok(());
            }
        }

        self._file_manager.delete_recovery_data();

        // self._lastProgressReport = None; self._updateProgressData()
        self._set_current_z(None);
        comm.start_print(
            pos,
            Some(with_trigger(&tags, "trigger:printer.start_print")),
            false,
            user,
        )?;
        Ok(())
    }

    /// `standard.py:792 pause_print(user=None, *args, **kwargs)`: pause the current printjob.
    pub fn pause_print(&self, user: Option<&str>, tags: Option<BTreeSet<String>>) {
        let Some(comm) = self._comm.get() else {
            return;
        };

        if comm.is_paused() {
            return;
        }

        comm.set_pause(
            true,
            true,
            user,
            Some(with_trigger(&tags, "trigger:printer.pause_print")),
        );
    }

    /// `standard.py:808 resume_print(user=None, *args, **kwargs)`: resume the current printjob.
    pub fn resume_print(&self, user: Option<&str>, tags: Option<BTreeSet<String>>) {
        let Some(comm) = self._comm.get() else {
            return;
        };

        if !comm.is_paused() {
            return;
        }

        comm.set_pause(
            false,
            true,
            user,
            Some(with_trigger(&tags, "trigger:printer.resume_print")),
        );
    }

    /// `standard.py:824 cancel_print(user=None, *args, **kwargs)`: cancel the current printjob.
    pub fn cancel_print(&self, user: Option<&str>, tags: Option<BTreeSet<String>>) {
        let Some(comm) = self._comm.get() else {
            return;
        };

        // tell comm layer to cancel - will also trigger our cancelled handler
        // for further processing
        comm.cancel_print(
            None,
            false,
            user,
            Some(with_trigger(&tags, "trigger:printer.cancel_print")),
            false,
        );
    }

    /// `standard.py:843 get_state_string(state=None, *args, **kwargs)`
    pub fn get_state_string(&self, state: Option<i64>) -> String {
        match self._comm.get() {
            None => "Offline".to_string(),
            Some(comm) => comm.get_state_string(state),
        }
    }

    /// `standard.py:849 get_state_id(state=None, *args, **kwargs)`
    pub fn get_state_id(&self, state: Option<i64>) -> String {
        match self._comm.get() {
            None => "OFFLINE".to_string(),
            Some(comm) => comm.get_state_id(state),
        }
    }

    /// `standard.py:855 get_error()`
    pub fn get_error(&self) -> String {
        match self._comm.get() {
            None => String::new(),
            Some(comm) => comm.get_error_string(),
        }
    }

    /// `standard.py:891 get_current_connection(*args, **kwargs)`: `(state string, port, baudrate, profile)`
    pub fn get_current_connection(&self) -> (String, Option<String>, Option<i64>, Option<Value>) {
        let Some(comm) = self._comm.get() else {
            return ("Closed".to_string(), None, None, None);
        };

        let (port, baudrate) = comm.get_connection();
        let printer_profile = self._printer_profile_manager.get_current_or_default();
        (
            comm.get_state_string(None),
            port,
            baudrate,
            Some(printer_profile),
        )
    }

    /// `standard.py:899 is_closed_or_error`
    pub fn is_closed_or_error(&self) -> bool {
        match self._comm.get() {
            None => true,
            Some(comm) => comm.is_closed_or_error(),
        }
    }

    /// `standard.py:902 is_operational`
    pub fn is_operational(&self) -> bool {
        self._comm.get().is_some_and(|c| c.is_operational())
    }

    /// `standard.py:905 is_printing`
    pub fn is_printing(&self) -> bool {
        self._comm.get().is_some_and(|c| c.is_printing())
    }

    /// `standard.py:908 is_cancelling`
    pub fn is_cancelling(&self) -> bool {
        self._comm.get().is_some_and(|c| c.is_cancelling())
    }

    /// `standard.py:911 is_pausing`
    pub fn is_pausing(&self) -> bool {
        self._comm.get().is_some_and(|c| c.is_pausing())
    }

    /// `standard.py:914 is_paused`
    pub fn is_paused(&self) -> bool {
        self._comm.get().is_some_and(|c| c.is_paused())
    }

    /// `standard.py:917 is_resuming`
    pub fn is_resuming(&self) -> bool {
        self._comm.get().is_some_and(|c| c.is_resuming())
    }

    /// `standard.py:920 is_finishing`
    pub fn is_finishing(&self) -> bool {
        self._comm.get().is_some_and(|c| c.is_finishing())
    }

    /// `standard.py:923 is_error`
    pub fn is_error(&self) -> bool {
        self._comm.get().is_some_and(|c| c.is_error())
    }

    /// `standard.py:926 is_ready`
    pub fn is_ready(&self) -> bool {
        self.is_operational()
            && !self._comm.get().is_some_and(|c| c.is_busy()) // isBusy is true when paused
            && !self._comm.get().is_some_and(|c| c.is_streaming())
    }

    /// `standard.py:934 is_sd_ready`
    pub fn is_sd_ready(&self) -> bool {
        match self._comm.get() {
            Some(comm) if settings().get_boolean(&["feature", "sdSupport"]) == Some(true) => {
                comm.is_sd_ready()
            }
            _ => false,
        }
    }

    // ~~ sd file handling

    /// `standard.py:942 get_sd_files(*args, **kwargs)`; `refresh` is `kwargs.get("refresh")`
    pub fn get_sd_files(&self, refresh: bool) -> Vec<Value> {
        if !self.is_sd_ready() {
            return Vec::new();
        }

        if refresh {
            self.refresh_sd_files(true, None, None);
        }

        self._comm
            .get()
            .expect("is_sd_ready is true, so there is a comm")
            .get_sd_files()
            .into_iter()
            .map(|x| {
                json!({
                    "name": x.0.chars().skip(1).collect::<String>(),
                    "size": x.1,
                    "display": x.2,
                    "date": x.3,
                })
            })
            .collect()
    }

    /// `standard.py:954 add_sd_file(filename, path, on_success=None, on_failure=None, *args, **kwargs)`;
    /// returns the remote name (`None`: the printer is not connected or busy)
    pub fn add_sd_file(
        &self,
        filename: &str,
        path: &str,
        on_success: Option<StreamingCallback>,
        on_failure: Option<StreamingCallback>,
        tags: Option<BTreeSet<String>>,
    ) -> Result<Option<String>, PrinterError> {
        match self._comm.get() {
            Some(comm) if !comm.is_busy() && comm.is_sd_ready() => {}
            _ => {
                // self._logger.error("No connection to printer or printer is busy")
                return Ok(None);
            }
        }

        self._streaming_finished_callback.set(on_success);
        self._streaming_failed_callback.set(on_failure);

        // `sd_upload_started` / `_succeeded` / `_failed` (:964-995) are only handed to the hooks
        // Ported as written: the loop hands the job to the first hook (none are supported).
        #[allow(clippy::never_loop)]
        for (_name, _hook) in self.sd_card_upload_hooks.iter() {
            // first sd card upload plugin that feels responsible gets the job
            unimplemented!(
                "standard.py:998 an octoprint.printer.sdcardupload hook gets callables (module doc, 5)"
            )
        }

        // no plugin feels responsible, use the default implementation
        self._add_sd_file(filename, path, tags)
    }

    /// `standard.py:1020 _get_free_remote_name(filename)`
    pub fn _get_free_remote_name(&self, filename: &str) -> Result<String, PrinterError> {
        self.refresh_sd_files(true, None, None);
        let existing_sd_files: Vec<String> = self
            ._comm
            .get()
            .expect("AttributeError: 'NoneType' object has no attribute 'getSdFiles'")
            .get_sd_files()
            .into_iter()
            .map(|x| x.0)
            .collect();

        if valid_file_type(filename, Some("gcode"), None) == Some(true) {
            // figure out remote filename
            Ok(get_dos_filename(
                filename,
                Some(&existing_sd_files),
                Some("gco"),
                Some(&["gco", "g"]),
                None,
            )?)
        } else {
            // probably something else added through a plugin, use it's basename as-is
            Ok(super::files::basename(filename).to_string())
        }
    }

    /// `standard.py:1038 _add_sd_file(filename, path, tags=None)`
    pub fn _add_sd_file(
        &self,
        filename: &str,
        path: &str,
        tags: Option<BTreeSet<String>>,
    ) -> Result<Option<String>, PrinterError> {
        // self._create_estimator("stream")
        let comm = self
            ._comm
            .get()
            .expect("AttributeError: 'NoneType' object has no attribute 'startFileTransfer'");
        Ok(comm.start_file_transfer(
            path,
            filename,
            None,
            valid_file_type(filename, Some("gcode"), None) != Some(true),
            Some(with_trigger(&tags, "trigger:printer.add_sd_file")),
        )?)
    }

    /// `standard.py:1052 delete_sd_file(filename, *args, **kwargs)`
    pub fn delete_sd_file(&self, filename: &str, tags: Option<BTreeSet<String>>) {
        match self._comm.get() {
            Some(comm) if comm.is_sd_ready() => {
                comm.delete_sd_file(
                    &format!("/{filename}"),
                    Some(with_trigger(&tags, "trigger:printer.delete_sd_file")),
                );
            }
            _ => {}
        }
    }

    /// `standard.py:1060 init_sd_card(*args, **kwargs)`
    pub fn init_sd_card(&self, tags: Option<BTreeSet<String>>) {
        match self._comm.get() {
            Some(comm) if !comm.is_sd_ready() => {
                comm.init_sd_card(Some(with_trigger(&tags, "trigger:printer.init_sd_card")));
            }
            _ => {}
        }
    }

    /// `standard.py:1067 release_sd_card(*args, **kwargs)`
    pub fn release_sd_card(&self, tags: Option<BTreeSet<String>>) {
        match self._comm.get() {
            Some(comm) if comm.is_sd_ready() => {
                comm.release_sd_card(Some(with_trigger(&tags, "trigger:printer.release_sd_card")));
            }
            _ => {}
        }
    }

    /// `standard.py:1074 refresh_sd_files(blocking=False, *args, **kwargs)`: refreshes the list of file
    /// stored on the SD card attached to printer (if available and printer communication available).
    /// Optional blocking parameter allows making the method block (max 10s) until the file list has been
    /// received. `timeout` is `kwargs.get("timeout", 10)`.
    pub fn refresh_sd_files(
        &self,
        blocking: bool,
        tags: Option<BTreeSet<String>>,
        timeout: Option<f64>,
    ) {
        match self._comm.get() {
            Some(comm) if comm.is_sd_ready() => {
                comm.refresh_sd_files(
                    Some(with_trigger(&tags, "trigger:printer.refresh_sd_files")),
                    blocking,
                    Some(timeout.unwrap_or(10.0)),
                );
            }
            _ => {}
        }
    }

    // ~~ state monitoring

    /// `standard.py:1093 _setCurrentZ(currentZ)`: only `self._currentZ`
    fn _set_current_z(&self, current_z: Option<f64>) {
        self._current_z.set(current_z);
        // self._stateMonitor.set_current_z(self._currentZ)
    }

    /// `standard.py:1097 _setState(state, state_string=None, error_string=None)`: the `StateMonitor` part is
    /// not ported
    fn _set_state(&self, state: i64, _state_string: Option<String>, _error_string: Option<String>) {
        self._state.set(Some(state));

        let payload = json!({
            "state_id": self.get_state_id(Some(state)),
            "state_string": self.get_state_string(Some(state)),
        });
        event_manager().fire(Events::PRINTER_STATE_CHANGED, Some(payload));
    }

    /// `standard.py:1236 _validateJob(filename, sd)`
    pub fn _validate_job(&self, filename: &str, sd: bool) -> Result<(), PrinterError> {
        if valid_file_type(filename, Some("machinecode"), None) != Some(true) {
            return Err(PrinterError::InvalidFileType(format!(
                "{filename} is not a machinecode file, cannot print"
            )));
        }

        if sd {
            return Ok(());
        }

        let path_on_disk = self
            ._file_manager
            .path_on_disk(FileDestinations::LOCAL, filename);
        if std::path::Path::new(filename).is_absolute() && filename != path_on_disk {
            return Err(PrinterError::InvalidFileLocation(format!(
                "{filename} is not located within local storage, cannot select for printing"
            )));
        }
        if !std::path::Path::new(&path_on_disk).is_file() {
            return Err(PrinterError::InvalidFileLocation(format!(
                "{filename} does not exist in local storage, cannot select for printing"
            )));
        }
        Ok(())
    }

    /// `standard.py:1257 _setJobData(filename, filesize, sd, user=None, data=None)`: sets
    /// `self._selectedFile` (the rest of the method feeds `StateMonitor`, module doc)
    fn _set_job_data(
        &self,
        filename: Option<&str>,
        filesize: Option<i64>,
        sd: bool,
        user: Option<&str>,
    ) {
        let _guard = self._selected_file_mutex.lock();
        match filename {
            Some(filename) => {
                let path_in_storage = if sd {
                    let name_in_storage = filename.strip_prefix('/').unwrap_or(filename);
                    name_in_storage.to_string()
                } else {
                    // path_on_disk and the name in storage only feed `StateMonitor`
                    self._file_manager
                        .path_in_storage(FileDestinations::LOCAL, filename)
                };
                self._selected_file.set(Some(SelectedFile {
                    filename: path_in_storage,
                    filesize,
                    sd,
                    user: user.map(str::to_string),
                }));
            }
            None => {
                self._selected_file.set(None);
            }
        }
    }

    /// `standard.py:1383 _updateJobUser(user)`
    fn _update_job_user(&self, user: Option<&str>) {
        let _guard = self._selected_file_mutex.lock();
        if let Some(mut selected) = self._selected_file.get() {
            if selected.user.as_deref() != user {
                selected.user = user.map(str::to_string);
                self._selected_file.set(Some(selected));
                // job_data = self.get_current_job(); self._stateMonitor.set_job_data(...)
            }
        }
    }

    /// `standard.py:1918 _payload_for_print_job_event(location=None, print_job_file=None, print_job_size=None,
    /// print_job_user=None, position=None, fileposition=None, progress=None, action_user=None)`; an empty
    /// map is Python's `{}`
    #[allow(clippy::too_many_arguments)]
    pub fn _payload_for_print_job_event(
        &self,
        location: Option<&str>,
        print_job_file: Option<&str>,
        print_job_size: Option<i64>,
        print_job_user: Option<&str>,
        position: Option<Value>,
        fileposition: Option<i64>,
        progress: Option<f64>,
        action_user: Option<&str>,
    ) -> Map<String, Value> {
        let (print_job_file, print_job_size, print_job_user, location): (
            Option<String>,
            Option<i64>,
            Option<String>,
            Option<String>,
        ) = if print_job_file.is_none() {
            let _guard = self._selected_file_mutex.lock();
            let Some(selected_file) = self._selected_file.get() else {
                return Map::new();
            };

            (
                Some(selected_file.filename.clone()),
                selected_file.filesize,
                selected_file.user.clone(),
                Some(
                    if selected_file.sd {
                        FileDestinations::SDCARD
                    } else {
                        FileDestinations::LOCAL
                    }
                    .to_string(),
                ),
            )
        } else {
            (
                print_job_file.map(str::to_string),
                print_job_size,
                print_job_user.map(str::to_string),
                location.map(str::to_string),
            )
        };

        let (print_job_file, location) = match (
            print_job_file.filter(|f| !f.is_empty()),
            location.filter(|l| !l.is_empty()),
        ) {
            (Some(f), Some(l)) => (f, l),
            _ => return Map::new(),
        };

        let (name, path, origin);
        if location == FileDestinations::SDCARD {
            let full_path = print_job_file
                .strip_prefix('/')
                .unwrap_or(&print_job_file)
                .to_string();
            name = full_path.clone();
            path = full_path;
            origin = FileDestinations::SDCARD;
        } else {
            path = self
                ._file_manager
                .path_in_storage(FileDestinations::LOCAL, &print_job_file);
            let (_, n) = self
                ._file_manager
                .split_path(FileDestinations::LOCAL, &path);
            name = n;
            origin = FileDestinations::LOCAL;
        }

        let mut result = Map::new();
        result.insert("name".into(), Value::String(name));
        result.insert("path".into(), Value::String(path));
        result.insert("origin".into(), Value::String(origin.to_string()));
        result.insert(
            "size".into(),
            print_job_size.map_or(Value::Null, Value::from),
        );

        if let Some(position) = position {
            result.insert("position".into(), position);
        }

        if let Some(fileposition) = fileposition {
            result.insert("fileposition".into(), Value::from(fileposition));
        }

        if let Some(progress) = progress {
            result.insert("progress".into(), Value::from((progress * 100.0) as i64));
        }

        if let Some(owner) = print_job_user {
            result.insert("owner".into(), Value::String(owner));
        }

        if let Some(user) = action_user {
            result.insert("user".into(), Value::String(user.to_string()));
        }

        result
    }

    /// OURS: a daemon thread that runs `f` (module doc, 9)
    fn spawn_daemon(f: impl FnOnce() + Send + 'static) {
        let thread = Thread::new(true);
        thread.start(f).expect("a fresh thread starts");
    }

    /// `script(name, context={"event": payload}, part_of_job=True, must_be_set=False)` of the
    /// `on_comm_print_job_*` callbacks; an `UnknownScript` cannot happen with `must_be_set=False`
    fn _script_for_job_event(&self, name: &str, payload: &Map<String, Value>) {
        let context = json!({"event": Value::Object(payload.clone())});
        if let Err(e) = self.script(name, Some(&context), false, true, None) {
            panic!("{e}");
        }
    }

    /// `finalize` of `on_comm_state_change` (:1492) and `on_comm_print_job_cancelled` (:1773): log the
    /// print as failed and fire `PrintFailed`
    fn _finalize_failed(&self, payload: Map<String, Value>) {
        let file_manager = self._file_manager.clone();
        let profile_manager = self._printer_profile_manager.clone();
        Self::spawn_daemon(move || {
            file_manager.log_print(
                payload.get("origin").and_then(Value::as_str).unwrap_or(""),
                payload.get("path").and_then(Value::as_str).unwrap_or(""),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_secs_f64()),
                payload.get("time").and_then(Value::as_f64),
                false,
                profile_manager.get_current_or_default()["id"]
                    .as_str()
                    .unwrap_or(""),
            );
            event_manager().fire(Events::PRINT_FAILED, Some(Value::Object(payload)));
        });
    }

    /// `standard.py:1868 on_comm_file_transfer_done(local_filename, remote_filename, elapsed, failed=False)`
    pub fn on_comm_file_transfer_done_failed(
        &self,
        local_filename: &str,
        remote_filename: &str,
        elapsed: Option<f64>,
        failed: bool,
    ) {
        self._sd_streaming.set(false);

        let payload = json!({"local": local_filename, "remote": remote_filename, "time": elapsed});

        if failed {
            event_manager().fire(Events::TRANSFER_FAILED, Some(payload));
            if let Some(callback) = self._streaming_failed_callback.get() {
                callback(remote_filename, remote_filename, FileDestinations::SDCARD);
            }
        } else {
            event_manager().fire(Events::TRANSFER_DONE, Some(payload));
            if let Some(callback) = self._streaming_finished_callback.get() {
                callback(remote_filename, remote_filename, FileDestinations::SDCARD);
            }
        }

        self._set_current_z(None);
        self._set_job_data(None, None, false, None);
        // self._updateProgressData(); self._stateMonitor.set_state(...)
    }
}

/// `comm.MachineComPrintCallback` implementation (`standard.py:1440`)
impl MachineComPrintCallback for Printer {
    // OURS (top edge): the calls `standard.py` leaves to its `StateMonitor` and message buffers (module
    // doc, "Callbacks left out"): they only reach the port's top edge.

    /// `standard.py:1443 on_comm_log(message)`
    fn on_comm_log(&self, message: &str) {
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_log(message);
        }
    }

    /// `standard.py:1449 on_comm_temperature_update(temp, bedTemp, chamberTemp, customTemp)`
    fn on_comm_temperature_update(
        &self,
        temp: &[(i64, super::comm::TempTuple)],
        bed_temp: super::comm::TempTuple,
        chamber_temp: super::comm::TempTuple,
        custom_temp: &[(String, super::comm::TempTuple)],
    ) {
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_temperature_update(temp, bed_temp, chamber_temp, custom_temp);
        }
    }

    /// `standard.py:1549 on_comm_message(message)`
    fn on_comm_message(&self, message: &str) {
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_message(message);
        }
    }

    /// `standard.py:1556 on_comm_progress()`
    fn on_comm_progress(&self) {
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_progress();
        }
    }

    /// `standard.py:1576 on_comm_sd_state_change(sdReady)`
    fn on_comm_sd_state_change(&self, sd_ready: bool) {
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_sd_state_change(sd_ready);
        }
    }

    /// `standard.py:1459 on_comm_position_update(position, reason=None)`
    fn on_comm_position_update(
        &self,
        position: &BTreeMap<String, Option<f64>>,
        reason: Option<&str>,
    ) {
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_position_update(position, reason);
        }
        let mut payload = Map::new();
        payload.insert(
            "reason".into(),
            reason.map_or(Value::Null, |r| Value::String(r.to_string())),
        );
        for (k, v) in position {
            payload.insert(k.clone(), v.map_or(Value::Null, Value::from));
        }
        event_manager().fire(Events::POSITION_UPDATE, Some(Value::Object(payload)));
    }

    /// `standard.py:1464 on_comm_state_change(state)`: called if the connection state changes
    fn on_comm_state_change(&self, state: i64) {
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_state_change(state);
        }
        let old_state = self._state.get();

        let mut state_string = None;
        let mut error_string = None;
        if let Some(comm) = self._comm.get() {
            state_string = Some(comm.get_state_string(None));
            error_string = Some(comm.get_error_string());
        }

        if old_state == Some(MachineCom::STATE_PRINTING) {
            // if we were still printing and went into an error state, mark the print as failed
            if [
                MachineCom::STATE_CLOSED,
                MachineCom::STATE_ERROR,
                MachineCom::STATE_CLOSED_WITH_ERROR,
            ]
            .contains(&state)
            {
                let _guard = self._selected_file_mutex.lock();
                if self._selected_file.get().is_some() {
                    let mut payload = self._payload_for_print_job_event(
                        None, None, None, None, None, None, None, None,
                    );
                    if !payload.is_empty() {
                        let comm = self._comm.get().expect(
                            "AttributeError: 'NoneType' object has no attribute 'getPrintTime'",
                        );
                        payload.insert(
                            "time".into(),
                            comm.get_print_time().map_or(Value::Null, Value::from),
                        );
                        payload.insert("reason".into(), Value::String("error".to_string()));
                        payload.insert("error".into(), Value::String(comm.get_error_string()));
                        payload.insert(
                            "progress".into(),
                            comm.get_print_progress().map_or(Value::Null, Value::from),
                        );

                        self._finalize_failed(payload);
                    }
                }
            }

            // self._analysisQueue.resume()  # printing done, put those cpu cycles to good use
        }
        // elif state == comm.MachineCom.STATE_PRINTING: the analysis queue is not ported

        if state == MachineCom::STATE_CLOSED || state == MachineCom::STATE_CLOSED_WITH_ERROR {
            if self._comm.get().is_some() {
                self._comm.set(None);
            }

            {
                let _guard = self._selected_file_mutex.lock();
                if self._selected_file.get().is_some() {
                    event_manager().fire(Events::FILE_DESELECTED, None);
                }
                self._set_job_data(None, None, false, None);
            }

            // self._updateProgressData(); self._setOffsets(None); self._addTemperatureData()
            self._set_current_z(None);
            self._printer_profile_manager.deselect();

            event_manager().fire(Events::DISCONNECTED, None);
        }

        self._set_state(state, state_string, error_string);
    }

    /// `standard.py:1543 on_comm_error(error, reason, consequence=None, faq=None, logs=None)`
    fn on_comm_error(
        &self,
        error: &str,
        reason: &str,
        consequence: Option<&str>,
        faq: Option<&str>,
        logs: Option<&[String]>,
    ) {
        // store error info
        self._error_info.set(Some(ErrorInformation {
            error: error.to_string(),
            reason: reason.to_string(),
            consequence: consequence.map(str::to_string),
            faq: faq.map(str::to_string),
            logs: logs.map(<[String]>::to_vec),
        }));
        // OURS (K18-1): the top edge learns of an emergency stop that comm's error path starts
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_error(error, reason, consequence, faq, logs);
        }
    }

    /// `standard.py:1564 on_comm_z_change(newZ)`: called upon change of the z-layer.
    fn on_comm_z_change(&self, new_z: Option<f64>) {
        let old_z = self._current_z.get();
        if new_z != old_z {
            // we have to react to all z-changes, even those that might "go backward" due to a slicer's
            // retraction or anti-backlash-routines. Event subscribes should individually take care to
            // filter out "wrong" z-changes
            event_manager().fire(Events::Z_CHANGE, Some(json!({"new": new_z, "old": old_z})));
        }

        self._set_current_z(new_z);
    }

    /// `standard.py:1585 on_comm_sd_files(files)`
    fn on_comm_sd_files(&self, _files: &[Value]) {
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_sd_files(_files);
        }
        event_manager().fire(Events::UPDATED_FILES, Some(json!({"type": "printables"})));
    }

    /// `standard.py:1588 on_comm_file_selected(full_path, size, sd, user=None, data=None)`; `data` only
    /// feeds `StateMonitor`
    fn on_comm_file_selected(
        &self,
        full_path: Option<&str>,
        size: Option<i64>,
        sd: bool,
        user: Option<&str>,
        _data: Option<&Value>,
    ) {
        if let Some(full_path) = full_path {
            let payload = self._payload_for_print_job_event(
                Some(if sd {
                    FileDestinations::SDCARD
                } else {
                    FileDestinations::LOCAL
                }),
                Some(full_path),
                None,
                user,
                None,
                None,
                None,
                user,
            );
            event_manager().fire(Events::FILE_SELECTED, Some(Value::Object(payload)));
            // self._logger_job.info("Print job selected - origin: {}, path: {}, owner: {}, user: {}")
        } else {
            event_manager().fire(Events::FILE_DESELECTED, None);
            // self._logger_job.info("Print job deselected - user: {}")
        }

        self._set_job_data(full_path, size, sd, user);
        // self._stateMonitor.set_state(...); self._create_estimator()

        if self._print_after_select.get() {
            self._print_after_select.set(false);
            if let Err(e) = self.start_print(self._pos_after_select.get(), user, None) {
                panic!("{e}");
            }
        }
    }

    /// `standard.py:1626 on_comm_print_job_started(suppress_script=False, user=None)`
    fn on_comm_print_job_started(&self, suppress_script: bool, user: Option<&str>) {
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_print_job_started(suppress_script, user);
        }
        // clear error info
        self._error_info.set(None);

        self._update_job_user(user); // the final job owner should always be whoever _started_ the job
        // self._stateMonitor.trigger_progress_update()
        let payload =
            self._payload_for_print_job_event(None, None, None, user, None, None, None, user);
        if !payload.is_empty() {
            event_manager().fire(Events::PRINT_STARTED, Some(Value::Object(payload.clone())));
            event_manager().fire(
                Events::CHART_MARKED,
                Some(json!({"type": "print", "label": "Start"})),
            );
            // self._logger_job.info("Print job started - origin: {}, path: {}, owner: {}, user: {}")

            if !suppress_script {
                self._script_for_job_event("beforePrintStarted", &payload);
            }
        }
    }

    /// `standard.py:1658 on_comm_print_job_done(suppress_script=False)`
    fn on_comm_print_job_done(&self, suppress_script: bool) {
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_print_job_done(suppress_script);
        }
        self._file_manager.delete_recovery_data();

        let mut payload =
            self._payload_for_print_job_event(None, None, None, None, None, None, None, None);
        if !payload.is_empty() {
            let comm = self
                ._comm
                .get()
                .expect("AttributeError: 'NoneType' object has no attribute 'getPrintTime'");
            payload.insert(
                "time".into(),
                comm.get_print_time().map_or(Value::Null, Value::from),
            );
            event_manager().fire(
                Events::CHART_MARKED,
                Some(json!({"type": "done", "label": "Done"})),
            );
            // self._updateProgressData(completion=1.0, filepos=payload["size"], printTime=payload["time"], printTimeLeft=0)
            // self._stateMonitor.set_state(...)

            event_manager().fire(Events::PRINT_DONE, Some(Value::Object(payload.clone())));
            // self._logger_job.info("Print job done - origin: {}, path: {}, owner: {}")

            if !suppress_script {
                self._script_for_job_event("afterPrintDone", &payload);
            }

            let file_manager = self._file_manager.clone();
            let profile_manager = self._printer_profile_manager.clone();
            Self::spawn_daemon(move || {
                file_manager.log_print(
                    payload.get("origin").and_then(Value::as_str).unwrap_or(""),
                    payload.get("path").and_then(Value::as_str).unwrap_or(""),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0.0, |d| d.as_secs_f64()),
                    payload.get("time").and_then(Value::as_f64),
                    true,
                    profile_manager.get_current_or_default()["id"]
                        .as_str()
                        .unwrap_or(""),
                );
            });
        }
        // else: self._updateProgressData(); self._stateMonitor.set_state(...)
    }

    /// `standard.py:1723 on_comm_print_job_cancelling(firmware_error=None, user=None)`
    fn on_comm_print_job_cancelling(&self, firmware_error: Option<&str>, user: Option<&str>) {
        let mut payload =
            self._payload_for_print_job_event(None, None, None, None, None, None, None, user);
        if !payload.is_empty() {
            if let Some(firmware_error) = firmware_error.filter(|e| !e.is_empty()) {
                payload.insert(
                    "firmwareError".into(),
                    Value::String(firmware_error.to_string()),
                );
            }
            event_manager().fire(Events::PRINT_CANCELLING, Some(Value::Object(payload)));
        }
    }

    /// `standard.py:1730 on_comm_print_job_cancelled(suppress_script=False, user=None)`
    fn on_comm_print_job_cancelled(&self, suppress_script: bool, user: Option<&str>) {
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_print_job_cancelled(suppress_script, user);
        }
        self._set_current_z(None);
        // self._updateProgressData()

        let comm = self._comm.get();
        let fileposition = comm.as_ref().and_then(|c| c.get_file_position());
        let progress = comm.as_ref().and_then(|c| c.get_print_progress());
        let position = comm.as_ref().map(|c| {
            Value::Object(
                c.cancel_position
                    .get()
                    .as_dict()
                    .into_iter()
                    .map(|(k, v)| (k, v.map_or(Value::Null, Value::from)))
                    .collect(),
            )
        });
        let mut payload = self._payload_for_print_job_event(
            None,
            None,
            None,
            None,
            position,
            fileposition.map(|f| f.pos),
            progress,
            user,
        );
        if !payload.is_empty() {
            let comm =
                comm.expect("AttributeError: 'NoneType' object has no attribute 'getPrintTime'");
            payload.insert(
                "time".into(),
                comm.get_print_time().map_or(Value::Null, Value::from),
            );

            event_manager().fire(
                Events::PRINT_CANCELLED,
                Some(Value::Object(payload.clone())),
            );
            event_manager().fire(
                Events::CHART_MARKED,
                Some(json!({"type": "cancel", "label": "Cancel"})),
            );
            // self._logger_job.info("Print job cancelled - origin: {}, path: {}, owner: {}, user: {}, fileposition: {}, position: {}")

            if !suppress_script {
                self._script_for_job_event("afterPrintCancelled", &payload);
            }

            payload.insert("reason".into(), Value::String("cancelled".to_string()));

            self._finalize_failed(payload);
        }
    }

    /// `standard.py:1788 on_comm_print_job_paused(suppress_script=False, user=None)`
    fn on_comm_print_job_paused(&self, suppress_script: bool, user: Option<&str>) {
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_print_job_paused(suppress_script, user);
        }
        let comm = self._comm.get();
        let fileposition = comm.as_ref().and_then(|c| c.get_file_position());
        let progress = comm.as_ref().and_then(|c| c.get_print_progress());
        let position = comm.as_ref().filter(|_| !suppress_script).map(|c| {
            Value::Object(
                c.pause_position
                    .get()
                    .as_dict()
                    .into_iter()
                    .map(|(k, v)| (k, v.map_or(Value::Null, Value::from)))
                    .collect(),
            )
        });
        let payload = self._payload_for_print_job_event(
            None,
            None,
            None,
            None,
            position,
            fileposition.map(|f| f.pos),
            progress,
            user,
        );
        if !payload.is_empty() {
            event_manager().fire(Events::PRINT_PAUSED, Some(Value::Object(payload.clone())));
            // self._logger_job.info("Print job paused - origin: {}, path: {}, owner: {}, user: {}, fileposition: {}, position: {}")
            event_manager().fire(
                Events::CHART_MARKED,
                Some(json!({"type": "pause", "label": "Pause"})),
            );
            if !suppress_script {
                self._script_for_job_event("afterPrintPaused", &payload);
            }
        }
    }

    /// `standard.py:1823 on_comm_print_job_resumed(suppress_script=False, user=None)`
    fn on_comm_print_job_resumed(&self, suppress_script: bool, user: Option<&str>) {
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_print_job_resumed(suppress_script, user);
        }
        let payload =
            self._payload_for_print_job_event(None, None, None, None, None, None, None, user);
        if !payload.is_empty() {
            event_manager().fire(Events::PRINT_RESUMED, Some(Value::Object(payload.clone())));
            event_manager().fire(
                Events::CHART_MARKED,
                Some(json!({"type": "resume", "label": "Resume"})),
            );
            // self._logger_job.info("Print job resumed - origin: {}, path: {}, owner: {}, user: {}")

            if !suppress_script {
                self._script_for_job_event("beforePrintResumed", &payload);
            }
        }
    }

    /// `standard.py:1848 on_comm_file_transfer_started(local_filename, remote_filename, filesize, user=None)`
    fn on_comm_file_transfer_started(
        &self,
        local_filename: &str,
        remote_filename: &str,
        filesize: Option<i64>,
        user: Option<&str>,
    ) {
        event_manager().fire(
            Events::TRANSFER_STARTED,
            Some(json!({"local": local_filename, "remote": remote_filename})),
        );

        self._sd_streaming.set(true);

        self._set_job_data(Some(remote_filename), filesize, true, user);
        // self._updateProgressData(completion=0.0, filepos=0, printTime=0); self._stateMonitor.set_state(...)
    }

    /// `standard.py:1868 on_comm_file_transfer_done(local_filename, remote_filename, elapsed, failed=False)`
    fn on_comm_file_transfer_done(
        &self,
        local_filename: &str,
        remote_filename: &str,
        elapsed: Option<f64>,
    ) {
        self.on_comm_file_transfer_done_failed(local_filename, remote_filename, elapsed, false);
    }

    /// `standard.py:1899 on_comm_file_transfer_failed(local_filename, remote_filename, elapsed)`
    fn on_comm_file_transfer_failed(
        &self,
        local_filename: &str,
        remote_filename: &str,
        elapsed: Option<f64>,
    ) {
        self.on_comm_file_transfer_done_failed(local_filename, remote_filename, elapsed, true);
    }

    /// `standard.py:1904 on_comm_force_disconnect()`
    fn on_comm_force_disconnect(&self) {
        self.disconnect();
    }

    /// `standard.py:1907 on_comm_record_fileposition(origin, name, pos)`
    fn on_comm_record_fileposition(&self, origin: &str, name: &str, pos: Option<i64>) {
        // except NoSuchStorage: pass; except Exception: self._logger.exception("Error while trying to persist ...")
        let _ = py_try(|| {
            let _ = self._file_manager.save_recovery_data(origin, name, pos);
        });
    }

    /// `standard.py:1915 on_comm_firmware_info(firmware_name, firmware_data)`
    fn on_comm_firmware_info(&self, firmware_name: &str, firmware_data: &PyDict<String>) {
        if let Some(edge) = self._top_edge.get() {
            edge.on_comm_firmware_info(firmware_name, firmware_data);
        }
        let data: Map<String, Value> = firmware_data
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        self._firmware_info.set(Some(
            json!({"name": firmware_name, "data": Value::Object(data)}),
        ));
    }
}

#[cfg(test)]
mod tests {
    //! `tests/printer/test_script.py` ported (`ScriptsTestCase`, three tests), and OURS for the rest
    //! (OctoPrint 1.11.8 has no test of the other methods of `printer/standard.py`).
    //!
    //! OURS: the reference mocks `comm` with a `MagicMock` (every call answers something truthy) and the
    //! event manager with a mock. Here `comm` is a real, unconnected `MachineCom` (nothing is sent, no serial
    //! port is opened) and the events are recorded by listeners subscribed to the event names the test
    //! uses, so `must_be_set=False` is passed where the reference relies on the mock's truthy answer for a
    //! script name that does not exist. What the commands are is read from the comm's command queue (the
    //! comm is put in the printing state, in which `sendCommand` queues a command).
    use super::*;
    use crate::ports::octoprint::events::Listener;
    use std::sync::Mutex;

    fn printer() -> (Arc<Printer>, Arc<MachineCom>) {
        let profile_manager = Arc::new(PrinterProfileManager::new());
        let printer = Printer::new(Arc::new(LocalFileManager), profile_manager.clone());
        let comm = MachineCom::new(None, None, None, Some(profile_manager));
        printer.set_comm(Some(comm.clone()));
        (printer, comm)
    }

    /// the printer with a comm in the state in which `sendCommand` puts the commands into the command queue
    fn printer_printing() -> (Arc<Printer>, Arc<MachineCom>) {
        let (printer, comm) = printer();
        comm._change_state(MachineCom::STATE_PRINTING);
        (printer, comm)
    }

    /// what was queued: `(command, tags)`
    fn queued(comm: &MachineCom) -> Vec<(String, BTreeSet<String>)> {
        comm._command_queue
            .clear()
            .into_iter()
            .map(|(cmd, _, _, tags)| match cmd {
                super::super::comm::QueuedCommand::Command(c) => (c, tags),
                _ => panic!("a marker"),
            })
            .collect()
    }

    fn texts(comm: &MachineCom) -> Vec<String> {
        queued(comm).into_iter().map(|(c, _)| c).collect()
    }

    fn tagset(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn record(events: &[&str]) -> Arc<Mutex<Vec<(String, Option<Value>)>>> {
        let seen: Arc<Mutex<Vec<(String, Option<Value>)>>> = Arc::new(Mutex::new(Vec::new()));
        for event in events {
            let seen = seen.clone();
            let listener: Listener =
                Arc::new(move |e, p| seen.lock().unwrap().push((e.to_string(), p.cloned())));
            event_manager().subscribe(event, listener);
        }
        seen
    }

    // ---- tests/printer/test_script.py ---------------------------------------------------------

    #[test]
    fn test_event_name() {
        let seen = record(&["GcodeScriptTestEventRunning"]);
        let (printer, _comm) = printer();
        let _ = printer.script("testEvent", None, true, false, None);
        crate::ports::octoprint::events::wait_idle(); // the events are delivered by the worker thread
        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .any(|(e, _)| e == "GcodeScriptTestEventRunning")
        );
    }

    #[test]
    fn test_event_order() {
        let seen = record(&[
            "GcodeScriptEventOrderRunning",
            "GcodeScriptEventOrderFinished",
        ]);
        let (printer, _comm) = printer();
        printer
            .script("EventOrder", None, false, false, None)
            .unwrap();
        crate::ports::octoprint::events::wait_idle(); // the events are delivered by the worker thread
        let names: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(e, _)| e.clone())
            .collect();
        assert_eq!(
            vec![
                "GcodeScriptEventOrderRunning",
                "GcodeScriptEventOrderFinished"
            ],
            names
        );
    }

    #[test]
    fn test_payload_handling() {
        let seen = record(&[
            "GcodeScriptGetPayloadRunning",
            "GcodeScriptGetPayloadFinished",
            "GcodeScriptWrongPayloadRunning",
            "GcodeScriptWrongPayloadFinished",
            "GcodeScriptWrongContextRunning",
            "GcodeScriptWrongContextFinished",
        ]);
        let (printer, _comm) = printer();
        let expected_payload = json!({"payloadKey": "payloadValue"});
        let context = json!({"event": expected_payload});

        printer
            .script("GetPayload", Some(&context), false, false, None)
            .unwrap();
        crate::ports::octoprint::events::wait_idle(); // the events are delivered by the worker thread
        assert_eq!(
            Some(expected_payload),
            seen.lock().unwrap().last().unwrap().1
        );

        printer
            .script("WrongPayload", Some(&json!([])), false, false, None)
            .unwrap();
        crate::ports::octoprint::events::wait_idle(); // the events are delivered by the worker thread
        assert_eq!(None, seen.lock().unwrap().last().unwrap().1);

        printer
            .script(
                "WrongContext",
                Some(&json!({"noEvent": {}})),
                false,
                false,
                None,
            )
            .unwrap();
        crate::ports::octoprint::events::wait_idle(); // the events are delivered by the worker thread
        assert_eq!(None, seen.lock().unwrap().last().unwrap().1);
    }

    // ---- OURS ------------------------------------------------------------------------------------

    #[test]
    fn ours_script_name_must_be_set() {
        let (printer, _comm) = printer();
        assert_eq!(
            Err(PrinterError::ValueError("name must be set".into())),
            printer.script("", None, true, false, None)
        );
        // an unknown script with must_be_set is UnknownScript (:503), after the Running event
        assert_eq!(
            Err(PrinterError::UnknownScript("noSuchScript".into())),
            printer.script("noSuchScript", None, true, false, None)
        );
    }

    #[test]
    fn ours_commands_need_a_comm_and_get_their_tag() {
        // no comm: nothing (:467)
        let printer = Printer::new(
            Arc::new(LocalFileManager),
            Arc::new(PrinterProfileManager::new()),
        );
        printer.commands(&json!("M105"), None, false);
        let (printer, comm) = printer_printing();
        printer.commands(&json!(["M105", "M114"]), Some(tagset(&["a"])), false);
        assert_eq!(
            vec![
                (
                    "M105".to_string(),
                    tagset(&["a", "trigger:printer.commands"])
                ),
                (
                    "M114".to_string(),
                    tagset(&["a", "trigger:printer.commands"])
                ),
            ],
            queued(&comm)
        );
        printer.commands(&json!("G28"), None, false);
        assert_eq!(vec!["G28".to_string()], texts(&comm));
    }

    #[test]
    fn ours_jog() {
        let (printer, comm) = printer_printing();
        // the default speed is the slowest of the axes' speeds in the profile (:524); X6000, Z200 -> F200
        printer
            .jog(&json!({"x": 10, "z": -1.5}), true, None, &[], None)
            .unwrap();
        assert_eq!(vec!["G91", "G0 X10 Z-1.5 F200", "G90"], texts(&comm));
        // absolute, with a speed
        printer
            .jog(
                &json!({"y": 5}),
                false,
                Some(&json!(3000)),
                &[],
                Some(tagset(&["t"])),
            )
            .unwrap();
        let q = queued(&comm);
        assert_eq!(
            vec!["G90", "G0 Y5 F3000"],
            q.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>()
        );
        assert!(q[0].1.contains("trigger:printer.jog") && q[0].1.contains("t"));
        // speed 0 and a bool speed give no F (:545)
        printer
            .jog(&json!({"y": 5}), false, Some(&json!(0)), &[], None)
            .unwrap();
        printer
            .jog(&json!({"y": 5}), false, Some(&json!(true)), &[], None)
            .unwrap();
        assert_eq!(vec!["G90", "G0 Y5", "G90", "G0 Y5"], texts(&comm));
        // the legacy format
        printer
            .jog(&json!("x"), true, Some(&json!(1)), &[json!(2.5)], None)
            .unwrap();
        assert_eq!(vec!["G91", "G0 X2.5 F1", "G90"], texts(&comm));
        // the refusals (:513-533)
        assert_eq!(
            Err(PrinterError::ValueError("amount not set".into())),
            printer.jog(&json!("x"), true, None, &[], None)
        );
        assert_eq!(
            Err(PrinterError::ValueError(
                "amount must be a valid number: abc".into()
            )),
            printer.jog(&json!("x"), true, None, &[json!("abc")], None)
        );
        assert_eq!(
            Err(PrinterError::ValueError(
                "At least one axis to jog must be provided".into()
            )),
            printer.jog(&json!({}), true, None, &[], None)
        );
        assert_eq!(
            Err(PrinterError::ValueError(
                "Invalid axis q, valid axes are x, y, z, e".into()
            )),
            printer.jog(&json!({"q": 1}), true, None, &[], None)
        );
        assert!(texts(&comm).is_empty());
    }

    #[test]
    fn ours_home() {
        let (printer, comm) = printer_printing();
        printer
            .home(&json!(["x", "Y"]), Some(tagset(&["api"])))
            .unwrap();
        let q = queued(&comm);
        assert_eq!(
            vec!["G91", "G28 X0 Y0", "G90"],
            q.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>()
        );
        assert!(q[1].1.contains("trigger:printer.home") && q[1].1.contains("api"));
        printer.home(&json!("z"), Some(BTreeSet::new())).unwrap();
        assert_eq!(vec!["G91", "G28 Z0", "G90"], texts(&comm));
        assert_eq!(
            Err(PrinterError::ValueError(
                "axes is neither a list nor a string: 5".into()
            )),
            printer.home(&json!(5), Some(BTreeSet::new()))
        );
        assert_eq!(
            Err(PrinterError::ValueError(
                "axes contains invalid axes: ['x', 'q']".into()
            )),
            printer.home(&json!(["x", "q"]), Some(BTreeSet::new()))
        );
        // :571 `kwargs.get("tags", set)`: without tags it is Python's TypeError, after the check
        assert!(matches!(
            printer.home(&json!(["x"]), None),
            Err(PrinterError::TypeError(_))
        ));
        assert!(texts(&comm).is_empty());
    }

    #[test]
    fn ours_extrude() {
        let (printer, comm) = printer_printing();
        // no speed: the profile's E speed, 300 (:586)
        printer.extrude(&json!(5), None, None).unwrap();
        assert_eq!(vec!["G91", "M83", "G1 E5 F300", "M82", "G90"], texts(&comm));
        // a speed above the profile's is capped, one below is kept (:589-591)
        printer
            .extrude(&json!(-2.5), Some(&json!(1000)), None)
            .unwrap();
        assert_eq!(
            vec!["G91", "M83", "G1 E-2.5 F300", "M82", "G90"],
            texts(&comm)
        );
        printer
            .extrude(&json!(1), Some(&json!(120.7)), None)
            .unwrap();
        assert_eq!(vec!["G91", "M83", "G1 E1 F120", "M82", "G90"], texts(&comm));
        assert_eq!(
            Err(PrinterError::ValueError(
                "amount must be a valid number: x".into()
            )),
            printer.extrude(&json!("x"), None, None)
        );
    }

    #[test]
    fn ours_change_tool_and_set_temperature() {
        let (printer, comm) = printer_printing();
        printer.change_tool(&json!("tool2"), None).unwrap();
        assert_eq!(vec!["T2"], texts(&comm));
        assert_eq!(
            Err(PrinterError::ValueError(
                "tool must match \"tool[0-9]+\": tool".into()
            )),
            printer.change_tool(&json!("tool"), None)
        );
        printer
            .set_temperature(&json!("tool"), &json!(200), None)
            .unwrap();
        printer
            .set_temperature(&json!("tool1"), &json!(210.5), None)
            .unwrap(); // one extruder: no T
        printer
            .set_temperature(&json!("bed"), &json!(60), None)
            .unwrap();
        printer
            .set_temperature(&json!("chamber"), &json!(0), None)
            .unwrap();
        assert_eq!(
            vec!["M104 S200", "M104 S210.5", "M140 S60", "M141 S0"],
            texts(&comm)
        );
        assert!(matches!(
            printer.set_temperature(&json!("hotend"), &json!(1), None),
            Err(PrinterError::ValueError(_))
        ));
        assert_eq!(
            Err(PrinterError::ValueError(
                "value must be a valid number >= 0: -1".into()
            )),
            printer.set_temperature(&json!("bed"), &json!(-1), None)
        );
        assert_eq!(
            Err(PrinterError::ValueError(
                "value must be a valid number >= 0: hot".into()
            )),
            printer.set_temperature(&json!("bed"), &json!("hot"), None)
        );
    }

    #[test]
    fn ours_set_temperature_of_a_specific_tool_with_several_extruders() {
        let profile = PrinterProfileManager::with_current(
            json!({"extruder": {"count": 2, "sharedNozzle": false}}),
        );
        let profile = Arc::new(profile);
        let printer = Printer::new(Arc::new(LocalFileManager), profile.clone());
        let comm = MachineCom::new(None, None, None, Some(profile));
        comm._change_state(MachineCom::STATE_PRINTING);
        printer.set_comm(Some(comm.clone()));
        printer
            .set_temperature(&json!("tool1"), &json!(215), None)
            .unwrap();
        assert_eq!(vec!["M104 T1 S215"], texts(&comm));
    }

    #[test]
    fn ours_rates() {
        let (printer, comm) = printer_printing();
        printer.feed_rate(&json!(0.5), None).unwrap(); // :672 int(0.5 * 100)
        printer.feed_rate(&json!(150), None).unwrap();
        printer.flow_rate(&json!(1.1), None).unwrap(); // int(1.1 * 100) = 110
        assert_eq!(vec!["M220 S50", "M220 S150", "M221 S110"], texts(&comm));
        assert_eq!(
            Err(PrinterError::ValueError("factor is not a number".into())),
            printer.feed_rate(&json!("x"), None)
        );
        assert_eq!(
            Err(PrinterError::ValueError(
                "factor must be a value >=1".into()
            )),
            printer.feed_rate(&json!(0), None)
        );
        assert_eq!(
            Err(PrinterError::ValueError(
                "factor must be a value >=1".into()
            )),
            printer.flow_rate(&json!(0.001), None)
        );
        // `_convert_rate_value`'s max (:676)
        assert_eq!(
            Err(PrinterError::ValueError(
                "factor must be a value <=10".into()
            )),
            printer._convert_rate_value(&json!(11), None, Some(10))
        );
        assert_eq!(
            Ok(7),
            printer._convert_rate_value(&json!(7), None, Some(10))
        );
    }

    #[test]
    fn ours_state_getters_without_a_comm() {
        let printer = Printer::new(
            Arc::new(LocalFileManager),
            Arc::new(PrinterProfileManager::new()),
        );
        assert_eq!("Offline", printer.get_state_string(None));
        assert_eq!("OFFLINE", printer.get_state_id(None));
        assert_eq!("", printer.get_error());
        assert_eq!(
            ("Closed".to_string(), None, None, None),
            printer.get_current_connection()
        );
        assert!(printer.is_closed_or_error());
        assert!(
            !printer.is_operational()
                && !printer.is_printing()
                && !printer.is_ready()
                && !printer.is_sd_ready()
        );
        assert_eq!(Vec::<Value>::new(), printer.get_sd_files(false));
        // with nothing connected, every control does nothing (:467, :792, :808, :824, :1052-1080)
        printer.pause_print(None, None);
        printer.resume_print(None, None);
        printer.cancel_print(None, None);
        printer.delete_sd_file("a.gco", None);
        printer.init_sd_card(None);
        printer.release_sd_card(None);
        printer.refresh_sd_files(false, None, None);
        assert_eq!(Ok(()), printer.start_print(None, None, None));
        assert_eq!(
            Ok(()),
            printer.select_file("a.gcode", false, false, None, None, None)
        );
    }

    #[test]
    fn ours_validate_job() {
        let (printer, _comm) = printer();
        assert!(matches!(
            printer._validate_job("a.stl", false),
            Err(PrinterError::InvalidFileType(_))
        ));
        assert!(matches!(
            printer._validate_job("a.stl", true),
            Err(PrinterError::InvalidFileType(_))
        ));
        assert_eq!(Ok(()), printer._validate_job("a.gcode", true)); // an SD file is not looked for
        assert!(matches!(
            printer._validate_job("/no/such/dir/a.gcode", false),
            Err(PrinterError::InvalidFileLocation(_))
        ));
    }

    #[test]
    fn ours_selected_file_and_payload() {
        let (printer, _comm) = printer();
        // nothing selected: {} (:1927)
        assert!(
            printer
                ._payload_for_print_job_event(None, None, None, None, None, None, None, None)
                .is_empty()
        );
        printer._set_job_data(Some("/jobs/a.gcode"), Some(10), false, Some("owner"));
        let payload = printer._payload_for_print_job_event(
            None,
            None,
            None,
            None,
            None,
            Some(4),
            Some(0.456),
            Some("me"),
        );
        assert_eq!(
            json!({"name": "a.gcode", "path": "/jobs/a.gcode", "origin": "local", "size": 10, "fileposition": 4, "progress": 45, "owner": "owner", "user": "me"}),
            Value::Object(payload)
        );
        printer._update_job_user(Some("other"));
        assert_eq!(
            Some("other".to_string()),
            printer._selected_file.get().unwrap().user
        );
        // an SD file loses its leading slash (:1264, :1946)
        printer._set_job_data(Some("/B.GCO"), None, true, None);
        let payload =
            printer._payload_for_print_job_event(None, None, None, None, None, None, None, None);
        assert_eq!(
            json!({"name": "B.GCO", "path": "B.GCO", "origin": "sdcard", "size": null}),
            Value::Object(payload)
        );
        printer._set_job_data(None, None, false, None);
        assert!(printer._selected_file.get().is_none());
    }

    #[test]
    fn ours_callbacks_keep_state() {
        let (printer, _comm) = printer();
        printer.on_comm_error("e", "r", Some("c"), None, Some(&["l1".to_string()]));
        assert_eq!(
            Some(
                json!({"error": "e", "reason": "r", "consequence": "c", "faq": null, "logs": ["l1"]})
            ),
            printer.error_info()
        );
        let mut data = PyDict::new();
        data.insert("FIRMWARE_NAME", "Marlin".to_string());
        printer.on_comm_firmware_info("Marlin", &data);
        assert_eq!(
            Some(json!({"name": "Marlin", "data": {"FIRMWARE_NAME": "Marlin"}})),
            printer.firmware_info()
        );
        // a z change fires an event once per change (:1567)
        let seen = record(&["ZChange"]);
        printer.on_comm_z_change(Some(1.0));
        printer.on_comm_z_change(Some(1.0));
        printer.on_comm_z_change(Some(2.0));
        crate::ports::octoprint::events::wait_idle(); // the events are delivered by the worker thread
        let payloads: Vec<Option<Value>> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(_, p)| p.clone())
            .collect();
        assert_eq!(
            vec![
                Some(json!({"new": 1.0, "old": null})),
                Some(json!({"new": 2.0, "old": 1.0}))
            ],
            payloads
        );
    }

    #[test]
    fn ours_state_change_to_closed_forgets_the_comm() {
        let (printer, _comm) = printer();
        let seen = record(&["Disconnected", "PrinterStateChanged"]);
        printer.on_comm_state_change(MachineCom::STATE_CLOSED);
        assert!(printer.comm().is_none());
        crate::ports::octoprint::events::wait_idle(); // the events are delivered by the worker thread
        let names: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(e, _)| e.clone())
            .collect();
        assert_eq!(vec!["Disconnected", "PrinterStateChanged"], names);
        // with the comm gone the state id is OFFLINE (:849)
        assert_eq!(
            Some(json!({"state_id": "OFFLINE", "state_string": "Offline"})),
            seen.lock().unwrap()[1].1
        );
    }
}
