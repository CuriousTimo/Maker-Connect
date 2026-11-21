//! Port of `command.cpp` / `command.hpp` and `command_id.cpp` for the commands a door controller has:
//! `SEND_INFO` and, from Part 7, `OPEN_LOCK`. Every other name Buddy knows is `UnknownCommand` here
//! (`command.cpp:122`, answered "Unknown command", `planner.cpp:674-676`).

use serde_json::Value;

/// Buddy's `CommandId` (`command_id.hpp`): 32 bit.
pub type CommandId = u32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandData {
    /// `command.cpp:155`
    SendInfo,
    /// Part 7.
    OpenLock { lock: u32, expires_at: u64 },
    /// `command.cpp:122`: no name matched.
    Unknown,
    /// `command.cpp:233` and others: the reason is the REJECTED reason.
    Broken(&'static str),
    /// `connect.cpp:231-240`: the buffer is held by another command.
    ProcessingOther,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub id: CommandId,
    pub data: CommandData,
}

const ARG_LOCK: u32 = 1 << 0;
const ARG_EXPIRES_AT: u32 = 1 << 1;

/// Buddy's `convert_num<R>` (`command.cpp:46-53`): the value must be an integer that fits.
fn convert_num<R: TryFrom<u64>>(v: &Value) -> Option<R> {
    // A `Primitive` token in jsmn; a non-integer number does not convert.
    v.as_u64().and_then(|n| R::try_from(n).ok())
}

/// Buddy 6.4.0 `command.cpp:109-434` `Command::parse_json_command`, with the `T(...)` table of
/// `:150-193` cut to the names above. `command` at depth 1 is the name, kwargs at depth 2 inside the
/// object `kwargs` (`:137-139`, `:266-269`); a missing or unconvertible argument is
/// "Missing or broken parameters" (`:421-423`); JSON that does not parse is "Error parsing JSON"
/// (`:425-427`).
pub fn parse_json_command(id: CommandId, body: &[u8]) -> Command {
    let mut data = CommandData::Unknown;
    let mut expected_args: u32 = 0;
    let mut seen_args: u32 = 0;

    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let success = matches!(parsed, Some(Value::Object(_)));

    if let Some(Value::Object(obj)) = &parsed {
        if let Some(Value::String(name)) = obj.get("command") {
            match name.as_str() {
                "SEND_INFO" => data = CommandData::SendInfo,
                crate::OPEN_LOCK => {
                    data = CommandData::OpenLock {
                        lock: 0,
                        expires_at: 0,
                    };
                    expected_args = ARG_LOCK | ARG_EXPIRES_AT;
                }
                _ => {}
            }
        }
        if let Some(Value::Object(kwargs)) = obj.get("kwargs")
            && let CommandData::OpenLock { lock, expires_at } = &mut data
        {
            {
                if let Some(v) = kwargs.get("lock").and_then(convert_num::<u32>) {
                    *lock = v;
                    seen_args |= ARG_LOCK;
                }
                if let Some(v) = kwargs.get("expires_at").and_then(convert_num::<u64>) {
                    *expires_at = v;
                    seen_args |= ARG_EXPIRES_AT;
                }
            }
        }
    }

    if expected_args & !seen_args != 0 {
        data = CommandData::Broken("Missing or broken parameters");
    }
    if !success {
        data = CommandData::Broken("Error parsing JSON");
    }
    Command { id, data }
}
