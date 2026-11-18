# DYMO LabelWriter 320: our port against DYMO's CUPS driver

**The one reference:** DYMO's own CUPS driver 1.4.0 (`dymo-cups-drivers-1.4.0`, the maker's own software,
official tarball from download.dymo.com, sha256 `c60797e7e986ca329f46e9a6ab1cb6382383952b15685ed69fd91f3c7ed64f71`,
GPL-2 "or any later version", Copyright (C) Sanford L.P., taken as GPL-3 so it combines with our AGPL-3.0), in
`reference/dymo-cups-drivers-1.4.0`: the filter `raster2dymolw` for the LabelWriter 320 (`ppd/lw320.ppd`,
`*FileVersion` 3.0). Its runtime is CUPS 2.4.20 (`ef80eea`), `reference/cups-2.4.20`. Ours: the same port as the
450's (`crates/engine/src/ports/dymo_cups/`, `docs/audits/dymo-labelwriter-450.md`), with the 320's PPD
(`cups/ppd_lw320.rs`) and the driver class the filter's dispatch gives the 320 (`docs/plans/agent-engine.md`,
Step 3f, W320-1 to W320-6). Checker: `scripts/dymo-labelwriter-450-check.py` (one checker for the reference).

## Result

- **The real filter against the port on the 320's PPD: identical** in **16 of 16** cases (stdout bytes, stderr
  lines, exit status; the no-printer case without the stderr lines and the repeated status requests, as in the
  450's audit, section 3) and in **57 of 57** `PageSize` choices for the page header (CUPS's `cupsRasterInterpretPPD`
  on `lw320.ppd` against the port's). The real filter takes the base class `CLabelWriterDriver` for this PPD
  (section 1); the port takes it by the same model name.
- Every dispatch and PPD row of sections 1 and 2 is extracted from the sources and held by `check`.
- **Not tried on a printer** (the owner's first 99012 label). The 320's IEEE-1284 device ID, read from the
  owner's printer: `MFG:DYMO;CMD: ;MDL:LabelWriter 320;CLASS:PRINTER;DESCRIPTION:DYMO LabelWriter 320;`;
  the port looks for `DYMO` / `LabelWriter 320` (W320-2), which matches. Its `CMD:` is empty.

Run: `python3 -I scripts/dymo-labelwriter-450-check.py check` (the PPD data of both models, the driver's source
facts, and the generated tables of both audits), and `... proof` (needs `cargo build -p maker-connect-engine
--example dymo_cups_proof`; builds the real filter and a helper on libcups, runs the 450 and then the 320, about 2
minutes).

## 1. The dispatch path of the 320: `raster2dymolw.cpp`

The filter picks its driver by `ppd->modelname` (`strcasecmp`): the table lists every branch as extracted from the
source and where the 320's name falls.

<!-- generated dispatch: begin -->
| Row | Reference | Ours | Result |
|---|---|---|---|
| dispatch branch `CLabelWriterDriverTwinTurbo` | `src/lw/raster2dymolw.cpp:78`, names: `DYMO LabelWriter Twin Turbo`, `DYMO LabelWriter 450 Twin Turbo` | `raster2dymolw.rs` `main` | not the 320's; identical (the 320's name `DYMO LabelWriter 320` is in no list) |
| dispatch branch `CLabelWriterDriver400` | `src/lw/raster2dymolw.cpp:92`, names: `DYMO LabelWriter 400`, `DYMO LabelWriter 400 Turbo`, `DYMO LabelWriter DUO Label`, `DYMO LabelWriter 4XL`, `DYMO LabelWriter 450`, `DYMO LabelWriter 450 Turbo`, `DYMO LabelWriter 450 DUO Label` | `raster2dymolw.rs` `main` | not the 320's; identical (the 320's name `DYMO LabelWriter 320` is in no list) |
| dispatch branch `CLabelWriterDriver` | `src/lw/raster2dymolw.cpp:111`, every other name (the last `else`) | `raster2dymolw.rs` `main` | **taken by the 320**; identical (the 320's name `DYMO LabelWriter 320` is in no list) |
| the 320's name | `lw320.ppd:42` `*ModelName` "DYMO LabelWriter 320" | `ppd_lw320.rs` `MODEL_NAME` | identical |
| `ProcessPPDOptions` width branches | `src/lw/CupsFilterLabelWriter.cpp:66-74` (300, 310, 315, 4XL, SE450; no 320) | `cups_filter_label_writer.rs` `process_ppd_options` | identical: `SetMaxPrintWidth` is not called for the 320, the driver keeps `MaxPrintWidth_(84)` (`LabelWriterDriver.cpp:38`) |
| base class `EndDoc` | `src/lw/LabelWriterDriver.cpp:58` | `label_writer_driver.rs` `Class::Driver`: nothing | identical (section 2, the bytes) |
| base class `EndPage` | `src/lw/LabelWriterDriver.cpp:78` | `label_writer_driver.rs` `Class::Driver`: `ESC E` | identical (section 2, the bytes) |
| the 400's `EndDoc` (not the 320's) | `src/lw/LabelWriterDriver.cpp:591` | `Class::Driver400` only | not used for the 320 |
| the 400's `EndPage` (not the 320's) | `src/lw/LabelWriterDriver.cpp:597` | `Class::Driver400` only | not used for the 320 |
<!-- generated dispatch: end -->

The driver's own tests of the base class (`LabelWriterDriverTest` builds `CLabelWriterDriver`, not the 400) are
ported and pass (`docs/audits/dymo-labelwriter-450.md`, section 2). The port's own tests for the 320
(`crates/engine/src/ports/dymo_cups/tests.rs`): `the_320_ppd_has_its_values`,
`the_dispatch_gives_the_320_the_base_class`, `top_edge::a_320_job_takes_the_base_class`; the hub's:
`profiles::lw320_tests::the_320_has_one_entry_per_page_size_and_no_rolls`,
`web::machines::tests::a_labelwriter_320_offers_its_page_sizes`.

## 2. The PPD: `ppd/lw320.ppd`

Every value the filter and CUPS's option marking read, with its line; `ppd_lw320.rs` is generated from the PPD and
`check` fails when it differs.

<!-- generated ppd: begin -->
| Row | `lw320.ppd` | `ppd_lw320.rs` | Result |
|---|---|---|---|
| `*cupsModelNumber` | `lw320.ppd:32` 0 | not read by the filter | not used |
| option `PageSize`: default | `lw320.ppd:149` `w167h288` | `OPTIONS` `default_choice` | identical |
| option `PageRegion`: default | `lw320.ppd:211` `w167h288` | `OPTIONS` `default_choice` | identical |
| option `Resolution`: default `300dpi`, choices `300dpi` | `lw320.ppd:401` (choices :402-402) | `OPTIONS` | identical |
| option `DymoHalftoning`: default `ErrorDiffusion`, choices `Default`, `ErrorDiffusion`, `NLL` | `lw320.ppd:407` (choices :408-410) | `OPTIONS` | identical |
| option `DymoPrintDensity`: default `Normal`, choices `Light`, `Medium`, `Normal`, `Dark` | `lw320.ppd:461` (choices :462-465) | `OPTIONS` | identical |
| option `DymoPrintQuality`: default `Text`, choices `Text`, `Graphics` | `lw320.ppd:525` (choices :526-527) | `OPTIONS` | identical |
| `*cupsIPPReason` `com.dymo.out-of-paper-error` | `lw320.ppd:47` "Out of labels." | `REASONS` | identical |
| `*cupsIPPReason` `com.dymo.read-error` | `lw320.ppd:48` "Cannot read data from printer." | `REASONS` | identical |
| `*cupsIPPReason` `com.dymo.ready` | `lw320.ppd:49` "Printer is ready." | `REASONS` | identical |
| `*cupsIPPReason` `com.dymo.general-error` | `lw320.ppd:50` "General print error." | `REASONS` | identical |
| `*cupsIPPReason` `com.dymo.head-overheat-error` | `lw320.ppd:51` "Print head is overheated." | `REASONS` | identical |
| `*cupsIPPReason` `com.dymo.slot-status-error` | `lw320.ppd:52` "Label path is blocked." | `REASONS` | identical |
| `*cupsIPPReason` `com.dymo.busy-error` | `lw320.ppd:53` "The printer is currently in use." | `REASONS` | identical |
| `*cupsIPPReason` `com.dymo.paper-size-error` | `lw320.ppd:54` "Label being printed does not match label width currently in printer." | `REASONS` | identical |
| `*cupsIPPReason` `com.dymo.paper-size-undefine-error` | `lw320.ppd:55` "Insert a label cassette in the printer." | `REASONS` | identical |
| `PageSize` `w154h64` 11351 Jewelry Label | `lw320.ppd:150` `PageSize[154 64]`, `PaperDimension` :331 [153.6, 63.12], `ImageableArea` :272 [0.0, 4.32, 150.72, 58.8] | `SIZES` | identical; page 628 x 227 dots (header identical, section 3) |
| `PageSize` `w72h154` 11352 Return Address Int | `lw320.ppd:151` `PageSize[72 154]`, `PaperDimension` :332 [72.0, 153.12], `ImageableArea` :273 [4.08, 4.32, 69.12, 146.64] | `SIZES` | identical; page 271 x 593 dots (header identical, section 3) |
| `PageSize` `w72h72` 11353 Multi-Purpose | `lw320.ppd:152` `PageSize[72 72]`, `PaperDimension` :333 [72.0, 72.0], `ImageableArea` :274 [4.08, 4.32, 69.12, 65.28] | `SIZES` | identical; page 271 x 254 dots (header identical, section 3) |
| `PageSize` `w162h90` 11354 Multi-Purpose | `lw320.ppd:153` `PageSize[162 90]`, `PaperDimension` :334 [162.0, 90.0], `ImageableArea` :275 [2.88, 4.32, 159.12, 85.68] | `SIZES` | identical; page 651 x 339 dots (header identical, section 3) |
| `PageSize` `w54h144` 11355 Multi-Purpose | `lw320.ppd:154` `PageSize[54 144]`, `PaperDimension` :335 [54.0, 144.0], `ImageableArea` :276 [4.08, 4.32, 51.12, 127.68] | `SIZES` | identical; page 196 x 514 dots (header identical, section 3) |
| `PageSize` `w118h252` 11356 White Name badge | `lw320.ppd:155` `PageSize[118 252]`, `PaperDimension` :336 [117.12, 252.0], `ImageableArea` :277 [4.08, 4.32, 114.24, 235.44] | `SIZES` | identical; page 459 x 963 dots (header identical, section 3) |
| `PageSize` `w167h188` 14681 CD/DVD Label | `lw320.ppd:156` `PageSize[167 188]`, `PaperDimension` :337 [166.32, 187.2], `ImageableArea` :278 [4.08, 17.04, 161.28, 170.4] | `SIZES` | identical; page 655 x 639 dots (header identical, section 3) |
| `PageSize` `w79h252` 30252 Address | `lw320.ppd:157` `PageSize[79 252]`, `PaperDimension` :338 [78.96, 252.0], `ImageableArea` :279 [4.32, 4.32, 76.08, 235.44] | `SIZES` | identical; page 299 x 963 dots (header identical, section 3) |
| `PageSize` `w167h252` 30253 Address (2 up) | `lw320.ppd:158` `PageSize[167 252]`, `PaperDimension` :339 [166.32, 252.0], `ImageableArea` :280 [4.08, 4.32, 163.44, 235.68] | `SIZES` | identical; page 664 x 964 dots (header identical, section 3) |
| `PageSize` `w167h288` 30256 Shipping | `lw320.ppd:159` `PageSize[167 288]`, `PaperDimension` :340 [166.56, 288.0], `ImageableArea` :281 [4.08, 4.32, 163.68, 271.2] | `SIZES` | identical; page 665 x 1112 dots (header identical, section 3) |
| `PageSize` `w154h198` 30258 Diskette | `lw320.ppd:160` `PageSize[154 198]`, `PaperDimension` :341 [153.12, 198.0], `ImageableArea` :282 [4.08, 4.32, 150.24, 182.16] | `SIZES` | identical; page 609 x 741 dots (header identical, section 3) |
| `PageSize` `w82h248` 30277 File Folder (2 up) | `lw320.ppd:161` `PageSize[82 248]`, `PaperDimension` :342 [81.12, 247.44], `ImageableArea` :283 [4.32, 4.32, 78.24, 233.04] | `SIZES` | identical; page 308 x 953 dots (header identical, section 3) |
| `PageSize` `w154h64.1` 30299 Jewelry Label (2 up) | `lw320.ppd:162` `PageSize[154 64]`, `PaperDimension` :343 [153.6, 63.12], `ImageableArea` :284 [0.0, 4.32, 150.72, 58.8] | `SIZES` | identical; page 628 x 227 dots (header identical, section 3) |
| `PageSize` `w79h252.1` 30320 Address | `lw320.ppd:163` `PageSize[79 252]`, `PaperDimension` :344 [78.96, 252.0], `ImageableArea` :285 [4.32, 4.32, 76.08, 235.44] | `SIZES` | identical; page 299 x 963 dots (header identical, section 3) |
| `PageSize` `w102h252` 30321 Large Address | `lw320.ppd:164` `PageSize[102 252]`, `PaperDimension` :345 [101.28, 251.04], `ImageableArea` :286 [4.32, 4.32, 98.4, 234.96] | `SIZES` | identical; page 392 x 961 dots (header identical, section 3) |
| `PageSize` `w154h286` 30323 Shipping | `lw320.ppd:165` `PageSize[154 286]`, `PaperDimension` :346 [153.12, 285.84], `ImageableArea` :287 [4.32, 4.32, 150.24, 270.48] | `SIZES` | identical; page 608 x 1109 dots (header identical, section 3) |
| `PageSize` `w154h198.1` 30324 Diskette | `lw320.ppd:166` `PageSize[154 198]`, `PaperDimension` :347 [153.12, 198.0], `ImageableArea` :288 [2.88, 4.32, 150.24, 182.16] | `SIZES` | identical; page 614 x 741 dots (header identical, section 3) |
| `PageSize` `w54h424` 30325 Video Spine | `lw320.ppd:167` `PageSize[54 424]`, `PaperDimension` :348 [54.0, 423.12], `ImageableArea` :289 [2.88, 4.32, 51.12, 407.28] | `SIZES` | identical; page 201 x 1679 dots (header identical, section 3) |
| `PageSize` `w131h221` 30326 Video Top | `lw320.ppd:168` `PageSize[131 221]`, `PaperDimension` :349 [130.56, 220.56], `ImageableArea` :290 [4.08, 4.32, 127.68, 204.72] | `SIZES` | identical; page 515 x 835 dots (header identical, section 3) |
| `PageSize` `w57h248` 30327 File Folder | `lw320.ppd:169` `PageSize[57 248]`, `PaperDimension` :350 [56.4, 247.44], `ImageableArea` :291 [0.0, 4.32, 53.52, 231.6] | `SIZES` | identical; page 223 x 947 dots (header identical, section 3) |
| `PageSize` `w54h144.1` 30330 Return Address | `lw320.ppd:170` `PageSize[54 144]`, `PaperDimension` :351 [54.0, 144.0], `ImageableArea` :292 [4.08, 4.32, 51.12, 127.68] | `SIZES` | identical; page 196 x 514 dots (header identical, section 3) |
| `PageSize` `w72h72.1` 30332 1 in x 1 in | `lw320.ppd:171` `PageSize[72 72]`, `PaperDimension` :352 [72.0, 72.0], `ImageableArea` :293 [4.08, 4.32, 69.12, 64.8] | `SIZES` | identical; page 271 x 252 dots (header identical, section 3) |
| `PageSize` `w72h72.2` 30333 1/2 in x 1 in (2 up) | `lw320.ppd:172` `PageSize[72 72]`, `PaperDimension` :353 [72.0, 72.0], `ImageableArea` :294 [4.08, 4.32, 69.12, 65.28] | `SIZES` | identical; page 271 x 254 dots (header identical, section 3) |
| `PageSize` `w162h90.1` 30334 2-1/4 in x 1-1/4 in | `lw320.ppd:173` `PageSize[162 90]`, `PaperDimension` :354 [162.0, 90.0], `ImageableArea` :295 [2.88, 4.32, 159.12, 85.68] | `SIZES` | identical; page 651 x 339 dots (header identical, section 3) |
| `PageSize` `w73h86` 30335 1/2 in x 1/2 in (4 up) | `lw320.ppd:174` `PageSize[73 86]`, `PaperDimension` :355 [72.96, 85.44], `ImageableArea` :296 [0.0, 4.32, 70.08, 81.12] | `SIZES` | identical; page 292 x 320 dots (header identical, section 3) |
| `PageSize` `w72h154.1` 30336 1 in x 2-1/8 in | `lw320.ppd:175` `PageSize[72 154]`, `PaperDimension` :356 [72.0, 153.12], `ImageableArea` :297 [4.08, 4.32, 69.12, 146.64] | `SIZES` | identical; page 271 x 593 dots (header identical, section 3) |
| `PageSize` `w118h252.1` 30337 Audio Cassette | `lw320.ppd:176` `PageSize[118 252]`, `PaperDimension` :357 [117.12, 252.0], `ImageableArea` :298 [4.08, 4.32, 114.24, 235.44] | `SIZES` | identical; page 459 x 963 dots (header identical, section 3) |
| `PageSize` `w54h203` 30339 8mm Video (2 up) | `lw320.ppd:177` `PageSize[54 203]`, `PaperDimension` :358 [54.0, 202.56], `ImageableArea` :299 [4.08, 4.32, 51.12, 186.72] | `SIZES` | identical; page 196 x 760 dots (header identical, section 3) |
| `PageSize` `w54h180` 30345 3/4 in x 2-1/2 in | `lw320.ppd:178` `PageSize[54 180]`, `PaperDimension` :359 [54.0, 180.0], `ImageableArea` :300 [4.08, 4.32, 51.12, 163.68] | `SIZES` | identical; page 196 x 664 dots (header identical, section 3) |
| `PageSize` `w36h136` 30346 1/2 in x 1-7/8 in | `lw320.ppd:179` `PageSize[36 136]`, `PaperDimension` :360 [36.0, 135.12], `ImageableArea` :301 [4.08, 4.32, 33.12, 118.8] | `SIZES` | identical; page 121 x 477 dots (header identical, section 3) |
| `PageSize` `w72h108` 30347 1 in x 1-1/2 in | `lw320.ppd:180` `PageSize[72 108]`, `PaperDimension` :361 [72.0, 108.0], `ImageableArea` :302 [4.08, 4.32, 69.12, 91.68] | `SIZES` | identical; page 271 x 364 dots (header identical, section 3) |
| `PageSize` `w65h90` 30348 9/10 in x 1-1/4 in | `lw320.ppd:181` `PageSize[65 90]`, `PaperDimension` :362 [64.8, 90.0], `ImageableArea` :303 [5.04, 4.32, 61.92, 73.68] | `SIZES` | identical; page 237 x 289 dots (header identical, section 3) |
| `PageSize` `w167h288.1` 30364 Name Badge Label | `lw320.ppd:182` `PageSize[167 288]`, `PaperDimension` :363 [166.56, 288.0], `ImageableArea` :304 [4.08, 4.32, 163.68, 271.2] | `SIZES` | identical; page 665 x 1112 dots (header identical, section 3) |
| `PageSize` `w168h252` 30365 Name Badge Card | `lw320.ppd:183` `PageSize[168 252]`, `PaperDimension` :364 [167.04, 252.0], `ImageableArea` :305 [0.0, 4.32, 161.28, 225.12] | `SIZES` | identical; page 672 x 920 dots (header identical, section 3) |
| `PageSize` `w144h169` 30370 Zip Disk | `lw320.ppd:184` `PageSize[144 169]`, `PaperDimension` :365 [144.0, 168.72], `ImageableArea` :306 [4.32, 4.32, 141.12, 152.88] | `SIZES` | identical; page 570 x 619 dots (header identical, section 3) |
| `PageSize` `w71h144` 30373 Price Tag Label | `lw320.ppd:185` `PageSize[71 144]`, `PaperDimension` :366 [70.32, 144.0], `ImageableArea` :307 [0.0, 85.44, 67.44, 139.68] | `SIZES` | identical; page 281 x 226 dots (header identical, section 3) |
| `PageSize` `w144h252` 30374 Appointment Card | `lw320.ppd:186` `PageSize[144 252]`, `PaperDimension` :367 [144.0, 252.0], `ImageableArea` :308 [8.64, 4.32, 141.12, 225.12] | `SIZES` | identical; page 552 x 920 dots (header identical, section 3) |
| `PageSize` `w80h144` 30376 Hanging File Insert | `lw320.ppd:187` `PageSize[80 144]`, `PaperDimension` :368 [79.2, 144.0], `ImageableArea` :309 [8.64, 4.32, 76.32, 121.44] | `SIZES` | identical; page 282 x 488 dots (header identical, section 3) |
| `PageSize` `w162h504` 30383 PC Postage 3-Part | `lw320.ppd:188` `PageSize[162 504]`, `PaperDimension` :369 [162.0, 504.0], `ImageableArea` :310 [4.08, 4.32, 159.12, 488.16] | `SIZES` | identical; page 646 x 2016 dots (header identical, section 3) |
| `PageSize` `w167h540` 30384 PC Postage 2-Part | `lw320.ppd:189` `PageSize[167 540]`, `PaperDimension` :370 [166.56, 540.0], `ImageableArea` :311 [4.08, 4.32, 163.68, 524.16] | `SIZES` | identical; page 665 x 2166 dots (header identical, section 3) |
| `PageSize` `w167h756` 30387 PC Postage EPS | `lw320.ppd:190` `PageSize[167 756]`, `PaperDimension` :371 [166.56, 756.0], `ImageableArea` :312 [4.08, 4.32, 163.68, 740.16] | `SIZES` | identical; page 665 x 3066 dots (header identical, section 3) |
| `PageSize` `w167h188.1` 30854 CD Label | `lw320.ppd:191` `PageSize[167 188]`, `PaperDimension` :372 [166.32, 187.2], `ImageableArea` :313 [4.08, 17.04, 161.28, 170.4] | `SIZES` | identical; page 655 x 639 dots (header identical, section 3) |
| `PageSize` `w176h292` 30856 Badge Card Label | `lw320.ppd:192` `PageSize[176 292]`, `PaperDimension` :373 [175.44, 291.6], `ImageableArea` :314 [0.0, 4.32, 161.28, 264.72] | `SIZES` | identical; page 672 x 1085 dots (header identical, section 3) |
| `PageSize` `w167h288.2` 30857 Badge Label | `lw320.ppd:193` `PageSize[167 288]`, `PaperDimension` :374 [166.56, 288.0], `ImageableArea` :315 [4.08, 4.32, 163.68, 271.2] | `SIZES` | identical; page 665 x 1112 dots (header identical, section 3) |
| `PageSize` `w112h126` 30886 CD Label | `lw320.ppd:194` `PageSize[112 126]`, `PaperDimension` :375 [111.36, 126.0], `ImageableArea` :316 [4.32, 18.0, 102.24, 110.16] | `SIZES` | identical; page 408 x 384 dots (header identical, section 3) |
| `PageSize` `w79h252.2` 99010 Standard Address | `lw320.ppd:195` `PageSize[79 252]`, `PaperDimension` :376 [78.96, 252.0], `ImageableArea` :317 [4.32, 4.32, 76.08, 235.44] | `SIZES` | identical; page 299 x 963 dots (header identical, section 3) |
| `PageSize` `w102h252.1` 99012 Large Address | `lw320.ppd:196` `PageSize[102 252]`, `PaperDimension` :377 [101.28, 251.04], `ImageableArea` :318 [4.32, 4.32, 98.4, 234.96] | `SIZES` | identical; page 392 x 961 dots (header identical, section 3) |
| `PageSize` `w154h286.1` 99014 Name Badge Label | `lw320.ppd:197` `PageSize[154 286]`, `PaperDimension` :378 [153.12, 285.84], `ImageableArea` :319 [4.32, 4.32, 150.24, 270.48] | `SIZES` | identical; page 608 x 1109 dots (header identical, section 3) |
| `PageSize` `w154h286.2` 99014 Shipping | `lw320.ppd:198` `PageSize[154 286]`, `PaperDimension` :379 [153.12, 285.84], `ImageableArea` :320 [4.32, 4.32, 150.24, 270.48] | `SIZES` | identical; page 608 x 1109 dots (header identical, section 3) |
| `PageSize` `w154h198.2` 99015 Diskette | `lw320.ppd:199` `PageSize[154 198]`, `PaperDimension` :380 [153.12, 198.0], `ImageableArea` :321 [2.88, 4.32, 150.24, 182.16] | `SIZES` | identical; page 614 x 741 dots (header identical, section 3) |
| `PageSize` `w63h419` 99016 Video Spine | `lw320.ppd:200` `PageSize[63 419]`, `PaperDimension` :381 [62.4, 418.56], `ImageableArea` :322 [0.0, 4.32, 59.52, 402.24] | `SIZES` | identical; page 248 x 1658 dots (header identical, section 3) |
| `PageSize` `w139h221` 99016 Video Top | `lw320.ppd:201` `PageSize[139 221]`, `PaperDimension` :382 [138.96, 220.56], `ImageableArea` :323 [0.0, 4.32, 136.08, 204.48] | `SIZES` | identical; page 567 x 834 dots (header identical, section 3) |
| `PageSize` `w36h144` 99017 Suspension File | `lw320.ppd:202` `PageSize[36 144]`, `PaperDimension` :383 [36.0, 144.0], `ImageableArea` :324 [2.88, 4.32, 33.12, 127.92] | `SIZES` | identical; page 126 x 515 dots (header identical, section 3) |
| `PageSize` `w108h539` 99018 Small Lever Arch | `lw320.ppd:203` `PageSize[108 539]`, `PaperDimension` :384 [107.76, 538.56], `ImageableArea` :325 [4.08, 4.32, 104.88, 522.72] | `SIZES` | identical; page 420 x 2160 dots (header identical, section 3) |
| `PageSize` `w167h539` 99019 Large Lever Arch | `lw320.ppd:204` `PageSize[167 539]`, `PaperDimension` :385 [166.56, 538.56], `ImageableArea` :326 [4.08, 4.32, 163.68, 522.72] | `SIZES` | identical; page 665 x 2160 dots (header identical, section 3) |
| `PageSize` `w154h7680` Banner | `lw320.ppd:205` `PageSize[154 7680]`, `PaperDimension` :386 [153.12, 7680.0], `ImageableArea` :327 [4.08, 17.04, 148.08, 7663.92] | `SIZES` | identical; page 600 x 31862 dots (header identical, section 3) |
| `PageSize` `w154h792` Continuous, Wide | `lw320.ppd:206` `PageSize[154 792]`, `PaperDimension` :387 [153.12, 792.0], `ImageableArea` :328 [4.32, 4.32, 150.24, 775.68] | `SIZES` | identical; page 608 x 3214 dots (header identical, section 3) |

57 `PageSize` choices, each with its `PageRegion` of the same name and size.
<!-- generated ppd: end -->

## 3. The real filter against the port, on the same pages

The real `raster2dymolw` (built here from the driver's sources untouched, as in the 450's audit) and the port's
filter (`examples/dymo_cups_proof.rs` with `DYMO_PROOF_MODEL=320`) run on the same CUPS raster, made from the same
PNG by CUPS's own `cupsRasterInterpretPPD` on `lw320.ppd`, with `PPD` set to `lw320.ppd` and `DEVICE_URI`
`usb://DYMO/LabelWriter%20320` for both. Compared: the bytes written to stdout, the stderr lines, the exit status.

| Case | Result |
|---|---|
| default size `w167h288`, defaults, status `TOF` | identical |
| default size, defaults, not local (`ipp://`, no status) | identical |
| 99012 Large Address (`w102h252.1`), defaults | identical |
| 30321 Large Address (`w102h252`), defaults (same bytes as 99012) | identical |
| 1 in x 1 in (`w72h72`), defaults | identical |
| jewelry `w154h64`, NLL | identical |
| shipping, Default (1 bit) | identical |
| address `w79h252`, Dark and Graphics | identical |
| `w72h154`, Light density | identical |
| 99012, Medium density | identical |
| continuous, wide (`w154h792`) | identical |
| `w54h144`, Medium density, no `DEVICE_URI` | identical |
| no printer: the back channel never answers (`w72h72`) | identical (stdout bytes without the repeated `ESC A`; exit status) |
| noise picture, ErrorDiffusion, 99012 | identical |
| noise picture, NLL (`w72h154`) | identical |
| noise picture, Default (`w162h90`) | identical |
| page header of each of the 57 `PageSize` choices | identical (several choices of one page differ only in the name `cupsPageSizeName`, which the filter does not read) |

## 4. Ours (not the driver's, not CUPS's): `docs/plans/agent-engine.md`, Step 3f

| # | What | Where |
|---|---|---|
| W320-1 | agent mode `labelwriter-320`; the port with the model as a parameter | `crates/agent/src/link.rs`, `ports.rs`; `port.rs` `LabelWriter::for_model` |
| W320-2 | `DEVICE_URI` `usb://DYMO/LabelWriter%20320`, manufacturer `DYMO`, model `LabelWriter 320` | `port.rs` |
| W320-3 | `ppd_lw320.rs` generated from `lw320.ppd` | `cups/ppd_lw320.rs` |
| W320-4 | the geometry rule of the 450's W2/W2b on the 320's PPD | `page.rs`, `ports/geometry.rs` |
| W320-5 | hub: profile `dymo/labelwriter-320`, one entry per `PageSize`, no catalogue | `profiles/dymo/labelwriter-320.toml`, `crates/hub/src/profiles.rs`, `pages/labelwriter_page.rs` |
| W320-6 | telemetry with the 320's reason texts | `port.rs` |

## 5. Where `lw320.ppd` differs from the 450's PPDs

Against the 1.4.0 driver's own `lw450.ppd` (the same tarball): the 320's has no `*Resolution 300x600dpi`, empty
codes for `*DymoPrintQuality` Text and Graphics (`lw320.ppd:526-527`; the 450's carry `HWResolution`), no
`*UIConstraints`, and 57 `PageSize` choices (the 450's has 61). The 450's port uses DYMO Connect's newer PPD
(FileVersion 3.1; owner, 2026-11-17); the 320 has none, so its values are the 1.4.0 PPD's (FileVersion 3.0), which
the port reads as they are. For `w102h252` (30321) `lw320.ppd:164` has `PageSize[102 252]`; the 450's PPD 3.1 has
`PageSize[102 251]` for the same choice: the 320's 99012 and 30321 are therefore the same page in the 320's PPD.

## 6. Not covered

- The other models of the driver: `lw300`, `lw310`, `lw315`, `lw330`, `lw330t`, the 400, 4XL, SE450, Turbo, DUO,
  Twin Turbo, the LabelManager driver (`src/lm/`).
- Everything the 450's audit lists as not covered (the filter without a PPD or back channel, compressed or
  byte-swapped raster streams, several pages, `DYMO_PRN_DIR`, the status flow of a paper-out against the real
  filter, the Twin Turbo).
- A real 320 on USB: `find_device` against its device ID (empty `CMD:`), the `nusb` stand-in for the `usb` backend, the first
  label.
- The hub's page against a browser (the Label tab's drawing for the 320's sizes is the 450's script).
