//! The GRBL laser port (LaserFoundry v0.5.0, `ports/laserfoundry`) behind the contract: device map §4.
//!
//! The state is LaserFoundry's `MachineState` (`crates/device/src/status.rs:3-14`), from
//! [`LaserFoundryPort::with_machine_state_hook`]. `mc.laser.start` is person-only in the registry: the
//! host refuses it from the queue or a token before this adapter sees it.
//!
//! Not in the baseline: `mc.laser.frame`. The owner's answer (2026-11-18) is the reference's
//! `job.frame` over the design's bounds; the port still frames a file's extent at 3000 mm/min (G4,
//! G12, G16), and changing that changes what goes on the wire: not this step's.

use super::{MakePort, Mapping, Planned, StateSink, integer, job_order, needed, number, text};
use crate::ports::laserfoundry::port::LaserFoundryPort;
use maker_connect_device::registry::*;
use maker_connect_device::{
    Arg, CapabilitySet, Command, FirmwareLabel, Identity, Kind, Unit, Verb,
};
use maker_connect_protocol::{Action, JobControl, Telemetry};
use std::sync::Arc;

/// The contract's name of a LaserFoundry `MachineState` variant's name.
pub fn state_name(variant: &str) -> Option<&'static str> {
    Some(match variant {
        "Idle" => "IDLE",
        "Run" => "RUN",
        "Hold" => "HOLD",
        "Jog" => "JOG",
        "Alarm" => "ALARM",
        "Door" => "DOOR",
        "Check" => "CHECK",
        "Home" => "HOME",
        "Sleep" => "SLEEP",
        "Unknown" => "UNKNOWN",
        _ => return None,
    })
}

/// The laser adapter.
pub struct Grbl {
    make: MakePort,
}

impl Grbl {
    /// LaserFoundry's port on `port` at `baud` (G14).
    pub fn new(port: Option<String>, baud: Option<u32>) -> Self {
        Self::with_port(Arc::new(move |sink: StateSink| {
            Box::new(
                LaserFoundryPort::new(port.clone(), baud)
                    .with_machine_state_hook(Arc::new(move |s| sink(s.and_then(state_name)))),
            )
        }))
    }

    /// LaserFoundry's port over a machine made by `machine` (the tests' simulator).
    pub fn on(machine: Arc<dyn Fn() -> maker_connect_grbl::GrblMachine + Send + Sync>) -> Self {
        Self::with_port(Arc::new(move |sink: StateSink| {
            Box::new(
                LaserFoundryPort::with_machine(machine())
                    .with_machine_state_hook(Arc::new(move |s| sink(s.and_then(state_name)))),
            )
        }))
    }

    /// Over another port (the tests').
    pub fn with_port(make: MakePort) -> Self {
        Self { make }
    }
}

/// The baseline (device map §4.1), with the reference's bounds.
pub fn baseline() -> CapabilitySet {
    let mm = || {
        // `MAX_JOG_MM` link.rs:26
        Arg::new(Kind::Number)
            .unit(Unit::Millimeter)
            .required()
            .max_abs(1000.0)
    };
    CapabilitySet::new()
        // `MAX_LINE_BYTES` stream.rs:10
        .with(
            LASER_GCODE,
            Verb::new().arg("line", Arg::new(Kind::String).required().max_abs(79.0)),
        )
        .with(
            LASER_JOG,
            Verb::new().arg("x", mm()).arg("y", mm()).arg(
                "feed",
                // link.rs:27
                Arg::new(Kind::Number)
                    .unit(Unit::MillimeterPerMinute)
                    .required()
                    .min(1.0)
                    .max_abs(30000.0),
            ),
        )
        .with(LASER_HOME, Verb::new())
        .with(LASER_UNLOCK, Verb::new())
        .with(LASER_SET_ORIGIN, Verb::new())
        .with(LASER_GO_ORIGIN, Verb::new())
        .with(LASER_READ_SETTINGS, Verb::new())
        .with(
            LASER_START,
            Verb::new()
                .arg("program", Arg::new(Kind::String).required())
                .arg("from_line", Arg::new(Kind::Integer).min(1.0)),
        )
        .with(LASER_PAUSE, Verb::new())
        .with(LASER_RESUME, Verb::new())
        .with(LASER_STOP, Verb::new())
}

impl Mapping for Grbl {
    fn kind(&self) -> &'static str {
        KIND_GRBL
    }

    fn baseline(&self) -> CapabilitySet {
        baseline()
    }

    fn identity(&self) -> Identity {
        Identity {
            vendor: String::new(),
            model: "GRBL (LaserFoundry v0.5.0)".into(),
            firmware: FirmwareLabel::default(),
            serial: String::new(),
            unit_uid: String::new(),
        }
    }

    fn plan(&self, cmd: &Command, job: i64) -> Result<Planned, String> {
        let action = match cmd.verb.as_str() {
            LASER_GCODE => Action::Gcode {
                text: needed(Ok(text(cmd, "line")), "line")?.to_owned(),
            },
            LASER_JOG => Action::Jog {
                x: Some(needed(number(cmd, "x"), "x")?),
                y: Some(needed(number(cmd, "y"), "y")?),
                z: None,
                feed: Some(needed(number(cmd, "feed"), "feed")?),
            },
            LASER_HOME => Action::Home { axes: Vec::new() },
            LASER_UNLOCK => Action::Unlock,
            LASER_SET_ORIGIN => Action::SetOrigin,
            LASER_GO_ORIGIN => Action::GoOrigin,
            LASER_READ_SETTINGS => Action::ReadSettings,
            LASER_START => {
                let (mut order, file) =
                    job_order(job, needed(Ok(text(cmd, "program")), "program")?);
                order.from_line = integer(cmd, "from_line")?
                    .map(|l| u64::try_from(l).map_err(|_| format!("from_line: {l}")))
                    .transpose()?;
                return Ok(Planned::Job { order, file });
            }
            LASER_PAUSE => return Ok(Planned::Control(JobControl::Pause)),
            LASER_RESUME => return Ok(Planned::Control(JobControl::Resume)),
            LASER_STOP => return Ok(Planned::Control(JobControl::Stop)),
            other => return Err(format!("{other}: not a laser verb of this port")),
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
