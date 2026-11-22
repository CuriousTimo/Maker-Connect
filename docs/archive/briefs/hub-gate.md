# Brief: drop the hub's one-command-at-a-time gate for agent machines (B3/B7 completion)

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust; git repo, never commit; no hardware).
PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`;
`export CARGO_TARGET_DIR=target-gate`. Two other agents work in parallel (the LabelWriter 450 in
`crates/engine/src/ports/dymo_cups/`, the laser in `ports/laserfoundry/` and `crates/grbl`): don't
touch those; in shared files only add lines, re-read before writing.

Read `CLAUDE.md` and the end of `docs/archive/plans/agent-engine.md`: "The four open points, decided" (1a), B3,
B7, K11. Implement exactly that, nothing else:

1. **Hub.** Leave `next_command_of_any_kind` (`store.rs` ~1807) unchanged: Connect, SimplyPrint,
   the virtual machines and the simulated GRBL keep their gate and 120 s. Add a new store function
   (e.g. `next_commands_for_agent`) that returns all Queued commands in the existing order
   (non-background first), and fill the reply's existing `commands` Vec from it at `main.rs` ~483,
   **only for agent machines whose port is the Marlin/OctoPrint port** (agent `link == "agent"`, as
   `main.rs` ~1401, and the plain-Marlin identity the hub already uses for K5, `plain_marlin`). Other
   agent ports (DYMO PnP, LabelWriter 450, laser) keep the gate and the 120 s: their ports do not
   yet fail pending commands on close; list that as an open item for them in K18. For the Marlin
   agent machines, no 120 s give-up. A command is given up only on the port's failed `CommandDone`,
   or by the agent-gone rule: the hub's existing "online" notion (`ONLINE_SECONDS` = 10,
   `store.rs` ~25, ~2347) — when such a machine has not been seen for longer than that, checked on
   the next poll/report, its Sent/Accepted commands (not Queued) fail with "the agent was gone". This
   rule and its text are ours (no new timeout); write them into K18 for approval. If the agent comes
   back and sends a Done for a command already failed: decide one order explicitly in a test and
   write it down (`command_went`, `store.rs` ~1866, overwrites today).
2. **The Marlin port** (`port.rs`): when comm closes (`Gone`) and on `Order::Close`, send
   `Done{ok:false}` for every command still pending, text "the printer connection closed" (ours).
   One new K row (K18) in Section K of `docs/archive/plans/agent-engine.md` for this and the hub rule above: "ours,
   engine↔hub, nothing on the wire to the device", the two texts, the agent-gone rule, the open item
   for the other ports. Change K11 to: agent Marlin machines are given up on the port's failed Done
   or the agent-gone rule; Buddy/Klipper and the other agent ports keep the 120 s.
3. **Concurrency and M4:** with the gate gone, several commands are pending at once, and the port's
   M4 bookkeeping (`port.rs` ~757: each received line and `ok` credited to the first pending command
   with `sent > acked`) may credit lines/`ok`s to the wrong command (e.g. `M112` forced ahead while
   an `M109` is un-acked). Add a port test (M109 pending, then M112): write down exactly what the
   port then reports. If the existing rule gives a wrong Done, record it as a numbered difference
   for approval; do not fix it with logic of our own.
4. **Tests:** hub: two commands for a Marlin agent machine go out without waiting; a typed `M112`
   goes out while an `M109` is still Sent; Buddy and the other agent ports keep their gate;
   agent-gone fails only Sent/Accepted, not Queued, and only of that machine; the late-Done order.
   Port: pending commands fail on close; the M4 concurrency test above. Run with this brief's
   `CARGO_TARGET_DIR`; report any red test from files outside this brief.
`cargo test --workspace` (your target dir) must pass; no new warnings.

Report briefly: what changed, K18 text, test counts.
