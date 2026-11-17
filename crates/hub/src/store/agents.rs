use super::*;

impl Store {
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
    /// The code keeps getting its token until the printer's first report with that token
    /// (`code_spent`): `registrator.cpp:233` takes the token only from the answer it reads.
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
            Some((token, Some(_))) => Some(Some(token)),
        })
    }

    /// The printer reported with its token (telemetry or event): it has read it, so its
    /// code is spent.
    pub fn code_spent(&self, token: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE machines SET pairing_code = NULL
             WHERE token = ?1 AND link = 'prusa-connect' AND space_id IS NOT NULL AND pairing_code IS NOT NULL",
            [token],
        )?;
        Ok(())
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

    pub(super) fn machine_id(&self, token: &str) -> rusqlite::Result<Option<String>> {
        self.db
            .query_row("SELECT id FROM machines WHERE token = ?1", [token], |r| {
                r.get(0)
            })
            .optional()
    }
}
