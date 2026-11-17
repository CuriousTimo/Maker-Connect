// Ported from CUPS 2.4.20 (reference/cups-2.4.20/backend/usb-libusb.c and cups/backchannel.c).
// Copyright © 2020-2025 by OpenPrinting, 2007-2019 by Apple Inc. Apache-2.0 with the GPL/LGPL exception
// (reference/cups-2.4.20/LICENSE). libusb is replaced by `nusb` (ours, W5), as the PnP port's pyusb is.

//! CUPS's `usb` backend as the LabelWriter 450's filter sees it: the pipe its stdout is, the back channel
//! (fd 3) the printer's answers come on, and the printer. `print_device` (`usb-libusb.c:169-687`) is
//! [`Backend`]: [`Backend::start`] is what it does before its read loop, [`Backend::write`] the read loop's
//! body (`print_buffer` of 8192 bytes, `libusb_bulk_transfer`), [`Backend::finish`] what it does after.
//!
//! **Ours (W5):** a write of the filter's reaches the backend as one call (the real pipe cuts and joins
//! writes as the kernel does: the bytes are the same); the printer is a [`UsbPrinterIo`] (libusb's bulk
//! transfers), found by a [`UsbHost`] ([`NusbHost`], `find_device` over nusb); the back channel is
//! [`BackChannel`], an in-process pipe; the printer is matched by its IEEE-1284 manufacturer and model
//! (the parts of `usb://DYMO/LabelWriter%20450`), the URI is not assembled.

use super::super::stderr;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// `backend.h` `CUPS_BACKEND_OK` and `CUPS_BACKEND_FAILED`.
pub const CUPS_BACKEND_OK: i32 = 0;
pub const CUPS_BACKEND_FAILED: i32 = 1;

/// `usb-libusb.c:28-30`.
const WAIT_EOF: bool = false;
const WAIT_EOF_DELAY: u64 = 7;

/// libusb's errors the backend tells apart (`libusb.h` `LIBUSB_ERROR_*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsbError {
    /// `LIBUSB_ERROR_TIMEOUT` (-7)
    Timeout,
    /// `LIBUSB_ERROR_PIPE` (-9)
    Pipe,
    /// `LIBUSB_ERROR_INTERRUPTED` (-10)
    Interrupted,
    /// Any other: libusb's code.
    Other(i32),
}

impl UsbError {
    /// The code `libusb_bulk_transfer` returns (`DEBUG: libusb write operation returned %x.`).
    pub fn code(&self) -> i32 {
        match self {
            UsbError::Timeout => -7,
            UsbError::Pipe => -9,
            UsbError::Interrupted => -10,
            UsbError::Other(c) => *c,
        }
    }
}

/// An open USB printer: `libusb_bulk_transfer` on its write and read endpoints (`usb_printer_t`).
pub trait UsbPrinterIo: Send + Sync {
    /// `libusb_bulk_transfer(handle, write_endp, data, len, &bytes, 0)`: the bytes written.
    fn bulk_write(&self, data: &[u8]) -> Result<usize, UsbError>;
    /// `libusb_bulk_transfer(handle, read_endp, buf, max, &rbytes, timeout)`.
    fn bulk_read(&self, max: usize, timeout_ms: u64) -> Result<Vec<u8>, UsbError>;
    /// `printer->read_endp != -1`.
    fn has_read_endpoint(&self) -> bool;
    /// `close_device` (:691-798): release the interface, restore the configuration, re-attach the kernel
    /// driver, close. Also ends a pending `bulk_read` (ours).
    fn close(&self);
}

/// What `find_device` looks through (`libusb_get_device_list`): the printers it finds, matched by
/// `print_cb` against the URI asked for.
pub trait UsbHost: Send + Sync {
    /// `find_device(print_cb, uri)` (:820-1043): an open printer that matches, or none.
    fn find_device(&self, manufacturer: &str, model: &str) -> Option<Arc<dyn UsbPrinterIo>>;
}

/// The pipe of fd 3: `cupsBackChannelWrite` (`backchannel.c:90-`) fills it, `cupsBackChannelRead` (:41-76)
/// empties it.
pub struct BackChannel {
    state: Mutex<(VecDeque<u8>, bool)>,
    cond: Condvar,
}

impl BackChannel {
    pub fn new() -> Arc<BackChannel> {
        Arc::new(BackChannel {
            state: Mutex::new((VecDeque::new(), false)),
            cond: Condvar::new(),
        })
    }

    /// `cupsBackChannelWrite(buffer, bytes, timeout)`: all the bytes, or -1 (never: the pipe has room).
    pub fn write(&self, buffer: &[u8]) -> isize {
        let mut s = self.state.lock().unwrap();
        s.0.extend(buffer.iter().copied());
        self.cond.notify_all();
        buffer.len() as isize
    }

    /// The write end is closed (the backend has ended): a read gets 0.
    pub fn close(&self) {
        let mut s = self.state.lock().unwrap();
        s.1 = true;
        self.cond.notify_all();
    }

    /// `cupsBackChannelRead(buffer, bytes, timeout)` (`backchannel.c:41-76`): "Wait for input ready" (a
    /// negative timeout waits for ever), "Read bytes from the pipe": what is there, up to `buffer.len()`;
    /// -1 on a timeout; 0 at the end of the pipe.
    pub fn read(&self, buffer: &mut [u8], timeout: f64) -> isize {
        let mut s = self.state.lock().unwrap();
        let deadline = if timeout < 0.0 {
            None
        } else {
            Some(Instant::now() + Duration::from_secs_f64(timeout))
        };
        while s.0.is_empty() && !s.1 {
            match deadline {
                None => s = self.cond.wait(s).unwrap(),
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        return -1; // Timeout!
                    }
                    s = self.cond.wait_timeout(s, d - now).unwrap().0;
                }
            }
        }
        if s.0.is_empty() {
            return 0; // select() says readable at the end of the pipe, read() gives 0
        }
        let n = buffer.len().min(s.0.len());
        for b in buffer.iter_mut().take(n) {
            *b = s.0.pop_front().unwrap();
        }
        n as isize
    }
}

/// The pipe between the filter's stdout and the backend's stdin (ours, W5): a byte queue of the size of
/// Linux's pipe (64 KiB), blocking the writer when it is full and the reader when it is empty.
pub struct Pipe {
    state: Mutex<PipeState>,
    cond: Condvar,
}

struct PipeState {
    data: VecDeque<u8>,
    writer_closed: bool,
    reader_gone: bool,
}

const PIPE_SIZE: usize = 65536;

impl Pipe {
    pub fn new() -> Arc<Pipe> {
        Arc::new(Pipe {
            state: Mutex::new(PipeState {
                data: VecDeque::new(),
                writer_closed: false,
                reader_gone: false,
            }),
            cond: Condvar::new(),
        })
    }

    /// `write(1, ...)` of the filter: all of `data`, waiting while the pipe is full; `Err(EPIPE)` when the
    /// backend has gone, or when `cancelled` says the filter is being ended.
    pub fn write(&self, data: &[u8], cancelled: &mut dyn FnMut() -> bool) -> Result<(), i32> {
        let mut rest = data;
        let mut s = self.state.lock().unwrap();
        while !rest.is_empty() {
            if s.reader_gone {
                return Err(32);
            }
            let room = PIPE_SIZE - s.data.len();
            if room == 0 {
                if cancelled() {
                    return Err(32);
                }
                s = self
                    .cond
                    .wait_timeout(s, Duration::from_millis(50))
                    .unwrap()
                    .0;
                continue;
            }
            let n = room.min(rest.len());
            s.data.extend(rest[..n].iter().copied());
            rest = &rest[n..];
            self.cond.notify_all();
        }
        Ok(())
    }

    /// `read(print_fd, print_buffer, 8192)` of the backend (`usb-libusb.c:327`): what there is, up to 8192
    /// bytes; none at the end of the input.
    pub fn read(&self) -> Option<Vec<u8>> {
        let mut s = self.state.lock().unwrap();
        loop {
            if !s.data.is_empty() {
                let n = s.data.len().min(8192);
                let out: Vec<u8> = s.data.drain(..n).collect();
                self.cond.notify_all();
                return Some(out);
            }
            if s.writer_closed {
                return None;
            }
            s = self.cond.wait(s).unwrap();
        }
    }

    /// The filter has ended: the backend reads the rest and then the end of its input.
    pub fn close_writer(&self) {
        self.state.lock().unwrap().writer_closed = true;
        self.cond.notify_all();
    }

    /// The backend has ended: the filter's writes fail.
    pub fn close_reader(&self) {
        self.state.lock().unwrap().reader_gone = true;
        self.cond.notify_all();
    }
}

/// `print_device` (`usb-libusb.c:169-687`) as one run, on a thread of its own beside the filter (W5): "Connect
/// to the printer" (the wait of :206-211), the read thread, the read loop over `pipe`, the end. `stop` is
/// ours (W6): SIGTERM while it still waits for the printer ends it; once it prints it drains the pipe (the
/// reference ignores SIGTERM then, :232-244). Returns the exit status, or `None` when it ended waiting.
pub fn print_device(
    host: &dyn UsbHost,
    manufacturer: &str,
    model: &str,
    pipe: &Pipe,
    back_channel: Arc<BackChannel>,
    stop: &AtomicBool,
    waiting: &mut dyn FnMut(),
    found: &mut dyn FnMut(),
) -> Option<i32> {
    let printer = wait_for_printer(
        host,
        manufacturer,
        model,
        &mut || stop.load(Ordering::SeqCst),
        waiting,
    );
    let Some(printer) = printer else {
        pipe.close_reader();
        return None;
    };
    found();
    let mut backend = Backend::start(printer, back_channel);
    while let Some(chunk) = pipe.read() {
        if backend.write(&chunk).is_err() {
            break;
        }
    }
    pipe.close_reader();
    Some(backend.finish())
}

/// `print_device`'s state (`usb_globals_t g`, :72-103).
pub struct Backend {
    printer: Arc<dyn UsbPrinterIo>,
    back_channel: Arc<BackChannel>,
    read_thread: Option<std::thread::JoinHandle<()>>,
    read_thread_stop: Arc<AtomicBool>,
    read_thread_done: Arc<(Mutex<bool>, Condvar)>,
    have_backchannel: bool,
    total_bytes: u64,
    /// `status` (`CUPS_BACKEND_OK` or `CUPS_BACKEND_FAILED`).
    pub status: i32,
}

impl Backend {
    /// `print_device` up to its read loop (:169-346): "Get the read thread going...".
    pub fn start(printer: Arc<dyn UsbPrinterIo>, back_channel: Arc<BackChannel>) -> Backend {
        let read_thread_stop = Arc::new(AtomicBool::new(false));
        let read_thread_done = Arc::new((Mutex::new(false), Condvar::new()));
        let mut read_thread = None;
        let mut have_backchannel = false;

        if printer.has_read_endpoint() {
            have_backchannel = true;
            let (p, bc, stop, done) = (
                printer.clone(),
                back_channel.clone(),
                read_thread_stop.clone(),
                read_thread_done.clone(),
            );
            read_thread = Some(std::thread::spawn(move || {
                read_thread_fn(&*p, &bc, &stop, &done)
            }));
        } else {
            stderr::write("DEBUG: Uni-directional device/mode, back channel deactivated.\n");
        }

        // "The main thread sends the print file..."
        stderr::write("INFO: Sending data to printer.\n");

        Backend {
            printer,
            back_channel,
            read_thread,
            read_thread_stop,
            read_thread_done,
            have_backchannel,
            total_bytes: 0,
            status: CUPS_BACKEND_OK,
        }
    }

    /// The body of the read loop (:340-444) for one read of the filter's pipe: "Check if we have print data
    /// ready" (up to `print_buffer`'s 8192 bytes at a time) and the `libusb_bulk_transfer` loop.
    /// `Err(errno)` once the backend has failed (the pipe is gone: `EPIPE`).
    pub fn write(&mut self, data: &[u8]) -> Result<(), i32> {
        if self.status != CUPS_BACKEND_OK {
            return Err(32); // EPIPE
        }
        for chunk in data.chunks(8192) {
            let mut print_bytes = chunk.len();
            let mut print_ptr = 0usize;
            stderr::write(&format!(
                "DEBUG: Read {} bytes of print data...\n",
                print_bytes
            ));

            while print_bytes > 0 {
                let mut iostatus = self
                    .printer
                    .bulk_write(&chunk[print_ptr..print_ptr + print_bytes]);

                // "Ignore timeout errors, but retain the number of bytes written to avoid sending
                // duplicate data..."
                if iostatus == Err(UsbError::Timeout) {
                    stderr::write("DEBUG: Got USB transaction timeout during write.\n");
                    iostatus = Ok(0);
                }
                // "If we've stalled, retry the write..."
                else if iostatus == Err(UsbError::Pipe) {
                    stderr::write("DEBUG: Got USB pipe stalled during write.\n");
                    iostatus = self
                        .printer
                        .bulk_write(&chunk[print_ptr..print_ptr + print_bytes]);
                }
                // "Retry a write after an aborted write since we probably just got SIGTERM..."
                else if iostatus == Err(UsbError::Interrupted) {
                    stderr::write("DEBUG: Got USB return aborted during write.\n");
                    iostatus = self
                        .printer
                        .bulk_write(&chunk[print_ptr..print_ptr + print_bytes]);
                }

                match iostatus {
                    Err(e) => {
                        // "Write error - bail if we don't see an error we can retry..."
                        stderr::write("ERROR: Unable to send data to printer.\n");
                        stderr::write(&format!(
                            "DEBUG: libusb write operation returned {:x}.\n",
                            e.code()
                        ));
                        self.status = CUPS_BACKEND_FAILED;
                        return Err(32);
                    }
                    Ok(bytes) if bytes > 0 => {
                        stderr::write(&format!("DEBUG: Wrote {} bytes of print data...\n", bytes));
                        print_bytes -= bytes;
                        print_ptr += bytes;
                        self.total_bytes += bytes as u64;
                    }
                    Ok(_) => {}
                }
            }
        }
        Ok(())
    }

    /// `print_device` after the loop (:441-685): "Signal the read thread to exit then wait 7 seconds for it
    /// to complete...", "Close the connection...". Returns its exit status.
    pub fn finish(mut self) -> i32 {
        stderr::write(&format!("DEBUG: Sent {} bytes...\n", self.total_bytes));

        if self.have_backchannel {
            self.read_thread_stop.store(true, Ordering::SeqCst);

            let (lock, cond) = &*self.read_thread_done;
            let mut done = lock.lock().unwrap();
            if !*done {
                stderr::write("DEBUG: Waiting for read thread to exit...\n");

                let (d, timeout) = cond
                    .wait_timeout_while(done, Duration::from_secs(WAIT_EOF_DELAY), |d| !*d)
                    .unwrap();
                done = d;

                // "If it didn't exit abort the pending read and wait an additional second..."
                if timeout.timed_out() && !*done {
                    stderr::write(
                        "DEBUG: Read thread still active, aborting the pending read...\n",
                    );
                    self.printer.close();
                    let (d, _) = cond
                        .wait_timeout_while(done, Duration::from_secs(1), |d| !*d)
                        .unwrap();
                    done = d;
                }
            }
            drop(done);
        }

        // "Close the connection and input file and general clean up..."
        self.printer.close();
        if let Some(t) = self.read_thread.take() {
            let _ = t.join();
        }
        self.back_channel.close();
        self.status
    }
}

/// `read_thread` (`usb-libusb.c:1820-1873`): "Thread to read the backchannel data on."
fn read_thread_fn(
    printer: &dyn UsbPrinterIo,
    back_channel: &BackChannel,
    stop: &AtomicBool,
    done: &(Mutex<bool>, Condvar),
) {
    loop {
        // "Try reading from the OUT (to host) endpoint..."
        let rd = printer.bulk_read(512, 60000);
        let mut ok_bytes = false;
        match &rd {
            Ok(b) if !b.is_empty() => {
                stderr::write(&format!(
                    "DEBUG: Read {} bytes of back-channel data...\n",
                    b.len()
                ));
                back_channel.write(b);
                ok_bytes = true;
            }
            Err(UsbError::Timeout) => {
                stderr::write("DEBUG: Got USB transaction timeout during read.\n")
            }
            Err(UsbError::Pipe) => stderr::write("DEBUG: Got USB pipe stalled during read.\n"),
            Err(UsbError::Interrupted) => {
                stderr::write("DEBUG: Got USB return aborted during read.\n")
            }
            _ => {}
        }

        // "Make sure this loop executes no more than once every 250 milliseconds..."
        if !ok_bytes && (WAIT_EOF || !stop.load(Ordering::SeqCst)) {
            // (in slices, to end soon after `stop`; the 250 ms are the reference's)
            let until = Instant::now() + Duration::from_millis(250);
            while Instant::now() < until && !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        if !(WAIT_EOF || !stop.load(Ordering::SeqCst)) {
            break;
        }
    }

    // "Let the main thread know that we have completed the read thread..."
    let (lock, cond) = done;
    *lock.lock().unwrap() = true;
    cond.notify_all();
}

/// `print_device`'s wait for the printer (:206-211): `while ((g.printer = find_device(print_cb, uri)) ==
/// NULL) { INFO "Waiting for printer to become available."; sleep(5); }`. `cancelled` is ours (W6): it ends
/// the wait (the real `sleep(5)` is cut short by SIGTERM). `waiting` is called when the printer was not
/// there (W8).
pub fn wait_for_printer(
    host: &dyn UsbHost,
    manufacturer: &str,
    model: &str,
    cancelled: &mut dyn FnMut() -> bool,
    waiting: &mut dyn FnMut(),
) -> Option<Arc<dyn UsbPrinterIo>> {
    loop {
        if let Some(p) = host.find_device(manufacturer, model) {
            return Some(p);
        }
        stderr::write("INFO: Waiting for printer to become available.\n");
        waiting();
        let until = Instant::now() + Duration::from_secs(5); // sleep(5)
        while Instant::now() < until {
            if cancelled() {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if cancelled() {
            return None;
        }
    }
}

#[cfg(not(test))]
pub use nusb_host::NusbHost;

/// `find_device`, `open_device`, `close_device`, `get_device_id` over `nusb` (not built for the tests: a
/// test never opens a real USB device).
#[cfg(not(test))]
mod nusb_host {
    use super::{UsbError, UsbHost, UsbPrinterIo};
    use nusb::MaybeFuture;
    use nusb::descriptors::TransferType;
    use nusb::transfer::{Buffer, Bulk, ControlIn, ControlType, In, Out, Recipient, TransferError};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// The system's USB (`libusb_init`, `libusb_get_device_list`).
    pub struct NusbHost;

    struct NusbPrinter {
        device: nusb::Device,
        write: Mutex<nusb::Endpoint<Bulk, Out>>,
        read: Option<Mutex<nusb::Endpoint<Bulk, In>>>,
        interface: Mutex<Option<nusb::Interface>>,
        closing: AtomicBool,
    }

    /// A device-to-host control transfer as libusb sends it. Elsewhere on the device; on Windows
    /// (WinUSB) libusb sends it on an interface (`reference/libusb-1.0.29/windows_winusb.c`,
    /// `winusbx_submit_control_transfer` :2998-3004, `get_valid_interface` :2909, `check_valid_interface`
    /// :2933, `auto_claim` :613, `auto_release` :658): with an interface recipient the claimed interface
    /// `wIndex & 0xff`, otherwise the lowest claimed interface; with none, the first interface that can be
    /// claimed is claimed for the transfer and released after it. The request on the wire is the same.
    #[cfg(not(windows))]
    fn control_in(
        device: &nusb::Device,
        _claimed: &BTreeMap<u8, nusb::Interface>,
        control: ControlIn,
        timeout: Duration,
    ) -> Result<Vec<u8>, TransferError> {
        device.control_in(control, timeout).wait()
    }

    #[cfg(windows)]
    fn control_in(
        device: &nusb::Device,
        claimed: &BTreeMap<u8, nusb::Interface>,
        control: ControlIn,
        timeout: Duration,
    ) -> Result<Vec<u8>, TransferError> {
        let valid = if control.recipient == Recipient::Interface {
            claimed.get(&((control.index & 0xff) as u8))
        } else {
            claimed.values().next()
        };
        if let Some(interface) = valid {
            return interface.control_in(control, timeout).wait();
        }
        // auto_claim: "No serviceable interface was found"; released when the transfer ends (dropped).
        let mut numbers: Vec<u8> = device
            .active_configuration()
            .map(|c| c.interfaces().map(|i| i.interface_number()).collect())
            .unwrap_or_default();
        numbers.sort_unstable();
        numbers.dedup();
        for number in numbers {
            if let Ok(interface) = device.claim_interface(number).wait() {
                return interface.control_in(control, timeout).wait();
            }
        }
        // "could not auto-claim any interface" (LIBUSB_ERROR_NOT_FOUND)
        Err(TransferError::InvalidArgument)
    }

    fn map_err(e: TransferError) -> UsbError {
        match e {
            TransferError::Cancelled => UsbError::Interrupted,
            TransferError::Stall => UsbError::Pipe,
            TransferError::Disconnected => UsbError::Other(-4),
            TransferError::Fault | TransferError::Unknown(_) | TransferError::InvalidArgument => {
                UsbError::Other(-1)
            }
        }
    }

    impl UsbPrinterIo for NusbPrinter {
        fn bulk_write(&self, data: &[u8]) -> Result<usize, UsbError> {
            let mut ep = self.write.lock().unwrap();
            let mut buf = Buffer::new(data.len());
            buf.extend_from_slice(data);
            // timeout 0 is no timeout: waits in long slices until the transfer completes
            let done = ep.transfer_blocking(buf, Duration::from_secs(3600));
            match done.status {
                Ok(()) => Ok(done.actual_len),
                Err(TransferError::Cancelled)
                    if done.actual_len == 0 && !self.closing.load(Ordering::SeqCst) =>
                {
                    Err(UsbError::Timeout)
                }
                Err(e) => Err(map_err(e)),
            }
        }

        fn bulk_read(&self, max: usize, timeout_ms: u64) -> Result<Vec<u8>, UsbError> {
            let Some(read) = &self.read else {
                return Err(UsbError::Other(-1));
            };
            let mut ep = read.lock().unwrap();
            let start = std::time::Instant::now();
            loop {
                if self.closing.load(Ordering::SeqCst) {
                    return Err(UsbError::Interrupted);
                }
                // slices of 100 ms (ours) so that `close` ends the read; a slice that times out goes on
                let len = max.max(ep.max_packet_size());
                let done = ep.transfer_blocking(Buffer::new(len), Duration::from_millis(100));
                match done.status {
                    Ok(()) => return Ok(done.buffer.to_vec()),
                    Err(TransferError::Cancelled) => {
                        if start.elapsed() >= Duration::from_millis(timeout_ms) {
                            return Err(UsbError::Timeout);
                        }
                    }
                    Err(e) => return Err(map_err(e)),
                }
            }
        }

        fn has_read_endpoint(&self) -> bool {
            self.read.is_some()
        }

        fn close(&self) {
            self.closing.store(true, Ordering::SeqCst);
            // `libusb_release_interface`: dropping the interface releases it
            drop(self.interface.lock().unwrap().take());
            let _ = &self.device;
        }
    }

    /// `_cupsGet1284Values` (`options.c:540-615`): the `key:value;` pairs of a device ID.
    fn get_1284_values(device_id: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut rest = device_id;
        while !rest.is_empty() {
            rest = rest.trim_start();
            let Some(colon) = rest.find(':') else { break };
            let key = rest[..colon].trim_end().to_owned();
            rest = rest[colon + 1..].trim_start();
            if rest.is_empty() {
                break;
            }
            let semi = rest.find(';');
            let value = rest[..semi.unwrap_or(rest.len())].trim_end().to_owned();
            out.push((key, value));
            match semi {
                Some(s) => rest = &rest[s + 1..],
                None => break,
            }
        }
        out
    }

    /// `get_device_id` (:1073-1165): the IEEE-1284 device ID of the printer interface.
    fn get_device_id(device: &nusb::Device, conf: u16, iface: u8, altset: u8) -> Option<String> {
        let data = control_in(
            device,
            &BTreeMap::new(),
            ControlIn {
                control_type: ControlType::Class,
                recipient: Recipient::Interface,
                request: 0,
                value: conf,
                index: ((iface as u16) << 8) | altset as u16,
                length: 1024,
            },
            Duration::from_millis(5000),
        )
        .ok()?;
        if data.len() < 2 {
            return None;
        }
        let bufsize = 1024usize;
        // "the length ... MSB first"; "If the length is out-of-range ... re-read the length as LSB first"
        let mut length = ((data[0] as usize) << 8) | data[1] as usize;
        if length > bufsize || length < 14 {
            length = ((data[1] as usize) << 8) | data[0] as usize;
        }
        if length > bufsize {
            length = bufsize;
        }
        if length < 14 {
            return None; // "Invalid device ID, clear it!"
        }
        let length = (length - 2).min(data.len().saturating_sub(2));
        Some(String::from_utf8_lossy(&data[2..2 + length]).into_owned())
    }

    /// `make_device_uri`'s manufacturer and model (:1386-1500), without the serial number and the URI
    /// assembly.
    fn make_mfg_mdl(device_id: &str, device_info: &nusb::DeviceInfo) -> (String, String) {
        let values = get_1284_values(device_id);
        let get = |k: &str| {
            values
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.clone())
        };
        let mut mfg = get("MANUFACTURER")
            .or_else(|| get("MFG"))
            .or_else(|| device_info.manufacturer_string().map(str::to_owned));
        let mut mdl = get("MODEL")
            .or_else(|| get("MDL"))
            .or_else(|| device_info.product_string().map(str::to_owned));

        if let Some(m) = &mfg {
            if m.eq_ignore_ascii_case("Hewlett-Packard") {
                mfg = Some("HP".to_owned());
            } else if m.eq_ignore_ascii_case("Lexmark International") {
                mfg = Some("Lexmark".to_owned());
            }
        }
        let mfg = mfg.unwrap_or_else(|| "Unknown".to_owned());
        let mut mdl_s = mdl.take().unwrap_or_else(|| {
            if mfg.to_ascii_lowercase().starts_with("unknown") {
                "Printer".to_owned()
            } else {
                "Unknown Model".to_owned()
            }
        });
        let l = mfg.len();
        if mdl_s.len() > l
            && mdl_s[..l].eq_ignore_ascii_case(&mfg)
            && mdl_s.as_bytes()[l].is_ascii_whitespace()
        {
            mdl_s = mdl_s[l + 1..].trim_start().to_owned();
        }
        (mfg, mdl_s)
    }

    impl UsbHost for NusbHost {
        /// `find_device(print_cb, uri)` (:820-1043) and `open_device` (:1521-1722) for the printer whose
        /// manufacturer and model are these (`print_cb`'s match without serial number and interface).
        fn find_device(&self, manufacturer: &str, model: &str) -> Option<Arc<dyn UsbPrinterIo>> {
            let list = nusb::list_devices().wait().ok()?;
            for info in list {
                if info.vendor_id() == 0 || info.product_id() == 0 {
                    continue;
                }
                let Ok(device) = info.open().wait() else {
                    continue;
                };

                // `find_device`'s loops (:855-1000): the first configuration/interface whose best alternate
                // setting is a printer interface that `print_cb` takes
                let mut found: Option<(usize, u8, u8, u8, Option<u8>, u8, usize)> = None;
                'search: for (conf_index, config) in device.configurations().enumerate() {
                    for iface in config.interfaces() {
                        // "Some printers offer multiple interfaces..."
                        let mut protocol = 0u8;
                        let mut best: Option<(u8, u8, Option<u8>)> = None; // altset, write, read
                        let mut alt_count = 0;
                        for alt in iface.alt_settings() {
                            alt_count += 1;
                            // "Currently we only support unidirectional and bidirectional printers."
                            if alt.class() != 7
                                || alt.subclass() != 1
                                || (alt.protocol() != 1 && alt.protocol() != 2)
                                || alt.protocol() < protocol
                            {
                                continue;
                            }
                            let (mut read_endp, mut write_endp) = (None, None);
                            for ep in alt.endpoints() {
                                if ep.transfer_type() == TransferType::Bulk {
                                    if ep.address() & 0x80 != 0 {
                                        read_endp = Some(ep.address());
                                    } else {
                                        write_endp = Some(ep.address());
                                    }
                                }
                            }
                            if let Some(w) = write_endp {
                                protocol = alt.protocol();
                                best = Some((
                                    alt.alternate_setting(),
                                    w,
                                    if protocol > 1 { read_endp } else { None },
                                ));
                            }
                        }
                        let Some((altset, write_addr, read_addr)) = best else {
                            continue;
                        };
                        let iface_number = iface.interface_number();

                        // get_device_id and `print_cb`
                        let device_id =
                            get_device_id(&device, conf_index as u16, iface_number, altset)
                                .unwrap_or_default();
                        let (mfg, mdl) = make_mfg_mdl(&device_id, &info);
                        if mfg != manufacturer || mdl != model {
                            continue;
                        }
                        found = Some((
                            conf_index,
                            config.configuration_value(),
                            iface_number,
                            altset,
                            read_addr,
                            write_addr,
                            alt_count,
                        ));
                        break 'search;
                    }
                }
                let Some((
                    _conf_index,
                    config_value,
                    iface_number,
                    altset,
                    read_addr,
                    write_addr,
                    alt_count,
                )) = found
                else {
                    continue;
                };

                // open_device: "Set the desired configuration, but only if it needs changing."
                let current = device
                    .active_configuration()
                    .map(|c| c.configuration_value())
                    .unwrap_or(0);
                if config_value != current {
                    let _ = device.set_configuration(config_value).wait();
                }
                // "Get the "usblp" kernel module out of the way", "Claim interfaces as needed..."
                let Ok(interface) = device.detach_and_claim_interface(iface_number).wait() else {
                    continue;
                };
                // "Set alternate setting, but only if there is more than one option."
                if alt_count > 1 && interface.set_alt_setting(altset).wait().is_err() {
                    continue;
                }
                let Ok(write) = interface.endpoint::<Bulk, Out>(write_addr) else {
                    continue;
                };
                let read = read_addr
                    .and_then(|a| interface.endpoint::<Bulk, In>(a).ok())
                    .map(Mutex::new);
                return Some(Arc::new(NusbPrinter {
                    device,
                    write: Mutex::new(write),
                    read,
                    interface: Mutex::new(Some(interface)),
                    closing: AtomicBool::new(false),
                }));
            }
            None
        }
    }
}
