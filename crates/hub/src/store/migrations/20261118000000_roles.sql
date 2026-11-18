-- The store's two roles (docs/plans/hub-core-v2.md §2.6, tenancy): the hub connects as the tables' owner and
-- runs each pool's work under one of these (`SET ROLE` on every new connection, crate::store::pg).
--   maker_connect_app    request work: no BYPASSRLS, not the owner, so row-level security holds;
--   maker_connect_admin  sign-in, session/token lookup, agent token lookup, instance admin: BYPASSRLS.
-- Roles belong to the cluster, not to a database: made once, kept (and tests make databases in parallel).
DO $$
BEGIN
    CREATE ROLE maker_connect_app NOLOGIN NOBYPASSRLS;
EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL;
END $$;
DO $$
BEGIN
    CREATE ROLE maker_connect_admin NOLOGIN BYPASSRLS;
EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL;
END $$;

-- The owner may take either role (a superuser may anyway). Tests migrate many databases at once.
DO $$
BEGIN
    GRANT maker_connect_app, maker_connect_admin TO CURRENT_USER;
EXCEPTION WHEN OTHERS THEN NULL;
END $$;
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

-- Instance-wide settings (`open_spaces`).
CREATE TABLE meta (
    key text PRIMARY KEY,
    value text NOT NULL
);
