//! PrusaLink's `serial/is_planner_fed.py` (`reference/Prusa-Link`, `171148ce`): Contains
//! implementation of the IsPlannerFed class, with HeapName and TimeValue classes. Tries to guess,
//! whether the printer planner is full (#245, section C).
//!
//! The constants it reads (`const.py`: `QUEUE_SIZE`, `HEAP_RATIO`, `IGNORE_ABOVE`,
//! `DEFAULT_THRESHOLD`, `USE_DYNAMIC_THRESHOLD`) are module globals there, read when used, and
//! PrusaLink's own tests (`tests/test_is_planner_fed.py`) patch them; here they are a
//! [`Constants`] the planner holds, [`Constants::default`] being `const.py`'s.
//!
//! The Python underneath: `float(str)` is [`py_float`], `str(float)` [`py_float_repr`],
//! `round()` rounds half to even, an index into an empty heap is `IndexError`, as is `popleft` of
//! an empty `deque`.
//!
//! **Not covered:**
//! - Logging (`log.debug` in `process_value`): kept as a comment, as in the other engine ports.
//! - A negative `QUEUE_SIZE` (`deque(maxlen=-1)` is a `ValueError`): the size is a `usize`.

use std::collections::VecDeque;
use std::io::Write;

use maker_connect_gcode_metadata::py::PyErr;
use maker_connect_sdk::pyos;

use super::consts::{
    DEFAULT_THRESHOLD, HEAP_RATIO, IGNORE_ABOVE, QUEUE_SIZE, USE_DYNAMIC_THRESHOLD,
};
use super::heap::{AsHeapItem, HeapItem, MaxHeap, MinHeap, Shared};
use super::util::{ensure_directory, get_clean_path, os_error};
use crate::ports::octoprint::comm::{py_float, py_float_repr};

/// The module globals of `const.py` that `is_planner_fed.py` imports (`:11-17`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Constants {
    pub queue_size: usize,
    pub heap_ratio: f64,
    pub ignore_above: f64,
    pub default_threshold: f64,
    pub use_dynamic_threshold: bool,
}

impl Default for Constants {
    fn default() -> Self {
        Constants {
            queue_size: QUEUE_SIZE as usize,
            heap_ratio: HEAP_RATIO,
            ignore_above: IGNORE_ABOVE,
            default_threshold: DEFAULT_THRESHOLD,
            use_dynamic_threshold: USE_DYNAMIC_THRESHOLD,
        }
    }
}

/// `HeapName` (`:24-27`): Heap name enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapName {
    ShortTimes,
    LongTimes,
}

impl HeapName {
    /// `.value`
    pub fn value(self) -> &'static str {
        match self {
            HeapName::ShortTimes => "SHORT_TIMES",
            HeapName::LongTimes => "LONG_TIMES",
        }
    }
}

/// `TimeValue(HeapItem)` (`:30-35`): Time value with info in which queue it currently resides
#[derive(Debug, Clone, PartialEq)]
pub struct TimeValue {
    pub item: HeapItem,
    pub heap_name: Option<HeapName>,
}

impl TimeValue {
    /// `__init__(value)` (`:33-35`)
    pub fn new(value: f64) -> TimeValue {
        TimeValue {
            item: HeapItem::new(value),
            heap_name: None,
        }
    }
}

impl AsHeapItem for TimeValue {
    fn heap_item(&self) -> &HeapItem {
        &self.item
    }
    fn heap_item_mut(&mut self) -> &mut HeapItem {
        &mut self.item
    }
}

/// `MinHeap.__getitem__(0).value`: `self.heap[0]`, `IndexError` when empty.
fn root_value(heap: &MinHeap<TimeValue>) -> Result<f64, PyErr> {
    match heap.get(0) {
        Some(item) => Ok(item.lock().unwrap().item.value),
        None => Err(PyErr::Other(
            "IndexError",
            "list index out of range".to_owned(),
        )),
    }
}

/// `IsPlannerFed` (`:38-216`)
///
/// If the planner queue is full, I expect the printer to take longer when confirming print
/// instructions, if the time surpasses a threshold, I assume full buffer. To stay future-proof,
/// let's compute this threshold on the go.
///
/// Let's measure the times for all instructions, disqualifying the ones that took too long. Now
/// the threshold computation mimics the way one would compute a moving median. I use the two heaps
/// approach.
///
/// left heap is a max_heap, the right one is a min_heap, when a number comes, I compare it with
/// the threshold and depending on the result I put it into one of the heaps. If that throws the
/// ratio of element counts off, the heap that is larger than supposed to gives its root to the
/// smaller one.
///
/// The threshold is an average between the two roots.
///
/// After the queue is full, the heaps shed the oldest values, so it can adapt, if for some reason
/// the print commands start taking different amounts of time during the print. Problems can arise
/// in hi-res cylindrical vases and other shapes with homogeneously long segments.
///
/// To get rid of the inaccuracies caused by an initially low number of measured values, let's use
/// a threshold from a previous run, or a default one until the values accumulate.
pub struct IsPlannerFed {
    pub consts: Constants,
    /// `deque(maxlen=QUEUE_SIZE)`, its `maxlen` fixed when made
    pub times_queue: VecDeque<Shared<TimeValue>>,
    maxlen: usize,
    pub threshold_path: String,
    pub default_threshold: f64,
    pub is_fed: bool,
    pub short_times: MaxHeap<TimeValue>,
    pub long_times: MinHeap<TimeValue>,
}

impl IsPlannerFed {
    /// `__init__(threshold_path)` (`:66-85`), with `const.py`'s constants.
    pub fn new(threshold_path: &str) -> Result<IsPlannerFed, PyErr> {
        IsPlannerFed::with_constants(threshold_path, Constants::default())
    }

    /// `__init__(threshold_path)` (`:66-85`), the constants as given.
    pub fn with_constants(threshold_path: &str, consts: Constants) -> Result<IsPlannerFed, PyErr> {
        let times_queue = VecDeque::new();
        let maxlen = consts.queue_size;

        let threshold_path = get_clean_path(threshold_path);
        ensure_directory(pyos::dirname(&threshold_path), None)?;

        let default_threshold = if !consts.use_dynamic_threshold {
            consts.default_threshold
        } else {
            // with open(self.threshold_path, encoding='utf-8') as threshold_file:
            //     self.default_threshold = float(threshold_file.read())
            // except (FileNotFoundError, ValueError): a UnicodeDecodeError is a ValueError
            match std::fs::read(&threshold_path) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => py_float(&text).unwrap_or(consts.default_threshold),
                    Err(_) => consts.default_threshold,
                },
                Err(e) => {
                    let os = pyos::OsError::from_io_with_filename(&e, threshold_path.as_bytes());
                    if os.is_file_not_found() {
                        consts.default_threshold
                    } else {
                        return Err(os_error(pyos::Error::Os(os)));
                    }
                }
            }
        };

        Ok(IsPlannerFed {
            consts,
            times_queue,
            maxlen,
            threshold_path,
            default_threshold,
            is_fed: false,
            short_times: MinHeap::new_max(),
            long_times: MinHeap::new(),
        })
    }

    /// `item_count` (`:87-90`): Return how many time values are contributing to the percentile
    pub fn item_count(&self) -> usize {
        self.times_queue.len()
    }

    /// `threshold` (`:92-101`): Depending on the internal state and settings, it returns the
    /// percentile threshold or the default
    pub fn threshold(&self) -> Result<f64, PyErr> {
        if self.item_count() < self.maxlen || !self.consts.use_dynamic_threshold {
            return Ok(self.default_threshold);
        }
        self.get_dynamic_threshold()
    }

    /// `get_dynamic_threshold` (`:103-109`): Returns the Nth percentile value. N is fixed in
    /// constants
    pub fn get_dynamic_threshold(&self) -> Result<f64, PyErr> {
        if self.short_times.is_empty() && self.long_times.is_empty() {
            return Ok(f64::INFINITY);
        }
        if self.long_times.is_empty() && !self.short_times.is_empty() {
            return root_value(&self.short_times);
        }
        Ok((root_value(&self.long_times)? + root_value(&self.short_times)?) / 2.0)
    }

    /// `__call__` (`:111-115`): Did it take long enough?
    pub fn call(&self) -> bool {
        self.is_fed
    }

    /// `process_value(value)` (`:117-135`): Adds the given value to tracked values and moves the
    /// percentile value accordingly. `value`: how long did it take from send to confirmation
    pub fn process_value(&mut self, value: f64) -> Result<(), PyErr> {
        if value > self.consts.ignore_above {
            return Ok(());
        }

        if self.item_count() >= self.maxlen {
            self._remove_last()?;
        }
        self._add(value)?;

        self.is_fed = value > self.threshold()?;

        if self.is_fed {
            // log.debug("Buffer is fed, threshold: %s, value: %s",
            //           self.threshold, value)
            self.threshold()?;
        }
        Ok(())
    }

    /// `_remove_last` (`:137-150`): For the median to be influenced only by the last N commands
    /// And for the RAM and CPU usage to not slowly creep up, the size of the queue and heaps is
    /// capped.
    ///
    /// This removes the item from the queue and from its associated heap
    pub fn _remove_last(&mut self) -> Result<(), PyErr> {
        let Some(item) = self.times_queue.pop_front() else {
            return Err(PyErr::Other(
                "IndexError",
                "pop from an empty deque".to_owned(),
            ));
        };
        let (name, index) = {
            let guard = item.lock().unwrap();
            (
                guard.heap_name,
                guard.item.heap_index.expect("a queued item was pushed"),
            )
        };
        if name == Some(HeapName::LongTimes) {
            self.long_times.pop(index)?;
        } else {
            self.short_times.pop(index)?;
        }
        self.balance()
    }

    /// `_add(value)` (`:152-176`): Adds a new value to the queue and to the one of the heaps
    ///
    /// Complexity should be O(log n)
    pub fn _add(&mut self, value: f64) -> Result<(), PyErr> {
        let item: Shared<TimeValue> = Shared::new(TimeValue::new(value).into());

        if self.short_times.is_empty() {
            self._short_push(item.clone());
        } else if self.long_times.is_empty() {
            if root_value(&self.short_times)? > value {
                let larger_item = self.short_times.pop(0)?;
                self._short_push(item.clone());
                self._long_push(larger_item);
            } else {
                self._long_push(item.clone());
            }
        } else {
            if value < self.get_dynamic_threshold()? {
                self._short_push(item.clone());
            } else {
                self._long_push(item.clone());
            }
            self.balance()?;
        }

        // deque.append: with a full deque the oldest one goes
        self.times_queue.push_back(item);
        if self.times_queue.len() > self.maxlen {
            self.times_queue.pop_front();
        }
        Ok(())
    }

    /// `balance` (`:178-191`): Balances heaps to maintain the percentile
    pub fn balance(&mut self) -> Result<(), PyErr> {
        let num_long = self.long_times.len();
        let num_short = self.short_times.len();
        let total = num_long + num_short;
        let ideal_short_count = (total as f64 * self.consts.heap_ratio).round_ties_even();
        let num_short = num_short as f64;
        if num_short < ideal_short_count - 1.0 {
            let item = self.long_times.pop(0)?;
            self._short_push(item);
        } else if num_short > ideal_short_count + 1.0 {
            let item = self.short_times.pop(0)?;
            self._long_push(item);
        }

        if root_value(&self.short_times)? > root_value(&self.long_times)? {
            return Err(PyErr::Other(
                "RuntimeError",
                "Smaller value heap has a higher value than the higher value heap, that's not \
                 right..."
                    .to_owned(),
            ));
        }
        Ok(())
    }

    /// `_short_push(item)` (`:193-198`): Pushes a value into the heap containing times shorter
    /// than percentile
    pub fn _short_push(&mut self, item: Shared<TimeValue>) {
        item.lock().unwrap().heap_name = Some(HeapName::ShortTimes);
        self.short_times.push(item);
    }

    /// `_long_push(item)` (`:200-205`): Pushes a value into the heap containing times longer
    /// than percentile
    pub fn _long_push(&mut self, item: Shared<TimeValue>) {
        item.lock().unwrap().heap_name = Some(HeapName::LongTimes);
        self.long_times.push(item);
    }

    /// `save` (`:207-216`): Saves the threshold, so when the prusa-link starts up again, it
    /// doesn't rely on the default threshold anymore
    pub fn save(&self) -> Result<(), PyErr> {
        if self.item_count() >= self.maxlen {
            let os = |e: std::io::Error| {
                os_error(pyos::Error::Os(pyos::OsError::from_io_with_filename(
                    &e,
                    self.threshold_path.as_bytes(),
                )))
            };
            // open(self.threshold_path, "w") truncates first, then str(get_dynamic_threshold())
            let mut threshold_file = std::fs::File::create(&self.threshold_path).map_err(os)?;
            let text = py_float_repr(self.get_dynamic_threshold()?);
            // threshold_file.write() goes into the text file's buffer, os.fsync(fileno()) syncs
            // what the OS has (nothing yet), the close at the end of `with` writes the buffer
            threshold_file.sync_all().map_err(os)?;
            threshold_file.write_all(text.as_bytes()).map_err(os)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
