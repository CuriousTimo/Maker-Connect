# The hub: a lean core plus modules (HS0) — design for approval

Owner, 2026-11-15: "opsplitsen is een must", "zo kan de hub ook lean en mean op zichzelf draaien",
"juist aparte modules. en later meer modules voor andere zaken". Designed by the architect.

## HS0a: split by moving code only (no behaviour change)

- One library crate (`src/lib.rs`) plus a thin binary (`src/main.rs`: usage, start, signals). Gives
  tests and the in-hub engine a real entry point. The binary name stays `maker-connect-hub`.
- `main.rs` (~160 KB) splits into `web/router.rs`, `web/shell.rs`, `web/machines.rs`, `link.rs`
  (register, telemetry, pair, agent_job), `commands.rs` (refused_now … the command handler),
  `devices/legacy_gcode.rs` (the hub's own G-code: jog_command, grbl_action, action(), probe_gcode,
  prusa_mk3_machine — kept together so it can go in one piece at HS8/HS10) and `devices/` for the
  device modules (prusa_connect, printer_files, simplyprint, sd_card, virtual_machines, virtual_grbl).
- `store.rs` becomes `store/` (mod, migrate, files, jobs, agents, history, commands, machines); child
  modules see the parent's private fields, so nothing's visibility changes.
- Before moving: `access::route_sources()` must read subfolders (today it reads only `src/*.rs`, so
  moved routes would silently drop out of the route tests); `include_str!` paths fixed with each move.
- Safety after every step: fmt, clippy `-D warnings`, the hub's tests by name and count, a checked-in
  route snapshot (`crates/hub/tests/routes.txt`: paths, `RULES`, and the status per method×path with each role)
  byte-identical, the route-walking auth test passing. One reviewable step each (no commit unless the owner asks); no other agent on the hub.
- `route_sources()` changes only what the tests read, never `RULES` or the guard (`access.rs` ~858);
  the route snapshot (step 0) lands first and proves the walked route set is identical.

## HS0b: the module boundary

A module is one registration value:

```rust
pub struct Module {
    pub name: &'static str,
    pub needs: &'static [&'static str],      // other modules it uses
    pub migrate: fn(&rusqlite::Connection) -> rusqlite::Result<()>, // its own tables
    pub routes: fn() -> axum::Router<Shared>, // its routes and pages
    pub rules: &'static [Rule],               // its access rules (same form as RULES)
    pub nav: &'static [NavLink],              // its links in the top bar
    pub start: Option<fn(Shared)>,            // its background task
    pub hooks: Hooks,                         // on_telemetry, machine_actions
}
```

A module may use from the core only: `Store::db()` for its own tables, typed module state
(`store.state::<T>()`, replacing module fields on `Store`), the core reads (machines, jobs, files,
spaces), `store.changed()`/`subscribe()` (live updates), `access::Access` and `Need`/`Role`, the shell
(`header`, `STYLE`, `escape`, `failed`), `link::agent_job`, and a small `settings` API over `meta`.
Modules add store methods through their own traits, not `impl Store`, so they can become crates. The
core never imports a module; a module imports another only if it is in `needs` (a boundary test reads
the sources). Two places that break this today become hooks (`machine_actions`, `on_telemetry`).
Adding a module later: a folder or crate, one `Module` value, one line in the list.

Stages: 1) inside the hub crate, `src/core/` + `src/modules/<name>/`, each module behind a Cargo
feature of its name; 2) one module at a time becomes `crates/hub-<name>` on `maker-connect-hub-core`
(first stock, spoolman, farm); HS9 (engine) and HS13 (LaserFoundry web) arrive directly as modules.

## The first modules

Core (enough for hub + agents): store, the agent link, files, queue, commands with their safety
limits, safety, accounts/auth/access, access_page, permissions_page, router/shell, now_page, the
machine list and page, updates, downloads, about, codes, profiles (FDM), the label crate (png only).

Modules: `prusa-connect` (prusa_connect, printer_files, sd_card), `klipper` (simplyprint), `virtual`
(virtual_machines, virtual_grbl; the only one needing grbl/LaserFoundry), `stock`, `spoolman` (needs
stock), `farm` (needs queue), `labels`, `samples`, `apps`, `projects`, `space` (core or module: owner);
later `engine`, `laserfoundry-web`.

Behaviour must stay exactly today's with all modules on: `RULES` per module keep today's order
within overlapping paths (the matcher is first-match, `access.rs` ~858), proven by the per-method×path
snapshot; module migrations run in today's order with the same schema bookkeeping; the module state map comes in its own step;
the hooks keep the call order and the lock of the two places they replace (named with file:line in
their step); the top bar's order and per-role visibility stay, the snapshot includes it per role.

Lean: `default = []` is the core. CI builds `--no-default-features` (with a `cargo tree` check: no
serialport, nusb, laserfoundry), each module alone, and `--all-features` (= today's route snapshot and
tests). The release binary size on aarch64 is recorded.

## Decided (proxy, 2026-11-15; the owner: "stel de rest van de vragen aan de agent", nobody uses it yet)

- A1: release and Pi builds ship all modules (`--all-features`, = today) until changed; making a
  release stays the owner's act.
- A2: farm, stock, labels, samples, apps, projects are modules; `space` is core (permissions are set
  only in Space).
- A3: no run-time switch for SpoolmanDB's fetch (a new feature); the fetch belongs to the
  `spoolman` module and runs when that module is on.
- A4: crates one module at a time (stage 2), no big-bang split.
- A5: a module that is off is absent: its routes 404, nothing of its own is created (no data
  exists yet, so nothing else to arrange).

## Steps

0 route snapshot + test-name list · 1 `route_sources` reads subfolders · 2 `lib.rs` · 3–8 main.rs
apart · 9 `devices/` · 10 `store/` one area per commit · 11 pages folder · 12 doc paths ·
13 `Module` type with today's registrations unchanged · 14 `RULES` per module · 15 menu from `nav` ·
16 module state + hooks · 17 features, boundary test, CI lean/each/all · 18 stage 2 crates.
