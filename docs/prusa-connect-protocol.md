# The printer side of Prusa Connect

> **Not verified (2026-11-09).** This document was written by reading the sources by hand,
> not checked item by item. The check against PrusaLink (`prusalink-audit.md`,
> `scripts/prusalink-audit-check.py`) found many gaps that such a reading had missed. Until a
> script checks this document against its sources in the same way, it is not to be taken as
> complete.

How a Prusa printer with the Buddy firmware (MINI, MK4, MK3.5, XL, CORE One) talks to
Connect, so that a Maker Connect hub can be its server instead. Learned by reading
Prusa-Firmware-Buddy (`src/connect`, GPL-3.0) and the Prusa Connect SDK (Freeware); written
here in our own words, nothing copied. Firmware 6.3.0 and master of 2026-11-03 compared
(`reference/buddy-6.3.0`, `reference/Prusa-Firmware-Buddy`), checked on 2026-11-08. Point a
printer at a Maker Connect hub with the settings file described in
[printer-protocols.md](printer-protocols.md) (and below).

## Two transports

| Firmware | How it talks | Files to the printer |
|---|---|---|
| up to 6.3.x | HTTP polling: `POST /p/telemetry`, the next command in the answer; `POST /p/events` | `START_ENCRYPTED_DOWNLOAD`: the printer fetches `GET /f/<iv>/raw` from the hub |
| 6.4.0 and later | One WebSocket, `GET /p/ws` | `START_INLINE_DOWNLOAD`: chunks inside the WebSocket |

- 6.4.0 (release notes): "Communication between the printer and Prusa Connect is now handled
  using WebSockets, replacing the previous polling-based method." The MINI is in 6.4.0; its
  latest firmware is 8.0.1 (2026-11-03). In 6.3.0 the WebSocket code was there but switched
  off ("the server part is not ready"); in master it is on for every printer but the iX.
- In a WebSocket build the HTTP polling **and** `START_ENCRYPTED_DOWNLOAD` are compiled out.
  A printer on 6.4 or later cannot use `/p/telemetry`, `/p/events` or `/f/<iv>/raw`.
- Prusa announced that firmware older than 6.4.0 will no longer work with Prusa Connect. The
  hub keeps speaking both.

## Registering: a code on the printer's screen

The same in both transports (plain HTTP, also on 6.4 and later):

1. `POST /p/register`, JSON: `sn` (serial number), `fingerprint`, `printer_type` (type,
   version and subversion: `2.1.0` for a MINI) and `firmware`.
2. The server answers `200` with a header `Code: <code>`. The printer shows the code.
3. The printer asks `GET /p/register` with the header `Code: <code>` until it is added:
   `202` (or `204`) while it waits, then `200` with a header `Token: <token>`.

Maker Connect uses its own pairing code as that code: the owner enters it on the website,
as for an agent.

Afterwards every request carries the headers `Fingerprint` and `Token`; before 6.4 also
`User-Agent-Printer` (`Buddy_MINI`) and `User-Agent-Version` (`6.3.0+0.LOCAL`), dropped since.
**A token is at most 19 characters**: the firmware keeps 20 bytes with the closing zero, and
a longer token does not work (seen on a MINI: every telemetry refused).

## HTTP polling (before 6.4)

- `POST /p/telemetry`, JSON, every 4–5 s, every second while things change. The answer
  carries the next command: `200` with `Command-Id: <number>` and
  `Content-Type: text/x.gcode` (G-code lines) or `application/json`
  (`{"command": ..., "kwargs": {...}}`), at most 512 bytes; `204` when there is none.
- `POST /p/events`, JSON: `event` with what it is about, `command_id` when it answers one.
- Firmware 6.3.0 never asks for `/p/ws`. Over HTTP a command's output does not come back.

## The WebSocket (6.4 and later)

**Handshake.** `GET /p/ws` with `Token`, `Fingerprint`, `Upgrade: websocket`,
`Connection: upgrade`, `Sec-WebSocket-Key`, `Sec-WebSocket-Version: 13`,
`Sec-WebSocket-Protocol: prusa-connect`. The printer needs `101` with
`Sec-WebSocket-Protocol: prusa-connect`, `Upgrade: websocket`, `Connection: Upgrade` and the
right `Sec-WebSocket-Accept`, else it gives up. `401`/`403`: bad token (it shows an auth
error); `400`: internal error; anything else: server error.

**Limits of the printer's side** (`src/common/http/websocket.hpp`):
- no extensions (no compression), frames from the server not masked;
- no single fragment over 65 535 bytes in either direction;
- no proper close; the server must not send before the printer's first message.

**Printer → server:** text messages, JSON, possibly in several fragments. Which kind by its
fields, there is no envelope:
- has `"event"`: an event, as over HTTP;
- has `"transfer": "inline"`: asks for chunks of a file (below);
- else: telemetry.

It pings now and then and answers pings; pongs are ignored.

**Server → printer:** each message starts with 9 bytes: one letter, then a 32-bit id as 8 hex
digits (no `0x`), then the body.

| Letter | Body | Id |
|---|---|---|
| `J` | a JSON command (`{"command": ..., "kwargs": ...}`) | the command id |
| `G` | G-code | the command id |
| `F` | G-code, "forced" (not told apart from `G` yet) | the command id |
| `T` | raw bytes of a file being sent; empty = the server gives up | the transfer's `file_id` |
| `D` | debug text, only logged | any |

- A command body over 512 bytes is refused ("Oversized command").
- While one command is still being carried out, another `J`/`G`/`F` is refused: one at a
  time, as with polling.
- Commands may be sent at any moment, not only after telemetry.

**Telemetry timing:** at least every 15 s, every 5 s while things change, never more often
than every 2 s; a full one (all values) every 5 minutes, otherwise often only what changed.

## Files: encrypted over HTTP (before 6.4)

`START_ENCRYPTED_DOWNLOAD {path, key, iv, orig_size}` (key and iv 16 bytes, as hex):
- The printer fetches `GET /f/<iv as hex>/raw` from the host and port it reports to. It
  sends no token, only `Content-Encryption-Mode: AES-CTR` and `Range: bytes=N-` (N a
  multiple of 16, to go on after a break).
- It wants `200` from the start, `206` from further on, and a `Content-Length`.
- The file is AES-128 in CTR mode: the counter block is the iv's first 12 bytes, with the
  block number big-endian in the last 4, from 0. Padded to whole blocks; `orig_size` cuts
  it off.
- A path outside `/usb`, or a file that is not G-code or firmware, is refused.

## Files: inline over the WebSocket (6.4 and later)

1. Server: `J` + `{"command":"START_INLINE_DOWNLOAD","kwargs":{"path":"/usb/name.bgcode",
   "hash":..., "team_id":..., "orig_size":...}}`. `hash` (up to 28 characters) and `team_id`
   are the server's own: the printer only gives them back.
2. Printer: `{"transfer":"inline","hash":...,"team_id":...,"transfer_id":...,"chunk":4096,
   "file_id":N,"start":0,"end":E}`; `hash`, `team_id` and `transfer_id` only in the first
   request of a download. `end` is inclusive. `file_id` is random, per download.
3. Server: `T` + `file_id` (8 hex) + the bytes from `start`, in pieces of at most `chunk`
   bytes, up to `end`.
4. The printer asks for the next segment once one is written, until the file is complete.

No encryption of its own: the WebSocket's TLS (or the LAN) carries it. A broken connection
resets the transfer.

Either way the end is `TRANSFER_FINISHED` (or `TRANSFER_ABORTED` / `TRANSFER_STOPPED`, with
`machine_reason` such as `STORAGE_FAILURE`), with `start_cmd_id`.

### The print preview a download opens (open question)

Read in Buddy 6.3.0 and 9.0.0 (`transfers/transfer.cpp`), seen on the MINI 2026-11-08:

- A download from Connect opens the file's print preview ("one-click print", PRINT / BACK)
  while it is still arriving: at the first save of its progress, when the printer is idle
  and has a screen. It is meant for print-while-downloading. The SDK's `printing` and
  `selecting` flags are not read by Buddy, so the server cannot ask for no preview.
- While the preview shows, the file is open, so the finished download cannot be moved into
  place: the printer keeps listing it `read_only`, and refuses to delete it ("File is being
  transferred"), until the preview is closed. `START_PRINT` is taken meanwhile.
- The preview sends no `dialog_id` (not seen on the MINI), so `DIALOG_ACTION` cannot reach
  it. `M604` closes it (`Aborting_Preview`, as its Back button; tried on the MINI); in a
  print, `M604` stops the print.
- For comparison: an upload from PrusaSlicer (the printer's own PrusaLink) opens the preview
  only after the file is complete and in place; PrusaLink (an MK3) does nothing for an
  upload only.

**Not known: how Prusa Connect handles this.** In years of use on Prusa Connect, files sent
to a MINI were never stuck. To trace: a MINI on Prusa Connect, upload only, and what the
printer shows and reports. Until then the hub closes the preview with `M604` after a download
only to the stick, as a **workaround** (only answering an `IDLE` report with no other command
waiting); see `on_report` in `crates/hub/src/printer_files.rs`.

## Commands

The same list in 6.3.0 and master: `SEND_INFO`, `SEND_JOB_INFO`, `SEND_FILE_INFO`,
`SEND_TRANSFER_INFO`, `SEND_STATE_INFO`, `PAUSE_PRINT`, `STOP_PRINT`, `RESUME_PRINT`,
`START_PRINT`, `SET_PRINTER_READY`, `CANCEL_PRINTER_READY`, `SET_IDLE`, `DELETE_FILE`,
`DELETE_FOLDER`, `CREATE_FOLDER`, `STOP_TRANSFER`, `SET_TOKEN`, `RESET_PRINTER` (`RESET`),
`DIALOG_ACTION`, `SET_VALUE`, `CANCEL_OBJECT`, `UNCANCEL_OBJECT`, `START_INLINE_DOWNLOAD`
(`START_CONNECT_DOWNLOAD`), and only before 6.4 `START_ENCRYPTED_DOWNLOAD`.

- Paths are under `/usb`, in 8.3 form as the listings give them. Master refuses a path
  longer than the firmware allows ("Path too long").
- `CANCEL_OBJECT` takes 16-bit object ids in master (8-bit in 6.3).
- `SET_VALUE`'s per-tool keys count tools from 0 in master (`tools.0.*`).
- **The same id twice is refused** in master ("Won't execute the same command multiple
  times"): ids must never repeat. The hub starts its ids from the clock.

G-code runs in the background, one command at a time: `ACCEPTED`, then `FINISHED` (or
`REJECTED`). Tried on a MINI: G28, relative G1 moves on each axis, M106/M107.

## How a command is answered

Every answer carries the command's `command_id`. A command is done when any of these comes:

| Event | For |
|---|---|
| `ACCEPTED` | G-code: taken on, runs in the background; then `FINISHED` or `FAILED` |
| `FINISHED` | `PAUSE_PRINT`, `RESUME_PRINT`, `STOP_PRINT`, `CANCEL_PRINTER_READY`, `SET_IDLE`, `STOP_TRANSFER`, `SET_TOKEN`, `DIALOG_ACTION`, `SET_VALUE`; `SET_PRINTER_READY` before 6.4 |
| `REJECTED` (with `reason`), `FAILED` | refused, or taken on and failed |
| `FILE_INFO` | `SEND_FILE_INFO` |
| `FILE_CHANGED` | `DELETE_FILE`, `DELETE_FOLDER`, `CREATE_FOLDER`: **no `FINISHED`** |
| `JOB_INFO` | `SEND_JOB_INFO`, `START_PRINT` (or `REJECTED`: `File not found`) |
| `INFO` | `SEND_INFO` |
| `TRANSFER_INFO` | `SEND_TRANSFER_INFO`; a download's start (`START_*_DOWNLOAD`), with `start_cmd_id` |
| `STATE_CHANGED` | `SEND_STATE_INFO`; `SET_PRINTER_READY` from 6.4 (was `FINISHED`) |
| `CANCELABLE_CHANGED` | `CANCEL_OBJECT`, `UNCANCEL_OBJECT` |
| nothing | `RESET_PRINTER` when it works: the printer restarts |

- A download's `TRANSFER_INFO` only means it started: the file is on the stick at
  `TRANSFER_FINISHED`. Refused: `Another transfer in progress`, `File already exists`, or a
  storage error (with `machine_reason`).
- Pause, resume and stop refuse with `No print to pause`, `No paused print to resume`,
  `No print to stop`.
- `REJECTED` **without** a reason: busy with another command; send it again later. The same
  id while it is still being carried out: `ACCEPTED` again.
- While G-code runs in the background, every telemetry carries its `command_id`: it is still
  going, however long it takes (a heat-up with `M109`).

## Events

The same in both: `ACCEPTED`, `REJECTED`, `FAILED`, `FINISHED`, `INFO`, `STATE_CHANGED`,
`JOB_INFO`, `FILE_INFO`, `FILE_CHANGED`, `TRANSFER_INFO`, `TRANSFER_FINISHED`,
`TRANSFER_ABORTED`, `TRANSFER_STOPPED`, `CANCELABLE_CHANGED`.

- `INFO` comes on start and now and then again: `sn`, `firmware`, `printer_type`,
  `network_info`, `storages` (the USB stick and its free space), `tools`, and the Prusa
  Link `api_key` (a secret: the hub's log leaves it out).
- `STATE_CHANGED`: while it asks someone (`ATTENTION`) or is in error, what is on its
  screen: `code`, `title`, `text`, buttons; telemetry carries a `dialog_id` then.
- `FILE_CHANGED`: `data.old_path` (deleted), `data.new_path` (made), `data.new_path` with
  `rescan: true` (several things changed at once: read the stick again); paths in 8.3 form;
  also `free_space`.
- A folder's `FILE_INFO` lists each child with `name` (8.3: use it in paths), `display_name`
  (the long name), `size`, `m_timestamp`, `read_only` and `type` (`PRINT_FILE`, `FOLDER`,
  `FIRMWARE`, `FILE`). **`read_only: true` = still arriving**: it cannot be printed or
  deleted yet ("File is being transferred"). A file's `FILE_INFO` has `preview` (a PNG
  thumbnail from the G-code, base64, the first larger than 17×17) and the G-code's metadata
  as fields (`estimated printing time (normal mode)`, `filament_type`, `filament used [g]`).

## Telemetry and INFO: old and new layout

States: `IDLE`, `BUSY`, `PRINTING`, `PAUSED`, `FINISHED`, `STOPPED`, `ERROR`, `ATTENTION`,
`READY`. Now and then a telemetry has only `state`: what it leaves out is as it was.

| | Before 6.4 | 6.4 and later (more in 8.0) |
|---|---|---|
| Nozzle | `temp_nozzle`, `target_nozzle` | `tools.<n>.nozzle_temp`, `nozzle_target_temp` (null when off) |
| Fans | `fan_print`, `fan_hotend` / `fan_extruder`, only while printing | `tools.<n>.print_fan_rpm`, `heatbreak_fan_rpm` |
| Which head | – | `tools.<n>.active`, `type: FFF_PRINT_HEAD`; tools numbered from `"0"` |
| Bed, speed, flow, axes | `temp_bed`, `target_bed`, `speed`, `flow`, `axis_*` | the same (`axis_x`/`axis_y` not while printing) |
| Job | `job_id`, `progress`, `time_printing`, `time_remaining` | the same, plus `filament_change_in` |
| Transfer | – | `transfer_id`, `transfer_transferred`, `transfer_progress`, `transfer_time_remaining` |
| INFO nozzle | `nozzle_diameter` | `tools.<n>.features.nozzle_diameter`, `nozzle_hardened`, `nozzle_high_flow` (top-level `nozzle_diameter` kept, deprecated) |
| INFO material | `tools.<n>.material` | `tools.<n>.material_package_instance.package.material`: `abbreviation`, `primary_color.color_rgba`, `tags`, `properties` (temperatures) |

## Every Buddy printer

From Buddy's own table (`include/common/printer_model_data.hpp`, 2026-11-03): the type it
registers with Connect, its `M862.2` code (MMU variant: 3 × 10000 more), its name, its
USB PID (the first two digits of its error codes), and the hub's profile.

| Printer | Connect type | M862.2 | Name | PID | Profile |
|---|---|---|---|---|---|
| MINI / MINI+ | 2.1.0 | 120 | MINI | 12 | `prusa/mini` |
| MK3.5 | 1.3.5 | 230 | MK3.5 | 23 | `prusa/mk3.5` |
| MK3.5S | 1.3.6 | 280 | MK3.5S | 28 | `prusa/mk3.5s` |
| MK3.9 | 1.3.9 | 210 | MK3.9 | 21 | `prusa/mk3.9` |
| MK3.9S | 1.3.10 | 270 | MK3.9S | 27 | `prusa/mk3.9s` |
| MK4 | 1.4.0 | 130 | MK4 | 13 | `prusa/mk4` |
| MK4S | 1.4.1 | 260 | MK4S | 26 | `prusa/mk4s` |
| XL | 3.1.0 | 170 | XL | 17 | `prusa/xl` |
| XL Development Kit | 5.1.0 | 170 | XL | 17 | `prusa/xl` |
| XL+ | 3.1.1 | 410 | XLP | 41 | `prusa/xl-plus` |
| iX | 4.1.0 | 160 | iX | 16 | `prusa/ix` |
| CORE One | 7.1.0 | 310 | COREONE | 31 | `prusa/coreone` |
| CORE One L | 8.1.0 | 350 | COREONEL | 35 | `prusa/coreone-l` |
| CORE One INDX | 7.10.0 | 360 | COREONEINDX | 36 | `prusa/coreone-indx` |
| CORE One L INDX | 8.10.0 | 370 | COREONEL-INDX | 37 | `prusa/coreone-l-indx` |
| Signature Oak | 7.2.0 | 380 | COREONEOAK | 38 | `prusa/signature-oak` |
| MK3 / MK3S (Prusa-Firmware; PrusaLink's SDK types) | 1.3.0 / 1.3.1 | 300 / 302 | MK3 / MK3S | – | `prusa/i3-mk3`, `prusa/mk3s` |
| MK2.5 / MK2.5S (the same) | 1.2.5 / 1.2.6 | 250 / 252 | | – | `prusa/mk2.5`, `prusa/mk2.5s` |

What they report besides the MINI's, and the hub reads: the CORE One's `chamber` (temperature
and target; 0 is none), the XL's `enclosure`, several heads under `tools` (the XL up to five,
an INDX four or eight; each shown, the active one marked). PrusaSlicer's settings (2.5.14) have
no profile yet for the CORE One L INDX and the iX: their printer model is Buddy's own name.

## Error codes (Prusa-Error-Codes)

`reference/Prusa-Error-Codes` (GPL-3.0, 2026-11-06). A code is five digits `XXYZZ`:

- `XX` the printer (its USB PID): 04 MMU, 10 SL1, 12 MINI, 13 MK4, 16 iX, 17 XL, 21 MK3.9,
  23 MK3.5, 26 MK4S, 27 MK3.9S, 28 MK3.5S, 29 M1, 31 CORE One, 35 CORE One L, 36 CORE One
  INDX, 37 CORE One L INDX, 38 Signature Oak, 41 XL+;
- `Y` the category: 1 mechanical, 2 temperature, 3 electronics, 4 connection, 5 system,
  6 bootloader, 7 warning, 8 a dialog for Connect (not an error);
- `ZZ` the error. Each has a page at `prusa.io/<code>`, as the printer's screen shows.

Buddy sends the code with its own title and text in `STATE_CHANGED` (`code`, `title`, `text`,
buttons); the hub shows them with `(#<code>, prusa.io/<code>)`. The list's texts are not
copied: the printer sends its own. The MK3 shows MMU errors (`04xxx`) only on its screen; the
agent reads them from its MMU chatter (docs/marlin-serial-protocol.md).

## Prusa's Connect SDK (PrusaLink, an MK3 behind a Pi)

`reference/Prusa-Connect-SDK-Printer` (2026-11-03) is the printer side PrusaLink uses. It
speaks HTTP polling only (no WebSocket) and differs from Buddy:

- Headers: `Fingerprint`, `Token`, `Timestamp`, `User-Agent` (`Prusa-Connect-SDK-Printer/…`),
  `User-Agent-Printer`, `User-Agent-Version`, `Clock-Adjusted: 1` after its clock jumped.
- Registration as Buddy's. `INFO` gives the printer type as `type`, `version`, `subversion`
  (1, 3, 1 for an MK3S) and also: `sdk`, `files` (the whole tree: `type` `DIR`/`FILE`,
  `name`, `size`, `m_timestamp`, `read_only`, `children`), `mbl` (its bed mesh),
  `sheet_settings`, `active_sheet`, `mmu`, `slots`.
- **Every command is answered `ACCEPTED` first**, the result later (`FINISHED`, `FAILED`,
  `REJECTED`, or the event it asked for). While one runs, another is `REJECTED` ("Another
  command is running"); the same id again gets its last state again; telemetry carries the
  running command's `command_id`.
- Commands: the JSON may carry `args` as well as `kwargs`; G-code comes as `text/x.gcode`,
  `Force: 1` to run it during a print. It knows `GCODE`, `UPGRADE`, `START_URL_DOWNLOAD`
  (`url`, `path`), `START_CONNECT_DOWNLOAD` (`team_id`, `hash`, `path`, `printing`,
  `selecting`), `LOAD_FILAMENT`, `UNLOAD_FILAMENT`, `SLOT_ACTION`, and the aliases
  `DELETE_DIRECTORY`, `CREATE_DIRECTORY`; **not `START_ENCRYPTED_DOWNLOAD`**.
- `SEND_FILE_INFO` on a folder is refused ("FILE_INFO doesn't work for folders"): folders come
  in `INFO`'s `files`. Deleting and making folders answer `FINISHED`, not `FILE_CHANGED`. A
  file's `FILE_INFO` has its metadata (gcode-metadata's keys) and `preview`, the biggest
  thumbnail.
- `START_CONNECT_DOWNLOAD`: the printer fetches `GET /p/teams/<team_id>/files/<hash>/raw`
  from the server with its own headers (`Token`, `Fingerprint`); it wants `200`, a
  `Content-Type` of `text/plain`, `text/x.gcode`, `application/binary` or
  `application/octet-stream`, and a `Content-Length`; no ranges, no encryption.
- Events it adds: `MEDIUM_INSERTED`, `MEDIUM_EJECTED`, `MESH_BED_DATA`, `SLOT_EVENT`. Event
  `source`s: `CONNECT`, `GUI`, `WUI`, `SERIAL`, `GCODE`, `MARLIN`, `FIRMWARE`, `HW`, `USER`,
  `SLOT`. Job states `PRINTING`, `PAUSED`, `FIN_OK`, `FIN_STOPPED`, `FIN_ERROR`,
  `FIN_HARVESTED`.
- Telemetry: the flat layout (`temp_nozzle`, `target_nozzle`, `temp_bed`, `target_bed`,
  `axis_*`, `fan_extruder`, `fan_hotend`, `fan_print`, `progress`, `time_printing`,
  `time_remaining`, `speed`, `flow`, `material`), plus `filament_change_in`, `odometer_*`,
  `total_filament`, `total_print_time`, `inaccurate_estimates`, `slot`, `transfer_*`.
- Cameras: `POST /p/camera` registers one (its token in the answer), snapshots go to
  `/c/snapshot`.

The hub reads its `INFO` and telemetry, settles its commands, and reads the stick again on
`MEDIUM_*`. **Not built:** its file tree from `INFO` (the USB page asks folder by folder),
`START_CONNECT_DOWNLOAD` and `/p/teams/…/raw` (sending files to it), cameras. An MK3 is
connected through the agent instead (docs/marlin-serial-protocol.md).

## In Maker Connect

`crates/hub/src/prusa_connect.rs` answers register, telemetry and events, and reads both
layouts (`to_telemetry`, `to_details`): the printer shows in the list with its state,
temperatures and progress, its profile recognised from its printer type, as "Link: Prusa
firmware ... (built-in Connect)". Every answer event in the table above settles its command;
a command whose id is in the telemetry is kept waiting for, not given up after two minutes.
The machine page's **USB** tab and `crates/hub/src/printer_files.rs` cover the stick:
- looking through it, with each file's picture and details
- printing from it, deleting (shown at once, the folder kept right from `FILE_CHANGED`; a
  file still arriving cannot be deleted), and making a folder
- sending a file of the space to it

Printing a file of the space sends it to the stick first, then `START_PRINT`; the job
follows the printer's state to its end.

**Still to do:**
- The WebSocket (`/p/ws`) for 6.4 and later, beside the HTTP polling for older firmware.
- Inline downloads over it (`START_INLINE_DOWNLOAD`), beside the encrypted HTTP ones.
- `DIALOG_ACTION`, `CANCEL_OBJECT`, `SET_VALUE`, `STOP_TRANSFER`: not used yet.

## Pointing a MINI here

A file `prusa_printer_settings.ini` in the root of the printer's USB stick:

```ini
[service::connect]
hostname = 192.168.1.20
port = 8460
tls = false
```

`hostname` is the machine the hub runs on. On the printer: Settings → Network → Prusa
Connect → Load Settings, then Add printer to Connect: the printer shows a code; enter it on
the website. Loading the file replaces all Connect settings, including the token: to go back
to Prusa Connect, load its settings again (`buddy-a.connect.prusa3d.com`, port 443, tls
true) and add the printer there anew.
