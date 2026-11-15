// Ported from DYMO's CUPS driver 1.4.0 (reference/dymo-cups-drivers-1.4.0):
//   src/common/CupsPrintEnvironment.h, src/common/CupsPrintEnvironment.cpp
// DYMO LabelWriter Drivers, Copyright (C) 2008 Sanford L.P.
// This program is free software; you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation; either version 2 of the License,
// or (at your option) any later version (taken as version 3, so it combines with this program's AGPL-3.0).
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even
// the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.

//! `CupsPrintEnvironment.cpp`: the two environments of the CUPS filter. The driver writes through
//! [`CupsPrintEnvironmentForDriver`], which writes to the filter's stdout and passes a copy to the language
//! monitor; the monitor writes and reads through [`CupsPrintEnvironmentForLm`].
//!
//! `write(1, ...)` and `cupsBackChannelRead` are CUPS's: the filter's stdout is the pipe to the `usb`
//! backend and fd 3 its back channel. Here they are [`FilterIo`] (ours, W5): [`super::cups::usb_backend`]
//! is the backend's side of both.

use super::printer_driver::{Buffer, JobStatus, PrintEnvironment, SharedMonitor};
use super::stderr;
use std::cell::RefCell;
use std::rc::Rc;

/// What the filter's process has of CUPS: `write(1, ...)`, `cupsBackChannelRead` and `getenv` (ours).
pub trait FilterIo {
    /// `write(1, buf, len)`: `Err(errno)` for a result of -1.
    fn write_stdout(&mut self, data: &[u8]) -> Result<(), i32>;
    /// `cupsBackChannelRead(buffer, bytes, timeout)` (`cups/backchannel.c:41-76`): the bytes read, `-1` on a
    /// timeout or error (what the C function returns), `0` when the channel is closed.
    fn back_channel_read(&mut self, buffer: &mut [u8], timeout: f64) -> isize;
    /// `getenv(name)`.
    fn getenv(&self, name: &str) -> Option<String>;
    /// `ppdOpenFile(getenv("PPD"))` (`CupsFilter.h:218`, `raster2dymolw.cpp:60`): the PPD of the queue, here
    /// the plain 450's (ours, W4). A new, unmarked one each time, as the C++ opens it each time.
    fn ppd_open_file(&self) -> Option<super::cups::ppd::PpdFile>;
    /// Ours (W6): the job was stopped; the next call into the environment ends the filter, as CUPS ends it
    /// with SIGTERM when a job is cancelled.
    fn cancelled(&mut self) -> bool;
}

/// The debug line `ReadData` writes when it has read from the back channel (:149), up to the count: the port
/// reads the status byte of the telemetry from it (W11); a test fails if this text changes.
pub const READ_DATA_LINE: &str = "DEBUG: CCupsPrintEnvironmentForLM::ReadData() has read ";

/// A shared [`FilterIo`].
pub type SharedIo = Rc<RefCell<dyn FilterIo>>;

/// Ours (W6): the unwind that ends the filter when its job is stopped (SIGTERM).
#[derive(Debug)]
pub struct Cancelled;

fn check_cancel(io: &SharedIo) {
    if io.borrow_mut().cancelled() {
        std::panic::resume_unwind(Box::new(Cancelled));
    }
}

/// `CCupsPrintEnvironmentForDriver` (.h:31-44): "this env will be forward output to LM also".
pub struct CupsPrintEnvironmentForDriver {
    io: SharedIo,
    prn_file: Option<std::fs::File>,
    language_monitor: SharedMonitor,
}

impl CupsPrintEnvironmentForDriver {
    /// :31-45. `DYMO_PRN_DIR` is read as the driver reads it; the job's printer name `PRINTER` too.
    pub fn new(io: SharedIo, language_monitor: SharedMonitor) -> Self {
        let mut prn_file = None;
        let prn_dir = io.borrow().getenv("DYMO_PRN_DIR");
        if let Some(prn_dir) = prn_dir {
            let mut file_name = prn_dir;
            match io.borrow().getenv("PRINTER") {
                Some(p) => file_name += &p,
                None => file_name += "~dymo",
            }
            file_name += ".prn";
            prn_file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(file_name).ok();
        }
        CupsPrintEnvironmentForDriver { io, prn_file, language_monitor }
    }
}

impl PrintEnvironment for CupsPrintEnvironmentForDriver {
    /// `WriteData` (:54-76).
    fn write_data(&mut self, data_buffer: &[u8]) {
        check_cancel(&self.io);
        stderr::write(&format!("DEBUG: CCupsPrintEnvironmentForDriver::WriteData() buffer size is {}\n", data_buffer.len()));

        if !data_buffer.is_empty() {
            if let Err(errno) = self.io.borrow_mut().write_stdout(data_buffer) {
                let _ = errno;
                stderr::write(&format!("ERROR: CCupsPrintEnvironmentForDriver::WriteData() write() failed, errno={}\n", errno));
            }

            if let Some(f) = self.prn_file.as_mut() {
                use std::io::Write;
                let res = f.write_all(data_buffer).map(|_| data_buffer.len()).unwrap_or(0);
                stderr::write(&format!("DEBUG: CCupsPrintEnvironmentForDriver::WriteData() PRN fwrite result is {}\n", res));
            }

            self.language_monitor.borrow_mut().process_data(data_buffer);
        }
    }

    /// `ReadData` (:78-84): "do nothing - driver is not able to read data, only LM is".
    fn read_data(&mut self) -> Buffer {
        Buffer::new()
    }

    /// `GetJobStatus` (:86-90).
    fn get_job_status(&mut self) -> JobStatus {
        JobStatus::Ok
    }

    /// `SetJobStatus` (:92-95).
    fn set_job_status(&mut self, _job_status: JobStatus) {}
}

/// `CCupsPrintEnvironmentForLM` (.h:48-61): "it simple output it is data to CUPS file descriptor".
pub struct CupsPrintEnvironmentForLm {
    io: SharedIo,
    /// `JobStatus_` (.h:60): never initialised in the C++ constructor (:102-104); the monitor sets it before
    /// the filter reads it (`CheckStatusAndReprint` :113). `Ok` here.
    job_status: JobStatus,
    /// Ours (W7): the last `STATE:` the environment wrote, for the telemetry.
    pub last_state: Option<&'static str>,
}

impl CupsPrintEnvironmentForLm {
    /// :102-104.
    pub fn new(io: SharedIo) -> Self {
        CupsPrintEnvironmentForLm { io, job_status: JobStatus::Ok, last_state: None }
    }
}

impl PrintEnvironment for CupsPrintEnvironmentForLm {
    /// `WriteData` (:111-125).
    fn write_data(&mut self, data_buffer: &[u8]) {
        check_cancel(&self.io);
        stderr::write(&format!("DEBUG: CCupsPrintEnvironmentForLM::WriteData() buffer size is {}\n", data_buffer.len()));
        if !data_buffer.is_empty() {
            if let Err(errno) = self.io.borrow_mut().write_stdout(data_buffer) {
                stderr::write(&format!("ERROR: CCupsPrintEnvironmentForLM::WriteData() write() failed, errno={}\n", errno));
            }
        }
    }

    /// `ReadData` (:127-151): reads what the printer sent on the back channel.
    fn read_data(&mut self) -> Buffer {
        check_cancel(&self.io);
        let mut data_buffer = Buffer::new();

        let mut buf = [0u8; 16];
        let bytes_read = self.io.borrow_mut().back_channel_read(&mut buf, 2.5);
        if bytes_read == -1 {
            // the C++ prints errno; the read has none here
            stderr::write("DEBUG: CCupsPrintEnvironmentForLM::ReadData() unable to read data, errno=0\n");
        } else if bytes_read == 0 {
            stderr::write("DEBUG: CCupsPrintEnvironmentForLM::ReadData() no data\n");
        } else {
            data_buffer.splice(0..0, buf[..bytes_read as usize].iter().copied());

            stderr::write(&format!("{}{} bytes {:x}\n", READ_DATA_LINE, bytes_read, data_buffer[0]));
        }
        data_buffer
    }

    /// `GetJobStatus` (:153-157). Ours (W6): after a stop, `jsDeleted`.
    fn get_job_status(&mut self) -> JobStatus {
        if self.io.borrow_mut().cancelled() {
            return JobStatus::Deleted;
        }
        self.job_status
    }

    /// `SetJobStatus` (:159-194).
    fn set_job_status(&mut self, job_status: JobStatus) {
        self.job_status = job_status;

        let (text, state) = match job_status {
            JobStatus::Ok => ("STATE: none\n", None),
            JobStatus::PaperOut => ("STATE: com.dymo.out-of-paper-error\n", Some("com.dymo.out-of-paper-error")),
            JobStatus::Error => ("STATE: com.dymo.general-error\n", Some("com.dymo.general-error")),
            JobStatus::PaperSizeError => ("STATE: com.dymo.paper-size-error\n", Some("com.dymo.paper-size-error")),
            JobStatus::PaperSizeUndefinedError => ("STATE: com.dymo.paper-size-undefine-error\n", Some("com.dymo.paper-size-undefine-error")),
            JobStatus::HeadOverheat => ("STATE: com.dymo.head-overheat-error\n", Some("com.dymo.head-overheat-error")),
            JobStatus::SlotStatusError => ("STATE: com.dymo.slot-status-error\n", Some("com.dymo.slot-status-error")),
            JobStatus::Busy => ("STATE: com.dymo.busy-error\n", Some("com.dymo.busy-error")),
            JobStatus::Deleted => unreachable!("assert(0): the switch has no case for jsDeleted"),
        };
        self.last_state = state;
        stderr::write(text);
    }
}
