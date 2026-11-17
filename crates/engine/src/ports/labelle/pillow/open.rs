//! `Image.open` of a PNG (`Image.py:3585`, `PngImagePlugin.py`, `Unpack.c`).
//!
//! **Ours:** the `png` crate stands for Pillow's PNG decoder (zlib, filters, interlacing); what
//! Pillow does with the decoded rows is ported: the mode a PNG opens as (`_MODES`,
//! `PngImagePlugin.py:69`), the unpackers (`Unpack.c`: `unpackL2` 261, `unpackL4` 351,
//! `unpackP1` 480, `unpackP2` 514, `unpackP4` 536, `unpackRGB16B` 617, `unpackRGBALA16B` 859,
//! `unpackRGBA16B` 1057, `unpackLA` 418), and the decompression bomb check
//! (`Image.py:3564`). Transparency chunks (`tRNS`) do not change a pixel in Pillow's
//! `convert("L")` and are not kept. Only PNG opens: any other file is reported as Pillow reports
//! one it cannot identify.

use super::image::{Image, Mode};

/// `Image.MAX_IMAGE_PIXELS` (`Image.py:86`): `1024 * 1024 * 1024 // 4 // 3`.
const MAX_IMAGE_PIXELS: u64 = 1024 * 1024 * 1024 / 4 / 3;

/// Python's `repr()` of a `str`, for the messages that show a path.
pub fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// What `Image.open` raises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// `PIL.UnidentifiedImageError`.
    Unidentified(String),
    /// Any other (`DecompressionBombError`, a PNG that does not read).
    Other(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Unidentified(m) | OpenError::Other(m) => f.write_str(m),
        }
    }
}

/// `Image.open(filename)` of the file's bytes. `filename` is only for the message Pillow gives
/// for a file it cannot identify.
pub fn open(bytes: &[u8], filename: &str) -> Result<Image, OpenError> {
    // `PngImagePlugin._accept`: the PNG signature.
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(OpenError::Unidentified(format!(
            "cannot identify image file {}",
            py_repr(filename)
        )));
    }
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder
        .read_info()
        .map_err(|e| OpenError::Other(format!("the PNG does not read: {e}")))?;
    let (width, height) = (reader.info().width as usize, reader.info().height as usize);
    let (depth, color) = (reader.info().bit_depth as u8, reader.info().color_type);
    let palette_chunk = reader.info().palette.as_ref().map(|p| p.to_vec());
    // `_decompression_bomb_check`.
    let pixels = width.max(1) as u64 * height.max(1) as u64;
    if pixels > 2 * MAX_IMAGE_PIXELS {
        return Err(OpenError::Other(format!(
            "Image size ({pixels} pixels) exceeds limit of {} pixels, could be decompression bomb DOS attack.",
            2 * MAX_IMAGE_PIXELS
        )));
    }
    let mut buffer = vec![0; reader.output_buffer_size()];
    let frame = reader
        .next_frame(&mut buffer)
        .map_err(|e| OpenError::Other(format!("the PNG does not read: {e}")))?;
    let line = frame.line_size;
    let rows = (0..height).map(|y| &buffer[y * line..(y + 1) * line]);

    use png::ColorType as C;
    let mode = match (depth, color) {
        (1, C::Grayscale) => Mode::One,
        (2 | 4 | 8, C::Grayscale) => Mode::L,
        (16, C::Grayscale) => Mode::I16,
        (8 | 16, C::Rgb) => Mode::Rgb,
        (1 | 2 | 4 | 8, C::Indexed) => Mode::P,
        (8, C::GrayscaleAlpha) => Mode::LA,
        (16, C::GrayscaleAlpha) => Mode::Rgba,
        (8 | 16, C::Rgba) => Mode::Rgba,
        _ => {
            return Err(OpenError::Unidentified(format!(
                "cannot identify image file {}",
                py_repr(filename)
            )));
        }
    };
    let mut image = Image::new(mode, width, height);
    for (y, row) in rows.enumerate() {
        let out = image.row_mut(y);
        match (depth, color) {
            // `unpack1`: a set bit is white.
            (1, C::Grayscale) => unpack_bits(row, 1, out, |v| if v != 0 { 255 } else { 0 }),
            (2, C::Grayscale) => unpack_bits(row, 2, out, |v| v * 0x55),
            (4, C::Grayscale) => unpack_bits(row, 4, out, |v| v * 0x11),
            (8, C::Grayscale) | (8, C::Indexed) => out.copy_from_slice(&row[..width]),
            (1 | 2 | 4, C::Indexed) => unpack_bits(row, depth, out, |v| v),
            // `unpackI16B_I16`: stored little-endian.
            (16, C::Grayscale) => {
                for (o, i) in out.chunks_exact_mut(2).zip(row.chunks_exact(2)) {
                    o.copy_from_slice(&[i[1], i[0]]);
                }
            }
            (8, C::Rgb) => {
                for (o, i) in out.chunks_exact_mut(4).zip(row.chunks_exact(3)) {
                    o.copy_from_slice(&[i[0], i[1], i[2], 255]);
                }
            }
            (16, C::Rgb) => {
                for (o, i) in out.chunks_exact_mut(4).zip(row.chunks_exact(6)) {
                    o.copy_from_slice(&[i[0], i[2], i[4], 255]);
                }
            }
            (8, C::GrayscaleAlpha) => {
                for (o, i) in out.chunks_exact_mut(4).zip(row.chunks_exact(2)) {
                    o.copy_from_slice(&[i[0], i[0], i[0], i[1]]);
                }
            }
            (16, C::GrayscaleAlpha) => {
                for (o, i) in out.chunks_exact_mut(4).zip(row.chunks_exact(4)) {
                    o.copy_from_slice(&[i[0], i[0], i[0], i[2]]);
                }
            }
            (8, C::Rgba) => out.copy_from_slice(&row[..width * 4]),
            (16, C::Rgba) => {
                for (o, i) in out.chunks_exact_mut(4).zip(row.chunks_exact(8)) {
                    o.copy_from_slice(&[i[0], i[2], i[4], i[6]]);
                }
            }
            _ => unreachable!("the modes above"),
        }
    }
    if mode == Mode::P {
        // `ImagePalette.raw("RGB", PLTE)`: entries not given are black (`Palette.c:24`).
        let mut palette = vec![0u8; 256 * 4];
        for i in 0..256 {
            palette[i * 4 + 3] = 255;
        }
        for (i, rgb) in palette_chunk
            .unwrap_or_default()
            .chunks_exact(3)
            .take(256)
            .enumerate()
        {
            palette[i * 4..i * 4 + 3].copy_from_slice(rgb);
        }
        image.palette = Some(palette);
    }
    Ok(image)
}

/// The packed samples of a row, `bits` each, the first in the high bits, through `f`.
fn unpack_bits(row: &[u8], bits: u8, out: &mut [u8], f: impl Fn(u8) -> u8) {
    let per_byte = 8 / bits as usize;
    let mask = ((1u16 << bits) - 1) as u8;
    for (x, o) in out.iter_mut().enumerate() {
        let byte = row[x / per_byte];
        let shift = 8 - bits as usize * (x % per_byte + 1);
        *o = f((byte >> shift) & mask);
    }
}
