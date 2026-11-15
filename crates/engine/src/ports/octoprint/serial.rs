//! The serial transport of the Marlin port: what `comm.py` gets from `pyserial` 3.5
//! (`serial.Serial`, `serial.Timeout`, `serial.SerialException`) and `comm.py:6910
//! BufferedReadlineWrapper` (OctoPrint 1.11.8, AGPL-3.0, Copyright (C) 2013 David Braam, Gina
//! Häußge & others).
//!
//! pyserial is stood for by the `serialport` crate (owner's approval M5, `docs/agent-engine.md`):
//! the crate opens and configures the port (raw 8N1 termios, baud rate, parity, DTR, the
//! exclusive lock); the rest of `serial.serialposix.Serial` that the comm layer reads through
//! (`read` with its abort pipe, `cancel_read`, `cancel_write`, `in_waiting`, `close`, the
//! `timeout` attribute, `Timeout`) is ported from pyserial 3.5 (`serial/serialposix.py`,
//! `serial/serialutil.py`), because the crate's `read` cannot be interrupted.
//!
//! Differences from pyserial (all OURS):
//!  1. The port is opened through `serialport::new(..).open_native()`: the crate sets the raw
//!     mode (`cfmakeraw`) and the line settings, takes `flock` (shared unless `exclusive` is set)
//!     and opens with `O_CLOEXEC`; pyserial sets the flags one by one (`_reconfigure_port`) and
//!     takes the lock only when `exclusive` was asked for. DTR is set on open
//!     (`dtr_on_open(true)`) and RTS is set afterwards, as `serialposix.py:334-338` does (an
//!     error of the two calls is ignored: pyserial ignores `EINVAL`/`ENOTTY`, the crate gives
//!     no errno). The input buffer is flushed (`serialposix.py:344`).
//!  2. `write_timeout` is kept as an attribute; writing is not part of this file yet (the send
//!     loop, `comm.py:4653` ff., is a later section).
//!  3. `read` waits with `poll(2)` (`serialposix.py:806` `PosixPollSerial` does too) where
//!     `Serial.read` uses `select.select`: the same results, without `FD_SETSIZE`.
//!  4. The texts of the exceptions that come from the operating system or the crate are theirs,
//!     not Python's (`SerialError.message`).
//!  5. `Serial.__repr__` holds `id(self)`, a memory address: here the address of the object.
//!  6. The data of `BufferedReadlineWrapper` (`_buffered`) lives on the wrapper (Python's
//!     `wrapt.ObjectProxy` stores it on the wrapped object, which nobody else reads).

use super::comm::{py_float_repr, SerialError, SerialTransport};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

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

/// `serial.serialposix.Serial`, the part the comm layer uses (`serial.Serial(baudrate=..,
/// timeout=.., write_timeout=.., exclusive=..)`, then `.port = ..`, `.parity = ..`, `.open()`).
pub struct Serial {
    port: Mutex<Option<String>>,
    baudrate: Mutex<i64>,
    timeout: Mutex<Option<f64>>,
    write_timeout: Mutex<Option<f64>>,
    parity: Mutex<serialport::Parity>,
    exclusive: Option<bool>,
    handle: Mutex<Option<serialport::TTYPort>>,
    /// `-1` is `None`
    fd: AtomicI32,
    pipe_abort_read_r: AtomicI32,
    pipe_abort_read_w: AtomicI32,
    pipe_abort_write_r: AtomicI32,
    pipe_abort_write_w: AtomicI32,
    is_open: AtomicBool,
}

fn os_error_text(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(code) => format!("[Errno {code}] {e}"),
        None => e.to_string(),
    }
}

fn pipe() -> std::io::Result<(RawFd, RawFd)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `fds` holds the two ints pipe(2) writes.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((fds[0], fds[1]))
}

fn close_fd(fd: &AtomicI32) {
    let f = fd.swap(-1, Ordering::SeqCst);
    if f >= 0 {
        // SAFETY: closing a descriptor this object owns.
        unsafe { libc::close(f) };
    }
}

impl Serial {
    /// `serial.Serial(baudrate=.., timeout=.., write_timeout=.., exclusive=..)`: no port yet,
    /// so nothing is opened. A baud rate that is not a non-negative int is `ValueError`
    /// (`serialutil.py:300`), as is a negative timeout (:368, :384).
    pub fn new(
        baudrate: i64,
        timeout: Option<f64>,
        write_timeout: Option<f64>,
        exclusive: Option<bool>,
    ) -> Result<Serial, SerialError> {
        if !(0..=u32::MAX as i64).contains(&baudrate) {
            return Err(SerialError::other("ValueError", format!("Not a valid baudrate: {baudrate:?}")));
        }
        for t in [timeout, write_timeout].into_iter().flatten() {
            if t < 0.0 {
                return Err(SerialError::other("ValueError", format!("Not a valid timeout: {}", py_float_repr(t))));
            }
        }
        Ok(Serial {
            port: Mutex::new(None),
            baudrate: Mutex::new(baudrate),
            timeout: Mutex::new(timeout),
            write_timeout: Mutex::new(write_timeout),
            parity: Mutex::new(serialport::Parity::None),
            exclusive,
            handle: Mutex::new(None),
            fd: AtomicI32::new(-1),
            pipe_abort_read_r: AtomicI32::new(-1),
            pipe_abort_read_w: AtomicI32::new(-1),
            pipe_abort_write_r: AtomicI32::new(-1),
            pipe_abort_write_w: AtomicI32::new(-1),
            is_open: AtomicBool::new(false),
        })
    }

    /// `serial_obj.port = str(p)`
    pub fn set_port(&self, port: &str) {
        *self.port.lock().unwrap_or_else(|e| e.into_inner()) = Some(port.to_string());
    }

    /// `serial_obj.port`
    pub fn port(&self) -> Option<String> {
        self.port.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// `serial_obj.parity = serial.PARITY_ODD` (`odd`) / `serial.PARITY_NONE`
    pub fn set_parity_odd(&self, odd: bool) {
        *self.parity.lock().unwrap_or_else(|e| e.into_inner()) =
            if odd { serialport::Parity::Odd } else { serialport::Parity::None };
    }

    /// `serial_obj.baudrate`
    pub fn baudrate(&self) -> i64 {
        *self.baudrate.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `serial_obj.baudrate = b` (`serialutil.py:299`): a value that is no baud rate is `ValueError`;
    /// an open port is reconfigured.
    pub fn set_baudrate(&self, baudrate: i64) -> Result<(), SerialError> {
        if !(0..=u32::MAX as i64).contains(&baudrate) {
            return Err(SerialError::other("ValueError", format!("Not a valid baudrate: {baudrate:?}")));
        }
        *self.baudrate.lock().unwrap_or_else(|e| e.into_inner()) = baudrate;
        if self.is_open() {
            use serialport::SerialPort;
            if let Some(tty) = self.handle.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                tty.set_baud_rate(baudrate as u32)
                    .map_err(|e| SerialError::serial_exception(format!("Could not configure port: {e}")))?;
            }
        }
        Ok(())
    }

    /// `serial_obj.timeout`
    pub fn timeout(&self) -> Option<f64> {
        *self.timeout.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `serial_obj.timeout = t` (`serialutil.py:361`): negative is `ValueError`.
    pub fn set_timeout(&self, timeout: Option<f64>) -> Result<(), SerialError> {
        if timeout.is_some_and(|t| t < 0.0) {
            return Err(SerialError::other(
                "ValueError",
                format!("Not a valid timeout: {}", py_float_repr(timeout.expect("checked"))),
            ));
        }
        *self.timeout.lock().unwrap_or_else(|e| e.into_inner()) = timeout;
        Ok(())
    }

    /// `serial_obj.write_timeout`
    pub fn write_timeout(&self) -> Option<f64> {
        *self.write_timeout.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `serial_obj.is_open`
    pub fn is_open(&self) -> bool {
        self.is_open.load(Ordering::SeqCst)
    }

    /// `serial_obj.fd` (`None` when closed)
    pub fn fd(&self) -> Option<RawFd> {
        Some(self.fd.load(Ordering::SeqCst)).filter(|f| *f >= 0)
    }

    /// `serialposix.py:311 open()`
    pub fn open(&self) -> Result<(), SerialError> {
        use serialport::SerialPort;

        let port = match self.port() {
            None => return Err(SerialError::serial_exception("Port must be configured before it can be used.")),
            Some(p) => p,
        };
        if self.is_open() {
            return Err(SerialError::serial_exception("Port is already open."));
        }

        // difference 1: opening, raw mode, line settings, lock, DTR
        let mut builder = serialport::new(port.clone(), self.baudrate() as u32)
            .parity(*self.parity.lock().unwrap_or_else(|e| e.into_inner()))
            .timeout(Duration::ZERO)
            .dtr_on_open(true);
        builder = builder.exclusive(self.exclusive == Some(true));
        let mut tty = builder
            .open_native()
            .map_err(|e| SerialError::serial_exception(format!("could not open port {port}: {e}")))?;

        // :337 `if not self._rtscts: self._update_rts_state()` (and DTR: the crate did that)
        let _ = tty.write_request_to_send(true);
        // :344 `self._reset_input_buffer()`
        let _ = tty.clear(serialport::ClearBuffer::Input);

        let fd = tty.as_raw_fd();

        // :346-349 the two abort pipes, their read ends non-blocking
        let setup = || -> std::io::Result<()> {
            let (rr, rw) = pipe()?;
            self.pipe_abort_read_r.store(rr, Ordering::SeqCst);
            self.pipe_abort_read_w.store(rw, Ordering::SeqCst);
            let (wr, ww) = pipe()?;
            self.pipe_abort_write_r.store(wr, Ordering::SeqCst);
            self.pipe_abort_write_w.store(ww, Ordering::SeqCst);
            // SAFETY: fcntl F_SETFL with O_NONBLOCK on descriptors this object owns. `fd` is the
            // device: pyserial opens it with `O_NONBLOCK` (:322) and the crate cleared the flag
            // (difference 2).
            unsafe {
                if libc::fcntl(rr, libc::F_SETFL, libc::O_NONBLOCK) < 0
                    || libc::fcntl(wr, libc::F_SETFL, libc::O_NONBLOCK) < 0
                    || libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        };
        if let Err(e) = setup() {
            // :350-368 close everything again, raise the original
            for p in [&self.pipe_abort_read_r, &self.pipe_abort_read_w, &self.pipe_abort_write_r, &self.pipe_abort_write_w] {
                close_fd(p);
            }
            drop(tty);
            return Err(SerialError::serial_exception(os_error_text(&e)));
        }

        self.fd.store(fd, Ordering::SeqCst);
        *self.handle.lock().unwrap_or_else(|e| e.into_inner()) = Some(tty);
        self.is_open.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// `serialposix.py:529 close()`
    pub fn close(&self) {
        if self.is_open() {
            if self.fd().is_some() {
                // `os.close(self.fd)`: dropping the crate's port closes it
                let handle = self.handle.lock().unwrap_or_else(|e| e.into_inner()).take();
                match handle {
                    Some(h) => {
                        self.fd.store(-1, Ordering::SeqCst);
                        drop(h);
                    }
                    None => close_fd(&self.fd),
                }
                close_fd(&self.pipe_abort_read_w);
                close_fd(&self.pipe_abort_read_r);
                close_fd(&self.pipe_abort_write_w);
                close_fd(&self.pipe_abort_write_r);
            }
            self.is_open.store(false, Ordering::SeqCst);
        }
    }

    fn port_not_open() -> SerialError {
        SerialError::new("PortNotOpenError", "Attempting to use a port that is not open", true)
    }

    /// `serialposix.py:546 in_waiting`: the number of bytes in the input buffer (`TIOCINQ`).
    pub fn in_waiting(&self) -> Result<usize, SerialError> {
        let Some(fd) = self.fd() else {
            // `fcntl.ioctl(None, ..)`
            return Err(SerialError::other("TypeError", "argument must be an int, or have a fileno() method.".to_string()));
        };
        let mut n: libc::c_int = 0;
        // SAFETY: FIONREAD writes one int.
        if unsafe { libc::ioctl(fd, libc::FIONREAD as _, &mut n) } < 0 {
            return Err(SerialError::other("OSError", os_error_text(&std::io::Error::last_os_error())));
        }
        Ok(n as usize)
    }

    /// `serialposix.py:553 read(size=1)`: "Read size bytes from the serial port. If a timeout is
    /// set it may return less characters as requested. With no timeout it will block until the
    /// requested number of bytes is read."
    pub fn read(&self, size: usize) -> Result<Vec<u8>, SerialError> {
        if !self.is_open() {
            return Err(Self::port_not_open());
        }
        let mut read: Vec<u8> = Vec::new();
        let mut timeout = Timeout::new(self.timeout());
        while read.len() < size {
            let fd = self.fd.load(Ordering::SeqCst);
            let pipe_r = self.pipe_abort_read_r.load(Ordering::SeqCst);
            let mut fds = [
                libc::pollfd { fd, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: pipe_r, events: libc::POLLIN, revents: 0 },
            ];
            let wait_ms: libc::c_int = match timeout.time_left() {
                None => -1,
                Some(t) => (t * 1000.0).ceil().min(libc::c_int::MAX as f64) as libc::c_int,
            };
            // SAFETY: `fds` is an array of two pollfd.
            let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, wait_ms) };
            let outcome: Result<Option<Vec<u8>>, std::io::Error> = if rc < 0 {
                Err(std::io::Error::last_os_error())
            } else if fds[1].revents != 0 {
                let mut junk = [0u8; 1000];
                // SAFETY: reading at most 1000 bytes into `junk`.
                unsafe { libc::read(pipe_r, junk.as_mut_ptr().cast(), junk.len()) };
                break;
            } else if rc == 0 || fds[0].revents == 0 {
                // If select was used with a timeout, and the timeout occurs, it returns with
                // empty lists -> thus abort read operation.
                break;
            } else {
                let mut buf = vec![0u8; size - read.len()];
                // SAFETY: reading at most `buf.len()` bytes into `buf`.
                let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    buf.truncate(n as usize);
                    Ok(Some(buf))
                }
            };
            match outcome {
                Err(e) => {
                    // ignore BlockingIOErrors and EINTR. other errors are shown
                    let ignored = e.raw_os_error().is_some_and(|code| {
                        [libc::EAGAIN, libc::EALREADY, libc::EWOULDBLOCK, libc::EINPROGRESS, libc::EINTR].contains(&code)
                    });
                    if !ignored {
                        return Err(SerialError::serial_exception(format!("read failed: {}", os_error_text(&e))));
                    }
                }
                Ok(buf) => {
                    let buf = buf.expect("a read result");
                    // read should always return some data as select reported it was ready to read
                    // when we get to this point.
                    if buf.is_empty() {
                        // Disconnected devices, at least on Linux, show the behavior that they are
                        // always ready to read immediately but reading returns nothing.
                        return Err(SerialError::serial_exception(
                            "device reports readiness to read but returned no data (device disconnected or multiple access on port?)",
                        ));
                    }
                    read.extend(buf);
                }
            }

            if timeout.expired() {
                break;
            }
        }
        Ok(read)
    }

    /// `serialposix.py:612 write(data)`: "Output the given byte string over the serial port." Returns
    /// the number of bytes written (all of them, or what `os.write` took when the write timeout is
    /// 0). `select.select` is `poll(2)` (difference 3).
    pub fn write(&self, data: &[u8]) -> Result<usize, SerialError> {
        enum Step {
            Next,
            Break,
            Return(usize),
        }
        if !self.is_open() {
            return Err(Self::port_not_open());
        }
        let mut d: &[u8] = data; // `d = to_bytes(data)`
        let length = d.len();
        let mut tx_len = length;
        let mut timeout = Timeout::new(self.write_timeout());
        let timeout_error = || SerialError::new("SerialTimeoutException", "Write timeout", true);
        while tx_len > 0 {
            let fd = self.fd.load(Ordering::SeqCst);
            // SAFETY: writing at most `d.len()` bytes from `d`.
            let n = unsafe { libc::write(fd, d.as_ptr().cast(), d.len()) };
            let step = if n < 0 {
                // `except OSError`: ignore BlockingIOErrors and EINTR. other errors are shown
                let e = std::io::Error::last_os_error();
                let ignored = e.raw_os_error().is_some_and(|code| {
                    [libc::EAGAIN, libc::EALREADY, libc::EWOULDBLOCK, libc::EINPROGRESS, libc::EINTR].contains(&code)
                });
                if !ignored {
                    return Err(SerialError::serial_exception(format!("write failed: {}", os_error_text(&e))));
                }
                Step::Next
            } else {
                let n = n as usize;
                if timeout.is_non_blocking() {
                    // Zero timeout indicates non-blocking - simply return the number of bytes of
                    // data actually written
                    Step::Return(n)
                } else {
                    let infinite = timeout.is_infinite();
                    if !infinite && timeout.expired() {
                        return Err(timeout_error());
                    }
                    // when timeout is set, use select to wait for being ready with the time left
                    // as timeout (without one: wait for the write operation)
                    let (abort, ready) = self.poll_for_write(timeout.time_left())?;
                    if abort {
                        Step::Break
                    } else if !ready {
                        return Err(if infinite { SerialError::serial_exception("write failed (select)") } else { timeout_error() });
                    } else {
                        d = &d[n..];
                        tx_len -= n;
                        Step::Next
                    }
                }
            };
            match step {
                Step::Next => {}
                Step::Break => break,
                Step::Return(n) => return Ok(n),
            }
            if !timeout.is_non_blocking() && timeout.expired() {
                return Err(timeout_error());
            }
        }
        Ok(length - d.len())
    }

    /// `select.select([self.pipe_abort_write_r], [self.fd], [], time_left)`: `(abort, ready)`; on
    /// abort the pipe is drained (`os.read(.., 1000)`; `1` without a timeout, :645).
    fn poll_for_write(&self, time_left: Option<f64>) -> Result<(bool, bool), SerialError> {
        let fd = self.fd.load(Ordering::SeqCst);
        let pipe_r = self.pipe_abort_write_r.load(Ordering::SeqCst);
        let mut fds = [
            libc::pollfd { fd: pipe_r, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd, events: libc::POLLOUT, revents: 0 },
        ];
        let wait_ms: libc::c_int = match time_left {
            None => -1,
            Some(t) => (t * 1000.0).ceil().min(libc::c_int::MAX as f64) as libc::c_int,
        };
        // SAFETY: `fds` is an array of two pollfd.
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, wait_ms) };
        if rc < 0 {
            let e = std::io::Error::last_os_error();
            let ignored = e.raw_os_error().is_some_and(|code| {
                [libc::EAGAIN, libc::EALREADY, libc::EWOULDBLOCK, libc::EINPROGRESS, libc::EINTR].contains(&code)
            });
            if ignored {
                return Ok((false, false));
            }
            return Err(SerialError::serial_exception(format!("write failed: {}", os_error_text(&e))));
        }
        if fds[0].revents != 0 {
            let mut junk = [0u8; 1000];
            let len = if time_left.is_none() { 1 } else { junk.len() };
            // SAFETY: reading at most `len` bytes into `junk`.
            unsafe { libc::read(pipe_r, junk.as_mut_ptr().cast(), len) };
            return Ok((true, false));
        }
        Ok((false, fds[1].revents != 0))
    }

    /// `serialposix.py:604 cancel_read()`
    pub fn cancel_read(&self) -> Result<(), SerialError> {
        if self.is_open() {
            self.write_abort(&self.pipe_abort_read_w)?;
        }
        Ok(())
    }

    /// `serialposix.py:608 cancel_write()`
    pub fn cancel_write(&self) -> Result<(), SerialError> {
        if self.is_open() {
            self.write_abort(&self.pipe_abort_write_w)?;
        }
        Ok(())
    }

    fn write_abort(&self, pipe_w: &AtomicI32) -> Result<(), SerialError> {
        // SAFETY: writing one byte from a static buffer.
        let n = unsafe { libc::write(pipe_w.load(Ordering::SeqCst), b"x".as_ptr().cast(), 1) };
        if n < 0 {
            return Err(SerialError::other("OSError", os_error_text(&std::io::Error::last_os_error())));
        }
        Ok(())
    }

    /// `serialposix.py:130 set_low_latency_mode(low_latency_settings)`, the Linux variant (the
    /// attribute does not exist elsewhere, where `comm.py:3922` takes its `else`).
    #[cfg(target_os = "linux")]
    pub fn set_low_latency_mode(&self, low_latency_settings: bool) -> Result<(), SerialError> {
        let fd = self.fd().ok_or_else(|| SerialError::other("TypeError", "argument must be an int, or have a fileno() method.".to_string()))?;
        let mut buf = [0 as libc::c_int; 32];
        let fail = |e: std::io::Error| {
            SerialError::other(
                "ValueError",
                format!("Failed to update ASYNC_LOW_LATENCY flag to {}: {}", if low_latency_settings { "True" } else { "False" }, os_error_text(&e)),
            )
        };
        // SAFETY: TIOCGSERIAL / TIOCSSERIAL read and write a serial_struct, which fits in 32 ints.
        unsafe {
            // get serial_struct
            if libc::ioctl(fd, libc::TIOCGSERIAL as _, buf.as_mut_ptr()) < 0 {
                return Err(fail(std::io::Error::last_os_error()));
            }
            // set or unset ASYNC_LOW_LATENCY flag
            if low_latency_settings {
                buf[4] |= 0x2000;
            } else {
                buf[4] &= !0x2000;
            }
            // set serial_struct
            if libc::ioctl(fd, libc::TIOCSSERIAL as _, buf.as_mut_ptr()) < 0 {
                return Err(fail(std::io::Error::last_os_error()));
            }
        }
        Ok(())
    }

    /// `serialutil.py:527 __repr__` (difference 5: `id` is the address of this object)
    pub fn repr(&self) -> String {
        let parity = match *self.parity.lock().unwrap_or_else(|e| e.into_inner()) {
            serialport::Parity::None => 'N',
            serialport::Parity::Odd => 'O',
            serialport::Parity::Even => 'E',
        };
        format!(
            "Serial<id=0x{:x}, open={}>(port={}, baudrate={}, bytesize=8, parity='{}', stopbits=1, timeout={}, xonxoff=False, rtscts=False, dsrdtr=False)",
            self as *const Serial as usize,
            if self.is_open() { "True" } else { "False" },
            match self.port() {
                Some(p) => format!("'{p}'"),
                None => "None".to_string(),
            },
            self.baudrate(),
            parity,
            self.timeout().map_or_else(|| "None".to_string(), py_float_repr),
        )
    }
}

impl Drop for Serial {
    /// `io.RawIOBase.__del__` closes
    fn drop(&mut self) {
        self.close();
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
        BufferedReadlineWrapper { serial: obj, buffered: Mutex::new(Vec::new()) }
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

#[cfg(test)]
mod tests {
    //! OURS: OctoPrint's tests do not cover pyserial or `BufferedReadlineWrapper`. No test opens a
    //! serial port: `Serial` is given the read end of a pipe where `open()` would have set the
    //! descriptor up.
    use super::*;

    /// a `Serial` that reads from a pipe, `(serial, write end of the data pipe)`
    fn piped(timeout: Option<f64>) -> (Serial, RawFd) {
        let s = Serial::new(115200, timeout, Some(0.0), None).unwrap();
        let (r, w) = pipe().unwrap();
        s.fd.store(r, Ordering::SeqCst);
        let (rr, rw) = pipe().unwrap();
        s.pipe_abort_read_r.store(rr, Ordering::SeqCst);
        s.pipe_abort_read_w.store(rw, Ordering::SeqCst);
        let (wr, ww) = pipe().unwrap();
        s.pipe_abort_write_r.store(wr, Ordering::SeqCst);
        s.pipe_abort_write_w.store(ww, Ordering::SeqCst);
        unsafe {
            libc::fcntl(rr, libc::F_SETFL, libc::O_NONBLOCK);
            libc::fcntl(wr, libc::F_SETFL, libc::O_NONBLOCK);
        }
        s.is_open.store(true, Ordering::SeqCst);
        (s, w)
    }

    fn put(w: RawFd, data: &[u8]) {
        assert_eq!(data.len() as isize, unsafe { libc::write(w, data.as_ptr().cast(), data.len()) });
    }

    #[test]
    fn timeout_behaves_like_pyserials() {
        let mut t = Timeout::new(Some(0.0));
        assert!(t.expired());
        assert_eq!(Some(0.0), t.time_left());
        let mut t = Timeout::new(None);
        assert!(!t.expired());
        assert_eq!(None, t.time_left());
        assert!(t.is_infinite());
        let mut t = Timeout::new(Some(0.05));
        assert!(!t.expired());
        std::thread::sleep(Duration::from_millis(70));
        assert!(t.expired());
        assert_eq!(Some(0.0), t.time_left());
    }

    #[test]
    fn read_returns_what_is_there_and_times_out() {
        let (s, w) = piped(Some(0.05));
        put(w, b"ab");
        assert_eq!(2, s.in_waiting().unwrap());
        assert_eq!(b"ab".to_vec(), s.read(2).unwrap());
        // asks for more than there is: waits for the timeout, returns what it has
        put(w, b"c");
        assert_eq!(b"c".to_vec(), s.read(5).unwrap());
        assert_eq!(Vec::<u8>::new(), s.read(1).unwrap());
        assert_eq!(Vec::<u8>::new(), s.read(0).unwrap());
        unsafe { libc::close(w) };
    }

    #[test]
    fn read_of_a_closed_pipe_is_a_serial_exception() {
        let (s, w) = piped(Some(0.05));
        unsafe { libc::close(w) };
        let e = s.read(1).unwrap_err();
        assert!(e.serial_exception);
        assert!(e.message.starts_with("device reports readiness to read but returned no data"));
    }

    #[test]
    fn cancel_read_wakes_a_blocked_read() {
        let (s, w) = piped(None);
        let s = std::sync::Arc::new(s);
        let s2 = s.clone();
        let t = std::thread::spawn(move || s2.read(1));
        std::thread::sleep(Duration::from_millis(50));
        s.cancel_read().unwrap();
        assert_eq!(Vec::<u8>::new(), t.join().unwrap().unwrap());
        unsafe { libc::close(w) };
    }

    #[test]
    fn closed_serial_is_not_readable() {
        let s = Serial::new(9600, Some(1.0), None, None).unwrap();
        let e = s.read(1).unwrap_err();
        assert_eq!("PortNotOpenError", e.type_name);
        assert!(e.serial_exception);
        assert_eq!("TypeError", s.in_waiting().unwrap_err().type_name);
        s.cancel_read().unwrap();
        s.close();
        assert!(s.open().unwrap_err().message.starts_with("Port must be configured"));
    }

    #[test]
    fn new_checks_baudrate_and_timeout() {
        assert_eq!("ValueError", Serial::new(-1, None, None, None).err().unwrap().type_name);
        assert_eq!("ValueError", Serial::new(9600, Some(-1.0), None, None).err().unwrap().type_name);
        let s = Serial::new(9600, Some(1.0), None, None).unwrap();
        assert_eq!("ValueError", s.set_timeout(Some(-0.5)).unwrap_err().type_name);
        s.set_timeout(Some(2.0)).unwrap();
        assert_eq!(Some(2.0), s.timeout());
        s.set_baudrate(250000).unwrap();
        assert_eq!(250000, s.baudrate());
        assert_eq!(None, s.write_timeout());
    }

    #[test]
    fn readline_splits_at_the_terminator_and_keeps_the_rest() {
        let (s, w) = piped(Some(0.2));
        let r = BufferedReadlineWrapper::new(s);
        put(w, b"ok T:20\nsecond\nthi");
        assert_eq!(b"ok T:20\n".to_vec(), r.readline().unwrap());
        assert_eq!(b"second\n".to_vec(), r.readline().unwrap());
        // an incomplete line is not returned, it times out and stays buffered
        assert_eq!(Vec::<u8>::new(), r.readline().unwrap());
        put(w, b"rd\n");
        assert_eq!(b"third\n".to_vec(), r.readline().unwrap());
        assert_eq!(Vec::<u8>::new(), r.readline().unwrap());
        unsafe { libc::close(w) };
    }

    #[test]
    fn readline_reads_byte_by_byte_when_nothing_was_waiting() {
        let (s, w) = piped(Some(1.0));
        let r = BufferedReadlineWrapper::new(s);
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            put(w, b"a");
            std::thread::sleep(Duration::from_millis(50));
            put(w, b"b\n");
            w
        });
        assert_eq!(b"ab\n".to_vec(), r.readline().unwrap());
        unsafe { libc::close(t.join().unwrap()) };
    }

    #[test]
    fn transport_forwards_to_the_serial() {
        let s = Serial::new(115200, Some(30.0), Some(0.0), Some(true)).unwrap();
        s.set_port("/dev/ttyFAKE");
        let r = BufferedReadlineWrapper::new(s);
        assert_eq!(Some(Some("/dev/ttyFAKE".to_string())), SerialTransport::port(&r));
        assert_eq!(Some(Some(115200)), SerialTransport::baudrate(&r));
        assert_eq!(Some(30.0), SerialTransport::timeout(&r));
        r.set_timeout(2.5);
        assert_eq!(Some(2.5), SerialTransport::timeout(&r));
        r.set_baudrate(250000);
        assert_eq!(Some(Some(250000)), SerialTransport::baudrate(&r));
        assert_eq!(Some(Ok(())), SerialTransport::cancel_read(&r));
        let repr = SerialTransport::str(&r);
        assert!(repr.starts_with("Serial<id=0x"), "{repr}");
        assert!(repr.ends_with(
            ", open=False>(port='/dev/ttyFAKE', baudrate=250000, bytesize=8, parity='N', stopbits=1, timeout=2.5, xonxoff=False, rtscts=False, dsrdtr=False)"
        ), "{repr}");
        assert_eq!(Ok(()), SerialTransport::close(&r));
    }

    #[test]
    fn open_of_a_missing_port_is_a_serial_exception() {
        let s = Serial::new(115200, Some(1.0), Some(0.0), None).unwrap();
        s.set_port("/dev/maker-connect-test-does-not-exist");
        let e = s.open().unwrap_err();
        assert!(e.serial_exception);
        assert!(e.message.starts_with("could not open port /dev/maker-connect-test-does-not-exist"), "{}", e.message);
        assert!(!s.is_open());
    }

    /// a `Serial` that writes to a pipe, `(serial, read end of the data pipe)`
    fn piped_for_write(write_timeout: Option<f64>) -> (Serial, RawFd) {
        let s = Serial::new(115200, None, write_timeout, None).unwrap();
        let (r, w) = pipe().unwrap();
        s.fd.store(w, Ordering::SeqCst);
        let (rr, rw) = pipe().unwrap();
        s.pipe_abort_read_r.store(rr, Ordering::SeqCst);
        s.pipe_abort_read_w.store(rw, Ordering::SeqCst);
        let (wr, ww) = pipe().unwrap();
        s.pipe_abort_write_r.store(wr, Ordering::SeqCst);
        s.pipe_abort_write_w.store(ww, Ordering::SeqCst);
        unsafe {
            libc::fcntl(rr, libc::F_SETFL, libc::O_NONBLOCK);
            libc::fcntl(wr, libc::F_SETFL, libc::O_NONBLOCK);
            libc::fcntl(w, libc::F_SETFL, libc::O_NONBLOCK);
        }
        s.is_open.store(true, Ordering::SeqCst);
        (s, r)
    }

    fn take(r: RawFd) -> Vec<u8> {
        let mut buf = [0u8; 100];
        let n = unsafe { libc::read(r, buf.as_mut_ptr().cast(), buf.len()) };
        buf[..n.max(0) as usize].to_vec()
    }

    #[test]
    fn write_returns_what_os_write_took_without_a_write_timeout() {
        let (s, r) = piped_for_write(Some(0.0));
        assert_eq!(Ok(5), s.write(b"M105\n").map_err(|e| e.message));
        assert_eq!(b"M105\n".to_vec(), take(r));
    }

    #[test]
    fn write_writes_everything_with_a_write_timeout_and_without_one() {
        for timeout in [Some(1.0), None] {
            let (s, r) = piped_for_write(timeout);
            assert_eq!(Ok(6), s.write(b"M105 \n").map_err(|e| e.message));
            assert_eq!(b"M105 \n".to_vec(), take(r));
        }
    }

    #[test]
    fn write_on_a_closed_port_is_port_not_open() {
        let s = Serial::new(115200, None, Some(0.0), None).unwrap();
        let e = s.write(b"x").unwrap_err();
        assert_eq!("PortNotOpenError", e.type_name);
    }
}
