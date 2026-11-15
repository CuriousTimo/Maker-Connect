# Brief: HS0a — split the hub by moving code only

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust; git repo, never commit unless told; no
hardware). PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`;
`export CARGO_TARGET_DIR=target-split`. Read `docs/hub-modules.md` (the approved design, HS0a and
"Decided (proxy)") and follow its steps 0–12 exactly.

**No other agent works on the hub now** (HS7a and the Spoolman audit are finished and committed,
`8d6828a`). Work in the main tree. Do not move `spoolman.rs` or `spoolman/` (it moves in HS0b); they
may be touched only for `use` paths. No commit unless the owner asks; no push, no deploy.

**Only moving code.** No renamed functions, no changed logic. Allowed edits: `mod` lines, `use`
paths, `pub(super)`/`pub(crate)`/`pub` visibility, `include_str!`/`include_bytes!` paths, and the
re-exports in `lib.rs` that keep every `crate::x` working. The only other edits: the snapshot test
and `routes.txt` (step 0), the `route_sources()` change (step 1), creating `lib.rs` (with `Shared`,
today private at `main.rs` ~65, made `pub(crate)`/`pub`) and the binary's `use maker_connect_hub::…`,
and `[lib]` in `Cargo.toml` keeping the `[[bin]]` name `maker-connect-hub` (step 2). Anything else:
stop and report. Re-read line ranges as you go (the design
cites today's lines; HS7a and the 404 changed `main.rs`).

Steps (one reviewable step each, report at the end):
0. Record `cargo test -p maker-connect-hub -- --list` into a file (names and count; it is 119 now);
   after each step diff the sorted list by the last path segment (module prefixes change as tests
   move). Add the snapshot as a unit test (in `access.rs` tests, `include_str!("../tests/routes.txt")`;
   the crate is binary-only until step 2): the sorted route paths as `route_sources` parses them,
   `RULES`, the status each method × path returns when actually calling the router with fixed
   made-up path parameters for every `Role` and for no session, and a hash of the served `/shell.js`
   and every embedded `.css`/`.js`. Nothing in it may depend on time, a database path or random ids:
   run it twice and diff. The test fails on a missing or different snapshot; a regenerate switch is
   used only in step 0.
1. `access::route_sources()` reads subfolders under `src/` (test side only; never `RULES` or the
   guard). The snapshot must not change.
2. `src/lib.rs` with every `mod` line, `Shared`, and re-exports; `main.rs` keeps the binary part.
3–8. Move out of `main.rs`: `web/router.rs` (app, not_found with the 404 page, failed),
   `web/shell.rs` (header, logo, escape, STYLE, PAGE, shell.js path), `link.rs` (register, telemetry,
   pair, agent_job, peer_address), `commands.rs` (refused_now … the command handler, with tests),
   `devices/legacy_gcode.rs` (grbl_action, jog_command, action, probe_gcode, prusa_mk3_machine, with
   tests), `web/machines.rs` (the machine list and admin routes, profile routes). `main.rs` ends as
   the binary only.
9. The device modules into `devices/` (prusa_connect, printer_files, simplyprint, sd_card,
   virtual_machines, virtual_grbl) with re-exports.
10. `store.rs` → `store/mod.rs` unchanged, then one area per step: migrate, files, jobs, agents,
    history, commands, machines.
11. The `*_page.rs` files into `pages/`.
12. Fix the paths cited in docs and scripts: list them with `grep -rn "crates/hub/src" scripts docs`
    (e.g. `scripts/klipper-audit-check.py:4`).

After every step: `cargo fmt --check -p maker-connect-hub`, `cargo clippy -p maker-connect-hub
--all-targets -- -D warnings` (if clippy is installed; say if not), `cargo test -p maker-connect-hub`
(same test names and count), the route snapshot byte-identical, the route-walking auth test passing. At the end also `cargo build --release -p maker-connect-hub`
(the binary keeps its name).

Report briefly: the new layout (file sizes), the test count before/after, the snapshot result, and
anything that would not move without a logic change (stop and report instead of changing it).
