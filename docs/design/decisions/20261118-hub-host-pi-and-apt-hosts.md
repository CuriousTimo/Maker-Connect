# Hub host for v2: the Pi with the Timescale image; apt hosts preferred

Date: 2026-11-18  
Status: accepted  
Decided by: owner (plan §5 Q3)

## Context

Timescale apt packages exist for arm64/amd64 only; the test Pi .67 is armhf.

## Decision

.67 (armhf) runs `timescale/timescaledb:2.32.0-pg17` in Docker as tried; arm64/amd64 hosts with apt packages are documented as the preferred makerspace host.

## Where it shows

[storage-ops.md](../storage-ops.md), [pi-hub.md](../../pi-hub.md).
