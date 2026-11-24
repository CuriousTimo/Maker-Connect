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
