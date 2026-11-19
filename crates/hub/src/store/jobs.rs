use super::*;

impl Legacy {
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

    /// Puts a file on a machine: its job's id, or why not (the machine has a job already, or it
    /// is a laser and nobody at the laser started it: [`StartedBy`]).
    pub fn add_job(
        &mut self,
        space: i64,
        machine: &str,
        file: &StoredFile,
        from_line: Option<u64>,
        started: StartedBy,
    ) -> rusqlite::Result<Result<i64, &'static str>> {
        // The machine's kind, from the SQLite copy (transitional, lane 3a: jobs move with lane 3b).
        let laser = self
            .db
            .query_row(
                "SELECT kind FROM machines WHERE id = ?1 AND space_id = ?2",
                params![machine, space],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|k| serde_json::from_str::<MachineKind>(&k).ok())
            .is_some_and(|k| k == MachineKind::Laser);
        if laser && !started.is_person_at_a_session() {
            return Ok(Err(LASER_NEVER_ALONE));
        }
        let by = started.name();
        if self.active_job(machine)?.is_some() {
            return Ok(Err("the machine has a job already"));
        }
        // HS7b (owner, 2026-11-17): a laser's job is loaded, not started: the person at the laser
        // frames it, sets the origin and presses Start burn (`start_burn`).
        let state = if laser { "loaded" } else { "queued" };
        if laser {
            // a new load is framed anew (proxy, 2026-11-17)
            self.laser_framed.remove(machine);
        }
        self.db.execute(
            "INSERT INTO jobs (space_id, machine_id, file_id, file_name, state, created_at, from_line, by_name)
             VALUES (?1, ?2, ?3, ?4, ?8, ?5, ?6, ?7)",
            params![space, machine, file.id, file.name, now(), from_line.map(|l| l as i64), by, state],
        )?;
        self.changed();
        Ok(Ok(self.db.last_insert_rowid()))
    }

    /// HS7b (ours, owner 2026-11-17: one loaded job per laser): takes a laser's loaded job off it,
    /// so another can be loaded (the person confirmed). It never ran: it ends stopped, with why.
    pub fn unload_laser_job(&mut self, machine: &str) -> rusqlite::Result<bool> {
        let n = self.db.execute(
            "UPDATE jobs SET state = 'stopped', ended_at = ?2, reason = 'replaced by another job before it was started'
             WHERE machine_id = ?1 AND state = 'loaded'",
            params![machine, now()],
        )?;
        self.laser_framed.remove(machine);
        self.changed();
        Ok(n > 0)
    }

    /// HS7b: Start burn: the loaded job goes to the laser (its agent takes it as a queued job).
    /// Only the caller's checks decide whether it may; this only moves a loaded job on.
    pub fn start_burn(
        &mut self,
        machine: &str,
        job: i64,
        by: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let n = self.db.execute(
            "UPDATE jobs SET state = 'queued', by_name = COALESCE(?3, by_name)
             WHERE id = ?1 AND machine_id = ?2 AND state = 'loaded'",
            params![job, machine, by],
        )?;
        self.changed();
        Ok(n > 0)
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
                 FROM jobs WHERE machine_id = ?1 AND state IN ('loaded', 'queued', 'printing', 'paused', 'stopping')
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
            .filter(|(_, state, taken)| state == "finished" && taken.is_some())
            .map(|(r, _, _)| r))
    }

    /// Not ready after all (PrusaLink's `cancel_ready`): the last print counts as on its bed
    /// again, and the next job waits. Prusa's MK3 through an agent is told so (M72 S0, as
    /// PrusaLink does, whether it was ready or not). Returns whether it was ready.
    pub fn not_ready(&mut self, space: i64, machine: &str) -> rusqlite::Result<bool> {
        let Some(_) = self.machine_on_mirror(space, machine)? else {
            return Ok(false);
        };
        let Some(ready) = self.ready_after(machine)? else {
            return Ok(false);
        };
        self.db
            .execute("UPDATE jobs SET taken_at = NULL WHERE id = ?1", [ready.job])?;
        self.changed();
        Ok(true)
    }

    /// Someone took the print off the machine's bed (a finished one, or what a failed or
    /// stopped job left): the bed is clear. Returns whether there was one.
    pub fn print_taken(&mut self, space: i64, machine: &str) -> rusqlite::Result<bool> {
        if self.machine_on_mirror(space, machine)?.is_none() {
            return Ok(false);
        }
        let n = self.db.execute(
            "UPDATE jobs SET taken_at = ?1 WHERE machine_id = ?2 AND state IN ('finished', 'stopped', 'failed') AND taken_at IS NULL",
            params![now(), machine],
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
                .query_row("SELECT space_id FROM spool WHERE id = ?1", [spool], |r| {
                    r.get(0)
                })
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
        let Some(file) = self.file_on_mirror(space, &job.file_id)? else {
            return Ok(());
        };
        let unit = unit.unwrap_or_else(|| "g".to_owned());
        // A die-cut label: one off the roll, also when it went wrong half-way (it is spoilt).
        // A label on tape: its length, and a lead before and after.
        let whole = if unit == "labels" {
            file.meta.label_dots.map(|_| 1.0)
        } else if unit == "m" {
            let dpi = self
                .machine_on_mirror(space, &job.machine_id)?
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
                .machine_on_mirror(space, &job.machine_id)?
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
}
