//! The port's top edge (`docs/plans/agent-engine.md`, Step 3d): the engine's [`Port`] for `--mode grbl`,
//! over [`GrblMachine`] (`crates/grbl`, LaserFoundry v0.5.0). It gives LaserFoundry's `Link` its
//! orders and tells the engine what it reports; nothing goes to the machine that LaserFoundry's
//! `Link` does not send. **All of it is ours** (G1 and on), as `docs/plans/agent-engine.md` lists:
//!
//! - G1: an `Action` is the `Link` call the LaserFoundry app makes for the same control:
//!   Unlock `Link::unlock` (`control_box.rs:160`), SetOrigin `set_zero` (:163), GoOrigin
//!   `go_to_zero` (:166), Home `home` (:157), ReadSettings `request_settings`
//!   (`settings_ui.rs:60`), Jog `jog(dx, dy, feed)` (`panels.rs:813`), typed G-code
//!   `send_line` (`panels.rs:865`), one call per line.
//! - G2: FDM actions, a jog with Z, and Home's axes: LaserFoundry has one homing cycle (`$H`)
//!   and no Z jog; the others fail "LaserFoundry has no such control".
//! - G3: `Link::stop`, a soft reset, is LaserFoundry's Stop button and is a job control here.
//! - G4: `Frame { download }` gets, from the agent's link, the local path of the file it fetched
//!   (B2 fetch); the port reads the file's extent (G16) and runs `GrblMachine::frame` on it
//!   (`gcode::frame` as a job, `panels.rs:734-740`). A path that is not a readable file fails.
//! - G5: a job is `GrblMachine::start`; `from_line` is `start_from_line` (`ui-egui` :1258-1266) with
//!   `resume_from`; the line to start again from is `restart_line`, in the design's numbering
//!   (`Restart::place_in_design`).
//! - G6: pause, resume and stop are `Link::pause`, `resume`, `stop`; a refusal is the report's
//!   `problem` until the next order. An answer to a question fails the same way: LaserFoundry
//!   asks none.
//! - G7: the port reads the snapshot every 100 ms (`TICK`), as LaserFoundry's app does for a
//!   threaded link (`ui-egui/src/lib.rs:1384`): no byte goes to the machine for it (the 200 ms
//!   `?` poll is the `Link`'s).
//! - G8: telemetry is the snapshot's: the state, the machine position (not a work position), the
//!   progress (the time left is a figure for the hub's UI only, `crates/grbl` `telemetry`; it is
//!   sent nowhere else), `$30`, `$32`, `$130`-`$132`, the greeting as the firmware; "GRBL" until it
//!   greets. Nothing is derived beyond that.
//! - G9: a command is done when the machine answered every line it sent (`ok`), failed when it
//!   answered `error:N` or `ALARM:N`, when the link refused it, or when no answer can come
//!   (connection lost, a stop's soft reset, the job failed). It never counts as done without
//!   an answer, and has no time limit of its own.
//! - G10: the port does not open the machine again after it failed to open at start, as
//!   LaserFoundry does not (its Connect button is the person's); after a lost connection it
//!   does what `reconnect_tick` does (see `crates/grbl`).
//! - G11: `Gone` is a lost connection (or a machine that did not open), `Back` a reconnect.
//! - G12: a frame's program is `gcode::frame` over a `Machine` made from the controller's
//!   `$130`, `$131` and `$30` (the app's is the design's), at 3000 mm/min and 0 % power.
//! - G13: a job that was running when the connection was lost ends failed with the link's
//!   reason (the app only shows it in its status bar), with its restart line.
//! - G16: the extent of a G-code file for a frame: the X and Y of its `G0`-`G3` moves (`G90`,
//!   `G91`, `;` and `(` comments), as the hub reads a file's `bounds_mm`; LaserFoundry has none
//!   (upstream proposal 6). The frame is run in machine coordinates (`Origin::TopLeft`).
//! - G14: `--baud` defaults to LaserFoundry's `DEFAULT_BAUD` (115 200, `serial.rs:9`); `--port`
//!   is required: without it the machine does not open, loudly.
//!
//! Not covered: the app's confirmation of a start from the head's position or a user origin
//! (`lib.rs:1250`; a program file has no origin setting, the person frames and starts at the
//! machine); Marlin and Smoothieware lasers; a Z axis.

use crate::{Event, Order, Port};
use maker_connect_grbl::{Command, GrblMachine, Ticket};
use maker_connect_protocol::{Action, CommandDone, JobEnd, MachineState, Telemetry};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

/// G2, G3: the answer to a control LaserFoundry has none of.
pub const NO_SUCH_CONTROL: &str = "LaserFoundry has no such control";
/// G4
pub const NO_FRAME_FILE: &str = "LaserFoundry frames a file: this command has none";
/// G6: the problem shown when an answer arrives.
pub const NO_QUESTION: &str = "LaserFoundry asks no question to answer";
/// G7: how often the port looks at what the link has heard (`ui-egui/src/lib.rs:1384`).
pub const TICK: Duration = Duration::from_millis(100);

/// How the port gets its machine.
enum Source {
    Open { port: Option<String>, baud: Option<u32> },
    Machine(Box<GrblMachine>),
}

/// The port of a GRBL laser.
pub struct LaserFoundryPort {
    source: Source,
}

impl LaserFoundryPort {
    /// The port for `--port` and `--baud` as given (G14).
    pub fn new(port: Option<String>, baud: Option<u32>) -> Self {
        LaserFoundryPort { source: Source::Open { port, baud } }
    }

    /// The port over a machine already made (the tests', and simulators').
    pub fn with_machine(machine: GrblMachine) -> Self {
        LaserFoundryPort { source: Source::Machine(Box::new(machine)) }
    }
}

/// G1, G2, G3, G4: the `Link` calls for an action, or why there are none.
pub fn commands_for(action: &Action) -> Result<Vec<Command>, String> {
    Ok(match action {
        Action::Gcode { text } => text.lines().map(|l| Command::Line(l.to_owned())).collect(),
        Action::Jog { x, y, z, feed } => {
            if z.is_some_and(|z| z != 0.0) {
                return Err(format!("{NO_SUCH_CONTROL} (no Z jog)"));
            }
            vec![Command::Jog(
                x.unwrap_or(0.0),
                y.unwrap_or(0.0),
                feed.unwrap_or(maker_connect_grbl::JOG_FEED),
            )]
        }
        // LaserFoundry has the one homing cycle: its axes are not chosen.
        Action::Home { .. } => vec![Command::Home],
        Action::Unlock => vec![Command::Unlock],
        Action::SetOrigin => vec![Command::SetZero],
        Action::GoOrigin => vec![Command::GoToZero],
        Action::ReadSettings => vec![Command::Settings],
        Action::Frame { .. } => return Err(NO_FRAME_FILE.to_owned()),
        Action::Extrude { .. }
        | Action::Temperature { .. }
        | Action::Fan { .. }
        | Action::Speed { .. }
        | Action::Flow { .. }
        | Action::MotorsOff
        | Action::LoadFilament { .. }
        | Action::UnloadFilament { .. }
        | Action::ProbeBed
        | Action::SdList
        | Action::SdPrint { .. }
        | Action::SdDelete { .. } => return Err(NO_SUCH_CONTROL.to_owned()),
    })
}

fn done(id: u32, result: Result<Vec<String>, String>) -> Event {
    Event::Done(match result {
        Ok(output) => CommandDone { id, ok: true, reason: None, output },
        Err(reason) => CommandDone { id, ok: false, reason: Some(reason), output: Vec::new() },
    })
}

fn failed_job(id: i64, reason: String) -> Event {
    Event::JobEnd(JobEnd { id, ok: false, stopped: false, reason: Some(reason), restart_line: None })
}

impl Port for LaserFoundryPort {
    fn run(self: Box<Self>, orders: Receiver<Order>, events: Sender<Event>) {
        let mut machine = match self.source {
            Source::Machine(machine) => Some(*machine),
            Source::Open { port, baud } => match open(port, baud) {
                Ok(machine) => Some(machine),
                Err(why) => {
                    // G10, G11: reported, not retried.
                    let _ = events.send(Event::Gone);
                    let telemetry = Telemetry {
                        state: MachineState::Disconnected,
                        problem: Some(format!("Could not open the machine: {why}")),
                        ..Telemetry::default()
                    };
                    let _ = events.send(Event::Telemetry(Box::new(telemetry)));
                    None
                }
            },
        };
        let mut tickets: HashMap<Ticket, u32> = HashMap::new();
        let mut told: Option<Telemetry> = None;
        // G6: a refusal of a control, shown until the next order.
        let mut note: Option<String> = None;
        loop {
            match orders.recv_timeout(TICK) {
                Ok(order) => {
                    note = None;
                    match order {
                        // K18 / HS4 (ours): every command still pending fails, "the connection closed"
                        Order::Close => {
                            close(&tickets, &orders, &events);
                            return;
                        }
                        Order::Command { id, action } => {
                            let result = match (&mut machine, &action) {
                                (Some(machine), Action::Frame { download }) => frame_of(machine, download),
                                (None, _) => Err("Not connected".to_owned()),
                                (machine, action) => match (machine, commands_for(action)) {
                                    (_, Err(why)) => Err(why),
                                    (None, Ok(_)) => Err("Not connected".to_owned()),
                                    (Some(machine), Ok(commands)) => machine.submit(&commands),
                                },
                            };
                            match result {
                                Ok(ticket) => {
                                    tickets.insert(ticket, id);
                                }
                                Err(why) => {
                                    let _ = events.send(done(id, Err(why)));
                                }
                            }
                        }
                        Order::Job { order, file } => {
                            let started = match &mut machine {
                                None => Err("Connect to a machine (or the simulator) first".to_owned()),
                                Some(machine) => std::fs::read_to_string(&file)
                                    .map_err(|e| format!("could not read the job's file: {e}"))
                                    .and_then(|program| {
                                        machine.start(order.id, &order.name, program, order.from_line)
                                    }),
                            };
                            if let Err(why) = started {
                                let _ = events.send(failed_job(order.id, why));
                            }
                        }
                        Order::Control(control) => {
                            if let Some(why) = match &mut machine {
                                Some(machine) => machine.control(control).err(),
                                None => Some("Not connected".to_owned()),
                            } {
                                note = Some(why);
                            }
                        }
                        Order::Answer(_) => note = Some(NO_QUESTION.to_owned()),
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    close(&tickets, &orders, &events);
                    return;
                }
            }
            let Some(machine) = machine.as_mut() else { continue };
            let update = machine.report();
            let mut telemetry = update.telemetry;
            if telemetry.problem.is_none() {
                telemetry.problem = note.clone();
            }
            if told.as_ref() != Some(&telemetry) {
                told = Some(telemetry.clone());
                let _ = events.send(Event::Telemetry(Box::new(telemetry)));
            }
            if update.gone {
                let _ = events.send(Event::Gone);
            }
            if update.back {
                let _ = events.send(Event::Back);
            }
            for finished in update.done {
                if let Some(id) = tickets.remove(&finished.ticket) {
                    let _ = events.send(done(id, finished.result));
                }
            }
            if let Some(end) = update.ended {
                let _ = events.send(Event::JobEnd(end));
            }
        }
    }
}

/// K18 / HS4 (ours, engine to hub, nothing to the machine): on close every command still pending
/// (submitted and not answered, or still waiting in the orders) fails, "the connection closed".
fn close(tickets: &HashMap<Ticket, u32>, orders: &Receiver<Order>, events: &Sender<Event>) {
    let mut ids: Vec<u32> = tickets.values().copied().collect();
    ids.sort_unstable();
    for id in ids {
        crate::fail_command(id, events);
    }
    crate::fail_queued_commands(orders, events);
}

/// G14, G10: `--port` required, `--baud` as given or LaserFoundry's default.
fn open(port: Option<String>, baud: Option<u32>) -> Result<GrblMachine, String> {
    let port = port.ok_or("--mode grbl needs --port (the laser's serial port)")?;
    GrblMachine::open(&port, baud.unwrap_or(maker_connect_grbl::DEFAULT_BAUD))
}

/// G4: the frame of the file the link fetched, at `path`.
fn frame_of(machine: &mut GrblMachine, path: &str) -> Result<Ticket, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("could not read the frame's file: {e}"))?;
    let bounds = extent(std::io::BufReader::new(file)).ok_or("the file has no moves to frame")?;
    machine.frame(bounds)
}

/// G16: where a program's moves go (`x0, y0, x1, y1`, mm): the X and Y of its `G0` to `G3` moves,
/// in `G90` or `G91`.
pub fn extent(lines: impl std::io::BufRead) -> Option<[f64; 4]> {
    let mut relative = false;
    let (mut x, mut y) = (0.0f64, 0.0f64);
    let mut bounds: Option<[f64; 4]> = None;
    for line in lines.lines().map_while(Result::ok) {
        let code = line.split([';', '(']).next().unwrap_or("").trim().to_ascii_uppercase();
        let Some(first) = code.split_whitespace().next() else { continue };
        let value = |letter: char| {
            code.split_whitespace()
                .skip(1)
                .find_map(|w| w.strip_prefix(letter)?.parse::<f64>().ok())
                .filter(|v| v.is_finite())
        };
        match first {
            "G90" => relative = false,
            "G91" => relative = true,
            "G0" | "G1" | "G2" | "G3" | "G00" | "G01" | "G02" | "G03" => {
                let (nx, ny) = (value('X'), value('Y'));
                if relative {
                    x += nx.unwrap_or(0.0);
                    y += ny.unwrap_or(0.0);
                } else {
                    x = nx.unwrap_or(x);
                    y = ny.unwrap_or(y);
                }
                if nx.is_some() || ny.is_some() {
                    bounds = Some(match bounds {
                        None => [x, y, x, y],
                        Some([a, b, c, d]) => [a.min(x), b.min(y), c.max(x), d.max(y)],
                    });
                }
            }
            _ => {}
        }
    }
    bounds
}
