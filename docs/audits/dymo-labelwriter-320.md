# DYMO LabelWriter 320: our port against DYMO's CUPS driver

**The one reference:** DYMO's own CUPS driver 1.4.0 (`dymo-cups-drivers-1.4.0`, sha256
`c60797e7e986ca329f46e9a6ab1cb6382383952b15685ed69fd91f3c7ed64f71`, GPL-2 "or any later version",
Copyright (C) Sanford L.P., taken as GPL-3 so it combines with our AGPL-3.0), in
`reference/dymo-cups-drivers-1.4.0`: the filter `raster2dymolw`, with the PPD of DYMO Connect 1.6.4.18
(`reference/dymo-connect-1.6.4.18/ppd/lw320.ppd`, FileVersion 3.1; the pairing the 450 has with `lw450.ppd`,
owner 2026-11-24). Its runtime is CUPS **v2.4.20, commit `ef80eea`** (`reference/cups-2.4.20`). Ours:
`crates/engine/src/ports/dymo_cups/`, the same port as the 450's ([dymo-labelwriter-450.md](dymo-labelwriter-450.md):
its sections 1 (every function of the ten driver files), 2 (the driver's own tests), 4 (the runtime) and 5 (ours)
are this port's too, unchanged). What the 320 adds is only below.

## Result

- **The real filter against the port, on the same pages: identical** in **14 of 14** cases (stdout bytes,
  the stderr lines, the exit status; the no-printer case without the stderr lines and the repeated status
  requests, as the 450's) and in **57 of 57** `PageSize` choices of `lw320.ppd` for the page header (CUPS's own
  `cupsRasterInterpretPPD` on the real PPD against the port's).
- The 320 takes the **base class** `CLabelWriterDriver`: in the real filter by its PPD's `*ModelName`, in the port by
  the same dispatch (`raster2dymolw.rs`). A deliberate break (the 320's page given the 450's class
  `CLabelWriterDriver400`) is caught: 0 of 14 cases then identical.
- **Not tried on the printer.** Nothing was read from a 320.

Run: `python3 -I scripts/dymo-labelwriter-320-check.py check` (the PPD data in `ppd_lw320.rs` is what `lw320.ppd` gives,
and the driver's source holds what makes the 320 take `CLabelWriterDriver`), and `... proof` (needs `cargo build -p
maker-connect-engine --example dymo_cups_proof`; builds the real filter and the helper on libcups, as the 450's). The
script runs the 450's code (`scripts/dymo-labelwriter-450-check.py`) with the 320's PPD, URI and cases;
`cargo test -p maker-connect-engine dymo_cups`.

## 1. Which driver class: `raster2dymolw.cpp:78-123`

| What | Reference | Ours | Result |
|---|---|---|---|
| The model name | `lw320.ppd:42` `*ModelName: "DYMO LabelWriter 320"` | `ppd_lw320.rs` `MODEL_NAME` (generated from the PPD, checked) | **identical** |
| The Twin Turbo list | `raster2dymolw.cpp:78-79`: the 320 is not in it | `raster2dymolw.rs`: the same two names, the 320 is not in it | **identical** (checked in the source by the script) |
| The 400/450 list | `raster2dymolw.cpp:92-98`: the 320 is not in it | `raster2dymolw.rs`: the same seven names, the 320 is not in it | **identical** (checked in the source by the script) |
| The last `else` | `:111-123` `CCupsFilter<CLabelWriterDriver, CDriverInitializerLabelWriterWithLM, CLabelWriterLanguageMonitor>` (`IsBackchannelSupported()` true, `:44-53`) | `CupsFilter::new(io, Class::Driver)` | **identical** (the proof, section 2: the bytes of all 14 cases) |
| The page's end and the document's end | `LabelWriterDriver.cpp:77-81` `SendFormFeed()` (`ESC E`) per page, `:57-60` `EndDoc` sends nothing | `label_writer_driver.rs` `end_page`, `end_doc` for `Class::Driver`; tests `driver_test_base_end_page_end_doc`, `a_job_on_the_320_prints_with_the_base_class` | **identical** (proof) |
| The width | `LabelWriterDriver.cpp:38` `MaxPrintWidth_(84)`; no 320 entry in `CupsFilterLabelWriter.cpp:66-75` | `LabelWriterDriver::new`, `cups_filter_label_writer.rs`: no 320 entry | **identical** (checked in the source by the script; proof) |
| The resolution | `lw320.ppd:443-447`: only `300dpi`; `CupsFilterLabelWriter.cpp` matches only `203dpi` and `203x138dpi`, so `SetResolution` is not called (the 450's "resolution trap") | the same code | **identical** (proof: `light density, 300 dpi`) |

## 2. The real filter against the port, on the same pages

As the 450's section 3 (`scripts/dymo-labelwriter-320-check.py proof`; the same helper `mkras.c` and the same example
`dymo_cups_proof`, now with the PPD of `lw320.ppd`, `DEVICE_URI` `usb://DYMO/LabelWriter%20320`). The cases are the
450's, on pages of the 320's PPD; the 450's `Resolution=300x600dpi` is not a choice of the 320's PPD, so that case
takes `Resolution=300dpi`.

| Case | Options (`argv[5]`) | Result |
|---|---|---|
| default size (`w167h288`), defaults, status TOF on the back channel | none | **identical** (40,165 bytes, 3,443 stderr lines) |
| default size, `DEVICE_URI` not `usb://` (no status asked) | none | **identical** (40,161 bytes) |
| 1 in x 1 in (`w72h72`) | none | **identical** (6,626 bytes) |
| jewelry (`w154h64`), NLL | `DymoHalftoning=NLL` | **identical** (10,317 bytes) |
| shipping (`w167h288`), Default (1 bit) | `DymoHalftoning=Default` | **identical** (27,677 bytes) |
| address (`w79h252`), Dark and Graphics | `DymoPrintDensity=Dark DymoPrintQuality=Graphics` | **identical** (17,962 bytes) |
| `w72h154`, light density, `300dpi` | `DymoPrintDensity=Light Resolution=300dpi` | **identical** (11,544 bytes) |
| continuous, wide (`w154h792`) | none | **identical** (82,461 bytes) |
| `w54h144`, medium density, `DEVICE_URI` not set | `DymoPrintDensity=Medium` | **identical** (8,405 bytes) |
| `w72h72`, no printer: the back channel never answers | none | **identical** in the label's bytes and the exit status (6,638 bytes); **not compared**: the stderr lines and the number of `ESC A` requests (macOS's libcups `cupsBackChannelRead` ignores its timeout, see the 450's section 3) |
| shipping, NLL | `DymoHalftoning=NLL` | **identical** (42,449 bytes) |
| noise picture, ErrorDiffusion (`w79h252`) | none | **identical** (41,191 bytes) |
| noise picture, NLL (`w72h154`) | `DymoHalftoning=NLL` | **identical** (22,895 bytes) |
| noise picture, Default (`w162h90`) | `DymoHalftoning=Default` | **identical** (29,685 bytes) |

The page header of **every one of the 57 `PageSize` choices** of the PPD: CUPS's (libcups on the real PPD)
against the port's, every field the interpreter sets: **57 of 57 identical**; 15 of them are choices that repeat a
page under another name and the port takes the name of the first (`cupsPageSizeName` is not read by the filter).

## 3. The printer and its device ID

| What | Reference | Ours | Result |
|---|---|---|---|
| The device URI and the IEEE-1284 manufacturer and model the backend finds the printer by | DYMO's PPD and driver carry no device ID (`lw320.ppd` has `*Manufacturer: "DYMO"`, `*Product: "(DYMO LabelWriter 320)"`, `*ModelName`); the `usb` backend takes them from the printer (`usb-libusb.c`, `make_device_uri`) | `port.rs`: `DYMO` and `LabelWriter 320` (the PPD's model name without the make, as the 450's `LabelWriter 450`) | **not checked on the unit**: what a 320 reports as `MFG` and `MDL` has not been read |

## 4. Ours

The agent device `dymo.labelwriter-320` (`crates/agent/src/main.rs`, `Kind::Lw320`): the 450's, with its own folder
`labelwriter-320/`, its own card and its own registration; the port `LabelWriter320` (`port.rs`) is the 450's loop
with the 320's PPD and device URI. Nothing of our own on the wire.

## 5. Not covered

- **A real printer.** No 320 was connected: the device ID (section 3), the status bytes of a 320, the USB
  interface and endpoints (the backend's `nusb` stand-in is untried, as the 450's), the labels' real position.
- The 320's other variants and the models that take the same base class (300, 310, 315, 330, 330 Turbo, SE450) and
  their widths (#216); the Twin Turbo (#217).
- The status flow of a paper-out, a roll change or a failed read against the real filter (the monitor's own
  mock tests only, the 450's section 2).
- The hub: its profile and rolls for the 320 (the hub knows the 450's `dymo/labelwriter-450` only).
- Everything the 450's section 6 lists (Windows, CUPS parts not ported, the old code).
