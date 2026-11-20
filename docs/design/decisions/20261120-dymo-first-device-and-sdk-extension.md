# The DYMO PnP as first device, and an extension of the Prusa Connect protocol

Date: 2026-11-20  
Status: accepted; decides OQ-49 and the direction of OQ-48  
Decided by: owner (2026-11-20, relayed by the coordinator); the additions' shapes by the architect, **ours (owner-approved direction 2026-11-20)**, waiting for the owner's approval

## Context

An agent without drivers registers nothing on the Prusa Connect printer protocol
([wire.md](../wire.md#the-unit-one-printer-per-device)). A device that is not a Prusa has no type, no
telemetry keys and no label job in that protocol.

## Decision

- **OQ-49: (a).** The first real registration and pairing come with the first device, the DYMO
  LabelManager PnP. Its driver is already ported and proven (`crates/engine/src/ports/labelle`,
  [audit](../../audits/dymo-labelmanager.md), adapter `crates/engine/src/device/labelmanager.rs`). The step
  is "the v2 agent with its settings page, plus the DYMO PnP as its first device, registered and paired
  over the SDK protocol". There is no test device and the agent is not itself a device.
- **OQ-48, direction.** The owner: "we hadden er in v1 al wat voor verzonnen. maar beter is om het
  Prusa-protocol aan te vullen hiervoor". A device that is not a Prusa uses the Prusa Connect protocol
  plus additions on top of it, not a link of our own. The additions are kept apart as a proposed SDK
  extension ([prusa-sdk-extension.md](../prusa-sdk-extension.md)): additive, ignored by a peer that does
  not know them, and minimal. v1's label fields were read as input only, and nothing of v1 is carried
  over.
- **The reference is read at `c0d7d233`** with `git show c0d7d233:<path>`, because the checkout is at
  `04c5bd59` ([wire.md](../wire.md)).

## Where it shows

[wire.md](../wire.md#the-first-device-dymo-labelmanager-pnp), [prusa-sdk-extension.md](../prusa-sdk-extension.md),
[risks-and-open.md](../risks-and-open.md).
