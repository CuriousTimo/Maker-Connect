# Brief: section M — finish the Marlin proof

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust; git repo, never commit; no hardware).
PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`. The LabelWriter 450 agent may
still be finishing in `ports/dymo_cups/` (its own target dir): don't touch it. Read `CLAUDE.md`, the
end of `docs/agent-engine.md` ("The five questions, decided by the agents", Section K incl. K18,
K18-1), `docs/audits/octoprint-marlin.md`, `docs/briefs/section-l-proof.md`. Reference:
`reference/OctoPrint-1.11.8`. Do these in order; one report at the end (stop early only if a step
contradicts this brief).

1. **CPython 3.13 for the reference.** Make `reference/.venv-octoprint-313` with CPython 3.13
   (`~/.local/share/uv/python/cpython-3.13.12-macos-aarch64-none/bin/python3.13` or `uv python
   install 3.13`), OctoPrint 1.11.8's own requirements as before, plus the reference's runtime needs the current
`.venv-octoprint` has (Unidecode, regex), and
   re-record the reference fixtures with it (5 runs per scenario, same settings). Note the version
   in the audit header; keep the 3.14 venv untouched.
2. **The bridge experiment (decides E1):** run the reference once more over the same out-of-process
   bridge our side uses (`scripts/octoprint-vp-bridge.py`), recording the same way, without
   changing any reference code. Report: does the reference then also read the `M112` echo (then E1 is
   the harness: record it as a read-only harness difference, with a loud check that the writes are
   identical)? Does it still never send the early `M105` (then S is ours)?
3. **`Thread.start`'s handshake** in `crates/engine/src/ports/octoprint/py_threading.rs`, from CPython
   3.13 `threading.py`: the cites show the Event handshake (`start` :956-982, `_started.wait()` :982;
   `_bootstrap_inner` :1028-1044, `_started.set()` :1033). That `start()` effectively returns only
   after the new thread's first blocking point (an `Event`/`Condition` wait, a lock acquire that
   waits, `Queue.get`/blocking `put`, `sleep`, a serial call) or its end is the approved GIL
   approximation (decision 1), not a line of `threading.py`. No full GIL, no CPython C sources, no
   timing of ours. Check each thread start of the port (event worker `events.rs` ~153; monitor and
   sender `comm.rs` ~4267/4269; the two `RepeatedTimer`s ~7604-7629; the `ResettableTimer`s
   ~5509/5754; the `Timer` ~8372) for its first blocking point, checked against its Python source, and say what it is; record any
   thread whose first blocking point you cannot find. Tests: the
   1000-run test (a `run_first` timer started before a flag is set must see it unset); a debug-only
   watchdog (`cfg(debug_assertions)` only, never on the wire) that panics if a `start()` waits longer
   than a bound: the bound is ours, a named constant in `py_threading.rs`, listed in the audit's
   "ours" list, its value told in the report. Then rerun the proof (on 3.13): the S race must be gone.
4. **K18-1:** comm's own lines after `M112` (`_trigger_emergency_stop`, comm.py:5487-5522: the
   `M104 T0 S0`/`M140 S0` via `_do_increment_and_send_with_checksum` :5017) carry no tags and fire
   no `gcode.queuing`/`sent` hooks; they only reach `_do_send_without_checksum` (:5036-5041), which
   logs `Send: …`. In `port.rs` (`on_comm_log` ~764-785) their `ok`s must credit no pending command.
   Derive the rule from comm.py, not a guess (e.g. a `Send:` line that never gets a `sent` phase for
   a `maker-connect:<id>` tag is comm's own — mind that comm logs `Send:` before the hook; or: once
   `_trigger_emergency_stop` has begun, comm's lines credit nothing), and write it as a numbered
   difference. On the wire nothing changes; this is crediting inside our port only. Test: an `M109`
   pending, then `M112`: the `M109` fails with the close, never "ok". Update K18-1.
5. **Test hygiene:** the proof tests and the top-edge test (`the_top_edge_on_a_fake_printer`) each run
   in their own process or are `#[ignore]` with a documented command, so the unit suite never
   shares OctoPrint's global event manager with them; bring back the K4 unplug phase as its own test
   (own process). `cargo test --workspace` must be green 5 times in a row; the proof run separately,
   green by the audit's rule.
6. Update `docs/audits/octoprint-marlin.md` (current result only) and its row 4 in
   `docs/audits/README.md`.

Report briefly: per step what changed and the result; the final proof table (N of 24 identical in
5/5 runs); anything left.
