//! This module checks for system clock adjustments (`clock.py`).

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// `time.time()`
pub fn time() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// `time.monotonic()`: seconds on a clock that system clock changes do not move.
pub fn monotonic() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Check if the clock has been adjusted by comparing the system time to HW clock.
///
/// It assumes that the system clock has been adjusted if `start_time - hw_clock_time` is
/// different from current values outside the [`ClockWatcher::TOLERANCE`].
#[derive(Debug, Clone)]
pub struct ClockWatcher {
    pub delta: f64,
}

impl ClockWatcher {
    /// seconds
    pub const TOLERANCE: f64 = 1.0;

    pub fn new() -> ClockWatcher {
        let mut watcher = ClockWatcher { delta: 0.0 };
        watcher.reset();
        watcher
    }

    /// Reset the measured delta
    pub fn reset(&mut self) {
        self.delta = time() - monotonic();
    }

    /// Check if the clock has been adjusted on the system
    pub fn clock_adjusted(&self) -> bool {
        (self.delta - Self::current_delta()).abs() >= Self::TOLERANCE
    }

    /// Return the difference between the current time from EPOCH and HW clock. Both values are
    /// in seconds.
    pub fn current_delta() -> f64 {
        time() - monotonic()
    }
}

impl Default for ClockWatcher {
    fn default() -> ClockWatcher {
        ClockWatcher::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_adjusted_right_after_reset() {
        let watcher = ClockWatcher::new();
        assert!(!watcher.clock_adjusted());
    }
}

/// `tests/test_clock.py` at `c0d7d233`: `test_clock_adjusted`. `test_loop_telemetry` and
/// `test_loop_event` run the Printer's loop and come with it.
#[cfg(test)]
mod sdk_tests {
    use super::*;

    /// Helper to mock adjusting the clock in `clock_watcher`
    fn adjust_clock(clock_watcher: &mut ClockWatcher) {
        clock_watcher.delta += ClockWatcher::TOLERANCE + 1.0;
    }

    #[test]
    fn test_clock_adjusted() {
        let mut adj_watcher = ClockWatcher::new();
        assert!(!adj_watcher.clock_adjusted());

        adjust_clock(&mut adj_watcher);
        assert!(adj_watcher.clock_adjusted());

        adj_watcher.reset();
        assert!(!adj_watcher.clock_adjusted());
    }
}
