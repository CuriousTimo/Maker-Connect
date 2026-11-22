# Hub core v2: each agent port mapped onto the device contract (research, GitHub #14)

Status: research only, nothing built, nothing decided. Feeds the adapter lane after the contract crate
(`docs/archive/plans/hub-core-v2.md` §2.1). Rule used (§2.1): a capability verb maps to the reference's own call; a
verb the reference has no call for is not declared. One reference per device (CLAUDE.md), the one its module
header and audit name. "Not checked" means not verified. "Ours: profile" means a bound or default that is
ours, not the reference's, and needs the owner's approval before it is declared. Nothing here is a guess:
a row without a citation is marked.

Abbreviations. `OP/` = `reference/OctoPrint-1.11.8/src/octoprint/`. `LF/` = LaserFoundry **v0.5.0**
(commit `61febdb5d18f08f299f8501e44656da047a2ab09`, read with `git show v0.5.0:<path>` in
`reference/laserfoundry`; see finding F1). `PC/` = `reference/prusa-connect/`. `Act` = `Action` in
`crates/protocol/src/lib.rs:491-567`.

Counts (verbs / reference states / not declared):

| Port | Verbs | States | Not declared |
|---|---|---|---|
| OctoPrint (Marlin) | 18 | 16 | 9 (+ 14 reference calls with no UI today, §1.5) |
| labelle (LabelManager PnP) | 1 | 0 in the reference (3 ours) | 7 |
| DYMO CUPS driver (LW450 and LW320) | 2 (1 not checked) | 9 (3 set by the 450 code) | 5 |
| LaserFoundry (GRBL lasers) | 12 | 14 (10 machine + 4 job) | 16 |
| usbscale (M-series scale) | 0 | 9 (8 + unknown) | 4 |

---

## 1. OctoPrint 1.11.8 (plain Marlin, `--mode marlin`)

Reference: OctoPrint **1.11.8**, commit `42be740`, `reference/OctoPrint-1.11.8` (module header
`crates/engine/src/ports/octoprint/mod.rs:1-2`; audit `docs/audits/octoprint-marlin.md:3`). Ours maps each `Act`
to a `Printer` call after the refusals of the API endpoint that makes it (`ports/octoprint/port.rs:13-14, 568-732`).

### 1.1 Verbs

Names: Prusa Connect's `core.command.*` where the meaning is the same (§1.6), else `mc.printer.*` (ours, name
needs approval). Connect's argument bounds (`PC/docs/src/content/docs/capabilities.md`) are Prusa's and are NOT
OctoPrint's; where OctoPrint states a bound it is used, where it states none the cell says so.

| # | Verb (proposed) | Reference's own call (file:line) | Args: kind, unit, bounds, source |
|---|---|---|---|
| 1 | `mc.printer.gcode` (no Connect verb for raw G-code: checked the verb list `PC/internal/connector/xbuddy/verbs.go:25-60`) | `Printer.commands` `OP/printer/standard.py:463`; endpoint `OP/server/api/printer.py:393-443` (`command`/`commands`) | `text` string. No bound in the reference (`printer.py:415-430`). Split at `\n`, one command per non-empty line is OUR K1 (`port.rs:26-30`), needs approval. Needs operational (`printer.py:397`). |
| 2 | `core.command.home` | `Printer.home` `standard.py:553`; endpoint `printer.py:327-336` | `axis` string (Connect: optional, free text, `PC/.../capabilities.md:174-187`). Reference takes a LIST of `x`,`y`,`z` (`printer.py:308,331`, `standard.py:553-573`; checks that every axis is valid). Supported set `{x,y,z}`: reference. Mismatch: Connect has one `axis`, OctoPrint a list (§1.6). |
| 3 | `core.command.move` (X, Y) | `Printer.jog` `standard.py:510`; endpoint `printer.py:310-324` | `x`,`y` number, mm, relative (jog: `relative=True`, `printer.py:320-324`, `standard.py:529-536`; the sign is the direction). No bound in the reference. `feedrate`: reference optional (`printer.py:321`), default = the slowest axis speed of the printer profile (`standard.py:541`); Connect makes it required (integer, mm/min, `capabilities.md:209-243`) so the default would be ours: profile (needs approval). Whether Connect's `move` is relative: not checked. |
| 4 | `core.command.move_z` | same `jog` call, axis `z` (`printer.py:308,313-318`) | `distance` number, mm (Connect `capabilities.md:269-293`); `feedrate` as row 3. OctoPrint jogs X, Y, Z in one call; Connect splits Z out (mismatch). |
| 5 | `core.command.move_e` | `Printer.extrude` `standard.py:575`; endpoint `printer.py:133-142` | `length` number, mm, negative retracts (`printer.py:138-141`: only "is a number"). `feedrate`: reference optional, capped at the profile's E speed (`standard.py:582-589`); Connect requires it. Refused while printing (`printer.py:134-136`). |
| 6 | `core.command.set_nozzle_temperature` | `Printer.set_temperature` `standard.py:606`; endpoint `printer.py:100-114` | `nozzle_temperature` number, celsius, minimum 0 (`standard.py:614`), no maximum in the reference. Connect: integer 0..300 (Prusa's). `tool` string `toolN` (`printer.py:86,105-107`; Connect: integer index). Maximum = ours: profile. |
| 7 | `core.command.set_bed_temperature` | same call, heater `bed` (`standard.py:633-634`); endpoint `printer.py:187-195` | `bed_temperature` number, celsius, minimum 0, no maximum (`standard.py:614`). Refused without a heated bed in the profile (`printer.py:176-177`). Connect: integer 0..120. Maximum = ours: profile. |
| 8 | `core.command.set_speed` | `Printer.feed_rate` `standard.py:682` (M220); endpoint `printer.py:338-345` | `speed` percent. Reference: number, a float is multiplied by 100, minimum 1, no maximum (`standard.py:668-681`). Connect: integer 20..999. Bounds ours: profile. |
| 9 | `core.command.set_flow` | `Printer.flow_rate` `standard.py:689` (M221); endpoint `printer.py:144-151` | `flow` percent, as row 8. |
| 10 | `core.command.disable_steppers` | the control panel's "Motors off" button, `OP/templates/tabs/control.jinja2:82` (sends `M18` through `printer/command`, row 1) | no args. The reference has no API call, only the button: needs approval that a template button counts as "the reference's own call". Connect's `disable_steppers` sends `M84` (`PC/internal/connector/xbuddy/verbs.go:310`); that M18 and M84 mean the same: not checked. Refused unless operational and not printing is the button's rule (`control.jinja2:82`). |
| 11 | `mc.printer.fan` | control panel "Fan on"/"Fan off", `control.jinja2:83-84` (`M106 S255`, `M106 S0`) | `on` boolean. Same caveat as row 10. Connect has no part-fan verb (only `set_chamber_fan_target`). Name = ours. Note: `docs/archive/plans/agent-engine.md` M1 cites `control.jinja2:149-151` for these buttons; in this tree they are at `:82-84` (F2). |
| 12 | `core.command.start_print` | `select_file` `standard.py:696`, `start_print` `standard.py:765`; endpoints `OP/server/api/files.py:845-905` (select) and `job.py:32-38` (start) | `path` string required (Connect `capabilities.md:828-852`). OctoPrint selects by a name in ITS file manager (`files.py:140-144,171`, machinecode extension, `files.py:144`); our K6 hard link `job-<id>.d/<name>` (`port.rs:37-39`) is ours. `tool_mapping`: no reference call, not declared. Refused when already printing/paused (`job.py:33-37`), not operational (`job.py:16-17`), not ready (`files.py:150-154`). |
| 13 | `core.command.pause_print` | `Printer.pause_print` `standard.py:792`; endpoint `job.py:46-60` | no args (OctoPrint's optional `action` toggle/pause/resume, `job.py:52-60`, is not used by the port). |
| 14 | `core.command.resume_print` | `Printer.resume_print` `standard.py:808`; `job.py:57-58` | no args. OctoPrint's separate `restart` (start on a paused print, `job.py:39-45`) is not used. |
| 15 | `core.command.stop_print` | `Printer.cancel_print` `standard.py:824`; `job.py:61-67` | no args. |
| 16 | `mc.printer.sd_list` | `Printer.refresh_sd_files` `standard.py:1074`; `printer.py:372-373` | no args. Needs `feature.sdSupport` (`printer.py:357-358`), operational and not printing/paused (`printer.py:360-361`). Connect has `send_file_info` etc. but no "list the card" verb in the list read: name ours. |
| 17 | `core.command.delete_file` (origin sd) | `Printer.delete_sd_file` `standard.py:1052`; `files.py:1313-1343` | `path` string required: the card's name, only a listed file (`files.py:505-507`, `_verifyFileExists`). `origin` = ours (the reference separates local and sdcard by URL, `files.py:845`). Refused for a file in use (`files.py:1329-1330`). |
| 18 | `core.command.dialog_action` | the `action_command_prompt` plugin, answer path: not checked (file:line not read in this spike); port `ports/octoprint/action_command_prompt.rs`, `port.rs:868` | `button` index. Connect's `dialog_action` arguments: not checked. |

SD print (our `SdPrint`) is row 12 with an origin: `select_file(sd=True, printAfterSelect=True)`
(`standard.py:696`; `files.py:156-172`; our K7 `port.rs:40-41`). Needs approval: `origin` arg is ours.

### 1.2 States (reference)

`MachineCom.STATE_*` `OP/util/comm.py:445-460`; text `getStateString` `comm.py:965-1014`; groups
`OPERATIONAL_STATES` `comm.py:463-473`, `PRINTING_STATES` `comm.py:476-483`.

| Reference state (comm.py) | Text (comm.py:969-1014) | In OPERATIONAL | In PRINTING | Proposed name | Connect name, same meaning? |
|---|---|---|---|---|---|
| NONE 0 (445) | Offline | no | no | OFFLINE | none |
| OPEN_SERIAL 1 | Opening serial connection | no | no | CONNECTING | none |
| DETECT_SERIAL 2 | Detecting serial connection | no | no | CONNECTING | none |
| CONNECTING 3 | Connecting | no | no | CONNECTING | none |
| OPERATIONAL 4 | Operational | yes | no | IDLE | IDLE (devicejobs.go:71, §1.6) |
| STARTING 5 | Starting / Starting print from SD / Starting to send file to SD | yes | yes | STARTING | no (BUSY means something else, §1.6) |
| PRINTING 6 | Printing / Printing from SD / Sending file to SD | yes | yes | PRINTING | PRINTING |
| PAUSED 7 | Paused | yes | no | PAUSED | PAUSED |
| PAUSING 8 | Pausing | yes | yes | PAUSING | none |
| RESUMING 9 | Resuming | yes | yes | RESUMING | none |
| FINISHING 10 | Finishing | yes | yes | FINISHING | none (FINISHED differs, §1.6) |
| CLOSED 11 | Offline | no | no | OFFLINE | none |
| ERROR 12 | Error | no | no | ERROR | ERROR |
| CLOSED_WITH_ERROR 13 | Offline after error | no | no | OFFLINE_AFTER_ERROR | none |
| TRANSFERING_FILE 14 | Transferring file to SD | yes | no | TRANSFERRING_FILE | none |
| CANCELLING 15 | Cancelling | yes | yes | CANCELLING | none |

16 reference states. The port folds them to our six `MachineState` (`port.rs:249-269`, M3); whether the contract
declares 16, or fewer by grouping, is the owner's call (Question 3). The prompt plugin's open prompt is "Attention"
in our port (`port.rs:16-17`, M3); OctoPrint has no such comm state (it is the plugin's, not checked).

### 1.3 Which verbs a state accepts, per the reference

Source of each rule: the endpoint's own refusals (409 and the like). `operational` = state in OPERATIONAL (above);
`printing` = state in PRINTING.

| Verb (row) | Accepted in (reference) | Rule (file:line) |
|---|---|---|
| gcode (1) | OPERATIONAL, STARTING, PRINTING, PAUSED, PAUSING, RESUMING, FINISHING, TRANSFERING_FILE, CANCELLING | operational: `printer.py:397` |
| home, move, move_z (2-4) | OPERATIONAL, PAUSED, TRANSFERING_FILE | operational and not printing: `printer.py:302` |
| move_e (5) | same as 2-4 | `printer.py:72` (operational), `:134` (not printing) |
| set_nozzle / set_bed (6, 7) | all OPERATIONAL states | `printer.py:72-73, 173-174` (bed also needs a heated bed, `:176`) |
| set_speed (8) | all OPERATIONAL states, printing included | `printer.py:302` (`command != "feedrate"`) |
| set_flow (9) | all OPERATIONAL states, printing included | `printer.py:72-73` (tool endpoint: operational only) |
| disable_steppers, fan (10, 11) | the button's own: fan when operational, motors off when operational and not printing | `control.jinja2:82-84` |
| start_print (12) | OPERATIONAL (and TRANSFERING_FILE: not checked), when a file is selected | `job.py:16,25,33`; `standard.py:765-776`; file select needs `is_ready` `standard.py:926-932` |
| pause_print (13) | PRINTING group and PAUSED | `job.py:25,46-51` |
| resume_print (14) | PAUSED | `job.py:46-51`, `standard.py:808-822` |
| stop_print (15) | PRINTING group and PAUSED | `job.py:25,61-66` |
| sd_list (16) | OPERATIONAL, TRANSFERING_FILE (not printing, not paused) | `printer.py:360-361` |
| delete_file sd (17) | not in use by the print | `files.py:1329-1330`, `_isBusy` `files.py:519`: whether it needs operational: not checked |
| NONE, OPEN_SERIAL, DETECT_SERIAL, CONNECTING, CLOSED, ERROR, CLOSED_WITH_ERROR | none of the above (409 "not operational") | `printer.py:72,397`, `job.py:16` |

### 1.4 Not declared (our UI offers it today, the reference has no call)

| # | What | Where it is today | Why not declared |
|---|---|---|---|
| 1 | Load filament (`LoadFilament`) | `Act` `lib.rs:535-541`; refused `port.rs:722` ("OctoPrint has no such control") | OctoPrint has no call (searched `printer.py`, `standard.py` call list) |
| 2 | Unload filament | `Act` `:542-547`; `port.rs:723` | same |
| 3 | Bed mesh (`ProbeBed`) | `Act` `:548-549`; `port.rs:724` | same |
| 4 | Unlock (laser) | `port.rs:725` | laser verb |
| 5 | Set origin (laser) | `port.rs:726` | laser verb |
| 6 | Go to origin (laser) | `port.rs:727` | laser verb |
| 7 | Read settings (laser) | `port.rs:728` | laser verb |
| 8 | Frame (laser) | `port.rs:729` | laser verb |
| 9 | Start a job from a line (`from_line`) | `JobOrder.from_line` `lib.rs:427-430`; failed at `port.rs:49-52` (K3) | The API does not pass `pos` (`job.py:38`); `start_print(pos=...)` exists at `standard.py:765` but no endpoint uses it |

Connect verbs with no OctoPrint call (so never declared for Marlin): `load_filament`, `unload_filament`,
`mesh_bed_leveling` (`PC/internal/connector/xbuddy/verbs.go:41,52-53`).

### 1.5 Reference calls we do not use today (declare only if the owner wants them in the UI)

Tool select `printer.py:92-97`; temperature offsets `printer.py:117-130,198-206`; chamber target
`printer.py:231-270`; script `printer.py:432-441`; connect/disconnect/fake_ack `connection.py:40-72`;
SD init/release `printer.py:370-375`; file unselect `files.py:174-192`; job restart and pause toggle
`job.py:39-45,52-55`; file copy/move/slice/analyse `files.py:927-1183`; upload `files.py:535`. 14 calls, not checked
whether each is meaningful for a bare Marlin.

### 1.6 Spike: names against Prusa Connect

Source (read only, names are facts): verbs `PC/internal/connector/xbuddy/verbs.go:25-60`, arguments
`PC/docs/src/content/docs/capabilities.md` (cited per row above), declared state sets
`PC/internal/connector/xbuddy/states.go:100-154` and `PC/examples/adapters/moonraker/states.go:19-60`, state
vocabulary `PC/internal/app/devicejobs.go:71` (quoted there from Buddy's `printer_state.cpp:563`):
`IDLE, PRINTING, PAUSED, FINISHED, STOPPED, READY, ERROR, BUSY, ATTENTION, UNKNOWN`.

Same meaning (names usable): `home`, `move`/`move_z`/`move_e` (jog and extrude), `set_nozzle_temperature`,
`set_bed_temperature`, `set_speed`, `set_flow`, `disable_steppers`, `start_print`, `pause_print`, `resume_print`,
`stop_print`, `delete_file`, `dialog_action`; states IDLE (OPERATIONAL), PRINTING, PAUSED, ERROR.

Mismatches:
1. `home`: Connect `axis` single free string; OctoPrint a list (`printer.py:329-336`).
2. `move`: Connect requires `feedrate` (integer); OctoPrint's is optional with a profile default (`standard.py:541`). Z and E are separate verbs in Connect, one `jog` call and one `extrude` call in OctoPrint (`printer.py:310`, `:133`).
3. Types: Connect temperatures and rates are integers with Prusa's bounds (0..300, 0..120, 20..999); OctoPrint takes numbers, minimum 0 / 1 (`standard.py:614, 668-681`), no maximum.
4. `set_nozzle_temperature#tool`: Connect integer, OctoPrint string `toolN` (`printer.py:86`).
5. `start_print#path`: Connect a device path; OctoPrint a file manager name (`files.py:140-171`).
6. States FINISHED, STOPPED: OctoPrint has no such comm state; a finished or cancelled print is an event (`standard.py:1658` `on_comm_print_job_done`, `:1730` `on_comm_print_job_cancelled`), the comm state returns to OPERATIONAL. READY: no counterpart. UNKNOWN: comm.py returns "Unknown State" text only (`:1005-1014` region, text at the last return of `getStateString`).
7. BUSY: Connect uses it for a long move, e.g. homing (`PC/internal/app/devicejobs.go:71-78`, comment); OctoPrint's STARTING/PAUSING/RESUMING/CANCELLING/FINISHING are job transitions, not a move. Mapping them to BUSY (what `port.rs:14-20` does) changes Connect's meaning.
8. ATTENTION: Connect (dialog on screen over a print, `devicejobs.go:64-69`); OctoPrint: the prompt plugin, not a comm state.
9. No Connect verb for raw G-code, part fan, SD listing.
10. OFFLINE, CONNECTING, TRANSFERRING_FILE: no Connect state (whether Connect Local models "not connected" as a state: not checked).

---

## 2. labelle (DYMO LabelManager PnP)

Reference: labelle (Apache-2.0), commit `5e8f1e5d4f2f4447c8827df96c174812275d678f`, `reference/labelle`
(`ports/labelle/mod.rs:1-3`; audit `docs/audits/dymo-labelmanager.md:3-4`). Runtime pyusb 1.3.1 and Pillow 12.3.0
as the audit names them. Prefix `mc.label.`.

### 2.1 Verbs

| # | Verb | Reference's call (file:line, under `reference/labelle/src/labelle/`) | Args |
|---|---|---|---|
| 1 | `mc.label.print` | `labelle --picture FILE --tape-size-mm N`: `cli/cli.py:108` (`default`), picture `:224`, build `DymoLabeler(tape_size_mm, device)` `:590`, print `:613` -> `DymoLabeler.print(bitmap)` `lib/devices/dymo_labeler.py:307` -> `print_label` `:216` | `picture` opaque (ours: the hub's job file, a PNG, one pixel per dot, `JobOrder` `protocol/src/lib.rs:431-434`). `tape_size_mm` integer, mm, supported `{19,12,9,6}` (`dymo_labeler.py:243`), default 12 (`:244`, used when none given `:251-252`), other values raise (`:253-257`). Bounds: reference. |

Other options the reference has and our `JobOrder` does not carry (so the UI cannot send them today):
`--margin-px` `cli.py:227` (default 56, `lib/constants.py:64`), `--justify` `:154`, `--min-length` `:166`,
`--max-length` `:173`, `--fixed-length` `:180`, `--output` `:187`, text/QR/barcode elements. Declare only on the owner's say.

### 2.2 States

The reference has no state set. It has: "ready" = a device is attached, `DymoLabeler.is_ready` `dymo_labeler.py:304`;
three errors: `DeviceManagerError` `lib/devices/device_manager.py:17`, `DymoLabelerDetectError` `dymo_labeler.py:26`,
`DymoLabelerPrintError` `dymo_labeler.py:32`; the printer's status answer is read and only logged
(`_get_status` `:211`, `LOG.debug(...)` `:234`), its bytes are not interpreted. 0 states in the reference.

Proposed (all ours, L1, L7, L10: `ports/labelle/port.rs:1-11, 133-138`): DISCONNECTED, IDLE, BUSY. Needs the owner's approval as a declared
set. `print` accepted in IDLE; in BUSY the port is one thread, a job waits (`port.rs:147-148`). The reference's per-state rules: none.

### 2.3 Not declared

1. Pause, resume, stop: labelle prints in one go (`port.rs:26-27`, L5).
2. Typed commands (`Order::Command` failed "labelle has no such control", `port.rs:24-25`, L4).
3. Cassette present and battery percent read from the status bytes (`port.rs:91-104`, L6): labelle does not interpret these bytes (`dymo_labeler.py:234`). The owner decided this on 2026-11-16; kept as an approved exception, not a reference fact.
4. The 15 s idle status poll `ESC A` (`port.rs:21-22,139-148`, L11): owner's exception; labelle sends nothing while idle.
5. Feed and cut: only private mid-level helpers exist (`_cut` `dymo_labeler.py:175`, `_skip_lines` `:192`, `_chain_mark` `:186`), no call from the CLI: not declared (per §2.1 "feed where the reference has it": it does not).
6. Copies: labelle has none; the hub queues one job per copy (`crates/hub/src/files.rs:868-885`).
7. List devices (`cli.py:94-105`, hidden command): not a user operation of ours.

---

## 3. DYMO CUPS driver 1.4.0 (LabelWriter 450 and 320)

Reference: DYMO's own CUPS driver **1.4.0**, sha256 `c60797e7e986ca329f46e9a6ab1cb6382383952b15685ed69fd91f3c7ed64f71`,
`reference/dymo-cups-drivers-1.4.0` (filter `raster2dymolw`, `ppd/lw450.ppd`, `ppd/lw320.ppd`); runtime CUPS
v2.4.20 commit `ef80eea`, `reference/cups-2.4.20` (`ports/dymo_cups/mod.rs:1-9`; audits
`docs/audits/dymo-labelwriter-450.md:3-9`, `dymo-labelwriter-320.md`). The `Open/Default` lines of the two PPDs are
identical (compared in this spike), so one table serves both. Paths below are under `src/` of the driver unless a PPD.

### 3.1 Verbs

| # | Verb | Reference's call (file:line) | Args |
|---|---|---|---|
| 1 | `mc.label.print` | the filter run `raster2dymolw job-id user title copies options [file]`: `common/CupsFilter.h:69-99` (usage `:75`), `lw/raster2dymolw.cpp:56` | `page_size` string, supported = the PPD's `PageSize` choices, default `w167h288` (`ppd/lw450.ppd:147-149`); ours: which one a roll takes (`JobOrder.page_size`, W2b, `ports/dymo_cups/page.rs:131-138`). `label_length_mm`: reference has none (the page size is the length); choosing the `PageSize` by length is OURS (W2, `page.rs:11-12`), needs approval. `picture` opaque (ours: PNG job file, W1). |
| 2 | `mc.label.cancel` (not checked) | CUPS cancel of the job: the scheduler terminates the filter; the driver sees `jsDeleted` (`lw/LabelWriterLanguageMonitor.cpp:247`, `common/PrinterDriver.h:59`) | none. Our Stop = terminating the filter (W6, `port.rs:9,149`). The CUPS side (`cups-2.4.20`) file:line: not checked. |

Options the reference has and our UI does not offer today (`JobOrder` has none): `Resolution` `ppd/lw450.ppd:415-419`
(`300dpi`, `300x600dpi`, default 300dpi), `DymoHalftoning` `:422-427` (`Default`, `ErrorDiffusion`, `NLL`; default
ErrorDiffusion), `DymoPrintDensity` `:476-482` (`Light`, `Medium`, `Normal`, `Dark`; default Normal, applied
`lw/CupsFilterLabelWriter.cpp:51-62`), `DymoPrintQuality` `:540-544` (`Text`, `Graphics`; default Text, applied
`CupsFilterLabelWriter.cpp:39-49`). The port uses the PPD defaults. The rolls catalogue
`crates/engine/src/ports/dymo_connect_lw450.rs` is DATA (SKU to `PageSize`, labels per page) generated from DYMO Connect
1.6.4.18, not a reference of calls (header lines 1-4, owner approval 2026-11-17); it declares no verb or state.

### 3.2 States

Reference: `job_status_t` `common/PrinterDriver.h:54-66`: `jsOK, jsPaperOut, jsError, jsDeleted, jsPaperSizeError,
jsPaperSizeUndefinedError, jsHeadOverheat, jsSlotStatusError, jsBusy`; as `STATE:` keywords
`common/CupsPrintEnvironment.cpp:160-190` (`none`, `com.dymo.out-of-paper-error`, `com.dymo.general-error`,
`...paper-size-error`, `...paper-size-undefine-error`, `...head-overheat-error`, `...slot-status-error`,
`...busy-error`); reason texts `ppd/lw450.ppd:47-55`. The 450 code sets only `jsOK`, `jsPaperOut`, `jsError` from the status byte
(`lw/LabelWriterLanguageMonitor.cpp:261-271`; bits `LabelWriterLanguageMonitor.h:40-44`: `0x02` TOF, `0x08` roll
changed, `0x20` paper out, `0x40` paper feed, `0x80` error); `jsDeleted` is read at `:247`; the other four are defined and mapped
but no non-test code under `lw/` or `common/` sets them (grep). 9 reference states, 3 set.

Proposed: IDLE (ours: nothing is sent while idle, `port.rs:7`), PRINTING (ours, a job runs, `port.rs:245`), OUT_OF_PAPER
(`jsPaperOut`), ERROR (`jsError`). The port shows the PPD reason text of the keyword as `problem` (W7, `port.rs:117`). IDLE and
PRINTING are ours: needs approval. `print` accepted in IDLE; in PRINTING a new job waits (`port.rs:136`). OUT_OF_PAPER, ERROR:
reference rule: the filter stops the page (`common/CupsFilter.h:123` ends when the status is not `jsOK`); whether a new `print`
is accepted then: not checked.

### 3.3 Not declared

1. Typed commands: "the DYMO driver has no such control" (`port.rs:26-27`, W6).
2. Pause and resume: none in the driver (`port.rs:28-29`).
3. Copies: the filter's `copies` argument is only counted in the usage check (`CupsFilter.h:75`; grep finds no other use); the hub queues copies (`files.rs:868-885`).
4. Answer to a question: none (`port.rs:30-31`).
5. `raw_status` hex of the status byte as a field (W7, `port.rs:100-104`): ours, nothing in the driver offers it as a value.

---

## 4. LaserFoundry v0.5.0 (GRBL lasers, `--mode grbl`)

Reference: LaserFoundry (MIT OR Apache-2.0) v0.5.0 commit `61febdb5d18f08f299f8501e44656da047a2ab09`
(`ports/laserfoundry/mod.rs:1-6`; audit `docs/audits/laserfoundry-grbl.md:3-6`). Calls are `Link` methods
(`LF/crates/device/src/link.rs`) and the app's controls (`LF/crates/ui-egui/src/`). Prefix `mc.laser.`. Our mapping: `port.rs:6-47, 99-132`.

### 4.1 Verbs

| # | Verb | Reference's call (file:line) | Args: kind, unit, bounds, source |
|---|---|---|---|
| 1 | `mc.laser.gcode` | `Link::send_line` `link.rs:161`; app `panels.rs:865` (console) | `line` string, at most 79 bytes (`stream.rs:10` `MAX_LINE_BYTES`; check `link.rs:166`), no `\n \r \0`, no forbidden byte; empty line sends nothing; refused while a job runs (`link.rs:169,277`). One call per line (our `text.lines()`, `port.rs:101`, is ours: G1). |
| 2 | `mc.laser.jog` | `Link::jog` `link.rs:197`; app `panels.rs:813` | `x`,`y` number, mm, relative, each `abs <= 1000` (`link.rs:26` `MAX_JOG_MM`; check `:306-310`); `feed` number, mm/min, `1..=30000` (`link.rs:27`). The UI's steps `[0.1,1,10,50]` (`panels.rs:797`) and feed 10..30000 (`panels.rs:802`) are the app's UI, ours to copy or not. Wire: `$J=G91 G21 X.. Y.. F..`. No Z: the reference has none (our G2). |
| 3 | `mc.laser.home` | `Link::home` `link.rs:222` (`$H`); `control_box.rs:158`, `panels.rs:838` | no args; one homing cycle, axes not choosable (our G2). |
| 4 | `mc.laser.unlock` | `Link::unlock` `link.rs:230` (`$X`); `control_box.rs:160-161` (only shown in Alarm), `panels.rs:841` | no args. |
| 5 | `mc.laser.set_origin` | `Link::set_zero` `link.rs:238` (`G92 X0 Y0`); `control_box.rs:164` | no args. |
| 6 | `mc.laser.go_origin` | `Link::go_to_zero` `link.rs:243` (`G90 G0 X0 Y0`); `control_box.rs:167` | no args. |
| 7 | `mc.laser.read_settings` | `Link::request_settings` `link.rs:184` (`$$`); `settings_ui.rs:60` | no args. GRBL only (`link.rs:269`). |
| 8 | `mc.laser.frame` | the app's `frame_action` `panels.rs:734-740` -> command `job.frame` -> `gcode::frame` `LF/crates/gcode/src/lib.rs:92` (bounds, machine, placement, speed, power) -> `start_job` | `power` percent `0..=100` (`panels.rs:778`), default 0 (`lib.rs:416`). The reference frames the DESIGN's bounds; ours frames a file's extent (G4, G16) and a fixed 3000 mm/min (G12): the speed is ours (the reference's frame speed: not checked), needs approval. |
| 9 | `mc.laser.start` | `start_job_with` `ui-egui/src/lib.rs:1221-1251` -> `Link::start_job` `link.rs:250` | `program` opaque (ours: the job file). `from_line` integer, 1-based, optional: `start_from_line` `lib.rs:1258-1266` with `resume_from` (G5). Refusals: no link (`lib.rs:1222`), protocol changed (`:1227`), state not ready (`not_ready` `lib.rs:1520-1532`), laser mode `$32` off (`lib.rs:1239-1247`), job already active (`link.rs:277`). The app's confirmation for non-absolute starts (`lib.rs:1250`): not covered (our audit). |
| 10 | `mc.laser.pause` | `Link::pause` `link.rs:255`; `control_box.rs:144`, `panels.rs:772` | no args; shown only while a job is active (`control_box.rs:138-145`). |
| 11 | `mc.laser.resume` | `Link::resume` `link.rs:259`; `control_box.rs:141`, `panels.rs:769` | no args. |
| 12 | `mc.laser.stop` | `Link::stop` `link.rs:265` (GRBL soft reset, laser off); `control_box.rs:147-149` ("always there"), `panels.rs:775` | no args; accepted in every state, also with no job (`control_box.rs:147`). |

Reference calls we do not use: `write_setting` `link.rs:190` (`$id=value`, UI `settings_ui.rs:89,99`), `jog_to` `link.rs:213`,
`send_lines` `link.rs:174`. Declare only if the owner wants them.

### 4.2 States (reference)

Machine: `MachineState` `LF/crates/device/src/status.rs:3-14` (at v0.5.0): Idle, Run, Hold, Jog, Alarm, Door, Check, Home, Sleep,
Unknown (10; from the `?` status report). Job: `JobState` `link.rs:32-38`: Idle, Running, Paused, Finished(Ok(lines) | Err(reason)) (4).
Proposed set: these 14, names as the reference's (upper-cased for the contract: owner's call). The plan's "framing" and "loaded" (`hub-core-v2.md:54`)
are NOT states of the reference: a frame is a job, a loaded file is the app's document (F3); if kept they are ours and need approval.

Which verbs a state accepts, per the reference:

| Verb | Rule in the reference |
|---|---|
| start (9) | machine Idle or Unknown only (`not_ready` `lib.rs:1520-1532`: Alarm, Hold, Door, Home, Run, Jog, Check, Sleep refuse with the app's texts) and job not Running/Paused (`link.rs:277`); `$32=1` (`lib.rs:1239-1247`) |
| gcode, jog, home, set_origin, go_origin, read_settings, unlock (1-7) | `Link` refuses only when a job runs (`require_idle` `link.rs:277`, `LinkError::Busy` `link.rs:108`); the app shows Home, Set origin, Go to origin, Frame only while no job is active (`control_box.rs:150-168`), Unlock only in Alarm (`control_box.rs:160`) in the control box (the Laser panel shows Unlock always, `panels.rs:841`). The controller's own refusal in each machine state is GRBL's, not LaserFoundry's: not checked here. |
| frame (8) | no job active (`control_box.rs:150-152`) |
| pause (10) | a job active (`control_box.rs:138-145`) |
| resume (11) | job Paused (`control_box.rs:140-141`) |
| stop (12) | every state (`control_box.rs:147-149`) |

### 4.3 Not declared (16)

1. Z jog (`port.rs:103-105`, G2): the reference has none.
2. Home with chosen axes (`port.rs:112-113`, G2): one cycle only.
3-14. The twelve FDM actions the hub still offers for any machine: `Extrude`, `Temperature`, `Fan`, `Speed`, `Flow`, `MotorsOff`, `LoadFilament`, `UnloadFilament`, `ProbeBed`, `SdList`, `SdPrint`, `SdDelete` (`port.rs:119-130`, "LaserFoundry has no such control"). Whether the hub shows them for lasers: not checked (profile decides).
15. State "framing" (plan), no reference state.
16. State "loaded" (plan), no reference state.

Also ours but not on this list (no wire effect, audit §4): command answers read from the console (G7, G9), report texts (G8, G11), job end on a lost connection (G13), `--baud` default (G14).

---

## 5. usbscale (DYMO M-series scale)

Reference: usbscale (GPL-3.0-or-later) commit `9ed1cb55a2dfd0daeb41548031a49ab756cb9114`, `reference/usbscale`
(`ports/usbscale/mod.rs:1-4`, `reference/usbscale/SOURCE.txt`; audit `docs/audits/usbscale.md`). Kind "scale": no verbs
(§2.1: "state only: weight, stable").

### 5.1 Verbs

0 declared. The reference has one call: `zero` (tare) `usbscale.c:113-114,142-143`, sends output report `{0x02, 0x02}` to
OUT endpoint 2, `usbscale.c:257-272`. The port sends nothing to the scale (`ports/usbscale/mod.rs:116-118`, E-decision); declaring
`mc.scale.zero` would be a change of that decision (Question 7).

### 5.2 States (reference)

The status byte of the weigh report (`dat[1]`, `usbscale.c:381-382`), switch `:406-452`; report id must be 3 or 4 else
"Error reading scale data" (`:396-398`); unit byte `dat[2]` indexes `UNITS[13]` (`:88-106`), exponent `dat[3]` (`:386`), weight
`dat[4..5]` little endian (`:388-392`):

| Byte | Reference text | Final? |
|---|---|---|
| 0x01 | Fault (stops) `:407-409` | stops the program |
| 0x02 | Scale is zero'd `:410-413` | wait |
| 0x03 | Weighing `:414-417` | wait |
| 0x04 | the weight is printed `:419-431` ("the only final, successful status") | final: stable |
| 0x05 | Under Zero `:432-435` | wait |
| 0x06 | Over Weight `:436-439` | wait |
| 0x07 | Calibration Needed `:440-443` | wait |
| 0x08 | Re-zeroing Needed `:444-447` | wait |
| other | Unknown status code n `:448-451` (stops) | stops |

9 states (8 + unknown). "stable" = 0x04, per the reference's own comment. No verb is accepted in any state (no verbs).

### 5.3 Not declared (4)

1. `zero` (tare): reference has it, port does not (5.1).
2. `--human` lbs and oz (`usbscale.c:117,424-428`): not ported.
3. `--index` (`usbscale.c:116`) and the VID:PID table `scales.h`: replaced by our usage page 0x8D selection (E2).
4. Reading on after a stop-status (our E3): the reference stops on 0x01 and unknown; ours reads on and reports (`ports/usbscale/port.rs:6-8`). Not a verb; listed because the state set differs in behaviour.

---

## 6. Needs the owner's approval

1. Raw G-code verb name `mc.printer.gcode`, fan `mc.printer.fan`, SD list `mc.printer.sd_list` (no Connect verb), the `origin` arg of delete/start.
2. Declaring a template button (Fan, Motors off) as the reference's call (rows 10, 11).
3. Bounds that are not the reference's: maxima for nozzle, bed, speed, flow (OctoPrint has none); default feed and E speed from the profile.
4. K1 (line splitting of typed G-code), K6 (job file link), K7 (SD print by select-and-print) as part of the contract.
5. The declared state set for Marlin (16 OctoPrint states, or grouped) and what BUSY means (§1.6, 7).
6. Laser: the frame speed (3000 mm/min) and the file-extent frame; whether "framing" and "loaded" stay as states.
7. Label: the set IDLE/BUSY (labelle) and IDLE/PRINTING/OUT_OF_PAPER/ERROR (DYMO) as declared states; the cassette and battery fields (L6).
8. Whether any reference option with no UI today (density, quality, halftoning, resolution; margins, justify, lengths; `write_setting`; the 14 OctoPrint calls in §1.5; scale `zero`) becomes a verb.

## 7. Findings (outside the rows)

- F1. `reference/laserfoundry` is checked out at `3b9b9fa` on `main` (`Cargo.toml` version 0.2.1), NOT at the audited v0.5.0 commit `61febdb`: files such as `crates/ui-egui/src/control_box.rs` do not exist in the working tree. This spike read v0.5.0 through `git show v0.5.0:<path>`; the line numbers match the audit's. Whoever opens the folder sees the wrong tree.
- F2. `docs/archive/plans/agent-engine.md` M1 (and `docs/archive/plans/hub-core-v2.md`) cite `control.jinja2:149-151` for the fan and motors buttons; in `reference/OctoPrint-1.11.8` they are at `control.jinja2:82-84`.
- F3. `docs/archive/plans/hub-core-v2.md:54` lists laser states "idle, framing, loaded, running, alarm"; the reference has the 10 GRBL machine states and 4 job states of 4.2.
- F4. The 450 audit calls the filter's reference CUPS v2.4.20; the CUPS-side cancel path (row 2 of §3.1) was not read in this spike.
- F5. Prusa Connect names used: from `reference/prusa-connect` as it is on disk (an OCL tree: names and meanings only, no text copied).

## Owner's answers (2026-11-18: "akkoord met alle vijf adviezen")

1. Fan and Motors off: declared, with exactly the G-code of OctoPrint's control template (`control.jinja2:82-84`).
2. Marlin: OctoPrint's 16 states declared as they are; mapping to Prusa Connect names only in the API layer.
3. Laser frame: through the reference's `job.frame` (the design's bounds, power default 0 %, laser off); our fixed 3000 mm/min and file-extent frame go. "framing" and "loaded" are not states.
4. Scale: no `zero` verb; nothing is sent to the scale (earlier decision kept).
5. DYMO Connect roll catalogue: kept as the approved data source.

## Step 1 as built (#14): not covered

- Not declared, because the map marks them "not checked": Marlin `dialog_action`, DYMO cancel.
- Declared without a check against the reference: Marlin `delete_file` accepted in every OPERATIONAL state.
- `mc.laser.frame` is in the registry with the reference's `job.frame` meaning, but the grbl adapter does not
  serve it yet: the port still frames at a fixed 3000 mm/min over the file extent (owner answer 3 removes
  that; a wire change, own sub-issue). A frame with `power` above 0 needs a person, like `mc.laser.start`.
- Laser states: the 10 GRBL machine states only; the 4 job states are not declared (IDLE would be there twice).
- LabelWriter 320: no adapter; the engine has no 320 port yet (own sub-issue).
- Marlin job start settles at the job's end: the adapter has no observed-start signal yet; fixed with the
  command records (step 4).
- The action each verb needs is provisional until the presets of step 3.
