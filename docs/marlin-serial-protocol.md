# Marlin over USB: everything a printer can say, and what we send

> **Not verified (2026-11-09).** This document was written by reading the sources by hand,
> not checked item by item. The check against PrusaLink (`prusalink-audit.md`,
> `scripts/prusalink-audit-check.py`) found many gaps that such a reading had missed. Until a
> script checks this document against its sources in the same way, it is not to be taken as
> complete.

What the agent (`crates/agent/src/marlin.rs`, `print.rs`) has to understand from a printer
that speaks the Marlin serial protocol, and which G-code it may send to which firmware.
Read on 2026-11-08 in the sources themselves, ideas and facts only, nothing copied:

| Source | What it is | In `reference/` |
|---|---|---|
| Marlin `bugfix-2.1.x` (2026-11-07) | upstream Marlin 2 | `Marlin` |
| Prusa-Firmware (MK3/MK3S/MK2.5) | Prusa's Marlin 1 fork | `Prusa-Firmware` |
| Prusa-Firmware-Buddy (MINI, MK4, XL, CORE One over USB) | Marlin 2.0 inside Buddy | `Prusa-Firmware-Buddy/lib/Marlin` |
| OctoPrint (`serial_comm.py`) | the reference host | `OctoPrint` |
| PrusaLink (`prusa/link/serial`, `regular_expressions.py`) | Prusa's own host for the MK3 | `Prusa-Link` |
| Prusa-Error-Codes | Prusa's error code list (`#17xxx` etc.) | `Prusa-Error-Codes` |

Not covered: Klipper (spoken to through Moonraker, not over serial), Repetier-, Smoothie- and
RepRapFirmware (other protocols); a firmware built with unusual options can still say things
not listed here. Unknown lines are always shown, never acted on.

## The conversation

- Every line we send: `N<n> <code>*<checksum>`, checksum = XOR of every byte before `*`. One
  line in flight; the next once the printer answered `ok`.
- `ok`, possibly followed by values: `ok T:210.0 /210.0 B:60.0 /60.0 @:64 B@:0`, or with
  `ADVANCED_OK` (Marlin): `ok N12 P15 B3`.
- A line damaged or out of order (all three firmwares, same words):
  `Error:Line Number is not Last Line Number+1, Last Line: N`,
  `Error:checksum mismatch, Last Line: N`, `Error:No Checksum with line number, Last Line: N`,
  MK3 also `Error:No Line Number with checksum, Last Line: N`; then `Resend: N+1`, then `ok`.
  Some firmware variants forget that last `ok` (OctoPrint waits 1 s and goes on).
- **A line that arrives twice** (its number already done) is dropped by Marlin **without
  any answer**. A line must therefore never be sent again unless the printer asks.
- `Resend: N` / `Resend:N` (Marlin, Prusa), `rs N` / `rs N12` (others): send N and every line
  after it again. A resend for a line not sent yet means the last line arrived twice: carry
  on. The same line asked for again and again: the link is broken.
- `wait` (Marlin with `NO_TIMEOUTS`): idle with an empty queue. While we wait for an `ok`, the
  `ok` (or the line) was lost.
- `busy: processing`, `echo:busy: processing` (`HOST_KEEPALIVE`, every 2 s by default): still
  working. `busy: paused for user` and `busy: paused for input`: **waiting for someone**
  (a button, `M0`, a filament change, a question on its screen).
- `start`: it (re)started; everything sent before is gone.
- `echo:Unknown command: "X1"` (Marlin, MK3), `Unknown M code: ...` (older): refused.
  `echo: cold extrusion prevented`, `echo: too long extrusion prevented`: a move refused.
- Heating on the MK3: `T:210.0 E:0 B:60.0`, `T:180.3 E:0 W:?` every second while `M109`/
  `M190` wait (no `busy` then).

## Host actions (`//action:`, the MK3 sometimes `// action:`)

| Action | From | Means |
|---|---|---|
| `pause`, `resume`, `cancel` | all | asked at its knob: the host pauses, resumes, stops |
| `paused`, `resumed` | all | it paused or resumed itself (filament change, runout) |
| `start` | all | "print again" from its menu |
| `prompt_begin <text>`, `prompt_button <label>` (old: `prompt_choice`), `prompt_show`, `prompt_end` | Marlin, Buddy | a question on its screen; answered with `M876 S<button index>` |
| `notification <text>` | all | its status line (`M117`) |
| `poweroff` (ACTION_ON_KILL) | Marlin, Buddy | it was killed: dead until reset |
| `shutdown` | all | "shut the host down": never acted on |
| `filament_runout [tool]` | Marlin | runout |
| `probe_rewipe`, `probe_failed` | Marlin | G29 retrying, G29 failed |
| `safety_timer_expired` | Buddy | heaters switched off after inactivity |
| `ready`, `not_ready` | MK3 | "Set Ready" in its menu |
| `uvlo_recovery_ready`, `uvlo_auto_recovery_ready` | MK3 | a print brought back after a power cut |
| `dump_available` | MK3 | a crash dump is ready |
| `sd_inserted`, `sd_ejected`, `sd_updated` | some | its card |

## Fatal: it stopped and must be reset

| Words | From |
|---|---|
| `Error:Printer halted. kill() called!` | all |
| `!! KILL caused by …` (Marlin), `KILL caused by …` (Buddy) | Marlin, Buddy |
| `Error:Printer stopped due to errors. Fix the error and use M999 to restart.` (Marlin, Buddy), `…Supervision required.` (MK3); afterwards every move answers the same line as `echo:` | all |
| `!! STOP called because of … error - restart with M999` (Marlin), `STOP called because of unhomed error` (Buddy) | Marlin, Buddy |
| `Thermal Runaway`, `Thermal Malfunction`, `Heating failed`, `MAXTEMP triggered`, `MINTEMP triggered` (Marlin, Buddy); `HOTEND THERMAL RUNAWAY`, `HEATBED THERMAL RUNAWAY`, `Heaters switched off. MAXTEMP [BED] triggered!`, `MINTEMP` (MK3; sometimes split: `Error:0` then the text on the next line) | all |
| `//action:poweroff` | Marlin, Buddy |

`Printer locked! (Unlock with M511 or LCD)` (Marlin `PASSWORD_FEATURE`): not dead, but every
command is refused until unlocked.

## Needs someone, not fatal (MK3)

`Bed leveling failed. Sensor triggered too high`, `Bed leveling failed. Sensor didn't
trigger. Debris on nozzle? Waiting for reset.`, `TM: error triggered!` (thermal model),
`Extruder|Hotend|Print fan speed is lower than expected`, `INT4` (power panic).

## Reports

- Temperatures (`M105`, `M155` auto-report, the `ok`s): `T:` (active tool), `T0:`…, `B:`,
  `C:` (chamber), `P:` (probe), `A:` (ambient), `L:` (cooler), each `now /target`; `@:` and
  `B@:` heater power. MK3 `M105` also: `E0:<rpm> RPM PRN1:<rpm> RPM E0@:<p> PRN1@:<p>`.
- Position (`M114`): `X:… Y:… Z:… E:… Count X:… Y:… Z:…`.
- `M115`: `FIRMWARE_NAME:… MACHINE_TYPE:… EXTRUDER_COUNT:…`, then `Cap:<NAME>:<0|1>` lines.
  The ones that matter to us: `EMERGENCY_PARSER` (`M108`/`M112`/`M410`/`M876` work at once,
  even while it heats), `AUTOREPORT_TEMP` (`M155`), `HOST_ACTION_COMMANDS`,
  `PROMPT_SUPPORT` (`M876`), `PROGRESS`/`BUILD_PERCENT` (`M73`).
- MK3 `M73`: `NORMAL MODE: Percent done: P; print time remaining in mins: R; Change in mins:
  C`. Started from its own screen: `echo:enqueing "M24"`.

## Prusa's MK3 as PrusaLink drives it

Every line PrusaLink recognises (`regular_expressions.py`) and every code it sends, read
2026-11-08; what the agent does with each:

| PrusaLink | MK3 firmware | Agent |
|---|---|---|
| `M79 S"PL"` every 12 s | "a host is there" for 30 s (`Configuration_adv.h`): only then "Set Ready" in its menu, "host print finished", resuming a host print after a power cut; the two letters show on its status line | `M79 S"MC"` every 12 s, idle and printing |
| `M155 S2 C7` / `M155 S0 C0` | auto-report temperatures (1), fans (2), position (4) | the same on the MK3 |
| `E0:… RPM PRN1:… RPM E0@:… PRN1@:…` | fan speeds and power | hotend and print fan rpm reported |
| `M75` / `M76` / `M77` | print timer start, pause, stop (its statistics) | sent at start, pause, resume (`M75`), end and stop |
| `M601` / `M602` / `M603` | pause (parks), resume, stop | used for pause, resume, stop |
| an `M601` in the file | it pauses itself (colour change) | nothing more sent until `//action:resumed` |
| `M72 S1` / `S0` | printer state ready / not ready | – (the hub does not set the MK3 ready from afar) |
| `M862.2 Q` → `302`, with an MMU `20302`/`30302` | its model code, MMU version × 10000 + model | both forms read (Buddy: `M862.2 P120`) |
| `M862.1 Q` → `0.40` | nozzle diameter | – |
| `PRUSA Fir`, `PRUSA SN`, `D3 …` | firmware, serial number, EEPROM reads (sheets, job id, print mode) | – |
| `M27 P`, `M73` lines | SD print position, progress (`NORMAL MODE: Percent done …`) | – (we stream; PrusaSlicer files carry their own `M73`) |
| `M20`, `M21`, `M23`, `M24`, `M25`, `M28`, `M29` | its SD card: list, mount, print, upload | – |
| `M707 A0x00…03`, `echo:MMU2:…` | MMU version and progress | MMU chatter kept out of the log |
| `M552` | the host's IP on its screen | – |
| `M300`, `M117`, `M0 …` | beep, message, wait for the knob | – |
| `TM: error triggered!` / `TM: error cleared` | thermal model error and its end | attention set and cleared |
| `INT4`, `//action:uvlo_*` | power panic; a host print waits to be resumed **by the host** (PrusaLink sends from the saved line) | shown as needing attention; resuming not built |
| `M603` after a reset during a print | – | – |

## G-code we send, per firmware (from each dispatcher)

| Code | Marlin 2 | Buddy (USB) | MK3 | Used for |
|---|---|---|---|---|
| G0/G1, G28, G90, G91, G92 | yes | yes | yes | jog, home, origin |
| G4 | yes | yes | yes | – |
| G29, M420 V | if levelling built in | if levelling | no | bed mesh (Marlin, Buddy) |
| G80, G81 | G80 only with motion modes; no G81 | G80 compat; no G81 | yes | bed mesh (MK3) |
| M18/M84, M104, M109, M140, M190, M105, M106, M107, M110, M114, M115, M117, M220, M221, M400 | yes | yes | yes | |
| M155 | if `AUTOREPORT_TEMP` | if auto-report | yes | temperatures while printing |
| M701, M702 | if `FILAMENT_LOAD_UNLOAD_GCODES` | yes | yes | load, unload |
| M600 | if `ADVANCED_PAUSE_FEATURE` | yes | yes | – |
| M108, M112, M410 | yes (at once with `EMERGENCY_PARSER`) | yes | **M112 only** | break off heating, emergency stop |
| M876 | if `HOST_PROMPT_SUPPORT` | yes | no | answer its question |
| M999 | yes | yes | no | restart after a stop |
| M601, M602, M603 | no | (own) | yes | MK3: pause, resume, stop with parking (PrusaLink) |
| M862.2 Q | **means something else** (I2C encoders) | in Prusa's code | yes | Prusa model code: only to Prusa firmware |

## How the reference hosts behave

- **OctoPrint:** long-running commands (`G4 G28 G29 G30 G32 M400 M226 M600`, heating) never
  time out; on silence it pokes with `M105` (an `ok` that settles a lost one, or a resend if
  the line was lost) and gives up after a few; emergency commands `M112 M108 M410` go at once,
  past the queue; `M0`/`M1` are not sent (they block); **any `Error:` during a print that is
  not a line error, an SD error or an unknown command cancels the print**; `kill() called`
  disconnects (and sends `M112`); a resend loop of 10 is an error; no `ok` after a resend:
  goes on after 1 s.
- **PrusaLink (MK3):** silence 25 s → the line is sent again (it assumes the printer dropped
  its buffer), after 3 times it disconnects; `busy`, "paused for user" and the heating lines
  keep it waiting; `M400` before an `M110`; pause/resume/stop with `M601`/`M602`/`M603`; job
  timer `M75`/`M76`/`M77`; progress to its screen with `M73`; after a reset during a print it
  sends `M603`.

## What the agent does (2026-11-08)

Every line in the lists above is in the test `every_line_the_firmware_can_say_is_read`
(`marlin.rs`); each behaviour below has a test of its own against a fake printer that
follows the firmware's rules (a line that arrives twice is dropped without an answer).

- Reads `ok` (also with values and `ADVANCED_OK`), every resend form, `busy` (both kinds of
  "waiting for someone"), `start`, `wait`, `Error:`, `!!`, `fatal:`, and host actions with
  or without the space.
- Counts the lines that have no `ok` yet. A resend sends the line asked for **and every line
  after it**; a resend for a line not sent yet is ignored; the `ok` after a resend request is
  waited for 1 s, then the lines go anyway; 10 requests for the same line in a row: broken.
- Silence while an `ok` is due (30 s; 15 min while homing, levelling, heating, `M400`,
  `M600`, ...), or `wait`: asks after it with a numbered `M105` (its `ok` settles a lost
  one; a lost line comes back as a resend), 3 times, then gives up.
- Fatal (every firmware's words, and `//action:poweroff`): the printer shows as in error;
  a print ends there.
- Needs someone (MK3 bed levelling, thermal model, fans, `INT4`; Marlin locked): the printer
  shows as needing attention, with the text.
- An `Error:` about the print (not a line error, SD error or unknown code) ends the print and
  switches the heaters off, as OctoPrint does.
- Reads `M115`: the firmware and its `Cap:` lines. `M155` only where it can (or did not
  say). `M862.2 Q` only to Prusa's firmware.
- Stop: breaks off a heat-up at once with a numbered `M108` where the emergency parser is
  there (its `ok` is counted), then heaters, fan and motors off. On Prusa's MK3: `M603`.
- Pause and resume on Prusa's MK3: `M601` / `M602` (it parks the head); elsewhere the host
  stops sending.

- **A question on its screen** (Marlin's host prompt) shows on the machine page with its
  buttons; pressed there it goes as a numbered `M876 S<index>` (also while a line waits: the
  emergency parser takes it). A Prusa on Connect gets `DIALOG_ACTION` (`dialog_id`, `button`).
- **Its own SD card** (Prusa's MK3, Marlin with `Cap:SDCARD`): the hub lists it with an
  ordinary `M20 L T` and reads the answer whole (`crates/hub/src/sd_card.rs`, the SD card
  tab); printing is `M23 <8.3 path>` and `M24`; the agent follows it with `M27` (`M27 C` on
  Marlin, for the name) and reports it as its job; pause, resume and stop from the machine
  page go to it (MK3 `M601`/`M602`/`M603`, Marlin `M25`/`M24`/`M524`).
- **A power cut on Prusa's MK3** (`crates/agent/src/recovery.rs`): a job keeps a copy of its
  G-code and a checkpoint (the next line's number and place in the file, and the numbers of
  recent lines that were not the file's) beside the agent's credentials. After the cut the
  MK3 waits, the agent reports the job paused again (the hub brings back the job it had
  failed, and puts what it counted off the spool back on). Resumed here (`M602`) or on the
  printer, the MK3 asks `Resend: N+1` for the line after the last it carried out; the job
  goes on from the file's line that goes with it, numbered from there. A warm bed
  (`uvlo_auto_recovery_ready`) is not resumed by itself: someone confirms.

Not done (features, not answers):
- Progress to its screen (`M73`) for files without their own (PrusaSlicer's carry it).
- Parking on pause and lifting on stop for Marlin itself (there is no single safe way for
  every machine; OctoPrint leaves it to scripts).
- Uploading to its SD card (`M28`/`M29`), deleting from it (`M30`).
- None of this is tried on a real printer yet.
