# Audit: the agent and hub against PrusaLink

Result by reading both sources at their current state; not yet produced by running PrusaLink against ours.

Reference: PrusaLink in `reference/Prusa-Link` (commit c453996), Prusa's host for the MK3 family over serial. Ours: `crates/agent` (`marlin.rs`, `hub.rs`, `print.rs`, `lcd.rs`) and `crates/hub` (`main.rs`, `sd_card.rs`, `queue.rs`, `store.rs`).

Every row: `Item | PrusaLink | Ours | Result`. Result is one of `**identical**`, `**differs**: <how, with both values>`, `**n/a**: <why>`, `**OPEN**: <not built>`. PrusaLink values are from its source (`printer_polling.py`, `auto_telemetry.py`, `serial_queue.py`, `file_printer.py`, `command_handlers.py`, `web/controls.py`, `regular_expressions.py` and the files named in the rows); ours from the agent and hub source. A code that PrusaLink sends over the web controls is sent in the normal queue; Connect commands and polls go to the front of its queue (`to_front`). The checker `scripts/prusalink-audit-check.py` takes every item below from the PrusaLink source and requires it to be named in backticks.

## 1. What PrusaLink sends

| Item | PrusaLink | Ours | Result |
|---|---|---|---|
| line numbering of sent lines | Only the lines of a file print are numbered and checksummed (`file_printer.py` `to_checksum=True`, `instruction.py` `fill_data`); every other code goes out plain, e.g. `M155 S2 C7` | Every line to an MK3 is sent numbered with checksum, e.g. `N7 M155 S2 C7*cs` (`Printer::send_cmd` and `send_line` call `send_numbered`); only the detection write at connecting is plain | **differs**: PrusaLink plain `M155 S2 C7` outside a file print; ours `N<k> M155 S2 C7*<checksum>` |
| order of the first codes after connecting | After opening: 8 s (`sleep(8)`, `_detect`) on USB, none on the Einsy's pins on a Pi (`is_rpi_port`), only read meanwhile; then `PRUSA Fir` + `M862.2 Q` in one write (`_get_info`); then the polls of `printer_polling.py` and the threads (`M79`, `M155`, `M552`, `M20 LT`) each in their own thread, so their order is not fixed in source | The same wait by port (`is_rpi_port`, `connect_as_prusalink`) and the same `PRUSA Fir` + `M862.2 Q`; then `M400`, `M110 N0`, and the codes of `get_to_know` in one order | **differs**: the wait and the first write identical; ours sends `M400` + `M110 N0` (see line numbering); the threads' order not fixed in PrusaLink |
| a failed poll (`on_fail_interval`) | A poll whose answer fails is asked again after 5 s (`item_updater.py` `default_on_fail_interval`; 10 s for `print_state`, `total_filament`, `total_print_time`; 30 s for `printer_type`; none for `mbl`, `filament_change_in`) | Asked once; an answer that fails is taken as empty (`get_to_know` `ask`), not asked again until the next connection | **differs**: PrusaLink asks again after 5 s (10 s, 30 s for the items named); ours never |
| `M155 S2 C7` | Thread started at once and then every 10 s (`update_interval = 10`): sent when any of temperatures, position, fans has not been heard for 60 s (`REPORTING_TIMEOUT`); all three start at 0, so it goes out at once; also while printing (`auto_telemetry.py`). Measured against PrusaLink itself (2026-11-12, on a pseudo-terminal): first sent only 76 s after opening, not explained by the source (likely a start-up race) | Sent at the end of connecting (`start_reports`); then whenever 60 s pass without all three (`REPORTS_SILENT`), checked every 10 s while idle | **differs**: identical while idle; during a host print ours does not check; the measured 76 s not reproduced |
| `M155 S0 C0` | Sent when PrusaLink stops, waiting up to 5 s (`auto_telemetry.py` `proper_stop`) | Never sent to an MK3 | **differs**: PrusaLink sends `M155 S0 C0` when it stops; ours never |
| `G28 X Y Z` | Web home: `gcode = f'G28 {axes}'` with `axes` a Python list, so the line is `G28 ['X', 'Y', 'Z']` (`web/controls.py` `home`); only in IDLE or READY | `G28 X Y Z` (`prusalink_control`) | **differs**: PrusaLink `G28 ['X', 'Y', 'Z']`; ours `G28 X Y Z` |
| `G90`/`G91` + `G1 F… X Y Z` | `G91`, then `G1 F2700 ['X10']` (`f'G1 F{feedrate} {axes}'` with `axes` a list); `G90` not sent afterwards (`web/controls.py` `jog`) | `G91`, then `G1 F2700 X10`; no `G90` afterwards (`prusalink_control`) | **differs**: PrusaLink `G1 F2700 ['X10']`; ours `G1 F2700 X10` |
| `M220 S` | `M220 S<factor>`, factor 10–999, default 100 (`set_speed`) | `M220 S<n>`, 10–999 (`prusalink_control`) | **identical** |
| `M221 S` | `M221 S<factor>`, factor 10–999 (`api_tool` `flowrate`) | `M221 S<n>`, 10–999 | **identical** |
| `M84` | `M84` (`disable_steppers`) | `M84` for the MK3 family (`prusalink_control`) | **identical** |
| `M83` + `G1 F… E…` | `M83`, then `G1 F100 E<amount>`; amount any number (the limits −10 and 100 are not checked), feed 0–100 (default 100); refused under 170 °C and while PRINTING (`extrude`, `api_tool`) | `M83`, then `G1 F100 E10` or `G1 F100 E-10` only; refused under 170 °C and while busy or paused (`action`, `refused_now`) | **differs**: PrusaLink any amount, refused only while printing; ours ±10 mm only, refused also while paused |
| `M104 S` / `M140 S` | `M104 S<n>` 0–305 °C (`api_tool` `target`); `M140 S<n>` 0–125 °C (`api_bed`); no state check | `M104 S<n>` 0–305; `M140 S<n>` 0–125 (`prusalink_control`) | **identical** |
| `M140`, `M109`, `M104` | Preheat for load and unload, in this order: `M140 S<bed>` always; `M109 S<round5(0.9 × nozzle)>` unless the nozzle is known to be that hot; `M104 S<nozzle>`; values come with the Connect command; refused in PRINTING, ATTENTION, ERROR (`command_handlers.py` `prepare_for_load_unload`) | The same order; `M140` only when the loaded spool names a bed temperature; nozzle from the spool, 215 °C when unknown; no state check found in the hub for load and unload | **differs**: PrusaLink always sends `M140 S<bed>` with the Connect value; ours omits it without a spool bed temperature and takes 215 °C for an unknown nozzle; refusal while printing, attention, error: unknown for ours, no check found |
| `M300 P500 S1` + `M0 Insert the filament` + `M701` | After the preheat codes: `M300 P500 S1`, `M0 Insert the filament`, `M701` | The same three, in this order, after the preheat codes (hub `main.rs`) | **identical** |
| `M702` | After the preheat codes: `M702` | `M702` after the preheat codes | **identical** |
| `M601` / `M602` / `M603` | Each sent once; then waits up to 15 s (`STATE_CHANGE_TIMEOUT`) for PAUSED, PRINTING, STOPPED and fails the command if not reached (`_try_until_state`) | Sent once (`print.rs`, `M603` after `M77` for a stop); a pause or resume not made within 15 s is only said as the job's problem, the job goes on | **differs**: PrusaLink fails the command at 15 s; ours marks a problem and goes on |
| `M75` / `M76` / `M77` | `M75` after `M110 N0` at the start of a host print, `M76` at a pause, `M75` when the printer has said `// action:resumed`, `M77` at the end (`file_printer.py`) | `M75` at the start, `M76` after `M601`, `M77` at the end; at a resume `M602` and `M75` straight after each other, without waiting for `// action:resumed` | **differs**: PrusaLink sends `M75` after `// action:resumed`; ours right after `M602` |
| `M73 P R Q S C D` | For a host-printed file that has no `M73`: at gcode line 100, 200, … and 10 lines before the end, `M73 Q<p> S<left> C-1 P<p> R<left> D-1`; p = round(line/total × 100), 100 at the tail; left = round(print time × (total − line)/line / 60), print time without pauses (`file_printer.py`, `print_stats.py`, `util.py`) | The same text at line 100, 200, … and 10 before the end; p = `n × 100 / total` rounded down, no 100 at the tail; left = minutes rounded down from the time since the start, pauses included (`print.rs`) | **differs**: percent round vs floor and 100 at the tail vs none; minutes rounded vs rounded down; pauses excluded vs included |
| `M73 Q-1 S-1 C-1 P-1 R-1 D-1` | Sent when the state leaves PRINTING, PAUSED, FINISHED, STOPPED for IDLE, BUSY, READY (`_reset_print_stats`) | Sent at the end of a host print and when an SD print has ended (seen at the next 2 s pass) | **differs**: PrusaLink at the state change; ours for an SD print up to 2 s later, at the next pass |
| `M72 S1` | `set_ready`: only from IDLE or READY, then `M72 S1`; also on `// action:ready` | Queued by the hub when "Print taken" is pressed for an MK3 through an agent, and on `// action:ready` | **differs**: PrusaLink only from IDLE or READY; ours whenever Print taken is pressed |
| `M72 S0` | `cancel_ready`: `M72 S0` first, then the state check; also on `// action:not_ready`; and after every printer restart (`printer_reconnected` enqueues `CancelReady`) | On "Not ready after all" and on `// action:not_ready`; not after a restart | **differs**: PrusaLink also after each printer restart; ours not |
| `M28 …/setready.g`, `M84`, `M29` | When `M20 LT` lists a folder "PrusaLink menu" with no `SETREADY.G`: `M28` `<sfn>/setready.g`, `M84`, `M29`, to the front, each confirmed by `ok` or `Done saving file.`; at each listing, which PrusaLink makes at start and at a card insert or eject while IDLE (`filesystem/sd_card.py` `handle_special_menu`) | The same three codes at each listing: at connecting and each time the hub lists the card (SD tab opened, listing older than `FRESH_SECONDS`) | **differs**: listing times: PrusaLink at start and card change; ours at connecting and on each hub listing |
| `M23 path` + `M24` | `M23 <sfn in lower case>`, `M24` only when `File opened` matched, else the command fails (`_load_file`) | `M23 <path in lower case>` (hub `sd_card.rs`), `M24` only after a `File opened:` line (`carry_out`) | **identical** |
| `M20 LT` | At start, and when the tree is invalidated by a card insert or eject, only while IDLE (`sd_card.py` `update`) | `M20 LT` at connecting, and each time the hub asks while the machine is idle and the listing is stale | **differs**: when: PrusaLink at start and card change; ours at connecting and on hub request |
| `M21` | Only when `M20 LT` lists no files and the card state is still unsure (`decide_presence`) | After an empty `M20 LT` at connecting | **identical** |
| `M27 P` | Polled every 1 s, idle and printing alike (`print_state`); after an answer that does not do, again in 10 s (`on_fail_interval`) | Every 1 s in every state (`Mk3Polls`); in an idle machine or SD print asked with the card (`follow_sd`) every loop, also after an answer that did not do | **differs**: identical in every state; ours idle or SD printing does not wait 10 s after an answer that did not do |
| `M79 S"PL"` | `M79 S"PL"`, or `S"PC"` when Connect is used; first at start, then 12 s after the last, to the front (`keepalive.py`) | `M79 S"PC"`, checked on each pass (2 s idle) and between file lines: 12 to 14 s apart (`keep_host_alive`) | **differs**: PrusaLink 12 s; ours 12 to 14 s (pass granularity) |
| `M552 P<ip>` | IP updater every 5 s: `M552 P<ip>` on every change of the IP, `M552 P0.0.0.0` without one, when PrusaLink stops, and after a printer restart (`ip_updater.py`) | Once while connecting: the agent's address, or `M552 P0.0.0.0` | **differs**: PrusaLink on each IP change, stop and restart; ours once at connecting |
| `M117 ~text` | LCD thread wakes at the end of the current line or every 0.2 s at the earliest; `M117 ~<unidecoded text>` to the front, chime after it (`lcd_printer.py`) | `lcd.rs` port, ticked once per pass of the main loop: every 2 s idle, every 1 s printing; `M117 ~<text>` (`ascii`) | **differs**: wake-up 0.2 s vs 1 to 2 s; unidecode vs our own `ascii` for non-ASCII characters: unknown, not compared |
| `M300 …` | Chimes `M300 P100 S3200`, `M300 P25 S0`, `M300 P25 S4800`, `M300 P75 S0`, `M300 P25 S4800` (welcome); `M300 P600 S5` (error); `M300 P14 S50` (upload); `M300 P500 S1` (load); `M300 P200 S600` (cannot re-print); `M300 S880 P200` (comm error) | The same strings (`lcd.rs` constants, hub `queue.rs`, `print.rs`) | **identical** |
| `M110 N0` | Only at the start of a host print (`reset_message_number`, to the front, plain) and when the number reaches 2147483647; not when connecting (`serial_queue.py`, `file_printer.py`) | Sent when connecting (numbered); not at the start of a host print on an MK3 | **differs**: PrusaLink at the start of each host print; ours at connecting only |
| `M400` | The queue puts `M400` in front of an `M110` when none went before (`m110_workaround_slot`): so before `M110 N0` at a host print start | `M400` before `M110 N0` when connecting; none at a host print start | **differs**: PrusaLink `M400` + `M110 N0` at each host print start; ours at connecting |
| `M300 S880 P200` + `M603` + `M1 FW COMM ERR. Aborted` | After the queue failed (an impossible resend) and the printer was reset by `ResetPrinter`, in this order (`_printer_reconnected`) | Sent in this order after a host print was lost on the wire and the printer did not restart, as far as it listens (`print.rs`); no printer reset | **differs**: PrusaLink after a failure and a reset of the printer; ours after a lost line, with no reset |
| `;C32u2_RMD` / `;C32u2_RME` | Turn the USB chip's reset off and on; off by default (`reset_disabling = False`) | — | **n/a**: off by default in PrusaLink and needs the USB chip; ours is on a serial port |
| `PRUSA Fir` | Detection write `PRUSA Fir` + `M862.2 Q`; polled once after the printer type is valid, to the front | The same detection write, then `PRUSA Fir` after `M862.2 Q`; no `M115` to the MK3 family | **identical** |
| `PRUSA SN` | Once; not for MK2.5; not asked when the USB port's serial number is known and valid (`_get_serial_number`) | Once; not for MK2.5; always asked | **differs**: PrusaLink skips the question when the USB port gives the number; ours always asks |
| `M862.2 Q` | Detection write; then polled every 30 s while not printing (`VERY_SLOW_POLL_INTERVAL`), not while printing; after an answer that does not do, again in 10 s (`on_fail_interval=SLOW_POLL_INTERVAL`) | At the detection write and once more at connecting; then on PrusaLink's schedule, again in 10 s after an answer that did not do (`Mk3Polls`) | **differs**: polling identical; ours asks it twice at connecting |
| `M862.1 Q` | Polled every 10 s while idle, not while printing | At connecting, then every 10 s while not printing, not in PRINTING, ATTENTION, ERROR (`Mk3Polls`, PrusaLink's `polling_ok` / `polling_not_ok` and its `ItemUpdater`'s schedule changes) | **identical** |
| `M707 A0x00…0x03` | Once, only for type code 30302 (`MMU3_TYPE_CODE`) | Once, only for 30302 | **identical** |
| `M420` | Read when the item is first enabled (at start), and whenever invalidated: by `tmc…_home_enter(axes_mask=…)` or `echo:enqueing "G80"`, and for an MK2.5 at each change to PRINTING or IDLE; to the front, also while printing | Read after those lines when no SD print runs (`take_mesh_stale`); an MK2.5 at each change to or from a print; not at connecting (the connecting sequence has no `M420`) | **differs**: PrusaLink also reads once at start and also while printing; ours neither |
| `M220` / `M221` (no S) | Every 1 s while idle; every 10 s while PRINTING, ATTENTION, ERROR (`polling_not_ok`) (`printer_polling.py`) | Every 1 s while not printing (idle, paused), every 10 s in PRINTING, ATTENTION, ERROR, SD and host print (`Mk3Polls`, PrusaLink's `polling_ok` / `polling_not_ok` and its `ItemUpdater`'s schedule changes) | **identical** |
| `M73` (no args) | Read once at start, then every 10 s while PRINTING, ATTENTION, ERROR (`print_progress`), SD or host print | Every 10 s in PRINTING, ATTENTION, ERROR, SD and host print (`Mk3Polls`, PrusaLink's `polling_ok` / `polling_not_ok` and its `ItemUpdater`'s schedule changes; its answer used for an SD print) | **differs**: identical but not read once at start |
| `D3 Ax…` (read) | Sheets `D3 AxD49 C88` every 30 s; active sheet `D3 AxDA1 C1` every 10 s; print mode `D3 AxFFF C1` every 10 s, also while printing; flash air `D3 AxFBB C1` every 30 s; job id `D3 AxD05 C4` at start and every 1 s until it equals the written one; totals `D3 AxFF1 C4`, `D3 AxFED C4` at start and when a print ends; again in 5 s after an answer that does not do | The same (`Mk3Polls`, `job_id_due` for a host print's job id) | **differs**: identical but the job id after an SD print starts (PrusaLink ensures it for every job; ours for host prints) |
| `D3 Ax0D05 X…` (write) | `D3 Ax0D05 X<job id:08x>` to the front whenever the job id is set or differs from the printer's (`job.py` `write`), for every job | The same text at the start of a host print on an MK3 (not when taken up again); not for an SD print | **differs**: PrusaLink for every job and on a mismatch; ours only at a host print start |
| `D3 Ax0d15/0d25` (write) | Writes a serial number into the printer (`web/lib/wizard.py`) | — | **n/a**: PrusaLink's own web wizard, factory use; the agent has no wizard |
| custom G-code | Each line to the front; refused in PRINTING, ATTENTION, ERROR unless forced; a reply `echo:Unknown command` or `echo: cold extrusion prevented` fails the command (`ExecuteGcode`) | Each line sent and waited for (`carry_out`); a rejecting reply is shown as a line and does not fail the command; typed G-code is refused while busy or paused | **differs**: PrusaLink fails the command on `Unknown command` and `cold extrusion`, refuses in PRINTING, ATTENTION, ERROR; ours does not fail it and refuses when busy or paused |

## 2. What the printer says, and whether we read it

| Item | PrusaLink | Ours | Result |
|---|---|---|---|
| `TEMPERATURE_REGEX` | `T:… /… B:… /… T0:… @:… B@:… P:… A:…` whole line | A line starting `T:` or `ok T:` counts as a temperature report (`Screen::heard`); values by `marlin::heat` | **differs**: PrusaLink the whole form; ours the prefix `T:` / `ok T:` |
| `POSITION_REGEX` | `X:… Y:… Z:… E:… Count X:… Y:… Z:… E:…` whole line | A line starting `X:` and containing ` Count ` counts as a position report | **differs**: PrusaLink the whole form; ours prefix and ` Count ` |
| `FAN_REGEX` | `E0:… RPM PRN1:… RPM E0@:… PRN1@:…` | A line starting `E0:` and containing ` RPM` counts as a fan report | **differs**: PrusaLink needs `PRN1`, `E0@`, `PRN1@` too; ours only `E0:` and ` RPM` |
| `HEATING_REGEX`, `HEATING_HOTEND_REGEX` | `T:… E:… B:…`, `T:… E:… W:…` while `M109`/`M190` wait; each counts as a temperature report | Counts as a temperature report by the prefix `T:` | **differs**: prefix only vs the two full forms |
| `CONFIRMATION_REGEX` | A line starting `ok` (any text after), or `Done saving file.` | `ok` or a line starting `ok ` (case-insensitive); `Done saving file.` not a confirmation | **differs**: PrusaLink confirms on `ok…` and `Done saving file.`; ours on `ok` and `ok …` only |
| `RESEND_REGEX` | `Resend: N` (space optional) | `Resend: N`, also `rs N`, `rs N12` (case-insensitive) | **differs**: ours also reads `rs N` and `rs 12` |
| `REJECTION_REGEX` | `echo:Unknown command: "…"`, `Unknown … code: …`, `echo: cold extrusion prevented`; fails a custom G-code | Read as a plain line and shown; nothing failed | **differs**: PrusaLink fails the custom G-code; ours only shows the line |
| `BUSY_REGEX` | `echo:busy: processing` only | A line starting `busy:` or `echo:busy` is busy, whatever follows | **differs**: ours reads every `busy:` and `echo:busy…` line, PrusaLink one text |
| `ATTENTION_REGEX` | `echo:busy: paused for user` only | Also `paused for input` anywhere in a `busy:` / `echo:busy` line | **differs**: ours also reads `paused for input` |
| `PAUSE_PRINT_REGEX` … `NOT_READY_REGEX` | `// action:pause` runs a pause (`M601`), `resume` runs a resume (`M602`), `ready` `M72 S1`, `not_ready` `M72 S0`; exactly `// ?action:<word>` ending the line | `//` + any spaces + `action:` + first word: `pause` and `paused` both set the paused flag, `resume` and `resumed` both clear it, `ready`, `not_ready` set the hub's flags | **differs**: PrusaLink `pause` sends `M601`, `resume` sends `M602`; ours sets flags; ours accepts any spacing and text after the word |
| `REPRINT_REGEX` | `// action:start`: the last job again (`StartPrint`), else `M300 P200 S600` and `M117 ~Cannot re-print` | The same through the hub: the last job again, else `M300 P200 S600`, `M117 ~Cannot re-print`; the hub also checks idle, bed clear, file suits | **differs**: ours adds the checks idle, bed clear, file suits |
| `PP_RECOVER_REGEX`, `PP_AUTO_RECOVER_REGEX` | `// action:uvlo_recovery_ready`, `uvlo_auto_recovery_ready` start `PPRecovery`: `D3 AxF91 C4`, message number set to it, file reopened at gcode line − 1 from the file written at every change of the message number | Both words set the power-loss flag; `D3 AxF91 C4` read once; the job goes on from the line after it; the checkpoint is written every 2 s or when the extra lines change | **differs**: PrusaLink writes its recovery file at every message number; ours every 2 s |
| `ERROR_REGEX` | `Error:Printer halted. kill() called!`, `Error:Printer stopped due to errors. Fix…` | An `Error:` line is fatal when it contains any of 12 words (`kill()`, `printer halted`, `thermal runaway`, `mintemp`, `maxtemp`, `heaters switched off`, `printer stopped`, `stopped due to errors`, …); `!!` and `fatal:` lines are errors too | **differs**: ours fatal on 12 words and on `!!`/`fatal:` lines; PrusaLink on two texts |
| `ERROR_REASON_REGEX` | `Error:…Heaters switched off. MINTEMP/MAXTEMP (BED) triggered!`, `THERMAL RUNAWAY` forms | Read by the same word list as `ERROR_REGEX`, also in lines without `Error:` | **differs**: ours also reads such text without `Error:` |
| `ATTENTION_REASON_REGEX` | `Bed leveling failed. Sensor triggered too high`, `… Sensor didn't trigger. Debris on nozzle? Waiting for reset.`, `TM: error triggered!` | Lines containing `bed leveling failed`, `tm: error triggered`, `fan speed is lower than expected`, `printer locked` (case-insensitive, `echo:` and `Error:` removed), and `INT4` | **differs**: ours also reads `printer locked` and `INT4` as needing someone, and by contains |
| `FAN_ERROR_REGEX` | `Extruder`, `Hotend` or `Print` followed by ` fan speed is lower than expected`, whole line; resolved when `FAN_REGEX` is seen again | A line containing `fan speed is lower than expected`; cleared by a resume | **differs**: ours by contains and cleared by a resume; PrusaLink whole line, cleared by a fan report |
| `TM_ERROR_LOG_REGEX` | `TM: error` with the deviation and the threshold, only written to its log | Not read | **n/a**: PrusaLink only logs it, nothing is sent |
| `TM_ERROR_CLEARED` | `TM: error cleared` (`clear_tm_error`) | `TM: error cleared` clears the attention | **differs**: unknown, `clear_tm_error` (the ignored pause, `tm_ignore_pause`) not compared with ours |
| `TM_CAL_START_REGEX`, `TM_CAL_END_REGEX` | `TM: calibration start` blocks the serial queue; `TM: calibr. failed!` or `Thermal Model settings:` unblocks it | Same texts (the end by prefix); nothing sent meanwhile; reported busy | **identical** |
| `MBL_TRIGGER_REGEX` | `tmc…_home_enter(axes_mask=0x..)` or `echo:enqueing "G80"` invalidates the mesh | The same two texts mark it stale | **identical** |
| `MBL_REGEX` | `Mesh bed leveling not active.`, `Num X,Y: a,b`, rows of numbers | `marlin::mesh` | **differs**: unknown, row parsing and the no-mesh text not compared line by line |
| `OPEN_RESULT_REGEX` | `File opened: <path> Size: <n>` or `open failed…` | `File opened: …` and `open failed` read | **identical** |
| `FILE_OPEN_REGEX`, `START_PRINT_REGEX` | `echo:enqueing "M23 <sfn>"` and `echo:enqueing "M24"`: a print from the printer's menu is PRINTING at once, unless the PrusaLink menu file was opened less than 1 s before | `echo:enqueing "M24"` marks an SD print started at once; the `M23` echo not read, the file comes from `File opened:` | **differs**: PrusaLink reads the `M23` echo for the file; ours takes the file from `File opened:` |
| `PRINT_DONE_REGEX` | `Done printing file`: FINISHED at once (or the PrusaLink menu action) | The same text: the print is done at once, or "Set Ready" for the menu file | **identical** |
| `M27_OUTPUT_REGEX` | A path `/….…`, `Not SD printing`, `Print saved`, `SD print paused`, `SD printing byte a/b`, `h:mm` | The same five texts and the bytes; the file from the line before the bytes | **identical** |
| `PRINT_INFO_REGEX` | `NORMAL MODE: Percent done: …; print time remaining in mins: …; Change in mins: …` (SILENT too); time values divided by the speed factor; mode chosen by `print_mode` (NORMAL, else silent when valid, else normal) | `marlin::print_info_full`, the printer's mode (`silent` or not); minutes taken as sent, not adjusted for speed | **differs**: PrusaLink divides the time by speed ÷ 100; ours does not; mode `auto` is silent for PrusaLink, normal for ours |
| `SD_PRESENT_REGEX`, `SD_EJECTED_REGEX` | `echo:SD card ok`, `echo:SD init fail`, `Error:volume.init failed`, `Error:openRoot failed`, `echo:SD card released`; an insert not asked for re-lists the card | The same five texts set "SD card in/none" | **differs**: PrusaLink re-lists the card after an insert or eject; ours only sets the flag |
| `LFN_CAPTURE` | `M20 LT` listing with `Begin file list`, `DIR_ENTER`, long names, `DIR_EXIT`, `End file list` | Read by the hub (`sd_card.rs`) | **differs**: unknown, long-name extension rules (`GCO`/`G`, 52-character limit, uniqueness) not compared |
| `PRINTER_BOOT_REGEX` | `start` | `start` (case-insensitive, whole line) | **differs**: ours also reads `Start`; PrusaLink only `start` |
| `POWER_PANIC_REGEX` | `INT4` | `INT4` (case-insensitive) as "Power failure: the print was saved" | **differs**: ours reads `int4` too, and takes it as a need for someone |
| `LCD_UPDATE_REGEX` | `LCD status changed`: counted, its own lines not (`lcd_updated`) | The same text counted per pass (`take_lcd_changed`) | **identical** |
| `M110_REGEX` | `M110 N…` echoed, used to track the line number (`serial_queue.py`) | Not read | **differs**: PrusaLink reads its own `M110` echo to set the message number; ours sets the number when sending |
| `D3_OUTPUT_REGEX` | `<address>  <hex bytes>` | `marlin::eeprom_bytes` | **differs**: unknown, parsing not compared line by line |
| `MMU_MAJOR/MINOR/REVISION/BUILD_REGEX` | `echo:MMU2:<R0 A…*..`, `echo:MMU3:<R0 A…` | `echo:MMU2:<R<n> A` and `echo:MMU3:<R<n> A` (`marlin::mmu_firmware`) | **identical** |
| `MMU_SLOT_REGEX` | `echo:MMU2:MMU2tool=<n>` | The same text | **identical** |
| `MMU_Q0_REGEX`, `MMU_Q0_RESPONSE_REGEX`, `MMU_PROGRESS_REGEX` | Q0 poll and answers, progress words from `MMU_PROGRESS_MAP` (26 texts), errors with Prusa's code | 26 progress texts, `mmu_error` codes | **differs**: unknown, the mapping tables (`MMU_ERROR_MAP` against `prusa_mmu_code`) not compared entry by entry |
| `RESET_ACTIVATED_REGEX`, `RESET_DEACTIVATED_REGEX` | `Reset mode activated/deactivated`, the answer to `;C32u2_RMD/RME` | — | **n/a**: with `;C32u2_RMD` in §1, off by default |
| `PRINTER_TYPE_REGEX`, `FW_REGEX`, `NOZZLE_REGEX`, `PERCENT_REGEX`, `SN_REGEX` | Answers to the polls of §1: code `\d{3,5}`, version `\d+.\d+.\d+-…`, `\d.\d+`, `\d{0,3}%`, `CZPX…` | `marlin::type_code`, `prusa_fir`, `nozzle`, `percent`, `prusa_serial` | **differs**: unknown, the exact accepted forms (for example the serial number pattern) not compared |
| `VALID_USERNAME_REGEX`, `VALID_PASSWORD_REGEX`, `URLS_FOR_WIZARD`, `VALID_SN_REGEX`, `NEW_SN_REGEX`, `ANY_REGEX` | PrusaLink's own web and wizard | — | **n/a**: not printer messages |

## 3. Result counts

Rows with a Result, §1, §2, §5 and §7: 166.

- **identical**: 53
- **differs**: 95
- **n/a**: 16
- **OPEN**: 2


## 4. Not covered by this audit

- **Buddy printers** (MINI, MK4, XL, CORE One): they do not use PrusaLink over serial; their
  own Connect protocol is in `docs/archive/prusa-connect-protocol.md`.
- **Plain Marlin and other firmware** (OctoPrint's ground): `docs/archive/marlin-serial-protocol.md`.
- **How PrusaLink talks to Prusa Connect** (its SDK side): in `docs/archive/prusa-connect-protocol.md`.
- **Timing and order inside PrusaLink's serial queue** (priorities, `to_front`): read for the
  protocol audit, not repeated here.

## 5. Every item by name

Every item `scripts/prusalink-audit-check.py` takes from the PrusaLink source that the sections above do not name one by one.

### Patterns and handlers named singly

| Item | PrusaLink | Ours | Result |
|---|---|---|---|
| `CANCEL_REGEX` | `// action:cancel`: the file print stops (`stop_print`), the state goes to STOPPED or IDLE | `Action::Cancel`: the job ends as "stopped at the printer" | **differs**: PrusaLink exact `// ?action:cancel`; ours any spacing and text after the word |
| `PAUSED_REGEX` | `// action:paused`: a pause event filtered by the state manager (`filter_pause_events`) | `Action::Paused`: the paused flag is set | **differs**: unknown, `filter_pause_events` (ignored in a thermal-model error) not compared |
| `RESUME_PRINT_REGEX` | `// action:resume` (knob): `fw_resume_print` runs `ResumePrint` (`M602`, waits 15 s) | `Action::Resume`: the paused flag cleared; no `M602` | **differs**: PrusaLink sends `M602` and waits for PRINTING; ours clears the flag only |
| `RESUMED_REGEX` | `// action:resumed`: the state goes to PRINTING and the file print resumes | `Action::Resume` | **identical** |
| `READY_REGEX` | `// action:ready` runs `SetReady` (`M72 S1`) | `Action::Ready`: the hub's "Print taken" and `M72 S1` back | **differs**: PrusaLink only from IDLE or READY; ours whenever Print taken is pressed |
| `MMU_MAJOR_REGEX`, `MMU_MINOR_REGEX`, `MMU_REVISION_REGEX`, `MMU_BUILD_REGEX` | Answers to `M707 A0x00` to `A0x03` | `marlin::mmu_firmware` | **identical** |

### Polled values (`WatchedItem`s of printer_polling.py)

| Item | PrusaLink | Ours | Result |
|---|---|---|---|
| `network_info` | The IP updater's address, MAC, host name for Connect | The agent's address in its report | **n/a**: PrusaLink's own network data for Connect; nothing goes to the printer |
| `printer_type` | `M862.2 Q` every 30 s idle | Every 30 s idle | **identical** (idle); see `M862.2 Q` for connecting |
| `firmware_version` | `PRUSA Fir` once after the type is valid | `PRUSA Fir` once after the type | **identical** |
| `nozzle_diameter` | `M862.1 Q` every 10 s idle | The same | **identical** |
| `serial_number` | `PRUSA SN` once | `PRUSA SN` once | **differs**: PrusaLink skips it when the USB port gives the number; ours always asks (not for MK2.5 in both) |
| `sheet_settings` | `D3 AxD49 C88` every 30 s idle | Every 30 s idle | **identical** |
| `active_sheet` | `D3 AxDA1 C1` every 10 s idle | Every 10 s idle | **identical** |
| `mmu_connected` | Type code is 30302 | Type code is 30302 | **identical** |
| `mmu_version` | `M707 A0x00…A0x03` once | Once | **identical** |
| `job_id` | `D3 AxD05 C4` at start, then every 1 s until it equals the job id after a change (`ensure_job_id`); 5 s after an answer that does not do | At connecting; for a host print 1 s after writing it, written again and read again every 1 s until equal, 5 s after an answer that did not do (`job_id_due`) | **differs**: identical for a host print; PrusaLink also for an SD print's job |
| `print_mode` | `D3 AxFFF C1` every 10 s, also while printing; 0 normal, 1 silent, 2 auto | Every 10 s idle, SD and host print; 0, 1, 2 read | **identical** |
| `mbl` | `M420` at start and when invalidated, also while printing | After a measuring line when no SD print runs; not at start | **differs**: PrusaLink reads at start and while printing; ours only after a measuring line when no SD print runs |
| `flash_air` | `D3 AxFBB C1` every 30 s; its only use is commented out (`sd_card.py`) | `D3 AxFBB C1` every 30 s while idle; not used | **identical** |
| `speed_multiplier` | `M220` every 1 s idle, 10 s printing | The same | **identical** |
| `flow_multiplier` | `M221` every 1 s idle, 10 s printing | The same | **identical** |
| `print_progress` | The printer's `M73` percent, every 10 s while printing | `M73` every 10 s while printing; in a host print the file's own `M73` is shown, the printer's answer not used | **differs**: on the wire identical; in a host print PrusaLink shows the printer's answer, ours the file's |
| `time_remaining` | The printer's `M73` time, divided by speed ÷ 100 | Taken as sent | **differs**: PrusaLink speed-adjusted; ours not |
| `filament_change_in` | "Change in mins" of `M73`, divided by speed ÷ 100; negative is none | Taken as sent, negative is none | **differs**: PrusaLink speed-adjusted; ours not |
| `progress_broken`, `time_broken` | Broken when the `M73` percent is outside 0 to 100 or the time is negative; then bytes and a guess are used | A negative or over-100 `M73` is no value; bytes and the guess are used (`sd_progress`) | **identical** |
| `inaccurate_estimates` | True when the time is broken or the speed is not 100 | `time_estimated`: true when the printer gave no time left of its own | **differs**: PrusaLink also true at a speed other than 100 %; ours not |
| `progress_from_bytes`, `byte_position`, `sd_seconds_printing`, `time_remaining_guesstimate`, `mixed_path` | Percent = whole percent of `M27` bytes when broken; time left = printed × (100 − p) / p, printed from `h:mm`; the long name from the listing | The same formulas (`sd_progress`); the long name in the hub | **identical** |
| `print_state` | `M27 P` every 1 s | `M27 P` every 1 s, idle and printing | **differs**: see `M27 P` |
| `total_filament` | `D3 AxFF1 C4` (metres × 1000) at start and when the state leaves PRINTING | At connecting and when an SD print or a host print ended | **identical** |
| `total_print_time` | `D3 AxFED C4` (minutes × 60) at start and when the state leaves PRINTING | The same | **identical** |

### EEPROM (`EEPROMParams`)

| Item | PrusaLink | Ours | Result |
|---|---|---|---|
| `SHEET_SETTINGS` | 0x0D49, 88 bytes: `D3 AxD49 C88` | `D3 AxD49 C88` | **identical** |
| `ACTIVE_SHEET` | 0x0DA1, 1 byte: `D3 AxDA1 C1` | `D3 AxDA1 C1` | **identical** |
| `PRINT_MODE` | 0x0FFF, 1 byte: `D3 AxFFF C1` | `D3 AxFFF C1` | **identical** |
| `TOTAL_FILAMENT` | 0x0FF1, 4 bytes: `D3 AxFF1 C4` | `D3 AxFF1 C4` | **identical** |
| `TOTAL_PRINT_TIME` | 0x0FED, 4 bytes: `D3 AxFED C4` | `D3 AxFED C4` | **identical** |
| `JOB_ID` | 0x0D05, 4 bytes: `D3 AxD05 C4` read, `D3 Ax0D05 X<8 hex>` written | The same read and write text | **identical** |
| `FLASH_AIR` | 0x0FBB, 1 byte: `D3 AxFBB C1` | `D3 AxFBB C1` (`EEPROM_FLASH_AIR`) | **identical** |
| `EEPROM_FILE_POSITION` | 0x0F91, 4 bytes: `D3 AxF91 C4` read in `PPRecovery` | `D3 AxF91 C4` read once while it waits after the power cut | **identical** |

### G-codes named singly

| Item | PrusaLink | Ours | Result |
|---|---|---|---|
| `D3` | EEPROM reads and the job id write | Reads and the job id write | **differs**: see `D3 Ax…` (read) |
| `G80` | Not sent: only in the pattern `echo:enqueing "G80"` | Not sent: only read | **identical** |
| `M0` | `M0 Insert the filament` when loading | The same | **identical** |
| `M1` | `M1 FW COMM ERR. Aborted` after a communication failure and a reset | The same text after a lost line, no reset | **differs**: PrusaLink after a failure and a printer reset; ours after a lost line, no reset |
| `M117` | Screen messages | PrusaLink's lines, ticked every 1 to 2 s | **differs**: PrusaLink wakes every 0.2 s at the earliest; ours every 1 to 2 s |
| `M155` | `M155 S2 C7` checked every 10 s; `S0 C0` when it stops | `M155 S2 C7` checked every 2 s while idle; `S0 C0` never to an MK3 | **differs**: PrusaLink checks every 10 s and sends `S0 C0` at stop; ours checks every 2 s idle and never sends `S0 C0` |
| `M25` | Not sent by PrusaLink itself; a file line starting `M25` (this includes `M250`, `M251`, …) or `M601` is sent, then pauses the file print (`react_to_gcode`, `startswith`) | A file line whose first word is exactly `M601` or `M25` is sent, then pauses | **differs**: PrusaLink matches by `startswith("M25")`, so `M250` pauses too; ours by the exact first word |
| `M300` | Beeps | The same strings | **identical** |
| `M707` | MMU version | The same | **identical** |
| `M79` | Keep-alive every 12 s | `M79 S"PC"` every 12 to 14 s | **differs**: PrusaLink every 12 s; ours every 12 to 14 s |
| `M862.1` | `M862.1 Q` every 10 s | Once | **differs**: PrusaLink every 10 s; ours once |
| `M862.2` | `M862.2 Q` every 30 s | Once (and the detection write) | **differs**: PrusaLink every 30 s; ours once |

### Commands PrusaLink takes (`command_handlers.py`)

| Item | PrusaLink | Ours | Result |
|---|---|---|---|
| `execute_gcode` | Runs G-code lines to the front; refused in PRINTING, ATTENTION, ERROR unless forced | Typed G-code refused while an agent machine is busy or paused; no force | **differs**: PrusaLink refuses ATTENTION and ERROR, not PAUSED; ours refuses busy and paused, no check for attention and error found |
| `load_filament`, `unload_filament` | §1: preheat, then `M300 P500 S1`, `M0 Insert the filament`, `M701`, or `M702` | The same sequence | **differs**: PrusaLink `M140` always with Connect's bed value; ours only with a spool bed value |
| `set_ready` | `M72 S1` (`M72`: ready, or not) after the state check IDLE or READY | On "Print taken" and on `// action:ready` | **differs**: PrusaLink only from IDLE or READY; ours whenever Print taken is pressed |
| `cancel_ready` | `M72 S0` first, then fails unless READY; also sent after a printer restart | On "Not ready after all" and on `// action:not_ready` | **differs**: PrusaLink also after each printer restart; ours not |
| `job_info` | The job's file, progress, times for Connect | The agent's report to the hub | **n/a**: Connect's own report; nothing goes to the printer |
| `pp_recovery` | Resume after a power cut: `D3 AxF91 C4`, queue number set, file from its own copy, recovery file written at every message number | `D3 AxF91 C4`, the file from the agent's copy, checkpoint every 2 s | **differs**: recovery file at every message number vs every 2 s |
| `reset_printer` | Resets the printer: GPIO 22, else a DTR blip on USB; waits up to 30 s for `start` | — | **OPEN**: not built |
| `disable_resets`, `enable_resets` | `;C32u2_RMD/RME` | — | **n/a**: off by default in PrusaLink (§1) |
| `upgrade_link` | Updates PrusaLink itself with pip | The agent updates itself from the hub, signed | **n/a**: the host's own update; nothing goes to the printer |

### Commands from Prusa Connect (`set_handler`)

| Item | PrusaLink | Ours | Result |
|---|---|---|---|
| `GCODE` | `execute_gcode` | Control's G-code | **differs**: PrusaLink refuses ATTENTION and ERROR, not PAUSED; ours refuses busy and paused |
| `START_PRINT` | SD: `M23`, `M24`; refused while printing or in a state override; host: `M110 N0` first | SD: `M23`, `M24` (identical); host print: `M155 S2 C7`, `D3 Ax0D05 X…`, `M75`, no `M110 N0` | **differs**: PrusaLink `M400`, `M110 N0`, `M75` at a host print start; ours `M155 S2 C7`, `D3 Ax0D05 X…`, `M75` |
| `STOP_PRINT`, `PAUSE_PRINT`, `RESUME_PRINT` | `M603`, `M601`, `M602` once, 15 s wait, the command fails if not reached | Sent once, 15 s check, a problem is said and the job goes on | **differs**: PrusaLink fails the command at 15 s; ours marks a problem and goes on |
| `SEND_JOB_INFO` | `job_info` | The agent's report | **n/a**: Connect's own report; nothing goes to the printer |
| `LOAD_FILAMENT`, `UNLOAD_FILAMENT` | `load_filament`, `unload_filament` | Load / Unload | **differs**: PrusaLink `M140` always with Connect's bed value; ours only with a spool bed value |
| `SET_PRINTER_READY` | `set_ready` | Print taken | **differs**: PrusaLink only from IDLE or READY; ours whenever Print taken is pressed |
| `CANCEL_PRINTER_READY` | `cancel_ready` | "Not ready after all" | **differs**: PrusaLink also after each printer restart; ours not |
| `RESET_PRINTER` | `reset_printer` | — | **OPEN**: not built, see `reset_printer` |
| `UPGRADE` | `upgrade_link` | The agent's own update | **n/a**: the host's own update; nothing goes to the printer |

### Web controls (`web/controls.py`): what they allow and refuse

| Item | PrusaLink | Ours | Result |
|---|---|---|---|
| `jog` | `G91`, `G1 F<feed> ['X10']`; feed 0–2700 (2700 when none given), refused outside; positions bounded only for absolute moves (X 0–255, Y −4–212.5, Z 0.15–210) | `G91`, `G1 F<feed> X10`; feed 0–2700, 2700 for the ±10 buttons; relative moves only | **differs**: PrusaLink `G1 F2700 ['X10']`; ours `G1 F2700 X10` |
| `api_printhead` | Z jog only in IDLE, READY, FINISHED, STOPPED; X and Y jog also in PAUSED; home only in IDLE or READY | Home and Z only when idle; X and Y when idle or paused (`refused_now`); the machine state has no READY, FINISHED, STOPPED | **differs**: PrusaLink Z also in READY, FINISHED, STOPPED; ours idle only (what a finished print reports as is unknown) |
| `home` | `G28 ['X', 'Y', 'Z']` when no axes are given | `G28 X Y Z` | **differs**: PrusaLink `G28 ['X', 'Y', 'Z']`; ours `G28 X Y Z` |
| `set_speed` | `M220 S<n>`, 10–999 % | `M220 S<n>`, 10–999 % | **identical** |
| `extrude` | `M83`, `G1 F100 E<amount>`; refused under 170 °C and while PRINTING | `M83`, `G1 F100 E10` / `E-10`; refused under 170 °C and while busy or paused | **differs**: ours also refuses while paused and offers ±10 mm only |
| `api_tool` | Nozzle `M104 S<n>` 0–305 °C; flow `M221 S<n>` 10–999 % | The same | **identical** |
| `api_bed` | Bed `M140 S<n>` 0–125 °C | The same | **identical** |
| (commands during a print) | Speed, flow and temperatures may be set while printing; X and Y while paused; web controls go into the normal queue behind the file's lines (`enqueue_instruction` without `to_front`) | The same set is allowed (`fits_a_print`); the job sends them between the file's lines (`PrintJob::inject`) | **differs**: PrusaLink queues them in the normal queue; ours sends them between two file lines |

## 6. What we send that PrusaLink does not

Every G-code in the agent's and hub's code (test code left out) that is not one of PrusaLink's
forms, mechanically listed by `scripts/prusalink-audit-check.py`, with how it is kept from
Prusa's MK3 family. The printer must not tell the agent from PrusaLink: an MK3 session is run
against the fake printer and every line sent is checked against PrusaLink's forms.

| Code | Where | Why it never reaches the MK3 family |
|---|---|---|
| `G1 X-10 F3000`, `G1 X10 F3000`, `G1 Y-10 F3000`, `G1 Y10 F3000`, `G1 Z-10 F600`, `G1 Z10 F600` | hub `action` | Other printers' ±10 buttons; the MK3 family's go through `prusalink_control` (`G91`, `G1 F2700 …`) |
| `G28` | hub `action`, `probe_gcode` | Other printers' home and bed measuring; the MK3 family homes `G28 X Y Z` and has no bed measuring (refused) |
| `G29`, `M420 V` | hub `probe_gcode`; agent `follow_idle` | Marlin and Buddy measuring and their mesh; the MK3 family: `M420` only |
| `G0 X0 Y0`, `G90 G0 X0 Y0`, `G92 X0 Y0`, `M5` | hub `action`, `grbl_action` | Lasers and GRBL (origin, laser off); refused on the MK3 family |
| `M106 S255`, `M107`, `M18` | hub `action`; agent `STOP` | Other printers' fan and motor buttons, and their stop; the MK3 family: fans refused and hidden, motors off `M84`, stopped with `M77`, `M603` |
| `M104`, `M140` | hub `prusalink_control`, `heat_command` | Only the code of a `format!("{code} S{value}")`: sent as `M104 S…`, `M140 S…`, PrusaLink's forms |
| `M105`, `M114`, `M115` | agent `ask_idle`, `get_to_know`, `poke` | Other firmware only: the MK3 family is asked none (its own reports, `PRUSA Fir`; silence: the line again) |
| `M108`, `M876 S{button}` | agent `wait_ok`, `answer` | Only with `Cap:EMERGENCY_PARSER` / `Cap:PROMPT_SUPPORT` from `M115`, never asked of the MK3 family |
| `M155 S0`, `M155 S2` | agent `print.rs` | Other firmware's temperature reports; the MK3 family `M155 S2 C7` |
| `M20 L T` | hub `sd_card.rs` | Other firmware's listing; the MK3 family `M20 LT` |
| `M27 C`, `M524` | agent `follow_sd`, `control_sd` | Marlin's SD state and stop; the MK3 family `M27 P`, `M603` |
| `M73 P{percent} R{left}` | agent `print.rs` | Marlin's progress; the MK3 family `M73 Q… S… C… P… R… D…` |
| `G1`, `M20`, `M23` | hub `refused_now`, `telemetry`, `sd_card.rs`; agent `carry_out` | Not sent: a line's beginning compared (`starts_with`) |
| `M113 S2`, `M154 S0`, `M154 S5`, `M27 S1`, `M876 P1`, `M112`, `M104 T0 S0`, `M106 S0`, `M110 N{k}` | agent `marlin.rs` (`connect_as_octoprint`, `after_ok`, `emergency_stop`, `octoprint_reset`, `send_cmd`), `print.rs` (`OCTOPRINT_CANCEL`, the pause) | A plain Marlin printer as OctoPrint drives it: only where `octoprint()` holds, never for the MK3 family (`--mode prusa`) |
| `G0 {} F{speed}`, `G1 E{mm} F{SPEED_E}`, `M82` | hub `octoprint_control` | OctoPrint's controls, for a plain Marlin printer through an agent only (the MK3 family gets `prusalink_control`) |
| `M110`, `G4` | agent `send_cmd` | Not sent: a command's word compared (`word == "M110"`, `"G4"`) |
| `M27` | agent `hub.rs` (`follow_idle`, an SD print's `//action:resumed`) | OctoPrint's bare `M27`, for a plain Marlin printer only (`octoprint()`); the MK3 family gets `M27 P` |
| `G80`, `M862.2 P` | agent `marlin.rs` | Not sent: text the printer says, read (`echo:enqueing "G80"`, `M862.2 P` in its answer) |
| `M1` | hub `gcode.rs` | Not sent: a code recognised in a file being checked |
| `G81`, `M106`, `M109`, `M190` | hub `virtual_machines.rs` | Not sent: the simulated printers' own handling of what they get |

## 7. PrusaLink's own lines on the printer's screen

`crates/agent/src/lcd.rs` is `printer_adapter/lcd_printer.py` and `structures/carousel.py`
ported: the same screens, priorities, texts, scrolling, timings and chimes, each line as
`M117 ~text` (between the file's lines while printing). The checker requires every text
literal of `lcd_printer.py` in `lcd.rs` (comments left out), or here.

| Text in `lcd_printer.py` | Why it is not in `lcd.rs` | Result |
|---|---|---|
| `filename`, `rewinding`, `lan`, `error`, `wizard_needed`, `ip`, `speed` | Keys of a screen's conditions (when to set its text again), not shown | **n/a** |
| `Displaying an error message %s`, `Printed: '%s' on the LCD.`, `LCDPrinter` | Its log lines and its thread's name, not shown | **n/a** |
| `{local_ip}/{self.printer_number}` | Several PrusaLinks on one Pi (its printer number): the agent runs one per printer | **n/a** |
| `Go: {local_ip}` | Its setup screen points to PrusaLink's own web page; the agent has none, it is set up by adding it on the hub: it shows `Code: <pairing code>` instead | **n/a**: the owner's choice (PrusaLink's text would point to nothing) |
| `Preparing recovery` | Shown while PrusaLink reads its file up to the line of a power cut; the agent starts at that line at once, so the screen has nothing to show then (the text is in `lcd.rs`) | **n/a** |
