-- A profile chosen for a Marlin printer whose M115 name (`MACHINE_TYPE`) no profile knows (#148 step
-- 2; wiki Machine Profiles, "Learned from real printers"): the name, the profile chosen, the firmware
-- it said (`FIRMWARE_NAME`), when. scripts/learned-names.sh adds the names to the profiles.
CREATE TABLE learned_names (
    space_id bigint NOT NULL REFERENCES spaces (id),
    machine_type text NOT NULL,
    profile text NOT NULL,
    firmware text,
    seen_at timestamptz NOT NULL,
    PRIMARY KEY (space_id, machine_type, profile)
);
GRANT SELECT, INSERT, UPDATE, DELETE ON learned_names TO maker_connect_app;
ALTER TABLE learned_names ENABLE ROW LEVEL SECURITY;
CREATE POLICY in_space ON learned_names USING (space_id = app_space());
