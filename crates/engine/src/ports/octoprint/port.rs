//! The top edge of the Marlin port (`--mode marlin`): the [`Port`] that gives OctoPrint's ported code
//! (`standard.rs`, `comm.rs`, ...) its orders and tells the engine what it reports. OURS, every piece
//! of it, as `docs/agent-engine.md` decided it (O1-O8, S1-S5, M1-M5, B1-B9, Sections G-J); what it
//! decided nothing about is listed below as K-items, "to approve (proxy)". Nothing of ours reaches the
//! printer: each byte comes from the ported OctoPrint code.
//!
//! # What it does
//!
//! * `run`: fires `Events::STARTUP`, registers the prompt plugin, creates the `Printer` with the default
//!   profile, installs the event sink (`events.rs`) and the top edge's listener (`Printer::set_top_edge`),
//!   then `connect(port, baudrate)` as the agent's options give them (`None`: OctoPrint's own detection).
//!   On `Order::Close`: `disconnect`, then `Events::SHUTDOWN`.
//! * M1: each `Action` is the OctoPrint call M1 lists, after the refusals of the API endpoint that
//!   makes it (`api_printer.rs`, S5/B3); a refusal is a failed `CommandDone` with OctoPrint's text.
//! * M2: what OctoPrint has no control for is a failed `CommandDone`, "OctoPrint has no such control".
//! * M3: the state table; `getStateString` as `problem` where it is not plain; the prompt plugin's open
//!   prompt is Attention with its text and buttons.
//! * M4: a `CommandDone` once the command's lines are sent and `ok`-ed, the lines received meanwhile as
//!   its output: from the tags (`maker-connect:<id>`) the lines carry through comm's `gcode.queuing` and
//!   `gcode.sent` hooks, and from comm's own terminal log (`on_comm_log`), not a reader of ours.
//! * Telemetry: from the `MachineComPrintCallback` calls comm already makes. No polling of our own.
//! * Jobs: `select_file(file, sd=false)` then `start_print`, without a position (`job.py:38,45`).
//!
//! # Ours, to approve (proxy): the K-items
//!
//! * K1: typed G-code goes as typed: the terminal page's upper-casing (`terminal.js:425-442`) is the web
//!   page's JavaScript, not ported. The text is split at `\n` into the lines a user would enter one by one
//!   (`terminal.js:419-452`): a line that is the empty string sends nothing (`if (!command) return`,
//!   `terminal.js:421`), the others, not trimmed, go to `commands` in one call (a trailing `\r` is dropped:
//!   the terminal's one-line input never carries one).
//! * K2: `job.py`'s and `files.py`'s refusals, ported into `api_printer.rs` (extends S5).
//! * K3: a `JobOrder` with `from_line` gets a failed `JobEnd`, "OctoPrint has no such control".
//! * K4: `Gone` is comm's state going to `STATE_CLOSED` or `STATE_CLOSED_WITH_ERROR` (the connection is
//!   over: unplugged, an error, or a disconnect), once; `Back` is comm's state `STATE_OPERATIONAL` after
//!   that. OctoPrint itself connects again only when it is asked to (its `connect`), so `Back` happens
//!   only if something connects again: this port does not.
//! * K6: OctoPrint selects a file by a name with a machinecode extension (`valid_file_type`), and the
//!   agent keeps a job's file as `job-<id>`. The port gives OctoPrint the file under the job's name, a
//!   hard link (a copy where that fails) in `job-<id>.d/` beside it, removed when the job ends.
//! * K7: SdPrint is `select_file(sd=True)` with `printAfterSelect` (the way the files API prints from the
//!   card: the card's answer to the selection starts the print), not a `start_print` straight after.
//! * K8: a refusal of a `Control` or an `Answer` (they have no `CommandDone`) is the report's `problem`
//!   with OctoPrint's text, until the printer's state changes or the next order comes.
//! * K9: `problem` is `getStateString` unless it is `Offline`, `Operational`, `Printing` or `Paused`
//!   (the plain text of Disconnected, Idle, Busy and Paused); an Error's text stays.
//! * K10: progress `done`/`total` are the file position and the file's size in bytes, `elapsed_s` is
//!   `getPrintTime`; no time left (OctoPrint's print time estimators are not ported). The position is
//!   the last `PositionUpdate` with x, y and z. The tools are listed when there is more than one.
//! * K11: a `CommandDone` of a command that sent no line is `ok` at once; of SdList it comes with the
//!   card's file list (one line per file: its name), at once when no card is ready. A command whose
//!   lines are never sent or never `ok`-ed (an `M112`) is given up when comm closes or on `Order::Close`
//!   (K18), or by the hub's agent-gone rule (K18).
//! * K18: ours, engine and hub, nothing on the wire to the device: on `Gone` and on `Order::Close` the port
//!   sends `Done { ok: false }, "the printer connection closed"` for every command still pending.
//!   K18-1 (comm's own lines after an emergency stop credit no pending command): `_trigger_emergency_stop`
//!   (comm.py:5487-5522) sends `M112`, `M104 T0 S0`, `M140 S0` itself through `_do_send_without_checksum`
//!   (:5017, :5036-5041): `Send:` in the log, no tags, no `gcode.queuing`/`sent` hook. `emergency` is set by a
//!   `gcode.queuing` hook with gcode `M112` (comm's `_gcode_M112_queuing` follows it) or by `on_comm_error` with
//!   consequence `emergency` (comm.py:4110-4136), and until `Gone`/`Back` the received lines credit nothing: an
//!   `M109` pending at an `M112` fails with the close, never `ok` (`the_top_edge_on_a_fake_printer`).
//! * K12: `start_print` that starts nothing (OctoPrint answers 204 either way) ends the job as failed,
//!   "OctoPrint did not start the print".
//! * K13: `M4`'s output is every line received from the `Send:` of the command's line to the `ok` that
//!   answers it, the `ok` line included (it can carry the answer, as `ok T:21.0 /0.0`).

use super::action_command_prompt::ActionCommandPromptPlugin;
use super::api_printer::{self, ApiError, PrinterCommand};
use super::comm::{MachineCom, MachineComPrintCallback, TempTuple};
use super::events::{event_manager, EventSink, Events};
use super::plugin::{plugin_manager, HookFn, MessageReceiver};
use super::profile::PrinterProfileManager;
use super::standard::{LocalFileManager, Printer, PrinterError};
use crate::{Event, Order, Port};
use maker_connect_protocol::{
    Action, Axis, CommandDone, Heater, JobControl, JobEnd, JobOrder, JobProgress, MachineState, Prompt, Telemetry,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

/// What a refusal or a failure of an order says when OctoPrint has no control for it (M2, K3)
pub const NO_SUCH_CONTROL: &str = "OctoPrint has no such control";

/// K1: the lines of typed G-code, each as one Enter in the terminal (`terminal.js:421`): split at
/// `\n`, a trailing `\r` dropped, lines that are the empty string skipped, nothing trimmed.
fn gcode_lines(text: &str) -> Vec<&str> {
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).filter(|l| !l.is_empty()).collect()
}

/// K12
const NOT_STARTED: &str = "OctoPrint did not start the print";

/// The hooks' identifier, as the plugin identifier of a hook (`core.py`'s `get_hooks` keys)
const IDENTIFIER: &str = "maker_connect_top_edge";

/// The Marlin port: OctoPrint's `Printer`, connected to the serial port of the agent's options
pub struct OctoPrintPort {
    port: Option<String>,
    baudrate: Option<i64>,
}

impl OctoPrintPort {
    /// `port` and `baudrate` as the agent's `--port` and `--baud`; `None` is OctoPrint's own choice
    /// (`connect`: the settings, then detection)
    pub fn new(port: Option<String>, baudrate: Option<u32>) -> OctoPrintPort {
        OctoPrintPort { port, baudrate: baudrate.map(i64::from) }
    }
}

impl Port for OctoPrintPort {
    fn run(self: Box<Self>, orders: Receiver<Order>, events: Sender<Event>) {
        // events.py: nothing is delivered before STARTUP
        event_manager().fire(Events::STARTUP, None);

        let printer = Printer::new(Arc::new(LocalFileManager), Arc::new(PrinterProfileManager::new()));
        let plugin = ActionCommandPromptPlugin::new(printer.clone());
        plugin.register();

        let edge = Edge::new(events);
        edge.printer.set(Arc::downgrade(&printer)).expect("set once");
        edge.install();
        event_manager().set_sink(edge.clone());
        printer.set_top_edge(edge.clone());

        edge.publish();
        printer.connect(self.port.clone(), self.baudrate, None);

        for order in orders {
            match order {
                Order::Command { id, action } => edge.command(&printer, id, action),
                Order::Job { order, file } => edge.start_job(&printer, order, &file),
                Order::Control(control) => edge.control(&printer, control),
                Order::Answer(button) => edge.answer(&plugin, button),
                Order::Close => break,
            }
        }

        // K18
        edge.fail_pending();
        printer.disconnect();
        edge.uninstall();
        event_manager().fire(Events::SHUTDOWN, None);
        *lock(&edge.out) = None;
    }
}

/// K18: the reason of a command given up when comm closes or the port is told to close
const CONNECTION_CLOSED: &str = "the printer connection closed";

/// A command waiting for its lines to be sent and `ok`-ed (M4)
struct Pending {
    id: u32,
    tag: String,
    /// lines comm queued with the tag (`gcode.queuing`), counted until `queued_all`
    queued: usize,
    /// the call that queues them has returned
    queued_all: bool,
    sent: usize,
    acked: usize,
    output: Vec<String>,
    /// SdList: done when comm has the file list (K11)
    sd_list: bool,
}

/// The job being run, from its order until its end
struct ActiveJob {
    id: i64,
    name: String,
    size: u64,
    /// `job-<id>.d` (K6), removed at the end
    dir: Option<PathBuf>,
    started: bool,
}

/// Temperatures as comm's last report gave them
#[derive(Default, Clone, PartialEq)]
struct Temps {
    tools: Vec<(i64, TempTuple)>,
    bed: TempTuple,
    chamber: TempTuple,
}

#[derive(Default)]
struct Inner {
    /// comm's state, as `on_comm_state_change` told it
    state: Option<i64>,
    /// `getStateString` of that state, from comm at the time
    state_text: String,
    gone: bool,
    temps: Temps,
    position: Option<[f32; 3]>,
    sd_ready: Option<bool>,
    prompt: Option<Prompt>,
    job: Option<ActiveJob>,
    pending: VecDeque<Pending>,
    /// the lines received since the last `Send:` of the terminal log, and whether one was an `ok`
    recent: Vec<String>,
    ok_since_send: bool,
    /// K18-1: comm has begun `_trigger_emergency_stop` (comm.py:5487): the lines it sends from there to the
    /// close are its own and credit no pending command, until the connection is gone or back
    emergency: bool,
    /// K8
    refusal: Option<String>,
    last: Option<Telemetry>,
}

/// What comm and the event manager call into
struct Edge {
    /// None once the port has ended: the engine's listener ends when the last sender is gone, and the
    /// hooks, the sink and the `Printer` keep this `Edge` alive after that
    out: Mutex<Option<Sender<Event>>>,
    printer: OnceLock<Weak<Printer>>,
    inner: Mutex<Inner>,
    receiver: Mutex<Option<MessageReceiver>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// OctoPrint's web page sends a number as JSON, and Python reads `215` as an `int` and `215.5` as a
/// `float`: `S215`, not `S215.0`, goes to the printer
fn json_number(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9.0e15 { Value::from(n as i64) } else { Value::from(n) }
}

fn f32_of(v: Option<f64>) -> Option<f32> {
    v.map(|v| v as f32)
}

/// M3: `MachineCom`'s states (`comm.py:445-460`) to our six
pub fn machine_state(state: i64) -> MachineState {
    match state {
        MachineCom::STATE_NONE
        | MachineCom::STATE_OPEN_SERIAL
        | MachineCom::STATE_DETECT_SERIAL
        | MachineCom::STATE_CONNECTING
        | MachineCom::STATE_CLOSED => MachineState::Disconnected,
        MachineCom::STATE_OPERATIONAL => MachineState::Idle,
        MachineCom::STATE_STARTING
        | MachineCom::STATE_PRINTING
        | MachineCom::STATE_PAUSING
        | MachineCom::STATE_RESUMING
        | MachineCom::STATE_FINISHING
        | MachineCom::STATE_CANCELLING
        | MachineCom::STATE_TRANSFERING_FILE => MachineState::Busy,
        MachineCom::STATE_PAUSED => MachineState::Paused,
        MachineCom::STATE_ERROR | MachineCom::STATE_CLOSED_WITH_ERROR => MachineState::Error,
        // a state `comm.py` does not have: its own text says so (`getStateString`)
        _ => MachineState::Error,
    }
}

/// K9: `getStateString`'s text that says nothing more than our state
fn plain(text: &str) -> bool {
    matches!(text, "Offline" | "Operational" | "Printing" | "Paused")
}

/// What an `ApiError` says
fn api_text(e: ApiError) -> String {
    match e {
        ApiError::Abort { description, .. } => description,
        ApiError::Exception(text) => text,
    }
}

fn tags_for(api: &str, id: u32) -> BTreeSet<String> {
    ["source:api".to_string(), api.to_string(), tag_of(id)].into_iter().collect()
}

fn tag_of(id: u32) -> String {
    format!("maker-connect:{id}")
}

impl Edge {
    fn new(events: Sender<Event>) -> Arc<Edge> {
        Arc::new(Edge {
            out: Mutex::new(Some(events)),
            printer: OnceLock::new(),
            inner: Mutex::new(Inner::default()),
            receiver: Mutex::new(None),
        })
    }

    fn printer(&self) -> Option<Arc<Printer>> {
        self.printer.get().and_then(Weak::upgrade)
    }

    fn send(&self, event: Event) {
        if let Some(out) = lock(&self.out).as_ref() {
            let _ = out.send(event);
        }
    }

    /// The hooks (M4, tags) and the prompt plugin's messages (M3)
    fn install(self: &Arc<Self>) {
        let manager = plugin_manager();
        for hook in ["octoprint.comm.protocol.gcode.queuing", "octoprint.comm.protocol.gcode.sent"] {
            let me = self.clone();
            let handler: HookFn = Arc::new(move |args: &[Value]| {
                // (phase, command, command_type, gcode, subcode, tags)
                let phase = args.first().and_then(Value::as_str).unwrap_or("");
                // K18-1: comm's own `_gcode_M112_queuing` (comm.py:5524) runs right after the hooks of this very
                // phase and calls `_trigger_emergency_stop`, whatever the command's tags are
                if phase == "queuing" && args.get(3).and_then(Value::as_str) == Some("M112") {
                    lock(&me.inner).emergency = true;
                }
                let tags = args.get(5).and_then(Value::as_array);
                if let Some(tag) = tags.and_then(|t| t.iter().filter_map(Value::as_str).find(|t| t.starts_with("maker-connect:"))) {
                    me.line_phase(phase, tag);
                }
                Value::Null
            });
            manager.register_hook(hook, IDENTIFIER, handler);
        }
        let me = self.clone();
        let receiver: MessageReceiver = Arc::new(move |plugin, data| {
            if plugin == super::action_command_prompt::IDENTIFIER {
                me.prompt_message(data);
            }
        });
        manager.register_message_receiver(receiver.clone());
        *lock(&self.receiver) = Some(receiver);
    }

    fn uninstall(&self) {
        if let Some(receiver) = lock(&self.receiver).take() {
            plugin_manager().unregister_message_receiver(&receiver);
        }
    }

    // ---- telemetry (M3) ----

    /// Our report as it is now; sent to the engine when it changed
    fn publish(&self) {
        let progress = self.printer().and_then(|p| p.comm()).map(|comm| {
            let position = comm.get_file_position().map(|f| f.pos);
            (position, comm.get_print_time(), comm.get_current_tool())
        });
        let mut inner = lock(&self.inner);
        let mut t = Telemetry::default();
        let state = inner.state.unwrap_or(MachineCom::STATE_NONE);
        t.state = machine_state(state);
        if !plain(&inner.state_text) && !inner.state_text.is_empty() {
            t.problem = Some(inner.state_text.clone());
        }
        if let Some(refusal) = &inner.refusal {
            t.problem = Some(refusal.clone());
        }
        if let Some(prompt) = &inner.prompt
            && t.state != MachineState::Disconnected
        {
            t.state = MachineState::Attention;
            t.prompt = Some(prompt.clone());
        }
        let current_tool = progress.as_ref().map(|p| p.2);
        let tool0 = inner.temps.tools.iter().find(|(n, _)| *n == 0).map(|(_, t)| *t);
        t.nozzle_c = tool0.and_then(|t| f32_of(t.0));
        t.nozzle_target_c = tool0.and_then(|t| f32_of(t.1));
        t.bed_c = f32_of(inner.temps.bed.0);
        t.bed_target_c = f32_of(inner.temps.bed.1);
        t.chamber_c = f32_of(inner.temps.chamber.0);
        t.chamber_target_c = f32_of(inner.temps.chamber.1);
        if inner.temps.tools.len() > 1 {
            t.tools = inner
                .temps
                .tools
                .iter()
                .map(|(n, temp)| maker_connect_protocol::ToolHeat {
                    number: *n as u32 + 1,
                    nozzle_c: f32_of(temp.0),
                    nozzle_target_c: f32_of(temp.1),
                    active: Some(*n) == current_tool,
                })
                .collect();
        }
        t.position_mm = inner.position;
        t.sd_card = inner.sd_ready;
        if let Some(job) = &inner.job {
            t.job = Some(JobProgress {
                file: job.name.clone(),
                done: progress.as_ref().and_then(|p| p.0).map_or(0, |p| p.max(0) as u64),
                total: job.size,
                elapsed_s: progress.as_ref().and_then(|p| p.1).map(|s| s.max(0.0) as u64),
                remaining_s: None,
            });
        }
        if inner.last.as_ref() != Some(&t) {
            inner.last = Some(t.clone());
            self.send(Event::Telemetry(Box::new(t)));
        }
    }

    // ---- the terminal log, the hooks (M4) ----

    fn line_phase(&self, phase: &str, tag: &str) {
        {
            let mut inner = lock(&self.inner);
            let Inner { pending, recent, ok_since_send, .. } = &mut *inner;
            let Some(p) = pending.iter_mut().find(|p| p.tag == tag) else { return };
            match phase {
                "queuing" => p.queued += 1,
                "sent" => {
                    p.sent += 1;
                    // the lines received since its `Send:` (comm logs it before it writes)
                    p.output.extend(recent.iter().cloned());
                    if *ok_since_send {
                        p.acked += 1;
                    }
                }
                _ => {}
            }
        }
        self.settle();
    }

    /// Commands whose lines are all sent and `ok`-ed are done
    fn settle(&self) {
        let done: Vec<Pending> = {
            let mut inner = lock(&self.inner);
            let mut done = Vec::new();
            let mut i = 0;
            while i < inner.pending.len() {
                let p = &inner.pending[i];
                if !p.sd_list && p.queued_all && p.sent >= p.queued && p.acked >= p.sent {
                    done.extend(inner.pending.remove(i));
                } else {
                    i += 1;
                }
            }
            done
        };
        for p in done {
            self.send(Event::Done(CommandDone { id: p.id, ok: true, reason: None, output: p.output }));
        }
    }

    /// K18 (ours): every command still pending is given up, "the printer connection closed"
    fn fail_pending(&self) {
        let pending: Vec<Pending> = lock(&self.inner).pending.drain(..).collect();
        for p in pending {
            self.send(Event::Done(CommandDone {
                id: p.id,
                ok: false,
                reason: Some(CONNECTION_CLOSED.to_owned()),
                output: p.output,
            }));
        }
    }

    fn done_now(&self, id: u32, ok: bool, reason: Option<String>, output: Vec<String>) {
        self.send(Event::Done(CommandDone { id, ok, reason, output }));
    }

    // ---- orders ----

    fn command(self: &Arc<Self>, printer: &Arc<Printer>, id: u32, action: Action) {
        lock(&self.inner).refusal = None;
        let tag = tag_of(id);
        lock(&self.inner).pending.push_back(Pending {
            id,
            tag,
            queued: 0,
            queued_all: false,
            sent: 0,
            acked: 0,
            output: Vec::new(),
            sd_list: false,
        });
        let result = self.carry_out(printer, id, action);
        match result {
            Err(reason) => {
                lock(&self.inner).pending.retain(|p| p.id != id);
                self.done_now(id, false, Some(reason), Vec::new());
            }
            Ok(sd_list) => {
                {
                    let mut inner = lock(&self.inner);
                    if let Some(p) = inner.pending.iter_mut().find(|p| p.id == id) {
                        p.queued_all = true;
                        p.sd_list = sd_list;
                        if p.queued == 0 && !sd_list {
                            // K11: nothing was sent
                            let p = inner.pending.iter().position(|p| p.id == id).and_then(|i| inner.pending.remove(i));
                            drop(inner);
                            self.send(Event::Done(CommandDone {
                                id,
                                ok: true,
                                reason: None,
                                output: p.map(|p| p.output).unwrap_or_default(),
                            }));
                            return;
                        }
                    }
                }
                if sd_list {
                    // the list may have come while the order was carried out
                    self.sd_list_ready(printer);
                } else {
                    self.settle();
                }
            }
        }
        self.publish();
    }

    /// M1: the OctoPrint call for each action, after its API endpoint's refusals. `Ok(true)` is an
    /// SdList that waits for the card's list.
    fn carry_out(&self, printer: &Arc<Printer>, id: u32, action: Action) -> Result<bool, String> {
        let refuse = |e: ApiError| api_text(e);
        let printer_error = |e: PrinterError| e.to_string();
        match action {
            Action::Gcode { text } => {
                // `printerCommand` (printer.py:396): operational, then the command as the API makes it
                api_printer::check_operational(printer).map_err(refuse)?;
                // K1: one command for each non-empty line, as typed
                let lines = gcode_lines(&text);
                let PrinterCommand::Commands(commands) = api_printer::printer_command(&json!({ "commands": lines })).map_err(refuse)? else {
                    unreachable!("a command, not a script")
                };
                printer.commands(&Value::Array(commands), Some(api_printer::printer_command_tags().into_iter().chain([tag_of(id)]).collect()), false);
            }
            Action::Home { axes } => {
                api_printer::printer_printhead_command(printer, "home").map_err(refuse)?;
                let axes: Vec<Value> = axes
                    .iter()
                    .map(|a| {
                        Value::String(
                            match a {
                                Axis::X => "x",
                                Axis::Y => "y",
                                Axis::Z => "z",
                            }
                            .to_string(),
                        )
                    })
                    .collect();
                printer.home(&Value::Array(axes), Some(tags_for("api:printer.printhead", id))).map_err(printer_error)?;
            }
            Action::Jog { x, y, z, feed } => {
                api_printer::printer_printhead_command(printer, "jog").map_err(refuse)?;
                let mut axes = serde_json::Map::new();
                for (name, value) in [("x", x), ("y", y), ("z", z)] {
                    if let Some(value) = value {
                        axes.insert(name.to_string(), json_number(value));
                    }
                }
                let speed = feed.map(json_number);
                printer
                    .jog(&Value::Object(axes), true, speed.as_ref(), &[], Some(tags_for("api:printer.printhead", id)))
                    .map_err(printer_error)?;
            }
            Action::Extrude { mm } => {
                api_printer::printer_tool_command(printer).map_err(refuse)?;
                api_printer::printer_tool_extrude(printer).map_err(refuse)?;
                printer.extrude(&json_number(mm), None, Some(tags_for("api:printer.tool", id))).map_err(printer_error)?;
            }
            Action::Temperature { heater, celsius } => {
                let name = match heater {
                    Heater::Nozzle => {
                        api_printer::printer_tool_command(printer).map_err(refuse)?;
                        "tool0"
                    }
                    Heater::Bed => {
                        api_printer::printer_bed_command(printer).map_err(refuse)?;
                        "bed"
                    }
                };
                let api = if name == "bed" { "api:printer.bed" } else { "api:printer.tool" };
                printer
                    .set_temperature(&Value::String(name.to_string()), &json_number(celsius), Some(tags_for(api, id)))
                    .map_err(printer_error)?;
            }
            Action::Fan { on } => {
                self.control_panel_command(printer, id, if on { "M106 S255" } else { "M106 S0" })?;
            }
            Action::MotorsOff => {
                self.control_panel_command(printer, id, "M18")?;
            }
            Action::Speed { percent } => {
                api_printer::printer_printhead_command(printer, "feedrate").map_err(refuse)?;
                printer
                    .feed_rate(&Value::from(percent), Some(tags_for("api:printer.printhead", id)))
                    .map_err(|_| "factor is invalid".to_string())?;
            }
            Action::Flow { percent } => {
                api_printer::printer_tool_command(printer).map_err(refuse)?;
                printer
                    .flow_rate(&Value::from(percent), Some(tags_for("api:printer.tool", id)))
                    .map_err(|_| "factor is invalid".to_string())?;
            }
            Action::SdList => {
                api_printer::printer_sd_command(printer).map_err(refuse)?;
                if printer.is_sd_ready() {
                    printer.refresh_sd_files(false, Some(tags_for("api:printer.sd", id)), None);
                    return Ok(true);
                }
            }
            Action::SdPrint { file } => {
                api_printer::sd_file_select_and_print(printer, &file).map_err(refuse)?;
                // K7
                printer.select_file(&file, true, true, None, None, Some(tags_for("api:files.sd", id))).map_err(printer_error)?;
            }
            Action::SdDelete { file } => {
                api_printer::sd_file_delete(printer, &file).map_err(refuse)?;
                if api_printer::selected_sd_file(printer).as_deref() == Some(file.as_str()) {
                    printer.unselect_file();
                }
                printer.delete_sd_file(&file, Some(tags_for("api:files.sd", id)));
            }
            // M2
            Action::LoadFilament { .. }
            | Action::UnloadFilament { .. }
            | Action::ProbeBed
            | Action::Unlock
            | Action::SetOrigin
            | Action::GoOrigin
            | Action::ReadSettings
            | Action::Frame { .. } => return Err(NO_SUCH_CONTROL.to_string()),
        }
        Ok(false)
    }

    /// The control panel's buttons send `{type: "command", command}` through `printerCommand` (M1)
    fn control_panel_command(&self, printer: &Arc<Printer>, id: u32, text: &str) -> Result<(), String> {
        api_printer::check_operational(printer).map_err(api_text)?;
        let PrinterCommand::Commands(commands) = api_printer::printer_command(&json!({ "command": text })).map_err(api_text)? else {
            unreachable!("a command, not a script")
        };
        printer.commands(&Value::Array(commands), Some(api_printer::printer_command_tags().into_iter().chain([tag_of(id)]).collect()), false);
        Ok(())
    }

    /// `Order::Job`: K6, then `select_file(file, sd=false)` and `start_print` as the files and job APIs do
    fn start_job(self: &Arc<Self>, printer: &Arc<Printer>, order: JobOrder, file: &Path) {
        lock(&self.inner).refusal = None;
        let mut job = ActiveJob { id: order.id, name: order.name.clone(), size: order.size, dir: None, started: false };
        let end = |me: &Edge, job: ActiveJob, why: String| {
            me.finish_job(job, JobEnd { id: order.id, ok: false, stopped: false, reason: Some(why), restart_line: None });
        };
        // K3
        if order.from_line.is_some() {
            return end(self, job, NO_SUCH_CONTROL.to_string());
        }
        // K6
        let name = Path::new(&order.name).file_name().map_or_else(|| "job".to_string(), |n| n.to_string_lossy().into_owned());
        let dir = file.with_file_name(format!("job-{}.d", order.id));
        let linked = dir.join(&name);
        let made = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::hard_link(file, &linked).or_else(|_| std::fs::copy(file, &linked).map(|_| ())));
        job.dir = Some(dir);
        if let Err(e) = made {
            return end(self, job, format!("could not give OctoPrint the file: {e}"));
        }
        lock(&self.inner).job = Some(ActiveJob { id: job.id, name: job.name.clone(), size: job.size, dir: job.dir.clone(), started: false });
        let tags = || -> BTreeSet<String> { ["source:api", "api:job"].iter().map(|s| s.to_string()).collect() };
        let path = linked.to_string_lossy().into_owned();
        let attempt = (|| -> Result<(), String> {
            // files.py, `select`
            api_printer::file_select(printer, &path).map_err(api_text)?;
            printer.select_file(&path, false, false, None, None, Some(tags())).map_err(|e| e.to_string())?;
            // job.py, `start`
            api_printer::job_start(printer).map_err(api_text)?;
            printer.start_print(None, None, Some(tags())).map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(why) = attempt {
            if let Some(job) = lock(&self.inner).job.take() {
                end(self, job, why);
            }
            return;
        }
        // K12
        let started = lock(&self.inner).job.as_ref().is_some_and(|j| j.started);
        if !started && printer.comm().is_none_or(|c| c.get_state() == MachineCom::STATE_OPERATIONAL) {
            if let Some(job) = lock(&self.inner).job.take() {
                end(self, job, NOT_STARTED.to_string());
            }
        }
        self.publish();
    }

    /// `Order::Control` (`job.py:48-68`)
    fn control(&self, printer: &Arc<Printer>, control: JobControl) {
        let tags = || -> BTreeSet<String> { ["source:api", "api:job"].iter().map(|s| s.to_string()).collect() };
        let refused = match control {
            JobControl::Pause => api_printer::job_pause(printer).map(|()| printer.pause_print(None, Some(tags()))),
            JobControl::Resume => api_printer::job_pause(printer).map(|()| printer.resume_print(None, Some(tags()))),
            JobControl::Stop => api_printer::job_cancel(printer).map(|()| printer.cancel_print(None, Some(tags()))),
        };
        lock(&self.inner).refusal = refused.err().map(api_text);
        self.publish();
    }

    /// `Order::Answer`: the prompt plugin's `select` (`action_command_prompt/__init__.py:122`)
    fn answer(&self, plugin: &ActionCommandPromptPlugin, button: u8) {
        lock(&self.inner).refusal = plugin.api_select(i64::from(button)).err().map(|(_, text)| text);
        self.publish();
    }

    // ---- jobs ending ----

    fn finish_job(&self, job: ActiveJob, end: JobEnd) {
        if let Some(dir) = &job.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
        self.send(Event::JobEnd(end));
    }

    /// A print event of OctoPrint's: the job it ends, if it is one of ours
    fn print_event(&self, event: &str, payload: Option<&Value>) {
        let reason = payload.and_then(|p| p.get("reason")).and_then(Value::as_str);
        let error = payload.and_then(|p| p.get("error")).and_then(Value::as_str).filter(|e| !e.is_empty());
        let end = |id: i64, ok: bool, stopped: bool, why: Option<String>| JobEnd { id, ok, stopped, reason: why, restart_line: None };
        let Some(job) = lock(&self.inner).job.take_if(|_| event == Events::PRINT_DONE || event == Events::PRINT_FAILED) else {
            return;
        };
        let end = match (event, reason) {
            (Events::PRINT_DONE, _) => end(job.id, true, false, None),
            (_, Some("cancelled")) => end(job.id, false, true, None),
            _ => end(job.id, false, false, Some(error.unwrap_or("OctoPrint: the print failed").to_string())),
        };
        self.finish_job(job, end);
        self.publish();
    }

    // ---- the prompt plugin's messages (M3) ----

    fn prompt_message(&self, data: &Value) {
        let prompt = match data.get("action").and_then(Value::as_str) {
            Some("show") => Some(Prompt {
                text: data.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
                buttons: data
                    .get("choices")
                    .and_then(Value::as_array)
                    .map(|c| c.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default(),
            }),
            Some("close") => None,
            _ => return,
        };
        lock(&self.inner).prompt = prompt;
        self.publish();
    }

    fn sd_list_ready(&self, printer: &Arc<Printer>) {
        let done: Vec<Pending> = {
            let mut inner = lock(&self.inner);
            let mut done = Vec::new();
            let mut i = 0;
            while i < inner.pending.len() {
                if inner.pending[i].sd_list && inner.pending[i].queued_all {
                    done.extend(inner.pending.remove(i));
                } else {
                    i += 1;
                }
            }
            done
        };
        if done.is_empty() {
            return;
        }
        let names: Vec<String> = printer.get_sd_files(false).iter().filter_map(|f| f["name"].as_str().map(str::to_string)).collect();
        for p in done {
            self.send(Event::Done(CommandDone { id: p.id, ok: true, reason: None, output: names.clone() }));
        }
    }
}

impl MachineComPrintCallback for Edge {
    /// comm's terminal log: `Send: ...` and `Recv: ...` (M4)
    fn on_comm_log(&self, message: &str) {
        if message.starts_with("Send: ") {
            let mut inner = lock(&self.inner);
            inner.recent.clear();
            inner.ok_since_send = false;
            return;
        }
        let Some(line) = message.strip_prefix("Recv: ") else { return };
        {
            let mut inner = lock(&self.inner);
            let ok = line.starts_with("ok");
            inner.recent.push(line.to_string());
            if ok {
                inner.ok_since_send = true;
            }
            // K18-1: from `_trigger_emergency_stop` on, the lines on the wire are comm's own (they carry no
            // tags and fire no `gcode.queuing`/`sent` hook, comm.py:5017, :5036-5041): they credit nothing
            // the command whose line is on the wire: its lines so far, and its `ok`
            if inner.emergency {
                // nothing is credited
            } else if let Some(p) = inner.pending.iter_mut().find(|p| p.sent > p.acked) {
                p.output.push(line.to_string());
                if ok {
                    p.acked += 1;
                }
            }
        }
        self.settle();
    }

    /// K18-1: comm's error path (comm.py:4110-4136) calls `_trigger_emergency_stop(close=False)` right after
    /// this callback when its consequence is `emergency`
    fn on_comm_error(&self, _error: &str, _reason: &str, consequence: Option<&str>, _faq: Option<&str>, _logs: Option<&[String]>) {
        if consequence == Some("emergency") {
            lock(&self.inner).emergency = true;
        }
    }

    fn on_comm_temperature_update(&self, temp: &[(i64, TempTuple)], bed_temp: TempTuple, chamber_temp: TempTuple, _custom_temp: &[(String, TempTuple)]) {
        lock(&self.inner).temps = Temps { tools: temp.to_vec(), bed: bed_temp, chamber: chamber_temp };
        self.publish();
    }

    fn on_comm_progress(&self) {
        self.publish();
    }

    fn on_comm_position_update(&self, position: &BTreeMap<String, Option<f64>>, _reason: Option<&str>) {
        if let (Some(Some(x)), Some(Some(y)), Some(Some(z))) = (position.get("x"), position.get("y"), position.get("z")) {
            lock(&self.inner).position = Some([*x as f32, *y as f32, *z as f32]);
            self.publish();
        }
    }

    fn on_comm_sd_state_change(&self, sd_ready: bool) {
        lock(&self.inner).sd_ready = Some(sd_ready);
        self.publish();
    }

    fn on_comm_sd_files(&self, _files: &[Value]) {
        if let Some(printer) = self.printer() {
            self.sd_list_ready(&printer);
        }
    }

    fn on_comm_print_job_started(&self, _suppress_script: bool, _user: Option<&str>) {
        if let Some(job) = lock(&self.inner).job.as_mut() {
            job.started = true;
        }
        self.publish();
    }

    /// K4, M3, and a job that a connection that ended leaves behind
    fn on_comm_state_change(&self, state: i64) {
        let text = self.printer().and_then(|p| p.comm()).map(|c| c.get_state_string(Some(state)));
        let error = self.printer().and_then(|p| p.comm()).map(|c| c.get_error_string()).filter(|e| !e.is_empty());
        let (gone, back, job) = {
            let mut inner = lock(&self.inner);
            inner.state = Some(state);
            inner.state_text = text.unwrap_or_default();
            inner.refusal = None;
            let closed = state == MachineCom::STATE_CLOSED || state == MachineCom::STATE_CLOSED_WITH_ERROR;
            let gone = closed && !inner.gone;
            let back = state == MachineCom::STATE_OPERATIONAL && inner.gone;
            if gone {
                inner.gone = true;
                inner.emergency = false;
            }
            if back {
                inner.gone = false;
                inner.emergency = false;
            }
            // `Printer.on_comm_state_change` fails a print that was printing; any other job that
            // the connection leaves behind ends here
            let ended = matches!(state, MachineCom::STATE_CLOSED | MachineCom::STATE_CLOSED_WITH_ERROR | MachineCom::STATE_ERROR);
            let job = if ended { inner.job.take() } else { None };
            (gone, back, job)
        };
        if let Some(job) = job {
            let why = error.unwrap_or_else(|| lock(&self.inner).state_text.clone());
            let id = job.id;
            self.finish_job(job, JobEnd { id, ok: false, stopped: false, reason: Some(why), restart_line: None });
        }
        if gone {
            // K18
            self.fail_pending();
            self.send(Event::Gone);
        }
        if back {
            self.send(Event::Back);
        }
        self.publish();
    }
}

impl EventSink for Edge {
    fn fire(&self, event: &str, payload: Option<Value>) {
        if event == Events::PRINT_DONE || event == Events::PRINT_FAILED {
            self.print_event(event, payload.as_ref());
        }
    }
}

#[cfg(test)]
mod tests {
    //! OURS, all of them: the top edge has no test of OctoPrint's. None opens a real device: the
    //! printer is a recording fake in memory that stands in for the serial object (`comm.rs`'s test seam
    //! in `_open_serial`), its name a path in the temp directory, as the test guard of `comm.rs` wants.
    //! The firmware answers `ok` to each line, as Marlin does.
    use super::*;
    use crate::{Engine, MachineId, Report};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn m3_each_state_of_comm_is_one_of_our_six() {
        use MachineState::*;
        let table = [
            (MachineCom::STATE_NONE, Disconnected),
            (MachineCom::STATE_OPEN_SERIAL, Disconnected),
            (MachineCom::STATE_DETECT_SERIAL, Disconnected),
            (MachineCom::STATE_CONNECTING, Disconnected),
            (MachineCom::STATE_CLOSED, Disconnected),
            (MachineCom::STATE_OPERATIONAL, Idle),
            (MachineCom::STATE_STARTING, Busy),
            (MachineCom::STATE_PRINTING, Busy),
            (MachineCom::STATE_PAUSING, Busy),
            (MachineCom::STATE_RESUMING, Busy),
            (MachineCom::STATE_FINISHING, Busy),
            (MachineCom::STATE_CANCELLING, Busy),
            (MachineCom::STATE_TRANSFERING_FILE, Busy),
            (MachineCom::STATE_PAUSED, Paused),
            (MachineCom::STATE_ERROR, Error),
            (MachineCom::STATE_CLOSED_WITH_ERROR, Error),
        ];
        // all sixteen states of `comm.py:445-460` are in the table
        assert_eq!(table.len(), MachineCom::STATE_NAMES.len());
        for (state, ours) in table {
            assert_eq!(ours, machine_state(state), "state {state}");
        }
    }

    #[test]
    fn k9_the_plain_texts_say_nothing_more() {
        for text in ["Offline", "Operational", "Printing", "Paused"] {
            assert!(plain(text), "{text}");
        }
        for text in ["Connecting", "Starting", "Pausing", "Cancelling", "Error", "Offline after error", "Printing from SD"] {
            assert!(!plain(text), "{text}");
        }
    }

    /// The page's numbers are JSON numbers: whole ones are Python `int`s (`S215`, not `S215.0`)
    #[test]
    fn numbers_are_as_the_web_page_sends_them() {
        assert_eq!(json!(215), json_number(215.0));
        assert_eq!(json!(-10), json_number(-10.0));
        assert_eq!(json!(0.5), json_number(0.5));
        assert!(json_number(215.0).is_i64());
    }

    /// A fake printer in memory, the serial object comm opens (`comm.rs`'s test seam): records every
    /// line written, answers like Marlin, and can say lines of its own.
    struct FakePrinter {
        name: String,
        received: Mutex<Vec<String>>,
        to_comm: Mutex<std::sync::mpsc::Sender<Vec<u8>>>,
        from_printer: Mutex<std::sync::mpsc::Receiver<Vec<u8>>>,
        /// the delay of each answer (a print that takes a while)
        slow: Mutex<Duration>,
        closed: AtomicBool,
        /// the cable is pulled: reading fails, as pyserial's does
        unplugged: AtomicBool,
    }

    /// K18: every command still pending is given up on close (the `Order::Close` path calls this; the
    /// `Gone` path is in `the_top_edge_on_a_fake_printer`), each with what it had received
    #[test]
    fn k18_pending_commands_fail_when_the_port_closes() {
        let (tx, rx) = std::sync::mpsc::channel();
        let edge = Edge::new(tx);
        let pending = |id: u32, output: &[&str]| Pending {
            id,
            tag: tag_of(id),
            queued: 1,
            queued_all: true,
            sent: 1,
            acked: 0,
            output: output.iter().map(|l| l.to_string()).collect(),
            sd_list: false,
        };
        lock(&edge.inner).pending.extend([pending(1, &["echo:busy"]), pending(2, &[])]);
        edge.fail_pending();
        let done: Vec<CommandDone> = rx
            .try_iter()
            .map(|e| match e {
                Event::Done(d) => d,
                _ => panic!("only Done events"),
            })
            .collect();
        let why = Some("the printer connection closed".to_string());
        assert_eq!(
            vec![
                CommandDone { id: 1, ok: false, reason: why.clone(), output: vec!["echo:busy".to_string()] },
                CommandDone { id: 2, ok: false, reason: why, output: Vec::new() },
            ],
            done
        );
        assert!(lock(&edge.inner).pending.is_empty());
    }

    #[test]
    fn ours_typed_gcode_lines() {
        assert_eq!(vec!["G28", "G1 X1"], gcode_lines("G28\n\nG1 X1"));
        // not trimmed: a line with leading spaces keeps them; a blank-but-not-empty line is a line
        assert_eq!(vec!["  G1 X1", " "], gcode_lines("  G1 X1\n \n"));
        assert_eq!(vec!["G28", "G1"], gcode_lines("G28\r\n\r\nG1\r\n"));
        assert!(gcode_lines("").is_empty());
    }

    fn strip(line: &str) -> String {
        // `N<n> <text>*<checksum>`
        let mut text = line.trim();
        if text.starts_with('N') {
            if let Some((_, rest)) = text.split_once(' ') {
                text = rest;
            }
            if let Some((rest, sum)) = text.rsplit_once('*') {
                if sum.chars().all(|c| c.is_ascii_digit()) {
                    text = rest;
                }
            }
        }
        text.to_string()
    }

    impl super::super::comm::SerialTransport for FakePrinter {
        fn port(&self) -> Option<Option<String>> {
            Some(Some(self.name.clone()))
        }
        fn close(&self) -> Result<(), String> {
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn str(&self) -> String {
            "FakePrinter".to_string()
        }
        fn set_timeout(&self, _timeout: f64) {}
        fn timeout(&self) -> Option<f64> {
            Some(1.0)
        }
        fn set_baudrate(&self, _baudrate: i64) {}
        fn readline(&self) -> Result<Vec<u8>, super::super::comm::SerialError> {
            if self.unplugged.load(Ordering::SeqCst) {
                return Err(super::super::comm::SerialError::serial_exception("device reports readiness to read but returned no data (device disconnected or multiple access on port?)"));
            }
            let rx = lock(&self.from_printer);
            // a timeout, as pyserial's, is an empty line
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(line) if !self.closed.load(Ordering::SeqCst) => Ok(line),
                _ => Ok(Vec::new()),
            }
        }
        fn write(&self, data: &[u8]) -> Result<Option<i64>, super::super::comm::SerialError> {
            for line in String::from_utf8_lossy(data).lines() {
                let text = strip(line);
                lock(&self.received).push(line.trim().to_string());
                let delay = *lock(&self.slow);
                std::thread::sleep(delay);
                let answer = match text.split_whitespace().next().unwrap_or("") {
                    "M105" => "ok T:21.0 /0.0 B:20.0 /0.0 @:0 B@:0".to_string(),
                    "M115" => "FIRMWARE_NAME:Marlin 2.1.2 (fake) PROTOCOL_VERSION:1.0\nCap:PROMPT_SUPPORT:1\nok".to_string(),
                    "M114" => "X:10.00 Y:20.00 Z:0.30 E:0.00 Count X:800 Y:1600 Z:120\nok".to_string(),
                    "M21" => "echo:SD card ok\nok".to_string(),
                    "M20" => "Begin file list\nBENCHY.GCO 1234\nEnd file list\nok".to_string(),
                    "M112" | "M109" => String::new(),
                    _ => "ok".to_string(),
                };
                for answer in answer.lines() {
                    self.say(answer);
                }
            }
            Ok(Some(data.len() as i64))
        }
    }

    impl FakePrinter {
        fn new() -> Arc<FakePrinter> {
            static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let name = std::path::Path::new(&super::super::comm::test_serial_dir())
                .join(format!("ttyFAKE{}-{}", std::process::id(), COUNT.fetch_add(1, Ordering::SeqCst)))
                .to_string_lossy()
                .into_owned();
            let (tx, rx) = std::sync::mpsc::channel();
            let printer = Arc::new(FakePrinter {
                name: name.clone(),
                received: Mutex::new(Vec::new()),
                to_comm: Mutex::new(tx),
                from_printer: Mutex::new(rx),
                slow: Mutex::new(Duration::ZERO),
                closed: AtomicBool::new(false),
                unplugged: AtomicBool::new(false),
            });
            lock(&super::super::comm::TEST_TRANSPORTS).push((name, printer.clone()));
            printer
        }

        /// the commands received so far, without line numbers and checksums, and without the
        /// temperature polls comm makes by itself
        fn commands(&self) -> Vec<String> {
            lock(&self.received).iter().map(|l| strip(l)).filter(|c| c != "M105").collect()
        }

        /// a line from the printer
        fn say(&self, line: &str) {
            let _ = lock(&self.to_comm).send(format!("{line}\n").into_bytes());
        }
    }

    fn until(engine: &Engine, id: MachineId, what: &str, ready: impl Fn(&Report) -> bool) -> Report {
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            let report = engine.report(id).unwrap();
            if ready(&report) {
                return report;
            }
            assert!(Instant::now() < deadline, "waited for {what}; the report: {report:#?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn done_of(engine: &Engine, id: MachineId, command: u32) -> CommandDone {
        let report = until(engine, id, &format!("command {command} done"), |r| r.done.iter().any(|d| d.id == command));
        report.done.into_iter().find(|d| d.id == command).unwrap()
    }

    fn job_order(id: i64, name: &str, size: u64) -> JobOrder {
        JobOrder { id, name: name.to_string(), download: String::new(), size, from_line: None, tape_mm: None, label_length_mm: None }
    }

    fn command(engine: &Engine, id: MachineId, n: u32, action: Action) -> CommandDone {
        engine.send(id, Order::Command { id: n, action }).unwrap();
        done_of(engine, id, n)
    }

    /// The whole top edge on a fake printer, one port from `STARTUP` to `SHUTDOWN` (the event
    /// manager's worker ends with `SHUTDOWN`, so a process has one such run): connecting, M3, M1 and
    /// M4 for the controls, the refusals of OctoPrint's API, a job from `select_file` to its end,
    /// pause, resume and stop, a prompt, K4's Gone after an `M112` (K18-1), and closing.
    ///
    /// Own process (it shares OctoPrint's global event manager with nothing else), so the unit suite
    /// skips it. Run it with `scripts/octoprint-own-process-tests.sh`, or
    /// `cargo test -p maker-connect-engine --lib -- --ignored --exact ports::octoprint::port::tests::the_top_edge_on_a_fake_printer`.
    #[test]
    #[ignore = "own process: scripts/octoprint-own-process-tests.sh"]
    fn the_top_edge_on_a_fake_printer() {
        let printer = FakePrinter::new();
        let mut engine = Engine::new();
        let id = engine.add(Box::new(OctoPrintPort::new(Some(printer.name.clone()), Some(115_200))));

        // M3: operational is Idle; the temperatures come from comm's own polling
        let report = until(&engine, id, "Idle", |r| r.telemetry.state == MachineState::Idle && r.telemetry.nozzle_c.is_some());
        assert_eq!(Some(21.0), report.telemetry.nozzle_c);
        assert_eq!(Some(20.0), report.telemetry.bed_c);
        assert_eq!(None, report.telemetry.problem, "Operational is plain");
        assert!(report.connected);

        // M1 + M4: a Home XY sends what `home(["x","y"])` sends, done after the `ok`s
        let done = command(&engine, id, 1, Action::Home { axes: vec![Axis::X, Axis::Y] });
        assert!(done.ok, "{done:?}");
        let sent = printer.commands();
        let at = sent.iter().position(|c| c == "G91").expect("G91 sent");
        assert_eq!(["G91", "G28 X0 Y0", "G90"], sent[at..at + 3]);
        assert!(done.output.iter().all(|l| l.starts_with("ok") || l.contains("T:")), "{done:?}");
        assert_eq!(3, done.output.iter().filter(|l| l.starts_with("ok")).count(), "an ok for each line: {done:?}");

        // typed G-code as typed, whole; its answer is the output
        let done = command(&engine, id, 2, Action::Gcode { text: "M105".into() });
        assert!(done.ok);
        assert!(done.output.iter().any(|l| l.starts_with("ok T:21.0")), "{done:?}");
        assert!(command(&engine, id, 20, Action::Gcode { text: "m114".into() }).ok);
        assert!(printer.commands().contains(&"m114".to_string()), "as typed (K1): {:?}", printer.commands());

        // K1: a line each, empty ones send nothing, the others as typed (leading spaces kept)
        assert!(command(&engine, id, 21, Action::Gcode { text: "G28\n\nG1 X1".into() }).ok);
        let sent = printer.commands();
        let at = sent.iter().position(|c| c == "G28").expect("G28 sent");
        assert_eq!(["G28", "G1 X1"], sent[at..at + 2], "{sent:?}");

        // M1: Z home, a jog with the numbers as the page sends them, fan, temperature
        assert!(command(&engine, id, 3, Action::Home { axes: vec![Axis::Z] }).ok);
        assert!(printer.commands().contains(&"G28 Z0".to_string()));
        assert!(command(&engine, id, 4, Action::Jog { x: Some(10.0), y: None, z: None, feed: Some(3000.0) }).ok);
        assert!(printer.commands().contains(&"G0 X10 F3000".to_string()), "{:?}", printer.commands());
        assert!(command(&engine, id, 5, Action::Fan { on: true }).ok);
        assert!(printer.commands().contains(&"M106 S255".to_string()));
        assert!(command(&engine, id, 6, Action::Temperature { heater: Heater::Nozzle, celsius: 215.0 }).ok);
        assert!(printer.commands().iter().any(|c| c == "M104 S215"), "{:?}", printer.commands());
        assert!(command(&engine, id, 7, Action::MotorsOff).ok);
        assert!(printer.commands().contains(&"M18".to_string()));

        // M2
        let done = command(&engine, id, 8, Action::LoadFilament { nozzle_c: None, bed_c: None });
        assert!(!done.ok);
        assert_eq!(Some(NO_SUCH_CONTROL.to_string()), done.reason);
        let done = command(&engine, id, 9, Action::Frame { download: String::new() });
        assert_eq!(Some(NO_SUCH_CONTROL.to_string()), done.reason);

        // the card: ready, SdList is done with its list (K11), and a file that is not on it is OctoPrint's 404
        until(&engine, id, "the SD card", |r| r.telemetry.sd_card == Some(true));
        let done = command(&engine, id, 11, Action::SdList);
        assert!(done.ok, "{done:?}");
        assert!(done.output.iter().any(|n| n.eq_ignore_ascii_case("benchy.gco")), "{done:?}");
        let done = command(&engine, id, 12, Action::SdPrint { file: "nothere.gco".into() });
        assert!(!done.ok);
        assert!(done.reason.as_deref().is_some_and(|r| r.starts_with("The requested URL was not found")), "{done:?}");
        let done = command(&engine, id, 13, Action::SdDelete { file: "nothere.gco".into() });
        assert!(!done.ok, "{done:?}");

        // a job: the file given as `job-<id>`, a long one, slow enough to pause it
        let dir = std::env::temp_dir().join(format!("maker-connect-top-edge-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("job-42");
        let gcode: String = (0..60).map(|i| format!("G1 X{i}\n")).collect();
        std::fs::write(&file, &gcode).unwrap();
        *lock(&printer.slow) = Duration::from_millis(50);
        engine.send(id, Order::Job { order: job_order(42, "benchy.gcode", gcode.len() as u64), file: file.clone() }).unwrap();
        let report = until(&engine, id, "Busy", |r| r.telemetry.state == MachineState::Busy && r.telemetry.job.is_some());
        assert_eq!(Some(42), report.job);
        let progress = report.telemetry.job.unwrap();
        assert_eq!("benchy.gcode", progress.file);
        assert_eq!(gcode.len() as u64, progress.total);

        // OctoPrint's refusal while it prints, as its text
        let done = command(&engine, id, 10, Action::Home { axes: vec![Axis::X] });
        assert!(!done.ok);
        assert_eq!(Some("Printer is not operational or currently printing".to_string()), done.reason);
        assert!(!printer.commands().iter().any(|c| c == "G28 X0"), "nothing was sent");
        // a second job is refused with job.py's text (files.py first: it is not ready)
        // pause, resume, stop: once it prints (a pause while it starts is overtaken by the print, as OctoPrint's)
        let deadline = Instant::now() + Duration::from_secs(20);
        while !printer.commands().contains(&"G1 X3".to_string()) {
            assert!(Instant::now() < deadline, "the print did not go on: {:?}", printer.commands());
            std::thread::sleep(Duration::from_millis(20));
        }
        engine.send(id, Order::Control(JobControl::Pause)).unwrap();
        until(&engine, id, "Paused", |r| r.telemetry.state == MachineState::Paused);
        engine.send(id, Order::Control(JobControl::Resume)).unwrap();
        until(&engine, id, "Busy again", |r| r.telemetry.state == MachineState::Busy);
        // a pause of a printer that is paused is refused with job.py's text only when not active; stopping works
        engine.send(id, Order::Control(JobControl::Stop)).unwrap();
        let report = until(&engine, id, "the job's end", |r| !r.job_ends.is_empty());
        assert_eq!(42, report.job_ends[0].id);
        assert!(!report.job_ends[0].ok);
        assert!(report.job_ends[0].stopped, "{:?}", report.job_ends);
        assert_eq!(None, report.job);
        assert!(!dir.join("job-42.d").exists(), "K6: the link is removed");
        engine.reported(id, 0, 1).unwrap();
        until(&engine, id, "Idle after the stop", |r| r.telemetry.state == MachineState::Idle);

        // a whole print, to its end
        *lock(&printer.slow) = Duration::ZERO;
        let short = dir.join("job-43");
        std::fs::write(&short, "G28\nG1 X1\nG1 X2\n").unwrap();
        engine.send(id, Order::Job { order: job_order(43, "short.gcode", 16), file: short }).unwrap();
        let report = until(&engine, id, "the end of job 43", |r| r.job_ends.iter().any(|e| e.id == 43));
        let end = report.job_ends.iter().find(|e| e.id == 43).unwrap();
        assert!(end.ok, "{end:?}");
        assert!(printer.commands().contains(&"G1 X2".to_string()));

        // a file that is not machinecode: OctoPrint's text, nothing sent
        let stl = dir.join("job-44");
        std::fs::write(&stl, "solid").unwrap();
        engine.send(id, Order::Job { order: job_order(44, "part.stl", 5), file: stl }).unwrap();
        let report = until(&engine, id, "the end of job 44", |r| r.job_ends.iter().any(|e| e.id == 44));
        let end = report.job_ends.iter().find(|e| e.id == 44).unwrap();
        assert_eq!(Some("Cannot select file for printing, not a machinecode file".to_string()), end.reason);
        // K3
        let resumed = dir.join("job-45");
        std::fs::write(&resumed, "G28\n").unwrap();
        let mut order = job_order(45, "again.gcode", 4);
        order.from_line = Some(3);
        engine.send(id, Order::Job { order, file: resumed }).unwrap();
        let report = until(&engine, id, "the end of job 45", |r| r.job_ends.iter().any(|e| e.id == 45));
        assert_eq!(Some(NO_SUCH_CONTROL.to_string()), report.job_ends.iter().find(|e| e.id == 45).unwrap().reason);

        // job.py's refusal of a stop with no job: the report says it (K8)
        engine.send(id, Order::Control(JobControl::Stop)).unwrap();
        let report = until(&engine, id, "the refusal", |r| r.telemetry.problem.is_some());
        assert_eq!(Some("Printer is neither printing nor paused, 'cancel' command cannot be performed".to_string()), report.telemetry.problem);

        // a question on the printer's screen is Attention, and its answer is `M876 S<n>`
        printer.say("//action:prompt_begin Change the filament?");
        printer.say("//action:prompt_button Continue");
        printer.say("//action:prompt_button Stop");
        printer.say("//action:prompt_show");
        let report = until(&engine, id, "Attention", |r| r.telemetry.state == MachineState::Attention);
        let prompt = report.telemetry.prompt.expect("its prompt");
        assert_eq!("Change the filament?", prompt.text);
        assert_eq!(["Continue", "Stop"], prompt.buttons[..]);
        engine.send(id, Order::Answer(1)).unwrap();
        until(&engine, id, "Idle after the answer", |r| r.telemetry.state == MachineState::Idle);
        assert!(printer.commands().contains(&"M876 S1".to_string()), "{:?}", printer.commands());

        // M4 with several commands pending (the hub's gate is gone, K18): an M109 the printer does not
        // answer, then an M112 typed after it. Comm forces the M112 ahead (`serial.emergencyCommands`),
        // sends it and the heaters off by itself, and closes: Gone.
        engine.send(id, Order::Command { id: 90, action: Action::Gcode { text: "M109 S200".into() } }).unwrap();
        until(&engine, id, "the M109 on the wire", |_| printer.commands().contains(&"M109 S200".to_string()));
        engine.send(id, Order::Command { id: 91, action: Action::Gcode { text: "M112".into() } }).unwrap();
        let report = until(&engine, id, "Gone", |r| !r.connected);
        assert!(!report.connected);
        // What the port reports (K18, K18-1): the M112 is failed by the close, as K18 says, and so is the
        // M109: the `ok`s that follow the M112 answer comm's own `M104 T0 S0` and `M140 S0` (comm.py:5487-5522),
        // which carry no tags and fire no hook, so they credit no pending command.
        let m112 = done_of(&engine, id, 91);
        assert_eq!(
            CommandDone { id: 91, ok: false, reason: Some("the printer connection closed".to_string()), output: Vec::new() },
            m112
        );
        let m109 = done_of(&engine, id, 90);
        assert_eq!(
            CommandDone { id: 90, ok: false, reason: Some("the printer connection closed".to_string()), output: Vec::new() },
            m109,
            "the M109 fails with the close, never `ok`"
        );
        let sent = printer.commands();
        let at = sent.iter().position(|c| c == "M109 S200").expect("M109 sent");
        assert_eq!(["M109 S200", "M112", "M112", "M104 T0 S0", "M140 S0"], sent[at..], "forced ahead, then the heaters off");

        // closing: disconnect, then SHUTDOWN
        engine.remove(id).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        // the first blocking point of each thread this port started (py_threading.rs, `Thread::start`; the audit
        // lists them against their Python sources): the event worker waits on its empty queue, the monitor
        // reads the serial line, the two repeated timers and the resettable timer wait on their events; the
        // sender thread has one too (a send-queue entry, or `_clear_to_send`)
        #[cfg(debug_assertions)]
        {
            let seen: Vec<String> = crate::ports::octoprint::py_threading::FIRST_BLOCKING
                .lock()
                .unwrap()
                .iter()
                .map(|(t, k, _)| format!("{t} | {k}"))
                .collect();
            let has = |start: &str, kind: &str| seen.iter().any(|x| x.contains(start) && x.ends_with(kind));
            assert!(has("EventManager::new", "Condition wait"), "{seen:?}");
            assert!(has("comm.rs:4267", "serial readline"), "{seen:?}");
            assert!(seen.iter().any(|x| x.contains("comm.rs:4269")), "{seen:?}");
            assert!(has("RepeatedTimer::start", "Condition wait"), "{seen:?}");
            assert!(has("ResettableTimer::start", "Condition wait"), "{seen:?}");
        }
    }

    /// K4: the cable is pulled. Comm's reader gets pyserial's `SerialException` (`comm.py:4176`), the connection
    /// ends with an error (`STATE_CLOSED_WITH_ERROR`), the report says `Gone`, and a command still pending is
    /// failed with the close (K18). The other phase of K4's `Gone` (an `M112`) is in `the_top_edge_on_a_fake_printer`.
    ///
    /// Own process, like that test (`scripts/octoprint-own-process-tests.sh`, or
    /// `cargo test -p maker-connect-engine --lib -- --ignored --exact ports::octoprint::port::tests::k4_an_unplugged_cable_is_gone`).
    #[test]
    #[ignore = "own process: scripts/octoprint-own-process-tests.sh"]
    fn k4_an_unplugged_cable_is_gone() {
        let printer = FakePrinter::new();
        let mut engine = Engine::new();
        let id = engine.add(Box::new(OctoPrintPort::new(Some(printer.name.clone()), Some(115_200))));
        let report = until(&engine, id, "Idle", |r| r.telemetry.state == MachineState::Idle && r.telemetry.nozzle_c.is_some());
        assert!(report.connected);

        // an M109 the printer never answers, on the wire, then the cable is pulled
        engine.send(id, Order::Command { id: 90, action: Action::Gcode { text: "M109 S200".into() } }).unwrap();
        until(&engine, id, "the M109 on the wire", |_| printer.commands().contains(&"M109 S200".to_string()));
        printer.unplugged.store(true, Ordering::SeqCst);

        let report = until(&engine, id, "Gone", |r| !r.connected);
        assert!(!report.connected);
        assert_eq!(
            CommandDone { id: 90, ok: false, reason: Some("the printer connection closed".to_string()), output: Vec::new() },
            done_of(&engine, id, 90)
        );
        engine.remove(id).unwrap();
    }
}
