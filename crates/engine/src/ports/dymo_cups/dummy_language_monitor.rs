// Ported from DYMO's CUPS driver 1.4.0 (reference/dymo-cups-drivers-1.4.0):
//   src/common/DummyLanguageMonitor.h, src/common/DummyLanguageMonitor.cpp
// DYMO LabelWriter Drivers, Copyright (C) 2008 Sanford L.P.
// This program is free software; you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation; either version 2 of the License,
// or (at your option) any later version (taken as version 3, so it combines with this program's AGPL-3.0).
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even
// the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.

//! `DummyLanguageMonitor.cpp`: a language monitor that does nothing (for a printer without a back
//! channel). The 450's filter does not use it (`raster2dymolw.cpp:100-103`, `IsBackchannelSupported()` is
//! true); the driver's own filter test does.

use super::printer_driver::{LanguageMonitor, SharedEnvironment};

/// `CDummyLanguageMonitor`.
pub struct DummyLanguageMonitor {
    #[allow(dead_code)] // :29 `Environment_`, kept and never used
    environment: SharedEnvironment,
}

impl DummyLanguageMonitor {
    /// `CDummyLanguageMonitor::CDummyLanguageMonitor(Environment)` (:29-32).
    pub fn new(environment: SharedEnvironment) -> Self {
        DummyLanguageMonitor { environment }
    }
}

impl LanguageMonitor for DummyLanguageMonitor {
    /// `StartDoc` (:38-41)
    fn start_doc(&mut self) {}
    /// `EndDoc` (:43-46)
    fn end_doc(&mut self) {}
    /// `StartPage` (:48-51)
    fn start_page(&mut self) {}
    /// `EndPage` (:53-56)
    fn end_page(&mut self) {}
    /// `ProcessData` (:60-63)
    fn process_data(&mut self, _data: &[u8]) {}
}
