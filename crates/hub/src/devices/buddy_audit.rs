//! HS15: the proof for `docs/audits/buddy.md` (the hub as Prusa Connect's server against
//! Prusa-Firmware-Buddy 6.3.0, `reference/buddy-6.3.0/src/connect`).
//!
//! Buddy's C++ is not built or run here (it needs the whole firmware); what stands in for it:
//!
//! - `buddy_takes` is a port of `Command::parse_json_command` (`command.cpp:108-433`): the
//!   same events in the same order (the body is read as ordered pairs, not as a map), the same
//!   depth, key and type tests, the same table of command names and expected arguments, the
//!   same 60-token limit of `MAX_TOKENS`, the same 512-byte answer buffer (`connect.cpp:203`).
//!   Without PRUSA_TOOL_MAPPING, enclosure, chamber and nozzle `SET_VALUE` arguments: the hub
//!   sends none of them (audit rows say "not used by the hub").
//! - The events below are written the way `render.cpp` `render_msg` writes them (key order,
//!   nesting, types, taken line by line); they are written by reading the serializer, not
//!   produced by it.
//!
//! Every test cites the reference line it stands for. A row of the audit is "identical" only
//! for what these tests show.

use crate::store::CommandState;
use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use http_body_util::BodyExt;
use serde::de::{MapAccess, Visitor};
use serde_json::{Value, json};
use tower::ServiceExt;

// ---- the port of Buddy's command parser (command.cpp) ------------------------------------

const ARG_JOB_ID: u32 = 1 << 0;
const ARG_PATH: u32 = 1 << 1;
const ARG_TOKEN: u32 = 1 << 2;
const ARG_KEY: u32 = 1 << 3;
const ARG_IV: u32 = 1 << 4;
const ARG_ORIG_SIZE: u32 = 1 << 5;
const ARG_DIALOG_ID: u32 = 1 << 6;
const ARG_RESPONSE: u32 = 1 << 7;
const ARG_SET_VALUE: u32 = 1 << 8;
const ARG_ID: u32 = 1 << 9;
const ARG_TEAM_ID: u32 = 1 << 10;
const ARG_HASH: u32 = 1 << 11;

/// command.cpp:154-192: the name, the type it becomes and the arguments it must have seen.
const TABLE: &[(&str, &str, u32)] = &[
    ("SEND_INFO", "SendInfo", 0),
    ("SEND_JOB_INFO", "SendJobInfo", ARG_JOB_ID),
    ("SEND_FILE_INFO", "SendFileInfo", ARG_PATH),
    ("SEND_TRANSFER_INFO", "SendTransferInfo", 0),
    ("PAUSE_PRINT", "PausePrint", 0),
    ("STOP_PRINT", "StopPrint", 0),
    ("RESUME_PRINT", "ResumePrint", 0),
    ("START_PRINT", "StartPrint", ARG_PATH),
    ("SET_PRINTER_READY", "SetPrinterReady", 0),
    ("CANCEL_PRINTER_READY", "CancelPrinterReady", 0),
    ("SET_IDLE", "SetPrinterIdle", 0),
    ("DELETE_FILE", "DeleteFile", ARG_PATH),
    ("DELETE_FOLDER", "DeleteFolder", ARG_PATH),
    ("CREATE_FOLDER", "CreateFolder", ARG_PATH),
    ("STOP_TRANSFER", "StopTransfer", 0),
    ("SET_TOKEN", "SetToken", ARG_TOKEN),
    ("RESET_PRINTER", "ResetPrinter", 0),
    ("RESET", "ResetPrinter", 0),
    ("SEND_STATE_INFO", "SendStateInfo", 0),
    (
        "DIALOG_ACTION",
        "DialogAction",
        ARG_DIALOG_ID | ARG_RESPONSE,
    ),
    ("SET_VALUE", "SetValue", ARG_SET_VALUE),
    ("CANCEL_OBJECT", "CancelObject", ARG_ID),
    ("UNCANCEL_OBJECT", "UncancelObject", ARG_ID),
    (
        "START_INLINE_DOWNLOAD",
        "StartInlineDownload",
        ARG_PATH | ARG_ORIG_SIZE | ARG_TEAM_ID | ARG_HASH,
    ),
    (
        "START_CONNECT_DOWNLOAD",
        "StartInlineDownload",
        ARG_PATH | ARG_ORIG_SIZE | ARG_TEAM_ID | ARG_HASH,
    ),
    (
        "START_ENCRYPTED_DOWNLOAD",
        "StartEncryptedDownload",
        ARG_PATH | ARG_KEY | ARG_IV | ARG_ORIG_SIZE,
    ),
];

/// general_response.cpp:7-62: the names `from_str` knows (6.3.0), exactly as written there.
pub const RESPONSES: &[&str] = &[
    "Abort",
    "Abort_invalidate_test",
    "Adjust",
    "All",
    "Back",
    "Calibrate",
    "Cancel",
    "Help",
    "Change",
    "Continue",
    "Cooldown",
    "Disable",
    "Done",
    "Filament",
    "Filament_removed",
    "Finish",
    "FS_disable",
    "Ignore",
    "Left",
    "Load",
    "MMU_disable",
    "Never",
    "Next",
    "No",
    "NotNow",
    "Ok",
    "Pause",
    "Print",
    "Purge_more",
    "Quit",
    "Reheat",
    "Replace",
    "Remove",
    "Restart",
    "Resume",
    "Retry",
    "Right",
    "Skip",
    "Slowly",
    "SpoolJoin",
    "Stop",
    "Unload",
    "Yes",
    "Heatup",
    "Postpone5Days",
    "PRINT",
    "Tool1",
    "Tool2",
    "Tool3",
    "Tool4",
    "Tool5",
];

/// What Buddy makes of a command (`CommandData`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Taken {
    /// The type: `SendFileInfo`, `StartPrint`, ... `UnknownCommand` and `BrokenCommand`.
    pub kind: String,
    /// `BrokenCommand`'s reason.
    pub broken: Option<String>,
    pub path: Option<String>,
    pub job_id: Option<u16>,
    pub dialog_id: Option<u32>,
    pub button: Option<String>,
    pub key: Option<[u8; 16]>,
    pub iv: Option<[u8; 16]>,
    pub orig_size: Option<u32>,
    pub port: Option<u16>,
}

impl Taken {
    fn broken(reason: &str) -> Taken {
        Taken {
            kind: "BrokenCommand".into(),
            broken: Some(reason.into()),
            ..Taken::default()
        }
    }
}

/// The pairs of a JSON object in the order they stand in the text (jsmn gives events in that
/// order, and `command.cpp:147` only sees arguments after the command's name).
struct Ordered(Vec<(String, Value)>);

impl<'de> serde::Deserialize<'de> for Ordered {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Ordered;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
                let mut pairs = Vec::new();
                while let Some(pair) = map.next_entry::<String, Value>()? {
                    pairs.push(pair);
                }
                Ok(Ordered(pairs))
            }
        }
        d.deserialize_map(V)
    }
}

/// How many tokens jsmn counts: every object, array, key, string and primitive is one.
fn tokens(value: &Value) -> usize {
    match value {
        Value::Object(map) => 1 + map.values().map(|v| 1 + tokens(v)).sum::<usize>(),
        Value::Array(items) => 1 + items.iter().map(tokens).sum::<usize>(),
        _ => 1,
    }
}

/// `convert_num<uint..>`: the text of a number as a whole number that fits.
fn whole(value: &Value, max: u64) -> Option<u64> {
    value.as_u64().filter(|n| *n <= max)
}

/// `decode_hex` (command.cpp:54-66): exactly 2 * 16 hex digits.
fn hex16(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

/// `Command::parse_json_command` for what the hub can send.
pub fn buddy_takes(body: &str) -> Taken {
    // connect.cpp:203: the answer's body is read into 512 bytes.
    if body.len() > 512 {
        return Taken::broken("body over 512 bytes");
    }
    // command.cpp:43, :118: 60 tokens; more is a jsmn error, "Error parsing JSON" (:425).
    let Ok(Ordered(top)) = serde_json::from_str::<Ordered>(body) else {
        return Taken::broken("Error parsing JSON");
    };
    if 1 + top.iter().map(|(_, v)| 1 + tokens(v)).sum::<usize>() > 60 {
        return Taken::broken("Error parsing JSON");
    }
    let mut data = Taken {
        kind: "UnknownCommand".into(),
        ..Taken::default()
    };
    let mut expected = 0u32;
    let mut seen = 0u32;
    let mut buffer_available = true;
    for (key, value) in &top {
        // command.cpp:147: depth 1, a string, key "command".
        if key == "command"
            && let Some(name) = value.as_str()
        {
            if let Some((_, kind, exp)) = TABLE.iter().find(|(n, _, _)| *n == name) {
                data = Taken {
                    kind: (*kind).into(),
                    ..Taken::default()
                };
                expected = *exp;
            }
            continue;
        }
        // command.cpp:265: depth 1, an object, key "kwargs"; its children are depth 2.
        if key != "kwargs" {
            continue;
        }
        let Some(kwargs) = value.as_object() else {
            continue;
        };
        // Order of the text again (an object read into a map is sorted or not; the pairs are
        // re-read from the body to keep the order).
        let ordered: Vec<(String, Value)> =
            serde_json::from_str::<Ordered>(&serde_json::to_string(value).unwrap_or_default())
                .map(|o| o.0)
                .unwrap_or_default();
        assert_eq!(ordered.len(), kwargs.len());
        for (name, v) in ordered {
            let is_string = v.is_string();
            let primitive = !(v.is_object() || v.is_array() || is_string);
            let kind = data.kind.clone();
            match name.as_str() {
                // :269
                "job_id" if primitive => {
                    if kind == "SendJobInfo"
                        && let Some(n) = whole(&v, u16::MAX as u64)
                    {
                        data.job_id = Some(n as u16);
                        seen |= ARG_JOB_ID;
                    }
                }
                // :271 and :209-216 (PATH_ARG): only the first path takes the buffer.
                "path" | "path_sfn" if is_string => {
                    if matches!(
                        kind.as_str(),
                        "SendFileInfo"
                            | "StartPrint"
                            | "DeleteFile"
                            | "DeleteFolder"
                            | "CreateFolder"
                            | "StartEncryptedDownload"
                            | "StartInlineDownload"
                    ) && buffer_available
                    {
                        data.path = v.as_str().map(str::to_owned);
                        buffer_available = false;
                        seen |= ARG_PATH;
                    }
                }
                // :323
                "button" if is_string => {
                    if kind == "DialogAction" {
                        seen |= ARG_RESPONSE;
                        let name = v.as_str().unwrap_or("");
                        if RESPONSES.contains(&name) {
                            data.button = Some(name.to_owned());
                        } else {
                            data = Taken::broken("Invalid button");
                        }
                    }
                }
                // :383-:387
                "port" if primitive => {
                    if kind == "StartEncryptedDownload" {
                        data.port = whole(&v, u16::MAX as u64).map(|n| n as u16);
                    }
                }
                "orig_size" if primitive => {
                    if matches!(
                        kind.as_str(),
                        "StartEncryptedDownload" | "StartInlineDownload"
                    ) && let Some(n) = whole(&v, u32::MAX as u64)
                    {
                        data.orig_size = Some(n as u32);
                        seen |= ARG_ORIG_SIZE;
                    }
                }
                // :396-:399 (HEX_ARG)
                "key" if is_string => {
                    if kind == "StartEncryptedDownload"
                        && let Some(k) = hex16(v.as_str().unwrap_or(""))
                    {
                        data.key = Some(k);
                        seen |= ARG_KEY;
                    }
                }
                "iv" if is_string => {
                    if kind == "StartEncryptedDownload"
                        && let Some(k) = hex16(v.as_str().unwrap_or(""))
                    {
                        data.iv = Some(k);
                        seen |= ARG_IV;
                    }
                }
                // :400
                "dialog_id" if primitive => {
                    if kind == "DialogAction"
                        && let Some(n) = whole(&v, u32::MAX as u64)
                    {
                        data.dialog_id = Some(n as u32);
                        seen |= ARG_DIALOG_ID;
                    }
                }
                _ => {}
            }
        }
    }
    // :420
    if expected & !seen != 0 {
        return Taken::broken("Missing or broken parameters");
    }
    data
}

// ---- the harness: a MINI on Prusa Connect, as the existing flow tests set one up -----------

struct Mini {
    app: Router,
    store: crate::Store,
    token: String,
    id: String,
    space: i64,
}

impl Mini {
    async fn new(pool: &sqlx::PgPool) -> Mini {
        let store = crate::Store::for_test(pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (space, id, token) = store
            .add_test_machine("prusa/mini", Some("prusa-connect"))
            .await;
        crate::auth::tester_may_operate(&store, &id).await;
        Mini {
            app,
            store,
            token,
            id,
            space,
        }
    }

    async fn call(&self, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let (status, headers) = (response.status(), response.headers().clone());
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, headers, body.to_vec())
    }

    /// A telemetry report as `render.cpp` writes the short one (state only, as `state` is the
    /// last field it always writes, render.cpp:260), and the answer.
    async fn report(&self, body: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
        self.call(
            Request::post("/p/telemetry")
                .header("Token", &self.token)
                .header("Fingerprint", "abc")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
    }

    async fn event(&self, body: Value) -> StatusCode {
        self.call(
            Request::post("/p/events")
                .header("Token", &self.token)
                .header("Fingerprint", "abc")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .0
    }

    async fn form(&self, path: String, form: &str) -> StatusCode {
        self.call(
            Request::post(path)
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .body(Body::from(form.to_owned()))
                .unwrap(),
        )
        .await
        .0
    }

    /// The next command in the answer to a telemetry report, as `connect.cpp` reads it:
    /// status 200, a `Command-Id` of decimal digits that fits 32 bits, a content type
    /// (`resp_parser.cpp:42-50`), a body Buddy can hold (512 bytes). Returns the id, the
    /// content type and the body.
    async fn gets(&self, state: &str) -> (u32, String, String) {
        let (status, headers, body) = self.report(&format!(r#"{{"state":"{state}"}}"#)).await;
        assert_eq!(status, StatusCode::OK, "a command was expected");
        let id: u32 = headers["command-id"].to_str().unwrap().parse().unwrap();
        let content_type = headers["content-type"].to_str().unwrap().to_owned();
        assert!(body.len() <= 512, "connect.cpp:203");
        (id, content_type, String::from_utf8(body).unwrap())
    }

    /// A JSON command, read as Buddy reads it.
    async fn takes(&self, state: &str) -> (u32, Taken, Value) {
        let (id, content_type, body) = self.gets(state).await;
        assert_eq!(content_type, "application/json");
        (id, buddy_takes(&body), serde_json::from_str(&body).unwrap())
    }

    /// Buddy's FINISHED for it (planner.cpp:679), so that the next one may go.
    async fn finished(&self, id: u32) {
        assert_eq!(
            self.event(json!({"state": "IDLE", "command_id": id, "event": "FINISHED"}))
                .await,
            StatusCode::NO_CONTENT
        );
    }

    async fn command_state(&self, id: u32) -> CommandState {
        let m = self
            .store
            .machine(self.space, &self.id)
            .await
            .unwrap()
            .unwrap();
        m.commands.iter().find(|c| c.id == id).unwrap().state
    }
}

fn taken(kind: &str) -> Taken {
    Taken {
        kind: kind.into(),
        ..Taken::default()
    }
}

// ---- tests ------------------------------------------------------------------------------

/// The port agrees with `command.cpp` on its own cases: what it refuses and why.
#[test]
fn the_port_refuses_what_command_cpp_refuses() {
    // :147 unknown name; :424 broken JSON; :420 missing argument; :269 a string is not a primitive.
    assert_eq!(
        buddy_takes(r#"{"command":"NOPE"}"#),
        taken("UnknownCommand")
    );
    assert_eq!(
        buddy_takes("{").broken.as_deref(),
        Some("Error parsing JSON")
    );
    assert_eq!(
        buddy_takes(r#"{"command":"START_PRINT"}"#)
            .broken
            .as_deref(),
        Some("Missing or broken parameters")
    );
    assert_eq!(
        buddy_takes(r#"{"command":"SEND_JOB_INFO","kwargs":{"job_id":"5"}}"#)
            .broken
            .as_deref(),
        Some("Missing or broken parameters")
    );
    // :147 comes first: arguments before the command's name are not seen.
    assert_eq!(
        buddy_takes(r#"{"kwargs":{"path":"/usb/a.gcode"},"command":"START_PRINT"}"#)
            .broken
            .as_deref(),
        Some("Missing or broken parameters")
    );
    // :323 a name not in general_response.cpp (the case counts: from_str compares exactly).
    assert_eq!(
        buddy_takes(r#"{"command":"DIALOG_ACTION","kwargs":{"dialog_id":1,"button":"ABORT"}}"#)
            .broken
            .as_deref(),
        Some("Invalid button")
    );
    // :269 job_id is a uint16_t.
    assert_eq!(
        buddy_takes(r#"{"command":"SEND_JOB_INFO","kwargs":{"job_id":70000}}"#)
            .broken
            .as_deref(),
        Some("Missing or broken parameters")
    );
    // :43 sixty tokens.
    let long = json!({"command": "SEND_INFO", "kwargs": (0..30).map(|i| (format!("k{i}"), json!(i))).collect::<serde_json::Map<_, _>>()});
    assert_eq!(
        buddy_takes(&long.to_string()).broken.as_deref(),
        Some("Error parsing JSON")
    );
    // a good one
    let ok = buddy_takes(r#"{"command":"START_PRINT","kwargs":{"path":"/usb/A.GCO"}}"#);
    assert_eq!(
        (ok.kind.as_str(), ok.path.as_deref()),
        ("StartPrint", Some("/usb/A.GCO"))
    );
}

/// Every command the hub sends as JSON, taken from what the hub really answers a printer's
/// telemetry with (its own routes and store drive it), is read by Buddy as the command it is
/// meant to be, with its arguments: `SEND_FILE_INFO` (command.cpp:156), `START_PRINT` (:161),
/// `DELETE_FILE` (:165), `DELETE_FOLDER` (:166), `CREATE_FOLDER` (:167), `PAUSE_PRINT` (:158),
/// `RESUME_PRINT` (:160), `STOP_PRINT` (:159), `SEND_JOB_INFO` (:155), `DIALOG_ACTION` (:180),
/// `START_ENCRYPTED_DOWNLOAD` (:192).
#[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
#[cfg_attr(
    not(feature = "pg-tests"),
    ignore = "needs Postgres: the pg-tests feature"
)]
async fn buddy_takes_every_command_the_hub_sends(pool: sqlx::PgPool) {
    let mini = Mini::new(&pool).await;
    mini.report(r#"{"state":"IDLE"}"#).await;
    let get = |p: String| Request::get(p).body(Body::empty()).unwrap();
    let id = &mini.id;

    // SEND_FILE_INFO: looking at the stick.
    mini.call(get(format!("/api/machines/{id}/usb?path=/usb")))
        .await;
    let (cid, command, _) = mini.takes("IDLE").await;
    assert_eq!(
        (
            command.kind.as_str(),
            command.path.as_deref(),
            command.broken
        ),
        ("SendFileInfo", Some("/usb"), None)
    );
    // Buddy answers FILE_INFO for the folder (render.cpp:465-497 and :1023-1072).
    assert_eq!(
        mini.event(json!({"data": {"children": [
            {"name": "BENCHY~1.BGC", "display_name": "Benchy.bgcode", "size": 812345, "m_timestamp": 0, "read_only": false, "type": "PRINT_FILE"},
            {"name": "BOX~1.BGC", "display_name": "Box.bgcode", "size": 10, "m_timestamp": 0, "read_only": false, "type": "PRINT_FILE"},
            {"name": "PARTS", "display_name": "parts", "size": 0, "m_timestamp": 0, "read_only": false, "type": "FOLDER"}
        ], "file_count": 3, "read_only": false, "display_name": "usb", "type": "FOLDER", "path": "/usb"},
            "state": "IDLE", "command_id": cid, "event": "FILE_INFO"}))
            .await,
        StatusCode::NO_CONTENT
    );

    // The read-ahead of its printable file (a SEND_FILE_INFO too) goes first; every command
    // in turn is read and answered until the one asked for turns up.
    let sfn = "path=/usb/BENCHY~1.BGC";
    // START_PRINT
    assert_eq!(
        mini.form(format!("/machines/{id}/usb/print"), sfn).await,
        StatusCode::NO_CONTENT
    );
    let mut seen: Vec<Taken> = Vec::new();
    for _ in 0..4 {
        let (cid, command, _) = mini.takes("IDLE").await;
        let done = command.kind == "StartPrint";
        seen.push(command);
        mini.finished(cid).await;
        if done {
            break;
        }
    }
    let start = seen.iter().find(|c| c.kind == "StartPrint").unwrap();
    assert_eq!(start.path.as_deref(), Some("/usb/BENCHY~1.BGC"));
    assert!(seen.iter().all(|c| c.broken.is_none()), "{seen:?}");

    // DELETE_FILE, DELETE_FOLDER, CREATE_FOLDER
    for (route, form, kind, path) in [
        (
            "delete",
            "path=/usb/BOX~1.BGC",
            "DeleteFile",
            "/usb/BOX~1.BGC",
        ),
        (
            "delete",
            "path=/usb/PARTS&folder=1",
            "DeleteFolder",
            "/usb/PARTS",
        ),
        (
            "folder",
            "path=/usb&name=New%20box",
            "CreateFolder",
            "/usb/New box",
        ),
    ] {
        assert_eq!(
            mini.form(format!("/machines/{id}/usb/{route}"), form).await,
            StatusCode::NO_CONTENT,
            "{kind}"
        );
        let (cid, command, _) = mini.takes("IDLE").await;
        assert_eq!(
            (
                command.kind.as_str(),
                command.path.as_deref(),
                command.broken
            ),
            (kind, Some(path), None)
        );
        mini.finished(cid).await;
    }

    // PAUSE_PRINT, RESUME_PRINT, STOP_PRINT
    for (action, kind) in [
        ("pause", "PausePrint"),
        ("resume", "ResumePrint"),
        ("stop", "StopPrint"),
    ] {
        crate::devices::printer_files::job_control(
            &mut mini.store.lock().unwrap(),
            mini.space,
            id,
            action,
        )
        .unwrap();
        let (cid, command, _) = mini.takes("IDLE").await;
        assert_eq!(command, taken(kind));
        mini.finished(cid).await;
    }

    // SEND_JOB_INFO: a print the hub does not know, as telemetry says it (render.cpp:149).
    let printing = r#"{"job_id":29,"time_printing":5,"progress":1,"state":"PRINTING"}"#;
    let mut job_info = None;
    for _ in 0..4 {
        let (status, headers, body) = mini.report(printing).await;
        assert_eq!(status, StatusCode::OK);
        let command = buddy_takes(&String::from_utf8(body).unwrap());
        let cid: u32 = headers["command-id"].to_str().unwrap().parse().unwrap();
        mini.finished(cid).await;
        if command.kind == "SendJobInfo" {
            job_info = Some(command);
            break;
        }
    }
    let command = job_info.expect("SEND_JOB_INFO");
    assert_eq!((command.job_id, command.broken), (Some(29), None));

    // DIALOG_ACTION: Buddy's STATE_CHANGED as render.cpp:597-683 writes it, with its buttons
    // by their names (general_response.cpp), answered from the website.
    assert_eq!(
        mini.event(json!({"data": {"code": "13501", "title": "Filament runout", "text": "Load filament.", "buttons": ["Continue", "Stop"]},
            "dialog_id": 7, "state": "ATTENTION", "event": "STATE_CHANGED"}))
            .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        mini.form(format!("/machines/{id}/answer"), "button=1")
            .await,
        StatusCode::NO_CONTENT
    );
    let (cid, command, _) = mini.takes("ATTENTION").await;
    assert_eq!(
        (
            command.kind.as_str(),
            command.dialog_id,
            command.button.as_deref(),
            command.broken
        ),
        ("DialogAction", Some(7), Some("Stop"), None)
    );
    mini.finished(cid).await;

    // START_ENCRYPTED_DOWNLOAD: a file of the space sent to the stick.
    let gcode = b"G1 X1\n".repeat(100);
    let (status, _, body) = mini
        .call(
            Request::post("/api/files?name=Part.gcode")
                .body(Body::from(gcode.clone()))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let file = serde_json::from_slice::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        mini.form(format!("/files/{file}/print"), &format!("machine={id}"))
            .await,
        StatusCode::NO_CONTENT
    );
    let (cid, command, raw) = mini.takes("IDLE").await;
    assert_eq!(command.kind, "StartEncryptedDownload", "{raw}");
    assert_eq!(command.broken, None);
    assert_eq!(command.path.as_deref(), Some("/usb/Part.gcode"));
    assert_eq!(command.orig_size, Some(gcode.len() as u32));
    assert!(command.key.is_some() && command.iv.is_some());
    // no `port`: Buddy uses the host and port it reports to (planner.cpp:169-175).
    assert_eq!(command.port, None);
    mini.finished(cid).await;
}

/// `SET_PRINTER_READY` is sent by `farm.rs` as the literal `{"command": "SET_PRINTER_READY"}`
/// (a route that needs a whole job to reach); the literal in the source is the body, and
/// Buddy reads it (command.cpp:162).
#[test]
fn buddy_takes_set_printer_ready() {
    let farm = include_str!("../farm.rs");
    assert!(farm.contains(r#"&json!({"command": "SET_PRINTER_READY"}),"#));
    assert_eq!(
        buddy_takes(&json!({"command": "SET_PRINTER_READY"}).to_string()),
        taken("SetPrinterReady")
    );
}

/// Every name Buddy 6.3.0 lists for a dialog's answer (general_response.cpp:7-62), as its
/// STATE_CHANGED lists them (render.cpp:654-667: `to_str`), goes back unchanged in a
/// `DIALOG_ACTION` and is taken by Buddy (command.cpp:323-330).
#[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
#[cfg_attr(
    not(feature = "pg-tests"),
    ignore = "needs Postgres: the pg-tests feature"
)]
async fn every_dialog_answer_of_6_3_0_goes_back_unchanged(pool: sqlx::PgPool) {
    let mini = Mini::new(&pool).await;
    mini.report(r#"{"state":"IDLE"}"#).await;
    // render.cpp:654: MAX_RESPONSES per dialog; the names are sent in as many dialogs as needed.
    for (n, name) in RESPONSES.iter().enumerate() {
        assert_eq!(
            mini.event(
                json!({"data": {"title": "t", "text": "x", "buttons": [name]},
                "dialog_id": 100 + n, "state": "ATTENTION", "event": "STATE_CHANGED"})
            )
            .await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            mini.form(format!("/machines/{}/answer", mini.id), "button=0")
                .await,
            StatusCode::NO_CONTENT
        );
        let (cid, command, _) = mini.takes("ATTENTION").await;
        assert_eq!(
            (command.button.as_deref(), command.dialog_id, command.broken),
            (Some(*name), Some(100 + n as u32), None),
            "{name}"
        );
        mini.finished(cid).await;
    }
}

/// A STATE_CHANGED that names no `dialog_id` (render.cpp:673 writes it only when the state has
/// a dialog): Buddy refuses a DIALOG_ACTION without one (command.cpp:86, :420), so the hub
/// offers no buttons and refuses a press, sending nothing.
#[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
#[cfg_attr(
    not(feature = "pg-tests"),
    ignore = "needs Postgres: the pg-tests feature"
)]
async fn a_dialog_answer_without_a_dialog_is_refused_by_the_hub(pool: sqlx::PgPool) {
    let mini = Mini::new(&pool).await;
    mini.report(r#"{"state":"IDLE"}"#).await;
    mini.event(
        json!({"data": {"title": "t", "text": "x", "buttons": ["Continue"]},
        "state": "ATTENTION", "event": "STATE_CHANGED"}),
    )
    .await;
    let buttons = mini
        .store
        .machine(mini.space, &mini.id)
        .await
        .unwrap()
        .unwrap()
        .buttons;
    assert!(buttons.is_empty(), "no dialog named: no buttons offered");
    let (status, _, body) = mini
        .call(
            Request::post(format!("/machines/{}/answer", mini.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .body(Body::from("button=0"))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        String::from_utf8_lossy(&body),
        "the printer named no dialog"
    );
    assert_eq!(
        mini.report(r#"{"state":"ATTENTION"}"#).await.0,
        StatusCode::NO_CONTENT,
        "nothing was queued"
    );
}

/// The events Buddy 6.3.0 sends, as `render.cpp` writes each, and what the hub does with a
/// command's state: planner.cpp:253-280 names the 14 types.
#[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
#[cfg_attr(
    not(feature = "pg-tests"),
    ignore = "needs Postgres: the pg-tests feature"
)]
async fn the_events_buddy_sends_settle_commands(pool: sqlx::PgPool) {
    let mini = Mini::new(&pool).await;
    mini.report(r#"{"state":"IDLE"}"#).await;
    // What each event settles for a command with its id at the top (render.cpp:677).
    let cases: &[(&str, CommandState, Value)] = &[
        // :269 Accepted: no job_id, no data (has_extra false).
        ("ACCEPTED", CommandState::Accepted, json!({})),
        // :677 Finished/Failed/Rejected: job_id (has_job), reason (:327), machine_reason (:331).
        ("FINISHED", CommandState::Finished, json!({"job_id": 29})),
        ("FAILED", CommandState::Rejected, json!({"job_id": 29})),
        (
            "REJECTED",
            CommandState::Rejected,
            json!({"reason": "File already exists", "machine_reason": "FILE_EXISTS"}),
        ),
        // :339-436 Info
        (
            "INFO",
            CommandState::Finished,
            json!({"data": {"firmware": "6.3.0+0.LOCAL", "printer_type": "2.1.0", "sn": "CZPX1234X017XC12345", "appendix": false,
                "fingerprint": "f", "nozzle_diameter": 0.4, "transfer_paused": false,
                "storages": [{"mountpoint": "/usb", "type": "USB", "read_only": false, "free_space": 1000, "is_sfn": true}],
                "network_info": {"lan_mac": "AA:BB:CC:DD:EE:FF", "lan_ipv4": "192.168.1.5", "hostname": "mini"},
                "tools": {"1": {"nozzle_diameter": 0.4, "high_flow": false, "hardened": false, "material": "PLA"}}, "slots": 1}}),
        ),
        // :437-464 JobInfo
        (
            "JOB_INFO",
            CommandState::Finished,
            json!({"job_id": 29, "data": {"state": "PRINTING", "size": 812345, "m_timestamp": 1, "display_name": "Benchy.bgcode", "start_cmd_id": 3, "path": "/usb/BENCHY~1.BGC"}}),
        ),
        // :465-497 FileInfo
        (
            "FILE_INFO",
            CommandState::Finished,
            json!({"data": {"size": 5, "m_timestamp": 1, "read_only": false, "display_name": "a.gcode", "type": "PRINT_FILE", "path": "/usb/A.GCO"}}),
        ),
        // :498-536 TransferInfo
        (
            "TRANSFER_INFO",
            CommandState::Finished,
            json!({"data": {"size": 100, "transferred": 0, "progress": 0.0, "time_remaining": 0, "time_transferring": 0, "path": "/usb/A.GCO", "start_cmd_id": 3, "type": "FROM_CONNECT"}}),
        ),
        // :543-567 FileChanged
        (
            "FILE_CHANGED",
            CommandState::Finished,
            json!({"data": {"free_space": 1, "new_path": "/usb/NEW", "file": {"type": "FOLDER", "name": "NEW"}}}),
        ),
        // :568-596 CancelableChanged
        (
            "CANCELABLE_CHANGED",
            CommandState::Finished,
            json!({"data": {"objects": [{"name": "a", "canceled": false, "id": 0}]}}),
        ),
        // :597-669 StateChanged
        (
            "STATE_CHANGED",
            CommandState::Finished,
            json!({"data": {"title": "t", "text": "x"}}),
        ),
    ];
    for (name, settled, extra) in cases {
        // a command to settle
        let id = mini
            .store
            .lock()
            .unwrap()
            .queue_connect_command(
                mini.space,
                &mini.id,
                "x",
                &json!({"command": "SEND_INFO"}),
                false,
            )
            .unwrap()
            .unwrap();
        let (status, headers, _) = mini.report(r#"{"state":"IDLE"}"#).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["command-id"].to_str().unwrap(), id.to_string());
        // The event as render.cpp:671-682 closes it: [dialog_id,] state, command_id, event.
        let mut event = extra.as_object().unwrap().clone();
        event.insert("state".into(), json!("IDLE"));
        event.insert("command_id".into(), json!(id));
        event.insert("event".into(), json!(name));
        assert_eq!(
            mini.event(Value::Object(event)).await,
            StatusCode::NO_CONTENT,
            "{name}"
        );
        assert_eq!(mini.command_state(id).await, *settled, "{name}");
        // free the line for the next
        mini.finished(id).await;
    }
}

/// TRANSFER_STOPPED, TRANSFER_ABORTED and TRANSFER_FINISHED carry no `command_id` at the top
/// (planner.cpp:466-472 builds them with a transfer id and `start_cmd_id`, render.cpp:537-542
/// puts it in `data`, :680 `transfer_id`): the hub settles the download command by
/// `data.start_cmd_id`, and takes the others (no transfer of its own) without a change.
#[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
#[cfg_attr(
    not(feature = "pg-tests"),
    ignore = "needs Postgres: the pg-tests feature"
)]
async fn transfer_events_name_their_command_in_data(pool: sqlx::PgPool) {
    let mini = Mini::new(&pool).await;
    mini.report(r#"{"state":"IDLE"}"#).await;
    let gcode = b"G1 X1\n".repeat(100);
    let (_, _, body) = mini
        .call(
            Request::post("/api/files?name=Part.gcode")
                .body(Body::from(gcode))
                .unwrap(),
        )
        .await;
    let file = serde_json::from_slice::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // A transfer of the hub's own (file sent, no print): the download command goes.
    assert_eq!(
        mini.form(
            format!("/machines/{}/usb/send", mini.id),
            &format!("file={file}")
        )
        .await,
        StatusCode::OK
    );
    let (cid, command, _) = mini.takes("IDLE").await;
    assert_eq!(command.kind, "StartEncryptedDownload");
    // planner.cpp:787 answers it with TRANSFER_INFO (render.cpp:498).
    assert_eq!(
        mini.event(json!({"data": {"size": 600, "transferred": 0, "progress": 0.0, "time_remaining": 0, "time_transferring": 0, "path": "/usb/PART~1.GCO", "start_cmd_id": cid, "type": "FROM_CONNECT"},
            "state": "BUSY", "command_id": cid, "transfer_id": 5, "event": "TRANSFER_INFO"}))
            .await,
        StatusCode::NO_CONTENT
    );
    // Finished: render.cpp:537-542, :677-680.
    assert_eq!(
        mini.event(json!({"data": {"start_cmd_id": cid}, "state": "IDLE", "transfer_id": 5, "event": "TRANSFER_FINISHED"}))
            .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(mini.command_state(cid).await, CommandState::Finished);
    let transfer_state: String = sqlx::query_scalar("SELECT state FROM printer_transfers")
        .fetch_one(&mini.store.pg().admin)
        .await
        .unwrap();
    assert_eq!(transfer_state, "finished");
}

/// The answer to a telemetry report, as `connect.cpp` (`send_command`, :724-790) and
/// `resp_parser.cpp` read it: 204 for nothing, 200 with a `Command-Id` and `application/json`
/// or `text/x.gcode` for a command; an unknown printer gets 401 (the printer's "Auth" error,
/// :782-783).
#[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
#[cfg_attr(
    not(feature = "pg-tests"),
    ignore = "needs Postgres: the pg-tests feature"
)]
async fn the_answer_to_a_report_is_what_buddy_reads(pool: sqlx::PgPool) {
    let mini = Mini::new(&pool).await;
    let (status, headers, body) = mini.report(r#"{"state":"IDLE"}"#).await;
    assert_eq!(status, StatusCode::NO_CONTENT); // :726
    assert!(body.is_empty() && headers.get("command-id").is_none());
    // G-code: one command for the background (planner.cpp:673 Gcode), text/x.gcode (resp_parser :45).
    let id = mini
        .store
        .lock()
        .unwrap()
        .queue_command(mini.space, &mini.id, "Home", "G28")
        .unwrap()
        .unwrap();
    let (cid, content_type, body) = mini.gets("IDLE").await;
    assert_eq!(
        (cid, content_type.as_str(), body.as_str()),
        (id, "text/x.gcode", "G28")
    );
    // :90-96: a G-code body over the 512-byte buffer is refused by the printer ("GCode too large").
    // One command at a time (planner.cpp:661-663, REJECTED "Processing other command").
    assert_eq!(
        mini.report(r#"{"state":"IDLE"}"#).await.0,
        StatusCode::NO_CONTENT
    );
    mini.finished(cid).await;
    // an unknown token
    let (status, _, _) = mini
        .call(
            Request::post("/p/telemetry")
                .header("Token", "nope")
                .body(Body::from(r#"{"state":"IDLE"}"#))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Registration as `registrator.cpp` does it: the body as :61-69 writes it (`sn`,
/// `fingerprint`, `printer_type`, `firmware`, in that order), `200` with a `Code` of at most
/// 25 characters (:131, registrator.hpp:14); the poll (:80-110) with `Code`, `202` until the
/// owner adds it, then `200` with a `Token` of at most 20 characters (printer.hpp:190).
#[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
#[cfg_attr(
    not(feature = "pg-tests"),
    ignore = "needs Postgres: the pg-tests feature"
)]
async fn registration_is_what_registrator_cpp_does(pool: sqlx::PgPool) {
    let store = crate::store::Store::for_test(&pool).await;
    let app = crate::auth::signed_in_app(store.clone()).await;
    let send = |request: Request<Body>| {
        let app = app.clone();
        async move {
            let r = app.oneshot(request).await.unwrap();
            (r.status(), r.headers().clone())
        }
    };
    let (status, headers) = send(
        Request::post("/p/register")
            .header("content-type", "application/json")
            .header("User-Agent-Printer", "Buddy_MINI")
            .header("User-Agent-Version", "6.3.0+0.LOCAL")
            .body(Body::from(
                r#"{"sn":"CZPX1234X017XC12345","fingerprint":"0123456789abcdef","printer_type":"2.1.0","firmware":"6.3.0+0.LOCAL"}"#,
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let code = headers["code"].to_str().unwrap().to_owned();
    assert!(!code.is_empty() && code.len() <= 25);
    let ask = || {
        Request::get("/p/register")
            .header("Code", &code)
            .header("User-Agent-Printer", "Buddy_MINI")
            .header("User-Agent-Version", "6.3.0+0.LOCAL")
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(send(ask()).await.0, StatusCode::ACCEPTED);
    let space = store.default_space().await.unwrap().0;
    assert!(store.pair(space, &code).await.unwrap().is_some());
    let (status, headers) = send(ask()).await;
    assert_eq!(status, StatusCode::OK);
    let token = headers["token"].to_str().unwrap().to_owned();
    assert!(!token.is_empty() && token.len() <= 20);
    // The answer may be lost: the code gets its token again until the printer's first report
    // with it (registrator.cpp:233), then it is spent.
    let (status, again) = send(ask()).await;
    assert_eq!(
        (status, again["token"].to_str().unwrap()),
        (StatusCode::OK, token.as_str())
    );
    let report = send(
        Request::post("/p/telemetry")
            .header("Token", &token)
            .header("Fingerprint", "0123456789abcdef")
            .body(Body::from(r#"{"state":"IDLE"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(report.0, StatusCode::NO_CONTENT);
    assert_eq!(send(ask()).await.0, StatusCode::NOT_FOUND);
}

/// The INFO event's `tools` (render.cpp:395) says `"material": "---"` for a slot with nothing
/// loaded: the hub reads that as no material.
#[test]
fn info_says_a_dash_for_no_material() {
    let info = json!({"firmware": "6.3.0", "printer_type": "2.1.0", "sn": "X", "nozzle_diameter": 0.4,
        "tools": {"1": {"nozzle_diameter": 0.4, "high_flow": false, "hardened": false, "material": "---"}}});
    let details = super::prusa_connect::to_details(&info);
    assert_eq!(details.material, None);
}

/// D4: `printer_type` is Buddy's string "%hhu.%hhu.%hhu" (render.cpp:342, registrator.cpp:63);
/// a registration that names the type as numbers is not a profile.
#[test]
fn printer_type_is_read_as_buddys_string_only() {
    let info = json!({"printer_type": "2.1.0", "firmware": "6.3.0", "sn": "X"});
    assert_eq!(
        super::prusa_connect::to_details(&info)
            .printer_type
            .as_deref(),
        Some("2.1.0")
    );
    let numbers = json!({"type": 2, "version": 1, "subversion": 0, "firmware": "6.3.0", "sn": "X"});
    assert_eq!(
        super::prusa_connect::to_details(&numbers).printer_type,
        None
    );
    let nested = json!({"printer_type": {"type": 2, "version": 1, "subversion": 0}});
    assert_eq!(super::prusa_connect::to_details(&nested).printer_type, None);
}

/// D3: Buddy 6.3.0 never sends MEDIUM_INSERTED or MEDIUM_EJECTED (the 14 events of
/// planner.cpp:253-280); a stick change comes as FILE_CHANGED with `rescan` (render.cpp:555),
/// which makes the hub read the stick again. The two events do nothing.
#[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
#[cfg_attr(
    not(feature = "pg-tests"),
    ignore = "needs Postgres: the pg-tests feature"
)]
async fn a_stick_change_comes_as_file_changed_with_rescan(pool: sqlx::PgPool) {
    let mini = Mini::new(&pool).await;
    mini.report(r#"{"state":"IDLE"}"#).await;
    let look = || async {
        mini.call(
            Request::get(format!("/api/machines/{}/usb?path=/usb", mini.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    };
    look().await;
    let (cid, command, _) = mini.takes("IDLE").await;
    assert_eq!(command.kind, "SendFileInfo");
    mini.event(json!({"data": {"children": [], "file_count": 0, "read_only": false, "display_name": "usb",
        "type": "FOLDER", "path": "/usb"}, "state": "IDLE", "command_id": cid, "event": "FILE_INFO"}))
        .await;
    // Listed: asking again sends nothing.
    look().await;
    assert_eq!(
        mini.report(r#"{"state":"IDLE"}"#).await.0,
        StatusCode::NO_CONTENT
    );
    for name in ["MEDIUM_INSERTED", "MEDIUM_EJECTED"] {
        mini.event(json!({"state": "IDLE", "event": name})).await;
        look().await;
        assert_eq!(
            mini.report(r#"{"state":"IDLE"}"#).await.0,
            StatusCode::NO_CONTENT,
            "{name}"
        );
    }
    // The stick changed: FILE_CHANGED with rescan (render.cpp:555), the listing is read again.
    mini.event(json!({"data": {"new_path": "/usb", "rescan": true}, "state": "IDLE", "event": "FILE_CHANGED"}))
        .await;
    look().await;
    let (_, command, _) = mini.takes("IDLE").await;
    assert_eq!(command.kind, "SendFileInfo");
}

/// D9: the hub sends a file again under a free name only when REJECTED says `machine_reason`
/// `FILE_EXISTS` (planner.cpp:793, render.cpp:330), not by the words of `reason`.
#[test]
fn a_free_name_is_decided_by_machine_reason() {
    use super::printer_files::file_exists;
    // planner.cpp:793: "File already exists", FILE_EXISTS.
    assert!(file_exists(
        &json!({"event": "REJECTED", "command_id": 3, "reason": "File already exists",
        "machine_reason": "FILE_EXISTS", "state": "IDLE"})
    ));
    // The same words without the machine reason, and the other machine reasons (planner.cpp:287-302).
    assert!(!file_exists(
        &json!({"event": "REJECTED", "reason": "File already exists"})
    ));
    assert!(!file_exists(
        &json!({"event": "REJECTED", "reason": "File not found"})
    ));
    for other in [
        "TRANSFER_IN_PROGRESS",
        "STORAGE_FAILURE",
        "NOT_READY",
        "NETWORK_FAILURE",
    ] {
        assert!(!file_exists(
            &json!({"event": "REJECTED", "reason": "File exists", "machine_reason": other})
        ));
    }
}

/// The states `to_str(DeviceState)` writes (state/printer_state.cpp:442-466), as the telemetry's
/// last field (render.cpp:260): each is read, and none is refused. How the hub shows them
/// (`PRINTING` and `BUSY` as busy, the rest as the hub's own states) is the hub's UI.
#[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
#[cfg_attr(
    not(feature = "pg-tests"),
    ignore = "needs Postgres: the pg-tests feature"
)]
async fn every_state_buddy_sends_is_read(pool: sqlx::PgPool) {
    use maker_connect_protocol::MachineState;
    let mini = Mini::new(&pool).await;
    for (name, shown) in [
        ("IDLE", MachineState::Idle),
        ("PRINTING", MachineState::Busy),
        ("PAUSED", MachineState::Paused),
        ("FINISHED", MachineState::Idle),
        ("STOPPED", MachineState::Idle),
        ("READY", MachineState::Idle),
        ("ERROR", MachineState::Error),
        ("BUSY", MachineState::Busy),
        ("ATTENTION", MachineState::Attention),
        ("UNKNOWN", MachineState::Idle),
    ] {
        let (status, _, _) = mini.report(&format!(r#"{{"state":"{name}"}}"#)).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{name}");
        let m = mini
            .store
            .machine(mini.space, &mini.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(m.telemetry.unwrap().state, shown, "{name}");
    }
}

/// STATE_CHANGED (render.cpp:597-683) and JOB_INFO (:437-464), as written there: what the
/// screen says (`code`, `title`, `text`, `buttons`, the top-level `dialog_id`), and which file
/// a print is (`job_id` at the top, `display_name` in `data`).
#[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
#[cfg_attr(
    not(feature = "pg-tests"),
    ignore = "needs Postgres: the pg-tests feature"
)]
async fn state_changed_and_job_info_reach_the_hub(pool: sqlx::PgPool) {
    let mini = Mini::new(&pool).await;
    mini.report(r#"{"state":"IDLE"}"#).await;
    mini.event(json!({"data": {"code": "13501", "title": "Filament runout", "text": "Load filament.", "buttons": ["Continue", "Stop"]},
        "dialog_id": 7, "state": "ATTENTION", "event": "STATE_CHANGED"}))
        .await;
    {
        let m = mini
            .store
            .machine(mini.space, &mini.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            m.asking,
            Some((
                "Filament runout".to_owned(),
                "Load filament. (#13501, prusa.io/13501)".to_owned()
            ))
        );
        assert_eq!(m.buttons, ["Continue", "Stop"]);
    }
    // Back to a state without a dialog: render.cpp writes no `data` then (only the state).
    mini.event(json!({"state": "PRINTING", "event": "STATE_CHANGED"}))
        .await;
    {
        let m = mini
            .store
            .machine(mini.space, &mini.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((m.asking, m.buttons.len()), (None, 0));
    }
    // JOB_INFO: job_id at the top (:323), the file in data (:454).
    mini.event(json!({"job_id": 29, "data": {"state": "PRINTING", "size": 812345, "m_timestamp": 1, "display_name": "Benchy 0.2mm PLA.bgcode", "start_cmd_id": 3, "path": "/usb/BENCHY~1.BGC"},
        "state": "PRINTING", "command_id": 4, "event": "JOB_INFO"}))
        .await;
    let known = mini
        .store
        .lock()
        .unwrap()
        .printing_file
        .get(&mini.token)
        .cloned();
    assert_eq!(
        known,
        Some((Some(29), "Benchy 0.2mm PLA.bgcode".to_owned()))
    );
}

/// FILE_INFO of a folder (render.cpp:465-497, :1023-1072) fills the hub's listing of the stick;
/// FILE_CHANGED (:543-567) with `old_path` takes a file out of it.
#[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
#[cfg_attr(
    not(feature = "pg-tests"),
    ignore = "needs Postgres: the pg-tests feature"
)]
async fn the_listing_follows_file_info_and_file_changed(pool: sqlx::PgPool) {
    let mini = Mini::new(&pool).await;
    mini.report(r#"{"state":"IDLE"}"#).await;
    let look = || async {
        let (_, _, body) = mini
            .call(
                Request::get(format!("/api/machines/{}/usb?path=/usb", mini.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        serde_json::from_slice::<Value>(&body).unwrap()
    };
    look().await;
    let (cid, command, _) = mini.takes("IDLE").await;
    assert_eq!(command.kind, "SendFileInfo");
    mini.event(json!({"data": {"children": [
            {"name": "BENCHY~1.BGC", "display_name": "Benchy 0.2mm PLA.bgcode", "size": 812345, "m_timestamp": 1730000000, "read_only": false, "type": "PRINT_FILE"},
            {"name": "BOX~1.BGC", "display_name": "Box.bgcode", "size": 10, "m_timestamp": 0, "read_only": true, "type": "PRINT_FILE"}
        ], "file_count": 2, "read_only": false, "display_name": "usb", "type": "FOLDER", "path": "/usb"},
        "state": "IDLE", "command_id": cid, "event": "FILE_INFO"}))
        .await;
    let listed = look().await;
    let children = listed["data"]["children"].as_array().unwrap();
    assert_eq!(children.len(), 2);
    assert_eq!(children[0]["display_name"], "Benchy 0.2mm PLA.bgcode");
    assert_eq!(
        children[1]["read_only"], true,
        "still arriving: read_only (:1067)"
    );
    // A file deleted (old_path, :552).
    mini.event(json!({"data": {"free_space": 5, "old_path": "/usb/BENCHY~1.BGC", "file": {"type": "PRINT_FILE", "name": "BENCHY~1.BGC"}},
        "state": "IDLE", "event": "FILE_CHANGED"}))
        .await;
    let names: Vec<String> = look().await["data"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["BOX~1.BGC"]);
}

/// D8: Buddy has no time limit for a command (none in src/connect; while one runs it rejects the next, planner.cpp:661-663); the hub does not give one up either: it
/// stays pending while Buddy carries it out (its `command_id` in telemetry, render.cpp:251), is
/// settled by its FINISHED, and fails only when the printer itself was gone (not seen for longer
/// than ONLINE_SECONDS, K18 (b)): "the printer was gone" (ours).
#[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
#[cfg_attr(
    not(feature = "pg-tests"),
    ignore = "needs Postgres: the pg-tests feature"
)]
async fn a_command_stays_pending_until_buddy_answers_it(pool: sqlx::PgPool) {
    let mini = Mini::new(&pool).await;
    mini.report(r#"{"state":"IDLE"}"#).await;
    let queue = || {
        mini.store
            .lock()
            .unwrap()
            .queue_connect_command(
                mini.space,
                &mini.id,
                "x",
                &json!({"command": "SEND_INFO"}),
                false,
            )
            .unwrap()
            .unwrap()
    };
    let (first, second) = (queue(), queue());
    let (status, headers, _) = mini.report(r#"{"state":"IDLE"}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["command-id"].to_str().unwrap(), first.to_string());
    // An hour on, still going: the next one waits, the first is not given up.
    mini.store
        .lock()
        .unwrap()
        .set_commands_at(&mini.id, crate::store::now() - 3600);
    assert_eq!(
        mini.report(&format!(r#"{{"command_id":{first},"state":"IDLE"}}"#))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(mini.command_state(first).await, CommandState::Accepted);
    assert_eq!(mini.command_state(second).await, CommandState::Queued);
    // Buddy's answer settles it, and the next one goes.
    mini.finished(first).await;
    assert_eq!(mini.command_state(first).await, CommandState::Finished);
    let (status, headers, _) = mini.report(r#"{"state":"IDLE"}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["command-id"].to_str().unwrap(), second.to_string());
    // The printer was gone (not seen for longer than ONLINE_SECONDS): what it had not answered
    // fails, named as ours; a later report goes on.
    let third = queue();
    mini.store
        .set_last_seen(
            &mini.id,
            crate::store::now() - crate::store::ONLINE_SECONDS - 1,
        )
        .await;
    let (status, headers, _) = mini.report(r#"{"state":"IDLE"}"#).await;
    assert_eq!(mini.command_state(second).await, CommandState::Rejected);
    let reason = {
        let m = mini
            .store
            .machine(mini.space, &mini.id)
            .await
            .unwrap()
            .unwrap();
        m.commands
            .iter()
            .find(|c| c.id == second)
            .unwrap()
            .reason
            .clone()
    };
    assert_eq!(reason.as_deref(), Some("the printer was gone"));
    assert_eq!(
        (status, headers["command-id"].to_str().unwrap()),
        (StatusCode::OK, third.to_string().as_str())
    );
}
