//! The DYMO LabelWriter 400 and 450's geometry as the hub uses it (`hub/src/profiles.rs:229-230`): the
//! head's dots across the label and dots for a length, at 300 dpi.
//!
//! The printer itself, its protocol and its USB are the engine's port of DYMO's own CUPS driver
//! (`crates/engine/src/ports/dymo_cups/`, `docs/archive/plans/agent-engine.md` Step 3c); the code that was here for
//! them (the encoder, the status byte, the USB sending) is gone. The hub should take these two numbers
//! from the port's constants (the PPD's, `300dpi` and the imageable areas); that waits for the hub side.

pub const DPI: f64 = 300.0;
/// Dots across the head (56 mm).
pub const HEAD_DOTS: usize = 672;

/// Dots for a length in mm, at 300 dpi.
pub fn dots_for_mm(mm: f64) -> usize {
    (mm * DPI / 25.4).round() as usize
}
