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

This page proposes exactly those additions, for seven kinds of device:
- a tape label printer (DYMO LabelManager PnP);
- label printers on DYMO's CUPS driver (LabelWriter 3xx and 4xx);
- a laser cutter running GRBL;
- a USB camera with live view;
- a door controller with electric locks (part 7);
- a DYMO M-series USB scale (part 8);
- a Marlin printer that is not a Prusa (part 9).

And one for the program that hosts such devices (part 6): which devices belong to one agent, and
whether that agent and the server know the same additions.

## The parts and their issues

Each part is written in the design step of its deliverable. Every such step carries the label
`sdk-extension`, so that the parts still to write can be taken up together. A part is written only
for what the SDK and PrusaLink do not have already: the bed mesh (`mbl` in `INFO`, the event
`MESH_BED_DATA`) and pictures per layer (the camera's layer triggers) are the SDK's, not parts here.

| Part | For | State | Issue |
|---|---|---|---|
| 1 | what a device is (a string `printer_type`) | written | |
| 2 | the DYMO LabelManager PnP | written | |
| 3 | label printers on DYMO's CUPS driver (the 450 and the 320) | written | |
| 4 | a laser cutter on GRBL | written | #89 |
| 5 | a USB camera with live view | written | #5 |
| 6a | who the agent is, and what each side knows | written | #51 |
| 6b | updating the agent: the SDK's own `UPGRADE`, Connect's `path`; no addition | written | #56 |
| 9 | a Marlin printer: its type only; the rest is PrusaLink's use of the SDK | written | #83 |
| 8 | a DYMO M-series scale: its type and three telemetry keys | written | #130 |
| 7 | a door controller: its type, `locks` and `OPEN_LOCK` | written | #121 |
| maybe | the print's place in its file (the G-code viewer), if the SDK has nothing for it | to look at | #112 |

## Rules for every addition

- **Searched first, and listed.** Before an addition is written, the SDK, PrusaLink, Buddy's Connect client
  and Connect's printer side are searched for the same meaning (owner, 2026-11-21). Every addition is a row
  of [Every addition](#every-addition), with what was searched; `scripts/sdk-extension-check.py` fails
  on a name a reference has and on an addition missing from that table.
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
| `marlin.fdm` | an FDM printer running Marlin that is not a Prusa (part 9) |
| `dymo.m-scale` | a DYMO M-series USB scale (M1, M2, M5, M10, M25): one type, as one port reads them all (part 8) |

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

### The reason: the SDK's own

No addition. The `STATE_CHANGED` event that enters `ATTENTION` or `ERROR` carries, in its `reason`
(SDK `models.py:107-160`), the PPD's own text for the driver's CUPS `STATE:` keyword (`ppd/lw450.ppd:47-55`;
the keywords `common/CupsPrintEnvironment.cpp:160-190`), as PrusaLink sends a readable reason with its own
`ERROR` and `ATTENTION` (Prusa-Link `prusa/link/printer_adapter/state_manager.py:241`, `:383-387`). The
text is the owner's choice over the bare keyword (2026-11-21, wire.md "The second device").

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

New key:

| Key | Type | Value |
|---|---|---|
| `machine_state` | string | GRBL's state as it reports it: `Idle`, `Run`, `Hold`, `Jog`, `Alarm`, `Door`, `Check`, `Home`, `Sleep`, or `Unknown` |

### The machine's settings: in `INFO`

What a machine is, rather than what it does now, is in `INFO`: the SDK sends the MMU's `slots` and the
`sheet_settings` there (`__init__.py:393-429`), Buddy 6.4.0 its `nozzle_diameter`
(`src/connect/render.cpp:352`). Four new `INFO` kwargs, as the device read them from GRBL after
connecting:

| Kwarg | Type | Value |
|---|---|---|
| `max_power` | number | `$30`, the spindle/laser maximum |
| `laser_mode` | boolean | `$32` |
| `travel_x`, `travel_y`, `travel_z` | number, mm | `$130`, `$131`, `$132` |

To read them again, the server sends the SDK's own `SEND_INFO` (`const.py:124`); the device reads the
settings from GRBL and answers with `INFO`.

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
Part 6a adds both. Part 6b says how an agent is updated, with nothing added.

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
| `extensions` | array of strings | the additions of this page that this device uses, by the names below |

Not the hostname and not the fingerprint: two agents can share a hostname, a hostname changes, and
the SDK's fingerprint is per printer (`__init__.py:415`).

**One new command, server to device:**

| Command | Does | Kwargs |
|---|---|---|
| `SET_SERVER_INFO` | tells the device what the server is and knows; the device answers `FINISHED` | `release`: the server's release, shown only; `extensions`: array, the names the server knows; `wants`: array, the names the server needs from a device of this type |

The server sends it after an `INFO` that had `extensions`, and only then: a Prusa printer never sends
`extensions`, so it never gets the command. A device keeps the last one until its socket drops.

**The names.** One per addition, in the order of this page. A change that a peer of the old meaning
cannot read gets a new name with `@` and a number (`label.page_size@2`); the old name then stays as it
was, so a side may know both.

| Name | The addition |
|---|---|
| `type.string` | Part 1: a string `printer_type`, and `get_info`'s `null` type |
| `pnp.telemetry` | Part 2: its states and telemetry keys |
| `label.tape_size_mm` | Part 2: `tape_size_mm` on a download |
| `cups.states` | Part 3: its states, the PPD's text as the event's `reason` |
| `label.page_size` | Part 3: `page_size` on a download |
| `label.stop` | Part 3: stopping a label |
| `grbl.telemetry` | Part 4: its states, `machine_state` and the settings in `INFO` |
| `grbl.commands` | Part 4: the new commands |
| `camera.stream` | Part 5: `/c/stream` and its two commands |
| `agent.id` | Part 6a: `agent_id` |
| `server.info` | Part 6a: `extensions` and `SET_SERVER_INFO` |
| `door.lock` | Part 7: `lock_count`, `locks` and `OPEN_LOCK` |
| `scale.telemetry` | Part 8: its three telemetry keys |
| `marlin.fdm` | Part 9: the type, and the states of OctoPrint's serial layer |
| `gcode.output` | Part 9: `gcode_output` on a `GCODE` command's `FINISHED` |

**Whether they fit** (the same rule on both sides; shown, nothing refused: owner 2026-11-21):
- a name in the device's `extensions` that is not in the server's: **the agent is newer than the
  server** for that addition, "the hub does not know `label.page_size@2`";
- a name in the server's `wants` that is not in the device's `extensions`: **the agent is older**
  for it, "the agent does not have `cups.states`";
- neither: they fit, whatever their releases are. A newer agent that adds nothing this device uses
  fits an older server.

Each side shows it where its people look (ours, UI only): the server on the device's page, the agent
on that device's card. The device keeps working; what it does with an addition the other side lacks
is that addition's own compatibility rule above (an unknown kwarg ignored, an unknown command
`REJECTED`).

**Compatibility:**
- An agent without 6a sends no `extensions`: the server sends no `SET_SERVER_INFO` and shows "the agent
  does not say what it knows" beside its release.
- A server without 6a ignores both kwargs and never sends the command: the agent shows "the hub does
  not say what it knows" and nothing more.
- Connect ignores the two kwargs: it reads only the `INFO` fields it names, `storages` and `firmware`
  (prusa-connect `internal/connector/xbuddy/files.go:388-393`, unmarshalled at `:406`; Go's
  `json.Unmarshal` drops unknown keys).
- An SDK without the command answers it `REJECTED` "Unknown command" (`command.py:181-183`).

### 6b. Updating the agent: the SDK's own `UPGRADE`

No addition: every piece below is the SDK's, PrusaLink's or Connect's.

**The command.** `UPGRADE` (`const.py:141`), sent to any one of the agent's devices; it updates the
agent, and so all its devices. The SDK has no handler of its own for it; PrusaLink sets one
(Prusa-Link `prusa/link/printer_adapter/prusa_link.py:207`, `:600-605`).

| `UPGRADE` | Whose form | What the agent does |
|---|---|---|
| no kwargs | PrusaLink's (`command_handlers.py:589-608`): it installs the newest release from its own source and restarts | takes the newest signed release from its own release host, as the hub's own update does (`crates/update`) |
| `path` | Connect's (`internal/connector/xbuddy/verbs.go:265-280`): the image is a file already on the device's storage, named by its path; Connect never carries the image in the command | takes the release in that folder |

**The release onto the agent, with `path`.** The server first puts the release on the agent with the
SDK's own `START_CONNECT_DOWNLOAD`, `printing: false` (part 2: a `LOCAL` storage attached with
`Printer.attach`), one download per file, into a folder named by the release's tag: the build for the
agent's system, `SHA256SUMS` and `SHA256SUMS.sig`. `path` then names the build in that folder. As
Buddy's image, the release is a file like any other until `UPGRADE` names it.

**What the agent takes** (ours, the same rules as the hub's own update, `crates/update`): only a build
whose `SHA256SUMS` is signed with the release key over its tag, the folder's name, and whose sum is on
that list; then the trial, and the way back to the build before. A release that fails any of these ends
the command `FAILED`, as PrusaLink's `CommandFailed` does ("No update available" when there is nothing
newer, `command_handlers.py:597-598`): the SDK answers a handler's error `FAILED` "Command error" with the
error itself (`command.py:192-198`). A build that passes restarts the agent, as PrusaLink restarts itself
(`:601`); the new build's `INFO` says its release (`firmware`) and its `extensions` (6a).

**When.** Only while the device it is sent to is `IDLE`, `READY`, `STOPPED` or `FINISHED`: Connect's
guard for `UPGRADE`, START_PRINT's states (`internal/connector/xbuddy/states.go:55-60`). The server holds
it to the other devices of the same agent too (ours: they stop with it).

**Which release** (ours): the server offers only the agent builds it carries itself, those of its own
release, so the agent and the server fit (6a) by construction. The agent's own page offers what its
release host has, as PrusaLink's page asks its own source (Prusa-Link `prusa/link/web/main.py:669-701`,
`:703-725`).

**Compatibility:** an agent without 6b answers `UPGRADE` `REJECTED` "Not Implemented", having no handler
(`command.py:184-186`). Connect sends `UPGRADE` with `path` as this part reads it.

## Part 7: a door controller (`maker-connect.door-controller`)

A board beside a cabinet with a few small electric locks, each opened from the server for whoever may
open it. The first is a Kincony KC868-A6 (ESP32, relays and opto inputs) with four SARY XG-07C-12V
cabinet locks. The firmware is ours, in Rust (owner, 2026-11-21).

### The link: Buddy's Connect client, ported

- **The board talks to the server itself**, without an agent, as a Buddy printer does: registration by
  code over HTTP, then one WebSocket. Its client is Buddy 6.4.0's own, `src/connect/`, ported to Rust
  function by function: registration (`registrator.cpp`), the WebSocket and its commands (`connect.cpp`,
  `command.cpp`, `planner.cpp`), what it sends (`render.cpp`), TLS (`tls/`). On the wire it is a Buddy
  6.4.0 printer in every respect, except the additions below. One difference off the wire: where Buddy
  stops registering after its retries (the person starts again on the screen), the board, which has no
  screen, starts again with a new code after a minute.
- **What Buddy has and the board leaves out:** files and storages, transfers, print jobs, G-code, the
  printer's own states beyond those named here. `INFO` carries no `storages`.

### The type and `INFO`

- **Type:** `maker-connect.door-controller` (part 1's form). The board is not in the type: another
  board running the same firmware is the same kind of device.
- **New `INFO` kwargs:**

  | Kwarg | Type | Meaning |
  |---|---|---|
  | `lock_count` | integer | how many locks the board drives, numbered from 0 |
  | `extensions` | array of strings | part 6a; for this part `door.lock` |

  `agent_id` (part 6a) is not sent: the board is not hosted by an agent.

### States and telemetry

- **The device's state:** `IDLE`; `BUSY` while a lock's pulse runs; `ERROR` with a reason when the
  firmware finds a fault it can name (a relay or input that does not answer). A relay's fault holds until
  the board restarts; an input's clears when all inputs read again. Nothing else of the printer states
  is used.
- **New telemetry key `locks`:** an array, one object per lock, in lock order:

  | Key | Type | Meaning |
  |---|---|---|
  | `lock` | integer | its number, from 0 |
  | `door` | `"open"`, `"closed"` or `null` | what the lock's input reads; `null` only when the input cannot be read (the state is then `ERROR`) |
  | `resting_s` | integer | seconds until it may be opened again (0: it may) |

- **When:** at the protocol's pace, and at once when a door's switch changes. The server keeps the
  changes: the log of when each door opened and closed. A door change is not an event of its own: the
  telemetry says it, as a printer's telemetry says a temperature.
- **Doors without a switch:** the board cannot tell an input with no switch wired from an open door; both
  read `"open"`. The hub keeps, per lock, whether a switch is fitted (ours, in the hub's UI, with the
  lock's name and who may open it) and ignores `door` for a lock without one. The board has no settings.

### Opening a lock

| Command | Does | Kwargs |
|---|---|---|
| `OPEN_LOCK` | one pulse on that lock's relay; `FINISHED` when the pulse has ended | `lock`: its number; `expires_at`: Unix seconds, after which the command must not run |

- **The pulse** is the lock's own limit (its datasheet): at most 0.5 s, then at least 10 s of rest, about
  2 A. The firmware keeps both; nothing the server sends lengthens the pulse or shortens the rest.
- **Refused** as Buddy refuses: `REJECTED` with the text in `reason` and a code in `machine_reason`
  (Buddy 6.4.0 `src/connect/planner.cpp:813-871`, rendered at `render.cpp:334-335`; Buddy's own codes are
  `TRANSFER_IN_PROGRESS`, `FILE_EXISTS`, `STORAGE_FAILURE`, `NETWORK_FAILURE` and `NOT_READY`,
  `planner.cpp:293-310`). Four new codes:

  | `machine_reason` | `reason` | When |
  |---|---|---|
  | `NO_SUCH_LOCK` | `"No such lock"` | `lock` is not below `lock_count` |
  | `LOCK_RESTING` | `"Lock resting"` | within the lock's rest |
  | `EXPIRED` | `"Expired"` | the board's clock is past `expires_at` |
  | `NO_TIME` | `"No time"` | the board's clock is not set (SNTP), so `expires_at` cannot be checked |
- **Why `expires_at`:** Connect keeps a device's queue while its socket is away and drains it on the next
  attach (prusa-connect `internal/connector/xbuddy/link.go:327-353`, `xbuddy.go:793-804`). A lock must
  never open minutes later, when nobody stands there. The server sets `expires_at` a few seconds after
  the person pressed Open; a command that arrives late is refused, and the person presses again.
  The board checks `expires_at` again just before the pulse: a command it took in time but could not
  answer `ACCEPTED` for (the socket dropped) ends `FAILED` with `"Expired"` and `EXPIRED` when it is late.
- **The server, too** (ours): sends `OPEN_LOCK` only to a device whose socket is up, and drops it from
  the queue when the socket goes.

### Without the server: fail-secure

- **No server, no opening** (owner, 2026-11-21): the board has no local way to open a lock, no button,
  no page that opens. Its own page shows its state only.
- **No power:** the XG-07C opens on a pulse and stays locked without power, so a power cut leaves every
  lock shut.
- **A door that is open** stays as it is: the board cannot close a door, only a lock that is pushed shut.

### Updating the firmware

By part 6b, the same way as the agent: the release downloaded onto the board, then `UPGRADE` with its
`path`; a signed release, checked by the board before it starts it (Secure Boot v2 on the ESP32).

### Compatibility

- Connect would read the board as an unknown printer type (part 1) and ignore `locks` and `lock_count`.
- A server without this part never sends `OPEN_LOCK`; the board then only reports its doors.
- A board that does not know `OPEN_LOCK` answers `REJECTED` "Unknown command", as Buddy does.

## Part 8: a USB scale (`dymo.m-scale`)

A DYMO M-series scale on the agent's computer, its weight live to the server; read only, nothing is sent
to the scale. The reference is usbscale `9ed1cb55`, already ported (`crates/engine/src/ports/usbscale`,
[audit](../audits/usbscale.md)); its `print_scale_data` (`usbscale.c:380-456`) reads the scale's report
as the USB-IF HID Point of Sale scale has it.

### States

`IDLE`, always: the scale has no state of its own beyond its report, and the port reports `Idle`
(`crates/engine/src/ports/usbscale/port.rs:81-88`). What the scale says of its reading is in
`scale_status`. Every printer command is answered as the SDK answers one with no handler (`REJECTED`
"Not Implemented", `command.py:184-186`); `SEND_INFO` is the SDK's own.

### Telemetry keys

Added to the telemetry object, at the agent's pace, and left out while there is no reading:

| Key | Type | Value |
|---|---|---|
| `weight` | number | the report's weight bytes times ten to its exponent, in the scale's own unit, as usbscale computes it (`usbscale.c:384-391`); not converted |
| `weight_unit` | string | usbscale's name for the unit byte, from its `UNITS` (`usbscale.c:93-107`, for example "g" or "kg") |
| `scale_status` | integer | the report's status byte as sent: `1` fault, `2` stable at zero, `3` in motion, `4` stable, `5` under zero, `6` over weight, `7` calibration needed, `8` re-zeroing needed (usbscale's cases, `usbscale.c:407-450`) |

Connect's telemetry has no key for a weight that is read: its one weight is a material's
(`actual_netto_full_weight`, `internal/connector/xbuddy/tools.go:511`), and the SDK's and Buddy 6.4.0's
have none.

### Compatibility

Connect would read the scale as an unknown printer type (part 1) and drop the three keys. A server
without this part shows an `IDLE` device.

## Part 9: a Marlin printer (`marlin.fdm`)

An FDM printer on Marlin that is not a Prusa, on the agent's USB. Nothing is added but the type (part 1) and its model in `INFO` (below):
PrusaLink is itself a host of a Marlin printer on the SDK, so it is the reference for what goes over the
protocol; OctoPrint 1.11.8 is the reference for every byte to the printer (its serial layer, ported:
`crates/engine/src/ports/octoprint`, [audit](../audits/octoprint-marlin.md)). An SDK command becomes the
port's `Action`, OctoPrint's own call (`port.rs:589-746`), never PrusaLink's G-code: PrusaLink speaks to
a Prusa's firmware, OctoPrint to any Marlin.

### Commands

The handlers PrusaLink sets (Prusa-Link `prusa/link/printer_adapter/prusa_link.py:203-217`), as far as a
plain Marlin has them:

| SDK command | PrusaLink's kwargs | On the printer (the port, OctoPrint's call) |
|---|---|---|
| `START_PRINT` | `path` (`prusa_link.py:563-570`) | streams the file from the agent's storage, or prints it from the printer's SD card (`SdPrint`) |
| `PAUSE_PRINT`, `RESUME_PRINT`, `STOP_PRINT` | none | `JobControl` `Pause`, `Resume`, `Stop` |
| `GCODE` | `gcode` (`prusa_link.py:545-552`) | `Gcode`, sent as OctoPrint sends a command |
| `SET_VALUE` | one of `speed`, `flow`, `nozzle_temperature`, `bed_temperature` (`command_handlers.py:395-415`) | `Speed`, `Flow`, `Temperature`; PrusaLink's ranges are the MK3's (`LimitsMK3`) and are not used: the ranges are the server's UI's (ours) |
| `SEND_JOB_INFO`, `SEND_INFO`, `SEND_FILE_INFO`, `DELETE_FILE` | the SDK's | the SDK's own, over the storages below |
| `START_CONNECT_DOWNLOAD` | the SDK's (`__init__.py:457-483`) | the file into the agent's `LOCAL` storage (part 2) |
| `UPGRADE` | part 6b | the agent |

Left without a handler, so the SDK answers them `REJECTED` "Not Implemented" (`command.py:184-186`):
`LOAD_FILAMENT` and `UNLOAD_FILAMENT` (a Prusa's own menus), `RESET_PRINTER` (PrusaLink's line reset of
a Prusa's board), `SET_PRINTER_READY` and `CANCEL_PRINTER_READY`.

### What the printer answered: `gcode_output` on `FINISHED`

PrusaLink sends a `GCODE` command's lines and answers `FINISHED` with nothing of what the printer said
(`command_handlers.py:332-385`, `prusa_link.py:545-552`); Buddy's `FINISHED` carries no data either
(`render.cpp`). A server with a terminal shows the answer (an `M115`'s firmware name and Cap lines, an
M503's settings). One new event field:

| Field | Of | Type | Value |
|---|---|---|---|
| `gcode_output` | `FINISHED` of `GCODE`, in `data` | list of strings | the lines the printer sent while the command's lines were sent and acknowledged, in order, without the line ends, as the port collects them (OctoPrint's terminal log, `port.rs` M4) |

Left out when the printer sent nothing; a server that does not know it drops it (below). A `FAILED` or `REJECTED` stays the SDK's: its `reason` says why.

**Compatibility:** the SDK passes an event's `data` as it is (`__init__.py` `event_cb`); Connect reads
only the fields it names of an event and drops the rest. A server without this part sees the SDK's
`FINISHED`; an agent without it sends no `gcode_output`, and the terminal shows only that the command
finished.

### Storages

As PrusaLink's: the agent's folder as a `LOCAL` storage, and the printer's SD card as `SD_CARD`
(Prusa-Link `prusa_link.py:945`), listed by the port (`SdList`, OctoPrint's `M20`).

### Its model: `machine_type` in `INFO`

A Prusa says its model by its type (`printer_type`, Buddy `render.cpp:390`; PrusaLink by `M862.2`); a
Marlin printer has no such code, only the name it answers to `M115`. One new `INFO` kwarg:

| Kwarg | Type | Value |
|---|---|---|
| `machine_type` | string | `MACHINE_TYPE` of the printer's `M115` answer, as OctoPrint keeps it (`firmware_info`, `standard.py:1915`); left out until the printer answered |

The server finds the model's profile by it. When the printer answers after the first `INFO`, the agent
sends `INFO` again, as PrusaLink does when its printer info changes (`printer_polling.py:1078-1083`).

### Telemetry

PrusaLink's own keys (Prusa-Link `printer_adapter/structures/model_classes.py:39-72`), those the port reads:
`temp_nozzle`, `target_nozzle`, `temp_bed`, `target_bed` (`port.rs:407-410`), `axis_x`, `axis_y`, `axis_z`
(the position OctoPrint last parsed, `:426`), `progress` and `time_printing` (the file's position and the
time printed, `:429-440`). The others are left out, `time_remaining` too: the
port has no estimate (`:440`), OctoPrint's estimator is not in its serial layer. `INFO` leaves out what only a Prusa has (`mbl`, `sheet_settings`, `mmu`);
serial number and fingerprint are the agent's, as the PnP's (wire.md).

### States

| OctoPrint (`comm.py:445-460`) | Protocol `state` |
|---|---|
| `OPEN_SERIAL`, `DETECT_SERIAL`, `CONNECTING`, `TRANSFERING_FILE` | `BUSY` |
| `OPERATIONAL` | `IDLE` |
| `STARTING`, `PRINTING`, `RESUMING`, `FINISHING`, `CANCELLING` | `PRINTING` |
| `PAUSING`, `PAUSED` | `PAUSED` |
| `ERROR`, `CLOSED_WITH_ERROR` | `ERROR` |
| `NONE`, `CLOSED` | none: the printer is gone, the socket ends, as the DYMO's (wire.md) |

The mapping is this proposal's: OctoPrint's states have no SDK names. After a print, PrusaLink's own rule:
`FINISHED`, or `STOPPED` when it was stopped, held until `PRINT_END_TIMEOUT` (11 s, Prusa-Link
`prusa/link/const.py:100`) has passed and the printer confirms an instruction, then `IDLE`
(`state_manager.py:721-749`).

### Compatibility

Connect reads `marlin.fdm` as an unknown printer type (part 1); everything else it reads as a printer's.

## Every addition

Every addition of this page, one row each, checked by `scripts/sdk-extension-check.py`. It fails on a name
that the SDK, PrusaLink, Buddy's Connect client or Connect's printer side (at the commits of wire.md)
already has, and on an addition named above but missing here. *Searched* says where the same meaning
was looked for under another name.

| Name | Kind | Of | Part | Searched |
|---|---|---|---|---|
| `printer_type` | value | registration | 1 | a string where the SDK has `PrinterType` (`const.py`); Buddy sends its own enum (`render.cpp`) |
| `media_loaded` | telemetry | | 2 | SDK `Telemetry` (`models.py`), Buddy telemetry (`render.cpp`), Connect `telemetryPaths` (`inbound.go`): no cassette |
| `battery_percent` | telemetry | | 2 | as `media_loaded`: no battery |
| `raw_status` | telemetry | | 2 | as `media_loaded`: no raw status |
| `tape_size_mm` | kwarg | START_CONNECT_DOWNLOAD | 2 | SDK `start_connect_download` (`__init__.py`), Buddy `command.cpp` kwargs: no media size |
| `page_size` | kwarg | START_CONNECT_DOWNLOAD | 3 | as `tape_size_mm` |
| `machine_state` | telemetry | | 4 | as `media_loaded`; the SDK's `State` (`const.py`) is required beside it |
| `max_power` | info | | 4 | SDK `get_info` (`__init__.py`), Buddy `INFO` (`render.cpp`): limits of a printer only |
| `laser_mode` | info | | 4 | SDK `get_info` (`__init__.py`), Buddy `INFO` (`render.cpp`): limits of a printer only |
| `travel_x` | info | | 4 | SDK `get_info` (`__init__.py`), Buddy `INFO` (`render.cpp`): limits of a printer only |
| `travel_y` | info | | 4 | SDK `get_info` (`__init__.py`), Buddy `INFO` (`render.cpp`): limits of a printer only |
| `travel_z` | info | | 4 | SDK `get_info` (`__init__.py`), Buddy `INFO` (`render.cpp`): limits of a printer only |
| `from_line` | kwarg | START_PRINT | 4 | PrusaLink `start_print` (`prusa_link.py`), Buddy `command.cpp`: `path`, `job_id`, `tool_mapping` only |
| `HOME` | command | | 4 | SDK `Command` (`const.py`), Buddy `command.cpp`: no motion but `GCODE`, which a laser may not take |
| `UNLOCK` | command | | 4 | as `HOME` |
| `JOG` | command | | 4 | as `HOME` |
| `x` | kwarg | JOG | 4 | a kwarg of a new command |
| `y` | kwarg | JOG | 4 | a kwarg of a new command |
| `feed` | kwarg | JOG | 4 | a kwarg of a new command |
| `SET_ORIGIN` | command | | 4 | as `HOME` |
| `GO_TO_ORIGIN` | command | | 4 | as `HOME` |
| `FRAME` | command | | 4 | as `HOME` |
| `power` | kwarg | FRAME | 4 | a kwarg of a new command |
| `/c/stream` | endpoint | | 5 | SDK `camera.py`, `camera_controller.py`: `/c/snapshot` only; Connect's live view dials a declared camera, not the printer protocol |
| `START_CAMERA_STREAM` | command | | 5 | SDK `Command` (`const.py`): no command reaches a camera |
| `STOP_CAMERA_STREAM` | command | | 5 | as `START_CAMERA_STREAM` |
| `camera_id` | kwarg | START_CAMERA_STREAM | 5 | the SDK's own name for a camera (`models.py`) |
| `fps` | kwarg | START_CAMERA_STREAM | 5 | a kwarg of a new command |
| `agent_id` | info | | 6a | SDK `get_info` (`__init__.py`), Buddy `INFO` (`render.cpp`): `sn` and `fingerprint` are per printer |
| `extensions` | info | | 6a | SDK `get_info` (`__init__.py`), Buddy `INFO` (`render.cpp`): nothing says what a device knows; `features` is taken, a tool's own object in Connect (`tools.go`) |
| `extensions` | kwarg | SET_SERVER_INFO | 6a | a kwarg of a new command |
| `SET_SERVER_INFO` | command | | 6a | SDK `Command`, Buddy `command.cpp`: `SET_TOKEN` and `SET_VALUE` set one value, nothing of the server |
| `release` | kwarg | SET_SERVER_INFO | 6a | a kwarg of a new command |
| `wants` | kwarg | SET_SERVER_INFO | 6a | a kwarg of a new command |
| `lock_count` | info | | 7 | SDK `get_info` (`__init__.py`): `slots`, a count of the MMU's, the same form; no locks |
| `locks` | telemetry | | 7 | as `media_loaded` |
| `lock` | sub-key | locks | 7 | inside a new key |
| `door` | sub-key | locks | 7 | inside a new key |
| `resting_s` | sub-key | locks | 7 | inside a new key |
| `OPEN_LOCK` | command | | 7 | as `HOME` |
| `expires_at` | kwarg | OPEN_LOCK | 7 | a kwarg of a new command |
| `weight` | telemetry | | 8 | SDK `Telemetry` (`models.py`), Buddy telemetry (`render.cpp`), Connect (`inbound.go`, `tools.go`): a material's weight only |
| `weight_unit` | telemetry | | 8 | as `weight` |
| `scale_status` | telemetry | | 8 | as `weight`; the SDK's `State` (`const.py`) is required beside it |
| `NO_SUCH_LOCK` | value | machine_reason | 7 | Buddy's `MachineReason` (`planner.hpp`): no lock codes |
| `LOCK_RESTING` | value | machine_reason | 7 | as `NO_SUCH_LOCK` |
| `EXPIRED` | value | machine_reason | 7 | as `NO_SUCH_LOCK` |
| `NO_TIME` | value | machine_reason | 7 | as `NO_SUCH_LOCK` |
| `gcode_output` | event-field | FINISHED | 9 | SDK `Event` and `event_cb` (`__init__.py`, `models.py`), PrusaLink `ExecuteGcode` (`command_handlers.py`): no lines back; Buddy `FINISHED` (`render.cpp`): no data |
| `machine_type` | info | | 9 | SDK `get_info` (`__init__.py`: `type`, a Prusa's code), Buddy `INFO` (`render.cpp`: `printer_type`), PrusaLink (`printer_polling.py`: `M862.2`): no model name |
