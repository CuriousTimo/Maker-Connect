// Ported from DYMO's CUPS driver 1.4.0 (reference/dymo-cups-drivers-1.4.0):
//   src/common/CommonTypedefs.h, src/common/PrinterDriver.h
// DYMO LabelWriter Drivers, Copyright (C) 2008 Sanford L.P.
// This program is free software; you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation; either version 2 of the License,
// or (at your option) any later version (taken as version 3, so it combines with this program's AGPL-3.0).
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even
// the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.

//! `CommonTypedefs.h` and `PrinterDriver.h`: the driver's types and the three interfaces between the
//! driver, its language monitor and the environment.

use std::cell::RefCell;
use std::rc::Rc;

/// `CommonTypedefs.h:31` `typedef unsigned char byte`.
pub type Byte = u8;
/// `CommonTypedefs.h:33` `typedef std::vector<byte> buffer_t`.
pub type Buffer = Vec<u8>;

/// `PrinterDriver.h:34-46` `IPrinterDriver`: "Generic interface to the driver."
pub trait PrinterDriver {
    fn start_doc(&mut self);
    fn end_doc(&mut self);
    fn start_page(&mut self);
    fn end_page(&mut self);
    fn process_raster_line(&mut self, line_buffer: &[u8]);
}

/// `PrinterDriver.h:54-65` `IPrintEnvironment::job_status_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Ok,
    PaperOut,
    Error,
    Deleted,
    PaperSizeError,
    PaperSizeUndefinedError,
    HeadOverheat,
    SlotStatusError,
    Busy,
}

/// `PrinterDriver.h:51-74` `IPrintEnvironment`: "the environment could be used by IPrinterDriver".
pub trait PrintEnvironment {
    fn write_data(&mut self, data_buffer: &[u8]);
    /// `ReadData(buffer_t& DataBuffer)`: the buffer is the result.
    fn read_data(&mut self) -> Buffer;
    fn get_job_status(&mut self) -> JobStatus;
    fn set_job_status(&mut self, job_status: JobStatus);
}

/// `PrinterDriver.h:80-92` `ILanguageMonitor`: "Interface to monitor sending printer data".
pub trait LanguageMonitor {
    fn start_doc(&mut self);
    fn end_doc(&mut self);
    fn start_page(&mut self);
    fn end_page(&mut self);
    fn process_data(&mut self, data: &[u8]);
}

/// A shared environment: the C++ objects hold references to each other (`CupsFilter.h:60-63`).
pub type SharedEnvironment = Rc<RefCell<dyn PrintEnvironment>>;
/// A shared language monitor.
pub type SharedMonitor = Rc<RefCell<dyn LanguageMonitor>>;
