//! DYMO LabelManager tape printers (thermal-label's "D1" family): what they take, and
//! (feature `hid`) sending it over USB HID.
//!
//! Tried on a LabelManager PnP (`0922:1001`, 12 mm tape) from a Mac, 2026-11-04. The print
//! head has 64 pins at 180 dpi; the tape passes it one column at a time, the start of the
//! label first. A label is:
//!
//! - `ESC C 0` (tape type: "black on white or clear", safe for any cassette),
//!   `ESC B n` (skip n bytes of pins: narrower tape runs in the middle of the head),
//!   `ESC D n` (n bytes a column);
//! - per column `SYN` (`16`) and its n bytes: bit 7 of the first byte is the **bottom** of the
//!   label as it is read, bit 0 of the last its top (found on the printer: the other way
//!   round prints it upside down);
//! - blank columns at the end, to bring the label past the cutter.
//!
//! Sent in parts of [`CHUNK_COLUMNS`] columns, each followed by a status request whose answer
//! is read before going on, as labelle does (long labels time out otherwise). Only the
//! opcodes above are sent: thermal-label warns that unknown ones can hang the printer until
//! it is switched off and on.
//!
//! From thermal-label's protocol notes (MIT) and labelle (Apache-2.0); see
//! `docs/integrations.md`.

use crate::Label;

pub const DPI: f64 = 180.0;
/// Pins on the head of a 12 mm LabelManager.
pub const HEAD_DOTS: usize = 64;
/// Columns sent before the printer is asked how it is.
pub const CHUNK_COLUMNS: usize = 64;
/// Blank tape after a label, to bring its end past the cutter (thermal-label: 16 mm).
pub const TRAILING_MM: f64 = 16.0;

const ESC: u8 = 0x1b;
const SYN: u8 = 0x16;
/// `ESC A`: the printer answers with its status byte.
pub const STATUS_REQUEST: [u8; 2] = [ESC, b'A'];

/// A tape width the head prints on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tape {
    pub width_mm: u32,
    /// Dots across the tape.
    pub dots: usize,
    /// Bytes of pins skipped before the tape (`ESC B`).
    pub dot_tab: u8,
}

impl Tape {
    /// D1 tape: 6, 9 or 12 mm on a 12 mm head.
    pub fn d1(width_mm: u32) -> Option<Tape> {
        let (dots, dot_tab) = match width_mm {
            6 => (32, 2),
            9 => (48, 1),
            12 => (64, 0),
            _ => return None,
        };
        Some(Tape {
            width_mm,
            dots,
            dot_tab,
        })
    }

    pub fn bytes_per_column(&self) -> usize {
        self.dots / 8
    }
}

/// Columns of blank tape for so many millimetres.
pub fn columns_for_mm(mm: f64) -> usize {
    (mm / 25.4 * DPI).round() as usize
}

/// The battery's voltage, read from bytes 5 and 6 of a status answer as millivolts
/// (`0d 6b` = 3435 mV). **Not confirmed:** found by reading the answers of one PnP
/// (2026-11-04): they jitter a few millivolts, as a measured voltage does, and sit where an
/// almost empty Li-ion cell would. None when the answer is short or the value is not one.
pub fn battery_volts(raw: &[u8]) -> Option<f32> {
    let mv = u16::from_be_bytes([*raw.get(4)?, *raw.get(5)?]);
    (2500..=4500).contains(&mv).then(|| f32::from(mv) / 1000.0)
}

/// Below this (at rest) the battery is low: printing still works, but short of breath.
pub const BATTERY_LOW_V: f32 = 3.6;
/// Below this a label is not printed: the print head's current pulls a Li-ion cell this
/// low under the point where the printer starts over (and feeds tape). Printing resumes
/// above [`BATTERY_LOW_V`], so it does not flap around the line.
pub const BATTERY_PRINT_MIN_V: f32 = 3.5;

/// What the printer says it is doing: its status byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status(pub u8);

impl Status {
    pub fn cassette(self) -> bool {
        self.0 & 0x40 != 0
    }
    /// Seen while it prints.
    pub fn busy(self) -> bool {
        self.0 & 0x20 != 0
    }
    pub fn cutter_jammed(self) -> bool {
        self.0 & 0x10 != 0
    }
    pub fn error(self) -> bool {
        self.0 & 0x04 != 0
    }

    /// Why it cannot print now, if it cannot.
    pub fn problem(self) -> Option<&'static str> {
        if !self.cassette() {
            Some("no cassette")
        } else if self.cutter_jammed() {
            Some("the cutter is jammed")
        } else if self.error() {
            Some("the printer reports an error")
        } else {
            None
        }
    }
}

/// A label as the printer takes it, in parts: send each, then ask the status and read it
/// before the next. The label is centred across the tape; one higher than the tape is
/// refused.
pub fn encode(label: &Label, tape: Tape) -> Result<Vec<Vec<u8>>, String> {
    if label.height > tape.dots {
        return Err(format!(
            "the label is {} dots high; {} mm tape takes {}",
            label.height, tape.width_mm, tape.dots
        ));
    }
    let bytes = tape.bytes_per_column();
    let top = (tape.dots - label.height) / 2;
    let columns = label.length + columns_for_mm(TRAILING_MM);
    let mut parts = Vec::new();
    let mut part = vec![
        ESC,
        b'C',
        0,
        ESC,
        b'B',
        tape.dot_tab,
        ESC,
        b'D',
        bytes as u8,
    ];
    for x in 0..columns {
        part.push(SYN);
        let mut column = vec![0u8; bytes];
        for y in 0..label.height {
            if label.get(x, y) {
                // Counted from the bottom of the tape: bit 7 of the first byte.
                let pin = tape.dots - 1 - (top + y);
                column[pin / 8] |= 0x80 >> (pin % 8);
            }
        }
        part.extend_from_slice(&column);
        if (x + 1) % CHUNK_COLUMNS == 0 {
            parts.push(std::mem::take(&mut part));
        }
    }
    if !part.is_empty() {
        parts.push(part);
    }
    Ok(parts)
}

#[cfg(feature = "hid")]
pub use hid::{Printer, find, hex};

#[cfg(feature = "hid")]
mod hid {
    use super::{STATUS_REQUEST, Status, Tape, battery_volts, encode};
    use crate::Label;

    const VENDOR: u16 = 0x0922;
    /// LabelManager PnP: as it first shows (with its small USB stick), and switched to
    /// printer mode (on Linux by `usb_modeswitch`). It takes labels on its HID interface in
    /// both.
    const PRODUCTS: [u16; 2] = [0x1001, 0x1002];
    /// HID reports carry 64 bytes.
    const REPORT: usize = 64;

    /// Bytes as `40 00 12 64`.
    pub fn hex(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The time of day (UTC), for the log: `13:12:41`.
    fn clock() -> String {
        let s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        format!("{:02}:{:02}:{:02} UTC", s / 3600 % 24, s / 60 % 60, s % 60)
    }

    /// A LabelManager on USB.
    pub struct Printer {
        device: hidapi::HidDevice,
        /// Kept while the device is open, and asked whether it is still on USB.
        api: hidapi::HidApi,
        pub product: String,
        /// What it last said. An idle printer is asked only now and then (the agent decides
        /// how often): on Windows it was seen to keep feeding tape (2026-11-04) while asked
        /// every two seconds; a battery near empty, the printer starting over and over, is
        /// as likely a cause.
        pub last: Option<Status>,
        /// Its whole last answer to a status request (8 bytes): only the first is known
        /// (the status); the others are kept to find out what they say (the battery?).
        pub last_raw: Vec<u8>,
        /// The answer last written to the log.
        said_raw: Vec<u8>,
        /// When it was last asked, so that an idle printer is asked only now and then.
        pub asked_at: Option<std::time::Instant>,
    }

    /// The first LabelManager PnP on USB, opened.
    pub fn find() -> Result<Option<Printer>, String> {
        let api = hidapi::HidApi::new().map_err(|e| e.to_string())?;
        let Some(info) = api
            .device_list()
            .find(|d| d.vendor_id() == VENDOR && PRODUCTS.contains(&d.product_id()))
        else {
            return Ok(None);
        };
        let product = info
            .product_string()
            .unwrap_or("DYMO LabelManager")
            .to_owned();
        let device = info.open_device(&api).map_err(|e| e.to_string())?;
        let printer = Printer {
            device,
            api,
            product,
            last: None,
            last_raw: Vec::new(),
            said_raw: Vec::new(),
            asked_at: None,
        };
        // Nothing is sent on opening: its status is asked when it prints.
        Ok(Some(printer))
    }

    impl Printer {
        /// Whether it is still on USB (switched on, plugged in), without sending it anything.
        pub fn present(&mut self) -> bool {
            if self.api.refresh_devices().is_err() {
                return true;
            }
            self.api
                .device_list()
                .any(|d| d.vendor_id() == VENDOR && PRODUCTS.contains(&d.product_id()))
        }

        fn send(&self, bytes: &[u8]) -> Result<(), String> {
            // Every message to the printer is said, so what it does can be set against it.
            let what = match bytes {
                [0x1b, b'A'] => "status request".to_owned(),
                _ => format!("{} bytes of a label", bytes.len()),
            };
            println!("{} to the DYMO: {what}", clock());
            for part in bytes.chunks(REPORT) {
                // Report id 0, then the bytes, the rest of the report zero.
                let mut report = [0u8; REPORT + 1];
                report[1..1 + part.len()].copy_from_slice(part);
                self.device.write(&report).map_err(|e| e.to_string())?;
            }
            Ok(())
        }

        /// Asks its status; keeps the whole answer, and says it when it differs from the last.
        pub fn status(&mut self) -> Result<Status, String> {
            self.send(&STATUS_REQUEST)?;
            self.asked_at = Some(std::time::Instant::now());
            let mut answer = [0u8; REPORT];
            match self.device.read_timeout(&mut answer, 2000) {
                Ok(n) if n > 0 => {
                    let raw = answer[..n.min(8)].to_vec();
                    // Said when it changes, but not for the battery's few millivolts of jitter.
                    let others = |r: &[u8]| {
                        r.iter()
                            .enumerate()
                            .filter(|(i, _)| *i != 4 && *i != 5)
                            .map(|(_, b)| *b)
                            .collect::<Vec<_>>()
                    };
                    let volts = |r: &[u8]| battery_volts(r).unwrap_or(0.0);
                    if others(&raw) != others(&self.said_raw)
                        || (volts(&raw) - volts(&self.said_raw)).abs() >= 0.02
                    {
                        let battery = battery_volts(&raw)
                            .map(|v| format!(" (battery {v:.2} V?)"))
                            .unwrap_or_default();
                        println!("{} the DYMO answers: {}{battery}", clock(), hex(&raw));
                        self.said_raw = raw.clone();
                    }
                    self.last_raw = raw;
                    let status = Status(answer[0]);
                    self.last = Some(status);
                    Ok(status)
                }
                Ok(_) => Err("the printer does not answer".to_owned()),
                Err(e) => Err(e.to_string()),
            }
        }

        /// Prints a label; `progress` hears the parts sent of all of them.
        pub fn print(
            &mut self,
            label: &Label,
            tape: Tape,
            mut progress: impl FnMut(usize, usize),
        ) -> Result<(), String> {
            let before = self.status()?;
            if let Some(problem) = before.problem() {
                return Err(problem.to_owned());
            }
            let parts = encode(label, tape)?;
            for (i, part) in parts.iter().enumerate() {
                self.send(part)?;
                let status = self.status()?;
                if let Some(problem) = status.problem() {
                    return Err(problem.to_owned());
                }
                progress(i + 1, parts.len());
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_is_framed_and_the_right_way_up() {
        // One column: its top dot and its bottom dot.
        let mut label = Label::blank(1, 64);
        label.set(0, 0, true);
        label.set(0, 63, true);
        let tape = Tape::d1(12).unwrap();
        let parts = encode(&label, tape).unwrap();
        let first = &parts[0];
        assert_eq!(&first[..9], &[0x1b, b'C', 0, 0x1b, b'B', 0, 0x1b, b'D', 8]);
        // The bottom of the label is bit 7 of the first byte, its top bit 0 of the last.
        assert_eq!(&first[9..18], &[0x16, 0x80, 0, 0, 0, 0, 0, 0, 0x01]);
        // Then blank columns past the cutter, in parts of 64 columns.
        let columns: usize = parts.iter().flatten().filter(|&&b| b == 0x16).count();
        assert_eq!(columns, 1 + columns_for_mm(TRAILING_MM));
        assert_eq!(columns_for_mm(TRAILING_MM), 113);
        assert_eq!(parts.len(), columns.div_ceil(CHUNK_COLUMNS));
    }

    #[test]
    fn narrow_tape_is_centred_and_too_high_a_label_refused() {
        let tape = Tape::d1(6).unwrap();
        assert_eq!(
            (tape.dots, tape.dot_tab, tape.bytes_per_column()),
            (32, 2, 4)
        );
        let mut label = Label::blank(1, 30);
        label.set(0, 0, true);
        let parts = encode(&label, tape).unwrap();
        // 30 dots on 32: one blank dot above, so its top is the tape's second dot from the top.
        assert_eq!(&parts[0][6..9], &[0x1b, b'D', 4]);
        assert_eq!(&parts[0][9..14], &[0x16, 0, 0, 0, 0x02]);
        assert!(encode(&Label::blank(1, 40), tape).is_err());
    }

    #[test]
    fn the_battery_is_read_from_the_answer() {
        // As a PnP answered (2026-11-04): 3.435 V, a cell nearly empty.
        let v = battery_volts(&[0x00, 0x00, 0x02, 0x64, 0x0d, 0x6b, 0x00, 0x00]).unwrap();
        assert!((v - 3.435).abs() < 0.0005);
        assert!(v < BATTERY_PRINT_MIN_V);
        assert_eq!(battery_volts(&[0x40, 0x00, 0x12]), None, "too short");
        assert_eq!(
            battery_volts(&[0, 0, 0, 0, 0, 0, 0, 0]),
            None,
            "not a voltage"
        );
    }

    #[test]
    fn the_status_says_what_is_wrong() {
        assert_eq!(Status(0x40).problem(), None);
        assert!(Status(0x60).busy());
        assert_eq!(Status(0x00).problem(), Some("no cassette"));
        assert_eq!(Status(0x50).problem(), Some("the cutter is jammed"));
    }
}
