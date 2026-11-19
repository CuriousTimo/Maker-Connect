# Source modules are mapped to blocks by a checked-in file

Date: 2026-11-19  
Status: accepted; decides OQ-35  
Decided by: coordinator on the owner's "beantwoord de 3 vragen voor me" (journal:DESIGN-DOC:0035); owner may overrule

## Context

The planned `architecture_dependencies_hold` test needs to know which block each source module
belongs to; today's modules do not follow the blocks.

## Decision

A checked-in map file, `crates/hub/tests/blocks.map` (one line per module path and its block; a module
missing from the map fails the test), read by the test, so the rule holds on today's code. The move
to one module per block happens inside the device, authz and history build steps, not as a step of
its own.

## Where it shows

[architecture.md](../architecture.md#may-depend-on).
