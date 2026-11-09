# Audit: the agent and hub against PrusaLink

PrusaLink is Prusa's own host for the MK3 family over serial. Everything it sends to the
printer, everything it asks the printer for, and every printer message it reacts to, set
against what Maker Connect's agent (`crates/agent`) and hub (`crates/hub`) do.

**How this list was made (2026-11-09):** taken mechanically from the PrusaLink source in
`reference/Prusa-Link` (commit c453996, 2026-10-17), not from memory:

- **Sends:** every `enqueue_instruction`, `enqueue_matchable`, `enqueue_list_from_str`,
  `do_instruction`, `do_matchable`, `do_multimatch` and raw `serial.write` in
  `prusa/link/**/*.py`, with the G-code it sends.
- **Polls:** every `WatchedItem` in `printer_adapter/printer_polling.py` with its G-code.
- **Parses:** every pattern in `printer_adapter/structures/regular_expressions.py`.
- **Ours:** looked up in the agent and hub source per row (the text the printer sends, or
  the G-code), then read where it is used.

Status: **same** (we do what PrusaLink does) · **differs** (we do it, otherwise) ·
**missing** (we do not) · **n/a** (a PrusaLink feature that does not apply to us, with why).
Rows marked **fixed, uncommitted** are fixed in the working tree, not yet released.

## 1. What PrusaLink sends

| G-code | PrusaLink: what for (source) | Ours | Status |
|---|---|---|---|
| `M155 S2 C7` | Temperatures, position and fans auto-reported **always**, and switched on again when any of the three is not heard for 60 s (`auto_telemetry.py`) | Always on for the MK3 from connecting, again after 60 s of silence (`start_reports`, `Screen::reports_by_itself`); kept on after a print | **fixed** (tried on the MK3S: fans and position while idle) |
| `M155 S0 C0` | Auto-report off when PrusaLink stops (`auto_telemetry.py`) | Not sent to an MK3 (kept on); other Marlin: off after a print | same for the MK3 |
| `G28 X Y Z` | Home from the web controls, always with axes, so no mesh bed levelling (`web/controls.py`) | `G28 W` for the MK3 family (the same: home only), `G28` elsewhere | **fixed** (test `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| `G90`/`G91` + `G1 F… X Y Z` | Move the head (`web/controls.py`) | `G91`, `G1`, `G90` | same |
| `M220 S` | Speed factor (`web/controls.py`) | `M220 S` | same |
| `M221 S` | Flow factor (`web/controls.py`) | `M221 S` | same |
| `M84` | Motors off (`web/controls.py`) | `M18` (the same in Marlin) | same |
| `M83` + `G1 F… E…` | Extrude or retract an amount (`web/controls.py`) | Control: Extrude / Retract 10 mm, `M83` + `G1 F100 E±10` (PrusaLink's E feed) | **fixed** (test) |
| `M104 S` / `M140 S` | Set nozzle / bed (`web/controls.py`) | `M104` / `M140` | same |
| `M140`, `M109`, `M104` | Preheat for load: bed; wait for 90 % of the temperature (rounded to 5) unless that hot already; then the full temperature (`command_handlers.py`) | The same for the MK3 family (bed from the loaded filament where it says one) | **fixed** (test) |
| `M300 P500 S1` + `M0 Insert the filament` + `M701` | Load filament: a beep and a prompt on the printer, then load (`command_handlers.py`) | The same for the MK3 family | **fixed** (test) |
| `M702` | Unload (`command_handlers.py`) | `M702` | same |
| `M601` / `M602` / `M603` | Pause / resume / stop: sent **once**, then waited for the state up to 15 s (`STATE_CHANGE_TIMEOUT`); the command fails if not reached (`command_handlers.py` `_try_until_state`). *(Corrected: an earlier version of this row said "sent again".)* | Sent once; not paused / not resumed within 15 s is said as the job's problem | **fixed** |
| `M75` / `M76` / `M77` | Printer's print timer start / pause / stop (`file_printer.py`) | `M75` / `M76` / `M77` | same |
| `M73 P R Q S C D` | Progress and time left sent to the printer during a host print, only for a file without its own `M73`: every 100 lines and 10 before the end (`file_printer.py`, `util.py`) | The same (`print.rs`, `STATS_EVERY`, `STATS_TAIL`) | **fixed** (tests `a_file_without_m73_is_given_it`, `a_file_with_m73_keeps_its_own`) |
| `M72 S1` / `M72 S0` | Printer "ready" state on (Connect's Set ready) and off (Cancel ready) (`command_handlers.py`) | `M72 S1` when "Print taken" is pressed for an MK3 through an agent; there is no "cancel ready" in the hub, so no `M72 S0` | **fixed** for S1 (test); S0 n/a |
| `M28 …/setready.g`, `M84`, `M29` | Writes a "Set ready" file into a folder "PrusaLink menu" on the SD card, only when the user made that folder (`filesystem/sd_card.py`) | — | n/a: an opt-in for PrusaLink's own menu folder; firmware 3.14 has Set Ready in its menu (`M72`) |
| `M23 path` + `M24` | Print from the SD card, checking "File opened" / "open failed" (`command_handlers.py`) | `M23`, `M24` | differs: we check "open failed" only |
| `M20 LT` | List the SD card with long names and times (`filesystem/sd_card.py`) | `M20 L T` | same |
| `M21` | Initialise the SD card (`filesystem/sd_card.py`) | Sent when connecting to an MK3 | **fixed** (tried: SD card "in") |
| `M27 P` | SD print state with the file's short path (`printer_polling.py`) | `M27`, which gives the long name (firmware `CardReader::getStatus`) | differs on purpose: the long name is what is shown; files are matched through `M20` |
| `M79 S"PL"` | Host keep-alive (`keepalive.py`) | `M79 S"MC"` every 12 s | same |
| `M552 P<ip>` | Tell the printer its IP address, shown in its menu (`ip_updater.py`) | The agent's address, sent when connecting to an MK3 | **fixed** (to look at on the printer) |
| `M117 ~text` | Messages on the printer's screen ("Cannot re-print", "Recovery failed"), and PrusaLink's own status (`lcd_printer.py`) | "Cannot re-print" (with Reprint below); "Recovery failed": our power-cut resume has its own path | fixed for Cannot re-print; PrusaLink's own status lines n/a (they tell PrusaLink's web and uploads) |
| `M300 …` | Chimes: upload, error, ready (`lcd_printer.py`); the load beep; Cannot re-print; comm error | The load beep, Cannot re-print, comm error | fixed for these; upload/ready chimes n/a (PrusaLink's own upload and status) |
| `M110 N0` | Line numbers back to 0 (`serial/serial_queue.py`) | `M110` | same |
| `M400` | Workaround slot around `M110` (`serial/serial_queue.py`) | `M400` | same |
| `M300 S880 P200` + `M603` + `M1 FW COMM ERR. Aborted` | On a communication failure mid-print: beep, stop, message (`serial/serial_queue.py`) | The same for an MK3, as far as it still listens | **fixed** |
| `;C32u2_RMD` / `;C32u2_RME` | Turn the MK3's USB chip's reset off and on, around SD prints (`serial/serial_adapter.py`) | — | n/a: **off by default in PrusaLink** (`reset_disabling = False`, its ini warns it wears out the chip's EEPROM); not needed on the GPIO serial |
| `PRUSA Fir` | Firmware version (`printer_polling.py`) | `M115` | same (other route) |
| `PRUSA SN` | Serial number (`printer_polling.py`) | Asked when connecting to an MK3 | **fixed** (tried: CZPX2918X004XK74799) |
| `M862.2 Q` | Printer type (`printer_polling.py`) | `M862.2 Q` | same |
| `M862.1 Q` | Nozzle diameter (`printer_polling.py`) | Asked when connecting to an MK3 | **fixed** (tried: 0.40) |
| `M707 A0x00…0x03` | MMU firmware version (`printer_polling.py`) | Asked when connecting to an MK3 | **fixed** (test; tried without an MMU: none) |
| `M420` | Read the bed mesh, again after levelling (`printer_polling.py`, `invalidate_mbl`) | `M420` after levelling was seen (MK3: same output as `G81`, `gcode_G81_M420`); the hub's Probe button measures on request | **fixed** |
| `M220` / `M221` (no S) | Speed and flow read back every second (`printer_polling.py`) | Every report (2 s) idle, every 5 s while printing | **fixed** (tried: 100 % / 100 %) |
| `M73` (no args) | The printer's own progress and time left, from the slicer's `M73` in the file (`printer_polling.py`) | SD prints: asked, in the printer's mode; host prints: the file's `M73` as it goes by | **fixed** (tests) |
| `D3 Ax…` (read) | EEPROM: steel sheets and the active one, job id, FlashAir, print mode silent/normal (`printer_polling.py`) | Sheets, the active one and the print mode every 10 s (addresses checked against firmware 3.14 `eeprom.h`) | **fixed** for sheet and mode (tried: BH-25, normal); job id and FlashAir n/a (below) |
| `D3 Ax0D05 X…` (write) | Store the job id in the printer's EEPROM (`job.py`) | — | n/a: our job and its place are kept by the agent (`recovery.rs`) |
| `D3 Ax0d15/0d25` (write) | Write a serial number into the printer (`web/lib/wizard.py`) | — | n/a (factory wizard) |
| custom G-code | Each line run, checked against "Unknown command" and "cold extrusion prevented" (`command_handlers.py`) | Same checks | same |

## 2. What the printer says, and whether we read it

| PrusaLink pattern | Printer message | Ours | Status |
|---|---|---|---|
| `TEMPERATURE_REGEX` | `T:… /… B:… /… T0:… @:… B@:… P:… A:…` | `marlin::heat` | same |
| `POSITION_REGEX` | `X:… Y:… Z:… E:… Count X:…` | Treated as noise until now | **missing → fixed, uncommitted** |
| `FAN_REGEX` | `E0:… RPM PRN1:… RPM E0@:… PRN1@:…` | `marlin::heat` | same (only heard while printing, see §1) |
| `HEATING_REGEX`, `HEATING_HOTEND_REGEX` | `T:… E:… B:…`, `T:… E:… W:…` while `M109`/`M190` wait | Read | same |
| `CONFIRMATION_REGEX` | `ok…`, `Done saving file.` | `ok` | same (no `M28` used) |
| `RESEND_REGEX` | `Resend: N` | Read | same |
| `REJECTION_REGEX` | `echo:Unknown command: "…"`, `Unknown … code`, `echo: cold extrusion prevented` | Read | same |
| `BUSY_REGEX` | `echo:busy: processing` | Read | same |
| `ATTENTION_REGEX` | `echo:busy: paused for user` | Read | same |
| `PAUSE_PRINT_REGEX` … `NOT_READY_REGEX` | `// action:pause`, `paused`, `resume`, `resumed`, `cancel`, `ready`, `not_ready` | Read | same |
| `REPRINT_REGEX` | `// action:start` (Reprint from the printer's menu): the last job again, else a beep and "Cannot re-print" | The same, through the hub, with its checks (idle, bed clear, file suits) | **fixed** (test) |
| `PP_RECOVER_REGEX`, `PP_AUTO_RECOVER_REGEX` | `// action:uvlo_recovery_ready`, `uvlo_auto_recovery_ready` | Read | same |
| `ERROR_REGEX` | `Error:Printer halted. kill() called!`, `Printer stopped due to errors…` | Read | same |
| `ERROR_REASON_REGEX` | MINTEMP / MAXTEMP (bed), thermal runaway, … | Read | same (check every reason against the pattern) |
| `ATTENTION_REASON_REGEX` | Bed levelling failed (sensor too high, didn't trigger, …) | Read | same |
| `FAN_ERROR_REGEX` | `Extruder/Hotend/Print fan speed is lower than expected` | Read | same |
| `TM_ERROR_LOG_REGEX`, `TM_ERROR_CLEARED` | Thermal model error and cleared | Read | same |
| `TM_CAL_START_REGEX`, `TM_CAL_END_REGEX` | Thermal model calibration: PrusaLink sends **nothing** meanwhile (it blocks its queue) | The agent only listens until it ends; reported busy, "Calibrating its thermal model" | **fixed** (test) |
| `MBL_TRIGGER_REGEX` | `tmc…_home_enter(…)`, `echo:enqueing "G80"`: the mesh is read again later (`invalidate_mbl`) | The same | **fixed** (test) |
| `MBL_REGEX` | `M420` mesh output | `marlin::mesh` (the same format as `G81`) | same |
| `OPEN_RESULT_REGEX` | `File opened: … Size: …` / `open failed` | Both | **fixed** |
| `FILE_OPEN_REGEX`, `START_PRINT_REGEX` | `echo:enqueing "M23 …"`, `echo:enqueing "M24"` (print started from the printer's menu) | Seen through `M27` every report (its file's long name); `File opened` read | same in effect |
| `PRINT_DONE_REGEX` | `Done printing file` | `M27` every 2 s sees the end | same in effect |
| `M27_OUTPUT_REGEX` | path, `Not SD printing`, `Print saved`, `SD print paused`, `SD printing byte a/b` | Read | same |
| `PRINT_INFO_REGEX` | `NORMAL MODE: Percent done: …; print time remaining in mins: …; Change in mins: …` | `marlin::print_info`, in the printer's mode | **fixed** (test) |
| `SD_PRESENT_REGEX`, `SD_EJECTED_REGEX` | `echo:SD card ok`, `SD init fail`, `SD card released` | Read; "SD card: in / none" on the machine page | **fixed** (test; tried: in) |
| `LFN_CAPTURE` | `M20 LT` listing | Read (hub) | same |
| `PRINTER_BOOT_REGEX` | `start` (the printer restarted) | Read | same |
| `POWER_PANIC_REGEX` | `INT4` | Read | same |
| `LCD_UPDATE_REGEX` | `LCD status changed` | Read (noise) | same |
| `M110_REGEX` | `M110 N…` echoed by the printer | Not needed: our `M110` is answered by its `ok`, and an echo is a plain line we skip | n/a |
| `D3_OUTPUT_REGEX` | EEPROM read output | `marlin::eeprom_bytes` | **fixed** (test; tried) |
| `MMU_MAJOR/MINOR/REVISION/BUILD_REGEX` | `echo:MMU2:<R0 A…` … | `marlin::mmu_firmware` | **fixed** (test) |
| `MMU_SLOT_REGEX` | `echo:MMU2:MMU2tool=N` (the active slot) | Read; "MMU: slot N" on the machine page | **fixed** (test) |
| `MMU_Q0_REGEX`, `MMU_Q0_RESPONSE_REGEX`, `MMU_PROGRESS_REGEX` | MMU state and progress messages | Progress: PrusaLink's 26 messages, shown; errors in the replies (already); the Q0 polls themselves are noise for PrusaLink too | **fixed** (test) |
| `RESET_ACTIVATED_REGEX`, `RESET_DEACTIVATED_REGEX` | `Reset mode activated/deactivated` (answer to `;C32u2_RMD/RME`) | — | n/a (with `;C32u2` in §1) |
| `PRINTER_TYPE_REGEX`, `FW_REGEX`, `NOZZLE_REGEX`, `PERCENT_REGEX`, `SN_REGEX` | Answers to the polls in §1 | Type, nozzle, percent, serial; firmware through `M115` | **fixed** (tests; tried) |
| `VALID_USERNAME_REGEX`, `VALID_PASSWORD_REGEX`, `URLS_FOR_WIZARD`, `VALID_SN_REGEX`, `NEW_SN_REGEX`, `ANY_REGEX` | PrusaLink's own web and wizard | — | n/a (not printer messages) |

## 3. Totals (after the fixes of 2026-11-09)

- **Same or fixed:** every row of §1 and §2 except those marked n/a. Fixed rows have a test,
  and those marked "tried" were seen on the MK3S (firmware 3.14.1) through the agent.
- **n/a, with why in the row:** `setready.g` (an opt-in folder of PrusaLink's), `;C32u2_RMD`
  (off by default in PrusaLink), writing the job id into the EEPROM (our agent keeps its
  job), `M72 S0` (no "cancel ready" in the hub), PrusaLink's own LCD status and chimes for
  its web and uploads, `M110` echo, PrusaLink's web and wizard.
- **Still to see on the printer** (needs someone there): Home now homes without
  levelling; the IP on its screen (`M552`); load with its beep and "Insert the filament";
  Extrude; "Reprint" from its menu; "ready" on its screen after Print taken; a print's
  progress on its screen (`M73`); a thermal model calibration.

## 5. Every item by name

Every item `scripts/prusalink-audit-check.py` takes from the PrusaLink source, that the
sections above do not name one by one. **OPEN** marks what is still to build (done in this
order after the list was complete); the check fails while an item is missing here.

### Patterns and handlers named singly

| Item | What it is | Ours | Status |
|---|---|---|---|
| `CANCEL_REGEX` | `// action:cancel`: stopped from the printer's menu | `Action::Cancel`: the job ends, "stopped at the printer" | same |
| `PAUSED_REGEX` | `// action:paused`: it paused itself | `Action::Paused` | same |
| `RESUME_PRINT_REGEX` | `// action:resume`: resume from its knob | `Action::Resume` | same |
| `RESUMED_REGEX` | `// action:resumed` | `Action::Resume` | same |
| `READY_REGEX` | `// action:ready`: Set Ready in its menu | `Action::Ready` → the hub's "Print taken" | same |
| `MMU_MAJOR_REGEX`, `MMU_MINOR_REGEX`, `MMU_REVISION_REGEX`, `MMU_BUILD_REGEX` | Answers to `M707 A0x00`…`A0x03` | `marlin::mmu_firmware` | fixed (test) |

### Polled values (`WatchedItem`s of printer_polling.py)

| Item | What PrusaLink does | Ours | Status |
|---|---|---|---|
| `network_info` | PrusaLink's own network (its web, Connect) | The agent's address in its report | n/a (PrusaLink's own) |
| `printer_type` | `M862.2 Q` | `M862.2 Q` | same |
| `firmware_version` | `PRUSA Fir` | `M115` | same in effect |
| `nozzle_diameter` | `M862.1 Q` | `M862.1 Q` | fixed (tried) |
| `serial_number` | `PRUSA SN` | `PRUSA SN` | fixed (tried) |
| `sheet_settings` | `D3 Ax0D49 C88` | read every 10 s | fixed (tried) |
| `active_sheet` | `D3 Ax0DA1 C1` | read every 10 s | fixed (tried) |
| `mmu_connected` | Whether `M707` answered | Whether `M707` answered | fixed |
| `mmu_version` | `M707` | `M707` | fixed |
| `job_id` | Its job id from the EEPROM, to know a print after PrusaLink restarts | — | n/a: our agent keeps its job (`recovery.rs`) |
| `print_mode` | `D3 Ax0FFF C1`: 0 normal, 1 silent, **2 auto** | 0 and 1 only | **OPEN**: 2 = auto |
| `mbl` | `M420`, again after levelling; on the MK2.5 also on every change to printing or idle (it says nothing when levelling) | After levelling was seen | **OPEN**: the MK2.5 rule |
| `flash_air` | `D3 Ax0FBB`, kept; its only use is commented out (`sd_card.py:289`) | — | n/a: no effect in PrusaLink |
| `speed_multiplier` | `M220` every second | every 2 s idle, 5 s printing | fixed (tried) |
| `flow_multiplier` | `M221` every second | the same | fixed (tried) |
| `print_progress` | The printer's `M73` percent | SD: `M73`; host: the file's `M73` | fixed |
| `time_remaining` | The printer's `M73` time left | the same | fixed |
| `filament_change_in` | "Change in mins" of `M73`, negative is none | Read, not shown | **OPEN**: report and show it |
| `progress_broken`, `time_broken` | The printer's `M73` is negative or missing: its own estimate used instead | A negative `M73` is no value; our estimate stands | same (**OPEN**: a test for it) |
| `inaccurate_estimates` | Says the estimate is its own, not the slicer's | — | **OPEN**: say on the page when the time is an estimate |
| `progress_from_bytes`, `byte_position`, `sd_seconds_printing`, `time_remaining_guesstimate`, `mixed_path` | An SD print's progress from `M27`'s bytes and time, and its file's path | From `M27`'s bytes and `h:m`; the long name | same in effect |
| `print_state` | `M27` every second | `M27` every report | same |
| `total_filament` | `D3 Ax0FF1 C4`, metres, read again after a print | — | **OPEN** |
| `total_print_time` | `D3 Ax0FED C4`, minutes, read again after a print | — | **OPEN** |

### EEPROM (`EEPROMParams`)

| Item | Address | Ours | Status |
|---|---|---|---|
| `SHEET_SETTINGS` | 0x0D49, 88 bytes | read | fixed (tried) |
| `ACTIVE_SHEET` | 0x0DA1, 1 | read | fixed (tried) |
| `PRINT_MODE` | 0x0FFF, 1 | read | fixed; **OPEN**: auto |
| `TOTAL_FILAMENT` | 0x0FF1, 4 (metres, firmware `EEPROM_FILAMENTUSED`) | — | **OPEN** |
| `TOTAL_PRINT_TIME` | 0x0FED, 4 (minutes, `EEPROM_TOTALTIME`) | — | **OPEN** |
| `JOB_ID` | 0x0D05, 4 | — | n/a (see `job_id`) |
| `FLASH_AIR` | 0x0FBB, 1 | — | n/a (see `flash_air`) |
| `EEPROM_FILE_POSITION` | 0x0F91, 4: the line the printer saved at a power cut, read to resume | The line from its `Resend: N+1` after `M602` (`recovery.rs`) | differs: another route to the same number; tried only against the fake printer, **OPEN**: try on the MK3S, and read 0x0F91 too as a check |

### G-codes named singly

| Item | PrusaLink | Ours | Status |
|---|---|---|---|
| `D3` | EEPROM reads (and writes, see §1) | reads | fixed |
| `G80` | Not sent: in the pattern `echo:enqueing "G80"` | read | same |
| `M0` | "Insert the filament" when loading | the same | fixed |
| `M1` | "FW COMM ERR. Aborted" | the same | fixed |
| `M117` | Screen messages | "Cannot re-print" | fixed (see §1) |
| `M155` | `M155 S2 C7` always; `S0 C0` when it stops | the same for the MK3 | fixed (tried) |
| `M25` | Not sent: recognised in a file being printed (a pause) | `M601` recognised | same in effect |
| `M300` | Beeps | load, Cannot re-print, comm error | fixed (see §1) |
| `M707` | MMU version | the same | fixed |
| `M79` | Keep-alive | the same | same |
| `M862.1` | `M862.1 Q` | the same | fixed |
| `M862.2` | `M862.2 Q` | the same | same |

### Commands PrusaLink takes (`command_handlers.py`)

| Item | What it does | Ours | Status |
|---|---|---|---|
| `execute_gcode` | Runs G-code, refused while printing unless forced | The hub gives a busy machine no command | same |
| `load_filament`, `unload_filament` | §1 | the same for the MK3 family | fixed (test) |
| `set_ready` | `M72 S1` | on "Print taken" | fixed (test) |
| `cancel_ready` | `M72 S0`: the printer no longer "ready" | — | **OPEN**: the hub has no "not ready" yet; to be decided with the user |
| `job_info` | The job's file, progress and times for Connect | The agent's report | same |
| `pp_recovery` | Resume after a power cut (EEPROM line, its own copy of the file) | `recovery.rs` | differs (see `EEPROM_FILE_POSITION`) |
| `reset_printer` | Resets the printer: GPIO 22 when on the Pi's header, else a DTR blip on USB; waits for `start` | — | **OPEN** |
| `disable_resets`, `enable_resets` | `;C32u2_RMD/RME` | — | n/a (off by default in PrusaLink, §1) |
| `upgrade_link` | Updates PrusaLink itself | The agent updates itself from the hub (signed) | same in effect |

### Commands from Prusa Connect (`set_handler`)

| Item | Ours (the hub's action for an agent's printer) | Status |
|---|---|---|
| `GCODE` | Control's G-code | same |
| `START_PRINT` | Print from Files, the queue, the USB/SD tab | same |
| `STOP_PRINT`, `PAUSE_PRINT`, `RESUME_PRINT` | Job card buttons | same; **OPEN**: the 15 s check for SD prints, and for stop |
| `SEND_JOB_INFO` | The agent's report | same |
| `LOAD_FILAMENT`, `UNLOAD_FILAMENT` | Load / Unload | fixed |
| `SET_PRINTER_READY` | Print taken | fixed |
| `CANCEL_PRINTER_READY` | — | **OPEN** (see `cancel_ready`) |
| `RESET_PRINTER` | — | **OPEN** (see `reset_printer`) |
| `UPGRADE` | The agent's own update | same in effect |

### Web controls (`web/controls.py`): what they allow and refuse

| Item | PrusaLink | Ours | Status |
|---|---|---|---|
| `jog` | Absolute or relative; X 0–255, Y −4–212.5, Z 0.15–210; feed up to 2700 (X, Y), 1000 (Z) | 10 mm relative, no bounds checked (the firmware's own soft endstops) | **OPEN**: check the move against the known position and the bed |
| `api_printhead` | Z moves only idle, ready, finished, stopped; X/Y also paused; home only idle or ready | The same: while paused no homing and no Z (`refused_now`); while busy no command at all | **fixed** (test `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| `home` | `G28` with axes | `G28 W` (MK3 family) | fixed |
| `set_speed` | 10–999 % | 10–300 % | differs on purpose (stricter) |
| `extrude` | −10 to 100 mm, up to 100 mm/min; refused under 170 °C and while printing | 10 mm at 100 mm/min; refused under 170 °C and while printing (`refused_now`) | **fixed** (test) |
| `api_tool` | Nozzle 0–305 °C; flow 10–999 % | Nozzle up to the profile's maximum: 295 °C for the MK3 family, as the printer's own menu allows (`HEATER_0_MAXTEMP - 10`; 305 °C is where it stops with MAXTEMP); flow 50–150 % | differs on purpose (the printer's own limit) |
| `api_bed` | Bed 0–125 °C | Up to the profile's maximum: 120 °C for the MK3 family, as its own menu (`BED_MAXTEMP - 5`; 125 °C stops it) | differs on purpose (the printer's own limit) |
| (commands during a print) | Speed, flow and temperatures may be set while printing; X/Y may move while paused (`api_printhead`, `api_tool`, `api_bed` have no print check for these) | The hub gives an agent's printer **no command at all** while its job runs | **OPEN**: hand these to the running job, between the file's lines |

## 4. Not covered by this audit

- **Buddy printers** (MINI, MK4, XL, CORE One): they do not use PrusaLink over serial; their
  own Connect protocol is in `prusa-connect-protocol.md`.
- **Plain Marlin and other firmware** (OctoPrint's ground): `marlin-serial-protocol.md`.
- **How PrusaLink talks to Prusa Connect** (its SDK side): in `prusa-connect-protocol.md`.
- **Timing and order inside PrusaLink's serial queue** (priorities, `to_front`): read for the
  protocol audit, not repeated here.
