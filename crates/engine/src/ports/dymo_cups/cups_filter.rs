// Ported from DYMO's CUPS driver 1.4.0 (reference/dymo-cups-drivers-1.4.0):
//   src/common/CupsFilter.h
// DYMO LabelWriter Drivers, Copyright (C) 2008 Sanford L.P.
// This program is free software; you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation; either version 2 of the License,
// or (at your option) any later version (taken as version 3, so it combines with this program's AGPL-3.0).
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even
// the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.

//! `CupsFilter.h`: "Generic Cups filter" `CCupsFilter<D, DI, LM>`. Here for the 450's instance
//! (`raster2dymolw.cpp:100-103`): `CCupsFilter<CLabelWriterDriver400, CDriverInitializerLabelWriterWithLM,
//! CLabelWriterLanguageMonitor>`, and the 320's (`raster2dymolw.cpp:113-116`): the same with `CLabelWriterDriver`
//! (the `class` argument). The other instances are not ported.
//!
//! The raster comes in as an in-memory [`CupsRaster`] instead of `open(argv[6])`/stdin and `cupsRasterOpen`
//! (W10); the page's rows are what `cupsRasterReadPixels` gives.

use super::cups::options::{CupsOption, cups_parse_options};
use super::cups::raster::CupsRaster;
use super::cups_filter_label_writer::{process_page_options_with_lm, process_ppd_options_with_lm};
use super::cups_print_environment::{
    CupsPrintEnvironmentForDriver, CupsPrintEnvironmentForLm, SharedIo,
};
use super::error_diffusion_halftoning::ErrorDiffusionHalftoning;
use super::halftoning::{HalftoneFilter, ImageBuffer, ImageType};
use super::label_writer_driver::{Class, LabelWriterDriver};
use super::label_writer_language_monitor::LabelWriterLanguageMonitor;
use super::nll_halftoning::NllHalftoning;
use super::printer_driver::{Buffer, JobStatus, LanguageMonitor, PrintEnvironment, PrinterDriver};
use super::stderr;
use std::cell::RefCell;
use std::rc::Rc;

/// `CCupsFilter` (:40-56).
pub struct CupsFilter {
    io: SharedIo,
    print_environment_for_lm: Rc<RefCell<CupsPrintEnvironmentForLm>>,
    language_monitor: Rc<RefCell<LabelWriterLanguageMonitor>>,
    #[allow(dead_code)] // :52 `PrintEnvironmentForDriver_`: owned by the driver here
    print_environment_for_driver: Rc<RefCell<CupsPrintEnvironmentForDriver>>,
    driver: LabelWriterDriver,
    halftoning_method: String,
}

impl CupsFilter {
    /// `CCupsFilter::CCupsFilter` (:59-65): the environment of the monitor, the monitor, the environment of
    /// the driver (which forwards to the monitor), the driver. `device_uri` is `getenv("DEVICE_URI")`.
    pub fn new(io: SharedIo, class: Class) -> Self {
        let device_uri = io.borrow().getenv("DEVICE_URI");
        let print_environment_for_lm =
            Rc::new(RefCell::new(CupsPrintEnvironmentForLm::new(io.clone())));
        let language_monitor = Rc::new(RefCell::new(LabelWriterLanguageMonitor::new(
            print_environment_for_lm.clone(),
            true,
            10,
            device_uri,
        )));
        let print_environment_for_driver = Rc::new(RefCell::new(
            CupsPrintEnvironmentForDriver::new(io.clone(), language_monitor.clone()),
        ));
        let driver = match class {
            // raster2dymolw.cpp:100-103
            Class::Driver400 => LabelWriterDriver::new400(print_environment_for_driver.clone()),
            // raster2dymolw.cpp:113-116
            Class::Driver => LabelWriterDriver::new(print_environment_for_driver.clone()),
        };
        CupsFilter {
            io,
            print_environment_for_lm,
            language_monitor,
            print_environment_for_driver,
            driver,
            halftoning_method: String::new(),
        }
    }

    /// The monitor and its environment, for the telemetry (W7, ours).
    pub fn language_monitor(&self) -> Rc<RefCell<LabelWriterLanguageMonitor>> {
        self.language_monitor.clone()
    }

    /// The monitor's environment (W7, ours).
    pub fn print_environment_for_lm(&self) -> Rc<RefCell<CupsPrintEnvironmentForLm>> {
        self.print_environment_for_lm.clone()
    }

    /// `CCupsFilter::Run(int argc, char* argv[])` (:67-210). `argv` is `argv[0]`... as the scheduler gives it
    /// (`job-id user title copies options [file]`, so 6 or 7 entries with the program name); the file is
    /// not opened (W10).
    pub fn run(&mut self, argv: &[String], raster: CupsRaster) -> i32 {
        let mut raster_data = raster;
        if argv.len() < 6 || argv.len() > 7 {
            stderr::write("ERROR: using: <filter> job-id user title copies options [file]\n");
            return 1;
        }

        self.init_document(&argv[5]);

        self.language_monitor.borrow_mut().start_doc();
        self.driver.start_doc();

        let mut page = 0;
        while let Some(page_header) = raster_data.cups_raster_read_header2() {
            page += 1;

            stderr::write(&format!("PAGE: {} 1\n", page));
            stderr::write(&format!(
                "DEBUG: ----- PageHeader.cupsBytesPerLine = {}\n",
                page_header.cups_bytes_per_line as i32
            ));
            stderr::write(&format!(
                "DEBUG: ----- PageHeader.cupsBitsPerColor = {}\n",
                page_header.cups_bits_per_color as i32
            ));
            stderr::write(&format!(
                "DEBUG: ----- PageHeader.cupsBitsPerPixel = {}\n",
                page_header.cups_bits_per_pixel as i32
            ));
            stderr::write(&format!(
                "DEBUG: ----- PageHeader.cupsColorOrder = {}\n",
                page_header.cups_color_order as i32
            ));
            stderr::write(&format!(
                "DEBUG: ----- PageHeader.cupsHeight = {}\n",
                page_header.cups_height as i32
            ));

            let mut buffer: Buffer = vec![0; page_header.cups_bytes_per_line as usize];

            process_page_options_with_lm(
                &mut self.driver,
                &mut self.language_monitor.borrow_mut(),
                &page_header,
            );
            self.language_monitor.borrow_mut().start_page();

            if self.print_environment_for_lm.borrow_mut().get_job_status() != JobStatus::Ok {
                break;
            }

            self.driver.start_page();

            let mut input_image: ImageBuffer = Vec::new();
            let mut output_line = Buffer::new();

            let use_custom_halftoning = page_header.cups_bits_per_pixel > 1;
            let mut is_process_line_supported = true;

            let mut h: Option<Box<dyn HalftoneFilter>> = None;
            if use_custom_halftoning {
                if self.halftoning_method == "NLL" {
                    h = Some(Box::new(
                        NllHalftoning::new(5, ImageType::RGB, ImageType::BW).expect("itBW"),
                    ));
                } else {
                    // error diffusion is default
                    h = Some(Box::new(
                        ErrorDiffusionHalftoning::new(ImageType::RGB, ImageType::BW, true)
                            .expect("itBW"),
                    ));
                }

                is_process_line_supported = h.as_ref().unwrap().is_process_line_supported();
            }

            for y in 0..page_header.cups_height as usize {
                if (y & 15) == 0 {
                    stderr::write(&format!(
                        "INFO: Printing page {}, {}% complete...\n",
                        page,
                        (100 * y / page_header.cups_height as usize) as i32
                    ));
                }

                for b in buffer.iter_mut() {
                    *b = 0;
                }

                let bytes_read = raster_data.cups_raster_read_pixels(&mut buffer);
                if bytes_read != page_header.cups_bytes_per_line {
                    stderr::write(&format!(
                        "Error: cupsRasterReadPixels() failed: expected {} read, actually was {}",
                        page_header.cups_bytes_per_line as i32, bytes_read as i32
                    ));

                    break;
                }

                // apply halftoning
                if use_custom_halftoning {
                    if is_process_line_supported {
                        h.as_mut().unwrap().process_line(&buffer, &mut output_line);
                        self.driver.process_raster_line(&output_line);
                    } else {
                        input_image.push(buffer.clone()); // cache for later processing
                    }
                } else {
                    self.driver.process_raster_line(&buffer); // process line as-is, because it is already B&W
                }
            } // all lines

            // process cached image by custom halftoning if needed
            if use_custom_halftoning && !is_process_line_supported {
                let mut output_image: ImageBuffer = Vec::new();
                h.as_mut()
                    .unwrap()
                    .process_image(&input_image, &mut output_image);
                for line in &output_image {
                    self.driver.process_raster_line(line);
                }
            }

            self.driver.end_page();
            self.language_monitor.borrow_mut().end_page();
        }

        self.driver.end_doc();
        self.language_monitor.borrow_mut().end_doc();

        if page == 0 {
            stderr::write("ERROR: No pages found!\n");
        } else {
            stderr::write("INFO: Ready to print.\n");
        }

        (page == 0) as i32
    }

    /// `CCupsFilter::InitDocument(const char* opts)` (:212-249).
    fn init_document(&mut self, opts: &str) {
        stderr::write(&format!(
            "DEBUG: -----------------------------options are: {}\n",
            opts
        ));

        let Some(mut ppd) = self.io.borrow().ppd_open_file() else {
            stderr::write("WARNING: Unable to open ppd file, use default settings - \n");
            return;
        };

        let mut options: Vec<CupsOption> = Vec::new();
        cups_parse_options(opts, &mut options);

        ppd.mark_defaults();

        // mark general options as ppd options
        for o in &options {
            ppd.mark_option(&o.name, &o.value);
        }

        // do CUPS specific
        ppd.cups_mark_options(&options);

        process_ppd_options_with_lm(
            &mut self.driver,
            &mut self.language_monitor.borrow_mut(),
            &ppd,
        );

        // extract halftoning method used
        if let Some(choice) = ppd.find_marked_choice("DymoHalftoning") {
            self.halftoning_method = choice.name.to_owned();
        }
    }
}
