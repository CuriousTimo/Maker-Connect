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

#[cfg(unix)]
use maker_connect_engine::Order;
use maker_connect_engine::{
    Engine, MachineId, Port, ports::dymo_cups::LabelWriter450, ports::labelle::DymoPnp,
};
#[cfg(unix)]
use maker_connect_gcode_metadata::py::repr_str;
#[cfg(unix)]
use maker_connect_protocol::JobOrder;
use maker_connect_protocol::MachineState;
#[cfg(unix)]
use maker_connect_sdk::command::HandlerError;
use maker_connect_sdk::command::{Command, CommandHandler, Finish};
#[cfg(unix)]
use maker_connect_sdk::consts::TransferType;
use maker_connect_sdk::consts::{self, Source, State};
#[cfg(unix)]
use maker_connect_sdk::download::{
    ConnDetailsCallback, DownloadFinishedCallback, DownloadMgr, Filesystem, PrintedFileCallback,
    Storage, Transfer,
};
use maker_connect_sdk::models::{EventCallback, Kwargs};
#[cfg(unix)]
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
}

impl Kind {
    const ALL: [Kind; 2] = [Kind::Pnp, Kind::Lw450];

    /// Its type on the wire (`docs/design/prusa-sdk-extension.md`, parts 2 and 3).
    fn printer_type(self) -> &'static str {
        match self {
            Kind::Pnp => "dymo.labelmanager-pnp",
            Kind::Lw450 => "dymo.labelwriter-450",
        }
    }

    /// Its folder under the agent's: its settings file and its labels' storage.
    fn folder(self) -> &'static str {
        match self {
            Kind::Pnp => "dymo",
            Kind::Lw450 => "labelwriter-450",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Kind::Pnp => "DYMO LabelManager PnP",
            Kind::Lw450 => "DYMO LabelWriter 450",
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
            Kind::Lw450 => &[
                "type.string",
                "cups.states",
                "label.page_size",
                "label.stop",
                "agent.id",
            ],
        }
    }

    fn port(self) -> Box<dyn Port> {
        match self {
            Kind::Pnp => Box::new(DymoPnp::new()),
            Kind::Lw450 => Box::new(LabelWriter450::new()),
        }
    }
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
/// ours: `[module] on`.
#[derive(Debug, Clone, Default)]
struct Settings {
    printer_type: &'static str,
    hostname: String,
    tls: bool,
    port: u16,
    token: String,
    serial: String,
    on: bool,
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
#[cfg(unix)]
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
    #[cfg(unix)]
    transfer: Arc<Transfer>,
    #[cfg(unix)]
    download: Mutex<Option<Arc<DownloadMgr>>>,
    /// The tape or page each download's print is for, by its command (`tape_size_mm`,
    /// `page_size`).
    #[cfg(unix)]
    media: Mutex<std::collections::HashMap<i64, Media>>,
    /// The state the last `STATE_CHANGED` said.
    last_state: Mutex<Option<State>>,
    /// The label being printed: its job (the download's command) and file.
    label: Mutex<Option<(i64, PathBuf)>>,
    /// Its link to the hub as the page shows it (ours, UI only).
    hub: Mutex<HubLink>,
}

/// Since when the hub takes this device's telemetry, and when it last did.
#[derive(Default, Clone, Copy)]
struct HubLink {
    since: Option<Instant>,
    last: Option<Instant>,
}

impl Agent {
    #[cfg(unix)]
    fn transfer(&self) -> Arc<Transfer> {
        self.transfer.clone()
    }

    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    fn new(kind: Kind, dir: PathBuf, settings: Settings, engine: Arc<Mutex<Engine>>) -> Agent {
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
            #[cfg(unix)]
            transfer: Arc::new(Transfer::new()),
            #[cfg(unix)]
            download: Mutex::new(None),
            #[cfg(unix)]
            media: Mutex::new(Default::default()),
            last_state: Mutex::new(None),
            hub: Mutex::new(HubLink::default()),
            label: Mutex::new(None),
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

    /// Turn the device's module on: its port in the engine.
    fn module_on(&self) {
        let mut dymo = self.dymo.lock().unwrap();
        if dymo.is_none() {
            *dymo = Some(self.engine.lock().unwrap().add(self.kind.port()));
        }
    }

    fn module_off(&self) {
        if let Some(id) = self.dymo.lock().unwrap().take() {
            let _ = self.engine.lock().unwrap().remove(id);
        }
        if let Some(ws) = self.ws.lock().unwrap().take() {
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
        let agent = agent.clone();
        std::thread::spawn(move || link_loop(&agent));
    }
    println!("maker-connect-agent {VERSION} on http://{listen}");
    web(&devices, &listen);
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
        events.send(&payload);
    });
    *agent.event_cb.lock().unwrap() = Some(event_cb.clone());
    let command = Arc::new(Command::new(event_cb.clone()));
    let me = agent.clone();
    let info: CommandHandler = Arc::new(move |_| Ok(send_info(&me)));
    command.set_handler(consts::Command::SendInfo, info);
    #[cfg(unix)]
    setup_download(agent, &command, event_cb);
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
}

/// The download handlers and manager (`__init__.py:149-154`, `:168-174`), the manager's loop in
/// a thread of its own as PrusaLink runs it (`sdk_augmentation/printer.py:40`, `:205-208`). Its
/// one storage is the labels' folder (ours: `files.py` is not ported, see
/// `maker_connect_sdk::download`). Unix only, as the SDK's storage (`wire.md`).
#[cfg(unix)]
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
    if agent.kind == Kind::Lw450 {
        let me = agent.clone();
        let stop: CommandHandler = Arc::new(move |_| stop_print(&me));
        command.set_handler(consts::Command::StopPrint, stop);
    }

    let mut storage_dict = std::collections::HashMap::new();
    storage_dict.insert(
        "labels".to_owned(),
        Storage {
            path_storage: agent.dir.join("labels").to_string_lossy().into_owned(),
        },
    );
    let me = agent.clone();
    let conn_details: ConnDetailsCallback =
        Arc::new(move || (me.settings().server(), me.headers(true)));
    let me = agent.clone();
    let printed_file: PrintedFileCallback = Arc::new(move || {
        me.label
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, file)| file.to_string_lossy().into_owned())
    });
    let me = agent.clone();
    let finished: DownloadFinishedCallback = Arc::new(move |t| label_downloaded(&me, t));
    let mgr = Arc::new(DownloadMgr::new(
        Filesystem {
            sep: "/".into(),
            storage_dict,
        },
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
#[cfg(unix)]
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

#[cfg(not(unix))]
fn event_transfer_id(_agent: &Agent, _kwargs: &mut Kwargs) {}

/// The transfer's telemetry (`telemetry`, `__init__.py:355-360`); `false` when it raises (a
/// `progress` of size 0), which leaves the telemetry out.
#[cfg(unix)]
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

#[cfg(not(unix))]
fn telemetry_transfer(_agent: &Agent, _kwargs: &mut Kwargs) -> bool {
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

/// `send_info` (`__init__.py:393-434`), as far as this device has it.
fn send_info(agent: &Agent) -> Finish {
    let s = agent.settings();
    let mut finish = Finish::new(Source::Connect);
    finish.event = Some(consts::Event::Info);
    let kwargs: Kwargs = json!({
        "type": agent.kind.printer_type(),
        "firmware": VERSION,
        "sdk": SDK_VERSION,
        "sn": s.serial,
        "fingerprint": s.fingerprint(),
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
    finish
}

#[cfg(unix)]
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
        Kind::Lw450 => (None, kwargs["page_size"].as_str().map(str::to_owned)),
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

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
/// `transfer_id == self.transfer.transfer_id`, Python's `==` of a JSON number and an int.
fn same_transfer(id: &Value, transfer: &Transfer) -> bool {
    match (id.as_f64(), transfer.data().transfer_id) {
        (Some(id), Some(current)) => id == f64::from(current),
        _ => false,
    }
}

#[cfg(unix)]
/// `repr(ValueError(message))`
fn value_error(message: &str) -> String {
    format!("ValueError({})", repr_str(message))
}

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
/// `download_finished_cb(transfer)`, as PrusaLink's (`prusa_link.py:519-540`) starts the print of
/// a download with `to_print`: the label goes to the DYMO port (ours: the port in place of
/// PrusaLink's `StartPrint`), its state `PRINTING` as `set_state` says it.
fn label_downloaded(agent: &Agent, transfer: &Transfer) {
    let d = transfer.data().clone();
    let (tape, page_size) = d
        .start_cmd_id
        .and_then(|id| agent.media.lock().unwrap().remove(&id))
        .unwrap_or_default();
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

/// `set_state(state, source, **kwargs)` (`__init__.py`): the `STATE_CHANGED` event. The source
/// is ours: the device.
fn set_state(agent: &Agent, state: State, reason: Option<&str>) {
    *agent.last_state.lock().unwrap() = Some(state);
    let Some(event_cb) = agent.event_cb.lock().unwrap().clone() else {
        return;
    };
    let mut kwargs = Kwargs::new();
    kwargs.insert("state".into(), state.value().into());
    if let Some(reason) = reason {
        kwargs.insert("reason".into(), reason.into());
    }
    event_cb(
        consts::Event::StateChanged,
        Source::Firmware,
        None,
        None,
        kwargs,
    );
}

fn current_state(agent: &Agent) -> State {
    let report = agent
        .dymo
        .lock()
        .unwrap()
        .and_then(|id| agent.engine.lock().unwrap().report(id).ok());
    if agent.printing.load(Ordering::SeqCst) {
        return match (agent.kind, report) {
            (Kind::Lw450, Some(r)) => lw450_state(&r.telemetry),
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

#[cfg(unix)]
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
        let Some(id) = *agent.dymo.lock().unwrap() else {
            agent.hub_seen(false);
            agent.set_status("the module is off");
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
        if let Some(job) = label
            && let Some(end) = report.job_ends.iter().find(|e| e.id == job).cloned()
        {
            let _ =
                agent
                    .engine
                    .lock()
                    .unwrap()
                    .reported(id, report.done.len(), report.job_ends.len());
            label_ended(agent, end.ok, end.reason);
        } else if label.is_some() && agent.kind == Kind::Lw450 {
            // The driver's job state as it changes, each change a `STATE_CHANGED` (`set_state`).
            let state = lw450_state(&report.telemetry);
            if *agent.last_state.lock().unwrap() != Some(state) {
                // The PPD's own text for the driver's keyword as the event's `reason` (part 3).
                set_state(agent, state, report.telemetry.problem.as_deref());
            }
        }
        // Gone: nothing is sent, the socket ends and the hub sees it away (`wire.md`).
        if !report.connected {
            agent.hub_seen(false);
            agent.set_status(format!("the {} is not connected", agent.kind.title()));
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
            Kind::Lw450 => Kwargs::new(),
        };
        if let Some(cmd) = agent.command.lock().unwrap().as_ref()
            && cmd.state().is_some()
        {
            kwargs.insert("command_id".into(), cmd.command_id().into());
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
                ("/module", Some(agent)) => {
                    let on = form.get("on").is_some_and(|v| v == "1");
                    {
                        let mut s = agent.settings.lock().unwrap();
                        s.on = on;
                        s.save(&agent.dir);
                    }
                    if on {
                        agent.module_on();
                    } else {
                        agent.module_off();
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
            let response = tiny_http::Response::empty(303)
                .with_header(tiny_http::Header::from_bytes(&b"Location"[..], &b"/"[..]).unwrap());
            let _ = request.respond(response);
            continue;
        }
        let html = if path == "/about" {
            about_page(devices)
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

fn esc(t: &str) -> String {
    t.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

/// One device's card (mockup `docs/design/ui/mockups/agent.html`); whether the page should look
/// again soon (a registration in progress).
fn device_card(agent: &Agent) -> (String, bool) {
    let s = agent.settings();
    let status = agent.status.lock().unwrap().clone();
    let code = agent.code.lock().unwrap().clone();
    let registered = if !s.token.is_empty() {
        "yes".to_owned()
    } else if let Some(code) = &code {
        format!("in progress: code {code}")
    } else {
        "no".to_owned()
    };
    let on = s.on;
    let linked = !s.token.is_empty() && on;
    let link = *agent.hub.lock().unwrap();
    let (state, state_class) = if !on {
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
    let code_block = match (s.token.is_empty(), code) {
        (true, Some(code)) => format!(
            "<p>Enter this code on the hub: Space, Add machine.</p><p class=code>{}</p>",
            esc(&code)
        ),
        _ => String::new(),
    };
    let what = match agent.kind {
        Kind::Pnp => "Tape printer · USB",
        Kind::Lw450 => "Label printer · USB",
    };
    let card = format!(
        r#"<div class="card{off}"><div class=head><h2>{title}</h2><span class="st {state_class}">{state}</span></div>
<p class=muted>{what}</p>{code_block}
<dl class=kv><dt>Hub</dt><dd>{hub}</dd><dt>Printer</dt><dd>{status}</dd><dt>Registered</dt><dd>{registered}</dd></dl>
<div class="row btns"><form method=post action=/module><input type=hidden name=device value="{device}"><input type=hidden name=on value="{toggle}"><span class=fw><button class="btn{toggle_class}">{button}</button></span></form>
<form method=post action=/reset onsubmit="return confirm('Reset the registration of the {title}? It forgets its token: it is no longer known at the hub until it is added again with a new code.')"><input type=hidden name=device value="{device}"><span class=fw><button class="btn ghost">Reset registration…</button></span></form></div></div>"#,
        off = if on { "" } else { " is-off" },
        title = agent.kind.title(),
        device = agent.kind.folder(),
        toggle = if on { "0" } else { "1" },
        toggle_class = if on { "" } else { " primary" },
        button = if on { "Turn off" } else { "Turn on" },
        status = esc(&status),
        registered = esc(&registered),
    );
    (card, on && !linked)
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

/// The agent's page around `main` (mockup `docs/design/ui/mockups/agent.html`): the header, the
/// agent's head with its link to the hub, a banner when the hub cannot be reached, the tabs
/// (Devices, About); the hub's stylesheet and components only.
fn layout(devices: &[Arc<Agent>], tab: &str, refresh: bool, main: &str) -> String {
    let tabs: String = [
        ("/", "Devices", Some(devices.len())),
        ("/about", "About", None),
    ]
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
    let s = devices[0].settings();
    let (connected, linked) = hub_link(devices);
    let (state, class) = match (connected, linked) {
        (_, 0) => ("No device linked".to_owned(), "idle"),
        (c, n) if c == n => ("Connected to the hub".to_owned(), "wait"),
        (0, _) => ("Not connected to the hub".to_owned(), "bad"),
        (c, n) => (format!("{c} of {n} connected to the hub"), "warn"),
    };
    let banner = match s.server() {
        None => r#"<div class="banner warn"><span><b>No hub address.</b> Set it under Hub, on Devices.</span></div>"#.to_owned(),
        Some(server) if linked > 0 && connected == 0 => format!(
            r#"<div class="banner bad"><span><b>The hub cannot be reached</b> at {}. The devices keep their state; a label already sent finishes. The agent tries again by itself.</span></div>"#,
            esc(&server)
        ),
        Some(_) => String::new(),
    };
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
        "{} device{}",
        devices.len(),
        if devices.len() == 1 { "" } else { "s" }
    ));
    what.push(format!("release {}", esc(VERSION)));
    format!(
        r##"<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1">
{refresh}{THEME_FIRST}<title>Maker Connect agent</title><link rel=icon href="data:image/svg+xml,{icon}"><style>{CSS}
.code{{font:700 var(--fs-6)/1 var(--mono);letter-spacing:.12em;color:var(--accent-text);margin:var(--s-2) 0 var(--s-3)}}
.brand .mark svg{{width:100%;height:100%;display:block}} .card form{{margin:0}} .card .row.btns{{margin-top:var(--s-3)}} .row.btns>form{{flex:0 0 auto}}</style></head><body>
<a class=skip href="#main">Skip to content</a>
<header class=top><span class=brand><span class=mark aria-hidden=true>{MARK}</span><span class=where><b>Maker Connect agent</b><small>{where_}</small></span></span><span class=spacer></span><span class=fw><button type=button class=theme data-theme-btn aria-label="Theme: Auto">Auto</button></span></header>
<main id=main tabindex=-1 class=solo>
{banner}<div class=card><div class=head><h1>This agent</h1><span class="st {class}">{state}</span></div>
<p class=muted>{what}</p></div>
<nav class=tabs aria-label="This agent">{tabs}</nav>
{main}
</main><script>{THEME_SWITCH}</script></body></html>"##,
        refresh = if refresh {
            "<meta http-equiv=refresh content=5>"
        } else {
            ""
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

fn page(devices: &[Arc<Agent>]) -> String {
    // The hub's address is the same in every device's settings file (the form writes all).
    let s = devices[0].settings();
    let (cards, refresh): (Vec<String>, Vec<bool>) = devices.iter().map(|d| device_card(d)).unzip();
    let (connected, linked) = hub_link(devices);
    let (hub_state, hub_class) = match (connected, linked) {
        (_, 0) => ("No device linked", "idle"),
        (c, n) if c == n => ("Connected", "wait"),
        (0, _) => ("Not connected", "bad"),
        _ => ("Partly connected", "warn"),
    };
    let address = s.server().map_or("not set".to_owned(), |a| esc(&a));
    let main = format!(
        r#"<div class=cols><div>
{cards}
</div><div>
<div class=card><div class=head><h2>Hub</h2><span class="st {hub_class}">{hub_state}</span></div>
<dl class=kv><dt>Address</dt><dd>{address}</dd><dt>Devices</dt><dd>{connected} of {linked} connected</dd></dl>
<details{open} style="margin-top:var(--s-3)"><summary>Change the address</summary><form method=post action=/hub style="margin-top:var(--s-3)">
<div class=row><label class=field>Address<input name=hostname value="{host}" placeholder="192.168.60.67"></label>
<label class=field style="flex:0 1 7rem">Port<input name=port inputmode=numeric value="{port}" placeholder="8460"></label></div>
<label class=check><input type=checkbox name=tls {tls}> TLS</label>
<div class="row btns"><span class=fw><button class="btn primary">Save</button></span></div></form></details></div>
</div></div>"#,
        open = if s.server().is_none() { " open" } else { "" },
        host = esc(&s.hostname),
        port = if s.port == 0 {
            String::new()
        } else {
            s.port.to_string()
        },
        tls = if s.tls { "checked" } else { "" },
        cards = cards.join("\n"),
    );
    layout(devices, "Devices", refresh.iter().any(|r| *r), &main)
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
    layout(devices, "About", false, &main)
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
                ("labelwriter-450", "dymo.labelwriter-450")
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
        let card = device_card(&agent).0;
        assert!(card.contains(">Not connected</span>"), "{card}");
        assert!(
            card.contains("<dt>Hub</dt><dd>not connected</dd>"),
            "{card}"
        );
        agent.hub_seen(true);
        let card = device_card(&agent).0;
        assert!(card.contains(">Connected</span>"), "{card}");
        assert!(
            card.contains("<dt>Hub</dt><dd>connected for 0 min · last contact now</dd>"),
            "{card}"
        );
        agent.hub_seen(false);
        let card = device_card(&agent).0;
        assert!(
            card.contains("<dt>Hub</dt><dd>not connected · last contact 0 min ago</dd>"),
            "{card}"
        );
        // The hub's theme switch, the same button and script.
        let page = page(&[Arc::new(agent)]);
        assert!(
            page.contains(r#"class=theme data-theme-btn aria-label="Theme: Auto">Auto</button>"#)
        );
        assert!(page.contains(THEME_SWITCH) && page.contains("localStorage.getItem(\"mc-theme\")"));
    }
}
