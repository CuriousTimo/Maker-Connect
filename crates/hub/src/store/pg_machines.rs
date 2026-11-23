//! Machines on Postgres (`migrations/20261118000300_machines.sql`): the queries of `machines.rs` and
//! `agents.rs` that touch only the `machines` table, as async [`Store`] methods (hub core v2 step 2,
//! lane 3). Space-keyed ones run in their space's transaction ([`pg::Pg::space`]), pairing too
//! (`claim_registration`); those keyed by a token or a registration code (an agent, a printer, maybe
//! in no space yet) use the admin pool (`Bypass::Secret`), and moving a machine to another space
//! uses `Bypass::InstanceAdmin`.

use super::*;
use crate::store::pg::Bypass;

/// The columns of a machine, as [`MachineRow`] reads them.
macro_rules! machine_select {
    ($tail:literal) => {
        concat!(
            "SELECT id, space_id, name, kind, agent_version, token, unix_seconds(last_seen) AS last_seen, telemetry,
                 profile, profile_chosen, link, details, mesh, media, powered_by,
                 power_channel::bigint AS power_channel, reached_through, agent, peer, out_of_use,
                 unix_seconds(out_of_use_at) AS out_of_use_at, laser_w, bed_mm, labels, induction_required, ppe,
                 usb_print_by, farm, sheet
             FROM machines ",
            $tail
        )
    };
}

/// A machine's row as the website reads it ([`machine_select!`]).
#[derive(sqlx::FromRow)]
pub(crate) struct MachineRow {
    pub id: String,
    pub space_id: Option<i64>,
    pub name: String,
    pub kind: String,
    pub agent_version: String,
    pub token: String,
    pub last_seen: Option<i64>,
    pub telemetry: Option<String>,
    pub profile: Option<String>,
    pub profile_chosen: bool,
    pub link: String,
    pub details: Option<String>,
    pub mesh: Option<String>,
    pub media: Option<String>,
    pub powered_by: Option<String>,
    pub power_channel: Option<i64>,
    pub reached_through: Option<String>,
    pub agent: Option<String>,
    pub peer: Option<String>,
    pub out_of_use: Option<String>,
    pub out_of_use_at: Option<i64>,
    pub laser_w: Option<f64>,
    pub bed_mm: Option<String>,
    pub labels: Option<String>,
    /// 0: open to every member; 1: needs permission; 2: needs training (crate::access).
    pub induction_required: i32,
    pub ppe: Option<String>,
    pub usb_print_by: Option<String>,
    pub farm: bool,
    pub sheet: Option<String>,
}

/// An agent as the hub knows it from its devices (`Store::agents`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentOfHub {
    /// Its `agent_id`.
    pub id: String,
    /// Its release, as its last-seen device's `INFO` said (`firmware`).
    pub release: Option<String>,
    /// Its computer's name (`network_info.hostname`).
    pub computer: Option<String>,
    /// Whether any of its devices is connected.
    pub online: bool,
    pub seen_seconds_ago: Option<i64>,
    pub devices: Vec<AgentDevice>,
}

/// A device of an agent, in whichever space it is.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentDevice {
    pub id: String,
    pub name: String,
    /// The space's name; none while it waits to be added.
    pub space: Option<String>,
    pub online: bool,
    pub seen_seconds_ago: Option<i64>,
    pub extensions: Vec<String>,
}

impl MachineRow {
    /// The machine as far as its own row tells; [`Legacy::complete_machines`] adds the rest.
    /// `attached`: a `/p/ws` socket of it is open ([`Legacy::attached`]).
    pub(crate) fn into_machine(self, now: i64, attached: bool) -> Machine {
        let seen = self.last_seen.map(|t| now - t);
        let online = attached || seen.is_some_and(|s| s <= ONLINE_SECONDS);
        let kind = self.kind;
        Machine {
            laser_framed: false,
            laser_origin_set: false,
            id: self.id,
            name: self.name,
            kind: serde_json::from_str(&kind).unwrap_or(MachineKind::Other),
            agent_version: self.agent_version,
            seen_seconds_ago: seen,
            online,
            telemetry: self
                .telemetry
                .filter(|_| online)
                .and_then(|t| serde_json::from_str(&t).ok())
                .map(|mut t| {
                    if self.link == "prusa-connect" {
                        crate::devices::prusa_connect::current_job(&mut t);
                    }
                    t
                }),
            profile: self.profile,
            profile_chosen: self.profile_chosen,
            details: self.details.and_then(|d| serde_json::from_str(&d).ok()),
            commands: Vec::new(),
            job: None,
            resumable: None,
            mesh: self.mesh.and_then(|m| serde_json::from_str(&m).ok()),
            media: self.media,
            powered_by: self.powered_by.map(|device| PowerLink {
                device,
                channel: self.power_channel.unwrap_or(0) as u32,
            }),
            reached_through: self.reached_through,
            agent: self.agent.and_then(|a| serde_json::from_str(&a).ok()),
            peer: self.peer,
            out_of_use: self.out_of_use.map(|reason| OutOfUse {
                reason,
                since: self.out_of_use_at.unwrap_or(0),
            }),
            loaded: None,
            on_bed: None,
            ready: None,
            queue: Vec::new(),
            queue_held: None,
            asking: None,
            buttons: Vec::new(),
            usb_print_by: self.usb_print_by,
            moonraker_token: (self.link == "simplyprint").then_some(self.token),
            link: self.link,
            laser_w: self.laser_w,
            bed_mm: self.bed_mm.and_then(|b| serde_json::from_str(&b).ok()),
            labels: self
                .labels
                .and_then(|l| serde_json::from_str(&l).ok())
                .unwrap_or_default(),
            induction_required: self.induction_required != 0,
            training: self.induction_required == 2,
            ppe: crate::safety::ppe_of(&kind, self.ppe.as_deref()),
            confirm: None,
            farm: self.farm,
            takes_work: self.farm,
            upkeep: None,
            sheet: self.sheet,
        }
    }
}

impl Store {
    /// Whether a token belongs to a machine.
    pub async fn knows(&self, token: &str) -> sqlx::Result<bool> {
        Ok(sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "n!" FROM machines WHERE token = $1"#,
            token
        )
        .fetch_one(self.pg().admin(Bypass::Secret))
        .await?
            > 0)
    }

    /// A machine's last report, by its token.
    pub async fn last_telemetry(&self, token: &str) -> sqlx::Result<Option<Telemetry>> {
        let text = sqlx::query_scalar!("SELECT telemetry FROM machines WHERE token = $1", token)
            .fetch_optional(self.pg().admin(Bypass::Secret))
            .await?
            .flatten();
        Ok(text.and_then(|t| serde_json::from_str(&t).ok()))
    }

    /// What a machine tells about itself. Returns whether the token is known.
    pub async fn set_details(&self, token: &str, details: &Details) -> sqlx::Result<bool> {
        let n = sqlx::query!(
            "UPDATE machines SET details = $1 WHERE token = $2",
            serde_json::to_string(details).unwrap_or_default(),
            token
        )
        .execute(self.pg().admin(Bypass::Secret))
        .await?
        .rows_affected();
        self.changed();
        Ok(n > 0)
    }

    /// Every agent and its devices, over all spaces (the Instance page, instance admins only):
    /// devices grouped by the `agent_id` their `INFO` carried (`docs/design/prusa-sdk-extension.md`
    /// part 6a), the agent seen most recently first. A device without one is not listed.
    pub async fn agents(&self) -> sqlx::Result<Vec<AgentOfHub>> {
        // id, name, space, last seen, details.
        type Row = (String, String, Option<String>, Option<i64>, Option<String>);
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT m.id, m.name, s.name, unix_seconds(m.last_seen), m.details
                 FROM machines m LEFT JOIN spaces s ON s.id = m.space_id
                 WHERE m.details IS NOT NULL
                 ORDER BY m.name COLLATE \"C\"",
        )
        .fetch_all(self.pg().admin(Bypass::InstanceAdmin))
        .await?;
        let now = now();
        let attached = self.lock().unwrap().attached.clone();
        let mut agents: Vec<AgentOfHub> = Vec::new();
        for (id, name, space, last_seen, details) in rows {
            let Some(details) = details.and_then(|d| serde_json::from_str::<Details>(&d).ok())
            else {
                continue;
            };
            let Some(agent_id) = details.agent_id.clone() else {
                continue;
            };
            let seen = last_seen.map(|t| (now - t).max(0));
            let online = attached.contains_key(&id) || seen.is_some_and(|s| s <= ONLINE_SECONDS);
            let device = AgentDevice {
                id,
                name,
                space,
                online,
                seen_seconds_ago: seen,
                extensions: details.extensions.clone(),
            };
            // Part 6a: the agent's own release (`User-Agent-Version`), not the device's firmware.
            let release = details.agent_release.clone();
            let computer = details.network.as_ref().and_then(|n| n.hostname.clone());
            match agents.iter_mut().find(|a| a.id == agent_id) {
                Some(agent) => {
                    // The device seen last says what the agent is now.
                    if seen.is_some()
                        && agent
                            .seen_seconds_ago
                            .is_none_or(|s| seen.is_some_and(|d| d < s))
                    {
                        agent.seen_seconds_ago = seen;
                        agent.release = release.or(agent.release.take());
                        agent.computer = computer.or(agent.computer.take());
                    }
                    agent.online |= online;
                    agent.devices.push(device);
                }
                None => agents.push(AgentOfHub {
                    id: agent_id,
                    release,
                    computer,
                    online,
                    seen_seconds_ago: seen,
                    devices: vec![device],
                }),
            }
        }
        agents.sort_by_key(|a| a.seen_seconds_ago.unwrap_or(i64::MAX));
        Ok(agents)
    }

    /// Puts the machine waiting with this pairing code in the space: its (id, name); None for an
    /// unknown code, or one another pairing took first. A code is used once. In the space's
    /// transaction, through `claim_registration` (design decisions/20261119-pairing-in-one-space-transaction.md):
    /// one statement that takes the machine only while it has no space.
    pub async fn pair(&self, space: i64, code: &str) -> sqlx::Result<Option<(String, String)>> {
        let code = code.trim().to_ascii_uppercase();
        let mut tx = self.pg().space(space).await?;
        let claimed: Option<(String, String)> =
            sqlx::query_as("SELECT machine_id, machine_name FROM claim_registration($1)")
                .bind(&code)
                .fetch_optional(&mut *tx)
                .await?;
        tx.commit().await?;
        if claimed.is_some() {
            self.changed();
        }
        Ok(claimed)
    }

    /// A new machine that reaches the hub by `link` (see [`Machine::link`]), not yet in a space,
    /// with a token and a pairing code: `name`, `kind`, and how it says it connects (`agent_version`).
    pub async fn register_linked(
        &self,
        name: &str,
        kind: MachineKind,
        agent_version: &str,
        link: &str,
    ) -> sqlx::Result<(String, String, String)> {
        let (id, token, code) = new_identity(link);
        // A new machine needs permission: members get it per machine or per group.
        sqlx::query!(
            "INSERT INTO machines (id, name, kind, agent_version, token, pairing_code, registered_at, link, induction_required)
             VALUES ($1, $2, $3, $4, $5, $6, at_unix($7), $8, 1)",
            id,
            name,
            serde_json::to_string(&kind).unwrap_or_default(),
            agent_version,
            token,
            code,
            now(),
            link
        )
        .execute(self.pg().admin(Bypass::Secret))
        .await?;
        self.changed();
        Ok((id, token, code))
    }

    /// A new machine made by someone of the space (a virtual machine, a Moonraker printer): straight
    /// into it, in its transaction, without a pairing code; its id. As [`Store::register_linked`]
    /// and then paired.
    async fn add_in_space(
        &self,
        space: i64,
        (name, kind, agent_version): (&str, MachineKind, &str),
        link: &str,
        profile: Option<&str>,
    ) -> sqlx::Result<String> {
        let (id, token, _) = new_identity(link);
        let mut tx = self.pg().space(space).await?;
        sqlx::query(
            "INSERT INTO machines (id, space_id, name, kind, agent_version, token, registered_at, link,
                 induction_required, profile, profile_chosen)
             VALUES ($1, $2, $3, $4, $5, $6, at_unix($7), $8, 1, $9, $10)",
        )
        .bind(&id)
        .bind(space)
        .bind(name)
        .bind(serde_json::to_string(&kind).unwrap_or_default())
        .bind(agent_version)
        .bind(&token)
        .bind(now())
        .bind(link)
        .bind(profile)
        .bind(profile.is_some())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.changed();
        Ok(id)
    }
}

/// A new machine's id, token and pairing code, random (a Prusa Connect printer's token as Buddy keeps it).
fn new_identity(link: &str) -> (String, String, String) {
    {
        let mut rng = rand::thread_rng();
        let token: String = if link == "prusa-connect" {
            // Buddy keeps at most 20 characters of a token (CONNECT_TOKEN_LEN, printer.hpp:190); 19 of 62 is 113 bits, which fits.
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
        (id, token, code)
    }
}

impl Store {
    /// A virtual machine of a profile, straight into the space: its id.
    pub async fn add_virtual(
        &self,
        space: i64,
        profile: &crate::profiles::Profile,
    ) -> sqlx::Result<String> {
        self.add_in_space(
            space,
            (
                &format!("Virtual {}", profile.model),
                profile.kind,
                "simulated in the hub",
            ),
            "virtual",
            Some(&profile.id),
        )
        .await
    }

    /// A printer on Klipper, reached through Moonraker (its SimplyPrint component pointed here),
    /// straight into the space: its id.
    pub async fn add_moonraker(&self, space: i64, name: &str) -> sqlx::Result<String> {
        self.add_in_space(
            space,
            (name, MachineKind::Fdm, "Moonraker"),
            "simplyprint",
            None,
        )
        .await
    }

    /// The name of the Moonraker printer with this token, if it is one in a space.
    pub async fn moonraker_name(&self, token: &str) -> sqlx::Result<Option<String>> {
        sqlx::query_scalar!(
            "SELECT name FROM machines WHERE token = $1 AND link = 'simplyprint' AND space_id IS NOT NULL",
            token
        )
        .fetch_optional(self.pg().admin(Bypass::Secret))
        .await
    }

    /// A Moonraker printer's token, for the address its config file names.
    pub async fn moonraker_token(&self, space: i64, machine: &str) -> sqlx::Result<Option<String>> {
        let mut tx = self.pg().space(space).await?;
        let token = sqlx::query_scalar!(
            "SELECT token FROM machines WHERE id = $1 AND space_id = $2 AND link = 'simplyprint'",
            machine,
            space
        )
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(token)
    }

    /// Every virtual machine of every space: its id, token, profile and space (each space's in its
    /// transaction; the spaces as a background task's list, [`Store::space_ids`]).
    pub async fn virtual_machines(
        &self,
    ) -> sqlx::Result<Vec<(String, String, Option<String>, i64)>> {
        let mut all = Vec::new();
        for space in self.space_ids().await? {
            let mut tx = self.pg().space(space).await?;
            let rows =
                sqlx::query!("SELECT id, token, profile FROM machines WHERE link = 'virtual'")
                    .fetch_all(&mut *tx)
                    .await?;
            tx.commit().await?;
            all.extend(rows.into_iter().map(|r| (r.id, r.token, r.profile, space)));
        }
        Ok(all)
    }

    /// Removes a machine of the space (its permissions and group places follow it: foreign keys).
    /// Returns whether it was there. The caller drops what the hub keeps in memory
    /// ([`Store::remove_machine`] does both).
    pub async fn remove_machine_row(&self, space: i64, machine: &str) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        let n = sqlx::query!(
            "DELETE FROM machines WHERE id = $1 AND space_id = $2",
            machine,
            space
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// The owner says what is loaded (a label printer's tape), or None. Returns whether the machine
    /// is in the space.
    pub async fn set_media(
        &self,
        space: i64,
        machine: &str,
        media: Option<&str>,
    ) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        let n = sqlx::query!(
            "UPDATE machines SET media = $1 WHERE id = $2 AND space_id = $3",
            media,
            machine,
            space
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        self.changed();
        Ok(n > 0)
    }

    async fn profile_and_media(
        &self,
        space: i64,
        machine: &str,
    ) -> sqlx::Result<Option<(Option<String>, Option<String>)>> {
        let mut tx = self.pg().space(space).await?;
        let found = sqlx::query!("SELECT profile, media FROM machines WHERE id = $1", machine)
            .fetch_optional(&mut *tx)
            .await?
            .map(|r| (r.profile, r.media));
        tx.commit().await?;
        Ok(found)
    }

    /// The tape or labels loaded in a label printer, as the owner said: (width across, a die-cut
    /// label's length) in mm.
    pub async fn loaded_media_mm(
        &self,
        space: i64,
        machine: &str,
    ) -> sqlx::Result<Option<(u32, Option<u32>)>> {
        Ok(self
            .profile_and_media(space, machine)
            .await?
            .and_then(|(profile, media)| {
                let profile = crate::profiles::find(&profile?)?;
                let media = crate::profiles::media(profile, &media?)?;
                Some((media.width_mm, media.length_mm))
            }))
    }

    /// HS2b (W2b): a LabelWriter's loaded roll as the PPD's `PageSize` choice (its medium id when the
    /// profile takes its media from the port), for the job to name its page.
    pub async fn loaded_page_size(
        &self,
        space: i64,
        machine: &str,
    ) -> sqlx::Result<Option<String>> {
        Ok(self
            .profile_and_media(space, machine)
            .await?
            .and_then(|(profile, media)| {
                let profile = crate::profiles::find(&profile?)?;
                let media = media?;
                crate::profiles::labelwriter_page(Some(profile), &media).map(|g| g.name.to_owned())
            }))
    }

    /// Hangs a device behind a channel of another of the space (None: on no plug). Refused when the
    /// other is not in the space, or is behind this one already (a loop).
    pub async fn set_powered_by(
        &self,
        space: i64,
        machine: &str,
        power: Option<&PowerLink>,
    ) -> sqlx::Result<Result<(), &'static str>> {
        let mut tx = self.pg().space(space).await?;
        // The device's own parent: None when it is not in the space.
        async fn parent(
            tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
            space: i64,
            id: &str,
        ) -> sqlx::Result<Option<Option<String>>> {
            sqlx::query_scalar!(
                "SELECT powered_by FROM machines WHERE id = $1 AND space_id = $2",
                id,
                space
            )
            .fetch_optional(&mut **tx)
            .await
        }
        if parent(&mut tx, space, machine).await?.is_none() {
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
                match parent(&mut tx, space, &id).await? {
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
        sqlx::query!(
            "UPDATE machines SET powered_by = $1, power_channel = $2 WHERE id = $3 AND space_id = $4",
            power.map(|p| p.device.as_str()),
            power.map(|p| p.channel as i32),
            machine,
            space
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.changed();
        Ok(Ok(()))
    }

    /// The owner renames a machine. Returns whether the machine is in the space.
    pub async fn rename(&self, space: i64, machine: &str, name: &str) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        let n = sqlx::query!(
            "UPDATE machines SET name = $1 WHERE id = $2 AND space_id = $3",
            name,
            machine,
            space
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        self.changed();
        Ok(n > 0)
    }

    /// The owner chooses a machine's profile (None: back to recognising it). Returns whether the
    /// machine is in the space.
    pub async fn set_profile(
        &self,
        space: i64,
        machine: &str,
        profile: Option<&str>,
    ) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        let n = sqlx::query!(
            "UPDATE machines SET profile = $1, profile_chosen = $2 WHERE id = $3 AND space_id = $4",
            profile,
            profile.is_some(),
            machine,
            space
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        self.changed();
        Ok(n > 0)
    }

    /// A machine's labels: trimmed, without doubles, at most 12 of at most 24 characters.
    pub async fn set_labels(
        &self,
        space: i64,
        machine: &str,
        labels: &[String],
    ) -> sqlx::Result<bool> {
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
        let mut tx = self.pg().space(space).await?;
        let n = sqlx::query!(
            "UPDATE machines SET labels = $3 WHERE id = $1 AND space_id = $2",
            machine,
            space,
            serde_json::to_string(&clean).unwrap_or_default()
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        self.changed();
        Ok(n > 0)
    }

    /// A Prusa Connect printer asking with its code whether it has been added: None for an unknown
    /// code, `Some(None)` while it waits, `Some(Some(token))` once it is in a space. The code keeps
    /// getting its token until the printer's first report with that token (`code_spent`):
    /// `registrator.cpp:233` takes the token only from the answer it reads.
    pub async fn token_for_code(&self, code: &str) -> sqlx::Result<Option<Option<String>>> {
        let found = sqlx::query!(
            "SELECT token, space_id FROM machines WHERE pairing_code = $1 AND link = 'prusa-connect'",
            code
        )
        .fetch_optional(self.pg().admin(Bypass::Secret))
        .await?;
        Ok(match found {
            None => None,
            Some(r) if r.space_id.is_none() => Some(None),
            Some(r) => Some(Some(r.token)),
        })
    }

    /// The printer reported with its token (telemetry or event): it has read it, so its code is spent.
    pub async fn code_spent(&self, token: &str) -> sqlx::Result<()> {
        sqlx::query!(
            "UPDATE machines SET pairing_code = NULL
             WHERE token = $1 AND link = 'prusa-connect' AND space_id IS NOT NULL AND pairing_code IS NOT NULL",
            token
        )
        .execute(self.pg().admin(Bypass::Secret))
        .await?;
        Ok(())
    }

    /// The profile recognised for a machine from what it said when it registered, unless its owner
    /// chose one.
    pub async fn recognise(&self, token: &str, profile: &str) -> sqlx::Result<()> {
        sqlx::query!(
            "UPDATE machines SET profile = $1 WHERE token = $2 AND NOT profile_chosen",
            profile,
            token
        )
        .execute(self.pg().admin(Bypass::Secret))
        .await?;
        Ok(())
    }

    /// Where a machine's reports come from, as the hub sees it (its address, or its router's when it
    /// is elsewhere).
    pub async fn set_peer(&self, token: &str, peer: &str) -> sqlx::Result<()> {
        sqlx::query!(
            "UPDATE machines SET peer = $1 WHERE token = $2",
            peer,
            token
        )
        .execute(self.pg().admin(Bypass::Secret))
        .await?;
        Ok(())
    }

    /// A machine's id by its token, if the token is known.
    pub async fn machine_id_by_token(&self, token: &str) -> sqlx::Result<Option<String>> {
        sqlx::query_scalar!("SELECT id FROM machines WHERE token = $1", token)
            .fetch_optional(self.pg().admin(Bypass::Secret))
            .await
    }

    /// Moves a machine to another space (design decisions/20261119-device-move.md): what someone may
    /// do with it there is that space's business. Refused while it has work under way. What goes
    /// along is the machine itself (name, labels, plate, service interval), what its stick holds,
    /// and its agent, which keeps its token. What stays: its jobs and the records of what was sent
    /// (their files are the old space's). What the old space said about it goes, in this
    /// transaction: its permissions, group places, requests for permission and queue items are
    /// deleted; a spool loaded in it and a camera watching it are unlinked (their keys name the
    /// machine with its space, so the move fails while one is left).
    pub async fn move_machine(
        &self,
        from: i64,
        machine: &str,
        to: i64,
    ) -> Result<Result<(), MoveRefused>, BoxError> {
        // The check and the move are one transaction, the machine's row locked: a job put on it
        // meanwhile ([`Store::add_job`] locks the same row) waits until it has moved, or is there
        // before the check sees it.
        let mut tx = self.pg().admin(Bypass::InstanceAdmin).begin().await?;
        let here = sqlx::query_scalar!(
            "SELECT id FROM machines WHERE id = $1 AND space_id = $2 FOR UPDATE",
            machine,
            from
        )
        .fetch_optional(&mut *tx)
        .await?
        .is_some();
        if !here {
            return Ok(Err(MoveRefused::NoSuchMachine));
        }
        if from == to {
            return Ok(Err(MoveRefused::InThatSpace));
        }
        let jobs = Self::job_facts_on(&mut tx, &[machine.to_owned()]).await?;
        let sending = Self::files_on_their_way(&mut tx, machine).await?;
        let stays = self.lock().unwrap().why_it_stays(&jobs, machine, sending);
        if let Some(why) = stays {
            return Ok(Err(MoveRefused::JobOpen(why)));
        }
        for table in [
            "machine_grants",
            "machine_group_members",
            "access_requests",
            "machine_queue",
        ] {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "DELETE FROM {table} WHERE machine_id = $1 AND space_id = $2"
            )))
            .bind(machine)
            .bind(from)
            .execute(&mut *tx)
            .await?;
        }
        // A spool loaded in it stays in the old space, no longer loaded; a camera watching it stays
        // too, watching nothing.
        sqlx::query("UPDATE spool SET machine = NULL WHERE machine = $1 AND space_id = $2")
            .bind(machine)
            .bind(from)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE cameras SET machine_id = NULL WHERE machine_id = $1 AND space_id = $2")
            .bind(machine)
            .bind(from)
            .execute(&mut *tx)
            .await?;
        sqlx::query!(
            "UPDATE machines SET space_id = $2, farm = false WHERE id = $1",
            machine,
            to
        )
        .execute(&mut *tx)
        .await?;
        Self::transfers_moved_away(&mut tx, machine).await?;
        tx.commit().await?;
        self.lock().unwrap().moved_away(machine);
        Ok(Ok(()))
    }

    /// Removes a machine of the space, with what the hub keeps of it in memory; its permissions,
    /// group places and requests go with its row (ON DELETE CASCADE). Returns whether it was there.
    pub async fn remove_machine(&self, space: i64, machine: &str) -> sqlx::Result<bool> {
        let removed = self.remove_machine_row(space, machine).await?;
        self.lock().unwrap().forget_machine(machine);
        Ok(removed)
    }

    /// Whether a machine is a laser (by its kind, without loading the whole machine).
    pub async fn is_laser(&self, space: i64, machine: &str) -> sqlx::Result<bool> {
        Ok(self.kind_of(space, machine).await? == Some(MachineKind::Laser))
    }

    /// A machine's kind, without loading the whole machine; None when it is not in the space.
    pub async fn kind_of(&self, space: i64, machine: &str) -> sqlx::Result<Option<MachineKind>> {
        let mut tx = self.pg().space(space).await?;
        let kind = sqlx::query_scalar!(
            "SELECT kind FROM machines WHERE id = $1 AND space_id = $2",
            machine,
            space
        )
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(kind.and_then(|k| serde_json::from_str::<MachineKind>(&k).ok()))
    }

    /// A Marlin printer's bed as its space says it (None: not known). Returns whether the machine
    /// is in the space.
    pub async fn set_bed_mm(
        &self,
        space: i64,
        machine: &str,
        bed: Option<[f64; 3]>,
    ) -> sqlx::Result<bool> {
        let bed = bed.map(|b| serde_json::to_string(&b).unwrap_or_default());
        let mut tx = self.pg().space(space).await?;
        let n = sqlx::query!(
            "UPDATE machines SET bed_mm = $1 WHERE id = $2 AND space_id = $3",
            bed,
            machine,
            space
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        self.changed();
        Ok(n > 0)
    }

    /// A laser's optical power as its owner says (None: the profile's). Returns whether the machine
    /// is in the space.
    pub async fn set_laser_w(
        &self,
        space: i64,
        machine: &str,
        watts: Option<f64>,
    ) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        let n = sqlx::query!(
            "UPDATE machines SET laser_w = $1 WHERE id = $2 AND space_id = $3",
            watts,
            machine,
            space
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        self.changed();
        Ok(n > 0)
    }

    /// Takes a machine out of use (with why), or back into use (None). Returns whether the machine
    /// is in the space.
    pub async fn set_out_of_use(
        &self,
        space: i64,
        machine: &str,
        reason: Option<&str>,
    ) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        let n = sqlx::query!(
            "UPDATE machines SET out_of_use = $1,
                 out_of_use_at = CASE WHEN $1::text IS NULL THEN NULL ELSE at_unix($2) END
             WHERE id = $3 AND space_id = $4",
            reason,
            now(),
            machine,
            space
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        self.changed();
        Ok(n > 0)
    }
}

impl Store {
    /// A space's machines, by name: their rows on Postgres, completed with what the modules still on
    /// SQLite and the hub's memory know (stock, jobs, queue, farm, commands, questions).
    pub async fn machines(&self, space: i64) -> Result<Vec<Machine>, BoxError> {
        Ok(self.machines_and_jobs(space).await?.0)
    }

    /// [`Store::machines`], and what their jobs tell ([`JobFacts`]), read once for both.
    pub(crate) async fn machines_and_jobs(
        &self,
        space: i64,
    ) -> Result<(Vec<Machine>, JobFacts), BoxError> {
        let mut tx = self.pg().space(space).await?;
        // An agent's cameras device is no machine (`MachineKind::Cameras`): its cameras are listed.
        let rows: Vec<MachineRow> = sqlx::query_as(machine_select!(
            "WHERE space_id = $1 AND kind <> '\"cameras\"' ORDER BY name COLLATE \"C\""
        ))
        .bind(space)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        let jobs = self.job_facts(space, &ids).await?;
        let completion = crate::stock::in_space!(self, space, |w| async {
            Ok::<_, sqlx::Error>(super::machines::Completion {
                loaded: w.loaded().await?,
                queues: w.queues(&ids).await?,
                confirms: w.confirms(&jobs, &ids).await?,
            })
        })
        .await?;
        let mut machines = self
            .lock()
            .unwrap()
            .complete_machines(rows, &jobs, completion);
        // What the queued files say of themselves (their time, a picture), by id as before.
        let queued: Vec<String> = machines
            .iter()
            .flat_map(|m| m.queue.iter().map(|q| q.file_id.clone()))
            .collect();
        if !queued.is_empty() {
            let mut tx = self.pg().space(space).await?;
            let metas: std::collections::HashMap<String, String> =
                sqlx::query_as("SELECT id, meta FROM files WHERE id = ANY($1)")
                    .bind(&queued)
                    .fetch_all(&mut *tx)
                    .await?
                    .into_iter()
                    .collect();
            tx.commit().await?;
            for q in machines.iter_mut().flat_map(|m| m.queue.iter_mut()) {
                if let Some(meta) = metas
                    .get(&q.file_id)
                    .and_then(|m| serde_json::from_str::<crate::gcode::FileMeta>(m).ok())
                {
                    q.estimated_s = meta.estimated_s;
                    q.thumbnail = meta.thumbnail;
                }
            }
        }
        Ok((machines, jobs))
    }

    /// One machine of a space.
    pub async fn machine(&self, space: i64, id: &str) -> Result<Option<Machine>, BoxError> {
        Ok(self.machines(space).await?.into_iter().find(|m| m.id == id))
    }
    /// A machine by its token as its own row tells (not completed: no jobs, stock or commands), and
    /// its space, if it is in one (`Bypass::Secret`).
    pub(crate) async fn machine_by_token(
        &self,
        token: &str,
    ) -> sqlx::Result<Option<(Machine, Option<i64>)>> {
        let found: Option<MachineRow> = sqlx::query_as(machine_select!("WHERE token = $1"))
            .bind(token)
            .fetch_optional(self.pg().admin(Bypass::Secret))
            .await?;
        Ok(found.map(|r| {
            let space = r.space_id;
            let attached = self.lock().unwrap().attached.contains_key(&r.id);
            (r.into_machine(now(), attached), space)
        }))
    }
}

/// Why a machine is not moved to another space ([`Store::move_machine`]).
#[derive(Debug, PartialEq, Eq)]
pub enum MoveRefused {
    /// Not a machine of the space it is moved from.
    NoSuchMachine,
    /// It is in the space it would move to.
    InThatSpace,
    /// It has work not over (`409 device/JOB_OPEN`, design decisions/20261119-device-move.md): why.
    JobOpen(String),
}

#[cfg(test)]
impl Store {
    /// For tests: a virtual machine of `profile` in the default space, reached by `link` when one is
    /// given (as if it had registered that way): its space, id and token.
    pub(crate) async fn add_test_machine(
        &self,
        profile: &str,
        link: Option<&str>,
    ) -> (i64, String, String) {
        let space = self.default_space().await.unwrap().0;
        let id = self
            .add_virtual(space, crate::profiles::find(profile).unwrap())
            .await
            .unwrap();
        if let Some(link) = link {
            sqlx::query("UPDATE machines SET link = $1 WHERE id = $2")
                .bind(link)
                .bind(&id)
                .execute(self.pg().admin_in_tests())
                .await
                .unwrap();
        }
        let token = self.token_of(&id).await;
        (space, id, token)
    }

    /// For tests: a machine registered as `name` of `kind` and paired into the default space: its
    /// space, id and token.
    pub(crate) async fn register_test_machine(
        &self,
        name: &str,
        kind: maker_connect_protocol::MachineKind,
    ) -> (i64, String, String) {
        let space = self.default_space().await.unwrap().0;
        let (id, token, code) = self
            .register_linked(name, kind, "0.2.0", "virtual")
            .await
            .unwrap();
        self.pair(space, &code).await.unwrap();
        (space, id, token)
    }

    /// For tests: when the machine's agent last reported (on Postgres, and its copy).
    pub(crate) async fn set_last_seen(&self, machine: &str, at: i64) {
        sqlx::query("UPDATE machines SET last_seen = at_unix($1) WHERE id = $2")
            .bind(at)
            .bind(machine)
            .execute(self.pg().admin_in_tests())
            .await
            .unwrap();
    }

    /// For tests: a machine's token.
    pub(crate) async fn token_of(&self, id: &str) -> String {
        sqlx::query_scalar("SELECT token FROM machines WHERE id = $1")
            .bind(id)
            .fetch_one(self.pg().admin_in_tests())
            .await
            .unwrap()
    }

    /// For tests: a machine reports it is idle.
    pub(crate) async fn idle_test_machine(&self, id: &str) {
        let token = self.token_of(id).await;
        let idle = maker_connect_protocol::Telemetry {
            state: maker_connect_protocol::MachineState::Idle,
            ..Default::default()
        };
        self.report(&token, &idle).await.unwrap();
    }
}
