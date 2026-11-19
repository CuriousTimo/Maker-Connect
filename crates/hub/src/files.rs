//! A space's G-code files, and printing one on a machine. Files are kept on disk beside the
//! database (`<id>.gcode`, with `<id>.png` for a thumbnail found in it); their metadata
//! ([`crate::gcode`]) in the store. Printing makes a job on the machine: virtual machines
//! play it ([`crate::virtual_machines`]), an agent streams it (GRBL lasers, Marlin printers
//! over USB); Prusas on built-in Connect follow.

use crate::store::{JobState, Machine, StoredFile};
use crate::{Store, failed, profiles};
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

pub fn routes() -> Router<Store> {
    Router::new()
        .route(
            "/api/files",
            get(list).post(upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/files/{id}/thumbnail.png", get(thumbnail))
        .route("/files/{id}/download", get(download))
        .route("/api/agent/v1/jobs/{id}/file", get(agent_file))
        .route("/api/agent/v1/files/{id}", get(agent_space_file))
        .route("/files/{id}/remove", post(remove))
        .route("/api/files/remove", post(remove_many))
        .route("/api/storage", get(storage))
        .route("/files/{id}/print", post(print))
        .route("/files/{id}/frame", post(frame))
        .route("/machines/{id}/job", post(control_job))
        .route("/machines/{id}/start-burn", post(start_burn))
}

/// Whether the slicer's printer model is this one: PrusaSlicer writes `MK3S`, OrcaSlicer
/// `Prusa MK3S`; the brand in front, and the case, do not matter.
pub(crate) fn same_model(sliced: &str, known: &str) -> bool {
    let sliced = sliced.trim().to_ascii_lowercase();
    let known = known.trim().to_ascii_lowercase();
    sliced == known || sliced.ends_with(&format!(" {known}"))
}

/// Whether a file can go on a machine now: Ok with a warning, or why not.
pub fn fit(file: &StoredFile, machine: &Machine) -> Result<Option<String>, String> {
    let warning = suits(file, machine)?;
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
    if machine.confirm.is_some() {
        return Err("a plate waits to be confirmed".to_owned());
    }
    if let Some(how) = &machine.queue_held {
        return Err(format!("the last job {how}: is the bed clear?"));
    }
    if let Some(job) = &machine.job {
        return Err(format!("busy with {}", job.file_name));
    }
    Ok(warning)
}

/// Whether a file is for this machine at all, whatever it does now (busy, offline, a print
/// on its bed): then it can wait in the machine's queue (crate::queue).
pub fn suits(file: &StoredFile, machine: &Machine) -> Result<Option<String>, String> {
    let profile = machine.profile.as_deref().and_then(profiles::find);
    // Binary G-code is read by Prusa's own firmware only.
    // A virtual Buddy printer (a MINI, an MK4, a CORE One) takes it as the real one would.
    let virtual_buddy = machine.link == "virtual"
        && profile.and_then(|p| p.firmware.as_deref()) == Some("prusa-buddy");
    if file.name.to_ascii_lowercase().ends_with(".bgcode")
        && machine.link != "prusa-connect"
        && !virtual_buddy
    {
        return Err("binary G-code (.bgcode) goes to a Prusa on its own Connect".to_owned());
    }
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
            Some(models) if models.iter().any(|m| same_model(model, m)) => {}
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
        // A Prusa on its own Connect fetches it onto its USB stick, then prints it
        // (crate::printer_files).
        "prusa-connect" => {}
        _ => return Err("this machine takes no files from here".to_owned()),
    }
    Ok(warning)
}

/// HS2: whether a page fits a LabelWriter now: the PNG must be exactly the loaded roll's page
/// as its port gives it (`ports::geometry::lw450`: width across the head, `cupsWidth` ×
/// `cupsHeight`); the job then tells the agent the roll's length (`label_length_mm`).
fn labelwriter_check(
    file: &StoredFile,
    machine: &Machine,
    profile: Option<&profiles::Profile>,
) -> Result<Option<String>, String> {
    let Some([w, h]) = file.meta.png_dots else {
        return Err("not a label".to_owned());
    };
    labelwriter_fits([w, h], profile, machine.media.as_deref())?;
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

/// HS2: a page of `[w, h]` dots against the roll loaded (`media`, the PPD's choice name).
fn labelwriter_fits(
    [w, h]: [u32; 2],
    profile: Option<&profiles::Profile>,
    media: Option<&str>,
) -> Result<(), String> {
    let Some(media) = media else {
        return Err("say which labels are loaded, on its Label tab".to_owned());
    };
    let Some(page) = profiles::labelwriter_page(profile, media) else {
        return Err("say which labels are loaded, on its Label tab".to_owned());
    };
    if [w, h] != page.dots {
        return Err(format!(
            "this page is {w} × {h} dots; the loaded roll {} takes {} × {}",
            profile
                .and_then(|p| profiles::media(p, media))
                .map_or(media, |m| m.name.as_str()),
            page.dots[0],
            page.dots[1]
        ));
    }
    Ok(())
}

/// Whether a label fits a label printer now: the tape loaded (as the owner said) must be
/// as high as the label.
fn label_check(
    file: &StoredFile,
    machine: &Machine,
    profile: Option<&profiles::Profile>,
) -> Result<Option<String>, String> {
    // HS2: a LabelWriter's page is the port's raster exactly; the PnP below as it was.
    if profile.is_some_and(|p| p.firmware.as_deref() == Some("dymo-lw")) {
        return labelwriter_check(file, machine, profile);
    }
    let Some([length, height]) = file.meta.label_dots else {
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
        return Err("say which tape is loaded, on its Label tab".to_owned());
    };
    let (dots, along) = profiles::media_dots(profile, media);
    if height as usize > dots {
        let what = if along.is_some() { "a label" } else { "tape" };
        return Err(format!(
            "{height} dots high; {} mm {what} takes {dots}",
            media.width_mm
        ));
    }
    if let Some(along) = along
        && length as usize > along
    {
        return Err(format!(
            "{length} dots long; a {} mm label takes {along}",
            media.length_mm.unwrap_or_default()
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
    /// Not now, but into its queue (busy, offline, others waiting).
    queue: bool,
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

async fn list(access: crate::access::Access, State(store): State<Store>) -> Response {
    let grants = match store.grants_of(&access).await {
        Ok(grants) => grants,
        Err(error) => return failed(error),
    };
    let found: Result<_, crate::store::BoxError> = async {
        Ok((
            store.files(access.space).await?,
            store.machines(access.space).await?,
            store.prints(access.space, 5).await?,
        ))
    }
    .await;
    let (files, machines, mut prints) = match found {
        Ok(found) => found,
        Err(error) => return failed(error),
    };
    let store = store.lock().unwrap();
    // Which machines this person may use (by permission, whatever their role): the others say
    // so here, before the button, rather than refusing after it. Granted only in Space.
    let mut may_use = std::collections::HashSet::new();
    for machine in &machines {
        if let Some(true) = store.may(&access, &machine.id, crate::access::Level::Use, &grants) {
            may_use.insert(machine.id.clone());
        }
    }
    let no_permission = if access.role >= crate::access::Role::Admin {
        "you have no permission to use it yet: Space → Members"
    } else {
        "you have no permission to use it: ask the staff"
    };
    let listed: Vec<Listed> = files
        .into_iter()
        .map(|file| {
            let mut targets: Vec<Target> = machines
                .iter()
                .map(|machine| {
                    // HS7b: a laser takes one loaded job and has no queue: loaded here already, or
                    // it replaces what is loaded (the person is asked first).
                    if machine.kind == maker_connect_protocol::MachineKind::Laser {
                        let loaded = machine
                            .job
                            .as_ref()
                            .filter(|j| j.state == crate::store::JobState::Loaded);
                        let mut free = machine.clone();
                        if loaded.is_some() {
                            free.job = None;
                        }
                        let (ok, note) = match (fit(&file, &free), loaded) {
                            (Err(why), _) => (false, Some(why)),
                            (Ok(_), Some(j)) if j.file_id == file.id => (
                                false,
                                Some(
                                    "loaded here: frame it and press Start burn at the laser"
                                        .to_owned(),
                                ),
                            ),
                            (Ok(_), Some(j)) => {
                                (true, Some(format!("replaces {} (asks first)", j.file_name)))
                            }
                            (Ok(warning), None) => (true, warning),
                        };
                        return Target {
                            id: machine.id.clone(),
                            name: machine.name.clone(),
                            ok,
                            queue: false,
                            note,
                        };
                    }
                    let waiting = machine.queue.len();
                    let (ok, queue, note) = match fit(&file, machine) {
                        Ok(warning) if waiting == 0 => (true, false, warning),
                        now => match suits(&file, machine) {
                            Ok(_) => (
                                false,
                                true,
                                Some(match now {
                                    Err(why) => format!("{why}: to its queue"),
                                    Ok(_) => format!("{waiting} waiting: to its queue"),
                                }),
                            ),
                            Err(why) => (false, false, Some(why)),
                        },
                    };
                    Target {
                        id: machine.id.clone(),
                        name: machine.name.clone(),
                        ok,
                        queue,
                        note,
                    }
                })
                .collect();
            for t in &mut targets {
                if !may_use.contains(&t.id) {
                    t.ok = false;
                    t.queue = false;
                    t.note = Some(no_permission.to_owned());
                }
            }
            targets.sort_by_key(|t| (!t.ok, !t.queue));
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
pub(crate) struct UploadQuery {
    pub(crate) name: String,
}

/// A file's name as kept: its last part, plain, at most 120 characters.
fn clean_name(name: &str) -> Option<String> {
    let name = name.rsplit(['/', '\\']).next()?.trim();
    let lower = name.to_ascii_lowercase();
    let known = [".gcode", ".gco", ".g", ".bgcode", ".png"];
    if !known.iter().any(|end| lower.ends_with(end)) {
        return None;
    }
    let name: String = name.chars().filter(|c| !c.is_control()).take(120).collect();
    (!name.is_empty()).then_some(name)
}

/// `POST /api/files?name=part.gcode`, the file as the body: streamed to disk, read for its
/// metadata and thumbnail, added to the space.
pub(crate) async fn upload(
    access: crate::access::Access,
    State(store): State<Store>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> Response {
    let Some(name) = clean_name(&query.name) else {
        return (
            StatusCode::BAD_REQUEST,
            "a G-code file (.gcode, .bgcode), or a label (.png)",
        )
            .into_response();
    };
    let (space, id, path, thumbnail_path) = {
        let store = store.lock().unwrap();
        let space = access.space;
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
    let (mut meta, png): (crate::gcode::FileMeta, Option<Vec<u8>>) =
        match tokio::task::spawn_blocking(move || crate::gcode::read(&read_path)).await {
            Ok(Ok(read)) => read,
            _ => Default::default(),
        };
    // What its name tells, where its contents did not (PrusaSlicer's naming).
    if meta.kind != Some(maker_connect_protocol::MachineKind::Laser) {
        crate::gcode::from_name(&mut meta, &name);
    }
    if let Some(png) = png {
        let _ = tokio::fs::write(&thumbnail_path, png).await;
    }
    match store.add_file(space, &id, &name, size, &meta).await {
        Ok(()) => {
            println!("file kept: {name} ({size} bytes)");
            (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
        }
        Err(error) => failed(error),
    }
}

/// A file of the space, or the answer to give when there is none.
async fn find(
    access: &crate::access::Access,
    store: &Store,
    id: &str,
) -> Result<(i64, StoredFile, std::path::PathBuf), Box<Response>> {
    let space = access.space;
    match store.file(space, id).await {
        Ok(Some(file)) => Ok((space, file, store.files_dir().to_owned())),
        Ok(None) => Err(Box::new(
            (StatusCode::NOT_FOUND, "no such file").into_response(),
        )),
        Err(error) => Err(Box::new(failed(error))),
    }
}

async fn thumbnail(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<String>,
) -> Response {
    let (_, file, dir) = match find(&access, &store, &id).await {
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

async fn download(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<String>,
) -> Response {
    let (_, file, dir) = match find(&access, &store, &id).await {
        Ok(found) => found,
        Err(response) => return *response,
    };
    serve(&file, &dir).await
}

/// An agent fetches the file of a job given to its machine, with the machine's token (the
/// address is the one in its [`maker_connect_protocol::JobOrder`]). Only that machine's jobs,
/// and only files of its space.
async fn agent_file(
    State(store): State<Store>,
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

/// An agent fetches a file of its machine's space that is not a job's (a laser's frame,
/// `Action::Frame`), with the machine's token: only files of that space.
async fn agent_space_file(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let found: Result<Option<(StoredFile, std::path::PathBuf)>, crate::store::BoxError> = async {
        if token.is_empty() {
            return Ok(None);
        }
        let Some((_, space)) = store.placed(token).await? else {
            return Ok(None);
        };
        Ok(store
            .file(space, &id)
            .await?
            .map(|f| (f, store.files_dir().to_owned())))
    }
    .await;
    match found {
        Ok(Some((file, dir))) => serve(&file, &dir).await,
        Ok(None) => (StatusCode::NOT_FOUND, "no such file for this machine").into_response(),
        Err(error) => failed(error),
    }
}

/// The file of job `job`, for the machine whose token is `token` (an agent's, or a Klipper
/// printer's): only that machine's jobs, only files of its space; else 404.
pub(crate) async fn job_file(store: &Store, token: &str, job: i64) -> Response {
    let found: Result<Option<(StoredFile, std::path::PathBuf)>, crate::store::BoxError> = async {
        if token.is_empty() {
            return Ok(None);
        }
        let Some((machine, space)) = store.placed(token).await? else {
            return Ok(None);
        };
        let job = store.job(space, job).await?;
        let Some(job) = job.filter(|j| j.machine_id == machine) else {
            return Ok(None);
        };
        Ok(store
            .file(space, &job.file_id)
            .await?
            .map(|f| (f, store.files_dir().to_owned())))
    }
    .await;
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
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".bgcode") {
        "application/octet-stream"
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

async fn remove(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<String>,
) -> Response {
    let removed = store.remove_file(access.space, &id).await;
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
async fn storage(access: crate::access::Access, State(store): State<Store>) -> Response {
    let (dir, used, count) = {
        let files = store.files(access.space).await.unwrap_or_default();
        let used: u64 = files.iter().map(|f| f.size).sum();
        (store.files_dir().to_owned(), used, files.len())
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
async fn remove_many(
    access: crate::access::Access,
    State(store): State<Store>,
    Json(many): Json<Many>,
) -> Response {
    let space = access.space;
    let mut removed = 0;
    let mut kept = Vec::new();
    for id in many.ids.iter().take(10_000) {
        match store.remove_file(space, id).await {
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
    /// How many times: the first now (when the machine is free), the rest in its queue.
    copies: Option<u32>,
    /// HS7b: a laser with a loaded job: take it off and load this one instead (the person
    /// confirmed).
    replace: Option<bool>,
}

/// Copies at most, in one go.
const MAX_COPIES: u32 = 50;

/// Puts a file on a machine, when it fits ([`fit`]).
async fn print(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Form(form): Form<PrintForm>,
) -> Response {
    let (space, file, _) = match find(&access, &store, &id).await {
        Ok(found) => found,
        Err(response) => return *response,
    };
    let ppe_checks = store.ppe_rule(access.space, &form.machine).await;
    let grants = match store.grants_of(&access).await {
        Ok(grants) => grants,
        Err(error) => return failed(error),
    };
    let found_machine = store.machine(access.space, &form.machine).await;
    {
        let store = store.lock().unwrap();
        // Running a job, or framing one, is using the machine.
        if let Err(refused) = crate::access::check_machine(
            &store,
            &access,
            &form.machine,
            crate::access::Level::Use,
            &grants,
        ) {
            return *refused;
        }
        if let Err(refused) = crate::safety::check_ppe(&store, &ppe_checks, &access, &form.machine)
        {
            return *refused;
        }
    }
    let machine = match found_machine {
        Ok(Some(machine)) => machine,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => return failed(error),
    };
    // HS7b (owner, 2026-11-17): a laser takes one loaded job, no queue and no copies; it is
    // loaded, not started (the person at the laser frames, sets the origin, presses Start burn).
    if machine.kind == maker_connect_protocol::MachineKind::Laser {
        if form.copies.unwrap_or(1) > 1 {
            return (
                StatusCode::CONFLICT,
                "a laser takes one job at a time: no copies",
            )
                .into_response();
        }
        // its loaded job is the one asked about below, not "busy"
        let mut free = machine.clone();
        if free
            .job
            .as_ref()
            .is_some_and(|j| j.state == crate::store::JobState::Loaded)
        {
            free.job = None;
        }
        if let Err(why) = fit(&file, &free) {
            return (StatusCode::CONFLICT, why).into_response();
        }
        match store.active_job(space, &machine.id).await {
            Ok(Some(job)) if job.state == crate::store::JobState::Loaded => {
                // Only who may load one replaces it (a person at a session,
                // crate::store::LASER_NEVER_ALONE): refused before it is asked, nothing removed.
                if !crate::store::StartedBy::person(&access).is_person_at_a_session() {
                    return (StatusCode::CONFLICT, crate::store::LASER_NEVER_ALONE).into_response();
                }
                if form.replace != Some(true) {
                    return (
                        StatusCode::CONFLICT,
                        Json(serde_json::json!({
                            "loaded": job.file_name,
                            "confirm": format!("{} is loaded on this laser: replace it with {}?", job.file_name, file.name),
                        })),
                    )
                        .into_response();
                }
                if let Err(error) = store.unload_laser_job(space, &machine.id).await {
                    return failed(error);
                }
            }
            Ok(Some(job)) => {
                return (StatusCode::CONFLICT, format!("busy with {}", job.file_name))
                    .into_response();
            }
            Ok(None) => {}
            Err(error) => return failed(error),
        }
        return match store
            .add_job(
                space,
                &machine.id,
                &file,
                None,
                crate::store::StartedBy::person(&access),
            )
            .await
        {
            Ok(Ok(_)) if headers.contains_key("x-requested-with") => {
                Json(serde_json::json!({ "loaded": file.name })).into_response()
            }
            Ok(Ok(_)) => Redirect::to(&format!("/machines/{}#control", machine.id)).into_response(),
            // removed meanwhile: what `find` answers
            Ok(Err(why)) if why == crate::store::NO_SUCH_FILE => {
                (StatusCode::NOT_FOUND, why).into_response()
            }
            Ok(Err(why)) => (StatusCode::CONFLICT, why).into_response(),
            Err(error) => failed(error),
        };
    }
    // Free, and nothing waiting before it: now. Busy, or others waiting: to the back of its
    // queue (crate::queue). Not for this machine at all: no.
    let copies = form.copies.unwrap_or(1).clamp(1, MAX_COPIES);
    let by = Some((access.user_id, access.actor.as_str()));
    // Copies after the first wait in the queue, one after another.
    let queue_rest = async |store: &Store, n: u32| -> sqlx::Result<usize> {
        let mut w = store.work(space).await?;
        let mut place = 0;
        for _ in 0..n {
            place = w.queue_add(&machine.id, &file, by).await?;
        }
        w.commit(store).await?;
        Ok(place)
    };
    if !machine.queue.is_empty() || fit(&file, &machine).is_err() {
        if let Err(why) = suits(&file, &machine) {
            return (StatusCode::CONFLICT, why).into_response();
        }
        return match queue_rest(&store, copies).await {
            Ok(place) if headers.contains_key("x-requested-with") => (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "queued": place, "copies": copies })),
            )
                .into_response(),
            Ok(_) => Redirect::to(&format!("/machines/{}#queue", machine.id)).into_response(),
            Err(error) => failed(error),
        };
    }
    match store
        .add_job(
            space,
            &machine.id,
            &file,
            None,
            crate::store::StartedBy::person(&access),
        )
        .await
    {
        Ok(Ok(_)) if copies > 1 => match queue_rest(&store, copies - 1).await {
            Ok(_) if headers.contains_key("x-requested-with") => Json(serde_json::json!({
                "started": true,
                "copies": copies,
            }))
            .into_response(),
            Ok(_) => Redirect::to(&format!("/machines/{}#queue", machine.id)).into_response(),
            Err(error) => failed(error),
        },
        Ok(Ok(_)) if headers.contains_key("x-requested-with") => {
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Ok(_)) => Redirect::to(&format!("/machines/{}#control", machine.id)).into_response(),
        // removed meanwhile: what `find` answers
        Ok(Err(why)) if why == crate::store::NO_SUCH_FILE => {
            (StatusCode::NOT_FOUND, why).into_response()
        }
        Ok(Err(why)) => (StatusCode::CONFLICT, why).into_response(),
        Err(error) => failed(error),
    }
}

/// Runs a laser file's outline with the beam off (GRBL jogs), to see where it will go.
async fn frame(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<String>,
    Form(form): Form<PrintForm>,
) -> Response {
    let (_, file, dir) = match find(&access, &store, &id).await {
        Ok(found) => found,
        Err(response) => return *response,
    };
    // Files kept before the hub read where moves go: read it now.
    let bounds = file.meta.bounds_mm.or_else(|| {
        let path = dir.join(format!("{}.gcode", file.id));
        let reader = std::io::BufReader::new(std::fs::File::open(path).ok()?);
        crate::gcode::extent(reader).0
    });
    let Some([x0, y0, x1, y1]) = bounds else {
        return (StatusCode::CONFLICT, "the file has no moves to frame").into_response();
    };
    let ppe_checks = store.ppe_rule(access.space, &form.machine).await;
    let grants = match store.grants_of(&access).await {
        Ok(grants) => grants,
        Err(error) => return failed(error),
    };
    let found_machine = store.machine(access.space, &form.machine).await;
    let mut store = store.lock().unwrap();
    // Running a job, or framing one, is using the machine.
    if let Err(refused) = crate::access::check_machine(
        &store,
        &access,
        &form.machine,
        crate::access::Level::Use,
        &grants,
    ) {
        return *refused;
    }
    if let Err(refused) = crate::safety::check_ppe(&store, &ppe_checks, &access, &form.machine) {
        return *refused;
    }
    let machine = match found_machine {
        Ok(Some(machine)) => machine,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => return failed(error),
    };
    if machine.kind != maker_connect_protocol::MachineKind::Laser {
        return (StatusCode::CONFLICT, "framing is for lasers").into_response();
    }
    // HS7b: the loaded job is framed on its laser; it does not make the laser busy for its own frame.
    let mut machine = machine;
    if machine
        .job
        .as_ref()
        .is_some_and(|j| j.state == crate::store::JobState::Loaded && j.file_id == file.id)
    {
        machine.job = None;
    }
    if let Err(why) = fit(&file, &machine) {
        return (StatusCode::CONFLICT, why).into_response();
    }
    // A laser behind an agent: framed by its port (the hub makes no G-code for it).
    if machine.link == "agent" {
        let action = maker_connect_protocol::Action::Frame {
            download: format!("/api/agent/v1/files/{}", file.id),
        };
        return match store.queue_action(
            Some(machine.link.as_str()),
            &machine.id,
            &format!("Frame {}", file.name),
            &action,
        ) {
            Some(_) => StatusCode::NO_CONTENT.into_response(),
            None => (StatusCode::CONFLICT, "the machine takes no commands").into_response(),
        };
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
    match store.queue_command(
        Some(machine.link.as_str()),
        &machine.id,
        &format!("Frame {}", file.name),
        &gcode,
    ) {
        Some(_) => StatusCode::NO_CONTENT.into_response(),
        None => (StatusCode::CONFLICT, "the machine takes no commands").into_response(),
    }
}

#[derive(Deserialize)]
struct StartBurnForm {
    /// The person confirmed the job's place on the bed is not checked (after a Set origin).
    confirm: Option<bool>,
}

/// HS7b: the confirm of every Start burn (ours).
pub(crate) fn start_confirm(file: &str) -> String {
    format!("Start burn: the laser fires on {file}. Framed, in place, glasses on?")
}

/// HS7b: the confirm after a Set origin (ours, after LaserFoundry's own: `ui-egui` `lib.rs:1250`).
pub(crate) const ORIGIN_CONFIRM: &str = "An origin was set on this laser: the job's place on the bed is not checked by the hub. Framed and in place?";

/// HS7b: Start burn, on the laser's machine page: the loaded job goes to the laser. Only a person at
/// the laser (operate, PPE, a session; never an API token), only when the job is loaded and was framed
/// completely after the last Set origin (owner, 2026-11-17), the laser idle with laser mode on (`$32`,
/// as LaserFoundry refuses otherwise), and after a Set origin with the person's confirm.
async fn start_burn(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<String>,
    Form(form): Form<StartBurnForm>,
) -> Response {
    let ppe_checks = store.ppe_rule(access.space, &id).await;
    let grants = match store.grants_of(&access).await {
        Ok(grants) => grants,
        Err(error) => return failed(error),
    };
    let found_machine = store.machine(access.space, &id).await;
    let active = store.active_job(access.space, &id).await;
    // The checks, with the store held only this long (a refusal is the response itself).
    #[allow(clippy::result_large_err)]
    let checked = (|| -> Result<(crate::store::Machine, crate::store::Job), Response> {
        let store = store.lock().unwrap();
        if let Err(refused) = crate::access::check_machine(
            &store,
            &access,
            &id,
            crate::access::Level::Operate,
            &grants,
        ) {
            return Err(*refused);
        }
        if let Err(refused) = crate::safety::check_ppe(&store, &ppe_checks, &access, &id) {
            return Err(*refused);
        }
        if !crate::store::StartedBy::person(&access).is_person_at_a_session() {
            return Err((StatusCode::FORBIDDEN, crate::store::LASER_NEVER_ALONE).into_response());
        }
        let machine = match found_machine {
            Ok(Some(machine)) => machine,
            Ok(None) => return Err((StatusCode::NOT_FOUND, "no such machine").into_response()),
            Err(error) => return Err(failed(error)),
        };
        if machine.kind != maker_connect_protocol::MachineKind::Laser {
            return Err((StatusCode::CONFLICT, "Start burn is for lasers").into_response());
        }
        let job = match active {
            Ok(Some(job)) if job.state == crate::store::JobState::Loaded => job,
            Ok(Some(job)) => {
                return Err(
                    (StatusCode::CONFLICT, format!("busy with {}", job.file_name)).into_response(),
                );
            }
            Ok(None) => {
                return Err(
                    (StatusCode::CONFLICT, "no job is loaded on this laser").into_response()
                );
            }
            Err(error) => return Err(failed(error)),
        };
        let telemetry = machine.telemetry.as_ref().filter(|_| machine.online);
        let Some(t) = telemetry else {
            return Err((StatusCode::CONFLICT, "offline").into_response());
        };
        if t.state != maker_connect_protocol::MachineState::Idle {
            return Err((StatusCode::CONFLICT, "the laser is not idle").into_response());
        }
        if let Some(problem) = &t.problem {
            return Err((StatusCode::CONFLICT, problem.clone()).into_response());
        }
        // LaserFoundry's own refusal and text (`ui-egui` `lib.rs:1240-1247`)
        if t.grbl.as_ref().and_then(|g| g.laser_mode) != Some(true) {
            return Err((
                StatusCode::CONFLICT,
                "Laser mode ($32) is off on this controller: turn it on in the machine settings before running a job",
            )
                .into_response());
        }
        if store.laser_framed_file(&machine.id) != Some(job.file_id.as_str()) {
            return Err((
                StatusCode::CONFLICT,
                "frame the loaded job first (after a Set origin it is framed again)",
            )
                .into_response());
        }
        // The person confirms every Start burn (ours); after a Set origin the confirm says the hub does not
        // check the job's place on the bed (LaserFoundry's own confirm, `ui-egui` `lib.rs:1250`). After a hub
        // restart the hub no longer knows whether an origin was set: the confirm is asked all the same.
        if form.confirm != Some(true) {
            let text = if store.laser_origin_was_set(&machine.id) {
                ORIGIN_CONFIRM.to_owned()
            } else {
                start_confirm(&job.file_name)
            };
            return Err((
                StatusCode::CONFLICT,
                Json(serde_json::json!({ "confirm": text })),
            )
                .into_response());
        }
        Ok((machine, job))
    })();
    let (machine, job) = match checked {
        Ok(checked) => checked,
        Err(refused) => return refused,
    };
    match store
        .start_burn(
            access.space,
            &machine.id,
            job.id,
            Some(access.actor.as_str()),
        )
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::CONFLICT, "the loaded job changed: look again").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct JobForm {
    action: String,
}

/// HS6: the job route's Stop, for the emergency-stop button of a laser (LaserFoundry's Stop).
pub(crate) async fn stop_job(
    access: crate::access::Access,
    state: State<Store>,
    id: Path<String>,
) -> Response {
    control_job(
        access,
        state,
        id,
        Form(JobForm {
            action: "stop".to_owned(),
        }),
    )
    .await
}

/// Pause, resume or stop a machine's job.
async fn control_job(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<String>,
    Form(form): Form<JobForm>,
) -> Response {
    let ppe_checks = store.ppe_rule(access.space, &id).await;
    let found_machine = store
        .machine(access.space, &id)
        .await
        .map_err(|e| e.to_string());
    // Starting and resuming wait for PPE; pausing and stopping never do.
    if matches!(form.action.as_str(), "restart" | "resume")
        && let Err(refused) =
            crate::safety::check_ppe(&store.lock().unwrap(), &ppe_checks, &access, &id)
    {
        return *refused;
    }
    // Start an interrupted job again, a little before where it stopped.
    if form.action == "restart" {
        let space = access.space;
        let last = match store.resumable_job(space, &id).await {
            Ok(Some(job)) => job,
            Ok(_) => {
                return (StatusCode::CONFLICT, "no interrupted job to start again").into_response();
            }
            Err(error) => return failed(error),
        };
        let file = match store.file(space, &last.file_id).await {
            Ok(Some(file)) => file,
            Ok(None) => return (StatusCode::CONFLICT, "its file is gone").into_response(),
            Err(error) => return failed(error),
        };
        return match store
            .add_job(
                space,
                &id,
                &file,
                last.restart_line,
                crate::store::StartedBy::person(&access),
            )
            .await
        {
            Ok(Ok(_)) => StatusCode::NO_CONTENT.into_response(),
            // removed meanwhile: what the lookup above answers
            Ok(Err(crate::store::NO_SUCH_FILE)) => {
                (StatusCode::CONFLICT, "its file is gone").into_response()
            }
            Ok(Err(why)) => (StatusCode::CONFLICT, why).into_response(),
            Err(error) => failed(error),
        };
    }
    let job = match store.active_job(access.space, &id).await {
        Ok(Some(job)) => job,
        Ok(None) => {
            // A print it started itself (from its SD card, or a Prusa's USB stick at its
            // screen): no job of the hub's, so the printer is told directly.
            let own = match found_machine.clone() {
                Ok(Some(m))
                    if m.online && m.telemetry.as_ref().is_some_and(|t| t.job.is_some()) =>
                {
                    m
                }
                // A laser's Stop (a soft reset) is offered with no job too.
                Ok(Some(m))
                    if m.online
                        && m.kind == maker_connect_protocol::MachineKind::Laser
                        && form.action == "stop" =>
                {
                    m
                }
                Ok(_) => return (StatusCode::NOT_FOUND, "no job on this machine").into_response(),
                Err(error) => return failed(error),
            };
            if own.link == "prusa-connect" {
                crate::printer_files::job_control(
                    &mut store.lock().unwrap(),
                    Some(own.link.as_str()),
                    &id,
                    &form.action,
                );
                return StatusCode::NO_CONTENT.into_response();
            }
            let control = match form.action.as_str() {
                "pause" => maker_connect_protocol::JobControl::Pause,
                "resume" => maker_connect_protocol::JobControl::Resume,
                "stop" => maker_connect_protocol::JobControl::Stop,
                _ => return (StatusCode::CONFLICT, "not now").into_response(),
            };
            store.lock().unwrap().own_job_control.insert(id, control);
            return StatusCode::NO_CONTENT.into_response();
        }
        Err(error) => return failed(error),
    };
    // HS7b: a loaded laser job never ran. "unload" takes it off the laser; Stop stays the
    // laser's own Stop (a soft reset), as with no job, so a moving head (a frame) is stopped.
    if job.state == JobState::Loaded {
        return match form.action.as_str() {
            "unload" => match store.unload_laser_job(access.space, &id).await {
                Ok(_) => StatusCode::NO_CONTENT.into_response(),
                Err(error) => failed(error),
            },
            "stop" => {
                store
                    .lock()
                    .unwrap()
                    .own_job_control
                    .insert(id, maker_connect_protocol::JobControl::Stop);
                StatusCode::NO_CONTENT.into_response()
            }
            _ => (
                StatusCode::CONFLICT,
                "not now: the job is loaded, not started",
            )
                .into_response(),
        };
    }
    let next = match (form.action.as_str(), job.state) {
        ("pause", JobState::Printing) => JobState::Paused,
        ("resume", JobState::Paused) => JobState::Printing,
        ("stop", JobState::Queued) => JobState::Stopped,
        ("stop", JobState::Printing | JobState::Paused) => JobState::Stopping,
        _ => return (StatusCode::CONFLICT, "not now").into_response(),
    };
    // A Prusa on its built-in Connect is told so itself.
    if let Ok(Some(machine)) = &found_machine
        && machine.link == "prusa-connect"
    {
        crate::printer_files::job_control(
            &mut store.lock().unwrap(),
            Some(machine.link.as_str()),
            &id,
            &form.action,
        );
    }
    match store.set_job(access.space, job.id, next, None).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;

    use tower::ServiceExt;

    async fn call(app: &axum::Router, request: Request<Body>) -> (StatusCode, String) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    /// A laser's own settings decide: laser mode off refuses, a file outside its travel is
    /// refused, a file made for a 0 to 255 power range is warned about.
    /// PrusaSlicer writes `MK3S`, OrcaSlicer `Prusa MK3S`: both are the MK3S, not the MINI.
    #[test]
    fn a_printer_model_by_either_slicer() {
        assert!(super::same_model("MK3S", "MK3S"));
        assert!(super::same_model("Prusa MK3S", "MK3S"));
        assert!(super::same_model("prusa mini", "MINI"));
        assert!(!super::same_model("Prusa MK3S", "MINI"));
        assert!(
            !super::same_model("MK3S+", "MK3S"),
            "not a model that only starts alike"
        );
    }

    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_laser_checks_its_settings(pool: sqlx::PgPool) {
        use maker_connect_protocol::{
            GrblSettings, MachineKind, MachineState, Register, Telemetry,
        };
        let store = crate::store::Store::for_test(&pool).await;
        let space = store.default_space().await.unwrap().0;
        let (id, token, code) = store
            .register(&Register {
                protocol: maker_connect_protocol::VERSION,
                name: "Ortur".into(),
                kind: MachineKind::Laser,
                agent_version: "0.1.0".into(),
            })
            .await
            .unwrap();
        store.pair(space, &code).await.unwrap();
        store
            .set_profile(space, &id, Some("ortur/lm2-pro-s2"))
            .await
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
        let report = |settings: GrblSettings| {
            let (store, token, id) = (&store, &token, &id);
            async move {
                let telemetry = Telemetry {
                    state: MachineState::Idle,
                    grbl: Some(settings),
                    ..Default::default()
                };
                store.report(token, &telemetry).await.unwrap();
                store.machine(space, id).await.unwrap().unwrap()
            }
        };
        let good = GrblSettings {
            laser_mode: Some(true),
            max_power: Some(1000.0),
            travel_mm: Some([400.0, 430.0, 0.0]),
        };
        let laser = report(good.clone()).await;
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
        })
        .await;
        assert!(
            super::fit(&file([10.0, 10.0, 58.0, 58.0], 500.0), &off)
                .unwrap_err()
                .contains("laser mode is off")
        );
    }

    /// A large file arrives whole, through every layer of the site (the request log once
    /// held bodies up to 1 MB and passed on an empty one past that).
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_large_file_is_uploaded_whole(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let mut gcode = String::from("; printer_model = MINI\n");
        while gcode.len() < 3_000_000 {
            gcode += "G1 X10.123 Y20.456 E0.0123 F1800\n";
        }
        let size = gcode.len();
        let (status, body) = call(
            &app,
            Request::post("/api/files?name=large.gcode")
                .body(Body::from(gcode))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let id = serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let path = store.file_path(&id);
        assert_eq!(std::fs::metadata(path).unwrap().len(), size as u64);
    }

    /// A file is uploaded and read; it goes on a virtual MINI once that is online, not on a
    /// laser, and not twice.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_file_is_uploaded_and_printed(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
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
        let refused = Request::post("/api/files?name=part.stl")
            .body(Body::from("x"))
            .unwrap();
        assert_eq!(call(&app, refused).await.0, StatusCode::BAD_REQUEST);

        let (space, mini, laser) = {
            let space = store.default_space().await.unwrap().0;
            let mini = store
                .add_virtual(space, crate::profiles::find("prusa/mini").unwrap())
                .await
                .unwrap();
            let laser = store
                .add_virtual(space, crate::profiles::find("ortur/lm2-pro-s2").unwrap())
                .await
                .unwrap();
            (space, mini, laser)
        };
        crate::auth::tester_may_operate(&store, &mini).await;
        crate::auth::tester_may_operate(&store, &laser).await;
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
        assert_eq!(note(&mini), "offline: to its queue");
        assert_eq!(note(&laser), "a file for a 3d printer");

        let print = |machine: &str| {
            Request::post(format!("/files/{id}/print"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .body(Body::from(format!("machine={machine}")))
                .unwrap()
        };
        // Offline: into its queue (crate::queue), not refused.
        assert_eq!(
            call(&app, print(&mini)).await.0,
            StatusCode::ACCEPTED,
            "offline: queued"
        );
        assert_eq!(
            call(&app, print(&laser)).await.0,
            StatusCode::CONFLICT,
            "not for a laser, not even its queue"
        );
        {
            let token = store
                .virtual_machines()
                .await
                .unwrap()
                .into_iter()
                .find(|(m, ..)| *m == mini)
                .unwrap()
                .1;
            store.report(&token, &Default::default()).await.unwrap();
            // Online and free: the queue starts it by itself.
            assert_eq!(store.start_queued().await.unwrap(), 1);
            assert!(
                store
                    .machine(space, &mini)
                    .await
                    .unwrap()
                    .unwrap()
                    .queue
                    .is_empty()
            );
        }
        // Busy: the next goes to its queue, and waits there.
        assert_eq!(
            call(&app, print(&mini)).await.0,
            StatusCode::ACCEPTED,
            "busy: queued"
        );
        assert_eq!(store.start_queued().await.unwrap(), 0, "waits while busy");
        assert_eq!(
            store
                .machine(space, &mini)
                .await
                .unwrap()
                .unwrap()
                .queue
                .len(),
            1
        );
        // Copies: two more, one after another in its queue.
        let copies = Request::post(format!("/files/{id}/print"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-requested-with", "fetch")
            .body(Body::from(format!("machine={mini}&copies=2")))
            .unwrap();
        let (status, body) = call(&app, copies).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["copies"],
            2
        );
        assert_eq!(
            store
                .machine(space, &mini)
                .await
                .unwrap()
                .unwrap()
                .queue
                .len(),
            3
        );
        let job = store
            .machine(space, &mini)
            .await
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
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_label_is_uploaded_and_printed(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
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
            let space = store.default_space().await.unwrap().0;
            let profile = crate::profiles::find("dymo/labelmanager-pnp").unwrap();
            let dymo = store.add_virtual(space, profile).await.unwrap();
            let token = store.virtual_machines().await.unwrap()[0].1.clone();
            let telemetry = maker_connect_protocol::Telemetry {
                state: maker_connect_protocol::MachineState::Idle,
                cassette: Some(true),
                ..Default::default()
            };
            store.report(&token, &telemetry).await.unwrap();
            (space, dymo, token)
        };
        crate::auth::tester_may_operate(&store, &dymo).await;
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
        store.set_media(space, &dymo, Some("43613")).await.unwrap();
        let (status, why) = call(&app, print()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(why.contains("32"), "{why}");
        store.set_media(space, &dymo, Some("45013")).await.unwrap();
        assert_eq!(call(&app, print()).await.0, StatusCode::NO_CONTENT);
        let report = maker_connect_protocol::Telemetry {
            state: maker_connect_protocol::MachineState::Idle,
            ..Default::default()
        };
        let of = crate::agent_of(&store, &token).await.unwrap();
        let (order, _) = crate::agent_job(&store, &of, &report).await.unwrap();
        let order = order.unwrap();
        assert_eq!(order.tape_mm, Some(12));
        assert_eq!(order.name, "label-test.png");
        let listed = store.file(space, &file).await.unwrap().unwrap();
        assert_eq!(listed.meta.label_dots, Some([120, 40]));
        assert!(listed.meta.thumbnail);
    }

    /// HS2: a LabelWriter takes a page of exactly its loaded roll's raster (the port's), and
    /// the job tells the agent the roll's length, so the printer finds the next gap.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_labelwriter_page_is_its_rolls_raster(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        // S0722400, DYMO's European Large Address roll (DYMO Connect's catalogue): its page and raster from the port.
        let page = crate::profiles::labelwriter_page(
            crate::profiles::find("dymo/labelwriter-450"),
            "S0722400",
        )
        .unwrap();
        let [pw, ph] = page.dots;
        let (pw, ph) = (pw as usize, ph as usize);
        let upload = |w: usize, h: usize| {
            let app = app.clone();
            async move {
                // `Label::blank(length, height)` writes a PNG of `length` × `height` pixels
                let label = maker_connect_label::Label::blank(w, h);
                let (status, body) = call(
                    &app,
                    Request::post("/api/files?name=address.png")
                        .body(Body::from(label.to_png()))
                        .unwrap(),
                )
                .await;
                assert_eq!(status, StatusCode::CREATED, "{body}");
                serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            }
        };
        let (fits, too_wide, too_long) = (
            upload(pw, ph).await,
            upload(pw + 1, ph).await,
            upload(pw, ph + 1).await,
        );
        let (writer, token) = {
            let space = store.default_space().await.unwrap().0;
            let profile = crate::profiles::find("dymo/labelwriter-450").unwrap();
            let writer = store.add_virtual(space, profile).await.unwrap();
            let token = store.virtual_machines().await.unwrap()[0].1.clone();
            let idle = maker_connect_protocol::Telemetry {
                state: maker_connect_protocol::MachineState::Idle,
                ..Default::default()
            };
            store.report(&token, &idle).await.unwrap();
            store
                .set_media(space, &writer, Some("S0722400"))
                .await
                .unwrap();
            (writer, token)
        };
        crate::auth::tester_may_operate(&store, &writer).await;
        let print = |file: &str| {
            Request::post(format!("/files/{file}/print"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .body(Body::from(format!("machine={writer}")))
                .unwrap()
        };
        for file in [&too_wide, &too_long] {
            let (status, why) = call(&app, print(file)).await;
            assert_eq!(status, StatusCode::CONFLICT);
            assert!(why.contains(&format!("takes {pw} × {ph}")), "{why}");
        }
        assert_eq!(call(&app, print(&fits)).await.0, StatusCode::NO_CONTENT);
        let report = maker_connect_protocol::Telemetry {
            state: maker_connect_protocol::MachineState::Idle,
            ..Default::default()
        };
        let of = crate::agent_of(&store, &token).await.unwrap();
        let order = crate::agent_job(&store, &of, &report)
            .await
            .unwrap()
            .0
            .unwrap();
        assert_eq!(
            (order.tape_mm, order.label_length_mm),
            (Some(page.width_mm), Some(page.length_mm))
        );
        // HS2b (W2b): the job names the roll's page, as CUPS's `-o PageSize=<name>`
        assert_eq!(order.page_size.as_deref(), Some(page.name));
    }

    /// HS2: every die-cut roll of the 450 takes a page of its own raster and no other.
    #[test]
    fn every_labelwriter_roll_takes_its_own_raster() {
        let profile = crate::profiles::find("dymo/labelwriter-450").unwrap();
        for media in &profile.media {
            let page = crate::profiles::labelwriter_page(Some(profile), &media.id).unwrap();
            let [w, h] = page.dots;
            assert_eq!(
                super::labelwriter_fits([w, h], Some(profile), Some(&media.id)),
                Ok(()),
                "{}",
                media.id
            );
            assert!(super::labelwriter_fits([w + 1, h], Some(profile), Some(&media.id)).is_err());
            assert!(super::labelwriter_fits([w, h + 1], Some(profile), Some(&media.id)).is_err());
            assert_eq!(page.length_mm, media.length_mm.unwrap());
        }
    }
}
