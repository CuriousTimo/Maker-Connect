# Database dumps: nightly, before an update, keep 7

Date: 2026-11-19  
Status: accepted (proxy)  
Decided by: owner-proxy (journal:V2-STORE-5:0021), owner to review

## Context

With Postgres the hub needs its own backups and a way back from a failed self-update.

## Decision

Nightly = once a day, checked hourly, no fixed time; a dump before each self-update, restored with the old binary on the way back (changes after the update started are lost); the last 7 of each kind kept, `--keep-dumps` changes it.

## Where it shows

[storage-ops.md](../storage-ops.md), [flows.md](../flows.md).
