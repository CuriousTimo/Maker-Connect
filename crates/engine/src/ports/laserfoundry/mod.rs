//! The laser port (`docs/archive/plans/agent-engine.md`, Step 3d): the engine's [`crate::Port`] for
//! `--mode grbl`, over `maker-connect-grbl` (`crates/grbl`), which is LaserFoundry v0.5.0
//! (61febdb, `laserfoundry-device` and `laserfoundry-gcode`, MIT OR Apache-2.0) and the link
//! logic of its app. **One reference:** LaserFoundry; every byte to the machine is its `Link`'s.
//! What is ours (G1 and on) is listed at the top of [`port`] and in the document.
//!
//! Only the GRBL laser. Marlin and Smoothieware lasers stay closed (no mode) until tried on a
//! machine; `--mode grbl` on a Smoothieware machine is untried.

// Ported to the letter (CLAUDE.md, "port, don't paraphrase"): the reference's own structure is kept,
// so clippy's style, complexity and performance rewrites are not applied here; its correctness
// lints still hold.
#![allow(clippy::style, clippy::complexity, clippy::perf)]
pub mod port;

#[cfg(test)]
mod proof;
#[cfg(test)]
mod tests;
