//! The store on Postgres (docs/archive/plans/hub-core-v2.md §2.7): one migrator assembled from every module's
//! `migrations/` folder, and two pools, one per database role (§2.6, tenancy):
//!
//! - **app** (`maker_connect_app`, no BYPASSRLS, not the tables' owner): request work, in a transaction
//!   that says its space ([`Pg::space`]); row-level security lets only that space's rows through.
//! - **admin** (`maker_connect_admin`, BYPASSRLS): only for what crosses spaces by nature, each use naming
//!   its reason (`Bypass`, `Pg::admin`); the uses are listed in `admin-uses.txt` beside this file, and
//!   a test fails when the code and the list differ.
//!
//! Roles (`scripts/db-roles.sql`, docs/design ADR 20261119-database-roles): the hub logs in as
//! `maker_connect_hub`: no superuser, owner of nothing, no table privilege of its own (NOINHERIT member
//! of the three roles below, which it may only `SET ROLE` to). The tables belong to
//! `maker_connect_owner` (NOLOGIN on a hub's server): the migrations run under it, and so do the backup
//! tools ([`crate::backup`]). Each pooled connection takes its role with `SET ROLE` as it opens; a
//! `RESET ROLE` (or `DISCARD ALL`) on one leaves the bare login, which reads no table at all.

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
    // In every build, with the feature `cameras` or without: one database has one set of migrations.
    static CAMERAS: Migrator = sqlx::migrate!("src/cameras/migrations");
    static GUIDES: Migrator = sqlx::migrate!("src/guides/migrations");
    vec![
        &CORE, &ACCOUNTS, &ACCESS, &STOCK, &QUEUE, &FARM, &CAMERAS, &GUIDES,
    ]
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
    /// Only through [`Pg::admin`], which names why.
    admin: PgPool,
}

pub const APP_ROLE: &str = "maker_connect_app";
pub const ADMIN_ROLE: &str = "maker_connect_admin";
/// Owns the tables; the migrations and the backup tools run under it.
pub const OWNER_ROLE: &str = "maker_connect_owner";
/// The hub's login (scripts/db-roles.sql); the tests' pools log in as it too ([`Pg::from_pool`]).
pub const HUB_LOGIN: &str = "maker_connect_hub";

/// Why a query crosses spaces: the admin-pool allowlist of design database.md (plan §2.6, completed
/// there; ADR 20261119-database-roles), by its numbers. Everything else runs in its space's transaction
/// ([`Pg::space`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Bypass {
    /// 1. Before the space is known: a device's registration is inserted (code, secret), and a
    ///    presented secret is looked up: session token, API token, agent or printer token,
    ///    registration code, invite token. The answer is what is needed to
    ///    open a space transaction (and, for a device not in a space yet, its own `machines` row).
    Secret,
    /// 2. Sign-in and setup: a user by name and their identity's secret, their sessions; the first
    ///    account on a fresh database; accepting an invite (user, identity, membership and the
    ///    invite's use in one transaction).
    SignIn,
    /// 3. Instance admin: only for a user with `instance_admin`: the instance's own settings and
    ///    views, what spans tenants, moving a device to another space.
    InstanceAdmin,
    /// 4. Writers that span spaces: the history writer (`sample`, `gap`; crate::store::history),
    ///    which has no app-role grant on those tables (decisions/20261120-history-views-not-rls.md).
    Writer,
    /// 5. Background tasks listing their work: which spaces and devices have work; each unit of work
    ///    then runs on the app pool in its space.
    TaskList,
    /// 6. The signed-in person's own memberships and organisation memberships and the spaces they
    ///    name, before a space is chosen: always `WHERE user_id` is the session's user.
    Me,
}

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

/// Refused as the hub's login (§2.6, R2): a superuser, a role with BYPASSRLS, or one that owns a table or
/// has a privilege on one of its own (an owner, or a member that inherits): a `RESET ROLE` on a pooled
/// connection would read every space's rows.
async fn refuse_a_login_that_reads_tables(pool: &PgPool) -> Result<(), sqlx::Error> {
    let (login, superuser, bypass, tables): (String, bool, bool, i64) = sqlx::query_as(
        "SELECT r.rolname::text, r.rolsuper, r.rolbypassrls,
                (SELECT COUNT(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
                  WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p', 'v', 'm')
                    AND (c.relowner = r.oid
                         OR has_table_privilege(r.oid, c.oid, 'SELECT, INSERT, UPDATE, DELETE')))
           FROM pg_roles r WHERE r.rolname = session_user",
    )
    .fetch_one(pool)
    .await?;
    let why = if superuser {
        "is a superuser"
    } else if bypass {
        "has BYPASSRLS"
    } else if tables > 0 {
        "owns tables or may read them itself"
    } else {
        return Ok(());
    };
    Err(sqlx::Error::Configuration(
        format!(
            "the database login {login} {why}: the hub logs in as {HUB_LOGIN}, which may only SET ROLE \
             (scripts/db-roles.sql, docs/pi-hub.md)"
        )
        .into(),
    ))
}

impl Pg {
    /// Connects to `url` as the hub's login, runs the migrations as the tables' owner
    /// ([`OWNER_ROLE`]), checks the login reads no table itself, and opens the two pools.
    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        let options: PgConnectOptions = url.parse()?;
        let owner = pool(&options, OWNER_ROLE, 1);
        let migrated = MIGRATOR.run(&owner).await;
        owner.close().await;
        migrated?;
        let login = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await?;
        let refused = refuse_a_login_that_reads_tables(&login).await;
        login.close().await;
        refused?;
        Ok(Self::with_options(&options, 8))
    }

    /// The pools over the database `pool` reaches (migrated already), logged in as the hub's login
    /// with `pool`'s password: for `#[sqlx::test]` (scripts/dev-db.sh, CI: one password for both).
    pub fn from_pool(pool: &PgPool) -> Self {
        Self::with_options(&Self::hub_login(&pool.connect_options()), 3)
    }

    /// `options` as the hub's login (the tests' own login owns the tables).
    pub(crate) fn hub_login(options: &PgConnectOptions) -> PgConnectOptions {
        options.clone().username(HUB_LOGIN)
    }

    /// The admin pool (BYPASSRLS), for `why` only: one of the reasons §2.6 allows. Each use is listed
    /// in `admin-uses.txt` (test `every_admin_use_is_listed_with_its_reason`).
    pub(crate) fn admin(&self, why: Bypass) -> &PgPool {
        let _ = why;
        &self.admin
    }

    /// The admin pool in tests (`#[cfg(test)]` only: to look across spaces at what a test did).
    #[cfg(test)]
    pub(crate) fn admin_in_tests(&self) -> &PgPool {
        &self.admin
    }

    /// Both pools closed ([`crate::store::Store::close`]).
    pub(crate) async fn close(&self) {
        self.app.close().await;
        self.admin.close().await;
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

    /// The admin pool's uses (design database.md#roles-and-pools, "A test extracts every admin-pool
    /// call"): every `.admin(…)` in the hub's source, as (file, function, allowlist number), is a line
    /// of `admin-uses.txt` with its reason, once per call; and every line there is such a call.
    #[test]
    fn every_admin_use_is_listed_with_its_reason() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let found = admin_uses_in(&src);
        assert!(
            !found.is_empty(),
            "no admin use found under {}",
            src.display()
        );
        let listed = admin_uses_listed(include_str!("admin-uses.txt"));
        let missing: Vec<_> = found.iter().filter(|u| !listed.contains(u)).collect();
        let extra: Vec<_> = listed.iter().filter(|u| !found.contains(u)).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "admin uses not in admin-uses.txt (path fn number, with a reason): {missing:?}; \
             listed but not in the code: {extra:?}"
        );
    }

    /// An unlisted use fails, and so does an admin use without a reason of the allowlist.
    #[test]
    fn an_unlisted_admin_use_fails_the_check() {
        let dir = std::env::temp_dir().join(format!("mc-admin-uses-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("x.rs"),
            "fn listed() { pg.admin(Bypass::Secret); }\nfn sneaky() {\n    pg.admin(Bypass::TaskList);\n}\n",
        )
        .unwrap();
        let found = admin_uses_in(&dir);
        let listed = admin_uses_listed("x.rs listed 1 a token\n");
        assert_eq!(
            found,
            vec![
                ("x.rs".to_owned(), "listed".to_owned(), "1".to_owned()),
                ("x.rs".to_owned(), "sneaky".to_owned(), "5".to_owned()),
            ]
        );
        assert!(found.iter().any(|u| !listed.contains(u)));
        std::fs::write(dir.join("x.rs"), "fn why() { pg.admin(reason); }\n").unwrap();
        assert_eq!(
            admin_uses_in(&dir),
            vec![("x.rs".to_owned(), "why".to_owned(), "no Bypass".to_owned())]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    type Use = (String, String, String);

    /// `.admin(Bypass::X)` calls under `dir` outside test code: (path from `dir`, enclosing fn,
    /// allowlist number); a call that names no `Bypass` gives "no Bypass", which no line lists.
    fn admin_uses_in(dir: &std::path::Path) -> Vec<Use> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(dir, &mut files);
        files.sort();
        let mut out = Vec::new();
        for file in files {
            let path = file
                .strip_prefix(dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            // Test code is not the hub's: a file of tests, and a file's `#[cfg(test)] mod … { }`.
            if file.file_name().is_some_and(|n| n == "tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&file).unwrap();
            let mut function = String::new();
            let mut after_cfg_test = false;
            // Inside a `#[cfg(test)] mod … {`: its depth in braces.
            let mut in_test: Option<i64> = None;
            for line in text.lines() {
                let code = line.split("//").next().unwrap_or("");
                let opens = code.matches('{').count() as i64 - code.matches('}').count() as i64;
                if after_cfg_test && code.trim_start().starts_with("mod ") && code.contains('{') {
                    in_test = Some(0);
                }
                after_cfg_test = code.trim() == "#[cfg(test)]";
                if let Some(depth) = in_test.as_mut() {
                    *depth += opens;
                    if *depth <= 0 {
                        in_test = None;
                    }
                    continue;
                }
                if let Some(at) = code.find("fn ") {
                    let before = &code[..at];
                    if before.is_empty() || before.ends_with([' ', '(']) {
                        let name: String = code[at + 3..]
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        if !name.is_empty() {
                            function = name;
                        }
                    }
                }
                let mut rest = code;
                while let Some(at) = rest.find(".admin(") {
                    rest = &rest[at + ".admin(".len()..];
                    let arg: String = rest.chars().take_while(|c| *c != ')').collect();
                    let number = match arg.trim() {
                        "Bypass::Secret" => "1",
                        "Bypass::SignIn" => "2",
                        "Bypass::InstanceAdmin" => "3",
                        "Bypass::Writer" => "4",
                        "Bypass::TaskList" => "5",
                        "Bypass::Me" => "6",
                        _ => "no Bypass",
                    };
                    out.push((path.clone(), function.clone(), number.to_owned()));
                }
            }
        }
        out.sort();
        out
    }

    /// The lines of `admin-uses.txt` as uses; each must give a reason after its number.
    fn admin_uses_listed(text: &str) -> Vec<Use> {
        let mut out: Vec<Use> = text
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|l| {
                let mut parts = l.splitn(4, ' ');
                let (path, function, number, reason) = (
                    parts.next().unwrap_or(""),
                    parts.next().unwrap_or(""),
                    parts.next().unwrap_or(""),
                    parts.next().unwrap_or("").trim(),
                );
                assert!(
                    ["1", "2", "3", "4", "5", "6"].contains(&number) && !reason.is_empty(),
                    "admin-uses.txt: {l:?} needs `path fn number reason`, number 1-6"
                );
                (path.to_owned(), function.to_owned(), number.to_owned())
            })
            .collect();
        out.sort();
        out
    }

    /// The SQLSTATE of a refused query (42501: insufficient_privilege; 23503: foreign_key_violation).
    fn code_of<T: std::fmt::Debug>(r: Result<T, sqlx::Error>) -> String {
        match r {
            Err(sqlx::Error::Database(e)) => e.code().map(|c| c.into_owned()).unwrap_or_default(),
            other => panic!("not refused by the database: {other:?}"),
        }
    }

    /// R2 (design database.md#roles-and-pools): the hub logs in as maker_connect_hub, which owns
    /// nothing and reads no table of its own; a pooled connection put back to the login (`RESET ROLE`,
    /// `SET ROLE NONE`, `DISCARD ALL`) is refused every table (42501); and the hub refuses to start
    /// with a login that owns tables.
    #[sqlx::test(migrations = false)]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_login_reads_nothing_after_reset_role(pool: PgPool) {
        let db: String = sqlx::query_scalar("SELECT current_database()::text")
            .fetch_one(&pool)
            .await
            .unwrap();
        let pg = Pg::connect(&crate::backup::hub_url_for_test(&db))
            .await
            .unwrap();
        let not_owner: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_tables WHERE schemaname = 'public' AND tableowner <> $1",
        )
        .bind(OWNER_ROLE)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(not_owner, 0, "every table is the owner's");
        let tables: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM pg_tables WHERE schemaname = 'public'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(tables > 10, "{tables}");
        let (superuser, bypass, inherit, owns, may): (bool, bool, bool, i64, i64) = sqlx::query_as(
            "SELECT r.rolsuper, r.rolbypassrls, r.rolinherit,
                    (SELECT COUNT(*) FROM pg_class c WHERE c.relowner = r.oid),
                    (SELECT COUNT(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
                      WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p', 'v', 'm')
                        AND has_table_privilege(r.oid, c.oid, 'SELECT, INSERT, UPDATE, DELETE'))
               FROM pg_roles r WHERE r.rolname = $1",
        )
        .bind(HUB_LOGIN)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            (superuser, bypass, inherit, owns, may),
            (false, false, false, 0, 0)
        );
        for (pool, role) in [(&pg.app, APP_ROLE), (&pg.admin, ADMIN_ROLE)] {
            for reset in ["RESET ROLE", "SET ROLE NONE", "DISCARD ALL"] {
                // Off the pool: this connection is not given back.
                let mut c = pool.acquire().await.unwrap().detach();
                let now: String = sqlx::query_scalar("SELECT current_user::text")
                    .fetch_one(&mut c)
                    .await
                    .unwrap();
                assert_eq!(now, role);
                c.execute(reset).await.unwrap();
                let who: String = sqlx::query_scalar("SELECT current_user::text")
                    .fetch_one(&mut c)
                    .await
                    .unwrap();
                assert_eq!(who, HUB_LOGIN, "{reset}");
                for table in ["spaces", "users", "machines", "jobs"] {
                    let read = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                        "SELECT COUNT(*) FROM {table}"
                    )))
                    .fetch_one(&mut c)
                    .await;
                    assert_eq!(code_of(read), "42501", "{role} after {reset}: {table}");
                }
            }
        }
        pg.close().await;
        // The tests' own login owns the tables: refused as the hub's.
        let owner_url =
            crate::backup::with_database(&std::env::var("DATABASE_URL").unwrap(), &db).unwrap();
        let refused = Pg::connect(&owner_url).await.unwrap_err();
        assert!(refused.to_string().contains("owns tables"), "{refused}");
    }

    /// Two organisations with a space each and a second space in the first, people in each, and a
    /// secret of each kind, as the owner: org 1 (spaces 1, 3), org 2 (space 2).
    async fn two_organisations(pool: &PgPool) {
        sqlx::raw_sql(
            "INSERT INTO organisations (uid, name, created_at) VALUES ('o2', 'Other', at_unix(0));
             INSERT INTO spaces (id, name, uid, organisation_id)
                 SELECT 2, 'Other', 's2', id FROM organisations WHERE uid = 'o2';
             INSERT INTO spaces (id, name, uid, organisation_id) VALUES (3, 'Lab', 's3', 1);
             INSERT INTO users (id, name, display_name, created_at) VALUES
                 (1, 'ann', 'Ann', at_unix(0)), (2, 'bob', 'Bob', at_unix(0)),
                 (3, 'cas', 'Cas', at_unix(0)), (4, 'dee', 'Dee', at_unix(0));
             INSERT INTO memberships (user_id, space_id, role, created_at) VALUES (1, 1, 'member', at_unix(0));
             INSERT INTO org_memberships (user_id, organisation_id, role, created_at) VALUES
                 (2, 1, 'admin', at_unix(0)),
                 (3, (SELECT id FROM organisations WHERE uid = 'o2'), 'owner', at_unix(0));
             INSERT INTO identities (user_id, provider, subject, secret, created_at)
                 SELECT id, 'password', name, 'hash', at_unix(0) FROM users;
             INSERT INTO sessions (token_hash, user_id, space_id, created_at, last_seen, expires_at)
                 VALUES ('h', 1, 1, at_unix(0), at_unix(0), at_unix(9999999999));
             INSERT INTO meta (key, value) VALUES ('open_spaces', '1');
             SELECT setval('spaces_id_seq', 10), setval('users_id_seq', 10);",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    /// Q2: the app role has no privilege on identities, sessions and meta (42501, in a space too), may
    /// not write users, and reads only the people of its space and of its space's organisation.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_app_role_reads_no_secrets_and_only_its_own_people(pool: PgPool) {
        two_organisations(&pool).await;
        let pg = Pg::from_pool(&pool);
        for table in ["identities", "sessions", "meta"] {
            let mut tx = pg.space(1).await.unwrap();
            let read = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM {table}"
            )))
            .fetch_one(&mut *tx)
            .await;
            assert_eq!(code_of(read), "42501", "{table}");
        }
        let mut tx = pg.space(1).await.unwrap();
        let wrote = sqlx::query("UPDATE users SET instance_admin = true")
            .execute(&mut *tx)
            .await;
        assert_eq!(code_of(wrote), "42501");
        drop(tx);
        let names = async |space: i64| {
            let mut tx = pg.space(space).await.unwrap();
            let names: Vec<String> = sqlx::query_scalar("SELECT name FROM users ORDER BY id")
                .fetch_all(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            names
        };
        assert_eq!(names(1).await, ["ann", "bob"]);
        assert_eq!(names(3).await, ["bob"]);
        assert_eq!(names(2).await, ["cas"]);
        let none: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&pg.app)
            .await
            .unwrap();
        assert_eq!(none, 0, "without a space");
    }

    /// Q3: organisation-wide work in a space of the organisation. In space 1 (org 1), org 2's
    /// organisation, memberships and space do not show; org 1's spaces do, a space is made in org 1
    /// only, and only space 1 itself is changed. Renaming org 1 from space 1 works, and through the
    /// store from space 2 renames org 2 only.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn org_wide_work_sees_its_own_organisation_only(pool: PgPool) {
        two_organisations(&pool).await;
        let pg = Pg::from_pool(&pool);
        let mut tx = pg.space(1).await.unwrap();
        let organisations: Vec<String> = sqlx::query_scalar("SELECT name FROM organisations")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert_eq!(organisations, ["My space"]);
        let members: Vec<i64> =
            sqlx::query_scalar("SELECT user_id FROM org_memberships ORDER BY user_id")
                .fetch_all(&mut *tx)
                .await
                .unwrap();
        assert_eq!(members, [2]);
        let spaces: Vec<i64> = sqlx::query_scalar("SELECT id FROM spaces ORDER BY id")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert_eq!(spaces, [1, 3]);
        let sibling = sqlx::query("UPDATE spaces SET name = 'x' WHERE id = 3")
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(sibling.rows_affected(), 0, "only its own space is changed");
        let renamed = sqlx::query("UPDATE organisations SET name = 'Fablab'")
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(renamed.rows_affected(), 1);
        tx.commit().await.unwrap();
        let mut tx = pg.space(1).await.unwrap();
        let elsewhere = sqlx::query(
            "INSERT INTO spaces (name, uid, organisation_id)
             SELECT 'x', 'x', id FROM organisations WHERE uid = 'o2'",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        assert_eq!(elsewhere.rows_affected(), 0, "org 2 does not even show");
        let by_id =
            sqlx::query("INSERT INTO spaces (name, uid, organisation_id) VALUES ('x', 'y', 2)")
                .execute(&mut *tx)
                .await;
        assert_eq!(code_of(by_id), "42501");
        drop(tx);

        let store = crate::Store::for_test(&pool).await;
        store.rename_organisation(2, "Zuid").await.unwrap();
        assert_eq!(store.organisation_name(1).await.unwrap(), "Fablab");
        assert_eq!(store.organisation_name(3).await.unwrap(), "Fablab");
        assert_eq!(store.organisation_name(2).await.unwrap(), "Zuid");
        let made = store.create_space(3, "Hal").await.unwrap();
        assert_eq!(store.organisation_of(made).await.unwrap(), 1);
        assert_eq!(
            store.org_role_of(2, 3).await.unwrap(),
            Some(crate::access::Role::Admin)
        );
        assert_eq!(store.org_role_of(2, 2).await.unwrap(), None);
    }

    /// FK (R1, the second wall): a row of space 1 never names space 2's machine, whoever writes it: the
    /// keys are (machine, space), and a key's check sees past row-level security.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_row_never_names_another_spaces_machine(pool: PgPool) {
        sqlx::raw_sql(
            "INSERT INTO organisations (uid, name, created_at) VALUES ('o2', 'Other', at_unix(0));
             INSERT INTO spaces (id, name, uid, organisation_id)
                 SELECT 2, 'Other', 's2', id FROM organisations WHERE uid = 'o2';
             INSERT INTO machines (id, space_id, name, kind, agent_version, token, registered_at, link)
                 VALUES ('m1', 1, 'M', '\"fdm\"', '0.1.0', 't1', at_unix(0), 'virtual'),
                        ('m2', 2, 'M', '\"fdm\"', '0.1.0', 't2', at_unix(0), 'virtual');
             INSERT INTO machine_groups (id, space_id, name, created_at) VALUES (1, 1, 'G', at_unix(0));",
        )
        .execute(&pool)
        .await
        .unwrap();
        let rows = [
            "INSERT INTO machine_grants (space_id, machine_id, user_id, level, granted_at)
                 VALUES (1, '{m}', 1, 'use', at_unix(0))",
            "INSERT INTO machine_group_members (space_id, group_id, machine_id) VALUES (1, 1, '{m}')",
            "INSERT INTO access_requests (space_id, user_id, machine_id, asked_at)
                 VALUES (1, 1, '{m}', at_unix(0))",
            "INSERT INTO printer_files (machine_id, space_id, path) VALUES ('{m}', 1, '/usb/a')",
        ];
        let pg = Pg::from_pool(&pool);
        for row in rows {
            let other = row.replace("{m}", "m2");
            let mut tx = pg.space(1).await.unwrap();
            let refused = sqlx::query(sqlx::AssertSqlSafe(other.clone()))
                .execute(&mut *tx)
                .await;
            assert_eq!(code_of(refused), "23503", "as the app role: {other}");
            drop(tx);
            let refused = sqlx::query(sqlx::AssertSqlSafe(other.clone()))
                .execute(&pool)
                .await;
            assert_eq!(code_of(refused), "23503", "as the owner: {other}");
            // Its own space's machine: taken.
            let mut tx = pg.space(1).await.unwrap();
            sqlx::query(sqlx::AssertSqlSafe(row.replace("{m}", "m1")))
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    }

    /// Composite keys (design decisions/20261119-composite-foreign-keys.md): every reference from a
    /// tenant row to another tenant row names the parent with its space, so a row of space 1 never
    /// points at space 2's job, file, part, project, vendor, filament, spool, machine or location,
    /// whoever writes it. One case per key.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_row_never_names_another_spaces_row(pool: PgPool) {
        // The parents, `…1` in space 1 and `…2` in space 2, as the owner.
        sqlx::raw_sql(
            "INSERT INTO organisations (uid, name, created_at) VALUES ('o2', 'Other', at_unix(0));
             INSERT INTO spaces (id, name, uid, organisation_id)
                 SELECT 2, 'Other', 's2', id FROM organisations WHERE uid = 'o2';
             INSERT INTO machines (id, space_id, name, kind, agent_version, token, registered_at, link)
                 VALUES ('m1', 1, 'M', '\"fdm\"', '0.1.0', 't1', at_unix(0), 'virtual'),
                        ('m2', 2, 'M', '\"fdm\"', '0.1.0', 't2', at_unix(0), 'virtual');
             INSERT INTO files (id, space_id, name, size, uploaded_at, meta)
                 VALUES ('f1', 1, 'a.gcode', 1, at_unix(0), '{}'), ('f2', 2, 'a.gcode', 1, at_unix(0), '{}');
             INSERT INTO jobs (id, space_id, machine_id, file_id, file_name, state, created_at)
                 VALUES (1, 1, 'm1', 'f1', 'a.gcode', 'finished', at_unix(0)),
                        (2, 2, 'm2', 'f2', 'a.gcode', 'finished', at_unix(0));
             INSERT INTO projects (id, space_id, name, created_at)
                 VALUES (1, 1, 'P', at_unix(0)), (2, 2, 'P', at_unix(0));
             INSERT INTO parts (id, project_id, space_id, name, target, created_at)
                 VALUES (1, 1, 1, 'A', 1, at_unix(0)), (2, 2, 2, 'A', 1, at_unix(0));
             INSERT INTO vendor (id, space_id, registered, name)
                 VALUES (1, 1, at_unix(0), 'V'), (2, 2, at_unix(0), 'V');
             INSERT INTO filament (id, space_id, registered) VALUES (1, 1, at_unix(0)), (2, 2, at_unix(0));
             INSERT INTO spool (id, space_id, registered, filament_id)
                 VALUES (1, 1, at_unix(0), 1), (2, 2, at_unix(0), 2);
             INSERT INTO locations (id, space_id, name) VALUES ('P1', 1, 'Shelf'), ('P2', 2, 'Shelf');
             SELECT setval(pg_get_serial_sequence('jobs', 'id'), 10);",
        )
        .execute(&pool)
        .await
        .unwrap();
        // `{n}`: 2 names space 2's parent (refused), 1 its own (taken).
        let rows = [
            "INSERT INTO printer_transfers (iv, machine_id, space_id, file_id, key, size, path, created_at)
                 VALUES ('iv-f{n}', 'm1', 1, 'f{n}', 'k', 1, '/usb/a', at_unix(0))",
            "INSERT INTO printer_transfers (iv, machine_id, space_id, file_id, job_id, key, size, path, created_at)
                 VALUES ('iv-j{n}', 'm1', 1, 'f1', {n}, 'k', 1, '/usb/a', at_unix(0))",
            "INSERT INTO part_ledger (space_id, part_id, at, delta, done_after, source, actor)
                 VALUES (1, {n}, at_unix(0), 1, 1, 'job', 'a')",
            "INSERT INTO part_ledger (space_id, part_id, job_id, at, delta, done_after, source, actor)
                 VALUES (1, 1, {n}, at_unix(0), 1, 1, 'job', 'a')",
            "INSERT INTO parts (project_id, space_id, name, target, created_at)
                 VALUES ({n}, 1, 'B', 1, at_unix(0))",
            "INSERT INTO part_files (part_id, space_id, profile, file_id, created_at)
                 VALUES ({n}, 1, 'p', 'f1', at_unix(0))",
            "INSERT INTO part_files (part_id, space_id, profile, file_id, created_at)
                 VALUES (1, 1, 'q', 'f{n}', at_unix(0))",
            "INSERT INTO jobs (space_id, machine_id, file_id, file_name, state, created_at, part_id)
                 VALUES (1, 'm1', 'f1', 'a.gcode', 'finished', at_unix(0), {n})",
            "INSERT INTO jobs (space_id, machine_id, file_id, file_name, state, created_at, spool_id)
                 VALUES (1, 'm1', 'f1', 'a.gcode', 'finished', at_unix(0), {n})",
            "INSERT INTO filament (space_id, registered, vendor_id) VALUES (1, at_unix(0), {n})",
            "INSERT INTO spool (space_id, registered, filament_id) VALUES (1, at_unix(0), {n})",
            "INSERT INTO spool (space_id, registered, filament_id, machine) VALUES (1, at_unix(0), 1, 'm{n}')",
            "INSERT INTO tag (space_id, uid, target_type, spool_id, added)
                 VALUES (1, 's{n}', 'spool', {n}, at_unix(0))",
            "INSERT INTO tag (space_id, uid, target_type, filament_id, added)
                 VALUES (1, 'f{n}', 'filament', {n}, at_unix(0))",
            "INSERT INTO cameras (space_id, name, url, machine_id, created_at)
                 VALUES (1, 'C', 'rtsp://x', 'm{n}', at_unix(0))",
            "INSERT INTO locations (id, space_id, name, parent) VALUES ('C{n}', 1, 'Box', 'P{n}')",
        ];
        let pg = Pg::from_pool(&pool);
        for row in rows {
            let other = row.replace("{n}", "2");
            let mut tx = pg.space(1).await.unwrap();
            let refused = sqlx::query(sqlx::AssertSqlSafe(other.clone()))
                .execute(&mut *tx)
                .await;
            assert_eq!(code_of(refused), "23503", "as the app role: {other}");
            drop(tx);
            let refused = sqlx::query(sqlx::AssertSqlSafe(other.clone()))
                .execute(&pool)
                .await;
            assert_eq!(code_of(refused), "23503", "as the owner: {other}");
            let mut tx = pg.space(1).await.unwrap();
            sqlx::query(sqlx::AssertSqlSafe(row.replace("{n}", "1")))
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
        // The keys point at (space, id): each referenced table has that unique key.
        let unique: Vec<String> = sqlx::query_scalar(
            "SELECT c.conrelid::regclass::text FROM pg_constraint c
             WHERE c.contype = 'u' AND c.connamespace = 'public'::regnamespace
               AND (SELECT array_agg(a.attname::text ORDER BY k.n) FROM unnest(c.conkey) WITH ORDINALITY k(attnum, n)
                    JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.attnum) = ARRAY['space_id', 'id']
             ORDER BY 1",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            unique,
            [
                "filament", "files", "guides", "jobs", "parts", "projects", "spool", "vendor"
            ],
            "UNIQUE (space_id, id)"
        );
    }

    /// Row-level security by construction (catalog): every table the app role may touch has
    /// row-level security (a tenant table without it, as `sessions`, is the admin role's alone); and
    /// a new table gives the app role nothing by default: a migration grants it beside its policy.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn every_app_table_has_row_level_security(pool: PgPool) {
        let without: Vec<String> = sqlx::query_scalar(
            "SELECT c.relname::text FROM pg_class c
             WHERE c.relnamespace = 'public'::regnamespace AND c.relkind IN ('r', 'p')
               AND NOT c.relrowsecurity
               AND has_table_privilege($1, c.oid, 'SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER')
             ORDER BY 1",
        )
        .bind(APP_ROLE)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(without.is_empty(), "no row-level security: {without:?}");
        let by_default: Vec<String> = sqlx::query_scalar(
            "SELECT d.defaclobjtype::text FROM pg_default_acl d, aclexplode(d.defaclacl) a
             WHERE a.grantee = $1::regrole AND d.defaclobjtype = 'r'",
        )
        .bind(APP_ROLE)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(by_default.is_empty(), "default privileges: {by_default:?}");
        // What the app role is granted, table by table (a new table is added here on purpose).
        let granted: Vec<String> = sqlx::query_scalar(
            "SELECT c.relname::text FROM pg_class c
             WHERE c.relnamespace = 'public'::regnamespace AND c.relkind IN ('r', 'p')
               AND has_table_privilege($1, c.oid, 'SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER')
             ORDER BY 1",
        )
        .bind(APP_ROLE)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            granted,
            [
                "access_requests",
                "api_tokens",
                "audit_log",
                "camera_snapshots",
                "cameras",
                "closed_days",
                "filament",
                "files",
                "group_grants",
                "group_locks",
                "guides",
                "invites",
                "jobs",
                "learned_names",
                "locations",
                "machine_grants",
                "machine_group_members",
                "machine_groups",
                "machine_queue",
                "machines",
                "memberships",
                "movements",
                "org_memberships",
                "organisations",
                "part_files",
                "part_ledger",
                "parts",
                "printer_files",
                "printer_transfers",
                "projects",
                "setting",
                "spaces",
                "spool",
                "tag",
                "time_windows",
                "users",
                "vendor",
            ]
        );
    }

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
        let pg = Pg::with_options(&Pg::hub_login(&pool.connect_options()), 1);
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
             INSERT INTO machines (id, space_id, name, kind, agent_version, token, registered_at, link)
                 SELECT 'm' || id, id, 'M', '\"fdm\"', '0.1.0', 't' || id, at_unix(0), 'virtual' FROM spaces;
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
        let pg = Pg::with_options(&Pg::hub_login(&pool.connect_options()), 1);
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
            "INSERT INTO machines (id, space_id, name, kind, agent_version, token, registered_at, link)
                 VALUES ('x', 2, 'X', '\"fdm\"', '0.1.0', 'tx', at_unix(0), 'virtual')",
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

    /// Row-level security on lane 4's tables (stock, Spoolman, queue, farm, cameras), as the app role: each
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
             INSERT INTO spool (space_id, registered, filament_id) SELECT space_id, at_unix(0), id FROM filament;
             INSERT INTO tag (space_id, uid, target_type, added) SELECT id, 'AA', 'spool', at_unix(0) FROM spaces;
             INSERT INTO machine_queue (space_id, machine_id, file_id, file_name, pos, added_at)
                 SELECT id, 'm', 'f', 'a.gcode', 1, at_unix(0) FROM spaces;
             INSERT INTO projects (space_id, name, created_at) SELECT id, 'P', at_unix(0) FROM spaces;
             INSERT INTO parts (project_id, space_id, name, target, created_at)
                 SELECT id, space_id, 'A', 1, at_unix(0) FROM projects;
             INSERT INTO files (id, space_id, name, size, uploaded_at, meta)
                 SELECT 'f' || id, id, 'a.gcode', 1, at_unix(0), '{}' FROM spaces;
             INSERT INTO part_files (part_id, space_id, profile, file_id, created_at)
                 SELECT p.id, p.space_id, 'prusa/mk3s', 'f' || p.space_id, at_unix(0) FROM parts p;
             INSERT INTO part_ledger (space_id, part_id, at, delta, done_after, source, actor)
                 SELECT space_id, id, at_unix(0), 1, 1, 'corrected', 'x' FROM parts;
             INSERT INTO cameras (space_id, name, url, created_at) SELECT id, 'C', 'rtsp://x', at_unix(0) FROM spaces;",
        )
        .execute(&pool)
        .await
        .unwrap();
        let pg = Pg::with_options(&Pg::hub_login(&pool.connect_options()), 1);
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
            (
                "cameras",
                "INSERT INTO cameras (space_id, name, url, created_at) VALUES (2, 'x', 'x', at_unix(0))",
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
