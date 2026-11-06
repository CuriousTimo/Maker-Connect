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
//! - **The ledger:** `done` changes in [`Store::adjust_part`] only, with one ledger row each
//!   time; a test reads the source and fails if anything else writes it.

use crate::access::{Access, Level, Role};
use crate::store::{JobState, Store, now};
use crate::{Shared, failed};
use axum::{
    Form, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How often the dispatcher looks for free machines.
const DISPATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(3);

pub fn migrate(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS projects (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL,
             name TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             state TEXT NOT NULL DEFAULT 'draft',
             priority INTEGER NOT NULL DEFAULT 0,
             created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS parts (
             id INTEGER PRIMARY KEY,
             project_id INTEGER NOT NULL,
             space_id INTEGER NOT NULL,
             name TEXT NOT NULL,
             target INTEGER NOT NULL,
             done INTEGER NOT NULL DEFAULT 0,
             sort_order INTEGER NOT NULL DEFAULT 0,
             created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS part_files (
             id INTEGER PRIMARY KEY,
             part_id INTEGER NOT NULL,
             space_id INTEGER NOT NULL,
             profile TEXT NOT NULL,
             file_id TEXT NOT NULL,
             per_plate INTEGER NOT NULL DEFAULT 1,
             material TEXT,
             color TEXT,
             created_at INTEGER NOT NULL,
             UNIQUE (part_id, profile)
         );
         CREATE TABLE IF NOT EXISTS part_ledger (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL,
             part_id INTEGER NOT NULL,
             at INTEGER NOT NULL,
             delta INTEGER NOT NULL,
             done_after INTEGER NOT NULL,
             source TEXT NOT NULL,
             job_id INTEGER,
             actor TEXT NOT NULL,
             note TEXT NOT NULL DEFAULT ''
         );",
    )?;
    for (table, column, definition) in [
        // A job for a part: which, the plate's file, and how many parts the plate holds.
        ("jobs", "part_id", "INTEGER"),
        ("jobs", "per_plate", "INTEGER"),
        // How it was confirmed: when, by whom, how many good.
        ("jobs", "confirmed_at", "INTEGER"),
        ("jobs", "confirmed_good", "INTEGER"),
        ("jobs", "confirmed_by", "TEXT"),
        // A machine takes work from the queue.
        ("machines", "farm", "INTEGER NOT NULL DEFAULT 0"),
        // Build plates (sheets): which a part's file needs, which is on a machine.
        ("part_files", "sheet", "TEXT"),
        ("machines", "sheet", "TEXT"),
        // Upkeep: a service every so many printing hours, and when the last was done.
        ("machines", "service_every_h", "REAL"),
        ("machines", "serviced_at", "INTEGER"),
    ] {
        let present: bool = db.query_row(
            &format!("SELECT COUNT(*) > 0 FROM pragma_table_info('{table}') WHERE name = ?1"),
            [column],
            |r| r.get(0),
        )?;
        if !present {
            db.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    Ok(())
}

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

impl Store {
    /// The space's projects, newest first within their priority, with their parts.
    pub fn projects(&self, space: i64) -> rusqlite::Result<Vec<Project>> {
        let mut q = self.db.prepare(
            "SELECT id, name, description, state, priority, created_at FROM projects
             WHERE space_id = ?1 ORDER BY CASE state WHEN 'active' THEN 0 WHEN 'paused' THEN 1 WHEN 'draft' THEN 2 ELSE 3 END,
                 priority, created_at",
        )?;
        let mut projects: Vec<Project> = q
            .query_map([space], |r| {
                Ok(Project {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    description: r.get(2)?,
                    state: r.get(3)?,
                    priority: r.get(4)?,
                    created_at: r.get(5)?,
                    parts: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        for project in &mut projects {
            project.parts = self.parts_of(space, project.id, &project.state)?;
        }
        Ok(projects)
    }

    fn parts_of(&self, space: i64, project: i64, state: &str) -> rusqlite::Result<Vec<Part>> {
        let mut q = self.db.prepare(
            "SELECT id, name, target, done FROM parts WHERE space_id = ?1 AND project_id = ?2
             ORDER BY sort_order, created_at, id",
        )?;
        let rows: Vec<(i64, String, i64, i64)> = q
            .query_map(params![space, project], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut parts = Vec::new();
        for (id, name, target, done) in rows {
            let (plates, held) = self.under_way(id)?;
            let files = self.part_files(space, id)?;
            let mut part = Part {
                id,
                name,
                target,
                done,
                plates_under_way: plates,
                parts_under_way: held,
                files,
                waiting: None,
            };
            part.waiting = self.why_waiting(space, state, &part)?;
            parts.push(part);
        }
        Ok(parts)
    }

    /// Plates of a part under way (on a bed, or ended and not yet confirmed), and the parts
    /// they hold.
    fn under_way(&self, part: i64) -> rusqlite::Result<(i64, i64)> {
        self.db.query_row(
            "SELECT COUNT(*), COALESCE(SUM(per_plate), 0) FROM jobs WHERE part_id = ?1 AND (
                 state IN ('queued', 'printing', 'paused', 'stopping')
                 OR (state = 'finished' AND confirmed_at IS NULL))",
            [part],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    }

    fn part_files(&self, space: i64, part: i64) -> rusqlite::Result<Vec<PartFile>> {
        let mut q = self.db.prepare(
            "SELECT pf.id, pf.profile, pf.file_id, COALESCE(f.name, '(file removed)'), pf.per_plate, pf.material, pf.color, pf.sheet
             FROM part_files pf LEFT JOIN files f ON f.id = pf.file_id AND f.space_id = pf.space_id
             WHERE pf.space_id = ?1 AND pf.part_id = ?2 ORDER BY pf.profile",
        )?;
        q.query_map(params![space, part], |r| {
            Ok(PartFile {
                id: r.get(0)?,
                profile: r.get(1)?,
                file_id: r.get(2)?,
                file_name: r.get(3)?,
                per_plate: r.get(4)?,
                material: r.get(5)?,
                color: r.get(6)?,
                sheet: r.get(7)?,
            })
        })?
        .collect()
    }

    /// Why a part sends no plate out now, in words (print-farm-manager's dispatch status).
    fn why_waiting(
        &self,
        space: i64,
        project_state: &str,
        part: &Part,
    ) -> rusqlite::Result<Option<String>> {
        if part.done + part.parts_under_way >= part.target {
            return Ok(None);
        }
        if project_state != "active" {
            return Ok(Some(format!("the project is {project_state}")));
        }
        if part.files.is_empty() {
            return Ok(Some("no file for any machine yet".to_owned()));
        }
        let machines = self.machines(space)?;
        let farm: Vec<_> = machines
            .iter()
            .filter(|m| self.takes_work(&m.id).unwrap_or(false))
            .collect();
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
            return Ok(Some(format!(
                "no machine in the queue is a {}",
                models.join(" or ")
            )));
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
            return Ok(Some(format!("nothing with {} loaded", wants.join(" or "))));
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
            return Ok(Some(format!(
                "no machine has a {} plate on",
                sheets.join(" or ")
            )));
        }
        if plated.iter().all(|m| !m.online) {
            return Ok(Some("its machines are offline".to_owned()));
        }
        Ok(Some(
            "its machines are busy, or wait for someone".to_owned(),
        ))
    }

    /// Whether a machine takes work from the queue.
    pub fn takes_work(&self, machine: &str) -> rusqlite::Result<bool> {
        Ok(self
            .db
            .query_row("SELECT farm FROM machines WHERE id = ?1", [machine], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or(false))
    }

    pub fn set_takes_work(
        &mut self,
        space: i64,
        machine: &str,
        on: bool,
    ) -> rusqlite::Result<bool> {
        let n = self.db.execute(
            "UPDATE machines SET farm = ?3 WHERE id = ?1 AND space_id = ?2",
            params![machine, space, on],
        )?;
        self.changed();
        Ok(n > 0)
    }

    /// The plate that ended on a machine and waits for someone, if any: its last job, when it
    /// was for a part and is not yet confirmed.
    pub fn waiting_confirm(&self, machine: &str) -> rusqlite::Result<Option<Confirm>> {
        self.db
            .query_row(
                "SELECT j.id, j.file_name, j.state, COALESCE(p.name, ''), COALESCE(pr.name, ''), COALESCE(j.per_plate, 1)
                 FROM jobs j LEFT JOIN parts p ON p.id = j.part_id LEFT JOIN projects pr ON pr.id = p.project_id
                 WHERE j.id = (SELECT MAX(id) FROM jobs WHERE machine_id = ?1)
                   AND j.part_id IS NOT NULL AND j.state IN ('finished', 'stopped', 'failed')
                   AND j.confirmed_at IS NULL",
                [machine],
                |r| {
                    Ok(Confirm {
                        job: r.get(0)?,
                        file_name: r.get(1)?,
                        state: r.get(2)?,
                        part: r.get(3)?,
                        project: r.get(4)?,
                        per_plate: r.get(5)?,
                    })
                },
            )
            .optional()
    }

    /// The only way `done` changes: by `delta` (or to `set`), with a line in the ledger saying
    /// why and who. Returns the new count.
    pub fn adjust_part(
        &mut self,
        space: i64,
        part: i64,
        change: PartChange,
        why: Why,
    ) -> rusqlite::Result<Option<i64>> {
        let Why {
            source,
            job,
            actor,
            note,
        } = why;
        let Some(done) = self
            .db
            .query_row(
                "SELECT done FROM parts WHERE id = ?1 AND space_id = ?2",
                params![part, space],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        else {
            return Ok(None);
        };
        let after = match change {
            PartChange::By(delta) => (done + delta).max(0),
            PartChange::To(value) => value.max(0),
        };
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "UPDATE parts SET done = ?2 WHERE id = ?1",
            params![part, after],
        )?;
        tx.execute(
            "INSERT INTO part_ledger (space_id, part_id, at, delta, done_after, source, job_id, actor, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![space, part, now(), after - done, after, source, job, actor, note],
        )?;
        tx.commit()?;
        self.changed();
        Ok(Some(after))
    }

    pub fn ledger(&self, space: i64, part: i64) -> rusqlite::Result<Vec<LedgerLine>> {
        let mut q = self.db.prepare(
            "SELECT at, delta, done_after, source, job_id, actor, note FROM part_ledger
             WHERE space_id = ?1 AND part_id = ?2 ORDER BY at DESC, id DESC LIMIT 200",
        )?;
        q.query_map(params![space, part], |r| {
            Ok(LedgerLine {
                at: r.get(0)?,
                delta: r.get(1)?,
                done_after: r.get(2)?,
                source: r.get(3)?,
                job_id: r.get(4)?,
                actor: r.get(5)?,
                note: r.get(6)?,
            })
        })?
        .collect()
    }

    /// Someone confirms the plate waiting on a machine: `good` parts are good (0: a bad
    /// print). Counts it once, releases the machine. Refused when nothing waits there, or
    /// when what waits is another job than the one the page showed.
    pub fn confirm_plate(
        &mut self,
        access: &Access,
        machine: &str,
        job: i64,
        good: i64,
        note: &str,
    ) -> rusqlite::Result<Result<i64, &'static str>> {
        let Some(waiting) = self.waiting_confirm(machine)? else {
            return Ok(Err("nothing waits to be confirmed on this machine"));
        };
        if waiting.job != job {
            return Ok(Err("another plate waits there now: reload the page"));
        }
        let part: i64 =
            self.db
                .query_row("SELECT part_id FROM jobs WHERE id = ?1", [job], |r| {
                    r.get(0)
                })?;
        let good = good.clamp(0, 10_000);
        // Confirmed first, in the same breath as the count: a second click finds it done.
        let n = self.db.execute(
            "UPDATE jobs SET confirmed_at = ?2, confirmed_good = ?3, confirmed_by = ?4, taken_at = COALESCE(taken_at, ?2)
             WHERE id = ?1 AND confirmed_at IS NULL",
            params![job, now(), good, access.actor],
        )?;
        if n == 0 {
            return Ok(Err("it was confirmed meanwhile"));
        }
        let source = if good == 0 { "bad print" } else { "confirmed" };
        let after = self
            .adjust_part(
                access.space,
                part,
                PartChange::By(good),
                Why {
                    source,
                    job: Some(job),
                    actor: &access.actor,
                    note,
                },
            )?
            .unwrap_or(0);
        Ok(Ok(after))
    }

    /// The next plate a free machine suits, if any (the rule in one place).
    /// The next plate a free machine suits, if any: the rule in one place, for the dispatcher
    /// (nothing planned) and the schedule (plates it has planned already, per part).
    fn candidate_with(
        &self,
        space: i64,
        machine: &crate::store::Machine,
        planned: &std::collections::HashMap<i64, i64>,
    ) -> rusqlite::Result<Option<Candidate>> {
        let Some(profile) = machine.profile.as_deref() else {
            return Ok(None);
        };
        let mut q = self.db.prepare(
            "SELECT p.id, pf.id, pf.file_id, pf.per_plate, p.target, p.done, pf.material, pf.color, pf.sheet
             FROM parts p JOIN projects pr ON pr.id = p.project_id
             JOIN part_files pf ON pf.part_id = p.id AND pf.profile = ?2
             JOIN files f ON f.id = pf.file_id AND f.space_id = p.space_id
             WHERE p.space_id = ?1 AND pr.state = 'active'
             ORDER BY pr.priority, pr.created_at, p.sort_order, p.created_at, p.id",
        )?;
        let rows: Vec<(i64, i64, i64, i64, PartFile)> = q
            .query_map(params![space, profile], |r| {
                Ok((
                    r.get(0)?,
                    r.get::<_, i64>(3)?.max(1),
                    r.get(4)?,
                    r.get(5)?,
                    PartFile {
                        id: r.get(1)?,
                        profile: profile.to_owned(),
                        file_id: r.get(2)?,
                        file_name: String::new(),
                        per_plate: r.get::<_, i64>(3)?.max(1),
                        material: r.get(6)?,
                        color: r.get(7)?,
                        sheet: r.get(8)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (part, per_plate, target, done, file) in rows {
            let (_, under_way) = self.under_way(part)?;
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

    /// One round of the dispatcher, over every space: each machine that takes work, online,
    /// free, in use, with a clear bed and nothing waiting for someone, gets its next plate.
    pub fn dispatch(&mut self) -> rusqlite::Result<usize> {
        let spaces: Vec<i64> = {
            let mut q = self.db.prepare(
                "SELECT DISTINCT space_id FROM machines WHERE farm = 1 AND space_id IS NOT NULL",
            )?;
            q.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut sent = 0;
        for space in spaces {
            for machine in self.machines(space)? {
                if !self.takes_work(&machine.id)? || !machine.online || machine.out_of_use.is_some()
                {
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
                    || self.waiting_confirm(&machine.id)?.is_some()
                {
                    continue;
                }
                let Some(c) = self.candidate_with(space, &machine, &Default::default())? else {
                    continue;
                };
                let Some(file) = self.file(space, &c.file_id)? else {
                    continue;
                };
                if crate::files::fit(&file, &machine).is_err() {
                    continue;
                }
                let by = match self.part_names(c.part) {
                    (_, Some(project)) => format!("queue · {project}"),
                    _ => "queue".to_owned(),
                };
                if let Ok(job) = self.add_job(space, &machine.id, &file, None, Some(&by))? {
                    self.db.execute(
                        "UPDATE jobs SET part_id = ?2, per_plate = ?3 WHERE id = ?1",
                        params![job, c.part, c.per_plate],
                    )?;
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

impl Store {
    /// A machine's upkeep, when a service interval is set.
    pub fn upkeep(&self, machine: &str) -> rusqlite::Result<Option<Upkeep>> {
        let found: Option<(Option<f64>, Option<i64>)> = self
            .db
            .query_row(
                "SELECT service_every_h, serviced_at FROM machines WHERE id = ?1",
                [machine],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((Some(every_h), since)) = found else {
            return Ok(None);
        };
        let seconds: i64 = self.db.query_row(
            "SELECT COALESCE(SUM(COALESCE(ended_at, ?3) - started_at), 0) FROM jobs
             WHERE machine_id = ?1 AND started_at IS NOT NULL AND COALESCE(ended_at, ?3) > ?2",
            params![machine, since.unwrap_or(0), now()],
            |r| r.get(0),
        )?;
        let since_h = seconds.max(0) as f64 / 3600.0;
        Ok(Some(Upkeep {
            every_h,
            since_h,
            due: since_h >= every_h,
        }))
    }

    pub fn set_upkeep(
        &mut self,
        space: i64,
        machine: &str,
        every_h: Option<f64>,
    ) -> rusqlite::Result<bool> {
        let n = self.db.execute(
            "UPDATE machines SET service_every_h = ?3 WHERE id = ?1 AND space_id = ?2",
            params![machine, space, every_h],
        )?;
        self.changed();
        Ok(n > 0)
    }

    /// The service was done: the count starts again.
    pub fn serviced(&mut self, space: i64, machine: &str) -> rusqlite::Result<bool> {
        let n = self.db.execute(
            "UPDATE machines SET serviced_at = ?3 WHERE id = ?1 AND space_id = ?2",
            params![machine, space, now()],
        )?;
        self.changed();
        Ok(n > 0)
    }

    /// The build plate on a machine (`Textured PEI`), as someone said.
    pub fn sheet_of(&self, machine: &str) -> rusqlite::Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT sheet FROM machines WHERE id = ?1", [machine], |r| {
                r.get(0)
            })
            .optional()?
            .flatten())
    }

    pub fn set_sheet(
        &mut self,
        space: i64,
        machine: &str,
        sheet: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let n = self.db.execute(
            "UPDATE machines SET sheet = ?3 WHERE id = ?1 AND space_id = ?2",
            params![machine, space, sheet],
        )?;
        self.changed();
        Ok(n > 0)
    }

    /// Parts that people confirmed good from a machine's plates.
    pub fn parts_made(&self, machine: &str) -> rusqlite::Result<i64> {
        self.db.query_row(
            "SELECT COALESCE(SUM(confirmed_good), 0) FROM jobs WHERE machine_id = ?1",
            [machine],
            |r| r.get(0),
        )
    }
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

impl Store {
    /// The names of a part and its project.
    fn part_names(&self, part: i64) -> (Option<String>, Option<String>) {
        self.db
            .query_row(
                "SELECT p.name, pr.name FROM parts p JOIN projects pr ON pr.id = p.project_id WHERE p.id = ?1",
                [part],
                |r| Ok((Some(r.get(0)?), Some(r.get(1)?))),
            )
            .unwrap_or((None, None))
    }

    /// What the machines that take work will print, and when: worked out from the jobs running
    /// and the dispatcher's own rule ([`Store::candidate_with`]), without changing anything.
    /// print-farm-manager's projection: a schedule that disagrees with dispatch is worse than
    /// none, so both use the same rule.
    pub fn schedule(&self, space: i64) -> rusqlite::Result<Vec<Lane>> {
        let at = now();
        let mut lanes = Vec::new();
        // Per lane: when it is free, and whether it can be planned.
        let mut free: Vec<(usize, i64, crate::store::Machine)> = Vec::new();
        for machine in self.machines(space)? {
            if !self.takes_work(&machine.id)? {
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
                        let file = self.file(space, &job.file_id).ok().flatten()?;
                        let whole = file.meta.estimated_s? as i64;
                        Some((whole - (at - job.started_at.unwrap_or(at))).max(60))
                    })
                    .unwrap_or(UNKNOWN_PLATE_SECONDS);
                let part: Option<i64> = self
                    .db
                    .query_row("SELECT part_id FROM jobs WHERE id = ?1", [job.id], |r| {
                        r.get(0)
                    })
                    .ok()
                    .flatten();
                let (part, project) = part.map_or((None, None), |p| self.part_names(p));
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
            let Some(c) = self.candidate_with(space, machine, &planned)? else {
                free.remove(next);
                continue;
            };
            let length = self
                .file(space, &c.file_id)?
                .and_then(|f| f.meta.estimated_s)
                .map_or(UNKNOWN_PLATE_SECONDS, |s| s as i64)
                .max(60);
            let (part, project) = self.part_names(c.part);
            let label = self
                .file(space, &c.file_id)?
                .map(|f| f.name)
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
    pub fn compatible(
        &self,
        space: i64,
        machine: &crate::store::Machine,
    ) -> rusqlite::Result<(i64, i64)> {
        let open: Vec<(i64, i64, i64)> = {
            let mut q = self.db.prepare(
                "SELECT p.id, p.target, p.done FROM parts p JOIN projects pr ON pr.id = p.project_id
                 WHERE p.space_id = ?1 AND pr.state = 'active'",
            )?;
            q.query_map([space], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut can = 0;
        let mut all = 0;
        for (part, target, done) in open {
            if done >= target {
                continue;
            }
            all += 1;
            if self
                .part_files(space, part)?
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
pub async fn run(store: Shared) {
    let mut tick = tokio::time::interval(DISPATCH_EVERY);
    loop {
        tick.tick().await;
        if let Err(error) = store.lock().unwrap().dispatch() {
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

pub fn routes() -> Router<Shared> {
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

async fn schedule(access: Access, State(store): State<Shared>) -> Response {
    match store.lock().unwrap().schedule(access.space) {
        Ok(lanes) => Json(json!({ "now": now(), "lanes": lanes })).into_response(),
        Err(error) => failed(error),
    }
}

async fn list(access: Access, State(store): State<Shared>) -> Response {
    let store = store.lock().unwrap();
    let machines: Vec<Value> = match store.machines(access.space) {
        Ok(ms) => ms
            .iter()
            .map(|m| {
                json!({
                    "id": m.id, "name": m.name, "profile": m.profile, "kind": m.kind,
                    "farm": store.takes_work(&m.id).unwrap_or(false),
                    "sheet": m.sheet,
                    "compatible": store.compatible(access.space, m).ok(),
                    "manage": store.machine_level(&access, &m.id).ok().flatten().is_some_and(|l| l >= Level::Manage),
                })
            })
            .collect(),
        Err(error) => return failed(error),
    };
    match store.projects(access.space) {
        Ok(projects) => Json(json!({ "projects": projects, "machines": machines, "admin": access.role >= Role::Admin })).into_response(),
        Err(error) => failed(error),
    }
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
    State(store): State<Shared>,
    Json(body): Json<ProjectBody>,
) -> Response {
    let Some(name) = body.name.as_deref().and_then(|n| clean(n, 80)) else {
        return bad_request("a project needs a name of at most 80 characters");
    };
    let store = store.lock().unwrap();
    let made = store.db.execute(
        "INSERT INTO projects (space_id, name, description, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            access.space,
            name,
            body.description.unwrap_or_default().trim(),
            now()
        ],
    );
    match made {
        Ok(_) => (
            StatusCode::CREATED,
            Json(json!({ "id": store.db.last_insert_rowid() })),
        )
            .into_response(),
        Err(error) => failed(error),
    }
}

async fn change_project(
    access: Access,
    State(store): State<Shared>,
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
    let store = store.lock().unwrap();
    let n = store.db.execute(
        "UPDATE projects SET name = COALESCE(?3, name), description = COALESCE(?4, description),
             state = COALESCE(?5, state), priority = COALESCE(?6, priority)
         WHERE id = ?1 AND space_id = ?2",
        params![
            id,
            access.space,
            name,
            body.description,
            body.state,
            body.priority
        ],
    );
    match n {
        Ok(0) => (StatusCode::NOT_FOUND, "no such project").into_response(),
        Ok(_) => {
            store.changed();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => failed(error),
    }
}

async fn remove_project(
    access: Access,
    State(store): State<Shared>,
    Path(id): Path<i64>,
) -> Response {
    let store = store.lock().unwrap();
    let busy: i64 = store
        .db
        .query_row(
            "SELECT COUNT(*) FROM jobs j JOIN parts p ON p.id = j.part_id WHERE p.project_id = ?1
               AND (j.state IN ('queued', 'printing', 'paused', 'stopping') OR (j.state IN ('finished', 'stopped', 'failed') AND j.confirmed_at IS NULL))",
            [id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if busy > 0 {
        return (
            StatusCode::CONFLICT,
            "plates of it are on a bed or wait for someone: pause it, and remove it after",
        )
            .into_response();
    }
    let removed = store
        .db
        .execute("DELETE FROM projects WHERE id = ?1 AND space_id = ?2", params![id, access.space])
        .and_then(|n| {
            store.db.execute(
                "DELETE FROM part_files WHERE part_id IN (SELECT id FROM parts WHERE project_id = ?1 AND space_id = ?2)",
                params![id, access.space],
            )?;
            store.db.execute("DELETE FROM parts WHERE project_id = ?1 AND space_id = ?2", params![id, access.space])?;
            Ok(n)
        });
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
    State(store): State<Shared>,
    Path(project): Path<i64>,
    Json(body): Json<PartBody>,
) -> Response {
    let Some(name) = body.name.as_deref().and_then(|n| clean(n, 80)) else {
        return bad_request("a part needs a name of at most 80 characters");
    };
    let Some(target) = body.target.filter(|t| (1..=1_000_000).contains(t)) else {
        return bad_request("how many: 1 or more");
    };
    let store = store.lock().unwrap();
    let in_space: bool = store
        .db
        .query_row(
            "SELECT COUNT(*) > 0 FROM projects WHERE id = ?1 AND space_id = ?2",
            params![project, access.space],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !in_space {
        return (StatusCode::NOT_FOUND, "no such project").into_response();
    }
    match store.db.execute(
        "INSERT INTO parts (project_id, space_id, name, target, sort_order, created_at)
         VALUES (?1, ?2, ?3, ?4, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM parts WHERE project_id = ?1), ?5)",
        params![project, access.space, name, target, now()],
    ) {
        Ok(_) => (StatusCode::CREATED, Json(json!({ "id": store.db.last_insert_rowid() }))).into_response(),
        Err(error) => failed(error),
    }
}

async fn change_part(
    access: Access,
    State(store): State<Shared>,
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
    let store = store.lock().unwrap();
    match store.db.execute(
        "UPDATE parts SET name = COALESCE(?3, name), target = COALESCE(?4, target), sort_order = COALESCE(?5, sort_order)
         WHERE id = ?1 AND space_id = ?2",
        params![id, access.space, name, body.target, body.sort_order],
    ) {
        Ok(0) => (StatusCode::NOT_FOUND, "no such part").into_response(),
        Ok(_) => {
            store.changed();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => failed(error),
    }
}

async fn remove_part(access: Access, State(store): State<Shared>, Path(id): Path<i64>) -> Response {
    let store = store.lock().unwrap();
    match store.under_way(id) {
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
    let removed = store
        .db
        .execute(
            "DELETE FROM parts WHERE id = ?1 AND space_id = ?2",
            params![id, access.space],
        )
        .and_then(|n| {
            store.db.execute(
                "DELETE FROM part_files WHERE part_id = ?1 AND space_id = ?2",
                params![id, access.space],
            )?;
            Ok(n)
        });
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
    State(store): State<Shared>,
    Path(part): Path<i64>,
    Json(body): Json<FileBody>,
) -> Response {
    if crate::profiles::find(&body.profile).is_none() {
        return bad_request("no such machine model");
    }
    let store = store.lock().unwrap();
    let file = match store.file(access.space, &body.file) {
        Ok(Some(f)) => f,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such file").into_response(),
        Err(error) => return failed(error),
    };
    let per_plate = body
        .per_plate
        .or_else(|| per_plate_from_name(&file.name))
        .unwrap_or(1)
        .clamp(1, 1000);
    let in_space: bool = store
        .db
        .query_row(
            "SELECT COUNT(*) > 0 FROM parts WHERE id = ?1 AND space_id = ?2",
            params![part, access.space],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !in_space {
        return (StatusCode::NOT_FOUND, "no such part").into_response();
    }
    let blank = |v: Option<String>| v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    match store.db.execute(
        "INSERT INTO part_files (part_id, space_id, profile, file_id, per_plate, material, color, created_at, sheet)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (part_id, profile) DO UPDATE SET file_id = excluded.file_id, per_plate = excluded.per_plate,
             material = excluded.material, color = excluded.color, sheet = excluded.sheet",
        params![part, access.space, body.profile, file.id, per_plate, blank(body.material), blank(body.color), now(), blank(body.sheet)],
    ) {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

async fn remove_file(
    access: Access,
    State(store): State<Shared>,
    Path((part, file)): Path<(i64, i64)>,
) -> Response {
    match store.lock().unwrap().db.execute(
        "DELETE FROM part_files WHERE id = ?1 AND part_id = ?2 AND space_id = ?3",
        params![file, part, access.space],
    ) {
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
    State(store): State<Shared>,
    Path(part): Path<i64>,
    Json(body): Json<CountBody>,
) -> Response {
    let note = body.note.unwrap_or_default();
    if note.trim().is_empty() {
        return bad_request("say why: it goes into the ledger");
    }
    match store.lock().unwrap().adjust_part(
        access.space,
        part,
        PartChange::To(body.done),
        Why {
            source: "corrected",
            job: None,
            actor: &access.actor,
            note: note.trim(),
        },
    ) {
        Ok(Some(done)) => Json(json!({ "done": done })).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no such part").into_response(),
        Err(error) => failed(error),
    }
}

async fn ledger(access: Access, State(store): State<Shared>, Path(part): Path<i64>) -> Response {
    match store.lock().unwrap().ledger(access.space, part) {
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
    State(store): State<Shared>,
    Path(machine): Path<String>,
    Form(form): Form<FarmForm>,
) -> Response {
    match store
        .lock()
        .unwrap()
        .set_takes_work(access.space, &machine, form.on.is_some())
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
    State(store): State<Shared>,
    Path(machine): Path<String>,
    Form(form): Form<SheetForm>,
) -> Response {
    let sheet: Option<String> = form
        .sheet
        .map(|s| s.trim().chars().take(40).collect::<String>())
        .filter(|s| !s.is_empty());
    match store
        .lock()
        .unwrap()
        .set_sheet(access.space, &machine, sheet.as_deref())
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
    State(store): State<Shared>,
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
    match store
        .lock()
        .unwrap()
        .set_upkeep(access.space, &machine, every)
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

async fn serviced(
    access: Access,
    State(store): State<Shared>,
    Path(machine): Path<String>,
) -> Response {
    match store.lock().unwrap().serviced(access.space, &machine) {
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
    State(store): State<Shared>,
    Path(machine): Path<String>,
    Form(form): Form<ReadyForm>,
) -> Response {
    confirm(access, store, machine, form, false)
}

/// Bad print: nothing good came off; the plate goes back in the queue by itself.
async fn bad(
    access: Access,
    State(store): State<Shared>,
    Path(machine): Path<String>,
    Form(form): Form<ReadyForm>,
) -> Response {
    confirm(access, store, machine, form, true)
}

fn confirm(access: Access, store: Shared, machine: String, form: ReadyForm, bad: bool) -> Response {
    let mut store = store.lock().unwrap();
    let per_plate = match store.waiting_confirm(&machine) {
        Ok(Some(w)) => w.per_plate,
        Ok(None) => {
            return (
                StatusCode::CONFLICT,
                "nothing waits to be confirmed on this machine",
            )
                .into_response();
        }
        Err(error) => return failed(error),
    };
    let good = if bad {
        0
    } else {
        form.good.unwrap_or(per_plate)
    };
    let done = store.confirm_plate(
        &access,
        &machine,
        form.job,
        good,
        form.note.as_deref().unwrap_or(""),
    );
    match done {
        Ok(Ok(count)) => {
            // A Prusa on its own Connect: ready on its screen too.
            if let Ok(Some(m)) = store.machine(access.space, &machine)
                && m.link == "prusa-connect"
            {
                let _ = store.queue_connect_command(
                    access.space,
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
        // SQL in the source: string literals, each a line starting with a quote.
        let mut writers = 0;
        for source in crate::access::route_sources() {
            for line in source.lines().filter(|l| l.trim_start().starts_with('"')) {
                let l = line.replace(' ', "");
                if l.contains("UPDATEparts") && l.contains("done=") {
                    assert!(
                        l.contains("SETdone=?2WHEREid=?1"),
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
    use maker_connect_protocol::{MachineState, Telemetry};
    use std::sync::{Arc, Mutex};
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
    #[tokio::test]
    async fn a_project_over_many_printers() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
        let mk3s = crate::profiles::find("prusa/mk3s").unwrap();
        let (space, a, b, c) = {
            let mut s = store.lock().unwrap();
            let space = s.default_space().unwrap().0;
            let a = s.add_virtual(space, mk3s).unwrap();
            let b = s.add_virtual(space, mk3s).unwrap();
            let c = s.add_virtual(space, mk3s).unwrap();
            s.set_takes_work(space, &a, true).unwrap();
            s.set_takes_work(space, &b, true).unwrap();
            (space, a, b, c)
        };
        let idle = |machine: &str| {
            let mut s = store.lock().unwrap();
            let token: String =
                s.db.query_row("SELECT token FROM machines WHERE id = ?1", [machine], |r| {
                    r.get(0)
                })
                .unwrap();
            s.report(
                &token,
                &Telemetry {
                    state: MachineState::Idle,
                    ..Telemetry::default()
                },
            )
            .unwrap();
        };
        for m in [&a, &b, &c] {
            idle(m);
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
        let waiting = || store.lock().unwrap().projects(space).unwrap()[0].parts[0].clone();
        assert_eq!(waiting().files[0].per_plate, 10, "from the file's name");
        assert_eq!(waiting().waiting.as_deref(), Some("the project is draft"));
        // Draft: nothing goes out. Active: a plate to each machine that takes work.
        assert_eq!(store.lock().unwrap().dispatch().unwrap(), 0);
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
            let s = store.lock().unwrap();
            let lanes = s.schedule(space).unwrap();
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
            let ma = s
                .machines(space)
                .unwrap()
                .into_iter()
                .find(|m| m.id == a)
                .unwrap();
            assert_eq!(s.compatible(space, &ma).unwrap(), (1, 1));
            assert!(
                s.active_job(&a).unwrap().is_none(),
                "looking ahead starts nothing"
            );
        }
        assert_eq!(store.lock().unwrap().dispatch().unwrap(), 2);
        let job_on = |m: &str| store.lock().unwrap().active_job(m).unwrap();
        assert!(
            job_on(&a).is_some() && job_on(&b).is_some() && job_on(&c).is_none(),
            "not to a machine that takes no work"
        );
        assert_eq!(waiting().plates_under_way, 2);
        assert_eq!(
            job_on(&a).unwrap().by.as_deref(),
            Some("queue · Brackets for the hall"),
            "the queue started it, for that project"
        );
        // A finished, b failed: both hold their machine.
        let (ja, jb) = (job_on(&a).unwrap().id, job_on(&b).unwrap().id);
        store
            .lock()
            .unwrap()
            .set_job(ja, JobState::Finished, None)
            .unwrap();
        store
            .lock()
            .unwrap()
            .set_job(jb, JobState::Failed, Some("spaghetti"))
            .unwrap();
        idle(&a);
        idle(&b);
        // One plate still needed (25 > 0 + 10 under way on a): it waits, both machines held.
        assert_eq!(
            store.lock().unwrap().dispatch().unwrap(),
            0,
            "held: nothing new until someone confirms"
        );
        assert!(
            store
                .lock()
                .unwrap()
                .machine(space, &a)
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
        assert_eq!(store.lock().unwrap().dispatch().unwrap(), 2);
        let (ja, jb) = (job_on(&a).unwrap().id, job_on(&b).unwrap().id);
        for (m, j) in [(&a, ja), (&b, jb)] {
            store
                .lock()
                .unwrap()
                .set_job(j, JobState::Finished, None)
                .unwrap();
            idle(m);
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
        assert_eq!(waiting().done, 29);
        assert_eq!(store.lock().unwrap().dispatch().unwrap(), 0, "enough");
        assert!(waiting().waiting.is_none());
        // Every change is in the ledger, with who and why.
        let ledger = store.lock().unwrap().ledger(space, part).unwrap();
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
        assert_eq!(store.lock().unwrap().dispatch().unwrap(), 0);
        assert_eq!(
            waiting().waiting.as_deref(),
            Some("nothing with PETG loaded")
        );
    }
}

#[cfg(test)]
mod upkeep {
    use super::*;
    use maker_connect_protocol::{MachineState, Telemetry};

    /// A machine due for its service takes no plate; once serviced, it does again.
    #[test]
    fn a_part_that_wants_a_plate_waits_for_it() {
        let mut s = Store::open(":memory:").unwrap();
        let space = s.default_space().unwrap().0;
        let m = s
            .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
            .unwrap();
        s.set_takes_work(space, &m, true).unwrap();
        s.db.execute(
            "INSERT INTO files (id, space_id, name, size, uploaded_at, meta) VALUES ('f1', ?1, '4x_a.gcode', 10, 0, '{}')",
            [space],
        )
        .unwrap();
        s.db.execute("INSERT INTO projects (space_id, name, state, created_at) VALUES (?1, 'P', 'active', 0)", [space]).unwrap();
        s.db.execute("INSERT INTO parts (project_id, space_id, name, target, created_at) VALUES (1, ?1, 'A', 4, 0)", [space]).unwrap();
        s.db.execute(
            "INSERT INTO part_files (part_id, space_id, profile, file_id, per_plate, created_at, sheet) VALUES (1, ?1, 'prusa/mk3s', 'f1', 4, 0, 'Textured PEI')",
            [space],
        )
        .unwrap();
        let token: String =
            s.db.query_row("SELECT token FROM machines WHERE id = ?1", [&m], |r| {
                r.get(0)
            })
            .unwrap();
        s.report(
            &token,
            &Telemetry {
                state: MachineState::Idle,
                ..Telemetry::default()
            },
        )
        .unwrap();
        assert_eq!(s.dispatch().unwrap(), 0, "no plate said: not sent");
        let why = s.projects(space).unwrap()[0].parts[0].waiting.clone();
        assert_eq!(
            why.as_deref(),
            Some("no machine has a Textured PEI plate on")
        );
        s.set_sheet(space, &m, Some("Smooth PEI")).unwrap();
        assert_eq!(s.dispatch().unwrap(), 0, "the wrong plate");
        s.set_sheet(space, &m, Some("textured pei")).unwrap();
        assert_eq!(
            s.dispatch().unwrap(),
            1,
            "the right plate, however it is written"
        );
    }

    #[test]
    fn a_service_due_holds_the_queue() {
        let mut s = Store::open(":memory:").unwrap();
        let space = s.default_space().unwrap().0;
        let m = s
            .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
            .unwrap();
        s.set_takes_work(space, &m, true).unwrap();
        s.set_upkeep(space, &m, Some(1.0)).unwrap();
        // Two hours printed: due.
        s.db.execute(
            "INSERT INTO files (id, space_id, name, size, uploaded_at, meta) VALUES ('f1', ?1, '10x_a.gcode', 10, 0, '{}')",
            [space],
        )
        .unwrap();
        s.db.execute(
            "INSERT INTO jobs (space_id, machine_id, file_id, file_name, state, created_at, started_at, ended_at, taken_at)
             VALUES (?1, ?2, 'f1', 'old.gcode', 'finished', ?3, ?3, ?4, ?4)",
            params![space, m, now() - 7300, now() - 100],
        )
        .unwrap();
        let u = s.upkeep(&m).unwrap().unwrap();
        assert!(u.due && u.since_h >= 2.0, "{u:?}");
        // Work waiting for it.
        s.db.execute("INSERT INTO projects (space_id, name, state, created_at) VALUES (?1, 'P', 'active', 0)", [space]).unwrap();
        s.db.execute("INSERT INTO parts (project_id, space_id, name, target, created_at) VALUES (1, ?1, 'A', 10, 0)", [space]).unwrap();
        s.db.execute(
            "INSERT INTO part_files (part_id, space_id, profile, file_id, per_plate, created_at) VALUES (1, ?1, 'prusa/mk3s', 'f1', 10, 0)",
            [space],
        )
        .unwrap();
        let token: String =
            s.db.query_row("SELECT token FROM machines WHERE id = ?1", [&m], |r| {
                r.get(0)
            })
            .unwrap();
        s.report(
            &token,
            &Telemetry {
                state: MachineState::Idle,
                ..Telemetry::default()
            },
        )
        .unwrap();
        assert_eq!(s.dispatch().unwrap(), 0, "due: no plate");
        s.serviced(space, &m).unwrap();
        assert!(!s.upkeep(&m).unwrap().unwrap().due);
        assert_eq!(s.dispatch().unwrap(), 1, "serviced: work again");
    }
}
