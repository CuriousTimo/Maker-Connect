# The hub side: devices that talk to the hub directly

Status: **draft, 2026-11-15**. Written while the owner delegated approvals to `owner-proxy`
("Ga door en maak af: … Hub kant"). Items marked *to decide (owner)* are reference choices the
proxy may not make.

## What it is

From `docs/agent-engine.md`: the MINI (Prusa Connect) and Klipper (Moonraker) connect to the hub
themselves. They get an **agent inside the hub**: the same engine (`crates/engine`) and the same
kind of ports, run in the hub's process. The hub then makes no G-code for them either (D9):
`jog_command`, `grbl_action` and `action()` in `crates/hub/src/main.rs`, and the queued
`"G28"`-style strings in `prusa_connect.rs` / `printer_files.rs` / `simplyprint.rs`, go.

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
  G-code. Candidates: (a) what PrusaLink sends for the same control (Prusa's own host software,
  being ported, D1); (b) what real Prusa Connect sends, learned by watching a MINI on it (still
  to do; listed as open in the notes). Recommendation (proxy): (a), with (b) for what PrusaLink
  lacks.
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
  (`crates/hub/src/store.rs` ~1229, tape taken off the roll; `virtual_machines.rs` ~157). The 16 mm
  is the old agent's own trailing blank, gone with the labelle port. Count the length with
  labelle's own margins instead, taken from the port's constants (`ports/labelle`,
  `labeler_margin_px`), so the count matches what the printer feeds.
