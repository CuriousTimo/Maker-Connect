# Buddy: the websocket dialect only

Date: 2026-11-19  
Status: accepted; decides OQ-3  
Decided by: owner ("v8 fw staat nu op de mini. oude non websocket support gaat in de ijskast", journal:V2-DEVICES:0005) and coordinator (journal:DESIGN-WS:0001); owner may overrule

## Context

The plan's `prusa-buddy` driver had two dialects on one `/p/register`: `buddy-http` (Buddy 6.3.0:
`/p/telemetry`, `/p/events`, `/f/…`) and `buddy-ws` (Buddy 6.4+: `/p/register`, `/p/auth`, `/p/ws`).
How a request shows its dialect was not checked (OQ-3). The owner's MINI now runs firmware with
websocket support.

## Decision

- v2 builds only the websocket dialect, `buddy-ws`, after Buddy 6.4.0 (`reference/buddy-6.4.0`) with
  prusa-connect's `internal/connector/xbuddy` as the server-side reference. Its routes are
  `/p/register`, `/p/auth` and `/p/ws` (`reference/prusa-connect` `internal/connector/xbuddy/xbuddy.go:16-19`,
  `:505`; `internal/transport/http/httpapi/printerroutes.go:15`).
- `buddy-http` (Buddy 6.3.0 over HTTP) is shelved: not built in v2, its routes not served. Its audit
  (`docs/audits/buddy.md`) stays as the record of that dialect.
- With one dialect nothing has to tell two dialects apart, so OQ-3's question falls away.

## Where it shows

Plan §2.2 and §3, [links.md](../links.md) (L10, L11), [architecture.md](../architecture.md),
[flows.md](../flows.md#2-a-prusa-printer-registers-buddy-630).
