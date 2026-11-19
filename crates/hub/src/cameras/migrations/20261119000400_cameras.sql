-- The cameras of a space (crate::cameras, feature `cameras`): today's table on Postgres (hub-core-v2.md
-- §3). In every build, with the feature or without: one database, whichever build opens it, has the same
-- migrations. Times are timestamptz through at_unix()/unix_seconds(); space_id with row-level security
-- on app_space(). Ids as SQLite's rowid gave them (next_rowid, stock/migrations): a camera's page has its
-- id. The password is kept as the machines' tokens are (plain, owner 2026-11-16).

CREATE OR REPLACE FUNCTION next_rowid(t text) RETURNS bigint LANGUAGE plpgsql VOLATILE SECURITY DEFINER
    SET search_path = public AS $$
DECLARE
    n bigint;
BEGIN
    IF t NOT IN ('vendor', 'filament', 'spool', 'tag', 'machine_queue', 'projects', 'parts', 'part_files',
                 'cameras')
    THEN
        RAISE EXCEPTION 'next_rowid: not for %', t;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtext('next_rowid:' || t)::bigint);
    EXECUTE format('SELECT COALESCE(max(id), 0) + 1 FROM %I', t) INTO n;
    RETURN n;
END $$;

CREATE TABLE cameras (
    id bigint PRIMARY KEY DEFAULT next_rowid('cameras'),
    space_id bigint NOT NULL REFERENCES spaces (id),
    name text NOT NULL,
    source text NOT NULL DEFAULT 'rtsp',
    url text NOT NULL,
    username text,
    password text,
    machine_id text,
    view_role text NOT NULL DEFAULT 'admin',
    created_at timestamptz NOT NULL,
    -- The machine it watches, in its space; that machine removed or moved: a room camera, seen from its
    -- view_role up.
    FOREIGN KEY (machine_id, space_id) REFERENCES machines (id, space_id) ON DELETE SET NULL (machine_id)
);
CREATE INDEX cameras_by_space ON cameras (space_id, name, id);

GRANT SELECT, INSERT, UPDATE, DELETE ON cameras TO maker_connect_app;
ALTER TABLE cameras ENABLE ROW LEVEL SECURITY;
CREATE POLICY in_space ON cameras USING (space_id = app_space());
