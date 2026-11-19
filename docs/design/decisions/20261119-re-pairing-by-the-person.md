# Re-pairing a device that has a row

Date: 2026-11-19  
Status: accepted; decides OQ-37 (a)  
Decided by: coordinator on the owner's delegation (journal:DESIGN-WS:0001); owner may overrule

## Context

A reinstalled agent, or a device moved to another agent, registers a device that already has a
`devices` row with the same identity (OQ-37, [decision](20261119-device-identity-per-port.md)).

## Decision

(a): the pairing page offers "this is MINI-1 again"; the row gets the new `agent_id` and keeps its
history and grants. Only by the person who pairs, never automatically.

## Where it shows

[flows.md](../flows.md#1-pairing-a-device-on-an-agent).
