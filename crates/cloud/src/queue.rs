//! A machine's queue: jobs to print one after another, as on Prusa Connect. "Print on" a
//! machine that is busy (or offline, or with a print still on its bed) puts the file in its
//! queue; the first starts by itself once the machine is free and its bed was cleared (the
//! print taken off, the plate confirmed). People reorder it by dragging, and take things
//! out. A queued file becomes a job only when it starts, so the jobs stay as they were.
//!
//! The project queue (crate::farm) gives way: it sends work only to a machine with an empty
//! queue of its own.

use crate::access::Access;
use crate::store::{Store, now};
use crate::{Shared, failed};
use axum::{
    Form, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

pub fn migrate(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS machine_queue (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL,
             machine_id TEXT NOT NULL,
             file_id TEXT NOT NULL,
             file_name TEXT NOT NULL,
             by_name TEXT,
             pos REAL NOT NULL,
             added_at INTEGER NOT NULL,
             -- Why it cannot start (the file no longer suits the machine); empty: it can.
             problem TEXT
         );",
    )?;
    // Who queued it, as a person: they may move and take out their own.
    let present: bool = db.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('machine_queue') WHERE name = 'by_user'",
        [],
        |r| r.get::<_, i64>(0).map(|n| n > 0),
    )?;
    if !present {
        db.execute("ALTER TABLE machine_queue ADD COLUMN by_user INTEGER", [])?;
    }
    Ok(())
}

/// A file waiting in a machine's queue.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Queued {
    pub id: i64,
    pub file_id: String,
    pub file_name: String,
    pub by: Option<String>,
    /// Who queued it: they may move and take out their own; staff any.
    pub by_user: Option<i64>,
    pub added_at: i64,
    /// From the file: how long, and whether it has a thumbnail.
    pub estimated_s: Option<u64>,
    pub thumbnail: bool,
    pub problem: Option<String>,
}

impl Store {
    /// How the machine's last job ended, when it failed or was stopped and nobody said yet
    /// that the bed is clear: then its queue waits (what it left may still be there).
    pub fn bed_unchecked(&self, machine: &str) -> rusqlite::Result<Option<String>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .db
            .query_row(
                "SELECT state, taken_at FROM jobs WHERE machine_id = ?1 ORDER BY id DESC LIMIT 1",
                [machine],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .optional()?
            .filter(|(state, taken)| {
                taken.is_none() && matches!(state.as_str(), "failed" | "stopped")
            })
            .map(|(state, _)| state))
    }

    /// A machine's queue, first first.
    pub fn queue_of(&self, machine: &str) -> rusqlite::Result<Vec<Queued>> {
        let rows: Vec<Queued> = {
            let mut q = self.db.prepare(
                "SELECT id, file_id, file_name, by_name, added_at, problem, by_user FROM machine_queue
                 WHERE machine_id = ?1 ORDER BY pos, id",
            )?;
            q.query_map([machine], |r| {
                Ok(Queued {
                    id: r.get(0)?,
                    file_id: r.get(1)?,
                    file_name: r.get(2)?,
                    by: r.get(3)?,
                    added_at: r.get(4)?,
                    estimated_s: None,
                    thumbnail: false,
                    problem: r.get(5)?,
                    by_user: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        // What the files say of themselves, from the files table (the same space).
        Ok(rows
            .into_iter()
            .map(|mut q| {
                if let Ok(meta) =
                    self.db
                        .query_row("SELECT meta FROM files WHERE id = ?1", [&q.file_id], |r| {
                            r.get::<_, String>(0)
                        })
                    && let Ok(meta) = serde_json::from_str::<crate::gcode::FileMeta>(&meta)
                {
                    q.estimated_s = meta.estimated_s;
                    q.thumbnail = meta.thumbnail;
                }
                q
            })
            .collect())
    }

    /// Puts a file at the end of a machine's queue: its place (1 is next).
    pub fn queue_add(
        &mut self,
        space: i64,
        machine: &str,
        file: &crate::store::StoredFile,
        by: Option<(i64, &str)>,
    ) -> rusqlite::Result<usize> {
        let last: f64 = self.db.query_row(
            "SELECT COALESCE(MAX(pos), 0) FROM machine_queue WHERE machine_id = ?1",
            [machine],
            |r| r.get(0),
        )?;
        self.db.execute(
            "INSERT INTO machine_queue (space_id, machine_id, file_id, file_name, by_name, pos, added_at, by_user)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![space, machine, file.id, file.name, by.map(|b| b.1), last + 1.0, now(), by.map(|b| b.0)],
        )?;
        self.changed();
        let n: i64 = self.db.query_row(
            "SELECT COUNT(*) FROM machine_queue WHERE machine_id = ?1",
            [machine],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Takes a file out of a machine's queue.
    pub fn queue_remove(&mut self, space: i64, machine: &str, id: i64) -> rusqlite::Result<bool> {
        let n = self.db.execute(
            "DELETE FROM machine_queue WHERE id = ?1 AND machine_id = ?2 AND space_id = ?3",
            params![id, machine, space],
        )?;
        self.changed();
        Ok(n > 0)
    }

    /// A machine's queue in this order (ids not given keep their place after them).
    pub fn queue_order(&mut self, space: i64, machine: &str, ids: &[i64]) -> rusqlite::Result<()> {
        let tx = self.db.transaction()?;
        for (i, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE machine_queue SET pos = ?4 WHERE id = ?1 AND machine_id = ?2 AND space_id = ?3",
                params![id, machine, space, i as f64 + 1.0],
            )?;
        }
        tx.commit()?;
        self.changed();
        Ok(())
    }

    /// Starts what may start: per machine with a queue, its first file once the machine is
    /// free, its bed clear and its service not due. How many started.
    pub fn start_queued(&mut self) -> rusqlite::Result<usize> {
        let waiting: Vec<(i64, String)> = {
            let mut q = self
                .db
                .prepare("SELECT DISTINCT space_id, machine_id FROM machine_queue")?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut started = 0;
        for (space, machine_id) in waiting {
            let Some(machine) = self.machine(space, &machine_id)? else {
                // Gone, or moved to another space: its queue goes too.
                self.db.execute(
                    "DELETE FROM machine_queue WHERE machine_id = ?1 AND space_id = ?2",
                    params![machine_id, space],
                )?;
                continue;
            };
            if machine.upkeep.as_ref().is_some_and(|u| u.due) || machine.queue_held.is_some() {
                continue;
            }
            let Some(first) = self.queue_of(&machine_id)?.into_iter().next() else {
                continue;
            };
            let Some(file) = self.file(space, &first.file_id)? else {
                self.queue_remove(space, &machine_id, first.id)?;
                continue;
            };
            // It no longer suits the machine (its profile changed): it waits, with why.
            if let Err(why) = crate::files::suits(&file, &machine) {
                self.db.execute(
                    "UPDATE machine_queue SET problem = ?2 WHERE id = ?1",
                    params![first.id, why],
                )?;
                continue;
            }
            if crate::files::fit(&file, &machine).is_err() {
                continue;
            }
            if let Ok(_job) = self.add_job(space, &machine_id, &file, None, first.by.as_deref())? {
                self.db
                    .execute("DELETE FROM machine_queue WHERE id = ?1", [first.id])?;
                started += 1;
                println!("from the queue: {} on {}", file.name, machine.name);
            }
        }
        if started > 0 {
            self.changed();
        }
        Ok(started)
    }
}

/// Every few seconds: the next file of each queue, where it may start.
pub async fn run(store: Shared) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        if let Err(error) = store.lock().unwrap().start_queued() {
            eprintln!("queue: {error}");
        }
    }
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/machines/{id}/queue/order", post(order))
        .route("/machines/{id}/queue/{item}/remove", post(remove))
}

#[derive(Deserialize)]
struct OrderForm {
    /// The queue's ids in their new order, comma separated.
    ids: String,
}

/// Staff (admins, owners) move and take out anything in a queue; anyone else only what
/// they queued themselves.
fn staff(access: &Access) -> bool {
    access.role >= crate::access::Role::Admin
}

/// A new order. Someone who is not staff only swaps their own prints among themselves:
/// everyone else's stays where it was, so nobody gets ahead of others.
async fn order(
    access: Access,
    State(store): State<Shared>,
    Path(machine): Path<String>,
    Form(form): Form<OrderForm>,
) -> Response {
    let ids: Vec<i64> = form
        .ids
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let mut store = store.lock().unwrap();
    let now: Vec<Queued> = match store.queue_of(&machine) {
        Ok(q) => q,
        Err(error) => return failed(error),
    };
    let mut same: Vec<i64> = now.iter().map(|q| q.id).collect();
    let mut asked = ids.clone();
    same.sort_unstable();
    asked.sort_unstable();
    if same != asked {
        return (StatusCode::CONFLICT, "the queue changed: look again").into_response();
    }
    if !staff(&access) {
        let theirs = |id: i64| {
            now.iter()
                .find(|q| q.id == id)
                .is_some_and(|q| q.by_user != Some(access.user_id))
        };
        let moved = now
            .iter()
            .zip(&ids)
            .any(|(was, is)| (theirs(was.id) || theirs(*is)) && was.id != *is);
        if moved {
            return (
                StatusCode::FORBIDDEN,
                "you may change the order of your own prints only; staff move the others",
            )
                .into_response();
        }
    }
    match store.queue_order(access.space, &machine, &ids) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

/// Takes a print out of the queue: one's own, or (staff) anyone's.
async fn remove(
    access: Access,
    State(store): State<Shared>,
    Path((machine, item)): Path<(String, i64)>,
) -> Response {
    let mut store = store.lock().unwrap();
    if !staff(&access) {
        let own = store
            .queue_of(&machine)
            .ok()
            .and_then(|q| q.into_iter().find(|q| q.id == item))
            .is_some_and(|q| q.by_user == Some(access.user_id));
        if !own {
            return (
                StatusCode::FORBIDDEN,
                "you may take out your own prints only; staff take out the others",
            )
                .into_response();
        }
    }
    match store.queue_remove(access.space, &machine, item) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "not in its queue").into_response(),
        Err(error) => failed(error),
    }
}

#[cfg(test)]
mod tests {
    use crate::store::{JobState, Store};
    use maker_connect_protocol::{MachineState, Telemetry};

    /// One after another: the next starts once the machine is free; after a failed job only
    /// when someone said the bed is clear.
    #[test]
    fn a_queue_waits_for_a_clear_bed() {
        let mut s = Store::open(":memory:").unwrap();
        let space = s.default_space().unwrap().0;
        let m = s
            .add_virtual(space, crate::profiles::find("prusa/mk3s").unwrap())
            .unwrap();
        s.db.execute(
            "INSERT INTO files (id, space_id, name, size, uploaded_at, meta) VALUES ('f1', ?1, 'a.gcode', 10, 0, '{}')",
            [space],
        )
        .unwrap();
        let token: String =
            s.db.query_row("SELECT token FROM machines WHERE id = ?1", [&m], |r| {
                r.get(0)
            })
            .unwrap();
        let idle = |s: &mut Store| {
            s.report(
                &token,
                &Telemetry {
                    state: MachineState::Idle,
                    ..Telemetry::default()
                },
            )
            .unwrap();
        };
        idle(&mut s);
        let file = s.file(space, "f1").unwrap().unwrap();
        let job = s
            .add_job(space, &m, &file, None, Some("Timo"))
            .unwrap()
            .unwrap();
        assert_eq!(s.queue_add(space, &m, &file, Some((2, "Mia"))).unwrap(), 1);
        assert_eq!(s.queue_add(space, &m, &file, Some((2, "Mia"))).unwrap(), 2);
        assert_eq!(s.start_queued().unwrap(), 0, "busy");
        // It failed: what it left may be on the bed.
        s.set_job(job, JobState::Failed, Some("spaghetti")).unwrap();
        idle(&mut s);
        assert_eq!(s.start_queued().unwrap(), 0, "the bed first");
        let machine = s.machine(space, &m).unwrap().unwrap();
        assert_eq!(machine.queue_held.as_deref(), Some("failed"));
        // "Set Ready" on the printer's own menu says the same as "Bed clear" on its page.
        s.report(
            &token,
            &Telemetry {
                state: MachineState::Idle,
                bed_clear: true,
                ..Telemetry::default()
            },
        )
        .unwrap();
        assert_eq!(s.start_queued().unwrap(), 1, "bed clear: the next");
        let machine = s.machine(space, &m).unwrap().unwrap();
        assert_eq!(
            machine.job.unwrap().by.as_deref(),
            Some("Mia"),
            "who queued it"
        );
        assert_eq!(machine.queue.len(), 1);
        // Reordering and taking out.
        let left = machine.queue[0].id;
        assert!(s.queue_remove(space, &m, left).unwrap());
        assert!(s.machine(space, &m).unwrap().unwrap().queue.is_empty());
    }
}
