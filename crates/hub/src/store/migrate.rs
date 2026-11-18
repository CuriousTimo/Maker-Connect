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
         );
         CREATE TABLE IF NOT EXISTS spaces (
             id INTEGER PRIMARY KEY,
             name TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS files (
             id TEXT PRIMARY KEY,
             space_id INTEGER NOT NULL REFERENCES spaces(id),
             name TEXT NOT NULL,
             size INTEGER NOT NULL,
             uploaded_at INTEGER NOT NULL,
             meta TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS jobs (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL,
             machine_id TEXT NOT NULL,
             file_id TEXT NOT NULL,
             file_name TEXT NOT NULL,
             state TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             started_at INTEGER,
             ended_at INTEGER,
             reason TEXT
         );
         CREATE TABLE IF NOT EXISTS machines (
             id TEXT PRIMARY KEY,
             space_id INTEGER REFERENCES spaces(id),
             name TEXT NOT NULL,
             kind TEXT NOT NULL,
             agent_version TEXT NOT NULL,
             token TEXT NOT NULL UNIQUE,
             pairing_code TEXT,
             registered_at INTEGER NOT NULL,
             last_seen INTEGER,
             telemetry TEXT
         );",
    )?;
    for (column, definition) in [
        ("from_line", "INTEGER"),
        // What a job used, and of which spool (counted off when it ends).
        ("used", "REAL"),
        ("spool_id", "INTEGER"),
        ("restart_line", "INTEGER"),
        // When someone took a finished print off the bed.
        ("taken_at", "INTEGER"),
        // Who started it: a person's or a token's name, or the queue's.
        ("by_name", "TEXT"),
    ] {
        let present: bool = db.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('jobs') WHERE name = ?1",
            [column],
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )?;
        if !present {
            db.execute(
                &format!("ALTER TABLE jobs ADD COLUMN {column} {definition}"),
                [],
            )?;
            // The prints from before were taken long ago.
            if column == "taken_at" {
                db.execute(
                    "UPDATE jobs SET taken_at = ?1 WHERE state IN ('finished', 'stopped', 'failed')",
                    [now()],
                )?;
            }
        }
    }
    // Columns added after the first version: a store from then gets them.
    for (column, definition) in [
        ("profile", "TEXT"),
        ("profile_chosen", "INTEGER NOT NULL DEFAULT 0"),
        ("link", "TEXT NOT NULL DEFAULT 'agent'"),
        ("details", "TEXT"),
        ("mesh", "TEXT"),
        ("media", "TEXT"),
        // The device tree (docs/api/integrations.md, "Devices in a tree").
        ("powered_by", "TEXT"),
        ("power_channel", "INTEGER"),
        ("reached_through", "TEXT"),
        // What an agent tells of itself, and where its reports come from.
        ("agent", "TEXT"),
        ("peer", "TEXT"),
        // Taken out of use: why (may be empty), and since when.
        ("out_of_use", "TEXT"),
        ("out_of_use_at", "INTEGER"),
        // A laser's module, as its owner says (the controller does not know it).
        ("laser_w", "REAL"),
        // Only those with an induction (a grant) may use it (crate::access).
        // 0: open to every member; 1: needs permission; 2: needs training (crate::access).
        ("induction_required", "INTEGER NOT NULL DEFAULT 0"),
        // What someone must have and wear to use it, comma separated ("laser safety
        // glasses"); empty: nothing; NULL: what its kind needs (crate::safety).
        ("ppe", "TEXT"),
        // Labels its owners gave it, as a JSON list.
        ("labels", "TEXT"),
        // Who started a print from its USB stick on the website, and when (no job row:
        // the file is the printer's).
        ("usb_print_by", "TEXT"),
        ("usb_print_at", "INTEGER"),
        // An update its agent was asked for (`agent-v0.3.0`, crate::updates), and when.
        ("agent_update_to", "TEXT"),
        ("agent_update_at", "INTEGER"),
    ] {
        let present: bool = db.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('machines') WHERE name = ?1",
            [column],
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )?;
        if !present {
            db.execute(
                &format!("ALTER TABLE machines ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    crate::stock::migrate(db)?;
    let spaces: i64 = db.query_row("SELECT COUNT(*) FROM spaces", [], |r| r.get(0))?;
    if spaces == 0 {
        db.execute("INSERT INTO spaces (name) VALUES ('My space')", [])?;
    }
    crate::printer_files::migrate(db)?;
    crate::farm::migrate(db)?;
    crate::queue::migrate(db)?;
    #[cfg(feature = "cameras")]
    crate::cameras::store::migrate(db)?;
    db.execute(
        "INSERT INTO meta (key, value) VALUES ('opened_by', ?1)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [BUILD],
    )?;
    Ok(())
}
