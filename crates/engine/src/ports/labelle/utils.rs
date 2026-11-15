//! `src/labelle/lib/utils.py` (labelle 5e8f1e5): `mm_to_px`, the one function of it the print
//! path calls. (Copyright (c) 2011 Sebastian J. Bronner, as `constants.rs`.)

use super::constants::PIXELS_PER_MM;

/// `mm_to_px` (`utils.py:43`).
pub fn mm_to_px(mm: f64) -> f64 {
    mm * PIXELS_PER_MM
}
