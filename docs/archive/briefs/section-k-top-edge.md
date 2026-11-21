# Brief: section K — the Marlin port's top edge, and B3–B7

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust workspace; git repo, never commit; never
deploy or touch hardware). PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`.
Read `CLAUDE.md`. The top edge is **ours**: every piece follows an approved decision in
`docs/plans/agent-engine.md` (O1–O8, S1–S5, M1–M5, the M3 table, B1–B9, and all "Section … decided"
paragraphs); anything not covered there is listed as a new item "K1, K2…, to approve (proxy)" and
kept to the minimum. Nothing of ours reaches the printer: every byte comes from the ported
OctoPrint code (`crates/engine/src/ports/octoprint/`), reference `reference/OctoPrint-1.11.8`.

Read first: `crates/engine/src/lib.rs` (Port, Order, Event, Report), `crates/agent/src/link.rs`
and `ports.rs`, `crates/protocol/src/lib.rs` (Action, Telemetry, MachineState, CommandDone,
JobEnd, Prompt), the octoprint port's `standard.rs`, `comm.rs` (MachineComPrintCallback),
`events.rs` (STARTUP/SHUTDOWN, sink), `action_command_prompt.rs`, `api_printer.rs`, `profile.rs`,
and the hub's command path (`crates/hub/src/main.rs` command route ~1374–1560, telemetry
~432–528, `agent_job` ~562–660; `crates/hub/src/store.rs` command queue ~1660–1940,
`fits_a_print` ~513–532) and `crates/hub/src/machine.js` / `machine_page.rs` (the Home button).

## 1. The port: `crates/engine/src/ports/octoprint/port.rs` (ours)

A `Port` for `--mode marlin`. On `run`: fire `Events::STARTUP`; register the prompt plugin
(`ActionCommandPromptPlugin::register`); create the `Printer` with the default profile; install
the event sink; `connect(port, baudrate)` from the agent's options (`--port`, `--baud`; `None`
means OctoPrint's own auto-detection, as `connect` does). On `Order::Close`: `disconnect`, then
fire `Events::SHUTDOWN`.

- **M1:** each `Action` to the OctoPrint call listed in M1, through `api_printer.rs`'s checks first
  (the refusals OctoPrint's API makes: S5, B3); a refusal is a failed `CommandDone` with
  OctoPrint's text. `Home { axes }` → `home(axes)` (B5). Fan/MotorsOff as M1 says. `Gcode { text }`
  → `commands([text])`, one string, not split (OctoPrint's terminal sends it whole,
  `terminal.js:419-452`); the terminal page's upper-casing (`terminal.js:425-442`) is the web
  page's JS, not ported: typed G-code goes as typed (K1). `Action::Gcode` has no parameters, so
  `command % parameters` is never called from here. `M112`/`M108`/`M410` are forced ahead by
  comm's own `_emergency_commands` (B7).
- **M2:** Load/Unload/ProbeBed and the laser actions: failed `CommandDone`, "OctoPrint has no such
  control".
- **M3:** the state table, exactly; `getStateString` as `problem` where not plain; the prompt
  plugin's open prompt → Attention with its text and buttons as `prompt`.
- **M4:** `CommandDone` once the command's lines are sent and `ok`-ed, the lines received between
  as its output (from comm's callbacks/terminal log, not a new reader).
- **Telemetry:** temperatures, progress, position, SD state from the `MachineComPrintCallback`
  calls comm already makes (`on_comm_temperature_update`, `on_comm_progress`,
  `on_comm_position_update`, …): implement them on the top edge's callback (the ones `standard.rs`
  left out as "UI only" are where the top edge listens). No polling of our own.
- **Jobs:** `Order::Job { order, file }` → `select_file(file, sd=false)` then `start_print`, as
  OctoPrint's job API starts a print, without a position (`job.py:38,45`). A `JobOrder` with
  `from_line: Some` gets a failed `JobEnd` "OctoPrint has no such control", nothing sent (K3).
  `JobEnd` from the print-done / failed / cancelled callbacks and events. `Control` →
  `pause_print` / `resume_print` / `cancel_print`, after the refusals of `server/api/job.py:32-80`
  (409 for start while a job is active, restart when not paused, pause/cancel when not printing
  or paused), ported into `api_printer.rs` with `job.py`'s exact texts, plus any 409 of the files
  API select (`server/api/files.py`) (K2, extends the S5 table).
- **Answer** → the prompt plugin's answer path.
- **Gone/Back** (K4, to approve): name exactly which comm state change gives `Gone` and which
  gives `Back`; keep it minimal, derived only from comm's own states.

## 2. The agent (`crates/agent`)

- `ports.rs`: `open` returns this port for `--mode marlin` (only add lines; the PnP is there).
- **B4:** `--mode` required: no default (`link.rs` ~98) and no default kind (~110); the usage text
  says so; values `prusa`, `marlin`, `grbl`, `dymo-pnp` (and `labelwriter-450` reserved, no port).
- **B6:** the link fills `Telemetry.job_id` from the engine's `Report.job`.
- **No invented baud:** `--baud` and `--port` are `Option` with no default (`link.rs` ~34, ~133:
  remove `unwrap_or(115_200)`); absent means `None` into `connect`, so OctoPrint's own settings
  and auto-detection decide (`standard.py` `connect`). Test it.

## 3. The protocol and the hub (`crates/protocol`, `crates/hub`)

- **B5:** `Action::Home` becomes `Home { axes: Vec<Axis> }` (axes as OctoPrint's API takes them,
  `server/api/printer.py:297`). For agent machines in marlin mode the machine page shows
  OctoPrint's two Home buttons, XY and Z (`control.jinja2:71,85`); other machines keep one button,
  sending `Home { axes: [x, y, z] }` (K5: what that means for non-agent machines stays the hub's
  current behaviour until the hub side). Update the hub's `agent_actions` and its test.
- **B3:** for agent machines the hub no longer refuses or holds commands (the "not while it prints"
  409, `fits_a_print`, the Attention hold): it queues them; the port refuses as OctoPrint does.
  Hub-side machines (MINI, Klipper, virtual) keep their current filters until the hub side.
  The page may still grey buttons (UI). Keep permissions and PPE checks (they are about people).
  B3 applies to every agent machine (modes without a port get S2's "no port yet" answer); the agent
  then takes commands with `next_command`, not `next_command_for_a_print` (`store.rs` ~1725); the
  hub's 120 s `COMMAND_SECONDS` give-up stays (hub queue, not the wire).
- **B7:** nothing extra: with B3, a typed `M112` reaches the port at once. No new action, no new
  button.

## 4. Tests

- Port tests with a recording fake serial (as section I's tests): a Home XY sends exactly what
  `home(["x","y"])` sends (`G91`, `G28 X0 Y0`, `G90`); a job runs `select_file`+`start_print`; a
  refusal while printing gives OctoPrint's text; the M3 mapping per state; Attention from a
  prompt; M4 done after `ok`. Mark them ours.
- Hub tests for B3 and B5; agent tests for B4 and B6.
- No test touches hardware. `cargo test --workspace` passes (count first), no new warnings.

Not in this step (the next one): running OctoPrint's ported `virtual_printer` through the port
(print, pause, resume, cancel) and comparing with real OctoPrint. Do not call the OctoPrint port
done in the report.

## Report (brief)

Files; each M/B item and how it is done; every new K-item; test counts.
