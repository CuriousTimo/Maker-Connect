//! The store on Postgres (docs/plans/hub-core-v2.md §2.7): one migrator assembled from every module's
//! `migrations/` folder, and two pools, one per database role (§2.6, tenancy):
//!
//! - **app** (`maker_connect_app`, no BYPASSRLS, not the tables' owner): request work, in a transaction
//!   that says its space ([`Pg::space`]); row-level security lets only that space's rows through.
//! - **admin** (`maker_connect_admin`, BYPASSRLS): sign-in, session and token lookup, agent token lookup
//!   and instance admin, which cross spaces by nature.
//!
//! The hub connects as the owner (it runs the migrations) and gives each pooled connection its role with
//! `SET ROLE` as it opens.

use sqlx::migrate::{Migration, MigrationSource, Migrator};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::{Executor, Postgres, Transaction};
use std::sync::LazyLock;

/// Each module's migrations, as `sqlx::migrate!` embeds them. Versions are timestamps, unique across
/// modules; a module adds its folder here.
fn modules() -> Vec<&'static Migrator> {
    static CORE: Migrator = sqlx::migrate!("src/store/migrations");
    static ACCOUNTS: Migrator = sqlx::migrate!("src/accounts/migrations");
    static ACCESS: Migrator = sqlx::migrate!("src/access/migrations");
    static STOCK: Migrator = sqlx::migrate!("src/stock/migrations");
    static QUEUE: Migrator = sqlx::migrate!("src/queue/migrations");
    static FARM: Migrator = sqlx::migrate!("src/farm/migrations");
    vec![&CORE, &ACCOUNTS, &ACCESS, &STOCK, &QUEUE, &FARM]
}

/// All modules' migrations in one list, by version.
#[derive(Debug)]
struct Merged;

impl MigrationSource<'static> for Merged {
    fn resolve(
        self,
    ) -> futures_util::future::BoxFuture<'static, Result<Vec<Migration>, sqlx::error::BoxDynError>>
    {
        let mut all: Vec<Migration> = modules()
            .into_iter()
            .flat_map(|m| m.iter().cloned())
            .collect();
        all.sort_by_key(|m| m.version);
        Box::pin(std::future::ready(Ok(all)))
    }
}

/// The hub's one migrator (for `#[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]` too).
pub static MIGRATOR: LazyLock<Migrator> = LazyLock::new(|| {
    use futures_util::FutureExt;
    Migrator::new(Merged)
        .now_or_never()
        .expect("the merged source resolves at once")
        .expect("the embedded migrations resolve")
});

/// The two pools.
#[derive(Clone, Debug)]
pub struct Pg {
    pub(crate) app: PgPool,
    pub(crate) admin: PgPool,
}

pub const APP_ROLE: &str = "maker_connect_app";
pub const ADMIN_ROLE: &str = "maker_connect_admin";

fn pool(options: &PgConnectOptions, role: &'static str, max: u32) -> PgPool {
    PgPoolOptions::new()
        .max_connections(max)
        .after_connect(move |conn, _| {
            Box::pin(async move {
                conn.execute(sqlx::AssertSqlSafe(format!("SET ROLE {role}")))
                    .await?;
                Ok(())
            })
        })
        .connect_lazy_with(options.clone())
}

impl Pg {
    /// Connects to `url` as the owner, runs the migrations, and opens the two pools.
    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        let options: PgConnectOptions = url.parse()?;
        let owner = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await?;
        MIGRATOR.run(&owner).await?;
        owner.close().await;
        Ok(Self::with_options(&options, 8))
    }

    /// The pools over the database `pool` reaches (migrated already): for `#[sqlx::test]`.
    pub fn from_pool(pool: &PgPool) -> Self {
        Self::with_options(&pool.connect_options(), 3)
    }

    pub(crate) fn with_options(options: &PgConnectOptions, max: u32) -> Self {
        Self {
            app: pool(options, APP_ROLE, max),
            admin: pool(options, ADMIN_ROLE, max),
        }
    }

    /// A transaction on the app pool in `space`: `SET LOCAL app.space_id`, so row-level security shows
    /// that space's rows only. Commit it (dropping it rolls back).
    pub async fn space(&self, space: i64) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
        let mut tx = self.app.begin().await?;
        sqlx::query("SELECT set_config('app.space_id', $1, true)")
            .bind(space.to_string())
            .execute(&mut *tx)
            .await?;
        Ok(tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spike of step 2: one migrator from several modules' folders, in version order.
    #[test]
    fn the_modules_migrations_merge_in_version_order() {
        let versions: Vec<i64> = MIGRATOR.iter().map(|m| m.version).collect();
        assert!(versions.len() >= 2, "{versions:?}");
        assert!(versions.windows(2).all(|w| w[0] < w[1]), "{versions:?}");
    }

    /// Row-level security, as the app role: without a space no tenant row shows; with one, only its
    /// own; and a fresh transaction after one with a space sees none again.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_query_without_a_space_sees_no_tenant_rows(pool: PgPool) {
        // A second space beside the first, as the owner.
        sqlx::raw_sql(
            "INSERT INTO organisations (uid, name, created_at) VALUES ('o2', 'Other', at_unix(0));
             INSERT INTO spaces (name, uid, organisation_id)
                 SELECT 'Other', 's2', id FROM organisations WHERE uid = 'o2';
             INSERT INTO audit_log (space_id, at, actor, action) SELECT id, at_unix(0), 'a', 'x' FROM spaces",
        )
        .execute(&pool)
        .await
        .unwrap();
        // One connection per pool, so the fresh transaction below is on the same connection.
        let pg = Pg::with_options(&pool.connect_options(), 1);
        let role: String = sqlx::query_scalar("SELECT current_user::text")
            .fetch_one(&pg.app)
            .await
            .unwrap();
        assert_eq!(role, APP_ROLE);
        let bypass: bool =
            sqlx::query_scalar("SELECT rolbypassrls FROM pg_roles WHERE rolname = current_user")
                .fetch_one(&pg.app)
                .await
                .unwrap();
        assert!(!bypass);

        let count = |table: &'static str| format!("SELECT COUNT(*) FROM {table}");
        for table in [
            "spaces",
            "organisations",
            "audit_log",
            "memberships",
            "org_memberships",
            "invites",
            "api_tokens",
        ] {
            let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(count(table)))
                .fetch_one(&pg.app)
                .await
                .unwrap();
            assert_eq!(n, 0, "{table} without a space");
        }

        let mut tx = pg.space(1).await.unwrap();
        let seen: Vec<i64> = sqlx::query_scalar("SELECT space_id FROM audit_log")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert_eq!(seen, vec![1]);
        let spaces: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM spaces")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(spaces, 1);
        tx.commit().await.unwrap();
        // Writing into another space is refused.
        let mut tx = pg.space(1).await.unwrap();
        let other = sqlx::query(
            "INSERT INTO audit_log (space_id, at, actor, action) VALUES (2, at_unix(0), 'a', 'x')",
        )
        .execute(&mut *tx)
        .await;
        assert!(other.is_err());
        drop(tx);

        // The same connection again (one in the pool), in a fresh transaction: nothing.
        let mut tx = pg.app.begin().await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(n, 0);
        tx.commit().await.unwrap();

        // The admin role sees across spaces.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
            .fetch_one(&pg.admin)
            .await
            .unwrap();
        assert_eq!(n, 2);
    }

    /// Row-level security on lane 3's tables, as the app role: each space-owned table (machines, files,
    /// jobs, printer_files, printer_transfers) shows nothing without a space, only its space's rows with
    /// one, and refuses a row for another space.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn lane_3_tables_show_their_space_only(pool: PgPool) {
        // A second space, and a row of each table in both, as the owner.
        sqlx::raw_sql(
            "INSERT INTO organisations (uid, name, created_at) VALUES ('o2', 'Other', at_unix(0));
             INSERT INTO spaces (name, uid, organisation_id)
                 SELECT 'Other', 's2', id FROM organisations WHERE uid = 'o2';
             INSERT INTO machines (id, space_id, name, kind, agent_version, token, registered_at)
                 SELECT 'm' || id, id, 'M', '\"fdm\"', '0.1.0', 't' || id, at_unix(0) FROM spaces;
             INSERT INTO files (id, space_id, name, size, uploaded_at, meta)
                 SELECT 'f' || id, id, 'a.gcode', 1, at_unix(0), '{}' FROM spaces;
             INSERT INTO jobs (space_id, machine_id, file_id, file_name, state, created_at)
                 SELECT id, 'm' || id, 'f' || id, 'a.gcode', 'finished', at_unix(0) FROM spaces;
             INSERT INTO printer_files (machine_id, space_id, path) SELECT 'm' || id, id, '/usb' FROM spaces;
             INSERT INTO printer_transfers (iv, machine_id, space_id, file_id, key, size, path, created_at)
                 SELECT 'iv' || id, 'm' || id, id, 'f' || id, 'k', 1, '/usb/a.gcode', at_unix(0) FROM spaces;",
        )
        .execute(&pool)
        .await
        .unwrap();
        let pg = Pg::with_options(&pool.connect_options(), 1);
        let tables = [
            "machines",
            "files",
            "jobs",
            "printer_files",
            "printer_transfers",
        ];
        for table in tables {
            let count = format!("SELECT COUNT(*) FROM {table}");
            let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(count.clone()))
                .fetch_one(&pg.app)
                .await
                .unwrap();
            assert_eq!(n, 0, "{table} without a space");
            let mut tx = pg.space(1).await.unwrap();
            let seen: Vec<i64> =
                sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT space_id FROM {table}")))
                    .fetch_all(&mut *tx)
                    .await
                    .unwrap();
            assert_eq!(seen, vec![1], "{table} in space 1");
            tx.commit().await.unwrap();
            let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(count))
                .fetch_one(&pg.admin)
                .await
                .unwrap();
            assert_eq!(n, 2, "{table} as the admin role");
        }
        // Writing a row of another space is refused, for each table.
        let other = [
            "INSERT INTO machines (id, space_id, name, kind, agent_version, token, registered_at)
                 VALUES ('x', 2, 'X', '\"fdm\"', '0.1.0', 'tx', at_unix(0))",
            "INSERT INTO files (id, space_id, name, size, uploaded_at, meta) VALUES ('x', 2, 'x', 1, at_unix(0), '{}')",
            "INSERT INTO jobs (space_id, machine_id, file_id, file_name, state, created_at)
                 VALUES (2, 'm2', 'f2', 'x', 'queued', at_unix(0))",
            "INSERT INTO printer_files (machine_id, space_id, path) VALUES ('m2', 2, '/usb/x')",
            "INSERT INTO printer_transfers (iv, machine_id, space_id, file_id, key, size, path, created_at)
                 VALUES ('x', 'm2', 2, 'f2', 'k', 1, '/usb/x', at_unix(0))",
        ];
        for insert in other {
            let mut tx = pg.space(1).await.unwrap();
            let refused = sqlx::query(sqlx::AssertSqlSafe(insert))
                .execute(&mut *tx)
                .await;
            assert!(refused.is_err(), "{insert}");
        }
    }

    /// Row-level security on lane 4's tables (stock, Spoolman, queue, farm), as the app role: each
    /// space-owned table shows nothing without a space, only its space's rows with one, and refuses a
    /// row for another space.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn lane_4_tables_show_their_space_only(pool: PgPool) {
        // A second space, and a row of each table in both, as the owner.
        sqlx::raw_sql(
            "INSERT INTO organisations (uid, name, created_at) VALUES ('o2', 'Other', at_unix(0));
             INSERT INTO spaces (name, uid, organisation_id)
                 SELECT 'Other', 's2', id FROM organisations WHERE uid = 'o2';
             INSERT INTO locations (id, space_id, name) SELECT 'L-' || id, id, 'Shelf' FROM spaces;
             INSERT INTO movements (space_id, item, kind, at) SELECT id, 'S-0001', 'moved', at_unix(0) FROM spaces;
             INSERT INTO setting (space_id, key, value, last_updated) SELECT id, 'currency', 'EUR', at_unix(0) FROM spaces;
             INSERT INTO vendor (space_id, registered, name) SELECT id, at_unix(0), 'V' FROM spaces;
             INSERT INTO filament (space_id, registered) SELECT id, at_unix(0) FROM spaces;
             INSERT INTO spool (space_id, registered, filament_id) SELECT id, at_unix(0), 1 FROM spaces;
             INSERT INTO tag (space_id, uid, target_type, added) SELECT id, 'AA', 'spool', at_unix(0) FROM spaces;
             INSERT INTO extra_field (space_id, entity_type, key, definition) SELECT id, 'spool', 'k', '{}' FROM spaces;
             INSERT INTO machine_queue (space_id, machine_id, file_id, file_name, pos, added_at)
                 SELECT id, 'm', 'f', 'a.gcode', 1, at_unix(0) FROM spaces;
             INSERT INTO projects (space_id, name, created_at) SELECT id, 'P', at_unix(0) FROM spaces;
             INSERT INTO parts (project_id, space_id, name, target, created_at) SELECT id, id, 'A', 1, at_unix(0) FROM spaces;
             INSERT INTO part_files (part_id, space_id, profile, file_id, created_at)
                 SELECT id, id, 'prusa/mk3s', 'f', at_unix(0) FROM spaces;
             INSERT INTO part_ledger (space_id, part_id, at, delta, done_after, source, actor)
                 SELECT id, id, at_unix(0), 1, 1, 'corrected', 'x' FROM spaces;",
        )
        .execute(&pool)
        .await
        .unwrap();
        let pg = Pg::with_options(&pool.connect_options(), 1);
        let tables = [
            (
                "locations",
                "INSERT INTO locations (id, space_id, name) VALUES ('x', 2, 'x')",
            ),
            (
                "movements",
                "INSERT INTO movements (space_id, item, kind, at) VALUES (2, 'x', 'x', at_unix(0))",
            ),
            (
                "setting",
                "INSERT INTO setting (space_id, key, value, last_updated) VALUES (2, 'x', 'x', at_unix(0))",
            ),
            (
                "vendor",
                "INSERT INTO vendor (space_id, registered, name) VALUES (2, at_unix(0), 'x')",
            ),
            (
                "filament",
                "INSERT INTO filament (space_id, registered) VALUES (2, at_unix(0))",
            ),
            (
                "spool",
                "INSERT INTO spool (space_id, registered, filament_id) VALUES (2, at_unix(0), 1)",
            ),
            (
                "tag",
                "INSERT INTO tag (space_id, uid, target_type, added) VALUES (2, 'BB', 'spool', at_unix(0))",
            ),
            (
                "extra_field",
                "INSERT INTO extra_field (space_id, entity_type, key, definition) VALUES (2, 'x', 'x', '{}')",
            ),
            (
                "machine_queue",
                "INSERT INTO machine_queue (space_id, machine_id, file_id, file_name, pos, added_at)
                 VALUES (2, 'x', 'x', 'x', 1, at_unix(0))",
            ),
            (
                "projects",
                "INSERT INTO projects (space_id, name, created_at) VALUES (2, 'x', at_unix(0))",
            ),
            (
                "parts",
                "INSERT INTO parts (project_id, space_id, name, target, created_at) VALUES (2, 2, 'x', 1, at_unix(0))",
            ),
            (
                "part_files",
                "INSERT INTO part_files (part_id, space_id, profile, file_id, created_at)
                 VALUES (9, 2, 'x', 'x', at_unix(0))",
            ),
            (
                "part_ledger",
                "INSERT INTO part_ledger (space_id, part_id, at, delta, done_after, source, actor)
                 VALUES (2, 2, at_unix(0), 1, 1, 'x', 'x')",
            ),
        ];
        for (table, insert) in tables {
            let count = format!("SELECT COUNT(*) FROM {table}");
            let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(count.clone()))
                .fetch_one(&pg.app)
                .await
                .unwrap();
            assert_eq!(n, 0, "{table} without a space");
            let mut tx = pg.space(1).await.unwrap();
            let seen: Vec<i64> =
                sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT space_id FROM {table}")))
                    .fetch_all(&mut *tx)
                    .await
                    .unwrap();
            assert_eq!(seen, vec![1], "{table} in space 1");
            tx.commit().await.unwrap();
            let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(count))
                .fetch_one(&pg.admin)
                .await
                .unwrap();
            assert_eq!(n, 2, "{table} as the admin role");
            let mut tx = pg.space(1).await.unwrap();
            let refused = sqlx::query(sqlx::AssertSqlSafe(insert))
                .execute(&mut *tx)
                .await;
            assert!(refused.is_err(), "{insert}");
        }
        // Ids as SQLite's rowid: one past the highest in any space, also as the app role.
        let mut tx = pg.space(1).await.unwrap();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO vendor (space_id, registered, name) VALUES (1, at_unix(0), 'W') RETURNING id",
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(id, 3);
    }
}
