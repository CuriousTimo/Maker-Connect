//! PrusaLink's own lines on the screen of Prusa's MK3 family, as PrusaLink shows them: its
//! `printer_adapter/lcd_printer.py` and `structures/carousel.py`, followed to the letter (read
//! 2026-11-10). Screens (printing, an error, a transfer, ready, idle, a power cut to recover,
//! "please wait") each have a priority; the enabled ones of the highest priority are shown in
//! turn, cut into lines of 19 characters that scroll; a one-off message goes first. Each line
//! goes as `M117 ~text`; a screen that comes up plays its chime once (`M300`). While the
//! firmware shows a message of its own (`LCD status changed` not caused by ours), nothing is
//! shown for 10 s.
//!
//! The agent looks once per report (PrusaLink's thread wakes every 0.2 s or when a line is
//! due): a line stays at least its time, a little longer at most. Time is passed in, so
//! tests can run it.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

/// The chimes (`lcd_printer.py`).
pub const WELCOME_CHIME: &[&str] =
    &["M300 P100 S3200", "M300 P25 S0", "M300 P25 S4800", "M300 P75 S0", "M300 P25 S4800"];
pub const ERROR_CHIME: &[&str] = &["M300 P600 S5"];
pub const UPLOAD_CHIME: &[&str] = &["M300 P14 S50"];

const ERROR_GRACE: f64 = 15.0;
const NETWORK_ERROR_GRACE: f64 = 20.0;
/// `const.py`.
const FW_MESSAGE_TIMEOUT: f64 = 10.0;
const SLEEP_SCREEN_TIMEOUT: f64 = 20.0;

const RECOVERY_PRIORITY: i32 = 60;
const PRINT_PRIORITY: i32 = 50;
const WIZARD_PRIORITY: i32 = 40;
const ERROR_PRIORITY: i32 = 30;
const UPLOAD_PRIORITY: i32 = 20;
const READY_PRIORITY: i32 = 11;
const IDLE_PRIORITY: i32 = 10;

/// What is wrong, as PrusaLink's conditions name it (`conditions.py` and the Connect SDK's),
/// with their priorities: the worst one is shown. The agent never meets some of them (it
/// looks at no network hardware, takes only the MK3 family, is never re-registered for a new
/// printer, has no DNS of its own): kept so the list is PrusaLink's whole.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Condition {
    Device,
    Phy,
    Lan,
    Id,
    Upgraded,
    Fw,
    Sn,
    JobId,
    Internet,
    Http,
    Token,
    Api,
}

impl Condition {
    fn priority(self) -> i32 {
        match self {
            Condition::Device => 1020,
            Condition::Phy => 1010,
            Condition::Lan => 1000,
            Condition::Id => 550,
            Condition::Fw => 540,
            Condition::Sn => 530,
            Condition::JobId => 520,
            Condition::Upgraded => 500,
            Condition::Internet => 130,
            Condition::Http => 120,
            Condition::Token => 110,
            Condition::Api => 100,
        }
    }

    /// `ERROR_MESSAGES`.
    fn message(self) -> &'static str {
        match self {
            Condition::Id => "Unsupported printer",
            Condition::Fw => "Err unsupported FW",
            Condition::Sn => "Err obtaining S/N",
            Condition::Upgraded => "Upgraded - re-reg.",
            Condition::JobId => "Err reading job id",
            Condition::Http => "HTTP error 5xx",
            Condition::Token => "Error bad token",
            Condition::Api => "HTTP error 4xx",
            Condition::Internet => "No Internet access",
            Condition::Lan => "No LAN access",
            Condition::Phy => "No usable NIC",
            Condition::Device => "No network hardware",
        }
    }

    /// Tracked as a network condition (`NET_TRACKER`: the device tree and the internet's).
    fn network(self) -> bool {
        !matches!(self, Condition::Id | Condition::Upgraded | Condition::Fw | Condition::Sn | Condition::JobId)
    }

    /// The worst of these (the highest priority).
    pub fn worst(all: &[Condition]) -> Option<Condition> {
        all.iter().copied().max_by_key(|c| c.priority())
    }
}

/// A transfer's direction (`FROM_TRANSFER_TYPES`, `TO_TRANSFER_TYPES`): the agent only
/// fetches (a job's file from the hub).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    From,
    To,
}

/// How things are, as the screens are decided by (`whats_going_on`).
#[derive(Debug, Clone, Default)]
pub struct Situation {
    /// A job in progress: its file's name.
    pub printing: Option<String>,
    /// Going back to where a power cut stopped it ("Preparing recovery").
    pub rewinding: bool,
    /// Printing as such (not paused, finished, stopped): errors play no chime then.
    pub state_printing: bool,
    /// Paused, finished or stopped (PrusaLink's other printing states).
    pub state_after_printing: bool,
    /// Ready for the next job (READY).
    pub ready: bool,
    /// A print a power cut stopped waits to be resumed.
    pub recovery_ready: bool,
    /// What is wrong now.
    pub conditions: Vec<Condition>,
    /// Setting up is not done: what to show (PrusaLink: `Go: <ip>`).
    pub wizard: Option<String>,
    /// A transfer: how far (percent) and which way; and, once over, whether it finished.
    pub transfer: Option<(f64, Direction)>,
    pub transfer_finished: bool,
    /// Its address on the network, if it has one (LAN).
    pub ip: Option<String>,
    /// The speed set on the printer (`M220`): 42 shows the answer.
    pub speed: Option<f32>,
    /// Network errors chime (PrusaLink's setting `network_error_chime`).
    pub network_error_chime: bool,
}

/// A line to show and the chime to play (`LCDLine`).
#[derive(Debug, Clone)]
pub struct Line {
    pub text: String,
    delay: f64,
    resets_idle: bool,
    pub chime: Vec<&'static str>,
    ends_at: Instant,
}

impl Line {
    fn new(text: String, delay: f64, resets_idle: bool, now: Instant) -> Self {
        Line { text, delay, resets_idle, chime: Vec::new(), ends_at: now + secs(delay) }
    }

    /// The delay counts from when it was shown.
    fn reset_end(&mut self, now: Instant) {
        self.ends_at = now + secs(self.delay);
    }
}

fn secs(s: f64) -> Duration {
    Duration::from_secs_f64(s.max(0.0))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Id {
    Print,
    Wizard,
    Wait,
    Error,
    Upload,
    Ready,
    Idle,
    Recovery,
}

const IDS: [Id; 8] = [Id::Print, Id::Wizard, Id::Wait, Id::Error, Id::Upload, Id::Ready, Id::Idle, Id::Recovery];

/// A screen (`Screen`).
#[derive(Debug, Clone)]
struct Screen {
    resets_idle: bool,
    chime: &'static [&'static str],
    conditions: String,
    changed: bool,
    text: String,
    scroll_delay: f64,
    first_line_extra: f64,
    scroll_amount: usize,
    last_line_extra: f64,
    priority: i32,
    order: i32,
    to_chime: bool,
}

impl Screen {
    fn new(resets_idle: bool, chime: &'static [&'static str], order: i32) -> Self {
        Screen {
            resets_idle,
            chime,
            conditions: String::new(),
            changed: false,
            text: String::new(),
            scroll_delay: 2.0,
            first_line_extra: 2.0,
            scroll_amount: 10,
            last_line_extra: 1.0,
            priority: 0,
            order,
            to_chime: false,
        }
    }

    /// Its text cut into lines of 19 characters (the 20th is the `~`), scrolling by
    /// `scroll_amount` (`Screen.lines`).
    fn lines(&self, now: Instant) -> Vec<Line> {
        let all: Vec<char> = self.text.chars().collect();
        let mut remaining: &[char] = &all;
        let mut lines = Vec::new();
        while remaining.len() > 19 {
            let last_index = remaining.len() - 19;
            let mut line = Line::new(remaining[..19].iter().collect(), self.scroll_delay, self.resets_idle, now);
            if remaining.len() == all.len() {
                line.delay += self.first_line_extra;
                line.ends_at = now + secs(line.delay);
            }
            remaining = &remaining[self.scroll_amount.min(last_index)..];
            lines.push(line);
        }
        lines.push(Line::new(
            remaining[..remaining.len().min(19)].iter().collect(),
            self.scroll_delay + self.last_line_extra,
            false,
            now,
        ));
        lines
    }
}

/// Where the carousel is in its round of lines (the generator `Carousel._lines`).
#[derive(Debug, Default)]
struct Round {
    started: bool,
    done: bool,
    screens: Vec<Id>,
    at: usize,
    lines: Vec<Line>,
    line: usize,
}

/// The screens and the one-off messages (`Carousel`).
#[derive(Debug)]
struct Carousel {
    screens: Vec<Screen>,
    enabled: HashSet<Id>,
    active_set: HashSet<Id>,
    active: Vec<Id>,
    to_rewind: bool,
    messages: VecDeque<Line>,
    round: Round,
}

impl Carousel {
    fn screen(&mut self, id: Id) -> &mut Screen {
        &mut self.screens[id as usize]
    }

    fn next_in_round(&mut self, now: Instant) -> Option<Line> {
        if self.round.done {
            return None;
        }
        if !self.round.started {
            self.round.started = true;
            self.round.screens = self.active.clone();
            self.round.at = 0;
            self.round.lines = self.round.screens.first().map(|id| self.screens[*id as usize].lines(now)).unwrap_or_default();
            self.round.line = 0;
        }
        loop {
            if self.round.at >= self.round.screens.len() {
                self.round.done = true;
                return None;
            }
            if self.round.line < self.round.lines.len() {
                let mut line = self.round.lines[self.round.line].clone();
                self.round.line += 1;
                if self.to_rewind {
                    self.round.done = true;
                    return None;
                }
                let id = self.round.screens[self.round.at];
                let screen = self.screen(id);
                if screen.to_chime {
                    screen.to_chime = false;
                    line.chime = screen.chime.to_vec();
                }
                line.resets_idle = screen.resets_idle;
                return Some(line);
            }
            self.round.at += 1;
            if let Some(id) = self.round.screens.get(self.round.at) {
                self.round.lines = self.screens[*id as usize].lines(now);
                self.round.line = 0;
            }
        }
    }

    /// The next line to show: a one-off message first (`get_next`).
    fn get_next(&mut self, now: Instant) -> Option<Line> {
        if let Some(message) = self.messages.pop_front() {
            self.set_rewind();
            return Some(message);
        }
        if let Some(line) = self.next_in_round(now) {
            return Some(line);
        }
        self.rewind();
        self.next_in_round(now)
    }

    fn rewind(&mut self) {
        self.to_rewind = false;
        self.round = Round::default();
    }

    fn set_rewind(&mut self) {
        self.to_rewind = true;
    }

    fn add_message(&mut self, line: Line) {
        self.messages.push_back(line);
    }

    fn set_text(&mut self, id: Id, text: &str, scroll_delay: f64, first_line_extra: f64, scroll_amount: usize, last_line_extra: f64) {
        let screen = self.screen(id);
        screen.changed = true;
        screen.text = text.to_owned();
        screen.scroll_delay = scroll_delay;
        screen.first_line_extra = first_line_extra;
        screen.scroll_amount = scroll_amount;
        screen.last_line_extra = last_line_extra;
        self.react();
    }

    fn enable(&mut self, id: Id, silent: bool) {
        if self.enabled.contains(&id) {
            return;
        }
        self.screen(id).to_chime = !silent;
        self.enabled.insert(id);
        self.react();
    }

    fn set_priority(&mut self, id: Id, priority: i32) {
        if self.screen(id).priority == priority {
            return;
        }
        self.screen(id).priority = priority;
        self.react();
    }

    fn disable(&mut self, id: Id) {
        if !self.enabled.remove(&id) {
            return;
        }
        self.react();
    }

    fn is_enabled(&self, id: Id) -> bool {
        self.enabled.contains(&id)
    }

    fn set_to_show(&self) -> HashSet<Id> {
        let Some(max) = self.enabled.iter().map(|id| self.screens[*id as usize].priority).max() else {
            return HashSet::new();
        };
        self.enabled.iter().copied().filter(|id| self.screens[*id as usize].priority == max).collect()
    }

    fn react(&mut self) {
        let new_set = self.set_to_show();
        if new_set != self.active_set || self.active_set.iter().any(|id| self.screens[*id as usize].changed) {
            self.set_rewind();
            for id in &new_set {
                self.screens[*id as usize].changed = false;
            }
            let mut active: Vec<Id> = new_set.iter().copied().collect();
            active.sort_by_key(|id| (self.screens[*id as usize].order, *id));
            self.active = active;
            self.active_set = new_set;
        }
    }
}

/// PrusaLink's `LCDPrinter`: what to show, and when.
#[derive(Debug)]
pub struct Lcd {
    carousel: Carousel,
    started: bool,
    ignore_errors_to: Instant,
    network_error_at: Option<Instant>,
    fw_msg_end_at: Instant,
    idle_from: Instant,
    /// How many `LCD status changed` are its own (one per `M117` sent).
    ignore: u32,
    current: Option<Line>,
}

impl Lcd {
    pub fn new(now: Instant) -> Self {
        let mut screens: Vec<Screen> = IDS
            .iter()
            .map(|id| match id {
                Id::Error => Screen::new(true, ERROR_CHIME, 0),
                Id::Upload => Screen::new(true, UPLOAD_CHIME, 0),
                Id::Wizard => Screen::new(true, WELCOME_CHIME, 0),
                Id::Print => Screen::new(true, &[], 1),
                _ => Screen::new(false, &[], 0),
            })
            .collect();
        for (id, p) in [
            (Id::Print, PRINT_PRIORITY),
            (Id::Wizard, WIZARD_PRIORITY),
            (Id::Error, ERROR_PRIORITY),
            (Id::Upload, UPLOAD_PRIORITY),
            (Id::Ready, READY_PRIORITY),
            (Id::Idle, IDLE_PRIORITY),
            (Id::Recovery, RECOVERY_PRIORITY),
        ] {
            screens[id as usize].priority = p;
        }
        let mut carousel = Carousel {
            screens,
            enabled: HashSet::new(),
            active_set: HashSet::new(),
            active: Vec::new(),
            to_rewind: false,
            messages: VecDeque::new(),
            round: Round::default(),
        };
        let wait: String = (1..8).map(|i| format!("{:<19}", format!("Please wait{}", ".".repeat(i)))).collect();
        carousel.set_text(Id::Wait, &wait, 1.5, 0.0, 19, 0.0);
        carousel.set_text(Id::Ready, "Ready to print", 5.0, 0.0, 10, 0.0);
        carousel.set_text(Id::Recovery, "Ready to recover", 5.0, 0.0, 10, 0.0);
        Lcd {
            carousel,
            started: false,
            ignore_errors_to: now + secs(ERROR_GRACE),
            network_error_at: None,
            fw_msg_end_at: now,
            idle_from: now,
            ignore: 0,
            current: None,
        }
    }

    /// The firmware said `LCD status changed`: ours (one per line sent), or a message of its
    /// own, which is left on its screen for 10 s (`lcd_updated`).
    pub fn lcd_updated(&mut self, now: Instant) {
        if self.ignore > 0 {
            self.ignore -= 1;
        } else {
            self.idle_from = now;
            self.fw_msg_end_at = now + secs(FW_MESSAGE_TIMEOUT);
            self.carousel.set_rewind();
        }
    }

    fn message_and_disable(&mut self, id: Id, message: &str, now: Instant) {
        if !self.carousel.is_enabled(id) {
            return;
        }
        self.carousel.add_message(Line::new(message.to_owned(), 5.0, false, now));
        self.carousel.disable(id);
    }

    fn whats_going_on(&mut self, s: &Situation, now: Instant) {
        // Printing.
        match &s.printing {
            Some(file) => {
                self.carousel.enable(Id::Print, false);
                let safe = file.replace(';', ",:");
                let conditions = format!("{safe}|{}", s.rewinding);
                if self.carousel.screens[Id::Print as usize].conditions != conditions {
                    self.carousel.screen(Id::Print).conditions = conditions;
                    let text = if s.rewinding { "Preparing recovery".to_owned() } else { safe };
                    self.carousel.set_text(Id::Print, &text, 2.0, 2.0, 10, 1.0);
                }
            }
            None => self.carousel.disable(Id::Print),
        }
        // Errors.
        let unfiltered = Condition::worst(&s.conditions);
        let grace_ended = now > self.ignore_errors_to;
        let error = self.filter_http(unfiltered, now);
        if error.is_some() && !grace_ended {
            self.carousel.enable(Id::Wait, false);
        } else {
            self.message_and_disable(Id::Wait, "PrusaLink OK", now);
        }
        match error {
            None => self.message_and_disable(Id::Error, "Errors resolved", now),
            Some(error) if grace_ended => {
                let silent = (error.network() && !s.network_error_chime) || s.state_printing;
                self.carousel.enable(Id::Error, silent);
                let conditions = format!("{}|{error:?}", s.ip.is_some());
                if self.carousel.screens[Id::Error as usize].conditions != conditions {
                    self.carousel.screen(Id::Error).conditions = conditions;
                    let message: String = error.message().chars().take(19).collect();
                    let mut text = format!("{message:<19}");
                    if let Some(ip) = &s.ip {
                        text += &format!("{:<19}", format!("see {ip}"));
                    }
                    self.carousel.set_text(Id::Error, &text, 2.0, 2.0, 19, 8.0);
                }
                let priority = if s.printing.is_some() { 50 } else { ERROR_PRIORITY };
                self.carousel.set_priority(Id::Error, priority);
            }
            Some(_) => {}
        }
        // Setting up.
        match (&s.wizard, &s.ip) {
            (Some(text), Some(_)) => {
                self.carousel.enable(Id::Wizard, false);
                if self.carousel.screens[Id::Wizard as usize].conditions != *text {
                    self.carousel.screen(Id::Wizard).conditions = text.clone();
                    self.carousel.set_text(Id::Wizard, text, 2.0, 2.0, 10, 10.0);
                }
            }
            _ => self.message_and_disable(Id::Wizard, "Setup completed", now),
        }
        // A transfer.
        let upload_priority = if s.state_after_printing { PRINT_PRIORITY + 10 } else { PRINT_PRIORITY };
        self.carousel.set_priority(Id::Upload, upload_priority);
        if let Some((progress, direction)) = s.transfer {
            self.carousel.enable(Id::Upload, false);
            self.carousel.set_text(Id::Upload, &progress_graphic(progress, Some(direction)), 0.5, 0.0, 10, 0.0);
        } else if self.carousel.is_enabled(Id::Upload) {
            let said = if s.transfer_finished { "Transfer finished" } else { "Transfer stopped" };
            self.message_and_disable(Id::Upload, said, now);
        } else {
            self.carousel.disable(Id::Upload);
        }
        // Ready.
        match (&s.ip, s.ready) {
            (Some(ip), true) => {
                self.carousel.enable(Id::Ready, false);
                if self.carousel.screens[Id::Ready as usize].conditions != *ip {
                    self.carousel.screen(Id::Ready).conditions = ip.clone();
                    let text = format!("{:<19}{:<19}", "Ready to print", ip);
                    self.carousel.set_text(Id::Ready, &text, 4.0, 2.0, 19, 5.0);
                }
            }
            _ => self.carousel.disable(Id::Ready),
        }
        // Idle.
        match &s.ip {
            Some(ip) if now.duration_since(self.idle_from) > secs(SLEEP_SCREEN_TIMEOUT) => {
                self.carousel.enable(Id::Idle, false);
                let conditions = format!("{ip}|{:?}", s.speed);
                if self.carousel.screens[Id::Idle as usize].conditions != conditions {
                    self.carousel.screen(Id::Idle).conditions = conditions;
                    if s.speed != Some(42.0) {
                        let text = format!("{:<19}{:<19}", "PrusaLink OK.", ip);
                        self.carousel.set_text(Id::Idle, &text, 2.0, 2.0, 19, 12.0);
                    } else {
                        self.carousel.set_text(
                            Id::Idle,
                            "The Answer to the Great Question... Of Life, the Universe and Everything... Is... Forty-Two.",
                            0.3,
                            2.0,
                            1,
                            5.0,
                        );
                    }
                }
            }
            _ => self.carousel.disable(Id::Idle),
        }
        // A power cut to recover from.
        if s.recovery_ready {
            self.carousel.enable(Id::Recovery, false);
        } else {
            self.carousel.disable(Id::Recovery);
        }
    }

    /// Network errors are kept quiet for their first 20 s (`_filter_http`).
    fn filter_http(&mut self, error: Option<Condition>, now: Instant) -> Option<Condition> {
        let Some(error) = error else {
            self.network_error_at = None;
            return None;
        };
        if error.network() {
            let Some(at) = self.network_error_at else {
                self.network_error_at = Some(now);
                return None;
            };
            if now.duration_since(at) < secs(NETWORK_ERROR_GRACE) {
                return None;
            }
        }
        Some(error)
    }

    fn should_advance(&self, now: Instant) -> bool {
        if now < self.fw_msg_end_at {
            return false;
        }
        match &self.current {
            Some(line) if now < line.ends_at => self.carousel.to_rewind,
            _ => true,
        }
    }

    /// Looks again (PrusaLink's display thread, each time it wakes): the G-code to send now,
    /// each `M117 ~text` with its chime after it; nothing when the line shown stays.
    pub fn tick(&mut self, s: &Situation, now: Instant) -> Vec<String> {
        let mut out = Vec::new();
        if !self.started {
            self.started = true;
            out.extend(self.show(Line::new("PrusaLink started".to_owned(), 5.0, false, now), now));
        }
        self.whats_going_on(s, now);
        if self.should_advance(now) {
            self.current = self.carousel.get_next(now);
            if let Some(line) = self.current.clone() {
                out.extend(self.show(line, now));
            }
        }
        out
    }

    fn show(&mut self, mut line: Line, now: Instant) -> Vec<String> {
        if line.resets_idle {
            self.idle_from = now;
        }
        self.ignore += 1;
        let mut out = vec![format!("M117 ~{}", ascii(&line.text))];
        out.extend(line.chime.iter().map(|c| (*c).to_owned()));
        line.reset_end(now);
        if let Some(current) = self.current.as_mut()
            && current.text == line.text
        {
            current.ends_at = line.ends_at;
        }
        out
    }

    /// The goodbye, when the agent stops (`stop`).
    pub fn stopped(&mut self, now: Instant) -> Vec<String> {
        self.show(Line::new("PrusaLink stopped".to_owned(), 5.0, false, now), now)
    }
}

/// A transfer's bar (`_get_progress_graphic`): `Sync~|:=====95%===>`.
fn progress_graphic(progress: f64, direction: Option<Direction>) -> String {
    let bar_length = 12usize;
    let rough_index = progress / (100.0 / (bar_length as f64 + 2.0));
    let index = (rough_index.floor() as usize).min(bar_length + 1);
    let display_arrow = 0 < index && index < 13;
    let mut background = "=".repeat(index.saturating_sub(1));
    if display_arrow {
        background += ">";
    }
    let background = format!("{background:<bar_length$}");
    let string_progress = format!("{}%", progress.round() as i64);
    let centered = center(&string_progress, bar_length);
    let at = centered.find(&string_progress).unwrap_or(0);
    let mut graphic = match direction {
        Some(Direction::From) => "Sync~|:".to_owned(),
        Some(Direction::To) => "Sync|~:".to_owned(),
        None => "Sync  :".to_owned(),
    };
    graphic += &background[..at.min(background.len())];
    graphic += &string_progress;
    graphic += background.get(at + string_progress.len()..).unwrap_or("");
    graphic
}

/// Python's `str.center`: the extra space on the right when it does not split evenly.
fn center(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        return text.to_owned();
    }
    let pad = width - len;
    // CPython: left = pad / 2 + (pad & width & 1).
    let left = pad / 2 + (pad & width & 1);
    format!("{}{}{}", " ".repeat(left), text, " ".repeat(pad - left))
}

/// Text as the printer can show it (PrusaLink: `unidecode`): accents taken off, anything
/// else not ASCII left out.
fn ascii(text: &str) -> String {
    text.chars()
        .filter_map(|c| {
            if c.is_ascii() {
                return Some(c.to_string());
            }
            let plain = match c {
                'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => "a",
                'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => "A",
                'è' | 'é' | 'ê' | 'ë' => "e",
                'È' | 'É' | 'Ê' | 'Ë' => "E",
                'ì' | 'í' | 'î' | 'ï' => "i",
                'Ì' | 'Í' | 'Î' | 'Ï' => "I",
                'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => "o",
                'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' => "O",
                'ù' | 'ú' | 'û' | 'ü' => "u",
                'Ù' | 'Ú' | 'Û' | 'Ü' => "U",
                'ñ' => "n",
                'Ñ' => "N",
                'ç' => "c",
                'Ç' => "C",
                'ß' => "ss",
                'ý' | 'ÿ' => "y",
                'Ý' => "Y",
                'æ' => "ae",
                'Æ' => "AE",
                '–' | '—' => "-",
                '‘' | '’' => "'",
                '“' | '”' => "\"",
                _ => return None,
            };
            Some(plain.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle(ip: &str) -> Situation {
        Situation { ip: Some(ip.to_owned()), ..Situation::default() }
    }

    /// "PrusaLink started" first; after 20 s without a line of its own, the idle screen:
    /// "PrusaLink OK." and its address, scrolled by 19.
    #[test]
    fn it_starts_and_says_it_is_ok_when_idle() {
        let t0 = Instant::now();
        let mut lcd = Lcd::new(t0);
        let s = idle("192.168.1.50");
        assert_eq!(lcd.tick(&s, t0), ["M117 ~PrusaLink started"]);
        assert!(lcd.tick(&s, t0 + secs(6.0)).is_empty(), "nothing enabled yet");
        let at = t0 + secs(21.0);
        assert_eq!(lcd.tick(&s, at), [format!("M117 ~{:<19}", "PrusaLink OK.")]);
        let next = at + secs(4.1);
        assert_eq!(lcd.tick(&s, next), [format!("M117 ~{:<19}", "192.168.1.50")]);
    }

    /// Printing shows its file (`;` as `,:`); a power cut to recover is shown above it.
    #[test]
    fn a_print_shows_its_file_and_a_power_cut_comes_first() {
        let t0 = Instant::now();
        let mut lcd = Lcd::new(t0);
        let mut s = idle("10.0.0.2");
        s.printing = Some("cube;v2.gcode".to_owned());
        assert_eq!(lcd.tick(&s, t0), ["M117 ~PrusaLink started", "M117 ~cube,:v2.gcode"]);
        s.recovery_ready = true;
        assert_eq!(lcd.tick(&s, t0 + secs(0.1)), ["M117 ~Ready to recover"], "a higher priority rewinds");
    }

    /// An error waits out its grace ("Please wait…"), then shows with its chime, then
    /// "Errors resolved" once it is gone; a network error is quiet for its first 20 s.
    #[test]
    fn an_error_is_shown_after_its_grace_with_its_chime() {
        let t0 = Instant::now();
        let mut lcd = Lcd::new(t0);
        let mut s = idle("10.0.0.2");
        s.conditions = vec![Condition::Fw];
        let first = lcd.tick(&s, t0);
        assert_eq!(first[0], "M117 ~PrusaLink started");
        assert_eq!(first[1], format!("M117 ~{:<19}", "Please wait."));
        let shown = lcd.tick(&s, t0 + secs(16.0));
        assert_eq!(shown[0], "M117 ~PrusaLink OK", "the wait screen goes with a message");
        let shown = lcd.tick(&s, t0 + secs(21.1));
        assert_eq!(shown, [format!("M117 ~{:<19}", "Err unsupported FW"), "M300 P600 S5".to_owned()]);
        s.conditions.clear();
        let gone = lcd.tick(&s, t0 + secs(40.0));
        assert_eq!(gone, ["M117 ~Errors resolved"]);
        // A network error: quiet for 20 s, silent (no chime unless set so).
        let mut lcd = Lcd::new(t0);
        s.conditions = vec![Condition::Http];
        lcd.tick(&s, t0 + secs(16.0));
        assert!(!lcd.carousel.is_enabled(Id::Error), "not in its first 20 s");
        let shown = lcd.tick(&s, t0 + secs(37.0));
        let all = shown.join("|") + &lcd.tick(&s, t0 + secs(43.0)).join("|");
        assert!(all.contains("M117 ~HTTP error 5xx  ") && !all.contains("M300"), "{all}");
    }

    /// A message of the firmware's own stays 10 s; ours do not count (one per line sent).
    #[test]
    fn the_firmwares_own_message_is_left_for_10_s() {
        let t0 = Instant::now();
        let mut lcd = Lcd::new(t0);
        let mut s = idle("10.0.0.2");
        s.ready = true;
        let out = lcd.tick(&s, t0);
        assert_eq!(out, ["M117 ~PrusaLink started".to_owned(), format!("M117 ~{:<19}", "Ready to print")]);
        lcd.lcd_updated(t0);
        lcd.lcd_updated(t0);
        assert_eq!(lcd.ignore, 0, "both ours");
        lcd.lcd_updated(t0 + secs(1.0));
        assert!(lcd.tick(&s, t0 + secs(8.0)).is_empty(), "its own message stays");
        assert!(!lcd.tick(&s, t0 + secs(11.5)).is_empty(), "then ours again");
    }

    /// The transfer bar as PrusaLink draws it.
    #[test]
    fn a_transfer_bar_is_drawn_as_prusalink_draws_it() {
        assert_eq!(progress_graphic(0.0, Some(Direction::From)), "Sync~|:     0%     ");
        assert_eq!(progress_graphic(5.0, Some(Direction::From)), "Sync~|:     5%     ");
        assert_eq!(progress_graphic(50.0, Some(Direction::To)), "Sync|~:====50%     ", "as Python draws it");
        assert_eq!(progress_graphic(95.0, Some(Direction::To)), "Sync|~:====95%=====", "the arrow hidden at 13");
        assert_eq!(progress_graphic(100.0, None), "Sync  :====100%====");
        assert_eq!(center("5%", 12), "     5%     ");
        assert_eq!(ascii("Bénchy – Ø"), "Benchy - O");
    }
}
