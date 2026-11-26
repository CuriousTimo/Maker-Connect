//! # Maker Connect agent
//!
//! The module next to a machine. Two devices in this step, each its own SDK printer with its own
//! settings file, registration and socket (`docs/design/wire.md`, "The unit: one printer per
//! device"): the DYMO LabelManager PnP (labelle's port, `maker_connect_engine::ports::labelle`;
//! "The first device") and the DYMO LabelWriter 450 (DYMO's CUPS driver,
//! `maker_connect_engine::ports::dymo_cups`; "The second device"). Each is linked to the hub over
//! the Prusa Connect printer protocol: registration with a code over `/p/register`, then `/p/ws`
//! with telemetry, events and commands (`maker_connect_sdk`). A label comes as
//! `START_CONNECT_DOWNLOAD` with `printing` and `tape_size_mm` or `page_size`: the SDK's download
//! (`maker_connect_sdk::download`) fetches it in its own thread, then the port prints it.
//!
//! A small page on the agent (`--listen`, default `0.0.0.0:8470`): the hub's address, each
//! device's module on or off, its registration code and its link's state.

use maker_connect_engine::Order;
use maker_connect_engine::{
    Engine, MachineId, Port,
    ports::dymo_cups::{LabelWriter320, LabelWriter400, LabelWriter450},
    ports::labelle::DymoPnp,
    ports::laserfoundry::port::{LaserFoundryPort, MachineStateHook},
    ports::octoprint::port::{LcdInputs, OctoPrintPort},
};

// The nokhwa camera driver (#94) is not on the SDK's configurator yet: nothing calls it from `main`.
#[allow(dead_code)]
mod camera;
mod cameras;
mod deck;
mod job;
mod laser;
mod marlin;
mod sd_card;
mod serial_ports;
mod wireguard;
use maker_connect_gcode_metadata::py::repr_str;
use maker_connect_protocol::JobOrder;
use maker_connect_protocol::MachineState;
use maker_connect_sdk::command::HandlerError;
use maker_connect_sdk::command::{Command, CommandHandler, Finish};
use maker_connect_sdk::consts::TransferType;
use maker_connect_sdk::consts::{self, Source, State, StorageType};
use maker_connect_sdk::download::{
    ConnDetailsCallback, DownloadFinishedCallback, DownloadMgr, PrintedFileCallback, Transfer,
};
use maker_connect_sdk::files::{Filesystem, InotifyHandler};
use maker_connect_sdk::models::{EventCallback, Kwargs};
use maker_connect_sdk::pyos;
use maker_connect_sdk::util::{py_float_repr, py_truthy};
use maker_connect_sdk::ws::PrinterWs;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The devices this agent carries (`docs/design/wire.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The DYMO LabelManager PnP ("The first device").
    Pnp,
    /// The DYMO LabelWriter 450 ("The second device").
    Lw450,
    /// The DYMO LabelWriter 320: the 450's stack with the 320's PPD (#25).
    Lw320,
    /// The DYMO LabelWriter 400: the 450's stack with the 400's PPD (#230).
    Lw400,
    /// A Marlin printer, not a Prusa (`docs/design/prusa-sdk-extension.md`, part 9).
    Marlin,
    /// A GRBL laser (`docs/design/prusa-sdk-extension.md`, part 4).
    Laser,
    /// The agent's USB cameras (`docs/design/prusa-sdk-extension.md`, part 5, `maker-connect.cameras`).
    Cameras,
}

impl Kind {
    const ALL: [Kind; 7] = [
        Kind::Pnp,
        Kind::Lw450,
        Kind::Lw320,
        Kind::Lw400,
        Kind::Marlin,
        Kind::Laser,
        Kind::Cameras,
    ];

    /// Its type on the wire (`docs/design/prusa-sdk-extension.md`, parts 2 and 3).
    fn printer_type(self) -> &'static str {
        match self {
            Kind::Pnp => "dymo.labelmanager-pnp",
            Kind::Lw450 => "dymo.labelwriter-450",
            Kind::Lw320 => "dymo.labelwriter-320",
            Kind::Lw400 => "dymo.labelwriter-400",
            Kind::Marlin => "marlin.fdm",
            Kind::Laser => "grbl.laser",
            Kind::Cameras => "maker-connect.cameras",
        }
    }

    /// Its folder under the agent's: its settings file and its labels' storage.
    fn folder(self) -> &'static str {
        match self {
            Kind::Pnp => "dymo",
            Kind::Lw450 => "labelwriter-450",
            Kind::Lw320 => "labelwriter-320",
            Kind::Lw400 => "labelwriter-400",
            Kind::Marlin => "marlin",
            Kind::Laser => "laser",
            Kind::Cameras => "cameras",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Kind::Pnp => "DYMO LabelManager PnP",
            // Owner, 2026-11-23: the models it drives (#209); the wire type stays `dymo.labelwriter-450`.
            Kind::Lw450 => "DYMO LabelWriter 450 · 450 Turbo · 450 DUO",
            Kind::Lw320 => "DYMO LabelWriter 320",
            Kind::Lw400 => "DYMO LabelWriter 400",
            Kind::Marlin => "Marlin printer",
            Kind::Laser => "GRBL laser",
            Kind::Cameras => "Cameras",
        }
    }

    /// The additions of `docs/design/prusa-sdk-extension.md` this device uses (part 6a's names).
    fn extensions(self) -> &'static [&'static str] {
        match self {
            Kind::Pnp => &[
                "type.string",
                "pnp.telemetry",
                "label.tape_size_mm",
                "agent.id",
            ],
            Kind::Lw450 | Kind::Lw320 | Kind::Lw400 => &[
                "type.string",
                "cups.states",
                "label.page_size",
                "label.stop",
                "agent.id",
            ],
            Kind::Marlin => &["type.string", "agent.id", "marlin.fdm", "gcode.output"],
            // Part 4: its telemetry and its commands (#171).
            Kind::Laser => &["type.string", "agent.id", "grbl.telemetry", "grbl.commands"],
            // Part 5: no telemetry keys and no commands of its own.
            Kind::Cameras => &["type.string", "agent.id"],
        }
    }

    /// Whether it is a DYMO LabelWriter (the 450's stack).
    fn labelwriter(self) -> bool {
        matches!(self, Kind::Lw450 | Kind::Lw320 | Kind::Lw400)
    }

    /// Whether it is on a serial port the agent's page chooses (its connection form).
    fn serial(self) -> bool {
        matches!(self, Kind::Marlin | Kind::Laser)
    }

    /// The device's port; none for a Marlin or a laser whose chosen USB device is not there
    /// now, or a laser with no port chosen (LaserFoundry has no AUTO: its port is required, G14).
    /// `hook` is told a laser's GRBL state (part 4's `machine_state`).
    /// `lcd` is what a Marlin's screen shows (#189, `marlin::show`).
    fn port(self, s: &Settings, hook: MachineStateHook, lcd: LcdInputs) -> Option<Box<dyn Port>> {
        Some(match self {
            Kind::Pnp => Box::new(DymoPnp::new()),
            Kind::Lw450 => Box::new(LabelWriter450::new()),
            Kind::Lw320 => Box::new(LabelWriter320::new()),
            Kind::Lw400 => Box::new(LabelWriter400::new()),
            // The port and baud rate chosen on the agent's page; AUTO is OctoPrint's own
            // detection (`None`). A USB port is opened under the name its device has now.
            Kind::Marlin => Box::new({
                // `[octoprint]`: the user's config.yaml over OctoPrint's defaults, given before
                // the port reads its first setting. Taken once per agent run (OctoPrint's
                // settings are made once too); a change needs a restart.
                if !s.octoprint.is_empty()
                    && maker_connect_engine::ports::octoprint::settings::configure(
                        octoprint_config(&s.octoprint),
                    )
                    .is_err()
                {
                    log::debug!("[octoprint] settings were given already; kept as they are");
                }
                OctoPrintPort::new(
                    marlin_port_name(s)?,
                    (s.baudrate != 0).then_some(s.baudrate),
                )
                .with_lcd(lcd)
            }),
            // LaserFoundry's baud rate (`DEFAULT_BAUD`): its connection panel has no choice.
            Kind::Laser => Box::new(
                LaserFoundryPort::new(Some(marlin_port_name(s)??), None)
                    .with_machine_state_hook(hook),
            ),
            // Part 5: its cameras are the SDK's, not a port of the engine (`cameras.rs`).
            Kind::Cameras => return None,
        })
    }
}

/// A Marlin's chosen USB device is not plugged in, or cannot be told from another of its kind
/// (ours, the agent's page only).
const NOT_THERE: &str =
    "the chosen serial port is not there: plug it in, or turn off and choose it";

/// The port field's value for "the chosen port that is not there": keep it.
const KEEP: &str = "KEEP";

/// The name to open a Marlin's port by: AUTO (`None` inside), the chosen USB device's name now,
/// or a port that is not USB by its name. `None` outside: the USB device is not there.
fn marlin_port_name(s: &Settings) -> Option<Option<String>> {
    if s.serial_port.is_empty() {
        return Some(None);
    }
    match serial_ports::UsbId::from_setting(&s.usb) {
        Some(usb) => serial_ports::find(&serial_ports::list(), &usb).map(|l| Some(l.name.clone())),
        None => Some(Some(s.serial_port.clone())),
    }
}

/// `[octoprint]`'s dotted keys as the nested JSON of OctoPrint's `config.yaml`:
/// `serial.sdCancelCommand = M524` is `{"serial": {"sdCancelCommand": "M524"}}`. A value that is
/// JSON (`true`, `5`, `["M25"]`) is taken as that, as YAML would; any other is a string.
fn octoprint_config(pairs: &[(String, String)]) -> Value {
    let mut root = Value::Object(Default::default());
    for (key, value) in pairs {
        let value = serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.clone()));
        let mut node = &mut root;
        let mut parts = key.split('.').peekable();
        while let Some(part) = parts.next() {
            let map = match node {
                Value::Object(map) => map,
                other => {
                    *other = Value::Object(Default::default());
                    other.as_object_mut().unwrap()
                }
            };
            if parts.peek().is_none() {
                map.insert(part.to_owned(), value.clone());
                break;
            }
            node = map
                .entry(part.to_owned())
                .or_insert_with(|| Value::Object(Default::default()));
        }
    }
    root
}

const SDK_VERSION: &str = "0.9.0dev0";
/// The build this agent is: a release's tag (`scripts/build-release.sh` sets
/// `MAKER_CONNECT_RELEASE`), or the crate's version. The page, `--version` and the hub see it.
const VERSION: &str = match option_env!("MAKER_CONNECT_RELEASE") {
    Some(tag) => tag,
    None => env!("CARGO_PKG_VERSION"),
};

/// The device's settings file, as PrusaLink's `prusa_printer_settings.ini` (`wire.md`, "What the
/// agent keeps"): `[service::connect] hostname, tls, port, token`, `[printer] type, serial`, and
/// ours: `[module] on` and, for a Marlin, `[connection] port, baudrate` as chosen on the
/// agent's page (empty and 0: AUTO), and `[octoprint]`: OctoPrint settings by their dotted path
/// (`serial.sdCancelCommand = M524`), what OctoPrint's `config.yaml` would hold.
#[derive(Debug, Clone, Default)]
struct Settings {
    printer_type: &'static str,
    hostname: String,
    tls: bool,
    port: u16,
    token: String,
    serial: String,
    on: bool,
    serial_port: String,
    baudrate: u32,
    /// the USB device behind `serial_port` (`serial_ports::UsbId`), to find it again by
    usb: String,
    /// `[octoprint]`, in file order
    octoprint: Vec<(String, String)>,
}

impl Settings {
    /// In the device's own folder.
    fn path(dir: &Path) -> PathBuf {
        dir.join("prusa_printer_settings.ini")
    }

    fn load(dir: &Path, printer_type: &'static str) -> Settings {
        let mut s = Settings {
            printer_type,
            ..Settings::default()
        };
        let text = std::fs::read_to_string(Self::path(dir)).unwrap_or_default();
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                section = line.trim_matches(['[', ']']).to_owned();
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim().to_owned());
            match (section.as_str(), k) {
                ("service::connect", "hostname") => s.hostname = v,
                ("service::connect", "tls") => s.tls = v == "True",
                ("service::connect", "port") => s.port = v.parse().unwrap_or(0),
                ("service::connect", "token") => s.token = v,
                ("printer", "serial") => s.serial = v,
                ("module", "on") => s.on = v == "True",
                ("connection", "port") => s.serial_port = v,
                ("connection", "baudrate") => s.baudrate = v.parse().unwrap_or(0),
                ("connection", "usb") => s.usb = v,
                ("octoprint", _) => s.octoprint.push((k.to_owned(), v)),
                _ => {}
            }
        }
        if s.serial.is_empty() {
            s.serial = random_serial();
            s.save(dir);
        }
        s
    }

    fn save(&self, dir: &Path) {
        let py = |b: bool| if b { "True" } else { "False" };
        let text = format!(
            "[printer]\ntype = {}\nserial = {}\n\n[service::connect]\nhostname = {}\ntls = {}\nport = {}\ntoken = {}\n\n[module]\non = {}\n",
            self.printer_type,
            self.serial,
            self.hostname,
            py(self.tls),
            self.port,
            self.token,
            py(self.on)
        );
        let mut text = text;
        if !self.serial_port.is_empty() || self.baudrate != 0 {
            text += &format!(
                "\n[connection]\nport = {}\nbaudrate = {}\nusb = {}\n",
                self.serial_port, self.baudrate, self.usb
            );
        }
        if !self.octoprint.is_empty() {
            text += "\n[octoprint]\n";
            for (k, v) in &self.octoprint {
                text += &format!("{k} = {v}\n");
            }
        }
        let path = Self::path(dir);
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = std::fs::write(&path, text);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }

    fn server(&self) -> Option<String> {
        if self.hostname.is_empty() {
            return None;
        }
        // WireGuard (#194): while the tunnel runs, the hub is its local port
        if let Some(local) = wireguard::local_hub() {
            return Some(format!("http://{local}"));
        }
        let protocol = if self.tls { "https" } else { "http" };
        Some(if self.port != 0 {
            format!("{protocol}://{}:{}", self.hostname, self.port)
        } else {
            format!("{protocol}://{}", self.hostname)
        })
    }

    /// SHA-256 of the serial, as PrusaLink makes it (`prusa/link/util.py:153-159`).
    fn fingerprint(&self) -> String {
        Sha256::digest(self.serial.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// Random bytes, from the system (`getrandom`: `/dev/urandom` on Unix, `BCryptGenRandom` on Windows).
fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).expect("the system has no random source");
    bytes
}

/// A serial drawn once (the port reads none; `wire.md`).
fn random_serial() -> String {
    format!(
        "MC{}",
        random_bytes::<8>()
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<String>()
    )
}

/// The agent's id (`docs/design/prusa-sdk-extension.md` part 6a, `wire.md` "What the agent
/// keeps"): `[agent] id` in `agent.ini` in the agent's folder, 32 lower-case hex digits drawn on
/// the first start and kept; the same for all its devices.
static AGENT_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// WireGuard inside the agent (`wireguard.rs`, #194): an optional tunnel to the hub.
static WG: std::sync::OnceLock<Arc<wireguard::Wg>> = std::sync::OnceLock::new();

/// The Stream Deck module (`deck.rs`; #190, idea, work in progress): local to the agent.
static DECK: std::sync::OnceLock<Arc<deck::Deck>> = std::sync::OnceLock::new();

fn load_agent_id(dir: &Path) -> String {
    let path = dir.join("agent.ini");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut section = "";
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line.trim_matches(['[', ']']);
        } else if let Some((key, value)) = line.split_once('=')
            && section == "agent"
            && key.trim() == "id"
        {
            let value = value.trim();
            if value.len() == 32
                && value
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            {
                return value.to_owned();
            }
        }
    }
    let id: String = random_bytes::<16>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let _ = std::fs::create_dir_all(dir);
    if let Err(e) = std::fs::write(&path, format!("[agent]\nid = {id}\n")) {
        log::warn!("{}: {e}", path.display());
    }
    id
}

/// A print's tape (`tape_size_mm`) or page (`page_size`).
type Media = (Option<u32>, Option<String>);

/// One device: what the page shows and its link shares.
struct Agent {
    kind: Kind,
    /// Its own folder (`Kind::folder`).
    dir: PathBuf,
    settings: Mutex<Settings>,
    code: Mutex<Option<String>>,
    status: Mutex<String>,
    /// The agent's one engine, shared by its devices.
    engine: Arc<Mutex<Engine>>,
    dymo: Mutex<Option<MachineId>>,
    printing: AtomicBool,
    ws: Mutex<Option<Arc<PrinterWs>>>,
    command: Mutex<Option<Arc<Command>>>,
    event_cb: Mutex<Option<EventCallback>>,
    /// The SDK's `queue` for events (`event_cb` puts, `loop_step` sends: `__init__.py:319-339`,
    /// `858-905`), each as its payload. Telemetry is sent from the link loop, not queued.
    queue: Mutex<std::collections::VecDeque<Value>>,
    /// Held while the socket is checked and dialled: the queue's sender and the link loop's
    /// telemetry share one socket, as the SDK's one loop does.
    dialling: Mutex<()>,
    /// What a Marlin printer's M115 said, in `INFO` (part 9 of the extension).
    firmware: Mutex<Option<marlin::Firmware>>,
    /// A laser's GRBL state by LaserFoundry's name, as its port last told it (part 4).
    machine_state: Arc<Mutex<Option<&'static str>>>,
    /// What a laser said of itself, in `INFO` (part 4).
    laser: Mutex<laser::Known>,
    transfer: Arc<Transfer>,
    /// The SDK's `self.fs` and `self.inotify_handler` (`__init__.py:168-169`, `:903-904`).
    fs: Mutex<Option<Arc<Filesystem>>>,
    inotify: Mutex<Option<Arc<InotifyHandler>>>,
    /// `FolderStorage`'s `attached_set` (`StorageData`, `module_data_classes.py`).
    folders: Mutex<std::collections::HashSet<String>>,
    /// A Marlin's SD card: the list its `SD Card` storage was last given, `None` while it is not
    /// attached (`sd_card.rs`).
    sd_files: Mutex<Option<Vec<maker_connect_protocol::SdFile>>>,
    download: Mutex<Option<Arc<DownloadMgr>>>,
    /// The tape or page each download's print is for, by its command (`tape_size_mm`,
    /// `page_size`).
    media: Mutex<std::collections::HashMap<i64, Media>>,
    /// The state the last `STATE_CHANGED` said.
    last_state: Mutex<Option<State>>,
    /// A Marlin printer's job number, as PrusaLink keeps it (`job.rs`).
    job: Mutex<job::Job>,
    /// The label being printed: its job (the download's command) and file. A Marlin's print.
    label: Mutex<Option<(i64, PathBuf)>>,
    /// A Marlin's print that ended, `FINISHED` or `STOPPED`, and when (`marlin::tick`).
    print_end: Mutex<Option<(State, Instant)>>,
    /// Its link to the hub as the page shows it (ours, UI only).
    hub: Mutex<HubLink>,
    /// What a Marlin's screen shows (#189), kept by `marlin::show`.
    lcd: LcdInputs,
    /// The cameras device's running cameras while its module is on (part 5, `cameras.rs`).
    cameras: Mutex<Option<cameras::Cameras>>,
}

/// Since when the hub takes this device's telemetry, and when it last did.
#[derive(Default, Clone, Copy)]
struct HubLink {
    since: Option<Instant>,
    last: Option<Instant>,
}

impl Agent {
    fn transfer(&self) -> Arc<Transfer> {
        self.transfer.clone()
    }

    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    fn new(kind: Kind, dir: PathBuf, settings: Settings, engine: Arc<Mutex<Engine>>) -> Agent {
        let job = Mutex::new(job::Job::load(&dir));
        Agent {
            kind,
            dir,
            settings: Mutex::new(settings),
            code: Mutex::new(None),
            status: Mutex::new("starting".into()),
            engine,
            dymo: Mutex::new(None),
            printing: AtomicBool::new(false),
            ws: Mutex::new(None),
            command: Mutex::new(None),
            event_cb: Mutex::new(None),
            queue: Mutex::new(Default::default()),
            dialling: Mutex::new(()),
            firmware: Mutex::new(None),
            machine_state: Arc::new(Mutex::new(None)),
            laser: Mutex::new(laser::Known::default()),
            transfer: Arc::new(Transfer::new()),
            fs: Mutex::new(None),
            inotify: Mutex::new(None),
            folders: Mutex::new(Default::default()),
            sd_files: Mutex::new(None),
            download: Mutex::new(None),
            media: Mutex::new(Default::default()),
            last_state: Mutex::new(None),
            job,
            hub: Mutex::new(HubLink::default()),
            label: Mutex::new(None),
            print_end: Mutex::new(None),
            lcd: Arc::new(Mutex::new(marlin::lcd_inputs())),
            cameras: Mutex::new(None),
        }
    }

    /// Whether the last send to the hub went through.
    fn hub_seen(&self, sent: bool) {
        let mut hub = self.hub.lock().unwrap();
        if sent {
            let now = Instant::now();
            hub.since.get_or_insert(now);
            hub.last = Some(now);
        } else {
            hub.since = None;
        }
    }

    fn set_status(&self, status: impl Into<String>) {
        *self.status.lock().unwrap() = status.into();
    }

    /// The common headers (`make_headers`, `__init__.py:273-292`), empty ones left out.
    fn headers(&self, token: bool) -> Vec<(String, Option<String>)> {
        let s = self.settings();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut h = vec![
            ("Fingerprint".to_owned(), Some(s.fingerprint())),
            ("Timestamp".to_owned(), Some(now.to_string())),
            (
                "User-Agent".to_owned(),
                Some(format!("Prusa-Connect-SDK-Printer/{SDK_VERSION}")),
            ),
            (
                "User-Agent-Printer".to_owned(),
                Some(self.kind.printer_type().to_owned()),
            ),
            ("User-Agent-Version".to_owned(), Some(VERSION.to_owned())),
        ];
        if token && !s.token.is_empty() {
            h.push(("Token".to_owned(), Some(s.token)));
        }
        h
    }

    /// Whether the device's module runs: its port in the engine, or the cameras device's cameras.
    fn module_running(&self) -> bool {
        match self.kind {
            Kind::Cameras => self.cameras.lock().unwrap().is_some(),
            _ => self.dymo.lock().unwrap().is_some(),
        }
    }

    /// Turn the device's module on: its port in the engine; the cameras device's cameras.
    fn module_on(self: &Arc<Self>) {
        if self.kind == Kind::Cameras {
            let mut cameras = self.cameras.lock().unwrap();
            if cameras.is_none() {
                *cameras = cameras::Cameras::start(self);
            }
            return;
        }
        let mut dymo = self.dymo.lock().unwrap();
        if dymo.is_none() {
            let told = self.machine_state.clone();
            let hook: MachineStateHook = Arc::new(move |state| *told.lock().unwrap() = state);
            match self.kind.port(&self.settings(), hook, self.lcd.clone()) {
                Some(port) => *dymo = Some(self.engine.lock().unwrap().add(port)),
                None => log::warn!("{}: {NOT_THERE}", self.kind.title()),
            }
        }
    }

    fn module_off(&self) {
        if let Some(cameras) = self.cameras.lock().unwrap().take() {
            cameras.stop();
        }
        if let Some(id) = self.dymo.lock().unwrap().take() {
            let _ = self.engine.lock().unwrap().remove(id);
        }
        // Part 4: `Unknown` until the next status report.
        *self.machine_state.lock().unwrap() = None;
        // The socket is set up once (`setup_link`) and kept: the next send dials it again.
        if let Some(ws) = self.ws.lock().unwrap().clone() {
            ws.disconnect();
        }
    }

    fn send(&self, payload: &Value) -> bool {
        let Some(ws) = self.ensure_ws() else {
            return false;
        };
        ws.send(payload)
    }

    /// The socket, dialled when an item is to be sent and none is open (`ensure_ws_connected`,
    /// `__init__.py:631-638`).
    fn ensure_ws(&self) -> Option<Arc<PrinterWs>> {
        let s = self.settings();
        let server = s.server()?;
        if s.token.is_empty() {
            return None;
        }
        let ws = self.ws.lock().unwrap().clone()?;
        let _dialling = self.dialling.lock().unwrap();
        if !ws.connected() {
            let url = format!(
                "{}/p/ws",
                server
                    .replacen("https://", "wss://", 1)
                    .replacen("http://", "ws://", 1)
            );
            ws.connect(&url, &self.headers(true));
        }
        Some(ws)
    }
}

fn main() {
    log::set_logger(&LOGGER).ok();
    log::set_max_level(log::LevelFilter::Info);
    let mut dir = std::env::var("HOME").map_or(PathBuf::from("."), |h| {
        PathBuf::from(h).join(".maker-connect-agent")
    });
    let mut listen = "0.0.0.0:8470".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => dir = args.next().map(PathBuf::from).unwrap_or(dir),
            "--listen" => listen = args.next().unwrap_or(listen),
            "--version" => {
                println!("maker-connect-agent {VERSION}");
                return;
            }
            _ => {}
        }
    }
    AGENT_ID.get_or_init(|| load_agent_id(&dir));
    let engine = Arc::new(Mutex::new(Engine::new()));
    let mut loaded: Vec<(PathBuf, Settings)> = Kind::ALL
        .into_iter()
        .map(|kind| {
            let dir = dir.join(kind.folder());
            let settings = Settings::load(&dir, kind.printer_type());
            (dir, settings)
        })
        .collect();
    share_hub(&mut loaded);
    let devices: Vec<Arc<Agent>> = Kind::ALL
        .into_iter()
        .zip(loaded)
        .map(|(kind, (dir, settings))| Arc::new(Agent::new(kind, dir, settings, engine.clone())))
        .collect();
    for agent in &devices {
        setup_link(agent);
        if agent.settings().on {
            agent.module_on();
        }
        let events = agent.clone();
        std::thread::spawn(move || {
            loop {
                if !loop_step(&events) {
                    std::thread::sleep(Duration::from_secs_f64(consts::TIMESTAMP_PRECISION));
                }
            }
        });
        let agent = agent.clone();
        std::thread::spawn(move || link_loop(&agent));
    }
    let wg = WG
        .get_or_init(|| Arc::new(wireguard::Wg::load(&dir)))
        .clone();
    if wg.settings().on {
        wg.start(&hub_target(&devices));
    }
    let deck = DECK
        .get_or_init(|| Arc::new(deck::Deck::load(&dir)))
        .clone();
    let shown = devices.clone();
    std::thread::spawn(move || deck::run(deck, move || deck_snapshot(&shown)));
    println!("maker-connect-agent {VERSION} on http://{listen}");
    web(&devices, &listen);
}

/// The SDK's `loop_step` (`__init__.py:858-905`) for an event: the next one in the queue goes over
/// the socket; with no socket to be had it is put back, kept for when the connection is back.
/// No server or no token: skipped. Returns whether an item was taken (`queue.get`'s timeout is the
/// caller's sleep).
fn loop_step(agent: &Agent) -> bool {
    let Some(item) = agent.queue.lock().unwrap().pop_front() else {
        return false;
    };
    let s = agent.settings();
    if s.server().is_none() {
        log::warn!("Server is not set, skipping item: {item}");
        return true;
    }
    if s.token.is_empty() {
        log::warn!("No token, skipping item: {item}");
        return true;
    }
    match agent.ensure_ws() {
        Some(ws) if ws.connected() => {
            ws.send(&item);
        }
        _ => {
            agent.queue.lock().unwrap().push_back(item);
            std::thread::sleep(Duration::from_secs_f64(consts::TIMESTAMP_PRECISION));
        }
    }
    true
}

/// The SDK's command and socket, with this device's handlers.
fn setup_link(agent: &Arc<Agent>) {
    let events = agent.clone();
    let event_cb: EventCallback = Arc::new(move |event, source, ts, command_id, kwargs| {
        if events.settings().token.is_empty() {
            return;
        }
        let mut kwargs = kwargs;
        event_transfer_id(&events, &mut kwargs);
        let e = maker_connect_sdk::models::Event::new(event, source, ts, command_id, kwargs);
        let mut payload = e.to_payload();
        if let Value::Object(map) = &mut payload
            && !map.contains_key("state")
        {
            map.insert("state".into(), current_state(&events).value().into());
        }
        // "Create event and push it to queue."
        events.queue.lock().unwrap().push_back(payload);
    });
    *agent.event_cb.lock().unwrap() = Some(event_cb.clone());
    setup_storage(agent, &event_cb);
    let command = Arc::new(Command::new(event_cb.clone()));
    let me = agent.clone();
    let info: CommandHandler =
        Arc::new(move |_| send_info(&me).map_err(|err| HandlerError::Failed(err.repr())));
    command.set_handler(consts::Command::SendInfo, info);
    setup_files(agent, &command);
    if agent.kind == Kind::Marlin {
        marlin::setup(agent, &command);
    }
    if agent.kind == Kind::Laser {
        laser::setup(agent, &command);
    }
    // A laser takes no download yet (#163): its jobs come with #164.
    if !matches!(agent.kind, Kind::Laser | Kind::Cameras) {
        setup_download(agent, &command, event_cb);
    }
    let incoming = command.clone();
    let ws = Arc::new(PrinterWs::new(Arc::new(move |message: String| {
        handle_ws_message(&incoming, &message)
    })));
    *agent.ws.lock().unwrap() = Some(ws);
    *agent.command.lock().unwrap() = Some(command.clone());
    // The runner (PrusaLink `command_handler.py:23-34`).
    std::thread::spawn(move || {
        loop {
            command.new_cmd_evt.wait(Some(Duration::from_secs(1)));
            command.run();
        }
    });
    let me = agent.clone();
    std::thread::spawn(move || inotify_loop(&me));
}

/// PrusaLink's `inotify_loop` (`sdk_augmentation/printer.py:196-204`), started with the SDK's
/// other threads (`start`, `:154-161`): `self.inotify_handler()`, then `sleep(0.2)`; an
/// exception is logged and the loop goes on, without the sleep. The handler is read again each
/// time, as `attach` replaces it. Its events come from notify's own thread (`files.rs`), where
/// the reference's come from the kernel's queue. Ours: with no handler (notify could not start,
/// where the SDK's `InotifyHandler(self.fs)` raises in `Printer.__init__`), the loop ends; it has
/// no `__inotify_running` to stop it, as the agent's other threads run until it ends.
fn inotify_loop(agent: &Agent) {
    loop {
        let Some(handler) = agent.inotify.lock().unwrap().clone() else {
            log::error!("no file watcher: the folder's changes are not followed");
            return;
        };
        match handler.call(0) {
            Ok(()) => std::thread::sleep(Duration::from_secs_f64(0.2)),
            Err(err) => log::error!("Unhandled exception: {}", err.repr()),
        }
    }
}

/// The device's storage, the folder its files are in: `gcodes` for a Marlin, `labels` for a
/// label printer; none for a laser (#163) or the cameras device.
fn storage(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Marlin => Some(marlin::STORAGE),
        Kind::Pnp | Kind::Lw450 | Kind::Lw320 | Kind::Lw400 => Some("labels"),
        Kind::Laser | Kind::Cameras => None,
    }
}

/// PrusaLink's `DIR_RESCAN_INTERVAL` (`const.py:80`): how often `FolderStorage` looks at its
/// folder.
const DIR_RESCAN_INTERVAL: f64 = 1.0;

/// The SDK's `Filesystem` and `InotifyHandler` (`__init__.py:168-169`: `Filesystem(sep=os.sep,
/// event_cb=self.event_cb)`), and PrusaLink's `FolderStorage` of the device's folder in a thread of
/// its own (`storage_controller.py:95-99`), each `DIR_RESCAN_INTERVAL` from the start of the last
/// look (`ThreadedUpdatable`, `loop_until`, `updatable.py:39-57`, `util.py:51-73`).
///
/// Ours: `sep` stays `/`, as the paths Connect sends are split by it; the folder is the agent's
/// own (`storage`), never configured, so `BLACKLISTED_PATHS`/`BLACKLISTED_NAMES`
/// (`storage.py:46-60`) are not applied; a label printer's is its `labels` folder (no PrusaLink of
/// its own).
fn setup_storage(agent: &Arc<Agent>, event_cb: &EventCallback) {
    let fs = Arc::new(Filesystem::new("/", Some(event_cb.clone())));
    *agent.inotify.lock().unwrap() = new_handler(&fs);
    *agent.fs.lock().unwrap() = Some(fs);
    if storage(agent.kind).is_some() {
        let me = agent.clone();
        std::thread::spawn(move || {
            loop {
                let last_called = Instant::now();
                if let Err(err) = folder_update(&me) {
                    // `loop_until` has no `try`: the exception ends the thread.
                    log::error!("Unhandled exception: {}", err.repr());
                    return;
                }
                let every = Duration::from_secs_f64(DIR_RESCAN_INTERVAL);
                std::thread::sleep(every.saturating_sub(last_called.elapsed()));
            }
        });
    }
}

/// `InotifyHandler(self.fs)`, `None` when notify cannot start.
fn new_handler(fs: &Arc<Filesystem>) -> Option<Arc<InotifyHandler>> {
    match InotifyHandler::new(fs.clone()) {
        Ok(handler) => Some(Arc::new(handler)),
        Err(err) => {
            log::error!("the file watcher: {}", err.repr());
            None
        }
    }
}

/// `Storage.update` (`storage.py:65-83`) of `FolderStorage`: what `get_storage` finds against
/// what was attached, each new folder attached (`folder_attach`, `prusa_link.py:934-936`), each
/// gone detached (`folder_detach`, `:938-940`), then the set kept. The lock keeps one update at a
/// time.
fn folder_update(agent: &Agent) -> Result<(), maker_connect_sdk::files::Error> {
    let mut attached = agent.folders.lock().unwrap();
    let new_storage_set = folder_get_storage(agent);
    let added: Vec<&String> = new_storage_set.difference(&attached).collect();
    let removed: Vec<&String> = attached.difference(&new_storage_set).collect();
    let fs = agent.fs.lock().unwrap().clone().expect("setup_storage");
    for path in added {
        log::info!("Newly attached {path}");
        // `printer.attach(path, os.path.basename(path))` (`__init__.py:888-904`).
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        fs.from_dir(path, &name, StorageType::Local, None)?;
        *agent.inotify.lock().unwrap() = new_handler(&fs);
    }
    for path in removed {
        log::info!("Detached {path}");
        // `printer.detach(os.path.basename(path))` (`__init__.py:906-912`).
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        fs.detach(&name)?;
        *agent.inotify.lock().unwrap() = new_handler(&fs);
    }
    *attached = new_storage_set;
    Ok(())
}

/// `FolderStorage.get_storage` (`storage.py:255-269`) of the device's one folder: made when it
/// does not exist (`ensure_directory`, `util.py:113-116`), logged when that fails, kept when it
/// exists and is readable (`dir_belongs`, `:271-279`).
fn folder_get_storage(agent: &Agent) -> std::collections::HashSet<String> {
    let mut new_directory_set = std::collections::HashSet::new();
    let Some(storage) = storage(agent.kind) else {
        return new_directory_set;
    };
    let path = agent.dir.join(storage);
    let directory = os_path_str(&path);
    if !pyos::exists(directory.as_bytes())
        && let Err(err) = std::fs::create_dir_all(&path)
    {
        log::error!("Cannot create a directory at {directory}: {err}");
    }
    let exists = pyos::exists(directory.as_bytes());
    let readable = exists && pyos::access(directory.as_bytes(), pyos::R_OK).unwrap_or(false);
    if exists && readable {
        new_directory_set.insert(directory);
    } else {
        log::warn!("Directory {directory} does not exist or isn't readable.");
    }
    new_directory_set
}

/// The SDK's file handlers, set in every `Printer` (`__init__.py:138-145`).
fn setup_files(agent: &Arc<Agent>, command: &Command) {
    let me = agent.clone();
    let info: CommandHandler = Arc::new(move |cmd| get_file_info(&me, cmd));
    command.set_handler(consts::Command::SendFileInfo, info);
    for name in [
        consts::Command::CreateFolder,
        consts::Command::CreateDirectory,
    ] {
        let me = agent.clone();
        let create: CommandHandler = Arc::new(move |cmd| create_folder(&me, cmd));
        command.set_handler(name, create);
    }
    let me = agent.clone();
    let delete: CommandHandler = Arc::new(move |cmd| delete_file(&me, cmd));
    command.set_handler(consts::Command::DeleteFile, delete);
    for name in [
        consts::Command::DeleteFolder,
        consts::Command::DeleteDirectory,
    ] {
        let me = agent.clone();
        let delete: CommandHandler = Arc::new(move |cmd| delete_folder(&me, cmd));
        command.set_handler(name, delete);
    }
}

/// `caller.kwargs["path"]` of a file handler, `message` raised when the kwargs lack it
/// (`if not caller.kwargs or "path" not in caller.kwargs`). A path that is not a `str` fails
/// as the handler's first use of it does: `attr` of it (`str.split`/`str.strip`), or, with no
/// `attr`, `os.path` (`TypeError`).
fn path_kwarg(cmd: &Command, message: &str, attr: Option<&str>) -> Result<String, HandlerError> {
    let failed = |e: String| HandlerError::Failed(e);
    let kwargs = cmd
        .kwargs()
        .filter(py_truthy)
        .filter(|k| k.get("path").is_some())
        .ok_or_else(|| failed(value_error(message)))?;
    let path = &kwargs["path"];
    match (path.as_str(), attr) {
        (Some(path), _) => Ok(path.to_owned()),
        (None, Some(attr)) => Err(failed(format!(
            "AttributeError({})",
            repr_str(&format!(
                "'{}' object has no attribute '{attr}'",
                py_type(path)
            ))
        ))),
        (None, None) => Err(failed(format!(
            "TypeError('expected str, bytes or os.PathLike object, not {}')",
            py_type(path)
        ))),
    }
}

/// The SDK's `self.fs` and `self.inotify_handler`, both set up by `setup_storage`.
fn fs_and_handler(agent: &Agent) -> (Arc<Filesystem>, Option<Arc<InotifyHandler>>) {
    let fs = agent.fs.lock().unwrap().clone().expect("setup_storage");
    (fs, agent.inotify.lock().unwrap().clone())
}

/// `self.inotify_handler.get_abs_os_path(path)`.
fn abs_os_path(handler: Option<&InotifyHandler>, path: &str) -> Result<String, HandlerError> {
    let Some(handler) = handler else {
        return Err(HandlerError::Failed(
            "AttributeError(\"'NoneType' object has no attribute 'get_abs_os_path'\")".into(),
        ));
    };
    handler
        .get_abs_os_path(path)
        .map_err(|err| HandlerError::Failed(err.repr()))
}

/// `get_file_info` (`__init__.py:526-538` at c0d7d233).
fn get_file_info(agent: &Agent, cmd: &Command) -> Result<Finish, HandlerError> {
    let path = path_kwarg(cmd, "SEND_FILE_INFO requires kwargs", Some("split"))?;
    let (fs, _) = fs_and_handler(agent);
    let mut finish = Finish::new(Source::Connect);
    finish.event = Some(consts::Event::FileInfo);
    // PrusaLink's `get_file_info` (`sdk_augmentation/printer.py:73-92`): a path on the card is
    // `sd_file_info`.
    finish.kwargs = if sd_card::file_is_on_sd(&path) {
        sd_card::sd_file_info(&fs, &path)
    } else {
        fs.file_info(&path)
    }
    .map_err(maker_connect_sdk::fileinfo::Error::into_handler_error)?;
    Ok(finish)
}

/// `self.fs.get(path).to_dict()["read_only"]`, `True` when truthy.
fn read_only(fs: &Filesystem, path: &str) -> Result<bool, HandlerError> {
    let Some(node) = fs.get(path) else {
        return Err(HandlerError::Failed(
            "AttributeError(\"'NoneType' object has no attribute 'to_dict'\")".into(),
        ));
    };
    match node.to_dict(true).get("read_only") {
        Some(value) => Ok(py_truthy(value)),
        None => Err(HandlerError::Failed("KeyError('read_only')".into())),
    }
}

/// `delete_file` (`__init__.py:549-564`).
fn delete_file(agent: &Agent, cmd: &Command) -> Result<Finish, HandlerError> {
    let name = cmd.command_name().unwrap_or_default();
    let path = path_kwarg(cmd, &format!("{name} requires kwargs"), Some("strip"))?;
    let (fs, handler) = fs_and_handler(agent);
    if read_only(&fs, &path)? {
        return Err(HandlerError::Failed(value_error("File is read only")));
    }
    if printed_file(agent).as_deref() == Some(path.as_str()) {
        return Err(HandlerError::Failed(value_error(
            "This file is currently printed",
        )));
    }
    let abs_path = abs_os_path(handler.as_deref(), &path)?;
    maker_connect_sdk::files::delete(&abs_path, false, false)
        .map_err(|err| HandlerError::Failed(err.repr()))?;
    Ok(Finish::new(Source::Connect))
}

/// `delete_folder` (`__init__.py:566-583`).
fn delete_folder(agent: &Agent, cmd: &Command) -> Result<Finish, HandlerError> {
    let name = cmd.command_name().unwrap_or_default();
    let path = path_kwarg(cmd, &format!("{name} requires kwargs"), Some("strip"))?;
    let (fs, handler) = fs_and_handler(agent);
    if read_only(&fs, &path)? {
        return Err(HandlerError::Failed(value_error("Folder is read only")));
    }
    // `if caller.kwargs["path"] in self.printed_file_cb()`: a substring.
    if printed_file(agent).is_some_and(|printed| printed.contains(&path)) {
        return Err(HandlerError::Failed(value_error(
            "The file inside of this folder is currently printed",
        )));
    }
    let abs_path = abs_os_path(handler.as_deref(), &path)?;
    let force = cmd
        .kwargs()
        .and_then(|k| k.get("force").cloned())
        .is_some_and(|f| py_truthy(&f));
    maker_connect_sdk::files::delete(&abs_path, true, force)
        .map_err(|err| HandlerError::Failed(err.repr()))?;
    Ok(Finish::new(Source::Connect))
}

/// `create_folder` (`__init__.py:585-595`): `os.makedirs(abs_path, exist_ok=True)`.
fn create_folder(agent: &Agent, cmd: &Command) -> Result<Finish, HandlerError> {
    let name = cmd.command_name().unwrap_or_default();
    let path = path_kwarg(cmd, &format!("{name} requires kwargs"), None)?;
    let (_, handler) = fs_and_handler(agent);
    let abs_path = abs_os_path(handler.as_deref(), &path)?;
    maker_connect_sdk::pyos::makedirs(abs_path.as_bytes(), true)
        .map_err(|err| HandlerError::Failed(err.repr()))?;
    Ok(Finish::new(Source::Connect))
}

/// The SDK's `printed_file_cb`: a Marlin's as PrusaLink sets it (`prusa_link.py:198`, `:512-516`,
/// [`job::Job::printed_file`]). Ours, for a label printer (no PrusaLink of its own): the Connect
/// path of the label being printed, `None` when none is.
fn printed_file(agent: &Agent) -> Option<String> {
    if agent.kind == Kind::Marlin {
        return agent.job.lock().unwrap().printed_file();
    }
    let label = agent.label.lock().unwrap();
    let (_, file) = label.as_ref()?;
    connect_path(agent, file)
}

/// A file in the device's folder as its Connect path: `/<storage>/<path in it>` (ours, on Windows:
/// its `\\` written `/`).
pub(crate) fn connect_path(agent: &Agent, file: &Path) -> Option<String> {
    let rest = file.strip_prefix(&agent.dir).ok()?;
    Some(format!("/{}", rest.to_string_lossy().replace('\\', "/")))
}

/// The download handlers and manager (`__init__.py:149-154`, `:168-174`), the manager's loop in
/// a thread of its own as PrusaLink runs it (`sdk_augmentation/printer.py:40`, `:205-208`), on
/// the one `Filesystem` of [`setup_storage`].
fn setup_download(agent: &Arc<Agent>, command: &Command, event_cb: EventCallback) {
    let me = agent.clone();
    let download: CommandHandler = Arc::new(move |cmd| start_connect_download(&me, cmd));
    command.set_handler(consts::Command::StartConnectDownload, download);
    let me = agent.clone();
    let stop: CommandHandler = Arc::new(move |cmd| transfer_stop(&me, cmd));
    command.set_handler(consts::Command::StopTransfer, stop);
    let me = agent.clone();
    let info: CommandHandler = Arc::new(move |cmd| transfer_info(&me, cmd));
    command.set_handler(consts::Command::SendTransferInfo, info);
    if agent.kind.labelwriter() {
        let me = agent.clone();
        let stop: CommandHandler = Arc::new(move |_| stop_print(&me));
        command.set_handler(consts::Command::StopPrint, stop);
    }

    let fs = agent.fs.lock().unwrap().clone().expect("setup_storage");
    let me = agent.clone();
    let conn_details: ConnDetailsCallback =
        Arc::new(move || (me.settings().server(), me.headers(true)));
    let me = agent.clone();
    let printed_file: PrintedFileCallback = Arc::new(move || printed_file(&me));
    let me = agent.clone();
    let finished: DownloadFinishedCallback = Arc::new(move |t| label_downloaded(&me, t));
    let mgr = Arc::new(DownloadMgr::new(
        fs,
        agent.transfer(),
        conn_details,
        event_cb,
        printed_file,
        finished,
    ));
    *agent.download.lock().unwrap() = Some(mgr.clone());
    std::thread::spawn(move || mgr.run_loop());
}

/// `if self.transfer.in_progress and self.transfer.start_ts: kwargs['transfer_id'] = ...`
/// (`event_cb`, `__init__.py:331-332`).
fn event_transfer_id(agent: &Agent, kwargs: &mut Kwargs) {
    let transfer = agent.transfer();
    let (in_progress, start_ts, transfer_id) = {
        let d = transfer.data();
        (
            d.type_ != TransferType::NoTransfer,
            d.start_ts,
            d.transfer_id,
        )
    };
    if in_progress && start_ts != 0.0 {
        kwargs.insert("transfer_id".into(), transfer_id.into());
    }
}

/// The transfer's telemetry (`telemetry`, `__init__.py:355-360`); `false` when it raises (a
/// `progress` of size 0), which leaves the telemetry out.
fn telemetry_transfer(agent: &Agent, kwargs: &mut Kwargs) -> bool {
    let transfer = agent.transfer();
    let (in_progress, start_ts, transfer_id) = {
        let d = transfer.data();
        (
            d.type_ != TransferType::NoTransfer,
            d.start_ts,
            d.transfer_id,
        )
    };
    if !(in_progress && start_ts != 0.0) {
        return true;
    }
    let (Ok(progress), Ok(remaining)) = (transfer.progress(), transfer.time_remaining()) else {
        return false;
    };
    kwargs.insert("transfer_id".into(), transfer_id.into());
    kwargs.insert("transfer_progress".into(), progress.into());
    kwargs.insert("transfer_time_remaining".into(), remaining.into());
    kwargs.insert("transfer_transferred".into(), transfer.transferred().into());
    kwargs.insert(
        "time_transferring".into(),
        transfer.time_transferring().into(),
    );
    true
}

/// `handle_ws_message` (`__init__.py:640-724`).
fn handle_ws_message(command: &Command, message: &str) {
    if message.len() < 9 {
        log::warn!("Too short message: {message}");
        return;
    }
    let (kind, rest) = message.split_at(1);
    let Ok(command_id) = i64::from_str_radix(&rest[..8], 16) else {
        command.reject(Source::Connect, "Invalid command_id", Kwargs::new());
        return;
    };
    let body = &rest[8..];
    let (name, args, kwargs, force) = match kind {
        "T" => return,
        "G" | "F" => (
            "GCODE".to_owned(),
            Some(json!([body])),
            Some(json!({"gcode": body})),
            kind == "F",
        ),
        "J" => {
            let Ok(data) = serde_json::from_str::<Value>(body) else {
                command.failed(
                    Source::Connect,
                    "Invalid JSON body",
                    Some(command_id),
                    Kwargs::new(),
                );
                return;
            };
            (
                data["command"].as_str().unwrap_or("").to_owned(),
                data.get("args").cloned(),
                data.get("kwargs").cloned(),
                false,
            )
        }
        _ => {
            log::warn!("Unknown message type: {kind}");
            return;
        }
    };
    if command.check_state(command_id, &name) {
        command.accept(command_id, &name, args, kwargs, force);
    }
}

/// `send_info` (`__init__.py:393-434`), as far as this device has it. Its `storages` are
/// `self.fs.storages()` (`:413`), whose error the command fails with.
fn send_info(agent: &Agent) -> Result<Finish, maker_connect_sdk::files::Error> {
    let s = agent.settings();
    let mut finish = Finish::new(Source::Connect);
    finish.event = Some(consts::Event::Info);
    let known = agent.firmware.lock().unwrap().clone().unwrap_or_default();
    let laser = agent.laser.lock().unwrap().clone();
    let kwargs: Kwargs = json!({
        "type": agent.kind.printer_type(),
        // The SDK's "Printer firmware version" (`__init__.py:88`): a Marlin's M115 name (part 9);
        // a device the agent cannot ask, the agent's.
        "firmware": known.name.as_deref().unwrap_or(VERSION),
        "sdk": SDK_VERSION,
        "sn": s.serial,
        "fingerprint": s.fingerprint(),
        // Ours: none before `setup_link` made the SDK's `Filesystem`.
        "storages": match agent.fs.lock().unwrap().clone() {
            Some(fs) => fs.storages()?,
            None => Vec::new(),
        },
        // As PrusaLink fills it (`printer_polling.py:501-506`), as far as the agent knows it.
        "network_info": {"hostname": maker_connect_system::computer().host},
        // Part 6a of the extension.
        "agent_id": AGENT_ID.get().cloned().unwrap_or_default(),
        "extensions": agent.kind.extensions(),
    })
    .as_object()
    .unwrap()
    .clone();
    finish.kwargs = kwargs;
    // Part 4: the laser's settings.
    if agent.kind == Kind::Laser {
        laser::info(&mut finish.kwargs, laser.settings.as_ref());
    }
    // Part 9: what the printer answered M115, once it did.
    if let Some(info) = known.info {
        finish.kwargs.insert("firmware_info".into(), info.into());
    }
    if let Some(caps) = known.capabilities {
        finish
            .kwargs
            .insert("firmware_capabilities".into(), caps.into());
    }
    Ok(finish)
}

/// PrusaLink's `_send_info_if_changed` (`printer_polling.py:1078-1083`): `INFO` again, as
/// `event_cb(**get_info())` sends it (source `CONNECT`, `__init__.py:409-411`).
pub(crate) fn info_changed(agent: &Agent) {
    let Some(event_cb) = agent.event_cb.lock().unwrap().clone() else {
        return;
    };
    let info = match send_info(agent) {
        Ok(info) => info,
        Err(err) => {
            log::error!("INFO: {}", err.repr());
            return;
        }
    };
    event_cb(
        consts::Event::Info,
        Source::Connect,
        None,
        None,
        info.kwargs,
    );
}

/// `start_connect_download` (`__init__.py:457-483` at c0d7d233): the download goes to the
/// download loop; the command ends with what `DownloadMgr.start` gave. The tape the hub names
/// (`tape_size_mm`, the extension's) is kept for the print.
fn start_connect_download(agent: &Agent, cmd: &Command) -> Result<Finish, HandlerError> {
    let name = consts::Command::StartConnectDownload;
    let failed = |e: String| HandlerError::Failed(e);
    let kwargs = cmd.kwargs().filter(py_truthy).ok_or_else(|| {
        failed(value_error(&format!(
            "Command.{} requires kwargs",
            name.value()
        )))
    })?;

    let Some(server) = agent.settings().server() else {
        return Err(failed("RuntimeError('Printer.server must be set!')".into()));
    };

    let missing = |key: &str| {
        failed(value_error(&format!(
            "Command.{} requires kwarg {}.",
            name.value(),
            repr_str(key)
        )))
    };
    let arg = |key: &str| kwargs.get(key).ok_or_else(|| missing(key));
    // "/p/teams/{team_id}/files/{hash}/raw".format(**caller.kwargs)
    let uri = format!(
        "/p/teams/{}/files/{}/raw",
        py_str(arg("team_id")?),
        py_str(arg("hash")?)
    );
    let path = arg("path")?;
    let Some(path) = path.as_str() else {
        return Err(failed(format!(
            "TypeError('expected str, bytes or os.PathLike object, not {}')",
            py_type(path)
        )));
    };
    let printing = kwargs.get("printing").is_some_and(py_truthy);
    let selecting = kwargs.get("selecting").is_some_and(py_truthy);
    let hash = py_str(arg("hash")?);
    let team_id = arg("team_id")?.clone();
    // The extension's kwarg of each device (parts 2 and 3).
    let media = match agent.kind {
        Kind::Pnp => (kwargs["tape_size_mm"].as_u64().map(|t| t as u32), None),
        Kind::Lw450 | Kind::Lw320 | Kind::Lw400 => {
            (None, kwargs["page_size"].as_str().map(str::to_owned))
        }
        Kind::Marlin | Kind::Laser | Kind::Cameras => (None, None),
    };
    let mgr = agent
        .download
        .lock()
        .unwrap()
        .clone()
        .expect("the download manager");
    let mut retval = mgr
        .start(
            TransferType::FromConnect,
            path,
            Some(&format!("{server}{uri}")),
            Some(printing),
            Some(selecting),
            Some(cmd.command_id()),
            Some(&hash),
            Some(team_id),
        )
        .map_err(|err| failed(err.repr()))?;
    if retval.event != Some(consts::Event::Rejected) {
        agent.media.lock().unwrap().insert(cmd.command_id(), media);
    }
    retval.source = Source::Connect;
    Ok(retval)
}

/// A path as the SDK's port takes it (`maker_connect_sdk::pyos::path_bytes`: on Windows with `/`).
fn os_path_str(path: &std::path::Path) -> String {
    String::from_utf8_lossy(&maker_connect_sdk::pyos::path_bytes(path)).into_owned()
}

/// `transfer_stop` (`__init__.py:485-492`).
fn transfer_stop(agent: &Agent, cmd: &Command) -> Result<Finish, HandlerError> {
    let transfer = agent.transfer();
    if let Some(id) = cmd.kwargs().and_then(|k| k.get("transfer_id").cloned())
        && py_truthy(&id)
        && !same_transfer(&id, &transfer)
    {
        return Err(HandlerError::Failed(
            "RuntimeError('Wrong transfer_id')".into(),
        ));
    }
    transfer.stop();
    Ok(Finish::new(Source::Connect))
}

/// `transfer_info` (`__init__.py:494-503`).
fn transfer_info(agent: &Agent, cmd: &Command) -> Result<Finish, HandlerError> {
    let transfer = agent.transfer();
    if let Some(id) = cmd.kwargs().and_then(|k| k.get("transfer_id").cloned())
        && py_truthy(&id)
        && !same_transfer(&id, &transfer)
    {
        return Err(HandlerError::Failed(
            "CommandFailed('Not current transfer.')".into(),
        ));
    }
    let info = transfer
        .to_dict()
        .map_err(|err| HandlerError::Failed(err.repr()))?;
    Ok(Finish {
        source: Source::Connect,
        event: Some(consts::Event::TransferInfo),
        kwargs: info,
    })
}

/// `transfer_id == self.transfer.transfer_id`, Python's `==` of a JSON number and an int.
fn same_transfer(id: &Value, transfer: &Transfer) -> bool {
    match (id.as_f64(), transfer.data().transfer_id) {
        (Some(id), Some(current)) => id == f64::from(current),
        _ => false,
    }
}

/// `repr(ValueError(message))`
fn value_error(message: &str) -> String {
    format!("ValueError({})", repr_str(message))
}

/// `str(value)` of a JSON value, as `str.format` writes it.
fn py_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) if n.is_f64() => py_float_repr(n.as_f64().unwrap_or(0.0)),
        other => other.to_string(),
    }
}

/// The Python type name of a JSON value.
fn py_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `download_finished_cb(transfer)`, as PrusaLink's (`prusa_link.py:519-540`) starts the print of
/// a download with `to_print`: the label goes to the DYMO port (ours: the port in place of
/// PrusaLink's `StartPrint`), its state `PRINTING` as `set_state` says it.
fn label_downloaded(agent: &Agent, transfer: &Transfer) {
    let d = transfer.data().clone();
    let (tape, page_size) = d
        .start_cmd_id
        .and_then(|id| agent.media.lock().unwrap().remove(&id))
        .unwrap_or_default();
    if agent.kind == Kind::Marlin {
        marlin::downloaded(agent, transfer);
        return;
    }
    if d.to_print != Some(true) {
        return;
    }
    let (Some(job), Some(file)) = (d.start_cmd_id, d.os_path.clone()) else {
        return;
    };
    let file = PathBuf::from(file);
    let Some(id) = *agent.dymo.lock().unwrap() else {
        set_state(agent, State::Error, Some("the DYMO module is off"));
        set_state(agent, State::Idle, None);
        return;
    };
    let order = JobOrder {
        id: job,
        name: d.path.clone().unwrap_or_default(),
        download: d.url.clone().unwrap_or_default(),
        size: std::fs::metadata(&file).map_or(0, |m| m.len()),
        from_line: None,
        tape_mm: tape,
        label_length_mm: None,
        page_size,
    };
    log::info!("label {job}: downloaded, to the printer");
    agent.printing.store(true, Ordering::SeqCst);
    *agent.label.lock().unwrap() = Some((job, file.clone()));
    set_state(agent, State::Printing, None);
    let sent = agent
        .engine
        .lock()
        .unwrap()
        .send(id, Order::Job { order, file });
    if sent.is_err() {
        label_ended(agent, false, Some("the DYMO port has ended".into()));
    }
}

/// A label printed, or not: the file goes, the state is `ERROR` with the port's reason when it
/// failed, then `IDLE` (`docs/design/wire.md`, "The first device").
///
/// A job that printed ended in the driver's `PRINTING` (the LabelWriter's driver ends a job only
/// once the printer is ready again): when the last `STATE_CHANGED` said otherwise, that comes
/// first (`docs/design/wire.md`, "The second device").
fn label_ended(agent: &Agent, ok: bool, reason: Option<String>) {
    log::info!(
        "label ended: {}",
        if ok {
            "printed"
        } else {
            reason.as_deref().unwrap_or("failed")
        }
    );
    if let Some((_, file)) = agent.label.lock().unwrap().take() {
        let _ = std::fs::remove_file(file);
    }
    agent.printing.store(false, Ordering::SeqCst);
    if ok && *agent.last_state.lock().unwrap() != Some(State::Printing) {
        set_state(agent, State::Printing, None);
    }
    if !ok {
        let reason = reason.unwrap_or_else(|| "the label did not print".into());
        set_state(agent, State::Error, Some(&reason));
    }
    set_state(agent, State::Idle, None);
}

/// Why an `Action` could not be handed to the device's port.
pub(crate) enum Gone {
    /// The device's module is off: no port.
    Off,
    /// Its port has ended.
    Ended,
}

/// An `Action` to the device's port, waited for until the port says it is done (the Marlin's
/// `run`, PrusaLink's `do_command`, `command_queue.py:69-90`): what the device said, or the port's
/// reason.
pub(crate) fn run_action(
    agent: &Agent,
    command_id: i64,
    action: maker_connect_protocol::Action,
) -> Result<Result<Vec<String>, String>, Gone> {
    let Some(id) = *agent.dymo.lock().unwrap() else {
        return Err(Gone::Off);
    };
    let order = command_id as u32;
    agent
        .engine
        .lock()
        .unwrap()
        .send(id, Order::Command { id: order, action })
        .map_err(|_| Gone::Ended)?;
    loop {
        // Ours: how often the port's report is looked at, not on the wire.
        std::thread::sleep(Duration::from_millis(100));
        let report = agent
            .engine
            .lock()
            .unwrap()
            .report(id)
            .map_err(|_| Gone::Ended)?;
        if let Some(i) = report.done.iter().position(|d| d.id == order) {
            let done = report.done[i].clone();
            let _ = agent.engine.lock().unwrap().reported(id, i + 1, 0);
            return Ok(if done.ok {
                Ok(done.output)
            } else {
                Err(done.reason.unwrap_or_else(|| "failed".to_owned()))
            });
        }
    }
}

/// `set_state(state, source, **kwargs)` (`__init__.py`): the `STATE_CHANGED` event. The source
/// is ours: the device.
fn set_state(agent: &Agent, state: State, reason: Option<&str>) {
    *agent.last_state.lock().unwrap() = Some(state);
    // A Marlin printer's job (PrusaLink's `pre_state_change` → `job.state_changed`, before the state
    // goes out, `prusa_link.py:1019-1024`).
    if agent.kind == Kind::Marlin {
        agent.job.lock().unwrap().state_changed(state);
    }
    let Some(event_cb) = agent.event_cb.lock().unwrap().clone() else {
        return;
    };
    let mut kwargs = Kwargs::new();
    kwargs.insert("state".into(), state.value().into());
    if let Some(reason) = reason {
        kwargs.insert("reason".into(), reason.into());
    }
    // The SDK's `event_cb` adds the printer's job while there is one (`__init__.py:322-323`).
    if agent.kind == Kind::Marlin
        && let Some(id) = agent.job.lock().unwrap().for_api()
    {
        kwargs.insert("job_id".into(), id.into());
    }
    event_cb(
        consts::Event::StateChanged,
        Source::Firmware,
        None,
        None,
        kwargs,
    );
    // `post_state_change` → `job.tick` (`prusa_link.py:1026-1029`): after the state went out.
    if agent.kind == Kind::Marlin {
        agent.job.lock().unwrap().tick();
    }
}

fn current_state(agent: &Agent) -> State {
    // Part 5: `IDLE` while its module is on; that state is ours, it prints nothing.
    if agent.kind == Kind::Cameras {
        return State::Idle;
    }
    let report = agent
        .dymo
        .lock()
        .unwrap()
        .and_then(|id| agent.engine.lock().unwrap().report(id).ok());
    if agent.kind == Kind::Marlin {
        return marlin::state(agent, report.as_ref());
    }
    if agent.kind == Kind::Laser {
        return laser::state(agent, report.as_ref());
    }
    if agent.printing.load(Ordering::SeqCst) {
        return match (agent.kind, report) {
            (Kind::Lw450 | Kind::Lw320 | Kind::Lw400, Some(r)) => lw450_state(&r.telemetry),
            _ => State::Printing,
        };
    }
    match report {
        Some(r) if r.telemetry.state == MachineState::Busy => State::Printing,
        _ => State::Idle,
    }
}

/// The LabelWriter's state during a job: the driver's job state by its `STATE:` keyword
/// (`docs/design/prusa-sdk-extension.md`, part 3; driver `src/common/CupsPrintEnvironment.cpp:159-194`):
/// `jsPaperOut` `ATTENTION`, `jsError` `ERROR`, otherwise `PRINTING`.
fn lw450_state(t: &maker_connect_protocol::Telemetry) -> State {
    match t.state_reason.as_deref() {
        Some("com.dymo.out-of-paper-error") => State::Attention,
        Some("com.dymo.general-error") => State::Error,
        _ => State::Printing,
    }
}

/// `STOP_PRINT` on the LabelWriter (`docs/design/prusa-sdk-extension.md`, part 3): the port's Stop,
/// a CUPS cancel. The job's end comes from the port.
fn stop_print(agent: &Agent) -> Result<Finish, HandlerError> {
    let id = *agent.dymo.lock().unwrap();
    if let Some(id) = id {
        let _ = agent
            .engine
            .lock()
            .unwrap()
            .send(id, Order::Control(maker_connect_protocol::JobControl::Stop));
    }
    Ok(Finish::new(Source::Connect))
}

/// The cameras device's turn of the link loop (part 5): registration, then `IDLE` with no keys of its
/// own while its module is on. Its cameras register themselves (`cameras.rs`).
fn cameras_link(agent: &Arc<Agent>, server: &str, s: &Settings) {
    if !agent.module_running() {
        agent.hub_seen(false);
        agent.set_status("the module is off");
        return;
    }
    if s.token.is_empty() {
        agent.hub_seen(false);
        register(agent, server, s);
        return;
    }
    let mut kwargs = Kwargs::new();
    if let Some(cmd) = agent.command.lock().unwrap().as_ref()
        && cmd.state().is_some()
    {
        kwargs.insert("command_id".into(), cmd.command_id().into());
    }
    let telemetry = maker_connect_sdk::models::Telemetry::new(current_state(agent), None, kwargs);
    let sent = agent.send(&telemetry.to_payload());
    agent.hub_seen(sent);
    agent.set_status(if sent {
        "linked to the hub"
    } else {
        "the hub cannot be reached"
    });
}

/// Registration, then telemetry once a second while the DYMO is there.
fn link_loop(agent: &Arc<Agent>) {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let s = agent.settings();
        let Some(server) = s.server() else {
            agent.hub_seen(false);
            agent.set_status("no hub address");
            continue;
        };
        if agent.kind == Kind::Marlin {
            marlin::show(agent);
        }
        if agent.kind == Kind::Cameras {
            cameras_link(agent, &server, &s);
            continue;
        }
        let Some(id) = *agent.dymo.lock().unwrap() else {
            agent.hub_seen(false);
            agent.set_status(if s.on && agent.kind.serial() {
                NOT_THERE
            } else {
                "the module is off"
            });
            continue;
        };
        if s.token.is_empty() {
            agent.hub_seen(false);
            register(agent, &server, &s);
            continue;
        }
        let report = match agent.engine.lock().unwrap().report(id) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let label = agent.label.lock().unwrap().as_ref().map(|(job, _)| *job);
        if agent.kind == Kind::Marlin {
            if marlin::present(&report) {
                marlin::tick(agent, &report);
            }
        } else if agent.kind == Kind::Laser {
            if laser::present(&report) {
                laser::tick(agent, &report);
            }
        } else if let Some(job) = label
            && let Some(end) = report.job_ends.iter().find(|e| e.id == job).cloned()
        {
            let _ =
                agent
                    .engine
                    .lock()
                    .unwrap()
                    .reported(id, report.done.len(), report.job_ends.len());
            label_ended(agent, end.ok, end.reason);
        } else if label.is_some() && agent.kind.labelwriter() {
            // The driver's job state as it changes, each change a `STATE_CHANGED` (`set_state`).
            let state = lw450_state(&report.telemetry);
            if *agent.last_state.lock().unwrap() != Some(state) {
                // The PPD's own text for the driver's keyword as the event's `reason` (part 3).
                set_state(agent, state, report.telemetry.problem.as_deref());
            }
        }
        // Gone: nothing is sent, the socket ends and the hub sees it away (`wire.md`).
        let gone = match agent.kind {
            Kind::Marlin => !marlin::present(&report),
            Kind::Laser => !laser::present(&report),
            Kind::Pnp | Kind::Lw450 | Kind::Lw320 | Kind::Lw400 => !report.connected,
            Kind::Cameras => unreachable!("the cameras device has its own turn (cameras_link)"),
        };
        if gone {
            agent.hub_seen(false);
            let status = format!("the {} is not connected", agent.kind.title());
            if *agent.status.lock().unwrap() != status {
                // why: the port's state and its text (OctoPrint's `getStateString`)
                log::warn!(
                    "{}: {:?} {:?}",
                    status,
                    report.telemetry.comm_state,
                    report.telemetry.problem
                );
            }
            agent.set_status(status);
            if let Some(ws) = agent.ws.lock().unwrap().clone()
                && ws.connected()
            {
                ws.disconnect();
            }
            continue;
        }
        // The LabelWriter adds no key: its reason is in its `STATE_CHANGED` (part 3).
        let mut kwargs = match agent.kind {
            Kind::Pnp => dymo_telemetry(&report.telemetry),
            Kind::Lw450 | Kind::Lw320 | Kind::Lw400 => Kwargs::new(),
            Kind::Marlin => marlin::telemetry(&report.telemetry),
            Kind::Laser => {
                laser::telemetry(&report.telemetry, *agent.machine_state.lock().unwrap())
            }
            Kind::Cameras => unreachable!("the cameras device has its own turn (cameras_link)"),
        };
        if let Some(cmd) = agent.command.lock().unwrap().as_ref()
            && cmd.state().is_some()
        {
            kwargs.insert("command_id".into(), cmd.command_id().into());
        }
        // The SDK's `telemetry` adds the printer's job while there is one (`__init__.py:346-347`):
        // a Marlin printer's, as PrusaLink keeps it (`job.rs`).
        if agent.kind == Kind::Marlin
            && let Some(id) = agent.job.lock().unwrap().for_api()
        {
            kwargs.insert("job_id".into(), id.into());
        }
        if !telemetry_transfer(agent, &mut kwargs) {
            continue;
        }
        let telemetry =
            maker_connect_sdk::models::Telemetry::new(current_state(agent), None, kwargs);
        let sent = agent.send(&telemetry.to_payload());
        agent.hub_seen(sent);
        if sent {
            agent.set_status("linked to the hub");
        } else {
            agent.set_status("the hub cannot be reached");
        }
    }
}

/// What the DYMO tells beyond its state, as the hub reads it (`docs/design/wire.md`, "The first
/// device"): whether a cassette is in, its charge, its status bytes; what the port does not know
/// is left out.
fn dymo_telemetry(t: &maker_connect_protocol::Telemetry) -> Kwargs {
    let mut kwargs = Kwargs::new();
    if let Some(c) = t.cassette {
        kwargs.insert("media_loaded".into(), c.into());
    }
    if let Some(b) = t.battery_percent {
        kwargs.insert("battery_percent".into(), b.into());
    }
    if let Some(r) = &t.raw_status {
        kwargs.insert("raw_status".into(), r.clone().into());
    }
    kwargs
}

/// `register` and the poll (`__init__.py:726-757`, `:823-882`).
fn register(agent: &Agent, server: &str, s: &Settings) {
    let code = agent.code.lock().unwrap().clone();
    match code {
        None => {
            let body = json!({
                "sn": s.serial,
                "fingerprint": s.fingerprint(),
                "printer_type": agent.kind.printer_type(),
                "firmware": VERSION,
            });
            let mut request = ureq::post(&format!("{server}/p/register"))
                .timeout(Duration::from_secs(consts::CONNECTION_TIMEOUT));
            for (k, v) in agent.headers(false) {
                if let Some(v) = v {
                    request = request.set(&k, &v);
                }
            }
            match request
                .set("Content-Type", "application/json")
                .send_string(&body.to_string())
            {
                Ok(r) => {
                    if let Some(code) = r.header("Code") {
                        *agent.code.lock().unwrap() = Some(code.to_owned());
                        agent.set_status(format!("enter code {code} on the hub"));
                    }
                }
                Err(e) => agent.set_status(format!("registration refused: {e}")),
            }
        }
        Some(code) => {
            let mut request = ureq::get(&format!("{server}/p/register"))
                .timeout(Duration::from_secs(consts::CONNECTION_TIMEOUT))
                .set("Code", &code);
            for (k, v) in agent.headers(false) {
                if let Some(v) = v {
                    request = request.set(&k, &v);
                }
            }
            match request.call() {
                Ok(r) if r.status() == 200 => {
                    if let Some(token) = r.header("Token") {
                        let mut settings = agent.settings.lock().unwrap();
                        settings.token = token.to_owned();
                        settings.save(&agent.dir);
                        *agent.code.lock().unwrap() = None;
                        agent.set_status("registered");
                    }
                }
                Ok(_) => agent.set_status(format!("enter code {code} on the hub")),
                Err(ureq::Error::Status(404, _)) => {
                    *agent.code.lock().unwrap() = None;
                }
                Err(e) => agent.set_status(format!("registration: {e}")),
            }
        }
    }
}

/// One hub for the agent (the page's Hub form writes every device's file): a device without an
/// address, such as one added in a newer version, gets the address the others have.
fn share_hub(devices: &mut [(PathBuf, Settings)]) {
    let Some((hostname, port, tls)) = devices
        .iter()
        .find(|(_, s)| !s.hostname.is_empty())
        .map(|(_, s)| (s.hostname.clone(), s.port, s.tls))
    else {
        return;
    };
    for (dir, s) in devices.iter_mut().filter(|(_, s)| s.hostname.is_empty()) {
        s.hostname = hostname.clone();
        s.port = port;
        s.tls = tls;
        s.save(dir);
    }
}

/// The agent's page.
fn web(devices: &[Arc<Agent>], listen: &str) {
    let server = tiny_http::Server::http(listen).expect("listen");
    for mut request in server.incoming_requests() {
        let path = request.url().to_owned();
        if request.method() == &tiny_http::Method::Post {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let form = parse_form(&body);
            let device = form
                .get("device")
                .and_then(|f| devices.iter().find(|d| d.kind.folder() == f));
            match (path.as_str(), device) {
                // One hub for the agent: each device's settings file gets its address.
                ("/hub", _) => {
                    for agent in devices {
                        let mut s = agent.settings.lock().unwrap();
                        s.hostname = form.get("hostname").cloned().unwrap_or_default();
                        s.port = form.get("port").and_then(|p| p.parse().ok()).unwrap_or(0);
                        s.tls = form.contains_key("tls");
                        s.save(&agent.dir);
                    }
                }
                ("/module", Some(agent)) => set_module(agent, &form),
                // WireGuard (#194): its fields while off, and Turn on / Turn off as every card's
                ("/wireguard", _) => {
                    if let Some(wg) = WG.get() {
                        set_wireguard(wg, devices, &form);
                    }
                }
                // the Stream Deck's Turn on / Turn off, as every card's (#191)
                ("/deck", _) => {
                    if let Some(deck) = DECK.get() {
                        deck.set_on(form.get("on").is_some_and(|v| v == "1"));
                    }
                }
                ("/reset", Some(agent)) => {
                    let mut s = agent.settings.lock().unwrap();
                    s.token.clear();
                    s.save(&agent.dir);
                    *agent.code.lock().unwrap() = None;
                    drop(s);
                    if let Some(ws) = agent.ws.lock().unwrap().clone() {
                        ws.disconnect();
                    }
                }
                _ => {}
            }
            // back to the tab the form is on: Settings for the hub and WireGuard
            let back: &[u8] = if matches!(path.as_str(), "/hub" | "/wireguard") {
                b"/settings"
            } else if device.is_some_and(|d| d.kind == Kind::Cameras) {
                b"/cameras"
            } else {
                b"/"
            };
            let response = tiny_http::Response::empty(303)
                .with_header(tiny_http::Header::from_bytes(&b"Location"[..], back).unwrap());
            let _ = request.respond(response);
            continue;
        }
        if path == "/events" {
            let devices = devices.to_vec();
            std::thread::spawn(move || events(&devices, request.into_writer()));
            continue;
        }
        let html = if path == "/about" {
            about_page(devices)
        } else if path == "/cameras" {
            cameras_page(devices)
        } else if path == "/settings" {
            settings_page(devices)
        } else {
            page(devices)
        };
        let response = tiny_http::Response::from_string(html).with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
                .unwrap(),
        );
        let _ = request.respond(response);
    }
}

/// `/events` (server-sent events, ours): the page's live part each second it changes, a comment
/// every 15 s to keep the connection; ends when the page goes.
fn events(devices: &[Arc<Agent>], mut out: Box<dyn std::io::Write + Send>) {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n";
    if out
        .write_all(head.as_bytes())
        .and_then(|_| out.flush())
        .is_err()
    {
        return;
    }
    let (mut last, mut quiet) = (String::new(), 0);
    loop {
        let now = live(devices);
        let sent = if now != last {
            quiet = 0;
            write!(out, "data: {now}\n\n")
        } else if quiet >= 15 {
            quiet = 0;
            out.write_all(b": keep-alive\n\n")
        } else {
            quiet += 1;
            Ok(())
        };
        if sent.and_then(|_| out.flush()).is_err() {
            return;
        }
        last = now;
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn esc(t: &str) -> String {
    t.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

/// Turn on / Turn off (`/module`), the same on every device (#191). A Marlin's or a laser's Turn
/// on carries its port and baud rate, chosen while it is off (OctoPrint's Connection panel,
/// `connection.py`, with Connect as Turn on: owner, 2026-11-23, no Disconnect on the agent); its
/// port opens with the module and closes with it.
fn set_module(agent: &Arc<Agent>, form: &std::collections::HashMap<String, String>) {
    let on = form.get("on").is_some_and(|v| v == "1");
    {
        let mut s = agent.settings.lock().unwrap();
        if on && agent.kind.serial() {
            choose_port(&mut s, form);
        }
        s.on = on;
        s.save(&agent.dir);
    }
    if on {
        agent.module_on();
    } else {
        agent.module_off();
    }
}

/// The port and baud rate of a Marlin's or a laser's Turn on: the chosen device again (`KEEP`,
/// its settings stay), or the port picked, remembered by its USB device.
fn choose_port(s: &mut Settings, form: &std::collections::HashMap<String, String>) {
    if form.get("port").is_none_or(|p| p == KEEP) {
        return;
    }
    s.serial_port = form
        .get("port")
        .filter(|p| p.as_str() != "AUTO")
        .cloned()
        .unwrap_or_default();
    // the USB device behind the name, to find it by after a restart
    s.usb = serial_ports::list()
        .into_iter()
        .find(|l| l.name == s.serial_port)
        .and_then(|l| l.usb)
        .map(|u| u.to_setting())
        .unwrap_or_default();
    s.baudrate = form
        .get("baudrate")
        .and_then(|b| b.parse().ok())
        .unwrap_or(0);
}

/// What a device's card shows that changes by itself (ours, the agent's page only): the page
/// renders it, and `/events` sends it again whenever it changes.
struct CardLive {
    state: &'static str,
    class: &'static str,
    hub: String,
    status: String,
    registered: String,
    /// what the card is built of (on, registered, its code): when it
    /// changes, the page is loaded again rather than patched
    shape: String,
}

fn card_live(agent: &Agent) -> CardLive {
    let s = agent.settings();
    let code = agent.code.lock().unwrap().clone();
    let registered = if !s.token.is_empty() {
        "yes".to_owned()
    } else if let Some(code) = &code {
        format!("in progress: code {code}")
    } else {
        "no".to_owned()
    };
    let linked = !s.token.is_empty() && s.on;
    let link = *agent.hub.lock().unwrap();
    let (state, class) = if !s.on {
        ("Off", "idle")
    } else if !linked {
        ("Not registered", "warn")
    } else if link.since.is_some() {
        ("Connected", "wait")
    } else {
        ("Not connected", "warn")
    };
    let ago = |t: Instant| maker_connect_system::duration(t.elapsed().as_secs());
    let hub = match (link.since, link.last) {
        (Some(since), _) => format!("connected for {} · last contact now", ago(since)),
        (None, Some(last)) => format!("not connected · last contact {} ago", ago(last)),
        (None, None) => "not connected".to_owned(),
    };
    CardLive {
        state,
        class,
        hub,
        status: agent.status.lock().unwrap().clone(),
        registered,
        shape: format!(
            "{}:{}:{}:{}",
            agent.kind.folder(),
            s.on,
            s.token.is_empty(),
            code.unwrap_or_default()
        ),
    }
}

/// One device's card (mockup `docs/design/ui/mockups/agent.html`).
fn device_card(agent: &Agent) -> String {
    let s = agent.settings();
    let code = agent.code.lock().unwrap().clone();
    let on = s.on;
    let live = card_live(agent);
    let (state, state_class, hub, status, registered) = (
        live.state,
        live.class,
        live.hub,
        live.status,
        live.registered,
    );
    let code_block = match (s.token.is_empty(), code) {
        (true, Some(code)) => format!(
            "<p>Enter this code on the hub: Space, Add machine.</p><p class=code>{}</p>",
            esc(&code)
        ),
        _ => String::new(),
    };
    let what = match agent.kind {
        Kind::Pnp => "Tape printer · USB",
        // dymo-cups-drivers 1.4.0: one driver class and the same PPD sizes for the three
        // (`raster2dymolw.cpp:92-98`); only the 450 tested.
        Kind::Lw450 => {
            "Label printer · USB · Tested: 450. Same driver, not tested: 450 Turbo, 450 DUO (labels)"
        }
        Kind::Lw320 => "Label printer · USB · Not tried on a printer yet",
        Kind::Lw400 => "Label printer · USB · Not tried on a printer yet",
        Kind::Marlin => "3D printer · USB",
        Kind::Laser => "Laser · USB",
        Kind::Cameras => "Cameras · USB",
    };
    // a Marlin or a laser: its port chosen while off, in the Turn on form; shown while on (#191)
    let (fields, port_row) = match (agent.kind.serial(), on) {
        (true, false) => (port_fields(agent, &s), String::new()),
        (true, true) => (String::new(), port_row(agent, &s)),
        _ => (String::new(), String::new()),
    };
    format!(
        r#"<div class="card{off}"><div class=head><h2>{title}</h2><span class="st {state_class}" data-live="{device}.state">{state}</span></div>
<p class=muted>{what}</p><div>{code_block}</div>
<dl class=kv><dt>Hub</dt><dd data-live="{device}.hub">{hub}</dd><dt>Printer</dt><dd data-live="{device}.status">{status}</dd>{port_row}<dt>Registered</dt><dd data-live="{device}.registered">{registered}</dd></dl>
<form method=post action=/module id="module-{device}"{form_class}><input type=hidden name=device value="{device}"><input type=hidden name=on value="{toggle}">{fields}</form><div class="row btns"><span class=fw><button class="btn{toggle_class}" form="module-{device}">{button}</button></span>
<form method=post action=/reset onsubmit="return confirm('Reset the registration of the {title}? It forgets its token: it is no longer known at the hub until it is added again with a new code.')"><input type=hidden name=device value="{device}"><span class=fw><button class="btn ghost">Reset registration…</button></span></form></div></div>"#,
        off = if on { "" } else { " is-off" },
        title = agent.kind.title(),
        device = agent.kind.folder(),
        // Turn on / Turn off, the same on every device (#191)
        form_class = if fields.is_empty() { "" } else { " class=row" },
        toggle = if on { "0" } else { "1" },
        toggle_class = if on { "" } else { " primary" },
        button = if on { "Turn off" } else { "Turn on" },
        status = esc(&status),
        registered = esc(&registered),
    )
}

/// The port a Marlin or a laser was turned on with, while it is on (ours: the port chosen and,
/// for a Marlin, the baud rate, AUTO when none).
fn port_row(agent: &Agent, s: &Settings) -> String {
    let port = if s.serial_port.is_empty() {
        "AUTO".to_owned()
    } else {
        s.serial_port.clone()
    };
    let text = if agent.kind == Kind::Laser {
        port
    } else if s.baudrate == 0 {
        format!("{port} · AUTO")
    } else {
        format!("{port} · {}", s.baudrate)
    };
    format!("<dt>Serial port</dt><dd>{}</dd>", esc(&text))
}

/// A Marlin's port and baud rate as OctoPrint's Connection panel has them
/// (`sidebar/connection.jinja2`): the serial port (AUTO first, then the ports with their USB
/// device, `serial_ports`) and the baud rate (AUTO, then `baudrateList`), chosen while it is off;
/// Turn on stands in for Connect (#191). The fields of the Turn on form.
fn port_fields(agent: &Agent, s: &Settings) -> String {
    use maker_connect_engine::ports::octoprint::comm::baudrate_list;
    let option = |value: &str, text: &str, chosen: bool| {
        format!(
            "<option value=\"{}\"{}>{}</option>",
            esc(value),
            if chosen { " selected" } else { "" },
            esc(text)
        )
    };
    let listed = serial_ports::list();
    // the chosen one: its USB device under the name it has now, else its name
    // the chosen one: its USB device under the name it has now, else a port by its name;
    // never another device that has its old name now
    let chosen = match serial_ports::UsbId::from_setting(&s.usb) {
        Some(u) => serial_ports::find(&listed, &u).map(|l| l.name.clone()),
        None => Some(s.serial_port.clone()).filter(|p| listed.iter().any(|l| l.name == *p)),
    };
    // A laser as LaserFoundry's connection panel (`ui-egui/src/panels.rs:616-646`): the serial
    // ports, no AUTO and no baud rate (it opens at `DEFAULT_BAUD`, `apps/laserfoundry/src/host.rs`).
    let laser = agent.kind == Kind::Laser;
    let mut ports = if laser {
        Vec::new()
    } else {
        vec![option("AUTO", "AUTO", s.serial_port.is_empty())]
    };
    if !s.serial_port.is_empty() && chosen.is_none() {
        // Connect with it: the same device again, once it is back
        ports.push(option(
            KEEP,
            &format!("{} (not there)", s.serial_port),
            true,
        ));
    }
    let chosen = chosen.unwrap_or_default();
    ports.extend(
        listed
            .iter()
            .map(|l| option(&l.name, &l.label, l.name == chosen)),
    );
    let mut rates = vec![option("0", "AUTO", s.baudrate == 0)];
    rates.extend(
        baudrate_list(None)
            .iter()
            .map(|b| option(&b.to_string(), &b.to_string(), *b == i64::from(s.baudrate))),
    );
    format!(
        r#"<label class=field style="flex:1 1 18rem;min-width:0;grid-template-columns:minmax(0,1fr)">Serial port<select name=port>{ports}</select></label>
{rates}"#,
        ports = ports.concat(),
        rates = if laser {
            String::new()
        } else {
            format!(
                r#"<label class=field style="flex:0 1 9rem">Baud rate<select name=baudrate>{}</select></label>
"#,
                rates.concat()
            )
        },
    )
}

/// The link to the hub over all devices: how many of those that are on and registered it
/// takes telemetry from now, of how many.
fn hub_link(devices: &[Arc<Agent>]) -> (usize, usize) {
    let linked: Vec<_> = devices
        .iter()
        .filter(|d| {
            let s = d.settings();
            s.on && !s.token.is_empty()
        })
        .collect();
    let connected = linked
        .iter()
        .filter(|d| d.hub.lock().unwrap().since.is_some())
        .count();
    (connected, linked.len())
}

/// The agent head's pill: the link to the hub over all devices.
fn agent_state(devices: &[Arc<Agent>]) -> (String, &'static str) {
    match hub_link(devices) {
        (_, 0) => ("No device linked".to_owned(), "idle"),
        (c, n) if c == n => ("Connected to the hub".to_owned(), "wait"),
        (0, _) => ("Not connected to the hub".to_owned(), "bad"),
        (c, n) => (format!("{c} of {n} connected to the hub"), "warn"),
    }
}

/// The Hub card's pill.
fn hub_state(devices: &[Arc<Agent>]) -> (&'static str, &'static str) {
    match hub_link(devices) {
        (_, 0) => ("No device linked", "idle"),
        (c, n) if c == n => ("Connected", "wait"),
        (0, _) => ("Not connected", "bad"),
        _ => ("Partly connected", "warn"),
    }
}

/// The Hub card's Devices row.
fn hub_devices(devices: &[Arc<Agent>]) -> String {
    let (connected, linked) = hub_link(devices);
    format!("{connected} of {linked} connected")
}

/// The banner above the agent's head: its kind (part of the page's shape) and its HTML.
fn banner(devices: &[Arc<Agent>]) -> (&'static str, String) {
    let (connected, linked) = hub_link(devices);
    match devices[0].settings().server() {
        None => ("address", r#"<div class="banner warn"><span><b>No hub address.</b> Set it under Hub, on <a href=/settings>Settings</a>.</span></div>"#.to_owned()),
        Some(server) if linked > 0 && connected == 0 => ("unreachable", format!(
            r#"<div class="banner bad"><span><b>The hub cannot be reached</b> at {}. The devices keep their state; a label already sent finishes. The agent tries again by itself.</span></div>"#,
            esc(&server)
        )),
        Some(_) => ("", String::new()),
    }
}

/// What the page is built of (each card's shape, the banner): when `/events` sends another, the
/// page is loaded again rather than patched.
fn shape(devices: &[Arc<Agent>]) -> String {
    let mut parts: Vec<String> = devices.iter().map(|d| card_live(d).shape).collect();
    parts.push(banner(devices).0.to_owned());
    if let Some(deck) = DECK.get() {
        parts.push(format!("deck:{}", deck.on()));
    }
    if let Some(wg) = WG.get() {
        parts.push(format!(
            "wg:{}:{}",
            wg.settings().on,
            wireguard::local_hub().is_some()
        ));
    }
    parts.join("|")
}

/// What `/events` sends: the page's shape and the text (and a pill's class) of each
/// `data-live` element.
fn live(devices: &[Arc<Agent>]) -> String {
    let mut live = serde_json::Map::new();
    for d in devices {
        let c = card_live(d);
        let f = d.kind.folder();
        live.insert(
            format!("{f}.state"),
            serde_json::json!({"text": c.state, "class": c.class}),
        );
        live.insert(format!("{f}.hub"), c.hub.into());
        live.insert(format!("{f}.status"), c.status.into());
        live.insert(format!("{f}.registered"), c.registered.into());
    }
    let (text, class) = agent_state(devices);
    live.insert(
        "agent.state".into(),
        serde_json::json!({"text": text, "class": class}),
    );
    let (text, class) = hub_state(devices);
    live.insert(
        "hub.state".into(),
        serde_json::json!({"text": text, "class": class}),
    );
    live.insert("hub.devices".into(), hub_devices(devices).into());
    if let Some(wg) = WG.get() {
        let (text, class) = wg_state(wg);
        live.insert(
            "wg.state".into(),
            serde_json::json!({"text": text, "class": class}),
        );
        live.insert("wg.status".into(), wg.status().into());
        live.insert("wg.numbers".into(), wg_numbers(wg).into());
    }
    if let Some(deck) = DECK.get() {
        let (text, class) = deck_state(deck);
        live.insert(
            "deck.state".into(),
            serde_json::json!({"text": text, "class": class}),
        );
        live.insert("deck.status".into(), deck.status().into());
    }
    serde_json::json!({"shape": shape(devices), "live": live}).to_string()
}

/// The page's live part (ours): `/events` patches the `data-live` elements; another shape
/// loads the page again (a choice being made in a form is kept while only text changes).
const LIVE_JS: &str = r#"(()=>{const shape=document.body.dataset.shape;const es=new EventSource("/events");es.onmessage=m=>{const d=JSON.parse(m.data);if(d.shape!==shape){es.close();location.reload();return}for(const[k,v]of Object.entries(d.live)){const el=document.querySelector('[data-live="'+k+'"]');if(!el)continue;if(typeof v==="string"){el.textContent=v}else{el.textContent=v.text;el.className="st "+v.class}}}})();"#;

/// The agent's page around `main` (mockup `docs/design/ui/mockups/agent.html`): the header, the
/// agent's head with its link to the hub, a banner when the hub cannot be reached, the tabs
/// (Devices, Cameras when the agent has the cameras device, Settings, About); the hub's stylesheet and
/// components only.
fn layout(devices: &[Arc<Agent>], tab: &str, main: &str) -> String {
    let mut tabs = vec![
        // the devices turned on (owner, 2026-11-23: only the Marlin hangs on it, not 4)
        (
            "/",
            "Devices",
            Some(
                devices
                    .iter()
                    .filter(|d| d.kind != Kind::Cameras && d.settings().on)
                    .count(),
            ),
        ),
        ("/settings", "Settings", None),
        ("/about", "About", None),
    ];
    // the cameras in a tab of their own (owner, 2026-11-23: "cameras horen in de agent in een eigen tab")
    if devices.iter().any(|d| d.kind == Kind::Cameras) {
        tabs.insert(1, ("/cameras", "Cameras", None));
    }
    let tabs: String = tabs
        .iter()
        .map(|(href, name, count)| {
            let current = if *name == tab {
                r#" aria-current="page""#
            } else {
                ""
            };
            let count = count.map_or(String::new(), |n| format!("<small>&nbsp;{n}</small>"));
            format!(r#"<a href="{href}"{current}>{name}{count}</a>"#)
        })
        .collect();
    let (state, class) = agent_state(devices);
    let banner = banner(devices).1;
    let computer = maker_connect_system::computer();
    let host = computer.host.clone().unwrap_or_default();
    let mut what = vec![];
    if !host.is_empty() {
        what.push(esc(&host));
    }
    what.push(format!(
        "{}, {}",
        std::env::consts::OS,
        computer.architecture
    ));
    what.push(format!(
        "{} of {} devices on",
        devices_on(devices),
        devices.len()
    ));
    what.push(format!("release {}", esc(VERSION)));
    format!(
        r##"<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1">
{THEME_FIRST}<title>Maker Connect agent</title><link rel=icon href="data:image/svg+xml,{icon}"><style>{CSS}
.code{{font:700 var(--fs-6)/1 var(--mono);letter-spacing:.12em;color:var(--accent-text);margin:var(--s-2) 0 var(--s-3)}}
.brand .mark svg{{width:100%;height:100%;display:block}} .card form{{margin:0}} .devices>.card{{display:grid;grid-template-rows:subgrid;grid-row:span 6;row-gap:var(--s-2);align-content:start}} .devices>.card>.head{{margin:0;display:grid;grid-template-columns:minmax(0,1fr) auto;align-items:start;gap:var(--s-2)}} .devices>.card>.head>h2{{margin:0}} .devices .row.btns{{margin-top:var(--s-2);align-self:end}} .devices dl.kv{{align-content:start;align-self:start}} .solo-stack{{display:grid;gap:var(--s-4)}} .card .row.btns{{margin-top:var(--s-3)}} .row.btns>form{{flex:0 0 auto}}</style></head><body data-shape="{shape}">
<a class=skip href="#main">Skip to content</a>
<header class=top><span class=brand><span class=mark aria-hidden=true>{MARK}</span><span class=where><b>Maker Connect agent</b><small>{where_}</small></span></span><span class=spacer></span><span class=fw><button type=button class=theme data-theme-btn aria-label="Theme: Auto">Auto</button></span></header>
<main id=main tabindex=-1 class=solo>
{banner}<div class=card><div class=head><h1>This agent</h1><span class="st {class}" data-live="agent.state">{state}</span>{vpn}</div>
<p class=muted>{what}</p></div>
<nav class=tabs aria-label="This agent">{tabs}</nav>
{main}
</main><script>{THEME_SWITCH}</script><script>{LIVE_JS}</script></body></html>"##,
        shape = esc(&shape(devices)),
        // through WireGuard (#194): a VPN mark beside the agent's state (owner, 2026-11-23)
        vpn = if wireguard::local_hub().is_some() {
            format!(
                r#"<span class="st val" title="The hub is reached through this agent's WireGuard tunnel">{VPN_ICON}WireGuard</span>"#
            )
        } else {
            String::new()
        },
        icon = MARK.replace('#', "%23").replace('"', "'").replace('\n', ""),
        where_ = if host.is_empty() {
            esc(VERSION)
        } else {
            format!("{} · {}", esc(&host), esc(VERSION))
        },
        what = what.join(" · "),
    )
}

/// A shield with a keyhole: the VPN mark (ours, drawn for the agent's page; `currentColor`).
const VPN_ICON: &str = r#"<svg class=ico viewBox="0 0 16 16" aria-hidden="true"><path fill="currentColor" d="M8 1 2 3.2v4.1c0 3.5 2.5 6.6 6 7.7 3.5-1.1 6-4.2 6-7.7V3.2L8 1zm0 4.2a1.6 1.6 0 0 1 .8 3v2.4H7.2V8.2A1.6 1.6 0 0 1 8 5.2z"/></svg>"#;

/// The hub's address as the agent's page has it (`host:port`): the tunnel's far end (#194).
fn hub_target(devices: &[Arc<Agent>]) -> String {
    let s = devices[0].settings();
    format!("{}:{}", s.hostname, if s.port == 0 { 80 } else { s.port })
}

/// `/wireguard`: `action=save` keeps the fields (while off); `on=1` / `on=0` turns it on or off.
/// Either way every device's socket is closed, so it dials again through the tunnel or not.
fn set_wireguard(
    wg: &wireguard::Wg,
    devices: &[Arc<Agent>],
    form: &std::collections::HashMap<String, String>,
) {
    let mut s = wg.settings();
    if form.get("action").is_some_and(|a| a == "save") {
        let f = |k: &str| form.get(k).map(|v| v.trim().to_owned()).unwrap_or_default();
        s.endpoint = f("endpoint");
        s.peer_public_key = f("public_key");
        s.address = f("address");
        s.preshared_key = f("preshared_key");
        s.keepalive = f("keepalive");
        wg.save(s);
        return;
    }
    let on = form.get("on").is_some_and(|v| v == "1");
    s.on = on;
    wg.save(s);
    if on {
        wg.start(&hub_target(devices));
    } else {
        wg.stop();
    }
    for d in devices {
        if let Some(ws) = d.ws.lock().unwrap().clone() {
            ws.disconnect();
        }
    }
}

/// The WireGuard card's pill (ours, the agent's page only).
fn wg_state(wg: &wireguard::Wg) -> (&'static str, &'static str) {
    let on = wg.settings().on;
    let running = wireguard::local_hub().is_some();
    let handshake = wg.numbers().and_then(|n| n.since_handshake);
    match (on, running, handshake) {
        (false, _, _) => ("Off", "idle"),
        (true, true, Some(_)) => ("Connected", "wait"),
        (true, true, None) => ("No handshake yet", "warn"),
        (true, false, _) => ("Not running", "bad"),
    }
}

/// The tunnel's numbers for the card: boringtun's `Tunn::stats`.
fn wg_numbers(wg: &wireguard::Wg) -> String {
    match wg.numbers() {
        None => "—".into(),
        Some(n) => format!(
            "last handshake {} · {} sent · {} received",
            n.since_handshake
                .map_or("none yet".to_owned(), |d| format!("{} s ago", d.as_secs())),
            wg_bytes(n.sent),
            wg_bytes(n.received)
        ),
    }
}

fn wg_bytes(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1} MB", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1} kB", n as f64 / 1e3)
    } else {
        format!("{n} B")
    }
}

/// The WireGuard card (#194): Turn on / Turn off as every card; the fields while off; this agent's
/// public key to put in the server as a peer.
fn wg_card(wg: &wireguard::Wg) -> String {
    let s = wg.settings();
    let on = s.on;
    let (state, class) = wg_state(wg);
    let field = |name: &str, label: &str, value: &str, hint: &str| {
        format!(
            r#"<label class=field>{label}<input name={name} value="{v}" placeholder="{hint}"{ro} autocomplete=off></label>"#,
            v = esc(value),
            ro = if on { " readonly" } else { "" },
        )
    };
    let save = if on {
        String::new()
    } else {
        r#"<div class="row btns"><span class=fw><button class=btn>Save</button></span></div>"#
            .to_owned()
    };
    format!(
        r#"<div class="card{off}"><div class=head><h2>WireGuard</h2><span class="st {class}" data-live="wg.state">{state}</span></div>
<p class=muted>Reach the hub through a WireGuard tunnel of the agent's own, with no VPN app (#194). The hub's address on this page is then its address on the far side.</p>
<dl class=kv><dt>Tunnel</dt><dd data-live="wg.status">{status}</dd><dt>Numbers</dt><dd data-live="wg.numbers">{numbers}</dd>
<dt>This agent's public key</dt><dd><code id=wg-pub>{public}</code> <button type=button class="btn small" onclick="navigator.clipboard&amp;&amp;navigator.clipboard.writeText(document.getElementById('wg-pub').textContent);this.textContent='Copied'">Copy</button></dd></dl>
<form method=post action=/wireguard><input type=hidden name=action value=save>
<div class=row>{endpoint}{public_key}</div><div class=row>{address}{keepalive}</div><div class=row>{preshared}</div>{save}</form>
<div class="row btns"><form method=post action=/wireguard><input type=hidden name=on value="{toggle}"><span class=fw><button class="btn{toggle_class}">{button}</button></span></form></div></div>"#,
        off = if on { "" } else { " is-off" },
        status = esc(&wg.status()),
        numbers = esc(&wg_numbers(wg)),
        public = esc(&s.public_key()),
        endpoint = field(
            "endpoint",
            "Endpoint (the server)",
            &s.endpoint,
            "home.example.net:13231"
        ),
        public_key = field(
            "public_key",
            "The server's public key",
            &s.peer_public_key,
            "base64"
        ),
        address = field(
            "address",
            "This agent's tunnel address",
            &s.address,
            "10.10.10.2"
        ),
        keepalive = field(
            "keepalive",
            "Keepalive, seconds (optional)",
            &s.keepalive,
            "none"
        ),
        preshared = field(
            "preshared_key",
            "Pre-shared key (optional)",
            &s.preshared_key,
            "base64"
        ),
        toggle = if on { "0" } else { "1" },
        toggle_class = if on { "" } else { " primary" },
        button = if on { "Turn off" } else { "Turn on" },
    )
}

/// What the Stream Deck shows: each device as its card on this page has it, the hub's link.
fn deck_snapshot(devices: &[Arc<Agent>]) -> deck::Snapshot {
    let (hub_text, hub_class) = hub_state(devices);
    deck::Snapshot {
        host: maker_connect_system::computer().host.unwrap_or_default(),
        devices: devices
            .iter()
            .map(|d| {
                let c = card_live(d);
                deck::DeviceSnap {
                    short: match d.kind {
                        Kind::Pnp => "PnP",
                        Kind::Lw450 => "LW450",
                        Kind::Lw320 => "LW320",
                        Kind::Lw400 => "LW400",
                        Kind::Marlin => "Marlin",
                        Kind::Laser => "Laser",
                        Kind::Cameras => "Cams",
                    },
                    title: d.kind.title(),
                    state: c.state,
                    class: c.class,
                    status: c.status,
                    hub: c.hub,
                    registered: c.registered,
                }
            })
            .collect(),
        hub: (hub_text.to_owned(), hub_class, hub_devices(devices)),
    }
}

/// The Stream Deck's pill (ours, the agent's page only).
fn deck_state(deck: &deck::Deck) -> (&'static str, &'static str) {
    match (deck.on(), deck.connected()) {
        (false, _) => ("Off", "idle"),
        (true, true) => ("Connected", "wait"),
        (true, false) => ("Not connected", "warn"),
    }
}

/// The Stream Deck's card: local to the agent, not at the hub (no registration, no code: part 10
/// of the SDK extension is still to be written); Turn on / Turn off as every card (#191).
fn deck_card(deck: &deck::Deck) -> String {
    let on = deck.on();
    let (state, class) = deck_state(deck);
    format!(
        r#"<div class="card{off}"><div class=head><h2>Stream Deck</h2><span class="st {class}" data-live="deck.state">{state}</span></div>
<p class=muted>Quick management and overview · USB · on this agent only, not at the hub yet (idea, work in progress)</p><div></div>
<dl class=kv><dt>Deck</dt><dd data-live="deck.status">{status}</dd><dt>Shows</dt><dd>this agent's devices, read only</dd></dl><div></div>
<div class="row btns"><form method=post action=/deck><input type=hidden name=on value="{toggle}"><span class=fw><button class="btn{toggle_class}">{button}</button></span></form></div></div>"#,
        off = if on { "" } else { " is-off" },
        status = esc(&deck.status()),
        toggle = if on { "0" } else { "1" },
        toggle_class = if on { "" } else { " primary" },
        button = if on { "Turn off" } else { "Turn on" },
    )
}

/// How many of the agent's devices are turned on.
fn devices_on(devices: &[Arc<Agent>]) -> usize {
    devices.iter().filter(|d| d.settings().on).count()
}

/// The Devices tab: the cards as tiles, and the tiles in a row share their rows (subgrid): head,
/// what, the code, the link, the serial port, the buttons; so the same things stand at the same
/// height (owner, 2026-11-23: "allerlei info op verschillende hoogtes", "de tegels zijn juist
/// goed").
fn page(devices: &[Arc<Agent>]) -> String {
    let mut cards: Vec<String> = devices
        .iter()
        .filter(|d| d.kind != Kind::Cameras)
        .map(|d| device_card(d))
        .collect();
    if let Some(deck) = DECK.get() {
        cards.push(deck_card(deck));
    }
    let main = format!("<div class=\"grid devices\">\n{}\n</div>", cards.join("\n"));
    layout(devices, "Devices", &main)
}

/// The Cameras tab (owner, 2026-11-23: "cameras horen in de agent in een eigen tab"): the cameras
/// device's card. A camera is set up in the hub (Space → Cameras), not here.
fn cameras_page(devices: &[Arc<Agent>]) -> String {
    let cards: String = devices
        .iter()
        .filter(|d| d.kind == Kind::Cameras)
        .map(|d| device_card(d))
        .collect();
    let main = format!(
        "<div class=\"grid devices\">\n{cards}\n</div>\n<p class=muted>A camera is set up in the hub, under Space → Cameras.</p>"
    );
    layout(devices, "Cameras", &main)
}

/// The Settings tab (owner, 2026-11-23: "WireGuard en Hub IP horen in een eigen setting tab"): the
/// hub's address, and WireGuard (#194).
fn settings_page(devices: &[Arc<Agent>]) -> String {
    // The hub's address is the same in every device's settings file (the form writes all).
    let s = devices[0].settings();
    let (hub_state, hub_class) = hub_state(devices);
    let devices_text = hub_devices(devices);
    // the address as entered (not the tunnel's local port while WireGuard runs)
    let address = if s.hostname.is_empty() {
        "not set".to_owned()
    } else {
        let plain = format!(
            "{}{}{}",
            if s.tls { "https://" } else { "http://" },
            esc(&s.hostname),
            if s.port == 0 {
                String::new()
            } else {
                format!(":{}", s.port)
            }
        );
        if wireguard::local_hub().is_some() {
            format!("{plain} · through WireGuard")
        } else {
            plain
        }
    };
    let wg = WG.get().map(|wg| wg_card(wg)).unwrap_or_default();
    let main = format!(
        r#"<div class=solo-stack>
<div class=card><div class=head><h2>Hub</h2><span class="st {hub_class}" data-live="hub.state">{hub_state}</span></div>
<dl class=kv><dt>Address</dt><dd>{address}</dd><dt>Devices</dt><dd data-live="hub.devices">{devices_text}</dd></dl>
<details{open} style="margin-top:var(--s-3)"><summary>Change the address</summary><form method=post action=/hub style="margin-top:var(--s-3)">
<div class=row><label class=field>Address<input name=hostname value="{host}" placeholder="192.168.60.67"></label>
<label class=field style="flex:0 1 7rem">Port<input name=port inputmode=numeric value="{port}" placeholder="8460"></label></div>
<label class=check><input type=checkbox name=tls {tls}> TLS</label>
<div class="row btns"><span class=fw><button class="btn primary">Save</button></span></div></form></details></div>
{wg}
</div>"#,
        open = if s.hostname.is_empty() { " open" } else { "" },
        host = esc(&s.hostname),
        port = if s.port == 0 {
            String::new()
        } else {
            s.port.to_string()
        },
        tls = if s.tls { "checked" } else { "" },
    );
    layout(devices, "Settings", &main)
}

/// The About tab (mockup `agent.html#about`): the agent, its load, the computer; the same rows as
/// the hub's About (crate `maker-connect-system`; ours, UI only). What a system does not tell
/// is left out.
fn about_page(devices: &[Arc<Agent>]) -> String {
    let (computer, load, me) = (
        maker_connect_system::computer(),
        maker_connect_system::load(),
        maker_connect_system::process(),
    );
    let kv = |rows: Vec<(&str, String)>| -> String {
        rows.into_iter()
            .map(|(k, v)| format!("<dt>{k}</dt><dd>{}</dd>", esc(&v)))
            .collect()
    };
    let mut agent = vec![("Release", VERSION.to_owned())];
    if let Some(running) = me.running {
        agent.push(("Agent up", maker_connect_system::duration(running)));
    }
    if let Some(memory) = me.memory {
        agent.push(("Agent memory", maker_connect_system::bytes(memory)));
    }
    let mut system = maker_connect_system::computer_rows(&computer);
    if let Some(up) = load.up {
        system.push(("Computer up", maker_connect_system::duration(up)));
    }
    let load_rows = maker_connect_system::load_rows(&load);
    let load_card = if load_rows.is_empty() {
        String::new()
    } else {
        format!(
            "<div class=card><h2>Load</h2><dl class=kv>{}</dl></div>",
            kv(load_rows)
        )
    };
    let main = format!(
        r#"<div class=cols-even>
<div class=card><h2>This agent</h2><dl class=kv>{agent}</dl></div>
{load_card}
<div class=card><h2>Computer</h2><dl class=kv>{system}</dl></div>
</div>"#,
        agent = kv(agent),
        system = kv(system),
    );
    layout(devices, "About", &main)
}

/// The hub's stylesheet and logo mark, so the agent's page is in the house style.
const CSS: &str = include_str!("../../hub/src/web/mc.css");
const MARK: &str = include_str!("../../../assets/logo-v2-mark.svg");
/// The hub's theme switch (Auto, Light, Dark), as its pages have it (`crates/hub/src/web/shell.rs`):
/// the choice before the first paint, and the button's script.
const THEME_FIRST: &str = r#"<script>try{var t=localStorage.getItem("mc-theme");if(t==="light"||t==="dark")document.documentElement.dataset.theme=t;}catch(e){}</script>"#;
const THEME_SWITCH: &str = include_str!("../../hub/src/theme.js");

fn parse_form(body: &str) -> std::collections::HashMap<String, String> {
    body.split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            Some((decode(k), decode(v)))
        })
        .collect()
}

fn decode(s: &str) -> String {
    let s = s.replace('+', " ");
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct Logger;
static LOGGER: Logger = Logger;
impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("{} {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mc-agent-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// The agent's id is drawn once, kept in `agent.ini`, and drawn anew only when it is not one.
    #[test]
    fn the_agent_id_is_kept() {
        let root = dir();
        let id = load_agent_id(&root);
        assert_eq!(id.len(), 32);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(
            std::fs::read_to_string(root.join("agent.ini")).unwrap(),
            format!("[agent]\nid = {id}\n")
        );
        assert_eq!(load_agent_id(&root), id);
        std::fs::write(root.join("agent.ini"), "[agent]\nid = nope\n").unwrap();
        let new = load_agent_id(&root);
        assert!(new != id && new.len() == 32);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A Marlin's connection as chosen on the page is kept in `[connection]`; AUTO is empty and 0.
    #[test]
    fn the_marlin_connection_is_kept() {
        let dir = dir();
        let marlin = Kind::Marlin.printer_type();
        let mut s = Settings::load(&dir, marlin);
        assert_eq!((s.serial_port.as_str(), s.baudrate), ("", 0), "AUTO");
        s.serial_port = "/dev/ttyUSB4".into();
        s.baudrate = 250000;
        s.usb = "1a86:7523 if=0 at=8-1.3".into();
        s.save(&dir);
        assert!(
            std::fs::read_to_string(Settings::path(&dir))
                .unwrap()
                .ends_with(
                    "\n[connection]\nport = /dev/ttyUSB4\nbaudrate = 250000\nusb = 1a86:7523 if=0 at=8-1.3\n"
                )
        );
        let back = Settings::load(&dir, marlin);
        assert_eq!(
            (back.serial_port.as_str(), back.baudrate, back.usb.as_str()),
            ("/dev/ttyUSB4", 250000, "1a86:7523 if=0 at=8-1.3")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `[octoprint]` is read, kept on save, and given to OctoPrint as its config.yaml would be.
    #[test]
    fn the_octoprint_settings_are_kept() {
        let dir = dir();
        let marlin = Kind::Marlin.printer_type();
        Settings::load(&dir, marlin);
        let text = std::fs::read_to_string(Settings::path(&dir)).unwrap()
            + "\n[octoprint]\nserial.sdCancelCommand = M524\nserial.timeout.communication = 30\n";
        std::fs::write(Settings::path(&dir), text).unwrap();
        let mut s = Settings::load(&dir, marlin);
        s.on = true;
        s.save(&dir);
        let back = Settings::load(&dir, marlin);
        assert_eq!(
            back.octoprint,
            [
                ("serial.sdCancelCommand".to_owned(), "M524".to_owned()),
                ("serial.timeout.communication".to_owned(), "30".to_owned())
            ]
        );
        assert_eq!(
            octoprint_config(&back.octoprint),
            json!({"serial": {"sdCancelCommand": "M524", "timeout": {"communication": 30}}})
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A device with no hub address gets the one another device has; one with an address keeps it.
    #[test]
    fn a_new_device_takes_the_agents_hub() {
        let root = dir();
        let pnp = Kind::Pnp.printer_type();
        let lw = Kind::ALL
            .iter()
            .find(|k| **k != Kind::Pnp)
            .unwrap()
            .printer_type();
        let mut linked = Settings::load(&root.join("a"), pnp);
        linked.hostname = "192.168.60.67".into();
        linked.port = 8460;
        let mut devices = vec![
            (root.join("a"), linked),
            (root.join("b"), Settings::load(&root.join("b"), lw)),
        ];
        share_hub(&mut devices);
        let back = Settings::load(&root.join("b"), lw);
        assert_eq!(
            (back.hostname.as_str(), back.port, back.tls),
            ("192.168.60.67", 8460, false)
        );
        assert!(back.token.is_empty(), "registers itself");
    }

    /// The settings file is PrusaLink's `prusa_printer_settings.ini` as configparser writes it
    /// (`key = value`, Python's `True`/`False`), read back as written; a serial is drawn once.
    #[test]
    fn the_settings_file_as_prusalink_keeps_it() {
        let dir = dir();
        let pnp = Kind::Pnp.printer_type();
        let drawn = Settings::load(&dir, pnp);
        assert!(drawn.serial.starts_with("MC") && drawn.serial.len() == 18);
        assert_eq!(Settings::load(&dir, pnp).serial, drawn.serial, "drawn once");
        let s = Settings {
            printer_type: pnp,
            hostname: "192.168.60.67".into(),
            tls: false,
            port: 8460,
            token: "t0k3n".into(),
            serial: "MC0011223344556677".into(),
            on: true,
            ..Settings::default()
        };
        s.save(&dir);
        assert_eq!(
            std::fs::read_to_string(Settings::path(&dir)).unwrap(),
            "[printer]\ntype = dymo.labelmanager-pnp\nserial = MC0011223344556677\n\n\
             [service::connect]\nhostname = 192.168.60.67\ntls = False\nport = 8460\ntoken = t0k3n\n\n\
             [module]\non = True\n"
        );
        let back = Settings::load(&dir, pnp);
        assert_eq!(
            (
                back.hostname.as_str(),
                back.tls,
                back.port,
                back.token.as_str(),
                back.serial.as_str(),
                back.on
            ),
            (
                "192.168.60.67",
                false,
                8460,
                "t0k3n",
                "MC0011223344556677",
                true
            )
        );
        assert_eq!(back.server().as_deref(), Some("http://192.168.60.67:8460"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(Settings::path(&dir))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the token is not for others");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `make_fingerprint` (`prusa/link/util.py:153-159`): `sha256(sn.encode()).hexdigest()`,
    /// the value CPython 3.13 gives for this serial.
    #[test]
    fn the_fingerprint_is_prusalinks() {
        let s = Settings {
            serial: "MC0011223344556677".into(),
            ..Settings::default()
        };
        assert_eq!(
            s.fingerprint(),
            "0ca5a92567ae13a3864ac94beb9bca2997046b9a509fd138edbf6dc98fb06ed1"
        );
    }

    /// No hub address, no server; otherwise the address as the SDK makes it
    /// (`Printer.connect_url`, `__init__.py:189-205`).
    #[test]
    fn the_server_from_the_settings() {
        assert_eq!(Settings::default().server(), None);
        let url = |host: &str, tls: bool, port: u16| {
            Settings {
                hostname: host.into(),
                tls,
                port,
                ..Settings::default()
            }
            .server()
            .unwrap()
        };
        // Its doctests.
        assert_eq!(url("connect", true, 0), "https://connect");
        assert_eq!(url("connect", false, 0), "http://connect");
        assert_eq!(url("connect", false, 8000), "http://connect:8000");
    }

    /// The DYMO's telemetry: what the port read goes out under the names the hub reads; what it
    /// did not read is not sent.
    #[test]
    fn the_dymo_tells_what_the_port_read() {
        let t = maker_connect_protocol::Telemetry {
            cassette: Some(true),
            battery_percent: Some(80),
            raw_status: Some("00 00".into()),
            ..Default::default()
        };
        assert_eq!(
            Value::Object(dymo_telemetry(&t)),
            json!({"media_loaded": true, "battery_percent": 80, "raw_status": "00 00"})
        );
        assert!(dymo_telemetry(&maker_connect_protocol::Telemetry::default()).is_empty());
    }

    /// The LabelWriter during a job: its state by the driver's keyword (part 3).
    #[test]
    fn the_labelwriter_tells_the_drivers_state() {
        let t = |kw: Option<&str>| maker_connect_protocol::Telemetry {
            state_reason: kw.map(str::to_owned),
            ..Default::default()
        };
        assert_eq!(
            lw450_state(&t(Some("com.dymo.out-of-paper-error"))),
            State::Attention
        );
        assert_eq!(
            lw450_state(&t(Some("com.dymo.general-error"))),
            State::Error
        );
        assert_eq!(lw450_state(&t(None)), State::Printing);
    }

    /// Each device its own folder and type: the PnP keeps `dymo/`.
    #[test]
    fn each_device_its_own_settings_file() {
        assert_eq!(
            Kind::ALL.map(|k| (k.folder(), k.printer_type())),
            [
                ("dymo", "dymo.labelmanager-pnp"),
                ("labelwriter-450", "dymo.labelwriter-450"),
                ("labelwriter-320", "dymo.labelwriter-320"),
                ("labelwriter-400", "dymo.labelwriter-400"),
                ("marlin", "marlin.fdm"),
                ("laser", "grbl.laser"),
                ("cameras", "maker-connect.cameras"),
            ]
        );
    }

    /// A form as the page posts it, `+` and `%XX` decoded.
    #[test]
    fn the_pages_form() {
        let form = parse_form("hostname=hub+one&port=8460&tls=on&x=%3C%26%3E");
        assert_eq!(form["hostname"], "hub one");
        assert_eq!(form["port"], "8460");
        assert_eq!(form["x"], "<&>");
    }

    /// The SDK's queue (`__init__.py:319-339`, `858-905`): an event waits in it while there is no
    /// socket, and goes in its turn; with no token or no server it is skipped.
    /// Turned off and on again (Turn off, Turn on), a device keeps its socket: the
    /// next send dials it again.
    #[test]
    fn a_device_turned_off_and_on_keeps_its_socket() {
        let dir = std::env::temp_dir().join(format!("mc-agent-onoff-{}", std::process::id()));
        let agent = Arc::new(Agent::new(
            Kind::Pnp,
            dir.clone(),
            Settings::load(&dir, Kind::Pnp.printer_type()),
            Arc::new(Mutex::new(Engine::new())),
        ));
        setup_link(&agent);
        let ws = agent.ws.lock().unwrap().clone().expect("set up");
        agent.module_off();
        let kept = agent.ws.lock().unwrap().clone().expect("kept");
        assert!(Arc::ptr_eq(&ws, &kept));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_event_waits_for_the_socket() {
        let dir = std::env::temp_dir().join(format!("mc-agent-queue-{}", std::process::id()));
        let mut settings = Settings::load(&dir, Kind::Marlin.printer_type());
        settings.hostname = "127.0.0.1".into();
        settings.port = 8460;
        settings.token = "t".into();
        let agent = Arc::new(Agent::new(
            Kind::Marlin,
            dir.clone(),
            settings,
            Arc::new(Mutex::new(Engine::new())),
        ));
        assert!(!loop_step(&agent), "nothing queued");
        agent
            .queue
            .lock()
            .unwrap()
            .push_back(json!({"event": "INFO"}));
        agent
            .queue
            .lock()
            .unwrap()
            .push_back(json!({"event": "FINISHED"}));
        // No socket yet: the first is put back, behind the second.
        assert!(loop_step(&agent));
        let names = |a: &Agent| -> Vec<String> {
            a.queue
                .lock()
                .unwrap()
                .iter()
                .map(|e| e["event"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(names(&agent), ["FINISHED", "INFO"]);
        // No token: skipped.
        agent.settings.lock().unwrap().token.clear();
        assert!(loop_step(&agent));
        assert_eq!(names(&agent), ["INFO"]);
        // `event_cb` puts into the queue (it sends nothing itself).
        agent.settings.lock().unwrap().token = "t".into();
        info_changed(&agent);
        assert!(agent.event_cb.lock().unwrap().is_none(), "no link set up");
        // Attaching the storage sends MEDIUM_INSERTED (`Filesystem.attach`, `files.py`).
        setup_link(&agent);
        folder_update(&agent).unwrap();
        info_changed(&agent);
        assert_eq!(names(&agent), ["INFO", "MEDIUM_INSERTED", "INFO"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #237: `INFO` carries `fs.storages()` (`__init__.py:413`): a Marlin's `gcodes`, attached
    /// by `setup_link`; a laser has none.
    #[test]
    fn info_names_the_storages() {
        for (kind, expected) in [(Kind::Marlin, vec!["gcodes"]), (Kind::Laser, vec![])] {
            let dir = std::env::temp_dir().join(format!(
                "mc-agent-storages-{}-{}",
                kind.printer_type(),
                std::process::id()
            ));
            let settings = Settings::load(&dir, kind.printer_type());
            let agent = Arc::new(Agent::new(
                kind,
                dir.clone(),
                settings,
                Arc::new(Mutex::new(Engine::new())),
            ));
            setup_link(&agent);
            folder_update(&agent).unwrap();
            let info = send_info(&agent).unwrap().kwargs;
            let storages = info["storages"].as_array().unwrap();
            let names: Vec<&str> = storages
                .iter()
                .map(|s| s["name"].as_str().unwrap())
                .collect();
            assert_eq!(names, expected);
            if let Some(s) = storages.first() {
                assert_eq!(s["mountpoint"], "/gcodes");
                assert_eq!(s["type"], "LOCAL");
                assert_eq!(s["read_only"], false);
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// #237: the SDK's file handlers (`__init__.py:526-595`) on a Marlin's `gcodes`, each answer
    /// as the SDK's `Command` sends it.
    #[test]
    fn the_file_handlers() {
        use maker_connect_sdk::consts::Event;
        let dir = std::env::temp_dir().join(format!("mc-agent-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let gcodes = dir.join("gcodes");
        std::fs::create_dir_all(gcodes.join("box/inner")).unwrap();
        std::fs::write(gcodes.join("a.gcode"), "G28\n").unwrap();
        std::fs::write(gcodes.join("box/b.gcode"), "G28\n").unwrap();
        let settings = Settings::load(&dir, Kind::Marlin.printer_type());
        let agent = Arc::new(Agent::new(
            Kind::Marlin,
            dir.clone(),
            settings,
            Arc::new(Mutex::new(Engine::new())),
        ));
        let quiet: EventCallback = Arc::new(|_, _, _, _, _| {});
        setup_storage(&agent, &quiet);
        folder_update(&agent).unwrap();
        type Sent = Arc<Mutex<Vec<(Event, Kwargs)>>>;
        let sent: Sent = Default::default();
        let to = sent.clone();
        let command = Command::new(Arc::new(move |event, _, _, _, kwargs| {
            to.lock().unwrap().push((event, kwargs));
        }));
        setup_files(&agent, &command);
        let mut id = 0;
        let mut run = |name: &str, kwargs: Option<Value>| {
            id += 1;
            command.accept(id, name, None, kwargs, false);
            command.run();
            sent.lock().unwrap().pop().unwrap()
        };
        // FAILED carries `error=repr(err)`, REJECTED its `reason` (`command.py:183-198`).
        let reason = |kwargs: &Kwargs| {
            kwargs
                .get("error")
                .or(kwargs.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };

        // SEND_FILE_INFO
        let (event, kwargs) = run("SEND_FILE_INFO", Some(json!({"path": "/gcodes/a.gcode"})));
        assert_eq!(event, Event::FileInfo);
        assert_eq!(kwargs["path"], "/gcodes/a.gcode");
        let (event, kwargs) = run("SEND_FILE_INFO", None);
        assert_eq!(event, Event::Failed);
        assert_eq!(
            reason(&kwargs),
            "ValueError('SEND_FILE_INFO requires kwargs')"
        );
        let (event, kwargs) = run("SEND_FILE_INFO", Some(json!({"path": "/gcodes/none"})));
        assert_eq!(
            (event, reason(&kwargs)),
            (Event::Rejected, "File not found".into())
        );

        // CREATE_FOLDER, CREATE_DIRECTORY
        let (event, _) = run("CREATE_FOLDER", Some(json!({"path": "/gcodes/new/deep"})));
        assert_eq!(event, Event::Finished);
        assert!(gcodes.join("new/deep").is_dir());
        let (event, _) = run("CREATE_DIRECTORY", Some(json!({"path": "/gcodes/new"})));
        assert_eq!(event, Event::Finished, "exist_ok");
        let (event, kwargs) = run("CREATE_FOLDER", Some(json!({"path": 5})));
        assert_eq!(
            (event, reason(&kwargs)),
            (
                Event::Failed,
                "TypeError('expected str, bytes or os.PathLike object, not int')".into()
            )
        );

        // DELETE_FILE: not while it is printed, the job IN_PROGRESS (`prusa_link.py:512-516`).
        {
            let mut job = agent.job.lock().unwrap();
            job.select("/gcodes/a.gcode", 4, None, Some(1));
            job.state_changed(State::Printing);
        }
        let (event, kwargs) = run("DELETE_FILE", Some(json!({"path": "/gcodes/a.gcode"})));
        assert_eq!(
            (event, reason(&kwargs)),
            (
                Event::Failed,
                "ValueError('This file is currently printed')".into()
            )
        );
        let (event, kwargs) = run("DELETE_FOLDER", Some(json!({"path": "/gcodes"})));
        assert_eq!(
            (event, reason(&kwargs)),
            (
                Event::Failed,
                "ValueError('The file inside of this folder is currently printed')".into()
            )
        );
        // FINISHED: the job is ENDING, no longer IN_PROGRESS, so the file may go.
        agent.job.lock().unwrap().state_changed(State::Finished);
        assert_eq!(printed_file(&agent), None);
        let (event, _) = run("DELETE_FILE", Some(json!({"path": "/gcodes/a.gcode"})));
        assert_eq!(event, Event::Finished);
        assert!(!gcodes.join("a.gcode").exists());
        // Not in the tree (nothing watched it in): `None.to_dict()`.
        let (event, kwargs) = run("DELETE_FILE", Some(json!({"path": "/gcodes/zz"})));
        assert_eq!(
            (event, reason(&kwargs)),
            (
                Event::Failed,
                "AttributeError(\"'NoneType' object has no attribute 'to_dict'\")".into()
            )
        );
        let (event, kwargs) = run("DELETE_FILE", Some(json!({"nopath": 1})));
        assert_eq!(
            (event, reason(&kwargs)),
            (
                Event::Failed,
                "ValueError('DELETE_FILE requires kwargs')".into()
            )
        );

        // DELETE_FOLDER, DELETE_DIRECTORY: `os.rmdir` unless `force`.
        let (event, kwargs) = run("DELETE_FOLDER", Some(json!({"path": "/gcodes/box"})));
        assert_eq!(event, Event::Failed);
        // `repr` of the `OSError` of `os.rmdir`: ENOTEMPTY and the OS's text. On Windows
        // ERROR_DIR_NOT_EMPTY is ENOTEMPTY, 41 (`PC/errmap.h:126`), its text without the dot
        // (`Python/errors.c:964-965`).
        #[cfg(target_os = "linux")]
        let expected = "OSError(39, 'Directory not empty')";
        #[cfg(target_os = "macos")]
        let expected = "OSError(66, 'Directory not empty')";
        #[cfg(windows)]
        let expected = "OSError(41, 'The directory is not empty')";
        assert_eq!(reason(&kwargs), expected);
        let (event, _) = run(
            "DELETE_DIRECTORY",
            Some(json!({"path": "/gcodes/box", "force": true})),
        );
        assert_eq!(event, Event::Finished);
        assert!(!gcodes.join("box").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #237: `FolderStorage.update` each `DIR_RESCAN_INTERVAL` (`storage.py:65-83`, `:255-279`):
    /// a folder gone is made again and stays attached; one that cannot be read is detached
    /// (`MEDIUM_EJECTED`), and attached again once it can (`MEDIUM_INSERTED`).
    #[test]
    fn the_folder_is_looked_at_again() {
        use maker_connect_sdk::consts::Event;
        let dir = std::env::temp_dir().join(format!("mc-agent-rescan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = Settings::load(&dir, Kind::Marlin.printer_type());
        let agent = Arc::new(Agent::new(
            Kind::Marlin,
            dir.clone(),
            settings,
            Arc::new(Mutex::new(Engine::new())),
        ));
        let events: Arc<Mutex<Vec<Event>>> = Default::default();
        let to = events.clone();
        let event_cb: EventCallback = Arc::new(move |event, _, _, _, _| {
            to.lock().unwrap().push(event);
        });
        let fs = Arc::new(Filesystem::new("/", Some(event_cb)));
        *agent.fs.lock().unwrap() = Some(fs.clone());
        let names = |fs: &Filesystem| -> Vec<String> {
            fs.storages()
                .unwrap()
                .iter()
                .map(|s| s["name"].as_str().unwrap().to_owned())
                .collect()
        };
        folder_update(&agent).unwrap();
        assert_eq!(names(&fs), ["gcodes"]);
        assert_eq!(*events.lock().unwrap(), [Event::MediumInserted]);

        std::fs::remove_dir_all(dir.join("gcodes")).unwrap();
        folder_update(&agent).unwrap();
        assert!(dir.join("gcodes").is_dir(), "ensure_directory");
        assert_eq!(names(&fs), ["gcodes"]);
        assert_eq!(events.lock().unwrap().len(), 1, "no event: still attached");

        // `os.access(R_OK)` of a folder that exists is always true on Windows.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let gcodes = dir.join("gcodes");
            std::fs::set_permissions(&gcodes, std::fs::Permissions::from_mode(0o000)).unwrap();
            // root reads it anyway (`os.access` says so), so the rest only for others.
            if !pyos::access(os_path_str(&gcodes).as_bytes(), pyos::R_OK).unwrap() {
                folder_update(&agent).unwrap();
                assert!(names(&fs).is_empty());
                assert_eq!(events.lock().unwrap().last(), Some(&Event::MediumEjected));
            }
            std::fs::set_permissions(&gcodes, std::fs::Permissions::from_mode(0o755)).unwrap();
            folder_update(&agent).unwrap();
            assert_eq!(names(&fs), ["gcodes"]);
            assert_eq!(events.lock().unwrap().last(), Some(&Event::MediumInserted));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #237: `inotify_loop` takes a file put in `gcodes` into the tree and tells Connect
    /// (`FILE_CHANGED`), so its handlers find it.
    #[test]
    fn the_loop_follows_the_folder() {
        use maker_connect_sdk::consts::Event;
        let dir = std::env::temp_dir().join(format!("mc-agent-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = Settings::load(&dir, Kind::Marlin.printer_type());
        let agent = Arc::new(Agent::new(
            Kind::Marlin,
            dir.clone(),
            settings,
            Arc::new(Mutex::new(Engine::new())),
        ));
        let events: Arc<Mutex<Vec<Event>>> = Default::default();
        let to = events.clone();
        let event_cb: EventCallback = Arc::new(move |event, _, _, _, _| {
            to.lock().unwrap().push(event);
        });
        setup_storage(&agent, &event_cb);
        folder_update(&agent).unwrap();
        let me = agent.clone();
        std::thread::spawn(move || inotify_loop(&me));
        std::fs::write(dir.join("gcodes/new.gcode"), "G28\n").unwrap();
        let fs = agent.fs.lock().unwrap().clone().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        // The node goes into the tree first, then FILE_CHANGED (`_process_create`).
        while fs.get("/gcodes/new.gcode").is_none()
            || !events.lock().unwrap().contains(&Event::FileChanged)
        {
            assert!(Instant::now() < deadline, "not followed");
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #69: a device's card says whether the hub takes its telemetry, and since when.
    #[test]
    fn the_card_says_the_link_to_the_hub() {
        let dir = std::env::temp_dir().join(format!("mc-agent-card-{}", std::process::id()));
        let mut settings = Settings::load(&dir, Kind::Lw450.printer_type());
        settings.token = "t".into();
        settings.on = true;
        let agent = Agent::new(
            Kind::Lw450,
            dir,
            settings,
            Arc::new(Mutex::new(Engine::new())),
        );
        let card = device_card(&agent);
        assert!(card.contains(">Not connected</span>"), "{card}");
        assert!(
            card.contains("<dt>Hub</dt><dd data-live=\"labelwriter-450.hub\">not connected</dd>"),
            "{card}"
        );
        agent.hub_seen(true);
        let card = device_card(&agent);
        assert!(card.contains(">Connected</span>"), "{card}");
        assert!(
            card.contains("<dt>Hub</dt><dd data-live=\"labelwriter-450.hub\">connected for 0 min · last contact now</dd>"),
            "{card}"
        );
        agent.hub_seen(false);
        let card = device_card(&agent);
        assert!(
            card.contains("<dt>Hub</dt><dd data-live=\"labelwriter-450.hub\">not connected · last contact 0 min ago</dd>"),
            "{card}"
        );
        // `/events`: the same text, and the page's shape; the shape changes with the card's
        // make-up (here: turned off), not with its text.
        let agent = Arc::new(agent);
        let devices = [agent.clone()];
        let now: serde_json::Value = serde_json::from_str(&live(&devices)).unwrap();
        assert_eq!(
            now["live"]["labelwriter-450.hub"],
            "not connected · last contact 0 min ago"
        );
        assert_eq!(now["live"]["labelwriter-450.state"]["class"], "warn");
        assert_eq!(now["live"]["hub.devices"], "0 of 1 connected");
        let before = now["shape"].clone();
        agent.hub_seen(true);
        let now: serde_json::Value = serde_json::from_str(&live(&devices)).unwrap();
        assert_eq!(now["shape"], before);
        agent.settings.lock().unwrap().on = false;
        let now: serde_json::Value = serde_json::from_str(&live(&devices)).unwrap();
        assert_ne!(now["shape"], before);
        agent.settings.lock().unwrap().on = true;
        // The hub's theme switch, the same button and script.
        let page = page(&devices);
        assert!(page.contains(r#"<body data-shape=""#) && page.contains(LIVE_JS));
        assert!(!page.contains("http-equiv=refresh"));
        assert!(
            page.contains(r#"class=theme data-theme-btn aria-label="Theme: Auto">Auto</button>"#)
        );
        assert!(page.contains(THEME_SWITCH) && page.contains("localStorage.getItem(\"mc-theme\")"));
    }

    /// #191: Turn on / Turn off on every device's card, in the same place; a Marlin's or a laser's
    /// port chosen in the Turn on form while it is off, shown while it is on; no Connect or
    /// Disconnect.
    #[test]
    fn turn_on_and_off_on_every_card() {
        for kind in Kind::ALL {
            let dir = std::env::temp_dir().join(format!(
                "mc-agent-toggle-{}-{}",
                kind.folder(),
                std::process::id()
            ));
            let agent = Agent::new(
                kind,
                dir.clone(),
                Settings::load(&dir, kind.printer_type()),
                Arc::new(Mutex::new(Engine::new())),
            );
            let device = kind.folder();
            let card = device_card(&agent);
            assert!(
                card.contains(r#"<input type=hidden name=on value="1">"#),
                "{card}"
            );
            assert!(
                card.contains(&format!(
                    r#"<button class="btn primary" form="module-{device}">Turn on</button>"#
                )),
                "{card}"
            );
            assert_eq!(card.contains("<select name=port>"), kind.serial(), "{card}");
            assert!(
                !card.contains("Connect<") && !card.contains("Disconnect"),
                "{card}"
            );
            assert!(!card.contains("action=/connection"), "{card}");
            // Turned on (a laser with no port chosen, a Marlin on AUTO: the test opens nothing
            // when no port is there)
            agent.settings.lock().unwrap().on = true;
            let card = device_card(&agent);
            assert!(
                card.contains(&format!(
                    r#"<button class="btn" form="module-{device}">Turn off</button>"#
                )),
                "{card}"
            );
            assert!(!card.contains("<select name=port>"), "{card}");
            assert_eq!(
                card.contains("<dt>Serial port</dt>"),
                kind.serial(),
                "{card}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// #191: a Marlin's Turn on keeps the port and baud rate it carries; Turn off keeps them for the
    /// next Turn on; `KEEP` keeps the chosen device.
    #[test]
    fn turn_on_carries_the_port() {
        let dir = std::env::temp_dir().join(format!("mc-agent-port-{}", std::process::id()));
        let mut s = Settings::load(&dir, Kind::Marlin.printer_type());
        choose_port(
            &mut s,
            &parse_form("device=marlin&on=1&port=%2Fdev%2Fnot-there&baudrate=115200"),
        );
        assert_eq!(
            (s.serial_port.as_str(), s.baudrate),
            ("/dev/not-there", 115200)
        );
        choose_port(
            &mut s,
            &parse_form("device=marlin&on=1&port=KEEP&baudrate=250000"),
        );
        assert_eq!(
            (s.serial_port.as_str(), s.baudrate),
            ("/dev/not-there", 115200)
        );
        // Turn off carries no port: nothing changes
        choose_port(&mut s, &parse_form("device=marlin&on=0"));
        assert_eq!(
            (s.serial_port.as_str(), s.baudrate),
            ("/dev/not-there", 115200)
        );
        choose_port(
            &mut s,
            &parse_form("device=marlin&on=1&port=AUTO&baudrate=0"),
        );
        assert_eq!((s.serial_port.as_str(), s.baudrate), ("", 0));
        let agent = Agent::new(
            Kind::Marlin,
            dir.clone(),
            s,
            Arc::new(Mutex::new(Engine::new())),
        );
        assert_eq!(
            port_row(&agent, &agent.settings()),
            "<dt>Serial port</dt><dd>AUTO · AUTO</dd>"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
