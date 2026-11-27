//! What PrusaLink takes from pydantic 1.10.13 (`pydantic/utils.py`, MIT, Copyright (c) 2017-2021
//! Samuel Colvin and other contributors) beside its models (those are in [`super::model_classes`]):
//! `deep_update` (`utils.py:213-221`), used by `telemetry_passer.py:170`. A ported second source,
//! as the reference uses it (owner, 2026-11-16; `docs/archive/plans/mk3-port-plan.md` §5).

use serde_json::{Map, Value};

/// `deep_update(mapping, *updating_mappings)` (`utils.py:213-221`)
pub fn deep_update(
    mapping: &Map<String, Value>,
    updating_mappings: &[&Map<String, Value>],
) -> Map<String, Value> {
    let mut updated_mapping = mapping.clone();
    for updating_mapping in updating_mappings {
        for (k, v) in *updating_mapping {
            match (updated_mapping.get(k), v) {
                (Some(Value::Object(old)), Value::Object(new)) => {
                    let merged = deep_update(old, &[new]);
                    updated_mapping.insert(k.clone(), Value::Object(merged));
                }
                _ => {
                    updated_mapping.insert(k.clone(), v.clone());
                }
            }
        }
    }
    updated_mapping
}

#[cfg(test)]
mod tests;
