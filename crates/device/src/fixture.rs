//! Test fixtures: a fake device ([`FixtureDriver`], [`FixtureSession`]) with only the registry's
//! `fixture.*` verbs, and a fake host ([`FixtureHost`]) that runs host-side validation in front of
//! a session. No I/O: nothing here talks to a machine, so nothing here can start a real laser.
//!
//! [`Flaws`] make the fixture break one contract rule each, so the conformance kit's tests can show
//! that the kit catches it.

use crate::capability::{Arg, CapabilitySet, EnabledSet, Epoch, Kind, Unit, Verb, resolve};
use crate::command::{Caller, Command, Outcome};
use crate::fault;
use crate::identity::{FirmwareLabel, Identity};
use crate::registry;
use crate::session::{Driver, Session};
use crate::state::{Patch, StateTree};
use crate::validate::{self, Refusal};
use serde_json::json;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

/// Where the fixture device puts its state name in its state tree (a fixture convention only).
pub const FIXTURE_STATE_PATH: &str = "/state";

/// The fixture driver's baseline: every `fixture.*` verb, with bounds.
pub fn fixture_baseline() -> CapabilitySet {
    CapabilitySet::new()
        .with(
            "fixture.echo",
            Verb::new()
                .arg("text", Arg::new(Kind::String).max_abs(32.0))
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
        .with("fixture.stop", Verb::new())
        .with("fixture.person_only", Verb::new())
}

/// Contract rules the fixture can be told to break, one each.
#[derive(Debug, Clone, Copy, Default)]
pub struct Flaws {
    /// `baseline()` differs from call to call.
    pub impure_baseline: bool,
    /// The probe adds a verb the profile did not have.
    pub widening_probe: bool,
    /// `capabilities()` changes without its epoch moving.
    pub moving_capabilities: bool,
    /// The identity has no model.
    pub empty_model: bool,
    /// Answers ok after close.
    pub ok_after_close: bool,
    /// The state channel stays open after close.
    pub state_open_after_close: bool,
    /// Answers ok to a verb it does not have.
    pub ok_to_unknown_verb: bool,
}

/// The fake device's driver.
#[derive(Debug, Default)]
pub struct FixtureDriver {
    /// What the device's probe reports; `None` = all it is given.
    pub probe: Option<CapabilitySet>,
    /// The device never answers: every command is [`Outcome::Unknown`].
    pub silent: bool,
    pub flaws: Flaws,
    calls: AtomicU64,
}

impl FixtureDriver {
    pub fn new() -> Self {
        Self::default()
    }
    /// A device whose probe reports `probe`.
    pub fn probing(probe: CapabilitySet) -> Self {
        Self {
            probe: Some(probe),
            ..Self::default()
        }
    }
    /// A device that never answers.
    pub fn silent() -> Self {
        Self {
            silent: true,
            ..Self::default()
        }
    }
    pub fn with_flaws(flaws: Flaws) -> Self {
        Self {
            flaws,
            ..Self::default()
        }
    }
}

impl Driver for FixtureDriver {
    fn kind(&self) -> &'static str {
        "fixture"
    }

    fn baseline(&self) -> CapabilitySet {
        let n = self.calls.fetch_add(1, Ordering::Relaxed);
        if self.flaws.impure_baseline && n % 2 == 1 {
            return fixture_baseline().without("fixture.stop");
        }
        fixture_baseline()
    }

    fn connect(&self, profile: &CapabilitySet) -> Result<Box<dyn Session>, String> {
        let mut caps = match &self.probe {
            Some(probe) => profile.intersect(probe),
            None => profile.clone(),
        };
        if self.flaws.widening_probe {
            caps = caps.with("fixture.widened", Verb::new());
        }
        let (tx, rx) = channel();
        let _ = tx.send(Patch::set(FIXTURE_STATE_PATH, json!("IDLE")));
        let leaked = self.flaws.state_open_after_close.then(|| tx.clone());
        Ok(Box::new(FixtureSession {
            identity: Identity {
                vendor: "fixture".into(),
                model: if self.flaws.empty_model {
                    String::new()
                } else {
                    "fixture-1".into()
                },
                firmware: FirmwareLabel::new("0.0.0-fixture"),
                serial: "FX0001".into(),
                unit_uid: "fixture-unit-1".into(),
            },
            enabled: EnabledSet::new(caps),
            silent: self.silent,
            flaws: self.flaws,
            calls: AtomicU64::new(0),
            inner: Mutex::new(Inner {
                tx: Some(tx),
                rx: Some(rx),
                closed: false,
                _leaked: leaked,
            }),
        }))
    }
}

struct Inner {
    tx: Option<Sender<Patch>>,
    rx: Option<Receiver<Patch>>,
    closed: bool,
    _leaked: Option<Sender<Patch>>,
}

/// One fake device.
pub struct FixtureSession {
    identity: Identity,
    enabled: EnabledSet,
    silent: bool,
    flaws: Flaws,
    calls: AtomicU64,
    inner: Mutex<Inner>,
}

impl FixtureSession {
    fn send(&self, patch: Patch) {
        if let Some(tx) = &self.inner.lock().unwrap().tx {
            let _ = tx.send(patch);
        }
    }
}

impl Session for FixtureSession {
    fn identity(&self) -> Identity {
        self.identity.clone()
    }

    fn capabilities(&self) -> (CapabilitySet, Epoch) {
        let n = self.calls.fetch_add(1, Ordering::Relaxed);
        if self.flaws.moving_capabilities && n % 2 == 1 {
            return (
                self.enabled.set().clone().without("fixture.echo"),
                self.enabled.epoch(),
            );
        }
        (self.enabled.set().clone(), self.enabled.epoch())
    }

    fn do_command(&self, cmd: &Command) -> Outcome {
        if self.inner.lock().unwrap().closed {
            return if self.flaws.ok_after_close {
                Outcome::Ok
            } else {
                Outcome::Failed {
                    reason: "session closed".into(),
                }
            };
        }
        if !self.enabled.set().has(&cmd.verb) {
            return if self.flaws.ok_to_unknown_verb {
                Outcome::Ok
            } else {
                Outcome::Refused {
                    fault: fault::CAPABILITY_NOT_SUPPORTED.code(),
                    reason: None,
                }
            };
        }
        if self.silent {
            return Outcome::Unknown {
                reason: "no answer from the fixture device".into(),
            };
        }
        match cmd.verb.as_str() {
            "fixture.echo" if cmd.args.get("text").is_some_and(|t| t == "refuse") => {
                Outcome::Refused {
                    fault: fault::FIXTURE_REFUSED.code(),
                    reason: Some("fixture says no".into()),
                }
            }
            "fixture.move" => {
                let distance: f64 = cmd
                    .args
                    .get("distance")
                    .and_then(|d| d.parse().ok())
                    .unwrap_or_default();
                self.send(Patch::set("/position", json!(distance)));
                Outcome::Ok
            }
            "fixture.start" => {
                self.send(Patch::set(FIXTURE_STATE_PATH, json!("BUSY")));
                Outcome::Ok
            }
            "fixture.stop" => {
                self.send(Patch::set(FIXTURE_STATE_PATH, json!("IDLE")));
                Outcome::Ok
            }
            _ => Outcome::Ok,
        }
    }

    fn take_state(&self) -> Option<Receiver<Patch>> {
        self.inner.lock().unwrap().rx.take()
    }

    fn close(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.closed = true;
        inner.tx = None;
    }
}

/// A fake host: resolves the enabled set, keeps the state tree from the session's patches, and
/// checks every command ([`validate::check`]) before the session sees it.
pub struct FixtureHost {
    session: Box<dyn Session>,
    kind: &'static str,
    enabled: EnabledSet,
    patches: Receiver<Patch>,
    tree: StateTree,
}

impl FixtureHost {
    pub fn connect(driver: &dyn Driver, profile: &CapabilitySet) -> Result<Self, String> {
        let session = driver.connect(profile)?;
        let patches = session
            .take_state()
            .ok_or("the state channel was already taken")?;
        let (probed, _) = session.capabilities();
        let enabled = EnabledSet::new(resolve(&driver.baseline(), profile, &probed));
        Ok(Self {
            session,
            kind: driver.kind(),
            enabled,
            patches,
            tree: StateTree::new(),
        })
    }

    pub fn enabled(&self) -> &EnabledSet {
        &self.enabled
    }

    /// Applies every patch that has arrived; returns the tree.
    pub fn tree(&mut self) -> &StateTree {
        while let Ok(p) = self.patches.try_recv() {
            let _ = self.tree.apply(&p);
        }
        &self.tree
    }

    /// Checks the command, then gives it to the session.
    pub fn command(&mut self, caller: Caller, cmd: &Command) -> Result<Outcome, Refusal> {
        let state = self
            .tree()
            .get(FIXTURE_STATE_PATH)
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let checked = validate::check(
            self.enabled.set(),
            registry::state_set(self.kind),
            state.as_deref(),
            caller,
            cmd,
        )?;
        Ok(self.session.do_command(&checked))
    }

    pub fn close(&self) {
        self.session.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> FixtureHost {
        FixtureHost::connect(&FixtureDriver::new(), &fixture_baseline()).unwrap()
    }

    #[test]
    fn the_fixture_baseline_is_registered() {
        for (name, verb) in fixture_baseline().iter() {
            let entry = registry::verb(name).unwrap();
            for (arg, a) in &verb.args {
                let e = entry.args.iter().find(|e| e.name == arg).unwrap();
                assert_eq!((e.kind, e.unit), (a.kind, a.unit), "{name} {arg}");
            }
        }
    }

    #[test]
    fn host_validation_runs_before_the_device() {
        let mut h = host();
        let r = h
            .command(Caller::Person, &Command::new("fixture.move"))
            .unwrap_err();
        assert_eq!(r.fault, &fault::ARGUMENT_MISSING);
        assert_eq!(h.tree().get("/position"), None);
        assert_eq!(
            h.command(
                Caller::Person,
                &Command::new("fixture.move").arg("distance", "3")
            ),
            Ok(Outcome::Ok)
        );
        assert_eq!(h.tree().get("/position"), Some(&json!(3.0)));
    }

    #[test]
    fn the_state_guard_follows_the_device_state() {
        let mut h = host();
        assert_eq!(
            h.command(Caller::Person, &Command::new("fixture.start")),
            Ok(Outcome::Ok)
        );
        let r = h
            .command(Caller::Person, &Command::new("fixture.start"))
            .unwrap_err();
        assert_eq!(r.fault, &fault::STATE_NOT_ACCEPTED);
        assert_eq!(
            h.command(Caller::Person, &Command::new("fixture.stop")),
            Ok(Outcome::Ok)
        );
        assert_eq!(
            h.command(Caller::Person, &Command::new("fixture.start")),
            Ok(Outcome::Ok)
        );
    }

    #[test]
    fn a_person_only_verb_reaches_the_device_only_from_a_person() {
        let mut h = host();
        for caller in [Caller::Queue, Caller::Token] {
            assert_eq!(
                h.command(caller, &Command::new("fixture.person_only"))
                    .unwrap_err()
                    .fault,
                &fault::PERSON_ONLY
            );
        }
        assert_eq!(
            h.command(Caller::Person, &Command::new("fixture.person_only")),
            Ok(Outcome::Ok)
        );
    }

    #[test]
    fn a_device_refusal_is_a_result_with_a_code_and_the_device_words() {
        let mut h = host();
        let got = h
            .command(
                Caller::Person,
                &Command::new("fixture.echo").arg("text", "refuse"),
            )
            .unwrap();
        assert_eq!(
            got,
            Outcome::Refused {
                fault: "fixture/REFUSED".into(),
                reason: Some("fixture says no".into())
            }
        );
    }

    #[test]
    fn a_silent_device_gives_an_unknown_outcome_never_ok() {
        let d = FixtureDriver {
            silent: true,
            ..FixtureDriver::new()
        };
        let mut h = FixtureHost::connect(&d, &fixture_baseline()).unwrap();
        let got = h
            .command(Caller::Person, &Command::new("fixture.echo"))
            .unwrap();
        assert!(matches!(got, Outcome::Unknown { .. }));
    }

    #[test]
    fn the_host_commands_against_the_resolved_set() {
        let d = FixtureDriver {
            probe: Some(fixture_baseline().without("fixture.echo")),
            ..FixtureDriver::new()
        };
        let mut h = FixtureHost::connect(&d, &fixture_baseline().without("fixture.start")).unwrap();
        assert!(!h.enabled().set().has("fixture.echo") && !h.enabled().set().has("fixture.start"));
        let r = h
            .command(Caller::Person, &Command::new("fixture.echo"))
            .unwrap_err();
        assert_eq!(r.fault, &fault::CAPABILITY_NOT_SUPPORTED);
    }
}
