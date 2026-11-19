# Restore order and dump durability

Date: 2026-11-19  
Status: accepted (orchestrator, owner may overrule)  
Decided by: orchestrator after review (journal:V2-STORE-5:0022); owner may overrule

## Context

PR #34 writes dumps and restores them. A correctness review (journal:V2-STORE-5:0022, :0023) found that a restore must never write over the database in use and that a dump must survive a crash.

## Decision

- A dump is written to a temporary name, fsynced, then renamed.
- A restore fills a fresh database first; only when that worked: live to `_old`, restored to live, then `_old` dropped.
- Renames end other sessions and retry for 30 s while the database is in use (`object_in_use`).

## Where it shows

[storage-ops.md](../storage-ops.md), [database dumps](20261119-database-dumps.md).
