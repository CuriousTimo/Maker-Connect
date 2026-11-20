//! Ours against what CPython 3.13.12 gave on the same inputs (`tests/py-cases.json`,
//! recorded by `scripts/gcode-metadata-py-cases.py`).

use maker_connect_gcode_metadata::py::{self, PyErr, PyValue};
use serde_json::Value;

fn cases() -> Value {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/py-cases.json"))
        .expect("py-cases.json");
    serde_json::from_str(&text).expect("py-cases.json is JSON")
}

/// `float.fromhex` of what `float.hex()` writes.
fn from_hex(text: &str) -> f64 {
    match text {
        "inf" => return f64::INFINITY,
        "-inf" => return f64::NEG_INFINITY,
        "nan" => return f64::NAN,
        _ => {}
    }
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let rest = rest.strip_prefix("0x").expect("hex float");
    let (mantissa, exp) = rest.split_once('p').expect("hex exponent");
    let (lead, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut bits: u64 = u64::from_str_radix(lead, 16).unwrap();
    for c in frac.chars() {
        bits = (bits << 4) | c.to_digit(16).unwrap() as u64;
    }
    let exp: i32 = exp.parse().unwrap();
    let scale = exp - 4 * frac.len() as i32;
    // exact: bits has at most 53 significant bits, and powers of two scale exactly in two steps
    let value = (bits as f64) * 2f64.powi(scale / 2) * 2f64.powi(scale - scale / 2);
    if negative { -value } else { value }
}

/// Python's JSON decode errors are `JSONDecodeError`, a `ValueError`.
fn expected(outcome: &Value) -> (String, String) {
    let kind = outcome[0].as_str().unwrap();
    let kind = if kind == "JSONDecodeError" {
        "ValueError"
    } else {
        kind
    };
    (kind.to_owned(), outcome[1].as_str().unwrap().to_owned())
}

fn ours<T>(result: Result<T, PyErr>, ok: impl Fn(T) -> String) -> (String, String) {
    match result {
        Ok(value) => ("ok".to_owned(), ok(value)),
        Err(err) => (err.class_name().to_owned(), err.message().to_owned()),
    }
}

fn check(
    name: &str,
    input: &str,
    got: (String, String),
    want: (String, String),
    bad: &mut Vec<String>,
) {
    if got != want {
        bad.push(format!("{name} {input:?}: ours {got:?}, CPython {want:?}"));
    }
}

fn report(bad: Vec<String>, total: usize) {
    let shown: Vec<&String> = bad.iter().take(25).collect();
    assert!(
        bad.is_empty(),
        "{} of {total} differ:\n{shown:#?}",
        bad.len()
    );
}

#[test]
fn recorded_on_cpython_3_13_12() {
    assert_eq!(cases()["python"], "3.13.12");
    assert_eq!(py::py_unicode_python_version(), "3.13.12");
}

#[test]
fn int_of_str() {
    let cases = cases();
    let list = cases["int"].as_array().unwrap();
    let mut bad = Vec::new();
    for case in list {
        let input = case[0].as_str().unwrap();
        let got = ours(py::int_from_str(input), |i| i.as_str().to_owned());
        check("int", input, got, expected(&case[1]), &mut bad);
    }
    report(bad, list.len());
}

#[test]
fn float_of_str() {
    let cases = cases();
    let list = cases["float"].as_array().unwrap();
    let mut bad = Vec::new();
    for case in list {
        let input = case[0].as_str().unwrap();
        let got = ours(py::float_from_str(input), py::float_repr);
        check("float", input, got, expected(&case[1]), &mut bad);
    }
    report(bad, list.len());
}

#[test]
fn float_repr_round_and_int() {
    let cases = cases();
    let mut bad = Vec::new();
    let mut total = 0;
    for case in cases["float_repr"].as_array().unwrap() {
        let x = from_hex(case[0].as_str().unwrap());
        let want = ("ok".to_owned(), case[1].as_str().unwrap().to_owned());
        check(
            "repr",
            case[0].as_str().unwrap(),
            ("ok".into(), py::float_repr(x)),
            want,
            &mut bad,
        );
        total += 1;
    }
    for case in cases["round2"].as_array().unwrap() {
        let x = from_hex(case[0].as_str().unwrap());
        let want = ("ok".to_owned(), case[1].as_str().unwrap().to_owned());
        let got = ("ok".to_owned(), py::float_repr(py::round_f64(x, 2)));
        check("round", case[0].as_str().unwrap(), got, want, &mut bad);
        total += 1;
    }
    for case in cases["int_of_float"].as_array().unwrap() {
        let x = from_hex(case[0].as_str().unwrap());
        let got = ours(py::int_from_f64(x), |i| i.as_str().to_owned());
        check(
            "int(float)",
            case[0].as_str().unwrap(),
            got,
            expected(&case[1]),
            &mut bad,
        );
        total += 1;
    }
    report(bad, total);
}

#[test]
fn repr_of_str() {
    let cases = cases();
    let list = cases["repr_str"].as_array().unwrap();
    let mut bad = Vec::new();
    for case in list {
        let input = case[0].as_str().unwrap();
        let want = ("ok".to_owned(), case[1].as_str().unwrap().to_owned());
        check(
            "repr",
            input,
            ("ok".into(), py::repr_str(input)),
            want,
            &mut bad,
        );
    }
    report(bad, list.len());
}

#[test]
fn json_loads_then_dumps() {
    let cases = cases();
    let list = cases["json_loads"].as_array().unwrap();
    let mut bad = Vec::new();
    let mut lone_surrogates = Vec::new();
    for case in list {
        let input = case[0].as_str().unwrap();
        let got = ours(py::json_loads(input), |v| py::json_dumps(&v, None));
        // not covered (py.rs): a lone surrogate, which CPython keeps and a `String` cannot
        if got.0 == "ValueError" && got.1.contains("not covered by the port") {
            lone_surrogates.push(input.to_owned());
            continue;
        }
        check("json", input, got, expected(&case[1]), &mut bad);
    }
    assert_eq!(
        lone_surrogates,
        [r#""\ud83d""#, r#""\ude00""#, r#""\ud83d\u0041""#],
        "only the recorded lone surrogates are refused"
    );
    report(bad, list.len());
}

#[test]
fn json_dump_with_indent_2() {
    let cases = cases();
    let list = cases["json_dump_indent2"].as_array().unwrap();
    let mut bad = Vec::new();
    for case in list {
        let input = case[0].as_str().unwrap();
        let value = py::json_loads(input).expect("recorded value loads");
        let want = ("ok".to_owned(), case[1].as_str().unwrap().to_owned());
        check(
            "dump",
            input,
            ("ok".into(), py::json_dumps(&value, Some(2))),
            want,
            &mut bad,
        );
    }
    report(bad, list.len());
}

#[test]
fn sum_of_numbers() {
    let cases = cases();
    let list = cases["sum"].as_array().unwrap();
    let mut bad = Vec::new();
    for case in list {
        let values: Vec<PyValue> = case[0]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| match v {
                Value::String(hex) => PyValue::Float(from_hex(hex)),
                Value::Bool(b) => PyValue::Bool(*b),
                Value::Number(n) => PyValue::int(n.as_i64().unwrap()),
                other => panic!("not a number: {other}"),
            })
            .collect();
        let got = ours(py::sum(&values), |v| v.repr());
        check(
            "sum",
            &case[0].to_string(),
            got,
            expected(&case[1]),
            &mut bad,
        );
    }
    report(bad, list.len());
}
