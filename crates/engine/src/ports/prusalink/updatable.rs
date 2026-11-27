//! PrusaLink's `printer_adapter/updatable.py` (`reference/Prusa-Link`, `171148ce`): Contains
//! implementation of the ThreadedUpdatable class. There was an updatable without a thread, but it
//! stopped being used (#242, section A).
//!
//! A subclass of `ThreadedUpdatable` is a type that holds one and is an [`Update`]: its
//! `update_interval` and `update`. `threading` is the CPython port ([`Thread`], [`Event`]).
//!
//! **Not covered:** `Thread`'s profiling (`profile_run`, `enable_profiling`, `disable_profiling`,
//! `:15-36`: cProfile, a developer's tool) and `thread_name` as the thread's name (it names the
//! thread for that and for `prctl_name`, nothing the printer sees).

use std::sync::Arc;

use super::util::loop_until;
use crate::ports::python::py_threading::{Event, RuntimeError, Thread};

/// A subclass's `update_interval` and `update`.
pub trait Update: Send + Sync + 'static {
    /// `update_interval = 1.0` (`:42`)
    fn update_interval(&self) -> f64 {
        1.0
    }

    /// `update()` (`:67-69`): Put code for updating here.
    fn update(&self);
}

/// `ThreadedUpdatable` (`:39-69`): Thread for parallel update operation.
pub struct ThreadedUpdatable {
    /// `thread_name = "updater_thread"` (`:41`), or the subclass's
    pub thread_name: &'static str,
    pub quit_evt: Arc<Event>,
    pub thread: Thread,
}

impl ThreadedUpdatable {
    /// `ThreadedUpdatable.thread_name` (`:41`)
    pub const THREAD_NAME: &'static str = "updater_thread";

    /// `__init__()` (`:44-53`)
    pub fn new(thread_name: &'static str) -> ThreadedUpdatable {
        ThreadedUpdatable {
            thread_name,
            quit_evt: Arc::new(Event::new()),
            thread: Thread::new(false),
        }
    }

    /// `start()` (`:55-57`): Start thread. The target made in `__init__`: `partial(loop_until,
    /// loop_evt=self.quit_evt, run_every_sec=lambda: self.update_interval, to_run=self.update)`.
    pub fn start(&self, updatable: Arc<dyn Update>) -> Result<(), RuntimeError> {
        let quit_evt = self.quit_evt.clone();
        self.thread.start(move || {
            loop_until(
                &quit_evt,
                || updatable.update_interval(),
                || updatable.update(),
            )
        })
    }

    /// `stop()` (`:59-61`): Stop the updatable
    pub fn stop(&self) {
        self.quit_evt.set();
    }

    /// `wait_stopped()` (`:63-65`): Wait for the updatable to be stopped
    pub fn wait_stopped(&self) -> Result<(), RuntimeError> {
        self.thread.join(None)
    }
}

#[cfg(test)]
mod tests;
