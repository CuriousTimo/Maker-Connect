# Brief HS6: an emergency-stop button per kind on the machine page

Row: `docs/archive/plans/hub-side.md` HS6. The button, who may press it and its texts are **ours, in our UI**; what
each kind sends is its reference's own action, traced below. Nothing new goes on any wire.

## Who may press it

Whoever may *use* the machine (`Need::Machine(Level::Use)`, `crates/hub/src/access.rs`, the same
level as the job route's Stop), with a session or a token (`Access` covers both). Stopping never waits
for PPE: `crates/hub/src/safety.rs:1-6` ("pausing, stopping, cooling down and switching off never do");
the handler calls no `check_ppe`. Audit label: "pressed the emergency stop of".

## Per kind

| Kind | Button | What it does | Reference |
|---|---|---|---|
| Agent machine on the Marlin/OctoPrint port (link agent, FDM, plain Marlin profile, firmware Marlin or none) | yes | queues the typed `Action::Gcode { "M112" }`, the same path as the terminal; the port gives it to OctoPrint's comm, whose `_gcode_M112_queuing` (`OctoPrint-1.11.8/src/octoprint/util/comm.py:5524`) runs `_trigger_emergency_stop` (`comm.py:5487-5522`: `M112` sent without and with checksum, comm closes). OctoPrint 1.11.8's own UI has no separate emergency-stop button (no match in `templates/` or `static/js/app` for one); its emergency stop is `M112` typed through comm (`docs/events/index.rst:716`) | OctoPrint 1.11.8 |
| Laser (GRBL, LaserFoundry) | yes | the same route as the laser's Stop button: `JobControl::Stop` to the port, `Link::stop` (`laserfoundry/crates/device/src/link.rs:265`, `stop_machine` :440-446: GRBL gets the soft reset byte `0x18`). LaserFoundry's app has exactly one stop: `Stop` button `crates/ui-egui/src/panels.rs:774-775` (also `control_box.rs:148`, `machine_setup.rs:472`) ("Soft reset: stops motion and turns the laser off"). Feed hold (`!`, `link.rs:541`) is its Pause, not a stop. So there is **no stop distinct from its Stop**; we use that one | LaserFoundry v0.5.0 (rev 61febdb) |
| DYMO PnP, LabelWriter 450 | none | no motion or heat to stop; their references have no such control (L5, W6) | labelle, DYMO driver |
| Prusa on Buddy (Connect / PrusaLink, MINI, MK3 family, MK4) | none (see open question) | Buddy 6.3.0 has `M112` only as an ordinary queued G-code (`lib/Marlin/Marlin/src/gcode/gcode.cpp:520`, `control/M108_M112_M410.cpp` `kill(...)`): `EMERGENCY_PARSER` is commented out in every `include/marlin/Configuration_*_adv.h` (e.g. `Configuration_MINI_adv.h:1269`), so it is not an immediate stop, it waits in the G-code queue. Prusa-Link and the Connect SDK have no emergency-stop command (no match for "emergency" or "M112"). `HAS_EMERGENCY_STOP` (`src/state/printer_state.cpp:544`) is a door sensor of some models, not a command | Buddy 6.3.0 |
| Hub's own machines (built-in Connect, SimplyPrint, Klipper, virtual) | none | they have no port here yet (HS8-HS11) | |

## Code

`commands::emergency_stop_of(machine)` decides the kind; `POST /machines/{id}/emergency-stop`
(`commands::emergency_stop`) answers 409 "this machine has no emergency stop" for the rest;
`store::on_marlin_port` is the Marlin predicate K18 already uses. The page shows the button (job card,
with a one-line hint, ours) only when `emergency_stop_of` is `Some`; `machine.js` asks once
("Emergency stop?") before sending.

## Tests

`commands::tests::hs6_the_emergency_stop_is_each_kinds_own`: a Marlin machine is given exactly
`Gcode "M112"` and nothing else; a laser gets `JobControl::Stop`; a label printer, MINI and MK4S are
refused (409), sent nothing, and the page shows no button; the pressing user has 'use' only (the
terminal is refused 403 for the same user, so the button is allowed by 'use').

## Ours (to approve)

The button and its confirmation, "Emergency stop" label, the two hint texts, the 409 text, the audit
label, 'use' as the level.

## Open question for the owner

Buddy: M112 exists but is queued (no emergency parser) and would reach the printer only through the
hub's typed G-code path (SEND_GCODE, 512 bytes). Offer it anyway as "Send M112"? Not done.
