//! PrusaLink's `serial/serial_queue.py` (`reference/Prusa-Link`, `171148ce`): Contains
//! implementation of the SerialQueue and the MonitoredSerialQueue (#245, section C).
//!
//! The idea was to separate the monitoring functionality to not clutter the queue and instruction
//! management.
//!
//! Python underneath: `deque` is a `VecDeque` (`appendleft` its front, `pop` and `[-1]` its back;
//! `deque(maxlen=HISTORY_LENGTH)` drops from the front when full), `threading.Event` and `Thread`
//! the CPython ports ([`py_threading`](crate::ports::python::py_threading)), `threading.Lock` a
//! `Mutex<()>` (not reentrant, as `Lock`), `blinker.Signal` section A's ([`Signal`]).
//!
//! **Differs** (owner, 2026-11-27: approved):
//! 1. What the tests patch is given to the queue, the source's own by default ([`Parts`]): the
//!    `SerialAdapter` and the `ThreadedSerialParser` are what the queue uses of them
//!    ([`SerialAdapter`]: `write`, `close`; [`Parser`]: `add_handler`, `remove_handler`),
//!    `IsPlannerFed` is a [`Planner`], `Thread` a [`Threads`] (the sender `:124`, the monitor
//!    `:613`, the recovery `:544`), and `time()` a [`Clock`], `time.time` unless given (as
//!    `item_updater`'s difference 4).
//! 2. `RPI_ENABLED` and `SERIAL` (`conditions.py:46-50`) are the printer module's conditions
//!    (section B2: one tree per module), given to the queue; it reads `SERIAL` (`bool(Condition)`:
//!    its state is `OK`, the SDK's `conditions.py:156`) and sets `RPI_ENABLED` (`:432`, `:538`).
//! 3. `MonitoredSerialQueue(SerialQueue)` is the same type with a monitor part
//!    ([`SerialQueue::monitored`]); where it overrides (`_confirmed`, `stop`, `wait_stopped`) the
//!    queue asks whether it has one. Its `wait_stopped` calls `super().stop()` (`:660-663`), not
//!    `super().wait_stopped()`, so the sender thread is not joined: kept as is.
//! 4. `metaclass=MCSingleton` (`:49`): the queue is the printer module's, as `SerialParser`'s
//!    (section B1, `serial_parser.rs` difference 2).
//! 5. Python's attributes, each read or written in one statement under the GIL, are one `Mutex`
//!    held for that statement only, never while the queue calls out (a write, a signal, the
//!    parser, the planner); `write_lock` is taken exactly where the source takes it.
//! 6. `self.current_instruction.output_captured` is a bound method: two of them are equal for the
//!    same instruction (`_hookup_output_capture` `:236-245`, `_teardown_output_capture`
//!    `:247-254`). Ours makes one handler when the current instruction is hooked up and removes
//!    that one at its teardown.
//!
//! **Not covered:**
//! - Logging (`log.debug`, `log.info`, `log.warning`, `log.error`) and
//!   `InterestingLogRotator.trigger` (`:450`, `:496`, `:648`, the rotation of the log files): kept
//!   as comments.
//! - `prctl_name()` (`:132`, `:557`, `:629`) and the threads' names (`sq_sender`,
//!   `serial_queue_reset_thread`, `sq_stall_recovery`): the system's thread names, as in section A.
//! - The `assert sender is not None` and `assert match is not None` of the handlers (`:371-372`,
//!   `:381`): the parser always gives both.
//! - `int()` of a `cmd_number` (`:286-291`, `:382`) with digits beyond ASCII (Python's `\d` and
//!   `int` take every Unicode decimal digit) or beyond an `i64`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};

use fancy_regex::Regex;
use maker_connect_gcode_metadata::py::PyErr;
use maker_connect_sdk::conditions::{CondState, Condition};

use super::blinker::Signal;
use super::consts::{
    HISTORY_LENGTH, MAX_INT, QUIT_INTERVAL, RX_SIZE, SERIAL_QUEUE_MONITOR_INTERVAL,
    SERIAL_QUEUE_TIMEOUT,
};
use super::instruction::{Instruction, Match};
use super::is_planner_fed::IsPlannerFed;
use super::item_updater::Clock;
use super::regular_expressions::{
    ATTENTION_REGEX, BUSY_REGEX, CONFIRMATION_REGEX, HEATING_HOTEND_REGEX, HEATING_REGEX,
    M110_REGEX, RESEND_REGEX,
};
use super::serial_parser::{Fired, Handler, SerialParser, ThreadedSerialParser, re_match};
use super::util::loop_until;
use crate::ports::python::py_threading::{Event, RuntimeError, Thread};

/// What the queue uses of `SerialAdapter` (`serial/serial_adapter.py`): `write` (`:307`) and
/// `close` (`:647`) (difference 1).
pub trait SerialAdapter: Send + Sync {
    /// `serial_adapter.write(data)`
    fn write(&self, data: &[u8]) -> Result<(), PyErr>;
    /// `serial_adapter.close()`
    fn close(&self);
}

/// What the queue uses of `ThreadedSerialParser` (`serial/serial_parser.py`): `add_handler`
/// (`:90-124`) and `remove_handler` (`:126-141`) (difference 1).
pub trait Parser: Send + Sync {
    fn add_handler(&self, regexp: &Regex, handler: &Handler, priority: f64) -> Result<(), PyErr>;
    fn remove_handler(&self, regexp: &Regex, handler: &Handler) -> Result<(), PyErr>;
}

impl Parser for SerialParser {
    fn add_handler(&self, regexp: &Regex, handler: &Handler, priority: f64) -> Result<(), PyErr> {
        SerialParser::add_handler(self, regexp, handler, priority)
    }
    fn remove_handler(&self, regexp: &Regex, handler: &Handler) -> Result<(), PyErr> {
        SerialParser::remove_handler(self, regexp, handler)
    }
}

impl Parser for ThreadedSerialParser {
    fn add_handler(&self, regexp: &Regex, handler: &Handler, priority: f64) -> Result<(), PyErr> {
        self.parser.add_handler(regexp, handler, priority)
    }
    fn remove_handler(&self, regexp: &Regex, handler: &Handler) -> Result<(), PyErr> {
        self.parser.remove_handler(regexp, handler)
    }
}

/// What the queue uses of `IsPlannerFed` (`serial/is_planner_fed.py`) (difference 1).
pub trait Planner: Send {
    /// `self.is_planner_fed()`
    fn call(&self) -> bool;
    /// `self.is_planner_fed.is_fed = value`
    fn set_is_fed(&mut self, value: bool);
    /// `self.is_planner_fed.process_value(value)`
    fn process_value(&mut self, value: f64) -> Result<(), PyErr>;
    /// `self.is_planner_fed.save()`
    fn save(&mut self) -> Result<(), PyErr>;
}

impl Planner for IsPlannerFed {
    fn call(&self) -> bool {
        IsPlannerFed::call(self)
    }
    fn set_is_fed(&mut self, value: bool) {
        self.is_fed = value;
    }
    fn process_value(&mut self, value: f64) -> Result<(), PyErr> {
        IsPlannerFed::process_value(self, value)
    }
    fn save(&mut self) -> Result<(), PyErr> {
        IsPlannerFed::save(self)
    }
}

/// Which `Thread(target=...)` of the source a thread is (difference 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    /// `Thread(name="sq_sender", target=self._keep_sending, daemon=True)` (`:124-126`)
    KeepSending,
    /// `Thread(target=self.keep_monitoring, name="sq_stall_recovery", daemon=True)` (`:613-615`)
    KeepMonitoring,
    /// `Thread(target=self._printer_reconnected, args=(was_printing, was_power_panic),
    /// name="serial_queue_reset_thread")` (`:544-546`)
    PrinterReconnected(bool, bool),
}

/// A started thread: what the queue does with it.
pub trait Joinable: Send + Sync {
    /// `thread.join()`
    fn join(&self) -> Result<(), RuntimeError>;
}

impl Joinable for Thread {
    fn join(&self) -> Result<(), RuntimeError> {
        Thread::join(self, None)
    }
}

/// `Thread(target=..., daemon=...).start()`: the module's `Thread` (difference 1).
pub trait Threads: Send + Sync {
    fn start(
        &self,
        target: Target,
        daemon: bool,
        run: Box<dyn FnOnce() + Send>,
    ) -> Result<Arc<dyn Joinable>, RuntimeError>;
}

/// The source's own: CPython's `Thread`.
pub struct PyThreads;

impl Threads for PyThreads {
    fn start(
        &self,
        _target: Target,
        daemon: bool,
        run: Box<dyn FnOnce() + Send>,
    ) -> Result<Arc<dyn Joinable>, RuntimeError> {
        let thread = Arc::new(Thread::new(daemon));
        thread.start(run)?;
        Ok(thread)
    }
}

/// What the queue gets besides its arguments (differences 1 and 2).
pub struct Parts {
    pub serial_adapter: Arc<dyn SerialAdapter>,
    pub serial_parser: Arc<dyn Parser>,
    /// `IsPlannerFed(threshold_path)` (`:120`), made by the queue at that point of its `__init__`
    pub planner: Box<dyn FnOnce() -> Result<Box<dyn Planner>, PyErr> + Send>,
    pub threads: Arc<dyn Threads>,
    pub clock: Clock,
    /// `RPI_ENABLED` (`conditions.py:50`)
    pub rpi_enabled: Condition,
    /// `SERIAL` (`conditions.py:46`)
    pub serial: Condition,
}

impl Parts {
    /// The source's own: `IsPlannerFed(threshold_path)`, CPython's `Thread`, `time.time`.
    pub fn new(
        serial_adapter: Arc<dyn SerialAdapter>,
        serial_parser: Arc<dyn Parser>,
        threshold_path: &str,
        rpi_enabled: Condition,
        serial: Condition,
    ) -> Parts {
        let threshold_path = threshold_path.to_owned();
        Parts {
            serial_adapter,
            serial_parser,
            planner: Box::new(move || {
                Ok(Box::new(IsPlannerFed::new(&threshold_path)?) as Box<dyn Planner>)
            }),
            threads: Arc::new(PyThreads),
            clock: Arc::new(super::util::time),
            rpi_enabled,
            serial,
        }
    }
}

/// `except (SerialException, OSError)` (`:145`): the class or one of `OSError`'s subclasses
/// (CPython's `Objects/exceptions.c`).
fn is_serial_or_os_error(error: &PyErr) -> bool {
    matches!(
        error.class_name(),
        "SerialException"
            | "OSError"
            | "BlockingIOError"
            | "ChildProcessError"
            | "ConnectionError"
            | "BrokenPipeError"
            | "ConnectionAbortedError"
            | "ConnectionRefusedError"
            | "ConnectionResetError"
            | "FileExistsError"
            | "FileNotFoundError"
            | "InterruptedError"
            | "IsADirectoryError"
            | "NotADirectoryError"
            | "PermissionError"
            | "ProcessLookupError"
            | "TimeoutError"
    )
}

/// `Instruction(message)`: the source's messages have no newline.
fn instruction(message: &str) -> Arc<Instruction> {
    Arc::new(Instruction::new(message, false, None, None).expect("no newline"))
}

/// The attributes Python changes (difference 5).
pub struct State {
    /// A queue of instructions for the printer
    pub queue: VecDeque<Arc<Instruction>>,
    /// This one shall contain time critical instructions
    pub priority_queue: VecDeque<Arc<Instruction>>,
    /// Instruction that is currently being handled
    pub current_instruction: Option<Arc<Instruction>>,
    /// Maximum bytes we'll write
    pub rx_max: i64,
    /// For numbered messages with checksums
    pub message_number: i64,
    /// When filament runs out or other buffer flushing calamity occurs, we need to re-send some
    /// commands that we already had dismissed as confirmed (`deque(maxlen=HISTORY_LENGTH)`)
    pub send_history: VecDeque<Arc<Instruction>>,
    /// A list which will contain all messages needed to recover
    pub recovery_list: Vec<Arc<Instruction>>,
    pub rx_yeet_slot: Option<Arc<Instruction>>,
    /// For stopping fast (power panic)
    pub closed: bool,
    /// Flag to be set when serial communication fails
    pub has_failed: bool,
    /// Workaround around M110 involves syncing the FW buffers using a G4. Whenever an M110 comes,
    /// a G4 needs to be prepended. To avoid getting stuck in an endless loop, let's flip a flag
    pub m110_workaround_slot: Option<Arc<Instruction>>,
    pub worked_around_m110: bool,
    /// Allows to temporarily block sending to the serial queue
    pub _block_sending: bool,
    /// `MonitoredSerialQueue.stuck_counter` (`:597`)
    pub stuck_counter: i64,
    /// `MonitoredSerialQueue.last_event_on` (`:612`): Remember when the last write or
    /// confirmation happened. If we want to time out, the communication has to be dead for some
    /// time. Useful only with unbuffered messages
    pub last_event_on: f64,
    /// The current instruction's `output_captured`, as hooked up (difference 6)
    capture: Option<(Arc<Instruction>, Handler)>,
}

/// `SerialQueue` (`:49-583`): Class responsible for sending commands to the printer.
///
/// Messages need to be sent one by one and need to be confirmed afterwards. There are many edge
/// cases like resend requests, message number resets, RX buffer dumping and so on, which this
/// class works around to provide as deterministic of a serial connection to a Prusa printer as
/// possible.
pub struct SerialQueue {
    me: Weak<SerialQueue>,
    pub serial_adapter: Arc<dyn SerialAdapter>,
    pub serial_parser: Arc<dyn Parser>,
    /// When the serial_queue cannot re-establish communication with the printer, let's signal
    /// this to other modules
    pub serial_queue_failed: Signal<Arc<SerialQueue>>,
    pub instruction_confirmed_signal: Signal<Arc<SerialQueue>>,
    pub message_number_changed: Signal<i64>,
    pub state: Mutex<State>,
    /// Make it possible to enqueue multiple consecutive instructions
    pub write_lock: Mutex<()>,
    pub is_planner_fed: Mutex<Box<dyn Planner>>,
    pub quit_evt: Event,
    pub send_event: Arc<Event>,
    pub sender_thread: Mutex<Option<Arc<dyn Joinable>>>,
    /// `MonitoredSerialQueue`'s thread (difference 3); `None` for a `SerialQueue`
    pub monitoring_thread: Mutex<Option<Arc<dyn Joinable>>>,
    monitored: bool,
    threads: Arc<dyn Threads>,
    clock: Clock,
    rpi_enabled: Condition,
    serial: Condition,
    /// The handlers the queue registered, kept so they stay the same ones
    handlers: Mutex<Vec<Handler>>,
}

impl SerialQueue {
    /// `SerialQueue(serial_adapter, serial_parser, threshold_path, rx_size=RX_SIZE)` (`:59-128`)
    pub fn new(parts: Parts, rx_size: Option<i64>) -> Result<Arc<SerialQueue>, PyErr> {
        let queue = Self::init(parts, rx_size.unwrap_or(RX_SIZE), false)?;
        queue.start_sender()?;
        Ok(queue)
    }

    /// `MonitoredSerialQueue(serial_adapter, serial_parser, threshold_path, rx_size=128)`
    /// (`:589-616`): Separates the queue monitoring into a different class.
    pub fn monitored(parts: Parts, rx_size: Option<i64>) -> Result<Arc<SerialQueue>, PyErr> {
        // super().__init__(serial_adapter, serial_parser, threshold_path, rx_size)
        let queue = Self::init(parts, rx_size.unwrap_or(128), true)?;
        queue.start_sender()?;

        queue.state.lock().unwrap().stuck_counter = 0;

        for regexp in [
            &*BUSY_REGEX,
            &*ATTENTION_REGEX,
            &*HEATING_REGEX,
            &*HEATING_HOTEND_REGEX,
        ] {
            let me = queue.me.clone();
            // lambda sender, match: self._renew_timeout()
            let handler: Handler = Arc::new(move |_: &Fired| {
                if let Some(me) = me.upgrade() {
                    me._renew_timeout(true);
                }
            });
            queue.handlers.lock().unwrap().push(handler.clone());
            queue.serial_parser.add_handler(regexp, &handler, 0.0)?;
        }

        // Remember when the last write or confirmation happened
        // If we want to time out, the communication has to be dead for some
        // time
        // Useful only with unbuffered messages
        let now = (queue.clock)();
        queue.state.lock().unwrap().last_event_on = now;
        let me = queue.me.clone();
        let thread = queue.threads.start(
            Target::KeepMonitoring,
            true,
            Box::new(move || {
                if let Some(me) = me.upgrade() {
                    me.keep_monitoring();
                }
            }),
        );
        *queue.monitoring_thread.lock().unwrap() = Some(thread.map_err(runtime)?);
        Ok(queue)
    }

    /// `__init__` up to the sender's start (`:64-126`).
    fn init(parts: Parts, rx_size: i64, monitored: bool) -> Result<Arc<SerialQueue>, PyErr> {
        let queue = Arc::new_cyclic(|me: &Weak<SerialQueue>| SerialQueue {
            me: me.clone(),
            serial_adapter: parts.serial_adapter,
            serial_parser: parts.serial_parser,
            serial_queue_failed: Signal::new(),
            instruction_confirmed_signal: Signal::new(),
            message_number_changed: Signal::new(),
            state: Mutex::new(State {
                queue: VecDeque::new(),
                priority_queue: VecDeque::new(),
                current_instruction: None,
                rx_max: rx_size,
                message_number: 0,
                send_history: VecDeque::new(),
                recovery_list: vec![],
                rx_yeet_slot: None,
                closed: false,
                has_failed: false,
                m110_workaround_slot: None,
                worked_around_m110: false,
                _block_sending: false,
                stuck_counter: 0,
                last_event_on: 0.0,
                capture: None,
            }),
            write_lock: Mutex::new(()),
            // replaced below, at the point of `__init__` where the source makes it
            is_planner_fed: Mutex::new(Box::new(NoPlanner)),
            quit_evt: Event::new(),
            send_event: Arc::new(Event::new()),
            sender_thread: Mutex::new(None),
            monitoring_thread: Mutex::new(None),
            monitored,
            threads: parts.threads,
            clock: parts.clock,
            rpi_enabled: parts.rpi_enabled,
            serial: parts.serial,
            handlers: Mutex::new(vec![]),
        });

        let me = queue.me.clone();
        let confirmation: Handler = Arc::new(move |fired: &Fired| {
            if let Some(me) = me.upgrade() {
                me._confirmation_handler(fired);
            }
        });
        let me = queue.me.clone();
        let resend: Handler = Arc::new(move |fired: &Fired| {
            if let Some(me) = me.upgrade() {
                me._resend_handler(fired);
            }
        });
        queue
            .handlers
            .lock()
            .unwrap()
            .extend([confirmation.clone(), resend.clone()]);
        queue
            .serial_parser
            .add_handler(&CONFIRMATION_REGEX, &confirmation, f64::INFINITY)?;
        queue
            .serial_parser
            .add_handler(&RESEND_REGEX, &resend, 0.0)?;

        *queue.is_planner_fed.lock().unwrap() = (parts.planner)()?;
        Ok(queue)
    }

    /// `self.sender_thread = Thread(...)`, `self.sender_thread.start()` (`:124-128`)
    fn start_sender(&self) -> Result<(), PyErr> {
        let me = self.me.clone();
        let thread = self.threads.start(
            Target::KeepSending,
            true,
            Box::new(move || {
                if let Some(me) = me.upgrade() {
                    me._keep_sending();
                }
            }),
        );
        *self.sender_thread.lock().unwrap() = Some(thread.map_err(runtime)?);
        Ok(())
    }

    fn me(&self) -> Arc<SerialQueue> {
        self.me.upgrade().expect("the queue is alive while it runs")
    }

    /// `_keep_sending()` (`:130-148`): Send the most important instruction when asked nicely
    pub fn _keep_sending(&self) {
        // prctl_name()
        loop {
            self.send_event.wait(None);
            if self.quit_evt.is_set() {
                break;
            }
            self.send_event.clear();
            if self.state.lock().unwrap()._block_sending {
                continue;
            }
            let _write_lock = self.write_lock.lock().unwrap();
            if !self.can_write() {
                continue;
            }
            match self._send() {
                Ok(()) => {}
                Err(error) if is_serial_or_os_error(&error) => {
                    // log.info("A serial write has failed, expecting serial "
                    //          "reader to fix the problem. In the meantime "
                    //          "waiting for a nudge to send again.")
                }
                // an exception ends the thread, as Python's does
                Err(error) => panic!("{}: {}", error.class_name(), error),
            }
        }
    }

    /// `block_sending()` (`:150-152`): Block sending of instructions until we unblock again
    pub fn block_sending(&self) {
        self.state.lock().unwrap()._block_sending = true;
    }

    /// `unblock_sending()` (`:154-158`): Unblock sending of instructions
    pub fn unblock_sending(&self) {
        let was_blocked = self.state.lock().unwrap()._block_sending;
        if was_blocked {
            self.state.lock().unwrap()._block_sending = false;
            self._try_writing();
        }
    }

    /// `_try_writing()` (`:160-164`): Nudge the sender thread to send an instruction
    pub fn _try_writing(&self) {
        self.send_event.set();
    }

    /// `stop()`: `SerialQueue.stop` (`:166-171`): Stops the serial queue sender;
    /// `MonitoredSerialQueue.stop` (`:652-658`): Stops the monitoring thread. If not required to
    /// go fast, saves the planner fed threshold
    pub fn stop(&self) -> Result<(), PyErr> {
        self.quit_evt.set();
        self.send_event.set();
        if self.monitored {
            self.is_planner_fed.lock().unwrap().save()?;
        }
        Ok(())
    }

    /// `wait_stopped()`: `SerialQueue.wait_stopped` (`:173-175`): Waits for the serial queue to
    /// stop; `MonitoredSerialQueue.wait_stopped` (`:660-663`): `super().stop()`, then the
    /// monitoring thread is joined (difference 3).
    pub fn wait_stopped(&self) -> Result<(), RuntimeError> {
        if self.monitored {
            // super().stop()
            self.quit_evt.set();
            self.send_event.set();
            let thread = self.monitoring_thread.lock().unwrap().clone();
            if let Some(thread) = thread {
                thread.join()?;
            }
        } else {
            let thread = self.sender_thread.lock().unwrap().clone();
            if let Some(thread) = thread {
                thread.join()?;
            }
        }
        Ok(())
    }

    /// `peek_next()` (`:177-192`): Look, what the next instruction is going to be
    pub fn peek_next(&self) -> Option<Arc<Instruction>> {
        let state = self.state.lock().unwrap();
        if let Some(slot) = &state.m110_workaround_slot {
            return Some(slot.clone());
        }
        if let Some(slot) = &state.rx_yeet_slot {
            return Some(slot.clone());
        }
        if let Some(last) = state.recovery_list.last() {
            return Some(last.clone());
        }
        if !state.priority_queue.is_empty() {
            let queue_last = state.queue.back().cloned();
            drop(state);
            if self.is_planner_fed.lock().unwrap().call() && queue_last.is_some() {
                return queue_last;
            }
            return self.state.lock().unwrap().priority_queue.back().cloned();
        }
        state.queue.back().cloned()
    }

    /// `_next_instruction()` (`:194-220`): Get a fresh instruction into the
    /// self.current_instruction handling slot
    pub fn _next_instruction(&self) -> Result<(), PyErr> {
        let mut state = self.state.lock().unwrap();
        if state.current_instruction.is_some() {
            return Err(PyErr::Other(
                "RuntimeError",
                "Cannot send a new instruction. When the last one didn't finish processing."
                    .to_owned(),
            ));
        }
        if state.m110_workaround_slot.is_some() {
            state.current_instruction = state.m110_workaround_slot.take();
        } else if state.rx_yeet_slot.is_some() {
            state.current_instruction = state.rx_yeet_slot.take();
        } else if !state.recovery_list.is_empty() {
            state.current_instruction = state.recovery_list.pop();
        } else if !state.priority_queue.is_empty() {
            drop(state);
            let fed = self.is_planner_fed.lock().unwrap().call();
            let mut state = self.state.lock().unwrap();
            if fed && !state.queue.is_empty() {
                // Invalidate, so the unimportant queue doesn't go all at once
                drop(state);
                self.is_planner_fed.lock().unwrap().set_is_fed(false);
                // log.debug("Allowing a non-important instruction through")
                let mut state = self.state.lock().unwrap();
                state.current_instruction = state.queue.pop_back();
            } else {
                state.current_instruction = state.priority_queue.pop_back();
            }
        } else if !state.queue.is_empty() {
            state.current_instruction = state.queue.pop_back();
        }
        Ok(())
    }

    // --- If statements in methods ---

    /// `can_write()` (`:223-226`): Determines whether we're in a state suitable for writing
    pub fn can_write(&self) -> bool {
        let idle = self.state.lock().unwrap().current_instruction.is_none();
        if !idle || self.is_empty() {
            return false;
        }
        let closed = self.state.lock().unwrap().closed;
        !closed
    }

    /// `is_empty()` (`:228-232`): Determines whether all queues and slots for writing are empty
    pub fn is_empty(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.queue.is_empty()
            && state.priority_queue.is_empty()
            && state.recovery_list.is_empty()
            && state.rx_yeet_slot.is_none()
            && state.m110_workaround_slot.is_none()
    }

    // --- Actual methods ---

    /// `self.current_instruction.output_captured` (difference 6)
    fn output_captured(instruction: &Arc<Instruction>) -> Handler {
        let instruction = instruction.clone();
        Arc::new(move |fired: &Fired| {
            if let Some(r#match) = &fired.r#match {
                instruction.output_captured(r#match.clone());
            }
        })
    }

    /// `_hookup_output_capture()` (`:236-245`): Instructions can capture output, this will
    /// register the handlers necessary
    pub fn _hookup_output_capture(&self) -> Result<(), PyErr> {
        let current = self.current();
        let handler = Self::output_captured(&current);
        self.state.lock().unwrap().capture = Some((current.clone(), handler.clone()));
        for regexp in &current.capturing_regexps {
            let priority = (self.clock)();
            self.serial_parser.add_handler(regexp, &handler, priority)?;
        }
        Ok(())
    }

    /// `_teardown_output_capture()` (`:247-254`): Tears down the capturing handlers, so they're
    /// not slowing us down and not preventing garbage collection
    pub fn _teardown_output_capture(&self) -> Result<(), PyErr> {
        let current = self.current();
        let handler = {
            let mut state = self.state.lock().unwrap();
            match state.capture.take() {
                Some((hooked, handler)) if Arc::ptr_eq(&hooked, &current) => handler,
                other => {
                    state.capture = other;
                    // never connected: an equal bound method of an instruction not hooked up
                    Self::output_captured(&current)
                }
            }
        };
        for regexp in &current.capturing_regexps {
            self.serial_parser.remove_handler(regexp, &handler)?;
        }
        Ok(())
    }

    /// `self.current_instruction`, where the source uses its attributes (an `AttributeError` on
    /// `None` there).
    fn current(&self) -> Arc<Instruction> {
        self.state
            .lock()
            .unwrap()
            .current_instruction
            .clone()
            .expect("AttributeError: 'NoneType' object has no attribute")
    }

    /// `_send()` (`:256-307`): Gets a new instruction and depending on what appears in the
    /// handling slot. Tries its best to send it
    pub fn _send(&self) -> Result<(), PyErr> {
        let next_instruction = self
            .peek_next()
            .expect("AttributeError: 'NoneType' object has no attribute 'message'");

        if re_match(&M110_REGEX, &next_instruction.message()).is_some()
            && !self.state.lock().unwrap().worked_around_m110
        {
            let mut state = self.state.lock().unwrap();
            state.m110_workaround_slot = Some(instruction("M400"));
            state.worked_around_m110 = true;
        }

        self._next_instruction()?;
        let instruction = self.current();

        if instruction.data().is_none() {
            if instruction.to_checksum {
                let reset = {
                    let mut state = self.state.lock().unwrap();
                    if state.send_history.len() as i64 == HISTORY_LENGTH {
                        state.send_history.pop_front();
                    }
                    state.send_history.push_back(instruction.clone());
                    state.message_number += 1;
                    state.message_number == MAX_INT
                };
                if reset {
                    self._reset_message_number();
                }
            }

            let message_number = self.state.lock().unwrap().message_number;
            instruction.fill_data(message_number)?;
        }

        // If the instruction is M110 read the value it'll set and save it
        let message = instruction.message();
        let m110_match = re_match(&M110_REGEX, &message).map(|c| Match::new(&M110_REGEX, &c));
        if let Some(m110_match) = &m110_match {
            let mut state = self.state.lock().unwrap();
            state.worked_around_m110 = false;
            state.send_history.clear();
            // log.debug("The message number is getting reset")
            if let Some(number) = m110_match.named("cmd_number") {
                state.message_number = py_int(number).unwrap_or(0);
            }
        }

        let _size = instruction.data().map_or(0, |data| data.len()) as i64;
        // if size > self.rx_max:
        //     log.warning(
        //         "The data %s we're trying to write is %sB. "
        //         "But we can only send %sB at most.",
        //         instruction.data.decode('ASCII'), size, self.rx_max)

        self._hookup_output_capture()?;
        self.current().sent();

        // Send the message number only after the instruction is sent
        if m110_match.is_some() {
            let message_number = self.state.lock().unwrap().message_number;
            self.message_number_changed.send(&message_number);
        }

        let data = self.current().data().unwrap_or_default();
        self.serial_adapter.write(&data)
    }

    /// `set_message_number(number)` (`:309-313`): Sets the message number to the given value.
    /// Only for power panic recovery
    pub fn set_message_number(&self, number: i64) {
        let _write_lock = self.write_lock.lock().unwrap();
        self.state.lock().unwrap().message_number = number;
    }

    /// `replenish_history(messages)` (`:315-323`): Expects that the message number is set to the
    /// current instruction ought to be sent next
    pub fn replenish_history(&self, messages: &[&str]) -> Result<(), PyErr> {
        let from_number = self.state.lock().unwrap().message_number - (messages.len() as i64 - 1);
        self.state.lock().unwrap().send_history.clear();
        for (i, message) in messages.iter().enumerate() {
            let instruction = Arc::new(Instruction::new(message, true, None, None)?);
            instruction.fill_data(from_number + i as i64)?;
            let mut state = self.state.lock().unwrap();
            if state.send_history.len() as i64 == HISTORY_LENGTH {
                state.send_history.pop_front();
            }
            state.send_history.push_back(instruction);
        }
        Ok(())
    }

    /// `_enqueue(instruction, to_front=False)` (`:325-330`): Internal method for enqueuing when
    /// already locked
    pub fn _enqueue(&self, instruction: Arc<Instruction>, to_front: bool) {
        let mut state = self.state.lock().unwrap();
        if to_front {
            state.priority_queue.push_front(instruction);
        } else {
            state.queue.push_front(instruction);
        }
    }

    /// `enqueue_one(instruction, to_front=False)` (`:332-346`): Enqueue one instruction. Don't
    /// interrupt, if anyone else is enqueueing instructions
    ///
    /// :param instruction: the thing to be enqueued
    /// :param to_front: whether to enqueue to front of the queue
    pub fn enqueue_one(&self, instruction: Arc<Instruction>, to_front: bool) {
        {
            let _write_lock = self.write_lock.lock().unwrap();
            // log.debug("%s enqueued %s", instruction,
            //           'to the front' if to_front else '')

            self._enqueue(instruction, to_front);
        }

        self._try_writing();
    }

    /// `enqueue_list(instruction_list, to_front=False)` (`:348-365`): Enqueue list of
    /// instructions. Don't interrupt, if anyone else is enqueueing instructions
    ///
    /// :param instruction_list: the list to enqueue
    /// :param to_front: whether to enqueue to front of the queue
    pub fn enqueue_list(&self, instruction_list: Vec<Arc<Instruction>>, to_front: bool) {
        {
            let _write_lock = self.write_lock.lock().unwrap();
            // log.debug("Instructions %s enqueued %s", instruction_list,
            //           'to the front' if to_front else '')

            for instruction in instruction_list {
                self._enqueue(instruction, to_front);
            }
        }

        self._try_writing();
    }

    // --- Static capture handlers ---

    /// `_confirmation_handler(sender, match)` (`:369-373`): Used to do M105 parsing, but that is
    /// not supported anymore.
    pub fn _confirmation_handler(&self, _fired: &Fired) {
        // assert sender is not None
        // assert match is not None
        if let Err(error) = self._confirmed(false) {
            panic!("{}: {}", error.class_name(), error);
        }
    }

    /// `_resend_handler(sender, match)` (`:375-395`): The printer can ask for re-sends of past
    /// numbered instructions. This method just parses the received match, does a bunch of checks
    /// and calls the actual handler resend()
    pub fn _resend_handler(&self, fired: &Fired) {
        // assert sender is not None
        let number = fired
            .r#match
            .as_ref()
            .and_then(|m| m.named("cmd_number"))
            .and_then(py_int)
            .expect("the regex matches digits only");
        let message_number = self.state.lock().unwrap().message_number;
        // log.info("Resend of %s requested. Current is %s", number,
        //          self.message_number)
        if message_number >= number {
            let not_numbered = self
                .state
                .lock()
                .unwrap()
                .current_instruction
                .as_ref()
                .is_none_or(|current| !current.to_checksum);
            if not_numbered {
                // log.warning("Re-send requested for a non-numbered message")
                // If that happened, the non-numbered message got yeeted from
                // the buffer, so let's solve that first
                if let Err(error) = self._rx_got_yeeted() {
                    panic!("{}: {}", error.class_name(), error);
                }
            }
            let message_number = self.state.lock().unwrap().message_number;
            self._resend((message_number - number) + 1);
        } else {
            // log.warning("We haven't sent anything with that number yet. "
            //             "The communication shouldn't fail after this.")
        }
    }

    // ---

    /// `_resend(count)` (`:399-418`): If possible, enqueue already sent instruction starting from
    /// the one requested back into the recovery list/queue, to be re-sent
    pub fn _resend(&self, count: i64) {
        let history_len = self.state.lock().unwrap().send_history.len() as i64;
        if !(0 < count && count < history_len) {
            // log.error("Impossible re-send request! Aborting...")
            self._worst_case_scenario();
        } else {
            let _write_lock = self.write_lock.lock().unwrap();
            let mut state = self.state.lock().unwrap();
            // get the instructions newest first, they are going to reverse
            // in the list
            let history: Vec<Arc<Instruction>> = state.send_history.iter().rev().cloned().collect();

            state.recovery_list.clear();
            for instruction_from_history in history.iter().take(count as usize) {
                let instruction = Instruction::new(
                    &instruction_from_history.message(),
                    true,
                    instruction_from_history.data(),
                    instruction_from_history.number(),
                )
                .expect("a sent message has no newline");
                state.recovery_list.push(Arc::new(instruction));
            }
        }
    }

    /// `_confirmed(force=False)`: `SerialQueue._confirmed` (`:420-455`): Printer confirmed an
    /// instruction. Tears down the instruction and prepares the module for processing of a new
    /// one; `MonitoredSerialQueue._confirmed` (`:665-668`): Adds a timeout renewal onto an
    /// instruction confirmation
    pub fn _confirmed(&self, force: bool) -> Result<(), PyErr> {
        if self.monitored {
            self._renew_timeout(true);
        }
        let current = self.state.lock().unwrap().current_instruction.clone();
        match current {
            Some(current) if current.is_sent() => {
                if current.confirm(force) {
                    if !force {
                        // If a message was successfully confirmed, the rpi port
                        // had to be ok imo
                        // (`OK` is not `UNKNOWN`: the state is set)
                        let _ = self.rpi_enabled.set_state(CondState::Ok);
                    }
                    self.instruction_confirmed_signal.send(&self.me());
                    let _write_lock = self.write_lock.lock().unwrap();
                    let instruction = self.current();

                    // If the instruction did not refuse to be confirmed
                    // Yes, that needs to happen
                    // log.debug("%s confirmed", instruction)

                    self._teardown_output_capture()?;

                    if instruction.to_checksum {
                        // Only check those times for check-summed instructions
                        let time_to_confirm = instruction.time_to_confirm().expect("confirmed");
                        self.is_planner_fed
                            .lock()
                            .unwrap()
                            .process_value(time_to_confirm)?;
                    }

                    self.state.lock().unwrap().current_instruction = None;
                } else {
                    // InterestingLogRotator.trigger("instruction refusing confirmation.")
                    // log.debug(
                    //     "%s refused confirmation. Hopefully it has a reason "
                    //     "for that", self.current_instruction)
                }
            }
            _ => {
                // log.error("Unexpected message confirmation. Ignoring")
            }
        }

        self._try_writing();
        Ok(())
    }

    /// `_rx_got_yeeted()` (`:457-472`): Something caused the RX buffer to get thrown out, let's
    /// re-send everything supposed to be in it.
    pub fn _rx_got_yeeted(&self) -> Result<(), PyErr> {
        // log.debug("Think that RX Buffer got yeeted, sending instruction again")
        // Let's bypass the check and write if we can.
        let current = self.state.lock().unwrap().current_instruction.clone();
        if let Some(instruction) = current {
            // These two types have to be recovered in their own ways
            let _write_lock = self.write_lock.lock().unwrap();
            self.state.lock().unwrap().rx_yeet_slot = Some(instruction.clone());
            self._teardown_output_capture()?;
            instruction.reset();
            self.state.lock().unwrap().current_instruction = None;
            self._send()?;
        }
        Ok(())
    }

    /// `reset_message_number()` (`:474-481`): Does not wait for the result, everything that gets
    /// enqueued after this will be executed after this. If this is no longer true, stuff will
    /// break
    pub fn reset_message_number(&self) {
        let _write_lock = self.write_lock.lock().unwrap();
        self._reset_message_number();
    }

    /// `_reset_message_number()` (`:483-486`): Sends a massage number reset gcode to the printer
    pub fn _reset_message_number(&self) {
        let instruction = instruction("M110 N0");
        self._enqueue(instruction, true);
    }

    /// `flush_print_queue()` (`:488-503`): Only printing instructions are checksummed, so let's
    /// get rid of those. We don't need to confirm them, they shouldn't be waited on. The only
    /// component able to wait on them is file printer and that should be stopping when this is
    /// called.
    pub fn flush_print_queue(&self) -> Result<(), PyErr> {
        let _write_lock = self.write_lock.lock().unwrap();
        // InterestingLogRotator.trigger("flushing of the serial queue.")
        {
            let mut state = self.state.lock().unwrap();
            let new_queue: VecDeque<Arc<Instruction>> = state
                .priority_queue
                .iter()
                .filter(|instruction| !instruction.to_checksum)
                .cloned()
                .collect();
            state.priority_queue = new_queue;
            state.recovery_list.clear();
        }
        self._throw_out_current_instruction()
    }

    /// `_flush_queues()` (`:505-522`): Tries to get rid of every queue by fake force confirming
    /// all instructions, to keep the serial queue consistent for example after a reboot.
    pub fn _flush_queues(&self) -> Result<(), PyErr> {
        if self.state.lock().unwrap().current_instruction.is_some() {
            // To flush the one instruction, that has not yet been confirmed
            // but has been sent, use the usual way
            self._throw_out_current_instruction()?;
            self._next_instruction()?;
        }
        loop {
            let Some(current) = self.state.lock().unwrap().current_instruction.clone() else {
                break;
            };
            // obviously don't send the other ones,
            // so they can be handled faster
            current.sent();
            current.confirm(true);
            self.state.lock().unwrap().current_instruction = None;
            self._next_instruction()?;
        }
        Ok(())
    }

    /// `_throw_out_current_instruction()` (`:524-529`): Throws out the currently executed
    /// instruction
    pub fn _throw_out_current_instruction(&self) -> Result<(), PyErr> {
        let current = self.state.lock().unwrap().current_instruction.clone();
        if let Some(current) = current {
            current.confirm(true);
            self._teardown_output_capture()?;
            self.state.lock().unwrap().current_instruction = None;
        }
        Ok(())
    }

    /// `_worst_case_scenario()` (`:531-539`): Everything has failed, let's abandon whatever we
    /// were doing and save the printer/user
    pub fn _worst_case_scenario(&self) {
        self.state.lock().unwrap().has_failed = true;
        // log.error("Communication failed. Aborting...")
        // (`NOK` is not `UNKNOWN`: the state is set)
        let _ = self.rpi_enabled.set_state(CondState::Nok);
        self.serial_queue_failed.send(&self.me());
    }

    /// `printer_reconnected(was_printing, was_power_panic)` (`:541-546`): The printer reset,
    /// starts a thread to recover the serial queue from such a state
    pub fn printer_reconnected(
        &self,
        was_printing: bool,
        was_power_panic: bool,
    ) -> Result<(), RuntimeError> {
        let me = self.me.clone();
        self.threads.start(
            Target::PrinterReconnected(was_printing, was_power_panic),
            // a thread made by a daemon thread or the main one: not a daemon (`Thread.__init__`)
            false,
            Box::new(move || {
                if let Some(me) = me.upgrade() {
                    if let Err(error) = me._printer_reconnected(was_printing, was_power_panic) {
                        panic!("{}: {}", error.class_name(), error);
                    }
                }
            }),
        )?;
        Ok(())
    }

    /// `_printer_reconnected(was_printing, was_power_panic)` (`:548-583`): Printer resets for two
    /// reasons, it has been stopped by the user, or the serial communication failed.
    ///
    /// Either way, the old instructions inside the serial queue are now useless. This method
    /// flushes the queues and depending on what caused the error, moves the printer head up, or
    /// demands user attention.
    pub fn _printer_reconnected(
        &self,
        was_printing: bool,
        was_power_panic: bool,
    ) -> Result<(), PyErr> {
        // prctl_name()
        let final_instruction = {
            let _write_lock = self.write_lock.lock().unwrap();
            self._flush_queues()?;
            self.state.lock().unwrap()._block_sending = false;

            let mut final_instruction = None;

            let has_failed = self.state.lock().unwrap().has_failed;
            if has_failed {
                let beep_instruction = instruction("M300 S880 P200");
                self._enqueue(beep_instruction, true);
                let stop_instruction = instruction("M603");
                self._enqueue(stop_instruction, true);
                let message_instruction = instruction("M1 FW COMM ERR. Aborted");
                self._enqueue(message_instruction.clone(), true);
                final_instruction = Some(message_instruction);
                self.state.lock().unwrap().has_failed = false;
            } else if was_printing && !was_power_panic {
                let stop_instruction = instruction("M603");
                self._enqueue(stop_instruction.clone(), true);
                final_instruction = Some(stop_instruction);
            }
            final_instruction
        };

        if let Some(final_instruction) = final_instruction {
            self._try_writing();
            while !self.state.lock().unwrap().closed {
                if final_instruction.wait_for_confirmation(Some(QUIT_INTERVAL)) {
                    break;
                }
            }
        }
        Ok(())
    }

    // --- MonitoredSerialQueue (`:586-674`) ---

    /// `get_current_delay()` (`:618-625`): If we are waiting on an instruction to be confirmed,
    /// returns the time we've been waiting
    pub fn get_current_delay(&self) -> f64 {
        if self.is_empty() && self.state.lock().unwrap().current_instruction.is_none() {
            return 0.0;
        }
        (self.clock)() - self.state.lock().unwrap().last_event_on
    }

    /// `keep_monitoring()` (`:627-631`): Runs the loop of monitoring the queue
    pub fn keep_monitoring(&self) {
        // prctl_name()
        loop_until(
            &self.quit_evt,
            || SERIAL_QUEUE_MONITOR_INTERVAL as f64,
            || {
                if let Err(error) = self.check_status() {
                    panic!("{}: {}", error.class_name(), error);
                }
            },
        );
    }

    /// `check_status()` (`:633-650`): Called periodically. If the confirmation wait times out,
    /// calls the appropriate handler
    pub fn check_status(&self) -> Result<(), PyErr> {
        if self.get_current_delay() > SERIAL_QUEUE_TIMEOUT as f64
            && self.serial.state() == CondState::Ok
        {
            // The printer did not respond in time, lets assume it forgot
            // what it was supposed to do
            // log.info("Timed out waiting for confirmation of %s after %ssec.",
            //          self.current_instruction, SERIAL_QUEUE_TIMEOUT)
            // log.debug("Assuming the printer yeeted our RX buffer")
            let stuck_counter = {
                let mut state = self.state.lock().unwrap();
                state.stuck_counter += 1;
                state.stuck_counter
            };
            if stuck_counter > 2 {
                // log.warning("Closing the serial, because it's stuck")
                self.serial_adapter.close();
            }
            // InterestingLogRotator.trigger("a stuck instruction")
            self._rx_got_yeeted()?;
            self._renew_timeout(false);
        }
        Ok(())
    }

    /// `_renew_timeout(unstuck=True)` (`:670-674`): Renews the instruction confirmation
    pub fn _renew_timeout(&self, unstuck: bool) {
        let now = (self.clock)();
        let mut state = self.state.lock().unwrap();
        state.last_event_on = now;
        if unstuck {
            state.stuck_counter = 0;
        }
    }
}

impl super::helpers::SerialQueue for SerialQueue {
    fn enqueue_one(&self, instruction: Arc<Instruction>, to_front: bool) {
        SerialQueue::enqueue_one(self, instruction, to_front)
    }
    fn enqueue_list(&self, instruction_list: Vec<Arc<Instruction>>, to_front: bool) {
        SerialQueue::enqueue_list(self, instruction_list, to_front)
    }
}

/// `int(text)` of the regexes' `cmd_number` groups (`\d+`, `-?\d*`): `ValueError` (`None`) for
/// an empty one or a lone `-`.
fn py_int(text: &str) -> Option<i64> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// A `RuntimeError` of `Thread.start` as Python's.
fn runtime(error: RuntimeError) -> PyErr {
    PyErr::Other("RuntimeError", error.0.to_owned())
}

/// Stands in for `self.is_planner_fed` before `__init__` makes it (`:120`): never called.
struct NoPlanner;

impl Planner for NoPlanner {
    fn call(&self) -> bool {
        unreachable!("the planner is made in __init__")
    }
    fn set_is_fed(&mut self, _value: bool) {
        unreachable!("the planner is made in __init__")
    }
    fn process_value(&mut self, _value: f64) -> Result<(), PyErr> {
        unreachable!("the planner is made in __init__")
    }
    fn save(&mut self) -> Result<(), PyErr> {
        unreachable!("the planner is made in __init__")
    }
}

#[cfg(test)]
mod tests;
