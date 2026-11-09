//! A printer's own SD card, through its agent (Prusa's MK3, Marlin with `Cap:SDCARD`): what
//! is on it and printing from it. The hub asks with an ordinary command (`M20 L T`) and reads
//! the answer itself; printing is `M23 <file>` and `M24`; the agent follows the print with
//! `M27` and reports it as its job (crates/agent: `Printer::follow_sd`).
//!
//! What `M20 L T` says (Prusa's MK3, `cardreader.cpp`; Marlin alike, without the quotes),
//! between `Begin file list` and `End file list`:
//! - a file: `<path in 8.3 names> <size> [0x<FAT date and time>] ["<long name>"]`, in a
//!   folder its path before it (`/PARTS/BOX~1.GCO`);
//! - a folder (with `L`): `DIR_ENTER: /PARTS/ "Parts"` … `DIR_EXIT`.

use crate::access::Access;
use crate::store::now;
use crate::{Shared, failed};
use axum::{
    Form, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

/// A listing is read again when it is older than this.
const FRESH_SECONDS: i64 = 30;

/// A file on the card.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SdFile {
    /// Its path in 8.3 names, for `M23`.
    pub path: String,
    /// Its long name, where the printer said it; else its 8.3 name.
    pub name: String,
    /// The folder it is in, by its long name ("" at the top).
    pub folder: String,
    pub size: u64,
    /// When it was written (Unix seconds), where the printer said it.
    pub modified: Option<i64>,
}

/// The files in an `M20` answer.
pub fn listing(lines: &[String]) -> Vec<SdFile> {
    let mut files = Vec::new();
    let mut inside = false;
    // Folders entered, by their long names.
    let mut folders: Vec<String> = Vec::new();
    for line in lines {
        let line = line.trim();
        match line {
            "Begin file list" => {
                inside = true;
                continue;
            }
            "End file list" => break,
            "DIR_EXIT" => {
                folders.pop();
                continue;
            }
            _ if !inside => continue,
            _ => {}
        }
        if let Some(rest) = line.strip_prefix("DIR_ENTER: ") {
            let long = rest
                .split_once(" \"")
                .map(|(_, l)| l.trim_end_matches('"').to_owned())
                .unwrap_or_else(|| rest.trim_matches('/').to_owned());
            folders.push(long);
            continue;
        }
        let mut words = line.splitn(2, ' ');
        let Some(path) = words.next().filter(|p| !p.is_empty()) else {
            continue;
        };
        let rest = words.next().unwrap_or("").trim();
        let mut rest_words = rest.splitn(2, ' ');
        let Some(size) = rest_words.next().and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        let mut rest = rest_words.next().unwrap_or("").trim();
        let mut modified = None;
        if let Some(hex) = rest.strip_prefix("0x") {
            let (stamp, after) = hex.split_once(' ').unwrap_or((hex, ""));
            modified = u32::from_str_radix(stamp, 16).ok().and_then(fat_time);
            rest = after.trim();
        }
        let short = path.rsplit('/').next().unwrap_or(path).to_owned();
        let long = rest.trim_matches('"').trim();
        files.push(SdFile {
            path: if path.starts_with('/') {
                path.to_owned()
            } else {
                format!("/{path}")
            },
            name: if long.is_empty() { short } else { long.to_owned() },
            folder: folders.join("/"),
            size,
            modified,
        });
    }
    // PrusaLink's menu (a top folder "PrusaLink menu", its `setready.g`) is not a file to
    // print: left out, as PrusaLink leaves it out of its tree (`handle_special_menu`).
    files.retain(|f| f.folder != "PrusaLink menu" && !f.folder.starts_with("PrusaLink menu/"));
    files
}

/// A FAT date and time (date in the high 16 bits: years since 1980, month, day; time in the
/// low: hours, minutes, seconds / 2), as Unix seconds (the card keeps no time zone: read as
/// UTC).
fn fat_time(value: u32) -> Option<i64> {
    let (date, time) = (value >> 16, value & 0xffff);
    let year = 1980 + (date >> 9) as i64;
    let month = ((date >> 5) & 0x0f) as i64;
    let day = (date & 0x1f) as i64;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (hour, minute, second) = ((time >> 11) as i64, ((time >> 5) & 0x3f) as i64, ((time & 0x1f) * 2) as i64);
    // Days since 1970 (civil from days, Howard Hinnant's algorithm).
    let (y, m) = if month <= 2 { (year - 1, month + 9) } else { (year, month - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/machines/{id}/sd", get(look))
        .route("/machines/{id}/sd/print", post(print))
}

/// What is on its card (read again when older than [`FRESH_SECONDS`], while it is idle).
async fn look(access: Access, State(store): State<Shared>, Path(id): Path<String>) -> Response {
    let mut store = store.lock().unwrap();
    let machine = match store.machine(access.space, &id) {
        Ok(Some(m)) => m,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => return failed(error),
    };
    let known = store.sd_files.get(&id).cloned();
    let fresh = known.as_ref().is_some_and(|(_, at)| now() - at < FRESH_SECONDS);
    let idle = machine
        .telemetry
        .as_ref()
        .is_some_and(|t| t.state == maker_connect_protocol::MachineState::Idle);
    let mut asking = false;
    if !fresh && machine.online && idle {
        // Not twice while one is on its way.
        let open = store.open_commands(&id).iter().any(|c| c.starts_with("M20"));
        asking = open
            || matches!(
                store.queue_command(
                    access.space,
                    &id,
                    "Read its SD card",
                    // Prusa's MK3 as PrusaLink lists it (`sd_card.py`); Marlin's long names.
                    if crate::prusa_mk3_machine(&machine) { "M20 LT" } else { "M20 L T" },
                ),
                Ok(Some(_))
            );
    }
    let (files, at) = known.unwrap_or_default();
    Json(serde_json::json!({
        "files": files,
        "at": (at > 0).then_some(at),
        "asking": asking,
        "online": machine.online,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct PrintForm {
    path: String,
}

/// Prints a file from its card (`M23` and `M24`): only while it is idle with nothing of the
/// hub's to print.
async fn print(
    access: Access,
    State(store): State<Shared>,
    Path(id): Path<String>,
    Form(form): Form<PrintForm>,
) -> Response {
    let path = form.path.trim();
    // 8.3 names and folders only: what M20 gave.
    let plain = path.starts_with('/')
        && path.len() <= 128
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/~_.-".contains(c));
    if !plain {
        return (StatusCode::BAD_REQUEST, "a file from its SD card's list").into_response();
    }
    let mut store = store.lock().unwrap();
    let machine = match store.machine(access.space, &id) {
        Ok(Some(m)) => m,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => return failed(error),
    };
    let idle = machine
        .telemetry
        .as_ref()
        .is_some_and(|t| t.state == maker_connect_protocol::MachineState::Idle);
    if !machine.online || !idle || machine.job.is_some() {
        return (StatusCode::CONFLICT, "it is busy: one print at a time").into_response();
    }
    let name = store
        .sd_files
        .get(&id)
        .and_then(|(files, _)| files.iter().find(|f| f.path == path).map(|f| f.name.clone()))
        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_owned());
    match store.queue_command(
        access.space,
        &id,
        &format!("Print {name} from its SD card"),
        // Prusa's MK3 as PrusaLink loads it: the path in lower case ("FW requires lower case").
        &if crate::prusa_mk3_machine(&machine) {
            format!("M23 {}\nM24", path.to_lowercase())
        } else {
            format!("M23 {path}\nM24")
        },
    ) {
        Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) => (StatusCode::CONFLICT, "it takes no commands").into_response(),
        Err(error) => failed(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    /// Prusa's MK3 with long names and times, folders entered and left; Marlin's plainer form.
    #[test]
    fn a_listing_is_read() {
        let files = listing(&said(
            "echo:SD card ok\nBegin file list\nBENCHY~1.GCO 812345 0x5b4e8c20 \"Benchy 0.2mm PLA.gcode\"\nDIR_ENTER: /PARTS/ \"Parts\"\n/PARTS/BOX~1.GCO 1000 0x5b4e8c20 \"Box.gcode\"\nDIR_EXIT\nCUBE.GCO 20\nEnd file list\nok",
        ));
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].path, "/BENCHY~1.GCO");
        assert_eq!(files[0].name, "Benchy 0.2mm PLA.gcode");
        assert_eq!(files[0].size, 812_345);
        // 0x5b4e: 2025-10-14; 0x8c20: 17:33:00.
        assert_eq!(files[0].modified, Some(1_760_463_180));
        assert_eq!((files[1].path.as_str(), files[1].folder.as_str()), ("/PARTS/BOX~1.GCO", "Parts"));
        assert_eq!((files[2].name.as_str(), files[2].folder.as_str()), ("CUBE.GCO", ""));
        assert_eq!(files[2].modified, None);
        assert!(listing(&said("echo:Unknown command: \"M20\"\nok")).is_empty());
    }

    /// Proves the row `LFN_CAPTURE` of docs/prusalink-audit.md: the answer to `M20 LT` (the
    /// command echoed first, as some hosts' logs have it) gives each file's long name, the
    /// folder it is in, its size and its time.
    #[test]
    fn the_long_names_of_an_m20_lt_listing_are_read() {
        let files = listing(&said(
            "M20 LT\nBegin file list\nBENCHY~1.GCO 812345 0x5b4e8c20 \"Benchy 0.2mm PLA.gcode\"\nDIR_ENTER: /PARTS/ \"Parts\"\n/PARTS/BOX~1.GCO 1000 0x5b4e8c20 \"Box with a long name.gcode\"\nDIR_EXIT\nEnd file list\nok",
        ));
        assert_eq!(files.len(), 2);
        assert_eq!((files[0].path.as_str(), files[0].name.as_str()), ("/BENCHY~1.GCO", "Benchy 0.2mm PLA.gcode"));
        assert_eq!((files[0].size, files[0].modified), (812_345, Some(1_760_463_180)));
        assert_eq!(
            (files[1].name.as_str(), files[1].folder.as_str()),
            ("Box with a long name.gcode", "Parts")
        );
        // PrusaLink's menu is no file to print (its `setready.g`): left out.
        let files = listing(&said(
            "M20 LT\nBegin file list\nDIR_ENTER: /PRUSAL~1/ \"PrusaLink menu\"\n/PRUSAL~1/SETREADY.G 4 \"setready.g\"\nDIR_EXIT\nBENCHY~1.GCO 812345 \"Benchy.gcode\"\nEnd file list\nok",
        ));
        assert_eq!(files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Benchy.gcode"]);
    }
}
