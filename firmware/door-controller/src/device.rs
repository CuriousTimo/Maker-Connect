//! The device Buddy's `Printer` stands for (`printer.hpp`): what the client asks about the board.
//! Only the fields a door controller needs are kept: `PrinterInfo` (`printer.hpp:38-49`) and `Config`
//! (`printer.hpp:181-191`, the token). The rest of the state is Part 7's: locks, doors, rest.

use crate::traits::{Clock, DoorState, Inputs, Relays, Store};
use crate::{LOCK_PULSE_MAX_MS, LOCK_REST_MIN_MS};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Buddy 6.4.0 `printer.hpp:184`: `CONNECT_TOKEN_LEN`.
pub const CONNECT_TOKEN_LEN: usize = 20;
/// Buddy 6.4.0 `printer.hpp:44`: `FINGERPRINT_HDR_SIZE`.
pub const FINGERPRINT_HDR_SIZE: usize = 16;

/// The device's state (Part 7, "States and telemetry").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviceState {
    Idle,
    Busy,
    Error,
}

impl DeviceState {
    pub fn as_str(self) -> &'static str {
        match self {
            DeviceState::Idle => "IDLE",
            DeviceState::Busy => "BUSY",
            DeviceState::Error => "ERROR",
        }
    }
}

/// Buddy 6.4.0 `printer.hpp:38-49` `PrinterInfo`.
#[derive(Clone, Debug)]
pub struct PrinterInfo {
    pub firmware_version: String,
    pub serial_number: String,
    pub fingerprint: String,
}

/// Buddy 6.4.0 `printer.hpp:181-191` `Config`, token only (host, port, TLS are the `Net` binding's).
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub token: String,
}

/// One lock as telemetry shows it (Part 7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockParams {
    pub lock: u32,
    pub door: Option<DoorState>,
    pub resting_s: u32,
}

#[derive(Clone, Debug)]
pub struct Params {
    pub state: DeviceState,
    pub locks: Vec<LockParams>,
}

impl Params {
    /// Buddy's `telemetry_fingerprint` / `state_fingerprint` (`planner.cpp:546`, `:523`): a change in what
    /// the server must hear at once. The rest countdown is not in it (Buddy leaves out what changes a lot).
    pub fn telemetry_fingerprint(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.state.hash(&mut h);
        for l in &self.locks {
            l.lock.hash(&mut h);
            l.door.map(|d| d == DoorState::Open).hash(&mut h);
            (l.resting_s > 0).hash(&mut h);
        }
        h.finish()
    }

    pub fn state_fingerprint(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.state.hash(&mut h);
        h.finish()
    }
}

pub struct Printer {
    relays: Box<dyn Relays>,
    inputs: Box<dyn Inputs>,
    clock: Box<dyn Clock>,
    store: Box<dyn Store>,
    lock_count: u32,
    info: PrinterInfo,
    config: Config,
    config_changed: bool,
    rest_until: Vec<Option<u32>>,
    /// A relay that did not answer: `ERROR` until the board restarts (a relay that fails once cannot be
    /// trusted to open or to stay off).
    relay_fault: bool,
    /// An input that did not answer on the last read: `ERROR` until all inputs read again.
    input_fault: bool,
}

impl Printer {
    pub fn new(
        relays: Box<dyn Relays>,
        inputs: Box<dyn Inputs>,
        clock: Box<dyn Clock>,
        store: Box<dyn Store>,
        lock_count: u32,
        firmware_version: &str,
    ) -> Printer {
        let info = PrinterInfo {
            firmware_version: firmware_version.to_string(),
            serial_number: store.serial(),
            fingerprint: store.fingerprint(),
        };
        let token = store.token().unwrap_or_default();
        Printer {
            relays,
            inputs,
            clock,
            store,
            lock_count,
            info,
            config: Config { token },
            config_changed: false,
            rest_until: vec![None; lock_count as usize],
            relay_fault: false,
            input_fault: false,
        }
    }

    pub fn printer_info(&self) -> &PrinterInfo {
        &self.info
    }

    pub fn lock_count(&self) -> u32 {
        self.lock_count
    }

    /// Buddy's `info_fingerprint()` (`planner.cpp:440`): what `INFO` says; here the lock count.
    pub fn info_fingerprint(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.lock_count.hash(&mut h);
        h.finish()
    }

    /// Buddy's `Printer::config(reset_changed)` (`connect.cpp:788`, `sleep.cpp:111`).
    pub fn config(&mut self, reset_changed: bool) -> (Config, bool) {
        let changed = self.config_changed;
        if reset_changed {
            self.config_changed = false;
        }
        (self.config.clone(), changed)
    }

    /// Buddy's `Printer::init_connect(token)` (`registrator.cpp:219`): keep the token.
    pub fn init_connect(&mut self, token: &str) {
        self.store.set_token(token);
        self.config.token = token.to_string();
        self.config_changed = true;
    }

    pub fn now(&self) -> u32 {
        self.clock.now_ms()
    }

    pub fn unix_time(&self) -> Option<u64> {
        self.clock.unix_time()
    }

    pub fn sleep(&mut self, ms: u32) {
        self.clock.sleep_ms(ms);
    }

    pub fn is_in_error(&self) -> bool {
        self.relay_fault || self.input_fault
    }

    /// Seconds until the lock may be opened again, rounded up; 0 when it may.
    pub fn resting_s(&self, lock: u32) -> u32 {
        match self.rest_until.get(lock as usize).copied().flatten() {
            Some(until) => {
                let left = until.wrapping_sub(self.clock.now_ms());
                // `until` in the past wraps to a huge value.
                if left == 0 || left > LOCK_REST_MIN_MS {
                    0
                } else {
                    left.div_ceil(1000)
                }
            }
            None => 0,
        }
    }

    pub fn params(&mut self, busy: bool) -> Params {
        let mut locks = Vec::new();
        let mut input_fault = false;
        for lock in 0..self.lock_count {
            let door = match self.inputs.read_door(lock) {
                Ok(d) => d,
                Err(_) => {
                    input_fault = true;
                    None
                }
            };
            locks.push(LockParams {
                lock,
                door,
                resting_s: self.resting_s(lock),
            });
        }
        self.input_fault = input_fault;
        let state = if self.is_in_error() {
            DeviceState::Error
        } else if busy {
            DeviceState::Busy
        } else {
            DeviceState::Idle
        };
        Params { state, locks }
    }

    /// One pulse on `lock` (Part 7): the pulse is the datasheet's, the rest starts when it ends.
    /// Nothing the server sends changes either value.
    pub fn open_lock(&mut self, lock: u32) -> bool {
        if self.relays.pulse(lock, LOCK_PULSE_MAX_MS).is_err() {
            self.relay_fault = true;
            return false;
        }
        let end = self.clock.now_ms();
        self.rest_until[lock as usize] = Some(end.wrapping_add(LOCK_REST_MIN_MS));
        true
    }
}
