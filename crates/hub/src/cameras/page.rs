//! The Cameras page of a space, one camera's page, and its player and video (docs/archive/plans/cameras.md,
//! "Step 2, the module `cameras`"). Who may watch: [`Legacy::may_view_camera`]; adding,
//! changing and removing take an admin (`access::RULES`, in the audit log), on the Space
//! page's Cameras tab ([`space_tab`]).

use super::store::{Camera, CameraForm};
use crate::access::{Access, Role};
use crate::{STYLE, Store, escape, failed, header};
use axum::{
    Form, Router,
    extract::{Path, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode, header as http_header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;

/// go2rtc's player (`www/video-rtc.js`, v1.9.15, MIT), unchanged.
const VIDEO_RTC: &str = include_str!("video-rtc.js");

/// One camera's MSE player: go2rtc's `VideoRTC` element, used as go2rtc's `video-stream.js`
/// uses it (a subclass defined as a custom element), in its `mse` mode, its `src` the camera's
/// websocket. Our UI's choice (owner, 2026-11-20): the video without the browser's controls.
const LIVE: &str = r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<style>html, body { margin: 0; height: 100%; background: #000; } video-rtc { display: block; height: 100%; }</style>
<script type="module">
import {VideoRTC} from "../video-rtc.js";
class Player extends VideoRTC {
    oninit() {
        super.oninit();
        this.video.controls = false;
    }
}
customElements.define("video-rtc", Player);
const player = document.createElement("video-rtc");
player.mode = "mse";
document.body.appendChild(player);
player.src = location.pathname.replace(/\/live$/, "/ws");
</script>
</head>
<body></body>
</html>"#;

/// An agent camera's player: its newest still.
const STILL: &str = r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<style>html, body { margin: 0; height: 100%; background: #000; } img { display: block; width: 100%; height: 100%; object-fit: contain; }</style>
</head>
<body><img src="snapshot" alt="">
</body>
</html>"#;

pub fn routes() -> Router<Store> {
    super::agent::routes().merge(shown_routes())
}

fn shown_routes() -> Router<Store> {
    Router::new()
        .route("/cameras", get(list).post(add))
        .route("/cameras/{id}", get(camera_page).post(change))
        .route("/cameras/{id}/remove", post(remove))
        .route("/cameras/video-rtc.js", get(video_rtc))
        .route("/cameras/{id}/live", get(live))
        .route("/cameras/{id}/ws", get(mse))
        .route("/cameras/{id}/snapshot", get(still))
}

/// An agent camera's newest still, as it was sent; for whoever may watch the camera.
async fn still(access: Access, State(store): State<Store>, Path(id): Path<i64>) -> Response {
    let grants = match store.grants_for(&access, crate::access::Level::View).await {
        Ok(grants) => grants,
        Err(error) => return failed(error),
    };
    if let Err(refused) = watchable(&store, &access, id, &grants).await {
        return *refused;
    }
    match crate::stock::in_space!(&store, access.space, |w| w.snapshot(id)).await {
        Ok(Some(jpeg)) => (
            [
                (http_header::CONTENT_TYPE, "image/jpeg"),
                (http_header::CACHE_CONTROL, "no-store"),
            ],
            jpeg,
        )
            .into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no picture yet").into_response(),
        Err(error) => failed(error),
    }
}

/// The camera, when `access` may watch it; else the answer.
async fn watchable(
    store: &Store,
    access: &Access,
    id: i64,
    grants: &crate::access::Grants,
) -> Result<Camera, Box<Response>> {
    let camera = match crate::stock::in_space!(store, access.space, |w| w.camera(id)).await {
        Ok(Some(camera)) => camera,
        Ok(None) => {
            return Err(Box::new(
                (StatusCode::NOT_FOUND, "no such camera").into_response(),
            ));
        }
        Err(error) => return Err(Box::new(failed(error))),
    };
    let may = store
        .lock()
        .unwrap()
        .may_view_camera(access, &camera, grants);
    if may {
        Ok(camera)
    } else {
        Err(Box::new(
            (StatusCode::FORBIDDEN, "you may not watch this camera").into_response(),
        ))
    }
}

async fn video_rtc() -> Response {
    ([(http_header::CONTENT_TYPE, "text/javascript")], VIDEO_RTC).into_response()
}

async fn live(access: Access, State(store): State<Store>, Path(id): Path<i64>) -> Response {
    let grants = match store.grants_for(&access, crate::access::Level::View).await {
        Ok(grants) => grants,
        Err(error) => return failed(error),
    };
    match watchable(&store, &access, id, &grants).await {
        Ok(camera) if camera.source == "agent" => Html(STILL).into_response(),
        Ok(_) => Html(LIVE).into_response(),
        Err(refused) => *refused,
    }
}

async fn mse(
    access: Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
    ws: WebSocketUpgrade,
) -> Response {
    let grants = match store.grants_for(&access, crate::access::Level::View).await {
        Ok(grants) => grants,
        Err(error) => return failed(error),
    };
    if let Err(refused) = watchable(&store, &access, id, &grants).await {
        return *refused;
    }
    match super::subscribe(id) {
        Some((backlog, frames_rx)) => {
            ws.on_upgrade(move |ws| super::serve_mse(ws, backlog, frames_rx))
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "this camera is not playing",
        )
            .into_response(),
    }
}

async fn camera_page(access: Access, State(store): State<Store>, Path(id): Path<i64>) -> Response {
    let space = match store.space_name(access.space).await {
        Ok(space) => space,
        Err(error) => return failed(error),
    };
    let machines = store.machines(access.space).await.unwrap_or_default();
    let grants = match store.grants_for(&access, crate::access::Level::View).await {
        Ok(grants) => grants,
        Err(error) => return failed(error),
    };
    let camera = match watchable(&store, &access, id, &grants).await {
        Ok(camera) => camera,
        Err(refused) => return *refused,
    };
    let watches = match &camera.machine_id {
        Some(machine) => match machines.iter().find(|m| &m.id == machine) {
            Some(m) => format!(
                r#"Watches <a href="../machines/{}">{}</a>"#,
                escape(&m.id),
                escape(&m.name)
            ),
            None => "Its machine is gone".to_owned(),
        },
        None => "A room camera".to_owned(),
    };
    Html(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{name} · {title}</title>
<link rel="icon" href="../logo.svg" type="image/svg+xml">
<style>
{STYLE}{CAMERAS_STYLE}
</style>
</head>
<body>
{header}
<main id="main" tabindex="-1">
  <h2>{name}</h2>
  <p class="cam-intro">{watches} · <a href="../cameras">All cameras</a></p>
  {status}<iframe class="cam-player" src="{id}/live" title="{name}" allow="picture-in-picture"></iframe>
</main>
</body>
</html>"##,
        name = escape(&camera.name),
        title = escape(&space),
        header = header(&space, "../", "cameras"),
        id = camera.id,
        status = status_note(&camera),
    ))
    .into_response()
}

async fn list(access: Access, State(store): State<Store>) -> Response {
    let space = match store.space_name(access.space).await {
        Ok(space) => space,
        Err(error) => return failed(error),
    };
    let machines = store.machines(access.space).await.unwrap_or_default();
    let grants = match store.grants_for(&access, crate::access::Level::View).await {
        Ok(grants) => grants,
        Err(error) => return failed(error),
    };
    let cameras = match store.cameras(access.space).await {
        Ok(cameras) => cameras,
        Err(error) => return failed(error),
    };
    let store = store.lock().unwrap();
    let mut rows = String::new();
    for camera in cameras
        .iter()
        .filter(|c| store.may_view_camera(&access, c, &grants))
    {
        rows.push_str(&format!(
            r#"<li class="cam"><a href="cameras/{id}"><b>{name}</b></a><small>{watches}</small>{status}</li>"#,
            id = camera.id,
            name = escape(&camera.name),
            watches = watches(camera, &machines),
            status = status_note(camera),
        ));
    }
    if rows.is_empty() {
        rows = r#"<li class="empty">No cameras you may watch yet.</li>"#.to_owned();
    }
    let manage = if access.role >= Role::Admin {
        r#" · <a href="space#cameras">Add or change cameras in Space</a>"#
    } else {
        ""
    };
    Html(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Cameras · {title}</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{STYLE}{CAMERAS_STYLE}
</style>
</head>
<body>
{header}
<main id="main" tabindex="-1">
  <h2>Cameras</h2>
  <p class="cam-intro">A camera on a machine is seen by whoever may see the machine; a room camera from the role it is set to{manage}.</p>
  <ul class="cams">{rows}</ul>
</main>
</body>
</html>"##,
        title = escape(&space),
        header = header(&space, "", "cameras"),
    ))
    .into_response()
}

/// Why a camera does not play, as one line on its pages ([`super::status`]); nothing while it
/// plays or connects.
fn status_note(camera: &Camera) -> String {
    if camera.source == "agent" {
        return String::new();
    }
    match super::status(camera.id) {
        // While it connects: nothing either, its player starts once it plays.
        super::Status::Playing | super::Status::Connecting => String::new(),
        super::Status::Failed(problem) => format!(
            r#"<p class="cam-status bad" role="status">Not playing: {}</p>"#,
            escape(&problem)
        ),
        super::Status::Off => {
            r#"<p class="cam-status bad" role="status">Not playing: the hub has not started it.</p>"#
                .to_owned()
        }
    }
}

/// What a camera watches, for a list.
fn watches(camera: &Camera, machines: &[crate::store::Machine]) -> String {
    let on = match &camera.machine_id {
        Some(m) => machines
            .iter()
            .find(|x| &x.id == m)
            .map_or_else(|| "its machine is gone".to_owned(), |x| escape(&x.name)),
        None => format!("room · from {}", camera.view_role.as_str()),
    };
    if camera.source == "agent" {
        format!("USB camera on an agent · {on}")
    } else {
        on
    }
}

/// The Cameras tab of the Space page (owner, 2026-11-16: "camera beheer hoort in /space
/// thuis als tab"): for an admin, every camera of the space to change or remove, and one to
/// add; empty for others.
pub(crate) async fn space_tab(
    store: &Store,
    access: &Access,
    machines: &[crate::store::Machine],
) -> String {
    if access.role < Role::Admin {
        return String::new();
    }
    let cameras = store.cameras(access.space).await.unwrap_or_default();
    let mut rows = String::new();
    for camera in &cameras {
        rows.push_str(&format!(
            r#"<li class="cam"><a href="cameras/{id}"><b>{name}</b></a><small>{watches}</small>{status}{edit}</li>"#,
            id = camera.id,
            name = escape(&camera.name),
            watches = watches(camera, machines),
            status = status_note(camera),
            edit = edit_form(camera, machines),
        ));
    }
    if rows.is_empty() {
        rows = r#"<li class="empty">No cameras yet.</li>"#.to_owned();
    }
    format!(
        // The style after the card: `.card + .card` gives it its margin from the card above.
        r#"<section class="card"><h3>Cameras</h3>
<p class="cam-intro">ONVIF/RTSP cameras on this hub's network; an agent's USB camera shows up here by itself. A camera on a machine is seen by whoever may see the machine; a room camera from the role set here. Watch them under <a href="cameras">Cameras</a>.</p>
<ul class="cams">{rows}</ul>
<details class="cam-add"><summary>Add a camera</summary>
<form method="post" action="cameras">{fields}<button>Add</button></form></details></section>
<style>{CAMERAS_STYLE}</style>"#,
        fields = fields(None, machines)
    )
}

/// The fields of the add and edit forms; the password is never filled in.
fn fields(camera: Option<&Camera>, machines: &[crate::store::Machine]) -> String {
    let value = |f: fn(&Camera) -> String| camera.map(f).unwrap_or_default();
    let on = camera.and_then(|c| c.machine_id.clone());
    let mut machine_options = format!(
        r#"<option value=""{}>None: a room camera</option>"#,
        if on.is_none() { " selected" } else { "" }
    );
    for m in machines {
        machine_options.push_str(&format!(
            r#"<option value="{id}"{sel}>{name}</option>"#,
            id = escape(&m.id),
            name = escape(&m.name),
            sel = if on.as_deref() == Some(m.id.as_str()) {
                " selected"
            } else {
                ""
            },
        ));
    }
    let role = camera.map_or(Role::Admin, |c| c.view_role);
    let from_agent = camera.is_some_and(|c| c.source == "agent");
    // An agent's camera has no address, user or password here: it sends its stills itself.
    let connection = if from_agent {
        String::new()
    } else {
        format!(
            r#"<label>Address <input name="url" required placeholder="rtsp://192.168.1.20:554/stream" value="{url}"></label>
<label>User <input name="username" autocomplete="off" value="{user}"></label>
<label>Password <input name="password" type="password" autocomplete="new-password" placeholder="{pw_hint}"></label>"#,
            url = escape(&value(|c| c.url.clone())),
            user = escape(&value(|c| c.username.clone().unwrap_or_default())),
            pw_hint = if camera.is_some() { "unchanged" } else { "" },
        )
    };
    let mut role_options = String::new();
    for r in [Role::Guest, Role::Member, Role::Admin, Role::Owner] {
        role_options.push_str(&format!(
            r#"<option value="{v}"{sel}>{v}</option>"#,
            v = r.as_str(),
            sel = if r == role { " selected" } else { "" }
        ));
    }
    format!(
        r#"<label>Name <input name="name" required maxlength="60" value="{name}"></label>
{connection}
<label>On machine <select name="machine">{machine_options}</select></label>
<label>Room camera seen from <select name="view_role">{role_options}</select></label>"#,
        name = escape(&value(|c| c.name.clone())),
    )
}

fn edit_form(camera: &Camera, machines: &[crate::store::Machine]) -> String {
    format!(
        r#"<details class="cam-edit"><summary>Change</summary>
<form method="post" action="cameras/{id}">{fields}<button>Save</button></form>
<form method="post" action="cameras/{id}/remove"><button class="danger">Remove</button></form></details>"#,
        id = camera.id,
        fields = fields(Some(camera), machines),
    )
}

#[derive(Deserialize)]
struct Fields {
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    machine: String,
    #[serde(default)]
    view_role: String,
}

impl Fields {
    /// What the admin sets on an agent's camera: its name, what it watches, who sees a room camera.
    fn agent(self) -> Result<(String, Option<String>, Role), Box<Response>> {
        let name = self.name.trim().to_owned();
        if name.is_empty() {
            return Err(Box::new(
                (StatusCode::BAD_REQUEST, "a camera needs a name").into_response(),
            ));
        }
        Ok((
            name,
            Some(self.machine).filter(|m| !m.is_empty()),
            Role::parse(&self.view_role).unwrap_or(Role::Admin),
        ))
    }

    fn form(self) -> Result<CameraForm, Box<Response>> {
        let name = self.name.trim().to_owned();
        let url = self.url.trim().to_owned();
        if name.is_empty() || url.is_empty() {
            return Err(Box::new(
                (
                    StatusCode::BAD_REQUEST,
                    "a camera needs a name and an address",
                )
                    .into_response(),
            ));
        }
        // An empty field is "not given", as a flag the example's command line leaves out.
        Ok(CameraForm {
            name,
            url,
            username: Some(self.username).filter(|u| !u.is_empty()),
            password: Some(self.password).filter(|p| !p.is_empty()),
            machine_id: Some(self.machine).filter(|m| !m.is_empty()),
            view_role: Role::parse(&self.view_role).unwrap_or(Role::Admin),
        })
    }
}

async fn add(
    access: Access,
    State(store): State<Store>,
    headers: HeaderMap,
    Form(fields): Form<Fields>,
) -> Response {
    let form = match fields.form() {
        Ok(form) => form,
        Err(refused) => return *refused,
    };
    let added = crate::stock::in_space!(&store, access.space, |w| async {
        let id = w.add_camera(&form).await?;
        w.camera(id).await
    })
    .await;
    match added {
        Ok(Some(camera)) => {
            super::start(&camera);
            crate::done(&headers, "/space#cameras")
        }
        Ok(None) => failed("the camera was not stored"),
        Err(error) => failed(error),
    }
}

async fn change(
    access: Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(fields): Form<Fields>,
) -> Response {
    let existing = match crate::stock::in_space!(&store, access.space, |w| w.camera(id)).await {
        Ok(existing) => existing,
        Err(error) => return failed(error),
    };
    let changed = if existing.is_some_and(|c| c.source == "agent") {
        let (name, machine, role) = match fields.agent() {
            Ok(set) => set,
            Err(refused) => return *refused,
        };
        crate::stock::in_space!(&store, access.space, |w| async {
            if w.change_agent_camera(id, &name, machine.as_deref(), role)
                .await?
            {
                w.camera(id).await
            } else {
                Ok(None)
            }
        })
        .await
    } else {
        let form = match fields.form() {
            Ok(form) => form,
            Err(refused) => return *refused,
        };
        crate::stock::in_space!(&store, access.space, |w| async {
            if w.change_camera(id, &form).await? {
                w.camera(id).await
            } else {
                Ok(None)
            }
        })
        .await
    };
    match changed {
        Ok(Some(camera)) => {
            super::start(&camera);
            crate::done(&headers, "/space#cameras")
        }
        Ok(None) => (StatusCode::NOT_FOUND, "no such camera").into_response(),
        Err(error) => failed(error),
    }
}

async fn remove(
    access: Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    match crate::stock::in_space!(&store, access.space, |w| w.remove_camera(id)).await {
        Ok(true) => {
            super::stop(id);
            crate::done(&headers, "/space#cameras")
        }
        Ok(false) => (StatusCode::NOT_FOUND, "no such camera").into_response(),
        Err(error) => failed(error),
    }
}

/// The Cameras tab of a machine's page, and the first camera for its Now tab: its cameras
/// that `access` may watch. Each player loads only while its tab is shown (machine.js, `data-src`).
pub(crate) async fn machine_tab(
    store: &Store,
    access: &Access,
    machine: &str,
    grants: &crate::access::Grants,
) -> Option<(String, String)> {
    let cameras = store.cameras(access.space).await.unwrap_or_default();
    let cameras = store
        .lock()
        .unwrap()
        .machine_cameras(access, machine, grants, cameras);
    let player = |id: i64, name: &str| {
        format!(
            r#"<iframe class="cam-player" data-src="../cameras/{id}/live" title="{name}" allow="picture-in-picture"></iframe>"#,
            name = escape(name),
        )
    };
    let first = cameras.first()?;
    let mut out = String::new();
    for camera in &cameras {
        out.push_str(&format!(
            r#"<h3><a href="../cameras/{id}">{name}</a></h3>{}{}"#,
            status_note(camera),
            player(camera.id, &camera.name),
            id = camera.id,
            name = escape(&camera.name),
        ));
    }
    let preview = format!(
        r##"<div class="card now-cam"><div class="card-head"><h3><a href="#cameras">{name}</a></h3></div>{}{}</div>"##,
        status_note(first),
        player(first.id, &first.name),
        name = escape(&first.name),
    );
    Some((out, preview))
}

pub(crate) const CAMERAS_STYLE: &str = r#"
  .cam-intro { color: var(--muted); margin: 0 0 12px; max-width: 640px; font-size: 14px; }
  .cams { list-style: none; padding: 0; margin: 0 0 20px; display: grid; gap: 10px; }
  .cam { background: var(--surface); border: 1px solid var(--line); border-radius: 0; padding: 12px 16px; display: grid; gap: 4px; }
  .cam small { color: var(--muted); }
  .cam-edit form, .cam-add form { display: grid; gap: 8px; max-width: 520px; margin-top: 8px; }
  .cam-edit label, .cam-add label { display: grid; gap: 4px; font-size: 14px; }
  .cam-status { margin: 0 0 8px; font-size: 14px; color: var(--muted); }
  .cam-status.bad { color: var(--st-bad); }
  .cam-player { width: 100%; aspect-ratio: 16 / 9; border: 0; border-radius: 0; background: #000; }
"#;
