# Action names, presets and grant subjects

Date: 2026-11-19  
Status: accepted; decides OQ-4  
Decided by: coordinator on the owner's "fix het" (journal:DESIGN-DOC:0037), taking the architect's recommendation; owner may overrule

## Context

The plan names eight actions and ends with "…" (plan §2.6). Every route and verb must name its action
before the authz step can be built, and the presets must translate today's levels and roles
(`access.rs:70-78`, `:1210`, `RULES` `:148`) one to one so that nothing is lost silently (risk R-5).

## Decision

- **Device actions** (per person per device, in `device_access`): the registry's `device.view`,
  `device.move`, `device.heat`, `device.gcode`, `job.start`, `job.control`, `mc.laser.start`, plus
  `device.manage` (today's Manage level on one device) and `device.firmware` (plan §5 Q4).
- **Area actions** (by role, never granted per device): one area per module with `.view`, `.use` where
  today's routes distinguish a member's work from an admin's, and `.manage`: organisation, space,
  members, tokens, audit, grants, devices, files, queue, farm, stock, cameras
  ([authz.md](../authz.md#actions)). Labels and door access wait on OQ-14 and OQ-10.
- **Presets:** View, Use, Operate, Manage and the four roles as [authz.md](../authz.md#presets-roles-and-grants)
  lists them; the per-role route snapshot checks them against today's `RULES`, and every difference is
  a decision. Known differences: `device.gcode` and `mc.laser.start` leave Operate (they are damaging,
  [decision](20261118-damaging-actions-by-grant.md)); reordering the queue needs `queue.manage`.
- **Subjects:** a grant goes to a person. A group of people as subject is not built until asked for.

## Where it shows

[authz.md](../authz.md), [api.md](../api.md), `docs/registries/actions.md` once the code follows.
