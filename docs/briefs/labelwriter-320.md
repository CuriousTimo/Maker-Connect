# Brief: the DYMO LabelWriter 320 on the existing LabelWriter stack (DYMO's CUPS driver 1.4.0)

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust workspace; never commit, never push).
PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`. Build in the normal
`target/` (no `target-*` folders). Read `CLAUDE.md` first: ONE reference, port don't paraphrase,
invent nothing, prove it mechanically. Then read `docs/briefs/labelwriter-450.md`,
`docs/audits/dymo-labelwriter-450.md` and "Step 3c" in `docs/plans/agent-engine.md`: the 320 is
added to that port, same reference, same rules.

## The reference

DYMO's own CUPS driver 1.4.0, unpacked in `reference/dymo-cups-drivers-1.4.0` (see the 450 brief
for the tarball's sha256 and licence). Nothing else: **not** `reference/dymo-connect-*` (its
`Labels.xml` would be a second source), not `reference/labelwriter`, not the old
`crates/label/`.

The 320's settings come from `ppd/lw320.ppd`: `*ModelName` "DYMO LabelWriter 320",
`*cupsModelNumber` 0, 300 dpi, media sizes (the owner has **99012 Large Address**, `w102h252.1`,
loaded), imageable areas, defaults. Cite file:line for every value. `lw300`, `lw310`, `lw315`,
`lw330`, `lw330t` are not covered.

**The dispatch differs from the 450, port it exactly.** `src/lw/raster2dymolw.cpp:78-117` picks the
driver by `ppd->modelname`. The 450 goes to `CLabelWriterDriver400` (lines 92-98); the 320 is in no
list and falls to the last `else`: the base class `CLabelWriterDriver` with
`CDriverInitializerLabelWriterWithLM` / `CDriverInitializerLabelWriter`. `ProcessPPDOptions`
(`src/lw/CupsFilterLabelWriter.cpp:66-77`) has no 320 branch (so no `SetMaxPrintWidth`). Port
whatever of the base class the 450 port does not yet carry (where `CLabelWriterDriver400`
overrides it), function by function with file:line, and select the class by model name as the
filter does.

## Step 0: design first (before any code)

Append "Step 3d: the LabelWriter 320" to `docs/plans/agent-engine.md`, in the form of Step 3c, items
W-numbered and marked "to approve (proxy)". It lists every choice of ours, at least:
- the agent mode `labelwriter-320` (and add it to B4's `--mode` list, ~line 310);
- the DEVICE_URI model string (`usb://DYMO/LabelWriter%20320`) the ported `find_device` matches:
  the 450 path matches manufacturer and model **strings** from the URI (`usb_backend.rs`, ported
  from `usb-libusb.c:820ff`), not vendor/product ids. Follow that path and change only the URI's
  model string; do not add id matching;
- the geometry rule and the hub's media list (below);
- any telemetry text.
Also remove the 320 from the plan's "not covered" models (~line 520) only in that new section's terms.
Then build.

## What to do

1. **Engine** (`crates/engine/src/ports/dymo_cups/`): generate `cups/ppd_lw320.rs` the same way
   `cups/ppd_lw450.rs` is made (same generator/script; keep `#[rustfmt::skip]` on its `mod` line);
   the base-class driver path and the model-name dispatch as above.
2. **Agent**: mode `labelwriter-320` beside `labelwriter-450` in `crates/agent/src/link.rs`
   (`Mode`, parsing, error texts, the mode test table) and `crates/agent/src/ports.rs`.
3. **Hub**, only this: a 320 can be added as a machine; its label page offers **one entry per
   PageSize in `lw320.ppd`**, no DYMO Connect catalog and no rolls (the catalog was approved by the
   owner for the 450 only). `profiles.rs` (~206) and `geometry.rs` are hard-wired to the 450
   (`mod lw450`, `lw450_media()`): make them take the model, nothing more. 99012 and 30321 have the
   same PageSize (102×252); the W2 rule "equal choices are the same page" covers them. List every
   difference from the 450's label page as a question for the owner; build nothing beyond this.
4. **Proof**: extend `scripts/dymo-labelwriter-450-check.py` (one checker per reference; rename
   nothing) and add `docs/audits/dymo-labelwriter-320.md` with its row in `docs/audits/README.md`:
   rows extracted from `lw320.ppd` and the 320's dispatch path, a test per row, and the real
   `raster2dymolw` run on the **320's PPD** on the same rasters as ours, comparing bytes, so the
   base-class path is covered. Result per row: identical, or exactly where it differs. A "not
   covered" list. No history.
5. `scripts/check.sh` passes.

No hardware: do not touch the Pi, do not print. The owner prints the first 99012 label later.

## Report back

What changed (files), the audit result summary, every "ours" item (W-items), anything in the
reference you could not place, and open questions. Keep it short.
