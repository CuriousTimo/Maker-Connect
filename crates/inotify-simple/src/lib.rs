//! `inotify_simple.py` 1.3.5 (github.com/chrisjbillington/inotify_simple at `2a8f830`, the
//! version the Connect SDK requires: `inotify_simple~=1.3.5`) ported to Rust, function by
//! function, on CPython 3.13.12's `select.poll`, `fcntl.ioctl`, `os.read` and `io.FileIO`.
//!
//! Copyright (c) 2016, Chris Billington. All rights reserved.
//!
//! Redistribution and use in source and binary forms, with or without modification, are
//! permitted provided that the following conditions are met:
//!
//! 1. Redistributions of source code must retain the above copyright notice, this list of
//!    conditions and the following disclaimer.
//! 2. Redistributions in binary form must reproduce the above copyright notice, this list of
//!    conditions and the following disclaimer in the documentation and/or other materials
//!    provided with the distribution.
//!
//! THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS
//! OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF
//! MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE
//! COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
//! EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
//! SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
//! HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
//! (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN
//! IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
//!
//! Python's types as they are here: an `Event`'s `name` (a `str` from `os.fsdecode`, which on
//! Linux turns bytes into a `str` and back without loss) is an `std::ffi::OsString`; a path
//! given to `add_watch` (encoded with `os.fsencode`) is a `std::path::Path`; `timeout` and
//! `read_delay` are whole milliseconds (Python also takes a `float`; the SDK passes `int`s).
//! `INotify` is Linux only, as the reference (`CDLL('libc.so.6')`); `parse_events` works on any
//! Unix, the flags everywhere. Python 2 paths are not ported.

#[cfg(unix)]
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::path::Path;

pub const VERSION: &str = "1.3.5";

/// A Python exception: `OSError` (with its errno) or another class with its message.
#[derive(Debug)]
pub enum Error {
    /// `OSError(errno, os.strerror(errno))`
    Os(std::io::Error),
    /// `struct.error` (class `error`), `ValueError`, `OverflowError`: the class and message.
    Py {
        class: &'static str,
        message: String,
    },
}

impl Error {
    #[cfg(unix)]
    fn py(class: &'static str, message: impl Into<String>) -> Error {
        Error::Py {
            class,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Os(err) => write!(f, "OSError: {err}"),
            Error::Py { class, message } => write!(f, "{class}: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Error {
        Error::Os(err)
    }
}

/// Wrapper which raises errors and retries on EINTR.
#[cfg(target_os = "linux")]
fn libc_call(function: impl Fn() -> libc::c_int) -> Result<libc::c_int, Error> {
    loop {
        let rc = function();
        if rc != -1 {
            return Ok(rc);
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EINTR) {
            return Err(Error::Os(err));
        }
    }
}

/// A ``namedtuple`` (wd, mask, cookie, name) for an inotify event.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub wd: i32,
    pub mask: u32,
    pub cookie: u32,
    pub name: OsString,
}

/// `calcsize('iIII')`
pub const EVENT_SIZE: usize = 16;

/// File-like object wrapping ``inotify_init1()``.
///
/// Python's `INotify(FileIO)`: the file descriptor is closed by [`INotify::close`], or when it
/// is dropped (`FileIO.__del__`, whose error is not raised).
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct INotify {
    /// `FileIO`'s fd: `None` once closed.
    fd: Option<libc::c_int>,
    /// `select.poll()` with the fd registered (`POLLIN | POLLPRI | POLLOUT`, its default).
    poller: libc::pollfd,
}

#[cfg(target_os = "linux")]
impl INotify {
    /// ``INotify(inheritable=False, nonblocking=False)``. Raises ``OSError`` on failure.
    ///
    /// `inheritable`: whether the inotify file descriptor will be inherited by child processes.
    /// `false` corresponds to passing the ``IN_CLOEXEC`` flag to ``inotify_init1()``.
    /// `nonblocking`: whether to open the inotify file descriptor in nonblocking mode
    /// (``IN_NONBLOCK``). This does not affect [`INotify::read`], which uses ``poll()``.
    pub fn new(inheritable: bool, nonblocking: bool) -> Result<INotify, Error> {
        let flags = (if !inheritable { libc::O_CLOEXEC } else { 0 })
            | (if nonblocking { libc::O_NONBLOCK } else { 0 });
        // SAFETY: inotify_init1 takes flags only.
        let fd = libc_call(|| unsafe { libc::inotify_init1(flags) })?;
        file_io_init(fd)?;
        Ok(INotify {
            fd: Some(fd),
            poller: libc::pollfd {
                fd,
                events: libc::POLLIN | libc::POLLPRI | libc::POLLOUT,
                revents: 0,
            },
        })
    }

    /// `fileno()`: the inotify file descriptor returned by ``inotify_init()``.
    pub fn fileno(&self) -> Result<libc::c_int, Error> {
        self.fd
            .ok_or_else(|| Error::py("ValueError", "I/O operation on closed file"))
    }

    /// The `fd` property: [`INotify::fileno`] (`fd = property(FileIO.fileno)`).
    pub fn fd(&self) -> Result<libc::c_int, Error> {
        self.fileno()
    }

    /// Wrapper around ``inotify_add_watch()``. Returns the watch descriptor or raises an
    /// ``OSError`` on failure.
    ///
    /// `path`: the path to watch, encoded with ``os.fsencode()`` (its bytes). ctypes passes
    /// them as a `char *`, so the path ends at a NUL byte, if it has one. `mask`: the mask of
    /// events to watch for, [`Flag`]s ORed together.
    pub fn add_watch(&self, path: &Path, mask: u32) -> Result<i32, Error> {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        let bytes = bytes.split(|b| *b == 0).next().unwrap_or(&[]);
        let path = std::ffi::CString::new(bytes).expect("cut at the first NUL");
        let fd = self.fileno()?;
        // SAFETY: a valid NUL-terminated path for the call's duration.
        libc_call(|| unsafe { libc::inotify_add_watch(fd, path.as_ptr(), mask) })
    }

    /// Wrapper around ``inotify_rm_watch()``. Raises ``OSError`` on failure.
    pub fn rm_watch(&self, wd: i32) -> Result<(), Error> {
        let fd = self.fileno()?;
        // SAFETY: plain integers.
        libc_call(|| unsafe { libc::inotify_rm_watch(fd, wd) })?;
        Ok(())
    }

    /// Read the inotify file descriptor and return the resulting [`Event`]s.
    ///
    /// `timeout`: the time in milliseconds to wait for events if there are none. If negative or
    /// `None`, block until there are events. If zero, return immediately if there are no events
    /// to be read. `read_delay`: if there are no events immediately available for reading, the
    /// time in milliseconds to wait after the first event arrives before reading the file
    /// descriptor, so that the kernel can coalesce like events.
    pub fn read(
        &mut self,
        timeout: Option<i64>,
        read_delay: Option<i64>,
    ) -> Result<Vec<Event>, Error> {
        let mut data = self.readall()?;
        if data.is_empty() && timeout != Some(0) && !self.poll(timeout)?.is_empty() {
            if let Some(read_delay) = read_delay {
                sleep(read_delay as f64 / 1000.0)?;
            }
            data = self.readall()?;
        }
        parse_events(&data)
    }

    fn readall(&self) -> Result<Vec<u8>, Error> {
        let bytes_avail = ioctl_fionread(self.fileno()?)?;
        if bytes_avail == 0 {
            return Ok(Vec::new());
        }
        os_read(self.fileno()?, bytes_avail)
    }

    /// `self._poller.poll(timeout)` (CPython 3.13 `select_poll_poll_impl`): the `(fd, revents)`
    /// that fired.
    fn poll(&mut self, timeout: Option<i64>) -> Result<Vec<(i32, i32)>, Error> {
        let mut timeout = timeout.unwrap_or(-1);
        if timeout < i32::MIN as i64 || timeout > i32::MAX as i64 {
            return Err(Error::py("OverflowError", "timeout is too large"));
        }
        let mut ms = timeout;
        let deadline = (timeout >= 0)
            .then(|| std::time::Instant::now() + std::time::Duration::from_millis(timeout as u64));
        if ms < 0 {
            ms = -1;
        }
        let result = loop {
            self.poller.revents = 0;
            // SAFETY: one valid pollfd.
            let result = unsafe { libc::poll(&mut self.poller, 1, ms as libc::c_int) };
            let err = std::io::Error::last_os_error();
            if !(result < 0 && err.raw_os_error() == Some(libc::EINTR)) {
                if result < 0 {
                    return Err(Error::Os(err));
                }
                break result;
            }
            // poll() was interrupted by a signal
            if let Some(deadline) = deadline {
                let now = std::time::Instant::now();
                if now > deadline {
                    break 0;
                }
                let left = deadline - now;
                // _PyTime_AsMilliseconds(timeout, _PyTime_ROUND_CEILING)
                timeout = left.as_nanos().div_ceil(1_000_000) as i64;
                ms = timeout;
                // retry poll() with the recomputed timeout
            }
        };
        let mut fired = Vec::new();
        if result > 0 && self.poller.revents != 0 {
            fired.push((self.poller.fd, (self.poller.revents as i32) & 0xffff));
        }
        Ok(fired)
    }

    /// `FileIO.close()`: closes the fd; a second call does nothing.
    pub fn close(&mut self) -> Result<(), Error> {
        if let Some(fd) = self.fd.take() {
            // SAFETY: our own fd, closed once.
            if unsafe { libc::close(fd) } < 0 {
                return Err(Error::Os(std::io::Error::last_os_error()));
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Drop for INotify {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// What `FileIO.__init__(fd, mode='rb')` does with a file descriptor it is given (CPython 3.13
/// `_io_FileIO___init___impl`): `fstat` errors other than `EBADF` are tolerated, a directory is
/// `IsADirectoryError`. On error the fd is not closed (it is not the `FileIO`'s own).
#[cfg(target_os = "linux")]
fn file_io_init(fd: libc::c_int) -> Result<(), Error> {
    // SAFETY: a zeroed stat is valid, filled by fstat.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: a valid stat buffer.
    if unsafe { libc::fstat(fd, &mut st) } < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EBADF) {
            return Err(Error::Os(err));
        }
    } else if st.st_mode & libc::S_IFMT == libc::S_IFDIR {
        return Err(Error::Os(std::io::Error::from_raw_os_error(libc::EISDIR)));
    }
    Ok(())
}

/// `ioctl(fd, FIONREAD, c_int())` (CPython 3.13 `fcntl_ioctl_impl`: no retry on EINTR).
#[cfg(target_os = "linux")]
fn ioctl_fionread(fd: libc::c_int) -> Result<usize, Error> {
    let mut bytes_avail: libc::c_int = 0;
    // A negative count (never given by the kernel) would make Python's `os.read` fail; read
    // as none here (see the audit's "Not covered").
    // SAFETY: FIONREAD writes one int.
    if unsafe { libc::ioctl(fd, libc::FIONREAD, &mut bytes_avail) } < 0 {
        return Err(Error::Os(std::io::Error::last_os_error()));
    }
    Ok(bytes_avail.max(0) as usize)
}

/// `os.read(fd, n)` (CPython 3.13 `_Py_read`: retried on EINTR; the result cut to what was
/// read).
#[cfg(target_os = "linux")]
fn os_read(fd: libc::c_int, n: usize) -> Result<Vec<u8>, Error> {
    let n = n.min(isize::MAX as usize);
    let mut buf = vec![0u8; n];
    loop {
        // SAFETY: buf has n bytes.
        let got = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), n) };
        if got >= 0 {
            buf.truncate(got as usize);
            return Ok(buf);
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EINTR) {
            return Err(Error::Os(err));
        }
    }
}

/// `time.sleep(seconds)`. A length too large for a `Duration` is an `OverflowError`, as
/// Python's is for one too large for its clock (whose message is not compared).
#[cfg(target_os = "linux")]
fn sleep(seconds: f64) -> Result<(), Error> {
    if seconds < 0.0 {
        return Err(Error::py("ValueError", "sleep length must be non-negative"));
    }
    let length = std::time::Duration::try_from_secs_f64(seconds)
        .map_err(|_| Error::py("OverflowError", "sleep length too large"))?;
    std::thread::sleep(length);
    Ok(())
}

/// Unpack data read from an inotify file descriptor into [`Event`]s (wd, mask, cookie, name).
/// For an application that reads the raw data from the inotify file descriptor itself.
#[cfg(unix)]
pub fn parse_events(data: &[u8]) -> Result<Vec<Event>, Error> {
    use std::os::unix::ffi::OsStringExt;
    let len = data.len() as u64;
    let mut pos: u64 = 0;
    let mut events = Vec::new();
    while pos < len {
        // unpack_from(_EVENT_FMT, data, pos)
        if len - pos < EVENT_SIZE as u64 {
            return Err(Error::py(
                "error",
                format!(
                    "unpack_from requires a buffer of at least {} bytes for unpacking {EVENT_SIZE} \
                     bytes at offset {pos} (actual buffer size is {len})",
                    pos + EVENT_SIZE as u64
                ),
            ));
        }
        let at = |i: u64| -> [u8; 4] {
            let i = (pos + i) as usize;
            data[i..i + 4].try_into().expect("4 bytes")
        };
        let wd = i32::from_ne_bytes(at(0));
        let mask = u32::from_ne_bytes(at(4));
        let cookie = u32::from_ne_bytes(at(8));
        let namesize = u32::from_ne_bytes(at(12)) as u64;
        pos += EVENT_SIZE as u64 + namesize;
        // data[pos - namesize : pos], as Python slices: clamped to the data
        let start = (pos - namesize).min(len) as usize;
        let end = pos.min(len) as usize;
        let field = &data[start..end.max(start)];
        let name = field.split(|b| *b == 0).next().unwrap_or(&[]);
        events.push(Event {
            wd,
            mask,
            cookie,
            name: OsString::from_vec(name.to_vec()),
        });
    }
    Ok(events)
}

/// Inotify flags as defined in ``inotify.h`` but with ``IN_`` prefix omitted (`flags`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Flag {
    /// File was accessed
    Access = 0x00000001,
    /// File was modified
    Modify = 0x00000002,
    /// Metadata changed
    Attrib = 0x00000004,
    /// Writable file was closed
    CloseWrite = 0x00000008,
    /// Unwritable file closed
    CloseNowrite = 0x00000010,
    /// File was opened
    Open = 0x00000020,
    /// File was moved from X
    MovedFrom = 0x00000040,
    /// File was moved to Y
    MovedTo = 0x00000080,
    /// Subfile was created
    Create = 0x00000100,
    /// Subfile was deleted
    Delete = 0x00000200,
    /// Self was deleted
    DeleteSelf = 0x00000400,
    /// Self was moved
    MoveSelf = 0x00000800,

    /// Backing fs was unmounted
    Unmount = 0x00002000,
    /// Event queue overflowed
    QOverflow = 0x00004000,
    /// File was ignored
    Ignored = 0x00008000,

    /// only watch the path if it is a directory
    Onlydir = 0x01000000,
    /// don't follow a sym link
    DontFollow = 0x02000000,
    /// exclude events on unlinked objects
    ExclUnlink = 0x04000000,
    /// add to the mask of an already existing watch
    MaskAdd = 0x20000000,
    /// event occurred against dir
    Isdir = 0x40000000,
    /// only send event once
    Oneshot = 0x80000000,
}

impl Flag {
    /// `flags.__members__.values()`, in definition order.
    pub const MEMBERS: [Flag; 21] = [
        Flag::Access,
        Flag::Modify,
        Flag::Attrib,
        Flag::CloseWrite,
        Flag::CloseNowrite,
        Flag::Open,
        Flag::MovedFrom,
        Flag::MovedTo,
        Flag::Create,
        Flag::Delete,
        Flag::DeleteSelf,
        Flag::MoveSelf,
        Flag::Unmount,
        Flag::QOverflow,
        Flag::Ignored,
        Flag::Onlydir,
        Flag::DontFollow,
        Flag::ExclUnlink,
        Flag::MaskAdd,
        Flag::Isdir,
        Flag::Oneshot,
    ];

    /// The flag's value (`int(flag)`).
    pub const fn bits(self) -> u32 {
        self as u32
    }

    /// The flag's name in Python (`flag.name`).
    pub const fn name(self) -> &'static str {
        match self {
            Flag::Access => "ACCESS",
            Flag::Modify => "MODIFY",
            Flag::Attrib => "ATTRIB",
            Flag::CloseWrite => "CLOSE_WRITE",
            Flag::CloseNowrite => "CLOSE_NOWRITE",
            Flag::Open => "OPEN",
            Flag::MovedFrom => "MOVED_FROM",
            Flag::MovedTo => "MOVED_TO",
            Flag::Create => "CREATE",
            Flag::Delete => "DELETE",
            Flag::DeleteSelf => "DELETE_SELF",
            Flag::MoveSelf => "MOVE_SELF",
            Flag::Unmount => "UNMOUNT",
            Flag::QOverflow => "Q_OVERFLOW",
            Flag::Ignored => "IGNORED",
            Flag::Onlydir => "ONLYDIR",
            Flag::DontFollow => "DONT_FOLLOW",
            Flag::ExclUnlink => "EXCL_UNLINK",
            Flag::MaskAdd => "MASK_ADD",
            Flag::Isdir => "ISDIR",
            Flag::Oneshot => "ONESHOT",
        }
    }

    /// Convenience method that returns a list of every flag in a mask.
    pub fn from_mask(mask: u32) -> Vec<Flag> {
        Flag::MEMBERS
            .into_iter()
            .filter(|flag| flag.bits() & mask != 0)
            .collect()
    }
}

/// Convenience masks as defined in ``inotify.h`` but with ``IN_`` prefix omitted (`masks`).
pub mod masks {
    use super::Flag;

    /// helper event mask equal to ``flags.CLOSE_WRITE | flags.CLOSE_NOWRITE``
    pub const CLOSE: u32 = Flag::CloseWrite.bits() | Flag::CloseNowrite.bits();
    /// helper event mask equal to ``flags.MOVED_FROM | flags.MOVED_TO``
    pub const MOVE: u32 = Flag::MovedFrom.bits() | Flag::MovedTo.bits();
    /// bitwise-OR of all the events that can be passed to `INotify::add_watch`
    pub const ALL_EVENTS: u32 = Flag::Access.bits()
        | Flag::Modify.bits()
        | Flag::Attrib.bits()
        | Flag::CloseWrite.bits()
        | Flag::CloseNowrite.bits()
        | Flag::Open.bits()
        | Flag::MovedFrom.bits()
        | Flag::MovedTo.bits()
        | Flag::Create.bits()
        | Flag::Delete.bits()
        | Flag::DeleteSelf.bits()
        | Flag::MoveSelf.bits();
}
