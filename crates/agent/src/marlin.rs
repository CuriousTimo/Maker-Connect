//! Streaming G-code to a printer that speaks the Marlin serial protocol (Marlin, Prusa's
//! firmware, and most other FDM printers on USB), the way OctoPrint does it
//! (`reference/OctoPrint`, `plugins/serial_connector/serial_comm.py`):
//!
//! - every line goes out with a line number and a checksum: `N12 G1 X10*85`, the checksum
//!   being the XOR of every byte before the `*`;
//! - one line at a time: the next one goes once the printer answered `ok`;
//! - `Resend: 12` (or `rs N12`) asks for line 12 and those after it again: the last lines
//!   sent are kept for that;
//! - `busy: processing` and `echo:busy` while it heats or homes: keep waiting;
//! - opening the port resets most printers: wait for `start` (or a quiet moment), then
//!   `M110 N0` sets the line numbers back to zero;
//! - what is on its screen comes as host actions (Marlin's `HOST_ACTION_COMMANDS` and
//!   `HOST_PROMPT_SUPPORT`, Prusa's firmware on the MK3 and older): a question
//!   (`//action:prompt_begin Filament runout`, `prompt_button Continue`, `prompt_show`,
//!   `prompt_end`), paused or stopped from its knob (`//action:pause`, `resume`, `cancel`,
//!   `paused`, `resumed`); and its fatal errors (`Error:Thermal Runaway`, `MINTEMP`,
//!   `Printer halted. kill() called!`). All kept in its [`Screen`].

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How many sent lines are kept for a resend.
const KEPT_LINES: usize = 64;
/// How long a printer may stay silent before the sender asks after it, unless it said busy.
const SILENT_TIMEOUT: Duration = Duration::from_secs(30);
/// The same, while a command that takes long runs (homing, levelling, a heat-up, `M400`)
/// on a printer that does not say busy.
const LONG_SILENCE: Duration = Duration::from_secs(15 * 60);
/// How often it is asked after (with a numbered `M105`) before the sender gives up.
const MAX_POKES: usize = 3;
/// How long to wait for the `ok` that should follow a resend request (some firmware
/// forgets it; OctoPrint waits 1 s too).
/// Prusa's MK3: how long PrusaLink waits for a sign of life before it sends the waiting line
/// again (`SERIAL_QUEUE_TIMEOUT`).
const PRUSALINK_TIMEOUT: Duration = Duration::from_secs(25);
const RESEND_OK_WAIT: Duration = Duration::from_secs(1);
/// How often the same line may be asked for again in a row before the link counts as broken
/// (OctoPrint's limit).
const MAX_SAME_RESENDS: usize = 10;
/// How long to wait for a printer to finish booting after the port was opened.
const BOOT_WAIT: Duration = Duration::from_secs(10);

/// A G-code line as it goes on the wire, without its comment: `None` for a line that is
/// only a comment or blank.
pub fn clean(line: &str) -> Option<&str> {
    let code = line.split(';').next().unwrap_or("").trim();
    (!code.is_empty()).then_some(code)
}

/// The XOR of every byte: Marlin's line checksum.
pub fn checksum(text: &str) -> u8 {
    text.bytes().fold(0, |sum, byte| sum ^ byte)
}

/// A line numbered and with its checksum, ready to send (without the newline).
pub fn numbered(number: u32, code: &str) -> String {
    let line = format!("N{number} {code}");
    format!("{line}*{}", checksum(&line))
}

/// What a line from the printer means for the sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// `ok`, possibly with a temperature report after it.
    Ok,
    /// `Resend: N` or `rs N`: send line N and those after it again.
    Resend(u32),
    /// `busy: processing`, `echo:busy`: it is working; keep waiting.
    Busy,
    /// `echo:busy: paused for user` or `paused for input`: it waits for someone to press
    /// its knob (`M0`, `M600`, a question on its screen). Kept waiting for, but said: nobody
    /// may be there.
    PausedForUser,
    /// `start`: it has (re)started.
    Start,
    /// `wait` (Marlin's `NO_TIMEOUTS`): idle with an empty queue. While an `ok` is awaited,
    /// it (or the line) was lost.
    Wait,
    /// `Error:...`.
    Error(String),
    /// `//action:...`: what is on its screen, or what was done at its knob.
    Action(Action),
    /// Anything else: temperatures, `echo:` messages.
    Other(String),
}

pub fn parse(line: &str) -> Reply {
    let line = line.trim();
    let lower = line.to_ascii_lowercase();
    if lower == "ok" || lower.starts_with("ok ") {
        return Reply::Ok;
    }
    // Marlin: "Resend: 12"; some firmwares: "rs N12" or "rs 12".
    let resend = lower
        .strip_prefix("resend:")
        .or_else(|| lower.strip_prefix("rs "))
        .map(|rest| rest.trim().trim_start_matches('n').trim());
    if let Some(Ok(number)) = resend.map(str::parse) {
        return Reply::Resend(number);
    }
    if lower.starts_with("busy:") || lower.starts_with("echo:busy") {
        if lower.contains("paused for user") || lower.contains("paused for input") {
            return Reply::PausedForUser;
        }
        return Reply::Busy;
    }
    if lower == "start" {
        return Reply::Start;
    }
    if lower == "wait" {
        return Reply::Wait;
    }
    if lower.starts_with("error:") {
        return Reply::Error(line[6..].trim().to_owned());
    }
    // Marlin's "!! KILL caused by ..." and "!! STOP called because of ..."; "fatal:" from
    // other firmware (OctoPrint reads both as errors).
    if let Some(rest) = line.strip_prefix("!!") {
        return Reply::Error(rest.trim().to_owned());
    }
    if lower.starts_with("fatal:") {
        return Reply::Error(line.to_owned());
    }
    // `//action:` and, from Prusa's firmware now and then, `// action:`.
    if let Some(rest) = line
        .strip_prefix("//")
        .map(str::trim_start)
        .and_then(|r| r.strip_prefix("action:"))
    {
        let (name, text) = rest.trim().split_once(' ').unwrap_or((rest.trim(), ""));
        let text = text.trim().to_owned();
        return Reply::Action(match name.to_ascii_lowercase().as_str() {
            "prompt_begin" => Action::PromptBegin(text),
            "prompt_button" | "prompt_choice" => Action::PromptButton(text),
            "prompt_show" => Action::PromptShow,
            "prompt_end" => Action::PromptEnd,
            "notification" => Action::Notification(text),
            // Asked from its knob ("pause", "resume", "cancel": the host does it), or done
            // by itself ("paused", "resumed": a filament change, a runout).
            "pause" => Action::Pause,
            "paused" => Action::Paused,
            "resume" | "resumed" => Action::Resume,
            "cancel" => Action::Cancel,
            // Prusa's firmware (MK3 and older): "Set Ready" in its menu, and a print it
            // brought back after a power cut, waiting to be resumed (Marlin_main.cpp).
            "ready" => Action::Ready,
            "not_ready" => Action::NotReady,
            "uvlo_recovery_ready" | "uvlo_auto_recovery_ready" => Action::PowerLoss,
            "start" => Action::Reprint,
            // Marlin's ACTION_ON_KILL: it was killed and stays dead until reset.
            "poweroff" | "kill" => Action::Killed,
            // Said for someone to read: a runout, a failed probe, heaters switched off for
            // inactivity (Buddy), a crash dump (Prusa's MK3).
            "filament_runout" => Action::Notice(format!("Filament runout {text}").trim().to_owned()),
            "probe_failed" => Action::Notice("Probing failed".to_owned()),
            "probe_rewipe" => Action::Notice("Probing again after wiping the nozzle".to_owned()),
            "safety_timer_expired" => {
                Action::Notice("Heaters switched off: nothing happened for too long".to_owned())
            }
            "dump_available" => Action::Notice("The printer has a crash dump".to_owned()),
            _ => Action::Other(rest.trim().to_owned()),
        });
    }
    Reply::Other(line.to_owned())
}

/// A host action: what a printer shows, or what someone did at its knob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    PromptBegin(String),
    PromptButton(String),
    PromptShow,
    PromptEnd,
    /// A message on its screen (`M117`).
    Notification(String),
    /// Pause, from its knob: the host pauses sending.
    Pause,
    /// It paused itself (a filament change, a runout).
    Paused,
    Resume,
    /// Stopped from its own menu: the host stops sending.
    Cancel,
    /// "Set Ready" in its menu: its bed is clear for the next print.
    Ready,
    NotReady,
    /// A print interrupted by a power cut, brought back and waiting to be resumed.
    PowerLoss,
    /// "Reprint" in its menu (Prusa's MK3, after a host's print): print the last job again.
    /// Asked of the hub, which checks it as any job (the bed clear first).
    Reprint,
    /// Killed (`poweroff`): dead until reset.
    Killed,
    /// Something for someone to read: a runout, a failed probe, heaters off for inactivity.
    Notice(String),
    /// Anything else, `shutdown` among them: a knob on the printer never shuts down the
    /// computer the agent runs on.
    Other(String),
}

/// Whether an error stops the printer (it must be restarted), unlike a line sent wrong,
/// which is only asked again.
/// The words are those of Marlin 2 (`language.h`), Buddy's Marlin and Prusa's MK3 firmware
/// (`temperature.cpp`, `Marlin_main.cpp`); see docs/marlin-serial-protocol.md.
pub fn fatal(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "kill()",
        "kill caused by",
        "stop called because of",
        "printer halted",
        "thermal runaway",
        "thermal malfunction",
        "mintemp",
        "maxtemp",
        "heating failed",
        "heaters switched off",
        "printer stopped",
        "stopped due to errors",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// Whether a line says the printer needs someone, without being dead: Prusa's MK3 (as
/// PrusaLink reads it: a failed bed levelling, its thermal model, a slow fan, a power cut)
/// and Marlin locked by its password. The text to show, if so.
pub fn needs_someone(line: &str) -> Option<String> {
    let text = line
        .trim()
        .trim_start_matches("echo:")
        .trim_start_matches("Error:")
        .trim();
    let lower = text.to_ascii_lowercase();
    if lower == "int4" {
        return Some("Power failure: the print was saved".to_owned());
    }
    [
        "bed leveling failed",
        "tm: error triggered",
        "fan speed is lower than expected",
        "printer locked",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    .then(|| text.to_owned())
}

/// Whether a line is a routine report, not worth a log line: temperatures (`T:`), fans
/// (`E0:… RPM`), position (`X:… Y:…`), the MK3's chatter with its MMU (`echo:MMU2:<…`,
/// `>Q0…`) and its screen (`LCD status changed`).
pub fn routine(line: &str) -> bool {
    let line = line.trim();
    line.starts_with("T:")
        || line.starts_with("ok T:")
        || line.starts_with("B:")
        || (line.starts_with("E0:") && line.contains(" RPM"))
        || (line.starts_with("X:") && line.contains(" Y:") && line.contains(" Z:"))
        || line.starts_with("echo:MMU2:<")
        || line.starts_with("echo:MMU2:>")
        || line.starts_with("echo:MMU3:<")
        || line.starts_with("echo:MMU3:>")
        || line == "LCD status changed"
}

/// A print from the printer's own SD card, from its `M27` answer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SdStatus {
    /// Its file (Prusa's MK3: the long name on the line before; Marlin with `M27 C`:
    /// `Current file: NAME.GCO long name`).
    pub file: Option<String>,
    pub done: u64,
    pub total: u64,
    /// How long it has printed (Prusa's MK3: `h:m` after the bytes).
    pub elapsed_s: Option<u64>,
    /// Paused (Prusa's MK3: `SD print paused`; Marlin says the bytes while paused too).
    pub paused: bool,
}

/// What `M27` says: `Some(Some(status))` while it prints from its card, `Some(None)` when
/// it does not (`Not SD printing`; on Prusa's MK3 also `Print saved`: a host's print,
/// paused or kept after a power cut, not the card's), `None` when it said neither (no card
/// support).
pub fn sd_status(lines: &[String]) -> Option<Option<SdStatus>> {
    let lines: Vec<&str> = lines.iter().map(|l| l.trim()).collect();
    if lines.iter().any(|l| *l == "Not SD printing" || *l == "Print saved") {
        return Some(None);
    }
    if lines.contains(&"SD print paused") {
        return Some(Some(SdStatus {
            paused: true,
            ..SdStatus::default()
        }));
    }
    let at = lines.iter().position(|l| l.starts_with("SD printing byte "))?;
    let (done, total) = lines[at]
        .trim_start_matches("SD printing byte ")
        .split_once('/')?;
    let mut status = SdStatus {
        done: done.trim().parse().ok()?,
        total: total.trim().parse().ok()?,
        ..SdStatus::default()
    };
    // The file: Marlin's "Current file:", or the MK3's line before the bytes.
    status.file = lines
        .iter()
        .find_map(|l| l.strip_prefix("Current file: "))
        .map(|f| f.split_once(' ').map_or(f, |(_, long)| long).trim().to_owned())
        .or_else(|| {
            at.checked_sub(1)
                .map(|i| lines[i])
                .filter(|l| !l.is_empty() && !l.starts_with("ok") && !routine(l) && !l.starts_with("echo:"))
                .map(|l| l.rsplit('/').next().unwrap_or(l).to_owned())
        })
        .filter(|f| !f.is_empty());
    // The MK3's "h:m" after the bytes.
    status.elapsed_s = lines.get(at + 1).and_then(|l| {
        let (h, m) = l.split_once(':')?;
        Some(h.parse::<u64>().ok()? * 3600 + m.parse::<u64>().ok()? * 60)
    });
    Some(Some(status))
}

/// The MMU's state in Prusa's MK3 firmware's chatter with it: an answer
/// `echo:MMU2:<T0 E8004*3c.` carries an error (`E` and its code), `P…`/`F…` none. `Some(None)`:
/// no error now; `Some(Some(code))`: Prusa's code for it (04xxx, as on its screen and at
/// prusa.io); `None`: not such a line.
pub fn mmu_error(line: &str) -> Option<Option<u32>> {
    let rest = line
        .trim()
        .strip_prefix("echo:MMU2:<")
        .or_else(|| line.trim().strip_prefix("echo:MMU3:<"))?;
    let (command, progress) = rest.split_once(' ')?;
    if !command.starts_with(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    let progress = progress.split('*').next()?;
    match progress.strip_prefix('E') {
        Some(hex) => Some(u32::from_str_radix(hex, 16).ok().map(prusa_mmu_code)),
        None if progress.starts_with(['P', 'F']) => Some(None),
        None => None,
    }
}

/// The MMU's own error number as Prusa numbers it (04xxx): the facts in the MK3 firmware's
/// `mmu2_error_converter.cpp` and PrusaLink's map. The driver an error is about adds 0x80
/// (selector) or 0x100 (idler) to it, and 10 or 20 to Prusa's number. Unknown: 04900.
fn prusa_mmu_code(mmu: u32) -> u32 {
    let (base, driver) = match mmu & 0x0180 {
        0x0080 => (mmu & !0x0080, 1),
        0x0100 => (mmu & !0x0100, 2),
        _ => (mmu, 0),
    };
    let code = match (base, driver) {
        (0x8001, 0) => 101,
        (0x8002, 0) => 102,
        (0x8003, 0) => 103,
        (0x8004, 0) => 104,
        (0x800b, 0) => 105,
        (0x8009, 0) => 106,
        (0x800a, 0) => 107,
        (0x802a, 0) => 108,
        (0x8007, 1) => 115,
        (0x800b, 1) => 116,
        (0x8007, 2) => 125,
        (0x800b, 2) => 126,
        (0xA000, d) => 201 + 10 * d,
        (0xC000, d) => 202 + 10 * d,
        (0x8200, d) => 301 + 10 * d,
        (0x8400, d) => 302 + 10 * d,
        (0x8800, d) => 303 + 10 * d,
        (0x9000, d) => 304 + 10 * d,
        (0xC200, d) => 305 + 10 * d,
        (0x800d, 0) => 306,
        (0x802e, 0) => 401,
        (0x802d, 0) => 402,
        _ => 900,
    };
    4000 + code
}

/// Whether an `Error:` is about the line on the wire (a resend follows) or harmless, rather
/// than about the print: OctoPrint's classes, which every firmware here fits.
pub fn harmless_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        // The line on the wire: a resend follows.
        "line number",
        "linenumber",
        "checksum",
        "format error",
        "expected line",
        // The SD card, not this print.
        "volume.init",
        "openroot",
        "workdir",
        "error writing to file",
        "cannot open",
        "open failed",
        "cannot enter",
        "unknown command",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// What a printer can do, from the `Cap:NAME:1` lines of its `M115` answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// `M108`, `M112`, `M410`, `M876` act at once, even while it heats or waits.
    pub emergency_parser: bool,
    /// `M155` works.
    pub autoreport_temp: bool,
    /// Questions on its screen come as `//action:prompt_*`, answered with `M876`.
    pub prompt_support: bool,
    /// Every capability it named, as it named it, and whether it is on.
    pub all: Vec<(String, bool)>,
}

pub fn capabilities(lines: &[String]) -> Capabilities {
    let mut caps = Capabilities::default();
    for line in lines {
        let Some(rest) = line.trim().strip_prefix("Cap:") else {
            continue;
        };
        let Some((name, on)) = rest.rsplit_once(':') else {
            continue;
        };
        let on = on.trim() == "1";
        match name.trim() {
            "EMERGENCY_PARSER" => caps.emergency_parser = on,
            "AUTOREPORT_TEMP" => caps.autoreport_temp = on,
            "PROMPT_SUPPORT" => caps.prompt_support = on,
            _ => {}
        }
        caps.all.push((name.trim().to_owned(), on));
    }
    caps
}

/// Commands that may take long before their `ok` (OctoPrint's list, plus heating, Prusa's
/// mesh and filament codes): no "it stopped answering" for them.
pub fn long_running(code: &str) -> bool {
    let word = code.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
    matches!(
        word.as_str(),
        "G4" | "G28"
            | "G29"
            | "G30"
            | "G32"
            | "G80"
            | "M0"
            | "M1"
            | "M109"
            | "M190"
            | "M191"
            | "M303"
            | "M400"
            | "M226"
            | "M600"
            | "M701"
            | "M702"
    )
}

/// A heating report, as PrusaLink knows one (`HEATING_REGEX`, `HEATING_HOTEND_REGEX`):
/// `T:210.5 E:0 B:60.1` or `T:210.5 E:0 W:3`.
fn prusalink_heating(line: &str) -> bool {
    let w: Vec<&str> = line.trim().split(' ').collect();
    let num = |t: &str| t.split_once('.').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty() && a.chars().chain(b.chars()).all(|c| c.is_ascii_digit()));
    let int_or_q = |t: &str| t == "?" || (!t.is_empty() && t.chars().all(|c| c.is_ascii_digit()));
    match w.as_slice() {
        [t, e, b] => {
            t.strip_prefix("T:").is_some_and(num)
                && ((e.strip_prefix("E:").is_some_and(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit()))
                    && b.strip_prefix("B:").is_some_and(num))
                    || (e.strip_prefix("E:").is_some_and(int_or_q) && b.strip_prefix("W:").is_some_and(int_or_q)))
        }
        _ => false,
    }
}

/// Commands that wait for a temperature, which `M108` breaks off (with the emergency
/// parser).
fn heats(code: &str) -> bool {
    let word = code.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
    matches!(word.as_str(), "M109" | "M190" | "M191")
}

/// What is on a printer's screen, as it said over the wire: shared with whoever reports.
#[derive(Debug, Default)]
pub struct Screen {
    /// `busy: paused for user`: it waits for its knob (an `M0`, a filament change).
    pub waiting: AtomicBool,
    state: Mutex<ScreenState>,
}

/// What a [`Screen`] holds now.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScreenState {
    /// The question on its screen, and its buttons ("Continue", "Purge more").
    pub asking: Option<(String, Vec<String>)>,
    /// A fatal error: it stopped, and waits to be restarted.
    pub fault: Option<String>,
    /// Paused at its knob (or by itself), until resumed.
    pub paused: bool,
    /// Stopped from its own menu: what is being sent stops.
    pub cancelled: bool,
    /// The last message on its screen.
    pub message: Option<String>,
    /// "Set Ready" was chosen on it, not reported yet.
    pub ready_said: bool,
    /// "Not ready" was chosen on it, not reported yet.
    pub not_ready_said: bool,
    /// A print interrupted by a power cut waits to be resumed on it.
    pub power_loss: bool,
    /// It needs someone without being dead (a failed bed levelling, a slow fan, locked).
    pub attention: Option<String>,
    /// "Reprint" was chosen on it, not reported yet.
    pub reprint_said: bool,
    /// Whether an SD card is in it, as it last said (`echo:SD card ok`, `SD init fail`,
    /// `SD card released`).
    pub sd_card: Option<bool>,
    /// Inside a file list (`Begin file list` until `End file list`): a file in it means a card
    /// is in, as PrusaLink takes files found as present (sd_card.py `update`).
    pub sd_listing: bool,
    /// A print from its card started at its menu (`echo:enqueing "M24"`), until `M27 P`
    /// says so; and one ended (`Done printing file`), until reported (PrusaLink:
    /// `START_PRINT_REGEX` PRINTING, `PRINT_DONE_REGEX` FINISHED, at once).
    pub sd_started: bool,
    pub sd_done: bool,
    /// How many `LCD status changed` it said, not taken yet (crate::lcd).
    pub lcd_changed: u32,
    /// PrusaLink's menu on its card (the folder "PrusaLink menu", by its short name in lower
    /// case), and when its `setready.g` was opened, started or done last: within 1 s of each
    /// other that is "Set Ready", not a print (`special_commands.py`).
    pub menu_sfn: Option<String>,
    pub special_at: Option<std::time::Instant>,
    /// The file a print from its card opened (`File opened: … Size: …`), its short name.
    pub sd_opened: Option<String>,
    /// Its MMU: the slot in use (`MMU2tool=N`), and what it is doing (its progress).
    pub mmu_slot: Option<u8>,
    pub mmu_doing: Option<String>,
    /// Calibrating its thermal model (`TM: calibration start` until it ends): nothing may be
    /// sent meanwhile (PrusaLink blocks its queue then).
    pub calibrating: bool,
    /// Its bed was measured anew (`echo:enqueing "G80"`, or the homing before it): its
    /// mesh is to be read again.
    pub mesh_stale: bool,
    /// When it last reported by itself (`M155`): temperatures, fans, position.
    pub auto_temps: Option<std::time::Instant>,
    pub auto_fans: Option<std::time::Instant>,
    pub auto_position: Option<std::time::Instant>,
    /// Its last own reports (temperatures, fans, position), as it said them: Prusa's MK3 is
    /// asked nothing for these (PrusaLink reads only what `M155 S2 C7` brings).
    pub reported: [Option<String>; 3],
    // A question being told, not shown yet.
    pending: Option<(String, Vec<String>)>,
}

impl Screen {
    pub fn now(&self) -> ScreenState {
        self.state.lock().unwrap().clone()
    }

    /// "Set Ready" from its menu, once: taken by whoever reports it.
    pub fn take_not_ready(&self) -> bool {
        std::mem::take(&mut self.state.lock().unwrap().not_ready_said)
    }

    pub fn take_ready(&self) -> bool {
        std::mem::take(&mut self.state.lock().unwrap().ready_said)
    }

    /// "Reprint" from its menu, once.
    pub fn take_reprint(&self) -> bool {
        std::mem::take(&mut self.state.lock().unwrap().reprint_said)
    }

    /// Its mesh is to be read again (a Prusa MK2.5, which says nothing when it levels: on
    /// every change to printing or idle, as PrusaLink does).
    pub fn mesh_changed(&self) {
        self.state.lock().unwrap().mesh_stale = true;
    }

    /// PrusaLink's menu folder on its card (its short name, lower case), once listed.
    pub fn set_menu(&self, menu_sfn: Option<String>) {
        self.state.lock().unwrap().menu_sfn = menu_sfn;
    }

    /// How many `LCD status changed` it said since asked last.
    pub fn take_lcd_changed(&self) -> u32 {
        std::mem::take(&mut self.state.lock().unwrap().lcd_changed)
    }

    /// Whether a print from its card ended (`Done printing file`), once.
    pub fn take_sd_done(&self) -> bool {
        std::mem::take(&mut self.state.lock().unwrap().sd_done)
    }

    /// A print from its card started at its menu is seen in `M27 P` now.
    pub fn sd_start_seen(&self) {
        self.state.lock().unwrap().sd_started = false;
    }

    /// Whether its mesh is to be read again, once.
    pub fn take_mesh_stale(&self) -> bool {
        std::mem::take(&mut self.state.lock().unwrap().mesh_stale)
    }

    /// Whether it reported temperatures, fans and position by itself within `within`
    /// (PrusaLink switches `M155` on again when any of them has not come for a minute).
    pub fn reports_by_itself(&self, within: Duration) -> bool {
        let s = self.state.lock().unwrap();
        [s.auto_temps, s.auto_fans, s.auto_position]
            .iter()
            .all(|t| t.is_some_and(|t| t.elapsed() < within))
    }

    /// Its last own reports (temperatures, fans, position), oldest kind first.
    pub fn reported(&self) -> Vec<String> {
        let s = self.state.lock().unwrap();
        [&s.reported[2], &s.reported[1], &s.reported[0]].into_iter().flatten().cloned().collect()
    }

    /// `M155` was (re)started: it is given `within` to report before it counts as silent.
    pub fn reporting_started(&self) {
        let mut s = self.state.lock().unwrap();
        let now = std::time::Instant::now();
        (s.auto_temps, s.auto_fans, s.auto_position) = (Some(now), Some(now), Some(now));
    }

    /// A job ends or starts: a stop from its menu is dealt with.
    pub fn clear_cancel(&self) {
        self.state.lock().unwrap().cancelled = false;
    }

    /// A print cut off by a power cut was taken up (or stopped): no longer waiting for it.
    pub fn clear_power_loss(&self) {
        self.state.lock().unwrap().power_loss = false;
    }

    /// A line from the printer, for tests elsewhere.
    #[cfg(test)]
    pub(crate) fn hear(&self, line: &str) {
        self.heard(&parse(line), line);
    }

    /// Paused by a line of the file itself (`M601` on Prusa's MK3): nothing more is sent
    /// until it says it resumed (`//action:resumed`).
    pub fn paused_by_file(&self) {
        self.state.lock().unwrap().paused = true;
    }

    /// What a line from the printer changes on its screen.
    fn heard(&self, reply: &Reply, line: &str) {
        self.waiting
            .store(*reply == Reply::PausedForUser, Ordering::Relaxed);
        let mut s = self.state.lock().unwrap();
        if let Some(text) = needs_someone(line) {
            s.attention = Some(text);
        }
        // Prusa's MK3: its thermal model error went away by itself.
        if line.trim() == "TM: error cleared" {
            s.attention = None;
        }
        let text = line.trim();
        // Its own reports (M155): when each kind last came.
        let now = std::time::Instant::now();
        if text.starts_with("T:") || text.starts_with("ok T:") {
            s.auto_temps = Some(now);
            if text.starts_with("T:") {
                s.reported[0] = Some(text.to_owned());
            }
        }
        if text.starts_with("E0:") && text.contains(" RPM") {
            s.auto_fans = Some(now);
            s.reported[1] = Some(text.to_owned());
        }
        if text.starts_with("X:") && text.contains(" Count ") {
            s.auto_position = Some(now);
            s.reported[2] = Some(text.to_owned());
        }
        // Its SD card (Prusa's MK3 and Marlin say so on their own).
        match text {
            "echo:SD card ok" => s.sd_card = Some(true),
            "echo:SD init fail" | "Error:volume.init failed" | "Error:openRoot failed"
            | "echo:SD card released" => s.sd_card = Some(false),
            _ => {}
        }
        // PrusaLink's menu: its `setready.g` opened, started and done within 1 s each is
        // "Set Ready" (`handle_file`, `handle_start`, `handle_done`, `CMD_TIMEOUT`).
        let special_now = s.special_at.is_some_and(|at| at.elapsed() < Duration::from_secs(1));
        if let Some(opened) = text.strip_prefix("File opened: ").and_then(|r| r.split(" Size: ").next())
            && let Some(menu) = s.menu_sfn.clone()
        {
            let path = opened.to_lowercase();
            let parts: Vec<&str> = path.rsplitn(3, '/').collect();
            if parts.len() >= 2 && parts[1] == menu && parts[0] == "setready.g" {
                s.special_at = Some(now);
                return;
            }
        }
        match text {
            "LCD status changed" => s.lcd_changed += 1,
            "echo:enqueing \"M24\"" if special_now => s.special_at = Some(now),
            "echo:enqueing \"M24\"" => {
                s.special_at = None;
                s.sd_started = true;
            }
            "Done printing file" if special_now => {
                s.special_at = None;
                s.ready_said = true;
            }
            "Done printing file" => {
                s.special_at = None;
                s.sd_done = true;
            }
            _ => {}
        }
        // Its file list: any file in it, and the card is in (the insert said nothing, or
        // was missed).
        match text {
            "Begin file list" => s.sd_listing = true,
            "End file list" => s.sd_listing = false,
            _ if s.sd_listing && !text.is_empty() && !text.starts_with("DIR_") && text != "ok" => {
                s.sd_card = Some(true)
            }
            _ => {}
        }
        if let Some(rest) = text.strip_prefix("File opened: ") {
            s.sd_opened = rest.split(" Size: ").next().map(str::to_owned);
            s.sd_card = Some(true);
        }
        // Prusa's MK3: calibrating its thermal model, and its end (failed, or its settings).
        if text == "TM: calibration start" {
            s.calibrating = true;
        } else if text.starts_with("TM: calibr. failed!") || text.starts_with("Thermal Model settings:") {
            s.calibrating = false;
        }
        // Its bed being measured: the mesh read later is new.
        if text == "echo:enqueing \"G80\"" || (text.starts_with("tmc") && text.contains("_home_enter(axes_mask=")) {
            s.mesh_stale = true;
        }
        // Prusa's MK3 with an MMU: the slot in use, and what it is doing.
        if let Some(slot) = text.strip_prefix("echo:MMU2:MMU2tool=").and_then(|n| n.parse().ok()) {
            s.mmu_slot = Some(slot);
        }
        if let Some(doing) = mmu_progress(text) {
            s.mmu_doing = (doing != "OK").then(|| doing.to_owned());
        }
        // Prusa's MK3 with an MMU: an error in its chatter, or none any more.
        match mmu_error(line) {
            Some(Some(code)) => {
                s.attention = Some(format!("MMU error {code:05} (prusa.io/{code:05})"));
            }
            Some(None) if s.attention.as_deref().is_some_and(|a| a.starts_with("MMU error")) => {
                s.attention = None;
            }
            _ => {}
        }
        match reply {
            Reply::Start => *s = ScreenState::default(),
            Reply::Error(text) if fatal(text) => s.fault = Some(text.clone()),
            Reply::Other(_) if fatal(line) => {
                s.fault = Some(line.trim_start_matches("echo:").trim().to_owned())
            }
            Reply::Action(action) => match action {
                Action::PromptBegin(text) => s.pending = Some((text.clone(), Vec::new())),
                Action::PromptButton(label) => {
                    if let Some((_, buttons)) = s.pending.as_mut() {
                        buttons.push(label.clone());
                    }
                }
                Action::PromptShow => s.asking = s.pending.take(),
                Action::PromptEnd => {
                    s.asking = None;
                    s.pending = None;
                }
                Action::Notification(text) => {
                    s.message = Some(text.clone()).filter(|t| !t.is_empty())
                }
                Action::Pause | Action::Paused => s.paused = true,
                Action::Resume => {
                    s.paused = false;
                    s.attention = None;
                }
                Action::Killed => s.fault = Some("Killed: it must be reset".to_owned()),
                Action::Notice(text) => s.message = Some(text.clone()),
                Action::Cancel => {
                    s.cancelled = true;
                    s.paused = false;
                }
                Action::Ready => s.ready_said = true,
                Action::NotReady => {
                    s.ready_said = false;
                    s.not_ready_said = true;
                }
                Action::PowerLoss => s.power_loss = true,
                Action::Reprint => s.reprint_said = true,
                Action::Other(_) => {}
            },
            _ => {}
        }
    }
}

/// The firmware from an `M115` answer: `FIRMWARE_NAME:Prusa-Firmware 3.14.0 based on
/// Marlin ...` gives "Prusa-Firmware 3.14.0", `FIRMWARE_NAME:Marlin 2.1.2 (...) SOURCE_...`
/// "Marlin 2.1.2".
pub fn firmware(lines: &[String]) -> Option<String> {
    let line = lines.iter().find_map(|l| l.split_once("FIRMWARE_NAME:"))?.1;
    let end = [" based on", " SOURCE_CODE_URL", " PROTOCOL_VERSION", " ("]
        .iter()
        .filter_map(|marker| line.find(marker))
        .min()
        .unwrap_or(line.len());
    Some(line[..end].trim().to_owned()).filter(|f| !f.is_empty())
}

/// The machine type from an `M115` answer: `MACHINE_TYPE:Prusa-MINI EXTRUDER_COUNT:1`
/// gives "Prusa-MINI", `MACHINE_TYPE:Prusa i3 MK3S EXTRUDER_COUNT:1` "Prusa i3 MK3S".
pub fn machine_type(lines: &[String]) -> Option<String> {
    let line = lines.iter().find_map(|l| l.split_once("MACHINE_TYPE:"))?.1;
    let end = line.find(" EXTRUDER_COUNT").unwrap_or(line.len());
    Some(line[..end].trim().to_owned()).filter(|t| !t.is_empty())
}

/// Prusa's type code as its `M862.2 Q` gives it, whole (PrusaLink's `PRINTER_TYPE_REGEX`: a
/// line of 3 to 5 digits; 30302 an MK3S with an MMU3).
pub fn type_code(lines: &[String]) -> Option<u32> {
    lines.iter().map(|l| l.trim()).find(|l| (3..=5).contains(&l.len()) && l.chars().all(|c| c.is_ascii_digit()))?.parse().ok()
}

/// Prusa's firmware version as `PRUSA Fir` gives it (PrusaLink's `FW_REGEX`: `3.14.1-8237`).
pub fn prusa_fir(lines: &[String]) -> Option<String> {
    lines.iter().map(|l| l.trim()).find(|l| {
        let (version, build) = match l.split_once('-') {
            Some(parts) => parts,
            None => return false,
        };
        let _ = build;
        let parts: Vec<&str> = version.split('.').collect();
        parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    }).map(str::to_owned)
}

/// PrusaLink's menu in a card listing (`M20 LT`): the top folder named "PrusaLink menu", by
/// its short name in lower case, and whether its `SETREADY.G` is in it already
/// (`sd_card.py` `handle_special_menu`).
pub fn prusalink_menu(lines: &[String]) -> Option<(String, bool)> {
    let mut depth = 0usize;
    let mut menu: Option<(String, bool)> = None;
    let mut inside_menu = false;
    for line in lines.iter().map(|l| l.trim()) {
        if let Some(rest) = line.strip_prefix("DIR_ENTER: ") {
            depth += 1;
            let (sdn, ldn) = rest.split_once(' ').unwrap_or((rest, ""));
            if depth == 1 && ldn.trim_matches('"') == "PrusaLink menu" {
                let short = sdn.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_lowercase();
                menu = Some((short, false));
                inside_menu = true;
            }
        } else if line == "DIR_EXIT" {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                inside_menu = false;
            }
        } else if inside_menu && depth == 1 {
            let name = line.rsplit_once('"').map(|(a, _)| a).and_then(|a| a.rsplit_once('"')).map(|(_, n)| n.to_owned());
            let sfn = line.split_whitespace().next().unwrap_or("").rsplit('/').next().unwrap_or("").to_owned();
            if name.unwrap_or(sfn).eq_ignore_ascii_case("SETREADY.G")
                && let Some(m) = menu.as_mut()
            {
                m.1 = true;
            }
        }
    }
    menu
}

/// Whether a card listing (`M20 LT`) has any file in it: a line between `Begin file list` and
/// `End file list` that is no folder mark (PrusaLink takes files found as a card present).
pub fn lists_files(lines: &[String]) -> bool {
    let mut inside = false;
    lines.iter().map(|l| l.trim()).any(|l| match l {
        "Begin file list" => { inside = true; false }
        "End file list" => { inside = false; false }
        _ => inside && !l.is_empty() && !l.starts_with("DIR_") && l != "ok",
    })
}

/// A Prusa's model code from an `M862.2 Q` answer: 302 for an MK3S, 120 for a MINI. Buddy
/// answers `echo:  M862.2 P120` (`M862_2_3.cpp`); Prusa's MK3 firmware only the number on
/// a line of its own (`302`, as PrusaLink reads it), with an MMU attached ten thousand
/// times its version more (`30302`: an MK3S with an MMU3, `printers.h`): the model is
/// what is left. Other firmware does not answer it.
pub fn model_code(lines: &[String]) -> Option<u32> {
    let code: u32 = match lines.iter().find_map(|l| l.split_once("M862.2 P")) {
        Some((_, rest)) => rest.split_whitespace().next()?.parse().ok()?,
        None => lines.iter().find_map(|l| {
            let l = l.trim();
            (l.len() >= 3 && l.len() <= 5 && l.bytes().all(|b| b.is_ascii_digit()))
                .then(|| l.parse().ok())
                .flatten()
        })?,
    };
    Some(code % 10_000)
}

/// What an MMU says it is doing (Prusa's MK3: `echo:MMU2:Feeding to nozzle`), from the
/// messages its firmware has (PrusaLink's `MMU_PROGRESS_MAP`, the MMU's progress codes).
const MMU_PROGRESS: [&str; 26] = [
    "OK", "Engaging idler", "Disengaging idler", "Unloading to FINDA", "Unloading to pulley",
    "Feeding to FINDA", "Feeding to extruder", "Feeding to nozzle", "Avoiding grind",
    "ERR Disengaging idler", "ERR Engaging idler", "ERR Wait for User", "ERR Internal",
    "ERR Help filament", "ERR TMC failed", "Selecting fil. slot", "Preparing blade",
    "Pushing filament", "Performing cut", "Returning selector", "Ejecting filament",
    "Parking selector", "Retract from FINDA", "Homing", "Moving selector",
    "Feeding to FSensor",
];

pub fn mmu_progress(line: &str) -> Option<&'static str> {
    let rest = line.trim().strip_prefix("echo:MMU2:")?;
    MMU_PROGRESS.iter().copied().find(|m| rest.starts_with(m) && !rest[m.len()..].starts_with(|c: char| c.is_alphanumeric()))
}

/// Its serial number from `PRUSA SN` (Prusa's MK3): `CZPX1234X567XC12345`, or the newer
/// `SN…`; None for "SN invalid" or nothing that looks like one.
pub fn prusa_serial(lines: &[String]) -> Option<String> {
    lines.iter().map(|l| l.trim()).find_map(|line| {
        let old = line.len() == 19
            && line.starts_with("CZPX")
            && line.as_bytes()[8] == b'X'
            && line.as_bytes()[12] == b'X';
        let new = line.starts_with("SN")
            && line.len() == 16
            && line[2..].bytes().all(|b| b.is_ascii_alphanumeric());
        (old || new).then(|| line.to_owned())
    })
}

/// The nozzle's diameter from `M862.1 Q` (Prusa's MK3): a line that is only it, "0.40".
pub fn nozzle(lines: &[String]) -> Option<String> {
    lines.iter().map(|l| l.trim()).find_map(|line| {
        let (whole, part) = line.split_once('.')?;
        (whole.len() == 1
            && whole.bytes().all(|b| b.is_ascii_digit())
            && !part.is_empty()
            && part.bytes().all(|b| b.is_ascii_digit()))
        .then(|| line.to_owned())
    })
}

/// A factor from `M220` or `M221` asked without a value: Prusa's MK3 says "100%",
/// Marlin "FR:100%" (speed) and "echo:E0 Flow: 100%" (flow).
pub fn percent(lines: &[String]) -> Option<f32> {
    lines.iter().map(|l| l.trim()).find_map(|line| {
        let number = line
            .strip_suffix('%')?
            .rsplit(|c: char| c == ':' || c == ' ')
            .next()?;
        number.trim().parse::<f32>().ok()
    })
}

/// The printer's own progress and time left from `M73` asked without values (Prusa's MK3,
/// the slicer's numbers it was given): its mode's line, as (percent, minutes left).
/// "NORMAL MODE: Percent done: 5; print time remaining in mins: 60; Change in mins: -1".
/// With the minutes until the next filament change ("Change in mins"); a negative value is
/// none, as PrusaLink takes it.
pub fn print_info_full(lines: &[String], silent: bool) -> Option<(u32, Option<u32>, Option<u32>)> {
    let mode = if silent { "SILENT MODE:" } else { "NORMAL MODE:" };
    let line = lines.iter().map(|l| l.trim()).find(|l| l.starts_with(mode))?;
    let field = |name: &str| -> Option<i64> {
        let lower = line.to_ascii_lowercase();
        let at = lower.find(name)? + name.len();
        lower[at..].split(';').next()?.trim().parse().ok()
    };
    let percent = u32::try_from(field("percent done:")?).ok()?;
    let left = field("print time remaining in mins:").and_then(|m| u32::try_from(m).ok());
    let change = field("change in mins:").and_then(|m| u32::try_from(m).ok());
    Some((percent, left, change))
}

/// A little-endian `uint32` from Prusa's MK3 EEPROM (an AVR): its lifetime filament
/// (metres, `EEPROM_FILAMENTUSED`) and printing time (minutes, `EEPROM_TOTALTIME`).
pub fn eeprom_u32(bytes: &[u8]) -> Option<u32> {
    let b: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    let value = u32::from_le_bytes(b);
    // An EEPROM never written says 0xFFFFFFFF.
    (value != u32::MAX).then_some(value)
}

/// The bytes of a `D3` EEPROM read (Prusa's MK3): lines "0D49  53 6D 6F 6F 74 68 31 …",
/// an address and up to 16 bytes in hex.
pub fn eeprom_bytes(lines: &[String]) -> Vec<u8> {
    lines
        .iter()
        .filter_map(|line| {
            let (address, data) = line.trim().split_once("  ")?;
            (address.len() >= 2 && address.bytes().all(|b| b.is_ascii_hexdigit())).then_some(data)
        })
        .flat_map(|data| data.split_whitespace().filter_map(|b| u8::from_str_radix(b, 16).ok()).collect::<Vec<_>>())
        .collect()
}

/// The sheet in use from Prusa's MK3 EEPROM: eight sheets of 11 bytes (a name of up to 7,
/// its Z offset, two temperatures), then the active sheet's number (`eeprom.h`, `Sheets`).
pub fn active_sheet(sheets: &[u8], active: u8) -> Option<String> {
    let at = active as usize * 11;
    let name = sheets.get(at..at + 7)?;
    let name: String = name
        .iter()
        .take_while(|b| **b != 0 && **b != 0xFF)
        .map(|b| *b as char)
        .collect();
    let name = name.trim().to_owned();
    (!name.is_empty() && name.chars().all(|c| c.is_ascii_graphic() || c == ' ')).then_some(name)
}

/// Prusa's MK3 print mode from its EEPROM (`EEPROM_SILENT`): 0 normal, 1 silent, 2 auto
/// (PrusaLink's `PRINT_MODE_ID_PAIRING`); else not known (an unset EEPROM says 0xFF).
pub fn print_mode(byte: u8) -> Option<&'static str> {
    match byte {
        0 => Some("normal"),
        1 => Some("silent"),
        2 => Some("auto"),
        _ => None,
    }
}

/// An MMU's firmware from `M707 A0x00`…`A0x03` (major, minor, revision, build), each
/// answered `echo:MMU2:<R0 A3*xx.` in hex: "3.0.3+896".
pub fn mmu_firmware(answers: [&[String]; 4]) -> Option<String> {
    let number = |lines: &[String], register: u8| -> Option<u32> {
        let prefix = [format!("echo:MMU2:<R{register} A"), format!("echo:MMU3:<R{register} A")];
        lines.iter().map(|l| l.trim()).find_map(|line| {
            let rest = prefix.iter().find_map(|p| line.strip_prefix(p.as_str()))?;
            u32::from_str_radix(rest.split('*').next()?, 16).ok()
        })
    };
    let [major, minor, revision, build] = answers;
    Some(format!(
        "{}.{}.{}+{}",
        number(major, 0)?,
        number(minor, 1)?,
        number(revision, 2)?,
        number(build, 3)?
    ))
}

/// The heaters from an `M105` answer, or a temperature report (`M155`, the `ok`s while
/// heating): `ok T:210.3 /210.0 B:60.1 /60.0 T0:210.3 /210.0 @:64 B@:0`. After the `/` is
/// what it heats to (0: off); `@:` and `B@:` are the heaters' power (0: no power now).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Heat {
    pub nozzle_c: Option<f32>,
    pub nozzle_target_c: Option<f32>,
    pub bed_c: Option<f32>,
    pub bed_target_c: Option<f32>,
    pub nozzle_heating: Option<bool>,
    pub bed_heating: Option<bool>,
    /// Fan speeds, from Prusa's MK3 firmware (in its `M105` answer and its fan auto-report,
    /// `M155 C2`): `E0:3500 RPM PRN1:0 RPM E0@:255 PRN1@:0`, the hotend fan and the print fan.
    pub hotend_fan_rpm: Option<u32>,
    pub print_fan_rpm: Option<u32>,
    /// Where the head is, from `M114` or the position auto-report (`M155 C4`):
    /// `X:10.00 Y:20.00 Z:0.15 E:0.00 Count X: 800 Y:1600 Z:60`, the part before `Count`.
    pub position_mm: Option<[f32; 3]>,
}

/// The head's position in a line of `M114` (or its auto-report): X, Y and Z before `Count`
/// (after it come the steppers' counts, not millimetres).
fn position(line: &str) -> Option<[f32; 3]> {
    let line = line.trim().strip_prefix("ok ").unwrap_or(line.trim());
    if !line.starts_with("X:") {
        return None;
    }
    let before = line.split(" Count").next()?;
    let value = |axis: &str| {
        before.split_whitespace().find_map(|word| word.strip_prefix(axis)?.parse::<f32>().ok())
    };
    Some([value("X:")?, value("Y:")?, value("Z:")?])
}

/// Each value from the newest line that has it: with `M155` on, an answer can hold several
/// of the printer's own reports, oldest first.
pub fn heat(lines: &[String]) -> Heat {
    // `E0:3500 RPM` and `PRN1:0 RPM` (not `E0@:` or `E:` alone, the heating lines' extruder).
    let rpm = |key: &str| {
        lines.iter().rev().find_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            words.iter().enumerate().find_map(|(i, word)| {
                let value = word.strip_prefix(key)?;
                (words.get(i + 1) == Some(&"RPM")).then(|| value.parse::<u32>().ok())?
            })
        })
    };
    // `T:210.3 /210.0`, or written together: `T:210.3/210.0`.
    let reading = |key: &str| {
        lines.iter().rev().find_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            words.iter().enumerate().find_map(|(i, word)| {
                let rest = word.strip_prefix(key)?;
                let (now, target) = match rest.split_once('/') {
                    Some((now, target)) => (now, Some(target)),
                    None => (rest, words.get(i + 1).and_then(|w| w.strip_prefix('/'))),
                };
                let now: f32 = now.parse().ok()?;
                Some((now, target.and_then(|t| t.parse::<f32>().ok())))
            })
        })
    };
    let power = |key: &str| {
        lines.iter().rev().find_map(|line| {
            line.split_whitespace()
                .find_map(|word| word.strip_prefix(key))
                .and_then(|v| v.parse::<f32>().ok())
                .map(|p| p > 0.0)
        })
    };
    let nozzle = reading("T:");
    let bed = reading("B:");
    Heat {
        nozzle_c: nozzle.map(|n| n.0),
        nozzle_target_c: nozzle.and_then(|n| n.1),
        bed_c: bed.map(|b| b.0),
        bed_target_c: bed.and_then(|b| b.1),
        nozzle_heating: power("@:"),
        bed_heating: power("B@:"),
        hotend_fan_rpm: rpm("E0:"),
        print_fan_rpm: rpm("PRN1:"),
        position_mm: lines.iter().rev().find_map(|line| position(line)),
    }
}

/// What happened while sending, for the caller to show.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The printer said something worth showing (an error, a message, temperatures).
    Said(String),
    /// A line was acknowledged: how many of the file's code lines are done.
    Progress { done: usize, total: usize },
    /// The printer asked for a line again.
    Resent(u32),
    /// An `Error:` about the print, not about the line on the wire: a job stops on it, as
    /// OctoPrint's does.
    Error(String),
}

/// A bed mesh in what a printer said, after a header that announces one: Prusa's `G81`
/// ("Measured points:", the back row first, no row numbers), Marlin's bilinear and mesh
/// levelling (`M420 V`: "Bilinear Leveling Grid:", "Mesh Bed Level data:", a header row of
/// column numbers, then each row after its number) and UBL ("Bed Topography Report", rows
/// numbered, `|` between, `.` where a point was not measured). Rows with numbers are put in
/// order by them (row 0 is the front); rows without, as Prusa prints them, are turned round.
pub fn mesh(lines: &[String]) -> Option<maker_connect_protocol::Mesh> {
    const HEADERS: [&str; 4] = [
        "Measured points:",
        "Bilinear Leveling Grid:",
        "Mesh Bed Level data:",
        "Bed Topography Report",
    ];
    let start = lines
        .iter()
        .rposition(|l| HEADERS.iter().any(|h| l.contains(h)))?;
    let mut numbered: Vec<(Option<usize>, Vec<Option<f32>>)> = Vec::new();
    for line in &lines[start + 1..] {
        let cleaned = line.replace(['|', '[', ']', ','], " ");
        let words: Vec<&str> = cleaned.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        let value = |w: &str| -> Option<Option<f32>> {
            match w {
                "." | "nan" | "NAN" => Some(None),
                _ if w.contains('.') => w.trim_start_matches('+').parse::<f32>().ok().map(Some),
                _ => None,
            }
        };
        // A row number first (no decimal point), then the values.
        let (index, rest) = match words[0].parse::<usize>() {
            Ok(n) if words.len() > 1 && value(words[1]).is_some() => (Some(n), &words[1..]),
            _ => (None, &words[..]),
        };
        let values: Option<Vec<Option<f32>>> = rest.iter().map(|w| value(w)).collect();
        match values {
            Some(values) if values.len() >= 2 => numbered.push((index, values)),
            // A header row of column numbers, or "(0,4)" corners: not data.
            _ if numbered.is_empty() => continue,
            // The first line after the rows ends them.
            _ => break,
        }
    }
    if numbered.len() < 2 {
        return None;
    }
    let width = numbered[0].1.len();
    if numbered.iter().any(|(_, row)| row.len() != width) {
        return None;
    }
    let rows = if numbered.iter().all(|(i, _)| i.is_some()) {
        numbered.sort_by_key(|(i, _)| *i);
        numbered.into_iter().map(|(_, row)| row).collect()
    } else {
        numbered.into_iter().rev().map(|(_, row)| row).collect()
    };
    Some(maker_connect_protocol::Mesh { rows })
}

/// A connection to a printer: anything that reads and writes bytes, with a read timeout
/// (a serial port; a fake printer in the tests).
pub struct Printer<L: Read + Write> {
    reader: BufReader<L>,
    next_number: u32,
    /// The last lines sent, by number, for a resend.
    sent: VecDeque<(u32, String)>,
    /// What is on its screen (waiting for its knob, a question, an error, paused or stopped
    /// at its knob): shared, so whoever reports meanwhile can say so.
    waiting: Arc<Screen>,
    /// Lines sent that have no `ok` yet.
    unacked: usize,
    /// The line last asked for again, and how often in a row.
    same_resend: (u32, usize),
    /// What it can do, from its `M115` answer.
    pub caps: Capabilities,
    /// Its firmware, from its `M115` answer.
    pub firmware: Option<String>,
    /// Set from elsewhere to break off a heat-up (`M109`, `M190`) at once, where the printer
    /// can (its emergency parser).
    pub interrupt: Arc<AtomicBool>,
    /// How long it may stay silent while an `ok` is due ([`SILENT_TIMEOUT`],
    /// [`LONG_SILENCE`]; shorter in tests), and how long the `ok` after a resend request may
    /// take ([`RESEND_OK_WAIT`]).
    silence: (Duration, Duration, Duration, Duration),
    /// When `M79` was last sent (Prusa's MK3 firmware: a host is there).
    last_m79: Option<Instant>,
    /// A print from its own SD card, as its last `M27` said.
    pub sd: Option<SdStatus>,
    /// What was last asked of its SD print (pause, resume, stop) and when, until its `M27`
    /// shows it was done; not within 15 s is said (PrusaLink's `STATE_CHANGE_TIMEOUT`).
    pub sd_asked: Option<(&'static str, Instant)>,
    /// The numbers of recent lines that were not a job's file's own (`M79`, an `M105` asking
    /// after it, an answer): a job's checkpoint keeps them (crate::recovery).
    pub extras: Vec<u32>,
    /// The next line sent is a job's file's own.
    file_line: bool,
    /// A line asked for again that was never sent (`Resend: N` beyond the last): after a
    /// power cut Prusa's MK3 asks for the line after the one it saved this way.
    stray_resend: Option<u32>,
    /// Set from elsewhere: the button pressed on the website for the question on its screen
    /// (Marlin's host prompt), sent as `M876 S<index>` at once, also while a line waits (its
    /// emergency parser takes it then).
    pub answer: Arc<Mutex<Option<u8>>>,
}

/// How often Prusa's MK3 firmware is told a host is there (`M79`; it forgets after 30 s,
/// `Configuration_adv.h`; PrusaLink says it every 12 s).
const HOST_KEEPALIVE: Duration = Duration::from_secs(12);

impl<L: Read + Write> Printer<L> {
    pub fn new(link: L) -> Self {
        Self {
            reader: BufReader::new(link),
            next_number: 1,
            sent: VecDeque::new(),
            waiting: Arc::default(),
            unacked: 0,
            same_resend: (0, 0),
            caps: Capabilities::default(),
            firmware: None,
            interrupt: Arc::default(),
            silence: (SILENT_TIMEOUT, LONG_SILENCE, RESEND_OK_WAIT, PRUSALINK_TIMEOUT),
            last_m79: None,
            answer: Arc::default(),
            sd: None,
            sd_asked: None,
            extras: Vec::new(),
            file_line: false,
            stray_resend: None,
        }
    }

    /// The number the next line goes with.
    pub fn next_number(&self) -> u32 {
        self.next_number
    }

    /// Goes on with this line number (a print taken up after a power cut: the printer set its
    /// own); what was sent before is forgotten.
    pub fn set_next_number(&mut self, number: u32) {
        self.next_number = number;
        self.sent.clear();
        self.unacked = 0;
        self.same_resend = (0, 0);
    }

    /// A line asked for that was never sent, once.
    pub fn take_stray_resend(&mut self) -> Option<u32> {
        self.stray_resend.take()
    }

    /// Sends one of a job's file's own lines (its number is not kept among `extras`).
    pub fn send_file_line(&mut self, code: &str, on: &mut impl FnMut(Event)) -> io::Result<String> {
        self.file_line = true;
        self.send_line(code, on)
    }

    /// Whether it has an SD card to print from: Prusa's MK3, or Marlin that says so
    /// (`Cap:SDCARD:1`).
    pub fn has_sd(&self) -> bool {
        self.is_prusa_mk3() || self.caps.all.iter().any(|(name, on)| name == "SDCARD" && *on)
    }

    /// Asks how a print from its card goes (`M27`; Marlin `M27 C` for the file's name), and
    /// keeps it in `sd`.
    pub fn follow_sd(&mut self) -> io::Result<Option<SdStatus>> {
        // Prusa's MK3 as PrusaLink asks (`printer_polling.py`): `M27 P`, the file's short path.
        let ask = if self.is_prusa_mk3() { "M27 P" } else { "M27 C" };
        let answer = self.command(ask)?;
        if let Some(status) = sd_status(&answer) {
            // Marlin forgets nothing between asks, the MK3 gives no file while paused: what
            // was known stays.
            let known = self.sd.take();
            self.sd = status.map(|mut s| {
                if let Some(k) = known {
                    s.file = s.file.or(k.file);
                    if s.total == 0 {
                        (s.done, s.total, s.elapsed_s) = (k.done, k.total, k.elapsed_s);
                    }
                }
                s
            });
        }
        if self.sd.is_some() {
            self.waiting.sd_start_seen();
        }
        Ok(self.sd.clone())
    }

    /// A print from its card that said it is done (`Done printing file`): its last report,
    /// the whole file done; it is over from now on (PrusaLink: FINISHED, at once).
    pub fn sd_finished(&mut self) -> Option<SdStatus> {
        if !self.waiting.take_sd_done() {
            return None;
        }
        self.sd.take().map(|mut s| {
            s.done = s.total;
            s
        })
    }

    /// Pause, resume or stop the print from its card: Prusa's MK3 parks with M601, goes on
    /// with M602 and stops with M603 (as PrusaLink does); Marlin M25, M24, M524.
    pub fn control_sd(&mut self, what: &str, on: &mut impl FnMut(Event)) -> io::Result<()> {
        self.sd_asked = match what {
            "pause" => Some(("pause", Instant::now())),
            "resume" => Some(("resume", Instant::now())),
            "stop" => Some(("stop", Instant::now())),
            _ => None,
        };
        let code = match (self.is_prusa_mk3(), what) {
            (true, "pause") => "M601",
            (true, "resume") => "M602",
            (true, "stop") => "M603",
            (false, "pause") => "M25",
            (false, "resume") => "M24",
            (false, "stop") => "M524",
            _ => return Ok(()),
        };
        self.send_line(code, on).map(|_| ())
    }

    /// Whether what was asked of its SD print was done, as its last `M27` shows: None while
    /// it is still in time or nothing was asked; else what it did not do within 15 s (once).
    pub fn sd_not_done(&mut self) -> Option<String> {
        let (what, at) = self.sd_asked?;
        let done = match what {
            "pause" => self.sd.as_ref().is_some_and(|s| s.paused),
            "resume" => self.sd.as_ref().is_some_and(|s| !s.paused),
            _ => self.sd.is_none(),
        };
        if done {
            self.sd_asked = None;
            return None;
        }
        if at.elapsed() < Duration::from_secs(15) {
            return None;
        }
        self.sd_asked = None;
        Some(format!("it did not {what} within 15 s"))
    }

    /// Sends someone's answer to its question now, if there is one and no line is waiting
    /// (idle, or a job paused for the question).
    pub fn send_answer(&mut self, on: &mut impl FnMut(Event)) -> io::Result<()> {
        let pending = self.answer.lock().unwrap().take();
        match pending {
            Some(button) => self.send_line(&format!("M876 S{button}"), on).map(|_| ()),
            None => Ok(()),
        }
    }

    /// Tells Prusa's MK3 firmware a host is there, every 12 s (`M79`): only then does it
    /// offer "Set Ready" in its menu, finish a host's print on its screen and offer to resume
    /// a host's print after a power cut. `MC` (Maker Connect) shows on its status line.
    /// Nothing for other firmware.
    pub fn keep_host_alive(&mut self, on: &mut impl FnMut(Event)) -> io::Result<()> {
        if !self.is_prusa_mk3() || self.last_m79.is_some_and(|t| t.elapsed() < HOST_KEEPALIVE) {
            return Ok(());
        }
        self.last_m79 = Some(Instant::now());
        // PrusaLink's keepalive (`keepalive.py`): "PC" when it works for Prusa Connect, as the
        // agent works for its hub.
        self.send_line("M79 S\"PC\"", on).map(|_| ())
    }

    /// Shorter waits, for tests with a fake printer.
    #[cfg(test)]
    pub(crate) fn hurry(&mut self) {
        self.silence = (
            Duration::from_millis(150),
            Duration::from_millis(600),
            Duration::from_millis(50),
            Duration::from_millis(150),
        );
    }

    /// Prusa's MK3 firmware (not Buddy): pauses, resumes and stops with `M601`, `M602`,
    /// `M603`, which park the head.
    pub fn is_prusa_mk3(&self) -> bool {
        self.firmware
            .as_deref()
            .is_some_and(|f| f.starts_with("Prusa-Firmware ") || f == "Prusa-Firmware")
    }

    /// The lines sent and not acknowledged yet.
    fn in_flight(&self) -> impl Iterator<Item = &str> {
        let skip = self.sent.len().saturating_sub(self.unacked);
        self.sent.iter().skip(skip).map(|(_, code)| code.as_str())
    }

    /// What is on its screen, readable from another thread.
    pub fn waiting(&self) -> Arc<Screen> {
        self.waiting.clone()
    }

    /// Shares a screen made before the printer was opened (to report while it connects).
    pub fn set_waiting(&mut self, waiting: Arc<Screen>) {
        self.waiting = waiting;
    }

    /// One line from the printer, or None when it stayed silent for the read timeout.
    fn read_line(&mut self) -> io::Result<Option<String>> {
        // Every byte read as a character (PrusaLink: `decode_line`, cp437, NULs dropped): a
        // stray byte while it boots spoils no read.
        let mut bytes = Vec::new();
        match self.reader.read_until(b'\n', &mut bytes) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the printer closed the connection",
            )),
            Ok(_) => Ok(Some(String::from_utf8_lossy(&bytes).replace('\0', "").trim().to_owned())),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn write_line(&mut self, text: &str) -> io::Result<()> {
        let link = self.reader.get_mut();
        link.write_all(text.as_bytes())?;
        link.write_all(b"\n")?;
        link.flush()
    }

    /// After opening the port: waits for the printer to boot (`start`, then quiet), and
    /// sets its line numbers back to zero (`M110 N0`).
    pub fn connect(&mut self, on: &mut impl FnMut(Event)) -> io::Result<()> {
        self.wait_for_boot(on)?;
        self.number_from_zero(on)
    }

    /// Connects as PrusaLink does (`serial_adapter.py`): after its boot, it is asked what it
    /// is with `PRUSA Fir` and `M862.2 Q`, unnumbered, in one write (`_get_info`), and only
    /// then numbered from zero. On a port PrusaLink does not take for a Prusa, `PRUSA Fir`
    /// alone first, and `M862.2 Q` only when it answered (in Marlin `M862` is the position
    /// encoder's test). Its firmware version and type code, where it gave them.
    pub fn connect_as_prusalink(
        &mut self,
        prusa_port: bool,
        on: &mut impl FnMut(Event),
    ) -> io::Result<(Option<String>, Option<u32>)> {
        self.wait_for_boot(on)?;
        let found = if prusa_port {
            let said = self.ask_unnumbered("PRUSA Fir\nM862.2 Q", 2)?;
            (prusa_fir(&said), type_code(&said))
        } else {
            let version = prusa_fir(&self.ask_unnumbered("PRUSA Fir", 1)?);
            let code = match version {
                Some(_) => type_code(&self.ask_unnumbered("M862.2 Q", 1)?),
                None => None,
            };
            (version, code)
        };
        self.number_from_zero(on)?;
        Ok(found)
    }

    /// Writes `text` as it is (no numbers) and reads what comes until its `oks` or for
    /// 5 s (PrusaLink's detection waits 5 s).
    fn ask_unnumbered(&mut self, text: &str, oks: usize) -> io::Result<Vec<String>> {
        self.write_line(text)?;
        let until = Instant::now() + self.silence.3.min(Duration::from_secs(5)).max(Duration::from_millis(100));
        let (mut seen, mut said) = (0, Vec::new());
        while seen < oks && Instant::now() < until {
            if let Some(line) = self.read_line()?.filter(|l| !l.is_empty()) {
                let reply = parse(&line);
                self.waiting.heard(&reply, &line);
                match reply {
                    Reply::Ok => seen += 1,
                    _ => said.push(line),
                }
            }
        }
        Ok(said)
    }

    /// Waits for the printer to boot (`start`, then quiet).
    fn wait_for_boot(&mut self, on: &mut impl FnMut(Event)) -> io::Result<()> {
        let deadline = Instant::now() + BOOT_WAIT;
        let mut quiet_since = Instant::now();
        while Instant::now() < deadline {
            match self.read_line()? {
                Some(line) if !line.is_empty() => {
                    quiet_since = Instant::now();
                    self.waiting.heard(&parse(&line), &line);
                    if !matches!(parse(&line), Reply::Ok) {
                        on(Event::Said(line));
                    }
                }
                // A printer that does not reset, or has finished booting, goes quiet.
                _ if quiet_since.elapsed() > Duration::from_secs(2) => break,
                _ => {}
            }
        }
        Ok(())
    }

    /// Its line numbers from zero (`M110 N0`), its buffers emptied first (`M400`).
    fn number_from_zero(&mut self, on: &mut impl FnMut(Event)) -> io::Result<()> {
        self.next_number = 0;
        self.sent.clear();
        self.unacked = 0;
        self.same_resend = (0, 0);
        // As PrusaLink does before an M110 (`m110_workaround_slot`): the firmware's buffers
        // run empty first (M400, without a number: the numbering is set only after it).
        self.write_line("M400")?;
        self.unacked += 1;
        self.wait_ok(on)?;
        self.send_numbered("M110 N0")?;
        self.wait_ok(on).map(|_| ())
    }

    /// Only listens, for `for_`: what the printer says by itself (its reports, the end of a
    /// calibration), with nothing sent. While Prusa's MK3 calibrates its thermal model
    /// nothing may be sent (PrusaLink blocks its queue then).
    pub fn listen(&mut self, for_: Duration) -> io::Result<Vec<String>> {
        let until = Instant::now() + for_;
        let mut lines = Vec::new();
        while Instant::now() < until {
            if let Some(line) = self.read_line()?.filter(|l| !l.is_empty()) {
                self.waiting.heard(&parse(&line), &line);
                lines.push(line);
            }
        }
        Ok(lines)
    }

    /// Sends one command and returns what the printer answered: its lines before the
    /// `ok`, and the `ok` line itself. An `M115` answer also gives its firmware and what it
    /// can do (`caps`).
    pub fn command(&mut self, code: &str) -> io::Result<Vec<String>> {
        self.send_numbered(code)?;
        let mut lines = Vec::new();
        let ok = self.wait_ok(&mut |event| {
            if let Event::Said(text) = event {
                lines.push(text);
            }
        })?;
        lines.push(ok);
        if code.trim().eq_ignore_ascii_case("M115") {
            let caps = capabilities(&lines);
            if !caps.all.is_empty() {
                self.caps = caps;
            }
            self.firmware = firmware(&lines).or(self.firmware.take());
        }
        Ok(lines)
    }

    fn send_numbered(&mut self, code: &str) -> io::Result<()> {
        let number = self.next_number;
        let line = numbered(number, code);
        self.write_line(&line)?;
        self.sent.push_back((number, code.to_owned()));
        if self.sent.len() > KEPT_LINES {
            self.sent.pop_front();
        }
        if !std::mem::take(&mut self.file_line) {
            self.extras.push(number);
            let excess = self.extras.len().saturating_sub(256);
            self.extras.drain(..excess);
        }
        self.next_number += 1;
        self.unacked += 1;
        Ok(())
    }

    /// Asks after a printer that stays silent while an `ok` is due: a numbered `M105`.
    /// The printer works in order, so its `ok` means every line before it was done too: from
    /// then on that one `ok` is waited for. Was a line lost instead, the printer asks for
    /// it again (a resend), and the line and the `M105` go again.
    fn poke(&mut self) -> io::Result<()> {
        self.send_numbered("M105")?;
        self.unacked = 1;
        Ok(())
    }

    /// Sends line `from` and every line after it again, as the printer asked (it threw
    /// away whatever came after the line it missed). A line not sent yet: the last line
    /// arrived twice, nothing to send.
    fn resend_from(&mut self, from: u32, on: &mut impl FnMut(Event)) -> io::Result<()> {
        if from >= self.next_number {
            // Beyond the last line sent: kept (after a power cut Prusa's MK3 asks this way
            // for the line after the one it saved), and otherwise nothing to send.
            if from > self.next_number {
                self.stray_resend = Some(from);
            }
            return Ok(());
        }
        if self.same_resend.0 == from {
            self.same_resend.1 += 1;
            if self.same_resend.1 >= MAX_SAME_RESENDS {
                return Err(io::Error::other(format!(
                    "the printer keeps asking for line {from} again: the connection is broken"
                )));
            }
        } else {
            self.same_resend = (from, 0);
        }
        let again: Vec<(u32, String)> = self
            .sent
            .iter()
            .filter(|(n, _)| *n >= from)
            .cloned()
            .collect();
        if again.first().map(|(n, _)| *n) != Some(from) {
            return Err(io::Error::other(format!(
                "the printer asks for line {from}, which is no longer kept"
            )));
        }
        on(Event::Resent(from));
        for (number, code) in &again {
            self.write_line(&numbered(*number, code))?;
        }
        self.unacked = again.len();
        Ok(())
    }

    /// Waits until every line sent has its `ok`, and returns the last `ok` line (after an
    /// `M105` it carries the temperatures). Along the way, as OctoPrint and PrusaLink do:
    ///
    /// - `Resend: N` is followed by an `ok` that belongs to the request; at that `ok` line N
    ///   and those after it go again. When that `ok` does not come (a firmware bug), they go
    ///   after a second anyway. The same line asked for again and again: broken.
    /// - Silence (or `wait`) while an `ok` is due: a numbered `M105` asks after it. Its
    ///   `ok` settles an `ok` that was lost; a line that was lost comes back as a resend.
    ///   After a few: it stopped answering. Busy messages, temperatures while it heats and
    ///   anything else it says count as an answer; a command that takes long gets longer.
    /// - `interrupt`: an `M108` breaks off a heat-up at once, where the printer has the
    ///   emergency parser (numbered, so its `ok` is counted like any other).
    fn wait_ok(&mut self, on: &mut impl FnMut(Event)) -> io::Result<String> {
        let mut last_heard = Instant::now();
        // A resend asked for (the line), and when; done at the `ok` that follows.
        let mut resend: Option<(u32, Instant)> = None;
        let mut pokes = 0;
        loop {
            if self.interrupt.swap(false, Ordering::Relaxed)
                && self.caps.emergency_parser
                && self.in_flight().any(heats)
            {
                on(Event::Said("Breaking off the heat-up (M108)".to_owned()));
                self.send_numbered("M108")?;
            }
            // Someone's answer to its question, numbered like any line (its `ok` is
            // counted): the emergency parser acts on it while this line still waits.
            let pending = self.answer.lock().unwrap().take();
            if let Some(button) = pending {
                on(Event::Said(format!("Answering its question: button {button} (M876)")));
                self.send_numbered(&format!("M876 S{button}"))?;
            }
            let Some(line) = self.read_line()? else {
                if let Some((from, at)) = resend
                    && at.elapsed() > self.silence.2
                {
                    resend = None;
                    self.resend_from(from, on)?;
                    last_heard = Instant::now();
                    continue;
                }
                // Prusa's MK3, as PrusaLink (serial_queue.py `check_status`): 25 s without a
                // sign of life and the line waiting goes again, with its own number (it
                // "forgot" it); the third time the port is given up.
                if self.is_prusa_mk3() {
                    if last_heard.elapsed() > self.silence.3 {
                        pokes += 1;
                        if pokes > 2 {
                            return Err(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "the printer stopped answering",
                            ));
                        }
                        on(Event::Said("No answer from the printer: the line again".to_owned()));
                        let current = self.next_number.saturating_sub(1);
                        self.resend_from(current, on)?;
                        last_heard = Instant::now();
                    }
                    continue;
                }
                let limit = if self.in_flight().any(long_running) {
                    self.silence.1
                } else {
                    self.silence.0
                };
                if last_heard.elapsed() > limit {
                    if pokes >= MAX_POKES {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "the printer stopped answering",
                        ));
                    }
                    pokes += 1;
                    on(Event::Said("No answer from the printer: asking after it (M105)".to_owned()));
                    self.poke()?;
                    last_heard = Instant::now();
                }
                continue;
            };
            let reply = parse(&line);
            // A sign of life: any line; on Prusa's MK3 only what PrusaLink takes as one (an
            // `ok`, `busy: processing`, `busy: paused for user`, a heating report).
            if !self.is_prusa_mk3()
                || matches!(reply, Reply::Ok | Reply::Busy | Reply::PausedForUser)
                || prusalink_heating(&line)
            {
                last_heard = Instant::now();
                if matches!(reply, Reply::Ok | Reply::Busy | Reply::PausedForUser) {
                    pokes = 0;
                }
            }
            self.waiting.heard(&reply, &line);
            // Anything but an `ok` after a resend request: its `ok` is not coming.
            if !matches!(reply, Reply::Ok | Reply::Resend(_))
                && let Some((from, _)) = resend.take()
            {
                self.resend_from(from, on)?;
            }
            match reply {
                Reply::Ok => {
                    if let Some((from, _)) = resend.take() {
                        self.resend_from(from, on)?;
                        continue;
                    }
                    self.unacked = self.unacked.saturating_sub(1);
                    if self.unacked == 0 {
                        self.same_resend = (0, 0);
                        return Ok(line);
                    }
                }
                Reply::Resend(number) => resend = Some((number, Instant::now())),
                Reply::Wait if self.is_prusa_mk3() => {}
                Reply::Wait => {
                    // Idle while an `ok` is due: ask after it now, rather than in 30 s.
                    if pokes < MAX_POKES {
                        pokes += 1;
                        self.poke()?;
                    }
                }
                Reply::Busy | Reply::PausedForUser => {}
                Reply::Start => return Err(io::Error::other("the printer restarted")),
                Reply::Error(text) => {
                    on(Event::Said(format!("Error: {text}")));
                    if !harmless_error(&text) {
                        on(Event::Error(text));
                    }
                }
                Reply::Action(_) => on(Event::Said(line.clone())),
                Reply::Other(text) if !text.is_empty() => on(Event::Said(text)),
                Reply::Other(_) => {}
            }
        }
    }

    /// Sends one line of a job and waits for its `ok`, which it returns (it carries the
    /// temperatures on most printers).
    pub fn send_line(&mut self, code: &str, on: &mut impl FnMut(Event)) -> io::Result<String> {
        self.send_numbered(code)?;
        self.wait_ok(on)
    }

    /// Sends G-code, line by line, each acknowledged before the next.
    pub fn send<'a>(
        &mut self,
        lines: impl IntoIterator<Item = &'a str>,
        on: &mut impl FnMut(Event),
    ) -> io::Result<()> {
        let codes: Vec<&str> = lines.into_iter().filter_map(clean).collect();
        let total = codes.len();
        for (done, code) in codes.into_iter().enumerate() {
            self.send_numbered(code)?;
            self.wait_ok(on)?;
            on(Event::Progress {
                done: done + 1,
                total,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn said(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    /// What is on its screen, line by line: a question with its buttons until it ends, a
    /// fatal error (not a line sent wrong), paused and stopped at its knob, and a restart
    /// wiping it all.
    #[test]
    fn a_screen_follows_what_the_printer_says() {
        let screen = Screen::default();
        let hear = |line: &str| screen.heard(&parse(line), line);
        assert_eq!(
            parse("//action:prompt_begin Filament runout"),
            Reply::Action(Action::PromptBegin("Filament runout".into()))
        );
        hear("//action:prompt_begin Filament runout");
        hear("//action:prompt_button Continue");
        hear("//action:prompt_button Purge more");
        assert_eq!(screen.now().asking, None, "not shown yet");
        hear("//action:prompt_show");
        assert_eq!(
            screen.now().asking,
            Some((
                "Filament runout".to_owned(),
                vec!["Continue".to_owned(), "Purge more".to_owned()]
            ))
        );
        hear("//action:prompt_end");
        assert_eq!(screen.now().asking, None);
        // A line sent wrong is no fault; a thermal runaway is.
        hear("Error:Line Number is not Last Line Number+1, Last Line: 12");
        assert_eq!(screen.now().fault, None);
        hear("Error:Thermal Runaway, system stopped! Heater_ID: 0");
        assert_eq!(
            screen.now().fault.as_deref(),
            Some("Thermal Runaway, system stopped! Heater_ID: 0")
        );
        hear("echo:busy: paused for user");
        assert!(screen.waiting.load(Ordering::Relaxed));
        hear("ok");
        assert!(!screen.waiting.load(Ordering::Relaxed));
        // Prusa's MK3 and older: paused, resumed and stopped from its knob.
        hear("//action:pause");
        assert!(screen.now().paused);
        hear("//action:resume");
        assert!(!screen.now().paused);
        hear("//action:cancel");
        assert!(screen.now().cancelled);
        // "Set Ready" in its menu: once, for whoever reports.
        hear("//action:ready");
        assert!(screen.take_ready() && !screen.take_ready());
        hear("//action:uvlo_recovery_ready");
        assert!(screen.now().power_loss);
        hear("//action:shutdown");
        assert_eq!(
            parse("//action:shutdown"),
            Reply::Action(Action::Other("shutdown".into())),
            "never acted on"
        );
        hear("start");
        assert_eq!(screen.now(), ScreenState::default(), "restarted: all gone");
    }

    /// Prusa's G81 prints the back row first; the mesh starts at the front.
    /// What PrusaLink reads of Prusa's MK3 (its `regular_expressions.py`), read here too.
    #[test]
    fn what_prusalink_reads_is_read() {
        let s = |t: &str| vec![t.to_owned(), "ok".to_owned()];
        // PRUSA SN: both forms; "SN invalid" is none.
        assert_eq!(prusa_serial(&s("CZPX1234X567XC12345")).as_deref(), Some("CZPX1234X567XC12345"));
        assert_eq!(prusa_serial(&s("SN25017C00123456")).as_deref(), Some("SN25017C00123456"));
        assert_eq!(prusa_serial(&s("SN invalid")), None);
        // M862.1 Q: the nozzle.
        assert_eq!(nozzle(&s("0.40")).as_deref(), Some("0.40"));
        assert_eq!(nozzle(&s("ok")), None);
        // M220, M221 asked back: the MK3's "95%", Marlin's "FR:100%" and "Flow: 98%".
        assert_eq!(percent(&s("95%")), Some(95.0));
        assert_eq!(percent(&s("FR:100%")), Some(100.0));
        assert_eq!(percent(&s("echo:E0 Flow: 98%")), Some(98.0));
        // M73 asked back: the printer's own progress, in its mode.
        let info = vec![
            "NORMAL MODE: Percent done: 25; print time remaining in mins: 40; Change in mins: -1".to_owned(),
            "SILENT MODE: Percent done: 24; print time remaining in mins: 42; Change in mins: -1".to_owned(),
        ];
        assert_eq!(print_info_full(&info, false), Some((25, Some(40), None)));
        assert_eq!(print_info_full(&info, true), Some((24, Some(42), None)));
        // D3: the EEPROM's bytes; the sheets and the one in use; the print mode.
        let mut sheets = vec![0xFFu8; 88];
        sheets[..7].copy_from_slice(b"Smooth1");
        sheets[11..18].copy_from_slice(b"Satin\0\0");
        let lines: Vec<String> = sheets
            .chunks(16)
            .enumerate()
            .map(|(i, c)| format!("{:04X}  {}", 0x0D49 + i * 16, c.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")))
            .collect();
        assert_eq!(eeprom_bytes(&lines), sheets);
        assert_eq!(active_sheet(&sheets, 0).as_deref(), Some("Smooth1"));
        assert_eq!(active_sheet(&sheets, 1).as_deref(), Some("Satin"));
        assert_eq!(active_sheet(&sheets, 2), None, "not set up");
        assert_eq!(
            (print_mode(0), print_mode(1), print_mode(2), print_mode(0xFF)),
            (Some("normal"), Some("silent"), Some("auto"), None)
        );
        // The lifetime totals, little-endian; an unwritten EEPROM is none.
        assert_eq!(eeprom_u32(&[0x2A, 0x01, 0, 0]), Some(298));
        assert_eq!(eeprom_u32(&[0xFF; 4]), None);
        // A negative time or change from the printer is none (PrusaLink: "broken").
        let broken = vec!["NORMAL MODE: Percent done: -1; print time remaining in mins: -1; Change in mins: -1".to_owned()];
        assert_eq!(print_info_full(&broken, false), None, "no percent");
        let no_time = vec!["NORMAL MODE: Percent done: 30; print time remaining in mins: -1; Change in mins: 12".to_owned()];
        assert_eq!(print_info_full(&no_time, false), Some((30, None, Some(12))));
        // M707: the MMU's firmware, from its four registers.
        let r = |n: u8, v: &str| vec![format!("echo:MMU2:<R{n} A{v}*2c."), "ok".to_owned()];
        let (a, b, c, d) = (r(0, "3"), r(1, "0"), r(2, "3"), r(3, "380"));
        assert_eq!(mmu_firmware([&a, &b, &c, &d]).as_deref(), Some("3.0.3+896"));
        assert_eq!(mmu_firmware([&s("ok"), &b, &c, &d]), None, "no MMU");
        // What its MMU is doing.
        assert_eq!(mmu_progress("echo:MMU2:Feeding to nozzle"), Some("Feeding to nozzle"));
        assert_eq!(mmu_progress("echo:MMU2:ERR Wait for User"), Some("ERR Wait for User"));
        assert_eq!(mmu_progress("echo:MMU2:<T0 E8004*3c."), None);
    }

    /// What Prusa's MK3 says by itself, as PrusaLink follows it.
    #[test]
    fn what_it_says_by_itself_is_followed() {
        let screen = Screen::default();
        // Its SD card.
        screen.hear("echo:SD card ok");
        assert_eq!(screen.now().sd_card, Some(true));
        screen.hear("echo:SD card released");
        assert_eq!(screen.now().sd_card, Some(false));
        screen.hear("File opened: BENCHY~1.GCO Size: 812345");
        assert_eq!((screen.now().sd_card, screen.now().sd_opened.as_deref()), (Some(true), Some("BENCHY~1.GCO")));
        // A file in its list (M20 L T): the card is in, even if the insert went unheard.
        screen.hear("echo:SD card released");
        screen.hear("Begin file list");
        screen.hear("End file list");
        assert_eq!(screen.now().sd_card, Some(false));
        screen.hear("Begin file list");
        screen.hear("BENCHY~1.GCO 812345 0x5f3a2b10 \"3DBenchy.gcode\"");
        screen.hear("End file list");
        assert_eq!(screen.now().sd_card, Some(true));
        // Calibrating its thermal model: nothing to be sent until it ends.
        screen.hear("TM: calibration start");
        assert!(screen.now().calibrating);
        screen.hear("Thermal Model settings:");
        assert!(!screen.now().calibrating);
        screen.hear("TM: calibration start");
        screen.hear("TM: calibr. failed!");
        assert!(!screen.now().calibrating);
        // Its bed measured: the mesh to be read again, once.
        screen.hear("echo:enqueing \"G80\"");
        assert!(screen.take_mesh_stale());
        assert!(!screen.take_mesh_stale());
        screen.hear("tmc2130_home_enter(axes_mask=0x04)");
        assert!(screen.take_mesh_stale());
        // Its MMU: the slot, and what it is doing until it is OK again.
        screen.hear("echo:MMU2:MMU2tool=3");
        screen.hear("echo:MMU2:Feeding to extruder");
        assert_eq!((screen.now().mmu_slot, screen.now().mmu_doing.as_deref()), (Some(3), Some("Feeding to extruder")));
        screen.hear("echo:MMU2:OK");
        assert_eq!(screen.now().mmu_doing, None);
        // Reprint from its menu, once.
        screen.hear("// action:start");
        assert!(screen.take_reprint());
        assert!(!screen.take_reprint());
        // Its own reports: each kind, so a silent one starts them again.
        assert!(!screen.reports_by_itself(Duration::from_secs(60)));
        screen.hear("T:20.0 /0.0 B:20.0 /0.0 T0:20.0 /0.0 @:0 B@:0 P:0.0 A:25.0");
        screen.hear("E0:0 RPM PRN1:0 RPM E0@:0 PRN1@:0");
        assert!(!screen.reports_by_itself(Duration::from_secs(60)), "no position yet");
        screen.hear("X:0.00 Y:0.00 Z:0.15 E:0.00 Count X: 0.00 Y:0.00 Z:0.15 E:0.00");
        assert!(screen.reports_by_itself(Duration::from_secs(60)));
    }

    /// A pause, resume or stop asked of a print from its SD card is checked against its next
    /// `M27`: done, nothing said; not done within 15 s, said once (as PrusaLink fails it).
    #[test]
    fn an_sd_print_does_what_was_asked_in_time() {
        let mut printer = Printer::new(FakeMarlin::new(Arc::default()));
        printer.sd = Some(SdStatus { done: 10, total: 100, ..SdStatus::default() });
        printer.sd_asked = Some(("pause", Instant::now()));
        assert_eq!(printer.sd_not_done(), None, "still in time");
        printer.sd_asked = Some(("pause", Instant::now() - Duration::from_secs(16)));
        assert_eq!(printer.sd_not_done().as_deref(), Some("it did not pause within 15 s"));
        assert_eq!(printer.sd_not_done(), None, "said once");
        // Paused as asked: nothing to say.
        printer.sd = Some(SdStatus { paused: true, ..SdStatus::default() });
        printer.sd_asked = Some(("pause", Instant::now() - Duration::from_secs(16)));
        assert_eq!(printer.sd_not_done(), None);
        // Stopped: no SD print any more.
        printer.sd = None;
        printer.sd_asked = Some(("stop", Instant::now() - Duration::from_secs(16)));
        assert_eq!(printer.sd_not_done(), None);
    }

    #[test]
    fn a_prusa_mesh_is_read_front_first() {
        let lines = said(
            "Num X,Y: 7,7\nZ search height: 5.00\nMeasured points:\n  0.10000  0.11000  0.12000\n  0.05000  0.06000  0.07000\n -0.01000  0.00000  0.01000\nok",
        );
        let mesh = mesh(&lines).unwrap();
        assert_eq!(mesh.rows[0], vec![Some(-0.01), Some(0.0), Some(0.01)]);
        assert_eq!(mesh.rows[2][2], Some(0.12));
        assert_eq!(mesh.range(), Some((-0.01, 0.12)));
    }

    /// Marlin numbers its rows: row 0 is the front, however they are printed.
    #[test]
    fn a_marlin_mesh_is_read_by_row_number() {
        let lines = said(
            "Bilinear Leveling Grid:\n      0      1      2\n 0 +0.012 -0.050 +0.100\n 1 +0.020 +0.000 -0.030\n 2 -0.100 +0.040 +0.060\nok",
        );
        let mesh = mesh(&lines).unwrap();
        assert_eq!(mesh.rows.len(), 3);
        assert_eq!(mesh.rows[0][1], Some(-0.05));
        let ubl = said(
            "Bed Topography Report:\n\n    (0,2)          (2,2)\n        0       1       2\n 2 | +0.100  +0.110   .\n   |\n 1 | +0.050  +0.060  +0.070\n   |\n 0 | -0.010 [+0.000] +0.010\n        0       1       2\n    (0,0)          (2,0)\nok",
        );
        let mesh = super::mesh(&ubl).unwrap();
        assert_eq!(mesh.rows[0], vec![Some(-0.01), Some(0.0), Some(0.01)]);
        assert_eq!(mesh.rows[2][2], None, "not measured");
    }

    #[test]
    fn no_mesh_without_one() {
        assert_eq!(mesh(&said("Mesh bed leveling not active.\nok")), None);
        assert_eq!(mesh(&said("T:21.0 /0.0 B:20.1 /0.0\nok")), None);
    }
    use std::sync::{Arc, Mutex};

    #[test]
    fn lines_get_a_number_and_a_checksum() {
        // As OctoPrint and Marlin compute it: the XOR of "N0 M110 N0".
        assert_eq!(
            numbered(0, "M110 N0"),
            format!("N0 M110 N0*{}", checksum("N0 M110 N0"))
        );
        assert_eq!(checksum("N1 G28"), b"N1 G28".iter().fold(0, |s, b| s ^ b));
        assert_eq!(clean("G1 X10 ; move"), Some("G1 X10"));
        assert_eq!(clean("; only a comment"), None);
        assert_eq!(clean("   "), None);
    }

    #[test]
    fn replies_are_read() {
        assert_eq!(parse("ok"), Reply::Ok);
        assert_eq!(parse("ok T:210.0 /210.0 B:60.0 /60.0"), Reply::Ok);
        assert_eq!(parse("Resend: 12"), Reply::Resend(12));
        assert_eq!(parse("rs N7"), Reply::Resend(7));
        assert_eq!(parse("echo:busy: processing"), Reply::Busy);
        assert_eq!(parse("busy: processing"), Reply::Busy);
        assert_eq!(parse("echo:busy: paused for user"), Reply::PausedForUser);
        assert_eq!(parse("start"), Reply::Start);
        assert_eq!(
            parse("Error:Line Number is not Last Line Number+1, Last Line: 3"),
            Reply::Error("Line Number is not Last Line Number+1, Last Line: 3".into())
        );
    }

    /// A printer in memory that checks what it gets as Marlin does: the line number and the
    /// checksum, asking for a resend when one is wrong; it can be told to spoil a line
    /// (as noise on the wire would) and to report busy first.
    pub(crate) struct FakeMarlin {
        expected: u32,
        out: VecDeque<u8>,
        input: Vec<u8>,
        /// Line numbers to treat once as arrived damaged.
        spoil: Vec<u32>,
        busy_first: usize,
        /// The codes it accepted, in order.
        accepted: Arc<Mutex<Vec<String>>>,
        /// Codes whose `ok` it loses once (done, but no `ok`).
        pub(crate) lose_ok: Vec<String>,
        /// Codes it never gets, once (lost on the wire: no answer at all).
        pub(crate) lose_line: Vec<String>,
        /// Forgets the `ok` after a resend request (a known firmware bug).
        pub(crate) no_ok_after_resend: bool,
        /// Asks for the same line forever.
        pub(crate) resend_forever: bool,
        /// Says `wait` when it lost an `ok` (Marlin's NO_TIMEOUTS).
        pub(crate) says_wait: bool,
        /// Errors it says for a code: (code, error).
        pub(crate) errors: Vec<(String, String)>,
        /// Its `M115` answer.
        pub(crate) m115: String,
        /// M109 waits until an M108 breaks it off (Marlin's emergency parser).
        pub(crate) heat_until_m108: bool,
        heating: bool,
        /// A code that asks a question on its screen (host prompt) and waits for M876.
        pub(crate) asks_at: Option<String>,
        asking: bool,
        /// A code after which it restarts (a power cut): it says `start`.
        pub(crate) restart_after: Option<String>,
        /// Started again after a power cut in a host's print, with the number of the last
        /// line it carried out: M602 resumes it and it asks for the next line (Prusa's MK3).
        pub(crate) recovering: Option<u32>,
        /// Each accepted code with its line number.
        pub(crate) numbers: Arc<Mutex<Vec<(u32, String)>>>,
    }

    impl FakeMarlin {
        pub(crate) fn new(accepted: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                expected: 0,
                out: b"start\necho: Marlin\n".iter().copied().collect(),
                input: Vec::new(),
                spoil: Vec::new(),
                busy_first: 0,
                accepted,
                lose_ok: Vec::new(),
                lose_line: Vec::new(),
                no_ok_after_resend: false,
                resend_forever: false,
                says_wait: false,
                errors: Vec::new(),
                m115: "FIRMWARE_NAME:Prusa-Firmware 3.14.0 based on Marlin FIRMWARE_URL:https://github.com/prusa3d/Prusa-Firmware PROTOCOL_VERSION:1.0 MACHINE_TYPE:Prusa i3 MK3S EXTRUDER_COUNT:1".to_owned(),
                heat_until_m108: false,
                heating: false,
                asks_at: None,
                asking: false,
                restart_after: None,
                recovering: None,
                numbers: Arc::default(),
            }
        }

        fn say(&mut self, text: &str) {
            self.out.extend(text.bytes());
            self.out.push_back(b'\n');
        }

        fn ask_again(&mut self) {
            self.say("Error:Line Number is not Last Line Number+1, Last Line: 0");
            let expected = self.expected;
            self.say(&format!("Resend: {expected}"));
            if !self.no_ok_after_resend {
                self.say("ok");
            }
        }

        fn receive(&mut self, line: &str) {
            let Some((body, sum)) = line.rsplit_once('*') else {
                // Unnumbered (as PrusaLink's detection asks it): answered as numbered.
                let prusa = self.m115.contains("Prusa-Firmware");
                match line {
                    "PRUSA Fir" | "M862.2 Q" if !prusa => self.say(&format!("echo:Unknown command: \"{line}\"")),
                    "M862.2 Q" => self.say("302"),
                    "PRUSA Fir" => {
                        let version = self.m115.split("Prusa-Firmware ").nth(1).and_then(|r| r.split_whitespace().next()).unwrap_or("3.14.1").to_owned();
                        self.say(&format!("{version}-8237"))
                    }
                    _ => {}
                }
                return self.say("ok");
            };
            let number: u32 = body[1..].split(' ').next().unwrap().parse().unwrap();
            let code = body.split_once(' ').unwrap().1.to_owned();
            if let Some(i) = self.lose_line.iter().position(|c| *c == code) {
                self.lose_line.remove(i);
                return;
            }
            if code.starts_with("M110") {
                self.expected = number + 1;
                return self.say("ok");
            }
            if self.resend_forever && code != "M110 N0" {
                return self.ask_again();
            }
            // A line done already, arriving again: dropped without an answer, as Marlin does.
            if number < self.expected && sum.parse::<u8>() == Ok(checksum(body)) {
                return;
            }
            let damaged = self.spoil.contains(&number) || sum.parse::<u8>() != Ok(checksum(body));
            if damaged || number != self.expected {
                self.spoil.retain(|n| *n != number);
                return self.ask_again();
            }
            for _ in 0..std::mem::take(&mut self.busy_first) {
                self.say("echo:busy: processing");
            }
            self.expected += 1;
            self.numbers.lock().unwrap().push((number, code.clone()));
            // Started again after a power cut: a host is there (M79), it says it is ready;
            // resumed (M602) it heats and moves back, takes the saved line's number and asks
            // for the next one, then answers the M602.
            if let Some(saved) = self.recovering {
                if code.starts_with("M79") {
                    self.say("//action:uvlo_recovery_ready");
                }
                if code == "M602" {
                    self.recovering = None;
                    self.accepted.lock().unwrap().push(code);
                    self.expected = saved + 1;
                    self.say(&format!("Resend: {}", saved + 1));
                    self.say("ok");
                    return self.say("ok");
                }
            }
            if code == "M108" && self.heating {
                // The heat-up ends (its ok), then the M108 line itself.
                self.heating = false;
                self.say("ok");
            }
            if code.starts_with("M876") && self.asking {
                // The question is answered: it goes away, the line that asked ends.
                self.asking = false;
                self.accepted.lock().unwrap().push(code.clone());
                self.say("//action:prompt_end");
                self.say("ok");
                return self.say("ok");
            }
            let errors: Vec<String> = self
                .errors
                .iter()
                .filter(|(c, _)| *c == code)
                .map(|(_, e)| e.clone())
                .collect();
            for error in errors {
                self.say(&error);
            }
            if let Some(i) = self.lose_ok.iter().position(|c| *c == code) {
                self.lose_ok.remove(i);
                self.accepted.lock().unwrap().push(code);
                if self.says_wait {
                    self.say("wait");
                }
                return;
            }
            match code.as_str() {
                "M115" => {
                    let m115 = self.m115.clone();
                    for line in m115.lines() {
                        self.say(line);
                    }
                }
                "M105" => return self.say("ok T:24.5 /0.0 B:23.1 /0.0 T0:24.5 /0.0 @:0 B@:0 P:0.0 A:28.2"),
                // As Prusa's MK3 firmware answers them: the number alone; its version with
                // its build. Other firmware knows neither.
                "M862.2 Q" | "PRUSA Fir" if !self.m115.contains("Prusa-Firmware") => {
                    self.say(&format!("echo:Unknown command: \"{code}\""))
                }
                "M862.2 Q" => self.say("302"),
                "PRUSA Fir" => {
                    let version = self.m115.split("Prusa-Firmware ").nth(1).and_then(|r| r.split_whitespace().next()).unwrap_or("3.14.1").to_owned();
                    self.say(&format!("{version}-8237"))
                }
                "M20 LT" => {
                    self.say("Begin file list");
                    self.say("BENCHY~1.GCO 812345 0x5f3a2b10 \"Benchy.gcode\"");
                    self.say("End file list");
                }
                _ => {}
            }
            self.accepted.lock().unwrap().push(code.clone());
            if code.starts_with("M109") && self.heat_until_m108 {
                self.heating = true;
                return self.say("T:180.0 E:0 W:?");
            }
            if self.restart_after.as_deref() == Some(code.as_str()) {
                // The power went: it starts again, everything it had is gone.
                self.restart_after = None;
                self.expected = 0;
                return self.say("start");
            }
            if self.asks_at.as_deref() == Some(code.as_str()) {
                self.asking = true;
                self.say("//action:prompt_begin Filament runout");
                self.say("//action:prompt_button Continue");
                self.say("//action:prompt_button Purge more");
                self.say("//action:prompt_show");
                return self.say("echo:busy: paused for user");
            }
            self.say("ok T:210.0 /210.0 B:60.0 /60.0");
        }
    }

    impl Read for FakeMarlin {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.out.is_empty() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "quiet"));
            }
            let n = buf.len().min(self.out.len());
            for byte in buf.iter_mut().take(n) {
                *byte = self.out.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    impl Write for FakeMarlin {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.input.extend_from_slice(buf);
            while let Some(end) = self.input.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.input.drain(..=end).collect();
                self.receive(String::from_utf8_lossy(&line).trim());
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    const GCODE: &str =
        "; a small print\nG28 ; home\nM104 S210\nG1 X10 Y10\n\nG1 X20 Y10 E1.5\nM84\n";

    fn run(fake: FakeMarlin) -> (Vec<String>, Vec<Event>) {
        let accepted = fake.accepted.clone();
        let mut printer = Printer::new(fake);
        let mut events = Vec::new();
        printer.connect(&mut |e| events.push(e)).unwrap();
        printer
            .send(GCODE.lines(), &mut |e| events.push(e))
            .unwrap();
        let got = accepted.lock().unwrap().clone();
        (got, events)
    }

    /// Every code line arrives once, in order, without comments or blank lines.
    #[test]
    fn a_file_is_streamed_line_by_line() {
        let (accepted, events) = run(FakeMarlin::new(Arc::default()));
        assert_eq!(
            accepted,
            ["G28", "M104 S210", "G1 X10 Y10", "G1 X20 Y10 E1.5", "M84"]
        );
        assert_eq!(events.last(), Some(&Event::Progress { done: 5, total: 5 }));
        assert!(events.contains(&Event::Said("echo: Marlin".into())));
    }

    /// A line that arrives damaged is sent again when the printer asks, and nothing is lost
    /// or doubled.
    #[test]
    fn a_damaged_line_is_resent() {
        let mut fake = FakeMarlin::new(Arc::default());
        fake.spoil = vec![3];
        let (accepted, events) = run(fake);
        assert_eq!(
            accepted,
            ["G28", "M104 S210", "G1 X10 Y10", "G1 X20 Y10 E1.5", "M84"]
        );
        assert!(events.contains(&Event::Resent(3)), "{events:?}");
    }

    /// One command and its answer: the firmware from M115, the temperatures from M105.
    #[test]
    fn firmware_and_temperatures_are_read() {
        let mut printer = Printer::new(FakeMarlin::new(Arc::default()));
        printer.connect(&mut |_| {}).unwrap();
        let answer = printer.command("M115").unwrap();
        assert_eq!(firmware(&answer).as_deref(), Some("Prusa-Firmware 3.14.0"));
        let answer = printer.command("M105").unwrap();
        assert_eq!(
            heat(&answer),
            Heat {
                nozzle_c: Some(24.5),
                nozzle_target_c: Some(0.0),
                bed_c: Some(23.1),
                bed_target_c: Some(0.0),
                nozzle_heating: Some(false),
                bed_heating: Some(false),
                hotend_fan_rpm: None,
                print_fan_rpm: None,
                position_mm: None,
            }
        );
        // Prusa's MK3: fans in its M105 answer and its fan auto-report.
        let fans = heat(&["ok T:215.0 /215.0 B:60.0 /60.0 T0:215.0 /215.0 @:90 B@:40 P:35.0 A:30.1 E0:3500 RPM PRN1:4800 RPM E0@:255 PRN1@:200".to_owned()]);
        assert_eq!((fans.hotend_fan_rpm, fans.print_fan_rpm), (Some(3500), Some(4800)));
        assert_eq!(fans.nozzle_c, Some(215.0));
        // The head's position (M114, or M155 C4), the millimetres before `Count`.
        let at = heat(&["X:10.00 Y:20.50 Z:0.15 E:0.00 Count X: 800 Y:1640 Z:60".to_owned(), "ok".to_owned()]);
        assert_eq!(at.position_mm, Some([10.0, 20.5, 0.15]));
        assert_eq!(heat(&["ok T:21.0 /0.0 B:21.0 /0.0".to_owned()]).position_mm, None);
        // Several of its own reports in one answer (M155 on): the newest counts.
        let newest = heat(&[
            "T:20.0 /0.0 B:20.0 /0.0 T0:20.0 /0.0 @:0 B@:0".to_owned(),
            "X:1.00 Y:1.00 Z:1.00 E:0.00 Count X: 0 Y:0 Z:0".to_owned(),
            "E0:0 RPM PRN1:0 RPM E0@:0 PRN1@:0".to_owned(),
            "T:25.0 /0.0 B:22.0 /0.0 T0:25.0 /0.0 @:0 B@:0".to_owned(),
            "X:2.00 Y:3.00 Z:4.00 E:0.00 Count X: 0 Y:0 Z:0".to_owned(),
            "E0:3000 RPM PRN1:0 RPM E0@:255 PRN1@:0".to_owned(),
            "ok".to_owned(),
        ]);
        assert_eq!((newest.nozzle_c, newest.bed_c), (Some(25.0), Some(22.0)));
        assert_eq!(newest.position_mm, Some([2.0, 3.0, 4.0]));
        assert_eq!(newest.hotend_fan_rpm, Some(3000));
        let fans = heat(&["E0:0 RPM PRN1:0 RPM E0@:0 PRN1@:0".to_owned()]);
        assert_eq!((fans.hotend_fan_rpm, fans.print_fan_rpm), (Some(0), Some(0)));
        assert!(routine("E0:0 RPM PRN1:0 RPM E0@:0 PRN1@:0"));
        assert!(routine("echo:MMU2:<P0 E0*2a.") && !routine("echo:Unknown command: \"X\""));
        // An MMU error in the MK3's chatter with it, as Prusa numbers it, and its end.
        assert_eq!(mmu_error("echo:MMU2:<T0 E8004*3c."), Some(Some(4104)));
        assert_eq!(mmu_error("echo:MMU2:<Q0 E8087*1a."), Some(Some(4115)), "selector homing");
        assert_eq!(mmu_error("echo:MMU3:<Q0 EC100*1a."), Some(Some(4222)), "idler overheat");
        assert_eq!(mmu_error("echo:MMU2:<Q0 P5*1a."), Some(None));
        assert_eq!(mmu_error("echo:MMU2:>Q0*ea."), None);
        let screen = Screen::default();
        screen.heard(&parse("echo:MMU2:<T0 E8004*3c."), "echo:MMU2:<T0 E8004*3c.");
        assert_eq!(
            screen.now().attention.as_deref(),
            Some("MMU error 04104 (prusa.io/04104)")
        );
        screen.heard(&parse("echo:MMU2:<Q0 F0*1a."), "echo:MMU2:<Q0 F0*1a.");
        assert!(screen.now().attention.is_none());
        // The thermal model's error, and its clearing.
        let screen = Screen::default();
        screen.heard(&parse("TM: error triggered!"), "TM: error triggered!");
        assert!(screen.now().attention.is_some());
        screen.heard(&parse("TM: error cleared"), "TM: error cleared");
        assert!(screen.now().attention.is_none());
        // Heating, written together, and Marlin without a power for the bed.
        let heating = heat(&["T:180.2/215.0 B:58.0 /60.0 @:127".to_owned()]);
        assert_eq!(heating.nozzle_target_c, Some(215.0));
        assert_eq!(heating.bed_target_c, Some(60.0));
        assert_eq!(heating.nozzle_heating, Some(true));
        assert_eq!(heating.bed_heating, None);
        let marlin = vec!["FIRMWARE_NAME:Marlin 2.1.2.1 (Jun 1 2024) SOURCE_CODE_URL:github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0".to_owned()];
        assert_eq!(firmware(&marlin).as_deref(), Some("Marlin 2.1.2.1"));
        let answer = printer.command("M115").unwrap();
        assert_eq!(machine_type(&answer).as_deref(), Some("Prusa i3 MK3S"));
        let answer = printer.command("M862.2 Q").unwrap();
        assert_eq!(model_code(&answer), Some(302));
        let mini = vec!["FIRMWARE_NAME:Prusa-Firmware-Buddy 6.3.0 (Github) SOURCE_CODE_URL:https://github.com/prusa3d/Prusa-Firmware-Buddy PROTOCOL_VERSION:1.0 MACHINE_TYPE:Prusa-MINI EXTRUDER_COUNT:1".to_owned()];
        assert_eq!(
            firmware(&mini).as_deref(),
            Some("Prusa-Firmware-Buddy 6.3.0")
        );
        assert_eq!(machine_type(&mini).as_deref(), Some("Prusa-MINI"));
    }

    /// Every kind of line in docs/marlin-serial-protocol.md, as Marlin 2, Buddy's Marlin and
    /// Prusa's MK3 firmware write it, read for what it means.
    #[test]
    fn every_line_the_firmware_can_say_is_read() {
        use Reply::{Busy, Error, Ok, Other, PausedForUser, Resend, Start, Wait};
        // The conversation.
        for ok in ["ok", "ok T:210.0 /210.0 B:60.0 /60.0 @:64 B@:0", "ok N12 P15 B3"] {
            assert_eq!(parse(ok), Ok, "{ok}");
        }
        for (line, n) in [("Resend: 12", 12), ("Resend:12", 12), ("rs N7", 7), ("rs 7", 7)] {
            assert_eq!(parse(line), Resend(n), "{line}");
        }
        for line in ["busy: processing", "echo:busy: processing"] {
            assert_eq!(parse(line), Busy, "{line}");
        }
        for line in [
            "echo:busy: paused for user",
            "busy: paused for user",
            "echo:busy: paused for input",
        ] {
            assert_eq!(parse(line), PausedForUser, "{line}");
        }
        assert_eq!(parse("start"), Start);
        assert_eq!(parse("wait"), Wait);
        // Host actions, with and without the space Prusa's firmware sometimes puts in.
        for (line, action) in [
            ("//action:pause", Action::Pause),
            ("// action:pause", Action::Pause),
            ("// action:cancel", Action::Cancel),
            ("//action:paused", Action::Paused),
            ("//action:resumed", Action::Resume),
            ("// action:ready", Action::Ready),
            ("// action:not_ready", Action::NotReady),
            ("// action:uvlo_auto_recovery_ready", Action::PowerLoss),
            ("//action:poweroff", Action::Killed),
            ("//action:prompt_end", Action::PromptEnd),
        ] {
            assert_eq!(parse(line), Reply::Action(action), "{line}");
        }
        for line in [
            "//action:filament_runout 0",
            "//action:probe_failed",
            "//action:probe_rewipe",
            "//action:safety_timer_expired",
            "// action:dump_available",
        ] {
            assert!(matches!(parse(line), Reply::Action(Action::Notice(_))), "{line}");
        }
        // Fatal, in every firmware's words.
        for line in [
            "Error:Printer halted. kill() called!",
            "!! KILL caused by KILL button/pin",
            "KILL caused by too much inactive time - current command: G1",
            "Error:Printer stopped due to errors. Fix the error and use M999 to restart.",
            "echo:Printer stopped due to errors. Supervision required.",
            "!! STOP called because of BLTouch error - restart with M999",
            "STOP called because of unhomed error - restart with M999",
            "Error:Thermal Runaway, system stopped! Heater_ID: 0",
            "Error:Thermal Malfunction, system stopped! Heater_ID: bed",
            "Error:Heating failed, system stopped! Heater_ID: 0",
            "Error:MAXTEMP triggered, system stopped! Heater_ID: 0",
            "Error:MINTEMP triggered, system stopped! Heater_ID: bed",
            "Error: HOTEND THERMAL RUNAWAY",
            "Error: HEATBED THERMAL RUNAWAY",
            "Error:0: Heaters switched off. MAXTEMP triggered!",
            "Error:Heaters switched off. MINTEMP BED triggered!",
        ] {
            assert!(fatal(line), "{line}");
            let screen = Screen::default();
            screen.heard(&parse(line), line);
            assert!(screen.now().fault.is_some(), "{line}");
        }
        assert!(matches!(parse("!! KILL caused by KILL button/pin"), Error(_)));
        assert!(matches!(parse("fatal: no comms"), Error(_)));
        // Needs someone, not dead.
        for line in [
            "Bed leveling failed. Sensor triggered too high",
            "Bed leveling failed. Sensor didn't trigger. Debris on nozzle? Waiting for reset.",
            "TM: error triggered!",
            "Extruder fan speed is lower than expected",
            "Print fan speed is lower than expected",
            "INT4",
            "echo:Printer locked! (Unlock with M511 or LCD)",
        ] {
            assert!(needs_someone(line).is_some(), "{line}");
            assert!(!fatal(line), "{line}");
        }
        // Errors about the line on the wire, the SD card or an unknown code: not about the
        // print. Others are.
        for text in [
            "Line Number is not Last Line Number+1, Last Line: 3",
            "checksum mismatch, Last Line: 3",
            "No Checksum with line number, Last Line: 3",
            "No Line Number with checksum, Last Line: 3",
            "volume.init failed",
            "openRoot failed",
            "open failed, File: X.GCO",
        ] {
            assert!(harmless_error(text), "{text}");
        }
        for text in ["Hotend too cold", "Probing Failed", "Printer halted. kill() called!", "0"] {
            assert!(!harmless_error(text), "{text}");
        }
        // Not fatal, not anyone's business: shown, not acted on.
        for line in [
            "echo:Unknown command: \"X1\"",
            "echo: cold extrusion prevented",
            "T:210.0 E:0 B:60.0",
            "T:180.3 E:0 W:?",
            "LCD status changed",
            "echo:enqueing \"M24\"",
            "E0:3500 RPM PRN1:0 RPM E0@:255 PRN1@:0",
        ] {
            assert!(!fatal(line) && needs_someone(line).is_none(), "{line}");
            assert!(matches!(parse(line), Other(_)), "{line}");
        }
    }

    /// `M862.2 Q` as Buddy and as Prusa's MK3 firmware answer it, an MMU attached or not.
    #[test]
    fn the_model_code_in_both_forms() {
        assert_eq!(model_code(&said("echo:  M862.2 P120\nok")), Some(120));
        assert_eq!(model_code(&said("302\nok")), Some(302));
        assert_eq!(model_code(&said("30302\nok")), Some(302), "MK3S with an MMU3");
        assert_eq!(model_code(&said("20250\nok")), Some(250), "MK2.5 with an MMU2");
        assert_eq!(model_code(&said("echo:Unknown command: \"M862.2 Q\"\nok")), None);
    }

    /// `M27` as Prusa's MK3 and Marlin 2 answer it.
    #[test]
    fn an_sd_print_is_followed() {
        let mk3 = sd_status(&said("Benchy 0.2mm PLA.gcode\nSD printing byte 1234/567890\n1:05\nok"))
            .unwrap()
            .unwrap();
        assert_eq!(mk3.file.as_deref(), Some("Benchy 0.2mm PLA.gcode"));
        assert_eq!((mk3.done, mk3.total), (1234, 567890));
        assert_eq!(mk3.elapsed_s, Some(3600 + 5 * 60));
        assert!(!mk3.paused);
        let path = sd_status(&said("/PARTS/BOX~1.GCO\nSD printing byte 1/2\n0:00\nok")).unwrap().unwrap();
        assert_eq!(path.file.as_deref(), Some("BOX~1.GCO"));
        assert!(sd_status(&said("SD print paused\nok")).unwrap().unwrap().paused);
        assert_eq!(sd_status(&said("Not SD printing\nok")), Some(None));
        assert_eq!(sd_status(&said("Print saved\nok")), Some(None), "a host's print, not the card's");
        let marlin = sd_status(&said("Current file: BENCHY~1.GCO Benchy.gcode\nSD printing byte 10/100\nok"))
            .unwrap()
            .unwrap();
        assert_eq!(marlin.file.as_deref(), Some("Benchy.gcode"));
        assert_eq!(marlin.elapsed_s, None);
        assert_eq!(sd_status(&said("echo:Unknown command: \"M27\"\nok")), None);
    }

    /// What a printer can do, from its `M115` answer.
    #[test]
    fn capabilities_are_read() {
        let caps = capabilities(&said(
            "FIRMWARE_NAME:Marlin 2.1.2\nCap:EEPROM:1\nCap:AUTOREPORT_TEMP:1\nCap:EMERGENCY_PARSER:1\nCap:PROMPT_SUPPORT:0\nok",
        ));
        assert!(caps.autoreport_temp && caps.emergency_parser && !caps.prompt_support);
        assert_eq!(caps.all.len(), 4);
        assert!(long_running("G28 W") && long_running("M109 S215") && !long_running("G1 X1"));
    }

    /// Prusa's MK3 as PrusaLink (serial_queue.py `check_status`): silent too long, the line
    /// waiting goes again with its own number (no `M105`); the third time the port is given
    /// up.
    #[test]
    fn a_silent_mk3_gets_the_line_again_as_prusalink_sends_it() {
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut fake = FakeMarlin::new(accepted.clone());
        // Lost on the wire, as PrusaLink takes a silence ("the printer yeeted our RX buffer").
        fake.lose_line = vec!["M220".into()];
        let mut printer = quick(fake);
        printer.firmware = Some("Prusa-Firmware 3.14.1-8237".to_owned());
        printer.command("M220").unwrap();
        let sent = accepted.lock().unwrap().clone();
        assert_eq!(sent.iter().filter(|c| *c == "M220").count(), 1, "the line again, taken: {sent:?}");
        assert!(!sent.iter().any(|c| c == "M105"), "no M105: {sent:?}");
        // Never answered: given up the third time.
        let mut fake = FakeMarlin::new(Arc::default());
        fake.lose_line = vec!["M221".into(); 5];
        let mut printer = quick(fake);
        printer.firmware = Some("Prusa-Firmware 3.14.1-8237".to_owned());
        assert!(printer.command("M221").is_err());
    }

    /// What PrusaLink sends Prusa's MK3 family, in its forms (`{}` any value): every literal
    /// here is checked against PrusaLink's source by scripts/prusalink-audit-check.py.
    pub(crate) const PRUSALINK_SENDS: &[&str] = &[
        "M400", "M110 N0", "M862.2 Q", "PRUSA Fir", "M862.1 Q", "PRUSA SN", "M707 A0x00",
        "M707 A0x01", "M707 A0x02", "M707 A0x03", "D3 Ax{} C{}", "D3 Ax0D05 X{}", "M20 LT", "M21",
        "M552 P{}", "M552 P0.0.0.0", "M155 S2 C7", "M155 S0 C0", "M79 S\"PC\"", "M220", "M221",
        "M73", "M27 P", "M420", "M75", "M76", "M77", "M601", "M602", "M603", "M25", "M23 {}", "M24",
        "M73 Q{} S{} C{} P{} R{} D{}", "M300 S880 P200", "M1 FW COMM ERR. Aborted",
        "M140 S{}", "M104 S{}", "M109 S{}", "M300 P500 S1", "M0 Insert the filament", "M701", "M702",
        "M72 S1", "M72 S0", "M300 P200 S600", "M117 ~Cannot re-print", "M84", "G90", "G91",
        "G1 F{} {}", "G28 {}", "M220 S{}", "M221 S{}", "M83", "G1 F{} E{}",
        "M28 {}/setready.g", "M29", "M117 ~{}", "M300 P100 S3200", "M300 P25 S0", "M300 P25 S4800",
        "M300 P75 S0", "M300 P600 S5", "M300 P14 S50",
    ];

    /// Whether `code` is one of PrusaLink's forms.
    pub(crate) fn prusalink_sends(code: &str) -> bool {
        PRUSALINK_SENDS.iter().any(|form| {
            let parts: Vec<&str> = form.split("{}").collect();
            let mut rest = code;
            for (i, part) in parts.iter().enumerate() {
                if i == 0 {
                    let Some(r) = rest.strip_prefix(part) else { return false };
                    rest = r;
                } else if i == parts.len() - 1 {
                    return !part.is_empty() && rest.len() > part.len() && rest.ends_with(part)
                        || part.is_empty() && !rest.is_empty();
                } else {
                    let Some(at) = rest.get(1..).and_then(|r| r.find(part)) else { return false };
                    rest = &rest[at + 1 + part.len()..];
                }
            }
            rest.is_empty()
        })
    }

    /// Connected as PrusaLink connects (`serial_adapter.py` `_get_info`): `PRUSA Fir` and
    /// `M862.2 Q` unnumbered in one write, then `M400` and `M110 N0`; elsewhere `PRUSA Fir`
    /// alone first, and plain Marlin is asked no `M862`.
    #[test]
    fn it_is_connected_as_prusalink_connects() {
        let wrote: Arc<Mutex<Vec<String>>> = Arc::default();
        let link = Tapped { fake: FakeMarlin::new(Arc::default()), wrote: wrote.clone(), partial: Vec::new() };
        let mut printer = Printer::new(link);
        printer.hurry();
        let found = printer.connect_as_prusalink(true, &mut |_| {}).unwrap();
        assert_eq!(found, (Some("3.14.0-8237".to_owned()), Some(302)));
        let wrote = wrote.lock().unwrap().clone();
        assert_eq!(wrote[..3], ["PRUSA Fir", "M862.2 Q", "M400"], "{wrote:?}");
        assert_eq!(wrote[3], numbered(0, "M110 N0"));

        let wrote: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut fake = FakeMarlin::new(Arc::default());
        fake.m115 = "FIRMWARE_NAME:Marlin 2.1.2.1 MACHINE_TYPE:Ender-3 EXTRUDER_COUNT:1".into();
        let link = Tapped { fake, wrote: wrote.clone(), partial: Vec::new() };
        let mut printer = Printer::new(link);
        printer.hurry();
        assert_eq!(printer.connect_as_prusalink(false, &mut |_| {}).unwrap(), (None, None));
        let wrote = wrote.lock().unwrap().clone();
        assert_eq!(wrote[..2], ["PRUSA Fir", "M400"], "no M862 to Marlin: {wrote:?}");
    }

    /// PrusaLink's menu on the card: found by its folder's name in a listing, with or without
    /// its `setready.g`; that file opened, started and done within 1 s each is "Set Ready",
    /// no print (`special_commands.py`); any other file is a print.
    #[test]
    fn prusalinks_menu_on_the_card_sets_it_ready() {
        let listing = |with: bool| -> Vec<String> {
            let mut l = vec!["Begin file list".to_owned(), "DIR_ENTER: /PRUSAL~1/ \"PrusaLink menu\"".to_owned()];
            if with {
                l.push("/PRUSAL~1/SETREADY.G 4 0x5f3a2b10 \"setready.g\"".to_owned());
            }
            l.extend(["DIR_EXIT".to_owned(), "BENCHY~1.GCO 812345 \"Benchy.gcode\"".to_owned(), "End file list".to_owned()]);
            l
        };
        assert_eq!(prusalink_menu(&listing(false)), Some(("prusal~1".to_owned(), false)));
        assert_eq!(prusalink_menu(&listing(true)), Some(("prusal~1".to_owned(), true)));
        assert_eq!(prusalink_menu(&["Begin file list".to_owned(), "End file list".to_owned()]), None);
        let screen = Screen::default();
        screen.set_menu(Some("prusal~1".to_owned()));
        screen.hear("File opened: /PRUSAL~1/SETREADY.G Size: 4");
        screen.hear("echo:enqueing \"M24\"");
        screen.hear("Done printing file");
        let now = screen.now();
        assert!(now.ready_said && !now.sd_started && !now.sd_done && now.sd_opened.is_none(), "Set Ready, no print");
        assert!(screen.take_ready());
        screen.hear("File opened: BENCHY~1.GCO Size: 812345");
        screen.hear("echo:enqueing \"M24\"");
        assert!(screen.now().sd_started, "a print");
    }

    fn quick(fake: FakeMarlin) -> Printer<FakeMarlin> {
        let mut printer = Printer::new(fake);
        printer.hurry();
        printer.connect(&mut |_| {}).unwrap();
        printer
    }

    /// An `ok` lost on the wire: the printer is asked after (a numbered M105), whose `ok`
    /// settles it; nothing is sent twice, nothing lost.
    #[test]
    fn a_lost_ok_is_asked_after() {
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut fake = FakeMarlin::new(accepted.clone());
        fake.lose_ok = vec!["M104 S210".into()];
        let mut printer = quick(fake);
        printer.send(GCODE.lines(), &mut |_| {}).unwrap();
        assert_eq!(
            *accepted.lock().unwrap(),
            ["G28", "M104 S210", "G1 X10 Y10", "G1 X20 Y10 E1.5", "M84"]
        );
    }

    /// The same, at once when the printer says `wait` (idle with an `ok` due).
    #[test]
    fn wait_means_the_ok_was_lost() {
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut fake = FakeMarlin::new(accepted.clone());
        fake.lose_ok = vec!["G28".into()];
        fake.says_wait = true;
        let mut printer = quick(fake);
        let started = Instant::now();
        printer.send(GCODE.lines(), &mut |_| {}).unwrap();
        assert!(started.elapsed() < Duration::from_millis(140), "did not wait for silence");
        assert_eq!(accepted.lock().unwrap().len(), 5);
    }

    /// A line lost on the wire: the M105 that asks after it makes the printer ask for the
    /// line again, and both go, in order.
    #[test]
    fn a_lost_line_comes_back_as_a_resend() {
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut fake = FakeMarlin::new(accepted.clone());
        fake.lose_line = vec!["G1 X10 Y10".into()];
        let mut printer = quick(fake);
        let mut events = Vec::new();
        printer.send(GCODE.lines(), &mut |e| events.push(e)).unwrap();
        assert_eq!(
            *accepted.lock().unwrap(),
            ["G28", "M104 S210", "G1 X10 Y10", "G1 X20 Y10 E1.5", "M84"]
        );
        assert!(events.iter().any(|e| matches!(e, Event::Resent(_))));
    }

    /// Firmware that forgets the `ok` after a resend request: the line goes anyway.
    #[test]
    fn a_resend_without_its_ok_still_goes() {
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut fake = FakeMarlin::new(accepted.clone());
        fake.spoil = vec![3];
        fake.no_ok_after_resend = true;
        let mut printer = quick(fake);
        printer.send(GCODE.lines(), &mut |_| {}).unwrap();
        assert_eq!(accepted.lock().unwrap().len(), 5);
    }

    /// A printer that asks for the same line forever: the connection counts as broken.
    #[test]
    fn a_resend_loop_is_given_up() {
        let mut fake = FakeMarlin::new(Arc::default());
        fake.resend_forever = true;
        let mut printer = quick(fake);
        let error = printer.send(GCODE.lines(), &mut |_| {}).unwrap_err();
        assert!(error.to_string().contains("keeps asking"), "{error}");
    }

    /// A printer that says nothing at all: asked after a few times, then given up.
    #[test]
    fn a_silent_printer_is_given_up() {
        let mut fake = FakeMarlin::new(Arc::default());
        fake.lose_ok = vec!["G28".into(), "M105".into(), "M105".into(), "M105".into()];
        let mut printer = quick(fake);
        let error = printer.send(GCODE.lines(), &mut |_| {}).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    /// An error about the print is told apart from one about the line on the wire.
    #[test]
    fn errors_about_the_print_are_told() {
        let mut fake = FakeMarlin::new(Arc::default());
        fake.errors = vec![
            ("G1 X10 Y10".into(), "Error:Hotend too cold".into()),
            ("M84".into(), "echo:Unknown command: \"M84\"".into()),
        ];
        let mut printer = quick(fake);
        let mut events = Vec::new();
        printer.send(GCODE.lines(), &mut |e| events.push(e)).unwrap();
        let errors: Vec<&Event> = events.iter().filter(|e| matches!(e, Event::Error(_))).collect();
        assert_eq!(errors, [&Event::Error("Hotend too cold".into())]);
    }

    /// The firmware and what it can do come from its M115 answer.
    #[test]
    fn m115_tells_the_firmware_and_what_it_can_do() {
        let mut fake = FakeMarlin::new(Arc::default());
        fake.m115 = "FIRMWARE_NAME:Marlin 2.1.2.1 (Jun 1 2024) SOURCE_CODE_URL:github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0 MACHINE_TYPE:Ender-3 EXTRUDER_COUNT:1\nCap:EMERGENCY_PARSER:1\nCap:AUTOREPORT_TEMP:0".into();
        let mut printer = quick(fake);
        printer.command("M115").unwrap();
        assert_eq!(printer.firmware.as_deref(), Some("Marlin 2.1.2.1"));
        assert!(printer.caps.emergency_parser && !printer.caps.autoreport_temp);
        assert!(!printer.is_prusa_mk3());
    }

    /// While the printer reports busy (heating, homing) the sender waits.
    #[test]
    fn a_busy_printer_is_waited_for() {
        let mut fake = FakeMarlin::new(Arc::default());
        fake.busy_first = 3;
        let (accepted, _) = run(fake);
        assert_eq!(accepted.len(), 5);
    }

    /// A printer that says each `line` right after it got the `code` before it, sent through
    /// `GCODE` as a job is: what it accepted, what was told, and its screen afterwards.
    fn hearing(after: &[(&str, &str)]) -> (Vec<String>, Vec<Event>, Arc<Screen>) {
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut fake = FakeMarlin::new(accepted.clone());
        fake.errors = after.iter().map(|(c, l)| (c.to_string(), l.to_string())).collect();
        let mut printer = quick(fake);
        let mut events = Vec::new();
        printer.send(GCODE.lines(), &mut |e| events.push(e)).unwrap();
        let got = accepted.lock().unwrap().clone();
        (got, events, printer.waiting())
    }

    const WHOLE_JOB: [&str; 5] = ["G28", "M104 S210", "G1 X10 Y10", "G1 X20 Y10 E1.5", "M84"];

    fn screen_after(line: &str) -> ScreenState {
        let screen = Screen::default();
        screen.hear(line);
        screen.now()
    }

    /// Proves the row `TEMPERATURE_REGEX` of docs/prusalink-audit.md: a temperature report
    /// gives both temperatures, both targets and whether the heaters have power; a report
    /// without numbers gives nothing.
    #[test]
    fn a_temperature_report_is_read() {
        let heat = heat(&["ok T:210.3 /210.0 B:60.1 /60.0 T0:210.3 /210.0 @:64 B@:0 P:35.0 A:30.1".to_owned()]);
        assert_eq!(
            (heat.nozzle_c, heat.nozzle_target_c, heat.bed_c, heat.bed_target_c),
            (Some(210.3), Some(210.0), Some(60.1), Some(60.0))
        );
        assert_eq!((heat.nozzle_heating, heat.bed_heating), (Some(true), Some(false)));
        assert_eq!(
            self::heat(&["T: / B: / T0: @: B@: P: A:".to_owned()]),
            Heat::default(),
            "no numbers, nothing read"
        );
    }

    /// Proves the row `POSITION_REGEX` of docs/prusalink-audit.md: the head's position is
    /// read from the part before `Count`, and the printer's own position reports are noticed.
    #[test]
    fn a_position_report_is_read() {
        let line = "X:10.00 Y:20.50 Z:0.15 E:0.00 Count X: 800 Y:1640 Z:60";
        assert_eq!(heat(&[line.to_owned()]).position_mm, Some([10.0, 20.5, 0.15]));
        assert!(routine(line));
        let screen = Screen::default();
        assert!(screen.now().auto_position.is_none());
        screen.hear(line);
        assert!(screen.now().auto_position.is_some());
        assert_eq!(heat(&["X: Y: Z: E: Count X:".to_owned()]).position_mm, None);
    }

    /// Proves the row `FAN_REGEX` of docs/prusalink-audit.md: both fans' speeds are read,
    /// and the printer's own fan reports are noticed.
    #[test]
    fn a_fan_report_is_read() {
        let line = "E0:3500 RPM PRN1:4800 RPM E0@:255 PRN1@:200";
        let fans = heat(&[line.to_owned()]);
        assert_eq!((fans.hotend_fan_rpm, fans.print_fan_rpm), (Some(3500), Some(4800)));
        assert!(routine(line));
        let screen = Screen::default();
        screen.hear(line);
        assert!(screen.now().auto_fans.is_some());
        assert_eq!(heat(&["E0: RPM PRN1: RPM E0@: PRN1@:".to_owned()]), Heat::default());
    }

    /// Proves the row `HEATING_REGEX`, `HEATING_HOTEND_REGEX` of docs/prusalink-audit.md:
    /// the lines the printer says while an `M109` or `M190` waits give the temperatures and
    /// are told, and the wait goes on to its `ok`.
    #[test]
    fn the_heating_lines_while_waiting_are_read() {
        let mut heating = heat(&["T:180.3 E:0 B:58.0".to_owned()]);
        assert_eq!((heating.nozzle_c, heating.bed_c), (Some(180.3), Some(58.0)));
        heating = heat(&["T:181.5 E:0 W:?".to_owned()]);
        assert_eq!((heating.nozzle_c, heating.bed_c), (Some(181.5), None));
        assert_eq!(parse("T:180.3 E:0 W:?"), Reply::Other("T:180.3 E:0 W:?".into()));
        assert!(long_running("M109 S215") && long_running("M190 S60") && heats("M109 S215"));
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut fake = FakeMarlin::new(accepted.clone());
        fake.errors = vec![
            ("M109 S215".into(), "T:180.3 E:0 W:?".into()),
            ("M190 S60".into(), "T:215.0 E:0 B:58.0".into()),
        ];
        let mut printer = quick(fake);
        let mut events = Vec::new();
        printer.send(["M109 S215", "M190 S60", "G28"], &mut |e| events.push(e)).unwrap();
        assert_eq!(*accepted.lock().unwrap(), ["M109 S215", "M190 S60", "G28"]);
        assert!(events.contains(&Event::Said("T:180.3 E:0 W:?".into())), "{events:?}");
        assert!(events.contains(&Event::Said("T:215.0 E:0 B:58.0".into())), "{events:?}");
        assert!(!events.iter().any(|e| matches!(e, Event::Error(_) | Event::Resent(_))));
    }

    /// Proves the row `CONFIRMATION_REGEX` of docs/prusalink-audit.md: `ok`, alone or with a
    /// report after it, confirms a line; `Done saving file.` (the answer to `M29`, which we
    /// never send) is plain text and confirms nothing, so every line still gets its own `ok`.
    #[test]
    fn only_an_ok_confirms_a_line() {
        assert_eq!(parse("ok"), Reply::Ok);
        assert_eq!(parse("ok T:210.0 /210.0 B:60.0 /60.0"), Reply::Ok);
        assert_eq!(parse("Done saving file."), Reply::Other("Done saving file.".into()));
        let (accepted, events, _) = hearing(&[("G28", "Done saving file.")]);
        assert_eq!(accepted, WHOLE_JOB);
        assert!(events.contains(&Event::Said("Done saving file.".into())));
        assert_eq!(events.last(), Some(&Event::Progress { done: 5, total: 5 }));
    }

    /// Proves the row `RESEND_REGEX` of docs/prusalink-audit.md: `Resend: 5` (and without
    /// the space) asks for line 5 again; the line is sent again, and nothing is lost.
    /// Something that is no number is no request.
    #[test]
    fn a_resend_request_is_read_and_followed() {
        assert_eq!(parse("Resend: 5"), Reply::Resend(5));
        assert_eq!(parse("Resend:5"), Reply::Resend(5));
        assert_eq!(parse("Resend: N"), Reply::Other("Resend: N".into()));
        let mut fake = FakeMarlin::new(Arc::default());
        fake.spoil = vec![2];
        let accepted = fake.accepted.clone();
        let mut printer = quick(fake);
        let mut events = Vec::new();
        printer.send(GCODE.lines(), &mut |e| events.push(e)).unwrap();
        assert!(events.contains(&Event::Resent(2)), "{events:?}");
        assert_eq!(*accepted.lock().unwrap(), WHOLE_JOB);
    }

    /// Proves the row `REJECTION_REGEX` of docs/prusalink-audit.md: a code it does not know
    /// (Marlin's and Prusa's words) and a refused cold extrusion are told, stop nothing and
    /// are no fault.
    #[test]
    fn a_rejected_code_is_told_and_the_job_goes_on() {
        let lines = [
            "echo:Unknown command: \"X1\"",
            "echo:Unknown command: \"\"",
            "Unknown M code: M1234",
            "echo: cold extrusion prevented",
        ];
        for line in lines {
            assert!(matches!(parse(line), Reply::Other(_)), "{line}");
            assert!(!fatal(line) && needs_someone(line).is_none(), "{line}");
        }
        let (accepted, events, screen) = hearing(&[
            ("G28", lines[0]),
            ("M104 S210", lines[1]),
            ("G1 X10 Y10", lines[2]),
            ("G1 X20 Y10 E1.5", lines[3]),
        ]);
        assert_eq!(accepted, WHOLE_JOB);
        for line in lines {
            assert!(events.contains(&Event::Said(line.into())), "{line}");
        }
        assert!(!events.iter().any(|e| matches!(e, Event::Error(_))), "{events:?}");
        assert_eq!(screen.now().fault, None);
        assert_eq!(screen.now().attention, None);
    }

    /// Proves the row `BUSY_REGEX` of docs/prusalink-audit.md: `echo:busy: processing` is
    /// waited for: no resend, no asking after the `ok`, nothing told, the job goes on.
    #[test]
    fn echo_busy_processing_is_waited_for() {
        assert_eq!(parse("echo:busy: processing"), Reply::Busy);
        let busy = "echo:busy: processing";
        let (accepted, events, screen) = hearing(&[("G28", busy), ("G28", busy), ("G28", busy)]);
        assert_eq!(accepted, WHOLE_JOB);
        assert!(!events.iter().any(|e| matches!(e, Event::Resent(_) | Event::Error(_))));
        assert!(!events.iter().any(|e| matches!(e, Event::Said(t) if t.contains("busy") || t.contains("No answer"))));
        assert!(!screen.waiting.load(Ordering::Relaxed), "busy is not waiting for its knob");
    }

    /// Proves the row `ATTENTION_REGEX` of docs/prusalink-audit.md: `echo:busy: paused for
    /// user` is read apart from plain busy: its screen says it waits for its knob until it
    /// speaks again, and the sender keeps waiting for the `ok` meanwhile.
    #[test]
    fn echo_busy_paused_for_user_is_noticed() {
        assert_eq!(parse("echo:busy: paused for user"), Reply::PausedForUser);
        let screen = Screen::default();
        screen.hear("echo:busy: paused for user");
        assert!(screen.waiting.load(Ordering::Relaxed));
        screen.hear("ok");
        assert!(!screen.waiting.load(Ordering::Relaxed));
        let (accepted, events, _) = hearing(&[("G28", "echo:busy: paused for user")]);
        assert_eq!(accepted, WHOLE_JOB);
        assert!(!events.iter().any(|e| matches!(e, Event::Resent(_) | Event::Error(_))));
    }

    /// Proves the row `PAUSE_PRINT_REGEX` … `NOT_READY_REGEX` of docs/prusalink-audit.md:
    /// each of the printer's host actions (with Prusa's space after the slashes) is read as
    /// what it is and changes its screen state as the row says.
    #[test]
    fn the_host_actions_of_the_printers_menu_are_followed() {
        for (line, action) in [
            ("// action:pause", Action::Pause),
            ("// action:paused", Action::Paused),
            ("// action:resume", Action::Resume),
            ("// action:resumed", Action::Resume),
            ("// action:cancel", Action::Cancel),
            ("// action:ready", Action::Ready),
            ("// action:not_ready", Action::NotReady),
        ] {
            assert_eq!(parse(line), Reply::Action(action), "{line}");
        }
        let screen = Screen::default();
        screen.hear("// action:pause");
        assert!(screen.now().paused);
        screen.hear("// action:resume");
        assert!(!screen.now().paused);
        screen.hear("// action:paused");
        assert!(screen.now().paused);
        screen.hear("// action:resumed");
        assert!(!screen.now().paused);
        screen.hear("// action:pause");
        screen.hear("// action:cancel");
        assert!(screen.now().cancelled && !screen.now().paused);
        screen.hear("// action:ready");
        assert!(screen.now().ready_said);
        screen.hear("// action:not_ready");
        assert!(!screen.now().ready_said);
    }

    /// Proves the row `PP_RECOVER_REGEX`, `PP_AUTO_RECOVER_REGEX` of docs/prusalink-audit.md:
    /// both messages say a print waits to be resumed after a power cut.
    #[test]
    fn a_print_brought_back_after_a_power_cut_is_noticed() {
        for line in ["// action:uvlo_recovery_ready", "// action:uvlo_auto_recovery_ready"] {
            assert_eq!(parse(line), Reply::Action(Action::PowerLoss), "{line}");
            assert!(screen_after(line).power_loss, "{line}");
        }
        assert!(!screen_after("// action:ready").power_loss);
    }

    /// Proves the row `ERROR_REGEX` of docs/prusalink-audit.md: a halted printer and a
    /// stopped one are read as errors of the print, told, and leave a fault on its screen.
    #[test]
    fn a_halted_or_stopped_printer_is_a_fault() {
        let halted = "Error:Printer halted. kill() called!";
        let stopped = "Error:Printer stopped due to errors. Fix the error and use M999 to restart.";
        for line in [halted, stopped] {
            assert!(matches!(parse(line), Reply::Error(_)), "{line}");
            let state = screen_after(line);
            assert_eq!(state.fault.as_deref(), Some(line.trim_start_matches("Error:")), "{line}");
            let (accepted, events, screen) = hearing(&[("G1 X10 Y10", line)]);
            assert_eq!(accepted, WHOLE_JOB);
            assert!(events.contains(&Event::Error(line.trim_start_matches("Error:").into())), "{events:?}");
            assert!(screen.now().fault.is_some());
        }
    }

    /// Proves the row `ERROR_REASON_REGEX` of docs/prusalink-audit.md: every wording
    /// PrusaLink's pattern takes (heaters off by MINTEMP or MAXTEMP, of the nozzle or the
    /// bed, with or without the `0: `; thermal runaway of either heater, with or without
    /// its preheat note) is a fault on the printer's screen. The lines are made from the
    /// pattern's own parts.
    #[test]
    fn every_reason_the_printer_stops_for_is_a_fault() {
        let mut lines = Vec::new();
        for first in ["", "0: "] {
            for kind in ["MINTEMP", "MAXTEMP"] {
                for bed in ["", "BED "] {
                    lines.push(format!("Error:{first}Heaters switched off. {kind} {bed}triggered!"));
                }
            }
        }
        for who in ["", " HOTEND", " HEATBED"] {
            for preheat in ["", " (PREHEAT HOTEND)", " ( PREHEAT HEATBED)"] {
                lines.push(format!("Error:{who} THERMAL RUNAWAY{preheat}"));
            }
        }
        assert_eq!(lines.len(), 8 + 9);
        for line in &lines {
            assert!(fatal(line.trim_start_matches("Error:")), "{line}");
            let state = screen_after(line);
            assert_eq!(state.fault.as_deref(), Some(line.trim_start_matches("Error:").trim()), "{line}");
        }
    }

    /// Proves the row `ATTENTION_REASON_REGEX` of docs/prusalink-audit.md: a failed bed
    /// levelling (both reasons) puts what it says on the screen as something for someone,
    /// and is no fault.
    #[test]
    fn a_failed_bed_levelling_needs_someone() {
        for line in [
            "Bed leveling failed. Sensor triggered too high",
            "Bed leveling failed. Sensor didn't trigger. Debris on nozzle? Waiting for reset.",
        ] {
            let state = screen_after(line);
            assert_eq!(state.attention.as_deref(), Some(line), "{line}");
            assert_eq!(state.fault, None, "{line}");
        }
    }

    /// Proves the row `FAN_ERROR_REGEX` of docs/prusalink-audit.md: each of the three fans
    /// that is slower than it should be puts it on the screen as something for someone.
    /// The lines are made from the pattern's text.
    #[test]
    fn a_slow_fan_needs_someone() {
        let template = "Extruder/Hotend/Print fan speed is lower than expected";
        let (names, rest) = template.split_once(" fan").unwrap();
        for name in names.split('/') {
            let line = format!("{name} fan{rest}");
            let state = screen_after(&line);
            assert_eq!(state.attention.as_deref(), Some(line.as_str()), "{line}");
            assert_eq!(state.fault, None);
        }
    }

    /// Proves the row `TM_ERROR_LOG_REGEX`, `TM_ERROR_CLEARED` of docs/prusalink-audit.md:
    /// the thermal model's error is put on the screen as something for someone, and goes
    /// when the printer says it cleared. Its deviation log line is only shown.
    #[test]
    fn the_thermal_model_error_comes_and_goes() {
        let screen = Screen::default();
        screen.hear("TM: error |3.50|>2.00");
        assert_eq!(screen.now().attention, None, "only a log of the deviation");
        screen.hear("TM: error triggered!");
        assert_eq!(screen.now().attention.as_deref(), Some("TM: error triggered!"));
        assert_eq!(screen.now().fault, None);
        screen.hear("TM: error cleared");
        assert_eq!(screen.now().attention, None);
    }

    /// Proves the row `MBL_REGEX` of docs/prusalink-audit.md: the answer to `M420`, a Prusa
    /// mesh (front row last) or "not active", is read, through the printer as a command.
    #[test]
    fn the_answer_to_m420_is_read() {
        let mut fake = FakeMarlin::new(Arc::default());
        fake.errors = [
            "Num X,Y: 3,3",
            "Z search height: 5.00",
            "Measured points:",
            "  0.10000  0.11000  0.12000",
            "  0.05000  0.06000  0.07000",
            " -0.01000  0.00000  0.01000",
        ]
        .iter()
        .map(|l| ("M420".to_owned(), l.to_string()))
        .collect();
        let mut printer = quick(fake);
        let answer = printer.command("M420").unwrap();
        let mesh = mesh(&answer).unwrap();
        assert_eq!(mesh.rows.len(), 3);
        assert_eq!(mesh.rows[0], vec![Some(-0.01), Some(0.0), Some(0.01)]);
        assert_eq!(mesh.rows[2][2], Some(0.12));
        assert_eq!(super::mesh(&said("Mesh bed leveling not active.\nok")), None);
    }

    /// Proves the row `OPEN_RESULT_REGEX` of docs/prusalink-audit.md: `File opened: … Size:
    /// …` gives the file and the card; a refused open (`open failed, File: …`) is the SD
    /// card's trouble, told, and no fault of the print.
    #[test]
    fn a_file_opened_or_not_is_read() {
        let state = screen_after("File opened: BENCHY~1.GCO Size: 812345");
        assert_eq!((state.sd_opened.as_deref(), state.sd_card), (Some("BENCHY~1.GCO"), Some(true)));
        let failed = "open failed, File: BENCHY~1.GCO.";
        let state = screen_after(failed);
        assert_eq!((state.sd_opened, state.fault), (None, None));
        let failed = "Error:open failed, File: BENCHY~1.GCO.";
        assert!(harmless_error(failed.trim_start_matches("Error:")));
        let (accepted, events, _) = hearing(&[("G28", failed)]);
        assert_eq!(accepted, WHOLE_JOB);
        assert!(events.contains(&Event::Said(format!("Error: {}", failed.trim_start_matches("Error:")))));
        assert!(!events.iter().any(|e| matches!(e, Event::Error(_))), "{events:?}");
    }

    /// A Prusa MK3 in memory: connected, with the firmware known, and answering `M27` as
    /// given.
    fn mk3_answering_m27(answer: &[&str]) -> Printer<FakeMarlin> {
        let mut fake = FakeMarlin::new(Arc::default());
        fake.errors = answer.iter().map(|l| ("M27 P".to_owned(), l.to_string())).collect();
        let mut printer = quick(fake);
        printer.firmware = Some("Prusa-Firmware 3.14.1-8237".to_owned());
        printer
    }

    /// Proves the row `FILE_OPEN_REGEX`, `START_PRINT_REGEX` of docs/prusalink-audit.md: a
    /// print started from the printer's menu: `echo:enqueing "M24"` says it prints, at once,
    /// as PrusaLink takes it (`special_commands.py` `handle_start`: PRINTING); `File opened`
    /// gives its file; `M27 P` its short path and how far it is.
    #[test]
    fn a_print_started_at_the_printer_is_seen_at_once() {
        let mut printer = mk3_answering_m27(&["/BENCHY~1.GCO", "SD printing byte 1234/567890", "0:05"]);
        let screen = printer.waiting();
        screen.hear("echo:enqueing \"M23 BENCHY~1.GCO\"");
        assert!(!screen.now().sd_started);
        screen.hear("echo:enqueing \"M24\"");
        assert!(screen.now().sd_started, "printing, at once");
        screen.hear("File opened: BENCHY~1.GCO Size: 567890");
        assert_eq!(screen.now().sd_opened.as_deref(), Some("BENCHY~1.GCO"));
        let status = printer.follow_sd().unwrap().unwrap();
        assert!(!screen.now().sd_started, "M27 P says so from now on");
        assert_eq!(status.file.as_deref(), Some("BENCHY~1.GCO"));
        assert_eq!((status.done, status.total, status.elapsed_s), (1234, 567890, Some(5 * 60)));
    }

    /// Proves the row `PRINT_DONE_REGEX` of docs/prusalink-audit.md: `Done printing file`
    /// ends it at once, as PrusaLink takes it (`handle_done`: FINISHED): the whole file done.
    #[test]
    fn the_end_of_a_print_from_the_card_is_seen_at_once() {
        let mut printer = mk3_answering_m27(&["/BENCHY~1.GCO", "SD printing byte 100/200", "0:05"]);
        assert!(printer.follow_sd().unwrap().is_some());
        let screen = printer.waiting();
        screen.hear("Done printing file");
        assert!(screen.now().sd_done);
        let last = printer.sd_finished().expect("its last report");
        assert_eq!((last.done, last.total), (200, 200));
        assert!(printer.sd.is_none() && !screen.now().sd_done);
    }

    /// Proves the row `PRINTER_BOOT_REGEX` of docs/prusalink-audit.md: `start` wipes what
    /// was on its screen, and a send in progress is broken off ("the printer restarted").
    #[test]
    fn a_restarted_printer_is_noticed() {
        let screen = Screen::default();
        screen.hear("Error:Printer halted. kill() called!");
        screen.hear("// action:cancel");
        assert!(screen.now().fault.is_some() && screen.now().cancelled);
        screen.hear("start");
        assert_eq!(screen.now(), ScreenState::default());
        let mut fake = FakeMarlin::new(Arc::default());
        fake.restart_after = Some("M104 S210".to_owned());
        let mut printer = quick(fake);
        let error = printer.send(GCODE.lines(), &mut |_| {}).unwrap_err();
        assert!(error.to_string().contains("restarted"), "{error}");
    }

    /// Proves the row `POWER_PANIC_REGEX` of docs/prusalink-audit.md: `INT4` (the power
    /// went, the print was saved) is put on the screen as something for someone; no fault.
    #[test]
    fn a_power_failure_is_noticed() {
        let state = screen_after("INT4");
        assert_eq!(state.attention.as_deref(), Some("Power failure: the print was saved"));
        assert_eq!(state.fault, None);
    }

    /// Proves the row `LCD_UPDATE_REGEX` of docs/prusalink-audit.md: `LCD status changed`
    /// is counted for PrusaLink's own lines on its screen (crate::lcd: the firmware's own
    /// message stays 10 s); a routine line otherwise, nothing told as a problem.
    #[test]
    fn lcd_status_changed_is_counted_for_the_screen() {
        assert!(routine("LCD status changed"));
        assert!(matches!(parse("LCD status changed"), Reply::Other(_)));
        assert_eq!(screen_after("LCD status changed"), ScreenState { lcd_changed: 1, ..ScreenState::default() });
        let (accepted, events, _) = hearing(&[("G28", "LCD status changed")]);
        assert_eq!(accepted, WHOLE_JOB);
        assert!(!events.iter().any(|e| matches!(e, Event::Error(_) | Event::Resent(_))));
    }

    /// Proves the row `M27_OUTPUT_REGEX` of docs/prusalink-audit.md: each answer of the
    /// printer to `M27` (its file, `Not SD printing`, `Print saved`, `SD print paused`, `SD
    /// printing byte a/b` and the time after it) is read as what it says.
    #[test]
    fn every_answer_to_m27_is_read() {
        let row = "M27_OUTPUT_REGEX";
        let read = |text: &str| sd_status(&said(text));
        assert_eq!(read("Not SD printing\nok"), Some(None), "{row}: no print");
        assert_eq!(read("Print saved\nok"), Some(None), "{row}: a host's print, not the card's");
        assert!(read("SD print paused\nok").unwrap().unwrap().paused, "{row}: paused");
        let printing = read("/PARTS/BOX~1.GCO\nSD printing byte 1234/567890\n1:05\nok").unwrap().unwrap();
        assert_eq!(printing.file.as_deref(), Some("BOX~1.GCO"), "{row}: its file");
        assert_eq!((printing.done, printing.total), (1234, 567890), "{row}: the bytes");
        assert_eq!(printing.elapsed_s, Some(3900), "{row}: the time");
        assert!(!printing.paused, "{row}");
    }

    /// Proves the row `PAUSED_REGEX` of docs/prusalink-audit.md: `// action:paused` (it
    /// paused itself) is a pause on its screen until it resumes.
    #[test]
    fn it_pausing_itself_is_noticed() {
        assert_eq!(parse("// action:paused"), Reply::Action(Action::Paused));
        let screen = Screen::default();
        screen.hear("// action:paused");
        assert!(screen.now().paused && !screen.now().cancelled);
    }

    /// Proves the row `RESUME_PRINT_REGEX` of docs/prusalink-audit.md: `// action:resume`
    /// (resume from its knob) ends the pause on its screen and clears what needed someone.
    #[test]
    fn resuming_from_its_knob_is_noticed() {
        assert_eq!(parse("// action:resume"), Reply::Action(Action::Resume));
        let screen = Screen::default();
        screen.hear("// action:pause");
        screen.hear("TM: error triggered!");
        screen.hear("// action:resume");
        assert!(!screen.now().paused);
        assert_eq!(screen.now().attention, None);
    }

    /// Proves the row `RESUMED_REGEX` of docs/prusalink-audit.md: `// action:resumed` ends
    /// the pause on its screen, like `resume`.
    #[test]
    fn it_resuming_is_noticed() {
        assert_eq!(parse("// action:resumed"), Reply::Action(Action::Resume));
        let screen = Screen::default();
        screen.hear("// action:paused");
        assert!(screen.now().paused);
        screen.hear("// action:resumed");
        assert!(!screen.now().paused);
    }

    /// A fake printer that also keeps every line the sender wrote to it, as it went on the
    /// wire (the fake itself does not keep `M110`).
    struct Tapped {
        fake: FakeMarlin,
        wrote: Arc<Mutex<Vec<String>>>,
        partial: Vec<u8>,
    }

    impl Read for Tapped {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.fake.read(buf)
        }
    }

    impl Write for Tapped {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.partial.extend_from_slice(buf);
            while let Some(end) = self.partial.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.partial.drain(..=end).collect();
                self.wrote.lock().unwrap().push(String::from_utf8_lossy(&line).trim().to_owned());
            }
            self.fake.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Connecting sends `M110 N0` first, as line 0 with its checksum, even when the sender
    /// had counted on to another number; the next line is number 1. (The connect itself
    /// waits 2 s for a quiet printer.)
    /// Proves the row `M110 N0` of docs/prusalink-audit.md.
    #[test]
    fn connecting_sets_the_line_numbers_back_with_m110_n0() {
        let wrote: Arc<Mutex<Vec<String>>> = Arc::default();
        let link = Tapped { fake: FakeMarlin::new(Arc::default()), wrote: wrote.clone(), partial: Vec::new() };
        let mut printer = Printer::new(link);
        printer.hurry();
        printer.set_next_number(7);
        printer.connect(&mut |_| {}).unwrap();
        printer.command("M115").unwrap();
        let wrote = wrote.lock().unwrap().clone();
        // M400 first, unnumbered, as PrusaLink does before an M110 (proves the row `M400`).
        assert_eq!(wrote[0], "M400", "{wrote:?}");
        assert_eq!(wrote[1], numbered(0, "M110 N0"), "{wrote:?}");
        assert_eq!(wrote[2], numbered(1, "M115"), "the count goes on from 0: {wrote:?}");
    }

    /// Prusa's MK3 is told a host is there with `M79 S"PC"` every 12 s: at once the first
    /// time, not again before the 12 s are over, again after them. Other firmware gets none.
    /// Proves the rows `M79 S"PL"` and `M79` of docs/prusalink-audit.md.
    #[test]
    fn the_host_says_it_is_there_every_12_s() {
        assert_eq!(HOST_KEEPALIVE, Duration::from_secs(12));
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut printer = quick(FakeMarlin::new(accepted.clone()));
        printer.firmware = Some("Prusa-Firmware 3.14.1-8237".to_owned());
        assert!(printer.is_prusa_mk3());
        let m79s = || accepted.lock().unwrap().iter().filter(|c| c.starts_with("M79")).cloned().collect::<Vec<_>>();
        printer.keep_host_alive(&mut |_| {}).unwrap();
        assert_eq!(m79s(), ["M79 S\"PC\""], "at once, as PrusaLink working for Prusa Connect");
        printer.keep_host_alive(&mut |_| {}).unwrap();
        assert_eq!(m79s().len(), 1, "not again within 12 s");
        // 11 s since the last: still not; 12 s: again.
        let ago = |s: u64| Instant::now().checked_sub(Duration::from_secs(s));
        printer.last_m79 = ago(11);
        printer.keep_host_alive(&mut |_| {}).unwrap();
        assert_eq!(m79s().len(), 1, "11 s: not yet");
        printer.last_m79 = ago(12);
        printer.keep_host_alive(&mut |_| {}).unwrap();
        assert_eq!(m79s().len(), 2, "12 s: again");
        // Other firmware: nothing.
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut fake = FakeMarlin::new(accepted.clone());
        fake.m115 = "FIRMWARE_NAME:Marlin 2.1.2.1 (Jun 1 2024) MACHINE_TYPE:Ender-3 EXTRUDER_COUNT:1".into();
        let mut other = quick(fake);
        other.command("M115").unwrap();
        other.keep_host_alive(&mut |_| {}).unwrap();
        assert!(!accepted.lock().unwrap().iter().any(|c| c.starts_with("M79")));
    }
}
