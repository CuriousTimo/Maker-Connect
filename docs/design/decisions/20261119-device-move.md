# Moving a device to another space

Date: 2026-11-19  
Status: accepted; confirms what the builder built (V2-STORE-6, `move_machine`)  
Decided by: coordinator on the owner's "beantwoord de vragen en fix het design waar nodig" (journal:DESIGN-DOC:0031, V2-STORE-6 Q4); owner may overrule

## Context

The target design had no way to move a device from one space to another; the builder's step had one
(handoff:V2-STORE-6:001) and asked which data goes with it.

## Decision

- History belongs to the space where it happened. A move hands over the device, not the old space's data.
- Old jobs, files sent, command and transfer records and samples stay in the old space: `commands`
  and `transfers` keep no foreign key to `devices` (as `jobs` do not), so the move leaves them whole
  and nothing kept forever is deleted (design review of PR #35, defect 4).
- The old space's grants, group memberships, `device_access` rows, access requests and queue items for
  the device are deleted in the move's transaction; a loaded spool and a camera watching the device
  are unlinked (`spool.device_id`, `cameras.watches_device_id` set null). The device's own
  `device_credentials`, `device_capabilities` and `cameras` rows follow it by `ON UPDATE CASCADE`.
- The move's transaction rebuilds the device's `device_access` rows in the new space from its roles
  and bumps both spaces' `authz_epoch`, so lists and SSE filters on both sides change at once.
- A device with a job not ended (`current_job_id` set) is not moved: `409 device/JOB_OPEN`. The
  job and its records stay in the old space; `devices.current_job_id` is a composite key into that
  space's `jobs` ([decision](20261119-composite-foreign-keys.md)).
- Moving is an instance-admin action: admin-pool allowlist #3.
- An agent-hosted device keeps its agent: an agent is a host, not a tenant row
  ([decision](20261119-agent-link-identity-and-delivery.md)).

Confirmed, not overruled: it matches the builder's code.

## Where it shows

[database.md](../database.md#roles-and-pools) (allowlist #3).
