//! pyserial 3.5 `serial/serialwin32.py` `Serial` (BSD-3-Clause, (C) 2001-2020 Chris Liechti; the
//! ctypes calls are `serial/win32.py`), the part `comm.py` (OctoPrint 1.11.8 pins `pyserial>=3.5,<4`,
//! `setup.py:67`) uses: the Windows backend of `serial.rs`. Ported, not stood for by the
//! `serialport` crate: the crate opens a COM port without `FILE_FLAG_OVERLAPPED`
//! (serialport 4.10.1 `src/windows/com.rs:65`), and `read`, `write`, `cancel_read` and
//! `cancel_write` below are built on overlapped I/O (#181).
//!
//! Only what `serial.Serial(baudrate=.., timeout=.., write_timeout=.., exclusive=..)` with its
//! defaults reaches is here: 8 data bits, parity none or odd, 1 stop bit, no xonxoff, rtscts,
//! dsrdtr, inter-byte timeout or RS485 mode, RTS and DTR on (`serialutil.py:210-211`). The other
//! branches of `_reconfigure_port` and the methods `comm.py` never calls (`flush`,
//! `reset_*_buffer`, break, modem lines, `set_buffer_size`, `set_output_flow_control`,
//! `out_waiting`) are not ported.
//!
//! Differences from pyserial (all OURS):
//!  1. `ctypes.WinError()` in an exception text is the operating system's message
//!     (`std::io::Error`, "… (os error N)"), not Python's `repr` of an `OSError`.
//!  2. `write` with a write timeout of 0 returns while the operating system may still be
//!     writing from the buffer it was given. Python keeps no reference to `data` either; here the
//!     bytes are copied into a buffer the object keeps until the output queue is empty
//!     (`ClearCommError` `cbOutQue == 0`, checked before the next write and on close), so the
//!     memory is never freed under a pending write. What goes to the port is the same.
//!  3. A baud rate above `u32::MAX` is `ValueError` when it is set, as on Unix (`serialposix.rs`);
//!     ctypes would cut it to a `DWORD`.
//!  4. `Serial.__repr__` holds `id(self)`, a memory address: here the address of the object.
//!  5. `int(port[3:])` (`serialwin32.py:49`) is read as Python reads an ASCII int (spaces around,
//!     a sign, single `_` between digits); other Unicode digits count as no number.

use super::comm::{SerialError, py_float_repr};
use super::py_format::py_str_repr;
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use windows_sys::Win32::Devices::Communication::{
    COMMTIMEOUTS, COMSTAT, ClearCommError, DCB, EV_ERR, GetCommState, GetCommTimeouts, NOPARITY,
    ODDPARITY, ONESTOPBIT, PURGE_RXABORT, PURGE_RXCLEAR, PURGE_TXABORT, PURGE_TXCLEAR, PurgeComm,
    SetCommMask, SetCommState, SetCommTimeouts, SetupComm,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_USER_BUFFER, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING,
    ERROR_NOT_ENOUGH_MEMORY, ERROR_OPERATION_ABORTED, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE,
    GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::SystemServices::MAXDWORD;
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent};
use windows_sys::Win32::System::WindowsProgramming::{DTR_CONTROL_ENABLE, RTS_CONTROL_ENABLE};

/// `serial.XON`, `serial.XOFF` (`serialutil.py:72-73`)
const XON: i8 = 17;
const XOFF: i8 = 19;

/// The bit fields of `win32.py:305 _DCB` (`fBinary` 1 bit, `fParity` 1, `fOutxCtsFlow` 1,
/// `fOutxDsrFlow` 1, `fDtrControl` 2, `fDsrSensitivity` 1, `fTXContinueOnXoff` 1, `fOutX` 1,
/// `fInX` 1, `fErrorChar` 1, `fNull` 1, `fRtsControl` 2, `fAbortOnError` 1, `fDummy2` 17), as
/// `(shift, width)`.
mod dcb_bits {
    pub const F_BINARY: (u32, u32) = (0, 1);
    pub const F_PARITY: (u32, u32) = (1, 1);
    pub const F_OUTX_CTS_FLOW: (u32, u32) = (2, 1);
    pub const F_OUTX_DSR_FLOW: (u32, u32) = (3, 1);
    pub const F_DTR_CONTROL: (u32, u32) = (4, 2);
    pub const F_OUTX: (u32, u32) = (8, 1);
    pub const F_INX: (u32, u32) = (9, 1);
    pub const F_ERROR_CHAR: (u32, u32) = (10, 1);
    pub const F_NULL: (u32, u32) = (11, 1);
    pub const F_RTS_CONTROL: (u32, u32) = (12, 2);
    pub const F_ABORT_ON_ERROR: (u32, u32) = (14, 1);
}

/// `comDCB.<field> = value` on a ctypes bit field: the value is cut to the field's width.
fn set_bits(bitfield: &mut u32, (shift, width): (u32, u32), value: u32) {
    let mask = ((1u32 << width) - 1) << shift;
    *bitfield = (*bitfield & !mask) | ((value << shift) & mask);
}

/// `max(int(t * 1000), 1)` assigned to a ctypes `DWORD` (`serialwin32.py:116`, :125): `int`
/// truncates toward zero, the `DWORD` keeps the low 32 bits.
fn timeout_ms(t: f64) -> u32 {
    let ms = ((t * 1000.0).trunc() as i128).max(1);
    ms.rem_euclid(1i128 << 32) as u32
}

/// `ctypes.WinError()` (difference 1): the text of `GetLastError()`.
fn win_error() -> String {
    std::io::Error::last_os_error().to_string()
}

/// `int(s)` for an ASCII `str` (difference 5): `None` where Python raises `ValueError`.
fn py_int(s: &str) -> Option<i128> {
    let t = s.trim_matches(|c: char| c.is_ascii_whitespace());
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let mut v: i128 = 0;
    for c in digits.chars().filter(|c| *c != '_') {
        v = v
            .saturating_mul(10)
            .saturating_add(i128::from(c as u8 - b'0'));
    }
    Some(if neg { -v } else { v })
}

/// `serial.serialwin32.Serial`, the part the comm layer uses (`serial.Serial(baudrate=..,
/// timeout=.., write_timeout=.., exclusive=..)`, then `.port = ..`, `.parity = ..`, `.open()`).
pub struct Serial {
    port: Mutex<Option<String>>,
    baudrate: Mutex<i64>,
    timeout: Mutex<Option<f64>>,
    write_timeout: Mutex<Option<f64>>,
    parity_odd: AtomicBool,
    /// `_port_handle`; null is `None`
    port_handle: AtomicPtr<c_void>,
    /// `_overlapped_read`, `_overlapped_write`: on the heap, so the address the operating system
    /// writes to stays put while I/O is pending; `*_set` is "not `None`".
    overlapped_read: Box<UnsafeCell<OVERLAPPED>>,
    overlapped_read_set: AtomicBool,
    overlapped_write: Box<UnsafeCell<OVERLAPPED>>,
    overlapped_write_set: AtomicBool,
    /// `_orgTimeouts`
    org_timeouts: Mutex<COMMTIMEOUTS>,
    /// the buffers of writes that may still be pending (difference 2)
    written: Mutex<Vec<Box<[u8]>>>,
    is_open: AtomicBool,
}

// SAFETY: the raw handles and the two OVERLAPPED structures are only handed to the operating
// system, which allows these calls from any thread (pyserial calls `cancel_read` from another
// thread, as `comm.py` does); the structures are written by us only while no I/O is pending
// (`open`).
unsafe impl Send for Serial {}
// SAFETY: see `Send`.
unsafe impl Sync for Serial {}

impl Serial {
    /// `serial.Serial(baudrate=.., timeout=.., write_timeout=.., exclusive=..)`: no port yet, so
    /// nothing is opened. In the order `SerialBase.__init__` sets them: a baud rate that is not a
    /// non-negative int is `ValueError` (`serialutil.py:291-294`; difference 3), as is a negative
    /// timeout (:368, :384) and `exclusive=False` (`serialwin32.py:471`).
    pub fn new(
        baudrate: i64,
        timeout: Option<f64>,
        write_timeout: Option<f64>,
        exclusive: Option<bool>,
    ) -> Result<Serial, SerialError> {
        if !(0..=u32::MAX as i64).contains(&baudrate) {
            return Err(SerialError::other(
                "ValueError",
                format!("Not a valid baudrate: {baudrate:?}"),
            ));
        }
        for t in [timeout, write_timeout].into_iter().flatten() {
            if t < 0.0 {
                return Err(SerialError::other(
                    "ValueError",
                    format!("Not a valid timeout: {}", py_float_repr(t)),
                ));
            }
        }
        if exclusive == Some(false) {
            return Err(SerialError::other(
                "ValueError",
                "win32 only supports exclusive access (not: False)",
            ));
        }
        Ok(Serial {
            port: Mutex::new(None),
            baudrate: Mutex::new(baudrate),
            timeout: Mutex::new(timeout),
            write_timeout: Mutex::new(write_timeout),
            parity_odd: AtomicBool::new(false),
            port_handle: AtomicPtr::new(std::ptr::null_mut()),
            overlapped_read: Box::new(UnsafeCell::new(OVERLAPPED::default())),
            overlapped_read_set: AtomicBool::new(false),
            overlapped_write: Box::new(UnsafeCell::new(OVERLAPPED::default())),
            overlapped_write_set: AtomicBool::new(false),
            org_timeouts: Mutex::new(COMMTIMEOUTS::default()),
            written: Mutex::new(Vec::new()),
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

    /// `serial_obj.parity = serial.PARITY_ODD` (`odd`) / `serial.PARITY_NONE`. `comm.py:3907-3910`
    /// sets it only on a closed port, where the setter (`serialutil.py:318`) does not reconfigure.
    pub fn set_parity_odd(&self, odd: bool) {
        debug_assert!(!self.is_open(), "parity is only set on a closed port");
        self.parity_odd.store(odd, Ordering::SeqCst);
    }

    /// `serial_obj.baudrate`
    pub fn baudrate(&self) -> i64 {
        *self.baudrate.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `serial_obj.baudrate = b` (`serialutil.py:284`): a value that is no baud rate is
    /// `ValueError`; an open port is reconfigured.
    pub fn set_baudrate(&self, baudrate: i64) -> Result<(), SerialError> {
        if !(0..=u32::MAX as i64).contains(&baudrate) {
            return Err(SerialError::other(
                "ValueError",
                format!("Not a valid baudrate: {baudrate:?}"),
            ));
        }
        *self.baudrate.lock().unwrap_or_else(|e| e.into_inner()) = baudrate;
        if self.is_open() {
            self._reconfigure_port()?;
        }
        Ok(())
    }

    /// `serial_obj.timeout`
    pub fn timeout(&self) -> Option<f64> {
        *self.timeout.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `serial_obj.timeout = t` (`serialutil.py:361`): negative is `ValueError`; an open port is
    /// reconfigured (on Windows the timeouts are the port's `COMMTIMEOUTS`).
    pub fn set_timeout(&self, timeout: Option<f64>) -> Result<(), SerialError> {
        if let Some(t) = timeout
            && t < 0.0
        {
            return Err(SerialError::other(
                "ValueError",
                format!("Not a valid timeout: {}", py_float_repr(t)),
            ));
        }
        *self.timeout.lock().unwrap_or_else(|e| e.into_inner()) = timeout;
        if self.is_open() {
            self._reconfigure_port()?;
        }
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

    /// `serial_obj._port_handle` (`None` when closed)
    pub fn port_handle(&self) -> Option<HANDLE> {
        Some(self.port_handle.load(Ordering::SeqCst)).filter(|h| !h.is_null())
    }

    fn handle(&self) -> HANDLE {
        self.port_handle.load(Ordering::SeqCst)
    }

    fn overlapped_read(&self) -> *mut OVERLAPPED {
        self.overlapped_read.get()
    }

    fn overlapped_write(&self) -> *mut OVERLAPPED {
        self.overlapped_write.get()
    }

    /// `serialwin32.py:35 open()`: "Open port with current settings. This may throw a
    /// SerialException if the port cannot be opened."
    pub fn open(&self) -> Result<(), SerialError> {
        let Some(portstr) = self.port() else {
            return Err(SerialError::serial_exception(
                "Port must be configured before it can be used.",
            ));
        };
        if self.is_open() {
            return Err(SerialError::serial_exception("Port is already open."));
        }
        // the "\\.\COMx" format is required for devices other than COM1-COM8
        // not all versions of windows seem to support this properly
        // so that the first few ports are used with the DOS device name
        let mut port = portstr.clone();
        if portstr.to_uppercase().starts_with("COM") {
            let rest: String = portstr.chars().skip(3).collect();
            // `except ValueError: pass` for like COMnotanumber
            if py_int(&rest).is_some_and(|n| n > 8) {
                port = format!("\\\\.\\{portstr}");
            }
        }
        let wide: Vec<u16> = port.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string; the other arguments are plain values.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,                // exclusive access
                std::ptr::null(), // no security
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(SerialError::serial_exception(format!(
                "could not open port {}: {}",
                py_str_repr(&portstr, false),
                win_error()
            )));
        }
        self.port_handle.store(handle, Ordering::SeqCst);

        let setup = || -> Result<(), SerialError> {
            // SAFETY: no I/O is pending on a port that is not open yet, so nothing else touches
            // the two structures; `CreateEventW` gets no attributes and no name.
            unsafe {
                let read = &mut *self.overlapped_read();
                *read = OVERLAPPED::default();
                read.hEvent = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
                self.overlapped_read_set.store(true, Ordering::SeqCst);
                let write = &mut *self.overlapped_write();
                *write = OVERLAPPED::default();
                write.hEvent = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
                self.overlapped_write_set.store(true, Ordering::SeqCst);

                // Setup a 4k buffer
                SetupComm(handle, 4096, 4096);

                // Save original timeout values:
                let mut org = COMMTIMEOUTS::default();
                GetCommTimeouts(handle, &mut org);
                *self.org_timeouts.lock().unwrap_or_else(|e| e.into_inner()) = org;
            }

            self._reconfigure_port()?;

            // Clear buffers:
            // Remove anything that was there
            // SAFETY: a plain call on the open handle.
            unsafe {
                PurgeComm(
                    handle,
                    PURGE_TXCLEAR | PURGE_TXABORT | PURGE_RXCLEAR | PURGE_RXABORT,
                );
            }
            Ok(())
        };
        if let Err(e) = setup() {
            // ignore any exception when closing the port
            // also to keep original exception that happened when setting up
            let _ = self._close();
            self.port_handle
                .store(std::ptr::null_mut(), Ordering::SeqCst);
            return Err(e);
        }
        self.is_open.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// `serialwin32.py:100 _reconfigure_port()`: "Set communication parameters on opened port."
    fn _reconfigure_port(&self) -> Result<(), SerialError> {
        let handle = self.handle();
        if handle.is_null() {
            return Err(SerialError::serial_exception(
                "Can only operate on a valid port handle",
            ));
        }

        // Set Windows timeouts values
        // timeouts is a tuple with the following items:
        // (ReadIntervalTimeout,ReadTotalTimeoutMultiplier,
        //  ReadTotalTimeoutConstant,WriteTotalTimeoutMultiplier,
        //  WriteTotalTimeoutConstant)
        let mut timeouts = COMMTIMEOUTS::default();
        match self.timeout() {
            None => {} // default of all zeros is OK
            Some(t) if t == 0.0 => timeouts.ReadIntervalTimeout = MAXDWORD,
            Some(t) => timeouts.ReadTotalTimeoutConstant = timeout_ms(t),
        }
        // `if self._timeout != 0 and self._inter_byte_timeout is not None`: no inter-byte timeout

        match self.write_timeout() {
            None => {}
            Some(t) if t == 0.0 => timeouts.WriteTotalTimeoutConstant = MAXDWORD,
            Some(t) => timeouts.WriteTotalTimeoutConstant = timeout_ms(t),
        }
        // SAFETY: plain calls on the open handle with a structure on the stack.
        unsafe {
            SetCommTimeouts(handle, &timeouts);
            SetCommMask(handle, EV_ERR);
        }

        // Setup the connection info.
        // Get state and modify it:
        let mut com_dcb = DCB::default();
        // SAFETY: `com_dcb` is a DCB the call fills.
        unsafe { GetCommState(handle, &mut com_dcb) };
        com_dcb.BaudRate = self.baudrate() as u32;

        // `self._bytesize == serial.EIGHTBITS`
        com_dcb.ByteSize = 8;

        let bits = &mut com_dcb._bitfield;
        if self.parity_odd.load(Ordering::SeqCst) {
            com_dcb.Parity = ODDPARITY;
            set_bits(bits, dcb_bits::F_PARITY, 1); // Enable Parity Check
        } else {
            com_dcb.Parity = NOPARITY;
            set_bits(bits, dcb_bits::F_PARITY, 0); // Disable Parity Check
        }

        // `self._stopbits == serial.STOPBITS_ONE`
        com_dcb.StopBits = ONESTOPBIT;

        set_bits(bits, dcb_bits::F_BINARY, 1); // Enable Binary Transmission
        // Char. w/ Parity-Err are replaced with 0xff (if fErrorChar is set to TRUE)
        // `self._rs485_mode is None`, `self._rtscts` False, `self._rts_state` True
        set_bits(bits, dcb_bits::F_RTS_CONTROL, RTS_CONTROL_ENABLE);
        set_bits(bits, dcb_bits::F_OUTX_CTS_FLOW, 0);

        // `self._dsrdtr` False, `self._dtr_state` True
        set_bits(bits, dcb_bits::F_DTR_CONTROL, DTR_CONTROL_ENABLE);
        set_bits(bits, dcb_bits::F_OUTX_DSR_FLOW, 0);
        set_bits(bits, dcb_bits::F_OUTX, 0); // `self._xonxoff` False
        set_bits(bits, dcb_bits::F_INX, 0);
        set_bits(bits, dcb_bits::F_NULL, 0);
        set_bits(bits, dcb_bits::F_ERROR_CHAR, 0);
        set_bits(bits, dcb_bits::F_ABORT_ON_ERROR, 0);
        com_dcb.XonChar = XON;
        com_dcb.XoffChar = XOFF;

        // SAFETY: `com_dcb` is a filled DCB.
        if unsafe { SetCommState(handle, &com_dcb) } == 0 {
            return Err(SerialError::serial_exception(format!(
                "Cannot configure port, something went wrong. Original message: {}",
                win_error()
            )));
        }
        Ok(())
    }

    /// `serialwin32.py:229 _close()`: "internal close port helper"
    fn _close(&self) -> Result<(), SerialError> {
        let handle = self.handle();
        if !handle.is_null() {
            // Restore original timeout values:
            let org = *self.org_timeouts.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: a plain call on the handle this object owns.
            unsafe { SetCommTimeouts(handle, &org) };
            if self.overlapped_read_set.load(Ordering::SeqCst) {
                self.cancel_read()?;
                // SAFETY: closing the event this object created.
                unsafe { CloseHandle((*self.overlapped_read()).hEvent) };
                self.overlapped_read_set.store(false, Ordering::SeqCst);
            }
            if self.overlapped_write_set.load(Ordering::SeqCst) {
                self.cancel_write()?;
                // SAFETY: closing the event this object created.
                unsafe { CloseHandle((*self.overlapped_write()).hEvent) };
                self.overlapped_write_set.store(false, Ordering::SeqCst);
            }
            // SAFETY: closing the handle this object owns.
            unsafe { CloseHandle(handle) };
            self.port_handle
                .store(std::ptr::null_mut(), Ordering::SeqCst);
        }
        Ok(())
    }

    /// `serialwin32.py:245 close()`: "Close port"
    pub fn close(&self) {
        if self.is_open() {
            let _ = self._close();
            self.is_open.store(false, Ordering::SeqCst);
        }
    }

    fn port_not_open() -> SerialError {
        SerialError::new(
            "PortNotOpenError",
            "Attempting to use a port that is not open",
            true,
        )
    }

    /// `ClearCommError(self._port_handle, byref(flags), byref(comstat))`, `Err` its
    /// `SerialException`.
    fn comstat(&self) -> Result<COMSTAT, SerialError> {
        let mut flags = 0u32;
        let mut comstat = COMSTAT::default();
        // SAFETY: `flags` and `comstat` are what the call fills.
        if unsafe { ClearCommError(self.handle(), &mut flags, &mut comstat) } == 0 {
            return Err(SerialError::serial_exception(format!(
                "ClearCommError failed ({})",
                win_error()
            )));
        }
        Ok(comstat)
    }

    /// `serialwin32.py:254 in_waiting`: "Return the number of bytes currently in the input buffer."
    pub fn in_waiting(&self) -> Result<usize, SerialError> {
        Ok(self.comstat()?.cbInQue as usize)
    }

    /// `serialwin32.py:262 read(size=1)`: "Read size bytes from the serial port. If a timeout is
    /// set it may return less characters as requested. With no timeout it will block until the
    /// requested number of bytes is read."
    pub fn read(&self, size: usize) -> Result<Vec<u8>, SerialError> {
        if !self.is_open() {
            return Err(Self::port_not_open());
        }
        if size == 0 {
            return Ok(Vec::new());
        }
        let handle = self.handle();
        let overlapped = self.overlapped_read();
        // SAFETY: the event this object created.
        unsafe { ResetEvent((*overlapped).hEvent) };
        let comstat = self.comstat()?;
        let n = if self.timeout() == Some(0.0) {
            (comstat.cbInQue as usize).min(size)
        } else {
            size
        };
        if n == 0 {
            return Ok(Vec::new());
        }
        let mut buf = vec![0u8; n];
        let mut rc = 0u32;
        // SAFETY: `buf` holds `n` bytes and lives until `GetOverlappedResult` below has waited for
        // the read to end; `overlapped` is this object's read structure.
        let read_ok = unsafe { ReadFile(handle, buf.as_mut_ptr(), n as u32, &mut rc, overlapped) };
        if read_ok == 0 {
            // SAFETY: a plain call.
            let err = unsafe { GetLastError() };
            if err != ERROR_SUCCESS && err != ERROR_IO_PENDING {
                return Err(SerialError::serial_exception(format!(
                    "ReadFile failed ({})",
                    win_error()
                )));
            }
        }
        // SAFETY: waits (`bWait` True) for the read started above on the same structure.
        let result_ok = unsafe { GetOverlappedResult(handle, overlapped, &mut rc, 1) };
        if result_ok == 0 {
            // SAFETY: a plain call.
            if unsafe { GetLastError() } != ERROR_OPERATION_ABORTED {
                return Err(SerialError::serial_exception(format!(
                    "GetOverlappedResult failed ({})",
                    win_error()
                )));
            }
        }
        buf.truncate(rc as usize);
        Ok(buf)
    }

    /// `serialwin32.py:303 write(data)`: "Output the given byte string over the serial port."
    pub fn write(&self, data: &[u8]) -> Result<usize, SerialError> {
        if !self.is_open() {
            return Err(Self::port_not_open());
        }
        if data.is_empty() {
            return Ok(0);
        }
        let handle = self.handle();
        let overlapped = self.overlapped_write();
        // difference 2: keep the bytes until no write can be pending on them
        let ptr = {
            let mut written = self.written.lock().unwrap_or_else(|e| e.into_inner());
            if self.comstat().is_ok_and(|c| c.cbOutQue == 0) {
                written.clear();
            }
            let kept: Box<[u8]> = data.into();
            let ptr = kept.as_ptr();
            written.push(kept);
            ptr
        };
        let mut n = 0u32;
        // SAFETY: `ptr` points at `data.len()` bytes kept in `self.written` (difference 2);
        // `overlapped` is this object's write structure.
        let success = unsafe { WriteFile(handle, ptr, data.len() as u32, &mut n, overlapped) };
        if self.write_timeout() != Some(0.0) {
            // if blocking (None) or w/ write timeout (>0)
            if success == 0 {
                // SAFETY: a plain call.
                let err = unsafe { GetLastError() };
                if err != ERROR_SUCCESS && err != ERROR_IO_PENDING {
                    return Err(SerialError::serial_exception(format!(
                        "WriteFile failed ({})",
                        win_error()
                    )));
                }
            }

            // Wait for the write to complete.
            // SAFETY: waits for the write started above on the same structure.
            unsafe { GetOverlappedResult(handle, overlapped, &mut n, 1) };
            // SAFETY: a plain call.
            if unsafe { GetLastError() } == ERROR_OPERATION_ABORTED {
                return Ok(n as usize); // canceled IO is no error
            }
            if n as usize != data.len() {
                return Err(SerialError::new(
                    "SerialTimeoutException",
                    "Write timeout",
                    true,
                ));
            }
            Ok(n as usize)
        } else {
            let errorcode = if success != 0 {
                ERROR_SUCCESS
            } else {
                // SAFETY: a plain call.
                unsafe { GetLastError() }
            };
            if [
                ERROR_INVALID_USER_BUFFER,
                ERROR_NOT_ENOUGH_MEMORY,
                ERROR_OPERATION_ABORTED,
            ]
            .contains(&errorcode)
            {
                Ok(0)
            } else if [ERROR_SUCCESS, ERROR_IO_PENDING].contains(&errorcode) {
                // no info on true length provided by OS function in async mode
                Ok(data.len())
            } else {
                Err(SerialError::serial_exception(format!(
                    "WriteFile failed ({})",
                    win_error()
                )))
            }
        }
    }

    /// `serialwin32.py:450 _cancel_overlapped_io(overlapped)`: "Cancel a blocking read operation,
    /// may be called from other thread". `set` false is `overlapped` `None`, where
    /// `ctypes.byref(None)` raises `TypeError`.
    fn _cancel_overlapped_io(
        &self,
        overlapped: *mut OVERLAPPED,
        set: bool,
    ) -> Result<(), SerialError> {
        if !set {
            return Err(SerialError::other(
                "TypeError",
                "byref() argument must be a ctypes instance, not 'NoneType'",
            ));
        }
        let handle = self.handle();
        // check if read operation is pending
        let mut rc = 0u32;
        // SAFETY: asks (`bWait` False) for the state of this object's structure.
        let err = unsafe { GetOverlappedResult(handle, overlapped, &mut rc, 0) };
        if err == 0 {
            // SAFETY: a plain call.
            let last = unsafe { GetLastError() };
            if last == ERROR_IO_PENDING || last == ERROR_IO_INCOMPLETE {
                // cancel, ignoring any errors (e.g. it may just have finished on its own)
                // SAFETY: cancels I/O on the handle and structure of this object.
                unsafe { CancelIoEx(handle, overlapped) };
            }
        }
        Ok(())
    }

    /// `serialwin32.py:463 cancel_read()`: "Cancel a blocking read operation, may be called from
    /// other thread"
    pub fn cancel_read(&self) -> Result<(), SerialError> {
        self._cancel_overlapped_io(
            self.overlapped_read(),
            self.overlapped_read_set.load(Ordering::SeqCst),
        )
    }

    /// `serialwin32.py:467 cancel_write()`: "Cancel a blocking write operation, may be called from
    /// other thread"
    pub fn cancel_write(&self) -> Result<(), SerialError> {
        self._cancel_overlapped_io(
            self.overlapped_write(),
            self.overlapped_write_set.load(Ordering::SeqCst),
        )
    }

    /// `serialutil.py:527 __repr__` (difference 4: `id` is the address of this object)
    pub fn repr(&self) -> String {
        format!(
            "Serial<id=0x{:x}, open={}>(port={}, baudrate={}, bytesize=8, parity='{}', stopbits=1, timeout={}, xonxoff=False, rtscts=False, dsrdtr=False)",
            self as *const Serial as usize,
            if self.is_open() { "True" } else { "False" },
            match self.port() {
                Some(p) => py_str_repr(&p, false),
                None => "None".to_string(),
            },
            self.baudrate(),
            if self.parity_odd.load(Ordering::SeqCst) {
                'O'
            } else {
                'N'
            },
            self.timeout()
                .map_or_else(|| "None".to_string(), py_float_repr),
        )
    }
}

impl Drop for Serial {
    /// `io.RawIOBase.__del__` closes
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    //! OURS: pyserial's tests need a real port or a loopback (`test/test_*.py` take a port
    //! argument). These cover what runs without one.
    use super::*;

    /// `win32.py:305 _DCB`: what `_reconfigure_port` sets lands in the documented bits; the bits
    /// it leaves alone (`fDsrSensitivity` 6, `fTXContinueOnXoff` 7, `fDummy2` 15-31) stay.
    #[test]
    fn dcb_bits_are_the_ctypes_fields() {
        let mut b: u32 = 0b1100_0000 | (0x1_ffff << 15);
        set_bits(&mut b, dcb_bits::F_BINARY, 1);
        set_bits(&mut b, dcb_bits::F_RTS_CONTROL, RTS_CONTROL_ENABLE);
        set_bits(&mut b, dcb_bits::F_DTR_CONTROL, DTR_CONTROL_ENABLE);
        assert_eq!(b, 0b1100_0000 | (0x1_ffff << 15) | 1 | (1 << 12) | (1 << 4));
        set_bits(&mut b, dcb_bits::F_RTS_CONTROL, 0b111);
        assert_eq!(b >> 12 & 0b11, 0b11, "cut to the field's two bits");
    }

    /// `max(int(t * 1000), 1)` into a `DWORD`
    #[test]
    fn timeouts_in_ms() {
        assert_eq!(timeout_ms(2.0), 2000);
        assert_eq!(timeout_ms(0.0004), 1);
        assert_eq!(timeout_ms(0.0019), 1);
        assert_eq!(timeout_ms(1.9999), 1999);
    }

    /// `int(port[3:]) > 8` (`serialwin32.py:49`)
    #[test]
    fn com_numbers() {
        assert_eq!(py_int("9"), Some(9));
        assert_eq!(py_int(" 12 "), Some(12));
        assert_eq!(py_int("1_0"), Some(10));
        assert_eq!(py_int("+3"), Some(3));
        assert_eq!(py_int("notanumber"), None);
        assert_eq!(py_int(""), None);
        assert_eq!(py_int("1__0"), None);
        assert_eq!(py_int("_1"), None);
    }

    /// A port that does not exist is the `SerialException` of `open` (`serialwin32.py:62-64`).
    #[test]
    fn a_missing_port_does_not_open() {
        let s = Serial::new(115200, Some(2.0), Some(0.0), None).unwrap();
        s.set_port("COM250");
        let e = s.open().unwrap_err();
        assert!(e.serial_exception, "{e:?}");
        assert!(
            e.message.starts_with("could not open port 'COM250': "),
            "{e:?}"
        );
        assert!(!s.is_open());
        assert!(s.port_handle().is_none());
    }

    #[test]
    fn exclusive_false_is_refused() {
        let e = Serial::new(115200, None, None, Some(false)).err().unwrap();
        assert_eq!(e.type_name, "ValueError");
        assert_eq!(
            e.message,
            "win32 only supports exclusive access (not: False)"
        );
    }

    /// `cancel_read` on a closed port: `ctypes.byref(None)`
    #[test]
    fn cancel_on_a_closed_port_is_type_error() {
        let s = Serial::new(115200, None, None, None).unwrap();
        assert_eq!(s.cancel_read().unwrap_err().type_name, "TypeError");
    }
}
