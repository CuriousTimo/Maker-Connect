# Audit: the agent against OctoPrint's serial layer (plain Marlin)

For every printer that is not Prusa's MK3 family (that one follows PrusaLink, see
`docs/prusalink-audit.md`), the reference for how a host talks to the printer is OctoPrint's
serial connector with its default settings and, for which G-codes exist and what they answer,
the Marlin firmware source. Where the two disagree, the firmware source decides. The rule:
follow the reference to the letter; nothing is done another way.

**How this list was made (2026-11-10):** mechanically, by `scripts/marlin-audit-check.py`, from
the sources in `reference/`, never typed by hand:

- `reference/OctoPrint/src/octoprint/plugins/serial_connector/serial_comm.py`: its compiled
  regexes, the literal tests on the printer's lines, the error lists, the capabilities, the
  firmware-name rules, the action commands, the G-code literals it sends, its per-G-code
  handlers, its fallback timing defaults, the methods that do the talking.
- `.../serial_connector/config_schema.py`: every default setting of the serial connector.
- `reference/OctoPrint/src/octoprint/schema/config/scripts.py`: the default gcode scripts.
- `reference/Marlin/Marlin/src/gcode/gcode.cpp`: the `case` list of G and M codes (used only
  for the reverse check: does the firmware have the code at all).

Run `python3 -I scripts/marlin-audit-check.py`. It lists every item not named here in
backticks, refuses rows done "another way", checks that a test named in a status exists and
holds the row's G-code, and checks the other direction: every G-code literal the agent (and
the hub's `sd_card.rs`, `main.rs`, `queue.rs`) can send is OctoPrint's form or is named in the
section "What we send that OctoPrint does not".

**Status:** every row is `UNCHECKED` for now. This is the list, not a claim that anything is
the same. A row is done only with a status that names a test; "same" or "fixed" without a test
is refused by the checker. The column "Our agent now" is what the Rust code in
`crates/agent/src` does for a plain Marlin printer, read on 2026-11-10.

**Not covered by the checker:**

- OctoPrint's behaviour is not extracted, only its names and literals; each row says in a
  line what the source does.
- G-code files of a print and G-code text the hub queues (`carry_out`) are the user's. The
  hub's code outside `sd_card.rs`, `main.rs`, `queue.rs` is not scanned.
- Codes built from pieces (`format!` with a variable code) are seen only as far as a literal
  starts with the code.
- Marlin's answers per G-code are not extracted; `gcode.cpp` gives only the list of codes.
- OctoPrint's printer profile, UI, plugins and hooks, and settings outside the serial
  connector, are not read. The hub's parsing of SD listings is not audited here.


## 1. Patterns the printer's output is read with (OctoPrint regexes)

| Pattern | OctoPrint / Marlin (source) | Our agent now, plain Marlin | Status |
|---|---|---|---|
| `RESERVED_IDENTIFIER_REGEX` | Names (`B`, `C`, `T\d*`) a custom temperature sensor may not take (`serial_comm.py:401`) | Not applicable: no custom sensors | `UNCHECKED` |
| `_temp_command_regex` | Finds the temperature in `M104/M109/M140/M190/...` commands, to apply the temperature offsets (`serial_comm.py:6459`) | Not applicable: no temperature offsets | `UNCHECKED` |
| `regex_checksum_suffix` | Strips `*123` from a command before it is renumbered (`serial_comm.py:193`) | Not needed: see `regex_linenumber_prefix` | `UNCHECKED` |
| `regex_command` | Reads the G/M/T/F code at the start of a command line (`[GM]\d+` with optional `.subcode`, `T\d+`, `F\d+`) (`serial_comm.py:61`) | No regex. `marlin::clean` cuts the comment (`;`); `marlin::long_running` takes the first word of the line (`marlin.rs:447`) | `UNCHECKED` |
| `regex_e_positions` | Several `En:value` extruder positions of a position report (`serial_comm.py:155`) | Not read | `UNCHECKED` |
| `regex_firmware_splitter` | Splits an `M115` answer into its `KEY:value` parts (`serial_comm.py:167`) | `marlin::firmware` (`marlin.rs:815`) finds `FIRMWARE_NAME:` and cuts at ` based on`, ` SOURCE_CODE_URL`, ` PROTOCOL_VERSION`, ` (`; `marlin::machine_type` finds `MACHINE_TYPE:` up to ` EXTRUDER_COUNT`; no general key splitter | `UNCHECKED` |
| `regex_float` | A float value (`serial_comm.py:65`) | No equivalent; Rust `parse::<f32>()` per value | `UNCHECKED` |
| `regex_invalid_extruder` | `invalid extruder N` answer; the tool is added to the known-invalid tools (`serial_comm.py:179`) | Not read | `UNCHECKED` |
| `regex_linenumber_prefix` | Strips `N123 ` from a command before it is renumbered (`serial_comm.py:184`) | Not needed: lines are numbered by `marlin::numbered` (`marlin.rs:61`) from the bare code | `UNCHECKED` |
| `regex_marlinKillError` | Kill errors that Marlin splits over two lines: `Heating failed`, `Thermal Runaway`, `Thermal Malfunction`, `MAXTEMP triggered`, `MINTEMP triggered`, `Invalid extruder number`, `Watchdog barked`, `KILL caused`; the next `Error:` line is joined on (`serial_comm.py:88`) | No two-line joining. `marlin::fatal` has `kill()`, `kill caused by`, `stop called because of`, `printer halted`, `thermal runaway`, `thermal malfunction`, `mintemp`, `maxtemp`, `heating failed`, `heaters switched off`, `printer stopped`, `stopped due to errors`; not `invalid extruder number`, not `watchdog barked` | `UNCHECKED` |
| `regex_minMaxError` | First line of a min/max temperature error (`Error:<digit>` then the next line is joined) (`serial_comm.py:85`) | No two-line joining. `marlin::fatal` (`marlin.rs:209`) tests the single `Error:` line for `maxtemp`, `mintemp` | `UNCHECKED` |
| `regex_position` | `X:.. Y:.. Z:.. E:..` or `E0:.. E1:..` position report (`serial_comm.py:143`) | `marlin::position` (`marlin.rs:1079`): the line must start with `X:` (after `ok `); reads X, Y, Z in mm; E and `E0..En` not read | `UNCHECKED` |
| `regex_repetierTempBed` | Repetier `TargetBed:temp` (only when Repetier is detected) (`serial_comm.py:133`) | Not read | `UNCHECKED` |
| `regex_repetierTempExtr` | Repetier `TargetExtrN:temp` (only when Repetier is detected) (`serial_comm.py:119`) | Not read | `UNCHECKED` |
| `regex_resend_linenumber` | The line number of a resend request (`N`, `N:` or bare digits) (`serial_comm.py:176`) | `marlin::parse` (`marlin.rs:92`): after `resend:` or `rs `, trims a leading `n`, parses a number | `UNCHECKED` |
| `regex_sdFileOpened` | `File opened: NAME Size: N` (`serial_comm.py:96`) | `File opened: ` read by prefix and ` Size: ` split (`marlin.rs:698`, `marlin.rs:735`, `hub.rs:1951`); size not used | `UNCHECKED` |
| `regex_sdPrintingByte` | `SD printing byte 123/456` -> current and total bytes (`serial_comm.py:93`) | `marlin::sd_status` (`marlin.rs:288`) reads `SD printing byte a/b`; also `Not SD printing`, `Print saved`, `SD print paused`, `Current file:` | `UNCHECKED` |
| `regex_serial_devices` | Which port names count as serial devices (`ttyUSB`, `ttyACM`, `tty.usb`, `cu.`, `cuaU`, `ttyS`, `rfcomm`) (`serial_comm.py:196`) | No filter: the port is given in the agent's config (`--port`, `config.rs:34`); `maker-connect-agent ports` lists them (`main.rs:24`) | `UNCHECKED` |
| `regex_temp` | Any temperature entry `T:`, `T0:`, `B:`, `C:` or a custom sensor name `NAME:actual /target` (`serial_comm.py:107`) | `marlin::heat` (`marlin.rs:1093`): only `T:` and `B:` (with `/target` joined or apart) and the `@:` / `B@:` power; no `T0:`..`Tn:`, no `C:`, no custom sensors | `UNCHECKED` |
| `regexes_parameters.floatE` | Reads the `E` parameter of a command we are sending (float) (`serial_comm.py:71`) | No general parameter reader | `UNCHECKED` |
| `regexes_parameters.floatF` | Reads the `F` parameter of a command we are sending (float) (`serial_comm.py:72`) | No general parameter reader | `UNCHECKED` |
| `regexes_parameters.floatP` | Reads the `P` parameter of a command we are sending (float) (`serial_comm.py:73`) | No general parameter reader | `UNCHECKED` |
| `regexes_parameters.floatR` | Reads the `R` parameter of a command we are sending (float) (`serial_comm.py:74`) | No general parameter reader | `UNCHECKED` |
| `regexes_parameters.floatS` | Reads the `S` parameter of a command we are sending (float) (`serial_comm.py:75`) | No general parameter reader | `UNCHECKED` |
| `regexes_parameters.floatX` | Reads the `X` parameter of a command we are sending (float) (`serial_comm.py:76`) | No general parameter reader | `UNCHECKED` |
| `regexes_parameters.floatY` | Reads the `Y` parameter of a command we are sending (float) (`serial_comm.py:77`) | No general parameter reader | `UNCHECKED` |
| `regexes_parameters.floatZ` | Reads the `Z` parameter of a command we are sending (float) (`serial_comm.py:78`) | No general parameter reader | `UNCHECKED` |
| `regexes_parameters.intN` | Reads the `N` parameter of a command we are sending (integer) (`serial_comm.py:79`) | No general parameter reader | `UNCHECKED` |
| `regexes_parameters.intP` | Reads the `P` parameter of a command we are sending (integer) (`serial_comm.py:80`) | No general parameter reader. Only `print::m73` (`print.rs:45`) reads P, R, S of `M73` and `print::m73_change_in` (`print.rs:65`) D, C | `UNCHECKED` |
| `regexes_parameters.intR` | Reads the `R` parameter of a command we are sending (integer) (`serial_comm.py:81`) | No general parameter reader. Only `print::m73` (`print.rs:45`) reads P, R, S of `M73` and `print::m73_change_in` (`print.rs:65`) D, C | `UNCHECKED` |
| `regexes_parameters.intS` | Reads the `S` parameter of a command we are sending (integer) (`serial_comm.py:82`) | No general parameter reader. Only `print::m73` (`print.rs:45`) reads P, R, S of `M73` and `print::m73_change_in` (`print.rs:65`) D, C | `UNCHECKED` |
| `regexes_parameters.intT` | Reads the `T` parameter of a command we are sending (integer) (`serial_comm.py:83`) | No general parameter reader | `UNCHECKED` |

## 2. What the printer says: literal checks in OctoPrint's read loop

Each literal is an `startswith`, `in` or `==` test on the printer's line in `serial_comm.py`. Ours compares more often exactly where OctoPrint tests for a part of the line.

| Line check | OctoPrint / Marlin (source) | Our agent now, plain Marlin | Status |
|---|---|---|---|
| `!!` | `_handle_errors`: `!!` lines are errors (Marlin kill / stop messages) (`serial_comm.py:4171`) | `Reply::Error` (`marlin.rs:123`) | `UNCHECKED` |
| `//` | A `//` line is debugging output; `//action:NAME PARAMS` is an action command (`serial_comm.py:2458`) | `//action:` and `// action:` read (`marlin.rs:131`); mapping in section 6 | `UNCHECKED` |
| `A:` | `B:` with `A:` is not a temperature report (Prusa `A:` ambient) (`serial_comm.py:2859`) | No such test; `B:` is read as the bed whenever it has a number | `UNCHECKED` |
| `B:` | A temperature report (`B:` at the start, without `A:`) (`serial_comm.py:2687`) | `marlin::heat` reads `B:` | `UNCHECKED` |
| `Begin file list` | Start of the `M20` list; the lines until `End file list` are file names (`serial_comm.py:3164`) | `Screen::heard` (`marlin.rs:728`) marks the listing (only to see that a card is in); the file names go to the hub as the command's answer (hub side not audited here) | `UNCHECKED` |
| `C:` | Part of the test whether an `ok` line carries a report (chamber) (`serial_comm.py:2688`) | Not read (no chamber) | `UNCHECKED` |
| `Card successfully initialized` | SD card present (same) (`serial_comm.py:3159`) | Not read | `UNCHECKED` |
| `Done printing file` | SD print finished (`serial_comm.py:3282`) | `Screen::heard` (`marlin.rs:719`): the SD print is done | `UNCHECKED` |
| `Done saving file` | `M29` answer: transfer done (`serial_comm.py:3297`) | Not read | `UNCHECKED` |
| `End file list` | End of the `M20` list (`serial_comm.py:2600`) | As `Begin file list` (`marlin.rs:729`) | `UNCHECKED` |
| `File deleted` | `M30` answer (with `ok` at the end of the line) (`serial_comm.py:3302`) | Not read (no `M30`) | `UNCHECKED` |
| `File opened` | `M23` answer: file name and size (`regex_sdFileOpened`), selects the file (`serial_comm.py:3233`) | `marlin.rs:698`, `marlin.rs:735`; `hub.rs:1951` goes on only on this line, only in code that carries out hub G-code | `UNCHECKED` |
| `M115:FIRMWARE_NAME` | Firmware name from the `M115` answer, drives the firmware rules (section 5) (`serial_comm.py:2976`) | `marlin::firmware` (`marlin.rs:815`), kept as `Printer::firmware`; no firmware rules (section 5) | `UNCHECKED` |
| `M115:NAME` | Malyan: name, with `VER` (`serial_comm.py:2991`) | Not read | `UNCHECKED` |
| `M115:VER` | Malyan: version (`serial_comm.py:2992`) | Not read | `UNCHECKED` |
| `NAME.` | An `M115` answer at the start of the line (Malyan `NAME. Malyan`) (`serial_comm.py:2973`) | Not read (no Malyan) | `UNCHECKED` |
| `NAME:` | An `M115` answer (`NAME:` anywhere): firmware name, rules by firmware (`serial_comm.py:2690`) | Not read; `FIRMWARE_NAME:` is looked for (`marlin::firmware`) | `UNCHECKED` |
| `No media` | SD card not there (`serial_comm.py:3150`) | Not read | `UNCHECKED` |
| `Not SD printing` | `M27` answer: not printing from SD; counted against `maxNotSdPrinting` (`serial_comm.py:3221`) | `sd_status` (`marlin.rs:290`): no print from the card; no `maxNotSdPrinting` count | `UNCHECKED` |
| `SD Card unmounted` | SD card not there (`serial_comm.py:3152`) | Not read | `UNCHECKED` |
| `SD card ok` | SD card present, file list is read (`refreshSdFiles`) (`serial_comm.py:3159`) | Exactly `echo:SD card ok` sets card present (`marlin.rs:690`) | `UNCHECKED` |
| `SD card released` | SD card not there (`serial_comm.py:3153`) | Exactly `echo:SD card released` sets card absent (`marlin.rs:692`) | `UNCHECKED` |
| `SD init fail` | SD card not there (`sdAvailable` false) (`serial_comm.py:3148`) | Exactly `echo:SD init fail` sets card absent (`marlin.rs:691`) | `UNCHECKED` |
| `SD printing byte` | `M27` answer: byte position and size (`regex_sdPrintingByte`) (`serial_comm.py:3172`) | `sd_status` (`marlin.rs:299`), after `M27 C` | `UNCHECKED` |
| `T0:` | A temperature report (`T0:` at the start) (`serial_comm.py:2686`) | Not read | `UNCHECKED` |
| `T:` | A temperature report (`T:` at the start) (`serial_comm.py:2685`) | `marlin::heat` reads `T:` words anywhere in the line; `routine` (`marlin.rs:256`) takes `T:` / `ok T:` as routine | `UNCHECKED` |
| `TargetBed` | Repetier bed target temperature (same) (`serial_comm.py:2881`) | Not read | `UNCHECKED` |
| `TargetExtr` | Repetier target temperature, only when `repetierTargetTemp` or Repetier is detected (`serial_comm.py:2881`) | Not read | `UNCHECKED` |
| `Writing to file` | `M28` answer while streaming to SD (`serial_comm.py:3280`) | Not read (no SD upload by M28 for plain Marlin) | `UNCHECKED` |
| `X:` | Position report (with `Y:` and `Z:` anywhere in the line); also in the `ok` test (`serial_comm.py:2689`) | `marlin::position`: the line must start with `X:`; `Y:`/`Z:` are looked up as words | `UNCHECKED` |
| `Y:` | Position report (see `X:`) (`serial_comm.py:2797`) | See `X:` | `UNCHECKED` |
| `Z:` | Position report (see `X:`) (`serial_comm.py:2797`) | See `X:` | `UNCHECKED` |
| `busy:` | Resets the ok timeout (`communicationBusy`); first time with the BUSY_PROTOCOL capability sets `M113 S<interval>` (`serial_comm.py:2413`) | `Reply::Busy` for `busy:` and `echo:busy` (`marlin.rs:106`): waits on; no `M113`; any line counts as a sign of life (`last_heard`) | `UNCHECKED` |
| `cap:` | A `Cap:NAME:0|1` capability line of `M115` (lower-cased, `parse_capability_line`) (`serial_comm.py:2748`) | `marlin::capabilities` (`marlin.rs:424`): `Cap:` case-sensitive; keeps all, uses `EMERGENCY_PARSER`, `AUTOREPORT_TEMP`, `PROMPT_SUPPORT`, and `SDCARD` for `has_sd` | `UNCHECKED` |
| `echo:busy:` | Same as `busy:` (`serial_comm.py:2413`) | Read as `echo:busy` (no colon needed), same as `busy:` | `UNCHECKED` |
| `error:` | `_handle_errors`: an `Error:` line goes through the error lists; unknown ones follow `errorHandling` (`disconnect` by default) (`serial_comm.py:4171`) | `Reply::Error` (`marlin.rs:118`); `harmless_error` (`marlin.rs:388`) lists the OctoPrint classes; others end the print (`Event::Error`); no `errorHandling` setting | `UNCHECKED` |
| `fatal:` | `_handle_errors`: `fatal:` (Repetier) becomes `Error:fatal:..`; in the fatal list (`serial_comm.py:4166`) | `Reply::Error` with the whole line (`marlin.rs:126`); no `Error:` prefix added | `UNCHECKED` |
| `invalid extruder` | Learns invalid tool numbers (`sanityCheckTools`) (`serial_comm.py:3071`) | Not read | `UNCHECKED` |
| `ok` | `line.startswith("ok")` (case as sent) counts as an acknowledgement, `_handle_ok`; `ok` with `T:`/`X:`/`NAME:` etc. is processed further (`serial_comm.py:2406`) | `marlin::parse` (`marlin.rs:92`): `ok` exactly, or starting `ok ` (lower-cased); `okay`, `ok:` are not an ok. Temperature after `ok` is read | `UNCHECKED` |
| `openRoot failed` | SD card not there (`serial_comm.py:3151`) | Exactly `Error:openRoot failed` sets card absent (`marlin.rs:691`) | `UNCHECKED` |
| `resend` | `lower_line.startswith("resend")` is a resend request, number by `regex_resend_linenumber` (`serial_comm.py:2695`) | Only `resend:` (with the colon) is read (`marlin.rs:100`); `Resend N` without colon is not | `UNCHECKED` |
| `rs` | `lower_line.startswith("rs")` is a resend request, `_handle_resend_request` (`serial_comm.py:2695`) | `rs ` followed by a number (`N` optional) is read (`marlin.rs:101`); `rs` without a space is not | `UNCHECKED` |
| `start` | `start` during detection/connect: connected, sends hello (`M110 N0`); while operational (exact `start`): external reset, line numbers reset, hello again; while printing: job aborted (`serial_comm.py:3349`) | `Reply::Start` only for exactly `start` (`marlin.rs:112`): while waiting for an ok it ends with `the printer restarted`; while booting (`wait_for_boot`) it is only said. No hello, no `M110 N0` again on a reset while idle | `UNCHECKED` |
| `unknown command` | `Error:` lines with `unknown command` are ignored (`serial_comm.py:4202`) | In `harmless_error` (`marlin.rs:405`) | `UNCHECKED` |
| `volume.init failed` | SD card not there (`serial_comm.py:3149`) | Exactly `Error:volume.init failed` sets card absent (`marlin.rs:691`) | `UNCHECKED` |
| `wait` | `line == "wait"` counts as an `ok` while printing (`supportWait`); in the connect phase it simulates a missed `ok` (`serial_comm.py:2678`) | `Reply::Wait` (`marlin.rs:115`): not an ok; while an ok is due it asks with a numbered `M105` (`poke`, `marlin.rs:1635`), max 3 (`MAX_POKES`) | `UNCHECKED` |

## 3. Error classes (OctoPrint's four lists)

Default for any other `Error:`: `errorHandling` = `disconnect` (`config_schema.py`); while printing OctoPrint with `cancel` would cancel the job. Ours: an error not in the lists ends the print and is reported (`Event::Error`, `print.rs:100`).

| Error text | OctoPrint / Marlin (source) | Our agent now, plain Marlin | Status |
|---|---|---|---|
| `cannot enter` | `_sd_card_errors`: same (`serial_comm.py:4149 (_sd_card_errors)`) | In `harmless_error` (`marlin.rs:398`) | `UNCHECKED` |
| `cannot open` | `_sd_card_errors`: same (`serial_comm.py:4149 (_sd_card_errors)`) | In `harmless_error` (`marlin.rs:398`) | `UNCHECKED` |
| `checksum` | `_resend_request_communication_errors`: same (`serial_comm.py:4139 (_resend_request_communication_errors)`) | In `harmless_error` (`marlin.rs:392`): not an error of the print; the resend is followed | `UNCHECKED` |
| `error writing to file` | `_sd_card_errors`: same (`serial_comm.py:4149 (_sd_card_errors)`) | In `harmless_error` (`marlin.rs:398`) | `UNCHECKED` |
| `expected line` | `_resend_request_communication_errors`: same (`serial_comm.py:4139 (_resend_request_communication_errors)`) | In `harmless_error` (`marlin.rs:392`): not an error of the print; the resend is followed | `UNCHECKED` |
| `fatal:` | `_fatal_errors`: always a disconnect (`serial_comm.py:4158 (_fatal_errors)`) | `parse` makes `fatal:` an `Error` (`marlin.rs:126`); not in `harmless_error`, so it ends the print | `UNCHECKED` |
| `format error` | `_resend_request_communication_errors`: same (`serial_comm.py:4139 (_resend_request_communication_errors)`) | In `harmless_error` (`marlin.rs:392`): not an error of the print; the resend is followed | `UNCHECKED` |
| `kill() called` | `_fatal_errors`: always a disconnect (`_trigger_error`) (`serial_comm.py:4158 (_fatal_errors)`) | `marlin::fatal` (`marlin.rs:209`) has `kill()`; the `Error`/`fatal:` line ends the print | `UNCHECKED` |
| `line number` | `_resend_request_communication_errors`: skipped, the printer sends a resend request (`serial_comm.py:4139 (_resend_request_communication_errors)`) | In `harmless_error` (`marlin.rs:392`): not an error of the print; the resend is followed | `UNCHECKED` |
| `linenumber` | `_resend_request_communication_errors`: same (`serial_comm.py:4139 (_resend_request_communication_errors)`) | In `harmless_error` (`marlin.rs:392`): not an error of the print; the resend is followed | `UNCHECKED` |
| `missing linenumber` | `_recoverable_communication_errors`: same (`serial_comm.py:4135 (_recoverable_communication_errors)`) | Not in `harmless_error` (`marlin.rs:388`): ends the print as an error. The `ok` is not simulated | `UNCHECKED` |
| `no line number with checksum` | `_recoverable_communication_errors`: the printer's comm error; `_handle_ok()` is triggered by hand (`serial_comm.py:4135 (_recoverable_communication_errors)`) | Not in `harmless_error` (`marlin.rs:388`): ends the print as an error. The `ok` is not simulated | `UNCHECKED` |
| `open failed` | `_sd_card_errors`: same (`serial_comm.py:4149 (_sd_card_errors)`) | In `harmless_error` (`marlin.rs:398`) | `UNCHECKED` |
| `openroot` | `_sd_card_errors`: same (`serial_comm.py:4149 (_sd_card_errors)`) | In `harmless_error` (`marlin.rs:398`) | `UNCHECKED` |
| `volume.init` | `_sd_card_errors`: SD card error, ignored (not an error of the print) (`serial_comm.py:4149 (_sd_card_errors)`) | In `harmless_error` (`marlin.rs:398`) | `UNCHECKED` |
| `workdir` | `_sd_card_errors`: same (`serial_comm.py:4149 (_sd_card_errors)`) | In `harmless_error` (`marlin.rs:398`) | `UNCHECKED` |

## 4. Capabilities (M115 `Cap:` lines)

| Capability | OctoPrint / Marlin (source) | Our agent now, plain Marlin | Status |
|---|---|---|---|
| `CAPABILITY_AUTOREPORT_POS` | `Cap:AUTOREPORT_POS:1` -> `M154 S<posAutoreport>` (`serial_comm.py:529`) | Read into `caps.all` only; `M154` never sent; `M114` is asked every idle report (`hub.rs:761`) | `UNCHECKED` |
| `CAPABILITY_AUTOREPORT_SD_STATUS` | `Cap:AUTOREPORT_SD_STATUS:1` -> `M27 S<sdStatusAutoreport>`; no `M27` polling (`serial_comm.py:528`) | Read into `caps.all` only; `M27 S` never sent; `M27 C` is asked (`marlin.rs:1324`) | `UNCHECKED` |
| `CAPABILITY_AUTOREPORT_TEMP` | `Cap:AUTOREPORT_TEMP:1` -> `M155 S<temperatureAutoreport>`; then no `M105` polling (`_temperature_autoreporting`) (`serial_comm.py:527`) | Read (`Capabilities::autoreport_temp`, `marlin.rs:415`). Used only while a job runs: `M155 S2` at start, `M155 S0` at the end (`print.rs:216-218`, also when `M115` said nothing at all). Idle: `M105` every 2 s (`hub.rs:759`). Not `M155 S<n>` at connect | `UNCHECKED` |
| `CAPABILITY_BUSY_PROTOCOL` | `Cap:BUSY_PROTOCOL:1`: on the first `busy:` line `M113 S<communicationBusy - 1>` is sent (`serial_comm.py:530`) | Read into `caps.all` only; `M113` never sent | `UNCHECKED` |
| `CAPABILITY_CHAMBER_TEMP` | `Cap:CHAMBER_TEMPERATURE:1`: chamber temperature read (`C:`) (`serial_comm.py:532`) | Not used | `UNCHECKED` |
| `CAPABILITY_EMERGENCY_PARSER` | `Cap:EMERGENCY_PARSER:1`: `M108`, `M112`, `M410` are sent at once (`_emergency_force_send`), not queued (`serial_comm.py:531`) | Read (`Capabilities::emergency_parser`); used for `M108` while heating (`marlin.rs:1705`) and for the answer `M876`. `M112`, `M410` never sent | `UNCHECKED` |
| `CAPABILITY_EXTENDED_M20` | `Cap:EXTENDED_M20:1`: list the card with `M20 L T`, else `M20` (`serial_comm.py:533`) | Read into `caps.all` only; the hub asks `M20 L T` for non-MK3 machines always (`sd_card.rs:159`) | `UNCHECKED` |
| `CAPABILITY_LFN_WRITE` | `Cap:LFN_WRITE:1`: long file names may be written to the card (else names made 8.3 on upload) (`serial_comm.py:534`) | Not used | `UNCHECKED` |
| `CAPABILITY_SUPPORT_DETECTED` | Setting value `detected`: use it when M115 says so (`serial_comm.py:537`) | No such setting; behaviour is always like `detected`, for three capabilities only | `UNCHECKED` |
| `CAPABILITY_SUPPORT_DISABLED` | Setting value `disabled`: never use it (`serial_comm.py:538`) | No such setting | `UNCHECKED` |
| `CAPABILITY_SUPPORT_ENABLED` | Setting value `enabled` of `capabilities.*`: always use it (`serial_comm.py:536`) | No such setting | `UNCHECKED` |

## 5. Firmware-name rules (from the `M115` `FIRMWARE_NAME`)

OctoPrint only applies them when `firmwareDetection` is true (default). Plain Marlin has no rule of its own in OctoPrint.

| Firmware name | OctoPrint / Marlin (source) | Our agent now, plain Marlin | Status |
|---|---|---|---|
| `anet_a8` | Same rule as Repetier (Anet A8 firmware) (`serial_comm.py:3006`) | No rule by firmware name: one behaviour for all plain Marlin. The name is read (`marlin::firmware`) and shown; `Prusa-Firmware` / Prusa's model code decide only the MK3 family branch (`hub.rs:925`) | `UNCHECKED` |
| `klipper` | Klipper: `unknownCommandsNeedAck` true (`serial_comm.py:3043`) | No rule by firmware name: one behaviour for all plain Marlin. The name is read (`marlin::firmware`) and shown; `Prusa-Firmware` / Prusa's model code decide only the MK3 family branch (`hub.rs:925`) | `UNCHECKED` |
| `malyan` | Malyan: `sendChecksum` always, `blockWhileDwelling`, `sdAlwaysAvailable` true, `M21` at once (`serial_comm.py:3023`) | No rule by firmware name: one behaviour for all plain Marlin. The name is read (`marlin::firmware`) and shown; `Prusa-Firmware` / Prusa's model code decide only the MK3 family branch (`hub.rs:925`) | `UNCHECKED` |
| `prusa-firmware` | Prusa firmware: `sdLowerCase` true, `sendChecksumWithUnknownCommands` true, `unknownCommandsNeedAck` true (`serial_comm.py:3057`) | No rule by firmware name: one behaviour for all plain Marlin. The name is read (`marlin::firmware`) and shown; `Prusa-Firmware` / Prusa's model code decide only the MK3 family branch (`hub.rs:925`) | `UNCHECKED` |
| `repetier` | Repetier: `sendChecksum` always, `blockWhileDwelling`, Repetier target temps, no external heat-up detection (`serial_comm.py:3005`) | No rule by firmware name: one behaviour for all plain Marlin. The name is read (`marlin::firmware`) and shown; `Prusa-Firmware` / Prusa's model code decide only the MK3 family branch (`hub.rs:925`) | `UNCHECKED` |
| `reprapfirmware` | RepRapFirmware: `sdRelativePath` true (`serial_comm.py:3017`) | No rule by firmware name: one behaviour for all plain Marlin. The name is read (`marlin::firmware`) and shown; `Prusa-Firmware` / Prusa's model code decide only the MK3 family branch (`hub.rs:925`) | `UNCHECKED` |
| `teacup` | Teacup: no external heat-up detection (`serial_comm.py:3036`) | No rule by firmware name: one behaviour for all plain Marlin. The name is read (`marlin::firmware`) and shown; `Prusa-Firmware` / Prusa's model code decide only the MK3 family branch (`hub.rs:925`) | `UNCHECKED` |
| `ultimaker2` | Ultimaker2: `disableSdPrintingDetection` true (`serial_comm.py:3050`) | No rule by firmware name: one behaviour for all plain Marlin. The name is read (`marlin::firmware`) and shown; `Prusa-Firmware` / Prusa's model code decide only the MK3 family branch (`hub.rs:925`) | `UNCHECKED` |

## 6. Action commands (`//action:NAME`)

Ours also reads `prompt_begin`, `prompt_button`, `prompt_choice`, `prompt_show`, `prompt_end`, `notification`, `ready`, `not_ready`, `uvlo_*`, `poweroff`, `kill`, `filament_runout`, `probe_failed`, `probe_rewipe`, `safety_timer_expired`, `dump_available` (`marlin.rs:131-180`); OctoPrint passes every action to its action-command hooks and plugins (`action_command_prompt`, `action_command_notification`).

| Action | OctoPrint / Marlin (source) | Our agent now, plain Marlin | Status |
|---|---|---|---|
| `cancel` | Cancels the print (`serial_comm.py:2478`) | `Action::Cancel`: the print is stopped (`marlin.rs:794`) | `UNCHECKED` |
| `disconnect` | Disconnects from the printer (`serial_comm.py:2521`) | `Action::Other`: ignored | `UNCHECKED` |
| `pause` | Pauses the print (`serial_comm.py:2486`) | `Action::Pause`: `paused` flag set; sending waits | `UNCHECKED` |
| `paused` | Printer says it paused: state switched, nothing sent (`serial_comm.py:2493`) | `Action::Paused`: same flag | `UNCHECKED` |
| `resume` | Resumes the print (`serial_comm.py:2503`) | `Action::Resume` (also for `resumed`): flag cleared | `UNCHECKED` |
| `resumed` | Printer says it resumed: state switched, nothing sent (`serial_comm.py:2511`) | `Action::Resume` | `UNCHECKED` |
| `sd_ejected` | SD card out (`serial_comm.py:2549`) | `Action::Other`: ignored | `UNCHECKED` |
| `sd_inserted` | SD card in: `refreshSdFiles` (only with SD enabled) (`serial_comm.py:2542`) | `Action::Other`: ignored | `UNCHECKED` |
| `sd_updated` | Card changed: `refreshSdFiles` (`serial_comm.py:2556`) | `Action::Other`: ignored | `UNCHECKED` |
| `shutdown` | Shuts down the host only if `enableShutdownActionCommand` (default false), else logs a warning (`serial_comm.py:2527`) | `Action::Other`: ignored (documented at `marlin.rs:200`) | `UNCHECKED` |
| `start` | Restarts the current job (if one is selected) (`serial_comm.py:2469`) | `Action::Reprint` (`marlin.rs:154`): Prusa MK3 menu `Reprint`; asked of the hub (not an immediate restart of the current job) | `UNCHECKED` |

## 7. G-codes OctoPrint sends by itself

Forms are as written in `serial_comm.py`, `config_schema.py` (`helloCommand`, `sdCancelCommand`) and the default gcode scripts (`schema/config/scripts.py`). `{...}` and `%s` stand for a value. "Sends" lists our literal forms that match (same rule as the checker's reverse direction); hub = `crates/hub/src/sd_card.rs`, `main.rs`, `queue.rs`.

| G-code form | OctoPrint / Marlin (source) | Our agent now, plain Marlin | Status |
|---|---|---|---|
| `M104 T{tool} S0` | On `M112` or a firmware error: heaters off per tool (`serial_comm.py:5662`) | Does not send it from the agent code. (The hub can queue any G-code text, `hub.rs:1468`; its own codes are in the section below) | `UNCHECKED` |
| `M104 T0 S0` | Default snippet `disable_hotends` for a shared nozzle (`scripts.py:35`) | Does not send it: `M104 S0` once in the stop list (`print.rs:125`) | `UNCHECKED` |
| `M105` | Temperature poll; also the detection hello (`serial_comm.py:3548`) | Sends: `M105` (agent/hub.rs:759, agent/marlin.rs:1636) | `UNCHECKED` |
| `M106 S0` | Fan off in the default `afterPrintCancelled` script (`scripts.py:17 (script afterPrintCancelled)`) | Does not send it from the agent code. (The hub can queue any G-code text, `hub.rs:1468`; its own codes are in the section below) | `UNCHECKED` |
| `M108` | On cancel while heating (`abortHeatupOnCancel`); emergency command (`serial_comm.py:1875`) | Sends: `M108` (agent/marlin.rs:1705) | `UNCHECKED` |
| `M110 N0` | `helloCommand` (default), sent on connect and after an external reset (`config_schema.py:175 (helloCommand)`) | Sends: `M110 N0` (agent/marlin.rs:1572) | `UNCHECKED` |
| `M110 N{linenum}` | Re-sent in the form OctoPrint rewrites an `M110` to (`serial_comm.py:5643`) | Sends: `M110 N0` (agent/marlin.rs:1572) | `UNCHECKED` |
| `M110 N{number}` | `resetLineNumbers`: sets the line number (`serial_comm.py:2228`) | Sends: `M110 N0` (agent/marlin.rs:1572) | `UNCHECKED` |
| `M112` | Emergency stop: sent twice (without and with checksum), then heaters off and close (`serial_comm.py:5652`) | Does not send it from the agent code. (The hub can queue any G-code text, `hub.rs:1468`; its own codes are in the section below) | `UNCHECKED` |
| `M113 S{interval}` | Busy-protocol interval = `communicationBusy` - 1 (`serial_comm.py:3866`) | Does not send it from the agent code. (The hub can queue any G-code text, `hub.rs:1468`; its own codes are in the section below) | `UNCHECKED` |
| `M114` | Position on pause/cancel (`logPositionOnPause`) and after `M400` (`serial_comm.py:1866`) | Sends: `M114` (agent/hub.rs:761) | `UNCHECKED` |
| `M115` | Right after connecting, firmware info and capabilities (`serial_comm.py:3891`) | Sends: `M115` (agent/hub.rs:725, agent/marlin.rs:1603) | `UNCHECKED` |
| `M140 S0` | Bed off on `M112` and in the default cancel script (`serial_comm.py:5665`) | Sends: `M140 S0` (agent/print.rs:125, hub/main.rs:1242) | `UNCHECKED` |
| `M154 S{interval}` | Position autoreport (`serial_comm.py:3852`) | Does not send it from the agent code. (The hub can queue any G-code text, `hub.rs:1468`; its own codes are in the section below) | `UNCHECKED` |
| `M155 S{interval}` | Temperature autoreport (`serial_comm.py:3828`) | Sends: `M155 S2 C7` (agent/hub.rs:556, agent/print.rs:216); `M155 S0 C0` (agent/print.rs:216); `M155 S2` (agent/print.rs:218); `M155 S0` (agent/print.rs:218) | `UNCHECKED` |
| `M20` | SD list (without long names) (`serial_comm.py:2152`) | Sends: `M20` (hub/main.rs:444, hub/sd_card.rs:151) | `UNCHECKED` |
| `M20 L T` | SD list with long names (EXTENDED_M20) (`serial_comm.py:2150`) | Sends: `M20 L T` (hub/sd_card.rs:159) | `UNCHECKED` |
| `M21` | SD init on connect (and `initSdCard`) (`serial_comm.py:2175`) | Sends: `M21` (agent/hub.rs:714) | `UNCHECKED` |
| `M22` | SD release (`serial_comm.py:2193`) | Does not send it from the agent code. (The hub can queue any G-code text, `hub.rs:1468`; its own codes are in the section below) | `UNCHECKED` |
| `M23 %s` | Select a file on the card (`startFileTransfer` path) (`serial_comm.py:1730`) | Sends: `M23 {}` (hub/sd_card.rs:221); `M23 {path}` (hub/sd_card.rs:223) | `UNCHECKED` |
| `M23 {filename}` | Select the file to print from the card (`serial_comm.py:1559`) | Sends: `M23 {}` (hub/sd_card.rs:221); `M23 {path}` (hub/sd_card.rs:223) | `UNCHECKED` |
| `M24` | Start / resume an SD print (`serial_comm.py:1582`) | Sends: `M24` (agent/marlin.rs:1372, hub/sd_card.rs:221, hub/sd_card.rs:223) | `UNCHECKED` |
| `M25` | Pause an SD print; `sdCancelCommand` default (`serial_comm.py:2036`) | Sends: `M25` (agent/marlin.rs:1371) | `UNCHECKED` |
| `M26 S0` | SD position reset on cancel (`serial_comm.py:1894`) | Does not send it from the agent code. (The hub can queue any G-code text, `hub.rs:1468`; its own codes are in the section below) | `UNCHECKED` |
| `M26 S{pos}` | SD position set when resuming from a position (`serial_comm.py:1571`) | Does not send it from the agent code. (The hub can queue any G-code text, `hub.rs:1468`; its own codes are in the section below) | `UNCHECKED` |
| `M27` | SD status poll; also after `M25` on cancel and after `M24` on resume (`serial_comm.py:1889`) | Does not send it from the agent code. (The hub can queue any G-code text, `hub.rs:1468`; its own codes are in the section below) | `UNCHECKED` |
| `M27 S{interval}` | SD status autoreport (`serial_comm.py:3839`) | Does not send it from the agent code. (The hub can queue any G-code text, `hub.rs:1468`; its own codes are in the section below) | `UNCHECKED` |
| `M28 %s` | Start writing a file to the card (`serial_comm.py:1653`) | Sends: `M28 {folder}/setready.g` (agent/hub.rs:641) | `UNCHECKED` |
| `M29` | Stop writing to the card (`serial_comm.py:1683`) | Sends: `M29` (agent/hub.rs:641) | `UNCHECKED` |
| `M30 {filename.lower()}` | Delete a file on the card (`serial_comm.py:2122`) | Does not send it from the agent code. (The hub can queue any G-code text, `hub.rs:1468`; its own codes are in the section below) | `UNCHECKED` |
| `M400` | Before `M114` on pause/cancel and at the end of a print (wait for moves) (`serial_comm.py:1911`) | Sends: `M400` (agent/marlin.rs:1569) | `UNCHECKED` |
| `M84` | Motors off in the default cancel script (`scripts.py:17 (script afterPrintCancelled)`) | Sends: `M84` (agent/hub.rs:641, agent/print.rs:125, hub/main.rs:1133) | `UNCHECKED` |
| `T0` | Tool selection (sanity check / tool change handling) (`serial_comm.py:2286`) | Does not send it from the agent code. (The hub can queue any G-code text, `hub.rs:1468`; its own codes are in the section below) | `UNCHECKED` |

### Default gcode scripts

| Script | OctoPrint (source) | Our agent now, plain Marlin | Status |
|---|---|---|---|
| `scripts.gcode.afterPrintCancelled` | After a cancel: `M84`, then per tool `M104 T<n> S0`, bed `M140 S0`, then `M106 S0` (`schema/config/scripts.py:17`, snippets `scripts.py:34-37`) | `STOP` = `M104 S0`, `M140 S0`, `M107`, `M84` (`print.rs:125`): other order, `M107` not `M106 S0`, `M104` without `T<n>` | `UNCHECKED` |
| `scripts.gcode.snippets.disable_hotends` | `M104 T<n> S0` for each extruder (one if a shared nozzle) (`scripts.py:35`) | `M104 S0` once (no `T`) | `UNCHECKED` |
| `scripts.gcode.snippets.disable_bed` | `M140 S0` if the profile has a heated bed (`scripts.py:36`) | `M140 S0` always | `UNCHECKED` |

## 8. What OctoPrint does around a G-code it queues, sends or has sent (handlers)

Handler names are `_gcode_<CODE>_<phase>` in `serial_comm.py` (phases `queuing`, `queued`, `sending`, `sent`). OctoPrint's work here is mostly host bookkeeping (position, tool, heating state) for its UI, plus `M110` renumbering, `M112`, `M155`/`M27`/`M154` state, `G4` timeout.

| Handler | OctoPrint / Marlin (source) | Our agent now, plain Marlin | Status |
|---|---|---|---|
| `_atcommand_cancel_queuing` | `@cancel` in a file cancels the print (`serial_comm.py:5800`) | `@` commands: not interpreted; a line starting with `@` is sent to the printer as it is | `UNCHECKED` |
| `_atcommand_pause_queuing` | `@pause` in a file pauses the print (`serial_comm.py:5794`) | As `@cancel` | `UNCHECKED` |
| `_atcommand_resume_queuing` | `@resume` resumes (`serial_comm.py:5808`) | As `@cancel` | `UNCHECKED` |
| `_gcode_G0_sent` | Tracks the position/feedrate of `G0`/`G1` (`serial_comm.py:5350`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_G28_sent` | Marks the position unknown after homing, tracks it (`serial_comm.py:5379`) | `G28` is in `long_running`; no position tracking of the host side | `UNCHECKED` |
| `_gcode_G4_sent` | Dwell: raises the timeout by the dwell time (`serial_comm.py:5689`) | `G4` is in `long_running` (15 min silence allowed), no dwell time used | `UNCHECKED` |
| `_gcode_G90_sent` | Tracks absolute positioning (`serial_comm.py:5705`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_G91_sent` | Tracks relative positioning (`serial_comm.py:5710`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_M104_sent` | Tracks the nozzle target (`serial_comm.py:5450`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_M106_sent` | Tracks the fan speed (`serial_comm.py:5725`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_M107_sent` | Tracks fan off (`serial_comm.py:5736`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_M109_sent` | Heat-up and wait: `_heating` true, timeouts relaxed (`serial_comm.py:5544`) | `marlin::long_running` / `heats` (`marlin.rs:447`, `marlin.rs:489`): the code gets the long silence (15 min), and `M108` may break it off | `UNCHECKED` |
| `_gcode_M110_sending` | Sets the host's line number to the `N` value, clears the kept lines, rewrites the command (`serial_comm.py:5615`) | `M110 N0` is sent only by `number_from_zero` (`marlin.rs:1572`); an `M110` in a job file is sent as any line: the agent's own number is not changed | `UNCHECKED` |
| `_gcode_M112_queuing` | Emergency stop right away, skips the queue (`serial_comm.py:5680`) | No `M112` handling in the agent; the stop goes through `M108` (heat-up) and the `STOP` list (`print.rs:125`) | `UNCHECKED` |
| `_gcode_M114_queued` | Resets the position log timers (`serial_comm.py:5684`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_M116_sent` | Wait for temperatures: `_heating` (`serial_comm.py:5568`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_M140_queuing` | Bed target checked / offsets applied (`serial_comm.py:5410`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_M140_sent` | Tracks the bed target (`serial_comm.py:5488`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_M141_queuing` | Chamber target (offsets) (`serial_comm.py:5429`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_M141_sent` | Tracks the chamber target (`serial_comm.py:5516`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_M154_sending` | Knows position autoreport is on/off (`serial_comm.py:5595`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_M155_sending` | Knows temperature autoreport is on/off (`serial_comm.py:5575`) | The agent sends it itself and does not track an on/off state by it; whether it reports is judged from `T:` lines (`reports_by_itself`, `marlin.rs:617`) | `UNCHECKED` |
| `_gcode_M190_sent` | Bed heat-up and wait: `_heating` (`serial_comm.py:5552`) | Same as `M109` (`long_running`, `heats`) | `UNCHECKED` |
| `_gcode_M191_sent` | Chamber heat-up and wait (`serial_comm.py:5560`) | Same as `M109` (`long_running`, `heats`) | `UNCHECKED` |
| `_gcode_M27_sending` | Knows SD status autoreport is on/off (`serial_comm.py:5585`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_M28_sent` | Starts streaming to SD: `triggerOkForM29` (`serial_comm.py:5391`) | No `M28`/`M29` for plain Marlin (MK3 menu only) | `UNCHECKED` |
| `_gcode_M29_sent` | Ends streaming to SD, triggers an `ok` (`triggerOkForM29`) (`serial_comm.py:5401`) | No `M28`/`M29` for plain Marlin (MK3 menu only) | `UNCHECKED` |
| `_gcode_M33_sending` | Remembers the long name asked for (`serial_comm.py:5605`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_M73_sent` | Progress and time left from `M73` in the file (`trustM73`) (`serial_comm.py:5741`) | `print::m73` (`print.rs:45`) reads P, R, S (and `m73_change_in` D, C) from the file's own `M73` lines for the progress shown | `UNCHECKED` |
| `_gcode_M82_sent` | Tracks absolute extruder (`serial_comm.py:5715`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_M83_sent` | Tracks relative extruder (`serial_comm.py:5720`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_T_queuing` | Tool change: checks the tool (`sanityCheckTools`), tool-change scripts (`serial_comm.py:5275`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_T_sending` | Tool change handling (`serial_comm.py:5316`) | No per-code handler for the code | `UNCHECKED` |
| `_gcode_T_sent` | Tool change: remembers the current tool (`serial_comm.py:5336`) | No per-code handler for the code | `UNCHECKED` |

## 9. Default settings (serial connector, `config_schema.py`)

| Setting | OctoPrint default (docstring) | Our agent now, plain Marlin | Status |
|---|---|---|---|
| `abortHeatupOnCancel` | Whether to send an abort heatup command on cancel. Default `True` (`config_schema.py:201`) | `M108` on stop if the printer has the emergency parser (`marlin.rs:1705`) | `UNCHECKED` |
| `ackMax` | Maximum number of ``ok`` acknowledgements to keep active. **DO NOT TOUCH THIS!** Changes can cause completely broken communication. Default `1` (`config_schema.py:271`) | One line in flight: send, wait for its `ok` (`wait_ok`); a file is sent line by line (`send`, `marlin.rs:1824`) | `UNCHECKED` |
| `additionalBaudrates` | Use this to define additional baud rates to offer for connecting to serial ports. Must be a list of valid integers. Default `[]` (`config_schema.py:133`) | No baud list | `UNCHECKED` |
| `additionalPorts` | Use this to define additional patterns to consider for serial port listing. Must be a list of valid `"glob" pattern <http://docs.python.org/3/library/glob.html>`_ Default `[]` (`config_schema.py:130`) | Port is given, not searched | `UNCHECKED` |
| `blockWhileDwelling` | Whether to block all sending to the printer while a ``G4`` (dwell) command is active (``true``, repetier) or not (``false``). Default `False` (`config_schema.py:256`) | No | `UNCHECKED` |
| `blockedCommands` | Commands which should not be sent to the printer, e.g. because they are known to block serial communication until physical interaction with the printer as is the case on most firmwares with the default ``M0`` and ``M1``. Default `['M0', 'M1']` (`config_schema.py:154`) | Not blocked: `M0`, `M1` of a file are sent (in `long_running`) | `UNCHECKED` |
| `blocklistedBaudrates` | Use this to define baudrates to ignore Default `[]` (`config_schema.py:139`) | No baud list | `UNCHECKED` |
| `blocklistedPorts` | Use this to define patterns of ports to ignore Default `[]` (`config_schema.py:136`) | Port is given | `UNCHECKED` |
| `capabilities.autoreport_pos` | Whether to enable position autoreport in the firmware if its support is detected. Default `True` (`config_schema.py:92`) | See `CAPABILITY_AUTOREPORT_POS` | `UNCHECKED` |
| `capabilities.autoreport_sdstatus` | Whether to enable SD printing autoreport in the firmware if its support is detected. Default `True` (`config_schema.py:89`) | See `CAPABILITY_AUTOREPORT_SD_STATUS` | `UNCHECKED` |
| `capabilities.autoreport_temp` | Whether to enable temperature autoreport in the firmware if its support is detected. Default `True` (`config_schema.py:86`) | See `CAPABILITY_AUTOREPORT_TEMP` | `UNCHECKED` |
| `capabilities.busy_protocol` | Whether to shorten the communication timeout if the firmware seems to support the busy protocol. Default `True` (`config_schema.py:95`) | See `CAPABILITY_BUSY_PROTOCOL` | `UNCHECKED` |
| `capabilities.chamber_temp` | Whether to process chamber temperature data from the firmware if its support is detected. Default `True` (`config_schema.py:98`) | See `CAPABILITY_CHAMBER_TEMP` | `UNCHECKED` |
| `capabilities.emergency_parser` | Whether to send emergency commands out of band if the firmware seems to support the emergency parser. Default `True` (`config_schema.py:101`) | See `CAPABILITY_EMERGENCY_PARSER` | `UNCHECKED` |
| `capabilities.extended_m20` | Whether to request extended M20 (file list) output from the firmware if its support is detected. Default `True` (`config_schema.py:104`) | See `CAPABILITY_EXTENDED_M20` | `UNCHECKED` |
| `capabilities.lfn_write` | Whether to enable long filename support for SD card writes if the firmware reports support for it. Default `True` (`config_schema.py:107`) | See `CAPABILITY_LFN_WRITE` | `UNCHECKED` |
| `checksumRequiringCommands` | Commands which need to always be send with a checksum. Default `['M110']` (`config_schema.py:172`) | All lines carry a number and checksum (`numbered`, `marlin.rs:61`) | `UNCHECKED` |
| `disableSdPrintingDetection` | Whether to disable the detection of SD print jobs triggered from the printer. Default `False` (`config_schema.py:268`) | No | `UNCHECKED` |
| `emergencyCommands` | Commands which are considered emergency commands and will be sent immediately, jumping OctoPrint's internal queues. Default `['M112', 'M108', 'M410']` (`config_schema.py:169`) | `M108` only (`marlin.rs:1705`); `M112`, `M410` never sent | `UNCHECKED` |
| `enableShutdownActionCommand` | Whether to enable support for the shutdown action command, allowing the printer to shut down OctoPrint and the system it's running on. Default `False` (`config_schema.py:295`) | Not applicable: `shutdown` is ignored | `UNCHECKED` |
| `enableUploadToSd` | Whether to enable uploading to SD through the serial connection. Can be *really* slow and is thus not recommended. Default `True` (`config_schema.py:210`) | Not applicable: no upload to the card for plain Marlin | `UNCHECKED` |
| `encoding` | Encoding to use when talking to a machine. ``ascii`` limits access to characters 0-127, ``latin_1`` enables access to the "extended" ascii characters 0-255. Other values can be used, see `Python's standard encodings <https://docs.python.org/3/library/codecs.html#standard-encodings>`_. Default `'ascii'` (`config_schema.py:288`) | Bytes read lossily as UTF-8, NULs dropped (`marlin.rs:1459`); written as UTF-8 | `UNCHECKED` |
| `errorHandling` | What to do when receiving unhandled errors from the printer. Default `'disconnect'` (`config_schema.py:181`) | Fixed: an error not in the harmless lists ends the print and is reported | `UNCHECKED` |
| `exclusive` | Whether to request exclusive access to the serial port. Default `True` (`config_schema.py:112`) | Not checked (serialport defaults) | `UNCHECKED` |
| `externalHeatupDetection` | Whether to enable external heatup detection (to detect heatup triggered e.g. through the printer's LCD panel or while printing from the printer's memory). Causes issues with Repetier's "first ok then response" approach to communication, so disable for printers running Repetier firmware. Default `True` (`config_schema.py:240`) | None: no `_heating` state from `T:` lines | `UNCHECKED` |
| `firmwareDetection` | Whether to attempt to auto detect the firmware of the printer and adjust settings accordingly (``true``) or not and rely on manual configuration (``false``). Default `True` (`config_schema.py:253`) | None (see section 5) | `UNCHECKED` |
| `helloCommand` | Command to send in order to initiate a handshake with the printer. Default `'M110 N0'` (`config_schema.py:175`) | `M110 N0` after an unnumbered `M400`, without `start` needed (`marlin.rs:1562`); also after `PRUSA Fir` (unnumbered) on every connection (`hub.rs:925`, `marlin.rs:1510`) | `UNCHECKED` |
| `ignoreEmptyPorts` | Whether to ignore that there are no serial ports detected or show a message Default `False` (`config_schema.py:285`) | Not applicable | `UNCHECKED` |
| `ignoredCommands` | Commands which should not be sent to the printer and just silently ignored. An example of when you may wish to use this would be to manually change a filament on M600, by using that as a Pausing command. Default `[]` (`config_schema.py:160`) | None ignored | `UNCHECKED` |
| `lastLineBufferSize` |  Default `50` (`config_schema.py:187`) | `KEPT_LINES` 64 (`marlin.rs:28`); OctoPrint 50 | `UNCHECKED` |
| `log` | Whether to log whole communication to ``serial.log`` (warning: might decrease performance). Default `False` (`config_schema.py:118`) | Not checked (the agent prints to stdout) | `UNCHECKED` |
| `logPositionOnCancel` | Whether to request and log the current position from the printer on a cancel. Default `False` (`config_schema.py:198`) | No | `UNCHECKED` |
| `logPositionOnPause` | Whether to request and log the current position from the printer on a pause. Default `True` (`config_schema.py:195`) | No | `UNCHECKED` |
| `logResends` | Whether to log resends to octoprint.log or not. Invaluable debug tool without performance impact, leave on if possible please. Default `True` (`config_schema.py:189`) | Resends are said as `Event::Resent` | `UNCHECKED` |
| `longRunningCommands` | Commands which are known to take a long time to be acknowledged by the firmware, e.g. homing, dwelling, auto leveling etc. Default `['G4', 'G28', 'G29', 'G30', 'G32', 'M400', 'M226', 'M600']` (`config_schema.py:142`) | `long_running` (`marlin.rs:447`): G4 G28 G29 G30 G32 M400 M226 M600 as OctoPrint, plus G80 M0 M1 M109 M190 M191 M303 M701 M702 | `UNCHECKED` |
| `lowLatency` | Whether to request low latency mode on the serial port. Default `False` (`config_schema.py:115`) | Not checked | `UNCHECKED` |
| `maxCommunicationTimeouts.idle` | Max. timeouts when the printer is idle. Default `2` (`config_schema.py:75`) | `MAX_POKES` 3 for all (`marlin.rs:35`) | `UNCHECKED` |
| `maxCommunicationTimeouts.long` | Max. timeouts when a long running command is active. Default `5` (`config_schema.py:81`) | `MAX_POKES` 3, but the silence for long codes is 15 min (`LONG_SILENCE`) | `UNCHECKED` |
| `maxCommunicationTimeouts.printing` | Max. timeouts when the printer is printing. Default `5` (`config_schema.py:78`) | `MAX_POKES` 3 | `UNCHECKED` |
| `maxConsecutiveResends` | Number of times to allow to resend the same line before the print job gets considered as failed. Default `10` (`config_schema.py:262`) | `MAX_SAME_RESENDS` 10 (`marlin.rs:44`) | `UNCHECKED` |
| `maxNotSdPrinting` | Maximum number of 'Not SD Printing' messages to support before assuming the print was cancelled externally Default `2` (`config_schema.py:234`) | No count: one `Not SD printing` ends the card print state | `UNCHECKED` |
| `maxWritePasses` | Maximum number of write attempts to serial during which nothing can be written before the communication with the printer is considered dead and OctoPrint will disconnect with an error. Default `5` (`config_schema.py:127`) | Not checked (`write_all`) | `UNCHECKED` |
| `pausingCommands` | Commands which should cause OctoPrint to pause any ongoing prints. Default `['M0', 'M1', 'M25']` (`config_schema.py:166`) | None: a `M0`/`M1`/`M25` of a file is sent and the printer pauses; ours knows only the screen's paused states, and for MK3 `M601`/`M25` (`print.rs:473`) | `UNCHECKED` |
| `repetierTargetTemp` | Whether the printer sends repetier style target temperatures in the format ``TargetExtr0:<temperature>`` instead of attaching that information to the regular ``M105`` responses. Default `False` (`config_schema.py:237`) | No | `UNCHECKED` |
| `resendRatioStart` | Initial resend ratio percentage Default `100` (`config_schema.py:282`) | No resend ratio check | `UNCHECKED` |
| `resendRatioThreshold` | Percentage of resend requests among all sent lines that should be considered critical. Default `10` (`config_schema.py:279`) | No resend ratio check | `UNCHECKED` |
| `sanityCheckTools` | Whether to sanity check the tool count. Default `True` (`config_schema.py:274`) | No | `UNCHECKED` |
| `sdAlwaysAvailable` | Whether to always assume that an SD card is present in the printer. Needed by some firmwares which don't report the SD card status properly. Default `False` (`config_schema.py:225`) | No; card presence from `echo:SD card ok` and file lists | `UNCHECKED` |
| `sdCancelCommand` | Command to cancel SD prints Default `'M25'` (`config_schema.py:231`) | `M524` (Marlin) for stop (`marlin.rs:1373`); OctoPrint default `M25` | `UNCHECKED` |
| `sdLowerCase` | Whether to treat all sd card file names as lower case Default `False` (`config_schema.py:228`) | No (MK3 path only, `hub/sd_card.rs:221`) | `UNCHECKED` |
| `sdRelativePath` | Specifies whether firmware expects relative paths for selecting SD files. Default `False` (`config_schema.py:222`) | No | `UNCHECKED` |
| `sendChecksum` | Specifies when OctoPrint should send linenumber + checksum with every GCODE command. Default `'print'` (`config_schema.py:213`) | Always numbered with checksum, idle and printing, every command (`marlin.rs:61`); OctoPrint default `print`: only while printing | `UNCHECKED` |
| `sendChecksumWithUnknownCommands` | Specifies whether OctoPrint should also send linenumber + checksum with commands that are *not* detected as valid GCODE (as in, they do not match the regular expression ``^\s*([GM]\d+|T)``). Default `False` (`config_schema.py:216`) | Always | `UNCHECKED` |
| `sendM112OnError` | Whether to send an ``M112`` on encountered errors. Default `True` (`config_schema.py:265`) | No `M112` sent on errors | `UNCHECKED` |
| `supportFAsCommand` | Whether to support ``F`` on its own as a valid GCODE command (``true``) or not (``false``). Default `False` (`config_schema.py:250`) | No | `UNCHECKED` |
| `supportResendsWithoutOk` | Whether to support resends without follow-up ok. Default `'detect'` (`config_schema.py:192`) | `RESEND_OK_WAIT` 1 s then resends anyway (`marlin.rs:1716`), not detected | `UNCHECKED` |
| `supportWait` | Whether to support ``wait`` responses from the printer and interpret them as a call to send more commands. Default `True` (`config_schema.py:247`) | `wait` is not an ok; asks `M105` (see `wait`) | `UNCHECKED` |
| `suppressSecondHello` | Whether to suppress the second hello command. Might be required for some printer configurations with custom hello commands. Default `False` (`config_schema.py:178`) | No second hello | `UNCHECKED` |
| `terminalLogSize` | Size of log lines to keep for logging error context. Default `20` (`config_schema.py:184`) | Not applicable | `UNCHECKED` |
| `timeout.baudrateDetectionPause` | Pause between baudrate detection attempts, in seconds. Default `1.0` (`config_schema.py:67`) | No baud detection: fixed `--baud`, default 115200 (`hub.rs:918`, `main.rs:25`) | `UNCHECKED` |
| `timeout.communication` | Timeout during serial communication, in seconds. Default `30.0` (`config_schema.py:40`) | `SILENT_TIMEOUT` 30 s (`marlin.rs:30`), then a numbered `M105` | `UNCHECKED` |
| `timeout.communicationBusy` | Timeout during serial communication when busy protocol support is detected, in seconds. Default `3.0` (`config_schema.py:43`) | No busy interval is set; `busy:` is waited on as any line | `UNCHECKED` |
| `timeout.connection` | Timeout for waiting to establish a connection with the selected port, in seconds. Default `10.0` (`config_schema.py:37`) | `BOOT_WAIT` 10 s (`marlin.rs:46`), ends after 2 s quiet; no hello retries | `UNCHECKED` |
| `timeout.detectionConsecutive` | Timeout on consecutive detection attempts, in seconds. Default `2.0` (`config_schema.py:34`) | No detection of the serial protocol | `UNCHECKED` |
| `timeout.detectionFirst` | Timeout during initial detection attempt, in seconds. Default `10.0` (`config_schema.py:31`) | No detection of the serial protocol | `UNCHECKED` |
| `timeout.posAutoreport` | Autoreporting interval to request for the position report, in seconds. Default `5.0` (`config_schema.py:61`) | No `M154` | `UNCHECKED` |
| `timeout.positionLogWait` | Time to wait to receive a position response before considering it unresponded, in seconds. Default `10.0` (`config_schema.py:70`) | No position log on pause/cancel | `UNCHECKED` |
| `timeout.resendOk` | Timeout after which to trigger an internal ``ok`` after a resend is received (works around a bug present in some firmwares), in seconds. Default `0.5` (`config_schema.py:64`) | `RESEND_OK_WAIT` 1 s (`marlin.rs:41`); OctoPrint's configured default is 0.5 s, its code fallback 1.0 s (`serial_comm.py`) | `UNCHECKED` |
| `timeout.sdStatus` | Timeout after which to query the SD status while SD printing. Default `1.0` (`config_schema.py:55`) | `M27 C` once per idle report (`REPORT_EVERY` 2 s, `hub.rs:25`) when the card is known | `UNCHECKED` |
| `timeout.sdStatusAutoreport` | Autoreporting interval to request for the SD status report, in seconds. Default `1.0` (`config_schema.py:58`) | No `M27 S` | `UNCHECKED` |
| `timeout.temperature` | Timeout after which to query temperature when no target is set, in seconds. Default `5.0` (`config_schema.py:46`) | `M105` every 2 s while idle (`REPORT_EVERY`, `hub.rs:25`), whatever the target | `UNCHECKED` |
| `timeout.temperatureAutoreport` | Autoreporting interval to request for the temperature report, in seconds. Default `2.0` (`config_schema.py:52`) | `M155 S2` only while a job runs (`print.rs:218`) | `UNCHECKED` |
| `timeout.temperatureTargetSet` | Timeout after which to query temperature when a target is set, in seconds. Default `2.0` (`config_schema.py:49`) | No separate interval (always 2 s) | `UNCHECKED` |
| `triggerOkForM29` | Whether to automatically trigger an ok for ``M29`` (a lot of versions of this command are buggy and the response skips on the ok). Default `True` (`config_schema.py:299`) | No `M28`/`M29` for plain Marlin | `UNCHECKED` |
| `trustM73` | Whether to trust M73 in printed GCODE files for progress reporting. Default `True` (`config_schema.py:302`) | `M73` of the file is read for progress (`print.rs:45`) | `UNCHECKED` |
| `unknownCommandsNeedAck` | Specifies whether OctoPrint should also use up acknowledgments (``ok``) for commands that are *not* detected as valid GCODE (as in, they do not match the regular expression ``^\s*([GM]\d+|T)``). Default `False` (`config_schema.py:219`) | No: an unknown command's `ok` is waited for like any | `UNCHECKED` |
| `useParityWorkaround` | Whether to use the parity workaround needed for connecting to some printers. Default `'detect'` (`config_schema.py:259`) | No | `UNCHECKED` |
| `waitForStartOnConnect` | Whether OctoPrint should wait for the ``start`` response from the printer before trying to send commands during connect. Default `False` (`config_schema.py:204`) | No `start` wait needed; boot wait 10 s | `UNCHECKED` |
| `waitToLoadSdFileList` | Specifies whether OctoPrint should wait to load the SD card file list until the first firmware capability report is processed. Default `True` (`config_schema.py:207`) | The hub waits for the command's answer | `UNCHECKED` |

## 10. Fallback defaults in the code

The values used in `serial_comm.py` when a setting is missing. They differ from `config_schema.py` in places (e.g. `resendOk` 1.0 vs 0.5, `communicationBusy` 2 vs 3).

| Fallback | OctoPrint (source) | Our agent now, plain Marlin | Status |
|---|---|---|---|
| `BAUDRATE_AUTO` | Value 0: detect the baud rate (`serial_comm.py:206`) | Not applicable: fixed baud | `UNCHECKED` |
| `MINIMAL_SD_TIMESTAMP` | A file date before this (2008) is taken as none (`serial_comm.py:202`) | Not applicable | `UNCHECKED` |
| `PORT_AUTO` | `AUTO`: pick the port (`serial_comm.py:205`) | Not applicable | `UNCHECKED` |
| `STANDARD_BAUDRATES` | Tried when detecting: 115200, 250000, 230400, 57600, 38400, 19200, 9600 (`serial_comm.py:208`) | Not applicable | `UNCHECKED` |
| `busy_default=4.0` | Fallback value in the code (`serial_comm.py:3933`) | Idle `M105` every 2 s, not 4/5 s | `UNCHECKED` |
| `communication=30.0` | Fallback value in the code (`serial_comm.py:3981`) | `SILENT_TIMEOUT` 30 s | `UNCHECKED` |
| `communicationBusy=2` | Fallback value in the code (`serial_comm.py:2430`) | No `M113` | `UNCHECKED` |
| `communicationBusy=2.0` | Fallback value in the code (`serial_comm.py:3979`) | No `M113` | `UNCHECKED` |
| `communicationBusy=3` | Fallback value in the code (`serial_comm.py:3861`) | No `M113` | `UNCHECKED` |
| `detectionConsecutive=2.0` | Fallback value in the code (`serial_comm.py:3975`) | No detection | `UNCHECKED` |
| `detectionFirst=10.0` | Fallback value in the code (`serial_comm.py:3972`) | No detection | `UNCHECKED` |
| `posAutoreport=5` | Fallback value in the code (`serial_comm.py:3848`) | No `M154` | `UNCHECKED` |
| `positionLogWait=10.0` | Fallback value in the code (`serial_comm.py:1760`) | No position log | `UNCHECKED` |
| `resendOk=1.0` | Fallback value in the code (`serial_comm.py:4566`) | `RESEND_OK_WAIT` 1 s | `UNCHECKED` |
| `sdStatus=1.0` | Fallback value in the code (`serial_comm.py:3962`) | `M27 C` per 2 s report | `UNCHECKED` |
| `sdStatusAutoreport=1` | Fallback value in the code (`serial_comm.py:3835`) | No `M27 S` | `UNCHECKED` |
| `target_default=2.0` | Fallback value in the code (`serial_comm.py:3934`) | 2 s always | `UNCHECKED` |
| `temperatureAutoreport=2` | Fallback value in the code (`serial_comm.py:3824`) | `M155 S2` during a job only | `UNCHECKED` |

## 11. Methods that do the talking (`serial_comm.py`)

| Method | OctoPrint (source) | Our agent now, plain Marlin | Status |
|---|---|---|---|
| `_checksum_enabled` | Checksum on if `sendChecksum` is `always`, or `print` while printing (`serial_comm.py:5146`) | Always on (every line) | `UNCHECKED` |
| `_continue_sending` | Takes the next thing to send on each `ok`: queue, job queue, then the file (`serial_comm.py:3682`) | `send` (`marlin.rs:1824`): line by line, `send_numbered` then `wait_ok` | `UNCHECKED` |
| `_do_increment_and_send_with_checksum` | Numbers the line (`N<n>`), adds `*checksum`, writes it (`serial_comm.py:5161`) | `numbered` + `send_numbered` (`marlin.rs:61`, `1613`) | `UNCHECKED` |
| `_do_send` | Writes one command, with or without checksum (`serial_comm.py:5154`) | `write_line` (`marlin.rs:1481`) | `UNCHECKED` |
| `_do_send_with_checksum` | Writes `N<n> <cmd>*<cs>` (`serial_comm.py:5168`) | `numbered` | `UNCHECKED` |
| `_do_send_without_checksum` | Writes the bare command (`serial_comm.py:5180`) | `ask_unnumbered` (`marlin.rs:1523`) and the unnumbered `M400` (`marlin.rs:1569`): used for `PRUSA Fir` and `M400` on every connect | `UNCHECKED` |
| `_emergency_force_send` | Sends `M108`/`M410` at once if the printer has the emergency parser (`serial_comm.py:5765`) | `M108` while heating, if `emergency_parser` (`marlin.rs:1705`), numbered | `UNCHECKED` |
| `_enqueue_for_sending` | Puts a line in the send queue (renumbers resends) (`serial_comm.py:4746`) | No queue; one line at a time | `UNCHECKED` |
| `_finish_heatup` | Ends the heating state when the temperature is reached (`serial_comm.py:3673`) | No heating state | `UNCHECKED` |
| `_get_communication_timeout_interval` | Serial read timeout: `communicationBusy` when the busy protocol is on, else `communication` (`serial_comm.py:3967`) | Serial read timeout 200 ms (`hub.rs:918`) | `UNCHECKED` |
| `_get_new_communication_timeout` | Next time-out point (`serial_comm.py:3994`) | `last_heard` in `wait_ok` | `UNCHECKED` |
| `_get_sd_status_timer_interval` | SD poll interval: `sdStatus` (`serial_comm.py:3961`) | `REPORT_EVERY` 2 s | `UNCHECKED` |
| `_get_temperature_timer_interval` | Temperature poll interval: `temperature` (busy 4.0 fallback); `temperatureTargetSet` when a target is set (`serial_comm.py:3932`) | `REPORT_EVERY` 2 s | `UNCHECKED` |
| `_handle_errors` | Error classes (section 3) (`serial_comm.py:4160`) | `Reply::Error`, `harmless_error`, `fatal` (`marlin.rs:1803`) | `UNCHECKED` |
| `_handle_ok` | An `ok`: one line acknowledged, clears the clear-to-send, sends the next (`serial_comm.py:3457`) | `wait_ok` (`marlin.rs:1694`): `unacked` counted down, returns at 0 | `UNCHECKED` |
| `_handle_resend_request` | Resend: checks it is possible, resends from the asked line on; `maxConsecutiveResends`; resend ratio (`serial_comm.py:4460`) | `resend_from` (`marlin.rs:1644`): resends from the asked line on, then lines after; `MAX_SAME_RESENDS` 10; no resend ratio | `UNCHECKED` |
| `_handle_timeout` | On silence: sends `M105` (idle) or resends; counts timeouts against `maxCommunicationTimeouts`; errors out after the max (`serial_comm.py:3486`) | `poke` `M105` numbered, 3 times, then `the printer stopped answering` error (`marlin.rs:1754`) | `UNCHECKED` |
| `_monitor` | The read loop (everything in sections 1 to 3) (`serial_comm.py:2337`) | `wait_ok` and `Printer::listen` | `UNCHECKED` |
| `_needs_checksum` | Whether this command needs a checksum (`checksumRequiringCommands`, `sendChecksum`) (`serial_comm.py:5134`) | Always | `UNCHECKED` |
| `_onConnected` | After connect: starts the `M105` and `M27` timers, `resetLineNumbers`, `M115`, then SD init (`M21`) (`serial_comm.py:3871`) | `open_printer` / `get_to_know` (`hub.rs:912`, `hub.rs:722`): `M115` after the connect; no `M21`; no timers, the hub loop asks `M105`/`M114`/`M220`/`M221` every 2 s (`ask_idle`, `hub.rs:743`) | `UNCHECKED` |
| `_on_external_reset` | On `start` while operational: clears queues, `M110 N0`, sets the autoreports again (`serial_comm.py:3906`) | `start` gives error `the printer restarted`; the caller reconnects | `UNCHECKED` |
| `_perform_detection_step` | Serial detection: tries baud rates and sends `M105` (`serial_comm.py:3559`) | None | `UNCHECKED` |
| `_poll_sd_status` | `M27` if an SD file is selected or printing, not if autoreporting (`serial_comm.py:3796`) | `follow_sd` `M27 C` (`marlin.rs:1322`) | `UNCHECKED` |
| `_poll_temperature` | `M105` if not autoreporting, not busy/heating/dwelling (`serial_comm.py:3772`) | `ask_idle` `M105` (`hub.rs:759`) | `UNCHECKED` |
| `_processTemperatures` | Reads a temperature line (tools, bed, chamber, custom), offsets, `_heating` and `_finish_heatup` (`serial_comm.py:2258`) | `marlin::heat` | `UNCHECKED` |
| `_process_atcommand_phase` | `@command` handlers (`serial_comm.py:5091`) | None | `UNCHECKED` |
| `_process_command_phase` | Runs the phase handlers (section 8) and hooks (`serial_comm.py:4958`) | None: a line goes as it is | `UNCHECKED` |
| `_readline` | Reads one line, decodes (ascii, latin1 fallback), strips (`serial_comm.py:4314`) | `read_line` (`marlin.rs:1459`) | `UNCHECKED` |
| `_resendCheckPossibility` | Checks the line asked for is still kept (`lastLineBufferSize`) (`serial_comm.py:4624`) | `resend_from` errors if the line is no longer kept (64) | `UNCHECKED` |
| `_resendNextCommand` | Resends the next kept line (`serial_comm.py:4581`) | `resend_from` | `UNCHECKED` |
| `_resendSameCommand` | Resends the same line again (`serial_comm.py:4578`) | `resend_from` | `UNCHECKED` |
| `_resendSimulateOk` | After a resend request with no `ok` in `resendOk` seconds: simulates the `ok` (`serial_comm.py:4570`) | `resend_from` after `RESEND_OK_WAIT` (`marlin.rs:1716`) | `UNCHECKED` |
| `_sendCommand` | One command through the phases, queue and send (`serial_comm.py:4651`) | `command` / `send_line` (`marlin.rs:1594`, `1818`) | `UNCHECKED` |
| `_send_firmware_info` | Hands the `M115` info to plugins (`serial_comm.py:3430`) | `command` keeps firmware and caps after `M115` (`marlin.rs:1603`) | `UNCHECKED` |
| `_send_from_command_queue` | Takes from the command queue (`serial_comm.py:3997`) | Hub commands via `carry_out` (`hub.rs:1941`) and `PrintJob::inject` (`print.rs:533`) | `UNCHECKED` |
| `_send_from_job` | Takes the next line of the file (`serial_comm.py:4402`) | `PrintJob` loop (`print.rs:147`) | `UNCHECKED` |
| `_send_from_job_queue` | Takes from the job queue (scripts, pause/cancel) (`serial_comm.py:4446`) | No job queue | `UNCHECKED` |
| `_send_loop` | The sending thread: waits for clear-to-send, writes (`serial_comm.py:4797`) | No thread: `wait_ok` is called after each line | `UNCHECKED` |
| `_set_autoreport_pos_interval` | `M154 S<n>` (`serial_comm.py:3843`) | None | `UNCHECKED` |
| `_set_autoreport_sdstatus_interval` | `M27 S<n>` (`serial_comm.py:3832`) | None | `UNCHECKED` |
| `_set_autoreport_temperature_interval` | `M155 S<n>` (`serial_comm.py:3821`) | `M155 S2` / `S0` in `PrintJob` (`print.rs:216`) | `UNCHECKED` |
| `_set_busy_protocol_interval` | `M113 S<n>` (`serial_comm.py:3857`) | None | `UNCHECKED` |
| `_trigger_emergency_stop` | `M112` twice, heaters off, closes the port (`serial_comm.py:5645`) | None | `UNCHECKED` |
| `_trigger_error` | Error state, closes the port (when disconnecting) (`serial_comm.py:4261`) | `Event::Error` then the job ends (`print.rs:202`) | `UNCHECKED` |
| `_use_up_clear` | Uses up an `ok` credit for a force-sent command (`serial_comm.py:4784`) | None | `UNCHECKED` |
| `apply_temperature_offsets` | Adds the temperature offsets to `M104`/`M109`/`M140`/`M190` commands (`serial_comm.py:6466`) | None | `UNCHECKED` |
| `cancelFileTransfer` | Ends an SD upload (`serial_comm.py:1668`) | None | `UNCHECKED` |
| `cancelPrint` | Cancel: `M108`, SD `M25`+`M27`+`M26 S0`, `M400`+`M114` when logging position, then the cancel script (`serial_comm.py:1794`) | `PrintJob::stop` (`print.rs:548`): `M108` if heating and emergency parser; then `STOP`; SD: `M524` (`marlin.rs:1373`) | `UNCHECKED` |
| `canonicalize_temperatures` | Maps `T`/`T0`/`B`/`C`/custom into tools, bed, chamber (`serial_comm.py:6620`) | None | `UNCHECKED` |
| `convert_feedback_controls` | Feedback controls of the user's custom controls (`serial_comm.py:6571`) | None | `UNCHECKED` |
| `convert_pause_triggers` | User-defined pause/resume/toggle trigger regexes (`serial_comm.py:6543`) | None | `UNCHECKED` |
| `deleteSdFile` | `M30 <file>` then list again (`serial_comm.py:2103`) | None | `UNCHECKED` |
| `fakeOk` | Test aid (`serial_comm.py:1338`) | None | `UNCHECKED` |
| `gcode_and_subcode_for_cmd` | Code and subcode of a command (`serial_comm.py:6955`) | `long_running` takes the first word | `UNCHECKED` |
| `gcode_command_for_cmd` | Code of a command (`G0`) (`serial_comm.py:6940`) | Same | `UNCHECKED` |
| `getSdFiles` | Cached SD list (`serial_comm.py:2092`) | Hub keeps the list (`sd_files`) | `UNCHECKED` |
| `initSdCard` | `M21` (`serial_comm.py:2165`) | No `M21` for plain Marlin (only the MK3 branch, `hub.rs:714`) | `UNCHECKED` |
| `parse_capability_line` | `Cap:NAME:0|1` (`serial_comm.py:6847`) | `marlin::capabilities` | `UNCHECKED` |
| `parse_file_list_line` | A line of an `M20` list: name, size, date (`serial_comm.py:6717`) | Hub side (`crates/hub`), not audited here | `UNCHECKED` |
| `parse_firmware_line` | `M115` `KEY:value` pairs (`serial_comm.py:6814`) | `marlin::firmware`, `machine_type` | `UNCHECKED` |
| `parse_position_line` | Position report (`serial_comm.py:6903`) | `marlin::position` | `UNCHECKED` |
| `parse_resend_line` | Resend line number (`serial_comm.py:6885`) | `marlin::parse` | `UNCHECKED` |
| `parse_temperature_line` | Temperature line (`serial_comm.py:6769`) | `marlin::heat` | `UNCHECKED` |
| `process_gcode_line` | Strips comments, normalizes a line (`serial_comm.py:6532`) | `marlin::clean` (cuts `;` comments) | `UNCHECKED` |
| `refreshSdFiles` | `M20` or `M20 L T` (`serial_comm.py:2130`) | The hub queues `M20 L T` (`sd_card.rs:159`) | `UNCHECKED` |
| `releaseSdCard` | `M22` (`serial_comm.py:2181`) | None | `UNCHECKED` |
| `resetLineNumbers` | `M110 N<n>` (`serial_comm.py:2220`) | `number_from_zero` (`marlin.rs:1562`): `M400` then `M110 N0` | `UNCHECKED` |
| `sayHello` | Sends `helloCommand` (`serial_comm.py:2205`) | `number_from_zero` | `UNCHECKED` |
| `selectFile` | Selects a local or SD file (`serial_comm.py:1712`) | Hub queues `M23 <path>` + `M24` (`sd_card.rs:221`) | `UNCHECKED` |
| `sendCommand` | Public: queue a command (`serial_comm.py:1341`) | `Printer::command` | `UNCHECKED` |
| `sendGcodeScript` | Runs a gcode script (`afterPrintCancelled`...) (`serial_comm.py:1484`) | None | `UNCHECKED` |
| `setPause` | Pause (SD `M25`, host: stop sending) / resume (SD `M24`+`M27`); logs the position with `M400`+`M114` if configured (`serial_comm.py:1958`) | `PrintJob::pause` (`print.rs:526`); SD: `control_sd` `M25` / `M24` (`marlin.rs:1360`); no `M27` after resume; no position log | `UNCHECKED` |
| `startFileTransfer` | Upload to the card: `M28` .. `M29` (`serial_comm.py:1623`) | None | `UNCHECKED` |
| `startPrint` | Host print: file lines; SD print: `M23`, optional `M26 S<pos>`, `M24` (`serial_comm.py:1517`) | Host: `PrintJob::start` (`print.rs:147`); SD: `M23 <path>\nM24` by the hub; no `M26 S` | `UNCHECKED` |
| `strip_comment` | Cuts the `;` comment (`serial_comm.py:6517`) | `marlin::clean` | `UNCHECKED` |
| `strip_linenumber_and_checksum` | Cuts `N..` and `*..` (`serial_comm.py:6981`) | Not needed | `UNCHECKED` |
| `unselectFile` | Clears the selected file (`serial_comm.py:1749`) | None | `UNCHECKED` |

## What we send that OctoPrint does not

Every G-code literal in the agent's code (`crates/agent/src/*.rs`, test code left out) and in the hub's `sd_card.rs`, `main.rs`, `queue.rs` whose form is not one OctoPrint sends (the forms of section 7). The agent also runs any G-code text the hub queues (`carry_out`, `hub.rs:1941`), and the files of a print, line by line: those are the user's, not listed. Codes for the MK3 family only are covered by `docs/prusalink-audit.md`.

| G-code form we send | Where from (file:line) | Why / for which machines | Status |
|---|---|---|---|
| `D3 Ax0D05 X{:08x}` | agent/print.rs:228 | MK3 family only: EEPROM reads/writes as PrusaLink (`hub.rs:601-619` in the MK3 branch, `print.rs:226` `if mk3`) | `UNCHECKED` |
| `D3 AxD05 C4` | agent/hub.rs:605 | MK3 family only: EEPROM reads/writes as PrusaLink (`hub.rs:601-619` in the MK3 branch, `print.rs:226` `if mk3`) | `UNCHECKED` |
| `D3 AxD49 C88` | agent/hub.rs:617 | MK3 family only: EEPROM reads/writes as PrusaLink (`hub.rs:601-619` in the MK3 branch, `print.rs:226` `if mk3`) | `UNCHECKED` |
| `D3 AxDA1 C1` | agent/hub.rs:618 | MK3 family only: EEPROM reads/writes as PrusaLink (`hub.rs:601-619` in the MK3 branch, `print.rs:226` `if mk3`) | `UNCHECKED` |
| `D3 AxF91 C4` | agent/hub.rs:601 | MK3 family only: EEPROM reads/writes as PrusaLink (`hub.rs:601-619` in the MK3 branch, `print.rs:226` `if mk3`) | `UNCHECKED` |
| `D3 AxFED C4` | agent/hub.rs:610 | MK3 family only: EEPROM reads/writes as PrusaLink (`hub.rs:601-619` in the MK3 branch, `print.rs:226` `if mk3`) | `UNCHECKED` |
| `D3 AxFF1 C4` | agent/hub.rs:609 | MK3 family only: EEPROM reads/writes as PrusaLink (`hub.rs:601-619` in the MK3 branch, `print.rs:226` `if mk3`) | `UNCHECKED` |
| `D3 AxFFF C1` | agent/hub.rs:619 | MK3 family only: EEPROM reads/writes as PrusaLink (`hub.rs:601-619` in the MK3 branch, `print.rs:226` `if mk3`) | `UNCHECKED` |
| `G0 X0 Y0` | hub/main.rs:1252 | Hub button `Go to origin` (`hub/main.rs:1252`); `G90 G0 X0 Y0` is the Grbl laser's (`hub/main.rs:1046`) | `UNCHECKED` |
| `G1` | hub/main.rs:1199, hub/main.rs:1200 | Hub move/jog buttons (`hub/main.rs:1108`, `1234-1239`, `1249-1250`) | `UNCHECKED` |
| `G1 F100 E-10` | hub/main.rs:1250 | Hub move/jog buttons (`hub/main.rs:1108`, `1234-1239`, `1249-1250`) | `UNCHECKED` |
| `G1 F100 E10` | hub/main.rs:1249 | Hub move/jog buttons (`hub/main.rs:1108`, `1234-1239`, `1249-1250`) | `UNCHECKED` |
| `G1 F{feed} {}` | hub/main.rs:1108 | Hub move/jog buttons (`hub/main.rs:1108`, `1234-1239`, `1249-1250`) | `UNCHECKED` |
| `G1 X-10 F3000` | hub/main.rs:1235 | Hub move/jog buttons (`hub/main.rs:1108`, `1234-1239`, `1249-1250`) | `UNCHECKED` |
| `G1 X10 F3000` | hub/main.rs:1234 | Hub move/jog buttons (`hub/main.rs:1108`, `1234-1239`, `1249-1250`) | `UNCHECKED` |
| `G1 Y-10 F3000` | hub/main.rs:1237 | Hub move/jog buttons (`hub/main.rs:1108`, `1234-1239`, `1249-1250`) | `UNCHECKED` |
| `G1 Y10 F3000` | hub/main.rs:1236 | Hub move/jog buttons (`hub/main.rs:1108`, `1234-1239`, `1249-1250`) | `UNCHECKED` |
| `G1 Z-10 F600` | hub/main.rs:1239 | Hub move/jog buttons (`hub/main.rs:1108`, `1234-1239`, `1249-1250`) | `UNCHECKED` |
| `G1 Z10 F600` | hub/main.rs:1238 | Hub move/jog buttons (`hub/main.rs:1108`, `1234-1239`, `1249-1250`) | `UNCHECKED` |
| `G28` | hub/main.rs:1233, hub/main.rs:1262, hub/main.rs:1279 | Hub `Home` (`hub/main.rs:1233`; `G28 X Y Z` MK3 `1121`; `G28\nG29\nM420 V` Probe `1279`); OctoPrint sends no `G28` of its own | `UNCHECKED` |
| `G28 X Y Z` | hub/main.rs:1121 | Hub `Home` (`hub/main.rs:1233`; `G28 X Y Z` MK3 `1121`; `G28\nG29\nM420 V` Probe `1279`); OctoPrint sends no `G28` of its own | `UNCHECKED` |
| `G29` | hub/main.rs:1279 | Hub Probe button: `G28`, `G29`, `M420 V` (`hub/main.rs:1279`); OctoPrint sends none of them | `UNCHECKED` |
| `G90` | hub/main.rs:1086, hub/main.rs:1234, hub/main.rs:1235, hub/main.rs:1236, hub/main.rs:1237, hub/main.rs:1238, hub/main.rs:1239, hub/main.rs:1252 | Hub move buttons `G91`..`G90` (`hub/main.rs:1086`, `1234-1239`) | `UNCHECKED` |
| `G90 G0 X0 Y0` | hub/main.rs:1046 | Hub move buttons `G91`..`G90` (`hub/main.rs:1086`, `1234-1239`) | `UNCHECKED` |
| `G91` | hub/main.rs:1086, hub/main.rs:1108, hub/main.rs:1234, hub/main.rs:1235, hub/main.rs:1236, hub/main.rs:1237, hub/main.rs:1238, hub/main.rs:1239 | Same as `G90` | `UNCHECKED` |
| `G92 X0 Y0` | hub/main.rs:1045, hub/main.rs:1251 | Hub `Set origin here` (`hub/main.rs:1251`; also the Grbl laser's `1045`) | `UNCHECKED` |
| `M0 Insert the filament` | hub/main.rs:1537 | MK3 family only: `M0 Insert the filament` (`hub/main.rs:1537`) | `UNCHECKED` |
| `M1 FW COMM ERR. Aborted` | agent/print.rs:454 | MK3 family only: `M1 FW COMM ERR. Aborted` on a lost print (`print.rs:454`) | `UNCHECKED` |
| `M104` | hub/main.rs:1130, hub/main.rs:1348 | Hub controls and presets: `M104 S215` etc. (`hub/main.rs:1240-1242`), typed values `M104 S{n}` (`hub/main.rs:1130`, `1348`), the MK3 load flow, and `M104 S0` in the stop list (`print.rs:125`; OctoPrint: `M104 T<n> S0`) | `UNCHECKED` |
| `M104 S0` | agent/print.rs:125, hub/main.rs:1242 | Hub controls and presets: `M104 S215` etc. (`hub/main.rs:1240-1242`), typed values `M104 S{n}` (`hub/main.rs:1130`, `1348`), the MK3 load flow, and `M104 S0` in the stop list (`print.rs:125`; OctoPrint: `M104 T<n> S0`) | `UNCHECKED` |
| `M104 S215` | hub/main.rs:1240 | Hub controls and presets: `M104 S215` etc. (`hub/main.rs:1240-1242`), typed values `M104 S{n}` (`hub/main.rs:1130`, `1348`), the MK3 load flow, and `M104 S0` in the stop list (`print.rs:125`; OctoPrint: `M104 T<n> S0`) | `UNCHECKED` |
| `M104 S240` | hub/main.rs:1241 | Hub controls and presets: `M104 S215` etc. (`hub/main.rs:1240-1242`), typed values `M104 S{n}` (`hub/main.rs:1130`, `1348`), the MK3 load flow, and `M104 S0` in the stop list (`print.rs:125`; OctoPrint: `M104 T<n> S0`) | `UNCHECKED` |
| `M104 S{hot:.0}` | hub/main.rs:1535 | Hub controls and presets: `M104 S215` etc. (`hub/main.rs:1240-1242`), typed values `M104 S{n}` (`hub/main.rs:1130`, `1348`), the MK3 load flow, and `M104 S0` in the stop list (`print.rs:125`; OctoPrint: `M104 T<n> S0`) | `UNCHECKED` |
| `M106 S255` | hub/main.rs:1245 | Hub button `Print fan on` `M106 S255` (`hub/main.rs:1245`); OctoPrint's default cancel script has `M106 S0` | `UNCHECKED` |
| `M107` | agent/print.rs:125, hub/main.rs:1246 | Hub button `Print fan off` (`hub/main.rs:1246`) and the stop list (`print.rs:125`); OctoPrint's default cancel script has `M106 S0` | `UNCHECKED` |
| `M109 S{extrude:.0}` | hub/main.rs:1533 | Hub buttons: load/unload filament `M109 S<hot>` before `M701`/`M702` (`hub/main.rs:1545`); MK3 form `M109 S<extrude>` (`hub/main.rs:1533`). OctoPrint sends no `M109` itself | `UNCHECKED` |
| `M109 S{hot:.0}` | hub/main.rs:1545 | Hub buttons: load/unload filament `M109 S<hot>` before `M701`/`M702` (`hub/main.rs:1545`); MK3 form `M109 S<extrude>` (`hub/main.rs:1533`). OctoPrint sends no `M109` itself | `UNCHECKED` |
| `M117 ~Cannot re-print` | hub/queue.rs:123 | MK3 family only: PrusaLink's screen lines (`lcd.rs:643`, `hub.rs:891`, `hub/queue.rs:123` goes to any machine as part of `Cannot re-print`) | `UNCHECKED` |
| `M117 ~Recovery failed` | agent/hub.rs:891 | MK3 family only: PrusaLink's screen lines (`lcd.rs:643`, `hub.rs:891`, `hub/queue.rs:123` goes to any machine as part of `Cannot re-print`) | `UNCHECKED` |
| `M117 ~{}` | agent/lcd.rs:643 | MK3 family only: PrusaLink's screen lines (`lcd.rs:643`, `hub.rs:891`, `hub/queue.rs:123` goes to any machine as part of `Cannot re-print`) | `UNCHECKED` |
| `M140` | hub/main.rs:1131, hub/main.rs:1354 | Same as `M104`: bed (`hub/main.rs:1131`, `1240-1242`, `1354`, `print.rs:125`); OctoPrint: `M140 S0` | `UNCHECKED` |
| `M140 S60` | hub/main.rs:1240 | Same as `M104`: bed (`hub/main.rs:1131`, `1240-1242`, `1354`, `print.rs:125`); OctoPrint: `M140 S0` | `UNCHECKED` |
| `M140 S80` | hub/main.rs:1241 | Same as `M104`: bed (`hub/main.rs:1131`, `1240-1242`, `1354`, `print.rs:125`); OctoPrint: `M140 S0` | `UNCHECKED` |
| `M140 S{bed:.0}` | hub/main.rs:1530 | Same as `M104`: bed (`hub/main.rs:1131`, `1240-1242`, `1354`, `print.rs:125`); OctoPrint: `M140 S0` | `UNCHECKED` |
| `M18` | hub/main.rs:1247 | Hub button `Motors off` (`hub/main.rs:1247`); OctoPrint's default cancel script uses `M84` | `UNCHECKED` |
| `M20 LT` | agent/hub.rs:1468, agent/hub.rs:712, hub/sd_card.rs:159 | MK3 family only (`hub.rs:712`, `hub/sd_card.rs:159`); non-MK3 machines are asked `M20 L T` (OctoPrint's, with the EXTENDED_M20 capability), but without checking the capability | `UNCHECKED` |
| `M220` | agent/hub.rs:766, agent/print.rs:412, hub/main.rs:1128, hub/main.rs:1321 | Speed factor read back every report and every 5 s in a job (`hub.rs:766`, `print.rs:412`); hub buttons set it for MK3 (`hub/main.rs:1128`, `1321`). OctoPrint never sends `M220`/`M221` by itself | `UNCHECKED` |
| `M221` | agent/hub.rs:767, agent/print.rs:413, hub/main.rs:1129, hub/main.rs:1322 | Same as `M220` | `UNCHECKED` |
| `M23` | agent/hub.rs:1951 | Fragment of the hub's `M23 <path>` (MK3: lower case, `hub/sd_card.rs:221`); the literal is a prefix test in `carry_out` (`hub.rs:1951`) | `UNCHECKED` |
| `M27 C` | agent/marlin.rs:1324 | SD status per report, Marlin form with the file name (`marlin.rs:1324`); OctoPrint's is `M27` | `UNCHECKED` |
| `M27 P` | agent/marlin.rs:1324 | MK3 family only (`marlin.rs:1324`) | `UNCHECKED` |
| `M300 P100 S3200` | agent/lcd.rs:19 | MK3 family only: PrusaLink's chimes (`lcd.rs`, `print.rs:454`, `hub/main.rs:1537`); `hub/queue.rs:123` `M300 P200 S600` goes to any machine (Cannot re-print) | `UNCHECKED` |
| `M300 P14 S50` | agent/lcd.rs:21 | MK3 family only: PrusaLink's chimes (`lcd.rs`, `print.rs:454`, `hub/main.rs:1537`); `hub/queue.rs:123` `M300 P200 S600` goes to any machine (Cannot re-print) | `UNCHECKED` |
| `M300 P200 S600` | hub/queue.rs:123 | MK3 family only: PrusaLink's chimes (`lcd.rs`, `print.rs:454`, `hub/main.rs:1537`); `hub/queue.rs:123` `M300 P200 S600` goes to any machine (Cannot re-print) | `UNCHECKED` |
| `M300 P25 S0` | agent/lcd.rs:19 | MK3 family only: PrusaLink's chimes (`lcd.rs`, `print.rs:454`, `hub/main.rs:1537`); `hub/queue.rs:123` `M300 P200 S600` goes to any machine (Cannot re-print) | `UNCHECKED` |
| `M300 P25 S4800` | agent/lcd.rs:19 | MK3 family only: PrusaLink's chimes (`lcd.rs`, `print.rs:454`, `hub/main.rs:1537`); `hub/queue.rs:123` `M300 P200 S600` goes to any machine (Cannot re-print) | `UNCHECKED` |
| `M300 P500 S1` | hub/main.rs:1537 | MK3 family only: PrusaLink's chimes (`lcd.rs`, `print.rs:454`, `hub/main.rs:1537`); `hub/queue.rs:123` `M300 P200 S600` goes to any machine (Cannot re-print) | `UNCHECKED` |
| `M300 P600 S5` | agent/lcd.rs:20 | MK3 family only: PrusaLink's chimes (`lcd.rs`, `print.rs:454`, `hub/main.rs:1537`); `hub/queue.rs:123` `M300 P200 S600` goes to any machine (Cannot re-print) | `UNCHECKED` |
| `M300 P75 S0` | agent/lcd.rs:19 | MK3 family only: PrusaLink's chimes (`lcd.rs`, `print.rs:454`, `hub/main.rs:1537`); `hub/queue.rs:123` `M300 P200 S600` goes to any machine (Cannot re-print) | `UNCHECKED` |
| `M300 S880 P200` | agent/print.rs:454 | MK3 family only: PrusaLink's chimes (`lcd.rs`, `print.rs:454`, `hub/main.rs:1537`); `hub/queue.rs:123` `M300 P200 S600` goes to any machine (Cannot re-print) | `UNCHECKED` |
| `M420` | agent/hub.rs:868 | MK3 family only: the mesh read with `M420` (`hub.rs:868`) | `UNCHECKED` |
| `M420 V` | agent/hub.rs:868, hub/main.rs:1279 | `M420 V` prints the bed mesh after a Probe (`hub/main.rs:1279`, `hub.rs:868`); `M420` without `V` MK3 only | `UNCHECKED` |
| `M5` | hub/main.rs:1244 | Hub button `Laser off` (`hub/main.rs:1244`); for lasers; in the same table as printer buttons | `UNCHECKED` |
| `M524` | agent/marlin.rs:1373 | Stop an SD print on Marlin (`marlin.rs:1373`); OctoPrint's `sdCancelCommand` is `M25` then `M27`, `M26 S0` | `UNCHECKED` |
| `M552 P0.0.0.0` | agent/hub.rs:719 | MK3 family only: the agent's address for its menu (`hub.rs:718`) | `UNCHECKED` |
| `M552 P{ip}` | agent/hub.rs:718 | MK3 family only: the agent's address for its menu (`hub.rs:718`) | `UNCHECKED` |
| `M601` | agent/marlin.rs:1368, agent/print.rs:311 | MK3 family only for pause (`marlin.rs:1368`, `print.rs:311`); not Marlin's dispatch | `UNCHECKED` |
| `M602` | agent/hub.rs:1627, agent/marlin.rs:1369, agent/print.rs:352 | MK3 family only (resume) | `UNCHECKED` |
| `M603` | agent/hub.rs:1632, agent/hub.rs:890, agent/marlin.rs:1370, agent/print.rs:271, agent/print.rs:454 | MK3 family only (stop, comm error, `print.rs:271`, `print.rs:454`, `hub.rs:890`) | `UNCHECKED` |
| `M701` | hub/main.rs:1537, hub/main.rs:1545 | Hub `Load filament` for every Marlin (`hub/main.rs:1537`, `1545`): Marlin's own filament change codes | `UNCHECKED` |
| `M702` | hub/main.rs:1539, hub/main.rs:1545 | Hub `Unload filament` (same) | `UNCHECKED` |
| `M72 S0` | agent/hub.rs:1458 | MK3 family only (Set ready, `hub.rs:1457`, `hub/main.rs:1851` `prusa_mk3_family`) | `UNCHECKED` |
| `M72 S1` | agent/hub.rs:1457, hub/main.rs:1851 | MK3 family only (Set ready, `hub.rs:1457`, `hub/main.rs:1851` `prusa_mk3_family`) | `UNCHECKED` |
| `M73` | agent/hub.rs:846, agent/print.rs:47 | MK3 family: PrusaLink's forms (`print.rs:427`, `hub.rs:846`, `hub.rs:858`). Plain Marlin: `M73 P{percent} R{left}` (`print.rs:429`) is told to the printer for a file without its own `M73` (OctoPrint reads `M73`, never sends it) | `UNCHECKED` |
| `M73 P{percent} R{left}` | agent/print.rs:429 | MK3 family: PrusaLink's forms (`print.rs:427`, `hub.rs:846`, `hub.rs:858`). Plain Marlin: `M73 P{percent} R{left}` (`print.rs:429`) is told to the printer for a file without its own `M73` (OctoPrint reads `M73`, never sends it) | `UNCHECKED` |
| `M73 Q-1 S-1 C-1 P-1 R-1 D-1` | agent/hub.rs:858, agent/print.rs:484 | MK3 family: PrusaLink's forms (`print.rs:427`, `hub.rs:846`, `hub.rs:858`). Plain Marlin: `M73 P{percent} R{left}` (`print.rs:429`) is told to the printer for a file without its own `M73` (OctoPrint reads `M73`, never sends it) | `UNCHECKED` |
| `M73 Q{percent} S{left} C-1 P{percent} R{left} D-1` | agent/print.rs:427 | MK3 family: PrusaLink's forms (`print.rs:427`, `hub.rs:846`, `hub.rs:858`). Plain Marlin: `M73 P{percent} R{left}` (`print.rs:429`) is told to the printer for a file without its own `M73` (OctoPrint reads `M73`, never sends it) | `UNCHECKED` |
| `M75` | agent/print.rs:229, agent/print.rs:353 | MK3 family only (print timer) | `UNCHECKED` |
| `M76` | agent/print.rs:312 | MK3 family only | `UNCHECKED` |
| `M77` | agent/print.rs:270, agent/print.rs:479 | MK3 family only | `UNCHECKED` |
| `M79 S"PC"` | agent/marlin.rs:1420 | MK3 family only (host keep-alive, `marlin.rs:1420`) | `UNCHECKED` |
| `M83` | hub/main.rs:1249, hub/main.rs:1250 | Hub button `Extrude`/`Retract`, PrusaLink's form (`hub/main.rs:1249-1250`) | `UNCHECKED` |
| `M862.1 Q` | agent/hub.rs:703 | MK3 family only (`hub.rs:703`) | `UNCHECKED` |
| `M862.2 P` | agent/marlin.rs:903 | `M862.2 Q` MK3 family; also sent to non-MK3 only when the firmware name starts `Prusa` (Buddy), `hub.rs:726`; `M862.2 P` is only an answer read (`marlin.rs:903`) | `UNCHECKED` |
| `M862.2 Q` | agent/hub.rs:676, agent/hub.rs:731, agent/marlin.rs:1507, agent/marlin.rs:1512 | `M862.2 Q` MK3 family; also sent to non-MK3 only when the firmware name starts `Prusa` (Buddy), `hub.rs:726`; `M862.2 P` is only an answer read (`marlin.rs:903`) | `UNCHECKED` |
| `M876 S{button}` | agent/marlin.rs:1404, agent/marlin.rs:1712 | Marlin / Buddy host prompts: the answer to `//action:prompt_*` (`marlin.rs:1404`, `1712`); not OctoPrint's (it hands prompts to the action-command prompt plugin) | `UNCHECKED` |
| `PRUSA Fir` | agent/hub.rs:677, agent/marlin.rs:1507, agent/marlin.rs:1510 | Sent on EVERY connection, also plain Marlin: `connect_as_prusalink` (`hub.rs:925`, `marlin.rs:1510`) asks `PRUSA Fir` unnumbered first; the MK3 detection. Not OctoPrint's. A plain Marlin answers `echo:Unknown command` | `UNCHECKED` |
| `PRUSA SN` | agent/hub.rs:705 | Sent on EVERY connection, also plain Marlin: `connect_as_prusalink` (`hub.rs:925`, `marlin.rs:1510`) asks `PRUSA Fir` unnumbered first; the MK3 detection. Not OctoPrint's. A plain Marlin answers `echo:Unknown command` | `UNCHECKED` |
