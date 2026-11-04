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
//! A proof of concept: no login yet, so run it where only you can reach it.

mod codes;
mod downloads;
mod files;
mod files_page;
mod gcode;
mod label_page;
mod machine_page;
mod profiles;
mod prusa_connect;
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

Open http://localhost:8460 and add machines with the code their agent shows.
";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut listen = "0.0.0.0:8460".to_owned();
    let mut db = "maker-connect.db".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--listen" => listen = args.next().ok_or("expected an address after --listen")?,
            "--db" => db = args.next().ok_or("expected a file after --db")?,
            _ => {
                print!("{USAGE}");
                return Ok(());
            }
        }
    }
    let mut opened = Store::open(&db)?;
    let reread = opened.reread_files()?;
    if reread > 0 {
        println!("read {reread} kept files again");
    }
    let store = Arc::new(Mutex::new(opened));
    tokio::spawn(virtual_machines::run(store.clone()));
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    println!("Maker Connect cloud on http://{listen} (data in {db})");
    axum::serve(
        listener,
        app(store).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}

pub fn app(store: Shared) -> Router {
    Router::new()
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
        .route("/api/machines/{id}/history", get(history))
        .route("/api/profiles", get(profile_list))
        .route("/profiles/{brand}/{file}", get(profile_image))
        .route("/virtual", post(add_virtual))
        .merge(prusa_connect::routes())
        .merge(files::routes())
        .merge(downloads::routes())
        .merge(codes::routes())
        .route("/files", get(files_page::page))
        .fallback(not_found)
        .layer(axum::middleware::from_fn(prusa_connect::log))
        .with_state(store)
}

async fn not_found(uri: axum::http::Uri) -> Response {
    (StatusCode::NOT_FOUND, format!("nothing at {uri}")).into_response()
}

fn failed(error: impl std::fmt::Display) -> Response {
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
            // No commands while a job runs: the machine is busy with it.
            let commands = if report.job_id.is_some() {
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
            let order = protocol::JobOrder {
                id: job.id,
                name: job.file_name.clone(),
                download: format!("/files/{}/download", job.file_id),
                size,
                from_line: job.from_line,
                tape_mm: store.loaded_tape_mm(&machine)?,
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

async fn pair(State(store): State<Shared>, Form(form): Form<PairForm>) -> Response {
    let store = store.lock().unwrap();
    let paired = store
        .default_space()
        .and_then(|(space, _)| store.pair(space, &form.code));
    match paired {
        Ok(Some(_)) => Redirect::to("/").into_response(),
        Ok(None) => Redirect::to("/?unknown-code").into_response(),
        Err(error) => failed(error),
    }
}

async fn machines(State(store): State<Shared>) -> Response {
    let store = store.lock().unwrap();
    match store
        .default_space()
        .and_then(|(space, _)| store.machines(space))
    {
        Ok(machines) => Json(machines).into_response(),
        Err(error) => failed(error),
    }
}

/// A space's machines as JSON.
fn machines_json(store: &Shared) -> String {
    let store = store.lock().unwrap();
    store
        .default_space()
        .and_then(|(space, _)| store.machines(space))
        .map(|machines| serde_json::to_string(&machines).unwrap_or_default())
        .unwrap_or_else(|_| "[]".to_owned())
}

/// The machines now, and again after every change, merged over a quarter second; and every
/// few seconds anyway, as "seen" and "online" change with time alone.
async fn machine_stream(
    State(store): State<Shared>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    use std::time::Duration;
    use tokio::sync::broadcast::error::TryRecvError;
    let changes = store.lock().unwrap().subscribe();
    let stream = futures_util::stream::unfold(
        (store, changes, true),
        |(store, mut changes, first)| async move {
            if !first {
                tokio::select! {
                    _ = changes.recv() => {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        while !matches!(changes.try_recv(), Err(TryRecvError::Empty | TryRecvError::Closed)) {}
                    }
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
            }
            let event = Event::default().data(machines_json(&store));
            Some((Ok(event), (store, changes, false)))
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn page(State(store): State<Shared>) -> Response {
    match store.lock().unwrap().default_space() {
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
    let link = |path: &str, name: &str, place: &str| {
        let current = if place == here {
            r#" aria-current="page""#
        } else {
            ""
        };
        format!(r#"<a href="{home}{path}"{current}>{name}</a>"#)
    };
    format!(
        r##"<header><div class="bar">
  <a class="brand" href="{home}"><img src="{root}logo.svg" alt="" width="28" height="28"><h1>Maker Connect</h1></a>
  <span class="space" title="The space these machines belong to">{space}</span>
  <nav class="nav">{machines}{files}{downloads}</nav>
  <div class="summary">
    <span class="count"><b data-sum="machines">–</b> machines</span>
    <span class="count"><b data-sum="online">–</b> online</span>
    <span class="count"><b data-sum="busy">–</b> busy</span>
    <span class="count attention-count" data-attention-count hidden><b data-sum="attention">–</b> attention</span>
    <span class="live" data-live-dot title="Live updates"><i></i>Live</span>
  </div>
  <button type="button" class="add" data-add-open>Add machine</button>
</div></header>
<dialog id="add-dialog" aria-labelledby="add-title">
  <div class="dialog-head">
    <h2 id="add-title">Add a machine</h2>
    <button type="button" class="close" data-add-close aria-label="Close">✕</button>
  </div>
  <section>
    <h3>With its code</h3>
    <p class="dialog-sub">An agent next to the machine shows a code, and so does a Prusa whose built-in Connect points here. No agent yet? <a href="{home}downloads">Download it</a>.</p>
    <form method="post" action="/pair">
      <input name="code" maxlength="6" autocomplete="off" required aria-label="Pairing code" placeholder="ABC234">
      <button>Add</button>
    </form>
    <p class="note" data-add-note></p>
  </section>
  <section>
    <h3>A virtual one</h3>
    <p class="dialog-sub">Simulated in the cloud: it moves, heats and measures its bed, to try everything without a machine.</p>
    <form method="post" action="/virtual">
      <select name="profile" required aria-label="Machine"><option value="">Choose a machine…</option>{options}</select>
      <button>Add virtual</button>
    </form>
  </section>
</dialog>
<script>
  // The dialog to add a machine: from the button, from any [data-add-open], and at #add.
  {{
    const dialog = document.getElementById("add-dialog");
    const open = () => {{ if (!dialog.open) dialog.showModal(); }};
    document.addEventListener("click", (event) => {{
      if (event.target.closest("[data-add-open]")) {{ event.preventDefault(); open(); }}
      if (event.target.closest("[data-add-close]") || event.target === dialog) dialog.close();
    }});
    if (location.search.includes("unknown-code")) {{
      document.querySelector("[data-add-note]").textContent = "No machine waits with that code.";
      open();
    }}
    if (location.hash === "#add") open();
  }}
  // Called by the page with every list of machines, and when its live stream opens or drops.
  function summarise(machines) {{
    const count = {{
      machines: machines.length,
      online: machines.filter((m) => m.online).length,
      busy: machines.filter((m) => m.online && m.telemetry && m.telemetry.state === "busy").length,
      attention: machines.filter((m) => m.online && m.telemetry && (m.telemetry.state === "attention" || m.telemetry.state === "error")).length,
    }};
    for (const el of document.querySelectorAll("[data-sum]")) el.textContent = count[el.dataset.sum];
    const attention = document.querySelector("[data-attention-count]");
    if (attention) attention.hidden = !count.attention;
  }}
  // How a machine's state is shown, the same on every page. A machine that waits for
  // someone (a label printer without a cassette) shows the reason, amber: not an error.
  function shownState(m) {{
    const t = m.online ? m.telemetry : null;
    if (!t) return {{ text: "Offline", cls: "offline" }};
    if (t.state === "attention" || (t.state === "idle" && t.problem)) return {{ text: t.problem || "Attention", cls: "attention" }};
    const names = {{ disconnected: "Disconnected", idle: "Idle", busy: "Busy", paused: "Paused", error: "Error" }};
    return {{ text: names[t.state] || t.state, cls: t.state }};
  }}
  // Puts a machine's state in every [data-live="state"], and its colour on the badges.
  function showState(m) {{
    const shown = shownState(m);
    for (const el of document.querySelectorAll('[data-live="state"]')) {{
      el.textContent = shown.text;
      if (el.classList.contains("state")) el.className = `state ${{shown.cls}}`;
    }}
  }}
  function live(on) {{
    const dot = document.querySelector("[data-live-dot]");
    dot.classList.toggle("on", on);
    dot.title = on ? "Live updates" : "Reconnecting…";
  }}
</script>"##,
        space = escape(space),
        options = profile_options(None),
        machines = link("", "Machines", "machines"),
        files = link("files", "Files", "files"),
        downloads = link("downloads", "Downloads", "downloads"),
    )
}

/// Every profile as `<option>`s, by brand.
pub(crate) fn profile_options(selected: Option<&str>) -> String {
    let mut options = String::new();
    let mut brand = "";
    for p in profiles::all() {
        if p.brand != brand {
            if !brand.is_empty() {
                options += "</optgroup>";
            }
            brand = &p.brand;
            options += &format!(r#"<optgroup label="{}">"#, escape(brand));
        }
        options += &format!(
            r#"<option value="{}"{}>{}</option>"#,
            escape(&p.id),
            if selected == Some(p.id.as_str()) {
                " selected"
            } else {
                ""
            },
            escape(&p.model)
        );
    }
    options + "</optgroup>"
}

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
    let image = file
        .strip_suffix(".svg")
        .and_then(|model| profiles::find(&format!("{brand}/{model}")))
        .and_then(|p| p.image());
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

async fn add_virtual(State(store): State<Shared>, Form(form): Form<VirtualForm>) -> Response {
    let Some(profile) = profiles::find(&form.profile) else {
        return (StatusCode::BAD_REQUEST, "no such profile").into_response();
    };
    let mut store = store.lock().unwrap();
    match store
        .default_space()
        .and_then(|(space, _)| store.add_virtual(space, profile))
    {
        Ok(id) => Redirect::to(&format!("/machines/{id}")).into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
struct ProfileForm {
    /// A profile id, or empty to recognise the machine from what it says.
    profile: String,
}

/// The same buttons on a GRBL machine: homing is `$H`, and moves are GRBL's own jogs (the
/// laser stays off, and a jog can be cancelled).
fn grbl_action(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "home" => ("Home", "$H"),
        "x+" => ("X +10 mm", "$J=G91 G21 X10 F3000"),
        "x-" => ("X −10 mm", "$J=G91 G21 X-10 F3000"),
        "y+" => ("Y +10 mm", "$J=G91 G21 Y10 F3000"),
        "y-" => ("Y −10 mm", "$J=G91 G21 Y-10 F3000"),
        "settings" => ("Read settings", "$$"),
        _ => return None,
    })
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
        _ => return None,
    })
}

/// The G-code that has a machine measure its bed and print the mesh, by its firmware; or
/// why it cannot be done.
pub(crate) fn probe_gcode(machine: &store::Machine) -> Result<&'static str, &'static str> {
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
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<CommandForm>,
) -> Response {
    let mut store = store.lock().unwrap();
    let space = match store.default_space() {
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
    let found = if grbl()
        && let Some(found) = grbl_action(&form.action)
    {
        Some(found)
    } else if form.action == "probe-bed" {
        match store.machine(space, &id) {
            Ok(Some(machine)) => match probe_gcode(&machine) {
                Ok(gcode) => Some(("Probe the bed", gcode)),
                Err(why) => return (StatusCode::BAD_REQUEST, why).into_response(),
            },
            Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
            Err(error) => return failed(error),
        }
    } else {
        action(&form.action)
    };
    let Some((label, gcode)) = found else {
        return (StatusCode::BAD_REQUEST, "no such command").into_response();
    };
    let queued = store.queue_command(space, &id, label, gcode);
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
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let mut store = store.lock().unwrap();
    match store
        .default_space()
        .and_then(|(space, _)| store.remove(space, &id))
    {
        Ok(true) => Redirect::to("/").into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

async fn history(
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let store = store.lock().unwrap();
    match store
        .default_space()
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
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Form(form): Form<NameForm>,
) -> Response {
    let name = form.name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return (StatusCode::BAD_REQUEST, "a name of 1 to 60 characters").into_response();
    }
    let store = store.lock().unwrap();
    match store
        .default_space()
        .and_then(|(space, _)| store.rename(space, &id, name))
    {
        Ok(true) => Redirect::to(&format!("/machines/{id}#settings")).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

async fn choose_profile(
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Form(form): Form<ProfileForm>,
) -> Response {
    let profile = Some(form.profile.as_str()).filter(|p| !p.is_empty());
    if profile.is_some_and(|p| profiles::find(p).is_none()) {
        return (StatusCode::BAD_REQUEST, "no such profile").into_response();
    }
    let store = store.lock().unwrap();
    match store
        .default_space()
        .and_then(|(space, _)| store.set_profile(space, &id, profile))
    {
        Ok(true) => Redirect::to(&format!("/machines/{id}#settings")).into_response(),
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
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Form(form): Form<MediaForm>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match store.default_space() {
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
        Ok(_) => Redirect::to(&format!("/machines/{id}#label")).into_response(),
        Err(error) => failed(error),
    }
}

/// A machine: its profile to choose (by brand), what the profile knows, and its bed.
async fn machine_page(
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let store = store.lock().unwrap();
    let found = store
        .default_space()
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
pub(crate) const STYLE: &str = r#"  :root { color-scheme: light dark; --bg: #f6f5f2; --card: #fff; --ink: #1d1d1b; --muted: #6b6b66; --line: #e2e0da; --accent: #2f7d6d; --off: #9a9893; --bad: #b3412e; --warn: #b7791f; }
  @media (prefers-color-scheme: dark) { :root { --bg: #111214; --card: #1b1c1f; --ink: #f2f1ee; --muted: #a3a29d; --line: #2e2f33; --accent: #5fbfa9; --off: #77767a; --bad: #e07a63; --warn: #d9a441; } }
  * { box-sizing: border-box; }
  body { margin: 0; background: var(--bg); color: var(--ink); font: 15px/1.45 system-ui, sans-serif; }
  header { position: sticky; top: 0; z-index: 10; background: color-mix(in srgb, var(--bg) 88%, transparent); backdrop-filter: blur(10px); -webkit-backdrop-filter: blur(10px); border-bottom: 1px solid var(--line); }
  header .bar { max-width: var(--page, 1000px); margin: 0 auto; padding: 12px 16px; display: flex; align-items: center; gap: 14px; }
  .brand { display: flex; align-items: center; gap: 10px; text-decoration: none; }
  .brand h1 { margin: 0; font-size: 18px; letter-spacing: -0.2px; }
  .space { padding: 3px 10px; border: 1px solid var(--line); border-radius: 999px; color: var(--muted); font-size: 13px; white-space: nowrap; }
  .nav { display: flex; gap: 4px; margin-left: 8px; }
  .nav a { padding: 6px 12px; border-radius: 8px; color: var(--muted); text-decoration: none; font-weight: 600; font-size: 14px; }
  .nav a:hover { color: var(--ink); }
  .nav a[aria-current=page] { color: var(--ink); background: var(--line); }
  .summary { margin-left: auto; display: flex; align-items: center; gap: 18px; color: var(--muted); font-size: 14px; white-space: nowrap; }
  .summary b { color: var(--ink); font-variant-numeric: tabular-nums; }
  .live { display: inline-flex; align-items: center; gap: 7px; }
  .live i { width: 8px; height: 8px; border-radius: 50%; background: var(--off); }
  .live.on i { background: var(--accent); box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 25%, transparent); }
  button.add { padding: 7px 14px; font-size: 14px; white-space: nowrap; }
  dialog { width: min(520px, calc(100vw - 32px)); padding: 0; border: 1px solid var(--line); border-radius: 16px; background: var(--card); color: var(--ink); box-shadow: 0 20px 60px rgb(0 0 0 / 0.35); }
  dialog::backdrop { background: rgb(0 0 0 / 0.45); backdrop-filter: blur(2px); }
  .dialog-head { display: flex; align-items: center; justify-content: space-between; padding: 18px 22px 0; }
  .dialog-head h2 { margin: 0; font-size: 20px; }
  dialog .close { background: transparent; color: var(--muted); font-size: 18px; padding: 4px 8px; }
  dialog section { padding: 16px 22px 18px; border-top: 1px solid var(--line); margin-top: 16px; }
  dialog section + section { margin-top: 0; }
  dialog h3 { margin: 0; font-size: 16px; }
  .dialog-sub { margin: 4px 0 12px; color: var(--muted); font-size: 14px; }
  dialog form { margin: 0; }
  dialog select { flex: 1; }
  dialog .note:empty { display: none; }
  dialog .note { margin: 10px 0 0; }
  @media (max-width: 720px) { .summary .count { display: none; } .space { display: none; } }
  main { max-width: 1000px; margin: 0 auto; padding: 24px 16px; }
  form { display: flex; gap: 8px; margin-bottom: 20px; flex-wrap: wrap; align-items: center; }
  input, select { font: inherit; padding: 8px 10px; border: 1px solid var(--line); border-radius: 8px; background: var(--card); color: var(--ink); max-width: 100%; }
  input[name=code] { width: 9em; text-transform: uppercase; letter-spacing: 2px; }
  button { font: inherit; font-weight: 600; padding: 8px 14px; border: 0; border-radius: 8px; background: var(--accent); color: #fff; cursor: pointer; }
  .note { color: var(--bad); }
  .machine { display: grid; grid-template-columns: 56px 1fr auto; gap: 4px 16px; padding: 16px 18px; margin-bottom: 12px; background: var(--card); border: 1px solid var(--line); border-radius: 14px; }
  .name { font-weight: 600; font-size: 17px; }
  .state { justify-self: end; font-weight: 700; letter-spacing: 1px; text-transform: uppercase; font-size: 12px; padding: 3px 10px; border-radius: 6px; background: var(--accent); color: #fff; }
  .state.offline, .state.disconnected { background: var(--off); }
  .state.error { background: var(--bad); }
  .state.attention { background: var(--warn); }
  .attention-count b { color: var(--warn) !important; }
  .thumb { grid-row: span 2; width: 56px; height: 56px; border-radius: 10px; background: var(--line); }
  .facts { grid-column: 2 / -1; display: flex; gap: 22px; flex-wrap: wrap; color: var(--muted); font-size: 14px; }
  .facts b { color: var(--ink); font-weight: 500; }
  .empty { color: var(--muted); }
  .card-progress { grid-column: 2 / -1; height: 6px; margin-top: 6px; border-radius: 3px; background: var(--line); overflow: hidden; }
  .card-progress i { display: block; height: 100%; background: var(--accent); transition: width 0.5s; }
  h2.group { margin: 26px 0 12px; font-size: 15px; color: var(--muted); text-transform: uppercase; letter-spacing: 1px; }
  h2.group span { margin-left: 6px; font-weight: 500; }
  a { color: inherit; }
  a.machine { text-decoration: none; }
  a.machine:hover { border-color: var(--accent); }
  .back { color: var(--muted); text-decoration: none; }
  h2 { margin: 12px 0 4px; font-size: 26px; }
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
  function fact(label, value) { return value == null ? "" : `<span>${label} <b>${esc(value)}</b></span>`; }
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
        const job = t.job ? `${t.job.file} ${percent} %` : null;
        const bar = t.job ? `<div class="card-progress"><i style="width: ${percent}%"></i></div>` : "";
        const profile = profiles[m.profile];
        const thumb = profile && profile.image ? `<img class="thumb" src="profiles/${esc(m.profile)}.svg" alt="">` : `<span class="thumb"></span>`;
        return `<a class="machine" href="machines/${encodeURIComponent(m.id)}">
          ${thumb}
          <div class="name">${esc(m.name)}</div>
          <div class="state ${esc(shown.cls)}">${esc(shown.text)}</div>
          <div class="facts">${fact("Type", profile ? profile.model : KIND[m.kind] || m.kind)}${fact("Firmware", t.firmware)}${fact("Nozzle", t.nozzle_c != null ? `${t.nozzle_c.toFixed(0)} °C` : null)}${fact("Bed", t.bed_c != null ? `${t.bed_c.toFixed(0)} °C` : null)}${fact("Job", job)}${fact(m.link === "agent" ? "Agent" : "Link", { virtual: "Virtual, in the cloud", "prusa-connect": "Built-in Prusa Connect" }[m.link] || m.agent_version)}${fact("Seen", seen(m.seen_seconds_ago))}</div>
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
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use maker_connect_protocol::{MachineKind, MachineState};
    use tower::ServiceExt;

    /// A laser on an agent gets a file to run, is told to pause, and says how it ended.
    #[tokio::test]
    async fn an_agent_runs_a_job() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = app(store.clone());
        let laser = Register {
            protocol: protocol::VERSION,
            name: "Ortur".into(),
            kind: MachineKind::Laser,
            agent_version: "0.1.0".into(),
        };
        let (_, body) = call(
            &app,
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
            let app = app.clone();
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
        let (status, gcode) = call(
            &app,
            Request::get(&order.download).body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(gcode.contains("M4 S0"));

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
        let app = app(store.clone());
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
        let app = app(Arc::new(Mutex::new(Store::open(":memory:").unwrap())));
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
        let app = app(store.clone());
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
        let app = app(Arc::new(Mutex::new(Store::open(":memory:").unwrap())));
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
        let app = app(Arc::new(Mutex::new(Store::open(":memory:").unwrap())));
        let report = r#"{"state":"idle"}"#.to_owned();
        let (status, _) = call(&app, json(protocol::TELEMETRY, Some("nope"), report)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let old = r#"{"protocol":0,"name":"x","kind":"fdm","agent_version":"0"}"#.to_owned();
        let (status, body) = call(&app, json(protocol::REGISTER, None, old)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("protocol 1"));
    }
}
