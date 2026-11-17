//! # Maker Connect agent
//!
//! The module next to a machine (`docs/plans/agent-engine.md`): its hub link ([`link`]) reports to the
//! Maker Connect hub ([`maker_connect_protocol`]) what the engine ([`maker_connect_engine`])
//! knows, and gives the engine what the hub asks; each machine's port ([`ports`]) is the only
//! code that talks to its device, as its reference does.

mod camera;
mod config;
mod link;
mod ports;

use std::process::ExitCode;

const USAGE: &str = "Maker Connect agent (proof of concept)

Usage:
  maker-connect-agent --version                        which agent this is: version, build, date
  maker-connect-agent ports                            list the serial ports
  maker-connect-agent run [--config <file>] [--save]
                          [--hub <http://host:8460>] [--name <name>]
                          --kind fdm|laser|cnc|cutter|label|paper|scale|other
                          [--port <serial port> [--baud <rate>]]
                          --mode prusa|marlin|grbl|dymo-pnp|labelwriter-450|dymo-scale|cameras
                          [--camera <nokhwa index>]
                          [--state <file>] [--new-code]
                                                       report a machine to the hub; shows the
                                                       pairing code to add it on the website;
                                                       carries out its commands and jobs.
                                                       --mode and --kind are required: there is
                                                       no default for either (in the options or
                                                       in the config file).
                                                       --mode: what is on the port, never guessed:
                                                       prusa (Prusa's MK3 family, as PrusaLink),
                                                       marlin (any other printer, as OctoPrint),
                                                       grbl (a laser), dymo-pnp (a DYMO
                                                       LabelManager PnP on USB, as labelle; found
                                                       by itself, no --port), labelwriter-450
                                                       (a DYMO LabelWriter 450 on USB, as DYMO's
                                                       CUPS driver; found by itself, no --port),
                                                       dymo-scale (a DYMO M-series scale, USB,
                                                       its weight reported live, as usbscale
                                                       reads it; --kind scale; --port = its HID
                                                       path, left out the one device with HID
                                                       usage page 0x8D, else it refuses and lists
                                                       what it saw). On Linux the scale's hidraw
                                                       device needs crates/agent/udev/60-maker-
                                                       connect-scale.rules in /etc/udev/rules.d/
                                                       (then udevadm control --reload; udevadm
                                                       trigger) and the user in the group plugdev.
                                                       cameras (no machine, only --camera: a USB
                                                       webcam by its nokhwa index, a JPEG every 5 s
                                                       to the hub's test page; needs --kind other;
                                                       on Linux the user in the group video).
                                                       --port and --baud: left out, OctoPrint's
                                                       own settings and detection choose.
                                                       --kind label: a DYMO LabelManager PnP or
                                                       LabelWriter 400/450 on USB, found by
                                                       itself (no --port)
                                                       --new-code: register again for a new
                                                       pairing code (to add it again, or to
                                                       another space); a new --hub gets one
                                                       by itself

The options can live in maker-connect-agent.toml (in the current folder, or next to the
agent): `run --save` with the options writes it. With that file, `run` needs nothing else,
and the agent on its own (a double-click) runs.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Started on its own (a double-click on Windows): run from the config file, if there is
    // one, and keep the window open on an error so it can be read.
    let alone = args.is_empty();
    if alone && config::places().iter().any(|p| p.is_file()) {
        if let Err(error) = link::run(&[]) {
            eprintln!("error: {error}");
            wait_for_enter();
        }
        return ExitCode::FAILURE;
    }
    let result = match args.first().map(String::as_str) {
        Some("--version" | "version" | "-V") => {
            println!("{}", version());
            Ok(())
        }
        Some("ports") => ports(),
        Some("run") => link::run(&args[1..]),
        _ => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The release this agent is (`v0.2.0-beta.1`, set by `scripts/build-release.sh`), or the
/// crate's version.
pub const RELEASE: &str = match option_env!("MAKER_CONNECT_RELEASE") {
    Some(tag) => tag,
    None => env!("CARGO_PKG_VERSION"),
};

/// What this agent is: `maker-connect-agent 0.2.0 (build 1d5c84c, 2026-11-04 14:35 UTC)`.
pub fn version() -> String {
    format!(
        "maker-connect-agent {} (build {}, {})",
        RELEASE,
        env!("MC_COMMIT"),
        env!("MC_BUILT")
    )
}

/// Keeps a window that the agent opened (a double-click) until Enter.
fn wait_for_enter() {
    eprintln!("press Enter to close");
    let _ = std::io::stdin().read_line(&mut String::new());
}

fn ports() -> Result<(), Box<dyn std::error::Error>> {
    let ports = serialport::available_ports()?;
    if ports.is_empty() {
        println!("no serial ports");
    }
    for port in ports {
        let kind = match port.port_type {
            serialport::SerialPortType::UsbPort(usb) => format!(
                "USB {:04x}:{:04x} {}",
                usb.vid,
                usb.pid,
                usb.product.unwrap_or_default()
            ),
            other => format!("{other:?}"),
        };
        println!("{}  {kind}", port.port_name);
    }
    Ok(())
}
