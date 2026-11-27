//! The CPython ports the device ports share (`docs/archive/plans/mk3-port-plan.md`, section 0:
//! "move only the CPython ports the MK3 port uses", proxy 2026-11-16): OctoPrint's port and
//! PrusaLink's both run on them.

// Moved from `ports/octoprint`, where the same lints were allowed.
#![allow(
    clippy::style,
    clippy::complexity,
    clippy::perf,
    clippy::manual_unwrap_or_default
)]

pub mod py_queue;
pub mod py_threading;
