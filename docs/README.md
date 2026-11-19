# Maker Connect documentation

What is where. `scripts/docs-links-check.py` fails when a document named anywhere in the repository
(code comments, scripts, docs) does not exist.

## Design: the v2 target

`design/README.md` is the map of the v2 target design (processes, links, the device contract, flows,
authz, database, API, wire, storage, UI), with dated decision records in `design/decisions/`.

## Plans: what we build, decided and open

| Document                     | What it holds                                                                                   |
|------------------------------|-------------------------------------------------------------------------------------------------|
| `plans/agent-engine.md`      | The agent and its engine: the design, every port's decisions (owner, proxy, architect), K-rows  |
| `plans/hub-side.md`          | The hub side: devices that talk to the hub, the HS step table with status, the list for later   |
| `plans/hub-modules.md`       | The hub as a lean core plus modules (HS0)                                                       |
| `plans/cameras.md`           | Cameras: network RTSP/ONVIF cameras to the hub, USB webcams through the agent                   |
| `plans/mk3-port-plan.md`     | The MK3 port of PrusaLink: reference, file inventory, section order (waits for "sectie 0")      |

## Audits: our code against its one reference

`audits/README.md` lists every device type, its reference and its status. Each audit is checked by a
script in `scripts/*-audit-check.py`; "checked by eye" counts as not checked (`CLAUDE.md`).

## Protocols: how a reference behaves

| Document                          | What it holds                                                                 |
|-----------------------------------|-------------------------------------------------------------------------------|
| `protocols/octoprint-serial-spec.md` | OctoPrint's serial layer for a plain Marlin printer, with default settings |
| `protocols/gcode-metadata.md`     | What a G-code file says about itself, as the hub reads it                     |

## For people and programs that use the hub

| Document               | What it holds                                                              |
|------------------------|----------------------------------------------------------------------------|
| `api/apps-api.md`      | The apps API: sending work to Maker Connect from another program           |
| `api/integrations.md`  | Tools and drivers to work with (laser design, label printers, Spoolman)    |

## Briefs: one per step, written before the work

Each brief says what a step does, how it is proven, what is ours and what is not covered; it is
committed with the work. All briefs below are built.

| Brief                                  | Step                                                             | Built in  |
|----------------------------------------|------------------------------------------------------------------|-----------|
| `briefs/section-i.md` … `section-m-finish-marlin.md`, `section-j-fixes.md`, `section-k-top-edge.md`, `section-l-proof.md`, `gil-model.md` | The Marlin port of OctoPrint 1.11.8 and its proof | `a3dc47a` |
| `briefs/dymo-pnp.md`                   | The DYMO LabelManager PnP port (labelle)                         | `a3dc47a` |
| `briefs/labelwriter-450.md`            | The LabelWriter 450 port (DYMO's CUPS driver)                    | `a3dc47a` |
| `briefs/laser-laserfoundry.md`         | The laser port (LaserFoundry)                                    | `a3dc47a` |
| `briefs/hub-gate.md`                   | No one-command gate for agent machines (B3/B7)                   | `a3dc47a` |
| `briefs/m2-scale.md`                   | The DYMO M-series scale                                          | `1de15b4` |
| `briefs/pnp-idle-poll.md`              | The PnP's idle status poll                                       | `1de15b4` |
| `briefs/hs0a-split-hub.md`             | HS0a: the hub split into a library and modules                   | `4757210` |
| `briefs/hs7a-laser-never-alone.md`     | HS7a: a laser job never starts by itself                         | `8d6828a` |
| `briefs/hs12-spoolman-audit.md`        | HS12: the Spoolman API audit                                     | `8d6828a` |
| `briefs/hs1-lw450-geometry.md`         | HS1: the 450's label sizes from its port                         | `f3371f7` |
| `briefs/hs2-lw450-label-page.md`       | HS2: the 450's own label page                                    | `f3371f7` |
| `briefs/hs2b-dymo-connect-catalog.md`  | HS2b: DYMO Connect's catalogue and PPD 3.1                       | `f3371f7` |
| `briefs/hs7b-laser-load-frame-start.md`| HS7b: load, frame, Start burn                                    | `e1b1b81` |
| `briefs/hs4-ports-fail-pending.md`     | HS4: ports fail pending commands on close                        | `f30810c` |
| `briefs/hs6-emergency-stop.md`         | HS6: an emergency stop per kind                                  | `f30810c` |
| `briefs/hs15-buddy-audit.md`           | HS15: the Buddy 6.3.0 audit                                      | `9f75435` |
| `briefs/usb-cam-step1.md`              | USB webcams, step 1                                              | `89a8c4e` |
| `briefs/hs13-laserfoundry-web.md`     | HS13: the LaserFoundry web fork, served by the hub               | this commit |

## Archive: superseded, kept for reference

Not leading any more; each says so at its top. Their place is taken by the audits.

| Document                             | Superseded by                                           |
|--------------------------------------|---------------------------------------------------------|
| `archive/marlin-serial-protocol.md`  | `audits/octoprint-marlin.md`, `audits/marlin.md`        |
| `archive/prusa-connect-protocol.md`  | `audits/buddy.md`, `audits/prusalink.md`                |
| `archive/printer-protocols.md`       | the audits                                              |

## Other

`design-overview.html` (superseded by `design/README.md`) and `screenshots/` (for the README).
