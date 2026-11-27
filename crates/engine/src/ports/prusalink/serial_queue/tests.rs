//! Tests of `serial_queue.rs`: PrusaLink's own queue and ours through the same operations
//! (`driver_serial_queue.py`), and PrusaLink's `tests/test_serial_queue.py` test by test.

use super::*;
use crate::ports::octoprint::comm::py_float_repr;
use crate::ports::prusalink::blinker::Receiver;
use crate::ports::prusalink::python;
use serde_json::{Value, json};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// What the queue logs: writes, closes, signals, threads.
type Log = Arc<Mutex<Vec<Value>>>;

/// A serial adapter that logs what is written and closed (as the driver's `Adapter`).
struct Adapter(Log);

impl SerialAdapter for Adapter {
    fn write(&self, data: &[u8]) -> Result<(), PyErr> {
        self.0.lock().unwrap().push(json!(["write", latin1(data)]));
        Ok(())
    }
    fn close(&self) {
        self.0.lock().unwrap().push(json!(["close"]));
    }
}

/// The driver's `Thread`: logs, starts nothing.
struct LoggedThreads(Log);

struct LoggedThread(Log, &'static str);

impl Joinable for LoggedThread {
    fn join(&self) -> Result<(), RuntimeError> {
        self.0.lock().unwrap().push(json!(["join", self.1]));
        Ok(())
    }
}

fn target_name(target: Target) -> (&'static str, Vec<bool>) {
    match target {
        Target::KeepSending => ("KeepSending", vec![]),
        Target::KeepMonitoring => ("KeepMonitoring", vec![]),
        Target::PrinterReconnected(a, b) => ("PrinterReconnected", vec![a, b]),
    }
}

impl Threads for LoggedThreads {
    fn start(
        &self,
        target: Target,
        daemon: bool,
        _run: Box<dyn FnOnce() + Send>,
    ) -> Result<Arc<dyn Joinable>, RuntimeError> {
        let (name, args) = target_name(target);
        let mut log = self.0.lock().unwrap();
        log.push(json!(["thread", name, args, daemon]));
        log.push(json!(["start", name]));
        Ok(Arc::new(LoggedThread(self.0.clone(), name)))
    }
}

/// The real `IsPlannerFed`, shared so the state can be read.
struct SharedPlanner(Arc<Mutex<IsPlannerFed>>);

impl Planner for SharedPlanner {
    fn call(&self) -> bool {
        self.0.lock().unwrap().call()
    }
    fn set_is_fed(&mut self, value: bool) {
        self.0.lock().unwrap().is_fed = value;
    }
    fn process_value(&mut self, value: f64) -> Result<(), PyErr> {
        self.0.lock().unwrap().process_value(value)
    }
    fn save(&mut self) -> Result<(), PyErr> {
        self.0.lock().unwrap().save()
    }
}

fn latin1(data: &[u8]) -> String {
    data.iter().map(|&b| b as char).collect()
}

fn py_error(error: &PyErr) -> Value {
    json!([error.class_name(), error.message()])
}

/// An instruction as the driver's `make` makes it: `[kind, message, to_checksum, capture?]`.
fn make(spec: &Value) -> Arc<Instruction> {
    let (kind, message, to_checksum) = (
        spec[0].as_str().unwrap(),
        spec[1].as_str().unwrap(),
        spec[2].as_bool().unwrap(),
    );
    Arc::new(match kind {
        "plain" => Instruction::new(message, to_checksum, None, None).unwrap(),
        _ => Instruction::matchable(
            message,
            to_checksum,
            Regex::new(spec[3].as_str().unwrap()).unwrap(),
            kind == "mandatory",
        )
        .unwrap(),
    })
}

fn instr(instruction: Option<&Arc<Instruction>>) -> Value {
    let Some(i) = instruction else {
        return Value::Null;
    };
    json!([
        i.message(),
        i.data().map(|d| latin1(&d)),
        i.number(),
        i.is_sent(),
        i.is_confirmed(),
        if i.kind == super::super::instruction::Kind::Instruction {
            Value::Null
        } else {
            json!(i.get_matches().len())
        }
    ])
}

/// Ours through one case, as `run_case` of the driver.
fn run_case(case: &Value) -> Vec<Value> {
    let log: Log = Arc::new(Mutex::new(vec![]));
    let clock = Arc::new(Mutex::new(1000.25));
    let serial = Condition::new("Port", "Serial device does not exist", None, None, 570).unwrap();
    let rpi = Condition::new(
        "RPIenabled",
        "RPi port is not enabled",
        Some(serial),
        None,
        560,
    )
    .unwrap();

    let folder = std::env::temp_dir().join(format!(
        "mc-serial-queue-{}-{}",
        std::process::id(),
        CASE.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("threshold");
    if let Some(threshold) = case["threshold"].as_str() {
        std::fs::write(&path, threshold).unwrap();
    }

    let parser = Arc::new(SerialParser::new());
    let planner = Arc::new(Mutex::new(
        IsPlannerFed::new(path.to_str().unwrap()).unwrap(),
    ));
    let c = clock.clone();
    let p = planner.clone();
    let parts = Parts {
        serial_adapter: Arc::new(Adapter(log.clone())),
        serial_parser: parser.clone(),
        planner: Box::new(move || Ok(Box::new(SharedPlanner(p)) as Box<dyn Planner>)),
        threads: Arc::new(LoggedThreads(log.clone())),
        clock: Arc::new(move || *c.lock().unwrap()),
        rpi_enabled: rpi,
        serial,
    };
    let monitored = case["monitored"].as_bool().unwrap();
    let queue = if monitored {
        SerialQueue::monitored(parts, None).unwrap()
    } else {
        SerialQueue::new(parts, None).unwrap()
    };
    let receivers: Vec<Arc<Receiver<Arc<SerialQueue>>>> = vec![
        {
            let log = log.clone();
            Arc::new(move |_: &Arc<SerialQueue>| log.lock().unwrap().push(json!(["failed"])))
        },
        {
            let log = log.clone();
            Arc::new(move |_: &Arc<SerialQueue>| log.lock().unwrap().push(json!(["confirmed"])))
        },
    ];
    queue.serial_queue_failed.connect(&receivers[0], false);
    queue
        .instruction_confirmed_signal
        .connect(&receivers[1], false);
    let number_changed: Arc<Receiver<i64>> = {
        let log = log.clone();
        Arc::new(move |n: &i64| log.lock().unwrap().push(json!(["number_changed", n])))
    };
    queue.message_number_changed.connect(&number_changed, false);

    let state = |queue: &SerialQueue| -> Value {
        let s = queue.state.lock().unwrap();
        let mut out = json!({
            "queue": s.queue.iter().map(|i| instr(Some(i))).collect::<Vec<_>>(),
            "priority_queue": s.priority_queue.iter().map(|i| instr(Some(i))).collect::<Vec<_>>(),
            "current": instr(s.current_instruction.as_ref()),
            "message_number": s.message_number,
            "send_history": s.send_history.iter().map(|i| instr(Some(i))).collect::<Vec<_>>(),
            "recovery_list": s.recovery_list.iter().map(|i| instr(Some(i))).collect::<Vec<_>>(),
            "rx_yeet_slot": instr(s.rx_yeet_slot.as_ref()),
            "m110_workaround_slot": instr(s.m110_workaround_slot.as_ref()),
            "worked_around_m110": s.worked_around_m110,
            "closed": s.closed,
            "has_failed": s.has_failed,
            "block_sending": s._block_sending,
            "stuck_counter": if monitored { json!(s.stuck_counter) } else { Value::Null },
            "last_event_on": if monitored { py_float_repr(s.last_event_on) } else { "None".to_owned() },
        });
        drop(s);
        let planner = planner.lock().unwrap();
        out["is_fed"] = json!(planner.call());
        out["planner_items"] = json!(planner.item_count());
        drop(planner);
        out["send_event"] = json!(queue.send_event.is_set());
        out["quit_evt"] = json!(queue.quit_evt.is_set());
        out["rpi"] = json!(rpi.state().name());
        out["can_write"] = json!(queue.can_write());
        out["is_empty"] = json!(queue.is_empty());
        out["peek_next"] = instr(queue.peek_next().as_ref());
        out["delay"] = if monitored {
            json!(py_float_repr(queue.get_current_delay()))
        } else {
            Value::Null
        };
        out["pairings"] = json!(parser.pairings());
        out
    };

    let take = |log: &Log| -> Vec<Value> { std::mem::take(&mut *log.lock().unwrap()) };
    let mut steps = vec![json!([Value::Null, take(&log), state(&queue)])];
    for op in case["ops"].as_array().unwrap() {
        let kind = op[0].as_str().unwrap();
        let q = queue.clone();
        let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<(), PyErr> {
            match kind {
                "enqueue" => q.enqueue_one(make(&op[1]), op[2].as_bool().unwrap()),
                "enqueue_list" => q.enqueue_list(
                    op[1].as_array().unwrap().iter().map(make).collect(),
                    op[2].as_bool().unwrap(),
                ),
                "send" => q._send()?,
                "line" => parser.decide(op[1].as_str().unwrap()),
                "confirm" => q._confirmed(op[1].as_bool().unwrap())?,
                "resend" => q._resend(op[1].as_i64().unwrap()),
                "yeet" => q._rx_got_yeeted()?,
                "flush_print" => q.flush_print_queue()?,
                "flush_queues" => q._flush_queues()?,
                "reset_number" => q.reset_message_number(),
                "set_number" => q.set_message_number(op[1].as_i64().unwrap()),
                "message_number" => {
                    q.state.lock().unwrap().message_number = op[1].as_i64().unwrap()
                }
                "replenish" => {
                    let messages: Vec<&str> = op[1]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|m| m.as_str().unwrap())
                        .collect();
                    q.replenish_history(&messages)?
                }
                "block" => q.block_sending(),
                "unblock" => q.unblock_sending(),
                "clear_send_event" => q.send_event.clear(),
                "close" => q.state.lock().unwrap().closed = true,
                "has_failed" => q.state.lock().unwrap().has_failed = true,
                "rx_max" => q.state.lock().unwrap().rx_max = op[1].as_i64().unwrap(),
                "reconnected" => {
                    q._printer_reconnected(op[1].as_bool().unwrap(), op[2].as_bool().unwrap())?
                }
                "printer_reconnected" => q
                    .printer_reconnected(op[1].as_bool().unwrap(), op[2].as_bool().unwrap())
                    .unwrap(),
                "clock" => *clock.lock().unwrap() = op[1].as_f64().unwrap(),
                "serial" => serial
                    .set_state(match op[1].as_str().unwrap() {
                        "OK" => CondState::Ok,
                        _ => CondState::Nok,
                    })
                    .unwrap(),
                "check_status" => q.check_status()?,
                "stop" => q.stop()?,
                "wait_stopped" => q.wait_stopped().unwrap(),
                other => panic!("unknown operation {other}"),
            }
            Ok(())
        }));
        let result = match outcome {
            Ok(Ok(())) => Value::Null,
            Ok(Err(error)) => py_error(&error),
            Err(panic) => json!([
                "panic",
                panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default()
            ]),
        };
        steps.push(json!([result, take(&log), state(&queue)]));
    }
    let _ = std::fs::remove_dir_all(&folder);
    steps
}

static CASE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn plain(message: &str) -> Value {
    json!(["plain", message, false])
}

fn numbered(message: &str) -> Value {
    json!(["plain", message, true])
}

fn case(monitored: bool, threshold: Option<&str>, ops: Vec<Value>) -> Value {
    json!({ "monitored": monitored, "threshold": threshold, "ops": ops })
}

/// The cases: PrusaLink's own tests as operations, then what they leave out.
fn cases() -> Vec<Value> {
    let ok = || json!(["line", "ok"]);
    let send = || json!(["send"]);
    vec![
        // order: priority first, otherwise first in first out
        case(
            false,
            None,
            vec![
                json!(["enqueue", plain("G1"), false]),
                json!(["enqueue", plain("G2"), false]),
                json!(["enqueue", plain("M105"), true]),
                send(),
                ok(),
                send(),
                ok(),
                send(),
                ok(),
                ok(),
            ],
        ),
        // a whole list, to the front and to the back
        case(
            false,
            None,
            vec![
                json!(["enqueue_list", [plain("G1"), plain("G2")], true]),
                json!(["enqueue_list", [plain("G3"), numbered("G4")], false]),
                send(),
                ok(),
                send(),
                ok(),
                send(),
                ok(),
                send(),
                ok(),
            ],
        ),
        // a fed planner lets the unimportant one through
        case(
            false,
            Some("-1"),
            vec![
                json!(["enqueue", numbered("G1 X1"), false]),
                send(),
                ok(),
                json!(["enqueue", plain("G1"), false]),
                json!(["enqueue", plain("M105"), true]),
                send(),
                ok(),
                send(),
                ok(),
            ],
        ),
        // busy: no second send, closed, blocked and unblocked
        case(
            false,
            None,
            vec![
                json!(["enqueue", plain("G1"), false]),
                json!(["enqueue", plain("G2"), false]),
                send(),
                send(),
                json!(["close"]),
                json!(["block"]),
                json!(["clear_send_event"]),
                json!(["unblock"]),
                json!(["clear_send_event"]),
                json!(["unblock"]),
            ],
        ),
        // numbered messages, checksums, history, planner values
        case(
            false,
            None,
            vec![
                json!(["enqueue", numbered("G1 X1"), false]),
                json!(["enqueue", numbered("G1 X2"), false]),
                send(),
                ok(),
                send(),
                ok(),
            ],
        ),
        // the number wraps around at MAX_INT with an M110 N0 in front
        case(
            false,
            None,
            vec![
                json!(["message_number", (1i64 << 31) - 3]),
                json!(["enqueue", numbered("G1"), false]),
                json!(["enqueue", numbered("G2"), false]),
                send(),
                ok(),
                send(),
                ok(),
                send(),
                ok(),
                send(),
                ok(),
            ],
        ),
        // M110 gets an M400 first; its number
        case(
            false,
            None,
            vec![
                json!(["enqueue", plain("M110 N5"), false]),
                send(),
                ok(),
                send(),
                ok(),
                json!(["enqueue", plain("M110 N"), false]),
                send(),
                ok(),
                send(),
                ok(),
                json!(["enqueue", plain("M110 N-7"), false]),
                send(),
                ok(),
                send(),
                ok(),
                json!(["enqueue", plain("N3 M110 N12"), false]),
                send(),
                ok(),
                send(),
                ok(),
            ],
        ),
        // too long is only a warning
        case(
            false,
            None,
            vec![
                json!(["rx_max", 3]),
                json!(["enqueue", plain("G28 X Y Z"), false]),
                send(),
                ok(),
            ],
        ),
        // capturing: matchable and mandatory, a refused confirmation, force
        case(
            false,
            None,
            vec![
                json!(["enqueue", ["matchable", "M105", false, r"^T:.*"], false]),
                send(),
                json!(["line", "T:20.0 E:0 B:20.0"]),
                json!(["line", "something else"]),
                ok(),
                json!([
                    "enqueue",
                    ["mandatory", "M115", false, r"^FIRMWARE.*"],
                    false
                ]),
                send(),
                ok(),
                json!(["confirm", true]),
                json!([
                    "enqueue",
                    ["mandatory", "M115", false, r"^FIRMWARE.*"],
                    false
                ]),
                send(),
                json!(["line", "FIRMWARE_NAME:Prusa"]),
                ok(),
            ],
        ),
        // unexpected confirmations
        case(
            false,
            None,
            vec![ok(), json!(["confirm", false]), json!(["confirm", true])],
        ),
        // message numbers: set, reset, replenished
        case(
            false,
            None,
            vec![
                json!(["set_number", 42]),
                json!(["enqueue", plain("M105"), true]),
                json!(["reset_number"]),
                json!(["message_number", 10]),
                json!(["replenish", ["G1", "G2", "G3"]]),
                send(),
                ok(),
                send(),
                ok(),
            ],
        ),
        // resends: of the past, of the future, while unnumbered, impossible
        case(
            false,
            None,
            vec![
                json!(["enqueue", numbered("G1 X0"), false]),
                json!(["enqueue", numbered("G1 X1"), false]),
                json!(["enqueue", numbered("G1 X2"), false]),
                json!(["enqueue", numbered("G1 X3"), false]),
                json!(["enqueue", numbered("G1 X4"), false]),
                send(),
                ok(),
                send(),
                ok(),
                send(),
                ok(),
                send(),
                ok(),
                send(),
                ok(),
                json!(["line", "Resend: 4"]),
                send(),
                ok(),
                send(),
                ok(),
                json!(["line", "Resend: 9"]),
                json!(["enqueue", plain("M105"), false]),
                send(),
                json!(["line", "Resend:3"]),
                ok(),
                send(),
                ok(),
                send(),
                ok(),
                json!(["resend", 50]),
                json!(["resend", 0]),
            ],
        ),
        // resends at the edges of the history: all of it, all but one, the first number
        case(
            false,
            None,
            vec![
                json!(["enqueue", numbered("G1 X0"), false]),
                json!(["enqueue", numbered("G1 X1"), false]),
                json!(["enqueue", numbered("G1 X2"), false]),
                send(),
                ok(),
                send(),
                ok(),
                send(),
                ok(),
                json!(["resend", 3]),
                json!(["resend", 2]),
                json!(["line", "Resend: 1"]),
                json!(["line", "Resend: 2"]),
                json!(["line", "Resend: 3"]),
                json!(["line", "Resend: 0"]),
            ],
        ),
        // yeeted RX buffers
        case(
            false,
            None,
            vec![
                json!(["yeet"]),
                json!(["enqueue", ["matchable", "M105", false, r"^T:.*"], false]),
                send(),
                json!(["yeet"]),
                json!(["line", "T:1.0 E:0 B:1.0"]),
                ok(),
            ],
        ),
        // flushing the print queue
        case(
            false,
            None,
            vec![
                json!(["enqueue", numbered("G1"), true]),
                json!(["enqueue", plain("M105"), true]),
                json!(["enqueue", plain("G2"), false]),
                json!(["flush_print"]),
                json!(["enqueue", numbered("G3"), false]),
                send(),
                json!(["flush_print"]),
            ],
        ),
        // flushing every queue
        case(
            false,
            None,
            vec![
                json!(["enqueue", plain("G1"), false]),
                json!(["enqueue", plain("G2"), false]),
                json!(["enqueue", plain("M105"), true]),
                send(),
                json!(["flush_queues"]),
                json!(["flush_queues"]),
            ],
        ),
        // reconnected while printing, after a power panic, after a failure, idle
        case(
            false,
            None,
            vec![
                json!(["close"]),
                json!(["enqueue", plain("G1"), false]),
                json!(["enqueue", plain("G2"), false]),
                send(),
                json!(["reconnected", true, false]),
                json!(["reconnected", true, true]),
                json!(["has_failed"]),
                json!(["reconnected", false, false]),
                json!(["reconnected", false, false]),
                json!(["printer_reconnected", true, false]),
                json!(["stop"]),
                json!(["wait_stopped"]),
            ],
        ),
        // monitored: signs of life, delays, time-outs, the stuck serial, stop
        case(
            true,
            None,
            vec![
                json!(["clock", 1001.0]),
                json!(["enqueue", plain("G1"), false]),
                json!(["clock", 1005.5]),
                send(),
                json!(["clock", 1040.0]),
                json!(["check_status"]),
                json!(["serial", "OK"]),
                json!(["check_status"]),
                json!(["clock", 1080.0]),
                json!(["check_status"]),
                json!(["clock", 1120.0]),
                json!(["check_status"]),
                json!(["clock", 1121.0]),
                json!(["line", "echo:busy: processing"]),
                json!(["clock", 1122.0]),
                json!(["line", "echo:busy: paused for user"]),
                json!(["line", "T:20.0 E:0 B:20.0"]),
                json!(["line", "T:20.0 E:0 W:?"]),
                json!(["clock", 1130.0]),
                ok(),
                json!(["clock", 1200.0]),
                json!(["check_status"]),
                json!(["serial", "NOK"]),
                json!(["enqueue", plain("G2"), false]),
                send(),
                json!(["clock", 1300.0]),
                json!(["check_status"]),
                json!(["stop"]),
                json!(["wait_stopped"]),
            ],
        ),
        // monitored with a full planner queue saves its threshold on stop
        case(true, None, {
            let mut ops = vec![];
            for i in 0..110 {
                ops.push(json!(["enqueue", numbered(&format!("G1 X{i}")), false]));
                ops.push(send());
                ops.push(ok());
            }
            ops.push(json!(["stop"]));
            ops
        }),
    ]
}

/// PrusaLink's own `SerialQueue` and `MonitoredSerialQueue` and ours through the same operations:
/// PrusaLink's tests as operations, and lines fed to the real parser (confirmations, resends,
/// captured output, signs of life), the clock set by the case, threads logged and not started.
/// After each operation: the same error, the same writes, closes, signals and threads in the same
/// order, and the same state (the queues, the slots, the history, the numbers, the flags, the
/// planner's `is_fed` and count, `RPI_ENABLED`, the parser's pairings).
#[test]
fn identical_to_prusalink() {
    let cases = cases();
    let Some(theirs) = python::run(
        include_str!("../driver_serial_queue.py"),
        &json!({ "cases": cases }),
    ) else {
        return;
    };
    for (number, (case, their)) in cases.iter().zip(theirs.as_array().unwrap()).enumerate() {
        let ours = run_case(case);
        let their = their.as_array().unwrap();
        assert_eq!(their.len(), ours.len());
        for (step, (t, o)) in their.iter().zip(&ours).enumerate() {
            let op = if step == 0 {
                json!("__init__")
            } else {
                case["ops"][step - 1].clone()
            };
            assert_eq!(t, o, "case {number} step {step}: {op}");
        }
    }
}

mod prusalink_tests {
    //! PrusaLink's `tests/test_serial_queue.py` (`reference/Prusa-Link`, `171148ce`, Prusa's commit
    //! `f256553`), test by test. Its fixtures patch the module's `RPI_ENABLED` (a mock), `SERIAL`
    //! (`True`), `InterestingLogRotator`, `IsPlannerFed` and `Thread` (mocks), and give the queue
    //! a `MagicMock` adapter and parser: here recorders of the same calls ([`Parts`]), fresh
    //! conditions with `SERIAL` at `OK`. The assertions on `logs.trigger` are not ported (the
    //! rotation of the log files is not covered, module doc).

    use super::super::*;
    use crate::ports::prusalink::blinker::Receiver;
    use crate::ports::prusalink::instruction::Match;
    use crate::ports::prusalink::serial_parser::RegexPairing;
    use crate::ports::python::py_threading::time_sleep;

    /// What the mocks recorded.
    #[derive(Default)]
    pub struct Calls {
        /// `serial_adapter.write.call_args_list`
        pub written: Vec<Vec<u8>>,
        /// `serial_adapter.close` calls
        pub closed: usize,
        /// `serial_adapter.write.side_effect`: the class of the error to raise
        pub write_error: Option<&'static str>,
        /// `serial_parser.add_handler.call_args_list`: the pattern, the handler, the priority
        pub added: Vec<(String, Handler, f64)>,
        /// `serial_parser.remove_handler.call_args_list`
        pub removed: Vec<(String, Handler)>,
        /// `planner.return_value.return_value`: what `is_planner_fed()` answers
        pub planner_fed: bool,
        /// `planner.return_value.is_fed`, once set
        pub is_fed: Option<bool>,
        /// `planner.return_value.process_value.call_args_list`
        pub processed: Vec<f64>,
        /// `planner.return_value.save` calls
        pub saved: usize,
        /// `Thread(...)` calls: the target and its daemon flag
        pub threads: Vec<(Target, bool)>,
        /// `thread.join()` calls, by target
        pub joined: Vec<Target>,
    }

    pub type Shared = Arc<Mutex<Calls>>;

    struct MockAdapter(Shared);
    impl SerialAdapter for MockAdapter {
        fn write(&self, data: &[u8]) -> Result<(), PyErr> {
            let mut calls = self.0.lock().unwrap();
            calls.written.push(data.to_vec());
            match calls.write_error {
                Some(class) => Err(PyErr::Other(class, "gone".to_owned())),
                None => Ok(()),
            }
        }
        fn close(&self) {
            self.0.lock().unwrap().closed += 1;
        }
    }

    struct MockParser(Shared);
    impl Parser for MockParser {
        fn add_handler(
            &self,
            regexp: &Regex,
            handler: &Handler,
            priority: f64,
        ) -> Result<(), PyErr> {
            let entry = (regexp.as_str().to_owned(), handler.clone(), priority);
            self.0.lock().unwrap().added.push(entry);
            Ok(())
        }
        fn remove_handler(&self, regexp: &Regex, handler: &Handler) -> Result<(), PyErr> {
            let entry = (regexp.as_str().to_owned(), handler.clone());
            self.0.lock().unwrap().removed.push(entry);
            Ok(())
        }
    }

    struct MockPlanner(Shared);
    impl Planner for MockPlanner {
        fn call(&self) -> bool {
            self.0.lock().unwrap().planner_fed
        }
        fn set_is_fed(&mut self, value: bool) {
            self.0.lock().unwrap().is_fed = Some(value);
        }
        fn process_value(&mut self, value: f64) -> Result<(), PyErr> {
            self.0.lock().unwrap().processed.push(value);
            Ok(())
        }
        fn save(&mut self) -> Result<(), PyErr> {
            self.0.lock().unwrap().saved += 1;
            Ok(())
        }
    }

    struct MockThreads(Shared);
    struct MockThread(Shared, Target);
    impl Joinable for MockThread {
        fn join(&self) -> Result<(), RuntimeError> {
            self.0.lock().unwrap().joined.push(self.1);
            Ok(())
        }
    }
    impl Threads for MockThreads {
        fn start(
            &self,
            target: Target,
            daemon: bool,
            _run: Box<dyn FnOnce() + Send>,
        ) -> Result<Arc<dyn Joinable>, RuntimeError> {
            self.0.lock().unwrap().threads.push((target, daemon));
            Ok(Arc::new(MockThread(self.0.clone(), target)))
        }
    }

    /// The fixtures: the queue, what its mocks recorded, `RPI_ENABLED`.
    pub struct Fixture {
        pub queue: Arc<SerialQueue>,
        pub calls: Shared,
        pub rpi: Condition,
    }

    fn parts(calls: &Shared) -> (Parts, Condition) {
        let serial =
            Condition::new("Port", "Serial device does not exist", None, None, 570).unwrap();
        let rpi = Condition::new(
            "RPIenabled",
            "RPi port is not enabled",
            Some(serial),
            None,
            560,
        )
        .unwrap();
        // `patch.object(module, "SERIAL", True)`
        serial.set_state(CondState::Ok).unwrap();
        let planner_calls = calls.clone();
        let parts = Parts {
            serial_adapter: Arc::new(MockAdapter(calls.clone())),
            serial_parser: Arc::new(MockParser(calls.clone())),
            planner: Box::new(move || Ok(Box::new(MockPlanner(planner_calls)) as Box<dyn Planner>)),
            threads: Arc::new(MockThreads(calls.clone())),
            clock: Arc::new(crate::ports::prusalink::util::time),
            rpi_enabled: rpi,
            serial,
        };
        (parts, rpi)
    }

    /// `queue`: Queue with the mocked adapter and parser, nothing runs on its own
    pub fn queue() -> Fixture {
        let calls = Shared::default();
        let (parts, rpi) = parts(&calls);
        Fixture {
            queue: SerialQueue::new(parts, None).unwrap(),
            calls,
            rpi,
        }
    }

    /// `monitored`: The monitored queue
    pub fn monitored() -> Fixture {
        let calls = Shared::default();
        let (parts, rpi) = parts(&calls);
        Fixture {
            queue: SerialQueue::monitored(parts, None).unwrap(),
            calls,
            rpi,
        }
    }

    /// `written(queue)`: Lines written to the serial adapter
    pub fn written(fixture: &Fixture) -> Vec<Vec<u8>> {
        fixture.calls.lock().unwrap().written.clone()
    }

    /// `send_all(queue)`: Sends and confirms everything which is queued
    pub fn send_all(queue: &SerialQueue) {
        while !queue.is_empty() {
            queue._send().unwrap();
            queue._confirmed(false).unwrap();
        }
    }

    /// `Instruction(message)`
    pub fn instruction(message: &str) -> Arc<Instruction> {
        Arc::new(Instruction::new(message, false, None, None).unwrap())
    }

    /// `Instruction(message, to_checksum=True)`
    pub fn numbered_instruction(message: &str) -> Arc<Instruction> {
        Arc::new(Instruction::new(message, true, None, None).unwrap())
    }

    /// What a handler gets from the parser: `sender` a pairing, and the match, if any.
    pub fn fired(r#match: Option<Arc<Match>>) -> Fired {
        Fired {
            sender: Arc::new(RegexPairing::new(Regex::new("").unwrap(), 0.0)),
            r#match,
        }
    }

    /// `module.RESEND_REGEX.match(line)`
    pub fn resend_match(line: &str) -> Option<Arc<Match>> {
        re_match(&RESEND_REGEX, line).map(|c| Arc::new(Match::new(&RESEND_REGEX, &c)))
    }

    fn messages(instructions: &VecDeque<Arc<Instruction>>) -> Vec<String> {
        instructions.iter().map(|i| i.message()).collect()
    }

    // --- basics ---

    /// `test_registers_handlers`: Confirmation has the highest priority
    #[test]
    fn test_registers_handlers() {
        let f = queue();
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.added[0].0, CONFIRMATION_REGEX.as_str());
        assert_eq!(calls.added[0].2, f64::INFINITY);
        assert_eq!(calls.added[1].0, RESEND_REGEX.as_str());
    }

    /// `test_empty_queue`: Nothing to send
    #[test]
    fn test_empty_queue() {
        let f = queue();
        assert!(f.queue.is_empty());
        assert!(f.queue.peek_next().is_none());
        assert!(!f.queue.can_write());
    }

    /// `test_order_of_instructions`: Priority queue first, otherwise first in first out
    #[test]
    fn test_order_of_instructions() {
        let f = queue();
        let (first, second, urgent) = (instruction("G1"), instruction("G2"), instruction("M105"));
        f.queue.enqueue_one(first, false);
        f.queue.enqueue_one(second, false);
        f.queue.enqueue_one(urgent.clone(), true);
        assert!(!f.queue.is_empty());
        assert!(Arc::ptr_eq(&f.queue.peek_next().unwrap(), &urgent));
        send_all(&f.queue);
        assert_eq!(
            written(&f),
            [b"M105\n".to_vec(), b"G1\n".to_vec(), b"G2\n".to_vec()]
        );
    }

    /// `test_enqueue_list`: Whole list stays together, in order
    #[test]
    fn test_enqueue_list() {
        let f = queue();
        f.queue
            .enqueue_list(vec![instruction("G1"), instruction("G2")], true);
        send_all(&f.queue);
        assert_eq!(written(&f), [b"G1\n".to_vec(), b"G2\n".to_vec()]);
        assert!(f.queue.send_event.is_set());
    }

    /// `test_planner_fed_lets_unimportant_instruction_through`: A full planner buffer is the time
    /// for the low priority ones
    #[test]
    fn test_planner_fed_lets_unimportant_instruction_through() {
        let f = queue();
        f.calls.lock().unwrap().planner_fed = true;
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue.enqueue_one(instruction("M105"), true);
        assert_eq!(f.queue.peek_next().unwrap().message(), "G1");
        f.queue._send().unwrap();
        let current = f
            .queue
            .state
            .lock()
            .unwrap()
            .current_instruction
            .clone()
            .unwrap();
        assert_eq!(current.message(), "G1");
        assert_eq!(f.calls.lock().unwrap().is_fed, Some(false));
    }

    /// `test_cannot_send_while_busy`: The next instruction waits for the confirmation
    #[test]
    fn test_cannot_send_while_busy() {
        let f = queue();
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue.enqueue_one(instruction("G2"), false);
        f.queue._send().unwrap();
        assert!(!f.queue.can_write());
        let Err(error) = f.queue._next_instruction() else {
            panic!("RuntimeError expected");
        };
        assert_eq!(error.class_name(), "RuntimeError");
    }

    /// `test_closed_queue_does_not_write`: Closed queue is silent
    #[test]
    fn test_closed_queue_does_not_write() {
        let f = queue();
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue.state.lock().unwrap().closed = true;
        assert!(!f.queue.can_write());
    }

    /// `test_block_and_unblock`: Blocking sending nudges the sender only when unblocking
    #[test]
    fn test_block_and_unblock() {
        let f = queue();
        f.queue.block_sending();
        f.queue.send_event.clear();
        f.queue.unblock_sending();
        assert!(f.queue.send_event.is_set());
        f.queue.send_event.clear();
        f.queue.unblock_sending();
        assert!(!f.queue.send_event.is_set());
    }

    /// `test_stop_wakes_the_sender`: Stop sets the events
    #[test]
    fn test_stop_wakes_the_sender() {
        let f = queue();
        f.queue.stop().unwrap();
        assert!(f.queue.quit_evt.is_set());
        assert!(f.queue.send_event.is_set());
        f.queue.wait_stopped().unwrap();
        assert_eq!(f.calls.lock().unwrap().joined, [Target::KeepSending]);
    }

    /// Runs `_keep_sending` in a thread and ends it with `stop()` once `until` holds or after a
    /// while. Differs: the source's tests replace `send_event` with a mock whose `wait` sets
    /// `quit_evt` on its second call; the event here is the queue's own, so the loop is ended
    /// from outside.
    fn keep_sending_until(f: &Fixture, until: impl Fn(&Calls) -> bool) {
        let queue = f.queue.clone();
        let sender = std::thread::spawn(move || queue._keep_sending());
        for _ in 0..100 {
            if until(&f.calls.lock().unwrap()) {
                break;
            }
            time_sleep(0.01);
        }
        f.queue.stop().unwrap();
        sender.join().expect("the sender ends without an error");
    }

    /// `test_keep_sending`: The sender thread sends one instruction per nudge
    #[test]
    fn test_keep_sending() {
        let f = queue();
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue.send_event.set();
        keep_sending_until(&f, |calls| !calls.written.is_empty());
        assert_eq!(written(&f), [b"G1\n".to_vec()]);
    }

    /// `test_keep_sending_survives_write_errors`: A failing write is left for the serial reader
    /// to fix
    #[test]
    fn test_keep_sending_survives_write_errors() {
        let f = queue();
        f.queue.enqueue_one(instruction("G1"), false);
        f.calls.lock().unwrap().write_error = Some("SerialException");
        keep_sending_until(&f, |calls| !calls.written.is_empty());
    }

    /// `test_keep_sending_blocked`: Blocked queue sends nothing
    #[test]
    fn test_keep_sending_blocked() {
        let f = queue();
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue.block_sending();
        keep_sending_until(&f, |_| false);
        assert!(written(&f).is_empty());
    }

    // --- sending ---

    /// `test_numbered_messages`: Print instructions get numbers, checksums and enter the history
    #[test]
    fn test_numbered_messages() {
        let f = queue();
        f.queue.enqueue_one(numbered_instruction("G1 X1"), false);
        f.queue.enqueue_one(numbered_instruction("G1 X2"), false);
        send_all(&f.queue);
        let data = written(&f);
        assert!(data[0].starts_with(b"N1 G1 X1 *"));
        assert!(data[1].starts_with(b"N2 G1 X2 *"));
        assert_eq!(f.queue.state.lock().unwrap().message_number, 2);
        assert_eq!(f.queue.state.lock().unwrap().send_history.len(), 2);
        assert!(!f.calls.lock().unwrap().processed.is_empty());
    }

    /// `test_message_number_wraps_around`: The maximum number is reset by M110
    #[test]
    fn test_message_number_wraps_around() {
        let f = queue();
        f.queue.state.lock().unwrap().message_number = MAX_INT - 1;
        f.queue.enqueue_one(numbered_instruction("G1"), false);
        f.queue._send().unwrap();
        let last = f
            .queue
            .state
            .lock()
            .unwrap()
            .priority_queue
            .back()
            .cloned()
            .unwrap();
        assert_eq!(last.message(), "M110 N0");
    }

    /// `test_m110_gets_a_g4_workaround`: M110 is preceded by M400 to sync the firmware buffers
    #[test]
    fn test_m110_gets_a_g4_workaround() {
        let f = queue();
        f.queue.enqueue_one(instruction("M110 N5"), false);
        let changed = Arc::new(Mutex::new(0));
        let count = changed.clone();
        let receiver: Arc<Receiver<i64>> = Arc::new(move |_: &i64| *count.lock().unwrap() += 1);
        f.queue.message_number_changed.connect(&receiver, false);
        f.queue._send().unwrap();
        assert_eq!(current(&f).message(), "M400");
        f.queue._confirmed(false).unwrap();
        f.queue._send().unwrap();
        assert_eq!(current(&f).message(), "M110 N5");
        assert_eq!(f.queue.state.lock().unwrap().message_number, 5);
        assert_eq!(*changed.lock().unwrap(), 1);
        assert!(!f.queue.state.lock().unwrap().worked_around_m110);
    }

    /// `queue.current_instruction`
    fn current(f: &Fixture) -> Arc<Instruction> {
        f.queue
            .state
            .lock()
            .unwrap()
            .current_instruction
            .clone()
            .unwrap()
    }

    /// `test_m110_without_a_number`: M110 with an empty number resets the number to zero
    #[test]
    fn test_m110_without_a_number() {
        let f = queue();
        f.queue.state.lock().unwrap().worked_around_m110 = true;
        f.queue.state.lock().unwrap().message_number = 9;
        f.queue.enqueue_one(instruction("M110 N"), false);
        f.queue._send().unwrap();
        assert_eq!(f.queue.state.lock().unwrap().message_number, 0);
    }

    /// `test_too_long_message_is_only_a_warning`: The queue sends it anyway
    #[test]
    fn test_too_long_message_is_only_a_warning() {
        let f = queue();
        f.queue.state.lock().unwrap().rx_max = 3;
        f.queue.enqueue_one(instruction("G28 X Y Z"), false);
        f.queue._send().unwrap();
        assert_eq!(written(&f), [b"G28 X Y Z\n".to_vec()]);
    }

    /// `test_output_capture`: Capturing instructions are hooked up while they are current. The
    /// handler is `instruction.output_captured`: the same one is removed (difference 6).
    #[test]
    fn test_output_capture() {
        let f = queue();
        let instruction = Arc::new(
            Instruction::matchable(
                "M105",
                false,
                crate::ports::prusalink::instruction::any_line(),
                false,
            )
            .unwrap(),
        );
        f.queue.enqueue_one(instruction.clone(), false);
        f.queue._send().unwrap();
        let (pattern, added, _) = f.calls.lock().unwrap().added.last().cloned().unwrap();
        assert_eq!(
            pattern,
            instruction.capture_matching.as_ref().unwrap().as_str()
        );
        f.queue._confirmed(false).unwrap();
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.removed.len(), 1);
        assert_eq!(calls.removed[0].0, pattern);
        assert!(Arc::ptr_eq(&calls.removed[0].1, &added));
        // the handler is the instruction's: a line it gets is captured
        drop(calls);
        added(&fired(Some(Arc::new(Match {
            groups: vec![Some("T:1".to_owned())],
            names: vec![None],
        }))));
        assert_eq!(instruction.get_matches().len(), 1);
    }

    /// `test_confirmation_signal_and_state`: A confirmation is announced and proves the port
    /// works
    #[test]
    fn test_confirmation_signal_and_state() {
        let f = queue();
        let confirmed = Arc::new(Mutex::new(0));
        let count = confirmed.clone();
        let receiver: Arc<Receiver<Arc<SerialQueue>>> =
            Arc::new(move |_: &Arc<SerialQueue>| *count.lock().unwrap() += 1);
        f.queue
            .instruction_confirmed_signal
            .connect(&receiver, false);
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue._send().unwrap();
        f.queue._confirmed(false).unwrap();
        assert_eq!(*confirmed.lock().unwrap(), 1);
        assert!(f.queue.state.lock().unwrap().current_instruction.is_none());
        assert_eq!(f.rpi.state(), CondState::Ok);
    }

    /// `test_unexpected_confirmation_is_ignored`: Confirmation without a sent instruction
    #[test]
    fn test_unexpected_confirmation_is_ignored() {
        let f = queue();
        f.queue._confirmed(false).unwrap();
        assert!(f.queue.state.lock().unwrap().current_instruction.is_none());
    }

    /// `test_instruction_refusing_confirmation`: A matchable instruction without a match is not
    /// confirmed (`global_conditions.logs.trigger.assert_called_once()` not ported)
    #[test]
    fn test_instruction_refusing_confirmation() {
        let f = queue();
        let instruction = Arc::new(
            Instruction::matchable(
                "M105",
                false,
                crate::ports::prusalink::instruction::any_line(),
                true,
            )
            .unwrap(),
        );
        f.queue.enqueue_one(instruction.clone(), false);
        f.queue._send().unwrap();
        f.queue._confirmed(false).unwrap();
        assert!(Arc::ptr_eq(&current(&f), &instruction));
        f.queue._confirmed(true).unwrap();
        assert!(f.queue.state.lock().unwrap().current_instruction.is_none());
    }

    /// `test_confirmation_handler`: The regular expression handler confirms
    #[test]
    fn test_confirmation_handler() {
        let f = queue();
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue._send().unwrap();
        f.queue._confirmation_handler(&fired(None));
        assert!(f.queue.state.lock().unwrap().current_instruction.is_none());
    }

    // --- message numbers ---

    /// `test_set_message_number`: The number can be set for the power panic recovery
    #[test]
    fn test_set_message_number() {
        let f = queue();
        f.queue.set_message_number(42);
        assert_eq!(f.queue.state.lock().unwrap().message_number, 42);
    }

    /// `test_reset_message_number`: M110 N0 gets in front
    #[test]
    fn test_reset_message_number() {
        let f = queue();
        f.queue.enqueue_one(instruction("M105"), true);
        f.queue.reset_message_number();
        assert_eq!(
            messages(&f.queue.state.lock().unwrap().priority_queue),
            ["M110 N0", "M105"]
        );
    }

    /// `test_replenish_history`: The history is rebuilt from the messages
    #[test]
    fn test_replenish_history() {
        let f = queue();
        f.queue.state.lock().unwrap().message_number = 10;
        f.queue.replenish_history(&["G1", "G2", "G3"]).unwrap();
        let state = f.queue.state.lock().unwrap();
        let numbers: Vec<Option<i64>> = state.send_history.iter().map(|i| i.number()).collect();
        assert_eq!(numbers, [Some(8), Some(9), Some(10)]);
        assert!(
            state.send_history[0]
                .data()
                .unwrap()
                .starts_with(b"N8 G1 *")
        );
    }

    // --- resends ---

    /// `numbered(queue, count)`: Sends and confirms count numbered instructions
    fn numbered(f: &Fixture, count: usize) {
        for i in 0..count {
            f.queue
                .enqueue_one(numbered_instruction(&format!("G1 X{i}")), false);
        }
        send_all(&f.queue);
    }

    fn recovery_numbers(f: &Fixture) -> Vec<Option<i64>> {
        f.queue
            .state
            .lock()
            .unwrap()
            .recovery_list
            .iter()
            .map(|i| i.number())
            .collect()
    }

    /// `test_resend_requested`: The old instructions are queued again for the recovery
    #[test]
    fn test_resend_requested() {
        let f = queue();
        numbered(&f, 5);
        f.queue._resend_handler(&fired(resend_match("Resend: 4")));
        assert_eq!(recovery_numbers(&f), [Some(5), Some(4)]);
        let last = f
            .queue
            .state
            .lock()
            .unwrap()
            .recovery_list
            .last()
            .cloned()
            .unwrap();
        assert!(Arc::ptr_eq(&f.queue.peek_next().unwrap(), &last));
        assert_eq!(f.queue.peek_next().unwrap().number(), Some(4));
        f.queue._send().unwrap();
        assert!(written(&f).last().unwrap().starts_with(b"N4 "));
    }

    /// `test_resend_of_the_future`: A number we have not sent is ignored
    #[test]
    fn test_resend_of_the_future() {
        let f = queue();
        numbered(&f, 2);
        f.queue._resend_handler(&fired(resend_match("Resend: 9")));
        assert!(f.queue.state.lock().unwrap().recovery_list.is_empty());
    }

    /// `test_resend_for_unnumbered_message_recovers_rx`: A resend while sending an unnumbered one
    /// means a lost buffer
    #[test]
    fn test_resend_for_unnumbered_message_recovers_rx() {
        let f = queue();
        numbered(&f, 3);
        f.queue.enqueue_one(instruction("M105"), false);
        f.queue._send().unwrap();
        f.queue._resend_handler(&fired(resend_match("Resend: 3")));
        assert_eq!(written(&f).last().unwrap(), b"M105\n"); // sent again
        assert!(!f.queue.state.lock().unwrap().recovery_list.is_empty());
    }

    /// `test_impossible_resend`: Asking for more than we remember is fatal
    #[test]
    fn test_impossible_resend() {
        let f = queue();
        let failed = Arc::new(Mutex::new(0));
        let count = failed.clone();
        let receiver: Arc<Receiver<Arc<SerialQueue>>> =
            Arc::new(move |_: &Arc<SerialQueue>| *count.lock().unwrap() += 1);
        f.queue.serial_queue_failed.connect(&receiver, false);
        numbered(&f, 2);
        f.queue._resend(50);
        assert_eq!(*failed.lock().unwrap(), 1);
        assert!(f.queue.state.lock().unwrap().has_failed);
    }

    /// `test_rx_got_yeeted`: The current instruction is sent again
    #[test]
    fn test_rx_got_yeeted() {
        let f = queue();
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue._send().unwrap();
        f.queue._rx_got_yeeted().unwrap();
        assert_eq!(written(&f), [b"G1\n".to_vec(), b"G1\n".to_vec()]);
    }

    /// `test_rx_yeet_without_instruction`: Nothing to resend
    #[test]
    fn test_rx_yeet_without_instruction() {
        let f = queue();
        f.queue._rx_got_yeeted().unwrap();
        assert!(written(&f).is_empty());
    }

    // --- flushing ---

    /// `test_flush_print_queue`: Numbered instructions are dropped, the others stay
    /// (`global_conditions.logs.trigger.assert_called_once()` not ported)
    #[test]
    fn test_flush_print_queue() {
        let f = queue();
        f.queue.enqueue_one(numbered_instruction("G1"), true);
        f.queue.enqueue_one(instruction("M105"), true);
        f.queue.enqueue_one(instruction("G2"), false);
        f.queue.flush_print_queue().unwrap();
        assert_eq!(
            messages(&f.queue.state.lock().unwrap().priority_queue),
            ["M105"]
        );
        assert_eq!(f.queue.state.lock().unwrap().queue.len(), 1);
    }

    /// `test_flush_print_queue_confirms_the_current_one`: The running instruction is dropped too
    #[test]
    fn test_flush_print_queue_confirms_the_current_one() {
        let f = queue();
        let instruction = numbered_instruction("G1");
        f.queue.enqueue_one(instruction.clone(), false);
        f.queue._send().unwrap();
        f.queue.flush_print_queue().unwrap();
        assert!(f.queue.state.lock().unwrap().current_instruction.is_none());
        assert!(instruction.is_confirmed());
    }

    /// `test_flush_queues`: Everything is confirmed without sending
    #[test]
    fn test_flush_queues() {
        let f = queue();
        let instructions = [instruction("G1"), instruction("G2"), instruction("M105")];
        f.queue.enqueue_one(instructions[0].clone(), false);
        f.queue.enqueue_one(instructions[1].clone(), false);
        f.queue.enqueue_one(instructions[2].clone(), true);
        f.queue._send().unwrap();
        let before = written(&f).len();
        f.queue._flush_queues().unwrap();
        assert!(f.queue.is_empty());
        assert!(f.queue.state.lock().unwrap().current_instruction.is_none());
        assert!(instructions.iter().all(|i| i.is_confirmed()));
        assert_eq!(written(&f).len(), before);
    }

    // --- reconnection ---

    /// `reconnected(queue, **kwargs)`: Runs the reconnection handler in this thread, confirming
    /// at once. Differs: the source patches `Instruction.wait_for_confirmation` to answer `True`;
    /// here the queue is closed, which ends the same wait (`while not self.closed`, `:580`).
    fn reconnected(f: &Fixture, was_printing: bool, was_power_panic: bool) {
        f.queue.state.lock().unwrap().closed = true;
        f.queue
            ._printer_reconnected(was_printing, was_power_panic)
            .unwrap();
    }

    /// `test_reconnected_while_printing`: The print is stopped
    #[test]
    fn test_reconnected_while_printing() {
        let f = queue();
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue.enqueue_one(instruction("G2"), false);
        f.queue._send().unwrap(); // G1 is in flight, G2 is waiting
        reconnected(&f, true, false);
        assert_eq!(
            messages(&f.queue.state.lock().unwrap().priority_queue),
            ["M603"]
        );
        assert!(f.queue.state.lock().unwrap().queue.is_empty());
    }

    /// `test_reconnected_after_power_panic`: The power panic keeps the printer as it is
    #[test]
    fn test_reconnected_after_power_panic() {
        let f = queue();
        reconnected(&f, true, true);
        assert!(f.queue.is_empty());
    }

    /// `test_reconnected_after_communication_failure`: The user is told and the printer stopped
    #[test]
    fn test_reconnected_after_communication_failure() {
        let f = queue();
        f.queue.state.lock().unwrap().has_failed = true;
        reconnected(&f, false, false);
        assert_eq!(
            messages(&f.queue.state.lock().unwrap().priority_queue),
            ["M1 FW COMM ERR. Aborted", "M603", "M300 S880 P200"]
        );
        assert!(!f.queue.state.lock().unwrap().has_failed);
    }

    /// `test_reconnected_starts_a_thread`: The recovery runs in its own thread
    #[test]
    fn test_reconnected_starts_a_thread() {
        let f = queue();
        f.calls.lock().unwrap().threads.clear();
        f.queue.printer_reconnected(true, false).unwrap();
        let threads = &f.calls.lock().unwrap().threads;
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].0, Target::PrinterReconnected(true, false));
    }

    // --- monitored ---

    /// `test_monitored_handlers`: Every sign of life renews the timeout
    #[test]
    fn test_monitored_handlers() {
        let f = monitored();
        let added = f.calls.lock().unwrap().added.clone();
        let regexes: Vec<&str> = added.iter().map(|(p, _, _)| p.as_str()).collect();
        for regex in [
            &*BUSY_REGEX,
            &*ATTENTION_REGEX,
            &*HEATING_REGEX,
            &*HEATING_HOTEND_REGEX,
        ] {
            assert!(regexes.contains(&regex.as_str()));
        }
        let handler = added
            .iter()
            .find(|(p, _, _)| p == BUSY_REGEX.as_str())
            .unwrap()
            .1
            .clone();
        f.queue.state.lock().unwrap().stuck_counter = 2;
        f.queue.state.lock().unwrap().last_event_on = 0.0;
        handler(&fired(None));
        assert_eq!(f.queue.state.lock().unwrap().stuck_counter, 0);
        assert!(f.queue.state.lock().unwrap().last_event_on > 0.0);
    }

    /// `test_current_delay`: The delay is known only with something to wait for
    #[test]
    fn test_current_delay() {
        let f = monitored();
        assert_eq!(f.queue.get_current_delay(), 0.0);
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue.state.lock().unwrap().last_event_on = crate::ports::prusalink::util::time() - 10.0;
        let delay = f.queue.get_current_delay();
        assert!(9.0 < delay && delay < 11.0);
    }

    /// `test_check_status_in_time`: Fresh confirmation, nothing happens
    /// (`global_conditions.logs.trigger.assert_not_called()` not ported: the same is shown by
    /// nothing written and the counter)
    #[test]
    fn test_check_status_in_time() {
        let f = monitored();
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue.check_status().unwrap();
        assert_eq!(f.queue.state.lock().unwrap().stuck_counter, 0);
    }

    /// `test_check_status_timeout`: The silent printer lost the buffer
    /// (`global_conditions.logs.trigger.assert_called_once()` not ported)
    #[test]
    fn test_check_status_timeout() {
        let f = monitored();
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue._send().unwrap();
        f.queue.state.lock().unwrap().last_event_on =
            crate::ports::prusalink::util::time() - SERIAL_QUEUE_TIMEOUT as f64 - 1.0;
        f.queue.check_status().unwrap();
        assert_eq!(f.queue.state.lock().unwrap().stuck_counter, 1);
        assert_eq!(written(&f), [b"G1\n".to_vec(), b"G1\n".to_vec()]);
        assert_eq!(f.calls.lock().unwrap().closed, 0);
    }

    /// `test_check_status_closes_a_stuck_serial`: Repeated timeouts close the port
    #[test]
    fn test_check_status_closes_a_stuck_serial() {
        let f = monitored();
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue._send().unwrap();
        for _ in 0..3 {
            f.queue.state.lock().unwrap().last_event_on =
                crate::ports::prusalink::util::time() - SERIAL_QUEUE_TIMEOUT as f64 - 1.0;
            f.queue.check_status().unwrap();
        }
        assert!(f.calls.lock().unwrap().closed > 0);
    }

    /// `test_monitored_confirmation_renews_timeout`: A confirmation is a sign of life
    #[test]
    fn test_monitored_confirmation_renews_timeout() {
        let f = monitored();
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue._send().unwrap();
        f.queue.state.lock().unwrap().last_event_on = 0.0;
        f.queue._confirmed(false).unwrap();
        assert!(f.queue.state.lock().unwrap().last_event_on > 0.0);
    }

    /// `test_keep_monitoring`: The monitoring loop calls the status check. Differs: the source
    /// patches `loop_until` and checks it gets `check_status`; here the loop runs in a thread on a
    /// timed-out queue until the check has counted it stuck, then `quit_evt` ends it.
    #[test]
    fn test_keep_monitoring() {
        let f = monitored();
        f.queue.enqueue_one(instruction("G1"), false);
        f.queue._send().unwrap();
        f.queue.state.lock().unwrap().last_event_on =
            crate::ports::prusalink::util::time() - SERIAL_QUEUE_TIMEOUT as f64 - 1.0;
        let queue = f.queue.clone();
        let monitor = std::thread::spawn(move || queue.keep_monitoring());
        for _ in 0..100 {
            if f.queue.state.lock().unwrap().stuck_counter > 0 {
                break;
            }
            time_sleep(0.01);
        }
        f.queue.quit_evt.set();
        monitor.join().unwrap();
        assert_eq!(f.queue.state.lock().unwrap().stuck_counter, 1);
    }

    /// `test_monitored_stop`: The threshold is saved, the monitor joined
    #[test]
    fn test_monitored_stop() {
        let f = monitored();
        f.queue.stop().unwrap();
        assert_eq!(f.calls.lock().unwrap().saved, 1);
        f.queue.wait_stopped().unwrap();
        assert_eq!(f.calls.lock().unwrap().joined, [Target::KeepMonitoring]);
    }
}
