//! Files on Postgres (`migrations/20261118000300_machines.sql`): `files.rs`'s queries as async [`Store`]
//! methods (hub core v2 step 2, lane 3). The G-code and thumbnails stay on disk, beside the database.

use super::*;

impl Store {
    /// Where a file is kept. Named `.gcode` from the first version, whatever is in it (a label is a
    /// PNG), so stores from then keep working.
    pub fn file_path(&self, id: &str) -> std::path::PathBuf {
        self.inner.files_dir.join(format!("{id}.gcode"))
    }

    /// Where uploaded files are kept.
    pub fn files_dir(&self) -> &std::path::Path {
        &self.inner.files_dir
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
        self.add_file_as(space, id, name, size, meta, false).await
    }

    /// [`Store::add_file`], or a temporary copy on its way to a printer's storage (`incoming`, D-J
    /// #225), in one: never listed in Files, not even for a moment.
    pub async fn add_file_as(
        &self,
        space: i64,
        id: &str,
        name: &str,
        size: u64,
        meta: &crate::gcode::FileMeta,
        incoming: bool,
    ) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query!(
            "INSERT INTO files (id, space_id, name, size, uploaded_at, meta, incoming) VALUES ($1, $2, $3, $4, at_unix($5), $6, $7)",
            id,
            space,
            name,
            size as i64,
            now(),
            serde_json::to_string(meta).unwrap_or_default(),
            incoming
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// A space's files, newest first.
    pub async fn files(&self, space: i64) -> sqlx::Result<Vec<StoredFile>> {
        let mut tx = self.pg().space(space).await?;
        let rows = sqlx::query!(
            r#"SELECT id, name, size, unix_seconds(uploaded_at) AS "uploaded_at!", meta, incoming FROM files
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
                incoming: r.incoming,
            })
            .collect())
    }

    pub async fn file(&self, space: i64, id: &str) -> sqlx::Result<Option<StoredFile>> {
        Ok(self.files(space).await?.into_iter().find(|f| f.id == id))
    }

    /// Reads again what the hub keeps about every file it kept before it read as much as it does now
    /// (where the moves go, the highest power; temperatures, colour, objects and QOI and JPG
    /// thumbnails, as Prusa's gcode-metadata reads them), and what its name tells; a thumbnail it did
    /// not have is kept now. Each space's in its transaction (the spaces as instance admin,
    /// [`Store::space_ids`]).
    pub async fn reread_files(&self) -> sqlx::Result<usize> {
        let mut stale: Vec<(i64, String, String)> = Vec::new();
        for space in self.space_ids().await? {
            let mut tx = self.pg().space(space).await?;
            stale.extend(
                sqlx::query!("SELECT id, name, meta FROM files")
                    .fetch_all(&mut *tx)
                    .await?
                    .into_iter()
                    .filter(|r| !r.meta.contains("bounds_mm") || !r.meta.contains("\"objects\""))
                    .map(|r| (space, r.id, r.name)),
            );
            tx.commit().await?;
        }
        for (space, id, name) in &stale {
            if let Ok((mut meta, png)) = crate::gcode::read(&self.file_path(id)) {
                crate::gcode::from_name(&mut meta, name);
                let thumbnail = self.thumbnail_path(id);
                if let Some(png) = png
                    && !thumbnail.exists()
                {
                    let _ = std::fs::write(&thumbnail, png);
                }
                let mut tx = self.pg().space(*space).await?;
                sqlx::query!(
                    "UPDATE files SET meta = $1 WHERE id = $2",
                    serde_json::to_string(&meta).unwrap_or_default(),
                    id
                )
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
        Ok(stale.len())
    }

    /// Removes a file, from the store and from disk. Returns whether it was there.
    pub async fn remove_file(&self, space: i64, id: &str) -> sqlx::Result<bool> {
        // The check and the removal are one transaction, the file's row locked: a job put on it
        // meanwhile ([`Store::add_job`] locks the same row to share) waits, and then finds it gone;
        // or it is there before the check sees it. A machine that moved away keeps its jobs in this
        // space: the check sees them.
        let mut tx = self.pg().space(space).await?;
        let there = sqlx::query_scalar!(
            "SELECT id FROM files WHERE id = $1 AND space_id = $2 FOR UPDATE",
            id,
            space
        )
        .fetch_optional(&mut *tx)
        .await?
        .is_some();
        // Not while a machine runs it (or has it queued).
        if !there || Self::file_in_use(&mut tx, id).await? {
            return Ok(false);
        }
        // Nor while a part's file or a transfer's record names it (their keys,
        // design decisions/20261119-composite-foreign-keys.md).
        let removed = match sqlx::query!(
            "DELETE FROM files WHERE id = $1 AND space_id = $2",
            id,
            space
        )
        .execute(&mut *tx)
        .await
        {
            Ok(done) => done.rows_affected(),
            Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => return Ok(false),
            Err(e) => return Err(e),
        };
        tx.commit().await?;
        if removed > 0 {
            let _ = std::fs::remove_file(self.file_path(id));
            let _ = std::fs::remove_file(self.thumbnail_path(id));
        }
        Ok(removed > 0)
    }

    /// A temporary copy (D-J #225) goes, with its transfers' records, as Prusa Connect Local's
    /// `incomingCopies.remove` (`internal/app/incoming.go`); not while one of its transfers is still
    /// on its way (its `inUse`). Whether it went.
    pub async fn forget_incoming(&self, space: i64, id: &str) -> Result<bool, BoxError> {
        let mut tx = self.pg().space(space).await?;
        let incoming: Option<bool> =
            sqlx::query_scalar("SELECT incoming FROM files WHERE space_id = $1 AND id = $2")
                .bind(space)
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if incoming != Some(true) {
            return Ok(false);
        }
        let on_its_way: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM printer_transfers WHERE space_id = $1 AND file_id = $2
                 AND state NOT IN ('finished', 'failed', 'done', 'gone'))",
        )
        .bind(space)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if on_its_way {
            return Ok(false);
        }
        sqlx::query("DELETE FROM printer_transfers WHERE space_id = $1 AND file_id = $2")
            .bind(space)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(self.remove_file(space, id).await?)
    }

    /// Prusa Connect Local's sweep of its temporary copies (`incoming.go`, `sweep`): every
    /// [`INCOMING_SWEEP_S`], the copies older than [`INCOMING_KEEP_S`] go (a send that failed keeps
    /// its copy that long, to be sent again); at start (`first`), every copy left from before.
    pub async fn sweep_incoming(&self, first: bool) -> Result<usize, BoxError> {
        let cutoff = now() - INCOMING_KEEP_S;
        let mut gone = 0;
        for space in self.space_ids().await? {
            for file in self.files(space).await? {
                if file.incoming
                    && (first || file.uploaded_at < cutoff)
                    && self.forget_incoming(space, &file.id).await?
                {
                    gone += 1;
                }
            }
        }
        Ok(gone)
    }
}

/// How long a temporary copy whose send failed is kept: Prusa Connect Local's `incomingKeep`
/// (`internal/app/incoming.go:39`, `time.Hour`).
pub const INCOMING_KEEP_S: i64 = 3600;
/// How often the temporary copies are swept: its `incomingSweep` (`incoming.go:42`, 10 minutes).
pub const INCOMING_SWEEP_S: u64 = 600;
