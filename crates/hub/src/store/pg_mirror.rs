//! Transitional (hub core v2 step 2, lane 3a; lane 3b removes it): Postgres holds the machines and
//! files, and each write there is copied onto the SQLite rows the modules still on SQLite read and join
//! (farm, queue, access, stock, printer_files, spoolman, jobs). One way only, Postgres to SQLite.
//!
//! The columns those SQLite modules write themselves stay theirs until lane 3b moves them, so the copy
//! never overwrites them (it sets them only on a new row): `induction_required` (crate::access),
//! `ppe` (crate::safety), `usb_print_by`/`_at` (crate::printer_files), `agent_update_to`/`_at`
//! (crate::updates), `farm`, `sheet`, `service_every_h`, `serviced_at` (crate::farm).

use super::*;

/// A machine's row as the website reads it, from either database (one way of reading it).
pub(crate) struct MachineRow {
    pub id: String,
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
    pub labels: Option<String>,
}

impl MachineRow {
    /// The machine as far as its own row tells; [`Legacy::complete_machines`] adds the rest.
    pub(crate) fn into_machine(self, now: i64) -> Machine {
        let seen = self.last_seen.map(|t| now - t);
        let online = seen.is_some_and(|s| s <= ONLINE_SECONDS);
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
                .and_then(|t| serde_json::from_str(&t).ok()),
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
            agent_update: None,
            agent_update_to: None,
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
            usb_print_by: None,
            moonraker_token: (self.link == "simplyprint").then_some(self.token),
            link: self.link,
            laser_w: self.laser_w,
            labels: self
                .labels
                .and_then(|l| serde_json::from_str(&l).ok())
                .unwrap_or_default(),
            induction_required: false,
            training: false,
            ppe: Vec::new(),
            confirm: None,
            farm: false,
            upkeep: None,
            sheet: None,
        }
    }
}

impl Store {
    /// Copies a machine's row (by id or by token) onto SQLite, or removes it there when it is gone.
    pub(crate) async fn mirror_machine(
        &self,
        id: Option<&str>,
        token: Option<&str>,
    ) -> sqlx::Result<()> {
        let rows = sqlx::query!(
            r#"SELECT id, space_id, name, kind, agent_version, token, pairing_code,
                   unix_seconds(registered_at) AS "registered_at!", unix_seconds(last_seen) AS last_seen,
                   telemetry, profile, profile_chosen, link, details, mesh, media, powered_by, power_channel,
                   reached_through, agent, peer, out_of_use, unix_seconds(out_of_use_at) AS out_of_use_at,
                   laser_w, labels, induction_required, ppe, usb_print_by,
                   unix_seconds(usb_print_at) AS usb_print_at, agent_update_to,
                   unix_seconds(agent_update_at) AS agent_update_at, farm, sheet, service_every_h,
                   unix_seconds(serviced_at) AS serviced_at
               FROM machines WHERE id = $1 OR token = $2"#,
            id,
            token
        )
        .fetch_all(&self.pg().admin)
        .await?;
        let legacy = self.lock().unwrap();
        if rows.is_empty() {
            if let Some(id) = id {
                let _ = legacy
                    .db
                    .execute("DELETE FROM machines WHERE id = ?1", [id]);
            }
            if let Some(token) = token {
                let _ = legacy
                    .db
                    .execute("DELETE FROM machines WHERE token = ?1", [token]);
            }
            return Ok(());
        }
        for r in rows {
            legacy
                .db
                .execute(
                    "INSERT INTO machines (id, space_id, name, kind, agent_version, token, pairing_code,
                         registered_at, last_seen, telemetry, profile, profile_chosen, link, details, mesh,
                         media, powered_by, power_channel, reached_through, agent, peer, out_of_use,
                         out_of_use_at, laser_w, labels, induction_required, ppe, usb_print_by,
                         usb_print_at, agent_update_to, agent_update_at, farm, sheet, service_every_h,
                         serviced_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                         ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33,
                         ?34, ?35)
                     ON CONFLICT (id) DO UPDATE SET space_id = excluded.space_id, name = excluded.name,
                         kind = excluded.kind, agent_version = excluded.agent_version,
                         token = excluded.token, pairing_code = excluded.pairing_code,
                         registered_at = excluded.registered_at, last_seen = excluded.last_seen,
                         telemetry = excluded.telemetry, profile = excluded.profile,
                         profile_chosen = excluded.profile_chosen, link = excluded.link,
                         details = excluded.details, mesh = excluded.mesh, media = excluded.media,
                         powered_by = excluded.powered_by, power_channel = excluded.power_channel,
                         reached_through = excluded.reached_through, agent = excluded.agent,
                         peer = excluded.peer, out_of_use = excluded.out_of_use,
                         out_of_use_at = excluded.out_of_use_at, laser_w = excluded.laser_w,
                         labels = excluded.labels",
                    rusqlite::params![
                        r.id,
                        r.space_id,
                        r.name,
                        r.kind,
                        r.agent_version,
                        r.token,
                        r.pairing_code,
                        r.registered_at,
                        r.last_seen,
                        r.telemetry,
                        r.profile,
                        r.profile_chosen,
                        r.link,
                        r.details,
                        r.mesh,
                        r.media,
                        r.powered_by,
                        r.power_channel,
                        r.reached_through,
                        r.agent,
                        r.peer,
                        r.out_of_use,
                        r.out_of_use_at,
                        r.laser_w,
                        r.labels,
                        r.induction_required,
                        r.ppe,
                        r.usb_print_by,
                        r.usb_print_at,
                        r.agent_update_to,
                        r.agent_update_at,
                        r.farm,
                        r.sheet,
                        r.service_every_h,
                        r.serviced_at,
                    ],
                )
                .map_err(|e| sqlx::Error::Protocol(format!("mirror to SQLite: {e}")))?;
        }
        Ok(())
    }

    /// Copies a file's row onto SQLite, or removes it there when it is gone.
    pub(crate) async fn mirror_file(&self, id: &str) -> sqlx::Result<()> {
        let row = sqlx::query!(
            r#"SELECT id, space_id, name, size, unix_seconds(uploaded_at) AS "uploaded_at!", meta
               FROM files WHERE id = $1"#,
            id
        )
        .fetch_optional(&self.pg().admin)
        .await?;
        let legacy = self.lock().unwrap();
        let done = match row {
            None => legacy.db.execute("DELETE FROM files WHERE id = ?1", [id]),
            Some(r) => legacy.db.execute(
                "INSERT INTO files (id, space_id, name, size, uploaded_at, meta) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (id) DO UPDATE SET space_id = excluded.space_id, name = excluded.name,
                     size = excluded.size, uploaded_at = excluded.uploaded_at, meta = excluded.meta",
                rusqlite::params![r.id, r.space_id, r.name, r.size, r.uploaded_at, r.meta],
            ),
        };
        done.map_err(|e| sqlx::Error::Protocol(format!("mirror to SQLite: {e}")))?;
        Ok(())
    }

    /// A space's machines, by name: their rows on Postgres, completed with what the modules still on
    /// SQLite and the hub's memory know (stock, jobs, queue, farm, commands, questions).
    pub async fn machines(&self, space: i64) -> Result<Vec<Machine>, BoxError> {
        let mut tx = self.pg().space(space).await?;
        let rows = sqlx::query_as!(
            MachineRow,
            r#"SELECT id AS "id!", name AS "name!", kind AS "kind!", agent_version AS "agent_version!",
                   token AS "token!", unix_seconds(last_seen) AS last_seen, telemetry, profile,
                   profile_chosen AS "profile_chosen!", link AS "link!", details, mesh, media, powered_by,
                   power_channel::bigint AS power_channel, reached_through, agent, peer, out_of_use,
                   unix_seconds(out_of_use_at) AS out_of_use_at, laser_w, labels
               FROM machines WHERE space_id = $1 ORDER BY name COLLATE "C""#,
            space
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(self.lock().unwrap().complete_machines(space, rows)?)
    }

    /// One machine of a space.
    pub async fn machine(&self, space: i64, id: &str) -> Result<Option<Machine>, BoxError> {
        Ok(self.machines(space).await?.into_iter().find(|m| m.id == id))
    }
}
