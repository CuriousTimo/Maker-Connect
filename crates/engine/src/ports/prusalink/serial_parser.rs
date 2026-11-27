//! PrusaLink's `serial/serial_parser.py` (`reference/Prusa-Link`, `171148ce`): Contains
//! implementation of the SerialParser and Regex pairing classes. The latter is used by the former
//! for tracking which regular expressions have which handlers (#243, section B1).
//!
//! As of writing this doc, the "ok" has infinite priority, then every instruction handler has the
//! current time as the priority, meaning later added handlers are evaluated first.
//!
//! A handler is called with the sender (the pairing) and the match: one [`Fired`]. `Signal` is
//! blinker's ([`super::blinker`]), `SortedKeyList` sortedcontainers' ([`super::sortedcontainers`]),
//! `queue.Queue` and `threading.Thread` CPython's ([`crate::ports::python`]); `threading.Lock` is a
//! `Mutex` (a lock that is not reentrant, as `Lock`). A `re.Pattern` in `pairing_dict` is its
//! pattern: two compiled patterns are equal when their pattern and flags are, and ours carry their
//! flags in the pattern.
//!
//! **Differences:**
//! 1. `fire` catches what a handler raises (`except Exception`) as a Rust panic
//!    (`catch_unwind`): a Rust handler has no exception. As in the source, the receivers after the
//!    one that failed are not called (blinker's `send` stops at the exception).
//! 2. `SerialParser(metaclass=MCSingleton)`: the instance is the printer module's
//!    ([`super::mc_singleton`]); PrusaLink's tests reset it after each test
//!    (`SerialParser._MCSingleton__instance = None`), ours make a new one per test.
//!
//! 3. A priority is an `f64`; its `str` writes a whole one as the `int` PrusaLink gives
//!    (`0`, the tests' `1`, `3`), so a whole `float` priority (`2.0`) would be written `2`.
//!    PrusaLink's floats are `time()` and `inf`.
//!
//! **Not covered:** logging (`log.debug`, `log.exception`): kept as comments.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use fancy_regex::{Captures, Regex};
use indexmap::IndexMap;
use maker_connect_gcode_metadata::py::PyErr;

use super::blinker::{Receiver, Signal};
use super::instruction::Match;
use super::sortedcontainers::SortedKeyList;
use crate::ports::python::py_queue::{Deque, Queue};
use crate::ports::python::py_threading::{RuntimeError, Thread};

/// What a handler gets: `signal.send(self, match=match)`.
pub struct Fired {
    pub sender: Arc<RegexPairing>,
    pub r#match: Option<Arc<Match>>,
}

/// A handler: `Callable[[Any, re.Match], None]`.
pub type Handler = Arc<Receiver<Fired>>;

/// `regexp.match(line)`: the leftmost match, which starts at 0 exactly when `match` finds one
/// (the same backtracking, tried from position 0 first).
pub(crate) fn re_match<'t>(regexp: &Regex, line: &'t str) -> Option<Captures<'t>> {
    regexp
        .captures(line)
        .ok()
        .flatten()
        .filter(|c| c.get(0).is_some_and(|m| m.start() == 0))
}

/// `RegexPairing` (`:25-57`): An object representing a bound regexp to its handler, with
/// priority, for us to be able to sort which regexps to try first
pub struct RegexPairing {
    pub regexp: Regex,
    pub signal: Signal<Fired>,
    /// `Union[float, int]`: changed by `add_handler`
    priority: Mutex<f64>,
}

impl RegexPairing {
    /// `__init__(regexp, priority=0)` (`:31-34`)
    pub fn new(regexp: Regex, priority: f64) -> RegexPairing {
        RegexPairing {
            regexp,
            signal: Signal::new(),
            priority: Mutex::new(priority),
        }
    }

    pub fn priority(&self) -> f64 {
        *self.priority.lock().unwrap()
    }

    fn set_priority(&self, priority: f64) {
        *self.priority.lock().unwrap() = priority;
    }

    /// `__str__` (`:36-41`), and `__repr__` (`:43-44`)
    pub fn str(&self) -> String {
        let receiver_count = self.signal.receivers_len();
        format!(
            "RegexPairing for {} with priority {} having {} handler{}",
            self.regexp.as_str(),
            py_number(self.priority()),
            receiver_count,
            if receiver_count > 1 { "s" } else { "" }
        )
    }

    /// `fire(match=None)` (`:46-57`): Fire the associated signal, catch and log errors, don't
    /// want to kill the serial reading component
    pub fn fire(self: &Arc<Self>, r#match: Option<Arc<Match>>) {
        // log.debug("Matched %s calling %s", self, self.signal.receivers)
        let fired = Fired {
            sender: self.clone(),
            r#match,
        };
        if catch_unwind(AssertUnwindSafe(|| self.signal.send(&fired))).is_err() {
            // log.exception("Exception during handling of the printer output. "
            //               "Caught to stay alive.")
        }
    }
}

/// `str` of a priority: an `int` (the default `0`) or a `float` (`time()`, `inf`). The port holds
/// every priority as `f64`; a whole one is written as Python writes the `int` it was given.
fn py_number(value: f64) -> String {
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    if value.fract() == 0.0 && value.abs() < 1e16 {
        return format!("{}", value as i64);
    }
    maker_connect_gcode_metadata::py::float_repr(value)
}

/// A pairing in `pattern_list`: equal only to itself (Python's default `__eq__`).
#[derive(Clone)]
struct Item(Arc<RegexPairing>);

impl PartialEq for Item {
    fn eq(&self, other: &Item) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for Item {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.str())
    }
}

struct Lists {
    pattern_list: SortedKeyList<Item, f64>,
    pairing_dict: IndexMap<String, Arc<RegexPairing>>,
}

/// `SerialParser(metaclass=MCSingleton)` (`:60-141`): Its job is to try and find an appropriate
/// handler for every line that we receive from the printer
pub struct SerialParser {
    /// `self.lock`, with what it guards
    lock: Mutex<Lists>,
}

impl Default for SerialParser {
    fn default() -> Self {
        SerialParser::new()
    }
}

impl SerialParser {
    /// `__init__()` (`:66-69`)
    pub fn new() -> SerialParser {
        SerialParser {
            lock: Mutex::new(Lists {
                pattern_list: SortedKeyList::new(|item: &Item| -item.0.priority()),
                pairing_dict: IndexMap::new(),
            }),
        }
    }

    /// `decide(line)` (`:71-88`): The meat of the class, trying different RegexPairings ordered
    /// by their priorities, to find the matching one
    pub fn decide(&self, line: &str) {
        let mut chosen: Option<(Arc<RegexPairing>, Arc<Match>)> = None;

        {
            let lists = self.lock.lock().unwrap();
            for pairing in lists.pattern_list.iter() {
                if let Some(captures) = re_match(&pairing.0.regexp, line) {
                    let found = Match::new(&pairing.0.regexp, &captures);
                    chosen = Some((pairing.0.clone(), Arc::new(found)));
                    break;
                }
            }
        }

        if let Some((chosen_pairing, found)) = chosen {
            chosen_pairing.fire(Some(found));
        } else {
            // log.debug("Match not found for %s", line)
        }
    }

    /// `add_handler(regexp, handler, priority=0)` (`:90-124`): Add an entry to output handlers.
    /// `regexp`: if this matches, your handler will get called. Warning, should be unique, or the
    /// exact same as another one, after the first match, the matching is stopped! and all the
    /// handlers for the regexp are called. `priority`: Higher priority means the regexp will be
    /// attempted sooner in the list. For items with the same priority, the newest gets used first
    pub fn add_handler(
        &self,
        regexp: &Regex,
        handler: &Handler,
        priority: f64,
    ) -> Result<(), PyErr> {
        let mut lists = self.lock.lock().unwrap();
        if let Some(existing_pairing) = lists.pairing_dict.get(regexp.as_str()).cloned() {
            let item = Item(existing_pairing.clone());
            if !lists.pattern_list.contains(&item) {
                // log.debug("%s is not in %s. What?!", existing_pairing, self.pattern_list)
            }
            if priority > existing_pairing.priority() {
                lists
                    .pattern_list
                    .remove(&item)
                    .map_err(|e| PyErr::Other("ValueError", e.0))?;
                existing_pairing.set_priority(priority);
                lists.pattern_list.add(item);
                // log.debug("Priority updated from %s to %s", existing_pairing.priority, priority)
            }
            existing_pairing.signal.connect(handler, false);
        } else {
            let new_pairing = Arc::new(RegexPairing::new(regexp.clone(), priority));
            new_pairing.signal.connect(handler, false);

            lists
                .pairing_dict
                .insert(regexp.as_str().to_owned(), new_pairing.clone());
            lists.pattern_list.add(Item(new_pairing));
        }
        Ok(())
    }

    /// `remove_handler(regexp, handler)` (`:126-141`): Removes the regexp and handler from the
    /// list of serial output handlers
    pub fn remove_handler(&self, regexp: &Regex, handler: &Handler) -> Result<(), PyErr> {
        let mut lists = self.lock.lock().unwrap();
        if let Some(pairing) = lists.pairing_dict.get(regexp.as_str()).cloned() {
            pairing.signal.disconnect(handler);
            if !pairing.signal.has_receivers() {
                // `del self.pairing_dict[regexp]`: a dict keeps the order of the others
                lists.pairing_dict.shift_remove(regexp.as_str());
                lists
                    .pattern_list
                    .remove(&Item(pairing))
                    .map_err(|e| PyErr::Other("ValueError", e.0))?;
            }
            Ok(())
        } else {
            Err(PyErr::Other(
                "RuntimeError",
                format!("There is no handler registered for {}", regexp.as_str()),
            ))
        }
    }

    /// `len(self.pattern_list)`
    pub fn pattern_list_len(&self) -> usize {
        self.lock.lock().unwrap().pattern_list.len()
    }

    /// `regexp in self.pairing_dict`
    pub fn has_pairing(&self, regexp: &Regex) -> bool {
        self.lock
            .lock()
            .unwrap()
            .pairing_dict
            .contains_key(regexp.as_str())
    }

    /// The pairings in the order `decide` tries them (`pattern_list`), as their `str`.
    pub fn pairings(&self) -> Vec<String> {
        let lists = self.lock.lock().unwrap();
        lists.pattern_list.iter().map(|item| item.0.str()).collect()
    }
}

/// A queued call: `partial(handler, sender, match=match)`, or `lambda: None`.
type Call = Box<dyn FnOnce() + Send>;

/// `ThreadedSerialParser(SerialParser)` (`:144-187`): Implements a way to de-couple serial reader
/// from the rest of the app while allowing serial queue to remain coupled
pub struct ThreadedSerialParser {
    pub parser: SerialParser,
    handler_queue: Arc<Queue<Deque<Option<Call>>>>,
    running: Arc<Mutex<bool>>,
    pub thread: Thread,
}

impl ThreadedSerialParser {
    /// `__init__()` (`:148-156`)
    pub fn new() -> Result<ThreadedSerialParser, RuntimeError> {
        let parser = ThreadedSerialParser {
            parser: SerialParser::new(),
            handler_queue: Arc::new(Queue::new(0, Deque::new())),
            running: Arc::new(Mutex::new(false)),
            // name="serial_decoupler"
            thread: Thread::new(true),
        };
        *parser.running.lock().unwrap() = true;
        let (queue, running) = (parser.handler_queue.clone(), parser.running.clone());
        parser
            .thread
            .start(move || Self::process(&queue, &running))?;
        Ok(parser)
    }

    /// `decoupled(handler)` (`:158-163`): A function generator decoupling the caller thread by
    /// enqueuing instead of calling the provided handler with its call arguments
    pub fn decoupled(&self, handler: &Handler) -> Handler {
        let (queue, handler) = (self.handler_queue.clone(), handler.clone());
        Arc::new(move |fired: &Fired| {
            let (handler, fired) = (
                handler.clone(),
                Fired {
                    sender: fired.sender.clone(),
                    r#match: fired.r#match.clone(),
                },
            );
            let call: Call = Box::new(move || handler(&fired));
            // an unbounded queue: `put` does not fail
            let _ = queue.put(Some(call), true, None);
        })
    }

    /// `process()` (`:165-170`): Processes the handler as a new thread
    fn process(handler_queue: &Queue<Deque<Option<Call>>>, running: &Mutex<bool>) {
        while *running.lock().unwrap() {
            let Ok(handler) = handler_queue.get(true, None) else {
                break;
            };
            if let Some(handler) = handler {
                handler();
            }
        }
    }

    /// `add_decoupled_handler(regexp, handler, priority=0)` (`:172-177`): Converts given handler,
    /// so it does not block the caller
    pub fn add_decoupled_handler(
        &self,
        regexp: &Regex,
        handler: &Handler,
        priority: f64,
    ) -> Result<(), PyErr> {
        self.parser
            .add_handler(regexp, &self.decoupled(handler), priority)
    }

    /// `stop()` (`:179-182`): Signals a stop to the decoupler
    pub fn stop(&self) {
        *self.running.lock().unwrap() = false;
        let call: Call = Box::new(|| {});
        let _ = self.handler_queue.put(Some(call), true, None);
    }

    /// `wait_stopped()` (`:184-187`): Waits until the decoupler is fully stopped
    pub fn wait_stopped(&self) -> Result<(), RuntimeError> {
        self.thread.join(None)
    }
}

/// A subclass has its base's methods: `decide`, `add_handler`, `remove_handler`.
impl std::ops::Deref for ThreadedSerialParser {
    type Target = SerialParser;
    fn deref(&self) -> &SerialParser {
        &self.parser
    }
}

#[cfg(test)]
mod tests;
