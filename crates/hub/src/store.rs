//! What the hub keeps: spaces and their machines, in one SQLite file. Everything belongs
//! to a space from the start, so the same software can serve one makerspace on a Pi or
//! many on a hosted service.

use maker_connect_protocol::{MachineKind, Register, Telemetry};
use rand::Rng;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Whether machines on this link take commands from the website (yet).
pub fn takes_commands(link: &str) -> bool {
    matches!(link, "prusa-connect" | "virtual" | "agent" | "simplyprint")
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

/// Before another build changes a database, a copy of it as it was:
/// `maker-connect-backups/maker-connect-<when>-<from>-to-<to>.db` beside it. A customer's
/// data is never only in the one file an upgrade is about to change.
fn back_up_before_upgrade(db: &Connection, path: &str) -> rusqlite::Result<()> {
    let has_meta: bool = db.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
        |r| r.get(0),
    )?;
    let from: Option<String> = if has_meta {
        db.query_row("SELECT value FROM meta WHERE key = 'opened_by'", [], |r| {
            r.get(0)
        })
        .optional()?
    } else {
        None
    };
    if from.as_deref() == Some(BUILD) {
        return Ok(());
    }
    let file = std::path::Path::new(path);
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("maker-connect");
    let dir = file.with_file_name(format!("{stem}-backups"));
    std::fs::create_dir_all(&dir)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let safe = |s: &str| s.replace(|c: char| !c.is_ascii_alphanumeric() && c != '.', "-");
    let copy = dir.join(format!(
        "{stem}-{}-{}-to-{}.db",
        crate::spoolman::iso(now()).replace(':', ""),
        safe(from.as_deref().unwrap_or("before")),
        safe(BUILD),
    ));
    db.execute("VACUUM INTO ?1", [copy.to_string_lossy()])?;
    println!(
        "database backed up before this build changes it: {}",
        copy.display()
    );
    Ok(())
}

/// The tables as this build has them: made when missing, columns added to older ones, data
/// moved over. Only ever adds; runs in one transaction, so a failure leaves the database as
/// it was.
fn migrate(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS spaces (
             id INTEGER PRIMARY KEY,
             name TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS files (
             id TEXT PRIMARY KEY,
             space_id INTEGER NOT NULL REFERENCES spaces(id),
             name TEXT NOT NULL,
             size INTEGER NOT NULL,
             uploaded_at INTEGER NOT NULL,
             meta TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS jobs (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL,
             machine_id TEXT NOT NULL,
             file_id TEXT NOT NULL,
             file_name TEXT NOT NULL,
             state TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             started_at INTEGER,
             ended_at INTEGER,
             reason TEXT
         );
         CREATE TABLE IF NOT EXISTS machines (
             id TEXT PRIMARY KEY,
             space_id INTEGER REFERENCES spaces(id),
             name TEXT NOT NULL,
             kind TEXT NOT NULL,
             agent_version TEXT NOT NULL,
             token TEXT NOT NULL UNIQUE,
             pairing_code TEXT,
             registered_at INTEGER NOT NULL,
             last_seen INTEGER,
             telemetry TEXT
         );",
    )?;
    for (column, definition) in [
        ("from_line", "INTEGER"),
        // What a job used, and of which spool (counted off when it ends).
        ("used", "REAL"),
        ("spool_id", "INTEGER"),
        ("restart_line", "INTEGER"),
        // When someone took a finished print off the bed.
        ("taken_at", "INTEGER"),
        // Who started it: a person's or a token's name, or the queue's.
        ("by_name", "TEXT"),
    ] {
        let present: bool = db.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('jobs') WHERE name = ?1",
            [column],
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )?;
        if !present {
            db.execute(
                &format!("ALTER TABLE jobs ADD COLUMN {column} {definition}"),
                [],
            )?;
            // The prints from before were taken long ago.
            if column == "taken_at" {
                db.execute(
                    "UPDATE jobs SET taken_at = ?1 WHERE state IN ('finished', 'stopped', 'failed')",
                    [now()],
                )?;
            }
        }
    }
    // Columns added after the first version: a store from then gets them.
    for (column, definition) in [
        ("profile", "TEXT"),
        ("profile_chosen", "INTEGER NOT NULL DEFAULT 0"),
        ("link", "TEXT NOT NULL DEFAULT 'agent'"),
        ("details", "TEXT"),
        ("mesh", "TEXT"),
        ("media", "TEXT"),
        // The device tree (docs/integrations.md, "Devices in a tree").
        ("powered_by", "TEXT"),
        ("power_channel", "INTEGER"),
        ("reached_through", "TEXT"),
        // What an agent tells of itself, and where its reports come from.
        ("agent", "TEXT"),
        ("peer", "TEXT"),
        // Taken out of use: why (may be empty), and since when.
        ("out_of_use", "TEXT"),
        ("out_of_use_at", "INTEGER"),
        // A laser's module, as its owner says (the controller does not know it).
        ("laser_w", "REAL"),
        // Only those with an induction (a grant) may use it (crate::access).
        // 0: open to every member; 1: needs permission; 2: needs training (crate::access).
        ("induction_required", "INTEGER NOT NULL DEFAULT 0"),
        // What someone must have and wear to use it, comma separated ("laser safety
        // glasses"); empty: nothing; NULL: what its kind needs (crate::safety).
        ("ppe", "TEXT"),
        // Labels its owners gave it, as a JSON list.
        ("labels", "TEXT"),
        // Who started a print from its USB stick on the website, and when (no job row:
        // the file is the printer's).
        ("usb_print_by", "TEXT"),
        ("usb_print_at", "INTEGER"),
        // An update its agent was asked for (`agent-v0.3.0`, crate::updates), and when.
        ("agent_update_to", "TEXT"),
        ("agent_update_at", "INTEGER"),
    ] {
        let present: bool = db.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('machines') WHERE name = ?1",
            [column],
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )?;
        if !present {
            db.execute(
                &format!("ALTER TABLE machines ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    crate::stock::migrate(db)?;
    let spaces: i64 = db.query_row("SELECT COUNT(*) FROM spaces", [], |r| r.get(0))?;
    if spaces == 0 {
        db.execute("INSERT INTO spaces (name) VALUES ('My space')", [])?;
    }
    // After the first space: it gets its organisation and its id there.
    crate::accounts::migrate(db)?;
    crate::printer_files::migrate(db)?;
    crate::farm::migrate(db)?;
    crate::queue::migrate(db)?;
    db.execute(
        "INSERT INTO meta (key, value) VALUES ('opened_by', ?1)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [BUILD],
    )?;
    Ok(())
}

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

/// Keeps a machine's commands to [`COMMANDS_KEPT`]: the oldest done ones go first, the
/// hub's own before someone's; one still waiting or on its way is never dropped.
/// Whether G-code may go into a running print: speed, flow, nozzle and bed targets, and a
/// relative X or Y move (no Z, no extruding), as PrusaLink's controls while printing.
pub(crate) fn fits_a_print(gcode: &str) -> bool {
    gcode.lines().map(str::trim).filter(|l| !l.is_empty()).all(|line| {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.first().copied() {
            Some("M220" | "M221" | "M104" | "M140") => words.len() == 2 && words[1].starts_with('S'),
            Some("G91" | "G90") => words.len() == 1,
            Some("G1") => words[1..].iter().all(|w| w.starts_with('F') || w.starts_with('X') || w.starts_with('Y')),
            _ => false,
        }
    })
}

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
            JobState::Queued | JobState::Printing | JobState::Paused | JobState::Stopping
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
    /// What is on an agent's printer's own SD card, as its `M20` said, and when
    /// (crate::sd_card).
    pub(crate) sd_files: std::collections::HashMap<String, (Vec<crate::sd_card::SdFile>, i64)>,
    /// Pause, resume or stop for a print the machine started itself (from its SD card): no
    /// job of the hub's, so said once with its next report.
    pub(crate) own_job_control:
        std::collections::HashMap<String, maker_connect_protocol::JobControl>,
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
            sd_files: Default::default(),
            own_job_control: Default::default(),
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

    /// A file's place on disk.
    /// Where a file is kept. Named `.gcode` from the first version, whatever is in it (a
    /// label is a PNG), so stores from then keep working.
    pub fn file_path(&self, id: &str) -> std::path::PathBuf {
        self.files_dir.join(format!("{id}.gcode"))
    }

    pub fn thumbnail_path(&self, id: &str) -> std::path::PathBuf {
        self.files_dir.join(format!("{id}.png"))
    }

    /// A new file of a space, already on disk at [`Store::file_path`] of `id`.
    pub fn add_file(
        &mut self,
        space: i64,
        id: &str,
        name: &str,
        size: u64,
        meta: &crate::gcode::FileMeta,
    ) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO files (id, space_id, name, size, uploaded_at, meta) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, space, name, size as i64, now(), serde_json::to_string(meta).unwrap_or_default()],
        )?;
        Ok(())
    }

    /// A space's files, newest first.
    pub fn files(&self, space: i64) -> rusqlite::Result<Vec<StoredFile>> {
        let mut query = self.db.prepare(
            "SELECT id, name, size, uploaded_at, meta FROM files WHERE space_id = ?1 ORDER BY uploaded_at DESC, name",
        )?;
        let rows = query.query_map([space], |r| {
            let meta: String = r.get(4)?;
            Ok(StoredFile {
                id: r.get(0)?,
                name: r.get(1)?,
                size: r.get::<_, i64>(2)? as u64,
                uploaded_at: r.get(3)?,
                meta: serde_json::from_str(&meta).unwrap_or_default(),
            })
        })?;
        rows.collect()
    }

    pub fn file(&self, space: i64, id: &str) -> rusqlite::Result<Option<StoredFile>> {
        Ok(self.files(space)?.into_iter().find(|f| f.id == id))
    }

    /// Reads again what the hub keeps about every file it kept before it read as much as it
    /// does now (where the moves go, the highest power; temperatures, colour, objects and
    /// QOI and JPG thumbnails, as Prusa's gcode-metadata reads them), and what its name
    /// tells; a thumbnail it did not have is kept now.
    pub fn reread_files(&mut self) -> rusqlite::Result<usize> {
        let mut query = self.db.prepare("SELECT id, name, meta FROM files")?;
        let stale: Vec<(String, String)> = query
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })?
            .filter_map(Result::ok)
            .filter(|(_, _, meta)| !meta.contains("bounds_mm") || !meta.contains("\"objects\""))
            .map(|(id, name, _)| (id, name))
            .collect();
        drop(query);
        for (id, name) in &stale {
            if let Ok((mut meta, png)) = crate::gcode::read(&self.file_path(id)) {
                crate::gcode::from_name(&mut meta, name);
                let thumbnail = self.thumbnail_path(id);
                if let Some(png) = png
                    && !thumbnail.exists()
                {
                    let _ = std::fs::write(&thumbnail, png);
                }
                self.db.execute(
                    "UPDATE files SET meta = ?1 WHERE id = ?2",
                    params![serde_json::to_string(&meta).unwrap_or_default(), id],
                )?;
            }
        }
        Ok(stale.len())
    }

    /// Removes a file, from the store and from disk. Returns whether it was there.
    pub fn remove_file(&mut self, space: i64, id: &str) -> rusqlite::Result<bool> {
        // Not while a machine runs it (or has it queued).
        let in_use: i64 = self.db.query_row(
            "SELECT COUNT(*) FROM jobs WHERE file_id = ?1 AND state IN ('queued', 'printing', 'paused', 'stopping')",
            [id],
            |r| r.get(0),
        )?;
        if in_use > 0 {
            return Ok(false);
        }
        let removed = self.db.execute(
            "DELETE FROM files WHERE id = ?1 AND space_id = ?2",
            params![id, space],
        )?;
        if removed > 0 {
            let _ = std::fs::remove_file(self.file_path(id));
            let _ = std::fs::remove_file(self.thumbnail_path(id));
        }
        Ok(removed > 0)
    }

    /// Where each file of a space was printed, newest first, at most `per_file` times each.
    pub fn prints(
        &self,
        space: i64,
        per_file: usize,
    ) -> rusqlite::Result<std::collections::HashMap<String, Vec<Printed>>> {
        let mut query = self.db.prepare(
            "SELECT file_id, machine_id, state, COALESCE(ended_at, started_at, created_at) AS at
             FROM jobs WHERE space_id = ?1 ORDER BY at DESC, id DESC",
        )?;
        let rows = query.query_map([space], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Printed {
                    machine_id: r.get(1)?,
                    state: JobState::parse(&r.get::<_, String>(2)?),
                    at: r.get(3)?,
                },
            ))
        })?;
        let mut prints: std::collections::HashMap<String, Vec<Printed>> = Default::default();
        for row in rows {
            let (file, printed) = row?;
            let list = prints.entry(file).or_default();
            if list.len() < per_file {
                list.push(printed);
            }
        }
        Ok(prints)
    }

    /// Puts a file on a machine: its job's id, or why not (the machine has a job already).
    pub fn add_job(
        &mut self,
        space: i64,
        machine: &str,
        file: &StoredFile,
        from_line: Option<u64>,
        by: Option<&str>,
    ) -> rusqlite::Result<Result<i64, &'static str>> {
        if self.active_job(machine)?.is_some() {
            return Ok(Err("the machine has a job already"));
        }
        self.db.execute(
            "INSERT INTO jobs (space_id, machine_id, file_id, file_name, state, created_at, from_line, by_name)
             VALUES (?1, ?2, ?3, ?4, 'queued', ?5, ?6, ?7)",
            params![space, machine, file.id, file.name, now(), from_line.map(|l| l as i64), by],
        )?;
        self.changed();
        Ok(Ok(self.db.last_insert_rowid()))
    }

    fn job_from(r: &rusqlite::Row) -> rusqlite::Result<Job> {
        Ok(Job {
            id: r.get(0)?,
            machine_id: r.get(1)?,
            file_id: r.get(2)?,
            file_name: r.get(3)?,
            state: JobState::parse(&r.get::<_, String>(4)?),
            created_at: r.get(5)?,
            started_at: r.get(6)?,
            ended_at: r.get(7)?,
            reason: r.get(8)?,
            from_line: r.get::<_, Option<i64>>(9)?.map(|l| l as u64),
            restart_line: r.get::<_, Option<i64>>(10)?.map(|l| l as u64),
            by: r.get(11)?,
        })
    }

    /// A machine's job that is not over yet.
    pub fn active_job(&self, machine: &str) -> rusqlite::Result<Option<Job>> {
        self.db
            .query_row(
                "SELECT id, machine_id, file_id, file_name, state, created_at, started_at, ended_at, reason,
                     from_line, restart_line, by_name
                 FROM jobs WHERE machine_id = ?1 AND state IN ('queued', 'printing', 'paused', 'stopping')
                 ORDER BY id LIMIT 1",
                [machine],
                Self::job_from,
            )
            .optional()
    }

    pub fn job(&self, id: i64) -> rusqlite::Result<Option<Job>> {
        self.db
            .query_row(
                "SELECT id, machine_id, file_id, file_name, state, created_at, started_at, ended_at, reason,
                     from_line, restart_line, by_name
                 FROM jobs WHERE id = ?1",
                [id],
                Self::job_from,
            )
            .optional()
    }

    /// Where a job that did not run to the end could start again.
    pub fn set_restart_line(&mut self, id: i64, line: u64) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE jobs SET restart_line = ?1 WHERE id = ?2",
            params![line as i64, id],
        )?;
        Ok(())
    }

    /// A machine's last job, when it was interrupted and says where to start again.
    pub fn resumable_job(&self, machine: &str) -> rusqlite::Result<Option<Job>> {
        let last = self
            .db
            .query_row(
                "SELECT id, machine_id, file_id, file_name, state, created_at, started_at, ended_at, reason,
                     from_line, restart_line, by_name
                 FROM jobs WHERE machine_id = ?1 ORDER BY id DESC LIMIT 1",
                [machine],
                Self::job_from,
            )
            .optional()?;
        Ok(last.filter(|j| {
            matches!(j.state, JobState::Stopped | JobState::Failed) && j.restart_line.is_some()
        }))
    }

    /// A print still on the bed: the machine's last job, when it finished and nobody has
    /// said yet that it was taken.
    pub fn on_bed(&self, machine: &str) -> rusqlite::Result<Option<OnBed>> {
        let last = self
            .db
            .query_row(
                "SELECT id, file_name, state, ended_at, taken_at FROM jobs WHERE machine_id = ?1
                 ORDER BY id DESC LIMIT 1",
                [machine],
                |r| {
                    Ok((
                        OnBed {
                            job: r.get(0)?,
                            file_name: r.get(1)?,
                            ended_at: r.get(3)?,
                        },
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<i64>>(4)?,
                    ))
                },
            )
            .optional()?;
        Ok(last
            .filter(|(_, state, taken)| state == "finished" && taken.is_none())
            .map(|(on_bed, _, _)| on_bed))
    }

    /// Ready for the next job: its last job finished and was taken off.
    pub fn ready_after(&self, machine: &str) -> rusqlite::Result<Option<OnBed>> {
        let last = self
            .db
            .query_row(
                "SELECT id, file_name, state, ended_at, taken_at FROM jobs WHERE machine_id = ?1
                 ORDER BY id DESC LIMIT 1",
                [machine],
                |r| {
                    Ok((
                        OnBed { job: r.get(0)?, file_name: r.get(1)?, ended_at: r.get(3)? },
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<i64>>(4)?,
                    ))
                },
            )
            .optional()?;
        Ok(last.filter(|(_, state, taken)| state == "finished" && taken.is_some()).map(|(r, _, _)| r))
    }

    /// Not ready after all (PrusaLink's `cancel_ready`): the last print counts as on its bed
    /// again, and the next job waits. Prusa's MK3 through an agent is told so (M72 S0, as
    /// PrusaLink does, whether it was ready or not). Returns whether it was ready.
    pub fn not_ready(&mut self, space: i64, machine: &str) -> rusqlite::Result<bool> {
        let Some(m) = self.machine(space, machine)? else {
            return Ok(false);
        };
        if m.link == "agent" && crate::prusa_mk3_machine(&m) {
            self.queue_command(space, machine, "Not ready on its screen", "M72 S0")?;
        }
        let Some(ready) = self.ready_after(machine)? else {
            return Ok(false);
        };
        self.db.execute("UPDATE jobs SET taken_at = NULL WHERE id = ?1", [ready.job])?;
        self.changed();
        Ok(true)
    }

    /// Someone took the print off the machine's bed (a finished one, or what a failed or
    /// stopped job left): the bed is clear. Returns whether there was one.
    pub fn print_taken(&mut self, space: i64, machine: &str) -> rusqlite::Result<bool> {
        if self.machine(space, machine)?.is_none() {
            return Ok(false);
        }
        let n = self.db.execute(
            "UPDATE jobs SET taken_at = ?1 WHERE machine_id = ?2 AND state IN ('finished', 'stopped', 'failed') AND taken_at IS NULL",
            params![now(), machine],
        )?;
        self.changed();
        Ok(n > 0)
    }

    /// A laser's optical power as its owner says (None: the profile's). Returns whether the
    /// machine is in the space.
    pub fn set_laser_w(
        &self,
        space: i64,
        machine: &str,
        watts: Option<f64>,
    ) -> rusqlite::Result<bool> {
        let n = self.db.execute(
            "UPDATE machines SET laser_w = ?1 WHERE id = ?2 AND space_id = ?3",
            params![watts, machine, space],
        )?;
        self.changed();
        Ok(n > 0)
    }

    /// Takes a machine out of use (with why), or back into use (None). Returns whether the
    /// machine is in the space.
    pub fn set_out_of_use(
        &self,
        space: i64,
        machine: &str,
        reason: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let n = self.db.execute(
            "UPDATE machines SET out_of_use = ?1, out_of_use_at = CASE WHEN ?1 IS NULL THEN NULL ELSE ?2 END
             WHERE id = ?3 AND space_id = ?4",
            params![reason, now(), machine, space],
        )?;
        self.changed();
        Ok(n > 0)
    }

    /// A job moves on: started, paused, asked to stop, over.
    pub fn set_job(
        &mut self,
        id: i64,
        state: JobState,
        reason: Option<&str>,
    ) -> rusqlite::Result<()> {
        let now = now();
        self.db.execute(
            "UPDATE jobs SET state = ?1, reason = COALESCE(?2, reason),
                 started_at = CASE WHEN ?1 = 'printing' AND started_at IS NULL THEN ?3 ELSE started_at END,
                 ended_at = CASE WHEN ?1 IN ('finished', 'stopped', 'failed') THEN ?3 ELSE ended_at END
             WHERE id = ?4",
            params![state.name(), reason, now, id],
        )?;
        if matches!(
            state,
            JobState::Finished | JobState::Stopped | JobState::Failed
        ) {
            self.count_off(id, state)?;
        }
        self.changed();
        Ok(())
    }

    /// A job that ended comes back (a power cut stopped it and its agent kept it): running or
    /// paused again, no end, with exactly this reason (None clears it). What it counted off
    /// its spool as it failed goes back on, booked as such, so its end counts it whole.
    pub fn revive_job(
        &mut self,
        id: i64,
        state: JobState,
        reason: Option<&str>,
    ) -> rusqlite::Result<()> {
        let counted: Option<(Option<f64>, Option<i64>, String, String)> = self
            .db
            .query_row(
                "SELECT used, spool_id, machine_id, file_name FROM jobs WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        if let Some((Some(used), Some(spool), machine, file_name)) = counted
            && used > 0.0
        {
            self.db.execute(
                "UPDATE spool SET used_weight = MAX(used_weight - ?1, 0) WHERE id = ?2",
                params![used, spool],
            )?;
            let space: Option<i64> = self
                .db
                .query_row("SELECT space_id FROM spool WHERE id = ?1", [spool], |r| r.get(0))
                .optional()?;
            if let Some(space) = space {
                self.movement(
                    space,
                    &crate::stock::spool_code(spool),
                    "used",
                    Some(used),
                    Some(&machine),
                    Some(&format!("job {file_name}: taken up after a power cut")),
                )?;
            }
        }
        self.db.execute(
            "UPDATE jobs SET state = ?1, reason = ?2, ended_at = NULL, used = NULL, spool_id = NULL
             WHERE id = ?3",
            params![state.name(), reason, id],
        )?;
        self.changed();
        Ok(())
    }

    /// A job that ended counts what it used off the spool loaded in its machine: all of
    /// what its file says when it finished, the part it got through when it stopped. Not when
    /// something else counted while it ran (Moonraker's `[spoolman]` does, as it prints).
    fn count_off(&mut self, id: i64, state: JobState) -> rusqlite::Result<()> {
        let Some(job) = self.job(id)? else {
            return Ok(());
        };
        let found: Option<(i64, i64, f64, f64, Option<String>)> = self
            .db
            .query_row(
                "SELECT s.id, s.space_id, f.density, f.diameter, f.unit FROM spool s
                 JOIN filament f ON f.id = s.filament_id
                 WHERE s.machine = ?1 AND s.archived = 0",
                [&job.machine_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((spool, space, density, diameter, unit)) = found else {
            return Ok(());
        };
        let counted_already: bool = self.db.query_row(
            "SELECT COUNT(*) > 0 FROM movements WHERE item = ?1 AND kind = 'used' AND note IS NULL AND at >= ?2",
            params![crate::stock::spool_code(spool), job.started_at.unwrap_or(job.created_at)],
            |r| r.get(0),
        )?;
        if counted_already {
            return Ok(());
        }
        let Some(file) = self.file(space, &job.file_id)? else {
            return Ok(());
        };
        let unit = unit.unwrap_or_else(|| "g".to_owned());
        // A die-cut label: one off the roll, also when it went wrong half-way (it is spoilt).
        // A label on tape: its length, and a lead before and after.
        let whole = if unit == "labels" {
            file.meta.label_dots.map(|_| 1.0)
        } else if unit == "m" {
            let dpi = self
                .machine(space, &job.machine_id)?
                .and_then(|m| m.profile)
                .and_then(|p| crate::profiles::find(&p))
                .and_then(|p| p.dpi)
                .unwrap_or(180.0);
            file.meta
                .label_dots
                .map(|[w, _]| (f64::from(w) / dpi * 25.4 + 16.0) / 1000.0)
        } else {
            file.meta.filament_g.or_else(|| {
                // Metres as grams: the length's volume times the density.
                let area = std::f64::consts::PI * (diameter / 2.0).powi(2);
                file.meta
                    .filament_m
                    .map(|m| m * 1000.0 * area * density / 1000.0)
            })
        };
        let Some(whole) = whole.filter(|w| *w > 0.0) else {
            return Ok(());
        };
        let share = match state {
            JobState::Finished => 1.0,
            // As far as it got: its last progress.
            _ => self
                .machine(space, &job.machine_id)?
                .and_then(|m| m.telemetry)
                .and_then(|t| t.job)
                .filter(|j| j.total > 0)
                .map_or(0.0, |j| (j.done as f64 / j.total as f64).clamp(0.0, 1.0)),
        };
        let used = if unit == "labels" {
            if share > 0.0 { whole } else { 0.0 }
        } else {
            whole * share
        };
        if used <= 0.0 {
            return Ok(());
        }
        let t = now();
        self.db.execute(
            "UPDATE spool SET used_weight = MAX(used_weight + ?1, 0), first_used = COALESCE(first_used, ?2),
                 last_used = ?2 WHERE id = ?3",
            params![used, t, spool],
        )?;
        self.db.execute(
            "UPDATE jobs SET used = ?1, spool_id = ?2 WHERE id = ?3",
            params![used, spool, id],
        )?;
        self.movement(
            space,
            &crate::stock::spool_code(spool),
            "used",
            Some(-used),
            Some(&job.machine_id),
            Some(&format!("job {}", job.file_name)),
        )?;
        self.spoolman_event("spool", "updated", spool);
        Ok(())
    }

    /// The space a self-hosted hub shows: its first.
    pub fn default_space(&self) -> rusqlite::Result<(i64, String)> {
        self.db
            .query_row("SELECT id, name FROM spaces ORDER BY id LIMIT 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
    }

    /// A new agent: its machine, not yet in a space, with a token and a pairing code.
    pub fn register(&self, agent: &Register) -> rusqlite::Result<(String, String, String)> {
        self.register_linked(agent, "agent")
    }

    /// A new machine that reaches the hub by `link` (see [`Machine::link`]).
    pub fn register_linked(
        &self,
        agent: &Register,
        link: &str,
    ) -> rusqlite::Result<(String, String, String)> {
        let mut rng = rand::thread_rng();
        let token: String = if link == "prusa-connect" {
            // Buddy firmware keeps at most 19 characters of a token: 19 of 62 is 113 bits.
            const ALPHANUMERIC: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
            (0..19)
                .map(|_| ALPHANUMERIC[rng.gen_range(0..ALPHANUMERIC.len())] as char)
                .collect()
        } else {
            (0..32)
                .map(|_| format!("{:02x}", rng.r#gen::<u8>()))
                .collect()
        };
        let id: String = (0..8)
            .map(|_| format!("{:02x}", rng.r#gen::<u8>()))
            .collect();
        let code: String = (0..6)
            .map(|_| CODE_CHARS[rng.gen_range(0..CODE_CHARS.len())] as char)
            .collect();
        self.db.execute(
            // A new machine needs permission: members get it per machine or per group.
            "INSERT INTO machines (id, name, kind, agent_version, token, pairing_code, registered_at, link, induction_required)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1)",
            params![
                id,
                agent.name,
                serde_json::to_string(&agent.kind).unwrap_or_default(),
                agent.agent_version,
                token,
                code,
                now(),
                link
            ],
        )?;
        self.changed();
        Ok((id, token, code))
    }

    /// A Prusa Connect printer asking with its code whether it has been added: None for an
    /// unknown code, `Some(None)` while it waits, `Some(Some(token))` once it is in a space.
    /// The token goes out once: the code is forgotten then.
    pub fn token_for_code(&self, code: &str) -> rusqlite::Result<Option<Option<String>>> {
        let found: Option<(String, Option<i64>)> = self
            .db
            .query_row(
                "SELECT token, space_id FROM machines WHERE pairing_code = ?1 AND link = 'prusa-connect'",
                [code],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match found {
            None => None,
            Some((_, None)) => Some(None),
            Some((token, Some(_))) => {
                self.db.execute(
                    "UPDATE machines SET pairing_code = NULL WHERE token = ?1",
                    [&token],
                )?;
                Some(Some(token))
            }
        })
    }

    /// The profile recognised for a machine from what it said when it registered, unless
    /// its owner chose one.
    pub fn recognise(&self, token: &str, profile: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE machines SET profile = ?1 WHERE token = ?2 AND profile_chosen = 0",
            params![profile, token],
        )?;
        Ok(())
    }

    /// A print the machine runs itself (no job of the hub's: from its USB stick or SD card,
    /// or started at its screen) is followed while it runs and recorded as a job when it
    /// ends: finished, stopped or failed, not yet taken off. So it shows among the machine's
    /// jobs, and what is left on the bed holds the queue, Files and Reprint back until
    /// someone says it was taken, as for the hub's own jobs (`files::fit`).
    ///
    /// How it ended: a Prusa on Connect says it (`raw`: FINISHED, STOPPED, ERROR); an agent
    /// only says it is idle again, so 99 % or more counts as finished, less as stopped (then
    /// "is the bed clear?" is asked, the careful side). A question on its screen (ATTENTION)
    /// or going offline is no end.
    pub fn follow_own_print(
        &mut self,
        token: &str,
        telemetry: &Telemetry,
        raw: Option<&str>,
    ) -> rusqlite::Result<()> {
        use maker_connect_protocol::MachineState as S;
        let Some((machine, space)) = self
            .db
            .query_row(
                "SELECT id, space_id FROM machines WHERE token = ?1 AND space_id IS NOT NULL",
                [token],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?
        else {
            return Ok(());
        };
        if self.active_job(&machine)?.is_some() {
            self.own_prints.remove(&machine);
            return Ok(());
        }
        let by: Option<String> = self
            .db
            .query_row("SELECT usb_print_by FROM machines WHERE id = ?1", [&machine], |r| r.get(0))
            .optional()?
            .flatten();
        match (telemetry.state, telemetry.job.as_ref()) {
            (S::Busy | S::Paused, Some(job)) => {
                let percent = job.done * 100 / job.total.max(1);
                let started = now() - job.elapsed_s.unwrap_or(0) as i64;
                let own = self.own_prints.entry(machine).or_insert(OwnPrint {
                    file: String::new(),
                    by: None,
                    started_at: started,
                    percent: 0,
                });
                if !job.file.is_empty() {
                    own.file = job.file.clone();
                }
                own.by = own.by.take().or(by);
                own.percent = percent;
            }
            // Still the same print: a question on its screen, or out of touch.
            (S::Attention | S::Disconnected, _) => {}
            _ => {
                let Some(own) = self.own_prints.remove(&machine) else {
                    return Ok(());
                };
                let (state, reason) = match raw {
                    Some("FINISHED") => (JobState::Finished, None),
                    Some("STOPPED") => (JobState::Stopped, Some("stopped on the printer")),
                    Some("ERROR") => (JobState::Failed, Some("the printer reports an error")),
                    _ if telemetry.state == S::Error => {
                        (JobState::Failed, Some("the printer reports an error"))
                    }
                    _ if own.percent >= 99 => (JobState::Finished, None),
                    _ => (JobState::Stopped, Some("ended at the printer before it was done")),
                };
                let file = if own.file.is_empty() {
                    "a print started on the printer".to_owned()
                } else {
                    own.file
                };
                self.db.execute(
                    "INSERT INTO jobs (space_id, machine_id, file_id, file_name, state, created_at, started_at, ended_at, reason, by_name)
                     VALUES (?1, ?2, '', ?3, ?4, ?5, ?5, ?6, ?7, ?8)",
                    params![
                        space,
                        machine,
                        file,
                        state.name(),
                        own.started_at,
                        now(),
                        reason,
                        own.by.as_deref().unwrap_or("on the printer")
                    ],
                )?;
                self.changed();
            }
        }
        Ok(())
    }

    /// An agent's report: kept as the machine's last. Returns the machine's space name if
    /// it is paired, or None when the token is unknown.
    pub fn report(
        &mut self,
        token: &str,
        telemetry: &Telemetry,
    ) -> rusqlite::Result<Option<Option<String>>> {
        // A mesh and command results are news once, not part of the machine's state.
        let kept = Telemetry {
            done: Vec::new(),
            mesh: None,
            job_end: None,
            bed_clear: false,
            not_ready: false,
            reprint: false,
            ..telemetry.clone()
        };
        // "Set Ready" on the printer's own menu: the bed is clear, as on its page.
        if telemetry.bed_clear
            && let Some((machine, space)) = self
                .db
                .query_row(
                    "SELECT id, space_id FROM machines WHERE token = ?1 AND space_id IS NOT NULL",
                    [token],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional()?
        {
            self.print_taken(space, &machine)?;
            // As PrusaLink answers it (`fw_set_ready`: `SetReady`, M72 S1).
            if self.machine(space, &machine)?.is_some_and(|m| m.link == "agent" && crate::prusa_mk3_machine(&m)) {
                self.queue_command(space, &machine, "Ready on its screen", "M72 S1")?;
            }
        }
        // "Not ready" on the printer's own menu: the next job waits again (PrusaLink's
        // `fw_cancel_ready`: `CancelReady`, M72 S0).
        if telemetry.not_ready
            && let Some((machine, space)) = self
                .db
                .query_row(
                    "SELECT id, space_id FROM machines WHERE token = ?1 AND space_id IS NOT NULL",
                    [token],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional()?
        {
            self.not_ready(space, &machine)?;
        }
        // "Reprint" on the printer's own menu: its last job again, where it may start.
        if telemetry.reprint
            && let Some((machine, space)) = self
                .db
                .query_row(
                    "SELECT id, space_id FROM machines WHERE token = ?1 AND space_id IS NOT NULL",
                    [token],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional()?
        {
            self.reprint(space, &machine)?;
        }
        let updated = self.db.execute(
            "UPDATE machines SET last_seen = ?1, telemetry = ?2 WHERE token = ?3",
            params![
                now(),
                serde_json::to_string(&kept).unwrap_or_default(),
                token
            ],
        )?;
        if let Some(agent) = &telemetry.agent {
            self.db.execute(
                "UPDATE machines SET agent = ?1 WHERE token = ?2",
                params![serde_json::to_string(agent).unwrap_or_default(), token],
            )?;
        }
        if let Some(mesh) = &telemetry.mesh {
            let stored = StoredMesh {
                rows: mesh.rows.clone(),
                taken_at: now(),
            };
            self.db.execute(
                "UPDATE machines SET mesh = ?1 WHERE token = ?2",
                params![serde_json::to_string(&stored).unwrap_or_default(), token],
            )?;
        }
        if updated == 0 {
            return Ok(None);
        }
        if let Some(id) = self.machine_id(token)? {
            self.sample(&id, telemetry);
        }
        // What the machine says it is: its profile, unless the owner chose one.
        if let Some(profile) = telemetry
            .identity
            .as_ref()
            .and_then(crate::profiles::detect)
        {
            self.db.execute(
                "UPDATE machines SET profile = ?1 WHERE token = ?2 AND profile_chosen = 0",
                params![profile.id, token],
            )?;
        }
        let space: Option<String> = self
            .db
            .query_row(
                "SELECT spaces.name FROM machines LEFT JOIN spaces ON spaces.id = machines.space_id
                 WHERE machines.token = ?1",
                [token],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        self.changed();
        Ok(Some(space))
    }

    /// Where a machine's reports come from, as the hub sees it (its address, or its
    /// router's when it is elsewhere).
    pub fn set_peer(&self, token: &str, peer: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE machines SET peer = ?1 WHERE token = ?2",
            params![peer, token],
        )?;
        Ok(())
    }

    /// A machine's id by its token, if the token is known.
    pub fn machine_id_by_token(&self, token: &str) -> rusqlite::Result<Option<String>> {
        self.machine_id(token)
    }

    fn machine_id(&self, token: &str) -> rusqlite::Result<Option<String>> {
        self.db
            .query_row("SELECT id FROM machines WHERE token = ?1", [token], |r| {
                r.get(0)
            })
            .optional()
    }

    fn sample(&mut self, machine: &str, telemetry: &Telemetry) {
        let t = now();
        let history = self.history.entry(machine.to_owned()).or_default();
        if history
            .back()
            .is_some_and(|last| t - last.t < SAMPLE_SECONDS)
        {
            return;
        }
        history.push_back(Sample {
            t,
            nozzle: telemetry.nozzle_c,
            nozzle_target: telemetry.nozzle_target_c,
            bed: telemetry.bed_c,
            bed_target: telemetry.bed_target_c,
            print_fan: telemetry.print_fan_rpm,
            hotend_fan: telemetry.hotend_fan_rpm,
        });
        while history
            .front()
            .is_some_and(|first| t - first.t > HISTORY_SECONDS)
        {
            history.pop_front();
        }
    }

    /// A machine's temperatures over the last half hour, oldest first.
    pub fn history(&self, space: i64, machine: &str) -> rusqlite::Result<Option<Vec<Sample>>> {
        Ok(self.machine(space, machine)?.map(|_| {
            self.history
                .get(machine)
                .map(|h| h.iter().copied().collect())
                .unwrap_or_default()
        }))
    }

    /// The owner gives a machine a command: its id, or None when the machine is not in
    /// the space or cannot take commands (yet: only printers on built-in Prusa Connect).
    pub fn queue_command(
        &mut self,
        space: i64,
        machine: &str,
        label: &str,
        gcode: &str,
    ) -> rusqlite::Result<Option<u32>> {
        if self
            .machine(space, machine)?
            .is_none_or(|m| !takes_commands(&m.link))
        {
            return Ok(None);
        }
        self.next_command = self.next_command.wrapping_add(1);
        let id = self.next_command;
        let commands = self.commands.entry(machine.to_owned()).or_default();
        commands.push(Command {
            id,
            label: label.to_owned(),
            gcode: gcode.to_owned(),
            json: false,
            background: false,
            output: Vec::new(),
            state: CommandState::Queued,
            reason: None,
            at: now(),
        });
        trim(commands);
        self.changed();
        Ok(Some(id))
    }

    /// The next command for the machine asking with this token, once the one before is
    /// done: one at a time, as a printer takes them.
    pub fn next_command(&mut self, token: &str) -> rusqlite::Result<Option<(u32, String)>> {
        Ok(self
            .next_command_of_any_kind(token)?
            .map(|(id, body, _)| (id, body)))
    }

    /// The next command that may go into a running print (PrusaLink's controls while it
    /// prints: speed, flow, temperatures, an X or Y move); others wait for its end.
    pub fn next_command_for_a_print(&mut self, token: &str) -> rusqlite::Result<Option<(u32, String)>> {
        let Some(machine) = self.machine_id(token)? else {
            return Ok(None);
        };
        let first_queued = self
            .commands
            .get(&machine)
            .and_then(|c| c.iter().find(|c| c.state == CommandState::Queued).map(|c| c.gcode.clone()));
        match first_queued {
            Some(gcode) if fits_a_print(&gcode) => self.next_command(token),
            _ => Ok(None),
        }
    }

    /// A command of Prusa Connect's for a printer on its built-in Connect (its id): queued
    /// like G-code, sent as JSON (`SEND_FILE_INFO`, `START_PRINT`, `START_ENCRYPTED_DOWNLOAD`).
    pub fn queue_connect_command(
        &mut self,
        space: i64,
        machine: &str,
        label: &str,
        command: &serde_json::Value,
        background: bool,
    ) -> rusqlite::Result<Option<u32>> {
        if self
            .machine(space, machine)?
            .is_none_or(|m| m.link != "prusa-connect")
        {
            return Ok(None);
        }
        let body = command.to_string();
        // The hub's own reading (a folder, a file's picture) once: asked again while the
        // same is still waiting or on its way, it is not queued twice.
        if background
            && let Some(same) = self.commands.get(machine).and_then(|commands| {
                commands.iter().find(|c| {
                    c.gcode == body
                        && matches!(
                            c.state,
                            CommandState::Queued | CommandState::Sent | CommandState::Accepted
                        )
                })
            })
        {
            return Ok(Some(same.id));
        }
        self.next_command = self.next_command.wrapping_add(1);
        let id = self.next_command;
        let commands = self.commands.entry(machine.to_owned()).or_default();
        commands.push(Command {
            id,
            label: label.to_owned(),
            gcode: body,
            json: true,
            background,
            output: Vec::new(),
            state: CommandState::Queued,
            reason: None,
            at: now(),
        });
        trim(commands);
        self.changed();
        Ok(Some(id))
    }

    /// The commands of a machine still open (waiting, sent or taken on): what they send.
    pub fn open_commands(&self, machine: &str) -> Vec<String> {
        self.commands
            .get(machine)
            .into_iter()
            .flatten()
            .filter(|c| {
                matches!(
                    c.state,
                    CommandState::Queued | CommandState::Sent | CommandState::Accepted
                )
            })
            .map(|c| c.gcode.clone())
            .collect()
    }

    /// A command given to the machine with this token, by its id: what it sent (G-code or
    /// Connect's JSON).
    pub fn command_text(&self, token: &str, id: u32) -> rusqlite::Result<Option<String>> {
        let Some(machine) = self.machine_id(token)? else {
            return Ok(None);
        };
        Ok(self
            .commands
            .get(&machine)
            .and_then(|commands| commands.iter().find(|c| c.id == id))
            .map(|c| c.gcode.clone()))
    }

    /// The paths on a printer's stick with a Connect command for them still open (waiting,
    /// sent or taken on), with the command's name (`DELETE_FILE`, ...).
    pub fn open_path_commands(&self, machine: &str) -> Vec<(String, String)> {
        self.commands
            .get(machine)
            .into_iter()
            .flatten()
            .filter(|c| {
                c.json
                    && matches!(
                        c.state,
                        CommandState::Queued | CommandState::Sent | CommandState::Accepted
                    )
            })
            .filter_map(|c| {
                let body: serde_json::Value = serde_json::from_str(&c.gcode).ok()?;
                Some((
                    body["kwargs"]["path"].as_str()?.to_owned(),
                    body["command"].as_str()?.to_owned(),
                ))
            })
            .collect()
    }

    /// The next command, with whether it is a Connect (JSON) command.
    pub fn next_command_of_any_kind(
        &mut self,
        token: &str,
    ) -> rusqlite::Result<Option<(u32, String, bool)>> {
        let Some(machine) = self.machine_id(token)? else {
            return Ok(None);
        };
        let Some(commands) = self.commands.get_mut(&machine) else {
            return Ok(None);
        };
        let now = now();
        for busy in commands
            .iter_mut()
            .filter(|c| matches!(c.state, CommandState::Sent | CommandState::Accepted))
        {
            if now - busy.at < COMMAND_SECONDS {
                return Ok(None);
            }
            busy.state = CommandState::Rejected;
            busy.reason = Some("no answer from the machine".to_owned());
        }
        // Someone's commands before the hub's own (reading a USB stick in the background).
        let at = commands
            .iter()
            .position(|c| c.state == CommandState::Queued && !c.background)
            .or_else(|| {
                commands
                    .iter()
                    .position(|c| c.state == CommandState::Queued)
            });
        let next = at.map(|i| &mut commands[i]).map(|c| {
            c.state = CommandState::Sent;
            c.at = now;
            (c.id, c.gcode.clone(), c.json)
        });
        if next.is_some() {
            self.changed();
        }
        Ok(next)
    }

    /// The printer says in its telemetry it is still carrying out this command (Buddy sends
    /// the `command_id` of G-code running in the background): not "no answer" while it does.
    pub fn command_still_running(&mut self, token: &str, id: u32) -> rusqlite::Result<()> {
        let Some(machine) = self.machine_id(token)? else {
            return Ok(());
        };
        if let Some(command) = self.commands.get_mut(&machine).and_then(|commands| {
            commands.iter_mut().find(|c| {
                c.id == id && matches!(c.state, CommandState::Sent | CommandState::Accepted)
            })
        }) {
            command.state = CommandState::Accepted;
            command.at = now();
        }
        Ok(())
    }

    /// The machine tells how a command went.
    pub fn command_went(
        &mut self,
        token: &str,
        id: u32,
        state: CommandState,
        reason: Option<String>,
        output: Vec<String>,
    ) -> rusqlite::Result<()> {
        let Some(machine) = self.machine_id(token)? else {
            return Ok(());
        };
        if let Some(command) = self
            .commands
            .get_mut(&machine)
            .and_then(|commands| commands.iter_mut().find(|c| c.id == id))
        {
            command.state = state;
            command.reason = reason;
            command.output.extend(output);
            let excess = command.output.len().saturating_sub(OUTPUT_KEPT);
            command.output.drain(..excess);
            command.at = now();
            self.changed();
        }
        Ok(())
    }

    /// Whether a token belongs to a machine.
    pub fn knows(&self, token: &str) -> rusqlite::Result<bool> {
        self.db.query_row(
            "SELECT COUNT(*) FROM machines WHERE token = ?1",
            [token],
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )
    }

    /// A machine's last report, by its token.
    pub fn last_telemetry(&self, token: &str) -> rusqlite::Result<Option<Telemetry>> {
        let text: Option<String> = self
            .db
            .query_row(
                "SELECT telemetry FROM machines WHERE token = ?1",
                [token],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(text.and_then(|t| serde_json::from_str(&t).ok()))
    }

    /// What a machine tells about itself. Returns whether the token is known.
    pub fn set_details(&self, token: &str, details: &Details) -> rusqlite::Result<bool> {
        let updated = self.db.execute(
            "UPDATE machines SET details = ?1 WHERE token = ?2",
            params![serde_json::to_string(details).unwrap_or_default(), token],
        )?;
        self.changed();
        Ok(updated > 0)
    }

    /// Adds the machine with this pairing code to a space: its name, or None for an
    /// unknown code. A code is used once.
    /// Puts the machine waiting with this code in the space: its (id, name).
    pub fn pair(&self, space: i64, code: &str) -> rusqlite::Result<Option<(String, String)>> {
        let code = code.trim().to_ascii_uppercase();
        let name: Option<(String, String)> = self
            .db
            .query_row(
                "SELECT id, name FROM machines WHERE pairing_code = ?1 AND space_id IS NULL",
                [&code],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if name.is_some() {
            self.db.execute(
                // A Prusa Connect printer still asks with its code for its token.
                "UPDATE machines SET space_id = ?1,
                     pairing_code = CASE link WHEN 'prusa-connect' THEN pairing_code ELSE NULL END
                 WHERE pairing_code = ?2",
                params![space, code],
            )?;
            self.changed();
        }
        Ok(name)
    }

    /// A virtual machine of a profile, straight into the space: its id.
    pub fn add_virtual(
        &mut self,
        space: i64,
        profile: &crate::profiles::Profile,
    ) -> rusqlite::Result<String> {
        let (id, _, _) = self.register_linked(
            &Register {
                protocol: maker_connect_protocol::VERSION,
                name: format!("Virtual {}", profile.model),
                kind: profile.kind,
                agent_version: "simulated in the hub".to_owned(),
            },
            "virtual",
        )?;
        self.db.execute(
            "UPDATE machines SET space_id = ?1, pairing_code = NULL, profile = ?2, profile_chosen = 1
             WHERE id = ?3",
            params![space, profile.id, id],
        )?;
        self.changed();
        Ok(id)
    }

    /// A printer on Klipper, reached through Moonraker (its SimplyPrint component pointed
    /// here), straight into the space: its id.
    pub fn add_moonraker(&mut self, space: i64, name: &str) -> rusqlite::Result<String> {
        let (id, _, _) = self.register_linked(
            &Register {
                protocol: maker_connect_protocol::VERSION,
                name: name.to_owned(),
                kind: MachineKind::Fdm,
                agent_version: "Moonraker".to_owned(),
            },
            "simplyprint",
        )?;
        self.db.execute(
            "UPDATE machines SET space_id = ?1, pairing_code = NULL WHERE id = ?2",
            params![space, id],
        )?;
        self.changed();
        Ok(id)
    }

    /// The name of the Moonraker printer with this token, if it is one in a space.
    pub fn moonraker_name(&self, token: &str) -> rusqlite::Result<Option<String>> {
        self.db
            .query_row(
                "SELECT name FROM machines WHERE token = ?1 AND link = 'simplyprint' AND space_id IS NOT NULL",
                [token],
                |r| r.get(0),
            )
            .optional()
    }

    /// A Moonraker printer's token, for the address its config file names.
    pub fn moonraker_token(&self, space: i64, machine: &str) -> rusqlite::Result<Option<String>> {
        self.db
            .query_row(
                "SELECT token FROM machines WHERE id = ?1 AND space_id = ?2 AND link = 'simplyprint'",
                params![machine, space],
                |r| r.get(0),
            )
            .optional()
    }

    /// Every virtual machine: its id, token and profile.
    pub fn virtual_machines(&self) -> rusqlite::Result<Vec<(String, String, Option<String>)>> {
        let mut query = self
            .db
            .prepare("SELECT id, token, profile FROM machines WHERE link = 'virtual'")?;
        let rows = query.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    /// The owner removes a machine from the space: gone, with its token.
    /// Moves a machine to another space: what someone may do with it there is that space's
    /// business. Refused (with the reason) while it has work under way. What goes along is
    /// the machine itself (name, labels, plate, service interval) and its agent, which keeps
    /// its token. What stays: its jobs (their files are the old space's), the rights given
    /// on it (people of the old space), the spool said to be loaded (the old space's
    /// Materials), and its place in the old space's queue.
    pub fn move_machine(
        &mut self,
        from: i64,
        machine: &str,
        to: i64,
    ) -> rusqlite::Result<Result<(), String>> {
        let here: bool = self.db.query_row(
            "SELECT COUNT(*) FROM machines WHERE id = ?1 AND space_id = ?2",
            params![machine, from],
            |r| r.get(0),
        )?;
        if !here {
            return Ok(Err("no such machine".to_owned()));
        }
        if from == to {
            return Ok(Err("it is in that space already".to_owned()));
        }
        if self.active_job(machine)?.is_some() {
            return Ok(Err(
                "it has a job that is not over: stop it, or let it finish".to_owned(),
            ));
        }
        if self.waiting_confirm(machine)?.is_some() {
            return Ok(Err(
                "a plate on it waits to be confirmed: Set ready or Bad print first".to_owned(),
            ));
        }
        if self.on_bed(machine)?.is_some() {
            return Ok(Err(
                "a print is still on its bed: say it was taken first".to_owned()
            ));
        }
        let sending: i64 = self.db.query_row(
            "SELECT COUNT(*) FROM printer_transfers WHERE machine_id = ?1 AND state IN ('waiting', 'sent')",
            [machine],
            |r| r.get(0),
        )?;
        if sending > 0 {
            return Ok(Err(
                "a file is being sent to it: wait until it is there".to_owned()
            ));
        }
        let tx = self.db.transaction()?;
        tx.execute(
            "UPDATE machines SET space_id = ?2, farm = 0 WHERE id = ?1",
            params![machine, to],
        )?;
        tx.execute(
            "DELETE FROM machine_grants WHERE machine_id = ?1",
            [machine],
        )?;
        tx.execute(
            "DELETE FROM machine_group_members WHERE machine_id = ?1",
            [machine],
        )?;
        tx.execute(
            "UPDATE spool SET machine = NULL WHERE machine = ?1 AND space_id = ?2",
            params![machine, from],
        )?;
        tx.execute(
            "UPDATE printer_transfers SET state = 'failed', reason = 'the machine moved to another space'
             WHERE machine_id = ?1 AND state NOT IN ('finished', 'done', 'failed')",
            [machine],
        )?;
        tx.commit()?;
        // Commands asked in the old space are not the new one's.
        self.commands.remove(machine);
        self.changed();
        Ok(Ok(()))
    }

    pub fn remove(&mut self, space: i64, machine: &str) -> rusqlite::Result<bool> {
        let removed = self.db.execute(
            "DELETE FROM machines WHERE id = ?1 AND space_id = ?2",
            params![machine, space],
        )?;
        if removed > 0 {
            self.db.execute(
                "DELETE FROM machine_grants WHERE machine_id = ?1",
                [machine],
            )?;
            self.db.execute(
                "DELETE FROM machine_group_members WHERE machine_id = ?1",
                [machine],
            )?;
        }
        self.commands.remove(machine);
        self.history.remove(machine);
        self.changed();
        Ok(removed > 0)
    }

    /// The owner says what is loaded (a label printer's tape), or None. Returns whether
    /// the machine is in the space.
    pub fn set_media(
        &self,
        space: i64,
        machine: &str,
        media: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let updated = self.db.execute(
            "UPDATE machines SET media = ?1 WHERE id = ?2 AND space_id = ?3",
            params![media, machine, space],
        )?;
        self.changed();
        Ok(updated > 0)
    }

    /// The width of the tape a label printer has loaded, as its owner said.
    /// The tape or labels loaded in a label printer, as the owner said: (width across, a
    /// die-cut label's length) in mm.
    pub fn loaded_media_mm(&self, machine: &str) -> rusqlite::Result<Option<(u32, Option<u32>)>> {
        let found: Option<(Option<String>, Option<String>)> = self
            .db
            .query_row(
                "SELECT profile, media FROM machines WHERE id = ?1",
                [machine],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(found.and_then(|(profile, media)| {
            let profile = crate::profiles::find(&profile?)?;
            let media = crate::profiles::media(profile, &media?)?;
            Some((media.width_mm, media.length_mm))
        }))
    }

    /// Hangs a device behind a channel of another of the space (None: on no plug). Refused
    /// when the other is not in the space, or is behind this one already (a loop).
    pub fn set_powered_by(
        &self,
        space: i64,
        machine: &str,
        power: Option<&PowerLink>,
    ) -> rusqlite::Result<Result<(), &'static str>> {
        let parent = |id: &str| -> rusqlite::Result<Option<Option<String>>> {
            self.db
                .query_row(
                    "SELECT powered_by FROM machines WHERE id = ?1 AND space_id = ?2",
                    params![id, space],
                    |r| r.get(0),
                )
                .optional()
        };
        if parent(machine)?.is_none() {
            return Ok(Err("no such device"));
        }
        if let Some(power) = power {
            // Up from the new parent: reaching this device would close a loop.
            let mut at = Some(power.device.clone());
            let mut steps = 0;
            while let Some(id) = at {
                if id == machine {
                    return Ok(Err("that device gets its power through this one"));
                }
                match parent(&id)? {
                    Some(next) => at = next,
                    None if id == power.device => {
                        return Ok(Err("no such device to hang it behind"));
                    }
                    None => break,
                }
                steps += 1;
                if steps > 64 {
                    return Ok(Err("the power tree is too deep"));
                }
            }
        }
        self.db.execute(
            "UPDATE machines SET powered_by = ?1, power_channel = ?2 WHERE id = ?3 AND space_id = ?4",
            params![power.map(|p| &p.device), power.map(|p| p.channel), machine, space],
        )?;
        self.changed();
        Ok(Ok(()))
    }

    /// The owner renames a machine. Returns whether the machine is in the space.
    pub fn rename(&self, space: i64, machine: &str, name: &str) -> rusqlite::Result<bool> {
        let updated = self.db.execute(
            "UPDATE machines SET name = ?1 WHERE id = ?2 AND space_id = ?3",
            params![name, machine, space],
        )?;
        self.changed();
        Ok(updated > 0)
    }

    /// The owner chooses a machine's profile (None: back to recognising it). Returns
    /// whether the machine is in the space.
    pub fn set_profile(
        &self,
        space: i64,
        machine: &str,
        profile: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let updated = self.db.execute(
            "UPDATE machines SET profile = ?1, profile_chosen = ?2 WHERE id = ?3 AND space_id = ?4",
            params![profile, profile.is_some(), machine, space],
        )?;
        self.changed();
        Ok(updated > 0)
    }

    /// The agent build the Downloads page offers: its commit and when it was built, from
    /// `.build` beside the downloads (written by `scripts/build-downloads.sh`).
    pub fn offered_build(&self) -> Option<OfferedBuild> {
        let text = std::fs::read_to_string(self.downloads_dir.join(".build")).ok()?;
        let mut lines = text.lines().map(|l| l.trim().to_owned());
        let commit = lines.next().filter(|c| !c.is_empty())?;
        Some(OfferedBuild {
            commit,
            built: lines.next().unwrap_or_default(),
            version: lines.next().filter(|v| !v.is_empty()),
        })
    }

    /// A machine's labels: trimmed, without doubles, at most 12 of at most 24 characters.
    pub fn set_labels(
        &mut self,
        space: i64,
        machine: &str,
        labels: &[String],
    ) -> rusqlite::Result<bool> {
        let mut clean: Vec<String> = Vec::new();
        for label in labels {
            let label: String = label.trim().chars().take(24).collect();
            if !label.is_empty()
                && !clean.iter().any(|l| l.eq_ignore_ascii_case(&label))
                && clean.len() < 12
            {
                clean.push(label);
            }
        }
        let n = self.db.execute(
            "UPDATE machines SET labels = ?3 WHERE id = ?1 AND space_id = ?2",
            params![
                machine,
                space,
                serde_json::to_string(&clean).unwrap_or_default()
            ],
        )?;
        self.changed();
        Ok(n > 0)
    }

    /// A machine's latest jobs, newest first.
    pub fn jobs_of(&self, space: i64, machine: &str, limit: usize) -> rusqlite::Result<Vec<Job>> {
        let ids: Vec<i64> = {
            let mut q = self.db.prepare(
                "SELECT id FROM jobs WHERE space_id = ?1 AND machine_id = ?2 ORDER BY created_at DESC, id DESC LIMIT ?3",
            )?;
            q.query_map(params![space, machine, limit as i64], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        Ok(ids
            .into_iter()
            .filter_map(|id| self.job(id).ok().flatten())
            .collect())
    }

    /// A machine's ended jobs in numbers.
    pub fn job_stats(&self, space: i64, machine: &str) -> rusqlite::Result<JobStats> {
        let mut stats = JobStats::default();
        let mut q = self.db.prepare(
            "SELECT j.state, j.started_at, j.ended_at, f.meta FROM jobs j LEFT JOIN files f ON f.id = j.file_id
             WHERE j.space_id = ?1 AND j.machine_id = ?2 AND j.ended_at IS NOT NULL",
        )?;
        let rows = q.query_map(params![space, machine], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        stats.parts = self.parts_made(machine)?;
        for (state, started, ended, meta) in rows.flatten() {
            if let (Some(s), Some(e)) = (started, ended) {
                stats.seconds += (e - s).max(0);
            }
            match state.as_str() {
                "\"finished\"" | "finished" => {
                    stats.finished += 1;
                    stats.grams += meta
                        .and_then(|m| serde_json::from_str::<crate::gcode::FileMeta>(&m).ok())
                        .and_then(|m| m.filament_g)
                        .unwrap_or(0.0);
                }
                "\"stopped\"" | "stopped" => stats.stopped += 1,
                "\"failed\"" | "failed" => stats.failed += 1,
                _ => {}
            }
        }
        Ok(stats)
    }

    /// One machine of a space.
    pub fn machine(&self, space: i64, id: &str) -> rusqlite::Result<Option<Machine>> {
        Ok(self.machines(space)?.into_iter().find(|m| m.id == id))
    }

    /// A space's machines, by name.
    pub fn machines(&self, space: i64) -> rusqlite::Result<Vec<Machine>> {
        let mut query = self.db.prepare(
            "SELECT id, name, kind, agent_version, last_seen, telemetry, profile, profile_chosen, link, details, mesh,
                    media, powered_by, power_channel, reached_through, agent, peer, out_of_use, out_of_use_at,
                    laser_w, labels, induction_required, ppe, agent_update_to
             FROM machines WHERE space_id = ?1 ORDER BY name",
        )?;
        let now = now();
        let rows = query.query_map([space], |r| {
            let kind: String = r.get(2)?;
            let last_seen: Option<i64> = r.get(4)?;
            let telemetry: Option<String> = r.get(5)?;
            let seen = last_seen.map(|t| now - t);
            let online = seen.is_some_and(|s| s <= ONLINE_SECONDS);
            Ok(Machine {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: serde_json::from_str(&kind).unwrap_or(MachineKind::Other),
                agent_version: r.get(3)?,
                seen_seconds_ago: seen,
                online,
                telemetry: telemetry
                    .filter(|_| online)
                    .and_then(|t| serde_json::from_str(&t).ok()),
                profile: r.get(6)?,
                profile_chosen: r.get(7)?,
                link: r.get(8)?,
                details: r
                    .get::<_, Option<String>>(9)?
                    .and_then(|d| serde_json::from_str(&d).ok()),
                commands: Vec::new(),
                job: None,
                resumable: None,
                mesh: r
                    .get::<_, Option<String>>(10)?
                    .and_then(|m| serde_json::from_str(&m).ok()),
                media: r.get(11)?,
                powered_by: r.get::<_, Option<String>>(12)?.map(|device| PowerLink {
                    device,
                    channel: r.get::<_, Option<u32>>(13).ok().flatten().unwrap_or(0),
                }),
                reached_through: r.get(14)?,
                agent: r
                    .get::<_, Option<String>>(15)?
                    .and_then(|a| serde_json::from_str(&a).ok()),
                peer: r.get(16)?,
                agent_update: None,
                agent_update_to: r.get(23)?,
                out_of_use: r.get::<_, Option<String>>(17)?.map(|reason| OutOfUse {
                    reason,
                    since: r.get::<_, Option<i64>>(18).ok().flatten().unwrap_or(0),
                }),
                loaded: None,
                on_bed: None,
                ready: None,
                queue: Vec::new(),
                queue_held: None,
                asking: None,
                buttons: Vec::new(),
                usb_print_by: None,
                moonraker_token: None,
                laser_w: r.get(19)?,
                labels: r
                    .get::<_, Option<String>>(20)?
                    .and_then(|l| serde_json::from_str(&l).ok())
                    .unwrap_or_default(),
                induction_required: r.get::<_, i64>(21)? != 0,
                training: r.get::<_, i64>(21)? == 2,
                ppe: crate::safety::ppe_of(
                    &r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(22)?.as_deref(),
                ),
                confirm: None,
                farm: false,
                upkeep: None,
                sheet: None,
            })
        })?;
        let mut machines: Vec<Machine> = rows.collect::<rusqlite::Result<_>>()?;
        let offered = self.offered_build();
        let items = self.items(space)?;
        let materials = self.materials(space)?;
        for machine in &mut machines {
            machine.loaded = items
                .iter()
                .find(|i| i.machine.as_deref() == Some(&machine.id) && i.state == "loaded")
                .map(|item| {
                    let material = materials.iter().find(|m| m.id == item.material);
                    Loaded {
                        item: item.id.clone(),
                        name: material
                            .map(|m| m.color_name.clone().unwrap_or_else(|| m.name.clone()))
                            .unwrap_or_else(|| item.material.clone()),
                        color: material.and_then(|m| m.color.clone()),
                        nozzle_c: material.and_then(|m| m.nozzle_c),
                        bed_c: material.and_then(|m| m.bed_c),
                        material: item.material.parse::<i64>().ok().and_then(|id| {
                            self.db
                                .query_row(
                                    "SELECT material FROM filament WHERE id = ?1 AND kind = 'filament'",
                                    [id],
                                    |r| r.get::<_, Option<String>>(0),
                                )
                                .ok()
                                .flatten()
                        }),
                        place: item.location.as_ref().and_then(|code| {
                            self.db
                                .query_row(
                                    "SELECT name FROM locations WHERE id = ?1 AND space_id = ?2",
                                    params![code, space],
                                    |r| r.get(0),
                                )
                                .ok()
                        }),
                        slot: crate::stock::spool_number(&item.id).and_then(|id| {
                            self.db
                                .query_row("SELECT slot FROM spool WHERE id = ?1", [id], |r| {
                                    r.get::<_, Option<i64>>(0)
                                })
                                .ok()
                                .flatten()
                        }),
                        amount: item.amount,
                        unit: material.map_or("pcs".to_owned(), |m| m.unit.clone()),
                    }
                });
            if machine.kind == MachineKind::Fdm {
                machine.on_bed = self.on_bed(&machine.id)?;
                machine.ready = self.ready_after(&machine.id)?;
                machine.queue = self.queue_of(&machine.id)?;
                machine.asking = self.asking.get(&machine.id).cloned();
                machine.buttons = self
                    .dialogs
                    .get(&machine.id)
                    .map(|(buttons, _)| buttons.clone())
                    .unwrap_or_default();
                // An agent's printer asks through its report.
                if let Some(prompt) = machine
                    .telemetry
                    .as_ref()
                    .and_then(|t| t.prompt.as_ref())
                    .filter(|_| machine.online)
                {
                    machine.asking = Some((prompt.text.clone(), prompt.text.clone()));
                    machine.buttons = prompt.buttons.clone();
                }
                machine.queue_held = if machine.job.is_some() {
                    None
                } else {
                    self.bed_unchecked(&machine.id)?
                };
                machine.usb_print_by = self
                    .db
                    .query_row(
                        "SELECT usb_print_by FROM machines WHERE id = ?1",
                        [&machine.id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                // A plate for a part waits for someone to confirm it (crate::farm): that, not
                // "take the print".
                machine.confirm = self.waiting_confirm(&machine.id)?;
                if machine.confirm.is_some() {
                    machine.on_bed = None;
                }
                machine.farm = self.takes_work(&machine.id)?;
                machine.upkeep = self.upkeep(&machine.id)?;
                machine.sheet = self.sheet_of(&machine.id)?;
            }
            if machine.link == "simplyprint" {
                machine.moonraker_token = self.moonraker_token(space, &machine.id)?;
            }
            // An agent from before builds were named is behind any build on offer.
            if let (Some(offered), Some(agent)) = (&offered, machine.agent.as_ref())
                && agent.build.as_ref() != Some(&offered.commit)
            {
                machine.agent_update = Some(format!("{} ({})", offered.commit, offered.built));
            }
            machine.job = self.active_job(&machine.id)?;
            if machine.job.is_none() {
                machine.resumable = self.resumable_job(&machine.id)?;
            }
            if let Some(commands) = self.commands.get(&machine.id) {
                machine.commands = commands
                    .iter()
                    .rev()
                    .filter(|c| !c.background)
                    .cloned()
                    .collect();
            }
        }
        Ok(machines)
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
            .add_job(space, &printer, &file, None, None)
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
            .add_job(space, &printer, &file, None, None)
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
