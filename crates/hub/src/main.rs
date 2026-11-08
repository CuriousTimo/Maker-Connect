//! # Maker Connect hub
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

mod about;
mod access;
mod access_page;
mod accounts;
mod apps;
mod auth;
mod codes;
mod downloads;
mod farm;
mod files;
mod files_page;
mod gcode;
mod label_page;
mod machine_page;
mod now_page;
mod permissions_page;
mod printer_files;
mod profiles;
mod projects_page;
mod prusa_connect;
mod queue;
mod safety;
mod samples;
mod sd_card;
mod simplyprint;
mod space_page;
mod spoolman;
mod spoolmandb;
mod stock;
mod store;
mod updates;
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

const USAGE: &str = "Maker Connect hub (proof of concept)

Usage:
  maker-connect-hub [--listen 0.0.0.0:8460] [--db maker-connect.db]
  maker-connect-hub reset-password <name> [--db maker-connect.db]

Open http://localhost:8460. On a fresh install, make the first account with the setup code
this prints at start; then add machines with the code their agent shows.

reset-password gives an account a new password (printed once) and signs it out everywhere:
for when its owner cannot sign in any more.
";

/// What a release's archive brings beside the hub, into the folders beside its database.
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
    // A new build tried before it is put in place (crate::updates): it opens the database (a
    // copy), builds its pages, takes a port, and ends.
    let mut trial = false;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--listen" => listen = args.next().ok_or("expected an address after --listen")?,
            "--db" => db = args.next().ok_or("expected a file after --db")?,
            "--trial" => trial = true,
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
    if trial {
        let opened = Store::open(&db)?;
        let _app = app(Arc::new(Mutex::new(opened)));
        let _port = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        println!("trial: {} opens its database and starts", store::BUILD);
        return Ok(());
    }
    // An update under way: this build on trial, or one to go back from.
    let exe = std::env::current_exe()?;
    let again: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let on_trial = if reset.is_some() {
        None
    } else {
        match maker_connect_update::on_start(&exe) {
            maker_connect_update::Start::Plain => None,
            maker_connect_update::Start::OnTrial(t) => {
                println!("updated from {} to {}: on trial", t.from, t.to);
                Some(t)
            }
            maker_connect_update::Start::RollBack(t, why) => go_back(&exe, &again, &t, &why),
        }
    };
    let mut opened = match Store::open(&db) {
        Ok(store) => store,
        Err(error) => match &on_trial {
            Some(t) => go_back(
                &exe,
                &again,
                t,
                &format!("it could not open the database: {error}"),
            ),
            None => return Err(error.into()),
        },
    };
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
    tokio::spawn(farm::run(store.clone()));
    tokio::spawn(samples::run(store.clone()));
    tokio::spawn(queue::run(store.clone()));
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(listener) => listener,
        Err(error) => match &on_trial {
            Some(t) => go_back(
                &exe,
                &again,
                t,
                &format!("it could not listen on {listen}: {error}"),
            ),
            None => return Err(error.into()),
        },
    };
    println!("Maker Connect hub on http://{listen} (data in {db})");
    // A new build that serves for a while works: it stays.
    if let Some(t) = on_trial {
        let exe = exe.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            maker_connect_update::confirm(&exe, &t);
            println!("{} works: the update is done", t.to);
        });
    }
    if let Some(code) = store.lock().unwrap().setup_code.clone() {
        println!("no accounts yet: open it in a browser and make the first with setup code {code}");
    }
    let serving = axum::serve(
        listener,
        app(store.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(stop_asked());
    tokio::select! {
        served = serving => return Ok(served?),
        // A new build is in place (crate::updates): this one stops serving (its port free).
        () = updates::UPDATE.notified() => {}
    }
    // The new build runs as this one's child, on trial; this one waits, its store held so
    // nothing of it runs meanwhile. Then the new build for good, or this one back.
    let (program, args) = (exe.clone(), again.clone());
    let tried = tokio::task::spawn_blocking(move || {
        let _held = store.lock().unwrap();
        maker_connect_update::supervise(&program, &args)
    })
    .await?;
    match tried {
        Ok(maker_connect_update::Supervised::Confirmed) => {
            println!("the new build works: starting it for good");
            maker_connect_update::restart(&exe, &again)
        }
        Ok(maker_connect_update::Supervised::Failed(t, why)) => go_back(&exe, &again, &t, &why),
        Err(error) => {
            eprintln!("could not try the new build: {error}");
            maker_connect_update::restart(&exe, &again)
        }
    }
}

/// Back to the build before an update that did not work: its database as it was, then that
/// build in this one's place.
fn go_back(
    exe: &std::path::Path,
    again: &[std::ffi::OsString],
    trial: &maker_connect_update::Trial,
    why: &str,
) -> ! {
    eprintln!(
        "the update to {} goes back to {}: {why}",
        trial.to, trial.from
    );
    updates::restore_database(trial);
    if let Err(error) = maker_connect_update::roll_back(exe, trial, why) {
        eprintln!("could not go back: {error}");
        std::process::exit(1);
    }
    maker_connect_update::restart(exe, again)
}

/// Ends when someone stops the hub (Ctrl-C, or `kill`/systemd/Docker's SIGTERM): no new
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
        .route("/pair", post(pair))
        .route("/api/machines", get(machines))
        .route("/api/machines/stream", get(machine_stream))
        // The machine list; the home page is Now (now_page).
        .route("/machines", get(page))
        .route("/machines/", get(|| async { Redirect::to("/machines") }))
        .route("/logo.svg", get(logo))
        .route("/machines/{id}", get(machine_page))
        .route("/machines/{id}/profile", post(choose_profile))
        .route("/machines/{id}/media", post(choose_media))
        .route("/machines/{id}/name", post(rename))
        .route("/machines/{id}/labels", post(set_labels))
        .route("/machines/{id}/command", post(command))
        .route("/machines/{id}/remove", post(remove))
        .route("/machines/{id}/move", post(move_machine))
        .route("/machines/{id}/out-of-use", post(out_of_use))
        .route("/machines/{id}/taken", post(taken))
        .route("/machines/{id}/answer", post(answer))
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
        .merge(updates::routes())
        .merge(about::routes())
        .merge(codes::routes())
        .merge(stock::routes())
        .route("/files", get(files_page::page))
        .merge(auth::routes())
        .merge(space_page::routes())
        .merge(permissions_page::routes())
        .merge(safety::routes())
        .merge(samples::routes())
        .merge(queue::routes())
        .merge(apps::routes())
        .merge(printer_files::routes())
        .merge(sd_card::routes())
        .merge(now_page::routes())
        .merge(farm::routes())
        .merge(projects_page::routes())
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
                "this hub speaks protocol {}, the agent {}",
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
        // What an SD card listing said, whole (before the terminal keeps only its end).
        if let Ok(Some(text)) = store.command_text(token, done.id)
            && text.starts_with("M20")
            && let Ok(Some(machine)) = store.machine_id_by_token(token)
        {
            let files = sd_card::listing(&done.output);
            store.sd_files.insert(machine, (files, store::now()));
        }
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
            let update = match updates::order_for(&store, token, &report) {
                Ok(update) => update,
                Err(error) => return failed(error),
            };
            // Someone's answer to the question on its screen, once; and pause, resume or stop
            // for a print it started itself (no job of the hub's).
            let (answer, job_control) = match store.machine_id_by_token(token) {
                Ok(Some(machine)) => (
                    store.answers.remove(&machine),
                    job_control.or_else(|| store.own_job_control.remove(&machine)),
                ),
                _ => (None, job_control),
            };
            Json(TelemetryReply {
                paired: space.is_some(),
                space,
                commands,
                job,
                job_control,
                update,
                answer,
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

/// Where a request came from: the address it reached the hub from, or, behind a reverse
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
/// Why a job is paused that a power cut stopped and its agent kept.
const POWER_CUT: &str = "a power cut stopped it: resume it here or on the printer";

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
    // A job a power cut stopped (Prusa's MK3; the agent kept it): it failed here when the
    // agent no longer ran it, and comes back as the agent reports it again, paused until
    // someone resumes it.
    if let Some(id) = report.job_id
        && matches!(report.state, MachineState::Paused | MachineState::Busy)
        && store.active_job(&machine)?.is_none()
        && let Some(lost) = store.job(id)?
        && lost.machine_id == machine
        && lost.state == JobState::Failed
    {
        let state = if report.state == MachineState::Paused {
            JobState::Paused
        } else {
            JobState::Printing
        };
        store.revive_job(id, state, Some(POWER_CUT))?;
    }
    let Some(job) = store.active_job(&machine)? else {
        return Ok((None, None));
    };
    // Resumed from here after the power cut: its reason goes (a later pause is an ordinary
    // one); the agent is told to resume below as for any job.
    if job.state == JobState::Printing && job.reason.as_deref() == Some(POWER_CUT) {
        store.revive_job(job.id, JobState::Printing, None)?;
    }
    // Taken up on the printer's own screen after the power cut: printing again, not to be
    // paused from here.
    if job.state == JobState::Paused
        && job.reason.as_deref() == Some(POWER_CUT)
        && report.job_id == Some(job.id)
        && report.state == MachineState::Busy
    {
        // Its reason cleared too: a later pause from here is an ordinary one.
        store.revive_job(job.id, JobState::Printing, None)?;
        return Ok((None, None));
    }
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
    let paired = store.space_of(&access).and_then(|(space, _)| {
        let paired = store.pair(space, &form.code)?;
        Ok(paired)
    });
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
        r##"<script>try {{ const t = localStorage.getItem("theme"); if (t === "light" || t === "dark") document.documentElement.dataset.theme = t; }} catch {{}}</script>
<header class="topbar">
  <a class="brand" href="{home}"><img src="{root}logo.svg" alt="" width="28" height="28"><span><b>Maker Connect</b><small>{space}</small></span></a>
  <nav class="side-nav" aria-label="Pages">{now}{machines}{projects}{files}{stock}{space_link}</nav>
  <div class="top-right" data-top-right>
    <div class="summary">
      <span class="count" title="Machines in this space"><svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="4" width="14" height="10" rx="1.5"/><path d="M7 17h6M10 14v3"/></svg><b data-sum="machines">–</b><span class="sr">machines</span></span>
      <span class="count" title="Online: reporting now"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3.5 8a9.5 9.5 0 0 1 13 0M6 11a6 6 0 0 1 8 0"/><circle cx="10" cy="14.5" r="1.2"/></svg><b data-sum="online">–</b><span class="sr">online</span></span>
      <span class="count busy-count" title="Busy: printing, cutting, running a job"><svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="6.5"/><path d="M10 6.5V10l2.5 2"/></svg><b data-sum="busy">–</b><span class="sr">busy</span></span>
      <span class="count attention-count" data-attention-count hidden title="Need someone: a plate to confirm, a print to take, an error"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 3.5 17 16H3z"/><path d="M10 8.5v3.5M10 14v.01"/></svg><b data-sum="attention">–</b><span class="sr">need someone</span></span>
      <span class="live" data-live-dot title="Live updates"><i></i></span>
    </div>
    <button type="button" class="search-open" data-palette-open title="Go to a machine, a file or a page"><svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="9" cy="9" r="5.5"/><path d="m13.2 13.2 3.8 3.8"/></svg><span>Go to…</span><kbd>⌘K</kbd></button>
  </div>
</header>
<aside class="sidebar" data-root="{home}">
  <div class="side-section">
    <div class="side-title"><span>Machines</span><button type="button" class="side-add" data-add-open title="Add a machine" aria-label="Add a machine"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 4.5v11M4.5 10h11"/></svg></button></div>
    <nav class="side-machines" data-side-machines aria-label="Machines"></nav>
  </div>
</aside>
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
      <button type="button" class="choice" data-go="virtual"><b>A virtual one</b><small>Simulated in the hub, to try everything without a machine</small></button>
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
      <li>Start it with this hub's address and the machine's port: the Downloads page gives the command.</li>
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
    <p class="dialog-sub">Simulated in the hub: it moves, heats and measures its bed. Pick its machine.</p>
    <div class="picker" data-picker data-action="{home}virtual"></div>
  </section>
</dialog>
<script>
{SHELL_SCRIPT}</script>"##,
        space = escape(space),
        now = link(
            "",
            "Now",
            "now",
            r#"<circle cx="10" cy="10" r="7"/><path d="M10 6v4l2.5 2.5"/>"#
        ),
        machines = link(
            "machines",
            "Machines",
            "machines",
            r#"<rect x="2.5" y="4" width="15" height="10" rx="2"/><path d="M7 17h6M10 14v3"/>"#
        ),
        projects = link(
            "projects",
            "Projects",
            "projects",
            r#"<rect x="3" y="4" width="14" height="13" rx="2"/><path d="M7 8h6M7 11h6M7 14h3"/>"#
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
    match store.space_of(&access).and_then(|(space, _)| {
        let id = store.add_virtual(space, profile)?;
        Ok(id)
    }) {
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
    // Switching things off never waits for PPE; everything else does.
    let off = form.gcode.trim().is_empty()
        && matches!(
            form.action.as_str(),
            "cooldown" | "laser-off" | "fan-off" | "motors-off"
        );
    if !off && let Err(refused) = crate::safety::check_ppe(&store, &access, &id) {
        return *refused;
    }
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
    } else if matches!(form.action.as_str(), "load-filament" | "unload-filament") {
        // Hot first, to the loaded spool's temperature (215 °C when it is not known), then the
        // firmware's own load or unload (Marlin's and Prusa's M701, M702).
        let machine = match store.machine(space, &id) {
            Ok(Some(machine)) => machine,
            Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
            Err(error) => return failed(error),
        };
        if machine.kind != protocol::MachineKind::Fdm {
            return (StatusCode::BAD_REQUEST, "filament goes into a 3D printer").into_response();
        }
        let hot = machine
            .loaded
            .as_ref()
            .and_then(|l| l.nozzle_c)
            .filter(|c| (150.0..=320.0).contains(c))
            .unwrap_or(215.0);
        Some(if form.action == "load-filament" {
            (
                format!("Load filament ({hot:.0} °C)"),
                format!("M109 S{hot:.0}\nM701"),
            )
        } else {
            (
                format!("Unload filament ({hot:.0} °C)"),
                format!("M109 S{hot:.0}\nM702"),
            )
        })
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

#[derive(serde::Deserialize)]
struct MoveForm {
    /// The space it goes to, by its id under the hood.
    space: String,
}

/// The other spaces someone who manages a machine could move it to: where they are owner or
/// admin too. (uid, "Organisation · Space"); none for a token.
fn move_targets(store: &Store, access: &access::Access, machine: &str) -> Vec<(String, String)> {
    let manages = store
        .may(access, machine, access::Level::Manage)
        .ok()
        .flatten()
        .unwrap_or(false);
    if !manages || access.token_id.is_some() {
        return Vec::new();
    }
    store
        .spaces_of(access.user_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.id != access.space && s.role >= access::Role::Admin)
        .map(|s| (s.uid, format!("{} · {}", s.organisation, s.name)))
        .collect()
}

/// Moves a machine to another space of someone who manages both.
async fn move_machine(
    access: crate::access::Access,
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Form(form): Form<MoveForm>,
) -> Response {
    if access.token_id.is_some() {
        return (
            StatusCode::FORBIDDEN,
            "a token belongs to one space: move machines signed in",
        )
            .into_response();
    }
    let mut store = store.lock().unwrap();
    let target = match store.space_by_uid(form.space.trim()) {
        Ok(found) => found,
        Err(error) => return failed(error),
    };
    // A space they are not owner or admin of is one they may not put machines in; whether it
    // exists is none of their business either.
    let Some(to) = target.filter(|to| {
        store
            .role_of(access.user_id, *to)
            .ok()
            .flatten()
            .is_some_and(|r| r >= access::Role::Admin)
    }) else {
        return (
            StatusCode::FORBIDDEN,
            "you are not owner or admin of that space",
        )
            .into_response();
    };
    let name = match store.machine(access.space, &id) {
        Ok(Some(m)) => m.name,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => return failed(error),
    };
    match store.move_machine(access.space, &id, to) {
        Ok(Ok(())) => {
            let (from_name, to_name) = (
                store.space_name(access.space).unwrap_or_default(),
                store.space_name(to).unwrap_or_default(),
            );
            let _ = store.audit(
                &access,
                "moved to another space",
                &name,
                &format!("to {to_name}"),
            );
            let there = access::Access {
                space: to,
                ..access.clone()
            };
            let _ = store.audit(&there, "moved here", &name, &format!("from {from_name}"));
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Err(why)) => (StatusCode::CONFLICT, why).into_response(),
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

#[derive(serde::Deserialize)]
struct LabelsForm {
    /// Comma-separated: `MK4-BL, Room 2`.
    labels: String,
}

/// A machine's labels, as typed: to find it, filter on and group by.
async fn set_labels(
    access: access::Access,
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Form(form): Form<LabelsForm>,
) -> Response {
    let labels: Vec<String> = form.labels.split(',').map(str::to_owned).collect();
    match store.lock().unwrap().set_labels(access.space, &id, &labels) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
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

#[derive(serde::Deserialize)]
struct AnswerForm {
    /// The button pressed, by its place among the question's buttons (from 0).
    button: usize,
}

/// A button of the question on a printer's screen, pressed from the website: a Prusa on
/// Connect gets `DIALOG_ACTION` (its dialog and the button's name), an agent's printer
/// `M876 S<index>` (with its next report).
async fn answer(
    access: crate::access::Access,
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Form(form): Form<AnswerForm>,
) -> Response {
    let mut store = store.lock().unwrap();
    let space = match store.space_of(&access) {
        Ok((space, _)) => space,
        Err(error) => return failed(error),
    };
    let machine = match store.machine(space, &id) {
        Ok(Some(m)) => m,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => return failed(error),
    };
    if let Some((buttons, dialog)) = store.dialogs.get(&id).cloned() {
        let Some(name) = buttons.get(form.button) else {
            return (StatusCode::BAD_REQUEST, "no such button").into_response();
        };
        let mut kwargs = serde_json::json!({ "button": name });
        if let Some(dialog) = dialog {
            kwargs["dialog_id"] = serde_json::json!(dialog);
        }
        let command = serde_json::json!({ "command": "DIALOG_ACTION", "kwargs": kwargs });
        return match store.queue_connect_command(space, &id, &format!("Answer: {name}"), &command, false) {
            Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
            Ok(None) => (StatusCode::CONFLICT, "the printer takes no commands").into_response(),
            Err(error) => failed(error),
        };
    }
    let buttons = machine.buttons.len();
    if buttons == 0 {
        return (StatusCode::CONFLICT, "it asks nothing now").into_response();
    }
    if form.button >= buttons || form.button > u8::MAX as usize {
        return (StatusCode::BAD_REQUEST, "no such button").into_response();
    }
    store.answers.insert(id, form.button as u8);
    StatusCode::NO_CONTENT.into_response()
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
            let move_to = move_targets(&store, &access, &machine.id);
            let manage = store
                .may(&access, &machine.id, access::Level::Manage)
                .ok()
                .flatten()
                .unwrap_or(false);
            Html(label_page::render(&space, &machine, &move_to, manage)).into_response()
        }
        Ok((space, Some(machine))) => {
            let extra = machine_page::Extra {
                me: access.user_id,
                staff: access.role >= access::Role::Admin,
                asked: store.asked(access.user_id, &machine.id).ok().flatten(),
                ppe_until: store.ppe_until(access.user_id, &machine.id),
                ppe_checks: store.ppe_checks(access.space),
                may_use: store
                    .may(&access, &machine.id, access::Level::Use)
                    .ok()
                    .flatten()
                    .unwrap_or(false),
                move_to: move_targets(&store, &access, &machine.id),
                manage: store
                    .may(&access, &machine.id, access::Level::Manage)
                    .ok()
                    .flatten()
                    .unwrap_or(false),
                jobs: store
                    .jobs_of(access.space, &machine.id, 10)
                    .unwrap_or_default(),
                stats: store
                    .job_stats(access.space, &machine.id)
                    .unwrap_or_default(),
            };
            Html(machine_page::render(&space, &machine, &extra)).into_response()
        }
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

pub(crate) const STYLE: &str = r#"  :root { color-scheme: light dark; --bg: #f6f5f2; --card: #fff; --ink: #1d1d1b; --muted: #6b6b66; --line: #e2e0da; --accent: #2f7d6d; --off: #9a9893; --bad: #b3412e; --warn: #b7791f; --busy: #2f6fd1; }
  @media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { --bg: #111214; --card: #1b1c1f; --ink: #f2f1ee; --muted: #a3a29d; --line: #2e2f33; --accent: #5fbfa9; --off: #77767a; --bad: #e07a63; --warn: #d9a441; --busy: #6ea8ff; } }
  :root[data-theme="dark"] { --bg: #111214; --card: #1b1c1f; --ink: #f2f1ee; --muted: #a3a29d; --line: #2e2f33; --accent: #5fbfa9; --off: #77767a; --bad: #e07a63; --warn: #d9a441; --busy: #6ea8ff; }
  * { box-sizing: border-box; }
  /* Hidden is hidden, whatever display a class gives (a notice is flex). */
  [hidden] { display: none !important; }
  body { margin: 0; background: var(--bg); color: var(--ink); font: 15px/1.45 system-ui, sans-serif; }
  /* The app's shell: a sidebar beside every page, the page beside it at one width. */
  :root[data-theme="light"] { color-scheme: light; } :root[data-theme="dark"] { color-scheme: dark; }
  .theme-pick { display: flex; gap: 2px; padding: 4px 4px 2px; }
  .theme-pick button { flex: 1; text-align: center !important; font-size: 13px !important; padding: 5px 4px !important; }
  .theme-pick button[aria-pressed="true"] { background: var(--line); }
  :root { --side: 248px; --fdm: #2f7d6d; --laser: #d9692b; --label: #7d5bd6; }
  @media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { --fdm: #5fbfa9; --laser: #f08a4b; --label: #a98bf5; } }
  :root[data-theme="dark"] { --fdm: #5fbfa9; --laser: #f08a4b; --label: #a98bf5; }
  @view-transition { navigation: auto; }
  ::view-transition-old(root), ::view-transition-new(root) { animation-duration: .18s; }
  .sidebar { view-transition-name: sidebar; position: fixed; inset: var(--top) auto 0 0; width: var(--side); z-index: 10; display: flex; flex-direction: column; gap: 6px; padding: 16px 12px; background: color-mix(in srgb, var(--card) 70%, var(--bg)); border-right: 1px solid var(--line); overflow: hidden; }
  /* The pages in a bar at the top; the sidebar holds only the machines. */
  :root { --top: 56px; }
  body { padding-left: var(--side); padding-top: var(--top); }
  .brand { display: flex; align-items: center; gap: 10px; padding: 0 8px 0 4px; text-decoration: none; width: calc(var(--side) - 28px); flex: none; }
  .brand span { display: grid; line-height: 1.2; min-width: 0; }
  .brand b { font-size: 16px; letter-spacing: -0.2px; }
  .brand small { color: var(--muted); font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sidebar svg, .topbar svg { width: 18px; height: 18px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; flex: none; }
  .search-open { display: flex; align-items: center; gap: 8px; width: 100%; padding: 7px 10px; margin-bottom: 6px; background: var(--bg); color: var(--muted); border: 1px solid var(--line); font-weight: 500; font-size: 14px; }
  .search-open span { flex: 1; text-align: left; }
  kbd { font: 11px ui-monospace, Menlo, monospace; padding: 1px 5px; border: 1px solid var(--line); border-radius: 4px; color: var(--muted); }
  .side-nav { display: grid; gap: 2px; }
  .side-nav a { display: flex; align-items: center; gap: 10px; padding: 8px 10px; border-radius: 8px; color: var(--muted); text-decoration: none; font-weight: 600; font-size: 14px; }
  .side-nav a:hover { color: var(--ink); background: color-mix(in srgb, var(--line) 60%, transparent); }
  .side-nav a[aria-current=page] { color: var(--ink); background: var(--line); }
  /* Only the machine list scrolls: the pages above and the user below stay put. */
  .side-section { margin-top: 14px; min-height: 0; flex: 1 1 auto; display: flex; flex-direction: column; }
  .side-title { display: flex; align-items: center; justify-content: space-between; padding: 0 6px 6px 10px; color: var(--muted); font-size: 11px; font-weight: 700; letter-spacing: 1px; text-transform: uppercase; }
  .side-add { background: transparent; color: var(--muted); padding: 3px; border-radius: 6px; display: grid; }
  .side-add:hover { color: var(--ink); background: var(--line); }
  .side-add svg { width: 16px; height: 16px; }
  .side-machines { display: grid; gap: 1px; align-content: start; min-height: 0; overflow-y: auto; overscroll-behavior: contain; }
  .side-machines a { position: relative; display: grid; grid-template-columns: auto 1fr auto; align-items: center; gap: 9px; padding: 7px 10px; border-radius: 8px; color: var(--ink); text-decoration: none; font-size: 14px; }
  .side-machines a:hover { background: color-mix(in srgb, var(--line) 60%, transparent); }
  .side-machines a[aria-current=page] { background: var(--line); }
  .side-machines a::before { content: ""; position: absolute; left: 0; top: 9px; bottom: 9px; width: 3px; border-radius: 2px; background: var(--k, transparent); }
  .side-machines a[data-kind=fdm] { --k: var(--fdm); } .side-machines a[data-kind=laser] { --k: var(--laser); } .side-machines a[data-kind=label] { --k: var(--label); }
  .side-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .side-machines small { color: var(--muted); font-size: 12px; max-width: 90px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .side-machines small.attention { color: var(--warn); } .side-machines small.error { color: var(--bad); }
  /* A file dragged from Files: the machines take it. */
  body.dragging-file .side-machines a { box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 35%, transparent); }
  .side-machines a.drop { background: color-mix(in srgb, var(--accent) 22%, transparent); box-shadow: inset 0 0 0 2px var(--accent); }
  .side-progress { grid-column: 2 / -1; height: 3px; border-radius: 2px; background: color-mix(in srgb, var(--busy) 28%, transparent); overflow: hidden; }
  .side-progress i { display: block; height: 100%; background: var(--busy); transition: width .5s; }
  .side-empty { margin: 4px 10px; color: var(--muted); font-size: 13px; }
  /* States, one list for badges and dots: quiet unless someone should look. Free is a ring
     (on, nothing to do), busy is filled, attention amber, an error red. */
  .dot { width: 9px; height: 9px; border-radius: 50%; background: var(--off); flex: none; }
  .dot.idle { background: transparent; box-shadow: inset 0 0 0 2px var(--accent); }
  /* Green: available. Blue: busy. */
  .dot.busy { background: var(--busy); box-shadow: 0 0 0 3px color-mix(in srgb, var(--busy) 25%, transparent); }
  .dot.paused, .dot.attention { background: var(--warn); } .dot.error { background: var(--bad); }
  .dot.cooling { background: var(--busy); } .dot.out { background: transparent; box-shadow: inset 0 0 0 1.5px var(--off); }
  /* The counts, at the top of every page. */
  .topbar { view-transition-name: topbar; position: fixed; inset: 0 0 auto 0; height: var(--top); z-index: 11; display: flex; align-items: center; gap: 14px; padding: 0 16px; background: color-mix(in srgb, var(--card) 70%, var(--bg)); border-bottom: 1px solid var(--line); box-sizing: border-box; }
  .top-right { margin-left: auto; display: flex; align-items: center; gap: 14px; min-width: 0; }
  .topbar .side-nav { display: flex; gap: 2px; min-width: 0; overflow-x: auto; scrollbar-width: none; }
  .topbar .side-nav::-webkit-scrollbar { display: none; }
  .topbar .side-nav a { padding: 7px 10px; gap: 7px; white-space: nowrap; }
  .topbar .side-nav svg { width: 17px; height: 17px; }
  .topbar .search-open { width: auto; margin: 0; padding: 6px 10px; }
  .topbar .search-open span { flex: none; }
  .topbar .side-user { margin: 0; padding: 0; border: 0; }
  .topbar .side-user summary { padding: 4px 6px 4px 10px; }
  .topbar .side-user .who { max-width: 160px; }
  .topbar .side-menu { top: calc(100% + 8px); bottom: auto; left: auto; right: 0; width: 260px; max-height: calc(100vh - var(--top) - 24px); }
  @media (max-width: 1180px) { .brand { width: auto; } .brand span, .topbar .side-user .who small, .topbar .search-open span, .topbar .search-open kbd { display: none; } }
  @media (max-width: 980px) { .topbar .side-nav a span { display: none; } .topbar .side-user .who { display: none; } }
  .summary { display: flex; align-items: center; gap: 18px; color: var(--muted); font-size: 14px; white-space: nowrap; }
  .summary b { color: var(--ink); font-variant-numeric: tabular-nums; }
  /* A count is an icon and a number; the word is in its hover (and for screen readers). */
  .summary .count { display: inline-flex; align-items: center; gap: 5px; cursor: default; }
  .summary .count svg { width: 17px; height: 17px; }
  .summary .busy-count svg { color: var(--busy); } .summary .attention-count svg, .summary .attention-count b { color: var(--warn); }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
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
  .side-user { flex: none; margin-top: auto; border-top: 1px solid var(--line); padding-top: 8px; position: relative; }
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
    /* A phone: no machine list at the side, the pages in a bar at the bottom. */
    body { padding-left: 0; padding-bottom: 72px; }
    .sidebar { display: none; }
    .topbar { padding: 0 10px; gap: 8px; }
    .topbar .side-nav { position: fixed; inset: auto 0 0 0; height: 64px; justify-content: space-around; padding: 6px 8px; background: color-mix(in srgb, var(--card) 70%, var(--bg)); border-top: 1px solid var(--line); overflow: visible; }
    .topbar .side-nav { gap: 0; padding: 6px 2px; }
    .topbar .side-nav a { flex: 1; min-width: 0; flex-direction: column; gap: 2px; font-size: 10.5px; padding: 6px 2px; }
    .topbar .side-nav a span { overflow: hidden; text-overflow: ellipsis; max-width: 100%; }
    .topbar .side-nav a span { display: block; }
    .summary { gap: 12px; font-size: 13px; }
    html, body { overflow-x: hidden; }
    main > * { max-width: 100%; }
  }

  dialog { width: min(600px, calc(100vw - 32px)); padding: 0; border: 1px solid var(--line); border-radius: 16px; background: var(--card); color: var(--ink); box-shadow: 0 20px 60px rgb(0 0 0 / 0.35); }
  dialog::backdrop { background: rgb(0 0 0 / 0.45); backdrop-filter: blur(2px); }
  /* A question before something is done (shell.js ask): in the middle, short. */
  dialog.ask { width: min(440px, calc(100vw - 32px)); padding: 22px 22px 18px; }
  dialog.ask p { margin: 0; font-size: 16px; line-height: 1.45; overflow-wrap: anywhere; }
  .ask-buttons { display: flex; justify-content: flex-end; gap: 10px; margin-top: 20px; }
  .ask-buttons .ghost { background: transparent; color: var(--ink); border: 1px solid var(--line); }
  .ask-buttons .danger { background: var(--bad); }
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
  /* The machine list's tools: chips with counts, a search, grouping, the view. */
  .list-tools { display: flex; flex-wrap: wrap; gap: 10px 16px; align-items: center; justify-content: space-between; margin-bottom: 14px; }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; }
  .chip { display: inline-flex; gap: 6px; align-items: baseline; padding: 5px 12px; border-radius: 999px; border: 1px solid var(--line); background: transparent; color: var(--muted); font-size: 13px; font-weight: 600; }
  .chip b { color: var(--ink); font-variant-numeric: tabular-nums; }
  .chip[aria-pressed=true] { background: var(--line); color: var(--ink); border-color: var(--muted); }
  .chip.busy b { color: var(--accent); } .chip.needs b { color: var(--warn); } .chip.error b { color: var(--bad); } .chip.offline b, .chip.out b { color: var(--off); }
  .list-controls { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .list-controls input[type=search] { width: 220px; }
  .view-switch { display: inline-flex; border: 1px solid var(--line); border-radius: 8px; overflow: hidden; }
  .view-switch button { background: transparent; color: var(--muted); padding: 6px 8px; border-radius: 0; display: grid; }
  .view-switch button[aria-pressed=true] { background: var(--line); color: var(--ink); }
  .view-switch svg { width: 18px; height: 18px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; }
  /* A plate that ended and waits for someone (the farm), on every page that shows it. */
  .notice.confirm { display: flex; align-items: center; justify-content: space-between; gap: 10px 16px; flex-wrap: wrap; padding: 10px 14px; margin: -4px 0 12px; border-radius: 10px; border: 1px solid var(--warn); background: color-mix(in srgb, var(--warn) 10%, var(--card)); }
  .notice.confirm span { flex: 1 1 300px; }
  .notice.confirm form { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; margin: 0; }
  .notice.confirm input[name=good] { width: 5em; padding: 5px 8px; }
  .notice.confirm .danger { background: transparent; color: var(--bad); border: 1px solid color-mix(in srgb, var(--bad) 50%, transparent); }
  /* One banner per kind of trouble; none when all is well. */
  .banners { display: grid; gap: 8px; margin-bottom: 14px; }
  .banners:empty { display: none; }
  .banner { padding: 10px 14px; border-radius: 10px; border: 1px solid; font-size: 14px; line-height: 1.6; }
  .banner b { margin-right: 6px; }
  .banner a { font-weight: 600; }
  .banner small { color: var(--muted); }
  .banner.needs { border-color: color-mix(in srgb, var(--warn) 50%, transparent); background: color-mix(in srgb, var(--warn) 10%, transparent); }
  .banner.error { border-color: color-mix(in srgb, var(--bad) 50%, transparent); background: color-mix(in srgb, var(--bad) 10%, transparent); }
  .banner.offline { border-color: var(--line); color: var(--muted); }
  /* Labels on machines. */
  .tags { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 4px; }
  .tag { display: inline-block; padding: 1px 7px; border-radius: 5px; background: color-mix(in srgb, var(--accent) 14%, transparent); color: var(--accent); font-size: 11px; font-weight: 700; letter-spacing: .3px; }
  .tag.induction { background: color-mix(in srgb, var(--warn) 14%, transparent); color: var(--warn); }
  /* The table view. */
  .table-wrap { overflow-x: auto; margin-bottom: 10px; border: 1px solid var(--line); border-radius: 12px; background: var(--card); }
  .machine-table { width: 100%; border-collapse: collapse; font-size: 14px; }
  .machine-table th { text-align: left; color: var(--muted); font-size: 12px; font-weight: 600; text-transform: uppercase; letter-spacing: .5px; padding: 8px 12px; border-bottom: 1px solid var(--line); white-space: nowrap; }
  .machine-table td { padding: 9px 12px; border-bottom: 1px solid var(--line); vertical-align: middle; }
  .machine-table tr:last-child td { border-bottom: 0; }
  .machine-table tbody tr { cursor: pointer; }
  .machine-table tbody tr:hover { background: color-mix(in srgb, var(--line) 50%, transparent); }
  .machine-table a { text-decoration: none; }
  .machine-table .state { font-size: 11px; padding: 2px 8px; }
  .machine-table small { color: var(--muted); display: block; max-width: 260px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .t-progress { width: 120px; height: 5px; border-radius: 3px; background: color-mix(in srgb, var(--busy) 28%, transparent); overflow: hidden; margin-bottom: 3px; }
  .t-progress i { display: block; height: 100%; background: var(--busy); }
  .muted { color: var(--muted); }
  .m-swatch { display: inline-block; width: 10px; height: 10px; border-radius: 50%; border: 1px solid var(--line); flex: none; }
  @media (max-width: 900px) { .machine { grid-template-columns: 56px minmax(0, 1fr) 104px; } .m-conn { display: none; } .m-now { grid-column: 2; } }
  .state { justify-self: end; font-weight: 700; letter-spacing: 1px; text-transform: uppercase; font-size: 12px; padding: 4px 10px; border-radius: 6px; background: transparent; color: var(--muted); box-shadow: inset 0 0 0 1.5px var(--line); }
  .state.busy { background: var(--busy); color: #fff; box-shadow: none; }
  /* Busy with a known progress: dark blue as far as it got, light blue for what is left. */
  .state.busy.pct { background: linear-gradient(to right, var(--busy) var(--pct), color-mix(in srgb, var(--busy) 38%, var(--card)) var(--pct)); letter-spacing: .3px; text-transform: none; font-variant-numeric: tabular-nums; white-space: nowrap; }
  .state.idle { color: var(--accent); box-shadow: inset 0 0 0 1.5px color-mix(in srgb, var(--accent) 60%, transparent); }
  .state.paused { background: var(--warn); color: #fff; box-shadow: none; }
  .state.offline, .state.disconnected { color: var(--off); box-shadow: inset 0 0 0 1.5px color-mix(in srgb, var(--off) 45%, transparent); }
  .state.error { background: var(--bad); color: #fff; box-shadow: none; }
  .state.attention { background: var(--warn); color: #fff; box-shadow: none; }
  .state.cooling { color: var(--busy); box-shadow: inset 0 0 0 1.5px var(--busy); }
  .state.out { background: transparent; color: var(--muted); box-shadow: inset 0 0 0 1.5px var(--off); }
  .attention-count b { color: var(--warn) !important; }
  .thumb { width: 56px; height: 56px; border-radius: 10px; background: var(--line); }
  .empty { color: var(--muted); }
  .card-progress { grid-column: 2 / -1; height: 6px; margin-top: 8px; border-radius: 3px; background: color-mix(in srgb, var(--busy) 28%, transparent); overflow: hidden; }
  .card-progress i { display: block; height: 100%; background: var(--busy); transition: width 0.5s; }
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
  <div class="list-tools" data-tools hidden>
    <div class="chips" data-chips role="toolbar" aria-label="Show"></div>
    <div class="list-controls">
      <input type="search" data-search placeholder="Name, model, label, file…" aria-label="Find a machine">
      <select data-group aria-label="Group by"><option value="kind">By kind</option><option value="model">By model</option><option value="label">By label</option><option value="none">Not grouped</option></select>
      <div class="view-switch" role="group" aria-label="View"><button type="button" data-view="cards" title="Cards" aria-label="Cards"><svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="4" width="14" height="5" rx="1.5"/><rect x="3" y="11" width="14" height="5" rx="1.5"/></svg></button><button type="button" data-view="table" title="Table" aria-label="Table"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3 5h14M3 10h14M3 15h14"/></svg></button></div>
    </div>
  </div>
  <div class="banners" data-banners></div>
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
  // What someone chose here (filter, search, grouping, view), kept in this browser only.
  const ui = (() => { try { return { filter: "all", q: "", group: "kind", view: "cards", ...JSON.parse(localStorage.getItem("machine-list") || "{}") }; } catch { return { filter: "all", q: "", group: "kind", view: "cards" }; } })();
  const keep = () => { try { localStorage.setItem("machine-list", JSON.stringify({ filter: ui.filter, group: ui.group, view: ui.view })); } catch { /* not kept */ } };
  // Each machine in one bucket, for the chips: what someone would look for.
  const BUCKETS = [["all", "All"], ["busy", "Busy"], ["free", "Free"], ["needs", "Needs someone"], ["error", "Error"], ["offline", "Offline"], ["out", "Out of use"]];
  function bucket(m) {
    const cls = shownState(m).cls;
    return { busy: "busy", cooling: "busy", idle: "free", paused: "needs", attention: "needs", error: "error", offline: "offline", disconnected: "offline", out: "out" }[cls] || "free";
  }
  const modelOf = (m) => {
    const p = profiles[m.profile];
    return p ? (p.model.includes(p.brand) ? p.model : `${p.brand} ${p.model}`) : KIND[m.kind] || m.kind;
  };
  let all = [];
  function render(machines) {
      all = machines;
      const list = document.getElementById("machines");
      document.querySelector("[data-tools]").hidden = !machines.length;
      if (!machines.length) { document.querySelector("[data-banners]").innerHTML = ""; list.innerHTML = '<p class="empty">No machines yet. <a href="#add" data-add-open>Add one</a>: with the code its agent shows, or a virtual one to try everything.</p>'; return; }
      // The chips, with their counts; one with nothing in it does not show (All always does).
      const counts = Object.fromEntries(BUCKETS.map(([k]) => [k, 0]));
      for (const m of machines) { counts.all++; counts[bucket(m)]++; }
      if (ui.filter !== "all" && !counts[ui.filter]) ui.filter = "all";
      document.querySelector("[data-chips]").innerHTML = BUCKETS.filter(([k]) => k === "all" || counts[k])
        .map(([k, name]) => `<button type="button" class="chip ${k}" data-filter="${k}" aria-pressed="${ui.filter === k}">${name} <b>${counts[k]}</b></button>`).join("");
      // One banner per kind of trouble, its machines named; nothing when all is well.
      const named = (ms) => ms.map((m) => `<a href="machines/${encodeURIComponent(m.id)}">${esc(m.name)}</a>${shownState(m).text && bucket(m) === "needs" ? ` <small>(${esc(shownState(m).text.toLowerCase())})</small>` : ""}`).join(", ");
      const by = (b) => machines.filter((m) => bucket(m) === b);
      const banners = [];
      const needs = by("needs"), errors = by("error"), gone = by("offline");
      if (needs.length) banners.push(`<div class="banner needs"><b>${needs.length === 1 ? "1 machine needs" : `${needs.length} machines need`} someone</b> ${named(needs)}</div>`);
      if (errors.length) banners.push(`<div class="banner error"><b>${errors.length === 1 ? "1 machine reports" : `${errors.length} machines report`} an error</b> ${errors.map((m) => `<a href="machines/${encodeURIComponent(m.id)}">${esc(m.name)}</a>${m.telemetry?.problem ? ` <small>(${esc(m.telemetry.problem)})</small>` : ""}`).join(", ")}</div>`);
      if (gone.length) banners.push(`<div class="banner offline"><b>${gone.length} offline</b> ${gone.map((m) => `<a href="machines/${encodeURIComponent(m.id)}">${esc(m.name)}</a> <small>(${m.seen_seconds_ago == null ? "never reported" : `seen ${seen(m.seen_seconds_ago)}`})</small>`).join(", ")}</div>`);
      document.querySelector("[data-banners]").innerHTML = banners.join("");
      // What is shown: the chip, then the search (name, model, labels, the file it runs).
      const q = ui.q.trim().toLowerCase();
      const shown = machines.filter((m) => (ui.filter === "all" || bucket(m) === ui.filter)
        && (!q || [m.name, modelOf(m), ...(m.labels || []), m.telemetry?.job?.file || ""].some((t) => t.toLowerCase().includes(q))));
      if (!shown.length) { list.innerHTML = '<p class="empty">No machine like that.</p>'; return; }
      const chips = (m) => [...(m.labels || []).map((l) => `<span class="tag">${esc(l)}</span>`), m.queue?.length ? `<span class="tag" title="Waiting to print here, one after another">${m.queue.length} in queue</span>` : "", m.induction_required ? (m.training ? '<span class="tag induction" title="Only who had the training may use it">training</span>' : '<span class="tag induction" title="Only who has permission may use it">permission</span>') : ""].join("");
      const card = (m) => {
        const t = m.telemetry || {};
        const shownS = shownState(m);
        const percent = t.job ? Math.round((t.job.done * 100) / Math.max(1, t.job.total)) : null;
        const bar = t.job ? `<div class="card-progress"><i style="width: ${percent}%"></i></div>` : "";
        const [doing, what] = now(m, t);
        const profile = profiles[m.profile];
        const thumb = profile && profile.image ? `<img class="thumb" src="profiles/${esc(m.profile)}.svg" alt="">`
          : ["fdm", "laser", "cnc"].includes(m.kind) ? `<img class="thumb" src="profiles/_generic/${esc(m.kind)}.svg" alt="">` : `<span class="thumb"></span>`;
        // The model under the name, unless the name says it already; how it connects with it.
        const model = modelOf(m);
        const under = [m.name.toLowerCase().includes(model.toLowerCase()) ? null : model, LINK[m.link] || m.link].filter(Boolean).join(" · ");
        const tags = chips(m);
        return `<a class="machine" href="machines/${encodeURIComponent(m.id)}">
          ${thumb}
          <div class="m-col"><b class="name">${esc(m.name)}</b><small>${esc(under)}</small>${m.loaded ? `<small class="m-spool"><i class="m-swatch" style="background:${esc(m.loaded.color || "transparent")}"></i>${esc(loadedText(m.loaded))}</small>` : ""}${tags ? `<span class="tags">${tags}</span>` : ""}</div>
          ${col("m-now", doing, what)}
          ${trouble(m)}
          ${stateBadge(m)}
          ${bar}
        </a>`;
      };
      // The table, for many machines: one line each, the same columns for every kind.
      const row = (m) => {
        const t = m.telemetry || {};
        const st = shownState(m);
        const percent = t.job ? Math.round((t.job.done * 100) / Math.max(1, t.job.total)) : null;
        const go = `machines/${encodeURIComponent(m.id)}`;
        return `<tr data-go="${esc(go)}"><td><a href="${esc(go)}"><b>${esc(m.name)}</b></a></td><td>${stateBadge(m, "span")}</td><td>${esc(modelOf(m))}</td>
          <td>${m.loaded ? `<span class="m-spool"><i class="m-swatch" style="background:${esc(m.loaded.color || "transparent")}"></i>${esc(m.loaded.material || m.loaded.name)}</span>` : '<span class="muted">–</span>'}</td>
          <td>${t.job ? `<div class="t-progress"><i style="width:${percent}%"></i></div><small>${percent} % · ${esc(t.job.file)}</small>` : '<span class="muted">–</span>'}</td>
          <td>${t.job && t.job.remaining_s != null ? esc(timeLeft(t.job.remaining_s).split(" · ")[1] || "") : '<span class="muted">–</span>'}</td>
          <td>${chips(m) || '<span class="muted">–</span>'}</td></tr>`;
      };
      // Grouped: by kind (as it was), model or label (a machine with two labels is in both).
      const GROUPS = [["fdm", "3D printers"], ["laser", "Lasers"], ["cnc", "CNC machines"], ["cutter", "Cutting plotters"], ["label", "Label printers"], ["paper", "Paper printers"], ["other", "Other machines"]];
      let groups;
      if (ui.group === "model") {
        const models = [...new Set(shown.map(modelOf))].sort();
        groups = models.map((g) => [g, shown.filter((m) => modelOf(m) === g)]);
      } else if (ui.group === "label") {
        const labels = [...new Set(shown.flatMap((m) => m.labels || []))].sort((a, b) => a.localeCompare(b));
        groups = [...labels.map((l) => [l, shown.filter((m) => (m.labels || []).includes(l))]), ["No label", shown.filter((m) => !(m.labels || []).length)]];
      } else if (ui.group === "none") {
        groups = [["", shown]];
      } else {
        groups = GROUPS.map(([kind, title]) => [title, shown.filter((m) => (GROUPS.some(([k]) => k === m.kind) ? m.kind : "other") === kind)]);
      }
      groups = groups.filter(([, ms]) => ms.length);
      for (const b of document.querySelectorAll("[data-view]")) b.setAttribute("aria-pressed", String(b.dataset.view === ui.view));
      list.innerHTML = groups.map(([title, ms]) => {
        const head = title ? `<h2 class="group">${esc(title)} <span>${ms.length}</span></h2>` : "";
        return ui.view === "table"
          ? `${head}<div class="table-wrap"><table class="machine-table"><thead><tr><th>Machine</th><th>State</th><th>Model</th><th>Material</th><th>Progress</th><th>Done at</th><th>Labels</th></tr></thead><tbody>${ms.map(row).join("")}</tbody></table></div>`
          : `${head}${ms.map((m) => card(m) + confirmHtml(m, "machines/")).join("")}`;
      }).join("");
  }
  // The tools: a chip, the search, the grouping, the view.
  document.querySelector("[data-chips]").addEventListener("click", (e) => {
    const chip = e.target.closest("[data-filter]");
    if (chip) { ui.filter = chip.dataset.filter; keep(); render(all); }
  });
  const search = document.querySelector("[data-search]");
  search.value = ui.q;
  search.addEventListener("input", () => { ui.q = search.value; render(all); });
  const group = document.querySelector("[data-group]");
  group.value = ui.group;
  group.addEventListener("change", () => { ui.group = group.value; keep(); render(all); });
  document.querySelector(".view-switch").addEventListener("click", (e) => {
    const b = e.target.closest("[data-view]");
    if (b) { ui.view = b.dataset.view; keep(); render(all); }
  });
  // A row of the table opens its machine, as a card does.
  document.getElementById("machines").addEventListener("click", (e) => {
    const tr = e.target.closest("tr[data-go]");
    if (tr && !e.target.closest("a")) location.href = tr.dataset.go;
  });
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
            crate::auth::tester_may_operate(&store, space, &registered.machine_id);
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
        // A laser: the tester has and wears the glasses (crate::safety).
        let user: i64 = store
            .lock()
            .unwrap()
            .db
            .query_row("SELECT id FROM users WHERE name = 'tester'", [], |r| {
                r.get(0)
            })
            .unwrap();
        store
            .lock()
            .unwrap()
            .confirm_ppe(user, &registered.machine_id);
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
    /// A printer behind an agent with its own SD card (Prusa's MK3S): its card read with an
    /// ordinary command (M20 L T) and its answer kept whole; a file printed from it (M23, M24);
    /// that print, which is no job of the hub's, stopped from the website; and a question on
    /// its screen answered (M876, by the button's index).
    #[tokio::test]
    async fn an_agents_printer_and_its_sd_card() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
        let agent = super::app(store.clone());
        let mk3 = Register {
            protocol: protocol::VERSION,
            name: "Prusa MK3S".into(),
            kind: MachineKind::Fdm,
            agent_version: "0.1.0".into(),
        };
        let (_, body) = call(
            &agent,
            json(protocol::REGISTER, None, serde_json::to_string(&mk3).unwrap()),
        )
        .await;
        let registered: Registered = serde_json::from_str(&body).unwrap();
        let id = registered.machine_id.clone();
        {
            let store = store.lock().unwrap();
            let space = store.default_space().unwrap().0;
            store.pair(space, &registered.pairing_code).unwrap();
            crate::auth::tester_may_operate(&store, space, &id);
        }
        let report = |telemetry: Telemetry| {
            let app = agent.clone();
            let token = registered.token.clone();
            async move {
                let (_, body) = call(
                    &app,
                    json(protocol::TELEMETRY, Some(&token), serde_json::to_string(&telemetry).unwrap()),
                )
                .await;
                serde_json::from_str::<TelemetryReply>(&body).unwrap()
            }
        };
        let idle = || Telemetry {
            state: MachineState::Idle,
            ..Telemetry::default()
        };
        let form = |path: String, body: &str| {
            Request::post(path)
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .body(Body::from(body.to_owned()))
                .unwrap()
        };
        report(idle()).await;

        // Looking at its card asks it (M20 L T), as an ordinary command.
        let look = || Request::get(format!("/api/machines/{id}/sd")).body(Body::empty()).unwrap();
        let (_, body) = call(&app, look()).await;
        let seen: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(seen["asking"], true);
        let reply = report(idle()).await;
        let ask = reply.commands.iter().find(|c| c.gcode == "M20 L T").expect("asked");
        // Its answer, whole, makes the list.
        let listing = "Begin file list\nBENCHY~1.GCO 812345 0x5b4e8c20 \"Benchy.gcode\"\nCUBE.GCO 20\nEnd file list\nok";
        report(Telemetry {
            done: vec![protocol::CommandDone {
                id: ask.id,
                ok: true,
                reason: None,
                output: listing.lines().map(str::to_owned).collect(),
            }],
            ..idle()
        })
        .await;
        let (_, body) = call(&app, look()).await;
        let seen: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(seen["files"][0]["name"], "Benchy.gcode");
        assert_eq!(seen["files"][1]["path"], "/CUBE.GCO");
        assert_eq!(seen["asking"], false, "fresh: not asked again");

        // Printed from its card.
        let (status, body) = call(&app, form(format!("/machines/{id}/sd/print"), "path=/BENCHY~1.GCO")).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        let reply = report(idle()).await;
        assert!(reply.commands.iter().any(|c| c.gcode == "M23 /BENCHY~1.GCO\nM24"));
        assert_eq!(
            call(&app, form(format!("/machines/{id}/sd/print"), "path=../etc")).await.0,
            StatusCode::BAD_REQUEST
        );

        // It prints from its card (its own job, none of the hub's): stopped from here.
        let printing = || Telemetry {
            state: MachineState::Busy,
            job: Some(protocol::JobProgress {
                file: "Benchy.gcode".into(),
                done: 1000,
                total: 812_345,
                elapsed_s: Some(60),
                remaining_s: None,
            }),
            ..Telemetry::default()
        };
        report(printing()).await;
        let (status, body) = call(&app, form(format!("/machines/{id}/job"), "action=stop")).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        assert_eq!(report(printing()).await.job_control, Some(protocol::JobControl::Stop));
        assert_eq!(report(printing()).await.job_control, None, "said once");

        // A question on its screen, answered by the button's index.
        let asking = || Telemetry {
            state: MachineState::Attention,
            prompt: Some(protocol::Prompt {
                text: "Filament runout".into(),
                buttons: vec!["Continue".into(), "Purge more".into()],
            }),
            ..Telemetry::default()
        };
        report(asking()).await;
        let (status, body) = call(&app, form(format!("/machines/{id}/answer"), "button=1")).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        assert_eq!(report(asking()).await.answer, Some(1));
        assert_eq!(report(idle()).await.answer, None, "said once");
        assert_eq!(
            call(&app, form(format!("/machines/{id}/answer"), "button=0")).await.0,
            StatusCode::CONFLICT,
            "it asks nothing now"
        );

        // A job of the hub's, cut off by a power cut (Prusa's MK3; the agent kept it).
        let upload = Request::post("/api/files?name=cube.gcode")
            .body(Body::from("G28\nM104 S215\nG1 X1\nG1 X2\n"))
            .unwrap();
        let (_, body) = call(&app, upload).await;
        let file: serde_json::Value = serde_json::from_str(&body).unwrap();
        let (status, body) = call(
            &app,
            form(format!("/files/{}/print", file["id"].as_str().unwrap()), &format!("machine={id}")),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        let order = report(idle()).await.job.expect("given out");
        let job_state = || store.lock().unwrap().job(order.id).unwrap().unwrap();
        // The agent went quiet (its Pi lost power too), then reports without the job: failed.
        store
            .lock()
            .unwrap()
            .db
            .execute("UPDATE jobs SET started_at = started_at - 100 WHERE id = ?1", [order.id])
            .unwrap();
        report(idle()).await;
        assert_eq!(job_state().state, store::JobState::Failed);
        // Started again, the printer waits to take it up: the agent reports it, paused.
        let waiting = || Telemetry {
            state: MachineState::Paused,
            job_id: Some(order.id),
            ..Telemetry::default()
        };
        report(waiting()).await;
        let back = job_state();
        assert_eq!(back.state, store::JobState::Paused, "back");
        assert_eq!(back.reason.as_deref(), Some(POWER_CUT));
        assert_eq!(back.ended_at, None);
        // Resumed from here: the agent is told, and the reason goes.
        let (status, body) = call(&app, form(format!("/machines/{id}/job"), "action=resume")).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        assert_eq!(report(waiting()).await.job_control, Some(protocol::JobControl::Resume));
        let running = || Telemetry {
            state: MachineState::Busy,
            job_id: Some(order.id),
            ..Telemetry::default()
        };
        assert_eq!(report(running()).await.job_control, None);
        assert_eq!(job_state().reason, None, "an ordinary job again");
        // Another power cut, taken up on the printer's own screen: printing, not paused.
        store
            .lock()
            .unwrap()
            .set_job(order.id, store::JobState::Failed, Some("lost"))
            .unwrap();
        report(waiting()).await;
        assert_eq!(job_state().state, store::JobState::Paused);
        assert_eq!(report(running()).await.job_control, None, "not paused from here");
        assert_eq!(job_state().state, store::JobState::Printing);
        assert_eq!(job_state().reason, None);
    }

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
        let page = label_page::render("My space", &machine, &[], false);
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
