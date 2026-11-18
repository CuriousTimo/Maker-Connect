//! Host-side checks, run before a driver sees a command (hub-core-v2.md §2.1, host-side rules).
//! Our own rules: they never change what a port sends, they only decide whether it is asked.
//!
//! In this order, the first refusal wins: the verb is in the enabled set and registered; a
//! person-only verb comes from a person's session; the current state accepts the verb; every
//! argument is declared, present when required (or defaulted), of its kind, allowed and in bounds.
//! Argument checks follow the meaning of `sdk/connectorapi/validate.go` (read only, OCL).

use crate::capability::{Arg, CapabilitySet, Kind};
use crate::command::{Caller, Command};
use crate::fault::{self, FaultCode};
use crate::registry;
use crate::state::StateSet;

/// A command the host refused: a registered code and what it was about.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub fault: &'static FaultCode,
    /// `(name, value)` pairs: `verb`, `argument`, `state`, `allowed`, `max_abs`, `min`, `kind`.
    pub params: Vec<(&'static str, String)>,
}

impl Refusal {
    fn new(fault: &'static FaultCode, verb: &str) -> Self {
        Self {
            fault,
            params: vec![("verb", verb.to_owned())],
        }
    }
    fn with(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.params.push((name, value.into()));
        self
    }
}

/// Checks `cmd` for a device whose enabled set is `enabled`, whose kind declares `states`, and
/// whose current state is `current` (`None`: not reported yet). Returns the command the driver is
/// given: the same verb, the sent arguments and the declared defaults for those not sent.
pub fn check(
    enabled: &CapabilitySet,
    states: Option<&StateSet>,
    current: Option<&str>,
    caller: Caller,
    cmd: &Command,
) -> Result<Command, Refusal> {
    let verb = cmd.verb.as_str();
    let (Some(declared), Some(entry)) = (enabled.get(verb), registry::verb(verb)) else {
        let names: Vec<&str> = enabled.names().collect();
        return Err(
            Refusal::new(&fault::CAPABILITY_NOT_SUPPORTED, verb).with("verbs", names.join(","))
        );
    };
    if entry.person_only && caller != Caller::Person {
        return Err(Refusal::new(&fault::PERSON_ONLY, verb));
    }
    if let Some(states) = states {
        match current {
            Some(state) if states.accepts(state, verb) => {}
            other => {
                return Err(Refusal::new(&fault::STATE_NOT_ACCEPTED, verb)
                    .with("state", other.unwrap_or("unknown")));
            }
        }
    }
    if let Some(name) = cmd.args.keys().find(|n| !declared.args.contains_key(*n)) {
        return Err(Refusal::new(&fault::ARGUMENT_UNKNOWN, verb).with("argument", name.as_str()));
    }
    let mut args = cmd.args.clone();
    for (name, arg) in &declared.args {
        match (cmd.args.get(name), &arg.default) {
            (Some(value), _) => check_value(verb, name, arg, value)?,
            (None, Some(default)) => {
                check_value(verb, name, arg, default)?;
                args.insert(name.clone(), default.clone());
            }
            (None, None) if arg.required => {
                return Err(
                    Refusal::new(&fault::ARGUMENT_MISSING, verb).with("argument", name.as_str())
                );
            }
            (None, None) => {}
        }
    }
    Ok(Command {
        verb: cmd.verb.clone(),
        args,
    })
}

fn check_value(verb: &str, name: &str, arg: &Arg, value: &str) -> Result<(), Refusal> {
    let refuse = |fault| Refusal::new(fault, verb).with("argument", name);
    if !arg.allowed.is_empty() && !arg.allowed.iter().any(|a| a == value) {
        return Err(refuse(&fault::ARGUMENT_NOT_SUPPORTED).with("allowed", arg.allowed.join(",")));
    }
    let Some(number) = read_as(arg.kind, value) else {
        return Err(refuse(&fault::ARGUMENT_MALFORMED).with("kind", arg.kind.as_str()));
    };
    if let Some(max) = arg.max_abs
        && (!max.is_finite() || number.abs() > max)
    {
        return Err(refuse(&fault::ARGUMENT_OUT_OF_RANGE).with("max_abs", max.to_string()));
    }
    if let Some(min) = arg.min
        && matches!(arg.kind, Kind::Integer | Kind::Number)
        && (!min.is_finite() || number < min)
    {
        return Err(refuse(&fault::ARGUMENT_OUT_OF_RANGE).with("min", min.to_string()));
    }
    Ok(())
}

/// The number the bounds apply to (the value, or a string's length in bytes); `None` when the
/// text is not of the kind. A number must be finite.
fn read_as(kind: Kind, value: &str) -> Option<f64> {
    match kind {
        Kind::String => Some(value.len() as f64),
        Kind::Integer => value.parse::<i64>().ok().map(|n| n as f64),
        Kind::Number => value.parse::<f64>().ok().filter(|n| n.is_finite()),
        Kind::Boolean => (value == "true" || value == "false").then_some(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{Unit, Verb};

    fn enabled() -> CapabilitySet {
        CapabilitySet::new()
            .with(
                "fixture.echo",
                Verb::new()
                    .arg("text", Arg::new(Kind::String).max_abs(4.0))
                    .arg(
                        "tone",
                        Arg::new(Kind::String)
                            .allowed(&["low", "high"])
                            .default_value("low"),
                    ),
            )
            .with(
                "fixture.move",
                Verb::new()
                    .arg(
                        "distance",
                        Arg::new(Kind::Number)
                            .unit(Unit::Millimeter)
                            .required()
                            .max_abs(100.0),
                    )
                    .arg(
                        "speed",
                        Arg::new(Kind::Integer)
                            .unit(Unit::MillimeterPerMinute)
                            .min(1.0),
                    ),
            )
            .with("fixture.start", Verb::new())
            .with("fixture.person_only", Verb::new())
    }

    fn states() -> &'static StateSet {
        registry::state_set("fixture").unwrap()
    }

    fn run(caller: Caller, state: Option<&str>, cmd: Command) -> Result<Command, Refusal> {
        check(&enabled(), Some(states()), state, caller, &cmd)
    }

    fn refused(cmd: Command) -> &'static FaultCode {
        run(Caller::Person, Some("IDLE"), cmd).unwrap_err().fault
    }

    #[test]
    fn a_good_command_passes_with_defaults_filled_in() {
        let got = run(
            Caller::Queue,
            Some("IDLE"),
            Command::new("fixture.echo").arg("text", "hi"),
        )
        .unwrap();
        assert_eq!(
            got,
            Command::new("fixture.echo")
                .arg("text", "hi")
                .arg("tone", "low")
        );
        let got = run(
            Caller::Token,
            Some("IDLE"),
            Command::new("fixture.move").arg("distance", "-12.5"),
        )
        .unwrap();
        assert_eq!(got, Command::new("fixture.move").arg("distance", "-12.5"));
    }

    #[test]
    fn a_verb_not_in_the_enabled_set_is_refused() {
        assert_eq!(
            refused(Command::new("fixture.stop")),
            &fault::CAPABILITY_NOT_SUPPORTED
        );
    }

    #[test]
    fn an_enabled_verb_that_is_not_registered_is_refused() {
        let set = enabled().with("fixture.unregistered", Verb::new());
        let r = check(
            &set,
            None,
            None,
            Caller::Person,
            &Command::new("fixture.unregistered"),
        )
        .unwrap_err();
        assert_eq!(r.fault, &fault::CAPABILITY_NOT_SUPPORTED);
    }

    #[test]
    fn an_unknown_argument_is_refused() {
        let r = run(
            Caller::Person,
            Some("IDLE"),
            Command::new("fixture.start").arg("x", "1"),
        )
        .unwrap_err();
        assert_eq!(r.fault, &fault::ARGUMENT_UNKNOWN);
        assert!(r.params.contains(&("argument", "x".to_owned())));
    }

    #[test]
    fn a_missing_required_argument_is_refused() {
        assert_eq!(
            refused(Command::new("fixture.move")),
            &fault::ARGUMENT_MISSING
        );
    }

    #[test]
    fn a_value_not_of_its_kind_is_refused() {
        for (arg, v) in [
            ("distance", "abc"),
            ("distance", "inf"),
            ("distance", "NaN"),
        ] {
            assert_eq!(
                refused(Command::new("fixture.move").arg(arg, v)),
                &fault::ARGUMENT_MALFORMED,
                "{v}"
            );
        }
        let cmd = Command::new("fixture.move")
            .arg("distance", "1")
            .arg("speed", "1.5");
        assert_eq!(refused(cmd), &fault::ARGUMENT_MALFORMED);
    }

    #[test]
    fn a_value_outside_its_bounds_is_refused() {
        assert_eq!(
            refused(Command::new("fixture.move").arg("distance", "-100.5")),
            &fault::ARGUMENT_OUT_OF_RANGE
        );
        let cmd = Command::new("fixture.move")
            .arg("distance", "1")
            .arg("speed", "0");
        assert_eq!(refused(cmd), &fault::ARGUMENT_OUT_OF_RANGE);
        assert_eq!(
            refused(Command::new("fixture.echo").arg("text", "hello")),
            &fault::ARGUMENT_OUT_OF_RANGE
        );
        assert!(
            run(
                Caller::Person,
                Some("IDLE"),
                Command::new("fixture.move").arg("distance", "100")
            )
            .is_ok()
        );
    }

    #[test]
    fn a_value_outside_the_allowed_set_is_refused() {
        assert_eq!(
            refused(Command::new("fixture.echo").arg("tone", "mid")),
            &fault::ARGUMENT_NOT_SUPPORTED
        );
    }

    #[test]
    fn a_boolean_is_only_true_or_false() {
        let set = CapabilitySet::new().with(
            "fixture.echo",
            Verb::new().arg("text", Arg::new(Kind::Boolean)),
        );
        let ok = |v| {
            check(
                &set,
                None,
                None,
                Caller::Person,
                &Command::new("fixture.echo").arg("text", v),
            )
            .is_ok()
        };
        assert!(ok("true") && ok("false"));
        assert!(!ok("TRUE") && !ok("1"));
    }

    #[test]
    fn a_verb_the_current_state_does_not_accept_is_refused() {
        let r = run(Caller::Person, Some("BUSY"), Command::new("fixture.start")).unwrap_err();
        assert_eq!(r.fault, &fault::STATE_NOT_ACCEPTED);
        assert!(r.params.contains(&("state", "BUSY".to_owned())));
        let r = run(Caller::Person, None, Command::new("fixture.start")).unwrap_err();
        assert_eq!(r.fault, &fault::STATE_NOT_ACCEPTED);
        assert!(r.params.contains(&("state", "unknown".to_owned())));
    }

    #[test]
    fn a_person_only_verb_is_refused_from_the_queue_and_a_token() {
        for caller in [Caller::Queue, Caller::Token] {
            let r = run(caller, Some("IDLE"), Command::new("fixture.person_only")).unwrap_err();
            assert_eq!(r.fault, &fault::PERSON_ONLY);
        }
        assert!(
            run(
                Caller::Person,
                Some("IDLE"),
                Command::new("fixture.person_only")
            )
            .is_ok()
        );
    }

    #[test]
    fn every_code_the_validator_returns_is_registered() {
        for f in [
            &fault::CAPABILITY_NOT_SUPPORTED,
            &fault::ARGUMENT_UNKNOWN,
            &fault::ARGUMENT_MISSING,
            &fault::ARGUMENT_NOT_SUPPORTED,
            &fault::ARGUMENT_MALFORMED,
            &fault::ARGUMENT_OUT_OF_RANGE,
            &fault::STATE_NOT_ACCEPTED,
            &fault::PERSON_ONLY,
        ] {
            assert_eq!(registry::fault_code(&f.code()), Some(f));
        }
    }
}
