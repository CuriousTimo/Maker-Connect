# Job states are derived

Date: 2026-11-19  
Status: accepted; decides OQ-5  
Decided by: coordinator (journal:DESIGN-DOC:0037), taking the architect's recommendation; owner may overrule

## Context

Jobs keep their own table and their start, pause and stop are command records (plan §2.5), but the job
states were not set.

## Decision

`jobs.state` is one of `queued`, `loaded`, `running`, `paused`, `ended`, with `jobs.outcome` (`done`,
`stopped`, `failed`) on `ended`. The state follows from the settled command records and the device's
job end; no page sets it. A laser job stays `loaded` until a person's Start burn
([device-contract.md](../device-contract.md#host-side-rules)).

## Where it shows

[database.md](../database.md#files-jobs-queue-and-farm), [flows.md](../flows.md#4-a-job-with-the-laser-rule).
