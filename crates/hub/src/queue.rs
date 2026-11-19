//! A machine's queue: jobs to print one after another, as on Prusa Connect. "Print on" a
//! machine that is busy (or offline, or with a print still on its bed) puts the file in its
//! queue; the first starts by itself once the machine is free and its bed was cleared (the
//! print taken off, the plate confirmed). People reorder it by dragging, and take things
//! out. A queued file becomes a job only when it starts, so the jobs stay as they were.
//!
//! The project queue (crate::farm) gives way: it sends work only to a machine with an empty
//! queue of its own.

use crate::access::Access;
use crate::store::{BoxError, Work, now};
use crate::{Store, failed};
use axum::{
    Form, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};

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

/// A queue's row, as [`Queued`] reads it.
type QueueRow = (
    i64,
    String,
    String,
    String,
    Option<String>,
    i64,
    Option<String>,
    Option<i64>,
);

const QUEUE_SELECT: &str =
    "SELECT id, machine_id, file_id, file_name, by_name, unix_seconds(added_at),
         problem, by_user FROM machine_queue";

fn queued((id, _, file_id, file_name, by, added_at, problem, by_user): QueueRow) -> Queued {
    Queued {
        id,
        file_id,
        file_name,
        by,
        added_at,
        estimated_s: None,
        thumbnail: false,
        problem,
        by_user,
    }
}

impl Work {
    /// A machine's queue, first first. What the files say of themselves (`estimated_s`,
    /// `thumbnail`) [`crate::Store::machines`] fills in.
    pub(crate) async fn queue_of(&mut self, machine: &str) -> sqlx::Result<Vec<Queued>> {
        let rows: Vec<QueueRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{QUEUE_SELECT} WHERE machine_id = $1 ORDER BY pos, id"
        )))
        .bind(machine)
        .fetch_all(self.c())
        .await?;
        Ok(rows.into_iter().map(queued).collect())
    }

    /// The queues of these machines, by machine id.
    pub(crate) async fn queues(
        &mut self,
        machines: &[String],
    ) -> sqlx::Result<std::collections::HashMap<String, Vec<Queued>>> {
        let rows: Vec<QueueRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{QUEUE_SELECT} WHERE machine_id = ANY($1) ORDER BY pos, id"
        )))
        .bind(machines)
        .fetch_all(self.c())
        .await?;
        let mut out: std::collections::HashMap<String, Vec<Queued>> = Default::default();
        for row in rows {
            out.entry(row.1.clone()).or_default().push(queued(row));
        }
        Ok(out)
    }

    /// Puts a file at the end of a machine's queue: its place (1 is next).
    pub(crate) async fn queue_add(
        &mut self,
        machine: &str,
        file: &crate::store::StoredFile,
        by: Option<(i64, &str)>,
    ) -> sqlx::Result<usize> {
        let last: f64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(pos), 0) FROM machine_queue WHERE machine_id = $1",
        )
        .bind(machine)
        .fetch_one(self.c())
        .await?;
        sqlx::query(
            "INSERT INTO machine_queue (space_id, machine_id, file_id, file_name, by_name, pos, added_at, by_user)
             VALUES ($1, $2, $3, $4, $5, $6, at_unix($7), $8)",
        )
        .bind(self.space)
        .bind(machine)
        .bind(&file.id)
        .bind(&file.name)
        .bind(by.map(|b| b.1))
        .bind(last + 1.0)
        .bind(now())
        .bind(by.map(|b| b.0))
        .execute(self.c())
        .await?;
        self.changed();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM machine_queue WHERE machine_id = $1")
            .bind(machine)
            .fetch_one(self.c())
            .await?;
        Ok(n as usize)
    }

    /// Takes a file out of a machine's queue.
    pub(crate) async fn queue_remove(&mut self, machine: &str, id: i64) -> sqlx::Result<bool> {
        let n = sqlx::query(
            "DELETE FROM machine_queue WHERE id = $1 AND machine_id = $2 AND space_id = $3",
        )
        .bind(id)
        .bind(machine)
        .bind(self.space)
        .execute(self.c())
        .await?
        .rows_affected();
        self.changed();
        Ok(n > 0)
    }

    /// A machine's queue in this order (ids not given keep their place after them).
    pub(crate) async fn queue_order(&mut self, machine: &str, ids: &[i64]) -> sqlx::Result<()> {
        for (i, id) in ids.iter().enumerate() {
            sqlx::query(
                "UPDATE machine_queue SET pos = $4 WHERE id = $1 AND machine_id = $2 AND space_id = $3",
            )
            .bind(id)
            .bind(machine)
            .bind(self.space)
            .bind(i as f64 + 1.0)
            .execute(self.c())
            .await?;
        }
        self.changed();
        Ok(())
    }
}

impl Store {
    /// A machine's queue in a space (`Work::queue_of`).
    pub async fn queue_of(&self, space: i64, machine: &str) -> sqlx::Result<Vec<Queued>> {
        crate::stock::in_space!(self, space, |w| w.queue_of(machine)).await
    }

    /// `Work::queue_add`, in its own transaction.
    pub async fn queue_add(
        &self,
        space: i64,
        machine: &str,
        file: &crate::store::StoredFile,
        by: Option<(i64, &str)>,
    ) -> sqlx::Result<usize> {
        crate::stock::in_space!(self, space, |w| w.queue_add(machine, file, by)).await
    }

    /// `Work::queue_remove`, in its own transaction.
    pub async fn queue_remove(&self, space: i64, machine: &str, id: i64) -> sqlx::Result<bool> {
        crate::stock::in_space!(self, space, |w| w.queue_remove(machine, id)).await
    }
}

impl Store {
    /// "Reprint" chosen on the printer itself (Prusa's MK3, after a host's print): its last
    /// job again, as PrusaLink does, when it may start now (the same checks as a queued
    /// file: free, its bed clear, the file still suits it). Else the printer beeps and says
    /// "Cannot re-print" on its screen, as PrusaLink does. Returns whether it started.
    pub async fn reprint(&self, space: i64, machine_id: &str) -> Result<bool, BoxError> {
        let Some(machine) = self.machine(space, machine_id).await? else {
            return Ok(false);
        };
        let file = match self.last_job(space, machine_id).await? {
            Some(last) => self.file(space, &last.job().file_id).await?,
            None => None,
        };
        // Idle by its own report too: a print it runs itself is no job of the hub's.
        let idle = machine
            .telemetry
            .as_ref()
            .is_some_and(|t| t.state == maker_connect_protocol::MachineState::Idle);
        let started = match file {
            Some(file)
                if idle
                    && machine.kind != maker_connect_protocol::MachineKind::Laser
                    && crate::files::suits(&file, &machine).is_ok()
                    && crate::files::fit(&file, &machine).is_ok() =>
            {
                self.add_job(
                    space,
                    machine_id,
                    &file,
                    None,
                    crate::store::StartedBy::Auto {
                        by: Some("Reprint on the printer"),
                    },
                )
                .await?
                .is_ok()
            }
            _ => false,
        };
        Ok(started)
    }

    /// Starts what may start: per machine with a queue, its first file once the machine is
    /// free, its bed clear and its service not due. How many started.
    pub async fn start_queued(&self) -> Result<usize, BoxError> {
        // Each space's queues in its transaction (the spaces as instance admin, Store::space_ids).
        let mut waiting: Vec<(i64, String)> = Vec::new();
        for space in self.space_ids().await? {
            let mut tx = self.pg().space(space).await?;
            let machines: Vec<String> =
                sqlx::query_scalar("SELECT DISTINCT machine_id FROM machine_queue")
                    .fetch_all(&mut *tx)
                    .await?;
            tx.commit().await?;
            waiting.extend(machines.into_iter().map(|m| (space, m)));
        }
        let mut started = 0;
        for (space, machine_id) in waiting {
            let Some(machine) = self.machine(space, &machine_id).await? else {
                // Gone, or moved to another space: its queue goes too.
                let mut w = self.work(space).await?;
                sqlx::query("DELETE FROM machine_queue WHERE machine_id = $1 AND space_id = $2")
                    .bind(&machine_id)
                    .bind(space)
                    .execute(w.c())
                    .await?;
                w.commit(self).await?;
                continue;
            };
            // A laser's queue waits for someone at the laser (crate::store::StartedBy).
            if machine.kind == maker_connect_protocol::MachineKind::Laser {
                continue;
            }
            if machine.upkeep.as_ref().is_some_and(|u| u.due) || machine.queue_held.is_some() {
                continue;
            }
            let Some(first) = self.queue_of(space, &machine_id).await?.into_iter().next() else {
                continue;
            };
            let Some(file) = self.file(space, &first.file_id).await? else {
                self.queue_remove(space, &machine_id, first.id).await?;
                continue;
            };
            // It no longer suits the machine (its profile changed): it waits, with why.
            if let Err(why) = crate::files::suits(&file, &machine) {
                let mut w = self.work(space).await?;
                sqlx::query("UPDATE machine_queue SET problem = $2 WHERE id = $1")
                    .bind(first.id)
                    .bind(why)
                    .execute(w.c())
                    .await?;
                w.commit(self).await?;
                continue;
            }
            if crate::files::fit(&file, &machine).is_err() {
                continue;
            }
            if let Ok(_job) = self
                .add_job(
                    space,
                    &machine_id,
                    &file,
                    None,
                    crate::store::StartedBy::Auto {
                        by: first.by.as_deref(),
                    },
                )
                .await?
            {
                let mut w = self.work(space).await?;
                sqlx::query("DELETE FROM machine_queue WHERE id = $1")
                    .bind(first.id)
                    .execute(w.c())
                    .await?;
                w.commit(self).await?;
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
pub async fn run(store: Store) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        if let Err(error) = store.start_queued().await {
            eprintln!("queue: {error}");
        }
    }
}

pub fn routes() -> Router<Store> {
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
    State(store): State<Store>,
    Path(machine): Path<String>,
    Form(form): Form<OrderForm>,
) -> Response {
    let ids: Vec<i64> = form
        .ids
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let mut w = match store.work(access.space).await {
        Ok(w) => w,
        Err(error) => return failed(error),
    };
    let now: Vec<Queued> = match w.queue_of(&machine).await {
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
    match w.queue_order(&machine, &ids).await {
        Ok(()) => match w.commit(&store).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(error) => failed(error),
        },
        Err(error) => failed(error),
    }
}

/// Takes a print out of the queue: one's own, or (staff) anyone's.
async fn remove(
    access: Access,
    State(store): State<Store>,
    Path((machine, item)): Path<(String, i64)>,
) -> Response {
    let mut w = match store.work(access.space).await {
        Ok(w) => w,
        Err(error) => return failed(error),
    };
    if !staff(&access) {
        let own = w
            .queue_of(&machine)
            .await
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
    match w.queue_remove(&machine, item).await {
        Ok(true) => match w.commit(&store).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(error) => failed(error),
        },
        Ok(false) => (StatusCode::NOT_FOUND, "not in its queue").into_response(),
        Err(error) => failed(error),
    }
}

#[cfg(test)]
mod tests {
    use crate::store::JobState;
    use maker_connect_protocol::{MachineState, Telemetry};

    /// One after another: the next starts once the machine is free; after a failed job only
    /// when someone said the bed is clear.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_queue_waits_for_a_clear_bed(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let (space, m, token) = store.add_test_machine("prusa/mk3s", None).await;
        store
            .add_file(space, "f1", "a.gcode", 10, &Default::default())
            .await
            .unwrap();
        store.idle_test_machine(&m).await;
        let file = store.file(space, "f1").await.unwrap().unwrap();
        {
            let job = store
                .add_job(
                    space,
                    &m,
                    &file,
                    None,
                    crate::store::StartedBy::Auto { by: Some("Timo") },
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                store
                    .queue_add(space, &m, &file, Some((2, "Mia")))
                    .await
                    .unwrap(),
                1
            );
            assert_eq!(
                store
                    .queue_add(space, &m, &file, Some((2, "Mia")))
                    .await
                    .unwrap(),
                2
            );
            assert_eq!(store.start_queued().await.unwrap(), 0, "busy");
            // It failed: what it left may be on the bed.
            store
                .set_job(space, job, JobState::Failed, Some("spaghetti"))
                .await
                .unwrap();
        }
        store.idle_test_machine(&m).await;
        assert_eq!(store.start_queued().await.unwrap(), 0, "the bed first");
        let machine = store.machine(space, &m).await.unwrap().unwrap();
        assert_eq!(machine.queue_held.as_deref(), Some("failed"));
        // "Set Ready" on the printer's own menu says the same as "Bed clear" on its page.
        store
            .report(
                &token,
                &Telemetry {
                    state: MachineState::Idle,
                    bed_clear: true,
                    ..Telemetry::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            store.start_queued().await.unwrap(),
            1,
            "bed clear: the next"
        );
        let machine = store.machine(space, &m).await.unwrap().unwrap();
        assert_eq!(
            machine.job.unwrap().by.as_deref(),
            Some("Mia"),
            "who queued it"
        );
        assert_eq!(machine.queue.len(), 1);
        // Reordering and taking out.
        let left = machine.queue[0].id;
        assert!(store.queue_remove(space, &m, left).await.unwrap());
        assert!(
            store
                .machine(space, &m)
                .await
                .unwrap()
                .unwrap()
                .queue
                .is_empty()
        );
    }

    /// A laser job is started only by a person in a session: not by the queue, a reprint, the
    /// farm, nor a person without a session (an API token). Its queue waits.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_laser_never_starts_alone(pool: sqlx::PgPool) {
        use crate::store::{LASER_NEVER_ALONE, StartedBy};
        let store = crate::store::Store::for_test(&pool).await;
        let (space, m, _) = store.add_test_machine("prusa/mk3s", None).await;
        store
            .add_file(space, "f1", "a.gcode", 10, &Default::default())
            .await
            .unwrap();
        store.idle_test_machine(&m).await;
        let file = store.file(space, "f1").await.unwrap().unwrap();
        // As an FDM printer: the guard lets everything through.
        {
            let job = store
                .add_job(
                    space,
                    &m,
                    &file,
                    None,
                    StartedBy::Auto { by: Some("queue") },
                )
                .await
                .unwrap()
                .unwrap();
            store
                .set_job(space, job, JobState::Finished, None)
                .await
                .unwrap();
        }
        // The same machine, now a laser, taking work.
        sqlx::query("UPDATE machines SET kind = $2 WHERE id = $1")
            .bind(&m)
            .bind(serde_json::to_string(&maker_connect_protocol::MachineKind::Laser).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        assert!(store.is_laser(space, &m).await.unwrap());
        store.set_takes_work(space, &m, true).await.unwrap();
        for _ in 0..2 {
            store
                .queue_add(space, &m, &file, Some((2, "Mia")))
                .await
                .unwrap();
        }
        let s = &store;
        assert_eq!(
            s.start_queued().await.unwrap(),
            0,
            "the queue never starts a laser"
        );
        assert_eq!(s.dispatch().await.unwrap(), 0, "nor does the farm");
        assert!(
            !s.reprint(space, &m).await.unwrap(),
            "nor a reprint on the machine"
        );
        assert!(s.active_job(space, &m).await.unwrap().is_none());
        assert_eq!(
            s.queue_of(space, &m).await.unwrap().len(),
            2,
            "the copies wait"
        );
        // The guard itself: Auto, and a person without a session, are refused.
        assert_eq!(
            s.add_job(space, &m, &file, None, StartedBy::Auto { by: None })
                .await
                .unwrap(),
            Err(LASER_NEVER_ALONE)
        );
        assert_eq!(
            s.add_job(
                space,
                &m,
                &file,
                None,
                StartedBy::Person {
                    user: "Mia",
                    session: None
                }
            )
            .await
            .unwrap(),
            Err(LASER_NEVER_ALONE),
            "an API token has no session"
        );
        // A person in a session starts it.
        let job = s
            .add_job(
                space,
                &m,
                &file,
                None,
                StartedBy::Person {
                    user: "Mia",
                    session: Some(1),
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(s.active_job(space, &m).await.unwrap().unwrap().id, job);
        assert_eq!(
            s.queue_of(space, &m).await.unwrap().len(),
            2,
            "starting one leaves the copies waiting"
        );
    }
}
