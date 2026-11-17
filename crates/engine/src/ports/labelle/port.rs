//! The port's top edge (`docs/agent-engine.md`, Step 3b): the engine's [`Port`] for the DYMO
//! LabelManager PnP, over the labelle port beside it. **All of it is ours** (L1 to L9 there);
//! labelle has no such part.
//!
//! - A job is a PNG, printed as `labelle --picture <file> --tape-size-mm <tape_mm>` does
//!   ([`cli::print_picture`]); it ends done, or failed with labelle's message (L2, L3).
//! - While no job runs, every 15 s one `ESC A` is sent and its answer read (the owner's exception
//!   to labelle's nothing-while-idle, L11); a job always goes first (L8, L11).
//! - What the printer answers during a print is reported as it is, in hex, its meaning unknown
//!   (L6), with the cassette and the battery read from them as the owner decided (2026-11-16); the state is Busy during a job and Idle after it (L10).
//! - A command, a pause/resume/stop or an answer fails loudly: labelle has none (L4, L5).

use super::cli::{self, Printer};
use super::pyusb::Backend;
use crate::{Event, Order, Port};
use maker_connect_protocol::{CommandDone, JobEnd, MachineState, Telemetry};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// L11: the idle poll's interval (the owner, 2026-11-16: "every 15 s").
pub const POLL_INTERVAL: Duration = Duration::from_secs(15);

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
    poll_interval: Duration,
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
        DymoPnp { make_backend, poll_interval: POLL_INTERVAL }
    }

    /// The tests' port with a shorter poll interval.
    #[cfg(test)]
    pub fn with_poll_interval(make_backend: MakeBackend, poll_interval: Duration) -> DymoPnp {
        DymoPnp { make_backend, poll_interval }
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
    cassette: Option<bool>,
    battery_percent: Option<u8>,
}

impl Seen {
    /// L6 (the owner's decision, 2026-11-16, overruling "raw only"): the first eight bytes of an
    /// answer in hex, and what the owner saw on his PnP: the cassette is in when bit `0x40` of
    /// byte 0 is set, the battery is byte 3 when it is at most 100 (percent).
    fn heard(&mut self, answer: &[u8]) {
        if !answer.is_empty() {
            self.raw_status = Some(hex(&answer[..answer.len().min(8)]));
            self.cassette = Some(answer[0] & 0x40 != 0);
        }
        if let Some(&b) = answer.get(3)
            && b <= 100
        {
            self.battery_percent = Some(b);
        }
    }

    fn telemetry(&self, state: MachineState, problem: Option<&str>) -> Event {
        Event::Telemetry(Box::new(Telemetry {
            state,
            raw_status: self.raw_status.clone(),
            cassette: self.cassette,
            battery_percent: self.battery_percent,
            problem: problem.map(str::to_owned),
            ..Telemetry::default()
        }))
    }
}

/// L7, L11: the one Gone/Back flag, shared by the poll and the job: an event only on a change.
fn found(printer: Option<Printer>, gone: &mut bool, events: &Sender<Event>) {
    match printer {
        Some(Printer::Absent) if !*gone => {
            *gone = true;
            let _ = events.send(Event::Gone);
        }
        Some(Printer::Present) if *gone => {
            *gone = false;
            let _ = events.send(Event::Back);
        }
        _ => {}
    }
}

impl Port for DymoPnp {
    fn run(self: Box<Self>, orders: Receiver<Order>, events: Sender<Event>) {
        let mut seen = Seen::default();
        let mut gone = false;
        // L1: ready for a job from the start.
        let _ = events.send(seen.telemetry(MachineState::Idle, None));
        // L11: the first poll 15 s after start-up, then 15 s after a poll or a job ended.
        let mut next_poll = Instant::now() + self.poll_interval;
        loop {
            let order = match orders.recv_timeout(next_poll.saturating_duration_since(Instant::now())) {
                Ok(order) => order,
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    // L11: no job runs (this thread runs one thing at a time, so a job that
                    // arrives waits for the poll to release the printer). The state stays Idle.
                    let mut printer = None;
                    let result = match (self.make_backend)() {
                        Ok(backend) => {
                            let mut heard = |answer: &[u8]| {
                                seen.heard(answer);
                                let _ = events.send(seen.telemetry(MachineState::Idle, None));
                            };
                            cli::poll_status(backend, &mut heard, &mut printer)
                        }
                        Err(why) => Err(cli::CliError(why)),
                    };
                    // A failed poll other than "no printer": released, no job fails, no Gone,
                    // no retry (the next poll comes on its time).
                    let _ = result;
                    found(printer, &mut gone, &events);
                    next_poll = Instant::now() + self.poll_interval;
                    continue;
                }
            };
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
                    found(printer, &mut gone, &events);
                    let end = match result {
                        Ok(()) => JobEnd { id: order.id, ok: true, stopped: false, reason: None, restart_line: None },
                        Err(why) => JobEnd { id: order.id, ok: false, stopped: false, reason: Some(why), restart_line: None },
                    };
                    let _ = events.send(seen.telemetry(MachineState::Idle, None));
                    let _ = events.send(Event::JobEnd(end));
                    next_poll = Instant::now() + self.poll_interval;
                }
                // L5
                Order::Control(_) => {
                    let _ = events.send(seen.telemetry(MachineState::Idle, Some(NO_JOB_CONTROL)));
                }
                Order::Answer(_) => {
                    let _ = events.send(seen.telemetry(MachineState::Idle, Some(NO_QUESTION)));
                }
                // K18 / HS4 (ours): a command still waiting fails, "the printer connection closed"
                Order::Close => {
                    crate::fail_queued_commands(&orders, &events);
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(answer: &[u8]) -> (Option<String>, Option<bool>, Option<u8>) {
        let mut seen = Seen::default();
        seen.heard(answer);
        (seen.raw_status, seen.cassette, seen.battery_percent)
    }

    #[test]
    fn the_status_bits_are_read_as_the_owner_decided() {
        // 0x40 in byte 0: a cassette; byte 3 = 0x4e = 78 %
        let (raw, cassette, battery) = read(&[0x40, 0x00, 0x12, 0x4E, 0x0D, 0x6B, 0x00, 0x00]);
        assert_eq!(raw.as_deref(), Some("40 00 12 4e 0d 6b 00 00"));
        assert_eq!((cassette, battery), (Some(true), Some(78)));
        // 0x60 while printing still has bit 0x40; 100 is the highest charge
        assert_eq!(read(&[0x60, 0, 0, 100, 0, 0, 0, 0]).1, Some(true));
        assert_eq!(read(&[0x60, 0, 0, 100, 0, 0, 0, 0]).2, Some(100));
        // 0x00: no cassette; byte 3 over 100 is no charge; none is kept from before
        assert_eq!(read(&[0x00, 0, 0, 101, 0, 0, 0, 0]), (Some("00 00 00 65 00 00 00 00".to_owned()), Some(false), None));
        let mut seen = Seen::default();
        seen.heard(&[0x40, 0, 0, 50, 0, 0, 0, 0]);
        seen.heard(&[0x40, 0, 0, 200, 0, 0, 0, 0]);
        assert_eq!(seen.battery_percent, Some(50));
        // a short answer reads no battery; an empty one changes nothing
        assert_eq!(read(&[0x40]).2, None);
        assert_eq!(read(&[]), (None, None, None));
    }
}
