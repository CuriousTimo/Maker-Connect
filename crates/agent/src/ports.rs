//! The machines' ports (`docs/agent-engine.md`, step 3): each a port of one reference, the only
//! code that talks to its device. The DYMO PnP (`--mode dymo-pnp`, labelle), the DYMO LabelWriter 450
//! (`--mode labelwriter-450`, DYMO's CUPS driver) and the Marlin printer (`--mode marlin`, OctoPrint)
//! so far: the other machines are reported as disconnected until their ports are built.

use crate::link::{Mode, Options};
use maker_connect_engine::Port;

/// The port for the machine the options describe, opened; none yet.
pub fn open(options: &Options) -> Option<Box<dyn Port>> {
    match options.mode {
        Mode::DymoPnp => Some(Box::new(maker_connect_engine::ports::labelle::DymoPnp::new())),
        // `--port` and `--baud` as given: absent, OctoPrint's own settings and detection decide
        Mode::Marlin => Some(Box::new(maker_connect_engine::ports::octoprint::port::OctoPrintPort::new(
            options.port.clone(),
            options.baud,
        ))),
        // `--port` and `--baud` as given; without `--baud`, LaserFoundry's own `DEFAULT_BAUD`
        Mode::Grbl => Some(Box::new(maker_connect_engine::ports::laserfoundry::port::LaserFoundryPort::new(
            options.port.clone(),
            options.baud,
        ))),
        Mode::LabelWriter450 => Some(Box::new(maker_connect_engine::ports::dymo_cups::LabelWriter450::new())),
        Mode::Prusa => None,
    }
}
