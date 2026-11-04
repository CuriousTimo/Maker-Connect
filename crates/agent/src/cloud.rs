//! `run`: the agent reports its machine to the cloud. It registers once (the credentials
//! are kept in a small state file, so a restart keeps the machine), shows the pairing code
//! until someone adds the machine on the website, and then keeps reporting what the machine
//! does every two seconds. A printer on USB is asked for its firmware (`M115`) once and its
//! temperatures (`M105`) every report; when it is switched off or unplugged the agent says
//! so and tries again. Commands come back in the cloud's answer: the agent sends their G-code
//! to the printer and says in its next report how they went, with a bed mesh if the printer
//! printed one.

use crate::config::{self, Config};
use crate::marlin;
use maker_connect_grbl as grbl;
use maker_connect_label::{Label, dymo};
use maker_connect_protocol::{
    self as protocol, CommandDone, Identity, MachineKind, MachineState, Register, Registered,
    Telemetry, TelemetryReply,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const REPORT_EVERY: Duration = Duration::from_secs(2);
/// How often a missing printer is looked for again.
const RECONNECT_EVERY: Duration = Duration::from_secs(10);

struct Options {
    cloud: String,
    name: String,
    kind: MachineKind,
    port: Option<String>,
    baud: u32,
    firmware: Firmware,
    state: String,
}

/// What the machine on the port speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Firmware {
    /// Told by what it says when the port opens (GRBL greets by name), else Marlin.
    Auto,
    Marlin,
    Grbl,
}

/// The options: the config file's (see [`crate::config`]), with the command line's on top.
/// `--save` writes them into the config file.
fn options(args: &[String]) -> Result<Options, Box<dyn std::error::Error>> {
    let mut typed = Config::default();
    let mut named = None;
    let mut save = false;
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .ok_or_else(|| format!("expected a value after {flag}"))
        };
        match flag.as_str() {
            "--cloud" => typed.cloud = Some(value()?),
            "--name" => typed.name = Some(value()?),
            "--kind" => typed.kind = Some(value()?),
            "--port" => typed.port = Some(value()?),
            "--baud" => typed.baud = Some(value()?.parse()?),
            "--firmware" => typed.firmware = Some(value()?),
            "--state" => typed.state = Some(value()?),
            "--config" => named = Some(value()?),
            "--save" => save = true,
            _ => return Err(format!("unknown option {flag}").into()),
        }
    }
    let loaded = config::load(named.as_deref())?;
    if let Some((path, _)) = &loaded {
        println!("config: {}", path.display());
    }
    let (path, file) = loaded.unwrap_or_else(|| {
        (
            PathBuf::from(named.as_deref().unwrap_or(config::FILE)),
            Config::default(),
        )
    });
    let all = file.and(typed);
    if save {
        config::save(&path, &all)?;
        println!("saved to {}", path.display());
    }
    let firmware = match all.firmware.as_deref().unwrap_or("auto") {
        "auto" => Firmware::Auto,
        "marlin" => Firmware::Marlin,
        "grbl" => Firmware::Grbl,
        other => return Err(format!("firmware: auto, marlin or grbl, not {other}").into()),
    };
    let kind = match all.kind.as_deref() {
        Some(kind) => {
            serde_json::from_value(serde_json::Value::String(kind.to_owned())).map_err(|_| {
                format!("kind: fdm, laser, cnc, cutter, label, paper or other, not {kind}")
            })?
        }
        None => MachineKind::Fdm,
    };
    // The credentials beside the config file, so a double-click finds them too.
    let state = all.state.map(PathBuf::from).unwrap_or_else(|| {
        path.parent()
            .unwrap_or(Path::new(""))
            .join("maker-connect-agent.json")
    });
    let (Some(cloud), Some(name)) = (all.cloud, all.name) else {
        return Err(format!(
            "run needs a cloud and a name: --cloud <address> --name <name>, or in {}",
            config::FILE
        )
        .into());
    };
    Ok(Options {
        cloud: cloud.trim_end_matches('/').to_owned(),
        name,
        kind,
        port: all.port,
        baud: all.baud.unwrap_or(115_200),
        firmware,
        state: state.to_string_lossy().into_owned(),
    })
}

/// The agent's credentials, kept between runs.
#[derive(serde::Serialize, serde::Deserialize)]
struct Credentials {
    cloud: String,
    registered: Registered,
}

fn register(options: &Options) -> Result<Registered, Box<dyn std::error::Error>> {
    let agent = Register {
        protocol: protocol::VERSION,
        name: options.name.clone(),
        kind: options.kind,
        agent_version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let registered: Registered = ureq::post(&format!("{}{}", options.cloud, protocol::REGISTER))
        .send_json(&agent)?
        .into_json()?;
    let credentials = Credentials {
        cloud: options.cloud.clone(),
        registered: registered.clone(),
    };
    std::fs::write(&options.state, serde_json::to_string_pretty(&credentials)?)?;
    Ok(registered)
}

/// The credentials from an earlier run against the same cloud, if any.
fn kept(options: &Options) -> Option<Registered> {
    let text = std::fs::read_to_string(&options.state).ok()?;
    let credentials: Credentials = serde_json::from_str(&text).ok()?;
    (credentials.cloud == options.cloud).then_some(credentials.registered)
}

fn show_code(registered: &Registered) {
    println!(
        "Add this machine on the website with code  {}  (it waits until then)",
        registered.pairing_code
    );
}

type Link = Box<dyn serialport::SerialPort>;

/// The machine on the port (or on USB HID, a label printer), connected.
enum Machine {
    Marlin(Box<marlin::Printer<Link>>, Known),
    Grbl(Box<grbl::GrblMachine>),
    Dymo(Box<dymo::Printer>),
}

/// Whether the machine on a port is GRBL: it greets by name when the port opens (most
/// boards reset then); if it says nothing, `$I` asks (GRBL answers `[VER:...]`, Marlin that
/// it does not know the command, which does no harm).
fn speaks_grbl(port: &str, baud: u32) -> bool {
    let Ok(mut link) = serialport::new(port, baud)
        .timeout(Duration::from_millis(200))
        .open()
    else {
        return false;
    };
    fn listen(link: &mut Link, heard: &mut String, for_how_long: Duration) {
        let until = Instant::now() + for_how_long;
        let mut buffer = [0u8; 256];
        while Instant::now() < until {
            if let Ok(n) = std::io::Read::read(link, &mut buffer) {
                heard.push_str(&String::from_utf8_lossy(&buffer[..n]));
            }
        }
    }
    let mut heard = String::new();
    listen(&mut link, &mut heard, Duration::from_millis(2500));
    if !heard.lines().any(grbl::greets) {
        let _ = std::io::Write::write_all(&mut link, b"\r\n$I\r\n");
        listen(&mut link, &mut heard, Duration::from_millis(1500));
    }
    heard
        .lines()
        .any(|l| grbl::greets(l) || l.starts_with("[VER:"))
}

/// The machine on the port, by what it speaks; a label printer is found on USB.
fn open_machine(options: &Options) -> Option<Machine> {
    if options.kind == MachineKind::Label {
        return match dymo::find() {
            Ok(Some(printer)) => {
                println!("label printer: {}", printer.product);
                Some(Machine::Dymo(Box::new(printer)))
            }
            Ok(None) => None,
            Err(error) => {
                println!("label printer: {error}");
                None
            }
        };
    }
    let port = options.port.as_ref()?;
    let grbl = match options.firmware {
        Firmware::Grbl => true,
        Firmware::Marlin => false,
        Firmware::Auto => speaks_grbl(port, options.baud),
    };
    if grbl {
        match grbl::GrblMachine::open(port, options.baud) {
            Ok(machine) => {
                println!("GRBL machine on {port}");
                Some(Machine::Grbl(Box::new(machine)))
            }
            Err(error) => {
                println!("machine on {port}: {error}");
                None
            }
        }
    } else {
        open_printer(options).map(|(printer, known)| Machine::Marlin(Box::new(printer), known))
    }
}

/// The computer the agent runs on: its name, and its address towards the cloud (the one
/// its reports leave from: a UDP socket "connected" to the cloud sends nothing).
fn agent_info(cloud: &str) -> protocol::AgentInfo {
    let hostname = std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|h| !h.is_empty());
    let host = cloud
        .split("://")
        .nth(1)
        .unwrap_or(cloud)
        .split('/')
        .next()
        .unwrap_or("");
    let host = if host.contains(':') {
        host.to_owned()
    } else {
        format!("{host}:80")
    };
    let address = std::net::ToSocketAddrs::to_socket_addrs(&host)
        .ok()
        .and_then(|mut a| a.next())
        .and_then(|cloud| {
            let local = if cloud.is_ipv4() {
                "0.0.0.0:0"
            } else {
                "[::]:0"
            };
            let socket = std::net::UdpSocket::bind(local).ok()?;
            socket.connect(cloud).ok()?;
            Some(socket.local_addr().ok()?.ip().to_string())
        });
    protocol::AgentInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        hostname,
        address,
    }
}

/// Fetches a label (a PNG) from the cloud.
fn fetch_label(
    options: &Options,
    token: &str,
    order: &protocol::JobOrder,
) -> Result<Label, String> {
    let response = ureq::get(&format!("{}{}", options.cloud, order.download))
        .set("authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| format!("the label did not come: {e}"))?;
    let mut png = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(response.into_reader(), 16 * 1024 * 1024),
        &mut png,
    )
    .map_err(|e| format!("the label did not come: {e}"))?;
    Label::from_png(&png)
}

/// Prints a label on a LabelManager, on the tape the cloud says is loaded.
fn print_label(
    options: &Options,
    token: &str,
    printer: &dymo::Printer,
    order: &protocol::JobOrder,
) -> Result<(), String> {
    let width = order.tape_mm.unwrap_or(12);
    let tape = dymo::Tape::d1(width).ok_or(format!("{width} mm tape does not fit this printer"))?;
    let label = fetch_label(options, token, order)?;
    printer.print(&label, tape, |sent, of| {
        print!("\rlabel: part {sent} of {of}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    })?;
    println!();
    Ok(())
}

/// Fetches a job's file from the cloud.
fn fetch(options: &Options, token: &str, order: &protocol::JobOrder) -> Result<String, String> {
    let response = ureq::get(&format!("{}{}", options.cloud, order.download))
        .set("authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| format!("the file did not come: {e}"))?;
    let mut text = String::new();
    std::io::Read::read_to_string(
        &mut std::io::Read::take(response.into_reader(), 512 * 1024 * 1024),
        &mut text,
    )
    .map_err(|e| format!("the file did not come: {e}"))?;
    Ok(text)
}

/// What the printer told about itself when the agent connected.
struct Known {
    firmware: Option<String>,
    identity: Identity,
}

/// The printer on the serial port, connected, with what it says it is, or None. `M115`
/// gives its firmware and machine type; a Prusa answers `M862.2 Q` with its model code
/// (other firmware says it does not know the command, which does no harm).
fn open_printer(options: &Options) -> Option<(marlin::Printer<Link>, Known)> {
    let port = options.port.as_ref()?;
    let link = serialport::new(port, options.baud)
        .timeout(Duration::from_millis(200))
        .open()
        .map_err(|error| println!("printer on {port}: {error}"))
        .ok()?;
    let mut printer = marlin::Printer::new(link);
    if let Err(error) = printer.connect(&mut |_| {}) {
        println!("printer on {port} does not answer: {error}");
        return None;
    }
    let about = printer.command("M115").unwrap_or_default();
    let identity = Identity {
        machine_type: marlin::machine_type(&about),
        model_code: printer
            .command("M862.2 Q")
            .ok()
            .and_then(|a| marlin::model_code(&a)),
        printer_type: None,
    };
    let known = Known {
        firmware: marlin::firmware(&about),
        identity,
    };
    println!(
        "printer on {port}: {}, {}{}",
        known.firmware.as_deref().unwrap_or("firmware unknown"),
        known
            .identity
            .machine_type
            .as_deref()
            .unwrap_or("type unknown"),
        known
            .identity
            .model_code
            .map(|c| format!(", model code {c}"))
            .unwrap_or_default(),
    );
    Some((printer, known))
}

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let options = options(args)?;
    let mut registered = match kept(&options) {
        Some(registered) => registered,
        None => register(&options)?,
    };
    println!(
        "{} reports to {} as machine {}",
        options.name, options.cloud, registered.machine_id
    );
    let mut machine: Option<Machine> = None;
    let agent = agent_info(&options.cloud);
    let mut next_look = Instant::now();
    let mut paired = None;
    // What the next report brings: how commands went, a mesh they produced, a job's end.
    let mut done: Vec<CommandDone> = Vec::new();
    let mut mesh = None;
    let mut job_end = None;
    loop {
        let wants = options.port.is_some() || options.kind == MachineKind::Label;
        if machine.is_none() && wants && Instant::now() >= next_look {
            machine = open_machine(&options);
            next_look = Instant::now() + RECONNECT_EVERY;
        }
        let mut telemetry = Telemetry {
            state: MachineState::Disconnected,
            ..Telemetry::default()
        };
        match machine.as_mut() {
            Some(Machine::Marlin(printer, known)) => match printer.command("M105") {
                Ok(answer) => {
                    let (nozzle, bed) = marlin::temperatures(&answer);
                    telemetry.state = MachineState::Idle;
                    telemetry.firmware = known.firmware.clone();
                    telemetry.identity = Some(known.identity.clone());
                    telemetry.nozzle_c = nozzle;
                    telemetry.bed_c = bed;
                }
                Err(error) => {
                    println!("printer lost: {error}");
                    machine = None;
                }
            },
            Some(Machine::Grbl(laser)) => {
                let (report, ended) = laser.report();
                if let Some(end) = ended {
                    println!(
                        "job {} ended: {}",
                        end.id,
                        if end.ok {
                            "done"
                        } else if end.stopped {
                            "stopped"
                        } else {
                            "failed"
                        }
                    );
                    job_end = Some(end);
                }
                let lost = report.state == MachineState::Disconnected;
                telemetry = report;
                if lost {
                    println!("machine lost");
                    machine = None;
                }
            }
            Some(Machine::Dymo(printer)) => match printer.status() {
                Ok(status) => {
                    telemetry.state = if status.problem().is_some() {
                        MachineState::Error
                    } else {
                        MachineState::Idle
                    };
                    telemetry.cassette = Some(status.cassette());
                    telemetry.problem = status.problem().map(str::to_owned);
                    telemetry.identity = Some(Identity {
                        machine_type: Some(printer.product.clone()),
                        ..Identity::default()
                    });
                }
                // Unplugged, or switched off (on its battery, it switches itself off).
                Err(error) => {
                    println!("label printer lost: {error}");
                    machine = None;
                }
            },
            None => {}
        }
        telemetry.agent = Some(agent.clone());
        telemetry.done = std::mem::take(&mut done);
        telemetry.mesh = mesh.take();
        telemetry.job_end = job_end.take();
        let sent = ureq::post(&format!("{}{}", options.cloud, protocol::TELEMETRY))
            .set("authorization", &format!("Bearer {}", registered.token))
            .send_json(&telemetry);
        match sent {
            Ok(response) => {
                let reply: TelemetryReply = response.into_json()?;
                for command in &reply.commands {
                    let outcome = match machine.as_mut() {
                        Some(Machine::Marlin(printer, _)) => carry_out(printer, &command.gcode),
                        Some(Machine::Grbl(laser)) => {
                            laser.carry_out(&command.gcode).map(|said| (said, None))
                        }
                        Some(Machine::Dymo(_)) => Err("a label printer takes no G-code".to_owned()),
                        None => Err("the machine is not connected".to_owned()),
                    };
                    match outcome {
                        Ok((output, found)) => {
                            mesh = found.or(mesh.take());
                            done.push(CommandDone {
                                id: command.id,
                                ok: true,
                                reason: None,
                                output,
                            });
                        }
                        Err(reason) => {
                            println!("command {}: {reason}", command.id);
                            done.push(CommandDone {
                                id: command.id,
                                ok: false,
                                reason: Some(reason),
                                output: Vec::new(),
                            });
                        }
                    }
                }
                if let Some(order) = &reply.job {
                    println!("job {}: {} ({} bytes)", order.id, order.name, order.size);
                    let started = match machine.as_mut() {
                        Some(Machine::Grbl(laser)) => fetch(&options, &registered.token, order)
                            .and_then(|program| {
                                laser.start(order.id, &order.name, program, order.from_line)
                            }),
                        Some(Machine::Marlin(..)) => {
                            Err("printing a file over USB on a Marlin printer follows".to_owned())
                        }
                        // A label takes seconds: printed here, its end in the next report.
                        Some(Machine::Dymo(printer)) => {
                            print_label(&options, &registered.token, printer, order).map(|()| {
                                println!("job {}: printed", order.id);
                                job_end = Some(protocol::JobEnd {
                                    id: order.id,
                                    ok: true,
                                    stopped: false,
                                    reason: None,
                                    restart_line: None,
                                });
                            })
                        }
                        None => Err("the machine is not connected".to_owned()),
                    };
                    if let Err(reason) = started {
                        println!("job {}: {reason}", order.id);
                        job_end = Some(protocol::JobEnd {
                            id: order.id,
                            ok: false,
                            stopped: false,
                            reason: Some(reason),
                            restart_line: None,
                        });
                    }
                }
                if let (Some(control), Some(Machine::Grbl(laser))) =
                    (reply.job_control, machine.as_mut())
                {
                    laser.control(control);
                }
                if paired != Some(reply.paired) {
                    match &reply.space {
                        Some(space) => println!("added to {space}: reporting"),
                        None => show_code(&registered),
                    }
                    paired = Some(reply.paired);
                }
            }
            // The cloud no longer knows this agent (its machine was removed): start over.
            Err(ureq::Error::Status(401, _)) => {
                println!("the cloud does not know this machine any more: registering again");
                registered = register(&options)?;
                paired = None;
            }
            Err(error) => println!("cloud not reached: {error}"),
        }
        // A job reports more often, so its progress moves on the page.
        let busy = matches!(&machine, Some(Machine::Grbl(laser)) if laser.running().is_some());
        std::thread::sleep(if busy {
            Duration::from_secs(1)
        } else {
            REPORT_EVERY
        });
    }
}

/// What carrying out a command gave: what the printer said, and a bed mesh in it.
type Outcome = (Vec<String>, Option<maker_connect_protocol::Mesh>);

/// Sends a command's G-code to the printer line by line, each answered before the next,
/// and finds a bed mesh in what it said (after `G81` or `M420 V`).
fn carry_out<L: std::io::Read + std::io::Write>(
    printer: &mut marlin::Printer<L>,
    gcode: &str,
) -> Result<Outcome, String> {
    let mut said = Vec::new();
    for line in gcode.lines().filter_map(marlin::clean) {
        println!("> {line}");
        said.extend(printer.command(line).map_err(|e| e.to_string())?);
    }
    let mesh = marlin::mesh(&said);
    Ok((said, mesh))
}
