# Agent link v2: the wire

Ours, not in the public spec (plan §2.3, §2.8). One WebSocket per agent, modelled on Buddy and Prusa
Connect's `/p/ws` ([decision](decisions/20261119-agent-link-one-websocket.md)). It replaces the v1
agent link (`/api/agent/v1/*` and the 2-second report loop) outright: no v1 fallback, no transition
period, no old agents served (owner, journal:DESIGN-HOST:0006). The path, the subprotocol name and the
JSON of every message below are ours; the generated schema (`schemars` from `crates/protocol`) fixes
them in the build, with these names as its start.

## Shape

```mermaid
sequenceDiagram
  participant A as Agent
  participant H as Hub
  A->>H: GET /api/agent/v2/ws?stream=uuid, Upgrade websocket, Sec-WebSocket-Protocol maker-connect-agent.v2, Bearer token
  H-->>A: 101 Switching Protocols, Sec-WebSocket-Protocol maker-connect-agent.v2
  H-->>A: open command records of this stream, re-sent
  A->>H: up, items with seq 41 to 44
  H-->>A: ack 44
  Note over A: items up to 44 leave the outbox
  H-->>A: command, job, answer, update, whenever one is due
  A->>H: up, item with seq 45
  Note over A,H: the connection drops before the ack: 45 stays in the outbox
  A->>H: GET /api/agent/v2/ws?stream=uuid, the same stream
  H-->>A: 101, then the open command records again
  A->>H: up, items with seq 45 and 46
  H-->>A: ack 46, 45 taken once
  Note over A,H: no frame sent for 15 s: the agent pings; the hub pings every 54 s, 60 s of silence ends the connection
```

- **One socket:** upstream items, their acknowledgements and every downstream message (commands, jobs,
  answers, updates) travel on the one WebSocket. Registration and file and update downloads stay HTTP
  (below), as Buddy registers over HTTP before it dials `/p/ws`
  (`reference/buddy-6.4.0` `src/connect/connect.cpp:151-189`, `src/connect/registrator.cpp`).
- **Upgrade:** `GET /api/agent/v2/ws?stream=<uuid>` with `Upgrade: websocket`,
  `Sec-WebSocket-Version: 13`, `Sec-WebSocket-Protocol: maker-connect-agent.v2` and
  `Authorization: Bearer <agent token>`. The hub checks the token before the upgrade and refuses with
  the fault's HTTP status ([api.md](api.md#conventions-apiv1)), as the reference server checks a
  printer's credentials before its upgrade (`reference/prusa-connect`
  `internal/connector/xbuddy/xbuddy.go:695-709`); the agent takes only a `101` that names the
  subprotocol, as Buddy does (`connect.cpp:592-617`).
- **Messages:** each WebSocket text message is one JSON object with a `type`. Buddy's own frames (a
  type letter and a hex command id before the body, `connect.cpp:363-466`) are not used.
- **Identity:** one agent token for the link (`agents`); the agent's devices are paired one by one
  under it, each with its own registration and code ([decision](decisions/20261119-agent-link-identity-and-delivery.md)).
- **Stream:** the agent makes a random `stream` uuid together with its outbox and sends it in the
  upgrade query. A new stream (a reinstalled agent, a lost outbox) resets the hub's cursor for that
  agent and settles the old stream's open command records as `outcome unknown`. The hub takes a new
  stream only while no connection of the current stream is open; otherwise it answers the upgrade
  with `409 agent/STREAM_IN_USE` and the agent dials again at its retry pace (agent-engine.md O5). So
  two hosts with one token cannot reset each other, and a clone of an agent is refused rather than
  let in. A reconnect **on the same stream** while the old connection is still open (a network drop
  the hub has not noticed) replaces the old connection without a reset, as prusa-connect replaces a
  printer's older connection (`internal/connector/xbuddy/link.go:126-130`): no record is settled by
  it ([decision](decisions/20261119-agent-link-identity-and-delivery.md)).
- **Upstream:** `up` messages. Each item has an agent-wide `seq`; the hub keeps the highest applied
  seq per agent and stream (`agents.up_seq`), answers with an `ack` carrying it, and takes each seq
  once. Items are applied idempotently by their own key (the command id, the job id, the capability
  epoch, the state rev); the ack is sent after the items' space transactions have committed.
  Unacknowledged items stay in the on-disk outbox across reconnects and restarts and are sent again
  on the next connection.
- **Downstream:** a command arrives at once on the socket. On every (re)connect on the same stream
  the hub re-sends the open command records it sent on this stream, as prusa-connect's reattach
  drains what was queued while a printer was away (`xbuddy.go:793-795`). The agent keeps an on-disk
  set of taken command ids with their results until acknowledged and never runs a taken id twice.
  What else is re-sent after a reconnect is [OQ-43](risks-and-open.md#oq-43-the-agent-websocket-open-points).
- **Ping and pong,** as the references: the agent sends a ping after 15 s of sending nothing and
  answers every ping with a pong (`connect.cpp:49-50`, `:828-829`, `:386-409`); the hub re-arms a
  60 s read deadline on every frame, ping and pong included, and pings every 54 s (`link.go:70-85`,
  `:241-249`; `internal/connector/xbuddy/inbound.go:25-36`). A close, a failed send or a passed
  deadline ends the connection; the agent dials again at its retry pace (O5), as Buddy throws a broken
  socket away and upgrades anew (`connect.cpp:559-574`).
- **A late result** (after the hub settled the record `unknown`) is stored on the record as
  `late_outcome`, `late_reason`, `late_at`: shown, never acted on; the outcome stays `unknown`.
- The agent token may only report and fetch for its own devices.

## Registration (HTTP)

| Message | Direction | Fields |
|---|---|---|
| `RegisterRequest` | agent → hub, `POST /api/agent/v2/register`; without a token the first time, with the agent token for every further device | `kind` string, `identity` Identity, `fingerprint` string, `agent` AgentInfo |
| `RegisterAnswer` | 201 | `registration_id` uuid, `code` string (the pairing code), `secret` string (once), `expires_at` time |
| `CollectAnswer` | `GET /api/agent/v2/register/{id}` with the secret | 202 while waiting; 200 once approved, once: `{token, device}` for the agent's first registration (the agent token, shown once), `{device}` for every further device; 410 expired |

## Agent to hub (`up`)

Message: `{"type": "up", "agent": AgentInfo, "items": [Item…]}`.

| Item `type` | Fields | When |
|---|---|---|
| `hello` | `seq`, `device` uuid, `identity` Identity, `capabilities` CapabilitySet, `state_set` StateSet | a session opens, or its set changes (new epoch) |
| `patch` | `seq`, `device`, `time`, `rev` integer (the device's state rev after these patches), `patches` [Patch] | the state tree changed |
| `result` | `seq`, `device`, `command` uuid (the hub's record id), `outcome` (`ok`, `refused`, `failed`), `fault` string or null, `reason` string or null, `output` string or null | a command settled at the device |
| `job_end` | `seq`, `device`, `job` uuid, `outcome` (`done`, `stopped`, `failed`), `reason` string or null, `lines` integer or null | a job ended |
| `offline` | `seq`, `device`, `why` string | the port says the device went away |
| `camera_frame` | open ([OQ-9](risks-and-open.md#oq-9-how-usb-camera-video-reaches-the-hub)) | — |

## Hub to agent

| Message `type` | Fields | Meaning |
|---|---|---|
| `ack` | `ack` integer | the highest seq applied on this stream; items up to it leave the outbox |
| `command` | `id` uuid (record id), `device`, `verb`, `args` object | run this verb; answer with a `result` |
| `job` | `id` uuid, `device`, `file` {`id`, `name`, `size`}, `from_line` integer or null | fetch the file whole (`GET /api/agent/v2/files/{id}`, at most 512 MB), then hand the job to the port; a laser job is loaded, not started |
| `answer` | `device`, `dialog` string, `button` string | a person's answer to a device's question |
| `update` | `tag` string | a signed agent release to take (`GET /api/agent/v2/update/{tag}/{file}`) |

## Files and updates (HTTP)

`GET /api/agent/v2/files/{id}` and `GET /api/agent/v2/update/{tag}/{file}`, with the agent token
(agent-engine.md O6, O7).

## Shared types

| Type | Fields |
|---|---|
| AgentInfo | `version` string, `os` string, `arch` string |
| Identity | `vendor`, `model`, `firmware_label`, `serial`, `unit_uid`: strings or null |
| CapabilitySet | `epoch` integer, `verbs` [Verb] |
| Verb | `name` string (registry), `args` [Arg] |
| Arg | `name`, `kind` (string, number, integer, boolean, opaque), `unit` or null, `bounds` or null, `required` boolean |
| StateSet | `states` [{`name`, `accepts` [verb names]}], `transitions` [{`from`, `to`, `verb`}] |
| Patch | `path` string (JSON pointer), `value` any JSON, `null` removes |

The agent's own values (report pace, retry, download progress, the update trial) are ours, as
approved for the agent (agent-engine.md O4-O7); what goes to a device is the port's alone.
