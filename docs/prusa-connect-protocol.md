# The printer side of Prusa Connect

How a Prusa printer with the Buddy firmware (MINI, MK4, XL) talks to Connect, so that a
Maker Connect hub can be its server instead. Learned by reading Prusa-Firmware-Buddy
(`src/connect`, GPL-3.0) and the Prusa Connect SDK (Freeware); written here in our own
words, nothing copied. Point a printer at a Maker Connect hub with the settings file
described in [printer-protocols.md](printer-protocols.md).

## Registering: a code on the printer's screen

1. `POST /p/register`, JSON: `sn` (serial number), `fingerprint`, `printer_type` (type,
   version and subversion: `2.1.0` for a MINI) and `firmware`.
2. The server answers `200` with a header `Code: <code>`. The printer shows the code.
3. The printer asks `GET /p/register` with the header `Code: <code>` until it is added:
   `202` (or `204`) while it waits, then `200` with a header `Token: <token>`.

Maker Connect uses its own pairing code as that code: the owner enters it on the website,
as for an agent.

## Afterwards

Every request carries the headers `Fingerprint` and `Token`, and `User-Agent-Printer`
(`Buddy_MINI`) and `User-Agent-Version` (the firmware: `6.3.0+0.LOCAL`). **A token is at
most 19 characters**: the firmware keeps 20 bytes with the closing zero, and a longer
token does not work (seen on a MINI: every telemetry refused).

Seen from a MINI on 6.3.0: telemetry every second or so (`state`, `temp_nozzle`,
`temp_bed`, targets, axes, `speed`, `flow`, `material`; now and then only `state`), and an
`INFO` event on start and after a while again, with `sn`, `firmware`, `printer_type`,
`nozzle_diameter`, `network_info`, `storages` (the USB stick and its free space), `tools`,
and its Prusa Link `api_key` (a secret: the hub's log leaves it out).

- `POST /p/telemetry`, JSON, every few seconds: `state` (`IDLE`, `BUSY`, `PRINTING`,
  `PAUSED`, `FINISHED`, `STOPPED`, `ERROR`, `ATTENTION`, `READY`), `temp_bed`,
  `target_bed`, the nozzle's `nozzle_temp` per tool (older firmware: `temp_nozzle`),
  `progress`, `time_remaining`, `time_printing`, axes, speed, flow, fans.
- `POST /p/events`, JSON: `event` (`INFO`, `STATE_CHANGED`, `JOB_INFO`, ...), with what
  it is about.
- `GET /p/ws`, an upgrade to a WebSocket with subprotocol `prusa-connect`: newer firmware
  takes its commands there (`SEND_INFO`, `START_PRINT` with a path, `PAUSE_PRINT`,
  `STOP_PRINT`, ...). Probably the "change in communication" for which Connect needs
  firmware 6.4.0 or later. Older firmware takes a command in the reply to its telemetry.

## Commands (firmware 6.3.0)

Firmware 6.3.0 takes its commands in the answer to its telemetry, not over the WebSocket
(it never asks for `/p/ws`): `200` with the headers `Command-Id: <number>` and
`Content-Type: text/x.gcode`, and G-code lines as the body (the answer may be 512 bytes at
most). It runs the lines in the background, one command at a time, and tells how it went
with events carrying `command_id`: `ACCEPTED`, then `FINISHED` (or `REJECTED` with a
`reason`). JSON commands (`application/json`, `{"command": "PAUSE_PRINT", ...}`) come the
same way. Tried on a MINI: G28, relative G1 moves on each axis, M106/M107.

Fan speeds (`fan_print`, `fan_extruder`) are in the telemetry only while a job runs.

## Files on its USB stick (firmware 6.3.0)

Read in Prusa-Firmware-Buddy 6.3.0 (GPL; nothing copied). JSON commands, in the answer to
telemetry like G-code, `Content-Type: application/json`, `{"command": ..., "kwargs": {...}}`:

- `SEND_FILE_INFO {path}`: a `FILE_INFO` event with `command_id`. A folder has `children`:
  `name` (the short 8.3 name: use it in paths), `display_name` (the long one), `size`,
  `m_timestamp`, `read_only` (still arriving), `type` (`PRINT_FILE`, `FOLDER`, `FIRMWARE`,
  `FILE`). A file has `preview`: a PNG thumbnail from the G-code, base64, the first larger
  than 17×17. It also has the G-code's metadata as fields, for example `estimated printing
  time (normal mode)`, `filament_type`, `filament used [g]`.
- `START_PRINT {path}`: `JOB_INFO`, or `REJECTED` with a `reason` (`File not found`).
- `DELETE_FILE`, `DELETE_FOLDER`, `CREATE_FOLDER {path}`: `FILE_CHANGED` with `new_path` or
  `old_path`.
- `START_ENCRYPTED_DOWNLOAD {path, key, iv, orig_size}` (key and iv 16 bytes, as hex):
  - The printer fetches `GET /f/<iv as hex>/raw` from the host and port it reports to.
    It sends no token, only `Content-Encryption-Mode: AES-CTR` and `Range: bytes=N-`
    (N a multiple of 16, to go on after a break).
  - It wants `200` from the start, `206` from further on, and a `Content-Length`.
  - The file is AES-128 in CTR mode: the counter block is the iv's first 12 bytes, with the
    block number big-endian in the last 4, from 0.
  - Padded to whole blocks; `orig_size` cuts it off.
  - Afterwards `TRANSFER_FINISHED`, or `TRANSFER_ABORTED` or `TRANSFER_STOPPED`, with
    `start_cmd_id`. A path outside `/usb`, or a file that is not G-code or firmware, is
    refused.

## In Maker Connect

`crates/hub/src/prusa_connect.rs` answers register, telemetry and events: the printer
shows in the list with its state, temperatures and progress, its profile recognised from
its printer type, as "Link: Prusa firmware ... (built-in Connect)". Commands go back in the
answer to telemetry. The machine page's **USB** tab and `crates/hub/src/printer_files.rs`
cover the stick:
- looking through it, with each file's picture and details
- printing from it, deleting, and making a folder
- sending a file of the space to it

Printing a file of the space sends it to the stick first, then `START_PRINT`; the job
follows the printer's state to its end. Still to do: the WebSocket (newer firmware).

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
