//! A port of Prusa-Connect-SDK-Printer to Rust.
//!
//! Reference: <https://github.com/prusa3d/Prusa-Connect-SDK-Printer>, branch `websockets` at
//! commit `c0d7d233`, ported file by file (`prusa/connect/printer/*.py`). The SDK has no licence
//! on GitHub ("Freeware" in its `setup.py`); the owner decided to port it (2026-11-12).
//!
//! Ported: `const`, `clock`, `util`, `errors`, `conditions`, `models`, `command`, `ws`,
//! `fileinfo` and `download` (with [`pyos`], the CPython os calls they stand on). Not ported:
//! the Printer core (`__init__.py`), `files` and the camera modules.

pub mod clock;
pub mod command;
pub mod conditions;
pub mod consts;
#[cfg(unix)]
pub mod download;
pub mod errors;
#[cfg(unix)]
pub mod fileinfo;
pub mod models;
#[cfg(unix)]
pub mod pyos;
pub mod util;
pub mod ws;
