//! OURS (a test, no part of the port): the proof of the Marlin port against real OctoPrint 1.11.8,
//! `docs/audits/octoprint-marlin.md`.
//!
//! The reference run is `scripts/octoprint-port-check.py`: OctoPrint's own `Printer` and `MachineCom`,
//! headless, against OctoPrint's own `VirtualPrinter` (`plugins/virtual_printer/virtual.py`, unchanged),
//! for each scenario of `tests/octoprint/scenarios.json`. This is our run: the same scenarios, as
//! `Order`s to the engine's `OctoPrintPort`, against the SAME Python `VirtualPrinter`, which is reached
//! through [`Bridge`], a serial object (`TEST_TRANSPORTS`, comm.rs) that talks to
//! `scripts/octoprint-vp-bridge.py`, a pure byte pass-through (no filtering, no retry, no timing of its
//! own). Both sides record every byte written to and every line read from the virtual printer, in
//! order, and are compared in two layers (see `compare`).
//!
//! OctoPrint's event manager ends its worker with `SHUTDOWN` (events.rs), so a process has one run of the
//! port: the test `proof_against_octoprint` starts the test binary again for each scenario
//! (`proof_child_run`), which runs it and writes `tests/octoprint/ours/<scenario>.json`.

use super::port::OctoPrintPort;
use crate::{Engine, Order};
use fancy_regex::Regex;
use maker_connect_protocol::{Action, JobControl, JobOrder, MachineState};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::comm::{SerialError, SerialTransport, TEST_TRANSPORTS};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/octoprint")
}

/// bytes as text, one character a byte (what the fixtures hold)
fn lat1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

// ---------------------------------------------------------------------------------------------
// the recording, as scripts/octoprint-port-check.py's `Recorder` does it
// ---------------------------------------------------------------------------------------------

struct RecInner {
    writes: Vec<String>,
    reads: Vec<String>,
    last: Instant,
}

struct Recorder {
    inner: Mutex<RecInner>,
    cond: Condvar,
}

impl Recorder {
    fn new() -> Recorder {
        Recorder { inner: Mutex::new(RecInner { writes: vec![], reads: vec![], last: Instant::now() }), cond: Condvar::new() }
    }

    fn write(&self, data: &[u8]) {
        let mut inner = lock(&self.inner);
        inner.writes.push(lat1(data));
        inner.last = Instant::now();
        self.cond.notify_all();
    }

    fn read(&self, line: &[u8]) {
        let mut inner = lock(&self.inner);
        inner.reads.push(lat1(line));
        inner.last = Instant::now();
        self.cond.notify_all();
    }

    /// wait until nothing was written or read for `quiet_ms`, counted from the start of the step at the
    /// earliest (the same rule as the reference's `settle`)
    fn settle(&self, quiet_ms: u64) {
        let quiet = Duration::from_millis(quiet_ms);
        let start = Instant::now();
        loop {
            let last = lock(&self.inner).last;
            let until = start.max(last) + quiet;
            let now = Instant::now();
            if now >= until {
                return;
            }
            std::thread::sleep(until - now);
        }
    }

    fn wait_count(&self, reads: bool, pattern: &str, count: usize) -> bool {
        let rx = Regex::new(pattern).expect("a valid pattern");
        let end = Instant::now() + Duration::from_secs(60);
        let mut inner = lock(&self.inner);
        loop {
            let list = if reads { &inner.reads } else { &inner.writes };
            if list.iter().filter(|x| rx.is_match(x).unwrap_or(false)).count() >= count {
                return true;
            }
            let now = Instant::now();
            if now >= end {
                return false;
            }
            inner = self.cond.wait_timeout(inner, end - now).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// the serial object: a pipe to scripts/octoprint-vp-bridge.py
// ---------------------------------------------------------------------------------------------

type Reply = Result<String, (String, String)>;

struct Bridge {
    name: String,
    baud: i64,
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    next: Mutex<u64>,
    waiting: Arc<Mutex<HashMap<u64, Sender<Reply>>>>,
    read_timeout: Mutex<f64>,
    rec: Arc<Recorder>,
}

impl Bridge {
    fn start(name: &str, scenario: &str, read_timeout: f64, baud: i64, rec: Arc<Recorder>) -> Arc<Bridge> {
        let python = root().join("reference/.venv-octoprint-313/bin/python");
        let mut child = Command::new(&python)
            .arg(root().join("scripts/octoprint-vp-bridge.py"))
            .arg(fixtures().join("scenarios.json"))
            .arg(scenario)
            .arg(read_timeout.to_string())
            .arg(baud.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("could not start the bridge with {python:?}: {e}"));
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let waiting: Arc<Mutex<HashMap<u64, Sender<Reply>>>> = Arc::new(Mutex::new(HashMap::new()));
        let (ready_tx, ready_rx) = channel::<()>();
        {
            let waiting = waiting.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let mut parts = line.splitn(3, ' ');
                    let (Some(id), Some(status)) = (parts.next(), parts.next()) else { continue };
                    let rest = parts.next().unwrap_or("").to_string();
                    let Ok(id) = id.parse::<u64>() else { continue };
                    if id == 0 {
                        let _ = ready_tx.send(());
                        continue;
                    }
                    let reply: Reply = if status == "ok" {
                        Ok(rest)
                    } else {
                        let (class, msg) = rest.split_once(' ').unwrap_or((rest.as_str(), ""));
                        Err((class.to_string(), String::from_utf8_lossy(&unhex(msg)).into_owned()))
                    };
                    if let Some(tx) = lock(&waiting).remove(&id) {
                        let _ = tx.send(reply);
                    }
                }
                // the bridge ended: everything still waiting fails
                for (_, tx) in lock(&waiting).drain() {
                    let _ = tx.send(Err(("SerialException".to_string(), "the bridge ended".to_string())));
                }
            });
        }
        ready_rx.recv_timeout(Duration::from_secs(60)).expect("the bridge did not say it was ready");
        Arc::new(Bridge {
            name: name.to_string(),
            baud,
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            next: Mutex::new(1),
            waiting,
            read_timeout: Mutex::new(read_timeout),
            rec,
        })
    }

    fn call(&self, request: &str) -> Reply {
        let id = {
            let mut n = lock(&self.next);
            let id = *n;
            *n += 1;
            id
        };
        let (tx, rx) = channel();
        lock(&self.waiting).insert(id, tx);
        {
            let mut stdin = lock(&self.stdin);
            if writeln!(stdin, "{id} {request}").and_then(|()| stdin.flush()).is_err() {
                lock(&self.waiting).remove(&id);
                return Err(("SerialException".to_string(), "the bridge ended".to_string()));
            }
        }
        rx.recv().unwrap_or_else(|_| Err(("SerialException".to_string(), "the bridge ended".to_string())))
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = lock(&self.child).kill();
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn serial_error(class: String, msg: String) -> SerialError {
    // pyserial's `SerialTimeoutException` is a `SerialException`
    SerialError::new(&class, msg, class.starts_with("Serial"))
}

impl SerialTransport for Bridge {
    fn port(&self) -> Option<Option<String>> {
        Some(Some(self.name.clone()))
    }
    fn baudrate(&self) -> Option<Option<i64>> {
        Some(Some(self.baud))
    }
    fn close(&self) -> Result<(), String> {
        self.call("C").map(|_| ()).map_err(|(class, msg)| format!("{class}: {msg}"))
    }
    fn str(&self) -> String {
        format!("VIRTUAL(read_timeout={})", lock(&self.read_timeout))
    }
    fn set_timeout(&self, timeout: f64) {
        // `VirtualPrinter.timeout = value`
        *lock(&self.read_timeout) = timeout;
        if let Err((class, msg)) = self.call(&format!("T {timeout}")) {
            panic!("setting the timeout: {class}: {msg}");
        }
    }
    fn timeout(&self) -> Option<f64> {
        // the value this object last set (`VirtualPrinter.timeout` returns what was set)
        Some(*lock(&self.read_timeout))
    }
    fn set_baudrate(&self, baudrate: i64) {
        panic!("VirtualPrinter.baudrate has no setter in OctoPrint's virtual printer; set to {baudrate}");
    }
    fn readline(&self) -> Result<Vec<u8>, SerialError> {
        match self.call("R") {
            Ok(h) => {
                let line = unhex(&h);
                if !line.is_empty() {
                    self.rec.read(&line);
                }
                Ok(line)
            }
            Err((class, msg)) => Err(serial_error(class, msg)),
        }
    }
    fn write(&self, data: &[u8]) -> Result<Option<i64>, SerialError> {
        self.rec.write(data);
        match self.call(&format!("W {}", hex(data))) {
            Ok(n) => Ok(Some(n.trim().parse::<i64>().expect("the bridge says how many bytes"))),
            Err((class, msg)) => Err(serial_error(class, msg)),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// our run of one scenario
// ---------------------------------------------------------------------------------------------

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) -> Result<(), String> {
    let end = Instant::now() + Duration::from_secs(60);
    while Instant::now() < end {
        if ready() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(format!("timed out waiting for {what}"))
}

/// Runs one scenario through the engine and the port; the transcript as the fixtures have it
fn run_scenario(top: &Value, scn: &Value, work: &Path) -> Value {
    let name = scn["name"].as_str().expect("a name").to_string();
    let rec = Arc::new(Recorder::new());

    // the settings changed from the defaults: the common ones, then the scenario's own
    // (`scripts/octoprint_proof_env.py: scenario_config`)
    fn merge(into: &mut Value, from: &Value) {
        match (into, from) {
            (Value::Object(a), Value::Object(b)) => {
                for (k, v) in b {
                    merge(a.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
            (a, b) => *a = b.clone(),
        }
    }
    let mut octoprint = top["octoprint"].clone();
    if let Some(own) = scn.get("octoprint") {
        merge(&mut octoprint, own);
    }
    // SAFETY: set before the settings singleton is first read, in a process of its own
    unsafe { std::env::set_var("MAKER_CONNECT_OCTOPRINT_PROOF_SETTINGS", octoprint.to_string()) };

    let read_timeout = super::settings::settings().get_float(&["serial", "timeout", "connection"]).expect("serial.timeout.connection");
    let serial_dir = PathBuf::from(super::comm::test_serial_dir());
    // a name `serialList` finds (regex_serial_devices) so that auto-detection has a port to try
    let device = serial_dir.join(format!("ttyUSBproof{}", std::process::id()));
    std::fs::write(&device, b"").expect("a stand-in device file in the test directory");
    let device_name = device.to_string_lossy().into_owned();

    let bridge = Bridge::start(&device_name, &name, read_timeout, 115_200, rec.clone());
    lock(&TEST_TRANSPORTS).push((device_name.clone(), bridge.clone()));

    let connect = &scn["connect"];
    let port = connect["port"].as_str().map(|_| device_name.clone());
    let baud = connect["baud"].as_u64().map(|b| b as u32);

    let mut engine = Engine::new();
    let id = engine.add(Box::new(OctoPrintPort::new(port, baud)));
    let mut error: Option<String> = None;
    let mut next_command = 0u32;
    let mut closed = false;

    let steps = scn["steps"].as_array().expect("steps");
    'steps: for step in steps {
        let result: Result<(), String> = (|| {
            if let Some(ms) = step.get("settle") {
                rec.settle(ms.as_u64().expect("ms"));
            } else if step.get("wait_operational").is_some() {
                wait_until("operational", || engine.report(id).is_ok_and(|r| r.telemetry.state == MachineState::Idle))?;
            } else if let Some(w) = step.get("wait_write") {
                let (m, c) = (w["match"].as_str().expect("match"), w.get("count").and_then(Value::as_u64).unwrap_or(1) as usize);
                if !rec.wait_count(false, m, c) {
                    return Err(format!("timed out waiting for a write matching \"{m}\""));
                }
            } else if let Some(w) = step.get("wait_read") {
                let (m, c) = (w["match"].as_str().expect("match"), w.get("count").and_then(Value::as_u64).unwrap_or(1) as usize);
                if !rec.wait_count(true, m, c) {
                    return Err(format!("timed out waiting for a line matching \"{m}\""));
                }
            } else if let Some(action) = step.get("command") {
                next_command += 1;
                let action: Action = serde_json::from_value(action.clone()).map_err(|e| format!("bad action: {e}"))?;
                engine.send(id, Order::Command { id: next_command, action }).map_err(|_| "the port ended".to_string())?;
            } else if let Some(job) = step.get("job") {
                next_command += 1;
                let file_name = job["file"].as_str().expect("file");
                let source = fixtures().join("files").join(file_name);
                let own = work.join(format!("job-{next_command}"));
                std::fs::copy(&source, &own).map_err(|e| format!("copying {source:?}: {e}"))?;
                let size = std::fs::metadata(&own).map(|m| m.len()).unwrap_or(0);
                let order = JobOrder { id: i64::from(next_command), name: file_name.to_string(), download: String::new(), size, from_line: None, tape_mm: None, label_length_mm: None };
                engine.send(id, Order::Job { order, file: own }).map_err(|_| "the port ended".to_string())?;
            } else if let Some(control) = step.get("control") {
                let control = match control.as_str().expect("control") {
                    "pause" => JobControl::Pause,
                    "resume" => JobControl::Resume,
                    "stop" => JobControl::Stop,
                    other => return Err(format!("no control {other}")),
                };
                engine.send(id, Order::Control(control)).map_err(|_| "the port ended".to_string())?;
            } else if let Some(choice) = step.get("answer") {
                engine.send(id, Order::Answer(choice.as_u64().expect("a button") as u8)).map_err(|_| "the port ended".to_string())?;
            } else if step.get("close").is_some() {
                // Order::Close: the port disconnects and ends; give it a time to do so
                closed = true;
                let (tx, rx) = channel();
                let mut engine_taken = Engine::new();
                std::mem::swap(&mut engine, &mut engine_taken);
                std::thread::spawn(move || {
                    let mut engine = engine_taken;
                    let _ = engine.remove(id);
                    let _ = tx.send(());
                });
                if rx.recv_timeout(Duration::from_secs(30)).is_err() {
                    return Err("the port did not end within 30 s of Order::Close".to_string());
                }
            } else {
                return Err(format!("unknown step {step}"));
            }
            Ok(())
        })();
        if let Err(e) = result {
            error = Some(e);
            break 'steps;
        }
    }
    if !closed {
        // an error ended the scenario early: the port is left to the process's end
        std::mem::forget(engine);
    }
    let inner = lock(&rec.inner);
    json!({ "scenario": name, "writes": inner.writes, "reads": inner.reads, "error": error })
}

// ---------------------------------------------------------------------------------------------
// the comparison (docs/audits/octoprint-marlin.md, "the normalisation rules")
// ---------------------------------------------------------------------------------------------

/// (b): the lines read back after dropping only `wait` lines and, where the simulated temperatures
/// are moving, the numbers behind `T:` and `B:`. Returns the lines, the number of `wait` lines dropped
/// and the number of numbers masked.
fn normalise(moving: bool, reads: &[String]) -> (Vec<String>, usize, usize) {
    let rx = Regex::new(r"(?<![A-Za-z0-9_])([TB]\d*:)\s*-?\d+(?:\.\d+)?").expect("valid");
    let (mut out, mut waits, mut masked) = (Vec::new(), 0, 0);
    for line in reads {
        if line.trim() == "wait" {
            waits += 1;
            continue;
        }
        if moving {
            masked += rx.find_iter(line).count();
            out.push(rx.replace_all(line, "${1}N").into_owned());
        } else {
            out.push(line.clone());
        }
    }
    (out, waits, masked)
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(|s| s.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
}

/// The lines that are in one list and not in the other, as `-line` (the reference's only) and `+line`
/// (ours only), in order: the longest common subsequence stays, as `diff` does it
fn diff_lines(reference: &[String], ours: &[String]) -> Vec<String> {
    let (n, m) = (reference.len(), ours.len());
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if reference[i] == ours[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n || j < m {
        if i < n && j < m && reference[i] == ours[j] {
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            out.push(format!("+{}", ours[j].trim_end_matches('\n')));
            j += 1;
        } else {
            out.push(format!("-{}", reference[i].trim_end_matches('\n')));
            i += 1;
        }
    }
    out
}

/// Compares one run of ours with the reference for one scenario: both layers and the error
fn compare_run(scn: &Value, reference: &Value, ours: &Value) -> Value {
    let moving = scn.get("temperatures_moving").and_then(Value::as_bool).unwrap_or(false);
    let (rw, ow) = (strings(&reference["writes"]), strings(&ours["writes"]));
    let (rr, dropped_ref, masked_ref) = normalise(moving, &strings(&reference["reads"]));
    let (or, dropped_ours, masked_ours) = normalise(moving, &strings(&ours["reads"]));
    let error_difference: Vec<Value> = if reference["error"] == ours["error"] { vec![] } else { vec![json!({ "reference": reference["error"], "ours": ours["error"] })] };
    json!({
        "signature": { "writes": diff_lines(&rw, &ow), "reads": diff_lines(&rr, &or), "error": error_difference },
        "writes_identical": rw == ow,
        "reads_identical": rr == or,
        "error_identical": reference["error"] == ours["error"],
        "writes_count": [rw.len(), ow.len()],
        "reads_count": [rr.len(), or.len()],
        "dropped_wait": [dropped_ref, dropped_ours],
        "masked": [masked_ref, masked_ours],
    })
}

/// The distinct differences of the runs, each with the number of runs that had it (an identical run has
/// the empty one)
fn signatures(compared: &[Value]) -> Value {
    let mut seen: Vec<(Value, usize)> = Vec::new();
    for c in compared {
        match seen.iter_mut().find(|(s, _)| *s == c["signature"]) {
            Some((_, n)) => *n += 1,
            None => seen.push((c["signature"].clone(), 1)),
        }
    }
    Value::Array(seen.into_iter().map(|(signature, runs)| json!({ "runs": runs, "signature": signature })).collect())
}

/// How many runs of ours (of `runs`) were identical in a layer
fn count(compared: &[Value], key: &str) -> usize {
    compared.iter().filter(|c| c[key] == true).count()
}

/// Ours, `RUNS` times, against the reference's transcript: "identical" if every run was identical in
/// both layers (and ended as the reference did), "differs" if none was, else "varies"
fn compare(scn: &Value, reference: &Value, runs: &[Value], reproducible: bool) -> Value {
    let compared: Vec<Value> = runs.iter().map(|r| compare_run(scn, reference, r)).collect();
    let whole = compared.iter().filter(|c| c["writes_identical"] == true && c["reads_identical"] == true && c["error_identical"] == true).count();
    let verdict = if whole == runs.len() { "identical" } else if whole == 0 { "differs" } else { "varies" };
    json!({
        "reproducible": reproducible,
        "verdict": verdict,
        "runs": runs.len(),
        "identical_runs": whole,
        "writes_identical_runs": count(&compared, "writes_identical"),
        "reads_identical_runs": count(&compared, "reads_identical"),
        "error_identical_runs": count(&compared, "error_identical"),
        "first_run": compared[0],
        "signatures": signatures(&compared),
        "reference_error": reference["error"],
        "ours_errors": runs.iter().map(|r| r["error"].clone()).collect::<Vec<_>>(),
    })
}

/// How many times each scenario is run (the reference too: `REPS` of `scripts/octoprint-port-check.py`)
const RUNS: usize = 5;

#[cfg(test)]
mod tests {
    use super::*;

    /// The child of the proof: runs the scenario named by `MAKER_CONNECT_PROOF_SCENARIO` (when it is set),
    /// writes its transcript to `MAKER_CONNECT_PROOF_OUT`, and ends the process (the engine's event
    /// manager has one run of a port per process). Without the variable it does nothing.
    #[test]
    #[ignore = "the child of the proof; the proof starts it (scripts/octoprint-own-process-tests.sh)"]
    fn proof_child_run() {
        let (Ok(name), Ok(out)) = (std::env::var("MAKER_CONNECT_PROOF_SCENARIO"), std::env::var("MAKER_CONNECT_PROOF_OUT")) else {
            return;
        };
        let top: Value = serde_json::from_str(&std::fs::read_to_string(fixtures().join("scenarios.json")).unwrap()).unwrap();
        let scn = top["scenarios"].as_array().unwrap().iter().find(|s| s["name"] == name.as_str()).expect("a scenario of that name");
        let work = std::env::temp_dir().join(format!("maker-connect-proof-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        let transcript = run_scenario(&top, scn, &work);
        std::fs::write(&out, serde_json::to_string(&transcript).unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&work);
        // the threads of the port (the monitor, the event manager's worker) end with the process
        std::process::exit(0);
    }

    /// Runs every scenario of `tests/octoprint/scenarios.json` through our port against OctoPrint's
    /// virtual printer, writes `tests/octoprint/ours/<scenario>.json` and `ours/result.json`, and
    /// compares them with the reference run's fixtures (`scripts/octoprint-port-check.py reference`).
    /// It REPORTS the measured differences; it asserts that they are the ones in
    /// `tests/octoprint/known-differences.json` (written with `MAKER_CONNECT_PROOF_UPDATE=1`), so the
    /// audit stays honest and a change of the port that changes a transcript shows up.
    ///
    /// Not in the unit suite (it starts 120 processes and needs `reference/.venv-octoprint-313`): run it with
    /// `scripts/octoprint-own-process-tests.sh proof`, or
    /// `cargo test -p maker-connect-engine --lib -- --ignored --exact ports::octoprint::proof::tests::proof_against_octoprint --nocapture`.
    #[test]
    #[ignore = "the proof against OctoPrint: scripts/octoprint-own-process-tests.sh proof"]
    fn proof_against_octoprint() {
        if std::env::var("MAKER_CONNECT_PROOF_SCENARIO").is_ok() {
            return;
        }
        let top: Value = serde_json::from_str(&std::fs::read_to_string(fixtures().join("scenarios.json")).unwrap()).unwrap();
        let summary: Value = serde_json::from_str(&std::fs::read_to_string(fixtures().join("reference/summary.json")).expect("run `scripts/octoprint-port-check.py reference` first")).unwrap();
        let scenarios = top["scenarios"].as_array().unwrap().clone();
        std::fs::create_dir_all(fixtures().join("ours")).unwrap();

        // each run of each scenario in a process of its own, six at a time
        let exe = std::env::current_exe().unwrap();
        let tmp = std::env::temp_dir().join(format!("maker-connect-proof-runs-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let queue = Arc::new(Mutex::new(
            scenarios.iter().flat_map(|s| (0..RUNS).map(move |k| (s["name"].as_str().unwrap().to_string(), k))).collect::<Vec<_>>(),
        ));
        let ran: Arc<Mutex<HashMap<(String, usize), bool>>> = Arc::new(Mutex::new(HashMap::new()));
        let workers: Vec<_> = (0..6)
            .map(|_| {
                let (queue, ran, exe, tmp) = (queue.clone(), ran.clone(), exe.clone(), tmp.clone());
                std::thread::spawn(move || loop {
                    let Some((name, k)) = lock(&queue).pop() else { return };
                    let out = tmp.join(format!("{name}.{k}.json"));
                    let private = tmp.join(format!("{name}.{k}.d"));
                    std::fs::create_dir_all(&private).expect("a private temp directory");
                    let mut child = Command::new(&exe)
                        .args(["--ignored", "--exact", "ports::octoprint::proof::tests::proof_child_run", "--nocapture", "--test-threads=1"])
                        .env("MAKER_CONNECT_PROOF_SCENARIO", &name)
                        .env("MAKER_CONNECT_PROOF_OUT", &out)
                        // a temp directory of its own: the stand-in device file that the auto-detection finds
                        // is in `<temp>/maker-connect-test-serial-dev`, which other tests (running at the
                        // same time) expect to be empty, and which another run must not find either
                        .env("TMPDIR", &private)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                        .expect("the test binary again");
                    let deadline = Instant::now() + Duration::from_secs(240);
                    let finished = loop {
                        match child.try_wait() {
                            Ok(Some(_)) => break true,
                            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
                            _ => break false,
                        }
                    };
                    if !finished {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    lock(&ran).insert((name, k), finished);
                })
            })
            .collect();
        for w in workers {
            w.join().unwrap();
        }
        let finished = lock(&ran).clone();

        let mut result = serde_json::Map::new();
        for scn in &scenarios {
            let name = scn["name"].as_str().unwrap();
            let reference: Value = serde_json::from_str(&std::fs::read_to_string(fixtures().join(format!("reference/{name}.json"))).unwrap()).unwrap();
            let reproducible = summary[name]["reproducible"].as_bool().unwrap_or(false);
            let runs: Vec<Value> = (0..RUNS)
                .map(|k| match std::fs::read_to_string(tmp.join(format!("{name}.{k}.json"))) {
                    Ok(text) => serde_json::from_str(&text).unwrap(),
                    Err(_) => json!({ "scenario": name, "writes": [], "reads": [], "error": if finished.get(&(name.to_string(), k)) == Some(&true) { "the run wrote no transcript" } else { "the run did not end within 240 s" } }),
                })
                .collect();
            std::fs::write(fixtures().join(format!("ours/{name}.json")), serde_json::to_string(&json!({ "runs": runs })).unwrap()).unwrap();
            result.insert(name.to_string(), compare(scn, &reference, &runs, reproducible));
        }
        let _ = std::fs::remove_dir_all(&tmp);
        let result = Value::Object(result);
        std::fs::write(fixtures().join("ours/result.json"), serde_json::to_string_pretty(&result).unwrap() + "\n").unwrap();

        println!("{:<24} {:<13} {:<10} {:>10} {:>8} {:>8}  {}", "scenario", "reproducible", "verdict", "identical", "writes", "reads", "");
        for (name, r) in result.as_object().unwrap() {
            println!(
                "{:<24} {:<13} {:<10} {:>7}/{} {:>6}/{} {:>6}/{}",
                name,
                r["reproducible"],
                r["verdict"].as_str().unwrap(),
                r["identical_runs"],
                RUNS,
                r["writes_identical_runs"],
                RUNS,
                r["reads_identical_runs"],
                RUNS,
            );
        }

        // The guard: every difference measured is a KNOWN one (`known-differences.json`: what has been measured
        // so far, per scenario). A scenario's runs may differ from each other (a race of the port's threads,
        // see the audit), so "the same set" would fail by chance. Known are the lists of writes that differ
        // (exact), the errors, and the lines read that differ (each line); a difference that is not known
        // fails here. The startup race (see the audit) happens before any scenario's own steps, so what is
        // known is known for every scenario. `MAKER_CONNECT_PROOF_UPDATE=1` adds what was measured to the list.
        let known_path = fixtures().join("known-differences.json");
        let mut known: Value = std::fs::read_to_string(&known_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
        let update = std::env::var("MAKER_CONNECT_PROOF_UPDATE").is_ok();
        let mut unknown = Vec::new();
        let union = |known: &Value, key: &str| -> Vec<Value> { known.as_object().unwrap().values().flat_map(|v| v[key].as_array().unwrap().clone()).collect() };
        let (known_writes, known_errors, known_lines) = (union(&known, "writes"), union(&known, "errors"), union(&known, "read_lines"));
        for (name, r) in result.as_object().unwrap() {
            let mut now = known.get(name).cloned().unwrap_or_else(|| json!({ "reproducible": r["reproducible"], "writes": [], "read_lines": [], "errors": [] }));
            for s in r["signatures"].as_array().unwrap() {
                let signature = &s["signature"];
                let (writes, reads, error) = (&signature["writes"], signature["reads"].as_array().unwrap(), &signature["error"]);
                if !writes.as_array().unwrap().is_empty() && !known_writes.contains(writes) {
                    unknown.push(format!("{name}: writes {writes}"));
                }
                if !writes.as_array().unwrap().is_empty() && !now["writes"].as_array().unwrap().contains(writes) {
                    now["writes"].as_array_mut().unwrap().push(writes.clone());
                }
                if !error.as_array().unwrap().is_empty() && !known_errors.contains(error) {
                    unknown.push(format!("{name}: error {error}"));
                }
                if !error.as_array().unwrap().is_empty() && !now["errors"].as_array().unwrap().contains(error) {
                    now["errors"].as_array_mut().unwrap().push(error.clone());
                }
                for line in reads {
                    if !known_lines.contains(line) {
                        unknown.push(format!("{name}: line {line}"));
                    }
                    if !now["read_lines"].as_array().unwrap().contains(line) {
                        now["read_lines"].as_array_mut().unwrap().push(line.clone());
                    }
                }
            }
            if known.get(name).is_some() && known[name]["reproducible"] != r["reproducible"] {
                unknown.push(format!("{name}: reproducible is {} (known: {})", r["reproducible"], known[name]["reproducible"]));
            }
            known[name] = now;
        }
        if update {
            std::fs::write(&known_path, serde_json::to_string_pretty(&known).unwrap() + "\n").unwrap();
        } else if known_path.exists() {
            assert!(unknown.is_empty(), "differences that are not in tests/octoprint/known-differences.json:\n{}", unknown.join("\n"));
        }
    }
}
