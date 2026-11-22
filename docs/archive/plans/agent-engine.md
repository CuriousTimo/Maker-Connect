# Maker Connect agent: design

Status: **v3, the owner's plan** (2026-11-12). Steps 1 and 2 may start; the Marlin port waits for D2.

> **2026-11-18 (owner):** the hub core is being redesigned first ([hub-core-v2.md](hub-core-v2.md), after
> [prusa-connect-study.md](prusa-connect-study.md) §7). Until that design is reviewed, no new phases here start.
> The Marlin/OctoPrint port is frozen as is (no new audit rows); the MK3 PrusaLink port (#4) follows PrusaLink's `websockets` branch (2026-11-19, [decision](../../design/decisions/20261119-mk3-reference-prusalink-websockets.md)).

## Owner's decisions on the shape (2026-11-12)

- **The hub does no G-code.** The agent does: each port makes the bytes from its reference. The hub
  sends typed commands (jog, home, extrude, heat, fan, speed, flow, load, unload, SD list/print/
  delete, a job, pause/resume/stop, a prompt answer). G-code a user types in is passed through as
  typed.
- **Devices that talk to the hub directly** (the MINI over Prusa Connect, Klipper over Moonraker)
  get an **agent inside the hub**: the same engine and the same kind of ports, run in the hub's
  process instead of over the network.
- So **the engine and the ports are one library** (`crates/engine`). The agent binary uses it with
  the hub link over the network; the hub uses it internally for its network devices.

```
 hub ── typed commands / reports ──┬── agent (binary): hub link ─ engine ─ ports ─ serial/USB devices
                                   └── agent (inside the hub):    engine ─ ports ─ network devices (MINI, Klipper)
```

- **`--mode prusa|marlin|grbl`** tells the agent what is on its serial port, never guessed (owner,
  earlier: "we geven gewoon een --mode prusa of --mode marlin mee bij opstarten", and grbl for
  lasers). The ports read it (step 3).

## The plan (the owner's order)

| Step | What | Reference |
|---|---|---|
| **1. Engine** | Manages the machines: which there are, their state, their jobs, the commands for them, and the report for the hub. Knows **nothing** about G-code or devices. | none: **ours**, every part approved by the owner |
| **2. Hub link** | Talks only to the engine and the hub: register, report, fetch commands and files, updates. Written anew; the old `hub.rs` goes. | none: **ours** |
| **3. Ports** | One per device, each a whole port of one reference: Marlin (OctoPrint), MK3 (PrusaLink), PnP (labelle), LabelWriter 450 (DYMO's driver), laser (a GRBL host). Each is tied to the engine by a small, written mapping the owner approves. | one per port |

Order (owner, latest 2026-11-15): engine → hub link → Marlin (OctoPrint) → laser (LaserFoundry) → MK3 → hub side; the DYMO PnP done in parallel, the LabelWriter 450 in parallel now ("na marlin doen we eerst GRBL en als de 450 cups agent mee valt dan kan die nu al") ("onze agent iig al afmaken" before the hub side). The `on-track` watchdog checks each
step. The old engine (`marlin.rs`, `print.rs`, `lcd.rs`, `recovery.rs`, the device parts of
`hub.rs`) goes: nobody uses it yet.

## Step 1: the engine

**Its words are the existing agent–hub protocol** (`crates/protocol/src/lib.rs`), not a new model:

| The engine has | From the protocol |
|---|---|
| A machine's state | `MachineState`: Disconnected, Idle, Busy, Paused, Attention, Error |
| A machine's report | `Telemetry` (whatever its port fills in; the rest stays empty) |
| A job to do | `JobOrder` (with its downloaded file) |
| Pause, resume, stop | `JobControl` |
| A job's end | `JobEnd` |
| A command and its result | `MachineCommand` → `CommandDone` |
| A question's answer | the prompt button chosen |

**What the engine does** (all ours):

1. **Holds the machines**: one per port, each running on its own thread.
2. **Passes on**: a command, a job, a control or an answer goes to the right machine's port, one at
   a time in the order received.
3. **Collects**: a port tells the engine its current `Telemetry`, and each `CommandDone` and
   `JobEnd`. The engine keeps the latest `Telemetry`, and **every** `CommandDone` and `JobEnd` in
   order until the hub link has reported it (`Telemetry.done` is a list). The `Telemetry` fields are
   passed on untouched: the engine does not read or change what a port fills in.
4. **Knows the job**: which job a machine has (`JobOrder.id`), until its `JobEnd`.
5. **Is told when a machine goes away or comes back**: the port says so. Whether, when and how often
   to open it again is the port's, as its reference does it. The engine retries nothing.

**What the engine does not do:** decide anything about a device. When to poll, what to send, how to
pause, what a state means: all in the port.

**The port's side of it** (the interface each port implements; ours):

| A port gets | A port gives |
|---|---|
| `open` (its settings: port, baud, …) | its `Telemetry`, whenever it changes |
| a `MachineCommand` | a `CommandDone` for it |
| a `JobOrder` + the file | a `JobEnd` for it |
| a `JobControl` | |
| a prompt answer | |
| `close` | that it went away |

### Ours, approved by the owner 2026-11-12 ("akkoord")

| # | What | Proposal |
|---|---|---|
| O1 | One machine per port, each on its own thread | yes |
| O2 | Commands, jobs, controls and answers go to a machine one at a time, in the order the hub sent them | yes |
| O3 | Every `CommandDone` and `JobEnd` kept until reported | yes |
| O4 | The hub link reports every 2 s, as today (`REPORT_EVERY`) | keep |
| O5 | When the hub cannot be reached: try again every few seconds, as today | keep |
| O6 | A job's file is downloaded whole before the job is handed on, at most 512 MB, as today | keep |
| O7 | Registering, credentials and agent updates as today | keep |
| O8 | The typed commands: exactly the controls the hub's pages offer today (jog, home, extrude, heat, fan, speed, flow, motors off, load, unload, SD list/print/delete, typed G-code, job, pause/resume/stop, prompt answer, laser jog/frame/settings, print label), nothing added | yes |

## Step 2: the hub link (ours, new)

Registers once (credentials kept as today), then in a loop:
- reports each machine's latest `Telemetry` (with any `CommandDone` and `JobEnd`) to the hub;
- takes from the hub's reply the commands, jobs and controls, and gives them to the engine;
- downloads a job's file before handing the job on;
- takes agent updates as today.

It never talks to a device. It carries the typed commands (owner's decision above) to the engine.

### Step 2, how (approved by the owner 2026-11-12: "Akkoord")

Owner's answers to the watchdog's points: (1) the hub-link functions are moved unchanged, with
their values of today (report every 2 s, retry every few seconds, "still not reached" after 60 s,
download progress every 0.5 s, update trial 180 s); `fetch_label`/`print_label` go with the DYMO
port, the device reconnect every 10 s goes. (2) The SD tab says "not available yet" until a port
answers `SdList`. (3) The hub sends `Action`s in step 2. (4) The hub's tests of its G-code controls
and SD go with that code.

1. **Protocol**: `MachineCommand` carries an `Action` (step 1) instead of G-code text.
2. **The agent's hub link, new file `crates/agent/src/link.rs`**: what is ours today and approved as
   "as today" (O4–O7) is **moved, unchanged**, out of the old `hub.rs`:
   - the settings (`options`, `config.rs`);
   - registering and the credentials (`register`, `kept`, `show_code`);
   - the HTTP link with its retries (`HubLink`);
   - the agent info and updates (`agent_info`, `outcome`, `own_file`, `self_update`, `may_update`);
   - the download (`fetch`), the stop on SIGTERM.

   New in it: the loop that takes each machine's `Report` from the engine, sends it (`done`,
   `job_end`, and Disconnected when not connected), and gives the hub's commands, jobs, controls
   and answers to the engine. A job's file is downloaded first (O6).
3. **No ports yet**: the agent registers and reports its machine as Disconnected until step 3.
4. **The old engine goes**:
   - in the agent: the device logic in `hub.rs`, and `marlin.rs`, `print.rs`, `lcd.rs`,
     `recovery.rs`;
   - the `stream` command in `main.rs`, which streams through `marlin.rs`.
5. **The hub sends `Action`s**: its pages' controls make an `Action` instead of G-code. `jog_command`,
   `prusalink_control`, `octoprint_control`, `action()`, the load/unload G-code and `sd_card.rs`'s
   own `M20` and `M23`/`M24` go. What an SD listing looks like coming back is the port's (step 3).

Done when: hub and agent build; the agent registers, reports Disconnected, takes a command and a job
from the hub and hands them to the engine; the hub's tests and the agent's link tests are green.

### Taken out of the hub in step 2, for the MK3 port (to approve then)

PrusaLink behaviour the hub did itself, as G-code, on events of its own: `M72 S1` and `M72 S0` when
a print is taken off the bed or a printer says it is not ready (PrusaLink's `SetReady`,
`CancelReady`), and `M300 P200 S600` + `M117 ~Cannot re-print` when a reprint cannot start
(PrusaLink's `RePrint`). These are not page controls (O8), so they need a typed message of their
own (for example "ready", "not ready", "cannot reprint") when the MK3 port is built. Until then
they are not sent.

### Ours in step 2 (approved by the owner 2026-11-12: S1–S4 "ok", S5 "aan de ports laten")

| # | What | As built |
|---|---|---|
| S1 | A job's file is kept as `job-<id>` beside the state file, removed once its end reached the hub | yes |
| S2 | Without a port: a command is answered "this machine has no port yet" (failed); a job ends failed with that reason, or "its port has ended" | yes |
| S3 | One job end a report (the protocol has one `job_end`); the next ones in the reports after | forced by the protocol |
| S4 | The download's progress is not reported: it went to the MK3's screen (the port's, step 3) | dropped |
| S5 | The hub's own refusals for agent machines while paused (no homing, no Z, no extruding under 170 °C) are not applied any more: the port refuses as its reference does | **decided (owner, 2026-11-12): left to the ports** ("aan de ports laten") |

## Step 3: the ports

Each its reference, whole. Before each port: its file list, and its mapping to the engine (its
states to `MachineState`, its results to `CommandDone` and `JobEnd`), written down and approved.


### Step 3a: the Marlin port, from OctoPrint 1.11.8 (approved by the owner 2026-11-12: M1–M5 and the way of building, "akkoord")

Reference: OctoPrint **1.11.8**, its latest release (owner's D2), commit `42be740`, AGPL-3.0 (like
ours), in `reference/OctoPrint-1.11.8`. Every ported file names its source and keeps OctoPrint's
copyright and licence.

**Where:** `crates/engine/src/ports/octoprint/`, one Rust file per OctoPrint file. Functions and
classes keep their names (`MachineCom`, `_monitor`, `_send_loop`, …) so each can be set against its
source.

**What is ported** (checked by the watchdog against 1.11.8):

| OctoPrint 1.11.8 | What it does on the wire |
|---|---|
| `util/comm.py` (7,056), whole | everything between OctoPrint and the printer |
| `printer/standard.py`, the parts that drive `comm.py`: `connect`, `commands`, `script`, `jog` (:510), `home` (:553), `extrude` (:575), `change_tool`, `set_temperature` (:606), `_convert_rate_value` (:668), `feed_rate` (:682), `flow_rate` (:689), `select_file` (:696), `unselect_file`, `start_print` (:765), `pause_print`, `resume_print`, `cancel_print`, the SD wrappers (`add_sd_file`, `delete_sd_file`, `init_sd_card`, `release_sd_card`, `refresh_sd_files`), the callbacks from `MachineCom`, the input checks | the bytes of each control; a job is `select_file` then `start_print` |
| `server/api/printer.py`, its refusals | not operational, or printing (all but the feed rate): refused (S5: the ports refuse as their reference) |
| `schema/config/serial.py`, `scripts.py`, `feature.py`, `printer_parameters.py`, `printer_profiles.py`, `temperature.py` (the defaults, pydantic models) | timeouts, intervals, the G-code scripts and their defaults (cancel: `M84`, heaters off by snippets, `M106 S0`) |
| `settings/__init__.py`: `loadScript` (:2115), `_get_script_template` (:924), its `SnippetExtension` | rendering the G-code scripts (Jinja2 with `{% snippet %}`) |
| `printer/profile.py`: the default profile and what `comm.py` asks of the profile manager | jog and extrude speeds, which heaters there are |
| `util/__init__.py`: `RepeatedTimer`, `ResettableTimer`, `CountedEvent`, `PrependableQueue`, `TypedQueue`, and what else `comm.py` imports | their own timing |
| `util/files.py` (`m20_timestamp_to_unix_timestamp`), `util/platform.py` (`get_os`, `set_close_exec`), `filemanager.valid_file_type` | SD listing dates, the port, file types |
| `events.py`: the events `comm.py` fires (18 kinds, and its `M226`…`M702` table) | which commands raise which event; the port tells the engine from them |
| The plugin hook points `comm.py` calls (14: transport, `gcode.queuing`/`queued`/`sending`/`sent`/`received`/`error`, `atcommand.*`, `firmware.*`, `action`, `scripts`, `temperatures.received`) | ported as hook points; the only plugin hooked into them is the one below |
| `plugins/action_command_prompt/` (302) | `M876 P1` (enabled "detected", :100), the answers to a question on the screen |

Not wire, not ported: the web pages and the rest of the API, the file
manager (the job's file is the one the hub link downloaded), `systemcommands` (a debug action),
the other plugins: `action_command_notification` only reads, `achievements` and `pluginmanager`
send nothing, and `eventmanager` and `customcontrolmanager` send only what is configured (empty by
default).

**Tests, the reference's own:** `tests/util/test_comm.py`, `tests/util/test_comm_helpers.py`, and
OctoPrint's own test printer `plugins/virtual_printer/virtual.py` as the test device.

**Ours, to approve before building:**

| # | What | Proposal |
|---|---|---|
| M1 | `Action` → OctoPrint: Home → `home`, Jog → `jog`, Extrude → `extrude`, Temperature → `set_temperature`, Fan → `commands("M106 S255"/"M106 S0")` and MotorsOff → `commands("M18")` as OctoPrint's control panel sends them (`templates/tabs/control.jinja2:149-151`), Speed/Flow → `feed_rate`/`flow_rate`, Gcode → `commands`, a job → `select_file` then `start_print`, pause/resume/stop → `pause_print`/`resume_print`/`cancel_print`, an answer → the prompt plugin, SdList → `refresh_sd_files`, SdPrint → `select_file(sd=True)` then `start_print`, SdDelete → `delete_sd_file` | as listed |
| M2 | Load, unload, probe the bed, the laser ones: OctoPrint has no control for them, and the hub makes no G-code (D9) | answered "OctoPrint has no such control" (failed `CommandDone`) |
| M3 | OctoPrint's states → our six | a table, before building |
| M4 | `CommandDone`: OctoPrint answers no command; ours: done once sent and `ok`-ed, the lines between as its output | yes |
| M5 | Rust crates for what Python had: `serialport` (pyserial; it differs in parity and DTR/RTS handling, so `comm.py`'s workarounds at :3900-3927 are kept), `minijinja` (Jinja2, with OctoPrint's `snippet` extension ported), `fancy-regex` (Python `re`, lookaheads included) | yes |

**M3, the state table (approved by the owner 2026-11-12, "akkord"):** `MachineCom`'s states
(`comm.py:445-460`) to `MachineState`, and its own text (`getStateString`, `comm.py:968`) as the
report's `problem` where it is not plain:

| OctoPrint (`comm.py`) | Ours |
|---|---|
| `STATE_NONE`, `STATE_OPEN_SERIAL`, `STATE_DETECT_SERIAL`, `STATE_CONNECTING`, `STATE_CLOSED` | Disconnected |
| `STATE_OPERATIONAL` | Idle |
| `STATE_STARTING`, `STATE_PRINTING`, `STATE_PAUSING`, `STATE_RESUMING`, `STATE_FINISHING`, `STATE_CANCELLING`, `STATE_TRANSFERING_FILE` | Busy |
| `STATE_PAUSED` | Paused |
| `STATE_ERROR`, `STATE_CLOSED_WITH_ERROR` | Error |
| (no state) a question open on its screen (the prompt plugin's `action:prompt_show`) | Attention, its text and buttons as the `prompt` |

**Second source, approved (owner, 2026-11-12, "ok"):** the parts of Python's standard library
OctoPrint's classes stand on (`queue.py`, `threading.py`), ported from **CPython 3.13** (the
newest Python OctoPrint 1.11.8 runs on).

**Approved stand-ins (owner, 2026-11-12, "akkoord"):** a setting `comm.py` reads, until the
settings are ported, takes its default from `schema/config/` (first: `supportFAsCommand`,
`serial.py:227`, false).
That stand-in is gone: the settings getters are ported (`ports/octoprint/settings.rs`), with all 78
`settings().get*` paths of `comm.py` and `standard.py` checked against OctoPrint's defaults.

**Approved as ours (owner, 2026-11-12, "ok"):** in the settings getters, Python's uncaught
`TypeError`/`OverflowError` (`getInt`/`getFloat` on a list, dict or infinity) are panics, and
Python's unbounded `int` is `i64`. Neither is reachable with OctoPrint's defaults.

**Approved as ours for `MachineCom` (owner, 2026-11-12, "ok"):** (1) each Python attribute
assigned after construction is its own field behind `Attr<T>` (`py_attr.rs`), since a Python
attribute read or write is atomic on its own; OctoPrint's own locks are the `py_threading.rs` ports,
no others; (2) `plugin_manager().get_hooks` returns empty maps until the prompt plugin is ported (Section J: `action_command_prompt.rs` registers its three hooks);
(3) `eventManager().fire` goes to a sink the port's top edge implements (the bus is not ported);
(4) the printer profile manager was a trait object until `printer/profile.py` was ported (Section J: now `profile.rs`, `PrinterProfileManager`, without the profile folder);
(5) methods later in `comm.py` are `unimplemented!("comm.py:LINE")` until their section, and
nothing calls them before. Logging (`_logger`, `_serialLogger`) is comments; `_log` is ported.
Also approved (owner, 2026-11-12, "ok"): the helper types of the `MachineCom` port: `PyDeque`
(`deque(maxlen)`) and `PyDict` (ordered `dict`); fixed shapes for the serial transport, the printing
file and the queue items where Python uses duck typing and tuples; `None` where Python stores
`False` for "nothing"; `__del__` as `Drop`. `threading.Timer` is the CPython 3.13 port
(`threading.py:1321`).

**Section F, approved (owner, 2026-11-13, "ok"):** a non-ASCII name on an SD upload
(`startFileTransfer` → `get_dos_filename`, without `LFN_WRITE`) is a known gap: OctoPrint uses
unidecode, emoji and pathvalidate there, which are not ported; the hub never uploads to the SD card.
Also ours: `py_float_repr` for a log line; hooks called without the comm instance (no plugins yet);
`Option` in callback arguments; the print file read line by line (same lines and positions).

**Threads:** `comm.py`'s own, as they are: its monitor thread, its send thread, its timers.

**How it is built:** `comm.py` from top to bottom, in sections of about 1000 lines (sizes decided (proxy) 2026-11-15). A
cheaper agent (Sonnet) translates a section; I check it against its source, mechanically where it
can be (its regexes, constants, G-code texts and intervals extracted from the source and compared,
not by eye), and re-read the rest line by line; the watchdog checks each section.

**Done when:** the ported OctoPrint tests pass; the ported virtual printer prints a file through
the port, pauses, resumes and cancels it; then it is tried on a real Marlin printer (D11).

## Decisions

| # | Decision | Blocks |
|---|---|---|
| ~~D1~~ | **Decided (owner, 2026-11-12): port PrusaLink (and the Connect SDK pieces it needs).** Noted: GitHub shows no licence for either, `setup.py` says "Freeware"; the code is public. Named as the source in every ported file. | — |
| ~~D2~~ | **Decided (owner, 2026-11-12, "ok" to the proposal): OctoPrint's latest release (1.11.x)**, not the `dev` branch. | — |
| ~~D4~~ | **Decided (owner, 2026-11-15):** LaserFoundry (`laserfoundry-device`, `-gcode`), see "D4 decided" below. | the laser port |
| D6 | The LabelWriter 450's reference (DYMO's CUPS driver?). | the 450 port |
| ~~D9~~ | **Decided (owner): the hub does no G-code.** The protocol gets typed commands (part of step 1); the hub's G-code builders (`hub/src/main.rs`: jog, home, extrude, load; `sd_card.rs`) go. | — |
| D8 | `CLAUDE.md`'s audit rule: replace by "the reference's own tests, ported, plus trying it on the device"? | — |

The reviews of v1 and v2 (findings on OctoPrint's `standard.py`, PrusaLink's full file list,
threads, the hub's own G-code) stay valid for step 3 and are taken up per port.

**Delegation (owner, 2026-11-15):** "Ga door en maak af: OctoPrint-port, Dymo, Hub kant. Gebruik
een goedkope ai om mijn rol vandaag te vervangen." Approval questions of that day are decided by
the `owner-proxy` agent (`.claude/agents/owner-proxy.md`, Sonnet, by the owner's recorded rules).
Each such approval below is marked **(proxy)** for the owner to review. The proxy may not approve
commits, releases, deploys, hardware actions, deletions, security changes, or an open choice of
reference.

**Section G, decided (proxy, 2026-11-15):** approved: `py_try` for `except Exception` (re-raises
stub panics; our own bugs take the loud crash path); `Flow` and the two closures in `_monitor`;
hooks with positional JSON (revisit with `plugin.rs`); controls/pauseTriggers from JSON;
`convert_line` None panics; `SerialTransport::str()` (Python's repr holds a memory address, cannot be
reproduced) and `set_timeout()`; inert `PositionRecord.valid` / `_firmware_name`; `size()` /
`set_size()` on the file trait; `_known_invalid_tools` as strings; `filter_non_utf8` false for a
Rust `str`; `last_position.t` f64 and `int(float)` as `as i64`; sections of ~1000 lines.
To fix: non-array controls/pauseTriggers must panic, not become empty; `parse_firmware_line` keeps
the firmware line's key order; the shutdown error log carries the panic text.

**Label printers, two separate stacks (owner, 2026-11-15):** the DYMO LabelManager PnP (tape) and
the DYMO LabelWriter 450 (labels) are completely different kinds of printer. Each gets its own
port of its own reference (PnP: labelle; 450: to decide, D6), its own files and its own tests; no
shared code between them. They come after the OctoPrint port, one at a time.

## The nine design decisions (2026-11-15)

The owner: "los die negen beslispunten op en laat als nodig weer testen door de agents die daar
verantwoordelijk voor zijn". Decided by Claude on the owner's request, checked by `owner-proxy`
and `on-track`; marked **(Claude)** for the owner to review. Source: `docs/archive/design-overview-v1.html`,
"Te beslissen".

| # | Question | Decision (Claude) | When |
|---|---|---|---|
| B1 | Order | The owner's latest word: **OctoPrint port → DYMO PnP → hub side**. MK3 (PrusaLink), LabelWriter 450 and the laser stay in the plan, after these, in that order. The Spoolman audit waits for the hub side. | now |
| B2 | Download | The hub link fetches a job file as bytes, streamed straight to `job-<id>` (no `String`, not all in memory), on its own thread, while the 2 s reports go on; the job goes to the engine when the file is complete. Ours (link). | first item of the DYMO step (a PNG cannot print without it) |
| B3 | Hub filters for agent machines | The hub stops refusing or holding commands for agent machines (S5: the port refuses as its reference does; OctoPrint refuses through `server/api/printer.py`, ported). The hub page may still hide or grey buttons (UI). Hub-side machines (MINI, Klipper) keep theirs until the hub side. | with the port's top edge (end of the OctoPrint port) |
| B4 | No silent default | `--mode` is required; no fallback to `marlin`, no default kind. Values: `prusa`, `marlin`, `grbl`, `dymo-pnp`, `labelwriter-450`. | with the port's top edge |
| B5 | Home | As each reference has it. `Action::Home` gets the axes (`Home { axes }`, as OctoPrint's API `home: ["axes"]`, `server/api/printer.py:297`; this widens O8's `Home`); for OctoPrint machines the page shows OctoPrint's two buttons, XY and Z (`control.jinja2:71,85`), sent as `home(["x","y"])` / `home(["z"])`. | with the port's top edge |
| B6 | `job_id` | The hub link fills `Telemetry.job_id` from the engine's `Report.job`. Ours (link). | with the port's top edge |
| B7 | Emergency stop | **No new action** (after the proxy's check: a new action would change O2 and O8). OctoPrint's own path is typed G-code: `M112` (and `M108`, `M410`) sent with `commands()` is forced ahead by `sendCommand` (`comm.py:1297`, `_emergency_commands` from `serial.emergencyCommands`, `schema/config/serial.py:155`). With B3 the hub no longer holds typed G-code for agent machines, so it reaches the port. The page may offer a button that sends `M112` as typed G-code (UI). | with the port's top edge |
| B8 | Label crate | The two DYMO printers get separate ports in `crates/engine/src/ports/` (PnP: `labelle/`; 450: its own later). `crates/label` loses its device code; the hub takes each printer's geometry from its port's ported constants, so hub and port cannot disagree. | DYMO step (PnP), later (450) |
| B9 | Open references | D6 LabelWriter 450: **DYMO's own CUPS driver** (`dymo-cups-drivers`, the maker's software; not `thermal-label/labelwriter`, a source in the earlier fusing). D4 laser (since decided by the owner: LaserFoundry) and the Klipper G-code: **left open for the owner** (the proxy found the owner's earlier wish for LaserFoundry, and Mainsail is a Moonraker client, not Klipper itself); both are far off. D8: proposal for the owner (it changes `CLAUDE.md`): proof for a port = an audit made by running the reference and ours against the reference's own test device (OctoPrint: `virtual_printer`), byte for byte, plus D11, the owner tries it on a real printer. | at each one's turn |

**Checked (2026-11-15):** `owner-proxy` approved B1–B6, B8, D6 and Section H; B7 revised as above; D4, Klipper and D8 left to the owner. `on-track` read the newer `reference/OctoPrint` by mistake for its line checks (corrected in its file); the 1.11.8 lines cited here are right.

**Section H (Claude, 2026-11-15; extends M5, for the owner to review):** pyserial 3.5 (OctoPrint's pinned dependency, `setup.py:67`; the
copy in `reference/.venv-octoprint/.../site-packages/serial/`) is part of the reference's runtime,
like CPython: its `Serial.read`, `Timeout` and `cancel_read` are ported beside the `serialport`
crate (extends M5). `py_format.rs` (written from the docs) must be compared with real Python's
`str.format` on the same cases, and match. Tests get a structural guard: under `cfg(test)`,
`serial_list()` scans a temp directory and `_open_serial` panics on a real device path.

**Owner, 2026-11-15:** the DYMO PnP may run in parallel with section I ("maar dat compileren is
toch af te stemmen"). The two agents keep the engine crate compiling between edit batches and
build in separate target directories. Only the PnP; the LabelWriter 450 and the hub side still wait.

**Section I (Claude, 2026-11-15; extends Section H, for the owner to review):** comm.py:4509-5688 (the send
path, the gcode / @command / command-phase handlers, emergency stop) is ported. Its writing side extends
Section H: pyserial 3.5 `Serial.write` (`serialposix.py:612`, with `Timeout`, :618, and `write_timeout`) is ported
beside `read` (same reference, ours). `py_format.rs` is now a port of CPython 3.13's `unicode_format.h` and
`formatter_unicode.c` and is compared with real `str.format` by `scripts/octoprint-format-check.py` (a fixture,
byte for byte). The test guard holds: under `cfg(test)`, `serial_list()` scans a temp directory and
`_open_serial` panics on any path outside it (except `/dev/maker-connect-test-does-not-exist`).
Differences 28-32 in `comm.rs` list what is ours. New: `__del__` (Drop) has no `Arc`, so it does not send the
`beforePrinterDisconnected` script (a script with lines is `unimplemented!`).

**Section I, decided (proxy, 2026-11-15):** approved: differences 28–32 (Arc signatures, the
unreachable `QueueMarker` branch, `_send_loop` via `py_try`/`LoopStep`, `hasattr` as a `match`,
hooks with positional JSON, pyserial `Serial.write`/`write_timeout`, `O_NONBLOCK` as pyserial
`serialposix.py:322`, `encode_serial_text` for ascii/utf-8/latin-1); `Drop` cannot send a
non-empty `beforePrinterDisconnected` script (empty by default; panics otherwise); `py_format.rs`
ported from CPython 3.13's C sources (now in `reference/cpython-3.13/`), identical on 18,329 calls,
192 attribute-of-str cases `unimplemented!` (user templates only). Fixed by Claude:
`last_fanspeed` is an integer as in Python. To fix: `tags=None` must stay `None` (not an empty
set) through `_enqueue_for_sending` and `_send_loop`, as comm.py:4631/4717/4780.

**Section J, plan additions (proxy + watchdog, 2026-11-15):** besides the listed `standard.py`
parts, also `disconnect` 424, the state getters 843–941, `__init__` 52 (needed by M3/M4 and the
callbacks) and the wire-acting `on_comm_*` callbacks (1443–1915); `set_temperature_offset` is not
ported.

**Owner, 2026-11-15:** the PnP's idle status poll (the 2026-11-11 decision) goes: "idle poll on the
PnP -> follow the example" (labelle sends nothing while idle). Superseded by the owner, 2026-11-16
(below, and L11): the poll comes back.

**Section J, decided (proxy, 2026-11-15):** approved: `serde_json::Value` arguments in
`standard.rs`, `LocalFileManager` (a stand-in; only event payloads differ), `Arc<Printer>` with a
`Weak` self, `Arc` hook handlers (difference 34), plugin settings = defaults, the prompt answer as a
string for a plain `{choice}`. To fix (brief `docs/archive/briefs/section-j-fixes.md`): the `%`-formatting
is ported from CPython 3.13 and proven; OctoPrint's `EventManager` queue and worker are ported.

### Step 3b: the DYMO PnP (2026-11-15; ours below: to approve (proxy))

Reference: **labelle** at commit `5e8f1e5` (Apache-2.0, `reference/labelle`), whole print path of
its command line `labelle --picture FILE --tape-size-mm N`, ported file by file and function by
function in `crates/engine/src/ports/labelle/` (`mod.rs` lists them). labelle's runtime, as the
owner approved it for CPython and pyserial: **pyusb 1.3.1** (`pyusb.rs`: `usb/core.py`,
`util.py`, `control.py`) and **Pillow 12.3.0** (`pillow/`, from its C source in
`reference/pillow-12.3.0`: `Image.open`'s unpackers, `resize`, `convert`, `paste`, `transpose`,
`tobytes`, the invert and Floyd-Steinberg dithering). Not ported: `online_device_manager.py`,
anything GUI, the other render engines, the old `status_session`. Proof and "not covered":
`docs/audits/dymo-labelmanager.md` (`scripts/labelle-port-check.py`: 52 of 52 identical).

**What is ours** (stand-ins and the top edge):

| # | What | Proposal |
|---|---|---|
| L1 | The port reports Idle from the start, before any print (labelle sends nothing while idle; the printer is looked for when a job prints; since 2026-11-16 the idle poll of L11 also looks for it) | approved (proxy, 2026-11-15); amended by L11 |
| L2 | The tape width is the job's `tape_mm`, passed as `--tape-size-mm`, never taken from the PNG. As `DymoLabeler.__init__` does (`dymo_labeler.py:246-258`), none is `DEFAULT_TAPE_SIZE_MM` (12), then a size labelle does not support fails the job with labelle's `ValueError` text (`Unsupported tape size 24mm. Supported sizes: (19, 12, 9, 6)`). As in `cli.py`, it is checked after the printer is found and set up; the printer is then released | approved (proxy, 2026-11-15) |
| L3 | `Order::Job { order, file }` (a PNG): labelle's picture render and print, then `Event::JobEnd` done, or failed with the message of labelle's error (`No supported devices found`, `Could not open a valid interface`, `Print error: [Errno 110] Operation timed out`, ...) | approved (proxy, 2026-11-15) |
| L4 | `Order::Command` with any `Action`: a failed `CommandDone`, `labelle has no such control` | approved (proxy, 2026-11-15) |
| L5 | `Order::Control` and `Order::Answer`: labelle has none; they arrive one at a time after the print (O2), so no job runs then: a report with `problem` set, `labelle has no pause, resume or stop: a label prints in one go` / `labelle asks no question to answer` | approved (proxy, 2026-11-15) |
| L6 | **Overruled by the owner, 2026-11-16 (see the last paragraph): the status bits are read too, from every status answer of a print and of the idle poll (L11): the cassette is in when bit `0x40` of byte 0 is set (`Telemetry.cassette`), the battery is byte 3 when it is at most 100 (`Telemetry.battery_percent`, the last good value kept); `raw_status` stays alongside.** The earlier text: The status answers a print reads (labelle reads and throws them away) are reported as the first eight bytes in hex (`raw_status`), their meaning unknown: no cassette, no charge, nothing derived. A hook (`heard`) hands the answers to the port; no byte is sent for it | approved (proxy, 2026-11-15); the "raw only" part overruled by the owner, 2026-11-16 |
| L7 | `Gone` when a print finds no printer (`DeviceManagerNoDevices`, or none of labelle's products), `Back` when a print finds it again after that. No scan loop. The idle poll of L11 (owner, 2026-11-16) also reports them, through the same one flag: an event only on a change, so a poll or a job that finds the printer after a `Gone` sends `Back` | approved (proxy, 2026-11-15); amended by L11 |
| L8 | The idle status poll of 2026-11-11 is not ported (owner, 2026-11-15) and `status_session` is removed. **Superseded by the owner, 2026-11-16:** see L11 (the poll is back, as L11, not as `status_session`) | owner's decision; approved (proxy, 2026-11-15); superseded by the owner, 2026-11-16 |
| L9 | The picture's options are `cli.py`'s defaults (justify left, `--margin-px` 56, no minimum, maximum or fixed length): the margins labelle adds (about 16 mm after the picture, and a cut of one column at its start). The hub's own 16 mm trailing blank, drawn into the PNG, comes on top of that: **for the owner, when the hub side comes** | approved (proxy, 2026-11-15) |
| L10 | `png` stands for Pillow's PNG decoder, `nusb` for pyusb's libusb1 backend (`nusb_backend.rs`: descriptors read from an opened device for labelle's vendor, the kernel-driver check from sysfs on Linux, libusb's error texts and errnos); an error after the printer is set up releases it, as labelle's process exit does; its devices are kept in hash order (labelle's `set` order is undefined); a `heard` hook; `Drop` is pyusb's `__del__`. Also ours: the state is Busy while a job prints and Idle after it (a poll does not change the state: it stays Idle, L11), nothing derived from the status answers (a print's or a poll's); `nusb` errors that map to no libusb code become `Input/Output Error` (errno 5), a cancelled transfer is a timeout (errno 110), errnos are Linux's, an OUT transfer returns the bytes asked, `No backend available: <why>` when the USB list fails, the macOS access-denied text names the device by its first line; link.rs's error texts (`the file did not come`, `larger than 512 MB`, `another job's file is still coming`). Not tried on the printer | approved (proxy, 2026-11-15) |
| L11 | **Ours, the owner's exception 2026-11-16 to labelle's nothing-while-idle:** while no job runs, an idle status poll. The owner: "Dus ik zou voor de PNP wel de status update poll doen." The sequence: every 15 s (the first 15 s after start-up, then 15 s after the previous poll ended or a job ended) the printer is opened exactly as the print path does (`DeviceManager.scan`, `find_and_select_device`, `UsbDevice.setup`: the ported functions, `cli::open_printer`), one `ESC A` is written (`1b 41`; `_get_status`, `dymo_labeler.py:211`, on a `DymoLabelerFunctions` with `synwait = None`, labelle's class default, so one write and one 512-byte read, not the second `ESC A` and read that `synwait = 64` adds in `send_command`), its answer is read, the device is released as `print` does (`dispose`). Nothing else is sent. The answer goes to telemetry as `raw_status` (first 8 bytes in hex, meaning unknown, as L6), the state stays Idle. A job takes precedence: no poll starts while a job runs, and a job that arrives while a poll runs waits for its release (the port does one thing at a time). `Gone` when a poll finds no printer, `Back` when a poll or a job finds it again, one flag for both (L7). A poll that fails otherwise (a timeout, an open error) releases as the print path's error path does, fails no job, sends no `Gone`, is not retried (the next one comes on its time). Tests (`ports::labelle::tests::idle_poll` and `port`): the exact bytes, the cadence (a test-only interval), no poll during a job, a job waiting for a poll, Gone/Back, a failed poll; the print proof (52 of 52) is unchanged | owner's exception, 2026-11-16 |

**Wiring (B2, B4 for the PnP):** `--mode dymo-pnp` opens this port (`crates/agent/src/ports.rs`);
the other modes and their default are as they were. B2 is done for all jobs in `link.rs`: a job's
file is fetched as bytes, streamed straight into `job-<id>` on a thread of its own (the 2 s reports
go on), at most 512 MB (a larger file fails the job), and goes to the engine when it is whole.
**Old code (B8):** `crates/label/src/dymo.rs` keeps only `Tape::d1` (the hub's tape widths,
`profiles.rs:227`); the printer code, `status_session`, `crates/label/tests/labelle` and
`scripts/dymo-audit-check.py` are gone. **Open:** the hub should read the tape widths from the
port's constants, but the hub cannot depend on the engine without pulling in the device crates and
the unfinished OctoPrint port: decided nothing.

**Section J fixes, decided (proxy, 2026-11-15):** approved: `py_percent.rs` takes its digits from
Rust's correctly rounded formatting instead of porting `_Py_dg_dtoa` (identical on 48,826 cases
vs CPython 3.13.12); serde_json `float_roundtrip`; the event manager's plugin handlers after the
listeners (`events.py:206`), our sink last in the worker thread in `py_try`; the 4.9 MB fixture.

### Section K: the Marlin port's top edge, and B3-B7 (Claude, 2026-11-15; to approve (proxy))

`crates/engine/src/ports/octoprint/port.rs` is the `Port` for `--mode marlin`: it gives OctoPrint's ported
code its orders (M1, M2) and tells the engine what it reports (M3, M4, telemetry, jobs). Every byte to the
printer still comes from the ported code. Done as decided: M1-M4, B3 (the hub no longer refuses or holds
commands for agent machines; `next_command_for_a_print` and `fits_a_print` are gone), B4 (`--mode` and
`--kind` required, `labelwriter-450` reserved), B5 (`Home { axes }`, two Home buttons for a plain Marlin
machine), B6 (`job_id`), B7 (nothing extra), and no invented baud (`--baud`/`--port` are `Option` into
`connect`). Not done: running OctoPrint's `virtual_printer` through the port (the next step); the port
is not called done.

New items, to approve (proxy); the list is also at the top of `port.rs`:

| # | What |
|---|---|
| K1 | Typed G-code goes as typed, one string, not split: the terminal page's upper-casing is the web page's JavaScript, not ported |
| K2 | `job.py`'s and `files.py`'s refusals (start, restart, pause/resume, cancel, select, the card's select and delete) ported into `api_printer.rs`, with their texts (extends S5) |
| K3 | A job with `from_line` ends failed, "OctoPrint has no such control", nothing sent |
| K4 | `Gone` is comm's state going to `STATE_CLOSED` or `STATE_CLOSED_WITH_ERROR`, once; `Back` is `STATE_OPERATIONAL` after that. OctoPrint connects again only when asked to, and this port does not |
| K5 | Other machines' Home button sends `Home { axes: [x, y, z] }`; the hub's own machines keep their G-code until the hub side. "Marlin mode" on the machine page is the hub's `plain_marlin` (an agent machine whose profile is not Prusa's MK3 family or GRBL) |
| K6 | OctoPrint selects a file by a name with a machinecode extension, and the agent keeps `job-<id>`: the port gives OctoPrint the file under the job's name (a hard link, a copy where that fails) in `job-<id>.d/`, removed at the job's end |
| K7 | SdPrint is `select_file(sd=True)` with `printAfterSelect`, as the files API prints from the card, not a `start_print` straight after (the card's answer to the selection starts it) |
| K8 | A refusal of a pause, resume, stop or answer (no `CommandDone`) is the report's `problem` with OctoPrint's text, until the state changes or the next order comes |
| K9 | `problem` is `getStateString` unless it is `Offline`, `Operational`, `Printing` or `Paused` |
| K10 | Progress: `done`/`total` are the file position and the file's size, `elapsed_s` is `getPrintTime`, no time left (OctoPrint's estimators are not ported); the position is the last `PositionUpdate` with x, y and z; tools are listed when there are several |
| K11 | A command that sent no line is done at once; SdList is done with the card's list (one line per file, its name), at once when no card is ready; a command that is never `ok`-ed (comm closed, `M112`) is given up on the port's failed Done or the hub's agent-gone rule (K18), for agent Marlin machines. Buddy, Klipper and the MK3 family keep the hub's 120 s; the label and laser ports fail pending on close too (K18 (d)) |
| K12 | A `start_print` that starts nothing (OctoPrint answers 204 either way) ends the job failed, "OctoPrint did not start the print" |
| K13 | M4's output is every line received from the `Send:` of the command's line to the `ok` that answers it, the `ok` line included |
| K14 | `Printer::set_top_edge` (`standard.rs`): the `Printer` passes each `MachineComPrintCallback` call to the port's listener, including the ones `standard.py` leaves to its `StateMonitor` |
| K15 | Under `cfg(test)`, `comm.rs`'s `_open_serial` opens a port name a test registered (`TEST_TRANSPORTS`) as that serial object: the tests' fake printer, in memory (a pseudo-terminal does not work on macOS: `serialport` sets the baud with `IOSSIOSPEED`) |
| K16 | Texts and rules of ours, found by the watchdog, listed to approve: a PRINT_FAILED without an error says "OctoPrint: the print failed"; a file that cannot be linked says "could not give OctoPrint the file: <io error>"; a job still held when comm goes to CLOSED, CLOSED_WITH_ERROR or ERROR ends failed with `getErrorString` or the state text (the reference fails only a print that was PRINTING); Attention is not shown while Disconnected; a state `comm.py` does not have is Error; tools are numbered from 1 and `active` is `getCurrentTool`; whole numbers go to OctoPrint as Python ints (`S215`, as the web page's JSON gives them); the state text is `getStateString(state)` at the change; the job tags `source:api`, `api:job` are passed to `select_file` (ignored there as `**kwargs`) |
| K17 | Open, for the owner: the hub's `typed_gcode` (existing, `crates/hub/src/commands.rs`) trims lines, drops blank ones and refuses more than 20 lines, 480 characters or non-ASCII before the port sees typed G-code, so it is ours on the way to the wire; K1 says "as typed". Not changed here. Also `Gone` fires on `Order::Close` too (comm goes CLOSED), not only on an unplug |
| K18 | Ours, engine to hub, nothing on the wire to the device. (a) The Marlin port, on `Gone` and on `Order::Close`, sends `Done { ok: false, reason: "the printer connection closed" }` for every command still pending (with the lines it had received). (b) The hub gives every Queued command of a Marlin agent machine at once (`next_commands_for_agent`: the same order, someone's before the hub's own), with no gate and no 120 s; "a Marlin agent machine" is link `agent`, an FDM kind, `plain_marlin` (K5), and a profile that says Marlin or nothing (not Buddy, Klipper, the MK3 family or GRBL). Such a command is given up only on the port's failed Done or by the agent-gone rule: when the machine was last seen longer ago than `ONLINE_SECONDS` (10 s), checked on its next report before that report is taken in, its Sent and Accepted commands (not Queued) fail with "the agent was gone". (c) A Done that comes after that failure wins, as `command_went` overwrites: the printer's own result replaces the hub's guess (tested, also for a Done in the report that ends the gap). (d) HS4, the other agent ports (`docs/archive/briefs/hs4-ports-fail-pending.md`): on `Order::Close` (and the laser also when its orders end) the DYMO PnP, LabelWriter 450 and laser ports fail every command still pending (a command waiting behind the Close, the 450's queue, the laser's unanswered tickets) with "the connection closed" (ours, approved by the owner 2026-11-17; the laser is not a printer). The hub's rule (b) then covers them: the machines it covers are the Marlin agent machines, label printers and lasers (agent link); Klipper and the MK3 family keep the gate and the 120 s (Buddy: see (f)). (e) Difference K18-1 (the agents' decision of 2026-11-15, for approval): comm's own lines after an emergency stop credit no pending command. `_trigger_emergency_stop` (comm.py:5487-5522, from `_gcode_M112_queuing` :5524 and from the error path :4110-4136) sends `M112`, `N<n> M112`, `M104 T<n> S0` and `M140 S0` through `_do_send_without_checksum` / `_do_increment_and_send_with_checksum` (:5017, :5036-5041): they log `Send: ...`, carry no tags and fire no `gcode.queuing`/`sent` hook, and comm then closes. The edge (`port.rs`) sets `emergency` when a `gcode.queuing` hook sees gcode `M112` (any tags; comm's gcode handler runs right after the hooks of that phase) or when `on_comm_error` says `consequence == "emergency"` (comm calls `_trigger_emergency_stop` right after); until `Gone`/`Back` the lines it receives are credited to nothing (no output line, no `ok`), so an `M109` pending when an `M112` is typed fails with the close, "the printer connection closed", never `ok` (`the_top_edge_on_a_fake_printer`). Crediting inside the port only, nothing on the wire. Other lines comm sends on its own (its temperature polls, say) are not changed by this. (f) A printer on the built-in Prusa Connect (link `prusa-connect`) has no 120 s either (Buddy has no time for a command, `planner.cpp:661`): its command stays pending until Buddy answers it (its `command_id` in telemetry keeps it Accepted; FINISHED, REJECTED or FAILED end it). Only when the printer was not seen for longer than `ONLINE_SECONDS`, checked on its next telemetry before that report is taken in, do its Sent and Accepted commands fail with "the printer was gone" (ours, nothing on the wire; a later answer from Buddy still wins as in (c)). Test `a_command_stays_pending_until_buddy_answers_it` |

**Section K, decided (proxy, 2026-11-15):** K1–K16 approved (K16 item by item; the extra job
failure on close/error and the report texts are listed as numbered differences; the job tags to
`select_file` change nothing). Also ours, approved: the M4 done rule (`acked >= sent`), progress
from `get_file_position` / `get_print_time` with no time left, the job link in `job-<id>.d/`.
K17 and multi-line G-code (Claude, from the proxy's split and OctoPrint's terminal,
`terminal.js:419-452`, where each Enter sends one line and an empty one sends nothing): the hub
keeps its refusals (more than 20 lines, more than 480 characters, non-ASCII) but sends the text
unchanged; the port gives each non-empty line, as typed, to `commands` as a list, as a user
pressing Enter per line would. The terminal page's upper-casing stays unported (K1).

**Owner, 2026-11-15:** no compatibility between versions to keep: "er is letterlijk niemand die er
al gebruik van maakt. dus geen moeilijk gedoe met versies die incompatible zijn." Hub and agent
change together; old agents need not keep working.

**Owner, 2026-11-19:** "geen oude v1 meer. er is toch niets en niemand actief": in v2 no v1 is
kept anywhere, no fallback and no transition. The v1 routes this plan names (`/api/agent/v1/*`, the
Apps API v1 and its extension) go with v2 ([wire.md](../../design/wire.md) replaces the agent link;
[overview.md](../../design/overview.md#rules-every-part-obeys) rule 8).

**D4 decided (owner, 2026-11-15):** the laser keeps LaserFoundry's GRBL (`laserfoundry-device`,
`laserfoundry-gcode`, used by `crates/grbl`; the owner's choice of 2026-11-04): "we kunnen gewoon
door blijven bouwen op basis van de GRBL in laserfoundry". At the laser's turn: `crates/grbl` becomes
the engine port for `--mode grbl`, and the pin moves from 3b9b9fa to LaserFoundry's newest release
(v0.5.0, 61febdb, checked 2026-11-15). Changes since 3b9b9fa in the two crates that touch us:
`69bc705` (#76, `resume_from` after `M5` no longer switches the laser back on: safety, we use
`resume_from`), `5c085ca` (#121, status parse takes MPos over WPos in either order), `70e12eb`
(#64, frame refuses feeds above MAX_FEED), and `sim.rs` (the virtual lasers). Not ours: the finish
position (#114), rotary limits, the machine block and setup wizard.

**Lasers, one stack (owner, 2026-11-15):** LaserFoundry also drives Marlin and Smoothieware lasers
(`laserfoundry-device` `Protocol::Grbl`, which Smoothieware speaks too, and `Protocol::Marlin`).
Every laser goes through LaserFoundry, whatever its firmware; a Marlin *laser* is never driven by
the OctoPrint port, and the two share no code ("die marlin moeten wij wel gescheiden houden van onze
octoprint stuff"). The firmware is told at start-up, never guessed (the mode names follow when the
laser port is built). Note for the owner: LaserFoundry says its Marlin protocol is "written from
Marlin's public G-code index and untested" (`crates/device/src/lib.rs:40-41`).

**LaserFoundry web fork (owner, 2026-11-15, for later):** a fork of LaserFoundry's browser version
only (`apps/laserfoundry-web`, WebAssembly, `https://stitchfoundry.github.io/laserfoundry/`) with
Maker Connect support, sending a drawing to a space through the Apps API (`docs/api/apps-api.md`), as
its "Publish to MakerFoundry" does (LaserFoundry's `laserfoundry/docs/makerfoundry.md`, `crates/ai/src/maker.rs`,
`crates/ui-egui/src/maker_ui.rs`). Licence MIT OR Apache-2.0: keep `LICENSE-MIT`, `LICENSE-APACHE`,
`NOTICE` and the copyright line; mark changed files; its own name (the licence grants no right to
the name "LaserFoundry"). The fork is where "Send to Maker Connect" is built, tried and used first;
once it works it is offered to LaserFoundry as a merge request (the owner: "zodat we het al kunnen
testen en gebruiken voordat we het als merge aan hun aanbieden"). So keep the change small and in
their style, close to their MakerFoundry code. Not started: after the laser port, in the owner's
order.

**Owner, 2026-11-17, the fork's button:** "Send to Maker Connect is onlogisch voor de fork ... door de
hub zelf geserveerd, dat is nl al maker connect. -> Send to [lasernaam]". The fork served by the hub
is already Maker Connect: its button names the laser it sends to ("Send to Ortur Laser Master"), and
what it sends becomes that laser's loaded job (HS7b: one loaded job, no laser queue).

**Owner overrules the watchdog on LaserFoundry (2026-11-15):** "LaserFoundry is inderdaad nieuw en
1 persoon. maar dat zijn wij ook. omdat het al in Rust is kunnen we het niet alleen zo gebruiken,
maar ook zo aan bijdragen. die andere projecten zoals Universal G-code Sender, gSender of LaserGRBL
zijn prima referenties. maar dan wel om direct aan de ontwikkeling van LaserFoundry bij te dragen."
So: LaserFoundry is the laser stack's one reference and library, an owner exception to "no
one-person projects". Where LaserFoundry falls short or is unproven, the fix goes into LaserFoundry
itself (upstream contributions), using a mature sender (Universal G-code Sender, gSender,
LaserGRBL) as the reference for that contribution — never mixed into Maker Connect's own code.
Our `crates/grbl` keeps only the mapping to the engine; the architect's fixes apply (`$I` out, `$$`
after the first status as `ui-egui` does, `ui-egui`'s link logic ported, a timeout fails, no
blocking during `$H`, a laser audit). Marlin and Smoothieware lasers stay closed until tried on a
machine.

**Owner, 2026-11-15:** "wij houden namelijk de GRBL sender strict gescheiden op de agent, van de
webfront end." The LaserFoundry web front end (the fork) never talks to a laser; only the agent's
laser port sends to the machine. The front end designs and sends the drawing to the hub ("Send to
Maker Connect"); the hub queues it; the agent streams it.

### Step 3c: the DYMO LabelWriter 450 (2026-11-15; ours below: to approve (proxy))

Reference: **DYMO's own CUPS driver 1.4.0** (`reference/dymo-cups-drivers-1.4.0`, official tarball from
download.dymo.com, sha256 `c60797e7e986ca329f46e9a6ab1cb6382383952b15685ed69fd91f3c7ed64f71`; GPL-2 "or any
later version", © Sanford L.P., taken as GPL-3 so it combines with our AGPL-3.0; the notice is in every
ported file), the filter `raster2dymolw` for the plain LabelWriter 450 (`ppd/lw450.ppd`), ported file by file
in `crates/engine/src/ports/dymo_cups/` (`mod.rs` lists them). The driver's **runtime is CUPS**
(`reference/cups-2.4.20`, OpenPrinting tag `v2.4.20`, commit `ef80eea`: the raster reader, the PPD option
marking, `cupsParseOptions`, `cupsBackChannelRead` and the `usb` backend `usb-libusb.c`), ported where the
driver uses it (runtime, to approve (proxy)). **Gate (owner: "als de 450 cups agent mee valt"): passed**,
`raster2dymolw` and the driver's 20 tests build on this Mac (clang, macOS CUPS headers, cppunit from
Homebrew) and pass. The LabelWriter 450 and the PnP share no code. Not ported (not covered): the Turbo, DUO
and Twin Turbo variants (`lw450t`, `lw450dl`, `lw450dt`, `lw450tt`), the LabelManager driver (`src/lm/`),
the 300/310/315/4XL/SE450/400 models (`ProcessPPDOptions` model names), `CLabelWriterDriverTwinTurbo`.

**What the filter gets and gives** (`CupsFilter.h`, `CupsFilterLabelWriter.cpp`): argv `job-id user title
copies options [file]`; env `PPD`, `DEVICE_URI` (the monitor skips the status unless `usb://`); page header
fields read: `cupsBytesPerLine`, `cupsBitsPerPixel`, `cupsHeight`, `cupsMediaType` (0 or 1), `PageSize[1]`,
`HWResolution[1]`, `cupsInteger[0]`; `cupsWidth` is not read. `ProcessPPDOptions` only matches "203dpi" and
"203x138dpi", so for the 450 (`300dpi`, `300x600dpi`) the driver's resolution is never set, and the
resolution enters only through `HWResolution[1]` (ported as it is, not "fixed").

| # | What (ours) | Proposal |
|---|---|---|
| W1 | `Order::Job { order, file }` (a PNG): the filter's run (`CCupsFilter::Run`) on the page made from the PNG (W2, W3), its output going to the `usb` backend's printer (W5); `JobEnd` done, or failed with the driver's own error line (`ERROR: ...` of the filter, or the backend's, e.g. `Unable to send data to printer.`) | to approve (proxy) |
| W2 | **Page geometry (ours, from the PPD).** The page is the PPD's `PageSize` choice at the PPD's resolution (default `300dpi`, `lw450.ppd:417-418`: `HWResolution [300 300]`); CUPS's raster is the *imageable area* of that size: `cupsWidth = (right - left) * xres / 72`, `cupsHeight = (top - bottom) * yres / 72` from `*ImageableArea` (`lw450.ppd:279ff`), `PageSize` the integers of `*PaperDimension`, `cupsInteger[0]` as the size's `cupsInteger0` (always 0). The PNG must be exactly that many pixels; any other size fails the job, nothing is scaled. The size is chosen by `JobOrder.label_length_mm`: the `PageSize` choices whose length (`PaperDimension` height in points, in mm, rounded) is that length **and** whose raster is the PNG's size (equal choices are the same page); none or several different ones fail the job; no length given: the PPD's default `w167h288` (`lw450.ppd:149`), and the PNG must fit it. The rule was checked against macOS's own `cupsfilter` on every size of the PPD (`scripts/dymo-labelwriter-450-check.py`) | to approve (proxy) |
| W3 | **Page colour (ours).** The default `DymoHalftoning ErrorDiffusion` (`lw450.ppd:424,426`) gives `cupsColorSpace 1` (RGB), 8 bits per colour, `cupsBitsPerPixel 24`: the PNG is decoded with the `png` crate (as the PnP), 16-bit reduced to 8, grey and palette expanded to RGB, alpha laid over white paper, and the filter halftones the lines itself. `Default` (1 bit, `cupsColorSpace 3`, black is 1) takes a PNG pixel as black where `RGBToGrayScale` (the driver's) is below 128; `NLL` is as `ErrorDiffusion` with the driver's NLL. A job always uses the PPD's defaults (`cupsParseOptions("")`, no options); the other choices are the tests' | to approve (proxy) |
| W4 | The filter's argv/env (ours): job id, user `maker-connect`, title the job's name, copies 1, options `""`; `PPD` is the plain 450's data in `ppd_lw450.rs` (generated from `lw450.ppd` by `scripts/dymo-labelwriter-450-check.py`, which also checks it), `DEVICE_URI` is `usb://DYMO/LabelWriter%20450` (the backend's matching, W5). No `DYMO_PRN_DIR` | to approve (proxy) |
| W5 | **The `usb` backend and the back channel** (CUPS runtime): `usb-libusb.c` `print_device`, `find_device`, `open_device`, `print_cb`, `read_thread` over `nusb` for libusb (ours, as the PnP's: the same crate choice, no shared code). **The backend and the filter start together, as CUPS starts them:** the backend runs on a thread of its own and waits for the printer inside `print_device` (forever, 5 s apart, `usb-libusb.c:206-211`) while the filter runs and writes into the pipe (an in-process pipe of 64 KiB, ours); the backend reads it 8192 bytes at a time (`print_buffer`, :327) and writes each read with one bulk transfer; with no printer the filter's first status read times out after 10 s and its data waits in the pipe, as in the reference. `cupsBackChannelRead(buf, 16, 2.5)` reads what the read thread (512-byte bulk-in reads, 60 s timeout) put on the back channel. Nothing is sent while idle |
| W6 | `Order::Command`: failed `CommandDone`, "the DYMO driver has no such control"; `Control` and `Answer` after a print (O2): a report with `problem` set, as L5. **Stop during a print = what CUPS does on cancel: the filter is terminated** (SIGTERM; the driver has no stop). In the driver's terms: the job status is `jsDeleted` (`PollUntilPaperIn` returns at once), and the filter ends at its next call to its environment (a `Cancelled` unwind caught where `Run` was called); the backend then ends as it does at the end of its input; `JobEnd { stopped }`. Pause and resume, a command or an answer that arrives during a print wait until the print ends | to approve (proxy) |
| W7 | Telemetry (ours, only what the monitor reads during a print): Busy during a job, Idle after; `raw_status` is the first status byte `ReadStatus` read, in hex; `problem` is the PPD's own reason text (`lw450.ppd:47-55`, English) for the `STATE: com.dymo.*` line the monitor's environment writes (`CupsPrintEnvironment.cpp:164-190`), none for `STATE: none`; `state_reason` is that line's keyword itself (`docs/design/wire.md`, "The second device", owner 2026-11-21). The last `STATE:` line holds until the next, through the status reads between them. No polling | to approve (proxy) |
| W8 | `Gone` when the backend's `find_device` finds no printer (it then waits and looks again every 5 s), `Back` when it finds it after that; at print time only | to approve (proxy) |
| W11 | Smaller numbers of ours, none on the wire: the backend's wait for the printer and the Stop poll use 50 ms slices of the 5 s `sleep`, the `nusb` reads 100 ms slices of the 60 s timeout (a Stop and the end of a job are heard); the write's "no timeout" (`libusb_bulk_transfer(..., 0)`) is waited for in slices of 3600 s (`nusb` has no infinite wait; a slice that ends is a timeout, which the backend ignores and retries as `usb-libusb.c:498-505`); 10 ms steps inside the read thread's 250 ms sleep so that it ends at job end; an `Idle` report at start-up (L1's rule); the status byte of the telemetry is read from the driver's own `ReadData()` debug line | to approve (proxy) |
| W9 | The filter's stderr (`DEBUG:`, `INFO:`, `STATE:`, `PAGE:`, `ERROR:` lines) goes to a sink the port keeps for the telemetry (W7) and the failed job's reason (W1); the lines are the driver's own | to approve (proxy) |
| W10 | `Run`'s `fd` (`open(argv[6])`, stdin) and `cupsRasterOpen` are replaced by the in-memory page (W2, W3): the header is made as `cups_raster_update` would validate it (ported), the lines are read as `cupsRasterReadPixels` returns them (uncompressed, ported). Not covered: compressed and byte-swapped raster streams, several pages, `DYMO_PRN_DIR` | to approve (proxy) |

**Wiring:** `--mode labelwriter-450` opens this port (`crates/agent/src/ports.rs`). **Old code (B8):**
`crates/label/src/labelwriter.rs` loses its device code (see the audit); the hub keeps `dots_for_mm` and
`HEAD_DOTS` (`profiles.rs:229-230`). Proof and "not covered": `docs/audits/dymo-labelwriter-450.md`.

**LaserFoundry web fork, the architect's advice (2026-11-15, decisions open for the owner):**
(1) a compile-time feature in `apps/laserfoundry-web` (e.g. `machine-link`, default on upstream) that
leaves Web Serial out; with it off `WebPlatform::machine_available()` is false, as LaserFoundry's
own "not available in this build" path; plus `Permissions-Policy: serial=(), usb=(), hid=()` from
the hub and a release check that no `requestPort`/`navigator.serial` is in the bundle.
(2) Served by the hub, same origin (works on an http LAN and offline; GitHub Pages fails on mixed
content, its fixed CSP and local-network checks). (3) Built by our release script from a pinned
fork commit at the same LaserFoundry tag as the agent's laser port, packed into the hub archive
(covered by SHA256SUMS and the signature). (4) Apps API v1 stays (v2: removed, no v1 kept, [overview.md](../../design/overview.md#rules-every-part-obeys) rule 8): the file lands in Files with the
laser chosen; a person frames and starts it ("Burn" to the queue would be a new safety decision and
an API v2). (5) The fork sends LaserFoundry's own exporter output only, nothing of ours in the
G-code. (6) The upstream merge request follows the MakerFoundry pattern (`crates/ai/src/maker_connect.rs`,
`crates/ui-egui/src/maker_connect_ui.rs`, `service.rs` same-origin only); no AGPL code into it.
Addendum (architect): "Burn → the laser's queue" fits the 2026-11-06 rule only if the hub's queue
never starts a laser job by itself (`queue.rs` `start_queued` skips lasers); a person at the laser
frames and presses Start on the machine page (operate + PPE, with a session; the app's token never
starts anything); optionally the hub refuses Start unless that file was just framed on that
machine. Apps API: a compatible v1 extension (v2: removed, no v1 kept, [overview.md](../../design/overview.md#rules-every-part-obeys) rule 8) (`POST /api/apps/v1/machines/{id}/queue?file=<id>`,
`queue_length`/`queue_held` per machine, `info.features: ["queue"]`), `docs/api/apps-api.md` changed in
the same change.

**Owner, 2026-11-15 — a laser never starts unattended:** "Laser is een machine waar je vanalles
moet doen om het materiaal en printer eerst klaar te maken. outer dimensions wil bekijken enz enz.
persoonlijke bescherming (bril) enz. dus nooit onbeheerd starten." "Burn" puts the file in the
laser's queue (the architect's v1 extension, v2: removed, no v1 kept, [overview.md](../../design/overview.md#rules-every-part-obeys) rule 8); the queue never starts a laser job by itself; a
person at the laser prepares the material and machine, frames to check the outer dimensions, meets
the PPE check (glasses), and presses Start on the machine page.

**Owner, 2026-11-15:** the GRBL laser port starts now, in parallel with the Marlin proof and the
LabelWriter 450 ("GRBL-laserport nu al parallel starten -> ja").

**The four open points, decided (proxy + architect, 2026-11-15; the owner: "die 4 vragen moet je aan
de agents vragen"):** (1a) APPROVED now, as B3/B7 completion: for agent machines the hub drops its
one-command-at-a-time gate (`store.rs` ~1818-1827) and the 120 s; a command is given up only when
the port sends `Done{ok:false}` for commands still pending when comm closes or on `Order::Close`,
or the agent disconnects (a new K row, ours, engine↔hub, nothing on the wire); K11 changes to
match; Buddy/Klipper keep the 120 s until the hub side. (1b) the hub queue in SQLite and (1c) the
engine's unreported reports on disk: at the hub side (architect's design noted: AUTOINCREMENT ids;
Queued commands after a hub restart are cancelled, not sent late; reports accepted only once).
(2) Emergency-stop button: at the hub side; OctoPrint machines typed `M112`, lasers LaserFoundry's
stop, label printers none, no new action. (3) Docs: each running agent updates only its own audit
file and README row; one pass over `design-overview.html` after all three. (4) Reference script:
after the laser agent; read LaserFoundry's rev from `Cargo.lock`, fail on a mismatch, cover every
ported or audited reference.

### Step 3d: the laser (2026-11-15; ours below: to approve (proxy))

> **Replaced as the reference (owner, 2026-11-21):** "gnea-wiki als referentie". The reference for
> what a GRBL laser is sent is now the gnea/grbl wiki `86e8ecd` and its `stream.py` (gnea/grbl
> `bfb67f0`), to the letter (CLAUDE.md, *Lasers (GRBL)*). LaserFoundry below is only the library. What
> this step took from LaserFoundry and its app that the wiki does not have is being removed (#171).
> The text below is the history of the step, not the rule.

Reference: **LaserFoundry v0.5.0** (`61febdb`, `reference/laserfoundry`, MIT OR Apache-2.0; owner's exception of
2026-11-15, see "Owner overrules the watchdog on LaserFoundry"): `laserfoundry-device` (`Link`, `SimulatedGrbl`,
`restart_line`), `laserfoundry-gcode` (`resume_from`, `frame`), and the link logic of its app
(`crates/ui-egui/src/lib.rs`, ported in `crates/grbl`). The pin in `crates/grbl/Cargo.toml` moved from `3b9b9fa` to
`61febdb` (v0.5.0: #76 `resume_from` after `M5`, #121 MPos over WPos, #64 the frame's feed limit); nothing in our
code needed changing for the new API, and the hub's virtual lasers (`crates/hub/src/devices/virtual_grbl.rs`) run on the
same `GrblMachine` (adapted to the answers coming later). LaserFoundry's own tests at v0.5.0 pass (`laserfoundry-device`
71, `laserfoundry-gcode` 49, and the app's `laserfoundry-ui-egui` 292, among them its three reconnect tests), run in a
separate `git worktree` of `reference/laserfoundry`. Only the GRBL laser: Marlin and Smoothieware lasers stay closed.
`--mode grbl` on a Smoothieware machine is untried (it speaks GRBL's protocol; it stays closed until tried on one).

**`crates/grbl`, what changed (the architect's fixes):** the `$I` at connect is gone (LaserFoundry never sends it; so is
`build_info`, and with it the machine's name from `$I`); `$$` goes only once the first status report has come
(`ui-egui/src/lib.rs:1401-1405`, `settings_asked` :262, reset :1178 and :1361); `carry_out` is gone: a command is handed
to the `Link` (`GrblMachine::submit`) and answered later from the snapshot's console (`Update::done`), as the OctoPrint
port's K11; `$H` and the frame do not block either (LaserFoundry's `Link` does not block on them); a command with no
answer fails (G9); `ANSWER_WITHIN` (90 s), the 3 s wait in `start()` and the 50 ms and 5 ms sleep loops are gone (the one
that is left is G7); `greets()` is gone (the firmware is told at start-up: `--mode grbl`). Ported from `ui-egui`, each with
its file and line in `crates/grbl/src/lib.rs`: `poll_link` :1379-1410, `reconnect_tick` :1347-1377 (`RECONNECT_ATTEMPTS`
8 :82, `RECONNECT_MAX_WAIT` 30 s :84, `reconnect_wait` 2 s :457, doubling :1371, `auto_reconnect` true :455, only for a
serial target, :1392, and `!platform.serial_async()`: the agent opens its serial port itself, so that holds; a running
job is never resumed, :1344-1345), and `start_job_with` :1221-1251: no link (:1222-1224), `not_ready(state)` (:1232-1236,
:1520-1532), laser mode off (:1239-1246, `settings_ui.rs:26-31`: `$32` is `0`). **Not covered:** the confirmation of a
start from the head's position or a user origin (:1250; a program file has no origin setting: the person frames at the
machine and presses Start there); `link.protocol() != link_protocol()` (:1227: the protocol is fixed).

**What is ours** (G1 to G16; the list is also at the top of `crates/engine/src/ports/laserfoundry/port.rs`):

| # | What | Proposal |
|---|---|---|
| G1 | An `Action` is the `Link` call LaserFoundry's app makes for the same control: Unlock `Link::unlock` (`control_box.rs:160`), SetOrigin `set_zero` (:163), GoOrigin `go_to_zero` (:166), Home `home` (:157), ReadSettings `request_settings` (`settings_ui.rs:60`), Jog `jog(dx, dy, feed)` (`panels.rs:813`; no feed: LaserFoundry's `jog_feed` 3000, `lib.rs:413`), typed G-code `send_line` (`panels.rs:865`, one call per line). Home's axes are not used: LaserFoundry has one homing cycle (`$H`) | to approve (proxy) |
| G2 | FDM actions, and a jog with Z (LaserFoundry has no Z jog) fail "LaserFoundry has no such control" | to approve (proxy) |
| G3 | `LaserOff` fails "LaserFoundry has no such control". LaserFoundry's app has no laser-off control: its Stop (`panels.rs:774`, `control_box.rs:148`: "Soft reset: stops motion and turns the laser off") is `Link::stop`, here a job control (G6). A bare `M5` is not sent (it would be ours). **For the owner:** whether the hub's laser-off button should send Stop (a soft reset) or stay absent | to approve (proxy); owner decides the button |
| G4 | `Frame { download }`: the agent's hub link (`link.rs`, `Commands`) fetches the command's file with the B2 fetch (a thread of its own, streamed to `frame-<id>`, at most 512 MB) from `/api/agent/v1/files/{id}` (v2: `/api/agent/v2/files/{id}`, no v1 kept) only (the path must be exactly that, an id of letters, digits, `-`, `_`; anything else fails the command; the hub's agent route authorises it as B2, with the agent's token), and gives the port the local path in `download`; commands behind it wait (O2), and the file goes once the command is reported done. The port reads the file's extent (G16) and runs `GrblMachine::frame`: `gcode::frame` (`crates/gcode/src/lib.rs:92`) as a job through the same refusals as Start, as `panels.rs:734-740` does; the command is done when the frame has run. A path that is not a readable file, or a file with no moves, fails the command | to approve (proxy); decided (proxy) |
| G5 | A job is `GrblMachine::start`: the refusals above, then `Link::start_job` (`lib.rs:1270-1273`); `from_line` is `start_from_line` (:1258-1266), `resume_from` for any line given (also 1: the app does); the line to start again from is `restart_line` over the acknowledged lines (`record_job_position` :1291-1328), in the design's numbering (`Restart::place_in_design` :105-115). The snapshot keeps the earlier job's end until the link's thread takes the new job up: this job's end is taken only once the snapshot changed or the link logged "job started" (no wait). A start while a job is held is refused with the link's `Busy` text | to approve (proxy) |
| G6 | Pause, resume and stop are `Link::pause`, `resume`, `stop` (`panels.rs:766-776`); stop works with or without a job, as the app's button. A refusal, and a question's answer (LaserFoundry asks none), are the report's `problem` until the next order | to approve (proxy) |
| G7 | The port looks at the link's snapshot every 100 ms (`TICK`), as LaserFoundry's app does for a threaded link (`ui-egui/src/lib.rs:1384`). No byte goes to the machine for it: the 200 ms `?` is the `Link`'s own and is counted apart in the proof. An order is acted on the moment it arrives (the port waits on its orders with the tick as the time limit), as the app calls `Link::stop` and `pause` when the button is pressed (`panels.rs:766-776`, `control_box.rs:141-149`) | decided (proxy) |
| G8 | Telemetry from the snapshot: state (Run, Jog, Home, Check: Busy; Hold, Door: Paused; Alarm: Error), the machine position (a work position, `position_is_work`, is not reported), the job's progress (`acknowledged` of `total`, time left from the lines so far: `elapsed * (1 - f) / f` once `f` is over 0.02, and the elapsed time without pauses, both ours, kept from the old port and not LaserFoundry's; LaserFoundry has no estimate in its app. **The time left exists only in `Telemetry.job.remaining_s`, made in `GrblMachine::telemetry` (`crates/grbl/src/lib.rs`) and shown by the hub's UI; nothing of it is used for, or sent to, the machine: the only writes to the device are LaserFoundry's `Link`'s**), `$30`, `$32`, `$130`-`$132`, the greeting (`Grbl 1.1f`) as the firmware ("GRBL" until it greets). Nothing else is derived | to approve (proxy) |
| G9 | A command is done when the machine answered every line it sent (`ok`); its output is the console's echo and answer lines. It fails on `error:N` or `ALARM:N`, when the link refuses it (the link's words), when the link ignored it ("! ignored: a job is running", a race with a job start), and when no answer can come: the connection was lost, a stop's soft reset cleared the answers owed, a job failed and reset the controller. It never counts as done without an answer and has no time limit of its own (a homing cycle takes as long as it takes; the hub gives up its commands after 120 s). The `$$` the link logic asks after the first status is an internal entry in the same queue, so its `ok` is nobody's command's. Answers go to commands in the order the lines were sent, as GRBL answers | to approve (proxy) |
| G10 | A machine that did not open at start (no `--port`, or the port will not open) is reported Disconnected with "Could not open the machine: …" (`lib.rs:1181`) and is not opened again: LaserFoundry's Connect is a button of the person's, and its reconnect is only for a connection that was there. The agent has to be restarted | to approve (proxy); for the owner |
| G11 | `Gone` is a lost connection (or the machine that did not open), `Back` a reconnect. The report's `problem` says what the app's status bar says ("The machine connection was lost; trying to reconnect. The job was stopped and will not resume by itself.", "Could not reconnect: …"); "Reconnected" (:1363) is not reported | to approve (proxy) |
| G12 | The frame's program is `gcode::frame` over `laserfoundry_doc::Machine::default()` with the bed from the controller's `$130` and `$131` and `max_s` from `$30` once read (the app's is the design's machine), `Origin::TopLeft` (the bounds are a file's own machine coordinates: no flip of Y), `Placement::Absolute`, 3000 mm/min (`engine/src/cmd/job.rs:241`), 0 % power (`lib.rs:416`: the beam is off) | to approve (proxy) |
| G13 | A job running when the connection is lost ends failed with the link's reason, and its restart line when lines were acknowledged (the app only shows it in its status bar and writes no position then) | to approve (proxy) |
| G14 | `--baud` is LaserFoundry's `serial::DEFAULT_BAUD` (115 200, `serial.rs:9`) when not given; `--port` is required: without it the machine does not open, loudly (G10) | to approve (proxy) |
| G15 | Answers are read from the snapshot's console (`CONSOLE_LIMIT` 500 lines): the `Link` has no per-command completion. A burst of more than 500 console lines between two looks (20 ms) could lose an answer; the command is then given up by the hub. `LaserFoundryPort::with_machine` and `GrblMachine::new` (a simulator: not opened again) are for the tests and the hub's virtual lasers; the virtual lasers' own thread (`virtual_grbl.rs`) waits for answers with a 10 ms sleep and no time limit, and `commands_of` turns its G-code text into `Link` calls (the hub's simulator, never the wire). The port sends a report only when it changed. "Could not reconnect" has no port name in it (the app's has) | to approve (proxy) |
| G16 | The extent of a G-code file for a frame (`port.rs` `extent`): the X and Y of its `G0` to `G3` moves in `G90` or `G91`, comments cut at `;` and `(`, as the hub reads a file's `bounds_mm` (`gcode.rs` `extent`). LaserFoundry has no such function (upstream proposal 6) | to approve (proxy) |

**The hub's laser checks (ours, in the UI, under B3):** `files.rs` `laser_check` refuses a laser file when the
machine's `$32` is off, when its moves go outside the machine's travel (`$130`, `$131`, else the profile's work area),
and warns when the file asks for more power than `$30` or looks made for a 0 to 255 range; `frame` refuses a file that
does not fit (`fit`), needs the PPE check (`safety::check_ppe`) and `Use` access. They never reach the wire; the
machine's own `$32` and travel are read by the port from the controller (G8). A laser job never starts by itself: the
queue does not start lasers, and the port starts nothing but the job it is given, and after a reconnect nothing
(owner, 2026-11-15, "dus nooit onbeheerd starten").

**Proof:** `docs/audits/laserfoundry-grbl.md` (`crates/engine/src/ports/laserfoundry/proof.rs`): LaserFoundry's
`SimulatedGrbl` driven through its `Link` directly, in the app's order of calls, and through this port; every byte
recorded; identical, with the `?` polls counted apart.

**Proposed upstream contributions to LaserFoundry** (the behaviour belongs there, not in our code; the mature senders
Universal G-code Sender, gSender and LaserGRBL are the references for each): (1) the app's link logic, `poll_link`,
`reconnect_tick` and the start refusals, as a host-neutral session in `laserfoundry-device`, so an agent, a command line
or the MCP server get it without egui (our `crates/grbl` ports it); (2) a completion for each immediate command
(an id or counter in the `Snapshot`, with its answer), so a host does not read the console (G9, G15); (3) a time limit
for an immediate line that nobody answers (the worker times out jobs only, `JOB_IDLE_TIMEOUT`); (4) a generation or id
for the job in the `Snapshot`, so the end of the job before is not taken for the next one's (G5), and `send_line`
racing with `start_job` (`require_idle` reads the snapshot; "! ignored" is only a console line); (5) a laser-off call
(`M5`) and a documented meaning of Stop (G3); (6) the extent of a G-code program in `laserfoundry-gcode`, so a host
can frame a file (G4, G12).


**Step 3d decided (proxy, 2026-11-15):** G1, G2, G5, G6, G9–G15 approved; G3 laser-off fails "no
such control" (a bare `M5` would be ours; whether the hub shows the button at all is the owner's;
the proxy recommends absent, Stop exists); G10 approved (no retry of a failed open; written under
"not covered"); G7 FIX: the port reads LaserFoundry's snapshot every 100 ms as its app does for a
non-inline link (`ui-egui/src/lib.rs:1309`), and the proof's reference side uses the same cadence;
G8 the time-left estimate only in the UI; the hub link fetches a command's file (Frame) with the B2
fetch over the existing `/api/agent/v1/files/{id}`, authorised as B2 (v2: `GET /api/agent/v2/files/{id}`, [wire.md](../../design/wire.md); no v1 kept). Upstream contributions:
proposals only; sending them is the owner's.

**Step 3c decided (proxy, 2026-11-15):** W6 approved (Stop = CUPS's cancel; write down that the
backend ends "as at end of input" where CUPS also SIGTERMs it); W11 approved (add a test that fails
if the driver's `ReadData` debug line changes); W5 FIX: start the backend and the filter together as
CUPS does, the 5 s wait inside the backend's `print_device`, and move the 50/100 ms slices into W11;
the hub/port size mismatch waits for the hub side (W2 fails a wrong size loudly). CUPS 2.4.20 as the
driver's runtime: for the owner (the proxy recommends yes, limited to the files the driver uses).

**The five questions, decided by the agents (proxy + architect, 2026-11-15; the owner: "laat de
agent die vragen beantwoorden"):** (1) no full GIL and no CPython interpreter C sources: port only
`Thread.start`'s behaviour under the GIL in `py_threading.rs` (it returns once the new thread first
blocks or ends), checked at each of the port's ~8 thread starts, with the 1000-run test and a
debug-only watchdog; (2) the read after `M112`: first run the reference over the same out-of-process
bridge; if it is the harness, record it as a read-only harness difference (no pyo3); (3) the
reference is re-recorded on CPython 3.13 in a venv under `reference/`; (4) no "Laser off" button
until LaserFoundry has a laser-off call (upstream proposal 5); Stop stays; (5) CUPS 2.4.20 is the
DYMO driver's runtime, limited to the files already ported, Apache-2.0 noted in their headers, no new
files without a reason on the wire. Also: `py_format.rs` and `py_percent.rs` are frozen (keep, stop
extending); stop porting runtimes; fix K18-1 in our crediting (the `ok`s of lines comm sends on its
own after `M112` credit no pending command); run the proof tests apart from the unit suite. The next
real step is hardware (D11): a Marlin printer with timeouts on and the DTR reset on open checked,
the PnP and 450 over `nusb`, an attended GRBL laser; the 450 needs the hub-side label size first.

**Section M decided (proxy, 2026-11-15):** approved: `START_HANDSHAKE_BOUND` = 10 s (debug builds
only, never on the wire); release builds keep `start()` without a bound (a bound would be a timing
of ours), on condition that the audit's table of each started thread's first blocking point stays
checked by a test, and every new thread comes with its row and test (the serial path is hooked at
`comm.rs` ~8177 and ~9214); K18-1 (after a queued `M112` or an "emergency" error, until Gone/Back,
no received line credits a pending command; nothing on the wire), written down with the note that
this window can also drop a genuine `ok` for an earlier command.

**Owner, 2026-11-16 — the PnP's idle status poll comes back (an owner exception to labelle):** the
spontaneous feeds were a tape sensor under the PnP with bad contact, not the poll. "Dus ik zou voor de
PNP wel de status update poll doen." As the owner's 2026-11-11 form: every 15 s, the printer opened as
for a label, one `ESC A`, its answer read, the printer released. Ours (labelle sends nothing while
idle), approved by the owner; the status stays raw (`raw_status`, meaning unknown) until the bits are
proven on the device.

**Owner, 2026-11-16 — the PnP's status bits come back (owner overrules L6):** "overruled. we hadden
een goed idee van de status bits en bytes." The port reads the cassette from byte 0 bit `0x40` and the
battery from byte 3 (when at most 100), as the earlier code did (seen on the owner's PnP), from every
status answer (during a print and from the idle poll); `raw_status` stays alongside. Ours, the owner's
decision; labelle discards these answers.

### Step 3e: the DYMO M-series scale (2026-11-16; ours below: decided (proxy))

The owner, 2026-11-16: "Bouw de M2 agent support nu ook meteen in." / "gewoon standaard gewicht meten en
doorgeven live. verder niet het hele spool system." / "zolang onze m2 driver werkt voor m1, m2, m5, m10 enz op
linux, windows en macos prima." In parallel with the other work, by the owner's word. A third DYMO stack: it shares
no code with labelle (PnP) or the LabelWriter.

Reference: **usbscale** (github.com/erjiang/usbscale, `9ed1cb55a2dfd0daeb41548031a49ab756cb9114`,
`reference/usbscale/usbscale.c`, GPL-3.0-or-later, which GPLv3 §13 lets us combine with our AGPL-3.0): its
`print_scale_data` (usbscale.c:368-456) and `UNITS[13]`, ported function by function with its header. Checked
against HID Usage Tables 1.6 ch. 33 and HID POS 1.02 §6.2.4 Figure 27 (`reference/hid-usage-tables/`), which are
used only to verify, never as a second source. Not the reference: weighty (one person, 2020, one scale, no tests;
read only to understand). Nothing is sent to the scale by us: no tare, no feature report, no GET_REPORT. The OS
opens the HID device (hidapi); what the OS itself sends on an open is not ours and is not known.

The scale's range is not read: it is a Feature report (Weight Limit) that needs a GET_REPORT, and usbscale does not
send one. The hub shows only the scale's own "over weight limit" status.

Agent: `--mode dymo-scale`, `--kind scale` (`MachineKind::Scale`), `--port` = the HID path (`/dev/hidrawN`, the
IOService path, `\\?\HID#…`). No other parameters.

Ours (decided (proxy, 2026-11-16); the owner delegated these: "laat de agent die vragen beantwoorden"):

| # | Ours | Why |
|---|------|-----|
| E1 | hidapi 2.6.7 instead of libusb (Linux `linux-native-basic-udev`, Windows `windows-native`, macOS IOKit) | on Windows/macOS the OS owns HID; like pyusb → nusb for labelle |
| E2 | the device: the one with top-level usage page 0x8D, or `--port`; none or several: refuse at start and list them | usbscale's `scales.h` VID:PID table lacks M1/M2/M5; B4: never guessed; no VID:PID fallback |
| E3 | reads on and on; every status reported; no exit on Fault or an unknown status (usbscale returns −1 and exits) | "live"; per status "differs, ours" in the audit |
| E4 | telemetry `Telemetry.scale`: status (raw + usbscale's text, or "unknown n"; status 4 has no text, as usbscale prints none for it), unit (raw + `UNITS` name, or "unknown unit n" when > 12), exponent, raw, weight in the scale's own unit; no conversion to grams | the owner: weight only; `UNITS[unit]` over 12 is undefined in usbscale |
| E5 | while the scale is gone, look for it again every 15 s (enumerate, open; nothing sent by us) | as labelle's `POLL_INTERVAL`; usbscale is a one-shot CLI |
| E6 | a read timeout of 1 s, only to see `Order::Close`; the device sees nothing of it | ours, off the wire |
| E7 | a Telemetry event only when the decoded report changes; Idle while reports arrive; a read error or a missing device: Gone, found again: Back | the engine's events |
| E8 | the bytes hidapi returns are taken as usbscale's `dat` (byte 0 = report ID): no byte stripped or skipped; a report that does not start with 3 or 4 is rejected as usbscale rejects it | hidapi puts the report ID first for numbered reports on every OS; to be seen on the M2 |
| E9 | report ID 4 is parsed as usbscale does (6 bytes) and marked "meaning unknown" in the audit | usbscale accepts it; POS calls it the 2-byte Status Report |
| E10 | the udev rule `SUBSYSTEM=="hidraw", ATTRS{idVendor}=="0922", MODE="0660", GROUP="plugdev"`, shipped as a file with an install note | hidraw is root-only; not on the wire |
| E11 | the hub: one live line on the machine page (UI) | the owner: show it live |
| E12 | a report whose ID is not 3 or 4, or shorter than 6 bytes (usbscale never checks the length): shown once as the problem "Error reading scale data" (usbscale's text), the last reading kept, read on; the problem clears on the next good report (decided (proxy, 2026-11-16): raised once, cleared only by a good report; a report not starting with 3 or 4 is never decoded) | usbscale exits there; E3 |
| E13 | commands and jobs sent to a scale are refused (`NO_COMMANDS`) (decided (proxy, 2026-11-16)) | the scale takes no commands; live weight only |

From usbscale, as it is: the old data. usbscale reads one report and throws it away ("we get old data the first
time", the extra read before the loop), then starts with `weigh_count = WEIGH_COUNT - 1` (`WEIGH_COUNT` 2,
`reference/usbscale/scales.h:15`) and decodes a report only while `weigh_count < 1` (usbscale.c:179, 322-327). The
port does exactly that after each open; the audit states the resulting count. usbscale's exits (a report ID other than
3 or 4, status 1 Fault, an unknown status: `return -1`) are each an audit row "differs, ours (E3/E8)".

Needs the real M2 (the audit's not-covered list): its VID:PID and report descriptor (usage page 0x8D, report ID 3,
Figure 27's 6 bytes), whether the first report is stale, the report rate, the kg/lb button (units and exponents),
known weights, over 2 kg (status 6), under zero after a tare (status 5, its weight bytes), auto-off, unplug and
replug, the first byte per OS, hidraw permissions and hidapi's `usage_page` on the Pi, macOS input-monitoring.

## Later: one agent, several machines (owner, 2026-11-17)

Owner: "zet één agent meerdere machines op de lijst" (asked after the LabelWriter 450 needed a second
agent beside the PnP's on .67). The engine already holds several machines, one per port on its own
thread (O1); the agent does not: its config has one `mode`, `name` and `kind`, and it registers one
machine with the hub (`crates/agent/src/config.rs`, `link.rs`). To do: several machines in one config,
each with its own mode/port and its own registration and pairing code, reported over the one link.
Design first (architect + proxy), then the owner. Until then: one agent process per machine (the
450's test agent: `~/maker-connect-lw450/`, user-service `maker-connect-agent-lw450`).
