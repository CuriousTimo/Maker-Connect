-- USB cameras on an agent (source = 'agent'): fingerprint, token, the registration as it came, and when the
-- newest still arrived.
ALTER TABLE cameras
    ADD COLUMN fingerprint text,
    ADD COLUMN token text,
    ADD COLUMN registration text,
    ADD COLUMN snapshot_at timestamptz;
CREATE UNIQUE INDEX cameras_by_fingerprint ON cameras (space_id, fingerprint) WHERE fingerprint IS NOT NULL;
CREATE UNIQUE INDEX cameras_by_token ON cameras (token) WHERE token IS NOT NULL;

-- The newest still of each camera, as it was sent.
CREATE TABLE camera_snapshots (
    camera_id bigint PRIMARY KEY REFERENCES cameras (id) ON DELETE CASCADE,
    space_id bigint NOT NULL REFERENCES spaces (id),
    jpeg bytea NOT NULL
);

GRANT SELECT, INSERT, UPDATE, DELETE ON camera_snapshots TO maker_connect_app;
ALTER TABLE camera_snapshots ENABLE ROW LEVEL SECURITY;
CREATE POLICY in_space ON camera_snapshots USING (space_id = app_space());
