//! Commands and their outcomes.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One request for a device to do something.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    /// A verb of the device's enabled set.
    pub verb: String,
    /// Argument values as text, read by their declared kind. After host validation they are
    /// checked and defaults are filled in, so a driver does not check them again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, String>,
}

impl Command {
    pub fn new(verb: &str) -> Self {
        Self {
            verb: verb.to_owned(),
            args: BTreeMap::new(),
        }
    }
    pub fn arg(mut self, name: &str, value: &str) -> Self {
        self.args.insert(name.to_owned(), value.to_owned());
        self
    }
}

/// Who sent a command. Only [`Caller::Person`] may send a person-only verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Caller {
    /// A person, from their own logged-in session.
    Person,
    /// The queue, on its own.
    Queue,
    /// An API token.
    Token,
}

/// How a command ended, as far as it was observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum Outcome {
    /// The device was seen to carry it out. Only for an observed outcome.
    Ok,
    /// The device refused it: a registered code (`domain/REASON`, branch on this) and the device's
    /// own words for it, unchanged (`None` when the device gives none).
    Refused {
        fault: String,
        reason: Option<String>,
    },
    /// It did not reach the device, or the device took it and was seen to fail.
    Failed { reason: String },
    /// Sent, but no answer was observed: it may or may not have happened. Never re-sent on its own.
    Unknown { reason: String },
}

impl Outcome {
    pub fn is_ok(&self) -> bool {
        matches!(self, Outcome::Ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_serialise_with_a_tag() {
        assert_eq!(
            serde_json::to_value(Outcome::Ok).unwrap(),
            serde_json::json!({"outcome":"ok"})
        );
        let refused = Outcome::Refused {
            fault: "fixture/REFUSED".into(),
            reason: Some("door open".into()),
        };
        let json = serde_json::to_value(&refused).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"outcome":"refused","fault":"fixture/REFUSED","reason":"door open"})
        );
        assert_eq!(serde_json::from_value::<Outcome>(json).unwrap(), refused);
        assert!(
            !Outcome::Unknown {
                reason: "no answer".into()
            }
            .is_ok()
        );
    }

    #[test]
    fn a_command_round_trips() {
        let c = Command::new("v").arg("a", "1");
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), c);
    }
}
