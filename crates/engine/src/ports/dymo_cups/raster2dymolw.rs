// Ported from DYMO's CUPS driver 1.4.0 (reference/dymo-cups-drivers-1.4.0):
//   src/lw/raster2dymolw.cpp
// DYMO LabelWriter Drivers, Copyright (C) 2008 Sanford L.P.
// This program is free software; you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation; either version 2 of the License,
// or (at your option) any later version (taken as version 3, so it combines with this program's AGPL-3.0).
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even
// the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.

//! `IsBackchannelSupported` (:44-53) returns true, so the monitor with the back channel is the one used.
//!
//! `raster2dymolw.cpp`: `main` of the filter: which driver, initializer and monitor the PPD's model name
//! takes. Only the plain LabelWriter 450's branch is here (:92-110, with `IsBackchannelSupported()`
//! true, :44-53); the filter without a PPD (:60-75), the Twin Turbo (:78-91) and the other models
//! (:111-123) are not ported.

use super::cups::raster::CupsRaster;
use super::cups_filter::CupsFilter;
use super::cups_print_environment::SharedIo;
use super::stderr;

/// `main(argc, argv)` (:55-125) for the 450: `argv` is as the scheduler passes it. Returns the exit status.
pub fn main(io: SharedIo, argv: &[String], raster: CupsRaster, filter_out: &mut Option<CupsFilter>) -> i32 {
    stderr::write("DEBUG: starting (raster2dymolw)\n");

    let Some(ppd) = io.borrow().ppd_open_file() else {
        stderr::write("WARNING: Unable to open ppd file, use default settings - \n");
        // :65-69 the filter for `CLabelWriterDriver` without a PPD: not ported
        return 1;
    };

    let m = ppd.modelname;
    if m.eq_ignore_ascii_case("DYMO LabelWriter Twin Turbo") || m.eq_ignore_ascii_case("DYMO LabelWriter 450 Twin Turbo") {
        // :78-91 not ported
        stderr::write("ERROR: the Twin Turbo filter is not ported\n");
        1
    } else if m.eq_ignore_ascii_case("DYMO LabelWriter 400")
        || m.eq_ignore_ascii_case("DYMO LabelWriter 400 Turbo")
        || m.eq_ignore_ascii_case("DYMO LabelWriter DUO Label")
        || m.eq_ignore_ascii_case("DYMO LabelWriter 4XL")
        || m.eq_ignore_ascii_case("DYMO LabelWriter 450")
        || m.eq_ignore_ascii_case("DYMO LabelWriter 450 Turbo")
        || m.eq_ignore_ascii_case("DYMO LabelWriter 450 DUO Label")
    {
        // :100-103 `IsBackchannelSupported()`
        let mut filter = CupsFilter::new(io);
        let status = filter.run(argv, raster);
        *filter_out = Some(filter);
        status
    } else {
        // :111-123 not ported
        stderr::write("ERROR: the filter for this model is not ported\n");
        1
    }
}
