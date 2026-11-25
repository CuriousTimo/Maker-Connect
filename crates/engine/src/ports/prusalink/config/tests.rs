//! `config.py` against PrusaLink's own: the same examples through `driver_config.py` (the real
//! `Config` and `Settings`) and through ours, compared as JSON.

use super::*;
use crate::ports::prusalink::python;
use serde_json::{Value as Json, json};

const DRIVER: &str = include_str!("../driver_config.py");

fn model(m: &Model) -> Json {
    Json::Array(
        m.0.iter()
            .map(|(k, v)| json!([k, v.type_name(), v.to_string()]))
            .collect(),
    )
}

fn error(err: &Error) -> Json {
    json!({"error": [err.class, err.message]})
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc-prusalink-config-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn args(path: &Path, given: &Json) -> Args {
    let mut args = Args::fake(path);
    let Some(given) = given.as_object() else {
        return args;
    };
    for (key, value) in given {
        match key.as_str() {
            "debug" => args.debug = value.as_bool().unwrap(),
            "foreground" => args.foreground = value.as_bool().unwrap(),
            "info" => args.info = value.as_bool().unwrap(),
            "pidfile" => args.pidfile = value.as_str().map(str::to_owned),
            "address" => args.address = value.as_str().map(str::to_owned),
            "serial_port" => args.serial_port = value.as_str().map(str::to_owned),
            "tcp_port" => args.tcp_port = value.as_i64(),
            "printer_number" => args.printer_number = value.as_i64(),
            "link_info" => args.link_info = value.as_bool(),
            "module_log_level" => {
                args.module_log_level = value.as_array().map(|levels| {
                    levels
                        .iter()
                        .map(|l| l.as_str().unwrap().to_owned())
                        .collect()
                })
            }
            other => panic!("no such argument: {other}"),
        }
    }
    args
}

fn ours_config(case: &Json, d: &Path) -> Json {
    let path = d.join("prusalink.ini");
    if let Some(ini) = case["ini"].as_str() {
        std::fs::write(&path, ini).unwrap();
    }
    let mut cfg = match Config::new(&args(&path, &case["args"])) {
        Ok(cfg) => cfg,
        Err(err) => return error(&err),
    };
    let handler = &cfg.configured_handler;
    let mut result = json!({
        "debug": cfg.debug,
        "daemon": model(&cfg.daemon),
        "log_settings": cfg.log_settings.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
        "http": model(&cfg.http),
        "printer": model(&cfg.printer),
        "cameras": model(&cfg.cameras),
        "handler": [handler.kind, handler.address, handler.format],
    });
    match cfg.update_sections() {
        Ok(()) => result["written"] = json!(cfg.parser.write()),
        Err(err) => result["written_error"] = json!([err.class, err.message]),
    }
    result
}

fn ours_settings(case: &Json, d: &Path) -> Json {
    let path = d.join("prusa_printer_settings.ini");
    if let Some(text) = case["settings"].as_str() {
        std::fs::write(&path, text).unwrap();
    }
    let mut result = match Settings::new(&path) {
        Ok(s) => json!({
            "printer": model(&s.printer),
            "network": model(&s.network),
            "service_connect": model(&s.service_connect),
            "service_local": model(&s.service_local),
            "is_wizard_needed": s.is_wizard_needed(),
            "use_connect": s.use_connect(),
        }),
        Err(err) => error(&err),
    };
    result["file"] = json!(std::fs::read_to_string(&path).ok());
    result
}

/// The examples: `ini` (a `prusalink.ini`, `null`: none) with `args` for `Config`, or
/// `settings` (a `prusa_printer_settings.ini`) for `Settings`.
fn cases() -> Json {
    json!([
        {"ini": null},
        {"ini": ""},
        {"ini": "[daemon]\ndata_dir = /tmp/pl\npid_file = x.pid\nprinter_number = 2\n\n[http]\naddress = 1.2.3.4\nport =  8081 \nlink_info = yes\n\n[printer]\nport = /dev/ttyACM0\nbaudrate = 250000\nreset_disabling = on\ndirectory = gcodes\n\n[cameras]\nauto_detect = off\n\n[log]\nurllib3 = DEBUG\nprusa.link = INFO\n"},
        {"ini": "[http]\nport = abc\n"},
        {"ini": "[http]\nport = +8_080\n"},
        {"ini": "[http]\nport = 1_\n"},
        {"ini": "[printer]\nreset_disabling = maybe\n"},
        {"ini": "[printer]\nreset_disabling = TRUE\n"},
        {"ini": "[DEFAULT]\nbase = /srv\n\n[daemon]\ndata_dir = %(base)s/link\npid_file = 100%%.pid\n"},
        {"ini": "[daemon]\ndata_dir = /tmp\n\n[printer]\ndirectory = %(missing)s\n"},
        {"ini": "[daemon]\nuser = a%b\n"},
        {"ini": "[daemon]\nuser = a%(b\n"},
        {"ini": "[DEFAULT]\na = %(b)s\nb = %(a)s\n\n[daemon]\ndata_dir = %(a)s\n"},
        {"ini": "[log]\nx = TRACE\n"},
        {"ini": "[DEFAULT]\nlevel = WARNING\n\n[log]\nprusa = %(level)s\n"},
        {"ini": "[logging]\nformat = %(message)s\n"},
        {"ini": "[logging]\nformat = %%(message)s at %%(asctime)s\n"},
        {"ini": "no header\n"},
        {"ini": "[daemon]\ndata_dir =\n"},
        {"ini": "[daemon]\ndata_dir = rel/dir\n"},
        {"ini": "[printer]\ndirectory = /\n"},
        {"ini": "", "args": {"pidfile": "rel.pid", "printer_number": 3, "address": "9.9.9.9",
                             "tcp_port": 9000, "link_info": true, "serial_port": "/dev/x",
                             "module_log_level": ["a=INFO", "urllib3=ERROR"], "debug": true}},
        {"ini": "", "args": {"pidfile": "", "address": "", "tcp_port": 0, "link_info": false,
                             "serial_port": ""}},
        {"ini": "", "args": {"module_log_level": ["a"]}},
        {"ini": "", "args": {"module_log_level": ["a=b=c"]}},
        {"settings": null},
        {"settings": ""},
        {"settings": "[printer]\ntype = MK3S\nname =   My MK3  \nlocation = Lab\nfarm_mode = yes\n\n[network]\nhostname = mk3\n\n[service::connect]\nhostname = 1.2.3.4\ntls = False\nport = 8460\ntoken = abc\n\n[service::local]\nenable = 0\nusername = u\ndigest = d\napi_key =   key  \n\n[other]\nx = 1\n"},
        {"settings": "[printer]\ntype = MINI\n"},
        {"settings": "[service::connect]\ntls = maybe\n"},
        {"settings": "[service::connect]\nport = 80a\n"},
        {"settings": "[service::connect]\ntoken = a%b\n[printer]\nname = %(x)s\n"},
        {"settings": "[DEFAULT]\nname = shared\n\n[printer]\ntype = MK2.5\n"},
        {"settings": "[printer]\ntype = MK3\n\n[service::local]\nusername = u\ndigest = d\n"},
    ])
}

#[test]
fn config_and_settings_are_prusalinks() {
    let cases = cases();
    let Some(theirs) = python::run(DRIVER, &cases) else {
        return;
    };
    assert_eq!(theirs["log_foreground"], json!(LOG_FORMAT_FOREGROUND));
    let theirs = theirs["cases"].as_array().unwrap();
    let cases = cases.as_array().unwrap();
    assert_eq!(theirs.len(), cases.len());
    for (i, (case, theirs)) in cases.iter().zip(theirs).enumerate() {
        let d = dir(&i.to_string());
        let ours = if case.get("settings").is_some() {
            ours_settings(case, &d)
        } else {
            ours_config(case, &d)
        };
        let _ = std::fs::remove_dir_all(&d);
        // the temporary folder, named in a parsing error, as DIR
        let folder = json!(d.to_string_lossy()).to_string();
        let ours: Json = serde_json::from_str(
            &ours
                .to_string()
                .replace(&folder[1..folder.len() - 1], "DIR"),
        )
        .unwrap();
        assert_eq!(&ours, theirs, "case {i}: {case}");
    }
}

#[test]
fn the_conf_types_are_constpys() {
    let keys: Vec<&str> = PRINTER_CONF_TYPES.iter().map(|(k, _)| *k).collect();
    assert_eq!(keys, ["MK2.5", "MK2.5S", "MK3", "MK3S"]);
    assert_eq!(sanitize_api_key("  a b "), "a b");
    assert_eq!(sanitize_api_key(""), "");
}
