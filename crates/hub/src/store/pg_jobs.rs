//! Jobs on Postgres (`migrations/20261118000300_machines.sql`): `jobs.rs`'s queries as async [`Store`]
//! methods (hub core v2 step 2, lane 3b). A job is found by its machine or its id across spaces, as
//! before (the admin pool: a machine's jobs from an earlier space stay its own); a space's lists go
//! through the space's transaction.
//!
//! What a job counts off its spool is the stock's (still on SQLite, lane 4): read from Postgres first,
//! then booked under the lock, never across an `.await`.

use super::*;
use std::collections::HashMap;

/// The columns of a job, as [`JobRow`] reads them.
macro_rules! job_select {
    ($tail:literal) => {
        concat!(
            "SELECT id, machine_id, file_id, file_name, state, unix_seconds(created_at) AS created_at,
                 unix_seconds(started_at) AS started_at, unix_seconds(ended_at) AS ended_at, reason,
                 from_line, restart_line, by_name, unix_seconds(taken_at) AS taken_at, part_id, per_plate,
                 unix_seconds(confirmed_at) AS confirmed_at
             FROM jobs ",
            $tail
        )
    };
}

/// A job's row: the job, and what the bed and the farm keep on it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct JobRow {
    id: i64,
    machine_id: String,
    file_id: String,
    file_name: String,
    state: String,
    created_at: i64,
    started_at: Option<i64>,
    ended_at: Option<i64>,
    reason: Option<String>,
    from_line: Option<i64>,
    restart_line: Option<i64>,
    by_name: Option<String>,
    pub taken_at: Option<i64>,
    pub part_id: Option<i64>,
    pub per_plate: Option<i64>,
    pub confirmed_at: Option<i64>,
}

impl JobRow {
    pub(crate) fn job(&self) -> Job {
        Job {
            id: self.id,
            machine_id: self.machine_id.clone(),
            file_id: self.file_id.clone(),
            file_name: self.file_name.clone(),
            state: JobState::parse(&self.state),
            created_at: self.created_at,
            started_at: self.started_at,
            ended_at: self.ended_at,
            reason: self.reason.clone(),
            from_line: self.from_line.map(|l| l as u64),
            restart_line: self.restart_line.map(|l| l as u64),
            by: self.by_name.clone(),
        }
    }

    fn on_bed(&self) -> OnBed {
        OnBed {
            job: self.id,
            file_name: self.file_name.clone(),
            ended_at: self.ended_at,
        }
    }
}

/// What the machines of a space show of their jobs, read at once (for [`Legacy::complete_machines`],
/// the farm and the queue): per machine its job that is not over and its last job, and per part the
/// plates under way.
#[derive(Debug, Default, Clone)]
pub(crate) struct JobFacts {
    active: HashMap<String, JobRow>,
    last: HashMap<String, JobRow>,
    /// Per machine with a service interval: (every so many hours, printing seconds since its service).
    upkeep: HashMap<String, (f64, i64)>,
    /// Per part: plates under way (on a bed, or ended and not yet confirmed), and the parts they hold.
    pub under_way: HashMap<i64, (i64, i64)>,
}

impl JobFacts {
    /// A machine's job that is not over yet.
    pub(crate) fn active_job(&self, machine: &str) -> Option<Job> {
        self.active.get(machine).map(JobRow::job)
    }

    /// The farm's part of a machine's job that is not over.
    pub(crate) fn active_part(&self, machine: &str) -> Option<i64> {
        self.active.get(machine).and_then(|j| j.part_id)
    }

    /// A machine's last job, when it was interrupted and says where to start again.
    pub(crate) fn resumable_job(&self, machine: &str) -> Option<Job> {
        self.last.get(machine).map(JobRow::job).filter(|j| {
            matches!(j.state, JobState::Stopped | JobState::Failed) && j.restart_line.is_some()
        })
    }

    /// A print still on the bed: the machine's last job, when it finished and nobody has said yet
    /// that it was taken.
    pub(crate) fn on_bed(&self, machine: &str) -> Option<OnBed> {
        self.last
            .get(machine)
            .filter(|j| j.state == "finished" && j.taken_at.is_none())
            .map(JobRow::on_bed)
    }

    /// Ready for the next job: its last job finished and was taken off.
    pub(crate) fn ready_after(&self, machine: &str) -> Option<OnBed> {
        self.last
            .get(machine)
            .filter(|j| j.state == "finished" && j.taken_at.is_some())
            .map(JobRow::on_bed)
    }

    /// How the machine's last job ended, when it failed or was stopped and nobody said yet that the
    /// bed is clear: then its queue waits (what it left may still be there).
    pub(crate) fn bed_unchecked(&self, machine: &str) -> Option<String> {
        self.last
            .get(machine)
            .filter(|j| j.taken_at.is_none() && matches!(j.state.as_str(), "failed" | "stopped"))
            .map(|j| j.state.clone())
    }

    /// The plate that ended on a machine and waits for someone: its last job, when it was for a part
    /// and is not yet confirmed. The part's and project's names are the farm's ([`Legacy`]).
    pub(crate) fn waiting_plate(&self, machine: &str) -> Option<&JobRow> {
        self.last.get(machine).filter(|j| {
            j.part_id.is_some()
                && matches!(j.state.as_str(), "finished" | "stopped" | "failed")
                && j.confirmed_at.is_none()
        })
    }

    /// A machine's upkeep, when a service interval is set.
    pub(crate) fn upkeep(&self, machine: &str) -> Option<crate::farm::Upkeep> {
        self.upkeep.get(machine).map(|&(every_h, seconds)| {
            let since_h = seconds.max(0) as f64 / 3600.0;
            crate::farm::Upkeep {
                every_h,
                since_h,
                due: since_h >= every_h,
            }
        })
    }

    /// A plate just sent for `part` counts as under way.
    pub(crate) fn sent(&mut self, part: i64, per_plate: i64) {
        let entry = self.under_way.entry(part).or_default();
        entry.0 += 1;
        entry.1 += per_plate;
    }
}

impl Store {
    /// What the machines `ids` show of their jobs (see [`JobFacts`]).
    pub(crate) async fn job_facts(&self, ids: &[String]) -> sqlx::Result<JobFacts> {
        let admin = &self.pg().admin;
        let mut facts = JobFacts::default();
        let last: Vec<JobRow> = sqlx::query_as(job_select!(
            "WHERE id IN (SELECT MAX(id) FROM jobs WHERE machine_id = ANY($1) GROUP BY machine_id)"
        ))
        .bind(ids)
        .fetch_all(admin)
        .await?;
        for row in last {
            facts.last.insert(row.machine_id.clone(), row);
        }
        // The first not over, by id.
        let active: Vec<JobRow> = sqlx::query_as(job_select!(
            "WHERE id IN (SELECT MIN(id) FROM jobs WHERE machine_id = ANY($1)
                 AND state IN ('loaded', 'queued', 'printing', 'paused', 'stopping') GROUP BY machine_id)"
        ))
        .bind(ids)
        .fetch_all(admin)
        .await?;
        for row in active {
            facts.active.insert(row.machine_id.clone(), row);
        }
        let now = now();
        let upkeep: Vec<(String, f64, i64)> = sqlx::query_as(
            "SELECT m.id, m.service_every_h,
                 COALESCE(SUM(unix_seconds(COALESCE(j.ended_at, at_unix($2))) - unix_seconds(j.started_at)), 0)::bigint
             FROM machines m LEFT JOIN jobs j ON j.machine_id = m.id AND j.started_at IS NOT NULL
                 AND unix_seconds(COALESCE(j.ended_at, at_unix($2))) > COALESCE(unix_seconds(m.serviced_at), 0)
             WHERE m.id = ANY($1) AND m.service_every_h IS NOT NULL
             GROUP BY m.id, m.service_every_h",
        )
        .bind(ids)
        .bind(now)
        .fetch_all(admin)
        .await?;
        for (id, every_h, seconds) in upkeep {
            facts.upkeep.insert(id, (every_h, seconds));
        }
        let under_way: Vec<(i64, i64, i64)> = sqlx::query_as(
            "SELECT part_id, COUNT(*), COALESCE(SUM(per_plate), 0)::bigint FROM jobs
             WHERE part_id IS NOT NULL AND (state IN ('loaded', 'queued', 'printing', 'paused', 'stopping')
                 OR (state = 'finished' AND confirmed_at IS NULL))
             GROUP BY part_id",
        )
        .fetch_all(admin)
        .await?;
        for (part, plates, held) in under_way {
            facts.under_way.insert(part, (plates, held));
        }
        Ok(facts)
    }

    /// [`Store::job_facts`] of one machine.
    pub(crate) async fn job_facts_of(&self, machine: &str) -> sqlx::Result<JobFacts> {
        self.job_facts(&[machine.to_owned()]).await
    }

    /// Where each file of a space was printed, newest first, at most `per_file` times each.
    pub async fn prints(
        &self,
        space: i64,
        per_file: usize,
    ) -> sqlx::Result<HashMap<String, Vec<Printed>>> {
        let mut tx = self.pg().space(space).await?;
        let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
            "SELECT file_id, machine_id, state, unix_seconds(COALESCE(ended_at, started_at, created_at)) AS at
             FROM jobs WHERE space_id = $1 ORDER BY at DESC, id DESC",
        )
        .bind(space)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let mut prints: HashMap<String, Vec<Printed>> = Default::default();
        for (file, machine_id, state, at) in rows {
            let list = prints.entry(file).or_default();
            if list.len() < per_file {
                list.push(Printed {
                    machine_id,
                    state: JobState::parse(&state),
                    at,
                });
            }
        }
        Ok(prints)
    }

    /// Puts a file on a machine: its job's id, or why not (the machine has a job already, or it is a
    /// laser and nobody at the laser started it: [`StartedBy`]). The check and the new job are one
    /// transaction, the machine's row locked: two at once cannot both find it free.
    pub async fn add_job(
        &self,
        space: i64,
        machine: &str,
        file: &StoredFile,
        from_line: Option<u64>,
        started: StartedBy<'_>,
    ) -> sqlx::Result<Result<i64, &'static str>> {
        let mut tx = self.pg().space(space).await?;
        let kind: Option<String> = sqlx::query_scalar(
            "SELECT kind FROM machines WHERE id = $1 AND space_id = $2 FOR UPDATE",
        )
        .bind(machine)
        .bind(space)
        .fetch_optional(&mut *tx)
        .await?;
        let laser = kind
            .and_then(|k| serde_json::from_str::<MachineKind>(&k).ok())
            .is_some_and(|k| k == MachineKind::Laser);
        if laser && !started.is_person_at_a_session() {
            return Ok(Err(LASER_NEVER_ALONE));
        }
        let by = started.name();
        let busy: bool = sqlx::query_scalar(concat!(
            "SELECT EXISTS (SELECT 1 FROM jobs WHERE machine_id = $1 AND state IN ",
            "('loaded', 'queued', 'printing', 'paused', 'stopping'))"
        ))
        .bind(machine)
        .fetch_one(&self.pg().admin)
        .await?;
        if busy {
            return Ok(Err("the machine has a job already"));
        }
        // HS7b (owner, 2026-11-17): a laser's job is loaded, not started: the person at the laser
        // frames it, sets the origin and presses Start burn (`start_burn`).
        let state = if laser { "loaded" } else { "queued" };
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO jobs (space_id, machine_id, file_id, file_name, state, created_at, from_line, by_name)
             VALUES ($1, $2, $3, $4, $5, at_unix($6), $7, $8) RETURNING id",
        )
        .bind(space)
        .bind(machine)
        .bind(&file.id)
        .bind(&file.name)
        .bind(state)
        .bind(now())
        .bind(from_line.map(|l| l as i64))
        .bind(by)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        if laser {
            // a new load is framed anew (proxy, 2026-11-17)
            self.lock().unwrap().laser_framed.remove(machine);
        }
        self.changed();
        Ok(Ok(id))
    }

    /// HS7b (ours, owner 2026-11-17: one loaded job per laser): takes a laser's loaded job off it, so
    /// another can be loaded (the person confirmed). It never ran: it ends stopped, with why.
    pub async fn unload_laser_job(&self, machine: &str) -> sqlx::Result<bool> {
        let n = sqlx::query(
            "UPDATE jobs SET state = 'stopped', ended_at = at_unix($2),
                 reason = 'replaced by another job before it was started'
             WHERE machine_id = $1 AND state = 'loaded'",
        )
        .bind(machine)
        .bind(now())
        .execute(&self.pg().admin)
        .await?
        .rows_affected();
        self.lock().unwrap().laser_framed.remove(machine);
        self.changed();
        Ok(n > 0)
    }

    /// HS7b: Start burn: the loaded job goes to the laser (its agent takes it as a queued job). Only
    /// the caller's checks decide whether it may; this only moves a loaded job on.
    pub async fn start_burn(
        &self,
        machine: &str,
        job: i64,
        by: Option<&str>,
    ) -> sqlx::Result<bool> {
        let n = sqlx::query(
            "UPDATE jobs SET state = 'queued', by_name = COALESCE($3, by_name)
             WHERE id = $1 AND machine_id = $2 AND state = 'loaded'",
        )
        .bind(job)
        .bind(machine)
        .bind(by)
        .execute(&self.pg().admin)
        .await?
        .rows_affected();
        self.changed();
        Ok(n > 0)
    }

    /// A machine's job that is not over yet.
    pub async fn active_job(&self, machine: &str) -> sqlx::Result<Option<Job>> {
        let row: Option<JobRow> = sqlx::query_as(job_select!(
            "WHERE machine_id = $1 AND state IN ('loaded', 'queued', 'printing', 'paused', 'stopping')
             ORDER BY id LIMIT 1"
        ))
        .bind(machine)
        .fetch_optional(&self.pg().admin)
        .await?;
        Ok(row.as_ref().map(JobRow::job))
    }

    pub async fn job(&self, id: i64) -> sqlx::Result<Option<Job>> {
        Ok(self.job_row(id).await?.as_ref().map(JobRow::job))
    }

    pub(crate) async fn job_row(&self, id: i64) -> sqlx::Result<Option<JobRow>> {
        sqlx::query_as(job_select!("WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pg().admin)
            .await
    }

    /// A machine's last job.
    pub(crate) async fn last_job(&self, machine: &str) -> sqlx::Result<Option<JobRow>> {
        sqlx::query_as(job_select!(
            "WHERE machine_id = $1 ORDER BY id DESC LIMIT 1"
        ))
        .bind(machine)
        .fetch_optional(&self.pg().admin)
        .await
    }

    /// Where a job that did not run to the end could start again.
    pub async fn set_restart_line(&self, id: i64, line: u64) -> sqlx::Result<()> {
        sqlx::query("UPDATE jobs SET restart_line = $1 WHERE id = $2")
            .bind(line as i64)
            .bind(id)
            .execute(&self.pg().admin)
            .await?;
        Ok(())
    }

    /// A machine's last job, when it was interrupted and says where to start again.
    pub async fn resumable_job(&self, machine: &str) -> sqlx::Result<Option<Job>> {
        Ok(self.job_facts_of(machine).await?.resumable_job(machine))
    }

    /// Whether `machine` is one of the space's.
    async fn in_space(&self, space: i64, machine: &str) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM machines WHERE id = $1 AND space_id = $2)",
        )
        .bind(machine)
        .bind(space)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(found)
    }

    /// Not ready after all (PrusaLink's `cancel_ready`): the last print counts as on its bed again,
    /// and the next job waits. Prusa's MK3 through an agent is told so (M72 S0, as PrusaLink does,
    /// whether it was ready or not). Returns whether it was ready.
    pub async fn not_ready(&self, space: i64, machine: &str) -> sqlx::Result<bool> {
        if !self.in_space(space, machine).await? {
            return Ok(false);
        }
        let Some(ready) = self.job_facts_of(machine).await?.ready_after(machine) else {
            return Ok(false);
        };
        sqlx::query("UPDATE jobs SET taken_at = NULL WHERE id = $1")
            .bind(ready.job)
            .execute(&self.pg().admin)
            .await?;
        self.changed();
        Ok(true)
    }

    /// Someone took the print off the machine's bed (a finished one, or what a failed or stopped job
    /// left): the bed is clear. Returns whether there was one.
    pub async fn print_taken(&self, space: i64, machine: &str) -> sqlx::Result<bool> {
        if !self.in_space(space, machine).await? {
            return Ok(false);
        }
        let n = sqlx::query(
            "UPDATE jobs SET taken_at = at_unix($1)
             WHERE machine_id = $2 AND state IN ('finished', 'stopped', 'failed') AND taken_at IS NULL",
        )
        .bind(now())
        .bind(machine)
        .execute(&self.pg().admin)
        .await?
        .rows_affected();
        self.changed();
        Ok(n > 0)
    }

    /// A job moves on: started, paused, asked to stop, over.
    pub async fn set_job(
        &self,
        id: i64,
        state: JobState,
        reason: Option<&str>,
    ) -> Result<(), BoxError> {
        sqlx::query(
            "UPDATE jobs SET state = $1, reason = COALESCE($2, reason),
                 started_at = CASE WHEN $1 = 'printing' AND started_at IS NULL THEN at_unix($3) ELSE started_at END,
                 ended_at = CASE WHEN $1 IN ('finished', 'stopped', 'failed') THEN at_unix($3) ELSE ended_at END
             WHERE id = $4",
        )
        .bind(state.name())
        .bind(reason)
        .bind(now())
        .bind(id)
        .execute(&self.pg().admin)
        .await?;
        if matches!(
            state,
            JobState::Finished | JobState::Stopped | JobState::Failed
        ) {
            self.count_off(id, state).await?;
        }
        self.changed();
        Ok(())
    }

    /// A job that ended comes back (a power cut stopped it and its agent kept it): running or paused
    /// again, no end, with exactly this reason (None clears it). What it counted off its spool as it
    /// failed goes back on, booked as such, so its end counts it whole.
    pub async fn revive_job(
        &self,
        id: i64,
        state: JobState,
        reason: Option<&str>,
    ) -> Result<(), BoxError> {
        let counted: Option<(Option<f64>, Option<i64>, String, String)> =
            sqlx::query_as("SELECT used, spool_id, machine_id, file_name FROM jobs WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pg().admin)
                .await?;
        if let Some((Some(used), Some(spool), machine, file_name)) = counted
            && used > 0.0
        {
            let legacy = self.lock().unwrap();
            legacy.db.execute(
                "UPDATE spool SET used_weight = MAX(used_weight - ?1, 0) WHERE id = ?2",
                params![used, spool],
            )?;
            let space: Option<i64> = legacy
                .db
                .query_row("SELECT space_id FROM spool WHERE id = ?1", [spool], |r| {
                    r.get(0)
                })
                .optional()?;
            if let Some(space) = space {
                legacy.movement(
                    space,
                    &crate::stock::spool_code(spool),
                    "used",
                    Some(used),
                    Some(&machine),
                    Some(&format!("job {file_name}: taken up after a power cut")),
                )?;
            }
        }
        sqlx::query(
            "UPDATE jobs SET state = $1, reason = $2, ended_at = NULL, used = NULL, spool_id = NULL
             WHERE id = $3",
        )
        .bind(state.name())
        .bind(reason)
        .bind(id)
        .execute(&self.pg().admin)
        .await?;
        self.changed();
        Ok(())
    }

    /// A job that ended counts what it used off the spool loaded in its machine: all of what its file
    /// says when it finished, the part it got through when it stopped. Not when something else counted
    /// while it ran (Moonraker's `[spoolman]` does, as it prints).
    async fn count_off(&self, id: i64, state: JobState) -> Result<(), BoxError> {
        let Some(job) = self.job(id).await? else {
            return Ok(());
        };
        // The spool, and whether something counted already (the stock's, on SQLite).
        let found: Option<(i64, i64, f64, f64, Option<String>)> = {
            let legacy = self.lock().unwrap();
            let found: Option<(i64, i64, f64, f64, Option<String>)> = legacy
                .db
                .query_row(
                    "SELECT s.id, s.space_id, f.density, f.diameter, f.unit FROM spool s
                     JOIN filament f ON f.id = s.filament_id
                     WHERE s.machine = ?1 AND s.archived = 0",
                    [&job.machine_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()?;
            match found {
                Some((spool, ..)) => {
                    let counted_already: bool = legacy.db.query_row(
                        "SELECT COUNT(*) > 0 FROM movements WHERE item = ?1 AND kind = 'used' AND note IS NULL AND at >= ?2",
                        params![crate::stock::spool_code(spool), job.started_at.unwrap_or(job.created_at)],
                        |r| r.get(0),
                    )?;
                    found.filter(|_| !counted_already)
                }
                None => None,
            }
        };
        let Some((spool, space, density, diameter, unit)) = found else {
            return Ok(());
        };
        // The file, and the machine as its space has it: Postgres.
        let file: Option<String> =
            sqlx::query_scalar("SELECT meta FROM files WHERE space_id = $1 AND id = $2")
                .bind(space)
                .bind(&job.file_id)
                .fetch_optional(&self.pg().admin)
                .await?;
        let Some(meta) = file else {
            return Ok(());
        };
        let meta: crate::gcode::FileMeta = serde_json::from_str(&meta).unwrap_or_default();
        let machine: Option<(Option<String>, Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT profile, unix_seconds(last_seen), telemetry FROM machines WHERE id = $1 AND space_id = $2",
        )
        .bind(&job.machine_id)
        .bind(space)
        .fetch_optional(&self.pg().admin)
        .await?;
        let (profile, telemetry) = match machine {
            Some((profile, last_seen, telemetry)) => {
                // As the machine shows: its report only while it is online.
                let online = last_seen.is_some_and(|t| now() - t <= ONLINE_SECONDS);
                let telemetry: Option<Telemetry> = telemetry
                    .filter(|_| online)
                    .and_then(|t| serde_json::from_str(&t).ok());
                (profile, telemetry)
            }
            None => (None, None),
        };
        let unit = unit.unwrap_or_else(|| "g".to_owned());
        // A die-cut label: one off the roll, also when it went wrong half-way (it is spoilt).
        // A label on tape: its length, and a lead before and after.
        let whole = if unit == "labels" {
            meta.label_dots.map(|_| 1.0)
        } else if unit == "m" {
            let dpi = profile
                .and_then(|p| crate::profiles::find(&p))
                .and_then(|p| p.dpi)
                .unwrap_or(180.0);
            meta.label_dots
                .map(|[w, _]| (f64::from(w) / dpi * 25.4 + 16.0) / 1000.0)
        } else {
            meta.filament_g.or_else(|| {
                // Metres as grams: the length's volume times the density.
                let area = std::f64::consts::PI * (diameter / 2.0).powi(2);
                meta.filament_m
                    .map(|m| m * 1000.0 * area * density / 1000.0)
            })
        };
        let Some(whole) = whole.filter(|w| *w > 0.0) else {
            return Ok(());
        };
        let share = match state {
            JobState::Finished => 1.0,
            // As far as it got: its last progress.
            _ => telemetry
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
        {
            let legacy = self.lock().unwrap();
            let t = now();
            legacy.db.execute(
                "UPDATE spool SET used_weight = MAX(used_weight + ?1, 0), first_used = COALESCE(first_used, ?2),
                     last_used = ?2 WHERE id = ?3",
                params![used, t, spool],
            )?;
            legacy.movement(
                space,
                &crate::stock::spool_code(spool),
                "used",
                Some(-used),
                Some(&job.machine_id),
                Some(&format!("job {}", job.file_name)),
            )?;
            legacy.spoolman_event("spool", "updated", spool);
        }
        sqlx::query("UPDATE jobs SET used = $1, spool_id = $2 WHERE id = $3")
            .bind(used)
            .bind(spool)
            .bind(id)
            .execute(&self.pg().admin)
            .await?;
        Ok(())
    }

    /// A machine's latest jobs, newest first.
    pub async fn jobs_of(&self, space: i64, machine: &str, limit: usize) -> sqlx::Result<Vec<Job>> {
        let mut tx = self.pg().space(space).await?;
        let rows: Vec<JobRow> = sqlx::query_as(job_select!(
            "WHERE space_id = $1 AND machine_id = $2 ORDER BY created_at DESC, id DESC LIMIT $3"
        ))
        .bind(space)
        .bind(machine)
        .bind(limit as i64)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows.iter().map(JobRow::job).collect())
    }

    /// Parts that people confirmed good from a machine's plates.
    pub async fn parts_made(&self, machine: &str) -> sqlx::Result<i64> {
        sqlx::query_scalar(
            "SELECT COALESCE(SUM(confirmed_good), 0)::bigint FROM jobs WHERE machine_id = $1",
        )
        .bind(machine)
        .fetch_one(&self.pg().admin)
        .await
    }

    /// A machine's ended jobs in numbers.
    pub async fn job_stats(&self, space: i64, machine: &str) -> sqlx::Result<JobStats> {
        let mut stats = JobStats {
            parts: self.parts_made(machine).await?,
            ..Default::default()
        };
        let mut tx = self.pg().space(space).await?;
        type Ended = (String, Option<i64>, Option<i64>, Option<String>);
        let rows: Vec<Ended> = sqlx::query_as(
            "SELECT j.state, unix_seconds(j.started_at), unix_seconds(j.ended_at), f.meta
             FROM jobs j LEFT JOIN files f ON f.id = j.file_id
             WHERE j.space_id = $1 AND j.machine_id = $2 AND j.ended_at IS NOT NULL",
        )
        .bind(space)
        .bind(machine)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        for (state, started, ended, meta) in rows {
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

    /// How the space's jobs ended since `since` (Unix seconds): finished, failed, stopped.
    pub async fn ended_since(&self, space: i64, since: i64) -> sqlx::Result<(i64, i64, i64)> {
        let mut tx = self.pg().space(space).await?;
        let counted = sqlx::query_as(
            "SELECT COUNT(*) FILTER (WHERE state = 'finished'), COUNT(*) FILTER (WHERE state = 'failed'),
                    COUNT(*) FILTER (WHERE state = 'stopped')
             FROM jobs WHERE space_id = $1 AND ended_at >= at_unix($2)",
        )
        .bind(space)
        .bind(since)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(counted)
    }

    /// Jobs printing or paused, in every space (the About page: whether the hub may update now).
    pub async fn printing_anywhere(&self) -> sqlx::Result<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE state IN ('printing', 'paused')")
            .fetch_one(&self.pg().admin)
            .await
    }

    /// Whether a machine runs a file (or has it queued).
    pub(crate) async fn file_in_use(&self, id: &str) -> sqlx::Result<bool> {
        sqlx::query_scalar(concat!(
            "SELECT EXISTS (SELECT 1 FROM jobs WHERE file_id = $1 AND state IN ",
            "('loaded', 'queued', 'printing', 'paused', 'stopping'))"
        ))
        .bind(id)
        .fetch_one(&self.pg().admin)
        .await
    }
}
