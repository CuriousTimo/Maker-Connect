# Flows

The main flows end to end. Links (L1…) are in [links.md](links.md), endpoints in [api.md](api.md),
agent messages in [wire.md](wire.md). Every hub step that touches tenant rows runs in a transaction
with `SET LOCAL app.space_id`; a lookup before the space is known runs on the admin pool
([database.md](database.md#roles-and-pools)).

## 1. Pairing a device on an agent

The agent registers once for its link and then each device it hosts; a person adds each device in
Space with the code the agent shows (registering as today, agent-engine.md O7; tables `agents`,
`device_registrations`, plan §2.7; [decision](decisions/20261119-agent-link-identity-and-delivery.md)).
The first registration carries no token and yields the agent token with the first device; every
further one carries the agent token and yields only the device. A device that already has a row is
paired again by the person, who picks "this is MINI-1 again": the row gets the new `agent_id` and keeps
its history and grants, never automatically ([decision](decisions/20261119-re-pairing-by-the-person.md)).

```mermaid
sequenceDiagram
  participant A as Agent
  participant H as Hub
  participant D as Database
  participant P as Person in Space
  A->>H: POST /api/agent/v2/register, identity and fingerprint
  H->>D: admin pool: insert device_registrations, code, secret hash, expires
  H-->>A: 201 registration id, pairing code, registration secret
  A->>A: show the pairing code
  loop until approved or expired
    A->>H: GET /api/agent/v2/register/{id} with the secret
    H-->>A: 202 waiting
  end
  P->>H: POST /api/v1/devices/pair, code and name
  H->>D: space tx: claim_registration(code) locks and approves it into this space, insert devices (agent_id from the registration, null for a first registration), event device.added
  H-->>P: 201 the device
  A->>H: GET /api/agent/v2/register/{id} with the secret
  H->>D: admin pool: registration by id and secret hash, its space
  H->>D: space tx: collect_registration(id, token hash) locks it, first registration inserts agents and sets devices.agent_id, collected
  H-->>A: 200 the agent token (first time only, shown once) and the device id
  A->>H: GET /api/agent/v2/ws?stream=…, WebSocket upgrade with the token
  H-->>A: 101, subprotocol maker-connect-agent.v2
  A->>H: up: hello with identity, capabilities, epoch, state set
  H->>D: admin pool: agent by token hash, registration spent at the token's first use
  Note over A,H: a further device: register with the agent token, a new code, pair, collect the device id
```

## 2. A Prusa printer registers (Buddy 6.3.0)

**Shelved:** v2 builds only the websocket dialect (Buddy 6.4+: `/p/register`, `/p/auth`, `/p/ws`;
[decision](decisions/20261119-buddy-websocket-dialect-only.md)); this flow and flow 5 are kept as the
record of the 6.3.0 dialect until the Buddy driver (V2-DEVICES step 3) draws the websocket one.
As Buddy 6.3.0's registrator does it (`reference/buddy-6.3.0` `src/connect`, audit
`docs/audits/buddy.md`). The token is kept in the registration until its first use, so a printer that
asks again gets the same token ([decision](decisions/20261119-prusa-token-until-spent.md)).

```mermaid
sequenceDiagram
  participant M as Printer, Buddy 6.3.0
  participant H as Hub, prusa-buddy driver
  participant D as Database
  participant P as Person in Space
  M->>H: POST /p/register, serial, fingerprint, printer type, firmware
  H->>D: admin pool: insert device_registrations
  H-->>M: 200 with header Code
  M->>M: show the code on its screen
  loop every few seconds, as Buddy does
    M->>H: GET /p/register with header Code
    H-->>M: 202 waiting
  end
  P->>H: POST /api/v1/devices/pair, code and name
  H->>D: space tx: claim_registration(code), insert devices kind prusa, dialect buddy-http, event device.added
  M->>H: GET /p/register with header Code
  H->>D: admin pool: registration by code, its space
  H->>D: space tx: collect_registration(id, token hash) locks it, insert device_credentials, collected
  H-->>M: 200 with header Token
  M->>H: POST /p/telemetry with headers Token and Fingerprint
  H->>D: admin pool: device by token hash, registration spent at the token's first use
  H->>D: space tx: state patches, last_seen
  H-->>M: 204, or a command as the answer
```

## 3. A command: click to settled

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
    H->>A: WebSocket: command with record id
    A->>X: the verb, in order
    X-->>A: result: ok, refused, failed, with the device's reason
    A->>H: WebSocket up: result, seq n
    H->>D: space tx: settle the record, event command.settled
    H-->>A: WebSocket: ack up to seq n
    H-->>B: SSE: command.settled
  end
```

A hub restart turns every open record into `outcome unknown`; nothing is re-sent (plan §2.5). An
agent that is offline keeps results in its outbox and sends them when it is back; the hub accepts
each seq once, per agent and stream, and applies each item by its own key, so a repeated batch
changes nothing. The agent keeps the ids of the commands it took, with their results, until the hub
acknowledged them, and never runs one twice; on a reconnect on the same stream the hub re-sends the
open records of that stream on the new WebSocket. A result that arrives after the record was settled `unknown` is kept
as a late result and never acted on ([wire.md](wire.md#shape)).

## 4. A job, with the laser rule

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
    H->>A: WebSocket: job with record id
    A->>H: GET /api/agent/v2/files/{id}, the whole file
    A->>A: hand the job to the port
    A->>H: WebSocket up: patches, then job end
    H-->>B: SSE: job.started, job.ended
  else the device is a laser
    H->>H: no start: the queue skips lasers
  end
  Note over B,A: Laser: the job is sent and loaded at the laser
  B->>H: POST /api/v1/devices/{id}/jobs, load the file
  H->>A: WebSocket: job, loaded
  B->>H: command mc.laser.frame
  B->>H: command mc.laser.set_origin
  B->>H: command mc.laser.frame
  B->>H: PPE confirmed in this session
  B->>H: command mc.laser.start from the person's own session
  H->>H: person-only, frame after last origin, PPE, grant mc.laser.start
  H->>A: WebSocket: command mc.laser.start with record id
  A->>H: WebSocket up: job end
```

## 5. A file to a printer

For an agent device the file is fetched whole before the job is handed on (agent-engine.md O6). For a
Prusa printer the file goes as Buddy's encrypted download; the transfer is a durable record like a
command (bytes, state, reason; plan §2.5).

```mermaid
sequenceDiagram
  participant B as Browser
  participant H as Hub
  participant D as Database
  participant M as Printer, Buddy 6.3.0
  B->>H: POST /api/v1/devices/{id}/transfers, file and path
  H->>D: space tx: insert transfers, state waiting, with iv and key
  M->>H: POST /p/telemetry
  H-->>M: answer: START_ENCRYPTED_DOWNLOAD with path, iv, key, size
  M->>H: GET /f/{iv}/raw with Range
  H-->>M: 206 AES-128-CTR bytes
  H->>D: transfer bytes and state
  M->>H: POST /p/events: transfer finished
  H->>D: space tx: transfer done, event transfer.settled
  H-->>B: SSE: transfer.settled
```

## 6. SSE resume (browser)

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

## 7. Sign-in and invite

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

## 8. Self-update, dump and the way back

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

## 9. The nightly dump

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
