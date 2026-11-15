// Ported from DYMO's CUPS driver 1.4.0 (reference/dymo-cups-drivers-1.4.0):
//   src/lw/LabelWriterLanguageMonitor.h, src/lw/LabelWriterLanguageMonitor.cpp
// DYMO LabelWriter Drivers, Copyright (C) 2008 Sanford L.P.
// This program is free software; you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation; either version 2 of the License,
// or (at your option) any later version (taken as version 3, so it combines with this program's AGPL-3.0).
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even
// the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.

//! `LabelWriterLanguageMonitor.cpp`: "LanguageMonitor for LabelWriter printers. Supported features are:
//! reprinting last page on end of roll". It asks the printer for its status (`ESC A`) and reads the answer
//! on the back channel; only inside a print (`StartPage`, `EndPage`).

use super::label_writer_driver::{get_roll_select_command, LabelWriterDriver, PaperType, Roll};
use super::printer_driver::{Buffer, JobStatus, LanguageMonitor, SharedEnvironment};
use super::stderr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `LabelWriterLanguageMonitor.h:38-45` `status_bits`.
pub const TOF_BIT: u8 = 0x02;
pub const ROLL_CHANGED_BIT: u8 = 0x08;
pub const PAPER_OUT_BIT: u8 = 0x20;
pub const PAPER_FEED_BIT: u8 = 0x40;
pub const ERROR_BIT: u8 = 0x80;

/// `CLabelWriterLanguageMonitor`.
pub struct LabelWriterLanguageMonitor {
    environment: SharedEnvironment,
    paper_type: PaperType,
    roll: Roll,
    roll_used: bool,
    is_first_page: bool,
    page_data: Buffer,
    use_sleep: bool,
    last_status: u8,
    #[allow(dead_code)] // :103 LastReadStatusResult_, initialised and never read (its uses are commented out)
    last_read_status_result: bool,
    read_status_timeout: usize,
    /// Ours: what `getenv("DEVICE_URI")` gives (:84); see [`super::cups_print_environment::FilterIo::getenv`].
    device_uri: Option<String>,
    /// Ours (W7): the first byte of the last status answer, for the telemetry.
    pub last_raw_status: Option<u8>,
}

/// `time(NULL)`.
fn time_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `ctime(&t)`: "Sun Nov 15 17:15:00 2026\n". Ours: in UTC (the C library uses the local zone), the text
/// goes only to the `DEBUG:` line.
fn ctime(t: u64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let days = (t / 86400) as i64;
    let secs = t % 86400;
    let (mut y, mut d) = (1970i64, days);
    loop {
        let len = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 366 } else { 365 };
        if d < len {
            break;
        }
        d -= len;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let ml = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0;
    while d >= ml[m] {
        d -= ml[m];
        m += 1;
    }
    format!(
        "{} {} {:2} {:02}:{:02}:{:02} {}\n",
        DAYS[((days + 4) % 7) as usize],
        MONTHS[m],
        d + 1,
        secs / 3600,
        secs % 3600 / 60,
        secs % 60,
        y
    )
}

impl LabelWriterLanguageMonitor {
    /// `CLabelWriterLanguageMonitor::CLabelWriterLanguageMonitor(Environment, UseSleep = true,
    /// ReadStatusTimeout = 10)` (.h:47, .cpp:34-37). `device_uri` is ours (see the field).
    pub fn new(environment: SharedEnvironment, use_sleep: bool, read_status_timeout: usize, device_uri: Option<String>) -> Self {
        LabelWriterLanguageMonitor {
            environment,
            paper_type: PaperType::Regular,
            roll: Roll::Auto,
            roll_used: false,
            is_first_page: true,
            page_data: Buffer::new(),
            use_sleep,
            last_status: 0,
            last_read_status_result: true,
            read_status_timeout,
            device_uri,
            last_raw_status: None,
        }
    }

    /// `SetPaperType` (:294-298).
    pub fn set_paper_type(&mut self, value: PaperType) {
        self.paper_type = value;
    }

    /// `SetRoll` (:300-305).
    pub fn set_roll(&mut self, value: Roll) {
        self.roll = value;
        self.roll_used = true;
    }

    /// `IsLocal` (:79-90): "check if printer is locally connected, i.e. to usb port".
    fn is_local(&self) -> bool {
        let mut b_is_local = true;
        if let Some(uri) = &self.device_uri {
            b_is_local = uri.as_bytes().starts_with(b"usb://");
        }
        b_is_local
    }

    /// `SynchronizeRoll` (:92-97).
    fn synchronize_roll(&mut self) {
        let buf = get_roll_select_command(self.roll);
        self.environment.borrow_mut().write_data(&buf);
    }

    /// `ResetPrinter` (:99-104).
    fn reset_printer(&mut self) {
        let buf = LabelWriterDriver::get_reset_command();
        self.environment.borrow_mut().write_data(&buf);
    }

    /// `CheckStatusAndReprint` (:107-175): "checks status and reprint label if needed".
    fn check_status_and_reprint(&mut self) {
        stderr::write("DEBUG: CLabelWriterLanguageMonitor::CheckStatusAndReprint()\n");

        // restore good status of the job
        self.set_job_status(TOF_BIT);

        if !self.is_local() {
            return;
        }

        loop {
            // reprint also can fail, so don't forget to recheck status after reprint
            stderr::write("DEBUG: CLabelWriterLanguageMonitor::CheckStatusAndReprint() 1\n");

            let mut status: u8 = 0;
            let begin_time = time_now();
            let mut _status_ok = self.read_status(&mut status);

            // request status while good or bad condition or timeout
            let mut i = 0;
            while (status & TOF_BIT) == 0
                && (status & ERROR_BIT) == 0
                && (status & ROLL_CHANGED_BIT) == 0
                && (time_now().saturating_sub(begin_time) as f64) < self.read_status_timeout as f64
            {
                stderr::write(&format!("DEBUG: CLabelWriterLanguageMonitor::CheckStatusAndReprint() 2 {}\n", i));
                _status_ok = self.read_status(&mut status);
                i += 1;
            }

            if time_now().saturating_sub(begin_time) as f64 >= self.read_status_timeout as f64 {
                stderr::write("DEBUG: CLabelWriterLanguageMonitor::CheckStatusAndReprint() timeout\n");
                break;
            }

            // error - needs reprint
            if (status & ERROR_BIT) != 0 || (status & ROLL_CHANGED_BIT) != 0 || (status & TOF_BIT) == 0 {
                // force error bit in case of timeout
                if (status & TOF_BIT) == 0 && (status & ROLL_CHANGED_BIT) == 0 {
                    status |= ERROR_BIT;
                }

                self.set_job_status(status);
                if self.poll_until_paper_in() {
                    // restore good status of the job
                    self.set_job_status(TOF_BIT);

                    self.reprint_label();
                }
            } else {
                break;
            }
        }

        // clear stored label data
        self.page_data.clear();

        stderr::write("DEBUG: CLabelWriterLanguageMonitor::CheckStatusAndReprint() return\n");
    }

    /// `ReadStatus` (:177-227): "Read status byte from the printer; return true if status has been read".
    fn read_status(&mut self, status: &mut u8) -> bool {
        let t = time_now();
        stderr::write(&format!("DEBUG: CLabelWriterLanguageMonitor::ReadStatus() {}\n", ctime(t)));

        let mut result = false;
        *status = 0; // default
        self.last_status = 0;

        {
            let request_status_command = LabelWriterDriver::get_request_status_command();
            self.environment.borrow_mut().write_data(&request_status_command);
        }

        let buf = self.environment.borrow_mut().read_data();

        if !buf.is_empty() {
            *status = buf[0];

            if self.paper_type == PaperType::Continuous {
                *status |= TOF_BIT;
            }

            result = true;
        }

        self.last_status = *status;
        self.last_raw_status = buf.first().copied();

        stderr::write(&format!("DEBUG: ReadStatus() returned {:x} {}\n", status, result as i32));
        result
    }

    /// `PollUntilPaperIn` (:229-258): "request status while the status become OK; return true on success".
    fn poll_until_paper_in(&mut self) -> bool {
        stderr::write("DEBUG: CLabelWriterLanguageMonitor::PollUntilPaperIn()\n");

        let mut status: u8 = 0;
        loop {
            // TODO: use platform-undependend call
            if self.use_sleep {
                std::thread::sleep(Duration::from_nanos(200_000_000)); // 0.2 second
            }

            if self.environment.borrow_mut().get_job_status() == JobStatus::Deleted {
                return false;
            }

            if !self.read_status(&mut status) {
                return false;
            }

            self.set_job_status(status); // update status

            if (status & TOF_BIT) != 0 && (status & ERROR_BIT) == 0 {
                return true;
            }
        }
    }

    /// `SetJobStatus` (:260-271): "update job status based on status read from the printer".
    fn set_job_status(&mut self, status: u8) {
        let mut job_status = JobStatus::Ok;

        if (status & PAPER_OUT_BIT) != 0 || (status & PAPER_FEED_BIT) != 0 {
            job_status = JobStatus::PaperOut;
        } else if (status & ERROR_BIT) != 0 {
            job_status = JobStatus::Error;
        }

        self.environment.borrow_mut().set_job_status(job_status);
    }

    /// `ReprintLabel` (:273-286): "reprint cached label".
    fn reprint_label(&mut self) {
        stderr::write("DEBUG: CLabelWriterLanguageMonitor::ReprintLabel()\n");

        // send form feed first
        if (self.last_status & ROLL_CHANGED_BIT) == 0 {
            let short_form_feed_command = LabelWriterDriver::get_short_form_feed_command();
            self.environment.borrow_mut().write_data(&short_form_feed_command);
        }

        let page_data = self.page_data.clone();
        self.environment.borrow_mut().write_data(&page_data);
    }
}

impl LanguageMonitor for LabelWriterLanguageMonitor {
    /// `StartDoc` (:43-51).
    fn start_doc(&mut self) {
        self.is_first_page = true;
        self.reset_printer();

        if self.roll_used {
            self.synchronize_roll();
        }
    }

    /// `EndDoc` (:53-58).
    fn end_doc(&mut self) {
        stderr::write("DEBUG: CLabelWriterLanguageMonitor::EndDoc()\n");
    }

    /// `StartPage` (:60-70).
    fn start_page(&mut self) {
        stderr::write("DEBUG: CLabelWriterLanguageMonitor::StartPage()\n");
        if self.is_first_page {
            self.check_status_and_reprint();
        }

        self.is_first_page = false;
    }

    /// `EndPage` (:72-77).
    fn end_page(&mut self) {
        stderr::write("DEBUG: CLabelWriterLanguageMonitor::EndPage()\n");
        self.check_status_and_reprint();
    }

    /// `ProcessData` (:288-292).
    fn process_data(&mut self, data: &[u8]) {
        self.page_data.extend_from_slice(data);
    }
}
