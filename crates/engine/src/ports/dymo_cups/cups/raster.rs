// Ported from CUPS 2.4.20 (reference/cups-2.4.20/cups/raster-interpret.c, raster-stream.c, raster-stubs.c,
// raster.h). Copyright © 2020-2025 by OpenPrinting, 2007-2019 by Apple Inc. Apache-2.0 with the GPL/LGPL
// exception (reference/cups-2.4.20/LICENSE).

//! The raster side of CUPS the filter uses: `cups_page_header2_t`, `cupsRasterInterpretPPD` (how CUPS makes
//! a page header from a PPD and its marked options), `cupsRasterReadHeader2` and `cupsRasterReadPixels` over
//! a page held in memory (W10; the stream's file format is not ported).

use super::options::{cups_get_option, CupsOption};
use super::ppd::PpdFile;
use std::collections::VecDeque;

/// `raster.h:123-178` `cups_cspace_t` (the values `cups_raster_update` and the interpreter switch on).
pub const CUPS_CSPACE_W: u32 = 0;
pub const CUPS_CSPACE_RGB: u32 = 1;
pub const CUPS_CSPACE_RGBA: u32 = 2;
pub const CUPS_CSPACE_K: u32 = 3;
pub const CUPS_CSPACE_CMY: u32 = 4;
pub const CUPS_CSPACE_YMC: u32 = 5;
pub const CUPS_CSPACE_CMYK: u32 = 6;
pub const CUPS_CSPACE_YMCK: u32 = 7;
pub const CUPS_CSPACE_KCMY: u32 = 8;
pub const CUPS_CSPACE_KCMYCM: u32 = 9;
pub const CUPS_CSPACE_GMCK: u32 = 10;
pub const CUPS_CSPACE_GMCS: u32 = 11;
pub const CUPS_CSPACE_WHITE: u32 = 12;
pub const CUPS_CSPACE_GOLD: u32 = 13;
pub const CUPS_CSPACE_SILVER: u32 = 14;
pub const CUPS_CSPACE_CIEXYZ: u32 = 15;
pub const CUPS_CSPACE_CIELAB: u32 = 16;
pub const CUPS_CSPACE_RGBW: u32 = 17;
pub const CUPS_CSPACE_SW: u32 = 18;
pub const CUPS_CSPACE_SRGB: u32 = 19;
pub const CUPS_CSPACE_ADOBERGB: u32 = 20;
pub const CUPS_CSPACE_ICC1: u32 = 32;
pub const CUPS_CSPACE_ICCF: u32 = 46;
pub const CUPS_CSPACE_DEVICE1: u32 = 48;
pub const CUPS_CSPACE_DEVICEF: u32 = 62;

/// `raster.h:219-223` `cups_order_t`.
pub const CUPS_ORDER_CHUNKED: u32 = 0;
pub const CUPS_ORDER_BANDED: u32 = 1;
pub const CUPS_ORDER_PLANAR: u32 = 2;

/// `raster-stream.c:29-33`.
const CUPS_MAX_BYTES_PER_LINE: u32 = 16 * 1024 * 1024;
const CUPS_MAX_HEIGHT: u32 = 0x00ffffff;
const CUPS_MAX_WIDTH: u32 = 0x00ffffff;

/// `cups_page_header2_t` (`raster.h`): the fields the interpreter sets, the stream checks and the driver
/// reads. (The others, `MediaClass`, `MediaColor`, `Duplex`, ..., are never set from this PPD or read.)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PageHeader {
    pub num_copies: u32,
    pub orientation: u32,
    pub page_size: [u32; 2],
    pub hw_resolution: [u32; 2],
    pub margins: [u32; 2],
    pub imaging_bounding_box: [u32; 4],
    pub cups_width: u32,
    pub cups_height: u32,
    pub cups_media_type: u32,
    pub cups_bits_per_color: u32,
    pub cups_bits_per_pixel: u32,
    pub cups_bytes_per_line: u32,
    pub cups_color_order: u32,
    pub cups_color_space: u32,
    pub cups_num_colors: u32,
    pub cups_integer: [u32; 16],
    pub cups_borderless_scaling_factor: f32,
    pub cups_page_size: [f32; 2],
    pub cups_imaging_bbox: [f32; 4],
    pub cups_page_size_name: String,
}

// ---- raster-interpret.c ---------------------------------------------------------------------------

/// A value of a `setpagedevice` dictionary (`_cups_ps_obj_t`, the types the PPD's codes use).
#[derive(Debug, Clone, PartialEq)]
enum PsObj {
    Name(String),
    Number(f64),
    Boolean(bool),
    Null,
    StartArray,
    EndArray,
    Str(String),
}

/// `scan_ps` (`raster-interpret.c`), for one `<</Name value ... >>setpagedevice` as a PPD choice has it:
/// the objects between `<<` and `>>`. `None` for code that is not that shape.
fn scan_dict(code: &str) -> Option<Vec<PsObj>> {
    let b = code.as_bytes();
    let mut i = 0;
    let mut objs = Vec::new();
    let skip_ws = |i: &mut usize| {
        while *i < b.len() && b[*i].is_ascii_whitespace() {
            *i += 1;
        }
    };
    skip_ws(&mut i);
    if !code[i..].starts_with("<<") {
        return None;
    }
    i += 2;
    loop {
        skip_ws(&mut i);
        if i >= b.len() {
            return None;
        }
        if code[i..].starts_with(">>") {
            return Some(objs);
        }
        match b[i] {
            b'/' => {
                let s = i + 1;
                i += 1;
                while i < b.len() && !b[i].is_ascii_whitespace() && !b"[]<>/(){}%".contains(&b[i]) {
                    i += 1;
                }
                objs.push(PsObj::Name(code[s..i].to_owned()));
            }
            b'[' => {
                objs.push(PsObj::StartArray);
                i += 1;
            }
            b']' => {
                objs.push(PsObj::EndArray);
                i += 1;
            }
            b'(' => {
                let s = i + 1;
                while i < b.len() && b[i] != b')' {
                    i += 1;
                }
                objs.push(PsObj::Str(code[s..i].to_owned()));
                i += 1;
            }
            _ => {
                let s = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && !b"[]<>/(){}%".contains(&b[i]) {
                    i += 1;
                }
                if s == i {
                    return None;
                }
                let tok = &code[s..i];
                match tok {
                    "null" => objs.push(PsObj::Null),
                    "true" => objs.push(PsObj::Boolean(true)),
                    "false" => objs.push(PsObj::Boolean(false)),
                    _ => objs.push(PsObj::Number(tok.parse::<f64>().ok()?)),
                }
            }
        }
    }
}

/// `_cupsRasterExecPS` for the `setpagedevice` of a choice's code, and `setpagedevice()`
/// (`raster-interpret.c:1420-1650`): the dictionary's names and values set the header. 0, or -1.
fn exec_ps(h: &mut PageHeader, code: &str) -> i32 {
    // the code is `<<...>>setpagedevice`
    let Some(objs) = scan_dict(code) else { return -1 };

    let mut n = 0;
    while n < objs.len() {
        // "Grab the name..."
        let PsObj::Name(name) = &objs[n] else { return -1 };
        let name = name.as_str();
        n += 1;
        let obj = objs.get(n);

        let num = |o: Option<&PsObj>| -> Option<f64> {
            match o {
                Some(PsObj::Number(v)) => Some(*v),
                _ => None,
            }
        };

        // "Then grab the value..."
        if name == "HWResolution" && matches!(obj, Some(PsObj::StartArray)) {
            match (num(objs.get(n + 1)), num(objs.get(n + 2)), objs.get(n + 3)) {
                (Some(a), Some(b), Some(PsObj::EndArray)) => {
                    h.hw_resolution[0] = a as u32;
                    h.hw_resolution[1] = b as u32;
                    n += 3;
                }
                _ => return -1,
            }
        } else if name == "PageSize" && matches!(obj, Some(PsObj::StartArray)) {
            match (num(objs.get(n + 1)), num(objs.get(n + 2)), objs.get(n + 3)) {
                (Some(a), Some(b), Some(PsObj::EndArray)) => {
                    h.cups_page_size[0] = a as f32;
                    h.cups_page_size[1] = b as f32;
                    h.page_size[0] = a as u32;
                    h.page_size[1] = b as u32;
                    n += 3;
                }
                _ => return -1,
            }
        } else if name == "Orientation" && num(obj).is_some() {
            h.orientation = num(obj).unwrap() as u32;
        } else if name == "NumCopies" && num(obj).is_some() {
            h.num_copies = num(obj).unwrap() as u32;
        } else if name == "cupsMediaType" && num(obj).is_some() {
            h.cups_media_type = num(obj).unwrap() as u32;
        } else if name == "cupsBitsPerColor" && num(obj).is_some() {
            h.cups_bits_per_color = num(obj).unwrap() as u32;
        } else if name == "cupsColorOrder" && num(obj).is_some() {
            h.cups_color_order = num(obj).unwrap() as u32;
        } else if name == "cupsColorSpace" && num(obj).is_some() {
            h.cups_color_space = num(obj).unwrap() as u32;
        } else if name == "cupsBorderlessScalingFactor" && num(obj).is_some() {
            h.cups_borderless_scaling_factor = num(obj).unwrap() as f32;
        } else if name.starts_with("cupsInteger") && num(obj).is_some() {
            let i: i32 = name[11..].chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
            if !(0..=15).contains(&i) {
                return -1;
            }
            h.cups_integer[i as usize] = num(obj).unwrap() as u32;
        } else {
            // "Ignore unknown name+value..." (the other fields of the header are not carried here, and
            // `ImagingBBox null`, `cupsBitsPerPixel` and the like are unknown to the interpreter too)
            while n + 1 < objs.len() && !matches!(objs[n + 1], PsObj::Name(_)) {
                n += 1;
            }
        }
        n += 1;
    }

    0
}

/// `cupsRasterInterpretPPD` / `_cupsRasterInterpretPPD` (`raster-interpret.c:125-475`): "Create a page header
/// from a PPD file and options". `ppd`'s marked options are those of the job (`ppdMarkDefaults`,
/// `cupsMarkOptions`). Returns the header, or the error CUPS would add (`-1`).
pub fn cups_raster_interpret_ppd(ppd: &mut PpdFile, options: &[CupsOption]) -> Result<PageHeader, String> {
    // "Reset the page header to the defaults..." (:155-185); `#ifdef __APPLE__ cupsInteger[0] = 0x80000000`
    // is not taken (a Linux build; the PPD sets `cupsInteger0` for every size anyway).
    let mut h = PageHeader {
        num_copies: 1,
        page_size: [612, 792],
        hw_resolution: [100, 100],
        cups_bits_per_color: 1,
        cups_color_order: CUPS_ORDER_CHUNKED,
        cups_color_space: CUPS_CSPACE_K,
        cups_borderless_scaling_factor: 1.0,
        cups_page_size: [612.0, 792.0],
        cups_imaging_bbox: [0.0, 0.0, 612.0, 792.0],
        cups_page_size_name: "Letter".to_owned(),
        ..PageHeader::default()
    };

    // "Apply patches and options to the page header..." (:187-240): the PPD has no `*cupsPreFilter`-like
    // patches; each section's code from `ppdEmitString`: DOCUMENT, ANY, PROLOG, PAGE (the 450's options are
    // all `AnySetup`).
    let mut status = 0;
    for code in ppd.emit_codes_any() {
        status |= exec_ps(&mut h, code);
    }
    let _ = status;

    // "Allow option override for page scaling..." (:242-253)
    if let Some(val) = cups_get_option("cupsBorderlessScalingFactor", options) {
        let sc: f64 = val.parse().unwrap_or(0.0);
        if (0.1..=2.0).contains(&sc) {
            h.cups_borderless_scaling_factor = sc as f32;
        }
    }

    // "Get the margins for the current size..." (:255-285)
    let (mut left, mut bottom, mut right, mut top);
    if let Some(size) = ppd.page_size(None) {
        left = size.left;
        bottom = size.bottom;
        right = size.right;
        top = size.top;
        h.cups_page_size_name = size.name.to_owned();
        h.cups_page_size[0] = size.width;
        h.cups_page_size[1] = size.length;
    } else {
        left = 0.0;
        bottom = 0.0;
        right = 612.0;
        top = 792.0;
    }

    // "Handle orientation..." (:287-327)
    match h.orientation {
        1 => {
            // CUPS_ORIENT_90
            let temp1 = h.cups_page_size[0];
            h.cups_page_size[0] = h.cups_page_size[1];
            h.cups_page_size[1] = temp1;

            let (t1, t2) = (left, right);
            left = h.cups_page_size[0] - top;
            right = h.cups_page_size[0] - bottom;
            bottom = h.cups_page_size[1] - t1;
            top = h.cups_page_size[1] - t2;
        }
        2 => {
            // CUPS_ORIENT_180
            let (t1, t2) = (left, bottom);
            left = h.cups_page_size[0] - right;
            right = h.cups_page_size[0] - t1;
            bottom = h.cups_page_size[1] - top;
            top = h.cups_page_size[1] - t2;
        }
        3 => {
            // CUPS_ORIENT_270
            let temp1 = h.cups_page_size[0];
            h.cups_page_size[0] = h.cups_page_size[1];
            h.cups_page_size[1] = temp1;

            let (t1, t2) = (left, right);
            left = bottom;
            right = top;
            bottom = h.cups_page_size[1] - t2;
            top = h.cups_page_size[1] - t1;
        }
        _ => {}
    }

    if left > right {
        std::mem::swap(&mut left, &mut right);
    }

    if bottom > top {
        std::mem::swap(&mut bottom, &mut top);
    }

    let sf = h.cups_borderless_scaling_factor;
    h.page_size[0] = (h.cups_page_size[0] * sf) as u32;
    h.page_size[1] = (h.cups_page_size[1] * sf) as u32;
    h.margins[0] = (left * sf) as u32;
    h.margins[1] = (bottom * sf) as u32;
    h.imaging_bounding_box[0] = (left * sf) as u32;
    h.imaging_bounding_box[1] = (bottom * sf) as u32;
    h.imaging_bounding_box[2] = (right * sf) as u32;
    h.imaging_bounding_box[3] = (top * sf) as u32;
    h.cups_imaging_bbox = [left, bottom, right, top];

    // "Check parameters..." (:373-384)
    if h.hw_resolution[0] == 0
        || h.hw_resolution[1] == 0
        || h.page_size[0] == 0
        || h.page_size[1] == 0
        || !matches!(h.cups_bits_per_color, 1 | 2 | 4 | 8 | 16)
        || h.cups_borderless_scaling_factor < 0.1
        || h.cups_borderless_scaling_factor > 2.0
    {
        return Err("Page header uses unsupported values.\n".to_owned());
    }

    // "Compute the bitmap parameters..." (:386-395)
    h.cups_width = ((right - left) * sf * h.hw_resolution[0] as f32 / 72.0f32 + 0.5f32) as u32;
    h.cups_height = ((top - bottom) * sf * h.hw_resolution[1] as f32 / 72.0f32 + 0.5f32) as u32;

    match h.cups_color_space {
        CUPS_CSPACE_W | CUPS_CSPACE_K | CUPS_CSPACE_WHITE | CUPS_CSPACE_GOLD | CUPS_CSPACE_SILVER | CUPS_CSPACE_SW => {
            h.cups_num_colors = 1;
            h.cups_bits_per_pixel = h.cups_bits_per_color;
        }
        CUPS_CSPACE_KCMYCM if h.cups_bits_per_color == 1 => {
            h.cups_bits_per_pixel = if h.cups_color_order == CUPS_ORDER_CHUNKED { 8 } else { 1 };
            h.cups_num_colors = 6;
        }
        CUPS_CSPACE_KCMYCM | CUPS_CSPACE_RGBA | CUPS_CSPACE_RGBW | CUPS_CSPACE_CMYK | CUPS_CSPACE_YMCK | CUPS_CSPACE_KCMY | CUPS_CSPACE_GMCK | CUPS_CSPACE_GMCS => {
            h.cups_bits_per_pixel = if h.cups_color_order == CUPS_ORDER_CHUNKED { h.cups_bits_per_color * 4 } else { h.cups_bits_per_color };
            h.cups_num_colors = 4;
        }
        CUPS_CSPACE_DEVICE1..=CUPS_CSPACE_DEVICEF => {
            h.cups_num_colors = h.cups_color_space - CUPS_CSPACE_DEVICE1 + 1;
            h.cups_bits_per_pixel = if h.cups_color_order == CUPS_ORDER_CHUNKED { h.cups_bits_per_color * h.cups_num_colors } else { h.cups_bits_per_color };
        }
        _ => {
            // :407-431 `default`: RGB, CMY, YMC, CIE, ICC, ... "Ensure that colorimetric colorspaces use at
            // least 8 bits per component..."
            if h.cups_color_space >= CUPS_CSPACE_CIEXYZ && h.cups_bits_per_color < 8 {
                h.cups_bits_per_color = 8;
            }

            // "Figure out the number of bits per pixel..."
            if h.cups_color_order == CUPS_ORDER_CHUNKED {
                if h.cups_bits_per_color >= 8 {
                    h.cups_bits_per_pixel = h.cups_bits_per_color * 3;
                } else {
                    h.cups_bits_per_pixel = h.cups_bits_per_color * 4;
                }
            } else {
                h.cups_bits_per_pixel = h.cups_bits_per_color;
            }

            h.cups_num_colors = 3;
        }
    }

    h.cups_bytes_per_line = (h.cups_bits_per_pixel * h.cups_width + 7) / 8;

    if h.cups_color_order == CUPS_ORDER_BANDED {
        h.cups_bytes_per_line *= h.cups_num_colors;
    }

    Ok(h)
}

// ---- raster-stream.c, raster-stubs.c ----------------------------------------------------------------

/// A page of a raster stream: its header and its pixels, line after line, `cupsBytesPerLine` each.
#[derive(Debug, Clone, PartialEq)]
pub struct RasterPage {
    pub header: PageHeader,
    pub pixels: Vec<u8>,
}

/// `cups_raster_t` for reading (`raster-private.h`), over pages in memory (W10).
pub struct CupsRaster {
    pages: VecDeque<RasterPage>,
    pixels: Vec<u8>,
    pos: usize,
    /// `r->remaining`: lines left of the current page.
    remaining: u32,
    /// `r->header`.
    header: PageHeader,
    /// `r->bpp`: bytes per pixel.
    bpp: u32,
    /// `_cupsRasterAddError`'s text, as far as this raster collected it.
    pub errors: String,
}

impl CupsRaster {
    /// `cupsRasterOpen(fd, CUPS_RASTER_READ)`, over these pages.
    pub fn open(pages: Vec<RasterPage>) -> CupsRaster {
        CupsRaster { pages: pages.into(), pixels: Vec::new(), pos: 0, remaining: 0, header: PageHeader::default(), bpp: 0, errors: String::new() }
    }

    fn add_error(&mut self, text: String) {
        self.errors.push_str(&text);
    }

    /// `cups_raster_update` (`raster-stream.c:1573-1757`): "Update the raster header and row count for the
    /// current page"; "Validate the page header". (v2/v3 streams, uncompressed: no pixel buffer to make.)
    fn cups_raster_update(&mut self) -> bool {
        let mut ret = true;
        let h = &mut self.header;

        // "Set the 'cupsNumColors' field according to the colorspace..." only for v1 streams or when it is 0
        if h.cups_num_colors == 0 {
            match h.cups_color_space {
                CUPS_CSPACE_W | CUPS_CSPACE_K | CUPS_CSPACE_WHITE | CUPS_CSPACE_GOLD | CUPS_CSPACE_SILVER | CUPS_CSPACE_SW => h.cups_num_colors = 1,
                CUPS_CSPACE_RGB | CUPS_CSPACE_CMY | CUPS_CSPACE_YMC | CUPS_CSPACE_CIEXYZ | CUPS_CSPACE_CIELAB | CUPS_CSPACE_SRGB | CUPS_CSPACE_ADOBERGB | CUPS_CSPACE_ICC1..=CUPS_CSPACE_ICCF => h.cups_num_colors = 3,
                CUPS_CSPACE_RGBA | CUPS_CSPACE_RGBW | CUPS_CSPACE_CMYK | CUPS_CSPACE_YMCK | CUPS_CSPACE_KCMY | CUPS_CSPACE_GMCK | CUPS_CSPACE_GMCS => h.cups_num_colors = 4,
                CUPS_CSPACE_KCMYCM => h.cups_num_colors = if h.cups_bits_per_pixel < 8 { 6 } else { 4 },
                CUPS_CSPACE_DEVICE1..=CUPS_CSPACE_DEVICEF => h.cups_num_colors = h.cups_color_space - CUPS_CSPACE_DEVICE1 + 1,
                _ => return false, // "Unknown color space"
            }
        }

        // "Set the number of bytes per pixel/color..."
        self.bpp = if h.cups_color_order == CUPS_ORDER_CHUNKED { (h.cups_bits_per_pixel + 7) / 8 } else { (h.cups_bits_per_color + 7) / 8 };

        if self.bpp == 0 {
            self.bpp = 1;
        }

        // "Set the number of remaining rows..."
        self.remaining = if h.cups_color_order == CUPS_ORDER_PLANAR { h.cups_height * h.cups_num_colors } else { h.cups_height };

        // "Validate the page header..."
        let mut errs: Vec<String> = Vec::new();
        if !matches!(h.cups_bits_per_color, 1 | 2 | 4 | 8 | 16) {
            errs.push(format!("Invalid bits per color {}.", h.cups_bits_per_color));
            ret = false;
        }

        if (h.cups_color_order != CUPS_ORDER_CHUNKED && h.cups_bits_per_pixel != h.cups_bits_per_color)
            || (h.cups_color_order == CUPS_ORDER_CHUNKED && h.cups_bits_per_pixel != h.cups_bits_per_color * h.cups_num_colors)
        {
            errs.push(format!("Invalid bits per pixel {}.", h.cups_bits_per_pixel));
            ret = false;
        }

        let mut bytes_per_line = (h.cups_width.wrapping_mul(h.cups_bits_per_pixel) + 7) / 8;
        if h.cups_color_order == CUPS_ORDER_BANDED {
            bytes_per_line *= h.cups_num_colors;
        }

        if h.cups_bytes_per_line == 0 {
            errs.push("Invalid raster line length 0.".to_owned());
            ret = false;
        } else if h.cups_bytes_per_line > CUPS_MAX_BYTES_PER_LINE {
            errs.push(format!("Raster line length {} is greater than {} bytes.", h.cups_bytes_per_line, CUPS_MAX_BYTES_PER_LINE));
            ret = false;
        } else if h.cups_bytes_per_line % self.bpp != 0 {
            errs.push(format!("Raster line length {} is not a multiple of the pixel size ({}).", h.cups_bytes_per_line, self.bpp));
            ret = false;
        } else if h.cups_bytes_per_line != bytes_per_line {
            errs.push(format!(
                "Raster line length {} does not match width ({}) and bits per pixel ({}).",
                h.cups_bytes_per_line, h.cups_width, h.cups_bits_per_pixel
            ));
            ret = false;
        }

        if h.cups_width == 0 || h.cups_width > CUPS_MAX_WIDTH {
            errs.push(format!("Invalid raster width {}.", h.cups_width));
            ret = false;
        }

        if h.cups_height == 0 || h.cups_height > CUPS_MAX_HEIGHT {
            errs.push(format!("Invalid raster height {}.", h.cups_height));
            ret = false;
        }

        for e in errs {
            self.add_error(e);
        }
        ret
    }

    /// `_cupsRasterReadHeader` (`raster-stream.c:593-745`) with `cupsRasterReadHeader2` (`raster-stubs.c:
    /// 192-212`): the next page's header, or `None` ("memset(h, 0, ...); return 0") at the end or for a
    /// header that does not pass.
    pub fn cups_raster_read_header2(&mut self) -> Option<PageHeader> {
        let page = self.pages.pop_front()?;
        self.header = page.header;
        self.pixels = page.pixels;
        self.pos = 0;

        // "Update the header and row count..."
        if !self.cups_raster_update() {
            self.header = PageHeader::default();
            return None;
        }

        let h = &self.header;
        let ok = h.cups_bits_per_pixel > 0
            && h.cups_bits_per_pixel <= 240
            && h.cups_bits_per_color > 0
            && h.cups_bits_per_color <= 16
            && h.cups_bytes_per_line > 0
            && h.cups_bytes_per_line <= 0x7fffffff
            && h.cups_height != 0
            && (h.cups_bytes_per_line % self.bpp) == 0;
        if !ok {
            self.header = PageHeader::default();
            return None;
        }
        Some(self.header.clone())
    }

    /// `_cupsRasterReadPixels` (`raster-stream.c:759-`) with `cupsRasterReadPixels` (`raster-stubs.c:
    /// 223-230`), uncompressed: "Read without compression...". Returns the bytes read, 0 for none.
    pub fn cups_raster_read_pixels(&mut self, p: &mut [u8]) -> u32 {
        let len = p.len() as u32;
        if self.remaining == 0 || self.header.cups_bytes_per_line == 0 {
            return 0;
        }

        self.remaining -= len / self.header.cups_bytes_per_line;

        // `cups_raster_io(r, p, len) < len`: the stream has no more
        if self.pos + p.len() > self.pixels.len() {
            return 0;
        }
        p.copy_from_slice(&self.pixels[self.pos..self.pos + p.len()]);
        self.pos += p.len();

        len
    }
}
