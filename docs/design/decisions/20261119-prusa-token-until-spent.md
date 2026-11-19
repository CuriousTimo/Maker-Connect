# The Prusa token until the code is spent

Date: 2026-11-19  
Status: accepted; decides OQ-12 (a)  
Decided by: coordinator on the owner's delegation (journal:DESIGN-WS:0001); owner may overrule

## Context

A printer may ask `GET /p/register` again until its first report spends the code; the hub must then
give the same token again, but tokens are stored hashed (OQ-12). Buddy 6.4.0's registrator polls with
the `Code` header and takes the `Token` header of a `200` as done (`reference/buddy-6.4.0`
`src/connect/registrator.cpp:204-221`); a poll whose answer is lost is made again.

## Decision

(a): the token is kept in `device_registrations` until it is spent (its first use), and only its hash
after that.

## Checked against the reference

prusa-connect at `2e8c62b` (`internal/connector/xbuddy/registration.go`) does not write the token
down: it keeps the plaintext only in the memory of the process that minted it and stores only the
hash (`registration.go:114-118`); a printer that collects after a restart gets a freshly minted token
(`registration.go:624-638`). Decision (a) keeps the plaintext in the database until the first use
instead. This difference is put to the owner; until answered, (a) stands as decided.

## Where it shows

[database.md](../database.md), [flows.md](../flows.md#2-a-prusa-printer-registers-buddy-630),
[decision](20261119-pairing-in-one-space-transaction.md).
