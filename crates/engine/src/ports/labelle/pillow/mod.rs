//! The part of **Pillow 12.3.0** that labelle runs for a picture label: its runtime, as the
//! owner approved it (Pillow and pyusb are what labelle's code stands on, like CPython for
//! OctoPrint). The C source it is ported from is in `reference/pillow-12.3.0/src/` (the version
//! installed in `reference/.venv-audit`); each function names its file and line.
//!
//! Pillow: Copyright (c) 1997-2011 by Secret Labs AB, Copyright (c) 1995-2011 by Fredrik Lundh
//! and contributors, Copyright (c) 2010 by Jeffrey A. Clark and contributors; licence HPND
//! (`reference/pillow-12.3.0/LICENSE`): "Permission to use, copy, modify and distribute this
//! software and its associated documentation for any purpose and without fee is hereby
//! granted, provided that the above copyright notice appears in all copies".
//!
//! What labelle calls (`render_engines/picture.py`, `margins.py`, `dymo_labeler.py`):
//! `Image.open` ([`open`], the `png` crate standing for Pillow's PNG decoder: ours),
//! `Image.resize`, `Image.convert("L")`, `ImageOps.invert`, `Image.convert("1")`,
//! `Image.new("1")`, `Image.paste`, `Image.transpose(ROTATE_270)` and `Image.tobytes`.

pub mod convert;
pub mod image;
pub mod open;
pub mod resample;

pub use image::{Image, Mode};
