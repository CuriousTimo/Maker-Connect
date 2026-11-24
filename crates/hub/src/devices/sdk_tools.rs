//! A device on the agent: its head in a telemetry frame, read as PrusaLink sends it through the
//! Prusa Connect SDK, the reference for the agent's printers (CLAUDE.md; the agent ports them):
//! `reference/Prusa-Link` at `171148c` and `reference/Prusa-Connect-SDK-Printer` at `c0d7d23`.
//!
//! What they send:
//! - The head's values at the top of the frame: `temp_nozzle` (`auto_telemetry.py:61`),
//!   `target_nozzle` (`:65`), `fan_hotend` and `fan_print` (`:93-98`). Our agent sends the same
//!   keys for a Marlin printer (`crates/agent/src/marlin.rs` `telemetry`, extension part 9).
//! - Only keys with a value: the SDK leaves out every null (`Telemetry.to_payload` →
//!   `filter_null`, `models.py:39-45`, `:190-192`). A key a frame leaves out keeps the value an
//!   earlier frame gave.

use maker_connect_protocol::Telemetry;
use serde_json::Value;

/// The head's values a frame carries, onto the telemetry.
pub(crate) fn read(t: &mut Telemetry, fields: &Value) {
    let number = |key: &str| fields.get(key).and_then(Value::as_f64);
    if let Some(v) = number("temp_nozzle") {
        t.nozzle_c = Some(v as f32);
    }
    if let Some(v) = number("target_nozzle") {
        t.nozzle_target_c = Some(v as f32);
    }
    if let Some(v) = number("fan_hotend") {
        t.hotend_fan_rpm = Some(v as u32);
    }
    if let Some(v) = number("fan_print") {
        t.print_fan_rpm = Some(v as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A Marlin printer on the agent: the head's values as PrusaLink's keys; a frame without them
    /// keeps what the one before said.
    #[test]
    fn prusalinks_keys_are_the_heads_values() {
        let mut t = Telemetry::default();
        read(
            &mut t,
            &json!({"state": "PRINTING", "temp_nozzle": 215.3, "target_nozzle": 215.0,
                "fan_hotend": 7100, "fan_print": 5200}),
        );
        assert_eq!((t.nozzle_c, t.nozzle_target_c), (Some(215.3), Some(215.0)));
        assert_eq!(
            (t.hotend_fan_rpm, t.print_fan_rpm),
            (Some(7100), Some(5200))
        );
        read(&mut t, &json!({"state": "PRINTING", "temp_nozzle": 216.0}));
        assert_eq!((t.nozzle_c, t.nozzle_target_c), (Some(216.0), Some(215.0)));
    }
}
