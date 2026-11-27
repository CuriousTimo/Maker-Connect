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
//!    WebSocket for everything after registration, as Buddy 8.0.2 dials it
//!    (`reference/buddy-8.0.2` `src/connect/connect.cpp:151-189`) and the Connect SDK dials it
//!    (`reference/Prusa-Connect-SDK-Printer` branch `feat/ws-connect-local` @ `04c5bd59`,
//!    `prusa/connect/printer/ws.py`). Up: a text frame is one JSON object, an event when it has an
//!    `event` key (Buddy writes it on every event, `render.cpp:854`), else telemetry; each is taken
//!    in exactly as its HTTP twin above. Down: a command as `J` (JSON) or `G` (G-code), its id as 8
//!    hex digits, then the body (the SDK's `handle_ws_message`, "BuddyEncoder"). A Buddy printer
//!    fetches a file over it: a text frame `{"transfer":"inline", ...}` asks for a segment, and
//!    the bytes go back in `T` frames (`inline_download.rs`, `segment`). A printer shows online
//!    while it is heard (`Legacy::linked`); a newer socket of it ends the older (`Attached`).

use crate::store::{CommandState, Details, Network, Storage};
use crate::{Store, failed};
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use maker_connect_protocol::{Identity, JobProgress, MachineKind, MachineState, Telemetry};
use serde_json::Value;

/// Logs what a printer asks under `/p/` (and anything not found), its secrets left out:
/// how we learn what a firmware expects.
pub async fn log(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let path = request.uri().path().to_owned();
    let watched = path.starts_with("/p/");
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
        .route(
            "/p/teams/{team}/files/{hash}/raw",
            axum::routing::get(raw_file),
        )
}

/// A file of the printer's space, as the SDK fetches it for START_CONNECT_DOWNLOAD
/// (`__init__.py:468`: `/p/teams/{team_id}/files/{hash}/raw`), with its `Token`.
async fn raw_file(
    State(store): State<Store>,
    headers: HeaderMap,
    axum::extract::Path((team, hash)): axum::extract::Path<(i64, String)>,
) -> Response {
    let token = header(&headers, "token");
    match store.placed(token).await {
        Ok(Some((_, space))) if space == team => {}
        Ok(_) => return StatusCode::FORBIDDEN.into_response(),
        Err(error) => return failed(error),
    }
    match store.file(team, &hash).await {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => return failed(error),
    }
    match tokio::fs::read(store.file_path(&hash)).await {
        Ok(bytes) => (StatusCode::OK, bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A label printer on the protocol (the agent's DYMO): a job waiting for it goes out as
/// START_CONNECT_DOWNLOAD with `printing` and `tape_size_mm` (docs/design/wire.md, "The first
/// device"), when it is idle, nothing else is in flight and no label of it is still open
/// ("When the hub calls a label printed").
async fn label_job(
    store: &Store,
    token: &str,
    machine: &crate::store::Machine,
    space: i64,
) -> Result<(), crate::store::BoxError> {
    {
        let store = store.lock().unwrap();
        if !store.open_commands(&machine.id).is_empty()
            || store.label_jobs.values().any(|j| j.token == token)
        {
            return Ok(());
        }
    }
    let of = crate::agent_of(store, token).await?;
    let report = Telemetry {
        state: MachineState::Idle,
        ..Telemetry::default()
    };
    let (Some(order), _) = crate::agent_job(store, &of, &report).await? else {
        return Ok(());
    };
    let hash = order
        .download
        .trim_start_matches("/files/")
        .trim_end_matches("/download")
        .to_owned();
    let mut kwargs = serde_json::json!({
        "path": format!("/labels/{}.png", order.id),
        "team_id": space,
        "hash": hash,
        "printing": true,
    });
    // The tape printer's `tape_size_mm`, the CUPS driver's `page_size` (the roll set as loaded;
    // none: the PPD's default), docs/design/prusa-sdk-extension.md parts 2 and 3.
    let labelwriter = machine
        .profile
        .as_deref()
        .and_then(crate::profiles::find)
        .is_some_and(|p| p.firmware.as_deref() == Some("dymo-lw"));
    if labelwriter {
        if let Some(page_size) = &order.page_size {
            kwargs["page_size"] = page_size.clone().into();
        }
    } else {
        kwargs["tape_size_mm"] = order.tape_mm.into();
    }
    let command = serde_json::json!({"command": "START_CONNECT_DOWNLOAD", "kwargs": kwargs});
    let mut legacy = store.lock().unwrap();
    if let Some(id) = legacy.queue_connect_command(
        Some(&machine.link),
        &machine.id,
        "Print the label",
        &command,
        false,
    ) {
        legacy.label_jobs.insert(
            id,
            crate::store::LabelJob {
                space,
                job: order.id,
                token: token.to_owned(),
                downloaded: false,
                error: None,
                stop_sent: false,
            },
        );
    }
    Ok(())
}

/// A Marlin printer's type on the agent (`docs/design/prusa-sdk-extension.md`, part 9).
pub(crate) const MARLIN_AGENT: &str = "marlin.fdm";

/// Whether a machine is a Marlin printer on the agent: its files go to the agent's `gcodes`
/// storage by the SDK's `START_CONNECT_DOWNLOAD`, not to a stick by Buddy's download over its socket.
pub(crate) fn marlin_agent(machine: &crate::store::Machine) -> bool {
    machine
        .details
        .as_ref()
        .and_then(|d| d.printer_type.as_deref())
        == Some(MARLIN_AGENT)
}

/// A GRBL laser's type on the agent (`docs/design/prusa-sdk-extension.md`, part 4).
pub(crate) const LASER_AGENT: &str = "grbl.laser";

/// The device that carries an agent's USB cameras (`docs/design/prusa-sdk-extension.md`, part 5):
/// not a machine ([`MachineKind::Cameras`]).
pub(crate) const CAMERAS_AGENT: &str = "maker-connect.cameras";

/// Whether a machine is a GRBL laser on the agent: its controls go as Part 4's commands.
pub(crate) fn laser_agent(machine: &crate::store::Machine) -> bool {
    machine
        .details
        .as_ref()
        .and_then(|d| d.printer_type.as_deref())
        == Some(LASER_AGENT)
}

/// Part 4's settings in a laser's `INFO`: `max_power` (`$30`), `laser_mode` (`$32`), `travel_x`,
/// `travel_y`, `travel_z` (`$130`-`$132`); None when it told none of them.
pub(crate) fn grbl_settings(info: &Value) -> Option<maker_connect_protocol::GrblSettings> {
    let travel = match (
        info["travel_x"].as_f64(),
        info["travel_y"].as_f64(),
        info["travel_z"].as_f64(),
    ) {
        (Some(x), Some(y), Some(z)) => Some([x, y, z]),
        _ => None,
    };
    let s = maker_connect_protocol::GrblSettings {
        laser_mode: info["laser_mode"].as_bool(),
        max_power: info["max_power"].as_f64(),
        travel_mm: travel,
    };
    (s != maker_connect_protocol::GrblSettings::default()).then_some(s)
}

/// A label job a person stopped: `STOP_TRANSFER` while it downloads, `STOP_PRINT` once it prints
/// (docs/design/wire.md, "The second device"), once.
async fn stop_label(
    store: &Store,
    token: &str,
    machine: &crate::store::Machine,
) -> Result<(), crate::store::BoxError> {
    let open = {
        let store = store.lock().unwrap();
        store
            .label_jobs
            .iter()
            .find(|(_, j)| j.token == token && !j.stop_sent)
            .map(|(id, j)| (*id, j.clone()))
    };
    let Some((id, job)) = open else {
        return Ok(());
    };
    let state = store.job(job.space, job.job).await?.map(|j| j.state);
    if state != Some(crate::store::JobState::Stopping) {
        return Ok(());
    }
    let command = if job.downloaded {
        serde_json::json!({"command": "STOP_PRINT"})
    } else {
        serde_json::json!({"command": "STOP_TRANSFER"})
    };
    let mut legacy = store.lock().unwrap();
    legacy.queue_connect_command(
        Some(&machine.link),
        &machine.id,
        "Stop the label",
        &command,
        false,
    );
    if let Some(job) = legacy.label_jobs.get_mut(&id) {
        job.stop_sent = true;
    }
    Ok(())
}

/// The subprotocol Buddy asks for (`connect.cpp:169`).
const SUBPROTOCOL: &str = "prusa-connect";
// No ping and no read deadline from the hub (owner, 2026-11-24, #212): Buddy pings after 15 s of
// sending nothing (`connect.cpp:49-50`, `:828-829`) and answers a server's ping
// (`connect.cpp:386-404`); the agent's SDK pings every 30 s (`ws.py:62-63`).

/// The upgrade, refused with `401` when the credentials are missing or unknown and `403` when the
/// hub does not take the printer (not paired into a space): Buddy reads either as an
/// authentication error (`connect.cpp:625-627`, `:777-779`); which one is ours.
async fn ws(
    State(store): State<Store>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    upgrade: axum::extract::ws::WebSocketUpgrade,
) -> Response {
    let fingerprint = header(&headers, "fingerprint");
    let token = header(&headers, "token").to_owned();
    // Which socket this is, numbered as the upgrades arrive: the newest of a printer's ends the
    // older ones (`Attached`).
    let order = NEXT_SOCKET.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
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
    let machine = match store.placed(&token).await {
        Ok(Some((machine, _))) => machine,
        Ok(None) => {
            return (
                StatusCode::FORBIDDEN,
                "the host does not accept this printer\n",
            )
                .into_response();
        }
        Err(error) => return failed(error),
    };
    // The newest socket of the printer from here on, before the upgrade is answered: an older
    // socket takes no command once this one is on its way (`Attached::replaced`).
    {
        let mut legacy = store.lock().unwrap();
        let newest = legacy
            .newest_link
            .get(&machine)
            .copied()
            .unwrap_or(0)
            .max(order);
        legacy.newest_link.insert(machine, newest);
    }
    upgrade
        .protocols([SUBPROTOCOL])
        .on_upgrade(move |socket| link(store, headers, extensions, token, order, socket))
}

/// The number of the last `/p/ws` upgrade ([`ws`]).
static NEXT_SOCKET: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    order: u64,
    mut socket: axum::extract::ws::WebSocket,
) {
    use axum::extract::ws::Message;
    // Counted while the socket is open; online while it is heard (`Legacy::linked`, ours: what
    // the page shows).
    // A command goes out as soon as it is queued (Buddy takes a command over the socket at any
    // time, `connect.cpp:386-420`): the store's change signal wakes the link. Subscribed before
    // the socket counts, so that what its start queues (a command sent again, `Attached::new`)
    // wakes it.
    let mut changes = store.subscribe();
    let (attached, machine_id) = match store.caller(&token).await {
        Ok(Some(caller)) => (
            Attached::new(&store, caller.id.clone(), order),
            Some(caller.id),
        ),
        _ => return,
    };
    // The downloads asked for on this socket: Buddy's `file_id` for each, and the file it is
    // (the `team_id` and `hash` of its first request, `inline_download.rs`).
    let mut downloads: std::collections::HashMap<u32, (i64, String)> = Default::default();
    // When the page is told the printer went quiet ([`crate::store::SILENT_SECONDS`]); the
    // socket stays open, it only takes no command until the printer is heard again (D12).
    let quiet = std::time::Duration::from_secs(crate::store::SILENT_SECONDS as u64 + 1);
    let mut quiet_at = tokio::time::Instant::now() + quiet;
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { break };
                // A newer socket of this printer: this one ends; nothing it says is taken and
                // nothing is answered on it (owner, 2026-11-24, B).
                if attached.replaced() {
                    break;
                }
                quiet_at = tokio::time::Instant::now() + quiet;
                if let Some(id) = &machine_id {
                    let mut legacy = store.lock().unwrap();
                    let was = legacy.linked(id);
                    legacy.last_frame.insert(id.clone(), crate::store::now());
                    // Heard again after a silence: shown online at once.
                    if !was {
                        legacy.changed();
                    }
                }
                let Message::Text(text) = message else { continue };
                let body = text.to_string();
                let parsed = serde_json::from_str::<Value>(&body).ok();
                if let Some(asked) = parsed.as_ref().and_then(crate::devices::inline_download::request) {
                    let frames = segment(&store, &token, &mut downloads, asked).await;
                    let mut sent = true;
                    for frame in frames {
                        if socket.send(Message::Binary(frame.into())).await.is_err() {
                            sent = false;
                            break;
                        }
                    }
                    if !sent {
                        break;
                    }
                    continue;
                }
                let is_event = parsed.is_some_and(|v| v.get("event").is_some());
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
            _ = tokio::time::sleep_until(quiet_at) => {
                store.lock().unwrap().changed();
                quiet_at = tokio::time::Instant::now() + std::time::Duration::from_secs(24 * 3600);
            }
            changed = changes.recv() => {
                if matches!(changed, Err(tokio::sync::broadcast::error::RecvError::Closed)) {
                    break;
                }
                // A newer socket of this printer: this one ends, and takes no command.
                if attached.replaced() {
                    break;
                }
                // Silent for more than `SILENT_SECONDS` (a socket that died without closing):
                // no command goes into it; it waits for the printer to be heard again, or for its
                // next socket (owner, 2026-11-24, B).
                if machine_id.as_ref().is_some_and(|id| !store.lock().unwrap().linked(id)) {
                    continue;
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
        }
    }
}

/// The `T` frames that answer a segment request (`inline_download.rs`): the segment's bytes, or
/// one empty frame (audit D9: Buddy marks the download failed and drops its socket; it then ends
/// the download or, after the reconnect, tries it again, a first request with the details again;
/// which comes first is not checked) when the request is broken, names a `file_id` without a first
/// request on this socket, names no file this printer may have, or the file has no such bytes. A
/// request without a `file_id` gets no frame.
pub(crate) async fn segment(
    store: &Store,
    token: &str,
    downloads: &mut std::collections::HashMap<u32, (i64, String)>,
    asked: Result<crate::devices::inline_download::Request, Option<u32>>,
) -> Vec<Vec<u8>> {
    use crate::devices::inline_download::{failed, frames};
    let request = match asked {
        Ok(request) => request,
        Err(file_id) => return file_id.map(failed).into_iter().collect(),
    };
    let id = request.file_id;
    if let Some(first) = request.first {
        downloads.insert(id, (first.1, first.0));
    }
    let Some((team, hash)) = downloads.get(&id).cloned() else {
        return vec![failed(id)];
    };
    let allowed = matches!(store.placed(token).await, Ok(Some((_, space))) if space == team)
        && matches!(store.file(team, &hash).await, Ok(Some(_)));
    if !allowed {
        return vec![failed(id)];
    }
    match read_segment(&store.file_path(&hash), request.start, request.end).await {
        Some(bytes) => frames(id, &bytes, request.chunk),
        None => vec![failed(id)],
    }
}

/// The bytes `start..=end` of a file, when the file has them all.
async fn read_segment(path: &std::path::Path, start: u64, end: u64) -> Option<Vec<u8>> {
    use crate::devices::inline_download::segment_len;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut file = tokio::fs::File::open(path).await.ok()?;
    let size = file.metadata().await.ok()?.len();
    let len = segment_len(start, end, size)?;
    file.seek(std::io::SeekFrom::Start(start)).await.ok()?;
    let mut bytes = vec![0; usize::try_from(len).ok()?];
    file.read_exact(&mut bytes).await.ok()?;
    Some(bytes)
}

/// A printer's `/p/ws` socket, counted in [`crate::store::Legacy::attached`] while it is open.
struct Attached {
    store: Store,
    machine: String,
    /// Which of the printer's sockets this is ([`crate::store::Legacy::newest_link`]).
    link: u64,
}

impl Attached {
    fn new(store: &Store, machine: String, order: u64) -> Self {
        let mut legacy = store.lock().unwrap();
        *legacy.attached.entry(machine.clone()).or_default() += 1;
        legacy
            .last_frame
            .insert(machine.clone(), crate::store::now());
        // The newest socket of this printer, by when its upgrade came ([`ws`]); an older one
        // ends when it next wakes (`link`).
        let link = order;
        let newest = legacy
            .newest_link
            .get(&machine)
            .copied()
            .unwrap_or(0)
            .max(order);
        legacy.newest_link.insert(machine.clone(), newest);
        // What went into an older socket and was never answered goes again on this one (D12).
        if link == newest {
            legacy.send_unanswered_again(&machine);
        }
        legacy.changed();
        drop(legacy);
        Self {
            store: store.clone(),
            machine,
            link,
        }
    }

    /// Whether a newer socket of the same printer took over.
    fn replaced(&self) -> bool {
        self.store.lock().unwrap().newest_link.get(&self.machine) != Some(&self.link)
    }
}

impl Drop for Attached {
    fn drop(&mut self) {
        let mut legacy = self.store.lock().unwrap();
        if let Some(n) = legacy.attached.get_mut(&self.machine) {
            *n -= 1;
            if *n == 0 {
                legacy.attached.remove(&self.machine);
                legacy.last_frame.remove(&self.machine);
            }
        }
        legacy.changed();
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

/// The printer type as a profile knows it: Buddy 8.0.2 writes `"2.1.0"` as one string
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
    // A Marlin printer on the agent (`docs/design/prusa-sdk-extension.md`, part 9): no profile
    // knows its model from the type; the space chooses one on its Machine tab.
    let marlin = identity.printer_type.as_deref() == Some(MARLIN_AGENT);
    // A GRBL laser on the agent (part 4): a laser; no profile knows its model from the type.
    let laser = identity.printer_type.as_deref() == Some(LASER_AGENT);
    // An agent's cameras (part 5): a device, no machine.
    let cameras = identity.printer_type.as_deref() == Some(CAMERAS_AGENT);
    let fallback = if marlin {
        "Marlin printer"
    } else if laser {
        "GRBL laser"
    } else if cameras {
        "Cameras"
    } else {
        // No profile knows the type (owner, 2026-11-24: "Prusa printer" is no good fallback): the
        // type as the device gives it.
        identity.printer_type.as_deref().unwrap_or("Unknown device")
    };
    let model = profile.map_or(fallback, |p| p.model.as_str());
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
    let kind = match profile {
        Some(p) => p.kind,
        None if laser => MachineKind::Laser,
        None if cameras => MachineKind::Cameras,
        None => MachineKind::Fdm,
    };
    let agent_version = if kind == MachineKind::Fdm && !marlin {
        format!("Prusa firmware {firmware} (built-in Connect)").replace("  ", " ")
    } else {
        format!("Maker Connect agent {firmware}")
    };
    let registered: sqlx::Result<_> = async {
        let (id, token, code) = store
            .register_linked(&name, kind, &agent_version, "prusa-connect")
            .await?;
        if let Some(profile) = profile {
            store.recognise(&token, &profile.id).await?;
        }
        if marlin || laser {
            let details = Details {
                serial: Some(serial.clone()).filter(|s| !s.is_empty()),
                printer_type: identity.printer_type.clone(),
                ..Details::default()
            };
            store.set_details(&token, &details).await?;
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

/// A telemetry frame taken in over the machine's last telemetry: a frame writes only the members
/// it carries, and everything it leaves out stays as it was. Buddy sends a Reduced frame most of
/// the time, without the temperatures, targets, speed, flow and position, and a Full one only when
/// its fingerprint changes or every 5 minutes (Buddy 8.0.2 `planner.cpp:111-113`, `:537-557`;
/// `render.cpp:177`), so the last Full frame's values stand until the next.
///
/// - The job's members (`progress`, `time_printing`, `time_remaining`) are kept when a frame leaves
///   them out, also after the print (ours: what a page shows of the last print);
///   [`current_job`] shows a job only while the frame names it (`job_id`). The file never comes
///   here (JOB_INFO tells it).
/// - Each axis is kept as told last: while it prints Buddy tells only `axis_z`
///   (`render.cpp:212-217`), so X and Y stay as told last, and unknown when never told.
/// - The tools: a Buddy printer's as Buddy 8.0.2 writes them, a device on the agent's as
///   PrusaLink sends them (`crate::devices::connect_tools`).
/// - A transfer is in every frame, Reduced or Full, while one runs (`render.cpp:146-159`, before
///   the `Mode::Full` part): a frame without `transfer_id` has none, so none is kept.
fn take_in(
    report: &Value,
    last: Option<Telemetry>,
    session: &mut crate::devices::connect_tools::Session,
    agent: bool,
) -> Telemetry {
    let mut t = last.unwrap_or_default();
    // A member the frame carries, as a number (a null: none).
    let carried = |key: &str| report.get(key).map(|v| v.as_f64().map(|n| n as f32));
    let set = |field: &mut Option<f32>, key: &str| {
        if let Some(v) = carried(key) {
            *field = v;
        }
    };
    // A label printer (the agent's DYMO, docs/design/wire.md "The first device"): whether a
    // cassette is in, its charge and its status bytes, as the agent reads them.
    if let Some(v) = report.get("media_loaded") {
        t.cassette = v.as_bool();
    }
    if let Some(v) = report.get("battery_percent") {
        t.battery_percent = v.as_u64().map(|b| b.min(100) as u8);
    }
    if let Some(v) = report.get("raw_status") {
        t.raw_status = v.as_str().map(str::to_owned);
    }
    // A label printer on DYMO's CUPS driver (docs/design/prusa-sdk-extension.md, part 3): the
    // reason of its last `STATE_CHANGED`, the PPD's own text, is its problem.
    if let Some(reason) = &session.label_reason {
        t.problem = reason.clone();
    }
    // A GRBL laser on the agent (part 4): its settings are in its `INFO`.
    if let Some(grbl) = &session.grbl {
        t.grbl = Some(grbl.clone());
    }
    // A file on its way: `transfer_id`, `transfer_transferred`, `transfer_time_remaining`,
    // `transfer_progress` (`render.cpp:155-158`).
    t.transfer =
        report["transfer_id"]
            .as_u64()
            .map(|id| maker_connect_protocol::TransferProgress {
                id,
                transferred: report["transfer_transferred"].as_u64().unwrap_or(0),
                percent: report["transfer_progress"].as_f64().unwrap_or(0.0) as f32,
                remaining_s: report["transfer_time_remaining"].as_u64().unwrap_or(0),
            });
    // The state comes in every frame (`render.cpp:303`).
    t.state = match report["state"].as_str().unwrap_or("") {
        "PRINTING" | "BUSY" => MachineState::Busy,
        "PAUSED" => MachineState::Paused,
        "ATTENTION" => MachineState::Attention,
        "ERROR" => MachineState::Error,
        _ => MachineState::Idle,
    };
    match report.get("progress").map(Value::as_f64) {
        Some(Some(percent)) => {
            t.job
                .get_or_insert_with(|| JobProgress {
                    file: String::new(),
                    done: 0,
                    total: 100,
                    elapsed_s: None,
                    remaining_s: None,
                })
                .done = percent.clamp(0.0, 100.0) as u64;
        }
        // `/job/progress` removed: no progress to show.
        Some(None) => t.job = None,
        None => {}
    }
    if let Some(job) = t.job.as_mut() {
        if let Some(v) = report.get("time_printing") {
            job.elapsed_s = v.as_u64();
        }
        if let Some(v) = report.get("time_remaining") {
            job.remaining_s = v.as_u64();
        }
    }
    set(&mut t.bed_c, "temp_bed");
    set(&mut t.bed_target_c, "target_bed");
    set(&mut t.speed_percent, "speed");
    // A flow of 0 is none read: Buddy's `flow_factor` is 0 until a tool is selected and its flow
    // read (`printer.hpp:175`, `marlin_printer.cpp:339-341`).
    if let Some(flow) = carried("flow") {
        t.flow_percent = flow.filter(|f| *f != 0.0);
    }
    let axes = [carried("axis_x"), carried("axis_y"), carried("axis_z")];
    if axes.iter().any(Option::is_some) {
        let mut position = t.position_mm.unwrap_or([None; 3]);
        for (at, axis) in position.iter_mut().zip(axes) {
            if let Some(v) = axis {
                *at = v;
            }
        }
        t.position_mm = position.iter().any(Option::is_some).then_some(position);
    }
    // The CORE One's chamber (`chamber.temp`, `target_temp`, every Full frame, `render.cpp:202-210`)
    // or the XL's enclosure (`enclosure.temp`, only while one is present, `:193-200`), as Buddy
    // writes them; a Full frame (it carries `temp_bed`) with neither block has none. Not on the
    // MINI.
    let number = |block: &Value, key: &str| block[key].as_f64().map(|n| n as f32);
    if let Some(chamber) = report.get("chamber") {
        if let Some(temp) = number(chamber, "temp") {
            t.chamber_c = Some(temp);
        }
        if let Some(target) = number(chamber, "target_temp") {
            t.chamber_target_c = Some(target);
        }
    } else if let Some(enclosure) = report.get("enclosure") {
        if let Some(temp) = number(enclosure, "temp") {
            t.chamber_c = Some(temp);
        }
    } else if report.get("temp_bed").is_some() {
        t.chamber_c = None;
    }
    session.telemetry(&mut t, report, agent);
    // The job, in every frame while there is one (`crate::devices::connect_job`).
    t.job_id = report["job_id"].as_i64();
    t
}

/// The job as a reader of the machine sees it: the stored telemetry keeps the job's progress after
/// the print ([`take_in`]), and a print is current only while the frame names its job
/// ([`crate::devices::connect_job::is_running`]: Buddy's and PrusaLink's `job_id`). Every reader of
/// [`crate::store::Machine`] goes through this (`MachineRow::into_machine`), and so does the
/// following of a print the printer runs itself (`telemetry`); what a job used is counted off the
/// stored telemetry instead.
pub(crate) fn current_job(t: &mut Telemetry) {
    if !crate::devices::connect_job::is_running(t) {
        t.job = None;
    }
}

/// One storage as an `INFO` `storages` entry names it, and as the SDK's MEDIUM_INSERTED carries it
/// (`Storage.to_dict`, `Prusa-Connect-SDK-Printer` `files.py:311-324`, sent by `attach`, :398-401).
fn storage_of(s: &Value) -> Storage {
    let some = |value: &Value| value.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    Storage {
        kind: some(&s["type"]).unwrap_or_default(),
        path: some(&s["mountpoint"]).unwrap_or_default(),
        free_bytes: s["free_space"].as_u64(),
        read_only: s["read_only"].as_bool().unwrap_or(false),
    }
}

/// A medium that came or went (the SDK's `Filesystem.attach` and `detach`, `files.py:398-419`):
/// MEDIUM_INSERTED's storage added, or put in place of the one with its `mountpoint`;
/// MEDIUM_EJECTED's `mountpoint` taken away. Ours: kept in the details INFO gave.
async fn medium(shared: &Store, token: &str, name: &str, data: &Value) -> sqlx::Result<bool> {
    let Some(mut details) = shared.details_of(token).await? else {
        return shared.knows(token).await;
    };
    let mountpoint = data["mountpoint"].as_str().unwrap_or_default();
    details.storages.retain(|s| s.path != mountpoint);
    if name == "MEDIUM_INSERTED" && !mountpoint.is_empty() {
        details.storages.push(storage_of(data));
    }
    shared.set_details(token, &details).await
}

/// What an `INFO` event tells about the printer, its secrets (the Prusa Link key) left out.
pub(crate) fn to_details(info: &Value) -> Details {
    let some = |value: &Value| value.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let network = &info["network_info"];
    Details {
        serial: some(&info["sn"]),
        // Buddy's `printer_type`; the SDK's `get_info` names it `type` (the agent's devices).
        printer_type: printer_type(&info["printer_type"]).or_else(|| printer_type(&info["type"])),
        firmware: some(&info["firmware"]),
        // At the top level of INFO as PrusaLink sends it for the agent's printers
        // (`Prusa-Link` `sdk_augmentation/printer.py:55`; owner, 2026-11-24: the fallback stays),
        // else per head as Buddy 8.0.2 writes it under `tools.<id>.features` (`render.cpp:455-456`).
        nozzle_mm: info["nozzle_diameter"].as_f64().or_else(|| {
            info["tools"]
                .as_object()?
                .values()
                .find_map(|t| t["features"]["nozzle_diameter"].as_f64())
        }),
        // Buddy 8.0.2: `tools.<id>.material_package_instance.package.material.abbreviation`, written
        // only when the spool has a name (`render.cpp:493-532`); a null instance is no spool.
        material: info["tools"].as_object().and_then(|tools| {
            tools.values().find_map(|t| {
                t["material_package_instance"]["package"]["material"]["abbreviation"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            })
        }),
        storages: info["storages"]
            .as_array()
            .map(|storages| storages.iter().map(storage_of).collect())
            .unwrap_or_default(),
        // As Buddy 8.0.2 writes it (`render.cpp:413-431`): a MAC as `JSON_MAC`, upper-case pairs
        // with colons, an IPv4 as `JSON_IP`, dotted (`render.cpp:51-52`), the SSID and hostname as
        // text; a key it leaves out (an interface that is down) is none.
        network: network.is_object().then(|| Network {
            hostname: some(&network["hostname"]),
            lan_ipv4: some(&network["lan_ipv4"]),
            lan_mac: some(&network["lan_mac"]),
            wifi_ipv4: some(&network["wifi_ipv4"]),
            wifi_mac: some(&network["wifi_mac"]),
            wifi_ssid: some(&network["wifi_ssid"]),
        }),
        // Part 6a of the extension: 32 lower-case hex digits, or not the agent's id.
        agent_id: some(&info["agent_id"]).filter(|id| {
            id.len() == 32
                && id
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        }),
        extensions: info["extensions"]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .filter_map(|n| n.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        // Set from the request's header by the caller (part 6a).
        agent_release: None,
        // Part 9: what a Marlin printer answered M115.
        firmware_info: info["firmware_info"].as_object().cloned(),
        firmware_capabilities: info["firmware_capabilities"].as_object().cloned(),
        firmware_area: info["firmware_area"].as_object().cloned(),
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
    // answered failed, before this report is taken in (K18 (b), `docs/archive/plans/agent-engine.md`).
    let caller = match store.caller(token).await {
        Ok(Some(caller)) => caller,
        // An unknown token: refused, and nothing of it kept (no session).
        Ok(None) => {
            return (StatusCode::UNAUTHORIZED, "unknown printer: register again").into_response();
        }
        Err(error) => return failed(error),
    };
    let machine = caller.id.clone();
    let caller = Some(caller);
    store
        .lock()
        .unwrap()
        .fail_commands_of_gone_printer(caller.as_ref());
    // The last report, from the session when it has one (the frame before this one, under the
    // lock), else as stored (after a restart); one it cannot read back: taken in over nothing.
    let kept = store
        .lock()
        .unwrap()
        .connect_tools
        .get(&machine)
        .is_some_and(|s| s.last.is_some());
    let stored = if kept {
        None
    } else {
        store.last_telemetry(token).await.ok().flatten()
    };
    let job_id = report["job_id"].as_u64();
    // A printer runs Prusa's firmware; any other kind on this protocol, and a Marlin printer
    // (extension part 9), is our agent.
    let agent = matches!(
        store.machine_by_token(token).await,
        Ok(Some((m, _))) if m.kind != MachineKind::Fdm || marlin_agent(&m)
    );
    let telemetry = {
        let mut store = store.lock().unwrap();
        let file = store.printing_file.get(token).cloned();
        let session = store.connect_tools.entry(machine).or_default();
        let last = session.last.take().or(stored);
        let mut telemetry = take_in(&report, last, session, agent);
        // Every request says the firmware's version; none said, none shown (as before).
        let version = header(&headers, "user-agent-version");
        telemetry.firmware = (!version.is_empty()).then(|| {
            if agent {
                format!("Maker Connect agent {version}")
            } else {
                format!("Prusa firmware {version}")
            }
        });
        // The file it prints, as its JOB_INFO said for this print (not the last report's, which
        // may be the print before).
        if let Some(job) = telemetry.job.as_mut() {
            match file {
                Some((known, name)) if known.is_none() || known == job_id => job.file = name,
                _ if job_id.is_some() => job.file.clear(),
                _ => {}
            }
        }
        session.last = Some(telemetry.clone());
        telemetry
    };
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
            .follow_own_print(
                token,
                &{
                    let mut current = telemetry.clone();
                    current_job(&mut current);
                    current
                },
                report["state"].as_str(),
                agent,
            )
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
    // A label printer: its job goes out as a download to print.
    let label = match store.machine_by_token(token).await {
        Ok(Some((m, Some(space)))) if m.kind == MachineKind::Label => Some((m, space)),
        _ => None,
    };
    if let Ok(Some(_)) = reported
        && let Some((machine, _)) = label.as_ref()
        && let Err(error) = stop_label(&store, token, machine).await
    {
        return failed(error);
    }
    if let Ok(Some(_)) = reported
        && let Some((machine, space)) = label.as_ref()
        && report["state"].as_str() == Some("IDLE")
        && let Err(error) = label_job(&store, token, machine, *space).await
    {
        return failed(error);
    }
    // A job for it goes to its stick when it is free; one printing follows its state.
    if let Ok(Some(_)) = reported
        && label.is_none()
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

/// A `STATE_CHANGED` as one line for the hub's log: the state, then what Buddy 8.0.2 sends with
/// it, unchanged and only when it sends it: `reason`, `machine_reason` (`render.cpp:375-379`),
/// `dialog_id` (`:844-845`) and the dialog in `data`, `code`, `title`, `text`, `buttons`
/// (`:769-842`). Buddy sends the event when its state or dialog changes, so a frame whose
/// state stays gives no line. Nothing for another event.
fn state_line(event: &Value) -> Option<String> {
    if event["event"] != "STATE_CHANGED" {
        return None;
    }
    let mut line = event["state"].as_str().unwrap_or("?").to_owned();
    let data = &event["data"];
    for (key, value) in [
        ("reason", &event["reason"]),
        ("machine_reason", &event["machine_reason"]),
        ("dialog_id", &event["dialog_id"]),
        ("code", &data["code"]),
        ("title", &data["title"]),
        ("text", &data["text"]),
        ("buttons", &data["buttons"]),
    ] {
        if !value.is_null() {
            line += &format!(" {key}={value}");
        }
    }
    // Each value as JSON: a text's line breaks stay `\n`, so it is one line.
    Some(line)
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
    // A change of its state in the hub's log, with Buddy's reason and dialog (#226).
    if let (Some(line), Some(caller)) = (state_line(&event), caller.as_ref()) {
        println!("state of {}: {line}", caller.id);
    }
    // A label printer, whose `STATE_CHANGED` reason is its problem (part 3).
    let label = matches!(
        shared.machine_by_token(token).await,
        Ok(Some((m, _))) if m.kind == MachineKind::Label
    );
    let mut ended_label = None;
    let mut mesh = None;
    {
        let mut store = shared.lock().unwrap();
        // A label printer's reason, the PPD's text, when a `STATE_CHANGED` carries one (part 3);
        // a state change without one clears it.
        if name == "STATE_CHANGED"
            && label
            && let Some(caller) = caller.as_ref()
        {
            store
                .connect_tools
                .entry(caller.id.clone())
                .or_default()
                .label_reason = Some(event["reason"].as_str().map(str::to_owned));
        }
        if let Some(id) = event["command_id"].as_u64() {
            let state = match name.as_str() {
                "ACCEPTED" => Some(CommandState::Accepted),
                // A command that asked for something is done when the answer comes. Deleting a
                // file or folder and making a folder answer with FILE_CHANGED, not FINISHED
                // (Buddy 8.0.2's connect/planner.cpp:896: "the result is generated through the
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
                // What the printer answered a G-code (extension part 9, `gcode.output`).
                let output: Vec<String> = event["data"]["gcode_output"]
                    .as_array()
                    .map(|lines| {
                        lines
                            .iter()
                            .filter_map(|l| l.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                // Probe bed's answer: each line through BedLevelVisualizer, as its Update Mesh
                // (#107; docs/audits/bed-level-visualizer.md), the mesh kept when one completes.
                if state == CommandState::Finished
                    && let Some(caller) = caller.as_ref()
                    && store.command_label(&caller.id, id as u32)
                        == Some(crate::devices::legacy_gcode::PROBE_LABEL)
                {
                    let blv = store
                        .bed_level_visualizers
                        .entry(caller.id.clone())
                        .or_default();
                    blv.enable_mesh_collection();
                    for line in &output {
                        if let Some(rows) = blv.process_gcode(line) {
                            mesh = Some(rows);
                        }
                    }
                }
                store.command_went(caller.as_ref(), id as u32, state, reason.clone(), output);
                // A label's command refused fails it; its answer only says the download started.
                if state == CommandState::Rejected
                    && let Some(job) = store.label_jobs.remove(&(id as u32))
                {
                    ended_label = Some((
                        job,
                        crate::store::JobState::Failed,
                        Some(reason.unwrap_or_else(|| "the label printer refused it".to_owned())),
                    ));
                }
            }
        }
        // A label's download, and then its printer's state (docs/design/wire.md, "When the hub
        // calls a label printed").
        let of_transfer = event["data"]["start_cmd_id"]
            .as_u64()
            .map(|id| id as u32)
            .filter(|id| store.label_jobs.get(id).is_some_and(|j| j.token == token));
        let downloaded = store
            .label_jobs
            .iter()
            .find(|(_, j)| j.token == token && j.downloaded)
            .map(|(id, _)| *id);
        match (name.as_str(), of_transfer, downloaded) {
            ("TRANSFER_FINISHED", Some(id), _) => {
                if let Some(job) = store.label_jobs.get_mut(&id) {
                    job.downloaded = true;
                }
            }
            ("TRANSFER_ABORTED" | "TRANSFER_STOPPED", Some(id), _) => {
                if let Some(job) = store.label_jobs.remove(&id) {
                    ended_label = Some(if job.stop_sent && name == "TRANSFER_STOPPED" {
                        (job, crate::store::JobState::Stopped, None)
                    } else {
                        (job, crate::store::JobState::Failed, Some(name.clone()))
                    });
                }
            }
            // An `ERROR` fails it only when `IDLE` comes next: the CUPS driver waits and prints
            // again after one (docs/design/wire.md, "The second device").
            ("STATE_CHANGED", _, Some(id)) => match event["state"].as_str() {
                Some("ERROR") => {
                    if let Some(job) = store.label_jobs.get_mut(&id) {
                        job.error = Some(event["reason"].as_str().unwrap_or("").to_owned());
                    }
                }
                Some("IDLE") => {
                    if let Some(job) = store.label_jobs.remove(&id) {
                        let ended = match (&job.error, job.stop_sent) {
                            (Some(_), true) => (crate::store::JobState::Stopped, None),
                            (Some(reason), false) => (
                                crate::store::JobState::Failed,
                                (!reason.is_empty()).then(|| reason.clone()),
                            ),
                            (None, _) => (crate::store::JobState::Finished, None),
                        };
                        ended_label = Some((job, ended.0, ended.1));
                    }
                }
                Some(_) => {
                    if let Some(job) = store.label_jobs.get_mut(&id) {
                        job.error = None;
                    }
                }
                None => {}
            },
            _ => {}
        }
    }
    // The mesh Probe bed's answer gave.
    if let Some(rows) = mesh
        && let Err(error) = shared.set_mesh(token, rows).await
    {
        return failed(error);
    }
    // A label printed, or not.
    if let Some((job, state, reason)) = ended_label
        && let Err(error) = shared
            .set_job(job.space, job.job, state, reason.as_deref())
            .await
    {
        return failed(error);
    }
    // Files on its stick, and transfers to it.
    if let Err(error) = crate::printer_files::on_event(&shared, token, &name, &event).await {
        return failed(error);
    }
    // Only a known printer has a session (an unknown token is refused below).
    if name == "INFO"
        && let Some(caller) = caller.as_ref()
    {
        let mut store = shared.lock().unwrap();
        // Buddy 8.0.2 names the spool only in INFO (`render.cpp:493-531`): kept for the telemetry.
        store
            .connect_tools
            .entry(caller.id.clone())
            .or_default()
            .material = to_details(&event["data"]).material;
        if header(&headers, "user-agent-printer") == LASER_AGENT {
            let session = store.connect_tools.entry(caller.id.clone()).or_default();
            session.grbl = grbl_settings(&event["data"]).or(session.grbl.take());
        }
    }
    let known = match name.as_str() {
        "INFO" => {
            // A Marlin printer's model, by the M115 name it gives (extension part 9,
            // `MACHINE_TYPE` of `firmware_info`): a profile only when exactly one is named so.
            let identity = Identity {
                machine_type: event["data"]["firmware_info"]["MACHINE_TYPE"]
                    .as_str()
                    .map(str::to_owned),
                ..Identity::default()
            };
            // A laser's, by the `MODEL` of its `$I` answer (part 4, "What the laser is").
            let laser = event["data"]["firmware_info"]["MODEL"]
                .as_str()
                .and_then(crate::profiles::detect_laser);
            if let Some(profile) = crate::profiles::detect(&identity).or(laser)
                && let Err(error) = shared.recognise(token, &profile.id).await
            {
                eprintln!("prusa-connect: the profile of {}: {error}", profile.id);
            }
            let mut details = to_details(&event["data"]);
            // Part 6a: the agent's release is its `User-Agent-Version`, not `firmware`.
            details.agent_release = Some(header(&headers, "user-agent-version"))
                .filter(|v| !v.is_empty())
                .map(str::to_owned);
            shared.set_details(token, &details).await
        }
        "MEDIUM_INSERTED" | "MEDIUM_EJECTED" => medium(&shared, token, &name, &event["data"]).await,
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

    use crate::devices::connect_tools::Session;
    use serde_json::json;

    /// One printer's frames, taken in one after the other in one session, as `telemetry` does;
    /// `model` is its `User-Agent-Printer`.
    struct Printer {
        session: Session,
        last: Option<Telemetry>,
        /// A device on the agent (PrusaLink's keys), not a Buddy printer.
        agent: bool,
    }

    impl Printer {
        /// A Buddy printer (the model's name only tells the reader which).
        fn new(_model: &'static str) -> Self {
            Self {
                session: Session::default(),
                last: None,
                agent: false,
            }
        }

        /// A Marlin printer on the agent.
        fn agent() -> Self {
            Self {
                agent: true,
                ..Self::new("")
            }
        }

        fn frame(&mut self, report: Value) -> Telemetry {
            let t = take_in(&report, self.last.take(), &mut self.session, self.agent);
            self.last = Some(t.clone());
            t
        }

        /// An INFO event: the spool it names, as the handler keeps it.
        fn info(&mut self, data: Value) {
            self.session.material = to_details(&data).material;
        }
    }

    /// A Buddy printer's frame taken in over nothing before it.
    fn first(report: Value) -> Telemetry {
        Printer::new("").frame(report)
    }

    /// A MINI's `STATE_CHANGED` to ATTENTION with a dialog, as Buddy 8.0.2 writes it
    /// (`render.cpp:769-848`), is one line with all it sent; one without a dialog only its
    /// state; any other event none (#226).
    #[test]
    fn a_state_change_is_one_line_with_buddys_reason_and_dialog() {
        // The shape is Buddy's; the code, title, text and buttons are only examples.
        let attention = json!({"data": {"code": "12345", "title": "A title",
            "text": "A text,\non two lines", "buttons": ["Continue", "Abort"]},
            "dialog_id": 3, "state": "ATTENTION", "command_id": 9, "event": "STATE_CHANGED"});
        assert_eq!(
            state_line(&attention).as_deref(),
            Some(
                r#"ATTENTION dialog_id=3 code="12345" title="A title" text="A text,\non two lines" buttons=["Continue","Abort"]"#
            )
        );
        assert_eq!(
            state_line(
                &json!({"data": {}, "state": "IDLE", "reason": "ok", "event": "STATE_CHANGED"})
            )
            .as_deref(),
            Some(r#"IDLE reason="ok""#)
        );
        assert_eq!(
            state_line(&json!({"state": "IDLE", "event": "JOB_INFO"})),
            None
        );
    }

    /// A file on its way to a MINI on Buddy 8.0.2: the four `transfer_*` members of every frame,
    /// Reduced or Full, while one runs (`render.cpp:146-159`); 0 s left while it has no estimate
    /// (`monitor.cpp:258-267`); a frame without them (the transfer over) leaves none.
    #[test]
    fn a_transfer_is_shown_while_the_printer_reports_it() {
        let mut mini = Printer::new("MINI");
        let t = mini.frame(
            json!({"state": "IDLE", "transfer_id": 5, "transfer_transferred": 0,
            "transfer_time_remaining": 0, "transfer_progress": 0.0}),
        );
        assert_eq!(
            t.transfer,
            Some(maker_connect_protocol::TransferProgress {
                id: 5,
                transferred: 0,
                percent: 0.0,
                remaining_s: 0
            })
        );
        let t = mini.frame(
            json!({"state": "IDLE", "transfer_id": 5, "transfer_transferred": 590_000,
            "transfer_time_remaining": 10, "transfer_progress": 41.9}),
        );
        let transfer = t.transfer.unwrap();
        assert_eq!(
            (transfer.transferred, transfer.percent, transfer.remaining_s),
            (590_000, 41.9, 10)
        );
        let t = mini.frame(json!({"state": "IDLE"}));
        assert_eq!(t.transfer, None, "over: not kept from the frame before");
    }

    fn at(x: f32, y: f32, z: f32) -> Option<[Option<f32>; 3]> {
        Some([Some(x), Some(y), Some(z)])
    }

    /// A MINI's INFO on Buddy 8.0.2 (`render.cpp:385-535`): its one head as tool "0" with the
    /// nozzle under `features` and the spool under `material_package_instance` (no ANFC on the
    /// MINI, so no uuids or weights). The hub keeps the nozzle and the material's abbreviation
    /// as Buddy writes them (`render.cpp:455-456`, `:493-532`); no spool, none.
    #[test]
    fn a_minis_info_on_8_0_2_gives_its_nozzle_and_material() {
        let info = |material: Value| {
            json!({"firmware": "8.0.2+12345", "printer_type": "2.1.0", "sn": "CZPX1234",
                "appendix": false, "fingerprint": "abc", "transfer_paused": false,
                "tools": {"0": {"type": "FFF_PRINT_HEAD",
                    "features": {"nozzle_diameter": 0.40, "nozzle_hardened": false, "nozzle_high_flow": false},
                    "material_package_instance": material}}})
        };
        let d = to_details(&info(json!({"class": "FFF", "package": {"class": "FFF",
            "material": {"class": "FFF", "abbreviation": "PETG"}}})));
        assert_eq!(
            (d.nozzle_mm, d.material.as_deref()),
            (Some(0.4), Some("PETG"))
        );
        assert_eq!(to_details(&info(Value::Null)).material, None, "no spool");
        // The rest of a MINI's INFO in the order `render.cpp` writes it (`:387-431`): firmware,
        // `printer_type` as text, serial, its USB stick (`:399-412`), its network (`:413-431`).
        let mut full = info(Value::Null);
        let more = json!({"storages": [{"mountpoint": "/usb", "type": "USB", "read_only": false,
                "free_space": 7456000000_u64, "is_sfn": true}],
            "network_info": {"lan_mac": "10:9C:70:1A:2B:3C", "lan_ipv4": "192.168.60.40",
                "wifi_ssid": "makerspace", "hostname": "prusa-mini"}});
        for (k, v) in more.as_object().unwrap() {
            full[k] = v.clone();
        }
        let d = to_details(&full);
        assert_eq!(
            (
                d.firmware.as_deref(),
                d.printer_type.as_deref(),
                d.serial.as_deref()
            ),
            (Some("8.0.2+12345"), Some("2.1.0"), Some("CZPX1234"))
        );
        let usb = &d.storages[0];
        assert_eq!(
            (
                usb.kind.as_str(),
                usb.path.as_str(),
                usb.free_bytes,
                usb.read_only
            ),
            ("USB", "/usb", Some(7456000000), false)
        );
        let net = d.network.expect("network_info");
        assert_eq!(
            (
                net.lan_ipv4.as_deref(),
                net.hostname.as_deref(),
                net.wifi_ipv4
            ),
            (Some("192.168.60.40"), Some("prusa-mini"), None)
        );
        // The MAC as Buddy writes it (`JSON_MAC`, `render.cpp:51`).
        assert_eq!(net.lan_mac.as_deref(), Some("10:9C:70:1A:2B:3C"));
        // The SSID, which the hub shows (D7).
        assert_eq!(net.wifi_ssid.as_deref(), Some("makerspace"));
    }

    /// A MINI on Buddy 8.0.2 printing: a full telemetry frame as `render_msg` writes it
    /// (`render.cpp:124-305`): the job (`:163-174`), `temp_bed`, `target_bed`, `speed`, `flow`
    /// (`:178-192`), no X and Y while a job runs (`:212-216`), `axis_z` (`:217`), its one head as
    /// tool "0" (`:219-240`; one virtual tool, `iter_tool_count` `:1373-1387`) with a null
    /// `consumed_weight` (no ANFC on the MINI, `:276-282`), then `state` (`:303`). Each member is
    /// read as Buddy writes it; the head in the tools block (`devices/buddy_tools.rs`); the hub
    /// keeps them as
    /// numbers for its pages.
    #[test]
    fn a_mini_on_8_0_2_is_read_as_buddy_writes_it() {
        let mut mini = Printer::new("MINI");
        let t = mini.frame(json!({
            "job_id": 41, "time_printing": 300, "time_remaining": 1200, "progress": 12,
            "temp_bed": 60.1, "target_bed": 60.0, "speed": 100, "flow": 95,
            "axis_z": 1.80,
            "tools": {"0": {"type": "FFF_PRINT_HEAD", "active": true, "nozzle_temp": 214.6,
                "nozzle_target_temp": 215, "heatbreak_fan_rpm": 3010, "print_fan_rpm": 2520,
                "consumed_weight": null}},
            "state": "PRINTING"
        }));
        assert_eq!((t.bed_c, t.bed_target_c), (Some(60.1), Some(60.0)));
        assert_eq!((t.speed_percent, t.flow_percent), (Some(100.0), Some(95.0)));
        assert_eq!(t.position_mm, Some([None, None, Some(1.8)]));
        let job = t.job.as_ref().expect("a job");
        assert_eq!(
            (job.done, job.elapsed_s, job.remaining_s),
            (12, Some(300), Some(1200))
        );
        assert_eq!((t.nozzle_c, t.nozzle_target_c), (Some(214.6), Some(215.0)));
        assert_eq!(
            (t.print_fan_rpm, t.hotend_fan_rpm),
            (Some(2520), Some(3010))
        );
        assert_eq!(t.state, MachineState::Busy);
        // On the hub's tool tree as Buddy wrote the head (`render.cpp:229-240`); a null consumed
        // weight (no ANFC on the MINI, `render.cpp:274-281`) is no member.
        let head = &t.tool_tree["0"];
        assert_eq!(head.get("active"), Some(&json!(true)));
        assert_eq!(head.get("nozzle_temp"), Some(&json!(214.6)));
        assert_eq!(head.get("consumed_weight"), None);
    }

    /// A MINI printing: a Full frame now and then (Z only while it prints, `render.cpp:212-217`),
    /// Reduced ones in between (job, progress, times). Each axis is held as told last, the time
    /// left too, and the job's progress stays when the print is over (ours: the page's last print).
    #[test]
    fn a_printing_mini_tells_z_and_keeps_the_rest() {
        let mut mini = Printer::new("MINI");
        let before = mini.frame(json!({
            "state": "IDLE", "axis_x": 176.4, "axis_y": 24.1, "axis_z": 2.0,
            "tools": {"0": {"type": "FFF_PRINT_HEAD", "active": true, "nozzle_temp": 27.0}}
        }));
        assert_eq!(before.position_mm, at(176.4, 24.1, 2.0));
        let full = mini.frame(json!({
            "state": "PRINTING", "axis_z": 1.8, "job_id": 29, "progress": 5
        }));
        assert_eq!(
            full.position_mm,
            at(176.4, 24.1, 1.8),
            "X and Y as told last"
        );
        assert_eq!(
            full.nozzle_c,
            Some(27.0),
            "no tools in the frame: as told last"
        );
        let short = mini.frame(json!({
            "state": "PRINTING", "job_id": 29, "progress": 6, "time_printing": 800
        }));
        assert_eq!(short.position_mm.and_then(|p| p[2]), Some(1.8));
        assert_eq!(short.job.as_ref().and_then(|j| j.remaining_s), None);
        mini.frame(json!({
            "state": "PRINTING", "job_id": 29, "progress": 6, "time_remaining": 3240
        }));
        let untold = mini.frame(json!({
            "state": "PRINTING", "job_id": 29, "progress": 6, "time_printing": 801
        }));
        assert_eq!(untold.job.as_ref().and_then(|j| j.remaining_s), Some(3240));
        assert_eq!(untold.job.as_ref().and_then(|j| j.elapsed_s), Some(801));
        let higher = mini.frame(json!({"state": "PRINTING", "axis_z": 2.0}));
        assert_eq!(
            higher.position_mm.and_then(|p| p[2]),
            Some(2.0),
            "Z goes on"
        );
        // Stopped, all three told; the job's members as last told.
        let after = mini.frame(json!({
            "state": "STOPPED", "axis_x": 10.0, "axis_y": 20.0, "axis_z": 30.0
        }));
        assert_eq!(after.position_mm, at(10.0, 20.0, 30.0));
        assert_eq!(after.state, MachineState::Idle);
        assert_eq!(
            after.job.as_ref().map(|j| (j.done, j.remaining_s)),
            Some((6, Some(3240))),
            "the hub keeps the job's last values when the print is over (ours: the page's last print)"
        );
        // The next print writes over them.
        let next = mini.frame(json!({"state": "PRINTING", "job_id": 30, "progress": 1}));
        assert_eq!(next.job.as_ref().map(|j| j.done), Some(1));
    }

    /// X and Y never told: unknown, not 0 (the server writes only what a frame carries).
    #[test]
    fn x_and_y_never_told_stay_unknown() {
        let t = first(json!({"state": "PRINTING", "axis_z": 1.2}));
        assert_eq!(t.position_mm, Some([None, None, Some(1.2)]));
        assert_eq!(first(json!({"state": "IDLE"})).position_mm, None);
    }

    /// A reader sees the kept job only while the frame names it: Buddy writes `job_id` in every
    /// frame while it has a job (`render.cpp:162-175`; `has_job`, `printer_state.cpp:613-626`:
    /// PRINTING, PAUSED, ATTENTION while printing), and none after; the telemetry keeps the job's
    /// progress.
    #[test]
    fn the_kept_job_is_current_only_while_the_frame_names_it() {
        let mut mini = Printer::new("MINI");
        for (frame, current) in [
            (
                json!({"state": "PRINTING", "job_id": 3, "progress": 50}),
                true,
            ),
            (
                json!({"state": "PAUSED", "job_id": 3, "progress": 50}),
                true,
            ),
            (
                json!({"state": "ATTENTION", "job_id": 3, "progress": 50}),
                true,
            ),
            (json!({"state": "FINISHED"}), false),
            (json!({"state": "STOPPED"}), false),
            (json!({"state": "IDLE"}), false),
            (json!({"state": "ERROR"}), false),
        ] {
            let state = frame["state"].clone();
            let mut t = mini.frame(frame);
            assert!(t.job.is_some(), "{state}: kept");
            current_job(&mut t);
            assert_eq!(t.job.is_some(), current, "{state}");
        }
    }

    /// A printer on Prusa Connect is online while its socket is attached and it is heard (ours:
    /// what the page shows, audit D11; `Attached::new` counts as heard). Closed: offline.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn connected_while_the_socket_is_attached(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let (space, mini, _) = store
            .add_test_machine("prusa/mini", Some("prusa-connect"))
            .await;
        let online = |m: crate::store::Machine| m.online;
        assert!(!online(store.machine(space, &mini).await.unwrap().unwrap()));
        let socket = Attached::new(&store, mini.clone(), 1);
        assert!(online(store.machine(space, &mini).await.unwrap().unwrap()));
        drop(socket);
        assert!(!online(store.machine(space, &mini).await.unwrap().unwrap()));
    }

    /// Bed levelling after a print (BUSY, no `job_id`: Buddy's `has_job` is false then,
    /// `printer_state.cpp:613-626`) brings the kept job back to no reader; a frame that still
    /// names the job is the print.
    #[test]
    fn busy_after_a_print_is_no_print() {
        let mut mini = Printer::new("MINI");
        let current = |mut t: Telemetry| {
            current_job(&mut t);
            t.job.is_some()
        };
        let printing = json!({"state": "PRINTING", "job_id": 29, "progress": 99});
        assert!(current(mini.frame(printing.clone())));
        assert!(current(mini.frame(json!({"state": "BUSY", "job_id": 29}))));
        assert!(current(mini.frame(printing)));
        assert!(!current(mini.frame(json!({"state": "FINISHED"}))));
        assert!(!current(mini.frame(json!({"state": "IDLE"}))));
        let levelling = mini.frame(json!({"state": "BUSY"}));
        assert!(levelling.job.is_some(), "the telemetry keeps it");
        assert!(!current(levelling), "no print while it levels the bed");
    }

    /// Buddy 8.0.2 (the owner's MINI): a Full frame (v8.0.2 `render.cpp:177-292`, the head under
    /// `tools."0"`), then Reduced ones with only the state (`planner.cpp:537-557`). What the
    /// Reduced frame leaves out keeps the Full frame's value; a frame with a new value changes it.
    #[test]
    fn a_reduced_frame_keeps_what_the_full_one_told() {
        let mut mini = Printer::new("MINI");
        let full = mini.frame(json!({
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
        let reduced = mini.frame(json!({"state": "IDLE"}));
        assert_eq!(
            reduced, full,
            "a Reduced frame changes nothing it leaves out"
        );
        // Cooled down: the bed's target 0, the nozzle's null: Buddy writes null only for a target
        // of 0 (`render.cpp:233-237`).
        let cooled = mini.frame(json!({
            "temp_bed": 58.9, "target_bed": 0.0, "speed": 100, "flow": 95, "axis_z": 10.0,
            "tools": {"0": {"type": "FFF_PRINT_HEAD", "active": true, "nozzle_temp": 201.3,
                "nozzle_target_temp": null, "heatbreak_fan_rpm": 7000, "print_fan_rpm": 0}},
            "state": "IDLE"
        }));
        assert_eq!(
            (cooled.nozzle_c, cooled.nozzle_target_c),
            (Some(201.3), Some(0.0))
        );
        assert_eq!((cooled.bed_c, cooled.bed_target_c), (Some(58.9), Some(0.0)));
        assert_eq!(cooled.hotend_fan_rpm, Some(7000));
        assert_eq!(cooled.position_mm, at(90.0, 90.0, 10.0));
        // A flow of 0 is no reading (`printer.hpp:175`, `marlin_printer.cpp:339-341`).
        assert_eq!(
            mini.frame(json!({"state": "IDLE", "flow": 0})).flow_percent,
            None
        );
    }

    /// Buddy 8's `tools` object with more than one head (an XL): each head in the list; the top
    /// is the active head's; slots in front of an MMU (`"0.<n>"`) are no heads.
    #[test]
    fn the_tools_object_of_8_lists_the_heads() {
        let xl = Printer::new("XL").frame(json!({
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
        let mmu = Printer::new("MK4S").frame(json!({
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

    /// Buddy 8.0.2's spool in INFO (`render.cpp:493-532`) is what the page shows with the
    /// telemetry; no spool, none.
    #[test]
    fn info_names_the_material_the_page_shows() {
        let mut mini = Printer::new("MINI");
        mini.info(json!({"tools": {"0": {"type": "FFF_PRINT_HEAD",
            "material_package_instance": {"class": "FFF", "package": {"class": "FFF",
                "material": {"class": "FFF", "abbreviation": "PETG"}}}}}}));
        let t = mini.frame(json!({"state": "IDLE",
            "tools": {"0": {"type": "FFF_PRINT_HEAD", "active": true, "nozzle_temp": 24.0}}}));
        assert_eq!(t.material.as_deref(), Some("PETG"));
        mini.info(
            json!({"tools": {"0": {"type": "FFF_PRINT_HEAD", "material_package_instance": null}}}),
        );
        assert_eq!(
            mini.frame(json!({"state": "IDLE"})).material,
            None,
            "no spool"
        );
    }

    /// A report with only the state keeps the temperatures of the one before.
    #[test]
    fn a_short_report_keeps_what_it_left_out() {
        let mut mini = Printer::new("MINI");
        mini.frame(
            json!({"state": "IDLE", "temp_bed": 23, "axis_x": 1, "axis_y": 2, "axis_z": 3,
            "tools": {"0": {"type": "FFF_PRINT_HEAD", "active": true, "nozzle_temp": 24}}}),
        );
        let short = mini.frame(json!({"state": "IDLE"}));
        assert_eq!(short.nozzle_c, Some(24.0));
        assert_eq!(short.position_mm, at(1.0, 2.0, 3.0));
    }

    /// A Marlin printer on the agent: its head by PrusaLink's keys at the top of the frame
    /// (`auto_telemetry.py:61-65`, `:93-98`), not by Buddy's tools object.
    #[test]
    fn the_agents_head_by_prusalinks_keys() {
        let t = Printer::agent().frame(
            json!({"job_id":7,"time_printing":600,"time_remaining":900,"progress":40,
            "temp_nozzle":215.3,"temp_bed":60.1,"target_nozzle":215.0,"target_bed":60.0,
            "fan_hotend":7100,"fan_print":5200,"axis_z":1.20,"state":"PRINTING"}),
        );
        assert_eq!((t.nozzle_c, t.nozzle_target_c), (Some(215.3), Some(215.0)));
        assert_eq!((t.bed_c, t.bed_target_c), (Some(60.1), Some(60.0)));
        assert_eq!(
            (t.print_fan_rpm, t.hotend_fan_rpm),
            (Some(5200), Some(7100))
        );
        assert!(t.tool_tree.is_empty());
        assert_eq!(t.job.map(|j| j.done), Some(40));
    }

    /// A CORE One's chamber and an XL's enclosure as Buddy 8.0.2 writes them (`render.cpp:193-210`).
    #[test]
    fn the_chamber_and_the_enclosure() {
        let mut core = Printer::new("CoreOne");
        let c = core.frame(json!({"chamber":{"temp":31.5,"target_temp":40,"fan_1_rpm":0,"fan_2_rpm":0,"fan_pwm_target":0,"led_intensity":100},"state":"IDLE"}));
        assert_eq!((c.chamber_c, c.chamber_target_c), (Some(31.5), Some(40.0)));
        assert!(c.tools.is_empty(), "no slot: no list");
        let xl =
            first(json!({"enclosure":{"temp":28,"fan_rpm":0,"time_in_use":3600},"state":"IDLE"}));
        assert_eq!(xl.chamber_c, Some(28.0));
        // A Reduced frame keeps the reading; a Full one with neither block has none (Buddy writes
        // the enclosure only while one is present, `render.cpp:194`).
        assert_eq!(core.frame(json!({"state": "IDLE"})).chamber_c, Some(31.5));
        assert_eq!(
            core.frame(json!({"state": "IDLE", "temp_bed": 22.0}))
                .chamber_c,
            None
        );
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

        let (status, first) = call(
            &app,
            Request::post("/p/telemetry")
                .header("Token", &token)
                .header("Fingerprint", "abc")
                .header("User-Agent-Version", "8.0.2+0.LOCAL")
                .body(Body::from(
                    r#"{"progress":42,"temp_bed":60.1,"target_bed":60,"tools":{"0":{"type":"FFF_PRINT_HEAD","active":true,"nozzle_temp":214.9,"nozzle_target_temp":null,"heatbreak_fan_rpm":0,"print_fan_rpm":0,"consumed_weight":null}},"state":"PRINTING","job_id":1}"#,
                ))
                .unwrap(),
        )
        .await;
        // A print it runs (Buddy writes `job_id` while it has one, printer_state.cpp:613-626)
        // that the hub does not know: it asks which file it is (SEND_JOB_INFO), and Buddy tells.
        assert_eq!(status, StatusCode::OK);
        let job_info = store.lock().unwrap().job_info_asked.get(&token).copied();
        assert_eq!(job_info, Some(1));
        let asked: u64 = first["command-id"].to_str().unwrap().parse().unwrap();
        let told = Request::post("/p/events")
            .header("Token", &token)
            .body(Body::from(
                json!({"event": "JOB_INFO", "command_id": asked, "job_id": 1, "state": "PRINTING",
                    "data": {"state": "PRINTING", "path": "/usb/PART~1.GCO", "display_name": "part.gcode"}})
                .to_string(),
            ))
            .unwrap();
        assert_eq!(call(&app, told).await.0, StatusCode::NO_CONTENT);
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
            Some("Prusa firmware 8.0.2+0.LOCAL")
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
                .body(Body::from(
                    r#"{"state":"PRINTING","progress":43,"job_id":1}"#,
                ))
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
        // Idle again: the stored telemetry keeps the job's members, and no reader of the machine
        // sees a print (no `job_id` in the frame, `crate::devices::connect_job`).
        assert_eq!(
            store
                .last_telemetry(&token)
                .await
                .unwrap()
                .and_then(|t| t.job)
                .map(|j| j.done),
            Some(43)
        );
        assert_eq!(machines[0].telemetry.as_ref().unwrap().job, None);

        let unknown = Request::post("/p/telemetry")
            .header("Token", "nope")
            .body(Body::from(r#"{"state":"IDLE"}"#))
            .unwrap();
        assert_eq!(call(&app, unknown).await.0, StatusCode::UNAUTHORIZED);
        let unknown_info = Request::post("/p/events")
            .header("Token", "nope")
            .body(Body::from(r#"{"event":"INFO","data":{"tools":{"0":{}}}}"#))
            .unwrap();
        assert_eq!(call(&app, unknown_info).await.0, StatusCode::UNAUTHORIZED);
        let sessions = || {
            let mut ids: Vec<String> = store
                .lock()
                .unwrap()
                .connect_tools
                .keys()
                .cloned()
                .collect();
            ids.sort();
            ids
        };
        assert_eq!(
            sessions(),
            vec![mini_id.clone()],
            "none for an unknown token"
        );
        assert!(store.remove_machine(space, &mini_id).await.unwrap());
        assert!(sessions().is_empty(), "gone with its machine");
    }

    /// Mesh bed leveling on a MINI over Prusa Connect: shown in its Controls to whoever may operate
    /// it, a G frame, its id as 8 hex digits, then `G28\r\nG29`; over HTTP the same G-code as the
    /// answer. The G-code is what Prusa Connect's server sends for it, not Buddy's own menu code,
    /// by the owner's decision (2026-11-24, #203: "ja, doe het zo").
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn mesh_bed_leveling_goes_out_as_the_server_sends_it(pool: sqlx::PgPool) {
        use http_body_util::BodyExt;
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (_, id, token) = store
            .add_test_machine("prusa/mini", Some("prusa-connect"))
            .await;
        crate::auth::tester_may_operate(&store, &id).await;
        let page = app
            .clone()
            .oneshot(
                Request::get(format!("/machines/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let html = page.into_body().collect().await.unwrap().to_bytes();
        assert!(
            String::from_utf8_lossy(&html)
                .contains(r#"<button name="action" value="probe-bed">Mesh bed leveling</button>"#),
            "the button in Controls"
        );
        let press = Request::post(format!("/machines/{id}/command"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-requested-with", "fetch")
            .body(Body::from("action=probe-bed"))
            .unwrap();
        assert_eq!(call(&app, press).await.0, StatusCode::NO_CONTENT);
        let answer = app
            .clone()
            .oneshot(
                Request::post("/p/telemetry")
                    .header("Token", &token)
                    .body(Body::from(r#"{"state":"IDLE"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(answer.status(), StatusCode::OK);
        assert_eq!(answer.headers()["content-type"], "text/x.gcode");
        let sent: u32 = answer.headers()["command-id"]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        let ws = command_of(answer).await.expect("a command");
        assert_eq!(ws, format!("G{sent:08x}G28\r\nG29"));
        let space = store.default_space().await.unwrap().0;
        let machine = store.machine(space, &id).await.unwrap().unwrap();
        assert_eq!(machine.commands[0].label, "Mesh bed leveling");
    }

    /// An INFO event as a MINI sends it: its details are kept, its Prusa Link key is not.
    #[test]
    fn info_becomes_details_without_secrets() {
        let info: Value = serde_json::from_str(
            r#"{"firmware":"8.0.2+12345","printer_type":"2.1.0","sn":"CZPX1234","api_key":"secret",
                "storages":[{"mountpoint":"/usb","type":"USB","read_only":false,"free_space":30537007104,"is_sfn":true}],
                "network_info":{"lan_ipv4":"192.168.1.64","hostname":"mini"},
                "tools":{"0":{"type":"FFF_PRINT_HEAD","features":{"nozzle_diameter":0.40},
                  "material_package_instance":{"class":"FFF","package":{"class":"FFF","material":{"class":"FFF","abbreviation":"PETG"}}}}}}"#,
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

    /// The nozzle at the top level of INFO, as PrusaLink sends it for the agent's printers
    /// (`sdk_augmentation/printer.py:55`), or per head as Buddy 8.0.2 writes it; `printer_type`
    /// only as Buddy's string (`render.cpp:390`): as numbers it is no type.
    #[test]
    fn info_gives_the_nozzle_in_either_references_shape() {
        let prusalink = to_details(&json!({"nozzle_diameter": 0.6, "printer_type": "2.1.0"}));
        assert_eq!(
            (prusalink.nozzle_mm, prusalink.printer_type.as_deref()),
            (Some(0.6), Some("2.1.0"))
        );
        let numbers: Value = serde_json::from_str(
            r##"{"firmware":"8.0.1+12","type":1,"version":3,"subversion":1,
                "tools":{"0":{"type":"FFF_PRINT_HEAD","features":{"nozzle_diameter":0.60},
                  "material_package_instance":{"package":{"material":{"abbreviation":"PETG"}}}}}}"##,
        )
        .unwrap();
        let d = to_details(&numbers);
        assert_eq!(
            (d.nozzle_mm, d.material.as_deref(), d.printer_type),
            (Some(0.6), Some("PETG"), None)
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

    /// A GRBL laser's settings (part 4) from its `INFO`; none when it told none.
    #[test]
    fn a_lasers_settings_from_its_info() {
        let s = grbl_settings(
            &json!({"type": "grbl.laser", "max_power": 1000, "laser_mode": true,
            "travel_x": 400.0, "travel_y": 430.0, "travel_z": 0.0}),
        )
        .unwrap();
        assert_eq!(s.max_power, Some(1000.0));
        assert_eq!(s.laser_mode, Some(true));
        assert_eq!(s.travel_mm, Some([400.0, 430.0, 0.0]));
        assert_eq!(grbl_settings(&json!({"type": "grbl.laser"})), None);
    }

    /// A laser's frame: its state and machine position; its settings from the session (`INFO`).
    #[test]
    fn a_lasers_telemetry_is_taken_in() {
        let mut laser = Printer::new("grbl.laser");
        laser.session.grbl = grbl_settings(&json!({"max_power": 1000, "laser_mode": true}));
        let t = laser.frame(
            json!({"state": "IDLE", "axis_x": 12.5, "axis_y": 40.0, "machine_state": "Idle"}),
        );
        assert_eq!(t.state, MachineState::Idle);
        assert_eq!(
            t.position_mm.map(|p| [p[0], p[1]]),
            Some([Some(12.5), Some(40.0)])
        );
        assert_eq!(t.grbl.as_ref().and_then(|g| g.max_power), Some(1000.0));
        let t = laser.frame(json!({"state": "ATTENTION", "machine_state": "Door"}));
        assert_eq!(t.state, MachineState::Attention);
        assert_eq!(
            t.position_mm.map(|p| [p[0], p[1]]),
            Some([Some(12.5), Some(40.0)]),
            "kept"
        );
    }

    /// An agent's cameras device (part 5) registers as `maker-connect.cameras`: no machine. It pairs,
    /// its token takes a camera's registration, and it is in no list of machines.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn an_agents_cameras_device_is_no_machine(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (status, headers) = call(
            &app,
            Request::post("/p/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"sn":"MC00112233445566CC","fingerprint":"fp","printer_type":"maker-connect.cameras","firmware":"0.3.0"}"#,
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let code = headers["code"].to_str().unwrap().to_owned();
        let space = store.default_space().await.unwrap().0;
        let (id, name) = store.pair(space, &code).await.unwrap().unwrap();
        assert_eq!(name, "Cameras 66CC");
        assert_eq!(
            store.kind_of(space, &id).await.unwrap(),
            Some(MachineKind::Cameras)
        );
        assert!(store.machines(space).await.unwrap().is_empty());
        // `/p/camera` is the module `cameras`'s (`cameras/agent.rs:19`).
        #[cfg(feature = "cameras")]
        {
            let (_, headers) = call(
                &app,
                Request::get("/p/register")
                    .header("Code", &code)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            let token = headers["token"].to_str().unwrap().to_owned();
            let (status, headers) = call(
            &app,
            Request::post("/p/camera")
                .header("Token", &token)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"fingerprint": "c8590ee0b141d78980f57878e22b507f16fdf0c62ac32fbfc496aa502899d85f",
                        "config": {"name": "USB camera", "camera_id": "usb-1"}})
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
            assert_eq!(status, StatusCode::OK);
            assert!(headers.contains_key("token"));
        }
    }

    /// A GRBL laser on the agent (part 4) registers as `grbl.laser`: a laser, named "GRBL laser"
    /// and the end of its serial, its type kept in its details.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_laser_on_the_agent_registers_as_a_laser(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (status, headers) = call(
            &app,
            Request::post("/p/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"sn":"MC00112233445566BB","fingerprint":"fp","printer_type":"grbl.laser","firmware":"0.3.0"}"#,
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let code = headers["code"].to_str().unwrap().to_owned();
        let space = store.default_space().await.unwrap().0;
        assert!(store.pair(space, &code).await.unwrap().is_some());
        let laser = store.machines(space).await.unwrap()[0].clone();
        assert_eq!(laser.kind, MachineKind::Laser);
        assert_eq!(laser.name, "GRBL laser 66BB");
        assert_eq!(laser.profile, None);
        assert_eq!(
            laser
                .details
                .as_ref()
                .and_then(|d| d.printer_type.as_deref()),
            Some(LASER_AGENT)
        );

        // Its profile by the `MODEL` of its `$I` answer in INFO (part 4, "What the laser is"):
        // a name no profile has finds none; the Ortur's own answer (#165) finds its profile.
        let (_, headers) = call(
            &app,
            Request::get("/p/register")
                .header("Code", &code)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        let token = headers["token"].to_str().unwrap().to_owned();
        let info = |model: &str| {
            Request::post("/p/events")
                .header("Token", &token)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"event": "INFO", "state": "IDLE", "data": {
                        "type": "grbl.laser", "sn": "MC00112233445566BB",
                        "firmware": "1.1f(ESP32).20210403",
                        "firmware_info": {"MODEL": model, "VER": "1.1f(ESP32).20210403:", "OLF": "190"}}})
                    .to_string(),
                ))
                .unwrap()
        };
        let profile = || async { store.machines(space).await.unwrap()[0].profile.clone() };
        call(&app, info("Some Laser")).await;
        assert_eq!(profile().await, None);
        call(&app, info("Ortur Laser Master 2 Pro S2")).await;
        assert_eq!(profile().await.as_deref(), Some("ortur/lm2-pro-s2"));
        let details = store.machines(space).await.unwrap()[0]
            .details
            .clone()
            .unwrap();
        assert_eq!(details.firmware.as_deref(), Some("1.1f(ESP32).20210403"));
        assert_eq!(
            details.firmware_info.and_then(|i| i.get("OLF").cloned()),
            Some(json!("190"))
        );
    }

    /// The DYMO LabelManager PnP on the agent (docs/design/wire.md, "The first device"): it
    /// registers as `dymo.labelmanager-pnp` and is a label printer; its cassette, charge and
    /// status bytes are kept; a label job goes out, when it is idle, as START_CONNECT_DOWNLOAD
    /// with `printing` and `tape_size_mm`; the file comes from `/p/teams/{team}/files/{hash}/raw`
    /// with its token only. The command's answer does not end the job, and no other label goes
    /// out while it is open; after TRANSFER_FINISHED a STATE_CHANGED IDLE ends it, an ERROR directly
    /// before it fails it with its reason; TRANSFER_ABORTED and a refused command fail it.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_dymo_on_the_agent_prints_a_label(pool: sqlx::PgPool) {
        use crate::store::{JobState, StartedBy};
        use http_body_util::BodyExt;
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (status, headers) = call(
            &app,
            Request::post("/p/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"sn":"MC0011223344556677","fingerprint":"fp","printer_type":"dymo.labelmanager-pnp","firmware":"0.3.0"}"#,
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let code = headers["code"].to_str().unwrap().to_owned();
        let space = store.default_space().await.unwrap().0;
        assert!(store.pair(space, &code).await.unwrap().is_some());
        let (_, headers) = call(
            &app,
            Request::get("/p/register")
                .header("Code", &code)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        let token = headers["token"].to_str().unwrap().to_owned();
        let dymo = store.machines(space).await.unwrap()[0].clone();
        assert_eq!(dymo.kind, MachineKind::Label);
        assert_eq!(dymo.profile.as_deref(), Some("dymo/labelmanager-pnp"));

        let png = b"\x89PNG label".to_vec();
        for f in ["l1", "l2", "l3", "l4"] {
            std::fs::write(store.file_path(f), &png).unwrap();
            store
                .add_file(
                    space,
                    f,
                    &format!("{f}.png"),
                    png.len() as u64,
                    &Default::default(),
                )
                .await
                .unwrap();
        }
        let idle = || {
            Request::post("/p/telemetry")
                .header("Token", &token)
                .body(Body::from(
                    r#"{"state":"IDLE","media_loaded":true,"battery_percent":80,"raw_status":"00 00 00 00 00 00 00 00"}"#,
                ))
                .unwrap()
        };
        assert_eq!(
            call(&app, idle()).await.0,
            StatusCode::NO_CONTENT,
            "no job yet"
        );
        let t = store.machines(space).await.unwrap()[0]
            .telemetry
            .clone()
            .unwrap();
        assert_eq!(
            (t.cassette, t.battery_percent, t.raw_status.as_deref()),
            (Some(true), Some(80), Some("00 00 00 00 00 00 00 00"))
        );

        let mut ends = Vec::new();
        for (f, events) in [
            (
                "l1",
                &[
                    r#""event":"TRANSFER_INFO","command_id":{sent}"#,
                    r#""event":"TRANSFER_FINISHED","data":{"start_cmd_id":{sent}}"#,
                    r#""event":"STATE_CHANGED","state":"PRINTING""#,
                    r#""event":"STATE_CHANGED","state":"IDLE""#,
                ][..],
            ),
            (
                "l2",
                &[
                    r#""event":"TRANSFER_INFO","command_id":{sent}"#,
                    r#""event":"TRANSFER_FINISHED","data":{"start_cmd_id":{sent}}"#,
                    r#""event":"STATE_CHANGED","state":"PRINTING""#,
                    r#""event":"STATE_CHANGED","state":"ERROR","reason":"no cassette""#,
                    r#""event":"STATE_CHANGED","state":"IDLE""#,
                ][..],
            ),
            (
                "l3",
                &[
                    r#""event":"TRANSFER_INFO","command_id":{sent}"#,
                    r#""event":"TRANSFER_ABORTED","reason":"404","data":{"start_cmd_id":{sent}}"#,
                ][..],
            ),
            (
                "l4",
                &[
                    r#""event":"REJECTED","command_id":{sent},"reason":"Another command is running""#,
                ][..],
            ),
        ] {
            let file = store.file(space, f).await.unwrap().unwrap();
            let job = store
                .add_job(
                    space,
                    &dymo.id,
                    &file,
                    None,
                    StartedBy::Auto { by: Some("test") },
                )
                .await
                .unwrap()
                .unwrap();
            let answer = app.clone().oneshot(idle()).await.unwrap();
            assert_eq!(answer.status(), StatusCode::OK);
            let sent: u32 = answer.headers()["command-id"]
                .to_str()
                .unwrap()
                .parse()
                .unwrap();
            let frame = command_of(answer).await.expect("a command");
            let command: Value = serde_json::from_str(&frame[9..]).unwrap();
            assert_eq!(&frame[..9], format!("J{sent:08x}"));
            assert_eq!(
                command,
                json!({"command": "START_CONNECT_DOWNLOAD", "kwargs": {
                    "path": format!("/labels/{job}.png"),
                    "team_id": space,
                    "hash": f,
                    "printing": true,
                    "tape_size_mm": null,
                }})
            );
            assert_eq!(
                call(&app, idle()).await.0,
                StatusCode::NO_CONTENT,
                "one at a time"
            );

            let raw = |t: &str| {
                Request::get(format!("/p/teams/{space}/files/{f}/raw"))
                    .header("Token", t)
                    .body(Body::empty())
                    .unwrap()
            };
            let answer = app.clone().oneshot(raw(&token)).await.unwrap();
            assert_eq!(answer.status(), StatusCode::OK);
            let body = answer.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), png.as_slice());
            assert_eq!(call(&app, raw("nope")).await.0, StatusCode::FORBIDDEN);

            let (last, before) = events.split_last().unwrap();
            let send = |event: &str| {
                Request::post("/p/events")
                    .header("Token", &token)
                    .body(Body::from(format!(
                        "{{{},\"source\":\"CONNECT\"}}",
                        event.replace("{sent}", &sent.to_string())
                    )))
                    .unwrap()
            };
            for event in before {
                assert_eq!(call(&app, send(event)).await.0, StatusCode::NO_CONTENT);
                let state = store.job(space, job).await.unwrap().unwrap().state;
                assert!(
                    !matches!(state, JobState::Finished | JobState::Failed),
                    "{f}: still open after {event}"
                );
                assert_eq!(
                    call(&app, idle()).await.0,
                    StatusCode::NO_CONTENT,
                    "{f}: no other label while it is open"
                );
            }
            assert_eq!(call(&app, send(last)).await.0, StatusCode::NO_CONTENT);
            let job = store.job(space, job).await.unwrap().unwrap();
            ends.push((job.state, job.reason));
        }
        assert_eq!(
            ends,
            vec![
                (JobState::Finished, None),
                (JobState::Failed, Some("no cassette".to_owned())),
                (JobState::Failed, Some("TRANSFER_ABORTED".to_owned())),
                (
                    JobState::Failed,
                    Some("Another command is running".to_owned())
                ),
            ]
        );
    }

    /// The DYMO LabelWriter 320 on the agent (#25): it registers as `dymo.labelwriter-320`, gets its own
    /// profile and name, and a label goes out with the loaded roll's `page_size` from the 320's PPD. A type
    /// no profile knows is named by that type, not "Prusa printer".
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_labelwriter_320_on_the_agent_prints_a_label(pool: sqlx::PgPool) {
        use crate::store::StartedBy;
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let space = store.default_space().await.unwrap().0;
        let mut tokens = Vec::new();
        for (sn, printer_type) in [
            ("MC0011223344556677", "dymo.labelwriter-320"),
            ("MC00112233445566AB", "acme.thing"),
        ] {
            let (status, headers) = call(
                &app,
                Request::post("/p/register")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"sn":"{sn}","fingerprint":"fp{sn}","printer_type":"{printer_type}","firmware":"0.3.0"}}"#
                    )))
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let code = headers["code"].to_str().unwrap().to_owned();
            assert!(store.pair(space, &code).await.unwrap().is_some());
            let (_, headers) = call(
                &app,
                Request::get("/p/register")
                    .header("Code", &code)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            tokens.push(headers["token"].to_str().unwrap().to_owned());
        }
        let machines = store.machines(space).await.unwrap();
        let named = |name: &str| machines.iter().find(|m| m.name == name).cloned();
        assert!(named("acme.thing 66AB").is_some(), "{machines:?}");
        let lw = named("LabelWriter 320 6677").expect("the 320 by its profile's name");
        assert_eq!(lw.kind, MachineKind::Label);
        assert_eq!(lw.profile.as_deref(), Some("dymo/labelwriter-320"));
        let profile = crate::profiles::find("dymo/labelwriter-320").unwrap();
        // 11353 on the box (S0722530): DYMO Connect's paper `30333 1/2 in x 1 in (2 up)`
        let page = crate::profiles::labelwriter_page(Some(profile), "S0722530").unwrap();
        assert_eq!(page.text, Some("30333 1/2 in x 1 in (2 up)"));
        assert!(
            store
                .set_media(space, &lw.id, Some("S0722530"))
                .await
                .unwrap()
        );

        let png = b"\x89PNG label".to_vec();
        std::fs::write(store.file_path("w1"), &png).unwrap();
        store
            .add_file(space, "w1", "w1.png", png.len() as u64, &Default::default())
            .await
            .unwrap();
        let file = store.file(space, "w1").await.unwrap().unwrap();
        let idle = || {
            Request::post("/p/telemetry")
                .header("Token", &tokens[0])
                .body(Body::from(r#"{"state":"IDLE"}"#))
                .unwrap()
        };
        let _ = app.clone().oneshot(idle()).await.unwrap();
        let mut page_file = file.clone();
        page_file.meta.png_dots = Some(page.dots);
        let machine = store
            .machines(space)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.id == lw.id)
            .unwrap();
        assert_eq!(crate::files::suits(&page_file, &machine), Ok(None));
        let job = store
            .add_job(
                space,
                &lw.id,
                &file,
                None,
                StartedBy::Auto { by: Some("test") },
            )
            .await
            .unwrap()
            .unwrap();
        let answer = app.clone().oneshot(idle()).await.unwrap();
        let frame = command_of(answer).await.expect("a command");
        let command: Value = serde_json::from_str(&frame[9..]).unwrap();
        assert_eq!(
            command,
            json!({"command": "START_CONNECT_DOWNLOAD", "kwargs": {
                "path": format!("/labels/{job}.png"),
                "team_id": space,
                "hash": "w1",
                "printing": true,
                "page_size": "w72h72.2",
            }})
        );
    }

    /// The DYMO LabelWriter 400 on the agent (#230): it registers as `dymo.labelwriter-400`, gets its own
    /// profile and name, and a label goes out with the roll's `page_size` from the 400's PPD: 30321 is the
    /// one page where the 400's PPD differs from the 450's (`w102h252`).
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_labelwriter_400_on_the_agent_prints_a_label(pool: sqlx::PgPool) {
        use crate::store::StartedBy;
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let space = store.default_space().await.unwrap().0;
        let (status, headers) = call(
            &app,
            Request::post("/p/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"sn":"MC0011223344556677","fingerprint":"fp","printer_type":"dymo.labelwriter-400","firmware":"0.3.0"}"#,
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let code = headers["code"].to_str().unwrap().to_owned();
        assert!(store.pair(space, &code).await.unwrap().is_some());
        let (_, headers) = call(
            &app,
            Request::get("/p/register")
                .header("Code", &code)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        let token = headers["token"].to_str().unwrap().to_owned();
        let lw = store.machines(space).await.unwrap()[0].clone();
        assert_eq!(lw.name, "LabelWriter 400 6677");
        assert_eq!(lw.kind, MachineKind::Label);
        assert_eq!(lw.profile.as_deref(), Some("dymo/labelwriter-400"));
        let profile = crate::profiles::find("dymo/labelwriter-400").unwrap();
        let page = crate::profiles::labelwriter_page(Some(profile), "30321").unwrap();
        assert_eq!(page.name, "w102h252");
        assert_eq!(page.text, Some("30321 Large Address"));
        assert!(store.set_media(space, &lw.id, Some("30321")).await.unwrap());

        let png = b"\x89PNG label".to_vec();
        std::fs::write(store.file_path("w1"), &png).unwrap();
        store
            .add_file(space, "w1", "w1.png", png.len() as u64, &Default::default())
            .await
            .unwrap();
        let file = store.file(space, "w1").await.unwrap().unwrap();
        let idle = || {
            Request::post("/p/telemetry")
                .header("Token", &token)
                .body(Body::from(r#"{"state":"IDLE"}"#))
                .unwrap()
        };
        let _ = app.clone().oneshot(idle()).await.unwrap();
        let mut page_file = file.clone();
        page_file.meta.png_dots = Some(page.dots);
        let machine = store
            .machines(space)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.id == lw.id)
            .unwrap();
        assert_eq!(crate::files::suits(&page_file, &machine), Ok(None));
        let job = store
            .add_job(
                space,
                &lw.id,
                &file,
                None,
                StartedBy::Auto { by: Some("test") },
            )
            .await
            .unwrap()
            .unwrap();
        let answer = app.clone().oneshot(idle()).await.unwrap();
        let frame = command_of(answer).await.expect("a command");
        let command: Value = serde_json::from_str(&frame[9..]).unwrap();
        assert_eq!(
            command,
            json!({"command": "START_CONNECT_DOWNLOAD", "kwargs": {
                "path": format!("/labels/{job}.png"),
                "team_id": space,
                "hash": "w1",
                "printing": true,
                "page_size": "w102h252",
            }})
        );
    }

    /// The DYMO LabelWriter 450 on the agent (docs/design/wire.md, "The second device"): it
    /// registers as `dymo.labelwriter-450`; a label goes out with the loaded roll's `page_size`;
    /// the `STATE_CHANGED` reason (the PPD's text) shows as its problem; an ERROR followed by PRINTING does not fail the
    /// label; a person's stop goes out as STOP_PRINT once it prints and ends the job Stopped.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_labelwriter_450_on_the_agent_prints_a_label(pool: sqlx::PgPool) {
        use crate::store::{JobState, StartedBy};
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (status, headers) = call(
            &app,
            Request::post("/p/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"sn":"MC8899AABBCCDDEEFF","fingerprint":"fp","printer_type":"dymo.labelwriter-450","firmware":"0.3.0"}"#,
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let code = headers["code"].to_str().unwrap().to_owned();
        let space = store.default_space().await.unwrap().0;
        assert!(store.pair(space, &code).await.unwrap().is_some());
        let (_, headers) = call(
            &app,
            Request::get("/p/register")
                .header("Code", &code)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        let token = headers["token"].to_str().unwrap().to_owned();
        let lw = store.machines(space).await.unwrap()[0].clone();
        assert_eq!(lw.kind, MachineKind::Label);
        assert_eq!(lw.profile.as_deref(), Some("dymo/labelwriter-450"));
        let profile = crate::profiles::find("dymo/labelwriter-450").unwrap();
        let roll = profile.media[0].clone();
        let page = crate::profiles::labelwriter_page(Some(profile), &roll.id).unwrap();
        assert!(
            store
                .set_media(space, &lw.id, Some(&roll.id))
                .await
                .unwrap()
        );

        let png = b"\x89PNG label".to_vec();
        let telemetry = |body: &'static str| {
            Request::post("/p/telemetry")
                .header("Token", &token)
                .body(Body::from(body))
                .unwrap()
        };
        let idle = || telemetry(r#"{"state":"IDLE"}"#);
        let mut ends = Vec::new();
        for (f, stop) in [("w1", false), ("w2", true)] {
            std::fs::write(store.file_path(f), &png).unwrap();
            store
                .add_file(
                    space,
                    f,
                    &format!("{f}.png"),
                    png.len() as u64,
                    &Default::default(),
                )
                .await
                .unwrap();
            let file = store.file(space, f).await.unwrap().unwrap();
            if f == "w1" {
                // The print button's check: a page of the loaded roll suits it, online and idle.
                let _ = app.clone().oneshot(idle()).await.unwrap();
                let mut page_file = file.clone();
                page_file.meta.png_dots = Some(page.dots);
                let machine = store.machines(space).await.unwrap()[0].clone();
                assert_eq!(crate::files::suits(&page_file, &machine), Ok(None));
            }
            let job = store
                .add_job(
                    space,
                    &lw.id,
                    &file,
                    None,
                    StartedBy::Auto { by: Some("test") },
                )
                .await
                .unwrap()
                .unwrap();
            let answer = app.clone().oneshot(idle()).await.unwrap();
            assert_eq!(answer.status(), StatusCode::OK);
            let sent: u32 = answer.headers()["command-id"]
                .to_str()
                .unwrap()
                .parse()
                .unwrap();
            let frame = command_of(answer).await.expect("a command");
            let command: Value = serde_json::from_str(&frame[9..]).unwrap();
            assert_eq!(
                command,
                json!({"command": "START_CONNECT_DOWNLOAD", "kwargs": {
                    "path": format!("/labels/{job}.png"),
                    "team_id": space,
                    "hash": f,
                    "printing": true,
                    "page_size": page.name,
                }})
            );
            let send = |event: String| {
                Request::post("/p/events")
                    .header("Token", &token)
                    .body(Body::from(format!(
                        "{{{},\"source\":\"CONNECT\"}}",
                        event.replace("{sent}", &sent.to_string())
                    )))
                    .unwrap()
            };
            for event in [
                r#""event":"TRANSFER_INFO","command_id":{sent}"#,
                r#""event":"TRANSFER_FINISHED","data":{"start_cmd_id":{sent}}"#,
                r#""event":"STATE_CHANGED","state":"PRINTING""#,
                r#""event":"STATE_CHANGED","state":"ERROR","reason":"General print error.""#,
            ] {
                assert_eq!(
                    call(&app, send(event.into())).await.0,
                    StatusCode::NO_CONTENT
                );
            }
            // out of labels while it waits: the PPD's text, as the event's reason
            assert_eq!(
                call(
                    &app,
                    send(
                        r#""event":"STATE_CHANGED","state":"ATTENTION","reason":"Out of labels.""#
                            .into()
                    )
                )
                .await
                .0,
                StatusCode::NO_CONTENT
            );
            assert_eq!(
                call(&app, telemetry(r#"{"state":"ATTENTION"}"#)).await.0,
                StatusCode::NO_CONTENT
            );
            let t = store.machines(space).await.unwrap()[0]
                .telemetry
                .clone()
                .unwrap();
            assert_eq!(t.problem.as_deref(), Some("Out of labels."));
            if stop {
                store
                    .set_job(space, job, JobState::Stopping, None)
                    .await
                    .unwrap();
                let answer = app.clone().oneshot(idle()).await.unwrap();
                assert_eq!(answer.status(), StatusCode::OK);
                let frame = command_of(answer).await.expect("the stop");
                let command: Value = serde_json::from_str(&frame[9..]).unwrap();
                assert_eq!(command, json!({"command": "STOP_PRINT"}));
                for event in [
                    r#""event":"STATE_CHANGED","state":"ERROR","reason":"stopped""#,
                    r#""event":"STATE_CHANGED","state":"IDLE""#,
                ] {
                    assert_eq!(
                        call(&app, send(event.into())).await.0,
                        StatusCode::NO_CONTENT
                    );
                }
            } else {
                for event in [
                    r#""event":"STATE_CHANGED","state":"PRINTING""#,
                    r#""event":"STATE_CHANGED","state":"IDLE""#,
                ] {
                    assert_eq!(
                        call(&app, send(event.into())).await.0,
                        StatusCode::NO_CONTENT
                    );
                }
            }
            assert_eq!(call(&app, idle()).await.0, StatusCode::NO_CONTENT);
            let t = store.machines(space).await.unwrap()[0]
                .telemetry
                .clone()
                .unwrap();
            assert_eq!(t.problem, None, "cleared");
            let job = store.job(space, job).await.unwrap().unwrap();
            ends.push((job.state, job.reason));
        }
        assert_eq!(
            ends,
            vec![(JobState::Finished, None), (JobState::Stopped, None)]
        );
    }
}
