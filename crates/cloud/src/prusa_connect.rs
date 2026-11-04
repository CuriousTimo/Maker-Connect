//! The printer side of Prusa Connect: a Prusa with the Buddy firmware (a MINI) whose
//! Connect settings point here registers and reports to this cloud itself, with no agent
//! next to it. The protocol is described in `docs/prusa-connect-protocol.md`, learned from
//! Prusa-Firmware-Buddy's `src/connect` (GPL-3.0); nothing of it is copied.
//!
//! 1. `POST /p/register` (its serial number, fingerprint, printer type, firmware): the
//!    machine is created, and its pairing code goes back in the `Code` header. The printer
//!    shows it; the owner enters it on the website, as for an agent.
//! 2. `GET /p/register` with that `Code`: `202` while it waits, then `200` with the
//!    machine's token in the `Token` header.
//! 3. `POST /p/telemetry` and `POST /p/events` with `Token` (and `Fingerprint`).
//!
//! Not yet: the WebSocket (`/p/ws`) and the commands, and files.

use crate::store::{CommandState, Details, Network, Storage};
use crate::{Shared, failed};
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
    let (parts, body) = request.into_parts();
    let path = parts.uri.path().to_owned();
    let bytes = axum::body::to_bytes(body, 1 << 20)
        .await
        .unwrap_or_default();
    let watched = path.starts_with("/p/") || path.starts_with("/f/");
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
    let method = parts.method.clone();
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

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/p/register", post(register).get(registered))
        .route("/p/telemetry", post(telemetry))
        .route("/p/events", post(event))
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

/// The printer type as a profile knows it: `2.1.0`, also when it comes as numbers.
fn printer_type(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Object(parts) => {
            let part = |k: &str| parts.get(k).and_then(Value::as_u64).unwrap_or(0);
            Some(format!(
                "{}.{}.{}",
                part("type"),
                part("version"),
                part("subversion")
            ))
        }
        _ => None,
    }
}

async fn register(State(store): State<Shared>, body: String) -> Response {
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
    let store = store.lock().unwrap();
    let registered =
        store
            .register_linked(&machine, "prusa-connect")
            .and_then(|(id, token, code)| {
                if let Some(profile) = profile {
                    store.recognise(&token, &profile.id)?;
                }
                Ok((id, code))
            });
    match registered {
        Ok((_, code)) => {
            println!("Prusa Connect printer registered: {name}, code {code}");
            (StatusCode::OK, [("Code", code)]).into_response()
        }
        Err(error) => failed(error),
    }
}

/// The printer asks with its code whether it has been added yet.
async fn registered(State(store): State<Shared>, headers: HeaderMap) -> Response {
    match store
        .lock()
        .unwrap()
        .token_for_code(header(&headers, "code"))
    {
        Ok(Some(Some(token))) => (StatusCode::OK, [("Token", token)]).into_response(),
        Ok(Some(None)) => StatusCode::ACCEPTED.into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "unknown code: register again").into_response(),
        Err(error) => failed(error),
    }
}

/// The first number under any of these keys, however deep (per tool, per slot).
fn find_number(value: &Value, keys: &[&str]) -> Option<f64> {
    match value {
        Value::Object(map) => keys
            .iter()
            .find_map(|k| map.get(*k).and_then(Value::as_f64))
            .or_else(|| map.values().find_map(|v| find_number(v, keys))),
        Value::Array(items) => items.iter().find_map(|v| find_number(v, keys)),
        _ => None,
    }
}

/// Prusa Connect's telemetry, as Maker Connect keeps it.
fn to_telemetry(report: &Value) -> Telemetry {
    let state = match report["state"].as_str().unwrap_or("") {
        "PRINTING" | "BUSY" => MachineState::Busy,
        "PAUSED" => MachineState::Paused,
        "ERROR" | "ATTENTION" => MachineState::Error,
        _ => MachineState::Idle,
    };
    let job = find_number(report, &["progress"])
        .filter(|_| matches!(state, MachineState::Busy | MachineState::Paused))
        .map(|percent| JobProgress {
            file: String::new(),
            done: percent.clamp(0.0, 100.0) as u64,
            total: 100,
            elapsed_s: report["time_printing"].as_u64(),
            remaining_s: report["time_remaining"].as_u64(),
        });
    let number = |key: &str| report[key].as_f64().map(|n| n as f32);
    let position = match (number("axis_x"), number("axis_y"), number("axis_z")) {
        (Some(x), Some(y), Some(z)) => Some([x, y, z]),
        _ => None,
    };
    Telemetry {
        state,
        nozzle_c: find_number(report, &["temp_nozzle", "nozzle_temp"]).map(|t| t as f32),
        bed_c: find_number(report, &["temp_bed", "bed_temp"]).map(|t| t as f32),
        job,
        position_mm: position,
        nozzle_target_c: number("target_nozzle"),
        bed_target_c: number("target_bed"),
        // Only while printing, on firmware 6.
        print_fan_rpm: report["fan_print"].as_f64().map(|n| n as u32),
        hotend_fan_rpm: report["fan_extruder"]
            .as_f64()
            .or(report["fan_hotend"].as_f64())
            .map(|n| n as u32),
        speed_percent: number("speed"),
        flow_percent: number("flow"),
        material: report["material"].as_str().map(str::to_owned),
        ..Telemetry::default()
    }
}

/// The printer leaves out what has not changed (now and then it sends only its state):
/// what it left out is as it was.
fn fill_in(telemetry: &mut Telemetry, last: Telemetry) {
    telemetry.nozzle_c = telemetry.nozzle_c.or(last.nozzle_c);
    telemetry.bed_c = telemetry.bed_c.or(last.bed_c);
    telemetry.nozzle_target_c = telemetry.nozzle_target_c.or(last.nozzle_target_c);
    telemetry.bed_target_c = telemetry.bed_target_c.or(last.bed_target_c);
    telemetry.position_mm = telemetry.position_mm.or(last.position_mm);
    telemetry.speed_percent = telemetry.speed_percent.or(last.speed_percent);
    telemetry.flow_percent = telemetry.flow_percent.or(last.flow_percent);
    telemetry.material = telemetry.material.take().or(last.material);
}

/// What an `INFO` event tells about the printer, its secrets (the Prusa Link key) left out.
fn to_details(info: &Value) -> Details {
    let some = |value: &Value| value.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let network = &info["network_info"];
    Details {
        serial: some(&info["sn"]),
        printer_type: printer_type(&info["printer_type"]),
        firmware: some(&info["firmware"]),
        nozzle_mm: info["nozzle_diameter"].as_f64(),
        material: info["tools"]
            .as_object()
            .and_then(|tools| tools.values().find_map(|t| some(&t["material"]))),
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
    State(store): State<Shared>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    body: String,
) -> Response {
    let report = match parse(&body) {
        Ok(report) => report,
        Err(refused) => return refused.into_response(),
    };
    let token = header(&headers, "token");
    let mut store = store.lock().unwrap();
    let mut telemetry = to_telemetry(&report);
    // Every request says the firmware's version.
    let version = header(&headers, "user-agent-version");
    telemetry.firmware = (!version.is_empty()).then(|| format!("Prusa firmware {version}"));
    if let Ok(Some(last)) = store.last_telemetry(token) {
        fill_in(&mut telemetry, last);
    }
    if let Some(peer) = crate::peer_address(&headers, &extensions) {
        let _ = store.set_peer(token, &peer);
    }
    match store.report(token, &telemetry) {
        // A command goes back as the answer, G-code as text; none: an empty answer.
        Ok(Some(_)) => match store.next_command(token) {
            Ok(Some((id, gcode))) => {
                println!(
                    "command {id} to the printer: {}",
                    gcode.replace('\n', " | ")
                );
                (
                    StatusCode::OK,
                    [
                        ("Command-Id", id.to_string()),
                        ("Content-Type", "text/x.gcode".to_owned()),
                    ],
                    gcode,
                )
                    .into_response()
            }
            Ok(None) => StatusCode::NO_CONTENT.into_response(),
            Err(error) => failed(error),
        },
        Ok(None) => (StatusCode::UNAUTHORIZED, "unknown printer: register again").into_response(),
        Err(error) => failed(error),
    }
}

async fn event(State(store): State<Shared>, headers: HeaderMap, body: String) -> Response {
    let event = match parse(&body) {
        Ok(event) => event,
        Err(refused) => return refused.into_response(),
    };
    let token = header(&headers, "token");
    let mut store = store.lock().unwrap();
    let name = text(&event, "event");
    if let Some(id) = event["command_id"].as_u64() {
        let state = match name.as_str() {
            "ACCEPTED" => Some(CommandState::Accepted),
            "FINISHED" => Some(CommandState::Finished),
            "REJECTED" => Some(CommandState::Rejected),
            _ => None,
        };
        if let Some(state) = state {
            let reason = event["reason"].as_str().map(str::to_owned);
            println!("command {id}: {name} {}", reason.as_deref().unwrap_or(""));
            if let Err(error) = store.command_went(token, id as u32, state, reason, Vec::new()) {
                return failed(error);
            }
        }
    }
    let known = match name.as_str() {
        "INFO" => store.set_details(token, &to_details(&event["data"])),
        _ => store.knows(token),
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
    use crate::store::Store;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    async fn call(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap) {
        let response = app.clone().oneshot(request).await.unwrap();
        (response.status(), response.headers().clone())
    }

    /// A MINI pointed at this cloud registers, shows its code, waits, is added with that
    /// code on the website, gets its token, and then shows with its telemetry and profile.
    #[tokio::test]
    async fn a_mini_registers_by_itself() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::app(store.clone());
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

        let space = store.lock().unwrap().default_space().unwrap().0;
        assert!(store.lock().unwrap().pair(space, &code).unwrap().is_some());
        let (status, headers) = call(&app, ask()).await;
        assert_eq!(status, StatusCode::OK);
        let token = headers["token"].to_str().unwrap().to_owned();
        assert_eq!(
            call(&app, ask()).await.0,
            StatusCode::NOT_FOUND,
            "the token goes out once"
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
        let machines = store.lock().unwrap().machines(space).unwrap();
        let mini = &machines[0];
        assert_eq!(mini.name, "Original Prusa MINI/+ 2345");
        assert_eq!(mini.link, "prusa-connect");
        assert!(token.len() < 20, "Buddy keeps at most 19 characters");
        assert_eq!(
            mini.telemetry.as_ref().unwrap().firmware.as_deref(),
            Some("Prusa firmware 6.3.0+0.LOCAL")
        );
        assert_eq!(mini.profile.as_deref(), Some("prusa/mini"));
        let telemetry = mini.telemetry.as_ref().unwrap();
        assert_eq!(telemetry.state, MachineState::Busy);
        assert_eq!(telemetry.nozzle_c, Some(214.9));
        assert_eq!(telemetry.job.as_ref().unwrap().done, 42);

        // A command from the website goes back as the answer to its next telemetry, and the
        // printer tells how it went.
        let mini_id = mini.id.clone();
        let id = store
            .lock()
            .unwrap()
            .queue_command(space, &mini_id, "Home", "G28")
            .unwrap()
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
        let machines = store.lock().unwrap().machines(space).unwrap();
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
        let full = to_telemetry(&serde_json::from_str(r#"{"state":"IDLE","temp_nozzle":24,"temp_bed":23,"axis_x":1,"axis_y":2,"axis_z":3}"#).unwrap());
        let mut short = to_telemetry(&serde_json::from_str(r#"{"state":"IDLE"}"#).unwrap());
        fill_in(&mut short, full);
        assert_eq!(short.nozzle_c, Some(24.0));
        assert_eq!(short.position_mm, Some([1.0, 2.0, 3.0]));
    }

    /// Newer firmware reports the nozzle per tool.
    #[test]
    fn the_nozzle_is_found_per_tool() {
        let report: Value = serde_json::from_str(
            r#"{"state":"IDLE","temp_bed":22,"tools":{"0":{"nozzle_temp":23.5}}}"#,
        )
        .unwrap();
        let telemetry = to_telemetry(&report);
        assert_eq!(telemetry.nozzle_c, Some(23.5));
        assert_eq!(telemetry.bed_c, Some(22.0));
        assert!(telemetry.job.is_none());
    }
}
