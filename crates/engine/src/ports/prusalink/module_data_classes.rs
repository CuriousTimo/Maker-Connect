//! PrusaLink's `printer_adapter/structures/module_data_classes.py` (`reference/Prusa-Link`,
//! `171148ce`): the data of the modules that keep it for the API (#242, section A).
//!
//! A pydantic 1.10.13 `BaseModel` is a struct here: each field with its default, in the source's
//! order, and `dict()` (pydantic's `BaseModel.dict`, `exclude_none` the one argument PrusaLink
//! passes: `telemetry_passer.py:203`, `:416`) as a JSON object in that order, as in
//! [`super::model_classes`]. A model is made as Python makes it, `Model(field=..., ...)`: a struct
//! literal over the model's `Default`.
//!
//! `State` is the SDK's (`prusa.connect.printer.const`), [`maker_connect_sdk::consts::State`];
//! `JobState` and `SDState` are PrusaLink's ([`super::model_classes`]).
//!
//! **Not covered:**
//! - pydantic's validation when a model is made (a value of the wrong type is converted, `int`
//!   from `"5"` or from `5.7`, or refused): Rust's types take only the field's own type.
//! - pydantic's `ValidationError` for a field Python requires that is missing: here the field
//!   takes the zero value (`Default`), or the model is made with the field named.
//! - pydantic's `dict()` keeps an `Enum` as it is (`JobState.IDLE`); here it is the enum's value
//!   (`"IDLE"`), the only thing a JSON object can hold.
//! - pydantic's `Set` in `dict()` (not JSON): here an array, sorted. A Python `set` has no order of
//!   its own (its hash order).
//! - `FilePrinterData.dict()`: its `enqueued` holds instructions, which are not data (no caller:
//!   only `Port.dict()` has one, `web/settings.py:78`).

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use indexmap::IndexMap;
use maker_connect_sdk::consts::State;
use serde_json::{Map, Value, json};

use super::instruction::Instruction;
use super::model_classes::{JobState, SDState};

/// `Port` (`module_data_classes.py:16-34`): Data known about a port
#[derive(Debug, Clone, PartialEq)]
pub struct Port {
    pub path: String,
    pub is_rpi_port: bool,
    pub checked: bool,       // False if it has not been finished checking
    pub usable: bool,        // We can probably use this port for communication
    pub selected: bool,      // PrusaLink selected to use this port
    pub description: String, // A nice human-readable status
    pub baudrate: i64,
    pub timeout: i64,
    pub sn: Option<String>, // Save the USB descriptor SN if valid
}

impl Default for Port {
    /// `Port()` (`module_data_classes.py:18-26`): the defaults of the fields.
    fn default() -> Port {
        Port {
            path: String::new(),
            is_rpi_port: false,
            checked: false,
            usable: false,
            selected: false,
            description: "Unknown".to_owned(),
            baudrate: 115200,
            timeout: 2,
            sn: None,
        }
    }
}

impl Port {
    /// `BaseModel.dict()` (`module_data_classes.py:16-26`)
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        let mut data = Map::new();
        let d = &mut data;
        put(d, exclude_none, "path", json!(self.path));
        put(d, exclude_none, "is_rpi_port", json!(self.is_rpi_port));
        put(d, exclude_none, "checked", json!(self.checked));
        put(d, exclude_none, "usable", json!(self.usable));
        put(d, exclude_none, "selected", json!(self.selected));
        put(d, exclude_none, "description", json!(self.description));
        put(d, exclude_none, "baudrate", json!(self.baudrate));
        put(d, exclude_none, "timeout", json!(self.timeout));
        put(d, exclude_none, "sn", json!(self.sn));
        data
    }
}

impl std::fmt::Display for Port {
    /// `__str__` (`module_data_classes.py:28-34`)
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Port: {}, \
             Checked: {}, \
             Usable: {}, \
             Selected: {}, \
             RPi port: {}, \
             Description: {}",
            self.path,
            py_bool(self.checked),
            py_bool(self.usable),
            py_bool(self.selected),
            py_bool(self.is_rpi_port),
            self.description
        )
    }
}

/// `SerialAdapterData` (`module_data_classes.py:37-42`): Data of the SerialAdapter class
#[derive(Debug, Clone, PartialEq)]
pub struct SerialAdapterData {
    pub ports: Vec<Port>,
    pub using_port: Option<Port>,
    pub reset_disabling: bool,
    pub resets_enabled: Option<bool>,
}

impl Default for SerialAdapterData {
    /// `SerialAdapterData()` (`module_data_classes.py:39-42`): the defaults of the fields.
    fn default() -> SerialAdapterData {
        SerialAdapterData {
            ports: Vec::new(),
            using_port: None,
            reset_disabling: true,
            resets_enabled: None,
        }
    }
}

impl SerialAdapterData {
    /// `BaseModel.dict()` (`module_data_classes.py:37-42`)
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        let mut data = Map::new();
        let d = &mut data;
        let ports: Vec<Value> = self
            .ports
            .iter()
            .map(|port| Value::Object(port.dict(exclude_none)))
            .collect();
        put(d, exclude_none, "ports", Value::Array(ports));
        put(
            d,
            exclude_none,
            "using_port",
            self.using_port
                .as_ref()
                .map_or(Value::Null, |port| Value::Object(port.dict(exclude_none))),
        );
        put(
            d,
            exclude_none,
            "reset_disabling",
            json!(self.reset_disabling),
        );
        put(
            d,
            exclude_none,
            "resets_enabled",
            json!(self.resets_enabled),
        );
        data
    }
}

/// `FilePrinterData` (`module_data_classes.py:45-58`): Data of the FilePrinter class
pub struct FilePrinterData {
    pub file_path: String,
    pub pp_file_path: String,
    pub printing: bool,
    pub recovering: bool,
    pub paused: bool,
    pub was_stopped: bool,
    pub power_panic: bool,
    pub recovery_ready: bool,
    // In reality Deque[Instruction] but that cannot be validated by pydantic
    pub enqueued: VecDeque<Arc<Instruction>>,
    pub gcode_number: i64,
}

/// `StateManagerData` (`module_data_classes.py:61-72`): Data of the StateManager class
///
/// No `Default`: `State` has no default member, and picking one would be ours.
#[derive(Debug, Clone, PartialEq)]
pub struct StateManagerData {
    // The ACTUAL states considered when reporting
    pub base_state: State,
    pub printing_state: Option<State>,
    pub override_state: Option<State>,
    // Reported state history
    pub last_state: State,
    pub current_state: State,
    pub state_history: VecDeque<State>,
    pub awaiting_error_reason: bool,
}

impl StateManagerData {
    /// `BaseModel.dict()` (`module_data_classes.py:61-72`)
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        let mut data = Map::new();
        let d = &mut data;
        put(d, exclude_none, "base_state", json!(self.base_state));
        put(
            d,
            exclude_none,
            "printing_state",
            json!(self.printing_state),
        );
        put(
            d,
            exclude_none,
            "override_state",
            json!(self.override_state),
        );
        put(d, exclude_none, "last_state", json!(self.last_state));
        put(d, exclude_none, "current_state", json!(self.current_state));
        put(d, exclude_none, "state_history", json!(self.state_history));
        put(
            d,
            exclude_none,
            "awaiting_error_reason",
            json!(self.awaiting_error_reason),
        );
        data
    }
}

/// `JobData` (`module_data_classes.py:75-100`): Data of the Job class
///
/// No `Default`: `JobState` has no default member, and picking one would be ours.
#[derive(Debug, Clone, PartialEq)]
pub struct JobData {
    pub job_id: Option<i64>,
    pub job_id_offset: i64,
    pub already_sent: Option<bool>,
    pub job_start_cmd_id: Option<i64>,
    pub selected_file_path: Option<String>,
    pub selected_file_m_timestamp: Option<i64>,
    pub selected_file_size: Option<String>,
    pub printing_file_byte: Option<i64>,
    pub path_incomplete: Option<bool>,
    pub from_sd: Option<bool>,
    pub inbuilt_reporting: Option<bool>,
    pub last_job_path: Option<String>,
    pub job_state: JobState,
}

impl JobData {
    /// `BaseModel.dict()` (`module_data_classes.py:75-100`)
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        let mut data = Map::new();
        let d = &mut data;
        put(d, exclude_none, "job_id", json!(self.job_id));
        put(d, exclude_none, "job_id_offset", json!(self.job_id_offset));
        put(d, exclude_none, "already_sent", json!(self.already_sent));
        put(
            d,
            exclude_none,
            "job_start_cmd_id",
            json!(self.job_start_cmd_id),
        );
        put(
            d,
            exclude_none,
            "selected_file_path",
            json!(self.selected_file_path),
        );
        put(
            d,
            exclude_none,
            "selected_file_m_timestamp",
            json!(self.selected_file_m_timestamp),
        );
        put(
            d,
            exclude_none,
            "selected_file_size",
            json!(self.selected_file_size),
        );
        put(
            d,
            exclude_none,
            "printing_file_byte",
            json!(self.printing_file_byte),
        );
        put(
            d,
            exclude_none,
            "path_incomplete",
            json!(self.path_incomplete),
        );
        put(d, exclude_none, "from_sd", json!(self.from_sd));
        put(
            d,
            exclude_none,
            "inbuilt_reporting",
            json!(self.inbuilt_reporting),
        );
        put(d, exclude_none, "last_job_path", json!(self.last_job_path));
        put(
            d,
            exclude_none,
            "job_state",
            Value::String(self.job_state.value().to_owned()),
        );
        data
    }

    /// `get_job_id_for_api()` (`module_data_classes.py:93-100`): The API does not send None values.
    /// This function returns None when no job is running, otherwise it gives the job_id
    pub fn get_job_id_for_api(&self) -> Option<i64> {
        if self.job_state == JobState::Idle {
            return None;
        }
        self.job_id
    }
}

/// `IPUpdaterData` (`module_data_classes.py:103-113`): Data of the IpUpdater class
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IPUpdaterData {
    pub local_ip: Option<String>,
    pub local_ip6: Option<String>,
    pub mac: Option<String>,
    pub is_wireless: bool,
    pub update_ip_on: f64,
    pub ssid: Option<String>,
    pub hostname: Option<String>,
    pub username: Option<String>,
    pub digest: Option<String>,
}

impl IPUpdaterData {
    /// `BaseModel.dict()` (`module_data_classes.py:103-113`)
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        let mut data = Map::new();
        let d = &mut data;
        put(d, exclude_none, "local_ip", json!(self.local_ip));
        put(d, exclude_none, "local_ip6", json!(self.local_ip6));
        put(d, exclude_none, "mac", json!(self.mac));
        put(d, exclude_none, "is_wireless", json!(self.is_wireless));
        put(d, exclude_none, "update_ip_on", json!(self.update_ip_on));
        put(d, exclude_none, "ssid", json!(self.ssid));
        put(d, exclude_none, "hostname", json!(self.hostname));
        put(d, exclude_none, "username", json!(self.username));
        put(d, exclude_none, "digest", json!(self.digest));
        data
    }
}

/// `SDCardData` (`module_data_classes.py:116-127`): Data of the SDCard class
///
/// No `Default`: `SDState` has no default member, and picking one would be ours.
#[derive(Debug, Clone, PartialEq)]
pub struct SDCardData {
    pub expecting_insertion: bool,
    pub invalidated: bool,
    pub is_flash_air: bool,
    pub last_updated: f64,
    pub last_checked_flash_air: f64,
    pub sd_state: SDState,
    pub files: Value, // We cannot type-check SDFile, only basic ones
    pub sfn_to_lfn_paths: IndexMap<String, String>,
    pub lfn_to_sfn_paths: IndexMap<String, String>,
    pub mixed_to_lfn_paths: IndexMap<String, String>,
}

impl SDCardData {
    /// `BaseModel.dict()` (`module_data_classes.py:116-127`)
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        let mut data = Map::new();
        let d = &mut data;
        put(
            d,
            exclude_none,
            "expecting_insertion",
            json!(self.expecting_insertion),
        );
        put(d, exclude_none, "invalidated", json!(self.invalidated));
        put(d, exclude_none, "is_flash_air", json!(self.is_flash_air));
        put(d, exclude_none, "last_updated", json!(self.last_updated));
        put(
            d,
            exclude_none,
            "last_checked_flash_air",
            json!(self.last_checked_flash_air),
        );
        put(
            d,
            exclude_none,
            "sd_state",
            Value::String(self.sd_state.value().to_owned()),
        );
        put(d, exclude_none, "files", self.files.clone());
        put(
            d,
            exclude_none,
            "sfn_to_lfn_paths",
            Value::Object(str_map(&self.sfn_to_lfn_paths)),
        );
        put(
            d,
            exclude_none,
            "lfn_to_sfn_paths",
            Value::Object(str_map(&self.lfn_to_sfn_paths)),
        );
        put(
            d,
            exclude_none,
            "mixed_to_lfn_paths",
            Value::Object(str_map(&self.mixed_to_lfn_paths)),
        );
        data
    }
}

/// `StorageData` (`module_data_classes.py:130-135`): Data of the Storage class
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StorageData {
    pub blacklisted_paths: Vec<String>,
    pub blacklisted_names: Vec<String>,
    pub configured_storage: HashSet<String>,
    pub attached_set: HashSet<String>,
}

impl StorageData {
    /// `BaseModel.dict()` (`module_data_classes.py:130-135`)
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        let mut data = Map::new();
        let d = &mut data;
        put(
            d,
            exclude_none,
            "blacklisted_paths",
            json!(self.blacklisted_paths),
        );
        put(
            d,
            exclude_none,
            "blacklisted_names",
            json!(self.blacklisted_names),
        );
        put(
            d,
            exclude_none,
            "configured_storage",
            json!(sorted(&self.configured_storage)),
        );
        put(
            d,
            exclude_none,
            "attached_set",
            json!(sorted(&self.attached_set)),
        );
        data
    }
}

/// `MMUObserverData` (`module_data_classes.py:138-140`): Data of the MMUObserver
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MMUObserverData {
    pub current_error_code: Option<String>,
}

impl MMUObserverData {
    /// `BaseModel.dict()` (`module_data_classes.py:138-140`)
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        let mut data = Map::new();
        let d = &mut data;
        put(
            d,
            exclude_none,
            "current_error_code",
            json!(self.current_error_code),
        );
        data
    }
}

/// `PrintStatsData` (`module_data_classes.py:143-150`): Data of the PrintStats class
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrintStatsData {
    pub print_time: f64,
    pub segment_start: f64,
    pub has_inbuilt_stats: bool,
    pub total_gcode_count: i64, // is not computed for files containg reporting
    //                         to speed stuff up
    pub start_gcode_number: i64,
}

impl PrintStatsData {
    /// `BaseModel.dict()` (`module_data_classes.py:143-150`)
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        let mut data = Map::new();
        let d = &mut data;
        put(d, exclude_none, "print_time", json!(self.print_time));
        put(d, exclude_none, "segment_start", json!(self.segment_start));
        put(
            d,
            exclude_none,
            "has_inbuilt_stats",
            json!(self.has_inbuilt_stats),
        );
        put(
            d,
            exclude_none,
            "total_gcode_count",
            json!(self.total_gcode_count),
        );
        put(
            d,
            exclude_none,
            "start_gcode_number",
            json!(self.start_gcode_number),
        );
        data
    }
}

/// `Sheet` (`module_data_classes.py:153-159`): Data available for sheets in the printer EEPROM
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub z_offset: f64,
    // temps at the time of calibration
    pub bed_temp: i64,
    pub pinda_temp: i64,
}

impl Sheet {
    /// `BaseModel.dict()` (`module_data_classes.py:153-159`)
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        let mut data = Map::new();
        let d = &mut data;
        put(d, exclude_none, "name", json!(self.name));
        put(d, exclude_none, "z_offset", json!(self.z_offset));
        put(d, exclude_none, "bed_temp", json!(self.bed_temp));
        put(d, exclude_none, "pinda_temp", json!(self.pinda_temp));
        data
    }
}

/// One field into a model's `dict()`: left out when `exclude_none` and it is `None`.
fn put(data: &mut Map<String, Value>, exclude_none: bool, name: &str, value: Value) {
    if !(exclude_none && value.is_null()) {
        data.insert(name.to_owned(), value);
    }
}

/// `str(bool)` of CPython: "True" or "False".
fn py_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

/// A `set` in `dict()`: sorted (see the module's doc).
fn sorted(set: &HashSet<String>) -> Vec<&String> {
    let mut items: Vec<&String> = set.iter().collect();
    items.sort();
    items
}

/// A `Dict[str, str]` as a JSON object, in the order the pairs are in.
fn str_map(pairs: &IndexMap<String, String>) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect()
}

#[cfg(test)]
mod tests;
