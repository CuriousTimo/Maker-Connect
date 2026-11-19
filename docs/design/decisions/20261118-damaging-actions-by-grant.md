# Damaging actions only by explicit grant

Date: 2026-11-18  
Status: accepted  
Decided by: owner (plan §5 Q4)

## Context

Being owner is not having had the laser course; raw G-code and firmware updates can damage a machine.

## Decision

Raw G-code (`device.gcode`), `mc.laser.start` and firmware update are granted by name to a person on a device, never part of the Operate preset; the Space UI shows them as separate switches. `mc.laser.start` is also person-only: never from the queue or a token.

## Where it shows

[authz.md](../authz.md), [device-contract.md](../device-contract.md).
