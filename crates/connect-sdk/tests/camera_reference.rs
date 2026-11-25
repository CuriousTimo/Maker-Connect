//! Ours (`src/camera*.rs`, `src/configparser.rs`) against the SDK's camera modules run on the
//! same steps: `tests/camera_scenario.py run` runs the reference and prints what it did; this
//! test runs ours the same way, with the same `Fake` driver, and wants the same. Both send their
//! snapshots to one server (`camera_scenario.py serve`, stdlib only), which logs what it got.
//!
//! The Python, with the SDK at c0d7d233 and requests 2.32.5 installed, on CPython 3.13.12:
//! `CAMERA_PYTHON`, else `reference/.venv-connect-sdk-313`. Without it it says so and passes.
//!
//! What is not compared as it is:
//! - The order of a run of the driver's `set_*` calls, at connecting and in `set_settings`: both
//!   walk `configurable_capabilities`, a `frozenset` whose order changes per process; a run is compared
//!   sorted (`calls`). Every other list the scenario sorts itself (see its doc).
//! - Where a `take_a_photo` falls among the other calls: the driver takes each photo on a thread
//!   of its own (`camera_driver.py` `trigger`), in both, so its place is the threads' timing; the
//!   photos are compared as a list of their own, sorted, after the other calls in order.
//! - The `Host` header (each side's port) and a `Timestamp` header of a photo taken now.

#![cfg(unix)]

use maker_connect_gcode_metadata::py::{Dict, PyErr, PyValue, json_loads};
use maker_connect_sdk::camera::{Camera, Error, Resolution, Settings, Snapshot, Value};
use maker_connect_sdk::camera_configurator::CameraConfigurator;
use maker_connect_sdk::camera_controller::CameraController;
use maker_connect_sdk::camera_driver::{
    CameraDriver, DisconnectedCb, DriverClass, DriverImpl, DriverState,
};
use maker_connect_sdk::configparser::ConfigParser;
use maker_connect_sdk::consts::{CameraConfig, CameraConfigs, CapabilityType};
use maker_connect_sdk::models::{CameraRegister, LoopObject};
use maker_connect_sdk::util::Session;
use serde_json::{Value as Json, json};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn manifest(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn python() -> Option<String> {
    let python = std::env::var("CAMERA_PYTHON").ok().unwrap_or_else(|| {
        manifest("../../reference/.venv-connect-sdk-313/bin/python")
            .to_string_lossy()
            .into_owned()
    });
    let check = Command::new(&python)
        .args([
            "-I",
            "-c",
            "import sys, requests, prusa.connect.printer.camera_configurator; \
             print(sys.version.split()[0], requests.__version__)",
        ])
        .output();
    match check {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
            assert_eq!(
                version, "3.13.12 2.32.5",
                "the reference's Python and requests"
            );
            Some(python)
        }
        _ => {
            // CI's job `references` sets REFERENCES_REQUIRED: there a skip is a failure.
            assert!(
                std::env::var_os("REFERENCES_REQUIRED").is_none(),
                "REFERENCES_REQUIRED: skipped: no Python with the SDK and requests (CAMERA_PYTHON)"
            );
            eprintln!("skipped: no Python with the SDK and requests (CAMERA_PYTHON)");
            None
        }
    }
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn serve(python: &str, log: &Path) -> (Server, u16) {
    let mut child = Command::new(python)
        .arg("-I")
        .arg(manifest("tests/camera_scenario.py"))
        .arg("serve")
        .arg(log)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    (Server(child), line.trim().parse().unwrap())
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc-camera-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// --- the Fake driver, as the scenario's ---

static CALLS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static SCAN: Mutex<CameraConfigs> = Mutex::new(Vec::new());

fn call(name: &str, arg: String) {
    CALLS.lock().unwrap().push((name.to_owned(), arg));
}

fn full() -> Vec<Resolution> {
    vec![Resolution::new(640, 480), Resolution::new(1280, 720)]
}

fn path_of(driver: &CameraDriver) -> Option<String> {
    driver.config_value("path")
}

struct Fake;

impl DriverImpl for Fake {
    fn connect(&self, driver: &CameraDriver) -> Result<(), PyErr> {
        call("connect", driver.camera_id());
        if path_of(driver).as_deref() == Some("/broken") {
            return Err(PyErr::Other("RuntimeError", "no such camera".into()));
        }
        Ok(())
    }

    fn disconnect(&self, driver: &CameraDriver) -> Result<(), PyErr> {
        call("disconnect", driver.camera_id());
        Ok(())
    }

    fn set_resolution(&self, _driver: &CameraDriver, resolution: &Resolution) -> Result<(), PyErr> {
        call("set_resolution", resolution.to_string());
        Ok(())
    }

    fn set_rotation(&self, driver: &CameraDriver, rotation: &Value) -> Result<(), PyErr> {
        call("set_rotation", rotation.to_py_str());
        if path_of(driver).as_deref() == Some("/failset") {
            return Err(PyErr::Other("RuntimeError", "the motor is stuck".into()));
        }
        Ok(())
    }

    fn set_exposure(&self, _driver: &CameraDriver, exposure: &Value) -> Result<(), PyErr> {
        call("set_exposure", exposure.to_py_str());
        Ok(())
    }

    fn set_focus(&self, _driver: &CameraDriver, focus: &Value) -> Result<(), PyErr> {
        call("set_focus", focus.to_py_str());
        Ok(())
    }

    fn take_a_photo(&self, driver: &CameraDriver) -> Result<Vec<u8>, PyErr> {
        call("take_a_photo", driver.camera_id());
        let path = path_of(driver).unwrap_or_default();
        if path == "/slow" {
            std::thread::sleep(Duration::from_millis(300));
        }
        Ok([b"JPEG".as_slice(), path.as_bytes()].concat())
    }
}

fn fake_init(state: &mut DriverState) -> Box<dyn DriverImpl> {
    let path = state
        .config
        .iter()
        .find(|(k, _)| k == "path")
        .map(|(_, v)| v.clone());
    state.capabilities = vec![CapabilityType::TriggerScheme, CapabilityType::Imaging];
    if matches!(path.as_deref(), Some("/full" | "/failset")) {
        state.capabilities = CapabilityType::ALL.to_vec();
        state.available_resolutions = full();
        let resolution =
            CameraDriver::get_initial_resolution(&state.available_resolutions, &state.config)
                .unwrap();
        set(&mut state.config, "resolution", &resolution.to_string());
        if !state.config.iter().any(|(k, _)| k == "focus") {
            state.config.push(("focus".into(), "0.5".into()));
        }
    }
    Box::new(Fake)
}

fn fake_scan() -> Result<CameraConfigs, PyErr> {
    Ok(SCAN.lock().unwrap().clone())
}

static FAKE: DriverClass = DriverClass {
    name: "Fake",
    class_name: "Fake",
    requires_settings: &[("path", "Path to the fake camera")],
    scan: fake_scan,
    is_config_valid: CameraDriver::base_is_config_valid,
    init: fake_init,
};

// --- helpers, as the scenario's ---

fn set(config: &mut CameraConfig, key: &str, value: &str) {
    match config.iter_mut().find(|(k, _)| k == key) {
        Some(entry) => entry.1 = value.to_owned(),
        None => config.push((key.to_owned(), value.to_owned())),
    }
}

fn cfg(pairs: &[(&str, &str)]) -> CameraConfig {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn err(e: &Error) -> Json {
    json!([e.name(), e.message()])
}

fn py_err(e: &PyErr) -> Json {
    json!([e.class_name(), e.message()])
}

fn calls() -> Json {
    let out: Vec<Json> = std::mem::take(&mut *CALLS.lock().unwrap())
        .into_iter()
        .map(|(n, a)| json!([n, a]))
        .collect();
    Json::Array(out)
}

fn requests_logged(log: &Path) -> Json {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    std::fs::write(log, "").unwrap();
    Json::Array(
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect(),
    )
}

fn value_str(value: &Value) -> String {
    match value {
        Value::Scheme(scheme) => scheme.name().to_owned(),
        other => other.to_py_str(),
    }
}

fn sorted_pairs(mut pairs: Vec<(String, Json)>) -> Json {
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    Json::Array(pairs.into_iter().map(|(k, v)| json!([k, v])).collect())
}

fn settings_json(settings: &Settings) -> Json {
    sorted_pairs(
        settings
            .iter()
            .map(|(k, v)| (k.clone(), json!(value_str(v))))
            .collect(),
    )
}

fn settings(camera: &Arc<Camera>) -> Json {
    settings_json(&camera.get_settings().unwrap())
}

fn payload(camera: &Arc<Camera>) -> Json {
    let data = CameraRegister::new(camera.clone()).to_payload().unwrap();
    let config = data["config"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut options: Vec<Json> = data["options"]["available_resolutions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| json!([r["width"], r["height"]]))
        .collect();
    options.sort_by_key(|o| (o[0].as_i64(), o[1].as_i64()));
    let mut capabilities: Vec<String> = data["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_owned())
        .collect();
    capabilities.sort();
    json!({
        "config": sorted_pairs(config),
        "options": options,
        "capabilities": capabilities,
        "fingerprint": data["fingerprint"],
    })
}

fn ini(path: &Path) -> Json {
    let mut parser = ConfigParser::new();
    parser.read(path).unwrap();
    Json::Array(
        parser
            .sections()
            .into_iter()
            .map(|s| {
                let items = parser
                    .items(&s)
                    .unwrap()
                    .into_iter()
                    .map(|(k, v)| (k, json!(v)))
                    .collect();
                json!([s, sorted_pairs(items)])
            })
            .collect(),
    )
}

fn counter() -> (DisconnectedCb, Arc<Mutex<usize>>) {
    let count = Arc::new(Mutex::new(0));
    let c = count.clone();
    (
        Arc::new(move |_driver: &CameraDriver| {
            *c.lock().unwrap() += 1;
            Ok(())
        }),
        count,
    )
}

fn new_camera(path: &str, disconnected: &DisconnectedCb) -> Arc<Camera> {
    let id = format!("id{}", path.trim_matches('/'));
    let driver = CameraDriver::new(
        &FAKE,
        &id,
        cfg(&[("name", path), ("driver", "Fake"), ("path", path)]),
        disconnected.clone(),
    );
    driver.connect().unwrap();
    Camera::new(driver).unwrap()
}

/// `Camera.settings_from_json({key: value})`, the pair as JSON text.
fn from_json(case: &str) -> Result<Settings, PyErr> {
    let PyValue::List(pair) = json_loads(case).unwrap() else {
        panic!("a pair")
    };
    let PyValue::Str(key) = &pair[0] else {
        panic!("a key")
    };
    let mut dict = Dict::new();
    dict.set(key, pair[1].clone());
    Camera::settings_from_json(&dict)
}

fn scheme(name: &str) -> Settings {
    from_json(&format!(r#"["trigger_scheme", "{name}"]"#)).unwrap()
}

const SETTER_CASES: &[&str] = &[
    r#"["rotation", 90]"#,
    r#"["rotation", 45]"#,
    r#"["rotation", "90"]"#,
    r#"["rotation", 90.0]"#,
    r#"["rotation", true]"#,
    r#"["rotation", [90]]"#,
    r#"["rotation", 0]"#,
    r#"["exposure", 1]"#,
    r#"["exposure", 1.5]"#,
    r#"["exposure", 3]"#,
    r#"["exposure", "1"]"#,
    r#"["exposure", -2]"#,
    r#"["exposure", null]"#,
    r#"["focus", 0.5]"#,
    r#"["focus", 0.25]"#,
    r#"["focus", 2]"#,
    r#"["resolution", {"width": 640, "height": 480}]"#,
    r#"["resolution", {"width": 100, "height": 100}]"#,
    r#"["resolution", "640x480"]"#,
    r#"["resolution", {"width": 640}]"#,
    r#"["resolution", {"width": 640, "height": 480, "x": 1}]"#,
    r#"["trigger_scheme", "TEN_SEC"]"#,
    r#"["trigger_scheme", "Every 10 seconds"]"#,
    r#"["trigger_scheme", "MANUAL"]"#,
    r#"["trigger_scheme", 5]"#,
    r#"["trigger_scheme", [1]]"#,
    r#"["name", "Renamed"]"#,
    r#"["token", "t0k"]"#,
];

const FROM_STRING_CASES: &[&[(&str, &str)]] = &[
    &[("resolution", "640x480")],
    &[("resolution", " 640 x 480 ")],
    &[("resolution", "640")],
    &[("resolution", "1x2x3")],
    &[("resolution", "ax2")],
    &[("resolution", "")],
    &[("rotation", " 90 ")],
    &[("rotation", "9_0")],
    &[("rotation", "90.0")],
    &[("exposure", "1e0")],
    &[("exposure", "nan")],
    &[("exposure", " -1.50 ")],
    &[("focus", "inf")],
    &[("focus", "x")],
    &[("trigger_scheme", "BOGUS")],
    &[("trigger_scheme", "EACH_LAYER")],
    &[("name", "n"), ("path", "/p")],
];

const INITIAL_RESOLUTION_CASES: &[&[(&str, &str)]] = &[
    &[],
    &[("resolution", "640x480")],
    &[("resolution", "100x100")],
    &[("resolution", "bad")],
    &[("resolution", "1x2x3")],
];

fn run(port: u16, log: &Path, folder: &Path) -> Json {
    let server = format!("http://127.0.0.1:{port}");
    let mut out = serde_json::Map::new();

    // --- one camera, its settings and registration ---
    let (disconnected, _) = counter();
    let camera = new_camera("/full", &disconnected);
    let string: Vec<(String, Json)> = Camera::string_from_settings(&camera.get_settings().unwrap())
        .unwrap()
        .into_iter()
        .map(|(k, v)| (k, json!(v)))
        .collect();
    out.insert(
        "full".into(),
        json!({
            "calls": calls(), "settings": settings(&camera), "payload": payload(&camera),
            "output_resolution": resolution_str(&camera),
            "string": sorted_pairs(string),
        }),
    );
    let minimal = new_camera("/min", &disconnected);
    out.insert(
        "min".into(),
        json!({"calls": calls(), "settings": settings(&minimal),
               "payload": payload(&minimal),
               "output_resolution": resolution_str(&minimal)}),
    );
    let min_rotation = match minimal.set_value(
        CapabilityType::Rotation,
        Value::Py(json_loads("90").unwrap()),
    ) {
        Ok(()) => Json::Null,
        Err(e) => err(&e),
    };
    out.insert("min_rotation".into(), min_rotation);

    // --- the setters, each on a camera of its own ---
    let mut cases = Vec::new();
    for case in SETTER_CASES {
        let (disconnected, count) = counter();
        let camera = new_camera("/full", &disconnected);
        calls();
        let error = match from_json(case).map_err(Error::from) {
            Ok(new) => camera.set_settings(&new, false),
            Err(e) => Err(e),
        };
        cases.push(json!({
            "case": serde_json::from_str::<Json>(case).unwrap(),
            "error": error.err().map(|e| err(&e)),
            "calls": calls(),
            "settings": settings(&camera),
            "disconnected": *count.lock().unwrap(),
            "busy": camera.is_busy(),
        }));
    }
    let (disconnected, count) = counter();
    let camera = new_camera("/failset", &disconnected);
    calls();
    let error = camera.set_settings(
        &vec![("rotation".into(), Value::Py(json_loads("90").unwrap()))],
        false,
    );
    cases.push(json!({
        "case": ["failset", 90], "error": error.err().map(|e| err(&e)), "calls": calls(),
        "settings": settings(&camera),
        "disconnected": *count.lock().unwrap(), "busy": camera.is_busy(),
    }));
    out.insert("setters".into(), Json::Array(cases));

    // --- conversions ---
    let records: Vec<Json> = FROM_STRING_CASES
        .iter()
        .map(|case| match Camera::settings_from_string(&cfg(case)) {
            Ok(converted) => Json::Array(
                converted
                    .iter()
                    .map(|(k, v)| json!([k, value_str(v)]))
                    .collect(),
            ),
            Err(e) => py_err(&e),
        })
        .collect();
    out.insert("from_string".into(), Json::Array(records));
    let mut records: Vec<Json> = INITIAL_RESOLUTION_CASES
        .iter()
        .map(|case| {
            json!(
                CameraDriver::get_initial_resolution(&full(), &cfg(case))
                    .unwrap()
                    .to_string()
            )
        })
        .collect();
    match CameraDriver::get_initial_resolution(&[], &cfg(&[])) {
        Ok(r) => records.push(json!(r.to_string())),
        Err(e) => records.push(py_err(&e)),
    }
    out.insert("initial_resolution".into(), Json::Array(records));

    // --- hashes, validation, scanning ---
    let mut required = CameraDriver::get_required_settings(&FAKE);
    required.sort();
    let valid = FAKE.is_config_valid;
    let hashes = vec![
        json!(CameraDriver::make_hash("/dev/video0").unwrap()),
        json!(
            CameraDriver::get_config_hash(
                &FAKE,
                &cfg(&[("name", "n"), ("driver", "Fake"), ("path", "/x")])
            )
            .unwrap()
        ),
        json!(
            CameraDriver::get_config_hash(&FAKE, &cfg(&[("name", "n"), ("driver", "Fake")]))
                .unwrap()
        ),
        json!(valid(
            &FAKE,
            &cfg(&[("name", "n"), ("driver", "Fake"), ("path", "/x")])
        )),
        json!(valid(&FAKE, &cfg(&[("name", "n"), ("path", "/x")]))),
        json!(required),
    ];
    out.insert("hashes".into(), Json::Array(hashes));
    *SCAN.lock().unwrap() = vec![
        ("/dev/a".into(), cfg(&[("name", "A"), ("path", "/full")])),
        ("/dev/b".into(), cfg(&[("name", "B")])),
        (
            "/dev/c".into(),
            cfg(&[("name", "C"), ("path", "/min"), ("trigger_scheme", "x")]),
        ),
    ];
    let scan: Vec<Json> = CameraDriver::scan(&FAKE)
        .unwrap()
        .into_iter()
        .map(|(k, v)| {
            let v = v.into_iter().map(|(a, b)| (a, json!(b))).collect();
            json!([k, sorted_pairs(v)])
        })
        .collect();
    out.insert("scan".into(), Json::Array(scan));

    // --- snapshots ---
    requests_logged(log);
    let session = Session::new();
    let shot = Snapshot {
        camera_id: Some("c".into()),
        camera_fingerprint: Some("f".repeat(64)),
        camera_token: Some("tok".into()),
        printer_uuid: Some("u u/1".into()),
        timestamp: Some(1763600000.1),
        data: Some(b"\xff\xd8JPEG".to_vec()),
    };
    let first = shot.send(&session, &server).unwrap().status_code;
    let incomplete = Snapshot {
        data: Some(Vec::new()),
        ..Snapshot::new()
    };
    let second = incomplete.send(&session, &server).unwrap().status_code;
    out.insert(
        "snapshots".into(),
        json!({"status": [first, second], "sent": requests_logged(log)}),
    );

    // --- the controller ---
    let sent: Arc<Mutex<Vec<LoopObject>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = sent.clone();
    let controller = CameraController::new(
        session.clone(),
        Some(server.clone()),
        Arc::new(move |item| sink.lock().unwrap().push(item)),
    );
    let (disconnected, _) = counter();
    let camera = new_camera("/full", &disconnected);
    controller.add_camera(camera.clone()).unwrap();
    controller.register_camera(&camera.camera_id());
    controller.register_camera("unknown");
    controller.tick().unwrap();
    calls();
    camera.set_settings(&scheme("EACH_LAYER"), false).unwrap();
    for _ in 0..5 {
        controller.layer_trigger().unwrap();
        camera.wait_ready(Some(5.0)).unwrap();
    }
    let layer_calls = calls();
    camera.set_settings(&scheme("FIFTH_LAYER"), false).unwrap();
    for _ in 0..10 {
        controller.layer_trigger().unwrap();
        camera.wait_ready(Some(5.0)).unwrap();
    }
    let fifth_calls = calls();
    let slow = new_camera("/slow", &disconnected);
    controller.add_camera(slow.clone()).unwrap();
    slow.set_settings(&scheme("MANUAL"), false).unwrap();
    calls();
    let manual = match &scheme("MANUAL")[0].1 {
        Value::Scheme(s) => *s,
        _ => unreachable!(),
    };
    controller.trigger_pile(manual).unwrap();
    controller.trigger_pile(manual).unwrap();
    slow.wait_ready(Some(5.0)).unwrap();
    let busy_calls = calls();
    // `camera._token = "bad"`: set_settings sets `_token` and nothing else
    camera
        .set_settings(
            &vec![("token".into(), Value::Py(PyValue::Str("bad".into())))],
            false,
        )
        .unwrap();
    let stored: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let store = stored.clone();
    camera.driver().set_store_cb(Arc::new(move |id| {
        store.lock().unwrap().push(id.to_owned());
        Ok(())
    }));
    let looping = controller.clone();
    let lp = std::thread::spawn(move || looping.snapshot_loop());
    requests_logged(log);
    camera.trigger_a_photo(None).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while camera.token().is_some() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    controller.stop();
    lp.join().unwrap();
    let register: Vec<Json> = sent
        .lock()
        .unwrap()
        .iter()
        .map(|item| match item {
            LoopObject::CameraRegister(r) => payload(&r.camera),
            _ => panic!("not a CameraRegister"),
        })
        .collect();
    out.insert(
        "controller".into(),
        json!({
            "register": register,
            "layer_calls": layer_calls, "fifth_calls": fifth_calls,
            "busy_calls": busy_calls, "token": camera.token(),
            "stored": *stored.lock().unwrap(), "sent": requests_logged(log),
            "in_order": controller.cameras_in_order().iter().map(|c| c.camera_id())
                .collect::<Vec<_>>(),
        }),
    );

    // --- the configurator ---
    let path = folder.join("cameras.ini");
    std::fs::write(
        &path,
        "[camera_order]\n1 = gone1\n2 = gone2\n3 = storedA\n4 = storedB\n\n\
         [camera::storedA]\nname = Stored A\ndriver = Fake\npath = /full\n\n\
         [camera::storedB]\nname = Stored B\ndriver = Fake\npath = /broken\n\
         resolution = 640x480\n\n\
         [camera::bad]\nname = Bad\ndriver = Nope\n",
    )
    .unwrap();
    *SCAN.lock().unwrap() = vec![(
        "/dev/video9".into(),
        cfg(&[("name", "Detected"), ("path", "/min")]),
    )];
    let mut config = ConfigParser::new();
    config.read(&path).unwrap();
    let sink = sent.clone();
    let controller = CameraController::new(
        session,
        Some(server),
        Arc::new(move |item| sink.lock().unwrap().push(item)),
    );
    let configurator = CameraConfigurator::new(
        controller.clone(),
        Arc::new(Mutex::new(config)),
        path.clone(),
        vec![&FAKE],
        true,
    )
    .unwrap();
    let mut steps = Vec::new();
    let mut record = |name: &str, error: Option<Error>| {
        for (_, driver) in configurator.loaded() {
            if driver.is_connected() {
                controller
                    .get_camera(&driver.camera_id())
                    .unwrap()
                    .wait_ready(Some(5.0))
                    .unwrap();
            }
        }
        let loaded: Vec<String> = configurator.loaded().into_iter().map(|(i, _)| i).collect();
        let mut stored = configurator.stored();
        stored.sort();
        let mut detected = configurator.detected();
        detected.sort();
        steps.push(json!({
            "step": name, "error": error.map(|e| err(&e)), "ini": ini(&path),
            "order": configurator.order(), "stored": stored, "detected": detected,
            "connected": loaded.iter().map(|i| configurator.is_connected(i))
                .collect::<Vec<_>>(),
            "loaded": loaded,
            "in_order": controller.cameras_in_order().iter().map(|c| c.camera_id())
                .collect::<Vec<_>>(),
            "calls": calls(),
        }));
    };
    let mut attempt = |name: &str, result: Result<(), Error>| record(name, result.err());

    attempt("init", Ok(()));
    let manual = cfg(&[("name", "Manual"), ("driver", "Fake"), ("path", "/min")]);
    attempt("add", configurator.add_camera("manual1", &manual));
    attempt("add_again", configurator.add_camera("manual1", &manual));
    attempt(
        "add_invalid",
        configurator.add_camera("x", &cfg(&[("name", "x")])),
    );
    attempt(
        "set_order",
        configurator.set_order(&["manual1".into(), "nope".into()]),
    );
    attempt(
        "token",
        controller
            .get_camera("storedA")
            .map_err(Error::from)
            .and_then(|c| c.set_token(Some("tok".into()))),
    );
    attempt(
        "rename",
        controller
            .get_camera("storedA")
            .map_err(Error::from)
            .and_then(|c| {
                c.set_settings(
                    &vec![("name".into(), Value::Py(PyValue::Str("A again".into())))],
                    true,
                )
            }),
    );
    attempt("reset", configurator.reset_to_defaults("storedA"));
    let detected_id = CameraDriver::make_hash("/dev/video9").unwrap();
    attempt("remove_detected", configurator.remove_camera(&detected_id));
    attempt("remove", configurator.remove_camera("manual1"));
    attempt("reload", configurator.load_cameras(false));
    attempt("reload_configs", configurator.load_cameras(true));
    out.insert("configurator".into(), Json::Array(steps));

    Json::Object(out)
}

fn resolution_str(camera: &Arc<Camera>) -> String {
    match camera.output_resolution().unwrap() {
        Some(r) => r.to_string(),
        None => "None".into(),
    }
}

/// What is not compared as it is (see the module doc).
fn normalize(v: &Json, in_sent: bool) -> Json {
    match v {
        Json::Object(map) => Json::Object(
            map.iter()
                .map(|(k, x)| {
                    let x = if k == "headers" {
                        let headers: Vec<Json> = x
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|h| match h[0].as_str() {
                                Some("host") => json!(["host", "<host>"]),
                                Some("timestamp") if in_sent => json!(["timestamp", "<now>"]),
                                _ => h.clone(),
                            })
                            .collect();
                        Json::Array(headers)
                    } else if k.ends_with("calls") {
                        sort_set_runs(x)
                    } else {
                        normalize(x, in_sent || k == "controller")
                    };
                    (k.clone(), x)
                })
                .collect(),
        ),
        Json::Array(items) => Json::Array(items.iter().map(|x| normalize(x, in_sent)).collect()),
        other => other.clone(),
    }
}

/// A run of `set_*` calls, sorted; the photos after the rest, sorted.
fn sort_set_runs(calls: &Json) -> Json {
    let (mut photos, items): (Vec<Json>, Vec<Json>) = calls
        .as_array()
        .unwrap()
        .iter()
        .cloned()
        .partition(|c| c[0] == "take_a_photo");
    photos.sort_by_key(|x| x.to_string());
    let mut out: Vec<Json> = Vec::new();
    let mut run: Vec<Json> = Vec::new();
    for c in &items {
        if c[0].as_str().unwrap().starts_with("set_") {
            run.push(c.clone());
        } else {
            run.sort_by_key(|x| x.to_string());
            out.append(&mut run);
            out.push(c.clone());
        }
    }
    run.sort_by_key(|x| x.to_string());
    out.append(&mut run);
    out.push(Json::Array(photos));
    Json::Array(out)
}

#[test]
fn camera_like_the_sdk() {
    let Some(python) = python() else { return };
    let base = tmpdir("run");
    let log = base.join("log");

    // the reference
    let (server, port) = serve(&python, &log);
    let folder = base.join("reference");
    std::fs::create_dir_all(&folder).unwrap();
    let reference = Command::new(&python)
        .arg("-I")
        .arg(manifest("tests/camera_scenario.py"))
        .arg("run")
        .arg(port.to_string())
        .arg(&log)
        .arg(&folder)
        .stderr(Stdio::null())
        .output()
        .unwrap();
    assert!(reference.status.success(), "the scenario failed");
    let reference: Json = serde_json::from_slice(&reference.stdout).unwrap();

    // ours, on the same server
    let folder = base.join("ours");
    std::fs::create_dir_all(&folder).unwrap();
    let ours = run(port, &log, &folder);
    drop(server);

    let reference = normalize(&reference, false);
    let ours = normalize(&ours, false);
    let keys: Vec<&String> = reference.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ours.as_object().unwrap().keys().collect::<Vec<_>>(),
        "the steps"
    );
    let mut differ = Vec::new();
    for key in keys {
        let (r, o) = (&reference[key], &ours[key]);
        if let (Some(r), Some(o)) = (r.as_array(), o.as_array())
            && r.len() == o.len()
        {
            for (i, (a, b)) in r.iter().zip(o).enumerate() {
                if a != b {
                    differ.push(format!("{key}[{i}]\n  reference: {a}\n  ours:      {b}"));
                }
            }
        } else if r != o {
            differ.push(format!("{key}\n  reference: {r}\n  ours:      {o}"));
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    assert!(differ.is_empty(), "differs:\n{}", differ.join("\n"));
}
