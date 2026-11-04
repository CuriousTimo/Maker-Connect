<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.svg">
    <img src="assets/logo.svg" alt="Maker Connect: an M drawn as one toolpath, with dots for the machines it connects" width="420">
  </picture>
</p>

# Maker Connect

One service for all of a maker's machines: 3D printers, lasers, CNC, paper printers, each
managed its own way today. A proof of concept, in Rust. Self-hosted on whatever you have
(a Pi, Docker, an old laptop, a NAS), and shaped so that it could one day be hosted for
many makerspaces at once.

Licence: [AGPL-3.0](LICENSE).

## Two parts

| Part | What | Runs on |
|---|---|---|
| **agent** (`crates/agent`) | The module next to a machine: drives it, reports to the cloud. Knows printers on USB that speak the Marlin protocol | A Pi or small computer next to the machine |
| **cloud** (`crates/cloud`) | The website and the API the agents report to: the machines of a space, live. One binary, one SQLite file | Your own server |
| **protocol** (`crates/protocol`) | What the two say to each other, shared so they always fit | — |

The agent always starts the conversation (as Prusa Link does with Prusa Connect), so a
machine works behind any NAT, with no port to open where it stands.

## Try it

```sh
cargo run -p maker-connect-cloud                    # http://localhost:8460
cargo run -p maker-connect-agent -- run --cloud http://localhost:8460 --name "MK3S Gonzo" \
    --port /dev/cu.usbmodem1234                     # the agent shows a pairing code
```

Enter the code under "Add machine" on the website: the machine appears with its state, firmware and
temperatures. Without `--port` (or with the printer off) it shows as disconnected.

**Machine profiles** (`profiles/`, one TOML file per model): the build volume, the bed and
how a machine is recognised. The agent asks the printer what it is (`M115`, and on a Prusa
`M862.2 Q`), the cloud picks the profile, and the owner can choose another on the machine's
page, which draws its bed to scale. For now: Prusa MINI, MK2.5S, MK3S, the Zaribo 320, the
Ultimaker 3 Extended and the Ortur Laser Master 2 Pro S2, each with a drawing of its own.

**Virtual machines**: "Add machine", then "A virtual one", adds a machine simulated in
the cloud, for any profile: it homes, moves, heats and cools, and runs its fan as a real
one would, so everything can be tried without a machine at hand.

**A machine's page** has tabs: Overview (its picture and facts), Control (its bed with the
head where it is, X/Y/Z and a Z bar, moves, heating, the fan, and how each command fared),
Terminal, Bed level, Telemetry
(temperatures over the last half hour, live) and Settings (name, profile, what the machine
told about itself). Everything on it updates live (server-sent events).

**Terminal**: type G-code and see what the machine answers, as in OctoPrint's terminal (an
agent and a virtual machine answer; a MINI on built-in Prusa Connect runs it, but its answers
do not come back over Connect).

**Bed level**: "Probe bed" has a printer measure its bed (Prusa's `G80`/`G81`, Marlin's
`G29`/`M420 V`); the agent reads the mesh from what the printer says (the formats the
OctoPrint Bed Level Visualizer knows), and the page shows it as a surface to turn, a map and
a table. A MINI on built-in Prusa Connect does not send its mesh: over Connect a command's
output does not come back.

**Kinds of machine**: 3D printers, lasers, CNC machines, cutting plotters, label printers and
paper printers, each a group of its own in the list.

**A Prusa MINI needs no agent**: its firmware speaks Prusa Connect, and the cloud speaks
the printer side of that protocol. Point the printer's Connect settings at your cloud (see
[docs/prusa-connect-protocol.md](docs/prusa-connect-protocol.md)) and add it with the code
it shows.

`maker-connect-agent ports` lists the serial ports; `maker-connect-agent send <port>
<file.gcode>` streams a G-code file to a printer on its own.

A proof of concept: the website has no login yet, so run it where only you can reach it.

## Reference

`reference/` holds read-only clones of other projects, for study; it is not part of this
repository. OctoPrint (AGPL-3.0) may be borrowed from; Prusa Link and the Prusa Connect SDK
are "Freeware", read to understand only; the Prusa MINI/MK4/XL firmware (Buddy) is GPL-3.0
for its code. What we learn is in [docs/printer-protocols.md](docs/printer-protocols.md).
