//! A port of Prusa-Connect-SDK-Printer to Rust.
//!
//! Reference: <https://github.com/prusa3d/Prusa-Connect-SDK-Printer>, branch `websockets` at
//! commit `c0d7d233`, ported file by file (`prusa/connect/printer/*.py`). The SDK has no licence
//! on GitHub ("Freeware" in its `setup.py`); the owner decided to port it (2026-11-12).
//!
//! Step A1: `const`, `clock`, `util`, `errors`, `conditions`, `models`, `command` and `ws`. The
//! Printer core (`__init__.py`), `files` and `download` follow; the camera modules are not
//! ported.

pub mod clock;
pub mod command;
pub mod conditions;
pub mod consts;
pub mod errors;
#[cfg(unix)]
pub mod fileinfo;
pub mod models;
#[cfg(unix)]
pub mod pyos;
pub mod util;
pub mod ws;
