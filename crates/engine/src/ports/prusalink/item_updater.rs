//! PrusaLink's `printer_adapter/structures/item_updater.py` (`reference/Prusa-Link`, `171148ce`):
//! Implements classes for monitoring and updating arbitrary values (#245, section C).
//!
//! Python underneath: `RLock`, `Thread` and `multiprocessing.Event` (in one process the same as
//! `threading.Event` for what is used here: `set`, `clear`, `wait(timeout)`) are the CPython
//! ports ([`py_threading`](crate::ports::python::py_threading)), `queue.Queue` and
//! `queue.PriorityQueue` too ([`py_queue`](crate::ports::python::py_queue)), `blinker.Signal`
//! section A's ([`Signal`]).
//!
//! **Differs** (owner, 2026-11-27: approved):
//! 1. A value is Python's `Any`; here an item has a type parameter `V` ([`WatchedValue`]), and the
//!    updater's queues, its `items` and a group hold items of any `V` as [`Tracked`].
//!    `item.value` starts as Python's `None` (`:54`): `None` here, and `changed = value !=
//!    item.value` (`:469`) treats a `V` that is Python's `None` ([`WatchedValue::is_none`]) as
//!    equal to it.
//! 2. `item.in_groups` (`:57`) holds its groups weakly (Python's set holds them, and the groups
//!    hold the items, a cycle the garbage collector ends); the group's owner keeps it, as
//!    `printer_polling.py` does (`self.other_stuff`, `self.telemetry`). A group that is gone is
//!    skipped.
//! 3. Orders the source leaves undefined: `item.in_groups` is a set (`:273`, `:481` call the
//!    groups' handlers in its order), ours the order of making the groups; `ItemUpdater.items`
//!    and a group's `valid_items` and `invalid_items` are sets of names (a `WatchedItem` hashes
//!    and compares by its name, `:107-118`), only their membership is used.
//! 4. `time()` is the updater's [`Clock`], `time.time` unless given ([`ItemUpdater::with_clock`]):
//!    the comparison with PrusaLink patches the module's `time`.
//! 5. The bodies of the three threads' `while self.running:` loops (`:509-517`, `:527-545`,
//!    `:552-569`) are their own functions (`_refresher_once`, ...), so the comparison can run one
//!    turn; the loops call them. An exception in a thread ends that thread, as Python's does (its
//!    traceback is `threading.excepthook`'s).
//!
//! **Not covered:**
//! - Logging (`log.debug`, `log.warning`, `log.exception`): kept as comments, as in the other
//!   engine ports.
//! - `prctl_name()` (`:508`, `:526`, `:551`) and the threads' names (`polling`,
//!   `item_invalidator`, `polling_timeout`, `:200-208`): the system's thread names, as in
//!   section A.
//! - `add_item`'s `TypeError` for what is not a `WatchedItem` (`:238-239`): the types allow only
//!   items. `WatchedItem.__repr__` (`:104-105`): the object's address. A receiver, `gather`,
//!   `write` or `validation` function that changes the item's attributes other than through the
//!   updater; a write function that raises ("Shall not throw anything EVER!", `:92`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use indexmap::IndexSet;
use maker_connect_gcode_metadata::py::PyErr;

use super::blinker::Signal;
use super::util::time;
use crate::ports::python::py_queue::{Deque, GetError, PriorityQueue, PyCompare, Queue};
use crate::ports::python::py_threading::{Event, RLock, Thread};

/// `math.inf`
const INF: f64 = f64::INFINITY;

/// What an item's value can be (difference 1).
pub trait WatchedValue: PartialEq + Clone + Send + Sync + 'static {
    /// Is this Python's `None`?
    fn is_none(&self) -> bool {
        false
    }
}

impl<T: PartialEq + Clone + Send + Sync + 'static> WatchedValue for Option<T> {
    fn is_none(&self) -> bool {
        self.is_none()
    }
}

macro_rules! watched_values {
    ($($t:ty),*) => { $(impl WatchedValue for $t {})* };
}
watched_values!(bool, i64, u32, u64, f64, String);

impl<T: PartialEq + Clone + Send + Sync + 'static> WatchedValue for Vec<T> {}
impl<A, B> WatchedValue for (A, B)
where
    A: PartialEq + Clone + Send + Sync + 'static,
    B: PartialEq + Clone + Send + Sync + 'static,
{
}

/// `SideEffectOnly` (`:18-21`): An exception to raise in a gatherer that has nothing to return,
/// but its side effects succeeded in setting a value or have otherwise ensured that the value
/// would be received eventually. With any other exception a gatherer raises.
#[derive(Debug, Clone, PartialEq)]
pub enum GatherError {
    SideEffectOnly,
    Raised(PyErr),
}

/// `time()`: when, in seconds (difference 4).
pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;
/// A function that returns a value, or throws an error (`:85-88`)
pub type GatherFn<V> = Arc<dyn Fn() -> Result<V, GatherError> + Send + Sync>;
/// If valid, returns Ture, if not, throws an error or returns False (`:89-90`)
pub type ValidationFn<V> = Arc<dyn Fn(&V) -> Result<bool, PyErr> + Send + Sync>;
/// Takes care of putting the value in the right places. Shall not throw anything EVER! (`:91-93`)
pub type WriteFn<V> = Arc<dyn Fn(&V) + Send + Sync>;

/// `WatchedItem.default_on_fail_interval` (`:40-41`): Set to None to disable automatic refreshes
/// on read/validation fails
pub const DEFAULT_ON_FAIL_INTERVAL: Option<f64> = Some(5.0);

/// `WatchedItem.__init__`'s keyword arguments (`:44-51`).
pub struct ItemArgs<V> {
    pub gather_function: Option<GatherFn<V>>,
    pub write_function: Option<WriteFn<V>>,
    pub validation_function: Option<ValidationFn<V>>,
    pub interval: Option<f64>,
    pub timeout: Option<f64>,
    pub on_fail_interval: Option<f64>,
}

impl<V> Default for ItemArgs<V> {
    fn default() -> Self {
        ItemArgs {
            gather_function: None,
            write_function: None,
            validation_function: None,
            interval: None,
            timeout: None,
            on_fail_interval: DEFAULT_ON_FAIL_INTERVAL,
        }
    }
}

/// An item's attributes that change (`:29`, `:59-69`).
#[derive(Debug, Clone, PartialEq)]
pub struct ItemState {
    pub valid: bool,
    /// Are we scheduled for a value refresh
    pub scheduled: bool,
    /// If set, gets invalidated each interval. Imprecise timing intended
    pub interval: Option<f64>,
    /// If True, the interval is overridden with None
    pub disabled: bool,
    /// Refresh reschedule timeout
    pub on_fail_interval: Option<f64>,
    /// How long can we be invalid, before timing out
    pub timeout: Option<f64>,
    pub invalidate_at: f64,
    pub times_out_at: f64,
}

/// `WatchedItem(Watchable)` (`:35-118`): A value, that can be polled or set. Can be tracked in
/// the info updater
pub struct WatchedItem<V> {
    pub name: String,
    pub lock: RLock,
    pub state: Mutex<ItemState>,
    value: Mutex<Option<V>>,
    in_groups: Mutex<Vec<Weak<WatchedGroup>>>,
    gather_function: Option<GatherFn<V>>,
    validation_function: ValidationFn<V>,
    write_function: WriteFn<V>,
    // `Watchable` (`:24-32`)
    pub became_valid_signal: Signal<Arc<WatchedItem<V>>>,
    pub became_invalid_signal: Signal<Arc<WatchedItem<V>>>,
    // -- Signals --
    pub timed_out_signal: Signal<Arc<WatchedItem<V>>>,
    pub error_refreshing_signal: Signal<Arc<WatchedItem<V>>>,
    /// kwargs: validation exception
    pub validation_error_signal: Signal<Arc<WatchedItem<V>>>,
    /// sender is the value
    pub value_changed_signal: Signal<V>,
    /// Combined gather error signal
    pub val_err_timeout_signal: Signal<Arc<WatchedItem<V>>>,
    me: Weak<WatchedItem<V>>,
}

impl<V: WatchedValue> WatchedItem<V> {
    /// `__init__(name, gather_function=None, write_function=None, validation_function=None,
    /// interval=None, timeout=None, on_fail_interval=default_on_fail_interval)` (`:44-102`)
    pub fn new(name: &str, args: ItemArgs<V>) -> Arc<WatchedItem<V>> {
        // _default_validation (`:71-73`), _default_write (`:75-77`)
        let validation_function = args
            .validation_function
            .unwrap_or_else(|| Arc::new(|_: &V| Ok(true)));
        let write_function = args.write_function.unwrap_or_else(|| Arc::new(|_: &V| {}));
        Arc::new_cyclic(|me| WatchedItem {
            name: name.to_owned(),
            lock: RLock::new(),
            state: Mutex::new(ItemState {
                valid: false,
                scheduled: false,
                interval: args.interval,
                disabled: false,
                on_fail_interval: args.on_fail_interval,
                timeout: args.timeout,
                invalidate_at: INF,
                times_out_at: INF,
            }),
            value: Mutex::new(None),
            in_groups: Mutex::new(vec![]),
            gather_function: args.gather_function,
            validation_function,
            write_function,
            became_valid_signal: Signal::new(),
            became_invalid_signal: Signal::new(),
            timed_out_signal: Signal::new(),
            error_refreshing_signal: Signal::new(),
            validation_error_signal: Signal::new(),
            value_changed_signal: Signal::new(),
            val_err_timeout_signal: Signal::new(),
            me: me.clone(),
        })
    }

    /// `item.value`: `None` until a value is set.
    pub fn value(&self) -> Option<V> {
        self.value.lock().unwrap().clone()
    }

    fn me(&self) -> Arc<WatchedItem<V>> {
        self.me
            .upgrade()
            .expect("a WatchedItem is used through its Arc")
    }
}

/// A [`WatchedItem`] of any value type: what the updater's queues, its `items` and a group hold.
pub trait Tracked: Send + Sync {
    fn name(&self) -> &str;
    fn lock(&self) -> &RLock;
    fn state(&self) -> &Mutex<ItemState>;
    /// `item.in_groups`, those still alive
    fn in_groups(&self) -> Vec<Arc<WatchedGroup>>;
    /// `item.in_groups.add(group)`
    fn add_group(&self, group: &Arc<WatchedGroup>);
    /// `item.became_invalid_signal.send(item)`
    fn send_became_invalid(&self);
    /// `ItemUpdater._time_out`'s signals: `timed_out_signal`, `val_err_timeout_signal`
    fn send_timed_out(&self);
    /// `ItemUpdater._gather(item)`
    fn gather(&self, updater: &ItemUpdater) -> Result<(), PyErr>;
}

impl<V: WatchedValue> Tracked for WatchedItem<V> {
    fn name(&self) -> &str {
        &self.name
    }
    fn lock(&self) -> &RLock {
        &self.lock
    }
    fn state(&self) -> &Mutex<ItemState> {
        &self.state
    }
    fn in_groups(&self) -> Vec<Arc<WatchedGroup>> {
        self.in_groups
            .lock()
            .unwrap()
            .iter()
            .filter_map(Weak::upgrade)
            .collect()
    }
    fn add_group(&self, group: &Arc<WatchedGroup>) {
        let mut groups = self.in_groups.lock().unwrap();
        if !groups
            .iter()
            .any(|g| std::ptr::eq(g.as_ptr(), Arc::as_ptr(group)))
        {
            groups.push(Arc::downgrade(group));
        }
    }
    fn send_became_invalid(&self) {
        self.became_invalid_signal.send(&self.me());
    }
    fn send_timed_out(&self) {
        let me = self.me();
        self.timed_out_signal.send(&me);
        self.val_err_timeout_signal.send(&me);
    }
    fn gather(&self, updater: &ItemUpdater) -> Result<(), PyErr> {
        updater._gather(&self.me())
    }
}

/// An item in a timer queue: `(when, item)` tuples compare the item by its name
/// (`WatchedItem.__lt__`, `__eq__`, `:107-115`).
#[derive(Clone)]
pub struct ItemKey(pub Arc<dyn Tracked>);

impl PyCompare for ItemKey {
    fn py_eq(&self, other: &Self) -> bool {
        self.0.name() == other.0.name()
    }
    fn py_lt(&self, other: &Self) -> bool {
        self.0.name() < other.0.name()
    }
}

/// `WatchedGroup(Watchable)` (`:121-175`): A group of watched items. Aggregates the validity
/// signals from its members
pub struct WatchedGroup {
    pub valid: Mutex<bool>,
    pub became_valid_signal: Signal<Arc<WatchedGroup>>,
    pub became_invalid_signal: Signal<Arc<WatchedGroup>>,
    pub all_items: Vec<Arc<dyn Tracked>>,
    pub valid_items: Mutex<IndexSet<String>>,
    pub invalid_items: Mutex<IndexSet<String>>,
    me: Weak<WatchedGroup>,
}

/// `set.remove(item)` of a missing one
fn key_error(item: &dyn Tracked) -> PyErr {
    PyErr::KeyError(item.name().to_owned())
}

impl WatchedGroup {
    /// `__init__(items)` (`:127-148`)
    pub fn new(items: Vec<Arc<dyn Tracked>>) -> Result<Arc<WatchedGroup>, PyErr> {
        if items.is_empty() {
            return Err(PyErr::ValueError(
                "Supply at least one item, or group to be watched".to_owned(),
            ));
        }

        let mut valid_items = IndexSet::new();
        let mut invalid_items = IndexSet::new();
        for item in &items {
            if item.state().lock().unwrap().valid {
                valid_items.insert(item.name().to_owned());
            } else {
                invalid_items.insert(item.name().to_owned());
            }
        }
        let valid = invalid_items.is_empty();

        let group = Arc::new_cyclic(|me| WatchedGroup {
            valid: Mutex::new(valid),
            became_valid_signal: Signal::new(),
            became_invalid_signal: Signal::new(),
            all_items: items,
            valid_items: Mutex::new(valid_items),
            invalid_items: Mutex::new(invalid_items),
            me: me.clone(),
        });
        // Tracking using these signals,
        for item in &group.all_items {
            item.add_group(&group);
        }
        Ok(group)
    }

    /// `__iter__` (`:150-151`)
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Tracked>> {
        self.all_items.iter()
    }

    /// `invalid_handler(item)` (`:153-163`): A member became invalid. Moves the member to the
    /// invalid pile. If the group was valid, it's not anymore and that gets signalled
    pub fn invalid_handler(&self, item: &dyn Tracked) -> Result<(), PyErr> {
        if !self.valid_items.lock().unwrap().shift_remove(item.name()) {
            return Err(key_error(item));
        }
        self.invalid_items
            .lock()
            .unwrap()
            .insert(item.name().to_owned());

        let was_valid = std::mem::replace(&mut *self.valid.lock().unwrap(), false);
        if was_valid {
            self.became_invalid_signal.send(&self.me.upgrade().unwrap());
        }
        Ok(())
    }

    /// `valid_handler(item)` (`:165-175`): A member became valid. Moves the member to the valid
    /// pile. If all members are valid, sends a signal
    pub fn valid_handler(&self, item: &dyn Tracked) -> Result<(), PyErr> {
        if !self.invalid_items.lock().unwrap().shift_remove(item.name()) {
            return Err(key_error(item));
        }
        self.valid_items
            .lock()
            .unwrap()
            .insert(item.name().to_owned());

        let becomes_valid = {
            let mut valid = self.valid.lock().unwrap();
            let becomes = !*valid && self.invalid_items.lock().unwrap().is_empty();
            if becomes {
                *valid = true;
            }
            becomes
        };
        if becomes_valid {
            self.became_valid_signal.send(&self.me.upgrade().unwrap());
        }
        Ok(())
    }
}

type Timers = Queue<PriorityQueue<(f64, ItemKey)>>;

struct Inner {
    quit_interval: f64,
    running: AtomicBool,
    invalidate_timers: Timers,
    invalidate_queue_event: Event,
    timeout_timers: Timers,
    timeout_queue_event: Event,
    refresh_queue: Queue<Deque<Arc<dyn Tracked>>>,
    refresher_thread: Thread,
    invalidator_thread: Thread,
    timeout_thread: Thread,
    items: Mutex<IndexSet<String>>,
    time: Clock,
}

/// `ItemUpdater` (`:178-569`)
///
/// This governs some defined variables
///
/// Variables can be made to be refreshed manually, or on a timer. Variable getters can time out,
/// which sends out a signal. Variables can be validated. On validation or read error, variable
/// refresh can be re-scheduled automatically on a timer
#[derive(Clone)]
pub struct ItemUpdater {
    inner: Arc<Inner>,
}

impl Default for ItemUpdater {
    fn default() -> Self {
        ItemUpdater::new(0.2)
    }
}

impl ItemUpdater {
    /// `__init__(quit_interval=0.2)` (`:189-210`)
    pub fn new(quit_interval: f64) -> ItemUpdater {
        ItemUpdater::with_clock(quit_interval, Arc::new(time))
    }

    /// `__init__(quit_interval)` with `time()` as given (difference 4).
    pub fn with_clock(quit_interval: f64, time: Clock) -> ItemUpdater {
        ItemUpdater {
            inner: Arc::new(Inner {
                quit_interval,
                running: AtomicBool::new(true),
                invalidate_timers: Queue::new(0, PriorityQueue::new()),
                invalidate_queue_event: Event::new(),
                timeout_timers: Queue::new(0, PriorityQueue::new()),
                timeout_queue_event: Event::new(),
                refresh_queue: Queue::new(0, Deque::new()),
                refresher_thread: Thread::new(true),
                invalidator_thread: Thread::new(true),
                timeout_thread: Thread::new(true),
                items: Mutex::new(IndexSet::new()),
                time,
            }),
        }
    }

    fn time(&self) -> f64 {
        (self.inner.time)()
    }

    /// `self.running`
    pub fn running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }

    /// `self.running = value`
    pub fn set_running(&self, value: bool) {
        self.inner.running.store(value, Ordering::SeqCst);
    }

    /// `self.items`: the names (difference 3)
    pub fn items(&self) -> IndexSet<String> {
        self.inner.items.lock().unwrap().clone()
    }

    /// `start` (`:212-216`): Starts up the governing threads
    pub fn start(&self) -> Result<(), PyErr> {
        let runtime = |e: crate::ports::python::py_threading::RuntimeError| {
            PyErr::Other("RuntimeError", e.0.to_owned())
        };
        let me = self.clone();
        self.inner
            .refresher_thread
            .start(move || me._refresher())
            .map_err(runtime)?;
        let me = self.clone();
        self.inner
            .invalidator_thread
            .start(move || me._process_invalidations())
            .map_err(runtime)?;
        let me = self.clone();
        self.inner
            .timeout_thread
            .start(move || me._process_timeouts())
            .map_err(runtime)
    }

    /// `stop` (`:218-222`): Stops the value tracker
    pub fn stop(&self) {
        self.set_running(false);
        self.inner.invalidate_queue_event.set();
        self.inner.timeout_queue_event.set();
    }

    /// `wait_stopped` (`:224-228`): waits for the value tracker to quit
    pub fn wait_stopped(&self) -> Result<(), PyErr> {
        let runtime = |e: crate::ports::python::py_threading::RuntimeError| {
            PyErr::Other("RuntimeError", e.0.to_owned())
        };
        self.inner.invalidator_thread.join(None).map_err(runtime)?;
        self.inner.timeout_thread.join(None).map_err(runtime)?;
        self.inner.refresher_thread.join(None).map_err(runtime)
    }

    /// `add_item(item, start_tracking=True)` (`:230-242`): Only invalid items can be added for
    /// now. `start_tracking`: Whether to invalidate the item. Without this, the item does not
    /// gather its value and has to be invalidated manually
    pub fn add_item<V: WatchedValue>(
        &self,
        item: &Arc<WatchedItem<V>>,
        start_tracking: bool,
    ) -> Result<(), PyErr> {
        self.inner.items.lock().unwrap().insert(item.name.clone());
        if start_tracking {
            self.invalidate(item)?;
        }
        Ok(())
    }

    /// `invalidate_group(group)` (`:244-249`): Invalidates every item of the supplied
    /// WatchedGroup
    pub fn invalidate_group(&self, group: &WatchedGroup) -> Result<(), PyErr> {
        for group_item in group.iter() {
            self.invalidate_dyn(group_item)?;
        }
        Ok(())
    }

    /// `invalidate(item)` (`:251-278`): Invalidates the item, putting it into the queue for
    /// validation. If the object has a timeout, sets up the timer for it
    ///
    /// Calling repeatedly should not affect anything, the first invalidation matters
    ///
    /// If the item already is invalidated but is not scheduled for a refresh, it gets scheduled
    pub fn invalidate<V: WatchedValue>(&self, item: &Arc<WatchedItem<V>>) -> Result<(), PyErr> {
        self.invalidate_dyn(&(item.clone() as Arc<dyn Tracked>))
    }

    /// [`ItemUpdater::invalidate`] of an item of any value type.
    pub fn invalidate_dyn(&self, item: &Arc<dyn Tracked>) -> Result<(), PyErr> {
        self._validate_is_tracked(item.as_ref())?;

        let _lock = item.lock().lock();
        if item.state().lock().unwrap().disabled {
            // log.debug("Will not invalidate item %s because it's disabled.",
            //           item.name)
            return Ok(());
        }
        // log.debug("Item %s has been invalidated", item.name)
        let was_valid = {
            let mut state = item.state().lock().unwrap();
            state.invalidate_at = INF;
            std::mem::replace(&mut state.valid, false)
        };
        if was_valid {
            for group in item.in_groups() {
                group.invalid_handler(item.as_ref())?;
            }
            item.send_became_invalid();
        }

        if !item.state().lock().unwrap().scheduled {
            self._enqueue_refresh(item);
        }
        Ok(())
    }

    /// `disable(item)` (`:280-288`): Disables the item polling without changing its interval
    pub fn disable<V: WatchedValue>(&self, item: &Arc<WatchedItem<V>>) -> Result<(), PyErr> {
        self._validate_is_tracked(item.as_ref())?;

        let _lock = item.lock.lock();
        if item.state.lock().unwrap().disabled {
            return Ok(());
        }
        item.state.lock().unwrap().disabled = true;
        self.cancel_scheduled_invalidation(item)
    }

    /// `enable(item)` (`:290-298`): Enables the item polling without changing its interval
    pub fn enable<V: WatchedValue>(&self, item: &Arc<WatchedItem<V>>) -> Result<(), PyErr> {
        self._validate_is_tracked(item.as_ref())?;

        let _lock = item.lock.lock();
        if !item.state.lock().unwrap().disabled {
            return Ok(());
        }
        item.state.lock().unwrap().disabled = false;
        self.invalidate(item)
    }

    /// `set_value(item, value)` (`:300-327`): Validates the value and writes it
    ///
    /// Forcefully re-schedules invalidation. This can be used to enable polling, when auto
    /// reporting stops for example
    pub fn set_value<V: WatchedValue>(
        &self,
        item: &Arc<WatchedItem<V>>,
        value: V,
    ) -> Result<(), PyErr> {
        self._validate_is_tracked(item.as_ref())?;

        let _lock = item.lock.lock();
        // if not item.validation_function(value):
        //     raise ValueError(f"Invalid value for {item.name}: {value}")
        // except Exception:
        if (item.validation_function)(&value) != Ok(true) {
            // log.debug("Validation of item %s has failed", item.name)
            item.validation_error_signal.send(item);
            item.val_err_timeout_signal.send(item);

            // If the item is valid, do not schedule a gather, as this
            // probably was a setter from the outside with a bad value
            if !item.state.lock().unwrap().valid {
                self._gather_error_reschedule(item)?;
            }
        } else {
            // log.debug("Value of item %s has been determined to be %s",
            //           item.name, value)
            self._set_value(item, value)?;
        }
        Ok(())
    }

    /// `schedule_invalidation(item, interval=None, reschedule=False)` (`:329-372`): Schedules an
    /// item invalidation at a certain time. Will not shift already scheduled invalidation unless
    /// forced to
    ///
    /// If an already invalid item is scheduled for example after a gather/validation error, it is
    /// just added to the refresh queue without emitting any additional signals. `interval`: How
    /// long in the future should we invalidate? If left empty, the default is used, if that's None
    /// an error will be raised. `reschedule`: If an invalidation is already scheduled, it won't
    /// get re-scheduled unless this is True
    pub fn schedule_invalidation<V: WatchedValue>(
        &self,
        item: &Arc<WatchedItem<V>>,
        interval: Option<f64>,
        reschedule: bool,
    ) -> Result<(), PyErr> {
        self._validate_is_tracked(item.as_ref())?;

        let _lock = item.lock.lock();
        let mut state = item.state.lock().unwrap();
        if state.disabled {
            // log.debug("Will not schedule item %s because it is disabled.",
            //           item.name)
            return Ok(());
        }
        if state.invalidate_at != INF && !reschedule {
            // log.debug(
            //     "Will not schedule an invalidation for item %s because "
            //     "another is already scheduled", item.name)
            return Ok(());
        }

        let interval = interval.or(state.interval);

        let Some(interval) = interval else {
            return Err(PyErr::AttributeError(format!(
                "No interval specified for item {} has no default and none has been provided!",
                item.name
            )));
        };

        // log.debug(
        //     "Scheduling invalidation of item %s for %ss in "
        //     "the future", item.name, interval)
        state.invalidate_at = self.time() + interval;
        let invalidate_at = state.invalidate_at;
        drop(state);
        self.inner
            .invalidate_timers
            .put((invalidate_at, ItemKey(item.clone())), true, None)
            .expect("an unbounded queue that is not shut down takes every item");
        self.inner.invalidate_queue_event.set();
        Ok(())
    }

    /// `cancel_scheduled_invalidation(item)` (`:374-388`): Cancels the scheduled invalidation.
    /// The timer itself cannot be cancelled, but the invalidate_at value has to match before
    /// anything is executed. Changing it to infinity will accomplish that nicely
    pub fn cancel_scheduled_invalidation<V: WatchedValue>(
        &self,
        item: &Arc<WatchedItem<V>>,
    ) -> Result<(), PyErr> {
        self._validate_is_tracked(item.as_ref())?;

        let _lock = item.lock.lock();
        let mut state = item.state.lock().unwrap();
        if state.invalidate_at == INF {
            return Ok(());
        }
        // log.debug("Cancelling scheduled invalidation of item %s ",
        //           item.name)
        state.invalidate_at = INF;
        Ok(())
    }

    // -- Private --

    /// `_time_out(item)` (`:392-403`): Times out the item, notifying everyone of the fail
    pub fn _time_out(item: &dyn Tracked) {
        let _lock = item.lock().lock();
        // log.warning("Timed out when getting item %s", item.name)
        item.state().lock().unwrap().times_out_at = INF;
        item.send_timed_out();
    }

    /// `_validate_is_tracked(item)` (`:405-408`)
    fn _validate_is_tracked(&self, item: &dyn Tracked) -> Result<(), PyErr> {
        if !self.inner.items.lock().unwrap().contains(item.name()) {
            return Err(PyErr::ValueError(format!(
                "Item {} is not tracked by this instance.",
                item.name()
            )));
        }
        Ok(())
    }

    /// `_gather(item)` (`:410-449`): Refreshes the item value, if the item has a refresh
    /// interval, sets up the timed invalidation
    ///
    /// If the value gathering throws an error, it re-schedules its refresh and notifies of a fail
    pub fn _gather<V: WatchedValue>(&self, item: &Arc<WatchedItem<V>>) -> Result<(), PyErr> {
        if item.state.lock().unwrap().valid {
            return Ok(());
        }

        // Items without gather functions have no point in spinning,
        // something else needs to take care of them
        let Some(gather_function) = &item.gather_function else {
            return Ok(());
        };

        // log.debug("Gathering new value for item %s", item.name)
        match gather_function() {
            Err(GatherError::SideEffectOnly) => {
                // Special case for gatherers with just side effects
                // Useful for when the value is autoreported and gather needs
                // to only turn the reporting on

                // If the gatherer sets its own items value, then let's not
                // re-schedule anything
                if !item.state.lock().unwrap().valid {
                    // Counting on set_item cancelling the re-schedule
                    self._gather_error_reschedule(item)?;
                }
            }
            Err(GatherError::Raised(_)) => {
                let _lock = item.lock.lock();
                // log.exception("Gather of %s has failed", item.name)
                item.error_refreshing_signal.send(item);
                item.val_err_timeout_signal.send(item);
                self._gather_error_reschedule(item)?;
            }
            Ok(value) => {
                let _lock = item.lock.lock();
                self.set_value(item, value)?;
            }
        }
        Ok(())
    }

    /// `_gather_error_reschedule(item)` (`:451-461`): Reschedules the value refresh on gather or
    /// validation errors. Reschedules only if the reschedule interval is set (default = 5s)
    fn _gather_error_reschedule<V: WatchedValue>(
        &self,
        item: &Arc<WatchedItem<V>>,
    ) -> Result<(), PyErr> {
        let _lock = item.lock.lock();
        let on_fail_interval = item.state.lock().unwrap().on_fail_interval;
        if let Some(on_fail_interval) = on_fail_interval {
            // log.debug(
            //     "Rescheduling gather of item %s for "
            //     "%ss in the future", item.name, item.on_fail_interval)
            self.schedule_invalidation(item, Some(on_fail_interval), false)?;
        }
        Ok(())
    }

    /// `_set_value(item, value)` (`:463-485`): Internal, only sets the value without validation.
    /// Should be pre-validate before this gets called
    fn _set_value<V: WatchedValue>(
        &self,
        item: &Arc<WatchedItem<V>>,
        value: V,
    ) -> Result<(), PyErr> {
        let _lock = item.lock.lock();
        let changed = match &*item.value.lock().unwrap() {
            // value != None
            None => !value.is_none(),
            Some(old) => value != *old,
        };
        if changed {
            // log.debug("Item %s got a new value! old: %s new: %s",
            //           item.name, item.value, value)
        }
        *item.value.lock().unwrap() = Some(value.clone());
        (item.write_function)(&value);
        let (was_invalid, interval) = {
            let mut state = item.state.lock().unwrap();
            let was_invalid = !state.valid;
            state.valid = true;
            state.times_out_at = INF;
            (was_invalid, state.interval)
        };
        if interval.is_some() {
            self.schedule_invalidation(item, None, true)?;
        }
        if was_invalid {
            for group in Tracked::in_groups(item.as_ref()) {
                group.valid_handler(item.as_ref())?;
            }
            item.became_valid_signal.send(item);
        }
        if changed {
            item.value_changed_signal.send(&value);
        }
        Ok(())
    }

    /// `_enqueue_refresh(item)` (`:487-502`): Forcefully enqueues the item for refresh. Does not
    /// re-schedule the time out. If the item failed to gather for example, it gets re-scheduled.
    /// But has to time out in the set time since it is invalid for more than X seconds
    fn _enqueue_refresh(&self, item: &Arc<dyn Tracked>) {
        let _lock = item.lock().lock();
        let mut state = item.state().lock().unwrap();
        if let Some(timeout) = state.timeout
            && state.times_out_at == INF
        {
            state.times_out_at = self.time() + timeout;
            let times_out_at = state.times_out_at;
            drop(state);
            self.inner
                .timeout_timers
                .put((times_out_at, ItemKey(item.clone())), true, None)
                .expect("an unbounded queue that is not shut down takes every item");
            state = item.state().lock().unwrap();
        }

        state.scheduled = true;
        drop(state);
        self.inner
            .refresh_queue
            .put(item.clone(), true, None)
            .expect("an unbounded queue that is not shut down takes every item");
    }

    /// `_refresher` (`:504-517`): Processes all values queued up for refreshing
    fn _refresher(&self) {
        // prctl_name()
        while self.running() {
            if self._refresher_once().is_err() {
                return;
            }
        }
    }

    /// One turn of `_refresher`'s loop (`:510-517`, difference 5).
    pub fn _refresher_once(&self) -> Result<(), PyErr> {
        match self
            .inner
            .refresh_queue
            .get(true, Some(self.inner.quit_interval))
        {
            Err(GetError::Empty) => {}
            Err(other) => return Err(get_error(other)),
            Ok(item) => {
                {
                    let _lock = item.lock().lock();
                    item.state().lock().unwrap().scheduled = false;
                }
                item.gather(self)?;
            }
        }
        Ok(())
    }

    /// `_process_invalidations` (`:519-545`): Processes the invalidation queue. If a timer is
    /// checked and does not match with the set timer on an item, it is discarded, so only valid
    /// timers call their callbacks
    fn _process_invalidations(&self) {
        // prctl_name()
        while self.running() {
            if self._process_invalidations_once().is_err() {
                return;
            }
        }
    }

    /// One turn of `_process_invalidations`' loop (`:528-545`, difference 5).
    pub fn _process_invalidations_once(&self) -> Result<(), PyErr> {
        match self
            .inner
            .invalidate_timers
            .get(true, Some(self.inner.quit_interval))
        {
            Err(GetError::Empty) => {}
            Err(other) => return Err(get_error(other)),
            Ok((invalidate_at, ItemKey(item))) => {
                // Check if the timer is valid
                if invalidate_at != item.state().lock().unwrap().invalidate_at {
                    return Ok(());
                }

                let current_time = self.time();
                if invalidate_at > current_time {
                    self.inner
                        .invalidate_timers
                        .put((invalidate_at, ItemKey(item)), true, None)
                        .expect("an unbounded queue that is not shut down takes every item");
                    self.inner
                        .invalidate_queue_event
                        .wait(Some(invalidate_at - current_time));
                    self.inner.invalidate_queue_event.clear();
                } else {
                    self.invalidate_dyn(&item)?;
                }
            }
        }
        Ok(())
    }

    /// `_process_timeouts` (`:547-569`): Same as invalidators, except its timeouts
    fn _process_timeouts(&self) {
        // prctl_name()
        while self.running() {
            if self._process_timeouts_once().is_err() {
                return;
            }
        }
    }

    /// One turn of `_process_timeouts`' loop (`:553-569`, difference 5).
    pub fn _process_timeouts_once(&self) -> Result<(), PyErr> {
        match self
            .inner
            .timeout_timers
            .get(true, Some(self.inner.quit_interval))
        {
            Err(GetError::Empty) => {}
            Err(other) => return Err(get_error(other)),
            Ok((times_out_at, ItemKey(item))) => {
                // Check if the timer is valid
                if times_out_at != item.state().lock().unwrap().times_out_at {
                    return Ok(());
                }

                let current_time = self.time();
                if times_out_at > current_time {
                    self.inner
                        .timeout_timers
                        .put((times_out_at, ItemKey(item)), true, None)
                        .expect("an unbounded queue that is not shut down takes every item");
                    self.inner
                        .timeout_queue_event
                        .wait(Some(times_out_at - current_time));
                    self.inner.timeout_queue_event.clear();
                } else {
                    ItemUpdater::_time_out(item.as_ref());
                }
            }
        }
        Ok(())
    }

    /// `invalidate_queue_event.set()`, `timeout_queue_event.set()`: what the comparison uses to
    /// make a turn's `wait` return at once.
    pub fn set_events(&self) {
        self.inner.invalidate_queue_event.set();
        self.inner.timeout_queue_event.set();
    }

    /// `len(queue.queue)` of the refresh queue and both timer queues, in their order.
    pub fn queued(&self) -> (Vec<String>, Vec<(f64, String)>, Vec<(f64, String)>) {
        let refresh = self
            .inner
            .refresh_queue
            .lock()
            .queue
            .queue
            .iter()
            .map(|i| i.name().to_owned())
            .collect();
        let timers = |q: &Timers| {
            q.lock()
                .queue
                .queue
                .iter()
                .map(|(t, ItemKey(i))| (*t, i.name().to_owned()))
                .collect()
        };
        (
            refresh,
            timers(&self.inner.invalidate_timers),
            timers(&self.inner.timeout_timers),
        )
    }
}

/// A `queue.get` error other than `Empty`, as the exception.
fn get_error(error: GetError) -> PyErr {
    match error {
        GetError::Empty => PyErr::Other("Empty", String::new()),
        GetError::ShutDown => PyErr::Other("ShutDown", String::new()),
        GetError::ValueError(message) => PyErr::ValueError(message),
    }
}

#[cfg(test)]
mod tests;
