# The system architecture is approved

Date: 2026-11-19  
Status: accepted  
Decided by: owner (journal:DESIGN-DOC:0023: the diagram first, then all chapters and code fit it); the review and its fixes by the coordinator on the owner's "beantwoord en beoordeel de 3 vragen voor me" (journal:DESIGN-DOC:0035, :0037); owner may overrule

## Context

[architecture.md](../architecture.md) at `c143048` was reviewed against the plan and the other
chapters. Two dependency defects and one wording conflict were found.

## Decision

The architecture is the foundation every chapter and all code fit into, with these fixes:

- **The route guard decides and never dispatches.** It answers who the caller is, which action the route
  needs and whether the caller holds it, and depends only on Accounts and tenancy, Registries and
  Store. The API hands an allowed request on to Command records, Modules, the Door access module or
  Camera sessions. A security boundary that depends on the modules it guards cannot be tested on its
  own, and the planned `architecture_dependencies_hold` test enforces the rule.
- **Command records ask the guard** for the action and person-only checks of every command, whoever
  asks (a person, a token, the queue), so a command from the queue meets the same rule as one from
  the API.
- **The guard reaches its tables through Store** (`device_access`, grants, `authz_epoch`); the
  arrow was missing.
- **USB camera frames:** the image data is never decoded or re-encoded; only the missing Huffman
  tables are inserted ([decision](20261119-usb-camera-dht-ffmpeg.md)).

## Where it shows

[architecture.md](../architecture.md#may-depend-on), [authz.md](../authz.md).
