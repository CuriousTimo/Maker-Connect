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
| `M155 S2 C7` | Temperatures, position and fans auto-reported **always**, and switched on again when any of the three is not heard for 60 s (`auto_telemetry.py`) | Always on for the MK3 from connecting, again after 60 s of silence (`start_reports`, `Screen::reports_by_itself`); kept on after a print | **fixed** (tried on the MK3S: fans and position while idle; tests `an_mk3_is_given_its_reports_and_asked_again_when_silent`, `an_mk3_keeps_its_reports_on_after_a_print`) |
| `M155 S0 C0` | Auto-report off when PrusaLink stops (`auto_telemetry.py`) | Not sent to an MK3 (kept on); other Marlin: off after a print | same for the MK3 (test `an_mk3_keeps_its_reports_on_after_a_print`) |
| `G28 X Y Z` | Home from the web controls, always with axes, so no mesh bed levelling (`web/controls.py`) | `G28 W` for the MK3 family (the same: home only), `G28` elsewhere | **fixed** (test `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| `G90`/`G91` + `G1 F… X Y Z` | Move the head (`web/controls.py`) | `G91`, `G1`, `G90` | same (test `the_move_buttons_send_g91_g1_g90`) |
| `M220 S` | Speed factor (`web/controls.py`) | `M220 S` | same (test `the_speed_goes_by_m220_s`) |
| `M221 S` | Flow factor (`web/controls.py`) | `M221 S` | same (test `the_flow_goes_by_m221_s`) |
| `M84` | Motors off (`web/controls.py`) | `M18` (the same in Marlin) | same (test `motors_go_off_with_m18`) |
| `M83` + `G1 F… E…` | Extrude or retract an amount (`web/controls.py`) | Control: Extrude / Retract 10 mm, `M83` + `G1 F100 E±10` (PrusaLink's E feed) | **fixed** (test) |
| `M104 S` / `M140 S` | Set nozzle / bed (`web/controls.py`) | `M104` / `M140` | same (test `typed_temperatures_stay_within_the_machine`) |
| `M140`, `M109`, `M104` | Preheat for load: bed; wait for 90 % of the temperature (rounded to 5) unless that hot already; then the full temperature (`command_handlers.py`) | The same for the MK3 family (bed from the loaded filament where it says one) | **fixed** (test) |
| `M300 P500 S1` + `M0 Insert the filament` + `M701` | Load filament: a beep and a prompt on the printer, then load (`command_handlers.py`) | The same for the MK3 family | **fixed** (test) |
| `M702` | Unload (`command_handlers.py`) | `M702` | same (test `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| `M601` / `M602` / `M603` | Pause / resume / stop: sent **once**, then waited for the state up to 15 s (`STATE_CHANGE_TIMEOUT`); the command fails if not reached (`command_handlers.py` `_try_until_state`). *(Corrected: an earlier version of this row said "sent again".)* | Sent once; not paused / not resumed within 15 s is said as the job's problem | **fixed** (tests `an_mk3_parks_itself`, `an_mk3_that_does_not_pause_in_15_s_is_the_jobs_problem`, `an_mk3_paused_from_here_is_resumed_from_here`). The resume check was unreachable (found by the proof, 2026-11-09: resuming from here waited for the printer to stop saying paused, which it does only after M602, so an MK3 paused from the hub never resumed); fixed. "Did not resume within 15 s" has no test yet |
| `M75` / `M76` / `M77` | Printer's print timer start / pause / stop (`file_printer.py`) | `M75` / `M76` / `M77` | same (test `the_mk3_print_timer_goes_m75_m76_m75_m77`) |
| `M73 P R Q S C D` | Progress and time left sent to the printer during a host print, only for a file without its own `M73`: every 100 lines and 10 before the end (`file_printer.py`, `util.py`) | The same (`print.rs`, `STATS_EVERY`, `STATS_TAIL`) | **fixed** (tests `a_file_without_m73_is_given_it`, `a_file_with_m73_keeps_its_own`) |
| `M72 S1` / `M72 S0` | Printer "ready" state on (Connect's Set ready) and off (Cancel ready) (`command_handlers.py`) | `M72 S1` when "Print taken" is pressed for an MK3 through an agent; there is no "cancel ready" in the hub, so no `M72 S0` | **fixed** for S1 (test); S0 n/a |
| `M28 …/setready.g`, `M84`, `M29` | Writes a "Set ready" file into a folder "PrusaLink menu" on the SD card, only when the user made that folder and it has none (`filesystem/sd_card.py` `handle_special_menu`) | The same, on connecting and on every listing (`special_menu`); choosing it on the printer is Set Ready (`setready.g` opened, started, done within 1 s each); the folder kept out of the SD tab | **fixed** (tests `prusalinks_menu_gets_its_setready_file`, `prusalinks_menu_on_the_card_sets_it_ready`, `the_long_names_of_an_m20_lt_listing_are_read`) |
| `M23 path` + `M24` | Print from the SD card: the path in lower case, `M24` only on "File opened", stopped on "open failed" (`command_handlers.py` `_load_file`) | `M23` with the path in lower case on the MK3 family (`sd_card.rs`), `M24` only after `File opened` (`carry_out`) | **fixed** (test `a_card_file_is_printed_only_once_opened`) |
| `M20 LT` | List the SD card with long names and times (`filesystem/sd_card.py`) | `M20 L T` | same (test `an_agents_printer_and_its_sd_card`) |
| `M21` | Initialise the SD card (`filesystem/sd_card.py`) | Sent when connecting to an MK3 | **fixed** (tried: SD card "in"; tests `the_mk3_is_asked_what_it_is_when_connected`, `plain_marlin_is_not_asked_prusas_questions`) |
| `M27 P` | SD print state with the file's short path (`printer_polling.py`) | `M27 P` on the MK3 family; the long name from the card's listing (`M20`: `M20 LT`), as `process_mixed_path` | **fixed** (test `a_print_started_at_the_printer_is_seen_at_once`, `an_mk3_sd_print_is_followed_with_m27_and_m73`) |
| `M79 S"PL"` | Host keep-alive (`keepalive.py`): `S"PL"`, or `S"PC"` working for Prusa Connect, every 12 s | `M79 S"PC"` every 12 s (the agent works for its hub, as PrusaLink for Connect); was `S"MC"` | **fixed** (test `the_host_says_it_is_there_every_12_s`) |
| `M552 P<ip>` | Tell the printer its IP address, shown in its menu (`ip_updater.py`) | The agent's address, sent when connecting to an MK3 | **fixed** (to look at on the printer) (tests `the_mk3_is_asked_what_it_is_when_connected`, `plain_marlin_is_not_asked_prusas_questions`); seen on the MK3S 2026-11-09: its menu shows the agent's address 192.168.60.65 |
| `M117 ~text` | Messages on the printer's screen ("Cannot re-print", "Recovery failed"), and PrusaLink's own lines (`lcd_printer.py`, see §7) | "Cannot re-print"; "Recovery failed" with `M603` when a power cut cannot be taken up; PrusaLink's own lines (`crate::lcd`) | **fixed** (tests `an_mk3_through_an_agent_is_handled_as_prusalink_does`, `a_power_cut_that_cannot_be_taken_up_is_given_up`, `it_starts_and_says_it_is_ok_when_idle`, `a_print_shows_its_file_and_a_power_cut_comes_first`, `an_error_is_shown_after_its_grace_with_its_chime`, `the_firmwares_own_message_is_left_for_10_s`, `a_transfer_bar_is_drawn_as_prusalink_draws_it`, `an_mk3_is_sent_only_what_prusalink_sends`) |
| `M300 …` | Chimes: welcome, error, upload (`lcd_printer.py`); the load beep; Cannot re-print; comm error | All of them (the chimes with their screens, `crate::lcd`) | **fixed** (tests `it_starts_and_says_it_is_ok_when_idle`, `a_print_shows_its_file_and_a_power_cut_comes_first`, `an_error_is_shown_after_its_grace_with_its_chime`, `the_firmwares_own_message_is_left_for_10_s`, `a_transfer_bar_is_drawn_as_prusalink_draws_it`, `an_mk3_is_sent_only_what_prusalink_sends`, `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| `M110 N0` | Line numbers back to 0 (`serial/serial_queue.py`) | `M110` | same (test `connecting_sets_the_line_numbers_back_with_m110_n0`) |
| `M400` | Workaround slot around `M110` (`serial/serial_queue.py`) | `M400` (unnumbered) before our `M110 N0` when connecting, our only `M110` | **fixed** (test `connecting_sets_the_line_numbers_back_with_m110_n0`; was WRONG: no `M400` was sent) |
| `M300 S880 P200` + `M603` + `M1 FW COMM ERR. Aborted` | On a communication failure mid-print: beep, stop, message (`serial/serial_queue.py`) | The same for an MK3, as far as it still listens | **fixed** (test `a_print_lost_on_the_wire_beeps_stops_and_says_so_on_an_mk3`) |
| `;C32u2_RMD` / `;C32u2_RME` | Turn the MK3's USB chip's reset off and on, around SD prints (`serial/serial_adapter.py`) | — | n/a: **off by default in PrusaLink** (`reset_disabling = False`, its ini warns it wears out the chip's EEPROM); not needed on the GPIO serial |
| `PRUSA Fir` | Firmware version (`printer_polling.py`) | `PRUSA Fir`, in PrusaLink's order after `M862.2 Q`; no `M115` to the MK3 family | **fixed** (test `the_mk3_is_asked_what_it_is_when_connected`) |
| `PRUSA SN` | Serial number (`printer_polling.py`) | Asked when connecting to an MK3 | **fixed** (tried: CZPX2918X004XK74799; tests `the_mk3_is_asked_what_it_is_when_connected`, `plain_marlin_is_not_asked_prusas_questions`) |
| `M862.2 Q` | Printer type (`printer_polling.py`) | `M862.2 Q` | same (tests `the_mk3_is_asked_what_it_is_when_connected`, `plain_marlin_is_not_asked_prusas_questions`) |
| `M862.1 Q` | Nozzle diameter (`printer_polling.py`) | Asked when connecting to an MK3 | **fixed** (tried: 0.40; tests `the_mk3_is_asked_what_it_is_when_connected`, `plain_marlin_is_not_asked_prusas_questions`) |
| `M707 A0x00…0x03` | MMU firmware version (`printer_polling.py`) | Asked when connecting to an MK3 | **fixed** (test `the_mk3_is_asked_what_it_is_when_connected`; tried without an MMU: none) |
| `M420` | Read the bed mesh, again after levelling (`printer_polling.py`, `invalidate_mbl`) | `M420` after levelling was seen (MK3: same output as `G81`, `gcode_G81_M420`); the hub's Probe button measures on request | **fixed** (tests `an_mk3_that_levelled_has_its_mesh_read_with_m420`, `the_mesh_of_marlin_and_an_mk25_is_read_when_due`) |
| `M220` / `M221` (no S) | Speed and flow read back every second (`printer_polling.py`) | Every report (2 s) idle, every 5 s while printing | **fixed** (tried: 100 % / 100 %; test `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`) |
| `M73` (no args) | The printer's own progress and time left, from the slicer's `M73` in the file (`printer_polling.py`) | SD prints: asked, in the printer's mode; host prints: the file's `M73` as it goes by | **fixed** (test `an_mk3_sd_print_is_followed_with_m27_and_m73`) |
| `D3 Ax…` (read) | EEPROM: steel sheets and the active one, job id, FlashAir, print mode silent/normal (`printer_polling.py`) | Sheets, the active one and the print mode every 10 s (addresses checked against firmware 3.14 `eeprom.h`) | **fixed** for sheet and mode (tried: BH-25, normal); job id and FlashAir n/a (below) |
| `D3 Ax0D05 X…` (write) | Store the job id in the printer's EEPROM (`job.py`) | — | n/a: our job and its place are kept by the agent (`recovery.rs`) |
| `D3 Ax0d15/0d25` (write) | Write a serial number into the printer (`web/lib/wizard.py`) | — | n/a (factory wizard) |
| custom G-code | Each line run, checked against "Unknown command" and "cold extrusion prevented" (`command_handlers.py`) | Same checks | same (test `a_rejected_code_is_told_and_the_job_goes_on`) |

## 2. What the printer says, and whether we read it

| PrusaLink pattern | Printer message | Ours | Status |
|---|---|---|---|
| `TEMPERATURE_REGEX` | `T:… /… B:… /… T0:… @:… B@:… P:… A:…` | `marlin::heat` | same (test `a_temperature_report_is_read`) |
| `POSITION_REGEX` | `X:… Y:… Z:… E:… Count X:…` | Treated as noise until now | **missing → fixed, uncommitted** (test `a_position_report_is_read`) |
| `FAN_REGEX` | `E0:… RPM PRN1:… RPM E0@:… PRN1@:…` | `marlin::heat` | same (only heard while printing, see §1; test `a_fan_report_is_read`) |
| `HEATING_REGEX`, `HEATING_HOTEND_REGEX` | `T:… E:… B:…`, `T:… E:… W:…` while `M109`/`M190` wait | Read | same (test `the_heating_lines_while_waiting_are_read`) |
| `CONFIRMATION_REGEX` | `ok…`, `Done saving file.` | `ok` (after `M28`…`M29` too, which the agent sends now) | **fixed** (tests `only_an_ok_confirms_a_line`, `prusalinks_menu_gets_its_setready_file`) |
| `RESEND_REGEX` | `Resend: N` | Read | same (test `a_resend_request_is_read_and_followed`) |
| `REJECTION_REGEX` | `echo:Unknown command: "…"`, `Unknown … code`, `echo: cold extrusion prevented` | Read | same (test `a_rejected_code_is_told_and_the_job_goes_on`) |
| `BUSY_REGEX` | `echo:busy: processing` | Read | same (test `echo_busy_processing_is_waited_for`) |
| `ATTENTION_REGEX` | `echo:busy: paused for user` | Read | same (test `echo_busy_paused_for_user_is_noticed`) |
| `PAUSE_PRINT_REGEX` … `NOT_READY_REGEX` | `// action:pause`, `paused`, `resume`, `resumed`, `cancel`, `ready`, `not_ready` | Read | same (test `the_host_actions_of_the_printers_menu_are_followed`) |
| `REPRINT_REGEX` | `// action:start` (Reprint from the printer's menu): the last job again, else a beep and "Cannot re-print" | The same, through the hub, with its checks (idle, bed clear, file suits) | **fixed** (test) |
| `PP_RECOVER_REGEX`, `PP_AUTO_RECOVER_REGEX` | `// action:uvlo_recovery_ready`, `uvlo_auto_recovery_ready` | Read | same (test `a_print_brought_back_after_a_power_cut_is_noticed`) |
| `ERROR_REGEX` | `Error:Printer halted. kill() called!`, `Printer stopped due to errors…` | Read | same (test `a_halted_or_stopped_printer_is_a_fault`) |
| `ERROR_REASON_REGEX` | MINTEMP / MAXTEMP (bed), thermal runaway, … | Read | same (check every reason against the pattern; test `every_reason_the_printer_stops_for_is_a_fault`) |
| `ATTENTION_REASON_REGEX` | Bed levelling failed (sensor too high, didn't trigger, …) | Read | same (test `a_failed_bed_levelling_needs_someone`) |
| `FAN_ERROR_REGEX` | `Extruder/Hotend/Print fan speed is lower than expected` | Read | same (test `a_slow_fan_needs_someone`) |
| `TM_ERROR_LOG_REGEX`, `TM_ERROR_CLEARED` | Thermal model error and cleared | Read | same (test `the_thermal_model_error_comes_and_goes`) |
| `TM_CAL_START_REGEX`, `TM_CAL_END_REGEX` | Thermal model calibration: PrusaLink sends **nothing** meanwhile (it blocks its queue) | The agent only listens until it ends; reported busy, "Calibrating its thermal model" | **fixed** (test) |
| `MBL_TRIGGER_REGEX` | `tmc…_home_enter(…)`, `echo:enqueing "G80"`: the mesh is read again later (`invalidate_mbl`) | The same | **fixed** (test) |
| `MBL_REGEX` | `M420` mesh output | `marlin::mesh` (the same format as `G81`) | same (test `the_answer_to_m420_is_read`) |
| `OPEN_RESULT_REGEX` | `File opened: … Size: …` / `open failed` | Both | **fixed** (test `a_file_opened_or_not_is_read`) |
| `FILE_OPEN_REGEX`, `START_PRINT_REGEX` | `echo:enqueing "M23 …"`, `echo:enqueing "M24"` (print started from the printer's menu): PRINTING at once (`special_commands.py` `handle_start`) | `echo:enqueing "M24"`: printing at once, its file from `File opened`, until `M27 P` says so | **fixed** (test `a_print_started_at_the_printer_is_seen_at_once`) |
| `PRINT_DONE_REGEX` | `Done printing file`: FINISHED at once (`handle_done`) | Done at once, reported whole | **fixed** (test `the_end_of_a_print_from_the_card_is_seen_at_once`) |
| `M27_OUTPUT_REGEX` | path, `Not SD printing`, `Print saved`, `SD print paused`, `SD printing byte a/b` | Read | same (tests `an_sd_print_is_followed`, `every_answer_to_m27_is_read`) |
| `PRINT_INFO_REGEX` | `NORMAL MODE: Percent done: …; print time remaining in mins: …; Change in mins: …` | `marlin::print_info`, in the printer's mode | **fixed** (test) |
| `SD_PRESENT_REGEX`, `SD_EJECTED_REGEX` | `echo:SD card ok`, `SD init fail`, `SD card released` | Read; "SD card: in / none" on the machine page | **fixed** (test; tried: in) |
| `LFN_CAPTURE` | `M20 LT` listing | Read (hub) | same (test `the_long_names_of_an_m20_lt_listing_are_read`) |
| `PRINTER_BOOT_REGEX` | `start` (the printer restarted) | Read | same (test `a_restarted_printer_is_noticed`) |
| `POWER_PANIC_REGEX` | `INT4` | Read | same (test `a_power_failure_is_noticed`) |
| `LCD_UPDATE_REGEX` | `LCD status changed`: the firmware's own message, left 10 s; its own lines not counted (`lcd_updated`) | The same (`Screen::take_lcd_changed`, `Lcd::lcd_updated`) | **fixed** (tests `lcd_status_changed_is_counted_for_the_screen`, `the_firmwares_own_message_is_left_for_10_s`) |
| `M110_REGEX` | `M110 N…` echoed by the printer | Not needed: our `M110` is answered by its `ok`, and an echo is a plain line we skip | n/a |
| `D3_OUTPUT_REGEX` | EEPROM read output | `marlin::eeprom_bytes` | **fixed** (test `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`; tried) |
| `MMU_MAJOR/MINOR/REVISION/BUILD_REGEX` | `echo:MMU2:<R0 A…` … | `marlin::mmu_firmware` | **fixed** (test `the_mk3_is_asked_what_it_is_when_connected`) |
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
| `CANCEL_REGEX` | `// action:cancel`: stopped from the printer's menu | `Action::Cancel`: the job ends, "stopped at the printer" | same (test `a_print_stopped_from_the_printers_menu_ends`) |
| `PAUSED_REGEX` | `// action:paused`: it paused itself | `Action::Paused` | same (test `it_pausing_itself_is_noticed`) |
| `RESUME_PRINT_REGEX` | `// action:resume`: resume from its knob | `Action::Resume` | same (test `resuming_from_its_knob_is_noticed`) |
| `RESUMED_REGEX` | `// action:resumed` | `Action::Resume` | same (test `it_resuming_is_noticed`) |
| `READY_REGEX` | `// action:ready`: Set Ready in its menu; PrusaLink answers it with its `SetReady` (`M72 S1`) | `Action::Ready` → the hub's "Print taken", and `M72 S1` back | **fixed** (tests `set_ready_at_the_printer_is_reported_once`, `a_queue_waits_for_a_clear_bed`, `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| `MMU_MAJOR_REGEX`, `MMU_MINOR_REGEX`, `MMU_REVISION_REGEX`, `MMU_BUILD_REGEX` | Answers to `M707 A0x00`…`A0x03` | `marlin::mmu_firmware` | fixed (test `the_mk3_is_asked_what_it_is_when_connected`) |

### Polled values (`WatchedItem`s of printer_polling.py)

| Item | What PrusaLink does | Ours | Status |
|---|---|---|---|
| `network_info` | PrusaLink's own network (its web, Connect) | The agent's address in its report | n/a (PrusaLink's own) |
| `printer_type` | `M862.2 Q` | `M862.2 Q` | same (test `the_mk3_is_asked_what_it_is_when_connected`) |
| `firmware_version` | `PRUSA Fir` | `PRUSA Fir` | **fixed** (test `the_mk3_is_asked_what_it_is_when_connected`) |
| `nozzle_diameter` | `M862.1 Q` | `M862.1 Q` | fixed (tried; test `the_mk3_is_asked_what_it_is_when_connected`) |
| `serial_number` | `PRUSA SN` | `PRUSA SN` | fixed (tried; test `the_mk3_is_asked_what_it_is_when_connected`) |
| `sheet_settings` | `D3 Ax0D49 C88` | read every 10 s | fixed (tried; test `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`) |
| `active_sheet` | `D3 Ax0DA1 C1` | read every 10 s | fixed (tried; test `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`) |
| `mmu_connected` | Whether `M707` answered | Whether `M707` answered | fixed (test `the_mk3_is_asked_what_it_is_when_connected`) |
| `mmu_version` | `M707` | `M707` | fixed (test `the_mk3_is_asked_what_it_is_when_connected`) |
| `job_id` | Its job id from the EEPROM, to know a print after PrusaLink restarts | — | n/a: our agent keeps its job (`recovery.rs`) |
| `print_mode` | `D3 Ax0FFF C1`: 0 normal, 1 silent, 2 auto | 0, 1 and 2 (`marlin::print_mode`) | **fixed** (test `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`; tried: normal) |
| `mbl` | `M420`, again after levelling; on the MK2.5 also on every change to printing or idle (it says nothing when levelling) | The same (`is_mk25`, `Screen::mesh_changed`) | **fixed** (tests `an_mk3_that_levelled_has_its_mesh_read_with_m420`, `the_mesh_of_marlin_and_an_mk25_is_read_when_due`; the MK2.5 rule against a fake MK2.5 only, none tried) |
| `flash_air` | `D3 Ax0FBB`, kept; its only use is commented out (`sd_card.py:289`) | — | n/a: no effect in PrusaLink |
| `speed_multiplier` | `M220` every second | every 2 s idle, 5 s printing | fixed (tried; test `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`) |
| `flow_multiplier` | `M221` every second | the same | fixed (tried; test `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`) |
| `print_progress` | The printer's `M73` percent | SD: `M73`; host: the file's `M73` | fixed (test `an_mk3_sd_print_is_followed_with_m27_and_m73`) |
| `time_remaining` | The printer's `M73` time left | the same | fixed (test `an_mk3_sd_print_is_followed_with_m27_and_m73`) |
| `filament_change_in` | "Change in mins" of `M73`, negative is none | From the file's `M73` (C/D) and the printer's `M73`; "filament change in …" on the job | **fixed** (tests `m73_is_read`, `what_prusalink_reads_is_read`) |
| `progress_broken`, `time_broken` | The printer's `M73` is negative or missing: its own estimate used instead | A negative `M73` is no value; our estimate stands | same (test in `what_prusalink_reads_is_read`) |
| `inaccurate_estimates` | Says the estimate is its own, not the slicer's | `time_estimated`: "(estimate)" on the page | **fixed**; no test of the flag yet |
| `progress_from_bytes`, `byte_position`, `sd_seconds_printing`, `time_remaining_guesstimate`, `mixed_path` | An SD print's progress: its own `M73` percent, else the whole percent of `M27`'s bytes; time left its own, else printed × (100 − p) / p; its file's long name from the listing | The same (`sd_progress`; long name in the hub) | **fixed** (test `an_sd_print_goes_as_far_as_prusalink_says`, `an_sd_print_is_followed`) |
| `print_state` | `M27` every second | `M27` every report | same (test `an_mk3_sd_print_is_followed_with_m27_and_m73`) |
| `total_filament` | `D3 Ax0FF1 C4`, metres, read again after a print | The same, after host and SD prints | **fixed** (parser test; tests `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`, `an_mk3_sd_print_is_followed_with_m27_and_m73`); not yet seen on the MK3S |
| `total_print_time` | `D3 Ax0FED C4`, minutes, read again after a print | The same | **fixed** (parser test; tests `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`, `an_mk3_sd_print_is_followed_with_m27_and_m73`); not yet seen on the MK3S |

### EEPROM (`EEPROMParams`)

| Item | Address | Ours | Status |
|---|---|---|---|
| `SHEET_SETTINGS` | 0x0D49, 88 bytes | read | fixed (tried; test `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`) |
| `ACTIVE_SHEET` | 0x0DA1, 1 | read | fixed (tried; test `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`) |
| `PRINT_MODE` | 0x0FFF, 1 | read, 0/1/2 | **fixed** (test `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`) |
| `TOTAL_FILAMENT` | 0x0FF1, 4 (metres, firmware `EEPROM_FILAMENTUSED`) | read | **fixed** (see total_filament; tests `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`, `an_unwritten_mk3_eeprom_gives_no_totals`) |
| `TOTAL_PRINT_TIME` | 0x0FED, 4 (minutes, `EEPROM_TOTALTIME`) | read | **fixed** (see total_print_time; test `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`) |
| `JOB_ID` | 0x0D05, 4 | — | n/a (see `job_id`) |
| `FLASH_AIR` | 0x0FBB, 1 | — | n/a (see `flash_air`) |
| `EEPROM_FILE_POSITION` | 0x0F91, 4: the line the printer saved at a power cut, read to resume | `D3 Ax0F91 C4` read once while it waits after the cut; the job goes on from the line after it (the firmware sets `M110 N<saved>` itself and asks `Resend: N+1`, `power_panic.cpp`), the Resend only says when | **fixed** (test `after_a_power_cut_the_saved_line_is_read_from_its_eeprom`); a real power cut on the MK3S still to try once printing is possible |

### G-codes named singly

| Item | PrusaLink | Ours | Status |
|---|---|---|---|
| `D3` | EEPROM reads (and writes, see §1) | reads | fixed (test `an_idle_mk3_is_asked_its_speed_flow_totals_and_sheet`) |
| `G80` | Not sent: in the pattern `echo:enqueing "G80"` | read | same (test `what_it_says_by_itself_is_followed`) |
| `M0` | "Insert the filament" when loading | the same | fixed (test `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| `M1` | "FW COMM ERR. Aborted" | the same | fixed (test `a_print_lost_on_the_wire_beeps_stops_and_says_so_on_an_mk3`) |
| `M117` | Screen messages | All of PrusaLink's (§1, §7) | **fixed** (tests `an_mk3_through_an_agent_is_handled_as_prusalink_does`, `a_power_cut_that_cannot_be_taken_up_is_given_up`) |
| `M155` | `M155 S2 C7` always; `S0 C0` when it stops | the same for the MK3 | fixed (tried; tests `an_mk3_is_given_its_reports_and_asked_again_when_silent`, `an_mk3_keeps_its_reports_on_after_a_print`) |
| `M25` | Not sent: recognised in a file being printed (a pause) | `M601` and `M25` recognised: nothing more is sent until it says it resumed | **fixed** (test `an_mk3_pause_by_m25_in_the_file_is_waited_for`; was WRONG: only `M601` was recognised, so a file pausing with `M25` went on being sent) |
| `M300` | Beeps | All of PrusaLink's (§1, §7) | **fixed** (tests `an_mk3_through_an_agent_is_handled_as_prusalink_does`, `a_print_lost_on_the_wire_beeps_stops_and_says_so_on_an_mk3`, `an_error_is_shown_after_its_grace_with_its_chime`) |
| `M707` | MMU version | the same | fixed (test `the_mk3_is_asked_what_it_is_when_connected`) |
| `M79` | Keep-alive | `M79 S"PC"` every 12 s | **fixed** (test `the_host_says_it_is_there_every_12_s`) |
| `M862.1` | `M862.1 Q` | the same | fixed (test `the_mk3_is_asked_what_it_is_when_connected`) |
| `M862.2` | `M862.2 Q` | the same | same (test `the_mk3_is_asked_what_it_is_when_connected`) |

### Commands PrusaLink takes (`command_handlers.py`)

| Item | What it does | Ours | Status |
|---|---|---|---|
| `execute_gcode` | Runs G-code, refused while printing unless forced | Typed G-code refused while it prints (no force) | **fixed** (tests `an_mk3_through_an_agent_is_handled_as_prusalink_does`, `typed_gcode_is_checked`) |
| `load_filament`, `unload_filament` | §1 | the same for the MK3 family | fixed (test) |
| `set_ready` | `M72 S1` (`M72`: ready, or not) | on "Print taken", and on Set Ready at its menu | **fixed** (test `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| `cancel_ready` | `M72 S0`, whether ready or not; READY back to IDLE | "Not ready after all" on its page (and `// action:not_ready` at its menu): `M72 S0`; the last print counts as on its bed again, the next job waits | **fixed** (tests `an_mk3_through_an_agent_is_handled_as_prusalink_does`, `set_ready_at_the_printer_is_reported_once`) |
| `job_info` | The job's file, progress and times for Connect | The agent's report | same; test `an_agents_printer_and_its_sd_card` |
| `pp_recovery` | Resume after a power cut: the line from its EEPROM (0x0F91), the file from its own copy | The line from 0x0F91 (`EEPROM_FILE_POSITION`), the file from the agent's own copy (`recovery.rs`) | **fixed** (test `after_a_power_cut_the_saved_line_is_read_from_its_eeprom`) |
| `reset_printer` | Resets the printer: GPIO 22 when on the Pi's header, else a DTR blip on USB; waits for `start` | — | **OPEN** |
| `disable_resets`, `enable_resets` | `;C32u2_RMD/RME` | — | n/a (off by default in PrusaLink, §1) |
| `upgrade_link` | Updates PrusaLink itself | The agent updates itself from the hub (signed) | n/a: the host's own update, nothing goes to the printer (tests `only_a_signed_file_on_the_list_is_taken`, `a_build_is_tried_put_in_place_and_can_go_back`, `an_update_waits_only_for_work`) |

### Commands from Prusa Connect (`set_handler`)

| Item | Ours (the hub's action for an agent's printer) | Status |
|---|---|---|
| `GCODE` | Control's G-code | same; test `an_mk3_through_an_agent_is_handled_as_prusalink_does`, `typed_gcode_is_checked` |
| `START_PRINT` | Print from Files, the queue, the USB/SD tab | same; test `an_agent_runs_a_job`, `an_agents_printer_and_its_sd_card` |
| `STOP_PRINT`, `PAUSE_PRINT`, `RESUME_PRINT` | Job card buttons. SD prints: pause, resume and stop checked against the next `M27`, not done within 15 s is said (`Printer::sd_not_done`). Host prints: pause and resume checked the same way; a stop is done by the agent itself (it stops sending; `M603` then parks the head), so there is nothing to wait for | **fixed** (test `an_sd_print_does_what_was_asked_in_time`) |
| `SEND_JOB_INFO` | The agent's report | same; test `an_agents_printer_and_its_sd_card` |
| `LOAD_FILAMENT`, `UNLOAD_FILAMENT` | Load / Unload | fixed; test `an_mk3_through_an_agent_is_handled_as_prusalink_does` |
| `SET_PRINTER_READY` | Print taken | fixed; test `an_mk3_through_an_agent_is_handled_as_prusalink_does` |
| `CANCEL_PRINTER_READY` | "Not ready after all" | **fixed** (test `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| `RESET_PRINTER` | — | **OPEN** (see `reset_printer`) |
| `UPGRADE` | The agent's own update | n/a: the host's own update, nothing goes to the printer (tests `only_a_signed_file_on_the_list_is_taken`, `a_build_is_tried_put_in_place_and_can_go_back`, `an_update_waits_only_for_work`) |

### Web controls (`web/controls.py`): what they allow and refuse

| Item | PrusaLink | Ours | Status |
|---|---|---|---|
| `jog` | `G91`, then `G1 F<feed> <axes>`; feed 0–2700 (2700 when none given), refused outside; positions bounded only for absolute moves (X 0–255, Y −4–212.5, Z 0.15–210) | The MK3 family: `G91`, `G1 F<feed> <axes>`, feed 0–2700 refused outside, 2700 for the ±10 buttons; relative moves only (none absolute, so no position bounds, as PrusaLink) | **fixed** (test `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| `api_printhead` | Z moves only idle, ready, finished, stopped; X/Y also paused; home only idle or ready | The MK3 family: home and Z only idle, X/Y idle or paused (`refused_now`) | **fixed** (test `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| `home` | `G28` with axes, X Y Z when none given (with axes the firmware does not level, `Marlin_main.cpp` G28) | `G28 X Y Z` (MK3 family) | **fixed** (test `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| `set_speed` | `M220 S<n>`, 10–999 % | `M220 S<n>`, 10–999 % (MK3 family) | **fixed** (test `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| `extrude` | −10 to 100 mm, up to 100 mm/min; refused under 170 °C and while printing | 10 mm at 100 mm/min; refused under 170 °C and while printing (`refused_now`) | **fixed** (test) |
| `api_tool` | Nozzle `M104 S<n>` 0–305 °C; flow `M221 S<n>` 10–999 % | The same (MK3 family) | **fixed** (test `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| `api_bed` | Bed `M140 S<n>` 0–125 °C | The same (MK3 family) | **fixed** (test `an_mk3_through_an_agent_is_handled_as_prusalink_does`) |
| (commands during a print) | Speed, flow and temperatures may be set while printing; X/Y may move while paused (`api_printhead`, `api_tool`, `api_bed` have no print check for these) | The same: the hub hands a printing agent only these (`fits_a_print`), the job sends them between the file's lines (`PrintJob::inject`) | **fixed** (tests `an_mk3_through_an_agent_is_handled_as_prusalink_does`, `g_code_while_printing_goes_between_the_files_lines`) |

## 4. Not covered by this audit

- **Buddy printers** (MINI, MK4, XL, CORE One): they do not use PrusaLink over serial; their
  own Connect protocol is in `prusa-connect-protocol.md`.
- **Plain Marlin and other firmware** (OctoPrint's ground): `marlin-serial-protocol.md`.
- **How PrusaLink talks to Prusa Connect** (its SDK side): in `prusa-connect-protocol.md`.
- **Timing and order inside PrusaLink's serial queue** (priorities, `to_front`): read for the
  protocol audit, not repeated here.

## 6. What we send that PrusaLink does not

Every G-code in the agent's and hub's code (test code left out) that is not one of PrusaLink's
forms, mechanically listed by `scripts/prusalink-audit-check.py`, with how it is kept from
Prusa's MK3 family. The printer must not tell the agent from PrusaLink: an MK3 session is run
against the fake printer and every line sent is checked against PrusaLink's forms.

| Code | Where | Why it never reaches the MK3 family | Proof |
|---|---|---|---|
| `G1 X-10 F3000`, `G1 X10 F3000`, `G1 Y-10 F3000`, `G1 Y10 F3000`, `G1 Z-10 F600`, `G1 Z10 F600` | hub `action` | Other printers' ±10 buttons; the MK3 family's go through `prusalink_control` (`G91`, `G1 F2700 …`) | test `an_mk3_through_an_agent_is_handled_as_prusalink_does` |
| `G28` | hub `action`, `probe_gcode` | Other printers' home and bed measuring; the MK3 family homes `G28 X Y Z` and has no bed measuring (refused) | test `an_mk3_through_an_agent_is_handled_as_prusalink_does` |
| `G29`, `M420 V` | hub `probe_gcode`; agent `follow_idle` | Marlin and Buddy measuring and their mesh; the MK3 family: `M420` only | test `an_mk3_is_sent_only_what_prusalink_sends` |
| `G0 X0 Y0`, `G90 G0 X0 Y0`, `G92 X0 Y0`, `M5` | hub `action`, `grbl_action` | Lasers and GRBL (origin, laser off); refused on the MK3 family | test `an_mk3_through_an_agent_is_handled_as_prusalink_does` |
| `M106 S255`, `M107`, `M18` | hub `action`; agent `STOP` | Other printers' fan and motor buttons, and their stop; the MK3 family: fans refused and hidden, motors off `M84`, stopped with `M77`, `M603` | test `an_mk3_through_an_agent_is_handled_as_prusalink_does`, `an_mk3_parks_itself` |
| `M104`, `M140` | hub `prusalink_control`, `heat_command` | Only the code of a `format!("{code} S{value}")`: sent as `M104 S…`, `M140 S…`, PrusaLink's forms | test `an_mk3_through_an_agent_is_handled_as_prusalink_does` |
| `M105`, `M114`, `M115` | agent `ask_idle`, `get_to_know`, `poke` | Other firmware only: the MK3 family is asked none (its own reports, `PRUSA Fir`; silence: the line again) | test `an_mk3_is_sent_only_what_prusalink_sends`, `a_silent_mk3_gets_the_line_again_as_prusalink_sends_it` |
| `M108`, `M876 S{button}` | agent `wait_ok`, `answer` | Only with `Cap:EMERGENCY_PARSER` / `Cap:PROMPT_SUPPORT` from `M115`, never asked of the MK3 family | test `an_mk3_is_sent_only_what_prusalink_sends` |
| `M155 S0`, `M155 S2` | agent `print.rs` | Other firmware's temperature reports; the MK3 family `M155 S2 C7` | test `an_mk3_parks_itself` |
| `M20 L T` | hub `sd_card.rs` | Other firmware's listing; the MK3 family `M20 LT` | test `an_mk3_is_sent_only_what_prusalink_sends` |
| `M27 C`, `M524` | agent `follow_sd`, `control_sd` | Marlin's SD state and stop; the MK3 family `M27 P`, `M603` | test `an_mk3_is_sent_only_what_prusalink_sends` |
| `M73 P{percent} R{left}` | agent `print.rs` | Marlin's progress; the MK3 family `M73 Q… S… C… P… R… D…` | test `a_file_without_m73_is_given_it` |
| `G1`, `M20`, `M23` | hub `refused_now`, `telemetry`, `sd_card.rs`; agent `carry_out` | Not sent: a line's beginning compared (`starts_with`) | n/a |
| `G80`, `M862.2 P` | agent `marlin.rs` | Not sent: text the printer says, read (`echo:enqueing "G80"`, `M862.2 P` in its answer) | n/a |
| `M1` | hub `gcode.rs` | Not sent: a code recognised in a file being checked | n/a |
| `G81`, `M106`, `M109`, `M190` | hub `virtual_machines.rs` | Not sent: the simulated printers' own handling of what they get | n/a |

## 7. PrusaLink's own lines on the printer's screen

`crates/agent/src/lcd.rs` is `printer_adapter/lcd_printer.py` and `structures/carousel.py`
ported: the same screens, priorities, texts, scrolling, timings and chimes, each line as
`M117 ~text` (between the file's lines while printing). The checker requires every text
literal of `lcd_printer.py` in `lcd.rs` (comments left out), or here.

| Text in `lcd_printer.py` | Why it is not in `lcd.rs` | Status |
|---|---|---|
| `filename`, `rewinding`, `lan`, `error`, `wizard_needed`, `ip`, `speed` | Keys of a screen's conditions (when to set its text again), not shown | n/a |
| `Displaying an error message %s`, `Printed: '%s' on the LCD.`, `LCDPrinter` | Its log lines and its thread's name, not shown | n/a |
| `{local_ip}/{self.printer_number}` | Several PrusaLinks on one Pi (its printer number): the agent runs one per printer | n/a |
| `Go: {local_ip}` | Its setup screen points to PrusaLink's own web page; the agent has none, it is set up by adding it on the hub: it shows `Code: <pairing code>` instead | n/a: kept so by the user's choice, 2026-11-10 (PrusaLink's text would point to nothing) |
| `Preparing recovery` | Shown while PrusaLink reads its file up to the line of a power cut; the agent starts at that line at once, so the screen has nothing to show then (the text is in `lcd.rs`) | n/a |

