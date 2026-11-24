# Adding a DYMO LabelWriter model

How the LabelWriter 320 was added (#25). It is the recipe for the next model, so the work does not
have to be worked out again. The LabelWriter stack is one port, `crates/engine/src/ports/dymo_cups`: a
port of DYMO's CUPS driver 1.4.0 (`reference/dymo-cups-drivers-1.4.0`). A model differs from the others
only in its PPD and in what the driver's source does for that PPD's `*ModelName`.

The LabelManager tape printers (`lm*.ppd`, labelle) are a different stack and are not covered here.

## What you need

- **The model's PPD**, from `reference/dymo-connect-1.6.4.18/ppd/` (DYMO Connect 1.6.4.18, the same set
  the 450 and the 320 use). The LabelWriter files there: `lw.ppd`, `lw300.ppd`, `lw310.ppd`, `lw315.ppd`,
  `lw320.ppd`, `lw330.ppd`, `lw330t.ppd`, `lw400.ppd`, `lw400t.ppd`, `lw450*.ppd`, `lw4xl.ppd`,
  `lw550*.ppd`, `lw5xl.ppd`, `lwduo*.ppd`, `lwtt.ppd`, `lww.ppd`, `lwwxl.ppd`, `se450.ppd`.
  - Not checked: whether the 1.4.0 driver handles the 550 and 5XL. Those are newer printers, so check
    the driver's source before you start on one of them.
- **The printer on .67**, for the proof on paper.

## Look up in DYMO's source (quote each line, as the 320's audit does)

1. **Which driver class** the filter takes for the PPD's `*ModelName`: `raster2dymolw.cpp:78-123` picks one:
   - `CLabelWriterDriverTwinTurbo` for the Twin Turbos
   - `CLabelWriterDriver400` for the 400 / 450 / 4XL / DUO Label
   - the base `CLabelWriterDriver` for every other model, such as the 320
   - `scripts/dymo-labelwriter-320-check.py` checks this for the 320. Copy that check for the new model.
2. **Its print width**: `MaxPrintWidth_(84)` bytes (672 dots) by default, at `LabelWriterDriver.cpp:38`.
   `CupsFilterLabelWriter.cpp:66-75` changes it for these models only:
   - 58 for the 300, 310 and 315
   - 156 for the 4XL
   - 56 for the SE450
   - This width is `head_dots` in the profile (bytes × 8).
3. **Its resolution**: the PPD's `*DefaultResolution`. This is `dpi` in the profile.

## The steps, one file each

| Step | File | From the 320 |
|---|---|---|
| 1. The PPD as Rust | `scripts/dymo-labelwriter-<model>-check.py` (copy the 320's: set `PPD`, `OUT`, `STEM`, `MODEL`, `URI`, the cases), then run `gen` | `ppd_lw320.rs` |
| 2. Open it | `PpdFile::lw<model>()` in `crates/engine/src/ports/dymo_cups/cups/ppd.rs`; `pub mod ppd_lw<model>` in `cups/mod.rs` | `PpdFile::lw320` |
| 3. The port | a `static MODEL_<model>: Model` in `crates/engine/src/ports/dymo_cups/port.rs` (its PPD, its `REASONS`, its device URI) and its type | `MODEL_320`, `LabelWriter320` |
| 4. The agent | a `Kind` in `crates/agent/src/main.rs`: its wire type `dymo.labelwriter-<model>`, its config name, its title, its USB product id | `Kind::Lw320` |
| 5. The rolls for the hub | `pub mod lw<model>` in `crates/engine/src/ports/geometry.rs`: `page_sizes()` from `page_sizes_of` with its PPD, and `rolls()`. A model with no DYMO Connect catalogue copies `lw320::rolls`: one roll per `PageSize` choice | `geometry::lw320` |
| 6. The hub | its name in `port_rolls` in `crates/hub/src/profiles.rs`; `profiles/dymo/labelwriter-<model>.toml` (`media_from_port`, `dpi`, `head_dots`, `[detect] printer_type`) and its `.svg` | `labelwriter-320.toml` |
| 7. The proof | `proof` in the checker: DYMO's real `raster2dymolw` against the port, on the same pages | `docs/audits/dymo-labelwriter-320.md` |
| 8. The audit | `docs/audits/dymo-labelwriter-<model>.md` and its line in `docs/audits/README.md` | the 320's |

The hub needs nothing else. Everything in the hub that is a LabelWriter goes by `firmware = "dymo-lw"`
in the profile:
- the label page
- the check that a label fits the roll
- `page_size` in `START_CONNECT_DOWNLOAD`

## Tests to copy

- `geometry::lw320::tests`:
  - every `PageSize` choice is a roll
  - the port takes every page the hub gives
- `a_labelwriter_320_on_the_agent_prints_a_label` in `crates/hub/src/devices/prusa_connect.rs`:
  - the model registers and gets its profile and name
  - a label goes out with the roll's `page_size`

## Notes from the models done so far (read before you start)

From the 400 (#230):

- **Compare the PPD with one that is done** before writing anything. Use `diff` on the `*PageSize`,
  `*ImageableArea` and `*PaperDimension` lines and on the `*Resolution` choices. The 400 differs from the 450
  only in 30321 (`w102h252`), where its `PageSize` is `[102 252]` and its `ImageableArea` is
  `"4.32 4.32 98.40 234.96"`. It also has only `300dpi`, so it takes the 320's proof cases, not the 450's
  (the 450's include `300x600dpi`).
- **Look in DYMO Connect's `Papers.xml` `PrinterGroups`** for the model's `DriverName`. Strip the XML
  comments first: some groups are commented out. Three outcomes:
  - **In a group:** it takes the catalogue's rolls (`dymo_connect_lw450.rs`) on its own PPD, less **its own
    `ExcludedPapers`**. Generate that list into `dymo_connect_lw<model>.rs` and check it.
  - **In no group** (the 320): one roll per `PageSize` choice (`lw320::rolls`).
  - **Watch out:** a grep that hides the `<PaperName>` lines also hides the exclusions.
  - A group lists several `DriverName`s: its exclusions hold for each of them (the 400's group also names the
    400 Turbo, the Twin Turbo and the DUO Label).
- **The 450's catalogue ignores `ExcludedPapers`**, an old gap: DYMO Connect hides e.g. "30337 Audio
  Cassette" on the 450. That is a separate issue, not part of a new model.
- **The agent finds the printer by its IEEE-1284 model** (`usb://DYMO/LabelWriter%20<model>`), not by a USB
  product id. Without a unit to read the device ID from, mark the URI NOT CHECKED, as `port.rs` does.
- **Write a test only for a value you have read from the file**, never one you expect.
- **The checker:** copy the 320's or the 400's. `check_driver_source` checks the class list the model is
  in, by a regex on `raster2dymolw.cpp`, plus the width and the resolution.
- **A generated module needs its own `#[rustfmt::skip]`** in `ports/mod.rs`. One put between the 450's attribute and
  its module takes that attribute, and `cargo fmt` then rewrites `dymo_connect_lw450.rs`, so its checker fails.
- **Adding the model to the agent's `match` arms:** `Kind::Lw450 | Kind::Lw320` occurs in longer patterns too. Replace
  each pattern once, or clippy finds `Kind::Lw400 | Kind::Lw400` (unreachable pattern).
