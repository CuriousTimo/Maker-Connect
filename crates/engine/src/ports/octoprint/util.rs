//! Port of the parts of OctoPrint 1.11.8 `src/octoprint/util/__init__.py` the port uses.
//!
//! Original: Copyright (C) Gina Häußge & others, AGPL-3.0.

/// `octoprint/util/__init__.py:477 filter_non_ascii(line)`: `to_bytes(to_unicode(line,
/// encoding="ascii"), encoding="ascii")` on a `str` is `line.encode("ascii")`, which raises (a
/// `ValueError`) for a non-ASCII character.
pub fn filter_non_ascii(line: &str) -> bool {
    !line.is_ascii()
}

/// `octoprint/util/__init__.py:495 filter_non_utf8(line)`: `line.encode("utf-8")` of a `str`.
/// OURS: Python raises only for a lone surrogate, which a Rust `str` cannot hold, so this is
/// `False` for every `str`.
pub fn filter_non_utf8(_line: &str) -> bool {
    false
}

/// `octoprint/util/__init__.py:503 chunks`
///
/// Yield successive n-sized chunks from l.
///
/// Taken from <http://stackoverflow.com/a/312464/2028598>
pub(crate) fn chunks<T>(l: &[T], n: usize) -> impl Iterator<Item = &[T]> {
    (0..l.len())
        .step_by(n)
        .map(move |i| &l[i..(i + n).min(l.len())])
}

use super::comm::{PyText, ValueError};
use serde_json::Value;

/// `octoprint/util/__init__.py:75 to_unicode(s_or_u, encoding="utf-8", errors="strict")`
///
/// Make sure ``s_or_u`` is a string (str). `s_or_u` is a `str` or `bytes` here (`PyText`); the
/// `None` and `str(s_or_u)` branches (:92-96) have no counterpart for a typed argument. Only the
/// encodings `utf-8` and `ascii` are ported (the ones the port passes); `errors` is `"strict"`
/// (`Err`, `UnicodeDecodeError`) or `"replace"` (U+FFFD for each undecodable sequence, as Python's
/// codecs do).
pub fn to_unicode(s_or_u: &PyText, encoding: &str, errors: &str) -> Result<String, ValueError> {
    match s_or_u {
        PyText::Bytes(b) => match (encoding, errors) {
            ("utf-8", "strict") => std::str::from_utf8(b)
                .map(str::to_string)
                .map_err(|_| ValueError("UnicodeDecodeError")),
            ("utf-8", "replace") => Ok(String::from_utf8_lossy(b).into_owned()),
            ("ascii", "strict") => {
                if b.is_ascii() {
                    Ok(b.iter().map(|c| *c as char).collect())
                } else {
                    Err(ValueError("UnicodeDecodeError"))
                }
            }
            ("ascii", "replace") => Ok(b
                .iter()
                .map(|c| if c.is_ascii() { *c as char } else { '\u{fffd}' })
                .collect()),
            _ => {
                unimplemented!("util/__init__.py:91, decode with another encoding or error handler")
            }
        },
        PyText::Str(s) => Ok(s.clone()),
    }
}

/// `octoprint/util/__init__.py:447 get_exception_string(fmt=...)`: text of the last raised
/// exception, for logs and error messages. A Rust panic is not an exception to read back, so this
/// is not ported.
pub fn get_exception_string() -> String {
    unimplemented!("util/__init__.py:447 get_exception_string")
}

/// `octoprint/util/__init__.py:467 sanitize_ascii(line)` for a `str` (a value of another type is
/// `ValueError`, which a typed argument cannot be): `to_unicode(line, encoding="ascii",
/// errors="replace").rstrip()`; a `str` is returned by `to_unicode` as it is.
pub fn sanitize_ascii(line: &str) -> String {
    let s =
        to_unicode(&PyText::Str(line.to_string()), "ascii", "replace").expect("errors=\"replace\"");
    super::comm::py_rstrip(&s).to_string()
}

/// What `get_exception_string` reads from `sys.exc_info()` and from `traceback.extract_tb(..)[0]`
/// (the first frame of the traceback: the frame that holds the `try`). OURS: Rust has no
/// exception object, so the caller says the exception's type name and message, and where the
/// `try` is (`file!()`, the name of the Python method it ports, `line!()` of the call).
pub struct ExceptionInfo<'a> {
    pub type_name: &'a str,
    pub message: &'a str,
    pub file: &'a str,
    pub function: &'a str,
    pub line: u32,
}

/// The default `fmt` of `get_exception_string`.
pub const EXCEPTION_STRING_FMT: &str = "{type}: '{message}' @ {file}:{function}:{line}";

/// `octoprint/util/__init__.py:447 get_exception_string(fmt=...)` for the exception `exc`
/// (`fmt.format(type=.., message=.., file=.., function=.., line=..)`).
pub fn get_exception_string_of(exc: &ExceptionInfo, fmt: &str) -> String {
    let mut out = String::new();
    let mut rest = fmt;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let value = [
            ("{type}", exc.type_name.to_string()),
            ("{message}", exc.message.to_string()),
            ("{file}", exc.file.to_string()),
            ("{function}", exc.function.to_string()),
            ("{line}", exc.line.to_string()),
        ]
        .into_iter()
        .find(|(k, _)| rest.starts_with(k));
        match value {
            Some((k, v)) => {
                out.push_str(&v);
                rest = &rest[k.len()..];
            }
            None => unimplemented!(
                "util/__init__.py:447 get_exception_string with a format other than the five names"
            ),
        }
    }
    out.push_str(rest);
    out
}

/// `octoprint/util/__init__.py:532 dict_merge(a, b, leaf_merger=None, in_place=False)`
///
/// Recursively deep-merges two dictionaries (JSON objects; `None` is `Value::Null`).
/// `in_place=True` is not ported (its one caller is not reachable, see `filemanager.rs`); the
/// result of `in_place=False` is what Python returns in both cases.
///
/// `leaf_merger` returns `Err(())` for Python's `ValueError` and `Ok(None)` for a `None` result.
pub fn dict_merge(
    a: &Value,
    b: &Value,
    leaf_merger: Option<&dyn Fn(&Value, &Value) -> Result<Option<Value>, ()>>,
) -> Value {
    let a = if a.is_null() {
        Value::Object(Default::default())
    } else {
        a.clone()
    };
    let b = if b.is_null() {
        Value::Object(Default::default())
    } else {
        b.clone()
    };

    let Value::Object(b) = b else {
        return b;
    };

    let Value::Object(mut result) = a else {
        panic!("TypeError: argument of type is not iterable (dict_merge of a non-dict)");
    };

    for (k, v) in b {
        if let Some(existing) = result.get(&k).filter(|x| x.is_object()) {
            let merged = dict_merge(existing, &v, leaf_merger);
            result.insert(k, merged);
        } else {
            let mut merged = None;
            if let (Some(existing), Some(lm)) = (result.get(&k), leaf_merger) {
                match lm(existing, &v) {
                    Ok(m) => merged = m,
                    Err(()) => {
                        // can't be merged by leaf merger
                    }
                }
            }
            let merged = match merged {
                Some(m) => m,
                None => v.clone(),
            };
            result.insert(k, merged);
        }
    }
    Value::Object(result)
}

/// `octoprint/util/__init__.py:720 dict_contains_keys(keys, dictionary)`
///
/// Recursively deep-checks if `dictionary` contains all keys found in `keys`.
pub fn dict_contains_keys(keys: &Value, dictionary: &Value) -> bool {
    let (Value::Object(keys), Value::Object(dictionary)) = (keys, dictionary) else {
        return false;
    };

    for (k, v) in keys {
        match dictionary.get(k) {
            None => return false,
            Some(d) => {
                if v.is_object() && !dict_contains_keys(v, d) {
                    return false;
                }
            }
        }
    }

    true
}

/// `octoprint/util/__init__.py:1079 BOMS`: encoding -> BOM (`codecs.BOM_*`)
pub const BOMS: [(&str, &[u8]); 5] = [
    ("utf-8-sig", &[0xef, 0xbb, 0xbf]),
    ("utf-16-le", &[0xff, 0xfe]),
    ("utf-16-be", &[0xfe, 0xff]),
    ("utf-32-le", &[0xff, 0xfe, 0x00, 0x00]),
    ("utf-32-be", &[0x00, 0x00, 0xfe, 0xff]),
];

/// `octoprint/util/__init__.py:1088 get_bom(filename, encoding)`
///
/// Check if the file has a BOM and if so return it.
///
/// Params:
///     filename (str): The file to check.
///     encoding (str): The encoding to check for.
///
/// Returns:
///     (bytes) the BOM or None if there is no BOM.
///
/// `open(filename, mode="rb")` failing is an `OSError`: `Err`.
pub fn get_bom(filename: &str, encoding: &str) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    let mut header = Vec::new();
    std::fs::File::open(filename)?
        .take(4)
        .read_to_end(&mut header)?;

    for (enc, bom) in BOMS {
        if header.starts_with(bom) && encoding.to_lowercase() == enc {
            return Ok(Some(bom.to_vec()));
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------------------------
// util/__init__.py:1184-1640 timers, event and queues
// ---------------------------------------------------------------------------------------------

use super::py_queue::{Deque, Empty, PutError, Queue, QueueStorage};
use crate::ports::python::py_threading::{
    Event, RLock, RuntimeError, Thread, cond_wait, deadline_after, lock, wait_cv,
};
use std::collections::HashSet;
use std::fmt::Display;
use std::hash::Hash;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// A Python callable taking no arguments (`function`, `condition`'s siblings, `on_finish`, ...).
/// Python's `args` / `kwargs` of the timers are what the closure captures.
pub type Callback = Arc<dyn Fn() + Send + Sync>;

/// The `interval` argument of `RepeatedTimer`: "float or callable".
pub enum Interval {
    Fixed(f64),
    Callable(Arc<dyn Fn() -> f64 + Send + Sync>),
}

struct RepeatedTimerFields {
    interval: Arc<dyn Fn() -> f64 + Send + Sync>,
    function: Callback,
    run_first: bool,
    condition: Arc<dyn Fn() -> bool + Send + Sync>,
    on_condition_false: Option<Callback>,
    on_cancelled: Option<Callback>,
    on_finish: Option<Callback>,
}

struct RepeatedTimerInner {
    fields: Mutex<RepeatedTimerFields>,
    finished: Event,
}

/// util/__init__.py:1184 RepeatedTimer
///
/// This class represents an action that should be run repeatedly in an interval. It is similar to python's
/// own :class:`threading.Timer` class, but instead of only running once the ``function`` will be run again and again,
/// sleeping the stated ``interval`` in between.
///
/// RepeatedTimers are started, as with threads, by calling their ``start()`` method. The timer can be stopped (in
/// between runs) by calling the :func:`cancel` method. The interval the time waited before execution of a loop may
/// not be exactly the same as the interval specified by the user.
///
/// Arguments:
///     interval (float or callable): The interval between each ``function`` call, in seconds. Can also be a callable
///         returning the interval to use, in case the interval is not static.
///     function (callable): The function to call.
///     args (list or tuple): The arguments for the ``function`` call. Defaults to an empty list.
///     kwargs (dict): The keyword arguments for the ``function`` call. Defaults to an empty dict.
///     run_first (boolean): If set to True, the function will be run for the first time *before* the first wait period.
///         If set to False (the default), the function will be run for the first time *after* the first wait period.
///     condition (callable): Condition that needs to be True for loop to continue. Defaults to ``lambda: True``.
///     on_condition_false (callable): Callback to call when the timer finishes due to condition becoming false. Will
///         be called before the ``on_finish`` callback.
///     on_cancelled (callable): Callback to call when the timer finishes due to being cancelled. Will be called
///         before the ``on_finish`` callback.
///     on_finish (callable): Callback to call when the timer finishes, either due to being cancelled or since
///         the condition became false.
///     daemon (bool): daemon flag to set on underlying thread.
pub struct RepeatedTimer {
    thread: Thread,
    inner: Arc<RepeatedTimerInner>,
}

impl RepeatedTimer {
    /// `args` and `kwargs` do not exist: the closure `function` carries them.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        interval: Interval,
        function: Callback,
        run_first: bool,
        condition: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
        on_condition_false: Option<Callback>,
        on_cancelled: Option<Callback>,
        on_finish: Option<Callback>,
        daemon: bool,
    ) -> Self {
        let condition = condition.unwrap_or_else(|| Arc::new(|| true));
        let interval: Arc<dyn Fn() -> f64 + Send + Sync> = match interval {
            Interval::Fixed(v) => Arc::new(move || v),
            Interval::Callable(f) => f,
        };
        RepeatedTimer {
            thread: Thread::new(daemon),
            inner: Arc::new(RepeatedTimerInner {
                fields: Mutex::new(RepeatedTimerFields {
                    interval,
                    function,
                    run_first,
                    condition,
                    on_condition_false,
                    on_cancelled,
                    on_finish,
                }),
                finished: Event::new(),
            }),
        }
    }

    /// `self.condition = ...` (assignable while the timer runs).
    pub fn set_condition(&self, condition: Arc<dyn Fn() -> bool + Send + Sync>) {
        lock(&self.inner.fields).condition = condition;
    }

    /// `self.interval = ...`
    pub fn set_interval(&self, interval: Interval) {
        lock(&self.inner.fields).interval = match interval {
            Interval::Fixed(v) => Arc::new(move || v),
            Interval::Callable(f) => f,
        };
    }

    /// `self.finished` (a `threading.Event`)
    pub fn finished(&self) -> &Event {
        &self.inner.finished
    }

    /// `Thread.start()`
    pub fn start(&self) -> Result<(), RuntimeError> {
        let inner = self.inner.clone();
        self.thread.start(move || inner.run())
    }

    /// `Thread.join(timeout)`
    pub fn join(&self, timeout: Option<f64>) -> Result<(), RuntimeError> {
        self.thread.join(timeout)
    }

    /// `Thread.is_alive()`
    pub fn is_alive(&self) -> bool {
        self.thread.is_alive()
    }

    pub fn cancel(&self) {
        let on_cancelled = lock(&self.inner.fields).on_cancelled.clone();
        self.inner._finish(&[on_cancelled]);
    }
}

impl RepeatedTimerInner {
    fn condition(&self) -> bool {
        let f = lock(&self.fields).condition.clone();
        f()
    }

    fn function(&self) {
        let f = lock(&self.fields).function.clone();
        f()
    }

    fn run(&self) {
        while self.condition() {
            let run_first = lock(&self.fields).run_first;
            if run_first {
                // if we are to run the function BEFORE waiting for the first time
                self.function();

                // make sure our condition is still met before running into the downtime
                if !self.condition() {
                    break;
                }
            }

            // wait, but break if we are cancelled
            let interval = lock(&self.fields).interval.clone();
            self.finished.wait(Some(interval()));
            if self.finished.is_set() {
                return;
            }

            let run_first = lock(&self.fields).run_first;
            if !run_first {
                // if we are to run the function AFTER waiting for the first time
                self.function();
            }
        }

        // we'll only get here if the condition was false
        let on_condition_false = lock(&self.fields).on_condition_false.clone();
        self._finish(&[on_condition_false]);
    }

    fn _finish(&self, callbacks: &[Option<Callback>]) {
        self.finished.set();

        for callback in callbacks {
            match callback {
                None => continue,
                Some(cb) => cb(),
            }
        }

        let on_finish = lock(&self.fields).on_finish.clone();
        if let Some(cb) = on_finish {
            cb();
        }
    }
}

struct ResettableTimerInner {
    _event: Event,
    _mutex: Mutex<()>,
    is_reset: AtomicBool,
    interval: Mutex<f64>,
    function: Callback,
    on_cancelled: Option<Callback>,
    on_reset: Option<Callback>,
}

/// util/__init__.py:1327 ResettableTimer
///
/// This class represents an action that should be run after a specified amount of time. It is similar to python's
/// own :class:`threading.Timer` class, with the addition of being able to reset the counter to zero.
///
/// ResettableTimers are started, as with threads, by calling their ``start()`` method. The timer can be stopped (in
/// between runs) by calling the :func:`cancel` method. Resetting the counter can be done with the :func:`reset` method.
///
/// Arguments:
///     interval (float or callable): The interval before calling ``function``, in seconds. Can also be a callable
///         returning the interval to use, in case the interval is not static.
///     function (callable): The function to call.
///     args (list or tuple): The arguments for the ``function`` call. Defaults to an empty list.
///     kwargs (dict): The keyword arguments for the ``function`` call. Defaults to an empty dict.
///     on_cancelled (callable): Callback to call when the timer finishes due to being cancelled.
///     on_reset (callable): Callback to call when the timer is reset.
///     daemon (bool): daemon flag to set on underlying thread.
///
/// (Narrowed: the Python docstring says `interval` may be a callable, but `run` passes it straight to
/// `Event.wait`, so only a number works there; it is an `f64` here.)
pub struct ResettableTimer {
    thread: Thread,
    inner: Arc<ResettableTimerInner>,
}

impl ResettableTimer {
    pub fn new(
        interval: f64,
        function: Callback,
        on_reset: Option<Callback>,
        on_cancelled: Option<Callback>,
        daemon: bool,
    ) -> Self {
        ResettableTimer {
            thread: Thread::new(daemon),
            inner: Arc::new(ResettableTimerInner {
                _event: Event::new(),
                _mutex: Mutex::new(()),
                is_reset: AtomicBool::new(true),
                interval: Mutex::new(interval),
                function,
                on_cancelled,
                on_reset,
            }),
        }
    }

    /// `self.is_reset`
    pub fn is_reset(&self) -> bool {
        self.inner.is_reset.load(Ordering::SeqCst)
    }

    /// `Thread.start()`
    pub fn start(&self) -> Result<(), RuntimeError> {
        let inner = self.inner.clone();
        self.thread.start(move || inner.run())
    }

    /// `Thread.join(timeout)`
    pub fn join(&self, timeout: Option<f64>) -> Result<(), RuntimeError> {
        self.thread.join(timeout)
    }

    /// `Thread.is_alive()`
    pub fn is_alive(&self) -> bool {
        self.thread.is_alive()
    }

    pub fn cancel(&self) {
        {
            let _g = lock(&self.inner._mutex);
            self.inner._event.set();
        }

        if let Some(cb) = &self.inner.on_cancelled {
            cb();
        }
    }

    /// `reset(interval=None)`; as in Python, `if interval:` ignores `None` and `0`.
    pub fn reset(&self, interval: Option<f64>) {
        {
            let _g = lock(&self.inner._mutex);
            if let Some(i) = interval {
                if i != 0.0 {
                    *lock(&self.inner.interval) = i;
                }
            }

            self.inner.is_reset.store(true, Ordering::SeqCst);
            self.inner._event.set();
            self.inner._event.clear();
        }

        if let Some(cb) = &self.inner.on_reset {
            cb();
        }
    }
}

impl ResettableTimerInner {
    fn run(&self) {
        while self.is_reset.load(Ordering::SeqCst) {
            {
                let _g = lock(&self._mutex);
                self.is_reset.store(false, Ordering::SeqCst);
            }
            let interval = *lock(&self.interval);
            self._event.wait(Some(interval));
        }

        if !self._event.is_set() {
            (self.function)();
        }
        let _g = lock(&self._mutex);
        self._event.set();
    }
}

/// util/__init__.py:1419 CountedEvent
///
/// (Narrowed: the counter and its bounds are `i64`, Python's are unbounded ints. The Python
/// `__init__(self, value=0, minimum=0, maximum=None, **kwargs)` takes `max` as a keyword that wins
/// over `maximum`; see [`CountedEvent::new`].)
pub struct CountedEvent {
    _counter: AtomicI64,
    _min: Mutex<Option<i64>>,
    _max: Mutex<Option<i64>>,
    _mutex: RLock,
    _event: Event,
}

impl CountedEvent {
    /// `CountedEvent(value, minimum, maximum, **kwargs)`: `max_kwarg` is `kwargs.get("max", maximum)`'s
    /// keyword: `None` when no `max=` was given, `Some(x)` when `max=x` was given (`x` itself may be
    /// `None`). The Python defaults are `value=0, minimum=Some(0), maximum=None`.
    pub fn new(
        value: i64,
        minimum: Option<i64>,
        maximum: Option<i64>,
        max_kwarg: Option<Option<i64>>,
    ) -> Self {
        let e = CountedEvent {
            _counter: AtomicI64::new(0),
            _min: Mutex::new(minimum),
            _max: Mutex::new(max_kwarg.unwrap_or(maximum)),
            _mutex: RLock::new(),
            _event: Event::new(),
        };

        e._internal_set(value);
        e
    }

    pub fn min(&self) -> Option<i64> {
        *lock(&self._min)
    }

    pub fn set_min(&self, val: Option<i64>) {
        let _g = self._mutex.lock();
        *lock(&self._min) = val;
    }

    pub fn max(&self) -> Option<i64> {
        *lock(&self._max)
    }

    pub fn set_max(&self, val: Option<i64>) {
        let _g = self._mutex.lock();
        *lock(&self._max) = val;
    }

    /// `is_set` property: Python returns the bound method `self._event.is_set`; this calls it.
    pub fn is_set(&self) -> bool {
        self._event.is_set()
    }

    pub fn counter(&self) -> i64 {
        let _g = self._mutex.lock();
        self._counter.load(Ordering::SeqCst)
    }

    pub fn set(&self) {
        let _g = self._mutex.lock();
        self._internal_set(self._counter.load(Ordering::SeqCst) + 1);
    }

    pub fn clear(&self, completely: bool) {
        let _g = self._mutex.lock();
        if completely {
            self._internal_set(0);
        } else {
            self._internal_set(self._counter.load(Ordering::SeqCst) - 1);
        }
    }

    pub fn reset(&self) {
        self.clear(true);
    }

    pub fn wait(&self, timeout: Option<f64>) {
        self._event.wait(timeout);
    }

    pub fn blocked(&self) -> bool {
        self.counter() <= 0
    }

    pub fn acquire(&self, blocking: bool) -> bool {
        self._mutex.acquire(blocking)
    }

    pub fn release(&self) -> Result<(), RuntimeError> {
        self._mutex.release()
    }

    fn _internal_set(&self, value: i64) {
        self._counter.store(value, Ordering::SeqCst);
        if self._counter.load(Ordering::SeqCst) <= 0 {
            if let Some(min) = self.min() {
                if self._counter.load(Ordering::SeqCst) < min {
                    self._counter.store(min, Ordering::SeqCst);
                }
            }
            self._event.clear();
        } else {
            if let Some(max) = self.max() {
                if self._counter.load(Ordering::SeqCst) > max {
                    self._counter.store(max, Ordering::SeqCst);
                }
            }
            self._event.set();
        }
    }
}

/// The `_prepend` hook of `PrependableQueue` (a subclass overrides it like `_put`).
pub trait PrependableStorage: QueueStorage {
    fn _prepend(&mut self, item: Self::Item) -> Result<(), Self::Error>;
}

impl<T> PrependableStorage for Deque<T> {
    /// `_prepend`: `self.queue.appendleft(item)`
    fn _prepend(&mut self, item: T) -> Result<(), Self::Error> {
        self.queue.push_front(item);
        Ok(())
    }
}

/// What `PrependableQueue.prepend`'s Python default `timeout=True` amounts to: `True` is `1`, so
/// `prepend(item)` waits at most 1 second (it is not "no timeout"). Pass this where Python's
/// default would apply.
pub const PREPEND_DEFAULT_TIMEOUT: Option<f64> = Some(1.0);

/// util/__init__.py:1526 PrependableQueue
///
/// A `queue.Queue` ([`Queue`], reached through `Deref`) with `prepend`.
pub struct PrependableQueue<S: PrependableStorage> {
    queue: Queue<S>,
}

impl<S: PrependableStorage> Deref for PrependableQueue<S> {
    type Target = Queue<S>;
    fn deref(&self) -> &Queue<S> {
        &self.queue
    }
}

impl<T> PrependableQueue<Deque<T>> {
    /// `PrependableQueue(maxsize=0)`
    pub fn new(maxsize: i64) -> Self {
        PrependableQueue {
            queue: Queue::new(maxsize, Deque::new()),
        }
    }
}

impl<S: PrependableStorage> PrependableQueue<S> {
    /// `PrependableQueue.__init__` for a subclass that brings its own `_put`/`_get`/`_prepend`.
    pub fn with_storage(maxsize: i64, storage: S) -> Self {
        PrependableQueue {
            queue: Queue::new(maxsize, storage),
        }
    }

    /// `prepend(self, item, block=True, timeout=True)`; see [`PREPEND_DEFAULT_TIMEOUT`].
    pub fn prepend(
        &self,
        item: S::Item,
        block: bool,
        timeout: Option<f64>,
    ) -> Result<(), PutError<S::Error>> {
        let q = &self.queue;
        let mut g = q.lock();
        if q.maxsize > 0 {
            if !block {
                if g.queue._qsize() as i64 == q.maxsize {
                    return Err(PutError::Full);
                }
            } else if timeout.is_none() {
                while g.queue._qsize() as i64 >= q.maxsize {
                    g = wait_cv(&q.not_full, g);
                }
            } else if timeout.unwrap() < 0.0 {
                return Err(PutError::ValueError(
                    "'timeout' must be a non-negative number".to_string(),
                ));
            } else {
                let endtime = deadline_after(timeout.unwrap());
                while g.queue._qsize() as i64 == q.maxsize {
                    if matches!(endtime, Some(e) if Instant::now() >= e) {
                        return Err(PutError::Full);
                    }
                    g = cond_wait(&q.not_full, g, endtime).0;
                }
            }
        }
        g.queue._prepend(item).map_err(PutError::Raised)?;
        g.unfinished_tasks += 1;
        q.not_empty.notify_one();
        Ok(())
    }
}

/// util/__init__.py:1610 TypeAlreadyInQueue
///
/// `t` is stored as `type` (Python `self.type`); the message is Python's `args[0]`.
/// (Narrowed: the type is any `Hash + Eq + Display` value `K`, Python's any hashable.)
#[derive(Debug, Clone, PartialEq)]
pub struct TypeAlreadyInQueue<K> {
    pub r#type: K,
    pub message: String,
}

impl<K> TypeAlreadyInQueue<K> {
    pub fn new(t: K, message: String) -> Self {
        TypeAlreadyInQueue { r#type: t, message }
    }
}

/// The storage of `TypedQueue`: `PrependableQueue._put/_get/_prepend` plus the `_lookup` set.
pub struct TypedStorage<T, K> {
    queue: Deque<(T, Option<K>)>,
    _lookup: HashSet<K>,
}

impl<T, K: Hash + Eq + Clone + Display> QueueStorage for TypedStorage<T, K> {
    type Item = (T, Option<K>);
    type Error = TypeAlreadyInQueue<K>;

    fn _qsize(&self) -> usize {
        self.queue._qsize()
    }

    fn _put(&mut self, item: Self::Item) -> Result<(), Self::Error> {
        let (_, item_type) = &item;
        if let Some(item_type) = item_type {
            if self._lookup.contains(item_type) {
                return Err(TypeAlreadyInQueue::new(
                    item_type.clone(),
                    format!("Type {item_type} is already in queue"),
                ));
            } else {
                self._lookup.insert(item_type.clone());
            }
        }

        self.queue._put(item).unwrap_or_else(|e| match e {});
        Ok(())
    }

    fn _get(&mut self) -> Result<Self::Item, Empty> {
        let item = self.queue._get()?;
        let (_, item_type) = &item;

        if let Some(item_type) = item_type {
            self._lookup.remove(item_type);
        }

        Ok(item)
    }
}

impl<T, K: Hash + Eq + Clone + Display> PrependableStorage for TypedStorage<T, K> {
    fn _prepend(&mut self, item: Self::Item) -> Result<(), Self::Error> {
        let (_, item_type) = &item;
        if let Some(item_type) = item_type {
            if self._lookup.contains(item_type) {
                return Err(TypeAlreadyInQueue::new(
                    item_type.clone(),
                    format!("Type {item_type} is already in queue"),
                ));
            } else {
                self._lookup.insert(item_type.clone());
            }
        }

        self.queue._prepend(item).unwrap_or_else(|e| match e {});
        Ok(())
    }
}

/// util/__init__.py:1561 TypedQueue
///
/// (`put`, `get` and `prepend` are the overrides taking/returning the item without its type; the
/// rest of `queue.Queue` is reached through `Deref`.)
pub struct TypedQueue<T, K: Hash + Eq + Clone + Display> {
    queue: PrependableQueue<TypedStorage<T, K>>,
}

impl<T, K: Hash + Eq + Clone + Display> Deref for TypedQueue<T, K> {
    type Target = PrependableQueue<TypedStorage<T, K>>;
    fn deref(&self) -> &Self::Target {
        &self.queue
    }
}

impl<T, K: Hash + Eq + Clone + Display> TypedQueue<T, K> {
    pub fn new(maxsize: i64) -> Self {
        TypedQueue {
            queue: PrependableQueue::with_storage(
                maxsize,
                TypedStorage {
                    queue: Deque::new(),
                    _lookup: HashSet::new(),
                },
            ),
        }
    }

    /// `put(self, item, item_type=None, *args, **kwargs)`: `block, timeout` are `Queue.put`'s.
    pub fn put(
        &self,
        item: T,
        item_type: Option<K>,
        block: bool,
        timeout: Option<f64>,
    ) -> Result<(), PutError<TypeAlreadyInQueue<K>>> {
        Queue::put(&self.queue, (item, item_type), block, timeout)
    }

    /// `get(self, *args, **kwargs)`: `block, timeout` are `Queue.get`'s.
    pub fn get(&self, block: bool, timeout: Option<f64>) -> Result<T, super::py_queue::GetError> {
        let (item, _) = Queue::get(&self.queue, block, timeout)?;
        Ok(item)
    }

    /// `prepend(self, item, item_type=None, *args, **kwargs)`: `block, timeout` are
    /// `PrependableQueue.prepend`'s (Python default `timeout=True`, see [`PREPEND_DEFAULT_TIMEOUT`]).
    pub fn prepend(
        &self,
        item: T,
        item_type: Option<K>,
        block: bool,
        timeout: Option<f64>,
    ) -> Result<(), PutError<TypeAlreadyInQueue<K>>> {
        PrependableQueue::prepend(&self.queue, (item, item_type), block, timeout)
    }
}

#[cfg(test)]
mod tests {
    //! Ports of `tests/util/test_repeated_timer.py`, `test_resettable_timer.py` and
    //! `test_counted_event.py` (the reference has no test for `PrependableQueue`/`TypedQueue`;
    //! the tests marked "ours" cover them and `queue.Queue`).
    use super::*;
    use crate::ports::octoprint::py_queue::GetError;
    use std::sync::atomic::AtomicUsize;

    /// `time.sleep(s)`: a blocking point of the GIL approximation (`py_threading.rs`)
    fn sleep(s: f64) {
        crate::ports::python::py_threading::time_sleep(s);
    }

    /// `mock.MagicMock()` with `call_count`
    #[derive(Clone)]
    struct Mock {
        calls: Arc<AtomicUsize>,
        side_effect: Option<Arc<dyn Fn() + Send + Sync>>,
    }
    impl Mock {
        fn new() -> Self {
            Mock {
                calls: Arc::new(AtomicUsize::new(0)),
                side_effect: None,
            }
        }
        fn with_side_effect(f: impl Fn() + Send + Sync + 'static) -> Self {
            Mock {
                calls: Arc::new(AtomicUsize::new(0)),
                side_effect: Some(Arc::new(f)),
            }
        }
        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
        fn cb(&self) -> Callback {
            let m = self.clone();
            Arc::new(move || {
                m.calls.fetch_add(1, Ordering::SeqCst);
                if let Some(f) = &m.side_effect {
                    f();
                }
            })
        }
    }

    /// `Countdown`
    #[derive(Clone)]
    struct Countdown(Arc<AtomicI64>);
    impl Countdown {
        fn new(start: i64) -> Self {
            Countdown(Arc::new(AtomicI64::new(start)))
        }
        fn step(&self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
        fn counter(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn timer(
        interval: f64,
        function: Callback,
        run_first: bool,
        condition: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
        on_condition_false: Option<Callback>,
        on_cancelled: Option<Callback>,
        on_finish: Option<Callback>,
    ) -> RepeatedTimer {
        RepeatedTimer::new(
            Interval::Fixed(interval),
            function,
            run_first,
            condition,
            on_condition_false,
            on_cancelled,
            on_finish,
            true,
        )
    }

    fn countdown_cond(c: &Countdown) -> Option<Arc<dyn Fn() -> bool + Send + Sync>> {
        let c = c.clone();
        Some(Arc::new(move || c.counter() > 0))
    }

    /// ours (the GIL approximation of `py_threading.rs`): `RepeatedTimer.run` with `run_first` runs the
    /// function before `Thread.start()` returns (CPython: the new thread keeps the GIL until it first
    /// blocks, at `finished.wait(interval)`), so a flag the starter sets after `start()` is still unset
    /// when the function reads it. Comm's `_onConnected` (comm.py:3717-3733) relies on it: the temperature
    /// timer's first `_poll_temperature` runs before `_changeState(STATE_OPERATIONAL)`.
    #[test]
    fn a_run_first_timer_started_before_a_flag_is_set_sees_it_unset_1000_times() {
        for round in 0..1000 {
            let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let seen_set = Arc::new(AtomicUsize::new(0));
            let (f, s) = (flag.clone(), seen_set.clone());
            let t = timer(
                60.0,
                Arc::new(move || {
                    if f.load(Ordering::SeqCst) {
                        s.fetch_add(1, Ordering::SeqCst);
                    }
                }),
                true,
                None,
                None,
                None,
                None,
            );
            t.start().unwrap();
            flag.store(true, Ordering::SeqCst);
            t.finished().set();
            t.join(None).unwrap();
            assert_eq!(
                0,
                seen_set.load(Ordering::SeqCst),
                "round {round}: the first run saw the flag already set"
            );
        }
    }

    // RepeatedTimerTest.test_condition
    #[test]
    fn repeated_timer_test_condition() {
        let countdown = Countdown::new(5);
        let c2 = countdown.clone();
        let timer_task = Mock::with_side_effect(move || c2.step());

        let t = timer(
            0.1,
            timer_task.cb(),
            false,
            countdown_cond(&countdown),
            None,
            None,
            None,
        );
        t.start().unwrap();

        // wait for it
        t.join(None).unwrap();

        assert_eq!(5, timer_task.call_count());
    }

    // RepeatedTimerTest.test_finished_callback
    #[test]
    fn repeated_timer_test_finished_callback() {
        let countdown = Countdown::new(5);
        let c2 = countdown.clone();
        let timer_task = Mock::with_side_effect(move || c2.step());
        let on_finished = Mock::new();

        let t = timer(
            0.1,
            timer_task.cb(),
            false,
            countdown_cond(&countdown),
            None,
            None,
            Some(on_finished.cb()),
        );
        t.start().unwrap();
        t.join(None).unwrap();

        assert_eq!(1, on_finished.call_count());
    }

    // RepeatedTimerTest.test_condition_callback
    #[test]
    fn repeated_timer_test_condition_callback() {
        let countdown = Countdown::new(5);
        let c2 = countdown.clone();
        let timer_task = Mock::with_side_effect(move || c2.step());
        let on_cancelled = Mock::new();
        let on_condition_false = Mock::new();

        let t = timer(
            0.1,
            timer_task.cb(),
            false,
            countdown_cond(&countdown),
            Some(on_condition_false.cb()),
            Some(on_cancelled.cb()),
            None,
        );
        t.start().unwrap();
        t.join(None).unwrap();

        assert_eq!(1, on_condition_false.call_count());
        assert_eq!(0, on_cancelled.call_count());
    }

    // RepeatedTimerTest.test_cancelled_callback
    #[test]
    fn repeated_timer_test_cancelled_callback() {
        let countdown = Countdown::new(5);
        let c2 = countdown.clone();
        let timer_task = Mock::with_side_effect(move || c2.step());
        let on_cancelled = Mock::new();
        let on_condition_false = Mock::new();

        let t = timer(
            10.0,
            timer_task.cb(),
            false,
            countdown_cond(&countdown),
            Some(on_condition_false.cb()),
            Some(on_cancelled.cb()),
            None,
        );
        t.start().unwrap();

        // give it some time to run
        sleep(1.0);

        // then cancel it and wait for the thread to really finish
        t.cancel();
        t.join(None).unwrap();

        assert_eq!(0, on_condition_false.call_count());
        assert_eq!(1, on_cancelled.call_count());
    }

    // RepeatedTimerTest.test_run_first
    #[test]
    fn repeated_timer_test_run_first() {
        let timer_task = Mock::new();

        let t = timer(60.0, timer_task.cb(), true, None, None, None, None);
        t.start().unwrap();

        // give it some time to run
        sleep(1.0);

        // then cancel it and wait for the thread to really finish
        t.cancel();
        t.join(None).unwrap();

        // should have run once
        assert_eq!(1, timer_task.call_count());
    }

    // RepeatedTimerTest.test_not_run_first
    #[test]
    fn repeated_timer_test_not_run_first() {
        let timer_task = Mock::new();

        let t = timer(60.0, timer_task.cb(), false, None, None, None, None);
        t.start().unwrap();

        // give it some time to run - should hang in the sleep phase though
        sleep(1.0);

        // then cancel it and wait for the thread to really finish
        t.cancel();
        t.join(None).unwrap();

        assert_eq!(0, timer_task.call_count());
    }

    // RepeatedTimerTest.test_adjusted_interval (class IncreasingInterval(start=3, factor=1))
    #[test]
    fn repeated_timer_test_adjusted_interval() {
        let (start, factor) = (3i64, 1i64);
        let counter = Countdown::new(start);
        let c2 = counter.clone();
        let timer_task = Mock::with_side_effect(move || c2.step());
        let c3 = counter.clone();
        let interval = move || ((start - c3.counter() + 1) * factor) as f64;

        let t = RepeatedTimer::new(
            Interval::Callable(Arc::new(interval)),
            timer_task.cb(),
            false,
            countdown_cond(&counter),
            None,
            None,
            None,
            true,
        );

        // this should take 1 + 2 + 3 = 6s
        let start_time = Instant::now();
        t.start().unwrap();
        t.join(None).unwrap();
        let duration = start_time.elapsed().as_secs_f64();

        assert_eq!(3, timer_task.call_count());
        assert!(duration >= 6.0);
        assert!(duration < 7.0);
    }

    // RepeatedTimerTest.test_condition_change_during_task
    #[test]
    fn repeated_timer_test_condition_change_during_task() {
        let timer_task = Mock::with_side_effect(|| sleep(2.0));

        let t = timer(0.1, timer_task.cb(), true, None, None, None, None);
        t.start().unwrap();

        sleep(1.0);
        t.set_condition(Arc::new(|| false));
        t.join(None).unwrap();

        assert_eq!(1, timer_task.call_count());
    }

    // ResettableTimerTest.test_function
    #[test]
    fn resettable_timer_test_function() {
        let timer_task = Mock::new();

        let t = ResettableTimer::new(10.0, timer_task.cb(), None, None, true);
        t.start().unwrap();

        // wait for it
        t.join(None).unwrap();

        assert_eq!(1, timer_task.call_count());
    }

    // ResettableTimerTest.test_reset_callback
    #[test]
    fn resettable_timer_test_reset_callback() {
        let timer_task = Mock::new();
        let on_reset_cb = Mock::new();

        let t = ResettableTimer::new(10.0, timer_task.cb(), Some(on_reset_cb.cb()), None, true);
        t.start().unwrap();

        t.reset(None);

        // wait for it
        t.join(None).unwrap();

        assert_eq!(1, timer_task.call_count());
        assert_eq!(1, on_reset_cb.call_count());
    }

    // ResettableTimerTest.test_canceled_callback
    #[test]
    fn resettable_timer_test_canceled_callback() {
        let timer_task = Mock::new();
        let on_cancelled_cb = Mock::new();

        let t = ResettableTimer::new(
            10.0,
            timer_task.cb(),
            None,
            Some(on_cancelled_cb.cb()),
            true,
        );
        t.start().unwrap();

        sleep(5.0);

        t.cancel();

        sleep(10.0);

        assert_eq!(0, timer_task.call_count());
        assert_eq!(1, on_cancelled_cb.call_count());
    }

    fn counted(value: i64) -> CountedEvent {
        CountedEvent::new(value, Some(0), None, None)
    }

    // CountedEventTest.test_set_once: "The counter should go from 0 to 1."
    #[test]
    fn counted_event_test_set_once() {
        let event = counted(0);

        assert_eq!(0, event._counter.load(Ordering::SeqCst));
        assert!(!event._event.is_set());

        event.set();

        assert_eq!(1, event._counter.load(Ordering::SeqCst));
        assert!(event._event.is_set());
    }

    // CountedEventTest.test_set_more_than_max: "The counter should never rise above max."
    #[test]
    fn counted_event_test_set_more_than_max() {
        let event = CountedEvent::new(0, Some(0), None, Some(Some(1)));

        assert_eq!(0, event._counter.load(Ordering::SeqCst));
        assert!(!event._event.is_set());

        event.set();

        assert_eq!(1, event._counter.load(Ordering::SeqCst));
        assert!(event._event.is_set());

        event.set();

        assert_eq!(1, event._counter.load(Ordering::SeqCst));
        assert!(event._event.is_set());
    }

    // CountedEventTest.test_clear_once: "The counter should to from 1 to 0."
    #[test]
    fn counted_event_test_clear_once() {
        let event = counted(1);

        assert_eq!(1, event._counter.load(Ordering::SeqCst));
        assert!(event._event.is_set());

        event.clear(false);

        assert_eq!(0, event._counter.load(Ordering::SeqCst));
        assert!(!event._event.is_set());
    }

    // CountedEventTest.test_clear_all: "The counter should go from 10 to 0."
    #[test]
    fn counted_event_test_clear_all() {
        let event = counted(10);

        assert_eq!(10, event._counter.load(Ordering::SeqCst));
        assert!(event._event.is_set());

        event.clear(true);

        assert_eq!(0, event._counter.load(Ordering::SeqCst));
        assert!(!event._event.is_set());
    }

    // CountedEventTest.test_clear_more_than_available: "The counter should never sink below 0."
    #[test]
    fn counted_event_test_clear_more_than_available() {
        let event = counted(1);

        assert_eq!(1, event._counter.load(Ordering::SeqCst));
        assert!(event._event.is_set());

        event.clear(false);

        assert_eq!(0, event._counter.load(Ordering::SeqCst));
        assert!(!event._event.is_set());

        event.clear(false);

        assert_eq!(0, event._counter.load(Ordering::SeqCst));
        assert!(!event._event.is_set());
    }

    // CountedEventTest.test_clear_more_than_available_without_minimum:
    // "The counter may sink below zero if initialized without a minimum."
    #[test]
    fn counted_event_test_clear_more_than_available_without_minimum() {
        let event = CountedEvent::new(1, None, None, None);

        assert_eq!(1, event._counter.load(Ordering::SeqCst));
        assert!(event._event.is_set());

        event.clear(false);

        assert_eq!(0, event._counter.load(Ordering::SeqCst));
        assert!(!event._event.is_set());

        event.clear(false);

        assert_eq!(-1, event._counter.load(Ordering::SeqCst));
        assert!(!event._event.is_set());
    }

    // CountedEventTest.test_blocked: "Blocked should only be true if the counter is 0."
    #[test]
    fn counted_event_test_blocked() {
        let event = counted(0);

        assert!(event.blocked());

        event.set();
        assert!(!event.blocked());

        event.clear(false);
        assert!(event.blocked());
    }

    // CountedEventTest.test_wait_immediately: "Unblocked wait should immediately return."
    #[test]
    fn counted_event_test_wait_immediately() {
        let event = counted(1);

        let start = Instant::now();
        event.wait(Some(2.0));
        let duration = start.elapsed().as_secs_f64();

        assert!(duration < 1.0);
    }

    // CountedEventTest.test_wait_blocking: "Set should immediately have blocked wait return."
    #[test]
    fn counted_event_test_wait_blocking() {
        let event = Arc::new(counted(0));

        let e2 = event.clone();
        let thread = Thread::new(true);
        thread
            .start(move || {
                sleep(1.0);
                e2.set();
            })
            .unwrap();

        let start = Instant::now();
        event.wait(Some(2.0));
        let duration = start.elapsed().as_secs_f64();

        assert!(duration < 2.0);
    }

    // CountedEventTest.test_wait_timeout: "Blocked should only wait until timeout."
    #[test]
    fn counted_event_test_wait_timeout() {
        let event = counted(0);

        let start = Instant::now();
        event.wait(Some(2.0));
        let duration = start.elapsed().as_secs_f64();

        assert!(duration >= 2.0);
        assert!(duration < 3.0);
    }

    // ---- ours: the reference has no tests for these ---------------------------------------------

    /// ours: `queue.Queue` blocking get/put with timeout, task_done/join, full/empty.
    #[test]
    fn ours_queue_stdlib_behaviour() {
        let q: PrependableQueue<Deque<i32>> = PrependableQueue::new(1);
        assert!(q.empty());
        q.put(1, true, None).unwrap();
        assert!(q.full());
        assert_eq!(q.put(2, false, None), Err(PutError::Full));
        let start = Instant::now();
        assert_eq!(q.put(2, true, Some(0.2)), Err(PutError::Full));
        assert!(start.elapsed().as_secs_f64() >= 0.2);
        assert_eq!(q.prepend(0, false, None), Err(PutError::Full));
        assert_eq!(q.get(true, None).unwrap(), 1);
        assert_eq!(q.get(true, Some(0.1)), Err(GetError::Empty));
        assert!(q.task_done().is_ok());
        assert!(q.task_done().is_err());
        q.join();
    }

    /// ours: `PrependableQueue.prepend` puts in front; a blocked `get` is woken by a `put`.
    #[test]
    fn ours_prependable_queue() {
        let q: Arc<PrependableQueue<Deque<i32>>> = Arc::new(PrependableQueue::new(0));
        q.put(1, true, None).unwrap();
        q.prepend(0, true, PREPEND_DEFAULT_TIMEOUT).unwrap();
        assert_eq!(q.get(true, None).unwrap(), 0);
        assert_eq!(q.get(true, None).unwrap(), 1);
        let q2 = q.clone();
        let th = Thread::new(true);
        th.start(move || {
            sleep(0.2);
            q2.put(7, true, None).unwrap();
        })
        .unwrap();
        assert_eq!(q.get(true, Some(5.0)).unwrap(), 7);
    }

    /// ours: `TypedQueue` refuses a second item of the same type until it is gotten.
    #[test]
    fn ours_typed_queue() {
        let q: TypedQueue<&str, &str> = TypedQueue::new(0);
        q.put("a", Some("M105"), true, None).unwrap();
        let err = q.put("b", Some("M105"), true, None).unwrap_err();
        assert_eq!(
            err,
            PutError::Raised(TypeAlreadyInQueue::new(
                "M105",
                "Type M105 is already in queue".to_string()
            ))
        );
        assert_eq!(q.qsize(), 1);
        q.prepend("p", None, true, None).unwrap();
        assert_eq!(q.get(true, None).unwrap(), "p");
        assert_eq!(q.get(true, None).unwrap(), "a");
        q.put("c", Some("M105"), true, None).unwrap();
    }
}

// ---------------------------------------------------------------------------------------------
// util/__init__.py:1616 CaseInsensitiveSet
// ---------------------------------------------------------------------------------------------

/// Python's `==` between two JSON-like values as `set` membership uses it: `True == 1 == 1.0`,
/// numbers by value, everything else by type and content.
fn py_value_eq(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value;
    fn num(v: &Value) -> Option<f64> {
        match v {
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Number(n) => n.as_f64(),
            _ => None,
        }
    }
    match (num(a), num(b)) {
        (Some(x), Some(y)) => x == y,
        (None, None) => a == b,
        _ => false,
    }
}

/// `octoprint/util/__init__.py:1616 CaseInsensitiveSet`
///
/// Basic case insensitive set
///
/// Any str values will be stored and compared in lower case. Other value types are left as-is.
/// The values are JSON values (`serde_json::Value`), standing for Python's `str`, `bool`, `int`
/// and the like.
#[derive(Debug, Clone)]
pub struct CaseInsensitiveSet {
    /// `self.data`, a Python `set`: no two entries are equal (`py_value_eq`).
    data: Vec<serde_json::Value>,
}

impl CaseInsensitiveSet {
    /// `util/__init__.py:1623 __init__`: `{x.lower() if isinstance(x, str) else x for x in args}`
    pub fn new(args: &[serde_json::Value]) -> Self {
        let mut data: Vec<serde_json::Value> = Vec::new();
        for x in args {
            let x = match x {
                serde_json::Value::String(s) => serde_json::Value::String(s.to_lowercase()),
                other => other.clone(),
            };
            if !data.iter().any(|d| py_value_eq(d, &x)) {
                data.push(x);
            }
        }
        CaseInsensitiveSet { data }
    }

    /// `util/__init__.py:1626 __contains__`
    pub fn contains(&self, item: &serde_json::Value) -> bool {
        match item {
            serde_json::Value::String(s) => {
                let lowered = serde_json::Value::String(s.to_lowercase());
                self.data.iter().any(|d| py_value_eq(d, &lowered))
            }
            other => self.data.iter().any(|d| py_value_eq(d, other)),
        }
    }

    /// `util/__init__.py:1632 __iter__`
    pub fn iter(&self) -> std::slice::Iter<'_, serde_json::Value> {
        self.data.iter()
    }

    /// `util/__init__.py:1635 __len__`
    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

#[cfg(test)]
mod case_insensitive_set_tests {
    //! Port of `tests/util/test_caseinsensitive_set.py`.
    use super::*;
    use serde_json::json;

    fn set() -> CaseInsensitiveSet {
        CaseInsensitiveSet::new(&[json!("A"), json!("FoO"), json!(true), json!(23)])
    }

    #[test]
    fn test_contained() {
        for value in [json!("A"), json!("a"), json!("foo"), json!(true), json!(23)] {
            assert!(set().contains(&value), "{value}");
        }
    }

    #[test]
    fn test_not_contained() {
        for value in [json!("b"), json!("fnord"), json!(false), json!(42)] {
            assert!(!set().contains(&value), "{value}");
        }
    }

    /// ours: `__len__` and `__iter__`, set semantics (`True` and `1` are one entry).
    #[test]
    fn ours_len_iter_and_equal_values() {
        let s = CaseInsensitiveSet::new(&[json!(true), json!(1), json!("Y"), json!("y")]);
        assert_eq!(2, s.len());
        assert_eq!(2, s.iter().count());
        assert!(s.contains(&json!(1.0)));
        assert!(s.contains(&json!("Y")));
    }
}

#[cfg(test)]
mod section_f_helper_tests {
    //! Ports of `tests/util/test_string_helpers.py` (`test_to_unicode_*`),
    //! `tests/util/test_file_helpers.py` (`GetBomTest`, with the two fixture files
    //! `tests/util/_files/utf8_with_bom.txt` / `utf8_without_bom.txt` re-created by their first
    //! bytes) and the docstring examples of `dict_merge` (util/__init__.py:532).
    use super::*;
    use serde_json::json;

    #[test]
    fn test_to_unicode() {
        assert_eq!(
            "test",
            to_unicode(&PyText::Str("test".into()), "utf-8", "strict").unwrap()
        );
        assert_eq!(
            "test",
            to_unicode(&PyText::Bytes(b"test".to_vec()), "utf-8", "strict").unwrap()
        );
        let data = "äöüß";
        assert_eq!(
            data,
            to_unicode(&PyText::Bytes(data.as_bytes().to_vec()), "utf-8", "strict").unwrap()
        );
        // test_to_unicode_bytes_utf8_vs_ascii: UnicodeDecodeError
        assert!(to_unicode(&PyText::Bytes(data.as_bytes().to_vec()), "ascii", "strict").is_err());
        // ... with errors="replace": the bytes decode to U+FFFD each
        assert_eq!(
            "\u{fffd}".repeat(data.len()),
            to_unicode(&PyText::Bytes(data.as_bytes().to_vec()), "ascii", "replace").unwrap()
        );
    }

    fn fixture(content: &[u8]) -> String {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "maker-connect-util-bom-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::write(&path, content).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn test_get_bom() {
        let with_bom = fixture(b"\xef\xbb\xbf# This is a text file encoded in UTF8 with a BOM");
        let without_bom = fixture(b"# This is a text file encoded in UTF8 without a BOM");
        // test_get_bom_no_bom
        assert_eq!(None, get_bom(&without_bom, "utf-8-sig").unwrap());
        // test_get_bom_utf8_bom
        assert_eq!(
            Some(b"\xef\xbb\xbf".to_vec()),
            get_bom(&with_bom, "utf-8-sig").unwrap()
        );
        // test_get_bom_wrong_encoding
        assert_eq!(None, get_bom(&with_bom, "utf-16-le").unwrap());
    }

    /// ours: UTF-16/32 BOMs are found, a missing file is an `OSError`
    #[test]
    fn ours_get_bom_others() {
        let le = fixture(b"\xff\xfeA\x00");
        assert_eq!(
            Some(b"\xff\xfe".to_vec()),
            get_bom(&le, "UTF-16-LE").unwrap()
        );
        assert!(get_bom("/nonexistent/file", "utf-8-sig").is_err());
    }

    #[test]
    fn test_dict_merge_docstring() {
        let a = json!({"foo": "foo", "bar": "bar", "fnord": {"a": 1}});
        let b = json!({"foo": "other foo", "fnord": {"b": 2, "l": ["some", "list"]}});
        let expected = json!({"foo": "other foo", "bar": "bar", "fnord": {"a": 1, "b": 2, "l": ["some", "list"]}});
        assert_eq!(expected, dict_merge(&a, &b, None));
        assert_eq!(a, dict_merge(&a, &Value::Null, None));
        assert_eq!(b, dict_merge(&Value::Null, &b, None));
        assert_eq!(json!({}), dict_merge(&Value::Null, &Value::Null, None));

        let leaf_merger = |a: &Value, b: &Value| -> Result<Option<Value>, ()> {
            match (a, b) {
                (Value::Array(x), Value::Array(y)) => {
                    Ok(Some(Value::Array(x.iter().chain(y).cloned().collect())))
                }
                _ => Err(()),
            }
        };
        let result = dict_merge(
            &json!({"l1": [3, 4], "l2": [1], "a": "a"}),
            &json!({"l1": [1, 2], "l2": "foo", "b": "b"}),
            Some(&leaf_merger),
        );
        assert_eq!(json!([3, 4, 1, 2]), result["l1"]);
        assert_eq!(json!("foo"), result["l2"]);
        assert_eq!(json!("a"), result["a"]);
        assert_eq!(json!("b"), result["b"]);
    }

    /// ours: the context merge `_getGcodeScript` does for a plugin's variables
    #[test]
    fn ours_dict_merge_plugins_context() {
        let context = json!({"printer_profile": {"a": 1}});
        let merged = dict_merge(&context, &json!({"plugins": {"p": {"x": 1}}}), None);
        let merged = dict_merge(&merged, &json!({"plugins": {"q": 2}}), None);
        assert_eq!(
            json!({"printer_profile": {"a": 1}, "plugins": {"p": {"x": 1}, "q": 2}}),
            merged
        );
        assert_eq!(json!(5), dict_merge(&context, &json!(5), None)); // `not isinstance(b, dict)`
    }
}
