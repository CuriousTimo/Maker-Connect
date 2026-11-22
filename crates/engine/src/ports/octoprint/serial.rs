//! The serial transport of the Marlin port: what `comm.py` gets from `pyserial` 3.5
//! (`serial.Serial`, `serial.Timeout`, `serial.SerialException`) and `comm.py:6910
//! BufferedReadlineWrapper` (OctoPrint 1.11.8, AGPL-3.0, Copyright (C) 2013 David Braam, Gina
//! Häußge & others).
//!
//! `Serial` is pyserial's platform backend: `serialposix.rs` (Unix, `serial/serialposix.py`) or
//! `serialwin32.rs` (Windows, `serial/serialwin32.py`). `Timeout` (`serial/serialutil.py`) and
//! `BufferedReadlineWrapper` are the same on both.
//!
//! Differences from the reference (OURS):
//!  1. The data of `BufferedReadlineWrapper` (`_buffered`) lives on the wrapper (Python's
//!     `wrapt.ObjectProxy` stores it on the wrapped object, which nobody else reads).

use super::comm::{SerialError, SerialTransport};
#[cfg(unix)]
pub use super::serialposix::Serial;
#[cfg(windows)]
pub use super::serialwin32::Serial;
use std::sync::Mutex;

/// `serial.LF` (`serialutil.py:25`)
pub const LF: &[u8] = b"\n";

/// `time.monotonic()` as `serialutil.Timeout.TIME` uses it.
fn now() -> f64 {
    super::comm::time_monotonic()
}

/// `serialutil.py:106 Timeout`: "Abstraction for timeout operations", also for 0 (non-blocking)
/// and `None` (fully blocking).
pub struct Timeout {
    is_infinite: bool,
    is_non_blocking: bool,
    duration: Option<f64>,
    target_time: Option<f64>,
}

impl Timeout {
    /// :127 `__init__(duration)`
    pub fn new(duration: Option<f64>) -> Self {
        Timeout {
            is_infinite: duration.is_none(),
            is_non_blocking: duration == Some(0.0),
            duration,
            target_time: duration.map(|d| now() + d),
        }
    }

    /// :137 `expired()`
    pub fn expired(&mut self) -> bool {
        self.target_time.is_some() && self.time_left().is_some_and(|t| t <= 0.0)
    }

    /// :141 `time_left()`: seconds left (`None` for an infinite timeout).
    pub fn time_left(&mut self) -> Option<f64> {
        if self.is_non_blocking {
            Some(0.0)
        } else if self.is_infinite {
            None
        } else {
            let duration = self.duration.expect("a finite timeout has a duration");
            let delta = self.target_time.expect("a finite timeout has a target") - now();
            if delta > duration {
                // clock jumped, recalculate
                self.target_time = Some(now() + duration);
                Some(duration)
            } else {
                Some(delta.max(0.0))
            }
        }
    }

    /// `is_infinite` (:129)
    pub fn is_infinite(&self) -> bool {
        self.is_infinite
    }

    /// `is_non_blocking` (:128)
    pub fn is_non_blocking(&self) -> bool {
        self.is_non_blocking
    }
}

/// `comm.py:6910 BufferedReadlineWrapper(wrapt.ObjectProxy)`: everything but `readline` is the
/// wrapped `Serial` (the `SerialTransport` methods forward to it).
pub struct BufferedReadlineWrapper {
    serial: Serial,
    buffered: Mutex<Vec<u8>>,
}

impl BufferedReadlineWrapper {
    /// :6911 `__init__(obj)`
    pub fn new(obj: Serial) -> Self {
        BufferedReadlineWrapper {
            serial: obj,
            buffered: Mutex::new(Vec::new()),
        }
    }

    /// the wrapped object
    pub fn serial(&self) -> &Serial {
        &self.serial
    }

    fn buffered(&self) -> std::sync::MutexGuard<'_, Vec<u8>> {
        self.buffered.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// :6916 `readline(terminator=serial.LF)`
    pub fn readline_with(&self, terminator: &[u8]) -> Result<Vec<u8>, SerialError> {
        let termlen = terminator.len();
        let mut timeout = Timeout::new(self.serial.timeout());

        while !timeout.expired() {
            let n = self.serial.in_waiting()?;
            let more = self.serial.read(n)?;
            self.buffered().extend(more);

            // check for terminator, if it's there we have found our line
            let termpos = {
                let b = self.buffered();
                b.windows(termlen).position(|w| w == terminator)
            };
            if let Some(termpos) = termpos {
                // line: everything up to and incl. the terminator, buffered: rest
                let mut b = self.buffered();
                let line: Vec<u8> = b.drain(..termpos + termlen).collect();
                return Ok(line);
            }

            if timeout.expired() {
                break;
            }

            let c = self.serial.read(1)?;
            if c.is_empty() {
                // EOF
                break;
            }

            self.buffered().extend(c);
        }

        Ok(Vec::new())
    }
}

impl SerialTransport for BufferedReadlineWrapper {
    fn port(&self) -> Option<Option<String>> {
        Some(self.serial.port())
    }
    fn baudrate(&self) -> Option<Option<i64>> {
        Some(Some(self.serial.baudrate()))
    }
    fn cancel_read(&self) -> Option<Result<(), String>> {
        Some(self.serial.cancel_read().map_err(|e| e.message))
    }
    fn cancel_write(&self) -> Option<Result<(), String>> {
        Some(self.serial.cancel_write().map_err(|e| e.message))
    }
    fn close(&self) -> Result<(), String> {
        self.serial.close();
        Ok(())
    }
    fn str(&self) -> String {
        self.serial.repr()
    }
    fn set_timeout(&self, timeout: f64) {
        self.serial
            .set_timeout(Some(timeout))
            .unwrap_or_else(|e| panic!("{}: {}", e.type_name, e.message));
    }
    fn timeout(&self) -> Option<f64> {
        self.serial.timeout()
    }
    fn set_baudrate(&self, baudrate: i64) {
        self.serial
            .set_baudrate(baudrate)
            .unwrap_or_else(|e| panic!("{}: {}", e.type_name, e.message));
    }
    fn readline(&self) -> Result<Vec<u8>, SerialError> {
        self.readline_with(LF)
    }
    fn write(&self, data: &[u8]) -> Result<Option<i64>, SerialError> {
        self.serial.write(data).map(|n| Some(n as i64))
    }
    #[cfg(target_os = "linux")]
    fn set_low_latency_mode(&self, on: bool) -> Option<Result<(), SerialError>> {
        Some(self.serial.set_low_latency_mode(on))
    }
}
