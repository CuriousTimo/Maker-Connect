# Brief: section J fixes (from the watchdog)

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust; never commit). PATH:
`export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`. Read `CLAUDE.md`: PORT, don't
paraphrase; invent nothing; reference OctoPrint 1.11.8 (`reference/OctoPrint-1.11.8`, never
`reference/OctoPrint`) and its runtime CPython 3.13 (`reference/cpython-3.13`). Another agent works
in `crates/engine/src/ports/labelle/`: never touch it; keep the engine crate compiling between edit
batches; in shared files only add lines. Only these fixes, nothing else.

## 1. `%`-formatting in `api_printer.rs` (`percent_format`, ~254-420)

It was written by hand and checked against Python 3.11.7. Replace it with a port of CPython 3.13's
`%` formatting for `str` (`Objects/unicodeobject.c`: `PyUnicode_Format` and the
`unicode_format_arg*` functions; fetch the 3.13 file from
`https://raw.githubusercontent.com/python/cpython/3.13/Objects/unicodeobject.c` into
`reference/cpython-3.13/Objects/`), function by function, with file:line, as `py_format.rs` was
ported from `unicode_format.h`. The conversions also use `Objects/longobject.c` (ints) and
`Python/pystrtod.c` (floats): reuse what `py_format.rs` already ported from them, and fetch/port
what is missing; no second hand-written copy. `%s`/`%r` of a dict, list or str needs Python's
`repr`: port it or `unimplemented!`, and check it against the fixture. Ints beyond what
`serde_json::Value` holds exactly: a loud error or `unimplemented!`, never rounded. Include the
dict-or-list `PyMapping_Check` case. Prove it like `py_format`: a script `scripts/octoprint-percent-check.py`
runs CPython 3.13.12 (`~/.local/share/uv/python/cpython-3.13.12-macos-aarch64-none/bin/python3.13`)
on a large set of cases (every conversion `%s %r %a %d %i %u %o %x %X %e %E %f %F %g %G %c %%`,
flags `# 0 - + space`, width, `*`, precision, length modifiers `l h L`, `%(key)`, tuple / dict /
single arguments, too few and too many arguments, big ints, floats, None, bool, the argument kinds
the JSON `parameters` of `printerCommand` can hold) and writes the expected output or exception type
to a fixture; a Rust test compares byte for byte. Known bugs to be gone: `'%s %s' % 'a'` must raise
`TypeError`; `%ld` must work; big numbers must not panic. Anything that cannot be done:
`unimplemented!` with the case, listed. The only wire caller is `server/api/printer.py:424-432`.

## 2. The event manager in `events.rs`

`fire` now runs listeners in the caller's thread. Port OctoPrint's `EventManager`
(`src/octoprint/events.py:160-251`): `__init__` (:164-179, the worker a daemon thread started with
the manager), `_work` (:181-213, with its SHUTDOWN handling and its outer `except`, "worker loop
crashed"), `fire` and `_enqueue` with the hold-back until STARTUP (:215-242, `_startup_signaled`,
`_held_back`, and the move of held-back events to the queue after STARTUP), and the subscribe
parts it uses. Add `Events::STARTUP` ("Startup", :31) and `Events::SHUTDOWN`. Use the existing
`py_queue.rs` / `py_threading.rs`. Without STARTUP nothing is delivered: note that the port's top
edge must fire it; tests using the global manager fire it under `cfg(test)`. The sink the top edge
installs is called in the worker thread after the plugin handlers, inside `py_try` (ours; document
it). Tests: a test-only bounded wait for the queue to drain (`_work` never calls `task_done`, so no
`join()`); local test managers are shut down with SHUTDOWN; watch for cross-talk on the global
manager. Correct the plan text (`docs/agent-engine.md` ~186, "the event bus as a bus" not ported)
and the `events.rs` module doc: the dispatcher is now ported (only that line; append nothing else).

## 3. Two small ones

- `action_command_prompt.rs`: a user-set `command` with more than a plain `{choice}` (a format spec
  or a conversion) is `unimplemented!` (known gap), not a `ValueError` or a silent difference.
- `api_printer.rs` "SD support is disabled" (printer.py:357-358): read
  `settings().get_boolean(&["feature","sdSupport"])` inside the function as `standard.rs` does, no
  `bool` parameter; it is ported, so take it off the "Ours" list.

## Tests

`cargo test -p maker-connect-engine` must pass (count first; it was 303), with no new warnings, and
pass 10 runs in a row (`for i in $(seq 10); do cargo test -p maker-connect-engine 2>&1 | grep '^test result'; done`).

## Report (brief)

What was ported with file:line; the check result (N cases identical); what is `unimplemented!`;
every "ours" item; test counts; the 10 runs.
