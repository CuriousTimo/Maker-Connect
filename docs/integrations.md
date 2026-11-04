# Tools and drivers to work with

Maker Connect is the machine side: it keeps the files, queues them, and gets them to the
machines and back what they do. Designing and slicing stay in the tools makers already use.
The same split everywhere: a tool makes the job, Maker Connect takes it from there.

| Tool makes the job | Maker Connect | The machine |
|---|---|---|
| PrusaSlicer, Cura: G-code | files, queue, print, progress | 3D printer (agent, or built-in Prusa Connect) |
| LaserFoundry: GRBL G-code | files, queue, stream (with its `laserfoundry-device`), progress | laser (agent, GRBL) |
| a label template in the cloud | render, queue, print | label printer (agent) |

## Lasers: LaserFoundry

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
the agent (with real serial ports) and the cloud (virtual lasers):

- GRBL recognised on the port by its greeting (or `$I`); commands (`$H`, `$X`, `$$`, any line)
  with their answers for the terminal;
- jogs as GRBL's own `$J=` through the link's `jog` and `jog_to` (laser off, checked);
- the settings that matter, read with `$$` on connecting: laser mode (`$32`), full power
  (`$30`), travel (`$130` to `$132`). The cloud refuses a job with laser mode off or outside the
  travel, and warns when the file's power range does not fit `$30`;
- Frame: a laser file's outline (where its moves go, read on upload) run as jogs, laser off;
- jobs fetched and streamed by character counting, with pause, resume and stop;
- starting again: an interrupted job says where it can start again (`restart_line`, a few lines
  back, in the file's numbering), and the new job starts there with `laserfoundry-gcode`'s
  `resume_from`, which first goes there with the beam off;
- virtual lasers: `SimulatedGrbl` behind the same code, on a thread of their own in the cloud.

Tested against `SimulatedGrbl`; not yet on a real laser.

**Plan (as it was):** the agent uses `laserfoundry-device` as a dependency (from git, pinned to a commit;
the licences fit AGPL-3.0) rather than writing GRBL streaming again; a GRBL machine is then a
`Transport` over our serial port. `SimulatedGrbl` serves the agent's tests, and can stand behind
the virtual laser. The user's Ortur on USB would be among its first real machines: Frame and
low power first. Later: an upload from LaserFoundry to Maker Connect (it already has "Publish
to MakerFoundry", makerfoundry.app, by the same makers), or the cloud turning an SVG into
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
   at a laptop. Against "the machine connects to the cloud", so at most an extra.

The cloud makes the label (template to image) and queues it; the agent prints it. Which label
printer to start with: to be asked.

### DYMO LabelManager PnP: tried (2026-11-04)

On the user's LabelManager PnP (USB `0922:1001`, the only id it showed), from a Mac without
DYMO's software: it shows a small USB stick ("LM PNP", DYMO's software for Windows) **and** a
HID interface (interface 0, usage page `ff00`, interrupt endpoints `0x01` out, `0x81` in, 64
bytes). No mode switch to `0922:1002` is needed: the printer listens on that HID interface as it
is. macOS keeps the interface for its HID driver (libusb or `nusb` cannot claim it), so we talk
to it through the system's HID API (the `hidapi` crate): no root, no driver.

- Each write is one 64-byte output report (report id 0) carrying the next 64 bytes of the
  stream; reads are 8 bytes, the first being the status.
- `ESC A` (`1B 41`): status. `0x40` cassette in, ready; `0x60` while printing (bit 5); `0x00` no
  cassette; `0x10` cutter jammed; `0x04` an error.
- A label: `ESC C 0` (tape type), `ESC B 0` (dot tab), `ESC D 8` (8 bytes a column: 64 dots,
  12 mm tape at 180 dpi; 6 for 9 mm, 4 for 6 mm with dot tab 1 or 2), then one `SYN` (`16`)
  and 8 bytes per column of the label, top pin first (bit 7 of the first byte). Like labelle,
  `ESC A` after every 64 columns, read before going on. Blank columns feed the tape; about
  113 (16 mm) bring the end of the label past the cutter, which is by hand on this model.

From thermal-label's protocol notes (its "D1" family) and labelle (Apache-2.0; its DYMO files
carry Sebastian J. Bronner's permissive notice). thermal-label warns that unknown opcodes can
hang the printer until it is switched off and on.

## Roadmap: what else to connect

Ideas, gathered 2026-11-04; none started unless marked. A maker's space is more than the
machines that make things: power, air, safety and the people using it.

**Power, and devices behind it**
- **Smart plugs and power meters**: Shelly (Gen2+ RPC over HTTP, WebSocket and MQTT), Tasmota
  (MQTT, HTTP). Switch and measure anything with a plug; the base for most of what follows.
- **Devices behind a plug**: one or more devices hang behind a plug's channel (a printer, a
  power strip with a dryer and a lamp). The plug's state explains theirs ("off at the plug",
  not "disconnected"), a device can be switched on from its own page, and the plug's energy
  counts for them. See [the device tree](#devices-in-a-tree) below.
- **Rules between devices**: the fume extractor on while the laser runs and two minutes after;
  the dryer on before a planned print; a printer off once it is done and cooled down; the
  laser's power off when its flame sensor fires.
- **Filament dryers** and **fume extractors**: mostly through a plug; dryers with their own
  Wi-Fi (temperature, humidity, time left) where they have an open protocol.

**The electronics bench**
- **Riden RD6006W** (and the RD60xx family): bench power supply, Modbus RTU over USB serial
  through the agent; its Wi-Fi module connects out to a server, so the cloud can be that
  server, as with the Prusa MINI. Volts, amps, watts live, output on and off, limits.
- **Instruments over SCPI** (LAN, USBTMC): oscilloscopes, multimeters, electronic loads
  (Rigol, Siglent, OWON).
- **Soldering irons** with telemetry: Pinecil (Bluetooth LE).

**More machines**
- **Klipper printers** through Moonraker's API: many hobby printers run it.
- **Bambu Lab** printers (MQTT in LAN mode), **resin printers** (Elegoo's SDCP on the network),
  wash and cure stations.
- **CNC** on GRBL, grblHAL, FluidNC: close to free with `maker-connect-grbl`.
- **Cutting plotters** (Graphtec/Silhouette GPGL, Roland HPGL), later embroidery machines.
- **Label printers**: Brother QL and PT next (TCP 9100), then the other DYMO families.

**Air, safety, the room**
- Air quality (PM2.5, VOC) at the laser and the resin printer; temperature in enclosures.
- A flame sensor in the laser, cutting power through a plug; the chiller's alarm (CW-5200),
  air assist pressure; smoke detectors.
- **Cameras** per machine (webcam, RTSP): live view, timelapses, later failure detection.

**For a makerspace**
- **Status displays**: a kiosk page (a Pi with a screen, an old tablet) with what runs where
  and how long it still takes; small ESP32 displays reading the live events.
- **Access**: an RFID or NFC badge per member; who has had the laser induction; a machine
  switched on (relay or plug) only after a badge.
- **Use and costs**: minutes, kWh and grams per member or per job.
- **Materials**: spools tracked (Spoolman, NFC tags), a scale under the spool.

**Outward**
- **Home Assistant**, through MQTT discovery: every device as entities.
- **Notifications**: ntfy, Telegram, Discord ("print done", "laser stopped with an alarm").
- **Labels as output**: a label with a QR code to the job when a print is done; labels for
  spools and bins.

## Devices in a tree

Designed 2026-11-04, before the first plug: devices are not a flat list. Two relations, kept
apart because they answer different questions:

1. **Powered by** (energy): a device hangs behind a channel of another device that switches
   or measures power: a Shelly Plus 2PM has two, a Tasmota power strip four. Several devices
   can share one channel (a dryer and a lamp on one strip), and the tree can go deeper (a
   strip behind a wall plug). The cloud refuses a loop.
   - A device whose plug is off shows "off at the plug", not "disconnected", and its page
     offers to switch the plug on.
   - Switching off is refused while something behind the plug is busy or still hot (a
     printer's bed or nozzle above 50 °C), unless forced.
   - The plug's power and energy are shown with the devices behind it, and count for their
     jobs (kWh per job).
   - A job can ask for its machine's plug to be switched on first; rules (see the roadmap)
     build on the same link.
2. **Reached through** (data): the agent or hub a device talks to the cloud through. Today
   one agent drives one machine. One agent on a Pi next to a printer, a label printer and a
   bench power supply will register each of them as a device of its own, under itself, with
   one connection to the cloud. A Shelly reached through an MQTT broker sits under that
   broker the same way.

In the store: `machines.powered_by` (a device id) with `machines.power_channel` (a number),
and `machines.reached_through` (a device id), all optional. A plug, a power meter or a bench
supply is a device like any other, of its own kind (`power`, `supply`).
