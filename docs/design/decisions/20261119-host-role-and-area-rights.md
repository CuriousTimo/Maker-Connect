# A host role, and area rights per space

Date: 2026-11-19  
Status: accepted  
Decided by: owner: "ja, akkoord met host-rol en rechten per space" (journal:DESIGN-DOC:0071), on the coordinator's proposals journal:DESIGN-DOC:0069 and :0070; owner may overrule

## Context

Who may add, edit or archive spools (`stock.use`) started it. The owner: it depends "op wat de maker
space zelf wil; prive of op kantoor is het weer anders" (journal:DESIGN-DOC:0069). The roles had no
step between member and admin, while a space has people who run the day under an admin: "medewerker
of host onder een admin" (journal:DESIGN-DOC:0070).

## Decision

**A host role.**

- The roles, lowest first: guest < member < host < admin < owner.
- A host has a member's rights plus the daily work: the queue's every item (`queue.manage`), stock
  handling, seeing the members, handling access requests for non-damaging actions, recording
  inductions.
- A host never holds `space.manage`, never changes member roles (`space.members.manage`), never
  holds `tokens.manage`, never a damaging device action, and never grants one.
- Each space names the role in its own words (host, medewerker, staff); the role itself stays
  `host`.

**Area rights per space.**

- Per area, the space chooses which role holds `.use` (and `.view`), for example stock: member, or
  admin only.
- The defaults come from a **space kind** chosen when the space is made: makerspace, office or
  private.
- Only an area's `.use` and `.view` actions move between roles. `.manage` actions and damaging
  device actions are never in a preset.
- A change needs `space.manage`, is logged, and bumps the space's `authz_epoch`. Nobody raises their
  own rights through it ([no self-elevation](20261119-no-self-elevation.md)).

Not decided here, as questions for the owner:
[OQ-41](../risks-and-open.md#oq-41-the-host-role-open-points) (the host's open points) and
[OQ-42](../risks-and-open.md#oq-42-area-rights-open-points) (the area rights' open points).

## Where it shows

[authz.md](../authz.md#presets-roles-and-grants), [authz.md](../authz.md#area-rights-per-space),
[database.md](../database.md#tenancy-and-accounts), [api.md](../api.md), [ui.md](../ui.md#space-admin).
