//! The port's top edge (`docs/agent-engine.md`, Step 3b): the engine's [`Port`] for the DYMO
//! LabelManager PnP, over the labelle port beside it. **All of it is ours** (L1 to L9 there);
//! labelle has no such part.
//!
//! - A job is a PNG, printed as `labelle --picture <file> --tape-size-mm <tape_mm>` does
//!   ([`cli::print_picture`]); it ends done, or failed with labelle's message (L2, L3).
//! - Nothing is sent while idle, and the printer is looked for when a job prints (L7, L8).
//! - What the printer answers during a print is reported as it is, in hex, its meaning unknown
//!   (L6); the state is Busy during a job and Idle after it (L10).
//! - A command, a pause/resume/stop or an answer fails loudly: labelle has none (L4, L5).

use super::cli::{self, Printer};
use super::pyusb::Backend;
use crate::{Event, Order, Port};
use maker_connect_protocol::{CommandDone, JobEnd, MachineState, Telemetry};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender};

/// L4: the answer to a command.
pub const NO_SUCH_CONTROL: &str = "labelle has no such control";
/// L5: the problem shown when a pause, resume or stop arrives (a print runs in one go).
pub const NO_JOB_CONTROL: &str = "labelle has no pause, resume or stop: a label prints in one go";
/// L5: the problem shown when an answer arrives.
pub const NO_QUESTION: &str = "labelle asks no question to answer";

/// Makes the USB backend of a print (the system's USB devices as they are then).
pub type MakeBackend = Box<dyn Fn() -> Result<Rc<dyn Backend>, String> + Send>;

/// The port of the DYMO LabelManager PnP.
pub struct DymoPnp {
    make_backend: MakeBackend,
}

impl DymoPnp {
    /// The port over nusb (the system's USB).
    pub fn new() -> DymoPnp {
        DymoPnp::with_backend(Box::new(|| {
            let backend = super::nusb_backend::NusbBackend::new().map_err(|e| e.to_string())?;
            Ok(Rc::new(backend) as Rc<dyn Backend>)
        }))
    }

    /// The port over another backend (the tests').
    pub fn with_backend(make_backend: MakeBackend) -> DymoPnp {
        DymoPnp { make_backend }
    }
}

impl Default for DymoPnp {
    fn default() -> Self {
        DymoPnp::new()
    }
}

/// `40 00 12 64`: bytes as hex.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

/// What the port tells of the printer between prints (L6): its last answer, in hex.
#[derive(Default, Clone)]
struct Seen {
    raw_status: Option<String>,
}

impl Seen {
    /// L6: the first eight bytes of an answer in hex; what they mean is unknown, nothing is
    /// read from them.
    fn heard(&mut self, answer: &[u8]) {
        if !answer.is_empty() {
            self.raw_status = Some(hex(&answer[..answer.len().min(8)]));
        }
    }

    fn telemetry(&self, state: MachineState, problem: Option<&str>) -> Event {
        Event::Telemetry(Box::new(Telemetry {
            state,
            raw_status: self.raw_status.clone(),
            problem: problem.map(str::to_owned),
            ..Telemetry::default()
        }))
    }
}

impl Port for DymoPnp {
    fn run(self: Box<Self>, orders: Receiver<Order>, events: Sender<Event>) {
        let mut seen = Seen::default();
        let mut gone = false;
        // L1: ready for a job from the start; the printer is looked for when one prints.
        let _ = events.send(seen.telemetry(MachineState::Idle, None));
        for order in orders {
            match order {
                // L4
                Order::Command { id, .. } => {
                    let _ = events.send(Event::Done(CommandDone {
                        id,
                        ok: false,
                        reason: Some(NO_SUCH_CONTROL.to_owned()),
                        output: Vec::new(),
                    }));
                }
                Order::Job { order, file } => {
                    let _ = events.send(seen.telemetry(MachineState::Busy, None));
                    // L2: `--tape-size-mm` from the order, never from the picture, never defaulted.
                    let tape = order.tape_mm.map(i64::from);
                    let mut printer = None;
                    let result = match (self.make_backend)() {
                        Ok(backend) => {
                            let mut heard = |answer: &[u8]| {
                                seen.heard(answer);
                                let _ = events.send(seen.telemetry(MachineState::Busy, None));
                            };
                            cli::print_picture(backend, &file.to_string_lossy(), tape, &mut heard, &mut printer)
                                .map_err(|e| e.to_string())
                        }
                        Err(why) => Err(why),
                    };
                    // L7: the printer is there, or not, as this print found it.
                    match printer {
                        Some(Printer::Absent) if !gone => {
                            gone = true;
                            let _ = events.send(Event::Gone);
                        }
                        Some(Printer::Present) if gone => {
                            gone = false;
                            let _ = events.send(Event::Back);
                        }
                        _ => {}
                    }
                    let end = match result {
                        Ok(()) => JobEnd { id: order.id, ok: true, stopped: false, reason: None, restart_line: None },
                        Err(why) => JobEnd { id: order.id, ok: false, stopped: false, reason: Some(why), restart_line: None },
                    };
                    let _ = events.send(seen.telemetry(MachineState::Idle, None));
                    let _ = events.send(Event::JobEnd(end));
                }
                // L5
                Order::Control(_) => {
                    let _ = events.send(seen.telemetry(MachineState::Idle, Some(NO_JOB_CONTROL)));
                }
                Order::Answer(_) => {
                    let _ = events.send(seen.telemetry(MachineState::Idle, Some(NO_QUESTION)));
                }
                Order::Close => return,
            }
        }
    }
}
