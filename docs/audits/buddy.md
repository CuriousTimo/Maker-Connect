# Buddy firmware audit (Prusa Connect, the hub's side)

Reference: **Prusa-Firmware-Buddy 8.0.2** (`reference/buddy-8.0.2`, commit `b1043d6`), the current release
and the only one supported (owner, 2026-11-24, #3): `src/connect` (the Connect client), `src/common/http`,
`src/common/general_response.cpp`, `src/state/printer_state.cpp`. The MINI is the printer tried on; what only a
printer with several tools or an MMU sends is #211. Ours: the hub as Prusa Connect's server,
`crates/hub/src/devices/prusa_connect.rs` and `printer_files.rs` (and what they call).
The Connect server itself is closed; what binds the hub is what the firmware's Connect client sends and
accepts.

Every item is extracted from the firmware's source by `scripts/buddy-audit-check.py` (never typed by hand).
Per row: Buddy's `file:line`, ours `file:line`, a Result, and the test that shows it. The checker fails when a
cited line does not contain the row's item (or the anchor after `=`), when a named test does not exist, when
the audit's command table or answer list differs from the firmware's, or when a row says "not used by the
hub" and the hub's code names it.

**How a row was proven.** Buddy's own C++ is not built or run (it needs the whole firmware). The tests are in
`crates/hub/src/devices/buddy_audit.rs`:

- a Rust port of `Command::parse_json_command` (`command.cpp:108-433`: events in text order, depth, key and type
  tests, the command table, `MAX_TOKENS` 60, the 512-byte answer buffer of `connect.cpp:203`), fed with the bodies
  the hub really answers a telemetry report with (the hub's own routes and store produce them);
- events written the way `render.cpp` `render_msg` writes them (key order, nesting, types), taken line by line by
  reading the serializer, not produced by it, fed to the hub's `/p/events`;
- the answers of the hub checked against what `connect.cpp` (`send_command`, `handle_server_resp`) and
  `registrator.cpp` accept.

Result is one of **identical** (the port or the fixtures show the hub's side matches the reference), **differs**
`D<n>` (see the Differences section), **not used by the hub** (a command or argument the hub never sends or
reads; not built), **n/a**, **OPEN** (not proven), or UNCHECKED (not looked at
in this step). A fixture written by reading is not the firmware's own output: where that is all there is, the
row says OPEN, not identical.

## Commands it takes from Connect (`command.cpp`)

| Item | Buddy 8.0.2 | Hub | Result | Test |
|---|---|---|---|---|
| `CANCEL_OBJECT` | `command.cpp:193` |  | **not used by the hub** |  |
| `CANCEL_PRINTER_READY` | `command.cpp:174` |  | **not used by the hub** |  |
| `CREATE_FOLDER` | `command.cpp:178` | `devices/printer_files.rs:1582` | **identical**: taken as `CreateFolder` with `path` | `buddy_takes_every_command_the_hub_sends` |
| `DELETE_FILE` | `command.cpp:176` | `devices/printer_files.rs:1546` | **identical**: taken as `DeleteFile` with `path` | `buddy_takes_every_command_the_hub_sends` |
| `DELETE_FOLDER` | `command.cpp:177` | `devices/printer_files.rs:1544` | **identical**: taken as `DeleteFolder` with `path` | `buddy_takes_every_command_the_hub_sends` |
| `DIALOG_ACTION` | `command.cpp:191` | `web/machines.rs:507` | **identical**: taken as `DialogAction` with `dialog_id` and `button`; the hub offers the buttons only when the printer named a dialog, and refuses a press without one ("the printer named no dialog"), so it never sends one without `dialog_id` | `buddy_takes_every_command_the_hub_sends`, `a_dialog_answer_without_a_dialog_is_refused_by_the_hub` |
| `PAUSE_PRINT` | `command.cpp:169` | `devices/printer_files.rs:1205` | **identical**: taken as `PausePrint` | `buddy_takes_every_command_the_hub_sends` |
| `RESET` | `command.cpp:189` |  | **not used by the hub** |  |
| `RESET_PRINTER` | `command.cpp:188` |  | **not used by the hub** |  |
| `RESUME_PRINT` | `command.cpp:171` | `devices/printer_files.rs:1206` | **identical**: taken as `ResumePrint` | `buddy_takes_every_command_the_hub_sends` |
| `SEND_FILE_INFO` | `command.cpp:167` | `devices/printer_files.rs:1017` `devices/printer_files.rs:1299` | **identical**: the body Buddy's port takes as `SendFileInfo` with its `path` | `buddy_takes_every_command_the_hub_sends` |
| `SEND_INFO` | `command.cpp:165` |  | **not used by the hub** |  |
| `SEND_JOB_INFO` | `command.cpp:166` | `devices/printer_files.rs:1086` | **identical**: taken as `SendJobInfo` with `job_id` | `buddy_takes_every_command_the_hub_sends` |
| `SEND_STATE_INFO` | `command.cpp:190` |  | **not used by the hub** |  |
| `SEND_TRANSFER_INFO` | `command.cpp:168` |  | **not used by the hub** |  |
| `SET_IDLE` | `command.cpp:175` |  | **not used by the hub** |  |
| `SET_PRINTER_READY` | `command.cpp:173` | `farm.rs:1605` | **identical**: the literal body, taken as `SetPrinterReady` | `buddy_takes_set_printer_ready` |
| `SET_TOKEN` | `command.cpp:180` |  | **not used by the hub** |  |
| `SET_VALUE` | `command.cpp:192` |  | **not used by the hub** for a Buddy printer: the hub sends it only to a Marlin printer on the agent (`commands.rs` `set_value`, extension part 9); to a Buddy printer speed, flow and targets go as G-code | `agent_names_never_go_to_a_buddy` |
| `START_CONNECT_DOWNLOAD` | `command.cpp:202` | `devices/prusa_connect.rs` `label_job` | **OPEN**: the hub sends it to the agent's label printer; not compared with Buddy |  |
| `START_ENCRYPTED_DOWNLOAD` | `command.cpp:204` | `devices/printer_files.rs:593` | **OPEN**: in Buddy 8.0.2 the command is in the table only `#if !WEBSOCKET()` (`command.cpp:203-205`), and the MINI is built with WebSocket (`ProjectOptions.cmake:833`): a MINI on 8.0.2 does not have it (#212). The test's port of the parser is 6.3.0's, which has it | `buddy_takes_every_command_the_hub_sends` |
| `START_INLINE_DOWNLOAD` | `command.cpp:195` |  | **not used by the hub** |  |
| `START_PRINT` | `command.cpp:172` | `devices/printer_files.rs:396` `devices/printer_files.rs:1493` | **identical**: taken as `StartPrint` with `path` (no `tool_mapping`: optional) | `buddy_takes_every_command_the_hub_sends` |
| `STOP_PRINT` | `command.cpp:170` | `devices/printer_files.rs:1207` | **identical**: taken as `StopPrint` | `buddy_takes_every_command_the_hub_sends` |
| `STOP_TRANSFER` | `command.cpp:179` |  | **not used by the hub** for a Buddy printer: the hub sends it only to a label printer on the agent (`devices/prusa_connect.rs` `stop_label`) | `agent_names_never_go_to_a_buddy` |
| `UNCANCEL_OBJECT` | `command.cpp:194` |  | **not used by the hub** |  |

## Arguments it reads from a command (`command.cpp`)

| Item | Buddy 8.0.2 | Hub | Result | Test |
|---|---|---|---|---|
| `addon_power` | `command.cpp:447` |  | **not used by the hub** |  |
| `button` | `command.cpp:348` | `web/machines.rs:506` | **identical**: one of the printer's own names (see Answers a dialog takes) | `buddy_takes_every_command_the_hub_sends` |
| `chamber.fan_pwm_target` | `command.cpp:445` |  | **not used by the hub** |  |
| `chamber.led_intensity` | `command.cpp:451` |  | **not used by the hub** |  |
| `chamber.target_temp` | `command.cpp:443` |  | **not used by the hub** |  |
| `command` | `command.cpp:158` | `devices/printer_files.rs:1440` | **identical**: in every body, a string at depth 1 (`:147`) | `buddy_takes_every_command_the_hub_sends` |
| `dialog_id` | `command.cpp:437` | `web/machines.rs:506` | **identical**: a number, the one the printer's `STATE_CHANGED` named (`render.cpp:673`); with none named there is no `DIALOG_ACTION` | `buddy_takes_every_command_the_hub_sends`, `a_dialog_answer_without_a_dialog_is_refused_by_the_hub` |
| `enclosure_enabled` | `command.cpp:371` |  | **not used by the hub** |  |
| `enclosure_postprint` | `command.cpp:375` |  | **not used by the hub** |  |
| `enclosure_postprint_filtration_time` | `command.cpp:377` |  | **not used by the hub** |  |
| `enclosure_printing_filtration` | `command.cpp:373` |  | **not used by the hub** |  |
| `hash` | `command.cpp:425` |  | **not used by the hub** for a Buddy printer: the hub sends it only to a Marlin printer on the agent, in the Connect SDK's `START_CONNECT_DOWNLOAD` (`devices/printer_files.rs` `start_transfer`) | `agent_names_never_go_to_a_buddy` |
| `hostname` | `command.cpp:357` |  | **not used by the hub** |  |
| `id` | `command.cpp:439` |  | **not used by the hub** |  |
| `iv` | `command.cpp:434` | `devices/printer_files.rs:597` | **identical**: 32 hex digits | `buddy_takes_every_command_the_hub_sends` |
| `job_id` | `command.cpp:289` | `devices/printer_files.rs:1086` | **identical**: a number that fits 16 bits (it is the printer's own `job_id`) | `buddy_takes_every_command_the_hub_sends` |
| `key` | `command.cpp:432` | `devices/printer_files.rs:596` | **identical**: 32 hex digits | `buddy_takes_every_command_the_hub_sends` |
| `kwargs` | `command.cpp:285` | `devices/printer_files.rs:1439` | **identical**: an object at depth 1 (`:265`) | `buddy_takes_every_command_the_hub_sends` |
| `orig_size` | `command.cpp:418` | `devices/printer_files.rs:598` | **identical**: a number that fits 32 bits | `buddy_takes_every_command_the_hub_sends` |
| `path` | `command.cpp:291` | `devices/printer_files.rs:1390` | **OPEN**: Buddy 8.0.2 refuses a path longer than `filename_defs::max_hybrid_path_length` as a broken command "Path too long" (`command.cpp:226`); the test's port of the parser is 6.3.0's, which has no such limit (#201) | `buddy_takes_every_command_the_hub_sends` |
| `path_sfn` | `command.cpp:291` |  | **not used by the hub** |  |
| `port` | `command.cpp:415` |  | **not used by the hub** |  |
| `team_id` | `command.cpp:423` |  | **not used by the hub** for a Buddy printer: as `hash`: only in the Marlin's `START_CONNECT_DOWNLOAD` on the agent | `agent_names_never_go_to_a_buddy` |
| `token` | `command.cpp:336` |  | **not used by the hub** |  |
| `tool_mapping` | `command.cpp:302` |  | **not used by the hub** |  |
| `tools.<n>.hardened` | `command.cpp:394` |  | **not used by the hub** |  |
| `tools.<n>.high_flow` | `command.cpp:391` |  | **not used by the hub** |  |
| `tools.<n>.nozzle_diameter` | `command.cpp:397` |  | **not used by the hub** |  |

## Events it sends (`planner.cpp` `to_str`, shaped by `render.cpp` `render_msg`)

Fourteen types, the whole list of `planner.cpp:253-280`. Common shape (`render.cpp:321-683`): `job_id` (when a job runs and the type is not `ACCEPTED`/`REJECTED`), `reason`, `machine_reason`, `data`, `dialog_id`, `state`, `command_id`, `transfer_id`, `event` last.

| Item | Buddy 8.0.2 | Hub | Result | Test |
|---|---|---|---|---|
| `ACCEPTED` | `planner.cpp:269` | `devices/prusa_connect.rs:1034=ACCEPTED` | **identical**: a command with that id becomes accepted; the event has no `job_id` or `data` (`render.cpp:269`) | `the_events_buddy_sends_settle_commands` |
| `REJECTED` | `planner.cpp:271` | `devices/prusa_connect.rs:1046=REJECTED` | **identical**: the command becomes rejected; `reason` and `machine_reason` (`render.cpp:327`, `:331`) are kept as the reason. The hub reads `machine_reason` `FILE_EXISTS` to send a file again under a free name, not the text | `the_events_buddy_sends_settle_commands`, `a_free_name_is_decided_by_machine_reason` |
| `FINISHED` | `planner.cpp:279` | `devices/prusa_connect.rs:1041=FINISHED` | **identical**: the command becomes finished | `the_events_buddy_sends_settle_commands` |
| `FAILED` | `planner.cpp:281` | `devices/prusa_connect.rs:1046=FAILED` | **identical**: the command becomes rejected (a background G-code that failed, `planner.cpp:1112`) | `the_events_buddy_sends_settle_commands` |
| `INFO` | `planner.cpp:267` | `devices/prusa_connect.rs:1194=INFO` | **identical**: the command is finished and the details are kept (serial, `printer_type` as the string `2.1.0`, firmware, `nozzle_diameter` at the top level, `tools.<n>.material` with `---` read as none, storages, network); keys only master writes are not read | `the_events_buddy_sends_settle_commands`, `info_says_a_dash_for_no_material`, `printer_type_is_read_as_buddys_string_only`, `info_of_6_3_0_is_read_as_buddy_writes_it` |
| `JOB_INFO` | `planner.cpp:273` | `devices/printer_files.rs:811=JOB_INFO` | **identical**: the command is finished; `job_id` at the top and `data.display_name` give the file a print is (as the hub's telemetry shows it) | `the_events_buddy_sends_settle_commands`, `state_changed_and_job_info_reach_the_hub` |
| `FILE_INFO` | `planner.cpp:275` | `devices/printer_files.rs:798=FILE_INFO` | **identical**: the command is finished; a folder's `children` and `path` fill the hub's listing of the stick | `the_events_buddy_sends_settle_commands`, `the_listing_follows_file_info_and_file_changed` |
| `TRANSFER_INFO` | `planner.cpp:277` | `devices/printer_files.rs:849=TRANSFER_INFO` | **identical**: the download command is finished (taken, the file on its way) | `the_events_buddy_sends_settle_commands`, `transfer_events_name_their_command_in_data` |
| `TRANSFER_STOPPED` | `planner.cpp:283` | `devices/printer_files.rs:868=TRANSFER_STOPPED` | **OPEN**: no top-level `command_id` (`render.cpp:537`); the hub takes the command from `data.start_cmd_id`; the stopped branch (failed transfer, job failed) is not run by a test here |  |
| `TRANSFER_ABORTED` | `planner.cpp:285` | `devices/printer_files.rs:868=TRANSFER_ABORTED` | **OPEN**: as `TRANSFER_STOPPED`; the `machine_reason` (`STORAGE_FAILURE`, `NETWORK_FAILURE`) wording is not run by a test here |  |
| `TRANSFER_FINISHED` | `planner.cpp:287` | `devices/printer_files.rs:868=TRANSFER_FINISHED` | **identical**: no top-level `command_id` (`render.cpp:537`); the hub settles the download command by `data.start_cmd_id` and marks the transfer finished | `transfer_events_name_their_command_in_data` |
| `FILE_CHANGED` | `planner.cpp:289` | `devices/printer_files.rs:828=FILE_CHANGED` | **identical**: the command (delete, make folder) is finished by it (`planner.cpp:850`); `old_path` takes the file out of the listing | `the_events_buddy_sends_settle_commands`, `the_listing_follows_file_info_and_file_changed` |
| `CANCELABLE_CHANGED` | `planner.cpp:291` | `devices/prusa_connect.rs:1042=CANCELABLE_CHANGED` | **identical**: the command (`CANCEL_OBJECT`, `UNCANCEL_OBJECT`, never sent by the hub) would be finished; the `objects` list is not read by the hub | `the_events_buddy_sends_settle_commands` |
| `STATE_CHANGED` | `planner.cpp:293` | `devices/printer_files.rs:751=STATE_CHANGED` | **identical**: `data.code`, `title`, `text`, `buttons` and the top-level `dialog_id` give what the screen asks (and the buttons for `DIALOG_ACTION`); a state without a dialog clears it | `the_events_buddy_sends_settle_commands`, `state_changed_and_job_info_reach_the_hub` |

## What it does with the server's answer, and its registration

| Item | Buddy 8.0.2 | Hub | Result | Test |
|---|---|---|---|---|
| status `204` to a report or event | `connect.cpp:721=NoContent` | `devices/prusa_connect.rs:989=NO_CONTENT` | **identical**: nothing to tell; the printer goes on | `the_answer_to_a_report_is_what_buddy_reads` |
| status `200` with a command | `connect.cpp:725=Status::Ok` | `devices/prusa_connect.rs:980=StatusCode::OK` | **identical**: `Command-Id` as decimal digits (`command_id.cpp`), a content type Buddy knows, a body of at most 512 bytes | `the_answer_to_a_report_is_what_buddy_reads`, `buddy_takes_every_command_the_hub_sends` |
| `Command-Id` header | `resp_parser.cpp:51=CommandId` | `devices/prusa_connect.rs:982=Command-Id` | **identical**: the command's number, as digits | `the_answer_to_a_report_is_what_buddy_reads` |
| `Content-Type: application/json` command | `resp_parser.cpp:42=ApplicationJson` | `devices/prusa_connect.rs:975=application/json` | **identical**: a JSON command parsed by `parse_json_command` | `buddy_takes_every_command_the_hub_sends` |
| `Content-Type: text/x.gcode` command | `resp_parser.cpp:45=TextGcodeDot` | `devices/prusa_connect.rs:977=text/x.gcode` | **identical**: G-code lines; `text/x-gcode` is the same to Buddy (`:48`) | `the_answer_to_a_report_is_what_buddy_reads` |
| a command body over 512 bytes | `connect.cpp:198=512` | `commands.rs:192=512` | **OPEN**: JSON commands the hub builds are far smaller (tested); the G-code of the hub's typed commands is limited to 480 characters on the hub's side, a limit that is the hub's UI. A G-code body of the MINI mapping (HS10) is not covered | `buddy_takes_every_command_the_hub_sends` |
| one command at a time | `planner.cpp:678=Processing other command` | `store/commands.rs:216=next_command_of_any_kind` | **identical**: the hub sends one and the next only once the first is answered (FINISHED or REJECTED), however long Buddy carries it out (its `command_id` in telemetry, `render.cpp:251`); no time of the hub's ends it. When the printer itself was not seen for longer than the hub's `ONLINE_SECONDS`, what it had not answered fails with "the printer was gone" (ours, K18 (b), not on the wire) | `the_answer_to_a_report_is_what_buddy_reads`, `a_command_stays_pending_until_buddy_answers_it` |
| status `401` to an unknown token | `connect.cpp:626=Unauthorized` | `devices/prusa_connect.rs:991=UNAUTHORIZED` | **identical**: Buddy reads it as an authentication error and refuses the report | `the_answer_to_a_report_is_what_buddy_reads` |
| a command in the answer to `/p/events` | `connect.cpp:692=send_command` | `devices/prusa_connect.rs:996=fn event` | **identical**: Buddy takes a command from any `200` answer; the hub answers events with `204` and sends commands in the answer to telemetry only | `the_events_buddy_sends_settle_commands` |
| `POST /p/register`: body | `registrator.cpp:60=sn` | `devices/prusa_connect.rs:507=register` | **identical**: `sn`, `fingerprint`, `printer_type` as the string `2.1.0`, `firmware`, in that order, read; the hub answers `200` with a `Code` header of at most 25 characters | `registration_is_what_registrator_cpp_does` |
| `GET /p/register`: poll | `registrator.cpp:210=Accepted` | `devices/prusa_connect.rs:591=ACCEPTED` | **identical**: `202` until added, then `200` with `Token` (at most 20 characters, `printer.hpp:190`), again for as long as the printer has not reported with it (`registrator.cpp:233`); once it has, the code is spent and gets `404` | `registration_is_what_registrator_cpp_does`, `a_mini_registers_by_itself` |
| raw G-code as a command | `command.cpp:101=gcode_command` | `devices/prusa_connect.rs:977=text/x.gcode` | **OPEN**: the transport is read as Buddy reads it; the G-code itself (what the hub sends for jog, heat, home; HS10) is not part of this step |  |

## Endpoints it calls

| Item | Buddy 8.0.2 | Hub | Result | Test |
|---|---|---|---|---|
| `/f/<iv>/raw` | `planner.cpp:187=/raw` | `devices/printer_files.rs:1229=/f/{iv}/raw` | **OPEN**: the download (Range, `Content-Encryption-Mode: AES-CTR`, decryption, 200/206) is not run against `transfers/download.cpp` in this step |  |
| `/p/events` | `connect.cpp:124` | `devices/prusa_connect.rs:108=/p/events` | **identical**: `POST`, `204` | `the_events_buddy_sends_settle_commands` |
| `/p/register` | `registrator.cpp:41` | `devices/prusa_connect.rs:106=/p/register` | **identical** | `registration_is_what_registrator_cpp_does` |
| `/p/telemetry` | `connect.cpp:121` | `devices/prusa_connect.rs:107=/p/telemetry` | **identical**: `POST`, the answer as described above | `the_answer_to_a_report_is_what_buddy_reads` |
| `/p/ws` | `connect.cpp:173` | `devices/prusa_connect.rs` `ws` | **OPEN**: the hub serves it (the agent; 6.4+, 6.3.0 never asks for it); not compared with Buddy |  |

## Headers it sends

| Item | Buddy 8.0.2 | Hub | Result | Test |
|---|---|---|---|---|
| `Code` | `registrator.cpp:100` | `devices/prusa_connect.rs:589=code` | **identical**: read from `GET /p/register` | `registration_is_what_registrator_cpp_does` |
| `Fingerprint` |  |  | **not used by the hub**: sent by the printer; the hub reads no `Fingerprint` |  |
| `Sec-WebSocket-Key` |  |  | **not used by the hub**: WebSocket (6.4+) |  |
| `Sec-WebSocket-Protocol` |  |  | **not used by the hub**: WebSocket (6.4+) |  |
| `Sec-WebSocket-Version` |  |  | **not used by the hub**: WebSocket (6.4+) |  |
| `Token` | `connect.cpp:138` | `devices/prusa_connect.rs:816=token` | **identical**: names the machine on every report and event | `buddy_takes_every_command_the_hub_sends` |
| `Upgrade` |  |  | **not used by the hub**: WebSocket (6.4+) |  |
| `User-Agent-Printer` |  |  | **not used by the hub**: sent by the printer; the hub reads none |  |
| `User-Agent-Version` |  | `devices/prusa_connect.rs:867=user-agent-version` | **OPEN**: read as the firmware's version; the value `6.3.0+0.LOCAL` is from an earlier run on the MINI, not from the firmware's source |  |

## What it reads in an answer (`common/http/resp_parser.cpp`)

| Item | Buddy 8.0.2 | Hub | Result | Test |
|---|---|---|---|---|
| `ApplicationJson` | `resp_parser.cpp:42` | `devices/prusa_connect.rs:975=application/json` | **identical**: the hub's JSON command says `application/json` | `buddy_takes_every_command_the_hub_sends` |
| `Body` | `resp_parser.cpp:32` |  | **n/a**: the automaton's own state for the body |  |
| `Code` | `resp_parser.cpp:54` | `devices/prusa_connect.rs:581=Code` | **identical**: `Code` of the registration answer | `registration_is_what_registrator_cpp_does` |
| `CommandId` | `resp_parser.cpp:51` | `devices/prusa_connect.rs:982=Command-Id` | **identical**: `Command-Id` | `the_answer_to_a_report_is_what_buddy_reads` |
| `ConnectionHeader` | `resp_parser.cpp:72` |  | **OPEN**: set by the HTTP server (axum); not read against the parser here |  |
| `ConnectionKeepAlive` | `resp_parser.cpp:76` |  | **OPEN**: as `ConnectionHeader` |  |
| `ConnectionUpgrade` | `resp_parser.cpp:69` |  | **not used by the hub**: WebSocket (6.4+) |  |
| `ContentEncryptionModeCBC` | `resp_parser.cpp:79` |  | **not used by the hub**: the hub says `AES-CTR` |  |
| `ContentEncryptionModeCTR` | `resp_parser.cpp:82` | `devices/printer_files.rs:1741=content-encryption-mode` | **OPEN**: the hub's download says `AES-CTR`; the download is not run against Buddy here |  |
| `ContentLength` | `resp_parser.cpp:36` |  | **OPEN**: set by the HTTP server for answers and by the download; not read against the parser here |  |
| `StatusCode` | `resp_parser.cpp:29` |  | **OPEN**: the codes the hub answers are listed above |  |
| `TextGcodeDash` | `resp_parser.cpp:48` |  | **not used by the hub**: the hub says `text/x.gcode` |  |
| `TextGcodeDot` | `resp_parser.cpp:45` | `devices/prusa_connect.rs:977=text/x.gcode` | **identical**: the hub's G-code command says `text/x.gcode` | `the_answer_to_a_report_is_what_buddy_reads` |
| `Token` | `resp_parser.cpp:57` | `devices/prusa_connect.rs:590=Token` | **identical**: `Token` of the registration answer | `registration_is_what_registrator_cpp_does` |
| `UpgradeWebsocket` | `resp_parser.cpp:66` |  | **not used by the hub**: WebSocket (6.4+) |  |
| `Version` | `resp_parser.cpp:85` |  | **OPEN**: HTTP version of the server's answer; not read against the parser here |  |
| `WebSocketAccept` | `resp_parser.cpp:60` |  | **not used by the hub**: WebSocket (6.4+) |  |
| `WsPrusaConnect` | `resp_parser.cpp:63` |  | **not used by the hub**: WebSocket (6.4+) |  |

## Fields it sends (telemetry, events, info)

Only the fields the event tests show are checked here; the rest of the telemetry and of `INFO` is UNCHECKED.

| Item | Buddy 8.0.2 | Hub | Result | Test |
|---|---|---|---|---|
| `abbreviation` |  |  | UNCHECKED |  |
| `active` |  |  | UNCHECKED |  |
| `actual_netto_full_weight` | `render.cpp:504` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `HAS_ANFC`, the Core One and Core One L only (`ProjectOptions.cmake:814`). Another Buddy printer: later (#87) |  |
| `addon_power` | `render.cpp:610` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under the Core One and Core One L only; the xBuddy extension, the Core One family and iX only (`ProjectOptions.cmake:520`). Another Buddy printer: later (#87) |  |
| `api_key` |  |  | UNCHECKED |  |
| `appendix` |  |  | UNCHECKED |  |
| `axis_x` |  |  | UNCHECKED |  |
| `axis_y` |  |  | UNCHECKED |  |
| `axis_z` |  |  | UNCHECKED |  |
| `base_model` | `render.cpp:466` |  | **n/a**: not on the MINI: written only `#if HAS_MMU2()` (`render.cpp`), which only the MK4, MK3.5, Core One and Core One L have (`ProjectOptions.cmake:592`); an MMU is #211 |  |
| `buttons` | `render.cpp:826` | `devices/printer_files.rs:781=buttons` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `canceled` |  |  | UNCHECKED |  |
| `chamber` | `render.cpp:203` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under the Core One and Core One L only. Another Buddy printer: later (#87) |  |
| `chamber_temperature` | `render.cpp:571` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `HAS_CHAMBER_API`, the XL and Core One family only (`ProjectOptions.cmake:595`). Another Buddy printer: later (#87) |  |
| `children` | `render.cpp:1186` | `devices/printer_files.rs:475=children` | **identical**: read from where `render.cpp` writes it | `the_listing_follows_file_info_and_file_changed` |
| `chunk` |  |  | UNCHECKED |  |
| `class` |  |  | UNCHECKED |  |
| `code` | `render.cpp:787` | `devices/printer_files.rs:768=code` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `color_rgba` |  |  | UNCHECKED |  |
| `command` |  |  | UNCHECKED |  |
| `command_id` | `render.cpp:294` | `devices/prusa_connect.rs:1032=command_id` | **identical**: read from where `render.cpp` writes it | `the_events_buddy_sends_settle_commands` |
| `consumed_weight` |  |  | UNCHECKED |  |
| `data` | `render.cpp:388` | `devices/printer_files.rs:747=data` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `dialog_id` | `render.cpp:298` | `devices/printer_files.rs:787=dialog_id` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub`, `buddy_takes_every_command_the_hub_sends` |
| `display_name` | `render.cpp:631` | `devices/printer_files.rs:812=display_name` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `enabled` | `render.cpp:589` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `XL_ENCLOSURE_SUPPORT`, the XL only (`ProjectOptions.cmake:601`). Another Buddy printer: later (#87) |  |
| `enclosure` | `render.cpp:195` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `XL_ENCLOSURE_SUPPORT`, the XL only (`ProjectOptions.cmake:601`). Another Buddy printer: later (#87) |  |
| `end` |  |  | UNCHECKED |  |
| `event` | `render.cpp:854` | `devices/prusa_connect.rs:1032=event` | **identical**: read from where `render.cpp` writes it | `the_events_buddy_sends_settle_commands` |
| `extruder_fs_state` | `render.cpp:184` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `PRINTER_IS_PRUSA_iX`, the iX only. Another Buddy printer: later (#87) |  |
| `fan_1_rpm` | `render.cpp:206` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under the Core One and Core One L only. Another Buddy printer: later (#87) |  |
| `fan_2_rpm` | `render.cpp:207` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under the Core One and Core One L only. Another Buddy printer: later (#87) |  |
| `fan_pwm_target` | `render.cpp:208` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under the Core One and Core One L only. Another Buddy printer: later (#87) |  |
| `fan_rpm` | `render.cpp:197` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `XL_ENCLOSURE_SUPPORT`, the XL only (`ProjectOptions.cmake:601`). Another Buddy printer: later (#87) |  |
| `features` |  |  | UNCHECKED |  |
| `feeder` | `render.cpp:243` |  | **n/a**: not on the MINI: written only `#if HAS_MMU2()` (`render.cpp`), which only the MK4, MK3.5, Core One and Core One L have (`ProjectOptions.cmake:592`); an MMU is #211 |  |
| `filament_change_in` |  |  | UNCHECKED |  |
| `file` |  |  | UNCHECKED |  |
| `file_count` |  |  | UNCHECKED |  |
| `file_id` |  |  | UNCHECKED |  |
| `filter_lifetime` | `render.cpp:593` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `XL_ENCLOSURE_SUPPORT`, the XL only (`ProjectOptions.cmake:601`). Another Buddy printer: later (#87) |  |
| `filtration_filaments` | `render.cpp:594` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `XL_ENCLOSURE_SUPPORT`, the XL only (`ProjectOptions.cmake:601`). Another Buddy printer: later (#87) |  |
| `fingerprint` |  |  | UNCHECKED |  |
| `firmware` |  |  | UNCHECKED |  |
| `flow` |  |  | UNCHECKED |  |
| `free_space` |  |  | UNCHECKED |  |
| `hash` |  |  | UNCHECKED |  |
| `heatbreak_fan_rpm` |  |  | UNCHECKED |  |
| `heatbreak_temperature` | `render.cpp:561` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `HAS_FILAMENT_HEATBREAK_PARAM`, the iX only (`ProjectOptions.cmake:490`). Another Buddy printer: later (#87) |  |
| `hostname` |  |  | UNCHECKED |  |
| `id` |  |  | UNCHECKED |  |
| `is_sfn` |  |  | UNCHECKED |  |
| `job_id` | `render.cpp:163` | `devices/printer_files.rs:817=job_id` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `lan_ipv4` |  |  | UNCHECKED |  |
| `lan_mac` |  |  | UNCHECKED |  |
| `led_intensity` | `render.cpp:209` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under the Core One and Core One L only. Another Buddy printer: later (#87) |  |
| `m_timestamp` |  |  | UNCHECKED |  |
| `machine_reason` |  |  | UNCHECKED |  |
| `material` |  |  | UNCHECKED |  |
| `material_package_instance` |  |  | UNCHECKED |  |
| `max_bed_temperature` |  |  | UNCHECKED |  |
| `max_chamber_temperature` | `render.cpp:568` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `HAS_CHAMBER_API`, the XL and Core One family only (`ProjectOptions.cmake:595`). Another Buddy printer: later (#87) |  |
| `max_print_temperature` |  |  | UNCHECKED |  |
| `min_bed_temperature` |  |  | UNCHECKED |  |
| `min_chamber_temperature` | `render.cpp:565` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `HAS_CHAMBER_API`, the XL and Core One family only (`ProjectOptions.cmake:595`). Another Buddy printer: later (#87) |  |
| `min_print_temperature` |  |  | UNCHECKED |  |
| `mmu_command_in_progress` | `render.cpp:248` |  | **n/a**: not on the MINI: written only `#if HAS_MMU2()` (`render.cpp`), which only the MK4, MK3.5, Core One and Core One L have (`ProjectOptions.cmake:592`); an MMU is #211 |  |
| `mmu_progress_code` | `render.cpp:244` |  | **n/a**: not on the MINI: written only `#if HAS_MMU2()` (`render.cpp`), which only the MK4, MK3.5, Core One and Core One L have (`ProjectOptions.cmake:592`); an MMU is #211 |  |
| `model` | `render.cpp:465` |  | **n/a**: not on the MINI: written only `#if HAS_MMU2()` (`render.cpp`), which only the MK4, MK3.5, Core One and Core One L have (`ProjectOptions.cmake:592`); an MMU is #211 |  |
| `mountpoint` |  |  | UNCHECKED |  |
| `name` |  |  | UNCHECKED |  |
| `network_info` |  |  | UNCHECKED |  |
| `new_path` |  |  | UNCHECKED |  |
| `nominal_netto_full_weight` | `render.cpp:517` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `HAS_ANFC`, the Core One and Core One L only (`ProjectOptions.cmake:814`). Another Buddy printer: later (#87) |  |
| `nozzle_diameter` |  |  | UNCHECKED |  |
| `nozzle_hardened` |  |  | UNCHECKED |  |
| `nozzle_high_flow` |  |  | UNCHECKED |  |
| `nozzle_target_temp` |  |  | UNCHECKED |  |
| `nozzle_temp` |  |  | UNCHECKED |  |
| `objects` |  |  | UNCHECKED |  |
| `odometer_e` | `render.cpp:286` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `HAS_FILAMENT_TRACKER`, not the MINI (`ProjectOptions.cmake:641-648`). Another Buddy printer: later (#87) |  |
| `odometer_id` | `render.cpp:285` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `HAS_FILAMENT_TRACKER`, not the MINI (`ProjectOptions.cmake:641-648`). Another Buddy printer: later (#87) |  |
| `old_path` | `render.cpp:729` | `devices/printer_files.rs:833=old_path` | **identical**: read from where `render.cpp` writes it | `the_listing_follows_file_info_and_file_changed` |
| `package` |  |  | UNCHECKED |  |
| `path` |  |  | UNCHECKED |  |
| `post_print` | `render.cpp:591` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `XL_ENCLOSURE_SUPPORT`, the XL only (`ProjectOptions.cmake:601`). Another Buddy printer: later (#87) |  |
| `post_print_filtration_time` | `render.cpp:592` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `XL_ENCLOSURE_SUPPORT`, the XL only (`ProjectOptions.cmake:601`). Another Buddy printer: later (#87) |  |
| `preheat_temperature` |  |  | UNCHECKED |  |
| `preview` |  |  | UNCHECKED |  |
| `primary_color` |  |  | UNCHECKED |  |
| `print_fan_rpm` |  |  | UNCHECKED |  |
| `printer_type` |  |  | UNCHECKED |  |
| `printing_filtration` | `render.cpp:590` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `XL_ENCLOSURE_SUPPORT`, the XL only (`ProjectOptions.cmake:601`). Another Buddy printer: later (#87) |  |
| `progress` |  |  | UNCHECKED |  |
| `properties` |  |  | UNCHECKED |  |
| `read_only` |  |  | UNCHECKED |  |
| `reason` |  |  | UNCHECKED |  |
| `remote_fs_state` | `render.cpp:187` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `PRINTER_IS_PRUSA_iX`, the iX only. Another Buddy printer: later (#87) |  |
| `rescan` |  |  | UNCHECKED |  |
| `size` |  |  | UNCHECKED |  |
| `slot_count` | `render.cpp:467` |  | **n/a**: not on the MINI: written only `#if HAS_MMU2()` (`render.cpp`), which only the MK4, MK3.5, Core One and Core One L have (`ProjectOptions.cmake:592`); an MMU is #211 |  |
| `sn` |  |  | UNCHECKED |  |
| `speed` |  |  | UNCHECKED |  |
| `start` |  |  | UNCHECKED |  |
| `start_cmd_id` | `render.cpp:637` | `devices/printer_files.rs:869=start_cmd_id` | **identical**: read from where `render.cpp` writes it | `transfer_events_name_their_command_in_data` |
| `state` | `render.cpp:242` | `devices/printer_files.rs:752=state` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub`, `every_state_buddy_sends_is_read` |
| `storages` |  |  | UNCHECKED |  |
| `tags` |  |  | UNCHECKED |  |
| `target_bed` |  |  | UNCHECKED |  |
| `target_temp` | `render.cpp:205` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under the Core One and Core One L only. Another Buddy printer: later (#87) |  |
| `team_id` |  |  | UNCHECKED |  |
| `temp` | `render.cpp:196` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under the Core One and Core One L only; `XL_ENCLOSURE_SUPPORT`, the XL only (`ProjectOptions.cmake:601`). Another Buddy printer: later (#87) |  |
| `temp_ambient` | `render.cpp:182` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `PRINTER_IS_PRUSA_iX`, the iX only. Another Buddy printer: later (#87) |  |
| `temp_bed` |  |  | UNCHECKED |  |
| `temp_heatbreak` | `render.cpp:180` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `PRINTER_IS_PRUSA_iX`, the iX only. Another Buddy printer: later (#87) |  |
| `temp_psu` | `render.cpp:181` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `PRINTER_IS_PRUSA_iX`, the iX only. Another Buddy printer: later (#87) |  |
| `text` | `render.cpp:808` | `devices/printer_files.rs:761=text` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `time_in_use` | `render.cpp:198` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `XL_ENCLOSURE_SUPPORT`, the XL only (`ProjectOptions.cmake:601`). Another Buddy printer: later (#87) |  |
| `time_printing` |  |  | UNCHECKED |  |
| `time_remaining` |  |  | UNCHECKED |  |
| `time_transferring` |  |  | UNCHECKED |  |
| `title` | `render.cpp:798` | `devices/printer_files.rs:761=title` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `tool_count` |  |  | UNCHECKED |  |
| `tools` |  |  | UNCHECKED |  |
| `transfer` |  |  | UNCHECKED |  |
| `transfer_id` |  |  | UNCHECKED |  |
| `transfer_paused` |  |  | UNCHECKED |  |
| `transfer_progress` |  |  | UNCHECKED |  |
| `transfer_time_remaining` |  |  | UNCHECKED |  |
| `transfer_transferred` |  |  | UNCHECKED |  |
| `transferred` |  |  | UNCHECKED |  |
| `type` |  |  | UNCHECKED |  |
| `uuid` | `render.cpp:500` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `HAS_ANFC`, the Core One and Core One L only (`ProjectOptions.cmake:814`). Another Buddy printer: later (#87) |  |
| `wifi_ipv4` |  |  | UNCHECKED |  |
| `wifi_mac` |  |  | UNCHECKED |  |
| `wifi_ssid` |  |  | UNCHECKED |  |

## Values it sends (states, events, types)

| Item | Buddy 8.0.2 | Hub | Result | Test |
|---|---|---|---|---|
| `ACCEPTED` | `planner.cpp:269` | `devices/prusa_connect.rs:1034=ACCEPTED` | **identical**: a command with that id becomes accepted; the event has no `job_id` or `data` (`render.cpp:269`) | `the_events_buddy_sends_settle_commands` |
| `CANCELABLE_CHANGED` | `planner.cpp:291` | `devices/prusa_connect.rs:1042=CANCELABLE_CHANGED` | **identical**: the command (`CANCEL_OBJECT`, `UNCANCEL_OBJECT`, never sent by the hub) would be finished; the `objects` list is not read by the hub | `the_events_buddy_sends_settle_commands` |
| `DISABLED` | `render.cpp:82` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `PRINTER_IS_PRUSA_iX`, the iX only. Another Buddy printer: later (#87) |  |
| `FAILED` | `planner.cpp:281` | `devices/prusa_connect.rs:1046=FAILED` | **identical**: the command becomes rejected (a background G-code that failed, `planner.cpp:1112`) | `the_events_buddy_sends_settle_commands` |
| `FFF` |  |  | UNCHECKED |  |
| `FFF_FEEDER_SLOT` | `render.cpp:264` |  | **n/a**: not on the MINI: written only `#if HAS_MMU2()` (`render.cpp`), which only the MK4, MK3.5, Core One and Core One L have (`ProjectOptions.cmake:592`); an MMU is #211 |  |
| `FFF_PRINT_HEAD` |  |  | UNCHECKED |  |
| `FILE_CHANGED` | `planner.cpp:289` | `devices/printer_files.rs:828=FILE_CHANGED` | **identical**: the command (delete, make folder) is finished by it (`planner.cpp:850`); `old_path` takes the file out of the listing | `the_events_buddy_sends_settle_commands`, `the_listing_follows_file_info_and_file_changed` |
| `FILE_EXISTS` |  |  | UNCHECKED |  |
| `FILE_INFO` | `planner.cpp:275` | `devices/printer_files.rs:798=FILE_INFO` | **identical**: the command is finished; a folder's `children` and `path` fill the hub's listing of the stick | `the_events_buddy_sends_settle_commands`, `the_listing_follows_file_info_and_file_changed` |
| `FINISHED` | `planner.cpp:279` | `devices/prusa_connect.rs:1041=FINISHED` | **identical**: the command becomes finished | `the_events_buddy_sends_settle_commands` |
| `FIN_OK` |  |  | UNCHECKED |  |
| `FIN_STOPPED` |  |  | UNCHECKED |  |
| `FOLDER` |  |  | UNCHECKED |  |
| `HAS_FILAMENT` | `render.cpp:76` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `PRINTER_IS_PRUSA_iX`, the iX only. Another Buddy printer: later (#87) |  |
| `INFO` | `planner.cpp:267` | `devices/prusa_connect.rs:1194=INFO` | **identical**: the command is finished and the details are kept (serial, `printer_type` as the string `2.1.0`, firmware, `nozzle_diameter` at the top level, `tools.<n>.material` with `---` read as none, storages, network); keys only master writes are not read | `the_events_buddy_sends_settle_commands`, `info_says_a_dash_for_no_material`, `printer_type_is_read_as_buddys_string_only`, `info_of_6_3_0_is_read_as_buddy_writes_it` |
| `JOB_INFO` | `planner.cpp:273` | `devices/printer_files.rs:811=JOB_INFO` | **identical**: the command is finished; `job_id` at the top and `data.display_name` give the file a print is (as the hub's telemetry shows it) | `the_events_buddy_sends_settle_commands`, `state_changed_and_job_info_reach_the_hub` |
| `MMU` | `render.cpp:464` |  | **n/a**: not on the MINI: written only `#if HAS_MMU2()` (`render.cpp`), which only the MK4, MK3.5, Core One and Core One L have (`ProjectOptions.cmake:592`); an MMU is #211 |  |
| `MMU3` | `render.cpp:465` |  | **n/a**: not on the MINI: written only `#if HAS_MMU2()` (`render.cpp`), which only the MK4, MK3.5, Core One and Core One L have (`ProjectOptions.cmake:592`); an MMU is #211 |  |
| `NETWORK_FAILURE` |  |  | UNCHECKED |  |
| `NOT_CALIBRATED` | `render.cpp:74` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `PRINTER_IS_PRUSA_iX`, the iX only. Another Buddy printer: later (#87) |  |
| `NOT_CONNECTED` | `render.cpp:80` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `PRINTER_IS_PRUSA_iX`, the iX only. Another Buddy printer: later (#87) |  |
| `NOT_INITIALIZED` | `render.cpp:72` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `PRINTER_IS_PRUSA_iX`, the iX only. Another Buddy printer: later (#87) |  |
| `NOT_READY` |  |  | UNCHECKED |  |
| `NO_FILAMENT` | `render.cpp:78` |  | **n/a**: not on the MINI: Buddy 8.0.2 writes it only under `PRINTER_IS_PRUSA_iX`, the iX only. Another Buddy printer: later (#87) |  |
| `NO_TRANSFER` |  |  | UNCHECKED |  |
| `PAUSED` | `render.cpp:622` | `devices/prusa_connect.rs:651=PAUSED` | **identical**: read as a device state (see States) | `every_state_buddy_sends_is_read` |
| `PRINTING` | `render.cpp:620` | `devices/prusa_connect.rs:650=PRINTING` | **identical**: read as a device state (see States) | `every_state_buddy_sends_is_read` |
| `REJECTED` | `planner.cpp:271` | `devices/prusa_connect.rs:1046=REJECTED` | **identical**: the command becomes rejected; `reason` and `machine_reason` (`render.cpp:327`, `:331`) are kept as the reason. The hub reads `machine_reason` `FILE_EXISTS` to send a file again under a free name, not the text | `the_events_buddy_sends_settle_commands`, `a_free_name_is_decided_by_machine_reason` |
| `STATE_CHANGED` | `planner.cpp:293` | `devices/printer_files.rs:751=STATE_CHANGED` | **identical**: `data.code`, `title`, `text`, `buttons` and the top-level `dialog_id` give what the screen asks (and the buttons for `DIALOG_ACTION`); a state without a dialog clears it | `the_events_buddy_sends_settle_commands`, `state_changed_and_job_info_reach_the_hub` |
| `STORAGE_FAILURE` |  |  | UNCHECKED |  |
| `TRANSFER_ABORTED` | `planner.cpp:285` | `devices/printer_files.rs:868=TRANSFER_ABORTED` | **OPEN**: as `TRANSFER_STOPPED`; the `machine_reason` (`STORAGE_FAILURE`, `NETWORK_FAILURE`) wording is not run by a test here |  |
| `TRANSFER_FINISHED` | `planner.cpp:287` | `devices/printer_files.rs:868=TRANSFER_FINISHED` | **identical**: no top-level `command_id` (`render.cpp:537`); the hub settles the download command by `data.start_cmd_id` and marks the transfer finished | `transfer_events_name_their_command_in_data` |
| `TRANSFER_INFO` | `planner.cpp:277` | `devices/printer_files.rs:849=TRANSFER_INFO` | **identical**: the download command is finished (taken, the file on its way) | `the_events_buddy_sends_settle_commands`, `transfer_events_name_their_command_in_data` |
| `TRANSFER_IN_PROGRESS` |  |  | UNCHECKED |  |
| `TRANSFER_STOPPED` | `planner.cpp:283` | `devices/printer_files.rs:868=TRANSFER_STOPPED` | **OPEN**: no top-level `command_id` (`render.cpp:537`); the hub takes the command from `data.start_cmd_id`; the stopped branch (failed transfer, job failed) is not run by a test here |  |
| `USB` |  |  | UNCHECKED |  |

## States it sends (`state/printer_state.cpp` `to_str(DeviceState)`)

| Item | Buddy 8.0.2 | Hub | Result | Test |
|---|---|---|---|---|
| `IDLE` | `printer_state.cpp:630` | `devices/prusa_connect.rs:404=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `PRINTING` | `printer_state.cpp:632` | `devices/prusa_connect.rs:650=PRINTING` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `PAUSED` | `printer_state.cpp:634` | `devices/prusa_connect.rs:651=PAUSED` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `FINISHED` | `printer_state.cpp:636` | `devices/prusa_connect.rs:404=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `STOPPED` | `printer_state.cpp:638` | `devices/prusa_connect.rs:404=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `READY` | `printer_state.cpp:640` | `devices/prusa_connect.rs:404=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `ERROR` | `printer_state.cpp:642` | `devices/prusa_connect.rs:653=ERROR` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `BUSY` | `printer_state.cpp:644` | `devices/prusa_connect.rs:650=BUSY` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `ATTENTION` | `printer_state.cpp:646` | `devices/prusa_connect.rs:652=ATTENTION` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `UNKNOWN` | `printer_state.cpp:649` | `devices/prusa_connect.rs:404=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |

## G-code metadata it passes on or leaves out (`render.cpp`)

| Item | Buddy 8.0.2 | Hub | Result | Test |
|---|---|---|---|---|
| `bed_temperature (Int)` |  |  | UNCHECKED |  |
| `brim_width (Int)` |  |  | UNCHECKED |  |
| `display_name (Ignore)` |  |  | UNCHECKED |  |
| `estimated_print_time (Int)` |  |  | UNCHECKED |  |
| `filament cost (Float)` |  |  | UNCHECKED |  |
| `filament used [cm3] (Float)` |  |  | UNCHECKED |  |
| `filament used [m] (Float)` |  |  | UNCHECKED |  |
| `filament used [mm3] (Float)` |  |  | **n/a**: sent as a number; the hub shows none of it, and reads the keys it shows as text or number alike |  |
| `filament used [mm] (Float)` |  |  | UNCHECKED |  |
| `ironing (Int)` |  |  | UNCHECKED |  |
| `layer_height (Float)` |  |  | UNCHECKED |  |
| `m_timestamp (Ignore)` |  |  | UNCHECKED |  |
| `max_layer_z (Float)` |  |  | UNCHECKED |  |
| `path (Ignore)` |  |  | UNCHECKED |  |
| `preview (Ignore)` |  |  | UNCHECKED |  |
| `read_only (Ignore)` |  |  | UNCHECKED |  |
| `size (Ignore)` |  |  | UNCHECKED |  |
| `support_material (Int)` |  |  | UNCHECKED |  |
| `temperature (Int)` |  |  | UNCHECKED |  |
| `total filament used for wipe tower [g] (Float)` |  |  | UNCHECKED |  |
| `type (Ignore)` |  |  | UNCHECKED |  |

## Answers a dialog takes (`common/general_response.cpp`)

| Item | Buddy 8.0.2 | Hub | Result | Test |
|---|---|---|---|---|
| `Abort` | `general_response.cpp:11` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Abort_invalidate_test` | `general_response.cpp:12` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Adjust` | `general_response.cpp:13` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `All` | `general_response.cpp:14` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Always` | `general_response.cpp:15` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Back` | `general_response.cpp:16` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Calibrate` | `general_response.cpp:17` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Cancel` | `general_response.cpp:18` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Change` | `general_response.cpp:20` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Continue` | `general_response.cpp:21` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Cooldown` | `general_response.cpp:22` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Details` | `general_response.cpp:23` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Disable` | `general_response.cpp:24` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Docks4` | `general_response.cpp:57` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Docks8` | `general_response.cpp:58` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Done` | `general_response.cpp:25` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Empty` | `general_response.cpp:60` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `FS_disable` | `general_response.cpp:27` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Finish` | `general_response.cpp:26` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Heatup` | `general_response.cpp:55` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Help` | `general_response.cpp:19` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Ignore` | `general_response.cpp:28` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Left` | `general_response.cpp:29` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Load` | `general_response.cpp:30` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `MMU_disable` | `general_response.cpp:31` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Never` | `general_response.cpp:32` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Next` | `general_response.cpp:33` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `No` | `general_response.cpp:34` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `NotNow` | `general_response.cpp:35` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Ok` | `general_response.cpp:36` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Other` | `general_response.cpp:59` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Pause` | `general_response.cpp:37` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Postpone5Days` | `general_response.cpp:56` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Print` | `general_response.cpp:38` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Purge_more` | `general_response.cpp:39` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Quit` | `general_response.cpp:40` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Reheat` | `general_response.cpp:41` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Remap` | `general_response.cpp:43` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Remove` | `general_response.cpp:44` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Replace` | `general_response.cpp:42` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Restart` | `general_response.cpp:45` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Resume` | `general_response.cpp:46` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Retry` | `general_response.cpp:47` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Right` | `general_response.cpp:48` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Skip` | `general_response.cpp:49` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Slowly` | `general_response.cpp:50` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `SpoolJoin` | `general_response.cpp:51` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Stop` | `general_response.cpp:52` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Unload` | `general_response.cpp:53` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |
| `Yes` | `general_response.cpp:54` | `web/machines.rs:506=button` | **identical**: sent back by the name the printer gave (`render.cpp:836`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_8_0_2_goes_back_unchanged` |

## Differences (each decided by the owner)

| # | Buddy 6.3.0 | Ours | What differs | Proposed fix |
|---|---|---|---|---|
| D6 | Nothing in Connect's commands closes the print preview that opens after a download (`transfers/transfer.cpp:423-430`, `print_begin` shows the preview, state `PrintPreviewImage`, which `printer_state.cpp:267-272` reports as IDLE or READY). `STOP_PRINT` (`marlin_printer.cpp:454-457`) is accepted only for Paused, Printing or Attention, and answers REJECTED "No print to stop" (`planner.cpp:687`); `SET_IDLE` (`marlin_printer.cpp:577-584`) only for Finished or Stopped; `DIALOG_ACTION` needs a `dialog_id`, which `render.cpp:255` writes only while the state has a dialog (`printer_state.cpp:355-367`: the preview itself has none; only its questions have). The preview is closed by Buddy's own screen (`print_abort` in a preview state, `marlin_server.cpp:1244-1251`), and `M604` reaches the same call (`src/marlin_stubs/host/M604.cpp:17`) | `devices/printer_files.rs:1056` | After a download that is not printed the hub sends `M604` as G-code to close the preview (its own comment: "WORKAROUND, NEEDS TRACING"). Buddy's Connect offers no command for it; `M604` is G-code, not a Connect command | **Ours, approved by the owner (2026-11-17, "D6 A")**: kept: after a download only to the stick, while the printer reports IDLE and nothing else waits, the hub sends `M604` (Buddy's abort, which in the preview acts as its Back button; tried on the MINI 2026-11-08). Buddy 6.3.0 gives Connect no other way |

## What we send or read that the firmware does not

| Item | Where | Why | Status |
|---|---|---|---|
| `/p/`, `/f/` | `prusa_connect.rs` | The hub's own route prefixes (`starts_with`, which requests to log), not endpoints | n/a |
| `Content-Type` | `prusa_connect.rs` | The header whose value the firmware reads (`ApplicationJson`, `TextGcodeDot`, `TextGcodeDash`) | see What it reads in an answer |
| `JOG`, `SET_ORIGIN` | `commands.rs` `laser_command` | A laser on the agent: our SDK extension, part 4 | not for Buddy: `agent_names_never_go_to_a_buddy` |
| `laser_mode`, `max_power`, `travel_x`, `travel_y`, `travel_z` | `devices/prusa_connect.rs` `grbl_settings` | Read from a laser's `INFO` on the agent: part 4 (`$32`, `$30`, `$130`-`$132`) | not for Buddy: read only from a laser |
| `media_loaded`, `battery_percent`, `raw_status` | `devices/prusa_connect.rs` `take_in` | Read from a DYMO label printer's telemetry on the agent: parts 2 and 3 | not for Buddy: read only from a label printer |
| `gcode_output`, `firmware_info`, `firmware_capabilities` | `devices/prusa_connect.rs` | Read from a Marlin printer (and `firmware_info` from a laser) on the agent: part 9 (part 4) | not for Buddy: read only from the agent |
| `agent_id`, `extensions` | `devices/prusa_connect.rs` `to_details` | Read from every device on the agent: part 6 | not for Buddy: a Buddy printer sends neither |
| `printing`, `/p/teams/<team>/files/<hash>/raw` | `devices/printer_files.rs` `start_transfer`, `devices/prusa_connect.rs` | The Connect SDK's own download (`START_CONNECT_DOWNLOAD`, Prusa-Connect-SDK-Printer `__init__.py:448-473`), sent only to a Marlin printer on the agent; not ours, not Buddy's | not for Buddy: `agent_names_never_go_to_a_buddy` |
| `slot`, `mmu`, `temp_nozzle`, `target_nozzle`, and a slot's `temp`, `fan_print`, `fan_hotend` | `devices/connect_tools.rs` | Firmware before Buddy 8.0.2 sends its tools this way (a `slot` block, the nozzle at the top of the frame, `mmu` in INFO); 8.0.2 does not. The hub reads them as Prusa Connect's server still does (`reference/prusa-connect` `internal/connector/xbuddy/telemetrytools.go:118`, `:160`, `:188`, `tools.go:98`; read only, OCL) | not sent by Buddy 8.0.2: harmless |

## Not covered

- The WebSocket (`/p/ws`) and `START_INLINE_DOWNLOAD`, the download Buddy 8.0.2 has on the MINI (#212);
  `START_CONNECT_DOWNLOAD`.
- The encrypted download (`/f/<iv>/raw`): Range, 200/206, `Content-Encryption-Mode`, the decryptor of `transfers/download.cpp`.
- The telemetry's fields and the INFO event's data beyond what the tests above read; G-code metadata; the
  transfer events' failure branches (`TRANSFER_STOPPED`, `TRANSFER_ABORTED`).
- The G-code the hub sends as `text/x.gcode` (the MINI's menu G-code is HS10).
- Still Buddy 6.3.0's, brought to 8.0.2 in #201: the line numbers in the Result text, in the Differences
  section (D6 was decided against 6.3.0) and in `buddy_audit.rs`'s comments, and the test's port of
  `parse_json_command` (8.0.2 adds the path limit and changes the tool mapping, `command.cpp`).
- Buddy's C++ is not run: every "identical" is against a port of its parser and fixtures written from its
  serializer, not against its own output. TLS, HTTP connection reuse and timing of the printer.
