//! The farm: projects of parts, each part made on whichever machine suits it, many machines
//! working on one project; people confirm what came off the bed. The design is the wiki's
//! *Print Farm*; much is learned from print-farm-manager by Joel Telling (MIT):
//!
//! - A **project** has **parts**: how many are needed (`target`) and how many are made
//!   (`done`). A part has a file per machine model (a profile) with how many parts one plate
//!   holds, and the material or colour it needs, if any.
//! - The **dispatcher** gives a free machine the next plate it suits: the part open, its
//!   project active, a file for the machine's profile, the material and colour loaded as
//!   needed; by project priority, then age, then the parts' order. Whole plates only: still
//!   needed are `ceil((target − done) / per plate)` less the plates under way (on a bed, or
//!   waiting to be confirmed).
//! - A machine takes work from the queue only when it is set to (`farm`), so machines used
//!   by hand are never sent work by surprise.
//! - **People confirm, machines do not assume.** A plate that ended keeps its machine
//!   waiting (held) until someone says how many parts are good (*Set Ready*) or that it
//!   failed (*Bad print*). Only that counts: never a time window, never a status seen after
//!   a restart (print-farm-manager's "phantom credit"). Code never releases a held machine
//!   by itself.
//! - **The ledger:** `done` changes in [`Work::adjust_part`] only, with one ledger row each
//!   time; a test reads the source and fails if anything else writes it.

use crate::access::{Access, Level, Role};
use crate::store::pg::Bypass;
use crate::store::{BoxError, JobFacts, JobState, StoredFile, Work, now};
use crate::{Store, failed};
use axum::{
    Form, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How often the dispatcher looks for free machines.
const DISPATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub description: String,
    /// `draft`, `active`, `paused`, `done`.
    pub state: String,
    pub priority: i64,
    pub created_at: i64,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Part {
    pub id: i64,
    pub name: String,
    pub target: i64,
    pub done: i64,
    /// Plates on a bed now, or waiting for someone, and the parts they hold.
    pub plates_under_way: i64,
    pub parts_under_way: i64,
    pub files: Vec<PartFile>,
    /// Why no plate goes out now, in words; `None` when one could, or none is needed.
    pub waiting: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PartFile {
    pub id: i64,
    pub profile: String,
    pub file_id: String,
    pub file_name: String,
    pub per_plate: i64,
    pub material: Option<String>,
    pub color: Option<String>,
    /// The kind of build plate it needs (`Textured PEI`); none: any.
    pub sheet: Option<String>,
}

/// A plate that ended and waits for someone to confirm it, on its machine.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Confirm {
    pub job: i64,
    pub file_name: String,
    pub state: String,
    pub part: String,
    pub project: String,
    pub per_plate: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LedgerLine {
    pub at: i64,
    pub delta: i64,
    pub done_after: i64,
    pub source: String,
    pub job_id: Option<i64>,
    pub actor: String,
    pub note: String,
}

/// The parts' files' profiles, materials and colours a machine must match, from a part file.
struct Candidate {
    part: i64,
    part_file: i64,
    file_id: String,
    per_plate: i64,
}

impl Work {
    /// The space's projects, newest first within their priority, with their parts.
    /// `machines` and `files`: the space's, read before.
    pub(crate) async fn projects(
        &mut self,
        jobs: &JobFacts,
        machines: &[crate::store::Machine],
        files: &[StoredFile],
    ) -> sqlx::Result<Vec<Project>> {
        let rows: Vec<(i64, String, String, String, i64, i64)> = sqlx::query_as(
            "SELECT id, name, description, state, priority, unix_seconds(created_at) FROM projects
             WHERE space_id = $1 ORDER BY CASE state WHEN 'active' THEN 0 WHEN 'paused' THEN 1 WHEN 'draft' THEN 2 ELSE 3 END,
                 priority, created_at",
        )
        .bind(self.space)
        .fetch_all(self.c())
        .await?;
        let mut projects: Vec<Project> = rows
            .into_iter()
            .map(
                |(id, name, description, state, priority, created_at)| Project {
                    id,
                    name,
                    description,
                    state,
                    priority,
                    created_at,
                    parts: Vec::new(),
                },
            )
            .collect();
        for project in &mut projects {
            project.parts = self
                .parts_of(project.id, &project.state, jobs, machines, files)
                .await?;
        }
        Ok(projects)
    }

    async fn parts_of(
        &mut self,
        project: i64,
        state: &str,
        jobs: &JobFacts,
        machines: &[crate::store::Machine],
        files: &[StoredFile],
    ) -> sqlx::Result<Vec<Part>> {
        let rows: Vec<(i64, String, i64, i64)> = sqlx::query_as(
            "SELECT id, name, target, done FROM parts WHERE space_id = $1 AND project_id = $2
             ORDER BY sort_order, created_at, id",
        )
        .bind(self.space)
        .bind(project)
        .fetch_all(self.c())
        .await?;
        let mut parts = Vec::new();
        for (id, name, target, done) in rows {
            let (plates, held) = under_way(jobs, id);
            let part_files = self.part_files(id, files).await?;
            let mut part = Part {
                id,
                name,
                target,
                done,
                plates_under_way: plates,
                parts_under_way: held,
                files: part_files,
                waiting: None,
            };
            part.waiting = why_waiting(state, &part, machines);
            parts.push(part);
        }
        Ok(parts)
    }

    /// A part's files; `files`: the space's, for their names.
    async fn part_files(&mut self, part: i64, files: &[StoredFile]) -> sqlx::Result<Vec<PartFile>> {
        type Row = (
            i64,
            String,
            String,
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT pf.id, pf.profile, pf.file_id, pf.per_plate, pf.material, pf.color, pf.sheet
             FROM part_files pf WHERE pf.space_id = $1 AND pf.part_id = $2 ORDER BY pf.profile COLLATE \"C\"",
        )
        .bind(self.space)
        .bind(part)
        .fetch_all(self.c())
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, profile, file_id, per_plate, material, color, sheet)| {
                    let file_name = files
                        .iter()
                        .find(|f| f.id == file_id)
                        .map_or_else(|| "(file removed)".to_owned(), |f| f.name.clone());
                    PartFile {
                        id,
                        profile,
                        file_id,
                        file_name,
                        per_plate,
                        material,
                        color,
                        sheet,
                    }
                },
            )
            .collect())
    }

    /// The plates that ended on these machines and wait for someone ([`Work::waiting_confirm`]),
    /// by machine id.
    pub(crate) async fn confirms(
        &mut self,
        jobs: &JobFacts,
        machines: &[String],
    ) -> sqlx::Result<std::collections::HashMap<String, Confirm>> {
        let mut out = std::collections::HashMap::new();
        for machine in machines {
            if let Some(confirm) = self.waiting_confirm(jobs, machine).await? {
                out.insert(machine.clone(), confirm);
            }
        }
        Ok(out)
    }

    /// The plate that ended on a machine and waits for someone, if any: its last job, when it
    /// was for a part and is not yet confirmed.
    /// The job is the jobs' ([`JobFacts::waiting_plate`]); the part's and project's names are here.
    pub(crate) async fn waiting_confirm(
        &mut self,
        jobs: &JobFacts,
        machine: &str,
    ) -> sqlx::Result<Option<Confirm>> {
        let Some(job) = jobs.waiting_plate(machine) else {
            return Ok(None);
        };
        let (part, project) = match job.part_id {
            Some(part) => sqlx::query_as::<_, (String, Option<String>)>(
                "SELECT p.name, pr.name FROM parts p LEFT JOIN projects pr ON pr.id = p.project_id
                 WHERE p.id = $1",
            )
            .bind(part)
            .fetch_optional(self.c())
            .await?
            .map(|(p, pr)| (p, pr.unwrap_or_default()))
            .unwrap_or_default(),
            None => Default::default(),
        };
        let job_ = job.job();
        Ok(Some(Confirm {
            job: job_.id,
            file_name: job_.file_name,
            state: job_.state.name(),
            part,
            project,
            per_plate: job.per_plate.unwrap_or(1),
        }))
    }

    /// The only way `done` changes: by `delta` (or to `set`), with a line in the ledger saying
    /// why and who. Returns the new count.
    pub async fn adjust_part(
        &mut self,
        part: i64,
        change: PartChange,
        why: Why<'_>,
    ) -> sqlx::Result<Option<i64>> {
        let Why {
            source,
            job,
            actor,
            note,
        } = why;
        let Some(done) = sqlx::query_scalar::<_, i64>(
            "SELECT done FROM parts WHERE id = $1 AND space_id = $2 FOR UPDATE",
        )
        .bind(part)
        .bind(self.space)
        .fetch_optional(self.c())
        .await?
        else {
            return Ok(None);
        };
        let after = match change {
            PartChange::By(delta) => (done + delta).max(0),
            PartChange::To(value) => value.max(0),
        };
        sqlx::query("UPDATE parts SET done = $2 WHERE id = $1")
            .bind(part)
            .bind(after)
            .execute(self.c())
            .await?;
        sqlx::query(
            "INSERT INTO part_ledger (space_id, part_id, at, delta, done_after, source, job_id, actor, note)
             VALUES ($1, $2, at_unix($3), $4, $5, $6, $7, $8, $9)",
        )
        .bind(self.space)
        .bind(part)
        .bind(now())
        .bind(after - done)
        .bind(after)
        .bind(source)
        .bind(job)
        .bind(actor)
        .bind(note)
        .execute(self.c())
        .await?;
        self.changed();
        Ok(Some(after))
    }

    pub(crate) async fn ledger(&mut self, part: i64) -> sqlx::Result<Vec<LedgerLine>> {
        type Row = (i64, i64, i64, String, Option<i64>, String, String);
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT unix_seconds(at), delta, done_after, source, job_id, actor, note FROM part_ledger
             WHERE space_id = $1 AND part_id = $2 ORDER BY at DESC, id DESC LIMIT 200",
        )
        .bind(self.space)
        .bind(part)
        .fetch_all(self.c())
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(at, delta, done_after, source, job_id, actor, note)| LedgerLine {
                    at,
                    delta,
                    done_after,
                    source,
                    job_id,
                    actor,
                    note,
                },
            )
            .collect())
    }

    /// The next plate a free machine suits, if any: the rule in one place, for the dispatcher
    /// (nothing planned) and the schedule (plates it has planned already, per part).
    async fn candidate_with(
        &mut self,
        machine: &crate::store::Machine,
        planned: &std::collections::HashMap<i64, i64>,
        jobs: &JobFacts,
        files: &[StoredFile],
    ) -> sqlx::Result<Option<Candidate>> {
        let Some(profile) = machine.profile.as_deref() else {
            return Ok(None);
        };
        type Row = (
            i64,
            i64,
            String,
            i64,
            i64,
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT p.id, pf.id, pf.file_id, pf.per_plate, p.target, p.done, pf.material, pf.color, pf.sheet
             FROM parts p JOIN projects pr ON pr.id = p.project_id
             JOIN part_files pf ON pf.part_id = p.id AND pf.profile = $2
             WHERE p.space_id = $1 AND pr.state = 'active'
             ORDER BY pr.priority, pr.created_at, p.sort_order, p.created_at, p.id",
        )
        .bind(self.space)
        .bind(profile)
        .fetch_all(self.c())
        .await?;
        let rows = rows.into_iter().map(
            |(part, id, file_id, per_plate, target, done, material, color, sheet)| {
                let per_plate = per_plate.max(1);
                (
                    part,
                    per_plate,
                    target,
                    done,
                    PartFile {
                        id,
                        profile: profile.to_owned(),
                        file_id,
                        file_name: String::new(),
                        per_plate,
                        material,
                        color,
                        sheet,
                    },
                )
            },
        );
        // Only files the space still has.
        for (part, per_plate, target, done, file) in
            rows.filter(|(.., f)| files.iter().any(|kept| kept.id == f.file_id))
        {
            let (_, under_way) = under_way(jobs, part);
            // Whole plates: still needed, less what is on a bed, waits to be confirmed, or
            // is planned already.
            if done + under_way + planned.get(&part).copied().unwrap_or(0) >= target {
                continue;
            }
            if !fits(&file, machine) {
                continue;
            }
            return Ok(Some(Candidate {
                part,
                part_file: file.id,
                file_id: file.file_id,
                per_plate,
            }));
        }
        Ok(None)
    }
}

/// Why a part sends no plate out now, in words (print-farm-manager's dispatch status).
fn why_waiting(
    project_state: &str,
    part: &Part,
    machines: &[crate::store::Machine],
) -> Option<String> {
    if part.done + part.parts_under_way >= part.target {
        return None;
    }
    if project_state != "active" {
        return Some(format!("the project is {project_state}"));
    }
    if part.files.is_empty() {
        return Some("no file for any machine yet".to_owned());
    }
    let farm: Vec<_> = machines.iter().filter(|m| m.takes_work).collect();
    let fitting: Vec<_> = farm
        .iter()
        .filter(|m| {
            part.files
                .iter()
                .any(|f| m.profile.as_deref() == Some(f.profile.as_str()))
        })
        .collect();
    if fitting.is_empty() {
        let models: Vec<String> = part
            .files
            .iter()
            .map(|f| {
                crate::profiles::find(&f.profile).map_or(f.profile.clone(), |p| p.model.clone())
            })
            .collect();
        return Some(format!(
            "no machine in the queue is a {}",
            models.join(" or ")
        ));
    }
    let loaded: Vec<_> = fitting
        .iter()
        .filter(|m| {
            part.files.iter().any(|f| {
                m.profile.as_deref() == Some(f.profile.as_str())
                    && material_fits(f, m.loaded.as_ref())
            })
        })
        .collect();
    if loaded.is_empty() {
        let wants: Vec<String> = part
            .files
            .iter()
            .filter_map(|f| f.material.clone().or_else(|| f.color.clone()))
            .collect();
        return Some(format!("nothing with {} loaded", wants.join(" or ")));
    }
    let plated: Vec<_> = loaded
        .iter()
        .filter(|m| {
            part.files
                .iter()
                .any(|f| m.profile.as_deref() == Some(f.profile.as_str()) && fits(f, m))
        })
        .collect();
    if plated.is_empty() {
        let sheets: Vec<String> = part.files.iter().filter_map(|f| f.sheet.clone()).collect();
        return Some(format!("no machine has a {} plate on", sheets.join(" or ")));
    }
    if plated.iter().all(|m| !m.online) {
        return Some("its machines are offline".to_owned());
    }
    Some("its machines are busy, or wait for someone".to_owned())
}

/// Plates of a part under way (on a bed, or ended and not yet confirmed), and the parts they hold.
fn under_way(jobs: &JobFacts, part: i64) -> (i64, i64) {
    jobs.under_way.get(&part).copied().unwrap_or((0, 0))
}

impl Store {
    /// The space's projects, newest first within their priority, with their parts.
    pub async fn projects(&self, space: i64) -> Result<Vec<Project>, BoxError> {
        let jobs = self.job_facts(space, &[]).await?;
        let machines = self.machines(space).await?;
        let files = self.files(space).await?;
        Ok(crate::stock::in_space!(self, space, |w| w.projects(&jobs, &machines, &files)).await?)
    }

    /// The schedule (`Work::schedule`) of a space's machines as they are now.
    pub async fn schedule(&self, space: i64) -> Result<Vec<Lane>, BoxError> {
        let (machines, jobs) = self.machines_and_jobs(space).await?;
        let files = self.files(space).await?;
        Ok(crate::stock::in_space!(self, space, |w| w.schedule(machines, &jobs, &files)).await?)
    }

    /// A part's ledger, newest first (`Work::ledger`).
    pub async fn ledger(&self, space: i64, part: i64) -> sqlx::Result<Vec<LedgerLine>> {
        crate::stock::in_space!(self, space, |w| w.ledger(part)).await
    }

    /// Someone confirms the plate waiting on a machine: `good` parts are good (0: a bad
    /// print). Counts it once, releases the machine. Refused when nothing waits there, or
    /// when what waits is another job than the one the page showed.
    pub async fn confirm_plate(
        &self,
        access: &Access,
        machine: &str,
        job: i64,
        good: i64,
        note: &str,
    ) -> Result<Result<i64, &'static str>, BoxError> {
        let jobs = self.job_facts_of(access.space, machine).await?;
        let Some(waiting) = jobs.waiting_plate(machine) else {
            return Ok(Err("nothing waits to be confirmed on this machine"));
        };
        if waiting.job().id != job {
            return Ok(Err("another plate waits there now: reload the page"));
        }
        let part = waiting.part_id.unwrap_or_default();
        let good = good.clamp(0, 10_000);
        // Confirmed first, in the same breath as the count: a second click finds it done.
        let mut tx = self.pg().space(access.space).await?;
        let n = sqlx::query(
            "UPDATE jobs SET confirmed_at = at_unix($2), confirmed_good = $3, confirmed_by = $4,
                 taken_at = COALESCE(taken_at, at_unix($2))
             WHERE id = $1 AND confirmed_at IS NULL",
        )
        .bind(job)
        .bind(now())
        .bind(good)
        .bind(&access.actor)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        if n == 0 {
            return Ok(Err("it was confirmed meanwhile"));
        }
        let source = if good == 0 { "bad print" } else { "confirmed" };
        let why = Why {
            source,
            job: Some(job),
            actor: &access.actor,
            note,
        };
        let after = crate::stock::in_space!(self, access.space, |w| w.adjust_part(
            part,
            PartChange::By(good),
            why
        ))
        .await?
        .unwrap_or(0);
        Ok(Ok(after))
    }

    /// One round of the dispatcher, over every space: each machine that takes work, online,
    /// free, in use, with a clear bed and nothing waiting for someone, gets its next plate.
    pub async fn dispatch(&self) -> Result<usize, BoxError> {
        let spaces: Vec<i64> = sqlx::query_scalar(
            "SELECT DISTINCT space_id FROM machines WHERE farm AND space_id IS NOT NULL ORDER BY space_id",
        )
        .fetch_all(self.pg().admin(Bypass::TaskList))
        .await?;
        let mut sent = 0;
        for space in spaces {
            let (machines, mut jobs) = self.machines_and_jobs(space).await?;
            let files = self.files(space).await?;
            for machine in machines {
                // A laser starts only when someone at the laser starts it.
                if machine.kind == maker_connect_protocol::MachineKind::Laser {
                    continue;
                }
                // A queue of its own (crate::queue) goes first.
                if !machine.queue.is_empty() {
                    continue;
                }
                let next = {
                    if !machine.takes_work || !machine.online || machine.out_of_use.is_some() {
                        continue;
                    }
                    let idle = machine
                        .telemetry
                        .as_ref()
                        .is_some_and(|t| t.state == maker_connect_protocol::MachineState::Idle);
                    if !idle
                        || machine.job.is_some()
                        || machine.on_bed.is_some()
                        || machine.upkeep.as_ref().is_some_and(|u| u.due)
                        || jobs.waiting_plate(&machine.id).is_some()
                    {
                        continue;
                    }
                    let mut w = self.work(space).await?;
                    let Some(c) = w
                        .candidate_with(&machine, &Default::default(), &jobs, &files)
                        .await?
                    else {
                        continue;
                    };
                    let by = match w.part_names(c.part).await? {
                        (_, Some(project)) => format!("queue · {project}"),
                        _ => "queue".to_owned(),
                    };
                    w.commit(self).await?;
                    (c, by)
                };
                let (c, by) = next;
                let Some(file) = self.file(space, &c.file_id).await? else {
                    continue;
                };
                if crate::files::fit(&file, &machine).is_err() {
                    continue;
                }
                if let Ok(job) = self
                    .add_job(
                        space,
                        &machine.id,
                        &file,
                        None,
                        crate::store::StartedBy::Auto { by: Some(&by) },
                    )
                    .await?
                {
                    let mut tx = self.pg().space(space).await?;
                    sqlx::query("UPDATE jobs SET part_id = $2, per_plate = $3 WHERE id = $1")
                        .bind(job)
                        .bind(c.part)
                        .bind(c.per_plate)
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await?;
                    jobs.sent(c.part, c.per_plate);
                    let _ = c.part_file;
                    println!(
                        "dispatched {} to {} (part {})",
                        file.name, machine.name, c.part
                    );
                    sent += 1;
                }
            }
        }
        if sent > 0 {
            self.changed();
        }
        Ok(sent)
    }
}

/// A machine's upkeep: a service every so many printing hours.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Upkeep {
    pub every_h: f64,
    /// Printing hours since the last service (or since it came, when never serviced).
    pub since_h: f64,
    pub due: bool,
}

/// Why `done` changes, for the ledger: what (`confirmed`, `bad print`, `corrected`), the
/// job it came from, who, and a note.
pub struct Why<'a> {
    pub source: &'a str,
    pub job: Option<i64>,
    pub actor: &'a str,
    pub note: &'a str,
}

/// A block on the schedule: a job running, a plate planned, or a machine waiting for
/// someone; from `start` to `end` (Unix seconds).
#[derive(Debug, Clone, Serialize)]
pub struct Block {
    pub start: i64,
    pub end: i64,
    /// `running`, `planned`, `waiting`.
    pub kind: &'static str,
    pub label: String,
    pub part: Option<String>,
    pub project: Option<String>,
}

/// A machine's row on the schedule; a note instead when it is not planned (offline, due for
/// its service, out of use).
#[derive(Debug, Clone, Serialize)]
pub struct Lane {
    pub machine: String,
    pub name: String,
    pub blocks: Vec<Block>,
    pub note: Option<String>,
}

/// How far ahead the schedule looks, and how many plates it plans at most.
const SCHEDULE_HORIZON: i64 = 48 * 3600;
const SCHEDULE_PLATES: usize = 60;
/// A plate whose file says nothing of its time is planned as this long.
const UNKNOWN_PLATE_SECONDS: i64 = 3600;

impl Work {
    /// The names of a part and its project.
    async fn part_names(&mut self, part: i64) -> sqlx::Result<(Option<String>, Option<String>)> {
        Ok(sqlx::query_as::<_, (String, String)>(
            "SELECT p.name, pr.name FROM parts p JOIN projects pr ON pr.id = p.project_id WHERE p.id = $1",
        )
        .bind(part)
        .fetch_optional(self.c())
        .await?
        .map_or((None, None), |(p, pr)| (Some(p), Some(pr))))
    }

    /// What the machines that take work will print, and when: worked out from the jobs running
    /// and the dispatcher's own rule (`Work::candidate_with`), without changing anything.
    /// print-farm-manager's projection: a schedule that disagrees with dispatch is worse than
    /// none, so both use the same rule.
    pub(crate) async fn schedule(
        &mut self,
        machines: Vec<crate::store::Machine>,
        jobs: &JobFacts,
        files: &[StoredFile],
    ) -> sqlx::Result<Vec<Lane>> {
        let file_of = |id: &str| files.iter().find(|f| f.id == id);
        let at = now();
        let mut lanes = Vec::new();
        // Per lane: when it is free, and whether it can be planned.
        let mut free: Vec<(usize, i64, crate::store::Machine)> = Vec::new();
        for machine in machines {
            if !machine.takes_work {
                continue;
            }
            let mut lane = Lane {
                machine: machine.id.clone(),
                name: machine.name.clone(),
                blocks: Vec::new(),
                note: None,
            };
            let mut free_at = at;
            if let Some(job) = &machine.job {
                let left = machine
                    .telemetry
                    .as_ref()
                    .and_then(|t| t.job.as_ref())
                    .and_then(|j| j.remaining_s)
                    .map(|s| s as i64)
                    .or_else(|| {
                        let file = file_of(&job.file_id)?;
                        let whole = file.meta.estimated_s? as i64;
                        Some((whole - (at - job.started_at.unwrap_or(at))).max(60))
                    })
                    .unwrap_or(UNKNOWN_PLATE_SECONDS);
                let part: Option<i64> = jobs.active_part(&machine.id);
                let (part, project) = match part {
                    Some(p) => self.part_names(p).await?,
                    None => (None, None),
                };
                lane.blocks.push(Block {
                    start: job.started_at.unwrap_or(at),
                    end: at + left,
                    kind: "running",
                    label: job.file_name.clone(),
                    part,
                    project,
                });
                free_at = at + left;
            } else if machine.confirm.is_some() || machine.on_bed.is_some() {
                lane.blocks.push(Block {
                    start: at,
                    end: at,
                    kind: "waiting",
                    label: "waits for someone".to_owned(),
                    part: machine.confirm.as_ref().map(|c| c.part.clone()),
                    project: None,
                });
            }
            lane.note = if machine.out_of_use.is_some() {
                Some("out of use".to_owned())
            } else if !machine.online {
                Some("offline".to_owned())
            } else if machine.upkeep.as_ref().is_some_and(|u| u.due) {
                Some("service due".to_owned())
            } else {
                None
            };
            if lane.note.is_none() {
                free.push((lanes.len(), free_at, machine));
            }
            lanes.push(lane);
        }
        let mut planned: std::collections::HashMap<i64, i64> = Default::default();
        let mut plates = 0;
        while plates < SCHEDULE_PLATES {
            // The machine free first takes the next plate it suits.
            let Some(next) = free
                .iter()
                .enumerate()
                .filter(|(_, (_, t, _))| *t < at + SCHEDULE_HORIZON)
                .min_by_key(|(_, (_, t, _))| *t)
                .map(|(i, _)| i)
            else {
                break;
            };
            let (lane, start, machine) = &free[next];
            let Some(c) = self.candidate_with(machine, &planned, jobs, files).await? else {
                free.remove(next);
                continue;
            };
            let length = file_of(&c.file_id)
                .and_then(|f| f.meta.estimated_s)
                .map_or(UNKNOWN_PLATE_SECONDS, |s| s as i64)
                .max(60);
            let (part, project) = self.part_names(c.part).await?;
            let label = file_of(&c.file_id)
                .map(|f| f.name.clone())
                .unwrap_or_default();
            lanes[*lane].blocks.push(Block {
                start: *start,
                end: start + length,
                kind: "planned",
                label,
                part,
                project,
            });
            *planned.entry(c.part).or_insert(0) += c.per_plate;
            free[next].1 = start + length;
            plates += 1;
        }
        Ok(lanes)
    }

    /// How many open parts (of active projects) a machine could make, of how many there are.
    pub(crate) async fn compatible(
        &mut self,
        machine: &crate::store::Machine,
        files: &[StoredFile],
    ) -> sqlx::Result<(i64, i64)> {
        let open: Vec<(i64, i64, i64)> = sqlx::query_as(
            "SELECT p.id, p.target, p.done FROM parts p JOIN projects pr ON pr.id = p.project_id
             WHERE p.space_id = $1 AND pr.state = 'active'",
        )
        .bind(self.space)
        .fetch_all(self.c())
        .await?;
        let mut can = 0;
        let mut all = 0;
        for (part, target, done) in open {
            if done >= target {
                continue;
            }
            all += 1;
            if self
                .part_files(part, files)
                .await?
                .iter()
                .any(|f| machine.profile.as_deref() == Some(f.profile.as_str()) && fits(f, machine))
            {
                can += 1;
            }
        }
        Ok((can, all))
    }
}

/// How `done` changes.
pub enum PartChange {
    By(i64),
    To(i64),
}

/// Whether what is loaded is what a part's file needs (nothing needed: anything goes).
fn material_fits(file: &PartFile, loaded: Option<&crate::store::Loaded>) -> bool {
    let material_ok = file.material.as_deref().is_none_or(|want| {
        loaded
            .and_then(|l| l.material.as_deref())
            .is_some_and(|have| have.eq_ignore_ascii_case(want))
    });
    let color_ok = file.color.as_deref().is_none_or(|want| {
        loaded.and_then(|l| l.color.as_deref()).is_some_and(|have| {
            have.trim_start_matches('#')
                .eq_ignore_ascii_case(want.trim_start_matches('#'))
        })
    });
    material_ok && color_ok
}

/// Whether the build plate on a machine is the kind a file needs (none needed: any).
fn sheet_fits(file: &PartFile, machine: &crate::store::Machine) -> bool {
    file.sheet.as_deref().is_none_or(|want| {
        machine
            .sheet
            .as_deref()
            .is_some_and(|have| have.trim().eq_ignore_ascii_case(want.trim()))
    })
}

/// Whether a machine suits a part's file now: what is loaded, and the plate on it.
fn fits(file: &PartFile, machine: &crate::store::Machine) -> bool {
    material_fits(file, machine.loaded.as_ref()) && sheet_fits(file, machine)
}

/// The dispatcher, in the background.
pub async fn run(store: Store) {
    let mut tick = tokio::time::interval(DISPATCH_EVERY);
    loop {
        tick.tick().await;
        if let Err(error) = store.dispatch().await {
            println!("dispatcher: {error}");
        }
    }
}

/// How many parts a file holds, from its name as print farms write it: `10x_bracket.gcode`.
pub fn per_plate_from_name(name: &str) -> Option<i64> {
    let (n, _) = name.split_once(['x', 'X'])?;
    n.parse::<i64>().ok().filter(|n| (1..=1000).contains(n))
}

// ---- routes ---------------------------------------------------------------------------

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/api/projects", get(list).post(new_project))
        .route("/api/schedule", get(schedule))
        .route("/api/projects/{id}", post(change_project))
        .route("/api/projects/{id}/remove", post(remove_project))
        .route("/api/projects/{id}/parts", post(new_part))
        .route("/api/parts/{id}", post(change_part))
        .route("/api/parts/{id}/remove", post(remove_part))
        .route("/api/parts/{id}/files", post(add_file))
        .route("/api/parts/{id}/files/{file}/remove", post(remove_file))
        .route("/api/parts/{id}/count", post(correct))
        .route("/api/parts/{id}/ledger", get(ledger))
        .route("/machines/{id}/farm", post(farm_toggle))
        .route("/machines/{id}/upkeep", post(upkeep))
        .route("/machines/{id}/sheet", post(sheet))
        .route("/machines/{id}/serviced", post(serviced))
        .route("/machines/{id}/ready", post(ready))
        .route("/machines/{id}/bad", post(bad))
}

fn bad_request(why: &str) -> Response {
    (StatusCode::BAD_REQUEST, why.to_owned()).into_response()
}

async fn schedule(access: Access, State(store): State<Store>) -> Response {
    match store.schedule(access.space).await {
        Ok(lanes) => Json(json!({ "now": now(), "lanes": lanes })).into_response(),
        Err(error) => failed(error),
    }
}

async fn list(access: Access, State(store): State<Store>) -> Response {
    let jobs = match store.job_facts(access.space, &[]).await {
        Ok(jobs) => jobs,
        Err(error) => return failed(error),
    };
    // The space's machines, files and machine modes, read first.
    let ms = match store.machines(access.space).await {
        Ok(ms) => ms,
        Err(error) => return failed(error),
    };
    let files = match store.files(access.space).await {
        Ok(files) => files,
        Err(error) => return failed(error),
    };
    let grants = match store.grants_for(&access, Level::Manage).await {
        Ok(grants) => grants,
        Err(error) => return failed(error),
    };
    let farm = crate::stock::in_space!(&store, access.space, |w| async {
        let mut compatible = Vec::new();
        for m in &ms {
            // A count that fails leaves its field empty, not the list.
            compatible.push(w.or_none(async |w| w.compatible(m, &files).await).await?);
        }
        let projects = w.projects(&jobs, &ms, &files).await?;
        Ok::<_, sqlx::Error>((compatible, projects))
    })
    .await;
    let (compatible, projects) = match farm {
        Ok(farm) => farm,
        Err(error) => return failed(error),
    };
    let store = store.lock().unwrap();
    let machines: Vec<Value> = ms
        .iter()
        .zip(compatible)
        .map(|(m, compatible)| {
            json!({
                "id": m.id, "name": m.name, "profile": m.profile, "kind": m.kind,
                "farm": m.takes_work,
                "sheet": m.sheet,
                "compatible": compatible,
                "manage": store.may(&access, &m.id, Level::Manage, &grants).unwrap_or(false),
            })
        })
        .collect();
    Json(json!({ "projects": projects, "machines": machines, "admin": access.role >= Role::Admin }))
        .into_response()
}

#[derive(Deserialize)]
struct ProjectBody {
    name: Option<String>,
    description: Option<String>,
    state: Option<String>,
    priority: Option<i64>,
}

fn clean(text: &str, max: usize) -> Option<String> {
    let t = text.trim();
    (!t.is_empty() && t.chars().count() <= max).then(|| t.to_owned())
}

async fn new_project(
    access: Access,
    State(store): State<Store>,
    Json(body): Json<ProjectBody>,
) -> Response {
    let Some(name) = body.name.as_deref().and_then(|n| clean(n, 80)) else {
        return bad_request("a project needs a name of at most 80 characters");
    };
    let description = body.description.unwrap_or_default();
    let made = crate::stock::in_space!(&store, access.space, |w| async {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO projects (space_id, name, description, created_at) VALUES ($1, $2, $3, at_unix($4))
             RETURNING id",
        )
        .bind(w.space)
        .bind(&name)
        .bind(description.trim())
        .bind(now())
        .fetch_one(w.c())
        .await
    })
    .await;
    match made {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Err(error) => failed(error),
    }
}

async fn change_project(
    access: Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
    Json(body): Json<ProjectBody>,
) -> Response {
    if let Some(state) = &body.state
        && !["draft", "active", "paused", "done"].contains(&state.as_str())
    {
        return bad_request("draft, active, paused or done");
    }
    let name = match body.name.as_deref().map(|n| clean(n, 80)) {
        Some(None) => return bad_request("a name of at most 80 characters"),
        Some(Some(n)) => Some(n),
        None => None,
    };
    let n = crate::stock::in_space!(&store, access.space, |w| async {
        let n = sqlx::query(
            "UPDATE projects SET name = COALESCE($3, name), description = COALESCE($4, description),
                 state = COALESCE($5, state), priority = COALESCE($6, priority)
             WHERE id = $1 AND space_id = $2",
        )
        .bind(id)
        .bind(w.space)
        .bind(&name)
        .bind(&body.description)
        .bind(&body.state)
        .bind(body.priority)
        .execute(w.c())
        .await?
        .rows_affected();
        if n > 0 {
            w.changed();
        }
        Ok::<_, sqlx::Error>(n)
    })
    .await;
    match n {
        Ok(0) => (StatusCode::NOT_FOUND, "no such project").into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

async fn remove_project(
    access: Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
) -> Response {
    // The project's parts, and whether plates of them are under way.
    let parts: Vec<i64> = crate::stock::in_space!(&store, access.space, |w| async {
        sqlx::query_scalar("SELECT id FROM parts WHERE project_id = $1")
            .bind(id)
            .fetch_all(w.c())
            .await
    })
    .await
    .unwrap_or_default();
    let busy: i64 = crate::stock::in_space!(&store, access.space, |w| async {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE part_id = ANY($1)
               AND (state IN ('loaded', 'queued', 'printing', 'paused', 'stopping') OR (state IN ('finished', 'stopped', 'failed') AND confirmed_at IS NULL))",
        )
        .bind(&parts)
        .fetch_one(w.c())
        .await
    })
    .await
    .unwrap_or(0);
    if busy > 0 {
        return (
            StatusCode::CONFLICT,
            "plates of it are on a bed or wait for someone: pause it, and remove it after",
        )
            .into_response();
    }
    let removed = crate::stock::in_space!(&store, access.space, |w| async {
        let n = sqlx::query("DELETE FROM projects WHERE id = $1 AND space_id = $2")
            .bind(id)
            .bind(w.space)
            .execute(w.c())
            .await?
            .rows_affected();
        sqlx::query(
            "DELETE FROM part_files WHERE part_id IN (SELECT id FROM parts WHERE project_id = $1 AND space_id = $2)",
        )
        .bind(id)
        .bind(w.space)
        .execute(w.c())
        .await?;
        sqlx::query("DELETE FROM parts WHERE project_id = $1 AND space_id = $2")
            .bind(id)
            .bind(w.space)
            .execute(w.c())
            .await?;
        Ok::<_, sqlx::Error>(n)
    })
    .await;
    match removed {
        Ok(0) => (StatusCode::NOT_FOUND, "no such project").into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct PartBody {
    name: Option<String>,
    target: Option<i64>,
    sort_order: Option<i64>,
}

async fn new_part(
    access: Access,
    State(store): State<Store>,
    Path(project): Path<i64>,
    Json(body): Json<PartBody>,
) -> Response {
    let Some(name) = body.name.as_deref().and_then(|n| clean(n, 80)) else {
        return bad_request("a part needs a name of at most 80 characters");
    };
    let Some(target) = body.target.filter(|t| (1..=1_000_000).contains(t)) else {
        return bad_request("how many: 1 or more");
    };
    let made = crate::stock::in_space!(&store, access.space, |w| async {
        let in_space: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM projects WHERE id = $1 AND space_id = $2",
        )
        .bind(project)
        .bind(w.space)
        .fetch_one(w.c())
        .await?;
        if !in_space {
            return Ok(None);
        }
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO parts (project_id, space_id, name, target, sort_order, created_at)
             VALUES ($1, $2, $3, $4, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM parts WHERE project_id = $1), at_unix($5))
             RETURNING id",
        )
        .bind(project)
        .bind(w.space)
        .bind(&name)
        .bind(target)
        .bind(now())
        .fetch_one(w.c())
        .await
        .map(Some)
    })
    .await;
    match made {
        Ok(Some(id)) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no such project").into_response(),
        Err(error) => failed(error),
    }
}

async fn change_part(
    access: Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
    Json(body): Json<PartBody>,
) -> Response {
    if body.target.is_some_and(|t| !(1..=1_000_000).contains(&t)) {
        return bad_request("how many: 1 or more");
    }
    let name = match body.name.as_deref().map(|n| clean(n, 80)) {
        Some(None) => return bad_request("a name of at most 80 characters"),
        Some(Some(n)) => Some(n),
        None => None,
    };
    let n = crate::stock::in_space!(&store, access.space, |w| async {
        let n = sqlx::query(
            "UPDATE parts SET name = COALESCE($3, name), target = COALESCE($4, target), sort_order = COALESCE($5, sort_order)
             WHERE id = $1 AND space_id = $2",
        )
        .bind(id)
        .bind(w.space)
        .bind(&name)
        .bind(body.target)
        .bind(body.sort_order)
        .execute(w.c())
        .await?
        .rows_affected();
        if n > 0 {
            w.changed();
        }
        Ok::<_, sqlx::Error>(n)
    })
    .await;
    match n {
        Ok(0) => (StatusCode::NOT_FOUND, "no such part").into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

async fn remove_part(access: Access, State(store): State<Store>, Path(id): Path<i64>) -> Response {
    match store
        .job_facts(access.space, &[])
        .await
        .map(|jobs| under_way(&jobs, id))
    {
        Ok((plates, _)) if plates > 0 => {
            return (
                StatusCode::CONFLICT,
                "plates of it are on a bed or wait for someone",
            )
                .into_response();
        }
        Err(error) => return failed(error),
        _ => {}
    }
    let removed = crate::stock::in_space!(&store, access.space, |w| async {
        let n = sqlx::query("DELETE FROM parts WHERE id = $1 AND space_id = $2")
            .bind(id)
            .bind(w.space)
            .execute(w.c())
            .await?
            .rows_affected();
        sqlx::query("DELETE FROM part_files WHERE part_id = $1 AND space_id = $2")
            .bind(id)
            .bind(w.space)
            .execute(w.c())
            .await?;
        Ok::<_, sqlx::Error>(n)
    })
    .await;
    match removed {
        Ok(0) => (StatusCode::NOT_FOUND, "no such part").into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct FileBody {
    file: String,
    profile: String,
    per_plate: Option<i64>,
    material: Option<String>,
    color: Option<String>,
    sheet: Option<String>,
}

/// A part's file for one machine model; one per model (a second replaces the first).
async fn add_file(
    access: Access,
    State(store): State<Store>,
    Path(part): Path<i64>,
    Json(body): Json<FileBody>,
) -> Response {
    if crate::profiles::find(&body.profile).is_none() {
        return bad_request("no such machine model");
    }
    let file = match store.file(access.space, &body.file).await {
        Ok(Some(f)) => f,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such file").into_response(),
        Err(error) => return failed(error),
    };
    let per_plate = body
        .per_plate
        .or_else(|| per_plate_from_name(&file.name))
        .unwrap_or(1)
        .clamp(1, 1000);
    let blank = |v: Option<String>| v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let (material, color, sheet) = (blank(body.material), blank(body.color), blank(body.sheet));
    let added = crate::stock::in_space!(&store, access.space, |w| async {
        let in_space: bool =
            sqlx::query_scalar("SELECT COUNT(*) > 0 FROM parts WHERE id = $1 AND space_id = $2")
                .bind(part)
                .bind(w.space)
                .fetch_one(w.c())
                .await?;
        if !in_space {
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO part_files (part_id, space_id, profile, file_id, per_plate, material, color, created_at, sheet)
             VALUES ($1, $2, $3, $4, $5, $6, $7, at_unix($8), $9)
             ON CONFLICT (part_id, profile) DO UPDATE SET file_id = excluded.file_id, per_plate = excluded.per_plate,
                 material = excluded.material, color = excluded.color, sheet = excluded.sheet",
        )
        .bind(part)
        .bind(w.space)
        .bind(&body.profile)
        .bind(&file.id)
        .bind(per_plate)
        .bind(&material)
        .bind(&color)
        .bind(now())
        .bind(&sheet)
        .execute(w.c())
        .await?;
        Ok::<_, sqlx::Error>(true)
    })
    .await;
    match added {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such part").into_response(),
        Err(error) => failed(error),
    }
}

async fn remove_file(
    access: Access,
    State(store): State<Store>,
    Path((part, file)): Path<(i64, i64)>,
) -> Response {
    let removed = crate::stock::in_space!(&store, access.space, |w| async {
        sqlx::query("DELETE FROM part_files WHERE id = $1 AND part_id = $2 AND space_id = $3")
            .bind(file)
            .bind(part)
            .bind(w.space)
            .execute(w.c())
            .await
            .map(|r| r.rows_affected())
    })
    .await;
    match removed {
        Ok(0) => (StatusCode::NOT_FOUND, "no such file of this part").into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct CountBody {
    done: i64,
    note: Option<String>,
}

/// An admin corrects a part's count (parts made by hand, a miscount): in the ledger, with why.
async fn correct(
    access: Access,
    State(store): State<Store>,
    Path(part): Path<i64>,
    Json(body): Json<CountBody>,
) -> Response {
    let note = body.note.unwrap_or_default();
    if note.trim().is_empty() {
        return bad_request("say why: it goes into the ledger");
    }
    let why = Why {
        source: "corrected",
        job: None,
        actor: &access.actor,
        note: note.trim(),
    };
    match crate::stock::in_space!(&store, access.space, |w| w.adjust_part(
        part,
        PartChange::To(body.done),
        why
    ))
    .await
    {
        Ok(Some(done)) => Json(json!({ "done": done })).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no such part").into_response(),
        Err(error) => failed(error),
    }
}

async fn ledger(access: Access, State(store): State<Store>, Path(part): Path<i64>) -> Response {
    match store.ledger(access.space, part).await {
        Ok(lines) => Json(lines).into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct FarmForm {
    on: Option<String>,
}

async fn farm_toggle(
    access: Access,
    State(store): State<Store>,
    Path(machine): Path<String>,
    Form(form): Form<FarmForm>,
) -> Response {
    match store
        .set_takes_work(access.space, &machine, form.on.is_some())
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct SheetForm {
    /// The kind of build plate on it now; empty: not said.
    sheet: Option<String>,
}

/// Someone swapped the build plate: which is on now.
async fn sheet(
    access: Access,
    State(store): State<Store>,
    Path(machine): Path<String>,
    Form(form): Form<SheetForm>,
) -> Response {
    let sheet: Option<String> = form
        .sheet
        .map(|s| s.trim().chars().take(40).collect::<String>())
        .filter(|s| !s.is_empty());
    match store
        .set_sheet(access.space, &machine, sheet.as_deref())
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct UpkeepForm {
    /// Printing hours between services; empty: no interval.
    every_h: Option<String>,
}

async fn upkeep(
    access: Access,
    State(store): State<Store>,
    Path(machine): Path<String>,
    Form(form): Form<UpkeepForm>,
) -> Response {
    let every = match form
        .every_h
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(text) => match text.parse::<f64>() {
            Ok(h) if (1.0..=100_000.0).contains(&h) => Some(h),
            _ => return bad_request("hours between services: 1 or more, or empty for none"),
        },
    };
    match store.set_upkeep(access.space, &machine, every).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

async fn serviced(
    access: Access,
    State(store): State<Store>,
    Path(machine): Path<String>,
) -> Response {
    match store.serviced(access.space, &machine).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct ReadyForm {
    /// The job the page showed: a different one waiting now is refused.
    job: i64,
    /// Parts that came off good (a plate's count by default).
    good: Option<i64>,
    note: Option<String>,
}

/// Set Ready: the plate is off the bed, `good` parts are good; the machine takes the next.
async fn ready(
    access: Access,
    State(store): State<Store>,
    Path(machine): Path<String>,
    Form(form): Form<ReadyForm>,
) -> Response {
    confirm(access, store, machine, form, false).await
}

/// Bad print: nothing good came off; the plate goes back in the queue by itself.
async fn bad(
    access: Access,
    State(store): State<Store>,
    Path(machine): Path<String>,
    Form(form): Form<ReadyForm>,
) -> Response {
    confirm(access, store, machine, form, true).await
}

async fn confirm(
    access: Access,
    store: Store,
    machine: String,
    form: ReadyForm,
    bad: bool,
) -> Response {
    let per_plate = match store.job_facts_of(access.space, &machine).await {
        Ok(jobs) => match jobs.waiting_plate(&machine) {
            Some(w) => w.per_plate.unwrap_or(1),
            None => {
                return (
                    StatusCode::CONFLICT,
                    "nothing waits to be confirmed on this machine",
                )
                    .into_response();
            }
        },
        Err(error) => return failed(error),
    };
    let good = if bad {
        0
    } else {
        form.good.unwrap_or(per_plate)
    };
    let done = store
        .confirm_plate(
            &access,
            &machine,
            form.job,
            good,
            form.note.as_deref().unwrap_or(""),
        )
        .await;
    // How it is reached, for its ready (Postgres, before the lock).
    let link = match &done {
        Ok(Ok(_)) => store.link_of(access.space, &machine).await.ok().flatten(),
        _ => None,
    };
    let mut store = store.lock().unwrap();
    match done {
        Ok(Ok(count)) => {
            // A Prusa on its own Connect: ready on its screen too.
            if link.as_deref() == Some("prusa-connect") {
                let _ = store.queue_connect_command(
                    link.as_deref(),
                    &machine,
                    "Set ready",
                    &json!({"command": "SET_PRINTER_READY"}),
                    false,
                );
            }
            let _ = JobState::Finished;
            Json(json!({ "done": count })).into_response()
        }
        Ok(Err(why)) => (StatusCode::CONFLICT, why).into_response(),
        Err(error) => failed(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plates_from_the_name() {
        assert_eq!(per_plate_from_name("10x_bracket.gcode"), Some(10));
        assert_eq!(per_plate_from_name("4X-bin.bgcode"), Some(4));
        assert_eq!(per_plate_from_name("bracket_10x.gcode"), None);
        assert_eq!(per_plate_from_name("x.gcode"), None);
    }

    /// `done` is written in one place only: adjust_part, with its ledger line.
    #[test]
    fn only_the_ledger_writes_done() {
        // SQL in the source: string literals, a line starting with a quote or a query's own line.
        let mut writers = 0;
        for source in crate::access::route_sources() {
            for line in source
                .lines()
                .filter(|l| l.trim_start().starts_with('"') || l.contains("query(\""))
            {
                let l = line.replace(' ', "");
                if l.contains("UPDATEparts") && l.contains("done=") {
                    assert!(
                        l.contains("SETdone=$2WHEREid=$1"),
                        "a part's done written outside adjust_part: {line}"
                    );
                    writers += 1;
                }
            }
        }
        assert_eq!(writers, 1, "adjust_part's own UPDATE, and nothing else");
    }
}

#[cfg(test)]
mod flow {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;

    use tower::ServiceExt;

    async fn call(app: &Router, request: Request<Body>) -> (StatusCode, String) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    fn json_post(path: String, body: Value) -> Request<Body> {
        Request::post(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn form_post(path: String, body: String) -> Request<Body> {
        Request::post(path)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap()
    }

    /// One project, one part of 25 at 10 a plate, on two MK3S printers that take work (and a
    /// third that does not): whole plates go out, a finished plate holds its machine until
    /// someone confirms it, Set Ready counts once, a bad print counts nothing and the plate
    /// goes out again, and never more plates than needed.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_project_over_many_printers(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let mk3s = crate::profiles::find("prusa/mk3s").unwrap();
        let (space, a, b, c) = {
            let space = store.default_space().await.unwrap().0;
            let a = store.add_virtual(space, mk3s).await.unwrap();
            let b = store.add_virtual(space, mk3s).await.unwrap();
            let c = store.add_virtual(space, mk3s).await.unwrap();
            store.set_takes_work(space, &a, true).await.unwrap();
            store.set_takes_work(space, &b, true).await.unwrap();
            (space, a, b, c)
        };
        for m in [&a, &b, &c] {
            crate::auth::tester_may_operate(&store, m).await;
        }
        let idle = async |machine: &str| store.idle_test_machine(machine).await;
        for m in [&a, &b, &c] {
            idle(m).await;
        }
        // A file, a project, a part, the file for the MK3S (10 a plate, from its name).
        let (status, body) = call(
            &app,
            Request::post("/api/files?name=10x_bracket.gcode")
                .body(Body::from("G28\nG1 X10 Y10\n"))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let file = serde_json::from_str::<Value>(&body).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let (_, body) = call(
            &app,
            json_post(
                "/api/projects".into(),
                json!({"name": "Brackets for the hall"}),
            ),
        )
        .await;
        let project = serde_json::from_str::<Value>(&body).unwrap()["id"]
            .as_i64()
            .unwrap();
        let (_, body) = call(
            &app,
            json_post(
                format!("/api/projects/{project}/parts"),
                json!({"name": "Bracket", "target": 25}),
            ),
        )
        .await;
        let part = serde_json::from_str::<Value>(&body).unwrap()["id"]
            .as_i64()
            .unwrap();
        let (status, _) = call(
            &app,
            json_post(
                format!("/api/parts/{part}/files"),
                json!({"file": file, "profile": "prusa/mk3s"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let waiting = async || store.projects(space).await.unwrap()[0].parts[0].clone();
        assert_eq!(
            waiting().await.files[0].per_plate,
            10,
            "from the file's name"
        );
        assert_eq!(
            waiting().await.waiting.as_deref(),
            Some("the project is draft")
        );
        // Draft: nothing goes out. Active: a plate to each machine that takes work.
        assert_eq!(store.dispatch().await.unwrap(), 0);
        call(
            &app,
            json_post(
                format!("/api/projects/{project}"),
                json!({"state": "active"}),
            ),
        )
        .await;
        // Ahead of dispatch, the schedule plans the same: 3 plates of 10 make 25, over a and
        // b (c takes no work), and nothing changes by looking.
        {
            let ma = store
                .machines(space)
                .await
                .unwrap()
                .into_iter()
                .find(|m| m.id == a)
                .unwrap();
            let lanes = store.schedule(space).await.unwrap();
            let active = store.active_job(space, &a).await.unwrap();
            let files = store.files(space).await.unwrap();
            let compatible = crate::stock::in_space!(&store, space, |w| w.compatible(&ma, &files))
                .await
                .unwrap();
            assert_eq!(lanes.len(), 2, "only the machines that take work");
            let plates: usize = lanes
                .iter()
                .map(|l| l.blocks.iter().filter(|b| b.kind == "planned").count())
                .sum();
            assert_eq!(plates, 3, "whole plates up to the target, no more");
            assert!(
                lanes.iter().all(|l| !l.blocks.is_empty()),
                "both machines get one"
            );
            assert_eq!(compatible, (1, 1));
            assert!(active.is_none(), "looking ahead starts nothing");
        }
        assert_eq!(store.dispatch().await.unwrap(), 2);
        let job_on = async |m: &str| store.active_job(space, m).await.unwrap();
        assert!(
            job_on(&a).await.is_some() && job_on(&b).await.is_some() && job_on(&c).await.is_none(),
            "not to a machine that takes no work"
        );
        assert_eq!(waiting().await.plates_under_way, 2);
        assert_eq!(
            job_on(&a).await.unwrap().by.as_deref(),
            Some("queue · Brackets for the hall"),
            "the queue started it, for that project"
        );
        // A finished, b failed: both hold their machine.
        let (ja, jb) = (job_on(&a).await.unwrap().id, job_on(&b).await.unwrap().id);
        store
            .set_job(space, ja, JobState::Finished, None)
            .await
            .unwrap();
        store
            .set_job(space, jb, JobState::Failed, Some("spaghetti"))
            .await
            .unwrap();
        idle(&a).await;
        idle(&b).await;
        // One plate still needed (25 > 0 + 10 under way on a): it waits, both machines held.
        assert_eq!(
            store.dispatch().await.unwrap(),
            0,
            "held: nothing new until someone confirms"
        );
        assert!(
            store
                .machine(space, &a)
                .await
                .unwrap()
                .unwrap()
                .confirm
                .is_some()
        );
        // Set Ready on a: 10 good. Twice the same is refused.
        let (status, body) = call(
            &app,
            form_post(format!("/machines/{a}/ready"), format!("job={ja}&good=10")),
        )
        .await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, r#"{"done":10}"#));
        let (status, _) = call(
            &app,
            form_post(format!("/machines/{a}/ready"), format!("job={ja}&good=10")),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "counted once");
        // Bad print on b: nothing counts.
        let (status, body) = call(
            &app,
            form_post(format!("/machines/{b}/bad"), format!("job={jb}")),
        )
        .await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, r#"{"done":10}"#));
        // 15 still needed: two plates go out again (whole plates: 10 + 20 + 10 > 25 is fine).
        assert_eq!(store.dispatch().await.unwrap(), 2);
        let (ja, jb) = (job_on(&a).await.unwrap().id, job_on(&b).await.unwrap().id);
        for (m, j) in [(&a, ja), (&b, jb)] {
            store
                .set_job(space, j, JobState::Finished, None)
                .await
                .unwrap();
            idle(m).await;
        }
        call(
            &app,
            form_post(format!("/machines/{a}/ready"), format!("job={ja}&good=10")),
        )
        .await;
        // 9 of 10 good on b: the part is at 29 of 25, nothing more goes out.
        call(
            &app,
            form_post(format!("/machines/{b}/ready"), format!("job={jb}&good=9")),
        )
        .await;
        assert_eq!(waiting().await.done, 29);
        assert_eq!(store.dispatch().await.unwrap(), 0, "enough");
        assert!(waiting().await.waiting.is_none());
        // Every change is in the ledger, with who and why.
        let ledger = store.ledger(space, part).await.unwrap();
        let sources: Vec<&str> = ledger.iter().map(|l| l.source.as_str()).collect();
        assert_eq!(
            sources,
            ["confirmed", "confirmed", "bad print", "confirmed"]
        );
        assert_eq!(ledger[0].actor, "Tester");
        // A colour asked for, nothing like it loaded: the reason says so.
        call(
            &app,
            json_post(format!("/api/parts/{part}"), json!({"target": 100})),
        )
        .await;
        call(
            &app,
            json_post(
                format!("/api/parts/{part}/files"),
                json!({"file": file, "profile": "prusa/mk3s", "material": "PETG"}),
            ),
        )
        .await;
        assert_eq!(store.dispatch().await.unwrap(), 0);
        assert_eq!(
            waiting().await.waiting.as_deref(),
            Some("nothing with PETG loaded")
        );
    }
}

#[cfg(test)]
mod upkeep {
    use super::*;

    /// A machine due for its service takes no plate; once serviced, it does again.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_part_that_wants_a_plate_waits_for_it(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let space = store.default_space().await.unwrap().0;
        let m = store
            .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
            .await
            .unwrap();
        store
            .add_file(space, "f1", "4x_a.gcode", 10, &Default::default())
            .await
            .unwrap();
        store.idle_test_machine(&m).await;
        store.set_takes_work(space, &m, true).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO projects (space_id, name, state, created_at) VALUES ({space}, 'P', 'active', at_unix(0));
             INSERT INTO parts (project_id, space_id, name, target, created_at) VALUES (1, {space}, 'A', 4, at_unix(0));
             INSERT INTO part_files (part_id, space_id, profile, file_id, per_plate, created_at, sheet) VALUES (1, {space}, 'prusa/mk3s', 'f1', 4, at_unix(0), 'Textured PEI');"
        )))
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            store.dispatch().await.unwrap(),
            0,
            "no plate said: not sent"
        );
        let why = store.projects(space).await.unwrap()[0].parts[0]
            .waiting
            .clone();
        assert_eq!(
            why.as_deref(),
            Some("no machine has a Textured PEI plate on")
        );
        store
            .set_sheet(space, &m, Some("Smooth PEI"))
            .await
            .unwrap();
        assert_eq!(store.dispatch().await.unwrap(), 0, "the wrong plate");
        store
            .set_sheet(space, &m, Some("textured pei"))
            .await
            .unwrap();
        assert_eq!(
            store.dispatch().await.unwrap(),
            1,
            "the right plate, however it is written"
        );
    }

    /// Why a part waits, as its machine's job tells: a plate under way on the machine counts
    /// towards the target (nothing to wait for once it covers it); short of it, the machine is busy.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn why_a_part_waits_sees_the_machines_job(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let space = store.default_space().await.unwrap().0;
        let m = store
            .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
            .await
            .unwrap();
        store
            .add_file(space, "f1", "4x_a.gcode", 10, &Default::default())
            .await
            .unwrap();
        store.idle_test_machine(&m).await;
        store.set_takes_work(space, &m, true).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO projects (space_id, name, state, created_at) VALUES ({space}, 'P', 'active', at_unix(0));
             INSERT INTO parts (project_id, space_id, name, target, created_at) VALUES (1, {space}, 'A', 8, at_unix(0));
             INSERT INTO part_files (part_id, space_id, profile, file_id, per_plate, created_at) VALUES (1, {space}, 'prusa/mk3s', 'f1', 4, at_unix(0));"
        )))
        .execute(&pool)
        .await
        .unwrap();
        let part = async || store.projects(space).await.unwrap()[0].parts[0].clone();
        assert_eq!(store.dispatch().await.unwrap(), 1, "one plate goes out");
        assert!(
            store.active_job(space, &m).await.unwrap().is_some(),
            "the machine has its job"
        );
        let now = part().await;
        assert_eq!((now.plates_under_way, now.parts_under_way), (1, 4));
        assert_eq!(
            now.waiting.as_deref(),
            Some("its machines are busy, or wait for someone"),
            "4 of 8 under way, its only machine has the job"
        );
        sqlx::query("UPDATE parts SET target = 4 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            part().await.waiting,
            None,
            "the plate under way makes the target: nothing to wait for"
        );
    }

    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_service_due_holds_the_queue(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let space = store.default_space().await.unwrap().0;
        let m = store
            .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
            .await
            .unwrap();
        store
            .add_file(space, "f1", "10x_a.gcode", 10, &Default::default())
            .await
            .unwrap();
        store.idle_test_machine(&m).await;
        store.set_takes_work(space, &m, true).await.unwrap();
        store.set_upkeep(space, &m, Some(1.0)).await.unwrap();
        // Two hours printed: due.
        sqlx::query(
            "INSERT INTO jobs (space_id, machine_id, file_id, file_name, state, created_at, started_at, ended_at, taken_at)
             VALUES ($1, $2, 'f1', 'old.gcode', 'finished', at_unix($3), at_unix($3), at_unix($4), at_unix($4))",
        )
        .bind(space)
        .bind(&m)
        .bind(now() - 7300)
        .bind(now() - 100)
        .execute(&pool)
        .await
        .unwrap();
        let u = store
            .job_facts_of(space, &m)
            .await
            .unwrap()
            .upkeep(&m)
            .unwrap();
        assert!(u.due && u.since_h >= 2.0, "{u:?}");
        // Work waiting for it.
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO projects (space_id, name, state, created_at) VALUES ({space}, 'P', 'active', at_unix(0));
             INSERT INTO parts (project_id, space_id, name, target, created_at) VALUES (1, {space}, 'A', 10, at_unix(0));
             INSERT INTO part_files (part_id, space_id, profile, file_id, per_plate, created_at) VALUES (1, {space}, 'prusa/mk3s', 'f1', 10, at_unix(0));"
        )))
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(store.dispatch().await.unwrap(), 0, "due: no plate");
        store.serviced(space, &m).await.unwrap();
        assert!(
            !store
                .job_facts_of(space, &m)
                .await
                .unwrap()
                .upkeep(&m)
                .unwrap()
                .due
        );
        assert_eq!(store.dispatch().await.unwrap(), 1, "serviced: work again");
    }
}
