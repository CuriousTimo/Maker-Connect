# The agent link over one WebSocket

Date: 2026-11-19  
Status: accepted; supersedes OQ-18 and the transport of [agent identity and delivery](20261119-agent-link-identity-and-delivery.md) (POST up, SSE down)  
Decided by: owner ("ja websocket, aparte PR", journal:DESIGN-WS:0001; no v1, journal:DESIGN-HOST:0006); the values taken from the references (15 s, 54 s, 60 s) owner may overrule

## Context

Link v2 was POST batches up and one SSE stream down per agent (plan §2.3). Buddy 6.4.0 and Prusa
Connect talk over one WebSocket per printer: registration over HTTP, then everything else, commands
included, on `/p/ws`. The owner chose that shape for the agent link.

What the references do (`reference/buddy-6.4.0` at `b91eeda`, `reference/prusa-connect` at `2e8c62b`):

- **Upgrade.** The printer sends `GET /p/ws` with `Fingerprint` and `Token`, `Upgrade: websocket`,
  `Sec-WebSocket-Key`, `Sec-WebSocket-Version: 13` and `Sec-WebSocket-Protocol: prusa-connect`
  (`src/connect/connect.cpp:151-189`). It takes only a `101` whose accept key matches and whose headers
  it supports; a `401` or `403` is an authentication error, anything else a server error
  (`connect.cpp:578-633`). The server checks the credentials before the upgrade and answers `401` or
  `403` (`internal/connector/xbuddy/xbuddy.go:695-709`) and offers only the `prusa-connect`
  subprotocol (`xbuddy.go:56-61`, `:388`).
- **Both ways on one socket.** The printer sends its telemetry and events as text messages on the
  socket (`connect.cpp:639-680`) and reads commands from it whenever they arrive (`connect.cpp:821-830`,
  `:360-557`).
- **Ping and pong.** The printer pings after 15 s of sending nothing (`connect.cpp:49-50`, `:828-829`,
  `:265-276`), answers a server ping with a pong (`connect.cpp:386-409`), ignores pongs
  (`connect.cpp:411-420`) and drops the connection on a close (`connect.cpp:421-426`). The server re-arms
  a read deadline on every frame, ping and pong included (`internal/connector/xbuddy/link.go:70-85`,
  `:241-249`), with a 60 s deadline and a ping every 54 s (`internal/connector/xbuddy/inbound.go:25-36`).
- **Reconnect.** A broken socket is thrown away and dialled again with a new upgrade
  (`connect.cpp:559-574`); after 10 s without a successful exchange the printer starts again with an
  INFO event (`src/connect/planner.cpp:109`, `:586-597`), and after a reconnect it sends fresh
  telemetry (`connect.cpp:832-837`, `src/connect/planner.hpp:247-251`). The server keeps the session
  across connections; a second dial replaces the older connection (`link.go:126-130`), and a reattach
  drains the commands queued while the printer was away (`xbuddy.go:793-795`).

## Decision

- **One WebSocket per agent** carries everything that was L6 and L7: upstream items and their
  acknowledgements, and downstream commands, jobs, answers and updates. Registration (L8) and file and
  update downloads (L9) stay HTTP, as Buddy registers over HTTP before it dials `/p/ws`.
- **No v1.** It replaces the v1 agent link (`/api/agent/v1/*` and the 2 s report loop) outright: no
  v1 fallback, no transition period, no old agents served (owner: "geen oude v1 meer. er is toch
  niets en niemand actief", journal:DESIGN-HOST:0006).
- **Ours, not Prusa's:** the path `GET /api/agent/v2/ws?stream=<uuid>`, the subprotocol name
  `maker-connect-agent.v2`, and the JSON of every message ([wire.md](../wire.md)). Only the shape is
  modelled on Buddy and Prusa Connect; the agent does not speak `prusa-connect`.
- **Authentication as before:** `Authorization: Bearer` with the agent token on the upgrade request,
  checked before the upgrade; a refusal is answered before the upgrade with the fault's HTTP status,
  as the reference server does.
- **Delivery unchanged:** each up item keeps its agent-wide `seq`; the hub answers with the highest
  applied seq; unacknowledged items stay in the on-disk outbox across reconnects and restarts; the
  stream uuid, the cursor `agents.up_seq`, one stream at a time (`409 agent/STREAM_IN_USE`, now on the
  upgrade), the taken command ids on disk and the late result stand as decided
  ([decision](20261119-agent-link-identity-and-delivery.md)).
- **Reconnect on the same stream:** a new connection replaces a still-open one without a reset, and
  the hub re-sends the open command records of that stream on it, as the reference server drains
  what was queued.
- **Ping and pong** as the references: the agent pings after 15 s of sending nothing and answers the
  hub's pings; the hub re-arms a read deadline on every frame (60 s) and pings every 54 s. A dead
  connection ends at the deadline; the agent dials again at its own retry pace (agent-engine.md O5).
- **The schema is generated** (`schemars` from `crates/protocol`), as before; wire.md's names are its
  starting point.
- **Browser live updates stay SSE** (`/api/v1/events`, plan §2.4); this decision is about the agent
  link only.

## Open

What the hub sends after a reconnect besides the open command records, whether the agent repeats
its hellos on a new connection, unreadable messages, message size and close codes:
[OQ-43](../risks-and-open.md#oq-43-the-agent-websocket-open-points).

## Where it shows

[wire.md](../wire.md), [links.md](../links.md) (L6, L7), [architecture.md](../architecture.md),
[processes.md](../processes.md#inside-the-agent), [flows.md](../flows.md), plan §2.3.
