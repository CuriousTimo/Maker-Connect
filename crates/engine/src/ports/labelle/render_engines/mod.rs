//! `src/labelle/lib/render_engines/` (labelle 5e8f1e5): the render engines a picture label goes
//! through, one file for each of labelle's: `render_engine.py`, `render_context.py`,
//! `picture.py`, `horizontally_combined.py`, `margins.py`, `print_payload.py`.
//!
//! labelle: Copyright (c) 2011 Sebastian J. Bronner <waschtl@sbronner.com>; the files' own
//! notice: "Copying and distribution of this file, with or without modification, are permitted
//! in any medium without royalty provided the copyright notice and this notice are preserved."
//! Not ported: the other engines (text, QR, barcode, sample pattern, preview, empty).

pub mod horizontally_combined;
pub mod margins;
pub mod picture;
pub mod print_payload;
pub mod render_context;
pub mod render_engine;
