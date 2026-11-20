# The DYMO PnP as first device, and an extension of the Prusa Connect protocol

Date: 2026-11-20  
Status: accepted; decides OQ-49 and the direction of OQ-48  
Decided by: owner (2026-11-20); the additions' shapes worked out with the owner in the coordinator's session, 2026-11-20

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
  not know them, and minimal.
- **The reference is read at `c0d7d233`** with `git show c0d7d233:<path>`, because the checkout is at
  `04c5bd59` ([wire.md](../wire.md)).

## The extension's shape (owner, 2026-11-20)

- **Written as a proposal Prusa could adopt.** No prefix on the names; the SDK's naming (upper-case
  commands, snake_case keys). The owner: "eerst werkend met de DYMO en niet zelf Prusa benaderen nu".
- **The protocol's own words first**, additions only where nothing existing means the same.
- **Three devices:** the DYMO PnP (owner: "akkoord"), the GRBL laser ("we hebben dus ook een extension
  nodig voor de laser"), the USB camera's live view ("we willen live beeld en hebben daar een
  uitbreiding voor nodig").
- **Laser:** GRBL's state mapped onto the SDK's states, with `Door` and `Sleep` as `ATTENTION`, and GRBL's
  own state in `machine_state`. Motion commands get general names (`HOME`, `JOG`, `FRAME`, …), not laser
  names, because a CNC needs the same (owner: "akkoord, algemene namen").
- **The proposal as a whole:** put to the owner item by item in the session (the type names; the DYMO's
  keys; the laser's state table, `machine_state`, the settings keys, the commands it reuses and the new
  ones; the camera's `/c/stream`, `START_CAMERA_STREAM`/`STOP_CAMERA_STREAM` with `camera_id` and `fps`
  default 5). The owner: "akkoord, algemene namen, schrijf het document". Afterwards the coordinator added two
  points and told the owner, who has not contradicted them: `Unknown` as `BUSY`, and no `time_printing` for the
  laser (its port does not give it).
- **Camera:** "eerst wat prusa heeft. daarna de live stream . alleen als iemand kijkt"; "in de camera
  settings een keuze tussen prusa mode / live". This decides OQ-9.

## Where it shows

[wire.md](../wire.md#the-first-device-dymo-labelmanager-pnp), [prusa-sdk-extension.md](../prusa-sdk-extension.md), [cameras.md](../cameras.md#usb-cameras-snapshots-then-live-view),
[risks-and-open.md](../risks-and-open.md).
