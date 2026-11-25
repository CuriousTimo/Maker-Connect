# MK3 port plan (PrusaLink): part 1, the reference and the file inventory

Status: draft, part 1 of 4, for the owner (task approved 2026-11-16, "akkoord op mk3 plan"; work in small
steps, owner 2026-11-16). Nothing is built. Rules: `CLAUDE.md` (one reference to the letter, port don't paraphrase).

## 1. The reference

| | Repo | Commit | Version |
|---|---|---|---|
| PrusaLink | `reference/Prusa-Link` (github.com/prusa3d/Prusa-Link), branch `websockets` | `171148ce7dbc7f1fb0633ea519523f3258877e2e` (2026-11-05, "Install the websockets branch of the SDK in README, CI and dev images") | `__version__ = "1.0.0.dev0"` (`prusa/link/__init__.py:8`) |
| Connect SDK (printer) | `reference/Prusa-Connect-SDK-Printer`, branch `websockets` | `c0d7d233d4ce2081e6c020d2e98fb92ad9b51521` (2026-11-03, "test: cover the WebSocket transport and WS command parsing") | `__version__ = "0.9.0dev0"` (`prusa/connect/printer/__init__.py:39`) |

- **Pin** (owner 2026-11-19, [decision](../../design/decisions/20261119-mk3-reference-prusalink-websockets.md)): the
  `websockets` branches, which speak the WebSocket to Connect. PrusaLink takes the SDK
  `@git+…Prusa-Connect-SDK-Printer.git@websockets` (`requirements.txt:1`), so the pair is fixed. The earlier
  pin was master `c453996d` and SDK `3d21ae59` (HTTP to Connect).
- **Not re-counted yet:** the line numbers and sizes in §2 to §9 were taken at the earlier pin. Against it,
  `prusa/` differs by +331/−105 lines in 18 files (PrusaLink) and by +609/−228 lines in 6 files (SDK), so
  every file:line must be extracted again at the new pin before a section is built.
- **Licence (D1, owner 2026-11-12):** port both. GitHub shows no licence for either; both `setup.py` say
  "Freeware" (`Prusa-Link/setup.py:129`, `Prusa-Connect-SDK-Printer/setup.py:29`, `pyproject.toml:5`). The
  code is public. Every ported file names its source and commit.
- `reference/Prusa-Connect-SDK-Printer/build/lib/` is a stale copy of the same modules; ignored.
- Reference's own tests: PrusaLink `tests/` (`test_carousel.py`, `test_ipc_queue.py`, `test_item_updater.py`,
  `test_serial_parser.py`, `util.py`); SDK `tests/` (21 files, e.g. `test_command.py`, `test_files.py`,
  `test_download.py`, `test_printer.py`, `test_telemetry.py`).

## 2. File inventory

Basis: the `from .` / `from prusa.connect` imports of every file (read with grep), the constructor of
`PrusaLink` (`printer_adapter/prusa_link.py:133-394`) and the web routes that produce G-code
(`web/controls.py`, `web/main.py:408-560`). Total `prusa/link`: 22,939 lines in 100 files; SDK `prusa/connect/printer`: 4,873 lines in 14 files (`find reference/Prusa-Connect-SDK-Printer/prusa -name '*.py' | xargs cat | wc -l`; part 1 first said 4,857, a mis-sum).
NEEDED = the MK3 printer side (serial, state, commands, file printing, telemetry/events). "Unclear" is said where
the imports do not decide.

### PrusaLink `prusa/link`

| Files (lines) | Mark | Why |
|---|---|---|
| `serial/serial.py` 161, `serial_adapter.py` 427, `serial_parser.py` 187, `serial_queue.py` 674, `instruction.py` 183, `helpers.py` 109, `is_planner_fed.py` 216 | NEEDED | the wire (own termios `Serial`, not pyserial: `serial.py:1-30`); imported by `prusa_link.py:38-43` |
| `printer_adapter/structures/` `regular_expressions.py` 190, `model_classes.py` 152, `module_data_classes.py` 159, `item_updater.py` 569, `carousel.py` 241 (used by `lcd_printer`), `heap.py` 170 (used by `is_planner_fed`), `mc_singleton.py` 20 | NEEDED | imported by the serial and state files; `carousel.py` only if `lcd_printer` is needed |
| `printer_adapter/` `state_manager.py` 829, `printer_polling.py` 1093, `auto_telemetry.py` 136, `telemetry_passer.py` 417, `job.py` 299, `model.py` 41, `print_stats.py` 106, `print_stat_doubler.py` 48, `updatable.py` 69, `keepalive.py` 64 (`M79`) | NEEDED | state, polling, telemetry to the SDK `Printer`, job id; wired in `prusa_link.py:236-262` |
| `printer_adapter/` `file_printer.py` 415, `command_handlers.py` 724, `command.py` 109, `command_queue.py` 130, `special_commands.py` 111, `mmu_observer.py` 132 | NEEDED | file printing, the commands (`StartPrint`, `PausePrint`, `SetReady`, `RePrint`, ...), the menu/MMU lines. Inside `command_handlers.py`: `UpgradeLink`, `check_update_prusalink`, `update_prusalink` (:52-66, :537) are PrusaLink self-update: NOT NEEDED |
| `printer_adapter/filesystem/` `sd_card.py` 446 | NEEDED | the MK3's SD card (`M20` family, SD list/print) |
| `printer_adapter/filesystem/` `storage_controller.py` 104, `storage.py` 279 | UNCLEAR | watch and list PrusaLink's local G-code folders (`config.py` paths, inotify); our job file is one downloaded file. Needed only if the SDK file tree is |
| `printer_adapter/` `lcd_printer.py` 585 | UNCLEAR | puts PrusaLink status on the MK3 screen (`M117`); wire-visible, but it reads the Connect conditions and transfer state. Hub dropped download progress to the screen (S4) |
| `printer_adapter/` `ip_updater.py` 195 | UNCLEAR | sends the host IP to the printer (`M552`); wire-visible, needs the Pi's addresses |
| `printer_adapter/prusa_link.py` 1104 | NEEDED, split | the glue and callbacks; its camera, Connect-registration and `debug_shell` parts are not |
| `sdk_augmentation/` `printer.py` 197, `command_handler.py` 38, `file.py` 31 | NEEDED | `MyPrinter` subclasses the SDK `Printer`: where state and telemetry leave toward Connect, and where commands arrive |
| `web/controls.py` 235 | NEEDED (its G-code) | the only place PrusaLink makes G-code for jog, home, speed, extrude, tool/bed temperature (`controls.py:38-233`); no fan control exists there |
| `web/main.py` 729 (`:408-430` ready, `:517-560` job control), `web/files.py` 382 (`:295`, `:334` start print) | UNCLEAR | web routes; only the command paths they call are needed |
| `conditions.py` 537, `config.py` 354, `const.py` 359, `util.py` 351, `interesting_logger.py` 238 | NEEDED, partly | imported by the serial files (`serial_adapter`, `serial_queue`) and state; each only for what they use; `util.get_usb_printers` (pyudev) and `get_local_ip*`: UNCLEAR |
| `__main__.py` 276, `daemon.py` 89 | NOT NEEDED | process start, `DaemonContext`, web server; the engine replaces them |
| `web/` rest (`__init__`, `cameras`, `connection`, `errors`, `files_legacy`, `lib/*`, `link_info`, `settings`, `wizard`) ~3,500 | NOT NEEDED | web pages and API |
| `cameras/` 3,588, `camera_governor.py` 62 | NOT NEEDED | cameras (nokhwa decision, memory) |
| `multi_instance/` 1,293, `service_discovery.py` 127 | NOT NEEDED | several printers on one Pi, zeroconf |

### Connect SDK `prusa/connect/printer`

| Files (lines) | Mark | Why |
|---|---|---|
| `const.py` 267, `command.py` 185, `models.py` 230 (imports `camera`), `conditions.py` 285 | NEEDED | imported by PrusaLink (`State`, `Source`, `Event`, `Command`, `CondState`) |
| `__init__.py` 870 (`Printer`) | NEEDED, split | state, `event_cb`, `telemetry`, `set_handler`, `get_info`, file commands (:290-620) are the hub-facing seam; `register`, `loop`, `loop_step`, `parse_command`, `make_headers` (:647-860) are the HTTP link to Connect: UNCLEAR (the hub link replaces it?) |
| `files.py` 937 (`Filesystem`, `File`, `InotifyHandler`) | UNCLEAR | `File` is used by SD and storage; the tree and inotify only if the file tree is needed |
| `download.py` 453 | UNCLEAR | Connect downloads (`START_*_DOWNLOAD`); our hub link downloads the job file (O6) |
| `util.py` 63, `errors.py` 115, `clock.py` 35 | UNCLEAR | `RetryingSession` and `get_timestamp` (`__init__.py` imports) |
| `camera.py` 564, `camera_driver.py` 307, `camera_controller.py` 192, `camera_configurator.py` 370 | NOT NEEDED | but `__init__.py` and `models.py` import them, so a port must cut those imports |

## 3. Mapping to Maker Connect (part 2)

Every line below is **to be approved by the owner**. Nothing is built.

### 3.1 The engine interface the MK3 port implements

`Port::run(self: Box<Self>, orders: Receiver<Order>, events: Sender<Event>)` (`crates/engine/src/lib.rs:57-59`), as the
Marlin port does (`ports/octoprint/port.rs:112`). New: `crates/engine/src/ports/prusalink/`, one Rust file per
PrusaLink/SDK file, header naming source and commit (as `ports/octoprint/mod.rs`). `--mode prusa` (B4).

| Engine side | PrusaLink concept (file:line) |
|---|---|
| `Order::Command{id, Action}` -> `Event::Done` | PrusaLink's command classes run through `CommandQueue` (`command_queue.py`, `command_handlers.py`); `Done` when the class returns or raises `CommandFailed` (`command.py:28`) |
| `Order::Job{order, file}` -> `Event::JobEnd` | `StartPrint` (`command_handlers.py:248-330`), then `Job`/`FilePrinter` (`file_printer.py:105-250`); the end is `file_printer_finished_printing`/`stopped` (`prusa_link.py:866-876`) and `Job` states (`job.py`) |
| `Order::Control(Pause/Resume/Stop)` | `PausePrint`, `ResumePrint`, `StopPrint` (`command_handlers.py:189-247`; `M601`/`M602`/`M603` inside) |
| `Order::Answer(button)` | none found in PrusaLink (it has no screen prompt); the MK3 shows `Attention` only (`state_manager.py:781`). UNCLEAR: confirm in part 3 |
| `Order::Close` | `PrusaLink.stop` (`prusa_link.py:432-500`) |
| `Event::Telemetry` | `Model.latest_telemetry` filled by `PrinterPolling` and `AutoTelemetry` and passed by `TelemetryPasser` (`telemetry_passer.py:85`), which today calls SDK `Printer.telemetry` (`__init__.py:330`) |
| `Event::Gone`/`Back` | `SerialAdapter` `failed_signal`/`renewed_signal` (`serial_adapter.py`, `prusa_link.py:878-886`); PrusaLink reopens itself (`_reopen`, :188): the port does as PrusaLink does, the engine retries nothing |

**States (to approve; table to be made in part 3):** SDK `State` has `IDLE, BUSY, PRINTING, PAUSED, FINISHED, STOPPED, ERROR,
ATTENTION, READY` (`const.py:24-34`), from `StateManager`. Proposed: serial down = Disconnected; IDLE, READY, FINISHED,
STOPPED = Idle; BUSY, PRINTING = Busy; PAUSED = Paused; ATTENTION = Attention; ERROR = Error. READY and FINISHED/STOPPED
as extra `problem` text, not new states (our six stay).

### 3.2 Actions to PrusaLink (O8)

PrusaLink makes G-code for controls only in `web/controls.py`; those functions are ported, not the web routes (as OctoPrint's
`standard.py`).

| `Action` | PrusaLink |
|---|---|
| `Jog` / `Home` | `jog` (`controls.py:38`), `home` (:91) |
| `Extrude` / `Speed` / `Flow` / `MotorsOff` | `extrude` (:118), `set_speed` (:102), `api_tool` flow branch (:175-215), `disable_steppers` (:112) |
| `Temperature` | `api_tool` (:175), `api_bed` (:218) |
| `Gcode` | `ExecuteGcode` (`command_handlers.py:331`) |
| `LoadFilament` / `UnloadFilament` | `LoadFilament`, `UnloadFilament` (:447-475), `prepare_for_load_unload` (:407) |
| `SdList` / `SdPrint` / `SdDelete` | `sd_card.py` (list), `StartPrint` SD branch (:289-300); delete: UNCLEAR (not looked for) |
| `Fan` | **No PrusaLink counterpart** (no `M106` in `controls.py`): answered "PrusaLink has no such control" (failed `Done`, as M2/K3 for OctoPrint). No fan control for the MK3, as the reference has none |
| `ProbeBed`, laser actions | no counterpart: same refusal |
| `Home{axes}` (B5, K5) | `home` takes an axes list; PrusaLink's own list-as-text quirk (`G28 ['X', 'Y', 'Z']`, audits/prusalink.md) is kept, as the reference does it |

Refusals (not printing, state checks) are PrusaLink's own in `controls.py`, `command_handlers.py`, `web/main.py:517-560` (S5).

### 3.3 What the hub did itself and PrusaLink does

`agent-engine.md:135-141`: `M72 S1`/`M72 S0` (`SetReady`/`CancelReady`, `command_handlers.py:595-628`) and
`M300 P200 S600` + `M117 ~Cannot re-print` (`RePrint`, :630-651). PrusaLink starts them from the firmware's `//action`
lines (`fw_set_ready`, `fw_cancel_ready`, `fw_reprint`, `prusa_link.py:639-656`) or from Connect commands. Proposal: the
port does these itself, from the firmware lines, as PrusaLink does; **typed messages are needed only for the hub-side
trigger** (ready, not ready, reprint). Question Q1.

### 3.4 The SDK's HTTP side (Connect)

`Printer.register`, `loop`, `loop_step`, `parse_command`, `make_headers` (`__init__.py:647-860`) and `MyPrinter.parse_command`
(`sdk_augmentation/printer.py`) speak to Prusa Connect. Proposal: **not ported**; the hub link (step 2) is the only link. Reopened by the new pin: whether the SDK's WebSocket side (`ws.py`) is ported and talks `/p/ws` to the hub is [OQ-45](../../design/risks-and-open.md#oq-45-how-the-mk3-port-reaches-the-hub).
The seam kept: `set_state`, `event_cb`, `telemetry`, `set_handler` (`__init__.py:290-340,619`), whose output goes to the
engine as `Event`s instead of a queue for HTTP. `Printer` is guarded by `if not self.token` (:315, :337), so a token-less
`Printer` would drop everything: the port must not rely on that. UNCLEAR: whether `Printer` stays as a class or its seam is
cut out (Q2).

### 3.5 UNCLEAR items: questions for the owner

| # | Item | Question (one line) | Recommended default |
|---|---|---|---|
| Q1 | `M72`, `RePrint` | Port them from the firmware `//action` lines only, no new typed message now? | yes, no new message |
| Q2 | SDK `Printer` | Keep it as the seam, with Connect HTTP not ported? | yes |
| Q3 | `storage.py`, `storage_controller.py`, `files.py` | Owner, 2026-11-20: `files.py` is ported with its inotify (`crates/inotify-simple`, a port of inotify_simple; later, owner 2026-11-25, #236: the crate notify instead, and `crates/inotify-simple` was removed). Before that: `StartPrint` needs the job file in `printer.fs` (`command_handlers.py:294`, `download_finished_cb` `prusa_link.py:509`): port `Filesystem` minimally (one folder holding `job-<id>`), no inotify, no tree for Connect? | yes |
| Q4 | `download.py` | Not ported (the hub link downloads, O6)? | not ported |
| Q5 | `lcd_printer.py` `M117` (`:523-533`) | Port the screen messages (a wire-visible thing PrusaLink does; S4 dropped download progress)? | port it, **before G2** (section L): `MyPrinter` needs `LCDPrinter` at once (`sdk_augmentation/printer.py:38`); changes Q5 "after the core", **for the proxy** |
| Q6 | `ip_updater.py` `M552` (`:166-182`) | Port it (shows the Pi's IP on the MK3 screen)? | port it, after the core |
| Q7 | `util.get_usb_printers` (pyudev) | `--mode prusa` names the port (B4): not ported, no detection? | not ported |
| Q8 | `Answer`, SD delete | Both refused if PrusaLink has none (to confirm in part 3)? | yes |

**Decided 2026-11-16 (owner delegated: "beantwoord de 4 vragen zelf voor me"):** Q1-Q8 as the recommended defaults
above. Checked against CLAUDE.md: Q1, Q5, Q6 keep what PrusaLink sends (Q6 after the core; Q5 changed, see section 4); Q2, Q3 are internal (no
wire change); Q4 follows O6 (the hub link downloads); Q7 follows B4; Q8 only if part 3 confirms PrusaLink has none.

## 4. Section order (part 3)

To be approved by the owner. Cut like the OctoPrint port (about 1,000 lines a section, a cheaper agent translates, I check
mechanically, the watchdog checks each; `agent-engine.md` "How it is built"). Smallest risk first; a section builds only on
earlier ones. Sizes are the reference's lines, counted in part 1.

**Settled from part 2:** PrusaLink has **no prompt answer**: the SDK's `dialog_action` only returns `{'source': CONNECT}` and
sends nothing (`Prusa-Connect-SDK-Printer/prusa/connect/printer/__init__.py:515-520`); `command_handlers.py` has no dialog
class. So `Order::Answer` is refused (Q8). It has **no SD delete**: `command_handlers.py` and `sd_card.py` send no `M30`;
the SDK's `delete_file` (:571-586) unlinks a file in the local tree (`files.py:45-59`), not on the card. So `SdDelete` is refused
(Q8, confirmed). "Refused" = a failed `Done`, "PrusaLink has no such control", as `Fan` (3.2).

Re-ordered after the architect's review (points 3, 4; **to be decided (proxy)**). Order: **0, A0, A, B1, B2, C, D, E, G1, E2, F2, F1, L, G2, I, H1, H2, J1, J2, K.**

| Sec. | Name and files (file:line) | Lines | Depends on | Proof |
|---|---|---|---|---|
| **0** | Move the shared Python ports to `ports/python/` (section 6): the Rust files, the data files beside them (`py_*_expected.json`, `py_*.txt`), and every script and doc that names the old paths: `scripts/octoprint-format-check.py` (:4-6, :26), `scripts/octoprint-percent-check.py` (:4-5, :35, :21), `docs/audits/octoprint-marlin.md` | move | nothing | all checkers re-run, the OctoPrint tests and its proof unchanged and green |
| **A0** | `config.py` 1-354 + `extendparser/get.py` 92 (section 6) | ~450 | 0 | settings call paths, defaults against Python |
| **A** | Constants and data: SDK `const.py` 1-267; PrusaLink `const.py` 1-359; `structures/` `regular_expressions.py` 190, `model_classes.py` 152, `module_data_classes.py` 159, `mc_singleton.py` 20, `heap.py` 170; `serial/instruction.py` 183; `updatable.py` 69; `util.py` (:105-250, :300-351); the libraries of section 5 (blinker, SortedKeyList, `deep_update`, Event) | ~1,500 + libs | 0, A0 | every regex, constant and G-code text extracted and compared (as `scripts/octoprint-patterns.py`); no reference test |
| **B1** | `serial/serial.py` 1-161, `serial_parser.py` 1-187, `serial/helpers.py` 1-109 | ~460 | A | `tests/test_serial_parser.py` (5); own serial against a pty |
| **B2** | Conditions: SDK `conditions.py` 1-285, PrusaLink `conditions.py` 1-537 (:88-123 and the `LinkError`s) | ~820 | A | SDK `tests/test_conditions.py` (15), `test_errors.py` (2) |
| **C** | `is_planner_fed.py` 1-216, `serial_queue.py` 1-674 (resend :375-420, reconnect :541-585, monitor :586-674), `structures/item_updater.py` 569 | ~1,460 | A, B1, B2 | harness run; `tests/test_item_updater.py` (15) |
| **D** | `serial_adapter.py` 1-427 (`_detect` :159, `_reopen` :188, `write` :330, DTR :359-410) | ~430 | B, C | harness run: open, 8 s wait, `PRUSA Fir` + `M862.2 Q`, DTR reset |
| **E** | `model.py` 41, `state_manager.py` 829, `print_stats.py` 106, `job.py` 299 | ~1,280 | A-D | harness run; no reference test |
| **G1** | SDK core: `command.py` 185, `models.py` (no cameras) ~150, `Printer` base :72-360 (`telemetry`, `event_cb`, `set_state`, `queue`, `is_initialised`) without the Connect HTTP, `files.py` minimal ~300 (Q3), `util.py` 63; and its **sink** (section 7) | ~1,100 | A, B2 | SDK `test_command.py` (11), `test_events.py` (3), `test_telemetry.py` (1), `test_printer.py` (41, the non-HTTP ones), `test_files.py` (46), `test_files_helpers.py` (4), `test_util.py` (5), `test_clock.py` (3) |
| **E2** | `filesystem/sd_card.py` 446, `sdk_augmentation/file.py` 31 (`printer_polling.py:33` imports `SDCard`) | ~480 | E, G1 | harness: `M20` list |
| **F2** | `telemetry_passer.py` 417 (:155-175 call `printer.telemetry`, `is_initialised`, `queue.qsize`), `mmu_observer.py` 132, `special_commands.py` 111 | ~660 | E, G1 (its imports: `config`, `const`, `util`, `model`, `model_classes`; none of `printer_polling`) | harness: same lines, order, gaps (`summarize.py`); `Prusa-Connect-SDK-Printer/tests/test_mmu.py` (5) |
| **F1** | `printer_polling.py` 1093, `auto_telemetry.py` 136 (`M155 S2 C7`), `keepalive.py` 64 (`M79`), `print_stat_doubler.py` 48 (imports `.printer_polling`) | ~1,340 | E, E2, G1, **F2** (`printer_polling.py:65` imports `TelemetryPasser`; :97, :105 take it in the constructor; `prusa_link.py:236-250`) | harness; the audit saw the first `M155` after 76 s, unexplained |
| **L** | `printer_adapter/lcd_printer.py` 585 (+ `structures/carousel.py` 241, `tests/test_carousel.py` 4): the screen messages (`M117` :523-533) | ~830 | C, E, G1 (its constructor takes the SDK `Printer`: `prusa_link.py` LCDPrinter(...)) | harness; `test_carousel.py` |
| **G2** | `sdk_augmentation/printer.py` 197 (`MyPrinter`; its constructor calls `LCDPrinter.get_instance()` :38 and `Keepalive.get_instance()` :39, imports :19-20, so both must exist) and `command_handler.py` 38 | ~240 | F1 (`Keepalive`), L (`LCDPrinter`), G1 | `test_printer.py` where it covers the augmented methods; harness |
| **I** | `web/controls.py` 38-233 (`api_printhead` :136-170 reads `model.state_manager.current_state` :141) | ~200 | A-E | same input to PrusaLink's functions and ours, compare the lines enqueued |
| **H1** | `command.py` 109, `command_queue.py` 130, `command_handlers.py` 115-475 and 595-724 (not :52-66, :476-557) | ~900 | A-G2, I | harness |
| **H2** | `file_printer.py` 415, `storage.py` 279, `storage_controller.py` 104 (minimal, Q3) | ~800 | H1 | harness: print a file, pause, resume, stop (`file_printer.py:209-250`) |
| **J1** | Top edge, wiring: `prusa_link.py` 133-394 (constructor) in `ports/prusalink/port.rs`: the sink, states table, `Order`s | ~600 | all above | harness end to end |
| **J2** | Top edge, callbacks: `prusa_link.py` 502-1100 | ~900 | J1 | harness end to end |
| **K** | After the core (Q6): `ip_updater.py` 195 (`M552` :166-182) | ~200 | J | harness |

**Proof by running PrusaLink itself** (as OctoPrint's `scripts/octoprint-port-check.py`): `scripts/mk3-harness/` is read, not
changed. `run_prusalink.py` starts the real PrusaLink on a serial port with the Linux-only modules stubbed; `fake_mk3.py`
is a fake MK3S on a pty, answers from Prusa-Firmware 3.14.1, records every host line with its time; `summarize.py` counts
each command, first time and gaps. Our side must be run against the same fake (the `TEST_TRANSPORTS` way in memory, as K15,
since a pty does not work on macOS) and compared in two layers (bytes in order; gaps within PrusaLink's own spread).
Timing is not deterministic in PrusaLink (threads), so gaps are compared as the set PrusaLink itself shows over several runs.

## 5. Libraries PrusaLink stands on (architect point 1)

Like CPython 3.13 for OctoPrint (`agent-engine.md` "Second source"), each is the reference's runtime, ported from its own
source where the port needs it. Each proposal is **to be decided (proxy/owner)**. Versions: `Prusa-Link/requirements.txt`;
the copies the harness runs are in `reference/.venv-prusalink/lib/python3.9/site-packages/` (installed: blinker 1.9.0,
sortedcontainers 2.4.0, pydantic 1.10.13, extendparser 0.3.1, Unidecode 1.3.8).

| Library | Pinned | Used in (file:line) | What the port needs | Proposal |
|---|---|---|---|---|
| `blinker` `Signal` | `~=1.5` (:5) | `serial_parser.py:17`, `serial_queue.py:15`, `serial_adapter.py:13`, `state_manager.py:9`, `file_printer.py:10`, `sd_card.py:13`, also `job.py:7`, `special_commands.py:6`, `mmu_observer.py:5`, `storage.py:11`, `storage_controller.py:10`, `ip_updater.py:8`, `item_updater.py:11` (13 files; `multi_instance/` not needed) | `Signal.connect` (incl. `weak=False`) and `send`; the exact list of methods used is made by a grep at the start of A | second source: port `blinker/base.py` (512 lines, 1.9.0) to the letter, only the methods used |
| `sortedcontainers.SortedKeyList` | `~=2.4.0` (:20) | `serial_parser.py:18`, used `:68` (`key=lambda item: -item.priority`) | `add`, iteration, `remove` order of equal keys | second source: port the `SortedKeyList` part of `sortedlist.py` (2,646 lines; only the used methods); the order of equal keys is the wire-visible bit (which handler runs first) |
| `pydantic` `BaseModel`, `utils.deep_update` | `==1.10.13` (:11) | `telemetry_passer.py:16-17` (`deep_update` :170), `model_classes.py:9`, `module_data_classes.py:9` (16 `BaseModel` classes under `printer_adapter/`) | plain attribute classes with defaults and `dict()`; `deep_update` is 10 lines | not a second source: the classes become Rust structs and `deep_update` a port of pydantic's function; **known gap** to list: pydantic's type coercion on assignment (a value of the wrong type is converted, not rejected), to be checked per class by a mechanical test |
| `multiprocessing.Event` / `RLock` | stdlib | `item_updater.py:5` (`Event` :195-197), SDK `command.py:3` (:42-43), SDK `conditions.py:16`, `interesting_logger.py:9` (not needed) | `set`, `clear`, `wait`, `is_set`; `RLock` | second source: CPython, as `py_threading` (they are `threading` primitives built on a semaphore); `util.py:247` `cpu_count()==1` (`is_potato_cpu`) is a platform call |
| `extendparser` `Get` | `~=0.3.1` (:6) | `config.py:11`, base of `Config` (:90) and `Settings` (:264) | `read`, `get_section`, typed getters | second source: port `get.py` (92 lines) with `config.py` (section A0) |
| `gcode_metadata` | `py-gcode-metadata` (:2); SDK `>=0.2.0`; copy in `reference/gcode-metadata` (1,119 lines, commit `7d8f25d`) | `sdk_augmentation/printer.py:8` (use :113 `FDMMetaData`), SDK `__init__.py:13` (use :545 `get_metadata`), `files.py:15` | only to tell Connect about a file (`SEND_FILE_INFO`, file tree): reaches the wire to Connect, not the printer | **not needed** if the hub link carries no file info (Q2, Q3); to be decided |
| `unidecode` | `~=1.3.6` (:21) | `util.py:153`, `lcd_printer.py:530` | G-code text | **decided (proxy, 2026-11-16, below, a)**: not repeated here |

**Python version.** PrusaLink: `python_requires>=3.9` (`setup.py:145`); SDK the same (`setup.py:52`). The harness venv runs
**3.9.6** (`pyvenv.cfg`). The Pi image: `printer-protocols.md:35` says Raspbian 11 (Python 3.9 as shipped, from my knowledge of
that release, not read here); `image_builder.py:41` names a Bookworm image (Python 3.11, same caveat). The OctoPrint port uses
CPython **3.13** (`reference/cpython-3.13`). Proposal: stdlib pieces the port needs come from 3.13 as already approved; whether 3.9 differs on them is **not judged by eye**: a checker `scripts/prusalink-python-check.py` runs the stdlib paths the port uses (`threading` `Event`/`RLock`/`Thread`/`Timer`, `queue`) on 3.9.6 (the harness venv) and on 3.13 (`reference/cpython-3.13`) with the same calls and compares what they do; any difference is a known gap in the audit. **To be decided (proxy/owner).**

## 6. Section A0: `config.py`, and where shared Python ports live (architect point 2)

**A0, `config.py` 1-354 (+ `extendparser/get.py` 92)**, before A, as the OctoPrint port's settings (78 `settings().get*`
paths checked against defaults). It holds the settings and defaults the other files read: `Config` (:90, the daemon ini:
`[daemon]` :101, `[http]` :153, `[printer]` :168 with port, baudrate, `reset_disabling`, settings path, `[cameras]` :198)
and `Settings` (:264, the printer's own settings file: `[printer]` :281, `[network]` :295, `[service::connect]` :299,
`[service::local]` :310). Needed for the MK3 port: only what serial, state, polling and file print read (`cfg.printer.*`,
`cfg.daemon.threshold_file`/`power_panic_file`, `cfg.daemon.printer_number`, `settings.use_connect()`); the web, camera and
Connect sections are not read (to be confirmed by the call-path grep). Proof: a checker lists every `cfg.`/`settings.`
read in the NEEDED files (as `scripts/octoprint-settings-paths.py` did), and compares the default of each with the Python
`Config`/`Settings` run on an empty ini, plus the user/home `pwd` calls (:108-125) written as not covered where our process
differs. No reference test exists. **To be decided (proxy/owner).**

**Where the shared CPython ports live** (`py_threading`, `py_queue`, `py_collections`, `py_attr`, `py_format`... now
`ports/octoprint/mod.rs:14-17`):
1. **Pure move to `crates/engine/src/ports/python/`** (tests unchanged, as HS0a), both ports `use` it.
2. Cross-port references (`prusalink` uses `octoprint::py_*`).

Recommendation: **option 1**, done before A as its own step with the OctoPrint proof run unchanged: the Python ports are
the second source of two references, not part of OctoPrint's, and option 2 would make the MK3 port depend on the Marlin
port. **To be decided (proxy/owner).**

## 7. The SDK `Printer` seam as a design (architect point 3)

**The problem.** `TelemetryPasser._update` -> `pass_telemetry` (`telemetry_passer.py:148-175`) sends nothing unless
(1) `Settings.use_connect()` (:151; it is `bool(service_connect["token"])`, `config.py:349-354`), (2) `printer.is_initialised()`
(:155; serial number, fingerprint and type all set, `__init__.py:260-267`), (3) no wizard pending (:159), and (4)
`printer.queue.qsize() < QUEUE_LENGTH_LIMIT` (:163, limit 4, :39). `Printer.telemetry` and `event_cb` themselves drop
everything without a token (`__init__.py:315`, `:337`). Only `Printer.loop_step` empties the queue (`:758-836`): `queue.get(timeout=
const.TIMESTAMP_PRECISION)` (:766), then it sends and handles the answer. Without a drain the queue fills at 4 and
telemetry stops.

**Design.** The port keeps `Printer` as the seam (Q2) and replaces `loop`/`loop_step` (:743-836) by **our top edge's drain
thread**, which takes from `printer.queue` at the same place (`queue.get(timeout=TIMESTAMP_PRECISION)`, :766) and, instead of
`item.send(...)` (:785), turns each `LoopObject` into engine `Event`s: a `Telemetry` item into `Event::Telemetry`, an `Event`
item (command results, job and state events) into `Event::Done`/`Event::JobEnd` by the table of part 2 (3.1). The Connect HTTP
parts (:786-835: the error conditions, `parse_command`, `Register`) are not ported (3.4). This is the same kind of seam as
the approved OctoPrint stand-in "`eventManager().fire` goes to a sink the port's top edge implements" (`agent-engine.md:236`,
approved by the owner 2026-11-12). Pass-through: the edge reads and changes nothing of what the SDK fills in.

**The guards: a named choice, no silent dummy.** The four checks of `pass_telemetry` and the token checks of
`telemetry`/`event_cb` and `loop_step` (:773-780) belong to the Connect link, which we do not have. Options:
1. **A dummy token and fingerprint** set on `Printer` so the unchanged code passes: the code stays identical, but the value is
   made up. Not recommended.
2. **The hub link's own credentials** as `printer.token`: a real value, but it ties the SDK to the hub's secret and makes
   `use_connect()` true by an accident of naming.
3. **A named difference in the ported file**: `Printer.telemetry`/`event_cb` and `pass_telemetry` ported without the lines
   :315, :337 and :151-161 (no token, no use_connect, no wizard), written in the file header as numbered differences, as the
   OctoPrint port lists its differences (28-32 in `comm.rs`). `is_initialised` (:155) stays and is fulfilled by PrusaLink's
   own `set_sn` (`prusa_link.py:888`) and `printer_type_changed` (:753); its fingerprint is `make_fingerprint(sn)` (`util.py:164`),
   a real value. The `qsize` check (:163) stays, with the drain keeping the queue short.

**Recommendation: option 3** (nothing reaches the printer; the changed lines are only checks for a link we do not have;
each is listed). **To be decided (proxy).** The wizard check (:159) and `Settings.is_wizard_needed` (:339) go with it.
Open for the proxy: whether `Event`/`Telemetry` items that the SDK builds only when a token exists (e.g. `event_cb` :315-328
adds `job_id`, transfer fields) must keep every field untouched: yes, as the engine passes them untouched.

## 8. When a control is done, and the state names (architect point 5)

**Controls have no command class.** `web/controls.py` only enqueues: `enqueue_instruction` makes an `Instruction` and calls
`queue.enqueue_one` (`serial/helpers.py:33-48`) and nothing waits. Rule, as the OctoPrint port's M4 (`agent-engine.md:202`:
"done once sent and `ok`-ed, the lines between as its output"; K13 :435, K11 :433): **a control's `CommandDone` is sent when
its last `Instruction` is confirmed**, i.e. when `SerialQueue._confirmed` (`serial_queue.py:420-433`) calls
`Instruction.confirm` (`instruction.py:56-66`, sets `confirmed_event`) and sends `instruction_confirmed_signal` (:433); the port
waits on the instruction's `wait_for_confirmation` (`instruction.py:91`, as `helpers.py:25-30` does). `output` is the lines
received between its send and that confirmation. A control that enqueues several lines (`jog`: `G90`/`G91` then `G1`,
`controls.py:81-88`; `extrude`: `M83` then `G1`, :130-133) is done at the last one. An instruction forced confirmed by a
flush (`serial_queue.py:520, :527`, `force=True`) ends its command as failed, "the queue was flushed" (ours, **to be decided
(proxy)**, like K18). A command class (`ExecuteGcode`, `StartPrint`, ...) is done as PrusaLink's `CommandQueue` finishes it.

**State names.** PrusaLink's own web shows (`web/main.py:51-59`): IDLE, READY, FINISHED as "Operational"; BUSY "Busy";
PRINTING "Printing"; PAUSED "Paused"; STOPPED "Cancelling"; ERROR and ATTENTION both "Error". Our table (3.1) differs:

| PrusaLink web | Ours (to be decided (proxy)) |
|---|---|
| STOPPED = "Cancelling" | Idle, `problem` "stopped" (our `JobEnd` carries the end) |
| ATTENTION = "Error" | Attention (kept apart from Error: the hub's pages treat them differently); text from `state_manager.py:505-530` |
| READY, FINISHED = "Operational" | Idle, `problem` "ready"/"finished" |

The state itself is `StateManager`'s (:261, `get_state`); the web's names are only its labels.

## 9. Proof tooling (architect point 6; all **to be decided (proxy)**)

As the OctoPrint port had it (`docs/audits/octoprint-marlin.md`), new scripts named for the MK3:
- **Patterns checker** `scripts/prusalink-patterns.py`, like `octoprint-patterns.py`: writes the regexes, constants and
  G-code texts of `regular_expressions.py`, `const.py` and the files of A as Python builds them, to a text file the Rust test
  compares to ours.
- **Same-input checker** `scripts/prusalink-port-check.py` (with a `prusalink_proof_env.py`), like `octoprint-port-check.py`
  and `octoprint_proof_compare.py`: runs real PrusaLink (`scripts/mk3-harness/run_prusalink.py`) against
  `fake_mk3.py` per scenario, 5 times, records every line the host writes with its time; `compare` checks ours against it and
  writes the audit's generated tables.
- **Citations checker** `scripts/prusalink-port-citations.py`, like `labelle-port-citations.py`: every `file:line` in
  `crates/engine/src/ports/prusalink/` must exist in the cited source and start a definition.
- **Bridge** `scripts/prusalink-fake-bridge.py`, like `octoprint-vp-bridge.py`: a pure byte pass-through from the Rust
  `TEST_TRANSPORTS` serial object (`comm.rs`) to `fake_mk3.py`'s answers (a pty does not work on macOS). `fake_mk3.py` itself
  is not changed (it is someone's uncommitted file: owner to say whether it is part of the proof).
- **Spread criterion** (like the OctoPrint "5 runs, `not reproducible`", audit lines 62-63): PrusaLink alone 5 times per
  scenario; per command the order of lines must be the same in all 5, else the scenario is `not reproducible` and on "not
  covered". "Within PrusaLink's own spread" = our gap between two sends lies between the smallest and largest gap PrusaLink
  showed for that pair over its 5 runs; our side also runs 5 times. 5 is the OctoPrint number, kept.
- **Audit file:** a new one, `prusalink-port.md` under `docs/audits/` (an index row in `docs/audits/README.md`). The old
  `docs/audits/prusalink.md` and `scripts/prusalink-audit-check.py` check the agent's and hub's own MK3 code by reading
  (`crates/agent` `marlin.rs`, `hub.rs`, which went in step 2): proposal, replaced and the old checker removed once the new audit
  is green; until then left alone. The hub's current MK3 path (its G-code builders, `M72` etc.) is already out
  (`agent-engine.md:129-141`).
- **Reference tests that are IN:** the lists of section 4, plus `Prusa-Connect-SDK-Printer/tests/test_mmu.py` (5 tests;
  `mmu_observer.py` is ported, F2).

## Not covered / not read (so far)

- Reference tests left out: `test_printer_loop.py` (the Connect loop, replaced by our drain, section 7), `test_download.py`
  (Q4), `test_small_gaps.py` (20 tests, not read: to be sorted by what each covers), the camera tests (`test_cameras.py`,
  `test_camera_*`: cameras not ported), `Prusa-Link/tests/test_ipc_queue.py` (multi-instance).
- Windows (proxy decision b): not covered; `serial/serial.py` is termios/fcntl.
- Q1's consequence (proxy decision c): the hub-side `M72`, RePrint and "cannot reprint" trigger is not covered; it needs a
  typed message later (an O8 extension).
- The libraries' gaps of section 5 (pydantic coercion, unidecode table) as named there; `gcode_metadata` not ported.
- The web, Connect HTTP, cameras, multi-instance, wizard, image builder, self-update: not ported (section 2).
- Behaviour on real hardware: the owner only (the MK3S "Gonzo", .65); not yet written.
- Files not read in full: `printer_polling.py`, `lcd_printer.py`, `sd_card.py`, `storage.py`, `file_printer.py` beyond the lines cited;
  every section re-reads its files fully before porting.

## Next parts (not written yet)

5. Hardware proof on the MK3S "Gonzo" (.65), only the owner; open questions; not covered / not read.

**Decided (proxy, 2026-11-16; the owner: "laat a, b en c beantwoorden door mijn vervanger", which covers the
reference choice in a):**
- a. `unidecode` (on the wire: `util.py:144-155` `get_gcode`, also `lcd_printer.py:530`): the reference is Python
  `unidecode~=1.3.6` (`requirements.txt:21`); the audit names the exact version it runs. A Rust port of its table only
  if a mechanical audit against the Python on our G-code shows it byte-identical; otherwise a known gap that fails
  loudly (an error on a non-ASCII character), never a value of ours. ASCII passes unchanged; `;` comments are cut first.
- b. Platform: PrusaLink's `Serial` is POSIX (`serial/serial.py:2-7`, termios, fcntl): the port runs on Linux and macOS;
  Windows is written as not covered; no serial layer of our own.
- c. Q1's consequence: the hub's "Taken" no longer sends `M72 S1` to an MK3 (`agent-engine.md:135-141`). Not covered:
  "hub-side M72/RePrint/'cannot reprint' trigger; needs a typed message later (O8 extension)".

**Decided (proxy, 2026-11-16, on the architect's second look):**
1. Section order 0, A0, A, B1, B2, C, D, E, G1, E2, F2, F1, L, G2, I, H1, H2, J1, J2, K: approved; Q5 becomes "before G2"
   (`sdk_augmentation/printer.py:38-39` needs `LCDPrinter` and `Keepalive`).
2. Section 0: FIX — move only the CPython ports the MK3 port uses (grep it; `mod.rs:14-17` holds `py_attr`,
   `py_collections`, `py_queue`, `py_threading`, `py_format` at :25) to `crates/engine/src/ports/python/`, with their data
   files and every script and doc path; `py_attr` (OctoPrint's `Attr<T>`) only if needed; the rest stays.
3. A0: approved; FIX — the `pwd` calls (`config.py:108-125`) a gap that fails loudly, never a value of ours.
4. Libraries: the principle (blinker, sortedcontainers, extendparser, pydantic's `deep_update` as ported second sources,
   a new class of exception to rule 2) is ONLY THE OWNER, the proxy recommends yes; the details approved (blinker 1.9.0
   used methods; `SortedKeyList` with the order of equal keys; pydantic as Rust structs, coercion gap per class failing
   loudly if reachable; Event/RLock via CPython; extendparser `Get` with `config.py`).
5. `gcode_metadata` not ported: approved (not covered).
6. Python version: approved (CPython 3.13 stdlib; `scripts/prusalink-python-check.py` on 3.9.6 and 3.13; differences as
   known gaps).
7. The `Printer` seam, option 3: approved (numbered differences; `is_initialised`, `qsize` kept; drain at `queue.get :766`).
8. A control done on its last confirmed `Instruction`: approved; FIX — say how a flush-forced confirm is detected (our
   edge flags the flush), as a numbered difference like K18.
9. State mapping approved; FIX — the `problem` text is PrusaLink's own state name verbatim (`STOPPED`, `READY`,
   `FINISHED`).
10. Tooling approved; ONLY THE OWNER: removing `docs/audits/prusalink.md` and `scripts/prusalink-audit-check.py` once the
    new audit is green, and whether the uncommitted `scripts/mk3-harness/fake_mk3.py` is part of the proof.

**Section 0, as #240 (2026-11-25):**
- The harness is back in `scripts/mk3-harness/` as a test tool, not agent code. `run_prusalink.py` puts the pinned
  checkouts first on the path, as the PrusaLink tests' drivers do (`crates/engine/src/ports/prusalink/driver_common.py`):
  PrusaLink `reference/Prusa-Link` (`websockets`, `171148ce`) and the SDK `reference/Prusa-Connect-SDK-Printer-c0d7d233`
  (`websockets`, `c0d7d233`). It refuses to run when a checkout is not at its pin; the venv's site-packages copies are not
  used. `websocket` (websocket-client, the SDK's link to Connect) is stubbed, as in the drivers; the harness config names
  no Connect.
- Seen running (macOS, 100 s): PrusaLink opens the fake, waits its 8 s (`serial_adapter.py:173`), detects it with
  `PRUSA Fir` + `M862.2 Q` (`:131`) and polls; `summarize.py` gives every command with its first time and gaps (the
  first `M155 S2 C7` again at 76.9 s).
- PrusaLink's own, not the fake's: `_get_info` (`serial_adapter.py:120-156`) returns once it has the name and the
  version, leaving the `ok` after `302` unread; that `ok` then confirms the queue's first instruction, so the first
  `printer_type` gather fails ("Printer responded with something unexpected", `printer_polling.py:476`) and two
  confirmations come unexpected (`serial_queue.py:427`). A real MK3S answers the same lines, so our port must do the same.
- Where the shared CPython ports live (section 6, decided 2026-11-16 point 2) stands: the ones the MK3 port uses move to
  `crates/engine/src/ports/python/`. Today nothing in `ports/prusalink/` (since #189) or `crates/connect-sdk` uses an
  `octoprint::py_*`, so the move is made in the first section that needs one (C: `py_queue`, `py_threading`), as its own
  commit before that section, with the OctoPrint checkers and proof run unchanged.
- `scripts/mk3-harness/fake_mk3.py` is part of the proof (owner, 2026-11-25: "fake_mk3.py ja"; decision 10).

**Decided (owner, 2026-11-16, "1 t/m 3 akkoord met voorstellen"):** (1) blinker, sortedcontainers, extendparser and
pydantic's `deep_update` are ported second sources, as the reference uses them (a new exception to rule 2, for the
reference's own runtime); (2) `docs/audits/prusalink.md` and `scripts/prusalink-audit-check.py` are removed once the new
audit is green; (3) `scripts/mk3-harness/fake_mk3.py` is part of the proof (committed with the port, when the owner asks).
