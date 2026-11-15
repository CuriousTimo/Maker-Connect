// Ported from DYMO's CUPS driver 1.4.0 (reference/dymo-cups-drivers-1.4.0):
//   src/lw/CupsFilterLabelWriter.h, src/lw/CupsFilterLabelWriter.cpp
// DYMO LabelWriter Drivers, Copyright (C) 2008 Sanford L.P.
// This program is free software; you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation; either version 2 of the License,
// or (at your option) any later version (taken as version 3, so it combines with this program's AGPL-3.0).
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even
// the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.

//! `CupsFilterLabelWriter.cpp`: "DriverInitializer - setup driver properties on different job steps": what
//! the PPD's marked options and the page header set in the driver. `CDriverInitializerLabelWriter` and its
//! `WithLM` variant (what the 450 uses, `raster2dymolw.cpp:102`); the Twin Turbo ones are not ported.
//!
//! **The resolution trap** (:26-37): only "203dpi" and "203x138dpi" are matched, so for the 450's `300dpi`
//! and `300x600dpi` `SetResolution` is never called and the resolution enters only through
//! `HWResolution[1]` (`ProcessPageOptions`, :91). Ported as it is.

use super::cups::ppd::PpdFile;
use super::cups::raster::PageHeader;
use super::label_writer_driver::{Density, LabelWriterDriver, PaperType, Point, Quality, Resolution};
use super::label_writer_language_monitor::LabelWriterLanguageMonitor;
use super::stderr;

/// `CDriverInitializerLabelWriter::ProcessPPDOptions` (:26-76). (The `CDummyLanguageMonitor& LM` argument is
/// not used there.)
pub fn process_ppd_options(driver: &mut LabelWriterDriver, ppd: &PpdFile) {
    let choice = ppd.find_marked_choice("Resolution");
    if let Some(choice) = choice {
        if choice.name.eq_ignore_ascii_case("203dpi") {
            driver.set_resolution(Resolution::Res204);
        } else if choice.name.eq_ignore_ascii_case("203x138dpi") {
            driver.set_resolution(Resolution::Res136);
        }
    } else {
        stderr::write("WARNING: unable to get Resolution choice\n");
    }

    let choice = ppd.find_marked_choice("DymoPrintQuality");
    if let Some(choice) = choice {
        if choice.name.eq_ignore_ascii_case("Text") {
            driver.set_quality(Quality::Text);
        } else if choice.name.eq_ignore_ascii_case("Graphics") {
            driver.set_quality(Quality::BarcodeAndGraphics);
        }
    } else {
        stderr::write("WARNING: unable to get PrintQuality choice\n");
    }

    let choice = ppd.find_marked_choice("DymoPrintDensity");
    if let Some(choice) = choice {
        if choice.name.eq_ignore_ascii_case("Light") {
            driver.set_density(Density::Low);
        } else if choice.name.eq_ignore_ascii_case("Medium") {
            driver.set_density(Density::Medium);
        } else if choice.name.eq_ignore_ascii_case("Normal") {
            driver.set_density(Density::Normal);
        } else if choice.name.eq_ignore_ascii_case("Dark") {
            driver.set_density(Density::High);
        }
    } else {
        stderr::write("WARNING: unable to get PrintDensity choice\n");
    }

    if ppd.modelname.eq_ignore_ascii_case("DYMO LabelWriter 300")
        || ppd.modelname.eq_ignore_ascii_case("DYMO LabelWriter 310")
        || ppd.modelname.eq_ignore_ascii_case("DYMO LabelWriter 315")
    {
        driver.set_max_print_width(58);
    }

    if ppd.modelname.eq_ignore_ascii_case("DYMO LabelWriter 4XL") {
        driver.set_max_print_width(156);
    }

    if ppd.modelname.eq_ignore_ascii_case("DYMO LabelWriter SE450") {
        driver.set_max_print_width(56);
    }
}

/// `CDriverInitializerLabelWriter::ProcessPageOptions` (:78-95).
pub fn process_page_options(driver: &mut LabelWriterDriver, page_header: &PageHeader) {
    if page_header.cups_media_type == PaperType::Regular as u32 || page_header.cups_media_type == PaperType::Continuous as u32 {
        driver.set_paper_type(if page_header.cups_media_type == 0 { PaperType::Regular } else { PaperType::Continuous });
    } else {
        stderr::write(&format!("WARNING: Invalid value for cupsMediaType ({})\n", page_header.cups_media_type as i32));
        driver.set_paper_type(PaperType::Regular);
    }
    // unsigned arithmetic: PageSize[1] * HWResolution[1] / 72
    driver.set_page_height((page_header.page_size[1].wrapping_mul(page_header.hw_resolution[1]) / 72) as usize);

    // page offset
    driver.set_page_offset(Point { x: page_header.cups_integer[0] as i32, y: 0 });
}

/// `CDriverInitializerLabelWriterWithLM::ProcessPPDOptions` (:125-128).
pub fn process_ppd_options_with_lm(driver: &mut LabelWriterDriver, _lm: &mut LabelWriterLanguageMonitor, ppd: &PpdFile) {
    process_ppd_options(driver, ppd);
}

/// `CDriverInitializerLabelWriterWithLM::ProcessPageOptions` (:130-135).
pub fn process_page_options_with_lm(driver: &mut LabelWriterDriver, lm: &mut LabelWriterLanguageMonitor, page_header: &PageHeader) {
    process_page_options(driver, page_header);
    lm.set_paper_type(driver.get_paper_type());
}
