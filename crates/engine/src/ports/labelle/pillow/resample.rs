//! `Image.resize`'s core: `_resize` (`_imaging.c:1937`), the nearest-neighbour scaling
//! (`ImagingScaleAffine`, `Geometry.c:832`) and `libImaging/Resample.c` (the 8-bit path: the
//! filters, `precompute_coeffs`, `normalize_coeffs_8bpc`, the two passes).

use super::image::{Image, Mode};

/// `Resampling`: the filters a resize here can take (`Image.resize` picks BICUBIC, or NEAREST
/// for "1" and "P").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    Nearest,
    Bicubic,
}

/// `PRECISION_BITS` (`Resample.c:93`).
const PRECISION_BITS: u32 = 32 - 8 - 2;

/// `bicubic_filter` (`Resample.c:48`), `a = -0.5`.
fn bicubic_filter(x: f64) -> f64 {
    const A: f64 = -0.5;
    let x = if x < 0.0 { -x } else { x };
    if x < 1.0 {
        return ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0;
    }
    if x < 2.0 {
        return (((x - 5.0) * x + 8.0) * x - 4.0) * A;
    }
    0.0
}

/// `clip8` (`Resample.c:178`): `_clip8_lookups[in >> PRECISION_BITS]`, which is 0 below 0, the
/// value from 0 to 255 and 255 above (the table holds -640 to 639).
fn clip8(value: i32) -> u8 {
    (value >> PRECISION_BITS).clamp(0, 255) as u8
}

/// `precompute_coeffs` (`Resample.c:183`) for the bicubic filter: the filter size, the bounds
/// (`xmin`, `xmax` for each output pixel) and the coefficients.
fn precompute_coeffs(
    in_size: usize,
    in0: f32,
    in1: f32,
    out_size: usize,
) -> (usize, Vec<(i32, i32)>, Vec<f64>) {
    // prepare for horizontal stretch
    let scale = (in1 - in0) as f64 / out_size as f64;
    let filterscale = if scale < 1.0 { 1.0 } else { scale };
    // determine support size (length of resampling filter)
    let support = 2.0 * filterscale;
    // maximum number of coeffs
    let ksize = support.ceil() as usize * 2 + 1;
    let mut kk = vec![0.0f64; out_size * ksize];
    let mut bounds = Vec::with_capacity(out_size);
    let inv_filterscale = 1.0 / filterscale;
    for xx in 0..out_size {
        let center = in0 as f64 + (xx as f64 + 0.5) * scale;
        let mut ww = 0.0;
        // Round the value
        let mut xmin = (center - support + 0.5) as i32;
        if xmin < 0 {
            xmin = 0;
        }
        // Round the value
        let mut xmax = (center + support + 0.5) as i32;
        if xmax > in_size as i32 {
            xmax = in_size as i32;
        }
        xmax -= xmin;
        let k = &mut kk[xx * ksize..(xx + 1) * ksize];
        for x in 0..xmax {
            let w = bicubic_filter((x as f64 + xmin as f64 - center + 0.5) * inv_filterscale);
            k[x as usize] = w;
            ww += w;
        }
        if ww != 0.0 {
            for x in 0..xmax {
                k[x as usize] /= ww;
            }
        }
        bounds.push((xmin, xmax));
    }
    (ksize, bounds, kk)
}

/// `normalize_coeffs_8bpc` (`Resample.c:270`).
fn normalize_coeffs_8bpc(kk: &[f64]) -> Vec<i32> {
    kk.iter()
        .map(|&k| {
            if k < 0.0 {
                (-0.5 + k * (1i64 << PRECISION_BITS) as f64) as i32
            } else {
                (0.5 + k * (1i64 << PRECISION_BITS) as f64) as i32
            }
        })
        .collect()
}

/// The bands of a pixel the 8-bit passes compute (`Resample.c:288`ff): one for "L" and "P", two
/// (bands 0 and 3) for "LA", three for "RGB", four for the rest.
fn channels(mode: Mode) -> &'static [usize] {
    match mode.bands() {
        2 => &[0, 3],
        3 => &[0, 1, 2],
        4 => &[0, 1, 2, 3],
        _ => &[0],
    }
}

/// `_ImagingResampleHorizontal_8bpc` (`Resample.c:288`).
fn horizontal_8bpc(
    out: &mut Image,
    im: &Image,
    offset: usize,
    ksize: usize,
    bounds: &[(i32, i32)],
    prekk: &[f64],
) {
    let kk = normalize_coeffs_8bpc(prekk);
    let pixel = im.mode.pixelsize();
    for yy in 0..out.height {
        let line_in = im.row(yy + offset).to_vec();
        let line_out = out.row_mut(yy);
        for xx in 0..bounds.len() {
            let (xmin, xmax) = bounds[xx];
            let k = &kk[xx * ksize..];
            for &c in channels(im.mode) {
                let mut ss = 1i32 << (PRECISION_BITS - 1);
                for x in 0..xmax as usize {
                    ss = ss.wrapping_add(
                        (line_in[(x + xmin as usize) * pixel + c] as i32).wrapping_mul(k[x]),
                    );
                }
                line_out[xx * pixel + c] = clip8(ss);
            }
        }
    }
}

/// `_ImagingResampleVertical_8bpc` (`Resample.c:392`).
fn vertical_8bpc(out: &mut Image, im: &Image, ksize: usize, bounds: &[(i32, i32)], prekk: &[f64]) {
    let kk = normalize_coeffs_8bpc(prekk);
    let pixel = im.mode.pixelsize();
    for yy in 0..out.height {
        let k = &kk[yy * ksize..];
        let (ymin, ymax) = bounds[yy];
        for xx in 0..out.width {
            for &c in channels(im.mode) {
                let mut ss = 1i32 << (PRECISION_BITS - 1);
                for y in 0..ymax as usize {
                    let v = im.data[(y + ymin as usize) * im.width * pixel + xx * pixel + c];
                    ss = ss.wrapping_add((v as i32).wrapping_mul(k[y]));
                }
                out.data[yy * out.width * pixel + xx * pixel + c] = clip8(ss);
            }
        }
    }
}

/// `ImagingResample` and `ImagingResampleInner` (`Resample.c:711`, `:773`) with the bicubic
/// filter.
fn resample_bicubic(
    im: &Image,
    xsize: usize,
    ysize: usize,
    box_: (f32, f32, f32, f32),
) -> Result<Image, String> {
    if matches!(im.mode, Mode::P | Mode::One) {
        return Err("The image mode is not supported".to_owned());
    }
    if im.mode == Mode::I16 {
        // `_ImagingResampleHorizontal_16bpc`: not ported (a 16-bit grey PNG taller than the tape).
        return Err("resizing an I;16 image is not ported".to_owned());
    }
    let need_horizontal = xsize != im.width || box_.0 != 0.0 || box_.2 != xsize as f32;
    let need_vertical = ysize != im.height || box_.1 != 0.0 || box_.3 != ysize as f32;
    let (ksize_vert, mut bounds_vert, kk_vert) =
        precompute_coeffs(im.height, box_.1, box_.3, ysize);
    // First used row in the source image
    let ybox_first = bounds_vert[0].0;
    // Last used row in the source image
    let ybox_last = bounds_vert[ysize - 1].0 + bounds_vert[ysize - 1].1;

    let mut temp: Option<Image> = None;
    // two-pass resize, horizontal pass
    if need_horizontal {
        let (ksize_horiz, bounds_horiz, kk_horiz) =
            precompute_coeffs(im.width, box_.0, box_.2, xsize);
        // Shift bounds for vertical pass
        for b in bounds_vert.iter_mut() {
            b.0 -= ybox_first;
        }
        let mut t = Image::new(im.mode, xsize, (ybox_last - ybox_first) as usize);
        horizontal_8bpc(
            &mut t,
            im,
            ybox_first as usize,
            ksize_horiz,
            &bounds_horiz,
            &kk_horiz,
        );
        temp = Some(t);
    }
    let source = temp.as_ref().unwrap_or(im);
    // vertical pass
    if need_vertical {
        let mut out = Image::new(source.mode, source.width, ysize);
        vertical_8bpc(&mut out, source, ksize_vert, &bounds_vert, &kk_vert);
        Ok(out)
    } else {
        // none of the previous steps are performed, copying
        Ok(source.clone())
    }
}

/// `ImagingScaleAffine` (`Geometry.c:832`) as `_resize` calls it for NEAREST
/// (`ImagingTransform(AFFINE, filter 0, fill 1)`): the pixel at the centre of each output pixel.
fn scale_affine(im: &Image, xsize: usize, ysize: usize, box_: (f32, f32, f32, f32)) -> Image {
    // `a[0] = (double)(box[2] - box[0]) / xsize`, and the same for y; `a[2] = box[0]`, `a[5] = box[1]`.
    let a0 = (box_.2 - box_.0) as f64 / xsize as f64;
    let a4 = (box_.3 - box_.1) as f64 / ysize as f64;
    let a2 = box_.0 as f64;
    let a5 = box_.1 as f64;
    let coord = |v: f64| if v < 0.0 { -1 } else { v as i32 };
    let mut out = Image::new(im.mode, xsize, ysize);
    out.palette = im.palette.clone();
    let pixel = im.mode.pixelsize();
    let mut xo = a2 + a0 * 0.5;
    let mut yo = a5 + a4 * 0.5;
    // Pretabulate horizontal pixel positions
    let mut xintab = vec![0usize; xsize];
    let (mut xmin, mut xmax) = (xsize, 0usize);
    for x in 0..xsize {
        let xin = coord(xo);
        if xin >= 0 && xin < im.width as i32 {
            xmax = x + 1;
            if x < xmin {
                xmin = x;
            }
            xintab[x] = xin as usize;
        }
        xo += a0;
    }
    for y in 0..ysize {
        let yi = coord(yo);
        if yi >= 0 && yi < im.height as i32 {
            for x in xmin..xmax {
                let from = (yi as usize * im.width + xintab[x]) * pixel;
                let to = (y * xsize + x) * pixel;
                out.data[to..to + pixel].copy_from_slice(&im.data[from..from + pixel]);
            }
        }
        yo += a4;
    }
    out
}

/// `_resize` (`_imaging.c:1937`).
pub fn resize(
    im: &Image,
    size: (usize, usize),
    filter: Filter,
    box_: (f32, f32, f32, f32),
) -> Result<Image, String> {
    let (xsize, ysize) = size;
    if xsize < 1 || ysize < 1 {
        return Err("height and width must be > 0".to_owned());
    }
    if box_.0 < 0.0 || box_.1 < 0.0 {
        return Err("box offset can't be negative".to_owned());
    }
    if box_.2 > im.width as f32 || box_.3 > im.height as f32 {
        return Err("box can't exceed original image size".to_owned());
    }
    if box_.2 - box_.0 < 0.0 || box_.3 - box_.1 < 0.0 {
        return Err("box can't be empty".to_owned());
    }
    // If box's coordinates are int and box size matches requested size
    if box_.0 - (box_.0 as i32) as f32 == 0.0
        && box_.2 - box_.0 == xsize as f32
        && box_.1 - (box_.1 as i32) as f32 == 0.0
        && box_.3 - box_.1 == ysize as f32
    {
        // `ImagingCrop`
        let (x0, y0) = (box_.0 as usize, box_.1 as usize);
        let pixel = im.mode.pixelsize();
        let mut out = Image::new(im.mode, xsize, ysize);
        out.palette = im.palette.clone();
        for y in 0..ysize {
            let from = ((y0 + y) * im.width + x0) * pixel;
            out.data[y * xsize * pixel..(y + 1) * xsize * pixel]
                .copy_from_slice(&im.data[from..from + xsize * pixel]);
        }
        return Ok(out);
    }
    match filter {
        Filter::Nearest => Ok(scale_affine(im, xsize, ysize, box_)),
        Filter::Bicubic => resample_bicubic(im, xsize, ysize, box_),
    }
}
