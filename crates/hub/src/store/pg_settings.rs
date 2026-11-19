//! The columns of a machine's row that the modules set (hub core v2 step 2, lane 3b), on Postgres: its
//! access mode (crate::access), PPE (crate::safety), farm settings (crate::farm), who started a print
//! from its USB stick (crate::devices::printer_files) and the agent update asked for (crate::updates).
//! Space-keyed ones run in their space's transaction ([`pg::Pg::space`]).

use super::*;
use crate::store::pg::Bypass;

impl Store {
    /// One space-keyed column update; whether the machine is in the space.
    async fn set_column(
        &self,
        space: i64,
        update: sqlx::query::Query<'_, sqlx::Postgres, sqlx::postgres::PgArguments>,
    ) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        let n = update.execute(&mut *tx).await?.rows_affected();
        tx.commit().await?;
        self.changed();
        Ok(n > 0)
    }

    /// Open to every member (0), needs permission (1), or needs training (2).
    pub async fn set_access_mode(&self, space: i64, machine: &str, mode: u8) -> sqlx::Result<bool> {
        let mode = i32::from(mode.min(2));
        let q = sqlx::query!(
            "UPDATE machines SET induction_required = $3 WHERE id = $1 AND space_id = $2",
            machine,
            space,
            mode
        );
        self.set_column(space, q).await
    }

    /// Needs permission (true) or is open to every member (false).
    pub async fn set_induction_required(
        &self,
        space: i64,
        machine: &str,
        on: bool,
    ) -> sqlx::Result<bool> {
        self.set_access_mode(space, machine, u8::from(on)).await
    }

    /// Sets a machine's PPE (an empty list: nothing needed). Returns whether it is in the space.
    pub async fn set_ppe(&self, space: i64, machine: &str, list: &[String]) -> sqlx::Result<bool> {
        let joined = list.join(", ");
        let q = sqlx::query!(
            "UPDATE machines SET ppe = $3 WHERE id = $1 AND space_id = $2",
            machine,
            space,
            joined
        );
        let found = self.set_column(space, q).await?;
        // What was confirmed was for the old list.
        self.lock().unwrap().ppe_ok.retain(|(_, m), _| m != machine);
        Ok(found)
    }

    /// Whether a machine takes work from the queue (crate::farm).
    pub async fn set_takes_work(&self, space: i64, machine: &str, on: bool) -> sqlx::Result<bool> {
        let q = sqlx::query!(
            "UPDATE machines SET farm = $3 WHERE id = $1 AND space_id = $2",
            machine,
            space,
            on
        );
        self.set_column(space, q).await
    }

    /// A service every so many printing hours (None: no upkeep).
    pub async fn set_upkeep(
        &self,
        space: i64,
        machine: &str,
        every_h: Option<f64>,
    ) -> sqlx::Result<bool> {
        let q = sqlx::query!(
            "UPDATE machines SET service_every_h = $3 WHERE id = $1 AND space_id = $2",
            machine,
            space,
            every_h
        );
        self.set_column(space, q).await
    }

    /// The service was done: the count starts again.
    pub async fn serviced(&self, space: i64, machine: &str) -> sqlx::Result<bool> {
        let q = sqlx::query!(
            "UPDATE machines SET serviced_at = at_unix($3) WHERE id = $1 AND space_id = $2",
            machine,
            space,
            now()
        );
        self.set_column(space, q).await
    }

    /// The build plate on a machine (`Textured PEI`), as someone said.
    pub async fn set_sheet(
        &self,
        space: i64,
        machine: &str,
        sheet: Option<&str>,
    ) -> sqlx::Result<bool> {
        let q = sqlx::query!(
            "UPDATE machines SET sheet = $3 WHERE id = $1 AND space_id = $2",
            machine,
            space,
            sheet
        );
        self.set_column(space, q).await
    }

    /// Who started a print from the machine's USB stick on the website, and when (no job row: the
    /// file is the printer's).
    pub async fn set_usb_print_by(
        &self,
        space: i64,
        machine: &str,
        by: &str,
    ) -> sqlx::Result<bool> {
        let q = sqlx::query!(
            "UPDATE machines SET usb_print_by = $3, usb_print_at = at_unix($4)
             WHERE id = $1 AND space_id = $2",
            machine,
            space,
            by,
            now()
        );
        self.set_column(space, q).await
    }

    /// A print from the stick is over once the printer is idle again (a minute on: it may still be
    /// starting).
    pub async fn usb_print_over(&self, space: i64, machine: &str) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query!(
            "UPDATE machines SET usb_print_by = NULL, usb_print_at = NULL
             WHERE id = $1 AND usb_print_by IS NOT NULL AND usb_print_at < at_unix($2)",
            machine,
            now() - 60
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The agent update asked for a machine (by its agent's token): its id, the release tag, and
    /// when it was asked (Unix seconds, 0 when never).
    pub async fn agent_update_asked(
        &self,
        token: &str,
    ) -> sqlx::Result<Option<(String, Option<String>, i64)>> {
        let row = sqlx::query!(
            r#"SELECT id, agent_update_to, COALESCE(unix_seconds(agent_update_at), 0) AS "at!"
               FROM machines WHERE token = $1"#,
            token
        )
        .fetch_optional(self.pg().admin(Bypass::Secret))
        .await?;
        Ok(row.map(|r| (r.id, r.agent_update_to, r.at)))
    }

    /// The agent is on the release asked for, or said how its update went: asked no more.
    pub async fn agent_update_done(&self, token: &str) -> sqlx::Result<()> {
        // By the agent's token: its own row, maybe in no space yet.
        sqlx::query(
            "UPDATE machines SET agent_update_to = NULL, agent_update_at = NULL WHERE token = $1",
        )
        .bind(token)
        .execute(self.pg().admin(Bypass::Secret))
        .await?;
        Ok(())
    }

    /// Asks the agents of `machines` (in space `space`) to update to `tag`: how many were asked.
    pub async fn ask_agent_update(
        &self,
        space: i64,
        machines: &[String],
        tag: &str,
        at: i64,
    ) -> sqlx::Result<u64> {
        let mut tx = self.pg().space(space).await?;
        let mut asked = 0;
        for machine in machines {
            asked += sqlx::query!(
                "UPDATE machines SET agent_update_to = $1, agent_update_at = at_unix($2)
                 WHERE id = $3 AND space_id = $4 AND link = 'agent'",
                tag,
                at,
                machine,
                space
            )
            .execute(&mut *tx)
            .await?
            .rows_affected();
        }
        tx.commit().await?;
        Ok(asked)
    }
}
