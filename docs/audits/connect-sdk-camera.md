# The SDK's camera modules: our port against the reference

**The reference:** the Connect SDK's `prusa/connect/printer/camera.py`, `camera_driver.py`,
`camera_controller.py`, `camera_configurator.py`, the camera part of `const.py` and
`CameraRegister` of `models.py`, at `c0d7d233` (branch `websockets`; `docs/design/wire.md`;
"Freeware" in its `setup.py`, ported on the owner's decision of 2026-11-12), on CPython 3.13.12
with requests 2.32.5. The ini is read and written by CPython 3.13.12's
`configparser.ConfigParser(interpolation=None)`, as PrusaLink makes it.

**Ours:** `crates/connect-sdk/src/camera.rs`, `camera_driver.rs`, `camera_controller.rs`,
`camera_configurator.rs`, `configparser.rs` (CPython 3.13.12's `Lib/configparser.py`, PSF licence),
`consts.rs` (the camera part) and `models.rs` (`CameraRegister`); a snapshot goes through
`util::Session::request_data`.

| Part of the reference | Ours |
|---|---|
| `camera.py`: `Snapshot`, `Resolution`, `Camera` | `Snapshot`, `Resolution`, `Camera`, the same methods; a setting's value is `Value` (a Python value, a `TriggerScheme` or a `Resolution`) |
| `camera_driver.py`: `CameraDriver` | `CameraDriver`; a subclass is a `DriverClass` (class attributes and class methods) with a `DriverImpl` (the methods it overrides) |
| `camera_controller.py`: `CameraController` | `CameraController`, the same methods |
| `camera_configurator.py`: `CameraConfigurator` | `CameraConfigurator`, the same methods, its `RLock` as a re-entrant lock |
| `models.py`: `CameraRegister` | `CameraRegister`, `LoopObject::CameraRegister` |
| `const.py`: `CapabilityType`, `TriggerScheme`, the camera constants and errors | `consts.rs`, the same |
| `configparser.ConfigParser(interpolation=None)`: `read`, `sections`, `items`, `get`, `set`, `has_section`, `has_option`, `add_section`, `remove_section`, `remove_option`, `write` | `configparser::ConfigParser`, the same |

## Results

`crates/connect-sdk/tests/camera_reference.rs` is the checker. `tests/camera_scenario.py run` runs the
reference's steps, the test runs ours on the same steps, and each step is compared. Both sides:
- use the same `Fake` driver, whose camera depends on its `path` (every capability, the minimum, one
  that cannot connect, a slow one, one whose rotation fails), and log every driver call;
- send their snapshots to one HTTP server (`camera_scenario.py serve`, stdlib only), which logs
  method, path, headers and body.

It wants CPython 3.13.12 with the SDK at `c0d7d233` and requests 2.32.5 (`CAMERA_PYTHON`, else
`reference/.venv-connect-sdk-313`); without it, it says "skipped" and passes. CI's job `store` runs it
on Linux.

| # | What | Result |
|---|---|---|
| 1 | Two cameras, full and minimal: the driver calls at connecting, `get_settings`, the `/p/camera` payload, `output_resolution`, `string_from_settings`; a setting the camera lacks | the same |
| 2 | `set_settings` from `settings_from_json`, 28 values and a failing driver: good and bad rotations, exposures, focus, resolutions, trigger schemes, name, token. For each: the error, driver calls, settings after, disconnects, busy | the same |
| 3 | `settings_from_string`, 17 configs; `_get_initial_resolution`, 5 configs and no resolutions | the same |
| 4 | `make_hash`, `get_config_hash` (valid and not), `is_config_valid`, `get_required_settings`, `scan` (with a config that is not valid and one with a trigger scheme) | the same |
| 5 | `Snapshot.send`, complete and incomplete: what reaches the server (path and query, every header, body) | the same |
| 6 | The controller: register a camera and an unknown one, layer triggers (`EACH_LAYER`, `FIFTH_LAYER`), a busy camera on a manual trigger, the snapshot loop and a `403`, which resets and stores the token | the same |
| 7 | The configurator on an ini with gone and stored cameras, a camera that cannot connect, a bad driver and a detected camera. 12 steps: load, add, add again, add an invalid one, set the order, token, rename, reset, remove a detected one, remove, reload, reload the configs. For each: the ini file as written, order, stored, detected, loaded, connected, the controller's order, driver calls | the same |
| 8 | Rows 1-7, 13 runs in a row on macOS | the same each time |
| 9 | Rows 1-7 on Linux (CI job `store`) | not yet run |

## Not covered

- **Not compared as it is:**
  - The order of a run of a driver's `set_*` calls, at connecting and in `set_settings`. Both walk
    `configurable_capabilities`, a `frozenset` whose order changes per process (`PYTHONHASHSEED`), so
    every run of `set_*` calls in every step is compared sorted.
  - What the scenario sorts before both sides are compared:
    - keys that come from that `frozenset`: the keys of `get_settings`, of `string_from_settings`, of
      the `config` of `/p/camera`, and of a camera's section in the ini (written from them);
    - Python sets: `capabilities`, `available_resolutions`, the configurator's `stored` and
      `detected`, `get_required_settings`.
    - Also sorted, although Python keeps their order: the keys of each config `scan` returns, and the
      keys of the `camera_order` section of the ini. Their order is not compared.
  - Where a `take_a_photo` falls among the other calls. The driver takes each photo on a thread of its
    own, in both, so the photos are compared as a sorted list.
  - The `Host` header (each side's port) and the `Timestamp` header of a photo taken now.
- **Sources besides the SDK**, each the reference's own runtime, and each compared by this test:
  - requests 2.32.5 for a snapshot's `User-Agent`: the SDK sets none (`REQUESTS_USER_AGENT`);
  - CPython 3.13.12's `configparser` for the ini, ported as PrusaLink uses it;
  - for `hashlib.blake2b` and `base64.urlsafe_b64encode` in `make_hash`, the crates `blake2` 0.10 and
    `base64` 0.22: used, not ported.
- **Known differences, written down in the modules:**
  - Python iterates a set (`capabilities`, `configurable_capabilities`, `available_resolutions`,
    `get_required_settings`) in hash order. Ours iterates in definition order, or in the order the
    driver gave. The scenario compares those lists sorted.
- **Not ported:**
  - A resolution whose width or height is not an `int` that fits in 64 bits: an error `NotCovered`
    (`camera.rs`, module doc).
  - A `TriggerScheme` or `Resolution` under another key in `json_from_settings`: an error `NotCovered`.
- **Not part of this audit:**
  - The camera bits of the SDK's `Printer` (`__init__.py`): they stay in the agent.
  - Real cameras: the nokhwa driver is #93 and #94.
