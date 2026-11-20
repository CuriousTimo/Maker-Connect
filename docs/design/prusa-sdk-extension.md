# Prusa Connect SDK: proposed extension for devices that are not Prusa printers

**Status: proposal.** This page is **ours (owner-approved direction 2026-11-20)**. It is not reference
behaviour, and nothing here is in Prusa-Connect-SDK-Printer or in Prusa Connect. The protocol it extends
is described in [wire.md](wire.md), which follows the SDK at `c0d7d233` to the letter. Everything below is
an addition on top of that protocol, kept as small as one device needs, and written so it could be
offered to Prusa.

Owner, 2026-11-20: "we hadden er in v1 al wat voor verzonnen. maar beter is om het Prusa-protocol aan te
vullen hiervoor" ([decision](decisions/20261120-dymo-first-device-and-sdk-extension.md)). What v1 had for
label printers was read only as input. Nothing of it is carried over.

The first device that needs the extension is the DYMO LabelManager PnP. Its port is labelle
(`crates/engine/src/ports/labelle`, audit [dymo-labelmanager.md](../audits/dymo-labelmanager.md)).

## Rules for every addition

- **Additive only.** No field, header, frame or command of the protocol changes its meaning. An addition
  is a new value where the protocol has an open value, or a new key that has an `mc_` prefix so it cannot
  collide with a key Prusa adds later.
- **Ignored by those who do not know it.** The SDK reads only the kwargs it names
  (`prusa/connect/printer/__init__.py:436-483`). Prusa's server maps only the telemetry keys it knows and
  drops the rest (`internal/connector/xbuddy/inbound.go:273-299`, `telemetryPaths` `:146-159`). A Prusa
  printer never sends an addition, so it is unaffected.

## E1: a printer type for a device that is not a Prusa

- **Purpose:** to say what the device is, so the hub picks its capabilities.
- **Shape:** the string `mc.dymo.labelmanager-pnp`. It travels in the registration body's
  `printer_type` (`__init__.py:733-738`) and in the `User-Agent-Printer` header of every request and of
  the upgrade (`:281`). In `INFO`, `type`, `version` and `subversion` are `null`, which is what the SDK
  already sends for a printer without a type (`:397-400`).
- **Why the protocol cannot carry it:** `printer_type` is `PrinterType`, an enum of Prusa models written
  as `a.b.c` numbers (`const.py:47-60`). A number of our own could collide with a model Prusa numbers
  later.
- **Compatibility:** Prusa printers send only `a.b.c` values. A server that does not know the string reads
  it as an unknown model (`xbuddy.go:816-829`) and refuses the device at admission (`403`, `:704-712`).
  It does not misread it as a Prusa.

## E2: three telemetry keys of a label printer

- **Purpose:** to show what the LabelManager PnP itself reports. These are the port's readings that the
  owner decided on 2026-11-16 (`crates/engine/src/ports/labelle/port.rs:90-103`).
- **Shape:** these keys are added to the telemetry object (`models.py:171-195`) and are left out when
  unknown:

  | Key | Type | Value |
  |---|---|---|
  | `mc_cassette` | boolean | a cassette is in (bit `0x40` of status byte 0) |
  | `mc_battery_percent` | integer 0-100 | status byte 3, when at most 100 |
  | `mc_raw_status` | string | the first 8 status bytes in hex |

- **Why the protocol cannot carry it:** Connect's telemetry has no key for a tape cassette, a battery or
  raw status bytes (`inbound.go:146-159`).
- **Compatibility:** a server that does not know a key drops it (`inbound.go:276-281`).

## E3: the tape width with a label print

- **Purpose:** to print a label on the tape the person says is loaded.
- **Shape:** the label is Connect's own "download and print". The hub sends `START_CONNECT_DOWNLOAD` with
  `path`, `hash`, `team_id` and `printing: true` (`__init__.py:457-483`). The file is a PNG fetched from
  `/p/teams/{team_id}/files/{hash}/raw` with the device's headers (`:468`; `download.py:399-413`). The
  extension adds one kwarg, `mc_tape_size_mm`, an integer from `19`, `12`, `9`, `6`, default `12`, which
  is the port's baseline (`crates/engine/src/device/labelmanager.rs:38-52`).
- **Why the protocol cannot carry it:** a Connect print takes its settings from the G-code file. A PNG has
  none, and no Connect command carries a tape width.
- **Compatibility:** the SDK's handler reads only `path`, `hash`, `team_id`, `printing` and `selecting`
  (`__init__.py:467-477`), so it ignores the kwarg. A Prusa printer is never sent it.

## Not extended

The rest of the LabelManager PnP needs no addition. It uses the protocol's own values, and the choices of
which value to use are **ours** (see [wire.md](wire.md#the-first-device-dymo-labelmanager-pnp)):

- states `IDLE` and `PRINTING` (`const.py:24-34`);
- a failed label as the transfer's or the state's own events, with `reason`;
- pause, resume and dialogs answered `REJECTED` "Not Implemented" by the SDK itself, because the device has
  no handler for them (`command.py:184-186`).
