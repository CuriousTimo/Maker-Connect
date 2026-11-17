//! Port of CPython 3.13 `Lib/queue.py` (`reference/cpython-3.13`): `Empty`, `Full`, `ShutDown` and `Queue`, the parts that
//! OctoPrint's `PrependableQueue`, `TypedQueue`, `CommandQueue`, `JobQueue` and `SendQueue`
//! (`octoprint/util/__init__.py`, `octoprint/util/comm.py`) subclass and rely on.
//!
//! Python's Queue is subclassed by overriding `_qsize`, `_put`, `_get`; here that is the
//! [`QueueStorage`] trait, and the three `threading.Condition`s that share one mutex are one
//! `std::sync::Mutex` with three `Condvar`s. Not ported: `PriorityQueue`, `LifoQueue`,
//! `SimpleQueue` (nothing here uses them), `__class_getitem__`.

use super::py_threading::{cond_wait, deadline_after, lock, wait_cv};
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Instant;

/// Python's `ValueError` with a message.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueError(pub String);

/// `queue.Empty`: 'Exception raised by Queue.get(block=0)/get_nowait().'
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Empty;

/// `queue.Full`: 'Exception raised by Queue.put(block=0)/put_nowait().'
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Full;

/// `queue.ShutDown`: 'Raised when put/get with shut-down queue.'
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShutDown;

/// What `put` (and `prepend`) can raise: `Full`, `ShutDown`, `ValueError`, or whatever `_put` /
/// `_prepend` raise (`E`, for example `TypeAlreadyInQueue`).
#[derive(Debug, Clone, PartialEq)]
pub enum PutError<E> {
    Full,
    ShutDown,
    ValueError(String),
    Raised(E),
}

/// What `get` can raise.
#[derive(Debug, Clone, PartialEq)]
pub enum GetError {
    Empty,
    ShutDown,
    ValueError(String),
}

/// The methods Python's `Queue` documents as "Override these methods to implement other queue
/// organizations". They are only called with the mutex held.
pub trait QueueStorage {
    type Item;
    /// What `_put` may raise (`Infallible`-like `()` when nothing).
    type Error;
    fn _qsize(&self) -> usize;
    fn _put(&mut self, item: Self::Item) -> Result<(), Self::Error>;
    /// Raises `Empty` only if a subclass' `_get` does (`SendQueue._get`).
    fn _get(&mut self) -> Result<Self::Item, Empty>;
}

/// `Queue._init` / `_qsize` / `_put` / `_get` of the base class: `self.queue = deque()`.
pub struct Deque<T> {
    pub queue: VecDeque<T>,
}

impl<T> Deque<T> {
    /// `_init(maxsize)`
    pub fn new() -> Self {
        Deque {
            queue: VecDeque::new(),
        }
    }
}

impl<T> Default for Deque<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> QueueStorage for Deque<T> {
    type Item = T;
    type Error = std::convert::Infallible;
    fn _qsize(&self) -> usize {
        self.queue.len()
    }
    fn _put(&mut self, item: T) -> Result<(), Self::Error> {
        self.queue.push_back(item);
        Ok(())
    }
    fn _get(&mut self) -> Result<T, Empty> {
        Ok(self
            .queue
            .pop_front()
            .expect("deque.popleft() on an empty deque (IndexError)"))
    }
}

/// What the mutex guards: `self.queue` (the storage), `unfinished_tasks`, `is_shutdown`.
pub struct QueueState<S> {
    pub queue: S,
    pub unfinished_tasks: i64,
    pub is_shutdown: bool,
}

/// `queue.Queue`: 'Create a queue object with a given maximum size.
///
/// If maxsize is <= 0, the queue size is infinite.'
pub struct Queue<S: QueueStorage> {
    pub maxsize: i64,
    /// mutex must be held whenever the queue is mutating. It is shared between the three
    /// conditions below.
    pub mutex: Mutex<QueueState<S>>,
    /// Notify not_empty whenever an item is added to the queue; a thread waiting to get is
    /// notified then.
    pub not_empty: Condvar,
    /// Notify not_full whenever an item is removed from the queue; a thread waiting to put is
    /// notified then.
    pub not_full: Condvar,
    /// Notify all_tasks_done whenever the number of unfinished tasks drops to zero; thread waiting
    /// to join() is notified to resume.
    pub all_tasks_done: Condvar,
}

fn endtime(timeout: f64) -> Option<Instant> {
    deadline_after(timeout)
}

impl<S: QueueStorage> Queue<S> {
    pub fn new(maxsize: i64, storage: S) -> Self {
        Queue {
            maxsize,
            mutex: Mutex::new(QueueState {
                queue: storage,
                unfinished_tasks: 0,
                is_shutdown: false,
            }),
            not_empty: Condvar::new(),
            not_full: Condvar::new(),
            all_tasks_done: Condvar::new(),
        }
    }

    /// `with self.mutex:`
    pub fn lock(&self) -> MutexGuard<'_, QueueState<S>> {
        lock(&self.mutex)
    }

    /// Indicate that a formerly enqueued task is complete.
    ///
    /// Used by Queue consumer threads.  For each get() used to fetch a task,
    /// a subsequent call to task_done() tells the queue that the processing
    /// on the task is complete.
    ///
    /// If a join() is currently blocking, it will resume when all items
    /// have been processed (meaning that a task_done() call was received
    /// for every item that had been put() into the queue).
    ///
    /// Raises a ValueError if called more times than there were items
    /// placed in the queue.
    pub fn task_done(&self) -> Result<(), ValueError> {
        let mut g = self.lock();
        let unfinished = g.unfinished_tasks - 1;
        if unfinished <= 0 {
            if unfinished < 0 {
                return Err(ValueError("task_done() called too many times".to_string()));
            }
            self.all_tasks_done.notify_all();
        }
        g.unfinished_tasks = unfinished;
        Ok(())
    }

    /// Blocks until all items in the Queue have been gotten and processed.
    ///
    /// The count of unfinished tasks goes up whenever an item is added to the
    /// queue. The count goes down whenever a consumer thread calls task_done()
    /// to indicate the item was retrieved and all work on it is complete.
    ///
    /// When the count of unfinished tasks drops to zero, join() unblocks.
    pub fn join(&self) {
        let mut g = self.lock();
        while g.unfinished_tasks != 0 {
            g = wait_cv(&self.all_tasks_done, g);
        }
    }

    /// Return the approximate size of the queue (not reliable!).
    pub fn qsize(&self) -> usize {
        self.lock().queue._qsize()
    }

    /// Return True if the queue is empty, False otherwise (not reliable!).
    ///
    /// This method is likely to be removed at some point.  Use qsize() == 0
    /// as a direct substitute, but be aware that either approach risks a race
    /// condition where a queue can grow before the result of empty() or
    /// qsize() can be used.
    ///
    /// To create code that needs to wait for all queued tasks to be
    /// completed, the preferred technique is to use the join() method.
    pub fn empty(&self) -> bool {
        self.lock().queue._qsize() == 0
    }

    /// Return True if the queue is full, False otherwise (not reliable!).
    ///
    /// This method is likely to be removed at some point.  Use qsize() >= n
    /// as a direct substitute, but be aware that either approach risks a race
    /// condition where a queue can shrink before the result of full() or
    /// qsize() can be used.
    pub fn full(&self) -> bool {
        0 < self.maxsize && self.maxsize <= self.lock().queue._qsize() as i64
    }

    /// Put an item into the queue.
    ///
    /// If optional args 'block' is true and 'timeout' is None (the default),
    /// block if necessary until a free slot is available. If 'timeout' is
    /// a non-negative number, it blocks at most 'timeout' seconds and raises
    /// the Full exception if no free slot was available within that time.
    /// Otherwise ('block' is false), put an item on the queue if a free slot
    /// is immediately available, else raise the Full exception ('timeout'
    /// is ignored in that case).
    ///
    /// Raises ShutDown if the queue has been shut down.
    pub fn put(
        &self,
        item: S::Item,
        block: bool,
        timeout: Option<f64>,
    ) -> Result<(), PutError<S::Error>> {
        let mut g = self.lock();
        if g.is_shutdown {
            return Err(PutError::ShutDown);
        }
        if self.maxsize > 0 {
            if !block {
                if g.queue._qsize() as i64 >= self.maxsize {
                    return Err(PutError::Full);
                }
            } else if timeout.is_none() {
                while g.queue._qsize() as i64 >= self.maxsize {
                    g = wait_cv(&self.not_full, g);
                    if g.is_shutdown {
                        return Err(PutError::ShutDown);
                    }
                }
            } else if timeout.unwrap() < 0.0 {
                return Err(PutError::ValueError(
                    "'timeout' must be a non-negative number".to_string(),
                ));
            } else {
                let end = endtime(timeout.unwrap());
                while g.queue._qsize() as i64 >= self.maxsize {
                    if matches!(end, Some(e) if Instant::now() >= e) {
                        return Err(PutError::Full);
                    }
                    g = cond_wait(&self.not_full, g, end).0;
                    if g.is_shutdown {
                        return Err(PutError::ShutDown);
                    }
                }
            }
        }
        g.queue._put(item).map_err(PutError::Raised)?;
        g.unfinished_tasks += 1;
        self.not_empty.notify_one();
        Ok(())
    }

    /// Remove and return an item from the queue.
    ///
    /// If optional args 'block' is true and 'timeout' is None (the default),
    /// block if necessary until an item is available. If 'timeout' is
    /// a non-negative number, it blocks at most 'timeout' seconds and raises
    /// the Empty exception if no item was available within that time.
    /// Otherwise ('block' is false), return an item if one is immediately
    /// available, else raise the Empty exception ('timeout' is ignored
    /// in that case).
    ///
    /// Raises ShutDown if the queue has been shut down and is empty,
    /// or if the queue has been shut down immediately.
    pub fn get(&self, block: bool, timeout: Option<f64>) -> Result<S::Item, GetError> {
        let mut g = self.lock();
        if g.is_shutdown && g.queue._qsize() == 0 {
            return Err(GetError::ShutDown);
        }
        if !block {
            if g.queue._qsize() == 0 {
                return Err(GetError::Empty);
            }
        } else if timeout.is_none() {
            while g.queue._qsize() == 0 {
                g = wait_cv(&self.not_empty, g);
                if g.is_shutdown && g.queue._qsize() == 0 {
                    return Err(GetError::ShutDown);
                }
            }
        } else if timeout.unwrap() < 0.0 {
            return Err(GetError::ValueError(
                "'timeout' must be a non-negative number".to_string(),
            ));
        } else {
            let end = endtime(timeout.unwrap());
            while g.queue._qsize() == 0 {
                if matches!(end, Some(e) if Instant::now() >= e) {
                    return Err(GetError::Empty);
                }
                g = cond_wait(&self.not_empty, g, end).0;
                if g.is_shutdown && g.queue._qsize() == 0 {
                    return Err(GetError::ShutDown);
                }
            }
        }
        let item = g.queue._get().map_err(|Empty| GetError::Empty)?;
        self.not_full.notify_one();
        Ok(item)
    }

    /// Put an item into the queue without blocking.
    ///
    /// Only enqueue the item if a free slot is immediately available.
    /// Otherwise raise the Full exception.
    pub fn put_nowait(&self, item: S::Item) -> Result<(), PutError<S::Error>> {
        self.put(item, false, None)
    }

    /// Remove and return an item from the queue without blocking.
    ///
    /// Only get an item if one is immediately available. Otherwise
    /// raise the Empty exception.
    pub fn get_nowait(&self) -> Result<S::Item, GetError> {
        self.get(false, None)
    }

    /// Shut-down the queue, making queue gets and puts raise ShutDown.
    ///
    /// By default, gets will only raise once the queue is empty. Set
    /// 'immediate' to True to make gets raise immediately instead.
    ///
    /// All blocked callers of put() and get() will be unblocked.
    ///
    /// If 'immediate', the queue is drained and unfinished tasks
    /// is reduced by the number of drained tasks.  If unfinished tasks
    /// is reduced to zero, callers of Queue.join are unblocked.
    pub fn shutdown(&self, immediate: bool) -> Result<(), Empty> {
        let mut g = self.lock();
        g.is_shutdown = true;
        if immediate {
            while g.queue._qsize() != 0 {
                g.queue._get()?;
                if g.unfinished_tasks > 0 {
                    g.unfinished_tasks -= 1;
                }
            }
            // release all blocked threads in `join()`
            self.all_tasks_done.notify_all();
        }
        // All getters need to re-check queue-empty to raise ShutDown
        self.not_empty.notify_all();
        self.not_full.notify_all();
        Ok(())
    }
}
