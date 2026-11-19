# Organisation actions come from an organisation role only

Date: 2026-11-19  
Status: accepted  
Decided by: coordinator on the owner's "fix het" and "kritisch en grondig" (design review of 2a2ec32, defect 1, journal:DESIGN-DOC:0044), under the owner's rule that permissions are given only in Space and nobody elevates themselves (memory security-first-no-self-elevation, [decision](20261119-no-self-elevation.md)); owner may overrule

## Context

The admin role held "every area action", and `organisation.manage` is an area action. A space admin
with no organisation role could therefore make spaces, rename the organisation and set organisation
memberships up to their own role: a space role reaching above its space.

## Decision

- `organisation.manage` is held by an organisation admin or organisation owner, and by nobody else.
  A space role, however high, never holds it; the admin and owner rows of the role table read "every
  area action but `organisation.manage`".
- The effective role (the higher of the organisation and the space role) still gives an organisation
  admin the admin's area actions and device presets in every space of the organisation. The other way
  round does not hold.
- Routes that name `organisation.manage` (`POST /api/v1/spaces`, `PATCH /api/v1/organisations/{id}`,
  organisation memberships, open spaces) run in a space transaction of a space of that organisation
  and the guard checks the organisation role ([database.md](../database.md#roles-and-pools), org-wide
  work on the app pool).

## Where it shows

[authz.md](../authz.md#actions) (area actions, roles), [api.md](../api.md), [architecture.md](../architecture.md#accounts-and-tenancy-the-organisation-and-space-boundary).
