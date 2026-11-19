use super::*;

impl Legacy {
    /// A file's place on disk.
    /// Where a file is kept. Named `.gcode` from the first version, whatever is in it (a
    /// label is a PNG), so stores from then keep working.
    pub fn file_path(&self, id: &str) -> std::path::PathBuf {
        self.files_dir.join(format!("{id}.gcode"))
    }

    pub fn thumbnail_path(&self, id: &str) -> std::path::PathBuf {
        self.files_dir.join(format!("{id}.png"))
    }

    /// Whether a machine runs a file (or has it queued): then it is not removed. Jobs are on
    /// SQLite until lane 3b ([`Store::remove_file`]).
    pub(crate) fn file_in_use(&self, id: &str) -> rusqlite::Result<bool> {
        self.db.query_row(
            "SELECT COUNT(*) FROM jobs WHERE file_id = ?1
             AND state IN ('loaded', 'queued', 'printing', 'paused', 'stopping')",
            [id],
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )
    }

    /// A file of a space, from the SQLite copy (transitional, lane 3a: for the modules still on
    /// SQLite; the website reads [`Store::file`]).
    pub(crate) fn file_on_mirror(
        &self,
        space: i64,
        id: &str,
    ) -> rusqlite::Result<Option<StoredFile>> {
        self.db
            .query_row(
                "SELECT id, name, size, uploaded_at, meta FROM files WHERE space_id = ?1 AND id = ?2",
                params![space, id],
                |r| {
                    let meta: String = r.get(4)?;
                    Ok(StoredFile {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        size: r.get::<_, i64>(2)? as u64,
                        uploaded_at: r.get(3)?,
                        meta: serde_json::from_str(&meta).unwrap_or_default(),
                    })
                },
            )
            .optional()
    }
}
