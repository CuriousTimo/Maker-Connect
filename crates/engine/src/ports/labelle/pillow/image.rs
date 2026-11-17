//! `PIL/Image.py` and the parts of `libImaging` that hold an image: [`Image`] is Pillow's core
//! image (`Imaging`), in the modes a picture label goes through.
//!
//! Pillow keeps "1", "L" and "P" at one byte a pixel (a "1" pixel is 0 or 255), "I;16" at two
//! (little-endian), and "LA", "RGB", "RGBA", "La" and "RGBa" at four (`Storage.c`; an "LA"
//! pixel is `L, L, L, A` after unpacking, `Unpack.c:418`).

use super::resample;

/// The modes (`Mode.c`) a picture passes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// "1"
    One,
    L,
    LA,
    P,
    Rgb,
    Rgba,
    /// "La": LA, premultiplied.
    LaPremul,
    /// "RGBa": RGBA, premultiplied.
    RgbaPremul,
    /// "I;16"
    I16,
}

impl Mode {
    /// Bytes a pixel takes in the core image.
    pub fn pixelsize(self) -> usize {
        match self {
            Mode::One | Mode::L | Mode::P => 1,
            Mode::I16 => 2,
            Mode::LA | Mode::Rgb | Mode::Rgba | Mode::LaPremul | Mode::RgbaPremul => 4,
        }
    }

    /// Pillow's name for the mode.
    pub fn name(self) -> &'static str {
        match self {
            Mode::One => "1",
            Mode::L => "L",
            Mode::LA => "LA",
            Mode::P => "P",
            Mode::Rgb => "RGB",
            Mode::Rgba => "RGBA",
            Mode::LaPremul => "La",
            Mode::RgbaPremul => "RGBa",
            Mode::I16 => "I;16",
        }
    }

    /// `Imaging->bands`.
    pub fn bands(self) -> usize {
        match self {
            Mode::One | Mode::L | Mode::P | Mode::I16 => 1,
            Mode::LA | Mode::LaPremul => 2,
            Mode::Rgb => 3,
            Mode::Rgba | Mode::RgbaPremul => 4,
        }
    }
}

/// Pillow's core image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub mode: Mode,
    pub width: usize,
    pub height: usize,
    /// Row by row, `width * pixelsize` bytes a row.
    pub data: Vec<u8>,
    /// A "P" image's palette: 256 entries of R, G, B, A (`Palette.c:24`: unset entries are
    /// black and opaque).
    pub palette: Option<Vec<u8>>,
}

impl Image {
    /// `Image.new(mode, (width, height))`: every pixel 0.
    pub fn new(mode: Mode, width: usize, height: usize) -> Image {
        Image {
            mode,
            width,
            height,
            data: vec![0; width * height * mode.pixelsize()],
            palette: None,
        }
    }

    pub fn row(&self, y: usize) -> &[u8] {
        let n = self.width * self.mode.pixelsize();
        &self.data[y * n..(y + 1) * n]
    }

    pub fn row_mut(&mut self, y: usize) -> &mut [u8] {
        let n = self.width * self.mode.pixelsize();
        &mut self.data[y * n..(y + 1) * n]
    }

    /// `Image.paste(im, box=(x0, y0))` of an image of the same mode (`Image.py:1897`, then
    /// `ImagingPaste`, `Paste.c:274`): the part inside the image is copied.
    pub fn paste(&mut self, source: &Image, x0: i64, y0: i64) -> Result<(), String> {
        if self.mode != source.mode {
            return Err("paste: the modes differ".to_owned());
        }
        let pixelsize = self.mode.pixelsize() as i64;
        let (mut dx0, mut dy0) = (x0, y0);
        let (mut xsize, mut ysize) = (source.width as i64, source.height as i64);
        let (mut sx0, mut sy0) = (0i64, 0i64);
        if dx0 < 0 {
            xsize += dx0;
            sx0 = -dx0;
            dx0 = 0;
        }
        if dx0 + xsize > self.width as i64 {
            xsize = self.width as i64 - dx0;
        }
        if dy0 < 0 {
            ysize += dy0;
            sy0 = -dy0;
            dy0 = 0;
        }
        if dy0 + ysize > self.height as i64 {
            ysize = self.height as i64 - dy0;
        }
        if xsize <= 0 || ysize <= 0 {
            return Ok(());
        }
        for y in 0..ysize {
            let from = ((sy0 + y) as usize * source.width + sx0 as usize) * pixelsize as usize;
            let to = ((dy0 + y) as usize * self.width + dx0 as usize) * pixelsize as usize;
            let n = xsize as usize * pixelsize as usize;
            self.data[to..to + n].copy_from_slice(&source.data[from..from + n]);
        }
        Ok(())
    }

    /// `Image.transpose(Image.Transpose.ROTATE_270)` (`ImagingRotate270`, `Geometry.c:311`):
    /// `out[x][height - 1 - y] = in[y][x]`.
    pub fn rotate_270(&self) -> Image {
        let mut out = Image::new(self.mode, self.height, self.width);
        out.palette = self.palette.clone();
        let p = self.mode.pixelsize();
        for y in 0..self.height {
            let yr = self.height - 1 - y;
            for x in 0..self.width {
                let from = (y * self.width + x) * p;
                let to = (x * self.height + yr) * p;
                out.data[to..to + p].copy_from_slice(&self.data[from..from + p]);
            }
        }
        out
    }

    /// `Image.tobytes()` of a "1" image (the raw encoder with rawmode "1": `pack1`,
    /// `Pack.c:83`): eight pixels a byte, the first in bit 7, each row padded to a byte.
    pub fn tobytes_one(&self) -> Result<Vec<u8>, String> {
        if self.mode != Mode::One {
            return Err("tobytes: only a \"1\" image is ported".to_owned());
        }
        let mut out = Vec::with_capacity(self.height * self.width.div_ceil(8));
        for y in 0..self.height {
            let row = self.row(y);
            let (mut b, mut m) = (0u8, 128u8);
            for &pixel in row {
                if pixel != 0 {
                    b |= m;
                }
                m >>= 1;
                if m == 0 {
                    out.push(b);
                    b = 0;
                    m = 128;
                }
            }
            if m != 128 {
                out.push(b);
            }
        }
        Ok(out)
    }

    /// `Image.point(list(range(255, -1, -1)))` on an "L" image: `ImageOps.invert`
    /// (`ImageOps.py:629`, `ImagingPoint`, `Point.c:131`, `im_point_8_8`).
    pub fn invert(&self) -> Result<Image, String> {
        match self.mode {
            Mode::L => {
                let mut out = self.clone();
                for v in &mut out.data {
                    *v = 255 - *v;
                }
                Ok(out)
            }
            // ImageOps._lut: P is "coming soon"; any other mode but "RGB" is an OSError.
            Mode::P => Err("mode P support coming soon".to_owned()),
            other => Err(format!("not supported for mode {}", other.name())),
        }
    }

    /// `Image.resize(size)` with no resample filter and no box (`Image.py:2329`): Pillow's
    /// default, BICUBIC, or NEAREST for "1" and "P".
    pub fn resize(&self, size: (usize, usize)) -> Result<Image, String> {
        self.resize_with(
            size,
            None,
            (0.0, 0.0, self.width as f32, self.height as f32),
        )
    }

    /// `Image.resize(size, resample, box)`. `resample`: None is the default.
    fn resize_with(
        &self,
        size: (usize, usize),
        resample: Option<resample::Filter>,
        box_: (f32, f32, f32, f32),
    ) -> Result<Image, String> {
        use resample::Filter;
        let mut resample = resample.unwrap_or(Filter::Bicubic);
        if (self.width, self.height) == size
            && box_ == (0.0, 0.0, self.width as f32, self.height as f32)
        {
            return Ok(self.clone());
        }
        if matches!(self.mode, Mode::One | Mode::P) {
            resample = Filter::Nearest;
        }
        if matches!(self.mode, Mode::LA | Mode::Rgba) && resample != Filter::Nearest {
            let (premul, back) = match self.mode {
                Mode::LA => (Mode::LaPremul, Mode::LA),
                _ => (Mode::RgbaPremul, Mode::Rgba),
            };
            let im = self.convert_premul(premul)?;
            let im = im.resize_with(size, Some(resample), box_)?;
            return im.convert_premul(back);
        }
        if self.height > self.width * 100 && size.1 < self.height {
            let im = resample::resize(
                self,
                (self.width, size.1),
                resample,
                (0.0, box_.1, self.width as f32, box_.3),
            )?;
            return resample::resize(&im, size, resample, (box_.0, 0.0, box_.2, size.1 as f32));
        }
        resample::resize(self, size, resample, box_)
    }
}
