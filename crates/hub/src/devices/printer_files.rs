//! Files on a Prusa printer's USB stick, over its built-in Connect (a MINI on firmware 6.3):
//! looking through it, a file's picture, printing from it, deleting, making a folder, and
//! sending a file of the space to it (then printing it, for a job).
//!
//! What the firmware does (read in Prusa-Firmware-Buddy 6.3.0, GPL; nothing copied):
//!
//! - `SEND_FILE_INFO {path}`: it answers with a `FILE_INFO` event. For a folder: `children`
//!   (`name`, the short 8.3 name to use in paths; `display_name`, the long one; `size`,
//!   `m_timestamp`, `type`: `PRINT_FILE`, `FOLDER`, `FIRMWARE`, `FILE`). For a file: its
//!   `preview` (a PNG thumbnail from the G-code, base64: the first larger than 17×17) and the
//!   G-code's metadata (`estimated printing time (normal mode)`, `filament_type`, ...).
//! - `START_PRINT {path}`, `DELETE_FILE {path}`, `DELETE_FOLDER {path}`, `CREATE_FOLDER {path}`;
//!   a change comes back as `FILE_CHANGED`.
//! - `START_ENCRYPTED_DOWNLOAD {path, key, iv, orig_size}`: the printer fetches
//!   `GET /f/<iv as hex>/raw` from the host and port it reports to (with `Range: bytes=N-`,
//!   N a multiple of 16, to go on after a break), and decrypts it with AES-128 in CTR mode:
//!   the counter block is the iv's first 12 bytes and the block number (big-endian) in the last
//!   4, from 0. The sender pads to whole blocks; `orig_size` says where the file ends. Then
//!   `TRANSFER_FINISHED` (or `TRANSFER_ABORTED`, `TRANSFER_STOPPED`), with `start_cmd_id`.
//!
//! The `iv` in the address is the only thing that lets the download through (the printer
//! sends no token with it): 96 random bits, a transfer of one file to one printer, for a day.

use crate::access::Access;
use crate::store::pg::Bypass;
use crate::store::{BoxError, JobState, Legacy, now};
use crate::{Store, failed};
use aes::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};
use axum::{
    Form, Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{Value, json};

type Cipher = ctr::Ctr32BE<aes::Aes128>;

/// A folder is listed again when its listing is older than this (files come and go).
const FOLDER_FRESH_SECONDS: i64 = 30;
/// A file's details and picture are kept this long: a file on the stick does not change
/// without a `FILE_CHANGED` (which forgets it at once) or a new size in its folder.
const FILE_FRESH_SECONDS: i64 = 6 * 3600;
/// Not asked again while an answer may still be on its way (a MINI takes 2 to 4 s a file;
/// a command waits at most two minutes for one).
const ASK_EVERY_SECONDS: i64 = 6;
const PREFETCH_ASK_EVERY_SECONDS: i64 = 120;
/// At most this many files of a folder are read ahead.
const PREFETCH_MAX: usize = 60;
/// A transfer's address works this long.
const TRANSFER_SECONDS: i64 = 24 * 3600;

/// A command for the log, its key left out.
pub fn for_the_log(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(mut v) if v.get("kwargs").and_then(|k| k.get("key")).is_some() => {
            v["kwargs"]["key"] = json!("…");
            v.to_string()
        }
        _ => body.replace('\n', " | "),
    }
}

/// A path on the stick: under `/usb`, no `..`, not too long.
fn usb_path(path: &str) -> Option<String> {
    let path = path.trim().trim_end_matches('/');
    let ok = (path == "/usb" || path.starts_with("/usb/"))
        && path.len() <= 200
        && !path.split('/').any(|p| p == ".." || p == ".")
        && !path.contains(['\\', '\0', '\n', '"']);
    ok.then(|| path.to_owned())
}

fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => "/usb",
        Some(i) => &path[..i],
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    text.len()
        .is_multiple_of(2)
        .then(|| {
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
                .collect()
        })
        .flatten()
}

/// What the printer last said of a path and when, and when it was last asked.
type Seen = (Option<(Value, i64)>, Option<i64>);

/// A transfer of a space's file to a printer.
#[derive(Debug, Clone)]
pub struct Transfer {
    pub iv: String,
    pub machine: String,
    pub space: i64,
    pub file: String,
    pub key: String,
    pub size: u64,
    pub path: String,
    pub job: Option<i64>,
    pub print_after: bool,
    pub state: String,
}

/// The columns of a transfer, as the transfer queries read them ([`TransferRow`]).
macro_rules! transfer_select {
    ($tail:literal) => {
        concat!(
            "SELECT iv, machine_id, space_id, file_id, key, size, path, job_id, print_after, state
             FROM printer_transfers WHERE ",
            $tail
        )
    };
}

#[derive(sqlx::FromRow)]
struct TransferRow {
    iv: String,
    machine_id: String,
    space_id: i64,
    file_id: String,
    key: String,
    size: i64,
    path: String,
    job_id: Option<i64>,
    print_after: bool,
    state: String,
}

impl From<TransferRow> for Transfer {
    fn from(r: TransferRow) -> Self {
        Transfer {
            iv: r.iv,
            machine: r.machine_id,
            space: r.space_id,
            file: r.file_id,
            key: r.key,
            size: r.size as u64,
            path: r.path,
            job: r.job_id,
            print_after: r.print_after,
            state: r.state,
        }
    }
}

/// What a printer's stick holds and what is on its way to it, on Postgres
/// (`store/migrations/20261118000300_machines.sql`). Each in the machine's space's transaction (the
/// caller says the space); a printer's stick moves with it to another space (Store::move_machine).
impl Store {
    /// What the printer last said of a path, and when (Unix seconds); and when it was last
    /// asked.
    async fn printer_file(&self, space: i64, machine: &str, path: &str) -> sqlx::Result<Seen> {
        let mut tx = self.pg().space(space).await?;
        let row = sqlx::query!(
            "SELECT data, unix_seconds(at) AS at, unix_seconds(asked_at) AS asked_at
             FROM printer_files WHERE machine_id = $1 AND path = $2",
            machine,
            path
        )
        .fetch_optional(&mut *tx)
        .await?;
        Ok(match row {
            Some(r) => (
                r.data.and_then(|d| serde_json::from_str(&d).ok()).zip(r.at),
                r.asked_at,
            ),
            None => (None, None),
        })
    }

    async fn set_printer_file(
        &self,
        space: i64,
        machine: &str,
        path: &str,
        data: &Value,
    ) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query!(
            "INSERT INTO printer_files (machine_id, space_id, path, data, at) VALUES ($1, $2, $3, $4, at_unix($5))
             ON CONFLICT (machine_id, path) DO UPDATE SET data = excluded.data, at = excluded.at",
            machine,
            space,
            path,
            data.to_string(),
            now()
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn asked_printer_file(&self, space: i64, machine: &str, path: &str) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query!(
            "INSERT INTO printer_files (machine_id, space_id, path, asked_at) VALUES ($1, $2, $3, at_unix($4))
             ON CONFLICT (machine_id, path) DO UPDATE SET asked_at = excluded.asked_at",
            machine,
            space,
            path,
            now()
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// A path changed: it and its folder are read again when next looked at.
    async fn forget_printer_path(&self, space: i64, machine: &str, path: &str) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query!(
            "UPDATE printer_files SET at = at_unix(0), asked_at = NULL
             WHERE machine_id = $1 AND (path = $2 OR path = $3)",
            machine,
            path,
            parent(path)
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// A new transfer of a file to a printer's stick: its iv (the address) and key, random.
    /// For a job, the file is printed once it is there.
    async fn create_transfer(
        &self,
        space: i64,
        machine: &str,
        file: &str,
        size: u64,
        path: &str,
        job: Option<i64>,
    ) -> sqlx::Result<Transfer> {
        let mut tx = self.pg().space(space).await?;
        let print_after = job.is_some();
        let (mut iv, mut key) = ([0u8; 16], [0u8; 16]);
        rand::rngs::OsRng.fill_bytes(&mut iv[..12]);
        rand::rngs::OsRng.fill_bytes(&mut key);
        let transfer = Transfer {
            iv: hex(&iv),
            machine: machine.to_owned(),
            space,
            file: file.to_owned(),
            key: hex(&key),
            size,
            path: path.to_owned(),
            job,
            print_after,
            state: "waiting".to_owned(),
        };
        sqlx::query!(
            "INSERT INTO printer_transfers (iv, machine_id, space_id, file_id, key, size, path, job_id, print_after, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, at_unix($10))",
            transfer.iv,
            machine,
            space,
            file,
            transfer.key,
            size as i64,
            path,
            job,
            print_after,
            now()
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(transfer)
    }

    /// A transfer by its address, while it lasts. The printer sends no token with it: its address
    /// (random, 96 bits) is its secret (`Bypass::Secret`).
    pub async fn transfer(&self, iv: &str) -> sqlx::Result<Option<Transfer>> {
        let row: Option<TransferRow> =
            sqlx::query_as(transfer_select!("iv = $1 AND created_at > at_unix($2)"))
                .bind(iv)
                .bind(now() - TRANSFER_SECONDS)
                .fetch_optional(self.pg().admin(Bypass::Secret))
                .await?;
        Ok(row.map(Transfer::from))
    }

    /// The paths on a printer's stick that we put there (downloads that got through).
    async fn sent_paths(&self, space: i64, machine: &str) -> sqlx::Result<Vec<String>> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query_scalar!(
            r#"SELECT DISTINCT path AS "path!" FROM printer_transfers
               WHERE machine_id = $1 AND state IN ('finished', 'starting', 'printing', 'done')"#,
            machine
        )
        .fetch_all(&mut *tx)
        .await
    }

    /// A file we put on the stick is not there any more: its transfers no longer say so.
    async fn gone_from_stick(&self, space: i64, machine: &str, path: &str) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query!(
            "UPDATE printer_transfers SET state = 'gone'
             WHERE machine_id = $1 AND path = $2 AND state IN ('finished', 'done')",
            machine,
            path
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Where this file already is on the printer's stick, put there by us before: the same
    /// file (and size), so it need not go again. With when that transfer began.
    async fn on_stick(
        &self,
        space: i64,
        machine: &str,
        file: &str,
        size: u64,
    ) -> sqlx::Result<Option<(String, i64)>> {
        let mut tx = self.pg().space(space).await?;
        Ok(sqlx::query!(
            r#"SELECT path, unix_seconds(created_at) AS "created_at!" FROM printer_transfers
               WHERE machine_id = $1 AND file_id = $2 AND size = $3
                 AND state IN ('finished', 'starting', 'printing', 'done')
               ORDER BY created_at DESC LIMIT 1"#,
            machine,
            file,
            size as i64
        )
        .fetch_optional(&mut *tx)
        .await?
        .map(|r| (r.path, r.created_at)))
    }

    /// How many downloads a job tried: not more than a few, then it fails.
    async fn transfers_of_job(&self, space: i64, job: i64) -> sqlx::Result<i64> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "n!" FROM printer_transfers WHERE job_id = $1"#,
            job
        )
        .fetch_one(&mut *tx)
        .await
    }

    /// How many files are on their way to a machine (what holds it in its space,
    /// [`Store::move_machine`], in its transaction on the admin pool).
    pub(crate) async fn files_on_their_way(
        admin: &mut sqlx::PgConnection,
        machine: &str,
    ) -> sqlx::Result<i64> {
        sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "n!" FROM printer_transfers WHERE machine_id = $1 AND state IN ('waiting', 'sent')"#,
            machine
        )
        .fetch_one(admin)
        .await
    }

    /// A machine moved to another space: its transfers not through yet fail (in
    /// [`Store::move_machine`]'s transaction on the admin pool).
    pub(crate) async fn transfers_moved_away(
        admin: &mut sqlx::PgConnection,
        machine: &str,
    ) -> sqlx::Result<()> {
        sqlx::query!(
            "UPDATE printer_transfers SET state = 'failed', reason = 'the machine moved to another space'
             WHERE machine_id = $1 AND state NOT IN ('finished', 'done', 'failed')",
            machine
        )
        .execute(admin)
        .await?;
        Ok(())
    }

    /// Prints a file that is on the stick: START_PRINT, and the transfer waits for it.
    async fn print_from_stick(&self, transfer: &Transfer, machine: &str) -> Result<(), BoxError> {
        let command = json!({"command": "START_PRINT", "kwargs": {"path": transfer.path}});
        let label = format!("Print {}", transfer.path.rsplit('/').next().unwrap_or(""));
        let link = self.link_of(transfer.space, machine).await?;
        let id = self.lock().unwrap().queue_connect_command(
            link.as_deref(),
            machine,
            &label,
            &command,
            false,
        );
        if let Some(id) = id {
            self.set_transfer(transfer.space, &transfer.iv, "starting", Some(id), None)
                .await?;
        }
        Ok(())
    }

    /// A job's file again, under a name not on the stick yet: after the printer said a file
    /// by that name exists (`machine_reason` `FILE_EXISTS`, `planner.cpp:294`).
    async fn send_again(&self, old: &Transfer, job: i64, machine: &str) -> Result<bool, BoxError> {
        if self.transfers_of_job(old.space, job).await? >= 3 {
            return Ok(false);
        }
        let Some(file) = self.file(old.space, &old.file).await? else {
            return Ok(false);
        };
        let mut taken = names_in(self, old.space, machine, "/usb").await;
        taken.push(old.path.rsplit('/').next().unwrap_or("").to_owned());
        let path = format!("/usb/{}", stick_name(&file.name, &taken));
        let fresh = self
            .create_transfer(old.space, machine, &file.id, file.size, &path, Some(job))
            .await?;
        Ok(self.start_transfer(&fresh, &file.name).await?.is_some())
    }

    /// Everything on a printer's stick is read again when next looked at.
    async fn forget_printer_stick(&self, space: i64, machine: &str) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query!(
            "UPDATE printer_files SET at = at_unix(0), asked_at = NULL WHERE machine_id = $1",
            machine
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// A path deleted from the stick: forgotten, and taken out of its folder's listing
    /// (which stays as fresh as it was: the rest of the folder did not change). In one
    /// transaction.
    async fn drop_printer_path(&self, space: i64, machine: &str, path: &str) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        // `LIKE` as SQLite had it: ASCII case does not matter.
        sqlx::query!(
            "DELETE FROM printer_files WHERE machine_id = $1 AND (path = $2 OR path ILIKE $3)",
            machine,
            path,
            format!("{path}/%")
        )
        .execute(&mut *tx)
        .await?;
        let folder = parent(path);
        let name = path.rsplit('/').next().unwrap_or("");
        let listing = sqlx::query!(
            "SELECT data, unix_seconds(at) AS at FROM printer_files
             WHERE machine_id = $1 AND path = $2 FOR UPDATE",
            machine,
            folder
        )
        .fetch_optional(&mut *tx)
        .await?
        .and_then(|r| {
            r.data
                .and_then(|d| serde_json::from_str::<Value>(&d).ok())
                .zip(r.at)
        });
        if let Some((mut data, at)) = listing
            && let Some(children) = data["children"].as_array_mut()
        {
            let before = children.len();
            children.retain(|c| c["name"].as_str() != Some(name));
            if children.len() != before {
                if let Some(count) = data["file_count"].as_i64() {
                    data["file_count"] = json!(count - 1);
                }
                sqlx::query!(
                    "UPDATE printer_files SET data = $3, at = at_unix($4) WHERE machine_id = $1 AND path = $2",
                    machine,
                    folder,
                    data.to_string(),
                    at
                )
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await
    }

    /// What a folder's listing says of one of its children, by its path.
    async fn listed_child(
        &self,
        space: i64,
        machine: &str,
        path: &str,
    ) -> sqlx::Result<Option<Value>> {
        let name = path.rsplit('/').next().unwrap_or("");
        Ok(self
            .printer_file(space, machine, parent(path))
            .await?
            .0
            .and_then(|(data, _)| {
                data["children"]
                    .as_array()?
                    .iter()
                    .find(|c| c["name"].as_str() == Some(name))
                    .cloned()
            }))
    }

    async fn transfer_of_command(
        &self,
        space: i64,
        machine: &str,
        command: u32,
    ) -> sqlx::Result<Option<Transfer>> {
        let mut tx = self.pg().space(space).await?;
        let row: Option<TransferRow> =
            sqlx::query_as(transfer_select!("machine_id = $1 AND command_id = $2"))
                .bind(machine)
                .bind(command as i64)
                .fetch_optional(&mut *tx)
                .await?;
        Ok(row.map(Transfer::from))
    }

    async fn transfer_of_job(&self, space: i64, job: i64) -> sqlx::Result<Option<Transfer>> {
        let mut tx = self.pg().space(space).await?;
        let row: Option<TransferRow> = sqlx::query_as(transfer_select!(
            "job_id = $1 ORDER BY created_at DESC LIMIT 1"
        ))
        .bind(job)
        .fetch_optional(&mut *tx)
        .await?;
        Ok(row.map(Transfer::from))
    }

    async fn set_transfer(
        &self,
        space: i64,
        iv: &str,
        state: &str,
        command: Option<u32>,
        reason: Option<&str>,
    ) -> sqlx::Result<()> {
        let mut tx = self.pg().space(space).await?;
        sqlx::query!(
            "UPDATE printer_transfers SET state = $2, command_id = COALESCE($3, command_id), reason = $4 WHERE iv = $1",
            iv,
            state,
            command.map(i64::from),
            reason
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Queues the download of a transfer on its printer.
    async fn start_transfer(
        &self,
        transfer: &Transfer,
        name: &str,
    ) -> Result<Option<u32>, BoxError> {
        let command = json!({
            "command": "START_ENCRYPTED_DOWNLOAD",
            "kwargs": {
                "path": transfer.path,
                "key": transfer.key,
                "iv": transfer.iv,
                "orig_size": transfer.size,
            }
        });
        let link = self.link_of(transfer.space, &transfer.machine).await?;
        let id = self.lock().unwrap().queue_connect_command(
            link.as_deref(),
            &transfer.machine,
            &format!("Send {name} to the USB stick"),
            &command,
            false,
        );
        if let Some(id) = id {
            self.set_transfer(transfer.space, &transfer.iv, "sent", Some(id), None)
                .await?;
        }
        Ok(id)
    }
}

/// A free name on the stick for a file: plain ASCII letters, digits, `-` and `_` (what FAT
/// takes everywhere), its extension kept, a number added if the folder has one by that name.
fn stick_name(name: &str, taken: &[String]) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) => (stem, ext.to_ascii_lowercase()),
        None => (name, "gcode".to_owned()),
    };
    let mut clean: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    clean.truncate(60);
    if clean.trim_matches('_').is_empty() {
        clean = "file".to_owned();
    }
    let mut candidate = format!("{clean}.{ext}");
    let mut n = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&candidate)) {
        candidate = format!("{clean}-{n}.{ext}");
        n += 1;
    }
    candidate
}

/// The long names in a folder, as the printer last listed it.
/// The names on the stick in a folder, as far as we know: what the printer listed, and what
/// we put there ourselves (a listing can be older than our last upload).
async fn names_in(store: &Store, space: i64, machine: &str, folder: &str) -> Vec<String> {
    let mut names = listed_in(store, space, machine, folder)
        .await
        .unwrap_or_default();
    names.extend(
        store
            .sent_paths(space, machine)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| {
                p.strip_prefix(folder)
                    .and_then(|rest| rest.strip_prefix('/'))
                    .filter(|name| !name.contains('/'))
                    .map(str::to_owned)
            }),
    );
    names
}

/// Where a file we put on the stick before still is. Not where the printer's listing of
/// that folder no longer has it: deleted since (on the printer, or by us; the printer says
/// so by its short name, which no transfer of ours is known by), and so forgotten.
async fn still_on_stick(
    store: &Store,
    space: i64,
    machine: &str,
    file: &str,
    size: u64,
) -> sqlx::Result<Option<String>> {
    let Some((path, began)) = store.on_stick(space, machine, file, size).await? else {
        return Ok(None);
    };
    let (folder, name) = path.rsplit_once('/').unwrap_or(("/usb", path.as_str()));
    // Only a listing the printer gave after the transfer (one from before it does not have
    // the file yet; a finished transfer marks its folder to be read again, `at` 0).
    let after = store
        .printer_file(space, machine, folder)
        .await?
        .0
        .is_some_and(|(_, at)| at > began);
    match listed_in(store, space, machine, folder).await {
        Some(names) if after && !names.iter().any(|n| n == name) => {
            store.gone_from_stick(space, machine, &path).await?;
            Ok(None)
        }
        _ => Ok(Some(path)),
    }
}

/// The names (long and short) the printer last listed in a folder of its stick; None when
/// it has not listed that folder yet.
async fn listed_in(store: &Store, space: i64, machine: &str, folder: &str) -> Option<Vec<String>> {
    let entry = store.printer_file(space, machine, folder).await.ok()?.0?;
    let children = entry.0["children"].as_array()?.clone();
    Some(
        children
            .iter()
            .flat_map(|c| [c["display_name"].as_str(), c["name"].as_str()])
            .flatten()
            .map(str::to_owned)
            .collect(),
    )
}

/// An event from a printer about its files and transfers (called for every event).
pub async fn on_event(
    store: &Store,
    token: &str,
    name: &str,
    event: &Value,
) -> Result<(), BoxError> {
    let Some((machine, space)) = store.placed(token).await? else {
        return Ok(());
    };
    let caller = store.caller(token).await?;
    let data = &event["data"];
    match name {
        // Its state changed; while it wants someone (ATTENTION) or is in error (ERROR), it says
        // what is on its screen (Buddy's connect/render.cpp: code, title, text, buttons).
        "STATE_CHANGED" => {
            let asks = matches!(event["state"].as_str(), Some("ATTENTION" | "ERROR"));
            let text = |key: &str| {
                data[key]
                    .as_str()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(str::to_owned)
            };
            let mut store = store.lock().unwrap();
            match (asks, text("title"), text("text")) {
                (true, title, body) if title.is_some() || body.is_some() => {
                    let title = title.or_else(|| body.clone()).unwrap_or_default();
                    let mut body = body.unwrap_or_default();
                    // Prusa's error codes (Prusa-Error-Codes: printer, category, number in
                    // five digits) each have a help page at prusa.io/<code>, as the printer's
                    // own screen shows.
                    if let Some(code) = text("code") {
                        let help = if code.len() == 5 && code.bytes().all(|b| b.is_ascii_digit()) {
                            format!(", prusa.io/{code}")
                        } else {
                            Default::default()
                        };
                        body = format!("{body} (#{code}{help})").trim().to_owned();
                    }
                    store.asking.insert(machine.clone(), (title, body));
                    // Its buttons, answered from the website with DIALOG_ACTION: the dialog
                    // (the event's `dialog_id`, `render.cpp:673`) and the button's name.
                    // Buddy refuses a DIALOG_ACTION without a `dialog_id`, so the buttons are
                    // offered only when the printer named a dialog.
                    let buttons: Vec<String> = data["buttons"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|b| b.as_str().map(str::to_owned))
                        .collect();
                    let dialog = event["dialog_id"].as_u64();
                    store.dialogs.insert(machine.clone(), (buttons, dialog));
                }
                (true, ..) => {}
                (false, ..) => {
                    store.asking.remove(&machine);
                    store.dialogs.remove(&machine);
                }
            }
            store.changed();
        }
        "FILE_INFO" => {
            if let Some(path) = data["path"].as_str().and_then(usb_path) {
                store.set_printer_file(space, &machine, &path, data).await?;
                // A folder: its files' details and pictures are read ahead, one by one.
                if data["children"].is_array() {
                    prefetch(store, space, &machine, &path, data).await?;
                }
                store.changed();
            }
        }
        // Which file a print is (ours or the printer's own): said as it starts, and when
        // asked (SEND_JOB_INFO, see `on_report`); its telemetry only says how far.
        "JOB_INFO" => {
            if let Some(name) = data["display_name"]
                .as_str()
                .or_else(|| data["path"].as_str())
                .filter(|n| !n.is_empty())
            {
                let job = event["job_id"].as_u64().or_else(|| data["job_id"].as_u64());
                store
                    .lock()
                    .unwrap()
                    .printing_file
                    .insert(token.to_owned(), (job, name.to_owned()));
            }
        }
        // A file or folder made or deleted (by a command of ours, or on the printer itself).
        // Buddy 6.3 (connect/render.cpp): `old_path` when deleted, `new_path` when made,
        // `new_path` with `rescan` when several things changed at once; paths in 8.3 form.
        "FILE_CHANGED" => {
            if data["rescan"].as_bool() == Some(true) {
                store.forget_printer_stick(space, &machine).await?;
            }
            if let Some(path) = data["old_path"].as_str().and_then(usb_path) {
                // Gone at once from its folder's listing: not shown until the folder is
                // read again.
                store.drop_printer_path(space, &machine, &path).await?;
            }
            if let Some(path) = data["new_path"].as_str() {
                store.forget_printer_path(space, &machine, path).await?;
            }
            store.changed();
        }
        // A download's start is answered with TRANSFER_INFO (Buddy's handle_transfer_result,
        // 6.3 and later) once the printer takes it, but the file is still on its way: said
        // so, until TRANSFER_FINISHED (or a stop) settles the command.
        "TRANSFER_INFO" | "FINISHED" => {
            if let Some(command) = event["command_id"].as_u64()
                && let Some(transfer) = store
                    .transfer_of_command(space, &machine, command as u32)
                    .await?
                && !matches!(
                    transfer.state.as_str(),
                    "finished" | "failed" | "starting" | "printing" | "done"
                )
            {
                store.lock().unwrap().command_went(
                    caller.as_ref(),
                    command as u32,
                    crate::store::CommandState::Finished,
                    Some("taken: the file is on its way (see USB)".to_owned()),
                    Vec::new(),
                );
            }
        }
        "TRANSFER_FINISHED" | "TRANSFER_ABORTED" | "TRANSFER_STOPPED" => {
            let command = data["start_cmd_id"]
                .as_u64()
                .or_else(|| event["command_id"].as_u64());
            let transfer = match command {
                Some(c) => store
                    .transfer_of_command(space, &machine, c as u32)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            let Some(transfer) = transfer else {
                return Ok(());
            };
            store
                .forget_printer_path(space, &machine, &transfer.path)
                .await?;
            // The download command is done with its transfer, whatever it said before.
            if let Some(command) = command {
                let (state, reason) = if name == "TRANSFER_FINISHED" {
                    (crate::store::CommandState::Finished, None)
                } else {
                    (crate::store::CommandState::Rejected, Some(name.to_owned()))
                };
                store.lock().unwrap().command_went(
                    caller.as_ref(),
                    command as u32,
                    state,
                    reason,
                    Vec::new(),
                );
            }
            if name == "TRANSFER_FINISHED" {
                println!("on the stick: {}", transfer.path);
                store
                    .set_transfer(space, &transfer.iv, "finished", None, None)
                    .await?;
                // A download opens the file's print preview on the printer's screen (Buddy's
                // one-click print), and while it shows the file stays where the download
                // put it, listed read-only. To print: started from there (Buddy takes
                // START_PRINT while the preview shows). Only to the stick: the preview is
                // closed (`on_report`; ours, approved, audit D6).
                if transfer.print_after {
                    store.print_from_stick(&transfer, &machine).await?;
                } else {
                    store.lock().unwrap().close_preview.insert(machine.clone());
                }
            } else {
                // Why, when it says: STORAGE_FAILURE (the USB stick), NETWORK_FAILURE.
                let why = match event["machine_reason"].as_str().filter(|r| !r.is_empty()) {
                    Some("STORAGE_FAILURE") => format!(
                        "the printer stopped the download ({name}): it could not write to its USB stick"
                    ),
                    Some("NETWORK_FAILURE") => format!(
                        "the printer stopped the download ({name}): it could not reach the hub"
                    ),
                    Some(reason) => format!("the printer stopped the download ({name}, {reason})"),
                    None => format!("the printer stopped the download ({name})"),
                };
                store
                    .set_transfer(space, &transfer.iv, "failed", None, Some(&why))
                    .await?;
                if let Some(job) = transfer.job {
                    store
                        .set_job(space, job, JobState::Failed, Some(&why))
                        .await?;
                }
            }
            store.changed();
        }
        // Refused at once, or started and then failed: for a download or a print of ours.
        "REJECTED" | "FAILED" => {
            if let Some(command) = event["command_id"].as_u64()
                && let Some(transfer) = store
                    .transfer_of_command(space, &machine, command as u32)
                    .await?
            {
                let why = event["reason"]
                    .as_str()
                    .unwrap_or("refused by the printer")
                    .to_owned();
                store
                    .set_transfer(space, &transfer.iv, "failed", None, Some(&why))
                    .await?;
                // A file by that name is there already (not ours, or changed): once more,
                // under a free name. Buddy says so in `machine_reason` (`planner.cpp:294`,
                // `render.cpp:330`), not in the text of `reason`.
                let again = file_exists(event);
                if let Some(job) = transfer.job
                    && !(again && store.send_again(&transfer, job, &machine).await?)
                {
                    store
                        .set_job(space, job, JobState::Failed, Some(&why))
                        .await?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Whether a REJECTED event says a file by that name is there already: Buddy's `machine_reason`
/// `FILE_EXISTS` (`planner.cpp:294`, `render.cpp:330`), top level of the event, not the text of
/// `reason`.
pub(crate) fn file_exists(event: &Value) -> bool {
    event["machine_reason"].as_str() == Some("FILE_EXISTS")
}

/// Whether what the printer said of a file still holds: recent enough, and the same size
/// as its folder lists it now.
fn still_good(entry: &Option<(Value, i64)>, size: Option<u64>) -> bool {
    entry.as_ref().is_some_and(|(data, at)| {
        now() - at < FILE_FRESH_SECONDS
            && (size.is_none() || data["size"].as_u64().is_none() || data["size"].as_u64() == size)
    })
}

/// Asks the printer (in the background, behind anyone's own commands) for each printable file
/// of a folder whose details it does not have yet.
async fn prefetch(
    store: &Store,
    space: i64,
    machine: &str,
    folder: &str,
    listing: &Value,
) -> Result<(), BoxError> {
    let link = store.link_of(space, machine).await?;
    let children = listing["children"].as_array().cloned().unwrap_or_default();
    let mut asked = 0;
    for child in children
        .iter()
        .filter(|c| c["type"] == "PRINT_FILE" && c["read_only"] != true)
    {
        let Some(name) = child["name"].as_str() else {
            continue;
        };
        let path = format!("{folder}/{name}");
        if usb_path(&path).is_none() {
            continue;
        }
        let (entry, when) = store.printer_file(space, machine, &path).await?;
        if still_good(&entry, child["size"].as_u64())
            || when.is_some_and(|a| now() - a < PREFETCH_ASK_EVERY_SECONDS)
        {
            continue;
        }
        let command = json!({"command": "SEND_FILE_INFO", "kwargs": {"path": path}});
        let queued = store.lock().unwrap().queue_connect_command(
            link.as_deref(),
            machine,
            &format!("Read {path}"),
            &command,
            true,
        );
        if queued.is_some() {
            store.asked_printer_file(space, machine, &path).await?;
            asked += 1;
        }
        if asked >= PREFETCH_MAX {
            break;
        }
    }
    Ok(())
}

/// What the page shows of a file: its picture (whether there is one), time, material and
/// weight. The rest the printer sends (the slicer's whole configuration, for a text G-code)
/// stays here.
fn summary(data: &Value, at: i64) -> Value {
    let text = |k: &str| {
        data[k]
            .as_str()
            .map(str::to_owned)
            .or_else(|| data[k].as_f64().map(|n| n.to_string()))
    };
    json!({
        "picture": data["preview"].as_str().is_some_and(|p| !p.is_empty()),
        "at": at,
        "time": text("estimated printing time (normal mode)"),
        "material": text("filament_type").map(|m| m.replace(';', ", ")),
        "grams": text("filament used [g]"),
        "layer_mm": text("layer_height"),
        "nozzle_mm": text("nozzle_diameter"),
        "printer_model": text("printer_model"),
    })
}

/// After each report of a printer: a job waiting for it goes to its stick (when it is free),
/// and a job printing follows the printer's state to its end.
///
/// `active` is the machine's job that is not over ([`Store::active_job`]).
pub async fn on_report(
    store: &Store,
    token: &str,
    state: &str,
    job: Option<u64>,
    active: Option<crate::store::Job>,
) -> Result<(), BoxError> {
    let Some((machine, space)) = store.placed(token).await? else {
        return Ok(());
    };
    let link = store.link_of(space, &machine).await?;
    let link = link.as_deref();
    {
        let mut store = store.lock().unwrap();
        // A print whose file is not known here (started at the printer's screen, or before the
        // hub started): asked once, as Prusa Connect does; JOB_INFO answers.
        if let Some(job) = job
            && store
                .printing_file
                .get(token)
                .is_none_or(|(known, _)| *known != Some(job))
            && store.job_info_asked.get(token) != Some(&job)
        {
            store.job_info_asked.insert(token.to_owned(), job);
            let command = json!({"command": "SEND_JOB_INFO", "kwargs": {"job_id": job}});
            store.queue_connect_command(link, &machine, "Which file it prints", &command, true);
        }
        // OURS, approved by the owner (2026-11-17, audit D6). Buddy opens a file's print preview
        // after a Connect download (`transfers/transfer.cpp:423-430`), and while it shows the
        // file stays read-only. Buddy 6.3.0 gives Connect no command that closes it (audit D6:
        // STOP_PRINT, SET_IDLE and DIALOG_ACTION do not apply; only its own screen or M604 reach
        // the abort, `marlin_server.cpp:1244-1251`). So a download only to the stick
        // closes the preview with M604 (in the preview Buddy only closes it, as its Back
        // button; tried on the MINI; in a print it would stop it). So only as the answer to a
        // report that says IDLE, with nothing else to go before it. Anything but IDLE:
        // someone pressed Print, or it is busy; no preview to close then.
        if store.close_preview.contains(&machine) {
            if state != "IDLE" {
                store.close_preview.remove(&machine);
            } else if store.open_commands(&machine).is_empty() {
                store.close_preview.remove(&machine);
                store.queue_command(link, &machine, "Close the print preview", "M604");
            }
        }
        // No longer asking, or out of its error: what it said goes.
        if !matches!(state, "ATTENTION" | "ERROR") && store.asking.remove(&machine).is_some() {
            store.dialogs.remove(&machine);
            store.changed();
        }
    }
    let Some(job) = active.filter(|j| j.machine_id == machine) else {
        return Ok(());
    };
    let transfer = store.transfer_of_job(space, job.id).await?;
    match (job.state, transfer) {
        (JobState::Queued, None) if matches!(state, "IDLE" | "READY" | "FINISHED" | "STOPPED") => {
            let Some(file) = store.file(space, &job.file_id).await? else {
                store
                    .set_job(space, job.id, JobState::Failed, Some("its file is gone"))
                    .await?;
                return Ok(());
            };
            // Sent here before and still the same file: print that copy, no download.
            if let Some(path) = still_on_stick(store, space, &machine, &file.id, file.size).await? {
                let transfer = store
                    .create_transfer(space, &machine, &file.id, file.size, &path, Some(job.id))
                    .await?;
                store
                    .set_transfer(
                        space,
                        &transfer.iv,
                        "finished",
                        None,
                        Some("on the stick already"),
                    )
                    .await?;
                store.print_from_stick(&transfer, &machine).await?;
                store
                    .set_job(space, job.id, JobState::Printing, None)
                    .await?;
                return Ok(());
            }
            let taken = names_in(store, space, &machine, "/usb").await;
            let path = format!("/usb/{}", stick_name(&file.name, &taken));
            let transfer = store
                .create_transfer(space, &machine, &file.id, file.size, &path, Some(job.id))
                .await?;
            if store.start_transfer(&transfer, &file.name).await?.is_some() {
                store
                    .set_job(space, job.id, JobState::Printing, None)
                    .await?;
            }
        }
        (JobState::Printing | JobState::Paused | JobState::Stopping, Some(t)) => {
            match (t.state.as_str(), state) {
                ("starting", "PRINTING") => {
                    store
                        .set_transfer(space, &t.iv, "printing", None, None)
                        .await?
                }
                ("printing", "PAUSED") if job.state == JobState::Printing => {
                    store.set_job(space, job.id, JobState::Paused, None).await?
                }
                ("printing", "PRINTING") if job.state == JobState::Paused => {
                    store
                        .set_job(space, job.id, JobState::Printing, None)
                        .await?
                }
                ("printing", "FINISHED") => {
                    store.set_transfer(space, &t.iv, "done", None, None).await?;
                    store
                        .set_job(space, job.id, JobState::Finished, None)
                        .await?;
                }
                ("printing", "STOPPED" | "IDLE" | "READY") => {
                    store.set_transfer(space, &t.iv, "done", None, None).await?;
                    store
                        .set_job(space, job.id, JobState::Stopped, None)
                        .await?;
                }
                ("printing" | "starting", "ERROR") => {
                    store.set_transfer(space, &t.iv, "done", None, None).await?;
                    store
                        .set_job(
                            space,
                            job.id,
                            JobState::Failed,
                            Some("the printer reports an error"),
                        )
                        .await?;
                }
                _ => {}
            }
        }
        _ => {}
    }
    Ok(())
}

/// The command for a job's pause, resume or stop on a printer on its built-in Connect.
pub fn job_control(store: &mut Legacy, link: Option<&str>, machine: &str, action: &str) {
    let command = match action {
        "pause" => "PAUSE_PRINT",
        "resume" => "RESUME_PRINT",
        "stop" => "STOP_PRINT",
        _ => return,
    };
    store.queue_connect_command(
        link,
        machine,
        &command.replace('_', " ").to_lowercase(),
        &json!({"command": command}),
        false,
    );
}

// ---- routes ---------------------------------------------------------------------------

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/api/machines/{id}/usb", get(look))
        .route("/machines/{id}/usb/preview", get(preview))
        .route("/machines/{id}/usb/print", post(print))
        .route("/machines/{id}/usb/delete", post(delete))
        .route("/machines/{id}/usb/folder", post(new_folder))
        .route("/machines/{id}/usb/send", post(send))
        .route("/f/{iv}/raw", get(download))
}

#[derive(Deserialize)]
struct PathQuery {
    path: Option<String>,
}

/// A Prusa printer of the space on its built-in Connect, or the answer to give.
async fn connect_printer(
    store: &Store,
    access: &Access,
    machine: &str,
) -> Result<crate::store::Machine, Box<Response>> {
    match store.machine(access.space, machine).await {
        Ok(Some(m)) if m.link == "prusa-connect" => Ok(m),
        Ok(Some(_)) => Err(Box::new(
            (
                StatusCode::CONFLICT,
                "only a Prusa on its built-in Connect shows its USB stick here",
            )
                .into_response(),
        )),
        Ok(None) => Err(Box::new(
            (StatusCode::NOT_FOUND, "no such machine").into_response(),
        )),
        Err(error) => Err(Box::new(failed(error))),
    }
}

/// What the printer last said of a path (a folder's children, a file's details), asking it
/// again when that is old; `asking` while an answer is on its way.
async fn look(
    access: Access,
    State(store): State<Store>,
    Path(machine): Path<String>,
    Query(query): Query<PathQuery>,
) -> Response {
    let Some(path) = usb_path(query.path.as_deref().unwrap_or("/usb")) else {
        return (StatusCode::BAD_REQUEST, "a path on the stick, under /usb").into_response();
    };
    let printer = match connect_printer(&store, &access, &machine).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let (entry, asked) = match store.printer_file(access.space, &machine, &path).await {
        Ok(found) => found,
        Err(error) => return failed(error),
    };
    let folder = entry
        .as_ref()
        .is_none_or(|(d, _)| d["children"].is_array() || d["type"] == "FOLDER");
    let fresh = if folder || path == "/usb" {
        entry
            .as_ref()
            .is_some_and(|(_, at)| now() - at < FOLDER_FRESH_SECONDS)
    } else {
        still_good(&entry, None)
    };
    let waiting = asked.is_some_and(|a| now() - a < ASK_EVERY_SECONDS);
    let mut asking = waiting && !fresh;
    if !fresh && !waiting && printer.online {
        let command = json!({"command": "SEND_FILE_INFO", "kwargs": {"path": path}});
        let queued = store.lock().unwrap().queue_connect_command(
            Some(printer.link.as_str()),
            &machine,
            &format!("Read {path}"),
            &command,
            true,
        );
        if queued.is_some() {
            let _ = store
                .asked_printer_file(access.space, &machine, &path)
                .await;
            asking = true;
        }
    }
    let Some((data, at)) = entry else {
        return Json(json!({"path": path, "data": null, "at": null, "asking": asking, "online": printer.online})).into_response();
    };
    // A folder: its children, each with what is known of it (its picture and details, read
    // ahead by the printer one by one), and how many are still to come.
    if let Some(children) = data["children"].as_array() {
        let mut pending = 0;
        // Children with a delete on its way: shown as such until the printer says it is done.
        let deleting: Vec<String> = store
            .lock()
            .unwrap()
            .open_path_commands(&machine)
            .into_iter()
            .filter(|(_, c)| c.starts_with("DELETE_"))
            .map(|(p, _)| p)
            .collect();
        let mut listed: Vec<Value> = Vec::with_capacity(children.len());
        for c in children {
            let mut c = c.clone();
            let child = format!("{path}/{}", c["name"].as_str().unwrap_or(""));
            if deleting.contains(&child) {
                c["deleting"] = json!(true);
            }
            if c["type"] == "PRINT_FILE" {
                let known = store
                    .printer_file(access.space, &machine, &child)
                    .await
                    .ok()
                    .map(|(e, _)| e)
                    .unwrap_or(None);
                match &known {
                    Some((d, at)) if still_good(&known, c["size"].as_u64()) => {
                        c["details"] = summary(d, *at);
                    }
                    _ => pending += 1,
                }
            }
            listed.push(c);
        }
        return Json(json!({
            "path": path,
            "data": { "type": "FOLDER", "children": listed, "file_count": data["file_count"] },
            "pending": pending,
            "at": at,
            "asking": asking,
            "online": printer.online,
        }))
        .into_response();
    }
    Json(json!({
        "path": path,
        "data": summary(&data, at),
        "at": at,
        "asking": asking,
        "online": printer.online,
    }))
    .into_response()
}

async fn preview(
    access: Access,
    State(store): State<Store>,
    Path(machine): Path<String>,
    Query(query): Query<PathQuery>,
) -> Response {
    let Some(path) = query.path.as_deref().and_then(usb_path) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if let Err(r) = connect_printer(&store, &access, &machine).await {
        return *r;
    }
    let png = store
        .printer_file(access.space, &machine, &path)
        .await
        .ok()
        .and_then(|(entry, _)| entry)
        .and_then(|(data, _)| data["preview"].as_str().map(str::to_owned))
        .and_then(|b64| {
            base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .ok()
        })
        .filter(|png| png.starts_with(b"\x89PNG"));
    match png {
        Some(png) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "max-age=60"),
            ],
            png,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct PathForm {
    path: String,
    /// For delete: a folder (and what is in it).
    folder: Option<String>,
    /// For a new folder: its name, in `path`.
    name: Option<String>,
}

/// Sends one of Connect's commands about a path, for someone.
async fn path_command(
    store: &Store,
    access: &Access,
    machine: &str,
    command: &str,
    label: String,
    path: &str,
) -> Response {
    let printer = match connect_printer(store, access, machine).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    if !printer.online {
        return (StatusCode::CONFLICT, "the printer is offline").into_response();
    }
    let body = json!({"command": command, "kwargs": {"path": path}});
    let queued = store.lock().unwrap().queue_connect_command(
        Some(printer.link.as_str()),
        machine,
        &label,
        &body,
        false,
    );
    match queued {
        Some(_) => {
            let _ = store.forget_printer_path(access.space, machine, path).await;
            StatusCode::NO_CONTENT.into_response()
        }
        None => (StatusCode::CONFLICT, "the printer takes no commands").into_response(),
    }
}

async fn print(
    access: Access,
    State(store): State<Store>,
    Path(machine): Path<String>,
    Form(form): Form<PathForm>,
) -> Response {
    let Some(path) = usb_path(&form.path).filter(|p| p != "/usb") else {
        return (StatusCode::BAD_REQUEST, "a file on the stick").into_response();
    };
    {
        let ppe_checks = store.ppe_rule(access.space, &machine).await;
        let active = match store.active_job(access.space, &machine).await {
            Ok(active) => active,
            Err(error) => return failed(error),
        };
        let found = store.machine(access.space, &machine).await;
        let s = store.lock().unwrap();
        if let Err(refused) = crate::safety::check_ppe(&s, &ppe_checks, &access, &machine) {
            return *refused;
        }
        if let Ok(Some(m)) = found {
            let state = m.telemetry.as_ref().map(|t| t.state);
            if active.is_some() || matches!(state, Some(maker_connect_protocol::MachineState::Busy))
            {
                return (StatusCode::CONFLICT, "it is busy: one print at a time").into_response();
            }
        }
    }
    let name = path.rsplit('/').next().unwrap_or("").to_owned();
    let response = path_command(
        &store,
        &access,
        &machine,
        "START_PRINT",
        format!("Print {name} from the stick"),
        &path,
    )
    .await;
    if response.status().is_success() {
        // Who started it, for as long as it runs (Now's "Ends next").
        let _ = store
            .set_usb_print_by(access.space, &machine, &access.actor)
            .await;
    }
    response
}

async fn delete(
    access: Access,
    State(store): State<Store>,
    Path(machine): Path<String>,
    Form(form): Form<PathForm>,
) -> Response {
    let Some(path) = usb_path(&form.path).filter(|p| p != "/usb") else {
        return (StatusCode::BAD_REQUEST, "a file or folder on the stick").into_response();
    };
    let folder = form.folder.is_some();
    let name = path.rsplit('/').next().unwrap_or("").to_owned();
    // A file still arriving is refused by the printer ("File is being transferred"):
    // said here, before it is asked. Buddy lists such a file as read_only.
    if !folder
        && let Ok(Some(child)) = store.listed_child(access.space, &machine, &path).await
        && child["read_only"].as_bool() == Some(true)
    {
        return (
            StatusCode::CONFLICT,
            "it is still arriving on the stick: delete it once it is there",
        )
            .into_response();
    }
    if store
        .lock()
        .unwrap()
        .open_path_commands(&machine)
        .iter()
        .any(|(p, c)| p == &path && c.starts_with("DELETE_"))
    {
        return (StatusCode::CONFLICT, "it is being deleted already").into_response();
    }
    let (command, label) = if folder {
        ("DELETE_FOLDER", format!("Delete the folder {name}"))
    } else {
        ("DELETE_FILE", format!("Delete {name}"))
    };
    path_command(&store, &access, &machine, command, label, &path).await
}

async fn new_folder(
    access: Access,
    State(store): State<Store>,
    Path(machine): Path<String>,
    Form(form): Form<PathForm>,
) -> Response {
    let name = form.name.as_deref().unwrap_or("").trim().to_owned();
    let plain = !name.is_empty()
        && name.len() <= 60
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " -_".contains(c));
    let Some(folder) = usb_path(&form.path) else {
        return (StatusCode::BAD_REQUEST, "a folder on the stick").into_response();
    };
    if !plain {
        return (
            StatusCode::BAD_REQUEST,
            "a folder name of letters, digits, spaces, - and _",
        )
            .into_response();
    }
    let path = format!("{folder}/{name}");
    path_command(
        &store,
        &access,
        &machine,
        "CREATE_FOLDER",
        format!("New folder {name}"),
        &path,
    )
    .await
}

#[derive(Deserialize)]
struct SendForm {
    /// A file of the space (its id).
    file: String,
    /// The folder on the stick.
    folder: Option<String>,
}

/// A file of the space onto the printer's stick (not printed: that is Print on the Files page).
async fn send(
    access: Access,
    State(store): State<Store>,
    Path(machine): Path<String>,
    Form(form): Form<SendForm>,
) -> Response {
    let printer = match connect_printer(&store, &access, &machine).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    if !printer.online {
        return (StatusCode::CONFLICT, "the printer is offline").into_response();
    }
    let Some(folder) = usb_path(form.folder.as_deref().unwrap_or("/usb")) else {
        return (StatusCode::BAD_REQUEST, "a folder on the stick").into_response();
    };
    let file = match store.file(access.space, &form.file).await {
        Ok(Some(f)) => f,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such file").into_response(),
        Err(error) => return failed(error),
    };
    if !file.name.to_ascii_lowercase().ends_with("code") {
        return (
            StatusCode::BAD_REQUEST,
            "only G-code (.gcode, .bgcode) goes on the stick",
        )
            .into_response();
    }
    // There already (sent by us, the same file): no second copy. Unless the printer's
    // listing of the folder no longer has it (deleted since, on the printer, by hand or by
    // us: the printer names it by its short name then, so the transfer is not matched).
    match still_on_stick(&store, access.space, &machine, &file.id, file.size).await {
        Ok(Some(path)) if path.rsplit_once('/').map(|(f, _)| f) == Some(folder.as_str()) => {
            return Json(json!({ "path": path, "already": true })).into_response();
        }
        Ok(_) => {}
        Err(error) => return failed(error),
    }
    let taken = names_in(&store, access.space, &machine, &folder).await;
    let path = format!("{folder}/{}", stick_name(&file.name, &taken));
    let sent = match store
        .create_transfer(access.space, &machine, &file.id, file.size, &path, None)
        .await
    {
        Ok(t) => store.start_transfer(&t, &file.name).await,
        Err(error) => Err(error.into()),
    };
    match sent {
        Ok(Some(_)) => Json(json!({ "path": path })).into_response(),
        Ok(None) => (StatusCode::CONFLICT, "the printer takes no commands").into_response(),
        Err(error) => failed(error),
    }
}

/// The printer fetches a transfer's file: encrypted (AES-128-CTR), padded to whole blocks,
/// from where it asks (`Range: bytes=N-`).
async fn download(
    State(store): State<Store>,
    Path(iv): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (transfer, path) = match store.transfer(&iv.to_ascii_lowercase()).await {
        Ok(Some(t)) => {
            let path = store.file_path(&t.file);
            (t, path)
        }
        Ok(None) => return (StatusCode::NOT_FOUND, "no such transfer").into_response(),
        Err(error) => return failed(error),
    };
    let (Some(key), Some(nonce)) = (unhex(&transfer.key), unhex(&transfer.iv)) else {
        return failed("a transfer without its key");
    };
    let padded = transfer.size.div_ceil(16) * 16;
    let start = headers
        .get(header::RANGE)
        .and_then(|r| r.to_str().ok())
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.split('-').next())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if start > padded {
        return StatusCode::RANGE_NOT_SATISFIABLE.into_response();
    }
    let mut file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(error) => return failed(error),
    };
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    if start > 0
        && file
            .seek(std::io::SeekFrom::Start(start.min(transfer.size)))
            .await
            .is_err()
    {
        return failed("could not seek in the file");
    }
    let mut cipher = Cipher::new(key.as_slice().into(), nonce.as_slice().into());
    cipher.seek(start);
    println!("the printer fetches {} from byte {start}", transfer.path);
    let size = transfer.size;
    // Read, pad with zeros to whole blocks, encrypt, and send, a chunk at a time.
    let stream = futures_util::stream::unfold(
        (file, cipher, start),
        move |(mut file, mut cipher, at)| async move {
            if at >= padded {
                return None;
            }
            let want = (64 * 1024).min(padded - at) as usize;
            let mut chunk = vec![0u8; want];
            let real = size.saturating_sub(at).min(want as u64) as usize;
            let mut read = 0;
            while read < real {
                match file.read(&mut chunk[read..real]).await {
                    Ok(0) => break,
                    Ok(n) => read += n,
                    Err(e) => return Some((Err(e), (file, cipher, padded))),
                }
            }
            cipher.apply_keystream(&mut chunk);
            Some((
                Ok::<_, std::io::Error>(chunk),
                (file, cipher, at + want as u64),
            ))
        },
    );
    let status = if start > 0 {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let mut response = (status, Body::from_stream(stream)).into_response();
    let h = response.headers_mut();
    h.insert(header::CONTENT_LENGTH, (padded - start).into());
    h.insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    // The printer takes nothing without it: it says how the body is encrypted (Buddy's
    // transfers/download.cpp aborts the download otherwise, as TRANSFER_ABORTED).
    h.insert("content-encryption-mode", "AES-CTR".parse().unwrap());
    if start > 0
        && let Ok(v) = format!("bytes {start}-{}/{padded}", padded - 1).parse()
    {
        h.insert(header::CONTENT_RANGE, v);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::KeyIvInit;

    #[test]
    fn paths_stay_on_the_stick() {
        assert_eq!(usb_path("/usb/").as_deref(), Some("/usb"));
        assert_eq!(
            usb_path("/usb/PARTS/A.GCO").as_deref(),
            Some("/usb/PARTS/A.GCO")
        );
        for bad in [
            "/",
            "/etc/passwd",
            "/usb/../x",
            "usb/a",
            "/usbx",
            "/usb/a\\b",
            "/usb/\"x",
        ] {
            assert!(usb_path(bad).is_none(), "{bad}");
        }
        assert_eq!(parent("/usb/a/b.gcode"), "/usb/a");
        assert_eq!(parent("/usb/b.gcode"), "/usb");
    }

    #[test]
    fn names_on_the_stick_are_plain_and_free() {
        assert_eq!(
            stick_name("Benchy 0.2mm PLA.gcode", &[]),
            "Benchy_0_2mm_PLA.gcode"
        );
        let taken = vec!["cube.bgcode".to_owned(), "CUBE-2.BGCODE".to_owned()];
        assert_eq!(stick_name("cube.bgcode", &taken), "cube-3.bgcode");
        assert_eq!(stick_name("ü.gcode", &[]), "file.gcode");
    }

    #[test]
    fn the_key_stays_out_of_the_log() {
        let body = json!({"command": "START_ENCRYPTED_DOWNLOAD", "kwargs": {"key": "00112233", "path": "/usb/a.gcode"}}).to_string();
        let logged = for_the_log(&body);
        assert!(
            !logged.contains("00112233") && logged.contains("/usb/a.gcode"),
            "{logged}"
        );
        assert_eq!(for_the_log("G28\nG1 X10"), "G28 | G1 X10");
    }

    /// As the firmware decrypts: AES-128-CTR, the counter in the last 4 bytes set to the
    /// block number, from any block where it goes on.
    #[test]
    fn the_cipher_is_the_printers() {
        use aes::cipher::{BlockEncrypt, KeyInit};
        let key = [7u8; 16];
        let mut iv = [9u8; 16];
        iv[12..].copy_from_slice(&[0, 0, 0, 0]);
        let plain: Vec<u8> = (0..100u8).collect();
        let mut sealed = plain.clone();
        Cipher::new(&key.into(), &iv.into()).apply_keystream(&mut sealed);
        // Block 2 (bytes 32..48) by hand: the counter block is iv[..12] and 2, big-endian.
        let mut block = iv;
        block[12..].copy_from_slice(&2u32.to_be_bytes());
        let mut stream = aes::cipher::generic_array::GenericArray::from(block);
        aes::Aes128::new(&key.into()).encrypt_block(&mut stream);
        let by_hand: Vec<u8> = plain[32..48]
            .iter()
            .zip(stream.iter())
            .map(|(p, k)| p ^ k)
            .collect();
        assert_eq!(&sealed[32..48], by_hand.as_slice());
        // Going on from byte 48 gives the same bytes as from the start.
        let mut rest = plain[48..].to_vec();
        let mut cipher = Cipher::new(&key.into(), &iv.into());
        cipher.seek(48u64);
        cipher.apply_keystream(&mut rest);
        assert_eq!(&sealed[48..], rest.as_slice());
    }
}

#[cfg(test)]
mod flow {
    use super::*;
    use crate::store::{Legacy, now};
    use axum::http::Request;
    use http_body_util::BodyExt;

    use tower::ServiceExt;

    async fn call(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
        let response = app.clone().oneshot(request).await.unwrap();
        let (status, headers) = (response.status(), response.headers().clone());
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, headers, body)
    }

    /// Deleting on a MINI's stick as Buddy 6.3 answers it: not while a file is still
    /// arriving, shown as being deleted until FILE_CHANGED (which settles the command, as
    /// Buddy sends no FINISHED for it), then gone from the listing; a folder is read once
    /// however often the page asks meanwhile.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn deleting_on_the_stick(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (space, mini, token) = store
            .add_test_machine("prusa/mini", Some("prusa-connect"))
            .await;
        crate::auth::tester_may_operate(&store, &mini).await;
        let report = || {
            Request::post("/p/telemetry")
                .header("Token", &token)
                .body(Body::from(r#"{"state":"IDLE"}"#))
                .unwrap()
        };
        let event = |body: Value| {
            Request::post("/p/events")
                .header("Token", &token)
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let look = |app: Router| {
            let mini = mini.clone();
            async move {
                let (_, _, body) = call(
                    &app,
                    Request::get(format!("/api/machines/{mini}/usb?path=/usb"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await;
                serde_json::from_slice::<Value>(&body).unwrap()
            }
        };
        let delete = |path: &str| {
            Request::post(format!("/machines/{mini}/usb/delete"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .body(Body::from(format!("path={path}")))
                .unwrap()
        };
        let reads = |s: &Legacy| {
            s.open_path_commands(&mini)
                .iter()
                .filter(|(p, c)| p == "/usb" && c == "SEND_FILE_INFO")
                .count()
        };
        call(&app, report()).await;

        // Asked for the folder twice before it answers: one read queued.
        look(app.clone()).await;
        sqlx::query("UPDATE printer_files SET asked_at = at_unix(0)")
            .execute(&pool)
            .await
            .unwrap();
        look(app.clone()).await;
        assert_eq!(reads(&store.lock().unwrap()), 1, "read once");
        let (_, headers, _) = call(&app, report()).await;
        let read: u64 = headers["command-id"].to_str().unwrap().parse().unwrap();
        call(&app, event(json!({"event": "FILE_INFO", "command_id": read, "state": "IDLE", "data": {
            "path": "/usb", "type": "FOLDER", "file_count": 2, "children": [
                {"name": "BENCHY~1.BGC", "display_name": "Benchy.bgcode", "size": 812345, "m_timestamp": 0, "read_only": false, "type": "PRINT_FILE"},
                {"name": "BOX~1.BGC", "display_name": "Box.bgcode", "size": 1000, "m_timestamp": 0, "read_only": true, "type": "PRINT_FILE"}
            ]}})))
        .await;

        // Still arriving: refused here, before the printer is asked.
        let (status, _, body) = call(&app, delete("/usb/BOX~1.BGC")).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(String::from_utf8_lossy(&body).contains("still arriving"));

        // Deleted: shown as being deleted, and not asked twice.
        assert_eq!(
            call(&app, delete("/usb/BENCHY~1.BGC")).await.0,
            StatusCode::NO_CONTENT
        );
        let listed = look(app.clone()).await;
        assert_eq!(listed["data"]["children"][0]["deleting"], true);
        assert_eq!(
            call(&app, delete("/usb/BENCHY~1.BGC")).await.0,
            StatusCode::CONFLICT
        );
        let (_, headers, body) = call(&app, report()).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"command": "DELETE_FILE", "kwargs": {"path": "/usb/BENCHY~1.BGC"}})
        );
        let id: u64 = headers["command-id"].to_str().unwrap().parse().unwrap();

        // Buddy answers with FILE_CHANGED only: the command is done and the file is gone.
        call(
            &app,
            event(
                json!({"event": "FILE_CHANGED", "command_id": id, "state": "IDLE",
            "data": {"free_space": 1, "old_path": "/usb/BENCHY~1.BGC", "file": {}}}),
            ),
        )
        .await;
        {
            let m = store.machine(space, &mini).await.unwrap().unwrap();
            let done = m.commands.iter().find(|c| c.id == id as u32).unwrap();
            assert_eq!(done.state, crate::store::CommandState::Finished);
        }
        let listed = look(app.clone()).await;
        let names: Vec<&str> = listed["data"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c["name"].as_str())
            .collect();
        assert_eq!(names, vec!["BOX~1.BGC"]);
        // And the next command goes at once, not two minutes later.
        let (status, headers, _) = call(&app, report()).await;
        assert_eq!(status, StatusCode::OK, "the folder read follows");
        let read: u64 = headers["command-id"].to_str().unwrap().parse().unwrap();
        call(
            &app,
            event(
                json!({"event": "FILE_INFO", "command_id": read, "state": "IDLE", "data": {
            "path": "/usb", "type": "FOLDER", "file_count": 0, "children": []}}),
            ),
        )
        .await;

        // A command the printer is still carrying out (its id in the telemetry) is taken on,
        // not given up on, and nothing else is sent meanwhile.
        let link = store.link_of(space, &mini).await.unwrap();
        let busy = store
            .lock()
            .unwrap()
            .queue_connect_command(
                link.as_deref(),
                &mini,
                "Home",
                &json!({"command": "SEND_INFO"}),
                false,
            )
            .unwrap();
        let (_, headers, _) = call(&app, report()).await;
        assert_eq!(headers["command-id"].to_str().unwrap(), busy.to_string());
        let running = Request::post("/p/telemetry")
            .header("Token", &token)
            .body(Body::from(format!(
                r#"{{"state":"BUSY","command_id":{busy}}}"#
            )))
            .unwrap();
        assert_eq!(call(&app, running).await.0, StatusCode::NO_CONTENT);
        let m = store.machine(space, &mini).await.unwrap().unwrap();
        let c = m.commands.iter().find(|c| c.id == busy).unwrap();
        assert_eq!(c.state, crate::store::CommandState::Accepted);
    }

    /// A MINI, as it talks: its stick looked through (with a file's picture), a file
    /// printed from it, and a file of the space sent to it (encrypted, fetched in two
    /// goes), printed, and its job followed to the end.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_mini_and_its_usb_stick(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        // A MINI registered and in the space.
        let (space, mini, token) = store
            .add_test_machine("prusa/mini", Some("prusa-connect"))
            .await;
        crate::auth::tester_may_operate(&store, &mini).await;
        let report = |state: &str| {
            Request::post("/p/telemetry")
                .header("Token", &token)
                .body(Body::from(format!(r#"{{"state":"{state}"}}"#)))
                .unwrap()
        };
        let event = |body: Value| {
            Request::post("/p/events")
                .header("Token", &token)
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let get = |path: String| Request::get(path).body(Body::empty()).unwrap();
        let post = |path: String, form: String| {
            Request::post(path)
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .body(Body::from(form))
                .unwrap()
        };
        call(&app, report("IDLE")).await;

        // Looking at the stick asks the printer; its answer comes as the next telemetry's.
        let (_, _, body) = call(&app, get(format!("/api/machines/{mini}/usb?path=/usb"))).await;
        let look: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            (look["asking"].as_bool(), look["data"].is_null()),
            (Some(true), true)
        );
        let (status, headers, body) = call(&app, report("IDLE")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["content-type"], "application/json");
        let command: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            command,
            json!({"command": "SEND_FILE_INFO", "kwargs": {"path": "/usb"}})
        );
        let id: u64 = headers["command-id"].to_str().unwrap().parse().unwrap();
        // Not again while the answer is on its way.
        call(&app, get(format!("/api/machines/{mini}/usb?path=/usb"))).await;
        assert_eq!(call(&app, report("IDLE")).await.0, StatusCode::NO_CONTENT);
        let listing = json!({"event": "FILE_INFO", "command_id": id, "state": "IDLE", "data": {
        "path": "/usb", "type": "FOLDER", "read_only": false, "display_name": "usb", "file_count": 2,
        "children": [
            {"name": "BENCHY~1.BGC", "display_name": "Benchy 0.2mm PLA.bgcode", "size": 812345, "m_timestamp": 1730000000, "read_only": false, "type": "PRINT_FILE"},
            {"name": "PARTS", "display_name": "parts", "size": 0, "m_timestamp": 0, "read_only": false, "type": "FOLDER"}
        ]}});
        assert_eq!(call(&app, event(listing)).await.0, StatusCode::NO_CONTENT);
        // Its printable file is read ahead, in the background (not listed with someone's commands).
        {
            let m = store.machine(space, &mini).await.unwrap().unwrap();
            assert!(
                m.commands.iter().all(|c| !c.label.contains("BENCHY")),
                "background: not listed"
            );
            let (_, asked) = store
                .printer_file(space, &mini, "/usb/BENCHY~1.BGC")
                .await
                .unwrap();
            assert!(asked.is_some(), "asked ahead");
        }
        let (_, _, body) = call(&app, get(format!("/api/machines/{mini}/usb?path=/usb"))).await;
        let look: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            look["data"]["children"][0]["display_name"],
            "Benchy 0.2mm PLA.bgcode"
        );
        assert_eq!(look["asking"], false, "fresh: not asked again");
        // A file's picture.
        let png = {
            let mut l = maker_connect_label::Label::blank(20, 20);
            l.set(1, 1, true);
            l.to_png()
        };
        let file_info = json!({"event": "FILE_INFO", "state": "IDLE", "data": {
            "path": "/usb/BENCHY~1.BGC", "preview": base64::engine::general_purpose::STANDARD.encode(&png),
            "filament_type": "PLA", "estimated printing time (normal mode)": "1h 12m 3s",
            "size": 812345, "type": "PRINT_FILE", "display_name": "Benchy 0.2mm PLA.bgcode"}});
        call(&app, event(file_info)).await;
        let (_, _, body) = call(
            &app,
            get(format!("/api/machines/{mini}/usb?path=/usb/BENCHY~1.BGC")),
        )
        .await;
        let look: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            (
                look["data"]["picture"].as_bool(),
                look["data"]["material"].as_str()
            ),
            (Some(true), Some("PLA"))
        );
        assert_eq!(look["data"]["time"], "1h 12m 3s");
        // The folder's listing carries it too, and nothing is still to come.
        let (_, _, body) = call(&app, get(format!("/api/machines/{mini}/usb?path=/usb"))).await;
        let folder: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(folder["data"]["children"][0]["details"]["picture"], true);
        assert_eq!(folder["pending"], 0);
        assert!(
            look["data"].get("preview").is_none(),
            "the picture comes on its own"
        );
        let (status, _, picture) = call(
            &app,
            get(format!(
                "/machines/{mini}/usb/preview?path=/usb/BENCHY~1.BGC"
            )),
        )
        .await;
        assert_eq!((status, &picture[..4]), (StatusCode::OK, &b"\x89PNG"[..]));

        // Printed from the stick.
        assert_eq!(
            call(
                &app,
                post(
                    format!("/machines/{mini}/usb/print"),
                    "path=/usb/BENCHY~1.BGC".into()
                )
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        let (_, headers, body) = call(&app, report("IDLE")).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"command": "START_PRINT", "kwargs": {"path": "/usb/BENCHY~1.BGC"}})
        );
        // (It says no: the stick was taken out meanwhile. One command at a time, so it answers.)
        let print_id: u64 = headers["command-id"].to_str().unwrap().parse().unwrap();
        call(&app, event(json!({"event": "REJECTED", "command_id": print_id, "reason": "File not found", "state": "IDLE"}))).await;
        // Nothing outside the stick.
        assert_eq!(
            call(
                &app,
                post(
                    format!("/machines/{mini}/usb/delete"),
                    "path=/usb/../etc".into()
                )
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );

        // A file of the space, printed on it: to the stick first, encrypted.
        let gcode: Vec<u8> = (0..1000u32)
            .flat_map(|i| format!("G1 X{} Y{}\n", i % 180, i % 170).into_bytes())
            .collect();
        let (status, _, body) = call(
            &app,
            Request::post("/api/files?name=Big%20part.gcode")
                .body(Body::from(gcode.clone()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let file = serde_json::from_slice::<Value>(&body).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let (status, _, body) = call(
            &app,
            post(format!("/files/{file}/print"), format!("machine={mini}")),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&body)
        );
        let (status, headers, body) = call(&app, report("IDLE")).await;
        assert_eq!(status, StatusCode::OK);
        let download: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(download["command"], "START_ENCRYPTED_DOWNLOAD");
        let kw = &download["kwargs"];
        assert_eq!(
            (kw["path"].as_str(), kw["orig_size"].as_u64()),
            (Some("/usb/Big_part.gcode"), Some(gcode.len() as u64))
        );
        let download_id: u64 = headers["command-id"].to_str().unwrap().parse().unwrap();
        let (key, iv) = (
            unhex(kw["key"].as_str().unwrap()).unwrap(),
            unhex(kw["iv"].as_str().unwrap()).unwrap(),
        );
        assert_eq!(&iv[12..], &[0, 0, 0, 0], "the counter starts at block 0");
        // Fetched as the printer does, with no token: once from the start, once going on.
        let fetch = |from: u64| {
            Request::get(format!("/f/{}/raw", kw["iv"].as_str().unwrap()))
                .header("Range", format!("bytes={from}-"))
                .header("Content-Encryption-Mode", "AES-CTR")
                .body(Body::empty())
                .unwrap()
        };
        let (status, headers, first) = call(&app, fetch(0)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers["content-encryption-mode"], "AES-CTR",
            "the printer aborts without it"
        );
        let padded = gcode.len().div_ceil(16) * 16;
        assert_eq!(
            (
                first.len(),
                headers["content-length"]
                    .to_str()
                    .unwrap()
                    .parse::<usize>()
                    .unwrap()
            ),
            (padded, padded)
        );
        let (status, _, rest) = call(&app, fetch(4096)).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            rest.as_slice(),
            &first[4096..],
            "going on gives the same bytes"
        );
        let mut plain = first.clone();
        Cipher::new(key.as_slice().into(), iv.as_slice().into()).apply_keystream(&mut plain);
        assert_eq!(
            &plain[..gcode.len()],
            gcode.as_slice(),
            "the printer decrypts it to the file"
        );
        assert_eq!(
            call(
                &app,
                Request::get("/f/00112233445566778899aabb00000000/raw")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        // On the stick: printed, and the job follows the printer.
        call(&app, event(json!({"event": "TRANSFER_FINISHED", "state": "IDLE", "data": {"start_cmd_id": download_id}}))).await;
        let (_, first_start, body) = call(&app, report("IDLE")).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"command": "START_PRINT", "kwargs": {"path": "/usb/Big_part.gcode"}})
        );
        let job = || async { store.machine(space, &mini).await.unwrap().unwrap().job };
        let job_id = job().await.unwrap().id;
        assert_eq!(job().await.unwrap().state, JobState::Printing);
        call(&app, report("PRINTING")).await;
        assert_eq!(job().await.unwrap().state, JobState::Printing);
        call(&app, report("FINISHED")).await;
        assert!(job().await.is_none(), "ended");
        let ended = store.job(space, job_id).await.unwrap().unwrap();
        assert_eq!(ended.state, JobState::Finished);

        // The printer said it started that print (as a real one does).
        let first_start: u64 = first_start["command-id"].to_str().unwrap().parse().unwrap();
        call(
            &app,
            event(json!({"event": "FINISHED", "command_id": first_start, "state": "IDLE"})),
        )
        .await;
        // Printed again: it is on the stick already, so no download, straight to printing.
        call(&app, post(format!("/machines/{mini}/taken"), String::new())).await;
        let (status, _, body) = call(
            &app,
            post(format!("/files/{file}/print"), format!("machine={mini}")),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&body)
        );
        // The command goes out with the printer's next report.
        let mut answer = call(&app, report("IDLE")).await;
        if answer.2.is_empty() {
            answer = call(&app, report("IDLE")).await;
        }
        let (_, headers, body) = answer;
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"command": "START_PRINT", "kwargs": {"path": "/usb/Big_part.gcode"}}),
            "the copy on the stick"
        );
        let start_id: u64 = headers["command-id"].to_str().unwrap().parse().unwrap();
        // A file by that name is on the stick: Buddy says FILE_EXISTS (planner.cpp:793), and
        // it is sent again, under a free name.
        call(
            &app,
            event(
                json!({"event": "REJECTED", "command_id": start_id, "reason": "File already exists",
            "machine_reason": "FILE_EXISTS", "state": "IDLE"}),
            ),
        )
        .await;
        let mut answer = call(&app, report("IDLE")).await;
        if answer.2.is_empty() {
            answer = call(&app, report("IDLE")).await;
        }
        let again: Value = serde_json::from_slice(&answer.2).unwrap();
        assert_eq!(again["command"], "START_ENCRYPTED_DOWNLOAD");
        assert_eq!(again["kwargs"]["path"], "/usb/Big_part-2.gcode");
        assert_eq!(
            job().await.unwrap().state,
            JobState::Printing,
            "the job goes on"
        );
        // It asks something on its screen: shown as it says it, until it no longer asks.
        call(&app, event(json!({"event": "STATE_CHANGED", "state": "ATTENTION", "dialog_id": 7, "data": {
            "code": "17630", "title": "Sliced for another printer", "text": "The G-code was sliced for MINIIS.", "buttons": ["Continue", "Abort"]
        }}))).await;
        // Its buttons, pressed from the website: DIALOG_ACTION with the dialog and the name
        // (once the download before has its answer: one command at a time).
        let again_id: u64 = answer.1["command-id"].to_str().unwrap().parse().unwrap();
        call(&app, event(json!({"event": "TRANSFER_INFO", "command_id": again_id, "state": "ATTENTION", "data": {}}))).await;
        let buttons = store.machine(space, &mini).await.unwrap().unwrap().buttons;
        assert_eq!(buttons, ["Continue", "Abort"]);
        let (status, _, _) = call(
            &app,
            post(format!("/machines/{mini}/answer"), "button=1".into()),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, _, body) = call(&app, report("ATTENTION")).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"command": "DIALOG_ACTION", "kwargs": {"dialog_id": 7, "button": "Abort"}})
        );
        let (status, _, _) = call(
            &app,
            post(format!("/machines/{mini}/answer"), "button=5".into()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "no such button");
        let asking = || async { store.machine(space, &mini).await.unwrap().unwrap().asking };
        assert_eq!(
            asking().await,
            Some((
                "Sliced for another printer".to_owned(),
                "The G-code was sliced for MINIIS. (#17630, prusa.io/17630)".to_owned()
            ))
        );
        call(&app, report("PRINTING")).await;
        assert_eq!(asking().await, None, "answered on the printer");
        let _ = now();
    }

    /// A print the MINI runs itself (from its stick, or its screen) is recorded as a job when
    /// it ends, as it says; what is left on its bed then holds a queued job back until it is
    /// taken off. Seen missing on the MINI (2026-11-08): a stopped Benchy was not known to be
    /// on the bed, so a queued job could have started on top of it.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_print_the_mini_runs_itself_holds_the_queue_when_it_ends(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (space, mini, token) = store
            .add_test_machine("prusa/mini", Some("prusa-connect"))
            .await;
        crate::auth::tester_may_operate(&store, &mini).await;
        let report = |body: &str| {
            Request::post("/p/telemetry")
                .header("Token", &token)
                .body(Body::from(body.to_owned()))
                .unwrap()
        };
        let jobs = async || -> (i64, Option<String>, Option<String>) {
            sqlx::query_as(
                "SELECT COUNT(*), MAX(state), MAX(file_name) FROM jobs WHERE machine_id = $1",
            )
            .bind(&mini)
            .fetch_one(store.pg().admin_in_tests())
            .await
            .unwrap()
        };
        // What `files::fit` holds a job back by: a finished print on the bed, or the last job
        // stopped or failed and not said to be cleared.
        let held = || async {
            let machine = store.machine(space, &mini).await.unwrap().unwrap();
            (machine.on_bed.is_some(), machine.queue_held.clone())
        };
        call(&app, report(r#"{"state":"IDLE"}"#)).await;
        // Its own print, its file said by JOB_INFO: no job while it runs.
        store
            .lock()
            .unwrap()
            .printing_file
            .insert(token.clone(), (Some(29), "Benchy.bgcode".into()));
        call(
            &app,
            report(r#"{"state":"PRINTING","job_id":29,"progress":40,"time_printing":600}"#),
        )
        .await;
        call(&app, report(r#"{"state":"ATTENTION","job_id":29}"#)).await;
        assert_eq!(jobs().await.0, 0, "a question on its screen is no end");
        call(
            &app,
            report(r#"{"state":"PRINTING","job_id":29,"progress":45,"time_printing":700}"#),
        )
        .await;
        // Stopped on the printer: a stopped job, and the queue waits until the bed is clear.
        call(&app, report(r#"{"state":"STOPPED"}"#)).await;
        assert_eq!(
            jobs().await,
            (1, Some("stopped".into()), Some("Benchy.bgcode".into()))
        );
        assert_eq!(held().await, (false, Some("stopped".into())));
        call(&app, report(r#"{"state":"IDLE"}"#)).await;
        assert_eq!(jobs().await.0, 1, "recorded once");
        store.print_taken(space, &mini).await.unwrap();
        assert_eq!(held().await, (false, None), "taken off: free again");
        // Finished on the printer: on its bed until taken.
        call(
            &app,
            report(r#"{"state":"PRINTING","job_id":30,"progress":99,"time_printing":60}"#),
        )
        .await;
        call(&app, report(r#"{"state":"FINISHED"}"#)).await;
        assert_eq!(jobs().await.0, 2);
        assert!(held().await.0, "a finished print on its bed");
    }

    /// Seen on the MINI (2026-11-08): a download only to the stick opens the file's print
    /// preview on the printer's screen, which keeps it read-only. The hub closes it with
    /// M604 (tried on the MINI), so nobody has to go to the printer; but only answering a
    /// report that says IDLE with nothing before it: in a print M604 would stop it.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_download_only_to_the_stick_closes_its_preview(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (_, mini, token) = store
            .add_test_machine("prusa/mini", Some("prusa-connect"))
            .await;
        crate::auth::tester_may_operate(&store, &mini).await;
        let report = |state: &str| {
            Request::post("/p/telemetry")
                .header("Token", &token)
                .body(Body::from(format!(r#"{{"state":"{state}"}}"#)))
                .unwrap()
        };
        let event = |body: Value| {
            Request::post("/p/events")
                .header("Token", &token)
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let form = |path: String, form: String| {
            Request::post(path)
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .body(Body::from(form))
                .unwrap()
        };
        // What the printer gets with a report (answered at once, as it does), as text.
        let next = async |state: &str| {
            let (_, headers, body) = call(&app, report(state)).await;
            if let Some(id) = headers.get("command-id") {
                let id: u64 = id.to_str().unwrap().parse().unwrap();
                call(
                    &app,
                    event(json!({"event": "FINISHED", "command_id": id, "state": state})),
                )
                .await;
            }
            String::from_utf8(body).unwrap()
        };
        // Sends a file only to the stick and lets the printer fetch it to the end.
        let send = async |name: &str| {
            let (status, _, body) = call(
                &app,
                Request::post(format!("/api/files?name={name}"))
                    .body(Body::from(format!("G28 ; {name}\n")))
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
            let file = serde_json::from_slice::<Value>(&body).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let (status, _, body) = call(
                &app,
                form(
                    format!("/machines/{mini}/usb/send"),
                    format!("file={file}&folder=/usb"),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
            let (_, headers, body) = call(&app, report("IDLE")).await;
            let download: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(download["command"], "START_ENCRYPTED_DOWNLOAD");
            let id: u64 = headers["command-id"].to_str().unwrap().parse().unwrap();
            call(&app, event(json!({"event": "TRANSFER_INFO", "command_id": id, "state": "IDLE", "data": {}}))).await;
            call(&app, event(json!({"event": "TRANSFER_FINISHED", "state": "IDLE", "data": {"start_cmd_id": id}}))).await;
        };
        call(&app, report("IDLE")).await;

        // Done and idle: the preview is closed.
        send("Benchy.bgcode").await;
        assert_eq!(next("IDLE").await, "M604");
        assert_eq!(next("IDLE").await, "", "once");

        // Someone pressed Print on the printer meanwhile: no M604, not even once idle again.
        send("Cube.bgcode").await;
        assert_eq!(next("PRINTING").await, "");
        assert_eq!(next("IDLE").await, "");

        // A command of someone's waits: it goes first, then the M604 (never one before it).
        send("Box.bgcode").await;
        let space = store.default_space().await.unwrap().0;
        let link = store.link_of(space, &mini).await.unwrap();
        let queued =
            store
                .lock()
                .unwrap()
                .queue_command(link.as_deref(), &mini, "Fan", "M106 S255");
        assert!(queued.is_some());
        assert_eq!(next("IDLE").await, "M106 S255");
        assert_eq!(next("IDLE").await, "M604");

        // Nothing sent: nothing to close.
        assert_eq!(next("IDLE").await, "");

        // A print the printer runs itself (started from its stick): its file asked once
        // (SEND_JOB_INFO), and Stop reaches it.
        let printing = || {
            Request::post("/p/telemetry")
                .header("Token", &token)
                .body(Body::from(
                    r#"{"state":"PRINTING","job_id":29,"progress":5,"time_printing":752,"time_remaining":3540}"#,
                ))
                .unwrap()
        };
        let (_, headers, body) = call(&app, printing()).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"command": "SEND_JOB_INFO", "kwargs": {"job_id": 29}})
        );
        let asked: u64 = headers["command-id"].to_str().unwrap().parse().unwrap();
        call(&app, event(json!({"event": "JOB_INFO", "command_id": asked, "job_id": 29, "state": "PRINTING", "data": {
            "display_name": "Benchy.bgcode", "path": "/usb/BENCHY~1.BGC"
        }}))).await;
        call(
            &app,
            event(json!({"event": "FINISHED", "command_id": asked, "state": "PRINTING"})),
        )
        .await;
        let (_, _, body) = call(&app, printing()).await;
        assert!(
            body.is_empty(),
            "asked once: {}",
            String::from_utf8_lossy(&body)
        );
        {
            let space = store.default_space().await.unwrap().0;
            let m = store.machine(space, &mini).await.unwrap().unwrap();
            assert_eq!(m.telemetry.unwrap().job.unwrap().file, "Benchy.bgcode");
        }
        let (status, _, body) = call(
            &app,
            form(format!("/machines/{mini}/job"), "action=stop".into()),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&body)
        );
        let (_, _, body) = call(&app, report("PRINTING")).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"command": "STOP_PRINT"})
        );
    }

    /// Seen on the MINI (2026-11-08): a Benchy sent by us, then deleted (the printer names
    /// it by its short name, `3DBENC~1.BGC`); Send here kept saying it was there already.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_file_deleted_from_the_stick_goes_again(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let (space, mini, _) = store.add_test_machine("prusa/mini", None).await;
        let path = "/usb/3dbenchy.bgcode";
        let sent = store
            .create_transfer(space, &mini, "file-1", 1000, path, None)
            .await
            .unwrap();
        store
            .set_transfer(space, &sent.iv, "finished", None, None)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE printer_transfers SET created_at = created_at - interval '100 seconds' WHERE iv = $1",
        )
        .bind(&sent.iv)
        .execute(&pool)
        .await
        .unwrap();
        let listing = |names: &[(&str, &str)]| {
            json!({"children": names.iter().map(|(short, long)| json!({
                "name": short, "display_name": long, "type": "PRINT_FILE", "read_only": false
            })).collect::<Vec<_>>()})
        };
        let back_dated = async |seconds: i64| {
            sqlx::query(
                "UPDATE printer_files SET at = at - make_interval(secs => $2)
                 WHERE machine_id = $1 AND path = '/usb'",
            )
            .bind(&mini)
            .bind(seconds as f64)
            .execute(&pool)
            .await
            .unwrap();
        };

        // A listing from before the transfer does not have it yet: still there.
        store
            .set_printer_file(space, &mini, "/usb", &listing(&[]))
            .await
            .unwrap();
        back_dated(200).await;
        assert_eq!(
            still_on_stick(&store, space, &mini, "file-1", 1000)
                .await
                .unwrap()
                .as_deref(),
            Some(path)
        );
        // Listed after it (by its long name): there.
        store
            .set_printer_file(
                space,
                &mini,
                "/usb",
                &listing(&[("3DBENC~1.BGC", "3dbenchy.bgcode")]),
            )
            .await
            .unwrap();
        assert_eq!(
            still_on_stick(&store, space, &mini, "file-1", 1000)
                .await
                .unwrap()
                .as_deref(),
            Some(path)
        );
        // Gone from the next listing: not there, and the transfer no longer says so.
        store
            .set_printer_file(space, &mini, "/usb", &listing(&[]))
            .await
            .unwrap();
        assert_eq!(
            still_on_stick(&store, space, &mini, "file-1", 1000)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            store.on_stick(space, &mini, "file-1", 1000).await.unwrap(),
            None
        );
        assert!(
            store.sent_paths(space, &mini).await.unwrap().is_empty(),
            "its name is free again"
        );
    }
}
