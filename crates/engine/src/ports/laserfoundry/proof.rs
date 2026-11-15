//! The proof (`docs/audits/laserfoundry-grbl.md`): LaserFoundry's own `SimulatedGrbl` driven once
//! through LaserFoundry's `Link` directly, in the order of calls its app makes (the file and
//! line of each call is at the step), and once through this port, with every byte each side wrote
//! recorded. The `?` bytes (LaserFoundry's own 200 ms status poll) are counted apart and not
//! compared: how many there are depends on how long a scenario takes.
//!
//! Where a pause or a stop falls among a job's lines depends on when the observer notices the
//! progress (both look every 100 ms, G7) and the order is given at a set time after the job starts, so
//! there the commands and the real-time bytes are compared in order, and the lines must be the
//! program's, in order, each once.

use super::port::LaserFoundryPort;
use crate::{Event, Order, Port};
use laserfoundry_device::{JobState, Link, SimulatedGrbl, Snapshot, Transport, restart_line};
use maker_connect_grbl::GrblMachine;
use maker_connect_protocol::{Action, JobControl, JobOrder, Telemetry};
use std::io;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A transport that records every byte written to it.
struct Recorder {
    inner: SimulatedGrbl,
    wire: Arc<Mutex<Vec<u8>>>,
}

impl Transport for Recorder {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.wire.lock().unwrap().extend_from_slice(bytes);
        self.inner.write_all(bytes)
    }
    fn read_line(&mut self, timeout: Duration) -> io::Result<Option<String>> {
        self.inner.read_line(timeout)
    }
}

fn sim() -> SimulatedGrbl {
    let mut sim = SimulatedGrbl::new();
    sim.set_latency(Duration::from_millis(2));
    sim
}

/// What was written, as tokens: `?` counted apart, real-time bytes (`!`, `~`, reset) and
/// each line by itself.
#[derive(Debug, Default, Clone, PartialEq)]
struct Wire {
    tokens: Vec<String>,
    polls: usize,
}

fn tokens(bytes: &[u8]) -> Wire {
    let mut wire = Wire::default();
    let mut line = Vec::new();
    for &b in bytes {
        match b {
            b'?' if line.is_empty() => wire.polls += 1,
            b'!' | b'~' | 0x18 if line.is_empty() => wire.tokens.push(format!("<{:02x}>", b)),
            b'\n' => {
                wire.tokens.push(String::from_utf8_lossy(&line).into_owned());
                line.clear();
            }
            b => line.push(b),
        }
    }
    assert!(line.is_empty(), "a line was left half written");
    wire
}

/// When a scenario presses Stop or Pause, from the job's start, and how long a pause lasts, the same
/// on both sides: the order is given at that moment (LaserFoundry's app calls `Link::stop` and
/// `pause` as the button is pressed, `panels.rs:766-776`, `control_box.rs:141-149`), not when a
/// look at the progress notices something.
const HOLD: Duration = Duration::from_millis(400);
const PAUSED: Duration = Duration::from_millis(300);

/// The reference's side looks at its link every 100 ms, as LaserFoundry's app does for a threaded
/// link (`ui-egui/src/lib.rs:1384`), the cadence of the port (G7).
const LOOK: Duration = Duration::from_millis(100);

fn wait(what: &str, ready: impl FnMut() -> bool) {
    wait_every(what, LOOK, ready)
}

fn wait_every(what: &str, every: Duration, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(every);
    }
}

fn job_program(lines: usize) -> String {
    (0..lines).map(|i| format!("G1 X{} Y{} S300 F3000\n", i % 50, i / 50)).collect()
}

/// The program of a frame as the port makes it: `gcode::frame` over the controller's own
/// travel and power range (G12).
fn frame_program(bounds: [f64; 4]) -> String {
    let mut machine = laserfoundry_doc::Machine::default();
    machine.origin = laserfoundry_doc::Origin::TopLeft;
    (machine.bed_width, machine.bed_height, machine.max_s) = (200.0, 200.0, 1000);
    laserfoundry_gcode::frame(
        laserfoundry_geom::Rect::new(bounds[0], bounds[1], bounds[2], bounds[3]),
        &machine,
        laserfoundry_gcode::Placement::Absolute,
        3000.0,
        0.0,
    )
    .unwrap()
}

#[derive(Debug, Clone)]
enum Step {
    Home,
    Unlock,
    Jog(f64, f64, f64),
    SetOrigin,
    GoOrigin,
    ReadSettings,
    Line(&'static str),
    Job(String),
    Frame([f64; 4]),
    PauseResume(String),
    Stop(String),
    /// A job stopped, then started again from the line the first one says (`restart`) or the given one.
    Restart(String),
}

/// What a side found out, besides the bytes.
#[derive(Default, Debug)]
struct Outcome {
    wire: Wire,
    /// The line a stopped job says to start again from.
    restart_line: Option<usize>,
}

// ---- the reference: LaserFoundry's `Link`, the app's calls ----

fn answers(snap: &Snapshot) -> usize {
    snap.console
        .iter()
        .filter(|l| {
            let l = l.trim_start_matches("< ");
            l == "ok" || l.starts_with("ok ") || l.starts_with("error:") || l.starts_with("ALARM:")
        })
        .count()
}

fn finished(snap: &Snapshot) -> bool {
    matches!(snap.job, JobState::Finished(_))
}

fn direct(steps: &[Step], restart: Option<usize>) -> Outcome {
    let wire = Arc::new(Mutex::new(Vec::new()));
    let mut out = Outcome::default();
    {
        // `ui-egui/src/lib.rs:1177`
        let link = Link::connect(Box::new(Recorder { inner: sim(), wire: wire.clone() }));
        // `ui-egui/src/lib.rs:1401-1405`: the settings once the first status report is in.
        wait("a status", || link.snapshot().status.is_some());
        link.request_settings().unwrap();
        wait("the settings", || link.snapshot().settings.len() > 20);
        wait("the settings answer", || answers(&link.snapshot()) >= 1);
        let command = |f: &dyn Fn(&Link) -> Result<(), laserfoundry_device::LinkError>| {
            let before = answers(&link.snapshot());
            f(&link).unwrap();
            wait("an answer", || answers(&link.snapshot()) > before);
        };
        for step in steps {
            match step {
                // `control_box.rs:157`, `panels.rs:838`
                Step::Home => command(&|l| l.home()),
                // `control_box.rs:160`, `panels.rs:841`
                Step::Unlock => command(&|l| l.unlock()),
                // `panels.rs:813`
                Step::Jog(dx, dy, feed) => command(&|l| l.jog(*dx, *dy, *feed)),
                // `control_box.rs:163`, `panels.rs:844`
                Step::SetOrigin => command(&|l| l.set_zero()),
                // `control_box.rs:166`, `panels.rs:847`
                Step::GoOrigin => command(&|l| l.go_to_zero()),
                // `settings_ui.rs:60`
                Step::ReadSettings => command(&|l| l.request_settings()),
                // `panels.rs:865`
                Step::Line(line) => command(&|l| l.send_line(line)),
                // `ui-egui/src/lib.rs:1270-1273`, `begin_job`
                Step::Job(program) => {
                    link.start_job(program.clone()).unwrap();
                    wait("the end", || finished(&link.snapshot()));
                }
                // `panels.rs:734-740`, `frame_action`: `gcode::frame`, then `start_job`
                Step::Frame(bounds) => {
                    link.start_job(frame_program(*bounds)).unwrap();
                    wait("the end", || finished(&link.snapshot()));
                }
                // `panels.rs:766-776`
                Step::PauseResume(program) => {
                    link.start_job(program.clone()).unwrap();
                    std::thread::sleep(HOLD);
                    link.pause().unwrap();
                    std::thread::sleep(PAUSED);
                    link.resume().unwrap();
                    wait("the end", || finished(&link.snapshot()));
                }
                Step::Stop(program) => {
                    link.start_job(program.clone()).unwrap();
                    std::thread::sleep(HOLD);
                    link.stop().unwrap();
                    wait("the end", || finished(&link.snapshot()));
                }
                // `ui-egui/src/lib.rs:1258-1266` `start_from_line`, `:1291-1328` `record_job_position`
                Step::Restart(program) => {
                    link.start_job(program.clone()).unwrap();
                    std::thread::sleep(HOLD);
                    link.stop().unwrap();
                    wait("the stop", || finished(&link.snapshot()));
                    let acknowledged = link.snapshot().progress.map_or(0, |p| p.acknowledged);
                    let total = program.lines().count();
                    let line = restart.unwrap_or_else(|| restart_line(program, acknowledged).min(total.max(1)));
                    out.restart_line = Some(line);
                    let text = laserfoundry_gcode::resume_from(program, line).unwrap();
                    // The earlier job's state still shows until the link takes this one up.
                    let before = link.snapshot().job;
                    link.start_job(text).unwrap();
                    wait("the second job", || link.snapshot().job != before);
                    wait("the end", || finished(&link.snapshot()));
                }
            }
        }
    }
    out.wire = tokens(&wire.lock().unwrap());
    out
}

// ---- ours: the port, through the engine's orders ----

struct Port_ {
    orders: Sender<Order>,
    events: Receiver<Event>,
    thread: Option<std::thread::JoinHandle<()>>,
    telemetry: Telemetry,
    ends: Vec<maker_connect_protocol::JobEnd>,
    done: Vec<maker_connect_protocol::CommandDone>,
    next_id: u32,
    dir: std::path::PathBuf,
}

impl Port_ {
    fn start(wire: Arc<Mutex<Vec<u8>>>) -> Port_ {
        let machine = GrblMachine::new(Box::new(Recorder { inner: sim(), wire }));
        let port: Box<dyn Port> = Box::new(LaserFoundryPort::with_machine(machine));
        let (orders, port_orders) = channel();
        let (port_events, events) = channel();
        let thread = std::thread::spawn(move || port.run(port_orders, port_events));
        let dir = std::env::temp_dir().join(format!("laserfoundry-proof-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        Port_ { orders, events, thread: Some(thread), telemetry: Telemetry::default(), ends: Vec::new(), done: Vec::new(), next_id: 1, dir }
    }

    fn pump(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Telemetry(t) => self.telemetry = *t,
                Event::Done(d) => self.done.push(d),
                Event::JobEnd(e) => self.ends.push(e),
                Event::Gone | Event::Back => {}
            }
        }
    }

    fn until(&mut self, what: &str, mut ready: impl FnMut(&mut Port_) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.pump();
            if ready(self) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}: {:?}", self.telemetry);
            std::thread::sleep(LOOK);
        }
    }

    fn command(&mut self, action: Action) {
        let id = self.next_id;
        self.next_id += 1;
        self.orders.send(Order::Command { id, action }).unwrap();
        self.until("a command done", |p| p.done.iter().any(|d| d.id == id));
        let done = self.done.iter().find(|d| d.id == id).unwrap();
        assert!(done.ok, "{done:?}");
    }

    fn job(&mut self, id: i64, program: &str, from_line: Option<u64>) {
        let file = self.dir.join(format!("job-{id}"));
        std::fs::write(&file, program).unwrap();
        let order = JobOrder { id, name: format!("job-{id}"), download: String::new(), size: program.len() as u64, from_line, tape_mm: None, label_length_mm: None };
        self.orders.send(Order::Job { order, file }).unwrap();
    }

    fn ended(&mut self, id: i64) -> maker_connect_protocol::JobEnd {
        self.until("a job end", |p| p.ends.iter().any(|e| e.id == id));
        self.ends.iter().find(|e| e.id == id).unwrap().clone()
    }

    fn acknowledged(&mut self) -> u64 {
        self.pump();
        self.telemetry.job.as_ref().map_or(0, |j| j.done)
    }
}

impl Drop for Port_ {
    fn drop(&mut self) {
        let _ = self.orders.send(Order::Close);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn ours(steps: &[Step], restart: Option<usize>) -> Outcome {
    if let [Step::Frame(bounds)] = steps {
        return frame_ours(steps, *bounds);
    }
    let wire = Arc::new(Mutex::new(Vec::new()));
    let mut out = Outcome::default();
    {
        let mut port = Port_::start(wire.clone());
        // The settings: once the first status report is in (`crates/grbl`'s `report`).
        port.until("the settings", |p| p.telemetry.grbl.is_some());
        let mut job_id = 0;
        for step in steps {
            match step {
                Step::Home => port.command(Action::Home { axes: vec![maker_connect_protocol::Axis::X, maker_connect_protocol::Axis::Y] }),
                Step::Unlock => port.command(Action::Unlock),
                Step::Jog(dx, dy, feed) => port.command(Action::Jog { x: Some(*dx), y: Some(*dy), z: None, feed: Some(*feed) }),
                Step::SetOrigin => port.command(Action::SetOrigin),
                Step::GoOrigin => port.command(Action::GoOrigin),
                Step::ReadSettings => port.command(Action::ReadSettings),
                Step::Line(line) => port.command(Action::Gcode { text: (*line).to_owned() }),
                Step::Job(program) => {
                    job_id += 1;
                    port.job(job_id, program, None);
                    assert!(port.ended(job_id).ok);
                }
                Step::Frame(_) => unreachable!("a frame is run by `frame_ours`"),
                Step::PauseResume(program) => {
                    job_id += 1;
                    port.job(job_id, program, None);
                    std::thread::sleep(HOLD);
                    port.orders.send(Order::Control(JobControl::Pause)).unwrap();
                    std::thread::sleep(PAUSED);
                    port.orders.send(Order::Control(JobControl::Resume)).unwrap();
                    assert!(port.ended(job_id).ok);
                }
                Step::Stop(program) => {
                    job_id += 1;
                    port.job(job_id, program, None);
                    std::thread::sleep(HOLD);
                    port.orders.send(Order::Control(JobControl::Stop)).unwrap();
                    assert!(port.ended(job_id).stopped);
                }
                Step::Restart(program) => {
                    job_id += 1;
                    port.job(job_id, program, None);
                    std::thread::sleep(HOLD);
                    port.orders.send(Order::Control(JobControl::Stop)).unwrap();
                    let first = port.ended(job_id);
                    let line = restart.map_or_else(|| first.restart_line.unwrap() as usize, |l| l);
                    out.restart_line = first.restart_line.map(|l| l as usize);
                    job_id += 1;
                    port.job(job_id, program, Some(line as u64));
                    assert!(port.ended(job_id).ok);
                }
            }
        }
    }
    out.wire = tokens(&wire.lock().unwrap());
    out
}

/// A frame through the machine under the port (`GrblMachine::frame`).
fn frame_ours(steps: &[Step], bounds: [f64; 4]) -> Outcome {
    assert_eq!(steps.len(), 1, "a frame scenario is a frame alone");
    let wire = Arc::new(Mutex::new(Vec::new()));
    {
        let mut machine = GrblMachine::new(Box::new(Recorder { inner: sim(), wire: wire.clone() }));
        wait("the settings", || {
            let update = machine.report();
            update.telemetry.grbl.is_some()
        });
        let ticket = machine.frame(bounds).unwrap();
        let mut done = false;
        wait("the frame", || {
            done |= machine.report().done.iter().any(|d| d.ticket == ticket && d.result.is_ok());
            done
        });
    }
    Outcome { wire: tokens(&wire.lock().unwrap()), restart_line: None }
}

// ---- the comparison ----

fn is_realtime(token: &str) -> bool {
    token.starts_with('<') && token.ends_with('>')
}

/// Compared in order: everything but the job's lines (commands, real-time bytes). The job's
/// lines are the program's, in order, each once.
fn framing(wire: &Wire, program: &str) -> Vec<String> {
    let lines: Vec<&str> = program.lines().collect();
    let job: Vec<&str> = wire.tokens.iter().filter(|t| t.starts_with('G')).map(String::as_str).collect();
    assert!(job.len() <= lines.len() && job == lines[..job.len()], "the lines are not the program's");
    wire.tokens.iter().filter(|t| is_realtime(t) || t.starts_with('$')).cloned().collect()
}

fn same(name: &str, steps: Vec<Step>) -> (Wire, Wire) {
    let reference = direct(&steps, None);
    let ours = ours(&steps, None);
    eprintln!("{name}: reference tokens {:?}", reference.wire.tokens.iter().take(12).collect::<Vec<_>>());
    eprintln!("{name}: reference {} tokens, {} status polls (not compared); ours {} tokens, {} polls", reference.wire.tokens.len(), reference.wire.polls, ours.wire.tokens.len(), ours.wire.polls);
    (reference.wire, ours.wire)
}

#[test]
fn connecting_sends_only_the_settings() {
    let (reference, ours) = same("connect", vec![]);
    assert_eq!(reference.tokens, ["$$"]);
    assert_eq!(ours.tokens, reference.tokens);
}

#[test]
fn every_control_sends_the_same_bytes() {
    for (name, steps) in [
        ("home", vec![Step::Home]),
        ("unlock", vec![Step::Unlock]),
        ("jog", vec![Step::Jog(10.0, -2.5, 3000.0)]),
        ("set and go to origin", vec![Step::Jog(30.0, 40.0, 3000.0), Step::SetOrigin, Step::Jog(5.0, 5.0, 1000.0), Step::GoOrigin]),
        ("read settings", vec![Step::ReadSettings]),
        ("typed line", vec![Step::Line("G0 X20 Y20")]),
        ("everything in a row", vec![Step::Home, Step::Unlock, Step::Jog(1.0, 2.0, 500.0), Step::SetOrigin, Step::GoOrigin, Step::ReadSettings, Step::Line("M5")]),
    ] {
        let (reference, ours) = same(name, steps);
        assert_eq!(ours.tokens, reference.tokens, "{name}");
        assert!(reference.tokens.len() >= 2, "{name}: {:?}", reference.tokens);
    }
}

#[test]
fn a_job_to_the_end_sends_the_same_bytes() {
    let program = job_program(150);
    let (reference, ours) = same("job", vec![Step::Job(program)]);
    assert_eq!(ours.tokens, reference.tokens);
    assert_eq!(reference.tokens.len(), 1 + 150);
}

#[test]
fn a_frame_sends_the_same_bytes() {
    let bounds = [10.0, 10.0, 30.0, 20.0];
    let reference = direct(&[Step::Frame(bounds)], None);
    let ours = frame_ours(&[Step::Frame(bounds)], bounds);
    assert_eq!(ours.wire.tokens, reference.wire.tokens);
    eprintln!("frame: {} tokens, first {:?}", reference.wire.tokens.len(), reference.wire.tokens.iter().take(9).collect::<Vec<_>>());
    assert!(reference.wire.tokens.len() > 8, "{:?}", reference.wire.tokens);
}

#[test]
fn pause_resume_and_stop_send_the_same_realtime_bytes() {
    let program = job_program(400);
    for (name, step, expected) in [
        ("pause and resume", Step::PauseResume(program.clone()), vec!["$$", "<21>", "<7e>"]),
        ("stop", Step::Stop(program.clone()), vec!["$$", "<18>"]),
    ] {
        let reference = direct(&[step.clone()], None);
        let ours = ours(&[step], None);
        eprintln!("{name}: {} polls reference, {} ours; {:?}", reference.wire.polls, ours.wire.polls, expected);
        assert_eq!(framing(&reference.wire, &program), expected, "{name}");
        assert_eq!(framing(&ours.wire, &program), expected, "{name}");
    }
}

#[test]
fn a_restart_from_a_line_sends_the_same_bytes() {
    let program = job_program(400);
    let step = Step::Restart(program.clone());
    let reference = direct(&[step.clone()], None);
    let line = reference.restart_line.expect("a line");
    assert!(line > 1, "{line}");
    // Ours restarted at the reference's line: the second job is the same bytes, line for line.
    let ours = ours(&[step], Some(line));
    let rebuilt = laserfoundry_gcode::resume_from(&program, line).unwrap();
    let rebuilt: Vec<&str> = rebuilt.lines().filter(|l| !l.is_empty() && !l.starts_with(';')).collect();
    for (name, wire) in [("reference", &reference.wire), ("ours", &ours.wire)] {
        assert_eq!(wire.tokens.iter().filter(|t| *t == "<18>").count(), 1, "{name}");
        let at = wire.tokens.iter().position(|t| t == "<18>").unwrap();
        let second: Vec<&str> = wire.tokens[at + 1..].iter().map(String::as_str).collect();
        assert_eq!(second, rebuilt, "{name}: the second job");
        assert_eq!(wire.tokens[0], "$$");
    }
    // The line the port gives the hub is the app's own working out of it.
    let ours_line = ours.restart_line.expect("a line");
    assert!(ours_line > 1 && ours_line <= 400, "{ours_line}");
}

/// `restart_line` is LaserFoundry's: the port's number is its function of what was acknowledged.
#[test]
fn the_restart_line_is_laserfoundrys() {
    let program = job_program(400);
    assert_eq!(restart_line(&program, 100), 80);
    assert_eq!(restart_line(&program, 10), 1);
}
