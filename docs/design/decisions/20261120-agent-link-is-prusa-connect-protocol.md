# The agent link is the Prusa Connect printer protocol

Date: 2026-11-20  
Status: accepted; supersedes [the agent link over one WebSocket](20261119-agent-link-one-websocket.md) and
[agent identity and delivery](20261119-agent-link-identity-and-delivery.md); decides OQ-43 and OQ-45; supersedes OQ-11 and OQ-18  
Decided by: owner (2026-11-19, relayed by the coordinator, handoff:V2-DEVICES:001): the agent↔hub link for
every device follows the Prusa Connect printer protocol to the letter; the pinned commit and everything
below by the architect from the references (lane V2-AGENT). Items marked **ours** wait for the owner's approval.

## Context

The agent link was our own protocol "modelled on" Buddy and Prusa Connect's `/p/ws`: our path, our
subprotocol, our JSON messages, an agent token, a stream with a sequence and an outbox
([wire.md](../wire.md) before this decision). The owner ruled that this is not wanted: every device on the
agent (Prusa, Marlin, lasers, DYMO) talks to the hub exactly as a Prusa Connect printer talks to Prusa
Connect. The client is Prusa-Connect-SDK-Printer, which the agent ports; the server's answers are read
from `reference/prusa-connect` `internal/connector/xbuddy` (OCL: read only, nothing copied). The hub side
of `/p/register` and `/p/ws` was built in PR #40.

Two SDK commits were in use. PR #40's proof ran the SDK at `feat/ws-connect-local` @ `04c5bd59`
(`crates/hub/src/devices/prusa_connect.rs:17`, `scripts/prusa-sdk/link.py:3`); the MK3 decision pinned the
`websockets` branch @ `c0d7d233`.

## Decision

- **The reference, one commit each:**
  - Client: Prusa-Connect-SDK-Printer, branch `websockets`, at `c0d7d233d4ce2081e6c020d2e98fb92ad9b51521`
    (2026-11-03). PrusaLink takes the SDK `@websockets` (Prusa-Link `171148ce` `requirements.txt:1`, also
    `.github/workflows/python-tests.yml:51`, `README.md:68`), and that branch's head is `c0d7d233`
    (`git ls-remote` of prusa3d/Prusa-Connect-SDK-Printer, 2026-11-20: `refs/heads/websockets`). The
    branch `feat/ws-connect-local` (`04c5bd59`, 4 commits on top) is not pinned by PrusaLink, so it is
    not the reference; it becomes the reference only when PrusaLink pins it, as the MK3 decision already
    says ([decision](20261119-mk3-reference-prusalink-websockets.md), its "These are not the reference").
  - How a program uses the client (settings file, registration from a page, token store, reset):
    PrusaLink, branch `websockets`, at `171148ce7dbc7f1fb0633ea519523f3258877e2e`.
  - Server answers: `reference/prusa-connect` at `2e8c62b`, `internal/connector/xbuddy`.
- **One printer per device.** Each device on the agent is one SDK `Printer` with its own serial,
  fingerprint, type, registration, token and `/p/ws`, as PrusaLink runs one instance with one settings
  file per printer (`prusa/link/multi_instance/config_component.py:145-175`,
  `prusa/link/multi_instance/const.py:30`). The agent itself has no identity on the wire: no agent token,
  no agent registration, no stream.
- **Nothing of our own on the wire.** Every message, path, header and timing is the SDK's (client) or
  xbuddy's (server), with file and line in [wire.md](../wire.md). Our old messages and routes are removed
  (listed in wire.md, "Removed").
- **The step "a v2 agent without drivers"** carries the client up to the SDK's `Printer`: register,
  poll, token store, `/p/ws` with its headers, ping, the frames both ways, the command lifecycle and the
  SDK's own SEND_INFO answer, plus the settings page. Where a device's serial, fingerprint and type come
  from, its telemetry pace, its commands and files are each port's, designed with that driver.
- **Delivery is the SDK's**, not ours: no outbox, no sequence, no acknowledgement. While the socket is
  down the SDK drops telemetry and events (`prusa/connect/printer/__init__.py:806-812`); the server keeps
  the session and drains queued commands when the printer reattaches (`xbuddy.go:793-804`).

## What this removes from earlier decisions

- [The agent link over one WebSocket](20261119-agent-link-one-websocket.md): our path, subprotocol and
  JSON; the 15 s agent ping (that was Buddy's, not the SDK's 30 s).
- [Agent identity and delivery](20261119-agent-link-identity-and-delivery.md): the agent credential
  (`agents` as the link's identity), stream and cursor, `409 agent/STREAM_IN_USE`, taken command ids on
  disk, the up sequence. The late result on a command record is a hub rule on `commands`
  ([database.md](../database.md#devices-and-commands)) and stays.
- [The agent's update is the instance admin's](20261119-agent-update-by-instance-admin.md): who may update
  stands; the `update` message and `/api/agent/v2/update/{tag}/{file}` are gone, so how the update reaches
  the agent is open ([OQ-46](../risks-and-open.md#oq-46-how-the-agents-own-update-reaches-it)).

## Open

[OQ-44](../risks-and-open.md#oq-44-the-agent-settings-page-open-points) (the page),
[OQ-46](../risks-and-open.md#oq-46-how-the-agents-own-update-reaches-it) (the agent's update),
[OQ-47](../risks-and-open.md#oq-47-the-info-probe-before-registering) (`/info`),
[OQ-48](../risks-and-open.md#oq-48-what-a-device-that-is-not-a-prusa-says-it-is) (a non-Prusa device's type, for the driver steps),
[OQ-49](../risks-and-open.md#oq-49-registering-in-an-agent-without-drivers) (registering without drivers).
The hub's tables shaped by the superseded identity decision (`agents`, `devices.agent_id`, `up_seq`,
`stream`) are reshaped in the build step, not here.

## Where it shows

[wire.md](../wire.md), [links.md](../links.md) (L6-L9), [flows.md](../flows.md#1-pairing-a-device-on-an-agent),
[architecture.md](../architecture.md), [processes.md](../processes.md#inside-the-agent),
[risks-and-open.md](../risks-and-open.md).
