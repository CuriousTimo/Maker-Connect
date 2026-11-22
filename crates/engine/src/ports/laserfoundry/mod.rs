//! The laser port (`docs/archive/plans/agent-engine.md`, Step 3d): the engine's [`crate::Port`] for
//! `--mode grbl`, over `maker-connect-grbl` (`crates/grbl`), which drives LaserFoundry's `Link`
//! (`laserfoundry-device`, MIT OR Apache-2.0, the fork rev pinned in `Cargo.toml`). **One
//! reference:** the gnea/grbl wiki `86e8ecd` and its `stream.py` (gnea/grbl `bfb67f0`), and
//! LaserFoundry where the wiki is silent (CLAUDE.md, *Lasers (GRBL)*). What is ours (G1 and on) is
//! listed at the top of [`port`].
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
