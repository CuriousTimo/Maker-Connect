//! The Marlin port (OctoPrint 1.11.8, `ports/octoprint`) behind the contract: device map §1.
//!
//! Each verb is the [`Action`] the port already maps to OctoPrint's own call (M1, `port.rs`); the
//! state is comm's `STATE_*` by name, from [`OctoPrintPort::with_comm_state_hook`].
//!
//! The baseline carries what the port's [`Action`] carries: `core.command.move_e` without
//! `feedrate` and `core.command.set_nozzle_temperature` without `tool` (the port extrudes without a
//! feed and heats `tool0`, `port.rs` M1); the factors as the port's `u16`.

use super::{MakePort, Mapping, Planned, StateSink, integer, job_order, needed, number, text};
use crate::ports::octoprint::comm::MachineCom;
use crate::ports::octoprint::port::OctoPrintPort;
use maker_connect_device::registry::*;
use maker_connect_device::{
    Arg, CapabilitySet, Command, FirmwareLabel, Identity, Kind, Unit, Verb,
};
use maker_connect_protocol::{Action, Axis, Heater, JobControl, Telemetry};
use std::sync::Arc;

/// comm's state by its `STATE_*` name without the prefix (`comm.py:445-460`, `MachineCom::STATE_NAMES`).
pub fn state_name(state: i64) -> Option<&'static str> {
    MachineCom::STATE_NAMES
        .iter()
        .find(|(_, n)| *n == state)
        .map(|(name, _)| name.trim_start_matches("STATE_"))
}

/// The Marlin adapter.
pub struct Marlin {
    make: MakePort,
}

impl Marlin {
    /// OctoPrint's port on `port` at `baud` (`--port`, `--baud`, as the agent gives them).
    pub fn new(port: Option<String>, baud: Option<u32>) -> Self {
        Self::with_port(Arc::new(move |sink: StateSink| {
            Box::new(
                OctoPrintPort::new(port.clone(), baud)
                    .with_comm_state_hook(Arc::new(move |s| sink(state_name(s)))),
            )
        }))
    }

    /// Over another port (the tests' fake); it tells the sink comm's state by name.
    pub fn with_port(make: MakePort) -> Self {
        Self { make }
    }
}

fn mm() -> Arg {
    Arg::new(Kind::Number).unit(Unit::Millimeter)
}
fn feed() -> Arg {
    Arg::new(Kind::Number).unit(Unit::MillimeterPerMinute)
}
fn factor() -> Arg {
    // OctoPrint: minimum 1 (standard.py:668-681); 65535: the port's `u16` (protocol `Action::Speed`)
    Arg::new(Kind::Integer)
        .unit(Unit::Percent)
        .required()
        .min(1.0)
        .max_abs(f64::from(u16::MAX))
}

/// The baseline (device map §1.1).
pub fn baseline() -> CapabilitySet {
    let celsius = || {
        Arg::new(Kind::Number)
            .unit(Unit::Celsius)
            .required()
            .min(0.0)
    };
    CapabilitySet::new()
        .with(
            GCODE,
            Verb::new().arg("text", Arg::new(Kind::String).required()),
        )
        .with(
            HOME,
            Verb::new().arg("axes", Arg::new(Kind::String).required()),
        )
        .with(
            MOVE,
            Verb::new()
                .arg("x", mm())
                .arg("y", mm())
                .arg("feedrate", feed()),
        )
        .with(
            MOVE_Z,
            Verb::new()
                .arg("distance", mm().required())
                .arg("feedrate", feed()),
        )
        .with(MOVE_E, Verb::new().arg("length", mm().required()))
        .with(SET_NOZZLE, Verb::new().arg("nozzle_temperature", celsius()))
        .with(SET_BED, Verb::new().arg("bed_temperature", celsius()))
        .with(SET_SPEED, Verb::new().arg("speed", factor()))
        .with(SET_FLOW, Verb::new().arg("flow", factor()))
        .with(DISABLE_STEPPERS, Verb::new())
        .with(
            FAN,
            Verb::new().arg("on", Arg::new(Kind::Boolean).required()),
        )
        .with(
            START_PRINT,
            Verb::new()
                .arg("path", Arg::new(Kind::String).required())
                .arg(
                    "origin",
                    Arg::new(Kind::String)
                        .allowed(&["local", "sd"])
                        .default_value("local"),
                ),
        )
        .with(PAUSE_PRINT, Verb::new())
        .with(RESUME_PRINT, Verb::new())
        .with(STOP_PRINT, Verb::new())
        .with(SD_LIST, Verb::new())
        .with(
            DELETE_FILE,
            Verb::new()
                .arg("path", Arg::new(Kind::String).required())
                .arg(
                    "origin",
                    Arg::new(Kind::String).allowed(&["sd"]).default_value("sd"),
                ),
        )
}

fn axes(list: &str) -> Result<Vec<Axis>, String> {
    list.split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(|a| match a {
            "x" => Ok(Axis::X),
            "y" => Ok(Axis::Y),
            "z" => Ok(Axis::Z),
            other => Err(format!("axes: {other} is not x, y or z")),
        })
        .collect()
}

fn percent(cmd: &Command, name: &str) -> Result<u16, String> {
    let v = needed(integer(cmd, name), name)?;
    u16::try_from(v).map_err(|_| format!("{name}: {v} is out of range"))
}

impl Mapping for Marlin {
    fn kind(&self) -> &'static str {
        KIND_MARLIN
    }

    fn baseline(&self) -> CapabilitySet {
        baseline()
    }

    fn identity(&self) -> Identity {
        Identity {
            vendor: String::new(),
            model: "Marlin (OctoPrint 1.11.8)".into(),
            firmware: FirmwareLabel::default(),
            serial: String::new(),
            unit_uid: String::new(),
        }
    }

    fn plan(&self, cmd: &Command, job: i64) -> Result<Planned, String> {
        let action = match cmd.verb.as_str() {
            GCODE => Action::Gcode {
                text: needed(Ok(text(cmd, "text")), "text")?.to_owned(),
            },
            HOME => Action::Home {
                axes: axes(needed(Ok(text(cmd, "axes")), "axes")?)?,
            },
            MOVE => Action::Jog {
                x: number(cmd, "x")?,
                y: number(cmd, "y")?,
                z: None,
                feed: number(cmd, "feedrate")?,
            },
            MOVE_Z => Action::Jog {
                x: None,
                y: None,
                z: Some(needed(number(cmd, "distance"), "distance")?),
                feed: number(cmd, "feedrate")?,
            },
            MOVE_E => Action::Extrude {
                mm: needed(number(cmd, "length"), "length")?,
            },
            SET_NOZZLE => Action::Temperature {
                heater: Heater::Nozzle,
                celsius: needed(number(cmd, "nozzle_temperature"), "nozzle_temperature")?,
            },
            SET_BED => Action::Temperature {
                heater: Heater::Bed,
                celsius: needed(number(cmd, "bed_temperature"), "bed_temperature")?,
            },
            SET_SPEED => Action::Speed {
                percent: percent(cmd, "speed")?,
            },
            SET_FLOW => Action::Flow {
                percent: percent(cmd, "flow")?,
            },
            DISABLE_STEPPERS => Action::MotorsOff,
            FAN => Action::Fan {
                on: needed(Ok(text(cmd, "on")), "on")? == "true",
            },
            START_PRINT => {
                let path = needed(Ok(text(cmd, "path")), "path")?;
                if text(cmd, "origin") == Some("sd") {
                    Action::SdPrint {
                        file: path.to_owned(),
                    }
                } else {
                    let (order, file) = job_order(job, path);
                    return Ok(Planned::Job { order, file });
                }
            }
            PAUSE_PRINT => return Ok(Planned::Control(JobControl::Pause)),
            RESUME_PRINT => return Ok(Planned::Control(JobControl::Resume)),
            STOP_PRINT => return Ok(Planned::Control(JobControl::Stop)),
            SD_LIST => Action::SdList,
            DELETE_FILE => Action::SdDelete {
                file: needed(Ok(text(cmd, "path")), "path")?.to_owned(),
            },
            other => return Err(format!("{other}: not a Marlin verb")),
        };
        Ok(Planned::Command(action))
    }

    fn state_of(&self, _: &Telemetry) -> Option<Option<&'static str>> {
        None
    }

    fn port(&self, state: StateSink) -> Box<dyn crate::Port> {
        (self.make)(state)
    }
}
