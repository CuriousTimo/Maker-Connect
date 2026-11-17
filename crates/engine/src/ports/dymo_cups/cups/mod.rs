//! The driver's runtime: CUPS (`reference/cups-2.4.20`, OpenPrinting tag `v2.4.20`, commit `ef80eea`;
//! Apache-2.0 with the GPL/LGPL exception, `reference/cups-2.4.20/LICENSE` and `NOTICE`). Ported where
//! `raster2dymolw` uses it; each function names its file:line there. Runtime, to approve (proxy)
//! (`docs/agent-engine.md`, Step 3c). Copyright 2007-2019 Apple Inc., 2020-2025 OpenPrinting.
//!
//! | CUPS file | Here |
//! |---|---|
//! | `cups/options.c` | [`options`]: `cupsParseOptions` |
//! | `cups/ppd-mark.c`, `ppd-page.c`, `ppd-emit.c` | [`ppd`]: `ppdMarkDefaults`, `ppdMarkOption`, `cupsMarkOptions`, `ppdFindMarkedChoice`, `ppdFindChoice`, `ppdFindOption`, `ppdPageSize`, `ppdEmitString` |
//! | `cups/raster-interpret.c` | [`raster`]: `cupsRasterInterpretPPD` |
//! | `cups/raster-stream.c`, `raster-stubs.c` | [`raster`]: `cupsRasterReadHeader2`, `cupsRasterReadPixels` (an in-memory page, W10) |
//! | `cups/backchannel.c` | [`usb_backend::BackChannel`]: `cupsBackChannelRead` |
//! | `backend/usb-libusb.c` | [`usb_backend`]: `print_device`, `find_device`, `open_device`, `print_cb`, `read_thread` |
//! | the PPD | [`ppd_lw450`] (generated from `ppd/lw450.ppd`) |
//!
//! Not ported (not covered): `ppdOpenFile` and the PPD parser (the plain 450's values are
//! [`ppd_lw450`], generated and checked against the PPD); `ppdConflicts` (only flags conflicts, marks
//! nothing); `cupsMarkOptions`'s mapping of `media`, `output-bin`, `print-quality`, `sides`,
//! `multiple-document-handling`, `finishings`, `APPrinterPreset`, `mirror` (needs the PPD cache or
//! attributes; the job passes no options); the `_cupsRasterExecPS` PostScript machine beyond the
//! `setpagedevice` dictionaries of the 450's choices; custom page sizes; the raster stream's file format
//! (header bytes, PackBits, byte swapping); the side channel (`CUPS_SC_FD`) and `usb-libusb.c`'s quirks
//! file, `get_device_id`'s 1284 parsing, `soft_reset` and the debug options `usb-unidir`,
//! `usb-no-reattach`.

pub mod options;
pub mod ppd;
pub mod ppd_lw450;
pub mod raster;
#[cfg(feature = "devices")]
pub mod usb_backend;
