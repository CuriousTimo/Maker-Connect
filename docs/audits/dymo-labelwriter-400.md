# DYMO LabelWriter 400: our port against DYMO's CUPS driver

**The one reference:** DYMO's own CUPS driver 1.4.0 (`dymo-cups-drivers-1.4.0`, sha256
`c60797e7e986ca329f46e9a6ab1cb6382383952b15685ed69fd91f3c7ed64f71`, GPL-2 "or any later version",
Copyright (C) Sanford L.P., taken as GPL-3 so it combines with our AGPL-3.0), in
`reference/dymo-cups-drivers-1.4.0`: the filter `raster2dymolw`, with the PPD of DYMO Connect 1.6.4.18
(`reference/dymo-connect-1.6.4.18/ppd/lw400.ppd`; the pairing the 450 and the 320 have). Its runtime is CUPS
**v2.4.20, commit `ef80eea`** (`reference/cups-2.4.20`). The rolls: DYMO Connect 1.6.4.18's catalogue
(`reference/dymo-connect-1.6.4.18/catalog/Papers.xml`), as the 450's. Ours: `crates/engine/src/ports/dymo_cups/`,
the same port as the 450's ([dymo-labelwriter-450.md](dymo-labelwriter-450.md): its sections 1, 2, 4 and 5 are this
port's too, unchanged). What the 400 adds is only below.

## Result

- **The real filter against the port, on the same pages: identical** in **14 of 14** cases (stdout bytes,
  the stderr lines, the exit status; the no-printer case without the stderr lines and the repeated status
  requests, as the 450's) and in **61 of 61** `PageSize` choices of `lw400.ppd` for the page header (CUPS's own
  `cupsRasterInterpretPPD` on the real PPD against the port's).
- The 400 takes **`CLabelWriterDriver400`**, the 450's class: in the real filter by its PPD's `*ModelName`, in the port
  by the same dispatch (`raster2dymolw.rs`).
- **The rolls:** the catalogue's 400 group is the 450's paper group; the 13 papers it excludes for the 400 are
  generated into `dymo_connect_lw400.rs` and checked. Every remaining roll's page is a choice of `lw400.ppd`.
- **Not tried on the printer.** Nothing was read from a 400.

Run: `python3 -I scripts/dymo-labelwriter-400-check.py check` (the PPD data in `ppd_lw400.rs` is what `lw400.ppd` gives,
the driver's source holds what makes the 400 take `CLabelWriterDriver400`, `dymo_connect_lw400.rs` is what
`Papers.xml` gives, and every roll's page is in `lw400.ppd` with the 450's text), and `... proof` (needs `cargo build -p
maker-connect-engine --example dymo_cups_proof`; builds the real filter and the helper on libcups, as the 450's). The
script runs the 450's code (`scripts/dymo-labelwriter-450-check.py`) with the 400's PPD, URI and cases;
`cargo test -p maker-connect-engine dymo_cups geometry`.

## 1. Which driver class: `raster2dymolw.cpp:78-123`

| What | Reference | Ours | Result |
|---|---|---|---|
| The model name | `lw400.ppd:42` `*ModelName: "DYMO LabelWriter 400"` | `ppd_lw400.rs` `MODEL_NAME` (generated from the PPD, checked) | **identical** |
| The Twin Turbo list | `raster2dymolw.cpp:78-79`: the 400 is not in it | `raster2dymolw.rs`: the same two names | **identical** (checked in the source by the script) |
| The 400/450 list | `raster2dymolw.cpp:92` `!strcasecmp(ppd->modelname, "DYMO LabelWriter 400")`, first of seven | `raster2dymolw.rs`: the same seven names | **identical** (checked in the source by the script; proof) |
| The width | `LabelWriterDriver.cpp:38` `MaxPrintWidth_(84)`; no 400 entry in `CupsFilterLabelWriter.cpp:66-75` | `LabelWriterDriver::new`, `cups_filter_label_writer.rs`: no 400 entry | **identical** (checked in the source by the script; proof) |
| The resolution | `lw400.ppd:463-464` `*DefaultResolution: 300dpi`, the only choice; `CupsFilterLabelWriter.cpp` matches only `203dpi` and `203x138dpi`, so `SetResolution` is not called | the same code | **identical** (proof: `light density, 300 dpi`) |

## 2. The real filter against the port, on the same pages

As the 450's section 3, with the PPD of `lw400.ppd` and `DEVICE_URI` `usb://DYMO/LabelWriter%20400`. The cases are the
320's (`scripts/dymo-labelwriter-320-check.py:31-46`: the 450's, with `Resolution=300dpi` for the 450's `300x600dpi`),
copied into `scripts/dymo-labelwriter-400-check.py:31-46`; `lw400.ppd:461-465` has only `300dpi` either.

| Case | Options (`argv[5]`) | Result |
|---|---|---|
| default size (`w167h288`), defaults, status TOF on the back channel | none | **identical** (40,167 bytes, 3,444 stderr lines) |
| default size, `DEVICE_URI` not `usb://` (no status asked) | none | **identical** (40,163 bytes) |
| 1 in x 1 in (`w72h72`) | none | **identical** (6,628 bytes) |
| jewelry (`w154h64`), NLL | `DymoHalftoning=NLL` | **identical** (10,319 bytes) |
| shipping (`w167h288`), Default (1 bit) | `DymoHalftoning=Default` | **identical** (27,679 bytes) |
| address (`w79h252`), Dark and Graphics | `DymoPrintDensity=Dark DymoPrintQuality=Graphics` | **identical** (17,964 bytes) |
| `w72h154`, light density, `300dpi` | `DymoPrintDensity=Light Resolution=300dpi` | **identical** (11,546 bytes) |
| continuous, wide (`w154h792`) | none | **identical** (82,463 bytes) |
| `w54h144`, medium density, `DEVICE_URI` not set | `DymoPrintDensity=Medium` | **identical** (8,407 bytes) |
| `w72h72`, no printer: the back channel never answers | none | **identical** in the label's bytes and the exit status (6,640 bytes); **not compared**: the stderr lines and the number of `ESC A` requests (macOS's libcups `cupsBackChannelRead` ignores its timeout, see the 450's section 3) |
| shipping, NLL | `DymoHalftoning=NLL` | **identical** (42,451 bytes) |
| noise picture, ErrorDiffusion (`w79h252`) | none | **identical** (41,193 bytes) |
| noise picture, NLL (`w72h154`) | `DymoHalftoning=NLL` | **identical** (22,897 bytes) |
| noise picture, Default (`w162h90`) | `DymoHalftoning=Default` | **identical** (29,687 bytes) |

The page header of **every one of the 61 `PageSize` choices** of the PPD: CUPS's (libcups on the real PPD)
against the port's: **61 of 61 identical**; 19 of them repeat a page under another name and the port takes the
name of the first (`cupsPageSizeName` is not read by the filter).

## 3. The rolls: DYMO Connect's catalogue

| What | Reference | Ours | Result |
|---|---|---|---|
| The 400's group | `Papers.xml:1099-1101` `<PrinterGroup PaperGroupId="LabelWriter400">` with `<DriverName>DYMO LabelWriter 400</DriverName>` (XML comments taken out); `:1102-1104` name the 400 Turbo, the Twin Turbo and the DUO Label in the same group, so its exclusions hold for them too | `geometry::lw400::rolls`: the catalogue's rolls (`dymo_connect_lw450.rs`, the same group) | **identical** (checked by the script) |
| The papers it excludes | that group's `<ExcludedPapers>`: 13 `<PaperName>`s | `dymo_connect_lw400.rs` `EXCLUDED_PAPERS` (generated, checked); `lw400::rolls` leaves them out | **identical** (checked by the script; test `every_catalogue_roll_is_on_a_page_of_the_400`) |
| The roll's page | each roll's `page_size` is a `PageSize` choice of `lw400.ppd`, with the same text | `catalogue_rolls` on `lw400::page_sizes()` | **identical** (checked by the script) |
| Where the 400's PPD differs from the 450's | 30321 (`w102h252`): `lw400.ppd:164` `PageSize[102 252]`, `:296` `ImageableArea "4.32 4.32 98.40 234.96"`; `lw450.ppd:164` `[102 251]`, `:294` `"4.32 3.84 98.40 234.48"` | the 400's roll takes the 400's page | **identical** (the headers above; hub test `a_labelwriter_400_on_the_agent_prints_a_label`) |

## 4. The printer and its device ID

| What | Reference | Ours | Result |
|---|---|---|---|
| The device URI and the IEEE-1284 manufacturer and model the backend finds the printer by | DYMO's PPD and driver carry no device ID; the `usb` backend takes them from the printer (`usb-libusb.c`, `make_device_uri`) | `port.rs`: `DYMO` and `LabelWriter 400` (the PPD's model name without the make, as the 450's) | **not checked on the unit**: what a 400 reports as `MFG` and `MDL` has not been read |

## 5. Ours

The agent device `dymo.labelwriter-400` (`crates/agent/src/main.rs`, `Kind::Lw400`): the 450's, with its own folder
`labelwriter-400/`, its own card and its own registration; the port `LabelWriter400` (`port.rs`) is the 450's loop with
the 400's PPD and device URI. The hub's profile `dymo/labelwriter-400` (`media_from_port = "lw400"`). Nothing of our own
on the wire.

## 6. Not covered

- **A real printer.** No 400 was connected: the device ID (section 4), the status bytes of a 400, the USB interface
  and endpoints, the labels' real position.
- The 400 Turbo (`lw400t.ppd`) and the 4XL: other PPDs, not done.
- The status flow of a paper-out, a roll change or a failed read against the real filter (the monitor's own mock
  tests only, the 450's section 2).
- Everything the 450's section 6 lists (Windows, CUPS parts not ported, the old code).
