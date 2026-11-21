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

This page proposes exactly those additions, for four kinds of device:
- a tape label printer (DYMO LabelManager PnP);
- label printers on DYMO's CUPS driver (LabelWriter 3xx and 4xx);
- a laser cutter running GRBL;
- a USB camera with live view.

And one for the program that hosts such devices (part 6): which devices belong to one agent, and
whether that agent and the server know the same additions.

## The parts and their issues

Each part is written in the design step of its deliverable. Every such step carries the label
`sdk-extension`, so that the parts still to write can be taken up together.

| Part | For | State | Issue |
|---|---|---|---|
| 1 | what a device is (a string `printer_type`) | written | |
| 2 | the DYMO LabelManager PnP | written | |
| 3 | label printers on DYMO's CUPS driver; the LabelWriter 320's type | written; the 320 to add | #25 |
| 4 | a laser cutter on GRBL | written | #89 |
| 5 | a USB camera with live view | written | #5 |
| 6a | who the agent is, and what each side knows | written | #51 |
| 6b | updating the agent (`UPGRADE` with a release) | to write | #56 |
| to come | a Marlin printer: its type and what it sends | to write | #83 |
| to come | a scale: its type and telemetry | to write | #130 |
| to come | the bed mesh: telemetry and a command to measure | to write | #107 |
| to come | a door controller: its type, states and `OPEN_LOCK` | to write | #121 |
| maybe | the print's place in its file (the G-code viewer) | to look at | #112 |
| maybe | a layer change for timelapses (likely there already) | to look at | #116 |

## Rules for every addition

- **Additive only.** No existing field, header, frame or command changes its meaning. An addition is a
  new value where the protocol has an open value, a new telemetry key, a new kwarg, a new command or a
  new endpoint.
- **The protocol's own words first.** Where the SDK or Connect already has a key or a command with the
  same meaning, it is used as it is (for example `axis_x`, `START_PRINT`, `/c/snapshot`). An addition is
  made only where nothing existing means the same.
- **The SDK's naming.** Commands are upper-case with underscores, like `LOAD_FILAMENT`
  (`const.py` `Command`). Telemetry keys and kwargs are lower-case snake_case, like `temp_bed`
  (prusa-connect `inbound.go:148`).
- **Ignored by a peer that does not know it.** The SDK reads only the kwargs it names
  (`__init__.py:436-483`). Connect maps only the telemetry keys it knows and drops the rest
  (prusa-connect `internal/connector/xbuddy/inbound.go:146-159`, `:273-299`). A command the SDK does
  not know is answered `REJECTED` "Unknown command" by the SDK itself (`command.py:181-183`); a known one
  with no handler, `REJECTED` "Not Implemented" (`:184-186`). A Prusa printer never sends an addition, so
  it is unaffected.

## Part 1: device types

- **Purpose:** to say what a device is, so the server picks its capabilities.
- **Shape:** a string `vendor-or-firmware.model`, lower case, in the registration body's
  `printer_type` (`__init__.py:733-738`) and in the `User-Agent-Printer` header of every request and of
  the upgrade (`:281`).
- **The SDK change it needs:** one, in `get_info`. A string in `Printer.type` already counts as set
  (`is_initialised` asks only that it is not `None`, `__init__.py:264-271`) and is sent as is by `str()`
  in the registration and the header (`:281`, `:736`). But `get_info` reads `self.__type.value`
  (`:397-398`), so `SEND_INFO` would end `FAILED` "Command error" (`command.py:192-198`). The extension has
  `get_info` send `type`, `version` and `subversion` as `null` for a string type, the values it sends
  when no type is set (`__init__.py:399-400`).

  | Type | Device |
  |---|---|
  | `dymo.labelmanager-pnp` | DYMO LabelManager PnP tape printer |
  | `dymo.labelwriter-450` | DYMO LabelWriter 450 |
  | `dymo.labelwriter-320` | DYMO LabelWriter 320 |
  | `grbl.laser` | a laser cutter running GRBL. GRBL is the firmware; the vendor differs per machine |

- **Why the protocol cannot carry it:** `printer_type` is `PrinterType`, an enum of Prusa models written
  as `a.b.c` numbers (`const.py:47-60`). A number for another device could collide with a model Prusa
  numbers later.
- **Compatibility:** Prusa printers send only `a.b.c` values. A Connect reads the model from the header
  (`xbuddy.go:816-829`). The string is not an `a.b.c` value, so it cannot be read as a Prusa model.

A further LabelWriter model takes the same form, `dymo.labelwriter-<model>`, when it is ported. DYMO's
driver has a PPD per model (`ppd/lw300.ppd` … `ppd/lw450tt.ppd`).

**Where the type comes from** (ours, owner 2026-11-20): the person picks the device's profile on the
agent's own page, where they also turn on its module (DYMO, laser, …) and see its registration code. The
agent keeps it as `[printer] type` in the device's settings file, where PrusaLink keeps its type
(Prusa-Link `prusa/link/config.py:280-289`). PrusaLink itself reads its type from the printer and writes it
there (`prusa/link/web/lib/wizard.py:188-191`); picking it on the page is ours.

A camera needs no type: it registers through the camera protocol (part 5).

## Part 2: a tape label printer (`dymo.labelmanager-pnp`)

### States

The protocol's own: `IDLE` and `PRINTING` (`const.py:24-34`). A label that fails is a
`STATE_CHANGED` event with state `ERROR` and the reason in `reason` (`models.py:148-163`), then `IDLE`.
Pause and resume are answered `REJECTED` "Not Implemented" by the SDK itself, because the device has no
handler for them (`command.py:184-186`). `DIALOG_ACTION` is the SDK's own handler (`__init__.py:156`,
`:519-524`); the device raises no dialogs.

### Telemetry keys

Added to the telemetry object (`models.py:171-195`), and left out when unknown:

| Key | Type | Value |
|---|---|---|
| `media_loaded` | boolean | a tape cassette is in (bit `0x40` of status byte 0) |
| `battery_percent` | integer 0-100 | status byte 3, when at most 100 |
| `raw_status` | string | the first 8 status bytes in hex |

These are **not** the reference driver's: labelle `5e8f1e5` does not interpret these bytes
(`dymo_labeler.py:234`). They are what the owner saw on his PnP, an approved exception (owner,
2026-11-16; `crates/engine/src/ports/labelle/port.rs:91-103`,
[device map](../plans/hub-core-v2-device-map.md), the label printer's other options, item 3). Connect's telemetry has no key for a cassette, a
battery or raw status bytes (`inbound.go:146-159`).

### Printing a label: `tape_size_mm`

A label is Connect's own "download and print". The server sends `START_CONNECT_DOWNLOAD` with `path`,
`hash`, `team_id` and `printing: true` (`__init__.py:457-483`). The file is a PNG fetched from
`/p/teams/{team_id}/files/{hash}/raw` with the device's headers (`:468`; `download.py:399-413`).

The file lands in a storage the device attaches with the SDK's own `Printer.attach`: a folder, as a
`LOCAL` storage (`__init__.py:888-904`). `attach` needs Linux, for inotify (`:899`). `path` must point
into the storage; otherwise `to_os_path` raises (`download.py:302`, `:314-334`) and the command ends
`FAILED` "Command error" (`command.py:192-198`). The storage's name and folder are ours.

The addition is one kwarg, `tape_size_mm`: an integer, one of `19`, `12`, `9` or `6`, default `12`.
That is labelle's own name, values and default (labelle `5e8f1e5` `lib/devices/dymo_labeler.py:243-257`;
[device map](../plans/hub-core-v2-device-map.md) row 1 of the label verbs). A Connect
print takes its settings from the G-code file. A PNG has none, and no Connect command carries a tape
width. The SDK's handler reads only `path`, `hash`, `team_id`, `printing` and `selecting`
(`__init__.py:467-477`), so it ignores the kwarg.

## Part 3: label printers on DYMO's CUPS driver (`dymo.labelwriter-450`, `dymo.labelwriter-320`)

The reference is DYMO's own CUPS driver 1.4.0 (`reference/dymo-cups-drivers-1.4.0`, filter `raster2dymolw`),
used for both the 450 and the 320; other 3xx and 4xx models have PPDs whose options differ, checked when each is ported. The `Open/Default` lines of `ppd/lw450.ppd` and `ppd/lw320.ppd` are
identical ([device map](../plans/hub-core-v2-device-map.md) §3). It is a stack apart from the tape printer
of part 2. Driver paths below are under its `src/` unless a PPD.

### States

| Driver | Protocol `state` |
|---|---|
| no job | `IDLE` |
| a job runs | `PRINTING` |
| `jsPaperOut` | `ATTENTION`: a person must load labels |
| `jsError` | `ERROR` |

The driver's job states are `common/PrinterDriver.h:54-66`. The 450's code sets only `jsOK`, `jsPaperOut` and
`jsError` (`lw/LabelWriterLanguageMonitor.cpp:261-271`). `IDLE` and `PRINTING` are this proposal's, as the
driver has no idle state of its own (device map §3.2).

### Telemetry key

| Key | Type | Value |
|---|---|---|
| `state_reason` | string | the driver's own CUPS `STATE:` keyword, unchanged, for example `com.dymo.out-of-paper-error` (`common/CupsPrintEnvironment.cpp:160-190`); left out while the driver reports none |

Connect's telemetry has no key for a printer's state reason (`inbound.go:146-159`).

### Printing a label: `page_size`

As on the tape printer, a label is Connect's own `START_CONNECT_DOWNLOAD` with `printing: true`, the file a
PNG. The addition is one kwarg, `page_size`: one of the PPD's own `PageSize` choices, for example
`w167h288`; without it the PPD's `DefaultPageSize` (`ppd/lw450.ppd:147-149`). Which page size a roll takes
is the server's data (DYMO Connect's roll catalogue), not part of the protocol.

### Stopping a label

`STOP_PRINT`, as the protocol has it, cancels the running job, as a CUPS cancel does: the driver sees
`jsDeleted` (`lw/LabelWriterLanguageMonitor.cpp:247`). CUPS's own side of the cancel is not checked
(device map §3.1, verb 2).

## Part 4: a GRBL laser cutter (`grbl.laser`)

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

The mapping is this proposal's. These rows follow the mapping the laser port already uses
(`crates/grbl/src/lib.rs:669-674`): `Idle`; `Jog`, `Home`, `Check` as `BUSY`; `Hold` as `PAUSED`;
`Alarm` as `ERROR`. These differ from it:
- `Run` is split, so that a running job is `PRINTING`; the port has `BUSY` for every `Run`.
- `Door` and `Sleep` are `ATTENTION`, because a person must act (owner, 2026-11-20). The port has
  `PAUSED` for `Door` and `IDLE` for `Sleep`.
- `Unknown` is `BUSY`, so that nothing is sent to a machine whose state is not yet known (owner,
  2026-11-20). The port has `IDLE`.

### Telemetry keys

The protocol's own keys, with their Connect meaning (`inbound.go:146-159`):

| Key | Value |
|---|---|
| `axis_x`, `axis_y` | the machine position in mm (GRBL's `MPos`) |
| `progress` | the job's progress in percent |
| `job_id` | the job, as for a print (`inbound.go:155`) |

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
| `START_PRINT` | starts the loaded job | `path`, as PrusaLink reads it (Prusa-Link `prusa/link/printer_adapter/prusa_link.py:563-570`), and the new kwarg `from_line`: an integer ≥ 1, to start from a line of the file |
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

## Part 5: a USB camera

### Snapshots: the SDK's camera protocol, unchanged

The camera side is the SDK's. Prusa's server side of these two endpoints is not in reference/prusa-connect,
so what the hub answers is **ours**, built to what the SDK reads: a `200` with a `Token` header on
registration (`__init__.py:855-859`).

- **Register:** `POST /p/camera` with `config`, `options`, `capabilities` and `fingerprint`
  (`models.py:198-225`). The answer's `Token` header is the camera's token (`__init__.py:855-859`).
- **Both are HTTP**, not the WebSocket: "Register (/p/register) and CameraRegister (/p/camera) remain
  HTTP" (`__init__.py:783`); the camera's registration is sent with the printer's own headers
  (`:822-826`).
- **Snapshots:** `PUT /c/snapshot` from the camera controller's own loop (`camera_controller.py:166-186`),
  a JPEG body with the headers `Timestamp`, `Fingerprint`, `Token` and `Content-Type: image/jpg`, and the
  query parameter `printer_uuid` when the camera belongs to a printer (`camera.py:31`, `:54-77`).
- **A refused snapshot:** on `401` or `403` the SDK drops the camera's token, so the camera registers again
  (`camera_controller.py:176-179`).
- **When:** the trigger schemes every 10, 30 or 60 seconds, on a layer change, on every fifth layer
  change, or manual (`const.py:246-253`).
- **Settings:** resolution, rotation, exposure and focus, as capabilities (`const.py:237-242`).

### Live view: one new endpoint and two new commands

The SDK has snapshots only, and no command that reaches a camera (`const.py` `Command`). Live view adds
an upload stream and two commands to start and stop it.

**The stream.** `POST /c/stream`, HTTP like the rest of the camera protocol, with the same headers and
`printer_uuid` as `/c/snapshot`. The
body is go2rtc's own MJPEG stream, as go2rtc v1.9.15 `7c05066` writes it (`pkg/mjpeg/writer.go:9-27`):
`Content-Type: multipart/x-mixed-replace; boundary=frame`, and per frame `--frame`, `Content-Type:
image/jpeg`, `Content-Length`, an empty line, the JPEG and `\r\n`. It is also the format go2rtc takes as a
pushed MJPEG source (`internal/mjpeg/mjpeg.go:173-191` `inputMjpeg`; the parser
`pkg/mpjpeg/multipart.go:13-60`). The stream
is its own connection, so video never delays telemetry, events or commands on the WebSocket.

**The commands.** They go over the WebSocket of the device the camera belongs to. The camera itself
registered over HTTP (`POST /p/camera`, `__init__.py:855-859`).

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
not know the commands answers them `REJECTED` "Unknown command" (`command.py:181-183`).

## Part 6: the agent and the server

One agent hosts several devices, each a printer of its own on the protocol: its own registration,
token and WebSocket (wire.md). The protocol has no idea of a host: nothing says that the LabelWriter
and the PnP run in the same program, and nothing says which of the additions above either side knows.
Part 6a adds both. Part 6b (updating an agent) follows, [#56](https://github.com/CuriousTimo/Maker-Connect/issues/56).

### 6a. Who the agent is, and what each side knows

**What already says it, unchanged:**
- **The agent's release:** `User-Agent-Version` on every request and the upgrade
  (`__init__.py:282`), and `firmware` in `INFO` (`:409`). Shown, never compared: a release name says
  nothing of what a build can do.
- **The computer:** `network_info` in `INFO` (`:411`), as PrusaLink fills it (`hostname`, `lan_ipv4`,
  …; Prusa-Link `prusa/link/printer_adapter/printer_polling.py:110-112`, `:501-506`).

**Two new `INFO` kwargs** (in the answer to `SEND_INFO`, which the server sends 100 ms after every
attach, wire.md "The WebSocket"):

| Kwarg | Type | Meaning |
|---|---|---|
| `agent_id` | string, 32 lower-case hex digits | the agent that hosts this device: made once at random by the agent and kept in its folder (`agent.ini`, `[agent] id`), the same for all its devices, never changed by a re-registration |
| `features` | array of strings | the additions of this page that this device uses, by the names below |

Not the hostname and not the fingerprint: two agents can share a hostname, a hostname changes, and
the SDK's fingerprint is per printer (`__init__.py:415`).

**One new command, server to device:**

| Command | Does | Kwargs |
|---|---|---|
| `SET_SERVER_INFO` | tells the device what the server is and knows; the device answers `FINISHED` | `release`: the server's release, shown only; `features`: array, the names the server knows; `wants`: array, the names the server needs from a device of this type |

The server sends it after an `INFO` that had `features`, and only then: a Prusa printer never sends
`features`, so it never gets the command. A device keeps the last one until its socket drops.

**The names.** One per addition, in the order of this page. A change that a peer of the old meaning
cannot read gets a new name with `@` and a number (`label.page_size@2`); the old name then stays as it
was, so a side may know both.

| Name | The addition |
|---|---|
| `type.string` | Part 1: a string `printer_type`, and `get_info`'s `null` type |
| `pnp.telemetry` | Part 2: its states and telemetry keys |
| `label.tape_size_mm` | Part 2: `tape_size_mm` on a download |
| `cups.state_reason` | Part 3: its states and `state_reason` |
| `label.page_size` | Part 3: `page_size` on a download |
| `label.stop` | Part 3: stopping a label |
| `grbl.telemetry` | Part 4: its states and telemetry keys |
| `grbl.commands` | Part 4: the new commands |
| `camera.stream` | Part 5: `/c/stream` and its two commands |
| `agent.id` | Part 6a: `agent_id` |
| `server.info` | Part 6a: `features` and `SET_SERVER_INFO` |

**Whether they fit** (the same rule on both sides; shown, nothing refused: owner 2026-11-21):
- a name in the device's `features` that is not in the server's: **the agent is newer than the
  server** for that addition, "the hub does not know `label.page_size@2`";
- a name in the server's `wants` that is not in the device's `features`: **the agent is older**
  for it, "the agent does not have `cups.state_reason`";
- neither: they fit, whatever their releases are. A newer agent that adds nothing this device uses
  fits an older server.

Each side shows it where its people look (ours, UI only): the server on the device's page, the agent
on that device's card. The device keeps working; what it does with an addition the other side lacks
is that addition's own compatibility rule above (an unknown kwarg ignored, an unknown command
`REJECTED`).

**Compatibility:**
- An agent without 6a sends no `features`: the server sends no `SET_SERVER_INFO` and shows "the agent
  does not say what it knows" beside its release.
- A server without 6a ignores both kwargs and never sends the command: the agent shows "the hub does
  not say what it knows" and nothing more.
- Connect ignores the two kwargs: it reads only the `INFO` fields it names, `storages` and `firmware`
  (prusa-connect `internal/connector/xbuddy/files.go:388-393`, unmarshalled at `:406`; Go's
  `json.Unmarshal` drops unknown keys).
- An SDK without the command answers it `REJECTED` "Unknown command" (`command.py:181-183`).
