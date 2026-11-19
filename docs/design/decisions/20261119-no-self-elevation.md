# Nobody gives a role above their own

Date: 2026-11-19  
Status: accepted  
Decided by: architect (journal:DESIGN-DOC:0024, review defect 7) under the owner's rule (memory security-first-no-self-elevation: permissions only in Space, no shortcuts; fix what was criticised, invent no extra rules); the coordinator kept the two rules that close the defect and moved the rest to a question; owner may overrule

## Context

A space admin could make themselves owner through `PATCH /spaces/{id}/members/{user}`, or through an
invite for the whole organisation.

## Decision

- A role given by invite or `PATCH` is at most the giver's own effective role (the higher of their
  organisation and space role); above it: `authz/ROLE_ABOVE_OWN`. Raising one's own role is therefore
  impossible.
- An invite for the whole organisation is made only by someone with an organisation role, and gives at
  most that role; otherwise `authz/ORG_ROLE_NEEDED`.
- Grants of device actions in Space stay as they are: an admin or owner sets them, their own included.

Not decided here, as a question for the owner ([OQ-39](../risks-and-open.md#oq-39-roles-above-ones-own-and-the-last-owner)):
whether an admin may change or remove a member above them, whether anyone may lower their own role,
and what happens to the last owner.

## Where it shows

[authz.md](../authz.md#presets-roles-and-grants), [api.md](../api.md), [flows.md](../flows.md#7-sign-in-and-invite).
