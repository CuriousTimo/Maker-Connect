//! The DYMO LabelWriter 450 port (`docs/archive/plans/agent-engine.md`, Step 3c): **DYMO's own CUPS driver 1.4.0**
//! (`reference/dymo-cups-drivers-1.4.0`, the official tarball from download.dymo.com, sha256
//! `c60797e7e986ca329f46e9a6ab1cb6382383952b15685ed69fd91f3c7ed64f71`), the filter `raster2dymolw` for
//! the plain LabelWriter 450 (`ppd/lw450.ppd`, `CLabelWriterDriver400`) the LabelWriter 320 (`ppd/lw320.ppd`, `CLabelWriterDriver`) and the LabelWriter 400 (`ppd/lw400.ppd`, `CLabelWriterDriver400`), ported file by file and function by function.
//!
//! Licence: the driver is GPL-2 "or any later version" (Copyright (C) 2008-2011 Sanford L.P.), taken as
//! GPL-3 so it combines with this program's AGPL-3.0; each ported file keeps its copyright and licence
//! notice. The runtime the driver runs in is CUPS (`reference/cups-2.4.20`, OpenPrinting tag `v2.4.20`,
//! commit `ef80eea`, Apache-2.0 with the GPL/LGPL exception), ported in [`cups`] where the driver uses it.
//!
//! This port has nothing in common with `ports::labelle` (the LabelManager PnP): no shared code.
//!
//! | Driver file | Here |
//! |---|---|
//! | `common/CommonTypedefs.h`, `common/PrinterDriver.h` | [`printer_driver`] |
//! | `lw/LabelWriterDriver.{h,cpp}` | [`label_writer_driver`] (`CLabelWriterDriver`, `CLabelWriterDriver400`) |
//! | `lw/LabelWriterLanguageMonitor.{h,cpp}` | [`label_writer_language_monitor`] |
//! | `lw/CupsFilterLabelWriter.{h,cpp}` | [`cups_filter_label_writer`] |
//! | `lw/raster2dymolw.cpp` | [`raster2dymolw`] |
//! | `common/CupsFilter.h` | [`cups_filter`] |
//! | `common/CupsPrintEnvironment.{h,cpp}` | [`cups_print_environment`] |
//! | `common/DummyLanguageMonitor.{h,cpp}` | [`dummy_language_monitor`] |
//! | `common/Halftoning.{h,cpp}` | [`halftoning`] |
//! | `common/ErrorDiffusionHalftoning.{h,cpp}` | [`error_diffusion_halftoning`] |
//! | `common/NonLinearLaplacianHalftoning.{h,cpp}` | [`nll_halftoning`] |
//! | `ppd/lw450.ppd` | [`cups::ppd_lw450`] (generated, checked by `scripts/dymo-labelwriter-450-check.py`) |
//! | `ppd/lw320.ppd` | [`cups::ppd_lw320`] (generated, checked by `scripts/dymo-labelwriter-320-check.py`) |
//! | `ppd/lw400.ppd` | [`cups::ppd_lw400`] (generated, checked by `scripts/dymo-labelwriter-400-check.py`) |
//! | `lw/tests/*`, `common/tests/*` | `tests` |
//!
//! **Not ported (not covered):** `CLabelWriterDriverTwinTurbo` and the Twin Turbo, Turbo, DUO and
//! LabelManager variants (`lw450t`, `lw450dl`, `lw450dt`, `lw450tt`, `lm450`, `se450` and the other PPDs);
//! the models other than the 450 and the 320 in `ProcessPPDOptions` (300, 310, 315, 4XL, SE450 widths) and
//! the Twin Turbo in `raster2dymolw.cpp`'s dispatch; the filter without a PPD or without the back channel
//! (`CDummyLanguageMonitor` is ported but only the tests use it); `ProcessDiagonal` and the block test
//! `IsInImage()` of the NLL halftoning (called only from a commented-out `ProcessImage`, :326-351);
//! `ProcessImage(const void*, ...)` of both halftonings except as the driver's own tests call it;
//! the language of the messages (the PPD's other languages).
//!
//! **Ours** (listed in `docs/archive/plans/agent-engine.md` Step 3c, W1 to W10): the top edge ([`port`]), the page made
//! from the PNG ([`page`]), the stand-ins for what CUPS and the scheduler give the filter
//! ([`cups_print_environment::FilterIo`], [`cups::usb_backend`]).

// Ported to the letter (CLAUDE.md, "port, don't paraphrase"): the reference's own structure is kept,
// so clippy's style, complexity and performance rewrites are not applied here; its correctness
// lints still hold.
#![allow(clippy::style, clippy::complexity, clippy::perf)]
pub mod cups;
#[cfg(feature = "devices")]
pub mod cups_filter;
#[cfg(feature = "devices")]
pub mod cups_filter_label_writer;
#[cfg(feature = "devices")]
pub mod cups_print_environment;
#[cfg(feature = "devices")]
pub mod dummy_language_monitor;
#[cfg(feature = "devices")]
pub mod error_diffusion_halftoning;
#[cfg(feature = "devices")]
pub mod halftoning;
#[cfg(feature = "devices")]
pub mod label_writer_driver;
#[cfg(feature = "devices")]
pub mod label_writer_language_monitor;
#[cfg(feature = "devices")]
pub mod nll_halftoning;
#[cfg(feature = "devices")]
pub mod page;
#[cfg(feature = "devices")]
pub mod port;
#[cfg(feature = "devices")]
pub mod printer_driver;
#[cfg(feature = "devices")]
pub mod raster2dymolw;
#[cfg(feature = "devices")]
pub mod stderr;
#[cfg(all(test, feature = "devices"))]
mod tests;

#[cfg(feature = "devices")]
pub use port::{LabelWriter320, LabelWriter400, LabelWriter450};
