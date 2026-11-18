//! Fault codes: `domain/REASON`, the HTTP status a caller gets, and our text. A caller branches on
//! the code, never on a text. The table of all of them is [`crate::registry::FAULT_CODES`].
//!
//! The reason names of the validator's codes are those of Prusa's connector contract for the same
//! meaning (`sdk/connectorapi/faults.go`, read only); the domains and texts are ours.
//! `STATE_NOT_ACCEPTED` and `PERSON_ONLY` are ours (hub-core-v2.md §2.1, host-side rules).

/// One registered fault code.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct FaultCode {
    pub domain: &'static str,
    pub reason: &'static str,
    pub status: u16,
    pub text: &'static str,
}

impl FaultCode {
    /// `domain/REASON`, as callers see it.
    pub fn code(&self) -> String {
        format!("{}/{}", self.domain, self.reason)
    }
}

/// The verb is not in the device's enabled set (the host refuses it; a driver refuses it too).
pub static CAPABILITY_NOT_SUPPORTED: FaultCode = FaultCode {
    domain: "device",
    reason: "CAPABILITY_NOT_SUPPORTED",
    status: 409,
    text: "The device does not have that capability in its current set.",
};
/// The command carries an argument the verb does not declare.
pub static ARGUMENT_UNKNOWN: FaultCode = FaultCode {
    domain: "validation",
    reason: "ARGUMENT_UNKNOWN",
    status: 400,
    text: "The verb does not take that argument.",
};
/// A required argument without a default was not sent.
pub static ARGUMENT_MISSING: FaultCode = FaultCode {
    domain: "validation",
    reason: "ARGUMENT_MISSING",
    status: 400,
    text: "A required argument was not sent.",
};
/// The value is not one of the argument's allowed values.
pub static ARGUMENT_NOT_SUPPORTED: FaultCode = FaultCode {
    domain: "validation",
    reason: "ARGUMENT_NOT_SUPPORTED",
    status: 400,
    text: "The value is not one of the values the argument allows.",
};
/// The value cannot be read as the argument's kind.
pub static ARGUMENT_MALFORMED: FaultCode = FaultCode {
    domain: "validation",
    reason: "ARGUMENT_MALFORMED",
    status: 400,
    text: "The value cannot be read as the argument's kind.",
};
/// The value is outside the argument's bounds.
pub static ARGUMENT_OUT_OF_RANGE: FaultCode = FaultCode {
    domain: "validation",
    reason: "ARGUMENT_OUT_OF_RANGE",
    status: 400,
    text: "The value is outside the bounds the argument declares.",
};
/// The device's current state does not accept the verb (or its state is not known yet).
pub static STATE_NOT_ACCEPTED: FaultCode = FaultCode {
    domain: "device",
    reason: "STATE_NOT_ACCEPTED",
    status: 409,
    text: "The device's current state does not accept that verb.",
};
/// The verb is accepted only from a person's own session, never from the queue or a token.
pub static PERSON_ONLY: FaultCode = FaultCode {
    domain: "authz",
    reason: "PERSON_ONLY",
    status: 403,
    text: "Only a person at their own session may send that verb; never the queue or a token.",
};
/// Test fixture only: the fixture device refused a command in its own words.
#[cfg(test)]
pub static FIXTURE_REFUSED: FaultCode = FaultCode {
    domain: "fixture",
    reason: "REFUSED",
    status: 409,
    text: "Test fixture: the fixture device refused the command.",
};
