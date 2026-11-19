-- A machine's queue (crate::queue): today's table on Postgres (hub-core-v2.md §3). Times are timestamptz
-- through at_unix()/unix_seconds(); `pos` a double, as SQLite's REAL. space_id with row-level security on
-- app_space(). Ids as SQLite's rowid gave them (next_rowid, stock/migrations): people see and send them.

CREATE OR REPLACE FUNCTION next_rowid(t text) RETURNS bigint LANGUAGE plpgsql VOLATILE SECURITY DEFINER
    SET search_path = public AS $$
DECLARE
    n bigint;
BEGIN
    IF t NOT IN ('vendor', 'filament', 'spool', 'tag', 'machine_queue') THEN
        RAISE EXCEPTION 'next_rowid: not for %', t;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtext('next_rowid:' || t)::bigint);
    EXECUTE format('SELECT COALESCE(max(id), 0) + 1 FROM %I', t) INTO n;
    RETURN n;
END $$;

CREATE TABLE machine_queue (
    id bigint PRIMARY KEY DEFAULT next_rowid('machine_queue'),
    space_id bigint NOT NULL REFERENCES spaces (id),
    machine_id text NOT NULL,
    file_id text NOT NULL,
    file_name text NOT NULL,
    by_name text,
    pos double precision NOT NULL,
    added_at timestamptz NOT NULL,
    -- Why it cannot start (the file no longer suits the machine); empty: it can.
    problem text,
    -- Who queued it, as a person: they may move and take out their own.
    by_user bigint
);
CREATE INDEX machine_queue_by_machine ON machine_queue (space_id, machine_id, pos, id);

ALTER TABLE machine_queue ENABLE ROW LEVEL SECURITY;
CREATE POLICY in_space ON machine_queue USING (space_id = app_space());
