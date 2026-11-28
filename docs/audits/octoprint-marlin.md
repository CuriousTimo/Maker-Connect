# Plain Marlin (`--mode marlin`): our port against OctoPrint 1.11.8

**The one reference:** OctoPrint **1.11.8** (AGPL-3.0), commit `42be740`, in `reference/OctoPrint-1.11.8`.
Ours: `crates/engine/src/ports/octoprint/`, the port of its `util/comm.py`, `printer/standard.py` and the rest
(`docs/archive/plans/agent-engine.md`, Step 3a), with the top edge `port.rs` (`OctoPrintPort`, the engine's `Port`).

**The test device is OctoPrint's own:** `plugins/virtual_printer/virtual.py` (`VirtualPrinter`; `write`
:2136, `readline` :2170), run unchanged in Python. Nothing of it is ported to Rust: both sides talk to the
same Python class, made by the plugin's own factory (`VirtualPrinterPlugin.virtual_printer_factory`).

**The reference's runtime** is `reference/.venv-octoprint-313`: CPython **3.13.12** (`cpython-3.13.12-macos-aarch64-none`),
pyserial 3.5; `setup.py:26` of OctoPrint 1.11.8 says `python_requires = ">=3.7, <3.14"`, so 3.13 is inside what OctoPrint
declares. The packages are OctoPrint's own `INSTALL_REQUIRES` (`setup.py:116`, read from the file, version ranges as
declared: Flask 3.0.3, tornado 6.5.10, blinker 1.8.2, ...) including the two the virtual printer's SD card needs
(`setup.py:107-108`, `Unidecode` 1.4.0 and `regex` 2026.11.15); not installed are the three bundled-plugin packages
(`OctoPrint-FileCheck`, `-FirmwareCheck`, `-PiSupport`), whose plugins the proof does not load. OctoPrint itself runs from
`reference/OctoPrint-1.11.8/src`, unchanged. The older `reference/.venv-octoprint` (CPython 3.14.7) is not used.

## How the proof runs

Run it after any change to the port (`scripts/octoprint-own-process-tests.sh proof` is the second line):

```
reference/.venv-octoprint-313/bin/python scripts/octoprint-port-check.py reference   # the reference alone, 5 times each
scripts/octoprint-own-process-tests.sh proof                                         # ours, 5 times each (cargo test ... --ignored --exact ...proof_against_octoprint)
reference/.venv-octoprint-313/bin/python scripts/octoprint-port-check.py compare     # compares, writes the tables below
```

`MAKER_CONNECT_PROOF_ONLY=<scenario>,<scenario>` before the first two lines runs only those scenarios on each side;
the others' results stay as they were measured last. The table below mixes such runs: `sd_job_after_list` was
measured on its own (2026-11-25, #238), the other 24 scenarios are the earlier full run's.

The port's other own-process tests (OctoPrint's event manager is global and ends with `SHUTDOWN`, so each runs in a fresh
process and is `#[ignore]` in the unit suite): `scripts/octoprint-own-process-tests.sh` runs `the_top_edge_on_a_fake_printer`
and `k4_an_unplugged_cable_is_gone`. `cargo test --workspace` runs everything else.

- **The scenarios** are in `crates/engine/tests/octoprint/scenarios.json` (with their G-code files in `files/`),
  read by both sides: connect, controls, typed G-code, prints, a resend, an `M112`, a prompt, the card. 22 of
  them were frozen before our first run (sha256 of the file then: `202c774bd75b53239282ca613993fa4444f5227ab665a1df7b56eb1cc3b29263`);
  none was dropped or reworded after. Three were added after our first run (`added_after_first_run`):
  `sd_print_after_list` and `sd_delete_after_list`, because OctoPrint's own API (`files.py:505`, `_verifyFileExists`)
  only selects or deletes a file it has listed, by its DOS name, which `sd_print` and `sd_delete` do not do:
  both sides refuse those two and send nothing (the scenario then waits for its line and ends with the same
  timeout on both sides); and `sd_job_after_list` (#238), the print from the card as a job (`Order::SdJob`, port
  K7 and K20), which the reference runs as the files API's print of a card file, as `sd_print_after_list`.
- **The reference run** (`scripts/octoprint-port-check.py`, with `scripts/octoprint_proof_env.py`) runs real
  OctoPrint's `Printer` (`printer/standard.py`) and `MachineCom` (`util/comm.py`), headless, in a process of
  its own per scenario. The settings, the plugin manager (only `virtual_printer` and `action_command_prompt`
  enabled, as in the port) and the implementations are set up as `octoprint/__init__.py` and
  `server/__init__.py` set them up. A scenario step is the call our top edge makes for it (M1): the
  `Action`s of `port.rs` become the `printer.*` calls of the API endpoints that make them, after the same
  refusals (`api_printer.rs`; a refused action sends nothing, on both sides). `serialList` scans an empty
  directory instead of `/dev` (as our test guard does), so the reference never looks at a real device.
  The file manager is the port's stand-in (`LocalFileManager`: the job's file is a local path).
- **Our run** is a Rust test (`crates/engine/src/ports/octoprint/proof.rs`): the engine's `OctoPrintPort`,
  given `Order`s (`Command`, `Job`, `Control`, `Answer`, `Close`), in a process of its own per run (OctoPrint's
  event manager ends with `SHUTDOWN`: one run of the port per process). Its serial object is `Bridge`, registered
  in the test seam `TEST_TRANSPORTS` (`comm.rs`), a pipe to **`scripts/octoprint-vp-bridge.py`**.
- **The bridge is ours and a pure byte pass-through:** it makes the `VirtualPrinter` (same factory, same settings
  as the reference) and passes on `write(bytes)`, `readline()`, `timeout = x` and `close()`. No filtering, no
  retry, no timing of its own, no recording. (The Rust `Bridge` keeps the value it last set for the `timeout`
  getter; `set_baudrate` panics, as the virtual printer has no setter for `baudrate`, and no scenario reaches it.)
- **Recording:** each side records, in order, every `write` (each call's bytes) and every non-empty line read.
  Steps wait for the wire only (`settle`: nothing written or read for N ms, counted from the start of the step;
  `wait_write` / `wait_read`: the Nth matching line) or for `Operational`: the same rules on both sides.
- **Reproducibility:** every scenario was run 5 times against the reference alone (`reference/summary.json`); one
  whose reference transcripts differ between its own runs is `not reproducible` and on "not covered", never
  "identical" (2 of 24 are: `connect_auto`, `emergency_m112`, see section 3). Our side also runs every scenario 5 times.
- **The bridge experiment** (`octoprint-port-check.py reference-bridge`, then `bridge-check`): the same reference run,
  OctoPrint's `Printer` and `MachineCom` unchanged, but the serial object `_open_serial` gets is a pipe to
  `octoprint-vp-bridge.py` (the process our side talks to), spoken to as the Rust `Bridge` speaks to it
  (`BridgeSerial` in the script). Fixtures: `crates/engine/tests/octoprint/reference-bridge/`, 5 runs per scenario.
  Result: the writes over the bridge are identical to the in-process reference's in all 24 scenarios, and
  all 24 scenarios are reproducible over it (`bridge-check` prints it and exits 1 if a write differs). No run over the
  bridge writes an early `M105` (comm's first temperature poll). The echo after `M112`
  (`echo:EMERGENCY SHUTDOWN DETECTED. KILLED.`) is read over the bridge in 30 of 30 extra runs of `emergency_m112`, and in-process
  in 30 of 30 (one process at a time); in the full 5-run sample, six runs at once, in-process 4 of 5 (section 3).

## The ported OctoPrint tests

`tests/util/test_comm.py` (16 test functions) and `tests/util/test_comm_helpers.py` (21) are ported, case by
case, into `crates/engine/src/ports/octoprint/comm.rs` (same names; checked by name, none missing) and pass in
`cargo test --workspace`.

## The normalisation rules

Two layers, the same rules for both sides:

- **(a) The bytes sent to the printer:** in order, identical. **No normalisation at all** (0 writes dropped or changed).
- **(b) The lines read back,** as an ordered sequence, after dropping only `wait` lines (a line that is `wait`) and the
  numbers behind `T:` / `B:` while the simulated temperatures are still moving (only in a scenario marked
  `temperatures_moving`: `temperature`). The counts of what these rules dropped are under the scenario table.

No other rule. The approved ours-only items of Section K do not change a single line on the wire, so none needs a rule;
how each is kept out of the comparison, and the lines it dropped (all 0):

| Item | What it changes | In this comparison | Lines dropped |
|---|---|---|---|
| K1 (typed G-code goes as typed, one command per non-empty line) | the lines `commands()` gets | the reference script splits the same text the same way (`gcode_lines`) | 0 |
| K2 (job.py's and files.py's refusals, S5) | a refused action sends nothing | the reference script makes the same checks (`refuse_unless`) before each call | 0 |
| K6 (the file arrives as `job-<id>.d/<name>`) | the path of the selected file | never on the wire (a local print sends the G-code lines only); the reference selects the fixture file | 0 |
| K7 (SdPrint is `select_file(sd=True)` with `printAfterSelect`) | the call | the reference script makes the same call | 0 |
| K11, K13 (the done rule, the output lines) | what the report says | not wire | 0 |
| K3, K4, K8, K9, K10, K12, K14, K16 | the report, jobs, `Gone`/`Back`, state texts | not wire | 0 |
| K18-1 (comm's own lines after `M112` credit no pending command) | which pending command gets a received line and `ok` | crediting inside the port only, nothing on the wire; `the_top_edge_on_a_fake_printer` | 0 |
| `Thread.start` returns at the new thread's first blocking point (the approved GIL approximation, `py_threading.rs`) | when a started thread's first action runs relative to its starter | the thread's own sequence is OctoPrint's; see "The thread start" below | 0 |
| K15 (`TEST_TRANSPORTS`) | where `_open_serial` gets its serial object | the test seam itself | 0 |
| K17 (the hub's `typed_gcode` filter) | what typed G-code reaches the port | the hub is not in the loop; open, for the owner | 0 |

## 1. The scenarios: OctoPrint's own code, run, against ours

Ours is run 5 times per scenario; "identical" means identical in all 5. The table is the last measurement (`ours/result.json`).

<!-- generated: begin -->
25 scenarios, each run 5 times on our side and 5 times on the reference's side (CPython 3.13.12): **23 identical**, 0 differ, 0 vary from run to run, 2 not reproducible (reference).

| Scenario | What it does | Reference (OctoPrint's own code, run) | Ours: runs identical in each layer | Result |
|---|---|---|---|---|
| `connect_given` | connect, port and baud given | 5 writes, 16 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `connect_auto` | connect, port and baud auto-detected | 6 writes, 19 lines read | writes 0/5, lines 5/5 | **not reproducible** (the reference differs between its own runs): not compared |
| `home_xy` | home X and Y (OctoPrint's XY button) | 8 writes, 19 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `home_z` | home Z (OctoPrint's Z button) | 8 writes, 19 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `jog` | jog X, Y and Z, with and without a feed rate | 14 writes, 25 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `extrude` | extrude and retract | 15 writes, 26 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `temperature` | set the nozzle and bed temperature, then off | 9 writes, 20 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `fan` | fan on and off | 7 writes, 18 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `motors_off` | motors off | 6 writes, 17 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `speed_flow` | feed rate and flow rate | 7 writes, 18 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `gcode_single` | typed G-code, one line | 7 writes, 20 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `gcode_several` | typed G-code, several lines (one with leading spaces, an empty one) | 9 writes, 22 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `print_small` | print a small G-code file to the end | 17 writes, 29 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `print_pause_resume` | pause and resume during a print | 18 writes, 31 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `print_cancel` | cancel during a print | 16 writes, 28 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `resend` | a resend request (the virtual printer's own trigger_resend_lineno) | 9 writes, 28 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `emergency_m112` | an emergency stop: M112 typed | 9 writes, 17 lines read | writes 5/5, lines 5/5 | **not reproducible** (the reference differs between its own runs): not compared |
| `prompt_answer` | an action:prompt from the printer and an answer | 11 writes, 25 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `sd_list` | list the SD card (after a first reply line, so that comm's deferred card refresh has run) | 8 writes, 27 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `sd_print` | select a file on the SD card and print it | 5 writes, 16 lines read; ended: timed out waiting for a line matching "Done print... | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `sd_delete` | delete a file on the SD card | 5 writes, 16 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `disconnect` | connect and disconnect | 5 writes, 16 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `sd_print_after_list` | select a file on the SD card and print it, after listing the card (OctoPrint's API only selects a file it has listed, by its DOS name) | 12 writes, 36 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `sd_delete_after_list` | delete a file on the SD card, after listing the card | 10 writes, 32 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |
| `sd_job_after_list` | the same print from the card as a job (#238, Order::SdJob): the files API's print of a card file, as sd_print_after_list | 12 writes, 36 lines read | writes 5/5, lines 5/5 | **identical** (5 of 5 runs) |

No difference is measured in a scenario the reference reproduces.

Normalisation counts (lines dropped, summed over the scenarios; the reference's fixture runs, and our runs):

- `wait` lines dropped: reference 0, ours 0 (the virtual printer's `sendWait` is off, so it sends none)
- numbers behind `T:`/`B:` masked (only in a scenario with `temperatures_moving`): reference 0, ours 0

Settings changed from OctoPrint's and the virtual printer's defaults (the same on both sides; `scenarios.json`):

| Setting | Value | Why |
|---|---|---|
| `virtual_printer.enabled` | `true` | the plugin only answers the port VIRTUAL when it is enabled |
| `virtual_printer.sendWait` | `false` | no `wait` lines every waitInterval seconds: they come from the wall clock |
| `virtual_printer.capabilities.AUTOREPORT_TEMP` | `false` | no timed temperature reports from the printer: they come from the wall clock (and comm would send M155) |
| `virtual_printer.capabilities.AUTOREPORT_SD_STATUS` | `false` | no timed SD status reports (and comm would send M27 S) |
| `octoprint.serial.timeout.temperature` | `3600.0` | comm's own temperature poll (default 5 s): it lands in a run at a moment that depends on how long the run took, so it is put off the clock; it also makes the read timeout 3601 s, so no timeout happens |
| `octoprint.serial.timeout.temperatureTargetSet` | `3600.0` | as above, the poll while a target is set (default 2 s) |
| `octoprint.serial.timeout.sdStatus` | `3600.0` | as above, the SD status poll while printing from the card (default 1 s) |
| `virtual_printer.capabilities.PROMPT_SUPPORT` (scenario `prompt_answer` only) | `true` | the printer must report that capability for the prompt plugin to show a prompt (`action_command_prompt`: `enable` is `detected`) |

Ported OctoPrint tests, checked by name: `tests/util/test_comm.py`: 16 of 16 test functions have a Rust test of the same name in `comm.rs`; `tests/util/test_comm_helpers.py`: 21 of 21 test functions have a Rust test of the same name in `comm.rs`.
<!-- generated: end -->

## 2. The differences

**None is measured in a scenario the reference reproduces:** 22 of the 22 are identical in all 5 runs, in both layers (writes
with no normalisation, lines read after the two rules). `tests/octoprint/known-differences.json` holds what has differed in the
two scenarios the reference does not reproduce, so that the test stays honest; the test fails on any difference it does not list.

**The two scenarios that are not compared** (the reference differs between its own 5 runs; ours is run as for the others):

- `connect_auto`: OctoPrint's own threads write the `M21` of auto-detection with a line number (`N2 M21*18`) in some runs and without
  (`M21`) in others (2 and 3 of the reference's 5 runs). Ours writes `M21` in 5 of 5 runs, the other of
  the reference's two variants; the lines read are identical in 5 of 5.
- `emergency_m112`: the printer's echo of `M112` (`echo:EMERGENCY SHUTDOWN DETECTED. KILLED.`) is read by the reference's monitor in
  some runs and not in others: 4 of 5 in the sample (six runs at once), 30 of 30 one at a time, in-process and over the bridge.
  The writes are identical in all runs (`N0 M110 N0*125` ×4, `N1 M115*39`, `M112`, `N2 M112*35`, `N3 M104 T0 S0*34`, `N4 M140 S0*97`);
  ours reads the echo in 5 of 5 runs and writes the same.

**The thread start (ours, approved: `docs/archive/plans/agent-engine.md`, "the five questions", decision 1).** CPython's `Thread.start()` waits on
`_started` (`threading.py:956-982`, `:982`), which the new thread sets in `_bootstrap_inner` (`:1028-1044`, `:1033`); holding the GIL it runs
on until it first blocks, so `start()` effectively returns after that. The GIL is not ported. `py_threading.rs: Thread::start` returns when
the new thread reaches its first blocking point (an `Event`/`Condition` wait, a lock acquire that has to wait, `Queue.get`/a blocking `put`,
`time.sleep`, `Thread.join`, a serial `readline`/`write`) or ends. The first blocking point of each thread the port starts, from its Python
source (observed by `the_top_edge_on_a_fake_printer`, debug builds, `py_threading::FIRST_BLOCKING`):

| Thread (port) | Python source | First blocking point |
|---|---|---|
| event worker (`events.rs:153`) | `events.py:177-179`, `_work` :184 | `self._queue.get(True)` on an empty queue (`queue.py` `not_empty.wait()`); `py_queue.rs` `get` |
| monitor (`comm.rs:4267`) | `comm.py:2260 _monitor`, :2312 | `sayHello` only queues; the first `self._readline()` (:4176, `self._serial.readline()`) |
| sender (`comm.rs:4269`) | `comm.py:4653 _send_loop` | `self._clear_to_send.wait()` (:4659), else `self._send_queue.get()` (:4665), else the first `self._serial.write` (`_do_send_without_checksum`, :5053); observed: the write |
| temperature and SD-status timers (`comm.rs:7604-7629`) | `util/__init__.py:1293-1300 RepeatedTimer.run`, `comm.py:3717-3733` | `run_first`: the poll (`_poll_temperature` :3618, `_poll_sd_status`), then `self.finished.wait(self.interval())`; the poll sends nothing while the state is not operational |
| position-log timers (`comm.rs:5509`, `:5754`) | `util/__init__.py` `ResettableTimer.run` | `self._event.wait(self.interval)` |
| resend-ok `Timer` (`comm.rs:8372`) | `threading.py:1342 Timer.run` | `self.finished.wait(self.interval)` |
| `finalize` / `log_print` threads (`standard.rs:1416`, `:1685`) | `standard.py:1492`, `:1699`, `:1773` | none: `log_print` of the port's stand-in file manager and `eventManager().fire` (an unbounded `put`) do not wait; the thread ends |

No thread of the port is without a first blocking point or an end. A debug-only watchdog (`cfg(debug_assertions)`, never on the
wire) panics when a `start()` waits longer than **`START_HANDSHAKE_BOUND` = 10 s** (ours: a named constant in `py_threading.rs`) for it,
so a thread that blocks somewhere that is not a blocking point shows up as a panic in a test, not as a hang. Tests:
`a_run_first_timer_started_before_a_flag_is_set_sees_it_unset_1000_times` (`util.rs`), `start_returns_*`,
`a_thread_that_starts_a_thread_*` and `the_watchdog_panics_*` (`py_threading.rs`). CPython's forced switch every 5 ms and every other
release of the GIL are not modelled.

**Ours, nothing on the wire (K18-1).** Comm's lines after an emergency stop are its own: `_trigger_emergency_stop` (`comm.py:5487-5522`,
reached from `_gcode_M112_queuing` :5524-5525 and from the error path :4110-4136) sends `M112`, `N<n> M112`, `M104 T<n> S0` and `M140 S0`
through `_do_send_without_checksum` / `_do_increment_and_send_with_checksum` (:5017, :5036-5041), which log `Send: ...` and write, with no tags
and no `gcode.queuing`/`sent` hook, and then closes. In `port.rs` the edge sets `emergency` when a `gcode.queuing` hook sees gcode `M112` (any
tags; comm's `_gcode_M112_queuing` runs right after the hooks of that phase) or `on_comm_error` says `consequence == "emergency"` (comm calls
`_trigger_emergency_stop` right after), and from then until `Gone`/`Back` the lines received credit no pending command (no output line, no `ok`).
An `M109` pending when an `M112` is typed fails with the close, "the printer connection closed", never `ok`
(`the_top_edge_on_a_fake_printer`).

**Exception, owner 2026-11-28 (#186, "b"): the time left at the start comes from the slicer.** OctoPrint gives the
estimator a total ahead of the print from its own G-code analysis: `standard.py:1328-1333` reads
`fileData["analysis"]["estimatedPrintTime"]` (written by `analysis.py:540`, `analysis["total_time"] * 60`) and sets it as the
selected file's `estimatedPrintTime`, type `"analysis"` (:1356-1359); `estimation.py:133-153` uses it until its own estimate takes
over. That analysis is not ported. Ours: `LocalFileManager::estimated_print_time` (`standard.rs`, difference 10) reads the slicer's
`; estimated printing time (normal mode)` with our gcode-metadata port (`estimated_to_seconds`) and sets it the same way, type
`"analysis"`. Without it the estimator is linear from the first second (`estimation.py:175-178`): 7 h 01 min left after 30 s of a
20 mm cube on the AtomX (hub .7, 2026-11-28). The estimator's arithmetic is OctoPrint's. Test:
`ours_selected_file_estimated_print_time`. Not covered: `averagePrintTime` (no print statistics are kept).

## 2b. Deliberate exceptions: what PrusaLink shows on a Marlin printer's screen (#187, #189)

**OctoPrint sends no `M73` and no `M117` of its own.** The comparison above (sections 1-2) is OctoPrint against ours and stays
identical: OctoPrint's virtual printer never says `Cap:BUILD_PERCENT:1` (no hit in `reference/OctoPrint-1.11.8/src`), so the port
sends no `M73` in any of its scenarios. What follows is PrusaLink's behaviour (`reference/Prusa-Link` @ `171148ce`) on a
non-Prusa machine, by the owner's decision. The module is `crates/engine/src/ports/prusalink/`; every claim below about stock
Marlin is from `reference/Marlin` @ `09787290`.

### `M73`, built and wired (#187)

Owner, 2026-11-22: "in deze specifieke geval is PrusaLink-gedrag op een niet-Prusa-machine acceptabel. het is namelijk een
quality of life feature die verder weinig impact heeft." Conditions decided in the issue: only for `Cap:BUILD_PERCENT:1`; OctoPrint's
numbers (#186) in PrusaLink's command at PrusaLink's moments; recorded here.

| Item | PrusaLink | Ours | Result |
|---|---|---|---|
| The line | `util.py:187-196 get_print_stats_gcode`: `M73 Q.. S.. C-1 P.. R.. D-1`, normal and quiet the same (`file_printer.py:315-331`) | `print_stats.rs get_print_stats_gcode`, `print_stats_command` | identical (`print_stats::tests::identical_to_prusalink`: 130 rounding cases and 14 files, PrusaLink's own functions run) |
| Which file | `print_stats.py:30-57 track_new_print`: no M73 of its own (`"M73" in gcode` after the comment is cut, `get_gcode`) | `PrintStats::track_new_print` | identical (same test; CR, CRLF, no last newline, unicode, a line that `unidecode` empties) |
| The moments | `file_printer.py:261-286 print_gcode`, `:333-344 to_print_stats`: every `STATS_EVERY` = 100 G-code lines (`const.py:122`) and at `total - TAIL_COMMANDS` (10, `const.py:123`) | `PrintStats::next_gcode`, called for each line comm queues from the file (`port.rs m73_before_file_line`) | identical decisions (same test); differs in where the line goes, below |
| The numbers | `print_stats.py:74-103 get_stats`: `round(fraction * 100)`, `round(sec_remaining / 60)` | OctoPrint's `completion` and `printTimeLeft` (#186), rounded the same way (Python's round half to even) | **differs, by decision** (owner: OctoPrint's numbers) |
| Only with | a Prusa always knows `M73` | `Cap:BUILD_PERCENT:1` (`M115.cpp:185`, `ENABLED(SET_PROGRESS_PERCENT)`), read from comm's capability report | **differs, by decision** |

**What else differs, and why.**

1. *Where in the stream.* PrusaLink enqueues the M73 with `to_front=True` before the Nth line, and the printer gets it before that line. OctoPrint's
   comm streams the file itself (`comm.py _send_from_job`, ours `comm.rs:9413`); the port is told of each line in comm's `queuing` phase (tag `source:file`),
   when that line is already on its way, and sends the M73 through `sendCommand` (`comm.rs:5276`), which while printing puts it in comm's command queue,
   served before the file at the next `_continue_sending` (`comm.rs:8323-8331`, `_send_from_command_queue` first). That is `to_front`'s place in OctoPrint,
   one line later: the M73 follows the 100th line, it does not precede it. Seen on the fake printer (`m73_goes_to_a_printer_with_build_percent`).
2. *Line numbers.* PrusaLink's M73 has no line number and checksum (`enqueue_instruction` without `to_checksum`); comm numbers and checksums every line it sends while printing, as it does for any command (OctoPrint's rule).
3. *Counting.* PrusaLink counts the non-empty lines after `get_gcode` (comment cut, strip, `unidecode`); ours counts the lines comm queues from the file (OctoPrint's own reader and its `process_gcode_line`, not compared with `get_gcode`: no `unidecode`, and what it drops is not checked). Compared on the fake printer with plain lines only.
4. *No number, no M73.* PrusaLink always has numbers. OctoPrint has `printTimeLeft` only once the progress is not 0 (`standard.py:1140`); the port reads the last published progress (`Printer.progress_data`, refreshed by the state monitor at most every 0.5 s). When `completion` or `printTimeLeft` is missing nothing is sent (the function's own default of -1 would be read by Marlin as a huge `R`, `parser.cpp:88-92`). In the fake printer's one-second print the first M73 therefore comes at line 200, not 100, and shows the snapshot of that moment.
5. *The tail.* `get_stats` returns 100 percent at `gcode_number == total - 10` (`print_stats.py:100-101`); that is part of its numbers and is not applied.

**What stock Marlin does with the letters Q, S, D** (the question of #187): nothing. `M73` reads `P`, `R` and `C` only (`gcode/lcd/M73.cpp:57-71`; the whole
function exists only with `SET_PROGRESS_MANUALLY`, :25, off by default, `Configuration_adv.h:1797`). The parser records every letter A-Z with its value as a
parameter and never rejects one (`gcode/parser.cpp:359-409`: `PARAM_OK` is `true`, or `A`-`Z` with `FASTER_GCODE_PARSER`; `string_arg` is only set for a letter without a value),
so `Q`, `S`, `D` are parsed and not read. Harmless. One consequence of PrusaLink's `C-1`: only with `SET_INTERACTION_TIME` (off, `Configuration_adv.h:1801`) `M73.cpp:69-71`
reads it, `dec_to_ulong("-1")` is `0xFFFFFFFF` (`parser.cpp:88-92`), `60 *` that wraps in 32 bits, and the interaction countdown shows nonsense. The `P` of ours is a whole percent
(`value_byte`, `parser.h:283`, or `value_float` with `PROGRESS_SCALE` > 1, `M73.cpp:59-62`); `R` is minutes (`M73.cpp:66`, only with `SET_REMAINING_TIME`, on with `SET_PROGRESS_MANUALLY`, `Configuration_adv.h:1800`).

### The printer's screen: built and wired (#189)

Owner, 2026-11-22: "We moeten dit ook doen. zowel op Prusa als op Marlin." Text "Maker Connect OK" where PrusaLink says "PrusaLink OK" (owner: "Maker Connect OK"); the registration code, "Is ook een quality of live. met verder weinig impact."

Built: `carousel.rs` (`structures/carousel.py`), `lcd_printer.rs` (`lcd_printer.py`: every screen, priority, timing, the 19-character cutting and scrolling, the chimes as data, the progress graphic, the 42 easter egg), `unidecode.rs` (+ the package's tables, generated by
`scripts/prusalink-unidecode-tables.py`, `--check` is green). **Result: identical** to PrusaLink's own `LCDPrinter` run on a fake clock with the product name "PrusaLink": the same `M117 ~text` and chime lines at the same moments, in the same order,
in 76 scenarios (`lcd_printer::tests::identical_to_prusalink`: ready, idle, 42, printing with a semicolon and non-ASCII name, recovering, wizard with and without a printer number, recovery, firmware messages, host messages, 13 conditions with and without the network chime, during a print and not, uploads of every transfer type
at 12 progress values, finished and stopped). `unidecode`: all code points identical (`unidecode::tests::identical_to_the_package`).
**Differs, by decision:** the product name (`PRODUCT` = "Maker Connect"; "Maker Connect OK." and its 19-character lines, `maker_connect_ok`), and one more screen, the registration code "Code: <code>" (Buddy `src/gui/dialogs/DialogConnectReg.cpp:109`; `registration_code`, priority 40, `the_registration_code_screen`); when the code goes the screen is disabled, with no text of its own.

**Wired to the Marlin path** (`port.rs` `Lcd`, `OctoPrintPort::with_lcd`; the agent's inputs in `crates/agent/src/marlin.rs` `show`). Owner, 2026-11-23: M117 on Marlin yes; chimes "Nee, niet op Marlin"; screens "Alles, behalve de wizard". The checks of the issue, from Marlin's source:

- *(a) M117.* There is no M115 capability for it: `M115.cpp:137-273` has no line for `HAS_STATUS_MESSAGE`. Without it `gcode.cpp:697` is `case 117: TERN_(HAS_STATUS_MESSAGE, M117()); break;`, a silent no-op, and the `ok` still comes (`gcode.cpp:1218 queue.ok_to_send()`; the AtomX's 2.1.2.10: `gcode.cpp:681`). **So M117 is sent to every Marlin:** on a printer without a status display it does nothing.
- *(b) M300.* With no beeper or speaker: `gcode.cpp:804-806`, `#if HAS_SOUND case 300`; `HAS_SOUND` needs a `BEEPER` pin or a buzzer panel (`Conditionals-5-post.h:3405-3419`, `M300.cpp:25`). Otherwise `default: parser.unknown_command_warning()` (`gcode.cpp:1210-1214`), which prints `echo:Unknown command: "M300 ..."` (`parser.cpp:453-455`, `language.h:143`), and the `ok` follows. M115 does not tell. **The chimes are not sent** (owner).
- *(c) M79.* Stock Marlin has it only with `DGUS_LCD_UI_SERMOON_V1` or `CREALITY_RTS` (`gcode.cpp:582-584`), where it is Creality's cloud status (`gcode/stats/M79.cpp:34-39`: `S` is a cloud command number); otherwise the unknown-command echo. **No M79 on Marlin** (the owner's proposal; nothing is built for it).
- *`M117 ~text`.* PrusaLink's text starts with `\x7E` (`lcd_printer.py:533`). On Marlin the `~` is not known to be anything but a character; not checked.

**Where it differs from PrusaLink's thread** (`port.rs` `Lcd`): its lines go through comm's `sendCommand`, as #187's `M73` (numbered and checksummed while printing); the thread starts when comm is first operational and sends nothing while it is not (PrusaLink's starts with its serial connection); nothing notifies its wait, so each wait is the whole interval. On the fake printer: `the_screen_shows_prusalinks_lines` ("Maker Connect started", "Code: ABC-123", "Maker Connect stopped", no `M300`). **Not tried on a real printer.**

**The screens and their inputs on our agent** (`marlin::show`, each second from the link loop):

| Screen | Input | On the agent |
|---|---|---|
| print (file name) | job running, selected file | exists: `agent.label` and `agent.printing` (`marlin.rs:256-257`) |
| recovery, "Preparing recovery" | `recovering`, `recovery_ready` | none: no power-panic recovery on Marlin (`from_line` is refused, `marlin.rs:251`) |
| upload / sync bar, "Transfer finished/stopped" | `printer.transfer` | wired: the SDK port's `Transfer` (`crates/connect-sdk/src/download.rs`: `in_progress`, `progress`, `transferred`, `type_`, `size`); Unix only, as the agent's downloads (#184) |
| errors, "Errors resolved", "Please wait" | `COND_TRACKER.get_worst()` | the SDK port has the condition chain (`crates/connect-sdk/src/conditions.rs`: Internet, HTTP, Token, API); read with `CHAIN.tracker.get_worst()`, but nothing sets the chain yet, so the error screens do not show; PrusaLink's own (ID, FW, SN, ... LAN, PHY, DEVICE) do not exist |
| ready ("Ready to print" + IP) | state `READY` | none: part 9's states have no `READY` (`marlin.rs:32-45`) |
| idle ("Maker Connect OK." + IP) | LAN, `local_ip`, `latest_telemetry.speed` | `local_ip`: `lcd_printer::get_local_ip` (`util.py:76-88`), each second; `lan`: an IP was found (ours: PrusaLink's `LAN` condition comes from its IP updater, not ported); `speed`: the telemetry's `speed_percent` (M220) |
| wizard ("Go: ip", "Setup completed") | `settings.is_wizard_needed()` | none: PrusaLink's web wizard |
| printer number (`ip/3`) | `printer_number` | none |
| registration code | `agent.code` | wired (`main.rs:1369-1419`, set from `/p/register`'s `Code` header at :1391, cleared at :1413 and :1419 when registered) |
| firmware messages | the printer's `LCD status changed` line (`lcd_updated`, `regular_expressions.py:143`) | Prusa-Firmware only (`messages.cpp:425`); stock Marlin has no such line (no hit in `reference/Marlin/Marlin`): the screen then runs without that input |

## 3. Not covered

- **The M2 actions** (load and unload filament, probe the bed, the laser actions, frame): OctoPrint has no such control and
  the port answers a failed `CommandDone`, "OctoPrint has no such control", sending nothing (M2;
  `the_top_edge_on_a_fake_printer` asserts it). Nothing is on the wire, so there is nothing to compare.
- **Everything that is not bytes to and lines from the printer:** the report (`Telemetry`, the M3 states, `problem`, progress,
  the prompt as Attention), `CommandDone` and its output (M4, K11, K13), `JobEnd`, `Gone`/`Back` (K4), the refusals' texts
  (K2, K8). They are asserted by the port's own tests (`port.rs`), not compared with OctoPrint here.
- **A real serial port.** Both sides talk to the virtual printer through an object, not a device: pyserial's
  `Serial.read`/`write` and the `BufferedReadlineWrapper` (`serial.rs`; `serialposix.rs` on Unix with the `serialport`
  crate, `serialwin32.rs` on Windows, a port of pyserial 3.5 `serialwin32.py`; `comm.py`'s workarounds at :3900-3927) are
  not exercised; `serialwin32.rs`'s own tests (Windows CI) cover only what runs without a port. The real `/dev` scan and the baud-rate loop beyond its first candidate are not either
  (`connect_auto` finds one stand-in device and the first baud rate works).
- **A real Marlin printer:** not tried (D11), also not for the M73 of section 2b (`SET_PROGRESS_MANUALLY` firmware for the AtomX is #188).
- **Section 2b:** the screen's `M117` traffic, its chimes, `M79` and the screens' inputs are not sent or wired; the counting of lines of OctoPrint's reader against `get_gcode` beyond plain lines; the M73 against a firmware with `SET_PROGRESS_MANUALLY`; Marlin's `M117` display (scrolling, the `~`).
- **Time.** The virtual printer's wall-clock behaviour is switched off (`sendWait`, the timed reports) and comm's own polls are
  put off the clock (`serial.timeout.temperature`, `temperatureTargetSet`, `sdStatus`: 3600 s), so the temperature polling
  and comm's timeouts are not covered: the communication and `ok` timeouts, the resend-with-timeout, `M109`/`M190` heat-up
  waits, `G4` dwells, `M600`'s busy loop, long-running commands, the SD status polling during a card print. Heating moves the
  simulated temperatures (`temperature` masks them, see the rule).
- **Other settings and printer flavours:** only the default OctoPrint settings (forced checksums, `neverSendChecksum`, other
  scripts, `serial.sdAlwaysAvailable`, ...) and the virtual printer's default firmware are run; not its other options
  (`okBeforeCommandOutput`, Repetier, Smoothie or Klipper reporting, several extruders, a chamber, `resend_ratio`, `supportM112`
  off) and not other resend variants than `trigger_resend_lineno` (checksum mismatch, missing line number, `rerequest_last`,
  the simulated errors at lines 100-115).
- **Not in a scenario:** a print from a file on the card with a pause or cancel; a disconnect during a print; an unplugged
  cable; uploading to the card (`startFileTransfer`; the hub never does); `select_file` without printing (there is no
  `Action` for it); `change_tool`; offsets; a print from a line (`from_line`, K3); typed G-code of an `@` command or a `!!DEBUG:`
  sequence beyond those used here.
- **Plugins:** only the prompt plugin is loaded on both sides; the other bundled plugins are disabled on the reference side
  (`action_command_notification`, `customcontrolmanager`, `eventmanager` ... send nothing by default).
- **The hub:** its typed-G-code filter (K17) and its queues are not in the loop.
- **`reference` samples:** the reference's own runs are a sample (5 per scenario, 24 scenarios, six at a time). A scenario that
  differs between the reference's own runs is `not reproducible` and not compared: `connect_auto` and `emergency_m112` (section 2).
- **An unplugged cable and the other half of K4:** not a scenario of the comparison; `k4_an_unplugged_cable_is_gone` (own process) tests that
  the report says `Gone` and a pending command fails with the close.
