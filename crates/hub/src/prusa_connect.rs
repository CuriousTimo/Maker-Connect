//! The printer side of Prusa Connect: a Prusa with the Buddy firmware (a MINI) whose
//! Connect settings point here registers and reports to this hub itself, with no agent
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

/// The print head in use, from the `tools` object firmware 6.4 and later send (per tool,
/// numbered from "0"): the active one, or else the first head.
fn active_head(report: &Value) -> Option<&Value> {
    let tools = report["tools"].as_object()?;
    let heads = || {
        tools
            .values()
            .filter(|t| t["type"] == "FFF_PRINT_HEAD" || t.get("nozzle_temp").is_some())
    };
    heads()
        .find(|t| t["active"].as_bool() == Some(true))
        .or_else(|| heads().next())
}

/// Prusa Connect's telemetry, as Maker Connect keeps it. Firmware before 6.4 sends the
/// values flat (`temp_nozzle`, `target_nozzle`, `fan_print`, ...); later firmware per tool
/// under `tools` (`nozzle_temp`, `nozzle_target_temp`, `print_fan_rpm`, `heatbreak_fan_rpm`).
fn to_telemetry(report: &Value) -> Telemetry {
    let head = active_head(report);
    let of_head = |key: &str| head.and_then(|h| h[key].as_f64());
    let state = match report["state"].as_str().unwrap_or("") {
        "PRINTING" | "BUSY" => MachineState::Busy,
        "PAUSED" => MachineState::Paused,
        "ATTENTION" => MachineState::Attention,
        "ERROR" => MachineState::Error,
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
        nozzle_c: of_head("nozzle_temp")
            .or_else(|| find_number(report, &["temp_nozzle", "nozzle_temp"]))
            .map(|t| t as f32),
        bed_c: find_number(report, &["temp_bed", "bed_temp"]).map(|t| t as f32),
        job,
        position_mm: position,
        // A head that is off says null (6.4 and later): 0, not "unknown".
        nozzle_target_c: number("target_nozzle").or_else(|| {
            head.filter(|h| h.get("nozzle_target_temp").is_some())
                .map(|h| h["nozzle_target_temp"].as_f64().unwrap_or(0.0) as f32)
        }),
        bed_target_c: number("target_bed"),
        // Before 6.4 only while printing.
        print_fan_rpm: report["fan_print"]
            .as_f64()
            .or_else(|| of_head("print_fan_rpm"))
            .map(|n| n as u32),
        hotend_fan_rpm: report["fan_extruder"]
            .as_f64()
            .or(report["fan_hotend"].as_f64())
            .or_else(|| of_head("heatbreak_fan_rpm"))
            .map(|n| n as u32),
        speed_percent: number("speed"),
        flow_percent: number("flow"),
        material: report["material"].as_str().map(str::to_owned),
        // The CORE One's chamber (`chamber.temp`, `target_temp`: 0 is none set), or the XL's
        // enclosure (`enclosure.temp`).
        chamber_c: report["chamber"]["temp"]
            .as_f64()
            .or(report["enclosure"]["temp"].as_f64())
            .map(|t| t as f32),
        chamber_target_c: report["chamber"]["target_temp"].as_f64().map(|t| t as f32),
        tools: tools(report),
        ..Telemetry::default()
    }
}

/// Each print head of a machine with more than one (the XL, an INDX), from the `tools`
/// object of firmware 6.4 and later ("0", "1", …: numbered from 1 here, as on its screen).
fn tools(report: &Value) -> Vec<maker_connect_protocol::ToolHeat> {
    let Some(tools) = report["tools"].as_object() else {
        return Vec::new();
    };
    let mut heads: Vec<maker_connect_protocol::ToolHeat> = tools
        .iter()
        .filter(|(_, t)| t["type"] == "FFF_PRINT_HEAD" || t.get("nozzle_temp").is_some())
        .filter_map(|(key, t)| {
            Some(maker_connect_protocol::ToolHeat {
                number: key.parse::<u32>().ok()? + 1,
                nozzle_c: t["nozzle_temp"].as_f64().map(|c| c as f32),
                nozzle_target_c: t
                    .get("nozzle_target_temp")
                    .map(|v| v.as_f64().unwrap_or(0.0) as f32),
                active: t["active"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    heads.sort_by_key(|h| h.number);
    if heads.len() > 1 { heads } else { Vec::new() }
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
    telemetry.chamber_c = telemetry.chamber_c.or(last.chamber_c);
    telemetry.chamber_target_c = telemetry.chamber_target_c.or(last.chamber_target_c);
    if telemetry.tools.is_empty() {
        telemetry.tools = last.tools;
    }
}

/// What an `INFO` event tells about the printer, its secrets (the Prusa Link key) left out.
fn to_details(info: &Value) -> Details {
    let some = |value: &Value| value.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let network = &info["network_info"];
    Details {
        serial: some(&info["sn"]),
        // Buddy: "printer_type"; Prusa's Connect SDK (PrusaLink): "type", "version",
        // "subversion" side by side.
        printer_type: printer_type(&info["printer_type"])
            .or_else(|| info["type"].as_u64().and_then(|_| printer_type(info))),
        firmware: some(&info["firmware"]),
        // Top level before 6.4 (kept since, "deprecated"); per head under `features` after.
        nozzle_mm: info["nozzle_diameter"].as_f64().or_else(|| {
            info["tools"]
                .as_object()?
                .values()
                .find_map(|t| t["features"]["nozzle_diameter"].as_f64())
        }),
        // A name per tool before 6.4; later the loaded material's abbreviation ("PETG")
        // under material_package_instance.package.material.
        material: info["tools"].as_object().and_then(|tools| {
            tools.values().find_map(|t| {
                some(&t["material"]).or_else(|| {
                    some(&t["material_package_instance"]["package"]["material"]["abbreviation"])
                })
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
    // G-code still running in the background: its command is not given up on.
    if let Some(id) = report["command_id"].as_u64() {
        let _ = store.command_still_running(token, id as u32);
    }
    let reported = store.report(token, &telemetry);
    // A job for it goes to its stick when it is free; one printing follows its state.
    if let Ok(Some(_)) = reported
        && let Some(state) = report["state"].as_str()
        && let Err(error) = crate::printer_files::on_report(&mut store, token, state)
    {
        return failed(error);
    }
    match reported {
        // A command goes back as the answer, G-code as text or JSON; none: an empty answer.
        Ok(Some(_)) => match store.next_command_of_any_kind(token) {
            Ok(Some((id, body, json))) => {
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
            // A command that asked for something is done when the answer comes. Deleting a
            // file or folder and making a folder answer with FILE_CHANGED, not FINISHED
            // (Buddy 6.3's connect/planner.cpp: "the result is generated through the
            // FILE_CHANGED event indirectly").
            // SET_PRINTER_READY (since 6.4) and SEND_STATE_INFO answer with STATE_CHANGED,
            // CANCEL_OBJECT and UNCANCEL_OBJECT with CANCELABLE_CHANGED.
            "FINISHED" | "FILE_INFO" | "FILE_CHANGED" | "JOB_INFO" | "INFO" | "TRANSFER_INFO"
            | "STATE_CHANGED" | "CANCELABLE_CHANGED" => {
                Some(CommandState::Finished)
            }
            // Refused at once, or taken on and then failed (Buddy's FAILED): either way not done.
            "REJECTED" | "FAILED" => Some(CommandState::Rejected),
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
    // Files on its stick, and transfers to it.
    if let Err(error) = crate::printer_files::on_event(&mut store, token, &name, &event) {
        return failed(error);
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

    /// A MINI pointed at this hub registers, shows its code, waits, is added with that
    /// code on the website, gets its token, and then shows with its telemetry and profile.
    #[tokio::test]
    async fn a_mini_registers_by_itself() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
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

    /// Firmware 6.4 and later (Buddy's connect/render.cpp): the head's values under
    /// `tools`, the active head counted, a head that is off with a null target.
    #[test]
    fn the_active_head_tells_on_new_firmware() {
        let report: Value = serde_json::from_str(
            r#"{"state":"PRINTING","temp_bed":60.1,"target_bed":60,"speed":100,"flow":95,"axis_z":1.2,
                "tools":{
                  "0":{"type":"FFF_PRINT_HEAD","active":false,"nozzle_temp":30.0,"nozzle_target_temp":null,"heatbreak_fan_rpm":0,"print_fan_rpm":0},
                  "1":{"type":"FFF_PRINT_HEAD","active":true,"nozzle_temp":215.3,"nozzle_target_temp":215,"heatbreak_fan_rpm":7100,"print_fan_rpm":5200}
                },"job_id":7,"progress":40,"time_printing":600,"time_remaining":900}"#,
        )
        .unwrap();
        let t = to_telemetry(&report);
        assert_eq!(t.nozzle_c, Some(215.3));
        assert_eq!(t.nozzle_target_c, Some(215.0));
        assert_eq!((t.print_fan_rpm, t.hotend_fan_rpm), (Some(5200), Some(7100)));
        assert_eq!(t.flow_percent, Some(95.0));
        assert_eq!(t.job.map(|j| j.done), Some(40));
        // Two heads: each one, numbered from 1; the active one's nozzle on top.
        assert_eq!(t.tools.len(), 2);
        assert_eq!((t.tools[1].number, t.tools[1].active), (2, true));
        assert_eq!(t.tools[0].nozzle_target_c, Some(0.0));
        // A CORE One's chamber, and an XL's enclosure.
        let core: Value = serde_json::from_str(
            r#"{"state":"IDLE","chamber":{"temp":31.5,"target_temp":40,"fan_1_rpm":0,"fan_2_rpm":0,"fan_pwm_target":0,"led_intensity":100},
                "tools":{"0":{"type":"FFF_PRINT_HEAD","active":true,"nozzle_temp":24.0,"nozzle_target_temp":null}}}"#,
        )
        .unwrap();
        let c = to_telemetry(&core);
        assert_eq!((c.chamber_c, c.chamber_target_c), (Some(31.5), Some(40.0)));
        assert!(c.tools.is_empty(), "one head: no list");
        let xl: Value = serde_json::from_str(r#"{"state":"IDLE","enclosure":{"temp":28,"fan_rpm":0,"time_in_use":3600}}"#).unwrap();
        assert_eq!(to_telemetry(&xl).chamber_c, Some(28.0));
        // Off: the target is 0, not unknown.
        let off: Value = serde_json::from_str(
            r#"{"state":"IDLE","tools":{"0":{"type":"FFF_PRINT_HEAD","active":true,"nozzle_temp":24.0,"nozzle_target_temp":null}}}"#,
        )
        .unwrap();
        assert_eq!(to_telemetry(&off).nozzle_target_c, Some(0.0));
    }

    /// INFO on firmware 6.4 and later: the nozzle under `features`, the material as an
    /// abbreviation deep in `material_package_instance`.
    #[test]
    fn info_on_new_firmware() {
        let info: Value = serde_json::from_str(
            r##"{"firmware":"8.0.1+12","printer_type":"2.1.0","sn":"CZPX1234",
                "tools":{"0":{"type":"FFF_PRINT_HEAD",
                  "features":{"nozzle_diameter":0.60,"nozzle_hardened":true,"nozzle_high_flow":false},
                  "material_package_instance":{"class":"FFF","package":{"class":"FFF",
                    "material":{"class":"FFF","abbreviation":"PETG","primary_color":{"color_rgba":"#ff8000"},"tags":[]}}}}}}"##,
        )
        .unwrap();
        let details = to_details(&info);
        assert_eq!(details.nozzle_mm, Some(0.6));
        assert_eq!(details.material.as_deref(), Some("PETG"));
    }

    /// INFO from Prusa's Connect SDK (PrusaLink on an MK3S): the type in three fields.
    #[test]
    fn info_from_the_connect_sdk() {
        let info: Value = serde_json::from_str(
            r#"{"type":1,"version":3,"subversion":1,"firmware":"3.14.1","sdk":"0.8.1",
                "sn":"CZPX4720X004XC12345","fingerprint":"abc","network_info":{"hostname":"prusalink"}}"#,
        )
        .unwrap();
        let details = to_details(&info);
        assert_eq!(details.printer_type.as_deref(), Some("1.3.1"));
        assert_eq!(details.firmware.as_deref(), Some("3.14.1"));
    }
}
