# The database address only from DATABASE_URL

Date: 2026-11-19  
Status: accepted  
Decided by: builder in the PR #34 review fixes (journal:V2-STORE-5:0023, b43a38f)

## Context

A `--database-url` flag put the password in argv (visible to every user of the host) and the trial child never got it.

## Decision

The hub reads its database address only from the `DATABASE_URL` environment variable; the address is never printed with its password, and a `?password=` is taken out of the query.

## Where it shows

[links.md](../links.md) L14, [database.md](../database.md), [OQ-8](../risks-and-open.md#oq-8-one-login-role-or-one-per-pool).
