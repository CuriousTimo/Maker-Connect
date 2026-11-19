# Accepting an invite

Date: 2026-11-19  
Status: accepted  
Decided by: architect (journal:DESIGN-DOC:0024, review defect 9); owner may overrule

## Context

The invite flow had a page and no endpoint, and said nothing about a person who already has an account.

## Decision

`POST /api/v1/invites/accept`, no action (the invite token is the credential):

- not signed in: `{token, name, display_name, password}` makes the account, its identity, the
  membership (or organisation membership) and marks the invite used, in one transaction on the admin
  pool (allowlist #2), and answers `201` with the session cookie;
- signed in: `{token}` adds the membership for the signed-in user and marks the invite used;
- already a member of that space or organisation: refused with `authz/ALREADY_MEMBER`; a role changes
  through `PATCH /spaces/{id}/members/{user}`, never through an invite.

The page at `/invite/{token}` shows the invite and asks a signed-out person whether they have an
account; it changes nothing itself.

## Where it shows

[api.md](../api.md), [flows.md](../flows.md#5-sign-in-and-invite), [database.md](../database.md#roles-and-pools).
