//! A Buddy printer's tools in a telemetry frame, read as Buddy 8.0.2 writes them: the one
//! reference for a Buddy printer (owner, 2026-11-24, #3 and #201): `reference/buddy-8.0.2`
//! (Prusa-Firmware-Buddy v8.0.2, `b1043d6`), `src/connect/render.cpp` `render_msg` for
//! `SendTelemetry` (`:124-305`) and `src/connect/tools.cpp`.
//!
//! What Buddy writes:
//! - The `tools` object only in a Full frame (inside `telemetry.mode == SendTelemetry::Mode::Full`,
//!   `render.cpp:177-292`); a Reduced frame has no tools, and the hub keeps what the last Full
//!   one said.
//! - One entry per tool, every tool in every Full frame (`for (state.iter = 0; state.iter <
//!   state.iter_tool_count(); ...)`, `:220`), keyed by the tool's Connect id (`to_connect_tool`,
//!   `tools.cpp`): a head `"<n>"`; in front of an MMU a slot `"0.<slot>"`.
//! - A disabled tool as `null` (`JSON_FIELD_NULL`, `:226`, `:256`).
//! - A head: `type` `FFF_PRINT_HEAD`, `active`, `nozzle_temp`, `nozzle_target_temp` (null when
//!   the target is 0, `:233-237`), `heatbreak_fan_rpm`, `print_fan_rpm` (`:229-240`); then, for
//!   the filament path through it, `consumed_weight` (null without ANFC, `:271-281`) and the
//!   odometer (`HAS_FILAMENT_TRACKER`, `:283-288`); with an MMU a `feeder` block (`:243-252`).
//!
//! Not read here: an MMU's slots (`"0.<slot>"`) and a head's `feeder`, which only a printer with
//! `HAS_MMU2` writes (the MINI has none, `ProjectOptions.cmake:592`): #211.

use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// A head's members as Buddy writes them (`render.cpp:229-240`, `:271-288`); its `type`
/// (`FFF_PRINT_HEAD`) is not kept (owner, 2026-11-24: a tool's type later).
const HEAD_MEMBERS: [&str; 8] = [
    "active",
    "nozzle_temp",
    "nozzle_target_temp",
    "heatbreak_fan_rpm",
    "print_fan_rpm",
    "consumed_weight",
    "odometer_id",
    "odometer_e",
];

/// One Full frame's `tools` object onto the hub's tool tree: every tool Buddy names is there
/// as it wrote it, each head with its members; a disabled (null) tool, a slot (#211) and a
/// member Buddy left null are not. A target Buddy wrote as null is 0: it writes null only when
/// the target is 0 (`render.cpp:233-237`).
pub(crate) fn read(tree: &mut BTreeMap<String, Map<String, Value>>, tools: &Value) {
    let Some(tools) = tools.as_object() else {
        return;
    };
    tree.clear();
    for (id, entry) in tools {
        if id.contains('.') {
            continue; // an MMU's slot: #211
        }
        let Some(members) = entry.as_object() else {
            continue; // null: a disabled tool
        };
        let mut head = Map::new();
        for name in HEAD_MEMBERS {
            match members.get(name) {
                Some(Value::Null) if name == "nozzle_target_temp" => {
                    head.insert(name.to_owned(), Value::from(0));
                }
                None | Some(Value::Null) => {}
                Some(value) => {
                    head.insert(name.to_owned(), value.clone());
                }
            }
        }
        tree.insert(id.clone(), head);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A MINI's Full frame (one head, no ANFC): its head as Buddy wrote it, the null consumed
    /// weight left out, a null target 0.
    #[test]
    fn a_minis_head_as_buddy_writes_it() {
        let mut tree = BTreeMap::new();
        read(
            &mut tree,
            &json!({"0": {"type": "FFF_PRINT_HEAD", "active": true, "nozzle_temp": 201.3,
                "nozzle_target_temp": null, "heatbreak_fan_rpm": 7000, "print_fan_rpm": 0,
                "consumed_weight": null}}),
        );
        assert_eq!(
            tree["0"],
            json!({"active": true, "nozzle_temp": 201.3,
                "nozzle_target_temp": 0, "heatbreak_fan_rpm": 7000, "print_fan_rpm": 0})
            .as_object()
            .unwrap()
            .clone()
        );
    }

    /// Every Full frame names every tool: a tool now null (disabled) is gone, a slot is not read
    /// (#211), and nothing of an earlier frame stays.
    #[test]
    fn each_full_frame_is_the_whole_tool_set() {
        let mut tree = BTreeMap::new();
        read(
            &mut tree,
            &json!({"0": {"active": true, "nozzle_temp": 200.0}, "1": {"active": false, "nozzle_temp": 180.0}}),
        );
        read(
            &mut tree,
            &json!({"0": {"active": true, "nozzle_temp": 201.0}, "1": null,
                "0.0": {"active": true, "type": "FFF_FEEDER_SLOT", "consumed_weight": null}}),
        );
        assert_eq!(tree.keys().collect::<Vec<_>>(), ["0"]);
        assert_eq!(tree["0"]["nozzle_temp"], json!(201.0));
    }
}
