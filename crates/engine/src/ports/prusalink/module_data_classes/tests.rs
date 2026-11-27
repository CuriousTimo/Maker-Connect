use super::*;
use crate::ports::prusalink::python;
use serde_json::json;

/// One case: the class, the kwargs Python gets, our `dict()`, `dict(exclude_none=True)` and the
/// extra (Port's `str`, JobData's `get_job_id_for_api`).
type Case = (
    &'static str,
    Value,
    Map<String, Value>,
    Map<String, Value>,
    Option<Value>,
);

fn case(
    name: &'static str,
    given: &[&str],
    all: Map<String, Value>,
    none_out: Map<String, Value>,
    extra: Option<Value>,
) -> Case {
    let kwargs: Map<String, Value> = all
        .iter()
        .filter(|(key, _)| given.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    (name, Value::Object(kwargs), all, none_out, extra)
}

fn port_cases(port: &Port, given: &[&str]) -> Case {
    case(
        "Port",
        given,
        port.dict(false),
        port.dict(true),
        Some(json!(port.to_string())),
    )
}

const PORT_FIELDS: [&str; 9] = [
    "path",
    "is_rpi_port",
    "checked",
    "usable",
    "selected",
    "description",
    "baudrate",
    "timeout",
    "sn",
];

fn cases() -> Vec<Case> {
    let mut cases = vec![];
    let bare = Port {
        path: "/dev/ttyACM0".to_owned(),
        ..Port::default()
    };
    cases.push(port_cases(&bare, &["path"]));
    let full = Port {
        path: "/dev/ttyAMA0".to_owned(),
        is_rpi_port: true,
        checked: true,
        usable: true,
        selected: true,
        description: "OK".to_owned(),
        baudrate: 250000,
        timeout: 5,
        sn: Some("CZPX1234X123X12345".to_owned()),
    };
    cases.push(port_cases(&full, &PORT_FIELDS));

    let sad = SerialAdapterData::default();
    cases.push(case(
        "SerialAdapterData",
        &[],
        sad.dict(false),
        sad.dict(true),
        None,
    ));
    let sad = SerialAdapterData {
        ports: vec![bare.clone(), full.clone()],
        using_port: Some(full.clone()),
        reset_disabling: false,
        resets_enabled: Some(true),
    };
    cases.push(case(
        "SerialAdapterData",
        &["ports", "using_port", "reset_disabling", "resets_enabled"],
        sad.dict(false),
        sad.dict(true),
        None,
    ));

    let smd = StateManagerData {
        base_state: State::Idle,
        printing_state: None,
        override_state: Some(State::Attention),
        last_state: State::Busy,
        current_state: State::Printing,
        state_history: VecDeque::from([State::Idle, State::Busy, State::Printing]),
        awaiting_error_reason: true,
    };
    cases.push(case(
        "StateManagerData",
        &[
            "base_state",
            "override_state",
            "last_state",
            "current_state",
            "state_history",
            "awaiting_error_reason",
        ],
        smd.dict(false),
        smd.dict(true),
        None,
    ));

    for (job_state, job_id) in [
        (JobState::Idle, Some(7)),
        (JobState::InProgress, Some(7)),
        (JobState::Ending, None),
    ] {
        let jd = JobData {
            job_id,
            job_id_offset: 3,
            already_sent: Some(false),
            job_start_cmd_id: None,
            selected_file_path: Some("/a.gcode".to_owned()),
            selected_file_m_timestamp: Some(1700000000),
            selected_file_size: Some("123".to_owned()),
            printing_file_byte: None,
            path_incomplete: Some(true),
            from_sd: None,
            inbuilt_reporting: Some(false),
            last_job_path: None,
            job_state,
        };
        cases.push(case(
            "JobData",
            &[
                "job_id",
                "job_id_offset",
                "already_sent",
                "selected_file_path",
                "selected_file_m_timestamp",
                "selected_file_size",
                "path_incomplete",
                "inbuilt_reporting",
                "job_state",
            ],
            jd.dict(false),
            jd.dict(true),
            Some(json!(jd.get_job_id_for_api())),
        ));
    }

    let ip = IPUpdaterData {
        local_ip: Some("192.168.1.5".to_owned()),
        local_ip6: None,
        mac: Some("aa:bb".to_owned()),
        is_wireless: true,
        update_ip_on: 12.5,
        ssid: None,
        hostname: Some("prusa".to_owned()),
        username: None,
        digest: Some("x".to_owned()),
    };
    cases.push(case(
        "IPUpdaterData",
        &[
            "local_ip",
            "mac",
            "is_wireless",
            "update_ip_on",
            "hostname",
            "digest",
        ],
        ip.dict(false),
        ip.dict(true),
        None,
    ));

    let sd = SDCardData {
        expecting_insertion: false,
        invalidated: true,
        is_flash_air: false,
        last_updated: 1.25,
        last_checked_flash_air: 0.5,
        sd_state: SDState::Present,
        files: json!({"name": "SD Card", "children": [1, 2]}),
        sfn_to_lfn_paths: IndexMap::from([
            ("/B~1.GCO".to_owned(), "/b long.gcode".to_owned()),
            ("/A.GCO".to_owned(), "/a.gcode".to_owned()),
        ]),
        lfn_to_sfn_paths: IndexMap::new(),
        mixed_to_lfn_paths: IndexMap::from([("/x".to_owned(), "/y".to_owned())]),
    };
    cases.push(case(
        "SDCardData",
        &[
            "expecting_insertion",
            "invalidated",
            "is_flash_air",
            "last_updated",
            "last_checked_flash_air",
            "sd_state",
            "files",
            "sfn_to_lfn_paths",
            "lfn_to_sfn_paths",
            "mixed_to_lfn_paths",
        ],
        sd.dict(false),
        sd.dict(true),
        None,
    ));

    let st = StorageData {
        blacklisted_paths: vec!["/b".to_owned(), "/a".to_owned()],
        blacklisted_names: vec![],
        configured_storage: HashSet::from(["z".to_owned(), "a".to_owned(), "m".to_owned()]),
        attached_set: HashSet::new(),
    };
    cases.push(case(
        "StorageData",
        &[
            "blacklisted_paths",
            "blacklisted_names",
            "configured_storage",
            "attached_set",
        ],
        st.dict(false),
        st.dict(true),
        None,
    ));

    for code in [None, Some("04101".to_owned())] {
        let mmu = MMUObserverData {
            current_error_code: code,
        };
        cases.push(case(
            "MMUObserverData",
            &["current_error_code"],
            mmu.dict(false),
            mmu.dict(true),
            None,
        ));
    }
    let mmu = MMUObserverData::default();
    cases.push(case(
        "MMUObserverData",
        &[],
        mmu.dict(false),
        mmu.dict(true),
        None,
    ));

    let ps = PrintStatsData {
        print_time: 10.5,
        segment_start: 2.0,
        has_inbuilt_stats: true,
        total_gcode_count: 999,
        ..PrintStatsData::default()
    };
    cases.push(case(
        "PrintStatsData",
        &[
            "print_time",
            "segment_start",
            "has_inbuilt_stats",
            "total_gcode_count",
        ],
        ps.dict(false),
        ps.dict(true),
        None,
    ));
    let ps = PrintStatsData {
        start_gcode_number: 17,
        ..ps
    };
    cases.push(case(
        "PrintStatsData",
        &[
            "print_time",
            "segment_start",
            "has_inbuilt_stats",
            "total_gcode_count",
            "start_gcode_number",
        ],
        ps.dict(false),
        ps.dict(true),
        None,
    ));

    let sheet = Sheet::default();
    cases.push(case(
        "Sheet",
        &[],
        sheet.dict(false),
        sheet.dict(true),
        None,
    ));
    let sheet = Sheet {
        name: "Smooth1".to_owned(),
        z_offset: -0.75,
        bed_temp: 60,
        pinda_temp: 35,
    };
    cases.push(case(
        "Sheet",
        &["name", "z_offset", "bed_temp", "pinda_temp"],
        sheet.dict(false),
        sheet.dict(true),
        None,
    ));
    cases
}

/// `module_data_classes.py`'s models, PrusaLink's own and ours: made with the same fields (the
/// defaults from only the required ones), the same `dict()` and `dict(exclude_none=True)` in the
/// same order, `Port.__str__` and `JobData.get_job_id_for_api()`.
#[test]
fn identical_to_prusalink() {
    let cases = cases();
    let request: Vec<Value> = cases
        .iter()
        .map(|(name, kwargs, ..)| json!([name, kwargs]))
        .collect();
    let Some(theirs) = python::run(
        include_str!("../driver_module_data.py"),
        &json!({ "cases": request }),
    ) else {
        return;
    };
    let theirs = theirs.as_array().unwrap();
    assert_eq!(theirs.len(), cases.len());
    for ((name, kwargs, all, none_out, extra), their) in cases.iter().zip(theirs) {
        let mut ours = vec![Value::Object(all.clone()), Value::Object(none_out.clone())];
        ours.extend(extra.clone());
        assert_eq!(
            serde_json::to_string(&ours).unwrap(),
            serde_json::to_string(their).unwrap(),
            "{name} {kwargs}"
        );
    }
}
