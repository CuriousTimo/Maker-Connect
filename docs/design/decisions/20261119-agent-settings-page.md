# The agent's settings page

Date: 2026-11-19  
Status: accepted; decides OQ-20 (a)  
Decided by: coordinator on the owner's delegation (journal:DESIGN-WS:0001); owner may overrule

## Context

One agent hosts several devices (#12). Who may open its small web page, and how, was open (OQ-20).

## Decision

(a): the agent has a settings page of its own, reached on the LAN only, behind a password made at
install, in the house style ([decision](20261119-house-colour.md)). The hub side of the design does
not change.

## Open

The config file or the page as the source, what "made at install" is, how the password is kept and
what "LAN only" checks: [OQ-44](../risks-and-open.md#oq-44-the-agent-settings-page-open-points).

## Where it shows

[architecture.md](../architecture.md#the-agent-update-credentials-settings-page),
[processes.md](../processes.md#inside-the-agent), plan §3 (V2-DEVICES step 2).
