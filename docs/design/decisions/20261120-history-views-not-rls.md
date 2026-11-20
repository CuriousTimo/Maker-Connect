# History: the space wall is a view, not row-level security

Date: 2026-11-20  
Status: accepted; amends [20261119-history-tenancy.md](20261119-history-tenancy.md)  
Decided by: main session, on the owner's "nu meteen doen" (build TimescaleDB history now)

## Context

The history tenancy decision put row-level security on `sample` and `gap`, with compression and
continuous aggregates on `sample`. TimescaleDB 2.32 refuses that combination, run against the dev
database (`timescale/timescaledb:2.32.0-pg17`):

```text
ALTER TABLE sample SET (timescaledb.compress, ...)  ERROR:  columnstore cannot be used on table with row security
CREATE MATERIALIZED VIEW sample_1m WITH (timescaledb.continuous) ...  ERROR:  cannot create continuous aggregate on hypertable with row security
```

## Decision

- `sample`, `sample_1m`, `sample_1h` and `gap` carry `space_id` as decided, but have no row-level
  security. The app role (`maker_connect_app`) has no grant on them.
- The app role reads them only through views owned by the owner role, with `security_barrier`, that
  keep `WHERE space_id = app_space()`: `sample_in_space`, `sample_1m_in_space`, `sample_1h_in_space`,
  `gap_in_space`. Without a space in the transaction `app_space()` is NULL and no row comes through,
  as with the policies.
- Writes stay on the admin pool (allowlist #4, the history writer).
- `metric` is the registry (crates/device `METRICS`), its ids fixed in the migration; a test checks
  both agree. The app role needs no grant on it: the hub maps ids by the registry.

Proven on the dev database: the app role reads 0 rows without a space, only its space's rows with one,
and `permission denied for table sample` when it reads the table itself.

## Where it shows

[database.md](../database.md#events-and-history), `crates/hub/src/store/migrations/20261120000000_history.sql`.
