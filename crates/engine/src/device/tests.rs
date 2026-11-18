//! One test per adapter: the conformance kit on it, and its commands as the port's orders. Fakes
//! only: no USB, no serial port; the laser is LaserFoundry's own simulator (`SimulatedGrbl`), so
//! nothing here can fire a laser.

use super::*;
use maker_connect_device::conformance::check_driver;
use maker_connect_device::registry::{self, *};
use maker_connect_device::validate::check;
use maker_connect_device::{Caller, StateTree};
use maker_connect_protocol::{Axis, Heater, MachineState, ScaleReading};
use std::time::{Duration, Instant};

/// A port that tells what it is scripted to and answers every command ok.
struct Script {
    seen: Arc<Mutex<Vec<Order>>>,
    sink: StateSink,
    state: Option<&'static str>,
    telemetry: Telemetry,
}

impl Port for Script {
    fn run(self: Box<Self>, orders: Receiver<Order>, events: Sender<Event>) {
        (self.sink)(self.state);
        let _ = events.send(Event::Telemetry(Box::new(self.telemetry.clone())));
        for order in orders {
            lock(&self.seen).push(order.clone());
            match order {
                Order::Command { id, .. } => {
                    let _ = events.send(Event::Done(CommandDone {
                        id,
                        ok: true,
                        reason: None,
                        output: vec!["ok".into()],
                    }));
                }
                Order::Job { order, .. } => {
                    let _ = events.send(Event::JobEnd(JobEnd {
                        id: order.id,
                        ok: true,
                        stopped: false,
                        reason: None,
                        restart_line: None,
                    }));
                }
                Order::Close => break,
                Order::Control(_) | Order::Answer(_) => {}
            }
        }
    }
}

fn script(
    seen: &Arc<Mutex<Vec<Order>>>,
    state: Option<&'static str>,
    telemetry: Telemetry,
) -> MakePort {
    let seen = seen.clone();
    Arc::new(move |sink| {
        Box::new(Script {
            seen: seen.clone(),
            sink,
            state,
            telemetry: telemetry.clone(),
        })
    })
}

/// Commands through the host's own validation, against a session of `driver`, with its tree.
struct Host {
    session: Box<dyn Session>,
    kind: &'static str,
    enabled: CapabilitySet,
    patches: Receiver<Patch>,
    tree: StateTree,
}

impl Host {
    fn new(driver: &dyn Driver) -> Host {
        let session = driver.connect(&driver.baseline()).unwrap();
        let patches = session.take_state().unwrap();
        Host {
            enabled: session.capabilities().0,
            kind: driver.kind(),
            session,
            patches,
            tree: StateTree::new(),
        }
    }

    /// Waits until the tree has `/state` = `state`.
    fn state(&mut self, state: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while self.tree.get("/state").and_then(Value::as_str) != Some(state) {
            let left = deadline.saturating_duration_since(Instant::now());
            let patch = self
                .patches
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no state {state}: {:?}", self.tree));
            self.tree.apply(&patch).unwrap();
        }
    }

    /// Applies the patches that come until none came for a while.
    fn settle(&mut self) {
        while let Ok(patch) = self.patches.recv_timeout(Duration::from_millis(200)) {
            self.tree.apply(&patch).unwrap();
        }
    }

    fn command(&mut self, caller: Caller, cmd: Command) -> Outcome {
        let state = self
            .tree
            .get("/state")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let checked = check(
            &self.enabled,
            registry::state_set(self.kind),
            state.as_deref(),
            caller,
            &cmd,
        )
        .unwrap_or_else(|r| panic!("refused by the host: {r:?}"));
        self.session.do_command(&checked)
    }
}

fn conforms(driver: &dyn Driver, safe: &Command) {
    let violations = check_driver(driver, Some(safe));
    assert!(violations.is_empty(), "{violations:#?}");
}

#[test]
fn marlin() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let make = script(&seen, Some("OPERATIONAL"), Telemetry::default());
    let driver = PortDriver::new(marlin::Marlin::with_port(make));
    conforms(&driver, &Command::new(SD_LIST));
    lock(&seen).clear();

    // comm's states by name, all 16 of the state set (comm.py:445-460)
    let names: Vec<&str> = (0..16).filter_map(marlin::state_name).collect();
    let declared: Vec<&str> = registry::state_set(KIND_MARLIN)
        .unwrap()
        .states
        .iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, declared);

    let mut host = Host::new(&driver);
    host.state("OPERATIONAL");
    let ok = |o: Outcome| assert_eq!(o, Outcome::Ok);
    ok(host.command(Caller::Person, Command::new(HOME).arg("axes", "x,y")));
    ok(host.command(Caller::Person, Command::new(MOVE_Z).arg("distance", "-0.5")));
    ok(host.command(
        Caller::Person,
        Command::new(SET_NOZZLE).arg("nozzle_temperature", "215"),
    ));
    ok(host.command(Caller::Person, Command::new(SET_SPEED).arg("speed", "150")));
    ok(host.command(Caller::Person, Command::new(FAN).arg("on", "false")));
    ok(host.command(Caller::Person, Command::new(DISABLE_STEPPERS)));
    ok(host.command(
        Caller::Person,
        Command::new(START_PRINT)
            .arg("path", "CUBE.GCO")
            .arg("origin", "sd"),
    ));
    // OctoPrint refuses a cancel when nothing prints (job.py:25,61-66): the host's state guard
    let state = host
        .tree
        .get("/state")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let refused = check(
        &host.enabled,
        registry::state_set(KIND_MARLIN),
        state.as_deref(),
        Caller::Person,
        &Command::new(STOP_PRINT),
    )
    .unwrap_err();
    assert_eq!(refused.fault, &fault::STATE_NOT_ACCEPTED);
    let actions: Vec<Order> = lock(&seen).clone();
    let cmd = |action| Order::Command { id: 0, action };
    let strip = |o: &Order| match o {
        Order::Command { action, .. } => cmd(action.clone()),
        other => other.clone(),
    };
    assert_eq!(
        actions.iter().map(strip).collect::<Vec<_>>(),
        vec![
            cmd(Action::Home {
                axes: vec![Axis::X, Axis::Y]
            }),
            cmd(Action::Jog {
                x: None,
                y: None,
                z: Some(-0.5),
                feed: None
            }),
            cmd(Action::Temperature {
                heater: Heater::Nozzle,
                celsius: 215.0
            }),
            cmd(Action::Speed { percent: 150 }),
            cmd(Action::Fan { on: false }),
            cmd(Action::MotorsOff),
            cmd(Action::SdPrint {
                file: "CUBE.GCO".into()
            }),
        ]
    );
    host.session.close();
}

#[test]
fn labelmanager() {
    // labelle's own port, over a USB that has no device
    let real = labelmanager::LabelManager::with_port(Arc::new(|_| {
        Box::new(crate::ports::labelle::DymoPnp::with_backend(Box::new(
            || Err("no USB in this test".to_owned()),
        )))
    }));
    conforms(
        &PortDriver::new(real),
        &Command::new(LABEL_PRINT).arg("picture", "x.png"),
    );

    let seen = Arc::new(Mutex::new(Vec::new()));
    let idle = Telemetry {
        state: MachineState::Idle,
        ..Telemetry::default()
    };
    let driver = PortDriver::new(labelmanager::LabelManager::with_port(script(
        &seen, None, idle,
    )));
    let mut host = Host::new(&driver);
    host.state("IDLE");
    assert_eq!(
        host.command(
            Caller::Queue,
            Command::new(LABEL_PRINT).arg("picture", "/tmp/l.png")
        ),
        Outcome::Ok
    );
    let Order::Job { order, file } = lock(&seen)[0].clone() else {
        panic!("a job")
    };
    // labelle's default width, 12 mm (dymo_labeler.py:244)
    assert_eq!((order.tape_mm, order.name.as_str()), (Some(12), "l.png"));
    assert_eq!(file, PathBuf::from("/tmp/l.png"));
    host.session.close();
}

struct NoPrinter;
impl crate::ports::dymo_cups::cups::usb_backend::UsbHost for NoPrinter {
    fn find_device(
        &self,
        _: &str,
        _: &str,
    ) -> Option<Arc<dyn crate::ports::dymo_cups::cups::usb_backend::UsbPrinterIo>> {
        None
    }
}

#[test]
fn labelwriter_450() {
    // the 450's own port, on a USB host with no printer: it sends nothing while idle (W5)
    let real = labelwriter::LabelWriter::lw450_on(Arc::new(NoPrinter));
    conforms(
        &PortDriver::new(real),
        &Command::new(LABEL_PRINT).arg("picture", "x.png"),
    );
    let mut host = Host::new(&PortDriver::new(labelwriter::LabelWriter::lw450_on(
        Arc::new(NoPrinter),
    )));
    host.state("IDLE");
    host.session.close();

    let seen = Arc::new(Mutex::new(Vec::new()));
    let idle = Telemetry {
        state: MachineState::Idle,
        ..Telemetry::default()
    };
    let driver = PortDriver::new(labelwriter::LabelWriter::with_port(script(
        &seen, None, idle,
    )));
    let mut host = Host::new(&driver);
    host.state("IDLE");
    assert_eq!(
        host.command(
            Caller::Queue,
            Command::new(LABEL_PRINT)
                .arg("picture", "/tmp/l.png")
                .arg("page_size", "w79h252")
        ),
        Outcome::Ok
    );
    let Order::Job { order, .. } = lock(&seen)[0].clone() else {
        panic!("a job")
    };
    assert_eq!(order.page_size.as_deref(), Some("w79h252"));
    // the driver's out-of-paper reason is OUT_OF_PAPER (lw450.ppd:47)
    let out = Telemetry {
        state: MachineState::Busy,
        problem: Some("Out of labels.".into()),
        ..Telemetry::default()
    };
    let m = labelwriter::LabelWriter::with_port(script(&seen, None, Telemetry::default()));
    assert_eq!(m.state_of(&out), Some(Some("OUT_OF_PAPER")));
    host.session.close();
}

#[test]
fn grbl_on_laserfoundrys_simulator() {
    let sim = || {
        let mut sim = maker_connect_grbl::SimulatedGrbl::new();
        sim.set_latency(Duration::from_millis(1));
        maker_connect_grbl::GrblMachine::new(Box::new(sim))
    };
    let driver = PortDriver::new(grbl::Grbl::on(Arc::new(sim)));
    conforms(&driver, &Command::new(LASER_HOME));
    assert!(
        !driver.baseline().has(LASER_FRAME),
        "frame waits for the port's job.frame"
    );

    let mut host = Host::new(&driver);
    host.state("IDLE");
    // `$X`, answered `ok` by the simulator: observed
    assert_eq!(
        host.command(Caller::Person, Command::new(LASER_UNLOCK)),
        Outcome::Ok
    );
    // the laser rule is the host's: Start from the queue never reaches the adapter
    let state = host
        .tree
        .get("/state")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let refused = check(
        &host.enabled,
        registry::state_set(KIND_GRBL),
        state.as_deref(),
        Caller::Queue,
        &Command::new(LASER_START).arg("program", "x.gcode"),
    )
    .unwrap_err();
    assert_eq!(refused.fault, &fault::PERSON_ONLY);
    host.session.close();

    let m = grbl::Grbl::with_port(Arc::new(|_| unreachable!()));
    assert_eq!(
        m.plan(
            &Command::new(LASER_JOG)
                .arg("x", "1")
                .arg("y", "-2")
                .arg("feed", "600"),
            1
        ),
        Ok(Planned::Command(Action::Jog {
            x: Some(1.0),
            y: Some(-2.0),
            z: None,
            feed: Some(600.0)
        }))
    );
}

#[test]
fn scale() {
    // usbscale's own port, with no HID device
    let real = scale::Scale::with_port(Arc::new(|_| {
        Box::new(crate::ports::usbscale::UsbScale::with_source(
            Box::new(|| Err("no HID in this test".to_owned())),
            None,
        ))
    }));
    conforms(&PortDriver::new(real), &Command::new("mc.scale.none"));

    let seen = Arc::new(Mutex::new(Vec::new()));
    let reading = Telemetry {
        scale: Some(ScaleReading {
            status: 4,
            status_text: String::new(),
            unit: 2,
            unit_name: "g".into(),
            exponent: 0,
            raw: 120,
            weight: 120.0,
        }),
        ..Telemetry::default()
    };
    let driver = PortDriver::new(scale::Scale::with_port(script(&seen, None, reading)));
    assert!(driver.baseline().is_empty());
    let mut host = Host::new(&driver);
    host.state("STABLE");
    host.settle();
    assert_eq!(host.tree.get("/report/scale/weight"), Some(&json!(120.0)));
    let names: Vec<&str> = (1..=9).map(scale::state_name).collect();
    let declared: Vec<&str> = registry::state_set(KIND_SCALE)
        .unwrap()
        .states
        .iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, declared);
    host.session.close();
}
