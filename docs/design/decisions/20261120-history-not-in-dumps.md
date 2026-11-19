# History: not in the dumps; a restore makes it again, empty

Date: 2026-11-20  
Status: accepted; answers OQ-33 / R-4 for the history  
Decided by: main session, building TimescaleDB history on the owner's "nu meteen doen"; owner may overrule

## Context

The dumps (crate::backup) run and restore as `maker_connect_owner`, never a superuser, and leave the
TimescaleDB extension out (the restore template holds it). Restoring hypertables and continuous
aggregates writes TimescaleDB's own catalog (`_timescaledb_catalog`, `_timescaledb_config.bgw_job`)
and `_timescaledb_internal`, all the superuser's. Run on the dev database:

```text
pg_restore: ERROR:  permission denied for schema _timescaledb_internal
SELECT timescaledb_pre_restore();  ERROR:  permission denied to set parameter "timescaledb.restoring"
(with both granted) ERROR:  permission denied for table bgw_job / chunk / hypertable_id_seq / ...
```

Granting the owner write on that catalog is not an option: a `bgw_job` row names the role its job
runs as, so whoever writes it runs code as the superuser.

Raw samples are also large: 30 days of one sample per 2 s per value is tens of millions of rows for
a handful of machines, too much for a nightly dump on a Pi's SD card.

## Decision

- `pg_dump` leaves out `_timescaledb_internal` and the history's objects (`metric`, `sample`,
  `sample_1m`, `sample_1h`, `gap` and their `*_in_space` views; crate::backup `HISTORY_OBJECTS`).
- A restore makes them again, empty, from their one migration (`HISTORY_SQL`,
  `20261120000000_history.sql`), as the owner, after `pg_restore`. A later change to the history's
  objects changes that file, not a new migration.
- So a restored database (a failed update going back, a disaster restore) has its records, jobs and
  settings, and starts its graphs afresh. The update's trial copy has no history.

Proven by `backup::tests::a_dump_restores_into_a_copy_with_the_same_rows` (a sample before the dump;
the copy has `sample` as a hypertable, both aggregates, the 6 metrics, and no sample) and the update
tests, with the container's client tools (`MC_PG_COMMAND`).

## Open

Keeping the hour aggregates across a restore (small: one row per machine, metric and hour), by
`COPY` beside the dump: later, if wanted.

## Where it shows

crates/hub/src/backup.rs, [storage-ops.md](../storage-ops.md).
