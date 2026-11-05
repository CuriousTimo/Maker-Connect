//! A space's G-code files, and printing one on a machine. Files are kept on disk beside the
//! database (`<id>.gcode`, with `<id>.png` for a thumbnail found in it); their metadata
//! ([`crate::gcode`]) in the store. Printing makes a job on the machine: virtual machines
//! play it ([`crate::virtual_machines`]), an agent streams it (GRBL lasers, Marlin printers
//! over USB); Prusas on built-in Connect follow.

use crate::store::{JobState, Machine, StoredFile};
use crate::{Shared, failed, profiles};
use axum::{
    Form, Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

/// The largest file taken.
const MAX_BYTES: u64 = 1024 * 1024 * 1024;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route(
            "/api/files",
            get(list).post(upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/files/{id}/thumbnail.png", get(thumbnail))
        .route("/files/{id}/download", get(download))
        .route("/api/agent/v1/jobs/{id}/file", get(agent_file))
        .route("/files/{id}/remove", post(remove))
        .route("/api/files/remove", post(remove_many))
        .route("/api/storage", get(storage))
        .route("/files/{id}/print", post(print))
        .route("/files/{id}/frame", post(frame))
        .route("/machines/{id}/job", post(control_job))
}

/// Whether a file can go on a machine now: Ok with a warning, or why not.
pub fn fit(file: &StoredFile, machine: &Machine) -> Result<Option<String>, String> {
    let profile = machine.profile.as_deref().and_then(profiles::find);
    if let Some(kind) = file.meta.kind
        && kind != machine.kind
    {
        return Err(format!("a file for a {}", kind.label().to_lowercase()));
    }
    if machine.kind == maker_connect_protocol::MachineKind::Label {
        return label_check(file, machine, profile);
    }
    let mut warning = None;
    if let Some(note) = laser_check(file, machine, profile)? {
        warning = Some(note);
    }
    if let Some(model) = &file.meta.printer_model {
        match profile.map(|p| &p.printer_models) {
            Some(models) if models.iter().any(|m| m == model) => {}
            Some(models) if !models.is_empty() => return Err(format!("sliced for {model}")),
            _ => warning = Some(format!("sliced for {model}; not known for this machine")),
        }
    } else if file.meta.kind.is_none() {
        warning = Some("the file does not say what it is for".to_owned());
    }
    match machine.link.as_str() {
        // An agent streams to a GRBL machine (a laser) and to a Marlin printer over USB.
        "virtual" | "agent" => {}
        // Moonraker fetches the file from here and prints it.
        "simplyprint" => {}
        _ => return Err("printing on a Prusa over built-in Connect follows".to_owned()),
    }
    if let Some(out) = &machine.out_of_use {
        return Err(if out.reason.is_empty() {
            "out of use".to_owned()
        } else {
            format!("out of use: {}", out.reason)
        });
    }
    if !machine.online {
        return Err("offline".to_owned());
    }
    if machine.on_bed.is_some() {
        return Err("a print is still on the bed: take it off first".to_owned());
    }
    if let Some(job) = &machine.job {
        return Err(format!("busy with {}", job.file_name));
    }
    Ok(warning)
}

/// Whether a label fits a label printer now: the tape loaded (as the owner said) must be
/// as high as the label.
fn label_check(
    file: &StoredFile,
    machine: &Machine,
    profile: Option<&profiles::Profile>,
) -> Result<Option<String>, String> {
    let Some([_, height]) = file.meta.label_dots else {
        return Err("not a label".to_owned());
    };
    let Some(profile) = profile else {
        return Err("choose the printer's profile under Settings".to_owned());
    };
    let Some(media) = machine
        .media
        .as_deref()
        .and_then(|m| profiles::media(profile, m))
    else {
        return Err("say which tape is loaded, under Settings".to_owned());
    };
    let dots = maker_connect_label::dymo::Tape::d1(media.width_mm).map_or(0, |t| t.dots);
    if height as usize > dots {
        return Err(format!(
            "{height} dots high; {} mm tape takes {dots}",
            media.width_mm
        ));
    }
    if !matches!(machine.link.as_str(), "virtual" | "agent") {
        return Err("labels go through an agent".to_owned());
    }
    if !machine.online {
        return Err("offline".to_owned());
    }
    if let Some(problem) = machine.telemetry.as_ref().and_then(|t| t.problem.as_ref()) {
        return Err(problem.clone());
    }
    if let Some(job) = &machine.job {
        return Err(format!("busy with {}", job.file_name));
    }
    Ok(None)
}

/// What a laser's own settings (`$$`, in its telemetry) and its profile say about a file:
/// refused with laser mode off or outside its travel; a warning when the power range differs.
fn laser_check(
    file: &StoredFile,
    machine: &Machine,
    profile: Option<&profiles::Profile>,
) -> Result<Option<String>, String> {
    if machine.kind != maker_connect_protocol::MachineKind::Laser {
        return Ok(None);
    }
    let settings = machine.telemetry.as_ref().and_then(|t| t.grbl.as_ref());
    if settings.and_then(|s| s.laser_mode) == Some(false) {
        return Err(
            "laser mode is off ($32=0): the beam would stay on where the head stops".to_owned(),
        );
    }
    let travel = settings
        .and_then(|s| s.travel_mm)
        .map(|[x, y, _]| (x, y, "the machine's travel"))
        .or_else(|| profile.map(|p| (p.build_volume_mm[0], p.build_volume_mm[1], "its work area")));
    if let (Some((width, depth, what)), Some([x0, y0, x1, y1])) = (travel, file.meta.bounds_mm)
        && (x0 < 0.0 || y0 < 0.0 || x1 > width || y1 > depth)
    {
        return Err(format!("goes outside {what} ({width:.0} × {depth:.0} mm)"));
    }
    match (settings.and_then(|s| s.max_power), file.meta.max_power) {
        (Some(full), Some(asked)) if asked > full => Ok(Some(format!(
            "asks up to S{asked:.0}; full power is S{full:.0}, so it runs at full power from there"
        ))),
        (Some(full), Some(asked)) if full >= 1000.0 && asked <= 255.0 && asked > 0.0 => {
            Ok(Some(format!(
                "asks at most S{asked:.0} of S{full:.0}: made for a 0 to 255 range? It would burn weakly"
            )))
        }
        _ => Ok(None),
    }
}

/// A machine as a file's list shows it: whether the file can go on it.
#[derive(Serialize)]
struct Target {
    id: String,
    name: String,
    ok: bool,
    note: Option<String>,
}

#[derive(Serialize)]
struct Listed {
    #[serde(flatten)]
    file: StoredFile,
    machines: Vec<Target>,
    /// Where it was printed, newest first (a few).
    printed: Vec<crate::store::Printed>,
}

async fn list(State(store): State<Shared>) -> Response {
    let store = store.lock().unwrap();
    let found = store.default_space().and_then(|(space, _)| {
        Ok((
            store.files(space)?,
            store.machines(space)?,
            store.prints(space, 5)?,
        ))
    });
    let (files, machines, mut prints) = match found {
        Ok(found) => found,
        Err(error) => return failed(error),
    };
    let listed: Vec<Listed> = files
        .into_iter()
        .map(|file| {
            let mut targets: Vec<Target> = machines
                .iter()
                .map(|machine| {
                    let (ok, note) = match fit(&file, machine) {
                        Ok(warning) => (true, warning),
                        Err(why) => (false, Some(why)),
                    };
                    Target {
                        id: machine.id.clone(),
                        name: machine.name.clone(),
                        ok,
                        note,
                    }
                })
                .collect();
            targets.sort_by_key(|t| !t.ok);
            Listed {
                printed: prints.remove(&file.id).unwrap_or_default(),
                file,
                machines: targets,
            }
        })
        .collect();
    Json(listed).into_response()
}

#[derive(Deserialize)]
struct UploadQuery {
    name: String,
}

/// A file's name as kept: its last part, plain, at most 120 characters.
fn clean_name(name: &str) -> Option<String> {
    let name = name.rsplit(['/', '\\']).next()?.trim();
    let lower = name.to_ascii_lowercase();
    let known = [".gcode", ".gco", ".g", ".png"];
    if !known.iter().any(|end| lower.ends_with(end)) {
        return None;
    }
    let name: String = name.chars().filter(|c| !c.is_control()).take(120).collect();
    (!name.is_empty()).then_some(name)
}

/// `POST /api/files?name=part.gcode`, the file as the body: streamed to disk, read for its
/// metadata and thumbnail, added to the space.
async fn upload(
    State(store): State<Shared>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> Response {
    let Some(name) = clean_name(&query.name) else {
        return (
            StatusCode::BAD_REQUEST,
            "a G-code file (.gcode), or a label (.png); binary G-code (.bgcode) is not read yet",
        )
            .into_response();
    };
    let (space, id, path, thumbnail_path) = {
        let store = store.lock().unwrap();
        let space = match store.default_space() {
            Ok((space, _)) => space,
            Err(error) => return failed(error),
        };
        let id = crate::store::random_id();
        let path = store.file_path(&id);
        let thumbnail = store.thumbnail_path(&id);
        (space, id, path, thumbnail)
    };
    let partial = path.with_extension("part");
    let written = async {
        let mut file = tokio::fs::File::create(&partial).await?;
        let mut size = 0u64;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(std::io::Error::other)?;
            size += chunk.len() as u64;
            if size > MAX_BYTES {
                return Err(std::io::Error::other("larger than 1 GB"));
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        tokio::fs::rename(&partial, &path).await?;
        Ok::<_, std::io::Error>(size)
    }
    .await;
    let size = match written {
        Ok(size) if size > 0 => size,
        Ok(_) => {
            let _ = tokio::fs::remove_file(&path).await;
            return (StatusCode::BAD_REQUEST, "the file is empty").into_response();
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&partial).await;
            return (StatusCode::BAD_REQUEST, format!("not kept: {error}")).into_response();
        }
    };
    let read_path = path.clone();
    let (meta, png) =
        match tokio::task::spawn_blocking(move || crate::gcode::read(&read_path)).await {
            Ok(Ok(read)) => read,
            _ => Default::default(),
        };
    if let Some(png) = png {
        let _ = tokio::fs::write(&thumbnail_path, png).await;
    }
    match store
        .lock()
        .unwrap()
        .add_file(space, &id, &name, size, &meta)
    {
        Ok(()) => {
            println!("file kept: {name} ({size} bytes)");
            (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
        }
        Err(error) => failed(error),
    }
}

/// A file of the space, or the answer to give when there is none.
fn find(store: &Shared, id: &str) -> Result<(i64, StoredFile, std::path::PathBuf), Box<Response>> {
    let store = store.lock().unwrap();
    let space = store.default_space().map_err(|e| Box::new(failed(e)))?.0;
    match store.file(space, id) {
        Ok(Some(file)) => Ok((space, file, store.files_dir.clone())),
        Ok(None) => Err(Box::new(
            (StatusCode::NOT_FOUND, "no such file").into_response(),
        )),
        Err(error) => Err(Box::new(failed(error))),
    }
}

async fn thumbnail(State(store): State<Shared>, Path(id): Path<String>) -> Response {
    let (_, file, dir) = match find(&store, &id) {
        Ok(found) => found,
        Err(response) => return *response,
    };
    match tokio::fs::read(dir.join(format!("{}.png", file.id))).await {
        Ok(png) => (
            [
                ("content-type", "image/png"),
                ("cache-control", "max-age=86400"),
            ],
            png,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "no thumbnail").into_response(),
    }
}

async fn download(State(store): State<Shared>, Path(id): Path<String>) -> Response {
    let (_, file, dir) = match find(&store, &id) {
        Ok(found) => found,
        Err(response) => return *response,
    };
    serve(&file, &dir).await
}

/// An agent fetches the file of a job given to its machine, with the machine's token (the
/// address is the one in its [`maker_connect_protocol::JobOrder`]). Only that machine's jobs,
/// and only files of its space.
async fn agent_file(
    State(store): State<Shared>,
    headers: HeaderMap,
    Path(job): Path<i64>,
) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    job_file(&store, token, job).await
}

/// The file of job `job`, for the machine whose token is `token` (an agent's, or a Klipper
/// printer's): only that machine's jobs, only files of its space; else 404.
pub(crate) async fn job_file(store: &Shared, token: &str, job: i64) -> Response {
    let found = {
        let store = store.lock().unwrap();
        (|| -> rusqlite::Result<Option<(StoredFile, std::path::PathBuf)>> {
            if token.is_empty() {
                return Ok(None);
            }
            let Some(machine) = store.machine_id_by_token(token)? else {
                return Ok(None);
            };
            let Some(job) = store.job(job)?.filter(|j| j.machine_id == machine) else {
                return Ok(None);
            };
            let space: Option<i64> = store.db.query_row(
                "SELECT space_id FROM machines WHERE id = ?1",
                [&machine],
                |r| r.get(0),
            )?;
            let Some(space) = space else { return Ok(None) };
            Ok(store
                .file(space, &job.file_id)?
                .map(|f| (f, store.files_dir.clone())))
        })()
    };
    match found {
        Ok(Some((file, dir))) => serve(&file, &dir).await,
        Ok(None) => (StatusCode::NOT_FOUND, "no such job for this machine").into_response(),
        Err(error) => failed(error),
    }
}

/// A kept file, streamed.
async fn serve(file: &StoredFile, dir: &std::path::Path) -> Response {
    match tokio::fs::File::open(dir.join(format!("{}.gcode", file.id))).await {
        Ok(handle) => {
            let stream = tokio_util_stream(handle);
            (
                [
                    ("content-type", content_type(&file.name).to_owned()),
                    ("content-length", file.size.to_string()),
                    (
                        "content-disposition",
                        format!("attachment; filename=\"{}\"", file.name.replace('"', "")),
                    ),
                ],
                Body::from_stream(stream),
            )
                .into_response()
        }
        Err(error) => failed(error),
    }
}

/// What a file is, by its name: a label (PNG), else G-code.
fn content_type(name: &str) -> &'static str {
    if name.to_ascii_lowercase().ends_with(".png") {
        "image/png"
    } else {
        "text/x.gcode"
    }
}

/// A file as a stream of chunks, for a download that does not sit in memory.
fn tokio_util_stream(
    file: tokio::fs::File,
) -> impl futures_util::Stream<Item = std::io::Result<Vec<u8>>> {
    futures_util::stream::unfold(file, |mut file| async move {
        use tokio::io::AsyncReadExt;
        let mut buffer = vec![0; 64 * 1024];
        match file.read(&mut buffer).await {
            Ok(0) => None,
            Ok(n) => {
                buffer.truncate(n);
                Some((Ok(buffer), file))
            }
            Err(error) => Some((Err(error), file)),
        }
    })
}

async fn remove(State(store): State<Shared>, Path(id): Path<String>) -> Response {
    let mut store = store.lock().unwrap();
    let removed = store
        .default_space()
        .and_then(|(space, _)| store.remove_file(space, &id));
    match removed {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::CONFLICT,
            "not removed: no such file, or a machine runs it",
        )
            .into_response(),
        Err(error) => failed(error),
    }
}

/// `GET /api/storage`: what the files take, and what the disk under them has left.
async fn storage(State(store): State<Shared>) -> Response {
    let (dir, used, count) = {
        let store = store.lock().unwrap();
        let files = store
            .default_space()
            .and_then(|(space, _)| store.files(space))
            .unwrap_or_default();
        let used: u64 = files.iter().map(|f| f.size).sum();
        (store.files_dir.clone(), used, files.len())
    };
    Json(serde_json::json!({
        "files": count,
        "used_bytes": used,
        "free_bytes": fs2::available_space(&dir).ok(),
        "disk_bytes": fs2::total_space(&dir).ok(),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct Many {
    ids: Vec<String>,
}

/// `POST /api/files/remove` with `{"ids": [...]}`: removes them all, but not one a machine
/// runs or has queued. Says how many went, and which stayed.
async fn remove_many(State(store): State<Shared>, Json(many): Json<Many>) -> Response {
    let mut store = store.lock().unwrap();
    let space = match store.default_space() {
        Ok((space, _)) => space,
        Err(error) => return failed(error),
    };
    let mut removed = 0;
    let mut kept = Vec::new();
    for id in many.ids.iter().take(10_000) {
        match store.remove_file(space, id) {
            Ok(true) => removed += 1,
            Ok(false) => kept.push(id.clone()),
            Err(error) => return failed(error),
        }
    }
    Json(serde_json::json!({ "removed": removed, "kept": kept })).into_response()
}

#[derive(Deserialize)]
struct PrintForm {
    machine: String,
}

/// Puts a file on a machine, when it fits ([`fit`]).
async fn print(
    State(store): State<Shared>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Form(form): Form<PrintForm>,
) -> Response {
    let (space, file, _) = match find(&store, &id) {
        Ok(found) => found,
        Err(response) => return *response,
    };
    let mut store = store.lock().unwrap();
    let machine = match store.machine(space, &form.machine) {
        Ok(Some(machine)) => machine,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => return failed(error),
    };
    if let Err(why) = fit(&file, &machine) {
        return (StatusCode::CONFLICT, why).into_response();
    }
    match store.add_job(space, &machine.id, &file, None) {
        Ok(Ok(_)) if headers.contains_key("x-requested-with") => {
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Ok(_)) => Redirect::to(&format!("/machines/{}#control", machine.id)).into_response(),
        Ok(Err(why)) => (StatusCode::CONFLICT, why).into_response(),
        Err(error) => failed(error),
    }
}

/// Runs a laser file's outline with the beam off (GRBL jogs), to see where it will go.
async fn frame(
    State(store): State<Shared>,
    Path(id): Path<String>,
    Form(form): Form<PrintForm>,
) -> Response {
    let (space, file, dir) = match find(&store, &id) {
        Ok(found) => found,
        Err(response) => return *response,
    };
    // Files kept before the cloud read where moves go: read it now.
    let bounds = file.meta.bounds_mm.or_else(|| {
        let path = dir.join(format!("{}.gcode", file.id));
        let reader = std::io::BufReader::new(std::fs::File::open(path).ok()?);
        crate::gcode::extent(reader).0
    });
    let Some([x0, y0, x1, y1]) = bounds else {
        return (StatusCode::CONFLICT, "the file has no moves to frame").into_response();
    };
    let mut store = store.lock().unwrap();
    let machine = match store.machine(space, &form.machine) {
        Ok(Some(machine)) => machine,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => return failed(error),
    };
    if machine.kind != maker_connect_protocol::MachineKind::Laser {
        return (StatusCode::CONFLICT, "framing is for lasers").into_response();
    }
    if let Err(why) = fit(&file, &machine) {
        return (StatusCode::CONFLICT, why).into_response();
    }
    let corner = |x: f64, y: f64| format!("$J=G90 G21 X{x:.3} Y{y:.3} F3000");
    let gcode = [
        corner(x0, y0),
        corner(x1, y0),
        corner(x1, y1),
        corner(x0, y1),
        corner(x0, y0),
    ]
    .join("\n");
    match store.queue_command(space, &machine.id, &format!("Frame {}", file.name), &gcode) {
        Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) => (StatusCode::CONFLICT, "the machine takes no commands").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct JobForm {
    action: String,
}

/// Pause, resume or stop a machine's job.
async fn control_job(
    State(store): State<Shared>,
    Path(id): Path<String>,
    Form(form): Form<JobForm>,
) -> Response {
    let mut store = store.lock().unwrap();
    // Start an interrupted job again, a little before where it stopped.
    if form.action == "restart" {
        let space = match store.default_space() {
            Ok((space, _)) => space,
            Err(error) => return failed(error),
        };
        let last = match store.resumable_job(&id) {
            Ok(Some(job)) => job,
            Ok(_) => {
                return (StatusCode::CONFLICT, "no interrupted job to start again").into_response();
            }
            Err(error) => return failed(error),
        };
        let file = match store.file(space, &last.file_id) {
            Ok(Some(file)) => file,
            Ok(None) => return (StatusCode::CONFLICT, "its file is gone").into_response(),
            Err(error) => return failed(error),
        };
        return match store.add_job(space, &id, &file, last.restart_line) {
            Ok(Ok(_)) => StatusCode::NO_CONTENT.into_response(),
            Ok(Err(why)) => (StatusCode::CONFLICT, why).into_response(),
            Err(error) => failed(error),
        };
    }
    let job = match store.active_job(&id) {
        Ok(Some(job)) => job,
        Ok(None) => return (StatusCode::NOT_FOUND, "no job on this machine").into_response(),
        Err(error) => return failed(error),
    };
    let next = match (form.action.as_str(), job.state) {
        ("pause", JobState::Printing) => JobState::Paused,
        ("resume", JobState::Paused) => JobState::Printing,
        ("stop", JobState::Queued) => JobState::Stopped,
        ("stop", JobState::Printing | JobState::Paused) => JobState::Stopping,
        _ => return (StatusCode::CONFLICT, "not now").into_response(),
    };
    match store.set_job(job.id, next, None) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

#[cfg(test)]
mod tests {
    use crate::store::Store;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    async fn call(app: &axum::Router, request: Request<Body>) -> (StatusCode, String) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    /// A laser's own settings decide: laser mode off refuses, a file outside its travel is
    /// refused, a file made for a 0 to 255 power range is warned about.
    #[test]
    fn a_laser_checks_its_settings() {
        use maker_connect_protocol::{
            GrblSettings, MachineKind, MachineState, Register, Telemetry,
        };
        let mut store = Store::open(":memory:").unwrap();
        let space = store.default_space().unwrap().0;
        let (id, token, code) = store
            .register(&Register {
                protocol: maker_connect_protocol::VERSION,
                name: "Ortur".into(),
                kind: MachineKind::Laser,
                agent_version: "0.1.0".into(),
            })
            .unwrap();
        store.pair(space, &code).unwrap();
        store
            .set_profile(space, &id, Some("ortur/lm2-pro-s2"))
            .unwrap();
        let file = |bounds: [f64; 4], power: f64| crate::store::StoredFile {
            id: "f".into(),
            name: "sign.gcode".into(),
            size: 1,
            uploaded_at: 0,
            meta: crate::gcode::FileMeta {
                kind: Some(MachineKind::Laser),
                bounds_mm: Some(bounds),
                max_power: Some(power),
                ..Default::default()
            },
        };
        let mut report = |settings: GrblSettings| {
            let telemetry = Telemetry {
                state: MachineState::Idle,
                grbl: Some(settings),
                ..Default::default()
            };
            store.report(&token, &telemetry).unwrap();
            store.machine(space, &id).unwrap().unwrap()
        };
        let good = GrblSettings {
            laser_mode: Some(true),
            max_power: Some(1000.0),
            travel_mm: Some([400.0, 430.0, 0.0]),
        };
        let laser = report(good.clone());
        assert_eq!(
            super::fit(&file([10.0, 10.0, 58.0, 58.0], 500.0), &laser),
            Ok(None)
        );
        assert!(
            super::fit(&file([10.0, 10.0, 500.0, 58.0], 500.0), &laser)
                .unwrap_err()
                .contains("outside")
        );
        assert!(
            super::fit(&file([10.0, 10.0, 58.0, 58.0], 255.0), &laser)
                .unwrap()
                .unwrap()
                .contains("0 to 255")
        );
        let off = report(GrblSettings {
            laser_mode: Some(false),
            ..good
        });
        assert!(
            super::fit(&file([10.0, 10.0, 58.0, 58.0], 500.0), &off)
                .unwrap_err()
                .contains("laser mode is off")
        );
    }

    /// A file is uploaded and read; it goes on a virtual MINI once that is online, not on a
    /// laser, and not twice.
    #[tokio::test]
    async fn a_file_is_uploaded_and_printed() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
        let gcode = "; generated by PrusaSlicer 2.8.1 on today\nM104 S215\nG28\n; estimated printing time (normal mode) = 2m 5s\n; printer_model = MINI\n";
        let (status, body) = call(
            &app,
            Request::post("/api/files?name=../../part.gcode")
                .body(Body::from(gcode))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let id = serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let refused = Request::post("/api/files?name=part.bgcode")
            .body(Body::from("x"))
            .unwrap();
        assert_eq!(call(&app, refused).await.0, StatusCode::BAD_REQUEST);

        let (space, mini, laser) = {
            let mut store = store.lock().unwrap();
            let space = store.default_space().unwrap().0;
            let mini = store
                .add_virtual(space, crate::profiles::find("prusa/mini").unwrap())
                .unwrap();
            let laser = store
                .add_virtual(space, crate::profiles::find("ortur/lm2-pro-s2").unwrap())
                .unwrap();
            (space, mini, laser)
        };
        let (_, list) = call(
            &app,
            Request::get("/api/files").body(Body::empty()).unwrap(),
        )
        .await;
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        assert_eq!(
            list[0]["name"], "part.gcode",
            "the name without its folders"
        );
        assert_eq!(list[0]["meta"]["estimated_s"], 125);
        let note = |id: &str| {
            list[0]["machines"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["id"] == id)
                .unwrap()["note"]
                .clone()
        };
        assert_eq!(note(&mini), "offline");
        assert_eq!(note(&laser), "a file for a 3d printer");

        let print = |machine: &str| {
            Request::post(format!("/files/{id}/print"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .body(Body::from(format!("machine={machine}")))
                .unwrap()
        };
        assert_eq!(
            call(&app, print(&mini)).await.0,
            StatusCode::CONFLICT,
            "offline"
        );
        {
            let mut store = store.lock().unwrap();
            let token = store
                .virtual_machines()
                .unwrap()
                .into_iter()
                .find(|(m, ..)| *m == mini)
                .unwrap()
                .1;
            store.report(&token, &Default::default()).unwrap();
        }
        assert_eq!(call(&app, print(&mini)).await.0, StatusCode::NO_CONTENT);
        assert_eq!(
            call(&app, print(&mini)).await.0,
            StatusCode::CONFLICT,
            "busy"
        );
        let job = store
            .lock()
            .unwrap()
            .machine(space, &mini)
            .unwrap()
            .unwrap()
            .job
            .unwrap();
        assert_eq!(job.file_name, "part.gcode");

        // Listed with where it went; removing many keeps the one a machine has queued.
        let (_, list) = call(
            &app,
            Request::get("/api/files").body(Body::empty()).unwrap(),
        )
        .await;
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        assert_eq!(list[0]["printed"][0]["machine_id"], mini.as_str());
        let (status, body) = call(
            &app,
            Request::post("/api/files/remove")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"ids":["{id}","nothing"]}}"#)))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let result: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(result["removed"], 0);
        assert_eq!(result["kept"][0], id.as_str());
    }

    /// A label (a PNG) is read as one; it goes on a label printer only once the tape loaded
    /// is said and is high enough, and the job tells the agent the tape's width.
    #[tokio::test]
    async fn a_label_is_uploaded_and_printed() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
        let mut label = maker_connect_label::Label::blank(120, 40);
        label.set(3, 3, true);
        let (status, body) = call(
            &app,
            Request::post("/api/files?name=label-test.png")
                .body(Body::from(label.to_png()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let file = serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let (space, dymo, token) = {
            let mut store = store.lock().unwrap();
            let space = store.default_space().unwrap().0;
            let profile = crate::profiles::find("dymo/labelmanager-pnp").unwrap();
            let dymo = store.add_virtual(space, profile).unwrap();
            let token = store.virtual_machines().unwrap()[0].1.clone();
            let telemetry = maker_connect_protocol::Telemetry {
                state: maker_connect_protocol::MachineState::Idle,
                cassette: Some(true),
                ..Default::default()
            };
            store.report(&token, &telemetry).unwrap();
            (space, dymo, token)
        };
        let print = || {
            Request::post(format!("/files/{file}/print"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .body(Body::from(format!("machine={dymo}")))
                .unwrap()
        };
        let (status, why) = call(&app, print()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(why.contains("which tape"), "{why}");
        // 6 mm tape takes 32 dots: too low for a label of 40.
        store
            .lock()
            .unwrap()
            .set_media(space, &dymo, Some("43613"))
            .unwrap();
        let (status, why) = call(&app, print()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(why.contains("32"), "{why}");
        store
            .lock()
            .unwrap()
            .set_media(space, &dymo, Some("45013"))
            .unwrap();
        assert_eq!(call(&app, print()).await.0, StatusCode::NO_CONTENT);
        let mut store = store.lock().unwrap();
        let report = maker_connect_protocol::Telemetry {
            state: maker_connect_protocol::MachineState::Idle,
            ..Default::default()
        };
        let (order, _) = crate::agent_job(&mut store, &token, &report).unwrap();
        let order = order.unwrap();
        assert_eq!(order.tape_mm, Some(12));
        assert_eq!(order.name, "label-test.png");
        let listed = store.file(space, &file).unwrap().unwrap();
        assert_eq!(listed.meta.label_dots, Some([120, 40]));
        assert!(listed.meta.thumbnail);
    }
}
