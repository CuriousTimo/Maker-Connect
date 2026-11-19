# History samples carry the space

Date: 2026-11-19  
Status: accepted; decides OQ-22  
Decided by: coordinator (journal:DESIGN-DOC:0037), taking the architect's recommendation; owner may overrule

## Context

`sample` had no `space_id` in the plan, so RLS could not filter history: the second wall had a hole.

## Decision

`sample` and `gap` get `space_id`, RLS policy `space`, and the continuous aggregates group by it. The
history subscriber writes them on the admin pool (allowlist #4); every read runs in a space
transaction. `space_id` sits in the compressed segment, so it costs little.

## Where it shows

[database.md](../database.md#events-and-history).
