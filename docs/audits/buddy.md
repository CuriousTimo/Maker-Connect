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
| `CREATE_FOLDER` | both | `command.cpp:167` | `devices/printer_files.rs:1469` | **identical**: taken as `CreateFolder` with `path` | `buddy_takes_every_command_the_hub_sends` |
| `DELETE_FILE` | both | `command.cpp:165` | `devices/printer_files.rs:1437` | **identical**: taken as `DeleteFile` with `path` | `buddy_takes_every_command_the_hub_sends` |
| `DELETE_FOLDER` | both | `command.cpp:166` | `devices/printer_files.rs:1435` | **identical**: taken as `DeleteFolder` with `path` | `buddy_takes_every_command_the_hub_sends` |
| `DIALOG_ACTION` | both | `command.cpp:180` | `web/machines.rs:483` | **identical**: taken as `DialogAction` with `dialog_id` and `button`; the hub offers the buttons only when the printer named a dialog, and refuses a press without one ("the printer named no dialog"), so it never sends one without `dialog_id` | `buddy_takes_every_command_the_hub_sends`, `a_dialog_answer_without_a_dialog_is_refused_by_the_hub` |
| `PAUSE_PRINT` | both | `command.cpp:158` | `devices/printer_files.rs:1114` | **identical**: taken as `PausePrint` | `buddy_takes_every_command_the_hub_sends` |
| `RESET` | both | `command.cpp:178` |  | **not used by the hub** |  |
| `RESET_PRINTER` | both | `command.cpp:177` |  | **not used by the hub** |  |
| `RESUME_PRINT` | both | `command.cpp:160` | `devices/printer_files.rs:1115` | **identical**: taken as `ResumePrint` | `buddy_takes_every_command_the_hub_sends` |
| `SEND_FILE_INFO` | both | `command.cpp:156` | `devices/printer_files.rs:941` `devices/printer_files.rs:1201` | **identical**: the body Buddy's port takes as `SendFileInfo` with its `path` | `buddy_takes_every_command_the_hub_sends` |
| `SEND_INFO` | both | `command.cpp:154` |  | **not used by the hub** |  |
| `SEND_JOB_INFO` | both | `command.cpp:155` | `devices/printer_files.rs:1010` | **identical**: taken as `SendJobInfo` with `job_id` | `buddy_takes_every_command_the_hub_sends` |
| `SEND_STATE_INFO` | both | `command.cpp:179` |  | **not used by the hub** |  |
| `SEND_TRANSFER_INFO` | both | `command.cpp:157` |  | **not used by the hub** |  |
| `SET_IDLE` | both | `command.cpp:164` |  | **not used by the hub** |  |
| `SET_PRINTER_READY` | both | `command.cpp:162` | `farm.rs:1592` | **identical**: the literal body, taken as `SetPrinterReady` | `buddy_takes_set_printer_ready` |
| `SET_TOKEN` | both | `command.cpp:169` |  | **not used by the hub** |  |
| `SET_VALUE` | both | `command.cpp:181` |  | **not used by the hub** |  |
| `START_CONNECT_DOWNLOAD` | both | `command.cpp:191` |  | **not used by the hub** |  |
| `START_ENCRYPTED_DOWNLOAD` | both | `command.cpp:192` | `devices/printer_files.rs:555` | **identical**: taken as `StartEncryptedDownload` with `path`, `key`, `iv`, `orig_size` (32 hex digits each), no `port` | `buddy_takes_every_command_the_hub_sends` |
| `START_INLINE_DOWNLOAD` | both | `command.cpp:184` |  | **not used by the hub** |  |
| `START_PRINT` | both | `command.cpp:161` | `devices/printer_files.rs:391` `devices/printer_files.rs:1388` | **identical**: taken as `StartPrint` with `path` (no `tool_mapping`: optional) | `buddy_takes_every_command_the_hub_sends` |
| `STOP_PRINT` | both | `command.cpp:159` | `devices/printer_files.rs:1116` | **identical**: taken as `StopPrint` | `buddy_takes_every_command_the_hub_sends` |
| `STOP_TRANSFER` | both | `command.cpp:168` |  | **not used by the hub** |  |
| `UNCANCEL_OBJECT` | both | `command.cpp:183` |  | **not used by the hub** |  |

## Arguments it reads from a command (`command.cpp`)

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `addon_power` | both | `command.cpp:410` |  | **not used by the hub** |  |
| `button` | both | `command.cpp:323` | `web/machines.rs:482` | **identical**: one of the printer's own names (see Answers a dialog takes) | `buddy_takes_every_command_the_hub_sends` |
| `chamber.fan_pwm_target` | both | `command.cpp:408` |  | **not used by the hub** |  |
| `chamber.led_intensity` | both | `command.cpp:414` |  | **not used by the hub** |  |
| `chamber.target_temp` | both | `command.cpp:406` |  | **not used by the hub** |  |
| `command` | both | `command.cpp:147` | `devices/printer_files.rs:1337` | **identical**: in every body, a string at depth 1 (`:147`) | `buddy_takes_every_command_the_hub_sends` |
| `dialog_id` | both | `command.cpp:400` | `web/machines.rs:482` | **identical**: a number, the one the printer's `STATE_CHANGED` named (`render.cpp:673`); with none named there is no `DIALOG_ACTION` | `buddy_takes_every_command_the_hub_sends`, `a_dialog_answer_without_a_dialog_is_refused_by_the_hub` |
| `enclosure_enabled` | both | `command.cpp:346` |  | **not used by the hub** |  |
| `enclosure_postprint` | both | `command.cpp:350` |  | **not used by the hub** |  |
| `enclosure_postprint_filtration_time` | both | `command.cpp:352` |  | **not used by the hub** |  |
| `enclosure_printing_filtration` | both | `command.cpp:348` |  | **not used by the hub** |  |
| `hash` | both | `command.cpp:390` |  | **not used by the hub** |  |
| `hostname` | both | `command.cpp:332` |  | **not used by the hub** |  |
| `id` | both | `command.cpp:402` |  | **not used by the hub** |  |
| `iv` | both | `command.cpp:398` | `devices/printer_files.rs:559` | **identical**: 32 hex digits | `buddy_takes_every_command_the_hub_sends` |
| `job_id` | both | `command.cpp:269` | `devices/printer_files.rs:1010` | **identical**: a number that fits 16 bits (it is the printer's own `job_id`) | `buddy_takes_every_command_the_hub_sends` |
| `key` | both | `command.cpp:396` | `devices/printer_files.rs:558` | **identical**: 32 hex digits | `buddy_takes_every_command_the_hub_sends` |
| `kwargs` | both | `command.cpp:265` | `devices/printer_files.rs:1337` | **identical**: an object at depth 1 (`:265`) | `buddy_takes_every_command_the_hub_sends` |
| `orig_size` | both | `command.cpp:385` | `devices/printer_files.rs:560` | **identical**: a number that fits 32 bits | `buddy_takes_every_command_the_hub_sends` |
| `path` | both | `command.cpp:271` | `devices/printer_files.rs:1337` | **identical**: a string; short names of the stick (`path_sfn` is the same argument, `:271`) | `buddy_takes_every_command_the_hub_sends` |
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
| `REJECTED` | both | `planner.cpp:258` | `devices/prusa_connect.rs:531=REJECTED` | **identical**: the command becomes rejected; `reason` and `machine_reason` (`render.cpp:327`, `:331`) are kept as the reason. The hub reads `machine_reason` `FILE_EXISTS` to send a file again under a free name, not the text | `the_events_buddy_sends_settle_commands`, `a_free_name_is_decided_by_machine_reason` |
| `FINISHED` | both | `planner.cpp:266` | `devices/prusa_connect.rs:526=FINISHED` | **identical**: the command becomes finished | `the_events_buddy_sends_settle_commands` |
| `FAILED` | both | `planner.cpp:268` | `devices/prusa_connect.rs:531=FAILED` | **identical**: the command becomes rejected (a background G-code that failed, `planner.cpp:1112`) | `the_events_buddy_sends_settle_commands` |
| `INFO` | both | `planner.cpp:254` | `devices/prusa_connect.rs:550=INFO` | **identical**: the command is finished and the details are kept (serial, `printer_type` as the string `2.1.0`, firmware, `nozzle_diameter` at the top level, `tools.<n>.material` with `---` read as none, storages, network); keys only master writes are not read | `the_events_buddy_sends_settle_commands`, `info_says_a_dash_for_no_material`, `printer_type_is_read_as_buddys_string_only`, `info_of_6_3_0_is_read_as_buddy_writes_it` |
| `JOB_INFO` | both | `planner.cpp:260` | `devices/printer_files.rs:748=JOB_INFO` | **identical**: the command is finished; `job_id` at the top and `data.display_name` give the file a print is (as the hub's telemetry shows it) | `the_events_buddy_sends_settle_commands`, `state_changed_and_job_info_reach_the_hub` |
| `FILE_INFO` | both | `planner.cpp:262` | `devices/printer_files.rs:736=FILE_INFO` | **identical**: the command is finished; a folder's `children` and `path` fill the hub's listing of the stick | `the_events_buddy_sends_settle_commands`, `the_listing_follows_file_info_and_file_changed` |
| `TRANSFER_INFO` | both | `planner.cpp:264` | `devices/printer_files.rs:782=TRANSFER_INFO` | **identical**: the download command is finished (taken, the file on its way) | `the_events_buddy_sends_settle_commands`, `transfer_events_name_their_command_in_data` |
| `TRANSFER_STOPPED` | both | `planner.cpp:270` | `devices/printer_files.rs:799=TRANSFER_STOPPED` | **OPEN**: no top-level `command_id` (`render.cpp:537`); the hub takes the command from `data.start_cmd_id`; the stopped branch (failed transfer, job failed) is not run by a test here |  |
| `TRANSFER_ABORTED` | both | `planner.cpp:272` | `devices/printer_files.rs:799=TRANSFER_ABORTED` | **OPEN**: as `TRANSFER_STOPPED`; the `machine_reason` (`STORAGE_FAILURE`, `NETWORK_FAILURE`) wording is not run by a test here |  |
| `TRANSFER_FINISHED` | both | `planner.cpp:274` | `devices/printer_files.rs:799=TRANSFER_FINISHED` | **identical**: no top-level `command_id` (`render.cpp:537`); the hub settles the download command by `data.start_cmd_id` and marks the transfer finished | `transfer_events_name_their_command_in_data` |
| `FILE_CHANGED` | both | `planner.cpp:276` | `devices/printer_files.rs:765=FILE_CHANGED` | **identical**: the command (delete, make folder) is finished by it (`planner.cpp:850`); `old_path` takes the file out of the listing | `the_events_buddy_sends_settle_commands`, `the_listing_follows_file_info_and_file_changed` |
| `CANCELABLE_CHANGED` | both | `planner.cpp:278` | `devices/prusa_connect.rs:527=CANCELABLE_CHANGED` | **identical**: the command (`CANCEL_OBJECT`, `UNCANCEL_OBJECT`, never sent by the hub) would be finished; the `objects` list is not read by the hub | `the_events_buddy_sends_settle_commands` |
| `STATE_CHANGED` | both | `planner.cpp:280` | `devices/printer_files.rs:689=STATE_CHANGED` | **identical**: `data.code`, `title`, `text`, `buttons` and the top-level `dialog_id` give what the screen asks (and the buttons for `DIALOG_ACTION`); a state without a dialog clears it | `the_events_buddy_sends_settle_commands`, `state_changed_and_job_info_reach_the_hub` |

## What it does with the server's answer, and its registration

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| status `204` to a report or event |  | `connect.cpp:726=NoContent` | `devices/prusa_connect.rs:492=NO_CONTENT` | **identical**: nothing to tell; the printer goes on | `the_answer_to_a_report_is_what_buddy_reads` |
| status `200` with a command |  | `connect.cpp:730=Status::Ok` | `devices/prusa_connect.rs:483=StatusCode::OK` | **identical**: `Command-Id` as decimal digits (`command_id.cpp`), a content type Buddy knows, a body of at most 512 bytes | `the_answer_to_a_report_is_what_buddy_reads`, `buddy_takes_every_command_the_hub_sends` |
| `Command-Id` header |  | `resp_parser.cpp:51=CommandId` | `devices/prusa_connect.rs:485=Command-Id` | **identical**: the command's number, as digits | `the_answer_to_a_report_is_what_buddy_reads` |
| `Content-Type: application/json` command |  | `resp_parser.cpp:42=ApplicationJson` | `devices/prusa_connect.rs:478=application/json` | **identical**: a JSON command parsed by `parse_json_command` | `buddy_takes_every_command_the_hub_sends` |
| `Content-Type: text/x.gcode` command |  | `resp_parser.cpp:45=TextGcodeDot` | `devices/prusa_connect.rs:480=text/x.gcode` | **identical**: G-code lines; `text/x-gcode` is the same to Buddy (`:48`) | `the_answer_to_a_report_is_what_buddy_reads` |
| a command body over 512 bytes |  | `connect.cpp:203=512` | `commands.rs:315=512` | **OPEN**: JSON commands the hub builds are far smaller (tested); the G-code of the hub's typed commands is limited to 480 characters on the hub's side, a limit that is the hub's UI. A G-code body of the MINI mapping (HS10) is not covered | `buddy_takes_every_command_the_hub_sends` |
| one command at a time |  | `planner.cpp:661=Processing other command` | `store/commands.rs:323=next_command_of_any_kind` | **identical**: the hub sends one and the next only once the first is answered (FINISHED or REJECTED), however long Buddy carries it out (its `command_id` in telemetry, `render.cpp:251`); no time of the hub's ends it. When the printer itself was not seen for longer than the hub's `ONLINE_SECONDS`, what it had not answered fails with "the printer was gone" (ours, K18 (b), not on the wire) | `the_answer_to_a_report_is_what_buddy_reads`, `a_command_stays_pending_until_buddy_answers_it` |
| status `401` to an unknown token |  | `connect.cpp:631=Unauthorized` | `devices/prusa_connect.rs:495=UNAUTHORIZED` | **identical**: Buddy reads it as an authentication error and refuses the report | `the_answer_to_a_report_is_what_buddy_reads` |
| a command in the answer to `/p/events` |  | `connect.cpp:697=send_command` | `devices/prusa_connect.rs:500=fn event` | **identical**: Buddy takes a command from any `200` answer; the hub answers events with `204` and sends commands in the answer to telemetry only | `the_events_buddy_sends_settle_commands` |
| `POST /p/register`: body |  | `registrator.cpp:61=sn` | `devices/prusa_connect.rs:134=register` | **identical**: `sn`, `fingerprint`, `printer_type` as the string `2.1.0`, `firmware`, in that order, read; the hub answers `200` with a `Code` header of at most 25 characters | `registration_is_what_registrator_cpp_does` |
| `GET /p/register`: poll |  | `registrator.cpp:219=Accepted` | `devices/prusa_connect.rs:187=ACCEPTED` | **identical**: `202` until added, then `200` with `Token` (at most 20 characters, `printer.hpp:190`), again for as long as the printer has not reported with it (`registrator.cpp:233`); once it has, the code is spent and gets `404` | `registration_is_what_registrator_cpp_does`, `a_mini_registers_by_itself` |
| raw G-code as a command |  | `command.cpp:90=gcode_command` | `devices/prusa_connect.rs:480=text/x.gcode` | **OPEN**: the transport is read as Buddy reads it; the G-code itself (what the hub sends for jog, heat, home; HS10) is not part of this step |  |

## Endpoints it calls

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `/f/<iv>/raw` | both | `planner.cpp:177=/raw` | `devices/printer_files.rs:1139=/f/{iv}/raw` | **OPEN**: the download (Range, `Content-Encryption-Mode: AES-CTR`, decryption, 200/206) is not run against `transfers/download.cpp` in this step |  |
| `/p/events` | both | `connect.cpp:124` | `devices/prusa_connect.rs:104=/p/events` | **identical**: `POST`, `204` | `the_events_buddy_sends_settle_commands` |
| `/p/register` | both | `registrator.cpp:42` | `devices/prusa_connect.rs:102=/p/register` | **identical** | `registration_is_what_registrator_cpp_does` |
| `/p/telemetry` | both | `connect.cpp:121` | `devices/prusa_connect.rs:103=/p/telemetry` | **identical**: `POST`, the answer as described above | `the_answer_to_a_report_is_what_buddy_reads` |
| `/p/ws` | both | `connect.cpp:178` |  | **not used by the hub**: the WebSocket (6.4+; 6.3.0 never asks for it) is later |  |

## Headers it sends

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `Code` | both | `registrator.cpp:107` | `devices/prusa_connect.rs:185=code` | **identical**: read from `GET /p/register` | `registration_is_what_registrator_cpp_does` |
| `Fingerprint` | both |  |  | **not used by the hub**: sent by the printer; the hub reads no `Fingerprint` |  |
| `Sec-WebSocket-Key` | both |  |  | **not used by the hub**: WebSocket (6.4+) |  |
| `Sec-WebSocket-Protocol` | both |  |  | **not used by the hub**: WebSocket (6.4+) |  |
| `Sec-WebSocket-Version` | both |  |  | **not used by the hub**: WebSocket (6.4+) |  |
| `Token` | both | `connect.cpp:138` | `devices/prusa_connect.rs:382=token` | **identical**: names the machine on every report and event | `buddy_takes_every_command_the_hub_sends` |
| `Upgrade` | both |  |  | **not used by the hub**: WebSocket (6.4+) |  |
| `User-Agent-Printer` | 6.3.0 only |  |  | **not used by the hub**: sent by the printer; the hub reads none |  |
| `User-Agent-Version` | 6.3.0 only |  | `devices/prusa_connect.rs:402=user-agent-version` | **OPEN**: read as the firmware's version; the value `6.3.0+0.LOCAL` is from an earlier run on the MINI, not from the firmware's source |  |

## What it reads in an answer (`common/http/resp_parser.cpp`)

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `ApplicationJson` | both | `resp_parser.cpp:42` | `devices/prusa_connect.rs:478=application/json` | **identical**: the hub's JSON command says `application/json` | `buddy_takes_every_command_the_hub_sends` |
| `Body` | both | `resp_parser.cpp:32` |  | **n/a**: the automaton's own state for the body |  |
| `Code` | both | `resp_parser.cpp:54` | `devices/prusa_connect.rs:177=Code` | **identical**: `Code` of the registration answer | `registration_is_what_registrator_cpp_does` |
| `CommandId` | both | `resp_parser.cpp:51` | `devices/prusa_connect.rs:485=Command-Id` | **identical**: `Command-Id` | `the_answer_to_a_report_is_what_buddy_reads` |
| `ConnectionHeader` | both | `resp_parser.cpp:72` |  | **OPEN**: set by the HTTP server (axum); not read against the parser here |  |
| `ConnectionKeepAlive` | both | `resp_parser.cpp:76` |  | **OPEN**: as `ConnectionHeader` |  |
| `ConnectionUpgrade` | both | `resp_parser.cpp:69` |  | **not used by the hub**: WebSocket (6.4+) |  |
| `ContentEncryptionModeCBC` | both | `resp_parser.cpp:79` |  | **not used by the hub**: the hub says `AES-CTR` |  |
| `ContentEncryptionModeCTR` | both | `resp_parser.cpp:82` | `devices/printer_files.rs:1624=content-encryption-mode` | **OPEN**: the hub's download says `AES-CTR`; the download is not run against Buddy here |  |
| `ContentLength` | both | `resp_parser.cpp:36` |  | **OPEN**: set by the HTTP server for answers and by the download; not read against the parser here |  |
| `StatusCode` | both | `resp_parser.cpp:29` |  | **OPEN**: the codes the hub answers are listed above |  |
| `TextGcodeDash` | both | `resp_parser.cpp:48` |  | **not used by the hub**: the hub says `text/x.gcode` |  |
| `TextGcodeDot` | both | `resp_parser.cpp:45` | `devices/prusa_connect.rs:480=text/x.gcode` | **identical**: the hub's G-code command says `text/x.gcode` | `the_answer_to_a_report_is_what_buddy_reads` |
| `Token` | both | `resp_parser.cpp:57` | `devices/prusa_connect.rs:186=Token` | **identical**: `Token` of the registration answer | `registration_is_what_registrator_cpp_does` |
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
| `buttons` | both | `render.cpp:654` | `devices/printer_files.rs:719=buttons` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `canceled` | both |  |  | UNCHECKED |  |
| `chamber` | both |  |  | UNCHECKED |  |
| `chamber_temperature` | master only |  |  | **not in 6.3.0** |  |
| `children` | both | `render.cpp:1023` | `devices/printer_files.rs:467=children` | **identical**: read from where `render.cpp` writes it | `the_listing_follows_file_info_and_file_changed` |
| `chunk` | both |  |  | UNCHECKED |  |
| `class` | master only |  |  | **not in 6.3.0** |  |
| `code` | both | `render.cpp:615` | `devices/printer_files.rs:706=code` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `color_rgba` | master only |  |  | **not in 6.3.0** |  |
| `command` | 6.3.0 only |  |  | UNCHECKED |  |
| `command_id` | both | `render.cpp:251` | `devices/prusa_connect.rs:517=command_id` | **identical**: read from where `render.cpp` writes it | `the_events_buddy_sends_settle_commands` |
| `consumed_weight` | master only |  |  | **not in 6.3.0** |  |
| `data` | both | `render.cpp:340` | `devices/printer_files.rs:685=data` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `dialog_id` | both | `render.cpp:255` | `devices/printer_files.rs:725=dialog_id` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub`, `buddy_takes_every_command_the_hub_sends` |
| `display_name` | both | `render.cpp:454` | `devices/printer_files.rs:749=display_name` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `enabled` | both |  |  | UNCHECKED |  |
| `enclosure` | both |  |  | UNCHECKED |  |
| `end` | both |  |  | UNCHECKED |  |
| `event` | both | `render.cpp:682` | `devices/prusa_connect.rs:510=event` | **identical**: read from where `render.cpp` writes it | `the_events_buddy_sends_settle_commands` |
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
| `job_id` | both | `render.cpp:149` | `devices/printer_files.rs:754=job_id` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
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
| `old_path` | both | `render.cpp:552` | `devices/printer_files.rs:769=old_path` | **identical**: read from where `render.cpp` writes it | `the_listing_follows_file_info_and_file_changed` |
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
| `start_cmd_id` | both | `render.cpp:460` | `devices/printer_files.rs:800=start_cmd_id` | **identical**: read from where `render.cpp` writes it | `transfer_events_name_their_command_in_data` |
| `state` | both | `render.cpp:242` | `devices/printer_files.rs:690=state` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub`, `every_state_buddy_sends_is_read` |
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
| `text` | both | `render.cpp:636` | `devices/printer_files.rs:699=text` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
| `time_in_use` | both |  |  | UNCHECKED |  |
| `time_printing` | both |  |  | UNCHECKED |  |
| `time_remaining` | both |  |  | UNCHECKED |  |
| `time_transferring` | both |  |  | UNCHECKED |  |
| `title` | both | `render.cpp:626` | `devices/printer_files.rs:699=title` | **identical**: read from where `render.cpp` writes it | `state_changed_and_job_info_reach_the_hub` |
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
| `FILE_CHANGED` | both | `planner.cpp:276` | `devices/printer_files.rs:765=FILE_CHANGED` | **identical**: the command (delete, make folder) is finished by it (`planner.cpp:850`); `old_path` takes the file out of the listing | `the_events_buddy_sends_settle_commands`, `the_listing_follows_file_info_and_file_changed` |
| `FILE_EXISTS` | both |  |  | UNCHECKED |  |
| `FILE_INFO` | both | `planner.cpp:262` | `devices/printer_files.rs:736=FILE_INFO` | **identical**: the command is finished; a folder's `children` and `path` fill the hub's listing of the stick | `the_events_buddy_sends_settle_commands`, `the_listing_follows_file_info_and_file_changed` |
| `FINISHED` | both | `planner.cpp:266` | `devices/prusa_connect.rs:526=FINISHED` | **identical**: the command becomes finished | `the_events_buddy_sends_settle_commands` |
| `FIN_OK` | both |  |  | UNCHECKED |  |
| `FIN_STOPPED` | both |  |  | UNCHECKED |  |
| `FOLDER` | both |  |  | UNCHECKED |  |
| `HAS_FILAMENT` | both |  |  | UNCHECKED |  |
| `INFO` | both | `planner.cpp:254` | `devices/prusa_connect.rs:550=INFO` | **identical**: the command is finished and the details are kept (serial, `printer_type` as the string `2.1.0`, firmware, `nozzle_diameter` at the top level, `tools.<n>.material` with `---` read as none, storages, network); keys only master writes are not read | `the_events_buddy_sends_settle_commands`, `info_says_a_dash_for_no_material`, `printer_type_is_read_as_buddys_string_only`, `info_of_6_3_0_is_read_as_buddy_writes_it` |
| `JOB_INFO` | both | `planner.cpp:260` | `devices/printer_files.rs:748=JOB_INFO` | **identical**: the command is finished; `job_id` at the top and `data.display_name` give the file a print is (as the hub's telemetry shows it) | `the_events_buddy_sends_settle_commands`, `state_changed_and_job_info_reach_the_hub` |
| `MMU` | master only |  |  | **not in 6.3.0** |  |
| `MMU3` | master only |  |  | **not in 6.3.0** |  |
| `NETWORK_FAILURE` | both |  |  | UNCHECKED |  |
| `NOT_CALIBRATED` | both |  |  | UNCHECKED |  |
| `NOT_CONNECTED` | both |  |  | UNCHECKED |  |
| `NOT_INITIALIZED` | both |  |  | UNCHECKED |  |
| `NOT_READY` | both |  |  | UNCHECKED |  |
| `NO_FILAMENT` | both |  |  | UNCHECKED |  |
| `NO_TRANSFER` | both |  |  | UNCHECKED |  |
| `PAUSED` | both | `render.cpp:445` | `devices/prusa_connect.rs:211=PAUSED` | **identical**: read as a device state (see States) | `every_state_buddy_sends_is_read` |
| `PRINTING` | both | `render.cpp:443` | `devices/prusa_connect.rs:210=PRINTING` | **identical**: read as a device state (see States) | `every_state_buddy_sends_is_read` |
| `REJECTED` | both | `planner.cpp:258` | `devices/prusa_connect.rs:531=REJECTED` | **identical**: the command becomes rejected; `reason` and `machine_reason` (`render.cpp:327`, `:331`) are kept as the reason. The hub reads `machine_reason` `FILE_EXISTS` to send a file again under a free name, not the text | `the_events_buddy_sends_settle_commands`, `a_free_name_is_decided_by_machine_reason` |
| `STATE_CHANGED` | both | `planner.cpp:280` | `devices/printer_files.rs:689=STATE_CHANGED` | **identical**: `data.code`, `title`, `text`, `buttons` and the top-level `dialog_id` give what the screen asks (and the buttons for `DIALOG_ACTION`); a state without a dialog clears it | `the_events_buddy_sends_settle_commands`, `state_changed_and_job_info_reach_the_hub` |
| `STORAGE_FAILURE` | both |  |  | UNCHECKED |  |
| `TRANSFER_ABORTED` | both | `planner.cpp:272` | `devices/printer_files.rs:799=TRANSFER_ABORTED` | **OPEN**: as `TRANSFER_STOPPED`; the `machine_reason` (`STORAGE_FAILURE`, `NETWORK_FAILURE`) wording is not run by a test here |  |
| `TRANSFER_FINISHED` | both | `planner.cpp:274` | `devices/printer_files.rs:799=TRANSFER_FINISHED` | **identical**: no top-level `command_id` (`render.cpp:537`); the hub settles the download command by `data.start_cmd_id` and marks the transfer finished | `transfer_events_name_their_command_in_data` |
| `TRANSFER_INFO` | both | `planner.cpp:264` | `devices/printer_files.rs:782=TRANSFER_INFO` | **identical**: the download command is finished (taken, the file on its way) | `the_events_buddy_sends_settle_commands`, `transfer_events_name_their_command_in_data` |
| `TRANSFER_IN_PROGRESS` | both |  |  | UNCHECKED |  |
| `TRANSFER_STOPPED` | both | `planner.cpp:270` | `devices/printer_files.rs:799=TRANSFER_STOPPED` | **OPEN**: no top-level `command_id` (`render.cpp:537`); the hub takes the command from `data.start_cmd_id`; the stopped branch (failed transfer, job failed) is not run by a test here |  |
| `USB` | both |  |  | UNCHECKED |  |

## States it sends (`state/printer_state.cpp` `to_str(DeviceState)`)

| Item | In | Buddy 6.3.0 | Hub | Result | Test |
|---|---|---|---|---|---|
| `IDLE` | both | `printer_state.cpp:445` | `devices/prusa_connect.rs:214=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `PRINTING` | both | `printer_state.cpp:447` | `devices/prusa_connect.rs:210=PRINTING` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `PAUSED` | both | `printer_state.cpp:449` | `devices/prusa_connect.rs:211=PAUSED` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `FINISHED` | both | `printer_state.cpp:451` | `devices/prusa_connect.rs:214=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `STOPPED` | both | `printer_state.cpp:453` | `devices/prusa_connect.rs:214=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `READY` | both | `printer_state.cpp:455` | `devices/prusa_connect.rs:214=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `ERROR` | both | `printer_state.cpp:457` | `devices/prusa_connect.rs:213=ERROR` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `BUSY` | both | `printer_state.cpp:459` | `devices/prusa_connect.rs:210=BUSY` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `ATTENTION` | both | `printer_state.cpp:461` | `devices/prusa_connect.rs:212=ATTENTION` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |
| `UNKNOWN` | both | `printer_state.cpp:464` | `devices/prusa_connect.rs:214=_` | **identical**: the name is read and taken; how it is shown (`PRINTING`/`BUSY` busy, the rest idle, paused, attention, error) is the hub's UI, nothing goes to the printer for it | `every_state_buddy_sends_is_read` |

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
| `Abort` | both | `general_response.cpp:9` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Abort_invalidate_test` | both | `general_response.cpp:10` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Adjust` | both | `general_response.cpp:11` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `All` | both | `general_response.cpp:12` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Always` | master only |  |  | **not in 6.3.0** |  |
| `Back` | both | `general_response.cpp:13` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Calibrate` | both | `general_response.cpp:14` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Cancel` | both | `general_response.cpp:15` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Change` | both | `general_response.cpp:17` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Continue` | both | `general_response.cpp:18` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Cooldown` | both | `general_response.cpp:19` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Details` | master only |  |  | **not in 6.3.0** |  |
| `Disable` | both | `general_response.cpp:20` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Docks4` | master only |  |  | **not in 6.3.0** |  |
| `Docks8` | master only |  |  | **not in 6.3.0** |  |
| `Done` | both | `general_response.cpp:21` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Empty` | master only |  |  | **not in 6.3.0** |  |
| `FS_disable` | both | `general_response.cpp:25` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Filament` | 6.3.0 only | `general_response.cpp:22` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Filament_removed` | 6.3.0 only | `general_response.cpp:23` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Finish` | both | `general_response.cpp:24` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Heatup` | both | `general_response.cpp:52` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Help` | both | `general_response.cpp:16` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Ignore` | both | `general_response.cpp:26` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Left` | both | `general_response.cpp:27` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Load` | both | `general_response.cpp:28` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `MMU_disable` | both | `general_response.cpp:29` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Never` | both | `general_response.cpp:30` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Next` | both | `general_response.cpp:31` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `No` | both | `general_response.cpp:32` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `NotNow` | both | `general_response.cpp:33` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Ok` | both | `general_response.cpp:34` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Other` | master only |  |  | **not in 6.3.0** |  |
| `PRINT` | 6.3.0 only | `general_response.cpp:54` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Pause` | both | `general_response.cpp:35` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Postpone5Days` | both | `general_response.cpp:53` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Print` | both | `general_response.cpp:36` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Purge_more` | both | `general_response.cpp:37` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Quit` | both | `general_response.cpp:38` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Reheat` | both | `general_response.cpp:39` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Remap` | master only |  |  | **not in 6.3.0** |  |
| `Remove` | both | `general_response.cpp:41` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Replace` | both | `general_response.cpp:40` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Restart` | both | `general_response.cpp:42` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Resume` | both | `general_response.cpp:43` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Retry` | both | `general_response.cpp:44` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Right` | both | `general_response.cpp:45` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Skip` | both | `general_response.cpp:46` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Slowly` | both | `general_response.cpp:47` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `SpoolJoin` | both | `general_response.cpp:48` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Stop` | both | `general_response.cpp:49` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Tool1` | 6.3.0 only | `general_response.cpp:55` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Tool2` | 6.3.0 only | `general_response.cpp:56` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Tool3` | 6.3.0 only | `general_response.cpp:57` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Tool4` | 6.3.0 only | `general_response.cpp:58` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Tool5` | 6.3.0 only | `general_response.cpp:59` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Unload` | both | `general_response.cpp:50` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |
| `Yes` | both | `general_response.cpp:51` | `web/machines.rs:482=button` | **identical**: sent back by the name the printer gave (`render.cpp:661`), taken by Buddy's `from_str` (it compares the whole name, case counts) | `every_dialog_answer_of_6_3_0_goes_back_unchanged` |

## Differences (each decided by the owner)

| # | Buddy 6.3.0 | Ours | What differs | Proposed fix |
|---|---|---|---|---|
| D6 | Nothing in Connect's commands closes the print preview that opens after a download (`transfers/transfer.cpp:423-430`, `print_begin` shows the preview, state `PrintPreviewImage`, which `printer_state.cpp:267-272` reports as IDLE or READY). `STOP_PRINT` (`marlin_printer.cpp:454-457`) is accepted only for Paused, Printing or Attention, and answers REJECTED "No print to stop" (`planner.cpp:687`); `SET_IDLE` (`marlin_printer.cpp:577-584`) only for Finished or Stopped; `DIALOG_ACTION` needs a `dialog_id`, which `render.cpp:255` writes only while the state has a dialog (`printer_state.cpp:355-367`: the preview itself has none; only its questions have). The preview is closed by Buddy's own screen (`print_abort` in a preview state, `marlin_server.cpp:1244-1251`), and `M604` reaches the same call (`src/marlin_stubs/host/M604.cpp:17`) | `devices/printer_files.rs:1027` | After a download that is not printed the hub sends `M604` as G-code to close the preview (its own comment: "WORKAROUND, NEEDS TRACING"). Buddy's Connect offers no command for it; `M604` is G-code, not a Connect command | **Ours, approved by the owner (2026-11-17, "D6 A")**: kept: after a download only to the stick, while the printer reports IDLE and nothing else waits, the hub sends `M604` (Buddy's abort, which in the preview acts as its Back button; tried on the MINI 2026-11-08). Buddy 6.3.0 gives Connect no other way |

## What we send or read that the firmware does not

| Item | Where | Why | Status |
|---|---|---|---|
| `/p/`, `/f/` | `prusa_connect.rs` | The hub's own route prefixes (`starts_with`, which requests to log), not endpoints | n/a |
| `Content-Type` | `prusa_connect.rs` | The header whose value the firmware reads (`ApplicationJson`, `TextGcodeDot`, `TextGcodeDash`) | see What it reads in an answer |

## Not covered

- The WebSocket (`/p/ws`), `START_INLINE_DOWNLOAD`, `START_CONNECT_DOWNLOAD` (6.4+).
- The encrypted download (`/f/<iv>/raw`): Range, 200/206, `Content-Encryption-Mode`, the decryptor of `transfers/download.cpp`.
- The telemetry's fields and the INFO event's data beyond what the tests above read; G-code metadata; the
  transfer events' failure branches (`TRANSFER_STOPPED`, `TRANSFER_ABORTED`).
- The G-code the hub sends as `text/x.gcode` (the MINI's menu G-code is HS10).
- Buddy's C++ is not run: every "identical" is against a port of its parser and fixtures written from its
  serializer, not against its own output. TLS, HTTP connection reuse and timing of the printer.
