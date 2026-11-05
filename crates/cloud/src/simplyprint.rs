//! Klipper printers, through Moonraker: the server side of the protocol Moonraker's
//! `[simplyprint]` component speaks, so a printer on Klipper connects here by itself, as a
//! Prusa MINI does with Prusa Connect. Two lines in its `moonraker.conf`:
//!
//! ```ini
//! [simplyprint]
//! url: ws://192.168.1.20:8460/simplyprint/<token>
//! ```
//!
//! The `url` option is in Moonraker's code (`components/simplyprint.py`), not in its docs:
//! a newer Moonraker may change it. Read from Moonraker (GPL-3.0) to learn the protocol;
//! none of its code is here. The printer opens a WebSocket and keeps it open; every message
//! either way is `{"type": …, "data": …}`.
//!
//! - **From the printer:** `temps` (`{"tool0": [now, target], "bed": [now]}`, the target only
//!   when it changed), `state_change` (`operational`, `printing`, `paused`, `error`, …),
//!   `job_info` (`progress`, `time` left, `filename`, `started`, `finished`, `cancelled`,
//!   `failed`), `file_progress` (a file being fetched), `mesh_data` (Klipper's `bed_mesh`),
//!   `machine_data` (its computer), `firmware`, `printer_error`, `gcode_executed` (a command
//!   done), `term_update` (what Klipper said), `ping`.
//! - **From here:** `connected` (with how often to report), `pong`, and `demand`s: `gcode`,
//!   `terminal`, `pause`, `resume`, `cancel`, `file` (a URL it fetches and prints).
//!
//! Never sent from here: `new_token` and `complete_setup`. Moonraker answers those by
//! switching its address back to SimplyPrint's own servers.

use crate::Shared;
use crate::store::CommandState;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use maker_connect_protocol::{
    self as protocol, AgentInfo, CommandDone, JobEnd, JobProgress, MachineState, Mesh, Telemetry,
};
use serde_json::{Value, json};
use std::time::Duration;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/simplyprint/{token}", get(connect))
        // Moonraker downloads a job's file with a plain GET: the printer's token is in the address.
        .route("/simplyprint/{token}/jobs/{job}/file", get(job_file))
        // Moonraker puts a reconnect token after the address when it was given one: never
        // from here, but taken all the same.
        .route("/simplyprint/{token}/{*rest}", get(connect_more))
}

async fn job_file(
    State(store): State<Shared>,
    Path((token, job)): Path<(String, i64)>,
) -> Response {
    crate::files::job_file(&store, &token, job).await
}

async fn connect_more(
    ws: WebSocketUpgrade,
    state: State<Shared>,
    Path((token, _)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    connect(ws, state, Path(token), headers).await
}

async fn connect(
    ws: WebSocketUpgrade,
    State(store): State<Shared>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    let name = {
        let store = store.lock().unwrap();
        match store.moonraker_name(&token) {
            Ok(Some(name)) => name,
            Ok(None) => return (StatusCode::UNAUTHORIZED, "unknown printer").into_response(),
            Err(error) => return crate::failed(error),
        }
    };
    // Files are fetched from the address the printer reached this cloud by.
    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("localhost:8460")
        .to_owned();
    ws.on_upgrade(move |socket| session(socket, store, token, name, format!("http://{host}")))
}

/// What is known of a printer during its connection, sent on as reports.
#[derive(Default)]
struct Printer {
    telemetry: Telemetry,
    /// The job given to it, from the file demand until it ends.
    job: Option<i64>,
    /// The command it carries out now, and what Klipper said meanwhile.
    command: Option<u32>,
    output: Vec<String>,
    /// News for the next report: commands done, a mesh, a job's end.
    done: Vec<CommandDone>,
    mesh: Option<Mesh>,
    job_end: Option<JobEnd>,
}

async fn session(mut socket: WebSocket, store: Shared, token: String, name: String, base: String) {
    println!("Moonraker connected: {name}");
    // How often it reports (in ms): every 2 s at least, so it counts as online.
    let hello = json!({"type": "connected", "data": {
        "name": name,
        "interval": {"job": 2000, "temps": 2000, "temps_target": 1000, "cpu": 30000, "ping": 2000},
    }});
    let terminal = demand("terminal", json!({"enabled": true}));
    if socket.send(text(&hello)).await.is_err() || socket.send(text(&terminal)).await.is_err() {
        return;
    }
    let mut printer = Printer {
        telemetry: Telemetry {
            state: MachineState::Disconnected,
            firmware: Some("Klipper".to_owned()),
            ..Telemetry::default()
        },
        ..Printer::default()
    };
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            message = socket.recv() => {
                let Some(Ok(message)) = message else { break };
                let Message::Text(body) = message else { continue };
                let Ok(packet) = serde_json::from_str::<Value>(body.as_str()) else { continue };
                let kind = packet["type"].as_str().unwrap_or("");
                if kind == "ping" && socket.send(text(&json!({"type": "pong"}))).await.is_err() {
                    break;
                }
                heard(&mut printer, kind, &packet["data"]);
            }
            _ = tick.tick() => {}
        }
        // Report, and what the cloud has for it: a command, a job, pause or stop.
        let out = report(&store, &token, &base, &mut printer);
        for message in out {
            if socket.send(text(&message)).await.is_err() {
                break;
            }
        }
    }
    println!("Moonraker gone: {name}");
}

fn text(value: &Value) -> Message {
    Message::Text(value.to_string().into())
}

fn demand(what: &str, mut data: Value) -> Value {
    data["demand"] = json!(what);
    json!({"type": "demand", "data": data})
}

/// A message from the printer, into what is known of it.
fn heard(printer: &mut Printer, kind: &str, data: &Value) {
    let t = &mut printer.telemetry;
    match kind {
        "temps" => {
            for (key, pair) in data.as_object().into_iter().flatten() {
                let now = pair.get(0).and_then(Value::as_f64).map(|v| v as f32);
                let target = pair.get(1).and_then(Value::as_f64).map(|v| v as f32);
                let (current, aim) = match key.as_str() {
                    "tool0" => (&mut t.nozzle_c, &mut t.nozzle_target_c),
                    "bed" => (&mut t.bed_c, &mut t.bed_target_c),
                    _ => continue,
                };
                if now.is_some() {
                    *current = now;
                }
                if target.is_some() {
                    *aim = target;
                }
            }
        }
        "state_change" => {
            t.state = state(data["new"].as_str().unwrap_or(""));
            if t.state != MachineState::Error {
                t.problem = None;
            }
        }
        "printer_error" => {
            t.problem = data["error"].as_str().map(str::to_owned);
        }
        "connection" if data["new"] == "disconnected" => {
            t.state = MachineState::Disconnected;
        }
        "firmware" => {
            let fw = &data["fw"];
            if let Some(version) = fw["firmware_version"].as_str() {
                t.firmware = Some(format!(
                    "{} {version}",
                    fw["firmware"].as_str().unwrap_or("Klipper")
                ));
            }
        }
        "machine_data" => {
            t.agent = Some(AgentInfo {
                version: format!("Moonraker {}", data["api_version"].as_str().unwrap_or("?")),
                build: None,
                built: None,
                os: data["os"].as_str().unwrap_or("linux").to_owned(),
                arch: data["machine"].as_str().unwrap_or("").to_owned(),
                hostname: data["hostname"].as_str().map(str::to_owned),
                address: data["local_ip"].as_str().map(str::to_owned),
            });
        }
        "mesh_data" => {
            // Klipper's bed_mesh: probed_matrix, its first row at the front, as ours.
            let matrix = data["probed_matrix"].as_array().filter(|m| !m.is_empty());
            printer.mesh = matrix.map(|rows| Mesh {
                rows: rows
                    .iter()
                    .map(|row| {
                        row.as_array()
                            .into_iter()
                            .flatten()
                            .map(|v| v.as_f64().map(|v| v as f32))
                            .collect()
                    })
                    .collect(),
            });
        }
        "job_info" => {
            let job = t.job.get_or_insert_with(|| JobProgress {
                file: String::new(),
                done: 0,
                total: 100,
                elapsed_s: None,
                remaining_s: None,
            });
            if let Some(file) = data["filename"].as_str() {
                job.file = file.to_owned();
            }
            if let Some(progress) = data["progress"].as_u64() {
                job.done = progress;
            }
            if let Some(left) = data["time"].as_f64() {
                job.remaining_s = Some(left as u64);
            }
            let ended = if data["finished"] == true {
                Some((true, false, None))
            } else if data["cancelled"] == true {
                Some((false, true, None))
            } else if data["failed"] == true {
                Some((false, false, data["error"].as_str().map(str::to_owned)))
            } else {
                None
            };
            if let Some((ok, stopped, reason)) = ended {
                t.job = None;
                end_job(printer, ok, stopped, reason);
            }
        }
        "file_progress" if data["state"] == "error" => {
            let reason = data["message"].as_str().unwrap_or("the file did not come");
            end_job(printer, false, false, Some(reason.to_owned()));
        }
        "term_update" => {
            for line in data["response"].as_array().into_iter().flatten() {
                if let Some(line) = line.as_str() {
                    printer.output.push(line.to_owned());
                }
            }
        }
        "gcode_executed" => {
            let id = data["identifier"].as_str().and_then(|i| i.parse().ok());
            if id.is_some() && id == printer.command {
                printer.command = None;
                printer.done.push(CommandDone {
                    id: id.unwrap_or_default(),
                    ok: data["success"] == true,
                    reason: data["message"].as_str().map(str::to_owned),
                    output: std::mem::take(&mut printer.output),
                });
            }
        }
        _ => {}
    }
}

fn end_job(printer: &mut Printer, ok: bool, stopped: bool, reason: Option<String>) {
    if let Some(id) = printer.job.take() {
        printer.job_end = Some(JobEnd {
            id,
            ok,
            stopped,
            reason,
            restart_line: None,
        });
    }
}

/// SimplyPrint's states as ours.
fn state(new: &str) -> MachineState {
    match new {
        "operational" => MachineState::Idle,
        "printing" | "resuming" | "cancelling" | "pausing" => MachineState::Busy,
        "paused" => MachineState::Paused,
        "error" => MachineState::Error,
        _ => MachineState::Disconnected,
    }
}

/// Reports the printer, and gives back what to send it: the next command, a job's file,
/// pause, resume or cancel.
fn report(store: &Shared, token: &str, base: &str, printer: &mut Printer) -> Vec<Value> {
    let mut report = printer.telemetry.clone();
    report.job_id = printer.job;
    report.done = std::mem::take(&mut printer.done);
    report.mesh = printer.mesh.take();
    report.job_end = printer.job_end.take();
    let mut store = store.lock().unwrap();
    for done in &report.done {
        let state = if done.ok {
            CommandState::Finished
        } else {
            CommandState::Rejected
        };
        let _ = store.command_went(
            token,
            done.id,
            state,
            done.reason.clone(),
            done.output.clone(),
        );
    }
    if !matches!(store.report(token, &report), Ok(Some(Some(_)))) {
        return Vec::new();
    }
    let mut out = Vec::new();
    // One command at a time, and none while it prints.
    if printer.command.is_none()
        && printer.job.is_none()
        && report.state == MachineState::Idle
        && let Ok(Some((id, gcode))) = store.next_command(token)
    {
        printer.command = Some(id);
        printer.output.clear();
        let lines: Vec<&str> = gcode.lines().collect();
        out.push(demand(
            "gcode",
            json!({"list": lines, "identifier": id.to_string()}),
        ));
    }
    match crate::agent_job(&mut store, token, &report) {
        Ok((Some(order), _)) => {
            printer.job = Some(order.id);
            out.push(demand(
                "file",
                json!({"url": format!("{base}/simplyprint/{token}/jobs/{}/file", order.id), "auto_start": 1, "job_id": order.id}),
            ));
        }
        Ok((None, Some(control))) => out.push(demand(
            match control {
                protocol::JobControl::Pause => "pause",
                protocol::JobControl::Resume => "resume",
                protocol::JobControl::Stop => "cancel",
            },
            json!({}),
        )),
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use futures_util::{SinkExt, StreamExt};
    use std::sync::{Arc, Mutex};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    /// A stand-in for Moonraker: connects as its SimplyPrint component does, says what it
    /// would, and does what it is asked.
    #[tokio::test]
    async fn a_klipper_printer_connects_takes_a_command_and_prints() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let (space, id, token) = {
            let mut s = store.lock().unwrap();
            let space = s.default_space().unwrap().0;
            let id = s.add_moonraker(space, "Voron").unwrap();
            let token = s.moonraker_token(space, &id).unwrap().unwrap();
            (space, id, token)
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Only what the printer does, so no one is signed in.
        let app = crate::app(store.clone());
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });

        // An unknown printer is refused.
        assert!(
            tokio_tungstenite::connect_async(format!("ws://{addr}/simplyprint/nobody"))
                .await
                .is_err()
        );

        let (mut ws, _) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/simplyprint/{token}"))
                .await
                .unwrap();
        // The next message of a type, skipping others.
        async fn next(
            ws: &mut (
                     impl StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
                     + Unpin
                 ),
            want: &str,
        ) -> Value {
            loop {
                let message = tokio::time::timeout(Duration::from_secs(5), ws.next())
                    .await
                    .expect("in time")
                    .unwrap()
                    .unwrap();
                if let WsMessage::Text(body) = message {
                    let v: Value = serde_json::from_str(body.as_str()).unwrap();
                    let kind = if v["type"] == "demand" {
                        v["data"]["demand"].as_str().unwrap_or("").to_owned()
                    } else {
                        v["type"].as_str().unwrap_or("").to_owned()
                    };
                    if kind == want {
                        return v;
                    }
                }
            }
        }
        let say = |kind: &str, data: Value| {
            WsMessage::Text(json!({"type": kind, "data": data}).to_string().into())
        };
        let hello = next(&mut ws, "connected").await;
        assert_eq!(hello["data"]["interval"]["ping"], 2000);
        assert_eq!(next(&mut ws, "terminal").await["data"]["enabled"], true);

        ws.send(say("state_change", json!({"new": "operational"})))
            .await
            .unwrap();
        ws.send(say("temps", json!({"tool0": [24, 0], "bed": [23, 0]})))
            .await
            .unwrap();
        ws.send(say(
            "firmware",
            json!({"fw": {"firmware": "Klipper", "firmware_version": "v0.13.0"}}),
        ))
        .await
        .unwrap();
        ws.send(say("ping", Value::Null)).await.unwrap();
        next(&mut ws, "pong").await;
        let machine = store.lock().unwrap().machine(space, &id).unwrap().unwrap();
        assert!(machine.online);
        let t = machine.telemetry.unwrap();
        assert_eq!(
            (t.state, t.nozzle_c, t.firmware.as_deref()),
            (MachineState::Idle, Some(24.0), Some("Klipper v0.13.0"))
        );

        // A command: sent as a gcode demand, done when Moonraker says so.
        let command = store
            .lock()
            .unwrap()
            .queue_command(space, &id, "Home", "G28")
            .unwrap()
            .unwrap();
        let asked = next(&mut ws, "gcode").await;
        assert_eq!(asked["data"]["list"], json!(["G28"]));
        assert_eq!(asked["data"]["identifier"], command.to_string());
        ws.send(say("term_update", json!({"response": ["// homed"]})))
            .await
            .unwrap();
        ws.send(say(
            "gcode_executed",
            json!({"identifier": command.to_string(), "success": true}),
        ))
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let machine = store.lock().unwrap().machine(space, &id).unwrap().unwrap();
        let done = machine.commands.iter().find(|c| c.id == command).unwrap();
        assert_eq!(done.state, CommandState::Finished);
        assert_eq!(done.output, vec!["// homed".to_owned()]);

        // A job: the file's address to fetch, then started, finished, and on the bed.
        let job = {
            let mut s = store.lock().unwrap();
            s.add_file(
                space,
                "f1",
                "cube.gcode",
                10,
                &crate::gcode::FileMeta::default(),
            )
            .unwrap();
            let file = s.file(space, "f1").unwrap().unwrap();
            s.add_job(space, &id, &file, None).unwrap().unwrap()
        };
        std::fs::write(store.lock().unwrap().file_path("f1"), "G28\nG1 X1\n").unwrap();
        let file = next(&mut ws, "file").await;
        let url = file["data"]["url"].as_str().unwrap().to_owned();
        assert_eq!(
            url,
            format!("http://{addr}/simplyprint/{token}/jobs/{job}/file")
        );
        // Moonraker fetches it with a plain GET: the token in the address is enough, and
        // only that printer's token.
        let (got, other) = tokio::task::spawn_blocking(move || {
            let got = ureq::get(&url).call().unwrap().into_string().unwrap();
            let other = ureq::get(&url.replace("/simplyprint/", "/simplyprint/x")).call();
            (got, other)
        })
        .await
        .unwrap();
        assert_eq!(got, "G28\nG1 X1\n", "10 bytes, as added");
        assert!(matches!(other, Err(ureq::Error::Status(404, _))));
        assert_eq!(file["data"]["job_id"], job);
        ws.send(say("state_change", json!({"new": "printing"})))
            .await
            .unwrap();
        ws.send(say(
            "job_info",
            json!({"started": true, "filename": "cube.gcode", "job_id": job}),
        ))
        .await
        .unwrap();
        ws.send(say("job_info", json!({"progress": 50, "job_id": job})))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let machine = store.lock().unwrap().machine(space, &id).unwrap().unwrap();
        assert_eq!(
            machine.job.as_ref().map(|j| j.state),
            Some(crate::store::JobState::Printing)
        );
        assert_eq!(
            machine.telemetry.and_then(|t| t.job).map(|j| j.done),
            Some(50)
        );
        ws.send(say("job_info", json!({"finished": true, "job_id": job})))
            .await
            .unwrap();
        ws.send(say("state_change", json!({"new": "operational"})))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let machine = store.lock().unwrap().machine(space, &id).unwrap().unwrap();
        assert!(machine.job.is_none());
        assert_eq!(machine.on_bed.map(|b| b.job), Some(job));
    }

    #[test]
    fn what_moonraker_says_becomes_a_report() {
        let mut p = Printer::default();
        heard(&mut p, "state_change", &json!({"new": "operational"}));
        heard(
            &mut p,
            "temps",
            &json!({"tool0": [214, 215], "bed": [59, 60], "chamber": [30]}),
        );
        heard(&mut p, "temps", &json!({"tool0": [215]}));
        let t = &p.telemetry;
        assert_eq!(t.state, MachineState::Idle);
        assert_eq!((t.nozzle_c, t.nozzle_target_c), (Some(215.0), Some(215.0)));
        assert_eq!((t.bed_c, t.bed_target_c), (Some(59.0), Some(60.0)));
        heard(
            &mut p,
            "firmware",
            &json!({"fw": {"firmware": "Klipper", "firmware_version": "v0.13.0-1"}}),
        );
        assert_eq!(p.telemetry.firmware.as_deref(), Some("Klipper v0.13.0-1"));
        heard(
            &mut p,
            "mesh_data",
            &json!({"probed_matrix": [[0.1, 0.2], [-0.05, 0.0]]}),
        );
        assert_eq!(p.mesh.as_ref().map(|m| m.rows[1][0]), Some(Some(-0.05)));

        // A job: given, started, progressing, finished.
        p.job = Some(7);
        heard(
            &mut p,
            "job_info",
            &json!({"started": true, "filename": "cube.gcode"}),
        );
        heard(&mut p, "job_info", &json!({"progress": 42, "time": 600}));
        let job = p.telemetry.job.clone().unwrap();
        assert_eq!(
            (job.file.as_str(), job.done, job.remaining_s),
            ("cube.gcode", 42, Some(600))
        );
        heard(&mut p, "job_info", &json!({"finished": true}));
        assert_eq!(p.job, None);
        assert!(p.job_end.as_ref().is_some_and(|e| e.id == 7 && e.ok));

        // A command: what Klipper said, then that it is done.
        p.command = Some(3);
        heard(
            &mut p,
            "term_update",
            &json!({"response": ["// probe at 10,10 is z=0.1"]}),
        );
        heard(
            &mut p,
            "gcode_executed",
            &json!({"identifier": "3", "success": true}),
        );
        assert_eq!(p.done.len(), 1);
        assert_eq!(
            p.done[0].output,
            vec!["// probe at 10,10 is z=0.1".to_owned()]
        );
        assert_eq!(state("paused"), MachineState::Paused);
    }
}
