use super::*;
use crate::ports::prusalink::python;
use serde_json::{Value, json};

fn enum_(name: &str, value: Value) -> Value {
    json!({"enum": name, "value": value})
}

fn printer_type(t: PrinterType) -> Value {
    let (a, b, c) = t.value();
    enum_("PrinterType", json!({"tuple": [a, b, c]}))
}

fn state(s: State) -> Value {
    enum_("State", json!(s.value()))
}

/// A Python `set`: its members sorted by their JSON text, as the driver does.
fn set(items: Vec<Value>) -> Value {
    let mut items = items;
    items.sort_by_key(|item| python_dumps(item));
    json!({ "set": items })
}

/// `json.dumps` with its default separators (", " and ": "), for the order of a set.
fn python_dumps(value: &Value) -> String {
    match value {
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(python_dumps)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}: {}", Value::from(k.as_str()), python_dumps(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => other.to_string(),
    }
}

fn states(states: &[State]) -> Value {
    set(states.iter().map(|s| state(*s)).collect())
}

fn dict(pairs: Vec<(Value, Value)>) -> Value {
    json!({"dict": pairs.into_iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>()})
}

fn limits(l: &Limits) -> Value {
    let mut attrs = serde_json::Map::new();
    let mut put = |name: &str, value: Value| {
        attrs.insert(name.to_owned(), value);
    };
    put("extrusion_max", json!(l.extrusion_max));
    put("extrusion_min", json!(l.extrusion_min));
    put("feedrate_e_max", json!(l.feedrate_e_max));
    put("feedrate_e_min", json!(l.feedrate_e_min));
    put("feedrate_x_max", json!(l.feedrate_x_max));
    put("feedrate_x_min", json!(l.feedrate_x_min));
    put("feedrate_y_max", json!(l.feedrate_y_max));
    put("feedrate_y_min", json!(l.feedrate_y_min));
    put("feedrate_z_max", json!(l.feedrate_z_max));
    put("feedrate_z_min", json!(l.feedrate_z_min));
    if let Some(id) = l.id {
        put("id", json!(id));
    }
    put("min_temp_nozzle_e", json!(l.min_temp_nozzle_e));
    if let Some(name) = l.name {
        put("name", json!(name));
    }
    put("position_x_max", json!(l.position_x_max));
    put("position_x_min", json!(l.position_x_min));
    put("position_y_max", json!(l.position_y_max));
    put("position_y_min", json!(l.position_y_min));
    put("position_z_max", json!(l.position_z_max));
    put("position_z_min", json!(l.position_z_min));
    put("print_flow_max", json!(l.print_flow_max));
    put("print_flow_min", json!(l.print_flow_min));
    put("print_speed_max", json!(l.print_speed_max));
    put("print_speed_min", json!(l.print_speed_min));
    if let Some(subversion) = l.subversion {
        put("subversion", json!(subversion));
    }
    put("temp_bed_max", json!(l.temp_bed_max));
    put("temp_bed_min", json!(l.temp_bed_min));
    put("temp_nozzle_max", json!(l.temp_nozzle_max));
    put("temp_nozzle_min", json!(l.temp_nozzle_min));
    if let Some(type_) = l.type_ {
        put("type", json!(type_));
    }
    if let Some(version) = l.version {
        put("version", json!(version));
    }
    json!({"class": l.class, "attrs": attrs})
}

/// Every name `const.py` defines and its value as ours makes it, in the driver's form; the
/// three of the module's "not covered" give only what can be the same.
fn ours() -> Vec<(&'static str, Value)> {
    let strs = |items: &[&str]| json!(items);
    vec![
        ("instance_id", json!({"type": "UUID"})),
        ("HEADER_DATETIME_FORMAT", json!(HEADER_DATETIME_FORMAT)),
        (
            "PRINTER_TYPES",
            dict(
                PRINTER_TYPES
                    .iter()
                    .map(|(k, v)| (json!(k), printer_type(*v)))
                    .collect(),
            ),
        ),
        ("MMU3_TYPE_CODE", json!(MMU3_TYPE_CODE)),
        (
            "PRINTER_CONF_TYPES",
            dict(
                PRINTER_CONF_TYPES
                    .iter()
                    .map(|(k, v)| (json!(k), printer_type(*v)))
                    .collect(),
            ),
        ),
        ("DATA_PATH", Value::Null),
        ("BASE_STATES", states(&BASE_STATES)),
        ("PRINTING_STATES", states(&PRINTING_STATES)),
        (
            "MK25_PRINTERS",
            set(MK25_PRINTERS
                .iter()
                .map(|t| {
                    let (a, b, c) = t.value();
                    json!({"tuple": [a, b, c]})
                })
                .collect()),
        ),
        ("JOB_STARTING_STATES", states(&JOB_STARTING_STATES)),
        ("JOB_ENDING_STATES", states(&JOB_ENDING_STATES)),
        ("JOB_DESTROYING_STATES", states(&JOB_DESTROYING_STATES)),
        ("JITTER_THRESHOLD", json!(JITTER_THRESHOLD)),
        ("PRUSA_VENDOR_ID", json!(PRUSA_VENDOR_ID)),
        ("TELEMETRY_IDLE_INTERVAL", json!(TELEMETRY_IDLE_INTERVAL)),
        (
            "TELEMETRY_PRINTING_INTERVAL",
            json!(TELEMETRY_PRINTING_INTERVAL),
        ),
        (
            "TELEMETRY_SLEEPING_INTERVAL",
            json!(TELEMETRY_SLEEPING_INTERVAL),
        ),
        ("TELEMETRY_SLEEP_AFTER", json!(TELEMETRY_SLEEP_AFTER)),
        (
            "TELEMETRY_REFRESH_INTERVAL",
            json!(TELEMETRY_REFRESH_INTERVAL),
        ),
        ("FAST_POLL_INTERVAL", json!(FAST_POLL_INTERVAL)),
        ("SLOW_POLL_INTERVAL", json!(SLOW_POLL_INTERVAL)),
        ("VERY_SLOW_POLL_INTERVAL", json!(VERY_SLOW_POLL_INTERVAL)),
        ("IP_UPDATE_INTERVAL", json!(IP_UPDATE_INTERVAL)),
        ("QUIT_INTERVAL", json!(QUIT_INTERVAL)),
        ("SD_INTERVAL", json!(SD_INTERVAL)),
        ("SD_FILESCAN_INTERVAL", json!(SD_FILESCAN_INTERVAL)),
        ("DIR_RESCAN_INTERVAL", json!(DIR_RESCAN_INTERVAL)),
        ("PRINTER_BOOT_WAIT", json!(PRINTER_BOOT_WAIT)),
        ("SEND_INFO_RETRY", json!(SEND_INFO_RETRY)),
        ("SERIAL_REOPEN_TIMEOUT", json!(SERIAL_REOPEN_TIMEOUT)),
        ("REPORTING_TIMEOUT", json!(REPORTING_TIMEOUT)),
        ("FW_MESSAGE_TIMEOUT", json!(FW_MESSAGE_TIMEOUT)),
        ("STATE_CHANGE_TIMEOUT", json!(STATE_CHANGE_TIMEOUT)),
        ("IP_WRITE_TIMEOUT", json!(IP_WRITE_TIMEOUT)),
        ("SN_OBTAIN_INTERVAL", json!(SN_OBTAIN_INTERVAL)),
        ("EXIT_TIMEOUT", json!(EXIT_TIMEOUT)),
        ("ERROR_REASON_TIMEOUT", json!(ERROR_REASON_TIMEOUT)),
        ("PATH_WAIT_TIMEOUT", json!(PATH_WAIT_TIMEOUT)),
        ("SLEEP_SCREEN_TIMEOUT", json!(SLEEP_SCREEN_TIMEOUT)),
        ("SELF_PING_TIMEOUT", json!(SELF_PING_TIMEOUT)),
        ("SELF_PING_RETRY_INTERVAL", json!(SELF_PING_RETRY_INTERVAL)),
        ("ATTENTION_CLEAR_INTERVAL", json!(ATTENTION_CLEAR_INTERVAL)),
        ("CAMERA_INIT_DELAY", json!(CAMERA_INIT_DELAY)),
        ("CAMERA_SCAN_INTERVAL", json!(CAMERA_SCAN_INTERVAL)),
        ("CAMERA_REGISTER_TIMEOUT", json!(CAMERA_REGISTER_TIMEOUT)),
        ("TIME_FOR_SNAPSHOT", json!(TIME_FOR_SNAPSHOT)),
        ("PRINT_END_TIMEOUT", json!(PRINT_END_TIMEOUT)),
        ("KEEPALIVE_INTERVAL", json!(KEEPALIVE_INTERVAL)),
        ("PP_MOVES_DELAY", json!(PP_MOVES_DELAY)),
        ("LCD_QUEUE_SIZE", json!(LCD_QUEUE_SIZE)),
        ("RX_SIZE", json!(RX_SIZE)),
        ("SERIAL_QUEUE_TIMEOUT", json!(SERIAL_QUEUE_TIMEOUT)),
        (
            "SERIAL_QUEUE_MONITOR_INTERVAL",
            json!(SERIAL_QUEUE_MONITOR_INTERVAL),
        ),
        ("HISTORY_LENGTH", json!(HISTORY_LENGTH)),
        ("QUEUE_SIZE", json!(QUEUE_SIZE)),
        ("HEAP_RATIO", json!(HEAP_RATIO)),
        ("IGNORE_ABOVE", json!(IGNORE_ABOVE)),
        ("DEFAULT_THRESHOLD", json!(DEFAULT_THRESHOLD)),
        ("USE_DYNAMIC_THRESHOLD", json!(USE_DYNAMIC_THRESHOLD)),
        ("STATS_EVERY", json!(STATS_EVERY)),
        ("TAIL_COMMANDS", json!(TAIL_COMMANDS)),
        ("PRINT_QUEUE_SIZE", json!(PRINT_QUEUE_SIZE)),
        ("MAX_FILENAME_LENGTH", json!(MAX_FILENAME_LENGTH)),
        ("SD_STORAGE_NAME", json!(SD_STORAGE_NAME)),
        ("BLACKLISTED_TYPES", strs(&BLACKLISTED_TYPES)),
        ("BLACKLISTED_PATHS", strs(&BLACKLISTED_PATHS)),
        ("BLACKLISTED_NAMES", strs(&BLACKLISTED_NAMES)),
        (
            "SFN_TO_LFN_EXTENSIONS",
            dict(
                SFN_TO_LFN_EXTENSIONS
                    .iter()
                    .map(|(k, v)| (json!(k), json!(v)))
                    .collect(),
            ),
        ),
        ("RESET_PIN", json!(RESET_PIN)),
        ("SUPPORTED_FIRMWARE", json!(SUPPORTED_FIRMWARE)),
        ("MINIMAL_FIRMWARE", json!({ "version": MINIMAL_FIRMWARE })),
        ("MAX_INT", json!(MAX_INT)),
        ("STATE_HISTORY_SIZE", json!(STATE_HISTORY_SIZE)),
        ("LOG_BUFFER_SIZE", json!(LOG_BUFFER_SIZE)),
        ("AFTERMATH_LOG_SIZE", json!(AFTERMATH_LOG_SIZE)),
        ("GZ_SUFFIX", json!(GZ_SUFFIX)),
        ("LOGS_PATH", json!(LOGS_PATH)),
        ("LOGS_FILES", json!({ "tuple": LOGS_FILES })),
        ("LimitsFDM", limits(&LIMITS_FDM)),
        ("LimitsMK25", limits(&LIMITS_MK25)),
        ("LimitsMK25S", limits(&LIMITS_MK25S)),
        ("LimitsMK3", limits(&LIMITS_MK3)),
        ("LimitsMK3S", limits(&LIMITS_MK3S)),
        (
            "PRINT_STATE_PAIRING",
            dict(
                PRINT_STATE_PAIRING
                    .iter()
                    .map(|(k, v)| (json!(k), enum_("PrintState", json!(v.value()))))
                    .collect(),
            ),
        ),
        (
            "PRINT_MODE_PAIRING",
            dict(
                PRINT_MODE_PAIRING
                    .iter()
                    .map(|(k, v)| (json!(k), enum_("PrintMode", json!(v.value()))))
                    .collect(),
            ),
        ),
        (
            "PRINT_MODE_ID_PAIRING",
            dict(
                PRINT_MODE_ID_PAIRING
                    .iter()
                    .map(|(k, v)| (json!(k), enum_("PrintMode", json!(v.value()))))
                    .collect(),
            ),
        ),
        (
            "SUPPORTED_PRINTERS",
            dict(
                SUPPORTED_PRINTERS
                    .iter()
                    .map(|(k, v)| (json!(k), set(v.iter().map(|m| json!(m)).collect())))
                    .collect(),
            ),
        ),
        ("MMU_SLOTS", json!(MMU_SLOTS)),
        (
            "MMU_PROGRESS_MAP",
            dict(
                MMU_PROGRESS_MAP
                    .iter()
                    .map(|(k, v)| (json!(k), json!(v)))
                    .collect(),
            ),
        ),
        (
            "MMU_ERROR_MAP",
            dict(
                MMU_ERROR_MAP
                    .iter()
                    .map(|(k, v)| (json!(k), json!(v)))
                    .collect(),
            ),
        ),
    ]
}

/// PrusaLink's own `const.py`, imported: every top-level name of its source, in its order, and
/// each value as Python builds it, the same as ours. `DATA_PATH` is where the package lies (the
/// module's "not covered"): only that it ends in `prusa/link/data`.
#[test]
fn identical_to_prusalink() {
    let Some(theirs) = python::run(
        include_str!("../driver_constants.py"),
        &json!({"modules": ["prusa.link.const"]}),
    ) else {
        return;
    };
    let theirs = &theirs["prusa.link.const"];
    let ours = ours();
    let names: Vec<&str> = ours.iter().map(|(name, _)| *name).collect();
    let their_names: Vec<&str> = theirs["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert_eq!(names, their_names, "the names, in the source's order");
    for (name, value) in &ours {
        let their = &theirs["values"][*name];
        if *name == "DATA_PATH" {
            let path = their.as_str().unwrap().replace('\\', "/");
            assert!(path.ends_with("prusa/link/data"), "{path}");
            continue;
        }
        assert_eq!(value, their, "{name}");
    }
}
