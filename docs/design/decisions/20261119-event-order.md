# Event order: seq is commit order, one relay publishes

Date: 2026-11-19  
Status: accepted  
Decided by: architect (journal:DESIGN-DOC:0024, review defect 2); owner may overrule

## Context

`seq` came from a sequence drawn inside the transaction, so a transaction that drew 1201 could commit
after one that drew 1202. A client resuming from 1201 would miss 1201 for good. The SSE handler also
queried stored facts before subscribing to the bus, so a fact committed in between was lost, and a
state patch could arrive before the snapshot it changes.

## Decision

- **Writing a fact:** the transaction takes `pg_advisory_xact_lock` of one fixed key before `nextval`
  on the event sequence, so seq order is commit order. It writes the row and publishes nothing.
- **One relay** task, woken by `NOTIFY` from each writing transaction, reads the committed facts with
  `seq` above the last it published, in order, and is the only publisher of facts on the bus. It reads
  every space's facts, so it runs on the admin pool (allowlist #4); the per-subscriber filter is
  applied on the bus. State patches reach the bus from the Device host directly, each with the
  device's `state_rev`, and only after the transaction that stored the patch and raised `state_rev`
  has committed: a snapshot's `rev` is then never behind a patch already published.
- **Resume:** the SSE handler subscribes to the bus first and buffers; sends a snapshot per visible
  device with its `state_rev`; then the stored facts above `Last-Event-ID`; then the buffered facts
  above the last sent; then live. A client drops a patch whose `rev` is not above its snapshot's.
- The lock serialises transactions that write a fact (risk R-17): measured on .67 in step 3. The
  alternative kept for then: a relay that orders by commit LSN (`pg_current_wal_lsn()` at commit),
  which needs no lock but reads `pg_stat` or logical decoding.

## Where it shows

[database.md](../database.md#events-and-history), [api.md](../api.md#the-sse-envelope),
[flows.md](../flows.md#6-sse-resume-browser), [processes.md](../processes.md#inside-the-hub).
