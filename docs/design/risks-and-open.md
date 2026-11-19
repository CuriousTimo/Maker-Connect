# Risks and open questions

What this design does not decide, and what could go wrong with it. Each open question has options and
the architect's recommendation; the owner, or the proxy for internal choices, decides, and the answer
becomes a decision record.

## Risks

| # | Risk | Mitigation |
|---|---|---|
| R-1 | The admin pool (BYPASSRLS) used where a space transaction should be: RLS is then no second wall | The allowlist of [database.md](database.md#roles-and-pools) and a test that extracts every admin-pool call from the source |
| R-2 | The hub's login escapes RLS | The login is neither superuser nor owner, `NOINHERIT`, no privileges of its own; `RESET ROLE` reads nothing (a test). **What stays:** the login may `SET ROLE maker_connect_owner` (the owner bypasses RLS without `FORCE ROW LEVEL SECURITY`, and `FORCE` breaks `pg_dump`'s `COPY`) or `SET ROLE maker_connect_admin`, and any role may `SET app.space_id`. RLS guards against our own query forgetting the space filter, not against a compromised process or an injected statement. Guards: sqlx's checked, parameterised queries only; a source test, `no_role_switch_outside_the_pool_module`, fails on `SET ROLE`, `RESET ROLE`, `set_config` or `app.space_id` in any source file but the pool module ([decision](decisions/20261119-database-roles.md), [OQ-8](#oq-8-one-login-role-or-one-per-pool)) |
| R-3 | Tokens in URLs end up in logs and browser history | What the request log redacts is [OQ-31](#oq-31-log-redaction-and-urls-a-device-hands-the-hub); Spoolman's case is [OQ-7](#oq-7-how-spoolman-clients-authenticate); `/f/{iv}/raw` is Buddy's own (the key is not in the URL) |
| R-4 | A TimescaleDB restore without a superuser (the template database, `--role=maker_connect_owner`) is unproven | Prove it in the store CI job before step 3 calls history done; else the restore step needs the operator |
| R-5 | Big rebuild while features exist: behaviour lost silently | Route and per-role snapshot rebuilt from actions in step 3; each module's tests ported, counted before and after (plan §4) |
| R-6 | Reproducing Prusa's REST surface under OCL | [Decision](decisions/20261118-their-rest-surface-under-ocl.md): from running their server, never their spec text; the public question to Prusa is still to be asked |
| R-7 | Their API is 0.x and moves daily | Pin a `reference/prusa-connect` commit per release of ours; the tracking test names each drift |
| R-8 | 1 GB armhf Pi: hub + Postgres + Docker | Limits from the trial; AppArmor confined; arm64/amd64 hosts with apt preferred |
| R-9 | Narrow history heavier than the wide trial | Trial repeated narrow on .67 in step 3; wide-per-kind only if the numbers say so |
| R-10 | sqlx offline cache and armv7 cross builds | `SQLX_OFFLINE=true` in builds, `cargo sqlx prepare --check` in CI |
| R-11 | A pooled connection keeps a setting | Only `SET LOCAL` inside a transaction; a test that a fresh transaction sees no space |
| R-12 | SSE fan-out filter cost on the Pi | Per-subscriber action cache keyed by `authz_epoch`; measured with 20 subscribers |
| R-13 | macOS and Windows CI have no Postgres | Store tests on Linux CI only, said so in `ci.yml` |
| R-14 | The contract is proven by the conformance kit before any port runs on it in production | Step 4's done-when: the agent on .67 drives the bench through v2 (the laser never fires) |
| R-15 | A camera that never sends another key frame grows its GOP without bound | Noted in cameras.md; a bound is ours to propose when it is seen |
| R-16 | The `users` policy (visible through a membership of the current space) costs a join on every read | Index `memberships (space_id)`; measure in step 3 |
| R-17 | The advisory lock that puts event `seq` in commit order serialises every transaction that writes a fact ([decision](decisions/20261119-event-order.md)) | The lock is held for the rest of a short transaction only; measure the event rate on .67 in step 3; the alternative (a relay that orders by commit LSN) is kept in the decision |
| R-18 | A power loss during an update trial or the way back leaves the old build facing a migrated database | The update marker ([decision](decisions/20261119-update-trial-marker.md)): no build opens the database while the marker says `going_back`; the restore is repeated at start |

## Open questions

### OQ-1: Exact paths and fields of our own endpoints

The plan fixes the namespace and the style (§2.8), not each path. Options: (a) take [api.md](api.md)'s
table as the step 6 starting point, the generated spec then being the only truth; (b) design each path
anew in step 6. **Recommendation:** (a).

### OQ-2: The 3D-printer REST surface

The paths mirror Prusa Connect's, written from running their server (R3, OCL). Open: which of their
resources come first; whether the hub also offers the upload path PrusaSlicer uses for PrusaLink hosts
(`/api/version`, digest auth: prusa3d/prusa-connect#27 shows their server does not), or waits for
PrusaSlicer 3's own upload to Prusa Connect Local (prusa3d/prusa-connect#24); their print queue is
announced as an extension (prusa3d/prusa-connect#8). **Recommendation:** printers, jobs, files and
storages, commands first, as the plan says; follow PrusaSlicer 3's local-Connect upload once it is
released rather than PrusaLink's; our queue stays ours until theirs exists.

### OQ-3: Telling the two Buddy dialects apart

Both use `/p/register`; how a request shows its dialect is not checked (plan §2.2). **Recommendation:**
settle it from Buddy's own sources in the #2 audit (the 6.3.0 User-Agent with two parts is one lead,
issue #2), never by comparing firmware versions.

### OQ-4: Action names and preset contents

**Decided:** one area per module with `.view`, `.use` and `.manage`, device actions as the registry has them plus `device.manage` and `device.firmware`; presets are today's levels and roles one to one, checked by the per-role route snapshot; subjects persons only ([authz.md](authz.md#actions), [decision](decisions/20261119-actions-and-presets.md)).

### OQ-5: The job lifecycle

**Decided:** derived, never set by a page: `queued`, `loaded`, `running`, `paused`, `ended` with an outcome ([database.md](database.md#files-jobs-queue-and-farm), [decision](decisions/20261119-job-states.md)).

### OQ-6: Ids people see and Spoolman's integer ids

uuid v7 for rows people see (plan §2.7), but Spoolman's API exposes integer ids, and today's id
helper takes the maximum over all spaces (one counter across tenants, and ids reused after a delete).
Options: (a) Spoolman tables keep integer ids from one sequence per table (never reused); (b) per
space counters; (c) uuids mapped to integers at the Spoolman edge. **Recommendation:** (a): a plain
identity per table, never reused, leaks no counts of other spaces beyond order.

### OQ-7: How Spoolman clients authenticate

Spoolman clients are configured with a base URL only. Options: (a) a token in the path
(`/t/<token>/spoolman/api/v1`); (b) no auth on the LAN for one space; (c) a token as a query
parameter. **Recommendation:** (a), a token with only Spoolman scopes, the request log never writes
the path's token.

### OQ-8: One login role or one per pool

`DATABASE_URL` only ([decision](decisions/20261119-database-url-only.md)) gives one login, which may
`SET ROLE` to the owner (bypasses RLS) and the admin (R-2). Options: (a) one login, the source test and
sqlx's checked queries as the guard; (b) a second login for the migrator and the dump tools only, so
the serving process can never reach the owner role, with the app and admin roles still behind one
login; (c) three logins, one per pool, each with its own address variable. **Recommendation:** (a) now,
(b) as the first step towards a hub hosted for others, (c) for the multi-tenant cloud.

### OQ-9: How USB camera video reaches the hub

Snapshots went over the agent link in step 1; live MJPEG is ~1.2 Mbit/s per camera at 5 fps. Options:
(a) frames as upstream items on link v2; (b) a separate streaming upload per camera; (c) go2rtc's MJPEG
way for USB cameras too (owner asked whether that could be used later). **Recommendation:** decide
with step 4 of cameras.md, after measuring (a) on .67.

### OQ-10: Door access link

Doors and badges are their own module; Home Assistant does the hardware first and Maker Connect answers
who may open which door when. Its tables, actions and link (HA calling the hub, or the hub calling
HA) are not designed. **Recommendation:** design it when the hardware is chosen; keep the module
boundary now.

### OQ-11: Agent identity and device credentials

**Decided:** (a), one agent credential for the link (`agents`), each device registered and paired
under it with its own code ([decision](decisions/20261119-agent-link-identity-and-delivery.md)).

### OQ-12: The Prusa token until the code is spent

The printer may ask `GET /p/register` again until its first report spends the code; the hub then
must give the same token again, but tokens are stored hashed. Options: (a) the token kept in
`device_registrations` until spent, then only its hash; (b) a new token each time. **Recommendation:**
whatever Buddy's registrator and Prusa's server do (read in #18); until read, (a).

### OQ-13: The event types

The event types registry is empty. Named in the plan: device online or offline, job started or ended,
command settled, grant changed, material moved, `stream.reset`; a snapshot per device on resume.
**Recommendation:** fill the registry in step 3 with those names (`device.online`, `device.offline`,
`job.started`, `job.ended`, `command.settled`, `transfer.settled`, `grant.changed`, `material.moved`,
`device.snapshot`, `stream.reset`), each with its data schema and who may see it.

### OQ-14: Tables of materials, items and labels

The plan lists `materials` and `items` and "labels: own tables" without columns; the Materials redesign
(#10: tapes apart from filaments, laser materials) and the label editor in Prepare (#6) shape them.
**Recommendation:** design them with #10 and #6, one subject per table, keeping Spoolman's tables as
Spoolman's.

### OQ-15: Device identity across replugs and agents

**Decided:** per port, from what its reference reads, never guessed; a port whose reference reads nothing leaves `unit_uid` empty and the person tells the devices apart by name ([decision](decisions/20261119-device-identity-per-port.md)). Re-pairing is [OQ-37](#oq-37-re-pairing-a-device-that-has-a-row).

### OQ-16: The house colour's text shade

`#93A682` is 2.62:1 on white. A darker shade of the same hue for text on light backgrounds: `#697C58`
gives 4.54:1 on white but 4.23:1 on `#f6f7f5`. Options: (a) `#697C58` with a white page; (b) a shade that
reaches 4.5:1 on the light page background chosen; (c) dark ink for all text, the colour only as fills.
**Recommendation:** (c) for body text and (b) for links, the exact shade checked by a contrast test once
the light background is set.

### OQ-17: Room camera rights in the action model

The owner set a minimum role per room camera (default admin). Options: (a) the role kept as a camera
setting, checked beside `device.view`; (b) `device.view` granted on the camera like any device.
**Recommendation:** (b), the default grant made from the role, so one model serves both.

### OQ-18: Agent link details

The JSON names of [wire.md](wire.md), the batch size, how often the agent retries and how the SSE
stream is resumed are ours and not in the plan. **Recommendation:** wire.md as the start; the agent's
values as today's approved ones (agent-engine.md O4-O7), fixed by the generated schema in step 4.

### OQ-19: The flag that names the folders

The folders (files, dumps, downloads) are named after `--db maker-connect.db`, a SQLite name the store
no longer has. **Recommendation:** one `--data <dir>` flag holding the three folders.

### OQ-20: The agent's management page

One agent for several devices (#12) with a small web page: who may open it (the host only, or a
password or pairing with the hub), the config file or the page as the source, a thread or a process per
device. **Recommendation:** decide with #12; the hub side of this design does not change.

### OQ-21: A printer's storage listing

What a printer's USB stick or SD card holds is a table today; the plan does not place it. Options:
(a) part of the device's state tree (`/storage/...`), as reported; (b) its own table. Prusa's SDK 0.9
moves to a lazy file protocol (INFO lists storages only, listings on request; prusa3d/Prusa-Link#1059,
#1060), which shapes what a Prusa reports. **Recommendation:** (a), following what the driver's
reference reports at its pinned commit.

### OQ-22: Tenancy of history samples

**Decided:** (a): `space_id` on `sample` and `gap`, RLS `space`, the aggregates group by it ([decision](decisions/20261119-history-tenancy.md)).

### OQ-23: Readings declared by the driver

Prusa proposes that a driver describes its readings in the device state (`/readings/<name>/kind`,
`unit`, `value_path`, `target_path`, `role`; prusa3d/prusa-connect#37 and PR #38, unmerged). Ours has a
core metric registry. **Recommendation:** for 3D printers follow theirs in the API layer once merged and
pinned (R3); until then our metrics registry, with the path of each value in the state tree.

### OQ-24: Serving under a path prefix

Behind Home Assistant Ingress or a sub-path proxy, root-absolute paths break (prusa3d/prusa-connect#29).
Our pages, the API and the camera WebSocket are root-absolute too. **Recommendation:** not now (LAN
hub); when wanted, one prefix setting applied to every URL the hub writes.

### OQ-25: Showing Prusa error codes

Buddy and the MK3 report Prusa error codes; Prusa-Error-Codes is still changing (PRs #203, #195, #98,
unmerged). **Recommendation:** pass on the printer's code and text as it sends them; render texts from
Prusa-Error-Codes only at a pinned commit, if ever.

### OQ-26: Does a command carry the epoch?

**Decided:** (a): no epoch on a command; the host checks the verb against the current set ([decision](decisions/20261119-no-epoch-on-commands.md)).

### OQ-27: The number of actions

Actions are bits of one `bigint` (plan §2.6); the plan states no limit. Options: (a) one `bigint`, at most 63 actions; (b) `bigint` now, a second column when needed. **Recommendation:** (a), with a registry test that fails at 63.

### OQ-28: A token's effective rights

**Decided:** (a): scopes intersected with the person's `device_access` and role at request time ([decision](decisions/20261119-token-effective-rights.md)).

### OQ-29: Repeated commands and the error body

The plan has an idempotency key; it does not say what a repeat returns or what an error body holds besides the code. Options: (a) a repeat returns the first record, the error body is `code` plus `text`; (b) a repeat is refused, the body is `code` only. **Recommendation:** (a).

### OQ-30: Origin check on state-changing requests

Browser sessions use a cookie. Options: (a) check `Origin` on every state-changing request; (b) SameSite cookie only. **Recommendation:** (a) plus SameSite; no setting (see prusa-connect PR #15).

### OQ-31: Log redaction and URLs a device hands the hub

Options: (a) the request log redacts `token`, `api_key`, `password`, `fingerprint` and URL credentials, and the hub never fetches a URL a camera or printer hands it that points elsewhere; (b) decide per link when built. **Recommendation:** (a).

### OQ-32: How retention runs

Retention periods are decided ([decision](decisions/20261118-retention-defaults.md)); the mechanism is not. Options: (a) TimescaleDB retention and compression policies; (b) a hub task that deletes by age. **Recommendation:** (a) for samples and aggregates, (b) for events and records that are not hypertables.

### OQ-33: Restoring TimescaleDB without a superuser

Options: (a) a template database with the extension, owned by `maker_connect_owner`, restored into with `--role`; (b) the operator restores with a superuser. **Recommendation:** (a), proven in the store CI job first ([R-4](#risks)).

### OQ-34: The order of the host-side checks

The plan lists argument validation, state guard, authz, the laser rule (plan §2.1), without an order. Options: (a) capability, arguments, state, authz, person-only; (b) authz first, so a caller without the right learns nothing about the device's state. **Decided:** (b) ([decision](decisions/20261119-host-checks-order.md)); [device-contract.md](device-contract.md#host-side-rules) and [flows.md](flows.md#3-a-command-click-to-settled) draw it.

### OQ-35: Source modules per architecture block

**Decided:** (a) first: `crates/hub/tests/blocks.map`, read by `architecture_dependencies_hold`; the move to one module per block happens inside the device, authz and history steps ([decision](decisions/20261119-block-map-file.md)).

### OQ-36: The mode of the agent's credentials file

`maker-connect-agent.json` holds the agent token in the clear and is written with the default mode
(`crates/agent/src/link.rs`); the design keeps it "as today" (agent-engine.md O7). **Recommendation:**
0600, as the hub's own secrets file ([storage-ops.md](storage-ops.md#what-is-kept-where)).

### OQ-37: Re-pairing a device that has a row

A reinstalled agent, or a device moved to another agent, registers a device that already has a
`devices` row (same identity, [OQ-15](#oq-15-device-identity-across-replugs-and-agents)). Options:
(a) the pairing page offers "this is MINI-1 again" and the row gets the new `agent_id`, keeping its
history and grants; (b) always a new row, the old one retired by hand. **Recommendation:** (a), by the
person who pairs, never automatically.

### OQ-38: A person-only laser console

Raw G-code is not offered on a laser ([decision](decisions/20261119-no-raw-gcode-on-a-laser.md)).
Options: (a) none; (b) a console verb that is person-only like `mc.laser.start`, needing a frame and
PPE first, and refusing `M3`, `M4`, `S` words. **Recommendation:** (a); (b) adds our own filter on the
wire, which the house rule forbids.

### OQ-39: Roles above one's own, and the last owner

[Decision](decisions/20261119-no-self-elevation.md) stops a role above the giver's own. Open: may an
admin change or remove a member whose role is above theirs; may anyone lower their own role; what
happens when the last owner would leave or be lowered. **Recommendation:** no, no, and the last owner is
kept (`authz/LAST_OWNER`); the owner decides.

### OQ-40: The pairing code's strength, expiry and attempt limit

`claim_registration(code)` is reachable from every space of the hub, so the code must not be
guessable within its life. Not stated: the code's alphabet and length, how long a registration stays
open (`device_registrations.expires_at`), and whether claims of unknown codes are counted and slowed.
**Recommendation:** for a Prusa printer exactly what Buddy expects and Prusa Connect issues (read in
#18, as OQ-12); for an agent today's code as the agent registers it (agent-engine.md O7), an expiry
of minutes, and the sign-in failure counter applied to unknown codes per caller; decided in the
device step.

## Issues the design cannot place yet

| Issue | Why |
|---|---|
| #9 Klipper | The SimplyPrint/Moonraker route is parked (plan §3); no link, device kind or table in v2 until a Moonraker mapping after their example |
| #4 MK3 family | Parked: the MK3 joins over `/p/ws` once PrusaLink gets WebSocket support (prusa3d/prusa-connect#9); it would then be the `prusa` kind |
| #12 multi-device agent page | The agent side is placed (one session per device); the page is [OQ-20](#oq-20-the-agents-management-page) |
| #10, #6 (tables) | Placed as pages and endpoints; their tables are [OQ-14](#oq-14-tables-of-materials-items-and-labels) |

## Upstream changes that move a reference or inform the design

We follow each reference at a pinned commit; an unmerged upstream pull request is not the reference.

| Upstream | State | Effect on us |
|---|---|---|
| prusa3d/prusa-connect#37, PR #38: readings declared by the driver | open | informs [OQ-23](#oq-23-readings-declared-by-the-driver) |
| prusa3d/prusa-connect#26, prusa3d/Prusa-Firmware-Buddy#5631: Connect hostname 20 bytes, raised to 35 | open | our #22: the limit belongs to each dialect's baseline; 20 for 6.3.0 until a pinned Buddy changes it |
| prusa3d/prusa-connect#29: path prefix | open | [OQ-24](#oq-24-serving-under-a-path-prefix) |
| prusa3d/prusa-connect#24, #27: upload from PrusaSlicer | open | [OQ-2](#oq-2-the-3d-printer-rest-surface) |
| prusa3d/prusa-connect#8: print queue as an extension | open | [OQ-2](#oq-2-the-3d-printer-rest-surface) |
| prusa3d/prusa-connect#9: MK3S over the WebSocket | open | #4 parked |
| prusa3d/prusa-connect#10: material import/export | open | none: ours is Spoolman-compatible |
| prusa3d/prusa-connect#13, #32, PR #41: Buddy3D camera, RTSP from a container (WebRTC ICE) | open | confirms our MSE-over-WebSocket choice (no ICE); Buddy3D onboarding later |
| prusa3d/prusa-connect#36: OIDC in settings | open | `identities (provider, subject)` already allows OIDC later |
| prusa3d/prusa-connect PR #15: allowed origins in settings | open | relates to [OQ-30](#oq-30-origin-check-on-state-changing-requests) |
| prusa3d/Prusa-Link PR #1049: SET_VALUE for speed, flow, temperatures | open | would change the MK3 reference (#4) when merged and pinned; Buddy 6.3.0's SET_VALUE does not cover these (hub-side.md) |
| prusa3d/Prusa-Link PRs #1056, #1058, #1059, #1060; SDK 0.9 lazy file protocol | open | the MK3 port's pin will move; informs [OQ-21](#oq-21-a-printers-storage-listing) |
| prusa3d/Prusa-Link#1000: no TRANSFER_FINISHED after a transfer | open | a reference quirk: a port follows its pinned reference as it is |
| prusa3d/Prusa-Link#1002 WebSocket, #154 multiple instances, #1039 and #943 systemd user units | open | #154 is our #12's question on their side; our agents run as `systemd --user` units; the hub is a system unit ([architecture.md](architecture.md#deployment)) |
| prusa3d/Prusa-Link PR #1062: Huffman tables for UVC MJPEG | open | not our reference: FFmpeg's `mjpeg2jpeg` is ([cameras.md](cameras.md)) |
| prusa3d/Prusa-Error-Codes PRs #203, #195, #98 | open | [OQ-25](#oq-25-showing-prusa-error-codes) |
| StitchFoundry/laserfoundry PRs #137, #138 | merged | our laser reference stays LaserFoundry v0.5.0 `61febdb` until re-pinned; the `maker-connect` fork follows upstream by sync merges |
