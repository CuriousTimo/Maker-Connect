//! Label printers. A label is a raster ([`Label`]): one dot per pixel, its height across the
//! tape, its length along it. The website draws it (SVG, rendered at the printer's
//! resolution) and sends it as a PNG; [`Label::from_png`] reads that back. [`dymo`] turns a
//! label into what a DYMO LabelManager takes, and (with the `hid` feature) sends it;
//! [`labelwriter`] the same for a DYMO LabelWriter 400 or 450 (with the `usb` feature).

pub mod dymo;
pub mod labelwriter;

/// A label to print: `height` dots across the tape (top first), `length` dots along it (the
/// start of the label first), dark dots burned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub length: usize,
    pub height: usize,
    /// Row by row from the top, each from the start of the label: `height × length`.
    dots: Vec<bool>,
}

impl Label {
    pub fn blank(length: usize, height: usize) -> Self {
        Self {
            length,
            height,
            dots: vec![false; length * height],
        }
    }

    pub fn get(&self, x: usize, y: usize) -> bool {
        x < self.length && y < self.height && self.dots[y * self.length + x]
    }

    pub fn set(&mut self, x: usize, y: usize, on: bool) {
        if x < self.length && y < self.height {
            self.dots[y * self.length + x] = on;
        }
    }

    /// A PNG as a label: a dot where the pixel is dark and not transparent. Its width is
    /// the label's length, its height across the tape.
    pub fn from_png(bytes: &[u8]) -> Result<Self, String> {
        let mut decoder = png::Decoder::new(bytes);
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().map_err(|e| format!("not a PNG: {e}"))?;
        let (width, height) = (reader.info().width as usize, reader.info().height as usize);
        if width == 0 || height == 0 || width > 20_000 || height > 1_000 {
            return Err(format!(
                "a label of {width} × {height} dots is not printable"
            ));
        }
        let mut buffer = vec![0; reader.output_buffer_size()];
        let frame = reader
            .next_frame(&mut buffer)
            .map_err(|e| format!("the PNG does not read: {e}"))?;
        let channels = frame.color_type.samples();
        let pixels = &buffer[..frame.buffer_size()];
        let mut label = Label::blank(width, height);
        for y in 0..height {
            for x in 0..width {
                let p = &pixels[(y * width + x) * channels..][..channels];
                let (grey, alpha) = match channels {
                    1 => (p[0] as u32, 255),
                    2 => (p[0] as u32, p[1] as u32),
                    3 => ((p[0] as u32 * 3 + p[1] as u32 * 6 + p[2] as u32) / 10, 255),
                    _ => (
                        (p[0] as u32 * 3 + p[1] as u32 * 6 + p[2] as u32) / 10,
                        p[3] as u32,
                    ),
                };
                // Dark on an opaque pixel: transparent counts as the tape.
                label.set(x, y, alpha >= 128 && grey < 128);
            }
        }
        Ok(label)
    }

    /// The label as a black and white PNG, to show it.
    pub fn to_png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, self.length as u32, self.height as u32);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("writing to memory");
            let pixels: Vec<u8> = self
                .dots
                .iter()
                .map(|&on| if on { 0 } else { 255 })
                .collect();
            writer.write_image_data(&pixels).expect("writing to memory");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_png_reads_back_as_the_same_label() {
        let mut label = Label::blank(5, 3);
        label.set(0, 0, true);
        label.set(4, 2, true);
        let read = Label::from_png(&label.to_png()).unwrap();
        assert_eq!(read, label);
        assert!(read.get(0, 0) && read.get(4, 2) && !read.get(1, 1));
    }
}
