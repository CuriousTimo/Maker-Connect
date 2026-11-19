# Web UI: server-rendered on the public API

Date: 2026-11-18  
Status: accepted  
Decided by: owner (plan §5 Q2)

## Context

An SPA needs a Node toolchain and a second build on the Pi release path, and rewrites every page.

## Decision

Server-rendered pages (askama templates) on the public `/api/v1` and `/api/v1/events`; an SPA only reconsidered for the multi-tenant cloud if page weight or offline use calls for it.

## Where it shows

[ui.md](../ui.md).
