# Buddy firmware audit (Prusa Connect, the hub's side)

Reference: **Prusa-Firmware-Buddy 6.3.0** (`reference/buddy-6.3.0`, commit `18b4c3c`), the firmware of the
MINI here: `src/connect` (the Connect client), `src/common/http`, `src/common/general_response.cpp`,
`src/state/printer_state.cpp`. Master (`reference/Prusa-Firmware-Buddy`, 6.4+, the WebSocket) is **not**
a reference here: items only in master are marked **not in 6.3.0**. Ours: the hub as Prusa Connect's server,
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
reads; not built), **not in 6.3.0** (only in master), **n/a**, **OPEN** (not proven), or UNCHECKED (not looked at
in this step). A fixture written by reading is not the firmware's own output: where that is all there is, the
row says OPEN, not identical.

## Commands it takes from Connect (`command.cpp`)

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `CANCEL_OBJECT` | both | `command.cpp:182` |  | **not used by the hub** |  |
| `CANCEL_PRINTER_READY` | both | `command.cpp:163` |  | **not used by the hub** |  |
| `CREATE_FOLDER` | both | `command.cpp:167` | `devices/printer_files.rs:1305` | **identical**: taken as `CreateFolder` with `path` | `buddy_takes_every_command_the_hub_sends` |
| `DELETE_FILE` | both | `command.cpp:165` | `devices/printer_files.rs:1273` | **identical**: taken as `DeleteFile` with `path` | `buddy_takes_every_command_the_hub_sends` |
| `DELETE_FOLDER` | both | `command.cpp:166` | `devices/printer_files.rs:1271` | **identical**: taken as `DeleteFolder` with `path` | `buddy_takes_every_command_the_hub_sends` |
| `DIALOG_ACTION` | both | `command.cpp:180` | `web/machines.rs:500` | **differs** D1: taken as `DialogAction` with `dialog_id` and `button` when the printer named the dialog; without a `dialog_id` the hub sends `button` alone and Buddy refuses it | `buddy_takes_every_command_the_hub_sends`, `a_dialog_answer_without_a_dialog_is_refused_by_buddy` |
| `PAUSE_PRINT` | both | `command.cpp:158` | `devices/printer_files.rs:964` | **identical**: taken as `PausePrint` | `buddy_takes_every_command_the_hub_sends` |
| `RESET` | both | `command.cpp:178` |  | **not used by the hub** |  |
| `RESET_PRINTER` | both | `command.cpp:177` |  | **not used by the hub** |  |
| `RESUME_PRINT` | both | `command.cpp:160` | `devices/printer_files.rs:965` | **identical**: taken as `ResumePrint` | `buddy_takes_every_command_the_hub_sends` |
| `SEND_FILE_INFO` | both | `command.cpp:156` | `devices/printer_files.rs:802` `devices/printer_files.rs:1052` | **identical**: the body Buddy's port takes as `SendFileInfo` with its `path` | `buddy_takes_every_command_the_hub_sends` |
| `SEND_INFO` | both | `command.cpp:154` |  | **not used by the hub** |  |
| `SEND_JOB_INFO` | both | `command.cpp:155` | `devices/printer_files.rs:857` | **identical**: taken as `SendJobInfo` with `job_id` | `buddy_takes_every_command_the_hub_sends` |
| `SEND_STATE_INFO` | both | `command.cpp:179` |  | **not used by the hub** |  |
| `SEND_TRANSFER_INFO` | both | `command.cpp:157` |  | **not used by the hub** |  |
| `SET_IDLE` | both | `command.cpp:164` |  | **not used by the hub** |  |
| `SET_PRINTER_READY` | both | `command.cpp:162` | `farm.rs:1562` | **identical**: the literal body, taken as `SetPrinterReady` | `buddy_takes_set_printer_ready` |
| `SET_TOKEN` | both | `command.cpp:169` |  | **not used by the hub** |  |
| `SET_VALUE` | both | `command.cpp:181` |  | **not used by the hub** |  |
| `START_CONNECT_DOWNLOAD` | both | `command.cpp:191` |  | **not used by the hub** |  |
| `START_ENCRYPTED_DOWNLOAD` | both | `command.cpp:192` | `devices/printer_files.rs:450` | **identical**: taken as `StartEncryptedDownload` with `path`, `key`, `iv`, `orig_size` (32 hex digits each), no `port` | `buddy_takes_every_command_the_hub_sends` |
| `START_INLINE_DOWNLOAD` | both | `command.cpp:184` |  | **not used by the hub** |  |
| `START_PRINT` | both | `command.cpp:161` | `devices/printer_files.rs:343` `devices/printer_files.rs:1223` | **identical**: taken as `StartPrint` with `path` (no `tool_mapping`: optional) | `buddy_takes_every_command_the_hub_sends` |
| `STOP_PRINT` | both | `command.cpp:159` | `devices/printer_files.rs:966` | **identical**: taken as `StopPrint` | `buddy_takes_every_command_the_hub_sends` |
| `STOP_TRANSFER` | both | `command.cpp:168` |  | **not used by the hub** |  |
| `UNCANCEL_OBJECT` | both | `command.cpp:183` |  | **not used by the hub** |  |

## Arguments it reads from a command (`command.cpp`)

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `addon_power` | both | `command.cpp:410` |  | **not used by the hub** |  |
| `button` | both | `command.cpp:323` | `web/machines.rs:496` | **identical**: one of the printer's own names (see Answers a dialog takes) | `buddy_takes_every_command_the_hub_sends` |
| `chamber.fan_pwm_target` | both | `command.cpp:408` |  | **not used by the hub** |  |
| `chamber.led_intensity` | both | `command.cpp:414` |  | **not used by the hub** |  |
| `chamber.target_temp` | both | `command.cpp:406` |  | **not used by the hub** |  |
| `command` | both | `command.cpp:147` | `devices/printer_files.rs:1185` | **identical**: in every body, a string at depth 1 (`:147`) | `buddy_takes_every_command_the_hub_sends` |
| `dialog_id` | both | `command.cpp:400` | `web/machines.rs:498` | **differs** D1: a number when the printer's `STATE_CHANGED` named the dialog; left out when it did not | `buddy_takes_every_command_the_hub_sends`, `a_dialog_answer_without_a_dialog_is_refused_by_buddy` |
| `enclosure_enabled` | both | `command.cpp:346` |  | **not used by the hub** |  |
| `enclosure_postprint` | both | `command.cpp:350` |  | **not used by the hub** |  |
| `enclosure_postprint_filtration_time` | both | `command.cpp:352` |  | **not used by the hub** |  |
| `enclosure_printing_filtration` | both | `command.cpp:348` |  | **not used by the hub** |  |
| `hash` | both | `command.cpp:390` |  | **not used by the hub** |  |
| `hostname` | both | `command.cpp:332` |  | **not used by the hub** |  |
| `id` | both | `command.cpp:402` |  | **not used by the hub** |  |
| `iv` | both | `command.cpp:398` | `devices/printer_files.rs:454` | **identical**: 32 hex digits | `buddy_takes_every_command_the_hub_sends` |
| `job_id` | both | `command.cpp:269` | `devices/printer_files.rs:857` | **identical**: a number that fits 16 bits (it is the printer's own `job_id`) | `buddy_takes_every_command_the_hub_sends` |
| `key` | both | `command.cpp:396` | `devices/printer_files.rs:453` | **identical**: 32 hex digits | `buddy_takes_every_command_the_hub_sends` |
| `kwargs` | both | `command.cpp:265` | `devices/printer_files.rs:1185` | **identical**: an object at depth 1 (`:265`) | `buddy_takes_every_command_the_hub_sends` |
| `orig_size` | both | `command.cpp:385` | `devices/printer_files.rs:455` | **identical**: a number that fits 32 bits | `buddy_takes_every_command_the_hub_sends` |
| `path` | both | `command.cpp:271` | `devices/printer_files.rs:1185` | **identical**: a string; short names of the stick (`path_sfn` is the same argument, `:271`) | `buddy_takes_every_command_the_hub_sends` |
| `path_sfn` | both | `command.cpp:271` |  | **not used by the hub** |  |
| `port` | both | `command.cpp:383` |  | **not used by the hub** |  |
| `team_id` | both | `command.cpp:388` |  | **not used by the hub** |  |
| `token` | both | `command.cpp:311` |  | **not used by the hub** |  |
| `tool_mapping` | both | `command.cpp:280` |  | **not used by the hub** |  |
| `tools.<n>.hardened` | both | `command.cpp:369` |  | **not used by the hub** |  |
| `tools.<n>.high_flow` | both | `command.cpp:366` |  | **not used by the hub** |  |
| `tools.<n>.nozzle_diameter` | both | `command.cpp:372` |  | **not used by the hub** |  |

## Events it sends (`planner.cpp` `to_str`, shaped by `render.cpp` `render_msg`)

Fourteen types, the whole list of `planner.cpp:253-280`. Common shape (`render.cpp:321-683`): `job_id` (when a job runs and the type is not `ACCEPTED`/`REJECTED`), `reason`, `machine_reason`, `data`, `dialog_id`, `state`, `command_id`, `transfer_id`, `event` last.

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `ACCEPTED` | both | `planner.cpp:256` | `devices/prusa_connect.rs:519=ACCEPTED` | **identical**: a command with that id becomes accepted; the event has no `job_id` or `data` (`render.cpp:269`) | `the_events_buddy_sends_settle_commands` |
| `REJECTED` | both | `planner.cpp:258` | `devices/prusa_connect.rs:531=REJECTED` | **identical**: the command becomes rejected; `reason` and `machine_reason` (`render.cpp:327`, `:331`) are kept as the reason. The hub reads the text (D9) | `the_events_buddy_sends_settle_commands` |
| `FINISHED` | both | `planner.cpp:266` | `devices/prusa_connect.rs:526=FINISHED` | **identical**: the command becomes finished | `the_events_buddy_sends_settle_commands` |
| `FAILED` | both | `planner.cpp:268` | `devices/prusa_connect.rs:531=FAILED` | **identical**: the command becomes rejected (a background G-code that failed, `planner.cpp:1112`) | `the_events_buddy_sends_settle_commands` |
| `INFO` | both | `planner.cpp:254` | `devices/prusa_connect.rs:547=INFO` | **differs** D2 (D4, D5): the command is finished and the details are kept (serial, type, firmware, nozzle, storages, network); `tools.<n>.material` says `---` for none and the hub keeps it as the material | `the_events_buddy_sends_settle_commands`, `info_says_a_dash_for_no_material` |
| `JOB_INFO` | both | `planner.cpp:260` | `devices/printer_files.rs:633=JOB_INFO` | **identical**: the command is finished; `job_id` at the top and `data.display_name` give the file a print is (as the hub's telemetry shows it) | `the_events_buddy_sends_settle_commands`, `state_changed_and_job_info_reach_the_hub` |
| `FILE_INFO` | both | `planner.cpp:262` | `devices/printer_files.rs:616=FILE_INFO` | **identical**: the command is finished; a folder's `children` and `path` fill the hub's listing of the stick | `the_events_buddy_sends_settle_commands`, `the_listing_follows_file_info_and_file_changed` |
| `TRANSFER_INFO` | both | `planner.cpp:264` | `devices/printer_files.rs:665=TRANSFER_INFO` | **identical**: the download command is finished (taken, the file on its way) | `the_events_buddy_sends_settle_commands`, `transfer_events_name_their_command_in_data` |
| `TRANSFER_STOPPED` | both | `planner.cpp:270` | `devices/printer_files.rs:682=TRANSFER_STOPPED` | **OPEN**: no top-level `command_id` (`render.cpp:537`); the hub takes the command from `data.start_cmd_id`; the stopped branch (failed transfer, job failed) is not run by a test here |  |
| `TRANSFER_ABORTED` | both | `planner.cpp:272` | `devices/printer_files.rs:682=TRANSFER_ABORTED` | **OPEN**: as `TRANSFER_STOPPED`; the `machine_reason` (`STORAGE_FAILURE`, `NETWORK_FAILURE`) wording is not run by a test here |  |
| `TRANSFER_FINISHED` | both | `planner.cpp:274` | `devices/printer_files.rs:682=TRANSFER_FINISHED` | **identical**: no top-level `command_id` (`render.cpp:537`); the hub settles the download command by `data.start_cmd_id` and marks the transfer finished | `transfer_events_name_their_command_in_data` |
| `FILE_CHANGED` | both | `planner.cpp:276` | `devices/printer_files.rs:648=FILE_CHANGED` | **identical**: the command (delete, make folder) is finished by it (`planner.cpp:850`); `old_path` takes the file out of the listing | `the_events_buddy_sends_settle_commands`, `the_listing_follows_file_info_and_file_changed` |
| `CANCELABLE_CHANGED` | both | `planner.cpp:278` | `devices/prusa_connect.rs:527=CANCELABLE_CHANGED` | **identical**: the command (`CANCEL_OBJECT`, `UNCANCEL_OBJECT`, never sent by the hub) would be finished; the `objects` list is not read by the hub | `the_events_buddy_sends_settle_commands` |
| `STATE_CHANGED` | both | `planner.cpp:280` | `devices/printer_files.rs:574=STATE_CHANGED` | **identical**: `data.code`, `title`, `text`, `buttons` and the top-level `dialog_id` give what the screen asks (and the buttons for `DIALOG_ACTION`); a state without a dialog clears it | `the_events_buddy_sends_settle_commands`, `state_changed_and_job_info_reach_the_hub` |

## What it does with the server's answer, and its registration

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| status `204` to a report or event |  | `connect.cpp:726=NoContent` | `devices/prusa_connect.rs:501=NO_CONTENT` | **identical**: nothing to tell; the printer goes on | `the_answer_to_a_report_is_what_buddy_reads` |
| status `200` with a command |  | `connect.cpp:730=Status::Ok` | `devices/prusa_connect.rs:492=StatusCode::OK` | **identical**: `Command-Id` as decimal digits (`command_id.cpp`), a content type Buddy knows, a body of at most 512 bytes | `the_answer_to_a_report_is_what_buddy_reads`, `buddy_takes_every_command_the_hub_sends` |
| `Command-Id` header |  | `resp_parser.cpp:51=CommandId` | `devices/prusa_connect.rs:494=Command-Id` | **identical**: the command's number, as digits | `the_answer_to_a_report_is_what_buddy_reads` |
| `Content-Type: application/json` command |  | `resp_parser.cpp:42=ApplicationJson` | `devices/prusa_connect.rs:487=application/json` | **identical**: a JSON command parsed by `parse_json_command` | `buddy_takes_every_command_the_hub_sends` |
| `Content-Type: text/x.gcode` command |  | `resp_parser.cpp:45=TextGcodeDot` | `devices/prusa_connect.rs:489=text/x.gcode` | **identical**: G-code lines; `text/x-gcode` is the same to Buddy (`:48`) | `the_answer_to_a_report_is_what_buddy_reads` |
| a command body over 512 bytes |  | `connect.cpp:203=512` | `commands.rs:216=512` | **OPEN**: JSON commands the hub builds are far smaller (tested); the G-code of the hub's typed commands is limited to 480 characters on the hub's side, a limit that is the hub's UI. A G-code body of the MINI mapping (HS10) is not covered | `buddy_takes_every_command_the_hub_sends` |
| one command at a time |  | `planner.cpp:661=Processing other command` | `store/commands.rs:280=next_command_of_any_kind` | **differs** D8: the hub sends one and waits for its answer, but gives up after 120 s with no answer and sends the next, while Buddy may still run the first (it then answers REJECTED "Processing other command") | `the_answer_to_a_report_is_what_buddy_reads`, `the_hub_sends_the_next_command_after_120_seconds` |
| status `401` to an unknown token |  | `connect.cpp:631=Unauthorized` | `devices/prusa_connect.rs:504=UNAUTHORIZED` | **identical**: Buddy reads it as an authentication error and refuses the report | `the_answer_to_a_report_is_what_buddy_reads` |
| a command in the answer to `/p/events` |  | `connect.cpp:697=send_command` | `devices/prusa_connect.rs:509=fn event` | **identical**: Buddy takes a command from any `200` answer; the hub answers events with `204` and sends commands in the answer to telemetry only | `the_events_buddy_sends_settle_commands` |
| `POST /p/register`: body |  | `registrator.cpp:61=sn` | `devices/prusa_connect.rs:144=register` | **identical**: `sn`, `fingerprint`, `printer_type` as the string `2.1.0`, `firmware`, in that order, read; the hub answers `200` with a `Code` header of at most 25 characters | `registration_is_what_registrator_cpp_does` |
| `GET /p/register`: poll |  | `registrator.cpp:219=Accepted` | `devices/prusa_connect.rs:203=ACCEPTED` | **differs** D10: `202` until added, then `200` with `Token` (at most 20 characters, `printer.hpp:190`); an unknown or already given code gets `404`, which the printer reads as a server error and polls again for ever | `registration_is_what_registrator_cpp_does`, `a_mini_registers_by_itself` |
| raw G-code as a command |  | `command.cpp:90=gcode_command` | `devices/prusa_connect.rs:489=text/x.gcode` | **OPEN**: the transport is read as Buddy reads it; the G-code itself (what the hub sends for jog, heat, home; HS10) is not part of this step |  |

## Endpoints it calls

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `/f/<iv>/raw` | both | `planner.cpp:177=/raw` | `devices/printer_files.rs:989=/f/{iv}/raw` | **OPEN**: the download (Range, `Content-Encryption-Mode: AES-CTR`, decryption, 200/206) is not run against `transfers/download.cpp` in this step |  |
| `/p/events` | both | `connect.cpp:124` | `devices/prusa_connect.rs:103=/p/events` | **identical**: `POST`, `204` | `the_events_buddy_sends_settle_commands` |
| `/p/register` | both | `registrator.cpp:42` | `devices/prusa_connect.rs:101=/p/register` | **identical** | `registration_is_what_registrator_cpp_does` |
| `/p/telemetry` | both | `connect.cpp:121` | `devices/prusa_connect.rs:102=/p/telemetry` | **identical**: `POST`, the answer as described above | `the_answer_to_a_report_is_what_buddy_reads` |
| `/p/ws` | both | `connect.cpp:178` |  | **not used by the hub**: the WebSocket (6.4+; 6.3.0 never asks for it) is later |  |

## Headers it sends

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `Code` | both | `registrator.cpp:107` | `devices/prusa_connect.rs:200=code` | **identical**: read from `GET /p/register` | `registration_is_what_registrator_cpp_does` |
| `Fingerprint` | both |  |  | **not used by the hub**: sent by the printer; the hub reads no `Fingerprint` |  |
| `Sec-WebSocket-Key` | both |  |  | **not used by the hub**: WebSocket (6.4+) |  |
| `Sec-WebSocket-Protocol` | both |  |  | **not used by the hub**: WebSocket (6.4+) |  |
| `Sec-WebSocket-Version` | both |  |  | **not used by the hub**: WebSocket (6.4+) |  |
| `Token` | both | `connect.cpp:138` | `devices/prusa_connect.rs:437=token` | **identical**: names the machine on every report and event | `buddy_takes_every_command_the_hub_sends` |
| `Upgrade` | both |  |  | **not used by the hub**: WebSocket (6.4+) |  |
| `User-Agent-Printer` | 6.3.0 only |  |  | **not used by the hub**: sent by the printer; the hub reads none |  |
| `User-Agent-Version` | 6.3.0 only |  | `devices/prusa_connect.rs:441=user-agent-version` | **OPEN**: read as the firmware's version; the value `6.3.0+0.LOCAL` is from an earlier run on the MINI, not from the firmware's source |  |

## What it reads in an answer (`common/http/resp_parser.cpp`)

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `ApplicationJson` | both | `resp_parser.cpp:42` | `devices/prusa_connect.rs:487=application/json` | **identical**: the hub's JSON command says `application/json` | `buddy_takes_every_command_the_hub_sends` |
| `Body` | both | `resp_parser.cpp:32` |  | **n/a**: the automaton's own state for the body |  |
| `Code` | both | `resp_parser.cpp:54` | `devices/prusa_connect.rs:189=Code` | **identical**: `Code` of the registration answer | `registration_is_what_registrator_cpp_does` |
| `CommandId` | both | `resp_parser.cpp:51` | `devices/prusa_connect.rs:494=Command-Id` | **identical**: `Command-Id` | `the_answer_to_a_report_is_what_buddy_reads` |
| `ConnectionHeader` | both | `resp_parser.cpp:72` |  | **OPEN**: set by the HTTP server (axum); not read against the parser here |  |
| `ConnectionKeepAlive` | both | `resp_parser.cpp:76` |  | **OPEN**: as `ConnectionHeader` |  |
| `ConnectionUpgrade` | both | `resp_parser.cpp:69` |  | **not used by the hub**: WebSocket (6.4+) |  |
| `ContentEncryptionModeCBC` | both | `resp_parser.cpp:79` |  | **not used by the hub**: the hub says `AES-CTR` |  |
| `ContentEncryptionModeCTR` | both | `resp_parser.cpp:82` | `devices/printer_files.rs:1459=content-encryption-mode` | **OPEN**: the hub's download says `AES-CTR`; the download is not run against Buddy here |  |
| `ContentLength` | both | `resp_parser.cpp:36` |  | **OPEN**: set by the HTTP server for answers and by the download; not read against the parser here |  |
| `StatusCode` | both | `resp_parser.cpp:29` |  | **OPEN**: the codes the hub answers are listed above |  |
| `TextGcodeDash` | both | `resp_parser.cpp:48` |  | **not used by the hub**: the hub says `text/x.gcode` |  |
| `TextGcodeDot` | both | `resp_parser.cpp:45` | `devices/prusa_connect.rs:489=text/x.gcode` | **identical**: the hub's G-code command says `text/x.gcode` | `the_answer_to_a_report_is_what_buddy_reads` |
| `Token` | both | `resp_parser.cpp:57` | `devices/prusa_connect.rs:202=Token` | **identical**: `Token` of the registration answer | `registration_is_what_registrator_cpp_does` |
| `UpgradeWebsocket` | both | `resp_parser.cpp:66` |  | **not used by the hub**: WebSocket (6.4+) |  |
| `Version` | both | `resp_parser.cpp:85` |  | **OPEN**: HTTP version of the server's answer; not read against the parser here |  |
| `WebSocketAccept` | both | `resp_parser.cpp:60` |  | **not used by the hub**: WebSocket (6.4+) |  |
| `WsPrusaConnect` | both | `resp_parser.cpp:63` |  | **not used by the hub**: WebSocket (6.4+) |  |

## Fields it sends (telemetry, events, info)

Only the fields the event tests show are checked here; the rest of the telemetry and of `INFO` is UNCHECKED.

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `abbreviation` | master only |  |  | **not in 6.3.0** |  |
| `active` | both |  |  | UNCHECKED |  |
| `actual_netto_full_weight` | master only |  |  | **not in 6.3.0** |  |
| `addon_power` | both |  |  | UNCHECKED |  |
| `api_key` | both |  |  | UNCHECKED |  |
| `appendix` | both |  |  | UNCHECKED |  |
| `axis_x` | both |  |  | UNCHECKED |  |
| `axis_y` | both |  |  | UNCHECKED |  |
| `axis_z` | both |  |  | UNCHECKED |  |
| `base_model` | master only |  |  | **not in 6.3.0** |  |
| `buttons` | both | `render.cpp:654` | `devices/printer_files.rs:599=buttons` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `canceled` | both |  |  | UNCHECKED |  |
| `chamber` | both |  |  | UNCHECKED |  |
| `chamber_temperature` | master only |  |  | **not in 6.3.0** |  |
| `children` | both | `render.cpp:1023` | `devices/printer_files.rs:389=children` | **identical**: read from where `render.cpp` writes it | `the_listing_follows_file_info_and_file_changed` |
| `chunk` | both |  |  | UNCHECKED |  |
| `class` | master only |  |  | **not in 6.3.0** |  |
| `code` | both | `render.cpp:615` | `devices/printer_files.rs:590=code` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `color_rgba` | master only |  |  | **not in 6.3.0** |  |
| `command` | 6.3.0 only |  |  | UNCHECKED |  |
| `command_id` | both | `render.cpp:251` | `devices/prusa_connect.rs:517=command_id` | **identical**: read from where `render.cpp` writes it | `the_events_buddy_sends_settle_commands` |
| `consumed_weight` | master only |  |  | **not in 6.3.0** |  |
| `data` | both | `render.cpp:340` | `devices/printer_files.rs:570=data` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `dialog_id` | both | `render.cpp:255` | `devices/printer_files.rs:605=dialog_id` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub`, `buddy_takes_every_command_the_hub_sends` |
| `display_name` | both | `render.cpp:454` | `devices/printer_files.rs:634=display_name` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `enabled` | both |  |  | UNCHECKED |  |
| `enclosure` | both |  |  | UNCHECKED |  |
| `end` | both |  |  | UNCHECKED |  |
| `event` | both | `render.cpp:682` | `devices/prusa_connect.rs:516=event` | **identical**: read from where `render.cpp` writes it | `the_events_buddy_sends_settle_commands` |
| `extruder_fs_state` | both |  |  | UNCHECKED |  |
| `fan_1_rpm` | both |  |  | UNCHECKED |  |
| `fan_2_rpm` | both |  |  | UNCHECKED |  |
| `fan_extruder` | 6.3.0 only |  |  | UNCHECKED |  |
| `fan_hotend` | 6.3.0 only |  |  | UNCHECKED |  |
| `fan_print` | 6.3.0 only |  |  | UNCHECKED |  |
| `fan_pwm_target` | both |  |  | UNCHECKED |  |
| `fan_rpm` | both |  |  | UNCHECKED |  |
| `features` | master only |  |  | **not in 6.3.0** |  |
| `feeder` | master only |  |  | **not in 6.3.0** |  |
| `filament` | 6.3.0 only |  |  | UNCHECKED |  |
| `filament_change_in` | both |  |  | UNCHECKED |  |
| `file` | both |  |  | UNCHECKED |  |
| `file_count` | both |  |  | UNCHECKED |  |
| `file_id` | both |  |  | UNCHECKED |  |
| `filter_lifetime` | both |  |  | UNCHECKED |  |
| `filtration_filaments` | both |  |  | UNCHECKED |  |
| `fingerprint` | both |  |  | UNCHECKED |  |
| `firmware` | both |  |  | UNCHECKED |  |
| `flow` | both |  |  | UNCHECKED |  |
| `free_space` | both |  |  | UNCHECKED |  |
| `hardened` | 6.3.0 only |  |  | UNCHECKED |  |
| `hash` | both |  |  | UNCHECKED |  |
| `heatbreak_fan_rpm` | master only |  |  | **not in 6.3.0** |  |
| `heatbreak_temperature` | master only |  |  | **not in 6.3.0** |  |
| `high_flow` | 6.3.0 only |  |  | UNCHECKED |  |
| `hostname` | both |  |  | UNCHECKED |  |
| `id` | both |  |  | UNCHECKED |  |
| `is_sfn` | both |  |  | UNCHECKED |  |
| `job_id` | both | `render.cpp:149` | `devices/printer_files.rs:639=job_id` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `lan_ipv4` | both |  |  | UNCHECKED |  |
| `lan_mac` | both |  |  | UNCHECKED |  |
| `led_intensity` | both |  |  | UNCHECKED |  |
| `m_timestamp` | both |  |  | UNCHECKED |  |
| `machine_reason` | both |  |  | UNCHECKED |  |
| `material` | both |  |  | UNCHECKED |  |
| `material_package_instance` | master only |  |  | **not in 6.3.0** |  |
| `max_bed_temperature` | master only |  |  | **not in 6.3.0** |  |
| `max_chamber_temperature` | master only |  |  | **not in 6.3.0** |  |
| `max_print_temperature` | master only |  |  | **not in 6.3.0** |  |
| `min_bed_temperature` | master only |  |  | **not in 6.3.0** |  |
| `min_chamber_temperature` | master only |  |  | **not in 6.3.0** |  |
| `min_print_temperature` | master only |  |  | **not in 6.3.0** |  |
| `mmu` | 6.3.0 only |  |  | UNCHECKED |  |
| `mmu_command_in_progress` | master only |  |  | **not in 6.3.0** |  |
| `mmu_progress_code` | master only |  |  | **not in 6.3.0** |  |
| `model` | master only |  |  | **not in 6.3.0** |  |
| `mountpoint` | both |  |  | UNCHECKED |  |
| `name` | both |  |  | UNCHECKED |  |
| `network_info` | both |  |  | UNCHECKED |  |
| `new_path` | both |  |  | UNCHECKED |  |
| `nominal_netto_full_weight` | master only |  |  | **not in 6.3.0** |  |
| `nozzle_diameter` | both |  |  | UNCHECKED |  |
| `nozzle_hardened` | master only |  |  | **not in 6.3.0** |  |
| `nozzle_high_flow` | master only |  |  | **not in 6.3.0** |  |
| `nozzle_target_temp` | master only |  |  | **not in 6.3.0** |  |
| `nozzle_temp` | master only |  |  | **not in 6.3.0** |  |
| `objects` | both |  |  | UNCHECKED |  |
| `odometer_e` | master only |  |  | **not in 6.3.0** |  |
| `odometer_id` | master only |  |  | **not in 6.3.0** |  |
| `old_path` | both | `render.cpp:552` | `devices/printer_files.rs:652=old_path` | **identical**: read from where `render.cpp` writes it | `the_listing_follows_file_info_and_file_changed` |
| `package` | master only |  |  | **not in 6.3.0** |  |
| `path` | both |  |  | UNCHECKED |  |
| `post_print` | both |  |  | UNCHECKED |  |
| `post_print_filtration_time` | both |  |  | UNCHECKED |  |
| `preheat_temperature` | master only |  |  | **not in 6.3.0** |  |
| `preview` | both |  |  | UNCHECKED |  |
| `primary_color` | master only |  |  | **not in 6.3.0** |  |
| `print_fan_rpm` | master only |  |  | **not in 6.3.0** |  |
| `printer_type` | both |  |  | UNCHECKED |  |
| `printing_filtration` | both |  |  | UNCHECKED |  |
| `progress` | both |  |  | UNCHECKED |  |
| `properties` | master only |  |  | **not in 6.3.0** |  |
| `read_only` | both |  |  | UNCHECKED |  |
| `reason` | both |  |  | UNCHECKED |  |
| `remote_fs_state` | both |  |  | UNCHECKED |  |
| `rescan` | both |  |  | UNCHECKED |  |
| `size` | both |  |  | UNCHECKED |  |
| `slot` | 6.3.0 only |  |  | UNCHECKED |  |
| `slot_count` | master only |  |  | **not in 6.3.0** |  |
| `slots` | 6.3.0 only |  |  | UNCHECKED |  |
| `sn` | both |  |  | UNCHECKED |  |
| `speed` | both |  |  | UNCHECKED |  |
| `start` | both |  |  | UNCHECKED |  |
| `start_cmd_id` | both | `render.cpp:460` | `devices/printer_files.rs:683=start_cmd_id` | **identical**: read from where `render.cpp` writes it | `transfer_events_name_their_command_in_data` |
| `state` | both | `render.cpp:242` | `devices/printer_files.rs:575=state` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub`, `every_state_buddy_sends_is_read` |
| `storages` | both |  |  | UNCHECKED |  |
| `tags` | master only |  |  | **not in 6.3.0** |  |
| `target_bed` | both |  |  | UNCHECKED |  |
| `target_nozzle` | 6.3.0 only |  |  | UNCHECKED |  |
| `target_temp` | both |  |  | UNCHECKED |  |
| `team_id` | both |  |  | UNCHECKED |  |
| `temp` | both |  |  | UNCHECKED |  |
| `temp_ambient` | both |  |  | UNCHECKED |  |
| `temp_bed` | both |  |  | UNCHECKED |  |
| `temp_heatbreak` | both |  |  | UNCHECKED |  |
| `temp_nozzle` | 6.3.0 only |  |  | UNCHECKED |  |
| `temp_psu` | both |  |  | UNCHECKED |  |
| `text` | both | `render.cpp:636` | `devices/printer_files.rs:583=text` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `time_in_use` | both |  |  | UNCHECKED |  |
| `time_printing` | both |  |  | UNCHECKED |  |
| `time_remaining` | both |  |  | UNCHECKED |  |
| `time_transferring` | both |  |  | UNCHECKED |  |
| `title` | both | `render.cpp:626` | `devices/printer_files.rs:583=title` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `tool_count` | master only |  |  | **not in 6.3.0** |  |
| `tools` | both |  |  | UNCHECKED |  |
| `transfer` | both |  |  | UNCHECKED |  |
| `transfer_id` | both |  |  | UNCHECKED |  |
| `transfer_paused` | both |  |  | UNCHECKED |  |
| `transfer_progress` | both |  |  | UNCHECKED |  |
| `transfer_time_remaining` | both |  |  | UNCHECKED |  |
| `transfer_transferred` | both |  |  | UNCHECKED |  |
| `transferred` | both |  |  | UNCHECKED |  |
| `type` | both |  |  | UNCHECKED |  |
| `uuid` | master only |  |  | **not in 6.3.0** |  |
| `version` | 6.3.0 only |  |  | UNCHECKED |  |
| `wifi_ipv4` | both |  |  | UNCHECKED |  |
| `wifi_mac` | both |  |  | UNCHECKED |  |
| `wifi_ssid` | both |  |  | UNCHECKED |  |

## Values it sends (states, events, types)

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `ACCEPTED` | both | `planner.cpp:256` | `devices/prusa_connect.rs:519=ACCEPTED` | **identical**: a command with that id becomes accepted; the event has no `job_id` or `data` (`render.cpp:269`) | `the_events_buddy_sends_settle_commands` |
| `CANCELABLE_CHANGED` | both | `planner.cpp:278` | `devices/prusa_connect.rs:527=CANCELABLE_CHANGED` | **identical**: the command (`CANCEL_OBJECT`, `UNCANCEL_OBJECT`, never sent by the hub) would be finished; the `objects` list is not read by the hub | `the_events_buddy_sends_settle_commands` |
| `DISABLED` | both |  |  | UNCHECKED |  |
| `FAILED` | both | `planner.cpp:268` | `devices/prusa_connect.rs:531=FAILED` | **identical**: the command becomes rejected (a background G-code that failed, `planner.cpp:1112`) | `the_events_buddy_sends_settle_commands` |
| `FFF` | master only |  |  | **not in 6.3.0** |  |
| `FFF_FEEDER_SLOT` | master only |  |  | **not in 6.3.0** |  |
| `FFF_PRINT_HEAD` | master only |  |  | **not in 6.3.0** |  |
| `FILE_CHANGED` | both | `planner.cpp:276` | `devices/printer_files.rs:648=FILE_CHANGED` | **identical**: the command (delete, make folder) is finished by it (`planner.cpp:850`); `old_path` takes the file out of the listing | `the_events_buddy_sends_settle_commands`, `the_listing_follows_file_info_and_file_changed` |
| `FILE_EXISTS` | both |  |  | UNCHECKED |  |
| `FILE_INFO` | both | `planner.cpp:262` | `devices/printer_files.rs:616=FILE_INFO` | **identical**: the command is finished; a folder's `children` and `path` fill the hub's listing of the stick | `the_events_buddy_sends_settle_commands`, `the_listing_follows_file_info_and_file_changed` |
| `FINISHED` | both | `planner.cpp:266` | `devices/prusa_connect.rs:526=FINISHED` | **identical**: the command becomes finished | `the_events_buddy_sends_settle_commands` |
| `FIN_OK` | both |  |  | UNCHECKED |  |
| `FIN_STOPPED` | both |  |  | UNCHECKED |  |
| `FOLDER` | both |  |  | UNCHECKED |  |
| `HAS_FILAMENT` | both |  |  | UNCHECKED |  |
| `INFO` | both | `planner.cpp:254` | `devices/prusa_connect.rs:547=INFO` | **differs** D2 (D4, D5): the command is finished and the details are kept (serial, type, firmware, nozzle, storages, network); `tools.<n>.material` says `---` for none and the hub keeps it as the material | `the_events_buddy_sends_settle_commands`, `info_says_a_dash_for_no_material` |
| `JOB_INFO` | both | `planner.cpp:260` | `devices/printer_files.rs:633=JOB_INFO` | **identical**: the command is finished; `job_id` at the top and `data.display_name` give the file a print is (as the hub's telemetry shows it) | `the_events_buddy_sends_settle_commands`, `state_changed_and_job_info_reach_the_hub` |
| `MMU` | master only |  |  | **not in 6.3.0** |  |
| `MMU3` | master only |  |  | **not in 6.3.0** |  |
| `NETWORK_FAILURE` | both |  |  | UNCHECKED |  |
| `NOT_CALIBRATED` | both |  |  | UNCHECKED |  |
| `NOT_CONNECTED` | both |  |  | UNCHECKED |  |
| `NOT_INITIALIZED` | both |  |  | UNCHECKED |  |
| `NOT_READY` | both |  |  | UNCHECKED |  |
| `NO_FILAMENT` | both |  |  | UNCHECKED |  |
| `NO_TRANSFER` | both |  |  | UNCHECKED |  |
| `PAUSED` | both | `render.cpp:445` | `devices/prusa_connect.rs:243=PAUSED` | **identical**: read as a device state (see States) | `every_state_buddy_sends_is_read` |
| `PRINTING` | both | `render.cpp:443` | `devices/prusa_connect.rs:242=PRINTING` | **identical**: read as a device state (see States) | `every_state_buddy_sends_is_read` |
| `REJECTED` | both | `planner.cpp:258` | `devices/prusa_connect.rs:531=REJECTED` | **identical**: the command becomes rejected; `reason` and `machine_reason` (`render.cpp:327`, `:331`) are kept as the reason. The hub reads the text (D9) | `the_events_buddy_sends_settle_commands` |
| `STATE_CHANGED` | both | `planner.cpp:280` | `devices/printer_files.rs:574=STATE_CHANGED` | **identical**: `data.code`, `title`, `text`, `buttons` and the top-level `dialog_id` give what the screen asks (and the buttons for `DIALOG_ACTION`); a state without a dialog clears it | `the_events_buddy_sends_settle_commands`, `state_changed_and_job_info_reach_the_hub` |
| `STORAGE_FAILURE` | both |  |  | UNCHECKED |  |
| `TRANSFER_ABORTED` | both | `planner.cpp:272` | `devices/printer_files.rs:682=TRANSFER_ABORTED` | **OPEN**: as `TRANSFER_STOPPED`; the `machine_reason` (`STORAGE_FAILURE`, `NETWORK_FAILURE`) wording is not run by a test here |  |
| `TRANSFER_FINISHED` | both | `planner.cpp:274` | `devices/printer_files.rs:682=TRANSFER_FINISHED` | **identical**: no top-level `command_id` (`render.cpp:537`); the hub settles the download command by `data.start_cmd_id` and marks the transfer finished | `transfer_events_name_their_command_in_data` |
| `TRANSFER_INFO` | both | `planner.cpp:264` | `devices/printer_files.rs:665=TRANSFER_INFO` | **identical**: the download command is finished (taken, the file on its way) | `the_events_buddy_sends_settle_commands`, `transfer_events_name_their_command_in_data` |
| `TRANSFER_IN_PROGRESS` | both |  |  | UNCHECKED |  |
| `TRANSFER_STOPPED` | both | `planner.cpp:270` | `devices/printer_files.rs:682=TRANSFER_STOPPED` | **OPEN**: no top-level `command_id` (`render.cpp:537`); the hub takes the command from `data.start_cmd_id`; the stopped branch (failed transfer, job failed) is not run by a test here |  |
| `USB` | both |  |  | UNCHECKED |  |

## States it sends (`state/printer_state.cpp` `to_str(DeviceState)`)

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `IDLE` | both | `printer_state.cpp:445` | `devices/prusa_connect.rs:246=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `PRINTING` | both | `printer_state.cpp:447` | `devices/prusa_connect.rs:242=PRINTING` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `PAUSED` | both | `printer_state.cpp:449` | `devices/prusa_connect.rs:243=PAUSED` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `FINISHED` | both | `printer_state.cpp:451` | `devices/prusa_connect.rs:246=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `STOPPED` | both | `printer_state.cpp:453` | `devices/prusa_connect.rs:246=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `READY` | both | `printer_state.cpp:455` | `devices/prusa_connect.rs:246=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `ERROR` | both | `printer_state.cpp:457` | `devices/prusa_connect.rs:245=ERROR` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `BUSY` | both | `printer_state.cpp:459` | `devices/prusa_connect.rs:242=BUSY` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `ATTENTION` | both | `printer_state.cpp:461` | `devices/prusa_connect.rs:244=ATTENTION` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `UNKNOWN` | both | `printer_state.cpp:464` | `devices/prusa_connect.rs:246=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |

## G-code metadata it passes on or leaves out (`render.cpp`)

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `bed_temperature (Int)` | both |  |  | UNCHECKED |  |
| `brim_width (Int)` | both |  |  | UNCHECKED |  |
| `display_name (Ignore)` | both |  |  | UNCHECKED |  |
| `estimated_print_time (Int)` | both |  |  | UNCHECKED |  |
| `filament cost (Float)` | both |  |  | UNCHECKED |  |
| `filament used [cm3] (Float)` | both |  |  | UNCHECKED |  |
| `filament used [m] (Float)` | both |  |  | UNCHECKED |  |
| `filament used [mm3] (Float)` | both |  |  | **n/a**: sent as a number; the hub shows none of it, and reads the keys it shows as text or number alike |  |
| `filament used [mm] (Float)` | both |  |  | UNCHECKED |  |
| `ironing (Int)` | both |  |  | UNCHECKED |  |
| `layer_height (Float)` | both |  |  | UNCHECKED |  |
| `m_timestamp (Ignore)` | both |  |  | UNCHECKED |  |
| `max_layer_z (Float)` | both |  |  | UNCHECKED |  |
| `path (Ignore)` | both |  |  | UNCHECKED |  |
| `preview (Ignore)` | both |  |  | UNCHECKED |  |
| `read_only (Ignore)` | both |  |  | UNCHECKED |  |
| `size (Ignore)` | both |  |  | UNCHECKED |  |
| `support_material (Int)` | both |  |  | UNCHECKED |  |
| `temperature (Int)` | both |  |  | UNCHECKED |  |
| `total filament used for wipe tower [g] (Float)` | both |  |  | UNCHECKED |  |
| `type (Ignore)` | both |  |  | UNCHECKED |  |

## Answers a dialog takes (`common/general_response.cpp`)

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `Abort` | both | `general_response.cpp:9` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Abort_invalidate_test` | both | `general_response.cpp:10` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Adjust` | both | `general_response.cpp:11` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `All` | both | `general_response.cpp:12` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Always` | master only |  |  | **not in 6.3.0** |  |
| `Back` | both | `general_response.cpp:13` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Calibrate` | both | `general_response.cpp:14` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Cancel` | both | `general_response.cpp:15` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Change` | both | `general_response.cpp:17` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Continue` | both | `general_response.cpp:18` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Cooldown` | both | `general_response.cpp:19` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Details` | master only |  |  | **not in 6.3.0** |  |
| `Disable` | both | `general_response.cpp:20` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Docks4` | master only |  |  | **not in 6.3.0** |  |
| `Docks8` | master only |  |  | **not in 6.3.0** |  |
| `Done` | both | `general_response.cpp:21` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Empty` | master only |  |  | **not in 6.3.0** |  |
| `FS_disable` | both | `general_response.cpp:25` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Filament` | 6.3.0 only | `general_response.cpp:22` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Filament_removed` | 6.3.0 only | `general_response.cpp:23` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Finish` | both | `general_response.cpp:24` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Heatup` | both | `general_response.cpp:52` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Help` | both | `general_response.cpp:16` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Ignore` | both | `general_response.cpp:26` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Left` | both | `general_response.cpp:27` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Load` | both | `general_response.cpp:28` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `MMU_disable` | both | `general_response.cpp:29` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Never` | both | `general_response.cpp:30` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Next` | both | `general_response.cpp:31` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `No` | both | `general_response.cpp:32` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `NotNow` | both | `general_response.cpp:33` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Ok` | both | `general_response.cpp:34` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Other` | master only |  |  | **not in 6.3.0** |  |
| `PRINT` | 6.3.0 only | `general_response.cpp:54` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Pause` | both | `general_response.cpp:35` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Postpone5Days` | both | `general_response.cpp:53` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Print` | both | `general_response.cpp:36` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Purge_more` | both | `general_response.cpp:37` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Quit` | both | `general_response.cpp:38` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Reheat` | both | `general_response.cpp:39` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Remap` | master only |  |  | **not in 6.3.0** |  |
| `Remove` | both | `general_response.cpp:41` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Replace` | both | `general_response.cpp:40` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Restart` | both | `general_response.cpp:42` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Resume` | both | `general_response.cpp:43` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Retry` | both | `general_response.cpp:44` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Right` | both | `general_response.cpp:45` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Skip` | both | `general_response.cpp:46` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Slowly` | both | `general_response.cpp:47` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `SpoolJoin` | both | `general_response.cpp:48` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Stop` | both | `general_response.cpp:49` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Tool1` | 6.3.0 only | `general_response.cpp:55` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Tool2` | 6.3.0 only | `general_response.cpp:56` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Tool3` | 6.3.0 only | `general_response.cpp:57` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Tool4` | 6.3.0 only | `general_response.cpp:58` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Tool5` | 6.3.0 only | `general_response.cpp:59` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Unload` | both | `general_response.cpp:50` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Yes` | both | `general_response.cpp:51` | `web/machines.rs:496=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |

## Differences (for the owner's decision; the hub's behaviour is not changed)

Each: Buddy 6.3.0, ours, what differs, and a proposed fix. D1 and D2 are shown by tests; the others are read
from the source and named so.

| # | Buddy 6.3.0 | Ours | What differs | Proposed fix |
|---|---|---|---|---|
| D1 | `command.cpp:180` (`ARGS_DIALOG_ACTION`, `:86`), `render.cpp:255` | `web/machines.rs:497` | `DIALOG_ACTION` needs `dialog_id` and `button`. The hub sends `button` alone when its record of the screen has buttons but no dialog id; Buddy answers REJECTED "Missing or broken parameters" (`command.cpp:420`). Test `a_dialog_answer_without_a_dialog_is_refused_by_buddy` | Do not offer the buttons (or refuse the press) when the printer named no dialog |
| D2 | `render.cpp:395` | `devices/prusa_connect.rs:397` | `INFO` writes `"---"` for a tool with nothing loaded; the hub keeps `---` as the machine's material. Test `info_says_a_dash_for_no_material` | Read `---` as no material |
| D3 | `planner.cpp:280` (the 14 events, `:253-280`) | `devices/printer_files.rs:627` | The hub reads `MEDIUM_INSERTED` and `MEDIUM_EJECTED` events (its comment: Prusa's Connect SDK, PrusaLink's). Buddy 6.3.0 never sends them: a second source | Remove (Buddy reports a stick change as `FILE_CHANGED` with `rescan`, `render.cpp:555`), or the owner approves it as ours |
| D4 | `render.cpp:342`, `registrator.cpp:63` | `devices/prusa_connect.rs:131`, `devices/prusa_connect.rs:384` | Buddy writes `printer_type` as a string `"2.1.0"`. The hub also reads `type`, `version`, `subversion` as numbers (Prusa's Connect SDK's form): a second source | Read the string only |
| D5 | telemetry and `INFO` of 6.3.0 (`render.cpp:129-436`: flat `temp_nozzle`, `target_nozzle`, `fan_print`, `fan_extruder`, `tools.<n>.material`) | `devices/prusa_connect.rs:223`, `devices/prusa_connect.rs:391` | The hub reads keys that exist only in master (6.4+): `tools.<n>.nozzle_temp`, `nozzle_target_temp`, `print_fan_rpm`, `heatbreak_fan_rpm`, `features`, `material_package_instance`, `package`, `abbreviation` (the checker lists them under "What we send or read that the firmware does not") | Keep for the 6.4+ step the owner put later, or remove; the owner decides |
| D6 | no such G-code in Buddy's Connect (`M604` is Buddy's "Abort serial print", `src/marlin_stubs/host/M604.cpp:9`) | `devices/printer_files.rs:874` | After a download that is not printed the hub sends `M604` as G-code to close the print preview (its own comment: "WORKAROUND, NEEDS TRACING"). Not a Connect command, and not shown to be what Buddy's UI or Connect sends | Replace by what Buddy 6.3.0 does for the same thing, once traced; or the owner approves it as ours |
| D7 | `command.cpp:162`, `planner.cpp:750`, `printer.hpp:190` | `devices/prusa_connect.rs:524` | Comments and `docs/prusa-connect-protocol.md` say `SET_PRINTER_READY` is since 6.4 (it is in 6.3.0, answered `FINISHED` or `REJECTED`, not `STATE_CHANGED`) and that a token is at most 19 characters (`CONNECT_TOKEN_LEN` is 20; the hub's 19 is taken). Words only, no effect on the wire | Correct the comment and the document |
| D8 | `planner.cpp:661`, `render.cpp:251` | `store/mod.rs:374` | The hub gives up on a command after 120 s without an answer (a number of ours; Buddy has none) and sends the next; a G-code still running in Buddy then makes it answer REJECTED. The telemetry's `command_id` keeps the hub's command alive while it runs | Drop the timer for a printer that reports its `command_id`; the owner decides what a command with no answer at all does |
| D9 | `planner.cpp:294`, `render.cpp:331` | `devices/printer_files.rs:750` | The hub decides to send a file again under a free name by looking for "exist" or "not found" in the human `reason`; Buddy gives `machine_reason` `FILE_EXISTS` for that. Works with 6.3.0's texts, but reads the wrong field | Read `machine_reason` |
| D10 | `registrator.cpp:233` | `devices/prusa_connect.rs:204` | The token goes out once: a second `GET /p/register` with the same code gets `404`, which the printer reads as a server error and polls again for ever (its `retries` is none). If the first answer is lost on the way, the printer never gets its token | Keep answering the code with its token until the printer's first report with it |

## What we send or read that the firmware does not

| Item | Where | Why | Status |
|---|---|---|---|
| `/p/`, `/f/` | `prusa_connect.rs` | The hub's own route prefixes (`starts_with`, which requests to log), not endpoints | n/a |
| `Content-Type` | `prusa_connect.rs` | The header whose value the firmware reads (`ApplicationJson`, `TextGcodeDot`, `TextGcodeDash`) | see What it reads in an answer |
| `abbreviation` | `prusa_connect.rs` | Read from the INFO event or telemetry; the key exists only in master (6.4+), not in 6.3.0 (D5) | OPEN: the owner decides (D5) |
| `features` | `prusa_connect.rs` | Read from the INFO event or telemetry; the key exists only in master (6.4+), not in 6.3.0 (D5) | OPEN: the owner decides (D5) |
| `material_package_instance` | `prusa_connect.rs` | Read from the INFO event or telemetry; the key exists only in master (6.4+), not in 6.3.0 (D5) | OPEN: the owner decides (D5) |
| `nozzle_target_temp` | `prusa_connect.rs` | Read from the INFO event or telemetry; the key exists only in master (6.4+), not in 6.3.0 (D5) | OPEN: the owner decides (D5) |
| `nozzle_temp` | `prusa_connect.rs` | Read from the INFO event or telemetry; the key exists only in master (6.4+), not in 6.3.0 (D5) | OPEN: the owner decides (D5) |
| `package` | `prusa_connect.rs` | Read from the INFO event or telemetry; the key exists only in master (6.4+), not in 6.3.0 (D5) | OPEN: the owner decides (D5) |

## Not covered

- The WebSocket (`/p/ws`), `START_INLINE_DOWNLOAD`, `START_CONNECT_DOWNLOAD` (6.4+).
- The encrypted download (`/f/<iv>/raw`): Range, 200/206, `Content-Encryption-Mode`, the decryptor of `transfers/download.cpp`.
- The telemetry's fields and the INFO event's data beyond what the tests above read; G-code metadata; the
  transfer events' failure branches (`TRANSFER_STOPPED`, `TRANSFER_ABORTED`).
- The G-code the hub sends as `text/x.gcode` (the MINI's menu G-code is HS10).
- Buddy's C++ is not run: every "identical" is against a port of its parser and fixtures written from its
  serializer, not against its own output. TLS, HTTP connection reuse and timing of the printer.
