//! A GRBL machine (a laser) on the agent's serial port, through LaserFoundry's
//! `laserfoundry-device` and `laserfoundry-gcode` (v0.5.0, 61febdb; MIT OR Apache-2.0). Its
//! `Link` keeps the connection on a thread of its own, asks the status every 200 ms, streams a
//! job by GRBL's character counting, and stops by a soft reset, which switches the laser off.
//!
//! **Only LaserFoundry's behaviour goes on the wire.** What this crate adds is the part of
//! LaserFoundry's app (`crates/ui-egui/src/lib.rs`, v0.5.0) a host without its window needs: the
//! link logic of `poll_link` (:1379-1410) and `reconnect_tick` (:1347-1377), and the refusals of
//! `start_job_with` (:1221-1251). Each is marked with its file and line in LaserFoundry; what is
//! ours is listed in `docs/plans/agent-engine.md`, "Step 3d: the laser" (G1 and on).
//!
//! Nothing here waits for the machine: a command is handed to the `Link` and answered later,
//! from the snapshot's console, as [`GrblMachine::report`] keeps reading it.

/// LaserFoundry's in-memory GRBL: for tests, and behind the hub's virtual lasers.
pub use laserfoundry_device::SimulatedGrbl;
use laserfoundry_device::{JobState, Link, MachineState as Grbl, Snapshot, Transport, restart_line};
use maker_connect_protocol::{GrblSettings, JobControl, JobEnd, JobProgress, MachineState, Telemetry};
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// `ui-egui/src/lib.rs:82`: attempts made to get a lost serial connection back before giving up.
pub const RECONNECT_ATTEMPTS: u32 = 8;
/// `ui-egui/src/lib.rs:84`: the longest wait between attempts.
pub const RECONNECT_MAX_WAIT: Duration = Duration::from_secs(30);
/// `ui-egui/src/lib.rs:457`: `reconnect_wait`, the first wait; each attempt doubles it.
pub const RECONNECT_WAIT: Duration = Duration::from_secs(2);
/// `ui-egui/src/lib.rs:413`: `jog_feed`, the feed of a jog the page chose none for, mm/min.
pub const JOG_FEED: f64 = 3000.0;
/// `crates/engine/src/cmd/job.rs:241`: `frame_speed`, mm/min.
pub const FRAME_SPEED: f64 = 3000.0;
/// `ui-egui/src/lib.rs:416`: `frame_power`, percent: the beam is off.
pub const FRAME_POWER: f64 = 0.0;
/// `crates/device/src/serial.rs:9`: `DEFAULT_BAUD`.
#[cfg(feature = "serial")]
pub use laserfoundry_device::serial::DEFAULT_BAUD;

/// Opens the transport again after a lost connection (a serial target only).
pub type Reopen = Box<dyn FnMut() -> Result<Box<dyn Transport + Send>, String> + Send>;

/// What the machine is asked: LaserFoundry's own `Link` calls.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `Link::home`.
    Home,
    /// `Link::unlock`.
    Unlock,
    /// `Link::set_zero`.
    SetZero,
    /// `Link::go_to_zero`.
    GoToZero,
    /// `Link::request_settings`.
    Settings,
    /// `Link::jog(dx, dy, feed)`.
    Jog(f64, f64, f64),
    /// `Link::jog_to(x, y, feed)`: to a place, in machine coordinates (the hub's virtual lasers).
    JogTo(f64, f64, f64),
    /// `Link::send_line`: a line as typed (the app's console, `panels.rs:865`).
    Line(String),
}

/// A command handed to the machine, to be answered in [`Update::done`].
pub type Ticket = u64;

/// How a command ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Done {
    pub ticket: Ticket,
    /// What the machine said, or the reason it failed.
    pub result: Result<Vec<String>, String>,
}

/// What [`GrblMachine::report`] found since the last call.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub telemetry: Telemetry,
    /// A job that ended (a frame is no job of the hub's: it answers its command).
    pub ended: Option<JobEnd>,
    /// Commands answered (or failed), in order.
    pub done: Vec<Done>,
    /// The connection was lost since the last call.
    pub gone: bool,
    /// The connection came back since the last call.
    pub back: bool,
}

/// A command handed to the link whose answers are not all in.
struct Outstanding {
    /// None: the link logic's own (`$$` after the first status).
    ticket: Option<Ticket>,
    /// Lines handed to the link, each answered `ok`, `error:N` or `ALARM:N`.
    sent: usize,
    /// Lines of it the link has echoed (`> line`) into its console.
    echoed: usize,
    answered: usize,
    /// Why a line after the first was refused by the link, once the lines before it are in.
    refused: Option<String>,
    output: Vec<String>,
    /// The machine answered `error:N` or `ALARM:N`.
    failed: Option<String>,
    /// No answer will come (the connection or the controller went).
    dead: bool,
}

impl Outstanding {
    fn new(ticket: Option<Ticket>, sent: usize, refused: Option<String>) -> Self {
        Outstanding {
            ticket,
            sent,
            echoed: 0,
            answered: 0,
            refused,
            output: Vec::new(),
            failed: None,
            dead: false,
        }
    }
}

/// A lost connection being retried (`ui-egui/src/lib.rs:87`).
struct Reconnect {
    attempts: u32,
    next: Instant,
}

pub struct GrblMachine {
    /// `ui-egui` `link`: none while the connection is lost or was never opened.
    link: Option<Link>,
    /// `ui-egui` `snap`: the snapshot of the last poll.
    snap: Option<Snapshot>,
    /// `ui-egui` `target`: a serial port to open again; none for a simulator.
    reopen: Option<Reopen>,
    reconnect: Option<Reconnect>,
    /// `ui-egui` `auto_reconnect` (:455), true.
    auto_reconnect: bool,
    /// `ui-egui` `reconnect_wait` (:457).
    reconnect_wait: Duration,
    /// `ui-egui` `settings_asked` (:262).
    settings_asked: bool,
    /// What is said about the connection, in the app's words (`ui-egui` `status`).
    problem: Option<String>,
    console: Vec<String>,
    outstanding: VecDeque<Outstanding>,
    next_ticket: Ticket,
    job: Option<Running>,
    firmware: Option<String>,
    gone: bool,
    back: bool,
}

/// The job this machine runs.
struct Running {
    /// None: a frame (it has no hub job of its own).
    id: Option<i64>,
    /// The command a frame answers.
    frame: Option<Ticket>,
    name: String,
    /// What was streamed (the file, or the file rebuilt from a line).
    streamed: String,
    /// `ui-egui` `Restart`: the design's program and the line the stream started at.
    restart: Option<(String, usize)>,
    /// The snapshot's job state when it was handed over: until the link's thread takes the job
    /// up, the snapshot still says how the one before ended.
    before: JobState,
    started_seen: bool,
    started: Instant,
    paused_for: Duration,
    paused_at: Option<Instant>,
    stopping: bool,
}

impl GrblMachine {
    /// A machine on a transport that cannot be opened again (a simulator: `Target::Simulator`).
    pub fn new(transport: Box<dyn Transport + Send>) -> Self {
        Self::with(Some(transport), None)
    }

    /// A machine on a serial target: opened already, and opened again as
    /// `ui-egui/src/lib.rs:1347-1377` does after a lost connection.
    pub fn with_reopen(first: Box<dyn Transport + Send>, reopen: Reopen) -> Self {
        Self::with(Some(first), Some(reopen))
    }

    fn with(first: Option<Box<dyn Transport + Send>>, reopen: Option<Reopen>) -> Self {
        GrblMachine {
            // `ui-egui/src/lib.rs:1177`: `Link::connect_with(t, Protocol::Grbl)`, which is `Link::connect`.
            link: first.map(Link::connect),
            snap: None,
            reopen,
            reconnect: None,
            auto_reconnect: true,
            reconnect_wait: RECONNECT_WAIT,
            settings_asked: false,
            problem: None,
            console: Vec::new(),
            outstanding: VecDeque::new(),
            next_ticket: 1,
            job: None,
            firmware: None,
            gone: false,
            back: false,
        }
    }

    /// Opens a GRBL machine on a serial port (LaserFoundry's `SerialTransport`), which is opened
    /// again after a lost connection.
    #[cfg(feature = "serial")]
    pub fn open(port: &str, baud: u32) -> Result<Self, String> {
        let port = port.to_owned();
        let mut reopen: Reopen = Box::new(move || {
            laserfoundry_device::serial::SerialTransport::open(&port, baud)
                .map(|t| Box::new(t) as Box<dyn Transport + Send>)
                .map_err(|e| e.to_string())
        });
        let first = reopen()?;
        Ok(Self::with_reopen(first, reopen))
    }

    /// The hub's job this machine runs (a frame has none).
    pub fn running(&self) -> Option<i64> {
        self.job.as_ref().and_then(|j| j.id)
    }

    /// Whether a job (or a frame) is streaming.
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// `ui-egui` `link_do` (:1331): runs `Link` calls and hands back a refusal as its text. The
    /// answers come later, in [`Update::done`]. Lines the link refuses after others went are
    /// reported once those are answered; a command that sends no line (an empty one) is done
    /// at once.
    pub fn submit(&mut self, commands: &[Command]) -> Result<Ticket, String> {
        let Some(link) = &self.link else {
            // `ui-egui/src/lib.rs:1333` (`link_do`)
            return Err("Not connected".to_owned());
        };
        if self.job.is_some() {
            // `crates/device/src/link.rs:54` and `:277-278`, `LinkError::Busy`, `require_idle`: the link's own words. It reads
            // its snapshot for this, which says so only once its thread took the job up.
            return Err("a job is running; stop it first".to_owned());
        }
        let mut sent = 0;
        let mut refused = None;
        for command in commands {
            let result = match command {
                Command::Home => link.home(),
                Command::Unlock => link.unlock(),
                Command::SetZero => link.set_zero(),
                Command::GoToZero => link.go_to_zero(),
                Command::Settings => link.request_settings(),
                Command::Jog(dx, dy, feed) => link.jog(*dx, *dy, *feed),
                Command::JogTo(x, y, feed) => link.jog_to(*x, *y, *feed),
                Command::Line(line) => link.send_line(line),
            };
            match result {
                // `Link::send_line` sends nothing for an empty line.
                Ok(()) if matches!(command, Command::Line(l) if l.trim().is_empty()) => {}
                Ok(()) => sent += 1,
                Err(error) => {
                    refused = Some(error.to_string());
                    break;
                }
            }
        }
        if sent == 0
            && let Some(error) = refused
        {
            return Err(error);
        }
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.outstanding
            .push_back(Outstanding::new(Some(ticket), sent, refused));
        Ok(ticket)
    }

    /// Starts a job as `ui-egui` `start_job_with` (:1221-1251) does: the refusals, then the
    /// program handed to the link; from a 1-based line of it when given, rebuilt by `resume_from`
    /// (`start_from_line`, :1258-1266) so the head first goes there with the beam off.
    pub fn start(
        &mut self,
        id: i64,
        name: &str,
        program: String,
        from_line: Option<u64>,
    ) -> Result<(), String> {
        let (streamed, restart) = match from_line {
            Some(line) => {
                let rebuilt = laserfoundry_gcode::resume_from(&program, line as usize)
                    .map_err(|e| format!("cannot start at line {line}: {e}"))?;
                (rebuilt, Some((program, line as usize)))
            }
            None => (program, None),
        };
        self.begin(Some(id), None, name, streamed, restart)
    }

    /// `crates/engine/src/cmd/job.rs:245` `frame` and `ui-egui/src/panels.rs:734-740`
    /// `frame_action`: the outline of `bounds` (`x0, y0, x1, y1`, mm) as
    /// `laserfoundry_gcode::frame` writes it, run as a job through the same refusals as Start.
    /// Answered in [`Update::done`] when the frame ends.
    pub fn frame(&mut self, bounds: [f64; 4]) -> Result<Ticket, String> {
        // The machine of the program is the document's in the app; here (ours, G12): the
        // controller's own travel and power range, once read.
        // The bounds are a file's own machine coordinates: no flip of Y (G12).
        let mut machine = laserfoundry_doc::Machine { origin: laserfoundry_doc::Origin::TopLeft, ..Default::default() };
        if let Some(snap) = &self.snap {
            let number = |id: u32| {
                snap.settings
                    .get(&id)
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .filter(|v| v.is_finite())
            };
            if let (Some(x), Some(y)) = (number(130), number(131))
                && x >= 1.0
                && y >= 1.0
            {
                machine.bed_width = x;
                machine.bed_height = y;
            }
            if let Some(s) = number(30)
                && (1.0..=f64::from(u32::MAX)).contains(&s)
            {
                machine.max_s = s.round() as u32;
            }
        }
        let [x0, y0, x1, y1] = bounds;
        let program = laserfoundry_gcode::frame(
            laserfoundry_geom::Rect::new(x0, y0, x1, y1),
            &machine,
            laserfoundry_gcode::Placement::Absolute,
            FRAME_SPEED,
            FRAME_POWER,
        )
        .map_err(|e| e.to_string())?;
        let ticket = self.next_ticket;
        self.begin(None, Some(ticket), "frame", program, None)?;
        self.next_ticket += 1;
        Ok(ticket)
    }

    fn begin(
        &mut self,
        id: Option<i64>,
        frame: Option<Ticket>,
        name: &str,
        streamed: String,
        restart: Option<(String, usize)>,
    ) -> Result<(), String> {
        let Some(link) = &self.link else {
            // `ui-egui/src/lib.rs:1222-1224`
            return Err("Connect to a machine (or the simulator) first".to_owned());
        };
        if self.job.is_some() {
            // `crates/device/src/link.rs:54` and `:277-278`, `LinkError::Busy`, `require_idle`: the link's own words.
            return Err("a job is running; stop it first".to_owned());
        }
        // `ui-egui/src/lib.rs:1232-1236` and `:1520-1532`
        if let Some(why) = self
            .snap
            .as_ref()
            .and_then(|s| s.status.as_ref())
            .and_then(|s| not_ready(s.state))
        {
            return Err(why.to_owned());
        }
        // `ui-egui/src/lib.rs:1239-1246` and `settings_ui.rs:26-31`
        if self.snap.as_ref().is_some_and(|s| laser_mode_off(&s.settings)) {
            return Err("Laser mode ($32) is off on this controller: turn it on in the machine settings before running a job".to_owned());
        }
        let before = link.snapshot().job;
        link.start_job(streamed.clone()).map_err(|e| e.to_string())?;
        self.job = Some(Running {
            id,
            frame,
            name: name.to_owned(),
            streamed,
            restart,
            before,
            started_seen: false,
            started: Instant::now(),
            paused_for: Duration::ZERO,
            paused_at: None,
            stopping: false,
        });
        Ok(())
    }

    /// Pause, resume or stop, as the app's buttons do (`panels.rs:766-776`): `Link::pause`,
    /// `Link::resume`, `Link::stop` (a soft reset, which the app offers with or without a job).
    pub fn control(&mut self, control: JobControl) -> Result<(), String> {
        let Some(link) = &self.link else {
            return Err("Not connected".to_owned());
        };
        let result = match control {
            JobControl::Pause => {
                if let Some(job) = self.job.as_mut()
                    && job.paused_at.is_none()
                {
                    job.paused_at = Some(Instant::now());
                }
                link.pause()
            }
            JobControl::Resume => {
                if let Some(job) = self.job.as_mut()
                    && let Some(at) = job.paused_at.take()
                {
                    job.paused_for += at.elapsed();
                }
                link.resume()
            }
            JobControl::Stop => {
                if let Some(job) = self.job.as_mut() {
                    job.stopping = true;
                }
                // The link clears the answers it still owed: nothing more is coming for them.
                for o in &mut self.outstanding {
                    o.dead = true;
                }
                link.stop()
            }
        };
        result.map_err(|e| e.to_string())
    }

    /// `ui-egui` `reconnect_tick` (:1347-1377). A reconnected machine is idle: a job that was
    /// running is never resumed.
    fn reconnect_tick(&mut self) {
        let Some(state) = &self.reconnect else { return };
        let (attempts, next) = (state.attempts, state.next);
        if Instant::now() < next {
            return;
        }
        let Some(reopen) = self.reopen.as_mut() else {
            self.reconnect = None;
            return;
        };
        match reopen() {
            Ok(t) => {
                self.link = Some(Link::connect(t));
                self.settings_asked = false;
                self.reconnect = None;
                self.back = true;
                self.problem = None;
            }
            Err(e) => {
                let attempts = attempts + 1;
                if attempts >= RECONNECT_ATTEMPTS {
                    self.reconnect = None;
                    self.problem = Some(format!("Could not reconnect: {e}"));
                } else {
                    let wait = wait_after(self.reconnect_wait, attempts);
                    self.reconnect = Some(Reconnect {
                        attempts,
                        next: Instant::now() + wait,
                    });
                }
            }
        }
    }

    /// `ui-egui` `poll_link` (:1379-1410), then what the hub is told: the commands answered, the
    /// job ended, and what the machine is doing.
    pub fn report(&mut self) -> Update {
        self.reconnect_tick();
        let mut done = Vec::new();
        let mut ended = None;
        if let Some(link) = &self.link {
            link.tick();
            let snap = link.snapshot();
            if snap.connected {
                if !self.settings_asked && snap.status.is_some() {
                    // Once the controller answers, learn its settings: a job is refused when
                    // laser mode is off.
                    self.settings_asked = true;
                    if link.request_settings().is_ok() {
                        self.outstanding.push_back(Outstanding::new(None, 1, None));
                    }
                }
                let new = new_lines(&self.console, &snap.console);
                self.console.clone_from(&snap.console);
                done.extend(self.answers(&new));
                if let Some(job) = self.job.as_mut()
                    && !job.started_seen
                {
                    job.started_seen =
                        snap.job != job.before || new.iter().any(|l| l.starts_with("job started: "));
                }
                if matches!(snap.job, JobState::Finished(_))
                    && self.job.as_ref().is_some_and(|j| j.started_seen)
                    && let Some(job) = self.job.take()
                {
                    let failed = matches!(snap.job, JobState::Finished(Err(_)));
                    ended = self.end(&job, &snap, &mut done);
                    // A job that failed reset the controller: the answers it owed are gone.
                    if failed {
                        for o in &mut self.outstanding {
                            o.dead = true;
                        }
                        done.extend(self.answers(&[]));
                    }
                }
                if self.firmware.is_none() {
                    self.firmware = banner(&snap);
                }
                self.snap = Some(snap);
            } else {
                // `ui-egui/src/lib.rs:1387-1400`
                let had_job = matches!(
                    snap.job,
                    JobState::Running | JobState::Paused | JobState::Finished(Err(_))
                );
                // The job ends with the connection (the app only says so in its status bar).
                if let Some(job) = self.job.take() {
                    ended = self.end(&job, &snap, &mut done);
                }
                for o in &mut self.outstanding {
                    o.dead = true;
                }
                done.extend(self.answers(&[]));
                self.link = None;
                self.snap = None;
                self.console.clear();
                self.gone = true;
                if self.auto_reconnect && self.reopen.is_some() {
                    self.reconnect = Some(Reconnect {
                        attempts: 0,
                        next: Instant::now() + self.reconnect_wait,
                    });
                    let note = if had_job {
                        " The job was stopped and will not resume by itself."
                    } else {
                        ""
                    };
                    self.problem = Some(format!(
                        "The machine connection was lost; trying to reconnect.{note}"
                    ));
                } else {
                    self.problem = Some("The machine connection was lost".to_owned());
                }
            }
        }
        Update {
            telemetry: self.telemetry(),
            ended,
            done,
            gone: std::mem::take(&mut self.gone),
            back: std::mem::take(&mut self.back),
        }
    }

    /// Takes the answers in `new` (console lines since the last look) for the commands waiting
    /// for them, oldest first, and returns the commands that are over: the machine answers each
    /// line in the order it was sent.
    fn answers(&mut self, new: &[String]) -> Vec<Done> {
        let mut over = Vec::new();
        for line in new {
            let Some(front) = self.outstanding.front_mut() else { break };
            if front.dead {
                break;
            }
            if let Some(why) = line.strip_prefix("! ignored") {
                // The link logged that it ignored a line: a job had been taken up first.
                front.failed = Some(format!("ignored{why}"));
                over.extend(self.outstanding.pop_front());
                continue;
            }
            if line.starts_with("> ") {
                // The link echoes each line it sends, in the order it was handed over: to the
                // first command that has lines not yet echoed.
                if let Some(owner) = self.outstanding.iter_mut().find(|o| o.echoed < o.sent) {
                    owner.echoed += 1;
                    owner.output.push(tidy(line));
                }
                continue;
            }
            let Some(front) = self.outstanding.front_mut() else { break };
            front.output.push(tidy(line));
            if is_answer(line) {
                front.answered += 1;
                if (line.contains("error:") || line.contains("ALARM:")) && front.failed.is_none() {
                    front.failed = Some(tidy(line));
                }
                if front.answered >= front.sent {
                    over.extend(self.outstanding.pop_front());
                }
            }
        }
        // A command that sent nothing is done; one that can get no answer has failed.
        while self
            .outstanding
            .front()
            .is_some_and(|o| o.dead || o.sent == 0)
        {
            over.extend(self.outstanding.pop_front());
        }
        over.into_iter()
            .filter_map(|o| {
                let ticket = o.ticket?;
                let result = if o.dead {
                    Err("no answer: the connection was lost or the controller was reset".to_owned())
                } else {
                    match (o.failed, o.refused) {
                        (Some(why), _) | (None, Some(why)) => Err(why),
                        (None, None) => Ok(o.output),
                    }
                };
                Some(Done { ticket, result })
            })
            .collect()
    }

    /// The end of a job as the hub is told it; a frame answers its command instead.
    /// `ui-egui` `record_job_position` (:1291-1328) is where the app works out the line to
    /// start again from.
    fn end(&self, job: &Running, snap: &Snapshot, done: &mut Vec<Done>) -> Option<JobEnd> {
        let result = match &snap.job {
            JobState::Finished(result) => result.clone(),
            _ => Err("the machine connection was lost".to_owned()),
        };
        if let Some(ticket) = job.frame {
            done.push(Done {
                ticket,
                result: result.map(|_| Vec::new()),
            });
            return None;
        }
        let ok = result.is_ok() && !job.stopping;
        let acknowledged = snap.progress.map_or(0, |p| p.acknowledged);
        Some(JobEnd {
            id: job.id?,
            ok,
            stopped: job.stopping,
            reason: result.err(),
            restart_line: (!ok && acknowledged > 0).then(|| job.file_line(acknowledged)),
        })
    }

    fn telemetry(&self) -> Telemetry {
        let snapshot = self.snap.as_ref();
        let status = snapshot.and_then(|s| s.status.as_ref());
        let mut state = match status.map(|s| s.state) {
            _ if self.link.is_none() => MachineState::Disconnected,
            Some(Grbl::Run | Grbl::Jog | Grbl::Home | Grbl::Check) => MachineState::Busy,
            Some(Grbl::Hold | Grbl::Door) => MachineState::Paused,
            Some(Grbl::Alarm) => MachineState::Error,
            _ => MachineState::Idle,
        };
        if let Some(snapshot) = snapshot
            && self.link.is_some()
            && self.job.is_some()
        {
            match snapshot.job {
                JobState::Running => state = MachineState::Busy,
                JobState::Paused => state = MachineState::Paused,
                _ => {}
            }
        }
        let job = self.job.as_ref().filter(|j| j.id.is_some()).map(|job| {
            let progress = snapshot.and_then(|s| s.progress);
            let (done, total) =
                progress.map_or((0, 0), |p| (p.acknowledged as u64, p.total as u64));
            let paused = job.paused_for + job.paused_at.map_or(Duration::ZERO, |at| at.elapsed());
            let elapsed = job.started.elapsed().saturating_sub(paused).as_secs();
            let fraction = done as f64 / total.max(1) as f64;
            JobProgress {
                file: job.name.clone(),
                done,
                total,
                elapsed_s: Some(elapsed),
                remaining_s: (fraction > 0.02)
                    .then(|| (elapsed as f64 * (1.0 - fraction) / fraction) as u64),
            }
        });
        Telemetry {
            state,
            grbl: snapshot.and_then(settings),
            firmware: self.firmware.clone().or(Some("GRBL".to_owned())),
            // Only a machine position: a work position shifts with the offset and cannot be
            // mapped onto the bed (`crates/device/src/status.rs`, `position_is_work`).
            position_mm: status
                .filter(|s| !s.position_is_work)
                .and_then(|s| s.position)
                .map(|(x, y, z)| [x as f32, y as f32, z as f32]),
            job_id: self.running(),
            job,
            problem: self.problem.clone(),
            ..Telemetry::default()
        }
    }
}

/// `ui-egui/src/lib.rs:1371`: the wait after `attempts` failed attempts, doubling, at most
/// [`RECONNECT_MAX_WAIT`].
fn wait_after(first: Duration, attempts: u32) -> Duration {
    (first * 2u32.saturating_pow(attempts)).min(RECONNECT_MAX_WAIT)
}

impl Running {
    /// `ui-egui` `record_job_position` (:1291-1328) and `Restart::place_in_design` (:105-115):
    /// the 1-based line to start again from after `acknowledged` lines of the stream were
    /// acknowledged, in the design's program when the stream was rebuilt from a line.
    fn file_line(&self, acknowledged: usize) -> u64 {
        let total = self.streamed.lines().count();
        let line = restart_line(&self.streamed, acknowledged).min(total.max(1));
        match &self.restart {
            Some((program, from)) => {
                let design_total = program.lines().count();
                let from = (*from).clamp(1, design_total.max(1));
                let header = total.saturating_sub(design_total - from + 1);
                let place = if line <= header {
                    from
                } else {
                    from.saturating_add(line - header - 1)
                };
                place.clamp(1, design_total.max(1)) as u64
            }
            None => line as u64,
        }
    }
}

/// `ui-egui/src/lib.rs:1520-1532` `not_ready`: why a job cannot start while the controller
/// reports `state` (`None` when it can).
fn not_ready(state: Grbl) -> Option<&'static str> {
    Some(match state {
        Grbl::Idle | Grbl::Unknown => return None,
        Grbl::Alarm => "The machine is in an alarm state: use Unlock ($X) or Home ($H) first",
        Grbl::Hold => "The machine is holding (paused): resume it or stop it first",
        Grbl::Door => "The machine's safety door is open",
        Grbl::Home => "The machine is homing: wait until it has finished",
        Grbl::Run | Grbl::Jog => "The machine is still moving: wait until it is idle",
        Grbl::Check => "The machine is in check mode ($C): turn it off first",
        Grbl::Sleep => "The machine is asleep: wake it first",
    })
}

/// `ui-egui/src/settings_ui.rs:29`: `$32` is `0` (`Advice::LaserModeOff`).
fn laser_mode_off(settings: &BTreeMap<u32, String>) -> bool {
    settings
        .get(&32)
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .is_some_and(|v| v == 0.0)
}

/// The settings that matter before a job, once the controller has reported them.
fn settings(snapshot: &Snapshot) -> Option<GrblSettings> {
    let number = |id: u32| {
        snapshot
            .settings
            .get(&id)
            .and_then(|v| v.trim().parse::<f64>().ok())
    };
    if snapshot.settings.is_empty() {
        return None;
    }
    let travel = match (number(130), number(131), number(132)) {
        (Some(x), Some(y), Some(z)) => Some([x, y, z]),
        _ => None,
    };
    Some(GrblSettings {
        laser_mode: number(32).map(|v| v != 0.0),
        max_power: number(30),
        travel_mm: travel,
    })
}

/// GRBL's greeting, `Grbl 1.1f ['$' for help]`, as its firmware: `Grbl 1.1f`.
fn banner(snapshot: &Snapshot) -> Option<String> {
    snapshot.console.iter().find_map(|line| {
        let line = line.trim_start_matches("< ");
        line.starts_with("Grbl ")
            .then(|| line.split(" [").next().unwrap_or(line).trim().to_owned())
    })
}

/// The lines of `after` that came since `before`. The console keeps only its last lines, so
/// `after` starts with the end of `before`: the longest such overlap is what was there.
fn new_lines(before: &[String], after: &[String]) -> Vec<String> {
    let kept = (0..=before.len())
        .map(|dropped| &before[dropped..])
        .find(|rest| after.starts_with(rest))
        .map_or(0, <[String]>::len);
    after[kept..].to_vec()
}

fn is_answer(line: &str) -> bool {
    let line = line.trim_start_matches("< ");
    line == "ok"
        || line.starts_with("ok ")
        || line.starts_with("error:")
        || line.starts_with("ALARM:")
}

/// A console line as the terminal shows it: what was sent stays `> line`, what came back
/// loses the link's `< `.
fn tidy(line: &str) -> String {
    line.strip_prefix("< ").unwrap_or(line).to_owned()
}

/// The commands of the hub's virtual lasers' G-code text (`crates/hub/src/devices/virtual_grbl.rs`,
/// their own simulator thread, which still speaks G-code): each line is the `Link` call
/// LaserFoundry has for it, anything else is typed as it is.
pub fn commands_of(gcode: &str) -> Result<Vec<Command>, String> {
    let mut commands = Vec::new();
    for line in gcode.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let upper = line.to_ascii_uppercase();
        commands.push(match upper.as_str() {
            "$H" => Command::Home,
            "$X" => Command::Unlock,
            "$$" => Command::Settings,
            "G92 X0 Y0" => Command::SetZero,
            "G90 G0 X0 Y0" => Command::GoToZero,
            _ if upper.starts_with("$J=") => match jog(&upper) {
                Some(command) => command,
                None => return Err(format!("not a jog this machine takes: {line}")),
            },
            _ => Command::Line(line.to_owned()),
        });
    }
    Ok(commands)
}

/// `$J=G91 G21 X10 F3000` (by) or `$J=G90 G21 X20 Y30 F3000` (to); X and Y only.
fn jog(line: &str) -> Option<Command> {
    let words = line.strip_prefix("$J=")?;
    let mut relative = false;
    let (mut x, mut y, mut feed) = (None, None, None);
    for word in words.split_whitespace() {
        let (letter, value) = word.split_at(1);
        match (letter, value) {
            ("G", "91") => relative = true,
            ("G", "90" | "21" | "53") => {}
            ("X", v) => x = Some(v.parse::<f64>().ok().filter(|v| v.is_finite())?),
            ("Y", v) => y = Some(v.parse::<f64>().ok().filter(|v| v.is_finite())?),
            ("F", v) => feed = Some(v.parse::<f64>().ok().filter(|v| *v > 0.0)?),
            _ => return None,
        }
    }
    let feed = feed?;
    if relative {
        Some(Command::Jog(x.unwrap_or(0.0), y.unwrap_or(0.0), feed))
    } else {
        Some(Command::JogTo(x?, y?, feed))
    }
}

#[cfg(test)]
mod tests;
