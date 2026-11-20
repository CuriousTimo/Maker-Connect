# Flows

The main flows end to end. Links (L1…) are in [links.md](links.md), endpoints in [api.md](api.md),
agent messages in [wire.md](wire.md). Every hub step that touches tenant rows runs in a transaction
with `SET LOCAL app.space_id`; a lookup before the space is known runs on the admin pool
([database.md](database.md#roles-and-pools)).

## 1. Pairing a device on an agent

Each device on an agent registers and pairs on its own, as a Prusa Connect printer does
([wire.md](wire.md#registration-http), [decision](decisions/20261120-agent-link-is-prusa-connect-protocol.md)):
the agent's `Printer` for that device posts `/p/register` and polls it with the code; a person types the
code shown on the agent's settings page into Space; the poll then carries the token, which the agent
writes into the device's settings file. The hub's side is the one built for Prusa printers. A device that
already has a row is paired again by the person, who picks "this is MINI-1 again": the row keeps its
history and grants, never automatically ([decision](decisions/20261119-re-pairing-by-the-person.md)).

```mermaid
sequenceDiagram
  participant A as Agent: Printer of one device
  participant H as Hub
  participant D as Database
  participant P as Person in Space
  A->>H: POST /p/register, sn, fingerprint, printer_type, firmware
  H->>D: admin pool: insert device_registrations, code, expires
  H-->>A: 200, header Code
  A->>A: settings page shows the code, status IN_PROGRESS
  loop every 1 s, at most 30 min
    A->>H: GET /p/register, header Code
    H-->>A: 202
  end
  P->>H: POST /api/v1/devices/pair, code and name
  H->>D: space tx: claim_registration(code) locks and approves it into this space, insert devices, event device.added
  H-->>P: 201 the device
  A->>H: GET /p/register, header Code
  H-->>A: 200, header Token
  A->>A: token into the device's prusa_printer_settings.ini, status FINISHED
  A->>H: GET /p/ws, Fingerprint and Token headers
  H->>D: admin pool: the device by its credentials, registration spent at the token's first use
  H-->>A: 101
  H-->>A: J frame SEND_INFO
  A->>H: ACCEPTED, then INFO
```

A Prusa printer (Buddy 6.4+, `/p/register`, `/p/auth`, `/p/ws`) is paired in Space by the code on its
screen the same way; its flow is drawn with the Buddy driver (V2-DEVICES step 3). The token's plaintext
is never stored by the hub: a device that collects again gets it from the hub process that minted it, and
after a hub restart a freshly minted one, as prusa-connect does (`internal/connector/xbuddy/registration.go:114-118`,
`:624-638`; [decision](decisions/20261119-prusa-token-never-stored.md)).

## 2. A command: click to settled

```mermaid
sequenceDiagram
  participant B as Browser
  participant H as Hub
  participant D as Database
  participant A as Agent
  participant X as Port
  B->>H: POST /api/v1/devices/{id}/commands, verb, args, idempotency key
  H->>H: route guard: the verb's action on the device, person-only from the person's own session
  H->>D: space tx: insert commands, state requested
  H->>H: host-side checks: set, args, state (order decided, OQ-34)
  alt refused by a host-side check
    H->>D: settle refused with fault code, event command.settled
    H-->>B: 409 or 400 with code domain/REASON
  else authorized
    H->>D: state authorized by grant, then sent
    H-->>B: 202 the command record
    H->>A: /p/ws: J or G frame, the command's 32-bit id
    A->>H: /p/ws: event ACCEPTED with that id
    A->>X: the verb, in order
    X-->>A: result: ok, refused, failed, with the device's reason
    A->>H: /p/ws: event FINISHED, REJECTED or FAILED with that id
    H->>D: space tx: settle the record, event command.settled
    H-->>B: SSE: command.settled
  end
```

A hub restart turns every open record into `outcome unknown`; nothing is re-sent (plan §2.5). The
link carries no outbox and no sequence: while a device's socket is down its `Printer` drops telemetry and
events (SDK `__init__.py:806-812`), the hub keeps the device's queue and sends it when the device
reattaches (`xbuddy.go:793-804`), and the same command id sent again is answered with its current or last
state, not run again (SDK `command.py:84-96`) ([wire.md](wire.md#the-websocket)). A result that arrives
after the record was settled `unknown` is kept as a late result and never acted on
([database.md](database.md#devices-and-commands)).

## 3. A job, with the laser rule

A job's start, pause and stop are command records (plan §2.5). The queue may start a job on a 3D
printer or a label printer; it never starts a laser. The job's state follows from those records and
the device's job end ([database.md](database.md#files-jobs-queue-and-farm)). A dispatcher never
starts what its person may not: a queue item is started on behalf of the person who queued it
(`machine_queue.by_user`), and the dispatcher checks that person's `job.start` on the device in the
space transaction before each start; without it the item keeps a `problem` and waits (`by_user` is
not null: every item has the person who queued it). The farm dispatcher starts
on behalf of the person who switched the farm on for that device (`devices.settings.farm_on_by`, set
with `device.manage` in the same write that switches the farm on, and cleared when it is switched off), with the same check and the same wait with a `problem` shown on the Now page.

```mermaid
sequenceDiagram
  participant B as Browser, a person
  participant Q as Queue dispatcher
  participant H as Hub
  participant A as Agent
  B->>H: POST /api/v1/queue, device and file: queue.manage and job.start on the device
  Q->>H: next item for a free device
  alt the device is a 3D printer or label printer
    H->>H: space tx: the item's person still holds job.start on the device, else the item gets a problem and waits
    H->>H: command job.start, origin queue, on the item's person's behalf
    H->>A: /p/ws: the job's commands, as the SDK carries them (designed with the first driver that takes a file)
    A->>A: hand the job to the port
    A->>H: /p/ws: telemetry and events
    H-->>B: SSE: job.started, job.ended
  else the device is a laser
    H->>H: no start: the queue skips lasers
  end
  Note over B,A: Laser: the job is sent and loaded at the laser
  B->>H: POST /api/v1/devices/{id}/jobs, load the file
  H->>A: /p/ws: the job, loaded
  B->>H: command mc.laser.frame
  B->>H: command mc.laser.set_origin
  B->>H: command mc.laser.frame
  B->>H: PPE confirmed in this session
  B->>H: command mc.laser.start from the person's own session
  H->>H: person-only, frame after last origin, PPE, grant mc.laser.start
  H->>A: /p/ws: the start command with its id
  A->>H: /p/ws: events
```

## 4. SSE resume (browser)

```mermaid
sequenceDiagram
  participant B as Browser
  participant H as Hub
  participant D as Database
  B->>H: GET /api/v1/events?types=..., header Last-Event-ID 1200
  H->>H: subscriber's actions from device_access, cached by authz_epoch
  alt 1200 older than the event retention
    H-->>B: event stream.reset
    B->>B: reload
  else
    H->>H: subscribe to the bus first, buffer what arrives
    H->>D: current state tree and state_rev of each visible device
    H-->>B: snapshot events, one per device, with rev
    H->>D: stored facts with seq above 1200, filtered
    H-->>B: each fact, id = seq
    H-->>B: buffered facts with seq above the last sent
    H-->>B: live facts and patches from the bus, a patch carries rev
  end
  Note over H,B: a grant or role change bumps authz_epoch and the filter refreshes. No membership left: stream.reset and the stream closes
```

## 5. Sign-in and invite

```mermaid
sequenceDiagram
  participant B as Browser
  participant H as Hub
  participant D as Database
  participant O as Admin in Space
  O->>H: POST /api/v1/spaces/{id}/invites, role, whole organisation or not
  H->>H: role at most the admin's own, whole organisation only with an org role
  H->>D: space tx: insert invites with token hash, expires, audit_log
  H-->>O: 201 the invite link, token shown once
  B->>H: GET /invite/{token}, the page
  H->>D: admin pool: invite by token hash, not used, not revoked, not expired
  alt not signed in
    B->>H: POST /api/v1/invites/accept, token, name, display name, password
    H->>D: admin pool, one tx: users, identities with Argon2id hash, memberships or org_memberships, invite used, sessions
    H-->>B: 201, Set-Cookie session, HttpOnly
  else signed in
    B->>H: POST /api/v1/invites/accept, token
    H->>D: admin pool, one tx: memberships or org_memberships, invite used, already a member: refused
    H-->>B: 201 the membership
  end
  Note over B,H: Later sign-ins
  B->>H: POST /api/v1/session, name and password
  H->>D: admin pool: user and identity by name
  H->>H: Argon2id verify, failure counter in memory
  H->>D: insert sessions
  H-->>B: Set-Cookie session
```

On a fresh database the hub logs a one-time setup code; `/setup` with that code makes the first
account, its organisation and space.

## 6. Self-update, dump and the way back

```mermaid
sequenceDiagram
  participant O as Old build
  participant R as Release host
  participant T as pg_dump and pg_restore
  participant N as New build on trial
  participant D as Database
  O->>R: release list, SHA256SUMS, SHA256SUMS.sig
  O->>O: Ed25519 over tag and sums holds, file sum listed
  O->>R: download the build
  O->>O: write and fsync the marker: phase trial, the dump's name
  O->>T: pg_dump -Fc, kind before-update
  T->>D: dump
  O->>O: abort own tasks, close pools
  O->>N: start as child, on trial
  N->>D: migrate, serve
  alt N confirms itself in time
    N-->>O: confirmed
    O->>O: the new build is started for good, the old kept as previous, marker removed last
  else N stops, hangs or is not confirmed
    O->>N: stop
    O->>O: marker: phase going_back
    O->>T: pg_restore into a fresh database
    T->>D: restore
    O->>D: rename live to _old, restored to live, drop _old
    O->>O: roll back the binary, remove the marker, say so on About
  end
  Note over O,D: at start, any build finding a marker with phase going_back restores first and never opens the live database
```

Changes made after the update started are lost on the way back
([decision](decisions/20261119-database-dumps.md)).

## 7. The nightly dump

```mermaid
sequenceDiagram
  participant H as Hub dumps task
  participant T as pg_dump
  participant F as Dumps folder
  loop every hour
    H->>F: newest nightly dump
    alt older than a day, or none
      H->>T: pg_dump -Fc, PGPASSWORD in the environment
      T->>F: temporary file, mode 0600
      H->>F: fsync, then rename to nightly-time.dump
      H->>F: delete all but the last 7 nightly
    end
  end
```
