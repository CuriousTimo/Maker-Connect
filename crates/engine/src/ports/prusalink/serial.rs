//! PrusaLink's `serial/serial.py` (`reference/Prusa-Link`, `171148ce`): Own Serail class (#243,
//! section B1). PrusaLink's own termios port, not pySerial: the same `open` flags, terminal
//! attributes, `ioctl`s, `select`, `os.read` of 1024 and `readline` loop.
//!
//! Linux only, as the reference: `termios.IUCLC` (`:59`) does not exist on macOS, where
//! PrusaLink's `Serial()` raises `AttributeError`, and there is no `termios` on Windows (the MK3
//! port is Linux first, owner 2026-11-27).
//!
//! The calls go through [`Os`]: [`Posix`] makes them (CPython 3.13's `os`, `termios`, `fcntl` and
//! `select` on libc), and PrusaLink's tests replace some (`patch.object(module.termios, ...)`), as
//! ours do with their own [`Os`]. A terminal's attributes are what `termios.tcgetattr` returns,
//! [`Tty`]. The exceptions are CPython 3.13's: an `OSError` by its `errno` ([`OsError`]),
//! `termios.error` as `error` with `(errno, 'strerror')`, the `TypeError` of a call on the
//! descriptor `None` (a closed port).
//!
//! **Not covered:** logging (`logging.getLogger(__name__).warning`, `:88-89`): kept as a comment.

use std::ffi::{CString, c_int, c_ulong};

use maker_connect_gcode_metadata::py::{PyErr, repr_str};
use maker_connect_sdk::pyos::{Filename, OsError};

use super::util::time;

/// `SerialException(RuntimeError)` (`:16-17`): Own exception type.
fn serial_exception(message: String) -> PyErr {
    PyErr::Other("SerialException", message)
}

/// `TIOCM_DTR_str = struct.pack('I', termios.TIOCM_DTR)` (`:12`)
pub const TIOCM_DTR_STR: u32 = libc::TIOCM_DTR as u32;
/// `TIOCM_RTS_str = struct.pack('I', termios.TIOCM_RTS)` (`:13`)
pub const TIOCM_RTS_STR: u32 = libc::TIOCM_RTS as u32;

/// What `termios.tcgetattr` returns: `[iflag, oflag, cflag, lflag, ispeed, ospeed, cc]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Tty {
    pub iflag: libc::tcflag_t,
    pub oflag: libc::tcflag_t,
    pub cflag: libc::tcflag_t,
    pub lflag: libc::tcflag_t,
    pub ispeed: libc::speed_t,
    pub ospeed: libc::speed_t,
    pub cc: Vec<libc::cc_t>,
}

/// The calls `serial.py` makes; `fd` `None` is a closed port (`self.fd = None`).
pub trait Os: Send + Sync {
    /// `os.open(port, flags)`
    fn open(&self, path: &str, flags: c_int) -> Result<c_int, PyErr>;
    /// `termios.tcgetattr(fd)`
    fn tcgetattr(&self, fd: c_int) -> Result<Tty, PyErr>;
    /// `termios.tcsetattr(fd, when, attributes)`
    fn tcsetattr(&self, fd: c_int, when: c_int, tty: &Tty) -> Result<(), PyErr>;
    /// `termios.tcflush(fd, queue)`
    fn tcflush(&self, fd: c_int, queue: c_int) -> Result<(), PyErr>;
    /// `fcntl.ioctl(fd, request, struct.pack('I', arg))`
    fn ioctl(&self, fd: Option<c_int>, request: c_ulong, arg: u32) -> Result<(), PyErr>;
    /// `select([fd], [], [], timeout)`: whether `fd` is readable
    fn select(&self, fd: Option<c_int>, timeout: f64) -> Result<bool, PyErr>;
    /// `os.read(fd, n)`
    fn read(&self, fd: Option<c_int>, n: usize) -> Result<Vec<u8>, PyErr>;
    /// `os.write(fd, data)`
    fn write(&self, fd: Option<c_int>, data: &[u8]) -> Result<usize, PyErr>;
    /// `os.close(fd)`
    fn close(&self, fd: c_int) -> Result<(), PyErr>;
}

/// The `OSError` of the last failed call, by `errno`.
fn last_os_error(filename: Option<Filename>) -> PyErr {
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    os_error(errno, filename)
}

fn os_error(errno: c_int, filename: Option<Filename>) -> PyErr {
    let error = OsError::from_errno(errno, filename);
    PyErr::Other(error.class_name(), error.str())
}

/// `termios.error`: `(errno, 'strerror')`
fn termios_error() -> PyErr {
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    let error = OsError::from_errno(errno, None);
    PyErr::Other(
        "error",
        format!("({}, {})", errno, repr_str(&error.strerror)),
    )
}

/// `select`'s and `fcntl.ioctl`'s `TypeError` for `None` (CPython 3.13).
fn not_a_fileno() -> PyErr {
    PyErr::TypeError("argument must be an int, or have a fileno() method.".to_owned())
}

/// `os.read`'s and `os.write`'s `TypeError` for `None` (CPython 3.13).
fn not_an_integer() -> PyErr {
    PyErr::TypeError("'NoneType' object cannot be interpreted as an integer".to_owned())
}

/// The real calls, on libc.
pub struct Posix;

impl Os for Posix {
    fn open(&self, path: &str, flags: c_int) -> Result<c_int, PyErr> {
        let filename = Some(Filename::Path(path.as_bytes().to_vec()));
        let Ok(c_path) = CString::new(path) else {
            return Err(PyErr::ValueError("embedded null byte".to_owned()));
        };
        // `os.open(path, flags, mode=0o777)`
        let fd = unsafe { libc::open(c_path.as_ptr(), flags, 0o777) };
        if fd < 0 {
            return Err(last_os_error(filename));
        }
        Ok(fd)
    }

    fn tcgetattr(&self, fd: c_int) -> Result<Tty, PyErr> {
        let mut mode: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut mode) } == -1 {
            return Err(termios_error());
        }
        Ok(Tty {
            iflag: mode.c_iflag,
            oflag: mode.c_oflag,
            cflag: mode.c_cflag,
            lflag: mode.c_lflag,
            ispeed: unsafe { libc::cfgetispeed(&mode) },
            ospeed: unsafe { libc::cfgetospeed(&mode) },
            cc: mode.c_cc.to_vec(),
        })
    }

    fn tcsetattr(&self, fd: c_int, when: c_int, tty: &Tty) -> Result<(), PyErr> {
        // CPython's `termios_tcsetattr_impl`: the current attributes, then each from the list
        let mut mode: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut mode) } == -1 {
            return Err(termios_error());
        }
        mode.c_iflag = tty.iflag;
        mode.c_oflag = tty.oflag;
        mode.c_cflag = tty.cflag;
        mode.c_lflag = tty.lflag;
        if tty.cc.len() != mode.c_cc.len() {
            return Err(PyErr::TypeError(format!(
                "tcsetattr: attributes[6] must be {} element list",
                mode.c_cc.len()
            )));
        }
        mode.c_cc.copy_from_slice(&tty.cc);
        if unsafe { libc::cfsetispeed(&mut mode, tty.ispeed) } == -1
            || unsafe { libc::cfsetospeed(&mut mode, tty.ospeed) } == -1
            || unsafe { libc::tcsetattr(fd, when, &mode) } == -1
        {
            return Err(termios_error());
        }
        Ok(())
    }

    fn tcflush(&self, fd: c_int, queue: c_int) -> Result<(), PyErr> {
        if unsafe { libc::tcflush(fd, queue) } == -1 {
            return Err(termios_error());
        }
        Ok(())
    }

    fn ioctl(&self, fd: Option<c_int>, request: c_ulong, arg: u32) -> Result<(), PyErr> {
        let fd = fd.ok_or_else(not_a_fileno)?;
        // a bytes argument: a copy in a buffer, its address passed
        let mut buffer = arg;
        if unsafe { libc::ioctl(fd, request as _, &mut buffer as *mut u32) } == -1 {
            return Err(last_os_error(None));
        }
        Ok(())
    }

    fn select(&self, fd: Option<c_int>, timeout: f64) -> Result<bool, PyErr> {
        let fd = fd.ok_or_else(not_a_fileno)?;
        if timeout < 0.0 {
            return Err(PyErr::ValueError("timeout must be non-negative".to_owned()));
        }
        let mut readable: libc::fd_set = unsafe { std::mem::zeroed() };
        unsafe { libc::FD_SET(fd, &mut readable) };
        let mut tv = libc::timeval {
            tv_sec: timeout.trunc() as libc::time_t,
            tv_usec: ((timeout - timeout.trunc()) * 1e6).ceil() as libc::suseconds_t,
        };
        let n = unsafe {
            libc::select(
                fd + 1,
                &mut readable,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut tv,
            )
        };
        if n < 0 {
            return Err(last_os_error(None));
        }
        Ok(n > 0 && unsafe { libc::FD_ISSET(fd, &readable) })
    }

    fn read(&self, fd: Option<c_int>, n: usize) -> Result<Vec<u8>, PyErr> {
        let fd = fd.ok_or_else(not_an_integer)?;
        let mut buffer = vec![0u8; n];
        let got = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), n) };
        if got < 0 {
            return Err(last_os_error(None));
        }
        buffer.truncate(got as usize);
        Ok(buffer)
    }

    fn write(&self, fd: Option<c_int>, data: &[u8]) -> Result<usize, PyErr> {
        let fd = fd.ok_or_else(not_an_integer)?;
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n < 0 {
            return Err(last_os_error(None));
        }
        Ok(n as usize)
    }

    fn close(&self, fd: c_int) -> Result<(), PyErr> {
        if unsafe { libc::close(fd) } == -1 {
            return Err(last_os_error(None));
        }
        Ok(())
    }
}

/// `Serial` (`:20-161`): PySerial compatible class.
pub struct Serial {
    os: Box<dyn Os>,
    pub timeout: f64,
    pub fd: Option<c_int>,
    buffer: Vec<u8>,
    dtr: bool,
}

impl Serial {
    /// `baudrates = MappingProxyType({115200: termios.B115200})` (`:22`)
    pub const BAUDRATES: [(i64, libc::speed_t); 1] = [(115200, libc::B115200)];

    /// `__init__(port, baudrate, timeout)` (`:24-94`): baudrate - must be valid baudrates from
    /// Serial.baudrates; timeout - read operation timeout
    pub fn new(port: &str, baudrate: i64, timeout: f64) -> Result<Serial, PyErr> {
        Serial::with_os(Box::new(Posix), port, baudrate, timeout)
    }

    /// [`Serial::new`] with the calls of `os`.
    pub fn with_os(
        os: Box<dyn Os>,
        port: &str,
        baudrate: i64,
        timeout: f64,
    ) -> Result<Serial, PyErr> {
        if !Serial::BAUDRATES.iter().any(|(rate, _)| *rate == baudrate) {
            return Err(serial_exception(format!(
                "Baudrate `{baudrate}` is not supported"
            )));
        }

        // pylint: disable=invalid-name
        let fd = os.open(port, libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK)?;
        let mut tty = os.tcgetattr(fd)?;

        // cflags
        tty.cflag &= !libc::PARENB;
        tty.cflag &= !libc::CSTOPB;
        tty.cflag |= libc::CS8;
        tty.cflag &= !libc::CRTSCTS;
        tty.cflag |= libc::CREAD | libc::CLOCAL;
        tty.cflag &= !libc::HUPCL; // disable hangup

        // lflags
        tty.lflag &= !libc::ICANON;
        tty.lflag &= !libc::ECHO;
        tty.lflag &= !libc::ECHOE;
        tty.lflag &= !libc::ECHONL;
        tty.lflag &= !libc::ECHOK;
        tty.lflag &= !libc::ISIG;
        tty.lflag &= !libc::IEXTEN;

        // iflags
        tty.iflag &= !(libc::IXON | libc::IXOFF | libc::IXANY);
        tty.iflag &= !(libc::IGNBRK | libc::ISTRIP | libc::INLCR | libc::IGNCR | libc::ICRNL);
        tty.iflag &= !(libc::IUCLC | libc::PARMRK);

        // oflags
        tty.oflag &= !libc::OPOST;
        tty.oflag &= !libc::ONLCR;
        tty.oflag &= !libc::OCRNL;

        // cc
        tty.cc[libc::VTIME] = 0; // can be timout*10
        tty.cc[libc::VMIN] = 0;

        // ispeed
        tty.ispeed = libc::B115200;
        // ospeed
        tty.ospeed = libc::B115200;

        os.tcsetattr(fd, libc::TCSANOW, &tty)?;
        // TCSAFLUSH set after everything is done
        os.tcsetattr(fd, libc::TCSAFLUSH, &tty)?;
        // clear input buffer
        os.tcflush(fd, libc::TCIFLUSH)?;

        let set_lines = || -> Result<(), PyErr> {
            // Data Terminal Ready
            os.ioctl(Some(fd), libc::TIOCMBIS as c_ulong, TIOCM_DTR_STR)?;
            // Request To Send
            os.ioctl(Some(fd), libc::TIOCMBIS as c_ulong, TIOCM_RTS_STR)
        };
        match set_lines() {
            Ok(()) => {}
            Err(PyErr::Other(_, text))
                if text.starts_with(&format!("[Errno {}]", libc::ENOTTY)) =>
            {
                // logging.getLogger(__name__).warning(
                //     "The file does not support ioctl() ignoring")
            }
            Err(error) => return Err(error),
        }

        Ok(Serial {
            os,
            timeout,
            fd: Some(fd),
            buffer: vec![],
            dtr: false,
        })
    }

    /// `close()` (`:96-105`): Close the port.
    pub fn close(&mut self) {
        let Some(fd) = self.fd else {
            return;
        };
        let _ = self.os.close(fd);
        self.fd = None;
    }

    /// `__read(timeout)` (`:107-118`): Fill internal buffer by read from file descriptor.
    fn read(&mut self, timeout: f64) -> Result<(), PyErr> {
        let attempt = (|| {
            let ready = self.os.select(self.fd, timeout)?;
            // `if ready[0] and self.fd`: a descriptor 0 is false
            if ready && self.fd.is_some_and(|fd| fd != 0) {
                let read_bytes = self.os.read(self.fd, 1024)?;
                if read_bytes.is_empty() {
                    return Err(serial_exception(
                        "The serial became disconnected.".to_owned(),
                    ));
                }
                self.buffer.extend_from_slice(&read_bytes);
            }
            Ok(())
        })();
        match attempt {
            Err(error)
                if matches!(
                    error.class_name(),
                    "BlockingIOError" | "InterruptedError" | "TypeError"
                ) =>
            {
                self.close();
                Err(serial_exception(format!(
                    "read failed: {}",
                    error.message()
                )))
            }
            other => other,
        }
    }

    /// `readline()` (`:120-139`): Return next line from local buffer or from serial port.
    pub fn readline(&mut self) -> Result<Vec<u8>, PyErr> {
        let times_out_at = time() + self.timeout;
        let mut start_at = 0;

        loop {
            let current_time = time();
            if let Some(pos) = self.buffer[start_at.min(self.buffer.len())..]
                .iter()
                .position(|b| *b == b'\n')
                .map(|p| p + start_at)
            {
                let line = self.buffer[..pos + 1].to_vec();
                self.buffer.drain(..pos + 1);
                return Ok(line);
            }

            start_at = self.buffer.len().saturating_sub(1);
            if current_time >= times_out_at {
                break;
            }

            self.read(times_out_at - current_time)?;
        }

        Ok(vec![])
    }

    /// `write(data)` (`:141-143`): Write data to serial port.
    pub fn write(&self, data: &[u8]) -> Result<usize, PyErr> {
        self.os.write(self.fd, data)
    }

    /// `is_open` (`:145-148`): Return true if port is open.
    pub fn is_open(&self) -> bool {
        self.fd.is_some()
    }

    /// `dtr` (`:150-153`): Data Terminal Ready State
    pub fn dtr(&self) -> bool {
        self.dtr
    }

    /// `dtr = value` (`:155-161`)
    pub fn set_dtr(&mut self, value: bool) -> Result<(), PyErr> {
        self.dtr = value;
        if value {
            self.os
                .ioctl(self.fd, libc::TIOCMBIS as c_ulong, TIOCM_DTR_STR)
        } else {
            self.os
                .ioctl(self.fd, libc::TIOCMBIC as c_ulong, TIOCM_DTR_STR)
        }
    }
}

#[cfg(test)]
mod tests;
