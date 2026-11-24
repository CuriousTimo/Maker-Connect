//! The ports (`docs/archive/plans/agent-engine.md`, step 3): each a port of one reference, the only code that
//! talks to its device.

/// The papers DYMO Connect's catalogue excludes for the 400 (#230): generated data, no device code.
// Generated (byte for byte its generator's output, checked): cargo fmt leaves it alone.
#[rustfmt::skip]
pub mod dymo_connect_lw400;
/// DYMO Connect's catalogue of the 450's rolls (HS2b): generated data, no device code.
// Generated (byte for byte its generator's output, checked): cargo fmt leaves it alone.
#[rustfmt::skip]
pub mod dymo_connect_lw450;
pub mod dymo_cups;
pub mod geometry;
#[cfg(feature = "devices")]
pub mod labelle;
#[cfg(feature = "devices")]
pub mod laserfoundry;
#[cfg(feature = "devices")]
pub mod octoprint;
#[cfg(feature = "devices")]
pub mod prusalink;
#[cfg(feature = "devices")]
pub mod usbscale;
