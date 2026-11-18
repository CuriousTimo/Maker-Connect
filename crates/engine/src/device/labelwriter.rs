//! The DYMO LabelWriter port (DYMO's CUPS driver 1.4.0, `ports/dymo_cups`) behind the contract:
//! device map §3. Only the 450 has a port so far.
//!
//! `mc.label.print` is a job: the port's `Order::Job` with the `PageSize` (W2b). The state: IDLE and
//! PRINTING are ours (the port's Idle and Busy); OUT_OF_PAPER and ERROR are the driver's `jsPaperOut`
//! and `jsError`, which the port reports as the PPD's reason text of their `STATE:` keyword (W7).

use super::{MakePort, Mapping, Planned, StateSink, job_order, needed, text};
use crate::ports::dymo_cups::LabelWriter450;
use crate::ports::dymo_cups::cups::ppd_lw450::REASONS;
use maker_connect_device::registry::{KIND_LABELWRITER, LABEL_PRINT};
use maker_connect_device::{Arg, CapabilitySet, Command, FirmwareLabel, Identity, Kind, Verb};
use maker_connect_protocol::{MachineState, Telemetry};
use std::sync::Arc;

/// The LabelWriter 450 adapter.
pub struct LabelWriter {
    make: MakePort,
}

impl LabelWriter {
    /// The 450's port over the system's USB.
    #[cfg(not(test))]
    pub fn lw450() -> Self {
        Self::with_port(Arc::new(|_| Box::new(LabelWriter450::new())))
    }

    /// Over another port (the tests').
    pub fn with_port(make: MakePort) -> Self {
        Self { make }
    }

    /// The 450's port over another USB host (the tests').
    pub fn lw450_on(host: Arc<dyn crate::ports::dymo_cups::cups::usb_backend::UsbHost>) -> Self {
        Self::with_port(Arc::new(move |_| {
            Box::new(LabelWriter450::with_host(host.clone()))
        }))
    }
}

/// The baseline (device map §3.1): `page_size` a `PageSize` choice; without it the PPD's default.
pub fn baseline() -> CapabilitySet {
    CapabilitySet::new().with(
        LABEL_PRINT,
        Verb::new()
            .arg("picture", Arg::new(Kind::String).required())
            .arg("page_size", Arg::new(Kind::String)),
    )
}

/// The PPD's reason text of a `STATE:` keyword (`ppd/lw450.ppd:47-55`).
fn reason(keyword: &str) -> Option<&'static str> {
    REASONS
        .iter()
        .find(|(k, _, _)| *k == keyword)
        .map(|(_, t, _)| *t)
}

impl Mapping for LabelWriter {
    fn kind(&self) -> &'static str {
        KIND_LABELWRITER
    }

    fn baseline(&self) -> CapabilitySet {
        baseline()
    }

    fn identity(&self) -> Identity {
        Identity {
            vendor: crate::ports::dymo_cups::port::MANUFACTURER.into(),
            model: crate::ports::dymo_cups::port::MODEL.into(),
            firmware: FirmwareLabel::default(),
            serial: String::new(),
            unit_uid: String::new(),
        }
    }

    fn plan(&self, cmd: &Command, job: i64) -> Result<Planned, String> {
        if cmd.verb != LABEL_PRINT {
            return Err(format!("{}: not a LabelWriter verb", cmd.verb));
        }
        let (mut order, file) = job_order(job, needed(Ok(text(cmd, "picture")), "picture")?);
        order.page_size = text(cmd, "page_size").map(str::to_owned);
        Ok(Planned::Job { order, file })
    }

    fn state_of(&self, t: &Telemetry) -> Option<Option<&'static str>> {
        let problem = t.problem.as_deref();
        Some(match t.state {
            MachineState::Idle => Some("IDLE"),
            MachineState::Busy if problem.is_none() => Some("PRINTING"),
            MachineState::Busy if problem == reason("com.dymo.out-of-paper-error") => {
                Some("OUT_OF_PAPER")
            }
            MachineState::Busy if problem == reason("com.dymo.general-error") => Some("ERROR"),
            // a keyword the 450 code never sets (device map §3.2), or no state of the set
            _ => None,
        })
    }

    fn port(&self, state: StateSink) -> Box<dyn crate::Port> {
        (self.make)(state)
    }
}
