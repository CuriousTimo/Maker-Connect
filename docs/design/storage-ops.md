# Storage and operations

## What is kept where

| What | Where | Kept |
|---|---|---|
| Every table | PostgreSQL + TimescaleDB, one database | see retention |
| File bytes (uploads, job files) | the files folder, named by file id | while the `files` row exists |
| Dumps | the dumps folder beside the files folder (0700; each dump 0600: it holds password and token hashes) | the last 7 of each kind, `--keep-dumps N` |
| Agent and release downloads | the downloads folder | the current builds |
| Hub settings | the unit's environment: `DATABASE_URL` (0600 file), the flags | — |
| Agent outbox and state | beside the agent's config | until acknowledged |
| Update marker | `<exe>.update.json` beside the binary (`crates/update`) | while a trial or a way back is open ([decision](decisions/20261119-update-trial-marker.md)) |
| In memory, on purpose | PPE confirmations, framing and origin flags, sign-in failure counters, camera GOPs, per-subscriber action caches | lost on restart, by design |

The folders' names come from a flag; which flag names them is [OQ-19](risks-and-open.md#oq-19-the-flag-that-names-the-folders).

## Retention

[Decision](decisions/20261118-retention-defaults.md): raw samples 30 days; 1-minute aggregates 1 year;
1-hour aggregates forever; events 1 year; command records and the audit log forever. Space admins may
lower them per space later. How the deletions run is [OQ-32](risks-and-open.md#oq-32-how-retention-runs).

## History

Narrow samples (`time, device_id, metric, value`), the metric registry, continuous aggregates at 1 min
and 1 h, compression `segmentby device_id, metric` after 7 days, `gap` rows for hub restarts and
agents offline (plan §2.7). Load: 10 devices × 15 values every 2 s ≈ 75 rows/s, against ≈ 7,400 rows/s
measured on .67. The .67 trial is repeated narrow in step 3 (size after compression, 1 h buckets over
3 days) and its numbers recorded in the plan; wide-per-kind only if those numbers say so.

## Dumps

[Decision](decisions/20261119-database-dumps.md):

- **Nightly** means once a day: the hub checks every hour and dumps when the last nightly dump is a
  day old; there is no fixed time of day.
- **Before each self-update** the hub takes a dump.
- **Going back** to the old version restores the database from that dump along with the old binary;
  changes made after the update started are lost.
- The last 7 of each kind are kept (`--keep-dumps`).
- Custom format (`pg_dump -Fc`), written to a temporary name, fsynced, then renamed.
- A restore never writes over the database in use: it fills a fresh database, and only once that
  worked: live → `_old`, restored → live, then `_old` dropped; renames end other sessions and retry
  while the database is in use (30 s). These details are the code of PR #34 after a correctness review
  ([decision](decisions/20261119-restore-order.md)).
- The client tools are of the server's major version, on the hub's `PATH`; the hub has no Docker
  rights ([decision](decisions/20261119-pg-client-no-docker-rights.md)). The password goes in
  `PGPASSWORD`, never on a command line; an address is never printed with its password.
- Dumps and restores run as `maker_connect_owner` (`--role`), the role that owns every object
  ([database.md](database.md#roles-and-pools)).

## Self-update

`crates/update`, for hub and agent alike:

- **Signed.** A release's tag and `SHA256SUMS` are signed together (Ed25519 over
  `<tag>\n<SHA256SUMS>`); the private key is never in the repository. A file is taken only when the
  signature holds and its sum is listed.
- **Tried first.** The hub: dump, then the old build stops its own tasks and closes its pools, then
  starts the new build as its child, on trial, on the live database (which the new build migrates). The
  agent: the new build's `--version`, then a run confirmed once the hub answered it.
- **A way back.** A new build that stops, hangs or is not confirmed in time is stopped and rolled back,
  with the database restored from the before-update dump ([flows.md](flows.md#8-self-update-dump-and-the-way-back)).
  The About page says what happened.
- **The marker drives it** ([decision](decisions/20261119-update-trial-marker.md)). Before the dump the
  old build writes and fsyncs `<exe>.update.json` (`from`, `to`, `until`, `starts`, the dump's name,
  `phase: trial`). Going back sets `phase: going_back`. **No build opens the database while a marker
  says `going_back`:** it restores the named dump into a fresh database first, swaps the databases,
  then the binary, then removes the marker. The `from` build finding a marker at start does the same,
  so a power loss during the trial or the way back repeats the restore instead of starting the old
  build on a migrated database. A finished trial removes the marker last.

## Deployment: the Pi (.67, armhf, 1 GB)

[Decision](decisions/20261118-hub-host-pi-and-apt-hosts.md). How-to: [pi-hub.md](../pi-hub.md).

| Part | Setting |
|---|---|
| Database | Docker `timescale/timescaledb:2.32.0-pg17` (arm/v7), `--memory 384m`, `shared_buffers=64MB`, `work_mem=4MB`, `max_connections=20`, on `127.0.0.1` or a Unix socket only, named volume on disk, `--restart unless-stopped`, confined by AppArmor `docker-default` (never `apparmor=unconfined`) |
| Client tools | PostgreSQL 17 `pg_dump`, `pg_restore`, `psql` built from source into the hub's folder |
| Hub | native binary, system unit, `User=maker-connect` (own system user, not in `docker`), `EnvironmentFile` with `DATABASE_URL` (0600) ([architecture.md](architecture.md#deployment)) |
| Preferred makerspace host | arm64/amd64 with TimescaleDB's apt packages, no Docker |

## Provisioning the database

Done once by the operator as the database superuser, never by the hub (the hub's login cannot):

1. Create the roles of [database.md](database.md#roles-and-pools): `maker_connect_owner` (NOLOGIN,
   CREATEDB), `maker_connect_app` (NOLOGIN), `maker_connect_admin` (NOLOGIN, BYPASSRLS), and the login
   `maker_connect_hub` (NOINHERIT, member of the three with SET and without INHERIT).
2. Create the database owned by `maker_connect_owner`, and in it `CREATE EXTENSION timescaledb`.
3. (Architect's proposal, [OQ-33](risks-and-open.md#oq-33-restoring-timescaledb-without-a-superuser)) Make a template database with the extension, owned by `maker_connect_owner`, for restores into a
   fresh database without a superuser.
4. Put the login's address in `DATABASE_URL`.

Whether a TimescaleDB restore works end to end without a superuser this way is unproven:
[R-4](risks-and-open.md#risks).
