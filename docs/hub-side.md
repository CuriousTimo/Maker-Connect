# The hub side: devices that talk to the hub directly

Status: **draft, 2026-11-15**. Written while the owner delegated approvals to `owner-proxy`
("Ga door en maak af: … Hub kant"). Items marked *to decide (owner)* are reference choices the
proxy may not make.

## What it is

From `docs/agent-engine.md`: the MINI (Prusa Connect) and Klipper (Moonraker) connect to the hub
themselves. They get an **agent inside the hub**: the same engine (`crates/engine`) and the same
kind of ports, run in the hub's process. The hub then makes no G-code for them either (D9):
`jog_command`, `grbl_action` and `action()` in `crates/hub/src/devices/legacy_gcode.rs`, and the queued
`"G28"`-style strings in `devices/prusa_connect.rs` / `devices/printer_files.rs` / `devices/simplyprint.rs`, go.

Plus **Spoolman**: the hub answers Spoolman's API (`crates/hub/src/spoolman.rs`). The owner
decided it is API- and data-compatible with Spoolman, not a copy of it (memory
`maker-connect-spoolman-compatible`). It has no audit yet.

## The three parts

| # | Part | Wire | Reference | State |
|---|---|---|---|---|
| H1 | Engine inside the hub | none (in-process) | none: ours, same engine as the agent | to build |
| H2 | MINI port | Prusa Connect, the hub is the server | the printer's side: Buddy at the version the MINI runs (`reference/buddy-6.3.0`, `src/connect`), what it sends and accepts | protocol done in the hub; mapping of typed actions *to decide (owner)* |
| H3 | Klipper port | Moonraker's `[simplyprint]` component, the hub is the server | the printer's side: Moonraker `components/simplyprint.py` at `b559b9a` | protocol done in the hub; mapping of typed actions *to decide (owner)* |
| H4 | Spoolman API | HTTP + WebSocket, the hub is the server | Spoolman at `eccb05e` (`reference/Spoolman`, MIT) | built; **audit to make**: run real Spoolman and ours on the same requests, "identical" or where it differs |

## H2 and H3: the open question (to decide, owner)

**Owner, 2026-11-15, for the MINI:** "G-code voor de MINI zit in de mini firmware": the
reference is Buddy 6.3.0 itself, what its own UI sends for the same control. Klipper still open.

Both servers are closed (Prusa Connect, SimplyPrint), so no server code can be ported. What the
printer accepts is known from its own code; what the real server *sends* for a hub control
(jog, home, extrude, heat, fan, speed, flow) is not.

- **MINI.** Buddy 6.3 accepts typed Connect commands (`src/connect/command.cpp:154-192`:
  `START_PRINT`, `PAUSE_PRINT`, `DIALOG_ACTION`, …) and raw G-code as a command
  (`gcode_command`, :90). `SET_VALUE` (:326-415) covers only hostname, enclosure, nozzle
  properties and chamber settings: no nozzle/bed target, fan, speed or flow. So jog, home,
  extrude, heat, fan, speed and flow can only go as raw G-code, and the question is which
  G-code. **Decided by the owner (2026-11-15): Buddy 6.3.0's own menu G-code** (see the top of this
  section); the earlier candidates (PrusaLink's G-code, watching real Prusa Connect) are dropped:
  one reference.
- **Klipper.** Moonraker's `simplyprint.py` (`_process_demand`, :338-355) accepts
  `pause`/`resume`/`cancel`/`gcode`. Candidates for the G-code behind jog/home/extrude/heat:
  (a) what SimplyPrint's own panel sends (closed; needs watching); (b) Mainsail; (c) Fluidd.
  Mainsail and Fluidd are not in `reference/`; adding one is the owner's call.

Until the owner decides, H2/H3 keep the protocol code they have; no new mapping is written.

## H4: the Spoolman audit (can start)

`scripts/spoolman-audit-check.py`: start real Spoolman (`reference/Spoolman`, its own
`pyproject`, SQLite in a temp dir) and send it a fixed list of requests (create vendor,
filament, spool; lists with filters, sort, limit/offset and `x-total-count`; `use`, `measure`;
errors), recording status, every response header real Spoolman sets, and JSON bodies. A Rust test sends the same list
to the hub's `/api/v1` and compares. Every difference is listed with **Result: identical** or
exactly where it differs. What the owner decided is Maker Connect's own (extra fields, places)
is listed as "ours (owner)". The audit ends with an explicit "not covered" list (e.g. SpoolmanDB
import, extra-field definitions, WebSocket sequences if not tested). Real Spoolman runs from
`reference/Spoolman` on SQLite in a temp dir; nothing touches the owner's data.

**Decided (proxy, 2026-11-15):** H4 approved to start; H1 approved as a plan (its design to approve
when written); H2/H3 keep their protocol code, no new mapping until the owner picks the G-code
reference.

## H1: the engine inside the hub (after H2/H3 are decided)

The hub's Prusa Connect and SimplyPrint routes become the ports' transport: a route hands what
the printer sent to its port and takes what the port wants sent. The engine reports to the
hub's store as the agent's hub link does over the network. Written as its own design, for
approval, once H2/H3's mappings are known.

## For the hub side (owner, 2026-11-15, "ok, neem mee als de hub-kant aan de beurt is")

- **PnP tape count.** The hub still counts each PnP label as its picture length + 16 mm
  (`crates/hub/src/store/jobs.rs`, `count_off`, tape taken off the roll; `devices/virtual_machines.rs` ~157). The 16 mm
  is the old agent's own trailing blank, gone with the labelle port. Count the length with
  labelle's own margins instead, taken from the port's constants (`ports/labelle`,
  `labeler_margin_px`), so the count matches what the printer feeds.

## The hub-side plan (draft for review, 2026-11-15)

Everything left for the hub, collected from `docs/agent-engine.md` and the audits, in a proposed
order. Each item gets its own brief, vetted by the proxy and the watchdog before work starts.

| # | Item | Why now / source | Reference |
|---|---|---|---|
| HS1 | The ports' geometry for the hub: a small crate (or engine feature) with only the ported constants (labelle tape sizes and margins; the 450's PPD page sizes and imageable areas), so the hub can depend on it without the device crates | blocks HS2 and HS3; B8 "the hub takes each printer's geometry from its port" | the ports' ported constants |
| HS2 | LabelWriter 450 labels: the hub draws the CUPS page the port needs (imageable area at 300 dpi, `HEAD_DOTS` 672 vs 665) | every hub label to a 450 fails today | DYMO PPD `lw450.ppd` via HS1 |
| HS3 | PnP: tape counted with labelle's own margins; the hub's own 16 mm trailing blank (L9) | tape count wrong; L9 for the owner | labelle via HS1 |
| HS4 | The other agent ports fail pending commands on close (K18 open item), then the gate/120 s goes for them too | B3 completion for PnP, 450, laser | each port's reference |
| HS5 | Persistence: the hub's command queue in SQLite (AUTOINCREMENT ids; Queued cancelled after a hub restart; Done/JobEnd accepted once) and the engine's unreported reports on disk beside the agent's config | §10 weak spots (1b, 1c) | ours (architect's design) |
| HS6 | Emergency-stop button per kind: OctoPrint machines typed `M112`; lasers LaserFoundry's stop; label printers none | decided (point 2) | each reference's path |
| HS7 | A laser never starts unattended: `start_queued` skips lasers; Start on the machine page after framing, operate + PPE | owner's rule | — (hub safety) |
| HS8 | The hub's own G-code for virtual machines (`jog_command`, `grbl_action`, `action()`): virtual machines go through the same ports (virtual lasers already use LaserFoundry) | D9 debt | the ports |
| HS9 | H1: the engine inside the hub (bridge sync engine ↔ tokio, reports into the store) | needed for HS10/HS11 | ours (design first) |
| HS10 | MINI (H2): typed actions → Buddy 6.3.0's own menu G-code (owner's decision), via Prusa Connect; the hub's `M604`, load `M109`/`M701` with 215 °C, etc. go | D9 debt on the MINI | Buddy 6.3.0 |
| HS11 | Klipper (H3): G-code reference still open for the owner | — | owner to choose |
| HS12 | Spoolman audit (H4) | approved, waits for its turn | Spoolman eccb05e |
| HS13 | LaserFoundry web fork: "Send to Maker Connect" / Burn into the laser's queue (Apps API v1 extension), served by the hub | after the laser port (done) | LaserFoundry v0.5.0 |
| HS14 | Docs pass: `docs/design-overview.html`, `docs/audits/README.md`; `scripts/reference-updates.sh` for every reference (pin read from `Cargo.lock`) | decided (points 3, 4) | — |

Proposed order: HS14 (docs, small, makes the plan readable) → HS1 → HS2 → HS3 → HS7 (safety) →
HS4 → HS6 → HS5 → HS8 → HS9 → HS10 → HS12 → HS13 → HS11 (when the owner picks).

### The hub-side plan, reviewed (architect + proxy + watchdog, 2026-11-15)

Order: **HS7a** (now: a laser job never starts by itself — `queue.rs` `start_queued`, `files.rs`
copies, `farm.rs` `dispatch` — guarded once in `store.add_job`; and the "Laser off" button removed
until LaserFoundry has a laser-off call; laser Stop offered also when no job runs) → HS14 (docs
only; the design page pass at the very end) → HS1 (engine Cargo features, the hub depends on the
engine with `default-features = false`, a small `ports::geometry` facade over ported code; test that
the hub has no `serialport`/`nusb`) → HS2 (the 450's raster size and orientation per PageSize,
`fit`/`media_dots`/`label.js`) → HS3 (PnP tape counted from labelle's own bitmap width; the hub draws
no 16 mm blank — the 16 mm exists only in the count, `store/jobs.rs` (`count_off`), `devices/virtual_machines.rs` ~157 —
so L9 is a count fix, not a question; head-to-cutter tape use "unknown") → HS4 (ours, K-row: PnP
nothing pending; 450 and laser fail pending on Close; the hub's gate rule becomes "an agent port that
fails pending on close") → HS6 (the button, our UI; agent OctoPrint already has typed `M112`) → HS7b
(Start on the machine page after framing, operate + PPE + a session; no "just framed" rule) → HS5a/b
(design first, approval: the hub's queue in SQLite; reports on disk with HS9's interface; running a
migration on the Pi is the owner's) → HS9 (design first, MINI scope) → HS8 (after HS9; which port a
virtual FDM machine uses is the owner's choice) → HS15 (new: make `buddy.md` and `klipper.md`
mechanical) → HS10 (Buddy 6.3.0's own menu G-code, a per-button table to approve; the hub's `M604`,
`G28`, 215 °C go; nothing of ours replaces them) → HS13 (the owner decides name, address, access and
the API extension first) → HS11 (the owner picks Klipper's reference). HS12 (Spoolman audit) in
parallel from the start.

**Owner, 2026-11-15: the hub is split up ("opsplitsen is een must").** The hub's own part is kept
and improved step by step; its device side is rebuilt bottom-up as engine ports inside the hub
(HS9/HS10/HS8); `crates/hub/src/main.rs` (~160 KB) and `store.rs` are split into modules. The split
(HS0) comes right after HS7a and HS12 finish (they edit hub files now), before the other hub items;
the architect designs the layout first (per module: keep / adjust / rebuild), for approval.
Owner, same day: "zo kan de hub ook lean en mean op zichzelf draaien" — the split also makes a lean
hub possible: a small core that runs on its own, with the optional parts (the in-hub engine and its
device ports, virtual machines, …) switched on only when wanted.
Owner: "juist aparte modules. en later meer modules voor andere zaken" — a lean core plus separate
modules with a clear boundary, so more modules can be added later without touching the core.
