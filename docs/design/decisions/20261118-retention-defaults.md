# Retention defaults

Date: 2026-11-18  
Status: accepted  
Decided by: owner (plan §5 Q5)

## Context

History and events grow without bound on a 1 GB Pi.

## Decision

Raw samples 30 days, 1-minute aggregates 1 year, 1-hour aggregates forever; events 1 year; command records and the audit log forever. Space admins may lower them per space later.

## Where it shows

[database.md](../database.md), [storage-ops.md](../storage-ops.md).
