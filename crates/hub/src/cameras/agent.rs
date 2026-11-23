//! The hub's side of an agent's USB camera: `POST /p/camera` and `PUT /c/snapshot`. The hub's
//! answers are ours, built to what SDK c0d7d233 reads: `models.py:198-225` (the registration),
//! `__init__.py:822-826` and `:855-859` (the printer's headers; the answer's `Token` header is the
//! camera's token), `camera.py:54-77` (the snapshot) and `camera_controller.py:176-179` (`401` or
//! `403`: the camera registers again).

use crate::{Store, failed};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{post, put},
};
use serde_json::Value;

pub fn routes() -> Router<Store> {
    Router::new().route("/p/camera", post(register)).route(
        "/c/snapshot",
        put(snapshot).layer(DefaultBodyLimit::disable()),
    )
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

/// `POST /p/camera`; an unknown printer token is `401`, as `/p/telemetry` answers.
async fn register(State(store): State<Store>, headers: HeaderMap, body: String) -> Response {
    let (_machine, space) = match store.placed(header(&headers, "token")).await {
        Ok(Some(placed)) => placed,
        Ok(None) => {
            return (StatusCode::UNAUTHORIZED, "unknown printer: register again").into_response();
        }
        Err(error) => return failed(error),
    };
    let mut registration: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => {
            return (StatusCode::BAD_REQUEST, format!("not JSON: {error}")).into_response();
        }
    };
    let Some(fingerprint) = registration["fingerprint"]
        .as_str()
        .filter(|f| !f.is_empty())
        .map(str::to_owned)
    else {
        return (StatusCode::BAD_REQUEST, "the camera has no fingerprint").into_response();
    };
    let name = registration["config"]["name"]
        .as_str()
        .filter(|n| !n.trim().is_empty())
        // Ours: the name when the SDK sends none.
        .unwrap_or("USB camera")
        .trim()
        .to_owned();
    if let Some(config) = registration["config"].as_object_mut() {
        config.remove("token");
    }
    let token = crate::accounts::random_token();
    let registration = registration.to_string();
    let stored = crate::stock::in_space!(&store, space, |w| w.register_agent_camera(
        &fingerprint,
        &name,
        &token,
        &registration
    ))
    .await;
    match stored {
        Ok(id) => {
            println!("USB camera {id} registered in space {space}: {name}");
            (StatusCode::OK, [("Token", token)]).into_response()
        }
        Err(error) => failed(error),
    }
}

/// `PUT /c/snapshot`: the camera's `Token` and `Fingerprint`; a bad one is `401`.
async fn snapshot(State(store): State<Store>, headers: HeaderMap, body: Bytes) -> Response {
    let unknown = || (StatusCode::UNAUTHORIZED, "unknown camera: register again").into_response();
    let token = header(&headers, "token");
    if token.is_empty() {
        return unknown();
    }
    let (id, space) = match store.camera_by_token(token).await {
        Ok(Some((id, space, Some(fingerprint))))
            if fingerprint == header(&headers, "fingerprint") =>
        {
            (id, space)
        }
        Ok(_) => return unknown(),
        Err(error) => return failed(error),
    };
    // Ours: an empty body is no picture; the SDK only logs a `400` (`camera_controller.py:183-184`).
    if body.is_empty() {
        return (StatusCode::BAD_REQUEST, "no picture").into_response();
    }
    match crate::stock::in_space!(&store, space, |w| w.keep_snapshot(id, &body)).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(error) => failed(error),
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode, header},
    };
    use serde_json::json;
    use tower::ServiceExt;

    use crate::store::Store;

    /// The fingerprint of an SDK camera: `make_fingerprint(camera_id)` is the SHA-256 of its id in hex
    /// (`util.py:22-24`); the test uses one such value.
    const CAMERA_ID: &str = "usb-0000:00:14.0-1";
    const FINGERPRINT: &str = "c8590ee0b141d78980f57878e22b507f16fdf0c62ac32fbfc496aa502899d85f";
    /// A JPEG's first and last bytes; the hub keeps whatever bytes come.
    const JPEG: &[u8] = &[
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0xFF, 0xD9,
    ];

    async fn send(
        app: &Router,
        request: Request<Body>,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, headers, body)
    }

    /// An agent's printer, registered and paired as the SDK does (`POST /p/register`, then
    /// `GET /p/register` with its `Code`); its token.
    async fn printer(store: &Store, app: &Router) -> String {
        let (status, headers, _) = send(
            app,
            Request::post("/p/register")
                .body(Body::from(
                    r#"{"sn":"AGENT-1","fingerprint":"agent-fingerprint","printer_type":"maker-connect/agent","firmware":"0.1.0"}"#,
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let code = headers["code"].to_str().unwrap().to_owned();
        let space = store.default_space().await.unwrap().0;
        assert!(store.pair(space, &code).await.unwrap().is_some());
        let (_, headers, _) = send(
            app,
            Request::get("/p/register")
                .header("Code", &code)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        headers["token"].to_str().unwrap().to_owned()
    }

    /// `CameraRegister.to_payload` (`models.py:206-225`) for a USB camera: `json_from_settings`
    /// (`camera.py:509-520`: `trigger_scheme` as the enum's name, `resolution` as a dict) plus
    /// `camera_id`, `name` (`camera.py:528`), `options.available_resolutions`, `capabilities` and
    /// `fingerprint`. `extra` is a key of `config` the SDK does not know (a later `stream_url`).
    fn payload(name: &str, extra: bool) -> String {
        let mut config = json!({
            "trigger_scheme": "THIRTY_SEC",
            "resolution": {"width": 640, "height": 480},
            "rotation": 0,
            "name": name,
            "camera_id": CAMERA_ID,
        });
        if extra {
            config["stream_url"] = json!("http://192.0.2.7:8080/cameras/1/stream.mjpg");
        }
        json!({
            "config": config,
            "options": {"available_resolutions": [
                {"width": 640, "height": 480}, {"width": 1280, "height": 720}]},
            "capabilities": ["trigger_scheme", "imaging", "resolution"],
            "fingerprint": FINGERPRINT,
        })
        .to_string()
    }

    /// `__init__.py:822-826` with `make_headers` (`:273-292`): the printer's `Fingerprint`, `Timestamp`
    /// and `Token`.
    fn register(printer_token: &str, body: String) -> Request<Body> {
        Request::post("/p/camera")
            .header("Fingerprint", "agent-fingerprint")
            .header("Timestamp", "1700000000.5")
            .header("Token", printer_token)
            .header("User-Agent", "Prusa-Connect-SDK-Printer/0.8.0")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap()
    }

    /// `Snapshot.send` (`camera.py:133-147`): `PUT` to `/c/snapshot` with the headers `Timestamp`,
    /// `Fingerprint`, `Token` and `Content-Type: image/jpg`, the query `printer_uuid` when set, the
    /// JPEG as the body.
    fn snapshot(camera_token: &str, fingerprint: &str, jpeg: &[u8]) -> Request<Body> {
        Request::put("/c/snapshot?printer_uuid=0b0f1d2a-0000-4000-8000-000000000001")
            .header("Timestamp", "1700000010.25")
            .header("Fingerprint", fingerprint)
            .header("Token", camera_token)
            .header(header::CONTENT_TYPE, "image/jpg")
            .body(Body::from(jpeg.to_vec()))
            .unwrap()
    }

    async fn camera_of(store: &Store, space: i64) -> crate::cameras::store::Camera {
        let mut all = store.cameras(space).await.unwrap();
        assert_eq!(all.len(), 1);
        all.remove(0)
    }

    /// `__init__.py:855-859`: on `200` the SDK takes `res.headers["Token"]` as the camera token. The
    /// camera shows in the space as an agent camera, named as the SDK names it, linked to nothing, and
    /// its `config` kept as it came.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn registration_gives_a_token(pool: sqlx::PgPool) {
        let store = Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let space = store.default_space().await.unwrap().0;
        let printer = printer(&store, &app).await;
        let (status, headers, _) =
            send(&app, register(&printer, payload("Logitech C270", true))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!headers["token"].to_str().unwrap().is_empty());
        let camera = camera_of(&store, space).await;
        assert_eq!(
            (
                camera.source.as_str(),
                camera.name.as_str(),
                camera.machine_id
            ),
            ("agent", "Logitech C270", None)
        );
        // `models.py:207-212`: the config as the SDK builds it, with a key it does not know.
        let kept = {
            let mut w = store.work(space).await.unwrap();
            let kept: String = sqlx::query_scalar("SELECT registration FROM cameras WHERE id = $1")
                .bind(camera.id)
                .fetch_one(w.c())
                .await
                .unwrap();
            w.commit(&store).await.unwrap();
            serde_json::from_str::<serde_json::Value>(&kept).unwrap()
        };
        assert_eq!(
            kept["config"]["stream_url"],
            "http://192.0.2.7:8080/cameras/1/stream.mjpg"
        );
        assert_eq!(kept["capabilities"][1], "imaging");
        // Not the printer's token, an unknown one: refused, nothing kept (as `/p/telemetry`).
        let (status, _, _) = send(&app, register("unknown", payload("x", false))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(store.cameras(space).await.unwrap().len(), 1);
    }

    /// `camera.py:133-147`: a snapshot with the camera's token and fingerprint is accepted; only the
    /// newest is kept; whoever may watch the camera gets it back.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_snapshot_with_its_token_is_kept(pool: sqlx::PgPool) {
        let store = Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let space = store.default_space().await.unwrap().0;
        let printer = printer(&store, &app).await;
        let (_, headers, _) = send(&app, register(&printer, payload("Logitech C270", false))).await;
        let token = headers["token"].to_str().unwrap().to_owned();
        assert_eq!(camera_of(&store, space).await.snapshot_at, None);
        let (status, _, _) = send(&app, snapshot(&token, FINGERPRINT, JPEG)).await;
        assert_eq!(status, StatusCode::OK);
        let camera = camera_of(&store, space).await;
        assert!(camera.snapshot_at.is_some());
        // A newer still replaces it.
        let newer: Vec<u8> = JPEG.iter().rev().copied().collect();
        assert_eq!(
            send(&app, snapshot(&token, FINGERPRINT, &newer)).await.0,
            StatusCode::OK
        );
        let kept = {
            let mut w = store.work(space).await.unwrap();
            let kept = w.snapshot(camera.id).await.unwrap();
            w.commit(&store).await.unwrap();
            kept
        };
        assert_eq!(kept.as_deref(), Some(newer.as_slice()));
        // Shown to whoever may watch it, as it was sent.
        let (status, headers, body) = send(
            &app,
            Request::get(format!("/cameras/{}/snapshot", camera.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CONTENT_TYPE], "image/jpeg");
        assert_eq!(body, newer);
    }

    /// `camera_controller.py:176-179`: on `401` or `403` the SDK drops the camera token and registers
    /// again. Ours: `401` for an unknown token, for another camera's fingerprint and for no token.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_wrong_token_is_refused(pool: sqlx::PgPool) {
        let store = Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let space = store.default_space().await.unwrap().0;
        let printer = printer(&store, &app).await;
        let (_, headers, _) = send(&app, register(&printer, payload("Logitech C270", false))).await;
        let token = headers["token"].to_str().unwrap().to_owned();
        for (token, fingerprint) in [
            ("not-a-token", FINGERPRINT),
            (token.as_str(), "another-camera"),
            // The printer's token is not a camera's.
            (printer.as_str(), FINGERPRINT),
            ("", FINGERPRINT),
        ] {
            let (status, _, _) = send(&app, snapshot(token, fingerprint, JPEG)).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{token:?} {fingerprint:?}"
            );
        }
        assert_eq!(camera_of(&store, space).await.snapshot_at, None);
    }

    /// The SDK registers again after a `401` (`camera_controller.py:176-179`): the same fingerprint is
    /// the same camera, with the name and the link the admin gave; the new token works, the old one not.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn registering_again_keeps_the_camera(pool: sqlx::PgPool) {
        let store = Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let space = store.default_space().await.unwrap().0;
        let printer = printer(&store, &app).await;
        let (_, first, _) = send(&app, register(&printer, payload("Logitech C270", false))).await;
        let first = first["token"].to_str().unwrap().to_owned();
        let camera = camera_of(&store, space).await;
        // The admin names it and links it to a machine.
        let machine = store
            .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
            .await
            .unwrap();
        let mut w = store.work(space).await.unwrap();
        assert!(
            w.change_agent_camera(
                camera.id,
                "Over the bed",
                Some(&machine),
                crate::access::Role::Admin
            )
            .await
            .unwrap()
        );
        w.commit(&store).await.unwrap();
        let (status, second, _) =
            send(&app, register(&printer, payload("Logitech C270", true))).await;
        assert_eq!(status, StatusCode::OK);
        let second = second["token"].to_str().unwrap().to_owned();
        assert_ne!(first, second);
        let again = camera_of(&store, space).await;
        assert_eq!(
            (again.id, again.name.as_str(), again.machine_id.as_deref()),
            (camera.id, "Over the bed", Some(machine.as_str()))
        );
        assert_eq!(
            send(&app, snapshot(&first, FINGERPRINT, JPEG)).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            send(&app, snapshot(&second, FINGERPRINT, JPEG)).await.0,
            StatusCode::OK
        );
    }
}
