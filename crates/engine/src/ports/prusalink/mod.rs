//! What PrusaLink shows and sends to a printer's screen, for a Marlin printer on the agent
//! (#187, #189). The reference is PrusaLink (`reference/Prusa-Link`, commit
//! `171148ce7dbc7f1fb0633ea519523f3258877e2e`), the maker's own software, files named in each module.
//! PrusaLink has no licence on GitHub ("Freeware" in `setup.py`); the owner decided to port it
//! (2026-11-12, CLAUDE.md).
//!
//! These are deliberate exceptions to "OctoPrint is the reference for a Marlin printer"
//! (`docs/audits/octoprint-marlin.md`): OctoPrint sends no `M73` and no `M117` of its own.

// Ported to the letter (CLAUDE.md, "port, don't paraphrase"): the reference's own structure is kept,
// so clippy's style, complexity and performance rewrites are not applied here; its correctness
// lints still hold.
#![allow(clippy::style, clippy::complexity, clippy::perf)]

pub mod blinker;
pub mod carousel;
pub mod conditions;
pub mod config;
pub mod consts;
pub mod heap;
pub mod helpers;
pub mod instruction;
pub mod is_planner_fed;
pub mod item_updater;
pub mod lcd_printer;
pub mod mc_singleton;
pub mod model_classes;
pub mod module_data_classes;
pub mod print_stats;
pub mod pydantic;
pub mod regular_expressions;
#[cfg(target_os = "linux")]
pub mod serial;
pub mod serial_parser;
pub mod serial_queue;
pub mod sortedcontainers;
pub mod unidecode;
pub mod updatable;
pub mod util;
#[rustfmt::skip]
mod unidecode_data;

/// The tests run PrusaLink's own Python (`reference/.venv-prusalink`) on the same examples, as the
/// estimator's test runs OctoPrint's.
#[cfg(test)]
pub(crate) mod python {
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    pub const COMMON: &str = include_str!("driver_common.py");

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn var_or(var: &str, local: &str) -> PathBuf {
        std::env::var_os(var)
            .map(PathBuf::from)
            .unwrap_or_else(|| repo().join(local))
    }

    /// `COMMON` and `driver` run with `stdin`; the one JSON line it prints. As the SDK's reference
    /// tests (crates/connect-sdk/tests/download_reference.rs): the Python with PrusaLink's
    /// packages in `PRUSALINK_PYTHON`, PrusaLink's and the SDK's checkouts in `PRUSALINK_SRC` and
    /// `PRUSALINK_SDK_SRC` (CI), else the local `reference/`; `None` (said on stderr) without
    /// that Python, and the test passes.
    pub fn run(driver: &str, stdin: &serde_json::Value) -> Option<serde_json::Value> {
        let python = var_or("PRUSALINK_PYTHON", "reference/.venv-prusalink/bin/python");
        if !python.exists() {
            eprintln!("not compared: no Python at {python:?} (set PRUSALINK_PYTHON)");
            return None;
        }
        let link = var_or("PRUSALINK_SRC", "reference/Prusa-Link");
        // the SDK at PrusaLink's pin c0d7d233 (`docs/archive/plans/mk3-port-plan.md`)
        let sdk = var_or(
            "PRUSALINK_SDK_SRC",
            "reference/Prusa-Connect-SDK-Printer-c0d7d233",
        );
        let mut child = Command::new(&python)
            .args(["-I", "-W", "ignore", "-c"])
            .arg(format!("{COMMON}\n{driver}"))
            .arg(&link)
            .arg(&sdk)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("{}: {e}", python.display()));
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "the Python driver failed");
        Some(serde_json::from_slice(&out.stdout).expect("one JSON line"))
    }
}
