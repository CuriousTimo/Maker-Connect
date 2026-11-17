//! **Ours (W2, W3, W10): the page CUPS would hand the filter, made from the hub's PNG.** In a CUPS queue an
//! upstream filter (`pdftoraster`, `imagetoraster`, ...) rasterises the document into the page the PPD
//! describes; here the document is a PNG that is already that raster.
//!
//! - The page *header* is CUPS's own: [`cups_raster_interpret_ppd`] over the PPD's marked options (the
//!   PPD's defaults plus the options given).
//! - The page is the PPD's `PageSize` at the PPD's resolution, as the *imageable area*
//!   (`cupsWidth = (right - left) * xres / 72`, `cupsHeight = (top - bottom) * yres / 72`, rounded:
//!   `raster-interpret.c:386-389`). The PNG must be exactly that size: anything else fails the job,
//!   nothing is scaled (W2).
//! - The size is chosen by the job's label length (W2): the `PageSize` choices whose length in mm
//!   (`PaperDimension`, rounded) is it and whose page is the PNG's size; the PPD's default (`DefaultPageSize`,
//!   `lw450.ppd:149`) when the job gives no length.
//! - The pixels are RGB, 8 bits (`DymoHalftoning ErrorDiffusion` and `NLL`, `lw450.ppd:426-427`), alpha laid
//!   over white paper, 16 bits cut to 8; or, for `DymoHalftoning Default` (1 bit, `cupsColorSpace 3`),
//!   black where the driver's `RGBToGrayScale` is under 128, packed 8 to a byte, black is 1 (W3).

use super::cups::options::{cups_parse_options, CupsOption};
use super::cups::ppd::PpdFile;
use super::cups::raster::{cups_raster_interpret_ppd, CupsRaster, PageHeader, RasterPage, CUPS_CSPACE_K, CUPS_CSPACE_RGB};
use super::halftoning::{HalftoneBase, ImageType};

/// A decoded PNG: RGB, 8 bits, `width * height * 3` bytes.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// Decodes a PNG: `png` for Pillow's PNG decoder in the PnP (ours; the same crate choice, no shared code).
pub fn decode_png(path: &std::path::Path) -> Result<Picture, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("cannot open the picture: {e}"))?;
    decode_png_reader(std::io::BufReader::new(file))
}

/// [`decode_png`] over a reader.
pub fn decode_png_reader<R: std::io::Read>(r: R) -> Result<Picture, String> {
    let mut decoder = png::Decoder::new(r);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| format!("not a PNG: {e}"))?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("not a PNG: {e}"))?;
    let (w, h) = (info.width as usize, info.height as usize);
    if info.bit_depth != png::BitDepth::Eight {
        return Err("the PNG is not 8 bits per channel after decoding".to_owned());
    }
    let data = &buf[..info.buffer_size()];
    let mut rgb = Vec::with_capacity(w * h * 3);
    // alpha laid over white paper: c * a/255 + 255 * (255-a)/255, rounded
    let over_white = |c: u8, a: u8| -> u8 { ((c as u32 * a as u32 + 255 * (255 - a as u32) + 127) / 255) as u8 };
    match info.color_type {
        png::ColorType::Grayscale => {
            for g in data {
                rgb.extend_from_slice(&[*g, *g, *g]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for p in data.chunks_exact(2) {
                let g = over_white(p[0], p[1]);
                rgb.extend_from_slice(&[g, g, g]);
            }
        }
        png::ColorType::Rgb => rgb.extend_from_slice(data),
        png::ColorType::Rgba => {
            for p in data.chunks_exact(4) {
                rgb.extend_from_slice(&[over_white(p[0], p[3]), over_white(p[1], p[3]), over_white(p[2], p[3])]);
            }
        }
        png::ColorType::Indexed => return Err("an indexed PNG was not expanded".to_owned()),
    }
    Ok(Picture { width: w as u32, height: h as u32, rgb })
}

/// A page's geometry from the PPD: the marked options made into CUPS's page header.
fn header_for(ppd: &mut PpdFile, options: &[CupsOption]) -> Result<PageHeader, String> {
    cups_raster_interpret_ppd(ppd, options)
}

// `length_mm` and `marked_ppd` (W2) live in `ports::geometry::lw450` (HS1), so the hub reads the same rule.
use crate::ports::geometry::lw450::{length_mm, marked_ppd};

/// The page for a picture, as a raster of one page.
pub fn make_raster(
    picture: &Picture,
    page_size: Option<&str>,
    label_length_mm: Option<u32>,
    option_string: &str,
    open_ppd: &dyn Fn() -> Option<PpdFile>,
) -> Result<CupsRaster, String> {
    Ok(CupsRaster::open(vec![make_page(picture, page_size, label_length_mm, option_string, open_ppd)?]))
}

/// The page for a picture: its header and its pixels.
pub fn make_page(
    picture: &Picture,
    page_size: Option<&str>,
    label_length_mm: Option<u32>,
    option_string: &str,
    open_ppd: &dyn Fn() -> Option<PpdFile>,
) -> Result<RasterPage, String> {
    let mut options: Vec<CupsOption> = Vec::new();
    cups_parse_options(option_string, &mut options);

    let sizes: Vec<&'static str> = {
        let mut ppd = open_ppd().ok_or("no PPD")?;
        ppd.mark_defaults();
        ppd.page_size_names()
    };

    // the header of the PPD's own choices
    let header = match (page_size, label_length_mm) {
        // W2b (proxy, 2026-11-17): the job names the PageSize choice, as CUPS's `-o PageSize=<name>`:
        // exactly that choice is marked, an unknown name fails, and the length rule is not used
        (Some(name), _) => {
            if !sizes.contains(&name) {
                return Err(format!("no PageSize {name} in the PPD"));
            }
            let mut ppd = marked_ppd(open_ppd, &options)?;
            ppd.mark_option("PageSize", name);
            header_for(&mut ppd, &options)?
        }
        (None, None) => {
            let mut ppd = marked_ppd(open_ppd, &options)?;
            header_for(&mut ppd, &options)?
        }
        (None, Some(mm)) => {
            let mut found: Vec<(PageHeader, &'static str)> = Vec::new();
            for name in &sizes {
                let mut ppd = marked_ppd(open_ppd, &options)?;
                ppd.mark_option("PageSize", name);
                let size = ppd.page_size(None).ok_or("no page size")?;
                if length_mm(size.length) != mm {
                    continue;
                }
                let h = header_for(&mut ppd, &options)?;
                if h.cups_width == picture.width && h.cups_height == picture.height && !found.iter().any(|(f, _)| same_page(f, &h)) {
                    found.push((h, name));
                }
            }
            match found.len() {
                1 => found.remove(0).0,
                0 => return Err(format!("no label of the PPD is {mm} mm long and {} x {} dots", picture.width, picture.height)),
                _ => return Err(format!("several labels of the PPD are {mm} mm long and {} x {} dots", picture.width, picture.height)),
            }
        }
    };

    if header.cups_width != picture.width || header.cups_height != picture.height {
        return Err(format!(
            "the picture is {} x {} dots, the label ({}) needs {} x {} dots",
            picture.width, picture.height, header.cups_page_size_name, header.cups_width, header.cups_height
        ));
    }

    let pixels = pixels_for(&header, picture)?;
    Ok(RasterPage { header, pixels })
}

/// The same page for the filter: every header field it reads, and the size.
fn same_page(a: &PageHeader, b: &PageHeader) -> bool {
    let mut a = a.clone();
    a.cups_page_size_name.clear();
    let mut b = b.clone();
    b.cups_page_size_name.clear();
    a == b
}

/// The pixels a page has for its header (W3).
fn pixels_for(header: &PageHeader, picture: &Picture) -> Result<Vec<u8>, String> {
    let (w, h) = (header.cups_width as usize, header.cups_height as usize);
    match (header.cups_color_space, header.cups_bits_per_pixel) {
        (CUPS_CSPACE_RGB, 24) => Ok(picture.rgb.clone()),
        (CUPS_CSPACE_K, 1) => {
            let base = HalftoneBase::new(ImageType::RGB, ImageType::BW);
            let mut out = Vec::with_capacity(header.cups_bytes_per_line as usize * h);
            for y in 0..h {
                let mut line = vec![0u8; header.cups_bytes_per_line as usize];
                for x in 0..w {
                    let i = (y * w + x) * 3;
                    let gray = base.rgb_to_gray_scale(picture.rgb[i], picture.rgb[i + 1], picture.rgb[i + 2]);
                    if gray < 128 {
                        line[x / 8] |= 1 << (7 - x % 8);
                    }
                }
                out.extend_from_slice(&line);
            }
            Ok(out)
        }
        (cs, bpp) => Err(format!("the PPD's page has colour space {cs}, {bpp} bits per pixel: not made from a picture")),
    }
}
