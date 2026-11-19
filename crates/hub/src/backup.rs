//! Backups of the database (docs/plans/hub-core-v2.md §2.7, "Pi deployment"): `pg_dump` every night
//! into a folder beside the files folder (`maker-connect-dumps/` beside `maker-connect-files/`), and
//! before a self-update ([`crate::updates`]), so a new build that does not stay up goes back together
//! with the database as it was.
//!
//! - The dumps are PostgreSQL's custom format (`pg_dump -Fc`), each file only for the hub's user
//!   (0600: they hold password hashes and token hashes), the folder too (0700).
//! - The last [`KEEP`] of each kind are kept (`--keep-dumps`); that number is our choice.
//! - The client tools (`pg_dump`, `pg_restore`, `psql`) must be those of the server's major version.
//!   Where the host has none (the Pi .67: Debian has 13, the server is 17), `--pg-command` names what
//!   runs them, e.g. `docker exec -i -e PGPASSWORD maker-connect-db` (the database's own container;
//!   it publishes `127.0.0.1:5432` on its own port 5432, so the same address works inside it).
//! - The password never goes on a command line: it is taken out of the address and handed over as
//!   `PGPASSWORD`.
//! - A restore never writes over the database in use: it fills a new database, and only once that
//!   worked does it take the old one's place ([`Dumps::restore_live`]).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The hub's dumps, as its flags set them (main); none in tests and `reset-password`.
pub static DUMPS: std::sync::OnceLock<Dumps> = std::sync::OnceLock::new();

/// How many dumps of each kind are kept unless `--keep-dumps` says otherwise (our choice: a week of
/// nightly dumps).
pub const KEEP: usize = 7;

/// The kinds of dump, the start of their file names.
pub const NIGHTLY: &str = "nightly";
pub const BEFORE_UPDATE: &str = "before-update";

/// Where dumps go and how they are made.
#[derive(Debug, Clone)]
pub struct Dumps {
    pub dir: PathBuf,
    pub keep: usize,
    /// What runs the client tools: empty for the host's own, else a prefix (`docker exec -i …`).
    command: Vec<String>,
    /// The database's address without its password.
    url: url::Url,
    password: Option<String>,
}

/// The dumps folder beside `files_dir` (`x-files` → `x-dumps`).
pub fn dir_beside(files_dir: &Path) -> PathBuf {
    let name = files_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    files_dir.with_file_name(match name.strip_suffix("-files") {
        Some(stem) => format!("{stem}-dumps"),
        None => format!("{name}-dumps"),
    })
}

impl Dumps {
    /// Dumps of the database at `database_url` into `dir` (made when missing), run by `command`
    /// (empty: the host's own tools).
    pub fn new(
        dir: PathBuf,
        keep: usize,
        command: &str,
        database_url: &str,
    ) -> Result<Self, String> {
        let mut url =
            url::Url::parse(database_url).map_err(|e| format!("{database_url:?}: {e}"))?;
        let password = url.password().map(percent_decode);
        let _ = url.set_password(None);
        if url.path().trim_start_matches('/').is_empty() {
            return Err("the database address names no database".into());
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        Ok(Self {
            dir,
            keep: keep.max(1),
            command: command.split_whitespace().map(str::to_owned).collect(),
            url,
            password,
        })
    }

    /// The name of the database in use.
    pub fn database(&self) -> String {
        self.url.path().trim_start_matches('/').to_owned()
    }

    /// The address of `database` on the same server, without the password.
    fn url_of(&self, database: &str) -> String {
        let mut url = self.url.clone();
        url.set_path(&format!("/{database}"));
        url.to_string()
    }

    fn tool(&self, name: &str) -> Command {
        let mut command = match self.command.split_first() {
            Some((first, rest)) => {
                let mut c = Command::new(first);
                c.args(rest).arg(name);
                c
            }
            None => Command::new(name),
        };
        if let Some(password) = &self.password {
            command.env("PGPASSWORD", password);
        }
        command.stdin(Stdio::null());
        command
    }

    fn finish(mut command: Command, what: &str) -> Result<(), String> {
        let out = command
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("{what}: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "{what} failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    /// SQL on the server's `postgres` database, one statement per `-c` (CREATE/DROP DATABASE run
    /// outside a transaction).
    fn psql(&self, statements: &[String]) -> Result<(), String> {
        let mut command = self.tool("psql");
        command.args([
            "-X",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "-d",
            &self.url_of("postgres"),
        ]);
        for s in statements {
            command.args(["-c", s]);
        }
        Self::finish(command, "psql")
    }

    /// A dump of the database in use, as `<kind>-<unix milliseconds>[-<tag>].dump`; then only the last
    /// [`Dumps::keep`] of that kind are kept.
    pub fn dump(&self, kind: &str, tag: Option<&str>) -> Result<PathBuf, String> {
        // Milliseconds: two dumps in one second keep apart.
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let name = match tag {
            Some(tag) => format!("{kind}-{at}-{tag}.dump"),
            None => format!("{kind}-{at}.dump"),
        };
        let path = self.dir.join(&name);
        let part = self.dir.join(format!("{name}.part"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options
            .open(&part)
            .map_err(|e| format!("{}: {e}", part.display()))?;
        let mut command = self.tool("pg_dump");
        command
            .args(["-Fc", "--dbname", &self.url_of(&self.database())])
            .stdout(file)
            .stderr(Stdio::piped());
        let out = command.output().map_err(|e| format!("pg_dump: {e}"));
        let out = match out {
            Ok(out) if out.status.success() => out,
            Ok(out) => {
                let _ = std::fs::remove_file(&part);
                return Err(format!(
                    "pg_dump failed ({}): {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                return Err(e);
            }
        };
        drop(out);
        std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
        self.prune(kind);
        Ok(path)
    }

    /// The dumps of `kind`, oldest first.
    pub fn of_kind(&self, kind: &str) -> Vec<PathBuf> {
        let prefix = format!("{kind}-");
        let mut found: Vec<(u128, PathBuf)> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let rest = name.strip_prefix(&prefix)?.strip_suffix(".dump")?;
                let at = rest.split('-').next()?.parse().ok()?;
                Some((at, e.path()))
            })
            .collect();
        found.sort();
        found.into_iter().map(|(_, p)| p).collect()
    }

    fn prune(&self, kind: &str) {
        let all = self.of_kind(kind);
        let extra = all.len().saturating_sub(self.keep);
        for old in &all[..extra] {
            let _ = std::fs::remove_file(old);
        }
    }

    /// `dump` restored into a new database `database` (one already there by that name is
    /// dropped first: never the database in use).
    pub fn restore_as(&self, dump: &Path, database: &str) -> Result<(), String> {
        fits(database)?;
        if database == self.database() {
            return Err("a restore never writes over the database in use".into());
        }
        self.drop_database(database)?;
        self.psql(&[format!(
            "CREATE DATABASE {} TEMPLATE template0",
            ident(database)
        )])?;
        let file = std::fs::File::open(dump).map_err(|e| format!("{}: {e}", dump.display()))?;
        let mut command = self.tool("pg_restore");
        command
            .args([
                "--exit-on-error",
                "--single-transaction",
                "--dbname",
                &self.url_of(database),
            ])
            .stdin(file);
        let restored = Self::finish(command, "pg_restore");
        if restored.is_err() {
            let _ = self.drop_database(database);
        }
        restored
    }

    /// Drops `database` if it is there (its connections ended), never the one in use.
    pub fn drop_database(&self, database: &str) -> Result<(), String> {
        fits(database)?;
        if database == self.database() {
            return Err("never the database in use".into());
        }
        self.psql(&[format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            ident(database)
        )])
    }

    /// The database in use back as `dump` holds it: restored as `<name>_back` first; only when that
    /// worked does it take the place of the one in use (whose connections end).
    pub fn restore_live(&self, dump: &Path) -> Result<(), String> {
        let live = self.database();
        let back = format!("{live}_back");
        self.restore_as(dump, &back)?;
        // pg_restore's session may still be ending when it has exited: the rename waits for it (10 s
        // at most; then it fails as it would have).
        let back_literal = format!("'{}'", back.replace('\'', "''"));
        let live_literal = format!("'{}'", live.replace('\'', "''"));
        self.psql(&[
            format!("DROP DATABASE {} WITH (FORCE)", ident(&live)),
            format!(
                "DO $$ BEGIN \
                 FOR i IN 1..100 LOOP \
                     EXIT WHEN NOT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = {back_literal}); \
                     PERFORM pg_sleep(0.1); \
                 END LOOP; \
                 EXECUTE format('ALTER DATABASE %I RENAME TO %I', {back_literal}, {live_literal}); \
                 END $$",
            ),
        ])
    }

    /// The name of the copy a new build is tried on ([`crate::updates`]).
    pub fn trial_database(&self) -> String {
        format!("{}_trial", self.database())
    }
}

/// PostgreSQL cuts longer names to 63 bytes, which could name the database in use: refused.
fn fits(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 63 {
        return Err(format!("{name:?} is no database name of 1 to 63 bytes"));
    }
    Ok(())
}

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// On the way back from an update that did not work (main): the database as the dump taken before
/// it (named in the trial's `note`, [`crate::updates::prepare_hub`]) holds it. `Ok(None)`: the update
/// took no dump.
pub fn back_as_before(dumps: &Dumps, note: &serde_json::Value) -> Result<Option<PathBuf>, String> {
    let Some(dump) = note.get("dump").and_then(|d| d.as_str()) else {
        return Ok(None);
    };
    let dump = PathBuf::from(dump);
    dumps.restore_live(&dump)?;
    Ok(Some(dump))
}

/// `database_url` with its database name replaced by `database`.
pub fn with_database(database_url: &str, database: &str) -> Result<String, String> {
    let mut url = url::Url::parse(database_url).map_err(|e| e.to_string())?;
    url.set_path(&format!("/{database}"));
    Ok(url.to_string())
}

/// The dumps every night: one at start when the newest nightly dump is a day old or there is none,
/// then whenever the newest is a day old (looked at every hour). Our choice of "nightly": no time of
/// day, a day apart.
pub async fn nightly(dumps: &'static Dumps) {
    const DAY: i64 = 24 * 60 * 60;
    loop {
        let newest = dumps
            .of_kind(NIGHTLY)
            .last()
            .and_then(|p| std::fs::metadata(p).ok()?.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        if newest.is_none_or(|at| crate::store::now() - at >= DAY) {
            match tokio::task::spawn_blocking(move || dumps.dump(NIGHTLY, None)).await {
                Ok(Ok(path)) => println!("database dumped to {}", path.display()),
                Ok(Err(why)) => eprintln!("the nightly dump failed: {why}"),
                Err(why) => eprintln!("the nightly dump failed: {why}"),
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(60 * 60)).await;
    }
}

/// For the tests: the dumps of the test's database, the tools as `MC_PG_COMMAND` says (check.sh:
/// the dev database's container), into a folder of the test's own.
#[cfg(test)]
pub(crate) async fn for_test(pool: &sqlx::PgPool) -> Dumps {
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(pool)
        .await
        .unwrap();
    let base = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let url = with_database(&base, &database).unwrap();
    let dir = std::env::temp_dir().join(format!("mc-dumps-{}", crate::store::random_id()));
    Dumps::new(
        dir,
        KEEP,
        &std::env::var("MC_PG_COMMAND").unwrap_or_default(),
        &url,
    )
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows per table of the public schema, in `pool`'s database.
    pub(crate) async fn counts(pool: &sqlx::PgPool) -> Vec<(String, i64)> {
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT table_name::text FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_type = 'BASE TABLE' ORDER BY 1",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        let mut out = Vec::new();
        for t in tables {
            let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT count(*) FROM {}",
                ident(&t)
            )))
            .fetch_one(pool)
            .await
            .unwrap();
            out.push((t, n));
        }
        out
    }

    pub(crate) async fn pool_of(database: &str) -> sqlx::PgPool {
        let base = std::env::var("DATABASE_URL").unwrap();
        sqlx::PgPool::connect(&with_database(&base, database).unwrap())
            .await
            .unwrap()
    }

    /// The nightly dumps: only for the hub's user, and only the last `keep` of them.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn dumps_are_private_and_only_the_last_are_kept(pool: sqlx::PgPool) {
        let mut dumps = for_test(&pool).await;
        dumps.keep = 2;
        let made: Vec<PathBuf> = (0..3).map(|_| dumps.dump(NIGHTLY, None).unwrap()).collect();
        assert_eq!(dumps.of_kind(NIGHTLY), made[1..].to_vec());
        assert!(!made[0].exists());
        // Another kind is kept on its own.
        let before = dumps.dump(BEFORE_UPDATE, Some("hub-v9.9.9")).unwrap();
        assert_eq!(dumps.of_kind(BEFORE_UPDATE), vec![before.clone()]);
        assert_eq!(dumps.of_kind(NIGHTLY).len(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&made[2], &before] {
                assert_eq!(
                    std::fs::metadata(p).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
            let dir = std::fs::metadata(&dumps.dir).unwrap().permissions().mode();
            assert_eq!(dir & 0o777, 0o700);
        }
        std::fs::remove_dir_all(&dumps.dir).unwrap();
    }

    /// A dump restored into another database holds the same rows; the database in use is never
    /// written over by `restore_as`.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_dump_restores_into_a_copy_with_the_same_rows(pool: sqlx::PgPool) {
        sqlx::query("INSERT INTO meta (key, value) VALUES ('a', '1'), ('b', '2')")
            .execute(&pool)
            .await
            .unwrap();
        let dumps = for_test(&pool).await;
        let dump = dumps.dump(NIGHTLY, None).unwrap();
        assert!(dumps.restore_as(&dump, &dumps.database()).is_err());
        let copy = format!("mc_copy_{}", crate::store::random_id().to_lowercase());
        dumps.restore_as(&dump, &copy).unwrap();
        let restored = pool_of(&copy).await;
        assert_eq!(counts(&restored).await, counts(&pool).await);
        restored.close().await;
        dumps.drop_database(&copy).unwrap();
        std::fs::remove_dir_all(&dumps.dir).unwrap();
    }
}
