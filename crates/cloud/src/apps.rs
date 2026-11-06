//! For other programs (LaserFoundry, a slicer, a script) to send work to a space: with an
//! API token of the space (Space page), as `Authorization: Bearer mct_…`. The contract is in
//! `docs/apps-api.md`.
//!
//! - `GET /api/apps/v1/info`, without a token: that this is a Maker Connect, and which API.
//! - `GET /api/apps/v1/me`: the space the token is for, its role.
//! - `GET /api/apps/v1/machines[?kind=laser]`: the space's machines with what a program needs
//!   to make work for them (a laser's work area, `$30` and `$32` as its controller said).
//! - `POST /api/apps/v1/files?name=sign.gcode[&machine=<id>]`: a file, as the body. It is
//!   kept in the space's Files, checked against the machine if one is named, and the answer
//!   has the page to open. **Nothing starts**: a laser job starts with someone at the laser,
//!   who frames it first.
//!
//! Browsers may call these from another site (LaserFoundry's web version): the answers allow
//! any origin, and need the token, never a cookie.

use crate::access::{Access, Level};
use crate::{Shared, failed, profiles};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use maker_connect_protocol::MachineKind;
use serde::Deserialize;
use serde_json::{Value, json};

/// This API's version: a program can tell what it may expect.
pub const API: u32 = 1;
pub const PREFIX: &str = "/api/apps/v1/";

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/apps/v1/info", get(info))
        .route("/api/apps/v1/me", get(me))
        .route("/api/apps/v1/machines", get(machines))
        .route(
            "/api/apps/v1/files",
            post(send).layer(DefaultBodyLimit::disable()),
        )
}

async fn info() -> Json<Value> {
    Json(json!({
        "service": "maker-connect",
        "api": API,
        "version": crate::store::BUILD,
        "tokens": "Space page → API tokens; send as Authorization: Bearer",
    }))
}

async fn me(access: Access, State(store): State<Shared>) -> Response {
    let store = store.lock().unwrap();
    let found = (|| -> rusqlite::Result<Value> {
        let organisation = store.organisation_of(access.space)?;
        Ok(json!({
            "space": { "id": store.space_uid(access.space)?, "name": store.space_name(access.space)? },
            "organisation": { "name": store.organisation_name(organisation)? },
            "role": access.role,
            "as": access.actor,
        }))
    })();
    match found {
        Ok(me) => Json(me).into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct MachinesQuery {
    kind: Option<String>,
}

/// A machine as a program needs it.
fn machine_json(machine: &crate::store::Machine, may: Option<Level>) -> Value {
    let profile = machine.profile.as_deref().and_then(profiles::find);
    let telemetry = machine.telemetry.as_ref().filter(|_| machine.online);
    let grbl = telemetry.and_then(|t| t.grbl.as_ref());
    // The work area: what the controller says it travels, else the model's.
    let area = grbl
        .and_then(|g| g.travel_mm)
        .map(|[x, y, _]| [x, y])
        .or_else(|| profile.map(|p| [p.build_volume_mm[0], p.build_volume_mm[1]]))
        .filter(|[x, y]| *x > 0.0 && *y > 0.0);
    json!({
        "id": machine.id,
        "name": machine.name,
        "kind": machine.kind,
        "model": profile.map(|p| format!("{} {}", p.brand, p.model)),
        "profile": machine.profile,
        "online": machine.online,
        "state": telemetry.map(|t| t.state),
        "in_use": machine.out_of_use.is_none(),
        "work_area_mm": area,
        "laser": (machine.kind == MachineKind::Laser).then(|| json!({
            "power_w": machine.laser_w.or_else(|| profile.and_then(|p| p.laser_w)),
            "max_s": grbl.and_then(|g| g.max_power),
            "laser_mode": grbl.and_then(|g| g.laser_mode),
        })),
        // What the token may do with it: "use" or more to send work for it.
        "may": may,
    })
}

async fn machines(
    access: Access,
    State(store): State<Shared>,
    Query(query): Query<MachinesQuery>,
) -> Response {
    let store = store.lock().unwrap();
    let all = match store.machines(access.space) {
        Ok(all) => all,
        Err(error) => return failed(error),
    };
    let list: Vec<Value> = all
        .iter()
        .filter(|m| {
            query.kind.as_deref().is_none_or(|k| {
                serde_json::to_value(m.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    == Some(k.to_owned())
            })
        })
        .map(|m| machine_json(m, store.machine_level(&access, &m.id).ok().flatten()))
        .collect();
    Json(list).into_response()
}

#[derive(Deserialize)]
struct SendQuery {
    name: String,
    machine: Option<String>,
}

/// A file from a program: kept in Files, checked against its machine, and the page to open.
async fn send(
    access: Access,
    State(store): State<Shared>,
    Query(query): Query<SendQuery>,
    body: Body,
) -> Response {
    // The machine first: one the token may not use is refused before anything is stored.
    if let Some(machine) = &query.machine
        && let Err(refused) =
            crate::access::check_machine(&store.lock().unwrap(), &access, machine, Level::Use)
    {
        return *refused;
    }
    let kept = crate::files::upload(
        access.clone(),
        State(store.clone()),
        Query(crate::files::UploadQuery {
            name: query.name.clone(),
        }),
        body,
    )
    .await;
    if kept.status() != StatusCode::CREATED {
        return kept;
    }
    let bytes = axum::body::to_bytes(kept.into_body(), 1 << 16)
        .await
        .unwrap_or_default();
    let Some(id) = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v["id"].as_str().map(str::to_owned))
    else {
        return failed("the file was kept, but its id did not come back");
    };
    let store = store.lock().unwrap();
    let file = match store.file(access.space, &id) {
        Ok(Some(file)) => file,
        Ok(None) => return failed("the file was kept, but is not there"),
        Err(error) => return failed(error),
    };
    // Whether it fits the machine now: the same check as Print on the Files page.
    let check = match &query.machine {
        None => json!(null),
        Some(machine) => match store.machine(access.space, machine) {
            Ok(Some(machine)) => match crate::files::fit(&file, &machine) {
                Ok(None) => json!({ "fits": true }),
                Ok(Some(warning)) => json!({ "fits": true, "warning": warning }),
                Err(problem) => json!({ "fits": false, "problem": problem }),
            },
            Ok(None) => json!(null),
            Err(error) => return failed(error),
        },
    };
    let page = match &query.machine {
        Some(machine) => format!("/files#file={id}&machine={}", encode(machine)),
        None => format!("/files#file={id}"),
    };
    (
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "name": file.name,
            "size": file.size,
            "work_area_mm": file.meta.bounds_mm,
            "max_s": file.meta.max_power,
            "check": check,
            // Open this in a browser: the file with its machine chosen, to frame and start.
            "page": page,
        })),
    )
        .into_response()
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::access::Role;
    use crate::store::Store;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    async fn send(
        app: &axum::Router,
        request: Request<Body>,
    ) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
        let response = app.clone().oneshot(request).await.unwrap();
        let (status, headers) = (response.status(), response.headers().clone());
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            headers,
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
        )
    }

    /// LaserFoundry's side of it: find the server, its lasers, send a file for one; it is
    /// in Files with the laser chosen, and nothing starts.
    #[tokio::test]
    async fn a_program_sends_a_laser_file() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::app(store.clone());
        let (space, laser, member, guest) = {
            let mut s = store.lock().unwrap();
            let owner = s.create_user("olga", "Olga", "x", false).unwrap();
            let (_, space) = s
                .create_organisation("Fablab", "Werkplaats", owner)
                .unwrap();
            let laser = s
                .add_virtual(space, crate::profiles::find("ortur/lm2-pro-s2").unwrap())
                .unwrap();
            let member = s
                .create_token(space, owner, "LaserFoundry", Role::Member)
                .unwrap();
            let guest = s.create_token(space, owner, "Viewer", Role::Guest).unwrap();
            (space, laser, member, guest)
        };
        // Is this a Maker Connect? No token needed, any site may ask.
        let (status, headers, info) = send(
            &app,
            Request::get("/api/apps/v1/info")
                .header("origin", "https://stitchwright.github.io")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(
            (status, info["service"].as_str()),
            (StatusCode::OK, Some("maker-connect"))
        );
        assert_eq!(headers["access-control-allow-origin"], "*");
        let (status, headers, _) = send(
            &app,
            Request::options("/api/apps/v1/files")
                .header("origin", "https://x.example")
                .header("access-control-request-method", "POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(
            headers["access-control-allow-headers"]
                .to_str()
                .unwrap()
                .contains("authorization")
        );
        // Without a token: no.
        let (status, _, _) = send(
            &app,
            Request::get("/api/apps/v1/machines")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let bearer = |t: &str| format!("Bearer {t}");
        // Its lasers, with a work area to lay out on.
        let (status, _, list) = send(
            &app,
            Request::get("/api/apps/v1/machines?kind=laser")
                .header("authorization", bearer(&member))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{list}");
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["id"], laser.as_str());
        assert!(list[0]["work_area_mm"][0].as_f64().unwrap() > 0.0, "{list}");
        assert_eq!(list[0]["may"], "operate");
        let (_, _, none) = send(
            &app,
            Request::get("/api/apps/v1/machines?kind=fdm")
                .header("authorization", bearer(&member))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(none, serde_json::json!([]));
        // A file for the laser.
        let gcode = "G90\nM4 S0\nG0 X10 Y10\nG1 X30 Y10 S300 F1500\nM5\n";
        let file = |token: &str, machine: &str| {
            Request::post(format!(
                "/api/apps/v1/files?name=sign.gcode&machine={machine}"
            ))
            .header("authorization", bearer(token))
            .header("origin", "https://stitchwright.github.io")
            .body(Body::from(gcode))
            .unwrap()
        };
        let (status, headers, sent) = send(&app, file(&member, &laser)).await;
        assert_eq!(status, StatusCode::CREATED, "{sent}");
        assert_eq!(headers["access-control-allow-origin"], "*");
        let id = sent["id"].as_str().unwrap().to_owned();
        assert_eq!(sent["page"], format!("/files#file={id}&machine={laser}"));
        assert_eq!(
            sent["work_area_mm"],
            serde_json::json!([10.0, 10.0, 30.0, 10.0])
        );
        assert!(sent["check"]["fits"].is_boolean(), "{sent}");
        {
            let s = store.lock().unwrap();
            assert_eq!(s.files(space).unwrap().len(), 1);
            assert!(
                s.machine(space, &laser).unwrap().unwrap().job.is_none(),
                "nothing starts by itself"
            );
            let log = s.audit_log(space, 5).unwrap();
            assert!(
                log.iter().any(|l| l.actor == "token \"LaserFoundry\""
                    && l.action == "sent a file from a program"),
                "{log:?}"
            );
        }
        // A guest's token sends nothing.
        assert_eq!(
            send(&app, file(&guest, &laser)).await.0,
            StatusCode::FORBIDDEN
        );
        // A laser that needs an induction the token's maker... has (an owner): but a member's
        // token of a member without one is refused, and nothing is stored.
        let mia_token = {
            let s = store.lock().unwrap();
            let mia = s.create_user("mia", "Mia", "x", false).unwrap();
            s.set_membership(mia, space, "member").unwrap();
            let mut s = s;
            s.set_induction_required(space, &laser, true).unwrap();
            s.create_token(space, mia, "Mia's LaserFoundry", Role::Member)
                .unwrap()
        };
        let (status, _, _) = send(&app, file(&mia_token, &laser)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            store.lock().unwrap().files(space).unwrap().len(),
            1,
            "not stored"
        );
        // A browser's cookie from another site does not count here.
        let cookie = {
            let s = store.lock().unwrap();
            format!(
                "{}={}",
                crate::auth::COOKIE,
                s.create_session(1, Some(space), "", "").unwrap()
            )
        };
        let (status, _, _) = send(
            &app,
            Request::get("/api/apps/v1/machines")
                .header("cookie", cookie)
                .header("origin", "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
