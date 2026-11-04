<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.svg">
    <img src="assets/logo.svg" alt="Maker Connect: an M drawn as one toolpath, with dots for the machines it connects" width="420">
  </picture>
</p>

# Maker Connect

One place for all of a maker's machines: 3D printers, lasers, label printers, and later CNC
machines and cutting plotters, each managed its own way today. Watch them live, control them,
send them files, and see how their jobs go, from a website you host yourself (a Pi, Docker, an
old laptop, a NAS). Shaped so that it could one day be hosted for many makerspaces at once.

A proof of concept, in Rust. Licence: [AGPL-3.0](LICENSE).

<p align="center">
  <img src="docs/screenshots/machines.png" alt="The machines of a space, grouped by kind: two Prusas, one printing a cube at 33 %, a laser and a DYMO label printer" width="860">
</p>

| | |
|---|---|
| ![A printer's control tab: the bed with the head where it is, X, Y and Z, the job with its thumbnail and progress, and the controls](docs/screenshots/control.png) | ![A printer's telemetry: nozzle, bed and fan tiles, and the temperatures of the last half hour](docs/screenshots/telemetry.png) |
| **Control**: the bed with the head where it is, the job, moves, heating, the fan | **Telemetry**: temperatures and fans, live, with a table view |
| ![The bed mesh as a surface to turn, with its range, lowest and highest point](docs/screenshots/bed-level.png) | ![The files page: filters by type and machine, labels and G-code with their pictures, where each was printed](docs/screenshots/files.png) |
| **Bed level**: the mesh as a surface, a map and a table | **Files**: sorted and filtered by type and machine, where each was printed, removed many at once |
| ![A label printer's label tab: text, font, a QR code, and the preview of the dots that will burn on 12 mm tape](docs/screenshots/label.png) | ![The downloads page: the agent for macOS and Windows, with the command to start it](docs/screenshots/downloads.png) |
| **Label**: text, a QR code or barcode, a drawing; the preview is the dots that will burn | **Downloads**: the agent for each platform, with the command for this cloud |

## What works

- **Machines report themselves.** An agent next to a machine, or the machine's own firmware,
  connects to the cloud: no port to open where the machine stands. Add one with the code it
  shows. Every page updates live (server-sent events).
- **A Prusa MINI needs no agent.** Its firmware speaks Prusa Connect; the cloud speaks the
  printer side of that protocol. Point the MINI's Connect settings at your cloud and it
  registers, reports, and takes commands. Tried on a real MINI (firmware 6.3.0).
- **Printers on USB** through the agent: Marlin and Prusa's 8-bit firmware (line numbers,
  checksums, resends), recognised by what they say (`M115`, `M862.2 Q`).
- **Lasers (GRBL)** through the agent, with [LaserFoundry](https://github.com/Stitchwright/laserfoundry)'s
  device crate: homing, GRBL's own jogs, the controller's settings checked before a job
  (laser mode, full power, travel), Frame (the outline with the laser off), streaming by GRBL's
  character counting, pause, resume, stop (laser off), and starting an interrupted job again a
  little before where it stopped. Not yet tried on a real laser.
- **Label printers (DYMO LabelManager PnP)** through the agent, over USB HID: no DYMO
  software, no driver, no mode switch. A label printer's page draws the label as SVG (text,
  frame, a drawing you add) at the tape's size and shows the dots that will burn; Print sends
  them, with a QR code or a barcode (Code 128, EAN-13) on whole dots if you like. Tried on a
  real PnP from a Mac and from a Windows laptop (12 mm tape); you say which tape is loaded, since the
  printer only tells whether a cassette is in.
- **Files and jobs.** Drop G-code on the Files page; the cloud reads what the slicer wrote
  (time, filament, layer height, the printer it was sliced for, the thumbnail) and offers only
  the machines it fits. Jobs show progress, time left, pause and stop.
- **Machine pages**: Overview, Control, Terminal (G-code and the machine's answers),
  Telemetry, Bed level (Prusa `G80`/`G81`, Marlin `G29`/`M420 V`, read the way the OctoPrint
  Bed Level Visualizer does), Settings.
- **Machine profiles** (`profiles/`, one TOML file and one drawing per model): build volume,
  bed, firmware, how a machine is recognised. Prusa MINI, MK2.5S, MK3S, Zaribo 320,
  Ultimaker 3 Extended, Ortur Laser Master 2 Pro S2, DYMO LabelManager PnP (with its tapes).
- **Virtual machines** for every profile, to try everything without a machine: printers home,
  heat, move, measure their bed and play files (ten times faster than real); virtual lasers
  run on LaserFoundry's simulated GRBL, through the same code as a real one; a virtual label
  printer takes labels and "prints" them at a real one's pace.

## Try it

```sh
cargo run -p maker-connect-cloud                 # http://localhost:8460
```

Then "Add machine", "A virtual one", and pick a printer, a laser or a label printer. For a real
machine, the cloud's **Downloads** page has the agent for each platform, with the command to
start it: [Getting Started](https://github.com/CuriousTimo/Maker-Connect/wiki/Getting-Started)
in the wiki goes through it, [The Agent](https://github.com/CuriousTimo/Maker-Connect/wiki/The-Agent)
has its options and its config file.

## Wiki

Guides and designs live in the [wiki](https://github.com/CuriousTimo/Maker-Connect/wiki):
getting started, the agent, label printers, machine profiles; the designs for devices behind
smart plugs, a multi-tenant cloud, and consumables and stock; and the roadmap.

## How it is built

| Part | What |
|---|---|
| **cloud** (`crates/cloud`) | The website and the API: one binary, one SQLite file, uploaded files beside it. Every record belongs to a space |
| **agent** (`crates/agent`) | Next to a machine: drives it over USB, reports to the cloud, carries out its commands and jobs |
| **grbl** (`crates/grbl`) | GRBL machines (lasers), on LaserFoundry's `laserfoundry-device` and `laserfoundry-gcode`; used by the agent and by the cloud's virtual lasers |
| **label** (`crates/label`) | Labels as rasters (PNG in, dots out), and the DYMO LabelManager's protocol over USB HID |
| **protocol** (`crates/protocol`) | What agents and the cloud say to each other |

The tools that make the jobs stay the makers' own: PrusaSlicer and Cura for printers,
LaserFoundry for lasers. Maker Connect takes it from the file on. See
[docs/integrations.md](docs/integrations.md), also for label printers.

## Not yet

No login: run it where only you can reach it, since anyone who reaches it can move your
machines (and download the agent). Label printers other than the DYMO LabelManager PnP, and
codes scanned off real labels, are still to try. Printing over USB on a Marlin printer through the agent, files for the MINI over
Prusa Connect, binary G-code (`.bgcode`), queues and a G-code viewer are still to come. Values
marked `# check` in the profiles are not yet confirmed on the machines.

**Safety:** a laser and a heated printer can start a fire. Frame a laser job and start at low
power; stay with a running machine.

## References and credits

- [LaserFoundry](https://github.com/Stitchwright/laserfoundry) (MIT OR Apache-2.0): its
  `laserfoundry-device` and `laserfoundry-gcode` crates drive GRBL lasers here, as a dependency.
- [OctoPrint](https://github.com/OctoPrint/OctoPrint) (AGPL-3.0): the reference for talking to
  printers; its Bed Level Visualizer plugin (MIT) for the mesh formats.
- Prusa-Firmware-Buddy (GPL-3.0 code) and Prusa-Firmware: read to learn the Prusa Connect
  protocol and the printers' replies. Prusa Link and the Prusa Connect SDK ("Freeware") were
  read to understand, never copied. Notes: [docs/printer-protocols.md](docs/printer-protocols.md).
- [thermal-label](https://thermal-label.github.io/) (MIT): the DYMO LabelManager's protocol and
  its media list; [labelle](https://github.com/labelle-org/labelle) (Apache-2.0) for the same
  protocol in practice. Notes: [docs/integrations.md](docs/integrations.md).
- [qrcode](https://crates.io/crates/qrcode) and [barcoders](https://crates.io/crates/barcoders)
  (both MIT OR Apache-2.0): QR codes and barcodes on labels.

`reference/` holds read-only clones of these projects for study; it is not part of this
repository. Maker Connect is independent of the makers of the machines and software it names.
