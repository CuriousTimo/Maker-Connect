use super::*;
use crate::ports::prusalink::python;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::OnceLock;

const NAMES: [&str; 10] = [
    "a", "b", "c", "d", "e", "f", "selfdis1", "selfdis2", "adder1", "adder2",
];

fn ops() -> Vec<Value> {
    // A fixed pseudo-random walk over the receivers not yet dropped
    let mut seed: u64 = 4242;
    let mut next = |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    let mut alive: Vec<&str> = NAMES.to_vec();
    let mut ops = vec![];
    for _ in 0..400 {
        let pick = |n: u64| alive[n as usize];
        match next(20) {
            _ if alive.is_empty() => ops.push(json!(["send", ops.len()])),
            0..=6 => ops.push(json!([
                "connect",
                pick(next(alive.len() as u64)),
                next(2) == 0
            ])),
            7..=9 => ops.push(json!(["disconnect", pick(next(alive.len() as u64))])),
            10 => {
                let name = alive.remove(next(alive.len() as u64) as usize);
                ops.push(json!(["drop", name]));
            }
            11..=17 => ops.push(json!(["send", ops.len()])),
            _ => ops.push(json!(["len"])),
        }
    }
    ops
}

type Calls = Arc<Mutex<Vec<(String, i64)>>>;

fn make(
    name: &str,
    signal: &Arc<Signal<i64>>,
    calls: &Calls,
    late: &Arc<Receiver<i64>>,
) -> Arc<Receiver<i64>> {
    let me: Arc<OnceLock<Weak<Receiver<i64>>>> = Arc::new(OnceLock::new());
    let (name, signal, calls, late, me2) = (
        name.to_owned(),
        Arc::downgrade(signal),
        calls.clone(),
        late.clone(),
        me.clone(),
    );
    let receiver: Arc<Receiver<i64>> = Arc::new(move |sender: &i64| {
        calls.lock().unwrap().push((name.clone(), *sender));
        let signal = signal.upgrade().unwrap();
        if name.starts_with("selfdis") {
            signal.disconnect(&me2.get().unwrap().upgrade().unwrap());
        }
        if name.starts_with("adder") {
            signal.connect(&late, false);
        }
    });
    me.set(Arc::downgrade(&receiver)).ok().unwrap();
    receiver
}

/// blinker's own `Signal` and ours, through the same 400 operations (weak and strong receivers,
/// receivers collected, disconnecting itself and connecting another while sending): after each,
/// the same receivers called with the same sender (sorted: the source's order is undefined), and
/// the same `len(receivers)` and truth.
#[test]
fn identical_to_blinker() {
    let ops = ops();
    let Some(theirs) = python::run(
        include_str!("../driver_blinker.py"),
        &json!({"names": NAMES, "ops": ops}),
    ) else {
        return;
    };
    let theirs = theirs.as_array().unwrap();
    assert_eq!(theirs.len(), ops.len());
    let signal: Arc<Signal<i64>> = Arc::new(Signal::new());
    let calls: Calls = Default::default();
    let noop: Arc<Receiver<i64>> = Arc::new(|_: &i64| {});
    let late = make("late", &signal, &calls, &noop);
    let mut held: HashMap<&str, Arc<Receiver<i64>>> = NAMES
        .iter()
        .map(|name| (*name, make(name, &signal, &calls, &late)))
        .collect();
    let mut sent = 0;
    for (op, their) in ops.iter().zip(theirs) {
        let name = op[1].as_str().unwrap_or_default();
        let ours = match op[0].as_str().unwrap() {
            "connect" => {
                signal.connect(&held[name], op[2].as_bool().unwrap());
                Value::Null
            }
            "disconnect" => {
                signal.disconnect(&held[name]);
                Value::Null
            }
            "drop" => {
                held.remove(name);
                Value::Null
            }
            "send" => {
                calls.lock().unwrap().clear();
                signal.send(&op[1].as_i64().unwrap());
                sent += 1;
                let mut got: Vec<(String, i64)> = calls.lock().unwrap().clone();
                got.sort();
                json!(
                    got.into_iter()
                        .map(|(n, s)| json!([n, s]))
                        .collect::<Vec<_>>()
                )
            }
            "len" => json!([signal.receivers_len(), signal.has_receivers()]),
            other => panic!("{other}"),
        };
        assert_eq!(
            serde_json::to_string(&ours).unwrap(),
            serde_json::to_string(their).unwrap(),
            "{op}"
        );
    }
    assert!(sent > 100);
}
