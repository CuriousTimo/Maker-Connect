// Ported from DYMO's CUPS driver 1.4.0 (reference/dymo-cups-drivers-1.4.0):
//   src/common/Halftoning.h, src/common/Halftoning.cpp
// DYMO LabelWriter Drivers, Copyright (C) 2008 Sanford L.P.
// This program is free software; you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation; either version 2 of the License,
// or (at your option) any later version (taken as version 3, so it combines with this program's AGPL-3.0).
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even
// the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.

//! `Halftoning.cpp`: the base of the halftoning filters (`CHalftoneFilter`): image types and the colour
//! helpers. `halftone_filter` classes ([`ErrorDiffusionHalftoning`](super::error_diffusion_halftoning),
//! [`NllHalftoning`](super::nll_halftoning)) implement [`HalftoneFilter`].

use super::printer_driver::Buffer;

/// `Halftoning.h:36-41` `CHalftoneFilter::image_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageType {
    /// Black and White
    BW,
    /// four bytes per pixel, 8 bits per color, msb is not used (default on MacOSX)
    XRGB,
    /// three bytes per pixel, 8 bits per color (default on CUPS)
    RGB,
}

/// `Halftoning.h:43` `image_buffer_t`.
pub type ImageBuffer = Vec<Buffer>;

/// `Halftoning.h:84-98` `EHalftoneError`: thrown by the halftoning constructors for an output type other
/// than `itBW`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HalftoneError {
    /// `heUnsupportedImageType = 1`
    UnsupportedImageType = 1,
}

/// `CHalftoneFilter`'s state (:31-34) and its non-virtual functions.
#[derive(Debug, Clone, Copy)]
pub struct HalftoneBase {
    input_image_type: ImageType,
    output_image_type: ImageType,
}

impl HalftoneBase {
    /// `CHalftoneFilter::CHalftoneFilter` (:31-34).
    pub fn new(input_image_type: ImageType, output_image_type: ImageType) -> Self {
        HalftoneBase {
            input_image_type,
            output_image_type,
        }
    }

    /// `GetInputImageType` (:40-44).
    pub fn get_input_image_type(&self) -> ImageType {
        self.input_image_type
    }

    /// `GetOutputImageType` (:46-50).
    pub fn get_output_image_type(&self) -> ImageType {
        self.output_image_type
    }

    /// `RGBToGrayScale` (:52-67): "white should remain white".
    pub fn rgb_to_gray_scale(&self, r: u8, g: u8, b: u8) -> u8 {
        if r == 255 && g == 255 && b == 255 {
            255
        } else if r == 0 && g == 0 && b == 0 {
            0
        } else {
            let r = 0
                + ((r as i32 * 299) / 1000)
                + ((g as i32 * 587) / 1000)
                + ((b as i32 * 114) / 1000);
            if r > 255 {
                return 255;
            }
            r as u8
        }
    }

    /// `SetPixelBW` (:69-78): "set pixel pixelNo to pixelValue (0 - white, 1 - black)".
    pub fn set_pixel_bw(&self, buf: &mut [u8], pixel_no: i32, pixel_value: i32) {
        if pixel_value != 0 {
            buf[(pixel_no / 8) as usize] |= 1 << (7 - pixel_no % 8);
        } else {
            buf[(pixel_no / 8) as usize] &= !(1u8 << (7 - pixel_no % 8));
        }
    }

    /// `ExtractRGB(InputLine, PixelNo, R, G, B)` (:80-98).
    pub fn extract_rgb(&self, input_line: &[u8], pixel_no: i32) -> (u8, u8, u8) {
        let p = pixel_no as usize;
        match self.input_image_type {
            ImageType::XRGB => (
                input_line[4 * p + 1],
                input_line[4 * p + 2],
                input_line[4 * p + 3],
            ),
            ImageType::RGB => (
                input_line[3 * p],
                input_line[3 * p + 1],
                input_line[3 * p + 2],
            ),
            _ => unreachable!("assert(0)"),
        }
    }

    /// `CalcImageWidth` (:100-114).
    pub fn calc_image_width(&self, input_line: &[u8]) -> usize {
        match self.input_image_type {
            ImageType::XRGB => input_line.len() / 4,
            ImageType::RGB => input_line.len() / 3,
            _ => unreachable!("assert(0)"),
        }
    }

    /// `CalcBufferSize` (:117-131).
    pub fn calc_buffer_size(&self, image_width: usize) -> usize {
        match self.input_image_type {
            ImageType::XRGB => image_width * 4,
            ImageType::RGB => image_width * 3,
            _ => unreachable!("assert(0)"),
        }
    }

    /// `CalcOutputBufferSize` (:133-148).
    pub fn calc_output_buffer_size(&self, image_width: usize) -> usize {
        match self.output_image_type {
            ImageType::BW => {
                if image_width % 8 == 0 {
                    image_width / 8
                } else {
                    image_width / 8 + 1
                }
            }
            _ => unreachable!("assert(0)"),
        }
    }
}

/// The virtual interface of `CHalftoneFilter` (.h:49-55).
pub trait HalftoneFilter {
    /// `IsProcessLineSupported`: line-by-line interface.
    fn is_process_line_supported(&self) -> bool;
    /// `ProcessLine`.
    fn process_line(&mut self, input_line: &[u8], output_line: &mut Buffer);
    /// `ProcessImage(const image_buffer_t&, image_buffer_t&)`: full-image-at-once interface.
    fn process_image(&mut self, input_image: &ImageBuffer, output_image: &mut ImageBuffer);
}
