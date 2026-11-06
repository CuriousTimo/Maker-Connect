//! DYMO LabelWriter 400 and 450 label printers (not the 550, which speaks another protocol
//! and takes only DYMO's labels with a chip), and (feature `usb`) sending to one over USB.
//!
//! The head has 672 dots at 300 dpi (56 mm) across the label; the label passes it one line
//! at a time, the start of the label first. Labels are die-cut on a roll: the printer finds
//! the gap to the next label itself (its top of form). A label is:
//!
//! - [`RESET_ESCAPES`] × `ESC`: brings the printer out of a half-sent line (a line has no
//!   frame, so after a cut-off job it may still be waiting for pixels), as DYMO's own driver
//!   starts every job; then `ESC @` (reset: top of form, defaults);
//! - `ESC h` (300 × 300 dpi), `ESC e` (normal density),
//! - `ESC L` and the label's length in dots, **most significant byte first** (DYMO's driver;
//!   thermal-label's code sends it the other way round, its own notes say this way);
//!   `FF FF` for a continuous roll;
//! - `ESC D` and the bytes a line (dot 0 at bit 7 of the first byte);
//! - per line `SYN` and its bytes, or `ETB` and the line run-length coded (bit 7 the colour,
//!   bits 0 to 6 the run less one), whichever is shorter; blank lines as `ESC f 1 n`;
//! - `ESC E` (form feed: the label to the tear bar), and `ESC A` (status: one byte back).
//!
//! A label narrower than the head runs at its dot 0 side: DYMO's PPDs put every label at
//! offset 0. Which side of the label that is ([`TOP_AT_DOT_ZERO`]) is to be checked on the
//! printer: printed the wrong way, a label comes out mirrored.
//!
//! From DYMO's *LabelWriter 450 Series Technical Reference*, as thermal-label's notes (MIT)
//! cite it, and DYMO's CUPS driver (`dymo-cups-drivers`, GPL: read for the byte order, not
//! copied).

use crate::Label;

pub const DPI: f64 = 300.0;
/// Dots across the head (56 mm).
pub const HEAD_DOTS: usize = 672;
/// `ESC` bytes that end any half-sent line, more than a line can hold (DYMO's driver: 156).
pub const RESET_ESCAPES: usize = 156;
/// Whether the top of the label (as it is read, its start first) is at the head's dot 0.
/// To check on the printer: if a label comes out mirrored, this is the other way round.
pub const TOP_AT_DOT_ZERO: bool = true;

const ESC: u8 = 0x1b;
const SYN: u8 = 0x16;
const ETB: u8 = 0x17;
/// `ESC A`: the printer answers with one status byte.
pub const STATUS_REQUEST: [u8; 2] = [ESC, b'A'];

/// USB: DYMO's vendor id, and the LabelWriters that speak this protocol with a 672-dot
/// head, by product id (thermal-label's device list; the 4XL, wider, is left out).
pub const VENDOR: u16 = 0x0922;
pub const PRODUCTS: [(u16, &str); 5] = [
    (0x0019, "LabelWriter 400"),
    (0x001a, "LabelWriter 400 Turbo"),
    (0x0020, "LabelWriter 450"),
    (0x0021, "LabelWriter 450 Turbo"),
    (0x0022, "LabelWriter 450 Twin Turbo"),
];

/// Dots for a length in mm, at 300 dpi.
pub fn dots_for_mm(mm: f64) -> usize {
    (mm * DPI / 25.4).round() as usize
}

/// What the printer said of itself (`ESC A`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status(pub u8);

impl Status {
    pub fn ready(self) -> bool {
        self.0 & 0x01 != 0
    }
    pub fn top_of_form(self) -> bool {
        self.0 & 0x02 != 0
    }
    pub fn no_paper(self) -> bool {
        self.0 & 0x20 != 0
    }
    pub fn jammed(self) -> bool {
        self.0 & 0x40 != 0
    }
    pub fn error(self) -> bool {
        self.0 & 0x80 != 0
    }

    /// What keeps it from printing, in words; `None` when it can print.
    pub fn problem(self) -> Option<&'static str> {
        if self.no_paper() {
            Some("no labels: put a roll in")
        } else if self.jammed() {
            Some("the labels are jammed")
        } else if self.error() {
            Some("the printer reports an error")
        } else {
            None
        }
    }
}

/// The label for a LabelWriter: `length` is how long the die-cut label is in dots (`None`
/// for a continuous roll). The label's `height` runs across the head, at most
/// [`HEAD_DOTS`]; it must not be longer than the label.
pub fn encode(label: &Label, length: Option<usize>) -> Result<Vec<u8>, String> {
    if label.height == 0 || label.height > HEAD_DOTS {
        return Err(format!(
            "a label {} dots high does not fit the head ({HEAD_DOTS} dots)",
            label.height
        ));
    }
    if let Some(length) = length {
        if label.length > length {
            return Err(format!(
                "the drawing is {} dots long, the label {length}",
                label.length
            ));
        }
        if length > 0x7fff {
            return Err("a label longer than 2.7 m".to_owned());
        }
    }
    let bytes_per_line = label.height.div_ceil(8);
    let mut out = vec![ESC; RESET_ESCAPES];
    out.extend_from_slice(&[ESC, b'@', ESC, b'h', ESC, b'e']);
    let feed = length.map_or(0xffff, |l| l as u16);
    out.extend_from_slice(&[ESC, b'L', (feed >> 8) as u8, feed as u8]);
    out.extend_from_slice(&[ESC, b'D', bytes_per_line as u8]);
    let mut blank = 0usize;
    let flush = |out: &mut Vec<u8>, blank: &mut usize| {
        while *blank > 0 {
            let n = (*blank).min(255);
            out.extend_from_slice(&[ESC, b'f', 1, n as u8]);
            *blank -= n;
        }
    };
    let mut line = vec![0u8; bytes_per_line];
    for x in 0..label.length {
        line.fill(0);
        for dot in 0..label.height {
            let y = if TOP_AT_DOT_ZERO {
                dot
            } else {
                label.height - 1 - dot
            };
            if label.get(x, y) {
                line[dot / 8] |= 0x80 >> (dot % 8);
            }
        }
        if line.iter().all(|b| *b == 0) {
            blank += 1;
            continue;
        }
        flush(&mut out, &mut blank);
        let packed = run_length(&line);
        if packed.len() < line.len() {
            out.push(ETB);
            out.extend_from_slice(&packed);
        } else {
            out.push(SYN);
            out.extend_from_slice(&line);
        }
    }
    // Blank lines at the end need not be sent: the form feed goes to the next label.
    out.extend_from_slice(&[ESC, b'E']);
    Ok(out)
}

/// A line run-length coded: per run a byte, bit 7 its colour (1 black), bits 0 to 6 its
/// length less one (runs of 1 to 128 dots).
pub fn run_length(line: &[u8]) -> Vec<u8> {
    let bit = |i: usize| line[i / 8] & (0x80 >> (i % 8)) != 0;
    let total = line.len() * 8;
    let mut out = Vec::new();
    let mut i = 0;
    while i < total {
        let on = bit(i);
        let mut run = 1;
        while run < 128 && i + run < total && bit(i + run) == on {
            run += 1;
        }
        out.push(((on as u8) << 7) | (run - 1) as u8);
        i += run;
    }
    out
}

#[cfg(feature = "usb")]
pub use usb::{Printer, find};

#[cfg(feature = "usb")]
mod usb {
    use super::{PRODUCTS, STATUS_REQUEST, Status, VENDOR, encode};
    use crate::Label;
    use nusb::MaybeFuture;
    use nusb::descriptors::TransferType;
    use nusb::transfer::{Buffer, Bulk, Direction, In, Out};
    use std::time::Duration;

    /// USB printer class.
    const PRINTER_CLASS: u8 = 0x07;
    /// Bytes sent at a time: a label is about 100 kB at most uncompressed.
    const CHUNK: usize = 16 * 1024;

    /// A LabelWriter on USB, its printer interface claimed.
    pub struct Printer {
        id: nusb::DeviceId,
        out: nusb::Endpoint<Bulk, Out>,
        input: nusb::Endpoint<Bulk, In>,
        pub product: String,
        /// What it last said.
        pub last: Option<Status>,
        /// When it was last asked.
        pub asked_at: Option<std::time::Instant>,
        _interface: nusb::Interface,
    }

    /// The first LabelWriter on USB, opened. On Linux the kernel's printer driver (`usblp`)
    /// lets go of it while the agent holds it.
    pub fn find() -> Result<Option<Printer>, String> {
        let devices = nusb::list_devices().wait().map_err(|e| e.to_string())?;
        let Some((info, name)) = devices.into_iter().find_map(|d| {
            let name = PRODUCTS
                .iter()
                .find(|(pid, _)| d.vendor_id() == VENDOR && *pid == d.product_id())?
                .1;
            Some((d, name))
        }) else {
            return Ok(None);
        };
        let product = format!("DYMO {name}");
        let device = info
            .open()
            .wait()
            .map_err(|e| format!("{product}: cannot open it ({e})"))?;
        let config = device
            .active_configuration()
            .map_err(|e| format!("{product}: {e}"))?;
        let found = config.interface_alt_settings().find_map(|alt| {
            if alt.class() != PRINTER_CLASS {
                return None;
            }
            let bulk = |dir: Direction| {
                alt.endpoints()
                    .find(|e| e.transfer_type() == TransferType::Bulk && e.direction() == dir)
                    .map(|e| e.address())
            };
            Some((
                alt.interface_number(),
                bulk(Direction::Out)?,
                bulk(Direction::In)?,
            ))
        });
        let Some((number, out, input)) = found else {
            return Err(format!("{product}: no printer interface"));
        };
        #[cfg(target_os = "linux")]
        let claimed = device.detach_and_claim_interface(number).wait();
        #[cfg(not(target_os = "linux"))]
        let claimed = device.claim_interface(number).wait();
        let interface = claimed.map_err(|e| {
            format!("{product}: cannot claim it ({e}); is another program (CUPS) using it?")
        })?;
        let out = interface
            .endpoint::<Bulk, Out>(out)
            .map_err(|e| format!("{product}: {e}"))?;
        let input = interface
            .endpoint::<Bulk, In>(input)
            .map_err(|e| format!("{product}: {e}"))?;
        Ok(Some(Printer {
            id: info.id(),
            out,
            input,
            product,
            last: None,
            asked_at: None,
            _interface: interface,
        }))
    }

    impl Printer {
        /// Whether it is still on USB, without sending it anything.
        pub fn present(&self) -> bool {
            nusb::list_devices()
                .wait()
                .map_or(true, |mut d| d.any(|d| d.id() == self.id))
        }

        fn send(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), String> {
            for part in bytes.chunks(CHUNK) {
                let done = self
                    .out
                    .transfer_blocking(Buffer::from(part.to_vec()), timeout);
                done.status.map_err(|e| format!("sending: {e}"))?;
            }
            Ok(())
        }

        /// Asks its status.
        pub fn status(&mut self) -> Result<Status, String> {
            self.send(&STATUS_REQUEST, Duration::from_secs(2))?;
            self.asked_at = Some(std::time::Instant::now());
            let size = self.input.max_packet_size();
            let answer = self
                .input
                .transfer_blocking(Buffer::new(size), Duration::from_secs(2));
            answer
                .status
                .map_err(|e| format!("the printer does not answer: {e}"))?;
            let byte = *answer.buffer.first().ok_or("the printer answers nothing")?;
            let status = Status(byte);
            if self.last != Some(status) {
                println!("the LabelWriter says {byte:#04x}");
            }
            self.last = Some(status);
            Ok(status)
        }

        /// Prints a label: `length` dots long (die-cut), or `None` on a continuous roll.
        pub fn print(&mut self, label: &Label, length: Option<usize>) -> Result<(), String> {
            if let Some(problem) = self.status()?.problem() {
                return Err(problem.to_owned());
            }
            let bytes = encode(label, length)?;
            println!("label: {} bytes to the LabelWriter", bytes.len());
            self.send(&bytes, Duration::from_secs(30))?;
            // It answers once the label is through.
            let after = self.status()?;
            match after.problem() {
                Some(problem) => Err(problem.to_owned()),
                None => Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_runs(packed: &[u8], dots: usize) -> Vec<bool> {
        let mut out = Vec::new();
        for b in packed {
            let run = (b & 0x7f) as usize + 1;
            out.extend(std::iter::repeat_n(b & 0x80 != 0, run));
        }
        assert_eq!(out.len(), dots, "runs add up to the line");
        out
    }

    #[test]
    fn runs_code_a_line() {
        // As the technical reference's examples: 1 white, 1 black, 16 white, 128 black.
        assert_eq!(run_length(&[0x00]), vec![0x07]);
        assert_eq!(run_length(&[0xff]), vec![0x87]);
        assert_eq!(run_length(&[0b1000_0000]), vec![0x80, 0x06]);
        assert_eq!(run_length(&[0xff; 16]), vec![0xff]);
        // A blank line of the head: 5 runs of 128, then 32.
        assert_eq!(
            run_length(&[0x00; 84]),
            vec![0x7f, 0x7f, 0x7f, 0x7f, 0x7f, 0x1f]
        );
        let line = [0x0f, 0xf0, 0x55, 0x00, 0xff];
        let bits: Vec<bool> = (0..40)
            .map(|i| line[i / 8] & (0x80 >> (i % 8)) != 0)
            .collect();
        assert_eq!(decode_runs(&run_length(&line), 40), bits);
    }

    #[test]
    fn a_label_is_framed_as_the_printer_wants() {
        // 36 mm across (425 dots), 89 mm long (1051 dots): a 99012 address label.
        let (across, along) = (dots_for_mm(36.0), dots_for_mm(89.0));
        assert_eq!((across, along), (425, 1051));
        let mut label = Label::blank(along, across);
        // A bar across the label at x 100 and 101, and the top dot at x 500.
        for y in 0..across {
            label.set(100, y, true);
            label.set(101, y, true);
        }
        label.set(500, 0, true);
        let bytes = encode(&label, Some(along)).unwrap();
        assert!(bytes[..RESET_ESCAPES].iter().all(|b| *b == ESC));
        let rest = &bytes[RESET_ESCAPES..];
        assert_eq!(&rest[..6], &[ESC, b'@', ESC, b'h', ESC, b'e']);
        // The length most significant byte first: 1051 is 04 1B.
        assert_eq!(&rest[6..10], &[ESC, b'L', 0x04, 0x1b]);
        assert_eq!(&rest[10..13], &[ESC, b'D', 54], "425 dots in 54 bytes");
        // 100 blank lines skipped, then the bar (all black: one run would not do, 425 > 128).
        assert_eq!(&rest[13..17], &[ESC, b'f', 1, 100]);
        assert_eq!(rest[17], ETB);
        assert!(bytes.ends_with(&[ESC, b'E']));
        // The lone dot: dot 0 of the head, bit 7 of the first byte.
        let at = rest
            .windows(4)
            .position(|w| w == [ESC, b'f', 1, 255])
            .unwrap();
        let after = &rest[at..];
        // 398 blank lines between 101 and 500: 255 then 143.
        assert_eq!(&after[..8], &[ESC, b'f', 1, 255, ESC, b'f', 1, 143]);
        assert_eq!(after[8], ETB);
        assert_eq!(after[9], 0x80, "one black dot first");
    }

    #[test]
    fn a_continuous_roll_and_what_does_not_fit() {
        let label = Label::blank(10, 64);
        let bytes = encode(&label, None).unwrap();
        let at = RESET_ESCAPES + 6;
        assert_eq!(&bytes[at..at + 4], &[ESC, b'L', 0xff, 0xff]);
        assert!(encode(&Label::blank(10, HEAD_DOTS + 1), None).is_err());
        assert!(
            encode(&Label::blank(200, 64), Some(100)).is_err(),
            "longer than the label"
        );
        assert!(encode(&Label::blank(100, 64), Some(100)).is_ok());
    }

    #[test]
    fn status_reads() {
        assert_eq!(Status(0x03).problem(), None);
        assert!(Status(0x03).top_of_form() && Status(0x03).ready());
        assert!(Status(0xa1).problem().unwrap().contains("no labels"));
        assert!(Status(0x41).problem().unwrap().contains("jammed"));
        assert!(Status(0x81).problem().is_some());
    }
}
