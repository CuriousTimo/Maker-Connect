-- A group holds single locks too (wiki Design: Devices and Rights, "A group holds machines and
-- single locks"): a door controller's lock by its number from 0.
CREATE TABLE group_locks (
    space_id bigint NOT NULL,
    group_id bigint NOT NULL,
    machine_id text NOT NULL,
    lock_no integer NOT NULL,
    PRIMARY KEY (group_id, machine_id, lock_no),
    FOREIGN KEY (group_id, space_id) REFERENCES machine_groups (id, space_id),
    FOREIGN KEY (machine_id, space_id) REFERENCES machines (id, space_id) ON DELETE CASCADE
);
CREATE INDEX group_locks_by_machine ON group_locks (machine_id);

GRANT SELECT, INSERT, UPDATE, DELETE ON group_locks TO maker_connect_app;
ALTER TABLE group_locks ENABLE ROW LEVEL SECURITY;
CREATE POLICY in_space ON group_locks USING (space_id = app_space());
