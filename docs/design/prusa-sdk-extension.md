# Prusa Connect SDK: a proposed extension for devices that are not Prusa printers

**Status: proposal.** Nothing on this page is in Prusa-Connect-SDK-Printer, PrusaLink or Prusa Connect.
It is an addition on top of the SDK's protocol. That protocol is described in [wire.md](wire.md), which
follows the SDK at `c0d7d233` to the letter. Source references below are to that commit unless they
name another project.

## Why

Makerspaces run more than 3D printers: laser cutters, label printers, cameras on the bench. The Prusa
Connect printer protocol already does most of what such a device needs:
- registration and pairing;
- one WebSocket per device;
- telemetry, events and commands with their `ACCEPTED`/`FINISHED`/`REJECTED` flow;
- file transfer, and cameras with snapshots.

What it lacks is small:
- a way to say what a device is, when it is not a Prusa model;
- a few telemetry keys;
- a few commands;
- live video.

This page proposes exactly those additions, for three devices:
- a tape label printer (DYMO LabelManager PnP);
- a laser cutter running GRBL;
- a USB camera with live view.

## Rules for every addition

- **Additive only.** No existing field, header, frame or command changes its meaning. An addition is a
  new value where the protocol has an open value, a new telemetry key, a new kwarg, a new command or a
  new endpoint.
- **The protocol's own words first.** Where the SDK or Connect already has a key or a command with the
  same meaning, it is used as it is (for example `axis_x`, `START_PRINT`, `/c/snapshot`). An addition is
  made only where nothing existing means the same.
- **The SDK's naming.** Commands are upper-case with underscores, like `LOAD_FILAMENT`
  (`const.py` `Command`). Telemetry keys and kwargs are lower-case snake_case, like `temp_bed`
  (`models.py:171-195`).
- **Ignored by a peer that does not know it.** The SDK reads only the kwargs it names
  (`__init__.py:436-483`). Connect maps only the telemetry keys it knows and drops the rest
  (prusa-connect `internal/connector/xbuddy/inbound.go:146-159`, `:273-299`). A command a printer has
  no handler for is answered `REJECTED` "Not Implemented" by the SDK itself (`command.py:184-186`). A
  Prusa printer never sends an addition, so it is unaffected.

## Part 1: device types

- **Purpose:** to say what a device is, so the server picks its capabilities.
- **Shape:** a string `vendor-or-firmware.model`, lower case, in the registration body's
  `printer_type` (`__init__.py:733-738`) and in the `User-Agent-Printer` header of every request and of
  the upgrade (`:281`). In `INFO`, `type`, `version` and `subversion` are `null`, which is what the SDK
  already sends for a printer without a type (`:397-400`).

  | Type | Device |
  |---|---|
  | `dymo.labelmanager-pnp` | DYMO LabelManager PnP tape printer |
  | `grbl.laser` | a laser cutter running GRBL. GRBL is the firmware; the vendor differs per machine |

- **Why the protocol cannot carry it:** `printer_type` is `PrinterType`, an enum of Prusa models written
  as `a.b.c` numbers (`const.py:47-60`). A number for another device could collide with a model Prusa
  numbers later.
- **Compatibility:** Prusa printers send only `a.b.c` values. A Connect that does not know the string
  reads it as an unknown model (`xbuddy.go:816-829`) and refuses the device at admission (`403`,
  `:704-712`). It never misreads it as a Prusa.

A camera needs no type: it registers through the camera protocol (part 4).

## Part 2: a tape label printer (`dymo.labelmanager-pnp`)

### States

The protocol's own: `IDLE` and `PRINTING` (`const.py:24-34`). A failed label is the transfer's or the
state's own event, with `reason`. Pause, resume and dialogs are answered `REJECTED` "Not Implemented" by
the SDK itself, because the device has no handler for them (`command.py:184-186`).

### Telemetry keys

Added to the telemetry object (`models.py:171-195`), and left out when unknown:

| Key | Type | Value |
|---|---|---|
| `media_loaded` | boolean | a tape cassette is in (bit `0x40` of status byte 0) |
| `battery_percent` | integer 0-100 | status byte 3, when at most 100 |
| `raw_status` | string | the first 8 status bytes in hex |

These are the readings the device's reference driver gives: labelle `5e8f1e5`, as ported in
`crates/engine/src/ports/labelle/port.rs:90-103`. Connect's telemetry has no key for a cassette, a
battery or raw status bytes (`inbound.go:146-159`).

### Printing a label: `tape_size_mm`

A label is Connect's own "download and print". The server sends `START_CONNECT_DOWNLOAD` with `path`,
`hash`, `team_id` and `printing: true` (`__init__.py:457-483`). The file is a PNG fetched from
`/p/teams/{team_id}/files/{hash}/raw` with the device's headers (`:468`; `download.py:399-413`).

The addition is one kwarg, `tape_size_mm`: an integer, one of `19`, `12`, `9` or `6`, default `12`.
That is labelle's own name and its default (`crates/engine/src/device/labelmanager.rs:38-52`). A Connect
print takes its settings from the G-code file. A PNG has none, and no Connect command carries a tape
width. The SDK's handler reads only `path`, `hash`, `team_id`, `printing` and `selecting`
(`__init__.py:467-477`), so it ignores the kwarg.

## Part 3: a GRBL laser cutter (`grbl.laser`)

The reference for the machine side is LaserFoundry v0.5.0 `61febdb`
([device-contract.md](device-contract.md#kinds)). A laser offers no raw G-code: nothing but a start can
switch the beam on ([decision](decisions/20261119-no-raw-gcode-on-a-laser.md)).

### States

The SDK requires one of its own states in every telemetry message and state event (`models.py:177-191`,
`const.py:24-34`). The laser sends the protocol state mapped from GRBL's state as below, and GRBL's
state itself, unchanged, in the new key `machine_state`.

| GRBL state | Protocol `state` |
|---|---|
| `Idle` | `IDLE` |
| `Run` while a job runs | `PRINTING` |
| `Run` otherwise (a frame, a go-to-origin) | `BUSY` |
| `Jog`, `Home`, `Check` | `BUSY` |
| `Hold` | `PAUSED` |
| `Door` | `ATTENTION`: a person must close the door |
| `Alarm` | `ERROR` |
| `Sleep` | `ATTENTION`: a person must wake it |
| `Unknown` (no status report yet) | `BUSY` |

The mapping is this proposal's. `Idle`, `Jog`, `Home`, `Check`, `Hold`, `Alarm` and `Unknown` follow
the mapping the laser port already uses (`crates/grbl/src/lib.rs:671-674`). `Run` is split so that a
running job is `PRINTING`. `Door` and `Sleep` become `ATTENTION`, because a person must act.

### Telemetry keys

The protocol's own keys, with their Connect meaning (`inbound.go:146-159`):

| Key | Value |
|---|---|
| `axis_x`, `axis_y` | the machine position in mm (GRBL's `MPos`) |
| `progress` | the job's progress in percent |
| `job_id` | the job, as the SDK adds it for a print (`__init__.py:323`) |

New keys:

| Key | Type | Value |
|---|---|---|
| `machine_state` | string | GRBL's state as it reports it: `Idle`, `Run`, `Hold`, `Jog`, `Alarm`, `Door`, `Check`, `Home`, `Sleep`, or `Unknown` |
| `max_power` | number | `$30`, the spindle/laser maximum |
| `laser_mode` | boolean | `$32` |
| `travel_x`, `travel_y`, `travel_z` | number, mm | `$130`, `$131`, `$132` |

The four settings keys are sent as the device read them: after connecting, and after `READ_SETTINGS`.

### Commands the protocol already has

| Command | On a laser | Kwargs |
|---|---|---|
| `START_CONNECT_DOWNLOAD` | loads a job file onto the device; with `printing: false` only. A download never starts the laser | the SDK's (`__init__.py:457-483`) |
| `START_PRINT` | starts the loaded job | `path`, as PrusaLink reads it (Prusa-Link `prusa_link.py:563-570`), and the new kwarg `from_line`: an integer ≥ 1, to start from a line of the file |
| `PAUSE_PRINT` | feed hold | none |
| `RESUME_PRINT` | resumes a feed hold | none |
| `STOP_PRINT` | stops the job | none |

### New commands

These are general machine-motion commands, not laser-only. A CNC router needs the same ones.

| Command | Does | Kwargs |
|---|---|---|
| `HOME` | runs the machine's homing cycle | none |
| `UNLOCK` | clears an alarm lock | none |
| `JOG` | moves relative to the current position | `x`, `y`: mm, at most 1000 either way; `feed`: mm/min, 1 to 30000 |
| `SET_ORIGIN` | makes the current position the job's origin | none |
| `GO_TO_ORIGIN` | moves to the job's origin | none |
| `FRAME` | traces the loaded job's bounds | `power`: percent 0-100, default 0 (beam off) |
| `READ_SETTINGS` | reads the machine's settings again and sends them as telemetry | none |

The bounds of `JOG` and the default of `FRAME` are LaserFoundry's: its `link.rs:26-27`, and its
`job.frame` (`crates/gcode/src/lib.rs:92`, default `:416`; see
[the device map](../plans/hub-core-v2-device-map.md)). Each command is answered with the protocol's own flow: `ACCEPTED`,
then `FINISHED` or `REJECTED` with a `reason`.

### Who may start the beam

`START_PRINT`, and `FRAME` with `power` above 0, switch the beam on. Whether a person is present is not
something the protocol can carry: the SDK has no idea of who sent a command. The server decides it
before sending. In our server, only a person's own session may send these, after framing the job and
confirming eye protection.

## Part 4: a USB camera

### Snapshots: the SDK's camera protocol, unchanged

- **Register:** `POST /p/camera` with `config`, `options`, `capabilities` and `fingerprint`
  (`models.py:198-225`). The answer's `Token` header is the camera's token (`__init__.py:855-859`).
- **Snapshots:** `PUT /c/snapshot`, a JPEG body with `Content-Type: image/jpg` and the headers
  `Fingerprint` and `Token` (`camera.py:31`, `:61-72`).
- **When:** the trigger schemes every 10, 30 or 60 seconds, on a layer change, on every fifth layer
  change, or manual (`const.py:246-253`).
- **Settings:** resolution, rotation, exposure and focus, as capabilities (`const.py:237-242`).

### Live view: one new endpoint and two new commands

The SDK has snapshots only, and no command that reaches a camera (`const.py` `Command`). Live view adds
an upload stream and two commands to start and stop it.

**The stream.** `POST /c/stream`, with the same `Fingerprint` and `Token` headers as `/c/snapshot`. The
body is a continuous `multipart/x-mixed-replace` stream of JPEG parts. Each part has a `Content-Length`
header and then the JPEG. This is the format go2rtc v1.9.15 `7c05066` takes as a pushed MJPEG source
(`internal/mjpeg/mjpeg.go:173-191` `inputMjpeg`; the parser `pkg/mpjpeg/multipart.go:13-60`). The stream
is its own connection, so video never delays telemetry, events or commands on the WebSocket.

**The commands.** They go over the device's WebSocket, which also registered the camera:

| Command | Does | Kwargs |
|---|---|---|
| `START_CAMERA_STREAM` | opens the stream for that camera | `camera_id`; `fps`: the most frames per second to send, default 5 |
| `STOP_CAMERA_STREAM` | ends it | `camera_id` |

The server sends `START_CAMERA_STREAM` when the first viewer opens the camera, and
`STOP_CAMERA_STREAM` when the last one leaves. When the server closes the upload, the stream ends too.
In between, snapshots go on as before.

**Per camera, a choice.** A camera's settings choose between the SDK's snapshots alone and snapshots
plus live view.

**Compatibility:** a Connect that does not know `/c/stream` answers it with an error. A printer that does
not know the commands answers them `REJECTED` "Not Implemented".
