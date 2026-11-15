//! The laser port (`docs/agent-engine.md`, Step 3d): the engine's [`crate::Port`] for
//! `--mode grbl`, over `maker-connect-grbl` (`crates/grbl`), which is LaserFoundry v0.5.0
//! (61febdb, `laserfoundry-device` and `laserfoundry-gcode`, MIT OR Apache-2.0) and the link
//! logic of its app. **One reference:** LaserFoundry; every byte to the machine is its `Link`'s.
//! What is ours (G1 and on) is listed at the top of [`port`] and in the document.
//!
//! Only the GRBL laser. Marlin and Smoothieware lasers stay closed (no mode) until tried on a
//! machine; `--mode grbl` on a Smoothieware machine is untried.

pub mod port;

#[cfg(test)]
mod proof;
#[cfg(test)]
mod tests;
