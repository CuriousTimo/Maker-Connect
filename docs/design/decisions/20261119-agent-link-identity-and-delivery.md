# Agent identity and delivery on link v2

Date: 2026-11-19  
Status: superseded by [the agent link is the Prusa Connect printer protocol](20261120-agent-link-is-prusa-connect-protocol.md) (2026-11-20); the late result on `commands` stays as a hub rule  
Decided by: architect (journal:DESIGN-DOC:0024, review defects 4 and 5) and coordinator (journal:DESIGN-DOC:0037 D3) on the owner's "fix het"; owner may overrule

## Context

The wire said "the hub takes each seq once" but no table held the agent's seq, and credentials were per
device while the link is per agent. A reinstalled agent would restart at seq 1 and its items would be
dropped silently; a command could run twice; a result after `outcome unknown` was undefined.

## Decision

- **One agent credential** for the link: table `agents` (token hash, generation, fingerprint, info,
  `stream`, `up_seq`, `last_seen`), admin only; `devices.agent_id` for agent-hosted devices.
  `device_credentials` stays for hub-hosted devices (Prusa printers). Each device is registered and
  paired under the agent with its own code.
- **Stream and cursor.** The agent makes a random `stream` uuid with its outbox; every up body and the
  down query carry it. The hub keeps the highest applied seq per agent and stream; a new stream resets
  the cursor and settles the old stream's open records `unknown`. Items are applied idempotently by
  their own key (command id, job id, epoch, state rev); the cursor advances after the items' space
  transactions committed (allowlist #4).
- **Never twice.** The agent keeps the taken command ids with their results on disk until acknowledged
  and never runs a taken id twice. On a reconnect on the same stream the hub re-sends that stream's
  open records.
- **One stream at a time** (design review 2a2ec32, defect 7). A new stream is taken only while no down
  connection of the current stream is open; otherwise `409 agent/STREAM_IN_USE` and the agent retries
  after the keepalive interval. A reconnect on the same stream replaces a still-open connection
  without a reset. So two hosts holding one token cannot keep resetting each other and settling
  every space's open records `unknown`.
- **A late result** is stored as `late_outcome`, `late_reason`, `late_at` on the record, shown and never
  acted on; the outcome stays `unknown`.
- **An agent is a host, not a tenant row.** `agents` has no space; each device under it belongs to its
  own space, every item is applied in that device's space transaction, and the agent is sent only its
  own devices' commands. The space that claims the first registration is not special; a moved device
  keeps its agent.
- `devices.state_rev` goes up per applied patch batch and is carried by snapshot and patch events, so a
  client drops a patch older than its snapshot.

## Where it shows

[wire.md](../wire.md#shape), [database.md](../database.md#devices-and-commands),
[flows.md](../flows.md#1-pairing-a-device-on-an-agent), [architecture.md](../architecture.md#agent).
