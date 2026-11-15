// Ported from DYMO's CUPS driver 1.4.0 (reference/dymo-cups-drivers-1.4.0):
//   src/common/ErrorDiffusionHalftoning.h, src/common/ErrorDiffusionHalftoning.cpp
// DYMO LabelWriter Drivers, Copyright (C) 2008 Sanford L.P.
// This program is free software; you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation; either version 2 of the License,
// or (at your option) any later version (taken as version 3, so it combines with this program's AGPL-3.0).
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even
// the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.

//! `ErrorDiffusionHalftoning.cpp`: the 450's default halftoning (`DymoHalftoning ErrorDiffusion`,
//! `lw450.ppd:424`, and the filter's choice when it is not `NLL`, `CupsFilter.h:140-141`).

use super::halftoning::{HalftoneBase, HalftoneError, HalftoneFilter, ImageBuffer, ImageType};
use super::printer_driver::Buffer;

/// `CErrorDiffusionHalftoning`.
pub struct ErrorDiffusionHalftoning {
    base: HalftoneBase,
    image_width: usize,     // image width in pixels
    errors: Vec<i32>,       // errors buffer
    gray_line: Vec<i32>,    // current line in gray scale color
    use_printer_color_space: bool, // if true then use 1 as black, 0 - as white; otherwise 0 is black 1 - white
}

impl ErrorDiffusionHalftoning {
    /// `CErrorDiffusionHalftoning::CErrorDiffusionHalftoning(Input, Output, UsePrinterColorSpace = true)`
    /// (.h:34, .cpp:29-34): throws for an output image type other than `itBW`.
    pub fn new(input_image_type: ImageType, output_image_type: ImageType, use_printer_color_space: bool) -> Result<Self, HalftoneError> {
        let base = HalftoneBase::new(input_image_type, output_image_type);
        if base.get_output_image_type() != ImageType::BW {
            return Err(HalftoneError::UnsupportedImageType);
        }
        Ok(ErrorDiffusionHalftoning { base, image_width: 0, errors: Vec::new(), gray_line: Vec::new(), use_printer_color_space })
    }

    /// `ProcessImage(const void* ImageData, ImageWidth, ImageHeight, LineDelta, OutputImage)` (:137-156):
    /// the image as a block of memory, `line_delta` bytes from one line to the next.
    pub fn process_image_data(&mut self, image_data: &[u8], image_width: usize, image_height: usize, line_delta: usize, output_image: &mut ImageBuffer) {
        output_image.clear();

        let buffer_size = self.base.calc_buffer_size(image_width);

        for i in 0..image_height {
            let input_line: Buffer = image_data[line_delta * i..line_delta * i + buffer_size].to_vec();

            output_image.push(Buffer::new());
            let last = output_image.len() - 1;
            self.process_line(&input_line, &mut output_image[last]);
        }
    }
}

impl HalftoneFilter for ErrorDiffusionHalftoning {
    /// `IsProcessLineSupported` (:41-45)
    fn is_process_line_supported(&self) -> bool {
        true
    }

    /// `ProcessLine` (:47-135)
    fn process_line(&mut self, input_line: &[u8], output_line: &mut Buffer) {
        // set image  width
        if self.image_width == 0 {
            self.image_width = self.base.calc_image_width(input_line);
        }
        let image_width = self.image_width;

        // check buffer size
        output_line.resize(self.base.calc_output_buffer_size(image_width), 0);
        for b in output_line.iter_mut() {
            *b = 0;
        }

        // initialize halftone errors array and line buffer
        if self.errors.is_empty() {
            self.errors.resize(image_width, 0);
        }

        if self.gray_line.is_empty() {
            self.gray_line.resize(image_width, 0);
        }

        // convert from RGB to GrayScale
        for i in 0..image_width {
            let (r, g, b) = self.base.extract_rgb(input_line, i as i32);
            self.gray_line[i] = self.base.rgb_to_gray_scale(r, g, b) as i32;
        }

        // apply errors from prev line
        for i in 0..image_width {
            // only if not black and white
            if self.gray_line[i] != 0 && self.gray_line[i] != 255 {
                if self.errors[i] + self.gray_line[i] >= 255 {
                    self.gray_line[i] = 255;
                } else if self.errors[i] + self.gray_line[i] <= 0 {
                    self.gray_line[i] = 0;
                } else {
                    self.gray_line[i] += self.errors[i];
                }
            }

            self.errors[i] = 0;
        }

        // compute output pixels and new errors
        for i in 0..image_width {
            let pixel_value = (self.gray_line[i] >= 128) as i32;
            let mut error = self.gray_line[i] - pixel_value * 255;

            if self.use_printer_color_space {
                self.base.set_pixel_bw(output_line, i as i32, (pixel_value == 0) as i32);
            } else {
                self.base.set_pixel_bw(output_line, i as i32, pixel_value);
            }

            // disribute error
            if i > 0 {
                self.errors[i - 1] += (error * 3) / 16;
            }

            self.errors[i] += (error * 5) / 16;

            if i + 1 < image_width {
                self.errors[i + 1] += error / 16;

                if self.gray_line[i + 1] != 0 && self.gray_line[i + 1] != 255 {
                    error = (error * 7) / 16;

                    if error + self.gray_line[i + 1] >= 255 {
                        self.gray_line[i + 1] = 255;
                    } else if error + self.gray_line[i + 1] <= 0 {
                        self.gray_line[i + 1] = 0;
                    } else {
                        self.gray_line[i + 1] += error;
                    }
                }
            }
        } // for all pixels
    }

    /// `ProcessImage(const std::vector<buffer_t>&, std::vector<buffer_t>&)` (:158-170).
    fn process_image(&mut self, input_image: &ImageBuffer, output_image: &mut ImageBuffer) {
        output_image.clear();

        let mut output_line = Buffer::new();

        for i in input_image {
            self.process_line(i, &mut output_line);
            output_image.push(output_line.clone());
        }
    }
}
