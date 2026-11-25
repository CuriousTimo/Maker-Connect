//! A port of Prusa-Connect-SDK-Printer to Rust.
//!
//! Reference: <https://github.com/prusa3d/Prusa-Connect-SDK-Printer>, branch `websockets` at
//! commit `c0d7d233`, ported file by file (`prusa/connect/printer/*.py`). The SDK has no licence
//! on GitHub ("Freeware" in its `setup.py`); the owner decided to port it (2026-11-12).
//!
//! Ported: `const`, `clock`, `util`, `errors`, `conditions`, `models`, `command`, `ws`,
//! `fileinfo`, `files` and `download` (with `pyos`, the CPython os calls they stand on), and the camera
//! modules `camera`, `camera_driver`, `camera_controller` and `camera_configurator` (with
//! `configparser`, CPython's, which the configurator stands on). Not ported: the Printer core
//! (`__init__.py`).

pub mod camera;
pub mod camera_configurator;
pub mod camera_controller;
pub mod camera_driver;
pub mod clock;
pub mod command;
pub mod conditions;
pub mod configparser;
pub mod consts;
pub mod download;
pub mod errors;
pub mod fileinfo;
pub mod files;
pub mod models;
pub mod pyos;
pub mod util;
pub mod ws;
