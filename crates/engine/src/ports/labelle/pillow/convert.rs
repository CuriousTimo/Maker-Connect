//! `Image.convert` and `libImaging/Convert.c`: the conversions a picture label goes through
//! ("L" from every mode a PNG opens as, "1" from "L" with Floyd-Steinberg dithering, and the
//! premultiplied modes `Image.resize` uses for "LA" and "RGBA").

use super::image::{Image, Mode};

/// `L24(rgb) >> 16` (`Convert.c:42`): ITU-R 601-2 luma, in fixed point.
fn l24(rgb: &[u8]) -> u8 {
    ((rgb[0] as u32 * 19595 + rgb[1] as u32 * 38470 + rgb[2] as u32 * 7471 + 0x8000) >> 16) as u8
}

/// `L(rgb)` (`Convert.c:41`).
fn l(rgb: &[u8]) -> i32 {
    rgb[0] as i32 * 299 + rgb[1] as i32 * 587 + rgb[2] as i32 * 114
}

/// `CLIP8` (`ImagingUtils.h:28`).
fn clip8(v: i32) -> i32 {
    if v <= 0 {
        0
    } else if v < 256 {
        v
    } else {
        255
    }
}

/// `MULDIV255` (`ImagingUtils.h:20`).
fn muldiv255(a: u32, b: u32) -> u32 {
    let tmp = a * b + 128;
    ((tmp >> 8) + tmp) >> 8
}

impl Image {
    /// `Image.convert("L")` (`Image.py:1017`, `convert`, `Convert.c:1579`): the same mode is a
    /// copy; a palette image goes through `frompalette` (`Convert.c:1079`, `p2l`: the
    /// palette's luma); the others through their entry of the `converters` table.
    pub fn convert_l(&self) -> Result<Image, String> {
        let mut out = Image::new(Mode::L, self.width, self.height);
        match self.mode {
            Mode::L => return Ok(self.clone()),
            // `bit2l` (`Convert.c:49`).
            Mode::One => {
                for (o, i) in out.data.iter_mut().zip(&self.data) {
                    *o = if *i != 0 { 255 } else { 0 };
                }
            }
            // `p2l` (`Convert.c:891`).
            Mode::P => {
                let palette = self.palette.as_ref().ok_or("no palette")?;
                for (o, i) in out.data.iter_mut().zip(&self.data) {
                    *o = l24(&palette[*i as usize * 4..]);
                }
            }
            // `la2l` (`Convert.c:171`).
            Mode::LA => {
                for (o, i) in out.data.iter_mut().zip(self.data.chunks_exact(4)) {
                    *o = i[0];
                }
            }
            // `rgb2l` (`Convert.c:212`), RGB and RGBA alike.
            Mode::Rgb | Mode::Rgba => {
                for (o, i) in out.data.iter_mut().zip(self.data.chunks_exact(4)) {
                    *o = l24(i);
                }
            }
            // `I16L_L` (`Convert.c:831`).
            Mode::I16 => {
                for (o, i) in out.data.iter_mut().zip(self.data.chunks_exact(2)) {
                    *o = if i[1] != 0 { 255 } else { i[0] };
                }
            }
            other => {
                return Err(format!(
                    "conversion from {} to L not supported",
                    other.name()
                ));
            }
        }
        Ok(out)
    }

    /// `Image.convert("1")` of an "L" image with the default dither, Floyd-Steinberg
    /// (`tobilevel`, `Convert.c:1349`). Also of an "RGB" image (its second branch).
    pub fn convert_one(&self) -> Result<Image, String> {
        if self.mode == Mode::One {
            return Ok(self.clone());
        }
        if self.mode != Mode::L && self.mode != Mode::Rgb {
            return Err(if self.mode == Mode::P {
                "conversion not supported".to_owned()
            } else {
                format!("conversion from {} to 1 not supported", self.mode.name())
            });
        }
        let mut out = Image::new(Mode::One, self.width, self.height);
        let mut errors = vec![0i32; self.width + 1];
        for y in 0..self.height {
            let row = self.row(y);
            let out_row = out.row_mut(y);
            let (mut l_, mut l0, mut l1) = (0i32, 0i32, 0i32);
            for x in 0..self.width {
                // pick closest colour
                let value = if self.mode == Mode::L {
                    row[x] as i32
                } else {
                    l(&row[x * 4..]) / 1000
                };
                l_ = clip8(value + (l_ + errors[x + 1]) / 16);
                out_row[x] = if l_ > 128 { 255 } else { 0 };
                // propagate errors
                l_ -= out_row[x] as i32;
                let l2 = l_;
                let d2 = l_ + l_;
                l_ += d2;
                errors[x] = l_ + l0;
                l_ += d2;
                l0 = l_ + l1;
                l1 = l2;
                l_ += d2;
            }
            errors[self.width] = l0;
        }
        Ok(out)
    }

    /// `Image.convert("La")` / `("RGBa")` (premultiply: `lA2la`, `Convert.c:113`, `rgbA2rgba`,
    /// `:420`) and back to `"LA"` / `"RGBA"` (`la2lA`, `:129`, `rgba2rgbA`, `:435`), as
    /// `Image.resize` does for images with an alpha channel (`Image.py:2407`).
    pub fn convert_premul(&self, mode: Mode) -> Result<Image, String> {
        let mut out = Image::new(mode, self.width, self.height);
        match (self.mode, mode) {
            (Mode::LA, Mode::LaPremul) => {
                for (o, i) in out.data.chunks_exact_mut(4).zip(self.data.chunks_exact(4)) {
                    let alpha = i[3] as u32;
                    let pixel = muldiv255(i[0] as u32, alpha) as u8;
                    o.copy_from_slice(&[pixel, pixel, pixel, alpha as u8]);
                }
            }
            (Mode::LaPremul, Mode::LA) => {
                for (o, i) in out.data.chunks_exact_mut(4).zip(self.data.chunks_exact(4)) {
                    let alpha = i[3] as i32;
                    let pixel = if alpha == 255 || alpha == 0 {
                        i[0] as i32
                    } else {
                        clip8(255 * i[0] as i32 / alpha)
                    } as u8;
                    o.copy_from_slice(&[pixel, pixel, pixel, alpha as u8]);
                }
            }
            (Mode::Rgba, Mode::RgbaPremul) => {
                for (o, i) in out.data.chunks_exact_mut(4).zip(self.data.chunks_exact(4)) {
                    let alpha = i[3] as u32;
                    o.copy_from_slice(&[
                        muldiv255(i[0] as u32, alpha) as u8,
                        muldiv255(i[1] as u32, alpha) as u8,
                        muldiv255(i[2] as u32, alpha) as u8,
                        i[3],
                    ]);
                }
            }
            (Mode::RgbaPremul, Mode::Rgba) => {
                for (o, i) in out.data.chunks_exact_mut(4).zip(self.data.chunks_exact(4)) {
                    let alpha = i[3] as i32;
                    if alpha == 255 || alpha == 0 {
                        o.copy_from_slice(i);
                    } else {
                        o.copy_from_slice(&[
                            clip8(255 * i[0] as i32 / alpha) as u8,
                            clip8(255 * i[1] as i32 / alpha) as u8,
                            clip8(255 * i[2] as i32 / alpha) as u8,
                            i[3],
                        ]);
                    }
                }
            }
            (from, to) => {
                return Err(format!(
                    "conversion from {} to {} not supported",
                    from.name(),
                    to.name()
                ));
            }
        }
        Ok(out)
    }
}
