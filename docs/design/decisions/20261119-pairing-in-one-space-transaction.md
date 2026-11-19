# Pairing runs in one space transaction

Date: 2026-11-19  
Status: accepted  
Decided by: architect (journal:DESIGN-DOC:0024, review defect 3); owner may overrule

## Context

`device_registrations` is admin only (there is no space before approval), yet the pairing flow wrote it
from a space transaction, and the alternative, claiming on the admin pool, would put a person's action
outside the space transaction and RLS.

## Decision

Two `SECURITY DEFINER` functions, owned by `maker_connect_owner`, `search_path` pinned to
`pg_catalog, public`, granted to `maker_connect_app`, are the only way a space transaction reaches the
table. Neither takes a space: both read `app_space()`.

- `claim_registration(code)`: an open, unexpired registration with that code is locked
  (`SELECT … FOR UPDATE`) and marked approved into `app_space()` by the session's user; returns its id,
  kind, identity and agent id. Called in the same transaction that inserts the `devices` row, with
  `agent_id` from the registration, or null for an agent's first registration (its `agents` row does
  not exist yet).
- `collect_registration(id, token_hash)`: locks the registration, checks it was approved into
  `app_space()` and not yet spent; for an agent's first registration inserts the `agents` row with
  the hash and sets `devices.agent_id`, for a hub-hosted device inserts the `device_credentials` row;
  marks it collected. The hub mints the token and passes only its hash.
- **Spent** is set at the token's first use (a printer's first report, an agent's first `up`), on the
  admin pool with the lookup by token hash (allowlist #1). An agent's token is shown once: a second
  collect gets `410`. Whether a Prusa printer may collect again until its code is spent is
  [OQ-12](../risks-and-open.md#oq-12-the-prusa-token-until-the-code-is-spent), open. The row locks make
  two claims of one code, or two collects of one registration, end with one winner.

The lookups that find the registration before the space is known (by code for a printer, by id and
secret for an agent) stay on the admin pool, allowlist #1. The app role is never granted the table.

## Where it shows

[database.md](../database.md#roles-and-pools), [flows.md](../flows.md#1-pairing-a-device-on-an-agent).
