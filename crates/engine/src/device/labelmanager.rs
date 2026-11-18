//! The DYMO LabelManager PnP port (labelle, `ports/labelle`) behind the contract: device map §2.
//!
//! `mc.label.print` is a job: the port's `Order::Job` with the tape width. The state is ours (L1, L7,
//! L10; the map's DISCONNECTED, IDLE, BUSY), from the port's reports.

use super::{MakePort, Mapping, Planned, StateSink, integer, job_order, needed, text};
use crate::ports::labelle::DymoPnp;
use maker_connect_device::registry::{KIND_LABELMANAGER, LABEL_PRINT};
use maker_connect_device::{
    Arg, CapabilitySet, Command, FirmwareLabel, Identity, Kind, Unit, Verb,
};
use maker_connect_protocol::{MachineState, Telemetry};
use std::sync::Arc;

/// The LabelManager adapter.
pub struct LabelManager {
    make: MakePort,
}

impl LabelManager {
    /// labelle's port over the system's USB.
    pub fn new() -> Self {
        Self::with_port(Arc::new(|_| Box::new(DymoPnp::new())))
    }

    /// Over another port (the tests').
    pub fn with_port(make: MakePort) -> Self {
        Self { make }
    }
}

impl Default for LabelManager {
    fn default() -> Self {
        Self::new()
    }
}

/// The baseline (device map §2.1): `tape_size_mm` from {19, 12, 9, 6}, default 12
/// (`lib/devices/dymo_labeler.py:243-257`).
pub fn baseline() -> CapabilitySet {
    CapabilitySet::new().with(
        LABEL_PRINT,
        Verb::new()
            .arg("picture", Arg::new(Kind::String).required())
            .arg(
                "tape_size_mm",
                Arg::new(Kind::Integer)
                    .unit(Unit::Millimeter)
                    .allowed(&["19", "12", "9", "6"])
                    .default_value("12"),
            ),
    )
}

impl Mapping for LabelManager {
    fn kind(&self) -> &'static str {
        KIND_LABELMANAGER
    }

    fn baseline(&self) -> CapabilitySet {
        baseline()
    }

    fn identity(&self) -> Identity {
        Identity {
            vendor: "DYMO".into(),
            model: "LabelManager PnP".into(),
            firmware: FirmwareLabel::default(),
            serial: String::new(),
            unit_uid: String::new(),
        }
    }

    fn plan(&self, cmd: &Command, job: i64) -> Result<Planned, String> {
        if cmd.verb != LABEL_PRINT {
            return Err(format!("{}: not a LabelManager verb", cmd.verb));
        }
        let (mut order, file) = job_order(job, needed(Ok(text(cmd, "picture")), "picture")?);
        order.tape_mm = integer(cmd, "tape_size_mm")?
            .map(|mm| u32::try_from(mm).map_err(|_| format!("tape_size_mm: {mm}")))
            .transpose()?;
        Ok(Planned::Job { order, file })
    }

    fn state_of(&self, t: &Telemetry) -> Option<Option<&'static str>> {
        Some(match t.state {
            MachineState::Disconnected => Some("DISCONNECTED"),
            MachineState::Idle => Some("IDLE"),
            MachineState::Busy => Some("BUSY"),
            _ => None,
        })
    }

    fn port(&self, state: StateSink) -> Box<dyn crate::Port> {
        (self.make)(state)
    }
}
