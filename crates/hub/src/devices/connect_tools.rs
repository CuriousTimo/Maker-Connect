//! What the hub keeps per machine on Prusa Connect's protocol, in memory, and what its pages show
//! of a printer's tools. The reading itself is per reference, apart: a Buddy printer's tools as
//! Buddy 8.0.2 writes them ([`super::buddy_tools`]), a device on the agent as PrusaLink and the
//! Connect SDK send them ([`super::sdk_tools`]). What a page shows of them (the head shown, the
//! list of heads) is ours.

use maker_connect_protocol::{Telemetry, ToolHeat};
use serde_json::Value;

/// What the hub keeps per machine while it is known (`Legacy::forget_machine` drops it).
#[derive(Debug, Clone, Default)]
pub(crate) struct Session {
    /// The telemetry as the last frame left it, so that the next frame is taken in over it under
    /// the hub's lock. None after a restart: the stored one then.
    pub(crate) last: Option<Telemetry>,
    /// A label printer on DYMO's CUPS driver: the `reason` of its last `STATE_CHANGED`, the PPD's own
    /// text (docs/design/prusa-sdk-extension.md, part 3), shown as its problem; None for any other
    /// machine, whose problem this leaves alone.
    pub(crate) label_reason: Option<Option<String>>,
    /// A GRBL laser on the agent: its settings as its last `INFO` gave them
    /// (docs/design/prusa-sdk-extension.md, part 4); None for any other machine.
    pub(crate) grbl: Option<maker_connect_protocol::GrblSettings>,
    /// What the hub keeps per machine about the print (`crate::devices::connect_job`).
    pub(crate) job: crate::devices::connect_job::Job,
    /// A Buddy printer's material as its last INFO named it: Buddy 8.0.2 writes the spool only in
    /// INFO (`render.cpp:493-531`), not in telemetry; the page shows it with the telemetry.
    pub(crate) material: Option<String>,
}

impl Session {
    /// One telemetry frame's tools onto `t`: a device on the agent by PrusaLink's keys, a Buddy
    /// printer by its `tools` object (only in a Full frame; a Reduced one keeps the last).
    pub(crate) fn telemetry(&mut self, t: &mut Telemetry, fields: &Value, agent: bool) {
        if agent {
            super::sdk_tools::read(t, fields);
            return;
        }
        if let Some(tools) = fields.get("tools") {
            super::buddy_tools::read(&mut t.tool_tree, tools);
        }
        show(t, self.material.as_deref());
    }
}

/// A tool's material as Buddy before 8.0.2 wrote it in INFO, read only as the fallback in
/// `to_details` (owner, 2026-11-24): `"---"` for a tool with nothing loaded (Buddy 6.3.0
/// `render.cpp:395`), an empty name: no material.
pub(crate) fn normalize_material(raw: &Value) -> Value {
    match raw {
        Value::Null => Value::String(String::new()),
        Value::String(s) if matches!(s.trim(), "" | "---") => Value::String(String::new()),
        other => other.clone(),
    }
}

/// What the page shows of a Buddy printer's tools (ours): the head that is active, or the only
/// head; its nozzle, target and fans (`heatbreak_fan_rpm` is the hotend's fan); the list of heads
/// only when there is more than one, head `"n"` numbered `n + 1`; the material from INFO.
fn show(t: &mut Telemetry, material: Option<&str>) {
    let tree = &t.tool_tree;
    let heads: Vec<&String> = tree.keys().collect();
    let active = |id: &str| tree[id].get("active").and_then(Value::as_bool) == Some(true);
    let shown = heads
        .iter()
        .find(|id| active(id))
        .or(match heads.as_slice() {
            [only] => Some(only),
            _ => None,
        })
        .map(|id| id.as_str());
    let number = |id: &str, name: &str| {
        tree.get(id)
            .and_then(|m| m.get(name))
            .and_then(Value::as_f64)
    };
    let head = |name: &str| shown.and_then(|id| number(id, name));
    t.nozzle_c = head("nozzle_temp").map(|v| v as f32);
    t.nozzle_target_c = head("nozzle_target_temp").map(|v| v as f32);
    t.print_fan_rpm = head("print_fan_rpm").map(|v| v as u32);
    t.hotend_fan_rpm = head("heatbreak_fan_rpm").map(|v| v as u32);
    t.material = material.map(str::to_owned);
    t.tools = if heads.len() > 1 {
        heads
            .iter()
            .map(|id| ToolHeat {
                number: id.parse::<u32>().map_or(0, |n| n + 1),
                nozzle_c: number(id, "nozzle_temp").map(|v| v as f32),
                nozzle_target_c: number(id, "nozzle_target_temp").map(|v| v as f32),
                active: active(id),
            })
            .collect()
    } else {
        Vec::new()
    };
}
