// Ported from DYMO's CUPS driver 1.4.0 (reference/dymo-cups-drivers-1.4.0):
//   src/common/NonLinearLaplacianHalftoning.h, src/common/NonLinearLaplacianHalftoning.cpp
// DYMO LabelWriter Drivers, Copyright (C) 2008 Sanford L.P.
// This program is free software; you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation; either version 2 of the License,
// or (at your option) any later version (taken as version 3, so it combines with this program's AGPL-3.0).
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even
// the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.

//! `NonLinearLaplacianHalftoning.cpp`: the `DymoHalftoning NLL` choice (`lw450.ppd:427`), "Nonlinear
//! Dithering". Not ported: `ProcessDiagonal` and `CNLLBlock::IsInImage()` (called only from the
//! commented-out `ProcessImage`, :326-351), `ProcessImage(const void*, ...)` (:279-284, "TODO:
//! non-implemented yet", does nothing).

use super::halftoning::{HalftoneBase, HalftoneError, HalftoneFilter, ImageBuffer, ImageType};
use super::printer_driver::Buffer;

/// `CNLLHalftoning`.
pub struct NllHalftoning {
    base: HalftoneBase,
    threshold: i32, // constant used to separate a block to classes using NLL
}

/// `CNLLBlock::point_t` (:82-86).
#[derive(Clone, Copy)]
struct PointT {
    x: i32,
    y: i32,
}

/// `CNLLBlock::PixelOffsets_` (:197-217). (The older tables in the comments, :100-195 and :232-254, are
/// not used.)
const PIXEL_OFFSETS: [PointT; 18] = [
    PointT { x: 0, y: 0 },
    PointT { x: -1, y: 1 },
    PointT { x: -1, y: -1 },
    PointT { x: 1, y: -1 },
    PointT { x: 1, y: 1 },
    PointT { x: -2, y: 0 },
    PointT { x: 2, y: 0 },
    PointT { x: 0, y: -2 },
    PointT { x: 0, y: 2 },
    PointT { x: -1, y: 0 },
    PointT { x: -2, y: -1 },
    PointT { x: -2, y: 1 },
    PointT { x: 0, y: -1 },
    PointT { x: 0, y: 1 },
    PointT { x: -3, y: 0 },
    PointT { x: -1, y: -2 },
    PointT { x: -1, y: 2 },
    PointT { x: 1, y: 0 },
];

/// `CNLLBlock::square_block_t` (:88-94) and `Squares_` (:219-229). `ReduceClasses(size_t, size_t)` uses
/// them; its two calls are commented out (:460-461), so it is ported and never called.
#[allow(dead_code)]
const SQUARES: [[usize; 4]; 8] = [
    [17, 8, 3, 13],
    [11, 3, 6, 10],
    [3, 13, 10, 1],
    [13, 4, 1, 18],
    [6, 10, 12, 2],
    [10, 1, 2, 14],
    [1, 18, 14, 5],
    [2, 14, 16, 9],
];

/// `CNLLBlock` (:29-98): "helper class defines image 'block' of 18 pixels".
struct NllBlock<'a> {
    parent: &'a NllHalftoning,
    image: &'a ImageBuffer,
    x1: i32,
    y1: i32,
    pixels: Vec<i32>,    // pixels' gray values
    classes: Vec<usize>, // pixels' classes
    image_width: usize,
    image_height: usize,
}

impl<'a> NllBlock<'a> {
    /// `CNLLBlock::CNLLBlock` (:439-445): "create a block from image with coordinates of pixel #1 at (x1, y1)".
    fn new(parent: &'a NllHalftoning, image: &'a ImageBuffer, x1: i32, y1: i32) -> Self {
        let image_width = parent.base.calc_image_width(&image[0]);
        let image_height = image.len();
        NllBlock {
            parent,
            image,
            x1,
            y1,
            pixels: vec![0; 18],
            classes: vec![0; 18],
            image_width,
            image_height,
        }
    }

    /// `FillBlock` (:447-455): "fill block information".
    fn fill_block(&mut self) {
        for i in 0..self.pixels.len() {
            self.fill_pixel(
                i + 1,
                self.x1 + PIXEL_OFFSETS[i].x,
                self.y1 + PIXEL_OFFSETS[i].y,
            );
        }
        self.reduce_classes();
    }

    /// `ReduceClasses()` (:457-462): "Split class 1 pixels to class 2 and class 5 to class 4"; both calls are
    /// commented out.
    fn reduce_classes(&mut self) {
        // ReduceClasses(1, 2);
        // ReduceClasses(5, 4);
    }

    /// `ReduceClasses(ClassFrom, ClassTo)` (:464-478). Never called (see above).
    #[allow(dead_code)]
    fn reduce_classes_from_to(&mut self, class_from: usize, class_to: usize) {
        for sq in SQUARES.iter() {
            if self.classes[sq[0] - 1] == class_from
                && self.classes[sq[1] - 1] == class_from
                && self.classes[sq[2] - 1] == class_from
                && self.classes[sq[3] - 1] == class_from
            {
                self.classes[sq[0] - 1] = class_to;
                self.classes[sq[2] - 1] = class_to;
            }
        }
    }

    /// `FillPixel` (:480-506): "fill info for one of 18 pixels".
    fn fill_pixel(&mut self, pixel_no: usize, x: i32, y: i32) {
        // fill pixels
        self.pixels[pixel_no - 1] = self.get_pixel_gray(x, y);

        // fill classes
        let nll = self.get_nll(x, y);
        let threshold = self.parent.get_threshold();

        // we added to new classes to those described in the papers
        // 0 - (same as 1) - it is set for black pixels, those should remeined black
        // 6 - (same as 5) - it is set for white pixels, those should remeined white
        if self.pixels[pixel_no - 1] == 0 {
            self.classes[pixel_no - 1] = 0;
        } else if self.pixels[pixel_no - 1] == 255 {
            self.classes[pixel_no - 1] = 6;
        } else if nll < -threshold {
            // as in papers
            self.classes[pixel_no - 1] = 1;
        } else if nll > threshold {
            self.classes[pixel_no - 1] = 5;
        } else {
            self.classes[pixel_no - 1] = 3;
        }
    }

    /// `GetPixelGray` (:508-519): "return grayscale value [0, 255] of pixel with coords (x, y)".
    fn get_pixel_gray(&self, x: i32, y: i32) -> i32 {
        if self.is_in_image(x, y) {
            let (r, g, b) = self.parent.base.extract_rgb(&self.image[y as usize], x);
            self.parent.base.rgb_to_gray_scale(r, g, b) as i32
        } else {
            255 // white
        }
    }

    /// `GetNLL` (:521-545): "return Laplacian value for pixel with coords (x, y)".
    fn get_nll(&self, x: i32, y: i32) -> i32 {
        let a = self.get_pixel_gray(x, y)
            - (self.get_pixel_gray(x - 1, y - 1)
                + self.get_pixel_gray(x + 1, y - 1)
                + self.get_pixel_gray(x - 1, y + 1)
                + self.get_pixel_gray(x + 1, y + 1))
                / 4;

        let b = self.get_pixel_gray(x, y)
            - (self.get_pixel_gray(x, y - 1)
                + self.get_pixel_gray(x, y + 1)
                + self.get_pixel_gray(x - 1, y)
                + self.get_pixel_gray(x + 1, y))
                / 4;

        if a > 0 && b > 0 {
            return a.min(b);
        }

        if a < 0 && b < 0 {
            return -(a.abs().min(b.abs()));
        }

        0
    }

    /// `GetBlockIntenseValue` (:548-558): "original number of pixels to draw in 'black'".
    fn get_block_intense_value(&self) -> usize {
        let mut intense: usize = 128;
        for p in &self.pixels {
            intense = intense.wrapping_add(*p as usize);
        }

        18 - (intense / 255).min(18)
    }

    /// `OutputBlock` (:560-577).
    fn output_block(&mut self, output_image: &mut ImageBuffer) {
        let mut remained_pixels = self.get_block_intense_value();

        // output all pixels for class 0
        let pixel_count = self.output_class(0, 18, output_image);

        if pixel_count < remained_pixels {
            remained_pixels -= pixel_count;
            remained_pixels -= self.output_class(1, remained_pixels, output_image);
            remained_pixels -= self.output_class(2, remained_pixels, output_image);
            remained_pixels -= self.output_class(3, remained_pixels, output_image);
            remained_pixels -= self.output_class(4, remained_pixels, output_image);
            let _ = remained_pixels;
        }
    }

    /// `OutputClass` (:579-610): "output Pixels of specific class; return number of pixels drawn".
    fn output_class(
        &mut self,
        class_no: usize,
        max_pixels_to_output: usize,
        output_image: &mut ImageBuffer,
    ) -> usize {
        if max_pixels_to_output == 0 {
            return 0;
        }

        // collect all pixels of a class
        let mut pixels: Vec<usize> = Vec::new();
        for (i, c) in self.classes.iter().enumerate() {
            if *c == class_no {
                pixels.push(i + 1);
            }
        }

        // output pixels
        let mut result = 0;
        for p in pixels {
            if result < max_pixels_to_output {
                self.output_pixel(p, output_image);
                result += 1;
            } else {
                break;
            }
        }

        result
    }

    /// `OutputPixel(size_t PixelNo)` (:612-616).
    fn output_pixel(&mut self, pixel_no: usize, output_image: &mut ImageBuffer) {
        self.output_pixel_xy(
            self.x1 + PIXEL_OFFSETS[pixel_no - 1].x,
            self.y1 + PIXEL_OFFSETS[pixel_no - 1].y,
            output_image,
        );
    }

    /// `OutputPixel(int x, int y)` (:618-623).
    fn output_pixel_xy(&mut self, x: i32, y: i32, output_image: &mut ImageBuffer) {
        if self.is_in_image(x, y) {
            self.parent
                .base
                .set_pixel_bw(&mut output_image[y as usize], x, 1);
        }
    }

    /// `IsInImage(int x, int y)` (:635-639): "return true if pixel (x, y) is inside image".
    fn is_in_image(&self, x: i32, y: i32) -> bool {
        x >= 0 && (x as usize) < self.image_width && y >= 0 && (y as usize) < self.image_height
    }
}

impl NllHalftoning {
    /// `CNLLHalftoning::CNLLHalftoning(Threshold, Input, Output)` (:256-261): throws for an output image
    /// type other than `itBW`.
    pub fn new(
        threshold: i32,
        input_image_type: ImageType,
        output_image_type: ImageType,
    ) -> Result<Self, HalftoneError> {
        let base = HalftoneBase::new(input_image_type, output_image_type);
        if base.get_output_image_type() != ImageType::BW {
            return Err(HalftoneError::UnsupportedImageType);
        }
        Ok(NllHalftoning { base, threshold })
    }

    /// `GetThreshold` (:429-433).
    pub fn get_threshold(&self) -> i32 {
        self.threshold
    }
}

impl HalftoneFilter for NllHalftoning {
    /// `IsProcessLineSupported` (:267-271)
    fn is_process_line_supported(&self) -> bool {
        false
    }

    /// `ProcessLine` (:273-277): does nothing.
    fn process_line(&mut self, _input_line: &[u8], _output_line: &mut Buffer) {}

    /// `ProcessImage` (:288-324)
    fn process_image(&mut self, input_image: &ImageBuffer, output_image: &mut ImageBuffer) {
        output_image.clear();
        if input_image.is_empty() {
            return;
        }

        let image_width = self.base.calc_image_width(&input_image[0]);
        let image_height = input_image.len();

        // create an empty output image
        let empty_line: Buffer = vec![0; image_width / 8 + 1];
        output_image.resize(image_height, empty_line);

        // split the image to 18-pixels block
        let row_count = (input_image.len() + 1) / 3 + 1;
        for r in 0..row_count {
            // get coordinates of pixel #1
            let mut x1: usize = if r % 2 != 0 { 3 } else { 0 };
            let y1: usize = 3 * r;

            // for all blocks in the row
            // both leftest and rightest pixels of the block is
            // (`x1 - 3` is a size_t: it wraps below 0)
            while x1.wrapping_sub(3) < image_width || x1 + 2 < image_width {
                let mut block = NllBlock::new(self, input_image, x1 as i32, y1 as i32);

                block.fill_block();
                block.output_block(output_image);

                // advance to next block
                x1 += 6;
            } // for blocks in the row
        } // for rows
    }
}
