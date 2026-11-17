//! The Marlin port: OctoPrint 1.11.8 (`reference/OctoPrint-1.11.8`, commit `42be740`), translated
//! file by file (`docs/plans/agent-engine.md`, step 3a).
//!
//! OctoPrint is Copyright (C) 2013 David Braam, Gina Häußge & others, released under the terms of
//! the GNU Affero General Public License v3 (as Maker Connect is). Each file here names the
//! OctoPrint file and lines it is ported from.

// Ported to the letter (CLAUDE.md, "port, don't paraphrase"): the reference's own structure is kept,
// so clippy's style, complexity and performance rewrites are not applied here; its correctness
// lints still hold.
#![allow(
    clippy::style,
    clippy::complexity,
    clippy::perf,
    clippy::manual_unwrap_or_default
)]
pub mod action_command_prompt;
pub mod api_printer;
pub mod comm;
pub mod events;
pub mod filemanager;
pub mod files;
pub mod platform;
pub mod plugin;
pub mod port;
pub mod profile;
#[cfg(test)]
mod proof;
pub mod py_attr;
pub mod py_collections;
pub mod py_format;
pub mod py_glob;
pub mod py_percent;
pub mod py_queue;
pub mod py_threading;
#[cfg(unix)]
pub mod serial;
pub mod settings;
pub mod standard;
pub mod text;
pub mod util;
