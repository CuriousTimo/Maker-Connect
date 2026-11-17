//! Maker Connect's engine (`docs/plans/agent-engine.md`, step 1). It manages the machines: which there
//! are, their reports, their jobs, and the orders for them. It knows nothing about G-code or
//! devices: each machine is a [`Port`], which talks to its device exactly as its reference does.
//!
//! All of it is ours (no reference), as the owner approved it on 2026-11-12:
//! - O1: each machine's port runs on its own thread;
//! - O2: orders reach a machine one at a time, in the order they were given;
//! - O3: every [`CommandDone`] and [`JobEnd`] is kept until it is reported.
//!
//! Ports are built with their settings (port, baud, …), so there is no separate `open`.
//!
//! Its words are the agent–hub protocol's ([`maker_connect_protocol`]): [`Telemetry`],
//! [`Action`], [`JobOrder`], [`JobControl`], [`JobEnd`], [`CommandDone`].

pub mod ports;

use maker_connect_protocol::{Action, CommandDone, JobControl, JobEnd, JobOrder, Telemetry};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// What a machine's port is given.
#[derive(Debug, Clone, PartialEq)]
pub enum Order {
    /// A command from the hub, to be answered with a [`CommandDone`] of the same id.
    Command { id: u32, action: Action },
    /// A job, its file downloaded already; ends with a [`JobEnd`] of its id.
    Job { order: JobOrder, file: PathBuf },
    /// Pause, resume or stop the job.
    Control(JobControl),
    /// The button chosen for the question on the device's screen.
    Answer(u8),
    /// Let go of the device and end.
    Close,
}

/// What a machine's port tells the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Its report as it is now. Passed on untouched: the engine does not read or change what a
    /// port fills in.
    Telemetry(Box<Telemetry>),
    /// A command carried out, refused or failed.
    Done(CommandDone),
    /// A job ended.
    JobEnd(JobEnd),
    /// The device went away (unplugged, switched off). Whether and when to open it again is the
    /// port's, as its reference does it: the engine retries nothing.
    Gone,
    /// The device is there again.
    Back,
}

/// K18 / HS4 (ours, engine to hub, nothing on the wire to the device): the reason of a command that
/// is given up because the connection closed.
pub const CONNECTION_CLOSED: &str = "the connection closed";

#[cfg(feature = "devices")]
/// K18 / HS4 (ours): every [`Order::Command`] still waiting in `orders` fails with
/// [`CONNECTION_CLOSED`]; the other orders are dropped. The ports call it when they close.
pub(crate) fn fail_queued_commands(orders: &Receiver<Order>, events: &Sender<Event>) {
    while let Ok(order) = orders.try_recv() {
        if let Order::Command { id, .. } = order {
            fail_command(id, events);
        }
    }
}

#[cfg(feature = "devices")]
/// K18 / HS4 (ours): one command given up because the connection closed.
pub(crate) fn fail_command(id: u32, events: &Sender<Event>) {
    let _ = events.send(Event::Done(CommandDone {
        id,
        ok: false,
        reason: Some(CONNECTION_CLOSED.to_owned()),
        output: Vec::new(),
    }));
}

/// A machine's port: the only code that talks to its device. It runs until it is given
/// [`Order::Close`] or its orders end.
pub trait Port: Send + 'static {
    fn run(self: Box<Self>, orders: Receiver<Order>, events: Sender<Event>);
}

/// A machine in the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MachineId(usize);

/// What the engine knows of a machine, for the hub link.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// Its latest report, as its port gave it (untouched; while it is gone the hub link says it is
    /// disconnected, from [`Report::connected`]).
    pub telemetry: Telemetry,
    /// Commands done since last reported, in order (O3).
    pub done: Vec<CommandDone>,
    /// Jobs ended since last reported, in order (O3).
    pub job_ends: Vec<JobEnd>,
    /// The job it has, from its order until its end.
    pub job: Option<i64>,
    /// Whether its device is there (no [`Event::Gone`] since the last [`Event::Back`]).
    pub connected: bool,
}

/// The engine has no such machine, or that machine's port has ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closed;

struct Machine {
    orders: Sender<Order>,
    report: Arc<Mutex<Report>>,
    threads: Vec<JoinHandle<()>>,
}

/// The machines and their ports.
#[derive(Default)]
pub struct Engine {
    machines: Vec<Option<Machine>>,
}

impl Engine {
    pub fn new() -> Self {
        Engine::default()
    }

    /// Starts a machine: its port on a thread of its own (O1), and the engine's listener for
    /// what it tells.
    pub fn add(&mut self, port: Box<dyn Port>) -> MachineId {
        let (orders, port_orders) = channel();
        let (port_events, events) = channel();
        let report = Arc::new(Mutex::new(Report {
            connected: true,
            ..Report::default()
        }));
        let runs = std::thread::spawn(move || port.run(port_orders, port_events));
        let listens = {
            let report = report.clone();
            std::thread::spawn(move || listen(&events, &report))
        };
        self.machines.push(Some(Machine {
            orders,
            report,
            threads: vec![runs, listens],
        }));
        MachineId(self.machines.len() - 1)
    }

    fn machine(&self, id: MachineId) -> Result<&Machine, Closed> {
        self.machines
            .get(id.0)
            .and_then(Option::as_ref)
            .ok_or(Closed)
    }

    /// Gives a machine an order; its port takes them one at a time, in this order (O2).
    pub fn send(&self, id: MachineId, order: Order) -> Result<(), Closed> {
        let machine = self.machine(id)?;
        if let Order::Job { order: job, .. } = &order {
            machine.report.lock().unwrap().job = Some(job.id);
        }
        machine.orders.send(order).map_err(|_| Closed)
    }

    /// What the engine knows of a machine now. Its commands done and jobs ended stay until
    /// [`Engine::reported`] says they reached the hub (O3).
    pub fn report(&self, id: MachineId) -> Result<Report, Closed> {
        Ok(self.machine(id)?.report.lock().unwrap().clone())
    }

    /// The first `done` commands and `ended` jobs of a [`Report`] reached the hub: forgotten.
    pub fn reported(&self, id: MachineId, done: usize, ended: usize) -> Result<(), Closed> {
        let mut report = self.machine(id)?.report.lock().unwrap();
        let done = done.min(report.done.len());
        let ended = ended.min(report.job_ends.len());
        report.done.drain(..done);
        report.job_ends.drain(..ended);
        Ok(())
    }

    /// Closes a machine: its port is told to let go, and the engine waits for it to end.
    pub fn remove(&mut self, id: MachineId) -> Result<(), Closed> {
        let machine = self
            .machines
            .get_mut(id.0)
            .and_then(Option::take)
            .ok_or(Closed)?;
        let _ = machine.orders.send(Order::Close);
        drop(machine.orders);
        for thread in machine.threads {
            let _ = thread.join();
        }
        Ok(())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        for id in 0..self.machines.len() {
            let _ = self.remove(MachineId(id));
        }
    }
}

/// What a port tells, into its machine's report, until the port ends.
fn listen(events: &Receiver<Event>, report: &Mutex<Report>) {
    for event in events {
        let mut report = report.lock().unwrap();
        match event {
            Event::Telemetry(telemetry) => report.telemetry = *telemetry,
            Event::Done(done) => report.done.push(done),
            Event::JobEnd(end) => {
                if report.job == Some(end.id) {
                    report.job = None;
                }
                report.job_ends.push(end);
            }
            Event::Gone => report.connected = false,
            Event::Back => report.connected = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A port for the engine's own tests (not a device): it answers each order with what the
    /// test scripted, so the engine's passing on and collecting can be seen.
    struct Echo {
        seen: Arc<Mutex<Vec<Order>>>,
    }

    impl Port for Echo {
        fn run(self: Box<Self>, orders: Receiver<Order>, events: Sender<Event>) {
            for order in orders {
                self.seen.lock().unwrap().push(order.clone());
                let _ = match order {
                    Order::Command {
                        id,
                        action: Action::Gcode { text },
                    } if text == "gone" => {
                        let _ = events.send(Event::Gone);
                        events.send(Event::Done(CommandDone {
                            id,
                            ok: true,
                            reason: None,
                            output: Vec::new(),
                        }))
                    }
                    Order::Command {
                        id,
                        action: Action::Gcode { text },
                    } if text == "back" => {
                        let _ = events.send(Event::Back);
                        events.send(Event::Done(CommandDone {
                            id,
                            ok: true,
                            reason: None,
                            output: Vec::new(),
                        }))
                    }
                    Order::Command { id, .. } => events.send(Event::Done(CommandDone {
                        id,
                        ok: true,
                        reason: None,
                        output: vec![format!("{id}")],
                    })),
                    Order::Job { order, .. } => events.send(Event::JobEnd(JobEnd {
                        id: order.id,
                        ok: true,
                        stopped: false,
                        reason: None,
                        restart_line: None,
                    })),
                    Order::Control(_) | Order::Answer(_) => Ok(()),
                    Order::Close => return,
                };
            }
        }
    }

    fn engine_with_echo() -> (Engine, MachineId, Arc<Mutex<Vec<Order>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new();
        let id = engine.add(Box::new(Echo { seen: seen.clone() }));
        (engine, id, seen)
    }

    fn gcode(id: u32, text: &str) -> Order {
        Order::Command {
            id,
            action: Action::Gcode {
                text: text.to_owned(),
            },
        }
    }

    /// Waits until `ready` holds of the machine's report (the port runs on its own thread).
    fn until(engine: &Engine, id: MachineId, ready: impl Fn(&Report) -> bool) -> Report {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let report = engine.report(id).unwrap();
            if ready(&report) || Instant::now() > deadline {
                return report;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// O2: orders reach the port one at a time, in the order given. O3: every result is kept
    /// until reported, then forgotten.
    #[test]
    fn orders_go_in_order_and_results_stay_until_reported() {
        let (engine, id, seen) = engine_with_echo();
        for n in 1..=5 {
            engine.send(id, gcode(n, "G28")).unwrap();
        }
        let report = until(&engine, id, |r| r.done.len() == 5);
        assert_eq!(
            report.done.iter().map(|d| d.id).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5]
        );
        let ids: Vec<u32> = seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|o| match o {
                Order::Command { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(ids, [1, 2, 3, 4, 5]);
        // Reported up to the third: the rest stays.
        engine.reported(id, 3, 0).unwrap();
        assert_eq!(
            engine
                .report(id)
                .unwrap()
                .done
                .iter()
                .map(|d| d.id)
                .collect::<Vec<_>>(),
            [4, 5]
        );
        engine.reported(id, 9, 9).unwrap();
        assert!(engine.report(id).unwrap().done.is_empty());
    }

    /// The engine knows a machine's job from its order until its end.
    #[test]
    fn a_job_is_known_until_it_ends() {
        let (engine, id, _) = engine_with_echo();
        let order = JobOrder {
            id: 42,
            name: "benchy.gcode".into(),
            download: "/f/1".into(),
            size: 10,
            from_line: None,
            tape_mm: None,
            label_length_mm: None,
            page_size: None,
        };
        engine
            .send(
                id,
                Order::Job {
                    order,
                    file: PathBuf::from("/tmp/benchy.gcode"),
                },
            )
            .unwrap();
        let report = until(&engine, id, |r| !r.job_ends.is_empty());
        assert_eq!(report.job, None, "ended");
        assert_eq!(report.job_ends[0].id, 42);
    }

    /// A port's report is passed on as it gave it; a device gone is Disconnected until back.
    #[test]
    fn a_report_is_passed_on_and_a_device_gone_is_disconnected() {
        let (engine, id, _) = engine_with_echo();
        assert!(engine.report(id).unwrap().connected);
        engine.send(id, gcode(1, "gone")).unwrap();
        let report = until(&engine, id, |r| !r.done.is_empty());
        assert!(!report.connected);
        assert_eq!(
            report.telemetry,
            Telemetry::default(),
            "its report untouched"
        );
        engine.send(id, gcode(2, "back")).unwrap();
        let report = until(&engine, id, |r| r.done.len() == 2);
        assert!(report.connected);
    }

    /// A machine removed: its port is closed and ended; orders to it fail.
    #[test]
    fn a_removed_machine_takes_no_orders() {
        let (mut engine, id, seen) = engine_with_echo();
        engine.remove(id).unwrap();
        assert_eq!(seen.lock().unwrap().last(), Some(&Order::Close));
        assert_eq!(engine.send(id, gcode(1, "G28")), Err(Closed));
        assert_eq!(engine.report(id), Err(Closed));
    }

    /// The typed actions read as the hub writes them (JSON, tagged by `action`).
    #[test]
    fn actions_are_typed_on_the_wire() {
        let jog: Action =
            serde_json::from_str(r#"{"action":"jog","x":10.0,"feed":3000.0}"#).unwrap();
        assert_eq!(
            jog,
            Action::Jog {
                x: Some(10.0),
                y: None,
                z: None,
                feed: Some(3000.0)
            }
        );
        let heat: Action =
            serde_json::from_str(r#"{"action":"temperature","heater":"nozzle","celsius":215.0}"#)
                .unwrap();
        assert_eq!(
            heat,
            Action::Temperature {
                heater: maker_connect_protocol::Heater::Nozzle,
                celsius: 215.0
            }
        );
        let home: Action = serde_json::from_str(r#"{"action":"home","axes":["x","y"]}"#).unwrap();
        assert_eq!(
            home,
            Action::Home {
                axes: vec![
                    maker_connect_protocol::Axis::X,
                    maker_connect_protocol::Axis::Y
                ]
            }
        );
        let typed: Action = serde_json::from_str(r#"{"action":"gcode","text":"M105"}"#).unwrap();
        assert_eq!(
            typed,
            Action::Gcode {
                text: "M105".into()
            }
        );
    }
}
