//! The port's top edge: what each order becomes, and what is refused.

use super::port::*;
use crate::{Event, Order, Port};
use maker_connect_grbl::{Command, GrblMachine, SimulatedGrbl};
use maker_connect_protocol::{Action, Axis, Heater, JobControl, JobOrder, MachineState};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

struct Running {
    orders: Sender<Order>,
    events: Receiver<Event>,
    thread: Option<std::thread::JoinHandle<()>>,
    seen: Vec<Event>,
}

impl Running {
    fn new(port: LaserFoundryPort) -> Running {
        let (orders, port_orders) = channel();
        let (port_events, events) = channel();
        let port: Box<dyn Port> = Box::new(port);
        let thread = std::thread::spawn(move || port.run(port_orders, port_events));
        Running { orders, events, thread: Some(thread), seen: Vec::new() }
    }

    fn sim() -> Running {
        let mut sim = SimulatedGrbl::new();
        sim.set_latency(Duration::from_millis(1));
        Running::new(LaserFoundryPort::with_machine(GrblMachine::new(Box::new(sim))))
    }

    fn until(&mut self, what: &str, ready: impl Fn(&[Event]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            while let Ok(event) = self.events.try_recv() {
                self.seen.push(event);
            }
            if ready(&self.seen) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}: {:?}", self.seen.last());
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn command(&mut self, id: u32, action: Action) -> maker_connect_protocol::CommandDone {
        self.orders.send(Order::Command { id, action }).unwrap();
        self.until("a command done", |seen| seen.iter().any(|e| matches!(e, Event::Done(d) if d.id == id)));
        self.seen
            .iter()
            .find_map(|e| match e {
                Event::Done(d) if d.id == id => Some(d.clone()),
                _ => None,
            })
            .unwrap()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.orders.send(Order::Close);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
fn the_laser_actions_are_laserfoundrys_link_calls() {
    let to = |a: Action| commands_for(&a).unwrap();
    assert_eq!(to(Action::Unlock), [Command::Unlock]);
    assert_eq!(to(Action::SetOrigin), [Command::SetZero]);
    assert_eq!(to(Action::GoOrigin), [Command::GoToZero]);
    assert_eq!(to(Action::ReadSettings), [Command::Settings]);
    assert_eq!(to(Action::Home { axes: vec![Axis::X, Axis::Y] }), [Command::Home]);
    assert_eq!(to(Action::Home { axes: vec![] }), [Command::Home]);
    assert_eq!(
        to(Action::Jog { x: Some(10.0), y: None, z: None, feed: Some(600.0) }),
        [Command::Jog(10.0, 0.0, 600.0)]
    );
    assert_eq!(
        to(Action::Jog { x: None, y: Some(-5.0), z: None, feed: None }),
        [Command::Jog(0.0, -5.0, 3000.0)],
        "LaserFoundry's own jog feed"
    );
    assert_eq!(
        to(Action::Gcode { text: "G0 X1\n\nM5".into() }),
        [Command::Line("G0 X1".into()), Command::Line("".into()), Command::Line("M5".into())]
    );
}

#[test]
fn what_laserfoundry_has_no_control_for_fails_loudly() {
    for action in [
        Action::Extrude { mm: 5.0 },
        Action::Temperature { heater: Heater::Nozzle, celsius: 200.0 },
        Action::Fan { on: true },
        Action::Speed { percent: 100 },
        Action::Flow { percent: 100 },
        Action::MotorsOff,
        Action::LoadFilament { nozzle_c: None, bed_c: None },
        Action::UnloadFilament { nozzle_c: None, bed_c: None },
        Action::ProbeBed,
        Action::SdList,
        Action::SdPrint { file: "a".into() },
        Action::SdDelete { file: "a".into() },
        Action::Jog { x: None, y: None, z: Some(5.0), feed: None },
    ] {
        let why = commands_for(&action).unwrap_err();
        assert!(why.starts_with(NO_SUCH_CONTROL), "{action:?}: {why}");
    }
    assert_eq!(commands_for(&Action::Frame { download: "/f/1".into() }).unwrap_err(), NO_FRAME_FILE);
}

#[test]
fn a_command_is_done_with_what_the_machine_said_and_a_refused_one_fails() {
    let mut port = Running::sim();
    let done = port.command(1, Action::Home { axes: vec![] });
    assert!(done.ok && done.output == ["> $H", "ok"], "{done:?}");
    let done = port.command(2, Action::Extrude { mm: 5.0 });
    assert!(!done.ok && done.reason.as_deref() == Some(NO_SUCH_CONTROL), "{done:?}");
    let done = port.command(3, Action::Fan { on: true });
    assert!(!done.ok);
    let done = port.command(4, Action::Gcode { text: "".into() });
    assert!(done.ok && done.output.is_empty(), "{done:?}");
}

#[test]
fn a_job_runs_ends_and_is_reported() {
    let dir = std::env::temp_dir().join(format!("laserfoundry-port-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("job-1");
    std::fs::write(&file, "G90\nM4\nG1 X5 Y5 S100 F1000\nM5\n").unwrap();
    let mut port = Running::sim();
    let order = JobOrder { id: 1, name: "a.gcode".into(), download: String::new(), size: 0, from_line: None, tape_mm: None, label_length_mm: None, page_size: None };
    port.orders.send(Order::Job { order, file }).unwrap();
    port.until("the end", |seen| seen.iter().any(|e| matches!(e, Event::JobEnd(_))));
    let end = port.seen.iter().find_map(|e| if let Event::JobEnd(end) = e { Some(end.clone()) } else { None }).unwrap();
    assert!(end.ok && end.id == 1, "{end:?}");
    // A missing file ends a job failed.
    let order = JobOrder { id: 2, name: "b".into(), download: String::new(), size: 0, from_line: None, tape_mm: None, label_length_mm: None, page_size: None };
    port.orders.send(Order::Job { order, file: dir.join("missing") }).unwrap();
    port.until("the failure", |seen| seen.iter().any(|e| matches!(e, Event::JobEnd(end) if end.id == 2)));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_question_has_no_answer_here_and_it_is_said() {
    let mut port = Running::sim();
    port.orders.send(Order::Answer(0)).unwrap();
    port.until("the problem", |seen| {
        seen.iter().any(|e| matches!(e, Event::Telemetry(t) if t.problem.as_deref() == Some(NO_QUESTION)))
    });
}

#[test]
fn a_machine_that_did_not_open_is_disconnected_and_says_why() {
    // No `--port`: loud.
    let mut port = Running::new(LaserFoundryPort::new(None, None));
    port.until("the report", |seen| {
        seen.iter().any(|e| matches!(e, Event::Telemetry(t) if t.state == MachineState::Disconnected
            && t.problem.as_deref().is_some_and(|p| p.contains("needs --port"))))
    });
    assert!(port.seen.iter().any(|e| matches!(e, Event::Gone)));
    let done = port.command(1, Action::Unlock);
    assert!(!done.ok && done.reason.as_deref() == Some("Not connected"), "{done:?}");
    // A port that is not there.
    let mut port = Running::new(LaserFoundryPort::new(Some("/dev/maker-connect-test-does-not-exist".into()), None));
    port.until("the report", |seen| {
        seen.iter().any(|e| matches!(e, Event::Telemetry(t) if t.problem.as_deref().is_some_and(|p| p.starts_with("Could not open the machine"))))
    });
    let order = JobOrder { id: 1, name: "a".into(), download: String::new(), size: 0, from_line: None, tape_mm: None, label_length_mm: None, page_size: None };
    port.orders.send(Order::Job { order, file: "/nonexistent".into() }).unwrap();
    port.until("the failure", |seen| seen.iter().any(|e| matches!(e, Event::JobEnd(end) if !end.ok)));
    port.orders.send(Order::Control(JobControl::Stop)).unwrap();
}

#[test]
fn a_default_baud_is_laserfoundrys() {
    assert_eq!(maker_connect_grbl::DEFAULT_BAUD, 115_200);
}

#[test]
fn a_frame_runs_the_file_the_link_fetched() {
    let dir = std::env::temp_dir().join(format!("laserfoundry-frame-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("frame-1");
    std::fs::write(&file, "G21\nG90\nM4 S0\nG0 X10 Y10\nG1 X30 Y10 S500 F1000\nG1 X30 Y20\nM5\n").unwrap();
    let empty = dir.join("frame-2");
    std::fs::write(&empty, "G21\nM5\n").unwrap();
    let mut port = Running::sim();
    let done = port.command(1, Action::Frame { download: file.to_string_lossy().into_owned() });
    assert!(done.ok, "{done:?}");
    let done = port.command(2, Action::Frame { download: empty.to_string_lossy().into_owned() });
    assert_eq!(done.reason.as_deref(), Some("the file has no moves to frame"));
    let done = port.command(3, Action::Frame { download: dir.join("missing").to_string_lossy().into_owned() });
    assert!(!done.ok && done.reason.unwrap().starts_with("could not read the frame's file"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_extent_of_a_program_is_where_its_moves_go() {
    let p = "G90\nG0 X10 Y20 (a comment)\nG1 X30 ; more\nG91\nG1 Y-5\nM5\n";
    assert_eq!(extent(p.as_bytes()), Some([10.0, 15.0, 30.0, 20.0]));
    assert_eq!(extent("M5\n".as_bytes()), None);
}

/// K18 / HS4 (ours): a command the machine has not answered when the port closes fails, "the printer
/// connection closed", as does one that comes after the Close; none stays pending.
#[test]
fn commands_pending_at_close_fail() {
    let mut sim = SimulatedGrbl::new();
    sim.set_latency(Duration::from_secs(3));
    let (orders, port_orders) = channel();
    let (port_events, events) = channel();
    let port: Box<dyn Port> = Box::new(LaserFoundryPort::with_machine(GrblMachine::new(Box::new(sim))));
    let thread = std::thread::spawn(move || port.run(port_orders, port_events));
    let gcode = |text: &str| Action::Gcode { text: text.into() };
    orders.send(Order::Command { id: 1, action: gcode("G4 P0") }).unwrap();
    orders.send(Order::Command { id: 2, action: gcode("G4 P0") }).unwrap();
    // the port has taken the two before the Close arrives
    std::thread::sleep(Duration::from_millis(300));
    orders.send(Order::Close).unwrap();
    let _ = orders.send(Order::Command { id: 3, action: gcode("G4 P0") });
    thread.join().unwrap();
    let mut done: Vec<_> = events
        .try_iter()
        .filter_map(|e| if let Event::Done(d) = e { Some((d.id, d.ok, d.reason)) } else { None })
        .collect();
    done.sort_by_key(|d| d.0);
    let closed = Some(crate::CONNECTION_CLOSED.to_owned());
    assert_eq!(done, vec![(1, false, closed.clone()), (2, false, closed.clone()), (3, false, closed)]);
}
