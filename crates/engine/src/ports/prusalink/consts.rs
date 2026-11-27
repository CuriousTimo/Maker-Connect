//! PrusaLink's `const.py` (`reference/Prusa-Link`, `171148ce`, `prusa/link/const.py`), ported name
//! by name in its order (#242, section A of `docs/archive/plans/mk3-port-plan.md`). The SDK's
//! `const.py` (`PrinterType`, `State`) is [`maker_connect_sdk::consts`]. A Python `int` is an
//! `i64` here and a `float` an `f64`, as the source writes each value; a `set`, `dict` or `list` is
//! an array in the source's order.
//!
//! **Not covered** (the test lists them; nothing else is left out):
//! - `instance_id` (`:17`, `uuid.uuid4()`): a new random value on each start, read only by
//!   `web/main.py:31` and `service_discovery.py:14`; it comes with them.
//! - `DATA_PATH` (`:43`): where the `prusa.link` package is installed; here it is given by whoever
//!   runs the port, as for the other paths.
//! - `MINIMAL_FIRMWARE` (`:165`, `Version(SUPPORTED_FIRMWARE)`): the text is the same; `packaging`'s
//!   `Version` comes with its one user, `printer_polling.py:878` (section F1).

use maker_connect_sdk::consts::{PrinterType, State};

use super::model_classes::{PrintMode, PrintState};

/// `HEADER_DATETIME_FORMAT` (`:20`): e.g. Mon, 07 Nov 2022 13:52:49 GMT
pub const HEADER_DATETIME_FORMAT: &str = "%a, %d %b %Y %X GMT";

/// `PRINTER_TYPES` (`:22-32`)
pub const PRINTER_TYPES: [(i64, PrinterType); 9] = [
    (250, PrinterType::I3Mk25),
    (252, PrinterType::I3Mk25S),
    (20250, PrinterType::I3Mk25),
    (20252, PrinterType::I3Mk25S),
    (300, PrinterType::I3Mk3),
    (20300, PrinterType::I3Mk3),
    (302, PrinterType::I3Mk3S),
    (20302, PrinterType::I3Mk3S),
    (30302, PrinterType::I3Mk3S),
];

/// `MMU3_TYPE_CODE` (`:34`)
pub const MMU3_TYPE_CODE: i64 = 30302;

/// `PRINTER_CONF_TYPES` (`:36-41`, a `bidict`)
pub const PRINTER_CONF_TYPES: [(&str, PrinterType); 4] = [
    ("MK2.5", PrinterType::I3Mk25),
    ("MK2.5S", PrinterType::I3Mk25S),
    ("MK3", PrinterType::I3Mk3),
    ("MK3S", PrinterType::I3Mk3S),
];

/// `BASE_STATES` (`:45`)
pub const BASE_STATES: [State; 3] = [State::Idle, State::Busy, State::Ready];
/// `PRINTING_STATES` (`:46`)
pub const PRINTING_STATES: [State; 4] = [
    State::Printing,
    State::Paused,
    State::Finished,
    State::Stopped,
];

/// `MK25_PRINTERS` (`:48`): the `.value`s
pub const MK25_PRINTERS: [PrinterType; 2] = [PrinterType::I3Mk25, PrinterType::I3Mk25S];

/// `JOB_STARTING_STATES` (`:50`)
pub const JOB_STARTING_STATES: [State; 2] = [State::Printing, State::Paused];
/// `JOB_ENDING_STATES` (`:51-54`)
pub const JOB_ENDING_STATES: [State; 2] = [State::Finished, State::Stopped];
/// `JOB_DESTROYING_STATES` (`:55-59`)
pub const JOB_DESTROYING_STATES: [State; 3] = [State::Error, State::Idle, State::Busy];

pub const JITTER_THRESHOLD: f64 = 0.5;
pub const PRUSA_VENDOR_ID: &str = "2c99";

// --- Intervals ---
// Values are in seconds

pub const TELEMETRY_IDLE_INTERVAL: f64 = 0.25;
pub const TELEMETRY_PRINTING_INTERVAL: i64 = 1;
pub const TELEMETRY_SLEEPING_INTERVAL: i64 = 4; // can be sleeping in any state
pub const TELEMETRY_SLEEP_AFTER: i64 = 3 * 60;
pub const TELEMETRY_REFRESH_INTERVAL: i64 = 5 * 60; // full telemetry re-send

pub const FAST_POLL_INTERVAL: i64 = 1;
pub const SLOW_POLL_INTERVAL: i64 = 10;
pub const VERY_SLOW_POLL_INTERVAL: i64 = 30;
pub const IP_UPDATE_INTERVAL: i64 = 5;
pub const QUIT_INTERVAL: f64 = 0.2;
pub const SD_INTERVAL: f64 = 0.2;
pub const SD_FILESCAN_INTERVAL: i64 = 60;
pub const DIR_RESCAN_INTERVAL: i64 = 1;
pub const PRINTER_BOOT_WAIT: i64 = 8;
pub const SEND_INFO_RETRY: i64 = 5;
pub const SERIAL_REOPEN_TIMEOUT: i64 = 2;
pub const REPORTING_TIMEOUT: i64 = 60;
pub const FW_MESSAGE_TIMEOUT: i64 = 10;
pub const STATE_CHANGE_TIMEOUT: i64 = 15;
pub const IP_WRITE_TIMEOUT: i64 = 5;
pub const SN_OBTAIN_INTERVAL: i64 = 5;
pub const EXIT_TIMEOUT: i64 = 15;
pub const ERROR_REASON_TIMEOUT: i64 = 2;
pub const PATH_WAIT_TIMEOUT: i64 = 10;
pub const SLEEP_SCREEN_TIMEOUT: i64 = 20;
pub const SELF_PING_TIMEOUT: i64 = 5;
pub const SELF_PING_RETRY_INTERVAL: i64 = 10;
pub const ATTENTION_CLEAR_INTERVAL: i64 = 5;
pub const CAMERA_INIT_DELAY: i64 = 2;
pub const CAMERA_SCAN_INTERVAL: i64 = 30;
pub const CAMERA_REGISTER_TIMEOUT: i64 = 5;
pub const TIME_FOR_SNAPSHOT: i64 = 1;
pub const PRINT_END_TIMEOUT: i64 = 11;
pub const KEEPALIVE_INTERVAL: i64 = 12;

pub const PP_MOVES_DELAY: i64 = 20;

// --- Lcd queue ---
pub const LCD_QUEUE_SIZE: i64 = 30;

// --- Serial queue ---
pub const RX_SIZE: i64 = 128; // Not used much, limits the max serial message size
pub const SERIAL_QUEUE_TIMEOUT: i64 = 25;
pub const SERIAL_QUEUE_MONITOR_INTERVAL: i64 = 1;
pub const HISTORY_LENGTH: i64 = 100; // How many messages to remember for Resends

// --- Is planner fed ---
pub const QUEUE_SIZE: i64 = 10000; // From how many messages to compute the percentile
pub const HEAP_RATIO: f64 = 0.95; // What percentile to compute
pub const IGNORE_ABOVE: f64 = 1.0; // Ignore instructions, that take longer than x sec
pub const DEFAULT_THRESHOLD: f64 = 0.13; // Percentile for uninitialised component
pub const USE_DYNAMIC_THRESHOLD: bool = true; // Compute the percentile or use a fixed value?

// --- File printer ---
pub const STATS_EVERY: i64 = 100;
pub const TAIL_COMMANDS: i64 = 10; // how many commands after the last progress report
pub const PRINT_QUEUE_SIZE: i64 = 4;

// --- Storage ---
pub const MAX_FILENAME_LENGTH: i64 = 52;
pub const SD_STORAGE_NAME: &str = "SD Card";
pub const BLACKLISTED_TYPES: [&str; 0] = [];
pub const BLACKLISTED_PATHS: [&str; 4] = ["/dev", "/sys", "/proc", "/tmp"];
pub const BLACKLISTED_NAMES: [&str; 1] = [SD_STORAGE_NAME];
pub const SFN_TO_LFN_EXTENSIONS: [(&str, &str); 3] = [("GCO", "gcode"), ("G", "g"), ("GC", "gc")];

pub const RESET_PIN: i64 = 22; // RPi gpio pin for resetting printer
pub const SUPPORTED_FIRMWARE: &str = "3.14.0";
/// `Version(SUPPORTED_FIRMWARE)` (`:165`): see the module's "not covered"
pub const MINIMAL_FIRMWARE: &str = SUPPORTED_FIRMWARE;
pub const MAX_INT: i64 = (1 << 31) - 1;
pub const STATE_HISTORY_SIZE: i64 = 10;

// --- Interesting_Logger ---
pub const LOG_BUFFER_SIZE: i64 = 200;
pub const AFTERMATH_LOG_SIZE: i64 = 100;

// --- Selected log files---
pub const GZ_SUFFIX: &str = ".gz";
pub const LOGS_PATH: &str = "/var/log";
pub const LOGS_FILES: [&str; 6] = [
    "auth.log",
    "daemon.log",
    "kern.log",
    "messages",
    "syslog",
    "user.log",
];

/// A number of the limits: Python's `int` or `float`, as `const.py` writes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Num {
    Int(i64),
    Float(f64),
}

/// `LimitsFDM` (`:190-226`) and its subclasses: Generic FDM Limits object. The printer object info
/// (`id`, `name`, `type`, `version`, `subversion`) is only annotated on `LimitsFDM`, so `None` there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub class: &'static str,
    // --- Printer Object info ---
    pub id: Option<&'static str>,
    pub name: Option<&'static str>,
    pub type_: Option<i64>,
    pub version: Option<i64>,
    pub subversion: Option<i64>,
    // --- Hardware limits ---
    pub extrusion_min: i64,
    pub extrusion_max: i64,
    pub feedrate_e_min: i64,
    pub feedrate_e_max: i64,
    pub feedrate_x_min: i64,
    pub feedrate_x_max: i64,
    pub feedrate_y_min: i64,
    pub feedrate_y_max: i64,
    pub feedrate_z_min: i64,
    pub feedrate_z_max: i64,
    pub min_temp_nozzle_e: i64,
    pub position_x_min: i64,
    pub position_x_max: i64,
    pub position_y_min: i64,
    pub position_y_max: f64,
    pub position_z_min: f64,
    pub position_z_max: i64,
    pub print_flow_min: i64,
    pub print_flow_max: i64,
    pub print_speed_min: i64,
    pub print_speed_max: i64,
    pub temp_bed_min: i64,
    pub temp_bed_max: i64,
    pub temp_nozzle_min: i64,
    pub temp_nozzle_max: i64,
}

/// `LimitsFDM` (`:190-226`)
pub const LIMITS_FDM: Limits = Limits {
    class: "LimitsFDM",
    id: None,
    name: None,
    type_: None,
    version: None,
    subversion: None,
    extrusion_min: -10,
    extrusion_max: 100,
    feedrate_e_min: 0,
    feedrate_e_max: 100,
    feedrate_x_min: 0,
    feedrate_x_max: 2700,
    feedrate_y_min: 0,
    feedrate_y_max: 2700,
    feedrate_z_min: 0,
    feedrate_z_max: 1000,
    min_temp_nozzle_e: 170,
    position_x_min: 0,
    position_x_max: 255,
    position_y_min: -4,
    position_y_max: 212.5,
    position_z_min: 0.15,
    position_z_max: 210,
    print_flow_min: 10,
    print_flow_max: 999,
    print_speed_min: 10,
    print_speed_max: 999,
    temp_bed_min: 0,
    temp_bed_max: 125,
    temp_nozzle_min: 0,
    temp_nozzle_max: 305,
};

/// `LimitsMK25(LimitsFDM)`: Printer MK2.5 Limits object
pub const LIMITS_MK25: Limits = Limits {
    class: "LimitsMK25",
    id: Some("1.2.5"),
    name: Some("Original Prusa i3 MK2.5"),
    type_: Some(1),
    version: Some(2),
    subversion: Some(5),
    ..LIMITS_FDM
};

/// `LimitsMK25S(LimitsFDM)`: Printer MK2.5S Limits object
pub const LIMITS_MK25S: Limits = Limits {
    class: "LimitsMK25S",
    id: Some("1.2.6"),
    name: Some("Original Prusa i3 MK2.5S"),
    type_: Some(1),
    version: Some(2),
    subversion: Some(6),
    ..LIMITS_FDM
};

/// `LimitsMK3(LimitsFDM)`: Printer MK3 Limits object
pub const LIMITS_MK3: Limits = Limits {
    class: "LimitsMK3",
    id: Some("1.3.0"),
    name: Some("Original Prusa i3 MK3"),
    type_: Some(1),
    version: Some(3),
    subversion: Some(0),
    ..LIMITS_FDM
};

/// `LimitsMK3S(LimitsFDM)`: Printer MK3S Limits object
pub const LIMITS_MK3S: Limits = Limits {
    class: "LimitsMK3S",
    id: Some("1.3.1"),
    name: Some("Original Prusa i3 MK3S"),
    type_: Some(1),
    version: Some(3),
    subversion: Some(1),
    ..LIMITS_FDM
};

/// `PRINT_STATE_PAIRING` (`:270-275`)
pub const PRINT_STATE_PAIRING: [(&str, PrintState); 4] = [
    ("sdn_lfn", PrintState::SdPrinting),
    ("sd_paused", PrintState::SdPaused),
    ("serial_paused", PrintState::SerialPaused),
    ("no_print", PrintState::NotSdPrinting),
];

/// `PRINT_MODE_PAIRING` (`:277`)
pub const PRINT_MODE_PAIRING: [(&str, PrintMode); 2] =
    [("SILENT", PrintMode::Silent), ("NORMAL", PrintMode::Normal)];

/// `PRINT_MODE_ID_PAIRING` (`:279-283`)
pub const PRINT_MODE_ID_PAIRING: [(i64, PrintMode); 3] = [
    (0, PrintMode::Normal),
    (1, PrintMode::Silent),
    (2, PrintMode::Auto),
];

/// `SUPPORTED_PRINTERS` (`:286-288`): keys are the manufacturer ids, values are supported models
pub const SUPPORTED_PRINTERS: [(&str, &[&str]); 1] = [("2c99", &["0001", "0002"])];

pub const MMU_SLOTS: i64 = 5;

/// `MMU_PROGRESS_MAP` (`:292-320`)
pub const MMU_PROGRESS_MAP: [(&str, i64); 26] = [
    ("OK", 0),
    ("Engaging idler", 1),
    ("Disengaging idler", 2),
    ("Unloading to FINDA", 3),
    ("Unloading to pulley", 4),
    ("Feeding to FINDA", 5),
    ("Feeding to extruder", 6),
    ("Feeding to nozzle", 7),
    ("Avoiding grind", 8),
    ("ERR Disengaging idler", 10),
    ("ERR Engaging idler", 11),
    ("ERR Wait for User", 12),
    ("ERR Internal", 13),
    ("ERR Help filament", 14),
    ("ERR TMC failed", 15),
    ("Selecting fil. slot", 18),
    ("Preparing blade", 19),
    ("Pushing filament", 20),
    ("Performing cut", 21),
    ("Returning selector", 22),
    ("Ejecting filament", 24),
    ("Parking selector", 23),
    ("Retract from FINDA", 25),
    ("Homing", 26),
    ("Moving selector", 27),
    ("Feeding to FSensor", 28),
];

/// `MMU_ERROR_MAP` (`:322-399`), the source's comments kept
pub const MMU_ERROR_MAP: [(i64, i64); 44] = [
    (0x8001, 101), // FINDA didn't switch on -> FINDA didn't trigger
    (0x8002, 102), // FINDA didn't switch off -> FINDA filament stuck
    (0x8003, 103), // Filament sensor didn't switch on -> FSENSOR didn't trigger
    (0x8004, 104), // Filament sensor didn't switch off -> FSENSOR filament stuck
    (0x800b, 105), // MOVE_PULLEY_FAILED -> MECHANICAL pulley cannot move
    (0x8009, 106), // FSensor triggered too early -> MECHANICAL FSENSOR too early
    (0x800a, 107), // FINDA flickers -> MECHANICAL inspect FINDA
    (0x802a, 108), // LOAD_TO_EXTRUDER_FAILED -> Loading to extruder failed.
    //                Inspect the filament tip shape. Refine the sensor
    //                calibration, if needed
    (0x8007 | 0x0080, 115), // Selector homing failed
    (0x800b | 0x0080, 116), // Selector move failed
    (0x8007 | 0x0100, 125), // Idler homing failed
    (0x800b | 0x0100, 126), // Idler move failed
    (0xA000, 201),          // TMC_OVER_TEMPERATURE_WARN -> Temperature warning TMC pulley
    //                         too hot
    (0xC000, 202), // TMC_OVER_TEMPERATURE_ERROR -> Temperature TMC pulley
    //                overheat error

    // Temperature errors for the selector driver
    (0xA000 | 0x0080, 211), // Temperature warning TMC selector too hot
    (0xC000 | 0x0080, 212), // Temperature TMC selector overheat error
    // Temperature errors for the idler driver
    (0xA000 | 0x0100, 221), // Temperature warning TMC idler too hot
    (0xC000 | 0x0100, 222), // Temperature TMC idler overheat error
    // Electrical errors
    (0x8200, 301), // TMC_IOIN_MISMATCH -> Electrical TMC pulley driver error
    (0x8400, 302), // TMC_RESET -> Electrical TMC pulley driver reset
    (0x8800, 303), // TMC_UNDERVOLTAGE_ON_CHARGE_PUMP -> Electrical TMC
    //                pulley undervoltage error
    (0x9000, 304), // TMC_SHORT_TO_GROUND -> Electrical TMC pulley driver shorted
    (0xC200, 305), // ERR_ELECTRICAL_MMU_PULLEY_SELFTEST_FAILED -> Electrical
    //                TMC pulley selftest failed

    // Electrical errors for the selector driver
    (0x8200 | 0x0080, 311), // Electrical TMC selector driver error
    (0x8400 | 0x0080, 312), // Electrical TMC selector driver reset
    (0x8800 | 0x0080, 313), // Electrical TMC selector undervoltage error
    (0x9000 | 0x0080, 314), // Electrical TMC selector driver shorted
    (0xC200 | 0x0080, 315), // Electrical TMC selector selftest failed
    // Electrical errors for the idler driver
    (0x8200 | 0x0100, 321), // Electrical TMC idler driver error
    (0x8400 | 0x0100, 322), // Electrical TMC idler driver reset
    (0x8800 | 0x0100, 323), // Electrical TMC idler undervoltage error
    (0x9000 | 0x0100, 324), // Electrical TMC idler driver shorted
    (0xC200 | 0x0100, 325), // Electrical TMC idler selftest failed
    (0x0800d, 306),         // MMU MCU detected a 5V undervoltage. There might be an
    //                         issue with the electronics. Check the wiring and
    //                         connectors

    // Connectivity errors
    (0x802e, 401), // MMU not responding -> CONNECT MMU not responding
    (0x802d, 402), // MMU not responding correctly. Check the wiring and
    //                connectors

    // System errors
    (0x8005, 501), // Filament already loaded -> SYSTEM filament already loaded
    (0x8006, 502), // Invalid tool -> SYSTEM invalid tool
    (0x802b, 503), // QUEUE_FULL -> MMU Firmware internal error, please reset
    //                the MMU
    (0x802c, 504), // VERSION_MISMATCH -> The MMU firmware version is
    //                incompatible with the printer's FW. Update to compatible
    //                version
    (0x802f, 505), // PROTOCOL_ERROR -> Internal runtime error. Try resetting
    //                the MMU or updating the firmware
    (0x8008, 506), // FINDA_VS_EEPROM_DISCREPANCY -> Unload manually
    (0x800c, 507), // Filament was ejected -> SYSTEM filament ejected
    (0x8029, 508), // FILAMENT_CHANGE -> SYSTEM filament change
];

#[cfg(test)]
mod tests;
