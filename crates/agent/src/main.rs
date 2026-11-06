//! # Maker Connect agent
//!
//! The module next to a machine: it drives the machine and reports to the Maker Connect
//! cloud ([`maker_connect_protocol`]). This proof of concept knows printers on USB that
//! speak the Marlin protocol ([`marlin`]): it streams G-code to them (`send`), and reports
//! one to the cloud (`run`); GRBL lasers; a DYMO LabelManager on USB HID, and a DYMO
//! LabelWriter on USB.

mod cloud;
mod config;
mod marlin;
mod print;

use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const USAGE: &str = "Maker Connect agent (proof of concept)

Usage:
  maker-connect-agent --version                        which agent this is: version, build, date
  maker-connect-agent ports                            list the serial ports
  maker-connect-agent send <port> <file.gcode> [--baud 115200]
                                                       stream a G-code file to a printer on USB
                                                       (Marlin protocol: line numbers, checksums,
                                                       ok, resend)
  maker-connect-agent run [--config <file>] [--save]
                          [--cloud <http://host:8460>] [--name <name>] [--kind fdm|laser|label]
                          [--port <serial port> [--baud 115200] [--firmware auto|marlin|grbl]]
                          [--state <file>] [--new-code]
                                                       report a machine to the cloud; shows the
                                                       pairing code to add it on the website;
                                                       carries out its commands and jobs.
                                                       --firmware auto: GRBL if it greets as GRBL
                                                       (a laser), else Marlin (a printer).
                                                       --kind label: a DYMO LabelManager PnP or
                                                       LabelWriter 400/450 on USB, found by
                                                       itself (no --port)
                                                       --new-code: register again for a new
                                                       pairing code (to add it again, or to
                                                       another space); a new --cloud gets one
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
        if let Err(error) = cloud::run(&[]) {
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
        Some("send") => send(&args[1..]),
        Some("run") => cloud::run(&args[1..]),
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

/// What this agent is: `maker-connect-agent 0.1.0 (build 1d5c84c, 2026-11-04 14:35 UTC)`.
pub fn version() -> String {
    format!(
        "maker-connect-agent {} (build {}, {})",
        env!("CARGO_PKG_VERSION"),
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

fn send(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (Some(port), Some(file)) = (args.first(), args.get(1)) else {
        return Err(format!("expected a port and a file\n\n{USAGE}").into());
    };
    let mut baud = 115_200;
    let mut rest = args[2..].iter();
    while let Some(flag) = rest.next() {
        match flag.as_str() {
            "--baud" => {
                baud = rest
                    .next()
                    .ok_or("expected a number after --baud")?
                    .parse()?
            }
            _ => return Err(format!("unknown option {flag}").into()),
        }
    }
    let gcode = std::fs::read_to_string(file)?;
    let link = serialport::new(port, baud)
        .timeout(Duration::from_millis(200))
        .open()?;
    let mut printer = marlin::Printer::new(link);
    let started = Instant::now();
    let mut show = |event: marlin::Event| match event {
        marlin::Event::Said(text) => println!("\r< {text}"),
        marlin::Event::Resent(number) => println!("\rresend line {number}"),
        marlin::Event::Progress { done, total } => {
            print!(
                "\r{done}/{total} lines ({:.0} %)",
                done as f64 * 100.0 / total.max(1) as f64
            );
            let _ = std::io::stdout().flush();
        }
    };
    println!("connecting to {port} at {baud} baud");
    printer.connect(&mut show)?;
    println!("streaming {file}");
    printer.send(gcode.lines(), &mut show)?;
    println!("\ndone in {:.0} s", started.elapsed().as_secs_f64());
    Ok(())
}
