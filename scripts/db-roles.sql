-- The hub's database roles (docs/plans/hub-core-v2.md §2.6; crate::store::pg), once per Postgres server, run
-- by a superuser. Safe to run again (it sets each role as below, and the password when given).
--
--   psql -v ON_ERROR_STOP=1 -v hub_password=… [-v owner_password=…] -f scripts/db-roles.sql
--
-- maker_connect_owner  owns the databases and every table; the migrations and the backup tools
--                      (pg_dump/pg_restore/psql, crate::backup) run under it. NOLOGIN, unless
--                      owner_password is given: the tests' own login (scripts/dev-db.sh, CI), which
--                      makes and migrates their databases.
-- maker_connect_app    request work, one space per transaction: row-level security holds.
-- maker_connect_admin  what crosses spaces by nature: BYPASSRLS.
-- maker_connect_hub    the hub's login: no superuser, owner of nothing, no privilege of its own. It is a
--                      member of the three above WITHOUT inheriting: it may only SET ROLE to one, so a
--                      `RESET ROLE` on a pooled connection reads no table.
\set ON_ERROR_STOP on
\if :{?hub_password}
\else
\echo 'db-roles.sql: give -v hub_password=…'
\quit
\endif

SELECT NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'maker_connect_owner') AS make \gset
\if :make
CREATE ROLE maker_connect_owner;
\endif
ALTER ROLE maker_connect_owner NOSUPERUSER CREATEDB NOCREATEROLE NOBYPASSRLS INHERIT NOLOGIN;

SELECT NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'maker_connect_app') AS make \gset
\if :make
CREATE ROLE maker_connect_app;
\endif
ALTER ROLE maker_connect_app NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS NOLOGIN;

SELECT NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'maker_connect_admin') AS make \gset
\if :make
CREATE ROLE maker_connect_admin;
\endif
ALTER ROLE maker_connect_admin NOSUPERUSER NOCREATEDB NOCREATEROLE BYPASSRLS NOLOGIN;

SELECT NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'maker_connect_hub') AS make \gset
\if :make
CREATE ROLE maker_connect_hub;
\endif
ALTER ROLE maker_connect_hub NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS NOINHERIT LOGIN
    PASSWORD :'hub_password';
GRANT maker_connect_owner, maker_connect_app, maker_connect_admin TO maker_connect_hub
    WITH INHERIT FALSE, SET TRUE;

\if :{?owner_password}
ALTER ROLE maker_connect_owner LOGIN PASSWORD :'owner_password';
\endif

-- The restore template (the design's storage-ops.md, OQ-33): a restore (crate::backup) fills a fresh
-- database made from it by maker_connect_owner, without a superuser. It holds the extension, made
-- here by the superuser; the hub's dumps leave the extension out (`--exclude-extension`), so a
-- restore never writes into TimescaleDB's own catalog. No connections: one would block a restore.
SELECT NOT EXISTS (SELECT FROM pg_database WHERE datname = 'maker_connect_template') AS make \gset
\if :make
CREATE DATABASE maker_connect_template OWNER maker_connect_owner TEMPLATE template0 IS_TEMPLATE true;
\c maker_connect_template
CREATE EXTENSION IF NOT EXISTS timescaledb;
\c postgres
ALTER DATABASE maker_connect_template ALLOW_CONNECTIONS false;
\endif
-- TimescaleDB's scheduler may have come in when the extension was made: it would block a restore.
SELECT count(pg_terminate_backend(pid)) AS ended FROM pg_stat_activity
    WHERE datname = 'maker_connect_template' \gset
