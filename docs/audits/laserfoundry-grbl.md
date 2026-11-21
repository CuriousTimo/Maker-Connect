# GRBL lasers: our port against LaserFoundry

**The one reference:** LaserFoundry (github.com/StitchFoundry/laserfoundry, MIT OR Apache-2.0) at
**v0.6.0** (`8f07f98`) plus two commits of ours on the fork branch `grbl-rx-buffer`, up to commit `fd87dc3`
(`fd87dc3ee32674ee45a23e488e05969c301eb9f3`, github.com/CuriousTimo/laserfoundry): the RX buffer that
`$I` reports, from the gnea/grbl wiki, and the lines that fit that buffer sent in one write (#168),
offered upstream in #169. It is read and run in a separate
`git worktree` at that commit; `reference/laserfoundry` is not moved. Its crates:
`laserfoundry-device` (`Link`, `SimulatedGrbl`, `restart_line`) and `laserfoundry-gcode`
(`resume_from`, `frame`), used as libraries by `crates/grbl`, and the link logic of its app
(`crates/ui-egui/src/lib.rs`), ported in `crates/grbl/src/lib.rs`. Ours: `crates/grbl` (the
machine) and `crates/engine/src/ports/laserfoundry/` (the engine's port for `--mode grbl`),
`docs/archive/plans/agent-engine.md`, Step 3d (G1 to G16, "ours").

LaserFoundry is an owner exception to "no one-person projects" (2026-11-15): where it falls short,
the fix is proposed upstream (Step 3d), not put in our code. The owner's rule that a laser never
starts unattended is the hub's (the queue never starts a laser job); the port starts nothing but the
job it is given, and after a reconnect nothing.

## 1. LaserFoundry's own tests, run

`cargo test -p laserfoundry-device -p laserfoundry-gcode` at `fd87dc3`, in the worktree:
**79 and 49 tests pass** (the `Link`'s, the stream's character counting, `resume_from`, `restart_line`,
`frame`). The app's own `cargo test -p laserfoundry-ui-egui`: **308 pass**, among them its three
reconnect tests (`tests.rs:2634-2746`) and its laser-mode and not-ready refusals (`tests.rs:1821`,
`:1838`), which `crates/grbl/src/tests.rs` repeats on the port's side (section 3).

## 2. The bytes: the reference's `Link` against our port

`crates/engine/src/ports/laserfoundry/proof.rs`. LaserFoundry's own `SimulatedGrbl` (2 ms a line)
behind a recorder that keeps every byte written to it, driven twice per scenario, each time from a
fresh connection:

- **the reference:** `Link::connect`, then the calls LaserFoundry's app makes in the order it makes
  them (the file and line of each call is a comment at its step in `proof.rs`: `lib.rs:1194` connect,
  `:1418-1422` the settings after the first status, `control_box.rs:157-166` and `panels.rs:888-949`
  the controls, `panels.rs:804-810` and `crates/gcode/src/lib.rs:96` the frame, `panels.rs:836-846`
  pause, resume and stop, `lib.rs:1275-1283` and `:1308-1345` start again from a line); not a
  script of ours;
- **ours:** the same scenario as orders to the engine's port (`Order::Command`, `Order::Job`,
  `Order::Control`), on the same simulator and recorder.

The bytes are cut into tokens: each line, and each real-time byte (`!` `<21>`, `~` `<7e>`, reset
`<18>`). The `?` status polls (LaserFoundry's own 200 ms poll, the `Link`'s) are counted apart and are
not compared: how many there are depends on how long a scenario takes (in these runs 1 each, 7
in the pause and resume run, on both sides). Where a pause or a stop falls among a job's lines
depends on timing: both sides give the order 400 ms after the job starts (the app calls `Link::stop` and
`pause` as the button is pressed, `panels.rs:836-846`, `control_box.rs:141-149`; the port acts on an
order the moment it arrives, the 100 ms tick only reads the snapshot, G7). In six runs each, stop
fell after 114 to 139 lines on the reference and 131 to 141 on ours, pause after 139 to 142 lines on
the reference and 140 to 142 on ours: the reference varies as much from run to run as the two sides
differ; there the commands and the real-time bytes are compared in order, and the lines of
both must be the program's, in order, each once.

| Scenario | The reference (LaserFoundry's `Link`, the app's calls) | Ours (the port) | Result |
|---|---|---|---|
| Connect, wait for the first status | `$I` then `$$`, once each, after the first `?` | the same | **identical** |
| Home | `$I`, `$$`, `$H` | the same | **identical** |
| Unlock | `$I`, `$$`, `$X` | the same | **identical** |
| Jog | `$I`, `$$`, `$J=G91 G21 X10 Y-2.5 F3000` | the same | **identical** |
| Set origin, go to origin (after a jog of 30, 40 and one of 5, 5) | `$I`, `$$`, `$J=G91 G21 X30 Y40 F3000`, `G92 X0 Y0`, `$J=G91 G21 X5 Y5 F1000`, `G90 G0 X0 Y0` | the same | **identical** |
| Read settings | `$I`, `$$`, `$$` | the same | **identical** |
| A typed line | `$I`, `$$`, `G0 X20 Y20` | the same | **identical** |
| Every control in a row (home, unlock, jog, set origin, go to origin, settings, `M5`) | 9 tokens | the same 9 | **identical** |
| A job of 150 lines to the end | `$I`, `$$` and the 150 lines | the same 152 tokens | **identical** |
| A frame of `[10, 10, 30, 20]` (through `GrblMachine::frame`, G4; the reference side uses G12's machine, 200 x 200, not a document's) | `gcode::frame` run by `start_job`: 12 tokens (`$I`, `$$`, `G21`, `G90`, `M4 S0`, `G0 X10 Y10`, `G1 X30 F3000`, `G1 Y20`, ...) | the same 12 | **identical** |
| Pause and resume of a job of 400 lines | `$I`, `$$`, `!`, `~`; lines the program's, in order | `$I`, `$$`, `!`, `~`; lines the program's, in order | **identical** in the commands and real-time bytes; where `!` and `~` fall among the 400 lines differs, as above (G7) |
| Stop of a job of 400 lines | `$I`, `$$`, reset; lines the program's, in order | the same | **identical** in the commands and the reset; the lines before the reset differ with the two looks (G7) |
| Stop, then start again from the line it says | the second job is `resume_from(program, line)`, line for line, after one reset | ours started again at the reference's line: the same lines after one reset | **identical** (the second job); the line the port tells the hub (G5) is `restart_line` over the acknowledged lines, in the design's numbering, and lies between 2 and 400 |

## 3. Behaviour of the link logic, tested against the app's own cases

Not bytes: what the host does with the connection and the refusals. Each row names the case in
LaserFoundry's app, and the test of the port's side (`crates/grbl/src/tests.rs`, unless noted).

| Item | LaserFoundry (v0.6.0) | Ours | Result |
|---|---|---|---|
| Lost connection, serial target | `reconnect_tick` and `poll_link` (`lib.rs:1364-1429`): `RECONNECT_ATTEMPTS` 8 (:84), `RECONNECT_MAX_WAIT` 30 s (:86), `reconnect_wait` 2 s (:476), doubling (:1388); test `a_lost_connection_is_retried_until_it_comes_back_and_nothing_resumes` (`tests.rs:2705-2715`): 3 opens, nothing runs after | `a_lost_connection_is_reopened_and_nothing_resumes`: 3 opens, no job resumes; `the_reconnect_waits_double_up_to_thirty_seconds`: 4, 8, 16, 30, 30, 30, 30 s | **same cases, same numbers; not compared by a run of both** (the reference's test is its app's window, not a script a host can run) |
| Giving up | test `it_gives_up_after_the_attempt_limit...` (:2718): 8 opens, "Could not reconnect" | `it_gives_up_after_eight_attempts`: 8 opens, "Could not reconnect: gone" | **same case; not compared by a run of both** |
| No retry for a simulator, or with the switch off | test `the_switch_turns_it_off_and_the_simulator_never_retries` (:2732) | `a_lost_connection_is_not_retried_when_it_is_switched_off`, `a_command_owed_an_answer_fails_when_the_connection_is_lost` | **same case; not compared by a run of both** |
| `$I` then `$$`, once each, after the first status | `lib.rs:1418-1424`, `settings_asked` | `only_laserfoundrys_bytes_at_connect` (section 2) | **identical** |
| Job refused: no link, not idle, laser mode off | `start_job_with` (`lib.rs:1238-1268`), `not_ready` (:1539-1551), `settings_ui.rs:26-31`; tests `tests.rs:1821`, `:1838` | `a_job_is_refused_when_laser_mode_is_off`, `a_job_is_refused_while_the_machine_is_not_idle` (all eight states, the app's texts) | **same refusals and texts; not compared by a run of both** |
| Job end, restart line | `record_job_position` (`lib.rs:1308-1345`), `Restart::place_in_design` (:107-117), `restart_line` | `an_interrupted_job_starts_again` (restart line in the file's numbering, the second job runs to the end) | ported formula (`file_line` re-implements `place_in_design`); **not compared** with the app's, only bounded (2 to 400) |
| `from_line` 1 | `start_from_line` rebuilds with `resume_from` for any line | `a_start_from_line_one_is_rebuilt_like_any_other` | **same call; not compared by a run of both** (the bytes after it are those of `resume_from`) |

## 4. Where ours is not LaserFoundry's (the list, `docs/archive/plans/agent-engine.md` G1 to G16)

Command answers come from the snapshot's console and the port reads it every 20 ms (G7, G9, G15);
FDM actions, a Z jog and a frame without a file fail loudly (G2, G4); the report's texts and
states (G8, G11); the frame's machine (G12); a job's end on a lost connection (G13); `--baud`
default and `--port` required (G14); a machine that did not open is not opened again (G10). None
of these changes a byte on the wire (section 2).

## Not covered

- **A real laser, a real serial port:** `laserfoundry_device::serial::SerialTransport` is LaserFoundry's
  and is used as it is; nothing here ran against a controller. The simulator is LaserFoundry's own
  (`sim.rs`): it has no greeting, no limit switches, no real timing, and answers one line at a time.
- **The app's confirmation of a start from the head's position or a user origin** (`lib.rs:1267`): a
  program file carries no origin setting; the person frames and starts at the machine.
- **`link.protocol() != link_protocol()`** (`lib.rs:1244`): the protocol is fixed.
- **Marlin and Smoothieware lasers**: closed. `--mode grbl` on a Smoothieware machine is untried.
- **The frame through the agent** (G4): the link's fetch of the file (`link.rs` tests) and the port's
  frame of a local file (`ports::laserfoundry::tests`) are tested, and `GrblMachine::frame` is proven
  byte for byte (section 2); the whole chain hub to machine is not run in one test.
- **The reconnect against a real unplugged cable**: `Unplug` in the tests fails reads and writes; a
  real cable's errors are what `SerialTransport` returns.
- **Console limit** (G15): more than 500 console lines between two looks of the port.
- **The `?` polls**: counted, not compared (section 2).
- **Hold-to-jog** (`Link::jog_cancel`, the jog-cancel byte `0x85`, `crates/device/src/link.rs:219-223`; the app's
  "Hold to jog" box, off by default, `ui-egui/src/panels.rs:875-878`): not ported; the port's jog is the step jog.
- **A reconnect on a machine that was homing or running a job** when the cable came out: the machine
  keeps its state; what the port reports after it is the first status.
- **The hub's laser checks** (`files.rs` `laser_check`, `fit`, PPE): ours, in the UI (B3), not on the wire.
- **How LaserFoundry itself follows GRBL** (which sender it was written against, and whether its
  streamer is proven on a controller): not audited here; the upstream contributions list in Step 3d
  names what falls short.
