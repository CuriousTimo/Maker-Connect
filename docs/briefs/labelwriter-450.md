# Brief: the DYMO LabelWriter 450 port (DYMO's CUPS driver), in parallel with the Marlin proof

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust workspace; git repo, never commit; never
deploy or touch hardware). PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`.
**Build in your own target directory:** `export CARGO_TARGET_DIR=target-lw450` on every cargo
command. Another agent runs the Marlin proof (tests and scripts under `crates/engine/` and
`scripts/`, the `octoprint/` folder): never touch `crates/engine/src/ports/octoprint/` or
`crates/engine/src/ports/labelle/`; in shared files (`crates/engine/Cargo.toml`,
`crates/engine/src/ports/mod.rs`, `crates/agent/src/ports.rs`, `crates/agent/src/link.rs` `Mode`,
`Cargo.lock`, `docs/agent-engine.md`) only add lines (re-read before writing). The only audit files
you write: the new `docs/audits/dymo-labelwriter-450.md` and row 7 of `docs/audits/README.md`
(replace its "not chosen / fused" text with the current result only). If the engine crate
fails to compile because of another folder, wait and retry; never fix it yourself.

Read `CLAUDE.md` first: PORT, don't paraphrase; invent nothing; ONE reference: **DYMO's own CUPS
driver 1.4.0** (official tarball from download.dymo.com, sha256
`c60797e7e986ca329f46e9a6ab1cb6382383952b15685ed69fd91f3c7ed64f71`), unpacked in
`reference/dymo-cups-drivers-1.4.0`. Licence: GPL-2 "or any later version" (© Sanford L.P.), taken
as GPL-3 so it combines with our AGPL-3.0: keep its copyright and licence notice in every ported
file. Anything not traceable to a line of that driver (or the runtime named below) is marked
"ours" and listed. Read `docs/agent-engine.md`: the engine (O1–O8), the hub link (S1–S5), B2, B4,
B8, the owner's notes that the PnP and the 450 are **completely separate stacks with no shared
code**, and "Step 3b" (the PnP port) as the model of a top edge. **Never** use
`thermal-label/labelwriter` (`reference/labelwriter`) or the old `crates/label/src/labelwriter.rs`
as a source: they are the fused code this replaces.

**Gate first (the owner: "als de 450 cups agent mee valt"):** before porting, check that the
driver's `raster2dymolw` and its tests build on this Mac (CUPS headers) and that the tests read
clearly. If it does not work out, stop and report why; no fallback, no other reference.

**Only the LabelWriter 450.** Nothing of the PnP, nothing of OctoPrint, nothing of the hub side.

## 1. The port: `crates/engine/src/ports/dymo_cups/`

One Rust file per driver file, function by function, with file:line: `src/lw/LabelWriterDriver.cpp`
(+ `.h`), `src/lw/CupsFilterLabelWriter.cpp`, `src/lw/raster2dymolw.cpp` (what it does after the
raster is read), `src/lw/LabelWriterLanguageMonitor.cpp` (status, as far as the 450 uses it), and
from `src/common/` what those use for the 450 (`CupsFilter.h`, `CupsPrintEnvironment.cpp`, the
halftoning the 450's PPD selects: `Halftoning.cpp`, `ErrorDiffusionHalftoning.cpp`,
`NonLinearLaplacianHalftoning.cpp` — port the one(s) the 450 uses; list the rest as not ported).
The settings come from the plain 450's PPD (`ppd/lw450.ppd`; not `lw450t`, `lw450dl`, `lw450dt`,
`lw450tt`, `lm450`, `se450`: the Turbo, DUO and Twin Turbo variants are listed as not covered): media sizes, defaults (density, quality,
halftoning): port the values the driver reads, from that file, with line numbers.

**The runtime around the driver:** CUPS itself (the raster reader, `ppdc`/options, and the `usb`
backend that writes to the printer and gives the language monitor its back channel) is not in the
tarball. Where the driver needs it: the raster comes from the hub's PNG, decoded with the `png`
crate (ours, as the PnP port), turned into the driver's raster lines exactly as CUPS's
`cupsRasterReadPixels` would give them for the 450's PPD colour model (state the model from the
PPD); USB with `nusb` standing for CUPS's `usb` backend (ours; USB printer class, bulk out, back
channel read as the monitor expects). Mark each of these "ours" and keep them minimal.
**Nothing is sent while idle.**

No `use` of, copying from, or shared module with `ports/labelle`: the top-edge scaffolding is
written for this port on its own (Step 3b is a model of the pattern only).

No hardware in tests: under `cfg(test)` opening a real USB device panics (as section H's serial
guard); a real print is the owner's later try (D11).

## 2. The top edge (ours; "Step 3c: the LabelWriter 450" appended to `docs/agent-engine.md`, items W1, W2…, "to approve (proxy)")

- `Order::Job { order, file }` (a PNG): the driver's print of that label; the label size from
  `JobOrder.label_length_mm` / the media the PPD names (never guessed; if the job gives none, the
  PPD's default, as CUPS would use it). `JobEnd` done, or failed with the driver's own error.
- `Order::Command` with any `Action`: failed `CommandDone`, "the DYMO driver has no such control".
- `Control` / `Answer`: fail loudly (the driver has none).
- Telemetry: only what the language monitor reads during a print; no polling.
- `Gone`/`Back` at print time only, as the backend sees the printer.

## 3. Wiring

`crates/agent/src/ports.rs`: `open` returns this port for `--mode labelwriter-450` (only add
lines). The download (B2) already handles binary files.

## 4. Proof

The driver's own tests (`src/lw/tests/`: `TestLabelWriterDriver.cpp`, `TestLabelWriterFilter.cpp`,
`TestLabelWriterLanguageMonitor.cpp`, with `MOCK_LWLMPrintEnvironment`): port them, each case.
Then build the real driver's filter (`raster2dymolw`) on this Mac if it compiles (it needs CUPS
headers; macOS has CUPS) and run it on the same rasters as ours (CUPS raster files made from the
same PNGs with CUPS's own tools if available), comparing the bytes it writes; if it cannot be
built here, say exactly why and rely on the ported tests. Write
`docs/audits/dymo-labelwriter-450.md` in the audit format (Result: identical or exactly where it
differs; a "not covered" list) and update its row in `docs/audits/README.md` (row 7; current result
only).

## 5. Old code (B8)

Touch only `crates/label/src/labelwriter.rs` and its `lib.rs` mod line: remove its device code
that the hub does not use; keep what the hub uses and list it (the hub side waits). Do not touch
`dymo.rs`, `crates/hub` or `profiles.rs`. No adapters.

`cargo test --workspace` (your target dir) must pass; no new warnings.

## Report (brief)

Files; driver items ported with file:line; PPD values used; the runtime stand-ins; W1…; the proof
result; what was removed; anything left open.

## Additions from the watchdog (binding)

1. **CUPS as the driver's runtime** (as pyusb/Pillow are labelle's): fetch the newest OpenPrinting
   CUPS 2.4.x release tag into `reference/cups-<version>/` (only the files used: the raster reader
   `cupsRasterReadHeader2`/`cupsRasterReadPixels`, `cupsBackChannelRead`, the `usb` backend
   `backend/usb-libusb.c`, and the PPD/option functions the filter calls: `ppdMarkDefaults`,
   `cupsMarkOptions`, `cupsParseOptions`), name the tag and commit, and port what is used from
   there with file:line. Never write CUPS behaviour from memory. List it as "runtime, to approve
   (proxy)".
2. **The filter's contract** (from `CupsFilter.h`, `CupsFilterLabelWriter.cpp`): argv `job-id user
   title copies options [file]`; env `PPD`, `DEVICE_URI` (the monitor skips status unless `usb://`,
   `LabelWriterLanguageMonitor.cpp` ~80-90); header fields read: `cupsBytesPerLine`,
   `cupsBitsPerPixel`, `cupsHeight`, `cupsMediaType` (0/1), `PageSize[1]`, `HWResolution[1]`,
   `cupsInteger[0]`; `cupsWidth` is not read.
3. **Page geometry and colour (ours, from the PPD):** the PNG becomes the page CUPS would make for
   the chosen `PageSize` at the chosen resolution, cropped/offset by the PPD's `ImageableArea`
   (`lw450.ppd` ~279ff); write that rule down with the PPD lines; a PNG that does not fit fails the
   job (no scaling of ours unless CUPS's own does it, then port it). The default path is
   `DymoHalftoning ErrorDiffusion` → `cupsColorSpace 1`, 8 bits per colour: the filter gets RGB
   lines and halftones them itself. `Default` is 1 bit black-and-white. Port all three halftoning
   choices the 450's PPD offers (Default, ErrorDiffusion, NLL).
4. **Resolution trap:** `ProcessPPDOptions` (`CupsFilterLabelWriter.cpp` 26-36) only matches
   "203dpi"/"203x138dpi", so for the 450 (`300dpi`, `300x600dpi`) `SetResolution` is never called
   and the resolution enters only through `HWResolution[1]`. Port exactly that; do not "fix" it.
5. **Back channel:** `cupsBackChannelRead(buf, 16, 2.5)` (`CupsPrintEnvironment.cpp:139`); the
   monitor's 10 s `ReadStatusTimeout` (`LabelWriterLanguageMonitor.cpp:47`) and `PollUntilPaperIn`
   (~225-250, 0.2 s sleeps until paper or `jsDeleted`): an engine Stop maps to `jsDeleted` (ours),
   or the job never ends. The monitor's `ESC A` requests happen only inside a print. Writes go
   exactly as the driver writes them; no chunking of ours (none of the old code's 16 KiB chunks,
   status timing, idle polling or orientation guess).
6. **More files:** `common/CommonTypedefs.h`, `common/PrinterDriver.h`, `common/DummyLanguageMonitor.*`,
   `src/lw/LabelWriterDriver.h` (`CLabelWriterDriver400`), and the tests in `src/common/tests/`.
   Not read: `src/lm/` (the LabelManager driver) and `lm450.ppd`.
7. **Separate from the PnP:** no `use` of `crate::ports::labelle` and none of its `nusb_backend.rs`,
   `utils.rs`, `constants.rs`; "png/nusb as the PnP" means the same crate choice, not shared code.
8. **Design first:** write "Step 3c" with W1… before the top-edge code.
