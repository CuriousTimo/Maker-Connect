//! # Maker Connect cloud
//!
//! The website and the API the agents report to. One binary with one SQLite file and no
//! other services, so it runs on a Pi, a NAS, an old laptop or in Docker; everything
//! belongs to a space ([`store`]), so the same software could serve many makerspaces.
//!
//! - `POST` [`protocol::REGISTER`], [`protocol::TELEMETRY`]: the agents ([`maker_connect_protocol`]).
//! - `GET /`: the machines of the space, live; `POST /pair`: add a machine with its code.
//! - `GET /api/machines`: the same list as JSON; `GET /api/machines/stream`: that list
//!   again on every change, as server-sent events, for the page.
//! - `/p/...`: the printer side of Prusa Connect, so a Buddy-firmware Prusa (a MINI)
//!   reports here itself, with no agent ([`prusa_connect`]).
//!
//! - `/login`, `/setup`, `/logout`: people sign in ([`auth`]); every other page and API
//!   needs a session, except the routes machines use with their own tokens.

mod access;
mod accounts;
mod apps;
mod auth;
mod codes;
mod downloads;
mod files;
mod files_page;
mod gcode;
mod label_page;
mod machine_page;
mod printer_files;
mod profiles;
mod prusa_connect;
mod simplyprint;
mod space_page;
mod spoolman;
mod spoolmandb;
mod stock;
mod store;
mod virtual_grbl;
mod virtual_machines;

use axum::{
    Form, Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{
        Html, IntoResponse, Redirect, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use maker_connect_protocol::{self as protocol, Register, Registered, Telemetry, TelemetryReply};
use std::sync::{Arc, Mutex};
use store::Store;

type Shared = Arc<Mutex<Store>>;

const USAGE: &str = "Maker Connect cloud (proof of concept)

Usage:
  maker-connect-cloud [--listen 0.0.0.0:8460] [--db maker-connect.db]
  maker-connect-cloud reset-password <name> [--db maker-connect.db]

Open http://localhost:8460. On a fresh install, make the first account with the setup code
this prints at start; then add machines with the code their agent shows.

reset-password gives an account a new password (printed once) and signs it out everywhere:
for when its owner cannot sign in any more.
";

/// What a release's archive brings beside the cloud, into the folders beside its database.
/// Never the other way: the archive holds no data, so unpacking a new release over an old
/// one leaves the database, the files and the backups as they were.
///
/// - `agents/`: the agents of this release go to the downloads when they are another build
///   than those offered (`.build` last, so a copy cut short is done again next time).
/// - `spoolmandb/`: SpoolmanDB as of the release, only when there is no copy yet.
fn from_release(store: &store::Store) {
    let Some(here) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
    else {
        return;
    };
    let agents = here.join("agents");
    let build = |dir: &std::path::Path| std::fs::read(dir.join(".build")).ok();
    if let Some(new) = build(&agents)
        && build(&store.downloads_dir).as_ref() != Some(&new)
    {
        let copied = std::fs::read_dir(&agents)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("maker-connect-agent-"))
            })
            .all(|e| std::fs::copy(e.path(), store.downloads_dir.join(e.file_name())).is_ok());
        if copied && std::fs::write(store.downloads_dir.join(".build"), &new).is_ok() {
            println!(
                "offering this release's agents: {}",
                String::from_utf8_lossy(&new).lines().next().unwrap_or("")
            );
        }
    }
    let seed = here.join("spoolmandb");
    let kept = store.files_dir.join("spoolmandb");
    if seed.join("filaments.json").is_file() && !kept.join("filaments.json").is_file() {
        let _ = std::fs::create_dir_all(&kept);
        for name in ["materials.json", "filaments.json"] {
            let _ = std::fs::copy(seed.join(name), kept.join(name));
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut listen = "0.0.0.0:8460".to_owned();
    let mut db = "maker-connect.db".to_owned();
    let mut reset: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--listen" => listen = args.next().ok_or("expected an address after --listen")?,
            "--db" => db = args.next().ok_or("expected a file after --db")?,
            "reset-password" => {
                reset = Some(
                    args.next()
                        .ok_or("expected a sign-in name after reset-password")?,
                )
            }
            _ => {
                print!("{USAGE}");
                return Ok(());
            }
        }
    }
    let mut opened = Store::open(&db)?;
    if let Some(name) = reset {
        let Some(user) = opened.user_id(&name)? else {
            return Err(format!("no account named {name:?} in {db}").into());
        };
        let password = accounts::random_token()[..20].to_owned();
        opened.set_password(user, &accounts::hash_password(&password))?;
        println!(
            "{name}'s new password: {password}\n(signed out everywhere; change it after signing in)"
        );
        return Ok(());
    }
    opened.end_old_sessions()?;
    let reread = opened.reread_files()?;
    if reread > 0 {
        println!("read {reread} kept files again");
    }
    from_release(&opened);
    // SpoolmanDB, kept beside the uploaded files and fetched every hour.
    spoolmandb::start(opened.files_dir.join("spoolmandb"));
    let store = Arc::new(Mutex::new(opened));
    tokio::spawn(virtual_machines::run(store.clone()));
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    println!("Maker Connect cloud on http://{listen} (data in {db})");
    if let Some(code) = store.lock().unwrap().setup_code.clone() {
        println!("no accounts yet: open it in a browser and make the first with setup code {code}");
    }
    axum::serve(
        listener,
        app(store).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(stop_asked())
    .await?;
    Ok(())
}

/// Ends when someone stops the cloud (Ctrl-C, or `kill`/systemd/Docker's SIGTERM): no new
/// connections from then on. Open pages keep their live streams open for ever, so waiting
/// for them would never end: after a few seconds, or a second Ctrl-C, the process ends
/// regardless (SQLite's journal keeps the database whole).
async fn stop_asked() {
    stop_signal().await;
    println!("stopping (Ctrl-C again to stop at once)");
    tokio::spawn(async {
        tokio::select! {
            _ = stop_signal() => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
        }
        println!("stopped");
        std::process::exit(0);
    });
}

async fn stop_signal() {
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                match term.as_mut() {
                    Some(term) => { term.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

pub fn app(store: Shared) -> Router {
    let app = Router::new()
        .route(protocol::REGISTER, post(register))
        .route(protocol::TELEMETRY, post(telemetry))
        .route("/", get(page))
        .route("/pair", post(pair))
        .route("/api/machines", get(machines))
        .route("/api/machines/stream", get(machine_stream))
        .route("/machines", get(|| async { Redirect::to("/") }))
        .route("/machines/", get(|| async { Redirect::to("/") }))
        .route("/logo.svg", get(logo))
        .route("/machines/{id}", get(machine_page))
        .route("/machines/{id}/profile", post(choose_profile))
        .route("/machines/{id}/media", post(choose_media))
        .route("/machines/{id}/name", post(rename))
        .route("/machines/{id}/command", post(command))
        .route("/machines/{id}/remove", post(remove))
        .route("/machines/{id}/out-of-use", post(out_of_use))
        .route("/machines/{id}/taken", post(taken))
        .route("/machines/{id}/laser", post(laser_power))
        .route("/api/machines/{id}/history", get(history))
        .route("/api/profiles", get(profile_list))
        .route("/profiles/{brand}/{file}", get(profile_image))
        .route("/virtual", post(add_virtual))
        .merge(prusa_connect::routes())
        .merge(simplyprint::routes())
        .merge(spoolman::routes())
        .merge(spoolmandb::routes())
        .route("/klipper", post(add_klipper))
        .merge(files::routes())
        .merge(downloads::routes())
        .merge(codes::routes())
        .merge(stock::routes())
        .route("/files", get(files_page::page))
        .merge(auth::routes())
        .merge(space_page::routes())
        .merge(apps::routes())
        .merge(printer_files::routes())
        // After a route is matched: what it needs, against who asks (access::RULES).
        .route_layer(axum::middleware::from_fn_with_state(
            store.clone(),
            access::permit,
        ))
        .fallback(not_found)
        // Before: who asks (a session, an API token), and that they are signed in.
        .layer(axum::middleware::from_fn_with_state(
            store.clone(),
            auth::guard,
        ))
        .layer(axum::middleware::from_fn(prusa_connect::log));
    auth::token_paths(app.with_state(store))
}

async fn not_found(uri: axum::http::Uri) -> Response {
    (StatusCode::NOT_FOUND, format!("nothing at {uri}")).into_response()
}

pub(crate) fn failed(error: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
}

async fn register(State(store): State<Shared>, Json(agent): Json<Register>) -> Response {
    if agent.protocol != protocol::VERSION {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "this cloud speaks protocol {}, the agent {}",
                protocol::VERSION,
                agent.protocol
            ),
        )
            .into_response();
    }
    match store.lock().unwrap().register(&agent) {
        Ok((machine_id, token, pairing_code)) => {
            println!(
                "agent registered: {} ({}), code {pairing_code}",
                agent.name,
                agent.kind.label()
            );
            Json(Registered {
                machine_id,
                token,
                pairing_code,
            })
            .into_response()
        }
        Err(error) => failed(error),
    }
}

async fn telemetry(
    State(store): State<Shared>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Json(report): Json<Telemetry>,
) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let mut store = store.lock().unwrap();
    for done in &report.done {
        let state = if done.ok {
            store::CommandState::Finished
        } else {
            store::CommandState::Rejected
        };
        if let Err(error) = store.command_went(
            token,
            done.id,
            state,
            done.reason.clone(),
            done.output.clone(),
        ) {
            return failed(error);
        }
    }
    // Agents before 2026-11-04 reported a label printer without a cassette as an error, or
    // as idle with the reason: it waits for someone (attention), it is not a fault.
    let mut report = report;
    if report.cassette == Some(false)
        && matches!(
            report.state,
            protocol::MachineState::Error | protocol::MachineState::Idle
        )
        && report.problem.as_deref() == Some("no cassette")
    {
        report.state = protocol::MachineState::Attention;
    }
    if let Some(peer) = peer_address(&headers, &extensions)
        && let Err(error) = store.set_peer(token, &peer)
    {
        return failed(error);
    }
    match store.report(token, &report) {
        Ok(Some(space)) => {
            // No commands while a job runs, or while the machine is busy with one (an agent
            // says so while a long command runs), or waits for someone at it: one at a time.
            let held = matches!(
                report.state,
                protocol::MachineState::Busy | protocol::MachineState::Attention
            );
            let commands = if report.job_id.is_some() || held {
                Vec::new()
            } else {
                match store.next_command(token) {
                    Ok(next) => next
                        .map(|(id, gcode)| protocol::MachineCommand { id, gcode })
                        .into_iter()
                        .collect(),
                    Err(error) => return failed(error),
                }
            };
            let (job, job_control) = match agent_job(&mut store, token, &report) {
                Ok(job) => job,
                Err(error) => return failed(error),
            };
            Json(TelemetryReply {
                paired: space.is_some(),
                space,
                commands,
                job,
                job_control,
            })
            .into_response()
        }
        Ok(None) => (StatusCode::UNAUTHORIZED, "unknown agent: register again").into_response(),
        Err(error) => failed(error),
    }
}

/// The answer to a form: none for one saved in the background (`x-requested-with`), else
/// back to the page it belongs to.
fn done(headers: &HeaderMap, to: &str) -> Response {
    if headers.contains_key("x-requested-with") {
        StatusCode::NO_CONTENT.into_response()
    } else {
        Redirect::to(to).into_response()
    }
}

/// Where a request came from: the address it reached the cloud from, or, behind a reverse
/// proxy, the first address the proxy says it forwarded for.
pub(crate) fn peer_address(
    headers: &HeaderMap,
    extensions: &axum::http::Extensions,
) -> Option<String> {
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty());
    forwarded.or_else(|| {
        extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|info| info.0.ip().to_string())
    })
}

/// A job for an agent's machine, as its report stands: how one ended, a queued one to
/// fetch, and pause, resume or stop for the one it runs.
pub(crate) fn agent_job(
    store: &mut Store,
    token: &str,
    report: &Telemetry,
) -> rusqlite::Result<(Option<protocol::JobOrder>, Option<protocol::JobControl>)> {
    use protocol::{JobControl, MachineState};
    use store::JobState;
    if let Some(end) = &report.job_end {
        let state = match (end.ok, end.stopped) {
            (true, _) => JobState::Finished,
            (false, true) => JobState::Stopped,
            (false, false) => JobState::Failed,
        };
        store.set_job(end.id, state, end.reason.as_deref())?;
        if let Some(line) = end.restart_line {
            store.set_restart_line(end.id, line)?;
        }
    }
    let Some(machine) = store.machine_id_by_token(token)? else {
        return Ok((None, None));
    };
    let Some(job) = store.active_job(&machine)? else {
        return Ok((None, None));
    };
    let runs = report.job_id == Some(job.id);
    Ok(match job.state {
        JobState::Queued if report.job_id.is_none() && report.state == MachineState::Idle => {
            let size = std::fs::metadata(store.file_path(&job.file_id)).map_or(0, |m| m.len());
            store.set_job(job.id, JobState::Printing, None)?;
            let media = store.loaded_media_mm(&machine)?;
            let order = protocol::JobOrder {
                id: job.id,
                name: job.file_name.clone(),
                download: format!("/api/agent/v1/jobs/{}/file", job.id),
                size,
                from_line: job.from_line,
                tape_mm: media.map(|(width, _)| width),
                label_length_mm: media.and_then(|(_, length)| length),
            };
            (Some(order), None)
        }
        JobState::Paused if runs && report.state != MachineState::Paused => {
            (None, Some(JobControl::Pause))
        }
        JobState::Printing if runs && report.state == MachineState::Paused => {
            (None, Some(JobControl::Resume))
        }
        JobState::Stopping if runs => (None, Some(JobControl::Stop)),
        // Given out, but the machine does not run it (the agent restarted, or lost it).
        JobState::Printing | JobState::Paused
            if !runs && job.started_at.is_some_and(|t| store::now() - t > 30) =>
        {
            let reason = "the machine does not run it (the agent restarted?)";
            store.set_job(job.id, JobState::Failed, Some(reason))?;
            (None, None)
        }
        // Asked to stop before the agent took it up, or it no longer runs it.
        JobState::Stopping => {
            store.set_job(job.id, JobState::Stopped, None)?;
            (None, None)
        }
        _ => (None, None),
    })
}

#[derive(serde::Deserialize)]
struct PairForm {
    code: String,
}

async fn pair(
    access: crate::access::Access,
    State(store): State<Shared>,
    headers: HeaderMap,
    Form(form): Form<PairForm>,
) -> Response {
    let store = store.lock().unwrap();
    let paired = store
        .space_of(&access)
        .and_then(|(space, _)| store.pair(space, &form.code));
    let fetched = headers.contains_key("x-requested-with");
    match paired {
        Ok(Some(_)) if fetched => StatusCode::NO_CONTENT.into_response(),
        Ok(None) if fetched => {
            (StatusCode::NOT_FOUND, "no machine waits with that code").into_response()
        }
        Ok(Some(_)) => Redirect::to("/").into_response(),
        Ok(None) => Redirect::to("/?unknown-code").into_response(),
        Err(error) => failed(error),
    }
}

async fn machines(access: crate::access::Access, State(store): State<Shared>) -> Response {
    let store = store.lock().unwrap();
    match store
        .space_of(&access)
        .and_then(|(space, _)| store.machines(space))
    {
        Ok(machines) => Json(machines).into_response(),
        Err(error) => failed(error),
    }
}

/// A space's machines as JSON.
fn machines_json(store: &Shared, access: &access::Access) -> String {
    let store = store.lock().unwrap();
    store
        .space_of(access)
        .and_then(|(space, _)| store.machines(space))
        .map(|machines| serde_json::to_string(&machines).unwrap_or_default())
        .unwrap_or_else(|_| "[]".to_owned())
}

/// The machines now, and again after every change, merged over a quarter second; and every
/// few seconds anyway, as "seen" and "online" change with time alone.
async fn machine_stream(
    access: access::Access,
    State(store): State<Shared>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    use std::time::Duration;
    use tokio::sync::broadcast::error::TryRecvError;
    let changes = store.lock().unwrap().subscribe();
    let stream = futures_util::stream::unfold(
        (store, changes, true, access),
        |(store, mut changes, first, access)| async move {
            if !first {
                tokio::select! {
                    _ = changes.recv() => {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        while !matches!(changes.try_recv(), Err(TryRecvError::Empty | TryRecvError::Closed)) {}
                    }
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
            }
            let event = Event::default().data(machines_json(&store, &access));
            Some((Ok(event), (store, changes, false, access)))
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn page(access: crate::access::Access, State(store): State<Shared>) -> Response {
    match store.lock().unwrap().space_of(&access) {
        Ok((_, name)) => Html(
            PAGE.replace("{style}", STYLE)
                .replace("{space}", &escape(&name))
                .replace("{header}", &header(&name, "", "machines")),
        )
        .into_response(),
        Err(error) => failed(error),
    }
}

/// The bar at the top of every page: the name (back to the machines), the space, and a
/// live count of its machines, with whether the page is getting live updates; and the
/// button that opens the dialog to add a machine (by its code, or a virtual one). `root`
/// leads from the page to the site's root ("" or "../"); `here` is the page's place in the
/// navigation ("machines", "files").
pub(crate) fn header(space: &str, root: &str, here: &str) -> String {
    // An empty link is the page itself: from the root, the machines are at "./".
    let home = if root.is_empty() { "./" } else { root };
    let link = |path: &str, name: &str, place: &str, icon: &str| {
        let current = if place == here {
            r#" aria-current="page""#
        } else {
            ""
        };
        format!(
            r#"<a href="{home}{path}"{current}><svg viewBox="0 0 20 20" aria-hidden="true">{icon}</svg><span>{name}</span></a>"#
        )
    };
    format!(
        r##"<aside class="sidebar" data-root="{home}">
  <a class="brand" href="{home}"><img src="{root}logo.svg" alt="" width="30" height="30"><span><b>Maker Connect</b><small>{space}</small></span></a>
  <button type="button" class="search-open" data-palette-open title="Go to a machine, a file or a page"><svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="9" cy="9" r="5.5"/><path d="m13.2 13.2 3.8 3.8"/></svg><span>Go to…</span><kbd>⌘K</kbd></button>
  <nav class="side-nav" aria-label="Pages">{machines}{files}{stock}{space_link}</nav>
  <div class="side-section">
    <div class="side-title"><span>Machines</span><button type="button" class="side-add" data-add-open title="Add a machine" aria-label="Add a machine"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 4.5v11M4.5 10h11"/></svg></button></div>
    <nav class="side-machines" data-side-machines aria-label="Machines"></nav>
  </div>
</aside>
<div class="topbar">
  <div class="summary">
    <span class="count"><b data-sum="machines">–</b> machines</span>
    <span class="count"><b data-sum="online">–</b> online</span>
    <span class="count"><b data-sum="busy">–</b> busy</span>
    <span class="count attention-count" data-attention-count hidden><b data-sum="attention">–</b> attention</span>
    <span class="live" data-live-dot title="Live updates"><i></i>Live</span>
  </div>
</div>
<div class="toasts" data-toasts aria-live="polite"></div>
<dialog class="palette" data-palette aria-label="Go to">
  <input type="search" data-palette-input placeholder="A machine, a file, a page…" autocomplete="off" aria-label="Search">
  <ul class="palette-results" data-palette-results role="listbox"></ul>
  <p class="palette-hint"><kbd>↑</kbd><kbd>↓</kbd> choose <kbd>↵</kbd> go <kbd>esc</kbd> close</p>
</dialog>
<dialog id="add-dialog" aria-labelledby="add-title">
  <div class="dialog-head">
    <button type="button" class="back" data-wizard-back hidden aria-label="Back">←</button>
    <h2 id="add-title">Add a machine</h2>
    <button type="button" class="close" data-add-close aria-label="Close">✕</button>
  </div>
  <section class="step" data-step="start">
    <p class="dialog-sub">How does it connect?</p>
    <div class="choices">
      <button type="button" class="choice" data-go="klipper"><b>A printer on Klipper</b><small>Voron, Sovol SV08, Neptune 4… with Mainsail or Fluidd: Moonraker connects here by itself</small></button>
      <button type="button" class="choice" data-go="prusa"><b>A Prusa with built-in Connect</b><small>MINI, MK4, XL: its own firmware connects here, no agent</small></button>
      <button type="button" class="choice" data-go="usb"><b>A machine on USB</b><small>A Marlin printer, an MK3S, a GRBL laser, a DYMO: through the agent on a computer beside it</small></button>
      <button type="button" class="choice" data-go="virtual"><b>A virtual one</b><small>Simulated in the cloud, to try everything without a machine</small></button>
    </div>
    <p class="dialog-foot">It already shows a code? <button type="button" class="link" data-go="code">Enter the code</button></p>
  </section>
  <section class="step" data-step="klipper" hidden>
    <h3>A printer on Klipper</h3>
    <p class="dialog-sub">Its name here; then two lines for its <code>moonraker.conf</code>.</p>
    <form data-klipper-form action="{home}klipper">
      <input name="name" maxlength="60" required aria-label="Name" placeholder="Voron 2.4">
      <button>Next</button>
    </form>
  </section>
  <section class="step" data-step="klipper-conf" hidden>
    <h3>On the printer</h3>
    <ol class="steps">
      <li>Open its Mainsail or Fluidd, then <b>Machine</b> (or Configuration) and <b>moonraker.conf</b>.</li>
      <li>Add these two lines, save, and restart Moonraker:<pre class="conf" data-klipper-conf></pre></li>
    </ol>
    <p class="hint" data-local-warning hidden>This page was opened as <b>localhost</b>, which the printer cannot reach: open it by this computer's address on the network, or put that address in the line.</p>
    <p class="waiting" data-klipper-wait><i></i>Waiting for it to connect…</p>
    <p class="dialog-foot"><a data-klipper-page href="#">Open its page</a> · the lines are on its Settings too.</p>
  </section>
  <section class="step" data-step="prusa" hidden>
    <h3>A Prusa with built-in Connect</h3>
    <ol class="steps">
      <li>Put a file <code>prusa_printer_settings.ini</code> on its USB stick:<pre class="conf" data-prusa-ini></pre></li>
      <li>On the printer: <b>Settings → Network → Prusa Connect → Load Settings</b>, then <b>Add printer to Connect</b>.</li>
      <li>It shows a code: enter it here.</li>
    </ol>
    <form method="post" action="{home}pair" data-pair-form>
      <input name="code" maxlength="6" autocomplete="off" required aria-label="Pairing code" placeholder="ABC234">
      <button>Add</button>
    </form>
    <p class="note" data-pair-note></p>
  </section>
  <section class="step" data-step="usb" hidden>
    <h3>A machine on USB</h3>
    <ol class="steps">
      <li><a href="{home}downloads">Download the agent</a> for the computer the machine is plugged into: a Pi, a PC, a Mac.</li>
      <li>Start it with this cloud's address and the machine's port: the Downloads page gives the command.</li>
      <li>It shows a code: enter it here.</li>
    </ol>
    <form method="post" action="{home}pair" data-pair-form>
      <input name="code" maxlength="6" autocomplete="off" required aria-label="Pairing code" placeholder="ABC234">
      <button>Add</button>
    </form>
    <p class="note" data-pair-note></p>
  </section>
  <section class="step" data-step="code" hidden>
    <h3>With its code</h3>
    <p class="dialog-sub">An agent shows a code when it starts, and a Prusa when its Connect points here.</p>
    <form method="post" action="{home}pair" data-pair-form>
      <input name="code" maxlength="6" autocomplete="off" required aria-label="Pairing code" placeholder="ABC234">
      <button>Add</button>
    </form>
    <p class="note" data-pair-note data-add-note></p>
  </section>
  <section class="step" data-step="virtual" hidden>
    <h3>A virtual one</h3>
    <p class="dialog-sub">Simulated in the cloud: it moves, heats and measures its bed. Pick its machine.</p>
    <div class="picker" data-picker data-action="{home}virtual"></div>
  </section>
</dialog>
<script>
{SHELL_SCRIPT}</script>"##,
        space = escape(space),
        machines = link(
            "",
            "Machines",
            "machines",
            r#"<rect x="2.5" y="4" width="15" height="10" rx="2"/><path d="M7 17h6M10 14v3"/>"#
        ),
        files = link(
            "files",
            "Files",
            "files",
            r#"<path d="M5 2.5h6.5l3.5 3.5v11.5H5z"/><path d="M11.5 2.5V6H15"/>"#
        ),
        stock = link(
            "stock",
            "Materials",
            "stock",
            r#"<path d="M3 6.5 10 3l7 3.5v7L10 17l-7-3.5z"/><path d="M3 6.5 10 10l7-3.5M10 10v7"/>"#
        ),
        space_link = link(
            "space",
            "Space",
            "space",
            r#"<circle cx="7.5" cy="7" r="2.5"/><circle cx="13.5" cy="8" r="2"/><path d="M3 16c.6-3 2.3-4.5 4.5-4.5S11.4 13 12 16M12.5 12.2c2.2-.4 3.9.9 4.5 3.8"/>"#
        ),
    )
}

/// Every machine profile, for the machine picker.
async fn profile_list() -> Response {
    #[derive(serde::Serialize)]
    struct Listed<'a> {
        #[serde(flatten)]
        profile: &'a profiles::Profile,
        image: bool,
    }
    let listed: Vec<Listed> = profiles::all()
        .iter()
        .map(|profile| Listed {
            profile,
            image: profile.image().is_some(),
        })
        .collect();
    Json(listed).into_response()
}

/// A profile's picture.
async fn profile_image(
    axum::extract::Path((brand, file)): axum::extract::Path<(String, String)>,
) -> Response {
    let image = file.strip_suffix(".svg").and_then(|model| {
        // A machine without a profile: the drawing of its kind (`_generic/fdm.svg`).
        if brand == "_generic" {
            let kind = serde_json::from_value(serde_json::Value::String(model.to_owned())).ok()?;
            return profiles::generic_image(kind);
        }
        profiles::find(&format!("{brand}/{model}")).and_then(|p| p.image())
    });
    match image {
        Some(svg) => (
            [
                ("content-type", "image/svg+xml"),
                ("cache-control", "max-age=3600"),
            ],
            svg,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "no such picture").into_response(),
    }
}

#[derive(serde::Deserialize)]
struct VirtualForm {
    profile: String,
}

async fn add_virtual(
    access: crate::access::Access,
    State(store): State<Shared>,
    Form(form): Form<VirtualForm>,
) -> Response {
    let Some(profile) = profiles::find(&form.profile) else {
        return (StatusCode::BAD_REQUEST, "no such profile").into_response();
    };
    let mut store = store.lock().unwrap();
    match store
        .space_of(&access)
        .and_then(|(space, _)| store.add_virtual(space, profile))
    {
        Ok(id) => Redirect::to(&format!("/machines/{id}")).into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
struct KlipperForm {
    name: String,
}

/// A printer on Klipper: made here, its Settings then give the lines for its moonraker.conf.
async fn add_klipper(
    access: crate::access::Access,
    State(store): State<Shared>,
    headers: HeaderMap,
    Form(form): Form<KlipperForm>,
) -> Response {
    let name = form.name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return (StatusCode::BAD_REQUEST, "a name of 1 to 60 characters").into_response();
    }
    let mut store = store.lock().unwrap();
    let added = store.space_of(&access).and_then(|(space, _)| {
        let id = store.add_moonraker(space, name)?;
        Ok((id.clone(), store.moonraker_token(space, &id)?))
    });
    match added {
        // The wizard shows its lines and waits for it.
        Ok((id, token)) if headers.contains_key("x-requested-with") => {
            Json(serde_json::json!({ "id": id, "token": token })).into_response()
        }
        Ok((id, _)) => Redirect::to(&format!("/machines/{id}#settings")).into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
struct ProfileForm {
    /// A profile id, or empty to recognise the machine from what it says.
    profile: String,
}

/// The same buttons on a GRBL machine: homing is `$H`, and moves are GRBL's own jogs (the
/// laser stays off, and a jog can be cancelled). The origin of a job can be set where the
/// head is (`G92 X0 Y0`, as LaserFoundry does it), and gone back to.
fn grbl_action(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "home" => ("Home", "$H"),
        "x+" => ("X +10 mm", "$J=G91 G21 X10 F3000"),
        "x-" => ("X −10 mm", "$J=G91 G21 X-10 F3000"),
        "y+" => ("Y +10 mm", "$J=G91 G21 Y10 F3000"),
        "y-" => ("Y −10 mm", "$J=G91 G21 Y-10 F3000"),
        "set-origin" => ("Set origin here", "G92 X0 Y0"),
        "go-origin" => ("Go to origin", "G90 G0 X0 Y0"),
        "settings" => ("Read settings", "$$"),
        _ => return None,
    })
}

/// The steps the jog pad offers (mm), and its speeds (mm/min): others are refused.
const JOG_STEPS: [f64; 4] = [0.1, 1.0, 10.0, 50.0];
const JOG_FEEDS: [f64; 4] = [600.0, 1500.0, 3000.0, 6000.0];

/// A move of the jog pad (lasers, CNCs, plotters: printers keep their buttons):
/// `jog:<x>:<y>`, each -1, 0 or 1 (diagonals too), by a step and at a speed it offers. On
/// GRBL a jog of its own (`$J=`, the laser off), else a relative move. Its label and G-code.
fn jog_command(action: &str, step: f64, feed: f64, grbl: bool) -> Option<(String, String)> {
    if !JOG_STEPS.contains(&step) || !JOG_FEEDS.contains(&feed) {
        return None;
    }
    let dir = |t: &str| match t {
        "-1" => Some(-1.0),
        "0" => Some(0.0),
        "1" => Some(1.0),
        _ => None,
    };
    let (x, y) = action.strip_prefix("jog:")?.split_once(':')?;
    let (x, y) = (dir(x)?, dir(y)?);
    if (x, y) == (0.0, 0.0) {
        return None;
    }
    let mut axes = String::new();
    let mut label = String::from("Move");
    for (name, d) in [("X", x), ("Y", y)] {
        if d != 0.0 {
            axes += &format!(" {name}{}", d * step);
            label += &format!(" {name}{}{step}", if d > 0.0 { "+" } else { "−" });
        }
    }
    label += " mm";
    let gcode = if grbl {
        format!("$J=G91 G21{axes} F{feed}")
    } else {
        format!("G91\nG1{axes} F{feed}\nG90")
    };
    Some((label, gcode))
}

/// The commands the website offers: a label and its G-code. Moves are relative, 10 mm.
fn action(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "home" => ("Home", "G28"),
        "x+" => ("X +10 mm", "G91\nG1 X10 F3000\nG90"),
        "x-" => ("X −10 mm", "G91\nG1 X-10 F3000\nG90"),
        "y+" => ("Y +10 mm", "G91\nG1 Y10 F3000\nG90"),
        "y-" => ("Y −10 mm", "G91\nG1 Y-10 F3000\nG90"),
        "z+" => ("Z +10 mm", "G91\nG1 Z10 F600\nG90"),
        "z-" => ("Z −10 mm", "G91\nG1 Z-10 F600\nG90"),
        "preheat-pla" => ("Preheat PLA (215/60 °C)", "M104 S215\nM140 S60"),
        "preheat-petg" => ("Preheat PETG (240/80 °C)", "M104 S240\nM140 S80"),
        "cooldown" => ("Cool down", "M104 S0\nM140 S0"),
        "unlock" => ("Unlock", "$X"),
        "laser-off" => ("Laser off", "M5"),
        "fan-on" => ("Print fan on", "M106 S255"),
        "fan-off" => ("Print fan off", "M107"),
        "motors-off" => ("Motors off", "M18"),
        "set-origin" => ("Set origin here", "G92 X0 Y0"),
        "go-origin" => ("Go to origin", "G90\nG0 X0 Y0"),
        _ => return None,
    })
}

/// The G-code that has a machine measure its bed and print the mesh, by its firmware; or
/// why it cannot be done.
pub(crate) fn probe_gcode(machine: &store::Machine) -> Result<&'static str, &'static str> {
    // Klipper measures with BED_MESH_CALIBRATE; Moonraker sends the mesh by itself.
    if machine.link == "simplyprint" {
        return Ok("G28\nBED_MESH_CALIBRATE");
    }
    if machine.link == "prusa-connect" {
        return Err(
            "this printer does not send its mesh over Prusa Connect: an agent on USB could read it",
        );
    }
    let firmware = machine
        .profile
        .as_deref()
        .and_then(profiles::find)
        .and_then(|p| p.firmware.as_deref());
    match firmware {
        // Prusa's 8-bit firmware: G80 measures, G81 prints the mesh.
        Some("prusa") => Ok("G28\nG80\nG81"),
        // Marlin (and Prusa's Buddy, which is Marlin): G29 measures, M420 V prints.
        Some("marlin" | "prusa-buddy") => Ok("G28\nG29\nM420 V"),
        // Klipper measures with BED_MESH_CALIBRATE, through Moonraker: not connected yet.
        Some("klipper") => {
            Err("measuring the bed of a printer on Klipper follows, with Klipper itself")
        }
        _ => Err("choose the machine's profile first: its firmware says how to measure"),
    }
}

#[derive(serde::Deserialize)]
struct CommandForm {
    /// One of the website's buttons ([`action`]).
    #[serde(default)]
    action: String,
    /// G-code typed in the terminal.
    #[serde(default)]
    gcode: String,
    /// The jog pad's step (mm) and speed (mm/min).
    #[serde(default)]
    step: Option<f64>,
    #[serde(default)]
    feed: Option<f64>,
    /// A temperature typed for the nozzle or the bed (°C).
    #[serde(default)]
    nozzle: Option<f64>,
    #[serde(default)]
    bed: Option<f64>,
    /// The speed and flow factors typed (%).
    #[serde(default)]
    speed: Option<f64>,
    #[serde(default)]
    flow: Option<f64>,
}

/// A speed or flow factor someone typed, as G-code: whole percents in a range a print
/// survives (speed 10 to 300 %, flow 50 to 150 %).
fn factor_command(
    action: &str,
    speed: Option<f64>,
    flow: Option<f64>,
) -> Option<Result<(String, String), String>> {
    let (what, code, value, low, high) = match action {
        "set-speed" => ("Speed", "M220", speed, 10.0, 300.0),
        "set-flow" => ("Flow", "M221", flow, 50.0, 150.0),
        _ => return None,
    };
    let Some(value) = value.filter(|v| v.is_finite()).map(f64::round) else {
        return Some(Err(format!("{what}: type a percentage")));
    };
    if !(low..=high).contains(&value) {
        return Some(Err(format!("{what}: {low} to {high} %")));
    }
    Some(Ok((
        format!("{what} {value} %"),
        format!("{code} S{value}"),
    )))
}

/// A temperature someone typed, as G-code: whole degrees from 0 (off) to the profile's
/// maximum (or a safe one without a profile).
fn heat_command(
    action: &str,
    nozzle: Option<f64>,
    bed: Option<f64>,
    profile: Option<&profiles::Profile>,
) -> Option<Result<(String, String), String>> {
    let (what, code, value, max) = match action {
        "set-nozzle" => (
            "Nozzle",
            "M104",
            nozzle,
            profile.and_then(|p| p.max_nozzle_c).unwrap_or(260.0),
        ),
        "set-bed" => (
            "Bed",
            "M140",
            bed,
            profile.and_then(|p| p.max_bed_c).unwrap_or(100.0),
        ),
        _ => return None,
    };
    let Some(value) = value.filter(|v| v.is_finite()) else {
        return Some(Err(format!("{what}: type a temperature")));
    };
    let value = value.round();
    if !(0.0..=max).contains(&value) {
        return Some(Err(format!("{what}: 0 to {max} °C on this machine")));
    }
    let label = if value == 0.0 {
        format!("{what} off")
    } else {
        format!("{what} to {value} °C")
    };
    Some(Ok((label, format!("{code} S{value}"))))
}

/// G-code from the terminal, tidied: its lines without blank ones, and a label (its first
/// line). Short enough for every machine: a Prusa on built-in Connect takes 512 bytes.
fn typed_gcode(text: &str) -> Result<(String, String), &'static str> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return Err("no G-code");
    }
    if lines.len() > 20 || text.len() > 480 {
        return Err("too long: at most 20 lines, 480 characters");
    }
    if lines
        .iter()
        .any(|l| l.chars().any(|c| c.is_control() || !c.is_ascii()))
    {
        return Err("G-code is plain text: letters, numbers, spaces");
    }
    let label = match lines.len() {
        1 => lines[0].to_owned(),
        n => format!("{} (+{} lines)", lines[0], n - 1),
    };
    Ok((label, lines.join("\n")))
}

async fn command(
    access: crate::access::Access,
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<CommandForm>,
) -> Response {
    let mut store = store.lock().unwrap();
    let space = match store.space_of(&access) {
        Ok((space, _)) => space,
        Err(error) => return failed(error),
    };
    if !form.gcode.trim().is_empty() {
        let (label, gcode) = match typed_gcode(&form.gcode) {
            Ok(typed) => typed,
            Err(why) => return (StatusCode::BAD_REQUEST, why).into_response(),
        };
        return match store.queue_command(space, &id, &label, &gcode) {
            Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
            Ok(None) => (
                StatusCode::NOT_FOUND,
                "no such machine, or it takes no commands",
            )
                .into_response(),
            Err(error) => failed(error),
        };
    }
    // A GRBL machine homes with $H, not G28.
    let grbl = || {
        store
            .machine(space, &id)
            .ok()
            .flatten()
            .and_then(|m| m.profile)
            .and_then(|p| profiles::find(&p))
            .and_then(|p| p.firmware.as_deref())
            == Some("grbl")
    };
    let owned = |(l, g): (&str, &str)| (l.to_owned(), g.to_owned());
    let is_grbl = grbl();
    let found = if form.action.starts_with("jog") {
        jog_command(
            &form.action,
            form.step.unwrap_or(10.0),
            form.feed.unwrap_or(3000.0),
            is_grbl,
        )
    } else if is_grbl && let Some(found) = grbl_action(&form.action) {
        Some(owned(found))
    } else if let Some(found) = factor_command(&form.action, form.speed, form.flow) {
        match found {
            Ok(found) => Some(found),
            Err(why) => return (StatusCode::BAD_REQUEST, why).into_response(),
        }
    } else if matches!(form.action.as_str(), "set-nozzle" | "set-bed") {
        let machine = match store.machine(space, &id) {
            Ok(Some(machine)) => machine,
            Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
            Err(error) => return failed(error),
        };
        let profile = machine.profile.as_deref().and_then(profiles::find);
        match heat_command(&form.action, form.nozzle, form.bed, profile) {
            Some(Ok(found)) => Some(found),
            Some(Err(why)) => return (StatusCode::BAD_REQUEST, why).into_response(),
            None => None,
        }
    } else if form.action == "probe-bed" {
        match store.machine(space, &id) {
            Ok(Some(machine)) => match probe_gcode(&machine) {
                Ok(gcode) => Some(owned(("Probe the bed", gcode))),
                Err(why) => return (StatusCode::BAD_REQUEST, why).into_response(),
            },
            Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
            Err(error) => return failed(error),
        }
    } else {
        action(&form.action).map(owned)
    };
    let Some((label, gcode)) = found else {
        return (StatusCode::BAD_REQUEST, "no such command").into_response();
    };
    let queued = store.queue_command(space, &id, &label, &gcode);
    match queued {
        // The page sends it without leaving; a plain form comes back to the page.
        Ok(Some(_)) if headers.contains_key("x-requested-with") => {
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Some(_)) => Redirect::to(&format!("/machines/{id}#control")).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            "no such machine, or it takes no commands",
        )
            .into_response(),
        Err(error) => failed(error),
    }
}

async fn remove(
    access: crate::access::Access,
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut store = store.lock().unwrap();
    match store
        .space_of(&access)
        .and_then(|(space, _)| store.remove(space, &id))
    {
        Ok(true) => done(&headers, "/"),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

async fn history(
    access: crate::access::Access,
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let store = store.lock().unwrap();
    match store
        .space_of(&access)
        .and_then(|(space, _)| store.history(space, &id))
    {
        Ok(Some(samples)) => Json(samples).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
struct NameForm {
    name: String,
}

async fn rename(
    access: crate::access::Access,
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<NameForm>,
) -> Response {
    let name = form.name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return (StatusCode::BAD_REQUEST, "a name of 1 to 60 characters").into_response();
    }
    let store = store.lock().unwrap();
    match store
        .space_of(&access)
        .and_then(|(space, _)| store.rename(space, &id, name))
    {
        Ok(true) => done(&headers, &format!("/machines/{id}#settings")),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
struct OutOfUseForm {
    /// Why it is out of use; may be empty.
    #[serde(default)]
    reason: String,
    /// `out` takes it out of use, `back` brings it back.
    action: String,
}

/// The owner takes a machine out of use, with why, or brings it back.
async fn out_of_use(
    access: crate::access::Access,
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<OutOfUseForm>,
) -> Response {
    let reason = form.reason.trim();
    if reason.chars().count() > 200 {
        return (
            StatusCode::BAD_REQUEST,
            "a reason of at most 200 characters",
        )
            .into_response();
    }
    let reason = match form.action.as_str() {
        "out" => Some(reason),
        "back" => None,
        _ => return (StatusCode::BAD_REQUEST, "out or back").into_response(),
    };
    let store = store.lock().unwrap();
    match store
        .space_of(&access)
        .and_then(|(space, _)| store.set_out_of_use(space, &id, reason))
    {
        Ok(true) => done(&headers, &format!("/machines/{id}#settings")),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
struct LaserForm {
    /// Watts, or empty for the profile's.
    #[serde(default)]
    watts: String,
}

/// A laser's module: its optical power, as the owner says.
async fn laser_power(
    access: crate::access::Access,
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<LaserForm>,
) -> Response {
    let watts = match form.watts.trim().replace(',', ".") {
        empty if empty.is_empty() => None,
        w => match w.parse::<f64>() {
            Ok(w) if (0.1..=200.0).contains(&w) => Some(w),
            _ => return (StatusCode::BAD_REQUEST, "watts: 0.1 to 200").into_response(),
        },
    };
    let store = store.lock().unwrap();
    match store
        .space_of(&access)
        .and_then(|(space, _)| store.set_laser_w(space, &id, watts))
    {
        Ok(true) => done(&headers, &format!("/machines/{id}#settings")),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

/// Someone took the finished print off the bed: the machine can take the next job.
async fn taken(
    access: crate::access::Access,
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut store = store.lock().unwrap();
    let space = match store.space_of(&access) {
        Ok((space, _)) => space,
        Err(error) => return failed(error),
    };
    match store.print_taken(space, &id) {
        Ok(true) => done(&headers, &format!("/machines/{id}")),
        Ok(false) => (StatusCode::NOT_FOUND, "no print on its bed").into_response(),
        Err(error) => failed(error),
    }
}

async fn choose_profile(
    access: crate::access::Access,
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<ProfileForm>,
) -> Response {
    let profile = Some(form.profile.as_str()).filter(|p| !p.is_empty());
    if profile.is_some_and(|p| profiles::find(p).is_none()) {
        return (StatusCode::BAD_REQUEST, "no such profile").into_response();
    }
    let store = store.lock().unwrap();
    match store
        .space_of(&access)
        .and_then(|(space, _)| store.set_profile(space, &id, profile))
    {
        Ok(true) => done(&headers, &format!("/machines/{id}#settings")),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
struct MediaForm {
    /// A media id of the machine's profile, or empty: not said.
    media: String,
}

/// The owner says which tape (or roll) is loaded.
async fn choose_media(
    access: crate::access::Access,
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<MediaForm>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match store.space_of(&access) {
        Ok((space, _)) => space,
        Err(error) => return failed(error),
    };
    let machine = match store.machine(space, &id) {
        Ok(Some(machine)) => machine,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => return failed(error),
    };
    let media = Some(form.media.as_str()).filter(|m| !m.is_empty());
    let profile = machine.profile.as_deref().and_then(profiles::find);
    if let Some(media) = media
        && profile.and_then(|p| profiles::media(p, media)).is_none()
    {
        return (StatusCode::BAD_REQUEST, "not a tape of this printer").into_response();
    }
    match store.set_media(space, &id, media) {
        // Saved in the background: the page gets the tape back, to draw the label for it.
        Ok(_) if headers.contains_key("x-requested-with") => {
            let machine = match store.machine(space, &id) {
                Ok(Some(machine)) => machine,
                _ => return StatusCode::NO_CONTENT.into_response(),
            };
            (
                [("content-type", "application/json")],
                label_page::tape_json(profile, &machine),
            )
                .into_response()
        }
        Ok(_) => Redirect::to(&format!("/machines/{id}#label")).into_response(),
        Err(error) => failed(error),
    }
}

/// A machine: its profile to choose (by brand), what the profile knows, and its bed.
async fn machine_page(
    access: crate::access::Access,
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let store = store.lock().unwrap();
    let found = store
        .space_of(&access)
        .and_then(|(space, name)| Ok((name, store.machine(space, &id)?)));
    match found {
        Ok((space, Some(machine))) if machine.kind == protocol::MachineKind::Label => {
            Html(label_page::render(&space, &machine)).into_response()
        }
        Ok((space, Some(machine))) => Html(machine_page::render(&space, &machine)).into_response(),
        Ok((_, None)) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

/// The logo mark, for the page's header and its icon.
async fn logo() -> Response {
    (
        [("content-type", "image/svg+xml")],
        include_str!("../../../assets/logo-mark.svg"),
    )
        .into_response()
}

pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The machine list: drawn by the page from `/api/machines` every two seconds.
/// The styling both pages share.
/// The app's shell on every page: the add dialog, the sidebar's machines, the counts, toasts,
/// forms saved in the background, and the ⌘K palette.
const SHELL_SCRIPT: &str = include_str!("shell.js");

pub(crate) const STYLE: &str = r#"  :root { color-scheme: light dark; --bg: #f6f5f2; --card: #fff; --ink: #1d1d1b; --muted: #6b6b66; --line: #e2e0da; --accent: #2f7d6d; --off: #9a9893; --bad: #b3412e; --warn: #b7791f; }
  @media (prefers-color-scheme: dark) { :root { --bg: #111214; --card: #1b1c1f; --ink: #f2f1ee; --muted: #a3a29d; --line: #2e2f33; --accent: #5fbfa9; --off: #77767a; --bad: #e07a63; --warn: #d9a441; } }
  * { box-sizing: border-box; }
  body { margin: 0; background: var(--bg); color: var(--ink); font: 15px/1.45 system-ui, sans-serif; }
  /* The app's shell: a sidebar beside every page, the page beside it at one width. */
  :root { --side: 248px; --fdm: #2f7d6d; --laser: #d9692b; --label: #7d5bd6; }
  @media (prefers-color-scheme: dark) { :root { --fdm: #5fbfa9; --laser: #f08a4b; --label: #a98bf5; } }
  @view-transition { navigation: auto; }
  ::view-transition-old(root), ::view-transition-new(root) { animation-duration: .18s; }
  .sidebar { view-transition-name: sidebar; position: fixed; inset: 0 auto 0 0; width: var(--side); z-index: 10; display: flex; flex-direction: column; gap: 6px; padding: 16px 12px; background: color-mix(in srgb, var(--card) 70%, var(--bg)); border-right: 1px solid var(--line); overflow-y: auto; }
  body { padding-left: var(--side); }
  .brand { display: flex; align-items: center; gap: 10px; padding: 4px 8px 10px; text-decoration: none; }
  .brand span { display: grid; line-height: 1.2; min-width: 0; }
  .brand b { font-size: 16px; letter-spacing: -0.2px; }
  .brand small { color: var(--muted); font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sidebar svg { width: 18px; height: 18px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; flex: none; }
  .search-open { display: flex; align-items: center; gap: 8px; width: 100%; padding: 7px 10px; margin-bottom: 6px; background: var(--bg); color: var(--muted); border: 1px solid var(--line); font-weight: 500; font-size: 14px; }
  .search-open span { flex: 1; text-align: left; }
  kbd { font: 11px ui-monospace, Menlo, monospace; padding: 1px 5px; border: 1px solid var(--line); border-radius: 4px; color: var(--muted); }
  .side-nav { display: grid; gap: 2px; }
  .side-nav a { display: flex; align-items: center; gap: 10px; padding: 8px 10px; border-radius: 8px; color: var(--muted); text-decoration: none; font-weight: 600; font-size: 14px; }
  .side-nav a:hover { color: var(--ink); background: color-mix(in srgb, var(--line) 60%, transparent); }
  .side-nav a[aria-current=page] { color: var(--ink); background: var(--line); }
  .side-section { margin-top: 14px; min-height: 0; }
  .side-title { display: flex; align-items: center; justify-content: space-between; padding: 0 6px 6px 10px; color: var(--muted); font-size: 11px; font-weight: 700; letter-spacing: 1px; text-transform: uppercase; }
  .side-add { background: transparent; color: var(--muted); padding: 3px; border-radius: 6px; display: grid; }
  .side-add:hover { color: var(--ink); background: var(--line); }
  .side-add svg { width: 16px; height: 16px; }
  .side-machines { display: grid; gap: 1px; }
  .side-machines a { position: relative; display: grid; grid-template-columns: auto 1fr auto; align-items: center; gap: 9px; padding: 7px 10px; border-radius: 8px; color: var(--ink); text-decoration: none; font-size: 14px; }
  .side-machines a:hover { background: color-mix(in srgb, var(--line) 60%, transparent); }
  .side-machines a[aria-current=page] { background: var(--line); }
  .side-machines a::before { content: ""; position: absolute; left: 0; top: 9px; bottom: 9px; width: 3px; border-radius: 2px; background: var(--k, transparent); }
  .side-machines a[data-kind=fdm] { --k: var(--fdm); } .side-machines a[data-kind=laser] { --k: var(--laser); } .side-machines a[data-kind=label] { --k: var(--label); }
  .side-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .side-machines small { color: var(--muted); font-size: 12px; max-width: 90px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .side-machines small.attention { color: var(--warn); } .side-machines small.error { color: var(--bad); }
  .side-progress { grid-column: 2 / -1; height: 3px; border-radius: 2px; background: var(--line); overflow: hidden; }
  .side-progress i { display: block; height: 100%; background: var(--accent); transition: width .5s; }
  .side-empty { margin: 4px 10px; color: var(--muted); font-size: 13px; }
  /* States, one list for badges and dots: quiet unless someone should look. Free is a ring
     (on, nothing to do), busy is filled, attention amber, an error red. */
  .dot { width: 9px; height: 9px; border-radius: 50%; background: var(--off); flex: none; }
  .dot.idle { background: transparent; box-shadow: inset 0 0 0 2px var(--accent); }
  .dot.busy { background: var(--accent); box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 25%, transparent); }
  .dot.paused, .dot.attention { background: var(--warn); } .dot.error { background: var(--bad); }
  .dot.cooling { background: var(--accent); } .dot.out { background: transparent; box-shadow: inset 0 0 0 1.5px var(--off); }
  /* The counts, at the top of every page. */
  .topbar { view-transition-name: topbar; position: sticky; top: 0; z-index: 9; display: flex; justify-content: flex-end; padding: 10px 36px; background: color-mix(in srgb, var(--bg) 88%, transparent); backdrop-filter: blur(10px); -webkit-backdrop-filter: blur(10px); border-bottom: 1px solid var(--line); }
  .summary { display: flex; align-items: center; gap: 18px; color: var(--muted); font-size: 14px; white-space: nowrap; }
  .summary b { color: var(--ink); font-variant-numeric: tabular-nums; }
  .live { display: inline-flex; align-items: center; gap: 8px; }
  .live i { width: 8px; height: 8px; border-radius: 50%; background: var(--off); }
  .live.on i { background: var(--accent); box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 25%, transparent); }
  .state.pulse { animation: pulse .9s ease-out; }
  @keyframes pulse { 0% { box-shadow: 0 0 0 0 currentColor; } 100% { box-shadow: 0 0 0 8px transparent; } }
  /* Toasts: what a save did, for a moment. */
  .toasts { position: fixed; right: 20px; bottom: 20px; z-index: 50; display: grid; gap: 8px; }
  .toast { padding: 10px 16px; border-radius: 10px; background: var(--ink); color: var(--bg); font-weight: 600; font-size: 14px; box-shadow: 0 8px 30px rgb(0 0 0 / .3); animation: toast-in .2s ease-out; transition: opacity .4s, transform .4s; }
  .toast.bad { background: var(--bad); color: #fff; }
  .toast.gone { opacity: 0; transform: translateY(6px); }
  @keyframes toast-in { from { opacity: 0; transform: translateY(8px); } }
  /* The machine picker: search, kinds, a list with drawings. */
  .picker { display: grid; gap: 10px; }
  .picker-search { width: 100%; }
  .picker-kinds { display: flex; gap: 6px; flex-wrap: wrap; }
  .picker-kinds button { background: transparent; color: var(--muted); border: 1px solid var(--line); padding: 4px 10px; font-size: 13px; }
  .picker-kinds button[aria-pressed=true] { color: var(--ink); border-color: var(--accent); background: color-mix(in srgb, var(--accent) 12%, transparent); }
  .picker-list { list-style: none; margin: 0; padding: 4px; max-height: 340px; overflow-y: auto; border: 1px solid var(--line); border-radius: 10px; }
  .picker.in-card .picker-list { max-height: 300px; }
  .picker-list li { display: flex; align-items: center; gap: 12px; padding: 6px 8px; border-radius: 8px; cursor: pointer; }
  .picker-list li[aria-selected=true] { background: var(--line); }
  .picker-list li:hover { background: color-mix(in srgb, var(--line) 70%, transparent); }
  .picker-list img, .picker-auto { width: 40px; height: 40px; border-radius: 8px; flex: none; }
  .picker-auto { display: grid; place-items: center; background: var(--line); color: var(--muted); font-size: 20px; }
  .picker-text { display: grid; min-width: 0; flex: 1; }
  .picker-text b { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .picker-text small { color: var(--muted); font-size: 12px; }
  .picker-tag { font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: .5px; color: var(--accent); }
  .picker-list li.chosen b { color: var(--accent); }
  .picker-none { color: var(--muted); cursor: default !important; background: none !important; font-size: 13px; padding: 10px !important; }
  /* The palette. */
  dialog.palette { width: min(560px, calc(100vw - 32px)); margin-top: 12vh; padding: 0; }
  .palette input { width: 100%; border: 0; border-bottom: 1px solid var(--line); border-radius: 0; padding: 16px 18px; font-size: 17px; background: transparent; outline: none; }
  .palette-results { list-style: none; margin: 0; padding: 6px; max-height: 50vh; overflow-y: auto; }
  .palette-results li { display: flex; align-items: center; gap: 10px; padding: 9px 12px; border-radius: 8px; cursor: pointer; }
  .palette-results li span { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .palette-results li small { color: var(--muted); }
  .palette-results li[aria-selected=true] { background: var(--line); }
  .palette-results li.none { color: var(--muted); cursor: default; }
  .palette-hint { margin: 0; padding: 8px 16px 12px; color: var(--muted); font-size: 12px; display: flex; gap: 6px; align-items: center; }
  /* On a phone: the pages as a tab bar at the bottom. */
  .side-user { margin-top: auto; border-top: 1px solid var(--line); padding-top: 8px; position: relative; }
  .side-user summary { display: flex; align-items: center; gap: 8px; padding: 6px 6px 6px 10px; border-radius: 8px; cursor: pointer; list-style: none; }
  .side-user summary::-webkit-details-marker { display: none; }
  .side-user summary:hover { background: color-mix(in srgb, var(--line) 60%, transparent); }
  .side-user summary svg { color: var(--muted); }
  .side-user[open] summary svg { transform: rotate(180deg); }
  .side-user .who { display: flex; flex-direction: column; min-width: 0; flex: 1; }
  .side-user .who b, .side-user .who small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .side-user .who b { font-size: 13px; }
  .side-user .who small { color: var(--muted); font-size: 12px; }
  .side-menu { position: absolute; bottom: calc(100% + 4px); left: 0; right: 0; display: grid; gap: 2px; padding: 6px; background: var(--card); border: 1px solid var(--line); border-radius: 10px; box-shadow: 0 10px 30px rgb(0 0 0 / .2); z-index: 20; max-height: 60vh; overflow-y: auto; }
  .side-menu > div { display: grid; gap: 2px; }
  .side-menu small { color: var(--muted); font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: .5px; padding: 6px 8px 2px; }
  .side-menu button, .side-menu a { display: block; width: 100%; text-align: left; padding: 7px 8px; border: 0; border-radius: 6px; background: none; color: var(--ink); font: inherit; font-size: 14px; font-weight: 500; text-decoration: none; cursor: pointer; }
  .side-menu button:hover, .side-menu a:hover { background: var(--line); }
  .side-menu button[aria-current] { color: var(--accent); font-weight: 700; }
  .side-menu form { margin: 0; border-top: 1px solid var(--line); padding-top: 2px; display: block; }
  @media (max-width: 820px) {
    body { padding-left: 0; padding-bottom: 72px; }
    .sidebar { inset: auto 0 0 0; width: auto; height: 64px; flex-direction: row; align-items: center; justify-content: space-around; padding: 6px 8px; border-right: 0; border-top: 1px solid var(--line); overflow: visible; }
    .sidebar .brand, .sidebar .side-section, .search-open span, .search-open kbd { display: none; }
    .side-user { margin: 0; padding: 0; border: 0; } .side-user .who { display: none; } .side-menu { left: auto; right: 0; width: 240px; bottom: calc(100% + 10px); }
    .topbar { padding: 8px 16px; }
    .summary { gap: 12px; font-size: 13px; }
    .search-open { width: auto; margin: 0; border: 0; background: transparent; }
    .side-nav { display: flex; gap: 4px; flex: 1; justify-content: space-around; }
    .side-nav a { flex-direction: column; gap: 2px; font-size: 11px; padding: 6px 10px; }
    .search-open { flex: none; padding: 8px; }
    html, body { overflow-x: hidden; }
    main > * { max-width: 100%; }
  }
  dialog { width: min(600px, calc(100vw - 32px)); padding: 0; border: 1px solid var(--line); border-radius: 16px; background: var(--card); color: var(--ink); box-shadow: 0 20px 60px rgb(0 0 0 / 0.35); }
  dialog::backdrop { background: rgb(0 0 0 / 0.45); backdrop-filter: blur(2px); }
  .dialog-head { display: flex; align-items: center; justify-content: space-between; padding: 18px 22px 0; }
  .dialog-head h2 { margin: 0; font-size: 20px; }
  dialog .close { background: transparent; color: var(--muted); font-size: 18px; padding: 4px 8px; }
  dialog .back { background: transparent; color: var(--muted); font-size: 18px; padding: 4px 8px; margin-right: 4px; }
  .dialog-head h2 { flex: 1; }
  /* The add wizard: a choice, then its steps. */
  .choices { display: grid; gap: 8px; }
  .choice { display: grid; gap: 2px; text-align: left; padding: 12px 14px; background: var(--bg); color: var(--ink); border: 1px solid var(--line); border-radius: 12px; font-weight: 400; }
  .choice:hover, .choice:focus-visible { border-color: var(--accent); }
  .choice b { font-size: 15px; }
  .choice small { color: var(--muted); font-size: 13px; }
  .dialog-foot { margin: 14px 0 0; color: var(--muted); font-size: 14px; }
  button.link { background: none; border: 0; padding: 0; color: var(--accent); font: inherit; font-weight: 600; text-decoration: underline; cursor: pointer; }
  ol.steps { margin: 8px 0 14px; padding-left: 20px; display: grid; gap: 8px; font-size: 14px; }
  ol.steps li { min-width: 0; }
  pre.conf { margin: 6px 0 0; padding: 10px 12px; border-radius: 8px; background: var(--bg); border: 1px solid var(--line); font-size: 13px; white-space: pre-wrap; overflow-wrap: anywhere; user-select: all; }
  .waiting { display: flex; align-items: center; gap: 8px; color: var(--muted); font-size: 14px; }
  .waiting i { width: 10px; height: 10px; border-radius: 50%; background: var(--off); animation: wait 1.2s ease-in-out infinite; }
  .waiting.done { color: var(--ink); font-weight: 600; }
  .waiting.done i { background: var(--accent); animation: none; }
  @keyframes wait { 50% { opacity: .3; } }
  dialog section { padding: 16px 22px 18px; border-top: 1px solid var(--line); margin-top: 16px; }
  dialog section + section { margin-top: 0; }
  dialog h3 { margin: 0; font-size: 16px; }
  .dialog-sub { margin: 4px 0 12px; color: var(--muted); font-size: 14px; }
  dialog form { margin: 0; }
  dialog select { flex: 1; }
  dialog .note:empty { display: none; }
  dialog .note { margin: 10px 0 0; }
  main, main.wide { max-width: 1320px; margin: 0 auto; padding: 28px 36px 40px; }
  @media (max-width: 820px) { main, main.wide { padding: 20px 16px 28px; } }
  form { display: flex; gap: 8px; margin-bottom: 20px; flex-wrap: wrap; align-items: center; }
  input, select { font: inherit; padding: 8px 10px; border: 1px solid var(--line); border-radius: 8px; background: var(--card); color: var(--ink); max-width: 100%; }
  input[name=code] { width: 9em; text-transform: uppercase; letter-spacing: 2px; }
  button { font: inherit; font-weight: 600; padding: 8px 14px; border: 0; border-radius: 8px; background: var(--accent); color: #fff; cursor: pointer; }
  .note { color: var(--bad); }
  /* A machine in the list: the same columns for every kind, so they line up down the page:
     picture, name and model, what it does now, how it connects, last seen, its state. */
  .machine { display: grid; grid-template-columns: 56px minmax(0, 1.6fr) minmax(0, 1.5fr) minmax(0, 1fr) 112px; align-items: center; gap: 4px 20px; padding: 14px 18px; margin-bottom: 10px; background: var(--card); border: 1px solid var(--line); border-radius: 14px; }
  .m-col { display: grid; gap: 2px; min-width: 0; }
  .m-col > * { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .m-col b { font-weight: 500; font-variant-numeric: tabular-nums; }
  .m-col small { color: var(--muted); font-size: 13px; }
  .name { font-weight: 600 !important; font-size: 17px; }
  .machine .state { justify-self: stretch; text-align: center; }
  .m-spool { display: flex; align-items: center; gap: 6px; }
  .m-swatch { display: inline-block; width: 10px; height: 10px; border-radius: 50%; border: 1px solid var(--line); flex: none; }
  @media (max-width: 900px) { .machine { grid-template-columns: 56px minmax(0, 1fr) 104px; } .m-conn { display: none; } .m-now { grid-column: 2; } }
  .state { justify-self: end; font-weight: 700; letter-spacing: 1px; text-transform: uppercase; font-size: 12px; padding: 4px 10px; border-radius: 6px; background: transparent; color: var(--muted); box-shadow: inset 0 0 0 1.5px var(--line); }
  .state.busy { background: var(--accent); color: #fff; box-shadow: none; }
  .state.paused { background: var(--warn); color: #fff; box-shadow: none; }
  .state.offline, .state.disconnected { color: var(--off); box-shadow: inset 0 0 0 1.5px color-mix(in srgb, var(--off) 45%, transparent); }
  .state.error { background: var(--bad); color: #fff; box-shadow: none; }
  .state.attention { background: var(--warn); color: #fff; box-shadow: none; }
  .state.cooling { color: var(--accent); box-shadow: inset 0 0 0 1.5px var(--accent); }
  .state.out { background: transparent; color: var(--muted); box-shadow: inset 0 0 0 1.5px var(--off); }
  .attention-count b { color: var(--warn) !important; }
  .thumb { width: 56px; height: 56px; border-radius: 10px; background: var(--line); }
  .empty { color: var(--muted); }
  .card-progress { grid-column: 2 / -1; height: 6px; margin-top: 8px; border-radius: 3px; background: var(--line); overflow: hidden; }
  .card-progress i { display: block; height: 100%; background: var(--accent); transition: width 0.5s; }
  h2.group { margin: 26px 0 12px; font-size: 15px; color: var(--muted); text-transform: uppercase; letter-spacing: 1px; }
  h2.group span { margin-left: 6px; font-weight: 500; }
  a { color: inherit; }
  a.machine { text-decoration: none; }
  a.machine:hover { border-color: var(--accent); }
  .back { color: var(--muted); text-decoration: none; }
  h2 { margin: 6px 0 4px; font-size: 24px; letter-spacing: -0.3px; }
  dl { display: grid; grid-template-columns: auto 1fr; gap: 8px 18px; margin: 16px 0; }
  dt { color: var(--muted); }
  dd { margin: 0; overflow-wrap: anywhere; }
"#;

const PAGE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Maker Connect</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{style}</style>
</head>
<body>
{header}
<main>
  <div id="machines"><p class="empty">Loading…</p></div>
</main>
<script>
  const esc = (t) => String(t).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const KIND = { fdm: "3D printer", laser: "Laser", cnc: "CNC", cutter: "Cutting plotter", label: "Label printer", paper: "Paper printer", other: "Machine" };
  let profiles = null;
  // A column of a machine's row: a value, and what it is under it.
  const col = (cls, value, caption) => `<div class="m-col ${cls}"><b>${esc(value ?? "–")}</b><small>${esc(caption ?? "")}</small></div>`;
  const LINK = { virtual: "Virtual", "prusa-connect": "Prusa Connect", agent: "Agent", simplyprint: "Moonraker" };
  // What it does now: its job, else what matters for its kind.
  function now(m, t) {
    if (t.job) {
      const percent = Math.round((t.job.done * 100) / Math.max(1, t.job.total));
      return [`${percent} % · ${t.job.file}`, timeLeft(t.job.remaining_s) || "printing"];
    }
    if (t.nozzle_c != null) return [`${t.nozzle_c.toFixed(0)} °C · ${t.bed_c != null ? `${t.bed_c.toFixed(0)} °C` : "–"}`, "nozzle · bed"];
    if (m.kind === "label") {
      const tape = t.cassette === false ? "no cassette" : m.media ? `tape ${m.media}` : "tape not set";
      return [`${tape}${t.battery_v != null ? ` · ${batteryPercent(t.battery_v)} %` : ""}`, t.battery_v != null ? "tape · battery" : "tape"];
    }
    if (t.position_mm) return [`X ${t.position_mm[0].toFixed(1)} · Y ${t.position_mm[1].toFixed(1)}`, "head, mm"];
    return [null, "nothing reported"];
  }
  // Said only when something is off: offline (since when), or late to report.
  function trouble(m) {
    if (m.out_of_use) return `<div class="m-col m-conn"></div>`;
    if (!m.online) return col("m-conn", "Offline", m.seen_seconds_ago == null ? "never reported" : `last seen ${seen(m.seen_seconds_ago)}`);
    if (m.seen_seconds_ago != null && m.seen_seconds_ago > 30) return col("m-conn", "Late", `last report ${seen(m.seen_seconds_ago)}`);
    return `<div class="m-col m-conn"></div>`;
  }
  function seen(seconds) {
    if (seconds == null) return "never";
    if (seconds < 60) return `${seconds} s ago`;
    if (seconds < 3600) return `${Math.round(seconds / 60)} min ago`;
    return `${Math.round(seconds / 3600)} h ago`;
  }
  function render(machines) {
      const list = document.getElementById("machines");
      if (!machines.length) { list.innerHTML = '<p class="empty">No machines yet. <a href="#add" data-add-open>Add one</a>: with the code its agent shows, or a virtual one to try everything.</p>'; return; }
      const card = (m) => {
        const t = m.telemetry || {};
        const shown = shownState(m);
        const percent = t.job ? Math.round((t.job.done * 100) / Math.max(1, t.job.total)) : null;
        const bar = t.job ? `<div class="card-progress"><i style="width: ${percent}%"></i></div>` : "";
        const [doing, what] = now(m, t);
        const profile = profiles[m.profile];
        const thumb = profile && profile.image ? `<img class="thumb" src="profiles/${esc(m.profile)}.svg" alt="">`
          : ["fdm", "laser", "cnc"].includes(m.kind) ? `<img class="thumb" src="profiles/_generic/${esc(m.kind)}.svg" alt="">` : `<span class="thumb"></span>`;
        // The model under the name, unless the name says it already; how it connects with it.
        const model = profile ? (profile.model.includes(profile.brand) ? profile.model : `${profile.brand} ${profile.model}`) : KIND[m.kind] || m.kind;
        const under = [m.name.toLowerCase().includes(model.toLowerCase()) ? null : model, LINK[m.link] || m.link].filter(Boolean).join(" · ");
        return `<a class="machine" href="machines/${encodeURIComponent(m.id)}">
          ${thumb}
          <div class="m-col"><b class="name">${esc(m.name)}</b><small>${esc(under)}</small>${m.loaded ? `<small class="m-spool"><i class="m-swatch" style="background:${esc(m.loaded.color || "transparent")}"></i>${esc(loadedText(m.loaded))}</small>` : ""}</div>
          ${col("m-now", doing, what)}
          ${trouble(m)}
          <div class="state ${esc(shown.cls)}">${esc(shown.text)}</div>
          ${bar}
        </a>`;
      };
      // By kind, so a laser does not stand between the printers; by name within each.
      const GROUPS = [["fdm", "3D printers"], ["laser", "Lasers"], ["cnc", "CNC machines"], ["cutter", "Cutting plotters"], ["label", "Label printers"], ["paper", "Paper printers"], ["other", "Other machines"]];
      list.innerHTML = GROUPS.map(([kind, title]) => {
        const group = machines.filter((m) => (GROUPS.some(([k]) => k === m.kind) ? m.kind : "other") === kind);
        return group.length ? `<h2 class="group">${title} <span>${group.length}</span></h2>${group.map(card).join("")}` : "";
      }).join("");
  }
  // The list comes again on every change (server-sent events); the browser reconnects
  // by itself when the connection drops.
  (async () => {
    try { profiles = Object.fromEntries((await (await fetch("api/profiles")).json()).map((p) => [p.id, p])); } catch (error) { profiles = {}; }
    const stream = new EventSource("api/machines/stream");
    stream.onmessage = (event) => {
      const machines = JSON.parse(event.data);
      render(machines);
      summarise(machines);
    };
    stream.onopen = () => live(true);
    stream.onerror = () => live(false);
  })();
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// The jog pad: diagonals, its steps and speeds only, GRBL's own jogs on a laser.
    #[test]
    fn the_jog_pad_moves_by_its_steps() {
        assert_eq!(
            jog_command("jog:1:-1", 0.1, 600.0, true),
            Some((
                "Move X+0.1 Y−0.1 mm".to_owned(),
                "$J=G91 G21 X0.1 Y-0.1 F600".to_owned()
            ))
        );
        assert_eq!(
            jog_command("jog:-1:0", 50.0, 6000.0, false).unwrap().1,
            "G91\nG1 X-50 F6000\nG90"
        );
        assert_eq!(jog_command("jog-z:1", 10.0, 3000.0, true), None, "no Z");
        assert_eq!(jog_command("jog:0:0", 10.0, 3000.0, true), None);
        assert_eq!(jog_command("jog:2:0", 10.0, 3000.0, true), None);
        assert_eq!(
            jog_command("jog:1:0", 500.0, 3000.0, true),
            None,
            "not a step offered"
        );
        assert_eq!(
            jog_command("jog:1:0", 10.0, 1e9, true),
            None,
            "not a speed offered"
        );
        assert_eq!(grbl_action("set-origin").unwrap().1, "G92 X0 Y0");
    }

    /// Typed temperatures: whole degrees, up to the profile's maximum, 0 is off.
    #[test]
    fn typed_temperatures_stay_within_the_machine() {
        let mk3s = profiles::find("prusa/mk3s");
        assert!(mk3s.is_some());
        assert_eq!(
            heat_command("set-nozzle", Some(212.6), None, mk3s),
            Some(Ok(("Nozzle to 213 °C".to_owned(), "M104 S213".to_owned())))
        );
        assert_eq!(
            heat_command("set-bed", None, Some(0.0), mk3s),
            Some(Ok(("Bed off".to_owned(), "M140 S0".to_owned())))
        );
        assert!(matches!(
            heat_command("set-bed", None, Some(121.0), mk3s),
            Some(Err(_))
        ));
        assert!(matches!(
            heat_command("set-nozzle", Some(-5.0), None, mk3s),
            Some(Err(_))
        ));
        assert!(matches!(
            heat_command("set-nozzle", None, None, mk3s),
            Some(Err(_))
        ));
        assert!(matches!(
            heat_command("set-nozzle", Some(270.0), None, None),
            Some(Err(_))
        ));
        assert_eq!(heat_command("set-origin", None, None, mk3s), None);
        assert_eq!(
            factor_command("set-speed", Some(149.6), None),
            Some(Ok(("Speed 150 %".to_owned(), "M220 S150".to_owned())))
        );
        assert!(matches!(
            factor_command("set-flow", None, Some(30.0)),
            Some(Err(_))
        ));
        assert!(matches!(
            factor_command("set-speed", None, None),
            Some(Err(_))
        ));
        assert_eq!(factor_command("set-bed", None, None), None);
    }
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use maker_connect_protocol::{MachineKind, MachineState};
    use tower::ServiceExt;

    /// A laser on an agent gets a file to run, is told to pause, and says how it ended.
    #[tokio::test]
    async fn an_agent_runs_a_job() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
        // The agent's own requests carry its token, no session: as from a real agent.
        let agent = super::app(store.clone());
        let laser = Register {
            protocol: protocol::VERSION,
            name: "Ortur".into(),
            kind: MachineKind::Laser,
            agent_version: "0.1.0".into(),
        };
        let (_, body) = call(
            &agent,
            json(
                protocol::REGISTER,
                None,
                serde_json::to_string(&laser).unwrap(),
            ),
        )
        .await;
        let registered: Registered = serde_json::from_str(&body).unwrap();
        let space = {
            let store = store.lock().unwrap();
            let space = store.default_space().unwrap().0;
            store.pair(space, &registered.pairing_code).unwrap();
            store
                .set_profile(space, &registered.machine_id, Some("ortur/lm2-pro-s2"))
                .unwrap();
            space
        };
        let report = |telemetry: Telemetry| {
            let app = agent.clone();
            let token = registered.token.clone();
            async move {
                let (_, body) = call(
                    &app,
                    json(
                        protocol::TELEMETRY,
                        Some(&token),
                        serde_json::to_string(&telemetry).unwrap(),
                    ),
                )
                .await;
                serde_json::from_str::<TelemetryReply>(&body).unwrap()
            }
        };
        let idle = || Telemetry {
            state: MachineState::Idle,
            ..Telemetry::default()
        };
        assert_eq!(report(idle()).await.job, None);

        let upload = Request::post("/api/files?name=sign.gcode")
            .body(Body::from("G90\nM4 S0\nG1 X10 Y10 S800 F3000\nM5\n"))
            .unwrap();
        let (_, body) = call(&app, upload).await;
        let file: serde_json::Value = serde_json::from_str(&body).unwrap();
        let print = Request::post(format!("/files/{}/print", file["id"].as_str().unwrap()))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-requested-with", "fetch")
            .body(Body::from(format!("machine={}", registered.machine_id)))
            .unwrap();
        let (status, body) = call(&app, print).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

        let order = report(idle()).await.job.expect("the job is given out");
        assert_eq!(order.name, "sign.gcode");
        let fetch = |token: &str| {
            Request::get(&order.download)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap()
        };
        let (status, gcode) = call(&agent, fetch(&registered.token)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(gcode.contains("M4 S0"));
        // Not with another token, nor without one.
        assert_eq!(
            call(&agent, fetch("not-its-token")).await.0,
            StatusCode::NOT_FOUND
        );
        let bare = Request::get(&order.download).body(Body::empty()).unwrap();
        assert_eq!(call(&agent, bare).await.0, StatusCode::NOT_FOUND);
        // The page's download still needs someone signed in.
        let page_download = format!("/files/{}/download", file["id"].as_str().unwrap());
        let bare = Request::get(&page_download).body(Body::empty()).unwrap();
        assert_eq!(call(&agent, bare).await.0, StatusCode::UNAUTHORIZED);

        let running = || Telemetry {
            state: MachineState::Busy,
            job_id: Some(order.id),
            ..Telemetry::default()
        };
        assert_eq!(report(running()).await.job, None, "given out once");
        store
            .lock()
            .unwrap()
            .set_job(order.id, store::JobState::Paused, None)
            .unwrap();
        assert_eq!(
            report(running()).await.job_control,
            Some(protocol::JobControl::Pause)
        );

        let ended = Telemetry {
            job_end: Some(protocol::JobEnd {
                id: order.id,
                ok: true,
                stopped: false,
                reason: None,
                restart_line: None,
            }),
            ..idle()
        };
        report(ended).await;
        let job = store.lock().unwrap().job(order.id).unwrap().unwrap();
        assert_eq!(job.state, store::JobState::Finished);
        assert!(
            store
                .lock()
                .unwrap()
                .machine(space, &registered.machine_id)
                .unwrap()
                .unwrap()
                .job
                .is_none()
        );
    }

    /// Typed G-code is tidied and kept short enough for every machine.
    #[test]
    fn typed_gcode_is_checked() {
        assert_eq!(
            typed_gcode(" G28 \n\nG1 X10\n"),
            Ok(("G28 (+1 lines)".to_owned(), "G28\nG1 X10".to_owned()))
        );
        assert!(typed_gcode("   ").is_err());
        assert!(typed_gcode(&"G4 P0\n".repeat(21)).is_err());
        assert!(typed_gcode("M117 \u{7}").is_err());
    }

    async fn call(app: &Router, request: Request<Body>) -> (StatusCode, String) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    fn json(path: &str, token: Option<&str>, body: String) -> Request<Body> {
        let mut request = Request::post(path).header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        request.body(Body::from(body)).unwrap()
    }

    /// An agent registers, reports unpaired, is added with its code, and then shows in the
    /// space's list with what it reported.
    /// A label printer without a cassette waits for someone: an older agent's "error" is
    /// read as attention, and the page shows the reason, not an error.
    #[tokio::test]
    async fn a_cassette_out_needs_attention_not_an_error() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
        let register = Register {
            protocol: protocol::VERSION,
            name: "Dymo".into(),
            kind: MachineKind::Label,
            agent_version: "0.1.0".into(),
        };
        let (_, body) = call(
            &app,
            json(
                protocol::REGISTER,
                None,
                serde_json::to_string(&register).unwrap(),
            ),
        )
        .await;
        let registered: Registered = serde_json::from_str(&body).unwrap();
        let space = store.lock().unwrap().default_space().unwrap().0;
        store
            .lock()
            .unwrap()
            .pair(space, &registered.pairing_code)
            .unwrap();
        let old_agent = Telemetry {
            state: MachineState::Error,
            cassette: Some(false),
            problem: Some("no cassette".into()),
            ..Telemetry::default()
        };
        call(
            &app,
            json(
                protocol::TELEMETRY,
                Some(&registered.token),
                serde_json::to_string(&old_agent).unwrap(),
            ),
        )
        .await;
        let machine = store
            .lock()
            .unwrap()
            .machine(space, &registered.machine_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            machine.telemetry.as_ref().unwrap().state,
            MachineState::Attention
        );
        assert_eq!(
            machine_page::shown_state(&machine),
            ("no cassette".to_owned(), "attention".to_owned())
        );
        let page = label_page::render("My space", &machine);
        assert!(page.contains(r#"class="state attention""#));
    }

    #[tokio::test]
    async fn an_agent_registers_is_paired_and_listed() {
        let app =
            crate::auth::signed_in_app(Arc::new(Mutex::new(Store::open(":memory:").unwrap())));
        let register = Register {
            protocol: protocol::VERSION,
            name: "MK3S Gonzo".into(),
            kind: MachineKind::Fdm,
            agent_version: "0.1.0".into(),
        };
        let (status, body) = call(
            &app,
            json(
                protocol::REGISTER,
                None,
                serde_json::to_string(&register).unwrap(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let registered: Registered = serde_json::from_str(&body).unwrap();
        assert_eq!(registered.pairing_code.len(), 6);

        let report = Telemetry {
            state: MachineState::Idle,
            firmware: Some("3.14.0".into()),
            nozzle_c: Some(24.0),
            bed_c: Some(23.0),
            job: None,
            identity: None,
            ..Telemetry::default()
        };
        let telemetry = || {
            json(
                protocol::TELEMETRY,
                Some(&registered.token),
                serde_json::to_string(&report).unwrap(),
            )
        };
        let (_, body) = call(&app, telemetry()).await;
        assert!(
            !serde_json::from_str::<TelemetryReply>(&body)
                .unwrap()
                .paired
        );
        let (_, body) = call(
            &app,
            Request::get("/api/machines").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(body, "[]", "not listed before it is paired");

        let form = Request::post("/pair")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(format!(
                "code={}",
                registered.pairing_code.to_lowercase()
            )))
            .unwrap();
        let (status, _) = call(&app, form).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        let (_, body) = call(&app, telemetry()).await;
        let reply: TelemetryReply = serde_json::from_str(&body).unwrap();
        assert!(reply.paired);
        assert_eq!(reply.space.as_deref(), Some("My space"));

        let (_, body) = call(
            &app,
            Request::get("/api/machines").body(Body::empty()).unwrap(),
        )
        .await;
        let machines: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(machines[0]["name"], "MK3S Gonzo");
        assert_eq!(machines[0]["online"], true);
        assert_eq!(machines[0]["telemetry"]["state"], "idle");
    }

    /// The page's stream sends the list at once, and again when a machine changes.
    #[tokio::test]
    async fn the_list_streams_its_changes() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
        let response = app
            .oneshot(
                Request::get("/api/machines/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let mut body = response.into_body();
        let mut next = async || {
            let frame = body.frame().await.unwrap().unwrap();
            String::from_utf8_lossy(frame.data_ref().unwrap()).into_owned()
        };
        assert_eq!(next().await, "data: []\n\n");
        let (_, _, code) = store
            .lock()
            .unwrap()
            .register(&Register {
                protocol: protocol::VERSION,
                name: "Gonzo".into(),
                kind: MachineKind::Fdm,
                agent_version: "0.1.0".into(),
            })
            .unwrap();
        store.lock().unwrap().pair(1, &code).unwrap();
        let changed = tokio::time::timeout(std::time::Duration::from_secs(2), next())
            .await
            .expect("an event within the merge time, not the 5 s tick");
        assert!(changed.contains(r#""name":"Gonzo""#), "{changed}");
    }

    /// A machine that says it is a MINI gets the MINI's profile; the owner can choose
    /// another (a Zaribo, which says it is an MK3S), and its page draws that bed.
    #[tokio::test]
    async fn a_machine_gets_its_profile() {
        let app =
            crate::auth::signed_in_app(Arc::new(Mutex::new(Store::open(":memory:").unwrap())));
        let register =
            r#"{"protocol":1,"name":"Mini","kind":"fdm","agent_version":"0.1.0"}"#.to_owned();
        let (_, body) = call(&app, json(protocol::REGISTER, None, register)).await;
        let registered: Registered = serde_json::from_str(&body).unwrap();
        let pair = Request::post("/pair")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(format!("code={}", registered.pairing_code)))
            .unwrap();
        call(&app, pair).await;
        let report =
            r#"{"state":"idle","identity":{"machine_type":"Prusa-MINI","model_code":120}}"#
                .to_owned();
        call(
            &app,
            json(protocol::TELEMETRY, Some(&registered.token), report.clone()),
        )
        .await;
        let (_, body) = call(
            &app,
            Request::get("/api/machines").body(Body::empty()).unwrap(),
        )
        .await;
        let machines: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(machines[0]["profile"], "prusa/mini");
        assert_eq!(machines[0]["profile_chosen"], false);

        let id = registered.machine_id;
        let choose = Request::post(format!("/machines/{id}/profile"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("profile=zaribo%2F320"))
            .unwrap();
        let (status, _) = call(&app, choose).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        // Its next report does not undo the choice.
        call(
            &app,
            json(protocol::TELEMETRY, Some(&registered.token), report),
        )
        .await;
        let (status, page) = call(
            &app,
            Request::get(format!("/machines/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(page.contains("Zaribo 320 (MK3S)"), "the chosen profile");
        assert!(
            page.contains(r#"width="250" height="210""#),
            "its bed, to scale"
        );
        assert!(page.contains("250 × 210 × 320 mm"));
    }

    /// An unknown token is refused, and an agent of another protocol version is told so.
    #[tokio::test]
    async fn unknown_agents_and_versions_are_refused() {
        let app =
            crate::auth::signed_in_app(Arc::new(Mutex::new(Store::open(":memory:").unwrap())));
        let report = r#"{"state":"idle"}"#.to_owned();
        let (status, _) = call(&app, json(protocol::TELEMETRY, Some("nope"), report)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let old = r#"{"protocol":0,"name":"x","kind":"fdm","agent_version":"0"}"#.to_owned();
        let (status, body) = call(&app, json(protocol::REGISTER, None, old)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("protocol 1"));
    }
}
