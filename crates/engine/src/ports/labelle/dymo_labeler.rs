//! `src/labelle/lib/devices/dymo_labeler.py` (labelle 5e8f1e5): `DymoLabelerFunctions` and
//! `DymoLabeler`, function by function (`print` is `dymo_labeler.py:307-352`).
//!
//! labelle: Copyright (c) 2011 Sebastian J. Bronner <waschtl@sbronner.com>; the file's own notice:
//! "Copying and distribution of this file, with or without modification, are permitted in any
//! medium without royalty provided the copyright notice and this notice are preserved."
//!
//! Not ported (nothing that prints calls them): `_reset_command`, `_dot_tab`, `_cut`,
//! `_chain_mark`, `_skip_lines`, `_init_label`, `minimum_horizontal_margin_mm`'s setter-free
//! siblings `device` (setter), `is_ready`. Logging (`LOG.debug`) is comments.
//!
//! **Ours:** `heard` (a hook called with every answer the printer gives: labelle reads and
//! throws the answers away), see `port.rs`.

use super::constants::{ESC, SYN};
use super::pillow::Image;
use super::pyusb::{Endpoint, Error as PyusbError};
use super::usb_device::{self, UsbDevice};
use super::utils::mm_to_px;

/// The `ValueError` and the errors `print` raises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `ValueError`.
    Value(String),
    /// `DymoLabelerPrintError`: its message is `Print error: ...`.
    Print(String),
    /// `RuntimeError("An internal problem was encountered ...")`, or another error pyusb
    /// raises that `print` does not wrap (a `ValueError`, `IndexError`).
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Value(m) | Error::Other(m) => f.write_str(m),
            Error::Print(m) => write!(f, "Print error: {m}"),
        }
    }
}

/// The command letters after `ESC` (`dymo_labeler.py:145` `_status_request`, `:148` `_tape_color`,
/// `:155` `_bytes_per_line`).
pub const CMD_STATUS_REQUEST: u8 = b'A';
pub const CMD_TAPE_COLOR: u8 = b'C';
pub const CMD_BYTES_PER_LINE: u8 = b'D';
/// The commands by the method that makes them (checked against the source by the audit).
pub const COMMANDS: [(&str, u8); 3] = [
    ("_status_request", CMD_STATUS_REQUEST),
    ("_tape_color", CMD_TAPE_COLOR),
    ("_bytes_per_line", CMD_BYTES_PER_LINE),
];
/// `self._maxLines = 200` (`dymo_labeler.py:75`): lines in one command.
pub const MAX_LINES: usize = 200;
/// `synwait=64` (`dymo_labeler.py:253`): `SYN` bytes in one transfer.
pub const SYNWAIT: usize = 64;

/// `DymoLabelerFunctions` (`dymo_labeler.py:38`).
struct DymoLabelerFunctions<'a> {
    cmd: Vec<u8>,
    response: bool,
    bytes_per_line: Option<usize>,
    max_lines: usize,
    devout: &'a Endpoint,
    devin: &'a Endpoint,
    synwait: Option<usize>,
    heard: &'a mut dyn FnMut(&[u8]),
}

/// What `array.tofile` writes at a time (`BLOCKSIZE` of `arraymodule.c`).
const WRITE_BLOCK: usize = 64 * 1024;

impl<'a> DymoLabelerFunctions<'a> {
    /// `__init__` (`dymo_labeler.py:64`).
    fn new(devout: &'a Endpoint, devin: &'a Endpoint, synwait: Option<usize>, heard: &'a mut dyn FnMut(&[u8])) -> Self {
        DymoLabelerFunctions {
            cmd: Vec::new(),
            response: false,
            bytes_per_line: None,
            max_lines: MAX_LINES,
            devout,
            devin,
            synwait,
            heard,
        }
    }

    /// `_max_bytes_per_line` (`dymo_labeler.py:81`): `int(8 * tape_size_mm / 12)`.
    fn max_bytes_per_line(tape_size_mm: i64) -> i64 {
        (8 * tape_size_mm) / 12
    }

    /// `height_px` (`dymo_labeler.py:85`).
    fn height_px(tape_size_mm: i64) -> i64 {
        Self::max_bytes_per_line(tape_size_mm) * 8
    }

    /// `array.array("B", data).tofile(self._devout)`: the file's `write`, a block at a time.
    fn tofile(&self, data: &[u8]) -> Result<(), PyusbError> {
        for block in data.chunks(WRITE_BLOCK) {
            self.devout.write(block)?;
        }
        Ok(())
    }

    /// `self._devin.read(512)`, the answer given to `heard`.
    fn read_answer(&mut self) -> Result<Vec<u8>, PyusbError> {
        let answer = self.devin.read(512)?;
        (self.heard)(&answer);
        Ok(answer)
    }

    /// `_send_command` (`dymo_labeler.py:88`): the answer of the status request at the end, if
    /// one was asked for.
    fn send_command(&mut self) -> Result<Option<Vec<u8>>, PyusbError> {
        if self.cmd.is_empty() {
            return Ok(None);
        }
        while !self.cmd.is_empty() {
            let (cmd_to_send, cmd_rest);
            match self.synwait {
                None => {
                    cmd_to_send = std::mem::take(&mut self.cmd);
                    cmd_rest = Vec::new();
                }
                Some(synwait) => {
                    // Send a status request
                    self.tofile(&[ESC, CMD_STATUS_REQUEST])?;
                    self.read_answer()?;
                    // Ok, we got a response. Now we can send a chunk of data

                    // Compute a chunk with at most synwait SYN characters
                    let mut syn_count = 0; // Number of SYN characters encountered in iteration
                    let mut pos: i64 = -1; // Index of last SYN character encountered in iteration
                    while syn_count < synwait {
                        // `pos += self._cmd[pos + 1 :].index(SYN) + 1`
                        match self.cmd[(pos + 1) as usize..].iter().position(|&b| b == SYN) {
                            Some(i) => {
                                pos += i as i64 + 1;
                                syn_count += 1;
                            }
                            // No more SYN characters in cmd
                            None => {
                                pos = self.cmd.len() as i64;
                                break;
                            }
                        }
                    }
                    cmd_to_send = self.cmd[..pos as usize].to_vec();
                    cmd_rest = self.cmd[pos as usize..].to_vec();
                }
            }
            // Remove the computed chunk from the command to be processed
            self.cmd = cmd_rest;
            // Send the chunk
            self.tofile(&cmd_to_send)?;
        }
        self.cmd = Vec::new(); // This looks redundant.
        if !self.response {
            return Ok(None);
        }
        self.response = false;
        Ok(Some(self.read_answer()?))
    }

    /// `_build_command` (`dymo_labeler.py:141`).
    fn build_command(&mut self, cmd: &[u8]) {
        self.cmd.extend_from_slice(cmd);
    }

    /// `_status_request` (`dymo_labeler.py:145`).
    fn status_request(&mut self) {
        self.build_command(&[ESC, CMD_STATUS_REQUEST]);
        self.response = true;
    }

    /// `_tape_color` (`dymo_labeler.py:160`).
    fn tape_color(&mut self, value: u8) {
        self.build_command(&[ESC, CMD_TAPE_COLOR, value]);
    }

    /// `_bytes_per_line` (`dymo_labeler.py:167`).
    fn set_bytes_per_line(&mut self, value: usize) {
        if Some(value) == self.bytes_per_line {
            return;
        }
        self.build_command(&[ESC, CMD_BYTES_PER_LINE, value as u8]);
        self.bytes_per_line = Some(value);
    }

    /// `_line` (`dymo_labeler.py:180`).
    fn line(&mut self, value: &[u8]) {
        self.set_bytes_per_line(value.len());
        let mut cmd = vec![SYN];
        cmd.extend_from_slice(value);
        self.build_command(&cmd);
    }

    /// `_get_status` (`dymo_labeler.py:211`).
    fn get_status(&mut self) -> Result<Option<Vec<u8>>, PyusbError> {
        self.status_request();
        self.send_command()
    }

    /// `print_label` (`dymo_labeler.py:216`): split when larger than `maxLines`.
    fn print_label(&mut self, mut lines: Vec<Vec<u8>>) -> Result<(), PyusbError> {
        while lines.len() > self.max_lines + 1 {
            self.raw_print_label(&lines[0..self.max_lines])?;
            lines.drain(0..self.max_lines);
        }
        self.raw_print_label(&lines)
    }

    /// `_raw_print_label` (`dymo_labeler.py:226`).
    fn raw_print_label(&mut self, lines: &[Vec<u8>]) -> Result<(), PyusbError> {
        // Here used to be a matrix optimization code that caused problems in issue #87
        self.tape_color(0);
        for line in lines {
            self.line(line);
        }
        self.status_request();
        // `status = self._get_status()`, logged only.
        self.get_status()?;
        Ok(())
    }
}

/// `DymoLabeler` (`dymo_labeler.py:237`).
#[derive(Debug)]
pub struct DymoLabeler {
    pub tape_size_mm: i64,
}

impl DymoLabeler {
    pub const LABELER_DISTANCE_BETWEEN_PRINT_HEAD_AND_CUTTER_MM: f64 = 8.1;
    pub const LABELER_PRINT_HEAD_HEIGHT_MM: f64 = 8.2;
    pub const SUPPORTED_TAPE_SIZES_MM: [i64; 4] = [19, 12, 9, 6];
    pub const DEFAULT_TAPE_SIZE_MM: i64 = 12;

    /// `__init__` (`dymo_labeler.py:244`): no tape size is `DEFAULT_TAPE_SIZE_MM`, then the
    /// supported-sizes check.
    pub fn new(tape_size_mm: Option<i64>) -> Result<DymoLabeler, Error> {
        let tape_size_mm = tape_size_mm.unwrap_or(Self::DEFAULT_TAPE_SIZE_MM);
        if !Self::SUPPORTED_TAPE_SIZES_MM.contains(&tape_size_mm) {
            return Err(Error::Value(format!(
                "Unsupported tape size {tape_size_mm}mm. Supported sizes: (19, 12, 9, 6)"
            )));
        }
        Ok(DymoLabeler { tape_size_mm })
    }

    /// `height_px` (`dymo_labeler.py:262`).
    pub fn height_px(&self) -> i64 {
        DymoLabelerFunctions::height_px(self.tape_size_mm)
    }

    /// `minimum_horizontal_margin_mm` (`dymo_labeler.py:275`).
    pub fn minimum_horizontal_margin_mm(&self) -> f64 {
        Self::LABELER_DISTANCE_BETWEEN_PRINT_HEAD_AND_CUTTER_MM
    }

    /// `labeler_margin_px` (`dymo_labeler.py:279`): (horizontal, vertical).
    pub fn labeler_margin_px(&self) -> (f64, f64) {
        let vertical_margin_mm = f64::max(0.0, (self.tape_size_mm as f64 - Self::LABELER_PRINT_HEAD_HEIGHT_MM) / 2.0);
        (mm_to_px(self.minimum_horizontal_margin_mm()), mm_to_px(vertical_margin_mm))
    }

    /// `print` (`dymo_labeler.py:307`): the label bitmap (mode "1", pixels equal to 1 burned) to
    /// the printer. `heard` is given every answer (ours).
    pub fn print(&self, device: &UsbDevice, bitmap: &Image, heard: &mut dyn FnMut(&[u8])) -> Result<(), Error> {
        // Convert the image to the proper matrix for the dymo labeler object so that rows span
        // the width of the label, and the first row corresponds to the left edge of the label.
        let rotated_bitmap = bitmap.rotate_270();

        // Convert the image to raw bytes. Pixels along rows are chunked into groups of 8
        // pixels, and subsequent rows are concatenated.
        let stream = rotated_bitmap.tobytes_one().map_err(Error::Other)?;

        // Regather the bytes into rows
        let stream_row_length = bitmap.height.div_ceil(8);
        if stream_row_length == 0 {
            return Err(Error::Other("integer division or modulo by zero".to_owned()));
        }
        if stream.len() / stream_row_length != bitmap.width {
            return Err(Error::Other(
                "An internal problem was encountered while processing the label bitmap!".to_owned(),
            ));
        }
        // Convert bytes into ints
        let label_matrix: Vec<Vec<u8>> = stream.chunks(stream_row_length).map(<[u8]>::to_vec).collect();

        // `_functions`: a new `DymoLabelerFunctions` with `synwait=64`.
        let (Some(devout), Some(devin)) = (device.devout(), device.devin()) else {
            return Err(Error::Other("'NoneType' object has no attribute 'write'".to_owned()));
        };
        let mut functions = DymoLabelerFunctions::new(devout, devin, Some(SYNWAIT), heard);
        let result = functions.print_label(label_matrix).map(|()| device.dispose());
        // POSSIBLE_USB_ERRORS = (UsbDeviceError, NoBackendError, USBError)
        match result {
            Ok(()) => Ok(()),
            Err(e @ (PyusbError::Usb(_) | PyusbError::NoBackend(_))) => Err(Error::Print(e.to_string())),
            Err(e) => Err(Error::Other(e.to_string())),
        }
    }
}

impl From<usb_device::Error> for Error {
    fn from(e: usb_device::Error) -> Error {
        Error::Other(e.to_string())
    }
}
