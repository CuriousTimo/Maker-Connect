//! A device kind's declared state set, and a device's state tree changed by patches.
//!
//! State sets are data, one per device kind, in [`crate::registry::STATE_SETS`]. Meaning after
//! `sdk/connectorapi/devicestate.go` (read only, OCL): names in upper case, each state lists the
//! verbs it accepts, transitions name the verb that causes them or none when the device moves on
//! its own. Unlike there, our host refuses a verb the current state does not accept
//! (hub-core-v2.md §2.1, "state guard from the state set").

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One declared state.
#[derive(Debug, PartialEq, Eq)]
pub struct StateDecl {
    /// `A-Z`, digits, `_`; starts with a letter, ends with a letter or digit, 2 to 63 bytes.
    pub name: &'static str,
    /// Every verb a device in this state accepts.
    pub accepts: &'static [&'static str],
    pub doc: &'static str,
}

/// One move between two declared states.
#[derive(Debug, PartialEq, Eq)]
pub struct Transition {
    pub from: &'static str,
    pub to: &'static str,
    /// The verb that causes it; `None` when the device moves on its own (a job ends).
    pub verb: Option<&'static str>,
}

/// The state model of one device kind.
#[derive(Debug, PartialEq, Eq)]
pub struct StateSet {
    pub kind: &'static str,
    pub states: &'static [StateDecl],
    pub transitions: &'static [Transition],
}

impl StateSet {
    pub fn state(&self, name: &str) -> Option<&StateDecl> {
        self.states.iter().find(|s| s.name == name)
    }

    /// Whether a device in `state` accepts `verb`. An undeclared state accepts nothing.
    pub fn accepts(&self, state: &str, verb: &str) -> bool {
        self.state(state).is_some_and(|s| s.accepts.contains(&verb))
    }

    /// What is wrong with the declaration; empty when nothing is.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.states.is_empty() {
            out.push(format!("{}: declares no state", self.kind));
        }
        for (i, s) in self.states.iter().enumerate() {
            if !valid_state_name(s.name) {
                out.push(format!("{}: invalid state name {:?}", self.kind, s.name));
            }
            if self.states[..i].iter().any(|t| t.name == s.name) {
                out.push(format!("{}: state {} declared twice", self.kind, s.name));
            }
            for (j, v) in s.accepts.iter().enumerate() {
                if s.accepts[..j].contains(v) {
                    out.push(format!("{}: state {} accepts {v} twice", self.kind, s.name));
                }
            }
        }
        for (i, t) in self.transitions.iter().enumerate() {
            for end in [t.from, t.to] {
                if self.state(end).is_none() {
                    out.push(format!(
                        "{}: transition names undeclared state {end}",
                        self.kind
                    ));
                }
            }
            if t.from == t.to {
                out.push(format!(
                    "{}: transition {} -> {} leaves and enters one state",
                    self.kind, t.from, t.to
                ));
            }
            if self.transitions[..i].contains(t) {
                out.push(format!(
                    "{}: transition {} -> {} declared twice",
                    self.kind, t.from, t.to
                ));
            }
            if let Some(v) = t.verb
                && self.state(t.from).is_some()
                && !self.accepts(t.from, v)
            {
                out.push(format!(
                    "{}: {v} moves {} -> {} but {} does not accept it",
                    self.kind, t.from, t.to, t.from
                ));
            }
        }
        out
    }
}

fn valid_state_name(name: &str) -> bool {
    let b = name.as_bytes();
    (2..=63).contains(&b.len())
        && b[0].is_ascii_uppercase()
        && (b[b.len() - 1].is_ascii_uppercase() || b[b.len() - 1].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
}

/// One change to a device's state tree: a JSON pointer (RFC 6901, `/a/b`, `~1` for `/`, `~0` for
/// `~`) and the node's new value. `null` removes the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patch {
    pub path: String,
    pub value: Value,
}

impl Patch {
    pub fn set(path: &str, value: Value) -> Self {
        Self {
            path: path.to_owned(),
            value,
        }
    }
    pub fn remove(path: &str) -> Self {
        Self {
            path: path.to_owned(),
            value: Value::Null,
        }
    }
}

/// Why a patch was refused. The tree is unchanged then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    /// Not a JSON pointer to a node below the root (empty, no leading `/`, a bad `~` escape).
    BadPath(String),
    /// A node on the way is not an object. Patches address object members only.
    NotAnObject(String),
}

/// A device's state tree: a JSON object, changed only by [`Patch`]es.
#[derive(Debug, Clone, PartialEq)]
pub struct StateTree {
    root: Map<String, Value>,
}

impl Default for StateTree {
    fn default() -> Self {
        Self::new()
    }
}

impl StateTree {
    pub fn new() -> Self {
        Self { root: Map::new() }
    }

    pub fn as_value(&self) -> Value {
        Value::Object(self.root.clone())
    }

    /// The node at a JSON pointer, if there is one.
    pub fn get(&self, path: &str) -> Option<&Value> {
        let segments = parse_pointer(path).ok()?;
        let (last, parents) = segments.split_last()?;
        let mut node = &self.root;
        for s in parents {
            node = node.get(s)?.as_object()?;
        }
        node.get(last)
    }

    /// Applies one patch. Setting makes missing objects on the way; removing a node that is not
    /// there changes nothing.
    pub fn apply(&mut self, patch: &Patch) -> Result<(), PatchError> {
        let segments = parse_pointer(&patch.path)?;
        let (last, parents) = segments
            .split_last()
            .ok_or_else(|| PatchError::BadPath(patch.path.clone()))?;
        let mut node = &mut self.root;
        for s in parents {
            if patch.value.is_null() && !node.contains_key(s) {
                return Ok(());
            }
            let next = node
                .entry(s.clone())
                .or_insert_with(|| Value::Object(Map::new()));
            node = next
                .as_object_mut()
                .ok_or_else(|| PatchError::NotAnObject(patch.path.clone()))?;
        }
        if patch.value.is_null() {
            node.remove(last);
        } else {
            node.insert(last.clone(), patch.value.clone());
        }
        Ok(())
    }
}

fn parse_pointer(path: &str) -> Result<Vec<String>, PatchError> {
    let bad = || PatchError::BadPath(path.to_owned());
    let rest = path.strip_prefix('/').ok_or_else(bad)?;
    rest.split('/')
        .map(|s| {
            let mut out = String::new();
            let mut chars = s.chars();
            while let Some(c) = chars.next() {
                if c == '~' {
                    match chars.next() {
                        Some('0') => out.push('~'),
                        Some('1') => out.push('/'),
                        _ => return Err(bad()),
                    }
                } else {
                    out.push(c);
                }
            }
            Ok(out)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    static GOOD: StateSet = StateSet {
        kind: "t",
        states: &[
            StateDecl {
                name: "IDLE",
                accepts: &["t.start"],
                doc: "",
            },
            StateDecl {
                name: "BUSY",
                accepts: &["t.stop"],
                doc: "",
            },
        ],
        transitions: &[
            Transition {
                from: "IDLE",
                to: "BUSY",
                verb: Some("t.start"),
            },
            Transition {
                from: "BUSY",
                to: "IDLE",
                verb: None,
            },
        ],
    };

    #[test]
    fn accepts_reads_the_declared_list() {
        assert!(GOOD.accepts("IDLE", "t.start"));
        assert!(!GOOD.accepts("BUSY", "t.start"));
        assert!(!GOOD.accepts("GONE", "t.start"));
        assert!(GOOD.problems().is_empty(), "{:?}", GOOD.problems());
    }

    #[test]
    fn problems_are_found() {
        static BAD: StateSet = StateSet {
            kind: "b",
            states: &[
                StateDecl {
                    name: "idle",
                    accepts: &["x", "x"],
                    doc: "",
                },
                StateDecl {
                    name: "A_",
                    accepts: &[],
                    doc: "",
                },
                StateDecl {
                    name: "A_",
                    accepts: &[],
                    doc: "",
                },
            ],
            transitions: &[
                Transition {
                    from: "A_",
                    to: "A_",
                    verb: None,
                },
                Transition {
                    from: "idle",
                    to: "NOPE",
                    verb: Some("y"),
                },
                Transition {
                    from: "A_",
                    to: "A_",
                    verb: None,
                },
            ],
        };
        let p = BAD.problems().join("\n");
        for needle in [
            "invalid state name \"idle\"",
            "invalid state name \"A_\"",
            "declared twice",
            "accepts x twice",
            "undeclared state NOPE",
            "leaves and enters one state",
            "y moves idle -> NOPE but idle does not accept it",
            "transition A_ -> A_ declared twice",
        ] {
            assert!(p.contains(needle), "missing {needle:?} in\n{p}");
        }
        static EMPTY: StateSet = StateSet {
            kind: "e",
            states: &[],
            transitions: &[],
        };
        assert_eq!(EMPTY.problems(), vec!["e: declares no state".to_owned()]);
    }

    #[test]
    fn a_patch_sets_a_node_and_makes_objects_on_the_way() {
        let mut t = StateTree::new();
        t.apply(&Patch::set("/a/b", json!(1))).unwrap();
        t.apply(&Patch::set("/c", json!([1, 2]))).unwrap();
        assert_eq!(t.as_value(), json!({"a":{"b":1},"c":[1,2]}));
        assert_eq!(t.get("/a/b"), Some(&json!(1)));
    }

    #[test]
    fn null_removes_and_removing_nothing_changes_nothing() {
        let mut t = StateTree::new();
        t.apply(&Patch::set("/a/b", json!(1))).unwrap();
        t.apply(&Patch::remove("/a/b")).unwrap();
        assert_eq!(t.as_value(), json!({"a":{}}));
        t.apply(&Patch::remove("/x/y")).unwrap();
        assert_eq!(t.as_value(), json!({"a":{}}));
    }

    #[test]
    fn pointer_escapes_are_read() {
        let mut t = StateTree::new();
        t.apply(&Patch::set("/a~1b/c~0d", json!("v"))).unwrap();
        assert_eq!(t.as_value(), json!({"a/b":{"c~d":"v"}}));
    }

    #[test]
    fn bad_paths_and_non_objects_are_refused_and_leave_the_tree_alone() {
        let mut t = StateTree::new();
        t.apply(&Patch::set("/a", json!(5))).unwrap();
        for p in ["", "a", "/x~2"] {
            assert_eq!(
                t.apply(&Patch::set(p, json!(1))),
                Err(PatchError::BadPath(p.into()))
            );
        }
        assert_eq!(
            t.apply(&Patch::set("/a/b", json!(1))),
            Err(PatchError::NotAnObject("/a/b".into()))
        );
        assert_eq!(t.as_value(), json!({"a":5}));
    }

    #[test]
    fn a_patch_round_trips_as_json() {
        let p = Patch::remove("/a");
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json, json!({"path":"/a","value":null}));
        assert_eq!(serde_json::from_value::<Patch>(json).unwrap(), p);
    }
}
