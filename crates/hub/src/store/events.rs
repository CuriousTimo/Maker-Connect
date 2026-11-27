//! Events (design database.md "Events and history"): what happened in a space, kept a year. Today the
//! door log (part 7): a door opened or closed, a lock's Open pressed.

use super::{BoxError, Store};

/// The key of the one advisory lock under which `seq` is taken (decisions/20261119-event-order.md).
const EVENT_LOCK: i64 = 0x4d43_4556;

/// An event as a page shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub at: i64,
    pub kind: String,
    pub source: String,
    pub origin: Option<serde_json::Value>,
    pub data: serde_json::Value,
}

/// A door opened: `door.opened`; closed: `door.closed`; Open pressed: `lock.open`.
pub const DOOR_OPENED: &str = "door.opened";
pub const DOOR_CLOSED: &str = "door.closed";
pub const LOCK_OPEN: &str = "lock.open";

impl Store {
    /// Records an event of the space.
    pub async fn add_event(
        &self,
        space: i64,
        kind: &str,
        source: &str,
        origin: Option<serde_json::Value>,
        data: serde_json::Value,
    ) -> Result<(), BoxError> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(EVENT_LOCK)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO event (time, seq, space_id, type, source, origin, data)
             VALUES (now(), nextval('event_seq'), $1, $2, $3, $4, $5)",
        )
        .bind(space)
        .bind(kind)
        .bind(source)
        .bind(origin)
        .bind(data)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The space's newest events, of one source or of all, newest first.
    pub async fn events(
        &self,
        space: i64,
        source: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Event>, BoxError> {
        let mut tx = self.pg().space(space).await?;
        let rows: Vec<(
            i64,
            String,
            String,
            Option<serde_json::Value>,
            serde_json::Value,
        )> = sqlx::query_as(
            "SELECT unix_seconds(time), type, source, origin, data FROM event
                  WHERE space_id = $1 AND ($2::text IS NULL OR source = $2)
                  ORDER BY time DESC, seq DESC LIMIT $3",
        )
        .bind(space)
        .bind(source)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(|(at, kind, source, origin, data)| Event {
                at,
                kind,
                source,
                origin,
                data,
            })
            .collect())
    }
}
