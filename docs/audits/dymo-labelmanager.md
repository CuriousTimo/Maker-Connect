# DYMO LabelManager (PnP and kin): our agent against labelle

**The one reference:** labelle (github.com/labelle-org/labelle, Apache-2.0) at commit
`5e8f1e5` (`5e8f1e5d4f2f4447c8827df96c174812275d678f`). Chosen 2026-11-10 as the
best-maintained program that prints on these printers (25 contributors, regular releases;
labelbee, the other candidate, is a two-commit ESP32 experiment). Its files:
`src/labelle/lib/devices/dymo_labeler.py` (here `dl`), `src/labelle/lib/devices/usb_device.py`
(`ud`). Ours: `crates/label/src/dymo.rs`, the agent's loop in `crates/agent/src/hub.rs`.

Proof: `scripts/dymo-audit-check.py` runs labelle's own code as its CLI does (`setup`,
`DymoLabeler`, `print`), with real pyusb and real Pillow, against a fake USB device with the
PnP's own descriptors (read on the agent's Orange Pi) that records every operation; our
`dymo::session` runs the same examples on a recorder with the same descriptors; every
operation of both is compared. Run it after any change to `dymo.rs`: it exits 0 only when
all are identical. A deliberate break is caught (timeout 1000 → 2000: 0 of 15; bit order
flipped: the 4 pictures differ).

## 1. What goes over USB for a label: labelle's code run against ours

<!-- generated: begin -->
labelle `5e8f1e5` with pyusb 1.3.1 and Pillow 12.3.0, run by `scripts/dymo-audit-check.py`. Every operation of both, in order, is in `crates/label/tests/labelle/labelle.txt` and `ours.txt`.

| Example | What it shows | labelle (its own code, run) | Ours (`dymo::session`) | Result |
|---|---|---|---|---|
| `short` | 10 lines, 12 mm (8 bytes a line) | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 2 writes (1 × `1B 41`), 2 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 2 writes (1 × `1B 41`), 2 reads of 512 bytes; timeout 1000 ms | **identical** |
| `blank` | 114 blank lines | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 4 writes (2 × `1B 41`), 3 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 4 writes (2 × `1B 41`), 3 reads of 512 bytes; timeout 1000 ms | **identical** |
| `one_transfer_edge` | 63 lines: just under one transfer | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 4 writes (2 × `1B 41`), 3 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 4 writes (2 × `1B 41`), 3 reads of 512 bytes; timeout 1000 ms | **identical** |
| `two_transfers` | 64 lines: the 64th SYN starts a second transfer | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 4 writes (2 × `1B 41`), 3 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 4 writes (2 × `1B 41`), 3 reads of 512 bytes; timeout 1000 ms | **identical** |
| `group_edge` | 201 lines: still one group (labelle: while > 201) | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 8 writes (4 × `1B 41`), 5 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 8 writes (4 × `1B 41`), 5 reads of 512 bytes; timeout 1000 ms | **identical** |
| `two_groups` | 202 lines: 200 + 2, ESC D only once | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 10 writes (5 × `1B 41`), 7 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 10 writes (5 × `1B 41`), 7 reads of 512 bytes; timeout 1000 ms | **identical** |
| `long` | 650 lines: groups 200, 200, 200, 50 | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 26 writes (13 × `1B 41`), 17 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 26 writes (13 × `1B 41`), 17 reads of 512 bytes; timeout 1000 ms | **identical** |
| `syn_in_data` | every byte 0x16: data counts as SYN | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 6 writes (3 × `1B 41`), 4 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 6 writes (3 × `1B 41`), 4 reads of 512 bytes; timeout 1000 ms | **identical** |
| `narrow_6mm` | 6 mm tape (4 bytes a line) | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 4 writes (2 × `1B 41`), 3 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 4 writes (2 × `1B 41`), 3 reads of 512 bytes; timeout 1000 ms | **identical** |
| `tape_9mm` | 9 mm tape (6 bytes a line) | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 4 writes (2 × `1B 41`), 3 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 4 writes (2 × `1B 41`), 3 reads of 512 bytes; timeout 1000 ms | **identical** |
| `tape_19mm` | 19 mm tape (12 bytes a line) | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 4 writes (2 × `1B 41`), 3 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 4 writes (2 × `1B 41`), 3 reads of 512 bytes; timeout 1000 ms | **identical** |
| `picture_12mm` | a picture 150 × 64, 12 mm: image to bytes | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 10 writes (5 × `1B 41`), 7 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 10 writes (5 × `1B 41`), 7 reads of 512 bytes; timeout 1000 ms | **identical** |
| `picture_6mm` | a picture 90 × 32, 6 mm | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 10 writes (5 × `1B 41`), 7 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 10 writes (5 × `1B 41`), 7 reads of 512 bytes; timeout 1000 ms | **identical** |
| `picture_9mm` | a picture 90 × 48, 9 mm | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 10 writes (5 × `1B 41`), 7 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 10 writes (5 × `1B 41`), 7 reads of 512 bytes; timeout 1000 ms | **identical** |
| `picture_long` | a picture 400 × 64: two groups | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 20 writes (10 × `1B 41`), 13 reads of 512 bytes; timeout 1000 ms | open_device, get_configuration, is_kernel_driver_active 0, detach_kernel_driver 0, claim_interface 0, release_interface 0, close_device; 20 writes (10 × `1B 41`), 13 reads of 512 bytes; timeout 1000 ms | **identical** |

**15 of 15 identical.**
<!-- generated: end -->

## 2. Run in section 1

Every operation in this list is in the recordings of section 1 and compared there:
opening (`open_device`, `get_configuration`, the interface chosen, `is_kernel_driver_active`,
`detach_kernel_driver`, `claim_interface`), every write (bytes, endpoint, timeout), every read
(size, endpoint, timeout), closing (`release_interface`, `close_device`), the commands
(`ESC C 0`, `ESC D n`, `SYN` + line, `ESC A` twice), the groups of 200 lines, the transfers to
the 64th `SYN`, the bytes a line per tape, and a picture turned into lines.

| Item | labelle | Ours | Result |
|---|---|---|---|
| Sent while idle | nothing: opened only to print (`dymo_labeler.py:346` `dispose()` after each label) | every 15 s (`DYMO_STATUS_EVERY`): opened as labelle opens it, `ESC A`, one read, let go of (`dymo::status_session`); test `the_idle_status_request_opens_and_closes_as_labelle_does` checks the opening and closing against labelle's recordings | **differs, on purpose**: ours, the owner's decision 2026-11-11, to show the cassette and battery between labels |

## 3. Not covered (said, not hidden)

- **The label's layout** (margins, centring a picture lower than the tape): labelle's
  renderer draws it from text; ours comes from the hub. A picture as high as the tape is
  turned into bytes as labelle does (section 1); our 16 mm blank tail is part of the
  picture.
- **Finding the printer:** labelle lists USB devices with pyusb and reads the product strings
  from the device; we read the kernel's copy from the system's device list (nothing sent).
- **The printer's answers:** labelle does not look at them. We show byte 0 (cassette) and
  byte 3 (charge, as the PnP says it) and act on neither.
- **Mode switch** to `0922:1002`: labelle does not do it; nor do we.
- **nusb's `detach_and_claim_interface`** is one call (Linux `USBDEVFS_DISCONNECT_CLAIM`)
  where pyusb makes three (`is_kernel_driver_active`, `detach_kernel_driver`,
  `claim_interface`); the recorder writes it as those three. The kernel does the same.
- **macOS:** labelle's issue #5; the agent prints on Linux.
- **Not yet tried on the printer** (2026-11-10): to be done on .67.
