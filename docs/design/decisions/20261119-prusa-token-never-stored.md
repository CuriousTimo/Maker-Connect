# The Prusa token is never stored

Date: 2026-11-19  
Status: accepted; decides OQ-12  
Decided by: coordinator on the owner's delegation, following prusa-connect to the letter (journal:DESIGN-WS:0005)

## Context

A printer may ask `GET /p/register` again until its first report spends the code (OQ-12). Buddy
6.4.0's registrator polls with the `Code` header and takes the `Token` header of a `200` as done
(`reference/buddy-6.4.0` `src/connect/registrator.cpp:204-221`); a poll whose answer is lost is made
again.

## Decision

As prusa-connect at `2e8c62b` does (`reference/prusa-connect` `internal/connector/xbuddy/registration.go`):

- The token's plaintext is never written down. It is held only in the memory of the hub process that
  minted it; the database keeps only its hash (`registration.go:114-118`).
- A printer that collects again from the process that minted its token gets that token from memory. A
  printer that collects after a hub restart, when no process holds the plaintext, gets a freshly minted
  token, and the new hash replaces the old one (`registration.go:624-638`).
- `device_registrations` has no `token` column.

## Where it shows

[database.md](../database.md#devices-and-commands), [risks-and-open.md](../risks-and-open.md#oq-12-the-prusa-token-until-the-code-is-spent),
[flows.md](../flows.md#1-pairing-a-device-on-an-agent), [decision](20261119-pairing-in-one-space-transaction.md).
