# The agent's update is the instance admin's

Date: 2026-11-19  
Status: accepted  
Decided by: coordinator on the owner's "fix het" and "kritisch en grondig" (design review of 2a2ec32, defect 3, journal:DESIGN-DOC:0044); owner may overrule

## Context

`POST /api/v1/devices/{id}/agent-update` needed `device.manage` on one device. An agent is a host, not
a tenant row ([decision](20261119-agent-link-identity-and-delivery.md)): it hosts devices of several
spaces. So an admin of space B could update, and break, the agent that space A's devices run on.

## Decision

- Updating an agent is an instance action: `POST /api/v1/instance/agents/{id}/update`, gate
  `instance.admin`, admin-pool allowlist #3. `GET /api/v1/instance/agents` lists every agent with its
  version, last seen, stream state and its devices with their spaces, for the Instance page.
- `device.manage` no longer covers the agent's update; it stays what one does to one device (rename,
  settings, retire, farm on or off, PPE list, upkeep).
- A device page may show its agent's version to anyone with `device.view`; it offers no update.

Rejected for now: `device.manage` on every device the agent hosts. Safe, but a rule people cannot see
at a glance, and the agent is instance infrastructure like the hub's own update
(`POST /api/v1/update`, `instance.admin`). It can be added later if a space-owned agent turns out to
need it.

## Where it shows

[api.md](../api.md), [authz.md](../authz.md#actions), [architecture.md](../architecture.md#accounts-and-tenancy-the-organisation-and-space-boundary),
[database.md](../database.md#roles-and-pools) (allowlist #3), [ui.md](../ui.md).
