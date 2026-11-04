//! What the cloud keeps: spaces and their machines, in one SQLite file. Everything belongs
//! to a space from the start, so the same software can serve one makerspace on a Pi or
//! many on a hosted service.

use maker_connect_protocol::{MachineKind, Register, Telemetry};
use rand::Rng;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Whether machines on this link take commands from the website (yet).
pub fn takes_commands(link: &str) -> bool {
    matches!(link, "prusa-connect" | "virtual" | "agent")
}

/// A bed mesh as the cloud keeps it: the machine's last, and when it came.
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

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A machine as the website shows it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Machine {
    pub id: String,
    pub name: String,
    pub kind: MachineKind,
    pub agent_version: String,
    /// How it reaches the cloud: `agent` (Maker Connect's agent next to it), or
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

/// One point of a machine's temperature history.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Sample {
    /// Unix seconds.
    pub t: i64,
    pub nozzle: Option<f32>,
    pub nozzle_target: Option<f32>,
    pub bed: Option<f32>,
    pub bed_target: Option<f32>,
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

pub struct Store {
    db: Connection,
    /// Commands per machine id, oldest first.
    commands: std::collections::HashMap<String, Vec<Command>>,
    next_command: u32,
    /// Temperature history per machine id, oldest first.
    history: std::collections::HashMap<String, std::collections::VecDeque<Sample>>,
    /// A signal whenever a machine changes, for the pages that show them live.
    changes: tokio::sync::broadcast::Sender<()>,
}

impl Store {
    /// Opens (or creates) the store; a new one gets one space.
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS spaces (
                 id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL
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
        // Columns added after the first version: a store from then gets them.
        for (column, definition) in [
            ("profile", "TEXT"),
            ("profile_chosen", "INTEGER NOT NULL DEFAULT 0"),
            ("link", "TEXT NOT NULL DEFAULT 'agent'"),
            ("details", "TEXT"),
            ("mesh", "TEXT"),
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
        let spaces: i64 = db.query_row("SELECT COUNT(*) FROM spaces", [], |r| r.get(0))?;
        if spaces == 0 {
            db.execute("INSERT INTO spaces (name) VALUES ('My space')", [])?;
        }
        Ok(Self {
            db,
            commands: Default::default(),
            // Ids that do not repeat soon after a restart, as a printer may remember the last.
            next_command: (now() % 1_000_000_000) as u32,
            history: Default::default(),
            changes: tokio::sync::broadcast::channel(16).0,
        })
    }

    /// Signals each change to a machine from now on (some may be merged into one).
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.changes.subscribe()
    }

    fn changed(&self) {
        // Nobody listening is fine.
        let _ = self.changes.send(());
    }

    /// The space a self-hosted cloud shows: its first.
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

    /// A new machine that reaches the cloud by `link` (see [`Machine::link`]).
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
            "INSERT INTO machines (id, name, kind, agent_version, token, pairing_code, registered_at, link)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
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
            ..telemetry.clone()
        };
        let updated = self.db.execute(
            "UPDATE machines SET last_seen = ?1, telemetry = ?2 WHERE token = ?3",
            params![
                now(),
                serde_json::to_string(&kept).unwrap_or_default(),
                token
            ],
        )?;
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
            output: Vec::new(),
            state: CommandState::Queued,
            reason: None,
            at: now(),
        });
        let excess = commands.len().saturating_sub(COMMANDS_KEPT);
        commands.drain(..excess);
        self.changed();
        Ok(Some(id))
    }

    /// The next command for the machine asking with this token, once the one before is
    /// done: one at a time, as a printer takes them.
    pub fn next_command(&mut self, token: &str) -> rusqlite::Result<Option<(u32, String)>> {
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
        let next = commands
            .iter_mut()
            .find(|c| c.state == CommandState::Queued)
            .map(|c| {
                c.state = CommandState::Sent;
                c.at = now;
                (c.id, c.gcode.clone())
            });
        if next.is_some() {
            self.changed();
        }
        Ok(next)
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
    pub fn pair(&self, space: i64, code: &str) -> rusqlite::Result<Option<String>> {
        let code = code.trim().to_ascii_uppercase();
        let name: Option<String> = self
            .db
            .query_row(
                "SELECT name FROM machines WHERE pairing_code = ?1 AND space_id IS NULL",
                [&code],
                |r| r.get(0),
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
                agent_version: "simulated in the cloud".to_owned(),
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

    /// Every virtual machine: its id, token and profile.
    pub fn virtual_machines(&self) -> rusqlite::Result<Vec<(String, String, Option<String>)>> {
        let mut query = self
            .db
            .prepare("SELECT id, token, profile FROM machines WHERE link = 'virtual'")?;
        let rows = query.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    /// The owner removes a machine from the space: gone, with its token.
    pub fn remove(&mut self, space: i64, machine: &str) -> rusqlite::Result<bool> {
        let removed = self.db.execute(
            "DELETE FROM machines WHERE id = ?1 AND space_id = ?2",
            params![machine, space],
        )?;
        self.commands.remove(machine);
        self.history.remove(machine);
        self.changed();
        Ok(removed > 0)
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

    /// One machine of a space.
    pub fn machine(&self, space: i64, id: &str) -> rusqlite::Result<Option<Machine>> {
        Ok(self.machines(space)?.into_iter().find(|m| m.id == id))
    }

    /// A space's machines, by name.
    pub fn machines(&self, space: i64) -> rusqlite::Result<Vec<Machine>> {
        let mut query = self.db.prepare(
            "SELECT id, name, kind, agent_version, last_seen, telemetry, profile, profile_chosen, link, details, mesh
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
                mesh: r
                    .get::<_, Option<String>>(10)?
                    .and_then(|m| serde_json::from_str(&m).ok()),
            })
        })?;
        let mut machines: Vec<Machine> = rows.collect::<rusqlite::Result<_>>()?;
        for machine in &mut machines {
            if let Some(commands) = self.commands.get(&machine.id) {
                machine.commands = commands.iter().rev().cloned().collect();
            }
        }
        Ok(machines)
    }
}
