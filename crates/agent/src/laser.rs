//! A GRBL laser on the agent's USB (`docs/design/prusa-sdk-extension.md`, part 4): the LaserFoundry
//! port (`maker_connect_engine::ports::laserfoundry`, LaserFoundry v0.6.0 `8f07f98`) hosted on the
//! SDK as the Marlin is.
//!
//! Reporting only (#163): its states, telemetry and settings in `INFO` (`grbl.telemetry`). It has
//! no handler but `SEND_INFO`, so the SDK rejects every other command, "Not Implemented"
//! (`command.py` `__call__`; `crates/connect-sdk/src/command.rs`): nothing from the hub reaches
//! the laser. Its commands are #164.

use super::{Agent, set_state};
use maker_connect_engine::Report;
use maker_connect_protocol::{GrblSettings, MachineState};
use maker_connect_sdk::consts::State;
use maker_connect_sdk::models::Kwargs;
use serde_json::Value;

/// Part 4's table: GRBL's state, by LaserFoundry's name for it (`crates/grbl` `state_name`), as the
/// protocol's `state`; `job` whether a job of the hub's runs. `None`: no status report yet
/// (`Unknown`).
pub(crate) fn protocol_state(machine_state: Option<&str>, job: bool) -> State {
    match machine_state {
        Some("Idle") => State::Idle,
        Some("Run") if job => State::Printing,
        Some("Run" | "Jog" | "Home" | "Check") => State::Busy,
        Some("Hold") => State::Paused,
        Some("Door" | "Sleep") => State::Attention,
        Some("Alarm") => State::Error,
        _ => State::Busy,
    }
}

/// Part 4's `machine_state`: GRBL's state as it reports it, `Unknown` before a status report.
pub(crate) fn machine_state_key(machine_state: Option<&str>) -> &str {
    machine_state.unwrap_or("Unknown")
}

/// Whether the laser is there: its port runs and its link is open.
pub(crate) fn present(report: &Report) -> bool {
    report.connected && report.telemetry.state != MachineState::Disconnected
}

/// The state the hub is told.
pub(crate) fn state(agent: &Agent, report: Option<&Report>) -> State {
    let job = report.is_some_and(|r| r.job.is_some());
    protocol_state(*agent.machine_state.lock().unwrap(), job)
}

/// Once a second, from the link loop: `INFO` again when the settings changed
/// (part 4: the device read them from GRBL after connecting), then a `STATE_CHANGED` when the
/// state is another than the last one said.
pub(crate) fn tick(agent: &Agent, report: &Report) {
    let known = Known {
        settings: report.telemetry.grbl.clone(),
    };
    if *agent.laser.lock().unwrap() != known {
        *agent.laser.lock().unwrap() = known;
        crate::info_changed(agent);
    }
    let now = state(agent, Some(report));
    if *agent.last_state.lock().unwrap() != Some(now) {
        set_state(agent, now, report.telemetry.problem.as_deref());
    }
}

/// What the laser said of itself, for `INFO`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Known {
    pub settings: Option<GrblSettings>,
}

/// Part 4's `INFO` kwargs, as far as the laser has told them: `max_power` (`$30`), `laser_mode`
/// (`$32`), `travel_x`, `travel_y`, `travel_z` (`$130`-`$132`).
pub(crate) fn info(kwargs: &mut Kwargs, settings: Option<&GrblSettings>) {
    let Some(s) = settings else {
        return;
    };
    if let Some(p) = s.max_power {
        kwargs.insert("max_power".into(), p.into());
    }
    if let Some(m) = s.laser_mode {
        kwargs.insert("laser_mode".into(), m.into());
    }
    if let Some([x, y, z]) = s.travel_mm {
        kwargs.insert("travel_x".into(), x.into());
        kwargs.insert("travel_y".into(), y.into());
        kwargs.insert("travel_z".into(), z.into());
    }
}

/// Part 4's telemetry: `axis_x`, `axis_y` (GRBL's `MPos`), `progress`, `job_id`, and
/// `machine_state`.
pub(crate) fn telemetry(
    t: &maker_connect_protocol::Telemetry,
    machine_state: Option<&str>,
) -> Kwargs {
    let mut kwargs = Kwargs::new();
    if let Some([x, y, _]) = t.position_mm {
        if let Some(x) = x {
            kwargs.insert("axis_x".into(), f64::from(x).into());
        }
        if let Some(y) = y {
            kwargs.insert("axis_y".into(), f64::from(y).into());
        }
    }
    if let Some(job) = &t.job
        && let Some(progress) = (job.done * 100).checked_div(job.total)
    {
        kwargs.insert("progress".into(), progress.into());
    }
    if let Some(id) = t.job_id {
        kwargs.insert("job_id".into(), id.into());
    }
    kwargs.insert(
        "machine_state".into(),
        Value::from(machine_state_key(machine_state)),
    );
    kwargs
}

#[cfg(test)]
mod tests {
    use super::*;
    use maker_connect_protocol::{JobProgress, Telemetry};

    /// Every row of part 4's state table.
    #[test]
    fn the_states_are_part_4s() {
        let rows = [
            (Some("Idle"), false, State::Idle),
            (Some("Run"), true, State::Printing),
            (Some("Run"), false, State::Busy),
            (Some("Jog"), false, State::Busy),
            (Some("Home"), false, State::Busy),
            (Some("Check"), false, State::Busy),
            (Some("Hold"), true, State::Paused),
            (Some("Door"), false, State::Attention),
            (Some("Alarm"), false, State::Error),
            (Some("Sleep"), false, State::Attention),
            (None, false, State::Busy),
        ];
        for (grbl, job, want) in rows {
            assert_eq!(protocol_state(grbl, job), want, "{grbl:?} job {job}");
        }
        assert_eq!(machine_state_key(None), "Unknown");
        assert_eq!(machine_state_key(Some("Door")), "Door");
    }

    /// The machine position, the progress and the job; `machine_state` always.
    #[test]
    fn telemetry_is_part_4s_keys() {
        let t = Telemetry {
            position_mm: Some([Some(12.5), Some(40.0), Some(0.0)]),
            job: Some(JobProgress {
                file: "a.gcode".into(),
                done: 25,
                total: 100,
                elapsed_s: Some(3),
                remaining_s: Some(9),
            }),
            job_id: Some(7),
            ..Telemetry::default()
        };
        let k = telemetry(&t, Some("Run"));
        assert_eq!(
            Value::Object(k),
            serde_json::json!({"axis_x": 12.5, "axis_y": 40.0, "progress": 25, "job_id": 7, "machine_state": "Run"})
        );
        let k = telemetry(&Telemetry::default(), None);
        assert_eq!(
            Value::Object(k),
            serde_json::json!({"machine_state": "Unknown"})
        );
    }

    /// The settings as part 4's `INFO` kwargs; nothing before GRBL told them.
    #[test]
    fn info_has_the_settings() {
        let mut k = Kwargs::new();
        info(&mut k, None);
        assert!(k.is_empty());
        let s = GrblSettings {
            laser_mode: Some(true),
            max_power: Some(1000.0),
            travel_mm: Some([400.0, 430.0, 0.0]),
        };
        info(&mut k, Some(&s));
        assert_eq!(
            Value::Object(k),
            serde_json::json!({"max_power": 1000.0, "laser_mode": true, "travel_x": 400.0, "travel_y": 430.0, "travel_z": 0.0})
        );
    }
}
