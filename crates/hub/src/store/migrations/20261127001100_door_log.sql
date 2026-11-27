-- The door log (docs/design/prusa-sdk-extension.md part 7, "When": "The server keeps the changes: the
-- log of when each door opened and closed"; wiki Design: Devices and Rights, "The door log: as events
-- per lock").

-- A door controller's locks with a door switch fitted (numbers from 0, a JSON list): the hub reads
-- `door` only for those; the board cannot tell an open door from no switch.
ALTER TABLE machines ADD COLUMN locks_switch text;

-- What happened, as design database.md "Events and history" has it: `seq` from one sequence, taken
-- under one advisory lock so that seq order is commit order (decisions/20261119-event-order.md);
-- `type` what happened, `source` the device, `origin` who or what did it, `data` the rest. Kept a year
-- (decisions/20261118-retention-defaults.md). No compression, so row-level security holds.
CREATE SEQUENCE event_seq;
CREATE TABLE event (
    time timestamptz NOT NULL,
    seq bigint NOT NULL,
    space_id bigint NOT NULL,
    type text NOT NULL,
    source text NOT NULL,
    origin jsonb,
    data jsonb NOT NULL DEFAULT '{}',
    UNIQUE (seq, time)
);
SELECT create_hypertable('event', by_range('time', INTERVAL '7 days'));
SELECT add_retention_policy('event', INTERVAL '1 year');
CREATE INDEX event_by_source ON event (space_id, source, time DESC);

GRANT SELECT, INSERT ON event TO maker_connect_app;
GRANT USAGE ON SEQUENCE event_seq TO maker_connect_app;
ALTER TABLE event ENABLE ROW LEVEL SECURITY;
CREATE POLICY in_space ON event USING (space_id = app_space());
