-- The store's roles (docs/plans/hub-core-v2.md §2.6, tenancy) belong to the cluster, not to a database: made
-- once by scripts/db-roles.sql (a superuser runs it), never here. The migrations run as their owner
-- (maker_connect_owner, crate::store::pg); the hub logs in as maker_connect_hub and gives each pooled
-- connection one of these with `SET ROLE` as it opens:
--   maker_connect_app    request work: no BYPASSRLS, not the owner, so row-level security holds;
--   maker_connect_admin  sign-in, session/token lookup, device token lookup, instance admin: BYPASSRLS.
GRANT USAGE ON SCHEMA public TO maker_connect_app, maker_connect_admin;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO maker_connect_app, maker_connect_admin;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO maker_connect_app, maker_connect_admin;

-- The space of the transaction (`SET LOCAL app.space_id`, crate::store::pg::Pg::space); none: NULL, so a
-- policy `space_id = app_space()` lets no row through. Once set in a session, the setting reads '' after its
-- transaction ends, hence the NULLIF.
CREATE FUNCTION app_space() RETURNS bigint LANGUAGE sql STABLE
    AS $$ SELECT NULLIF(current_setting('app.space_id', true), '')::bigint $$;

-- Times are timestamptz (§2.7); the hub's Rust still counts Unix seconds (i64), as its pages and JSON
-- show them. These two are the one place the two meet, so what the hub shows stays byte for byte:
-- `at_unix($1)` where a time is written or compared, `unix_seconds(col)` where one is read.
CREATE FUNCTION at_unix(bigint) RETURNS timestamptz LANGUAGE sql IMMUTABLE STRICT
    AS $$ SELECT to_timestamp($1) $$;
CREATE FUNCTION unix_seconds(timestamptz) RETURNS bigint LANGUAGE sql IMMUTABLE STRICT
    AS $$ SELECT floor(extract(epoch FROM $1))::bigint $$;

-- Instance-wide settings (`open_spaces`).
CREATE TABLE meta (
    key text PRIMARY KEY,
    value text NOT NULL
);
-- The admin role's alone (design database.md#roles-and-pools).
REVOKE ALL ON meta FROM maker_connect_app;
