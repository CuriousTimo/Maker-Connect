//! Python's `threading` pieces that the ported OctoPrint classes use (CPython 3.13, `reference/cpython-3.13`,
//! `Lib/threading.py`): `Event`, `RLock` and the start/join/daemon behaviour of `Thread`.
//!
//! Named after the stdlib source; written with `std::sync::{Mutex, Condvar}` and `std::thread`.
//! Python's locks do not poison: a poisoned `std` mutex is used as it is.
//!
//! **OURS, approved (docs/plans/agent-engine.md, "the five questions", decision 1): `Thread::start` under the GIL.**
//! CPython's `Thread.start()` (`threading.py:956-982`) waits on `self._started` (`:982`); the new thread
//! (`_bootstrap_inner`, `:1028-1044`) sets it (`:1033`) and, holding the GIL, runs on until it first
//! blocks (or ends) before the starting thread gets the GIL back. That "the starter continues only
//! after the new thread's first blocking point" is not a line of `threading.py`, it is the GIL, which is
//! not ported. Here: `start()` returns when the new thread reaches its first blocking point
//! (`blocking_point`: an `Event`/`Condition` wait, a lock acquire that has to wait, `Queue.get`/a
//! blocking `put`, `time.sleep`, `Thread.join`, a serial `readline`/`write`) or its end. CPython's
//! forced switch every 5 ms and every other release of the GIL are not modelled. No timing of ours.
//! In debug builds only (`cfg(debug_assertions)`, never on the wire) a watchdog panics when a `start()`
//! waits longer than [`START_HANDSHAKE_BOUND`] (OURS, the bound is ours): a thread whose first blocking
//! point is not hooked would otherwise hang the starter.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use std::cell::RefCell;

/// The bound of the debug-only watchdog in [`Thread::start`]: OURS. A `start()` that waits for the new
/// thread's first blocking point longer than this panics (debug builds only).
#[cfg(debug_assertions)]
pub const START_HANDSHAKE_BOUND: Duration = Duration::from_secs(10);

/// `threading._started` as the starter waits on it: set when the new thread blocks for the first time
/// or ends. See the module doc.
struct Handshake {
    done: Mutex<bool>,
    cv: Condvar,
}

impl Handshake {
    fn signal(&self) {
        *self.done.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.cv.notify_all();
    }
}

std::thread_local! {
    /// The handshake of the thread being started, until this thread first blocks.
    static STARTING: RefCell<Option<Arc<Handshake>>> = const { RefCell::new(None) };
}

/// Debug only: which blocking point released each `start()` (the thread's closure type, the kind of
/// blocking point and where it is called from).
#[cfg(debug_assertions)]
pub static FIRST_BLOCKING: Mutex<Vec<(String, &'static str, String)>> = Mutex::new(Vec::new());

#[cfg(debug_assertions)]
std::thread_local! {
    static START_NAME: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The calling thread is about to block (see the module doc): if it has been started and has not yet
/// blocked, its starter may continue. `kind` names the blocking point.
#[cfg_attr(debug_assertions, track_caller)]
pub(crate) fn blocking_point(kind: &'static str) {
    let hs = STARTING.try_with(|s| s.borrow_mut().take()).ok().flatten();
    if let Some(hs) = hs {
        #[cfg(debug_assertions)]
        {
            let at = std::panic::Location::caller();
            let name = START_NAME
                .try_with(|n| n.borrow().clone())
                .unwrap_or_default();
            lock(&FIRST_BLOCKING).push((name, kind, format!("{}:{}", at.file(), at.line())));
        }
        let _ = kind;
        hs.signal();
    }
}

/// `lock.acquire()` of a plain `std` mutex: a blocking point when it has to wait.
#[cfg_attr(debug_assertions, track_caller)]
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            blocking_point("lock acquire that waits");
            m.lock().unwrap_or_else(|e| e.into_inner())
        }
    }
}

/// `m.lock().unwrap_or_else(|e| e.into_inner())` of the port, as a blocking point when it has to wait.
pub(crate) trait LockExt<T> {
    fn plock(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    #[cfg_attr(debug_assertions, track_caller)]
    fn plock(&self) -> MutexGuard<'_, T> {
        lock(self)
    }
}

/// `Condition.wait()` without a timeout: a blocking point, then the wait.
#[cfg_attr(debug_assertions, track_caller)]
pub(crate) fn wait_cv<'a, T>(cv: &Condvar, g: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    blocking_point("Condition wait");
    cv.wait(g).unwrap_or_else(|e| e.into_inner())
}

/// `time.sleep(seconds)`: a blocking point, then the sleep.
#[cfg_attr(debug_assertions, track_caller)]
pub fn time_sleep(seconds: f64) {
    blocking_point("time.sleep");
    std::thread::sleep(Duration::from_secs_f64(seconds.max(0.0)));
}

/// Python's `RuntimeError`.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeError(pub &'static str);

/// A timeout in seconds as Python's `Condition.wait(timeout)` treats it: zero or negative is
/// "do not wait". `None` for a timeout that does not fit an `Instant` (waits without end).
pub(crate) fn deadline_after(seconds: f64) -> Option<Instant> {
    let d = if seconds > 0.0 {
        Duration::try_from_secs_f64(seconds).ok()?
    } else {
        Duration::ZERO
    };
    Instant::now().checked_add(d)
}

/// Wait on `cv` until `deadline` (or, for `None`, without end) for one wake-up. Returns the
/// guard and whether time ran out.
#[cfg_attr(debug_assertions, track_caller)]
pub(crate) fn cond_wait<'a, T>(
    cv: &Condvar,
    g: MutexGuard<'a, T>,
    deadline: Option<Instant>,
) -> (MutexGuard<'a, T>, bool) {
    match deadline {
        None => (wait_cv(cv, g), false),
        Some(d) => {
            let now = Instant::now();
            if now >= d {
                return (g, true);
            }
            blocking_point("Condition wait");
            let (g, r) = cv
                .wait_timeout(g, d - now)
                .unwrap_or_else(|e| e.into_inner());
            (g, r.timed_out())
        }
    }
}

struct EventState {
    flag: bool,
    /// Counts `set()` calls: Python's `Event.wait()` returns `True` when it was *notified*, even if
    /// the flag was cleared again before the waiter ran (`ResettableTimer.reset` relies on that).
    notified: u64,
}

/// `threading.Event`: "Class implementing event objects."
pub struct Event {
    _cond: Mutex<EventState>,
    cv: Condvar,
}

impl Event {
    pub fn new() -> Self {
        Event {
            _cond: Mutex::new(EventState {
                flag: false,
                notified: 0,
            }),
            cv: Condvar::new(),
        }
    }

    /// Return true if and only if the internal flag is true.
    pub fn is_set(&self) -> bool {
        lock(&self._cond).flag
    }

    /// Set the internal flag to true. All threads waiting for it to become true are awakened.
    pub fn set(&self) {
        let mut g = lock(&self._cond);
        g.flag = true;
        g.notified += 1;
        self.cv.notify_all();
    }

    /// Reset the internal flag to false.
    pub fn clear(&self) {
        lock(&self._cond).flag = false;
    }

    /// Block until the internal flag is true (or until `timeout` seconds). Returns `true` if the
    /// flag was set or the wait was notified, `false` on timeout.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn wait(&self, timeout: Option<f64>) -> bool {
        let mut g = lock(&self._cond);
        if g.flag {
            return true;
        }
        let seen = g.notified;
        let deadline = match timeout {
            None => None,
            Some(t) => match deadline_after(t) {
                Some(d) => Some(d),
                None => None,
            },
        };
        loop {
            let (ng, timed_out) = cond_wait(&self.cv, g, deadline);
            g = ng;
            if g.notified != seen {
                return true;
            }
            if timed_out {
                return false;
            }
        }
    }
}

impl Default for Event {
    fn default() -> Self {
        Self::new()
    }
}

/// `threading.RLock`: re-entrant lock, with Python's `acquire(blocking)` / `release()` pair.
pub struct RLock {
    state: Mutex<(Option<ThreadId>, usize)>,
    cv: Condvar,
}

impl RLock {
    pub fn new() -> Self {
        RLock {
            state: Mutex::new((None, 0)),
            cv: Condvar::new(),
        }
    }

    /// `acquire(blocking=True)`
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn acquire(&self, blocking: bool) -> bool {
        let me = std::thread::current().id();
        let mut g = lock(&self.state);
        loop {
            match g.0 {
                Some(owner) if owner == me => {
                    g.1 += 1;
                    return true;
                }
                None => {
                    g.0 = Some(me);
                    g.1 = 1;
                    return true;
                }
                Some(_) => {
                    if !blocking {
                        return false;
                    }
                    g = wait_cv(&self.cv, g);
                }
            }
        }
    }

    /// `release()`: `RuntimeError` if the calling thread does not own the lock.
    pub fn release(&self) -> Result<(), RuntimeError> {
        let me = std::thread::current().id();
        let mut g = lock(&self.state);
        if g.0 != Some(me) {
            return Err(RuntimeError("cannot release un-acquired lock"));
        }
        g.1 -= 1;
        if g.1 == 0 {
            g.0 = None;
            self.cv.notify_one();
        }
        Ok(())
    }

    /// `with lock:`
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn lock(&self) -> RLockGuard<'_> {
        self.acquire(true);
        RLockGuard(self)
    }
}

impl Default for RLock {
    fn default() -> Self {
        Self::new()
    }
}

pub struct RLockGuard<'a>(&'a RLock);

impl Drop for RLockGuard<'_> {
    fn drop(&mut self) {
        let _ = self.0.release();
    }
}

struct Finished {
    done: Mutex<bool>,
    cv: Condvar,
}

/// The new thread's end signals its starter if it never blocked (`_bootstrap_inner` ends, `:1044`).
struct EndsHandshake(Arc<Handshake>);

impl Drop for EndsHandshake {
    fn drop(&mut self) {
        let _ = STARTING.try_with(|s| s.borrow_mut().take());
        self.0.signal();
    }
}

struct FinishGuard(Arc<Finished>);

impl Drop for FinishGuard {
    fn drop(&mut self) {
        *lock(&self.0.done) = true;
        self.0.cv.notify_all();
    }
}

static NON_DAEMON: Mutex<Vec<Arc<Finished>>> = Mutex::new(Vec::new());

/// What Python does at interpreter exit (`threading._shutdown`): wait for every started thread
/// whose `daemon` flag is false. Daemon threads are simply left behind when the process ends.
/// Call it at the end of `main` if a non-daemon thread must be waited for.
pub fn threading_shutdown() {
    loop {
        let next = lock(&NON_DAEMON).pop();
        match next {
            Some(f) => {
                let mut g = lock(&f.done);
                while !*g {
                    g = wait_cv(&f.cv, g);
                }
            }
            None => break,
        }
    }
}

/// The start/join/daemon part of `threading.Thread`. The thread's `run()` is the closure given to
/// [`Thread::start`].
pub struct Thread {
    pub daemon: bool,
    started: Mutex<Option<Arc<Finished>>>,
}

impl Thread {
    pub fn new(daemon: bool) -> Self {
        Thread {
            daemon,
            started: Mutex::new(None),
        }
    }

    /// `start()`: `RuntimeError` if started before. Returns when the new thread has first blocked or
    /// ended (module doc, OURS).
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn start<F: FnOnce() + Send + 'static>(&self, run: F) -> Result<(), RuntimeError> {
        self.start_bounded(
            run,
            #[cfg(debug_assertions)]
            START_HANDSHAKE_BOUND,
        )
    }

    /// [`Thread::start`] with the watchdog's bound as an argument (debug builds), so that a test can
    /// give a short one.
    #[cfg_attr(debug_assertions, track_caller)]
    fn start_bounded<F: FnOnce() + Send + 'static>(
        &self,
        run: F,
        #[cfg(debug_assertions)] bound: Duration,
    ) -> Result<(), RuntimeError> {
        let mut s = lock(&self.started);
        if s.is_some() {
            return Err(RuntimeError("threads can only be started once"));
        }
        let fin = Arc::new(Finished {
            done: Mutex::new(false),
            cv: Condvar::new(),
        });
        let guard = FinishGuard(fin.clone());
        let handshake = Arc::new(Handshake {
            done: Mutex::new(false),
            cv: Condvar::new(),
        });
        let theirs = handshake.clone();
        #[cfg(debug_assertions)]
        let name = {
            let at = std::panic::Location::caller();
            format!(
                "{} (started at {}:{})",
                std::any::type_name::<F>(),
                at.file(),
                at.line()
            )
        };
        std::thread::Builder::new()
            .spawn(move || {
                // `_bootstrap_inner`: `self._started.set()` (:1033) is the starter's cue only once this thread
                // blocks (module doc)
                STARTING.with(|s| *s.borrow_mut() = Some(theirs.clone()));
                #[cfg(debug_assertions)]
                START_NAME.with(|n| *n.borrow_mut() = name);
                let _ends = EndsHandshake(theirs);
                let _guard = guard;
                run();
            })
            .map_err(|_| RuntimeError("can't start new thread"))?;
        if !self.daemon {
            lock(&NON_DAEMON).push(fin.clone());
        }
        *s = Some(fin);
        drop(s);
        // `self._started.wait()` (:982) is itself a blocking point of the starting thread
        blocking_point("Thread.start waits for _started");
        let mut done = handshake.done.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(debug_assertions)]
        let began = Instant::now();
        while !*done {
            #[cfg(debug_assertions)]
            {
                let left = bound.saturating_sub(began.elapsed());
                let (g, r) = handshake
                    .cv
                    .wait_timeout(done, left)
                    .unwrap_or_else(|e| e.into_inner());
                done = g;
                if r.timed_out() && !*done {
                    panic!(
                        "Thread.start() waited more than {bound:?} for the new thread's first blocking point: {}",
                        std::any::type_name::<F>()
                    );
                }
            }
            #[cfg(not(debug_assertions))]
            {
                done = handshake.cv.wait(done).unwrap_or_else(|e| e.into_inner());
            }
        }
        Ok(())
    }

    /// `join(timeout=None)`: `RuntimeError` if not started.
    pub fn join(&self, timeout: Option<f64>) -> Result<(), RuntimeError> {
        let fin = lock(&self.started)
            .clone()
            .ok_or(RuntimeError("cannot join thread before it is started"))?;
        let deadline = timeout.and_then(deadline_after);
        let mut g = lock(&fin.done);
        if !*g {
            blocking_point("Thread.join");
        }
        while !*g {
            let (ng, timed_out) =
                cond_wait(&fin.cv, g, if timeout.is_some() { deadline } else { None });
            g = ng;
            if timed_out && timeout.is_some() {
                break;
            }
        }
        Ok(())
    }

    /// `is_alive()`
    pub fn is_alive(&self) -> bool {
        match lock(&self.started).as_ref() {
            Some(f) => !*lock(&f.done),
            None => false,
        }
    }
}

/// CPython 3.13 `threading.py:1321 Timer(Thread)`: call a function after a specified number of
/// seconds. `Thread.__init__(self)` takes `daemon=None`, which is the creating thread's daemon flag
/// (`threading.py:906`); Rust threads have no such flag, so the caller passes it as `daemon`.
pub struct Timer {
    thread: Thread,
    interval: f64,
    function: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    pub finished: Arc<Event>,
}

impl Timer {
    /// `threading.py:1330 __init__(self, interval, function, args=None, kwargs=None)`: the
    /// arguments are captured by the closure.
    pub fn new(interval: f64, function: impl FnOnce() + Send + 'static, daemon: bool) -> Self {
        Timer {
            thread: Thread::new(daemon),
            interval,
            function: Mutex::new(Some(Box::new(function))),
            finished: Arc::new(Event::new()),
        }
    }

    /// `threading.py:1338 cancel`: stop the timer if it hasn't finished yet.
    pub fn cancel(&self) {
        self.finished.set();
    }

    /// `Thread.start()` running `threading.py:1342 run`.
    pub fn start(&self) -> Result<(), RuntimeError> {
        let function = lock(&self.function).take();
        let finished = self.finished.clone();
        let interval = self.interval;
        self.thread.start(move || {
            finished.wait(Some(interval));
            if !finished.is_set() {
                if let Some(f) = function {
                    f();
                }
            }
            finished.set();
        })
    }

    pub fn join(&self, timeout: Option<f64>) -> Result<(), RuntimeError> {
        self.thread.join(timeout)
    }

    pub fn is_alive(&self) -> bool {
        self.thread.is_alive()
    }
}

#[cfg(test)]
mod timer_tests {
    //! ours: `Timer` runs its function after the interval unless cancelled first.
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn timer_runs_after_interval() {
        let ran = Arc::new(AtomicBool::new(false));
        let r = ran.clone();
        let t = Timer::new(0.05, move || r.store(true, Ordering::SeqCst), true);
        t.start().unwrap();
        t.join(None).unwrap();
        assert!(ran.load(Ordering::SeqCst));
        assert!(t.finished.is_set());
    }

    #[test]
    fn timer_cancelled_does_not_run() {
        let ran = Arc::new(AtomicBool::new(false));
        let r = ran.clone();
        let t = Timer::new(5.0, move || r.store(true, Ordering::SeqCst), true);
        t.start().unwrap();
        t.cancel();
        t.join(None).unwrap();
        assert!(!ran.load(Ordering::SeqCst));
    }
}

#[cfg(test)]
mod start_tests {
    //! ours: `Thread.start()` under the GIL (module doc): it returns when the new thread has first
    //! blocked or ended.
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// A thread that does not block runs to its end before `start()` returns.
    #[test]
    fn start_returns_after_a_thread_that_never_blocks_has_ended() {
        for _ in 0..1000 {
            let ran = Arc::new(AtomicBool::new(false));
            let r = ran.clone();
            let t = Thread::new(true);
            t.start(move || r.store(true, Ordering::SeqCst)).unwrap();
            assert!(ran.load(Ordering::SeqCst));
        }
    }

    /// A thread runs up to its first blocking point (an `Event.wait`) before `start()` returns, and no
    /// further than that is promised: what it did before it blocked is visible, what it does after is not.
    #[test]
    fn start_returns_at_the_first_blocking_point() {
        for _ in 0..1000 {
            let before = Arc::new(AtomicBool::new(false));
            let flag = Arc::new(AtomicBool::new(false));
            let seen = Arc::new(AtomicBool::new(true));
            let gate = Arc::new(Event::new());
            let (b, f, s2, g) = (before.clone(), flag.clone(), seen.clone(), gate.clone());
            let t = Thread::new(true);
            t.start(move || {
                b.store(true, Ordering::SeqCst);
                s2.store(f.load(Ordering::SeqCst), Ordering::SeqCst);
                g.wait(None);
            })
            .unwrap();
            assert!(
                before.load(Ordering::SeqCst),
                "the thread ran to its first blocking point"
            );
            flag.store(true, Ordering::SeqCst); // set by the starter after start() returned
            assert!(
                !seen.load(Ordering::SeqCst),
                "the new thread saw the flag before the starter set it"
            );
            gate.set();
            t.join(None).unwrap();
        }
    }

    /// Nested: a starter that has not yet blocked lets its own starter continue when it calls `start()`
    /// (`_started.wait()` is a blocking point).
    #[test]
    fn a_thread_that_starts_a_thread_does_not_hold_up_its_own_starter() {
        let inner_ran = Arc::new(AtomicUsize::new(0));
        let i = inner_ran.clone();
        let outer = Thread::new(true);
        outer
            .start(move || {
                let inner = Thread::new(true);
                let i2 = i.clone();
                inner
                    .start(move || {
                        i2.fetch_add(1, Ordering::SeqCst);
                    })
                    .unwrap();
                assert_eq!(1, i.load(Ordering::SeqCst));
            })
            .unwrap();
        outer.join(None).unwrap();
        assert_eq!(1, inner_ran.load(Ordering::SeqCst));
    }

    /// The debug-only watchdog: a thread whose first blocking is not a blocking point (here a bare
    /// `std::thread::sleep`) holds the starter up, and `start()` panics after the bound.
    #[cfg(debug_assertions)]
    #[test]
    fn the_watchdog_panics_when_start_waits_longer_than_the_bound() {
        let t = Thread::new(true);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            t.start_bounded(
                || std::thread::sleep(Duration::from_millis(1500)),
                Duration::from_millis(200),
            )
            .unwrap();
        }));
        let err = result.expect_err("the watchdog fires");
        let text = err.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(text.contains("waited more than"), "{text}");
    }

    /// The bound is a named constant, 10 s.
    #[cfg(debug_assertions)]
    #[test]
    fn the_bound_is_ten_seconds() {
        assert_eq!(Duration::from_secs(10), START_HANDSHAKE_BOUND);
    }
}
