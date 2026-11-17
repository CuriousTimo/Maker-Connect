use super::*;

impl Store {
    /// A file's place on disk.
    /// Where a file is kept. Named `.gcode` from the first version, whatever is in it (a
    /// label is a PNG), so stores from then keep working.
    pub fn file_path(&self, id: &str) -> std::path::PathBuf {
        self.files_dir.join(format!("{id}.gcode"))
    }

    pub fn thumbnail_path(&self, id: &str) -> std::path::PathBuf {
        self.files_dir.join(format!("{id}.png"))
    }

    /// A new file of a space, already on disk at [`Store::file_path`] of `id`.
    pub fn add_file(
        &mut self,
        space: i64,
        id: &str,
        name: &str,
        size: u64,
        meta: &crate::gcode::FileMeta,
    ) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO files (id, space_id, name, size, uploaded_at, meta) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, space, name, size as i64, now(), serde_json::to_string(meta).unwrap_or_default()],
        )?;
        Ok(())
    }

    /// A space's files, newest first.
    pub fn files(&self, space: i64) -> rusqlite::Result<Vec<StoredFile>> {
        let mut query = self.db.prepare(
            "SELECT id, name, size, uploaded_at, meta FROM files WHERE space_id = ?1 ORDER BY uploaded_at DESC, name",
        )?;
        let rows = query.query_map([space], |r| {
            let meta: String = r.get(4)?;
            Ok(StoredFile {
                id: r.get(0)?,
                name: r.get(1)?,
                size: r.get::<_, i64>(2)? as u64,
                uploaded_at: r.get(3)?,
                meta: serde_json::from_str(&meta).unwrap_or_default(),
            })
        })?;
        rows.collect()
    }

    pub fn file(&self, space: i64, id: &str) -> rusqlite::Result<Option<StoredFile>> {
        Ok(self.files(space)?.into_iter().find(|f| f.id == id))
    }

    /// Reads again what the hub keeps about every file it kept before it read as much as it
    /// does now (where the moves go, the highest power; temperatures, colour, objects and
    /// QOI and JPG thumbnails, as Prusa's gcode-metadata reads them), and what its name
    /// tells; a thumbnail it did not have is kept now.
    pub fn reread_files(&mut self) -> rusqlite::Result<usize> {
        let mut query = self.db.prepare("SELECT id, name, meta FROM files")?;
        let stale: Vec<(String, String)> = query
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })?
            .filter_map(Result::ok)
            .filter(|(_, _, meta)| !meta.contains("bounds_mm") || !meta.contains("\"objects\""))
            .map(|(id, name, _)| (id, name))
            .collect();
        drop(query);
        for (id, name) in &stale {
            if let Ok((mut meta, png)) = crate::gcode::read(&self.file_path(id)) {
                crate::gcode::from_name(&mut meta, name);
                let thumbnail = self.thumbnail_path(id);
                if let Some(png) = png
                    && !thumbnail.exists()
                {
                    let _ = std::fs::write(&thumbnail, png);
                }
                self.db.execute(
                    "UPDATE files SET meta = ?1 WHERE id = ?2",
                    params![serde_json::to_string(&meta).unwrap_or_default(), id],
                )?;
            }
        }
        Ok(stale.len())
    }

    /// Removes a file, from the store and from disk. Returns whether it was there.
    pub fn remove_file(&mut self, space: i64, id: &str) -> rusqlite::Result<bool> {
        // Not while a machine runs it (or has it queued).
        let in_use: i64 = self.db.query_row(
            "SELECT COUNT(*) FROM jobs WHERE file_id = ?1 AND state IN ('loaded', 'queued', 'printing', 'paused', 'stopping')",
            [id],
            |r| r.get(0),
        )?;
        if in_use > 0 {
            return Ok(false);
        }
        let removed = self.db.execute(
            "DELETE FROM files WHERE id = ?1 AND space_id = ?2",
            params![id, space],
        )?;
        if removed > 0 {
            let _ = std::fs::remove_file(self.file_path(id));
            let _ = std::fs::remove_file(self.thumbnail_path(id));
        }
        Ok(removed > 0)
    }
}
