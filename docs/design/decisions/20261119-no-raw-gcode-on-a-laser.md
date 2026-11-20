# No raw G-code on a laser

Date: 2026-11-19  
Status: accepted  
Decided by: architect (journal:DESIGN-DOC:0024, review defect 1) under the owner's laser rules (hub-core-v2.md:29; memory maker-connect-laser-never-unattended); owner may overrule  
Confirmed by the owner, 2026-11-20 ("houden"): the hub refuses typed G-code to a laser (`LASER_NO_GCODE`).

## Context

`mc.laser.gcode` (LaserFoundry's console line) needed `device.gcode`. A raw line can switch the laser on
(`M3`, `M4`) without a frame, PPE or a person's Start: the laser rule would be hollow.

## Decision

The laser does not offer raw G-code. `mc.laser.gcode` leaves the `grbl` verb set (12 verbs become 11);
`device.gcode` is a 3D-printer action only. Not declaring a verb the reference has is allowed: the
rule is that we declare no verb the reference lacks. Whether a person-only laser console comes later
is [OQ-38](../risks-and-open.md#oq-38-a-person-only-laser-console); the recommendation is no.

## Where it shows

[authz.md](../authz.md#actions), [device-contract.md](../device-contract.md#kinds),
`docs/registries/capability-verbs.md` once the code follows.
