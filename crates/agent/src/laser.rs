//! A GRBL laser on the agent's USB (`docs/design/prusa-sdk-extension.md`, part 4): the laser port
//! (`maker_connect_engine::ports::laserfoundry`) hosted on the SDK as the Marlin is. What the laser
//! is sent follows the gnea/grbl wiki `86e8ecd`, and LaserFoundry where the wiki is silent
//! (CLAUDE.md, *Lasers (GRBL)*).
//!
//! Its states, telemetry and settings in `INFO` (`grbl.telemetry`), and Part 4's commands
//! (`grbl.commands`): `HOME`, `UNLOCK`, `JOG`, `SET_ORIGIN` and `GO_TO_ORIGIN`, each the port's
//! `Action` for LaserFoundry's `Link` call. Its jobs and `FRAME` are not here yet (#173, #172).

use super::{Agent, set_state};
use maker_connect_engine::Report;
use maker_connect_protocol::{Action, GrblSettings, MachineState};
use maker_connect_sdk::command::{Command, CommandHandler, Finish, HandlerError};
use maker_connect_sdk::consts::{self, Source, State};
use maker_connect_sdk::models::Kwargs;
use serde_json::Value;
use std::sync::Arc;

/// Part 4's commands, set on the SDK as the Marlin's are (`marlin.rs` `setup`).
pub(crate) fn setup(agent: &Arc<Agent>, command: &Command) {
    for name in [
        consts::Command::Home,
        consts::Command::Unlock,
        consts::Command::Jog,
        consts::Command::SetOrigin,
        consts::Command::GoToOrigin,
    ] {
        let me = agent.clone();
        let h: CommandHandler = Arc::new(move |cmd| handle(&me, name, cmd));
        command.set_handler(name, h);
    }
}

/// Part 4: `FINISHED` once the laser is done, or `REJECTED` with the reason (GRBL's `error:`, or
/// LaserFoundry's refusal).
fn handle(agent: &Agent, name: consts::Command, cmd: &Command) -> Result<Finish, HandlerError> {
    send(agent, cmd.command_id(), action(name, cmd)?).map(|_| Finish::new(Source::Connect))
}

/// Part 4's table: the command as the port's `Action`, which the port turns into the `Link` call
/// (`ports/laserfoundry/port.rs` `commands_for`).
fn action(name: consts::Command, cmd: &Command) -> Result<Action, HandlerError> {
    Ok(match name {
        consts::Command::Home => Action::Home { axes: Vec::new() },
        consts::Command::Unlock => Action::Unlock,
        consts::Command::Jog => {
            let kwargs = cmd.kwargs();
            // As the SDK reads a kwarg it needs (`command_handlers.py`), `KeyError` when it is not there.
            let number = |key: &str| {
                kwargs
                    .as_ref()
                    .and_then(|k| k.get(key))
                    .and_then(Value::as_f64)
                    .ok_or_else(|| HandlerError::Rejected(format!("KeyError('{key}')")))
            };
            Action::Jog {
                x: Some(number("x")?),
                y: Some(number("y")?),
                z: None,
                feed: Some(number("feed")?),
            }
        }
        consts::Command::SetOrigin => Action::SetOrigin,
        consts::Command::GoToOrigin => Action::GoOrigin,
        other => {
            return Err(HandlerError::Rejected(format!(
                "{}: not a laser command",
                other.value()
            )));
        }
    })
}

/// The action to the laser's port, waited for: what the laser said, or why not.
fn send(agent: &Agent, command_id: i64, action: Action) -> Result<Vec<String>, HandlerError> {
    match crate::run_action(agent, command_id, action) {
        Ok(done) => done.map_err(HandlerError::Rejected),
        Err(crate::Gone::Off) => Err(HandlerError::Rejected("the laser module is off".into())),
        Err(crate::Gone::Ended) => Err(HandlerError::Rejected("the laser port has ended".into())),
    }
}

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
    // Part 4, *What the laser is*: its `$I` answer in `INFO` (`firmware`, `firmware_info`), once
    // it answered, sent again when it changed.
    if let Some(info) = report
        .telemetry
        .identity
        .as_ref()
        .and_then(|i| i.firmware_info.clone())
    {
        let firmware = crate::marlin::Firmware {
            name: info
                .contains_key("VER")
                .then(|| report.telemetry.firmware.clone())
                .flatten(),
            info: Some(info),
            capabilities: None,
        };
        if agent.firmware.lock().unwrap().as_ref() != Some(&firmware) {
            *agent.firmware.lock().unwrap() = Some(firmware);
            crate::info_changed(agent);
        }
    }
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

    /// Part 4's commands through the SDK to LaserFoundry's simulated GRBL: each `FINISHED`, and the
    /// line the `Link` sent for it.
    #[test]
    fn the_commands_reach_the_laser() {
        let (agent, cmd, sent) = laser();
        let cases = [
            ("HOME", None, "> $H"),
            ("UNLOCK", None, "> $X"),
            (
                "JOG",
                Some(serde_json::json!({"x": 5, "y": 0, "feed": 3000})),
                "> $J=G91 G21 X5 F3000",
            ),
            ("SET_ORIGIN", None, "> G92 X0 Y0"),
            ("GO_TO_ORIGIN", None, "> G90 G0 X0 Y0"),
        ];
        for (id, (name, kwargs, line)) in (1..).zip(cases) {
            cmd.accept(id, name, None, kwargs.clone(), false);
            cmd.run();
            let last = sent.lock().unwrap().last().cloned().unwrap();
            assert_eq!(
                (last.0, last.2),
                (consts::Event::Finished, Some(id)),
                "{name}"
            );
            // The same command again, its output looked at.
            cmd.accept(id + 100, name, None, kwargs, false);
            let output = send(
                &agent,
                id + 100,
                action(name.parse().unwrap(), &cmd).unwrap(),
            )
            .unwrap();
            assert_eq!(
                output.first().map(String::as_str),
                Some(line),
                "{name}: {output:?}"
            );
        }
    }

    /// A jog LaserFoundry refuses is `REJECTED` with its reason; a missing kwarg likewise.
    #[test]
    fn a_refused_or_incomplete_jog_is_rejected() {
        let (_agent, cmd, sent) = laser();
        cmd.accept(
            1,
            "JOG",
            None,
            Some(serde_json::json!({"x": 5000, "y": 0, "feed": 3000})),
            false,
        );
        cmd.run();
        let last = sent.lock().unwrap().last().cloned().unwrap();
        assert_eq!((last.0, last.2), (consts::Event::Rejected, Some(1)));
        // LaserFoundry's own text (`Link::jog`, its `MAX_JOG_MM` and feed bounds).
        assert_eq!(
            last.3.get("reason"),
            Some(&Value::from(
                "jog steps must be within ±1000 mm and feed within 1–30000 mm/min"
            ))
        );
        cmd.accept(
            2,
            "JOG",
            None,
            Some(serde_json::json!({"x": 5, "y": 0})),
            false,
        );
        cmd.run();
        let last = sent.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.3.get("reason"), Some(&Value::from("KeyError('feed')")));
    }

    type Sent = Arc<std::sync::Mutex<Vec<(consts::Event, Source, Option<i64>, Kwargs)>>>;

    /// An agent for a laser whose port drives LaserFoundry's simulated GRBL, its commands set.
    fn laser() -> (Arc<Agent>, Command, Sent) {
        use maker_connect_engine::ports::laserfoundry::port::LaserFoundryPort;
        let dir = std::env::temp_dir().join(format!("mc-laser-{}", std::process::id()));
        let engine = Arc::new(std::sync::Mutex::new(maker_connect_engine::Engine::new()));
        let agent = Arc::new(Agent::new(
            crate::Kind::Laser,
            dir,
            crate::Settings::default(),
            engine.clone(),
        ));
        let mut sim = maker_connect_grbl::SimulatedGrbl::new();
        sim.set_latency(std::time::Duration::from_millis(1));
        let port =
            LaserFoundryPort::with_machine(maker_connect_grbl::GrblMachine::new(Box::new(sim)));
        *agent.dymo.lock().unwrap() = Some(engine.lock().unwrap().add(Box::new(port)));
        let sent: Sent = Default::default();
        let to = sent.clone();
        let cmd = Command::new(Arc::new(move |event, source, _, id, kwargs| {
            to.lock().unwrap().push((event, source, id, kwargs));
        }));
        setup(&agent, &cmd);
        (agent, cmd, sent)
    }

    /// Part 4, *What the laser is*: the `$I` answer in `INFO` as `firmware` and `firmware_info`, once
    /// it answered, and `INFO` again only when it changed.
    #[test]
    fn its_build_info_in_info() {
        use maker_connect_sdk::consts::Event;
        use std::sync::{Arc, Mutex};
        let dir = std::env::temp_dir().join(format!("mc-laser-{}", std::process::id()));
        let a = Agent::new(
            crate::Kind::Laser,
            dir,
            crate::Settings::default(),
            Arc::new(Mutex::new(maker_connect_engine::Engine::new())),
        );
        let sent = Arc::new(Mutex::new(Vec::new()));
        let seen = sent.clone();
        let cb: maker_connect_sdk::models::EventCallback =
            Arc::new(move |event, _, _, _, kwargs| seen.lock().unwrap().push((event, kwargs)));
        *a.event_cb.lock().unwrap() = Some(cb);
        let report = |info: Option<Value>| Report {
            telemetry: Telemetry {
                firmware: Some(
                    if info.is_some() {
                        "1.1f(ESP32).20210403"
                    } else {
                        "GRBL"
                    }
                    .into(),
                ),
                identity: info.map(|i| maker_connect_protocol::Identity {
                    firmware_info: i.as_object().cloned(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            connected: true,
            ..Default::default()
        };
        let ortur = serde_json::json!({"MODEL": "Ortur Laser Master 2 Pro S2", "VER": "1.1f(ESP32).20210403:", "OPT": "VNMZHSL,35,7680,3"});
        let info = crate::send_info(&a).unwrap().kwargs;
        assert!(!info.contains_key("firmware_info"));
        tick(&a, &report(None));
        tick(&a, &report(Some(ortur.clone())));
        tick(&a, &report(Some(ortur.clone())));
        let infos: Vec<_> = sent
            .lock()
            .unwrap()
            .iter()
            .filter(|(e, _)| *e == Event::Info)
            .map(|(_, k)| k.clone())
            .collect();
        assert_eq!(infos.len(), 1, "the build info's INFO, once: {infos:?}");
        let info = crate::send_info(&a).unwrap().kwargs;
        assert_eq!(info["firmware"], "1.1f(ESP32).20210403");
        assert_eq!(info["firmware_info"], ortur);
    }

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
