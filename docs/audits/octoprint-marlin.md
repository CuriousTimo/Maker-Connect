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

The port's other own-process tests (OctoPrint's event manager is global and ends with `SHUTDOWN`, so each runs in a fresh
process and is `#[ignore]` in the unit suite): `scripts/octoprint-own-process-tests.sh` runs `the_top_edge_on_a_fake_printer`
and `k4_an_unplugged_cable_is_gone`. `cargo test --workspace` runs everything else.

- **The scenarios** are in `crates/engine/tests/octoprint/scenarios.json` (with their G-code files in `files/`),
  read by both sides: connect, controls, typed G-code, prints, a resend, an `M112`, a prompt, the card. 22 of
  them were frozen before our first run (sha256 of the file then: `202c774bd75b53239282ca613993fa4444f5227ab665a1df7b56eb1cc3b29263`);
  none was dropped or reworded after. Two were added after our first run (`added_after_first_run`:
  `sd_print_after_list`, `sd_delete_after_list`), because OctoPrint's own API (`files.py:505`, `_verifyFileExists`)
  only selects or deletes a file it has listed, by its DOS name, which `sd_print` and `sd_delete` do not do:
  both sides refuse those two and send nothing (the scenario then waits for its line and ends with the same
  timeout on both sides).
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
24 scenarios, each run 5 times on our side and 5 times on the reference's side (CPython 3.13.12): **22 identical**, 0 differ, 0 vary from run to run, 2 not reproducible (reference).

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

## 3. Not covered

- **The M2 actions** (load and unload filament, probe the bed, the laser actions, frame): OctoPrint has no such control and
  the port answers a failed `CommandDone`, "OctoPrint has no such control", sending nothing (M2;
  `the_top_edge_on_a_fake_printer` asserts it). Nothing is on the wire, so there is nothing to compare.
- **Everything that is not bytes to and lines from the printer:** the report (`Telemetry`, the M3 states, `problem`, progress,
  the prompt as Attention), `CommandDone` and its output (M4, K11, K13), `JobEnd`, `Gone`/`Back` (K4), the refusals' texts
  (K2, K8). They are asserted by the port's own tests (`port.rs`), not compared with OctoPrint here.
- **A real serial port.** Both sides talk to the virtual printer through an object, not a device: pyserial's
  `Serial.read`/`write` and the `BufferedReadlineWrapper` (`serial.rs`, the `serialport` crate, `comm.py`'s workarounds
  at :3900-3927) are not exercised. The real `/dev` scan and the baud-rate loop beyond its first candidate are not either
  (`connect_auto` finds one stand-in device and the first baud rate works).
- **A real Marlin printer:** not tried (D11).
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
