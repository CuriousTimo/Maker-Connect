//! `run`: the agent reports its machine to the hub. It registers once (the credentials
//! are kept in a small state file, so a restart keeps the machine), shows the pairing code
//! until someone adds the machine on the website, and then keeps reporting what the machine
//! does every two seconds. A printer on USB is asked for its firmware (`M115`) once and its
//! temperatures (`M105`) every report; when it is switched off or unplugged the agent says
//! so and tries again. Commands come back in the hub's answer: the agent sends their G-code
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
use maker_connect_update as update;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const REPORT_EVERY: Duration = Duration::from_secs(2);
/// How often an idle DYMO is asked its status (its cassette).
const DYMO_STATUS_EVERY: Duration = Duration::from_secs(15);
/// How often a missing printer is looked for again.
const RECONNECT_EVERY: Duration = Duration::from_secs(10);

struct Options {
    hub: String,
    name: String,
    kind: MachineKind,
    port: Option<String>,
    baud: u32,
    firmware: Firmware,
    state: String,
    /// Register again for a new pairing code, forgetting the credentials kept (`--new-code`).
    new_code: bool,
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
    let mut new_code = false;
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .ok_or_else(|| format!("expected a value after {flag}"))
        };
        match flag.as_str() {
            // `--cloud`: its name before 0.2.0-beta.2.
            "--hub" | "--cloud" => typed.hub = Some(value()?),
            "--name" => typed.name = Some(value()?),
            "--kind" => typed.kind = Some(value()?),
            "--port" => typed.port = Some(value()?),
            "--baud" => typed.baud = Some(value()?.parse()?),
            "--firmware" => typed.firmware = Some(value()?),
            "--state" => typed.state = Some(value()?),
            "--config" => named = Some(value()?),
            "--save" => save = true,
            "--new-code" => new_code = true,
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
    let (Some(hub), Some(name)) = (all.hub, all.name) else {
        return Err(format!(
            "run needs a hub and a name: --hub <address> --name <name>, or in {}",
            config::FILE
        )
        .into());
    };
    Ok(Options {
        hub: hub.trim_end_matches('/').to_owned(),
        name,
        kind,
        port: all.port,
        baud: all.baud.unwrap_or(115_200),
        firmware,
        state: state.to_string_lossy().into_owned(),
        new_code,
    })
}

/// The agent's credentials, kept between runs.
#[derive(serde::Serialize, serde::Deserialize)]
struct Credentials {
    /// The hub they are for; `cloud` in files from before 0.2.0-beta.2 (read as well, so an
    /// agent updated by hand keeps its machine).
    #[serde(alias = "cloud")]
    hub: String,
    registered: Registered,
}

fn register(options: &Options) -> Result<Registered, Box<dyn std::error::Error>> {
    let agent = Register {
        protocol: protocol::VERSION,
        name: options.name.clone(),
        kind: options.kind,
        agent_version: crate::RELEASE.to_owned(),
    };
    let registered: Registered = ureq::post(&format!("{}{}", options.hub, protocol::REGISTER))
        .send_json(&agent)?
        .into_json()?;
    let credentials = Credentials {
        hub: options.hub.clone(),
        registered: registered.clone(),
    };
    std::fs::write(&options.state, serde_json::to_string_pretty(&credentials)?)?;
    Ok(registered)
}

/// The credentials from an earlier run against the same hub, if any.
fn kept(options: &Options) -> Option<Registered> {
    if options.new_code {
        // Asked for a new code: the old credentials go, so they are not used again.
        if std::fs::remove_file(&options.state).is_ok() {
            println!(
                "forgot the old registration ({}): registering again",
                options.state
            );
        }
        return None;
    }
    let text = std::fs::read_to_string(&options.state).ok()?;
    let credentials: Credentials = serde_json::from_str(&text).ok()?;
    (credentials.hub == options.hub).then_some(credentials.registered)
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
fn open_machine(options: &Options, waiting: &Arc<marlin::Screen>) -> Option<Machine> {
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

/// Whether the hub answers, said when it changes rather than at every report: once when it
/// is lost, every minute while it stays away, and when it is back.
struct HubLink {
    lost_at: Option<Instant>,
    said_at: Instant,
}

impl HubLink {
    fn new() -> Self {
        Self {
            lost_at: None,
            said_at: Instant::now(),
        }
    }

    fn lost(&mut self, why: &str) {
        match self.lost_at {
            None => {
                println!("hub not reached: {why} (trying again every few seconds)");
                self.lost_at = Some(Instant::now());
                self.said_at = Instant::now();
            }
            Some(since) if self.said_at.elapsed() >= Duration::from_secs(60) => {
                println!(
                    "hub still not reached after {}: {why}",
                    how_long(since.elapsed())
                );
                self.said_at = Instant::now();
            }
            Some(_) => {}
        }
    }

    fn reached(&mut self) {
        if let Some(since) = self.lost_at.take() {
            println!("hub reached again after {}", how_long(since.elapsed()));
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

/// The computer the agent runs on: its name, and its address towards the hub (the one
/// its reports leave from: a UDP socket "connected" to the hub sends nothing).
fn agent_info(hub: &str) -> protocol::AgentInfo {
    let hostname = std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|h| !h.is_empty());
    let host = hub
        .split("://")
        .nth(1)
        .unwrap_or(hub)
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
        .and_then(|hub| {
            let local = if hub.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
            let socket = std::net::UdpSocket::bind(local).ok()?;
            socket.connect(hub).ok()?;
            Some(socket.local_addr().ok()?.ip().to_string())
        });
    protocol::AgentInfo {
        version: crate::RELEASE.to_owned(),
        build: Some(env!("MC_COMMIT").to_owned()),
        built: Some(env!("MC_BUILT").to_owned()),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        hostname,
        address,
        can_update: true,
        last_update: std::env::current_exe()
            .ok()
            .and_then(|exe| update::last(&exe))
            .map(outcome),
    }
}

/// How the last update went, as the hub is told.
fn outcome(o: update::Outcome) -> protocol::UpdateOutcome {
    protocol::UpdateOutcome {
        at: o.at,
        from: o.from,
        to: o.to,
        ok: o.ok,
        note: o.note,
    }
}

/// The name of this agent's build in a release: `maker-connect-agent-linux-armv6`.
fn own_file() -> Option<String> {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    update::this_system().map(|system| format!("maker-connect-agent-{system}{exe}"))
}

/// Fetches the build the hub offers, checks it against the release key, tries it
/// (`--version`) and puts it in place, the running one kept for the way back.
fn self_update(
    options: &Options,
    token: &str,
    exe: &std::path::Path,
    order: &protocol::AgentUpdate,
) -> Result<(), String> {
    if own_file().as_deref() != Some(order.file.as_str()) {
        return Err(format!(
            "the hub offers {}, not this system's build",
            order.file
        ));
    }
    if !update::newer(&order.version, crate::RELEASE) {
        return Err(format!(
            "{} is not newer than {}",
            order.version,
            crate::RELEASE
        ));
    }
    if order.tag != format!("agent-{}", order.version) {
        return Err(format!(
            "{} is not the release of {}",
            order.tag, order.version
        ));
    }
    // Before fetching: a list that is not signed is not worth the download.
    update::verify(
        &order.tag,
        &order.sums,
        &order.signature,
        &update::RELEASE_KEY,
    )?;
    let response = ureq::get(&format!("{}{}", options.hub, order.download))
        .set("authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| format!("could not fetch it: {e}"))?;
    let mut build = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(response.into_reader(), 128 << 20),
        &mut build,
    )
    .map_err(|e| format!("could not fetch it: {e}"))?;
    update::check(
        &build,
        &order.file,
        &order.tag,
        &order.sums,
        &order.signature,
        &update::RELEASE_KEY,
    )?;
    update::install(
        exe,
        &build,
        &["--version".into()],
        crate::RELEASE,
        &order.version,
        TRIAL_S,
        serde_json::Value::Null,
    )
}

/// Whether the agent may update now: no job on the machine, and it is not busy or paused.
/// A machine that waits for someone without a job (a label printer without its cassette, a
/// question on a printer's screen) or is not connected may: nothing of it runs.
fn may_update(report: &Telemetry, printing: bool) -> bool {
    !printing
        && report.job_id.is_none()
        && !matches!(report.state, MachineState::Busy | MachineState::Paused)
}

/// How long a new build has to reach the hub before it is rolled back (the old one waits).
const TRIAL_S: i64 = 180;

/// Fetches a label (a PNG) from the hub.
fn fetch_label(
    options: &Options,
    token: &str,
    order: &protocol::JobOrder,
) -> Result<Label, String> {
    let response = ureq::get(&format!("{}{}", options.hub, order.download))
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

/// Prints a label on a LabelManager, on the tape the hub says is loaded.
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

/// Fetches a job's file from the hub.
fn fetch(
    options: &Options,
    token: &str,
    order: &protocol::JobOrder,
    mut progress: impl FnMut(f64),
) -> Result<String, String> {
    let response = ureq::get(&format!("{}{}", options.hub, order.download))
        .set("authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| format!("the file did not come: {e}"))?;
    let mut reader = std::io::Read::take(response.into_reader(), 512 * 1024 * 1024);
    let (mut bytes, mut chunk) = (Vec::new(), vec![0u8; 64 * 1024]);
    let mut told = Instant::now();
    loop {
        let n = std::io::Read::read(&mut reader, &mut chunk).map_err(|e| format!("the file did not come: {e}"))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        // How far, now and then (its screen shows it, as PrusaLink shows a transfer).
        if told.elapsed() >= Duration::from_millis(500) && order.size > 0 {
            told = Instant::now();
            progress((bytes.len() as f64 * 100.0 / order.size as f64).min(100.0));
        }
    }
    String::from_utf8(bytes).map_err(|e| format!("the file did not come: {e}"))
}

/// What the printer said by itself and what was read back, into the report: speed and
/// flow, its sheet and print mode, its SD card, its MMU, "Reprint" from its menu, and what it
/// is doing by itself (calibrating its thermal model: busy, nothing sent meanwhile).
fn screen_facts(
    telemetry: &mut Telemetry,
    waiting: &marlin::Screen,
    known: &Known,
    factors: (Option<f32>, Option<f32>),
) {
    let screen = waiting.now();
    (telemetry.speed_percent, telemetry.flow_percent) = factors;
    telemetry.sheet = known.sheet.clone();
    telemetry.print_mode = known.print_mode.clone();
    (telemetry.total_filament_m, telemetry.total_print_min) = known.totals;
    telemetry.sd_card = screen.sd_card;
    telemetry.mmu_slot = screen.mmu_slot;
    telemetry.mmu_doing = screen.mmu_doing.clone();
    telemetry.reprint = waiting.take_reprint();
    if screen.calibrating {
        telemetry.state = MachineState::Busy;
        telemetry.doing = Some("Calibrating its thermal model".to_owned());
    }
}

/// Prusa's MK3 reports its temperatures, position and fans by itself every 2 s
/// (`M155 S2 C7`), as PrusaLink keeps it (auto_telemetry.py); switched on again when any of
/// them has not come for a minute (its `REPORTING_TIMEOUT`).
fn start_reports(printer: &mut marlin::Printer<Link>) {
    if printer.command("M155 S2 C7").is_ok() {
        printer.waiting().reporting_started();
    }
}

/// How long Prusa's MK3 may be silent about any of its reports before `M155` is sent again.
const REPORTS_SILENT: Duration = Duration::from_secs(60);

/// Prusa's MK2.5 and MK2.5S (model codes 250, 252; with an MMU 20250, 20252, which
/// `marlin::model_code` gives as 250, 252): they level without saying so over the wire.
fn is_mk25(known: &Known) -> bool {
    matches!(known.identity.model_code, Some(250 | 252))
}

/// What the printer told about itself when the agent connected, and what is asked of it
/// now and then (Prusa's MK3: its sheet and print mode, from its EEPROM).
struct Known {
    firmware: Option<String>,
    identity: Identity,
    sheet: Option<String>,
    print_mode: Option<String>,
    /// When its EEPROM was last read; PrusaLink reads the sheets every 30 s
    /// (`VERY_SLOW_POLL_INTERVAL`) and the print mode every 10 s.
    eeprom_at: Option<Instant>,
    /// Its lifetime totals (metres of filament, minutes printing), read when connecting and
    /// again after each print (PrusaLink's `invalidate_statistics`).
    totals: (Option<u32>, Option<u32>),
    totals_due: bool,
    /// An SD print was running at the last report (its end makes the totals due).
    sd_printing: bool,
    /// After a power cut: the line number it saved (`D3 Ax0F91 C4`, PrusaLink's
    /// `EEPROM_FILE_POSITION`), read once while it waits to be resumed.
    saved_line: Option<u32>,
    /// Ready for the next job (PrusaLink's READY): told so (`M72 S1`), until a print starts
    /// or it is told otherwise (`M72 S0`).
    ready: bool,
    /// Its job id was read from its EEPROM (PrusaLink: `JOB_ID` condition otherwise).
    job_id_read: bool,
    /// The speed it said last (`M220`).
    speed: Option<f32>,
}

/// Prusa's MK3 EEPROM: the line number saved at a power cut during a host's print (the
/// firmware's `saved_sdpos`, `power_panic.cpp`); resumed, it sets its own number to it
/// (`M110 N…`) and asks for the next one.
const EEPROM_FILE_POSITION: &str = "D3 AxF91 C4";

/// Prusa's MK3 EEPROM: the job id (PrusaLink's `EEPROMParams.JOB_ID`, 0x0D05, 4 bytes), read
/// when it connects and written when a job starts (`job.py` `write`: `D3 Ax0D05 X…`).
const EEPROM_JOB_ID: &str = "D3 AxD05 C4";

/// Prusa's MK3 EEPROM: lifetime filament (metres) and printing time (minutes), as
/// PrusaLink reads them (`EEPROMParams.TOTAL_FILAMENT`, `TOTAL_PRINT_TIME`).
const EEPROM_TOTAL_FILAMENT: &str = "D3 AxFF1 C4";
const EEPROM_TOTAL_TIME: &str = "D3 AxFED C4";

/// How often Prusa's MK3 EEPROM is read for its sheet and print mode.
const EEPROM_EVERY: Duration = Duration::from_secs(10);

/// Prusa's MK3 EEPROM (`eeprom.h` of firmware 3.14, as PrusaLink reads it,
/// `EEPROMParams`): the eight sheets (88 bytes), the active one, the print mode.
const EEPROM_SHEETS: &str = "D3 AxD49 C88";
const EEPROM_ACTIVE_SHEET: &str = "D3 AxDA1 C1";
const EEPROM_PRINT_MODE: &str = "D3 AxFFF C1";

/// Whether the port is one PrusaLink takes for Prusa's MK3 family without asking: Prusa's
/// USB (vendor 2c99, product 0001 or 0002: `const.py` `SUPPORTED_PRINTERS`), or the Pi's own
/// header, which PrusaLink is made for (`/dev/serial0`, `/dev/ttyAMA0`).
fn prusa_port(port: &str) -> bool {
    if matches!(port, "/dev/serial0" | "/dev/ttyAMA0") {
        return true;
    }
    let real = std::fs::canonicalize(port).ok();
    serialport::available_ports().unwrap_or_default().into_iter().any(|p| {
        let same = p.port_name == port || real.as_deref() == Some(std::path::Path::new(&p.port_name));
        same && matches!(p.port_type, serialport::SerialPortType::UsbPort(ref usb) if usb.vid == 0x2c99 && matches!(usb.pid, 1 | 2))
    })
}

/// PrusaLink's menu on the card, as PrusaLink keeps it (`handle_special_menu`): a folder
/// named "PrusaLink menu" gets its `setready.g` (`M28 <folder>/setready.g`, `M84`, `M29`)
/// when it has none; choosing it on the printer is "Set Ready" (crate::marlin, `Screen`).
fn special_menu(printer: &mut marlin::Printer<Link>, listed: &[String]) {
    let menu = marlin::prusalink_menu(listed);
    if let Some((folder, false)) = &menu {
        for code in [format!("M28 {folder}/setready.g"), "M84".to_owned(), "M29".to_owned()] {
            let _ = printer.command(&code);
        }
    }
    printer.waiting().set_menu(menu.map(|(folder, _)| folder));
}

/// Prusa's MK3 family by its model code (`const.py` `PRINTER_TYPES`), named as its `M115`
/// names it.
fn mk3_family_type(code: u32) -> Option<&'static str> {
    Some(match code {
        250 | 20250 => "Prusa i3 MK2.5",
        252 | 20252 => "Prusa i3 MK2.5S",
        300 | 20300 => "Prusa i3 MK3",
        302 | 20302 | 30302 => "Prusa i3 MK3S",
        _ => return None,
    })
}

/// Its MMU3 attached (`MMU3_TYPE_CODE`).
const MMU3_TYPE_CODE: u32 = 30302;

/// What the printer is, asked once it answers (after `connect`). Prusa's MK3 family as
/// PrusaLink asks it (`printer_polling.py`, `printer_info` in its order; `sd_card.py`;
/// `ip_updater.py`; `auto_telemetry.py`): its type `M862.2 Q`, firmware `PRUSA Fir`, nozzle
/// `M862.1 Q`, serial number `PRUSA SN` (not of an MK2.5), its MMU `M707` (only with an MMU3
/// attached), its job id `D3 AxD05 C4`; its card listed (`M20 LT`, and `M21` when it lists
/// nothing); the agent's address on its screen (`M552`); its own reports from now on. Never
/// `M115`. `prusa`: it said it is one of the MK3 family when connected
/// (`Printer::connect_as_prusalink`). Other firmware: `M115`, and Prusa's Buddy `M862.2 Q`.
/// Apart from opening the port, so a test can run it against a fake printer.
fn get_to_know(printer: &mut marlin::Printer<Link>, address: Option<String>, prusa: bool) -> Known {
    let ask = |printer: &mut marlin::Printer<Link>, code: &str| printer.command(code).unwrap_or_default();
    // Its polling, numbered, in PrusaLink's order: type, then firmware.
    let (code, version) = if prusa {
        let code = marlin::type_code(&ask(printer, "M862.2 Q"));
        (code, marlin::prusa_fir(&ask(printer, "PRUSA Fir")))
    } else {
        (None, None)
    };
    let mut known = Known {
        firmware: None,
        identity: Identity::default(),
        sheet: None,
        print_mode: None,
        eeprom_at: None,
        totals: (None, None),
        totals_due: true,
        sd_printing: false,
        saved_line: None,
        ready: false,
        job_id_read: false,
        speed: None,
    };
    if let (Some(code), Some(version)) = (code, version)
        && let Some(kind) = mk3_family_type(code)
    {
        let firmware = format!("Prusa-Firmware {version}");
        printer.firmware = Some(firmware.clone());
        known.firmware = Some(firmware);
        known.identity.machine_type = Some(kind.to_owned());
        known.identity.model_code = Some(code % 10_000);
        known.identity.nozzle_mm = marlin::nozzle(&ask(printer, "M862.1 Q"));
        if !matches!(code, 250 | 252 | 20250 | 20252) {
            known.identity.serial = marlin::prusa_serial(&ask(printer, "PRUSA SN"));
        }
        if code == MMU3_TYPE_CODE {
            let mmu: Vec<Vec<String>> = (0..4).map(|r| ask(printer, &format!("M707 A0x0{r}"))).collect();
            known.identity.mmu_firmware = marlin::mmu_firmware([&mmu[0], &mmu[1], &mmu[2], &mmu[3]]);
        }
        known.job_id_read = marlin::eeprom_u32(&marlin::eeprom_bytes(&ask(printer, EEPROM_JOB_ID))).is_some();
        let listed = ask(printer, "M20 LT");
        if !marlin::lists_files(&listed) {
            ask(printer, "M21");
        }
        special_menu(printer, &listed);
        match address.filter(|a| a.parse::<std::net::Ipv4Addr>().is_ok()) {
            Some(ip) => ask(printer, &format!("M552 P{ip}")),
            None => ask(printer, "M552 P0.0.0.0"),
        };
        start_reports(printer);
        return known;
    }
    // Not Prusa's MK3 family.
    let about = printer.command("M115").unwrap_or_default();
    // Prusa's model code: only asked of Prusa's firmware (in Marlin, M862 is an I2C
    // position encoder's check).
    let prusa_firmware = marlin::firmware(&about).is_some_and(|f| f.starts_with("Prusa"));
    known.identity.machine_type = marlin::machine_type(&about);
    known.identity.model_code = prusa_firmware
        .then(|| printer.command("M862.2 Q").ok())
        .flatten()
        .and_then(|a| marlin::model_code(&a));
    known.firmware = marlin::firmware(&about);
    known
}

/// One look at an idle printer (as each report): keeps its MK3 reporting by itself, asks its
/// temperatures (and, where it does not report it, its position), its speed and flow, and on
/// Prusa's MK3 its lifetime totals and, now and then, its sheet and print mode. Returns what
/// it answered and the speed and flow factors. Apart from the loop, so a test can run it
/// against a fake printer.
fn ask_idle(
    printer: &mut marlin::Printer<Link>,
    known: &mut Known,
) -> std::io::Result<(Vec<String>, (Option<f32>, Option<f32>))> {
    let waiting = printer.waiting();
    let mk3 = printer.is_prusa_mk3();
    printer.keep_host_alive(&mut |_| {})?;
    // Prusa's MK3 reports temperatures, position and fans by itself (M155, as PrusaLink
    // keeps it); asked again when it went quiet.
    if mk3 && !waiting.reports_by_itself(REPORTS_SILENT) {
        start_reports(printer);
    }
    // Its temperatures; and, where it does not report it by itself, where its head is
    // (M114, any Marlin).
    // Prusa's MK3 is asked none of these: what it reported by itself is read (PrusaLink
    // reads only its M155 reports).
    let mut answer = if mk3 { waiting.reported() } else { printer.command("M105")? };
    if !mk3 {
        answer.extend(printer.command("M114")?);
    }
    // Speed and flow as set now, also at its knob (PrusaLink reads them back every second):
    // "100%" (MK3), "FR:100%" and "Flow: 100%".
    let factors = (
        marlin::percent(&printer.command("M220")?),
        marlin::percent(&printer.command("M221")?),
    );
    // Prusa's MK3: its lifetime totals, when connected and after a print.
    if mk3 && known.totals_due {
        known.totals_due = false;
        known.totals = (
            marlin::eeprom_u32(&marlin::eeprom_bytes(&printer.command(EEPROM_TOTAL_FILAMENT)?)),
            marlin::eeprom_u32(&marlin::eeprom_bytes(&printer.command(EEPROM_TOTAL_TIME)?)),
        );
    }
    // Prusa's MK3: its sheet and print mode, now and then.
    if mk3 && known.eeprom_at.is_none_or(|t| t.elapsed() >= EEPROM_EVERY) {
        known.eeprom_at = Some(Instant::now());
        let sheets = marlin::eeprom_bytes(&printer.command(EEPROM_SHEETS)?);
        let active = marlin::eeprom_bytes(&printer.command(EEPROM_ACTIVE_SHEET)?);
        known.sheet = active.first().and_then(|a| marlin::active_sheet(&sheets, *a));
        known.print_mode = marlin::eeprom_bytes(&printer.command(EEPROM_PRINT_MODE)?)
            .first()
            .and_then(|m| marlin::print_mode(*m))
            .map(str::to_owned);
    }
    Ok((answer, factors))
}

/// What following an idle printer found besides its answer: a print from its SD card (with
/// the slicer's time left and filament change as its MK3 knows them, M73), a pause, resume or
/// stop of it not done in time, and its mesh read again after levelling.
#[derive(Debug, Default)]
struct Followed {
    sd: Option<marlin::SdStatus>,
    /// Its own percent done (`M73`), where it gives one.
    percent: Option<u32>,
    slicer_left: Option<u64>,
    change_in: Option<u64>,
    sd_problem: Option<String>,
    mesh: Option<protocol::Mesh>,
}

/// How far a print from its card is, and its time left, as PrusaLink takes them
/// (`printer_polling.py`): its own percent (`M73`) where it gives one, else the whole percent
/// of the bytes done (`_get_progress_from_byte_position`); its own time left, else guessed
/// from how long it has printed (`_guess_time_remaining`: printed × (100 − p) / p). As done
/// of total: percent of 100.
fn sd_progress(sd: &marlin::SdStatus, percent: Option<u32>, own_left: Option<u64>) -> (u64, u64, Option<u64>) {
    let progress = percent.unwrap_or_else(|| (sd.done * 100 / sd.total.max(1)) as u32).min(100);
    let remaining = own_left.or_else(|| {
        let printed = sd.elapsed_s?;
        (progress > 0).then(|| (printed as f64 * f64::from(100 - progress) / f64::from(progress)) as u64)
    });
    (u64::from(progress), 100, remaining)
}

/// After the look at an idle printer: a print from its own SD card followed (M27; on Prusa's
/// MK3 its own time left, M73, as PrusaLink asks it); the totals due again when it ends; an
/// MK2.5's mesh marked stale as one starts or ends; the mesh read again once levelled (M420 on
/// the MK3, as PrusaLink does). Apart from the loop, so a test can run it.
fn follow_idle(printer: &mut marlin::Printer<Link>, known: &mut Known) -> std::io::Result<Followed> {
    let waiting = printer.waiting();
    let mk3 = printer.is_prusa_mk3();
    let mut followed = Followed::default();
    // Its card's print: done (`Done printing file`: its last report, whole), or as `M27 P`
    // says; started at its menu (`echo:enqueing "M24"`) and not in `M27 P` yet: printing
    // all the same, as PrusaLink takes both at once.
    let sd = match printer.sd_finished() {
        Some(last) => Some(last),
        None if printer.has_sd() => {
            let sd = printer.follow_sd()?;
            let screen = waiting.now();
            sd.or_else(|| {
                (mk3 && screen.sd_started)
                    .then(|| marlin::SdStatus { file: screen.sd_opened.clone(), total: 1, ..Default::default() })
            })
        }
        None => None,
    };
    // A pause, resume or stop asked of its SD print and not done in 15 s.
    followed.sd_problem = printer.sd_not_done();
    if mk3 && sd.is_some() {
        let silent = known.print_mode.as_deref() == Some("silent");
        if let Some((percent, left, change)) = marlin::print_info_full(&printer.command("M73")?, silent) {
            followed.percent = Some(percent).filter(|p| *p <= 100);
            followed.slicer_left = left.map(|m| u64::from(m) * 60);
            followed.change_in = change.map(|m| u64::from(m) * 60);
        }
    }
    // An SD print ended: its totals changed. Started or ended: an MK2.5's mesh is read again.
    if known.sd_printing && sd.is_none() {
        known.totals_due = true;
        // Its progress and time left cleared, as PrusaLink clears them when a print ends
        // (`_reset_print_stats`).
        if mk3 {
            printer.command("M73 Q-1 S-1 C-1 P-1 R-1 D-1")?;
        }
    }
    if known.sd_printing != sd.is_some() && is_mk25(known) {
        waiting.mesh_changed();
    }
    known.sd_printing = sd.is_some();
    // Its bed measured anew (G80, or the homing before it): its mesh read again once it is
    // idle (M420 on the MK3, as PrusaLink does).
    if sd.is_none() && waiting.take_mesh_stale() {
        let ask = if mk3 { "M420" } else { "M420 V" };
        followed.mesh = marlin::mesh(&printer.command(ask)?);
    }
    // A power cut: the line it saved, read once, as PrusaLink reads it (`PPRecovery`).
    if !waiting.now().power_loss {
        known.saved_line = None;
    } else if mk3 && known.saved_line.is_none() {
        known.saved_line = marlin::eeprom_u32(&marlin::eeprom_bytes(&printer.command(EEPROM_FILE_POSITION)?));
    }
    followed.sd = sd;
    Ok(followed)
}

/// A print a power cut stopped that cannot be taken up (no copy of it kept, or the line it
/// saved not read from its EEPROM): given up as PrusaLink gives it up (`PPRecovery`: `M603`
/// and `M117 ~Recovery failed`, both to the front of its queue, so `M603` goes first).
/// Returns whether it was.
fn recovery_failed(printer: &mut marlin::Printer<Link>, known: &Known, kept: bool) -> std::io::Result<bool> {
    let waiting = printer.waiting();
    if !printer.is_prusa_mk3() || !waiting.now().power_loss || (kept && known.saved_line.is_some()) {
        return Ok(false);
    }
    printer.command("M603")?;
    printer.command("M117 ~Recovery failed")?;
    waiting.clear_power_loss();
    Ok(true)
}

/// The line a print taken up after a power cut goes on from: the one after the line it saved
/// (its EEPROM, as PrusaLink takes it), which it asks for; else what it asked for.
fn resume_from(saved: Option<u32>, asked: u32) -> u32 {
    match saved {
        Some(n) if n + 1 != asked => {
            println!("power cut: it saved line {n} but asks for {asked}: going on from {}", n + 1);
            n + 1
        }
        Some(n) => n + 1,
        None => asked,
    }
}

/// The printer on the serial port, connected, with what it says it is, or None. `M115`
/// gives its firmware and machine type; a Prusa answers `M862.2 Q` with its model code
/// (other firmware says it does not know the command, which does no harm).
fn open_printer(
    options: &Options,
    waiting: &Arc<marlin::Screen>,
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
    let found = match printer.connect_as_prusalink(prusa_port(port), &mut |_| {}) {
        Ok(found) => found,
        Err(error) => {
            println!("printer on {port} does not answer: {error}");
            return None;
        }
    };
    let mk3 = matches!(found, (Some(_), Some(code)) if mk3_family_type(code).is_some());
    let address = agent_info(&options.hub).address;
    let known = get_to_know(&mut printer, address, mk3);
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
    // An update under way: this build on trial, or one to roll back.
    let exe = std::env::current_exe().ok();
    // How to start again after an update: as now, without asking for a new code again.
    let again: Vec<std::ffi::OsString> = std::env::args_os()
        .skip(1)
        .filter(|a| a != "--new-code")
        .collect();
    let mut trial = None;
    if let Some(exe) = &exe {
        match update::on_start(exe) {
            update::Start::Plain => {}
            update::Start::OnTrial(t) => {
                println!(
                    "updated from {} to {}: on trial until it reaches the hub",
                    t.from, t.to
                );
                trial = Some(t);
            }
            update::Start::RollBack(t, why) => {
                println!("the update to {} goes back to {}: {why}", t.to, t.from);
                match update::roll_back(exe, &t, &why) {
                    Ok(()) => update::restart(exe, &again),
                    Err(error) => println!("could not go back: {error}"),
                }
            }
        }
    }
    let mut hub = HubLink::new();
    // A first run with the hub away waits for it, rather than giving up.
    let mut registered = match kept(&options) {
        Some(registered) => registered,
        None => loop {
            match register(&options) {
                Ok(registered) => {
                    hub.reached();
                    break registered;
                }
                Err(error) => {
                    hub.lost(&error.to_string());
                    std::thread::sleep(REPORT_EVERY);
                }
            }
        },
    };
    println!(
        "{} reports to {} as machine {}",
        options.name, options.hub, registered.machine_id
    );
    let mut machine: Option<Machine> = None;
    // A Marlin job's checkpoint and G-code, kept for a power cut (crate::recovery).
    let recovery = crate::recovery::Recovery::beside(std::path::Path::new(&options.state));
    // PrusaLink's own lines on the screen of Prusa's MK3 family (crate::lcd), and how the
    // hub was reached last, as PrusaLink's conditions name it.
    let mut lcd = crate::lcd::Lcd::new(Instant::now());
    let mut hub_condition: Option<crate::lcd::Condition>;
    // How the last transfer (a job's file from the hub) ended, for its screen.
    let mut transfer_ok = false;
    let mut agent = agent_info(&options.hub);
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
    stop_on_sigterm();
    loop {
        // Stopped (systemd's SIGTERM): Prusa's MK3 says so on its screen, as PrusaLink does
        // when it stops (`LCDPrinter.stop`), then the agent ends.
        if STOPPING.load(Ordering::Relaxed) {
            match machine.as_mut() {
                Some(Machine::Marlin(printer, _)) if printer.is_prusa_mk3() => {
                    for line in lcd.stopped(Instant::now()) {
                        let _ = printer.command(&line);
                    }
                }
                Some(Machine::Printing(job, known)) if known.firmware.as_deref().is_some_and(|f| f.starts_with("Prusa-Firmware")) => {
                    job.inject(None, &lcd.stopped(Instant::now()).join("\n"));
                    std::thread::sleep(Duration::from_secs(1));
                }
                _ => {}
            }
            println!("stopped");
            std::process::exit(0);
        }
        let wants = options.port.is_some() || options.kind == MachineKind::Label;
        if machine.is_none() && wants && Instant::now() >= next_look {
            let waiting = Arc::new(marlin::Screen::default());
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
            if let Some(Machine::Printing(job, mut known)) = machine.take() {
                // A print ended: its totals changed (read again, as PrusaLink does); an
                // MK2.5's mesh too.
                known.totals_due = true;
                if is_mk25(&known) {
                    job.waiting.mesh_changed();
                }
                // A printer that stopped answering is looked for again.
                machine = (!lost).then(|| Machine::Marlin(Box::new(printer), known));
            }
        }
        match machine.as_mut() {
            Some(Machine::Printing(job, known)) => {
                // The hub's commands sent between the file's lines meanwhile.
                for (id, went) in job.injected() {
                    done.push(match went {
                        Ok(output) => CommandDone { id, ok: true, reason: None, output },
                        Err(reason) => CommandDone { id, ok: false, reason: Some(reason), output: Vec::new() },
                    });
                }
                let p = job.progress();
                for said in &p.said {
                    println!("< {said}");
                }
                telemetry.state = if p.paused {
                    MachineState::Paused
                } else {
                    MachineState::Busy
                };
                // What its screen says: a question, an error, paused at its knob, an M0 or a
                // filament change in the file waiting for someone.
                on_screen(&mut telemetry, &job.waiting);
                telemetry.firmware = known.firmware.clone();
                telemetry.identity = Some(known.identity.clone());
                set_heat(&mut telemetry, p.heat);
                telemetry.job_id = Some(job.id);
                screen_facts(&mut telemetry, &job.waiting, known, (p.speed_percent, p.flow_percent));
                if telemetry.problem.is_none() {
                    telemetry.problem = p.problem.clone();
                }
                telemetry.filament_change_s = p.filament_change_s;
                // What is left: the slicer's own time where the file tells it (its M73, in
                // the printer's mode); else from how long the lines so far took.
                let remaining_s = p.slicer_left_s.or_else(|| {
                    (p.done > 20).then(|| p.elapsed_s * (p.total - p.done) as u64 / p.done as u64)
                });
                telemetry.time_estimated = p.slicer_left_s.is_none() && remaining_s.is_some();
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
                let mut factors = (None, None);
                let answer = if waiting.now().calibrating {
                    // Calibrating its thermal model: nothing sent, only listened to until it
                    // says it is done.
                    printer.listen(Duration::from_secs(1))
                } else {
                    while_busy(&options, &registered.token, &last, &waiting, || {
                        let (answer, asked) = ask_idle(printer, known)?;
                        factors = asked;
                        known.speed = asked.0;
                        Ok::<_, std::io::Error>(answer)
                    })
                };
                let calibrating = waiting.now().calibrating;
                let mut followed = Followed::default();
                let answer = answer.and_then(|answer| {
                    if calibrating {
                        return Ok((answer, printer.sd.clone()));
                    }
                    followed = follow_idle(printer, known)?;
                    recovery_failed(printer, known, recovery.load().is_some())?;
                    Ok((answer, followed.sd.clone()))
                });
                let (percent, slicer_left, change_in, sd_problem, mesh_now) = (
                    followed.percent,
                    followed.slicer_left,
                    followed.change_in,
                    followed.sd_problem.clone(),
                    followed.mesh.take(),
                );
                match answer {
                    Ok((answer, sd)) => {
                        telemetry.state = MachineState::Idle;
                        telemetry.firmware = known.firmware.clone();
                        telemetry.identity = Some(known.identity.clone());
                        set_heat(&mut telemetry, marlin::heat(&answer));
                        screen_facts(&mut telemetry, &waiting, known, factors);
                        if telemetry.problem.is_none() {
                            telemetry.problem = sd_problem.clone();
                        }
                        if let Some(mesh) = mesh_now {
                            telemetry.mesh = Some(mesh);
                        }
                        if let Some(sd) = sd {
                            telemetry.state = if sd.paused {
                                MachineState::Paused
                            } else {
                                MachineState::Busy
                            };
                            let (done, total, remaining_s) = sd_progress(&sd, percent, slicer_left);
                            telemetry.filament_change_s = change_in;
                            telemetry.time_estimated = slicer_left.is_none() && remaining_s.is_some();
                            telemetry.job = Some(protocol::JobProgress {
                                file: sd.file.clone().unwrap_or_else(|| "SD card".to_owned()),
                                done,
                                total,
                                elapsed_s: sd.elapsed_s,
                                remaining_s,
                            });
                        }
                        // A print a power cut stopped, kept (crate::recovery): reported as its
                        // job, paused, until someone resumes it (here or at the printer) or
                        // stops it.
                        if waiting.now().power_loss
                            && let Some((cp, _)) = recovery.load()
                        {
                            telemetry.state = MachineState::Paused;
                            telemetry.job_id = Some(cp.job_id);
                            telemetry.job = Some(protocol::JobProgress {
                                file: cp.name.clone(),
                                done: cp.index as u64,
                                total: cp.total.max(1) as u64,
                                elapsed_s: None,
                                remaining_s: None,
                            });
                            telemetry.problem = Some(
                                "a power cut stopped it: resume it here or on the printer"
                                    .to_owned(),
                            );
                        }
                    }
                    Err(error) => {
                        println!("printer lost: {error}");
                        machine = None;
                    }
                }
                // Resumed after a power cut (here, or on its own screen): it heated and moved
                // back itself, set its line number to the last line it carried out, and asks
                // for the next one. The job goes on from the line of its file that goes with
                // it, numbered from there.
                let stray = match &mut machine {
                    Some(Machine::Marlin(printer, _)) => printer.take_stray_resend(),
                    _ => None,
                };
                let saved = match &machine {
                    Some(Machine::Marlin(_, known)) => known.saved_line,
                    _ => None,
                };
                if let Some(line) = stray.map(|asked| resume_from(saved, asked))
                    && let Some((cp, gcode)) = recovery.load()
                    && let Some(index) = cp.index_of(line)
                    && let Some(Machine::Marlin(mut printer, known)) = machine.take()
                {
                    println!(
                        "job {}: taken up after a power cut at line {} of {}",
                        cp.job_id,
                        index + 1,
                        cp.total
                    );
                    printer.waiting().clear_power_loss();
                    printer.set_next_number(line);
                    let job = PrintJob::start_with(
                        cp.job_id,
                        &cp.name,
                        gcode,
                        *printer,
                        crate::print::Begin {
                            recovery: Some(recovery.clone()),
                            from_index: index,
                            resumed: true,
                            silent: known.print_mode.as_deref() == Some("silent"),
                        },
                    );
                    machine = Some(Machine::Printing(Box::new(job), known));
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
        let sent = ureq::post(&format!("{}{}", options.hub, protocol::TELEMETRY))
            .set("authorization", &format!("Bearer {}", registered.token))
            .send_json(&telemetry);
        match sent {
            Ok(response) => {
                // An answer that does not read (a hub restarting behind a proxy): try again.
                let reply: TelemetryReply = match response.into_json() {
                    Ok(reply) => {
                        hub.reached();
                        hub_condition = None;
                        // A new build that reached the hub works: it stays.
                        if let (Some(exe), Some(t)) = (&exe, trial.take()) {
                            update::confirm(exe, &t);
                            println!("{} works: the update is done", t.to);
                            agent.last_update = update::last(exe).map(outcome);
                        }
                        reply
                    }
                    Err(error) => {
                        // Most likely a hub newer than this agent, with something to say
                        // that this agent does not know.
                        hub.lost(&format!(
                            "its answer did not read ({error}): is the hub newer than this agent? Get the agent again from the hub's Downloads page"
                        ));
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
                // An update, once nothing runs on the machine.
                if let (Some(order), Some(exe)) = (&reply.update, &exe)
                    && trial.is_none()
                    && may_update(&telemetry, matches!(machine, Some(Machine::Printing(..))))
                {
                    println!("the hub asks to update to {}", order.version);
                    match self_update(&options, &registered.token, exe, order) {
                        Ok(()) => {
                            println!("{} is in place: trying it", order.version);
                            // Its port free for the new build; this one waits meanwhile.
                            drop(machine.take());
                            match update::supervise(exe, &again) {
                                Ok(update::Supervised::Confirmed) => {
                                    println!("{} works: starting it for good", order.version);
                                }
                                Ok(update::Supervised::Failed(t, why)) => {
                                    println!("{} did not work ({why}): back to {}", t.to, t.from);
                                    if let Err(error) = update::roll_back(exe, &t, &why) {
                                        println!("could not go back: {error}");
                                    }
                                }
                                Err(error) => println!("could not try it: {error}"),
                            }
                            update::restart(exe, &again);
                        }
                        Err(why) => {
                            println!("not updated to {}: {why}", order.version);
                            update::note_failure(exe, crate::RELEASE, &order.version, &why);
                            agent.last_update = update::last(exe).map(outcome);
                        }
                    }
                }
                for command in &reply.commands {
                    // Printing: between the file's lines, as PrusaLink sends controls while it
                    // prints (speed, flow, temperatures; the hub offers only those then); how
                    // it went comes with a later report.
                    if let Some(Machine::Printing(job, _)) = machine.as_ref() {
                        job.inject(Some(command.id), &command.gcode);
                        continue;
                    }
                    let outcome = match machine.as_mut() {
                        Some(Machine::Marlin(printer, known)) => {
                            // Ready as PrusaLink's READY: told M72 S1, until M72 S0.
                            for line in command.gcode.lines().map(str::trim) {
                                match line {
                                    "M72 S1" => known.ready = true,
                                    "M72 S0" => known.ready = false,
                                    _ => {}
                                }
                            }
                            let waiting = printer.waiting();
                            let went = while_busy(&options, &registered.token, &busy, &waiting, || {
                                carry_out(printer, &command.gcode)
                            });
                            // A listing of its card: PrusaLink's menu kept, as on every listing.
                            if printer.is_prusa_mk3()
                                && command.gcode.trim() == "M20 LT"
                                && let Ok((said, _)) = &went
                            {
                                special_menu(printer, said);
                            }
                            went
                        }
                        Some(Machine::Grbl(laser)) => {
                            while_busy(&options, &registered.token, &busy, &Arc::default(), || {
                                laser.carry_out(&command.gcode)
                            })
                            .map(|said| (said, None))
                        }
                        _ => Err(refused(machine.as_ref()).unwrap_or_default()),
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
                        let address = agent.address.clone();
                        let wizard = (paired != Some(true)).then(|| format!("Code: {}", registered.pairing_code));
                        let fetched = fetch(&options, &registered.token, order, |percent| {
                            if let Some(machine) = machine.as_mut() {
                                let transfer = (Some((percent, crate::lcd::Direction::From)), false);
                                show_on_screen(&mut lcd, machine, &recovery, hub_condition, wizard.clone(), address.clone(), transfer);
                            }
                        });
                        transfer_ok = fetched.is_ok();
                        match fetched {
                            // The printer goes to the job only once its file is here.
                            Ok(gcode) => {
                                if let Some(Machine::Marlin(printer, known)) = machine.take() {
                                    // An MK2.5 levels as a print starts, saying nothing.
                                    if is_mk25(&known) {
                                        printer.waiting().mesh_changed();
                                    }
                                    // Kept for a power cut while it runs (crate::recovery).
                                    let job = PrintJob::start_with(
                                        order.id,
                                        &order.name,
                                        gcode,
                                        *printer,
                                        crate::print::Begin {
                                            recovery: Some(recovery.clone()),
                                            silent: known.print_mode.as_deref() == Some("silent"),
                                            ..Default::default()
                                        },
                                    );
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
                        Some(Machine::Grbl(laser)) => fetch(&options, &registered.token, order, |_| {})
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
                    // A print a power cut stopped, waiting (crate::recovery): resumed with M602
                    // (Prusa's MK3 heats and moves back, then asks for its next line), or
                    // stopped with M603 and forgotten.
                    (Some(control), Some(Machine::Marlin(printer, _)))
                        if printer.waiting().now().power_loss && recovery.load().is_some() =>
                    {
                        let cp = recovery.load().map(|(cp, _)| cp);
                        match control {
                            protocol::JobControl::Resume => {
                                if let Err(error) = printer.send_line("M602", &mut |_| {}) {
                                    println!("resuming after a power cut: {error}");
                                }
                            }
                            protocol::JobControl::Stop => {
                                let _ = printer.send_line("M603", &mut |_| {});
                                printer.waiting().clear_power_loss();
                                recovery.clear();
                                if let Some(cp) = cp {
                                    job_end = Some(protocol::JobEnd {
                                        id: cp.job_id,
                                        ok: false,
                                        stopped: true,
                                        reason: Some("stopped after a power cut".to_owned()),
                                        restart_line: None,
                                    });
                                }
                            }
                            protocol::JobControl::Pause => {}
                        }
                    }
                    // A print from its own SD card (no job of the hub's).
                    (Some(control), Some(Machine::Marlin(printer, _))) if printer.sd.is_some() => {
                        let what = match control {
                            protocol::JobControl::Pause => "pause",
                            protocol::JobControl::Resume => "resume",
                            protocol::JobControl::Stop => "stop",
                        };
                        if let Err(error) = printer.control_sd(what, &mut |_| {}) {
                            println!("the print from its SD card: {error}");
                        }
                    }
                    _ => {}
                }
                // A button of the question on its screen, pressed on the website (M876).
                if let Some(button) = reply.answer {
                    match machine.as_mut() {
                        Some(Machine::Printing(job, _)) => job.answer(button),
                        Some(Machine::Marlin(printer, _)) => {
                            *printer.answer.lock().unwrap() = Some(button);
                            if let Err(error) = printer.send_answer(&mut |_| {}) {
                                println!("answering the printer: {error}");
                            }
                        }
                        _ => {}
                    }
                }
                if paired != Some(reply.paired) {
                    match &reply.space {
                        Some(space) => println!("added to {space}: reporting"),
                        None => show_code(&registered),
                    }
                    paired = Some(reply.paired);
                }
            }
            // The hub no longer knows this agent (its machine was removed): start over.
            Err(ureq::Error::Status(401, _)) => {
                hub.reached();
                hub_condition = Some(crate::lcd::Condition::Token);
                println!("the hub does not know this machine any more: registering again");
                match register(&options) {
                    Ok(again) => {
                        registered = again;
                        paired = None;
                    }
                    Err(error) => hub.lost(&format!("registering again failed: {error}")),
                }
            }
            Err(error) => {
                // As the Connect SDK takes it: a 5xx or no connection HTTP, any other 4xx API.
                hub_condition = Some(match &error {
                    ureq::Error::Status(code, _) if *code < 500 => crate::lcd::Condition::Api,
                    _ => crate::lcd::Condition::Http,
                });
                hub.lost(&error.to_string());
            }
        }
        if let Some(machine) = machine.as_mut() {
            let address = agent.address.clone();
            let wizard = (paired != Some(true)).then(|| format!("Code: {}", registered.pairing_code));
            show_on_screen(&mut lcd, machine, &recovery, hub_condition, wizard, address, (None, transfer_ok));
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

/// PrusaLink's own lines on the screen of Prusa's MK3 family (crate::lcd): its situation as
/// PrusaLink sees it, and the lines due sent (between the file's lines while it prints).
/// The setup screen shows the code to add it with (PrusaLink's own is set up on its web
/// page, `Go: <ip>`; the agent is added on the hub).
fn show_on_screen(
    lcd: &mut crate::lcd::Lcd,
    machine: &mut Machine,
    recovery: &crate::recovery::Recovery,
    hub: Option<crate::lcd::Condition>,
    wizard: Option<String>,
    address: Option<String>,
    (transfer, transfer_finished): (Option<(f64, crate::lcd::Direction)>, bool),
) {
    use crate::lcd::{Condition, Situation};
    let now = Instant::now();
    let ip = address.filter(|a| a.parse::<std::net::Ipv4Addr>().is_ok());
    // PrusaLink's conditions: the hub's, no address (LAN), its firmware older than 3.14.0
    // (FW), no serial number (SN, not asked of an MK2.5), its job id not read (JOB_ID).
    let conditions = |known: &Known| {
        let mut all: Vec<Condition> = hub.into_iter().collect();
        if ip.is_none() {
            all.push(Condition::Lan);
        }
        let version = known.firmware.as_deref().and_then(|f| f.strip_prefix("Prusa-Firmware ")).map(|v| {
            v.split(['-', '+']).next().unwrap_or("").split('.').map(|n| n.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>()
        });
        if version.is_some_and(|v| v < vec![3, 14, 0]) {
            all.push(Condition::Fw);
        }
        if known.identity.serial.is_none() && !is_mk25(known) {
            all.push(Condition::Sn);
        }
        if !known.job_id_read {
            all.push(Condition::JobId);
        }
        all
    };
    match machine {
        Machine::Marlin(printer, known) if printer.is_prusa_mk3() => {
            let screen = printer.waiting();
            for _ in 0..screen.take_lcd_changed() {
                lcd.lcd_updated(now);
            }
            let sd = printer.sd.clone();
            if sd.is_some() {
                known.ready = false;
            }
            let situation = Situation {
                printing: sd.as_ref().map(|sd| sd.file.clone().unwrap_or_default()),
                state_printing: sd.as_ref().is_some_and(|sd| !sd.paused),
                state_after_printing: sd.as_ref().is_some_and(|sd| sd.paused),
                ready: known.ready,
                recovery_ready: screen.now().power_loss && recovery.load().is_some(),
                conditions: conditions(known),
                wizard,
                ip,
                speed: known.speed,
                transfer,
                transfer_finished,
                ..Situation::default()
            };
            for line in lcd.tick(&situation, now) {
                let _ = printer.command(&line);
            }
        }
        Machine::Printing(job, known) if known.firmware.as_deref().is_some_and(|f| f.starts_with("Prusa-Firmware")) => {
            known.ready = false;
            for _ in 0..job.waiting.take_lcd_changed() {
                lcd.lcd_updated(now);
            }
            let (paused, speed) = job.paused_and_speed();
            let situation = Situation {
                printing: Some(job.name.clone()),
                state_printing: !paused,
                state_after_printing: paused,
                conditions: conditions(known),
                wizard,
                ip,
                speed,
                transfer,
                transfer_finished,
                ..Situation::default()
            };
            let lines = lcd.tick(&situation, now);
            if !lines.is_empty() {
                job.inject(None, &lines.join("\n"));
            }
        }
        _ => {}
    }
}

/// Set when the agent is told to stop (SIGTERM, as systemd stops it).
static STOPPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// SIGTERM sets [`STOPPING`]; the loop says goodbye on the printer's screen and ends.
fn stop_on_sigterm() {
    #[cfg(unix)]
    {
        extern "C" fn on_term(_: libc::c_int) {
            STOPPING.store(true, Ordering::Relaxed);
        }
        // SAFETY: the handler only stores to an atomic, which is signal-safe.
        unsafe {
            libc::signal(libc::SIGTERM, on_term as extern "C" fn(libc::c_int) as libc::sighandler_t);
        }
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
    telemetry.hotend_fan_rpm = heat.hotend_fan_rpm;
    telemetry.print_fan_rpm = heat.print_fan_rpm;
    telemetry.position_mm = heat.position_mm;
}

/// Does `work` (a command the machine takes a while over), and meanwhile reports `busy` to
/// the hub every [`REPORT_EVERY`], so the machine stays online there. The hub gives a
/// busy machine no command and no job, so its answers carry nothing to act on.
/// Why a machine takes no G-code now, if it does not: a label printer never; a printer that
/// is printing not until the print ends (PrusaLink's `execute_gcode` refuses it while
/// printing, too); nothing connected.
fn refused(machine: Option<&Machine>) -> Option<String> {
    match machine {
        Some(Machine::Marlin(..) | Machine::Grbl(_)) => None,
        Some(Machine::Dymo(_) | Machine::LabelWriter(_)) => {
            Some("a label printer takes no G-code".to_owned())
        }
        // Printing: sent between the file's lines (`PrintJob::inject`).
        Some(Machine::Printing(..)) => None,
        None => Some("the machine is not connected".to_owned()),
    }
}

fn while_busy<T>(
    options: &Options,
    token: &str,
    busy: &Telemetry,
    waiting: &Arc<marlin::Screen>,
    work: impl FnOnce() -> T,
) -> T {
    let (done, wait) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                wait.recv_timeout(REPORT_EVERY)
            {
                let report = waiting_for_someone(busy, waiting);
                let _ = ureq::post(&format!("{}{}", options.hub, protocol::TELEMETRY))
                    .set("authorization", &format!("Bearer {token}"))
                    .send_json(&report);
            }
        });
        let result = work();
        drop(done);
        result
    })
}

/// A report as it is, with what is on the printer's screen ([`on_screen`]).
fn waiting_for_someone(report: &Telemetry, screen: &marlin::Screen) -> Telemetry {
    let mut report = report.clone();
    on_screen(&mut report, screen);
    report
}

/// What the printer's screen says, put in a report, as Prusa Connect's printers say it:
/// a fatal error (it stopped: restart it), a question and its buttons ("Filament runout:
/// Continue / Purge more"), waiting for its knob (an `M0`, a filament change), paused at
/// its knob.
fn on_screen(report: &mut Telemetry, screen: &marlin::Screen) {
    report.bed_clear |= screen.take_ready();
    report.not_ready |= screen.take_not_ready();
    let now = screen.now();
    if let Some(fault) = now.fault {
        report.state = MachineState::Error;
        report.problem = Some(fault);
    } else if let Some((question, buttons)) = now.asking {
        // Its buttons, to be pressed from the website (M876 S<index>).
        report.prompt = Some(protocol::Prompt {
            text: question.clone(),
            buttons: buttons.clone(),
        })
        .filter(|p| !p.buttons.is_empty());
        report.state = MachineState::Attention;
        report.problem = Some(if buttons.is_empty() {
            question
        } else {
            format!("{question} ({})", buttons.join(" / "))
        });
    } else if let Some(attention) = now.attention {
        // A failed bed levelling, a slow fan, its thermal model, locked: someone must come.
        report.state = MachineState::Attention;
        report.problem = Some(attention);
    } else if now.power_loss && report.state != MachineState::Busy && report.job_id.is_none() {
        report.state = MachineState::Attention;
        report.problem =
            Some("a print was interrupted by a power cut: resume it on the printer".to_owned());
    } else if screen.waiting.load(Ordering::Relaxed) {
        report.state = MachineState::Attention;
        report.problem = Some(
            now.message
                .unwrap_or_else(|| "waits for someone at the printer: press its knob".to_owned()),
        );
    } else if now.paused && report.state == MachineState::Busy {
        report.state = MachineState::Paused;
        report.problem = Some("paused at the printer".to_owned());
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
        let answer = printer.command(line).map_err(|e| e.to_string())?;
        // A file opened from its card (`M23`): only on `File opened` does it go on, as
        // PrusaLink loads one (`command_handlers.py` `_load_file`, `OPEN_RESULT_REGEX`).
        if line.starts_with("M23 ") && !answer.iter().any(|l| l.starts_with("File opened: ")) {
            return Err(format!("Wrong file name, or bad file. File name: {}", &line[4..]));
        }
        said.extend(answer);
    }
    let mesh = marlin::mesh(&said);
    Ok((said, mesh))
}

#[cfg(test)]
mod tests {
    /// `--new-code`: the registration kept is forgotten, and a new one asked; without it,
    /// the same hub keeps it.
    #[test]
    fn an_update_waits_only_for_work() {
        let report = |state, job_id| Telemetry {
            state,
            job_id,
            ..Telemetry::default()
        };
        assert!(super::may_update(&report(MachineState::Idle, None), false));
        assert!(
            super::may_update(&report(MachineState::Attention, None), false),
            "a DYMO without its cassette"
        );
        assert!(super::may_update(
            &report(MachineState::Disconnected, None),
            false
        ));
        assert!(!super::may_update(&report(MachineState::Busy, None), false));
        assert!(!super::may_update(
            &report(MachineState::Paused, None),
            false
        ));
        assert!(!super::may_update(
            &report(MachineState::Attention, Some(4)),
            false
        ));
        assert!(!super::may_update(&report(MachineState::Idle, None), true));
    }

    /// Proves the row `READY_REGEX` of docs/prusalink-audit.md: "Set Ready" in the printer's
    /// menu (`// action:ready`) makes the next report say the bed is clear, once; the hub
    /// takes that as "Print taken" (see `a_queue_waits_for_a_clear_bed` in the hub).
    #[test]
    fn set_ready_at_the_printer_is_reported_once() {
        let screen = marlin::Screen::default();
        let mut report = Telemetry::default();
        super::on_screen(&mut report, &screen);
        assert!(!report.bed_clear, "nothing said yet");
        screen.hear("// action:ready");
        super::on_screen(&mut report, &screen);
        assert!(report.bed_clear);
        let mut again = Telemetry::default();
        super::on_screen(&mut again, &screen);
        assert!(!again.bed_clear, "said once");
        // "Not ready" on its menu (PrusaLink: `fw_cancel_ready`): reported once too.
        screen.hear("// action:not_ready");
        let mut report = Telemetry::default();
        super::on_screen(&mut report, &screen);
        assert!(report.not_ready && !report.bed_clear);
        let mut again = Telemetry::default();
        super::on_screen(&mut again, &screen);
        assert!(!again.not_ready, "said once");
    }

    #[test]
    fn credentials_from_before_the_hub_are_kept() {
        let old = r#"{"cloud": "http://pi:8460", "registered": {"machine_id": "m1", "token": "t1", "pairing_code": "ABC123"}}"#;
        let kept: Credentials = serde_json::from_str(old).unwrap();
        assert_eq!(kept.hub, "http://pi:8460");
        assert_eq!(kept.registered.machine_id, "m1");
    }

    #[test]
    fn a_new_code_forgets_the_old_registration() {
        let dir = std::env::temp_dir().join(format!("mc-agent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("maker-connect-agent.json");
        std::fs::write(dir.join("none.toml"), "").unwrap();
        let credentials = super::Credentials {
            hub: "http://pi:8460".to_owned(),
            registered: protocol::Registered {
                machine_id: "m1".to_owned(),
                token: "t1".to_owned(),
                pairing_code: "ABC234".to_owned(),
            },
        };
        let write =
            || std::fs::write(&state, serde_json::to_string(&credentials).unwrap()).unwrap();
        let args = |more: &[&str]| {
            let mut a: Vec<String> = [
                "--hub",
                "http://pi:8460",
                "--name",
                "Dymo",
                "--config",
                dir.join("none.toml").to_str().unwrap(),
                "--state",
                state.to_str().unwrap(),
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            a.extend(more.iter().map(|s| s.to_string()));
            super::options(&a).unwrap()
        };
        write();
        assert_eq!(
            super::kept(&args(&[])).map(|r| r.token),
            Some("t1".to_owned()),
            "kept"
        );
        assert!(super::kept(&args(&["--new-code"])).is_none(), "asked anew");
        assert!(!state.exists(), "and forgotten");
        std::fs::remove_dir_all(&dir).ok();
    }

    use super::*;

    #[test]
    fn durations_read_well() {
        assert_eq!(how_long(Duration::from_secs(45)), "45 s");
        assert_eq!(how_long(Duration::from_secs(200)), "3 min 20 s");
        assert_eq!(how_long(Duration::from_secs(7500)), "2 h 5 min");
    }

    /// A serial port that is a fake Marlin, so the agent's own functions can be run on it.
    struct FakePort(crate::marlin::tests::FakeMarlin);

    impl std::io::Read for FakePort {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl std::io::Write for FakePort {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.flush()
        }
    }

    impl serialport::SerialPort for FakePort {
        fn name(&self) -> Option<String> {
            None
        }
        fn baud_rate(&self) -> serialport::Result<u32> {
            Ok(115_200)
        }
        fn data_bits(&self) -> serialport::Result<serialport::DataBits> {
            Ok(serialport::DataBits::Eight)
        }
        fn flow_control(&self) -> serialport::Result<serialport::FlowControl> {
            Ok(serialport::FlowControl::None)
        }
        fn parity(&self) -> serialport::Result<serialport::Parity> {
            Ok(serialport::Parity::None)
        }
        fn stop_bits(&self) -> serialport::Result<serialport::StopBits> {
            Ok(serialport::StopBits::One)
        }
        fn timeout(&self) -> Duration {
            Duration::from_millis(10)
        }
        fn set_baud_rate(&mut self, _: u32) -> serialport::Result<()> {
            Ok(())
        }
        fn set_data_bits(&mut self, _: serialport::DataBits) -> serialport::Result<()> {
            Ok(())
        }
        fn set_flow_control(&mut self, _: serialport::FlowControl) -> serialport::Result<()> {
            Ok(())
        }
        fn set_parity(&mut self, _: serialport::Parity) -> serialport::Result<()> {
            Ok(())
        }
        fn set_stop_bits(&mut self, _: serialport::StopBits) -> serialport::Result<()> {
            Ok(())
        }
        fn set_timeout(&mut self, _: Duration) -> serialport::Result<()> {
            Ok(())
        }
        fn write_request_to_send(&mut self, _: bool) -> serialport::Result<()> {
            Ok(())
        }
        fn write_data_terminal_ready(&mut self, _: bool) -> serialport::Result<()> {
            Ok(())
        }
        fn read_clear_to_send(&mut self) -> serialport::Result<bool> {
            Ok(false)
        }
        fn read_data_set_ready(&mut self) -> serialport::Result<bool> {
            Ok(false)
        }
        fn read_ring_indicator(&mut self) -> serialport::Result<bool> {
            Ok(false)
        }
        fn read_carrier_detect(&mut self) -> serialport::Result<bool> {
            Ok(false)
        }
        fn bytes_to_read(&self) -> serialport::Result<u32> {
            Ok(0)
        }
        fn bytes_to_write(&self) -> serialport::Result<u32> {
            Ok(0)
        }
        fn clear(&self, _: serialport::ClearBuffer) -> serialport::Result<()> {
            Ok(())
        }
        fn try_clone(&self) -> serialport::Result<Box<dyn serialport::SerialPort>> {
            Err(serialport::Error::new(serialport::ErrorKind::Unknown, "a fake"))
        }
        fn set_break(&self) -> serialport::Result<()> {
            Ok(())
        }
        fn clear_break(&self) -> serialport::Result<()> {
            Ok(())
        }
    }

    /// Prusa's MK3 gets `M155 S2 C7` from `start_reports` (when it connects, and again when
    /// a report has been silent for the 60 s of `REPORTS_SILENT`), and counts as reporting
    /// by itself from then on until that time has passed.
    /// Proves the row `M155 S2 C7` of docs/prusalink-audit.md (the wiring in the main loop
    /// that calls it is not run here).
    #[test]
    fn an_mk3_is_given_its_reports_and_asked_again_when_silent() {
        let accepted: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let link: Link = Box::new(FakePort(crate::marlin::tests::FakeMarlin::new(accepted.clone())));
        let mut printer = marlin::Printer::new(link);
        printer.hurry();
        printer.connect(&mut |_| {}).unwrap();
        printer.command("M115").unwrap();
        assert!(printer.is_prusa_mk3());
        let screen = printer.waiting();
        assert_eq!(REPORTS_SILENT, Duration::from_secs(60));
        assert!(!screen.reports_by_itself(REPORTS_SILENT), "nothing heard yet: M155 is due");
        start_reports(&mut printer);
        assert!(accepted.lock().unwrap().contains(&"M155 S2 C7".to_owned()));
        assert!(screen.reports_by_itself(REPORTS_SILENT), "given its time to report");
        assert!(
            !screen.reports_by_itself(Duration::ZERO),
            "silent for longer than allowed: M155 S2 C7 is due again"
        );
    }

    /// A connected fake printer saying `m115`, with the lines it answers after some codes.
    fn fake_printer(
        m115: &str,
        says: &[(&str, &str)],
    ) -> (marlin::Printer<Link>, Arc<std::sync::Mutex<Vec<String>>>) {
        let accepted: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let mut fake = crate::marlin::tests::FakeMarlin::new(accepted.clone());
        fake.m115 = m115.to_owned();
        fake.errors = says.iter().map(|(c, l)| ((*c).to_owned(), (*l).to_owned())).collect();
        let link: Link = Box::new(FakePort(fake));
        let mut printer = marlin::Printer::new(link);
        printer.hurry();
        printer.connect(&mut |_| {}).unwrap();
        (printer, accepted)
    }

    /// While it prints, a command from the hub goes between the file's lines (PrusaLink
    /// sends its controls while printing; the hub refuses typed G-code then, as PrusaLink's
    /// `execute_gcode`). A connected printer takes it; nothing connected, no.
    #[test]
    fn g_code_while_printing_goes_between_the_files_lines() {
        let (mut printer, accepted) = fake_printer(MK3_M115, &[]);
        let known = get_to_know(&mut printer, None, true);
        let idle = Machine::Marlin(Box::new(printer), known);
        assert_eq!(refused(Some(&idle)), None, "idle: it takes G-code");
        let Machine::Marlin(printer, known) = idle else { unreachable!() };
        let gcode: String = (0..2000).map(|i| format!("G1 X{} Y1\n", i % 100)).collect();
        let job = PrintJob::start(1, "cube.gcode", gcode, *printer);
        let printing = Machine::Printing(Box::new(job), known);
        assert_eq!(refused(Some(&printing)), None, "printing: sent by the job");
        let Machine::Printing(job, _) = printing else { unreachable!() };
        job.inject(Some(7), "M220 S120");
        let mut went = Vec::new();
        for _ in 0..400 {
            went.extend(job.injected());
            if !went.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(went.as_slice(), [(7, Ok(_))]), "{went:?}");
        assert!(sent(&accepted).contains(&"M220 S120".to_owned()));
        job.stop();
        assert!(refused(None).is_some(), "nothing connected");
    }

    /// After a power cut, Prusa's MK3 is asked the line it saved (`D3 Ax0F91 C4`, PrusaLink's
    /// `EEPROM_FILE_POSITION`), once while it waits; the print goes on from the line after it.
    /// Proves the row `EEPROM_FILE_POSITION` of docs/prusalink-audit.md.
    #[test]
    fn after_a_power_cut_the_saved_line_is_read_from_its_eeprom() {
        let (mut printer, accepted) = fake_printer(MK3_M115, &[("D3 AxF91 C4", "0F91  2A 00 00 00")]);
        let mut known = get_to_know(&mut printer, None, true);
        follow_idle(&mut printer, &mut known).unwrap();
        assert_eq!(known.saved_line, None, "no power cut: not read");
        printer.waiting().hear("// action:uvlo_recovery_ready");
        follow_idle(&mut printer, &mut known).unwrap();
        follow_idle(&mut printer, &mut known).unwrap();
        assert_eq!(known.saved_line, Some(42));
        assert_eq!(count(&accepted, "D3 AxF91 C4"), 1, "read once");
        assert_eq!(resume_from(known.saved_line, 43), 43);
        assert_eq!(resume_from(known.saved_line, 40), 43, "its EEPROM, as PrusaLink takes it");
        assert_eq!(resume_from(None, 40), 40);
        printer.waiting().clear_power_loss();
        follow_idle(&mut printer, &mut known).unwrap();
        assert_eq!(known.saved_line, None, "resumed: forgotten");
    }

    /// A file from its card opened (`M23`) goes on to `M24` only on `File opened`, as
    /// PrusaLink loads one (`_load_file`, `OPEN_RESULT_REGEX`); `open failed`: stopped there.
    /// Proves the row `M23 path` + `M24` of docs/prusalink-audit.md.
    #[test]
    fn a_card_file_is_printed_only_once_opened() {
        let (mut printer, accepted) = fake_printer(MK3_M115, &[("M23 /benchy~1.gco", "File opened: BENCHY~1.GCO Size: 812345")]);
        assert!(carry_out(&mut printer, "M23 /benchy~1.gco\nM24").is_ok());
        assert!(sent(&accepted).ends_with(&["M23 /benchy~1.gco".to_owned(), "M24".to_owned()]));
        let (mut printer, accepted) = fake_printer(MK3_M115, &[("M23 /nope.gco", "open failed, File: nope.gco.")]);
        let why = carry_out(&mut printer, "M23 /nope.gco\nM24").unwrap_err();
        assert!(why.contains("Wrong file name"), "{why}");
        assert!(!sent(&accepted).contains(&"M24".to_owned()), "not started");
    }

    /// An SD print's progress and time left as PrusaLink takes them: its own percent, else
    /// the whole percent of the bytes; its own time, else printed × (100 − p) / p.
    /// Proves the row `progress_from_bytes`, … of docs/prusalink-audit.md.
    #[test]
    fn an_sd_print_goes_as_far_as_prusalink_says() {
        let sd = marlin::SdStatus { done: 333, total: 1000, elapsed_s: Some(600), ..Default::default() };
        assert_eq!(sd_progress(&sd, None, None), (33, 100, Some(1218)), "33 % of the bytes; 600 s × 67 / 33");
        assert_eq!(sd_progress(&sd, Some(50), None), (50, 100, Some(600)), "its own percent first");
        assert_eq!(sd_progress(&sd, Some(50), Some(900)), (50, 100, Some(900)), "its own time first");
        let start = marlin::SdStatus { done: 5, total: 1000, elapsed_s: Some(10), ..Default::default() };
        assert_eq!(sd_progress(&start, None, None), (0, 100, None), "0 %: no guess");
    }

    /// Everything Prusa's MK3 is sent while connecting, looked at idle, its card followed, a
    /// power cut noticed, is one of PrusaLink's own G-codes in its form (the agent must not be
    /// told from PrusaLink). Proves section 6 of docs/prusalink-audit.md for these paths.
    #[test]
    fn an_mk3_is_sent_only_what_prusalink_sends() {
        let says = [("M862.2 Q", "30302"), ("D3 AxF91 C4", "0F91  2A 00 00 00"), ("M20 LT", "DIR_ENTER: /PRUSAL~1/ \"PrusaLink menu\""), ("M20 LT", "DIR_EXIT")];
        let (mut printer, accepted) = fake_printer(MK3_M115, &says);
        let mut known = get_to_know(&mut printer, Some("192.168.1.50".to_owned()), true);
        printer.waiting().hear("T:24.5 /0.0 B:23.1 /0.0 T0:24.5 /0.0 @:0 B@:0 P:0.0 A:28.2");
        for _ in 0..2 {
            ask_idle(&mut printer, &mut known).unwrap();
            follow_idle(&mut printer, &mut known).unwrap();
        }
        printer.waiting().hear("G80 done");
        printer.waiting().hear("// action:uvlo_recovery_ready");
        follow_idle(&mut printer, &mut known).unwrap();
        carry_out(&mut printer, "M23 /benchy~1.gco\nM24").ok();
        // PrusaLink's own lines on its screen, idle and set up, and with a print running.
        let mut lcd = crate::lcd::Lcd::new(Instant::now() - Duration::from_secs(60));
        let recovery = crate::recovery::Recovery::beside(&std::env::temp_dir().join("mc-lcd-test.json"));
        let mut machine = Machine::Marlin(Box::new(printer), known);
        show_on_screen(&mut lcd, &mut machine, &recovery, None, Some("Code: WPMFSS".to_owned()), Some("192.168.1.50".to_owned()), (None, false));
        show_on_screen(&mut lcd, &mut machine, &recovery, Some(crate::lcd::Condition::Http), None, Some("192.168.1.50".to_owned()), (Some((40.0, crate::lcd::Direction::From)), false));
        let all = sent(&accepted);
        assert!(all.iter().any(|c| c == "M117 ~PrusaLink started"), "{all:?}");
        assert!(all.iter().any(|c| c.starts_with("M117 ~Code: WPMFSS")), "{all:?}");
        assert!(all.len() > 20, "{all:?}");
        for code in &all {
            assert!(crate::marlin::tests::prusalink_sends(code), "not PrusaLink's: {code} in {all:?}");
        }
        // None of what other firmware is sent (section 6 of the audit).
        for code in ["M115", "M105", "M114", "M27 C", "M524", "M108", "M876", "G29", "M420 V", "M155 S2", "M20 L T"] {
            let same = |c: &String| c == code || (!code.contains(' ') && c.starts_with(&format!("{code} ")));
            assert!(!all.iter().any(same), "{code} sent: {all:?}");
        }
        // And the matcher refuses what is not PrusaLink's.
        for code in ["M115", "M105", "M114", "M27", "M79 S\"MC\"", "M20 L T", "M18", "M107"] {
            assert!(!crate::marlin::tests::prusalink_sends(code), "{code}");
        }
    }

    /// A card with PrusaLink's menu and no `setready.g` in it gets one when it connects, as
    /// PrusaLink writes it: `M28 prusal~1/setready.g`, `M84`, `M29`. With it there: nothing.
    /// Proves the rows `M28`, `M29`, `handle_special_menu` of docs/prusalink-audit.md.
    #[test]
    fn prusalinks_menu_gets_its_setready_file() {
        let says = [("M20 LT", "DIR_ENTER: /PRUSAL~1/ \"PrusaLink menu\""), ("M20 LT", "DIR_EXIT")];
        let (mut printer, accepted) = fake_printer(MK3_M115, &says);
        get_to_know(&mut printer, None, true);
        let all = sent(&accepted);
        let at = all.iter().position(|c| c == "M28 prusal~1/setready.g").expect("written");
        assert_eq!(all[at..at + 3], ["M28 prusal~1/setready.g", "M84", "M29"]);
        assert_eq!(printer.waiting().now().menu_sfn.as_deref(), Some("prusal~1"));
        let says = [
            ("M20 LT", "DIR_ENTER: /PRUSAL~1/ \"PrusaLink menu\""),
            ("M20 LT", "/PRUSAL~1/SETREADY.G 4 \"setready.g\""),
            ("M20 LT", "DIR_EXIT"),
        ];
        let (mut printer, accepted) = fake_printer(MK3_M115, &says);
        get_to_know(&mut printer, None, true);
        assert!(!sent(&accepted).iter().any(|c| c.starts_with("M28")), "there already");
    }

    /// A power cut with nothing to take up (no copy kept, or its saved line not read): given
    /// up as PrusaLink does, `M603` then `M117 ~Recovery failed`; with both, it waits.
    /// Proves the rows `M117 ~text` and `pp_recovery` (its failure) of docs/prusalink-audit.md.
    #[test]
    fn a_power_cut_that_cannot_be_taken_up_is_given_up() {
        let (mut printer, accepted) = fake_printer(MK3_M115, &[("D3 AxF91 C4", "0F91  2A 00 00 00")]);
        let mut known = get_to_know(&mut printer, None, true);
        printer.waiting().hear("// action:uvlo_recovery_ready");
        follow_idle(&mut printer, &mut known).unwrap();
        assert!(!recovery_failed(&mut printer, &known, true).unwrap(), "kept and read: it waits");
        assert!(recovery_failed(&mut printer, &known, false).unwrap());
        assert!(sent(&accepted).ends_with(&["M603".to_owned(), "M117 ~Recovery failed".to_owned()]));
        assert!(!printer.waiting().now().power_loss);
    }

    const MK3_M115: &str = "FIRMWARE_NAME:Prusa-Firmware 3.14.1 based on Marlin FIRMWARE_URL:https://github.com/prusa3d/Prusa-Firmware PROTOCOL_VERSION:1.0 MACHINE_TYPE:Prusa i3 MK3S EXTRUDER_COUNT:1";
    const MARLIN_M115: &str = "FIRMWARE_NAME:Marlin 2.1.2 (Github) SOURCE_CODE_URL:https://github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0 MACHINE_TYPE:Ender EXTRUDER_COUNT:1 Cap:SDCARD:1";

    fn sent(accepted: &Arc<std::sync::Mutex<Vec<String>>>) -> Vec<String> {
        accepted.lock().unwrap().clone()
    }

    fn count(accepted: &Arc<std::sync::Mutex<Vec<String>>>, code: &str) -> usize {
        sent(accepted).iter().filter(|c| *c == code).count()
    }

    /// An MK3's EEPROM sheets as `D3` prints them: eight of 11 bytes, "Smooth1" and the like.
    fn sheet_lines() -> Vec<(&'static str, String)> {
        let mut bytes = Vec::new();
        for name in ["Smooth1", "Textur2", "Satin_3", "Smooth4", "Smooth5", "Smooth6", "Smooth7", "Smooth8"] {
            bytes.extend(name.bytes());
            bytes.extend([0, 0, 215, 60]);
        }
        bytes
            .chunks(16)
            .enumerate()
            .map(|(i, chunk)| {
                let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
                ("D3 AxD49 C88", format!("{:04X}  {}", 0x0D49 + i * 16, hex.join(" ")))
            })
            .collect()
    }

    /// Prusa's MK3, when it has answered, is asked `M862.2 Q`, `PRUSA SN`, `M862.1 Q`,
    /// `M707 A0x00`..`A0x03`, `M21` and `M552 P<ip>` (the agent's IPv4 only), and what it says
    /// is read into what is known: model 302, serial, nozzle, MMU firmware (none without an MMU).
    /// Proves the rows `PRUSA SN`, `M862.2 Q`, `M862.1 Q`, `M707 A0x00…0x03`, `M21`, `M552 P<ip>`,
    /// `M707`, `M862.1`, `M862.2`, `printer_type`, `nozzle_diameter`, `serial_number`,
    /// `mmu_connected` and `mmu_version` of docs/prusalink-audit.md.
    #[test]
    fn the_mk3_is_asked_what_it_is_when_connected() {
        let says = [
            ("M862.2 Q", "30302"),
            ("PRUSA SN", "CZPX2918X004XK74799"),
            ("M862.1 Q", "0.40"),
            ("M707 A0x00", "echo:MMU2:<R0 A3*2c."),
            ("M707 A0x01", "echo:MMU2:<R1 A0*2d."),
            ("M707 A0x02", "echo:MMU2:<R2 A3*2e."),
            ("M707 A0x03", "echo:MMU2:<R3 A380*2f."),
        ];
        // On a port PrusaLink takes for a Prusa: exactly what PrusaLink asks, in its order
        // (printer_polling.py `printer_info`), and never M115.
        let (mut printer, accepted) = fake_printer(MK3_M115, &says);
        let known = get_to_know(&mut printer, Some("192.168.1.50".to_owned()), true);
        assert_eq!(
            sent(&accepted),
            [
                "M862.2 Q", "PRUSA Fir", "M862.1 Q", "PRUSA SN", "M707 A0x00", "M707 A0x01",
                "M707 A0x02", "M707 A0x03", "D3 AxD05 C4", "M20 LT", "M552 P192.168.1.50",
                "M155 S2 C7",
            ]
        );
        assert_eq!(known.identity.model_code, Some(302));
        assert_eq!(known.identity.serial.as_deref(), Some("CZPX2918X004XK74799"));
        assert_eq!(known.identity.nozzle_mm.as_deref(), Some("0.40"));
        assert_eq!(known.identity.mmu_firmware.as_deref(), Some("3.0.3+896"));
        assert_eq!(known.firmware.as_deref(), Some("Prusa-Firmware 3.14.1-8237"));
        assert!(printer.is_prusa_mk3());

        // No MMU3 (302): no M707. No IP address: M552 P0.0.0.0.
        let (mut printer, accepted) = fake_printer(MK3_M115, &[]);
        let known = get_to_know(&mut printer, None, true);
        assert_eq!(
            sent(&accepted),
            ["M862.2 Q", "PRUSA Fir", "M862.1 Q", "PRUSA SN", "D3 AxD05 C4", "M20 LT", "M552 P0.0.0.0", "M155 S2 C7"]
        );
        assert_eq!(known.identity.mmu_firmware, None);
        assert_eq!(known.identity.serial, None);
        assert_eq!(known.identity.nozzle_mm, None);

        // An address that is not an IPv4: none, as PrusaLink tells it (ip_updater.py).
        let (mut printer, accepted) = fake_printer(MK3_M115, &[]);
        get_to_know(&mut printer, Some("printer.local".to_owned()), true);
        assert_eq!(count(&accepted, "M552 P0.0.0.0"), 1);
    }

    /// Plain Marlin is asked `M115` only: none of Prusa's questions (`M862` is something else
    /// there), no `M21`, no `M552`, no EEPROM reads, no reports set up; it is asked its position
    /// (`M114`) and, as any, its speed and flow.
    /// Proves the rows `PRUSA SN`, `M862.2 Q`, `M862.1 Q`, `M21`, `M552 P<ip>` and `M707`
    /// (MK3 only) of docs/prusalink-audit.md.
    #[test]
    fn plain_marlin_is_not_asked_prusas_questions() {
        let (mut printer, accepted) = fake_printer(MARLIN_M115, &[]);
        let mut known = get_to_know(&mut printer, Some("192.168.1.50".to_owned()), false);
        assert_eq!(known.identity.model_code, None);
        assert_eq!(known.identity.serial, None);
        ask_idle(&mut printer, &mut known).unwrap();
        let all = sent(&accepted);
        assert_eq!(all[0], "M115", "{all:?}");
        for code in all.iter() {
            assert!(
                !(code.starts_with("PRUSA") || code.starts_with("M862") || code.starts_with("M707")
                    || code.starts_with("M21") || code.starts_with("M552") || code.starts_with("D3")
                    || code.starts_with("M155") || code.starts_with("M79")),
                "{code} is for Prusa's MK3 only"
            );
        }
        assert!(all.contains(&"M114".to_owned()), "where it is: {all:?}");
        assert!(all.contains(&"M220".to_owned()) && all.contains(&"M221".to_owned()));
    }

    /// One look at an idle MK3: `M220` and `M221` without a value answered `100%` and `90%` are
    /// read as the speed and flow; no `M114` (it reports its position); the totals (`D3 Ax0FF1 C4`,
    /// `D3 Ax0FED C4`) once; the sheets, active sheet and print mode (`D3 Ax0D49 C88`,
    /// `D3 Ax0DA1 C1`, `D3 Ax0FFF C1`) once within 10 s; and what they say is read into what is known.
    /// Proves the rows `M220` / `M221` (no S), `D3`, `speed_multiplier`, `flow_multiplier`,
    /// `sheet_settings`, `active_sheet`, `total_filament`, `total_print_time`, `print_mode`,
    /// `SHEET_SETTINGS`, `ACTIVE_SHEET`, `PRINT_MODE`, `TOTAL_FILAMENT` and `TOTAL_PRINT_TIME`
    /// of docs/prusalink-audit.md.
    #[test]
    fn an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet() {
        let mut says: Vec<(&str, &str)> = vec![
            ("M220", "100%"),
            ("M221", "90%"),
            // 42 m of filament, 10000 minutes, as little-endian numbers.
            ("D3 AxFF1 C4", "0FF1  2A 00 00 00"),
            ("D3 AxFED C4", "0FED  10 27 00 00"),
            ("D3 AxDA1 C1", "0DA1  01"),
            ("D3 AxFFF C1", "0FFF  01"),
        ];
        let sheets = sheet_lines();
        for (code, line) in &sheets {
            says.push((code, line));
        }
        let (mut printer, accepted) = fake_printer(MK3_M115, &says);
        let mut known = get_to_know(&mut printer, None, true);
        assert!(known.totals_due && known.totals == (None, None) && known.sheet.is_none());
        // Its temperatures as it reported them by itself (M155 S2 C7): it is asked none.
        printer.waiting().hear("T:24.5 /0.0 B:23.1 /0.0 T0:24.5 /0.0 @:0 B@:0 P:0.0 A:28.2");
        let (answer, factors) = ask_idle(&mut printer, &mut known).unwrap();
        assert!(answer.iter().any(|l| l.contains("T:24.5")), "its temperatures: {answer:?} {:?}", printer.waiting().now().reported);
        assert_eq!(count(&accepted, "M105"), 0, "PrusaLink asks no M105");
        assert_eq!(factors, (Some(100.0), Some(90.0)));
        assert_eq!(known.totals, (Some(42), Some(10_000)));
        assert!(!known.totals_due);
        assert_eq!(known.sheet.as_deref(), Some("Textur2"));
        assert_eq!(known.print_mode.as_deref(), Some("silent"));
        for code in ["M220", "M221", "D3 AxFF1 C4", "D3 AxFED C4", "D3 AxD49 C88", "D3 AxDA1 C1", "D3 AxFFF C1"] {
            assert_eq!(count(&accepted, code), 1, "{code}");
        }
        assert_eq!(count(&accepted, "M114"), 0, "an MK3 reports its position by itself");
        // Looked at again at once: speed and flow again, the EEPROM not (totals not due, sheet not for 10 s).
        ask_idle(&mut printer, &mut known).unwrap();
        assert_eq!(count(&accepted, "M220"), 2);
        assert_eq!(count(&accepted, "M221"), 2);
        for code in ["D3 AxFF1 C4", "D3 AxFED C4", "D3 AxD49 C88", "D3 AxDA1 C1", "D3 AxFFF C1"] {
            assert_eq!(count(&accepted, code), 1, "{code} not again");
        }
        // After 10 s the sheet and print mode are read again; the totals only when due.
        known.eeprom_at = Some(Instant::now() - EEPROM_EVERY);
        ask_idle(&mut printer, &mut known).unwrap();
        assert_eq!(count(&accepted, "D3 AxD49 C88"), 2);
        assert_eq!(count(&accepted, "D3 AxFFF C1"), 2);
        assert_eq!(count(&accepted, "D3 AxFF1 C4"), 1);
    }

    /// An unwritten EEPROM (all `FF`) gives no totals, no sheet and no print mode.
    /// Proves the row `TOTAL_FILAMENT` of docs/prusalink-audit.md for the unwritten case.
    #[test]
    fn an_unwritten_mk3_eeprom_gives_no_totals() {
        let says = [
            ("D3 AxFF1 C4", "0FF1  FF FF FF FF"),
            ("D3 AxFFF C1", "0FFF  FF"),
        ];
        let (mut printer, _) = fake_printer(MK3_M115, &says);
        let mut known = get_to_know(&mut printer, None, true);
        ask_idle(&mut printer, &mut known).unwrap();
        assert_eq!(known.totals.0, None);
        assert_eq!(known.print_mode, None);
    }

    /// An SD print on an MK3 is followed with `M27` and its own progress is asked with `M73`
    /// (time left and filament change in seconds); when it ends the totals are due again, and
    /// `M73` is not asked of an idle printer.
    /// Proves the rows `M73` (no args), `print_state`, `print_progress`, `time_remaining`,
    /// `filament_change_in` and `total_filament` (read again after a print) of docs/prusalink-audit.md.
    #[test]
    fn an_mk3_sd_print_is_followed_with_m27_and_m73() {
        let says = [
            ("M27 P", "/MY_BEN~1.GCO"),
            ("M27 P", "SD printing byte 1000/2000"),
            ("M73", "NORMAL MODE: Percent done: 50; print time remaining in mins: 30; Change in mins: 10"),
            ("M73", "SILENT MODE: Percent done: 50; print time remaining in mins: 40; Change in mins: 12"),
        ];
        let (mut printer, accepted) = fake_printer(MK3_M115, &says);
        let mut known = get_to_know(&mut printer, None, true);
        known.totals_due = false;
        let followed = follow_idle(&mut printer, &mut known).unwrap();
        let sd = followed.sd.expect("printing from the card");
        assert_eq!((sd.done, sd.total), (1000, 2000));
        assert_eq!(followed.slicer_left, Some(30 * 60), "normal mode");
        assert_eq!(followed.change_in, Some(10 * 60));
        assert_eq!(count(&accepted, "M27 P"), 1);
        assert_eq!(count(&accepted, "M73"), 1);
        assert!(known.sd_printing && !known.totals_due);

        // In silent mode the silent line is read.
        known.print_mode = Some("silent".to_owned());
        let followed = follow_idle(&mut printer, &mut known).unwrap();
        assert_eq!(followed.slicer_left, Some(40 * 60));
        assert_eq!(followed.change_in, Some(12 * 60));

        // Not printing from the card: no M73, and an end makes the totals due.
        let (mut printer, accepted) = fake_printer(MK3_M115, &[("M27 P", "Not SD printing")]);
        let mut known = get_to_know(&mut printer, None, true);
        known.totals_due = false;
        known.sd_printing = true;
        let followed = follow_idle(&mut printer, &mut known).unwrap();
        assert!(followed.sd.is_none() && followed.slicer_left.is_none());
        assert!(known.totals_due, "the print ended: totals read again");
        assert!(!known.sd_printing);
        assert_eq!(count(&accepted, "M73"), 0);
        let mut again = known;
        again.eeprom_at = Some(Instant::now());
        ask_idle(&mut printer, &mut again).unwrap();
        assert_eq!(count(&accepted, "D3 AxFF1 C4"), 1, "asked after the print");
        assert_eq!(count(&accepted, "D3 AxFED C4"), 1);
        assert!(!again.totals_due);
    }

    const MESH_ROWS: [&str; 3] = [
        "  0.00000  0.00100  0.00200",
        " -0.00100  0.00000  0.00100",
        " -0.00200 -0.00100  0.00000",
    ];

    /// Once an MK3 has been seen levelling (`echo:enqueing "G80"`), an idle look reads the mesh
    /// with `M420` (not `M420 V`), once; without levelling seen it is not asked.
    /// Proves the rows `M420` and `mbl` of docs/prusalink-audit.md (MK3 part).
    #[test]
    fn an_mk3_that_levelled_has_its_mesh_read_with_m420() {
        let mut says = vec![("M420", "Num X,Y: 3,3"), ("M420", "Measured points:")];
        says.extend(MESH_ROWS.iter().map(|r| ("M420", *r)));
        let (mut printer, accepted) = fake_printer(MK3_M115, &says);
        let mut known = get_to_know(&mut printer, None, true);
        let followed = follow_idle(&mut printer, &mut known).unwrap();
        assert!(followed.mesh.is_none());
        assert_eq!(count(&accepted, "M420"), 0, "nothing levelled yet");
        printer.waiting().hear("echo:enqueing \"G80\"");
        let followed = follow_idle(&mut printer, &mut known).unwrap();
        let mesh = followed.mesh.expect("the mesh read");
        assert_eq!(mesh.rows.len(), 3);
        assert_eq!(mesh.rows[0].len(), 3);
        assert_eq!(count(&accepted, "M420"), 1);
        assert_eq!(count(&accepted, "M420 V"), 0);
        assert!(follow_idle(&mut printer, &mut known).unwrap().mesh.is_none());
        assert_eq!(count(&accepted, "M420"), 1, "once");
    }

    /// Other Marlin reads its mesh with `M420 V` after levelling; an MK2.5 (model 250) has it
    /// read when an SD print ends (it says nothing when it levels), an MK3S (302) does not.
    /// Proves the row `mbl` of docs/prusalink-audit.md for the MK2.5 and other Marlin.
    #[test]
    fn the_mesh_of_marlin_and_an_mk25_is_read_when_due() {
        let (mut printer, accepted) = fake_printer(MARLIN_M115, &[]);
        let mut known = get_to_know(&mut printer, None, false);
        printer.waiting().hear("echo:enqueing \"G80\"");
        follow_idle(&mut printer, &mut known).unwrap();
        assert_eq!(count(&accepted, "M420 V"), 1);
        assert_eq!(count(&accepted, "M420"), 0);

        // An MK2.5 whose SD print ended: the mesh is read again.
        let (mut printer, accepted) = fake_printer(MK3_M115, &[("M862.2 Q", "250"), ("M27", "Not SD printing")]);
        let mut known = get_to_know(&mut printer, None, true);
        assert_eq!(known.identity.model_code, Some(250));
        known.sd_printing = true;
        follow_idle(&mut printer, &mut known).unwrap();
        assert_eq!(count(&accepted, "M420"), 1, "an MK2.5 after an SD print");

        // An MK3S whose SD print ended: its mesh is not.
        let (mut printer, accepted) = fake_printer(MK3_M115, &[("M27", "Not SD printing")]);
        let mut known = get_to_know(&mut printer, None, true);
        known.sd_printing = true;
        follow_idle(&mut printer, &mut known).unwrap();
        assert_eq!(count(&accepted, "M420"), 0);
    }
}
