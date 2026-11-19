# Composite foreign keys keep references inside the space

Date: 2026-11-19  
Status: accepted  
Decided by: architect (journal:DESIGN-DOC:0024, review defect 6), after the coordinator found the hole in #34 (a grant pointing at another space's machine); owner may overrule

## Context

A foreign key on `device_id` alone lets a row in space A reference a device of space B; RLS does not
check foreign keys, so a grant, a transfer or a camera could point across the tenant boundary.

## Decision

Every reference from a tenant row to another tenant row is a foreign key on `(space_id, parent_id)`
to the parent's `UNIQUE (space_id, id)`. The referenced tenant tables (`devices`, `device_groups`,
`files`, `commands`, `jobs`, `projects`, `parts`, `vendor`, `filament`, `spool`, `locations`, …) carry
that unique constraint. `jobs` and `machine_queue` keep no foreign key to `devices` or `files`,
because a job outlives both; they look the ids up in the space transaction before writing. A job is
itself referenced: `commands.job_id`, `transfers.job_id`, `part_ledger.job_id` and
`devices.current_job_id` are composite keys to `jobs`; `jobs.part_id` and `jobs.spool_id` are
composite keys with `ON DELETE SET NULL`, so a job survives its project and its spool. A table whose
ids people choose (`locations`) is keyed `(space_id, id)`, with its `parent` a composite key, so one
space's names never block or reveal another's. References to `users` (not a tenant table) are checked
through a membership in the same transaction. Moving a device ([decision](20261119-device-move.md))
deletes the old space's dependent rows first and refuses while the device has a job not ended, so no
`ON UPDATE` rule is needed.

## Where it shows

[database.md](../database.md) (conventions and every table's keys).
