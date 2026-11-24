// Ported from DYMO's CUPS driver 1.4.0 (reference/dymo-cups-drivers-1.4.0):
//   src/lw/LabelWriterDriver.h, src/lw/LabelWriterDriver.cpp
// DYMO LabelWriter Drivers, Copyright (C) 2008 Sanford L.P.
// This program is free software; you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation; either version 2 of the License,
// or (at your option) any later version (taken as version 3, so it combines with this program's AGPL-3.0).
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even
// the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.

//! `LabelWriterDriver.cpp`: the LabelWriter command set. `CLabelWriterDriver` and its derived
//! `CLabelWriterDriver400` (what the filter uses for the 450, `raster2dymolw.cpp:92-103`; `CLabelWriterDriver`
//! is the 320's, :111-123) are one struct
//! here, [`LabelWriterDriver`], with the class as its [`Class`]: the virtual `StartDoc`, `EndDoc` and
//! `EndPage` are chosen by it. `CLabelWriterDriverTwinTurbo` is not ported (not a 450).

use super::printer_driver::{Buffer, Byte, PrinterDriver, SharedEnvironment};
use super::stderr;

const ESC: Byte = 0x1B; // LabelWriterDriver.cpp:31
const SYN: Byte = 0x16; // :32
const ETB: Byte = 0x17; // :33

/// `LabelWriterDriver.h:37-43` `density_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Low = 0,
    Medium,
    Normal,
    High,
}

/// `LabelWriterDriver.h:45-49` `quality_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Text = 0,
    BarcodeAndGraphics,
}

/// `LabelWriterDriver.h:51-55` `paper_type_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaperType {
    Regular = 0,
    Continuous,
}

/// `LabelWriterDriver.h:57-62` `resolution_t` ("unknown or irrelevant resolution", 136x204, 204x204 dpi).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Unknown = 0,
    Res136,
    Res204,
}

/// `LabelWriterDriver.h:64-74` `point_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// The driver classes of `LabelWriterDriver.h`: 34 `CLabelWriterDriver`, 144 `CLabelWriterDriver400`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Driver,
    Driver400,
}

/// `LabelWriterDriver.h:162-167` `CLabelWriterDriverTwinTurbo::roll_t`. Only the language monitor
/// (`LabelWriterLanguageMonitor.cpp:95`) uses it for a 450; the Twin Turbo driver is not ported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Roll {
    Auto = 0,
    Left,
    Right,
}

/// `CLabelWriterDriverTwinTurbo::GetRollSelectCommand` (:650-663): `ESC q 0|1|2`.
pub fn get_roll_select_command(value: Roll) -> Buffer {
    let third = match value {
        Roll::Left => b'1',
        Roll::Right => b'2',
        Roll::Auto => b'0',
    };
    vec![ESC, b'q', third]
}

/// `CLabelWriterDriver` / `CLabelWriterDriver400`.
pub struct LabelWriterDriver {
    class: Class,
    environment: SharedEnvironment,
    resolution: Resolution,
    density: Density,
    quality: Quality,
    page_height: usize,
    paper_type: PaperType,
    max_print_width: usize,
    page_offset: Point,
    last_dot_tab: usize,
    last_bytes_per_line: usize,
    empty_lines_count: usize,
}

/// `size_t(-1)`.
const SIZE_T_MINUS_1: usize = usize::MAX;

impl LabelWriterDriver {
    /// `CLabelWriterDriver::CLabelWriterDriver` (:35-40).
    pub fn new(environment: SharedEnvironment) -> Self {
        LabelWriterDriver {
            class: Class::Driver,
            environment,
            resolution: Resolution::Unknown,
            density: Density::Normal,
            quality: Quality::Text,
            page_height: 0x0800,
            paper_type: PaperType::Regular,
            max_print_width: 84,
            page_offset: Point { x: 0, y: 0 },
            last_dot_tab: SIZE_T_MINUS_1,
            last_bytes_per_line: SIZE_T_MINUS_1,
            empty_lines_count: 0,
        }
    }

    /// `CLabelWriterDriver400::CLabelWriterDriver400` (:575-578).
    pub fn new400(environment: SharedEnvironment) -> Self {
        let mut d = Self::new(environment);
        d.class = Class::Driver400;
        d
    }

    // ---- :84-146 ------------------------------------------------------------------------------

    /// `SendNotCompressedData` (:84-101).
    fn send_not_compressed_data(
        &mut self,
        buf: &[u8],
        leader_blanks: usize,
        trailer_blanks: usize,
    ) {
        let syn = SYN;
        let data_size = buf.len() - leader_blanks - trailer_blanks;

        // set bytes per line in case of it changes from last raster line
        if self.last_bytes_per_line != data_size {
            self.send_bytes_per_line(data_size);
            self.last_bytes_per_line = data_size;
        }

        self.send_command(&[syn]);
        self.send_command(&buf[leader_blanks..leader_blanks + data_size]);
    }

    /// `SendCompressedData` (:103-118).
    fn send_compressed_data(&mut self, compressed_buf: &[u8], not_compressed_size: usize) {
        let etb = ETB;

        // set bytes per line in case of it changes from last raster line
        if self.last_bytes_per_line != not_compressed_size {
            self.send_bytes_per_line(not_compressed_size);
            self.last_bytes_per_line = not_compressed_size;
        }

        self.send_command(&[etb]);
        self.send_command(compressed_buf);
    }

    /// `GetBlanks` (:120-146): (leader, trailer).
    fn get_blanks(buf: &[u8]) -> (usize, usize) {
        let mut leader_blanks = 0;
        let mut trailer_blanks = 0;
        let buf_size = buf.len();

        // count left spaces
        let mut i = 0;
        while i < buf_size {
            if buf[i] == 0 {
                leader_blanks += 1;
            } else {
                break;
            }
            i += 1;
        }

        if i == buf_size {
            return (leader_blanks, trailer_blanks);
        }

        // count right spaces
        let mut i = buf_size - 1;
        loop {
            if buf[i] == 0 {
                trailer_blanks += 1;
            } else {
                break;
            }
            // (the C++ loop `i >= 0` on a size_t never ends by itself; a non-zero byte exists here)
            i -= 1;
        }
        (leader_blanks, trailer_blanks)
    }

    // ---- accessors :347-417, :559-569 -----------------------------------------------------------

    /// `GetResolution` (:347-351).
    pub fn get_resolution(&self) -> Resolution {
        self.resolution
    }
    /// `GetDensity` (:353-357).
    pub fn get_density(&self) -> Density {
        self.density
    }
    /// `GetQuality` (:359-363).
    pub fn get_quality(&self) -> Quality {
        self.quality
    }
    /// `GetPageHeight` (:365-369).
    pub fn get_page_height(&self) -> usize {
        self.page_height
    }
    /// `GetPaperType` (:371-375).
    pub fn get_paper_type(&self) -> PaperType {
        self.paper_type
    }
    /// `SetResolution` (:377-381).
    pub fn set_resolution(&mut self, value: Resolution) {
        self.resolution = value;
    }
    /// `SetDensity` (:383-387).
    pub fn set_density(&mut self, value: Density) {
        self.density = value;
    }
    /// `SetQuality` (:389-393).
    pub fn set_quality(&mut self, value: Quality) {
        self.quality = value;
    }
    /// `SetPageHeight` (:395-399).
    pub fn set_page_height(&mut self, value: usize) {
        self.page_height = value;
    }
    /// `SetPaperType` (:401-405).
    pub fn set_paper_type(&mut self, value: PaperType) {
        self.paper_type = value;
    }
    /// `SetMaxPrintWidth` (:407-411).
    pub fn set_max_print_width(&mut self, value: usize) {
        self.max_print_width = value;
    }
    /// `SetPageOffset` (:413-417).
    pub fn set_page_offset(&mut self, value: Point) {
        self.page_offset = value;
    }
    /// `GetEmptyLinesCount` (:559-563); ported, the filter does not call it.
    #[allow(dead_code)]
    pub fn get_empty_lines_count(&self) -> usize {
        self.empty_lines_count
    }
    /// `SetEmptyLinesCount` (:565-569); ported, the filter does not call it.
    #[allow(dead_code)]
    pub fn set_empty_lines_count(&mut self, value: usize) {
        self.empty_lines_count = value;
    }

    // ---- :335-345 -------------------------------------------------------------------------------

    /// `SendCommand` (:335-339) and `SendCommand` (:341-345): both overloads.
    fn send_command(&mut self, buf: &[u8]) {
        self.environment.borrow_mut().write_data(buf);
    }

    // ---- :419-543 -------------------------------------------------------------------------------

    /// `SendLineTab` (:419-427): `ESC Q hi lo`.
    fn send_line_tab(&mut self, value: usize) {
        let buf = [ESC, b'Q', ((value >> 8) & 0xff) as u8, (value & 0xff) as u8];
        self.send_command(&buf);
    }

    /// `SendDotTab` (:429-436): `ESC B n`, `n` the low byte.
    fn send_dot_tab(&mut self, value: usize) {
        let buf = [ESC, b'B', value as u8];
        self.send_command(&buf);
    }

    /// `SendFormFeed` (:438-444): `ESC E`.
    fn send_form_feed(&mut self) {
        self.send_command(&[ESC, b'E']);
    }

    /// `SendBytesPerLine` (:446-453): `ESC D n`, `n` the low byte.
    fn send_bytes_per_line(&mut self, value: usize) {
        let buf = [ESC, b'D', value as u8];
        self.send_command(&buf);
    }

    /// `SendSkipLines` (:455-478): "a hardware can skip no more 255 lines at time", `ESC f 1 n`.
    fn send_skip_lines(&mut self, mut value: usize) {
        const MAX_LINES: usize = 255;
        let mut buf = [ESC, b'f', 1, 0];
        while value > 0 {
            if value > MAX_LINES {
                buf[3] = MAX_LINES as u8;
                value -= MAX_LINES;
            } else {
                buf[3] = value as u8;
                value = 0;
            }
            self.send_command(&buf);
        }
    }

    /// `SendLabelLength` (:480-488): `ESC L hi lo`.
    fn send_label_length(&mut self, value: usize) {
        let buf = [ESC, b'L', ((value >> 8) & 0xff) as u8, (value & 0xff) as u8];
        self.send_command(&buf);
    }

    /// `SendResolution` (:490-511): nothing for `resUnknown`, else `ESC z` (136) or `ESC y` (204).
    fn send_resolution(&mut self, value: Resolution) {
        if value == Resolution::Unknown {
            return;
        }
        let second = match value {
            Resolution::Res136 => b'z',
            Resolution::Res204 => b'y',
            Resolution::Unknown => unreachable!("assert(0)"),
        };
        self.send_command(&[ESC, second]);
    }

    /// `SendPrintDensity` (:513-528): `ESC c|d|e|g`.
    fn send_print_density(&mut self, value: Density) {
        let second = match value {
            Density::Low => b'c',
            Density::Medium => b'd',
            Density::Normal => b'e',
            Density::High => b'g',
        };
        self.send_command(&[ESC, second]);
    }

    /// `SendPrintQuality` (:530-543): `ESC h` (text) or `ESC i` (barcode and graphics).
    fn send_print_quality(&mut self, value: Quality) {
        let second = match value {
            Quality::Text => b'h',
            Quality::BarcodeAndGraphics => b'i',
        };
        self.send_command(&[ESC, second]);
    }

    /// `GetResetCommand` (:545-549): 156 `ESC`.
    pub fn get_reset_command() -> Buffer {
        vec![ESC; 156]
    }

    /// `GetRequestStatusCommand` (:551-557): `ESC A`.
    pub fn get_request_status_command() -> Buffer {
        vec![ESC, b'A']
    }

    /// `CLabelWriterDriver400::GetShortFormFeedCommand` (:602-608): `ESC G`.
    pub fn get_short_form_feed_command() -> Buffer {
        vec![ESC, b'G']
    }

    /// `CLabelWriterDriver400::SendShortFormFeed` (:610-616).
    fn send_short_form_feed(&mut self) {
        self.send_command(&[ESC, b'G']);
    }

    /// `CLabelWriterDriver::StartDoc` (:46-55).
    fn base_start_doc(&mut self) {
        self.send_command(&Self::get_reset_command());
        let r = self.resolution;
        self.send_resolution(r);
        self.send_line_tab(0);
        self.send_dot_tab(0);
        let q = self.quality;
        self.send_print_quality(q);
        let d = self.density;
        self.send_print_density(d);
    }
}

// ---- :146-332 ---------------------------------------------------------------------------------

/// `GetBitValue` (:154-158): "Returns Value of bit BitNo in byte Data; if bit unset returns 0, else not 0".
fn get_bit_value(data: u8, bit_no: usize) -> u8 {
    data & (1u32 << bit_no) as u8
}

/// `NextBit` (:161-171): "Advanses to one bit in byte sequence".
fn next_bit(cur_byte_no: &mut usize, cur_bit_no: &mut usize) {
    if *cur_bit_no == 0 {
        *cur_byte_no += 1;
        *cur_bit_no = 7;
    } else {
        *cur_bit_no -= 1;
    }
}

/// `GetCompressedSequenceValue` (:173-203).
fn get_compressed_sequence_value(
    data: &[u8],
    data_len: usize,
    cur_byte_no: &mut usize,
    cur_bit_no: &mut usize,
) -> u8 {
    let mut bit_count: u8 = 0;
    let start_bit_value = get_bit_value(data[*cur_byte_no], *cur_bit_no);
    next_bit(cur_byte_no, cur_bit_no);

    // while data exist and not max len of sequence
    while *cur_byte_no < data_len && bit_count < 0x7f {
        let bit_value = get_bit_value(data[*cur_byte_no], *cur_bit_no);

        // same as prev bit
        if (start_bit_value != 0 && bit_value != 0) || (start_bit_value == 0 && bit_value == 0) {
            bit_count += 1;
            next_bit(cur_byte_no, cur_bit_no);
        } else {
            // end of sequence
            break;
        }
    }

    if start_bit_value != 0 {
        bit_count | 0x80 // set high bit for "black" pixels
    } else {
        bit_count
    }
}

/// `CompressData` (:205-222).
fn compress_data(compressed_data: &mut Buffer, data: &[u8], data_size: usize) {
    let mut cur_byte_no = 0;
    let mut cur_bit_no = 7;
    let mut compressed_offset = 0usize;

    while cur_byte_no < data_size {
        if compressed_offset >= data_size.wrapping_sub(1) {
            compressed_data.clear(); // will write non-compressed data
            return;
        }
        compressed_data.push(get_compressed_sequence_value(
            data,
            data_size,
            &mut cur_byte_no,
            &mut cur_bit_no,
        ));
        compressed_offset += 1;
    }
}

/// `ShiftDataRight` (:224-241).
fn shift_data_right(buf: &[u8], shifted_buf: &mut [u8], mut shift_value: usize) {
    // shift bytes first
    let shifted_len = shifted_buf.len() as i64 - (shift_value / 8) as i64;
    let shifted_offset = shift_value / 8;
    shift_value %= 8;

    if shifted_len <= 0 || buf.is_empty() {
        return;
    }
    let shifted_len = shifted_len as usize;

    // shift bits
    shifted_buf[shifted_offset] = buf[0] >> shift_value; // first
    let mut i = 1;
    while i < buf.len() && i < shifted_len {
        shifted_buf[shifted_offset + i] =
            (((buf[i - 1] as u32) << (8 - shift_value)) | ((buf[i] as u32) >> shift_value)) as u8;
        i += 1;
    }
    if i < shifted_len {
        shifted_buf[shifted_offset + buf.len()] =
            ((buf[buf.len() - 1] as u32) << (8 - shift_value)) as u8;
    }
}

/// `ShiftDataLeft` (:243-258). Ported; `ProcessRasterLine` only shifts right (`PageOffset_.x > 0`).
#[allow(dead_code)]
fn shift_data_left(buf: &[u8], shifted_buf: &mut [u8], mut shift_value: usize) {
    // shift bytes first
    let shifted_len = shifted_buf.len() as i64 - (shift_value / 8) as i64;
    shift_value %= 8;

    if shifted_len <= 0 || buf.is_empty() {
        return;
    }
    let shifted_len = shifted_len as usize;

    // shift bits
    let mut i = 0;
    while i < buf.len() - 1 && i < shifted_len {
        shifted_buf[i] =
            (((buf[i] as u32) << shift_value) | ((buf[i + 1] as u32) >> (8 - shift_value))) as u8;
        i += 1;
    }
    if i < shifted_len {
        shifted_buf[buf.len() - 1] = ((buf[buf.len() - 1] as u32) << shift_value) as u8; // last
    }
}

/// `ShiftData` (:261-272).
fn shift_data(buf: &[u8], shifted_buf: &mut [u8], shift_value: i32) {
    // clear shift buffer first
    for b in shifted_buf.iter_mut() {
        *b = 0;
    }

    if shift_value >= 0 {
        shift_data_right(buf, shifted_buf, shift_value as usize);
    } else {
        shift_data_left(buf, shifted_buf, (-shift_value) as usize);
    }
}

impl PrinterDriver for LabelWriterDriver {
    /// `CLabelWriterDriver::StartDoc` (:46-55); `CLabelWriterDriver400::StartDoc` (:584-588) calls it.
    fn start_doc(&mut self) {
        self.base_start_doc();
    }

    /// `CLabelWriterDriver::EndDoc` (:57-60), nothing; `CLabelWriterDriver400::EndDoc` (:590-594) sends
    /// the form feed.
    fn end_doc(&mut self) {
        if self.class == Class::Driver400 {
            self.send_form_feed();
        }
    }

    /// `CLabelWriterDriver::StartPage` (:62-75).
    fn start_page(&mut self) {
        match self.paper_type {
            PaperType::Regular => {
                let h = self.page_height;
                self.send_label_length(h)
            }
            PaperType::Continuous => self.send_label_length(0xffff),
        }

        self.last_dot_tab = SIZE_T_MINUS_1;
        self.last_bytes_per_line = SIZE_T_MINUS_1;
        self.empty_lines_count = 0;
    }

    /// `CLabelWriterDriver::EndPage` (:77-81), the form feed; `CLabelWriterDriver400::EndPage`
    /// (:596-600), the short form feed.
    fn end_page(&mut self) {
        match self.class {
            Class::Driver => self.send_form_feed(),
            Class::Driver400 => self.send_short_form_feed(),
        }
    }

    /// `CLabelWriterDriver::ProcessRasterLine` (:274-332).
    fn process_raster_line(&mut self, line_buffer: &[u8]) {
        let mut b: Buffer = line_buffer.to_vec();

        if self.page_offset.x > 0 {
            let x = self.page_offset.x as usize;
            let mut b2: Buffer = vec![0; b.len() + (x + 7) / 8];
            shift_data(&b, &mut b2, self.page_offset.x);
            b = b2;
        }

        if b.len() > self.max_print_width {
            stderr::write(
                "WARNING: CLabelWriterDriver::ProcessRasterLine(): page width is greater max page width, truncated\n",
            );
            b.truncate(self.max_print_width);
        }

        // get blanks count
        let (leader_blanks, trailer_blanks) = Self::get_blanks(&b);

        if leader_blanks + trailer_blanks == b.len() {
            // remember empty line
            self.empty_lines_count += 1;
        } else {
            // not empty line
            // skip empty lines
            if self.empty_lines_count != 0 {
                let n = self.empty_lines_count;
                self.send_skip_lines(n);
            }

            self.empty_lines_count = 0;

            // set dot tab
            // Bug Fix for DLS80AM-1421
            // NOTE: an ESC B needs to be send for each raster line. Otherwise the LW 3xx series output
            // will be distorted.
            self.send_dot_tab(leader_blanks);
            self.last_dot_tab = leader_blanks;

            // calculate compressed data size
            let mut compressed_data = Buffer::new();
            let data_size = b.len() - leader_blanks - trailer_blanks;
            compress_data(&mut compressed_data, &b[leader_blanks..], data_size);

            if !compressed_data.is_empty() && compressed_data.len() < data_size {
                self.send_compressed_data(&compressed_data, data_size);
            } else {
                self.send_not_compressed_data(&b, leader_blanks, trailer_blanks);
            }
        }
    }
}
