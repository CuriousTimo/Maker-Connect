use super::*;
use crate::ports::prusalink::config::{Args, Value as ConfigValue};
use crate::ports::prusalink::model_classes::Slot;
use crate::ports::prusalink::python;
use serde_json::json;

fn result(r: Result<Value, PyErr>) -> Value {
    match r {
        Ok(value) => json!({ "ok": value }),
        Err(error) => json!({"error": [error.class_name(), error.message()]}),
    }
}

fn individual(slot: &IndividualSlot) -> Value {
    Value::Object(slot.dict(false))
}

/// Ours on one case, in the driver's form.
fn ours(name: &str, args: &Value, dir: &std::path::Path) -> Value {
    let a = |i: usize| &args[i];
    let s = |i: usize| a(i).as_str().unwrap();
    let n = |i: usize| a(i).as_i64().unwrap();
    result(match name {
        "get_clean_path" => Ok(json!(get_clean_path(s(0)))),
        "get_gcode" => Ok(json!(get_gcode(s(0)))),
        "file_is_on_sd" => {
            let parts: Vec<&str> = a(0)
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap())
                .collect();
            Ok(json!(file_is_on_sd(&parts)))
        }
        "make_fingerprint" => Ok(json!(make_fingerprint(s(0)))),
        "fat_datetime_to_tuple" => {
            fat_datetime_to_tuple(n(0)).map(|(y, mo, d, h, mi, se)| json!([y, mo, d, h, mi, se]))
        }
        "get_print_stats_gcode" => Ok(json!(get_print_stats_gcode(
            n(0),
            n(1),
            n(2),
            n(3),
            n(4),
            n(5)
        ))),
        "get_d3_code" => get_d3_code(n(0), n(1)).map(|code| json!(code)),
        "round_to_five" => Ok(json!(round_to_five(a(0).as_f64().unwrap()))),
        "decode_line" => {
            let bytes: Vec<u8> = a(0)
                .as_array()
                .unwrap()
                .iter()
                .map(|b| b.as_u64().unwrap() as u8)
                .collect();
            Ok(json!(decode_line(&bytes)))
        }
        "walk_dict" => Ok(json!(
            walk_dict(a(0).as_object().unwrap(), None)
                .into_iter()
                .map(|(path, value)| json!([path, value]))
                .collect::<Vec<_>>()
        )),
        "slots_with_param" => {
            let telemetry = Telemetry {
                slot: a(0).as_object().map(|fields| Slot {
                    active: fields.get("active").and_then(Value::as_i64),
                    ..Slot::default()
                }),
                ..Telemetry::default()
            };
            slots_with_param(&telemetry, s(1), a(2), a(3)).map(|slots| match slots {
                None => Value::Null,
                Some(slots) => json!(
                    slots
                        .iter()
                        .map(|(key, slot)| json!([key, individual(slot)]))
                        .collect::<Vec<_>>()
                ),
            })
        }
        "parse_little_endian_uint32" => parse_little_endian_uint32(s(0)).map(|n| json!(n)),
        "power_panic_delay" => {
            let mut cfg = Config::new(&Args::fake(&dir.join("no-config.ini"))).unwrap();
            cfg.daemon
                .set("power_panic_file", ConfigValue::Str(s(0).to_owned()));
            let start = std::time::Instant::now();
            power_panic_delay(&cfg).map(|()| json!([null, start.elapsed().as_secs_f64() < 1.0]))
        }
        "ensure_directory" => {
            let path = s(0).replace("{side}", "rust");
            ensure_directory(&path, None)
                .map(|()| json!([null, std::path::Path::new(&path).is_dir()]))
        }
        other => panic!("{other}"),
    })
}

fn cases(dir: &std::path::Path) -> Vec<Value> {
    let pp = |name: &str, data: Value| {
        let path = dir.join(name);
        std::fs::write(&path, data.to_string()).unwrap();
        path.to_str().unwrap().to_owned()
    };
    let rip = pp(
        "rip.json",
        json!({"file_path": "/a", "connect_path": "/b", "message_number": 1, "gcode_number": 2, "using_rip_port": true}),
    );
    let missing_field = pp("missing.json", json!({"file_path": "/a"}));
    let mut cases = vec![];
    for path in [
        "", ".", "./", "/", "//", "///", "////a", "a", "a/", "a//b", "/a/./b/", "//a", "//a//b",
        "///a/b", "a/..", "./a", "a/b/.", "/./", "a/./../b", " a / b ",
    ] {
        cases.push(json!(["get_clean_path", [path]]));
    }
    for line in [
        "",
        "G1 X10 ; move",
        "  M104 S215  ",
        "; only a comment",
        "M117 Grüße ; ü",
        "G28;;",
        "\tG1 Y5\u{a0}",
        "M117 日本",
    ] {
        cases.push(json!(["get_gcode", [line]]));
    }
    for parts in [
        json!([]),
        json!(["/"]),
        json!(["/", "SD Card"]),
        json!(["/", "SD Card", "a.gco"]),
        json!(["/", "PrusaLink gcodes", "a.gcode"]),
        json!(["SD Card"]),
    ] {
        cases.push(json!(["file_is_on_sd", [parts]]));
    }
    for sn in ["CZPX1234X123X12345", "", "ü"] {
        cases.push(json!(["make_fingerprint", [sn]]));
    }
    for fat in [
        0x66a4d55_i64,
        0,
        0x0021_0000,            // 1980-01-01
        0x0041_0000 | 30 << 16, // February 30th
        0x0021_0000 | 24 << 11,
        0x0021_0000 | 60 << 5,
        0x0021_0000 | 30,
        0x0021_0000 | 13 << 21,
        0x0021_0000 | 0 << 21,
        0x7f9fbf7d,
        0x9021_0000,
        0x50290000, // 2020-01-09
        0x405d0000, // 2012-02-29
        0x467d0000, // 2015-03-29
        0x463d0000, // 2015-01-29
        0x465d0000, // 2015-02-29
        0x405d0000 | 59 << 5 | 29,
        -1,
    ] {
        cases.push(json!(["fat_datetime_to_tuple", [fat]]));
    }
    cases.push(json!(["get_print_stats_gcode", [-1, -1, -1, -1, -1, -1]]));
    cases.push(json!(["get_print_stats_gcode", [10, 20, 30, 40, 50, 60]]));
    for (address, count) in [
        (0x0D05_i64, 4_i64),
        (0x0FBB, 1),
        (0xffff, 999),
        (0x10000, 1),
        (5, 0),
        (5, 1000),
        (-5, 3),
        (0, 1),
        (0xabc, 88),
    ] {
        cases.push(json!(["get_d3_code", [address, count]]));
    }
    for number in [
        23.0, 22.0, 22.6, 22.4, 22.5, 27.5, 12.5, 17.5, -2.5, -7.5, 0.0, -0.0, 2.4999, 1e6,
    ] {
        cases.push(json!(["round_to_five", [number]]));
    }
    let all: Vec<u8> = (0..=255).collect();
    for bytes in [
        all,
        b"  ok\x00\r\n".to_vec(),
        b"\x00".to_vec(),
        b"\x00 \x00T:20.0\x00".to_vec(),
        b"\xff\xa0 x \xa0".to_vec(),
        b"\x1c\x1d\x1e\x1fok\x85".to_vec(),
        vec![],
    ] {
        cases.push(json!(["decode_line", [bytes]]));
    }
    cases.push(json!(["walk_dict", [{}]]));
    cases.push(
        json!(["walk_dict", [{"a": 1, "b": {"c": {"d": null, "e": [1, 2]}, "f": {}}, "g": "x"}]]),
    );
    cases.push(json!(["slots_with_param", [null, "temp", null, 215.0]]));
    cases.push(json!(["slots_with_param", [{"active": 2}, "temp", null, 215.0]]));
    cases.push(json!(["slots_with_param", [{"active": 9}, "material", "---", "PLA"]]));
    cases.push(json!(["slots_with_param", [{}, "fan_print", 0, 100]]));
    cases.push(json!(["slots_with_param", [{"active": 1}, "colour", 0, 1]]));
    for data in [
        "01 00 00 00",
        "ff ff ff ff",
        "0d05 0000",
        "01 00",
        "01 00 00 00 00",
        "zz 00 00 00",
        "0 1000000",
        "",
    ] {
        cases.push(json!(["parse_little_endian_uint32", [data]]));
    }
    cases.push(json!([
        "power_panic_delay",
        [dir.join("absent").to_str().unwrap()]
    ]));
    cases.push(json!(["power_panic_delay", [rip]]));
    cases.push(json!(["power_panic_delay", [missing_field]]));
    let base = dir.join("{side}");
    for path in ["a", "a", "b/c/d", "b/c/d/"] {
        cases.push(json!([
            "ensure_directory",
            [base.join(path).to_str().unwrap()]
        ]));
    }
    // a file where the directory should be: `os.makedirs`'s FileExistsError? No: `exists` is
    // true, so nothing is done.
    std::fs::create_dir_all(dir.join("python")).unwrap();
    std::fs::create_dir_all(dir.join("rust")).unwrap();
    std::fs::write(dir.join("python/file"), "").unwrap();
    std::fs::write(dir.join("rust/file"), "").unwrap();
    cases.push(json!([
        "ensure_directory",
        [base.join("file").to_str().unwrap()]
    ]));
    // under a file: makedirs fails
    cases.push(json!([
        "ensure_directory",
        [base.join("file/x").to_str().unwrap()]
    ]));
    cases
}

/// `util.py`'s functions of section A, PrusaLink's own and ours, on the same arguments: the same
/// result or the same exception. `power_panic_delay` without its 20 seconds' wait (a file that
/// is missing, `using_rip_port`, a field missing); `is_potato_cpu` on this machine.
#[test]
fn identical_to_prusalink() {
    let dir = std::env::temp_dir().join(format!("maker-connect-util-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cases = cases(&dir);
    let Some(theirs) = python::run(
        include_str!("../driver_util.py"),
        &json!({ "cases": cases }),
    ) else {
        return;
    };
    let results = theirs["results"].as_array().unwrap();
    assert_eq!(results.len(), cases.len());
    assert_eq!(cases.len(), 111);
    for (case, their) in cases.iter().zip(results) {
        let name = case[0].as_str().unwrap();
        let mut ours = ours(name, &case[1], &dir);
        let mut their = their.clone();
        if name == "power_panic_delay" && their.get("error").is_some() {
            // pydantic's ValidationError text against ours: only the class
            ours["error"][1] = Value::Null;
            their["error"][1] = Value::Null;
        }
        let (mut ours, mut their) = (
            serde_json::to_string(&ours).unwrap(),
            serde_json::to_string(&their).unwrap(),
        );
        if name == "ensure_directory" {
            // each side makes its own directories
            ours = ours.replace("/rust/", "/{side}/");
            their = their.replace("/python/", "/{side}/");
        }
        assert_eq!(ours, their, "{case}");
    }
    assert_eq!(json!(is_potato_cpu().unwrap()), theirs["potato"]);
    std::fs::remove_dir_all(&dir).unwrap();
}
