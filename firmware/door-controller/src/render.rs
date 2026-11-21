//! Port of `render.cpp`: the JSON of telemetry and events. Field order is Buddy's.

use crate::command::CommandId;
use crate::device::{Params, PrinterInfo};
use crate::planner::{Event, EventType, MachineReason, machine_reason_str};
use crate::traits::DoorState;
use crate::{DEVICE_TYPE, FEATURE_DOOR_LOCK};

fn str_json(s: &str) -> String {
    serde_json::to_string(s).expect("a string always serialises")
}

/// The Part 7 telemetry key `locks`.
fn locks_json(params: &Params) -> String {
    let items: Vec<String> = params
        .locks
        .iter()
        .map(|l| {
            let door = match l.door {
                Some(DoorState::Open) => "\"open\"",
                Some(DoorState::Closed) => "\"closed\"",
                None => "null",
            };
            format!(
                "{{\"lock\":{},\"door\":{},\"resting_s\":{}}}",
                l.lock, door, l.resting_s
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// Buddy 6.4.0 `render.cpp:122-271` `render_msg(SendTelemetry)`. Buddy's print, temperature, axis and
/// slot fields (`:137-256`) are not sent; `locks` is Part 7's and goes where the Full fields go (`:175`).
/// `command_id` while a background command runs (`:257-259`) and `state` last (`:264-267`).
pub fn render_telemetry(params: &Params, background_command_id: Option<CommandId>) -> String {
    let mut out = String::from("{");
    out.push_str(&format!("\"locks\":{},", locks_json(params)));
    if let Some(id) = background_command_id {
        out.push_str(&format!("\"command_id\":{id},"));
    }
    out.push_str(&format!("\"state\":\"{}\"}}", params.state.as_str()));
    out
}

/// Buddy 6.4.0 `render.cpp:273-685` `render_msg(Event)`: `reason` (`:330-332`), `machine_reason` (`:334-335`), the `data` block of
/// `INFO` (`:343-440`) with `firmware`, `printer_type`, `sn`, `fingerprint` (`:345-349`) and Part 7's
/// `lock_count` and `extensions`; `STATE_CHANGED` has an empty `data` (`:596-668` with nothing to say);
/// `state` (`:674`), `command_id` (`:675-677`), `event` (`:681`).
pub fn render_event(event: &Event, params: &Params, info: &PrinterInfo, lock_count: u32) -> String {
    let mut out = String::from("{");
    if let Some(reason) = event.reason {
        out.push_str(&format!("\"reason\":{},", str_json(reason)));
    }
    if event.machine_reason != MachineReason::None {
        out.push_str(&format!(
            "\"machine_reason\":{},",
            str_json(machine_reason_str(event.machine_reason))
        ));
    }
    match event.event_type {
        EventType::Info => {
            out.push_str(&format!(
                "\"data\":{{\"firmware\":{},\"printer_type\":{},\"sn\":{},\"fingerprint\":{},\"lock_count\":{},\"extensions\":[{}]}},",
                str_json(&info.firmware_version),
                str_json(DEVICE_TYPE),
                str_json(&info.serial_number),
                str_json(&info.fingerprint),
                lock_count,
                str_json(FEATURE_DOOR_LOCK),
            ));
        }
        EventType::StateChanged => out.push_str("\"data\":{},"),
        _ => {}
    }
    out.push_str(&format!("\"state\":\"{}\",", params.state.as_str()));
    if let Some(id) = event.command_id {
        out.push_str(&format!("\"command_id\":{id},"));
    }
    out.push_str(&format!("\"event\":\"{}\"}}", event.event_type.as_str()));
    out
}
