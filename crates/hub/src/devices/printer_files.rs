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
use crate::store::{JobState, Store, now};
use crate::{Shared, failed};
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
use rusqlite::{Connection, OptionalExtension, params};
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

pub fn migrate(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS printer_files (
             machine_id TEXT NOT NULL,
             path TEXT NOT NULL,
             data TEXT,
             at INTEGER,
             asked_at INTEGER,
             PRIMARY KEY (machine_id, path)
         );
         CREATE TABLE IF NOT EXISTS printer_transfers (
             iv TEXT PRIMARY KEY,
             machine_id TEXT NOT NULL,
             space_id INTEGER NOT NULL,
             file_id TEXT NOT NULL,
             key TEXT NOT NULL,
             size INTEGER NOT NULL,
             path TEXT NOT NULL,
             command_id INTEGER,
             job_id INTEGER,
             print_after INTEGER NOT NULL DEFAULT 0,
             state TEXT NOT NULL DEFAULT 'waiting',
             reason TEXT,
             created_at INTEGER NOT NULL
         );",
    )
}

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

impl Store {
    /// The machine (id, space) by its id, when it is in a space.
    fn printer_of_machine(&self, machine: &str) -> rusqlite::Result<Option<(String, i64)>> {
        self.db
            .query_row(
                "SELECT id, space_id FROM machines WHERE id = ?1 AND space_id IS NOT NULL",
                [machine],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
    }

    /// The machine (id, space) a printer's token is for.
    fn printer_of(&self, token: &str) -> rusqlite::Result<Option<(String, i64)>> {
        self.db
            .query_row(
                "SELECT id, space_id FROM machines WHERE token = ?1 AND space_id IS NOT NULL",
                [token],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
    }

    /// What the printer last said of a path, and when (Unix seconds); and when it was last
    /// asked.
    fn printer_file(&self, machine: &str, path: &str) -> rusqlite::Result<Seen> {
        let row: Option<(Option<String>, Option<i64>, Option<i64>)> = self
            .db
            .query_row(
                "SELECT data, at, asked_at FROM printer_files WHERE machine_id = ?1 AND path = ?2",
                params![machine, path],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(match row {
            Some((data, at, asked)) => (
                data.and_then(|d| serde_json::from_str(&d).ok()).zip(at),
                asked,
            ),
            None => (None, None),
        })
    }

    fn set_printer_file(&self, machine: &str, path: &str, data: &Value) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO printer_files (machine_id, path, data, at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (machine_id, path) DO UPDATE SET data = excluded.data, at = excluded.at",
            params![machine, path, data.to_string(), now()],
        )?;
        Ok(())
    }

    fn asked_printer_file(&self, machine: &str, path: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO printer_files (machine_id, path, asked_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (machine_id, path) DO UPDATE SET asked_at = excluded.asked_at",
            params![machine, path, now()],
        )?;
        Ok(())
    }

    /// A path changed: it and its folder are read again when next looked at.
    fn forget_printer_path(&self, machine: &str, path: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE printer_files SET at = 0, asked_at = NULL
             WHERE machine_id = ?1 AND (path = ?2 OR path = ?3)",
            params![machine, path, parent(path)],
        )?;
        Ok(())
    }

    /// A new transfer of a file to a printer's stick: its iv (the address) and key, random.
    /// For a job, the file is printed once it is there.
    fn create_transfer(
        &self,
        space: i64,
        machine: &str,
        file: &str,
        size: u64,
        path: &str,
        job: Option<i64>,
    ) -> rusqlite::Result<Transfer> {
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
        self.db.execute(
            "INSERT INTO printer_transfers (iv, machine_id, space_id, file_id, key, size, path, job_id, print_after, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![transfer.iv, machine, space, file, transfer.key, size as i64, path, job, print_after, now()],
        )?;
        Ok(transfer)
    }

    fn transfer_where(
        &self,
        filter: &str,
        values: &[&dyn rusqlite::ToSql],
    ) -> rusqlite::Result<Option<Transfer>> {
        self.db
            .query_row(
                &format!(
                    "SELECT iv, machine_id, space_id, file_id, key, size, path, job_id, print_after, state
                     FROM printer_transfers WHERE {filter}"
                ),
                values,
                |r| {
                    Ok(Transfer {
                        iv: r.get(0)?,
                        machine: r.get(1)?,
                        space: r.get(2)?,
                        file: r.get(3)?,
                        key: r.get(4)?,
                        size: r.get::<_, i64>(5)? as u64,
                        path: r.get(6)?,
                        job: r.get(7)?,
                        print_after: r.get(8)?,
                        state: r.get(9)?,
                    })
                },
            )
            .optional()
    }

    /// A transfer by its address, while it lasts.
    pub fn transfer(&self, iv: &str) -> rusqlite::Result<Option<Transfer>> {
        self.transfer_where(
            "iv = ?1 AND created_at > ?2",
            &[&iv, &(now() - TRANSFER_SECONDS)],
        )
    }

    /// The paths on a printer's stick that we put there (downloads that got through).
    fn sent_paths(&self, machine: &str) -> rusqlite::Result<Vec<String>> {
        let mut q = self.db.prepare(
            "SELECT DISTINCT path FROM printer_transfers
             WHERE machine_id = ?1 AND state IN ('finished', 'starting', 'printing', 'done')",
        )?;
        q.query_map([machine], |r| r.get(0))?.collect()
    }

    /// A file we put on the stick is not there any more: its transfers no longer say so.
    fn gone_from_stick(&self, machine: &str, path: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE printer_transfers SET state = 'gone'
             WHERE machine_id = ?1 AND path = ?2 AND state IN ('finished', 'done')",
            params![machine, path],
        )?;
        Ok(())
    }

    /// Where this file already is on the printer's stick, put there by us before: the same
    /// file (and size), so it need not go again. With when that transfer began.
    fn on_stick(
        &self,
        machine: &str,
        file: &str,
        size: u64,
    ) -> rusqlite::Result<Option<(String, i64)>> {
        use rusqlite::OptionalExtension;
        self.db
            .query_row(
                "SELECT path, created_at FROM printer_transfers
                 WHERE machine_id = ?1 AND file_id = ?2 AND size = ?3
                   AND state IN ('finished', 'starting', 'printing', 'done')
                 ORDER BY created_at DESC LIMIT 1",
                params![machine, file, size as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
    }

    /// How many downloads a job tried: not more than a few, then it fails.
    fn transfers_of_job(&self, job: i64) -> rusqlite::Result<i64> {
        self.db.query_row(
            "SELECT COUNT(*) FROM printer_transfers WHERE job_id = ?1",
            [job],
            |r| r.get(0),
        )
    }

    /// Prints a file that is on the stick: START_PRINT, and the transfer waits for it.
    fn print_from_stick(&mut self, transfer: &Transfer, machine: &str) -> rusqlite::Result<()> {
        let command = json!({"command": "START_PRINT", "kwargs": {"path": transfer.path}});
        let label = format!("Print {}", transfer.path.rsplit('/').next().unwrap_or(""));
        if let Some(id) =
            self.queue_connect_command(transfer.space, machine, &label, &command, false)?
        {
            self.set_transfer(&transfer.iv, "starting", Some(id), None)?;
        }
        Ok(())
    }

    /// A job's file again, under a name not on the stick yet: after the printer said a file
    /// by that name exists, or that the one we meant to print is gone.
    fn send_again(&mut self, old: &Transfer, job: i64, machine: &str) -> rusqlite::Result<bool> {
        if self.transfers_of_job(job)? >= 3 {
            return Ok(false);
        }
        let Some(file) = self.file(old.space, &old.file)? else {
            return Ok(false);
        };
        let mut taken = names_in(self, machine, "/usb");
        taken.push(old.path.rsplit('/').next().unwrap_or("").to_owned());
        let path = format!("/usb/{}", stick_name(&file.name, &taken));
        let fresh =
            self.create_transfer(old.space, machine, &file.id, file.size, &path, Some(job))?;
        Ok(self.start_transfer(&fresh, &file.name)?.is_some())
    }

    /// Everything on a printer's stick is read again when next looked at.
    fn forget_printer_stick(&self, machine: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE printer_files SET at = 0, asked_at = NULL WHERE machine_id = ?1",
            [machine],
        )?;
        Ok(())
    }

    /// A path deleted from the stick: forgotten, and taken out of its folder's listing
    /// (which stays as fresh as it was: the rest of the folder did not change).
    fn drop_printer_path(&self, machine: &str, path: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "DELETE FROM printer_files WHERE machine_id = ?1 AND (path = ?2 OR path LIKE ?3)",
            params![machine, path, format!("{path}/%")],
        )?;
        let folder = parent(path);
        let name = path.rsplit('/').next().unwrap_or("");
        if let (Some((mut data, at)), _) = self.printer_file(machine, folder)?
            && let Some(children) = data["children"].as_array_mut()
        {
            let before = children.len();
            children.retain(|c| c["name"].as_str() != Some(name));
            if children.len() != before {
                if let Some(count) = data["file_count"].as_i64() {
                    data["file_count"] = json!(count - 1);
                }
                self.db.execute(
                    "UPDATE printer_files SET data = ?3, at = ?4 WHERE machine_id = ?1 AND path = ?2",
                    params![machine, folder, data.to_string(), at],
                )?;
            }
        }
        Ok(())
    }

    /// What a folder's listing says of one of its children, by its path.
    fn listed_child(&self, machine: &str, path: &str) -> rusqlite::Result<Option<Value>> {
        let name = path.rsplit('/').next().unwrap_or("");
        Ok(self
            .printer_file(machine, parent(path))?
            .0
            .and_then(|(data, _)| {
                data["children"]
                    .as_array()?
                    .iter()
                    .find(|c| c["name"].as_str() == Some(name))
                    .cloned()
            }))
    }

    fn transfer_of_command(
        &self,
        machine: &str,
        command: u32,
    ) -> rusqlite::Result<Option<Transfer>> {
        self.transfer_where("machine_id = ?1 AND command_id = ?2", &[&machine, &command])
    }

    fn transfer_of_job(&self, job: i64) -> rusqlite::Result<Option<Transfer>> {
        self.transfer_where("job_id = ?1 ORDER BY created_at DESC LIMIT 1", &[&job])
    }

    fn set_transfer(
        &self,
        iv: &str,
        state: &str,
        command: Option<u32>,
        reason: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE printer_transfers SET state = ?2, command_id = COALESCE(?3, command_id), reason = ?4 WHERE iv = ?1",
            params![iv, state, command, reason],
        )?;
        Ok(())
    }

    /// Queues the download of a transfer on its printer.
    fn start_transfer(&mut self, transfer: &Transfer, name: &str) -> rusqlite::Result<Option<u32>> {
        let command = json!({
            "command": "START_ENCRYPTED_DOWNLOAD",
            "kwargs": {
                "path": transfer.path,
                "key": transfer.key,
                "iv": transfer.iv,
                "orig_size": transfer.size,
            }
        });
        let id = self.queue_connect_command(
            transfer.space,
            &transfer.machine,
            &format!("Send {name} to the USB stick"),
            &command,
            false,
        )?;
        if let Some(id) = id {
            self.set_transfer(&transfer.iv, "sent", Some(id), None)?;
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
fn names_in(store: &Store, machine: &str, folder: &str) -> Vec<String> {
    let mut names = listed_in(store, machine, folder).unwrap_or_default();
    names.extend(
        store
            .sent_paths(machine)
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
fn still_on_stick(
    store: &Store,
    machine: &str,
    file: &str,
    size: u64,
) -> rusqlite::Result<Option<String>> {
    let Some((path, began)) = store.on_stick(machine, file, size)? else {
        return Ok(None);
    };
    let (folder, name) = path.rsplit_once('/').unwrap_or(("/usb", path.as_str()));
    // Only a listing the printer gave after the transfer (one from before it does not have
    // the file yet; a finished transfer marks its folder to be read again, `at` 0).
    let after = store
        .printer_file(machine, folder)?
        .0
        .is_some_and(|(_, at)| at > began);
    match listed_in(store, machine, folder) {
        Some(names) if after && !names.iter().any(|n| n == name) => {
            store.gone_from_stick(machine, &path)?;
            Ok(None)
        }
        _ => Ok(Some(path)),
    }
}

/// The names (long and short) the printer last listed in a folder of its stick; None when
/// it has not listed that folder yet.
fn listed_in(store: &Store, machine: &str, folder: &str) -> Option<Vec<String>> {
    let entry = store.printer_file(machine, folder).ok()?.0?;
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
pub fn on_event(store: &mut Store, token: &str, name: &str, event: &Value) -> rusqlite::Result<()> {
    let Some((machine, _)) = store.printer_of(token)? else {
        return Ok(());
    };
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
            match (asks, text("title"), text("text")) {
                (true, title, body) if title.is_some() || body.is_some() => {
                    let title = title.or_else(|| body.clone()).unwrap_or_default();
                    let mut body = body.unwrap_or_default();
                    // Prusa's error codes (Prusa-Error-Codes: printer, category, number in
                    // five digits) each have a help page at prusa.io/<code>, as the printer's
                    // own screen shows.
                    if let Some(code) = text("code") {
                        let help = (code.len() == 5 && code.bytes().all(|b| b.is_ascii_digit()))
                            .then(|| format!(", prusa.io/{code}"))
                            .unwrap_or_default();
                        body = format!("{body} (#{code}{help})").trim().to_owned();
                    }
                    store.asking.insert(machine.clone(), (title, body));
                    // Its buttons, answered from the website with DIALOG_ACTION: the dialog
                    // (telemetry's and the event's `dialog_id`) and the button's name.
                    let buttons: Vec<String> = data["buttons"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|b| b.as_str().map(str::to_owned))
                        .collect();
                    let dialog = event["dialog_id"].as_u64().or(data["dialog_id"].as_u64());
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
                store.set_printer_file(&machine, &path, data)?;
                // A folder: its files' details and pictures are read ahead, one by one.
                if data["children"].is_array() {
                    prefetch(store, &machine, &path, data)?;
                }
                store.changed();
            }
        }
        // A stick put in or taken out (Prusa's Connect SDK): all of it is read again.
        "MEDIUM_INSERTED" | "MEDIUM_EJECTED" => {
            store.forget_printer_stick(&machine)?;
            store.changed();
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
                    .printing_file
                    .insert(token.to_owned(), (job, name.to_owned()));
            }
        }
        // A file or folder made or deleted (by a command of ours, or on the printer itself).
        // Buddy 6.3 (connect/render.cpp): `old_path` when deleted, `new_path` when made,
        // `new_path` with `rescan` when several things changed at once; paths in 8.3 form.
        "FILE_CHANGED" => {
            if data["rescan"].as_bool() == Some(true) {
                store.forget_printer_stick(&machine)?;
            }
            if let Some(path) = data["old_path"].as_str().and_then(usb_path) {
                // Gone at once from its folder's listing: not shown until the folder is
                // read again.
                store.drop_printer_path(&machine, &path)?;
            }
            if let Some(path) = data["new_path"].as_str() {
                store.forget_printer_path(&machine, path)?;
            }
            store.changed();
        }
        // A download's start is answered with TRANSFER_INFO (Buddy's handle_transfer_result,
        // 6.3 and later) once the printer takes it, but the file is still on its way: said
        // so, until TRANSFER_FINISHED (or a stop) settles the command.
        "TRANSFER_INFO" | "FINISHED" => {
            if let Some(command) = event["command_id"].as_u64()
                && let Some(transfer) = store.transfer_of_command(&machine, command as u32)?
                && !matches!(
                    transfer.state.as_str(),
                    "finished" | "failed" | "starting" | "printing" | "done"
                )
            {
                store.command_went(
                    token,
                    command as u32,
                    crate::store::CommandState::Finished,
                    Some("taken: the file is on its way (see USB)".to_owned()),
                    Vec::new(),
                )?;
            }
        }
        "TRANSFER_FINISHED" | "TRANSFER_ABORTED" | "TRANSFER_STOPPED" => {
            let command = data["start_cmd_id"]
                .as_u64()
                .or_else(|| event["command_id"].as_u64());
            let Some(transfer) =
                command.and_then(|c| store.transfer_of_command(&machine, c as u32).ok().flatten())
            else {
                return Ok(());
            };
            store.forget_printer_path(&machine, &transfer.path)?;
            // The download command is done with its transfer, whatever it said before.
            if let Some(command) = command {
                let (state, reason) = if name == "TRANSFER_FINISHED" {
                    (crate::store::CommandState::Finished, None)
                } else {
                    (crate::store::CommandState::Rejected, Some(name.to_owned()))
                };
                let _ = store.command_went(token, command as u32, state, reason, Vec::new());
            }
            if name == "TRANSFER_FINISHED" {
                println!("on the stick: {}", transfer.path);
                store.set_transfer(&transfer.iv, "finished", None, None)?;
                // A download opens the file's print preview on the printer's screen (Buddy's
                // one-click print), and while it shows the file stays where the download
                // put it, listed read-only. To print: started from there (Buddy takes
                // START_PRINT while the preview shows). Only to the stick: the preview is
                // closed (`on_report`; a WORKAROUND that needs tracing, see there).
                if transfer.print_after {
                    store.print_from_stick(&transfer, &machine)?;
                } else {
                    store.close_preview.insert(machine.clone());
                }
            } else {
                // Why, when it says: STORAGE_FAILURE (the USB stick), NETWORK_FAILURE.
                let why = match data["machine_reason"]
                    .as_str()
                    .or_else(|| event["machine_reason"].as_str())
                    .filter(|r| !r.is_empty())
                {
                    Some("STORAGE_FAILURE") => format!(
                        "the printer stopped the download ({name}): it could not write to its USB stick"
                    ),
                    Some("NETWORK_FAILURE") => format!(
                        "the printer stopped the download ({name}): it could not reach the hub"
                    ),
                    Some(reason) => format!("the printer stopped the download ({name}, {reason})"),
                    None => format!("the printer stopped the download ({name})"),
                };
                store.set_transfer(&transfer.iv, "failed", None, Some(&why))?;
                if let Some(job) = transfer.job {
                    store.set_job(job, JobState::Failed, Some(&why))?;
                }
            }
            store.changed();
        }
        // Refused at once, or started and then failed: for a download or a print of ours.
        "REJECTED" | "FAILED" => {
            if let Some(command) = event["command_id"].as_u64()
                && let Some(transfer) = store.transfer_of_command(&machine, command as u32)?
            {
                let why = event["reason"]
                    .as_str()
                    .unwrap_or("refused by the printer")
                    .to_owned();
                store.set_transfer(&transfer.iv, "failed", None, Some(&why))?;
                // A file by that name is there already (not ours, or changed), or the copy we
                // meant to print is gone: once more, under a free name.
                let lower = why.to_lowercase();
                let again = lower.contains("exist") || lower.contains("not found");
                if let Some(job) = transfer.job
                    && !(again && store.send_again(&transfer, job, &machine)?)
                {
                    store.set_job(job, JobState::Failed, Some(&why))?;
                }
            }
        }
        _ => {}
    }
    Ok(())
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
fn prefetch(
    store: &mut Store,
    machine: &str,
    folder: &str,
    listing: &Value,
) -> rusqlite::Result<()> {
    let Some((_, space)) = store.printer_of_machine(machine)? else {
        return Ok(());
    };
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
        let (entry, when) = store.printer_file(machine, &path)?;
        if still_good(&entry, child["size"].as_u64())
            || when.is_some_and(|a| now() - a < PREFETCH_ASK_EVERY_SECONDS)
        {
            continue;
        }
        let command = json!({"command": "SEND_FILE_INFO", "kwargs": {"path": path}});
        if store
            .queue_connect_command(space, machine, &format!("Read {path}"), &command, true)?
            .is_some()
        {
            store.asked_printer_file(machine, &path)?;
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
pub fn on_report(
    store: &mut Store,
    token: &str,
    state: &str,
    job: Option<u64>,
) -> rusqlite::Result<()> {
    let Some((machine, space)) = store.printer_of(token)? else {
        return Ok(());
    };
    // A print whose file is not known here (started at the printer's screen, or before the
    // hub started): asked once, as Prusa Connect does; JOB_INFO answers.
    if let Some(job) = job
        && store.printing_file.get(token).is_none_or(|(known, _)| *known != Some(job))
        && store.job_info_asked.get(token) != Some(&job)
    {
        store.job_info_asked.insert(token.to_owned(), job);
        let command = json!({"command": "SEND_JOB_INFO", "kwargs": {"job_id": job}});
        store.queue_connect_command(space, &machine, "Which file it prints", &command, true)?;
    }
    // WORKAROUND, NEEDS TRACING (2026-11-08). Buddy opens a file's print preview while a
    // Connect download is still arriving (`transfers/transfer.cpp`, print-while-
    // downloading), and while it shows the file stays read-only; Prusa Connect is not
    // known to close it. How Prusa Connect really handles this is not known yet: trace a
    // MINI on Prusa Connect, then replace this. Until then, a download only to the stick
    // closes the preview with M604 (in the preview Buddy only closes it, as its Back
    // button; tried on the MINI; in a print it would stop it). So only as the answer to a
    // report that says IDLE, with nothing else to go before it. Anything but IDLE:
    // someone pressed Print, or it is busy; no preview to close then.
    if store.close_preview.contains(&machine) {
        if state != "IDLE" {
            store.close_preview.remove(&machine);
        } else if store.open_commands(&machine).is_empty() {
            store.close_preview.remove(&machine);
            store.queue_command(space, &machine, "Close the print preview", "M604")?;
        }
    }
    // No longer asking, or out of its error: what it said goes.
    if !matches!(state, "ATTENTION" | "ERROR") && store.asking.remove(&machine).is_some() {
        store.dialogs.remove(&machine);
        store.changed();
    }
    // A print from the stick is over once the printer is idle again (a minute on: it may
    // still be starting).
    if matches!(state, "IDLE" | "READY" | "FINISHED" | "STOPPED") {
        store.db.execute(
            "UPDATE machines SET usb_print_by = NULL, usb_print_at = NULL
             WHERE id = ?1 AND usb_print_by IS NOT NULL AND usb_print_at < ?2",
            params![machine, crate::store::now() - 60],
        )?;
    }
    let Some(job) = store.active_job(&machine)? else {
        return Ok(());
    };
    let transfer = store.transfer_of_job(job.id)?;
    match (job.state, transfer) {
        (JobState::Queued, None) if matches!(state, "IDLE" | "READY" | "FINISHED" | "STOPPED") => {
            let Some(file) = store.file(space, &job.file_id)? else {
                return store.set_job(job.id, JobState::Failed, Some("its file is gone"));
            };
            // Sent here before and still the same file: print that copy, no download.
            if let Some(path) = still_on_stick(store, &machine, &file.id, file.size)? {
                let transfer = store.create_transfer(
                    space,
                    &machine,
                    &file.id,
                    file.size,
                    &path,
                    Some(job.id),
                )?;
                store.set_transfer(&transfer.iv, "finished", None, Some("on the stick already"))?;
                store.print_from_stick(&transfer, &machine)?;
                store.set_job(job.id, JobState::Printing, None)?;
                return Ok(());
            }
            let taken = names_in(store, &machine, "/usb");
            let path = format!("/usb/{}", stick_name(&file.name, &taken));
            let transfer =
                store.create_transfer(space, &machine, &file.id, file.size, &path, Some(job.id))?;
            if store.start_transfer(&transfer, &file.name)?.is_some() {
                store.set_job(job.id, JobState::Printing, None)?;
            }
        }
        (JobState::Printing | JobState::Paused | JobState::Stopping, Some(t)) => {
            match (t.state.as_str(), state) {
                ("starting", "PRINTING") => store.set_transfer(&t.iv, "printing", None, None)?,
                ("printing", "PAUSED") if job.state == JobState::Printing => {
                    store.set_job(job.id, JobState::Paused, None)?
                }
                ("printing", "PRINTING") if job.state == JobState::Paused => {
                    store.set_job(job.id, JobState::Printing, None)?
                }
                ("printing", "FINISHED") => {
                    store.set_transfer(&t.iv, "done", None, None)?;
                    store.set_job(job.id, JobState::Finished, None)?;
                }
                ("printing", "STOPPED" | "IDLE" | "READY") => {
                    store.set_transfer(&t.iv, "done", None, None)?;
                    store.set_job(job.id, JobState::Stopped, None)?;
                }
                ("printing" | "starting", "ERROR") => {
                    store.set_transfer(&t.iv, "done", None, None)?;
                    store.set_job(
                        job.id,
                        JobState::Failed,
                        Some("the printer reports an error"),
                    )?;
                }
                _ => {}
            }
        }
        _ => {}
    }
    Ok(())
}

/// The command for a job's pause, resume or stop on a printer on its built-in Connect.
pub fn job_control(
    store: &mut Store,
    space: i64,
    machine: &str,
    action: &str,
) -> rusqlite::Result<()> {
    let command = match action {
        "pause" => "PAUSE_PRINT",
        "resume" => "RESUME_PRINT",
        "stop" => "STOP_PRINT",
        _ => return Ok(()),
    };
    store.queue_connect_command(
        space,
        machine,
        &command.replace('_', " ").to_lowercase(),
        &json!({"command": command}),
        false,
    )?;
    Ok(())
}

// ---- routes ---------------------------------------------------------------------------

pub fn routes() -> Router<Shared> {
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
fn connect_printer(
    store: &Store,
    access: &Access,
    machine: &str,
) -> Result<crate::store::Machine, Box<Response>> {
    match store.machine(access.space, machine) {
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
    State(store): State<Shared>,
    Path(machine): Path<String>,
    Query(query): Query<PathQuery>,
) -> Response {
    let Some(path) = usb_path(query.path.as_deref().unwrap_or("/usb")) else {
        return (StatusCode::BAD_REQUEST, "a path on the stick, under /usb").into_response();
    };
    let mut store = store.lock().unwrap();
    let printer = match connect_printer(&store, &access, &machine) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let (entry, asked) = match store.printer_file(&machine, &path) {
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
        if let Ok(Some(_)) = store.queue_connect_command(
            access.space,
            &machine,
            &format!("Read {path}"),
            &command,
            true,
        ) {
            let _ = store.asked_printer_file(&machine, &path);
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
            .open_path_commands(&machine)
            .into_iter()
            .filter(|(_, c)| c.starts_with("DELETE_"))
            .map(|(p, _)| p)
            .collect();
        let listed: Vec<Value> = children
            .iter()
            .map(|c| {
                let mut c = c.clone();
                let child = format!("{path}/{}", c["name"].as_str().unwrap_or(""));
                if deleting.contains(&child) {
                    c["deleting"] = json!(true);
                }
                if c["type"] == "PRINT_FILE" {
                    let known = store
                        .printer_file(&machine, &child)
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
                c
            })
            .collect();
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
    State(store): State<Shared>,
    Path(machine): Path<String>,
    Query(query): Query<PathQuery>,
) -> Response {
    let Some(path) = query.path.as_deref().and_then(usb_path) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let store = store.lock().unwrap();
    if let Err(r) = connect_printer(&store, &access, &machine) {
        return *r;
    }
    let png = store
        .printer_file(&machine, &path)
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
fn path_command(
    store: &Shared,
    access: &Access,
    machine: &str,
    command: &str,
    label: String,
    path: &str,
) -> Response {
    let mut store = store.lock().unwrap();
    let printer = match connect_printer(&store, access, machine) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    if !printer.online {
        return (StatusCode::CONFLICT, "the printer is offline").into_response();
    }
    let body = json!({"command": command, "kwargs": {"path": path}});
    match store.queue_connect_command(access.space, machine, &label, &body, false) {
        Ok(Some(_)) => {
            let _ = store.forget_printer_path(machine, path);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => (StatusCode::CONFLICT, "the printer takes no commands").into_response(),
        Err(error) => failed(error),
    }
}

async fn print(
    access: Access,
    State(store): State<Shared>,
    Path(machine): Path<String>,
    Form(form): Form<PathForm>,
) -> Response {
    let Some(path) = usb_path(&form.path).filter(|p| p != "/usb") else {
        return (StatusCode::BAD_REQUEST, "a file on the stick").into_response();
    };
    {
        let s = store.lock().unwrap();
        if let Err(refused) = crate::safety::check_ppe(&s, &access, &machine) {
            return *refused;
        }
        if let Ok(Some(m)) = s.machine(access.space, &machine) {
            let state = m.telemetry.as_ref().map(|t| t.state);
            if m.job.is_some() || matches!(state, Some(maker_connect_protocol::MachineState::Busy))
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
    );
    if response.status().is_success() {
        // Who started it, for as long as it runs (Now's "Ends next").
        let _ = store.lock().unwrap().db.execute(
            "UPDATE machines SET usb_print_by = ?3, usb_print_at = ?4 WHERE id = ?1 AND space_id = ?2",
            params![machine, access.space, access.actor, crate::store::now()],
        );
    }
    response
}

async fn delete(
    access: Access,
    State(store): State<Shared>,
    Path(machine): Path<String>,
    Form(form): Form<PathForm>,
) -> Response {
    let Some(path) = usb_path(&form.path).filter(|p| p != "/usb") else {
        return (StatusCode::BAD_REQUEST, "a file or folder on the stick").into_response();
    };
    let folder = form.folder.is_some();
    let name = path.rsplit('/').next().unwrap_or("").to_owned();
    {
        let s = store.lock().unwrap();
        // A file still arriving is refused by the printer ("File is being transferred"):
        // said here, before it is asked. Buddy lists such a file as read_only.
        if !folder
            && let Ok(Some(child)) = s.listed_child(&machine, &path)
            && child["read_only"].as_bool() == Some(true)
        {
            return (
                StatusCode::CONFLICT,
                "it is still arriving on the stick: delete it once it is there",
            )
                .into_response();
        }
        if s
            .open_path_commands(&machine)
            .iter()
            .any(|(p, c)| p == &path && c.starts_with("DELETE_"))
        {
            return (StatusCode::CONFLICT, "it is being deleted already").into_response();
        }
    }
    let (command, label) = if folder {
        ("DELETE_FOLDER", format!("Delete the folder {name}"))
    } else {
        ("DELETE_FILE", format!("Delete {name}"))
    };
    path_command(&store, &access, &machine, command, label, &path)
}

async fn new_folder(
    access: Access,
    State(store): State<Shared>,
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
    State(store): State<Shared>,
    Path(machine): Path<String>,
    Form(form): Form<SendForm>,
) -> Response {
    let mut store = store.lock().unwrap();
    let printer = match connect_printer(&store, &access, &machine) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    if !printer.online {
        return (StatusCode::CONFLICT, "the printer is offline").into_response();
    }
    let Some(folder) = usb_path(form.folder.as_deref().unwrap_or("/usb")) else {
        return (StatusCode::BAD_REQUEST, "a folder on the stick").into_response();
    };
    let file = match store.file(access.space, &form.file) {
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
    match still_on_stick(&store, &machine, &file.id, file.size) {
        Ok(Some(path)) if path.rsplit_once('/').map(|(f, _)| f) == Some(folder.as_str()) => {
            return Json(json!({ "path": path, "already": true })).into_response();
        }
        Ok(_) => {}
        Err(error) => return failed(error),
    }
    let taken = names_in(&store, &machine, &folder);
    let path = format!("{folder}/{}", stick_name(&file.name, &taken));
    let sent = store
        .create_transfer(access.space, &machine, &file.id, file.size, &path, None)
        .and_then(|t| store.start_transfer(&t, &file.name));
    match sent {
        Ok(Some(_)) => Json(json!({ "path": path })).into_response(),
        Ok(None) => (StatusCode::CONFLICT, "the printer takes no commands").into_response(),
        Err(error) => failed(error),
    }
}

/// The printer fetches a transfer's file: encrypted (AES-128-CTR), padded to whole blocks,
/// from where it asks (`Range: bytes=N-`).
async fn download(
    State(store): State<Shared>,
    Path(iv): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (transfer, path) = {
        let store = store.lock().unwrap();
        match store.transfer(&iv.to_ascii_lowercase()) {
            Ok(Some(t)) => {
                let path = store.file_path(&t.file);
                (t, path)
            }
            Ok(None) => return (StatusCode::NOT_FOUND, "no such transfer").into_response(),
            Err(error) => return failed(error),
        }
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
    use crate::store::{Store, now};
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::sync::{Arc, Mutex};
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
    #[tokio::test]
    async fn deleting_on_the_stick() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
        let (space, mini, token) = {
            let mut s = store.lock().unwrap();
            let space = s.default_space().unwrap().0;
            let mini = s
                .add_virtual(space, crate::profiles::find("prusa/mini").unwrap())
                .unwrap();
            crate::auth::tester_may_operate(&s, space, &mini);
            let token: String =
                s.db.query_row("SELECT token FROM machines WHERE id = ?1", [&mini], |r| {
                    r.get(0)
                })
                .unwrap();
            s.db.execute(
                "UPDATE machines SET link = 'prusa-connect' WHERE id = ?1",
                [&mini],
            )
            .unwrap();
            (space, mini, token)
        };
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
        let reads = |s: &Store| {
            s.open_path_commands(&mini)
                .iter()
                .filter(|(p, c)| p == "/usb" && c == "SEND_FILE_INFO")
                .count()
        };
        call(&app, report()).await;

        // Asked for the folder twice before it answers: one read queued.
        look(app.clone()).await;
        store
            .lock()
            .unwrap()
            .db
            .execute("UPDATE printer_files SET asked_at = 0", [])
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
        assert_eq!(call(&app, delete("/usb/BENCHY~1.BGC")).await.0, StatusCode::NO_CONTENT);
        let listed = look(app.clone()).await;
        assert_eq!(listed["data"]["children"][0]["deleting"], true);
        assert_eq!(call(&app, delete("/usb/BENCHY~1.BGC")).await.0, StatusCode::CONFLICT);
        let (_, headers, body) = call(&app, report()).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"command": "DELETE_FILE", "kwargs": {"path": "/usb/BENCHY~1.BGC"}})
        );
        let id: u64 = headers["command-id"].to_str().unwrap().parse().unwrap();

        // Buddy answers with FILE_CHANGED only: the command is done and the file is gone.
        call(&app, event(json!({"event": "FILE_CHANGED", "command_id": id, "state": "IDLE",
            "data": {"free_space": 1, "old_path": "/usb/BENCHY~1.BGC", "file": {}}})))
        .await;
        {
            let s = store.lock().unwrap();
            let m = s.machine(space, &mini).unwrap().unwrap();
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
        call(&app, event(json!({"event": "FILE_INFO", "command_id": read, "state": "IDLE", "data": {
            "path": "/usb", "type": "FOLDER", "file_count": 0, "children": []}})))
        .await;

        // A command the printer is still carrying out (its id in the telemetry) is taken on,
        // not given up on, and nothing else is sent meanwhile.
        let busy = store
            .lock()
            .unwrap()
            .queue_connect_command(space, &mini, "Home", &json!({"command": "SEND_INFO"}), false)
            .unwrap()
            .unwrap();
        let (_, headers, _) = call(&app, report()).await;
        assert_eq!(headers["command-id"].to_str().unwrap(), busy.to_string());
        let running = Request::post("/p/telemetry")
            .header("Token", &token)
            .body(Body::from(format!(r#"{{"state":"BUSY","command_id":{busy}}}"#)))
            .unwrap();
        assert_eq!(call(&app, running).await.0, StatusCode::NO_CONTENT);
        let s = store.lock().unwrap();
        let m = s.machine(space, &mini).unwrap().unwrap();
        let c = m.commands.iter().find(|c| c.id == busy).unwrap();
        assert_eq!(c.state, crate::store::CommandState::Accepted);
    }

    /// A MINI, as it talks: its stick looked through (with a file's picture), a file
    /// printed from it, and a file of the space sent to it (encrypted, fetched in two
    /// goes), printed, and its job followed to the end.
    #[tokio::test]
    async fn a_mini_and_its_usb_stick() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
        // A MINI registered and in the space.
        let (space, mini, token) = {
            let mut s = store.lock().unwrap();
            let space = s.default_space().unwrap().0;
            let mini = s
                .add_virtual(space, crate::profiles::find("prusa/mini").unwrap())
                .unwrap();
            crate::auth::tester_may_operate(&s, space, &mini);
            let token: String =
                s.db.query_row("SELECT token FROM machines WHERE id = ?1", [&mini], |r| {
                    r.get(0)
                })
                .unwrap();
            s.db.execute(
                "UPDATE machines SET link = 'prusa-connect' WHERE id = ?1",
                [&mini],
            )
            .unwrap();
            (space, mini, token)
        };
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
            let s = store.lock().unwrap();
            let m = s.machine(space, &mini).unwrap().unwrap();
            assert!(
                m.commands.iter().all(|c| !c.label.contains("BENCHY")),
                "background: not listed"
            );
            let (_, asked) = s.printer_file(&mini, "/usb/BENCHY~1.BGC").unwrap();
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
        let job = || {
            store
                .lock()
                .unwrap()
                .machine(space, &mini)
                .unwrap()
                .unwrap()
                .job
        };
        let job_id = job().unwrap().id;
        assert_eq!(job().unwrap().state, JobState::Printing);
        call(&app, report("PRINTING")).await;
        assert_eq!(job().unwrap().state, JobState::Printing);
        call(&app, report("FINISHED")).await;
        assert!(job().is_none(), "ended");
        let ended = store.lock().unwrap().job(job_id).unwrap().unwrap();
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
        // Someone took it off the stick meanwhile: sent again, under a free name.
        call(&app, event(json!({"event": "REJECTED", "command_id": start_id, "reason": "File not found", "state": "IDLE"}))).await;
        let mut answer = call(&app, report("IDLE")).await;
        if answer.2.is_empty() {
            answer = call(&app, report("IDLE")).await;
        }
        let again: Value = serde_json::from_slice(&answer.2).unwrap();
        assert_eq!(again["command"], "START_ENCRYPTED_DOWNLOAD");
        assert_eq!(again["kwargs"]["path"], "/usb/Big_part-2.gcode");
        assert_eq!(job().unwrap().state, JobState::Printing, "the job goes on");
        // It asks something on its screen: shown as it says it, until it no longer asks.
        call(&app, event(json!({"event": "STATE_CHANGED", "state": "ATTENTION", "dialog_id": 7, "data": {
            "code": "17630", "title": "Sliced for another printer", "text": "The G-code was sliced for MINIIS.", "buttons": ["CONTINUE", "ABORT"]
        }}))).await;
        // Its buttons, pressed from the website: DIALOG_ACTION with the dialog and the name
        // (once the download before has its answer: one command at a time).
        let again_id: u64 = answer.1["command-id"].to_str().unwrap().parse().unwrap();
        call(&app, event(json!({"event": "TRANSFER_INFO", "command_id": again_id, "state": "ATTENTION", "data": {}}))).await;
        let buttons = store.lock().unwrap().machine(space, &mini).unwrap().unwrap().buttons;
        assert_eq!(buttons, ["CONTINUE", "ABORT"]);
        let (status, _, _) = call(
            &app,
            post(format!("/machines/{mini}/answer"), "button=1".into()),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, _, body) = call(&app, report("ATTENTION")).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"command": "DIALOG_ACTION", "kwargs": {"dialog_id": 7, "button": "ABORT"}})
        );
        let (status, _, _) = call(
            &app,
            post(format!("/machines/{mini}/answer"), "button=5".into()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "no such button");
        let asking = || {
            store
                .lock()
                .unwrap()
                .machine(space, &mini)
                .unwrap()
                .unwrap()
                .asking
        };
        assert_eq!(
            asking(),
            Some((
                "Sliced for another printer".to_owned(),
                "The G-code was sliced for MINIIS. (#17630, prusa.io/17630)".to_owned()
            ))
        );
        call(&app, report("PRINTING")).await;
        assert_eq!(asking(), None, "answered on the printer");
        let _ = now();
    }

    /// A print the MINI runs itself (from its stick, or its screen) is recorded as a job when
    /// it ends, as it says; what is left on its bed then holds a queued job back until it is
    /// taken off. Seen missing on the MINI (2026-11-08): a stopped Benchy was not known to be
    /// on the bed, so a queued job could have started on top of it.
    #[tokio::test]
    async fn a_print_the_mini_runs_itself_holds_the_queue_when_it_ends() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
        let (space, mini, token) = {
            let mut s = store.lock().unwrap();
            let space = s.default_space().unwrap().0;
            let mini = s.add_virtual(space, crate::profiles::find("prusa/mini").unwrap()).unwrap();
            crate::auth::tester_may_operate(&s, space, &mini);
            let token: String = s
                .db
                .query_row("SELECT token FROM machines WHERE id = ?1", [&mini], |r| r.get(0))
                .unwrap();
            s.db.execute("UPDATE machines SET link = 'prusa-connect' WHERE id = ?1", [&mini]).unwrap();
            (space, mini, token)
        };
        let report = |body: &str| {
            Request::post("/p/telemetry")
                .header("Token", &token)
                .body(Body::from(body.to_owned()))
                .unwrap()
        };
        let jobs = || {
            store
                .lock()
                .unwrap()
                .db
                .query_row(
                    "SELECT COUNT(*), MAX(state), MAX(file_name) FROM jobs WHERE machine_id = ?1",
                    [&mini],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)),
                )
                .unwrap()
        };
        // What `files::fit` holds a job back by: a finished print on the bed, or the last job
        // stopped or failed and not said to be cleared.
        let held = |s: &Store| {
            let machine = s.machine(space, &mini).unwrap().unwrap();
            (machine.on_bed.is_some(), machine.queue_held.clone())
        };
        call(&app, report(r#"{"state":"IDLE"}"#)).await;
        // Its own print, its file said by JOB_INFO: no job while it runs.
        store.lock().unwrap().printing_file.insert(token.clone(), (Some(29), "Benchy.bgcode".into()));
        call(&app, report(r#"{"state":"PRINTING","job_id":29,"progress":40,"time_printing":600}"#)).await;
        call(&app, report(r#"{"state":"ATTENTION","job_id":29}"#)).await;
        assert_eq!(jobs().0, 0, "a question on its screen is no end");
        call(&app, report(r#"{"state":"PRINTING","job_id":29,"progress":45,"time_printing":700}"#)).await;
        // Stopped on the printer: a stopped job, and the queue waits until the bed is clear.
        call(&app, report(r#"{"state":"STOPPED"}"#)).await;
        assert_eq!(jobs(), (1, Some("stopped".into()), Some("Benchy.bgcode".into())));
        assert_eq!(held(&store.lock().unwrap()), (false, Some("stopped".into())));
        call(&app, report(r#"{"state":"IDLE"}"#)).await;
        assert_eq!(jobs().0, 1, "recorded once");
        store.lock().unwrap().print_taken(space, &mini).unwrap();
        assert_eq!(held(&store.lock().unwrap()), (false, None), "taken off: free again");
        // Finished on the printer: on its bed until taken.
        call(&app, report(r#"{"state":"PRINTING","job_id":30,"progress":99,"time_printing":60}"#)).await;
        call(&app, report(r#"{"state":"FINISHED"}"#)).await;
        assert_eq!(jobs().0, 2);
        assert_eq!(held(&store.lock().unwrap()).0, true, "a finished print on its bed");
    }

    /// Seen on the MINI (2026-11-08): a download only to the stick opens the file's print
    /// preview on the printer's screen, which keeps it read-only. The hub closes it with
    /// M604 (tried on the MINI), so nobody has to go to the printer; but only answering a
    /// report that says IDLE with nothing before it: in a print M604 would stop it.
    #[tokio::test]
    async fn a_download_only_to_the_stick_closes_its_preview() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
        let (mini, token) = {
            let mut s = store.lock().unwrap();
            let space = s.default_space().unwrap().0;
            let mini = s
                .add_virtual(space, crate::profiles::find("prusa/mini").unwrap())
                .unwrap();
            crate::auth::tester_may_operate(&s, space, &mini);
            let token: String =
                s.db.query_row("SELECT token FROM machines WHERE id = ?1", [&mini], |r| {
                    r.get(0)
                })
                .unwrap();
            s.db.execute(
                "UPDATE machines SET link = 'prusa-connect' WHERE id = ?1",
                [&mini],
            )
            .unwrap();
            (mini, token)
        };
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
                call(&app, event(json!({"event": "FINISHED", "command_id": id, "state": state}))).await;
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
                form(format!("/machines/{mini}/usb/send"), format!("file={file}&folder=/usb")),
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
        let queued = {
            let mut s = store.lock().unwrap();
            let space = s.default_space().unwrap().0;
            s.queue_command(space, &mini, "Fan", "M106 S255").unwrap()
        };
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
        call(&app, event(json!({"event": "FINISHED", "command_id": asked, "state": "PRINTING"}))).await;
        let (_, _, body) = call(&app, printing()).await;
        assert!(body.is_empty(), "asked once: {}", String::from_utf8_lossy(&body));
        {
            let s = store.lock().unwrap();
            let space = s.default_space().unwrap().0;
            let m = s.machine(space, &mini).unwrap().unwrap();
            assert_eq!(m.telemetry.unwrap().job.unwrap().file, "Benchy.bgcode");
        }
        let (status, _, body) = call(
            &app,
            form(format!("/machines/{mini}/job"), "action=stop".into()),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{}", String::from_utf8_lossy(&body));
        let (_, _, body) = call(&app, report("PRINTING")).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"command": "STOP_PRINT"})
        );
    }

    /// Seen on the MINI (2026-11-08): a Benchy sent by us, then deleted (the printer names
    /// it by its short name, `3DBENC~1.BGC`); Send here kept saying it was there already.
    #[test]
    fn a_file_deleted_from_the_stick_goes_again() {
        let mut store = Store::open(":memory:").unwrap();
        let space = store.default_space().unwrap().0;
        let mini = store
            .add_virtual(space, crate::profiles::find("prusa/mini").unwrap())
            .unwrap();
        let path = "/usb/3dbenchy.bgcode";
        let sent = store
            .create_transfer(space, &mini, "file-1", 1000, path, None)
            .unwrap();
        store.set_transfer(&sent.iv, "finished", None, None).unwrap();
        store
            .db
            .execute(
                "UPDATE printer_transfers SET created_at = created_at - 100 WHERE iv = ?1",
                [&sent.iv],
            )
            .unwrap();
        let listing = |names: &[(&str, &str)]| {
            json!({"children": names.iter().map(|(short, long)| json!({
                "name": short, "display_name": long, "type": "PRINT_FILE", "read_only": false
            })).collect::<Vec<_>>()})
        };
        let back_dated = |seconds: i64| {
            store
                .db
                .execute(
                    "UPDATE printer_files SET at = at - ?2 WHERE machine_id = ?1 AND path = '/usb'",
                    params![mini, seconds],
                )
                .unwrap();
        };

        // A listing from before the transfer does not have it yet: still there.
        store.set_printer_file(&mini, "/usb", &listing(&[])).unwrap();
        back_dated(200);
        assert_eq!(
            still_on_stick(&store, &mini, "file-1", 1000).unwrap().as_deref(),
            Some(path)
        );
        // Listed after it (by its long name): there.
        store
            .set_printer_file(&mini, "/usb", &listing(&[("3DBENC~1.BGC", "3dbenchy.bgcode")]))
            .unwrap();
        assert_eq!(
            still_on_stick(&store, &mini, "file-1", 1000).unwrap().as_deref(),
            Some(path)
        );
        // Gone from the next listing: not there, and the transfer no longer says so.
        store.set_printer_file(&mini, "/usb", &listing(&[])).unwrap();
        assert_eq!(still_on_stick(&store, &mini, "file-1", 1000).unwrap(), None);
        assert_eq!(store.on_stick(&mini, "file-1", 1000).unwrap(), None);
        assert!(
            store.sent_paths(&mini).unwrap().is_empty(),
            "its name is free again"
        );
    }
}
