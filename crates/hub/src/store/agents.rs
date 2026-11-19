use super::*;
use crate::store::pg::Bypass;

impl Legacy {
    /// A report kept ([`Store::report`]): its temperatures go into the machine's history.
    pub(crate) fn reported(&mut self, machine: &str, telemetry: &Telemetry) {
        self.sample(machine, telemetry);
        self.changed();
    }
}

impl Store {
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
    pub(crate) async fn follow_own_print_of(
        &self,
        machine: String,
        space: i64,
        telemetry: &Telemetry,
        raw: Option<&str>,
    ) -> Result<(), BoxError> {
        use maker_connect_protocol::MachineState as S;
        if self.active_job(space, &machine).await?.is_some() {
            self.lock().unwrap().own_prints.remove(&machine);
            return Ok(());
        }
        let mut tx = self.pg().space(space).await?;
        let by: Option<String> =
            sqlx::query_scalar("SELECT usb_print_by FROM machines WHERE id = $1")
                .bind(&machine)
                .fetch_optional(&mut *tx)
                .await?
                .flatten();
        tx.commit().await?;
        let ended = {
            let mut legacy = self.lock().unwrap();
            match (telemetry.state, telemetry.job.as_ref()) {
                (S::Busy | S::Paused, Some(job)) => {
                    let percent = job.done * 100 / job.total.max(1);
                    let started = now() - job.elapsed_s.unwrap_or(0) as i64;
                    let own = legacy
                        .own_prints
                        .entry(machine.clone())
                        .or_insert(OwnPrint {
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
                    None
                }
                // Still the same print: a question on its screen, or out of touch.
                (S::Attention | S::Disconnected, _) => None,
                _ => legacy.own_prints.remove(&machine),
            }
        };
        let Some(own) = ended else {
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
            _ => (
                JobState::Stopped,
                Some("ended at the printer before it was done"),
            ),
        };
        let mut tx = self.pg().space(space).await?;
        let file = if own.file.is_empty() {
            "a print started on the printer".to_owned()
        } else {
            own.file
        };
        sqlx::query(
            "INSERT INTO jobs (space_id, machine_id, file_id, file_name, state, created_at, started_at, ended_at, reason, by_name)
             VALUES ($1, $2, '', $3, $4, at_unix($5), at_unix($5), at_unix($6), $7, $8)",
        )
        .bind(space)
        .bind(&machine)
        .bind(file)
        .bind(state.name())
        .bind(own.started_at)
        .bind(now())
        .bind(reason)
        .bind(own.by.as_deref().unwrap_or("on the printer"))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.changed();
        Ok(())
    }

    /// What an agent's report asks of the bed, from the printer's own menu ([`Store::report`]).
    pub(crate) async fn bed_from_menu(
        &self,
        space: i64,
        machine: &str,
        telemetry: &Telemetry,
    ) -> Result<(), BoxError> {
        // "Set Ready" on the printer's own menu: the bed is clear, as on its page.
        if telemetry.bed_clear {
            self.print_taken(space, machine).await?;
        }
        // "Not ready" on the printer's own menu: the next job waits again (PrusaLink's
        // `fw_cancel_ready`: `CancelReady`, M72 S0).
        if telemetry.not_ready {
            self.not_ready(space, machine).await?;
        }
        // "Reprint" on the printer's own menu: its last job again, where it may start.
        if telemetry.reprint {
            self.reprint(space, machine).await?;
        }
        Ok(())
    }

    /// A machine in a space, by its token: (id, space).
    pub(crate) async fn placed(&self, token: &str) -> sqlx::Result<Option<(String, i64)>> {
        Ok(sqlx::query!(
            r#"SELECT id, space_id AS "space_id!" FROM machines WHERE token = $1 AND space_id IS NOT NULL"#,
            token
        )
        .fetch_optional(self.pg().admin(Bypass::Secret))
        .await?
        .map(|r| (r.id, r.space_id)))
    }

    /// A print the machine runs itself (no job of the hub's: from its USB stick or SD card, or
    /// started at its screen) is followed while it runs and recorded as a job when it ends:
    /// `Legacy::follow_own_print_of`.
    pub async fn follow_own_print(
        &self,
        token: &str,
        telemetry: &Telemetry,
        raw: Option<&str>,
    ) -> Result<(), BoxError> {
        let Some((machine, space)) = self.placed(token).await? else {
            return Ok(());
        };
        self.follow_own_print_of(machine, space, telemetry, raw)
            .await
    }

    /// An agent's report: kept as the machine's last. Returns the machine's space id if it is
    /// paired (`Some(None)` while it is not), or None when the token is unknown.
    pub async fn report(
        &self,
        token: &str,
        telemetry: &Telemetry,
    ) -> Result<Option<Option<i64>>, BoxError> {
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
        if (telemetry.bed_clear || telemetry.not_ready || telemetry.reprint)
            && let Some((machine, space)) = self.placed(token).await?
        {
            self.bed_from_menu(space, &machine, telemetry).await?;
        }
        let mut tx = self.pg().admin(Bypass::Secret).begin().await?;
        let updated = sqlx::query!(
            "UPDATE machines SET last_seen = at_unix($1), telemetry = $2 WHERE token = $3 RETURNING id, space_id",
            now(),
            serde_json::to_string(&kept).unwrap_or_default(),
            token
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(agent) = &telemetry.agent {
            sqlx::query!(
                "UPDATE machines SET agent = $1 WHERE token = $2",
                serde_json::to_string(agent).unwrap_or_default(),
                token
            )
            .execute(&mut *tx)
            .await?;
        }
        if let Some(mesh) = &telemetry.mesh {
            let stored = StoredMesh {
                rows: mesh.rows.clone(),
                taken_at: now(),
            };
            sqlx::query!(
                "UPDATE machines SET mesh = $1 WHERE token = $2",
                serde_json::to_string(&stored).unwrap_or_default(),
                token
            )
            .execute(&mut *tx)
            .await?;
        }
        let Some(row) = updated else {
            tx.commit().await?;
            return Ok(None);
        };
        // What the machine says it is: its profile, unless the owner chose one.
        if let Some(profile) = telemetry
            .identity
            .as_ref()
            .and_then(crate::profiles::detect)
        {
            sqlx::query!(
                "UPDATE machines SET profile = $1 WHERE token = $2 AND NOT profile_chosen",
                profile.id,
                token
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        self.lock().unwrap().reported(&row.id, telemetry);
        Ok(Some(row.space_id))
    }
}
