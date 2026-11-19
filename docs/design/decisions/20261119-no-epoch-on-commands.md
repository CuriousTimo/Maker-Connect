# A command carries no capability epoch

Date: 2026-11-19  
Status: accepted; decides OQ-26  
Decided by: coordinator (journal:DESIGN-DOC:0037), taking the architect's recommendation; owner may overrule

## Context

The capability epoch changes when a device's enabled set changes (plan §2.1). Whether a command should
carry the epoch the UI saw was open.

## Decision

No epoch on a command. The host checks the verb against the device's current enabled set and refuses
with `device/CAPABILITY_NOT_SUPPORTED`; a page that is behind sees the refusal. Revisited only when a
stale-UI case shows up.

## Where it shows

[device-contract.md](../device-contract.md#parts), [wire.md](../wire.md).
