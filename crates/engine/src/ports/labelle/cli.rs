//! `src/labelle/cli/cli.py` (labelle 5e8f1e5): what its `default` command does to print a
//! picture on the printer, `labelle --picture FILE --tape-size-mm N`: lines 449-504 (the
//! options' defaults, the picture engine), 569-580 (`mm_to_payload_px`, `get_device_manager`,
//! `find_and_select_device`, `device.setup()`), 583-612 (the labeler, the render and `print`).
//!
//! labelle: Copyright (c) 2011 Sebastian J. Bronner <waschtl@sbronner.com>; the file's own
//! notice: "Copying and distribution of this file, with or without modification, are permitted
//! in any medium without royalty provided the copyright notice and this notice are preserved."
//!
//! The options are labelle's defaults (`cli.py:154` `justify` LEFT, `:166`-`:184` `min_length`
//! none → 0, no `max_length`, no `fixed_length`, `:227` `margin_px` = `DEFAULT_MARGIN_PX`):
//! ours, as the owner is to approve (they are the margins labelle adds to a picture).
//!
//! **Ours:** an error after the device is set up releases it (`dispose`): what labelle's
//! process exit does (the OS closes the device). Errors are given as their messages; labelle's
//! CLI prints `Error: {e}` for `DeviceManagerNoDevices` and a traceback for the rest.

use super::constants::{DEFAULT_MARGIN_PX, Direction, PIXELS_PER_MM};
use super::device_manager::{self, DeviceManager};
use super::dymo_labeler::{self, DymoLabeler};
use super::pyusb::Backend;
use super::render_engines::horizontally_combined;
use super::render_engines::picture::PictureRenderEngine;
use super::render_engines::print_payload::PrintPayloadRenderEngine;
use super::render_engines::render_context::RenderContext;
use std::rc::Rc;

/// `mm_to_payload_px` (`cli.py:57`): `max(0, (mm * PIXELS_PER_MM) - margin * 2)`.
fn mm_to_payload_px(mm: f64, margin: f64) -> f64 {
    f64::max(0.0, (mm * PIXELS_PER_MM) - margin * 2.0)
}

/// Why a print did not happen, as the message labelle gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError(pub String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a print found of the printer: ours, for the port's `Gone` and `Back`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Printer {
    /// `DeviceManagerNoDevices` or no matching device: none there.
    Absent,
    /// `find_and_select_device` gave one.
    Present,
}

/// `get_device_manager()`, `find_and_select_device()` and `device.setup()` (`cli.py:577-580`): the
/// printer found and set up, or why not (`printer` says whether none was there).
fn open_printer(
    backend: Rc<dyn Backend>,
    printer: &mut Option<Printer>,
) -> Result<(DeviceManager, super::device_manager::SharedDevice), CliError> {
    let mut device_manager = DeviceManager::new(backend);
    device_manager.scan().map_err(|e| {
        if matches!(e, device_manager::Error::NoDevices(_)) {
            *printer = Some(Printer::Absent);
        }
        CliError(e.to_string())
    })?;
    let device = device_manager
        .find_and_select_device()
        .map_err(|e: device_manager::Error| {
            if e == device_manager::Error::Manager("No matching devices found".to_owned()) {
                *printer = Some(Printer::Absent);
            }
            CliError(e.to_string())
        })?;
    *printer = Some(Printer::Present);
    device
        .borrow_mut()
        .setup()
        .map_err(|e| CliError(e.to_string()))?;
    // The manager is returned too: it lives as long as the command does (`get_device_manager()`).
    Ok((device_manager, device))
}

/// **Ours** (L11, the owner's exception of 2026-11-16): the idle status poll. The printer is
/// opened as the print path does ([`open_printer`]), one `ESC A` is sent and its answer read
/// ([`DymoLabeler::status_poll`]), the printer is released. An error after the printer is set up
/// releases it, as the print path does.
pub fn poll_status(
    backend: Rc<dyn Backend>,
    heard: &mut dyn FnMut(&[u8]),
    printer: &mut Option<Printer>,
) -> Result<(), CliError> {
    let (_device_manager, device) = open_printer(backend, printer)?;
    let result =
        DymoLabeler::status_poll(&device.borrow(), heard).map_err(|e| CliError(e.to_string()));
    if result.is_err() {
        device.borrow().dispose();
    }
    result
}

/// What the print path does when the user's command line is `labelle --picture <picture>
/// --tape-size-mm <tape_size_mm>` (output PRINTER). `heard` is given every answer the printer
/// gives (ours).
pub fn print_picture(
    backend: Rc<dyn Backend>,
    picture: &str,
    tape_size_mm: Option<i64>,
    heard: &mut dyn FnMut(&[u8]),
    printer: &mut Option<Printer>,
) -> Result<(), CliError> {
    let justify = Direction::Left;
    let margin_px = DEFAULT_MARGIN_PX;
    let min_length = 0.0;
    let (min_label_mm_len, max_label_mm_len): (f64, Option<f64>) = (min_length, None);

    // `render_engines.append(PictureRenderEngine(picture))` (`cli.py:503`): before the device.
    let picture_engine = PictureRenderEngine::new(picture).map_err(|e| CliError(e.to_string()))?;

    let min_payload_len_px = mm_to_payload_px(min_label_mm_len, margin_px);
    let max_payload_len_px = max_label_mm_len.map(|mm| mm_to_payload_px(mm, margin_px));

    let (_device_manager, device) = open_printer(backend, printer)?;

    let result = (|| {
        let dymo_labeler = DymoLabeler::new(tape_size_mm)
            .map_err(|e: dymo_labeler::Error| CliError(e.to_string()))?;
        let render_context = RenderContext::new(dymo_labeler.height_px());
        let render = PrintPayloadRenderEngine::new(
            justify,
            margin_px,
            dymo_labeler.labeler_margin_px(),
            max_payload_len_px,
            Some(min_payload_len_px),
        );
        // `HorizontallyCombinedRenderEngine([engine]).render(context)`, then the margins.
        let payload = picture_engine
            .render(&render_context)
            .map_err(|e| CliError(e.to_string()))?;
        let payload = horizontally_combined::render(vec![payload]).map_err(CliError)?;
        let (bitmap, _) = render
            .render_with_meta(&payload)
            .map_err(|e| CliError(e.to_string()))?;
        dymo_labeler
            .print(&device.borrow(), &bitmap, heard)
            .map_err(|e| CliError(e.to_string()))
    })();
    if result.is_err() {
        // What the end of labelle's process does: the OS lets go of the device.
        device.borrow().dispose();
    }
    result
}
