# Brief HS4: the other agent ports fail pending commands on close; the hub's gate and 120 s go for them too

Rows: `docs/archive/plans/hub-side.md` HS4; `docs/archive/plans/agent-engine.md` K11, K18 (d). All of it is **ours** (engine to
hub, nothing new on the wire to any device); no reference is involved.

## What

1. Per port: when its connection closes (`Order::Close`, or its orders end), every command still
   pending gets `Done { ok: false, reason: "the connection closed" }` (ours, `engine::CONNECTION_CLOSED`;
   the Marlin port keeps K18 (a)'s "the printer connection closed").
2. The hub's K18 (b) rule (every Queued command at once, no 120 s gate, the agent-gone rule) is
   extended from "Marlin agent machines" to "agent machines whose port fails pending on close":
   Marlin/OctoPrint (as before), label printers (DYMO PnP, LabelWriter 450) and lasers.
   Buddy, Klipper and the MK3 family keep the gate and the 120 s. Store function renamed
   `is_marlin_agent` to `fails_pending_on_close`.

## Per port

| Port | What is pending | Change | Proven by |
|---|---|---|---|
| DYMO PnP (`labelle/port.rs`) | Nothing: a command is answered at once ("labelle has no such control", L4). Only a command queued behind the Close in the channel is unanswered | on `Close`, the commands still in the channel fail | `a_command_after_the_close_fails` (fake USB scene) |
| LabelWriter 450 (`dymo_cups/port.rs`) | A command that arrives during a print waits in the port's queue until the print ends; one after a Close stayed unread | `look_for_orders` stops reading at a Close; on `Close` the queue and the channel's commands fail | `commands_waiting_at_close_fail` (bench host, printer absent) |
| Laser (`laserfoundry/port.rs`) | Commands submitted to the machine and not yet answered (`tickets`), and commands in the channel. A lost connection already fails them (`crates/grbl`, G9) | on `Close` and on the orders ending, every ticket's command and every command in the channel fails | `commands_pending_at_close_fail` (simulated GRBL with latency; nothing is sent to a real device) |

A command that the port had already answered with its own failure keeps that answer (the 450's
"no such control" for a command that came before the Close).

## Reason texts (ours)

- "the connection closed" (port fails; owner, 2026-11-17: "akkoord met alle voorstellen", the laser
  is not a printer)
- "the agent was gone" (hub, K18 (b), unchanged)

## Hub tests

`k18_the_other_agent_machines_keep_their_gate` (Prusa profiles, U30: unchanged behaviour),
`hs4_label_and_laser_agent_machines_have_no_gate` (both at once, no 120 s, agent-gone fails Sent not
Queued).

## Not covered

The USB scale port (no commands). A job in flight at a laser's Close (a job, not a command; unchanged).
