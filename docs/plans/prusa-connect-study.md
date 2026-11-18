# Prusa Connect (self-hosted) as a reference: the architect's study

Reference: `reference/prusa-connect` at commit 2e8c62b (v0.1.0-beta.1), read only. Licence OCL v1.1 +
Micro + NoSaaS + SWAtt: nothing (code, `openapi.json`, `profiles.json`, docs text) is copied into this
repo; it is read to understand. Owner decision 2026-11-18: not ported. `pc/` = the reference,
`D/` = `pc/docs/src/content/docs/`.

## 1. What it is

One Go binary with the web app built in; public beta, API and SDK 0.x.

- **Kernel** (`internal/kernel`): per-device state tree patched by JSON pointer
  (`sdk/connectorapi/driver.go:138`); "the core knows devices, not printers", "no history in the core".
- **Drivers** (compiled in, `sdk/connectorapi`): Baseline / Session Identity, Capabilities, Do, State;
  inbound or outbound; capabilities = driver ∩ model profile ∩ device probe
  (`internal/printer/capability/resolver.go`); conformance suite. Shipped: `internal/connector/xbuddy`
  (Buddy over WebSocket); Moonraker is an example module.
- **Modules/extensions**: out-of-process, manifest, permissions, UI slots (materials, parts, fleet-stats,
  server-slicing, threemf, buddy-debug-logs). Adapters: storage, camera (rtsp, mjpeg, push, generic),
  sinks (mqtt, amqp, kafka, webhook), auth (oidc, prusaaccount), db.
- **Transports**: REST `/api/v1` (147 paths), SSE CloudEvents `/api/v1/events` with `Last-Event-ID`,
  MQTT sink, printer wire `/p/register` `/p/auth` `/p/ws`, mDNS `_prusa-connect._tcp`/`_prusalink._tcp`.
- **Auth vs authz**: accounts/credentials/scopes/RFC 8628 app pairing apart from a relation-tuple
  authorization graph (`internal/authz`); actions checked, not roles; damaging verbs granted by name.
- **Storage**: SQLite default, Postgres optional; history is a separate pluggable store fed by events.

### 1a. Their design decisions (`D/design/`), and what they mean for us

| Their decision (our summary) | Us now | Lesson |
|---|---|---|
| The core knows devices, not printers: no brand, model or file format in the core | Hub knows kinds (printer, laser, label printer) and Prusa specifics in `devices/` | Keep brand logic in engine ports/agent, not in hub pages |
| A driver declares commands, arguments and states; the core never compares firmware versions | UI and hub decide per kind what to show | Ports declare capabilities and states, hub and UI read them (§7 item 8, #12) |
| Features are modules (out of process, manifest); adapters compiled in | Everything in the hub binary | Later: only if the multi-tenant cloud needs it |
| Modules ask: manifest lists permissions, ports, DB, settings; acts with the caller's rights | n/a | Same rule for agent modules (#12) |
| The SDK is the contract: one small module, std-lib only, a test suite per contract | `engine` crate shared, no formal contract suites | A conformance test per port contract |
| One registry per kind of name: error codes, events, permissions, settings; docs generated, check fails on drift | Texts written where they are used (not checked in full) | Registered error codes and event types (§7 item 7) |
| Events in one envelope with type, origin and resume position; registered before publishing | 4 SSE streams, no `Last-Event-ID` (grep) | One event envelope with `Last-Event-ID` resume |
| Permissions checked on every path; lists filtered in the query, so counts reveal nothing | Space roles + machine groups (where lists filter: not checked) | Check that lists filter in SQL, also in SSE |
| No history in the core: a history adapter subscribes to events | 30 min in memory | Telemetry history as its own store fed by events (Timescale, §7 items 3-4) |
| API described by generated OpenAPI; a check names each change additive, neutral or breaking; the web app uses the same API as any client | `docs/api/apps-api.md` by hand, no OpenAPI (grep) | Generate our OpenAPI from code; web app on the public API (fits R3) |

## 2. Our open Prusa questions

| Item | What their server tells us | Files |
|---|---|---|
| #2 Buddy 6.4+ WebSocket (their server = server-side reference; firmware stays normative, `D/prusa-xbuddy-websocket-protocol.md:774`) | Paths `/p/register`, `/p/auth` (200 `{"uuid"}`, 401 missing header, 403 unknown), `/p/ws` subprotocol `prusa-connect`; pending 202/204; refused code `Retry-After: 54` | `internal/connector/xbuddy/xbuddy.go:14-24,57-68,128`, `auth.go` |
| | Framing: 9-byte header `[J/G/F/D/T][8 hex id]`; one command in flight per printer | `wire.go`, `session.go`, `D/architecture.md:67-80` |
| | Answers: ACCEPTED only for G/F; a query is answered by its data event; REJECTED without reason while G-code runs | `D/prusa-xbuddy-websocket-protocol.md:702-710`, `session.go:857` |
| | Fingerprint: 50 chars in body, first 16 in header | `xbuddy.go:70-114` |
| | Timeouts: read 60 s, ping 54 s, ping timeout 10 s | `session.go:76,352-361` |
| | Inline download: printer pulls blocks; `hash` = 16 random bytes for matching; chunk ≤ 1 MiB; each `T` frame ≤ 1011 bytes (TLS record); STOPPED = operator, ABORTED/FAILED = broke | `transfer.go:21-75` |
| | Not served: START_ENCRYPTED_DOWNLOAD (`verbs.go:242`); queue 32, ≤ 400 registrations | `xbuddy.go:120-200`, `registration.go` |
| | TLS 1.2, one suite ECDHE-ECDSA-AES128-GCM-SHA256, custom CA, IP-literal workaround | `D/certificates.md:331` |
| | Gap: they serve only 6.4.0+ (`D/prusa-command-architecture.md:51-53`); our hub keeps the 6.3.0 HTTP side and adds `/p/ws` beside it | |
| #3 Buddy 6.3.0 audit | 6.4+ telemetry keys as a cross-check list only (`inbound.go`, `telemetry*.go`); no help for `/f/<iv>/raw` | |
| HS10 MINI buttons | What their server *sends*: HOME `G28 [axis]`; MESH `G28\r\nG29`; MOVE `G91\r\nG1 F..X..Y..\r\nG90`; MOVE_E `M83\r\nG1 F.. E..`; LOAD `M701 S"<name>" W2`; UNLOAD `M702 W2`; `M104`/`M140`/`M220`/`M221`; BEEP `M300 S950 P800`; `M84`; `M999 R`; `M997 <path>`; CRLF. Conflicts with the owner's HS10 ruling (Buddy's menu G-code): owner decision | `verbs.go:299-350`, `gcode.go:24-33,142-280` |
| | Per-model command matrix / device_type_id | `D/prusa-command-architecture.md:33-44,150-202`, `internal/printer/profile/profiles.json` |
| #4 MK3 (PrusaLink) | Not supported by their server (`D/prusa-command-architecture.md:46-49`); nothing changes `mk3-port-plan.md` | |
| Printer error codes | They pass dialog code/title/text through raw; keep our Prusa-Error-Codes reading | `inbound.go:67-80` |

### 2a. Buddy 6.3.0 against 6.4.0: one code base, two transports (checked 2026-11-18)

`reference/buddy-6.3.0` against `reference/buddy-6.4.0` (sparse clone of tag v6.4.0, b91eeda: `src/connect`,
`ProjectOptions.cmake`).
- **The only switch is the build option `WEBSOCKET`**: OFF in 6.3.0, ON in 6.4.0 (`ProjectOptions.cmake:719` /
  `:789`, the comment calls the server side "not ready" in 6.3.0). Both versions already carry both code paths
  (`src/connect/connect.cpp`: `BasicRequest` for HTTP, `UpgradeRequest` to `/p/ws` with subprotocol `prusa-connect`).
- **Same command names** (26 string constants in `command.cpp`), **same event names** (12 in `render.cpp`) and
  **the same JSON keys** in `render.cpp`: no difference between the two versions (string sets compared, scratch).
- **Transport differs**: 6.3.0 sends `POST /p/telemetry` and `POST /p/events` and reads the command from the
  answer; 6.4.0 opens one WebSocket `/p/ws` and frames messages. Registration `/p/register` is the same path in both.
- Small differences: 6.3.0 sends two User-Agent parts in its headers (`user_agent.hpp`), 6.4.0 does not;
  6.4.0 reads a ping fragment fully (`rx_exact`); telemetry timing is the same (750 ms min, 1 s busy, 4 s idle,
  full every 5 min, `planner.cpp:88-102`). Inline downloads (`inline_request`) only exist on the WebSocket path.
- Not checked: 6.4.1 and later (their server says 6.4.0+; Buddy main is at 9.0.0).

So the Prusa driver in hub-core v2 has **one shared core** (commands, events, telemetry, files) and **two
transports**; the 6.3.0 audit rows for content carry over to 6.4.0.

## 3. Where we build what they already have

| Ours | Theirs | Recommendation |
|---|---|---|
| Machine profiles (`profiles/`, `hub/src/profiles.rs`): geometry | Per model+firmware command sets (`profiles.json`); geometry from the driver | Keep ours; adopt the idea of a per-model command set filled from firmware sources |
| Registration (`protocol`, `devices/prusa_connect.rs`) | `device_registration`, hashed tokens, approval, identity `{vendor,model,serial,unit_uid}` | Keep ours; check tokens are hashed; stable unit id; 16/50 fingerprint rule for #2 |
| Access vs permissions (`access.rs`) | Authn apart from authz graph | Keep ours (Space); adopt: rules name actions, agent tokens report/fetch only, damaging commands by explicit grant, RFC 8628 pairing for apps |
| Jobs/files/queue/farm | `printer_job`, storages; no queue/farm | Keep ours; for #11 adopt "durable command record" requested → authorized → settled (`pc/docs/decisions/20260901-atx-durable-command-records.md`) |
| Telemetry: SSE, 30 min in memory (`store/history.rs`) | History = event subscriber with its own store | Adopt the design (entry point for TimescaleDB); keep our SSE |
| Cameras (retina + go2rtc MSE) | rtsp/mjpeg/push/generic; camera grants follow its printer | Keep ours; adopt: visibility follows the machine; agent cameras push frames (#5); 304 snapshots |
| Materials / Spoolman | materials extension, OpenPrintTag, per-tool assignment with odometer | Keep ours (Spoolman decision); idea for #10 |
| Firmware command matrix (HS10 open) | `prusa-command-architecture.md`, `verbs.go`, `gcode.go` | Owner decision; prove each row against firmware |
| Errors: plain texts | `sdk/fault` stable `domain/REASON` codes, device issues with severities | Adopt the design (own codes) for the apps API |
| Engine ports vs their connectors | Capabilities/states declared by the driver | Adopt the idea: ports declare capabilities and states, hub UI reads them (#12) |
| Discovery: none | mDNS | Later, optional |

## 4. Database

Theirs: authz (`authz_*`, `device_access`), auth (`auth_*`), devicestore (`device_record` with last state
JSONB, `printer_job`, `device_registration`, `camera`), plus a **separate history store**
(`adapters/db/internal/histstore`): `history_series` + `history_sample` (BRIN on time), `history_fact`,
`history_gap`, `history_command`, `history_transfer`; retention 48 h default / 168 h from the wizard; no
buckets. Their decision rf2 foresees "Timescale for metrics beside Postgres for audit".

Ours: 37 SQLite tables via one `rusqlite::Connection` in `Arc<Mutex<Store>>`; ~471 SQL sites, ~431 lock
sites, 20 `rowid` uses; telemetry history not persisted; 162 hub tests on `:memory:`.

TimescaleDB sketch: plain tables stay; new hypertables `telemetry` (per machine: state, temps, fans, job,
progress; compression after 7 d, retention 90 d, continuous aggregates 1 m / 1 h) and `machine_event`
(jsonb, 1 y); `audit_log` as hypertable; jobs stay plain. Unique keys on hypertables must include time.

Cost of a full move: async store or store actor (431 lock sites), `sqlx`/`tokio-postgres`, migrations,
`RETURNING id`, `pg_dump` backups, tests need Postgres, Pi install needs Postgres + Timescale (arm64 and
1 GB Pi not verified). Compression/retention/aggregates are TSL community features (separate process, no
AGPL conflict). Full move: weeks. History only: small.

Recommendation (superseded by R2 in §6: everything to Postgres + Timescale): as Prusa does: keep SQLite default; add a `History` sink trait with a SQLite and an
optional Timescale implementation; move the rest only if still wanted.

### 4a. Their schema read closer, and what we take from it

Read from the Postgres migrations of each store (`internal/platform/devicestore`, `internal/authz`,
`internal/platform/auth/dbstore`, `adapters/db/internal/histstore`, `.../recstore`). Our summary, not their text.

| What they do | Why it matters | For us |
|---|---|---|
| **No tenant column anywhere**; one server = one owner. Several sites are linked as a tree of servers (Fractal) | Their multi-site answer is many servers, not one shared DB | We already have `organisations`, `spaces`, `memberships`; for the multi-tenant cloud keep `space_id` on every tenant table and consider Postgres row-level security |
| Each store owns its own migrations, in a SQLite and a Postgres folder side by side | A store can be swapped or dropped on its own | One migration folder per store module (machines, jobs, access, stock, …) |
| Device = current state only: one row with a `JSONB` state document (with `CHECK jsonb_typeof = object`), plus `last_seen`, `state`, `current_job_id`, `retired_at` | Current state is one read; history lives elsewhere | `machines` gets a `jsonb` last-state; telemetry goes to the hypertable |
| Retire, don't delete (`retired_reason`, `retired_at`); FKs `ON DELETE CASCADE` from the device | Old jobs keep a name; a real delete cleans up | Same for machines |
| Jobs: one row per print with `state`, `outcome`, `progress`, `started`/`ended`; **partial index** `WHERE ended IS NULL` for running jobs | Running jobs are found without scanning history | Same idea for `jobs` and `machine_queue` |
| Registration: `(connector, code)` key, unique fingerprint, **only a token hash** stored, `approved`/`admitted_by`/`expires_at` | No usable secret in the DB | Check our agent and printer tokens are stored hashed |
| Permissions: a relation-tuple graph (`authz_tuple`, Zanzibar style) **plus a materialised table** `device_access(subject, device, actions BIGINT bitmask)` and an `authz_epoch` counter | The graph is flexible; the bitmask table makes "which machines may I see" one indexed join, and lists filter in SQL | Our `machine_grants`/`group_grants` → a materialised per-user per-machine action set, rebuilt on change |
| Credentials: only a digest; `scopes`, `expires_at`, `idle_secs`, `last_seen`; **generation counters** and `minted_by`/`root_creator` | Removing a person or bumping a generation kills every token minted from them at once | Same for `api_tokens` and sessions |
| App pairing per RFC 8628 (`user_code`, `interval_secs`, `polled_at`, decided scopes) | Apps get a token without typing a password | For the apps API later |
| History is narrow: `history_series(ref, metric)` + `history_sample(series_id, at_us, value or text)`, BRIN index on time | One table for every metric, new metrics need no migration | With Timescale: choose **wide** (one row per sample, columns per value, `segmentby machine_id`, as tested on .67) for the fixed set, or narrow for free-form metrics; decide in the move step |
| `history_fact`: events with `(ref, epoch, sequence)` and `JSONB` payload; `history_gap`: where data was lost | A client can resume; a hole in the data is a stated fact, not silence | `machine_event` hypertable with sequence; record gaps (hub restart, agent offline) |
| `history_command`: requested → authorized → settled, with `outcome_unknown`, `fault_code`, who authorised; partial index on open ones | The durable command record (§3, #11) | One table for every command we send to a machine |
| `history_transfer`: bytes total/sent, state, reason per file transfer | Upload progress and failures survive a restart | Same for `printer_transfers` |
| `module_state`: a namespaced key/value store with `version` and `bytes` | Modules keep data without own tables | For agent modules (#12), later |
| Text ids, `TIMESTAMPTZ` (history uses integer microseconds), `COLLATE "C"` sort keys with `(sort_key, id)` indexes | Stable keyset paging | Use `timestamptz` (Timescale needs it) and keyset paging on long lists |

Ours today (37 tables): `access_requests api_tokens audit_log cameras extra_field filament files group_grants
identities invites items jobs locations machine_grants machine_group_members machine_groups machine_queue machines
materials memberships meta movements org_memberships organisations part_files part_ledger parts printer_files
printer_transfers projects sessions setting spaces spool tag users vendor`. Tenancy (`organisations`, `spaces`)
is something they do not have.

## 5. API compatibility

Their surface: REST 147 paths, SSE CloudEvents, MQTT, `/p/*`; driver SDK unpublished; out-of-process
drivers not built yet. Clash: our Spoolman API also uses `/api/v1` (`/api/v1/health` both).

| Option | Verdict |
|---|---|
| (a) our hub serves their REST/SSE | 0.x API, no spaces/lasers/labels in their model, second API to keep; spec must be our own writing |
| (b) our agent as a driver in their server | Go, compiled into their binary (AGPL vs OCL conflict unless permissive or out-of-process); non-Prusa verbs stripped |
| (c) both | both costs |
| (d) neither now | was recommended, superseded by R3 in §6; take the design ideas; revisit (b) when their out-of-process contract ships and API is 1.0 |

### 5a. Connector compatibility (owner, 2026-11-18)

Owner: Prusa has thought the 3D-printer side through (years of Connect in the cloud, the 32-bit printers like
our MINI, a Klipper/Moonraker example); make the 3D-printer API compatible, and a connector someone builds for
Prusa Connect Local should work for us too.

How their connectors attach today (read from `D/connector-contract.md`, `D/creating-connectors.md`,
`D/plugin-wire-protocol.md`, `examples/adapters/moonraker/build.yaml`):
- A driver is a **Go module compiled into their server binary** (`build.yaml` admits it; Moonraker is built
  this way). It implements `sdk/connectorapi`: `Baseline`, `Outbound.Connect` and/or `Inbound.Handler`,
  `Session.Identity / Capabilities / Do / State / Close`; the host intersects capabilities (driver baseline ∩
  model profile ∩ device probe), validates every command before the driver sees it, and supervises the session.
  A conformance suite (`connectorapitest`) holds drivers to it.
- An **out-of-process wire protocol** exists (child process, one JSON handshake line, frames on stdio, JSON-RPC,
  credit per stream; vectors in `docs/wire/plugin-wire-protocol.json`), but **the printer-driver method set over
  that wire is "not built"**.

So: a connector written for them **today** cannot run in our hub (Go, linked into their binary). It becomes
possible when they publish the driver contract over the wire: then a driver is a separate program in any
language, and our hub can host it by speaking the host side of that wire.

Plan:
1. **Their device contract is our device model.** Same concepts and meanings: identity
   `{vendor, model, serial, unit_uid}`, inbound/outbound, capability set by intersection, host-side validation,
   `Do` that never reports OK for an outcome it did not observe, state as patches. Written in Rust by us.
   Our engine ports (agent) implement it; lasers and label printers declare their own verbs and states with the
   same mechanism.
2. **Their 3D-printer model is our 3D-printer model**: verbs, states, telemetry keys, jobs, per-model command
   sets, MINI (device_type 2.1.0) included. Proven against Buddy firmware row by row, as always.
3. **Their REST for 3D printers** (R3), our calls for the rest in their style.
4. **Wire host**: our hub can launch and host out-of-process drivers over their wire protocol as soon as the
   driver method set is published; until then, follow `plugin-wire-protocol.md` and do not build ahead of it.
5. Moonraker: their example driver is the reference for mapping Klipper onto the capability model (#9).

### 5b. Their repo on 2026-11-18 (issues and PRs, what Prusa staff said)

Head still 2e8c62b (#23), release only 0.1.0-beta.1; 33 issues/PRs in two days, Prusa staff answer daily.
- #9: PrusaLink gets a last update **on the WebSocket protocol**, so the MK3S will be supported (staff, no date).
  → MK3 goes over `/p/ws` like Buddy; our #4 (MK3 PrusaLink HTTP port) stays parked.
- #24: **the next PrusaSlicer 3 alpha uploads to local Connect**. → An API-compatible hub gets PrusaSlicer upload
  for free. A community extension already speaks OctoPrint's upload API for PrusaSlicer 2.x (ZachGoldberg/connect-octoprint-upload).
- #27: they announce `_prusalink._tcp` but `/api/version` is 404 (PrusaSlicer's PrusaLink host fails); open.
- #8: print queue "soon, as an extension". → Watch its design before #11.
- #13: Buddy3D camera onboarding planned with new camera firmware; a user reports their `rtsp` camera never dials.
- #29: path prefix for Home Assistant Ingress being researched (`prusa3d/prusa-connect-homeassistant`).
- #14: hardware floor not stated in the README; releases list the systems (32-bit ARM built).
- Open PRs: #28 chamber temperature on dashboard + MQTT, #15 EasyPrint, #30 trust mark icon, #33 image tags.
- #15 (draft, not to merge before Printables ships): **EasyPrint** (Printables' browser slicer) sends prints
  to a person's own server from `https://slicer.printables.com`, with an app-pairing credential; the server
  only allows that origin (CORS allowed origins, empty by default) and links to Printables' pairing page.
  → Step 6: an API-compatible hub gets EasyPrint for free if it has their app pairing, a CORS allowed-origins
  list (empty by default, one switch per origin) and HTTPS (Firefox blocks plain HTTP to a private address).

- #26: a Buddy printer stores the Connect hostname in a fixed 20-byte slot (6.3.0:
  `config_store/old_eeprom/constants.hpp:9` `CONNECT_HOST_SIZE { 20 }`); Prusa's own domains are compressed.
  Open firmware PR prusa3d/Prusa-Firmware-Buddy#5631 raises it to 35. → Our add-machine dialog writes the
  page's hostname into `prusa_printer_settings.ini` unchecked (`crates/hub/src/shell.js:89`): own issue.
  In v2 the limit belongs to each dialect's baseline, by firmware.

### 5c. Other prusa3d repos, 2026-11-18

- `Prusa-Connect-SDK-Printer` branch `feat/ws-connect-local` (14 commits ahead of master `3d21ae5`, head
  `04c5bd59`, 2026-11-17): telemetry and events over a WebSocket to the local server, file transfer inline
  in it, reconnect. `Prusa-Link` branch `websockets` (71 ahead) installs that SDK branch. Masters unchanged,
  not released. → Confirms #9: the MK3 comes over the WebSocket; the reference for that is these branches
  once released. Read them for how a printer tells `/p/register` for the WebSocket apart (§2, not checked).
- `Prusa-Firmware-Buddy`: releases v8.0.1 (11-03), **v8.0.2** (11-16), v8.0.3-beta. → Proposal: the
  `buddy-ws` dialect (hub-core-v2 §2.2, step 5) takes the newest stable release as its reference instead
  of 6.4.0; the MINI stays on 6.3.0.
- New, **MIT**: `prusa-connect-extension-example` (Go process + UI bundle, v1.0.0),
  `prusa-connect-marketplace-publish` (Action: check, sign, publish an extension),
  `prusa-connect-homeassistant` (HA integration, a client of their REST API, v0.1.1). MIT allows use, unlike
  the server (OCL). → The HA integration is a ready client for step 6's tracking test against our hub.
- `Prusa-Firmware-ESP32-Cam` (GPL-3.0) updated: camera for Connect; read with the cameras plan.

## 6. Owner rulings (2026-11-18)

- **R1**: yes. Their server is the read-only reference for server-side behaviour; Buddy firmware stays normative.
- **R2**: everything to Postgres + TimescaleDB, after a Pi check (no users, no data to move; the cost is the code only).
  Pi check 2026-11-18, hub Pi .67 (Orange Pi PC, armv7l/armhf, Armbian 26.8.3 bullseye, 991 MB RAM, ~748 MB available, 11 GB free):
  Debian has `postgresql-13` for armhf; Timescale's apt repo has **no armhf/armel packages** (bullseye/bookworm: arm64 and amd64 only);
  the official Docker image `timescale/timescaledb` 2.32.0 (pg16/17/18, also `-oss`) **is published for arm/v7**. Docker is not
  installed on .67.
  Trial on .67 (Debian `docker.io` 20.10.5, `--security-opt apparmor=unconfined` because `apparmor_parser` is missing;
  image `timescale/timescaledb:2.32.0-pg17`, 1.62 GB, pull 7 m 42 s; PostgreSQL 17.11 32-bit; container limit 384 MiB,
  `shared_buffers=64MB`, `work_mem=4MB`, `max_connections=20`), hub and four agents running alongside:
  - ready 16 s after start; idle ~48 MiB, after the load ~110 MiB, peak 191 MiB; hub stayed active; host load 3.6 during the bulk load.
  - 1,296,010 rows (10 machines × every 2 s × 3 days, 1-day chunks, `segmentby machine_id`): bulk insert 2 m 14 s; 120 MB raw.
  - live insert: 10 rows (one per machine) × 1000 in 1.36 s (≈ 7,400 rows/s, far above 5 rows/s for 10 machines at 2 s).
  - queries (machine 3): 1-minute buckets over 1 h 32 ms; 1-hour buckets over 3 days 406 ms raw, 267 ms with compressed chunks.
  - compression: chunks 40 MB → 4.6 MB and 6.4 MB → 0.8 MB (≈ 9×, random test values; compress run 25 s).
  - continuous aggregate 1 min: first full refresh 37 s; 3 days of one machine (4,317 rows) read in 18 ms.
  - the `tsdb.hypertable` + `segmentby` table gets a columnstore (compression) policy by itself; retention and aggregate policies added.
  Verdict: Postgres + Timescale runs on the 1 GB armhf hub Pi with room to spare. Container `tsdb-test` stopped, volume kept.
- **R3**: their REST for 3D printers; our own calls for everything else (lasers, labels, spaces, …), written in their design style.
  This replaces the (d) recommendation in §5.
- **Both Buddy protocols** (owner, 2026-11-18): the hub keeps the 6.3.0 HTTP Connect side (the owner's MINI stays
  on 6.3.0) and adds the 6.4+ `/p/ws` side beside it. Their server serves only 6.4+, so a 6.3.0 MINI cannot be
  compared against it; Buddy 6.3.0 firmware stays the reference for that side.
- **Test bench idea**: their `make firmware-up` / `mini404-smoke` run **real Buddy firmware under MINI404**
  (vintagepc/MINI404, QEMU) against a server (`tools/mini404/knowngood.json` pins MK4 and CoreOne at Buddy
  `master@e96ce2b92`). A MINI build of Buddy 6.3.0 under MINI404 would test our HTTP side on real firmware, and a
  6.4+ build the `/p/ws` side against their server and ours. Not tried; needs brew build deps on the Mac. The spec is our own writing (licence); the `/api/v1` clash with Spoolman has to be settled.

## 7. Order (owner, 2026-11-18: "we gaan eerst hier aan de gang")

1. **Now: the hub-core v2 design** (`docs/plans/hub-core-v2.md`), design only. Nothing below starts before the
   owner has reviewed it; the items below are chapters of that design, not separate work.
2. Then build v2 in the order the design gives (device model, store on Postgres + Timescale, events, command
   records, API, UI).
3. Parked behind the v2 device model: HS10 (#3), `/p/ws` (#2). Parked: #4 (MK3 PrusaLink HTTP port; Prusa will move
   PrusaLink to WebSocket, issue #9 on their repo). Frozen as is: the Marlin/OctoPrint port (#8 audit rows).
4. Absorbed into the v2 design, not done separately: #11 (job history, durable command records), #12 (multi-device
   agent, declared capabilities), #5 (USB cameras as devices), #7 (Spoolman API vs `/api/v1`).
5. Untouched for now: #1, #6, #9, #10.
6. UI lessons: [prusa-connect-ui-study.md](prusa-connect-ui-study.md) §4, after v2.
