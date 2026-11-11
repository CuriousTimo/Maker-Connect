//! DYMO LabelManager tape printers (the PnP and its kin), and (feature `usb`) printing on one.
//!
//! **Exactly as labelle does it** (labelle-org/labelle, Apache-2.0, at commit `5e8f1e5`:
//! `lib/devices/dymo_labeler.py`, `lib/devices/usb_device.py`, `lib/constants.py`): the
//! best-maintained program that prints on these printers. Its sequence is followed byte for
//! byte, its quirks included; nothing of our own is added to what goes to the printer. Read
//! for what it sends, not copied. See `docs/audits/dymo-labelmanager.md`.
//!
//! A label is sent as labelle's `print_label` sends it:
//!
//! - the label's columns ("lines") in groups of [`MAX_LINES`] (while more than
//!   `MAX_LINES + 1` are left), each group one command: `ESC C 0` (tape colour), then per
//!   line `ESC D n` where n changes (bytes a line, once for the whole label) and `SYN` with
//!   its n bytes, then `ESC A` twice (`_raw_print_label`'s status request, then
//!   `_get_status`'s);
//! - each command sent in transfers of at most [`SYN_WAIT`] `SYN` bytes (every `0x16`
//!   counts, pixel data too: labelle counts bytes), each transfer after an `ESC A` and a read
//!   of its answer; after the last, one more read;
//! - the printer opened for the label and let go of afterwards: nothing is sent while idle.

use crate::Label;

pub const DPI: f64 = 180.0;
/// labelle's `_maxLines`: lines in one command.
pub const MAX_LINES: usize = 200;
/// labelle's `synwait` (`DymoLabeler._functions`): `SYN` bytes in one transfer.
pub const SYN_WAIT: usize = 64;
/// Blank tape after a label, part of the label's picture (layout, not protocol): brings its
/// end past the cutter.
pub const TRAILING_MM: f64 = 16.0;

const ESC: u8 = 0x1b;
const SYN: u8 = 0x16;
/// `ESC A`: the printer answers with its status.
pub const STATUS_REQUEST: [u8; 2] = [ESC, b'A'];

/// A tape width the head prints on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tape {
    pub width_mm: u32,
    /// Dots across the tape: labelle's `height_px`, 8 × its bytes a line.
    pub dots: usize,
}

impl Tape {
    /// labelle's tape sizes on a LabelManager; 19 mm only on the printers that take it.
    pub fn d1(width_mm: u32) -> Option<Tape> {
        if ![19, 12, 9, 6].contains(&width_mm) {
            return None;
        }
        Some(Tape {
            width_mm,
            dots: Tape::bytes_for(width_mm) * 8,
        })
    }

    /// labelle's `_max_bytes_per_line`: `int(8 * tape_size_mm / 12)`.
    fn bytes_for(width_mm: u32) -> usize {
        (8 * width_mm / 12) as usize
    }

    pub fn bytes_per_column(&self) -> usize {
        self.dots / 8
    }
}

/// Columns of blank tape for so many millimetres.
pub fn columns_for_mm(mm: f64) -> usize {
    (mm / 25.4 * DPI).round() as usize
}

/// How full its pack is, as the PnP itself says it: byte 3 of a status answer, 0 to 100.
/// Seen 2026-11-10 on one PnP: 100 with an old pack on the charger; a new pack put in read
/// 43 and rose while charging. Only read from the answers labelle's own requests bring;
/// labelle itself does not read it (its issue #120). None when the answer is short or the
/// value is no percentage.
pub fn battery_reported(raw: &[u8]) -> Option<u8> {
    raw.get(3).copied().filter(|p| *p <= 100)
}

/// What the printer says it is doing: the first byte of its answer. Only read, never acted
/// on (labelle does not look at it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status(pub u8);

impl Status {
    pub fn cassette(self) -> bool {
        self.0 & 0x40 != 0
    }
}

/// The commands for a label, as labelle's `print_label` builds them: one a group of lines.
/// The label is centred across the tape; one higher than the tape is refused.
pub fn encode(label: &Label, tape: Tape) -> Result<Vec<Vec<u8>>, String> {
    if label.height > tape.dots {
        return Err(format!(
            "the label is {} dots high; {} mm tape takes {}",
            label.height, tape.width_mm, tape.dots
        ));
    }
    let bytes = tape.bytes_per_column();
    let top = (tape.dots - label.height) / 2;
    let count = label.length + columns_for_mm(TRAILING_MM);
    let lines: Vec<Vec<u8>> = (0..count)
        .map(|x| {
            let mut line = vec![0u8; bytes];
            for y in 0..label.height {
                if label.get(x, y) {
                    // Counted from the bottom of the tape: bit 7 of the first byte (found on
                    // the printer: the other way round prints it upside down).
                    let pin = tape.dots - 1 - (top + y);
                    line[pin / 8] |= 0x80 >> (pin % 8);
                }
            }
            line
        })
        .collect();
    Ok(commands(lines))
}

/// labelle's `print_label` on its lines (a line: one column of the label, its bytes): the
/// commands, one a group of lines.
pub fn commands(mut lines: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    // `print_label`: groups of MAX_LINES while more than MAX_LINES + 1 are left. Its bytes
    // a line are kept across the groups (one `DymoLabelerFunctions` for the label).
    let mut groups = Vec::new();
    while lines.len() > MAX_LINES + 1 {
        let rest = lines.split_off(MAX_LINES);
        groups.push(std::mem::replace(&mut lines, rest));
    }
    groups.push(lines);
    let mut bytes_per_line = None;
    groups
        .into_iter()
        .map(|group| {
            // `_raw_print_label`.
            let mut cmd = vec![ESC, b'C', 0];
            for line in group {
                if bytes_per_line != Some(line.len()) {
                    cmd.extend_from_slice(&[ESC, b'D', line.len() as u8]);
                    bytes_per_line = Some(line.len());
                }
                cmd.push(SYN);
                cmd.extend_from_slice(&line);
            }
            cmd.extend_from_slice(&STATUS_REQUEST);
            cmd.extend_from_slice(&STATUS_REQUEST);
            cmd
        })
        .collect()
}

/// One step on USB: a write to the OUT endpoint, or a read (of up to 512 bytes) from the IN
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Write(Vec<u8>),
    Read,
}

/// `array.tofile` writes in blocks of 64 KiB.
const WRITE_BLOCK: usize = 64 * 1024;

/// Everything sent and read for these commands, in order, as labelle's `_send_command` (with
/// `synwait`) does it: per transfer `ESC A`, a read, the transfer; after a command one more
/// read.
pub fn steps(commands: &[Vec<u8>]) -> Vec<Step> {
    let write = |out: &mut Vec<Step>, bytes: &[u8]| {
        out.extend(bytes.chunks(WRITE_BLOCK).map(|b| Step::Write(b.to_vec())));
    };
    let mut out = Vec::new();
    for cmd in commands {
        let mut rest = &cmd[..];
        while !rest.is_empty() {
            write(&mut out, &STATUS_REQUEST);
            out.push(Step::Read);
            let (now, later) = next_transfer(rest);
            write(&mut out, now);
            rest = later;
        }
        out.push(Step::Read);
    }
    out
}

/// labelle's `_send_command` split: the next transfer of a command and what is left. It runs
/// to (not including) the [`SYN_WAIT`]th `SYN` byte from its start, or to the end.
pub fn next_transfer(cmd: &[u8]) -> (&[u8], &[u8]) {
    let mut pos: Option<usize> = None;
    let mut syns = 0;
    while syns < SYN_WAIT {
        let from = pos.map_or(0, |p| p + 1);
        match cmd[from..].iter().position(|&b| b == SYN) {
            Some(i) => {
                pos = Some(from + i);
                syns += 1;
            }
            None => {
                pos = Some(cmd.len());
                break;
            }
        }
    }
    cmd.split_at(pos.unwrap_or(cmd.len()))
}

/// A USB device's descriptors, as far as opening a LabelManager reads them.
#[derive(Debug, Clone)]
pub struct Configuration {
    pub value: u8,
    pub interfaces: Vec<AltSetting>,
}

#[derive(Debug, Clone)]
pub struct AltSetting {
    pub number: u8,
    pub alt: u8,
    pub class: u8,
    /// Endpoint addresses (bit 7: IN), in descriptor order.
    pub endpoints: Vec<u8>,
}

/// What a session does on USB: the operations of pyusb's backend that labelle's code reaches.
/// Implemented on nusb for the printer and by a recorder in the tests.
pub trait Usb {
    fn open_device(&mut self) -> Result<(), String>;
    fn configurations(&self) -> Vec<Configuration>;
    /// `get_configuration`: the active configuration's value, None when unconfigured.
    fn get_configuration(&mut self) -> Option<u8>;
    fn set_configuration(&mut self, value: u8) -> Result<(), String>;
    /// labelle: `is_kernel_driver_active`, then `detach_kernel_driver` when it is (in
    /// `setup`), then `claim_interface` (pyusb, at the first transfer). nusb does the three in
    /// one call (`detach_and_claim_interface`, Linux `USBDEVFS_DISCONNECT_CLAIM`).
    fn detach_and_claim(&mut self, interface: u8) -> Result<(), String>;
    fn write(&mut self, endpoint: u8, bytes: &[u8], timeout_ms: u32) -> Result<(), String>;
    fn read(&mut self, endpoint: u8, size: usize, timeout_ms: u32) -> Result<Vec<u8>, String>;
    /// `dispose`: `release_interface`, then `close_device`.
    fn release_and_close(&mut self, interface: u8);
}

/// pyusb's default timeout (`_DEFAULT_TIMEOUT`); labelle sets none.
pub const TIMEOUT_MS: u32 = 1000;
/// labelle reads `_devin.read(512)`.
pub const READ_SIZE: usize = 512;
/// labelle's `PRINTER_INTERFACE_CLASS` and `HID_INTERFACE_CLASS`.
const PRINTER_CLASS: u8 = 0x07;
const HID_CLASS: u8 = 0x03;

/// labelle's `UsbDevice.setup`: the active configuration (set to the first when there is
/// none), the printer-class interface, else the HID one, its kernel driver detached and
/// claimed, its first OUT and IN endpoint. Gives (interface, OUT, IN).
fn open(usb: &mut impl Usb) -> Result<(u8, u8, u8), String> {
    usb.open_device()?;
    let configs = usb.configurations();
    let active = match usb.get_configuration() {
        Some(value) => value,
        None => {
            let first = configs.first().ok_or("no configuration")?.value;
            usb.set_configuration(first)?;
            first
        }
    };
    let config = configs
        .iter()
        .find(|c| c.value == active)
        .ok_or("Configuration not set")?;
    let alt = config
        .interfaces
        .iter()
        .find(|a| a.class == PRINTER_CLASS)
        .or_else(|| config.interfaces.iter().find(|a| a.class == HID_CLASS))
        .ok_or("Could not open a valid interface")?;
    let out = alt.endpoints.iter().find(|e| *e & 0x80 == 0);
    let input = alt.endpoints.iter().find(|e| *e & 0x80 != 0);
    let (Some(&out), Some(&input)) = (out, input) else {
        return Err("The device endpoints not be found".to_owned());
    };
    usb.detach_and_claim(alt.number)?;
    Ok((alt.number, out, input))
}

/// A label printed as labelle's `DymoLabeler(device=…)` + `print` does it: [`open`], the
/// commands as [`steps`], then `dispose`. `heard` gets every answer.
pub fn session(
    usb: &mut impl Usb,
    commands: &[Vec<u8>],
    mut heard: impl FnMut(Vec<u8>),
    mut progress: impl FnMut(usize, usize),
) -> Result<(), String> {
    let (interface, out, input) = open(usb)?;
    let steps = steps(commands);
    let total = steps.len();
    for (i, step) in steps.iter().enumerate() {
        match step {
            Step::Write(bytes) => usb.write(out, bytes, TIMEOUT_MS)?,
            Step::Read => heard(usb.read(input, READ_SIZE, TIMEOUT_MS)?),
        }
        progress(i + 1, total);
    }
    usb.release_and_close(interface);
    Ok(())
}

/// **Ours, not labelle's** (labelle sends nothing while idle; the owner's decision
/// 2026-11-11, to show the cassette and battery between labels): the printer opened as for a
/// label ([`open`]), one `ESC A` and its answer read, then let go of (`dispose`).
pub fn status_session(usb: &mut impl Usb, mut heard: impl FnMut(Vec<u8>)) -> Result<(), String> {
    let (interface, out, input) = open(usb)?;
    usb.write(out, &STATUS_REQUEST, TIMEOUT_MS)?;
    heard(usb.read(input, READ_SIZE, TIMEOUT_MS)?);
    usb.release_and_close(interface);
    Ok(())
}

#[cfg(feature = "usb")]
pub use usb::{Printer, find, hex};

#[cfg(feature = "usb")]
mod usb {
    use super::{AltSetting, Configuration, Status, Tape, Usb, encode, session, status_session};
    use crate::Label;
    use nusb::MaybeFuture;
    use nusb::descriptors::TransferType;
    use nusb::transfer::{Buffer, Bulk, In, Interrupt, Out};
    use std::time::Duration;

    /// labelle's `DEV_VENDOR` and the LabelManagers of its `SUPPORTED_PRODUCTS`.
    const VENDOR: u16 = 0x0922;
    const PRODUCTS: [u16; 12] = [
        0x0011, 0x001c, 0x0015, 0x1001, 0x1002, 0x1003, 0x1004, 0x1005, 0x1006, 0x1007,
        0x1008, 0x1009,
    ];

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

    /// A LabelManager seen on USB. Not opened: as labelle, it is opened for a label only.
    pub struct Printer {
        id: nusb::DeviceId,
        pub product: String,
        /// The last answer it gave (during a label, or to an idle status request), its first
        /// 8 bytes.
        pub last_raw: Vec<u8>,
        /// When it was last asked while idle ([`Printer::status`]).
        pub asked_at: Option<std::time::Instant>,
    }

    /// The first LabelManager on USB (from the system's device list: nothing is sent).
    pub fn find() -> Result<Option<Printer>, String> {
        let devices = nusb::list_devices().wait().map_err(|e| e.to_string())?;
        let Some(info) = devices
            .into_iter()
            .find(|d| d.vendor_id() == VENDOR && PRODUCTS.contains(&d.product_id()))
        else {
            return Ok(None);
        };
        Ok(Some(Printer {
            id: info.id(),
            product: info.product_string().unwrap_or("DYMO LabelManager").to_owned(),
            last_raw: Vec::new(),
            asked_at: None,
        }))
    }

    /// [`Usb`] on nusb.
    struct Nusb {
        info: nusb::DeviceInfo,
        device: Option<nusb::Device>,
        interface: Option<nusb::Interface>,
    }

    impl Nusb {
        fn device(&self) -> Result<&nusb::Device, String> {
            self.device.as_ref().ok_or("not open".to_owned())
        }

        fn interrupt(&self, endpoint: u8) -> bool {
            self.device.as_ref().is_some_and(|d| {
                d.configurations().any(|c| {
                    c.interface_alt_settings().any(|a| {
                        a.endpoints().any(|e| {
                            e.address() == endpoint
                                && e.transfer_type() == TransferType::Interrupt
                        })
                    })
                })
            })
        }
    }

    impl Usb for Nusb {
        fn open_device(&mut self) -> Result<(), String> {
            let device = self.info.open().wait().map_err(|e| format!("cannot open it ({e})"))?;
            self.device = Some(device);
            Ok(())
        }

        fn configurations(&self) -> Vec<Configuration> {
            let Some(device) = &self.device else {
                return Vec::new();
            };
            device
                .configurations()
                .map(|c| Configuration {
                    value: c.configuration_value(),
                    interfaces: c
                        .interface_alt_settings()
                        .map(|a| AltSetting {
                            number: a.interface_number(),
                            alt: a.alternate_setting(),
                            class: a.class(),
                            endpoints: a.endpoints().map(|e| e.address()).collect(),
                        })
                        .collect(),
                })
                .collect()
        }

        fn get_configuration(&mut self) -> Option<u8> {
            let device = self.device.as_ref()?;
            device.active_configuration().ok().map(|c| c.configuration_value())
        }

        fn set_configuration(&mut self, value: u8) -> Result<(), String> {
            self.device()?
                .set_configuration(value)
                .wait()
                .map_err(|e| e.to_string())
        }

        fn detach_and_claim(&mut self, interface: u8) -> Result<(), String> {
            let device = self.device()?;
            #[cfg(target_os = "linux")]
            let claimed = device.detach_and_claim_interface(interface).wait();
            #[cfg(not(target_os = "linux"))]
            let claimed = device.claim_interface(interface).wait();
            self.interface = Some(claimed.map_err(|e| format!("cannot claim it ({e})"))?);
            Ok(())
        }

        fn write(&mut self, endpoint: u8, bytes: &[u8], timeout_ms: u32) -> Result<(), String> {
            let interrupt = self.interrupt(endpoint);
            let interface = self.interface.as_ref().ok_or("not claimed")?;
            let (buf, timeout) = (Buffer::from(bytes.to_vec()), Duration::from_millis(timeout_ms.into()));
            let done = if interrupt {
                let mut ep = interface.endpoint::<Interrupt, Out>(endpoint).map_err(|e| e.to_string())?;
                ep.transfer_blocking(buf, timeout)
            } else {
                let mut ep = interface.endpoint::<Bulk, Out>(endpoint).map_err(|e| e.to_string())?;
                ep.transfer_blocking(buf, timeout)
            };
            done.status.map_err(|e| format!("sending: {e}"))
        }

        fn read(&mut self, endpoint: u8, size: usize, timeout_ms: u32) -> Result<Vec<u8>, String> {
            let interrupt = self.interrupt(endpoint);
            let interface = self.interface.as_ref().ok_or("not claimed")?;
            let (buf, timeout) = (Buffer::new(size), Duration::from_millis(timeout_ms.into()));
            let done = if interrupt {
                let mut ep = interface.endpoint::<Interrupt, In>(endpoint).map_err(|e| e.to_string())?;
                ep.transfer_blocking(buf, timeout)
            } else {
                let mut ep = interface.endpoint::<Bulk, In>(endpoint).map_err(|e| e.to_string())?;
                ep.transfer_blocking(buf, timeout)
            };
            done.status.map_err(|e| format!("the printer does not answer: {e}"))?;
            Ok(done.buffer.to_vec())
        }

        fn release_and_close(&mut self, _interface: u8) {
            self.interface = None;
            self.device = None;
        }
    }

    impl Printer {
        pub fn last(&self) -> Option<Status> {
            self.last_raw.first().map(|b| Status(*b))
        }

        /// Whether it is still on USB, from the system's device list: nothing is sent.
        pub fn present(&self) -> bool {
            nusb::list_devices()
                .wait()
                .map_or(true, |mut d| d.any(|d| d.id() == self.id))
        }

        /// Opened afresh for every session, as labelle opens it for every label.
        fn usb(&self) -> Result<Nusb, String> {
            let info = nusb::list_devices()
                .wait()
                .map_err(|e| e.to_string())?
                .find(|d| d.id() == self.id)
                .ok_or(format!("{}: gone from USB", self.product))?;
            Ok(Nusb {
                info,
                device: None,
                interface: None,
            })
        }

        /// Keeps its answers; says them in the log (`always`), or only when the known bytes
        /// (0 to 3: status, charge) change.
        fn heard(&mut self, answers: Vec<Vec<u8>>, always: bool) {
            for answer in answers {
                let raw = answer[..answer.len().min(8)].to_vec();
                if raw.is_empty() {
                    continue;
                }
                let known = |r: &[u8]| r.iter().take(4).copied().collect::<Vec<_>>();
                if always || known(&raw) != known(&self.last_raw) {
                    let battery = super::battery_reported(&raw)
                        .map(|p| format!(" (battery {p} %)"))
                        .unwrap_or_default();
                    println!("{} the DYMO answers: {}{battery}", clock(), hex(&raw));
                }
                self.last_raw = raw;
            }
        }

        /// Prints a label as labelle does ([`session`]); `progress` hears the steps done.
        pub fn print(
            &mut self,
            label: &Label,
            tape: Tape,
            progress: impl FnMut(usize, usize),
        ) -> Result<(), String> {
            let commands = encode(label, tape)?;
            let mut usb = self.usb()?;
            println!("{} to the DYMO: a label", clock());
            let mut answers = Vec::new();
            let result = session(&mut usb, &commands, |a| answers.push(a), progress);
            self.heard(answers, true);
            result.map_err(|e| format!("{}: {e}", self.product))
        }

        /// **Ours, not labelle's** (the owner's decision, 2026-11-11): its status asked while
        /// idle ([`status_session`]), for the cassette and battery between labels.
        pub fn status(&mut self) -> Result<(), String> {
            self.asked_at = Some(std::time::Instant::now());
            let mut usb = self.usb()?;
            let mut answers = Vec::new();
            let result = status_session(&mut usb, |a| answers.push(a));
            self.heard(answers, false);
            result.map_err(|e| format!("{}: {e}", self.product))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_is_built_as_labelle_builds_it() {
        // One column: its top dot and its bottom dot.
        let mut label = Label::blank(1, 64);
        label.set(0, 0, true);
        label.set(0, 63, true);
        let tape = Tape::d1(12).unwrap();
        let commands = encode(&label, tape).unwrap();
        let lines = 1 + columns_for_mm(TRAILING_MM);
        assert_eq!(lines, 114);
        assert_eq!(commands.len(), 1, "fewer than MAX_LINES + 2 lines: one command");
        let cmd = &commands[0];
        // `ESC C 0`, `ESC D 8` once, no `ESC B`.
        assert_eq!(&cmd[..6], &[0x1b, b'C', 0, 0x1b, b'D', 8]);
        // The bottom of the label is bit 7 of the first byte, its top bit 0 of the last.
        assert_eq!(&cmd[6..15], &[0x16, 0x80, 0, 0, 0, 0, 0, 0, 0x01]);
        assert_eq!(cmd.len(), 6 + lines * 9 + 4);
        // Ends in `ESC A` twice.
        assert_eq!(&cmd[cmd.len() - 4..], &[0x1b, b'A', 0x1b, b'A']);
    }

    #[test]
    fn a_long_label_goes_in_groups_as_labelle_splits_it() {
        let tape = Tape::d1(12).unwrap();
        let trailing = columns_for_mm(TRAILING_MM);
        // 201 lines: still one group (labelle: while len > 201).
        let commands = encode(&Label::blank(201 - trailing, 64), tape).unwrap();
        assert_eq!(commands.len(), 1);
        // 202 lines: 200, then 2; `ESC D` only in the first.
        let commands = encode(&Label::blank(202 - trailing, 64), tape).unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(&commands[0][..6], &[0x1b, b'C', 0, 0x1b, b'D', 8]);
        assert_eq!(commands[0].len(), 6 + 200 * 9 + 4);
        assert_eq!(&commands[1][..4], &[0x1b, b'C', 0, 0x16]);
        assert_eq!(commands[1].len(), 3 + 2 * 9 + 4);
    }

    #[test]
    fn transfers_end_before_the_64th_syn_as_labelle_cuts_them() {
        // ESC C 0 ESC D 1, then 100 lines `SYN 00`, then ESC A ESC A.
        let mut cmd = vec![0x1b, b'C', 0, 0x1b, b'D', 1];
        for _ in 0..100 {
            cmd.extend_from_slice(&[0x16, 0x00]);
        }
        cmd.extend_from_slice(&[0x1b, b'A', 0x1b, b'A']);
        let (first, rest) = next_transfer(&cmd);
        // Up to the 64th SYN: the prefix and 63 lines.
        assert_eq!(first.len(), 6 + 63 * 2);
        assert_eq!(rest[0], 0x16);
        // The rest (37 lines and the two `ESC A`): no 64th SYN, so to the end.
        let (second, rest) = next_transfer(rest);
        assert!(rest.is_empty());
        assert_eq!(second.len(), 37 * 2 + 4);
        assert_eq!(&second[second.len() - 4..], &[0x1b, b'A', 0x1b, b'A']);
        // A data byte 0x16 counts as a SYN, as labelle counts bytes.
        let data = [0x16u8; 70];
        assert_eq!(next_transfer(&data).0.len(), 63);
    }

    #[test]
    fn narrow_tape_takes_labelles_bytes_a_line() {
        let tape = Tape::d1(6).unwrap();
        assert_eq!((tape.dots, tape.bytes_per_column()), (32, 4));
        assert_eq!(Tape::d1(9).unwrap().dots, 48);
        assert_eq!(Tape::d1(19).unwrap().dots, 96);
        assert_eq!(Tape::d1(24), None);
        let mut label = Label::blank(1, 30);
        label.set(0, 0, true);
        let commands = encode(&label, tape).unwrap();
        assert_eq!(&commands[0][3..6], &[0x1b, b'D', 4]);
        // 30 dots on 32: one blank dot above, so its top is the tape's second dot from the top.
        assert_eq!(&commands[0][6..11], &[0x16, 0, 0, 0, 0x02]);
        assert!(encode(&Label::blank(1, 40), tape).is_err());
    }

    #[test]
    fn the_battery_is_what_it_says() {
        assert_eq!(battery_reported(&[0x40, 0, 0x12, 78, 0x0d, 0x6b, 0, 0]), Some(78));
        assert_eq!(battery_reported(&[0x40, 0, 0x12]), None, "too short");
        assert_eq!(battery_reported(&[0, 0, 0, 200]), None, "no percentage");
        assert!(Status(0x40).cassette());
    }
}
