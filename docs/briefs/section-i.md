# Brief: section I of the Marlin port (and two section-H fixes)

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust; not a git repo; never commit). PATH:
`export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`. Read `CLAUDE.md` first: PORT,
don't paraphrase; invent nothing; one reference: OctoPrint 1.11.8 in `reference/OctoPrint-1.11.8`
(never `reference/OctoPrint`, a newer checkout), with its runtime: CPython 3.13 stdlib
(`reference/cpython-3.13`; fetch more from `https://raw.githubusercontent.com/python/cpython/3.13/Lib/<file>`)
and pyserial 3.5 (`reference/.venv-octoprint/lib/python3.14/site-packages/serial/`). Anything not
traceable to a source line is marked "ours" and listed in the report.

Read `docs/agent-engine.md` from "Approved as ours for `MachineCom`" to the end: all translation
rules and decisions, including "Section G, decided (proxy)", "The nine design decisions" and the
"Section H" paragraph. Apply them exactly. Read `crates/engine/src/ports/octoprint/comm.rs`
(MachineCom, its numbered differences 1–27, its stubs) and the other files there; keep their style.

**Only section I and the two fixes below. Nothing of the port's top edge, the DYMO or the hub.**

## Part 1: two fixes from section H

1. `py_format.rs` was written from the docs. Write `scripts/octoprint-format-check.py` that runs
   real Python's `str.format` (CPython 3.13: `~/.local/share/uv/python/cpython-3.13.12-macos-aarch64-none/bin/python3.13`)
   on every case the tests use plus the shapes `comm.py:3562-3592` can produce (str and None
   arguments, positional and named fields, conversions, format specs, `{{`/`}}`, errors), writing
   the expected output or exception type to a fixture; a Rust test compares `py_format` with it,
   byte for byte. Fix `py_format` where it differs. Anything Python accepts that `py_format`
   cannot: `unimplemented!` with the case, listed.
2. Test guard: under `cfg(test)`, `serial_list()` scans a temp directory, never `/dev`, and
   `_open_serial` panics on any path outside the temp directory, except the nonexistent
   `/dev/maker-connect-test-does-not-exist`. No test may open a real device.

## Part 2: section I, comm.py 4509–5688

Port every method in order: `_sendCommand` 4509, `_enqueue_for_sending` 4602, `_use_up_clear`
4640, `_send_loop` 4653, `_log_command_phase` 4794, `_process_command_phase` 4814,
`_process_atcommand_phase` 4947, `_needs_checksum` 4990, `_checksum_enabled` 5002, `_do_send`
5010, `_do_increment_and_send_with_checksum` 5017, `_do_send_with_checksum` 5024,
`_do_send_without_checksum` 5036, every `_gcode_*_queuing/sending/sent` handler 5131–5578,
`_trigger_emergency_stop` 5487, `_emergency_force_send` 5565, `_validate_tool` 5580,
`_reset_position_timers` 5587, the `_atcommand_*` handlers 5595–5616, `_command_phase_queuing`
5617, `_command_phase_sending` 5675. Replace their stubs. The writing side of the serial
transport (pyserial `Serial.write`, `serialposix.py:612` onward, with its `Timeout` at :618, and `write_timeout`, from the pyserial 3.5 copy above, beside the
`serialport` crate) with file:line. Helpers outside comm.py that affect what is sent: port from
their source with file:line. Only pure logging helpers may be stubs; stubs panic.

The stubs present now for this range are only those for 4509, 4602, 4653, 5036 and 5487; the other
methods have no stub and are written new, in file order. After section I, no
`unimplemented!("comm.py:…")` may remain for lines 4509–5688. The only stubs left are: the Windows
registry branch of `serialList`, the Windows default serial factory (`comm.py:3879`),
`perform_system_shutdown` (`systemcommands/__init__.py:89`, needs `util/commandline.py`), and those
in `plugin.rs`. List them in the report. Un-ignore `test_other_error_disconnect` (it needs 5487).

Porting pyserial's `Serial.write` / `write_timeout` extends the Section H decision (which named
`read`, `Timeout`, `cancel_read`): same reference (pyserial 3.5), mark it ours in the report and in
the numbered list.

## Tests

Port the reference tests that now apply (`reference/OctoPrint-1.11.8/tests/util/test_comm.py`,
`test_comm_helpers.py`; possibly none for section I — then say so). Own tests marked "ours". Required: a test that an emergency command (`M112`, `M108`, `M410`, from `serial.emergencyCommands`) is forced ahead of queued commands, and uses up an `ok` only when the EMERGENCY_PARSER capability is set (`_gcode_M112_queuing` 5524, `_emergency_force_send` 5565–5578). No test
calls `start()` on a real port or touches hardware. `cargo test -p maker-connect-engine` must pass
(confirm the count before you start; it was 241 + 1 ignored), with no new warnings.

## Report (brief)

Fixes done; items ported with lines; stubs remaining; helpers outside comm.py; every new "ours"
item and difference (also in the numbered list in comm.rs); test counts (ported vs ours).
