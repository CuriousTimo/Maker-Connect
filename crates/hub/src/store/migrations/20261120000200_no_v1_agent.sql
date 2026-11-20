-- The v1 agent link is gone (owner: "geen oude v1 zooi meer"): its update columns, and `agent` as the
-- link a machine has unless it says otherwise. Every machine names its link (`register_linked`,
-- `add_in_space`); the v2 agent's own tables are design database.md's `agents` and `devices`.
ALTER TABLE machines
    DROP COLUMN agent_update_to,
    DROP COLUMN agent_update_at,
    ALTER COLUMN link DROP DEFAULT;
