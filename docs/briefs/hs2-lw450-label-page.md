# Brief: HS2 (LabelWriter 450 only) — the hub draws the page the 450's port takes, on its own page

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust; never commit unless told).
PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`.
Owner, 2026-11-17: "QR en barcode er ook in, dan HS2 bouwen", after reviewing the design
https://claude.ai/artifact/8BJZKGpNjQzwJsJ6GPNSWU (roll list, preview on scale with PPD paper and
imageable area, 11353 as two labels, text/size/align, QR/Code 128/EAN-13, copies, print). Also: "de
450 (320 enz) hebben dus een andere pagina nodig dan de pnp". HS1 is done (uncommitted): the
hub's 450 media are the PPD's die-cut `PageSize` choices from `ports::geometry::lw450`.

Why: every hub label to a 450 fails today. The port (`engine/src/ports/dymo_cups/page.rs`, W2)
takes a PNG of exactly the chosen page's raster (`cupsWidth` × `cupsHeight` dots, the imageable
area at 300 dpi) and chooses the page by `label_length_mm` and that size. The hub draws PnP-style:
width = the label's length, height = across, `media_dots` = full mm at 300 dpi capped at 672, an
invented 1.5 mm edge (`label.js` `spacing`), and `Label::from_png` refuses more than 1000 dots high.

Scope: the LabelWriter 450 only; the PnP page, `label.js` and the PnP's paths stay as they are
(separate stacks). The 320 comes later with its own PPD.

1. **The page the port takes (wire side, proven).** A 450 label file is a PNG of exactly the loaded
   roll's page as `ports::geometry::lw450` gives it: PNG width = `dots[0]` (`cupsWidth`, across the
   head), height = `dots[1]` (`cupsHeight`), row 0 first, as the port hands it to `raster2dymolw`
   unchanged. No rotation or scaling anywhere in hub, agent or port. The job keeps sending only
   `label_length_mm` (the roll's `length_mm`), as today. Which edge of the page leaves the
   printer first is not yet verified: it is found by a
   hardware print (item 6) and only then drawn in the preview's "as it comes out" view.
2. **The hub's file and check.** `gcode.rs` `read_label`: also record the PNG's own size
   (e.g. `png_dots: [w, h]`) so a 450 page (up to 3066 dots high) is read; the PnP's `label_dots`
   and `Label::from_png` stay as they are for the PnP. `files.rs` `label_check` branches on a `dymo-lw`
   profile, the PnP path byte for byte unchanged (a test for that); for the 450: the PNG size must equal the loaded roll's facade `dots` exactly ("this page is W × H
   dots; the loaded roll 11353 takes 271 × 254"); `media_dots` and the 672 cap are no longer used
   for the 450.
3. **Its own page** for a `dymo-lw` profile: `pages/labelwriter_page.rs`, `labelwriter.js`,
   `labelwriter.css` (ours, UI), laid out as the design: roll list with search (the facade's 59
   die-cut choices: number, name, W × L mm, choice id), the preview to scale (PaperDimension and
   ImageableArea from the facade), text, size, align, code (None / QR / Code 128 / EAN-13 through
   the hub's existing `GET /api/codes`, a hub service, not device code), copies (the existing `copies` field of
   `files/{id}/print`, 1..=50, extra copies as queued jobs; the filter still gets copies=1, W4),
   print (upload the PNG, then `files/{id}/print` as today). `labelwriter.js` calls `/api/codes`
   itself and imports nothing from `label.js`. The preview shows the page as the PNG is (width
   across), with no feed arrow and no "as it comes out" claim until item 6. `web/machines.rs` picks this page for `dymo-lw`,
   the PnP page otherwise. The roll choice is the existing media form (`{id}/media`). The PNG is
   drawn at exactly `dots[0]` × `dots[1]`, content inside it (it is the imageable area already:
   no own margins).
4. **11353 (ours, UI only).** The owner says 11353 is two labels of 24 × 12 mm side by side on the
   one page. The page offers "same / different" and draws two halves of the page across the head;
   marked as the owner's information in the UI. Nothing of it reaches the wire: it is one page.
5. **Not in HS2:** continuous rolls (Banner, Continuous Wide); the 320; persistence or anything of
   HS3+.
6. **Hardware proof — ONLY THE OWNER (the porter builds and tests, does not deploy):** an agent with `--mode labelwriter-450` on the Pi .67 next to
   the PnP's agent (own config), the hub test build on .67, a label printed from the new page with
   an orientation mark ("TOP", an arrow) on 11353: the owner says which edge came out first and
   whether the code scans. Deploying to .67 and the agent's second service only as test builds.

Questions for the proxy:
- Q1 Copies: the design has a copies field. Does the hub's print take copies for label printers
  today (as `files.rs` copies 2–50 for other jobs), or is it a new UI rule? If new: in or out of HS2?
- Q2 The preview's "as it comes out" orientation before item 6 is known: show the page as the PNG
  is (width across) with no claim, or leave the preview's arrow out until the hardware print?
- Q3 `/api/codes` (QR at EC L, Code 128, EAN-13) is the hub's own service the PnP page uses; the
  450 page calling it is sharing hub UI code between the two stacks, not device code. Acceptable?

`cargo test --workspace` passes; no new warnings. Tests: a PNG of every die-cut roll's facade
size passes `label_check` with that roll loaded and the job order carries its `length_mm`; a PNG
one dot off fails with the message; the PnP's label tests unchanged; `web/machines.rs` serves the
450 page for a `dymo-lw` machine and the PnP page otherwise.

Decided (proxy, 2026-11-17): acceptable with the corrections folded in above. Q1 copies: in, the
existing field, nothing new. Q2: the page as the PNG is, no arrow until the hardware print. Q3:
`/api/codes` acceptable as a hub service, called directly by `labelwriter.js`.
Watchdog before the step (2026-11-17): ON TRACK. Its points: the code parameters (QR EC L, Code 128,
EAN-13) are the hub's own, UI only — owner's approval asked; copies and the 11353 split never
change what the filter gets; `media_dots` and the 672 cap stay only while the PnP uses them, no
adapter for the 450.

Watchdog after the step (2026-11-17): ON TRACK. Wire clean (a PNG of the facade's dots and
`label_length_mm` only; the 11353 split and copies UI-only). Points: the preview's placement of the
page inside the label assumes the raster's first row is the PPD's top — now marked NOT YET VERIFIED
in `labelwriter.js`; a PNG over 1000 dots high is now kept as a label with no `label_dots` (it was kept
with no kind before; the PnP still says "not a label"); the page's own UI numbers (4 % padding,
barcode 3-dot module and quiet zones, QR fit, threshold 128, text metrics, S/M/L, preview scale) and the
"Turn" control (not in the brief) wait for the owner's approval.

Owner, 2026-11-17: "akkoord, draaien houden, zet de printtest klaar": the page's UI numbers approved
as built, the Turn control kept, the hardware print (item 6) to be set up on .67 as a test build.

Hardware print (owner, 2026-11-17, 11353, Turn 90, QR "hs2-test" + text): both labels printed, side by
side across the head, each with its own half of the page; the QR scans. Orientation seen on the
printer: the page's first row leaves first, its width runs left to right across the head seen from
the front, so the preview as drawn (PNG as is) is the label as it comes out; the arrow is added.
Owner: "de text staat te veel naar beneden" (the content sits toward the trailing end) — open.

Owner, 2026-11-17: "allebei, A standaard en B erbij": the content centred on the label (PaperDimension, from the PPD) by default, plus a Shift control (0.25 mm steps along the label, kept per printer and roll in the browser); both UI only.

Owner, 2026-11-17, after a print with "Different": each label of a pair now gets its own code (its own text when "What it encodes" is empty; was: one code from the first text, a bug). Content still not right across the label (right label about 2.5 mm, left about 1.4 mm too far left on the photo): the Shift got left/right too (0.25 mm steps, per printer and roll, UI only).
