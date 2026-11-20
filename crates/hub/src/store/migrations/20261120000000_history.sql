-- A machine's history on TimescaleDB (design database.md "Events and history"): narrow samples, one
-- row per value, compressed after 7 days and kept 30; aggregates per minute (1 year) and per hour
-- (forever) (decisions/20261118-retention-defaults.md). TimescaleDB refuses compression and continuous
-- aggregates on a table with row-level security, so the space wall is a view: the app role reads
-- only the *_in_space views (`WHERE space_id = app_space()`), never the tables
-- (decisions/20261120-history-views-not-rls.md). The history writer writes on the admin pool (#4).

-- The registry's metrics (crates/device registry.rs METRICS), ids in its order from 1; the hub maps
-- ids by the registry (crate::store::history), a test checks both agree.
CREATE TABLE metric (
    id smallint PRIMARY KEY,
    name text NOT NULL UNIQUE,
    unit text NOT NULL
);
INSERT INTO metric (id, name, unit) VALUES
    (1, 'temp_nozzle', 'celsius'),
    (2, 'target_nozzle', 'celsius'),
    (3, 'temp_bed', 'celsius'),
    (4, 'target_bed', 'celsius'),
    (5, 'fan_print', 'rpm'),
    (6, 'fan_hotend', 'rpm');

-- No key to the machine: a sample stays where it happened (database.md, moving a device).
CREATE TABLE sample (
    time timestamptz NOT NULL,
    space_id bigint NOT NULL,
    device_id text NOT NULL,
    metric smallint NOT NULL,
    value double precision NOT NULL
);
SELECT create_hypertable('sample', by_range('time', INTERVAL '1 day'));
CREATE INDEX sample_by_device ON sample (device_id, metric, time DESC);
ALTER TABLE sample SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'space_id, device_id, metric',
    timescaledb.compress_orderby = 'time DESC'
);
SELECT add_compression_policy('sample', INTERVAL '7 days');
SELECT add_retention_policy('sample', INTERVAL '30 days');

-- Real time (materialized_only false): the newest buckets come from `sample` until they are made.
CREATE MATERIALIZED VIEW sample_1m WITH (timescaledb.continuous, timescaledb.materialized_only = false) AS
    SELECT time_bucket(INTERVAL '1 minute', time) AS bucket, space_id, device_id, metric,
           avg(value) AS value, min(value) AS low, max(value) AS high
    FROM sample
    GROUP BY bucket, space_id, device_id, metric
    WITH NO DATA;
SELECT add_continuous_aggregate_policy('sample_1m',
    start_offset => INTERVAL '1 hour', end_offset => INTERVAL '1 minute', schedule_interval => INTERVAL '1 minute');
SELECT add_retention_policy('sample_1m', INTERVAL '1 year');

CREATE MATERIALIZED VIEW sample_1h WITH (timescaledb.continuous, timescaledb.materialized_only = false) AS
    SELECT time_bucket(INTERVAL '1 hour', bucket) AS bucket, space_id, device_id, metric,
           avg(value) AS value, min(low) AS low, max(high) AS high
    FROM sample_1m
    GROUP BY 1, space_id, device_id, metric
    WITH NO DATA;
SELECT add_continuous_aggregate_policy('sample_1h',
    start_offset => INTERVAL '3 hours', end_offset => INTERVAL '1 hour', schedule_interval => INTERVAL '1 hour');

-- Where a machine's history has a hole, and why (hub restart, agent offline).
CREATE TABLE gap (
    space_id bigint NOT NULL,
    device_id text NOT NULL,
    from_time timestamptz NOT NULL,
    to_time timestamptz NOT NULL,
    why text NOT NULL
);
CREATE INDEX gap_by_device ON gap (device_id, from_time);

-- The app role's only way in. security_barrier: no function of the caller's sees a row first.
CREATE VIEW sample_in_space WITH (security_barrier) AS
    SELECT time, device_id, metric, value FROM sample WHERE space_id = app_space();
CREATE VIEW sample_1m_in_space WITH (security_barrier) AS
    SELECT bucket, device_id, metric, value, low, high FROM sample_1m WHERE space_id = app_space();
CREATE VIEW sample_1h_in_space WITH (security_barrier) AS
    SELECT bucket, device_id, metric, value, low, high FROM sample_1h WHERE space_id = app_space();
CREATE VIEW gap_in_space WITH (security_barrier) AS
    SELECT device_id, from_time, to_time, why FROM gap WHERE space_id = app_space();
GRANT SELECT ON sample_in_space, sample_1m_in_space, sample_1h_in_space, gap_in_space TO maker_connect_app;
