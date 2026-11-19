# Repeated commands and the error body

Date: 2026-11-19  
Status: accepted; decides OQ-29 (a)  
Decided by: coordinator on the owner's delegation (journal:DESIGN-WS:0001); owner may overrule

## Context

The plan has an idempotency key on a command or transfer; what a repeat returns, and what an error
body holds besides the code, was open (OQ-29).

## Decision

(a): a repeat with the same idempotency key returns the first record; the error body is `code` plus
`text`.

## Where it shows

[api.md](../api.md#conventions-apiv1).
