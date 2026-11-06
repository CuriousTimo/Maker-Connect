# Tools and drivers to work with

Maker Connect is the machine side: it keeps the files, queues them, and gets them to the
machines and back what they do. Designing and slicing stay in the tools makers already use.
The same split everywhere: a tool makes the job, Maker Connect takes it from there.

| Tool makes the job | Maker Connect | The machine |
|---|---|---|
| PrusaSlicer, Cura: G-code | files, queue, print, progress | 3D printer (agent, or built-in Prusa Connect) |
| LaserFoundry: GRBL G-code | files, queue, stream (with its `laserfoundry-device`), progress | laser (agent, GRBL) |
| a label template in the hub | render, queue, print | label printer (agent) |

## Lasers: LaserFoundry

**Sending from LaserFoundry** (or any program) goes through the apps API with a space's API
token: [apps-api.md](apps-api.md). The file waits in Files with the laser chosen; someone at the
laser frames and starts it. LaserFoundry's side (a plugin or a merge request to them) is still
to do.

[LaserFoundry](https://github.com/Stitchwright/laserfoundry) (MIT OR Apache-2.0, Rust 2024,
version 0.2.1, read at commit `3b9b9fa`, 2026-11-03): the user's choice to replace LightBurn. A
clean-room, LightBurn-class laser application: layout, cut layers with speed, power, passes and
fill, image dithering, nesting, a toolpath preview, GRBL G-code (and Marlin-laser and
Smoothieware dialects); a desktop app (egui), a browser build (Web Serial) and a CLI
(`laserfoundry-cli gcode design.svg -o job.gcode`, and an MCP server). Its status, by its own
README: early; "everything machine-side has run against the built-in simulator only".

Streaming the G-code to the machine is Maker Connect's job (user, 2026-11-04): Maker Connect
receives the file, queues it, and the agent streams it. And LaserFoundry has the piece for that,
apart from its UI: the crate **`laserfoundry-device`** (`crates/device`), "talking to GRBL
controllers: a transport abstraction, a character-counting streamer, status parsing, and a
simulated controller". Its dependencies are small (`serialport` behind a `serial` feature,
`thiserror`, `log`, `web-time`); no UI, no OS serial code in it.

- `Transport` (write bytes, read a line with a timeout): our agent's serial port can be one.
- `stream_program` with `StreamOptions` and `Progress` (sent, acknowledged, total): GRBL's
  character counting in its 128-byte receive buffer, lines of at most 79 bytes; `restart_line`
  to resume a job.
- `Link`: a persistent connection with status and position, jog, home, unlock, feed hold and
  resume, stop (soft reset, laser off), a console, the `$$` settings; `JobState`, `Snapshot`.
- `parse_status` (`<Idle|MPos:...>`), and GRBL's error and alarm codes with texts and hints.
- `SimulatedGrbl`: an in-memory GRBL that models the 128-byte buffer and catches over-sending;
  also `SimulatedMarlin`.

GRBL is not Marlin: flow control by character counting, no line numbers or checksums; status
by the realtime `?`; realtime bytes outside the stream (`!` hold, `~` resume, Ctrl-X reset);
`$H` homing, `$$` settings, `ALARM:N` after a limit or reset.

**Built (2026-11-04):** the crate `maker-connect-grbl` (`crates/grbl`) wraps its `Link`, shared by
the agent (with real serial ports) and the hub (virtual lasers):

- GRBL recognised on the port by its greeting (or `$I`); commands (`$H`, `$X`, `$$`, any line)
  with their answers for the terminal;
- jogs as GRBL's own `$J=` through the link's `jog` and `jog_to` (laser off, checked). The
  website's jog pad (lasers, CNCs, plotters; printers keep their ±10 mm buttons) sends
  `jog:<x>:<y>` with a step of 0.1, 1, 10 or 50 mm and a speed of 600 to 6000 mm/min; the hub
  refuses anything else and turns it into `$J=G91 G21 X… Y… F…`;
- the job's origin: *Set origin here* is `G92 X0 Y0` and *Go to origin* `G90 G0 X0 Y0`, through
  the link's `set_zero` and `go_to_zero` (as LaserFoundry does it). GRBL keeps a `G92` offset
  until it restarts, so it is set again after a reset or a stop (a stop is a soft reset);
- the settings that matter, read with `$$` on connecting: laser mode (`$32`), full power
  (`$30`), travel (`$130` to `$132`). The hub refuses a job with laser mode off or outside the
  travel, and warns when the file's power range does not fit `$30`;
- Frame: a laser file's outline (where its moves go, read on upload) run as jogs, laser off;
- jobs fetched and streamed by character counting, with pause, resume and stop;
- starting again: an interrupted job says where it can start again (`restart_line`, a few lines
  back, in the file's numbering), and the new job starts there with `laserfoundry-gcode`'s
  `resume_from`, which first goes there with the beam off;
- virtual lasers: `SimulatedGrbl` behind the same code, on a thread of their own in the hub.

Tested against `SimulatedGrbl`; not yet on a real laser.

**Plan (as it was):** the agent uses `laserfoundry-device` as a dependency (from git, pinned to a commit;
the licences fit AGPL-3.0) rather than writing GRBL streaming again; a GRBL machine is then a
`Transport` over our serial port. `SimulatedGrbl` serves the agent's tests, and can stand behind
the virtual laser. The user's Ortur on USB would be among its first real machines: Frame and
low power first. Later: an upload from LaserFoundry to Maker Connect (it already has "Publish
to MakerFoundry", makerfoundry.app, by the same makers), or the hub turning an SVG into
G-code with its engine.

## Label printers: thermal-label

Label printers are a real maker tool, and almost always awkward because of their software.
[thermal-label](https://thermal-label.github.io/) (MIT, TypeScript): drivers for 83 label
printers without vendor SDKs, with reverse-engineered wire protocols documented per family.

- Brother QL and PT (USB, TCP 9100, Bluetooth), DYMO LabelWriter, LabelManager and LetraTag,
  Marklife / Deli. Not Zebra, Niimbot or Phomemo.
- Node (USB through libusb, TCP, serial) and the browser (WebUSB, Web Serial, Web Bluetooth);
  a small CLI. No layout: templates, barcodes and CSV batches are left to its sister project
  [burnmark-io](https://burnmark-io.github.io/) (not read yet).
- Young and small (one maker, active until October 2026).

How it fits:

1. **As the reference (preferred).** Port the drivers needed to Rust, in the agent, from its
   protocol documentation; MIT allows taking code too, with credit. Brother QL over the network
   (TCP 9100) first: no USB permissions, one binary.
2. As a helper beside the agent: the agent runs its CLI. Quick, but needs Node 24.
3. From the browser (WebUSB, Web Bluetooth), without an agent: for a Bluetooth label printer
   at a laptop. Against "the machine connects to the hub", so at most an extra.

The hub makes the label (template to image) and queues it; the agent prints it. Which label
printer to start with: the DYMO LabelManager PnP, the one at hand (below); Brother QL over the
network next.

### DYMO LabelManager PnP: tried (2026-11-04)

On the user's LabelManager PnP (USB `0922:1001`, the only id it showed), from a Mac without
DYMO's software: it shows a small USB stick ("LM PNP", DYMO's software for Windows) **and** a
HID interface (interface 0, usage page `ff00`, interrupt endpoints `0x01` out, `0x81` in, 64
bytes). No mode switch to `0922:1002` is needed: the printer listens on that HID interface as it
is. macOS keeps the interface for its HID driver (libusb or `nusb` cannot claim it), so we talk
to it through the system's HID API (the `hidapi` crate): no root, no driver.

- Each write is one 64-byte output report (report id 0) carrying the next 64 bytes of the
  stream; reads are 8 bytes, the first being the status.
- `ESC A` (`1B 41`): status, 8 bytes. Byte 0: `0x40` cassette in, ready; `0x60` while printing
  (bit 5); `0x00` no cassette; `0x10` cutter jammed; `0x04` an error. The rest is not
  documented; read on one PnP (2026-11-04), **to confirm**:
  - bytes 4 and 5, big-endian: half the pack's voltage in mV (`0d dd` = 3549 mV, a 7.10 V
    pack). It jitters a few mV, rose while charging and fell while not: a measured voltage.
    The agent reports it; below 7.0 V a label waits (a weak pack under the print head makes the
    printer start over, and feed tape), from 7.2 V on it prints again;
  - byte 3: rose with the voltage (`40`, `4c`, then `64` = 100 while charging): perhaps DYMO's
    own level, saturating at 100 on the charger. Not used;
  - byte 2: always `02`, once `12` (bit 4) for one reading while charging. Perhaps "charging"
    or a moment's busy flag; to find out by plugging and unplugging the charger.
- A label: `ESC C 0` (tape type), `ESC B 0` (dot tab), `ESC D 8` (8 bytes a column: 64 dots,
  12 mm tape at 180 dpi; 6 for 9 mm, 4 for 6 mm with dot tab 1 or 2), then one `SYN` (`16`)
  and 8 bytes per column of the label, top pin first (bit 7 of the first byte). Like labelle,
  `ESC A` after every 64 columns, read before going on. Blank columns feed the tape; about
  113 (16 mm) bring the end of the label past the cutter, which is by hand on this model.

From thermal-label's protocol notes (its "D1" family) and labelle (Apache-2.0; its DYMO files
carry Sebastian J. Bronner's permissive notice). thermal-label warns that unknown opcodes can
hang the printer until it is switched off and on.

## Klipper printers: Moonraker's SimplyPrint component

Read 2026-11-04 in Moonraker (GPL-3.0, `components/simplyprint.py`), to learn its protocol;
none of its code is used. A printer on Klipper always has a Linux computer (Klippy runs
there), and nearly always Moonraker beside it (Mainsail and Fluidd need it). Moonraker has
three components that connect outward by themselves:

- **`[simplyprint]`** opens a WebSocket to SimplyPrint's servers, or to the address in its
  `url` option. That option is in the code, not in Moonraker's docs, so a newer Moonraker may
  change it. Over it: temperatures, state, job progress, the bed mesh, the computer, Klipper's
  console; and from the server G-code, pause, resume, cancel, and a file to fetch from a URL
  and print. Built here: `crates/hub/src/simplyprint.rs`. The server must never send
  `new_token` or `complete_setup`: Moonraker answers those by going back to SimplyPrint's own
  servers. A printer has one `[simplyprint]`: pointed here, it is not on SimplyPrint.
- **`[mqtt]`** connects to an MQTT broker and takes most of Moonraker's API over it, but not
  files. Not used: the user prefers not to need a broker for this.
- **`[spoolman]`** reports the filament used to a Spoolman server (`PUT /v1/spool/{id}/use`).
  Next: Maker Connect's Materials speaks Spoolman's API, so Moonraker, the OctoPrint plugin
  and Mainsail/Fluidd can use it as their Spoolman, and its data stays compatible for moving
  either way.

Profiles: Klipper's own `config/printer-*.cfg` (GPL-3.0) for the printers Marlin's examples
lack (`maker-connect-import klipper`), and the Vorons by hand from VoronDesign's configs.

## Spoolman: Materials speaks its API

Read 2026-11-04 in Spoolman (github.com/Donkie/Spoolman, MIT; its models, API, tag module,
and its client_v2 for the library's design), to be compatible, not to copy. Materials keeps
Spoolman's model (`vendor`, `filament`, `spool` tables, extra fields, tags, settings) and
serves `/api/v1` the way Spoolman does (`crates/hub/src/spoolman.rs`), so Moonraker's
`[spoolman]` (`server: http://<hub>:8460`; it calls `/api/v1/spool/{id}`, `/use`, and the
`/api/v1/spool` WebSocket), OctoPrint's plugin, Mainsail, Fluidd and scales find what they
expect. SpoolmanDB (MIT) is fetched hourly as the catalogue (`spoolmandb.rs`). A spool's id
is its code's number (`S-0007` is 7); locations are Maker Connect's (with codes, labels, a
description, and slots for a dry box) and go out as names (`PATCH /api/v1/location/{name}`
renames one, as in Spoolman); Maker Connect's own fields (kind, unit, colour name, ink,
width, `loaded_in`, `slot`) go out as extra fields. Spoolman's labels (`WEB+SPOOLMAN:S-7`, `…/spool/show/7`) are read too.

Scales that speak to a Spoolman, and so to this hub: SpoolmanScale (GPL-3.0, ESP32, NFC,
touch display; FilaMan and BamBuddy too), and others to look at: Filament-Scale-Database
(ESP32 and RFID, to Google Sheets; no licence: ideas only), spoolscale (ESP32-C3, HX711, OLED;
no licence, no Spoolman yet).

## Roadmap

Moved to the wiki: [Roadmap](https://github.com/CuriousTimo/Maker-Connect/wiki/Roadmap).

## Devices in a tree

The full design, with what it gives: [Devices in a Tree](https://github.com/CuriousTimo/Maker-Connect/wiki/Design:-Devices-in-a-Tree)
in the wiki. In short, designed 2026-11-04, before the first plug: devices are not a flat list. Two relations, kept
apart because they answer different questions:

1. **Powered by** (energy): a device hangs behind a channel of another device that switches
   or measures power: a Shelly Plus 2PM has two, a Tasmota power strip four. Several devices
   can share one channel (a dryer and a lamp on one strip), and the tree can go deeper (a
   strip behind a wall plug). The hub refuses a loop.
   - A device whose plug is off shows "off at the plug", not "disconnected", and its page
     offers to switch the plug on.
   - Switching off is refused while something behind the plug is busy or still hot (a
     printer's bed or nozzle above 50 °C), unless forced.
   - The plug's power and energy are shown with the devices behind it, and count for their
     jobs (kWh per job).
   - A job can ask for its machine's plug to be switched on first; rules (see the roadmap)
     build on the same link.
2. **Reached through** (data): the agent or broker a device talks to the hub through. Today
   one agent drives one machine. One agent on a Pi next to a printer, a label printer and a
   bench power supply will register each of them as a device of its own, under itself, with
   one connection to the hub. A Shelly reached through an MQTT broker sits under that
   broker the same way.

In the store: `machines.powered_by` (a device id) with `machines.power_channel` (a number),
and `machines.reached_through` (a device id), all optional. A plug, a power meter or a bench
supply is a device like any other, of its own kind (`power`, `supply`).
