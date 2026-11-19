use super::*;

/// Before another build changes a database, a copy of it as it was:
/// `maker-connect-backups/maker-connect-<when>-<from>-to-<to>.db` beside it. A customer's
/// data is never only in the one file an upgrade is about to change.
pub(super) fn back_up_before_upgrade(db: &Connection, path: &str) -> rusqlite::Result<()> {
    let has_meta: bool = db.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
        |r| r.get(0),
    )?;
    let from: Option<String> = if has_meta {
        db.query_row("SELECT value FROM meta WHERE key = 'opened_by'", [], |r| {
            r.get(0)
        })
        .optional()?
    } else {
        None
    };
    if from.as_deref() == Some(BUILD) {
        return Ok(());
    }
    let file = std::path::Path::new(path);
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("maker-connect");
    let dir = file.with_file_name(format!("{stem}-backups"));
    std::fs::create_dir_all(&dir)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let safe = |s: &str| s.replace(|c: char| !c.is_ascii_alphanumeric() && c != '.', "-");
    let copy = dir.join(format!(
        "{stem}-{}-{}-to-{}.db",
        crate::spoolman::iso(now()).replace(':', ""),
        safe(from.as_deref().unwrap_or("before")),
        safe(BUILD),
    ));
    db.execute("VACUUM INTO ?1", [copy.to_string_lossy()])?;
    println!(
        "database backed up before this build changes it: {}",
        copy.display()
    );
    Ok(())
}

/// The tables as this build has them: made when missing, columns added to older ones, data
/// moved over. Only ever adds; runs in one transaction, so a failure leaves the database as
/// it was.
pub(super) fn migrate(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );",
    )?;
    db.execute(
        "INSERT INTO meta (key, value) VALUES ('opened_by', ?1)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [BUILD],
    )?;
    Ok(())
}
