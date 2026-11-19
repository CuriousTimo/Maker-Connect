# A token never outgrows its person

Date: 2026-11-19  
Status: accepted; decides OQ-28  
Decided by: coordinator (journal:DESIGN-DOC:0037), taking the architect's recommendation; owner may overrule

## Context

A token has scopes (an action set); its person has `device_access` and a role.

## Decision

A token's effective rights are its scopes intersected with its person's `device_access` and role at
the time of the request. Losing a right or a membership takes it from every token at once; the
person's generation bump ends every token they minted.

## Where it shows

[authz.md](../authz.md#the-decision-on-a-request).
