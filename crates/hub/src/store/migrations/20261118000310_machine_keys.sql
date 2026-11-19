-- Permissions follow their machine (hub-core-v2.md §3, "Step 2's tables"; lane 3b): now that machines are
-- on Postgres, a machine's grants, group places and requests for permission go with it (ON DELETE CASCADE).
-- Jobs and files keep no foreign key to machines: a job outlives its machine and its file (the old space
-- keeps its history, 20261118000300_machines.sql).

ALTER TABLE machine_grants
    ADD CONSTRAINT machine_grants_machine FOREIGN KEY (machine_id) REFERENCES machines (id) ON DELETE CASCADE;
ALTER TABLE machine_group_members
    ADD CONSTRAINT machine_group_members_machine FOREIGN KEY (machine_id) REFERENCES machines (id) ON DELETE CASCADE;
ALTER TABLE access_requests
    ADD CONSTRAINT access_requests_machine FOREIGN KEY (machine_id) REFERENCES machines (id) ON DELETE CASCADE;

-- The plan's partial index on running jobs (§2.7: "`jobs` (partial index on running)"): a machine's job
-- that is not over.
CREATE INDEX jobs_running ON jobs (machine_id) WHERE state IN ('loaded', 'queued', 'printing', 'paused', 'stopping');
