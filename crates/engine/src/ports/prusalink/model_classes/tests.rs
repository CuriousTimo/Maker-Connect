use super::*;
use crate::ports::prusalink::python;

fn s(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}
fn i(v: &Value) -> Option<i64> {
    v.as_i64()
}
fn f(v: &Value) -> Option<f64> {
    v.as_f64()
}

fn individual(fields: &Value) -> IndividualSlot {
    IndividualSlot {
        material: s(&fields["material"]),
        temp: f(&fields["temp"]),
        fan_hotend: i(&fields["fan_hotend"]),
        fan_print: i(&fields["fan_print"]),
    }
}

fn slot(fields: &Value) -> Slot {
    Slot {
        active: i(&fields["active"]),
        state: i(&fields["state"]),
        progress: i(&fields["progress"]),
        command: s(&fields["command"]),
        slots: fields["slots"].as_array().map(|pairs| {
            pairs
                .iter()
                .map(|pair| (pair[0].as_str().unwrap().to_owned(), individual(&pair[1])))
                .collect()
        }),
    }
}

fn telemetry(fields: &Value) -> Telemetry {
    let g = |name: &str| &fields[name];
    Telemetry {
        temp_nozzle: f(g("temp_nozzle")),
        temp_bed: f(g("temp_bed")),
        target_nozzle: f(g("target_nozzle")),
        target_bed: f(g("target_bed")),
        axis_x: f(g("axis_x")),
        axis_y: f(g("axis_y")),
        axis_z: f(g("axis_z")),
        fan_extruder: i(g("fan_extruder")),
        fan_hotend: i(g("fan_hotend")),
        fan_print: i(g("fan_print")),
        target_fan_extruder: i(g("target_fan_extruder")),
        target_fan_hotend: i(g("target_fan_hotend")),
        target_fan_print: i(g("target_fan_print")),
        progress: i(g("progress")),
        filament: s(g("filament")),
        flow: i(g("flow")),
        speed: i(g("speed")),
        time_printing: i(g("time_printing")),
        time_transferring: i(g("time_transferring")),
        time_remaining: i(g("time_remaining")),
        odometer_x: i(g("odometer_x")),
        odometer_y: i(g("odometer_y")),
        odometer_z: i(g("odometer_z")),
        odometer_e: i(g("odometer_e")),
        material: s(g("material")),
        total_filament: i(g("total_filament")),
        total_print_time: i(g("total_print_time")),
        filament_change_in: i(g("filament_change_in")),
        inaccurate_estimates: g("inaccurate_estimates").as_bool(),
        slot: (!g("slot").is_null()).then(|| slot(g("slot"))),
    }
}

fn network(fields: &Value) -> NetworkInfo {
    let g = |name: &str| s(&fields[name]);
    NetworkInfo {
        lan_ipv4: g("lan_ipv4"),
        lan_ipv6: g("lan_ipv6"),
        lan_mac: g("lan_mac"),
        wifi_ipv4: g("wifi_ipv4"),
        wifi_ipv6: g("wifi_ipv6"),
        wifi_mac: g("wifi_mac"),
        wifi_ssid: g("wifi_ssid"),
        hostname: g("hostname"),
        username: g("username"),
        digest: g("digest"),
    }
}

fn ours(model: &str, fields: &Value, exclude_none: bool) -> Map<String, Value> {
    match model {
        "IndividualSlot" => individual(fields).dict(exclude_none),
        "Slot" => slot(fields).dict(exclude_none),
        "Telemetry" => telemetry(fields).dict(exclude_none),
        "NetworkInfo" => network(fields).dict(exclude_none),
        "PPData" => {
            let mut pp = PPData::new(
                s(&fields["file_path"]).unwrap(),
                s(&fields["connect_path"]).unwrap(),
                i(&fields["message_number"]).unwrap(),
                i(&fields["gcode_number"]).unwrap(),
            );
            if let Some(rip) = fields["using_rip_port"].as_bool() {
                pp.using_rip_port = rip;
            }
            pp.dict(exclude_none)
        }
        other => panic!("{other}"),
    }
}

fn cases() -> Vec<Value> {
    let slots = json!([
        ["1", {"material": "PLA", "temp": 215.5, "fan_hotend": 3000}],
        ["2", {}],
        ["3", {"fan_print": 0}],
    ]);
    let full_slot =
        json!({"active": 2, "state": 1, "progress": 40, "command": "T1", "slots": slots});
    let full = json!({
        "temp_nozzle": 215.0, "temp_bed": 60.5, "target_nozzle": 215.0, "target_bed": 60.0,
        "axis_x": 1.5, "axis_y": -4.0, "axis_z": 0.15, "fan_extruder": 1, "fan_hotend": 2,
        "fan_print": 3, "target_fan_extruder": 4, "target_fan_hotend": 5, "target_fan_print": 6,
        "progress": 7, "filament": "PLA", "flow": 100, "speed": 95, "time_printing": 8,
        "time_transferring": 9, "time_remaining": 10, "odometer_x": 11, "odometer_y": 12,
        "odometer_z": 13, "odometer_e": 14, "material": "PETG", "total_filament": 15,
        "total_print_time": 16, "filament_change_in": 17, "inaccurate_estimates": true,
        "slot": full_slot,
    });
    let mut out = Vec::new();
    for exclude_none in [false, true] {
        let kwargs = if exclude_none {
            json!({"exclude_none": true})
        } else {
            json!({})
        };
        for (model, fields) in [
            ("IndividualSlot", json!({})),
            ("IndividualSlot", json!({"temp": 0.0, "material": ""})),
            ("Slot", json!({})),
            ("Slot", full_slot.clone()),
            ("Slot", json!({"active": 0, "slots": []})),
            ("Telemetry", json!({})),
            ("Telemetry", full.clone()),
            ("Telemetry", json!({"temp_nozzle": 20.0, "slot": {}})),
            (
                "Telemetry",
                json!({"progress": 0, "slot": {"slots": [["1", {}]]}}),
            ),
            ("NetworkInfo", json!({})),
            (
                "NetworkInfo",
                json!({"wifi_ipv4": "192.168.1.2", "hostname": "mk3"}),
            ),
            (
                "PPData",
                json!({"file_path": "/a.gcode", "connect_path": "/b", "message_number": 5, "gcode_number": 6}),
            ),
            (
                "PPData",
                json!({"file_path": "", "connect_path": "", "message_number": 0, "gcode_number": 0, "using_rip_port": true}),
            ),
        ] {
            out.push(json!({"model": model, "fields": fields, "kwargs": kwargs}));
        }
    }
    out
}

/// Each model made with the same fields in PrusaLink and here, then `dict()` with and without
/// `exclude_none`: the same keys, in the same order, with the same values (`float` stays a float).
#[test]
fn dicts_identical_to_prusalink() {
    let cases = cases();
    let Some(theirs) = python::run(
        include_str!("../driver_models.py"),
        &json!({ "cases": cases }),
    ) else {
        return;
    };
    let theirs = theirs.as_array().unwrap();
    assert_eq!(theirs.len(), cases.len());
    assert_eq!(cases.len(), 26);
    for (case, their) in cases.iter().zip(theirs) {
        let exclude_none = case["kwargs"]["exclude_none"].as_bool().unwrap_or(false);
        let ours = Value::Object(ours(
            case["model"].as_str().unwrap(),
            &case["fields"],
            exclude_none,
        ));
        assert_eq!(
            serde_json::to_string(&ours).unwrap(),
            serde_json::to_string(their).unwrap(),
            "{case}"
        );
    }
}

/// The enums' members, in their order, with their values, and the module's names as its source
/// has them.
#[test]
fn enums_identical_to_prusalink() {
    let Some(theirs) = python::run(
        include_str!("../driver_constants.py"),
        &json!({"modules": ["prusa.link.printer_adapter.structures.model_classes"]}),
    ) else {
        return;
    };
    let theirs = &theirs["prusa.link.printer_adapter.structures.model_classes"];
    assert_eq!(
        theirs["names"],
        json!([
            "IndividualSlot",
            "Slot",
            "Telemetry",
            "NetworkInfo",
            "FileType",
            "JobState",
            "SDState",
            "PrintState",
            "PrintMode",
            "EEPROMParams",
            "PPData"
        ])
    );
    fn members<'a>(class: &str, items: impl Iterator<Item = (&'a str, Value)>) -> (String, Value) {
        let members: Vec<Value> = items.map(|(name, value)| json!([name, value])).collect();
        (
            class.to_owned(),
            json!({"class": class, "members": members}),
        )
    }
    let ours = [
        members(
            "FileType",
            FileType::ALL.iter().map(|m| (m.name(), json!(m.value()))),
        ),
        members(
            "JobState",
            JobState::ALL.iter().map(|m| (m.name(), json!(m.value()))),
        ),
        members(
            "SDState",
            SDState::ALL.iter().map(|m| (m.name(), json!(m.value()))),
        ),
        members(
            "PrintState",
            PrintState::ALL.iter().map(|m| (m.name(), json!(m.value()))),
        ),
        members(
            "PrintMode",
            PrintMode::ALL.iter().map(|m| (m.name(), json!(m.value()))),
        ),
        members(
            "EEPROMParams",
            EEPROMParams::ALL.iter().map(|m| {
                let (address, length) = m.value();
                (m.name(), json!({"tuple": [address, length]}))
            }),
        ),
    ];
    for (class, value) in ours {
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            serde_json::to_string(&theirs["values"][&class]).unwrap(),
            "{class}"
        );
    }
}
