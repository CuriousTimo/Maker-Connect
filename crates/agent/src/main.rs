//! # Maker Connect agent
//!
//! The module next to a machine: it drives the machine and reports to the Maker Connect
//! cloud ([`maker_connect_protocol`]). This proof of concept knows printers on USB that
//! speak the Marlin protocol ([`marlin`]): it streams G-code to them (`send`), and reports
//! one to the cloud (`run`).

mod cloud;
mod marlin;

use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const USAGE: &str = "Maker Connect agent (proof of concept)

Usage:
  maker-connect-agent ports                            list the serial ports
  maker-connect-agent send <port> <file.gcode> [--baud 115200]
                                                       stream a G-code file to a printer on USB
                                                       (Marlin protocol: line numbers, checksums,
                                                       ok, resend)
  maker-connect-agent run --cloud <http://host:8460> --name <name> [--kind fdm|laser]
                          [--port <serial port> [--baud 115200] [--firmware auto|marlin|grbl]]
                          [--state <file>]
                                                       report a machine to the cloud; shows the
                                                       pairing code to add it on the website;
                                                       carries out its commands and jobs.
                                                       --firmware auto: GRBL if it greets as GRBL
                                                       (a laser), else Marlin (a printer)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
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
