//! The printer side of Prusa Connect: a Prusa with the Buddy firmware (a MINI) whose
//! Connect settings point here registers and reports to this hub itself, with no agent
//! next to it. The protocol is described in `docs/archive/prusa-connect-protocol.md`, learned from
//! Prusa-Firmware-Buddy's `src/connect` (GPL-3.0); nothing of it is copied.
//!
//! 1. `POST /p/register` (its serial number, fingerprint, printer type, firmware): the
//!    machine is created, and its pairing code goes back in the `Code` header. The printer
//!    shows it; the owner enters it on the website, as for an agent.
//! 2. `GET /p/register` with that `Code`: `202` while it waits, then `200` with the
//!    machine's token in the `Token` header, for as long as the printer has not reported
//!    with that token (its first report spends the code).
//! 3. `POST /p/telemetry` and `POST /p/events` with `Token` (and `Fingerprint`).
//! 4. `GET /p/ws` with `Fingerprint`, `Token` and `Sec-WebSocket-Protocol: prusa-connect`: one
//!    WebSocket for everything after registration, as Prusa's own server serves it
//!    (`reference/prusa-connect` `internal/connector/xbuddy`, read only: OCL, nothing copied) and
//!    the Connect SDK dials it (`reference/Prusa-Connect-SDK-Printer` branch
//!    `feat/ws-connect-local` @ `04c5bd59`, `prusa/connect/printer/ws.py`). Up: a text frame is one
//!    JSON object, an event when it has an `event` key, else telemetry (`wire.go:237-270`); each is
//!    taken in exactly as its HTTP twin above. Down: a command as `J` (JSON) or `G` (G-code), its
//!    id as 8 hex digits, then the body (the SDK's `handle_ws_message`, "BuddyEncoder").

use crate::store::{CommandState, Details, Network, Storage};
use crate::{Store, failed};
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use maker_connect_protocol::{
    Identity, JobProgress, MachineKind, MachineState, Register, Telemetry,
};
use serde_json::Value;

/// Logs what a printer asks under `/p/` (and anything not found), its secrets left out:
/// how we learn what a firmware expects.
pub async fn log(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let path = request.uri().path().to_owned();
    let watched = path.starts_with("/p/") || path.starts_with("/f/");
    let method = request.method().clone();
    if !watched {
        // Everything else passes untouched: an upload is streamed, never held here.
        let response = next.run(request).await;
        let status = response.status();
        if status == StatusCode::NOT_FOUND || method == axum::http::Method::POST {
            println!("{method} {path} -> {}", status.as_u16());
        }
        return response;
    }
    // A Prusa's messages, logged with their bodies (small JSON).
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, 1 << 20).await {
        Ok(bytes) => bytes,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "too large").into_response(),
    };
    let shown = watched.then(|| {
        let headers: Vec<String> = parts
            .headers
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "token" | "authorization" | "fingerprint"))
            .map(|(k, v)| format!("{k}: {}", v.to_str().unwrap_or("?")))
            .collect();
        let body = serde_json::from_slice::<Value>(&bytes)
            .map(|mut v| {
                redact(&mut v);
                v.to_string()
            })
            .unwrap_or_else(|_| format!("{} bytes", bytes.len()));
        format!("[{}] {body}", headers.join(", "))
    });
    let response = next
        .run(axum::extract::Request::from_parts(
            parts,
            axum::body::Body::from(bytes),
        ))
        .await;
    let status = response.status();
    if watched || status == StatusCode::NOT_FOUND || method == axum::http::Method::POST {
        println!(
            "{method} {path} -> {} {}",
            status.as_u16(),
            shown.unwrap_or_default()
        );
    }
    response
}

/// Leaves secrets out of a logged body.
fn redact(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if ["token", "api_key", "password", "fingerprint", "pass"].contains(&key.as_str()) {
                    *v = Value::String("…".into());
                } else {
                    redact(v);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/p/register", post(register).get(registered))
        .route("/p/telemetry", post(telemetry))
        .route("/p/events", post(event))
        .route("/p/ws", axum::routing::get(ws))
}

/// The one subprotocol the server offers (`xbuddy.go:61`, `:388`).
const SUBPROTOCOL: &str = "prusa-connect";
/// The server pings every 54 s and ends a link after 60 s without a frame
/// (`internal/connector/xbuddy/inbound.go:25-36`, `link.go:70-85`).
const PING_EVERY: std::time::Duration = std::time::Duration::from_secs(54);
const READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// The upgrade, refused as the reference server refuses it (`xbuddy.go:695-712`): `401` when the
/// credentials are missing or unknown, `403` when the hub does not take the printer (not paired
/// into a space).
async fn ws(
    State(store): State<Store>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    upgrade: axum::extract::ws::WebSocketUpgrade,
) -> Response {
    let fingerprint = header(&headers, "fingerprint");
    let token = header(&headers, "token").to_owned();
    let refused = || {
        (
            StatusCode::UNAUTHORIZED,
            "the printer's credentials were refused\n",
        )
            .into_response()
    };
    if fingerprint.is_empty() || token.is_empty() {
        return refused();
    }
    match store.knows(&token).await {
        Ok(true) => {}
        Ok(false) => return refused(),
        Err(error) => return failed(error),
    }
    match store.placed(&token).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::FORBIDDEN,
                "the host does not accept this printer\n",
            )
                .into_response();
        }
        Err(error) => return failed(error),
    }
    upgrade
        .protocols([SUBPROTOCOL])
        .on_upgrade(move |socket| link(store, headers, extensions, token, socket))
}

/// A command as the SDK reads it: type letter, 8 hex digits of its id, the body.
fn frame(id: u32, body: &str, json: bool) -> String {
    format!("{}{id:08x}{body}", if json { 'J' } else { 'G' })
}

/// The command a telemetry answer carries over HTTP, as a frame for the socket.
async fn command_of(response: Response) -> Option<String> {
    if response.status() != StatusCode::OK {
        return None;
    }
    let id: u32 = response
        .headers()
        .get("Command-Id")?
        .to_str()
        .ok()?
        .parse()
        .ok()?;
    let json = response
        .headers()
        .get("Content-Type")
        .is_some_and(|v| v.as_bytes().starts_with(b"application/json"));
    let body = axum::body::to_bytes(response.into_body(), 16 << 20)
        .await
        .ok()?;
    Some(frame(id, &String::from_utf8_lossy(&body), json))
}

async fn link(
    store: Store,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    token: String,
    mut socket: axum::extract::ws::WebSocket,
) {
    use axum::extract::ws::Message;
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    // A command goes out as soon as it is queued, as the reference server sends one when it
    // arrives: the store's change signal wakes the link.
    let mut changes = store.subscribe();
    let mut last_frame = tokio::time::Instant::now();
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { break };
                last_frame = tokio::time::Instant::now();
                let Message::Text(text) = message else { continue };
                let body = text.to_string();
                let is_event = serde_json::from_str::<Value>(&body)
                    .is_ok_and(|v| v.get("event").is_some());
                let mut up = headers.clone();
                up.insert("token", token.parse().expect("token came from a header"));
                let answer = if is_event {
                    event(State(store.clone()), up, body).await
                } else {
                    telemetry(State(store.clone()), up, extensions.clone(), body).await
                };
                if answer.status() == StatusCode::UNAUTHORIZED {
                    break;
                }
                if let Some(command) = command_of(answer).await
                    && socket.send(Message::Text(command.into())).await.is_err()
                {
                    break;
                }
            }
            _ = tokio::time::sleep_until(last_frame + READ_DEADLINE) => break,
            changed = changes.recv() => {
                if matches!(changed, Err(tokio::sync::broadcast::error::RecvError::Closed)) {
                    break;
                }
                let caller = match store.caller(&token).await {
                    Ok(Some(caller)) => caller,
                    _ => break,
                };
                let next = store.lock().unwrap().next_command_of_any_kind(Some(&caller));
                if let Some((id, body, json)) = next {
                    println!("command {id} to the printer over /p/ws: {}", crate::printer_files::for_the_log(&body));
                    if socket.send(Message::Text(frame(id, &body, json).into())).await.is_err() {
                        break;
                    }
                }
            }
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
        }
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

/// The body as JSON, whatever content type it came with.
fn parse(body: &str) -> Result<Value, (StatusCode, String)> {
    serde_json::from_str(body)
        .map_err(|error| (StatusCode::BAD_REQUEST, format!("not JSON: {error}")))
}

fn text(value: &Value, key: &str) -> String {
    match &value[key] {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The printer type as a profile knows it: Buddy 6.3.0 writes `"2.1.0"` as one string
/// (`render.cpp:342`, `registrator.cpp:63`, "%hhu.%hhu.%hhu"), and only that is read.
fn printer_type(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

async fn register(State(store): State<Store>, body: String) -> Response {
    let about = match parse(&body) {
        Ok(about) => about,
        Err(refused) => return refused.into_response(),
    };
    let serial = text(&about, "sn");
    let identity = Identity {
        printer_type: printer_type(&about["printer_type"]),
        ..Identity::default()
    };
    let profile = crate::profiles::detect(&identity);
    let model = profile.map_or("Prusa printer", |p| p.model.as_str());
    let tail: String = serial
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let name = if tail.is_empty() {
        model.to_owned()
    } else {
        format!("{model} {tail}")
    };
    let firmware = text(&about, "firmware");
    let machine = Register {
        protocol: maker_connect_protocol::VERSION,
        name: name.clone(),
        kind: MachineKind::Fdm,
        agent_version: format!("Prusa firmware {firmware} (built-in Connect)").replace("  ", " "),
    };
    let registered: sqlx::Result<_> = async {
        let (id, token, code) = store.register_linked(&machine, "prusa-connect").await?;
        if let Some(profile) = profile {
            store.recognise(&token, &profile.id).await?;
        }
        Ok((id, code))
    }
    .await;
    match registered {
        Ok((_, code)) => {
            println!("Prusa Connect printer registered: {name}, code {code}");
            (StatusCode::OK, [("Code", code)]).into_response()
        }
        Err(error) => failed(error),
    }
}

/// The printer asks with its code whether it has been added yet.
async fn registered(State(store): State<Store>, headers: HeaderMap) -> Response {
    match store.token_for_code(header(&headers, "code")).await {
        Ok(Some(Some(token))) => (StatusCode::OK, [("Token", token)]).into_response(),
        Ok(Some(None)) => StatusCode::ACCEPTED.into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "unknown code: register again").into_response(),
        Err(error) => failed(error),
    }
}

/// A telemetry frame taken in over the machine's last telemetry: as Prusa's server keeps a
/// printer's state (`reference/prusa-connect` `internal/connector/xbuddy`, read only: OCL, nothing
/// copied), a frame writes only the members it carries and everything it leaves out stays as it
/// was (`inbound.go:136-159`, `:273-299`; `telemetrytools.go:210`, "Only members the frame carries
/// are written").
/// Buddy sends a Reduced frame most of the time, without the temperatures, targets, speed, flow
/// and position, and a Full one only when its fingerprint changes or every 5 minutes (v8.0.2
/// `planner.cpp:111-113`, `:537-557`; `render.cpp:177`).
///
/// The tools come in one of the firmware's shapes, read as the server reads them
/// (`telemetrytools.go:102-151`): Buddy 8's `tools` object (v8.0.2 `render.cpp:219-292`), Buddy
/// 6's `slot` object on a machine with more than one tool (6.3.0 `render.cpp:222-247`), or else
/// the one head's values at the top of the frame (`temp_nozzle`, `target_nozzle`, `material`).
fn take_in(report: &Value, last: Option<Telemetry>) -> Telemetry {
    let mut t = last.unwrap_or_default();
    // A member the frame carries, as a number (a null: none).
    let carried = |value: &Value, key: &str| value.get(key).map(|v| v.as_f64().map(|n| n as f32));
    let set = |field: &mut Option<f32>, value: &Value, key: &str| {
        if let Some(v) = carried(value, key) {
            *field = v;
        }
    };
    // The state comes in every frame (`render.cpp:303`).
    t.state = match report["state"].as_str().unwrap_or("") {
        "PRINTING" | "BUSY" => MachineState::Busy,
        "PAUSED" => MachineState::Paused,
        "ATTENTION" => MachineState::Attention,
        "ERROR" => MachineState::Error,
        _ => MachineState::Idle,
    };
    // The job's members come in every frame while there is one (`render.cpp:162-175`); the file
    // never does (JOB_INFO tells it).
    t.job = if matches!(t.state, MachineState::Busy | MachineState::Paused) {
        let mut job = t.job.take();
        if let Some(percent) = report["progress"].as_f64() {
            let job = job.get_or_insert_with(|| JobProgress {
                file: String::new(),
                done: 0,
                total: 100,
                elapsed_s: None,
                remaining_s: None,
            });
            job.done = percent.clamp(0.0, 100.0) as u64;
        }
        if let Some(job) = job.as_mut() {
            if let Some(v) = report.get("time_printing") {
                job.elapsed_s = v.as_u64();
            }
            if let Some(v) = report.get("time_remaining") {
                job.remaining_s = v.as_u64();
            }
        }
        job
    } else {
        None
    };
    set(&mut t.bed_c, report, "temp_bed");
    set(&mut t.bed_target_c, report, "target_bed");
    set(&mut t.speed_percent, report, "speed");
    // A flow of 0 is none read, not a flow of nothing (`inbound.go:287-293`).
    set(&mut t.flow_percent, report, "flow");
    t.flow_percent = t.flow_percent.filter(|f| *f != 0.0);
    // While it prints Buddy tells only `axis_z` (`render.cpp:212-217`): X and Y stay as told
    // last, marked as not current.
    let (x, y, z) = (
        carried(report, "axis_x").flatten(),
        carried(report, "axis_y").flatten(),
        carried(report, "axis_z").flatten(),
    );
    if x.is_some() || y.is_some() || z.is_some() {
        let mut position = t.position_mm.unwrap_or([0.0; 3]);
        for (i, v) in [x, y, z].into_iter().enumerate() {
            if let Some(v) = v {
                position[i] = v;
            }
        }
        t.position_mm = Some(position);
        t.only_z = x.is_none() || y.is_none();
    }
    // The CORE One's chamber (`chamber.temp`, `target_temp`: 0 is none set), or the XL's
    // enclosure (`enclosure.temp`), as the server reads them (`telemetrychamber.go:32-124`): only
    // numbers are members, a reading of 0 is none, and a full frame (it carries `temp_bed`) with
    // neither block means the enclosure was switched off.
    let number = |block: &Value, key: &str| block[key].as_f64().map(|n| n as f32);
    if let Some(chamber) = report.get("chamber") {
        if let Some(temp) = number(chamber, "temp") {
            t.chamber_c = Some(temp).filter(|c| *c != 0.0);
        }
        if let Some(target) = number(chamber, "target_temp") {
            t.chamber_target_c = Some(target);
        }
    } else if let Some(enclosure) = report.get("enclosure") {
        if let Some(temp) = number(enclosure, "temp") {
            t.chamber_c = Some(temp).filter(|c| *c != 0.0);
        }
    } else if report.get("temp_bed").is_some() {
        t.chamber_c = None;
    }
    if let Some(tree) = report.get("tools").and_then(Value::as_object) {
        take_in_tree(&mut t, tree);
    } else if let Some(slot) = report.get("slot").filter(|s| s.is_object()) {
        // The top of the frame is the selected tool's (`telemetrytools.go:118-133`, `:146-183`).
        t.tools = tools(slot);
        set(&mut t.nozzle_c, report, "temp_nozzle");
        set(&mut t.nozzle_target_c, report, "target_nozzle");
    } else {
        // One head: the top of the frame is all it says of it (`telemetrytools.go:134-150`,
        // `:184-206`): its nozzle and material. The fans at the top of a Buddy 6 frame are not
        // read there, so not here.
        set(&mut t.nozzle_c, report, "temp_nozzle");
        set(&mut t.nozzle_target_c, report, "target_nozzle");
        if let Some(material) = report.get("material") {
            t.material = material.as_str().map(str::to_owned);
        }
    }
    t
}

/// Buddy 8's `tools` object (v8.0.2 `render.cpp:219-292`), keyed by tool id (`tools.cpp:20-48`):
/// `"0"`, `"1"`, ... a head, `"0.<n>"` a slot in front of an MMU (no heat of its own: not read).
/// A head carries `active`, `nozzle_temp`, `nozzle_target_temp` (null: off, 0 as the server
/// holds it), `print_fan_rpm` and `heatbreak_fan_rpm`; a head that is not enabled is `null`, and
/// its readings go (`telemetrytools.go:393-466`). Head `n` is tool `n + 1` on the page, as the
/// server puts slot entry `n` on head `n - 1` (`telemetrytools.go:232`). The values at the
/// top of our telemetry are the active head's, or the only one's.
fn take_in_tree(t: &mut Telemetry, tree: &serde_json::Map<String, Value>) {
    use maker_connect_protocol::ToolHeat;
    // The heads as last told: the list, or the one head at the top.
    let mut heads: Vec<ToolHeat> = if t.tools.is_empty() {
        vec![ToolHeat {
            number: 1,
            nozzle_c: t.nozzle_c,
            nozzle_target_c: t.nozzle_target_c,
            active: true,
        }]
    } else {
        std::mem::take(&mut t.tools)
    };
    let mut fans = Vec::new();
    for (key, value) in tree {
        let Ok(id) = key.parse::<u32>() else { continue };
        let number = id + 1;
        let Some(members) = value.as_object() else {
            if value.is_null() {
                heads.retain(|h| h.number != number);
            }
            continue;
        };
        let at = match heads.iter().position(|h| h.number == number) {
            Some(at) => at,
            None => {
                heads.push(ToolHeat {
                    number,
                    ..ToolHeat::default()
                });
                heads.len() - 1
            }
        };
        let head = &mut heads[at];
        if let Some(v) = members.get("nozzle_temp") {
            head.nozzle_c = v.as_f64().map(|n| n as f32);
        }
        if let Some(v) = members.get("nozzle_target_temp") {
            head.nozzle_target_c = Some(v.as_f64().unwrap_or(0.0) as f32);
        }
        if let Some(active) = members.get("active").and_then(Value::as_bool) {
            head.active = active;
        }
        let rpm = |key: &str| members.get(key).map(|v| v.as_f64().map(|n| n as u32));
        fans.push((number, rpm("print_fan_rpm"), rpm("heatbreak_fan_rpm")));
    }
    heads.sort_by_key(|h| h.number);
    let shown = match heads.as_slice() {
        [only] => Some(only.clone()),
        _ => heads.iter().find(|h| h.active).cloned(),
    };
    t.nozzle_c = shown.as_ref().and_then(|h| h.nozzle_c);
    t.nozzle_target_c = shown.as_ref().and_then(|h| h.nozzle_target_c);
    if let Some((_, print, hotend)) = fans
        .into_iter()
        .find(|(n, _, _)| shown.as_ref().is_some_and(|h| h.number == *n))
    {
        if let Some(print) = print {
            t.print_fan_rpm = print;
        }
        if let Some(hotend) = hotend {
            t.hotend_fan_rpm = hotend;
        }
    }
    t.tools = if heads.len() > 1 { heads } else { Vec::new() };
}

/// Each tool of a machine with more than one (the XL, an MMU), from the `slot` object of
/// Buddy 6.3.0 (`render.cpp:222-247`): `slot.<n>.temp` per tool, numbered from 1 as Buddy does,
/// and `slot.active` the number of the tool in use (0: none). No per-tool target is sent.
fn tools(slot: &Value) -> Vec<maker_connect_protocol::ToolHeat> {
    let Some(slot) = slot.as_object() else {
        return Vec::new();
    };
    let active = slot.get("active").and_then(Value::as_u64);
    let mut heads: Vec<maker_connect_protocol::ToolHeat> = slot
        .iter()
        .filter_map(|(key, t)| {
            let number = key.parse::<u32>().ok()?;
            Some(maker_connect_protocol::ToolHeat {
                number,
                nozzle_c: t["temp"].as_f64().map(|c| c as f32),
                nozzle_target_c: None,
                active: active == Some(u64::from(number)),
            })
        })
        .collect();
    heads.sort_by_key(|h| h.number);
    if heads.len() > 1 { heads } else { Vec::new() }
}

/// What an `INFO` event tells about the printer, its secrets (the Prusa Link key) left out.
pub(crate) fn to_details(info: &Value) -> Details {
    let some = |value: &Value| value.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let network = &info["network_info"];
    Details {
        serial: some(&info["sn"]),
        printer_type: printer_type(&info["printer_type"]),
        firmware: some(&info["firmware"]),
        // Buddy 6.3.0 writes it at the top level of INFO (`render.cpp:352`).
        nozzle_mm: info["nozzle_diameter"].as_f64(),
        // A name per tool under `tools.<n>.material`, "---" for a tool with nothing loaded
        // (`render.cpp:395`): no material.
        material: info["tools"].as_object().and_then(|tools| {
            tools
                .values()
                .find_map(|t| some(&t["material"]).filter(|m| m != "---"))
        }),
        storages: info["storages"]
            .as_array()
            .map(|storages| {
                storages
                    .iter()
                    .map(|s| Storage {
                        kind: some(&s["type"]).unwrap_or_default(),
                        path: some(&s["mountpoint"]).unwrap_or_default(),
                        free_bytes: s["free_space"].as_u64(),
                        read_only: s["read_only"].as_bool().unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        network: network.is_object().then(|| Network {
            hostname: some(&network["hostname"]),
            lan_ipv4: some(&network["lan_ipv4"]),
            lan_mac: some(&network["lan_mac"]),
            wifi_ipv4: some(&network["wifi_ipv4"]),
            wifi_mac: some(&network["wifi_mac"]),
            wifi_ssid: some(&network["wifi_ssid"]),
        }),
    }
}

async fn telemetry(
    State(store): State<Store>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    body: String,
) -> Response {
    let report = match parse(&body) {
        Ok(report) => report,
        Err(refused) => return refused.into_response(),
    };
    let token = header(&headers, "token");
    // Its first report with the token: the code is spent (`registrator.cpp:233`).
    if let Err(error) = store.code_spent(token).await {
        return failed(error);
    }
    // A printer not seen for longer than ONLINE_SECONDS: what it was given and had not
    // answered failed, before this report is taken in (K18 (b), `docs/plans/agent-engine.md`).
    let caller = match store.caller(token).await {
        Ok(caller) => caller,
        Err(error) => return failed(error),
    };
    store
        .lock()
        .unwrap()
        .fail_commands_of_gone_printer(caller.as_ref());
    let last = match store.last_telemetry(token).await {
        Ok(last) => last,
        Err(error) => return failed(error),
    };
    let mut telemetry = take_in(&report, last);
    // Every request says the firmware's version.
    let version = header(&headers, "user-agent-version");
    if !version.is_empty() {
        telemetry.firmware = Some(format!("Prusa firmware {version}"));
    }
    // The file it prints, as its JOB_INFO said for this print (not the last report's, which
    // may be the print before).
    let job_id = report["job_id"].as_u64();
    if let Some(job) = telemetry.job.as_mut() {
        match store.lock().unwrap().printing_file.get(token) {
            Some((known, name)) if known.is_none() || *known == job_id => job.file = name.clone(),
            _ if job_id.is_some() => job.file.clear(),
            _ => {}
        }
    }
    if let Some(peer) = crate::peer_address(&headers, &extensions) {
        let _ = store.set_peer(token, &peer).await;
    }
    // G-code still running in the background: its command is not given up on.
    if let Some(id) = report["command_id"].as_u64() {
        store
            .lock()
            .unwrap()
            .command_still_running(caller.as_ref(), id as u32);
    }
    let reported = store.report(token, &telemetry).await;
    // A print it runs itself (from its stick, or started at its screen): recorded as a job
    // when it ends, as it says (FINISHED, STOPPED, ERROR).
    if let Ok(Some(_)) = reported
        && let Err(error) = store
            .follow_own_print(token, &telemetry, report["state"].as_str())
            .await
    {
        return failed(error);
    }
    // A print from the stick is over once the printer is idle again (a minute on: it may
    // still be starting).
    if let Ok(Some(_)) = reported
        && matches!(
            report["state"].as_str(),
            Some("IDLE" | "READY" | "FINISHED" | "STOPPED")
        )
        && let Ok(Some((machine, space))) = store.placed(token).await
        && let Err(error) = store.usb_print_over(space, &machine).await
    {
        return failed(error);
    }
    // A job for it goes to its stick when it is free; one printing follows its state.
    if let Ok(Some(_)) = reported
        && let Some(state) = report["state"].as_str()
    {
        let active = match store.placed(token).await {
            Ok(Some((machine, space))) => store.active_job(space, &machine).await,
            Ok(None) => Ok(None),
            Err(error) => Err(error),
        };
        let active = match active {
            Ok(active) => active,
            Err(error) => return failed(error),
        };
        if let Err(error) =
            crate::printer_files::on_report(&store, token, state, job_id, active).await
        {
            return failed(error);
        }
    }
    let mut store = store.lock().unwrap();
    match reported {
        // A command goes back as the answer, G-code as text or JSON; none: an empty answer.
        Ok(Some(_)) => match store.next_command_of_any_kind(caller.as_ref()) {
            Some((id, body, json)) => {
                println!(
                    "command {id} to the printer: {}",
                    crate::printer_files::for_the_log(&body)
                );
                let kind = if json {
                    "application/json"
                } else {
                    "text/x.gcode"
                };
                (
                    StatusCode::OK,
                    [
                        ("Command-Id", id.to_string()),
                        ("Content-Type", kind.to_owned()),
                    ],
                    body,
                )
                    .into_response()
            }
            None => StatusCode::NO_CONTENT.into_response(),
        },
        Ok(None) => (StatusCode::UNAUTHORIZED, "unknown printer: register again").into_response(),
        Err(error) => failed(error),
    }
}

async fn event(State(store): State<Store>, headers: HeaderMap, body: String) -> Response {
    let event = match parse(&body) {
        Ok(event) => event,
        Err(refused) => return refused.into_response(),
    };
    let token = header(&headers, "token");
    if let Err(error) = store.code_spent(token).await {
        return failed(error);
    }
    let shared = store;
    let name = text(&event, "event");
    let caller = match shared.caller(token).await {
        Ok(caller) => caller,
        Err(error) => return failed(error),
    };
    {
        let mut store = shared.lock().unwrap();
        if let Some(id) = event["command_id"].as_u64() {
            let state = match name.as_str() {
                "ACCEPTED" => Some(CommandState::Accepted),
                // A command that asked for something is done when the answer comes. Deleting a
                // file or folder and making a folder answer with FILE_CHANGED, not FINISHED
                // (Buddy 6.3's connect/planner.cpp: "the result is generated through the
                // FILE_CHANGED event indirectly").
                // SET_PRINTER_READY answers with FINISHED or REJECTED (`planner.cpp:750`), SEND_STATE_INFO with STATE_CHANGED,
                // CANCEL_OBJECT and UNCANCEL_OBJECT with CANCELABLE_CHANGED.
                "FINISHED" | "FILE_INFO" | "FILE_CHANGED" | "JOB_INFO" | "INFO"
                | "TRANSFER_INFO" | "STATE_CHANGED" | "CANCELABLE_CHANGED" => {
                    Some(CommandState::Finished)
                }
                // Refused at once, or taken on and then failed (Buddy's FAILED): either way not done.
                "REJECTED" | "FAILED" => Some(CommandState::Rejected),
                _ => None,
            };
            if let Some(state) = state {
                let reason = event["reason"].as_str().map(str::to_owned);
                println!("command {id}: {name} {}", reason.as_deref().unwrap_or(""));
                store.command_went(caller.as_ref(), id as u32, state, reason, Vec::new());
            }
        }
    }
    // Files on its stick, and transfers to it.
    if let Err(error) = crate::printer_files::on_event(&shared, token, &name, &event).await {
        return failed(error);
    }
    let known = match name.as_str() {
        "INFO" => shared.set_details(token, &to_details(&event["data"])).await,
        _ => shared.knows(token).await,
    };
    match known {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::UNAUTHORIZED, "unknown printer: register again").into_response(),
        Err(error) => failed(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::Request;

    use tower::ServiceExt;

    /// A frame taken in over nothing before it.
    fn first(report: Value) -> Telemetry {
        take_in(&report, None)
    }

    /// The MINI while printing on 6.3.0 (2026-11-08): a full report now and then (Z only), short
    /// ones every second in between (job, progress, times). Z goes on, X and Y are not shown as if
    /// they were current, the time left stays as told last.
    #[test]
    fn a_printing_mini_tells_z_and_keeps_the_rest() {
        let before = first(serde_json::json!({
            "state": "IDLE", "axis_x": 176.4, "axis_y": 24.1, "axis_z": 2.0, "temp_nozzle": 27.0
        }));
        assert_eq!(
            (before.position_mm, before.only_z),
            (Some([176.4, 24.1, 2.0]), false)
        );
        let full = take_in(
            &serde_json::json!({
                "state": "PRINTING", "axis_z": 1.8, "fan_extruder": 2786, "fan_print": 2337,
                "job_id": 29, "progress": 5
            }),
            Some(before),
        );
        assert_eq!(full.position_mm, Some([176.4, 24.1, 1.8]));
        assert!(full.only_z, "X and Y are the last told, not current");
        assert_eq!(full.nozzle_c, Some(27.0), "not in the frame: as told last");
        // The fans at the top of a Buddy 6 frame are not read, as the server does not.
        assert_eq!((full.print_fan_rpm, full.hotend_fan_rpm), (None, None));
        let short = take_in(
            &serde_json::json!({
                "state": "PRINTING", "job_id": 29, "progress": 6, "time_printing": 800
            }),
            Some(full.clone()),
        );
        assert_eq!(short.position_mm.map(|p| p[2]), Some(1.8));
        assert!(short.only_z);
        assert_eq!(full.job.as_ref().and_then(|j| j.remaining_s), None);
        let told = take_in(
            &serde_json::json!({
                "state": "PRINTING", "job_id": 29, "progress": 6, "time_remaining": 3240
            }),
            Some(short.clone()),
        );
        let untold = take_in(
            &serde_json::json!({
                "state": "PRINTING", "job_id": 29, "progress": 6, "time_printing": 801
            }),
            Some(told),
        );
        assert_eq!(untold.job.as_ref().and_then(|j| j.remaining_s), Some(3240));
        assert_eq!(untold.job.as_ref().and_then(|j| j.elapsed_s), Some(801));
        let higher = take_in(
            &serde_json::json!({"state": "PRINTING", "axis_z": 2.0}),
            Some(short),
        );
        assert_eq!(higher.position_mm.map(|p| p[2]), Some(2.0), "Z goes on");
        // Stopped, all three told: current, and no job.
        let after = take_in(
            &serde_json::json!({
                "state": "STOPPED", "axis_x": 10.0, "axis_y": 20.0, "axis_z": 30.0
            }),
            Some(higher),
        );
        assert_eq!(
            (after.position_mm, after.only_z),
            (Some([10.0, 20.0, 30.0]), false)
        );
        assert!(after.job.is_none());
    }

    /// Buddy 8.0.2 (the owner's MINI): a Full frame (v8.0.2 `render.cpp:177-292`, the head under
    /// `tools."0"`), then Reduced ones with only the state (`planner.cpp:537-557`). What the
    /// Reduced frame leaves out keeps the Full frame's value; a frame with a new value changes it.
    #[test]
    fn a_reduced_frame_keeps_what_the_full_one_told() {
        let full = first(serde_json::json!({
            "temp_bed": 59.8, "target_bed": 60.0, "speed": 100, "flow": 95,
            "axis_x": 90.0, "axis_y": 90.0, "axis_z": 10.0,
            "tools": {"0": {"type": "FFF_PRINT_HEAD", "active": true, "nozzle_temp": 214.6,
                "nozzle_target_temp": 215, "heatbreak_fan_rpm": 7100, "print_fan_rpm": 0}},
            "state": "IDLE"
        }));
        assert_eq!(
            (full.nozzle_c, full.nozzle_target_c),
            (Some(214.6), Some(215.0))
        );
        assert_eq!((full.bed_c, full.bed_target_c), (Some(59.8), Some(60.0)));
        assert_eq!(
            (full.print_fan_rpm, full.hotend_fan_rpm),
            (Some(0), Some(7100))
        );
        assert!(full.tools.is_empty(), "one head: no list");
        let reduced = take_in(&serde_json::json!({"state": "IDLE"}), Some(full.clone()));
        assert_eq!(
            reduced, full,
            "a Reduced frame changes nothing it leaves out"
        );
        // Cooled down: the bed's target 0, the nozzle's null (off, `render.cpp:233-237`).
        let cooled = take_in(
            &serde_json::json!({
                "temp_bed": 58.9, "target_bed": 0.0, "speed": 100, "flow": 95, "axis_z": 10.0,
                "tools": {"0": {"type": "FFF_PRINT_HEAD", "active": true, "nozzle_temp": 201.3,
                    "nozzle_target_temp": null, "heatbreak_fan_rpm": 7000, "print_fan_rpm": 0}},
                "state": "IDLE"
            }),
            Some(reduced),
        );
        assert_eq!(
            (cooled.nozzle_c, cooled.nozzle_target_c),
            (Some(201.3), Some(0.0))
        );
        assert_eq!((cooled.bed_c, cooled.bed_target_c), (Some(58.9), Some(0.0)));
        assert_eq!(cooled.hotend_fan_rpm, Some(7000));
        assert_eq!(cooled.position_mm, Some([90.0, 90.0, 10.0]));
    }

    /// Buddy 8's `tools` object with more than one head (an XL): each head in the list, numbered
    /// from 1; the top our telemetry shows is the active head's; a head that is not enabled
    /// (`null`) goes; slots in front of an MMU (`"0.<n>"`) are no heads.
    #[test]
    fn the_tools_object_of_8_lists_the_heads() {
        let xl = first(serde_json::json!({
            "temp_bed": 60.0, "target_bed": 60.0,
            "tools": {
                "0": {"type": "FFF_PRINT_HEAD", "active": false, "nozzle_temp": 170.0,
                    "nozzle_target_temp": 170, "heatbreak_fan_rpm": 0, "print_fan_rpm": 0},
                "1": {"type": "FFF_PRINT_HEAD", "active": true, "nozzle_temp": 215.0,
                    "nozzle_target_temp": 215, "heatbreak_fan_rpm": 7000, "print_fan_rpm": 4000},
                "2": null
            },
            "state": "PRINTING"
        }));
        assert_eq!(
            xl.tools
                .iter()
                .map(|h| (h.number, h.nozzle_c, h.active))
                .collect::<Vec<_>>(),
            [(1, Some(170.0), false), (2, Some(215.0), true)]
        );
        assert_eq!((xl.nozzle_c, xl.print_fan_rpm), (Some(215.0), Some(4000)));
        let mmu = first(serde_json::json!({
            "tools": {
                "0": {"type": "FFF_PRINT_HEAD", "active": true, "nozzle_temp": 215.0,
                    "nozzle_target_temp": 215, "heatbreak_fan_rpm": 7000, "print_fan_rpm": 0},
                "0.0": {"active": true, "type": "FFF_FEEDER_SLOT", "consumed_weight": null},
                "0.1": {"active": false, "type": "FFF_FEEDER_SLOT", "consumed_weight": null}
            },
            "state": "IDLE"
        }));
        assert!(mmu.tools.is_empty(), "one head");
        assert_eq!(mmu.nozzle_c, Some(215.0));
    }

    async fn call(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap) {
        let response = app.clone().oneshot(request).await.unwrap();
        (response.status(), response.headers().clone())
    }

    /// A MINI pointed at this hub registers, shows its code, waits, is added with that
    /// code on the website, gets its token, and then shows with its telemetry and profile.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_mini_registers_by_itself(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (status, headers) = call(
            &app,
            Request::post("/p/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"sn":"CZPX1234X017XC12345","fingerprint":"abc","printer_type":"2.1.0","firmware":"6.3.0"}"#,
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let code = headers["code"].to_str().unwrap().to_owned();
        let ask = || {
            Request::get("/p/register")
                .header("Code", &code)
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(call(&app, ask()).await.0, StatusCode::ACCEPTED);

        let space = store.default_space().await.unwrap().0;
        assert!(store.pair(space, &code).await.unwrap().is_some());
        let (status, headers) = call(&app, ask()).await;
        assert_eq!(status, StatusCode::OK);
        let token = headers["token"].to_str().unwrap().to_owned();
        // The answer may be lost on the way: the code gets its token again until the
        // printer has reported with it (registrator.cpp:233).
        let (status, again) = call(&app, ask()).await;
        assert_eq!(
            (status, again["token"].to_str().unwrap()),
            (StatusCode::OK, token.as_str())
        );

        let (status, _) = call(
            &app,
            Request::post("/p/telemetry")
                .header("Token", &token)
                .header("Fingerprint", "abc")
                .header("User-Agent-Version", "6.3.0+0.LOCAL")
                .body(Body::from(
                    r#"{"state":"PRINTING","temp_bed":60.1,"target_bed":60,"temp_nozzle":214.9,"progress":42}"#,
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(
            call(&app, ask()).await.0,
            StatusCode::NOT_FOUND,
            "its first report spends the code"
        );
        let machines = store.machines(space).await.unwrap();
        let mini = &machines[0];
        assert_eq!(mini.name, "Original Prusa MINI/+ 2345");
        assert_eq!(mini.link, "prusa-connect");
        assert!(
            token.len() <= 20,
            "CONNECT_TOKEN_LEN is 20 (printer.hpp:190)"
        );
        assert_eq!(
            mini.telemetry.as_ref().unwrap().firmware.as_deref(),
            Some("Prusa firmware 6.3.0+0.LOCAL")
        );
        assert_eq!(mini.profile.as_deref(), Some("prusa/mini"));
        let telemetry = mini.telemetry.as_ref().unwrap();
        assert_eq!(telemetry.state, MachineState::Busy);
        assert_eq!(telemetry.nozzle_c, Some(214.9));
        assert_eq!(telemetry.job.as_ref().unwrap().done, 42);
        // A Reduced frame (the state and the job only): the temperatures stay as told, and the
        // history the chart draws holds what the page shows.
        let (status, _) = call(
            &app,
            Request::post("/p/telemetry")
                .header("Token", &token)
                .body(Body::from(r#"{"state":"PRINTING","progress":43}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let kept = store.machines(space).await.unwrap()[0]
            .telemetry
            .clone()
            .unwrap();
        assert_eq!(
            (kept.nozzle_c, kept.bed_c, kept.bed_target_c),
            (Some(214.9), Some(60.1), Some(60.0))
        );
        assert_eq!(kept.job.as_ref().unwrap().done, 43);
        let history = store.history(space, &mini.id, 600).await.unwrap();
        assert_eq!(
            (history[0].nozzle, history[0].bed, history[0].bed_target),
            (Some(214.9), Some(60.1), Some(60.0))
        );

        // A command from the website goes back as the answer to its next telemetry, and the
        // printer tells how it went.
        let mini_id = mini.id.clone();
        let link = store.link_of(space, &mini_id).await.unwrap();
        let id = store
            .lock()
            .unwrap()
            .queue_command(link.as_deref(), &mini_id, "Home", "G28")
            .unwrap();
        let report = || {
            Request::post("/p/telemetry")
                .header("Token", &token)
                .body(Body::from(r#"{"state":"IDLE"}"#))
                .unwrap()
        };
        let (status, headers) = call(&app, report()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["command-id"], id.to_string().as_str());
        assert_eq!(headers["content-type"], "text/x.gcode");
        assert_eq!(
            call(&app, report()).await.0,
            StatusCode::NO_CONTENT,
            "one at a time"
        );
        let finished = Request::post("/p/events")
            .header("Token", &token)
            .body(Body::from(format!(
                r#"{{"event":"FINISHED","command_id":{id},"state":"IDLE"}}"#
            )))
            .unwrap();
        assert_eq!(call(&app, finished).await.0, StatusCode::NO_CONTENT);
        let machines = store.machines(space).await.unwrap();
        assert_eq!(machines[0].commands[0].state, CommandState::Finished);

        let unknown = Request::post("/p/telemetry")
            .header("Token", "nope")
            .body(Body::from(r#"{"state":"IDLE"}"#))
            .unwrap();
        assert_eq!(call(&app, unknown).await.0, StatusCode::UNAUTHORIZED);
    }

    /// An INFO event as a MINI sends it: its details are kept, its Prusa Link key is not.
    #[test]
    fn info_becomes_details_without_secrets() {
        let info: Value = serde_json::from_str(
            r#"{"api_key":"secret","firmware":"6.3.0+0.LOCAL","nozzle_diameter":0.4,
                "printer_type":"2.1.0","sn":"CZPX1234","network_info":{"hostname":"mini","lan_ipv4":"192.168.1.64"},
                "storages":[{"free_space":30537007104,"mountpoint":"/usb","read_only":false,"type":"USB"}],
                "tools":{"1":{"material":"PETG","nozzle_diameter":0.4}}}"#,
        )
        .unwrap();
        let details = to_details(&info);
        assert_eq!(details.serial.as_deref(), Some("CZPX1234"));
        assert_eq!(details.material.as_deref(), Some("PETG"));
        assert_eq!(details.storages[0].free_bytes, Some(30_537_007_104));
        assert_eq!(details.network.unwrap().hostname.as_deref(), Some("mini"));
        assert!(
            !serde_json::to_string(&to_details(&info))
                .unwrap()
                .contains("secret")
        );
    }

    /// A report with only the state keeps the temperatures of the one before.
    #[test]
    fn a_short_report_keeps_what_it_left_out() {
        let full = first(serde_json::from_str(r#"{"state":"IDLE","temp_nozzle":24,"temp_bed":23,"axis_x":1,"axis_y":2,"axis_z":3}"#).unwrap());
        let short = take_in(
            &serde_json::from_str(r#"{"state":"IDLE"}"#).unwrap(),
            Some(full),
        );
        assert_eq!(short.nozzle_c, Some(24.0));
        assert_eq!(short.position_mm, Some([1.0, 2.0, 3.0]));
    }

    /// Buddy 6.3.0's telemetry (render.cpp:129-257): flat values for the one head; its fans at
    /// the top are not read, as the server reads only the nozzle and material there
    /// (`telemetrytools.go:184-206`).
    #[test]
    fn the_flat_keys_of_6_3_0_tell_the_heads_values() {
        let report: Value = serde_json::from_str(
            r#"{"job_id":7,"time_printing":600,"time_remaining":900,"progress":40,
                "temp_nozzle":215.3,"temp_bed":60.1,"target_nozzle":215.0,"target_bed":60.0,
                "speed":100,"flow":95,"material":"PETG","axis_z":1.20,
                "fan_extruder":7100,"fan_print":5200,"filament":3.5,"state":"PRINTING"}"#,
        )
        .unwrap();
        let t = first(report);
        assert_eq!((t.nozzle_c, t.nozzle_target_c), (Some(215.3), Some(215.0)));
        assert_eq!((t.bed_c, t.bed_target_c), (Some(60.1), Some(60.0)));
        assert_eq!((t.print_fan_rpm, t.hotend_fan_rpm), (None, None));
        assert_eq!(t.flow_percent, Some(95.0));
        assert_eq!(t.material.as_deref(), Some("PETG"));
        assert_eq!(t.job.map(|j| j.done), Some(40));
        // A flow of 0 is no reading (`inbound.go:287-293`).
        assert_eq!(
            first(serde_json::json!({"state": "IDLE", "flow": 0})).flow_percent,
            None
        );
    }

    /// Buddy 6.3.0's `slot` object (render.cpp:222-247) for a machine with several tools:
    /// `slot.<n>` (from 1) with `temp`, and `active` the tool in use. A CORE One's chamber and
    /// an XL's enclosure (render.cpp:178-199).
    #[test]
    fn the_slot_object_of_6_3_0_lists_the_tools() {
        let report: Value = serde_json::from_str(
            r#"{"temp_nozzle":215.3,"axis_z":1.2,
                "slot":{"1":{"material":"PLA","temp":30.0,"fan_hotend":0.0,"fan_print":0.0},
                        "2":{"material":"PETG","temp":215.3,"fan_hotend":7100.0,"fan_print":5200.0},
                        "state":0,"command":"-","active":2},"state":"PRINTING"}"#,
        )
        .unwrap();
        let t = first(report);
        assert_eq!(t.tools.len(), 2);
        assert_eq!(
            (t.tools[0].number, t.tools[0].nozzle_c, t.tools[0].active),
            (1, Some(30.0), false)
        );
        assert_eq!(
            (t.tools[1].number, t.tools[1].nozzle_c, t.tools[1].active),
            (2, Some(215.3), true)
        );
        let core: Value = serde_json::from_str(
            r#"{"chamber":{"temp":31.5,"target_temp":40,"fan_1_rpm":0,"fan_2_rpm":0,"fan_pwm_target":0,"led_intensity":100},"state":"IDLE"}"#,
        )
        .unwrap();
        let c = first(core);
        assert_eq!((c.chamber_c, c.chamber_target_c), (Some(31.5), Some(40.0)));
        assert!(c.tools.is_empty(), "no slot: no list");
        let xl: Value = serde_json::from_str(
            r#"{"enclosure":{"temp":28,"fan_rpm":0,"time_in_use":3600},"state":"IDLE"}"#,
        )
        .unwrap();
        assert_eq!(first(xl).chamber_c, Some(28.0));
        // A Reduced frame keeps the reading; a full one with neither block: switched off
        // (`telemetrychamber.go:43-45`).
        let kept = take_in(&serde_json::json!({"state": "IDLE"}), Some(c.clone()));
        assert_eq!(kept.chamber_c, Some(31.5));
        let off = take_in(
            &serde_json::json!({"state": "IDLE", "temp_bed": 22.0}),
            Some(c),
        );
        assert_eq!(off.chamber_c, None);
    }

    /// INFO as Buddy 6.3.0 writes it (render.cpp:340-401): the nozzle at the top level, the
    /// material per tool, `"---"` for none; `printer_type` only as the string; master's
    /// `features` and `material_package_instance` are not read.
    #[test]
    fn info_of_6_3_0_is_read_as_buddy_writes_it() {
        let info: Value = serde_json::from_str(
            r#"{"firmware":"6.3.0+0.LOCAL","printer_type":"2.1.0","sn":"CZPX1234","appendix":false,
                "fingerprint":"abc","nozzle_diameter":0.60,"transfer_paused":false,
                "tools":{"1":{"nozzle_diameter":0.6,"high_flow":false,"hardened":false,"material":"---"},
                         "2":{"nozzle_diameter":0.6,"high_flow":false,"hardened":false,"material":"PETG"}},"slots":2}"#,
        )
        .unwrap();
        let details = to_details(&info);
        assert_eq!(details.nozzle_mm, Some(0.6));
        assert_eq!(details.material.as_deref(), Some("PETG"));
        assert_eq!(details.printer_type.as_deref(), Some("2.1.0"));
        let master: Value = serde_json::from_str(
            r##"{"firmware":"8.0.1+12","type":1,"version":3,"subversion":1,
                "tools":{"0":{"type":"FFF_PRINT_HEAD","features":{"nozzle_diameter":0.60},
                  "material_package_instance":{"package":{"material":{"abbreviation":"PETG"}}}}}}"##,
        )
        .unwrap();
        let d = to_details(&master);
        assert_eq!(
            (d.nozzle_mm, d.material, d.printer_type),
            (None, None, None)
        );
    }

    /// The Prusa Connect SDK itself (the reference client) registers, is paired, gets its token,
    /// links over `/p/ws`, reports, and runs the G-code the hub gives it. Needs the SDK's Python:
    /// `PRUSA_SDK_PYTHON` (a venv with `pip install -e reference/Prusa-Connect-SDK-Printer`).
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_prusa_sdk_links_over_p_ws(pool: sqlx::PgPool) {
        use tokio::io::AsyncBufReadExt;
        let Ok(python) = std::env::var("PRUSA_SDK_PYTHON") else {
            eprintln!("PRUSA_SDK_PYTHON not set: the SDK run is skipped");
            return;
        };
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let script = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/prusa-sdk/link.py"
        );
        let mut child = tokio::process::Command::new(python)
            .args([script, "127.0.0.1", &port.to_string()])
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let mut next = async || {
            tokio::time::timeout(std::time::Duration::from_secs(30), lines.next_line())
                .await
                .expect("the SDK went quiet")
                .unwrap()
                .expect("the SDK ended")
        };
        let code = next()
            .await
            .strip_prefix("CODE ")
            .expect("a code first")
            .to_owned();
        let space = store.default_space().await.unwrap().0;
        let (machine, _) = store.pair(space, &code).await.unwrap().expect("paired");
        assert_eq!(next().await, "TOKEN");
        assert_eq!(next().await, "LINKED");
        let id =
            store
                .lock()
                .unwrap()
                .queue_command(Some("prusa-connect"), &machine, "test", "M115");
        assert!(id.is_some(), "the hub takes a command for it");
        assert_eq!(
            next().await,
            "GOT M115",
            "the G-code reached the SDK over /p/ws"
        );
    }
}
