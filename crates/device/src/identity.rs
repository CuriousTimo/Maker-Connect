//! What a device says it is.

use serde::{Deserialize, Serialize};

/// What a device reported about itself when its session was made. Compared whole with `==`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Identity {
    pub vendor: String,
    /// Must not be empty: the model profile is chosen by it.
    pub model: String,
    pub firmware: FirmwareLabel,
    pub serial: String,
    pub unit_uid: String,
}

/// A firmware version exactly as the device reported it. A label, never ordered: it has no
/// `PartialOrd`/`Ord`, so it cannot be compared as a version. Behaviour follows capabilities.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FirmwareLabel(String);

impl FirmwareLabel {
    pub fn new(reported: impl Into<String>) -> Self {
        Self(reported.into())
    }

    /// The reported text, unchanged.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The device reported no firmware.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_label_keeps_the_reported_text_and_compares_only_for_equality() {
        let a = FirmwareLabel::new("6.4.0");
        let b = FirmwareLabel::new("6.10.0");
        assert_eq!(a.as_str(), "6.4.0");
        assert_ne!(a, b);
        assert!(FirmwareLabel::default().is_empty());
    }

    #[test]
    fn identity_round_trips_as_json_with_its_field_names() {
        let id = Identity {
            vendor: "v".into(),
            model: "m".into(),
            firmware: FirmwareLabel::new("1.0"),
            serial: "s".into(),
            unit_uid: "u".into(),
        };
        let json = serde_json::to_value(&id).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"vendor":"v","model":"m","firmware":"1.0","serial":"s","unit_uid":"u"})
        );
        assert_eq!(serde_json::from_value::<Identity>(json).unwrap(), id);
    }
}
