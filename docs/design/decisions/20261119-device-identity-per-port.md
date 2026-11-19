# Device identity across replugs comes from the port's reference

Date: 2026-11-19  
Status: accepted; decides OQ-15  
Decided by: coordinator (journal:DESIGN-DOC:0037), taking the architect's recommendation; owner may overrule

## Context

`unit_uid` is in the identity; how a USB device or camera keeps its identity across replugs is not the
same for every kind (two C270s may share a serial).

## Decision

Each port fills `unit_uid` with what its reference reads to tell one unit from another, and nothing
else: no USB path, no guess. A port whose reference reads nothing leaves it empty; the person tells
such devices apart by the name given at pairing. Re-pairing a device that already has a row is
[OQ-37](../risks-and-open.md#oq-37-re-pairing-a-device-that-has-a-row).

## Where it shows

[device-contract.md](../device-contract.md#parts), the per-port audits.
