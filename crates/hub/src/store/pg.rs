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
    vec![&CORE, &ACCOUNTS, &ACCESS]
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
}
