# DYMO LabelManager PnP: our port against labelle

**The one reference:** labelle (github.com/labelle-org/labelle, Apache-2.0) at commit
`5e8f1e5` (`5e8f1e5d4f2f4447c8827df96c174812275d678f`), in `reference/labelle`. Its runtime:
pyusb 1.3.1 and Pillow 12.3.0 (the versions in `reference/.venv-audit`; Pillow's C source is in
`reference/pillow-12.3.0`). Ours: `crates/engine/src/ports/labelle/`, a port of labelle file by
file and function by function (`docs/agent-engine.md`, Step 3b), with pyusb and Pillow ported
for what labelle calls of them.

Proof: `scripts/labelle-port-check.py` runs **labelle's own command line**
(`labelle --picture FILE --tape-size-mm N`, its real code with real pyusb and real Pillow) on a
fake USB backend that shows the devices of a scene (the PnP's own descriptors, read on the
agent's Orange Pi, `0922:1001`) and records every operation pyusb asks of the backend: opening,
the control reads of the string descriptors, configuration, kernel driver, claiming, every
transfer with its bytes, endpoint and timeout, releasing, closing. The Rust test
(`ports::labelle::tests::proof_every_example_goes_as_labelle_sends_it`) runs the port on a
recorder over the same devices and the same PNG files. Every operation of both, in order, and
the result (`done`, or the error message) are compared. Run it after any change to the port: it
exits 0 only when all are identical. Deliberate breaks are caught (the dither threshold, the timeout, the bicubic constant and the cutter margin each make examples differ).

## 1. What goes over USB for a picture label: labelle's code run against ours

<!-- generated: begin -->
labelle `5e8f1e5` with pyusb 1.3.1 and Pillow 12.3.0, run by `scripts/labelle-port-check.py`: labelle's own command line (`labelle --picture FILE --tape-size-mm N`) on a fake pyusb backend, and the Rust port on a recorder over the same devices. Every operation of both, in order, with the final result, is in `crates/engine/tests/labelle/labelle.txt` and `ours.txt`.

| Example | What it shows | labelle (its own code, run) | Result |
|---|---|---|---|
| `picture_rgb` | RGB 150 x 64, 12 mm | 34 operations: 10 writes (5 × `1B 41`), 7 reads, 8 control reads; done | **identical** |
| `picture_rgba` | RGBA 120 x 64, alpha varying (ignored by convert L), 12 mm | 34 operations: 10 writes (5 × `1B 41`), 7 reads, 8 control reads; done | **identical** |
| `picture_grey` | grey 100 x 64 (dithered), 12 mm | 34 operations: 10 writes (5 × `1B 41`), 7 reads, 8 control reads; done | **identical** |
| `picture_one_bit` | 1-bit 100 x 64, 12 mm | 34 operations: 10 writes (5 × `1B 41`), 7 reads, 8 control reads; done | **identical** |
| `picture_palette` | palette 100 x 64, 16 colours, 12 mm | 34 operations: 10 writes (5 × `1B 41`), 7 reads, 8 control reads; done | **identical** |
| `picture_palette_trns` | palette with a transparent entry, 12 mm | 34 operations: 10 writes (5 × `1B 41`), 7 reads, 8 control reads; done | **identical** |
| `picture_la` | grey and alpha 100 x 64, 12 mm | 34 operations: 10 writes (5 × `1B 41`), 7 reads, 8 control reads; done | **identical** |
| `picture_hub_label` | as the hub's Label::to_png: grey 8-bit, 0 or 255, 400 x 64, 12 mm | 50 operations: 20 writes (10 × `1B 41`), 13 reads, 8 control reads; done | **identical** |
| `picture_wide` | grey 700 x 64: many groups of lines, 12 mm | 73 operations: 34 writes (17 × `1B 41`), 22 reads, 8 control reads; done | **identical** |
| `picture_short` | grey 80 x 40 on a 64-dot tape: fewer bytes a line, 12 mm | 30 operations: 8 writes (4 × `1B 41`), 5 reads, 8 control reads; done | **identical** |
| `picture_tall_rgb` | RGB 200 x 160: taller than the tape, resized, 12 mm | 30 operations: 8 writes (4 × `1B 41`), 5 reads, 8 control reads; done | **identical** |
| `picture_tall_rgba` | RGBA 200 x 160: taller than the tape, resized, 12 mm | 30 operations: 8 writes (4 × `1B 41`), 5 reads, 8 control reads; done | **identical** |
| `picture_tall_grey` | L 200 x 160: taller than the tape, resized, 12 mm | 30 operations: 8 writes (4 × `1B 41`), 5 reads, 8 control reads; done | **identical** |
| `picture_tall_la` | LA 200 x 160: taller than the tape, resized, 12 mm | 30 operations: 8 writes (4 × `1B 41`), 5 reads, 8 control reads; done | **identical** |
| `picture_tall_palette` | palette 200 x 160: resized (nearest), 12 mm | 30 operations: 8 writes (4 × `1B 41`), 5 reads, 8 control reads; done | **identical** |
| `picture_tall_one_bit` | 1-bit 200 x 160: resized (nearest), 12 mm | 30 operations: 8 writes (4 × `1B 41`), 5 reads, 8 control reads; done | **identical** |
| `picture_tall_thin` | grey 30 x 3000: more than 100 times taller than wide (resized in two steps), 12 mm | 24 operations: 4 writes (2 × `1B 41`), 3 reads, 8 control reads; done | **identical** |
| `picture_grey2` | grey 2-bit 64 x 64, 12 mm | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `picture_grey4` | grey 4-bit 64 x 64, 12 mm | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `picture_grey1` | grey 1-bit 64 x 64, 12 mm | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `picture_grey1_odd` | grey 1-bit 61 x 64, 12 mm | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `picture_grey16` | grey 16-bit 64 x 64 (mode I;16), 12 mm | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `picture_grey16_low` | grey 16-bit 64 x 64, values under 256, 12 mm | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `picture_rgb16` | RGB 16-bit 64 x 64, 12 mm | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `picture_rgba16` | RGBA 16-bit 64 x 64, 12 mm | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `picture_la16` | grey and alpha 16-bit 64 x 64, 12 mm | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `picture_palette1` | palette 1-bit 64 x 64, 12 mm | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `picture_palette2` | palette 2-bit 64 x 64, 12 mm | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `picture_palette4` | palette 4-bit 64 x 64, 12 mm | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `picture_palette_short` | palette 8-bit with 4 entries only, indexes beyond it, 12 mm | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `tape_6mm` | grey 100 x 64 on 6 mm tape: resized to 32 dots | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `tape_9mm` | grey 100 x 64 on 9 mm tape: resized to 48 dots | 27 operations: 6 writes (3 × `1B 41`), 4 reads, 8 control reads; done | **identical** |
| `tape_19mm` | grey 200 x 160 on 19 mm tape (96 dots): resized | 34 operations: 10 writes (5 × `1B 41`), 7 reads, 8 control reads; done | **identical** |
| `tape_19mm_small` | 400 x 64 on 19 mm tape: 8 bytes a line | 50 operations: 20 writes (10 × `1B 41`), 13 reads, 8 control reads; done | **identical** |
| `scene_single` | scene single | 28 operations: 10 writes (5 × `1B 41`), 7 reads, 4 control reads; done | **identical** |
| `scene_unconfigured` | scene unconfigured | 29 operations: 10 writes (5 × `1B 41`), 7 reads, 4 control reads; done | **identical** |
| `scene_printer_class` | scene printer_class | 27 operations: 10 writes (5 × `1B 41`), 7 reads, 4 control reads; done | **identical** |
| `scene_no_interface` | scene no_interface | 7 operations: 0 writes (0 × `1B 41`), 0 reads, 4 control reads; error Could not open a valid interface | **identical** |
| `scene_no_endpoints` | scene no_endpoints | 9 operations: 0 writes (0 × `1B 41`), 0 reads, 4 control reads; error The device endpoints not be found | **identical** |
| `scene_no_dymo` | scene no_dymo | 0 operations: 0 writes (0 × `1B 41`), 0 reads, 0 control reads; error No supported devices found | **identical** |
| `scene_only_labelwriter` | scene only_labelwriter | 0 operations: 0 writes (0 × `1B 41`), 0 reads, 0 control reads; error No matching devices found | **identical** |
| `scene_no_langid` | scene no_langid | 20 operations: 0 writes (0 × `1B 41`), 0 reads, 1 control reads; post them at that link. | **identical** |
| `scene_write_timeout` | scene write_timeout | 15 operations: 3 writes (2 × `1B 41`), 1 reads, 4 control reads; error Print error: [Errno 110] Operation timed out | **identical** |
| `scene_read_timeout` | scene read_timeout | 16 operations: 3 writes (2 × `1B 41`), 2 reads, 4 control reads; error Print error: [Errno 110] Operation timed out | **identical** |
| `scene_busy_setconfig` | scene busy_setconfig | 9 operations: 0 writes (0 × `1B 41`), 0 reads, 4 control reads; error Failed setup USB device: [Errno None] Configuration no | **identical** |
| `scene_denied_setconfig` | scene denied_setconfig | 8 operations: 0 writes (0 × `1B 41`), 0 reads, 4 control reads; error Access denied | **identical** |
| `scene_no_kernel_api` | scene no_kernel_api | 27 operations: 10 writes (5 × `1B 41`), 7 reads, 4 control reads; done | **identical** |
| `scene_no_strings` | scene no_strings | 24 operations: 10 writes (5 × `1B 41`), 7 reads, 0 control reads; done | **identical** |
| `tape_24mm` | unsupported tape size 24 mm | 9 operations: 0 writes (0 × `1B 41`), 0 reads, 4 control reads; error Unsupported tape size 24mm. Supported sizes: (19, 12,  | **identical** |
| `tape_none` | no tape size given: labelle takes 12 mm | 28 operations: 10 writes (5 × `1B 41`), 7 reads, 4 control reads; done | **identical** |
| `picture_missing` | a picture that is not there | 0 operations: 0 writes (0 × `1B 41`), 0 reads, 0 control reads; error Picture path does not exist: FIX/pngs/missing.png | **identical** |
| `picture_text` | a file that is no picture | 9 operations: 0 writes (0 × `1B 41`), 0 reads, 4 control reads; error cannot identify image file 'FIX/pngs/not_a_png.txt' | **identical** |

**52 of 52 examples identical.**

### Constants, command bytes and sizes, extracted from labelle's source with `ast`

| Row of labelle's source | Rust test | Result |
|---|---|---|
| `COMMAND _bytes_per_line ESC D` | `row_command_bytes_per_line` | **identical** |
| `COMMAND _status_request ESC A` | `row_command_status_request` | **identical** |
| `COMMAND _tape_color ESC C` | `row_command_tape_color` | **identical** |
| `DEFAULT_MARGIN_PX 56` | `row_default_margin_px` | **identical** |
| `DEFAULT_TAPE_SIZE_MM 12` | `row_default_tape_size` | **identical** |
| `DEV_VENDOR 2338` | `row_dev_vendor` | **identical** |
| `DPI 180` | `row_dpi` | **identical** |
| `ESC 27` | `row_esc` | **identical** |
| `HID_INTERFACE_CLASS 3` | `row_hid_interface_class` | **identical** |
| `LABELER_DISTANCE_BETWEEN_PRINT_HEAD_AND_CUTTER_MM 8.1` | `row_distance_head_cutter` | **identical** |
| `LABELER_PRINT_HEAD_HEIGHT_MM 8.2` | `row_print_head_height` | **identical** |
| `MM_PER_INCH 25.4` | `row_mm_per_inch` | **identical** |
| `PRINTER_INTERFACE_CLASS 7` | `row_printer_interface_class` | **identical** |
| `SUPPORTED_PRODUCTS 0x0011 DYMO LabelMANAGER PC` | `row_supported_products` | **identical** |
| `SUPPORTED_PRODUCTS 0x0015 LabelPoint 350` | `row_supported_products` | **identical** |
| `SUPPORTED_PRODUCTS 0x001c DYMO LabelMANAGER PC II` | `row_supported_products` | **identical** |
| `SUPPORTED_PRODUCTS 0x1001 LabelManager PnP (no mode switch)` | `row_supported_products` | **identical** |
| `SUPPORTED_PRODUCTS 0x1002 LabelManager PnP (mode switch)` | `row_supported_products` | **identical** |
| `SUPPORTED_PRODUCTS 0x1003 LabelManager 420P (no mode switch) WARNING: This device is no...` | `row_supported_products` | **identical** |
| `SUPPORTED_PRODUCTS 0x1004 LabelManager 420P (mode switch) WARNING: This device is not c...` | `row_supported_products` | **identical** |
| `SUPPORTED_PRODUCTS 0x1005 LabelManager 280 (no mode switch)` | `row_supported_products` | **identical** |
| `SUPPORTED_PRODUCTS 0x1006 LabelManager 280 (no mode switch)` | `row_supported_products` | **identical** |
| `SUPPORTED_PRODUCTS 0x1007 LabelManager Wireless PnP (no mode switch) WARNING: This devi...` | `row_supported_products` | **identical** |
| `SUPPORTED_PRODUCTS 0x1008 LabelManager Wireless PnP (mode switch) WARNING: This device ...` | `row_supported_products` | **identical** |
| `SUPPORTED_PRODUCTS 0x1009 MobileLabeler WARNING: This device is not confirmed to work w...` | `row_supported_products` | **identical** |
| `SUPPORTED_TAPE_SIZES_MM (19, 12, 9, 6)` | `row_supported_tape_sizes` | **identical** |
| `SYN 22` | `row_syn` | **identical** |
| `UNCONFIRMED_MESSAGE WARNING: This device is not confirmed to work with this software. P...` | `row_unconfirmed_message` | **identical** |
| `_maxLines 200` | `row_max_lines` | **identical** |
| `synwait 64` | `row_synwait` | **identical** |

**30 of 30 identical** (rows only on one side: 0).
<!-- generated: end -->

## 2. Ours, not labelle's (the owner to approve; Step 3b of `docs/agent-engine.md`)

| Item | labelle | Ours | Result |
|---|---|---|---|
| Sent while idle | nothing | nothing (the idle status poll is gone) | identical |
| Looking for the printer | `DeviceManager.scan` and `find_and_select_device` once per print (`cli.py:577-579`) | the same, once per print, no scan loop | identical |
| Tape width | `--tape-size-mm`, default 12 | from the job's `tape_mm`; none is 12 and an unsupported size fails with labelle's `ValueError` text, as `DymoLabeler.__init__` (the `tape_none` and `tape_24mm` examples) | identical |
| After an error | the process ends, the OS lets go of the device | the printer is released (`dispose`), the same operations as the exit-time finalizers (the `scene_*` and `tape_24mm` examples above) | identical |
| The picture's file | `Image.open` (any format Pillow knows) | the `png` crate decodes (PNG only); what Pillow does with the decoded rows is ported | **differs**: any other file is "cannot identify image file" |
| USB | pyusb's libusb1 backend | `nusb` (`nusb_backend.rs`): descriptors read from an opened device for labelle's vendor; kernel-driver check from sysfs on Linux; errors given the text and errno pyusb gives | **differs**: not tried on the printer |
| The printer's answers | read and thrown away | read the same; the first eight bytes go to the hub in hex, meaning unknown (L6) | ours |
| Command, pause/resume/stop, answer | none | fail loudly (L4, L5) | ours |
| Margins | `--justify` left, `--margin-px 56`, no min or max length (the CLI's defaults) | the same defaults, no options | identical (L9) |

## 3. Not covered (said, not hidden)

- **The real printer.** Nothing here has run on a PnP yet: the `nusb` stand-in (descriptor reads,
  `is_kernel_driver_active` from sysfs, transfers, the error texts) is tried only against the
  recorder. To try on the agent's Orange Pi.
- **Other PNGs than the examples:** interlaced PNGs (the `png` crate de-interlaces), PNG chunks
  other than the header, palette and data (Pillow's `convert("L")` ignores `tRNS` and the rest),
  16-bit grey (`I;16`) taller than the tape (its resize is not ported), the decompression-bomb
  check (ported, not run), any format but PNG.
- **Not ported, nothing that prints calls them:** `_dot_tab` (`ESC B`), `_cut` (`ESC E`),
  `_chain_mark`, `_skip_lines`, `_init_label`, `_reset_command`; `online_device_manager.py`; the
  text, QR, barcode, sample and preview engines; the CLI's `--min-length`, `--max-length`,
  `--fixed-length`, `--justify` and `--margin-px` options; patterns for `--device`.
- **Platforms:** the access-denied text is checked for Linux (the `scene_no_langid` example, with
  `platform.system()` set to Linux in both); the macOS text names the device by its first line
  only (pyusb's `str(device)` is not ported); the Windows text is not run.
- **The order labelle closes several open devices at its exit** is undefined (a `set`); the
  examples keep one device open at a time.
- **The 64 KiB blocks of `array.tofile`** are ported (`WRITE_BLOCK`); no example sends a transfer
  that large (a transfer holds at most 63 lines).
