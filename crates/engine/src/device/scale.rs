//! The DYMO M-series scale port (usbscale, `ports/usbscale`) behind the contract: device map §5.
//! No verbs (owner, 2026-11-18: no `zero`; nothing is sent to the scale). The state is the status
//! byte of the last reading (`usbscale.c:406-452`); none before the first reading.

use super::{MakePort, Mapping, Planned, StateSink};
use crate::ports::usbscale::UsbScale;
use maker_connect_device::registry::KIND_SCALE;
use maker_connect_device::{CapabilitySet, Command, FirmwareLabel, Identity};
use maker_connect_protocol::Telemetry;
use std::sync::Arc;

/// The scale adapter.
pub struct Scale {
    make: MakePort,
}

impl Scale {
    /// usbscale's port over the OS's HID; `--port` as given.
    pub fn new(port: Option<String>) -> Self {
        Self::with_port(Arc::new(move |_| Box::new(UsbScale::new(port.clone()))))
    }

    /// Over another port (the tests').
    pub fn with_port(make: MakePort) -> Self {
        Self { make }
    }
}

/// The state of a status byte (`usbscale.c:406-452`).
pub fn state_name(status: u8) -> &'static str {
    match status {
        0x01 => "FAULT",
        0x02 => "ZEROED",
        0x03 => "WEIGHING",
        0x04 => "STABLE",
        0x05 => "UNDER_ZERO",
        0x06 => "OVER_WEIGHT",
        0x07 => "CALIBRATION_NEEDED",
        0x08 => "REZEROING_NEEDED",
        _ => "UNKNOWN",
    }
}

impl Mapping for Scale {
    fn kind(&self) -> &'static str {
        KIND_SCALE
    }

    fn baseline(&self) -> CapabilitySet {
        CapabilitySet::new()
    }

    fn identity(&self) -> Identity {
        Identity {
            vendor: "DYMO".into(),
            model: "M-series scale".into(),
            firmware: FirmwareLabel::default(),
            serial: String::new(),
            unit_uid: String::new(),
        }
    }

    fn plan(&self, cmd: &Command, _: i64) -> Result<Planned, String> {
        Err(format!("{}: a scale has no verbs", cmd.verb))
    }

    fn state_of(&self, t: &Telemetry) -> Option<Option<&'static str>> {
        Some(t.scale.as_ref().map(|r| state_name(r.status)))
    }

    fn port(&self, state: StateSink) -> Box<dyn crate::Port> {
        (self.make)(state)
    }
}
