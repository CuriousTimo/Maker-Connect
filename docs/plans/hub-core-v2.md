# Hub core v2: design

Status: **design for review, 2026-11-18** (architect). Builds on
[prusa-connect-study.md](prusa-connect-study.md) (§1a, §3, §4a, §5a, owner rulings R1-R3 in §6) and
[prusa-connect-ui-study.md](prusa-connect-ui-study.md). `pc/` = `reference/prusa-connect` at 2e8c62b, read
only (OCL: nothing copied; every line here is our own writing). `D/` = `pc/docs/src/content/docs/`.
Claims about our code carry `file:line` (paths under `crates/` unless written out); "not checked" where not.

## 1. What v2 is

The hub core is rebuilt around one device contract: every machine (3D printer, laser, label printer,
scale, camera) is a **device** with an identity, a declared set of capabilities and states, a state tree
that changes by patches, and commands that are **durable records** from request to outcome. Everything
that happens goes out as **one kind of event** (stored, resumable over SSE). Who may do what is a set of
**actions within a Space**, filtered in SQL. The store moves whole to **Postgres + TimescaleDB** (R2);
history is an event subscriber, not part of the core. The API is **API-first**: OpenAPI generated from
the Rust code, a diff check in CI, and our own web app uses only that API. For 3D printers the hub serves
the REST surface of Prusa's server (R3); for everything else, our own calls in the same style.
Nobody uses the hub: no compatibility, no data migration; old code is deleted, not wrapped.

| Stays (ours, better) | Goes / is replaced |
|---|---|
| Organisations → Spaces → memberships (`accounts.rs:115-128`, not in their schema) | `Arc<Mutex<Store>>` over one rusqlite connection (`lib.rs:49`, `store/mod.rs:486-487`) |
| Our agent at the machine and its ports, each to the letter of its reference (`engine/src/ports/mod.rs:1-16`) | The fixed `Telemetry` struct (`protocol/src/lib.rs:154`) and `Action` enum (`protocol/src/lib.rs:491`) as the core's vocabulary |
| Makerspace features: Now/wall, queue, farm, materials (Spoolman-compatible), stock, labels, lasers, projects | In-memory command lists with a wrapping `u32` id (`store/mod.rs:494`, `store/commands.rs:33`) |
| Door access apart from machine permissions (`pages/access_page.rs:1-9`) | 30 min temperature history in a `VecDeque` (`store/history.rs:4-28`, `store/mod.rs:531`) |
| Permissions managed only in Space (`pages/permissions_page.rs`, not re-read) | Role/Level checks per handler (`access.rs:1447-1497`) and unfiltered lists (`store/machines.rs:413`) |
| SSE to the browser; camera media over WebSocket as go2rtc does (`cameras.rs:303-322`) | Full machine list re-sent on every change, no event id (`web/machines.rs:52-66`) |
| Laser never starts unattended (`store/mod.rs:33`, `queue.rs:228`) | Hand-kept route table as the only permission source (`access.rs:147`) and hand-written API docs (`docs/api/apps-api.md`, not re-read) |
| One subject per page/tab | SQLite and `rusqlite` (`hub/Cargo.toml:46`) |

## 2. The pillars

### 2.1 Device model and capabilities

**Decision.** A new crate `crates/device` holds the contract, written in Rust by us after the meaning of
their driver contract (`pc/sdk/connectorapi/driver.go`, `capability.go`, `devicestate.go`, read to
understand): identity `{vendor, model, firmware label, serial, unit_uid}`; a **capability** is a verb
with typed arguments (kind, unit, bounds); a device's enabled set = driver baseline ∩ model profile ∩
device probe, with an **epoch** that changes when the set changes; a declared **state set** (states,
which verbs each state accepts, transitions); a **state tree** updated by patches (path + JSON value,
null removes); `do(command) → result` where a refusal by the device is a result with a fault code and
the device's own reason, and a result is never "ok" for something not observed. Firmware versions are
labels, never compared; behaviour follows capabilities.

**Why.** One core for every kind; the UI shows a control because the device declared it and the
current state accepts it, not because the page knows the brand.

**Kinds as devices.**

| Kind | Capability names | States | Where its port is |
|---|---|---|---|
| 3D printer | their core verbs and states, same names and meanings (R3, study §5a item 2) | theirs | hub (Prusa, §2.2), agent (Marlin) |
| Laser | ours, prefix `mc.laser.` (frame, set/go origin, unlock, read settings, start, stop) | ours (idle, framing, loaded, running, alarm, …) | agent, LaserFoundry |
| Label printer | `mc.label.` (print, feed where the reference has it) | ours | agent, labelle / CUPS 450 / 320 |
| Scale | none (state only: weight, stable) | ours | agent, usbscale |
| Camera | `mc.camera.` (snapshot, stream) | online/offline | hub (RTSP, `cameras.rs`), agent (USB, `agent/src/camera.rs`) |

A camera is its own device linked to a machine; whoever may view the machine may view its camera (study
§3). The `MachineKind` enum (`protocol/src/lib.rs:34`) becomes a device kind label for the UI only.

**Rule kept (CLAUDE.md).** The contract is internal. What reaches a machine is still made by a port that
follows its one reference to the letter; a capability verb maps to the reference's own call, and a verb
the reference has no call for is not declared. Our own limits sit in host-side validation and the UI.

**Host-side rules (ours, outside every driver).** Arguments validated against the declared set before a
driver sees them; state guard from the state set; authz (§2.6); the laser rule: `mc.laser.start` is
accepted only from a person's session, never from the queue or a token (today `LASER_NEVER_ALONE`,
`store/mod.rs:33`, checked in `queue.rs:228` and `access.rs:3308,3353`).

**Maps from / replaces.** `engine::Order`/`Event` (`engine/src/lib.rs:25-51`) → contract command/patch/
result; `engine::Port` (`engine/src/lib.rs:83`) gains a contract adapter; `protocol::Action`
(`protocol/src/lib.rs:491-570`), `Telemetry` (`:154`), `CommandDone` (`:594`) go. Machine profiles
(`hub/src/profiles.rs`) become the "model profile" term of the intersection. A conformance test kit in
`crates/device` (fake host, fake device) every port and driver passes.

### 2.2 The Prusa driver, the Buddy websocket protocol

**Decision.** One driver `prusa-buddy`, inbound, hosted in the hub process, with one dialect `buddy-ws`;
Buddy 6.3.0's HTTP dialect is not built (owner, journal:V2-DEVICES:0005; clean break, journal:DESIGN-WS:0005;
[decision](../design/decisions/20261119-buddy-websocket-dialect-only.md)).

| Dialect | Wire | Reference (normative) | Server-side reference | Today |
|---|---|---|---|---|
| `buddy-ws` | 6.4+: `/p/register`, `/p/auth`, `/p/ws` | Buddy 6.4+ | their `internal/connector/xbuddy` (R1) | not built (#2) |

It maps onto the 3D-printer capability set, declared by the dialect's baseline. The hub's own G-code for Prusa
(`devices/legacy_gcode.rs`, HS10) is not carried into the driver; the button rows wait for HS10.

**Replaces.** hub-side.md H1/HS9 ("engine inside the hub"): the hub hosts this driver directly through
the contract; no sync engine bridge is needed for inbound printers.

### 2.3 The agent as host of the contract

**Decision.** The agent hosts one session per device (fits "one agent, many devices", agent-engine.md
§Later). It sends upstream: identity, capability set + epoch, state set, state patches, command results,
job ends; it takes downstream: commands (contract form, with the hub's record id), jobs, answers.
Agent link v2 replaces `/api/agent/v1/*` (`protocol/src/lib.rs:25-29`) and the 2 s report loop
(`agent/src/link.rs:24`) outright: no v1 fallback, no transition period, no old agents served (owner,
journal:DESIGN-HOST:0006). It is **one WebSocket per agent**, modelled on Buddy 6.4.0 and Prusa Connect's
`/p/ws` (upgrade with a subprotocol, both ways on one socket, ping and pong, dial again on a break;
`reference/buddy-6.4.0` `src/connect/connect.cpp:151-189`, `:559-680`; `reference/prusa-connect`
`internal/connector/xbuddy/link.go`). Path (`/api/agent/v2/ws`), subprotocol (`maker-connect-agent.v2`)
and message JSON are ours. Upstream items keep a sequence the hub acknowledges (unacknowledged items kept
on disk across reconnects, which absorbs HS5b); commands, jobs, answers and updates arrive on the same
socket at once. Registration and file and update downloads stay HTTP. Browser live updates stay SSE
(§2.4). Agent tokens: report and fetch only, stored hashed (today the machine token is plain text,
`store/migrate.rs:85`). ([Decision](../design/decisions/20261119-agent-link-one-websocket.md),
journal:DESIGN-WS:0001; the wire: [wire.md](../design/wire.md).)

Ports and their references are untouched; only the edge between engine and link changes. Their
out-of-process wire (`D/plugin-wire-protocol.md`) is **not** built: the hub keeps a `Host` boundary
(sessions arrive in-process or from an agent) so a third kind (wire child process) fits later, once
Prusa publishes the driver method set.

### 2.4 One event envelope, SSE with resume

**Decision.** Every event: `seq` (global `bigint`, the SSE id), `type` (from the registry), `time`,
`space_id`, `source` (device or resource id), `origin` (person, token, agent, queue, device), `data`
(jsonb). Facts (device online/offline, job started/ended, command settled, grant changed, material
moved) are stored in the `event` hypertable and published on an in-process bus. State patches go on the
bus live but are not stored as events (they feed history, §2.7). One SSE endpoint, `/api/v1/events`,
filtered per subscriber by the materialised access table (§2.6) and by a `types` query.
**Resume:** `Last-Event-ID` → the server sends a current snapshot of each visible device's state tree,
then every stored fact after that id; an id older than retention gets a `stream.reset` event and the
client reloads.

**Replaces.** `Store::changes` broadcast of `()` (`store/mod.rs:533`), `machine_stream`
(`web/machines.rs:52-66`), the USB camera SSE (`usbcam.rs:163`; frames go to the camera device). Spoolman's
WebSockets stay (`spoolman.rs:25`) because Spoolman's clients expect them, fed from the same bus.

### 2.5 Durable command records

**Decision.** Every command to a device is a row before it is sent: requested (who, verb, args,
idempotency key) → authorized (by which grant, or refused with a fault code) → sent → settled (ok,
refused, failed, or **outcome unknown**). On hub restart, open records become outcome unknown; nothing is
re-sent. Partial index on unsettled rows. File transfers to printers get the same treatment (bytes,
state, reason), replacing `printer_transfers` (`devices/printer_files.rs:68`).

**Replaces.** `Store::commands` (`store/mod.rs:494`), `queue_command` (`store/commands.rs:19-40`),
HS5a; feeds #11 (job history). Jobs keep their own table; a job's start/pause/stop are command records.

### 2.6 Authorization: actions within a Space

**Decision.**
- **Actions**, registered by name (`device.view`, `device.move`, `device.heat`, `device.gcode`,
  `job.start`, `job.control`, `mc.laser.start`, `space.members.manage`, …). Each capability verb names the
  action it needs.
- Today's `Level` (view/use/operate/manage, `access.rs:69`) and `Role` (`access.rs:36`) stay as **named
  presets** of actions in the Space UI; grants (per person or group, on a device or a device group, with
  expiry) store an action set. Induction (`access.rs:1464-1485`) stays a rule of the preset.
- A **materialised** `device_access(space_id, user_id, device_id, actions bigint)` is rebuilt in the same
  transaction as any grant or membership change, and a per-space `authz_epoch` bumps so SSE filters
  refresh. Lists, counts and SSE join it; no list filters after fetching (today `machines(space)` returns
  all, `store/machines.rs:413`; guests get view on all, `access.rs:1477`).
- Route guard: every route names its action (generated from the OpenAPI operation, replacing `RULES`
  `access.rs:147` and `need()` `:933`); unknown route = refused, as now (`access.rs:933-945`).
- Tokens: hashed (already, `accounts.rs:94`, `:264`), plus scopes, expiry, last use and a generation
  counter per person so one bump ends every token they minted.
- **Tenancy:** `space_id` on every tenant row; **Postgres row-level security** on all tenant tables,
  keyed on `SET LOCAL app.space_id` inside each request's transaction. Cross-space work (instance admin,
  agent token lookup, the history writer) runs as a separate database role with `BYPASSRLS`. RLS is the
  second wall; the SQL filters stay the first. The roles, the login that owns nothing and the
  admin pool's allowlist (checked by a test): the design's database.md, "Roles and pools".
- Door access (doors, badges) is its own module with its own tables and actions; it never grants
  machine actions and machine grants never open doors.

### 2.7 Store: Postgres + TimescaleDB

**Access crate: `sqlx` (Postgres only, rustls).** Why: compile-time checked queries with an offline
cache (`.sqlx/`, so CI and armv7 cross builds need no database), a built-in migrator, `#[sqlx::test]`
gives each test a fresh database, pool built in. `tokio-postgres` is lighter but leaves pooling,
migrations and checked queries to us. The store becomes a cloneable handle (pool + bus + a few
in-memory runtime maps behind their own small locks); `Shared = Arc<Mutex<Store>>` (`lib.rs:49`) goes.
What is in memory on purpose stays in memory: PPE confirmations, "framed since last origin" and "origin
set since start" (`store/mod.rs:497,523-526`), sign-in failure counters.

**Migrations per module.** Each module owns `migrations/` with timestamped files (unique across
modules); the hub assembles one migrator from all enabled modules (whether sqlx's migrator takes a
merged list without workarounds: not checked, spike in step 2). Replaces `store/migrate.rs` and the
`CREATE TABLE IF NOT EXISTS` blocks spread over `accounts.rs:31-160`, `farm.rs:42`, `queue.rs:25`,
`stock.rs:28-132`, `cameras/store.rs:11`, `devices/printer_files.rs:60-68`, and the HS0b `migrate`
field (hub-modules.md).

**Schema sketch** (types: `uuid` v7 ids for rows people see, `bigint` identity for logs, `timestamptz`):

| Module | Tables (plain unless marked) |
|---|---|
| tenancy/accounts | `organisations`, `spaces`, `users`, `identities`, `memberships`, `org_memberships`, `sessions`, `invites`, `api_tokens` (+ scopes, generation), `audit_log` (plain, kept) |
| authz | `action_grants` (subject, device or group, actions, expires_at, granted_by), `device_groups`, `device_group_members`, `device_access` (materialised), `authz_epoch`, `access_requests` |
| devices | `devices` (space_id, kind, name, identity columns, dialect, link, `state jsonb` current tree with a `CHECK` that it is an object, state name, last_seen, current_job_id, retired_at/reason, settings jsonb), `device_credentials` (token hash, generation), `device_registrations` (code, fingerprint, token hash, expires, approved_by), `device_capabilities` (device, epoch, set jsonb, state set jsonb) |
| commands | `commands` (partial index `WHERE settled_at IS NULL`), `transfers` |
| jobs/queue/farm | `files`, `jobs` (partial index on running), `machine_queue`, `projects`, `parts`, `part_files`, `part_ledger` |
| materials/stock | Spoolman tables (`vendor`, `filament`, `spool`, `extra_field`, `setting`, `tag`) and ours (`materials`, `items`, `locations`, `movements`) |
| cameras, labels, door access | own tables |
| events (hypertable) | `event` (time, seq, space_id, type, source, origin, data jsonb); unique keys include time; retention per §5 Q5 |
| history (hypertable) | `sample` (time, device_id, metric smallint, value double precision), `metric` (id, name, unit) registry table; continuous aggregates 1 min and 1 h; compression `segmentby device_id, metric` after 7 d; `gap` (device, from, to, why) for hub restarts and agents offline |

**Wide or narrow history: narrow.** The core does not know which values a device has: a laser, a scale
and a printer report different things, and capabilities may add values without a migration. Narrow
fits that; wide would need a column per value per kind. Cost: ~15 rows per sample instead of 1, i.e. 10
devices × 15 values every 2 s ≈ 75 rows/s, against ≈ 7,400 rows/s measured live on .67 (study §6).
The .67 trial was wide, so step 3 repeats it narrow (size after compression, 1 h buckets over 3 days)
before history is called done. Text values (state names, dialog texts) are events, not samples.

**Tests on Postgres.** All store tests use `#[sqlx::test]` (fresh database per test, migrations applied).
Locally: `scripts/dev-db.sh` starts `timescale/timescaledb:2.32.0-pg17` in Docker and sets
`DATABASE_URL`; `scripts/check.sh` runs it. CI: a job of its own, `store` on `ubuntu-24.04`, gets the
same image as a service container and runs the store tests (behind a `pg-tests` feature, on there); the
platform matrix (`ubuntu-24.04`, `ubuntu-24.04-arm`, macOS, Windows) runs everything else and lists the
store tests as ignored (journal:V2-STORE-1:0008). `cargo sqlx prepare --check` in CI keeps `.sqlx/` current.

**Pi deployment (.67, armhf, 1 GB).** Docker `timescale/timescaledb:2.32.0-pg17` (arm/v7), limits as
tried (384 MiB, `shared_buffers=64MB`, `work_mem=4MB`, `max_connections=20`), listening on a Unix
socket or `127.0.0.1` only; AppArmor set up properly instead of `apparmor=unconfined` (study §6);
volume on disk; nightly `pg_dump` to beside the files dir; the hub binary stays native. Timescale apt
packages exist for arm64/amd64 only (study §6), so those hosts can skip Docker. Nightly means once a
day: the hub checks hourly and dumps when the last nightly dump is a day old; there is no fixed time of
day. Before each self-update the hub takes a dump; going back to the old version restores the database
from that dump along with the old binary, so changes made after the update started are lost. The hub
keeps the last 7 dumps of each kind (nightly and pre-update) and deletes older ones; `--keep-dumps`
changes the number (proxy, journal:V2-STORE-5:0021). The hub's client tools (`pg_dump`, `pg_restore`,
`psql`) are of the server's version, on `PATH`: on Debian 11 armhf built from PostgreSQL's source; the
hub gets no Docker rights (journal:V2-STORE-5:0014). How: [docs/pi-hub.md](../pi-hub.md).

### 2.8 API

**Decision.**
- **`/api/v1`** is the hub's public API, one namespace, their design style: nouns, keyset paging,
  stable error bodies `{code: "domain/REASON", …}`, events on `/api/v1/events`.
  - **3D printers:** their REST paths and fields for printers, jobs, files/storages and commands (R3).
    The spec is written by us from their behaviour (see §5 Q1), pinned to a `pc/` commit; a tracking
    test runs our hub and their server on the same requests where both can serve them.
  - **Ours:** spaces, members, grants, devices of other kinds, lasers, labels, queue, farm, materials
    and stock, cameras, door access, the agent's pairing; same conventions.
  - The Apps API (`/api/apps/v1`, `access.rs:177`) is removed: what it does is a call of `/api/v1`;
    no `/api/apps/v1` path is kept, no fallback (owner, 2026-11-19: "geen oude v1 meer").
- **Spoolman moves to `/spoolman/api/v1`.** Spoolman itself serves its API under a configurable base path
  (`reference/Spoolman/spoolman/main.py:57`), so a client pointed at `http://hub/spoolman` meets the same
  paths as a real Spoolman with that base path: to the letter of the reference, and the `/api/v1` clash
  (`spoolman.rs:37-49`) is gone. The H4 audit runs real Spoolman with the same base path.
- **Agent wire** stays its own namespace (`/api/agent/v2`), ours, not in the public spec.
- **OpenAPI from Rust: `utoipa`** (+ `utoipa-axum`): derives schemas from our serde types and operations
  from handler attributes, works with axum 0.8, the most used of the options; `aide` couples tighter to
  axum but is less used. The generated `openapi.json` is checked in; a test fails when the code and the
  file differ.
- **Diff check:** `oasdiff` in CI compares the checked-in spec with `main`'s and labels each change
  additive, neutral or breaking; breaking needs a line in a new `CHANGES.md` file in `docs/api/`. Before 1.0 a breaking
  change is allowed but never silent.

### 2.9 Web UI on the public API

**Decision: keep server-rendered pages, no SPA rewrite.** Every read the page does live and every
change it makes goes through the public `/api/v1` and `/api/v1/events`; the HTML shell stays rendered by
the hub. Why: ~6,700 lines of HTML/JS today in Rust strings and six JS files (`shell.js`, `machine.js`,
`files.js`, `library.js`, `label.js`, `labelwriter.js`, ~4,100 lines by `wc`); an SPA means a Node
toolchain, a second build on the Pi release path and a rewrite of every page with nothing gained for the
makerspace screens that already work. What changes: form posts to page routes become API calls; HTML
moves out of Rust string literals into template files (`askama`, compile-time checked) page by page as
each page moves onto the API; the UI study's rules (states, unknown vs zero, disabled-with-reason) apply
from then on. Controls are shown from the device's capabilities and the current state's accepted verbs.

### 2.10 Registries

One Rust table each, the single source; docs and OpenAPI enums generated; a test fails on drift.

| Registry | Holds | Replaces |
|---|---|---|
| Capability verbs | name, args (kind, unit, bounds), action needed, kinds | `protocol::Action` (`protocol/src/lib.rs:491`) |
| State sets | per kind | `MachineState` (`protocol/src/lib.rs:88`) |
| Event types | name, data schema, who may see | the `()` change signal (`store/mod.rs:533`) |
| Actions | name, presets that include it, damaging or not | `Level`/`Role` checks (`access.rs:36-100`), `RULES` (`access.rs:147`) |
| Fault codes | `domain/REASON`, HTTP status, our text | plain texts (e.g. `store/mod.rs:33`) |
| Metrics | name, unit | the fields of `Sample` (`store/history.rs:12-20`) |

## 3. Build order (big steps)

Each step ends green (fmt, clippy `-D warnings`, tests, audit checkers) and is reviewed before the next;
`on-track` runs before each. Old code goes in the step that replaces it.

| # | Step | Done when |
|---|---|---|
| 1 | **Contract + registries**: `crates/device` (identity, capabilities, state sets, patches, command/result, host validation), conformance kit, the six registries with generated docs | Conformance kit passes on a fake device and on each agent port behind an adapter (Marlin, labelle, 450, 320, LaserFoundry, usbscale); registry drift test in `check.sh` |
| 2 | **Store on Postgres**: sqlx, per-module migrations, the schema of §2.7 except history, RLS, `Store` handle; every module's queries moved; `rusqlite` removed | All hub tests on Postgres locally and in Linux CI; `cargo tree -p maker-connect-hub` has no `rusqlite`; hub runs on .67 against the container; an RLS test shows a query without `app.space_id` sees nothing |
| 3 | **Authz + events + history**: actions and presets, `device_access`, route guard from actions; event table and bus, `/api/v1/events` with resume; history subscriber into `sample` with aggregates; narrow trial repeated on .67 | Route walk: every route has an action; list/count/SSE filtering tests per role; resume test (disconnect, events, reconnect with id) ; .67 numbers recorded in this doc |
| 4 | **Commands + agent link v2**: command records, transfers, agent hosts sessions, one WebSocket per agent, on-disk outbox | Restart test: open command → outcome unknown, not re-sent; agent on .67 drives the MINI-less bench (Marlin + DYMO + laser framing only, laser never fires: safety rule) through v2 |
| 5 | **Prusa driver, websocket dialect only** (#2 `buddy-ws`) in the hub on the contract | #2 its own audit, tested on the owner's MINI with websocket firmware |
| 6 | **API**: utoipa spec, their 3D-printer REST, ours, Spoolman at `/spoolman/api/v1`, oasdiff in CI | Spec checked in and in sync; tracking test against their server for the 3D-printer paths both serve; Spoolman audit green with base path |
| 7 | **Web UI on the API**, page by page (Now, machines, machine, files, queue, labels, Space tabs) | No page route changes data; each page's HTML in a template; UI-study states on each page |

**Step 2's tables** (2026-11-18, orchestrator, on-track ON TRACK): step 2 moves today's tables and behaviour
onto Postgres, with `space_id` and RLS on every tenant table. The tables of §2.7 that only exist for a later
step come in that step: `action_grants`, `device_groups`, `device_access`, `authz_epoch` and `event`, `sample`
(step 3); `commands`, `transfers`, `device_capabilities`, `devices.state` (steps 4-5); token scopes and
generation (step 3). Today's id types stay (integer ids become `bigint`, text ids `text`); uuid v7 comes
with the API (step 6). Pages and JSON stay byte for byte as they are; timestamps serialise as today.

**V2-DEVICES** (2026-11-19, journal:DESIGN-WS:0001, journal:V2-DEVICES:0005) takes steps 4 and 5 in this
order: (1) the hub's device contract and agent link v2 (one WebSocket per agent); (2) agent v2 with
several devices per agent and its settings page
([decision](../design/decisions/20261119-agent-settings-page.md)); (3) the Buddy driver, websocket dialect
only. The first agent v2 devices (owner): the DYMO LabelManager PnP (labelle stack, tape) and the DYMO
LabelWriter 450 (DYMO CUPS stack, labels), kept as two separate stacks with no shared code.

**Parked:** HS10 (MINI button G-code: owner's choice, lands in the driver after step 5); #2 waits for
step 5 (the device model first); #4 MK3 PrusaLink HTTP port parked (MK3 to come over `/p/ws`, study §5b);
Marlin/OctoPrint port frozen as is (only wrapped by the step 1 adapter); HS11 Klipper and the SimplyPrint
route (`devices/simplyprint.rs`) frozen, Moonraker mapping later after their example; hosting their
out-of-process drivers (not built until Prusa publishes the driver methods); modules out of process.

**What v2 does to existing plans.**

| Plan | Replaced by v2 | Unchanged |
|---|---|---|
| hub-side.md | H1/HS9 (§2.2, §2.3), HS5 (§2.5, §2.3 outbox), HS8 (virtual devices implement the contract) | HS3, HS10, HS11, HS12 (moves to the new base path), HS13, HS15 |
| hub-modules.md | HS0b `Module` struct with `migrate: fn(&rusqlite::Connection)` and `RULES` per module → a module = migrations folder + routes with OpenAPI + registry entries; module state map | HS0a (done: `lib.rs`, `web/`, `store/`, `pages/` exist), Cargo features per module, lean build |
| agent-engine.md | Step 2 hub link and `protocol` crate (§2.3); `Order`/`Event` edge | Every port, its reference and audit; "one agent, many devices" |
| cameras.md | How a camera is listed and permitted (a device, visibility from its machine) | RTSP via retina + go2rtc, USB via nokhwa |

## 4. Risks

| Risk | Mitigation |
|---|---|
| Big rebuild while features exist: behaviour lost silently | Route and per-role snapshot rebuilt in step 3 from actions; each module's tests ported, counted before and after |
| OCL: reproducing their REST surface | §5 Q1; spec from our observations of behaviour, no `openapi.json`, no text |
| Their API is 0.x and moves daily (study §5b) | Pin a `pc/` commit per release of ours; the tracking test names each drift |
| 1 GB armhf Pi: hub + Postgres + Docker without proper AppArmor | Limits from the trial; AppArmor fixed in step 2; arm64 hosts may use apt |
| Narrow history heavier than the wide trial | Repeat trial in step 3 before it is called done; fall back to wide-per-kind only if numbers say so |
| sqlx offline cache and armv7 cross builds | `SQLX_OFFLINE=true` in builds; `prepare --check` in CI |
| Two Buddy dialects on `/p/register` | Gone: only the websocket dialect is built (§2.2) |
| RLS: a pooled connection keeps a setting | Only `SET LOCAL` inside a transaction; a test that a fresh transaction sees no space |
| SSE fan-out filter cost on the Pi | Per-subscriber action cache keyed by `authz_epoch`; measure with 20 subscribers |
| macOS/Windows CI lack Postgres | Store tests on Linux CI only, said so in `ci.yml` |

## 5. Owner's answers (2026-11-18: "akkoord met alle vijf adviezen")

All five recommendations below are accepted by the owner. The public question to Prusa (1) is still to be asked.

1. **Their REST surface under OCL.** Serving the same paths, field names and status codes for 3D printers
   means our spec mirrors theirs in substance. *Recommendation:* yes, written by us from running their
   server and our tests, never from their `openapi.json` or docs text; and ask Prusa in a public issue on
   their repo whether an API-compatible server is fine with them (they answer daily, study §5b).
2. **UI: server-rendered vs SPA.** *Recommendation:* server-rendered on the public API (§2.9); revisit an
   SPA only for the multi-tenant cloud if page weight or offline use calls for it.
3. **Hub host for v2.** *Recommendation:* .67 (armhf) with the Timescale Docker image as tried; document
   arm64/amd64 with apt packages as the preferred host for makerspaces.
4. **Damaging actions by explicit grant.** Raw G-code, `mc.laser.start`, firmware update granted by name to
   a person on a device, not included in the Operate preset (fits `access.rs:1444-1446`: being owner is
   not having had the laser course). *Recommendation:* yes; Space UI shows them as separate switches.
5. **Retention defaults.** *Recommendation:* raw samples 30 d, 1-min aggregates 1 y, 1-h aggregates
   forever; events 1 y; command records and audit log forever (space admins may lower per space later).
