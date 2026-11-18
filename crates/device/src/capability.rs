//! Capabilities: the verbs a device can carry out, each with typed arguments, and the enabled set
//! = driver baseline ∩ model profile ∩ device probe, with an epoch.
//!
//! Meaning after `sdk/connectorapi/set.go` and `capability.go` (read only, OCL): a profile or a
//! probe only narrows, there is no union; the narrower bound and the smaller allowed set win; a
//! required argument stays required; a verb that loses a required argument is dropped.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How an argument's text is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Any text; the bounds apply to its length in bytes.
    String,
    /// A whole number (`i64`).
    Integer,
    /// A finite decimal number (`f64`).
    Number,
    /// `true` or `false`, nothing else.
    Boolean,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::String => "string",
            Kind::Integer => "integer",
            Kind::Number => "number",
            Kind::Boolean => "boolean",
        }
    }
}

/// The unit an argument or a metric is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    #[serde(rename = "celsius")]
    Celsius,
    #[serde(rename = "percent")]
    Percent,
    #[serde(rename = "mm")]
    Millimeter,
    #[serde(rename = "mm/min")]
    MillimeterPerMinute,
    #[serde(rename = "s")]
    Second,
    #[serde(rename = "byte")]
    Byte,
}

impl Unit {
    pub fn as_str(self) -> &'static str {
        match self {
            Unit::Celsius => "celsius",
            Unit::Percent => "percent",
            Unit::Millimeter => "mm",
            Unit::MillimeterPerMinute => "mm/min",
            Unit::Second => "s",
            Unit::Byte => "byte",
        }
    }
}

/// One typed argument of a verb.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arg {
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<Unit>,
    /// A command without it is refused, unless there is a default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    /// Sent when the caller sends none; checked like a sent value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// The only values accepted; empty = any value of the kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed: Vec<String>,
    /// Upper bound on the magnitude (`|value|`, or a string's length), inclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_abs: Option<f64>,
    /// Lower bound on a number, inclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
}

impl Arg {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            unit: None,
            required: false,
            default: None,
            allowed: Vec::new(),
            max_abs: None,
            min: None,
        }
    }
    pub fn unit(mut self, unit: Unit) -> Self {
        self.unit = Some(unit);
        self
    }
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
    pub fn default_value(mut self, value: &str) -> Self {
        self.default = Some(value.to_owned());
        self
    }
    pub fn allowed(mut self, values: &[&str]) -> Self {
        self.allowed = values.iter().map(|v| (*v).to_owned()).collect();
        self
    }
    pub fn max_abs(mut self, bound: f64) -> Self {
        self.max_abs = Some(bound);
        self
    }
    pub fn min(mut self, bound: f64) -> Self {
        self.min = Some(bound);
        self
    }

    fn canonical_allowed(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.allowed.iter().map(String::as_str).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The narrower of two declarations of one argument; `None` when they cannot meet (other
    /// kind, other unit, no allowed value in common).
    fn meet(&self, other: &Arg) -> Option<Arg> {
        if self.kind != other.kind {
            return None;
        }
        let unit = match (self.unit, other.unit) {
            (None, u) | (u, None) => u,
            (Some(a), Some(b)) if a == b => Some(a),
            _ => return None,
        };
        let allowed: Vec<String> = match (self.allowed.is_empty(), other.allowed.is_empty()) {
            (true, _) => other
                .canonical_allowed()
                .into_iter()
                .map(str::to_owned)
                .collect(),
            (_, true) => self
                .canonical_allowed()
                .into_iter()
                .map(str::to_owned)
                .collect(),
            _ => {
                let theirs = other.canonical_allowed();
                let common: Vec<String> = self
                    .canonical_allowed()
                    .into_iter()
                    .filter(|v| theirs.contains(v))
                    .map(str::to_owned)
                    .collect();
                if common.is_empty() {
                    return None;
                }
                common
            }
        };
        let default = match (&self.default, &other.default) {
            (Some(a), Some(b)) if a == b && (allowed.is_empty() || allowed.contains(a)) => {
                Some(a.clone())
            }
            _ => None,
        };
        Some(Arg {
            kind: self.kind,
            unit,
            required: self.required || other.required,
            default,
            allowed,
            max_abs: narrower(self.max_abs, other.max_abs, f64::min),
            min: narrower(self.min, other.min, f64::max),
        })
    }
}

fn narrower(a: Option<f64>, b: Option<f64>, pick: fn(f64, f64) -> f64) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(pick(a, b)),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Equal when they accept the same values: the order and repetition of `allowed` do not count.
impl PartialEq for Arg {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.unit == other.unit
            && self.required == other.required
            && self.default == other.default
            && self.canonical_allowed() == other.canonical_allowed()
            && self.max_abs == other.max_abs
            && self.min == other.min
    }
}

/// One verb a device can carry out, with its arguments by name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Verb {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, Arg>,
}

impl Verb {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn arg(mut self, name: &str, arg: Arg) -> Self {
        self.args.insert(name.to_owned(), arg);
        self
    }
}

/// A set of verbs by name. Empty is a valid set (a device that can do nothing).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySet {
    verbs: BTreeMap<String, Verb>,
}

impl CapabilitySet {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with(mut self, name: &str, verb: Verb) -> Self {
        self.verbs.insert(name.to_owned(), verb);
        self
    }
    pub fn without(mut self, name: &str) -> Self {
        self.verbs.remove(name);
        self
    }
    pub fn get(&self, name: &str) -> Option<&Verb> {
        self.verbs.get(name)
    }
    pub fn has(&self, name: &str) -> bool {
        self.verbs.contains_key(name)
    }
    /// Verb names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.verbs.keys().map(String::as_str)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Verb)> {
        self.verbs.iter().map(|(n, v)| (n.as_str(), v))
    }
    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    /// What both sets permit. A verb or argument absent from either is absent; an argument whose
    /// declarations cannot meet is dropped; a verb that lost an argument either side requires is
    /// dropped (or a caller could send it without that argument).
    pub fn intersect(&self, other: &CapabilitySet) -> CapabilitySet {
        let mut out = BTreeMap::new();
        for (name, mine) in &self.verbs {
            let Some(theirs) = other.verbs.get(name) else {
                continue;
            };
            let mut args = BTreeMap::new();
            let mut lost_required = false;
            for (arg_name, a) in &mine.args {
                match theirs.args.get(arg_name).and_then(|b| a.meet(b)) {
                    Some(merged) => {
                        args.insert(arg_name.clone(), merged);
                    }
                    None => {
                        lost_required |=
                            a.required || theirs.args.get(arg_name).is_some_and(|b| b.required)
                    }
                }
            }
            lost_required |= theirs
                .args
                .iter()
                .any(|(n, b)| b.required && !mine.args.contains_key(n));
            if !lost_required {
                out.insert(name.clone(), Verb { args });
            }
        }
        CapabilitySet { verbs: out }
    }

    /// Whether `self` permits nothing `ceiling` does not (it is `ceiling` or narrower).
    pub fn is_within(&self, ceiling: &CapabilitySet) -> bool {
        self.intersect(ceiling) == *self
    }
}

/// The enabled set: driver baseline ∩ model profile ∩ device probe.
pub fn resolve(
    baseline: &CapabilitySet,
    profile: &CapabilitySet,
    probe: &CapabilitySet,
) -> CapabilitySet {
    baseline.intersect(profile).intersect(probe)
}

/// Names one enabled set of a session. It changes whenever the set changes, never within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Epoch(pub u64);

/// An enabled set with its epoch.
#[derive(Debug, Clone, PartialEq)]
pub struct EnabledSet {
    set: CapabilitySet,
    epoch: Epoch,
}

impl EnabledSet {
    pub fn new(set: CapabilitySet) -> Self {
        Self {
            set,
            epoch: Epoch(1),
        }
    }
    pub fn set(&self) -> &CapabilitySet {
        &self.set
    }
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }
    /// Puts in a newly resolved set; the epoch moves only when it differs. Returns whether it moved.
    pub fn update(&mut self, set: CapabilitySet) -> bool {
        if set == self.set {
            return false;
        }
        self.set = set;
        self.epoch = Epoch(self.epoch.0 + 1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speed() -> Arg {
        Arg::new(Kind::Integer)
            .unit(Unit::MillimeterPerMinute)
            .min(1.0)
            .max_abs(6000.0)
    }

    fn set() -> CapabilitySet {
        CapabilitySet::new()
            .with(
                "a.move",
                Verb::new()
                    .arg("speed", speed())
                    .arg("mode", Arg::new(Kind::String).allowed(&["x", "y"])),
            )
            .with("a.stop", Verb::new())
    }

    #[test]
    fn intersection_with_itself_is_itself() {
        assert_eq!(set().intersect(&set()), set());
    }

    #[test]
    fn intersection_drops_a_verb_absent_from_either_side() {
        let narrowed = set().intersect(&set().without("a.stop"));
        assert!(!narrowed.has("a.stop"));
        assert!(narrowed.has("a.move"));
    }

    #[test]
    fn intersection_never_adds() {
        let wide = set().with("a.extra", Verb::new());
        assert!(!set().intersect(&wide).has("a.extra"));
        assert!(!wide.intersect(&set()).has("a.extra"));
    }

    #[test]
    fn narrower_bounds_and_common_allowed_values_win() {
        let other = CapabilitySet::new().with(
            "a.move",
            Verb::new()
                .arg("speed", Arg::new(Kind::Integer).min(10.0).max_abs(3000.0))
                .arg("mode", Arg::new(Kind::String).allowed(&["y", "z"])),
        );
        let got = set().intersect(&other);
        let v = got.get("a.move").unwrap();
        assert_eq!(v.args["speed"].min, Some(10.0));
        assert_eq!(v.args["speed"].max_abs, Some(3000.0));
        assert_eq!(v.args["speed"].unit, Some(Unit::MillimeterPerMinute));
        assert_eq!(v.args["mode"].allowed, vec!["y".to_owned()]);
    }

    #[test]
    fn an_argument_that_cannot_meet_is_dropped() {
        for other in [
            Arg::new(Kind::Number),                         // other kind
            Arg::new(Kind::Integer).unit(Unit::Millimeter), // other unit
        ] {
            let got = set().intersect(
                &CapabilitySet::new().with(
                    "a.move",
                    Verb::new()
                        .arg("speed", other)
                        .arg("mode", Arg::new(Kind::String)),
                ),
            );
            assert!(!got.get("a.move").unwrap().args.contains_key("speed"));
        }
        let no_common = CapabilitySet::new().with(
            "a.move",
            Verb::new()
                .arg("speed", speed())
                .arg("mode", Arg::new(Kind::String).allowed(&["q"])),
        );
        assert!(
            !set()
                .intersect(&no_common)
                .get("a.move")
                .unwrap()
                .args
                .contains_key("mode")
        );
    }

    #[test]
    fn a_verb_that_loses_a_required_argument_is_dropped() {
        let a = CapabilitySet::new().with(
            "v",
            Verb::new().arg("n", Arg::new(Kind::Integer).required()),
        );
        let b = CapabilitySet::new().with("v", Verb::new());
        assert!(!a.intersect(&b).has("v"));
        assert!(!b.intersect(&a).has("v"));
    }

    #[test]
    fn required_on_either_side_stays_required_and_a_default_needs_both_sides() {
        let a = CapabilitySet::new().with(
            "v",
            Verb::new().arg("n", Arg::new(Kind::Integer).required().default_value("1")),
        );
        let b = CapabilitySet::new().with("v", Verb::new().arg("n", Arg::new(Kind::Integer)));
        let got = a.intersect(&b);
        let n = &got.get("v").unwrap().args["n"];
        assert!(n.required);
        assert_eq!(n.default, None);
        assert_eq!(
            a.intersect(&a).get("v").unwrap().args["n"]
                .default
                .as_deref(),
            Some("1")
        );
    }

    #[test]
    fn allowed_order_and_repetition_do_not_change_equality() {
        let a = Arg::new(Kind::String).allowed(&["b", "a", "a"]);
        let b = Arg::new(Kind::String).allowed(&["a", "b"]);
        assert_eq!(a, b);
    }

    #[test]
    fn resolve_is_baseline_profile_probe_intersected() {
        let profile = set().without("a.stop");
        let probe = set();
        assert_eq!(resolve(&set(), &profile, &probe), profile);
    }

    #[test]
    fn is_within_tells_narrower_from_wider() {
        assert!(set().without("a.stop").is_within(&set()));
        assert!(!set().with("a.extra", Verb::new()).is_within(&set()));
        let looser =
            CapabilitySet::new().with("a.move", Verb::new().arg("speed", Arg::new(Kind::Integer)));
        assert!(
            !looser
                .is_within(&CapabilitySet::new().with("a.move", Verb::new().arg("speed", speed())))
        );
    }

    #[test]
    fn epoch_moves_only_when_the_set_changes() {
        let mut e = EnabledSet::new(set());
        let first = e.epoch();
        assert!(!e.update(set()));
        assert_eq!(e.epoch(), first);
        assert!(e.update(set().without("a.stop")));
        assert!(e.epoch() > first);
        assert!(!e.set().has("a.stop"));
    }

    #[test]
    fn a_set_round_trips_as_json() {
        let json = serde_json::to_string(&set()).unwrap();
        assert_eq!(serde_json::from_str::<CapabilitySet>(&json).unwrap(), set());
    }
}
