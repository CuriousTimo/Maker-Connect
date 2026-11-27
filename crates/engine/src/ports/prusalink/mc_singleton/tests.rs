use super::*;
use crate::ports::prusalink::python;
use serde_json::{Value, json};

/// PrusaLink's `MCSingleton` and ours: `get_instance()` before, the first instance, after, a
/// second (`RuntimeError`), after: the same.
#[test]
fn identical_to_prusalink() {
    let Some(theirs) = python::run(include_str!("../driver_mc_singleton.py"), &json!({})) else {
        return;
    };
    let cell: MCSingleton<i64> = MCSingleton::default();
    let instance = || json!(cell.get_instance().map(|v| *v));
    let attempt = |value: i64| match cell.call(|| value) {
        Ok(v) => json!({ "ok": *v }),
        Err(error) => json!({"error": [error.class_name(), error.message()]}),
    };
    let ours: Value = json!([instance(), attempt(1), instance(), attempt(2), instance()]);
    assert_eq!(
        serde_json::to_string(&ours).unwrap(),
        serde_json::to_string(&theirs).unwrap()
    );
}
