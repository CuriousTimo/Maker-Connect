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
use crate::print::PrintJob;
use maker_connect_grbl as grbl;
use maker_connect_label::{Label, dymo, labelwriter};
use maker_connect_protocol::{
    self as protocol, CommandDone, Identity, MachineKind, MachineState, Register, Registered,
    Telemetry, TelemetryReply,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const REPORT_EVERY: Duration = Duration::from_secs(2);
/// How often an idle DYMO is asked its status (its cassette).
const DYMO_STATUS_EVERY: Duration = Duration::from_secs(15);
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
    /// A Marlin printer printing a file: the job holds it until the print ends.
    Printing(Box<PrintJob<Link>>, Known),
    Grbl(Box<grbl::GrblMachine>),
    Dymo(Box<dymo::Printer>),
    LabelWriter(Box<labelwriter::Printer>),
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
fn open_machine(options: &Options, waiting: &Arc<AtomicBool>) -> Option<Machine> {
    if options.kind == MachineKind::Label {
        match dymo::find() {
            Ok(Some(printer)) => {
                println!("label printer: {}", printer.product);
                return Some(Machine::Dymo(Box::new(printer)));
            }
            Ok(None) => {}
            Err(error) => println!("label printer: {error}"),
        }
        return match labelwriter::find() {
            Ok(Some(printer)) => {
                println!("label printer: {}", printer.product);
                Some(Machine::LabelWriter(Box::new(printer)))
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
        open_printer(options, waiting)
            .map(|(printer, known)| Machine::Marlin(Box::new(printer), known))
    }
}

/// Whether the cloud answers, said when it changes rather than at every report: once when it
/// is lost, every minute while it stays away, and when it is back.
struct CloudLink {
    lost_at: Option<Instant>,
    said_at: Instant,
}

impl CloudLink {
    fn new() -> Self {
        Self {
            lost_at: None,
            said_at: Instant::now(),
        }
    }

    fn lost(&mut self, why: &str) {
        match self.lost_at {
            None => {
                println!("cloud not reached: {why} (trying again every few seconds)");
                self.lost_at = Some(Instant::now());
                self.said_at = Instant::now();
            }
            Some(since) if self.said_at.elapsed() >= Duration::from_secs(60) => {
                println!(
                    "cloud still not reached after {}: {why}",
                    how_long(since.elapsed())
                );
                self.said_at = Instant::now();
            }
            Some(_) => {}
        }
    }

    fn reached(&mut self) {
        if let Some(since) = self.lost_at.take() {
            println!("cloud reached again after {}", how_long(since.elapsed()));
        }
    }
}

/// "45 s", "3 min 20 s", "2 h 5 min".
fn how_long(time: Duration) -> String {
    let s = time.as_secs();
    match s {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min {} s", s / 60, s % 60),
        s => format!("{} h {} min", s / 3600, s % 3600 / 60),
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
        build: Some(env!("MC_COMMIT").to_owned()),
        built: Some(env!("MC_BUILT").to_owned()),
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
    printer: &mut dymo::Printer,
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
fn open_printer(
    options: &Options,
    waiting: &Arc<AtomicBool>,
) -> Option<(marlin::Printer<Link>, Known)> {
    let port = options.port.as_ref()?;
    let link = serialport::new(port, options.baud)
        .timeout(Duration::from_millis(200))
        .open()
        .map_err(|error| println!("printer on {port}: {error}"))
        .ok()?;
    let mut printer = marlin::Printer::new(link);
    // Shared before connecting: a printer waiting for its knob says so meanwhile.
    printer.set_waiting(waiting.clone());
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
    println!("{}", crate::version());
    let mut cloud = CloudLink::new();
    // A first run with the cloud away waits for it, rather than giving up.
    let mut registered = match kept(&options) {
        Some(registered) => registered,
        None => loop {
            match register(&options) {
                Ok(registered) => {
                    cloud.reached();
                    break registered;
                }
                Err(error) => {
                    cloud.lost(&error.to_string());
                    std::thread::sleep(REPORT_EVERY);
                }
            }
        },
    };
    println!(
        "{} reports to {} as machine {}",
        options.name, options.cloud, registered.machine_id
    );
    let mut machine: Option<Machine> = None;
    let agent = agent_info(&options.cloud);
    let mut next_look = Instant::now();
    let mut paired = None;
    // A label printer's battery under the minimum to print (see `dymo::BATTERY_PRINT_MIN_V`).
    let mut battery_too_low = false;
    // What the next report brings: how commands went, a mesh they produced, a job's end.
    let mut done: Vec<CommandDone> = Vec::new();
    let mut mesh = None;
    let mut job_end = None;
    // The last report, sent again meanwhile while the printer keeps the agent waiting
    // (connecting, a long command, a question on its screen).
    let mut last = Telemetry {
        state: MachineState::Disconnected,
        agent: Some(agent.clone()),
        ..Telemetry::default()
    };
    loop {
        let wants = options.port.is_some() || options.kind == MachineKind::Label;
        if machine.is_none() && wants && Instant::now() >= next_look {
            let waiting = Arc::new(AtomicBool::new(false));
            machine = while_busy(&options, &registered.token, &last, &waiting, || {
                open_machine(&options, &waiting)
            });
            next_look = Instant::now() + RECONNECT_EVERY;
        }
        let mut telemetry = Telemetry {
            state: MachineState::Disconnected,
            ..Telemetry::default()
        };
        // A print that has ended hands its printer back.
        if let Some(Machine::Printing(job, _)) = machine.as_mut()
            && let Some((printer, end)) = job.finished()
        {
            println!(
                "job {} ended: {}",
                end.id,
                if end.ok {
                    "printed"
                } else if end.stopped {
                    "stopped"
                } else {
                    end.reason.as_deref().unwrap_or("failed")
                }
            );
            let lost = end.reason.is_some();
            job_end = Some(end);
            if let Some(Machine::Printing(_, known)) = machine.take() {
                // A printer that stopped answering is looked for again.
                machine = (!lost).then(|| Machine::Marlin(Box::new(printer), known));
            }
        }
        match machine.as_mut() {
            Some(Machine::Printing(job, known)) => {
                let p = job.progress();
                for said in &p.said {
                    println!("< {said}");
                }
                telemetry.state = if p.paused {
                    MachineState::Paused
                } else {
                    MachineState::Busy
                };
                // An M0 or a filament change in the file: it waits for someone.
                if job.waiting.load(Ordering::Relaxed) {
                    telemetry.state = MachineState::Attention;
                    telemetry.problem =
                        Some("waits for someone at the printer: press its knob".to_owned());
                }
                telemetry.firmware = known.firmware.clone();
                telemetry.identity = Some(known.identity.clone());
                set_heat(&mut telemetry, p.heat);
                telemetry.job_id = Some(job.id);
                // What is left, from how long the lines so far took.
                let remaining_s =
                    (p.done > 20).then(|| p.elapsed_s * (p.total - p.done) as u64 / p.done as u64);
                telemetry.job = Some(protocol::JobProgress {
                    file: job.name.clone(),
                    done: p.done as u64,
                    total: p.total as u64,
                    elapsed_s: Some(p.elapsed_s),
                    remaining_s,
                });
            }
            Some(Machine::Marlin(printer, known)) => {
                let waiting = printer.waiting();
                let answer = while_busy(&options, &registered.token, &last, &waiting, || {
                    printer.command("M105")
                });
                match answer {
                    Ok(answer) => {
                        telemetry.state = MachineState::Idle;
                        telemetry.firmware = known.firmware.clone();
                        telemetry.identity = Some(known.identity.clone());
                        set_heat(&mut telemetry, marlin::heat(&answer));
                    }
                    Err(error) => {
                        println!("printer lost: {error}");
                        machine = None;
                    }
                }
            }
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
            // Not asked for its status while idle (see `dymo::Printer::last`): only whether
            // it is still on USB; its cassette as it said on connecting and around labels.
            Some(Machine::Dymo(printer)) => {
                if printer.present() {
                    // Asked now and then while idle (not at every report): its cassette.
                    let due = printer
                        .asked_at
                        .is_none_or(|at| at.elapsed() >= DYMO_STATUS_EVERY);
                    if due && let Err(error) = printer.status() {
                        println!("label printer does not answer: {error}");
                    }
                    let status = printer.last;
                    telemetry.raw_status =
                        (!printer.last_raw.is_empty()).then(|| dymo::hex(&printer.last_raw));
                    // The battery: too low to print below the minimum, until it is back above
                    // "low" (so it does not flap around the line).
                    let battery = dymo::battery_volts(&printer.last_raw);
                    telemetry.battery_v = battery;
                    if let Some(v) = battery {
                        if v < dymo::BATTERY_PRINT_MIN_V {
                            battery_too_low = true;
                        } else if v >= dymo::BATTERY_LOW_V {
                            battery_too_low = false;
                        }
                    }
                    // No cassette, or a battery too low: it waits for someone (attention),
                    // the reason said as the problem; only a jammed cutter or its error bit
                    // is an error.
                    telemetry.state = match status {
                        Some(s) if s.cutter_jammed() || s.error() => MachineState::Error,
                        Some(s) if !s.cassette() => MachineState::Attention,
                        _ if battery_too_low => MachineState::Attention,
                        _ => MachineState::Idle,
                    };
                    telemetry.cassette = status.map(|s| s.cassette());
                    telemetry.problem = status.and_then(|s| s.problem()).map(str::to_owned);
                    if telemetry.problem.is_none() && battery_too_low {
                        telemetry.problem = Some(format!(
                            "battery too low to print reliably ({:.2} V)",
                            battery.unwrap_or_default()
                        ));
                    }
                    telemetry.identity = Some(Identity {
                        machine_type: Some(printer.product.clone()),
                        ..Identity::default()
                    });
                } else {
                    // Unplugged, or switched off (on its battery, it switches itself off).
                    println!("label printer gone from USB");
                    machine = None;
                }
            }
            // A LabelWriter: on mains power, asked now and then; no labels waits for someone.
            Some(Machine::LabelWriter(printer)) => {
                if printer.present() {
                    let due = printer
                        .asked_at
                        .is_none_or(|at| at.elapsed() >= DYMO_STATUS_EVERY);
                    if due && let Err(error) = printer.status() {
                        println!("label printer does not answer: {error}");
                    }
                    let status = printer.last;
                    telemetry.raw_status = status.map(|s| format!("{:02x}", s.0));
                    telemetry.state = match status {
                        Some(s) if s.jammed() || (s.error() && !s.no_paper()) => {
                            MachineState::Error
                        }
                        Some(s) if s.no_paper() => MachineState::Attention,
                        _ => MachineState::Idle,
                    };
                    telemetry.problem = status.and_then(|s| s.problem()).map(str::to_owned);
                    telemetry.identity = Some(Identity {
                        machine_type: Some(printer.product.clone()),
                        ..Identity::default()
                    });
                } else {
                    println!("label printer gone from USB");
                    machine = None;
                }
            }
            None => {}
        }
        telemetry.agent = Some(agent.clone());
        last = Telemetry {
            done: Vec::new(),
            mesh: None,
            job_end: None,
            ..telemetry.clone()
        };
        telemetry.done = std::mem::take(&mut done);
        telemetry.mesh = mesh.take();
        telemetry.job_end = job_end.take();
        let sent = ureq::post(&format!("{}{}", options.cloud, protocol::TELEMETRY))
            .set("authorization", &format!("Bearer {}", registered.token))
            .send_json(&telemetry);
        match sent {
            Ok(response) => {
                // An answer that does not read (a cloud restarting behind a proxy): try again.
                let reply: TelemetryReply = match response.into_json() {
                    Ok(reply) => {
                        cloud.reached();
                        reply
                    }
                    Err(error) => {
                        cloud.lost(&format!("its answer did not read: {error}"));
                        std::thread::sleep(REPORT_EVERY);
                        continue;
                    }
                };
                // What is said meanwhile: busy, with the last temperatures, so a long command
                // (homing, measuring the bed, heating) does not look like a machine gone.
                let busy = Telemetry {
                    state: MachineState::Busy,
                    done: Vec::new(),
                    mesh: None,
                    job_end: None,
                    ..telemetry.clone()
                };
                for command in &reply.commands {
                    let outcome = match machine.as_mut() {
                        Some(Machine::Marlin(printer, _)) => {
                            let waiting = printer.waiting();
                            while_busy(&options, &registered.token, &busy, &waiting, || {
                                carry_out(printer, &command.gcode)
                            })
                        }
                        Some(Machine::Grbl(laser)) => {
                            while_busy(&options, &registered.token, &busy, &Arc::default(), || {
                                laser.carry_out(&command.gcode)
                            })
                            .map(|said| (said, None))
                        }
                        Some(Machine::Dymo(_) | Machine::LabelWriter(_)) => {
                            Err("a label printer takes no G-code".to_owned())
                        }
                        Some(Machine::Printing(..)) => {
                            Err("printing: commands wait until the print ends".to_owned())
                        }
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
                    // A Marlin printer: the file fetched, then streamed by a job that holds
                    // the printer until it ends.
                    if let Some(Machine::Marlin(..)) = machine {
                        match fetch(&options, &registered.token, order) {
                            // The printer goes to the job only once its file is here.
                            Ok(gcode) => {
                                if let Some(Machine::Marlin(printer, known)) = machine.take() {
                                    let job =
                                        PrintJob::start(order.id, &order.name, gcode, *printer);
                                    machine = Some(Machine::Printing(Box::new(job), known));
                                }
                            }
                            Err(reason) => {
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
                    }
                    let started = match machine.as_mut() {
                        Some(Machine::Printing(job, _)) if job.id == order.id => Ok(()),
                        Some(Machine::Printing(..)) => Err("printing another file".to_owned()),
                        Some(Machine::Grbl(laser)) => fetch(&options, &registered.token, order)
                            .and_then(|program| {
                                laser.start(order.id, &order.name, program, order.from_line)
                            }),
                        // Its file did not come (said above).
                        Some(Machine::Marlin(..)) => Ok(()),
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
                        Some(Machine::LabelWriter(printer)) => {
                            fetch_label(&options, &registered.token, order)
                                .and_then(|label| {
                                    let length = order
                                        .label_length_mm
                                        .map(|mm| labelwriter::dots_for_mm(mm as f64));
                                    printer.print(&label, length)
                                })
                                .map(|()| {
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
                match (reply.job_control, machine.as_mut()) {
                    (Some(control), Some(Machine::Grbl(laser))) => laser.control(control),
                    (Some(protocol::JobControl::Pause), Some(Machine::Printing(job, _))) => {
                        job.pause(true)
                    }
                    (Some(protocol::JobControl::Resume), Some(Machine::Printing(job, _))) => {
                        job.pause(false)
                    }
                    (Some(protocol::JobControl::Stop), Some(Machine::Printing(job, _))) => {
                        job.stop()
                    }
                    _ => {}
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
                cloud.reached();
                println!("the cloud does not know this machine any more: registering again");
                match register(&options) {
                    Ok(again) => {
                        registered = again;
                        paired = None;
                    }
                    Err(error) => cloud.lost(&format!("registering again failed: {error}")),
                }
            }
            Err(error) => cloud.lost(&error.to_string()),
        }
        // A job reports more often, so its progress moves on the page.
        let busy = matches!(&machine, Some(Machine::Grbl(laser)) if laser.running().is_some())
            || matches!(&machine, Some(Machine::Printing(..)));
        std::thread::sleep(if busy {
            Duration::from_secs(1)
        } else {
            REPORT_EVERY
        });
    }
}

/// The heaters as the printer told them, into the report.
fn set_heat(telemetry: &mut Telemetry, heat: marlin::Heat) {
    telemetry.nozzle_c = heat.nozzle_c;
    telemetry.nozzle_target_c = heat.nozzle_target_c;
    telemetry.bed_c = heat.bed_c;
    telemetry.bed_target_c = heat.bed_target_c;
    telemetry.nozzle_heating = heat.nozzle_heating;
    telemetry.bed_heating = heat.bed_heating;
}

/// Does `work` (a command the machine takes a while over), and meanwhile reports `busy` to
/// the cloud every [`REPORT_EVERY`], so the machine stays online there. The cloud gives a
/// busy machine no command and no job, so its answers carry nothing to act on.
fn while_busy<T>(
    options: &Options,
    token: &str,
    busy: &Telemetry,
    waiting: &Arc<AtomicBool>,
    work: impl FnOnce() -> T,
) -> T {
    let (done, wait) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                wait.recv_timeout(REPORT_EVERY)
            {
                let report = waiting_for_someone(busy, waiting);
                let _ = ureq::post(&format!("{}{}", options.cloud, protocol::TELEMETRY))
                    .set("authorization", &format!("Bearer {token}"))
                    .send_json(&report);
            }
        });
        let result = work();
        drop(done);
        result
    })
}

/// A report as it is, or, while the printer waits for someone at it (`paused for user`),
/// as needing attention, with why.
fn waiting_for_someone(report: &Telemetry, waiting: &AtomicBool) -> Telemetry {
    let mut report = report.clone();
    if waiting.load(Ordering::Relaxed) {
        report.state = MachineState::Attention;
        report.problem = Some("waits for someone at the printer: press its knob".to_owned());
    }
    report
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_well() {
        assert_eq!(how_long(Duration::from_secs(45)), "45 s");
        assert_eq!(how_long(Duration::from_secs(200)), "3 min 20 s");
        assert_eq!(how_long(Duration::from_secs(7500)), "2 h 5 min");
    }
}
