//! What the hub keeps: spaces and their machines, on Postgres. Everything belongs
//! to a space from the start, so the same software can serve one makerspace on a Pi or
//! many on a hosted service.

use maker_connect_protocol::{MachineKind, Telemetry};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

mod machines;

mod pg_machines;
pub use pg_files::{INCOMING_KEEP_S, INCOMING_SWEEP_S};
pub use pg_machines::{AgentDevice, AgentOfHub, InstanceDevice, MoveRefused};

mod pg_files;
// Jobs on Postgres (lane 3b).
mod pg_jobs;
pub(crate) use pg_jobs::JobFacts;

mod pg_settings;

mod commands;

mod history;

mod agents;

mod files;

pub mod pg;

/// An error from either database while both are in use (transitional, lanes 1-4).
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Whether machines on this link take commands from the website (yet).
pub fn takes_commands(link: &str) -> bool {
    matches!(link, "prusa-connect" | "virtual" | "simplyprint")
}

/// Why a laser job is refused when nobody at the laser started it (our UI text).
pub const LASER_NEVER_ALONE: &str = "a laser starts only when someone at the laser starts it";
/// Why a job is not put on a file that is gone ([`Store::add_job`]): what main says of a missing
/// file (`files::find`).
pub const NO_SUCH_FILE: &str = "no such file";

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
        matches!(
            self,
            StartedBy::Person {
                session: Some(_),
                ..
            }
        )
    }

    fn name(&self) -> Option<&'a str> {
        match self {
            StartedBy::Person { user, .. } => Some(user),
            StartedBy::Auto { by } => *by,
        }
    }
}

/// A label job sent to a label printer as START_CONNECT_DOWNLOAD: its space, job and printer
/// (by token), whether its TRANSFER_FINISHED came, the reason of an `ERROR` that is the printer's
/// state now, and whether a stop went out (docs/design/wire.md, "When the hub calls a label
/// printed").
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LabelJob {
    pub space: i64,
    pub job: i64,
    pub token: String,
    pub downloaded: bool,
    pub error: Option<String>,
    pub stop_sent: bool,
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

/// A printer on its `/p/ws` socket counts as online while it sent a frame within this many
/// seconds (audit D11; and no command goes into the socket of a printer silent this long, D12).
/// Buddy sends telemetry at least every 15 s (`planner.cpp:97-99`) and pings after 15 s of
/// sending nothing (`connect.cpp:49-50`); it says nothing of when a server counts it gone. The
/// number is Prusa's server's (owner, 2026-11-24: "daar hebben we toch voorbeelden van"): it
/// takes a Buddy printer silent for 60 s as gone (`reference/prusa-connect`
/// `internal/connector/xbuddy/inbound.go:25-33`, `readDeadline = 60 * time.Second`; read, not
/// ported). Unlike that server the hub sends no ping and ends no socket for silence (#212).
pub const SILENT_SECONDS: i64 = 60;

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
    /// How it reaches the hub: `prusa-connect` (its own firmware, speaking Prusa Connect's printer
    /// protocol), `simplyprint` (Klipper through Moonraker), or `virtual` (simulated in the hub).
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
    /// Taken out of use by its owner (broken, waiting for a part): no jobs until it is back.
    pub out_of_use: Option<OutOfUse>,
    /// The spool (or cassette) loaded in it, from Materials.
    pub loaded: Option<Loaded>,
    /// A print that finished and is still on the bed, until someone says it was taken.
    pub on_bed: Option<OnBed>,
    /// Ready for the next job: its last print finished and was taken off (PrusaLink's
    /// READY), until a job starts or someone says it is not ready after all.
    pub ready: Option<OnBed>,
    /// Files waiting to go to its USB stick because a file by that name is there, until the
    /// user replaces it (owner, 2026-11-24, #212 c, B5).
    pub name_taken: Vec<NameTaken>,
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
    /// A Marlin printer's bed (X, Y, Z mm) as its space says it, for a profile from its firmware.
    pub bed_mm: Option<[f64; 3]>,
    /// Labels its owners gave it (`MK4-BL`, `Room 2`): to find it, filter on, group by.
    pub labels: Vec<String>,
    /// A door controller's names for its locks, in order; "" or missing: "Lock N".
    pub lock_names: Vec<String>,
    /// A door controller's relays that drive no lock (numbers from 0): hidden, never opened.
    pub locks_off: Vec<u32>,
    /// A door controller's locks open to every member (numbers from 0); any other lock needs
    /// permission through a group that holds it.
    pub locks_open: Vec<u32>,
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
    /// Its farm flag as kept, whatever its kind (`farm` shows it for a 3D printer only): the
    /// project queue's ([`crate::farm`]). Not sent.
    #[serde(skip)]
    pub takes_work: bool,
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

/// A file that waits to go to a printer's stick: a file by its name is there
/// (`printer_files::Store::place`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NameTaken {
    /// The transfer, as the Replace form names it.
    pub transfer: String,
    pub path: String,
    pub file_name: String,
    pub reason: Option<String>,
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
    /// The agent that hosts this device (`agent_id`, `docs/design/prusa-sdk-extension.md` part 6a):
    /// none for a Prusa, or an agent from before 6a.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// The additions this device uses (`extensions`, part 6a); none said is empty.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// The release of the agent that hosts it, as its `INFO` request said it (`User-Agent-Version`,
    /// part 6a); `firmware` is the device's own where the agent reads one (a Marlin's, part 9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_release: Option<String>,
    /// A Marlin printer's `M115` answer, its first line's keys and values (`firmware_info`,
    /// `docs/design/prusa-sdk-extension.md` part 9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware_info: Option<serde_json::Map<String, serde_json::Value>>,
    /// Its `Cap:` lines, each supported or not (`firmware_capabilities`, part 9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware_capabilities: Option<serde_json::Map<String, serde_json::Value>>,
    /// Its `area:` line, the travel (`full`) and the bed (`work`) (`firmware_area`, part 9, #152).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware_area: Option<serde_json::Map<String, serde_json::Value>>,
    /// A door controller's locks, numbered from 0 (`lock_count`, part 7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_count: Option<u32>,
}

impl Details {
    /// The bed the printer said (`firmware_area`'s `work`, `max` less `min` per axis), in mm: what
    /// Connect's `build_volume` means, "The space a print may fill on this printer" (part 9).
    pub fn said_bed_mm(&self) -> Option<[f64; 3]> {
        let work = self.firmware_area.as_ref()?.get("work")?;
        let size = |axis: &str| {
            let end = |e: &str| work.get(e)?.get(axis)?.as_f64();
            Some(end("max")? - end("min")?)
        };
        Some([size("x")?, size("y")?, size("z")?])
    }

    /// Whether the printer said it can do this (`Cap:<name>:1`, part 9).
    pub fn can(&self, capability: &str) -> bool {
        self.firmware_capabilities
            .as_ref()
            .and_then(|caps| caps.get(capability))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }
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
    /// Who asked for it, as the audit log names them (a door's Open, a board's update).
    pub by: Option<String>,
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

/// One point of a machine's history: the values of one time (crate::store::history).
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

/// What a machine's page shows of its history: the last 2 hours (ours: the page's choice).
pub const HISTORY_SECONDS: i64 = 2 * 3600;
/// Commands shown per machine.
const COMMANDS_KEPT: usize = 40;
/// Lines of a command's output kept.
const OUTPUT_KEPT: usize = 200;
/// A command given to a machine that does not answer about it is given up after this (not for
/// a printer on the built-in Prusa Connect: Buddy has no such time, see `PRINTER_GONE`).
const COMMAND_SECONDS: i64 = 120;
/// Why a Sent or Accepted command of a Prusa Connect printer fails when the printer was gone
/// (ours, K18 (b) for that link).
pub const PRINTER_GONE: &str = "the printer was gone";
/// Why a door controller's waiting `OPEN_LOCK` left the queue (part 7, ours): its link went.
pub const LINK_GONE: &str = "the board's link went: press Open again";

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
    /// A temporary copy on its way to a printer's storage (D-J #225): not one of the space's Files.
    pub incoming: bool,
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

/// One page of the jobs list ([`Store::job_list`], #71): each filter `None` is no filter.
#[derive(Debug, Clone, Default)]
pub struct JobQuery {
    pub machine: Option<String>,
    /// Connect's `phase`: `Some(true)` only ended, `Some(false)` only running.
    pub ended: Option<bool>,
    /// On the end time, Unix seconds.
    pub from: Option<i64>,
    pub to: Option<i64>,
    /// `by_name`, as the job keeps who started it.
    pub person: Option<String>,
    pub file: Option<String>,
    /// The states as kept (`finished`, `stopped`, ...).
    pub states: Option<Vec<String>>,
    /// The machines' kinds as kept (JSON: `"fdm"`).
    pub kinds: Option<Vec<String>>,
    /// The page after this one's last (started, id).
    pub after: Option<(i64, i64)>,
    pub limit: usize,
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
            JobState::Loaded
                | JobState::Queued
                | JobState::Printing
                | JobState::Paused
                | JobState::Stopping
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

/// The store: a cloneable handle on the database (docs/archive/plans/hub-core-v2.md §2.7), the change bus,
/// and what is kept in memory on purpose (each behind its own small lock).
///
/// The folders and what is kept in memory are in [`Legacy`], reached with [`Store::lock`].
#[derive(Clone)]
pub struct Store {
    inner: std::sync::Arc<Inner>,
}

struct Inner {
    pg: pg::Pg,
    /// Where uploaded files are kept (the same folder as [`Legacy::files_dir`]; transitional).
    files_dir: std::path::PathBuf,
    legacy: std::sync::Mutex<Legacy>,
    changes: tokio::sync::broadcast::Sender<()>,
    /// A vendor, filament or spool added, changed or removed: for Spoolman's WebSockets.
    spoolman: tokio::sync::broadcast::Sender<SpoolmanEvent>,
    /// Spoolman's tag readers and scans of this hub (in memory, as Spoolman keeps them).
    spoolman_scans: std::sync::Arc<crate::spoolman::Scans>,
    /// While nobody has an account: the code (printed at start) that makes the first one.
    setup_code: std::sync::Mutex<Option<String>>,
    /// Failed sign-ins per key (`ip:…`, `name:…`), Unix seconds, for slowing guessing down.
    failures: std::sync::Mutex<std::collections::HashMap<String, Vec<i64>>>,
}

impl Store {
    /// The store over `legacy` (transitional) and Postgres; a setup code while nobody has an account.
    pub async fn new(legacy: Legacy, pg: pg::Pg) -> Result<Self, sqlx::Error> {
        let changes = legacy.changes.clone();
        let store = Self {
            inner: std::sync::Arc::new(Inner {
                pg,
                files_dir: legacy.files_dir.clone(),
                legacy: std::sync::Mutex::new(legacy),
                changes,
                spoolman: tokio::sync::broadcast::channel(256).0,
                spoolman_scans: Default::default(),
                setup_code: Default::default(),
                failures: Default::default(),
            }),
        };
        if !store.has_users().await? {
            *store.inner.setup_code.lock().unwrap() = Some(crate::accounts::setup_code());
        }
        Ok(store)
    }

    /// The folders beside `path` and the database at `database_url`, migrated.
    pub async fn open(
        path: &str,
        database_url: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let legacy = Legacy::open(path)?;
        let pg = pg::Pg::connect(database_url).await?;
        Ok(Self::new(legacy, pg).await?)
    }

    /// For `#[sqlx::test]`: the test's database, and folders of its own.
    #[cfg(test)]
    pub(crate) async fn for_test(pool: &sqlx::PgPool) -> Self {
        Self::with_legacy(pool, Legacy::open(":memory:").unwrap()).await
    }

    /// For `#[sqlx::test]`: a new store on the test's database emptied again, as a new hub has it
    /// (the seed of `accounts/migrations/20261118000100_accounts.sql`): for a test that needs
    /// several fresh hubs in one run.
    #[cfg(test)]
    pub(crate) async fn fresh_for_test(pool: &sqlx::PgPool) -> Self {
        sqlx::raw_sql(
            "TRUNCATE meta, organisations, spaces, users, identities, memberships, org_memberships,
                 sessions, invites, api_tokens, audit_log, machine_grants, machine_groups,
                 machine_group_members, group_locks, time_windows, closed_days, access_requests, group_grants RESTART IDENTITY CASCADE;
             INSERT INTO organisations (uid, name, created_at)
                 VALUES (gen_random_uuid()::text, 'My space', now());
             INSERT INTO spaces (name, uid, organisation_id)
                 SELECT 'My space', gen_random_uuid()::text, id FROM organisations;",
        )
        .execute(pool)
        .await
        .unwrap();
        Self::for_test(pool).await
    }

    /// For `#[sqlx::test]`: the test's database with folders the test made.
    #[cfg(test)]
    pub(crate) async fn with_legacy(pool: &sqlx::PgPool, legacy: Legacy) -> Self {
        Self::new(legacy, pg::Pg::from_pool(pool)).await.unwrap()
    }

    /// The folders and what is kept in memory.
    pub fn lock(&self) -> std::sync::LockResult<std::sync::MutexGuard<'_, Legacy>> {
        self.inner.legacy.lock()
    }

    pub(crate) fn pg(&self) -> &pg::Pg {
        &self.inner.pg
    }

    /// The database closed for this process: whatever of it still runs (a task, an agent's open
    /// connection) gets an error from now on instead of a connection. For a hub handing over to a new
    /// build (main), which must not write meanwhile.
    pub async fn close(&self) {
        self.inner.pg.close().await;
    }

    /// Signals each change to a machine from now on (some may be merged into one).
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.inner.changes.subscribe()
    }

    pub(crate) fn changed(&self) {
        let _ = self.inner.changes.send(());
    }

    /// Spoolman's changes from now on (crate::spoolman's WebSockets).
    pub fn subscribe_spoolman(&self) -> tokio::sync::broadcast::Receiver<SpoolmanEvent> {
        self.inner.spoolman.subscribe()
    }

    /// Spoolman's tag readers and scans (in memory).
    pub(crate) fn spoolman_scans(&self) -> std::sync::Arc<crate::spoolman::Scans> {
        self.inner.spoolman_scans.clone()
    }

    /// A transaction in `space` that tells what changed once it is committed ([`Work`]).
    pub async fn work(&self, space: i64) -> sqlx::Result<Work> {
        Ok(Work {
            tx: self.pg().space(space).await?,
            space,
            events: Vec::new(),
            changed: false,
        })
    }

    /// While nobody has an account: the code that makes the first one.
    pub fn setup_code(&self) -> Option<String> {
        self.inner.setup_code.lock().unwrap().clone()
    }

    /// The setup code, taken: whoever takes it makes the first account.
    pub(crate) fn take_setup_code(&self) -> Option<String> {
        self.inner.setup_code.lock().unwrap().take()
    }

    pub(crate) fn set_setup_code(&self, code: Option<String>) {
        *self.inner.setup_code.lock().unwrap() = code;
    }

    /// Failed sign-ins per key, Unix seconds (crate::auth).
    pub(crate) fn failures(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, Vec<i64>>> {
        self.inner.failures.lock().unwrap()
    }
}

/// What the hub keeps beside the database: its folders, and what is kept in memory on purpose.
pub struct Legacy {
    /// Where uploaded files are kept: `<id>.gcode`, and `<id>.png` for a thumbnail.
    pub files_dir: std::path::PathBuf,
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
    /// What the hub keeps per machine on Prusa Connect's protocol, by machine id, for a known
    /// printer only (crate::devices::connect_tools).
    pub(crate) connect_tools:
        std::collections::HashMap<String, crate::devices::connect_tools::Session>,
    /// The printers with a `/p/ws` socket attached right now, by machine id, and how many: a
    /// printer on Prusa Connect is online while one is and it is heard ([`Legacy::linked`]).
    pub(crate) attached: std::collections::HashMap<String, usize>,
    /// When each attached printer last sent a frame on its socket (any frame: report, event,
    /// segment request, ping), by machine id ([`Legacy::linked`]).
    pub(crate) last_frame: std::collections::HashMap<String, i64>,
    /// Each printer's newest `/p/ws` socket, by machine id: a newer one ends the older (owner,
    /// 2026-11-24: "ja, nieuwe verbinding sluit de oude"; Buddy keeps one socket,
    /// `connect.cpp:560-570` drops its old one before it connects again).
    pub(crate) newest_link: std::collections::HashMap<String, u64>,
    /// A label printer's START_CONNECT_DOWNLOAD in flight, by command id, until the label printed
    /// or failed (docs/design/wire.md, "When the hub calls a label printed").
    pub(crate) label_jobs: std::collections::HashMap<u32, LabelJob>,
    /// The print each Prusa on Connect was last asked about (SEND_JOB_INFO), by token: once.
    pub(crate) job_info_asked: std::collections::HashMap<String, u64>,
    /// Prints a machine runs itself (from its USB stick or SD card, or started at its
    /// screen), by machine id, while they run: recorded as a job when they end (see
    /// `follow_own_print`).
    pub(crate) own_prints: std::collections::HashMap<String, OwnPrint>,
    /// HS7b: per laser, the file id it last framed completely since its last Set origin.
    pub(crate) laser_framed: std::collections::HashMap<String, String>,
    /// HS7b: lasers on which a Set origin was done since the hub started.
    pub(crate) laser_origin_set: std::collections::HashSet<String>,
    /// OctoPrint-BedLevelVisualizer's state per Marlin printer, by machine id, while the hub runs
    /// (crate::devices::bed_level_visualizer, #107).
    pub(crate) bed_level_visualizers:
        std::collections::HashMap<String, crate::devices::bed_level_visualizer::Blv>,
    next_command: u32,
    /// Per machine id, when its history last got a sample (crate::store::history).
    last_sample: std::collections::HashMap<String, i64>,
    /// A signal whenever a machine changes, for the pages that show them live.
    changes: tokio::sync::broadcast::Sender<()>,
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

/// A space's transaction on Postgres ([`Store::work`]), with what it changed: the live pages and
/// Spoolman's WebSockets hear of it once it is committed, so what they read then is there.
pub struct Work {
    pub(crate) tx: sqlx::Transaction<'static, sqlx::Postgres>,
    pub(crate) space: i64,
    events: Vec<SpoolmanEvent>,
    changed: bool,
}

impl Work {
    /// The transaction's connection, for a query.
    pub(crate) fn c(&mut self) -> &mut sqlx::PgConnection {
        &mut self.tx
    }

    /// `f` in a savepoint: when it fails, what it did is undone and the transaction goes on
    /// (`None`), as a failed SQLite statement left the others be.
    pub(crate) async fn or_none<T>(
        &mut self,
        f: impl AsyncFnOnce(&mut Self) -> sqlx::Result<T>,
    ) -> sqlx::Result<Option<T>> {
        sqlx::query("SAVEPOINT or_none").execute(self.c()).await?;
        match f(self).await {
            Ok(value) => {
                sqlx::query("RELEASE SAVEPOINT or_none")
                    .execute(self.c())
                    .await?;
                Ok(Some(value))
            }
            Err(_) => {
                sqlx::query("ROLLBACK TO SAVEPOINT or_none")
                    .execute(self.c())
                    .await?;
                Ok(None)
            }
        }
    }

    /// Something a live page shows changed.
    pub(crate) fn changed(&mut self) {
        self.changed = true;
    }

    /// Spoolman's data changed: `spool` 7 `updated`, in this space.
    pub(crate) fn spoolman_event(&mut self, resource: &'static str, kind: &'static str, id: i64) {
        let space = matches!(resource, "spool" | "filament" | "vendor").then_some(self.space);
        self.events.push(SpoolmanEvent {
            space,
            resource,
            kind,
            id,
            was: None,
        });
    }

    /// One of Spoolman's things removed, with what it was.
    pub(crate) fn spoolman_removed(
        &mut self,
        resource: &'static str,
        id: i64,
        was: serde_json::Value,
    ) {
        self.events.push(SpoolmanEvent {
            space: Some(self.space),
            resource,
            kind: "deleted",
            id,
            was: Some(was),
        });
    }

    /// Commits, then tells.
    pub(crate) async fn commit(self, store: &Store) -> sqlx::Result<()> {
        self.tx.commit().await?;
        for event in self.events {
            let _ = store.inner.spoolman.send(event);
        }
        if self.changed {
            store.changed();
        }
        Ok(())
    }
}

impl Legacy {
    /// A printer linked over `/p/ws` that is still heard ([`SILENT_SECONDS`]).
    pub(crate) fn linked(&self, machine: &str) -> bool {
        self.attached.contains_key(machine)
            && self
                .last_frame
                .get(machine)
                .is_some_and(|t| now() - t <= SILENT_SECONDS)
    }
}

impl Legacy {
    /// The files folder beside `path` (`maker-connect.db` → `maker-connect-files/`).
    pub fn files_dir_of(path: &str) -> std::path::PathBuf {
        let db = std::path::Path::new(path);
        let stem = db
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("maker-connect");
        db.with_file_name(format!("{stem}-files"))
    }

    /// The folders beside `path` (`maker-connect.db` keeps its files in `maker-connect-files/`),
    /// made when missing; `:memory:` (tests) a folder of its own.
    pub fn open(path: &str) -> std::io::Result<Self> {
        let files_dir = if path == ":memory:" {
            std::env::temp_dir().join(format!("maker-connect-test-{}", random_id()))
        } else {
            Self::files_dir_of(path)
        };
        std::fs::create_dir_all(&files_dir)?;
        let store = Self {
            files_dir,
            commands: Default::default(),
            ppe_ok: Default::default(),
            asking: Default::default(),
            dialogs: Default::default(),
            close_preview: Default::default(),
            printing_file: Default::default(),
            connect_tools: Default::default(),
            attached: Default::default(),
            last_frame: Default::default(),
            newest_link: Default::default(),
            label_jobs: Default::default(),
            job_info_asked: Default::default(),
            own_prints: Default::default(),
            laser_framed: Default::default(),
            laser_origin_set: Default::default(),
            bed_level_visualizers: Default::default(),
            // Ids that do not repeat soon after a restart, as a printer may remember the last.
            next_command: (now() % 1_000_000_000) as u32,
            last_sample: Default::default(),
            changes: tokio::sync::broadcast::channel(16).0,
        };
        Ok(store)
    }

    /// Signals each change to a machine from now on (some may be merged into one).
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.changes.subscribe()
    }

    pub(crate) fn changed(&self) {
        // Nobody listening is fine.
        let _ = self.changes.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What fails in a savepoint is `None` and the transaction goes on: so the projects list
    /// leaves one machine's count of compatible parts empty when it fails, as it did on SQLite.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_failure_in_a_savepoint_leaves_the_rest_going(pool: sqlx::PgPool) {
        let store = Store::for_test(&pool).await;
        let (space, _) = store.default_space().await.unwrap();
        let got = crate::stock::in_space!(&store, space, |w| async {
            let failed = w
                .or_none(async |w| {
                    sqlx::query_scalar::<_, i32>("SELECT 1 / 0")
                        .fetch_one(w.c())
                        .await
                })
                .await?;
            let kept = w
                .or_none(async |w| {
                    sqlx::query_scalar::<_, i32>("SELECT 2")
                        .fetch_one(w.c())
                        .await
                })
                .await?;
            let after: i32 = sqlx::query_scalar("SELECT 3").fetch_one(w.c()).await?;
            Ok::<_, sqlx::Error>((failed, kept, after))
        })
        .await
        .unwrap();
        assert_eq!(got, (None, Some(2), 3));
    }

    async fn device(store: &Store, space: i64, name: &str) -> String {
        let profile = crate::profiles::find("prusa/mini").unwrap();
        let id = store.add_virtual(space, profile).await.unwrap();
        store.rename(space, &id, name).await.unwrap();
        id
    }

    /// A finished print stays on the bed until taken; a machine out of use says why.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_print_on_the_bed_and_a_machine_out_of_use(pool: sqlx::PgPool) {
        let pg = Store::for_test(&pool).await;
        let (space, _) = pg.default_space().await.unwrap();
        let printer = device(&pg, space, "MINI").await;
        pg.add_file(
            space,
            "f1",
            "cube.gcode",
            10,
            &crate::gcode::FileMeta::default(),
        )
        .await
        .unwrap();
        let file = pg.file(space, "f1").await.unwrap().unwrap();
        let job = pg
            .add_job(space, &printer, &file, None, StartedBy::Auto { by: None })
            .await
            .unwrap()
            .unwrap();
        pg.set_job(space, job, JobState::Printing, None)
            .await
            .unwrap();
        assert_eq!(
            pg.machine(space, &printer).await.unwrap().unwrap().on_bed,
            None
        );
        pg.set_job(space, job, JobState::Finished, None)
            .await
            .unwrap();
        let on_bed = pg.machine(space, &printer).await.unwrap().unwrap().on_bed;
        assert_eq!(on_bed.map(|b| b.file_name), Some("cube.gcode".to_owned()));
        assert!(pg.print_taken(space, &printer).await.unwrap());
        assert_eq!(
            pg.machine(space, &printer).await.unwrap().unwrap().on_bed,
            None
        );
        assert!(!pg.print_taken(space, &printer).await.unwrap());

        assert!(
            pg.set_out_of_use(space, &printer, Some("nozzle clogged"))
                .await
                .unwrap()
        );
        let out = pg
            .machine(space, &printer)
            .await
            .unwrap()
            .unwrap()
            .out_of_use
            .unwrap();
        assert_eq!(out.reason, "nozzle clogged");
        assert!(out.since > 0);
        assert!(pg.set_out_of_use(space, &printer, None).await.unwrap());
        assert_eq!(
            pg.machine(space, &printer)
                .await
                .unwrap()
                .unwrap()
                .out_of_use,
            None
        );
    }

    /// A finished print counts its filament off the loaded spool; a stopped one its part.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_job_counts_its_filament_off_the_loaded_spool(pool: sqlx::PgPool) {
        let pg = Store::for_test(&pool).await;
        let (space, _) = pg.default_space().await.unwrap();
        let printer = device(&pg, space, "MINI").await;
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
        let material = pg.add_material(space, pla).await.unwrap();
        let target = pg.machine(space, &printer).await.unwrap();
        let mut w = pg.work(space).await.unwrap();
        let spool = w
            .receive(&material, 1, None)
            .await
            .unwrap()
            .unwrap()
            .remove(0);
        let media = w.load_item(&spool, target.as_ref()).await.unwrap().unwrap();
        w.commit(&pg).await.unwrap();
        if let Some(media) = media {
            pg.set_media(space, &printer, Some(&media)).await.unwrap();
        }
        let meta = crate::gcode::FileMeta {
            filament_g: Some(12.5),
            ..crate::gcode::FileMeta::default()
        };
        pg.add_file(space, "f1", "cube.gcode", 10, &meta)
            .await
            .unwrap();
        let file = pg.file(space, "f1").await.unwrap().unwrap();
        let job = pg
            .add_job(space, &printer, &file, None, StartedBy::Auto { by: None })
            .await
            .unwrap()
            .unwrap();
        pg.set_job(space, job, JobState::Printing, None)
            .await
            .unwrap();
        pg.set_job(space, job, JobState::Finished, None)
            .await
            .unwrap();
        let left = pg.items(space).await.unwrap()[0].amount;
        assert_eq!(left, 987.5);
        let last = &pg.movements(space, &spool).await.unwrap()[0];
        assert_eq!(
            (last.kind.as_str(), last.note.as_deref()),
            ("used", Some("job cube.gcode"))
        );
    }

    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn devices_hang_behind_a_plug_without_loops(pool: sqlx::PgPool) {
        let pg = Store::for_test(&pool).await;
        let (space, _) = pg.default_space().await.unwrap();
        let wall = device(&pg, space, "Wall plug").await;
        let strip = device(&pg, space, "Power strip").await;
        let dryer = device(&pg, space, "Dryer").await;
        let lamp = device(&pg, space, "Lamp").await;
        let link = |device: &str, channel| PowerLink {
            device: device.to_owned(),
            channel,
        };
        // Two devices on one channel of the strip, the strip on the wall plug.
        assert_eq!(
            pg.set_powered_by(space, &dryer, Some(&link(&strip, 1)))
                .await
                .unwrap(),
            Ok(())
        );
        assert_eq!(
            pg.set_powered_by(space, &lamp, Some(&link(&strip, 1)))
                .await
                .unwrap(),
            Ok(())
        );
        assert_eq!(
            pg.set_powered_by(space, &strip, Some(&link(&wall, 0)))
                .await
                .unwrap(),
            Ok(())
        );
        let dryer_now = pg.machine(space, &dryer).await.unwrap().unwrap();
        assert_eq!(dryer_now.powered_by, Some(link(&strip, 1)));
        // The wall plug behind the dryer would be a loop; so would a device behind itself.
        assert!(
            pg.set_powered_by(space, &wall, Some(&link(&dryer, 0)))
                .await
                .unwrap()
                .is_err()
        );
        assert!(
            pg.set_powered_by(space, &lamp, Some(&link(&lamp, 0)))
                .await
                .unwrap()
                .is_err()
        );
        assert!(
            pg.set_powered_by(space, &lamp, Some(&link("nothing", 0)))
                .await
                .unwrap()
                .is_err()
        );
        // Off the plug again.
        assert_eq!(
            pg.set_powered_by(space, &dryer, None).await.unwrap(),
            Ok(())
        );
        assert_eq!(
            pg.machine(space, &dryer).await.unwrap().unwrap().powered_by,
            None
        );
    }
}
