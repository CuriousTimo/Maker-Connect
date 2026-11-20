//! A port of gcode-metadata (prusa3d/gcode-metadata at 7d8f25d, py-gcode-metadata 0.2.1,
//! LGPLv2+, "(c) 2025 Prusa 3D", authors Ondřej Tůma, Michal Zoubek, Tomáš Jozífek, Šárka
//! Faloutová) to Rust: the library the Connect SDK reads a print file's metadata and
//! thumbnails with (`files.py`, `fileinfo.py`).
//!
//! The Python it stands on is CPython 3.13.12 ([`py`]).

pub mod py;
pub mod py_unicode;
