# The order of the host-side checks: authorization first

Date: 2026-11-19  
Status: accepted; decides OQ-34 (b)  
Decided by: coordinator on the owner's "fix het" (design review of 2a2ec32, defect 11: the two drawings disagreed, journal:DESIGN-DOC:0044); the architect's recommendation; owner may overrule

## Context

The plan lists argument validation, the state guard, authz and the laser rule (plan §2.1) without an
order. device-contract.md drew capability, arguments, state, then authz; flows.md drew the route guard
first. With (a), a caller without the right learns from the fault code whether the device has the
verb and what state it is in.

## Decision

1. The route guard: the caller holds the verb's action on the device, and a person-only verb comes
   from a person's own session. A refusal here is the guard's authz fault and writes no command record.
2. Then the record is written (`requested`) and the host-side checks run in this order: the verb is
   in the enabled set (`device/CAPABILITY_NOT_SUPPORTED`), the arguments are valid
   (`validation/ARGUMENT_*`), the current state accepts the verb (`device/STATE_NOT_ACCEPTED`). A
   refusal here settles the record `refused` with its fault code.
3. The laser rule (frame after the last origin, PPE confirmed) is part of step 2 for `mc.laser.start`.

## Where it shows

[device-contract.md](../device-contract.md#host-side-rules), [flows.md](../flows.md#2-a-command-click-to-settled),
[authz.md](../authz.md#the-decision-on-a-request), [risks-and-open.md](../risks-and-open.md#oq-34-the-order-of-the-host-side-checks).
