//! What the hub keeps: spaces and their machines, in one SQLite file. Everything belongs
//! to a space from the start, so the same software can serve one makerspace on a Pi or
//! many on a hosted service.

use maker_connect_protocol::{MachineKind, Register, Telemetry};
use rand::Rng;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

mod machines;

mod commands;

mod history;

mod agents;

mod jobs;

mod files;

mod migrate;
use migrate::{back_up_before_upgrade, migrate};

/// Whether machines on this link take commands from the website (yet).
pub fn takes_commands(link: &str) -> bool {
    matches!(link, "prusa-connect" | "virtual" | "agent" | "simplyprint")
}

/// Why a laser job is refused when nobody at the laser started it (our UI text).
pub const LASER_NEVER_ALONE: &str = "a laser starts only when someone at the laser starts it";

/// Who starts a job. A laser job is started only by a person, in their own session.
#[derive(Debug, Clone, Copy)]
pub enum StartedBy<'a> {
    /// Someone pressing Start; `session` is theirs (an API token has none).
    Person { user: &'a str, session: Option<i64> },
    /// The hub itself: the queue, a reprint, the farm, SimplyPrint. `by` is shown as the job's origin.
    Auto { by: Option<&'a str> },
}

impl<'a> StartedBy<'a> {
    /// A person's start, from the access of their request.
    pub fn person(access: &'a crate::access::Access) -> Self {
        StartedBy::Person {
            user: &access.actor,
            session: access.session_id,
        }
    }

    pub(crate) fn is_person_at_a_session(&self) -> bool {
        matches!(self, StartedBy::Person { session: Some(_), .. })
    }

    fn name(&self) -> Option<&'a str> {
        match self {
            StartedBy::Person { user, .. } => Some(user),
            StartedBy::Auto { by } => *by,
        }
    }
}

/// A bed mesh as the hub keeps it: the machine's last, and when it came.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredMesh {
    /// Front row first, each from left to right (see [`maker_connect_protocol::Mesh`]).
    pub rows: Vec<Vec<Option<f32>>>,
    /// Unix seconds.
    pub taken_at: i64,
}

/// A machine counts as online while its agent reported within this many seconds.
pub const ONLINE_SECONDS: i64 = 10;

/// Characters of a pairing code: no 0/O, 1/I, so it reads well from a small screen.
const CODE_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// 16 hex characters, for ids nobody guesses.
pub fn random_id() -> String {
    let mut rng = rand::thread_rng();
    (0..8)
        .map(|_| format!("{:02x}", rng.r#gen::<u8>()))
        .collect()
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The build this hub is, as it writes itself into a database it opened: a release's tag
/// (`scripts/build-release.sh` sets it), or the crate's version.
pub const BUILD: &str = match option_env!("MAKER_CONNECT_RELEASE") {
    Some(tag) => tag,
    None => env!("CARGO_PKG_VERSION"),
};

/// A machine as the website shows it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Machine {
    pub id: String,
    pub name: String,
    pub kind: MachineKind,
    pub agent_version: String,
    /// How it reaches the hub: `agent` (Maker Connect's agent next to it), or
    /// `prusa-connect` (its own firmware, speaking Prusa Connect's printer protocol).
    pub link: String,
    /// Seconds since its agent last reported.
    pub seen_seconds_ago: Option<i64>,
    pub online: bool,
    /// Its last report, while online.
    pub telemetry: Option<Telemetry>,
    /// Its machine profile (`prusa/mk3s`): recognised from what it says, or chosen.
    pub profile: Option<String>,
    /// Whether the owner chose the profile (then recognition leaves it alone).
    pub profile_chosen: bool,
    /// What it last told about itself, kept while it is offline too.
    pub details: Option<Details>,
    /// Its latest commands, newest first.
    pub commands: Vec<Command>,
    /// Its last bed mesh.
    pub mesh: Option<StoredMesh>,
    /// What it is printing (or has queued, or paused).
    pub job: Option<Job>,
    /// HS7b: a laser's loaded job was framed completely after its last Set origin (live on its page).
    pub laser_framed: bool,
    /// HS7b: a Set origin was done on this laser since the hub started.
    pub laser_origin_set: bool,
    /// Its last job, when that was interrupted and can start again where it stopped.
    pub resumable: Option<Job>,
    /// What the owner says is loaded: a label printer's tape (a media id of its profile).
    pub media: Option<String>,
    /// The device whose power it hangs behind (a plug's channel), if any.
    pub powered_by: Option<PowerLink>,
    /// The device it reaches the hub through (an agent, a broker), if any.
    pub reached_through: Option<String>,
    /// The agent's computer, as it last told (kept while it is offline).
    pub agent: Option<maker_connect_protocol::AgentInfo>,
    /// The address its last report came from, as the hub saw it.
    pub peer: Option<String>,
    /// The agent build on the Downloads page, when this machine's agent is another.
    pub agent_update: Option<String>,
    /// The agent release its agent was asked to update to (`agent-v0.3.0`), till it has.
    pub agent_update_to: Option<String>,
    /// Taken out of use by its owner (broken, waiting for a part): no jobs until it is back.
    pub out_of_use: Option<OutOfUse>,
    /// The spool (or cassette) loaded in it, from Materials.
    pub loaded: Option<Loaded>,
    /// A print that finished and is still on the bed, until someone says it was taken.
    pub on_bed: Option<OnBed>,
    /// Ready for the next job: its last print finished and was taken off (PrusaLink's
    /// READY), until a job starts or someone says it is not ready after all.
    pub ready: Option<OnBed>,
    /// Files waiting to print here, next first (crate::queue).
    pub queue: Vec<crate::queue::Queued>,
    /// What the printer asks on its screen now, when it wants someone: its title, and the
    /// whole text (crate::printer_files).
    pub asking: Option<(String, String)>,
    /// The buttons of that question, which can be pressed from the website (POST
    /// `/machines/{id}/answer`): a Prusa on Connect (DIALOG_ACTION) or an agent's printer
    /// (`M876`).
    pub buttons: Vec<String>,
    /// Its last job failed or was stopped, and nobody said yet the bed is clear: how it
    /// ended ("failed", "stopped"). Nothing starts on it until then (crate::queue).
    pub queue_held: Option<String>,
    /// Who started the print from its USB stick on the website, while it may still run.
    pub usb_print_by: Option<String>,
    /// A printer on Klipper: the token in the address its moonraker.conf names. Shown on its
    /// page, never in the live list.
    #[serde(skip)]
    pub moonraker_token: Option<String>,
    /// A laser's optical power, as its owner says: its module (a profile names one).
    pub laser_w: Option<f64>,
    /// Labels its owners gave it (`MK4-BL`, `Room 2`): to find it, filter on, group by.
    pub labels: Vec<String>,
    /// Only who has permission may use it (crate::access).
    pub induction_required: bool,
    /// The permission is a training: someone had it (crate::access).
    pub training: bool,
    /// What someone must have and wear to use it (crate::safety); empty: nothing.
    pub ppe: Vec<String>,
    /// A plate for a part ended here and waits for someone to confirm it (crate::farm).
    pub confirm: Option<crate::farm::Confirm>,
    /// It takes work from the queue (crate::farm).
    pub farm: bool,
    /// Its upkeep: a service every so many printing hours (crate::farm).
    pub upkeep: Option<crate::farm::Upkeep>,
    /// The build plate on it (`Textured PEI`), as someone said (crate::farm).
    pub sheet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutOfUse {
    /// Why, as its owner said ("nozzle clogged, waiting for a part"); may be empty.
    pub reason: String,
    pub since: i64,
}

/// What is loaded in a machine, as its card says it: `S-0007 · Galaxy Black · 412 g left`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Loaded {
    pub item: String,
    /// The material's colour name, else its name (`Galaxy Black`, `Prusament PETG`).
    pub name: String,
    pub color: Option<String>,
    /// What it is: `PETG`, `PLA` (none for a tape).
    pub material: Option<String>,
    /// Where it feeds the machine from: a dry box (and its slot), a shelf.
    pub place: Option<String>,
    pub slot: Option<i64>,
    pub amount: f64,
    /// `g`, `m`, `pcs`.
    pub unit: String,
    /// The material's nozzle temperature, for loading and unloading it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nozzle_c: Option<f64>,
    /// The bed's temperature for it, where the filament says one.
    pub bed_c: Option<f64>,
}

/// A machine's jobs in numbers (ended ones): for its page.
#[derive(Debug, Clone, Default, Serialize)]
pub struct JobStats {
    pub finished: i64,
    pub stopped: i64,
    pub failed: i64,
    /// Seconds from start to end, over ended jobs.
    pub seconds: i64,
    /// Filament the finished jobs used, as their slicers said (grams).
    pub grams: f64,
    /// Parts people confirmed good (the farm's plates).
    pub parts: i64,
}

/// A print on the bed: the job that made it, and when it ended.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OnBed {
    pub job: i64,
    pub file_name: String,
    pub ended_at: Option<i64>,
}

/// A print on the bed can be taken once the bed is at or below this (°C); above, it cools.
pub const TAKE_BELOW_C: f32 = 30.0;

/// The agent build the Downloads page offers.
#[derive(Debug, Clone, PartialEq)]
pub struct OfferedBuild {
    /// The git commit: `d12d6c7`.
    pub commit: String,
    /// When: `2026-11-04 13:20 UTC`.
    pub built: String,
    /// The agent's version: `0.1.0`.
    pub version: Option<String>,
}

/// Where a device gets its power: a channel of another device (a plug, a power strip).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PowerLink {
    pub device: String,
    /// The channel, from 0: a Shelly Plus 2PM has 0 and 1.
    pub channel: u32,
}

/// What a machine tells about itself now and then, beyond its telemetry: shown on its
/// page. All of it optional; a machine tells what it knows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Details {
    pub serial: Option<String>,
    /// A Buddy-firmware Prusa's type, as it tells Prusa Connect.
    pub printer_type: Option<String>,
    pub firmware: Option<String>,
    pub nozzle_mm: Option<f64>,
    pub material: Option<String>,
    #[serde(default)]
    pub storages: Vec<Storage>,
    pub network: Option<Network>,
}

/// Somewhere the machine keeps files: a USB stick, an SD card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Storage {
    /// `USB`, `SDCARD`.
    pub kind: String,
    pub path: String,
    pub free_bytes: Option<u64>,
    pub read_only: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Network {
    pub hostname: Option<String>,
    pub lan_ipv4: Option<String>,
    pub lan_mac: Option<String>,
    pub wifi_ipv4: Option<String>,
    pub wifi_mac: Option<String>,
    pub wifi_ssid: Option<String>,
}

/// A command for a machine, from the website, and how far it got.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Command {
    pub id: u32,
    /// What the owner asked ("Home", "X +10 mm").
    pub label: String,
    pub gcode: String,
    /// A command of Prusa Connect's (`{"command": "START_PRINT", ...}`) rather than G-code:
    /// only for printers on their built-in Connect.
    pub json: bool,
    /// A typed command for a machine behind an agent (an `Action` as JSON in `gcode`): the
    /// hub makes no G-code for those (owner, 2026-11-12).
    #[serde(default)]
    pub action: bool,
    /// Asked by the hub itself (reading the USB stick), not by someone: not listed.
    #[serde(skip)]
    pub background: bool,
    /// What the machine said while carrying it out, where it tells (not over Prusa Connect).
    pub output: Vec<String>,
    pub state: CommandState,
    /// Why the machine refused it.
    pub reason: Option<String>,
    /// When it last changed state (Unix seconds).
    pub at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandState {
    /// Waiting for the machine to ask.
    Queued,
    /// Given to the machine.
    Sent,
    /// The machine took it on.
    Accepted,
    Finished,
    Rejected,
}

/// A print the machine runs itself, as its reports tell it.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnPrint {
    pub file: String,
    pub by: Option<String>,
    pub started_at: i64,
    /// How far it got, in percent, as last reported.
    pub percent: u64,
}

/// One point of a machine's temperature history.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Sample {
    /// Unix seconds.
    pub t: i64,
    pub nozzle: Option<f32>,
    pub nozzle_target: Option<f32>,
    pub bed: Option<f32>,
    pub bed_target: Option<f32>,
    /// The fans' speeds, where told.
    pub print_fan: Option<u32>,
    pub hotend_fan: Option<u32>,
}

/// A machine's temperatures are kept every this many seconds, for this long: in memory,
/// so a restart starts the graphs afresh.
const SAMPLE_SECONDS: i64 = 2;
pub const HISTORY_SECONDS: i64 = 30 * 60;
/// Commands shown per machine.
const COMMANDS_KEPT: usize = 40;
/// Lines of a command's output kept.
const OUTPUT_KEPT: usize = 200;
/// A command given to a machine that does not answer about it is given up after this.
const COMMAND_SECONDS: i64 = 120;
/// K18: why a Sent or Accepted command of a Marlin agent machine fails when its agent was gone.
pub const AGENT_GONE: &str = "the agent was gone";

/// Keeps a machine's commands to [`COMMANDS_KEPT`]: the oldest done ones go first, the
/// hub's own before someone's; one still waiting or on its way is never dropped.
fn trim(commands: &mut Vec<Command>) {
    while commands.len() > COMMANDS_KEPT {
        let done = |c: &Command| !matches!(c.state, CommandState::Queued | CommandState::Sent);
        let Some(i) = commands
            .iter()
            .position(|c| done(c) && c.background)
            .or_else(|| commands.iter().position(done))
        else {
            break;
        };
        commands.remove(i);
    }
}

/// A G-code file of a space.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StoredFile {
    pub id: String,
    pub name: String,
    pub size: u64,
    /// Unix seconds.
    pub uploaded_at: i64,
    pub meta: crate::gcode::FileMeta,
}

/// A file put on a machine once: where, how it went, when (Unix seconds).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Printed {
    pub machine_id: String,
    pub state: JobState,
    pub at: i64,
}

/// A file printed (or cut, or engraved) on a machine.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Job {
    pub id: i64,
    pub machine_id: String,
    pub file_id: String,
    pub file_name: String,
    pub state: JobState,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub reason: Option<String>,
    /// It started at this line of the file (resuming an earlier job).
    pub from_line: Option<u64>,
    /// Where it could start again, when it did not run to the end.
    pub restart_line: Option<u64>,
    /// Who started it: a person's or a token's name, or "queue · project".
    pub by: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    /// HS7b: a laser's job, sent to it and ready, not started: the person at the laser frames,
    /// sets the origin and presses Start burn. Never sent to the machine as a job until then.
    Loaded,
    /// Waiting for the machine to take it.
    Queued,
    Printing,
    Paused,
    /// Asked to stop; the machine has not yet.
    Stopping,
    Finished,
    Stopped,
    Failed,
}

impl JobState {
    pub fn active(self) -> bool {
        matches!(
            self,
            JobState::Loaded | JobState::Queued | JobState::Printing | JobState::Paused | JobState::Stopping
        )
    }

    fn parse(text: &str) -> JobState {
        serde_json::from_value(serde_json::Value::String(text.to_owned()))
            .unwrap_or(JobState::Failed)
    }

    pub(crate) fn name(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    }
}

pub struct Store {
    pub(crate) db: Connection,
    /// Where uploaded files are kept: `<id>.gcode`, and `<id>.png` for a thumbnail.
    pub files_dir: std::path::PathBuf,
    /// What `/downloads` offers (the agent for each platform): `maker-connect-downloads/`
    /// beside the database.
    pub downloads_dir: std::path::PathBuf,
    /// Commands per machine id, oldest first.
    commands: std::collections::HashMap<String, Vec<Command>>,
    /// Who said they have and wear a machine's PPE, until when: (user, machine) → Unix
    /// seconds (crate::safety).
    pub(crate) ppe_ok: std::collections::HashMap<(i64, String), i64>,
    /// What a printer asks on its screen while it wants someone (a Prusa's ATTENTION: "Sliced
    /// for another printer?"), per machine id: (title, text).
    pub(crate) asking: std::collections::HashMap<String, (String, String)>,
    /// A Prusa Connect printer's question: its buttons and the dialog they answer
    /// (`DIALOG_ACTION`), by machine (crate::printer_files).
    pub(crate) dialogs: std::collections::HashMap<String, (Vec<String>, Option<u64>)>,
    /// Printers whose print preview is to be closed (crate::printer_files::on_report), by
    /// machine id.
    pub(crate) close_preview: std::collections::HashSet<String>,
    /// The file a Prusa on Connect prints, by its token: its JOB_INFO says it once, as a
    /// print starts; its telemetry only says how far.
    pub(crate) printing_file: std::collections::HashMap<String, (Option<u64>, String)>,
    /// The print each Prusa on Connect was last asked about (SEND_JOB_INFO), by token: once.
    pub(crate) job_info_asked: std::collections::HashMap<String, u64>,
    /// Prints a machine runs itself (from its USB stick or SD card, or started at its
    /// screen), by machine id, while they run: recorded as a job when they end (see
    /// `follow_own_print`).
    pub(crate) own_prints: std::collections::HashMap<String, OwnPrint>,
    /// Answers to an agent's printer's question, by machine, until its next report takes
    /// them (`M876 S<index>`).
    pub(crate) answers: std::collections::HashMap<String, u8>,
    /// Pause, resume or stop for a print the machine started itself (from its SD card): no
    /// job of the hub's, so said once with its next report.
    pub(crate) own_job_control:
        std::collections::HashMap<String, maker_connect_protocol::JobControl>,
    /// HS7b: per laser, the file id it last framed completely since its last Set origin.
    pub(crate) laser_framed: std::collections::HashMap<String, String>,
    /// HS7b: lasers on which a Set origin was done since the hub started.
    pub(crate) laser_origin_set: std::collections::HashSet<String>,
    next_command: u32,
    /// Temperature history per machine id, oldest first.
    history: std::collections::HashMap<String, std::collections::VecDeque<Sample>>,
    /// A signal whenever a machine changes, for the pages that show them live.
    changes: tokio::sync::broadcast::Sender<()>,
    /// A vendor, filament or spool added, changed or removed: for Spoolman's WebSockets.
    spoolman: tokio::sync::broadcast::Sender<SpoolmanEvent>,
    /// While nobody has an account: the code (printed at start) that makes the first one.
    pub setup_code: Option<String>,
    /// Failed sign-ins per key (`ip:…`, `name:…`), Unix seconds, for slowing guessing down.
    pub(crate) failures: std::collections::HashMap<String, Vec<i64>>,
}

/// A change to Spoolman's data: `spool` 7 `updated`. A removed one carries what it was.
#[derive(Debug, Clone)]
pub struct SpoolmanEvent {
    /// Whose it is: only that space's WebSockets hear of it.
    pub space: Option<i64>,
    pub resource: &'static str,
    pub kind: &'static str,
    pub id: i64,
    pub was: Option<serde_json::Value>,
}

impl Store {
    /// Opens (or creates) the store; a new one gets one space.
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let existed = path != ":memory:" && std::fs::metadata(path).is_ok_and(|m| m.len() > 0);
        let mut db = Connection::open(path)?;
        db.execute_batch("PRAGMA journal_mode = WAL;")?;
        if existed {
            back_up_before_upgrade(&db, path)?;
        }
        let tx = db.transaction()?;
        migrate(&tx)?;
        tx.commit()?;
        // Files beside the database (`maker-connect.db` keeps them in `maker-connect-files/`);
        // an in-memory store (tests) in a folder of its own.
        let files_dir = if path == ":memory:" {
            std::env::temp_dir().join(format!("maker-connect-test-{}", random_id()))
        } else {
            let db = std::path::Path::new(path);
            let stem = db
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("maker-connect");
            db.with_file_name(format!("{stem}-files"))
        };
        std::fs::create_dir_all(&files_dir)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let downloads_dir =
            files_dir.with_file_name(files_dir.file_name().and_then(|n| n.to_str()).map_or(
                "maker-connect-downloads".to_owned(),
                |n| {
                    n.replace("-files", "-downloads")
                        .replace("-test-", "-test-downloads-")
                },
            ));
        std::fs::create_dir_all(&downloads_dir)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let mut store = Self {
            db,
            files_dir,
            downloads_dir,
            commands: Default::default(),
            ppe_ok: Default::default(),
            asking: Default::default(),
            dialogs: Default::default(),
            close_preview: Default::default(),
            printing_file: Default::default(),
            job_info_asked: Default::default(),
            own_prints: Default::default(),
            answers: Default::default(),
            own_job_control: Default::default(),
            laser_framed: Default::default(),
            laser_origin_set: Default::default(),
            // Ids that do not repeat soon after a restart, as a printer may remember the last.
            next_command: (now() % 1_000_000_000) as u32,
            history: Default::default(),
            changes: tokio::sync::broadcast::channel(16).0,
            spoolman: tokio::sync::broadcast::channel(256).0,
            setup_code: None,
            failures: Default::default(),
        };
        if !store.has_users()? {
            store.setup_code = Some(crate::accounts::setup_code());
        }
        Ok(store)
    }

    /// Signals each change to a machine from now on (some may be merged into one).
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.changes.subscribe()
    }

    /// Spoolman's data changed: `spool` 7 `updated`.
    pub(crate) fn spoolman_event(&self, resource: &'static str, kind: &'static str, id: i64) {
        let space = match resource {
            "spool" | "filament" | "vendor" => self
                .db
                .query_row(
                    &format!("SELECT space_id FROM {resource} WHERE id = ?1"),
                    [id],
                    |r| r.get(0),
                )
                .ok(),
            _ => None,
        };
        let _ = self.spoolman.send(SpoolmanEvent {
            space,
            resource,
            kind,
            id,
            was: None,
        });
    }

    /// One of Spoolman's things removed, with what it was.
    pub(crate) fn spoolman_removed(
        &self,
        space: i64,
        resource: &'static str,
        id: i64,
        was: serde_json::Value,
    ) {
        let _ = self.spoolman.send(SpoolmanEvent {
            space: Some(space),
            resource,
            kind: "deleted",
            id,
            was: Some(was),
        });
    }

    pub fn subscribe_spoolman(&self) -> tokio::sync::broadcast::Receiver<SpoolmanEvent> {
        self.spoolman.subscribe()
    }

    pub(crate) fn changed(&self) {
        // Nobody listening is fine.
        let _ = self.changes.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upgrade_backs_up_the_database_first() {
        let dir = std::env::temp_dir().join(format!("maker-connect-upgrade-{}", random_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("maker-connect.db");
        let path = path.to_str().unwrap();
        let backups = || {
            std::fs::read_dir(dir.join("maker-connect-backups"))
                .map(|d| {
                    d.flatten()
                        .map(|e| e.file_name().into_string().unwrap())
                        .collect()
                })
                .unwrap_or_else(|_| Vec::<String>::new())
        };
        // A new database: nothing to back up.
        let id = {
            let mut store = Store::open(path).unwrap();
            device(&mut store, 1, "MINI")
        };
        assert!(backups().is_empty());
        // The same build again: no copy either.
        drop(Store::open(path).unwrap());
        assert!(backups().is_empty());
        // As if an older build had it last.
        Connection::open(path)
            .unwrap()
            .execute(
                "UPDATE meta SET value = '0.0.9' WHERE key = 'opened_by'",
                [],
            )
            .unwrap();
        let store = Store::open(path).unwrap();
        let made = backups();
        assert_eq!(made.len(), 1, "{made:?}");
        assert!(
            made[0].ends_with(&format!("-0.0.9-to-{BUILD}.db")),
            "{made:?}"
        );
        // The copy holds the machine, and so does the database itself.
        let copy = Connection::open(dir.join("maker-connect-backups").join(&made[0])).unwrap();
        let kept: i64 = copy
            .query_row("SELECT COUNT(*) FROM machines WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kept, 1);
        assert!(store.machine(1, &id).unwrap().is_some());
        drop(store);
        drop(Store::open(path).unwrap());
        assert_eq!(backups().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn device(store: &mut Store, space: i64, name: &str) -> String {
        let profile = crate::profiles::find("prusa/mini").unwrap();
        let id = store.add_virtual(space, profile).unwrap();
        store.rename(space, &id, name).unwrap();
        id
    }

    /// A finished print stays on the bed until taken; a machine out of use says why.
    #[test]
    fn a_print_on_the_bed_and_a_machine_out_of_use() {
        let mut store = Store::open(":memory:").unwrap();
        let (space, _) = store.default_space().unwrap();
        let printer = device(&mut store, space, "MINI");
        store
            .add_file(
                space,
                "f1",
                "cube.gcode",
                10,
                &crate::gcode::FileMeta::default(),
            )
            .unwrap();
        let file = store.file(space, "f1").unwrap().unwrap();
        let job = store
            .add_job(space, &printer, &file, None, StartedBy::Auto { by: None })
            .unwrap()
            .unwrap();
        store.set_job(job, JobState::Printing, None).unwrap();
        assert_eq!(
            store.machine(space, &printer).unwrap().unwrap().on_bed,
            None
        );
        store.set_job(job, JobState::Finished, None).unwrap();
        let on_bed = store.machine(space, &printer).unwrap().unwrap().on_bed;
        assert_eq!(on_bed.map(|b| b.file_name), Some("cube.gcode".to_owned()));
        assert!(store.print_taken(space, &printer).unwrap());
        assert_eq!(
            store.machine(space, &printer).unwrap().unwrap().on_bed,
            None
        );
        assert!(!store.print_taken(space, &printer).unwrap());

        assert!(
            store
                .set_out_of_use(space, &printer, Some("nozzle clogged"))
                .unwrap()
        );
        let out = store
            .machine(space, &printer)
            .unwrap()
            .unwrap()
            .out_of_use
            .unwrap();
        assert_eq!(out.reason, "nozzle clogged");
        assert!(out.since > 0);
        assert!(store.set_out_of_use(space, &printer, None).unwrap());
        assert_eq!(
            store.machine(space, &printer).unwrap().unwrap().out_of_use,
            None
        );
    }

    /// A finished print counts its filament off the loaded spool; a stopped one its part.
    #[test]
    fn a_job_counts_its_filament_off_the_loaded_spool() {
        let mut store = Store::open(":memory:").unwrap();
        let (space, _) = store.default_space().unwrap();
        let printer = device(&mut store, space, "MINI");
        let pla = crate::stock::Material {
            id: String::new(),
            kind: "filament".into(),
            brand: "Prusament".into(),
            name: "PLA Galaxy Black".into(),
            color: None,
            color_name: None,
            ink: None,
            unit: "g".into(),
            package: Some(1000.0),
            ean: None,
            minimum: None,
            media: None,
            width_mm: None,
            diameter_mm: Some(1.75),
            density_g_cm3: Some(1.24),
            spool_g: None,
            nozzle_c: None,
            bed_c: None,
            source: None,
        };
        let material = store.add_material(space, pla).unwrap();
        let spool = store
            .receive(space, &material, 1, None)
            .unwrap()
            .unwrap()
            .remove(0);
        store.load_item(space, &spool, &printer).unwrap().unwrap();
        let meta = crate::gcode::FileMeta {
            filament_g: Some(12.5),
            ..crate::gcode::FileMeta::default()
        };
        store
            .add_file(space, "f1", "cube.gcode", 10, &meta)
            .unwrap();
        let file = store.file(space, "f1").unwrap().unwrap();
        let job = store
            .add_job(space, &printer, &file, None, StartedBy::Auto { by: None })
            .unwrap()
            .unwrap();
        store.set_job(job, JobState::Printing, None).unwrap();
        store.set_job(job, JobState::Finished, None).unwrap();
        let left = store.items(space).unwrap()[0].amount;
        assert_eq!(left, 987.5);
        let last = &store.movements(space, &spool).unwrap()[0];
        assert_eq!(
            (last.kind.as_str(), last.note.as_deref()),
            ("used", Some("job cube.gcode"))
        );
    }

    #[test]
    fn devices_hang_behind_a_plug_without_loops() {
        let mut store = Store::open(":memory:").unwrap();
        let (space, _) = store.default_space().unwrap();
        let wall = device(&mut store, space, "Wall plug");
        let strip = device(&mut store, space, "Power strip");
        let dryer = device(&mut store, space, "Dryer");
        let lamp = device(&mut store, space, "Lamp");
        let link = |device: &str, channel| PowerLink {
            device: device.to_owned(),
            channel,
        };
        // Two devices on one channel of the strip, the strip on the wall plug.
        assert_eq!(
            store
                .set_powered_by(space, &dryer, Some(&link(&strip, 1)))
                .unwrap(),
            Ok(())
        );
        assert_eq!(
            store
                .set_powered_by(space, &lamp, Some(&link(&strip, 1)))
                .unwrap(),
            Ok(())
        );
        assert_eq!(
            store
                .set_powered_by(space, &strip, Some(&link(&wall, 0)))
                .unwrap(),
            Ok(())
        );
        let dryer_now = store.machine(space, &dryer).unwrap().unwrap();
        assert_eq!(dryer_now.powered_by, Some(link(&strip, 1)));
        // The wall plug behind the dryer would be a loop; so would a device behind itself.
        assert!(
            store
                .set_powered_by(space, &wall, Some(&link(&dryer, 0)))
                .unwrap()
                .is_err()
        );
        assert!(
            store
                .set_powered_by(space, &lamp, Some(&link(&lamp, 0)))
                .unwrap()
                .is_err()
        );
        assert!(
            store
                .set_powered_by(space, &lamp, Some(&link("nothing", 0)))
                .unwrap()
                .is_err()
        );
        // Off the plug again.
        assert_eq!(store.set_powered_by(space, &dryer, None).unwrap(), Ok(()));
        assert_eq!(
            store.machine(space, &dryer).unwrap().unwrap().powered_by,
            None
        );
    }

    #[test]
    fn an_agent_behind_the_downloads_is_told() {
        let mut store = Store::open(":memory:").unwrap();
        let (space, _) = store.default_space().unwrap();
        let id = device(&mut store, space, "Dymo");
        let token = store.virtual_machines().unwrap()[0].1.clone();
        let report = |build: &str| Telemetry {
            agent: Some(maker_connect_protocol::AgentInfo {
                version: "0.1.0".into(),
                build: Some(build.into()),
                ..Default::default()
            }),
            ..Telemetry::default()
        };
        store.report(&token, &report("1d5c84c")).unwrap();
        assert_eq!(
            store.machine(space, &id).unwrap().unwrap().agent_update,
            None
        );
        std::fs::write(
            store.downloads_dir.join(".build"),
            "abc1234\n2026-11-04 15:00 UTC\n",
        )
        .unwrap();
        assert_eq!(
            store
                .machine(space, &id)
                .unwrap()
                .unwrap()
                .agent_update
                .as_deref(),
            Some("abc1234 (2026-11-04 15:00 UTC)")
        );
        store.report(&token, &report("abc1234")).unwrap();
        assert_eq!(
            store.machine(space, &id).unwrap().unwrap().agent_update,
            None
        );
    }
}
