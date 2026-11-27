//! PrusaLink's `tests/test_serial_helpers.py`, test by test.

use super::*;
use crate::ports::prusalink::instruction::Kind;
use std::sync::Mutex;

/// `Mock()` as a queue: the calls of `enqueue_one` and `enqueue_list`.
#[derive(Default)]
struct Queue {
    one: Mutex<Vec<(Arc<Instruction>, bool)>>,
    list: Mutex<Vec<(Vec<Arc<Instruction>>, bool)>>,
}

impl SerialQueue for Queue {
    fn enqueue_one(&self, instruction: Arc<Instruction>, to_front: bool) {
        self.one.lock().unwrap().push((instruction, to_front));
    }
    fn enqueue_list(&self, instruction_list: Vec<Arc<Instruction>>, to_front: bool) {
        self.list.lock().unwrap().push((instruction_list, to_front));
    }
}

/// `Mock()` as an instruction: `wait_for_confirmation` answers in turn (`side_effect`).
struct Waiting {
    answers: Mutex<Vec<bool>>,
    calls: Mutex<usize>,
}

impl Waiting {
    fn new(answers: &[bool]) -> Waiting {
        Waiting {
            answers: Mutex::new(answers.iter().rev().copied().collect()),
            calls: Mutex::new(0),
        }
    }
}

impl WaitForConfirmation for Waiting {
    fn wait_for_confirmation(&self, _timeout: Option<f64>) -> bool {
        *self.calls.lock().unwrap() += 1;
        // a Mock returns a Mock, which is true, once the side effects are used up
        self.answers.lock().unwrap().pop().unwrap_or(true)
    }
}

/// `test_enqueue_instruction`: A plain instruction is created and enqueued
#[test]
fn test_enqueue_instruction() {
    let queue = Queue::default();
    let instruction = enqueue_instruction(&queue, "G28", true, true).unwrap();
    assert_eq!(instruction.kind, Kind::Instruction);
    assert_eq!(instruction.message(), "G28");
    assert!(instruction.to_checksum);
    let one = queue.one.lock().unwrap();
    assert_eq!(one.len(), 1);
    assert!(Arc::ptr_eq(&one[0].0, &instruction) && one[0].1);
}

/// `test_enqueue_matchable_mandatory`: By default the instruction has to match
#[test]
fn test_enqueue_matchable_mandatory() {
    let queue = Queue::default();
    let regex = Regex::new("ok").unwrap();
    let instruction = enqueue_matchable(&queue, "M105", &regex, false, false, true).unwrap();
    assert_eq!(instruction.kind, Kind::MandatoryMatchable);
    assert_eq!(
        instruction.capture_matching.as_ref().unwrap().as_str(),
        regex.as_str()
    );
    let one = queue.one.lock().unwrap();
    assert_eq!(one.len(), 1);
    assert!(Arc::ptr_eq(&one[0].0, &instruction) && !one[0].1);
}

/// `test_enqueue_matchable_optional`: The match can be made optional
#[test]
fn test_enqueue_matchable_optional() {
    let queue = Queue::default();
    let regex = Regex::new("ok").unwrap();
    let instruction = enqueue_matchable(&queue, "M105", &regex, false, false, false).unwrap();
    assert_eq!(instruction.kind, Kind::Matchable);
}

/// `test_enqueue_list_from_str`: Every message becomes one matchable, they are enqueued together
#[test]
fn test_enqueue_list_from_str() {
    let queue = Queue::default();
    let regex = Regex::new("ok").unwrap();
    let instructions =
        enqueue_list_from_str(&queue, &["G28", "M105"], &regex, false, true).unwrap();
    let messages: Vec<String> = instructions.iter().map(|i| i.message()).collect();
    assert_eq!(messages, ["G28", "M105"]);
    assert!(instructions.iter().all(|i| {
        i.capture_matching.as_ref().unwrap().as_str() == regex.as_str() && i.to_checksum
    }));
    let list = queue.list.lock().unwrap();
    assert_eq!(list.len(), 1);
    assert!(!list[0].1);
    assert!(
        list[0]
            .0
            .iter()
            .zip(&instructions)
            .all(|(a, b)| Arc::ptr_eq(a, b))
    );
}

/// `test_wait_for_instruction_confirmed`: Returns True once the instruction is confirmed
#[test]
fn test_wait_for_instruction_confirmed() {
    let instruction = Waiting::new(&[false, true]);
    assert!(wait_for_instruction(
        &instruction,
        &|| true,
        None,
        Some(0.01)
    ));
    assert_eq!(*instruction.calls.lock().unwrap(), 2);
}

/// `test_wait_for_instruction_should_not_wait`: Returns False without waiting, when waiting is not
/// wanted
#[test]
fn test_wait_for_instruction_should_not_wait() {
    let instruction = Waiting::new(&[]);
    assert!(!wait_for_instruction(&instruction, &|| false, None, None));
    assert_eq!(*instruction.calls.lock().unwrap(), 0);
}

/// `test_wait_for_instruction_event`: The quit event stops the waiting
#[test]
fn test_wait_for_instruction_event() {
    let instruction = Waiting::new(&[]);
    let evt = Event::new();
    evt.set();
    assert!(!wait_for_instruction(
        &instruction,
        &|| true,
        Some(&evt),
        None
    ));
    assert_eq!(*instruction.calls.lock().unwrap(), 0);
}

fn describe(instruction: &Instruction) -> serde_json::Value {
    let class = match instruction.kind {
        Kind::Instruction => "Instruction",
        Kind::Matchable => "MatchableInstruction",
        Kind::MandatoryMatchable => "MandatoryMatchableInstruction",
    };
    serde_json::json!([
        class,
        instruction.str(),
        instruction.to_checksum,
        instruction.capture_matching.as_ref().map(|r| r.as_str()),
        instruction
            .capturing_regexps
            .iter()
            .map(|r| r.as_str())
            .collect::<Vec<_>>()
    ])
}

/// PrusaLink's own helpers and ours, the same calls (each kind, `to_front`, `to_checksum`,
/// `has_to_match`, a message with a newline first, in the middle and last of a list, an empty
/// list) and waits (confirmed at once or later, `should_wait` running out, the event set): the
/// instructions the queue was given and the call returned, as their class, str, `to_checksum`,
/// `capture_matching` and `capturing_regexps`, whether those are the same objects, and for a wait
/// its result and how often it asked the instruction.
#[test]
fn identical_to_prusalink() {
    use serde_json::{Value, json};
    let patterns = [r"^ok\b", r".*", r"(?P<a>T:)(\d+)"];
    let messages = ["G28", "M105", "", "M1\nM2", "ü G1 X1"];
    let mut calls = vec![];
    for (i, message) in messages.iter().enumerate() {
        for to_front in [false, true] {
            for to_checksum in [false, true] {
                calls.push(json!(["instruction", message, to_front, to_checksum]));
                for has_to_match in [false, true] {
                    let pattern = patterns[i % 3];
                    calls.push(json!([
                        "matchable",
                        message,
                        pattern,
                        to_front,
                        to_checksum,
                        has_to_match
                    ]));
                }
            }
        }
    }
    let lists: [&[&str]; 5] = [
        &[],
        &["G28", "M105"],
        &["M1\n", "G28"],
        &["G28", "M1\nM2", "M105"],
        &["G28", "M105", "\n"],
    ];
    for (i, list) in lists.iter().enumerate() {
        for to_front in [false, true] {
            calls.push(json!(["list", list, patterns[i % 3], to_front, i % 2 == 0]));
        }
    }
    let waits = json!([
        [[], null, false],
        [[false, false, true], null, false],
        [[false, false, false], 2, false],
        [[false], 0, false],
        [[], null, true],
        [[false, true], 1, false],
        [[false, true], 2, false],
    ]);
    let Some(theirs) = crate::ports::prusalink::python::run(
        include_str!("../driver_helpers.py"),
        &json!({"calls": calls, "waits": waits}),
    ) else {
        return;
    };
    let regex = |p: &Value| Regex::new(p.as_str().unwrap()).unwrap();
    let mut ours_calls = vec![];
    for call in &calls {
        let queue = Queue::default();
        let b = |v: &Value| v.as_bool().unwrap();
        let returned: Result<Vec<Arc<Instruction>>, PyErr> = match call[0].as_str().unwrap() {
            "instruction" => {
                enqueue_instruction(&queue, call[1].as_str().unwrap(), b(&call[2]), b(&call[3]))
                    .map(|i| vec![i])
            }
            "matchable" => enqueue_matchable(
                &queue,
                call[1].as_str().unwrap(),
                &regex(&call[2]),
                b(&call[3]),
                b(&call[4]),
                b(&call[5]),
            )
            .map(|i| vec![i]),
            _ => {
                let list: Vec<&str> = call[1]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|m| m.as_str().unwrap())
                    .collect();
                enqueue_list_from_str(&queue, &list, &regex(&call[2]), b(&call[3]), b(&call[4]))
            }
        };
        let mut queued = vec![];
        let mut given = vec![];
        for (instruction, to_front) in queue.one.lock().unwrap().iter() {
            queued.push(json!(["one", to_front, [describe(instruction)]]));
            given.push(instruction.clone());
        }
        for (list, to_front) in queue.list.lock().unwrap().iter() {
            queued.push(json!([
                "list",
                to_front,
                list.iter().map(|i| describe(i)).collect::<Vec<_>>()
            ]));
            given.extend(list.iter().cloned());
        }
        let (out, same) = match returned {
            Ok(objects) => {
                let same = given.len() == objects.len()
                    && given.iter().zip(&objects).all(|(a, b)| Arc::ptr_eq(a, b));
                let out = match call[0].as_str().unwrap() {
                    "list" => json!(objects.iter().map(|i| describe(i)).collect::<Vec<_>>()),
                    _ => describe(&objects[0]),
                };
                (out, json!(same))
            }
            Err(error) => (json!([error.class_name(), error.message()]), Value::Null),
        };
        ours_calls.push(json!([queued, out, same]));
    }
    let mut ours_waits = vec![];
    for wait in waits.as_array().unwrap() {
        let answers: Vec<bool> = wait[0]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_bool().unwrap())
            .collect();
        let instruction = Waiting::new(&answers);
        let left = Mutex::new(wait[1].as_i64());
        let should_wait = || match &mut *left.lock().unwrap() {
            None => true,
            Some(n) => {
                *n -= 1;
                *n >= 0
            }
        };
        let event = Event::new();
        if wait[2].as_bool().unwrap() {
            event.set();
        }
        let result = wait_for_instruction(&instruction, &should_wait, Some(&event), Some(0.0));
        ours_waits.push(json!([result, *instruction.calls.lock().unwrap()]));
    }
    for (ours, theirs) in ours_calls.iter().zip(theirs["calls"].as_array().unwrap()) {
        assert_eq!(
            serde_json::to_string(ours).unwrap(),
            serde_json::to_string(theirs).unwrap()
        );
    }
    assert_eq!(ours_calls.len(), calls.len());
    assert_eq!(json!(ours_waits), theirs["waits"]);
}
