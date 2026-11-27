use super::*;
use crate::ports::prusalink::python;
use serde_json::json;

fn pairs(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Array(map.iter().map(|(k, v)| json!([k, pairs(v)])).collect()),
        other => other.clone(),
    }
}

/// pydantic's own `deep_update` and ours on the same mappings: the same result, keys in the same
/// order (a key updated keeps its place, a new one goes last).
#[test]
fn deep_update_identical_to_pydantic() {
    let cases = json!([
        [{}, []],
        [{}, [{}]],
        [{"a": 1}, [{"b": 2}]],
        [{"a": 1, "b": 2}, [{"a": 3}]],
        [{"a": {"x": 1, "y": 2}, "b": 2}, [{"a": {"y": 3, "z": 4}}]],
        [{"a": {"x": 1}}, [{"a": 5}]],
        [{"a": 5}, [{"a": {"x": 1}}]],
        [{"a": {"x": {"deep": 1, "keep": 2}}}, [{"a": {"x": {"deep": 3}}}, {"a": {"x": {"new": 4}}, "c": null}]],
        [{"temp_nozzle": 20.5, "slot": {"1": {"temp": 200}}}, [{"slot": {"1": {"temp": 210}, "2": {"temp": 0}}, "temp_nozzle": 21}]],
        [{"a": null}, [{"a": {"x": 1}}, {"a": null}]],
        [{"a": [1, 2]}, [{"a": [3]}]],
    ]);
    let Some(theirs) = python::run(
        include_str!("../driver_pydantic.py"),
        &json!({ "cases": cases }),
    ) else {
        return;
    };
    let cases = cases.as_array().unwrap();
    assert_eq!(theirs.as_array().unwrap().len(), cases.len());
    for (case, their) in cases.iter().zip(theirs.as_array().unwrap()) {
        let mapping = case[0].as_object().unwrap();
        let updating: Vec<&Map<String, Value>> = case[1]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_object().unwrap())
            .collect();
        let ours = pairs(&Value::Object(deep_update(mapping, &updating)));
        assert_eq!(&ours, their, "{case}");
    }
}
