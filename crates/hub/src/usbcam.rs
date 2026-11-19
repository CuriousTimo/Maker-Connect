//! USB webcams, step 1, the proof (docs/briefs/usb-cam-step1.md): an agent POSTs a JPEG of its camera
//! ([`maker_connect_protocol::CAMERA_FRAME`]), the hub keeps only the latest per (agent, camera index)
//! in memory and says "new frame" over server-sent events; an admin's test page shows it.
//!
//! All ours (no reference: nothing goes to a device). Step 2 moves this into the module `cameras`.

use crate::access::Access;
use crate::{STYLE, Store, escape, failed, header};
use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header as http_header},
    response::{
        Html, IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use maker_connect_protocol as protocol;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The latest frame of a camera: the agent's machine id and the camera's index on it.
struct Latest {
    jpeg: Bytes,
    /// Counts the frames this hub run has had of this camera; the page reloads when it changes.
    seq: u64,
}

static LATEST: Mutex<Option<HashMap<(String, u32), Latest>>> = Mutex::new(None);
static EVENTS: OnceLock<tokio::sync::broadcast::Sender<String>> = OnceLock::new();

fn events() -> &'static tokio::sync::broadcast::Sender<String> {
    EVENTS.get_or_init(|| tokio::sync::broadcast::channel(64).0)
}

/// What the page is told: which camera has a new frame.
fn event_data(machine: &str, index: u32, seq: u64) -> String {
    serde_json::json!({ "machine": machine, "index": index, "seq": seq }).to_string()
}

/// Keeps `jpeg` as the camera's latest frame and announces it.
fn keep(machine: &str, index: u32, jpeg: Bytes) {
    let mut latest = LATEST.lock().unwrap();
    let slot = latest
        .get_or_insert_with(HashMap::new)
        .entry((machine.to_owned(), index))
        .or_insert(Latest {
            jpeg: Bytes::new(),
            seq: 0,
        });
    slot.jpeg = jpeg;
    slot.seq += 1;
    let _ = events().send(event_data(machine, index, slot.seq));
}

pub fn routes() -> Router<Store> {
    Router::new()
        .route(protocol::CAMERA_FRAME, post(frame_in))
        .route("/usb-cameras/test", get(test_page))
        .route("/usb-cameras/test/events", get(stream))
        .route("/usb-cameras/test/frame/{machine}/{index}", get(frame_out))
}

/// An agent's frame, by its token (as for telemetry): only a JPEG is kept.
async fn frame_in(
    State(store): State<Store>,
    Path(index): Path<u32>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let machine = match store.machine_id_by_token(token).await {
        Ok(Some(machine)) => machine,
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(error) => return failed(error),
    };
    if !body.starts_with(&[0xff, 0xd8]) {
        return (StatusCode::BAD_REQUEST, "not a JPEG").into_response();
    }
    keep(&machine, index, body);
    StatusCode::NO_CONTENT.into_response()
}

/// The camera's latest frame, if its agent's machine is in the admin's space.
async fn frame_out(
    access: Access,
    State(store): State<Store>,
    Path((machine, index)): Path<(String, u32)>,
) -> Response {
    match store.machine(access.space, &machine).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, "no such camera").into_response(),
        Err(error) => return failed(error),
    }
    let jpeg = LATEST
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&(machine, index)).map(|l| l.jpeg.clone()));
    match jpeg {
        Some(jpeg) => (
            [
                (http_header::CONTENT_TYPE, "image/jpeg"),
                (http_header::CACHE_CONTROL, "no-store"),
            ],
            jpeg,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "no frame yet").into_response(),
    }
}

/// The cameras of the admin's space that have a frame now, then every new frame of theirs.
async fn stream(
    access: Access,
    State(store): State<Store>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    use tokio::sync::broadcast::error::RecvError;
    let rx = events().subscribe();
    let mut now: Vec<String> = LATEST
        .lock()
        .unwrap()
        .as_ref()
        .map(|m| {
            m.iter()
                .map(|((id, i), l)| event_data(id, *i, l.seq))
                .collect()
        })
        .unwrap_or_default();
    now.reverse();
    let stream = futures_util::stream::unfold((rx, now), move |(mut rx, mut now)| {
        let store = store.clone();
        async move {
            loop {
                let data = match now.pop() {
                    Some(data) => data,
                    None => match rx.recv().await {
                        Ok(data) => data,
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => return None,
                    },
                };
                let machine = serde_json::from_str::<serde_json::Value>(&data)
                    .ok()
                    .and_then(|v| v["machine"].as_str().map(str::to_owned))
                    .unwrap_or_default();
                let mine = matches!(store.machine(access.space, &machine).await, Ok(Some(_)));
                if mine {
                    return Some((Ok(Event::default().data(data)), (rx, now)));
                }
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

const PAGE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>USB cameras test · {space}</title>
<link rel="icon" href="../logo.svg" type="image/svg+xml">
<style>
{style}
.usbcam { display: flex; flex-wrap: wrap; gap: 1rem; }
.usbcam figure { margin: 0; }
.usbcam img { max-width: 100%; display: block; }
</style>
</head>
<body>
{header}
<main>
  <h2>USB cameras (test)</h2>
  <p>The latest frame of each camera an agent sends; reloaded when a new one comes. Nothing is kept on disk.</p>
  <div class="usbcam" id="cams"><p id="none">No frame yet.</p></div>
</main>
<script>
const base = location.pathname.replace(/\/$/, "");
const cams = document.getElementById("cams");
const events = new EventSource(base + "/events");
events.onmessage = (e) => {
  const f = JSON.parse(e.data);
  const id = "cam-" + f.machine + "-" + f.index;
  let img = document.getElementById(id);
  if (!img) {
    document.getElementById("none")?.remove();
    const fig = document.createElement("figure");
    img = document.createElement("img");
    img.id = id;
    img.alt = "camera " + f.index;
    const cap = document.createElement("figcaption");
    cap.textContent = f.machine + " camera " + f.index;
    fig.append(img, cap);
    cams.append(fig);
  }
  img.src = base + "/frame/" + encodeURIComponent(f.machine) + "/" + f.index + "?n=" + f.seq;
};
</script>
</body>
</html>"##;

async fn test_page(access: Access, State(store): State<Store>) -> Response {
    let space = match store.space_name(access.space).await {
        Ok(space) => space,
        Err(error) => return failed(error),
    };
    Html(
        PAGE.replace("{style}", STYLE)
            .replace("{header}", &header(&space, "../", "machines"))
            .replace("{space}", &escape(&space)),
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::Role;

    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use maker_connect_protocol::{MachineKind, Register};

    use tower::ServiceExt;

    const JPEG: [u8; 14] = [0xff, 0xd8, 0xff, 0xe0, 1, 2, 3, 4, 5, 6, 7, 8, 0xff, 0xd9];

    struct Rig {
        app: Router,
        machine: String,
        token: String,
        admin: String,
        member: String,
    }
    async fn rig(pool: &sqlx::PgPool) -> Rig {
        let store = crate::store::Store::for_test(pool).await;
        let (space, _) = store.default_space().await.unwrap();
        let (machine, token) = {
            let (machine, token, code) = store
                .register(&Register {
                    protocol: protocol::VERSION,
                    name: "cam agent".into(),
                    kind: MachineKind::Other,
                    agent_version: "0.2.0".into(),
                })
                .await
                .unwrap();
            store.pair(space, &code).await.unwrap().unwrap();
            (machine, token)
        };
        let cookie = async |name: &str, role: Role| {
            let user = store.create_user(name, name, "x", false).await.unwrap();
            store
                .set_org_membership(user, space, role.as_str())
                .await
                .unwrap();
            format!(
                "{}={}",
                crate::auth::COOKIE,
                store.create_session(user, None, "", "").await.unwrap()
            )
        };
        let (admin, member) = (
            cookie("adam", Role::Admin).await,
            cookie("mia", Role::Member).await,
        );
        Rig {
            app: crate::app(store),
            machine,
            token,
            admin,
            member,
        }
    }

    impl Rig {
        async fn send(&self, request: Request<Body>) -> axum::response::Response {
            self.app.clone().oneshot(request).await.unwrap()
        }
        async fn post_frame(&self, token: &str, body: &[u8]) -> StatusCode {
            let path = protocol::CAMERA_FRAME.replace("{index}", "0");
            let request = Request::post(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "image/jpeg")
                .body(Body::from(body.to_vec()))
                .unwrap();
            self.send(request).await.status()
        }
        async fn get(&self, cookie: Option<&str>, path: &str) -> axum::response::Response {
            let mut request = Request::get(path);
            if let Some(cookie) = cookie {
                request = request.header("cookie", cookie);
            }
            self.send(request.body(Body::empty()).unwrap()).await
        }
    }

    async fn next_chunk(body: &mut Body) -> String {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(60), body.frame())
            .await
            .expect("an event in time")
            .unwrap()
            .unwrap();
        String::from_utf8_lossy(frame.data_ref().unwrap()).into_owned()
    }

    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_frame_is_kept_latest_only_and_announced(pool: sqlx::PgPool) {
        let r = rig(&pool).await;
        let (bad, not_jpeg) = (
            r.post_frame("nope", &JPEG).await,
            r.post_frame(&r.token, b"GIF89a-------").await,
        );
        assert_eq!(
            (bad, not_jpeg),
            (StatusCode::UNAUTHORIZED, StatusCode::BAD_REQUEST)
        );
        assert_eq!(r.post_frame(&r.token, &JPEG).await, StatusCode::NO_CONTENT);
        let url = format!("/usb-cameras/test/frame/{}/0", r.machine);
        let first = r.get(Some(&r.admin), &url).await;
        assert_eq!(first.headers()["content-type"], "image/jpeg");
        assert_eq!(
            first
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .as_ref(),
            JPEG
        );

        // The page's stream: the frame there is, then the next one.
        let events = r.get(Some(&r.admin), "/usb-cameras/test/events").await;
        assert_eq!(events.headers()["content-type"], "text/event-stream");
        let mut body = events.into_body();
        let now = next_chunk(&mut body).await;
        assert!(
            now.contains(&r.machine) && now.contains(r#""seq":1"#),
            "{now}"
        );
        let mut second = JPEG.to_vec();
        second.insert(4, 9);
        assert_eq!(
            r.post_frame(&r.token, &second).await,
            StatusCode::NO_CONTENT
        );
        let new = next_chunk(&mut body).await;
        assert!(new.contains(r#""seq":2"#), "{new}");
        let latest = r
            .get(Some(&r.admin), &url)
            .await
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(
            latest.as_ref(),
            second.as_slice(),
            "only the latest is kept"
        );
        let kept = LATEST.lock().unwrap();
        assert_eq!(
            kept.as_ref()
                .unwrap()
                .keys()
                .filter(|(m, _)| *m == r.machine)
                .count(),
            1
        );
    }

    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_test_page_is_for_admins(pool: sqlx::PgPool) {
        let r = rig(&pool).await;
        let frame = format!("/usb-cameras/test/frame/{}/0", r.machine);
        for path in [
            "/usb-cameras/test",
            "/usb-cameras/test/events",
            frame.as_str(),
        ] {
            assert_eq!(
                r.get(None, path).await.status(),
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
            assert_eq!(
                r.get(Some(&r.member), path).await.status(),
                StatusCode::FORBIDDEN,
                "{path}"
            );
        }
        let page = r.get(Some(&r.admin), "/usb-cameras/test").await;
        assert_eq!(page.status(), StatusCode::OK);
        let html = String::from_utf8(
            page.into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("EventSource"));
        assert_eq!(
            r.get(Some(&r.admin), &frame).await.status(),
            StatusCode::NOT_FOUND,
            "no frame yet"
        );
    }
}
