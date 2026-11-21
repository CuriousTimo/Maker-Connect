# Brief: the laser port (LaserFoundry), in parallel with the Marlin proof and the 450

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust workspace; git repo, never commit; never
deploy or touch hardware). PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`.
**Build in your own target directory:** `export CARGO_TARGET_DIR=target-laser`. Two other agents
work at the same time (the Marlin proof: `crates/engine/src/ports/octoprint/` and its scripts; the
LabelWriter 450: `crates/engine/src/ports/dymo_cups/`): never touch those folders or
`ports/labelle/`; in shared files (`crates/engine/Cargo.toml`, `crates/engine/src/ports/mod.rs`,
`crates/agent/src/ports.rs`, `crates/agent/src/link.rs` `Mode`, `Cargo.lock`, `docs/plans/agent-engine.md`, `scripts/reference-updates.sh`, `docs/audits/README.md`)
only add lines and re-read before writing. If the engine crate fails to compile because of another
folder, wait and retry; never fix it yourself.

Read `CLAUDE.md`, then `docs/plans/agent-engine.md`: the engine (O1–O8), hub link (S1–S5), B2–B6, and at
the end "D4 decided", "Lasers, one stack" and "Owner overrules the watchdog on LaserFoundry": the
one reference and library is **LaserFoundry** (`reference/laserfoundry`; crates `laserfoundry-device`
and `laserfoundry-gcode`, MIT OR Apache-2.0). Where LaserFoundry falls short, the fix belongs
upstream in LaserFoundry (write it down as a proposed contribution; do not put that behaviour into
our code). Everything of ours is marked "ours" and listed.

**Only the GRBL laser.** Marlin and Smoothieware lasers stay closed (no mode) until tried on a
machine. Nothing of OctoPrint, the DYMO printers, the web fork or the hub side.

## 1. LaserFoundry v0.5.0

Move the pin in `crates/grbl/Cargo.toml` from 3b9b9fa to the v0.5.0 tag (`61febdb`; it brings #76
`resume_from` after `M5`, #121 MPos over WPos, #64 frame feed limit). Fix whatever their API change
needs. The hub's virtual lasers (`crates/hub/src/virtual_grbl.rs`) must keep working.

## 2. `crates/grbl`: only LaserFoundry's behaviour on the wire (the architect's fixes)

- Remove the `$I` send at connect (`lib.rs` ~49): LaserFoundry never sends it. `build_info` and
  what it feeds go with it.
- `$$` (`request_settings`) only once the first status report has arrived, as LaserFoundry's app does
  (`reference/laserfoundry/crates/ui-egui/src/lib.rs` ~1326-1330, `settings_asked`).
- Port the rest of the app's link logic that a host needs, from `ui-egui` (with file:line): the job
  refused when laser mode is off (find where the app does it), and what happens on a lost
  connection: port `ui-egui/src/lib.rs` ~1313-1323 exactly, including the reconnect on a serial
  target (`auto_reconnect` defaults to true, :411; only when `!platform.serial_async()`, :1317);
  state what that means for a host without an egui platform as a numbered ours-item; nothing about
  reconnecting may change what is sent.
- `carry_out` must not block the port: hand the line to `Link` and return; the `CommandDone` comes
  later from the snapshot as the port keeps reading (as the OctoPrint port's K11). `$H` and the
  frame must not block the port either; where LaserFoundry itself blocks, list it as a gap that
  fails loudly, or as a proposed upstream contribution. A command with
  no answer must fail, never count as done. Remove the 90 s `ANSWER_WITHIN`, the 3 s wait in
  `start()` and the 50 ms / 5 ms sleep loops, or, where one is still needed, list it as ours with
  the reason (none of them may change what is sent).
- `greets()` (telling GRBL from Marlin on a port) is not used to choose a stack: the firmware is
  told at start-up.

## 3. The port: `crates/engine/src/ports/laserfoundry/port.rs` (ours, items G1, G2…, "to approve (proxy)")

A `Port` for `--mode grbl` around `GrblMachine`: the laser `Action`s (Unlock, LaserOff, SetOrigin,
GoOrigin, ReadSettings, Frame{download}, Jog, Home, Gcode) to LaserFoundry's `Link` calls (`$X`,
`M5`/laser off as LaserFoundry does it, set zero, go to zero, `$$`, frame, `$J=` jog, `$H`, a line);
FDM actions (temperatures, fan, extrude, speed/flow, filament, SD): failed `CommandDone`
"LaserFoundry has no such control". Jobs: stream as LaserFoundry's `Link` does; pause/resume/stop
as its calls; `from_line` → LaserFoundry's `restart_line` + `resume_from`. Telemetry from its
snapshot (state, position, progress, the settings `$30`/`$32`/`$130`–`$132` the hub uses); no
polling of our own (LaserFoundry's own 200 ms status poll is theirs). Write "Step 3d: the laser"
at the end of `docs/plans/agent-engine.md` with G1…; also list the hub's laser checks in `files.rs`
(`laser_check`: `$32` off refused, travel, power range warning) as ours, in the UI, under B3.

## 4. Wiring

`crates/agent/src/ports.rs`: `open` returns this port for `--mode grbl` with `--port` and `--baud`
as given (no default of ours; if LaserFoundry has a default baud, it decides; if it has none,
`--baud` is required and its absence is a loud error). Only add lines.

## 5. Proof

Port LaserFoundry's own tests that cover `Link` and `resume_from` behaviour we rely on, or run them
(`cargo test -p laserfoundry-device -p laserfoundry-gcode` in `reference/laserfoundry` at v0.5.0,
with `CARGO_TARGET_DIR` set to your own directory)
and say they pass. Then a comparison: drive LaserFoundry's own `SimulatedGrbl` once through
LaserFoundry's `Link` directly (the reference) and once through our port, the same scenarios
(connect, settings, home, unlock, jog, frame, set/go origin, a job to the end, pause/resume, stop,
restart from a line, a lost connection), recording every byte sent; identical, or exactly where it
differs. Write `docs/audits/laserfoundry-grbl.md` (audit format, "not covered" list) and its row in
`docs/audits/README.md`. Update `scripts/reference-updates.sh`: LaserFoundry pinned to v0.5.0 and
compared with its newest `v*` tag.

`cargo test --workspace` (your target dir) must pass; no new warnings.

## Report (brief)

Files; the pin move; each `crates/grbl` fix; G1…; the proof result; proposed upstream contributions.

## Additions from the watchdog (binding; line numbers are v0.5.0, `crates/ui-egui/src/lib.rs`)

1. Link logic: `poll_link` 1380-1404 (`settings_asked` 1401-1404, field 262, reset 1178/1361) and
   `reconnect_tick` 1345-1376. `auto_reconnect` defaults to true (455): port the reconnect as it is
   (`RECONNECT_ATTEMPTS` 8 at :82, `RECONNECT_MAX_WAIT` 30 s at :84, `reconnect_wait` 2 s at :457,
   doubling, only for a serial target when `!platform.serial_async()` :1392, a running job is never
   resumed), each constant with file:line.
2. `start_job`'s refusals, all of them, each ported (with file:line) or listed as not covered:
   `not_ready(state)` (1233, 1520-1532), laser mode off (1239-1246, `settings_ui::advice` →
   port the `$32` logic from `settings_ui.rs` with file:line), the bed/origin confirmation (1248).
3. `LaserOff`: GRBL's `Link` has no laser-off call. Map it to what LaserFoundry's own UI does for
   its own laser-off control if it has one (find it, file:line); otherwise fail it as "no such
   control". Never a bare `M5` of ours.
4. Frame is `gcode::frame` (`crates/gcode/src/lib.rs:92`) run as a job through `start_job`, as the
   app does (`panels.rs:734-740`).
5. Baud: LaserFoundry's `serial::DEFAULT_BAUD` = 115 200 (`serial.rs:9`) is the default when
   `--baud` is not given.
6. Proof: the "direct" side follows the app's own call sequence (with file:line), not a script of
   ours; reuse the bytes `link.rs`'s own tests pin; the lost-connection/reconnect case comes from
   the `ui-egui` tests (~`tests.rs:2693`) or is "not covered"; the 200 ms `?` status polls are
   counted separately from the compared bytes (ours, in the audit); run v0.5.0's tests in a separate
   `git worktree` of `reference/laserfoundry`, never by moving its checkout.
7. Note in the G-items: `--mode grbl` on a Smoothieware machine is untried (it stays closed).
