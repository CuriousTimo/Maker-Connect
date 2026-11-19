# Database roles and pools (risks R1, R2)

Date: 2026-11-19  
Status: accepted  
Decided by: architect (journal:DESIGN-DOC:0005 R1/R2; owner: fix before merge, journal:DESIGN-DOC:0006); owner may overrule

## Context

The admin pool (BYPASSRLS) was used far beyond plan §2.6, and the hub logged in as superuser and set roles per connection, so `RESET ROLE` escaped RLS.

## Decision

A login `maker_connect_hub`: not superuser, owns nothing, no privileges of its own, `NOINHERIT` member of `maker_connect_owner` (owns everything, CREATEDB; migrations, dumps), `maker_connect_app` (NOBYPASSRLS; every space transaction) and `maker_connect_admin` (BYPASSRLS; an allowlist of six uses checked by a test: registrations and secret
lookups; getting in and the signed-in person's own account rows; instance admin; tasks that span
spaces (the event relay, the history subscriber, the sweeps); background tasks listing their work; the
signed-in person's own memberships and spaces; [database.md](../database.md#roles-and-pools)).
Where a person's action in Space must touch an admin-only table (pairing), a `SECURITY DEFINER` function
owned by `maker_connect_owner` does the one thing needed inside the space transaction
([decision](20261119-pairing-in-one-space-transaction.md)); the app pool is never given the table.

**What this does not give (R-2 in full).** The login may `SET ROLE maker_connect_owner` or
`maker_connect_admin`, and any role may set `app.space_id`; the owner bypasses RLS without `FORCE`, and
`FORCE ROW LEVEL SECURITY` is not an option because `pg_dump`'s `COPY` needs the owner to read every
row. RLS is the second wall against our own query forgetting the space filter, not against a
compromised process or an injected statement. The guards for that are sqlx's checked, parameterised
queries and a source test that fails on `SET ROLE`, `RESET ROLE`, `set_config` or `app.space_id`
outside the pool module. Splitting the login is [OQ-8](../risks-and-open.md#oq-8-one-login-role-or-one-per-pool). Roles, the database and the TimescaleDB extension are made once by the operator, never by a migration.

Answers to the builder's questions (V2-STORE-6 Q1-Q3; coordinator, journal:DESIGN-DOC:0031):

- `maker_connect_owner` stays `NOLOGIN` on a server. Dev and CI alone give it a login (`scripts/db-roles.sql -v owner_password=`), because `sqlx::test` makes a database per test and needs a `CREATEDB` login. The hub refuses to start on a login that owns tables.
- The app role has `SELECT` only on `users`, under an RLS policy (a membership in `app_space()` or an org membership in its organisation); no space transaction writes a `users` row (the person's own `display_name` and `token_generation` are #2, `instance_admin` and `disabled` are #3). `identities`, `sessions` and `meta` are admin only and revoked from the app role.
- Org-wide operations get no allowlist reason: they run on the app pool, in a space transaction of a space of that organisation, with the guard checking the org role. Organisation admins are not allowlist #3. Creating an organisation stays #2 (first account) or #3 (instance admin).

## Where it shows

[database.md](../database.md#roles-and-pools), [storage-ops.md](../storage-ops.md), risks R-2, R-4.
