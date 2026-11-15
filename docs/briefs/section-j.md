# Brief: section J of the Marlin port — the OctoPrint files around comm.py

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust; git repo, never commit). PATH:
`export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`. Read `CLAUDE.md` first: PORT,
don't paraphrase; invent nothing; one reference: OctoPrint 1.11.8 in `reference/OctoPrint-1.11.8`
(never `reference/OctoPrint`), with its runtime (CPython 3.13 in `reference/cpython-3.13`,
pyserial 3.5 in `reference/.venv-octoprint/lib/python3.14/site-packages/serial/`). Anything not
traceable to a source line is marked "ours" and listed in the report.

Another agent may work in `crates/engine/src/ports/labelle/` at the same time: never touch it; in
`crates/engine/Cargo.toml` and `ports/mod.rs` only add lines; keep the engine crate compiling at
the end of each batch of edits (`cargo check -p maker-connect-engine`).

Read `docs/agent-engine.md` from "Step 3a" to the end: the file list ("What is ported"), M1–M5,
all translation rules and decisions (MachineCom rules, Sections C–I, B1–B9). Read
`crates/engine/src/ports/octoprint/` (comm.rs and its numbered differences, settings.rs, events.rs,
profile.rs, plugin.rs). Keep their style: one Rust file per OctoPrint file, source line per item.

**Only this section. Not the port's top edge to the engine, not the agent, not the hub, not the DYMO.**

## Part 1: the fix from section I

`tags=None` stays `None` (not an empty set) through `_enqueue_for_sending` and `_send_loop`
(comm.py:4631, 4717, 4780): `SendQueueEntry::Line.tags` becomes `Option<BTreeSet<String>>`, and
every place that now does `tags.unwrap_or_default()` (comm.rs ~4866, 5051, 5090, 5210, 5256, 5310,
5420, 5613, 5803, 5844) is checked against comm.py for what Python does with `None` there (some are
Python's own `if tags is None: tags = set()`, e.g. :1299, 1423, 1465: keep those). Update the
"ours" note in comm.rs (~2125) that says `tags=None` is stored as the empty set.

## Part 2: port, each to its own Rust file

1. `printer/standard.py` → `standard.rs`: the parts the plan lists (`docs/agent-engine.md`,
   "What is ported"): `connect` 380, `disconnect` 424, `commands` 463, `script` 480, `jog` 510,
   `home` 553, `extrude` 575, `change_tool` 596, `set_temperature` 606, `_convert_rate_value` 668, `feed_rate` 682, `flow_rate` 689, `select_file` 696,
   `unselect_file` 744, `start_print` 765, `pause_print` 792, `resume_print` 808, `cancel_print`
   824, the SD wrappers (`get_sd_files` 942, `add_sd_file` 954, `delete_sd_file` 1052,
   `init_sd_card` 1060, `release_sd_card` 1067, `refresh_sd_files` 1074). Not in the plan's list
   but needed by M3/M4 and the callbacks (added to the plan table): `disconnect` 424, the state
   getters 843–941, `__init__` 52 as far as these need it. Not ported: `set_temperature_offset`.
   The `on_comm_*` callbacks (1443–1915) are ported where they act on the wire: notably
   `on_comm_print_job_started/done/cancelled/paused/resumed` (1626–1823) send G-code scripts
   through comm. Name each callback left out and why (UI only). Also
   the `MachineComPrintCallback` methods `Printer` implements (the `on_comm_*` callbacks). Every
   input check and `ValueError` as in Python. Not ported (not wire): the web/plugin callbacks,
   estimators, analysis queue, file manager beyond what `select_file` needs (the job's file is a
   local path).
2. `printer/profile.py` → fill `profile.rs`: the class `default` (:222-257), `get_default` :443,
   `get_current_or_default` :466, `_ensure_valid_profile` :630, and what comm.py and standard.py
   read of a profile. Not ported (not wire): file I/O (`_load_all`, `_save_to_path`, `select`,
   `save`, `remove`). Replace the
   approved stand-in trait with the port where that is now possible; say what remains.
3. `events.py`: the parts comm.py and standard.py use that are not yet in `events.rs` (check).
4. `plugins/action_command_prompt/__init__.py` (302 lines) → `action_command_prompt.rs`: the whole
   wire parts: `COMMAND = "M876"` :41, `action_command_handler` :160, `gcode_queuing_handler`
   :201 (force-sends `M876 Sx`, :224), `firmware_capability_handler` :234, `_show_prompt` :244,
   `_close_prompt` :260, `_answer_prompt` :269. Register three of its `__plugin_hooks__` (:297-302): action, gcode.queuing, firmware.capabilities
   (not `octoprint.access.permissions`, not wire) through
   `plugin.rs`, replacing the empty-map stand-in for these hooks. Not ported (not wire): assets,
   templates, settings save, its API. It sends `M876 P1` on connect (:92, :240) through
   `printer.commands` and answers with `force=True` (:277): port it after `standard.rs`.
5. `server/api/printer.py`: the refusals as functions the top edge will call, in
   `api_printer.rs`; only the checks, not the HTTP layer: 409 not operational (:23, 73, 161, 174,
   216, 236, 278, 398, 456), "currently printing" :136, "not operational or currently printing"
   :302-304, "not operational or currently busy" :360-361, no heated bed / chamber (:177, 219, 239, 281),
   and the command validation of `printerCommand` (:393-445).

## Tests

Reference tests that exist: `tests/printer/test_script.py`, `tests/test_events.py` (and
`test_estimation.py`, not wire): port the first two where they apply. Everything for profile, the prompt plugin and the API
checks is ours, marked ours. No test touches
hardware. `cargo test -p maker-connect-engine` must pass (count it first; it was 268), no new
warnings.

## Report (brief)

Fix done; items ported with file:line; what is not ported and why; every new "ours" item and
difference (also in the numbered lists); test counts (ported vs ours).
