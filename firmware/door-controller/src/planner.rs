//! Port of `planner.cpp` / `planner.hpp`: decides what goes to the server next and what each command
//! answers. Left out: everything of transfers, files, jobs, cancel objects (see the crate doc).

use crate::command::{Command, CommandData, CommandId};
use crate::device::Printer;

// planner.cpp:88-109, :114
pub const TELEMETRY_INTERVAL_MIN: u32 = 750;
pub const TELEMETRY_INTERVAL_LONG: u32 = 1000 * 4;
pub const TELEMETRY_INTERVAL_SHORT: u32 = 1000;
pub const TELEMETRY_INTERVAL_FULL: u32 = 1000 * 60 * 5;
pub const LOOP_WAKEUP: u32 = 500;
pub const RECONNECT_AFTER: u32 = 1000 * 10;
pub const GIVE_UP_AFTER_ATTEMPTS: u8 = 5;
// planner.hpp:139-141
pub const COOLDOWN_BASE: u32 = 100;
pub const COOLDOWN_MAX: u32 = 1000 * 60;

/// Buddy 6.4.0 `planner.cpp:257-291` `to_str(EventType)`, the types this board sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventType {
    Info,
    Accepted,
    Rejected,
    Finished,
    Failed,
    StateChanged,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            EventType::Info => "INFO",
            EventType::Accepted => "ACCEPTED",
            EventType::Rejected => "REJECTED",
            EventType::Finished => "FINISHED",
            EventType::Failed => "FAILED",
            EventType::StateChanged => "STATE_CHANGED",
        }
    }
}

/// Buddy 6.4.0 `planner.hpp:55-62` `MachineReason`: `None`, and Part 7's four codes in place of Buddy's
/// own (transfers, files, storage, network, not ready), which this board never sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineReason {
    None,
    NoSuchLock,
    LockResting,
    Expired,
    NoTime,
}

/// Buddy 6.4.0 `planner.cpp:293-310` `to_str(MachineReason)`.
pub fn machine_reason_str(reason: MachineReason) -> &'static str {
    match reason {
        // Not actually rendered, just to cover the case.
        MachineReason::None => "",
        MachineReason::NoSuchLock => "NO_SUCH_LOCK",
        MachineReason::LockResting => "LOCK_RESTING",
        MachineReason::Expired => "EXPIRED",
        MachineReason::NoTime => "NO_TIME",
    }
}

/// Buddy 6.4.0 `planner.hpp:66-83` `Event`, the fields this board uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub event_type: EventType,
    pub command_id: Option<CommandId>,
    pub reason: Option<&'static str>,
    pub machine_reason: MachineReason,
}

impl Event {
    pub fn new(event_type: EventType) -> Event {
        Event {
            event_type,
            command_id: None,
            reason: None,
            machine_reason: MachineReason::None,
        }
    }
    pub fn for_command(event_type: EventType, id: CommandId) -> Event {
        Event {
            event_type,
            command_id: Some(id),
            reason: None,
            machine_reason: MachineReason::None,
        }
    }
    pub fn rejected(id: CommandId, reason: &'static str) -> Event {
        Event::rejected_with(id, reason, MachineReason::None)
    }
    /// A refusal with its code, as Buddy plans one (`planner.cpp:813-871`).
    pub fn rejected_with(
        id: CommandId,
        reason: &'static str,
        machine_reason: MachineReason,
    ) -> Event {
        Event {
            event_type: EventType::Rejected,
            command_id: Some(id),
            reason: Some(reason),
            machine_reason,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TelemetryMode {
    Full,
    Reduced,
}

/// Buddy 6.4.0 `planner.hpp:85-92` `Action`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    SendTelemetry(TelemetryMode),
    Event(Event),
    Sleep {
        ms: u32,
        run_background: bool,
        wake_on_readable: bool,
    },
    ReadCommand,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionResult {
    Ok,
    Refused,
    RefusedFast,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundResult {
    Success,
    Failure,
}

/// Buddy's `Tracked` (`changes.hpp:8-34`).
#[derive(Default)]
struct Tracked {
    hash: u64,
    dirty: bool,
}

impl Tracked {
    fn new_dirty() -> Tracked {
        Tracked {
            hash: 0,
            dirty: true,
        }
    }
    fn set_hash(&mut self, new_hash: u64) -> bool {
        if new_hash != self.hash {
            self.dirty = true;
            self.hash = new_hash;
        }
        self.dirty
    }
    fn mark_clean(&mut self) {
        self.dirty = false;
    }
    fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

/// `buddy::ExponentialBackoff<Duration, COOLDOWN_BASE, COOLDOWN_MAX>` (`planner.hpp:146`). Its header is
/// not in `reference/buddy-6.4.0`; doubling from base to max is an assumption (open question).
#[derive(Default)]
struct Cooldown {
    failures: u32,
}

impl Cooldown {
    fn get(&self) -> Option<u32> {
        if self.failures == 0 {
            None
        } else {
            let shift = (self.failures - 1).min(20);
            Some((COOLDOWN_BASE << shift).min(COOLDOWN_MAX))
        }
    }
    fn fail(&mut self) {
        self.failures = self.failures.saturating_add(1);
    }
    fn reset(&mut self) {
        self.failures = 0;
    }
}

/// `planner.cpp:116-124`
fn since(now: u32, past: Option<u32>) -> Option<u32> {
    past.map(|p| now.wrapping_sub(p))
}

/// `planner.cpp:239-241`: Buddy accepts only these in the error state; of them the board has `SEND_INFO`.
fn command_is_error_whitelisted(command: &Command) -> bool {
    matches!(command.data, CommandData::SendInfo)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BackgroundCommand {
    id: CommandId,
    lock: u32,
}

pub struct Planner {
    planned_event: Option<Event>,
    info_changes: Tracked,
    state_info: Tracked,
    telemetry_changes: Tracked,
    last_telemetry: Option<u32>,
    last_full_telemetry: Option<u32>,
    last_telemetry_mode: TelemetryMode,
    last_success: Option<u32>,
    last_command_id: Option<CommandId>,
    background_command: Option<BackgroundCommand>,
    command_waiting: bool,
    cooldown: Cooldown,
    perform_cooldown: bool,
    failed_attempts: u8,
}

impl Planner {
    /// `planner.cpp:314-324`. The first state hash is IDLE so no STATE_CHANGED goes out on boot.
    pub fn new(printer: &mut Printer) -> Planner {
        let mut p = Planner {
            planned_event: None,
            info_changes: Tracked::new_dirty(),
            state_info: Tracked::default(),
            telemetry_changes: Tracked::new_dirty(),
            last_telemetry: None,
            last_full_telemetry: None,
            last_telemetry_mode: TelemetryMode::Full,
            last_success: None,
            last_command_id: None,
            background_command: None,
            command_waiting: false,
            cooldown: Cooldown::default(),
            perform_cooldown: false,
            failed_attempts: 0,
        };
        p.reset();
        let idle = crate::device::Params {
            state: crate::device::DeviceState::Idle,
            locks: vec![],
        };
        p.state_info.set_hash(idle.state_fingerprint());
        p.state_info.mark_clean();
        p.telemetry_changes.mark_dirty();
        let _ = printer;
        p
    }

    /// `planner.cpp:326-339`
    pub fn reset(&mut self) {
        self.info_changes.mark_dirty();
        self.last_telemetry = None;
        self.telemetry_changes.mark_dirty();
        self.cooldown.reset();
        self.perform_cooldown = false;
        self.failed_attempts = 0;
        self.last_command_id = None;
    }

    /// `planner.cpp:341-344`
    pub fn reset_telemetry(&mut self) {
        self.last_telemetry = None;
        self.telemetry_changes.mark_dirty();
    }

    /// `connect.cpp` `notify_command_waiting` (`sleep.cpp:242`).
    pub fn notify_command_waiting(&mut self) {
        self.command_waiting = true;
    }

    /// `planner.cpp:580-582`
    pub fn can_receive_command(&self) -> bool {
        self.planned_event.is_none()
    }

    pub fn busy(&self) -> bool {
        self.background_command.is_some()
    }

    pub fn background_command_id(&self) -> Option<CommandId> {
        self.background_command.as_ref().map(|c| c.id)
    }

    /// `planner.cpp:346-385` `Planner::sleep`.
    fn sleep(&self, printer: &Printer, amount: u32, wake_on_readable: bool) -> Action {
        // :349-351: a command waiting that we cannot take must not cut the sleep short.
        let wake = wake_on_readable && self.can_receive_command();
        // :353-355: in the error state nothing "extra" is done.
        if printer.is_in_error() {
            return Action::Sleep {
                ms: amount,
                run_background: false,
                wake_on_readable: wake,
            };
        }
        // planner.cpp:363-364: a background command runs only when no event waits to be sent.
        let run_background = self.background_command.is_some() && self.planned_event.is_none();
        Action::Sleep {
            ms: amount,
            run_background,
            wake_on_readable: wake,
        }
    }

    /// `planner.cpp:387-574` `Planner::next_action`, without the transfer, file and job branches.
    pub fn next_action(&mut self, printer: &mut Printer, wake_on_readable: bool) -> Action {
        if self.perform_cooldown {
            self.perform_cooldown = false;
            let ms = self.cooldown.get().unwrap_or(0);
            return self.sleep(printer, ms, false);
        }

        if let Some(ev) = self.planned_event.clone() {
            // planner.cpp:425-434: update the hashes so the event is not made again.
            match ev.event_type {
                EventType::Info => {
                    self.info_changes.set_hash(printer.info_fingerprint());
                }
                EventType::StateChanged => {
                    let params = printer.params(self.busy());
                    self.state_info.set_hash(params.state_fingerprint());
                }
                _ => {}
            }
            return Action::Event(ev);
        }

        if self.info_changes.set_hash(printer.info_fingerprint()) {
            let ev = Event::new(EventType::Info);
            self.planned_event = Some(ev.clone());
            return Action::Event(ev);
        }

        let params = printer.params(self.busy());

        if self.state_info.set_hash(params.state_fingerprint()) {
            let ev = Event::new(EventType::StateChanged);
            self.planned_event = Some(ev.clone());
            return Action::Event(ev);
        }

        if self.command_waiting && self.can_receive_command() {
            self.command_waiting = false;
            return Action::ReadCommand;
        }

        let now = printer.now();
        let changes = self
            .telemetry_changes
            .set_hash(params.telemetry_fingerprint());
        // planner.cpp:547: printing, a background command or a transfer is "busy".
        let want_short = self.background_command.is_some();
        let telemetry_interval = if want_short {
            TELEMETRY_INTERVAL_SHORT
        } else {
            TELEMETRY_INTERVAL_LONG
        };
        let since_telemetry =
            since(now, self.last_telemetry).unwrap_or(TELEMETRY_INTERVAL_FULL * 2);
        let since_full =
            since(now, self.last_full_telemetry).unwrap_or(TELEMETRY_INTERVAL_FULL * 2);

        let send_telemetry = since_telemetry >= TELEMETRY_INTERVAL_MIN
            && (changes || since_telemetry >= telemetry_interval);
        let want_full = changes || since_full >= TELEMETRY_INTERVAL_FULL;

        if send_telemetry {
            self.last_telemetry_mode = if want_full {
                TelemetryMode::Full
            } else {
                TelemetryMode::Reduced
            };
            Action::SendTelemetry(self.last_telemetry_mode)
        } else {
            let amount =
                (telemetry_interval - since_telemetry.min(telemetry_interval)).min(LOOP_WAKEUP);
            self.sleep(printer, amount, wake_on_readable)
        }
    }

    /// `planner.cpp:584-672` `Planner::action_done`.
    pub fn action_done(&mut self, printer: &mut Printer, result: ActionResult) {
        let now = printer.now();
        let exponential_backoff = |s: &mut Planner| {
            let since_success = since(now, s.last_success).unwrap_or(0);
            if since_success >= RECONNECT_AFTER && s.planned_event.is_none() {
                s.planned_event = Some(Event::new(EventType::Info));
                s.last_success = None;
                s.last_command_id = None;
            }
            s.cooldown.fail();
            s.perform_cooldown = true;
        };

        fn cleanups(s: &mut Planner, now: u32) {
            s.failed_attempts = 0;
            if let Some(ev) = &s.planned_event {
                match ev.event_type {
                    EventType::Info => s.info_changes.mark_clean(),
                    EventType::StateChanged => s.state_info.mark_clean(),
                    _ => {}
                }
                s.last_telemetry = None;
            } else {
                s.last_telemetry = Some(now);
                if s.last_telemetry_mode == TelemetryMode::Full {
                    s.last_full_telemetry = Some(now);
                    s.telemetry_changes.mark_clean();
                }
            }
            s.planned_event = None;
        }

        match result {
            ActionResult::Refused => {
                cleanups(self, now);
                exponential_backoff(self);
            }
            ActionResult::RefusedFast => {
                cleanups(self, now);
                self.perform_cooldown = false;
                self.cooldown.reset();
            }
            ActionResult::Ok => {
                self.last_success = Some(now);
                self.perform_cooldown = false;
                self.cooldown.reset();
                cleanups(self, now);
            }
            ActionResult::Failed => {
                self.failed_attempts += 1;
                if self.failed_attempts >= GIVE_UP_AFTER_ATTEMPTS {
                    if self
                        .planned_event
                        .as_ref()
                        .is_some_and(|e| e.event_type != EventType::Info)
                    {
                        cleanups(self, now);
                        self.last_command_id = None;
                    }
                    self.failed_attempts = 0;
                }
                exponential_backoff(self);
            }
        }
    }

    /// `planner.cpp:1070-1110` `Planner::command(Command)`, with the dispatch of `:674-796` for the two
    /// commands of this board.
    pub fn command(&mut self, printer: &mut Printer, command: Command) {
        // planner.cpp:1075 asserts there is no planned event: commands come only when none waits.
        if printer.is_in_error() && !command_is_error_whitelisted(&command) {
            self.planned_event = Some(Event::rejected(
                command.id,
                "Won't accept commands in error state",
            ));
            return;
        }

        if self.background_command.is_some() {
            // :1085-1093. In websocket mode no command is "this" one (that is for the HTTP resend), so Rejected.
            self.planned_event = Some(Event {
                event_type: EventType::Rejected,
                command_id: Some(command.id),
                reason: None,
                machine_reason: MachineReason::None,
            });
            return;
        }

        if self.last_command_id == Some(command.id) {
            self.planned_event = Some(Event::rejected(
                command.id,
                "Won't execute the same command multiple times",
            ));
            return;
        }

        self.last_command_id = Some(command.id);

        match command.data {
            // :674-676
            CommandData::Unknown => {
                self.planned_event = Some(Event::rejected(command.id, "Unknown command"))
            }
            // :678-680
            CommandData::Broken(reason) => {
                self.planned_event = Some(Event::rejected(command.id, reason))
            }
            // :686-688
            CommandData::ProcessingOther => {
                self.planned_event = Some(Event::rejected(command.id, "Processing other command"))
            }
            // :734-739
            CommandData::SendInfo => {
                self.planned_event = Some(Event::for_command(EventType::Info, command.id))
            }
            CommandData::OpenLock { lock, expires_at } => {
                self.open_lock(printer, command.id, lock, expires_at)
            }
        }
    }

    /// Part 7 "Opening a lock", in the form of Buddy's G-code command (`planner.cpp:690-696`):
    /// the command becomes the background command and `ACCEPTED` goes out; the pulse runs in the next
    /// sleep and `FINISHED` follows (`:1120-1133`). The refusals go out as `REJECTED` with the reason and its `machine_reason`.
    /// The order of the checks is the order Part 7 lists them (open question).
    fn open_lock(&mut self, printer: &mut Printer, id: CommandId, lock: u32, expires_at: u64) {
        let reason = if lock >= printer.lock_count() {
            Some(("No such lock", MachineReason::NoSuchLock))
        } else if printer.resting_s(lock) > 0 {
            Some(("Lock resting", MachineReason::LockResting))
        } else {
            match printer.unix_time() {
                None => Some(("No time", MachineReason::NoTime)),
                Some(now) if now > expires_at => Some(("Expired", MachineReason::Expired)),
                Some(_) => None,
            }
        };
        match reason {
            Some((reason, code)) => {
                self.planned_event = Some(Event::rejected_with(id, reason, code))
            }
            None => {
                self.background_command = Some(BackgroundCommand { id, lock });
                self.planned_event = Some(Event::for_command(EventType::Accepted, id));
            }
        }
    }

    /// The step of the background command, as `background_cmd_step` would do it from `sleep.cpp:162-194`
    /// (`SubmitBackgroundCmd`): the pulse, then `background_done`. Returns whether the sleep wakes.
    pub fn run_background(&mut self, printer: &mut Printer) -> bool {
        let Some(cmd) = self.background_command.clone() else {
            return false;
        };
        let result = if printer.open_lock(cmd.lock) {
            BackgroundResult::Success
        } else {
            BackgroundResult::Failure
        };
        self.background_done(result);
        true
    }

    /// `planner.cpp:1120-1133`
    fn background_done(&mut self, result: BackgroundResult) {
        let ev_type = if result == BackgroundResult::Success {
            EventType::Finished
        } else {
            EventType::Failed
        };
        self.planned_event = Some(Event {
            event_type: ev_type,
            command_id: self.background_command_id(),
            reason: None,
            machine_reason: MachineReason::None,
        });
        self.background_command = None;
    }
}
