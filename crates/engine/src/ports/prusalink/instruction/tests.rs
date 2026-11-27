use super::*;
use crate::ports::prusalink::python;
use serde_json::{Value, json};

fn err(error: PyErr) -> Value {
    json!({"error": [error.class_name(), error.message()]})
}

fn run(
    kind: &str,
    message: &str,
    to_checksum: bool,
    number: i64,
    captures: &[&str],
    force: bool,
) -> Value {
    let made = match kind {
        "plain" => Instruction::new(message, to_checksum, None, None),
        "matchable" => Instruction::matchable(message, to_checksum, any_line(), false),
        _ => Instruction::matchable(message, to_checksum, any_line(), true),
    };
    let instruction = match made {
        Ok(i) => i,
        Err(error) => return json!([err(error)]),
    };
    let mut out = vec![];
    out.push(json!(instruction.str()));
    out.push(json!(instruction.str()));
    out.push(json!([instruction.is_sent(), instruction.is_confirmed()]));
    out.push(match instruction.fill_data(number) {
        Ok(()) => json!([instruction.data().unwrap(), instruction.number()]),
        Err(error) => err(error),
    });
    let regex = Regex::new(r"(?P<word>\w+)(.*)").unwrap();
    for text in captures {
        let captures = regex.captures(text).unwrap().unwrap();
        instruction.output_captured(Arc::new(Match::new(&regex, &captures)));
    }
    if kind != "plain" {
        out.push(json!(
            (0..3)
                .map(|i| instruction
                    .r#match(i)
                    .map(|m| m.group(0).unwrap().to_owned()))
                .collect::<Vec<_>>()
        ));
        out.push(json!(
            instruction
                .get_matches()
                .iter()
                .map(|m| m.named("word").unwrap().to_owned())
                .collect::<Vec<_>>()
        ));
        out.push(json!(
            instruction.capture_matching.as_ref().unwrap().as_str()
        ));
    }
    out.push(json!(instruction.wait_for_send(Some(0.0))));
    instruction.sent();
    out.push(json!([
        instruction.is_sent(),
        instruction.wait_for_send(Some(0.0))
    ]));
    let confirmed = instruction.confirm(force);
    out.push(json!([
        confirmed,
        instruction.is_confirmed(),
        instruction.wait_for_confirmation(Some(0.0)),
        instruction.time_to_confirm().is_some()
    ]));
    instruction.reset();
    out.push(json!([instruction.is_sent(), instruction.sent_at()]));
    json!(out)
}

/// `instruction.py`'s three classes, PrusaLink's own and ours, on the same messages: `str`,
/// `fill_data`'s bytes (numbered with a checksum or not, `UnicodeEncodeError`), captures, `match`,
/// `get_matches`, sending, confirming (refused without a capture, forced), `reset`; and
/// `get_checksum` on every byte.
#[test]
fn identical_to_prusalink() {
    let mut cases = vec![];
    let messages = [
        "G28",
        "  M105  ",
        "M117 Hi there",
        "",
        "M117 Grüße",
        "M117 日本",
        "M117 a😀",
        "M117 éü x é",
        "G1 X1\nG1 X2",
        "\tM115\u{a0}",
    ];
    for kind in ["plain", "matchable", "mandatory"] {
        for (i, message) in messages.iter().enumerate() {
            for to_checksum in [false, true] {
                for (captures, force) in [
                    (vec![], false),
                    (vec![], true),
                    (vec!["ok T:20", "echo x"], false),
                ] {
                    cases.push(json!([
                        kind,
                        message,
                        to_checksum,
                        (i * 37) as i64,
                        captures,
                        force
                    ]));
                }
            }
        }
    }
    let checksums: Vec<Vec<u8>> = vec![
        vec![],
        (0..=255).collect(),
        b"N1 G28 ".to_vec(),
        vec![255, 255, 1],
    ];
    let Some(theirs) = python::run(
        include_str!("../driver_instruction.py"),
        &json!({"cases": cases, "checksums": checksums}),
    ) else {
        return;
    };
    let results = theirs["results"].as_array().unwrap();
    assert_eq!(results.len(), cases.len());
    for (case, their) in cases.iter().zip(results) {
        let captures: Vec<&str> = case[4]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        let ours = run(
            case[0].as_str().unwrap(),
            case[1].as_str().unwrap(),
            case[2].as_bool().unwrap(),
            case[3].as_i64().unwrap(),
            &captures,
            case[5].as_bool().unwrap(),
        );
        assert_eq!(
            serde_json::to_string(&ours).unwrap(),
            serde_json::to_string(their).unwrap(),
            "{case}"
        );
    }
    let ours: Vec<u8> = checksums
        .iter()
        .map(|d| Instruction::get_checksum(d))
        .collect();
    assert_eq!(json!(ours), theirs["checksums"]);
}
