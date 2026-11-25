# PrusaLink port (MK3 family, agent): audit

The one reference is PrusaLink (`reference/Prusa-Link`, branch `websockets`, `171148ce`) with its
Connect SDK (branch `websockets`, `c0d7d233`), ported section by section
(`docs/archive/plans/mk3-port-plan.md` §4). Each section is proved by running PrusaLink's own code
and ours on the same examples (`crates/engine/src/ports/prusalink/driver_*.py`, run by the
section's Rust test with `reference/.venv-prusalink`, Python 3.9.6, here, and with CPython 3.13
in CI's PrusaLink job).

## A0: `config.py` and extendparser's `Get` (#241)

Ours: `crates/engine/src/ports/prusalink/config.rs`, on CPython 3.13.12's `configparser`
(`crates/connect-sdk/src/configparser.rs`; `BasicInterpolation`, `getboolean`, `ConfigParser.set`'s
check, `in` and `parser[section].items()` added for it).

Proof: `config/tests.rs` `config_and_settings_are_prusalinks` runs 34 examples through
`driver_config.py` (the real `Config` and `Settings`) and through ours: every model value's type
and `str()`, `log_settings`, the log handler's kind and format, what `update_sections` and `write`
give (or its error), the settings file written, `is_wizard_needed`, `use_connect`, and each
exception's class and text. **Result: 34 of 34 identical.** The examples include a missing and an
empty file, overrides from the arguments, Python `int()` (`+8_080`, `1_`), `getboolean`
(`TRUE`, `maybe`), `%(name)s` from `[DEFAULT]`, `%%`, a missing name, bad syntax, the depth
limit, an unsupported printer type, a parsing error and a `%` that `update_sections` refuses.

What the MK3 port reads (the call paths in `printer_adapter/`, `sdk_augmentation/`,
`conditions.py`, `util.py`): `cfg.printer.{port, baudrate, reset_disabling, settings, directory,
storage}`, `cfg.daemon.{threshold_file, power_panic_file, printer_number}`, `cfg.http.port`,
`cfg.cameras.auto_detect`, `settings.printer.{type, network_error_chime}`,
`settings.service_connect.{hostname, port, tls, token}`, `settings.service_local.api_key`,
`settings.use_connect()`, `is_wizard_needed()`, `update_sections()`. The rest is read by the web
side and `multi_instance/` only. `cfg.printer.storage` is not set by `config.py` (its option is
commented out, `:184`): a read is `AttributeError`, as in PrusaLink.

**Not covered:** `set_global_log_level` and the log handler itself (Python's `logging`; what
`get_log_handler` reads from the file is covered); `Settings.instance` (one per process in
PrusaLink, one per printer module here, #218); `bidict` (only `in` of `PRINTER_CONF_TYPES`'s keys
is used); `get_option`'s `list`, `tuple` and `set` targets (unused); `pwd` on Windows (an error
names the call); a `SysLogHandler` that cannot connect (the examples run in the foreground); an
`OSError`'s text (its `repr` is given).
