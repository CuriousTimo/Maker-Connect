//! Port of PrusaLink `printer_adapter/structures/carousel.py` (`LCDLine`, `Screen`, `Carousel`),
//! "Implements the helper classes for LCD printer".
//!
//! Python's generators (`Screen.lines`, `Carousel._lines`) are state machines here, run one `next`
//! at a time as Python runs them (a generator's body starts at its first `next`, and reads the
//! screen's fields then). `time()` is the `now` the caller passes (seconds); a `Screen` is a
//! [`ScreenId`] into the carousel's `screens`.
//!
//! Differences (all OURS):
//!  1. `self.screens` and the active sets are Python `set`s of objects; their iteration order is the
//!     objects' addresses. Screens of the same priority and the same `order` are taken here in the
//!     order they were given to `Carousel::new`.
//!  2. `Screen.conditions` is a dict; here a `BTreeMap<String, String>` of the values' text.
//!  3. The `ValueError("This screen is not in the carousel")` of `verify_tracked` cannot happen
//!     (a `ScreenId` only comes from `Carousel::new`).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// `Screen` in a `Carousel`'s list
pub type ScreenId = usize;

/// `carousel.py:11 LCDLine`: "Info about the text to show and the chime to play"
#[derive(Debug, Clone, PartialEq)]
pub struct LCDLine {
    pub text: String,
    pub delay: f64,
    pub chime_gcode: Vec<String>,
    pub resets_idle: bool,
    pub ends_at: f64,
}

impl LCDLine {
    /// `LCDLine(text, delay=5.0, resets_idle=False, chime_gcode=None)`
    pub fn new(
        text: &str,
        delay: f64,
        resets_idle: bool,
        chime_gcode: Option<Vec<String>>,
        now: f64,
    ) -> LCDLine {
        LCDLine {
            text: text.to_string(),
            delay,
            chime_gcode: chime_gcode.unwrap_or_default(),
            resets_idle,
            ends_at: now + delay,
        }
    }

    /// `LCDLine(text)`: the defaults
    pub fn plain(text: &str, now: f64) -> LCDLine {
        LCDLine::new(text, 5.0, false, None, now)
    }

    /// `reset_end()`: "the delay is the minimum time the message is shown on screen"
    pub fn reset_end(&mut self, now: f64) {
        self.ends_at = now + self.delay;
    }
}

/// `Screen.lines()`'s generator
#[derive(Debug, Clone, Default)]
struct ScreenLines {
    started: bool,
    remaining_text: Vec<char>,
    finished_loop: bool,
    done: bool,
}

/// `carousel.py:35 Screen`: "A Screen - like an error screen, or an easter egg scrolling screen"
#[derive(Debug, Clone)]
pub struct Screen {
    pub resets_idle: bool,
    pub chime_gcode: Vec<String>,
    pub conditions: BTreeMap<String, String>,
    pub changed: bool,
    pub text: String,
    pub scroll_delay: f64,
    pub first_line_extra: f64,
    pub scroll_amount: usize,
    pub last_line_extra: f64,
    /// "only the things with the highest priority get displayed"
    pub priority: i64,
    /// "if there are more than one, they get ordered by this number"
    pub order: i64,
    pub enabled: bool,
    pub to_chime: bool,
    lines: ScreenLines,
}

impl Screen {
    /// `Screen(resets_idle=True, chime_gcode=None, order=0)`
    pub fn new(resets_idle: bool, chime_gcode: Option<Vec<String>>, order: i64) -> Screen {
        Screen {
            resets_idle,
            chime_gcode: chime_gcode.unwrap_or_default(),
            conditions: BTreeMap::new(),
            changed: false,
            text: String::new(),
            scroll_delay: 2.0,
            first_line_extra: 2.0,
            scroll_amount: 10,
            last_line_extra: 1.0,
            priority: 0,
            order,
            enabled: false,
            to_chime: false,
            lines: ScreenLines::default(),
        }
    }

    /// `Screen.lines()`: "The status display has 19 usable chars (20). Iterating over this cuts the
    /// text into displayable messages (lines) to output, so a scrolling or paginated appearance can
    /// be achieved". Starts a new run of the generator.
    pub fn start_lines(&mut self) {
        self.lines = ScreenLines::default();
    }

    /// `next()` of the generator of `lines()`
    pub fn next_line(&mut self, now: f64) -> Option<LCDLine> {
        if self.lines.done {
            return None;
        }
        if !self.lines.started {
            self.lines.started = true;
            self.lines.remaining_text = self.text.chars().collect();
        }
        let len = self.lines.remaining_text.len() as i64;
        let last_index = len - 19;
        if !self.lines.finished_loop && last_index > 0 {
            let first: String = self.lines.remaining_text[..19].iter().collect();
            let mut line = LCDLine::new(&first, self.scroll_delay, self.resets_idle, None, now);
            let remaining: String = self.lines.remaining_text.iter().collect();
            if remaining == self.text {
                line.delay += self.first_line_extra;
            }
            let actual_scroll_amount = (self.scroll_amount as i64).min(last_index) as usize;
            self.lines.remaining_text = self.lines.remaining_text[actual_scroll_amount..].to_vec();
            return Some(line);
        }
        self.lines.finished_loop = true;
        self.lines.done = true;
        let rest: String = self.lines.remaining_text.iter().take(19).collect();
        Some(LCDLine::new(
            &rest,
            self.scroll_delay + self.last_line_extra,
            false,
            None,
            now,
        ))
    }
}

/// `Carousel._lines()`'s generator
#[derive(Debug, Clone, Default)]
struct CarouselLines {
    started: bool,
    snapshot: Vec<ScreenId>,
    index: usize,
    running: bool,
    done: bool,
}

/// `carousel.py:94 Carousel`: "Manages Screens and spurious messages. Ignores the timing, focuses
/// just on what line and screen to show if asked"
#[derive(Debug)]
pub struct Carousel {
    pub screens: Vec<Screen>,
    pub enabled_screens: BTreeSet<ScreenId>,
    pub active_set: BTreeSet<ScreenId>,
    pub active_screens: Vec<ScreenId>,
    pub current_screen: Option<ScreenId>,
    pub to_rewind: bool,
    pub messages: VecDeque<LCDLine>,
    line_generator: CarouselLines,
}

impl Carousel {
    /// `Carousel(screens)`
    pub fn new(screens: Vec<Screen>) -> Carousel {
        Carousel {
            screens,
            enabled_screens: BTreeSet::new(),
            active_set: BTreeSet::new(),
            active_screens: vec![],
            current_screen: None,
            to_rewind: false,
            messages: VecDeque::new(),
            line_generator: CarouselLines::default(),
        }
    }

    /// `next(self.line_generator)` of `_lines()`: `None` is `StopIteration`
    fn next_from_generator(&mut self, now: f64) -> Option<LCDLine> {
        if self.line_generator.done {
            return None;
        }
        if !self.line_generator.started {
            // `for self.current_screen in copy(self.active_screens)`
            self.line_generator.started = true;
            self.line_generator.snapshot = self.active_screens.clone();
            self.line_generator.index = 0;
            self.line_generator.running = false;
        }
        loop {
            if !self.line_generator.running {
                let Some(&screen) = self.line_generator.snapshot.get(self.line_generator.index)
                else {
                    self.line_generator.done = true;
                    return None;
                };
                self.current_screen = Some(screen);
                self.screens[screen].start_lines();
                self.line_generator.running = true;
            }
            let screen = self.current_screen.expect("a running screen");
            match self.screens[screen].next_line(now) {
                None => {
                    self.line_generator.running = false;
                    self.line_generator.index += 1;
                }
                Some(mut line) => {
                    if self.to_rewind {
                        self.current_screen = None;
                        self.line_generator.done = true;
                        return None;
                    }
                    if self.screens[screen].to_chime {
                        self.screens[screen].to_chime = false;
                        line.chime_gcode = self.screens[screen].chime_gcode.clone();
                    }
                    line.resets_idle = self.screens[screen].resets_idle;
                    return Some(line);
                }
            }
        }
    }

    /// `get_next()`: "Handles giving out lines to show. The spurious messages have priority"
    pub fn get_next(&mut self, now: f64) -> Option<LCDLine> {
        if !self.messages.is_empty() {
            self.set_rewind();
            return self.messages.pop_front();
        }
        if let Some(line) = self.next_from_generator(now) {
            return Some(line);
        }
        self._rewind();
        self.next_from_generator(now) // `None`: "nothing to show"
    }

    /// `_rewind()`: "Re-winds to the start. This updates what lines will get output"
    fn _rewind(&mut self) {
        self.to_rewind = false;
        self.line_generator = CarouselLines::default();
    }

    /// `set_rewind()`: "Next time a line will be requested, the carousel will start from the first
    /// Line on the first Screen"
    pub fn set_rewind(&mut self) {
        self.to_rewind = true;
    }

    /// `add_message(line)`: "Adds a "spurious" message to be displayed. Long ones (over 19 chars)
    /// aren't supported"
    pub fn add_message(&mut self, line: LCDLine) {
        self.messages.push_back(line);
    }

    /// `set_text(screen, text, scroll_delay=2.0, first_line_extra=2.0, scroll_amount=10,
    /// last_line_extra=1.0)`: "Setting text to an active screen rewinds the carousel"
    pub fn set_text(
        &mut self,
        screen: ScreenId,
        text: &str,
        scroll_delay: f64,
        first_line_extra: f64,
        scroll_amount: usize,
        last_line_extra: f64,
    ) {
        let s = &mut self.screens[screen];
        s.changed = true;
        s.text = text.to_string();
        s.scroll_delay = scroll_delay;
        s.first_line_extra = first_line_extra;
        s.scroll_amount = scroll_amount;
        s.last_line_extra = last_line_extra;
        self._react();
    }

    /// `set_text(screen, text)` with the defaults
    pub fn set_text_default(&mut self, screen: ScreenId, text: &str) {
        self.set_text(screen, text, 2.0, 2.0, 10, 1.0);
    }

    /// `enable(screen, silent=False)`
    pub fn enable(&mut self, screen: ScreenId, silent: bool) {
        if self.enabled_screens.contains(&screen) {
            return; // Has no effect
        }
        self.screens[screen].to_chime = !silent;
        self.enabled_screens.insert(screen);
        self._react();
    }

    /// `set_priority(screen, priority)`
    pub fn set_priority(&mut self, screen: ScreenId, priority: i64) {
        if priority == self.screens[screen].priority {
            return; // has no effect
        }
        self.screens[screen].priority = priority;
        self._react();
    }

    /// `disable(screen)`
    pub fn disable(&mut self, screen: ScreenId) {
        if !self.enabled_screens.contains(&screen) {
            return; // has no effect
        }
        self.enabled_screens.remove(&screen);
        self._react();
    }

    /// `is_enabled(screen)`
    pub fn is_enabled(&self, screen: ScreenId) -> bool {
        self.enabled_screens.contains(&screen)
    }

    /// `get_set_to_show()`: "What screens should get shown according to the current state"
    pub fn get_set_to_show(&self) -> BTreeSet<ScreenId> {
        let max_priority = self
            .enabled_screens
            .iter()
            .map(|&s| self.screens[s].priority)
            .max();
        match max_priority {
            // `-1 * math.inf`: no screen has that priority
            None => BTreeSet::new(),
            Some(max) => self
                .enabled_screens
                .iter()
                .copied()
                .filter(|&s| self.screens[s].priority == max)
                .collect(),
        }
    }

    /// `_react()`: "Reacts to the changes in Screen settings. Sets active screens according to the
    /// Screen settings/state"
    fn _react(&mut self) {
        let new_set = self.get_set_to_show();
        if new_set != self.active_set || self.active_set.iter().any(|&s| self.screens[s].changed) {
            self.set_rewind();

            self.active_set = new_set;
            for &screen in &self.active_set {
                self.screens[screen].changed = false;
            }
            let mut sorted: Vec<ScreenId> = self.active_set.iter().copied().collect();
            sorted.sort_by_key(|&s| self.screens[s].order);
            self.active_screens = sorted;
        }
    }
}
