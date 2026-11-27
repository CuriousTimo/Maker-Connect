//! Door-controller firmware mockup: the client side of Part 7 of
//! `docs/design/prusa-sdk-extension.md`, built and tested on the host.
//!
//! Reference (one, to the letter): Buddy 6.4.0, `reference/buddy-6.4.0/src/connect/` (Prusa-Firmware-Buddy,
//! licence in `reference/buddy-6.4.0/LICENSE.md`), ported function by function; every ported function
//! cites `file:line` of that tree. The rest is Part 7 (type, `INFO` kwargs, `locks`, `OPEN_LOCK`).
//!
//! # Ported from Buddy
//! - `registrator.cpp`/`.hpp`: registration by code ([`registrator`]).
//! - `connect.cpp`: the WebSocket session, ping, receive, send, upgrade ([`connect`]).
//! - `planner.cpp`/`.hpp`: what to send next, cooldown, events for commands ([`planner`]).
//! - `command.cpp`: parsing a `J` command ([`command`]).
//! - `render.cpp`: the shape of telemetry and events ([`render`]).
//! - `sleep.cpp`: a sleep that runs the background command ([`connect::Sleep`] via [`planner`]).
//! - `status.cpp`/`.hpp`: error and status types ([`status`]).
//! - `printer.hpp` `Config`/`PrinterInfo` (fields only): [`device`].
//!
//! # Not ported (Part 7: Buddy has it, the board leaves it out)
//! Files and storages (`SEND_FILE_INFO`, `DELETE_*`, `CREATE_FOLDER`, `FILE_INFO`, `FILE_CHANGED`,
//! `storages` in `INFO`), transfers (`START_*_DOWNLOAD`, `T` frames, `STOP_TRANSFER`, `TRANSFER_*`
//! events, `transfer_*` telemetry), print jobs (`START_PRINT`, `PAUSE/RESUME/STOP_PRINT`, `JOB_INFO`,
//! `job_id` and the other print telemetry), G-code (`G`/`F` frames), temperatures, axes, tools, MMU,
//! enclosure, `SET_VALUE`, `CANCEL_OBJECT`, `DIALOG_ACTION`, `SET_PRINTER_READY`/`SET_IDLE`,
//! `SET_TOKEN`, `RESET_PRINTER`, `SEND_STATE_INFO`, the plain-HTTP (non-WebSocket) exchange
//! (`BasicRequest`, `handle_server_resp`), the `UpgradeRequest` key/accept check (the `Net` binding does
//! it), and `tls/` (the `Net` binding does it).
//!
//! Hardware sits behind the traits in [`traits`]; [`fakes`] has the host fakes the tests use.
//! Fail-secure (Part 7): nothing in this crate opens a lock except `OPEN_LOCK` from the server.

pub mod command;
pub mod connect;
pub mod device;
pub mod fakes;
pub mod planner;
pub mod registrator;
pub mod render;
pub mod status;
pub mod traits;

/// The device type (Part 7, "The type and `INFO`").
pub const DEVICE_TYPE: &str = "maker-connect.door-controller";
/// The feature name of Part 7 (Part 6a, "The names").
pub const FEATURE_DOOR_LOCK: &str = "door.lock";
/// The command of Part 7.
pub const OPEN_LOCK: &str = "OPEN_LOCK";
/// The SDK's `UPGRADE` (`const.py:141`), part 6b / part 7.
pub const UPGRADE: &str = "UPGRADE";

/// Longest pulse the lock takes: SARY XG-07C-12V datasheet, as Part 7 gives it ("at most 0.5 s").
pub const LOCK_PULSE_MAX_MS: u32 = 500;
/// Shortest rest between two pulses: SARY XG-07C-12V datasheet, as Part 7 gives it ("at least 10 s").
pub const LOCK_REST_MIN_MS: u32 = 10_000;
