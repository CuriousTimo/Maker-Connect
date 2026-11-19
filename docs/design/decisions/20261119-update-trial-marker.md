# The update marker drives the way back

Date: 2026-11-19  
Status: accepted  
Decided by: architect (journal:DESIGN-DOC:0024, review defect 10); owner may overrule

## Context

A power loss during the trial left the old build to start on a database the new build had migrated;
nothing on disk said a trial was open.

## Decision

`crates/update` writes `<exe>.update.json` beside the binary before the dump, fsynced: `from`, `to`,
`until`, `starts`, the dump's name, `phase: trial`. Going back sets `phase: going_back`. The rule:
**no build opens the database while a marker says `going_back`.** It restores the named dump into a
fresh database, swaps the databases, then the binary, then removes the marker. A build that finds a
`trial` marker at start that is not its own trial (the `from` build after a power loss) sets
`going_back` and does the same. A power loss during the restore repeats it. A finished trial removes
the marker last. The agent keeps its `--version` trial; `install`'s try-out proves the new build runs.

## Where it shows

[storage-ops.md](../storage-ops.md#self-update), [flows.md](../flows.md#6-self-update-dump-and-the-way-back), risk R-18.
