# Brief: HS7b — a laser job is loaded, aligned, then "Start burn" by a person at the laser

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust; never commit unless told).
PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`.

Owner, 2026-11-17, after the first motion test of the Ortur LM2 Pro S2 (Home, Jog, Set origin, Go origin
and a frame all worked on the hardware): "die start niet een job als een 3d printer. die zend hem naar
de laser en zet hem klaar. dan komt frame, set origin, weer frame, jog tot het goed is enz enz", "dan als
alles klaar staat -> start job", "of start burn". Standing rules: a laser job never starts by itself
(HS7a, memory maker-connect-laser-never-unattended: Start only by a person at the laser, operate + PPE +
a session, never an API token). And today: the laser stands in the living room — NO burning; every test
of this step stops before Start burn.

Reference: LaserFoundry v0.5.0 (`61febdb`), its app (`crates/ui-egui`) and `Link` (`crates/device`):
- Frame (`panels.rs:734`) and Start (`panels.rs:743`, `lib.rs:1240-1266`) both come from the same loaded
  job, so what is framed is what is burned.
- Set origin is `Link::set_zero` = `G92 X0 Y0` (`link.rs:238`); Go origin `G90 G0 X0 Y0`; jog `$J=…`.
  The frame and job programs are plain `G0`/`G1` moves (no `G53`), so GRBL applies the `G92` offset to
  both: "set origin, frame again" should already follow the origin — **to verify on the laser**, not assumed.
- Before a start: the app refuses when laser mode (`$32`) is off (`lib.rs:1240`), and when the job's place
  on the bed is not known to the program (a user origin or the head's position) it asks the person to
  confirm first (`lib.rs:1250`, `pending_start`).

What changes:
1. **Load, not start.** Sending a laser job ("Print"/"Burn" on a file, the LaserFoundry web front end's
   Burn) puts it on the laser as its **loaded job**: one per laser, shown on the machine page with its file,
   its extent and who loaded it. It never starts by itself (HS7a's guard stays). A new load replaces the
   loaded job only after the person confirms; nothing is queued behind it for a laser.
2. **Align at the laser** (machine page, laser tab), all motion only, as LaserFoundry: Home, Jog, Set
   origin, Go origin, and **Frame** — of the loaded job (no separate file picker), with LaserFoundry's
   `gcode::frame` at 0 % as today (G4/G12). The frame follows `G92` as GRBL does; the hub claims nothing more.
3. **Start burn**, on the machine page only, enabled when: a job is loaded, the laser is idle, it was
   framed since the last Set origin or jog (ours: "frame before burn", the owner's flow), laser mode is on
   (`$32 = 1`, from the controller, as LaserFoundry refuses otherwise), and the person may operate it, has
   confirmed PPE and has a session (HS7a). When an origin was set (`G92` since homing), the person confirms
   "the job's place on the bed is not checked" first, as LaserFoundry does. Start burn runs LaserFoundry's
   own start (`GrblMachine::start`, G5) of the loaded job's file — nothing of ours on the wire.
4. **The hub's own bed check** (a file's `bounds_mm` against the travel): after Set origin the bounds are
   relative to the new zero, so the hub cannot check them (LaserFoundry neither): say so in the confirm, do
   not refuse.
5. Not in HS7b: designing on the hub, "Start here"/restart from a line (exists as G5's `from_line`, UI later),
   cameras on the bed, the web fork (HS13).
6. Hardware proof — ONLY THE OWNER, and with **no burning** while the laser is in the living room: load the
   frame-test file, frame, Set origin elsewhere, frame again (does the square move with the origin?), jog,
   frame; Start burn must stay disabled until framed, then be enabled — **not pressed**.

Questions for the proxy:
- Q1 "Framed since the last Set origin or jog" as the condition for Start burn: ours (LaserFoundry has no such
  gate; it has the confirm for a user origin). Keep it (the owner's flow) or only LaserFoundry's confirm?
- Q2 One loaded job per laser, replacing only after a confirm, and no laser queue: acceptable, or keep the
  queue with "waits for someone at the laser" (HS7a) and make the first queued job the loaded one?

`cargo test --workspace` passes; no new warnings. Tests: a laser job sent is loaded and never started (queue,
farm, reprint, API token); Frame uses the loaded job; Start burn refused without a loaded job, while busy,
before a frame after Set origin/jog, with `$32` off, without operate/PPE/session; the confirm after Set origin;
Start burn starts the loaded file once.

Watchdog before the step (2026-11-17): DRIFTING, corrected here:
- The line numbers above are of the pinned LaserFoundry revision `61febdb` (the cargo checkout the
  agent is built from). `reference/laserfoundry` is an older copy (`3b9b9fa`, 2026-11-03), where the
  same code sits at `lib.rs:1167` (`start_job`), `:1185` (`$32`), `:1197` (`pending_start`); the
  reference folder should be brought to `61febdb` (separate step).
- "Framed since the last Set origin or jog" (Q1) contradicts the reviewed plan (hub-side.md: "no 'just
  framed' rule"): only the owner can decide it. Q2 (one loaded job, no laser queue) touches HS7a/HS13:
  the owner's too.
- "One loaded job per laser" and "replace only after a confirm" are ours (hub UI), named as such.
- Order: the owner chose HS7b now, ahead of HS4/HS6 ("ja, schrijf de brief voor HS7b", 2026-11-17).

Proxy (2026-11-17): acceptable with corrections, folded in here:
- "An origin was set" is our own hub state (LaserFoundry keys its confirm on the design's `start_from`,
  which a G-code file has not); nothing of it reaches the wire. Whether homing clears `G92` is not claimed
  until checked on the laser.
- Use LaserFoundry's own `$32` advice and its not-ready checks, no new reads of ours.
- "Framed" = a completed frame of the same loaded job; an aborted frame does not count; a new load resets it.
- Existing laser queue rows are not deleted silently: they become loadable (the person picks one to load).
- The Start burn confirm texts are ours.
- Tests: the frame program has no `S` above 0 (it does have LaserFoundry's `M3 S0`/`M4 S0`, already run on
  the laser by the owner, 2026-11-17); no test ever sends Start burn to a real device.
- Recommendation for the owner: Q1 keep the gate (Set origin; "or jog" optional, the owner decides);
  Q2 one loaded job, no laser queue.

Owner, 2026-11-17: **"ja framen na set origin, niet na jog. één job, geen wachtrij"**.
- Q1: Start burn needs a completed frame of the loaded job after the last Set origin; a jog does not
  reset it.
- Q2: one loaded job per laser, no laser queue.

Built (2026-11-17). Watchdog after the step: ON TRACK (no path brings a laser job to the laser without
Start burn; nothing new on the wire; the owner's decisions as built). Its findings, corrected:
- a new load is framed anew (`add_job` clears the laser's frame);
- a loaded job can be taken off ("Take it off", `stop` on a loaded job: it ends stopped, never started);
- what still waits in a laser's old queue gets a "Load" button (the queue takes nothing new for a laser);
- every Start burn asks the person to confirm (ours); after a Set origin with LaserFoundry's meaning (the
  place on the bed is not checked). After a hub restart the frame and the origin are forgotten: the job is
  framed again, and the confirm is asked all the same.
- Not covered: a `G92` typed in the terminal is not tracked as a Set origin (the terminal is Operate).

Hardware proof (owner, 2026-11-17, Ortur LM2 Pro S2, no burning): load → nothing moves; Frame runs the
loaded file's outline; after a Set origin elsewhere the frame follows the new origin ("vierkant volgt het
nieuwe nulpunt"); Start burn stays disabled until the frame is reported done, then turns red live (the
card follows the hub's state over the machine stream); Start burn not pressed. The hub logs each command's
outcome (`command <label> on <machine>: Finished/Rejected (reason)`).
