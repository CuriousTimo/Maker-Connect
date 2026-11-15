//! The Marlin port: OctoPrint 1.11.8 (`reference/OctoPrint-1.11.8`, commit `42be740`), translated
//! file by file (`docs/agent-engine.md`, step 3a).
//!
//! OctoPrint is Copyright (C) 2013 David Braam, Gina Häußge & others, released under the terms of
//! the GNU Affero General Public License v3 (as Maker Connect is). Each file here names the
//! OctoPrint file and lines it is ported from.

pub mod comm;
pub mod events;
pub mod filemanager;
pub mod files;
pub mod plugin;
pub mod profile;
pub mod py_attr;
pub mod py_collections;
pub mod py_queue;
pub mod py_threading;
pub mod settings;
pub mod text;
pub mod util;
pub mod platform;
#[cfg(unix)]
pub mod serial;
pub mod py_glob;
pub mod py_format;
pub mod py_percent;
pub mod standard;
pub mod action_command_prompt;
pub mod api_printer;
pub mod port;
#[cfg(test)]
mod proof;
