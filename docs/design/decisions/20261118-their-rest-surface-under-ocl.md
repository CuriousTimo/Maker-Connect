# Prusa's REST surface for 3D printers, under OCL

Date: 2026-11-18  
Status: accepted  
Decided by: owner (plan §5 Q1, "akkoord met alle vijf adviezen")

## Context

Serving the same paths, field names and status codes for 3D printers means our spec mirrors Prusa Connect's in substance; their code and docs are OCL (not AGPL-compatible, no automated text or data mining).

## Decision

Yes: the spec is written by us from running their server and our tests, never from their `openapi.json` or docs text, pinned to a `reference/prusa-connect` commit; Prusa is asked in a public issue whether an API-compatible server is fine with them (still to ask).

## Where it shows

A tracking test runs our hub and their server on the same requests; [api.md](../api.md), [OQ-2](../risks-and-open.md#oq-2-the-3d-printer-rest-surface).
