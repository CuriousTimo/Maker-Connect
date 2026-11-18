//! The conformance kit every driver and every port adapter passes (hub-core-v2.md §2.1, step 1):
//! `assert!(check_driver(&driver, safe).is_empty())` in its tests.
//!
//! The rules follow the meaning of Prusa's contract suite (`sdk/connectorapi/connectorapitest`,
//! `connector-contract.md`; read only, OCL), as far as this crate's contract has them. The kit
//! connects to the device several times; it sends only an undeclared verb, and the caller's
//! `safe` command only after the session is closed.

use crate::capability::CapabilitySet;
use crate::command::{Command, Outcome};
use crate::fault;
use crate::registry;
use crate::session::Driver;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

/// A verb no device declares.
pub const UNDECLARED_VERB: &str = "conformance.undeclared";

/// How long a closed session's state channel may take to end.
pub const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// One broken rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub rule: &'static str,
    pub detail: String,
}

pub const BASELINE_PURE: &str = "baseline is pure";
pub const BASELINE_REGISTERED: &str = "baseline holds only registered verbs of the driver's kind";
pub const KIND_REGISTERED: &str = "the driver's kind has a registered state set";
pub const CONNECTS: &str = "connects";
pub const IDENTITY_MODEL: &str = "identity has a model";
pub const PROBE_NARROWS: &str = "probe narrows, never widens";
pub const STABLE_IN_EPOCH: &str = "capabilities do not move within an epoch";
pub const UNDECLARED_REFUSED: &str = "a verb not in the set is refused";
pub const STATE_ONCE: &str = "the state channel is given once";
pub const CLOSE_IDEMPOTENT: &str = "close is idempotent";
pub const STATE_CLOSES: &str = "the state channel ends on close";
pub const NO_OK_UNOBSERVED: &str = "never ok without observation";

/// Runs every rule against `driver`. `safe` is a command of the driver's that is harmless, sent
/// only to a closed session (it must not be ok); `None` skips that part.
pub fn check_driver(driver: &dyn Driver, safe: Option<&Command>) -> Vec<Violation> {
    let mut out = Vec::new();
    let mut fail = |rule, detail: String| out.push(Violation { rule, detail });

    let baseline = driver.baseline();
    if driver.baseline() != baseline {
        fail(BASELINE_PURE, "two calls gave two sets".into());
    }
    let kind = driver.kind();
    if registry::state_set(kind).is_none() {
        fail(KIND_REGISTERED, kind.into());
    }
    for detail in unregistered(&baseline, kind) {
        fail(BASELINE_REGISTERED, detail);
    }

    let narrowed = match baseline.names().next() {
        Some(first) => baseline.clone().without(first),
        None => baseline.clone(),
    };
    for (name, profile) in [
        ("baseline", &baseline),
        ("baseline less a verb", &narrowed),
        ("empty", &CapabilitySet::new()),
    ] {
        let session = match driver.connect(profile) {
            Ok(s) => s,
            Err(e) => {
                fail(CONNECTS, format!("profile {name}: {e}"));
                continue;
            }
        };
        let (caps, _) = session.capabilities();
        if !caps.is_within(profile) || !caps.is_within(&baseline) {
            fail(
                PROBE_NARROWS,
                format!("profile {name}: the session's set is wider"),
            );
        }
        session.close();
    }

    let session = match driver.connect(&baseline) {
        Ok(s) => s,
        Err(e) => {
            fail(CONNECTS, e);
            return out;
        }
    };
    if session.identity().model.is_empty() {
        fail(IDENTITY_MODEL, "empty model".into());
    }
    let first = session.capabilities();
    for _ in 0..3 {
        let again = session.capabilities();
        if again.1 == first.1 && again.0 != first.0 {
            fail(
                STABLE_IN_EPOCH,
                format!("epoch {:?} gave two sets", first.1),
            );
            break;
        }
    }
    match session.do_command(&Command::new(UNDECLARED_VERB)) {
        Outcome::Refused { fault: f, .. } if f == fault::CAPABILITY_NOT_SUPPORTED.code() => {}
        other => fail(UNDECLARED_REFUSED, format!("{other:?}")),
    }
    let state = session.take_state();
    if state.is_none() {
        fail(STATE_ONCE, "not given the first time".into());
    }
    if session.take_state().is_some() {
        fail(STATE_ONCE, "given twice".into());
    }
    session.close();
    if catch_unwind(AssertUnwindSafe(|| session.close())).is_err() {
        fail(CLOSE_IDEMPOTENT, "the second close panicked".into());
    }
    if let Some(rx) = state {
        let deadline = Instant::now() + CLOSE_WAIT;
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(_) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    fail(
                        STATE_CLOSES,
                        format!("still open {CLOSE_WAIT:?} after close"),
                    );
                    break;
                }
            }
        }
    }
    for cmd in [Some(&Command::new(UNDECLARED_VERB)), safe]
        .into_iter()
        .flatten()
    {
        if session.do_command(cmd).is_ok() {
            fail(
                NO_OK_UNOBSERVED,
                format!("{} answered ok after close", cmd.verb),
            );
        }
    }
    out
}

/// Verbs and arguments of `set` the registry does not have for `kind` (or has with another kind
/// or unit).
pub fn unregistered(set: &CapabilitySet, kind: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (name, verb) in set.iter() {
        let Some(entry) = registry::verb(name).filter(|e| e.kinds.contains(&kind)) else {
            out.push(format!("{name} is not a registered verb of kind {kind}"));
            continue;
        };
        for (arg, a) in &verb.args {
            match entry.args.iter().find(|e| e.name == arg) {
                Some(e) if e.kind == a.kind && e.unit == a.unit => {}
                Some(_) => out.push(format!(
                    "{name} {arg}: kind or unit differs from the registry"
                )),
                None => out.push(format!("{name} {arg}: not a registered argument")),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{Arg, Kind, Verb};
    use crate::fixture::{FixtureDriver, Flaws, fixture_baseline};

    fn safe() -> Command {
        Command::new("fixture.echo")
    }

    fn rules(driver: &dyn Driver) -> Vec<&'static str> {
        check_driver(driver, Some(&safe()))
            .into_iter()
            .map(|v| v.rule)
            .collect()
    }

    #[test]
    fn the_fixture_device_passes() {
        assert_eq!(check_driver(&FixtureDriver::new(), Some(&safe())), vec![]);
        let probing = FixtureDriver::probing(fixture_baseline().without("fixture.move"));
        assert_eq!(check_driver(&probing, Some(&safe())), vec![]);
        let silent = FixtureDriver::silent();
        assert_eq!(check_driver(&silent, Some(&safe())), vec![]);
    }

    #[test]
    fn each_broken_rule_is_caught() {
        let cases: [(Flaws, &str); 7] = [
            (
                Flaws {
                    impure_baseline: true,
                    ..Flaws::default()
                },
                BASELINE_PURE,
            ),
            (
                Flaws {
                    widening_probe: true,
                    ..Flaws::default()
                },
                PROBE_NARROWS,
            ),
            (
                Flaws {
                    moving_capabilities: true,
                    ..Flaws::default()
                },
                STABLE_IN_EPOCH,
            ),
            (
                Flaws {
                    empty_model: true,
                    ..Flaws::default()
                },
                IDENTITY_MODEL,
            ),
            (
                Flaws {
                    ok_after_close: true,
                    ..Flaws::default()
                },
                NO_OK_UNOBSERVED,
            ),
            (
                Flaws {
                    state_open_after_close: true,
                    ..Flaws::default()
                },
                STATE_CLOSES,
            ),
            (
                Flaws {
                    ok_to_unknown_verb: true,
                    ..Flaws::default()
                },
                UNDECLARED_REFUSED,
            ),
        ];
        for (flaws, rule) in cases {
            let got = rules(&FixtureDriver::with_flaws(flaws));
            assert!(
                got.contains(&rule),
                "{flaws:?}: expected {rule:?}, got {got:?}"
            );
        }
    }

    #[test]
    fn an_unregistered_verb_or_argument_is_caught() {
        let set = fixture_baseline()
            .with("fixture.unregistered", Verb::new())
            .with(
                "fixture.start",
                Verb::new().arg("x", Arg::new(Kind::Integer)),
            )
            .with("fixture.stop", Verb::new());
        let got = unregistered(&set, "fixture");
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(!unregistered(&fixture_baseline(), "other").is_empty());
        assert!(unregistered(&fixture_baseline(), "fixture").is_empty());
    }
}
