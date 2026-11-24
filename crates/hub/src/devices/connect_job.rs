//! Whether a printer on Prusa Connect's protocol has a print now: while its telemetry carries a
//! `job_id`. Both references say so, each for its own devices:
//!
//! - A Buddy printer, Buddy 8.0.2 (`reference/buddy-8.0.2`, `b1043d6`): `render.cpp` writes
//!   `job_id` into every telemetry frame, Full or Reduced, `if (params.has_job)` (`:162-175`, before
//!   the Full block at `:177`), and `has_job` is PRINTING or PAUSED, or ATTENTION while it prints
//!   or shows the print preview (`src/state/printer_state.cpp:613-626`).
//! - A device on the agent, PrusaLink through the Connect SDK (`reference/Prusa-Link` `171148c`,
//!   `reference/Prusa-Connect-SDK-Printer` `c0d7d23`): the SDK adds `job_id` to every telemetry
//!   while the printer has one (`__init__.py:346-347`), and PrusaLink gives it one only while a job
//!   runs (`get_job_id_for_api`, `structures/module_data_classes.py:93-100`); our agent keeps it so
//!   (`crates/agent/src/job.rs`).

use maker_connect_protocol::Telemetry;

/// Whether the print in the telemetry is the one on the bed now.
pub(crate) fn is_running(t: &Telemetry) -> bool {
    t.job_id.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with a job is a print; one without is none, whatever state it says.
    #[test]
    fn a_print_runs_while_the_frame_names_its_job() {
        let mut t = Telemetry {
            job_id: Some(29),
            ..Telemetry::default()
        };
        assert!(is_running(&t));
        t.job_id = None;
        assert!(!is_running(&t));
    }
}
