//! The port's top edge (`docs/archive/plans/agent-engine.md`, Step 3e): the engine's [`Port`] for the DYMO M-series
//! scale, over the usbscale port beside it. **All of it is ours** (E3, E5 to E8); the reads on and the
//! discard of old data are usbscale's `main`, ported.
//!
//! - After each open: usbscale's extra first read, thrown away, then `weigh_count = WEIGH_COUNT - 1`;
//!   a report is decoded only while `weigh_count < 1` (usbscale.c:179, 322-327): two reports are thrown
//!   away per open, the third is the first decoded ([`DISCARDED_PER_OPEN`]).
//! - Every decoded report is a [`ScaleReading`], whatever its status (E3); an event only when it changes
//!   (E7); a report usbscale refuses (E8) is shown as the problem "Error reading scale data", and the
//!   reading goes on.
//! - A read error is Gone; while gone, every 15 s the scale is looked for again (E5); found: Back.
//! - Reads wait 1 s at most, only to see [`Order::Close`] (E6). A command or a job is refused: nothing is
//!   sent to a scale.

use super::hid::{self, HidDevice, HidSource};
use super::usbscale::{self, ERROR_READING, Rejected, WEIGH_COUNT, WEIGH_REPORT_SIZE};
use crate::{Event, Order, Port};
use maker_connect_protocol::{CommandDone, JobEnd, MachineState, ScaleReading, Telemetry};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

/// E5: while the scale is gone, how often it is looked for again.
pub const FIND_INTERVAL: Duration = Duration::from_secs(15);
/// E6: a read waits this long at most, only to see `Order::Close`.
pub const READ_TIMEOUT_MS: i32 = 1000;
/// Reports thrown away after each open: the extra read, and the loop's first (`WEIGH_COUNT - 1` = 1).
pub const DISCARDED_PER_OPEN: i32 = 1 + (WEIGH_COUNT - 1);

/// What a command or a job gets: nothing is sent to a scale.
pub const NO_COMMANDS: &str = "a scale takes no commands or jobs: its weight is only read";

/// Makes the HID source, inside the port's thread (hidapi's one `HidApi` lives there).
pub type MakeSource = Box<dyn Fn() -> Result<Box<dyn HidSource>, String> + Send>;

/// The port of the DYMO M-series scale.
pub struct UsbScale {
    make_source: MakeSource,
    port: Option<String>,
    find_interval: Duration,
    read_timeout_ms: i32,
}

impl UsbScale {
    /// The port over the OS's HID (hidapi); `--port` as given, or the one device with usage page 0x8D.
    pub fn new(port: Option<String>) -> UsbScale {
        UsbScale::with_source(Box::new(hid::system_source), port)
    }

    /// The port over another source (the tests').
    pub fn with_source(make_source: MakeSource, port: Option<String>) -> UsbScale {
        UsbScale {
            make_source,
            port,
            find_interval: FIND_INTERVAL,
            read_timeout_ms: READ_TIMEOUT_MS,
        }
    }

    /// The tests' port with shorter times.
    #[cfg(test)]
    pub fn with_times(
        make_source: MakeSource,
        port: Option<String>,
        find_interval: Duration,
        read_timeout_ms: i32,
    ) -> UsbScale {
        UsbScale {
            make_source,
            port,
            find_interval,
            read_timeout_ms,
        }
    }
}

/// E2, at the agent's start: the device to read, or why not (the agent refuses to start).
pub fn check(port: Option<&str>) -> Result<(), String> {
    hid::select(hid::system_source()?.as_mut(), port).map(|_| ())
}

fn telemetry(reading: Option<&ScaleReading>, problem: Option<&str>) -> Event {
    Event::Telemetry(Box::new(Telemetry {
        state: MachineState::Idle,
        scale: reading.cloned(),
        problem: problem.map(str::to_owned),
        ..Telemetry::default()
    }))
}

/// An order to a scale: a command or a job is refused; true when it is `Close`.
fn handle(order: Order, events: &Sender<Event>) -> bool {
    match order {
        Order::Close => return true,
        Order::Command { id, .. } => {
            let _ = events.send(Event::Done(CommandDone {
                id,
                ok: false,
                reason: Some(NO_COMMANDS.to_owned()),
                output: Vec::new(),
            }));
        }
        Order::Job { order, .. } | Order::SdJob { order, .. } => {
            let _ = events.send(Event::JobEnd(JobEnd {
                id: order.id,
                ok: false,
                stopped: false,
                reason: Some(NO_COMMANDS.to_owned()),
                restart_line: None,
            }));
        }
        Order::Control(_) | Order::Answer(_) => {}
    }
    false
}

/// How a wait for a report ended.
enum Next {
    Report(Vec<u8>),
    Close,
    Error,
}

/// One report: `libusb_interrupt_transfer`'s place. A read that times out only looks at the orders (E6).
fn next_report(
    device: &mut dyn HidDevice,
    timeout_ms: i32,
    orders: &Receiver<Order>,
    events: &Sender<Event>,
) -> Next {
    // hidapi returns the report as it came, report ID first (E8)
    let mut buf = [0u8; 64];
    loop {
        match device.read(&mut buf, timeout_ms) {
            Ok(0) => loop {
                match orders.try_recv() {
                    Ok(order) => {
                        if handle(order, events) {
                            return Next::Close;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Next::Close,
                }
            },
            Ok(n) => return Next::Report(buf[..n].to_vec()),
            Err(_) => return Next::Error,
        }
    }
}

/// usbscale's `main` after the open: "we get old data the first time, so let's just get that out of the
/// way now", then the loop, `weigh_count` and all. True: closed; false: the device failed.
fn read_loop(
    device: &mut dyn HidDevice,
    timeout_ms: i32,
    orders: &Receiver<Order>,
    events: &Sender<Event>,
) -> bool {
    let mut weigh_count = WEIGH_COUNT - 1;
    let mut shown: Option<ScaleReading> = None;
    let mut problem = false;
    // the extra read
    match next_report(device, timeout_ms, orders, events) {
        Next::Report(_) => {}
        Next::Close => return true,
        Next::Error => return false,
    }
    loop {
        let dat = match next_report(device, timeout_ms, orders, events) {
            Next::Report(dat) => dat,
            Next::Close => return true,
            Next::Error => return false,
        };
        if weigh_count < 1 {
            // E7: an event only when what is shown changes
            match usbscale::print_scale_data(&dat) {
                Ok(reading) => {
                    if shown.as_ref() != Some(&reading) || problem {
                        let _ = events.send(telemetry(Some(&reading), None));
                    }
                    shown = Some(reading);
                    problem = false;
                }
                // E8: usbscale returns -1 and ends; here the reading goes on and says so, once
                Err(Rejected::NotAReport(_) | Rejected::Short(_)) => {
                    if !problem {
                        let _ = events.send(telemetry(shown.as_ref(), Some(ERROR_READING)));
                    }
                    problem = true;
                }
            }
        }
        weigh_count -= 1;
    }
}

impl Port for UsbScale {
    fn run(self: Box<Self>, orders: Receiver<Order>, events: Sender<Event>) {
        let mut source: Option<Box<dyn HidSource>> = None;
        let mut gone = false;
        loop {
            // find and open (E2, E5): nothing is sent to the scale
            let opened = (|| {
                if source.is_none() {
                    source = Some((self.make_source)()?);
                }
                let source = source.as_mut().expect("made above");
                let path = hid::select(source.as_mut(), self.port.as_deref())?;
                source.open(&path)
            })();
            match opened {
                Ok(mut device) => {
                    // (after a read error `gone` is always set, and so is Gone sent)
                    if gone {
                        let _ = events.send(Event::Back);
                    }
                    let _ = events.send(telemetry(None, None));
                    if read_loop(device.as_mut(), self.read_timeout_ms, &orders, &events) {
                        return;
                    }
                    // a read error (E7): the scale is gone; the device is let go
                    drop(device);
                    gone = true;
                    let _ = events.send(Event::Gone);
                }
                Err(_) if !gone => {
                    gone = true;
                    let _ = events.send(Event::Gone);
                }
                Err(_) => {}
            }
            // E5: again in 15 s, orders meanwhile
            let next = Instant::now() + self.find_interval;
            loop {
                match orders.recv_timeout(next.saturating_duration_since(Instant::now())) {
                    Ok(order) => {
                        if handle(order, &events) {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        }
    }
}

const _: () = assert!(WEIGH_REPORT_SIZE <= 64);
