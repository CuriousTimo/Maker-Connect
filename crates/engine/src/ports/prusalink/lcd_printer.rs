//! Port of PrusaLink `printer_adapter/lcd_printer.py` (`LCDPrinter`): "Should inform the user about
//! everything important in PrusaLink while not obstructing anything else the printer wrote" (#189).
//!
//! The reference is a thread that waits, handles one event, looks at the model and sends the next
//! line of its carousel as `M117`. Here the thread's loop is [`LCDPrinter::wait_interval`] and
//! [`LCDPrinter::iteration`] (the body after the wait), and what the thread sends through
//! `enqueue_instruction` is returned as [`Printed`]; the caller does the waiting (with the
//! interval) and the sending. `time()` is the `now` the caller passes (seconds). The model, the
//! settings, the condition tracker and the printer's transfer that `whats_going_on` reads are
//! [`Inputs`].
//!
//! Differences (all OURS):
//!  1. Texts: "PrusaLink" is [`PRODUCT`] ("Maker Connect", the owner's decision, #189).
//!  2. The registration code screen (#189, owner 2026-11-22): "Code: <code>" (Buddy
//!     `src/gui/dialogs/DialogConnectReg.cpp:109`), shown while [`Inputs::registration_code`] is set,
//!     with [`CODE_PRIORITY`]; PrusaLink has no such screen. When the code is gone, the screen is
//!     disabled, with no message of its own (owner, 2026-11-23).
//!  3. `unidecode`: [`super::unidecode::unidecode`].
//!  4. `time()` and the thread: see above. `through_queue`/`add_event` put an [`Event`] on the queue
//!     (`add_event`), one handled per iteration as the thread does.
//!  5. `self.model...` is [`Inputs`]; `COND_TRACKER.get_worst()` and `NET_TRACKER.is_tracked()` are
//!     [`Inputs::error`] and [`Cond::net_tracked`] (the conditions of `conditions.py` and of the
//!     SDK's `conditions.py`, with `use_connect_errors(True)`).

use super::carousel::{Carousel, LCDLine, Screen, ScreenId};
use super::unidecode::unidecode;
use std::collections::VecDeque;

/// "PrusaLink" in PrusaLink's texts (owner, #189: "Maker Connect OK")
pub const PRODUCT: &str = "Maker Connect";

/// `const.py:77`
pub const QUIT_INTERVAL: f64 = 0.2;
/// `const.py:85`
pub const FW_MESSAGE_TIMEOUT: f64 = 10.0;
/// `const.py:92`
pub const SLEEP_SCREEN_TIMEOUT: f64 = 20.0;

pub fn welcome_chime() -> Vec<String> {
    [
        "M300 P100 S3200",
        "M300 P25 S0",
        "M300 P25 S4800",
        "M300 P75 S0",
        "M300 P25 S4800",
    ]
    .map(String::from)
    .to_vec()
}
pub fn error_chime() -> Vec<String> {
    vec!["M300 P600 S5".to_string()]
}
pub fn upload_chime() -> Vec<String> {
    vec!["M300 P14 S50".to_string()]
}

pub const ERROR_GRACE: f64 = 15.0;

pub const RECOVERY_PRIORITY: i64 = 60;
pub const PRINT_PRIORITY: i64 = 50;
pub const WIZARD_PRIORITY: i64 = 40;
pub const ERROR_PRIORITY: i64 = 30;
pub const ERROR_WAIT_PRIORITY: i64 = 31;
pub const UPLOAD_PRIORITY: i64 = 20;
pub const READY_PRIORITY: i64 = 11;
pub const IDLE_PRIORITY: i64 = 10;
/// OURS (difference 2): above the wizard's, which is what it is for
pub const CODE_PRIORITY: i64 = 40;

pub const NETWORK_ERROR_GRACE: f64 = 20.0;

/// The conditions with a message (`ERROR_MESSAGES`, :62-76), and `Other` for any other condition
/// the tracker may return (`error not in ERROR_MESSAGES`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    Id,
    Fw,
    Sn,
    Upgraded,
    JobId,
    Http,
    Token,
    Api,
    Internet,
    Lan,
    Phy,
    Device,
    Other,
}

impl Cond {
    /// `ERROR_MESSAGES[error]`
    pub fn message(self) -> Option<&'static str> {
        Some(match self {
            Cond::Id => "Unsupported printer",
            Cond::Fw => "Err unsupported FW",
            Cond::Sn => "Err obtaining S/N",
            Cond::Upgraded => "Upgraded - re-reg.",
            Cond::JobId => "Err reading job id",
            Cond::Http => "HTTP error 5xx",
            Cond::Token => "Error bad token",
            Cond::Api => "HTTP error 4xx",
            Cond::Internet => "No Internet access",
            Cond::Lan => "No LAN access",
            Cond::Phy => "No usable NIC",
            Cond::Device => "No network hardware",
            Cond::Other => return None,
        })
    }

    /// `NET_TRACKER.is_tracked(error)`: the `DEVICE` tree of `conditions.py` and, with
    /// `use_connect_errors(True)`, the `INTERNET` tree
    pub fn net_tracked(self) -> bool {
        matches!(
            self,
            Cond::Http
                | Cond::Token
                | Cond::Api
                | Cond::Internet
                | Cond::Lan
                | Cond::Phy
                | Cond::Device
        )
    }
}

/// `prusa.connect.printer.const.State`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Busy,
    Printing,
    Paused,
    Finished,
    Stopped,
    Error,
    Attention,
    Ready,
}

impl State {
    /// `PRINTING_STATES` (`const.py:46`)
    pub fn in_printing_states(self) -> bool {
        matches!(
            self,
            State::Printing | State::Paused | State::Finished | State::Stopped
        )
    }
}

/// `prusa.connect.printer.const.TransferType`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferType {
    NoTransfer,
    FromWeb,
    FromConnect,
    FromPrinter,
    FromSlicer,
    FromClient,
    ToConnect,
    ToClient,
}

impl TransferType {
    /// `FROM_TRANSFER_TYPES` (:78-82)
    fn is_from(self) -> bool {
        matches!(
            self,
            TransferType::FromPrinter
                | TransferType::FromWeb
                | TransferType::FromConnect
                | TransferType::FromClient
                | TransferType::FromSlicer
        )
    }

    /// `TO_TRANSFER_TYPES` (:84)
    fn is_to(self) -> bool {
        matches!(self, TransferType::ToConnect | TransferType::ToClient)
    }
}

/// `self.printer.transfer`
#[derive(Debug, Clone, PartialEq)]
pub struct Transfer {
    pub in_progress: bool,
    pub progress: f64,
    pub kind: TransferType,
    pub transferred: i64,
    pub size: i64,
}

/// What `whats_going_on` reads (difference 5)
#[derive(Debug, Clone, PartialEq)]
pub struct Inputs {
    /// `model.job.job_state == JobState.IN_PROGRESS`
    pub job_in_progress: bool,
    /// `model.job.selected_file_path`
    pub selected_file_path: Option<String>,
    /// `model.file_printer.recovering`
    pub recovering: bool,
    /// `model.file_printer.recovery_ready`
    pub recovery_ready: bool,
    /// `model.state_manager.current_state`
    pub state: State,
    /// `COND_TRACKER.get_worst()`
    pub error: Option<Cond>,
    /// `bool(LAN)`
    pub lan: bool,
    /// `settings.is_wizard_needed()`
    pub wizard_needed: bool,
    /// `settings.printer.network_error_chime`
    pub network_error_chime: bool,
    /// `model.ip_updater.local_ip`
    pub local_ip: Option<String>,
    /// `model.latest_telemetry.speed`
    pub speed: Option<i64>,
    pub transfer: Transfer,
    /// OURS (difference 2): the code the hub is waiting for, while it is
    pub registration_code: Option<String>,
}

/// What `_print` enqueues: the `M117` and the chime lines after it
#[derive(Debug, Clone, PartialEq)]
pub struct Printed {
    pub m117: String,
    pub chime: Vec<String>,
}

/// `add_event(handler)`'s handlers
#[derive(Debug, Clone)]
enum Event {
    /// `self.carousel.set_rewind`
    Rewind,
    /// `self.carousel.add_message(line)`
    Message(LCDLine),
}

/// Python `str.ljust(width)`
fn ljust(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - len))
    }
}

/// Python `str.center(width)`
fn center(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        return s.to_string();
    }
    let marg = width - len;
    let left = marg / 2 + (marg & width & 1);
    format!("{}{s}{}", " ".repeat(left), " ".repeat(marg - left))
}

/// `Path(x).name`
fn path_name(path: &str) -> String {
    path.split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .next_back()
        .unwrap_or("")
        .to_string()
}

fn py_str(ip: &Option<String>) -> String {
    ip.clone().unwrap_or_else(|| "None".to_string())
}

/// `util.py:76 get_local_ip()`, what PrusaLink's `IPUpdater` (`ip_updater.py:106-124`) keeps as
/// `local_ip`: "Gets the local ip used for connecting to MQTT_HOSTNAME ... does not matter if host is
/// reachable or not, any client interface that is UP should suffice". `Err` is its `socket.error`.
pub fn get_local_ip() -> std::io::Result<String> {
    let sock = std::net::UdpSocket::bind(("0.0.0.0", 0))?;
    sock.connect(("8.8.8.8", 1))?;
    Ok(sock.local_addr()?.ip().to_string())
}

/// `lcd_printer.py:112 LCDPrinter`: "Reports PrusaLink status on the printer LCD whenever possible"
#[derive(Debug)]
pub struct LCDPrinter {
    /// "PrusaLink" in PrusaLink's texts; [`PRODUCT`] (difference 1)
    pub product: String,
    pub printer_number: Option<String>,
    event_queue: VecDeque<Event>,
    pub quit: bool,
    /// `self.notiff_event`
    pub notified: bool,

    error_screen: ScreenId,
    upload_screen: ScreenId,
    wizard_screen: ScreenId,
    print_screen: ScreenId,
    wait_screen: ScreenId,
    ready_screen: ScreenId,
    idle_screen: ScreenId,
    recovery_screen: ScreenId,
    /// OURS (difference 2)
    code_screen: ScreenId,
    pub carousel: Carousel,

    ignore_errors_to: f64,
    network_error_at: Option<f64>,
    pub fw_msg_end_at: f64,
    pub idle_from: f64,
    /// "Used for ignoring LCD status updated that we generate"
    pub ignore: i64,
    pub current_line: Option<LCDLine>,
}

impl LCDPrinter {
    /// `__init__(.., printer_number)`; `product` is "PrusaLink" in PrusaLink's own texts, [`PRODUCT`] here
    pub fn new(printer_number: Option<String>, product: &str, now: f64) -> LCDPrinter {
        let screens = vec![
            // print_screen
            Screen::new(true, None, 1),
            // wizard_screen
            Screen::new(true, Some(welcome_chime()), 0),
            // wait_screen
            Screen::new(false, None, 0),
            // error_screen
            Screen::new(true, Some(error_chime()), 0),
            // upload_screen
            Screen::new(true, Some(upload_chime()), 0),
            // ready_screen
            Screen::new(false, None, 0),
            // idle_screen
            Screen::new(false, None, 0),
            // recovery_screen
            Screen::new(false, None, 0),
            // code_screen (OURS)
            Screen::new(true, None, 0),
        ];
        let (print_screen, wizard_screen, wait_screen, error_screen, upload_screen) =
            (0, 1, 2, 3, 4);
        let (ready_screen, idle_screen, recovery_screen, code_screen) = (5, 6, 7, 8);
        let mut carousel = Carousel::new(screens);

        carousel.set_priority(print_screen, PRINT_PRIORITY);
        carousel.set_priority(wizard_screen, WIZARD_PRIORITY);
        carousel.set_priority(error_screen, ERROR_PRIORITY);
        carousel.set_priority(upload_screen, UPLOAD_PRIORITY);
        carousel.set_priority(ready_screen, READY_PRIORITY);
        carousel.set_priority(idle_screen, IDLE_PRIORITY);
        carousel.set_priority(recovery_screen, RECOVERY_PRIORITY);
        carousel.set_priority(code_screen, CODE_PRIORITY);

        let wait_text: String = (1..=7)
            .map(|i| ljust(&format!("Please wait{}", ".".repeat(i)), 19))
            .collect();
        carousel.set_text(wait_screen, &wait_text, 1.5, 0.0, 19, 0.0);

        carousel.set_text(ready_screen, "Ready to print", 5.0, 0.0, 10, 0.0);

        carousel.set_text(recovery_screen, "Ready to recover", 5.0, 0.0, 10, 0.0);

        LCDPrinter {
            product: product.to_string(),
            printer_number,
            event_queue: VecDeque::new(),
            quit: false,
            notified: false,
            error_screen,
            upload_screen,
            wizard_screen,
            print_screen,
            wait_screen,
            ready_screen,
            idle_screen,
            recovery_screen,
            code_screen,
            carousel,
            ignore_errors_to: now + ERROR_GRACE,
            network_error_at: None,
            fw_msg_end_at: now,
            idle_from: now,
            ignore: 0,
            current_line: None,
        }
    }

    /// `_lcd_printer()` before its loop: `self._print(LCDLine("PrusaLink started"))`
    pub fn start(&mut self, now: f64) -> Printed {
        let mut line = LCDLine::plain(&format!("{} started", self.product), now);
        self._print(&mut line, now)
    }

    /// `stop(fast=False)` after the thread ended: `self._print(LCDLine("PrusaLink stopped"))`
    pub fn stop_line(&mut self, now: f64) -> Printed {
        self.quit = true;
        let mut line = LCDLine::plain(&format!("{} stopped", self.product), now);
        self._print(&mut line, now)
    }

    /// `lcd_updated(sender, match)`: each time the firmware prints out "LCD status changed"
    pub fn lcd_updated(&mut self, now: f64) {
        if self.ignore > 0 {
            self.ignore -= 1;
        } else {
            self._reset_idle(now);
            self.fw_msg_end_at = now + FW_MESSAGE_TIMEOUT;
            self.add_event(Event::Rewind);
        }
    }

    fn _message_and_disable(&mut self, screen: ScreenId, message: &str, now: f64) {
        if !self.carousel.is_enabled(screen) {
            return;
        }
        self.carousel.add_message(LCDLine::plain(message, now));
        self.carousel.disable(screen);
    }

    /// `whats_going_on()`
    pub fn whats_going_on(&mut self, inputs: &Inputs, now: f64) {
        self._check_printing(inputs);
        self._check_errors(inputs, now);
        self._check_wizard(inputs, now);
        self._check_upload(inputs, now);
        self._check_ready(inputs);
        self._check_idle(inputs, now);
        self._check_recovery(inputs);
        self._check_code(inputs, now);
    }

    fn cond(screen: &mut Screen, conditions: Vec<(&str, String)>) -> bool {
        let new: std::collections::BTreeMap<String, String> = conditions
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        if screen.conditions != new {
            screen.conditions = new;
            true
        } else {
            false
        }
    }

    fn _check_printing(&mut self, inputs: &Inputs) {
        if let (true, Some(path)) = (inputs.job_in_progress, &inputs.selected_file_path) {
            // We're printing! Display the file name
            self.carousel.enable(self.print_screen, false);

            let filename = path_name(path);
            // MK3 cannot print semicolons, replace them with an approximation
            let safe_filename = filename.replace(';', ",:");
            let rewinding = inputs.recovering;
            let changed = Self::cond(
                &mut self.carousel.screens[self.print_screen],
                vec![
                    ("filename", safe_filename.clone()),
                    ("rewinding", rewinding.to_string()),
                ],
            );
            if changed {
                if rewinding {
                    self.carousel
                        .set_text_default(self.print_screen, "Preparing recovery");
                } else {
                    self.carousel
                        .set_text_default(self.print_screen, &safe_filename);
                }
            }
        } else {
            self.carousel.disable(self.print_screen);
        }
    }

    /// `_filter_http(error)`: "Filter any network errors for the first X seconds"
    fn _filter_http(&mut self, error: Option<Cond>, now: f64) -> Option<Cond> {
        if error.is_none() {
            self.network_error_at = None;
        }
        if error.is_some_and(Cond::net_tracked) {
            // Silence the error until timeout
            let Some(at) = self.network_error_at else {
                self.network_error_at = Some(now);
                return None;
            };
            let time_since_error = now - at;
            if time_since_error < NETWORK_ERROR_GRACE {
                return None;
            }
        }
        error
    }

    fn _check_errors(&mut self, inputs: &Inputs, now: f64) {
        let unfiltered_error = inputs.error;
        let error_grace_ended = now - self.ignore_errors_to > 0.0;

        let error = self._filter_http(unfiltered_error, now);

        if error.is_some() && !error_grace_ended {
            self.carousel.enable(self.wait_screen, false);
        } else {
            self._message_and_disable(self.wait_screen, &format!("{} OK", self.product), now);
        }

        match error {
            None => self._message_and_disable(self.error_screen, "Errors resolved", now),
            Some(e) if e.message().is_none() => self.carousel.disable(self.error_screen),
            Some(error) if error_grace_ended => {
                // An error has been discovered, tell the user what it is
                let current_state = inputs.state;

                let silence_because_network = error.net_tracked() && !inputs.network_error_chime;
                let silence_because_printing = current_state == State::Printing;

                self.carousel.enable(
                    self.error_screen,
                    silence_because_network || silence_because_printing,
                );

                let changed = Self::cond(
                    &mut self.carousel.screens[self.error_screen],
                    vec![
                        ("lan", inputs.lan.to_string()),
                        ("error", format!("{error:?}")),
                    ],
                );
                if changed {
                    // No scrolling errors, just a screen worth of explanations
                    // and another one for the IP address
                    let message: String = error.message().unwrap().chars().take(19).collect();
                    let mut text = ljust(&message, 19);

                    if let Some(ip) = &inputs.local_ip {
                        text += &ljust(&format!("see {ip}"), 19);
                    }
                    self.carousel
                        .set_text(self.error_screen, &text, 2.0, 2.0, 19, 8.0);
                }

                if inputs.job_in_progress {
                    self.carousel.set_priority(self.error_screen, 50);
                } else {
                    self.carousel
                        .set_priority(self.error_screen, ERROR_PRIORITY);
                }
            }
            Some(_) => {}
        }
    }

    fn _check_wizard(&mut self, inputs: &Inputs, now: f64) {
        if inputs.wizard_needed && inputs.lan {
            self.carousel.enable(self.wizard_screen, false);
            let ip = inputs.local_ip.clone();
            let changed = Self::cond(
                &mut self.carousel.screens[self.wizard_screen],
                vec![
                    ("lan", inputs.lan.to_string()),
                    ("wizard_needed", inputs.wizard_needed.to_string()),
                    ("ip", py_str(&ip)),
                ],
            );
            if changed {
                let local_ip = py_str(&inputs.local_ip);
                let text = if let Some(number) = &self.printer_number {
                    format!("{local_ip}/{number}")
                } else {
                    // Can't have a capital G because old FW doesn't understand
                    // What's a print command and what's not. It differentiated
                    // between them using `"G" in command` condition
                    format!("Go: {local_ip}")
                };
                self.carousel
                    .set_text(self.wizard_screen, &text, 2.0, 2.0, 10, 10.0);
            }
        } else {
            self._message_and_disable(self.wizard_screen, "Setup completed", now);
        }
    }

    fn _get_progress_graphic(progress: f64, sync_type: TransferType) -> String {
        let bar_length: i64 = 12;
        // Have 12 characters for the load bar,
        // increased to 14 by the arrow visibility
        // [Sync|->:     0%     ]
        // [Sync|->:>    5%     ]
        // [Sync|->:=====95%===>]
        // [Sync|->:====100%====]

        // index of 0 and 13 means a hidden arrow
        let rough_index = progress / (100.0 / ((bar_length + 2) as f64));
        let index = (rough_index.floor() as i64).min(bar_length + 1);
        let display_arrow = 0 < index && index < 13;

        let mut progress_background = "=".repeat((index - 1).max(0) as usize);
        if display_arrow {
            progress_background.push('>');
        }
        let progress_background = ljust(&progress_background, bar_length as usize);

        // Put percentage over the background
        let int_progress = progress.round_ties_even() as i64;
        let string_progress = format!("{int_progress}%");
        let centered_progress = center(&string_progress, bar_length as usize);
        let centering_index = centered_progress
            .find(&string_progress)
            .expect("the text is in its centered copy");

        let mut progress_graphic = "Sync  :".to_string();
        if sync_type.is_from() {
            progress_graphic = "Sync\x7E|:".to_string();
        }
        if sync_type.is_to() {
            progress_graphic = "Sync|\x7E:".to_string();
        }
        let background: Vec<char> = progress_background.chars().collect();
        let head: String = background.iter().take(centering_index).collect();
        let tail: String = background
            .iter()
            .skip(centering_index + string_progress.chars().count())
            .collect();
        progress_graphic += &head;
        progress_graphic += &string_progress;
        progress_graphic += &tail;
        progress_graphic
    }

    fn _check_upload(&mut self, inputs: &Inputs, now: f64) {
        let state = inputs.state;
        if state.in_printing_states() && state != State::Printing {
            self.carousel
                .set_priority(self.upload_screen, PRINT_PRIORITY + 10);
        } else {
            self.carousel
                .set_priority(self.upload_screen, PRINT_PRIORITY);
        }
        let transfer = &inputs.transfer;
        if transfer.in_progress {
            self.carousel.enable(self.upload_screen, false);
            let progress_graphic = Self::_get_progress_graphic(transfer.progress, transfer.kind);
            self.carousel
                .set_text(self.upload_screen, &progress_graphic, 0.5, 0.0, 10, 0.0);
        } else if self.carousel.is_enabled(self.upload_screen) {
            let finished = transfer.transferred == transfer.size;
            if finished {
                self._message_and_disable(self.upload_screen, "Transfer finished", now);
            } else {
                self._message_and_disable(self.upload_screen, "Transfer stopped", now);
            }
        } else {
            self.carousel.disable(self.upload_screen);
        }
    }

    fn _check_ready(&mut self, inputs: &Inputs) {
        if inputs.state == State::Ready && inputs.lan {
            self.carousel.enable(self.ready_screen, false);
            let ip = inputs.local_ip.clone();
            let changed = Self::cond(
                &mut self.carousel.screens[self.ready_screen],
                vec![("ip", py_str(&ip))],
            );
            if changed {
                let text = ljust("Ready to print", 19) + &ljust(&py_str(&ip), 19);
                self.carousel
                    .set_text(self.ready_screen, &text, 4.0, 2.0, 19, 5.0);
            }
        } else {
            self.carousel.disable(self.ready_screen);
        }
    }

    fn _check_recovery(&mut self, inputs: &Inputs) {
        if inputs.recovery_ready {
            self.carousel.enable(self.recovery_screen, false);
        } else {
            self.carousel.disable(self.recovery_screen);
        }
    }

    fn _check_idle(&mut self, inputs: &Inputs, now: f64) {
        if now - self.idle_from > SLEEP_SCREEN_TIMEOUT && inputs.lan {
            self.carousel.enable(self.idle_screen, false);
            let local_ip = py_str(&inputs.local_ip);
            let ip_text = if let Some(number) = &self.printer_number {
                format!("{local_ip}/{number}")
            } else {
                local_ip.clone()
            };
            let speed = inputs.speed;
            let changed = Self::cond(
                &mut self.carousel.screens[self.idle_screen],
                vec![
                    ("ip", local_ip),
                    (
                        "speed",
                        speed.map_or_else(|| "None".to_string(), |s| s.to_string()),
                    ),
                ],
            );
            if changed {
                if speed != Some(42) {
                    let text = ljust(&format!("{} OK.", self.product), 19) + &ljust(&ip_text, 19);
                    self.carousel
                        .set_text(self.idle_screen, &text, 2.0, 2.0, 19, 12.0);
                } else {
                    self.carousel.set_text(
                        self.idle_screen,
                        "The Answer to the Great Question... Of Life, the \
                         Universe and Everything... Is... Forty-Two.",
                        0.3,
                        2.0,
                        1,
                        5.0,
                    );
                }
            }
        } else {
            self.carousel.disable(self.idle_screen);
        }
    }

    /// OURS (difference 2)
    fn _check_code(&mut self, inputs: &Inputs, _now: f64) {
        if let Some(code) = &inputs.registration_code {
            self.carousel.enable(self.code_screen, false);
            let changed = Self::cond(
                &mut self.carousel.screens[self.code_screen],
                vec![("code", code.clone())],
            );
            if changed {
                self.carousel.set_text(
                    self.code_screen,
                    &format!("Code: {code}"),
                    2.0,
                    0.0,
                    10,
                    0.0,
                );
            }
        } else {
            self.carousel.disable(self.code_screen);
        }
    }

    /// `get_wait_interval()`: "How long to wait until the next line might want to be shown"
    pub fn wait_interval(&self, now: f64) -> f64 {
        let mut wait_for = QUIT_INTERVAL;
        wait_for = wait_for.max(self.fw_msg_end_at - now);
        if let Some(line) = &self.current_line {
            wait_for = wait_for.max(line.ends_at - now);
        }
        wait_for
    }

    /// `should_advance_carousel()`: "Should we get a new line from the carousel?"
    pub fn should_advance_carousel(&self, now: f64) -> bool {
        let mut to_advance_carousel = true;
        let line = &self.current_line;
        if now < self.fw_msg_end_at {
            to_advance_carousel = false;
        } else if let Some(line) = line
            && now < line.ends_at
            && !self.carousel.to_rewind
        {
            to_advance_carousel = false;
        }
        to_advance_carousel
    }

    /// The body of `_lcd_printer`'s loop after the wait: one event, `whats_going_on()`, and the
    /// next line, if it is time for one
    pub fn iteration(&mut self, inputs: &Inputs, now: f64) -> Option<Printed> {
        self.notified = false;

        if let Some(event) = self.event_queue.pop_front() {
            match event {
                Event::Rewind => self.carousel.set_rewind(),
                Event::Message(line) => self.carousel.add_message(line),
            }
        }

        // Lets update our state
        self.whats_going_on(inputs, now);

        if self.should_advance_carousel(now) {
            self.current_line = self.carousel.get_next(now);
            // Get the line and send it to the printer
            if let Some(mut line) = self.current_line.take() {
                let printed = self._print(&mut line, now);
                self.current_line = Some(line);
                return Some(printed);
            }
        }
        None
    }

    /// `_print(line)`: "Sends the given message using M117 gcode"
    pub fn _print(&mut self, line: &mut LCDLine, now: f64) -> Printed {
        if line.resets_idle {
            self._reset_idle(now);
        }
        let ascii_text = unidecode(&line.text);
        self.ignore += 1;
        let printed = Printed {
            m117: format!("M117 \x7E{ascii_text}"),
            // Play a sound accompanying the newly shown thing
            chime: line.chime_gcode.clone(),
        };
        line.reset_end(now);
        printed
    }

    fn _reset_idle(&mut self, now: f64) {
        self.idle_from = now;
    }

    fn add_event(&mut self, event: Event) {
        self.event_queue.push_back(event);
        self.notify();
    }

    /// `notify()`
    pub fn notify(&mut self) {
        self.notified = true;
    }

    /// `reset_error_grace()`
    pub fn reset_error_grace(&mut self, now: f64) {
        self.ignore_errors_to = now + ERROR_GRACE;
    }

    /// `print_message(line)`: "Print a message at most 19 chars long"
    pub fn print_message(&mut self, line: LCDLine) {
        self.add_event(Event::Message(line));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::prusalink::python;
    use serde_json::{Value, json};

    fn idle_inputs() -> Inputs {
        Inputs {
            job_in_progress: false,
            selected_file_path: None,
            recovering: false,
            recovery_ready: false,
            state: State::Idle,
            error: None,
            lan: false,
            wizard_needed: false,
            network_error_chime: false,
            local_ip: None,
            speed: None,
            transfer: Transfer {
                in_progress: false,
                progress: 0.0,
                kind: TransferType::NoTransfer,
                transferred: 0,
                size: 0,
            },
            registration_code: None,
        }
    }

    fn state_of(name: &str) -> State {
        match name {
            "IDLE" => State::Idle,
            "BUSY" => State::Busy,
            "PRINTING" => State::Printing,
            "PAUSED" => State::Paused,
            "FINISHED" => State::Finished,
            "STOPPED" => State::Stopped,
            "ERROR" => State::Error,
            "ATTENTION" => State::Attention,
            "READY" => State::Ready,
            other => panic!("{other}"),
        }
    }

    fn cond_of(name: &str) -> Cond {
        match name {
            "ID" => Cond::Id,
            "FW" => Cond::Fw,
            "SN" => Cond::Sn,
            "UPGRADED" => Cond::Upgraded,
            "JOB_ID" => Cond::JobId,
            "HTTP" => Cond::Http,
            "TOKEN" => Cond::Token,
            "API" => Cond::Api,
            "INTERNET" => Cond::Internet,
            "LAN" => Cond::Lan,
            "PHY" => Cond::Phy,
            "DEVICE" => Cond::Device,
            "OTHER" => Cond::Other,
            other => panic!("{other}"),
        }
    }

    fn kind_of(name: &str) -> TransferType {
        match name {
            "NO_TRANSFER" => TransferType::NoTransfer,
            "FROM_WEB" => TransferType::FromWeb,
            "FROM_CONNECT" => TransferType::FromConnect,
            "FROM_PRINTER" => TransferType::FromPrinter,
            "FROM_SLICER" => TransferType::FromSlicer,
            "FROM_CLIENT" => TransferType::FromClient,
            "TO_CONNECT" => TransferType::ToConnect,
            "TO_CLIENT" => TransferType::ToClient,
            other => panic!("{other}"),
        }
    }

    /// What the driver's `apply(action)` does to the model, to `Inputs` (and the printer)
    fn apply(lcd: &mut LCDPrinter, inputs: &mut Inputs, action: &Value, now: f64) {
        if let Some(set) = action.get("set") {
            for (key, v) in set.as_object().unwrap() {
                match key.as_str() {
                    "job_in_progress" => inputs.job_in_progress = v.as_bool().unwrap(),
                    "selected_file_path" => {
                        inputs.selected_file_path = v.as_str().map(str::to_string)
                    }
                    "recovering" => inputs.recovering = v.as_bool().unwrap(),
                    "recovery_ready" => inputs.recovery_ready = v.as_bool().unwrap(),
                    "state" => inputs.state = state_of(v.as_str().unwrap()),
                    "wizard_needed" => inputs.wizard_needed = v.as_bool().unwrap(),
                    "network_error_chime" => inputs.network_error_chime = v.as_bool().unwrap(),
                    "local_ip" => inputs.local_ip = v.as_str().map(str::to_string),
                    "speed" => inputs.speed = v.as_i64(),
                    "transfer" => {
                        inputs.transfer = Transfer {
                            in_progress: v["in_progress"].as_bool().unwrap(),
                            progress: v["progress"].as_f64().unwrap(),
                            kind: kind_of(v["kind"].as_str().unwrap()),
                            transferred: v["transferred"].as_i64().unwrap(),
                            size: v["size"].as_i64().unwrap(),
                        }
                    }
                    "conditions" => {
                        inputs.lan = v["lan"].as_bool().unwrap();
                        inputs.error = v["error"].as_str().map(cond_of);
                    }
                    other => panic!("{other}"),
                }
            }
        }
        if action["lcd_updated"] == true {
            lcd.lcd_updated(now);
        }
        if let Some(text) = action.get("message") {
            lcd.print_message(LCDLine::plain(text.as_str().unwrap(), now));
        }
        if action["reset_error_grace"] == true {
            lcd.reset_error_grace(now);
        }
    }

    /// The driver's `FakeEvent` and `_lcd_printer()` loop: what `_print` enqueues, and when
    fn simulate(scenario: &Value, product: &str, code: Option<&str>) -> Vec<(f64, String, bool)> {
        let mut now = 0.0;
        let mut lcd = LCDPrinter::new(
            scenario["printer_number"].as_str().map(str::to_string),
            product,
            now,
        );
        let mut inputs = idle_inputs();
        inputs.registration_code = code.map(str::to_string);
        let end = scenario["end"].as_f64().unwrap();
        let mut script: VecDeque<&Value> = scenario["script"].as_array().unwrap().iter().collect();
        let mut sent: Vec<(f64, String, bool)> = vec![];
        let record = |sent: &mut Vec<(f64, String, bool)>, now: f64, printed: Printed| {
            sent.push((now, printed.m117, true));
            for chime in printed.chime {
                sent.push((now, chime, false));
            }
        };
        let started = lcd.start(now);
        record(&mut sent, now, started);
        while !lcd.quit {
            if !lcd.notified {
                now += lcd.wait_interval(now);
            }
            while script
                .front()
                .is_some_and(|a| a["at"].as_f64().unwrap() <= now)
            {
                let action = script.pop_front().unwrap();
                apply(&mut lcd, &mut inputs, action, now);
            }
            if now >= end {
                lcd.quit = true;
            }
            if let Some(printed) = lcd.iteration(&inputs, now) {
                record(&mut sent, now, printed);
            }
        }
        sent
    }

    fn a(at: f64, set: Value) -> Value {
        json!({"at": at, "set": set})
    }

    fn up(ip: &str, state: &str) -> Value {
        json!({"local_ip": ip, "state": state, "conditions": {"lan": true, "error": null}})
    }

    fn scenarios() -> Vec<Value> {
        let transfer = |p: f64, kind: &str| {
            json!({"transfer": {"in_progress": true, "progress": p, "kind": kind,
                "transferred": 1, "size": 100}})
        };
        let mut all = vec![
            // ready, then idle ("The status display" every 5 s; idle after 20 s)
            json!({"printer_number": null, "end": 120.0, "script": [a(0.1, up("192.168.1.5", "READY"))]}),
            // idle, with a printer number and the 42 easter egg
            json!({"printer_number": "3", "end": 90.0, "script": [
                a(0.1, up("10.0.0.7", "IDLE")), a(50.0, json!({"speed": 42})), a(70.0, json!({"speed": 100}))]}),
            // printing a file with a long name and a semicolon, then recovering
            json!({"printer_number": null, "end": 100.0, "script": [
                a(0.1, up("10.0.0.7", "PRINTING")),
                a(1.0, json!({"job_in_progress": true,
                    "selected_file_path": "/usb/folder/my;long file name ünïcode 北亰.gcode"})),
                a(60.0, json!({"recovering": true})),
                a(75.0, json!({"recovering": false, "job_in_progress": false}))]}),
            // the wizard, with and without a printer number, then done
            json!({"printer_number": null, "end": 70.0, "script": [
                a(0.1, json!({"wizard_needed": true, "local_ip": "172.16.1.20",
                    "conditions": {"lan": true, "error": null}})),
                a(40.0, json!({"wizard_needed": false}))]}),
            json!({"printer_number": "12", "end": 50.0, "script": [
                a(0.1, json!({"wizard_needed": true, "local_ip": "172.16.1.20",
                    "conditions": {"lan": true, "error": null}}))]}),
            // recovery ready
            json!({"printer_number": null, "end": 60.0, "script": [
                a(0.1, up("10.0.0.7", "READY")), a(10.0, json!({"recovery_ready": true})),
                a(30.0, json!({"recovery_ready": false}))]}),
            // firmware messages, and messages of the host
            json!({"printer_number": null, "end": 80.0, "script": [
                a(0.1, up("10.0.0.7", "READY")),
                {"at": 12.0, "lcd_updated": true}, {"at": 12.0, "lcd_updated": true},
                {"at": 30.0, "message": "Hello printer"},
                {"at": 45.0, "lcd_updated": true}, {"at": 46.0, "reset_error_grace": true}]}),
        ];
        // every condition, in the grace of the start and after it, with and without the chime,
        // while printing and not
        for error in [
            "ID", "FW", "SN", "UPGRADED", "JOB_ID", "HTTP", "TOKEN", "API", "INTERNET", "LAN",
            "PHY", "DEVICE", "OTHER",
        ] {
            let lan = !matches!(error, "LAN" | "PHY" | "DEVICE");
            for chime in [false, true] {
                for printing in [false, true] {
                    let mut script = vec![
                        a(
                            0.1,
                            json!({"local_ip": "10.0.0.7", "state": "READY",
                            "network_error_chime": chime,
                            "conditions": {"lan": true, "error": null}}),
                        ),
                        a(5.0, json!({"conditions": {"lan": lan, "error": error}})),
                        a(70.0, json!({"conditions": {"lan": true, "error": null}})),
                    ];
                    if printing {
                        script.push(a(
                            20.0,
                            json!({"state": "PRINTING", "job_in_progress": true,
                                "selected_file_path": "/usb/a.gcode"}),
                        ));
                    }
                    script.sort_by(|x, y| x["at"].as_f64().partial_cmp(&y["at"].as_f64()).unwrap());
                    all.push(json!({"printer_number": null, "end": 110.0, "script": script}));
                }
            }
        }
        // the progress bar of an upload, from each kind, a finished and a stopped one
        for kind in [
            "FROM_WEB",
            "FROM_CONNECT",
            "FROM_PRINTER",
            "FROM_SLICER",
            "FROM_CLIENT",
            "TO_CONNECT",
            "TO_CLIENT",
            "NO_TRANSFER",
        ] {
            let mut script = vec![a(0.1, up("10.0.0.7", "READY"))];
            let mut at = 3.0;
            for p in [
                0.0, 0.4, 3.4, 5.0, 7.2, 14.3, 50.0, 50.5, 71.4, 95.0, 99.9, 100.0,
            ] {
                script.push(a(at, transfer(p, kind)));
                at += 2.5;
            }
            script.push(a(
                at,
                json!({"transfer": {"in_progress": false, "progress": 100.0, "kind": kind,
                    "transferred": 100, "size": 100}}),
            ));
            let mut stopped = script.clone();
            stopped.pop();
            stopped.push(a(
                at,
                json!({"transfer": {"in_progress": false, "progress": 50.0, "kind": kind,
                    "transferred": 50, "size": 100}}),
            ));
            all.push(json!({"printer_number": null, "end": at + 30.0, "script": script}));
            all.push(json!({"printer_number": null, "end": at + 30.0, "script": stopped}));
        }
        // an upload while a print is paused or finished: priority above the print's
        all.push(json!({"printer_number": null, "end": 60.0, "script": [
            a(0.1, up("10.0.0.7", "PAUSED")),
            a(1.0, json!({"job_in_progress": true, "selected_file_path": "/usb/a.gcode"})),
            a(10.0, transfer(33.0, "FROM_WEB"))]}));
        all
    }

    /// PrusaLink's own `LCDPrinter` (and `Carousel`) on the same scenarios, driven by a fake clock
    /// (`driver_lcd.py`): the same `M117` lines and chimes, at the same moments, in the same order
    #[test]
    fn identical_to_prusalink() {
        let scenarios = scenarios();
        let theirs = python::run(include_str!("driver_lcd.py"), &json!(scenarios));
        let mut lines = 0;
        for (i, scenario) in scenarios.iter().enumerate() {
            let ours = simulate(scenario, "PrusaLink", None);
            let want: Vec<(f64, String, bool)> = theirs[i]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    (
                        e[0].as_f64().unwrap(),
                        e[1].as_str().unwrap().to_string(),
                        e[2].as_bool().unwrap(),
                    )
                })
                .collect();
            assert_eq!(want, ours, "scenario {i}: {scenario}");
            lines += ours.len();
        }
        assert!(lines > 1000, "{lines}");
    }

    /// The only change in the texts: "PrusaLink" is "Maker Connect"; "Maker Connect OK." and its
    /// 19-character lines
    #[test]
    fn maker_connect_ok() {
        let scenario = &scenarios()[0];
        let ours = simulate(scenario, PRODUCT, None);
        assert_eq!(ours[0].1, "M117 ~Maker Connect started");
        let scenario = &scenarios()[1];
        let ours = simulate(scenario, PRODUCT, None);
        assert!(
            ours.iter()
                .any(|(_, m, _)| m == "M117 ~Maker Connect OK.  "),
            "{ours:?}"
        );
    }

    /// OURS (#189, the owner's decision): "Code: <code>" while the code is waited for; the
    /// screen goes, with no text of its own, when the code is gone
    #[test]
    fn the_registration_code_screen() {
        let mut lcd = LCDPrinter::new(None, PRODUCT, 0.0);
        let mut inputs = idle_inputs();
        inputs.registration_code = Some("ABC123".to_string());
        let mut now = 0.0;
        let mut said = vec![lcd.start(now).m117];
        for _ in 0..60 {
            now += lcd.wait_interval(now);
            if let Some(p) = lcd.iteration(&inputs, now) {
                said.push(p.m117);
            }
        }
        assert!(
            said[1..].iter().all(|m| m == "M117 ~Code: ABC123"),
            "{said:?}"
        );
        assert!(said.len() > 3);
        inputs.registration_code = None;
        let mut after = vec![];
        for _ in 0..10 {
            now += lcd.wait_interval(now);
            if let Some(p) = lcd.iteration(&inputs, now) {
                after.push(p.m117);
            }
        }
        assert!(!after.iter().any(|m| m.contains("Code:")), "{after:?}");
        assert!(!after.iter().any(|m| m.contains("Regist")), "{after:?}");
    }
}
