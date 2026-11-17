//! The agent's hub link (`docs/agent-engine.md`, step 2). It registers once (the credentials are
//! kept in a small state file, so a restart keeps the machine), shows the pairing code until
//! someone adds the machine on the website, then reports every two seconds what the engine
//! knows of its machine, and gives the engine what the hub asks: commands, a job (its file
//! fetched first), pause, resume or stop, a question's answer. It never talks to a device.
//!
//! Ours. Moved unchanged from the old `hub.rs` (owner's approval O4–O7 and the step 2 answers):
//! the options, registering and the credentials, the link's "lost / still not reached / back"
//! (retry every report, said again every 60 s), the agent's info and updates (trial 180 s), the
//! download (at most 512 MB, progress every 0.5 s), the stop on SIGTERM. New: the loop with the
//! engine.

use crate::config::{self, Config};
use maker_connect_engine::{Engine, MachineId, Order, Report};
use maker_connect_protocol::{
    self as protocol, CommandDone, JobEnd, MachineKind, MachineState, Register, Registered, Telemetry,
    TelemetryReply,
};
use maker_connect_update as update;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// How often the hub is told (O4).
const REPORT_EVERY: Duration = Duration::from_secs(2);

/// The agent's options. `port`, `baud`, `mode` and `kind` are for the ports (step 3).
#[allow(dead_code)]
pub(crate) struct Options {
    hub: String,
    name: String,
    pub(crate) kind: MachineKind,
    pub(crate) port: Option<String>,
    /// None: the port's reference decides (OctoPrint's settings and detection), no baud of ours
    pub(crate) baud: Option<u32>,
    state: String,
    /// Register again for a new pairing code, forgetting the credentials kept (`--new-code`).
    new_code: bool,
    /// What the machine on the port is (`--mode prusa|marlin|grbl|dymo-pnp|labelwriter-450|dymo-scale`).
    pub(crate) mode: Mode,
}

/// What the machine on the port is, as told at startup (`--mode`), never guessed and with no default
/// (B4): Prusa's MK3 family (PrusaLink's ways), any other Marlin printer (OctoPrint's ways), GRBL (a
/// laser), a DYMO LabelManager PnP on USB (labelle's ways), or a LabelWriter 450 (DYMO's CUPS
/// driver's ways), or a DYMO M-series scale (usbscale's ways).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Prusa,
    Marlin,
    Grbl,
    DymoPnp,
    LabelWriter450,
    DymoScale,
}

/// `--mode`, which must be given (B4).
fn parse_mode(given: Option<&str>) -> Result<Mode, String> {
    match given {
        None => Err("mode: say what is on the port with --mode prusa, marlin, grbl, dymo-pnp, labelwriter-450 or dymo-scale: there is no default".to_owned()),
        Some("prusa") => Ok(Mode::Prusa),
        Some("marlin") => Ok(Mode::Marlin),
        Some("grbl") => Ok(Mode::Grbl),
        Some("dymo-pnp") => Ok(Mode::DymoPnp),
        Some("labelwriter-450") => Ok(Mode::LabelWriter450),
        Some("dymo-scale") => Ok(Mode::DymoScale),
        Some(other) => Err(format!("mode: prusa, marlin, grbl, dymo-pnp, labelwriter-450 or dymo-scale, not {other}")),
    }
}

/// `--kind`, which must be given (B4).
fn parse_kind(given: Option<&str>) -> Result<MachineKind, String> {
    let kind = given.ok_or("kind: say what the machine is with --kind fdm, laser, cnc, cutter, label, paper, scale or other: there is no default")?;
    serde_json::from_value(serde_json::Value::String(kind.to_owned()))
        .map_err(|_| format!("kind: fdm, laser, cnc, cutter, label, paper, scale or other, not {kind}"))
}

/// The options: the config file's (see [`crate::config`]), with the command line's on top.
/// `--save` writes them into the config file.
fn options(args: &[String]) -> Result<Options, Box<dyn std::error::Error>> {
    let mut typed = Config::default();
    let mut named = None;
    let mut save = false;
    let mut new_code = false;
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .ok_or_else(|| format!("expected a value after {flag}"))
        };
        match flag.as_str() {
            // `--cloud`: its name before 0.2.0-beta.2.
            "--hub" | "--cloud" => typed.hub = Some(value()?),
            "--name" => typed.name = Some(value()?),
            "--kind" => typed.kind = Some(value()?),
            "--port" => typed.port = Some(value()?),
            "--baud" => typed.baud = Some(value()?.parse()?),
            "--firmware" => typed.firmware = Some(value()?),
            "--mode" => typed.mode = Some(value()?),
            "--state" => typed.state = Some(value()?),
            "--config" => named = Some(value()?),
            "--save" => save = true,
            "--new-code" => new_code = true,
            _ => return Err(format!("unknown option {flag}").into()),
        }
    }
    let loaded = config::load(named.as_deref())?;
    if let Some((path, _)) = &loaded {
        println!("config: {}", path.display());
    }
    let (path, file) = loaded.unwrap_or_else(|| {
        (
            PathBuf::from(named.as_deref().unwrap_or(config::FILE)),
            Config::default(),
        )
    });
    let all = file.and(typed);
    if save {
        config::save(&path, &all)?;
        println!("saved to {}", path.display());
    }
    // `--mode`, never a default (B4); an older config's `firmware = "grbl"` or `"marlin"` still reads.
    let mode = parse_mode(all.mode.as_deref().or(all.firmware.as_deref().filter(|f| *f != "auto")))?;
    let kind = parse_kind(all.kind.as_deref())?;
    // The credentials beside the config file, so a double-click finds them too.
    let state = all.state.map(PathBuf::from).unwrap_or_else(|| {
        path.parent()
            .unwrap_or(Path::new(""))
            .join("maker-connect-agent.json")
    });
    let (Some(hub), Some(name)) = (all.hub, all.name) else {
        return Err(format!(
            "run needs a hub and a name: --hub <address> --name <name>, or in {}",
            config::FILE
        )
        .into());
    };
    Ok(Options {
        hub: hub.trim_end_matches('/').to_owned(),
        name,
        kind,
        port: all.port,
        baud: all.baud,
        state: state.to_string_lossy().into_owned(),
        new_code,
        mode,
    })
}

/// The agent's credentials, kept between runs.
#[derive(serde::Serialize, serde::Deserialize)]
struct Credentials {
    /// The hub they are for; `cloud` in files from before 0.2.0-beta.2 (read as well, so an
    /// agent updated by hand keeps its machine).
    #[serde(alias = "cloud")]
    hub: String,
    registered: Registered,
}

fn register(options: &Options) -> Result<Registered, Box<dyn std::error::Error>> {
    let agent = Register {
        protocol: protocol::VERSION,
        name: options.name.clone(),
        kind: options.kind,
        agent_version: crate::RELEASE.to_owned(),
    };
    let registered: Registered = ureq::post(&format!("{}{}", options.hub, protocol::REGISTER))
        .send_json(&agent)?
        .into_json()?;
    let credentials = Credentials {
        hub: options.hub.clone(),
        registered: registered.clone(),
    };
    std::fs::write(&options.state, serde_json::to_string_pretty(&credentials)?)?;
    Ok(registered)
}

/// The credentials from an earlier run against the same hub, if any.
fn kept(options: &Options) -> Option<Registered> {
    if options.new_code {
        // Asked for a new code: the old credentials go, so they are not used again.
        if std::fs::remove_file(&options.state).is_ok() {
            println!(
                "forgot the old registration ({}): registering again",
                options.state
            );
        }
        return None;
    }
    let text = std::fs::read_to_string(&options.state).ok()?;
    let credentials: Credentials = serde_json::from_str(&text).ok()?;
    (credentials.hub == options.hub).then_some(credentials.registered)
}

fn show_code(registered: &Registered) {
    println!(
        "Add this machine on the website with code  {}  (it waits until then)",
        registered.pairing_code
    );
}

/// Whether the hub answers, said when it changes rather than at every report: once when it
/// is lost, every minute while it stays away, and when it is back.
struct HubLink {
    lost_at: Option<Instant>,
    said_at: Instant,
}

impl HubLink {
    fn new() -> Self {
        Self {
            lost_at: None,
            said_at: Instant::now(),
        }
    }

    fn lost(&mut self, why: &str) {
        match self.lost_at {
            None => {
                println!("hub not reached: {why} (trying again every few seconds)");
                self.lost_at = Some(Instant::now());
                self.said_at = Instant::now();
            }
            Some(since) if self.said_at.elapsed() >= Duration::from_secs(60) => {
                println!(
                    "hub still not reached after {}: {why}",
                    how_long(since.elapsed())
                );
                self.said_at = Instant::now();
            }
            Some(_) => {}
        }
    }

    fn reached(&mut self) {
        if let Some(since) = self.lost_at.take() {
            println!("hub reached again after {}", how_long(since.elapsed()));
        }
    }
}

/// "45 s", "3 min 20 s", "2 h 5 min".
fn how_long(time: Duration) -> String {
    let s = time.as_secs();
    match s {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min {} s", s / 60, s % 60),
        s => format!("{} h {} min", s / 3600, s % 3600 / 60),
    }
}

/// The computer the agent runs on: its name, and its address towards the hub (the one
/// its reports leave from: a UDP socket "connected" to the hub sends nothing).
fn agent_info(hub: &str) -> protocol::AgentInfo {
    let hostname = std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|h| !h.is_empty());
    let host = hub
        .split("://")
        .nth(1)
        .unwrap_or(hub)
        .split('/')
        .next()
        .unwrap_or("");
    let host = if host.contains(':') {
        host.to_owned()
    } else {
        format!("{host}:80")
    };
    let address = std::net::ToSocketAddrs::to_socket_addrs(&host)
        .ok()
        .and_then(|mut a| a.next())
        .and_then(|hub| {
            let local = if hub.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
            let socket = std::net::UdpSocket::bind(local).ok()?;
            socket.connect(hub).ok()?;
            Some(socket.local_addr().ok()?.ip().to_string())
        });
    protocol::AgentInfo {
        version: crate::RELEASE.to_owned(),
        build: Some(env!("MC_COMMIT").to_owned()),
        built: Some(env!("MC_BUILT").to_owned()),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        hostname,
        address,
        can_update: true,
        last_update: std::env::current_exe()
            .ok()
            .and_then(|exe| update::last(&exe))
            .map(outcome),
    }
}

/// How the last update went, as the hub is told.
fn outcome(o: update::Outcome) -> protocol::UpdateOutcome {
    protocol::UpdateOutcome {
        at: o.at,
        from: o.from,
        to: o.to,
        ok: o.ok,
        note: o.note,
    }
}

/// The name of this agent's build in a release: `maker-connect-agent-linux-armv6`.
fn own_file() -> Option<String> {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    update::this_system().map(|system| format!("maker-connect-agent-{system}{exe}"))
}

/// Fetches the build the hub offers, checks it against the release key, tries it
/// (`--version`) and puts it in place, the running one kept for the way back.
fn self_update(
    options: &Options,
    token: &str,
    exe: &std::path::Path,
    order: &protocol::AgentUpdate,
) -> Result<(), String> {
    if own_file().as_deref() != Some(order.file.as_str()) {
        return Err(format!(
            "the hub offers {}, not this system's build",
            order.file
        ));
    }
    if !update::newer(&order.version, crate::RELEASE) {
        return Err(format!(
            "{} is not newer than {}",
            order.version,
            crate::RELEASE
        ));
    }
    if order.tag != format!("agent-{}", order.version) {
        return Err(format!(
            "{} is not the release of {}",
            order.tag, order.version
        ));
    }
    // Before fetching: a list that is not signed is not worth the download.
    update::verify(
        &order.tag,
        &order.sums,
        &order.signature,
        &update::RELEASE_KEY,
    )?;
    let response = ureq::get(&format!("{}{}", options.hub, order.download))
        .set("authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| format!("could not fetch it: {e}"))?;
    let mut build = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(response.into_reader(), 128 << 20),
        &mut build,
    )
    .map_err(|e| format!("could not fetch it: {e}"))?;
    update::check(
        &build,
        &order.file,
        &order.tag,
        &order.sums,
        &order.signature,
        &update::RELEASE_KEY,
    )?;
    update::install(
        exe,
        &build,
        &["--version".into()],
        crate::RELEASE,
        &order.version,
        TRIAL_S,
        serde_json::Value::Null,
    )
}

/// Whether the agent may update now: no job on the machine, and it is not busy or paused.
/// A machine that waits for someone without a job (a label printer without its cassette, a
/// question on a printer's screen) or is not connected may: nothing of it runs.
fn may_update(report: &Telemetry, printing: bool) -> bool {
    !printing
        && report.job_id.is_none()
        && !matches!(report.state, MachineState::Busy | MachineState::Paused)
}

/// How long a new build has to reach the hub before it is rolled back (the old one waits).
const TRIAL_S: i64 = 180;

/// The most a job's file may be (O6).
const FETCH_LIMIT: u64 = 512 * 1024 * 1024;

/// Fetches a job's file from the hub, as bytes, streamed straight into `file` (never a `String`,
/// never all of it in memory; B2). A file that is not whole is removed. At most [`FETCH_LIMIT`]
/// (more fails: a cut file would be a broken job).
fn fetch(hub: &str, token: &str, order: &protocol::JobOrder, file: &Path) -> Result<(), String> {
    fetch_path(hub, token, &order.download, file)
}

/// The same for a hub path (a job's, or a command's file).
fn fetch_path(hub: &str, token: &str, download: &str, file: &Path) -> Result<(), String> {
    let result = (|| {
        let response = ureq::get(&format!("{hub}{download}"))
            .set("authorization", &format!("Bearer {token}"))
            .call()
            .map_err(|e| format!("the file did not come: {e}"))?;
        let mut reader = std::io::Read::take(response.into_reader(), FETCH_LIMIT + 1);
        let out = std::fs::File::create(file).map_err(|e| format!("the file could not be kept: {e}"))?;
        let mut out = std::io::BufWriter::new(out);
        let size = std::io::copy(&mut reader, &mut out).map_err(|e| format!("the file did not come: {e}"))?;
        std::io::Write::flush(&mut out).map_err(|e| format!("the file could not be kept: {e}"))?;
        if size > FETCH_LIMIT {
            return Err(format!("the file is larger than {} MB", FETCH_LIMIT >> 20));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(file);
    }
    result
}

/// A job's file on its way: fetched on a thread of its own, so the reports go on (B2).
struct Fetching {
    order: protocol::JobOrder,
    file: PathBuf,
    done: std::sync::mpsc::Receiver<Result<(), String>>,
}

impl Fetching {
    fn start(hub: &str, token: &str, order: protocol::JobOrder, file: PathBuf) -> Fetching {
        let (hub, token, job, path) = (hub.to_owned(), token.to_owned(), order.clone(), file.clone());
        let (sent, done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sent.send(fetch(&hub, &token, &job, &path));
        });
        Fetching { order, file, done }
    }

    /// How it went, once it is over.
    fn finished(&self) -> Option<Result<(), String>> {
        match self.done.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err("the download ended without an answer".to_owned())),
        }
    }
}

/// The only path a command's file may be fetched from: `/api/agent/v1/files/{id}` (the hub's
/// own agent route, authorised there as B2 for this agent's machine); anything else is refused.
fn frame_download_ok(download: &str) -> bool {
    download
        .strip_prefix("/api/agent/v1/files/")
        .is_some_and(|id| !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}

/// A command's file on its way (a laser's Frame): fetched on a thread of its own, as a job's (B2).
struct CommandFile {
    command: u32,
    file: PathBuf,
    done: std::sync::mpsc::Receiver<Result<(), String>>,
}

/// The commands the hub gave, in order (O2), while a Frame's file comes: the ones behind it wait.
#[derive(Default)]
struct Commands {
    held: std::collections::VecDeque<protocol::MachineCommand>,
    fetching: Option<CommandFile>,
}

impl Commands {
    /// Gives the engine what can go now. A Frame's `download` becomes the local path of the file
    /// fetched from the hub; what comes after it waits for the file.
    fn step(
        &mut self,
        hub: &str,
        token: &str,
        files: &Path,
        machine: Option<MachineId>,
        engine: &Engine,
        own_done: &mut Vec<CommandDone>,
    ) {
        let failed = |id: u32, why: &str| CommandDone { id, ok: false, reason: Some(why.to_owned()), output: Vec::new() };
        loop {
            let (command, file) = if let Some(fetching) = &self.fetching {
                let result = match fetching.done.try_recv() {
                    Ok(result) => result,
                    Err(std::sync::mpsc::TryRecvError::Empty) => return,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the download ended without an answer".to_owned()),
                };
                let CommandFile { command, file, .. } = self.fetching.take().expect("it was there");
                match result {
                    Ok(()) => (Some(command), Some(file)),
                    Err(why) => {
                        own_done.push(failed(command, &why));
                        continue;
                    }
                }
            } else {
                (None, None)
            };
            let order = match (command, file) {
                (Some(id), Some(file)) => protocol::MachineCommand {
                    id,
                    action: protocol::Action::Frame { download: file.to_string_lossy().into_owned() },
                },
                _ => match self.held.pop_front() {
                    Some(command) => command,
                    None => return,
                },
            };
            if let (protocol::Action::Frame { download }, None) = (&order.action, command) {
                if !frame_download_ok(download) {
                    own_done.push(failed(order.id, "not a file of the hub's for this machine"));
                    continue;
                }
                let (hub, token, path) = (hub.to_owned(), token.to_owned(), download.clone());
                let file = files.join(format!("frame-{}", order.id));
                let (sent, done) = std::sync::mpsc::channel();
                let target = file.clone();
                std::thread::spawn(move || {
                    let _ = sent.send(fetch_path(&hub, &token, &path, &target));
                });
                self.fetching = Some(CommandFile { command: order.id, file, done });
                continue;
            }
            let given = machine.map(|id| engine.send(id, Order::Command { id: order.id, action: order.action }));
            if !matches!(given, Some(Ok(()))) {
                own_done.push(failed(order.id, "this machine has no port yet"));
            }
        }
    }
}

static STOPPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// SIGTERM sets [`STOPPING`]; the loop ends.
fn stop_on_sigterm() {
    #[cfg(unix)]
    {
        extern "C" fn on_term(_: libc::c_int) {
            STOPPING.store(true, Ordering::Relaxed);
        }
        // SAFETY: the handler only stores to an atomic, which is signal-safe.
        unsafe {
            libc::signal(libc::SIGTERM, on_term as extern "C" fn(libc::c_int) as libc::sighandler_t);
        }
    }
}


/// What the hub is told: the engine's report of the machine as its port gave it, Disconnected while
/// there is no port or its device is gone, with the commands done and the job ended (and the agent's
/// own, for want of a port), the job it runs (B6: `job_id`, from the engine's `Report.job`) and the
/// agent itself.
fn telemetry_of(
    report: Option<&Report>,
    own_done: &[CommandDone],
    own_end: Option<&JobEnd>,
    agent: &protocol::AgentInfo,
) -> Telemetry {
    let mut telemetry = report.map(|r| r.telemetry.clone()).unwrap_or_default();
    if report.is_none_or(|r| !r.connected) {
        telemetry.state = MachineState::Disconnected;
    }
    telemetry.done = report.map(|r| r.done.clone()).unwrap_or_default();
    telemetry.done.extend(own_done.iter().cloned());
    // One job end a report (the protocol's `job_end`); the next ones in the reports after.
    telemetry.job_end = report.and_then(|r| r.job_ends.first().cloned()).or_else(|| own_end.cloned());
    // B6
    telemetry.job_id = report.and_then(|r| r.job);
    telemetry.agent = Some(agent.clone());
    telemetry
}

/// Runs the agent: registers, then reports and passes on, until stopped.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let options = options(args)?;
    // A scale not found, or several: refused here, with what was seen (Step 3e, E2).
    crate::ports::check(&options)?;
    println!("{}", crate::version());
    // An update under way: this build on trial, or one to roll back.
    let exe = std::env::current_exe().ok();
    // How to start again after an update: as now, without asking for a new code again.
    let again: Vec<std::ffi::OsString> = std::env::args_os()
        .skip(1)
        .filter(|a| a != "--new-code")
        .collect();
    let mut trial = None;
    if let Some(exe) = &exe {
        match update::on_start(exe) {
            update::Start::Plain => {}
            update::Start::OnTrial(t) => {
                println!(
                    "updated from {} to {}: on trial until it reaches the hub",
                    t.from, t.to
                );
                trial = Some(t);
            }
            update::Start::RollBack(t, why) => {
                println!("the update to {} goes back to {}: {why}", t.to, t.from);
                match update::roll_back(exe, &t, &why) {
                    Ok(()) => update::restart(exe, &again),
                    Err(error) => println!("could not go back: {error}"),
                }
            }
        }
    }
    let mut hub = HubLink::new();
    // A first run with the hub away waits for it, rather than giving up.
    let mut registered = match kept(&options) {
        Some(registered) => registered,
        None => loop {
            match register(&options) {
                Ok(registered) => {
                    hub.reached();
                    break registered;
                }
                Err(error) => {
                    hub.lost(&error.to_string());
                    std::thread::sleep(REPORT_EVERY);
                }
            }
        },
    };
    println!(
        "{} reports to {} as machine {}",
        options.name, options.hub, registered.machine_id
    );
    let mut engine = Engine::new();
    let mut machine: Option<MachineId> = crate::ports::open(&options).map(|port| engine.add(port));
    if machine.is_none() {
        println!("no port for this machine yet: it is reported as disconnected");
    }
    let mut agent = agent_info(&options.hub);
    let mut paired = None;
    // What the agent says itself, for want of a port: commands and jobs it cannot pass on.
    let mut own_done: Vec<CommandDone> = Vec::new();
    let mut own_end: Option<JobEnd> = None;
    // The job file on its way (B2).
    let mut fetching: Option<Fetching> = None;
    // The commands, while a Frame's file comes.
    let mut commands = Commands::default();
    let files = Path::new(&options.state).parent().unwrap_or(Path::new("")).to_path_buf();
    stop_on_sigterm();
    loop {
        if STOPPING.load(Ordering::Relaxed) {
            println!("stopping");
            return Ok(());
        }
        // A job's file that has come whole goes to the engine; one that did not, ends the job.
        if let Some(result) = fetching.as_ref().and_then(Fetching::finished) {
            let Fetching { order, file, .. } = fetching.take().expect("it was there");
            let given = result.and_then(|()| match machine {
                Some(id) => engine
                    .send(id, Order::Job { order: order.clone(), file })
                    .map_err(|_| "its port has ended".to_owned()),
                None => Err("this machine has no port yet".to_owned()),
            });
            if let Err(why) = given {
                own_end = Some(JobEnd { id: order.id, ok: false, stopped: false, reason: Some(why), restart_line: None });
            }
        }
        commands.step(&options.hub, &registered.token, &files, machine, &engine, &mut own_done);
        // What the engine knows of the machine; disconnected while it has none, or while its
        // device is gone.
        let report = machine.and_then(|id| engine.report(id).ok());
        let telemetry = telemetry_of(report.as_ref(), &own_done, own_end.as_ref(), &agent);
        let done_sent = report.as_ref().map_or(0, |r| r.done.len());
        let end_sent = report.as_ref().and_then(|r| r.job_ends.first().cloned());
        let sent = ureq::post(&format!("{}{}", options.hub, protocol::TELEMETRY))
            .set("authorization", &format!("Bearer {}", registered.token))
            .send_json(&telemetry);
        match sent {
            Ok(response) => {
                // An answer that does not read (a hub restarting behind a proxy): try again.
                let reply: TelemetryReply = match response.into_json() {
                    Ok(reply) => {
                        hub.reached();
                        // A new build that reached the hub works: it stays.
                        if let (Some(exe), Some(t)) = (&exe, trial.take()) {
                            update::confirm(exe, &t);
                            println!("{} works: the update is done", t.to);
                            agent.last_update = update::last(exe).map(outcome);
                        }
                        reply
                    }
                    Err(error) => {
                        // Most likely a hub newer than this agent, with something to say
                        // that this agent does not know.
                        hub.lost(&format!(
                            "its answer did not read ({error}): is the hub newer than this agent? Get the agent again from the hub's Downloads page"
                        ));
                        std::thread::sleep(REPORT_EVERY);
                        continue;
                    }
                };
                // Reached the hub: forgotten (O3).
                if let Some(id) = machine {
                    let _ = engine.reported(id, done_sent, usize::from(end_sent.is_some()));
                }
                // A Frame's file goes once its command is reported done.
                if let Some(report) = &report {
                    for done in report.done.iter().take(done_sent) {
                        let _ = std::fs::remove_file(files.join(format!("frame-{}", done.id)));
                    }
                }
                // A job's file goes once its end reached the hub.
                if let Some(end) = &end_sent {
                    let _ = std::fs::remove_file(files.join(format!("job-{}", end.id)));
                }
                own_done.clear();
                if end_sent.is_none() {
                    own_end = None;
                }
                // An update, once nothing runs on the machine.
                if let (Some(order), Some(exe)) = (&reply.update, &exe)
                    && trial.is_none()
                    && may_update(&telemetry, report.as_ref().is_some_and(|r| r.job.is_some()))
                {
                    println!("the hub asks to update to {}", order.version);
                    match self_update(&options, &registered.token, exe, order) {
                        Ok(()) => {
                            println!("{} is in place: trying it", order.version);
                            // Its device free for the new build; this one waits meanwhile.
                            if let Some(id) = machine.take() {
                                let _ = engine.remove(id);
                            }
                            match update::supervise(exe, &again) {
                                Ok(update::Supervised::Confirmed) => {
                                    println!("{} works: starting it for good", order.version);
                                }
                                Ok(update::Supervised::Failed(t, why)) => {
                                    println!("{} did not work ({why}): back to {}", t.to, t.from);
                                    if let Err(error) = update::roll_back(exe, &t, &why) {
                                        println!("could not go back: {error}");
                                    }
                                }
                                Err(error) => println!("could not try it: {error}"),
                            }
                            update::restart(exe, &again);
                        }
                        Err(why) => {
                            println!("not updated to {}: {why}", order.version);
                            update::note_failure(exe, crate::RELEASE, &order.version, &why);
                            agent.last_update = update::last(exe).map(outcome);
                        }
                    }
                }
                // What the hub asks, to the engine in the order given (O2).
                commands.held.extend(reply.commands);
                commands.step(&options.hub, &registered.token, &files, machine, &engine, &mut own_done);
                if let Some(order) = reply.job {
                    let end = |why: &str| JobEnd { id: order.id, ok: false, stopped: false, reason: Some(why.to_owned()), restart_line: None };
                    match (&fetching, machine) {
                        (_, None) => own_end = Some(end("this machine has no port yet")),
                        // Said again while its file is coming: it is on its way.
                        (Some(f), _) if f.order.id == order.id => {}
                        (Some(_), _) => own_end = Some(end("another job's file is still coming")),
                        (None, Some(_)) => {
                            let file = files.join(format!("job-{}", order.id));
                            fetching = Some(Fetching::start(&options.hub, &registered.token, order.clone(), file));
                        }
                    }
                }
                if let (Some(control), Some(id)) = (reply.job_control, machine) {
                    let _ = engine.send(id, Order::Control(control));
                }
                if let (Some(button), Some(id)) = (reply.answer, machine) {
                    let _ = engine.send(id, Order::Answer(button));
                }
                if paired != Some(reply.paired) {
                    match &reply.space {
                        Some(space) => println!("added to {space}: reporting"),
                        None => show_code(&registered),
                    }
                    paired = Some(reply.paired);
                }
            }
            // The hub no longer knows this agent (its machine was removed): start over.
            Err(ureq::Error::Status(401, _)) => {
                hub.reached();
                println!("the hub does not know this machine any more: registering again");
                match register(&options) {
                    Ok(again) => {
                        registered = again;
                        paired = None;
                    }
                    Err(error) => hub.lost(&format!("registering again failed: {error}")),
                }
            }
            Err(error) => hub.lost(&error.to_string()),
        }
        std::thread::sleep(REPORT_EVERY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A hub that answers one request with `body`, and remembers the request it got.
    fn hub_serving(body: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let served = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            let head = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (address, served)
    }

    fn order() -> protocol::JobOrder {
        protocol::JobOrder {
            id: 9,
            name: "label.png".into(),
            download: "/api/agent/jobs/9/file".into(),
            size: 0,
            from_line: None,
            tape_mm: Some(12),
            label_length_mm: None,
            page_size: None,
        }
    }

    /// B2: a PNG (not UTF-8) comes as it is, straight into `job-<id>`.
    #[test]
    fn a_png_is_fetched_as_bytes_into_its_file() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend((0..=255u8).cycle().take(300_000));
        let (hub, served) = hub_serving(png.clone());
        let dir = std::env::temp_dir().join(format!("mc-fetch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("job-9");
        fetch(&hub, "secret", &order(), &file).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), png);
        let request = served.join().unwrap();
        assert!(request.starts_with("GET /api/agent/jobs/9/file "), "{request}");
        assert!(request.to_lowercase().contains("authorization: bearer secret"), "{request}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// B2: it runs on a thread of its own, and says when the file is whole.
    #[test]
    fn a_fetch_on_its_thread_ends_with_the_file_whole() {
        let (hub, _served) = hub_serving(vec![1, 2, 3, 255, 254]);
        let dir = std::env::temp_dir().join(format!("mc-fetching-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("job-9");
        let fetching = Fetching::start(&hub, "t", order(), file.clone());
        let result = loop {
            if let Some(result) = fetching.finished() {
                break result;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(result, Ok(()));
        assert_eq!(std::fs::read(&file).unwrap(), [1, 2, 3, 255, 254]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A hub that does not answer: the job fails with the reason, and no file is left.
    #[test]
    fn a_fetch_that_fails_leaves_no_file() {
        let dir = std::env::temp_dir().join(format!("mc-fetch-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("job-9");
        let why = fetch("http://127.0.0.1:1", "t", &order(), &file).unwrap_err();
        assert!(why.starts_with("the file did not come"), "{why}");
        assert!(!file.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A Frame's file is fetched only from the hub's agent route for files.
    #[test]
    fn a_frames_file_comes_only_from_the_agent_files_route() {
        assert!(frame_download_ok("/api/agent/v1/files/ab12-CD_3"));
        for bad in ["", "/api/agent/v1/files/", "/api/agent/v1/files/../x", "/api/agent/v1/files/a/b", "/api/agent/jobs/9/file", "http://elsewhere/api/agent/v1/files/1", "/etc/passwd", "/api/agent/v1/files/1?x=2"] {
            assert!(!frame_download_ok(bad), "{bad}");
        }
    }

    /// The commands behind a Frame wait for its file; the Frame goes with the local path of it.
    #[test]
    fn a_frame_command_gets_its_file_and_holds_the_ones_behind_it() {
        use maker_connect_engine::{Event, Port};
        use std::sync::mpsc::{Receiver, Sender};
        use std::sync::{Arc, Mutex};
        struct Seen(Arc<Mutex<Vec<Order>>>);
        impl Port for Seen {
            fn run(self: Box<Self>, orders: Receiver<Order>, _events: Sender<Event>) {
                for order in orders {
                    if order == Order::Close {
                        return;
                    }
                    self.0.lock().unwrap().push(order);
                }
            }
        }
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new();
        let id = engine.add(Box::new(Seen(seen.clone())));
        let (hub, served) = hub_serving(b"G90\nG1 X5 Y5 F100\n".to_vec());
        let dir = std::env::temp_dir().join(format!("mc-frame-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut commands = Commands::default();
        let mut own_done = Vec::new();
        commands.held.extend([
            protocol::MachineCommand { id: 1, action: protocol::Action::Unlock },
            protocol::MachineCommand { id: 2, action: protocol::Action::Frame { download: "/api/agent/v1/files/f1".into() } },
            protocol::MachineCommand { id: 3, action: protocol::Action::Home { axes: vec![] } },
            protocol::MachineCommand { id: 4, action: protocol::Action::Frame { download: "/etc/passwd".into() } },
        ]);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            commands.step(&hub, "secret", &dir, Some(id), &engine, &mut own_done);
            if seen.lock().unwrap().len() == 3 || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let request = served.join().unwrap();
        assert!(request.starts_with("GET /api/agent/v1/files/f1 "), "{request}");
        assert!(request.to_lowercase().contains("authorization: bearer secret"), "{request}");
        let file = dir.join("frame-2");
        assert_eq!(std::fs::read(&file).unwrap(), b"G90\nG1 X5 Y5 F100\n");
        let orders = seen.lock().unwrap().clone();
        assert_eq!(orders[0], Order::Command { id: 1, action: protocol::Action::Unlock });
        assert_eq!(orders[1], Order::Command { id: 2, action: protocol::Action::Frame { download: file.to_string_lossy().into_owned() } });
        assert_eq!(orders[2], Order::Command { id: 3, action: protocol::Action::Home { axes: vec![] } });
        assert_eq!(own_done.len(), 1, "the foreign path fails: {own_done:?}");
        assert!(own_done[0].id == 4 && !own_done[0].ok);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn args(more: &[&str]) -> Vec<String> {
        let dir = std::env::temp_dir().join(format!("maker-connect-agent-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("empty.toml");
        std::fs::write(&config, "").unwrap();
        ["--config", config.to_str().unwrap(), "--hub", "http://hub:8460", "--name", "Gonzo"]
            .iter()
            .chain(more)
            .map(|s| s.to_string())
            .collect()
    }

    /// B4: `--mode` and `--kind` are required, with no default; `--port` and `--baud` too: none given is
    /// none (the port's reference decides)
    #[test]
    fn b4_mode_and_kind_have_no_default() {
        let error = |more: &[&str]| options(&args(more)).err().map(|e| e.to_string()).expect("an error");
        assert!(error(&["--kind", "fdm"]).contains("--mode"), "{}", error(&["--kind", "fdm"]));
        assert!(error(&["--kind", "fdm"]).contains("no default"));
        assert!(error(&["--mode", "marlin"]).contains("--kind"), "{}", error(&["--mode", "marlin"]));
        assert!(error(&["--mode", "marlin"]).contains("no default"));
        assert!(error(&["--kind", "fdm", "--mode", "octoprint"]).contains("not octoprint"));
        for (given, mode) in [
            ("prusa", Mode::Prusa),
            ("marlin", Mode::Marlin),
            ("grbl", Mode::Grbl),
            ("dymo-pnp", Mode::DymoPnp),
            ("labelwriter-450", Mode::LabelWriter450),
            ("dymo-scale", Mode::DymoScale),
        ] {
            let options = options(&args(&["--kind", "fdm", "--mode", given])).unwrap();
            assert_eq!(options.mode, mode);
            assert_eq!(options.port, None);
            assert_eq!(options.baud, None, "no invented baud rate");
        }
        let options = options(&args(&["--kind", "fdm", "--mode", "marlin", "--port", "/dev/ttyACM0", "--baud", "250000"])).unwrap();
        assert_eq!((options.port.as_deref(), options.baud), (Some("/dev/ttyACM0"), Some(250_000)));
    }

    /// B6: `job_id` is the engine's job; Disconnected while the device is gone
    #[test]
    fn b6_the_job_id_comes_from_the_engine() {
        let agent = protocol::AgentInfo::default();
        let report = Report {
            telemetry: Telemetry { state: MachineState::Busy, ..Telemetry::default() },
            job: Some(42),
            connected: true,
            ..Report::default()
        };
        let telemetry = telemetry_of(Some(&report), &[], None, &agent);
        assert_eq!(telemetry.job_id, Some(42));
        assert_eq!(telemetry.state, MachineState::Busy);
        let idle = Report { job: None, connected: true, ..report.clone() };
        assert_eq!(telemetry_of(Some(&idle), &[], None, &agent).job_id, None);
        let gone = Report { connected: false, ..report };
        let telemetry = telemetry_of(Some(&gone), &[], None, &agent);
        assert_eq!(telemetry.state, MachineState::Disconnected);
        assert_eq!(telemetry_of(None, &[], None, &agent).state, MachineState::Disconnected);
    }
}
