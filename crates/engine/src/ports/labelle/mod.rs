//! The DYMO LabelManager PnP's port (`docs/agent-engine.md`, Step 3b): a port of **labelle**
//! (github.com/labelle-org/labelle, Apache-2.0, commit `5e8f1e5d4f2f4447c8827df96c174812275d678f`,
//! in `reference/labelle`). Its source files, one Rust file each, function by function:
//!
//! | labelle (`src/labelle/`) | here |
//! |---|---|
//! | `lib/constants.py` | `constants.rs` |
//! | `lib/utils.py` | `utils.rs` |
//! | `lib/devices/usb_device.py` | `usb_device.rs` |
//! | `lib/devices/device_manager.py` | `device_manager.rs` |
//! | `lib/devices/dymo_labeler.py` | `dymo_labeler.rs` |
//! | `lib/render_engines/{render_engine,render_context,picture,horizontally_combined,margins,print_payload}.py` | `render_engines/` |
//! | `cli/cli.py` (the print path of `default`) | `cli.rs` |
//!
//! labelle's runtime, which its code stands on (the versions installed in
//! `reference/.venv-audit`): **pyusb 1.3.1** (`pyusb.rs`, with `nusb_backend.rs` standing for its
//! libusb1 backend) and **Pillow 12.3.0** (`pillow/`, from its C source in
//! `reference/pillow-12.3.0`). `port.rs` is the engine's top edge (ours, listed in the doc).
//!
//! Licences: labelle is Apache-2.0 (Copyright (c) 2011 Sebastian J. Bronner and the labelle
//! contributors), pyusb BSD-3-Clause, Pillow HPND; each file names its source. This port is
//! AGPL-3.0 like the rest of Maker Connect (Apache-2.0 fits it). The old printer code of
//! `crates/label` is not a source.
//!
//! Not ported: `online_device_manager.py`, anything GUI, the text/QR/barcode/sample/preview
//! engines, the old `status_session`. Ours, the owner's exception (L11): while idle, `port.rs` polls
//! the printer every 15 s with one `ESC A` (`cli::poll_status`, `DymoLabeler::status_poll`).

pub mod cli;
pub mod constants;
pub mod device_manager;
pub mod dymo_labeler;
pub mod nusb_backend;
pub mod pillow;
pub mod port;
pub mod pyusb;
pub mod render_engines;
pub mod usb_device;
pub mod utils;

pub use port::DymoPnp;

#[cfg(test)]
mod tests;
