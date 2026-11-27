use super::*;
use crate::ports::prusalink::python;
use maker_connect_sdk::conditions::CondState;
use serde_json::{Value, json};
use std::sync::{LazyLock, Mutex, MutexGuard};

/// The conditions are one process-wide store with the SDK's chain: one printer module for every
/// test, made once, one test at a time.
static MODULE: LazyLock<Mutex<Conditions>> =
    LazyLock::new(|| Mutex::new(Conditions::new().expect("the first module")));

fn module() -> MutexGuard<'static, Conditions> {
    MODULE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Every class of the module, in the source's order: `LinkError` first.
static ALL: [&ErrorClass; 51] = [
    &LINK_ERROR,
    &BAD_REQUEST_ERROR,
    &TEMPERATURE_TOO_LOW,
    &TEMPERATURE_TOO_HIGH,
    &VALUE_TOO_LOW,
    &VALUE_TOO_HIGH,
    &CANT_MOVE_AXIS,
    &CANT_MOVE_AXIS_Z,
    &DESTINATION_SAME_AS_SOURCE,
    &NO_FILE_IN_REQUEST,
    &FILE_SIZE_MISMATCH,
    &INVALID_INI_FILE_FORMAT,
    &INVALID_BOOLEAN_HEADER,
    &FORBIDDEN_CHARACTERS,
    &FILENAME_TOO_LONG,
    &FOLDERNAME_TOO_LONG,
    &FILE_UPLOAD_FAILED,
    &CANT_CONNECT,
    &CANT_RESOLVE_HOSTNAME,
    &NOT_SUPPORTED_FILE_TYPE,
    &FORBIDDEN_ERROR,
    &NOT_FOUND_ERROR,
    &NOT_CURRENT_JOB,
    &GONE_ERROR,
    &THUMBNAIL_UNAVAILABLE,
    &FILE_NOT_FOUND,
    &FOLDER_NOT_FOUND,
    &LOCATION_NOT_FOUND,
    &CONFLICT_ERROR,
    &DIRECTORY_NOT_EMPTY,
    &CURRENTLY_PRINTING,
    &NOT_STATE_TO_PRINT,
    &NOT_PRINTING,
    &NOT_PAUSED,
    &FILE_CURRENTLY_PRINTED,
    &TRANSFER_CONFLICT,
    &TRANSFER_STOPPED,
    &UNAVAILABLE_UPDATE,
    &UNABLE_TO_UPDATE,
    &LENGTH_REQUIRED,
    &ENTITY_TOO_LARGE,
    &FILE_ALREADY_EXISTS,
    &FOLDER_ALREADY_EXISTS,
    &STORAGE_NOT_EXIST,
    &SD_CARD_READ_ONLY,
    &SD_CARD_NOT_SUPPORTED,
    &UNSUPPORTED_MEDIA_ERROR,
    &INTERNAL_SERVER_ERROR,
    &RESPONSE_TIMEOUT,
    &PRINTER_UNAVAILABLE,
    &REQUEST_TIMEOUT,
];

fn err(error: PyErr) -> Value {
    json!([error.class_name(), error.message()])
}

fn headers(error: &LinkError) -> Value {
    Value::Object(
        error
            .gen_headers()
            .into_iter()
            .map(|(k, v)| (k.to_owned(), json!(v)))
            .collect(),
    )
}

fn instance(class: &'static ErrorClass, details: &str) -> Value {
    match LinkError::new(class, details) {
        Ok(mut error) => {
            let mut out = vec![
                json!(error.details),
                json!(error.path),
                json!(error.template),
                json!([error.args.0]),
                json!(error.url),
                headers(&error),
            ];
            error.url = "http://link/error/x".to_owned();
            out.push(headers(&error));
            json!(out)
        }
        Err(e) => err(e),
    }
}

fn describe(class: &'static ErrorClass) -> Value {
    json!([
        class.name,
        class.parent.map_or("RuntimeError", |p| p.name),
        class.title,
        class.text,
        class.id,
        class.status_code,
        class.use_basic_template,
        instance(class, ""),
        instance(class, "it is gone")
    ])
}

fn status_json(status: Status) -> Value {
    json!({"ok": status.ok, "message": status.message})
}

/// `status()` sorted by name (difference 2), `printer_status()`, `connect_status()`, the
/// SDK's `COND_TRACKER`'s worst
fn snapshot(module: &Conditions, settings: &Settings) -> Vec<Value> {
    let mut status = module.status();
    status.sort();
    vec![
        json!(
            status
                .into_iter()
                .map(|(k, (s, m))| json!([k, [s, m]]))
                .collect::<Vec<_>>()
        ),
        status_json(module.printer_status()),
        status_json(module.connect_status(settings)),
        json!(CHAIN.tracker.get_worst().map(|c| c.name())),
    ]
}

fn by_name(module: &Conditions, name: &str) -> Condition {
    match name {
        "ROOT_COND" => module.root_cond,
        "DEVICE" => module.device,
        "PHY" => module.phy,
        "LAN" => module.lan,
        "SERIAL" => module.serial,
        "RPI_ENABLED" => module.rpi_enabled,
        "ID" => module.id,
        "UPGRADED" => module.upgraded,
        "FW" => module.fw,
        "SN" => module.sn,
        "JOB_ID" => module.job_id,
        "HW" => module.hw,
        "INTERNET" => CHAIN.internet,
        "HTTP" => CHAIN.http,
        "TOKEN" => CHAIN.token,
        "API" => CHAIN.api,
        other => panic!("{other}"),
    }
}

const NAMES: [&str; 16] = [
    "ROOT_COND",
    "DEVICE",
    "PHY",
    "LAN",
    "SERIAL",
    "RPI_ENABLED",
    "ID",
    "UPGRADED",
    "FW",
    "SN",
    "JOB_ID",
    "HW",
    "INTERNET",
    "HTTP",
    "TOKEN",
    "API",
];

fn ops() -> Vec<Value> {
    let mut seed: u64 = 244;
    let mut next = |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    let mut ops = vec![];
    for _ in 0..200 {
        match next(12) {
            0..=8 => {
                let name = NAMES[next(16) as usize];
                let state = ["OK", "NOK", "NOK", "UNKNOWN"][next(4) as usize];
                ops.push(json!(["set", name, state]));
            }
            9..=10 => ops.push(json!(["use_connect", next(2) == 0])),
            _ => ops.push(json!(["settings", next(2)])),
        }
    }
    // the end: everything fixed, children first, then Connect off and on
    for name in NAMES.iter().rev() {
        ops.push(json!(["set", name, "OK"]));
    }
    ops.push(json!(["use_connect", true]));
    ops.push(json!(["settings", 1]));
    ops.push(json!(["settings", 0]));
    ops
}

/// PrusaLink's `conditions.py` and ours: every one of the 51 classes (its base, the class
/// attributes as Python resolves them, an instance without and with details, its headers before
/// and after a url), and the tree through 219 ops: 200 at random (a condition of PrusaLink's tree
/// or of the SDK's chain set OK, NOK or back to UNKNOWN, `use_connect_errors` on and off, a
/// settings file with and without a token), then everything fixed, children first, and Connect off
/// and on. After each the result, `status()`, `printer_status()`, `connect_status()` and
/// `COND_TRACKER`'s worst; and the SDK's `INTERNET` joined to a second `Lan`.
#[test]
fn identical_to_prusalink() {
    let module = module();
    let directory = std::env::temp_dir().join(format!("mc-conditions-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let files = [
        ("token.ini", "[service::connect]\ntoken = secret\n"),
        ("none.ini", "[service::connect]\n"),
    ];
    let paths: Vec<String> = files
        .iter()
        .map(|(name, text)| {
            let path = directory.join(name);
            std::fs::write(&path, text).unwrap();
            path.to_str().unwrap().to_owned()
        })
        .collect();
    let ops = ops();
    let Some(theirs) = python::run(
        include_str!("../driver_conditions.py"),
        &json!({"settings": paths, "ops": ops}),
    ) else {
        std::fs::remove_dir_all(&directory).unwrap();
        return;
    };

    let ours: Vec<Value> = ALL.iter().map(|c| describe(c)).collect();
    let their_errors = theirs["errors"].as_array().unwrap();
    assert_eq!(ours.len(), their_errors.len(), "the classes");
    for (ours, theirs) in ours.iter().zip(their_errors) {
        assert_eq!(ours.to_string(), theirs.to_string());
    }

    let settings_of = |i: usize| Settings::new(std::path::Path::new(&paths[i])).unwrap();
    let mut settings = settings_of(0);
    let mut tree = vec![json!(snapshot(&module, &settings))];
    for op in &ops {
        let result = match op[0].as_str().unwrap() {
            "set" => {
                let state = match op[2].as_str().unwrap() {
                    "OK" => CondState::Ok,
                    "NOK" => CondState::Nok,
                    _ => CondState::Unknown,
                };
                match by_name(&module, op[1].as_str().unwrap()).set_state(state) {
                    Ok(()) => Value::Null,
                    Err(e) => json!(["ValueError", e.to_string()]),
                }
            }
            "use_connect" => {
                module.use_connect_errors(op[1].as_bool().unwrap());
                Value::Null
            }
            _ => {
                settings = settings_of(op[1].as_u64().unwrap() as usize);
                Value::Null
            }
        };
        let mut row = vec![result];
        row.extend(snapshot(&module, &settings));
        tree.push(json!(row));
    }
    std::fs::remove_dir_all(&directory).unwrap();
    let their_tree = theirs["tree"].as_array().unwrap();
    assert_eq!(tree.len(), their_tree.len());
    for ((op, ours), theirs) in std::iter::once(&Value::Null)
        .chain(&ops)
        .zip(&tree)
        .zip(their_tree)
    {
        assert_eq!(ours.to_string(), theirs.to_string(), "{op}");
    }

    // the walk reaches each answer of the status functions and the refused reset
    let all = json!(tree).to_string();
    for seen in [
        r#"{"ok":true,"message":"OK"}"#,
        r#"{"ok":true,"message":"Connect isn't configured"}"#,
        r#"{"ok":false,"message":"Serial device does not exist"}"#,
        r#""ok":false"#,
        "Can't reset a condition to unknown",
    ] {
        assert!(all.contains(seen), "{seen}");
    }

    let lan = Condition::new("Lan", "Eth|WLAN has no IP address", None, None, 0).unwrap();
    let second = match CHAIN.internet.set_parent(lan) {
        Ok(()) => Value::Null,
        Err(e) => json!(["ValueError", e.to_string()]),
    };
    assert_eq!(second, theirs["second"]);
}

/// A second printer module: the SDK's chain is the first one's (difference 1).
#[test]
fn a_second_module_fails_at_the_sdks_chain() {
    let first = module();
    assert!(matches!(
        Conditions::new(),
        Err(ConditionError::ReplaceParent)
    ));
    assert_eq!(CHAIN.internet.parent(), Some(first.lan));
}

// PrusaLink's own tests (`tests/test_conditions.py`, `171148ce`), test by test; not its four of
// `set_url`, `json_response` and `text_response` (not covered, see the module).

/// The group classes (BadRequestError, ...) only set the status code
fn all_errors() -> Vec<&'static ErrorClass> {
    ALL[1..]
        .iter()
        .copied()
        .filter(|c| c.title.is_some())
        .collect()
}

/// Every error has everything needed for a response
#[test]
fn test_error_is_complete() {
    for error in all_errors() {
        assert!(!error.title.unwrap().is_empty(), "{}", error.name);
        assert!(!error.text.unwrap().is_empty());
        assert!((400..600).contains(&error.status_code.unwrap()));
        let instance = LinkError::new(error, "").unwrap();
        assert_eq!(instance.args, (error.text.unwrap().to_owned(),));
        assert!(!instance.template.is_empty());
    }
}

/// Ids make up the urls, so they must not collide
#[test]
fn test_error_ids_are_unique() {
    // Subclasses may inherit the id of their parent on purpose
    let ids: Vec<&str> = all_errors()
        .into_iter()
        .filter(|e| e.id.is_some() && e.parent.is_none_or(|p| p.id != e.id))
        .map(|e| e.id.unwrap())
        .filter(|id| !id.is_empty())
        .collect();
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(ids.len(), unique.len());
}

/// Details are optional, templates follow the basic flag
#[test]
fn test_details_and_template() {
    let error = LinkError::new(&FILE_NOT_FOUND, "it is gone").unwrap();
    assert_eq!(error.details.as_deref(), Some("it is gone"));
    assert_eq!(error.template, "error.html");
    assert_eq!(
        error.path.as_deref(),
        Some(format!("/error/{}", FILE_NOT_FOUND.id.unwrap()).as_str())
    );
    assert_eq!(LinkError::new(&FILE_NOT_FOUND, "").unwrap().details, None);
}

/// Not basic template is named after the id
#[test]
fn test_custom_template() {
    static CUSTOM: ErrorClass = ErrorClass {
        name: "Custom",
        parent: Some(&LINK_ERROR),
        title: Some("t"),
        text: Some("x"),
        id: Some("custom"),
        status_code: Some(400),
        use_basic_template: false,
    };
    assert_eq!(
        LinkError::new(&CUSTOM, "").unwrap().template,
        "error-custom.html"
    );
}

/// Without a broken condition the printer is OK
///
/// The module is as made, or as `identical_to_prusalink` leaves it: everything fixed.
#[test]
fn test_printer_status_ok() {
    assert_eq!(module().printer_status(), Status::ok_msg());
}

/// The status lists every condition by its name
#[test]
fn test_status() {
    let result = module().status();
    assert!(result.iter().any(|(name, _)| name == "Root"));
    assert!(result.iter().any(|(name, _)| name == "Port"));
}
