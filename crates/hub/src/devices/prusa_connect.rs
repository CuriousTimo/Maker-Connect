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
use maker_connect_protocol::{Identity, JobProgress, MachineKind, MachineState, Telemetry};
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
    if machine.profile.as_deref() == Some(LABELWRITER_450) {
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
/// storage by the SDK's `START_CONNECT_DOWNLOAD`, not to a stick by Buddy's encrypted download.
pub(crate) fn marlin_agent(machine: &crate::store::Machine) -> bool {
    machine
        .details
        .as_ref()
        .and_then(|d| d.printer_type.as_deref())
        == Some(MARLIN_AGENT)
}

/// The LabelWriter 450's profile (docs/design/wire.md, "The second device").
const LABELWRITER_450: &str = "dymo/labelwriter-450";

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
    // Connected while the socket is attached (`Session.Connected`, `xbuddy/session.go:466-471`).
    let _attached = match store.caller(&token).await {
        Ok(Some(caller)) => Attached::new(&store, caller.id),
        _ => return,
    };
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

/// A printer's `/p/ws` socket, counted in [`crate::store::Legacy::attached`] while it is open.
struct Attached {
    store: Store,
    machine: String,
}

impl Attached {
    fn new(store: &Store, machine: String) -> Self {
        let mut legacy = store.lock().unwrap();
        *legacy.attached.entry(machine.clone()).or_default() += 1;
        legacy.changed();
        drop(legacy);
        Self {
            store: store.clone(),
            machine,
        }
    }
}

impl Drop for Attached {
    fn drop(&mut self) {
        let mut legacy = self.store.lock().unwrap();
        if let Some(n) = legacy.attached.get_mut(&self.machine) {
            *n -= 1;
            if *n == 0 {
                legacy.attached.remove(&self.machine);
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
    // A Marlin printer on the agent (`docs/design/prusa-sdk-extension.md`, part 9): no profile
    // knows its model from the type; the space chooses one on its Machine tab.
    let marlin = identity.printer_type.as_deref() == Some(MARLIN_AGENT);
    let fallback = if marlin {
        "Marlin printer"
    } else {
        "Prusa printer"
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
    let kind = profile.map_or(MachineKind::Fdm, |p| p.kind);
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
        if marlin {
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

/// A telemetry frame taken in over the machine's last telemetry: as Prusa's server keeps a
/// printer's state (`reference/prusa-connect` `internal/connector/xbuddy`, read only: OCL, nothing
/// copied), a frame writes only the members it carries, a null removes one, and everything it
/// leaves out stays as it was (`inbound.go:136-159`, `:273-299`). Buddy sends a Reduced frame most
/// of the time, without the temperatures, targets, speed, flow and position, and a Full one only
/// when its fingerprint changes or every 5 minutes (v8.0.2 `planner.cpp:111-113`, `:537-557`;
/// `render.cpp:177`).
///
/// - The job's members (`job_id`, `progress`, `time_printing`, `time_remaining`) land on `/job/*`
///   when carried and are kept whatever the state, as the server keeps them (`inbound.go:146-159`;
///   only `/job/state` is the server's to clear, `jobrefresh.go:425-440`). The file never comes
///   here (JOB_INFO tells it).
/// - Each axis lands on `/axis/x|y|z` when carried (`inbound.go:150-152`): while it prints Buddy
///   tells only `axis_z` (`render.cpp:212-217`), so X and Y stay as told last, and unknown when
///   never told.
/// - The tools, in whichever shape the frame has, are read onto the tool tree as the server reads
///   them (`telemetrytools.go:101-144`; `crate::devices::connect_tools`).
fn take_in(
    report: &Value,
    last: Option<Telemetry>,
    session: &mut crate::devices::connect_tools::Session,
    model: &str,
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
    // A flow of 0 is none read, not a flow of nothing (`inbound.go:287-293`).
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
    session.telemetry(&mut t, report, model);
    session.job.telemetry(
        &mut t,
        report["job_id"].as_i64(),
        report["state"].as_str().unwrap_or(""),
    );
    t
}

/// The job as a reader of the machine sees it: the stored telemetry keeps `/job/*` after the print
/// (the server's state tree, [`take_in`]), and the server's readers take a print as current only
/// while `jobFacts.running` holds over the machine's word and the print's
/// (`internal/app/devicejobs.go:270-290`, `:325-331`; [`crate::devices::connect_job::is_running`]).
/// Every reader of [`crate::store::Machine`] goes through this (`MachineRow::into_machine`), and
/// so does the following of a print the printer runs itself (`telemetry`); what a job used is
/// counted off the stored telemetry instead, as the server keeps a stopped print's last figure
/// (`devicejobs.go:230-235`).
pub(crate) fn current_job(t: &mut Telemetry) {
    if !crate::devices::connect_job::is_running(t) {
        t.job = None;
    }
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
        // Buddy 6.3.0 writes it at the top level of INFO (`render.cpp:352`).
        nozzle_mm: info["nozzle_diameter"].as_f64(),
        // A name per tool under `tools.<n>.material`, "---" for a tool with nothing loaded
        // (`render.cpp:395`): no material, as the server normalizes it (`tools.go:612-626`).
        material: info["tools"].as_object().and_then(|tools| {
            tools.values().find_map(|t| {
                t.get("material")
                    .filter(|m| !m.is_null())
                    .map(crate::devices::connect_tools::normalize_material)
                    .and_then(|m| m.as_str().filter(|m| !m.is_empty()).map(str::to_owned))
            })
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
    // The model, as the server reads it off the request (`identityOf`, `xbuddy.go:817-835`;
    // Buddy's `httpc.cpp:218`).
    let model = header(&headers, "user-agent-printer");
    let job_id = report["job_id"].as_u64();
    // A printer runs Prusa's firmware; any other kind on this protocol is our agent.
    let agent = matches!(
        store.machine_by_token(token).await,
        Ok(Some((m, _))) if m.kind != MachineKind::Fdm
    );
    let telemetry = {
        let mut store = store.lock().unwrap();
        let file = store.printing_file.get(token).cloned();
        let session = store.connect_tools.entry(machine).or_default();
        let last = session.last.take().or(stored);
        let mut telemetry = take_in(&report, last, session, model);
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
    // A label printer, whose `STATE_CHANGED` reason is its problem (part 3).
    let label = matches!(
        shared.machine_by_token(token).await,
        Ok(Some((m, _))) if m.kind == MachineKind::Label
    );
    let mut ended_label = None;
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
                store.command_went(
                    caller.as_ref(),
                    id as u32,
                    state,
                    reason.clone(),
                    Vec::new(),
                );
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
        store
            .connect_tools
            .entry(caller.id.clone())
            .or_default()
            .info(&event["data"], header(&headers, "user-agent-printer"));
    }
    // Every event carries the machine's state, and a JOB_INFO the print's (`inbound.go:245-258`).
    if let Some(caller) = caller.as_ref() {
        let mut store = shared.lock().unwrap();
        store
            .connect_tools
            .entry(caller.id.clone())
            .or_default()
            .job
            .event(&name, event["state"].as_str().unwrap_or(""), &event["data"]);
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

    use crate::devices::connect_tools::{MATERIAL, Session, normalize_material};
    use serde_json::json;

    /// One printer's frames, taken in one after the other in one session, as `telemetry` does;
    /// `model` is its `User-Agent-Printer`.
    struct Printer {
        session: Session,
        last: Option<Telemetry>,
        model: &'static str,
    }

    impl Printer {
        fn new(model: &'static str) -> Self {
            Self {
                session: Session::default(),
                last: None,
                model,
            }
        }

        fn frame(&mut self, report: Value) -> Telemetry {
            let t = take_in(&report, self.last.take(), &mut self.session, self.model);
            self.last = Some(t.clone());
            t
        }

        fn info(&mut self, data: Value) {
            self.session.info(&data, self.model);
        }
    }

    /// A frame taken in over nothing before it, from a model the server has no figures for.
    fn first(report: Value) -> Telemetry {
        Printer::new("").frame(report)
    }

    fn at(x: f32, y: f32, z: f32) -> Option<[Option<f32>; 3]> {
        Some([Some(x), Some(y), Some(z)])
    }

    /// The MINI while printing on 6.3.0 (2026-11-08): a full report now and then (Z only), short
    /// ones every second in between (job, progress, times). Each axis is held as told last
    /// (`/axis/*`, `inbound.go:150-152`), the time left too, and the job's members stay when the
    /// print is over (`/job/*`, `inbound.go:155-158`).
    #[test]
    fn a_printing_mini_tells_z_and_keeps_the_rest() {
        let mut mini = Printer::new("MINI");
        let before = mini.frame(json!({
            "state": "IDLE", "axis_x": 176.4, "axis_y": 24.1, "axis_z": 2.0, "temp_nozzle": 27.0
        }));
        assert_eq!(before.position_mm, at(176.4, 24.1, 2.0));
        let full = mini.frame(json!({
            "state": "PRINTING", "axis_z": 1.8, "fan_extruder": 2786, "fan_print": 2337,
            "job_id": 29, "progress": 5
        }));
        assert_eq!(
            full.position_mm,
            at(176.4, 24.1, 1.8),
            "X and Y as told last"
        );
        assert_eq!(full.nozzle_c, Some(27.0), "not in the frame: as told last");
        // The fans at the top of a Buddy 6 frame are not read, as the server does not.
        assert_eq!((full.print_fan_rpm, full.hotend_fan_rpm), (None, None));
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
            "the server keeps /job/* whatever the state"
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

    /// The one-path shape (no `tools`, no `slot`): a null `temp_nozzle` removes the head's nozzle
    /// (`reportHead`, `telemetrytools.go:184-201`), and with the tree empty nothing of the frame
    /// before is shown (its nozzle, target and fans).
    #[test]
    fn a_null_nozzle_on_the_one_path_shape_shows_none() {
        let mut mini = Printer::new("");
        let told = mini.frame(json!({
            "state": "IDLE", "temp_nozzle": 215.0, "target_nozzle": 215.0
        }));
        assert_eq!(
            (told.nozzle_c, told.nozzle_target_c),
            (Some(215.0), Some(215.0))
        );
        let gone = mini.frame(json!({
            "state": "IDLE", "temp_nozzle": null, "target_nozzle": null
        }));
        assert!(gone.tool_tree.is_empty(), "{:?}", gone.tool_tree);
        assert_eq!(
            (
                gone.nozzle_c,
                gone.nozzle_target_c,
                gone.print_fan_rpm,
                gone.hotend_fan_rpm,
                gone.material.as_deref()
            ),
            (None, None, None, None, None)
        );
    }

    /// A reader sees the kept job only while the print runs (`jobFacts.running`,
    /// `devicejobs.go:325-331`: PRINTING, PAUSED, ATTENTION); the telemetry keeps it.
    #[test]
    fn the_kept_job_is_current_only_while_it_runs() {
        let mut mini = Printer::new("MINI");
        for (state, current) in [
            ("PRINTING", true),
            ("PAUSED", true),
            ("ATTENTION", true),
            ("FINISHED", false),
            ("STOPPED", false),
            ("IDLE", false),
            ("ERROR", false),
        ] {
            let mut t = mini.frame(json!({"state": state, "job_id": 3, "progress": 50}));
            assert!(t.job.is_some(), "{state}: kept");
            current_job(&mut t);
            assert_eq!(t.job.is_some(), current, "{state}");
        }
    }

    /// A printer on Prusa Connect is connected while its socket is attached
    /// (`xbuddy/session.go:466-471`), not while its last frame is recent: Buddy's websocket
    /// telemetry comes every 15 s when nothing changes (`planner.cpp:97-99`). Closed: offline.
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
        let socket = Attached::new(&store, mini.clone());
        assert!(online(store.machine(space, &mini).await.unwrap().unwrap()));
        drop(socket);
        assert!(!online(store.machine(space, &mini).await.unwrap().unwrap()));
    }

    /// Bed levelling after a print (G29: BUSY, `jobrefresh.go:76-81`) brings the kept job back
    /// to no reader: the server counts a BUSY only over a print that still runs
    /// (`devicejobs.go:325-331`). A long move within a print keeps it.
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
        // Cooled down: the bed's target 0, the nozzle's null (off, `render.cpp:233-237`): 0 on the
        // tree (`telemetrytools.go:453-454`).
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
        // A flow of 0 is no reading (`inbound.go:287-293`).
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

    /// A null head in Buddy 8's `tools` loses the readings this session wrote for it, and only
    /// those (`staleReadings`, `telemetrytools.go:350-391`): its active member stays, and a head
    /// whose readings came from before this session keeps them.
    #[test]
    fn a_null_head_clears_only_what_it_wrote() {
        let mut xl = Printer::new("XL");
        xl.frame(json!({"state": "IDLE", "tools": {
            "0": {"active": true, "nozzle_temp": 200.0, "nozzle_target_temp": 200},
            "1": {"active": false, "nozzle_temp": 180.0, "nozzle_target_temp": 0,
                "consumed_weight": 3.5}
        }}));
        let off = xl.frame(json!({"state": "IDLE", "tools": {
            "0": {"active": true, "nozzle_temp": 201.0}, "1": null
        }}));
        assert_eq!(
            off.tool_tree.get("1"),
            Some(&serde_json::Map::from_iter([(
                "active".to_owned(),
                json!(false)
            )])),
            "its readings go, its active member stays"
        );
        assert_eq!(
            off.tools
                .iter()
                .map(|h| (h.number, h.nozzle_c, h.nozzle_target_c))
                .collect::<Vec<_>>(),
            [(1, Some(201.0), Some(200.0)), (2, None, None)]
        );
        // A new session (the printer linked again) never wrote head 1's readings: a null leaves
        // them.
        let mut again = Printer::new("XL");
        again.last = xl.last.clone().map(|mut t| {
            t.tool_tree
                .get_mut("1")
                .unwrap()
                .insert("nozzle_temp".to_owned(), json!(25.0));
            t
        });
        let kept = again.frame(json!({"state": "IDLE", "tools": {"1": null}}));
        assert_eq!(kept.tool_tree["1"]["nozzle_temp"], json!(25.0));
    }

    /// Buddy 6's `slot` block in front of an MMU (the layout from INFO's `mmu` block,
    /// `tools.go:310-331`): entry `n` is slot `0.<n-1>` with its material; the nozzle and target at
    /// the top are the one head's, its fans the selected entry's (`telemetrytools.go:146-158`,
    /// `:242-257`); the material at the top of the frame is not read in this shape.
    #[test]
    fn a_slot_block_in_front_of_an_mmu() {
        let mut mk4 = Printer::new("MK4");
        mk4.info(json!({
            "tools": {"1": {"material": "PLA"}, "2": {"material": "---"}, "3": {"material": "PETG"}},
            "mmu": {"enabled": true, "version": "3.0.3"}, "slots": 3
        }));
        let frame = |active: u32| {
            json!({
                "state": "PRINTING", "temp_nozzle": 214.0, "target_nozzle": 215, "material": "ASA",
                "slot": {"active": active, "state": 1, "command": "-",
                    "1": {"material": "PLA", "temp": 214, "fan_print": 0, "fan_hotend": 6900},
                    "2": {"material": "---", "temp": 214, "fan_print": 100, "fan_hotend": 7000},
                    "3": {"material": "PETG", "temp": 214, "fan_print": 5200, "fan_hotend": 7100}}
            })
        };
        let t = mk4.frame(frame(3));
        assert!(t.tools.is_empty(), "one head");
        assert_eq!((t.nozzle_c, t.nozzle_target_c), (Some(214.0), Some(215.0)));
        assert_eq!(
            (t.print_fan_rpm, t.hotend_fan_rpm),
            (Some(5200), Some(7100))
        );
        assert_eq!(t.material.as_deref(), Some("PETG"), "the loaded slot's");
        assert_eq!(
            t.tool_tree["0.1"][MATERIAL],
            json!(""),
            "\"---\": no material"
        );
        assert!(
            !t.tool_tree["0"].contains_key(MATERIAL),
            "the top's material is not read"
        );
        let empty = mk4.frame(frame(2));
        assert_eq!(empty.material, None, "\"---\" is no material");
        assert_eq!(
            (empty.print_fan_rpm, empty.hotend_fan_rpm),
            (Some(100), Some(7000))
        );
        // Before any INFO, a model with one head reads the block as slots too
        // (`currentLayout`, `telemetrytools.go:68-81`).
        let t = Printer::new("MINI").frame(frame(1));
        assert!(t.tools.is_empty());
        assert_eq!(t.material.as_deref(), Some("PLA"));
    }

    /// Buddy 6's `slot` block on a toolchanger (an XL): entry `n` is head `n-1` with its
    /// temperature and fans; only the target at the top is read, onto the picked head, and the
    /// head it was on before loses it (`reportTopReadings`, `telemetrytools.go:146-182`).
    #[test]
    fn a_toolchangers_top_target_goes_to_the_picked_head() {
        let mut xl = Printer::new("XL");
        xl.info(json!({"tools": {"1": {"material": "PLA"}, "2": {"material": "PETG"}}}));
        let frame = |active: u32, target: u32| {
            json!({
                "state": "PRINTING", "temp_nozzle": 999.0, "target_nozzle": target,
                "slot": {"active": active,
                    "1": {"material": "PLA", "temp": 200.0, "fan_print": 10, "fan_hotend": 20},
                    "2": {"material": "PETG", "temp": 30.0, "fan_print": 30, "fan_hotend": 40}}
            })
        };
        let heads = |t: &Telemetry| {
            t.tools
                .iter()
                .map(|h| (h.number, h.nozzle_c, h.nozzle_target_c, h.active))
                .collect::<Vec<_>>()
        };
        let one = xl.frame(frame(1, 210));
        assert_eq!(
            heads(&one),
            [
                (1, Some(200.0), Some(210.0), true),
                (2, Some(30.0), None, false)
            ]
        );
        assert_eq!(
            one.nozzle_c,
            Some(200.0),
            "the top temp_nozzle is not read here"
        );
        assert_eq!(
            (one.print_fan_rpm, one.hotend_fan_rpm),
            (Some(10), Some(20))
        );
        assert_eq!(one.material.as_deref(), Some("PLA"));
        let two = xl.frame(frame(2, 220));
        assert_eq!(
            heads(&two),
            [
                (1, Some(200.0), None, false),
                (2, Some(30.0), Some(220.0), true)
            ]
        );
        // None picked: the target says nothing of a head, and the last one loses it.
        let none = xl.frame(frame(0, 0));
        assert_eq!(
            heads(&none),
            [(1, Some(200.0), None, false), (2, Some(30.0), None, false)]
        );
        assert_eq!(
            none.nozzle_c, None,
            "no head picked, more than one: none at the top"
        );
    }

    /// The firmware's material, normalized as the server does (`normalizeMaterial`,
    /// `tools.go:612-626`): `"---"` and an empty name are no material.
    #[test]
    fn material_is_normalized_as_the_server_does() {
        assert_eq!(normalize_material(&json!("---")), json!(""));
        assert_eq!(normalize_material(&json!("  ")), json!(""));
        assert_eq!(normalize_material(&json!(" --- ")), json!(""));
        assert_eq!(normalize_material(&json!(null)), json!(""));
        assert_eq!(normalize_material(&json!("PLA")), json!("PLA"));
        assert_eq!(normalize_material(&json!(5)), json!(5));
        let mut mini = Printer::new("MINI");
        assert_eq!(
            mini.frame(json!({"state": "IDLE", "material": "PETG"}))
                .material
                .as_deref(),
            Some("PETG")
        );
        assert_eq!(
            mini.frame(json!({"state": "IDLE", "material": "---"}))
                .material,
            None
        );
        assert_eq!(
            mini.frame(json!({"state": "IDLE", "material": ""}))
                .material,
            None
        );
        let info =
            to_details(&json!({"tools": {"1": {"material": " "}, "2": {"material": "ASA"}}}));
        assert_eq!(info.material.as_deref(), Some("ASA"));
    }

    /// A report with only the state keeps the temperatures of the one before.
    #[test]
    fn a_short_report_keeps_what_it_left_out() {
        let mut mini = Printer::new("MINI");
        mini.frame(
            json!({"state":"IDLE","temp_nozzle":24,"temp_bed":23,"axis_x":1,"axis_y":2,"axis_z":3}),
        );
        let short = mini.frame(json!({"state": "IDLE"}));
        assert_eq!(short.nozzle_c, Some(24.0));
        assert_eq!(short.position_mm, at(1.0, 2.0, 3.0));
    }

    /// Buddy 6.3.0's telemetry (render.cpp:129-257): flat values for the one head; its fans at
    /// the top are not read, as the server reads only the nozzle and material there
    /// (`telemetrytools.go:184-206`).
    #[test]
    fn the_flat_keys_of_6_3_0_tell_the_heads_values() {
        let t = Printer::new("MINI").frame(
            json!({"job_id":7,"time_printing":600,"time_remaining":900,"progress":40,
            "temp_nozzle":215.3,"temp_bed":60.1,"target_nozzle":215.0,"target_bed":60.0,
            "speed":100,"flow":95,"material":"PETG","axis_z":1.20,
            "fan_extruder":7100,"fan_print":5200,"filament":3.5,"state":"PRINTING"}),
        );
        assert_eq!((t.nozzle_c, t.nozzle_target_c), (Some(215.3), Some(215.0)));
        assert_eq!((t.bed_c, t.bed_target_c), (Some(60.1), Some(60.0)));
        assert_eq!((t.print_fan_rpm, t.hotend_fan_rpm), (None, None));
        assert_eq!(t.flow_percent, Some(95.0));
        assert_eq!(t.material.as_deref(), Some("PETG"));
        assert_eq!(t.job.map(|j| j.done), Some(40));
    }

    /// A CORE One's chamber and an XL's enclosure (render.cpp:178-199), read as the server reads
    /// them (`telemetrychamber.go:32-124`).
    #[test]
    fn the_chamber_and_the_enclosure() {
        let mut core = Printer::new("CoreOne");
        let c = core.frame(json!({"chamber":{"temp":31.5,"target_temp":40,"fan_1_rpm":0,"fan_2_rpm":0,"fan_pwm_target":0,"led_intensity":100},"state":"IDLE"}));
        assert_eq!((c.chamber_c, c.chamber_target_c), (Some(31.5), Some(40.0)));
        assert!(c.tools.is_empty(), "no slot: no list");
        let xl =
            first(json!({"enclosure":{"temp":28,"fan_rpm":0,"time_in_use":3600},"state":"IDLE"}));
        assert_eq!(xl.chamber_c, Some(28.0));
        // A Reduced frame keeps the reading; a full one with neither block: switched off
        // (`telemetrychamber.go:43-45`).
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
        // Idle again: the stored telemetry keeps the job's members (`/job/*`), and no reader of
        // the machine sees a print (`jobFacts.running`, `devicejobs.go:325-331`).
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
    /// it, and on the wire exactly as Prusa's server sends `core.command.mesh_bed_leveling`
    /// (`internal/connector/xbuddy/verbs.go:303-308`, `wire_test.go:29`): a G frame, its id as 8
    /// hex digits, then `G28\r\nG29` (`wire.go:53-58`); over HTTP the same G-code as the answer.
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
