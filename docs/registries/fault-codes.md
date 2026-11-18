# Fault codes

<!-- Generated from crates/device/src/registry.rs. Do not edit: run `MC_WRITE_REGISTRIES=1 cargo test -p maker-connect-device --test registry_docs`. -->

| Code | HTTP | Text |
|---|---|---|
| `device/CAPABILITY_NOT_SUPPORTED` | 409 | The device does not have that capability in its current set. |
| `validation/ARGUMENT_UNKNOWN` | 400 | The verb does not take that argument. |
| `validation/ARGUMENT_MISSING` | 400 | A required argument was not sent. |
| `validation/ARGUMENT_NOT_SUPPORTED` | 400 | The value is not one of the values the argument allows. |
| `validation/ARGUMENT_MALFORMED` | 400 | The value cannot be read as the argument's kind. |
| `validation/ARGUMENT_OUT_OF_RANGE` | 400 | The value is outside the bounds the argument declares. |
| `device/STATE_NOT_ACCEPTED` | 409 | The device's current state does not accept that verb. |
| `authz/PERSON_ONLY` | 403 | Only a person at their own session may send that verb; never the queue or a token. |
