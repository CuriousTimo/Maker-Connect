//! The device contract's adapters (`docs/plans/hub-core-v2.md` §2.1, §3 step 1; GitHub #14): each
//! port behind a contract [`Driver`] and [`Session`], as `docs/plans/hub-core-v2-device-map.md` maps
//! it. **All of it is ours**, and none of it reaches a machine: a command becomes the [`Order`] the
//! port already takes, and what the port tells ([`Event`]) becomes state patches and outcomes. What
//! goes on the wire is still only the port's, made by its reference's code, unchanged.
//!
//! - A1: a session runs its port on a thread of its own, as the engine does (O1), and a second
//!   thread that reads the port's events.
//! - A2: the state tree: `/state` is the reference's own state (the map's state set of the kind),
//!   removed when it is not known; `/report/<field>` is each field of the port's [`Telemetry`] as it
//!   is serialised, removed when absent; `/job_end` is the last [`JobEnd`]; `/connected` follows
//!   `Gone`/`Back`; `/output` holds the output lines of the last command that had any.
//! - A3: a command waits for its [`CommandDone`] (no time limit of ours): `ok` is [`Outcome::Ok`]; not
//!   `ok` is [`Outcome::Failed`] with the port's reason, unless the reason says the connection closed
//!   first ([`Outcome::Unknown`]: it may have gone out). A session that ends before an answer is
//!   [`Outcome::Unknown`].
//! - A4: a job waits for its [`JobEnd`] (`ok`: [`Outcome::Ok`]; else [`Outcome::Failed`]), or for a
//!   report naming it as running (`Telemetry::job_id`, set by the laser's port).
//! - A5: pause, resume and stop have no answer from a port (their refusal is the report's
//!   `problem`): [`Outcome::Unknown`], and the state shows what happened.
//! - A6: the enabled set is the baseline narrowed by the profile; the ports do not probe.
//!
//! The ports cannot tell the contract the reference's state through their [`Telemetry`] alone (it
//! holds our six `MachineState`s): the Marlin and laser ports take a hook that tells it
//! ([`crate::ports::octoprint::port::CommStateHook`], [`crate::ports::laserfoundry::port::MachineStateHook`]).

pub mod grbl;
pub mod labelmanager;
pub mod labelwriter;
pub mod marlin;
pub mod scale;

#[cfg(test)]
mod tests;

use crate::{Event, Order, Port};
use maker_connect_device::{
    CapabilitySet, Command, Driver, EnabledSet, Epoch, Identity, Outcome, Patch, Session, fault,
};
use maker_connect_protocol::{Action, CommandDone, JobControl, JobEnd, JobOrder, Telemetry};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

/// Sets `/state` to the reference's state (`None`: removed, not known).
pub type StateSink = Arc<dyn Fn(Option<&'static str>) + Send + Sync>;

/// Makes the port of a session, given the sink its state hook tells (the tests' fakes, or the real
/// port).
pub type MakePort = Arc<dyn Fn(StateSink) -> Box<dyn Port> + Send + Sync>;

/// What a command becomes for the port.
#[derive(Debug, Clone, PartialEq)]
pub enum Planned {
    Command(Action),
    Job { order: JobOrder, file: PathBuf },
    Control(JobControl),
}

/// One port's side of an adapter: its kind, baseline, identity, how a checked command becomes an
/// order, and where the reference's state comes from.
pub trait Mapping: Send + Sync + 'static {
    fn kind(&self) -> &'static str;
    /// The verbs this port carries, with the reference's bounds (device map); pure.
    fn baseline(&self) -> CapabilitySet;
    fn identity(&self) -> Identity;
    /// The order for a command the host checked against the enabled set. `job` names the job, if
    /// it is one. `Err` is the reason it cannot be made (a malformed argument the host let pass).
    fn plan(&self, cmd: &Command, job: i64) -> Result<Planned, String>;
    /// The reference's state from a report: `Some(state)` when this port's state comes from its
    /// reports, `None` when it comes from a hook.
    fn state_of(&self, telemetry: &Telemetry) -> Option<Option<&'static str>>;
    /// The port, with `state` given to its hook where it has one.
    fn port(&self, state: StateSink) -> Box<dyn Port>;
}

/// A port behind the contract.
pub struct PortDriver {
    mapping: Arc<dyn Mapping>,
}

impl PortDriver {
    pub fn new(mapping: impl Mapping) -> Self {
        Self {
            mapping: Arc::new(mapping),
        }
    }
}

impl Driver for PortDriver {
    fn kind(&self) -> &'static str {
        self.mapping.kind()
    }

    fn baseline(&self) -> CapabilitySet {
        self.mapping.baseline()
    }

    fn connect(&self, profile: &CapabilitySet) -> Result<Box<dyn Session>, String> {
        Ok(Box::new(PortSession::open(self.mapping.clone(), profile)))
    }
}

/// What a waiting `do_command` is told.
enum Answer {
    Done(CommandDone),
    Ended(JobEnd),
    Running,
}

#[derive(Default)]
struct Shared {
    state: Option<Sender<Patch>>,
    closed: bool,
    commands: HashMap<u32, Sender<Answer>>,
    jobs: HashMap<i64, Sender<Answer>>,
    report: Map<String, Value>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn patch(&self, patch: Patch) {
        if let Some(tx) = &self.state {
            let _ = tx.send(patch);
        }
    }
}

/// One port, running, behind the contract.
pub struct PortSession {
    mapping: Arc<dyn Mapping>,
    identity: Identity,
    enabled: EnabledSet,
    shared: Arc<Mutex<Shared>>,
    rx: Mutex<Option<Receiver<Patch>>>,
    orders: Mutex<Option<Sender<Order>>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    next_command: AtomicU32,
    next_job: AtomicI64,
}

impl PortSession {
    fn open(mapping: Arc<dyn Mapping>, profile: &CapabilitySet) -> Self {
        let (tx, rx) = channel();
        let shared = Arc::new(Mutex::new(Shared {
            state: Some(tx),
            ..Shared::default()
        }));
        let sink: StateSink = {
            let shared = Arc::downgrade(&shared);
            Arc::new(move |state| {
                if let Some(shared) = shared.upgrade() {
                    lock(&shared).patch(state_patch(state));
                }
            })
        };
        let port = mapping.port(sink);
        let (orders, port_orders) = channel();
        let (port_events, events) = channel();
        let runs = std::thread::spawn(move || port.run(port_orders, port_events));
        let listens = {
            let shared = shared.clone();
            let mapping = mapping.clone();
            std::thread::spawn(move || listen(&*mapping, &events, &shared))
        };
        Self {
            identity: mapping.identity(),
            enabled: EnabledSet::new(mapping.baseline().intersect(profile)),
            mapping,
            shared,
            rx: Mutex::new(Some(rx)),
            orders: Mutex::new(Some(orders)),
            threads: Mutex::new(vec![runs, listens]),
            next_command: AtomicU32::new(1),
            next_job: AtomicI64::new(1),
        }
    }

    fn send(&self, order: Order) -> bool {
        lock(&self.orders)
            .as_ref()
            .is_some_and(|o| o.send(order).is_ok())
    }
}

fn state_patch(state: Option<&'static str>) -> Patch {
    match state {
        Some(s) => Patch::set("/state", json!(s)),
        None => Patch::remove("/state"),
    }
}

/// A2, A3, A4: what the port tells, into patches and answers, until the port ends.
fn listen(mapping: &dyn Mapping, events: &Receiver<Event>, shared: &Mutex<Shared>) {
    for event in events {
        let mut s = lock(shared);
        match event {
            Event::Telemetry(t) => {
                if let Some(state) = mapping.state_of(&t) {
                    s.patch(state_patch(state));
                }
                if let Some(id) = t.job_id
                    && let Some(w) = s.jobs.remove(&id)
                {
                    let _ = w.send(Answer::Running);
                }
                let Ok(Value::Object(now)) = serde_json::to_value(&*t) else {
                    continue;
                };
                let before = std::mem::take(&mut s.report);
                for (k, v) in &now {
                    if before.get(k) != Some(v) {
                        s.patch(Patch::set(&format!("/report/{}", escape(k)), v.clone()));
                    }
                }
                for k in before.keys().filter(|k| !now.contains_key(*k)) {
                    s.patch(Patch::remove(&format!("/report/{}", escape(k))));
                }
                s.report = now;
            }
            Event::Done(done) => {
                if !done.output.is_empty() {
                    s.patch(Patch::set("/output", json!(done.output)));
                }
                if let Some(w) = s.commands.remove(&done.id) {
                    let _ = w.send(Answer::Done(done));
                }
            }
            Event::JobEnd(end) => {
                s.patch(Patch::set(
                    "/job_end",
                    serde_json::to_value(&end).unwrap_or(Value::Null),
                ));
                if let Some(w) = s.jobs.remove(&end.id) {
                    let _ = w.send(Answer::Ended(end));
                }
            }
            Event::Gone => s.patch(Patch::set("/connected", json!(false))),
            Event::Back => s.patch(Patch::set("/connected", json!(true))),
        }
    }
    // the port ended: nothing more will be answered
    let mut s = lock(shared);
    s.commands.clear();
    s.jobs.clear();
    s.state = None;
}

/// RFC 6901: `~` as `~0`, `/` as `~1`.
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// A3: the reasons a port gives a command it gave up because the connection closed (K18 / HS4).
fn closed_first(reason: Option<&str>) -> bool {
    matches!(
        reason,
        Some(crate::CONNECTION_CLOSED) | Some(crate::ports::octoprint::port::CONNECTION_CLOSED)
    )
}

const NO_ANSWER: &str = "the session ended before the port answered";

impl Session for PortSession {
    fn identity(&self) -> Identity {
        self.identity.clone()
    }

    fn capabilities(&self) -> (CapabilitySet, Epoch) {
        (self.enabled.set().clone(), self.enabled.epoch())
    }

    fn do_command(&self, cmd: &Command) -> Outcome {
        if lock(&self.shared).closed {
            return Outcome::Failed {
                reason: "session closed".into(),
            };
        }
        if !self.enabled.set().has(&cmd.verb) {
            return Outcome::Refused {
                fault: fault::CAPABILITY_NOT_SUPPORTED.code(),
                reason: None,
            };
        }
        let job = self.next_job.fetch_add(1, Ordering::Relaxed);
        let planned = match self.mapping.plan(cmd, job) {
            Ok(p) => p,
            Err(reason) => return Outcome::Failed { reason },
        };
        let (tx, rx) = channel();
        let order = match planned {
            Planned::Control(c) => {
                return if self.send(Order::Control(c)) {
                    Outcome::Unknown {
                        reason: "sent: a port answers no job control; see /state".into(),
                    }
                } else {
                    Outcome::Failed {
                        reason: "the port has ended".into(),
                    }
                };
            }
            Planned::Command(action) => {
                let id = self.next_command.fetch_add(1, Ordering::Relaxed);
                lock(&self.shared).commands.insert(id, tx);
                Order::Command { id, action }
            }
            Planned::Job { order, file } => {
                lock(&self.shared).jobs.insert(order.id, tx);
                Order::Job { order, file }
            }
        };
        if !self.send(order) {
            return Outcome::Failed {
                reason: "the port has ended".into(),
            };
        }
        match rx.recv() {
            Ok(Answer::Done(d)) if d.ok => Outcome::Ok,
            Ok(Answer::Done(d)) if closed_first(d.reason.as_deref()) => Outcome::Unknown {
                reason: d.reason.unwrap_or_default(),
            },
            Ok(Answer::Done(d)) => Outcome::Failed {
                reason: d.reason.unwrap_or_default(),
            },
            Ok(Answer::Ended(e)) if e.ok => Outcome::Ok,
            Ok(Answer::Ended(e)) => Outcome::Failed {
                reason: e.reason.unwrap_or_default(),
            },
            Ok(Answer::Running) => Outcome::Ok,
            Err(_) => Outcome::Unknown {
                reason: NO_ANSWER.into(),
            },
        }
    }

    fn take_state(&self) -> Option<Receiver<Patch>> {
        lock(&self.rx).take()
    }

    fn close(&self) {
        {
            let mut s = lock(&self.shared);
            s.closed = true;
            s.state = None;
            s.commands.clear();
            s.jobs.clear();
        }
        if let Some(orders) = lock(&self.orders).take() {
            let _ = orders.send(Order::Close);
        }
        let threads = std::mem::take(&mut *lock(&self.threads));
        for t in threads {
            let _ = t.join();
        }
    }
}

impl Drop for PortSession {
    fn drop(&mut self) {
        self.close();
    }
}

/// An argument's text, as the host checked it.
pub(crate) fn text<'a>(cmd: &'a Command, name: &str) -> Option<&'a str> {
    cmd.args.get(name).map(String::as_str)
}

/// A number argument; `Err` when it does not read as one.
pub(crate) fn number(cmd: &Command, name: &str) -> Result<Option<f64>, String> {
    text(cmd, name)
        .map(|t| {
            t.parse::<f64>()
                .map_err(|_| format!("{name}: not a number"))
        })
        .transpose()
}

/// A required argument.
pub(crate) fn needed<T>(v: Result<Option<T>, String>, name: &str) -> Result<T, String> {
    v?.ok_or_else(|| format!("{name}: missing"))
}

/// An integer argument.
pub(crate) fn integer(cmd: &Command, name: &str) -> Result<Option<i64>, String> {
    text(cmd, name)
        .map(|t| {
            t.parse::<i64>()
                .map_err(|_| format!("{name}: not an integer"))
        })
        .transpose()
}

/// A job order for a file on the agent (`name` its file name, `size` its length).
pub(crate) fn job_order(id: i64, path: &str) -> (JobOrder, PathBuf) {
    let file = PathBuf::from(path);
    let order = JobOrder {
        id,
        name: file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        download: String::new(),
        size: std::fs::metadata(&file).map_or(0, |m| m.len()),
        from_line: None,
        tape_mm: None,
        label_length_mm: None,
        page_size: None,
    };
    (order, file)
}
