//! The ports (`docs/agent-engine.md`, step 3): each a port of one reference, the only code that
//! talks to its device.

#[cfg(feature = "devices")]
pub mod octoprint;
#[cfg(feature = "devices")]
pub mod labelle;
#[cfg(feature = "devices")]
pub mod laserfoundry;
pub mod dymo_cups;
#[cfg(feature = "devices")]
pub mod usbscale;
pub mod geometry;
/// DYMO Connect's catalogue of the 450's rolls (HS2b): generated data, no device code.
pub mod dymo_connect_lw450;
