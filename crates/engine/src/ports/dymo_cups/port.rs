//! The port's top edge (`docs/plans/agent-engine.md`, Step 3c): the engine's [`Port`] for the DYMO LabelWriter 450
//! over the driver beside it. **All of it is ours** (W1 to W10 there); the driver has no such part.
//!
//! - A job is a PNG, made into the page CUPS would hand the filter ([`page`]) and printed by
//!   `raster2dymolw`'s run ([`raster2dymolw::main`]), whose output goes to the `usb` backend
//!   ([`cups::usb_backend`]); it ends done, or failed with the driver's own error line (W1).
//! - Nothing is sent while idle. The printer is looked for when a job prints and waited for as the backend
//!   does (every 5 s, for ever): `Gone` when it is not there, `Back` when it is (W5, W8).
//! - A Stop during a print is CUPS's cancel: the filter is terminated (W6).
//! - What the monitor reads during a print is reported as it is (W7); nothing else.
//! - A command, a pause/resume/stop (outside a print) or an answer fails loudly: the driver has none (W6).

use super::cups::usb_backend::{print_device, BackChannel, Pipe, UsbHost, CUPS_BACKEND_OK};
use super::cups::ppd::PpdFile;
use super::cups_print_environment::{Cancelled, FilterIo, SharedIo, READ_DATA_LINE};
use super::{cups::ppd_lw450::REASONS, page, raster2dymolw, stderr};
use crate::{Event, Order, Port};
use maker_connect_protocol::{CommandDone, JobControl, JobEnd, JobOrder, MachineState, Telemetry};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// W6: the answer to a command.
pub const NO_SUCH_CONTROL: &str = "the DYMO driver has no such control";
/// W6: the problem shown when a pause, resume or stop arrives and no print runs.
pub const NO_JOB_CONTROL: &str = "the DYMO driver has no pause, resume or stop outside a print";
/// W6: the problem shown when an answer arrives.
pub const NO_QUESTION: &str = "the DYMO driver asks no question to answer";

/// W4: `DEVICE_URI`.
pub const DEVICE_URI: &str = "usb://DYMO/LabelWriter%20450";
/// W4: the manufacturer and model of that URI, as `make_device_uri` makes them from the printer's IEEE-1284
/// device ID (`MFG:DYMO;MDL:LabelWriter 450`).
pub const MANUFACTURER: &str = "DYMO";
pub const MODEL: &str = "LabelWriter 450";

/// The port of the DYMO LabelWriter 450.
pub struct LabelWriter450 {
    host: Arc<dyn UsbHost>,
}

impl LabelWriter450 {
    /// The port over nusb (the system's USB).
    #[cfg(not(test))]
    pub fn new() -> LabelWriter450 {
        LabelWriter450::with_host(Arc::new(super::cups::usb_backend::NusbHost))
    }

    /// The port over another host of USB printers (the tests').
    pub fn with_host(host: Arc<dyn UsbHost>) -> LabelWriter450 {
        LabelWriter450 { host }
    }
}

#[cfg(not(test))]
impl Default for LabelWriter450 {
    fn default() -> Self {
        LabelWriter450::new()
    }
}

/// What the port tells of the printer: the status byte the monitor read last (W7), in hex.
#[derive(Default, Clone)]
struct Seen {
    raw_status: Option<String>,
}

impl Seen {
    fn telemetry(&self, state: MachineState, problem: Option<&str>) -> Event {
        Event::Telemetry(Box::new(Telemetry {
            state,
            raw_status: self.raw_status.clone(),
            problem: problem.map(str::to_owned),
            ..Telemetry::default()
        }))
    }
}

/// W7: the PPD's own reason text (`lw450.ppd:47-55`) for a `STATE:` keyword.
fn reason_text(keyword: &str) -> Option<&'static str> {
    REASONS.iter().find(|(k, _, _)| *k == keyword).map(|(_, t, _)| *t)
}

/// The filter's process, as the scheduler runs it (ours): `write(1, ...)` is the backend's pipe, fd 3 the
/// back channel, the environment is CUPS's.
struct PortIo {
    pipe: Arc<Pipe>,
    /// Ours (W6): set when a Stop comes, so that a backend still waiting for the printer ends.
    backend_stop: Arc<AtomicBool>,
    back_channel: Arc<BackChannel>,
    orders: Rc<Receiver<Order>>,
    /// Orders that came during the print (W6): they wait for its end.
    pending: Rc<RefCell<VecDeque<Order>>>,
    stopped: bool,
    closed: bool,
}

impl PortIo {
    /// W6: Stop during a print ends it (SIGTERM); the other orders wait.
    fn look_for_orders(&mut self) {
        // K18 / HS4 (ours): after a Close nothing more is read; the rest stays for the port's Close
        while !self.closed && let Ok(order) = self.orders.try_recv() {
            match order {
                Order::Control(JobControl::Stop) => self.stopped = true,
                Order::Close => {
                    self.closed = true;
                    self.stopped = true;
                    // K18 / HS4 (ours): what comes after the Close stays in the channel; the port's
                    // Close fails the commands in it
                    break;
                }
                other => self.pending.borrow_mut().push_back(other),
            }
        }
        if self.stopped {
            self.backend_stop.store(true, Ordering::SeqCst);
        }
    }
}

impl FilterIo for PortIo {
    fn write_stdout(&mut self, data: &[u8]) -> Result<(), i32> {
        let pipe = self.pipe.clone();
        let mut cancelled = || self.cancelled();
        pipe.write(data, &mut cancelled)
    }

    fn back_channel_read(&mut self, buffer: &mut [u8], timeout: f64) -> isize {
        self.back_channel.read(buffer, timeout)
    }

    fn getenv(&self, name: &str) -> Option<String> {
        match name {
            "DEVICE_URI" => Some(DEVICE_URI.to_owned()),
            _ => None,
        }
    }

    fn ppd_open_file(&self) -> Option<PpdFile> {
        Some(PpdFile::lw450())
    }

    fn cancelled(&mut self) -> bool {
        self.look_for_orders();
        self.stopped
    }
}

/// How a print ended.
enum Outcome {
    Done,
    Failed(String),
    Stopped,
}

/// The last `ERROR:` line of the filter, without its tag (W1, W9).
fn last_error(lines: &[String]) -> Option<String> {
    lines.iter().rev().find_map(|l| l.strip_prefix("ERROR: ").map(|e| e.trim_end().to_owned()))
}

/// The first `ERROR:` line, without its tag: the backend's, which comes before the filter's own errors
/// about the pipe it can no longer write to (W1, W9).
fn first_error(lines: &[String]) -> Option<String> {
    lines.iter().find_map(|l| l.strip_prefix("ERROR: ").map(|e| e.trim_end().to_owned()))
}

impl Port for LabelWriter450 {
    fn run(self: Box<Self>, orders: Receiver<Order>, events: Sender<Event>) {
        let orders = Rc::new(orders);
        let pending: Rc<RefCell<VecDeque<Order>>> = Rc::new(RefCell::new(VecDeque::new()));
        let mut seen = Seen::default();
        let mut gone = false;
        // ready for a job from the start; the printer is looked for when one prints (nothing is sent idle)
        let _ = events.send(seen.telemetry(MachineState::Idle, None));

        loop {
            let order = match pending.borrow_mut().pop_front() {
                Some(o) => o,
                None => match orders.recv() {
                    Ok(o) => o,
                    Err(_) => return,
                },
            };
            match order {
                // W6
                Order::Command { id, .. } => {
                    let _ = events.send(Event::Done(CommandDone { id, ok: false, reason: Some(NO_SUCH_CONTROL.to_owned()), output: Vec::new() }));
                }
                Order::Job { order, file } => {
                    let _ = events.send(seen.telemetry(MachineState::Busy, None));
                    let (outcome, raw) = print_job(&self.host, &order, &file, &orders, &pending, &events, &mut gone, &seen);
                    if raw.is_some() {
                        seen.raw_status = raw;
                    }
                    let end = match outcome {
                        Outcome::Done => JobEnd { id: order.id, ok: true, stopped: false, reason: None, restart_line: None },
                        Outcome::Stopped => JobEnd { id: order.id, ok: false, stopped: true, reason: Some("stopped".to_owned()), restart_line: None },
                        Outcome::Failed(why) => JobEnd { id: order.id, ok: false, stopped: false, reason: Some(why), restart_line: None },
                    };
                    let _ = events.send(seen.telemetry(MachineState::Idle, None));
                    let _ = events.send(Event::JobEnd(end));
                }
                // W6
                Order::Control(_) => {
                    let _ = events.send(seen.telemetry(MachineState::Idle, Some(NO_JOB_CONTROL)));
                }
                Order::Answer(_) => {
                    let _ = events.send(seen.telemetry(MachineState::Idle, Some(NO_QUESTION)));
                }
                // K18 / HS4 (ours): a command still waiting fails, "the connection closed"
                Order::Close => {
                    for order in pending.borrow_mut().drain(..) {
                        if let Order::Command { id, .. } = order {
                            crate::fail_command(id, &events);
                        }
                    }
                    crate::fail_queued_commands(&orders, &events);
                    return;
                }
            }
        }
    }
}

/// W1: one job: the page, then the backend (`print_device`, on a thread) and the filter together as CUPS
/// starts them (W5), the filter's run, the backend's end.
#[allow(clippy::too_many_arguments)]
fn print_job(
    host: &Arc<dyn UsbHost>,
    order: &JobOrder,
    file: &std::path::Path,
    orders: &Rc<Receiver<Order>>,
    pending: &Rc<RefCell<VecDeque<Order>>>,
    events: &Sender<Event>,
    gone: &mut bool,
    seen: &Seen,
) -> (Outcome, Option<String>) {
    // W2, W3: the page
    let picture = match page::decode_png(file) {
        Ok(p) => p,
        Err(why) => return (Outcome::Failed(why), None),
    };
    let ppd_open = || Some(PpdFile::lw450());
    let raster = match page::make_raster(&picture, order.page_size.as_deref(), order.label_length_mm, "", &ppd_open) {
        Ok(r) => r,
        Err(why) => return (Outcome::Failed(why), None),
    };

    let back_channel = BackChannel::new();
    let pipe = Pipe::new();
    let backend_stop = Arc::new(AtomicBool::new(false));
    let io = Rc::new(RefCell::new(PortIo {
        pipe: pipe.clone(),
        backend_stop: backend_stop.clone(),
        back_channel: back_channel.clone(),
        orders: orders.clone(),
        pending: pending.clone(),
        stopped: false,
        closed: false,
    }));

    // W5, W8: the backend, `print_device`, starts with the filter: it finds the printer, waiting for it
    let is_gone = Arc::new(AtomicBool::new(*gone));
    let backend = {
        let (host, pipe, bc, stop, ev, is_gone) = (host.clone(), pipe.clone(), back_channel.clone(), backend_stop.clone(), events.clone(), is_gone.clone());
        std::thread::spawn(move || {
            stderr::start(None);
            let (ev2, gone2) = (ev.clone(), is_gone.clone());
            let status = print_device(
                &*host,
                MANUFACTURER,
                MODEL,
                &pipe,
                bc,
                &stop,
                &mut || {
                    if !is_gone.swap(true, Ordering::SeqCst) {
                        let _ = ev.send(Event::Gone);
                    }
                },
                &mut || {
                    if gone2.swap(false, Ordering::SeqCst) {
                        let _ = ev2.send(Event::Back);
                    }
                },
            );
            (status, stderr::take())
        })
    };

    // W7, W9: the filter's stderr, as it is written
    let raw: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let tel_events = events.clone();
    let tel_raw = raw.clone();
    let tel_seen = seen.clone();
    stderr::start(Some(Box::new(move |line: &str| {
        if let Some(rest) = line.strip_prefix("STATE: ") {
            let kw = rest.trim_end();
            let problem = if kw == "none" { None } else { reason_text(kw) };
            let mut s = tel_seen.clone();
            s.raw_status = tel_raw.borrow().clone();
            let _ = tel_events.send(s.telemetry(MachineState::Busy, problem));
        } else if let Some(rest) = line.strip_prefix(READ_DATA_LINE) {
            // "<n> bytes <first byte in hex>"
            if let Some(hex) = rest.trim_end().rsplit(' ').next() {
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    let v = format!("{b:02x}");
                    *tel_raw.borrow_mut() = Some(v.clone());
                    let mut s = tel_seen.clone();
                    s.raw_status = Some(v);
                    let _ = tel_events.send(s.telemetry(MachineState::Busy, None));
                }
            }
        }
    })));

    // W4: argv as the scheduler gives it
    let argv: Vec<String> = vec![
        "raster2dymolw".to_owned(),
        order.id.to_string(),
        "maker-connect".to_owned(),
        order.name.clone(),
        "1".to_owned(),
        String::new(),
    ];
    let shared: SharedIo = io.clone();
    let mut filter = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| raster2dymolw::main(shared, &argv, raster, &mut filter)));
    let cancelled = match &result {
        Err(payload) => {
            if payload.downcast_ref::<Cancelled>().is_none() {
                // not ours: a bug of the port
                std::panic::resume_unwind(Box::new("the DYMO filter panicked"));
            }
            true
        }
        Ok(_) => false,
    };
    let lines = stderr::take();
    let raw_out = raw.borrow().clone();

    // the filter has ended: the backend gets the end of its input (W6: "as at end of input", where CUPS also
    // sends it SIGTERM, which it ignores once it prints and which ends it while it still waits). A Stop
    // that comes meanwhile ends the wait.
    pipe.close_writer();
    if cancelled {
        backend_stop.store(true, Ordering::SeqCst);
    }
    while !backend.is_finished() {
        io.borrow_mut().cancelled();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let (backend_status, backend_lines) = backend.join().unwrap();
    *gone = is_gone.load(Ordering::SeqCst);

    if io.borrow().closed {
        pending.borrow_mut().push_back(Order::Close);
    }

    let outcome = if cancelled || io.borrow().stopped || backend_status.is_none() {
        Outcome::Stopped
    } else if backend_status != Some(CUPS_BACKEND_OK) {
        Outcome::Failed(first_error(&backend_lines).unwrap_or_else(|| "Unable to send data to printer.".to_owned()))
    } else if let Ok(status) = result {
        if status != 0 {
            Outcome::Failed(last_error(&lines).unwrap_or_else(|| format!("raster2dymolw failed ({status})")))
        } else {
            Outcome::Done
        }
    } else {
        Outcome::Stopped
    };
    (outcome, raw_out)
}
