use super::*;
use laserfoundry_device::SimulatedGrbl;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// A machine driven the way the port drives it: `report` until what a test waits for happened.
struct Run {
    machine: GrblMachine,
    done: Vec<Done>,
    ended: Vec<JobEnd>,
    telemetry: Telemetry,
    gone: u32,
    back: u32,
}

impl Run {
    fn new(machine: GrblMachine) -> Run {
        Run {
            machine,
            done: Vec::new(),
            ended: Vec::new(),
            telemetry: Telemetry::default(),
            gone: 0,
            back: 0,
        }
    }

    fn sim(latency_ms: u64) -> Run {
        let mut sim = SimulatedGrbl::new();
        sim.set_latency(Duration::from_millis(latency_ms));
        Run::new(GrblMachine::new(Box::new(sim)))
    }

    fn step(&mut self) {
        let update = self.machine.report();
        self.telemetry = update.telemetry;
        self.done.extend(update.done);
        self.ended.extend(update.ended);
        self.gone += u32::from(update.gone);
        self.back += u32::from(update.back);
        std::thread::sleep(Duration::from_millis(5));
    }

    fn until(&mut self, what: &str, ready: impl Fn(&Run) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !ready(self) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: {:?}",
                self.telemetry
            );
            self.step();
        }
    }

    fn answer(&mut self, ticket: Ticket) -> Result<Vec<String>, String> {
        self.until("an answer", |r| r.done.iter().any(|d| d.ticket == ticket));
        self.done
            .iter()
            .find(|d| d.ticket == ticket)
            .unwrap()
            .result
            .clone()
    }

    fn send(&mut self, gcode: &str) -> Result<Vec<String>, String> {
        let ticket = self.machine.submit(&commands_of(gcode)?)?;
        self.answer(ticket)
    }
}

/// A transport that records every byte written to it.
struct Recorder<T: Transport> {
    inner: T,
    wire: Arc<Mutex<Vec<u8>>>,
}

impl<T: Transport> Transport for Recorder<T> {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.wire.lock().unwrap().extend_from_slice(bytes);
        self.inner.write_all(bytes)
    }
    fn read_line(&mut self, timeout: Duration) -> io::Result<Option<String>> {
        self.inner.read_line(timeout)
    }
}

/// A transport that goes away when told to (a cable pulled).
struct Unplug<T: Transport> {
    inner: T,
    gone: Arc<AtomicBool>,
}

impl<T: Transport> Transport for Unplug<T> {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.gone.load(Ordering::SeqCst) {
            return Err(io::Error::other("unplugged"));
        }
        self.inner.write_all(bytes)
    }
    fn read_line(&mut self, timeout: Duration) -> io::Result<Option<String>> {
        if self.gone.load(Ordering::SeqCst) {
            return Err(io::Error::other("unplugged"));
        }
        self.inner.read_line(timeout)
    }
}

fn quick() -> SimulatedGrbl {
    let mut sim = SimulatedGrbl::new();
    sim.set_latency(Duration::from_millis(1));
    sim
}

fn long_program(lines: usize) -> String {
    (0..lines)
        .map(|i| format!("G1 X{} Y{} S300 F3000", i % 50, i / 50))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A command is handed over at once and answered later, with what the machine said.
#[test]
fn a_command_is_not_waited_for() {
    let mut run = Run::sim(60);
    let started = Instant::now();
    let ticket = run.machine.submit(&[Command::Home]).unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "submit waited"
    );
    assert!(run.machine.report().done.is_empty(), "not answered yet");
    let said = run.answer(ticket).unwrap();
    assert_eq!(said, ["> $H", "ok"]);
    run.until("the head home", |r| {
        r.telemetry.position_mm == Some([Some(0.0), Some(0.0), Some(0.0)])
    });
}

/// LaserFoundry asks `$I` then `$$`, once each, only once the first status report has come.
#[test]
fn only_laserfoundrys_bytes_at_connect() {
    let wire = Arc::new(Mutex::new(Vec::new()));
    let mut run = Run::new(GrblMachine::new(Box::new(Recorder {
        inner: quick(),
        wire: wire.clone(),
    })));
    run.until("the settings", |r| r.telemetry.grbl.is_some());
    let sent = String::from_utf8(wire.lock().unwrap().clone()).unwrap();
    let status = sent.find('?').unwrap();
    let build_info = sent.find("$I\n").unwrap();
    let settings = sent.find("$$\n").unwrap();
    assert!(
        status < build_info && build_info < settings,
        "`$I` then `$$`, after the first status: {sent:?}"
    );
    assert_eq!(sent.matches("$I\n").count(), 1, "{sent:?}");
    assert_eq!(sent.matches("$$\n").count(), 1, "{sent:?}");
}

/// The answer to the `$$` the link logic asked for is not taken for a command's.
#[test]
fn the_settings_answer_is_nobodys_command() {
    let mut run = Run::sim(1);
    let ticket = run
        .machine
        .submit(&[Command::Line("G0 X5".into())])
        .unwrap();
    assert_eq!(run.answer(ticket).unwrap(), ["> G0 X5", "ok"]);
    run.until("the settings", |r| r.telemetry.grbl.is_some());
    let ticket = run
        .machine
        .submit(&[Command::Line("G0 X6".into())])
        .unwrap();
    assert_eq!(run.answer(ticket).unwrap(), ["> G0 X6", "ok"]);
}

/// A command with no answer fails; it never counts as done.
#[test]
fn a_command_without_an_answer_fails() {
    let mut sim = SimulatedGrbl::new();
    sim.go_silent();
    let mut run = Run::new(GrblMachine::new(Box::new(sim)));
    let ticket = run.machine.submit(&[Command::Home]).unwrap();
    for _ in 0..40 {
        run.step();
    }
    assert!(run.done.is_empty(), "silence is not done: {:?}", run.done);
    run.machine.control(JobControl::Stop).unwrap();
    let why = run.answer(ticket).unwrap_err();
    assert!(why.contains("no answer"), "{why}");
}

/// An error from the machine is the command's failure, in the machine's words.
#[test]
fn the_machines_error_is_the_commands_failure() {
    let mut sim = quick();
    sim.fail_on("G1");
    let mut run = Run::new(GrblMachine::new(Box::new(sim)));
    let why = run.send("G1 X1").unwrap_err();
    assert!(why.starts_with("error:20"), "{why}");
    assert!(why.ends_with("(for: G1 X1)"), "{why}");
}

/// An empty line sends nothing and is done at once.
#[test]
fn an_empty_command_is_done_at_once() {
    let mut run = Run::sim(1);
    let ticket = run.machine.submit(&[Command::Line("  ".into())]).unwrap();
    assert_eq!(run.answer(ticket).unwrap(), Vec::<String>::new());
}

/// A job streams to the end and says so; no commands meanwhile.
#[test]
fn a_job_runs_to_the_end() {
    let mut run = Run::sim(1);
    run.machine
        .start(
            7,
            "square.gcode",
            format!("G90\nM4\n{}\nM5\n", long_program(40)),
            None,
        )
        .unwrap();
    assert!(
        run.machine.start(8, "b", "G0 X1\n".into(), None).is_err(),
        "one job at a time"
    );
    assert!(
        run.machine
            .submit(&[Command::Line("G0 X0".into())])
            .is_err(),
        "no commands in a job"
    );
    run.until("the end", |r| !r.ended.is_empty());
    assert_eq!(run.ended.len(), 1);
    assert!(run.ended[0].ok && run.ended[0].id == 7, "{:?}", run.ended);
    assert_eq!(run.telemetry.job_id, None);
}

/// The example files for lasers stream to the end.
#[test]
fn the_laser_examples_stream() {
    for (id, program) in [
        (1, include_str!("../../../examples/laser/first-light.gcode")),
        (
            2,
            include_str!("../../../examples/laser/power-speed-grid.gcode"),
        ),
    ] {
        let mut sim = SimulatedGrbl::new();
        sim.set_latency(Duration::ZERO);
        let mut run = Run::new(GrblMachine::new(Box::new(sim)));
        run.machine
            .start(id, "example", program.to_owned(), None)
            .unwrap();
        run.until("the end", |r| !r.ended.is_empty());
        assert!(run.ended[0].ok, "example {id}: {:?}", run.ended);
    }
}

/// Stopping a job ends it as stopped, not failed.
#[test]
fn a_stopped_job_says_so() {
    let mut run = Run::sim(5);
    run.machine
        .start(8, "long.gcode", long_program(2000), None)
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    run.machine.control(JobControl::Stop).unwrap();
    run.until("the end", |r| !r.ended.is_empty());
    assert!(run.ended[0].stopped && !run.ended[0].ok, "{:?}", run.ended);
}

/// The job before it ended must not be taken for the end of the next one.
#[test]
fn the_job_before_is_not_this_ones_end() {
    let mut run = Run::sim(1);
    run.machine
        .start(1, "a", "G90\nG1 X1 F100\n".into(), None)
        .unwrap();
    run.until("the first end", |r| !r.ended.is_empty());
    run.machine.start(2, "b", long_program(300), None).unwrap();
    for _ in 0..4 {
        run.step();
    }
    assert_eq!(
        run.ended.len(),
        1,
        "the second job has not ended: {:?}",
        run.ended
    );
    run.until("the second end", |r| r.ended.len() == 2);
    assert_eq!(run.ended[1].id, 2);
}

/// Pause holds, resume goes on.
#[test]
fn pause_and_resume() {
    let mut run = Run::sim(3);
    run.machine.start(3, "p", long_program(300), None).unwrap();
    run.until("running", |r| r.telemetry.state == MachineState::Busy);
    run.machine.control(JobControl::Pause).unwrap();
    run.until("paused", |r| r.telemetry.state == MachineState::Paused);
    run.machine.control(JobControl::Resume).unwrap();
    run.until("the end", |r| !r.ended.is_empty());
    assert!(run.ended[0].ok, "{:?}", run.ended);
}

/// Laser mode off is refused, as the app refuses it.
#[test]
fn a_job_is_refused_when_laser_mode_is_off() {
    let mut sim = quick();
    sim.set_setting(32, "0");
    let mut run = Run::new(GrblMachine::new(Box::new(sim)));
    // `$32` itself, not the first of the settings (`$0` comes long before it).
    run.until("$32", |r| {
        r.telemetry
            .grbl
            .as_ref()
            .is_some_and(|g| g.laser_mode.is_some())
    });
    let why = run
        .machine
        .start(1, "a", "G1 X1 F100\n".into(), None)
        .unwrap_err();
    assert!(
        why.starts_with("Laser mode ($32) is off on this controller"),
        "{why}"
    );
    assert_eq!(run.telemetry.grbl.unwrap().laser_mode, Some(false));
}

/// `not_ready` says what the app says for each state.
#[test]
fn a_job_is_refused_while_the_machine_is_not_idle() {
    assert_eq!(not_ready(Grbl::Idle), None);
    assert_eq!(not_ready(Grbl::Unknown), None);
    for (state, text) in [
        (Grbl::Alarm, "alarm state"),
        (Grbl::Hold, "holding"),
        (Grbl::Door, "safety door"),
        (Grbl::Home, "homing"),
        (Grbl::Run, "still moving"),
        (Grbl::Jog, "still moving"),
        (Grbl::Check, "check mode"),
        (Grbl::Sleep, "asleep"),
    ] {
        assert!(not_ready(state).unwrap().contains(text), "{state:?}");
    }
    let mut sim = quick();
    sim.alarm_after_last_line();
    let mut run = Run::new(GrblMachine::new(Box::new(sim)));
    run.send("G0 X1").ok();
    run.until("the alarm", |r| r.telemetry.state == MachineState::Error);
    let why = run
        .machine
        .start(1, "a", "G1 X1 F100\n".into(), None)
        .unwrap_err();
    assert!(why.contains("alarm state"), "{why}");
}

/// The settings come in once connected; jogs go through the link.
#[test]
fn settings_are_read_and_jogs_go_through() {
    let mut run = Run::sim(1);
    // `$$` answers line by line: wait for the last of the three, not the first.
    run.until("the settings", |r| {
        r.telemetry.grbl.as_ref().is_some_and(|g| {
            g.laser_mode.is_some() && g.max_power.is_some() && g.travel_mm.is_some()
        })
    });
    let settings = run.telemetry.grbl.clone().unwrap();
    assert_eq!(settings.laser_mode, Some(true));
    assert_eq!(settings.max_power, Some(1000.0));
    assert_eq!(settings.travel_mm, Some([200.0, 200.0, 200.0]));
    run.send("$J=G90 G21 X20 Y30 F3000").unwrap();
    run.send("$J=G91 G21 X5 F3000").unwrap();
    run.until("the head", |r| {
        r.telemetry.position_mm == Some([Some(25.0), Some(30.0), Some(0.0)])
    });
    assert!(run.send("$J=G91 Z5 F100").is_err(), "X and Y only");
}

/// The origin set where the head is, moved away from, gone back to.
#[test]
fn the_origin_is_set_and_gone_back_to() {
    let mut run = Run::sim(1);
    run.send("$J=G90 G21 X40 Y15 F3000").unwrap();
    run.send("G92 X0 Y0").unwrap();
    run.send("$J=G91 G21 X10 Y-5 F3000").unwrap();
    run.until("moved", |r| {
        r.telemetry.position_mm == Some([Some(50.0), Some(10.0), Some(0.0)])
    });
    run.send("G90 G0 X0 Y0").unwrap();
    run.until("back", |r| {
        r.telemetry.position_mm == Some([Some(40.0), Some(15.0), Some(0.0)])
    });
}

#[test]
fn jogs_are_read() {
    assert_eq!(
        jog("$J=G91 G21 X10 F3000"),
        Some(Command::Jog(10.0, 0.0, 3000.0))
    );
    assert_eq!(
        jog("$J=G90 G21 X1 Y2 F600"),
        Some(Command::JogTo(1.0, 2.0, 600.0))
    );
    assert_eq!(jog("$J=G90 X1 F600"), None, "to a place needs X and Y");
    assert_eq!(jog("$J=G91 X1"), None, "a feed is needed");
    assert_eq!(jog("$J=G91 XNaN F600"), None);
    assert_eq!(
        commands_of("$H\n\n$X").unwrap(),
        [Command::Home, Command::Unlock]
    );
}

/// A stopped job says where to start again, in the file's lines; started there, it runs to the end.
#[test]
fn an_interrupted_job_starts_again() {
    let program: String = std::iter::once("G21\nG90\nM4 S0".to_owned())
        .chain((0..400).map(|i| format!("G1 X{} Y{} S300 F3000", i % 50, i / 50)))
        .chain(std::iter::once("M5".to_owned()))
        .collect::<Vec<_>>()
        .join("\n");
    let mut run = Run::sim(2);
    run.machine
        .start(9, "rows.gcode", program.clone(), None)
        .unwrap();
    // Stop once more than LaserFoundry's 20 lines of restart margin (`RESTART_BACKUP_LINES`)
    // are acknowledged; a fixed wait is too short on a slow runner.
    run.until("over 20 lines done", |r| {
        r.telemetry.job.as_ref().is_some_and(|j| j.done > 40)
    });
    run.machine.control(JobControl::Stop).unwrap();
    run.until("the end", |r| !r.ended.is_empty());
    let line = run.ended[0]
        .restart_line
        .expect("a line to start again from");
    assert!(line > 1 && line < 404, "{line}");

    run.machine
        .start(10, "rows.gcode", program.clone(), Some(line))
        .unwrap();
    run.until("the second end", |r| r.ended.len() == 2);
    assert!(run.ended[1].ok, "{:?}", run.ended);

    // Stopped again at once, the restart line is in the file's numbering, past where it began.
    let mut again = Run::sim(2);
    again
        .machine
        .start(11, "rows.gcode", program, Some(line))
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    again.machine.control(JobControl::Stop).unwrap();
    again.until("the end", |r| !r.ended.is_empty());
    if let Some(next) = again.ended[0].restart_line {
        assert!(next >= line, "{next} < {line}");
    }
}

/// `from_line` 1 is rebuilt by `resume_from` too, as the app's "Start here" does.
#[test]
fn a_start_from_line_one_is_rebuilt_like_any_other() {
    let wire = Arc::new(Mutex::new(Vec::new()));
    let mut run = Run::new(GrblMachine::new(Box::new(Recorder {
        inner: quick(),
        wire: wire.clone(),
    })));
    let program = "G21\nG90\nM4 S0\nG1 X5 Y5 S100 F1000\nM5\n";
    run.machine.start(1, "a", program.into(), Some(1)).unwrap();
    run.until("the end", |r| !r.ended.is_empty());
    let sent = String::from_utf8(wire.lock().unwrap().clone()).unwrap();
    let rebuilt = laserfoundry_gcode::resume_from(program, 1).unwrap();
    for line in rebuilt
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with(';'))
    {
        assert!(
            sent.contains(&format!("{line}\n")),
            "{line} not in {sent:?}"
        );
    }
}

/// A frame is `gcode::frame` run as a job; it answers its command, not a hub job.
#[test]
fn a_frame_runs_as_a_job_and_answers_its_command() {
    let mut sim = quick();
    sim.set_setting(130, "100");
    sim.set_setting(131, "80");
    let wire = Arc::new(Mutex::new(Vec::new()));
    let mut run = Run::new(GrblMachine::new(Box::new(Recorder {
        inner: sim,
        wire: wire.clone(),
    })));
    run.until("the settings", |r| r.telemetry.grbl.is_some());
    let ticket = run.machine.frame([10.0, 10.0, 30.0, 20.0]).unwrap();
    assert_eq!(run.answer(ticket).unwrap(), Vec::<String>::new());
    assert!(run.ended.is_empty(), "a frame is no job of the hub's");
    let machine = laserfoundry_doc::Machine {
        origin: laserfoundry_doc::Origin::TopLeft,
        bed_width: 100.0,
        bed_height: 80.0,
        max_s: 1000,
        ..Default::default()
    };
    let program = laserfoundry_gcode::frame(
        laserfoundry_geom::Rect::new(10.0, 10.0, 30.0, 20.0),
        &machine,
        laserfoundry_gcode::Placement::Absolute,
        FRAME_SPEED,
        FRAME_POWER,
    )
    .unwrap();
    let sent = String::from_utf8(wire.lock().unwrap().clone()).unwrap();
    for line in program
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with(';'))
    {
        assert!(
            sent.contains(&format!("{line}\n")),
            "{line} not in {sent:?}"
        );
    }
    // HS7b safety (proxy, 2026-11-17): what a frame sends never asks for power: every S word is 0,
    // and the laser is switched off (M5) at its end.
    for word in sent
        .split_whitespace()
        .filter(|w| w.starts_with('S') && w.len() > 1)
    {
        assert_eq!(
            word[1..].parse::<f64>().ok(),
            Some(0.0),
            "a frame asked for power: {word} in {sent:?}"
        );
    }
    assert!(
        sent.contains("M5\n"),
        "the frame ends with the laser off: {sent:?}"
    );
    // Outside the travel it reads: the program is not made.
    assert!(run.machine.frame([10.0, 10.0, 300.0, 20.0]).is_err());
}

/// The waits as `ui-egui/src/lib.rs:1383` makes them.
#[test]
fn the_reconnect_waits_double_up_to_thirty_seconds() {
    let waits: Vec<u64> = (1..RECONNECT_ATTEMPTS)
        .map(|a| wait_after(RECONNECT_WAIT, a).as_secs())
        .collect();
    assert_eq!(waits, [4, 8, 16, 30, 30, 30, 30]);
    assert_eq!(
        (RECONNECT_ATTEMPTS, RECONNECT_MAX_WAIT, RECONNECT_WAIT),
        (8, Duration::from_secs(30), Duration::from_secs(2))
    );
}

/// A lost connection: the job ends, the commands fail, the serial port is opened again, nothing
/// is resumed.
#[test]
fn a_lost_connection_is_reopened_and_nothing_resumes() {
    let unplugged = Arc::new(AtomicBool::new(false));
    let opened = Arc::new(AtomicUsize::new(0));
    let first = Unplug {
        inner: quick(),
        gone: unplugged.clone(),
    };
    let reopen: Reopen = {
        let (unplugged, opened) = (unplugged.clone(), opened.clone());
        Box::new(move || {
            opened.fetch_add(1, Ordering::SeqCst);
            // The cable is back after the third try.
            if opened.load(Ordering::SeqCst) < 3 {
                return Err("no such file".to_owned());
            }
            unplugged.store(false, Ordering::SeqCst);
            Ok(Box::new(quick()) as Box<dyn Transport + Send>)
        })
    };
    let mut machine = GrblMachine::with_reopen(Box::new(first), reopen);
    machine.reconnect_wait = Duration::from_millis(1);
    let mut run = Run::new(machine);
    run.until("the settings", |r| r.telemetry.grbl.is_some());
    run.machine
        .start(5, "long", long_program(2000), None)
        .unwrap();
    run.until("the job under way", |r| {
        r.telemetry.job.as_ref().is_some_and(|j| j.done > 0)
    });
    // Handed to the link during the job: refused, as a command during a job is.
    assert!(run.machine.submit(&[Command::Home]).is_err());
    unplugged.store(true, Ordering::SeqCst);
    run.until("lost", |r| r.gone == 1);
    assert_eq!(run.telemetry.state, MachineState::Disconnected);
    assert!(
        run.telemetry
            .problem
            .as_deref()
            .unwrap()
            .starts_with("The machine connection was lost; trying to reconnect."),
        "{:?}",
        run.telemetry.problem
    );
    assert!(
        run.telemetry
            .problem
            .as_deref()
            .unwrap()
            .contains("The job was stopped and will not resume by itself.")
    );
    assert_eq!(run.ended.len(), 1, "{:?}", run.ended);
    assert!(!run.ended[0].ok && run.ended[0].id == 5, "{:?}", run.ended);
    assert!(run.ended[0].restart_line.is_some_and(|l| l >= 1));
    assert!(
        run.machine.submit(&[Command::Home]).is_err(),
        "not connected"
    );
    run.until("back", |r| r.back == 1);
    assert_eq!(opened.load(Ordering::SeqCst), 3);
    assert_eq!(run.telemetry.problem, None);
    run.until("idle again", |r| {
        r.telemetry.state == MachineState::Idle && r.telemetry.grbl.is_some()
    });
    assert_eq!(run.telemetry.job_id, None, "nothing resumes");
    run.send("G0 X3").unwrap();
}

/// A command whose answer was owed when the connection went fails.
#[test]
fn a_command_owed_an_answer_fails_when_the_connection_is_lost() {
    let unplugged = Arc::new(AtomicBool::new(false));
    let mut sim = SimulatedGrbl::new();
    sim.set_latency(Duration::from_millis(300));
    let first = Unplug {
        inner: sim,
        gone: unplugged.clone(),
    };
    let mut run = Run::new(GrblMachine::new(Box::new(first)));
    let ticket = run.machine.submit(&[Command::Home]).unwrap();
    unplugged.store(true, Ordering::SeqCst);
    let why = run.answer(ticket).unwrap_err();
    assert!(why.contains("no answer"), "{why}");
    assert_eq!(
        run.telemetry.problem.as_deref(),
        Some("The machine connection was lost"),
        "a simulator is not reopened"
    );
}

/// After eight attempts it gives up, saying so.
#[test]
fn it_gives_up_after_eight_attempts() {
    let unplugged = Arc::new(AtomicBool::new(false));
    let tries = Arc::new(AtomicUsize::new(0));
    let reopen: Reopen = {
        let tries = tries.clone();
        Box::new(move || {
            tries.fetch_add(1, Ordering::SeqCst);
            Err("gone".to_owned())
        })
    };
    let mut machine = GrblMachine::with_reopen(
        Box::new(Unplug {
            inner: quick(),
            gone: unplugged.clone(),
        }),
        reopen,
    );
    machine.reconnect_wait = Duration::from_millis(1);
    let mut run = Run::new(machine);
    run.until("a status", |r| r.telemetry.position_mm.is_some());
    unplugged.store(true, Ordering::SeqCst);
    run.until("given up", |r| {
        r.telemetry
            .problem
            .as_deref()
            .is_some_and(|p| p.starts_with("Could not reconnect"))
    });
    assert_eq!(tries.load(Ordering::SeqCst), RECONNECT_ATTEMPTS as usize);
    assert_eq!(
        run.telemetry.problem.as_deref(),
        Some("Could not reconnect: gone")
    );
}

/// The console lines that are new, also when the console dropped its first ones.
#[test]
fn new_console_lines() {
    let before = vec!["< ok".to_owned()];
    let after = vec!["< ok".to_owned(), "> G0 X1".to_owned(), "< ok".to_owned()];
    assert_eq!(new_lines(&before, &after), vec!["> G0 X1", "< ok"]);
    let trimmed = vec!["> G0 X1".to_owned(), "< ok".to_owned(), "> M5".to_owned()];
    assert_eq!(
        new_lines(&after, &trimmed),
        vec!["> M5"],
        "the console dropped its first line"
    );
}

/// `auto_reconnect` off (`ui-egui/src/tests.rs:2733`): a lost connection is only said.
#[test]
fn a_lost_connection_is_not_retried_when_it_is_switched_off() {
    let unplugged = Arc::new(AtomicBool::new(false));
    let tries = Arc::new(AtomicUsize::new(0));
    let reopen: Reopen = {
        let tries = tries.clone();
        Box::new(move || {
            tries.fetch_add(1, Ordering::SeqCst);
            Err("gone".to_owned())
        })
    };
    let mut machine = GrblMachine::with_reopen(
        Box::new(Unplug {
            inner: quick(),
            gone: unplugged.clone(),
        }),
        reopen,
    );
    machine.auto_reconnect = false;
    machine.reconnect_wait = Duration::ZERO;
    let mut run = Run::new(machine);
    run.until("a status", |r| r.telemetry.position_mm.is_some());
    unplugged.store(true, Ordering::SeqCst);
    run.until("lost", |r| r.gone == 1);
    for _ in 0..20 {
        run.step();
    }
    assert_eq!(tries.load(Ordering::SeqCst), 0);
    assert_eq!(
        run.telemetry.problem.as_deref(),
        Some("The machine connection was lost")
    );
}
