<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.svg">
    <img src="assets/logo.svg" alt="Maker Connect: an M drawn as one toolpath, with dots for the machines it connects" width="420">
  </picture>
</p>

# Maker Connect

One place for all of a maker's machines: 3D printers, lasers, and later CNC machines, cutting
plotters and label printers, each managed its own way today. Watch them live, control them,
send them files, and see how their jobs go, from a website you host yourself (a Pi, Docker, an
old laptop, a NAS). Shaped so that it could one day be hosted for many makerspaces at once.

A proof of concept, in Rust. Licence: [AGPL-3.0](LICENSE).

<p align="center">
  <img src="docs/screenshots/machines.png" alt="The machines of a space, grouped by kind: a Prusa MINI, an Ultimaker, a Zaribo printing a cube at 28 %, and a laser" width="860">
</p>

| | |
|---|---|
| ![A printer's control tab: the bed with the head where it is, X, Y and Z, the job with its thumbnail and progress, and the controls](docs/screenshots/control.png) | ![A printer's telemetry: nozzle, bed and fan tiles, and the temperatures of the last half hour](docs/screenshots/telemetry.png) |
| **Control**: the bed with the head where it is, the job, moves, heating, the fan | **Telemetry**: temperatures and fans, live, with a table view |
| ![The bed mesh as a surface to turn, with its range, lowest and highest point](docs/screenshots/bed-level.png) | ![The files page: G-code with its thumbnail, time and filament, and the machines it fits](docs/screenshots/files.png) |
| **Bed level**: the mesh as a surface, a map and a table | **Files**: what the slicer wrote, and the machines a file fits |

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
- **Files and jobs.** Drop G-code on the Files page; the cloud reads what the slicer wrote
  (time, filament, layer height, the printer it was sliced for, the thumbnail) and offers only
  the machines it fits. Jobs show progress, time left, pause and stop.
- **Machine pages**: Overview, Control, Terminal (G-code and the machine's answers),
  Telemetry, Bed level (Prusa `G80`/`G81`, Marlin `G29`/`M420 V`, read the way the OctoPrint
  Bed Level Visualizer does), Settings.
- **Machine profiles** (`profiles/`, one TOML file and one drawing per model): build volume,
  bed, firmware, how a machine is recognised. Prusa MINI, MK2.5S, MK3S, Zaribo 320,
  Ultimaker 3 Extended, Ortur Laser Master 2 Pro S2.
- **Virtual machines** for every profile, to try everything without a machine: printers home,
  heat, move, measure their bed and play files (ten times faster than real); virtual lasers
  run on LaserFoundry's simulated GRBL, through the same code as a real one.

## Try it

```sh
cargo run -p maker-connect-cloud                 # http://localhost:8460
```

Then "Add machine", "A virtual one", and pick a printer or a laser. To print, drop a file on the
Files page: [examples/laser](examples/laser/README.md) has two test files for a laser.

A real machine, through an agent on the computer it is plugged into:

```sh
cargo run -p maker-connect-agent -- run --cloud http://localhost:8460 --name "MK3S Gonzo" \
    --port /dev/cu.usbmodem1234                   # shows a code: enter it under "Add machine"
cargo run -p maker-connect-agent -- run --cloud http://localhost:8460 --name "Ortur" \
    --kind laser --port /dev/cu.usbserial-1420    # GRBL is recognised; choose its profile
```

`maker-connect-agent ports` lists the serial ports. A Prusa MINI is pointed at the cloud with a
settings file on its USB stick: see [docs/prusa-connect-protocol.md](docs/prusa-connect-protocol.md).

## How it is built

| Part | What |
|---|---|
| **cloud** (`crates/cloud`) | The website and the API: one binary, one SQLite file, uploaded files beside it. Every record belongs to a space |
| **agent** (`crates/agent`) | Next to a machine: drives it over USB, reports to the cloud, carries out its commands and jobs |
| **grbl** (`crates/grbl`) | GRBL machines (lasers), on LaserFoundry's `laserfoundry-device` and `laserfoundry-gcode`; used by the agent and by the cloud's virtual lasers |
| **protocol** (`crates/protocol`) | What agents and the cloud say to each other |

The tools that make the jobs stay the makers' own: PrusaSlicer and Cura for printers,
LaserFoundry for lasers. Maker Connect takes it from the file on. See
[docs/integrations.md](docs/integrations.md), also for label printers.

## Not yet

No login: run it where only you can reach it, since anyone who reaches it can move your
machines. Printing over USB on a Marlin printer through the agent, files for the MINI over
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

`reference/` holds read-only clones of these projects for study; it is not part of this
repository. Maker Connect is independent of the makers of the machines and software it names.
