//! PrusaLink's `printer_adapter/structures/model_classes.py` (`reference/Prusa-Link`, `171148ce`):
//! Contains models that were originally intended for sending to the connect. Pydantic makes a
//! great tool for cleanly serializing simple python objects, while enforcing their type
//! (#242, section A).
//!
//! A pydantic 1.10.13 `BaseModel` is a struct here: each field with its default, in the source's
//! order, and `dict()` (pydantic's `BaseModel.dict`, `exclude_none` the one argument PrusaLink
//! passes: `telemetry_passer.py:203`, `:416`) as a JSON object in that order.
//!
//! **Not covered:** pydantic's validation when a model is made (a value of the wrong type is
//! converted, `int` from `"5"` or from `5.7`, or refused): Rust's types take only the field's own
//! type. Where PrusaLink makes a model from values of another type, the port of that caller
//! converts as pydantic does (sections E-G).

use maker_connect_gcode_metadata::py::PyErr;
use serde_json::{Map, Value, json};

/// A Python `Enum` whose values are strings: `.value` and `Enum(value)`.
macro_rules! str_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $value:literal,)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant,)*
        }

        impl $name {
            /// Every member, in definition order.
            pub const ALL: &'static [$name] = &[$($name::$variant,)*];

            /// `.name`
            pub fn name(self) -> &'static str {
                match self {
                    $($name::$variant => $value,)*
                }
            }

            /// `.value`
            pub fn value(self) -> &'static str {
                self.name()
            }

            /// `Enum(value)`; `None` is its `ValueError`
            pub fn from_value(value: &str) -> Option<$name> {
                match value {
                    $($value => Some($name::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

/// `IndividualSlot` (`:12-17`): Support the slot number specific telemetry structure
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IndividualSlot {
    pub material: Option<String>,
    pub temp: Option<f64>,
    pub fan_hotend: Option<i64>,
    pub fan_print: Option<i64>,
}

impl IndividualSlot {
    /// `BaseModel.dict()`
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        let mut data = Map::new();
        put(&mut data, exclude_none, "material", json!(self.material));
        put(&mut data, exclude_none, "temp", json!(self.temp));
        put(
            &mut data,
            exclude_none,
            "fan_hotend",
            json!(self.fan_hotend),
        );
        put(&mut data, exclude_none, "fan_print", json!(self.fan_print));
        data
    }
}

/// `Slot` (`:20-36`): Support the telemetry item described here:
/// <https://connect.prusa3d.com/docs/mmu> (Internal doc)
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Slot {
    pub active: Option<i64>,
    pub state: Option<i64>,
    pub progress: Option<i64>,
    pub command: Option<String>,
    /// `Dict[str, IndividualSlot]`, in its insertion order
    pub slots: Option<Vec<(String, IndividualSlot)>>,
}

impl Slot {
    /// `dict` (`:30-36`): Override the dict method to respect the Connect telemetry API
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        // data = super().dict(**kwargs)
        let mut data = Map::new();
        put(&mut data, exclude_none, "active", json!(self.active));
        put(&mut data, exclude_none, "state", json!(self.state));
        put(&mut data, exclude_none, "progress", json!(self.progress));
        put(&mut data, exclude_none, "command", json!(self.command));
        let slots = self.slots.as_ref().map(|slots| {
            slots
                .iter()
                .map(|(key, slot)| (key.clone(), Value::Object(slot.dict(exclude_none))))
                .collect::<Map<String, Value>>()
        });
        put(&mut data, exclude_none, "slots", json!(slots));
        // if "slots" in data and data["slots"] is not None:
        if data.get("slots").is_some_and(|slots| !slots.is_null()) {
            // slots = data.pop("slots"); data.update(slots)
            let Some(Value::Object(slots)) = data.shift_remove("slots") else {
                unreachable!("slots is a dict")
            };
            for (key, value) in slots {
                data.insert(key, value);
            }
        }
        data
    }
}

/// `Telemetry` (`:39-78`): The Telemetry model
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Telemetry {
    // time_remaining is deprecated, kept for compatibility
    pub temp_nozzle: Option<f64>,
    pub temp_bed: Option<f64>,
    pub target_nozzle: Option<f64>,
    pub target_bed: Option<f64>,
    pub axis_x: Option<f64>,
    pub axis_y: Option<f64>,
    pub axis_z: Option<f64>,
    pub fan_extruder: Option<i64>,
    pub fan_hotend: Option<i64>,
    pub fan_print: Option<i64>,
    pub target_fan_extruder: Option<i64>,
    pub target_fan_hotend: Option<i64>,
    pub target_fan_print: Option<i64>,
    pub progress: Option<i64>,
    pub filament: Option<String>,
    pub flow: Option<i64>,
    pub speed: Option<i64>,
    pub time_printing: Option<i64>,
    pub time_transferring: Option<i64>,
    pub time_remaining: Option<i64>,
    pub odometer_x: Option<i64>,
    pub odometer_y: Option<i64>,
    pub odometer_z: Option<i64>,
    pub odometer_e: Option<i64>,
    pub material: Option<String>,
    pub total_filament: Option<i64>,
    pub total_print_time: Option<i64>,
    pub filament_change_in: Option<i64>,
    pub inaccurate_estimates: Option<bool>,
    pub slot: Option<Slot>,
}

impl Telemetry {
    /// `dict` (`:74-78`)
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        // data = super().dict(**kwargs)
        let mut data = Map::new();
        let d = &mut data;
        put(d, exclude_none, "temp_nozzle", json!(self.temp_nozzle));
        put(d, exclude_none, "temp_bed", json!(self.temp_bed));
        put(d, exclude_none, "target_nozzle", json!(self.target_nozzle));
        put(d, exclude_none, "target_bed", json!(self.target_bed));
        put(d, exclude_none, "axis_x", json!(self.axis_x));
        put(d, exclude_none, "axis_y", json!(self.axis_y));
        put(d, exclude_none, "axis_z", json!(self.axis_z));
        put(d, exclude_none, "fan_extruder", json!(self.fan_extruder));
        put(d, exclude_none, "fan_hotend", json!(self.fan_hotend));
        put(d, exclude_none, "fan_print", json!(self.fan_print));
        put(
            d,
            exclude_none,
            "target_fan_extruder",
            json!(self.target_fan_extruder),
        );
        put(
            d,
            exclude_none,
            "target_fan_hotend",
            json!(self.target_fan_hotend),
        );
        put(
            d,
            exclude_none,
            "target_fan_print",
            json!(self.target_fan_print),
        );
        put(d, exclude_none, "progress", json!(self.progress));
        put(d, exclude_none, "filament", json!(self.filament));
        put(d, exclude_none, "flow", json!(self.flow));
        put(d, exclude_none, "speed", json!(self.speed));
        put(d, exclude_none, "time_printing", json!(self.time_printing));
        put(
            d,
            exclude_none,
            "time_transferring",
            json!(self.time_transferring),
        );
        put(
            d,
            exclude_none,
            "time_remaining",
            json!(self.time_remaining),
        );
        put(d, exclude_none, "odometer_x", json!(self.odometer_x));
        put(d, exclude_none, "odometer_y", json!(self.odometer_y));
        put(d, exclude_none, "odometer_z", json!(self.odometer_z));
        put(d, exclude_none, "odometer_e", json!(self.odometer_e));
        put(d, exclude_none, "material", json!(self.material));
        put(
            d,
            exclude_none,
            "total_filament",
            json!(self.total_filament),
        );
        put(
            d,
            exclude_none,
            "total_print_time",
            json!(self.total_print_time),
        );
        put(
            d,
            exclude_none,
            "filament_change_in",
            json!(self.filament_change_in),
        );
        put(
            d,
            exclude_none,
            "inaccurate_estimates",
            json!(self.inaccurate_estimates),
        );
        // `super().dict` puts pydantic's own dict of `slot` here, then
        // if self.slot is not None: data['slot'] = self.slot.dict(**kwargs)
        // replaces it in the same place.
        put(
            d,
            exclude_none,
            "slot",
            self.slot
                .as_ref()
                .map_or(Value::Null, |slot| Value::Object(slot.dict(exclude_none))),
        );
        data
    }
}

/// `NetworkInfo` (`:81-93`): The Network Info model
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetworkInfo {
    pub lan_ipv4: Option<String>, // not implemented yet
    pub lan_ipv6: Option<String>, // not implemented yet
    pub lan_mac: Option<String>,  // not implemented yet
    pub wifi_ipv4: Option<String>,
    pub wifi_ipv6: Option<String>, // not implemented yet
    pub wifi_mac: Option<String>,
    pub wifi_ssid: Option<String>, // not implemented yet
    pub hostname: Option<String>,
    pub username: Option<String>,
    pub digest: Option<String>,
}

impl NetworkInfo {
    /// `BaseModel.dict()`
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        let mut data = Map::new();
        let d = &mut data;
        put(d, exclude_none, "lan_ipv4", json!(self.lan_ipv4));
        put(d, exclude_none, "lan_ipv6", json!(self.lan_ipv6));
        put(d, exclude_none, "lan_mac", json!(self.lan_mac));
        put(d, exclude_none, "wifi_ipv4", json!(self.wifi_ipv4));
        put(d, exclude_none, "wifi_ipv6", json!(self.wifi_ipv6));
        put(d, exclude_none, "wifi_mac", json!(self.wifi_mac));
        put(d, exclude_none, "wifi_ssid", json!(self.wifi_ssid));
        put(d, exclude_none, "hostname", json!(self.hostname));
        put(d, exclude_none, "username", json!(self.username));
        put(d, exclude_none, "digest", json!(self.digest));
        data
    }
}

str_enum! {
    /// `FileType` (`:96-100`): File type enum
    FileType {
        File = "FILE",
        Folder = "FOLDER",
        Storage = "STORAGE",
    }
}

str_enum! {
    /// `JobState` (`:103-107`): Job state enum
    JobState {
        Idle = "IDLE",
        InProgress = "IN_PROGRESS",
        Ending = "ENDING",
    }
}

str_enum! {
    /// `SDState` (`:110-115`): SD State enum
    SDState {
        Present = "PRESENT",
        Initialising = "INITIALISING",
        Unsure = "UNSURE",
        Absent = "ABSENT",
    }
}

str_enum! {
    /// `PrintState` (`:118-123`): States which the printer can report on its own
    PrintState {
        SdPrinting = "SD_PRINTING",
        SdPaused = "SD_PAUSED",
        SerialPaused = "SERIAL_PAUSED",
        NotSdPrinting = "NOT_SD_PRINTING",
    }
}

str_enum! {
    /// `PrintMode` (`:126-130`): The "Mode" from the printer LCD settings
    PrintMode {
        Silent = "SILENT",
        Normal = "NORMAL",
        Auto = "AUTO",
    }
}

/// `EEPROMParams` (`:133-142`): List of EEPROM addresses read by PrusaLink. The value is the
/// tuple (address, length).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EEPROMParams {
    JobId,
    FlashAir,
    PrintMode,
    SheetSettings,
    ActiveSheet,
    TotalFilament,
    TotalPrintTime,
    EepromFilePosition,
}

impl EEPROMParams {
    /// Every member, in definition order.
    pub const ALL: &'static [EEPROMParams] = &[
        EEPROMParams::JobId,
        EEPROMParams::FlashAir,
        EEPROMParams::PrintMode,
        EEPROMParams::SheetSettings,
        EEPROMParams::ActiveSheet,
        EEPROMParams::TotalFilament,
        EEPROMParams::TotalPrintTime,
        EEPROMParams::EepromFilePosition,
    ];

    /// `.name`
    pub fn name(self) -> &'static str {
        match self {
            EEPROMParams::JobId => "JOB_ID",
            EEPROMParams::FlashAir => "FLASH_AIR",
            EEPROMParams::PrintMode => "PRINT_MODE",
            EEPROMParams::SheetSettings => "SHEET_SETTINGS",
            EEPROMParams::ActiveSheet => "ACTIVE_SHEET",
            EEPROMParams::TotalFilament => "TOTAL_FILAMENT",
            EEPROMParams::TotalPrintTime => "TOTAL_PRINT_TIME",
            EEPROMParams::EepromFilePosition => "EEPROM_FILE_POSITION",
        }
    }

    /// `.value`: (address, length)
    pub fn value(self) -> (i64, i64) {
        match self {
            EEPROMParams::JobId => (0x0D05, 4),
            EEPROMParams::FlashAir => (0x0FBB, 1),
            EEPROMParams::PrintMode => (0x0FFF, 1),
            EEPROMParams::SheetSettings => (0x0D49, 88),
            EEPROMParams::ActiveSheet => (0x0DA1, 1),
            EEPROMParams::TotalFilament => (0x0FF1, 4),
            EEPROMParams::TotalPrintTime => (0x0FED, 4),
            EEPROMParams::EepromFilePosition => (0x0F91, 4),
        }
    }
}

/// `PPData` (`:145-152`): Not things like length or diameter, just path and the command number ->
/// gcode command number
#[derive(Debug, Clone, PartialEq)]
pub struct PPData {
    pub file_path: String,
    pub connect_path: String,
    pub message_number: i64, // N number on the printer
    pub gcode_number: i64,   // From file printer
    pub using_rip_port: bool,
}

impl PPData {
    /// `PPData(file_path=..., connect_path=..., message_number=..., gcode_number=...)`:
    /// `using_rip_port` defaults to `False`.
    pub fn new(
        file_path: String,
        connect_path: String,
        message_number: i64,
        gcode_number: i64,
    ) -> PPData {
        PPData {
            file_path,
            connect_path,
            message_number,
            gcode_number,
            using_rip_port: false,
        }
    }

    /// `PPData(**data)` of a JSON object: each field of its own type (see the module's "not
    /// covered"), a missing one pydantic's `ValidationError`, others left out.
    pub fn from_json(data: &Value) -> Result<PPData, PyErr> {
        let missing = |name: &str| {
            PyErr::Other(
                "ValidationError",
                format!(
                    "1 validation error for PPData\n{name}\n  field required (type=value_error.missing)"
                ),
            )
        };
        let wrong = |name: &str| {
            PyErr::Other(
                "ValidationError",
                format!("PPData {name}: not of the field's type"),
            )
        };
        let str_ = |name: &str| match data.get(name) {
            None => Err(missing(name)),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(wrong(name)),
        };
        let int = |name: &str| match data.get(name) {
            None => Err(missing(name)),
            Some(v) => v.as_i64().ok_or_else(|| wrong(name)),
        };
        Ok(PPData {
            file_path: str_("file_path")?,
            connect_path: str_("connect_path")?,
            message_number: int("message_number")?,
            gcode_number: int("gcode_number")?,
            using_rip_port: match data.get("using_rip_port") {
                None => false,
                Some(v) => v.as_bool().ok_or_else(|| wrong("using_rip_port"))?,
            },
        })
    }

    /// `BaseModel.dict()`
    pub fn dict(&self, exclude_none: bool) -> Map<String, Value> {
        let mut data = Map::new();
        let d = &mut data;
        put(d, exclude_none, "file_path", json!(self.file_path));
        put(d, exclude_none, "connect_path", json!(self.connect_path));
        put(
            d,
            exclude_none,
            "message_number",
            json!(self.message_number),
        );
        put(d, exclude_none, "gcode_number", json!(self.gcode_number));
        put(
            d,
            exclude_none,
            "using_rip_port",
            json!(self.using_rip_port),
        );
        data
    }
}

/// One field into a model's `dict()`: left out when `exclude_none` and it is `None`.
fn put(data: &mut Map<String, Value>, exclude_none: bool, name: &str, value: Value) {
    if !(exclude_none && value.is_null()) {
        data.insert(name.to_owned(), value);
    }
}

#[cfg(test)]
mod tests;
