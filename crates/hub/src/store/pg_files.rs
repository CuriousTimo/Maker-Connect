//! Files on Postgres (`migrations/20261118000300_machines.sql`): `files.rs`'s queries as async [`Store`]
//! methods (hub core v2 step 2, lane 3). The G-code and thumbnails stay on disk, beside the database.
//!
//! Transitional: the same-named `Legacy` methods stay until their callers move; then they go.

use super::*;

impl Store {
    /// Where a file is kept. Named `.gcode` from the first version, whatever is in it (a label is a
    /// PNG), so stores from then keep working.
    pub fn file_path(&self, id: &str) -> std::path::PathBuf {
        self.inner.files_dir.join(format!("{id}.gcode"))
    }

    pub fn thumbnail_path(&self, id: &str) -> std::path::PathBuf {
        self.inner.files_dir.join(format!("{id}.png"))
    }

    /// A new file of a space, already on disk at [`Store::file_path`] of `id`.
    pub async fn add_file(
        &self,
        space: i64,
        id: &str,
        name: &str,
        size: u64,
        meta: &crate::gcode::FileMeta,
    ) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query!(
            "INSERT INTO files (id, space_id, name, size, uploaded_at, meta) VALUES ($1, $2, $3, $4, at_unix($5), $6)",
            id,
            space,
            name,
            size as i64,
            now(),
            serde_json::to_string(meta).unwrap_or_default()
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    /// A space's files, newest first.
    pub async fn files(&self, space: i64) -> sqlx::Result<Vec<StoredFile>> {
        let mut tx = self.pg().space(space).await?;
        let rows = sqlx::query!(
            r#"SELECT id, name, size, unix_seconds(uploaded_at) AS "uploaded_at!", meta FROM files
               WHERE space_id = $1 ORDER BY uploaded_at DESC, name COLLATE "C""#,
            space
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(|r| StoredFile {
                id: r.id,
                name: r.name,
                size: r.size as u64,
                uploaded_at: r.uploaded_at,
                meta: serde_json::from_str(&r.meta).unwrap_or_default(),
            })
            .collect())
    }

    pub async fn file(&self, space: i64, id: &str) -> sqlx::Result<Option<StoredFile>> {
        Ok(self.files(space).await?.into_iter().find(|f| f.id == id))
    }

    /// Reads again what the hub keeps about every file it kept before it read as much as it does now
    /// (where the moves go, the highest power; temperatures, colour, objects and QOI and JPG
    /// thumbnails, as Prusa's gcode-metadata reads them), and what its name tells; a thumbnail it did
    /// not have is kept now. Across spaces: the admin pool.
    pub async fn reread_files(&self) -> sqlx::Result<usize> {
        let stale: Vec<(String, String)> = sqlx::query!("SELECT id, name, meta FROM files")
            .fetch_all(&self.pg().admin)
            .await?
            .into_iter()
            .filter(|r| !r.meta.contains("bounds_mm") || !r.meta.contains("\"objects\""))
            .map(|r| (r.id, r.name))
            .collect();
        for (id, name) in &stale {
            if let Ok((mut meta, png)) = crate::gcode::read(&self.file_path(id)) {
                crate::gcode::from_name(&mut meta, name);
                let thumbnail = self.thumbnail_path(id);
                if let Some(png) = png
                    && !thumbnail.exists()
                {
                    let _ = std::fs::write(&thumbnail, png);
                }
                sqlx::query!(
                    "UPDATE files SET meta = $1 WHERE id = $2",
                    serde_json::to_string(&meta).unwrap_or_default(),
                    id
                )
                .execute(&self.pg().admin)
                .await?;
            }
        }
        Ok(stale.len())
    }

    /// Removes a file, from the store and from disk. Returns whether it was there.
    pub async fn remove_file(&self, space: i64, id: &str) -> sqlx::Result<bool> {
        let mut tx = self.pg().space(space).await?;
        // Not while a machine runs it (or has it queued).
        let in_use = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "n!" FROM jobs WHERE file_id = $1
               AND state IN ('loaded', 'queued', 'printing', 'paused', 'stopping')"#,
            id
        )
        .fetch_one(&mut *tx)
        .await?;
        if in_use > 0 {
            return Ok(false);
        }
        let removed = sqlx::query!("DELETE FROM files WHERE id = $1 AND space_id = $2", id, space)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        if removed > 0 {
            let _ = std::fs::remove_file(self.file_path(id));
            let _ = std::fs::remove_file(self.thumbnail_path(id));
        }
        Ok(removed > 0)
    }
}
