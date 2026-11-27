use super::*;
use crate::ports::prusalink::python;
use serde_json::{Value, json};

/// A pairing: equal only to itself, as `RegexPairing`
struct P {
    ident: u64,
    priority: f64,
}

impl PartialEq for P {
    fn eq(&self, other: &P) -> bool {
        self.ident == other.ident
    }
}

impl Debug for P {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "P({})", self.ident)
    }
}

fn ops() -> Vec<(&'static str, u64, f64)> {
    // A fixed pseudo-random walk: few priorities, so many equal keys
    let mut seed: u64 = 12345;
    let mut next = |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    let priorities = [0.0, 1.0, -1.0, 2.5, 1.0];
    let mut ops = vec![];
    let mut made: Vec<(u64, f64)> = vec![];
    for step in 0..600 {
        let ident = step as u64;
        match next(10) {
            0..=4 => {
                let priority = priorities[next(5) as usize];
                made.push((ident, priority));
                ops.push(("add", ident, priority));
            }
            5..=7 if !made.is_empty() => {
                let (ident, priority) = made[next(made.len() as u64) as usize];
                ops.push(("remove", ident, priority));
            }
            8 if !made.is_empty() => {
                let (ident, priority) = made[next(made.len() as u64) as usize];
                ops.push(("contains", ident, priority));
            }
            _ => {
                // one never added
                ops.push(("remove", 100_000 + ident, priorities[next(5) as usize]));
            }
        }
    }
    ops
}

/// sortedcontainers' own `SortedKeyList` and ours, through the same 600 operations with a load
/// factor of 4: after each, the same result (`ValueError` and its text included), the same order
/// and the same sublists.
#[test]
fn identical_to_sortedcontainers() {
    let ops = ops();
    let load = 4;
    let Some(theirs) = python::run(
        include_str!("../driver_sortedcontainers.py"),
        &json!({"load": load, "ops": ops}),
    ) else {
        return;
    };
    let theirs = theirs.as_array().unwrap();
    let mut skl: SortedKeyList<P, f64> = SortedKeyList::new(|item| -item.priority);
    skl._reset(load);
    let mut made: std::collections::HashMap<u64, f64> = Default::default();
    for ((op, ident, priority), their) in ops.iter().zip(theirs) {
        let priority = *made.entry(*ident).or_insert(*priority);
        let item = P {
            ident: *ident,
            priority,
        };
        let result = match *op {
            "add" => {
                skl.add(item);
                Value::Null
            }
            "contains" => json!(skl.contains(&item)),
            _ => match skl.remove(&item) {
                Ok(()) => Value::Null,
                Err(NotInList(text)) => json!(["ValueError", text]),
            },
        };
        let order: Vec<u64> = skl.iter().map(|p| p.ident).collect();
        let sublists: Vec<Vec<u64>> = skl
            .sublists()
            .iter()
            .map(|sub| sub.iter().map(|p| p.ident).collect())
            .collect();
        assert_eq!(json!([result, order, sublists]), *their, "{op} {ident}");
    }
    assert!(skl.sublists().len() > 2, "the walk splits sublists");
}
