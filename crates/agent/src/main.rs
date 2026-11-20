//! # Maker Connect agent
//!
//! The module next to a machine. One device in this step: the DYMO LabelManager PnP (labelle's
//! port, `maker_connect_engine::ports::labelle`), linked to the hub over the Prusa Connect printer
//! protocol as an SDK printer (`docs/design/wire.md`, "The first device"): registration with a code
//! over `/p/register`, then `/p/ws` with telemetry, events and commands (`maker_connect_sdk`).
//! A label comes as `START_CONNECT_DOWNLOAD` with `printing` and `tape_size_mm`.
//!
//! A small page on the agent (`--listen`, default `0.0.0.0:8470`): the hub's address, the DYMO
//! module on or off, the registration code and the link's state.

use maker_connect_engine::{Engine, MachineId, Order, ports::labelle::DymoPnp};
use maker_connect_protocol::{JobOrder, MachineState};
use maker_connect_sdk::command::{Command, CommandHandler, Finish, HandlerError};
use maker_connect_sdk::consts::{self, Source, State};
use maker_connect_sdk::models::{EventCallback, Kwargs};
use maker_connect_sdk::ws::PrinterWs;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PRINTER_TYPE: &str = "dymo.labelmanager-pnp";
const SDK_VERSION: &str = "0.9.0dev0";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The device's settings file, as PrusaLink's `prusa_printer_settings.ini` (`wire.md`, "What the
/// agent keeps"): `[service::connect] hostname, tls, port, token`, `[printer] type, serial`, and
/// ours: `[module] on`.
#[derive(Debug, Clone, Default)]
struct Settings {
    hostname: String,
    tls: bool,
    port: u16,
    token: String,
    serial: String,
    on: bool,
}

impl Settings {
    fn path(dir: &Path) -> PathBuf {
        dir.join("dymo").join("prusa_printer_settings.ini")
    }

    fn load(dir: &Path) -> Settings {
        let mut s = Settings::default();
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
            "[printer]\ntype = {PRINTER_TYPE}\nserial = {}\n\n[service::connect]\nhostname = {}\ntls = {}\nport = {}\ntoken = {}\n\n[module]\non = {}\n",
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

/// A serial drawn once (the port reads none; `wire.md`).
fn random_serial() -> String {
    let mut bytes = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut bytes);
    }
    format!(
        "MC{}",
        bytes.iter().map(|b| format!("{b:02X}")).collect::<String>()
    )
}

/// What the page shows and the link shares.
struct Agent {
    dir: PathBuf,
    settings: Mutex<Settings>,
    code: Mutex<Option<String>>,
    status: Mutex<String>,
    engine: Mutex<Engine>,
    dymo: Mutex<Option<MachineId>>,
    printing: AtomicBool,
    ws: Mutex<Option<Arc<PrinterWs>>>,
    command: Mutex<Option<Arc<Command>>>,
}

impl Agent {
    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
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
                Some(PRINTER_TYPE.to_owned()),
            ),
            ("User-Agent-Version".to_owned(), Some(VERSION.to_owned())),
        ];
        if token && !s.token.is_empty() {
            h.push(("Token".to_owned(), Some(s.token)));
        }
        h
    }

    /// Turn the DYMO module on: its port in the engine.
    fn module_on(&self) {
        let mut dymo = self.dymo.lock().unwrap();
        if dymo.is_none() {
            *dymo = Some(self.engine.lock().unwrap().add(Box::new(DymoPnp::new())));
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
    let settings = Settings::load(&dir);
    let agent = Arc::new(Agent {
        dir,
        settings: Mutex::new(settings.clone()),
        code: Mutex::new(None),
        status: Mutex::new("starting".into()),
        engine: Mutex::new(Engine::new()),
        dymo: Mutex::new(None),
        printing: AtomicBool::new(false),
        ws: Mutex::new(None),
        command: Mutex::new(None),
    });
    setup_link(&agent);
    if settings.on {
        agent.module_on();
    }
    {
        let agent = agent.clone();
        std::thread::spawn(move || link_loop(&agent));
    }
    println!("maker-connect-agent {VERSION} on http://{listen}");
    web(&agent, &listen);
}

/// The SDK's command and socket, with this device's handlers.
fn setup_link(agent: &Arc<Agent>) {
    let events = agent.clone();
    let event_cb: EventCallback = Arc::new(move |event, source, ts, command_id, kwargs| {
        if events.settings().token.is_empty() {
            return;
        }
        let e = maker_connect_sdk::models::Event::new(event, source, ts, command_id, kwargs);
        let mut payload = e.to_payload();
        if let Value::Object(map) = &mut payload
            && !map.contains_key("state")
        {
            map.insert("state".into(), current_state(&events).value().into());
        }
        events.send(&payload);
    });
    let command = Arc::new(Command::new(event_cb));
    let me = agent.clone();
    let info: CommandHandler = Arc::new(move |_| Ok(send_info(&me)));
    command.set_handler(consts::Command::SendInfo, info);
    let me = agent.clone();
    let download: CommandHandler = Arc::new(move |cmd| print_label(&me, cmd));
    command.set_handler(consts::Command::StartConnectDownload, download);
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
        "type": PRINTER_TYPE,
        "firmware": VERSION,
        "sdk": SDK_VERSION,
        "sn": s.serial,
        "fingerprint": s.fingerprint(),
    })
    .as_object()
    .unwrap()
    .clone();
    finish.kwargs = kwargs;
    finish
}

/// START_CONNECT_DOWNLOAD with `printing`: fetch the label from the hub
/// (`/p/teams/{team_id}/files/{hash}/raw`, `__init__.py:468`) and print it with the tape the hub
/// names.
fn print_label(agent: &Agent, cmd: &Command) -> Result<Finish, HandlerError> {
    let kwargs = cmd
        .kwargs()
        .ok_or_else(|| HandlerError::Failed("START_CONNECT_DOWNLOAD requires kwargs".into()))?;
    let server = agent
        .settings()
        .server()
        .ok_or_else(|| HandlerError::Failed("Printer.server must be set!".into()))?;
    let team = &kwargs["team_id"];
    let hash = kwargs["hash"].as_str().unwrap_or("");
    let url = format!("{server}/p/teams/{team}/files/{hash}/raw");
    let id = agent
        .dymo
        .lock()
        .unwrap()
        .ok_or_else(|| HandlerError::Rejected("the DYMO module is off".into()))?;
    agent.printing.store(true, Ordering::SeqCst);
    let result = (|| {
        let mut request = ureq::get(&url);
        for (k, v) in agent.headers(true) {
            if let Some(v) = v {
                request = request.set(&k, &v);
            }
        }
        let response = request
            .call()
            .map_err(|e| HandlerError::Failed(format!("download: {e}")))?;
        let mut bytes = Vec::new();
        use std::io::Read;
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|e| HandlerError::Failed(format!("download: {e}")))?;
        let labels = agent.dir.join("dymo").join("labels");
        let _ = std::fs::create_dir_all(&labels);
        let file = labels.join(format!("{}.png", cmd.command_id()));
        std::fs::write(&file, &bytes).map_err(|e| HandlerError::Failed(e.to_string()))?;
        let job = cmd.command_id();
        let order = JobOrder {
            id: job,
            name: kwargs["path"].as_str().unwrap_or("label.png").to_owned(),
            download: url.clone(),
            size: bytes.len() as u64,
            from_line: None,
            tape_mm: kwargs["tape_size_mm"].as_u64().map(|t| t as u32),
            label_length_mm: None,
            page_size: None,
        };
        let engine = agent.engine.lock().unwrap();
        engine
            .send(
                id,
                Order::Job {
                    order,
                    file: file.clone(),
                },
            )
            .map_err(|_| HandlerError::Failed("the DYMO port has ended".into()))?;
        drop(engine);
        loop {
            std::thread::sleep(Duration::from_millis(200));
            let engine = agent.engine.lock().unwrap();
            let report = engine
                .report(id)
                .map_err(|_| HandlerError::Failed("the DYMO port has ended".into()))?;
            if let Some(end) = report.job_ends.iter().find(|e| e.id == job).cloned() {
                let _ = engine.reported(id, report.done.len(), report.job_ends.len());
                let _ = std::fs::remove_file(&file);
                return if end.ok {
                    Ok(Finish::new(Source::Connect))
                } else {
                    Err(HandlerError::Failed(
                        end.reason
                            .unwrap_or_else(|| "the label did not print".into()),
                    ))
                };
            }
        }
    })();
    agent.printing.store(false, Ordering::SeqCst);
    result
}

fn current_state(agent: &Agent) -> State {
    if agent.printing.load(Ordering::SeqCst) {
        return State::Printing;
    }
    let Some(id) = *agent.dymo.lock().unwrap() else {
        return State::Idle;
    };
    match agent.engine.lock().unwrap().report(id) {
        Ok(r) if r.telemetry.state == MachineState::Busy => State::Printing,
        _ => State::Idle,
    }
}

/// Registration, then telemetry once a second while the DYMO is there.
fn link_loop(agent: &Arc<Agent>) {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let s = agent.settings();
        let Some(server) = s.server() else {
            agent.set_status("no hub address");
            continue;
        };
        let Some(id) = *agent.dymo.lock().unwrap() else {
            agent.set_status("the DYMO module is off");
            continue;
        };
        if s.token.is_empty() {
            register(agent, &server, &s);
            continue;
        }
        let report = match agent.engine.lock().unwrap().report(id) {
            Ok(r) => r,
            Err(_) => continue,
        };
        // Gone: nothing is sent, the socket ends and the hub sees it away (`wire.md`).
        if !report.connected {
            agent.set_status("the DYMO is not connected");
            if let Some(ws) = agent.ws.lock().unwrap().clone()
                && ws.connected()
            {
                ws.disconnect();
            }
            continue;
        }
        let mut kwargs = dymo_telemetry(&report.telemetry);
        if let Some(cmd) = agent.command.lock().unwrap().as_ref()
            && cmd.state().is_some()
        {
            kwargs.insert("command_id".into(), cmd.command_id().into());
        }
        let telemetry =
            maker_connect_sdk::models::Telemetry::new(current_state(agent), None, kwargs);
        if agent.send(&telemetry.to_payload()) {
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
                "printer_type": PRINTER_TYPE,
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

/// The agent's page.
fn web(agent: &Arc<Agent>, listen: &str) {
    let server = tiny_http::Server::http(listen).expect("listen");
    for mut request in server.incoming_requests() {
        let path = request.url().to_owned();
        if request.method() == &tiny_http::Method::Post {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let form = parse_form(&body);
            match path.as_str() {
                "/hub" => {
                    let mut s = agent.settings.lock().unwrap();
                    s.hostname = form.get("hostname").cloned().unwrap_or_default();
                    s.port = form.get("port").and_then(|p| p.parse().ok()).unwrap_or(0);
                    s.tls = form.contains_key("tls");
                    s.save(&agent.dir);
                }
                "/module" => {
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
                "/reset" => {
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
        let html = page(agent);
        let response = tiny_http::Response::from_string(html).with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
                .unwrap(),
        );
        let _ = request.respond(response);
    }
}

fn page(agent: &Agent) -> String {
    let s = agent.settings();
    let status = agent.status.lock().unwrap().clone();
    let registration = if !s.token.is_empty() {
        "registered".to_owned()
    } else if let Some(code) = agent.code.lock().unwrap().clone() {
        format!("in progress: code {code}")
    } else {
        "not registered".to_owned()
    };
    let on = s.on;
    let esc = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('"', "&quot;")
    };
    let linked = !s.token.is_empty() && on;
    let (state, state_class) = if !on {
        ("off", "idle")
    } else if linked {
        ("linked", "wait")
    } else {
        ("not linked", "warn")
    };
    format!(
        r#"<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1">
{refresh}<title>Maker Connect agent</title><link rel=icon href="data:image/svg+xml,{icon}"><style>{CSS}
.code{{font:700 var(--fs-6)/1 var(--mono);letter-spacing:.12em;color:var(--accent-text);margin:var(--s-2) 0 var(--s-3)}}
.brand .mark svg{{width:100%;height:100%;display:block}} .card form{{margin:0}} .card .row.btns{{margin-top:var(--s-3)}}</style></head><body>
<header class=top><span class=brand><span class=mark>{MARK}</span><span class=where><b>Maker Connect agent</b><small>{VERSION}</small></span></span></header>
<main class=bare>
<div class=head><h1>This agent</h1></div>
<section class=card><h2>Hub</h2><form method=post action=/hub>
<div class=row><label class=field>Address <input name=hostname value="{host}" placeholder="192.168.60.67"></label>
<label class=field style="flex:0 1 7rem">Port <input name=port inputmode=numeric value="{port}" placeholder="8460"></label></div>
<label class=check><input type=checkbox name=tls {tls}> TLS</label>
<div class="row btns"><span class=fw><button class="btn primary">Save</button></span></div></form></section>
<section class="card{off}"><div class=head><h2 style="margin:0">DYMO LabelManager PnP</h2><span class="st {state_class}">{state}</span></div>
{code_block}<dl class=kv><dt>Module</dt><dd>{module}</dd><dt>Registration</dt><dd>{registration}</dd><dt>Link</dt><dd>{status}</dd></dl>
<div class="row btns"><form method=post action=/module><input type=hidden name=on value="{toggle}"><span class=fw><button class="btn{toggle_class}">{button}</button></span></form>
<form method=post action=/reset><span class=fw><button class="btn ghost">Reset registration</button></span></form></div></section>
</main></body></html>"#,
        refresh = if linked || !on {
            ""
        } else {
            "<meta http-equiv=refresh content=5>"
        },
        icon = MARK.replace('#', "%23").replace('"', "'").replace('\n', ""),
        host = esc(&s.hostname),
        port = if s.port == 0 {
            String::new()
        } else {
            s.port.to_string()
        },
        tls = if s.tls { "checked" } else { "" },
        off = if on { "" } else { " is-off" },
        code_block = match (s.token.is_empty(), agent.code.lock().unwrap().clone()) {
            (true, Some(code)) => format!(
                "<p class=muted>Enter this code on the hub: Space, add a machine.</p><p class=code>{}</p>",
                esc(&code)
            ),
            _ => String::new(),
        },
        toggle = if on { "0" } else { "1" },
        toggle_class = if on { "" } else { " primary" },
        button = if on { "Turn off" } else { "Turn on" },
        module = if on { "on" } else { "off" },
        status = esc(&status),
    )
}

/// The hub's stylesheet and logo mark, so the agent's page is in the house style.
const CSS: &str = include_str!("../../hub/src/web/mc.css");
const MARK: &str = include_str!("../../../assets/logo-v2-mark.svg");

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

    /// The settings file is PrusaLink's `prusa_printer_settings.ini` as configparser writes it
    /// (`key = value`, Python's `True`/`False`), read back as written; a serial is drawn once.
    #[test]
    fn the_settings_file_as_prusalink_keeps_it() {
        let dir = dir();
        let drawn = Settings::load(&dir);
        assert!(drawn.serial.starts_with("MC") && drawn.serial.len() == 18);
        assert_eq!(Settings::load(&dir).serial, drawn.serial, "drawn once");
        let s = Settings {
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
        let back = Settings::load(&dir);
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

    /// A form as the page posts it, `+` and `%XX` decoded.
    #[test]
    fn the_pages_form() {
        let form = parse_form("hostname=hub+one&port=8460&tls=on&x=%3C%26%3E");
        assert_eq!(form["hostname"], "hub one");
        assert_eq!(form["port"], "8460");
        assert_eq!(form["x"], "<&>");
    }
}
