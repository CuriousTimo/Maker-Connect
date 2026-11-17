# Brief: HS2b (LabelWriter 450) — DYMO Connect's label catalogue and the newer PPD

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust; never commit unless told).
PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`.

Owner, 2026-11-17: "zoek online voor een beter reference voor die rollen", "ipv zelf lopen klooien
per model", then, after the findings: **"ja, gebruik de catalogus en de nieuwe ppd"**. The owner
downloaded DYMO Connect for Mac (`~/Downloads/DCDMac1.6.4.18-Arm64.pkg`); "niet installeren": it is
only expanded (`pkgutil --expand-full`), nothing of it runs.

Why: the hub split the 11353 page in two by halving the imageable area (our guess); on the printer
the right label's content sat ~2.5 mm too far left. DYMO's own software knows each roll's labels on
its page. And the PPD in DYMO's current macOS driver is newer than ours (FileVersion 3.1 vs 3.0).

Licence: DYMO Connect and its driver are DYMO's (closed). The owner approved using its data
(2026-11-17), as for PrusaLink (2026-11-12): sizes and shapes only, no code.

## Part 1 — the newer PPD (wire side, proven)

1. `reference/dymo-connect-1.6.4.18/`: the pkg's SHA-256 and where it came from (dymo.com, DYMO
   Connect for Desktop 1.6.4.18, Arm64), and the files taken from it unchanged:
   `Library/Printers/PPDs/Contents/Resources/lw450.ppd.gz` (unpacked to `lw450.ppd`). A script
   (`scripts/dymo-connect-extract.py`, standard library, `python3 -I`) does the expanding and
   copying, so it can be redone from the pkg.
2. `scripts/dymo-labelwriter-450-check.py`: `PPD` points at the new file; `gen` regenerates
   `ppd_lw450.rs`; the proof (the real `raster2dymolw` 1.4.0 built here and libcups 2.4.20 on the
   PPD vs the port) runs again on the new PPD: every case and every `PageSize` header identical,
   or the difference named. Write down every line that differs between the two PPDs (the whole
   file, not only sizes) in the audit.
3. `docs/audits/dymo-labelwriter-450.md` and `docs/audits/README.md` name the new PPD and the result.

## Part 2 — the catalogue (hub side, UI)

4. The script also takes from `Contents/MonoBundle/DYMO.LabelAPI.dll` the XML documents embedded in
   it, byte for byte, into the reference folder: `Labels` (DieCutLabel/ContinuousLabel with
   `PaperName` and `DrawCommands`), `SKUs` (DieCutSKU: SKU, PaperName, colours, SearchTerms),
   `SkuLinks`, `Papers` (per PaperGroup). Nothing is edited; the script finds each document by its
   XML declaration and root element and checks it parses.
5. A generated Rust file beside the 450's geometry (`ports/geometry`), checked like `ppd_lw450.rs`
   (`check` fails if it differs from what the XML gives): for each die-cut SKU whose `PaperName` is
   a `PageSize` text of the 450's PPD: the SKU, its `PaperName`, its search terms and the
   `DrawCommands` shapes of the DieCutLabel with that `PaperName` (`RoundRectangle`, `Rectangle`,
   `LineTo`, `CutoutShape` as they are, in twips). SKUs whose paper is not in the PPD are listed
   as not covered.
6. The facade gives, per `PageSize` choice, its label shapes in the page's raster dots: twips
   → points (/20) → the raster (minus the PPD's `ImageableArea` origin at 300 dpi), with the same
   axes as the hub's page (width across the head, the page's first row first; verified on the
   printer 2026-11-17). Which corner is the twips origin is checked against a print (item 9), not
   assumed.
7. The hub's roll list becomes the catalogue's SKUs for the 450 (number, DYMO's own name from
   `SkuLinks`, size), searchable by DYMO's search terms; choosing one keeps the medium id as the
   PPD choice (the job still sends only `label_length_mm`; nothing on the wire changes). The page
   draws one cell per label shape (content centred in each shape, clipped to the page); the
   owner's "OURS" split for 11353 goes (S0722530/11353 is DYMO's paper 30333, two labels of
   720 × 1440 twips side by side).
8. Not in HS2b: continuous rolls; DYMO Connect's own `PrintableSize`/`PrintableOrigin` (its layout
   area, not the driver's; the page sent stays the PPD's imageable area); the 320 and other models
   (the catalogue covers them; each its own step).
9. Hardware proof — ONLY THE OWNER: S0722530 with two different labels and their QR codes; the
   content centred on each label.

Questions for the proxy:
- Q1 The roll list by SKU (DYMO's numbers on the box, EU and US) instead of by PPD choice: one
  entry per SKU, several SKUs on one paper. Agreed?
- Q2 A SKU whose paper has no DieCutLabel shape: draw the whole page as one label, or leave it out?

`cargo test --workspace` passes; no new warnings.

Decided (proxy, 2026-11-17): acceptable with corrections, folded in here.
- The PPD change is a wire change. The two PPDs differ in more than sizes: 30321 `PageSize`/`PageRegion`
  `[102 252]` → `[102 251]`, its `ImageableArea` and `PaperDimension`, 30252's `PaperDimension`,
  `cupsFilter` (the macOS path), `APPrinterIconPath`, and a new option `DymoContinuousPaper`
  (`cupsMediaType 1`). The audit lists every differing line; the Mac `cupsFilter` path is not
  taken over (the agent runs the ported filter); `DymoContinuousPaper` stays out of HS2b's UI.
- The audit says plainly that filter (1.4.0) and PPD (3.1, DYMO Connect 1.6.4.18's macOS driver) come
  from two DYMO packages, by the owner's choice, and where the filter was proven against the new PPD.
- Names only from `SkuLinks` (`Name`, by a fixed, written region order); the `SKUs` names are
  placeholders. A SKU without a link shows its number only. Each SKU number once. Sizes from the
  shapes/PPD, never from a name.
- Q1: one entry per SKU (the number on the box); the medium id stays the PPD choice.
- Q2: a SKU without a DieCutLabel shape is left out, listed as not covered in the audit.
- DYMO Connect's `PrintableSize`/`PrintableOrigin` are not used.

Owner, 2026-11-17: "ja, nummers en namen mogen erin": DYMO's SKU numbers and product names (from SkuLinks) may be in the repo (generated data), beside the sizes and shapes.

Built (2026-11-17). Deviation from item 7, accepted by the watchdog: the medium id is the SKU, not the PPD
choice (several SKUs share one page and each roll button needs its own id); the job still names the PPD
choice (`loaded_page_size` → `labelwriter_page(..).name`). Watchdog after the step: ON TRACK; ours, for
the owner: the region-order fallback, the Path "inside another = hole" rule and the lines-only box. The
twips origin waits for the owner's print (item 9).

Owner, 2026-11-17: "akkoord met alle drie": the region-order fallback, the Path hole rule and the lines-only box approved.

Hardware print (owner, 2026-11-17, S0722530, Turn 90, Different, QR): "doet het" — the content sits on each of the two labels; the twips origin as taken (page left and top edge) holds.
