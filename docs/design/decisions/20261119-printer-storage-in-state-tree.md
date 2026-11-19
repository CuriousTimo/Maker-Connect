# A printer's storage listing is in its state tree

Date: 2026-11-19  
Status: accepted; decides OQ-21 (a)  
Decided by: coordinator on the owner's delegation (journal:DESIGN-WS:0001); owner may overrule

## Context

What a printer's USB stick or SD card holds was a table; the plan did not place it (OQ-21).

## Decision

(a): part of the device's state tree (`/storage/...`), as the driver's reference reports it at its
pinned commit. No table of its own.

## Where it shows

[database.md](../database.md#files-jobs-queue-and-farm), [device-contract.md](../device-contract.md).
