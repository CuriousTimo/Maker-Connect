# DYMO LabelWriter 450: our port against DYMO's CUPS driver

**The one reference:** DYMO's own CUPS driver 1.4.0 (`dymo-cups-drivers-1.4.0`, the maker's own
software, official tarball from download.dymo.com, sha256
`c60797e7e986ca329f46e9a6ab1cb6382383952b15685ed69fd91f3c7ed64f71`, GPL-2 "or any later version",
Copyright (C) Sanford L.P., taken as GPL-3 so it combines with our AGPL-3.0), in
`reference/dymo-cups-drivers-1.4.0`: the filter `raster2dymolw` for the plain LabelWriter 450
(`ppd/lw450.ppd`). Its runtime is CUPS: OpenPrinting CUPS **v2.4.20, commit `ef80eea`**, in
`reference/cups-2.4.20` (only the files used; `SOURCE.txt` lists them). Ours:
`crates/engine/src/ports/dymo_cups/`, a port file by file and function by function
(`docs/archive/plans/agent-engine.md`, Step 3c, items W1 to W10), with the parts of CUPS the driver runs on ported
beside it (`cups/`). The LabelWriter 450 and the DYMO LabelManager PnP share no code.

## Result

- **The gate** (`als de 450 cups agent mee valt`): **passed.** DYMO's `raster2dymolw` and its tests
  build on this Mac (clang++ 17 on macOS, the SDK's CUPS 2.3.4 headers, cppunit 1.15.1 from Homebrew):
  the 20 tests of `src/lw/tests` and the 6 of `src/common/tests` pass (`OK (20)`, `OK (6)`). Building
  needs, besides the driver's sources untouched: `-include` of `cups/ppd.h`, `cups/raster.h`,
  `cups/backend.h` and `cups/sidechannel.h` (the 2011 sources rely on `cups/cups.h` pulling them in),
  and a stream operator for `std::vector<unsigned char>` in `namespace std` (a current cppunit finds
  the tests' own `operator<<` by lookup rules the old one did not use). The Filter tests read
  `../../../ppd/lwtt.ppd` relative to the tests' directory.
- **Ported tests: 24 of 26 cases pass** (section 2); the 2 not covered are the Twin Turbo's.
- **The real filter against the port, on the same pages: identical** in **14 of 14** cases (stdout bytes,
  the stderr lines, the exit status; the no-printer case without the stderr lines and the repeated status requests, see section 3) and in **61 of 61** `PageSize` choices of the PPD for the page
  header (CUPS's own `cupsRasterInterpretPPD` on the real PPD against the port's) (section 3).
  Deliberate breaks are caught (the error-diffusion weight 5/16 changed to 4/16: 8 of 13 cases then differed).
- **Not tried on a printer** (the owner's later try, D11). The `nusb` stand-in for libusb is not tried
  on the printer; nothing in the tests opens a USB device (the stand-in is not built for tests).
- **Open (hub side, waits):** the hub draws a label as `length x height` dots (`crates/label`), the
  port needs the page CUPS makes for the PPD's size (width across the head, the imageable area, e.g.
  665 x 1112 dots for the default label): a picture of another size fails the job with that message.
  The hub's `dots_for_mm` and `HEAD_DOTS` (`profiles.rs:229-230`) are kept in `crates/label` until
  then.

Run: `python3 -I scripts/dymo-labelwriter-450-check.py check` (the PPD data and the driver's source
facts the port relies on, and this table, regenerated from the sources: it exits 0 only when the file
on disk is what they give), and `... proof` (needs `cargo build -p maker-connect-engine --example
dymo_cups_proof`; builds the real filter and a helper on libcups, about 4 minutes), and
`cargo test -p maker-connect-engine dymo_cups`.

## 1. What goes to the printer: the driver's code, function by function

The table is generated (`... audit`) from the C++ sources: every function definition of the ten files
the port covers, and the doc comment of the port that names it with its lines. A function that is not
in the port is in the script's list with its reason: nothing is left out unnamed.

<!-- generated functions: begin -->
| Driver function | Line | The port | Result |
|---|---|---|---|
| `CLabelWriterDriver::CLabelWriterDriver` | `src/lw/LabelWriterDriver.cpp:35` | ported as `LabelWriterDriver::new` | ported |
| `CLabelWriterDriver::~CLabelWriterDriver` | `src/lw/LabelWriterDriver.cpp:42` | not ported: nothing to do | not covered |
| `CLabelWriterDriver::StartDoc` | `src/lw/LabelWriterDriver.cpp:47` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:371` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::EndDoc` | `src/lw/LabelWriterDriver.cpp:58` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:525` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::StartPage` | `src/lw/LabelWriterDriver.cpp:63` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:533` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::EndPage` | `src/lw/LabelWriterDriver.cpp:78` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:548` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SendNotCompressedData` | `src/lw/LabelWriterDriver.cpp:85` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:134` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SendCompressedData` | `src/lw/LabelWriterDriver.cpp:104` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:154` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::GetBlanks` | `src/lw/LabelWriterDriver.cpp:121` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:168` | ported (tested in section 2, proved in section 3) |
| `GetBitValue` | `src/lw/LabelWriterDriver.cpp:155` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:387` | ported (tested in section 2, proved in section 3) |
| `NextBit` | `src/lw/LabelWriterDriver.cpp:162` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:392` | ported (tested in section 2, proved in section 3) |
| `GetCompressedSequenceValue` | `src/lw/LabelWriterDriver.cpp:177` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:402` | ported (tested in section 2, proved in section 3) |
| `CompressData` | `src/lw/LabelWriterDriver.cpp:206` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:434` | ported (tested in section 2, proved in section 3) |
| `ShiftDataRight` | `src/lw/LabelWriterDriver.cpp:225` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:455` | ported (tested in section 2, proved in section 3) |
| `ShiftDataLeft` | `src/lw/LabelWriterDriver.cpp:244` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:481` | ported (tested in section 2, proved in section 3) |
| `ShiftData` | `src/lw/LabelWriterDriver.cpp:262` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:505` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::ProcessRasterLine` | `src/lw/LabelWriterDriver.cpp:275` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:557` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SendCommand` | `src/lw/LabelWriterDriver.cpp:336` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:266` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SendCommand` | `src/lw/LabelWriterDriver.cpp:342` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:266` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::GetResolution` | `src/lw/LabelWriterDriver.cpp:348` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:205` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::GetDensity` | `src/lw/LabelWriterDriver.cpp:354` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:209` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::GetQuality` | `src/lw/LabelWriterDriver.cpp:360` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:213` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::GetPageHeight` | `src/lw/LabelWriterDriver.cpp:366` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:217` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::GetPaperType` | `src/lw/LabelWriterDriver.cpp:372` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:221` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SetResolution` | `src/lw/LabelWriterDriver.cpp:378` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:225` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SetDensity` | `src/lw/LabelWriterDriver.cpp:384` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:229` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SetQuality` | `src/lw/LabelWriterDriver.cpp:390` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:233` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SetPageHeight` | `src/lw/LabelWriterDriver.cpp:396` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:237` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SetPaperType` | `src/lw/LabelWriterDriver.cpp:402` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:241` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SetMaxPrintWidth` | `src/lw/LabelWriterDriver.cpp:408` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:245` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SetPageOffset` | `src/lw/LabelWriterDriver.cpp:414` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:249` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SendLineTab` | `src/lw/LabelWriterDriver.cpp:420` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:273` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SendDotTab` | `src/lw/LabelWriterDriver.cpp:430` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:279` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SendFormFeed` | `src/lw/LabelWriterDriver.cpp:439` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:285` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SendBytesPerLine` | `src/lw/LabelWriterDriver.cpp:447` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:290` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SendSkipLines` | `src/lw/LabelWriterDriver.cpp:456` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:296` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SendLabelLength` | `src/lw/LabelWriterDriver.cpp:481` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:312` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SendResolution` | `src/lw/LabelWriterDriver.cpp:491` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:318` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SendPrintDensity` | `src/lw/LabelWriterDriver.cpp:514` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:331` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SendPrintQuality` | `src/lw/LabelWriterDriver.cpp:531` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:342` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::GetResetCommand` | `src/lw/LabelWriterDriver.cpp:546` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:351` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::GetRequestStatusCommand` | `src/lw/LabelWriterDriver.cpp:552` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:356` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::GetEmptyLinesCount` | `src/lw/LabelWriterDriver.cpp:560` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:253` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver::SetEmptyLinesCount` | `src/lw/LabelWriterDriver.cpp:566` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:258` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver400::CLabelWriterDriver400` | `src/lw/LabelWriterDriver.cpp:575` | ported as `LabelWriterDriver::new400` | ported |
| `CLabelWriterDriver400::~CLabelWriterDriver400` | `src/lw/LabelWriterDriver.cpp:580` | not ported: nothing to do | not covered |
| `CLabelWriterDriver400::StartDoc` | `src/lw/LabelWriterDriver.cpp:585` | ported as `LabelWriterDriver::start_doc` (calls the base, :584-588) | ported |
| `CLabelWriterDriver400::EndDoc` | `src/lw/LabelWriterDriver.cpp:591` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:525` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver400::EndPage` | `src/lw/LabelWriterDriver.cpp:597` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:548` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver400::GetShortFormFeedCommand` | `src/lw/LabelWriterDriver.cpp:603` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:361` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriver400::SendShortFormFeed` | `src/lw/LabelWriterDriver.cpp:611` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:366` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriverTwinTurbo::CLabelWriterDriverTwinTurbo` | `src/lw/LabelWriterDriver.cpp:622` | not ported: Twin Turbo (not a 450) | not covered |
| `CLabelWriterDriverTwinTurbo::~CLabelWriterDriverTwinTurbo` | `src/lw/LabelWriterDriver.cpp:627` | not ported: Twin Turbo (not a 450) | not covered |
| `CLabelWriterDriverTwinTurbo::StartDoc` | `src/lw/LabelWriterDriver.cpp:632` | not ported: Twin Turbo (not a 450) | not covered |
| `CLabelWriterDriverTwinTurbo::GetRoll` | `src/lw/LabelWriterDriver.cpp:639` | not ported: Twin Turbo (not a 450) | not covered |
| `CLabelWriterDriverTwinTurbo::SetRoll` | `src/lw/LabelWriterDriver.cpp:645` | not ported: Twin Turbo (not a 450) | not covered |
| `CLabelWriterDriverTwinTurbo::GetRollSelectCommand` | `src/lw/LabelWriterDriver.cpp:651` | `crates/engine/src/ports/dymo_cups/label_writer_driver.rs:77` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterDriverTwinTurbo::SendRollSelect` | `src/lw/LabelWriterDriver.cpp:666` | not ported: Twin Turbo (not a 450) | not covered |
| `CLabelWriterLanguageMonitor::CLabelWriterLanguageMonitor` | `src/lw/LabelWriterLanguageMonitor.cpp:34` | ported as `LabelWriterLanguageMonitor::new` | ported |
| `CLabelWriterLanguageMonitor::~CLabelWriterLanguageMonitor` | `src/lw/LabelWriterLanguageMonitor.cpp:39` | not ported: nothing to do | not covered |
| `CLabelWriterLanguageMonitor::StartDoc` | `src/lw/LabelWriterLanguageMonitor.cpp:44` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:333` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterLanguageMonitor::EndDoc` | `src/lw/LabelWriterLanguageMonitor.cpp:54` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:343` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterLanguageMonitor::StartPage` | `src/lw/LabelWriterLanguageMonitor.cpp:61` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:348` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterLanguageMonitor::EndPage` | `src/lw/LabelWriterLanguageMonitor.cpp:73` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:358` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterLanguageMonitor::IsLocal` | `src/lw/LabelWriterLanguageMonitor.cpp:80` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:144` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterLanguageMonitor::SynchronizeRoll` | `src/lw/LabelWriterLanguageMonitor.cpp:93` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:153` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterLanguageMonitor::ResetPrinter` | `src/lw/LabelWriterLanguageMonitor.cpp:100` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:159` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterLanguageMonitor::CheckStatusAndReprint` | `src/lw/LabelWriterLanguageMonitor.cpp:108` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:165` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterLanguageMonitor::ReadStatus` | `src/lw/LabelWriterLanguageMonitor.cpp:178` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:234` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterLanguageMonitor::PollUntilPaperIn` | `src/lw/LabelWriterLanguageMonitor.cpp:230` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:275` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterLanguageMonitor::SetJobStatus` | `src/lw/LabelWriterLanguageMonitor.cpp:261` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:302` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterLanguageMonitor::ReprintLabel` | `src/lw/LabelWriterLanguageMonitor.cpp:274` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:315` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterLanguageMonitor::ProcessData` | `src/lw/LabelWriterLanguageMonitor.cpp:289` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:364` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterLanguageMonitor::SetPaperType` | `src/lw/LabelWriterLanguageMonitor.cpp:295` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:133` | ported (tested in section 2, proved in section 3) |
| `CLabelWriterLanguageMonitor::SetRoll` | `src/lw/LabelWriterLanguageMonitor.cpp:301` | `crates/engine/src/ports/dymo_cups/label_writer_language_monitor.rs:138` | ported (tested in section 2, proved in section 3) |
| `CDriverInitializerLabelWriter::ProcessPageOptions` | `src/lw/CupsFilterLabelWriter.cpp:79` | `crates/engine/src/ports/dymo_cups/cups_filter_label_writer.rs:82` | ported (tested in section 2, proved in section 3) |
| `CDriverInitializerLabelWriterTwinTurbo::ProcessPPDOptions` | `src/lw/CupsFilterLabelWriter.cpp:99` | not ported: Twin Turbo (not a 450) | not covered |
| `CDriverInitializerLabelWriterTwinTurbo::ProcessPageOptions` | `src/lw/CupsFilterLabelWriter.cpp:118` | not ported: Twin Turbo (not a 450) | not covered |
| `CDriverInitializerLabelWriterWithLM::ProcessPageOptions` | `src/lw/CupsFilterLabelWriter.cpp:131` | ported in `process_page_options_with_lm` | ported |
| `CDriverInitializerLabelWriterTwinTurboWithLM::ProcessPPDOptions` | `src/lw/CupsFilterLabelWriter.cpp:139` | not ported: Twin Turbo (not a 450) | not covered |
| `CDriverInitializerLabelWriterTwinTurboWithLM::ProcessPageOptions` | `src/lw/CupsFilterLabelWriter.cpp:146` | not ported: Twin Turbo (not a 450) | not covered |
| `IsBackchannelSupported` | `src/lw/raster2dymolw.cpp:45` | `crates/engine/src/ports/dymo_cups/raster2dymolw.rs:10` | ported (tested in section 2, proved in section 3) |
| `main` | `src/lw/raster2dymolw.cpp:56` | `crates/engine/src/ports/dymo_cups/raster2dymolw.rs:23` | ported (tested in section 2, proved in section 3) |
| `CCupsFilter::CCupsFilter` | `src/common/CupsFilter.h:60` | ported in `CupsFilter::new` | ported |
| `CCupsFilter::Run` | `src/common/CupsFilter.h:69` | `crates/engine/src/ports/dymo_cups/cups_filter.rs:85` | ported (tested in section 2, proved in section 3) |
| `CCupsFilter::InitDocument` | `src/common/CupsFilter.h:214` | `crates/engine/src/ports/dymo_cups/cups_filter.rs:227` | ported (tested in section 2, proved in section 3) |
| `CCupsPrintEnvironmentForDriver::CCupsPrintEnvironmentForDriver` | `src/common/CupsPrintEnvironment.cpp:31` | ported as `CupsPrintEnvironmentForDriver::new` | ported |
| `CCupsPrintEnvironmentForDriver::~CCupsPrintEnvironmentForDriver` | `src/common/CupsPrintEnvironment.cpp:47` | not ported: closes the PRN file: Rust drops it | not covered |
| `CCupsPrintEnvironmentForDriver::WriteData` | `src/common/CupsPrintEnvironment.cpp:55` | `crates/engine/src/ports/dymo_cups/cups_print_environment.rs:93` | ported (tested in section 2, proved in section 3) |
| `CCupsPrintEnvironmentForDriver::ReadData` | `src/common/CupsPrintEnvironment.cpp:79` | `crates/engine/src/ports/dymo_cups/cups_print_environment.rs:126` | ported (tested in section 2, proved in section 3) |
| `CCupsPrintEnvironmentForDriver::GetJobStatus` | `src/common/CupsPrintEnvironment.cpp:87` | `crates/engine/src/ports/dymo_cups/cups_print_environment.rs:131` | ported (tested in section 2, proved in section 3) |
| `CCupsPrintEnvironmentForDriver::SetJobStatus` | `src/common/CupsPrintEnvironment.cpp:93` | `crates/engine/src/ports/dymo_cups/cups_print_environment.rs:136` | ported (tested in section 2, proved in section 3) |
| `CCupsPrintEnvironmentForLM::CCupsPrintEnvironmentForLM` | `src/common/CupsPrintEnvironment.cpp:102` | ported as `CupsPrintEnvironmentForLm::new` | ported |
| `CCupsPrintEnvironmentForLM::~CCupsPrintEnvironmentForLM` | `src/common/CupsPrintEnvironment.cpp:106` | not ported: nothing to do | not covered |
| `CCupsPrintEnvironmentForLM::WriteData` | `src/common/CupsPrintEnvironment.cpp:112` | `crates/engine/src/ports/dymo_cups/cups_print_environment.rs:162` | ported (tested in section 2, proved in section 3) |
| `CCupsPrintEnvironmentForLM::ReadData` | `src/common/CupsPrintEnvironment.cpp:128` | `crates/engine/src/ports/dymo_cups/cups_print_environment.rs:179` | ported (tested in section 2, proved in section 3) |
| `CCupsPrintEnvironmentForLM::GetJobStatus` | `src/common/CupsPrintEnvironment.cpp:154` | `crates/engine/src/ports/dymo_cups/cups_print_environment.rs:204` | ported (tested in section 2, proved in section 3) |
| `CCupsPrintEnvironmentForLM::SetJobStatus` | `src/common/CupsPrintEnvironment.cpp:160` | `crates/engine/src/ports/dymo_cups/cups_print_environment.rs:212` | ported (tested in section 2, proved in section 3) |
| `CDummyLanguageMonitor::CDummyLanguageMonitor` | `src/common/DummyLanguageMonitor.cpp:29` | ported as `DummyLanguageMonitor::new` | ported |
| `CDummyLanguageMonitor::~CDummyLanguageMonitor` | `src/common/DummyLanguageMonitor.cpp:34` | not ported: nothing to do | not covered |
| `CDummyLanguageMonitor::StartDoc` | `src/common/DummyLanguageMonitor.cpp:39` | `crates/engine/src/ports/dymo_cups/dummy_language_monitor.rs:30` | ported (tested in section 2, proved in section 3) |
| `CDummyLanguageMonitor::EndDoc` | `src/common/DummyLanguageMonitor.cpp:44` | `crates/engine/src/ports/dymo_cups/dummy_language_monitor.rs:32` | ported (tested in section 2, proved in section 3) |
| `CDummyLanguageMonitor::StartPage` | `src/common/DummyLanguageMonitor.cpp:49` | `crates/engine/src/ports/dymo_cups/dummy_language_monitor.rs:34` | ported (tested in section 2, proved in section 3) |
| `CDummyLanguageMonitor::EndPage` | `src/common/DummyLanguageMonitor.cpp:54` | `crates/engine/src/ports/dymo_cups/dummy_language_monitor.rs:36` | ported (tested in section 2, proved in section 3) |
| `CDummyLanguageMonitor::ProcessData` | `src/common/DummyLanguageMonitor.cpp:61` | `crates/engine/src/ports/dymo_cups/dummy_language_monitor.rs:38` | ported (tested in section 2, proved in section 3) |
| `CHalftoneFilter::CHalftoneFilter` | `src/common/Halftoning.cpp:31` | ported as `HalftoneBase::new` | ported |
| `CHalftoneFilter::~CHalftoneFilter` | `src/common/Halftoning.cpp:36` | not ported: nothing to do | not covered |
| `CHalftoneFilter::GetInputImageType` | `src/common/Halftoning.cpp:41` | `crates/engine/src/ports/dymo_cups/halftoning.rs:54` | ported (tested in section 2, proved in section 3) |
| `CHalftoneFilter::GetOutputImageType` | `src/common/Halftoning.cpp:47` | `crates/engine/src/ports/dymo_cups/halftoning.rs:59` | ported (tested in section 2, proved in section 3) |
| `CHalftoneFilter::RGBToGrayScale` | `src/common/Halftoning.cpp:53` | `crates/engine/src/ports/dymo_cups/halftoning.rs:64` | ported (tested in section 2, proved in section 3) |
| `CHalftoneFilter::SetPixelBW` | `src/common/Halftoning.cpp:72` | `crates/engine/src/ports/dymo_cups/halftoning.rs:82` | ported (tested in section 2, proved in section 3) |
| `CHalftoneFilter::ExtractRGB` | `src/common/Halftoning.cpp:81` | `crates/engine/src/ports/dymo_cups/halftoning.rs:91` | ported (tested in section 2, proved in section 3) |
| `CHalftoneFilter::CalcImageWidth` | `src/common/Halftoning.cpp:101` | `crates/engine/src/ports/dymo_cups/halftoning.rs:109` | ported (tested in section 2, proved in section 3) |
| `CHalftoneFilter::CalcBufferSize` | `src/common/Halftoning.cpp:118` | `crates/engine/src/ports/dymo_cups/halftoning.rs:118` | ported (tested in section 2, proved in section 3) |
| `CHalftoneFilter::CalcOutputBufferSize` | `src/common/Halftoning.cpp:134` | `crates/engine/src/ports/dymo_cups/halftoning.rs:127` | ported (tested in section 2, proved in section 3) |
| `CHalftoneFilter::ExtractRGB` | `src/common/Halftoning.cpp:151` | not ported: the packed-int overload (:150-170, which ORs with `||`) is not used by any halftoning code | not covered |
| `EHalftoneError::EHalftoneError` | `src/common/Halftoning.cpp:176` | ported as the enum `HalftoneError` | ported |
| `EHalftoneError::GetErrorCode` | `src/common/Halftoning.cpp:181` | ported as the enum `HalftoneError` | ported |
| `CErrorDiffusionHalftoning::CErrorDiffusionHalftoning` | `src/common/ErrorDiffusionHalftoning.cpp:29` | ported as `ErrorDiffusionHalftoning::new` | ported |
| `CErrorDiffusionHalftoning::~CErrorDiffusionHalftoning` | `src/common/ErrorDiffusionHalftoning.cpp:36` | not ported: nothing to do | not covered |
| `CErrorDiffusionHalftoning::IsProcessLineSupported` | `src/common/ErrorDiffusionHalftoning.cpp:42` | `crates/engine/src/ports/dymo_cups/error_diffusion_halftoning.rs:72` | ported (tested in section 2, proved in section 3) |
| `CErrorDiffusionHalftoning::ProcessLine` | `src/common/ErrorDiffusionHalftoning.cpp:48` | `crates/engine/src/ports/dymo_cups/error_diffusion_halftoning.rs:77` | ported (tested in section 2, proved in section 3) |
| `CErrorDiffusionHalftoning::ProcessImage` | `src/common/ErrorDiffusionHalftoning.cpp:138` | `crates/engine/src/ports/dymo_cups/error_diffusion_halftoning.rs:46` | ported (tested in section 2, proved in section 3) |
| `CErrorDiffusionHalftoning::ProcessImage` | `src/common/ErrorDiffusionHalftoning.cpp:159` | `crates/engine/src/ports/dymo_cups/error_diffusion_halftoning.rs:159` | ported (tested in section 2, proved in section 3) |
| `CNLLHalftoning::CNLLHalftoning` | `src/common/NonLinearLaplacianHalftoning.cpp:256` | ported as `NllHalftoning::new` | ported |
| `CNLLHalftoning::~CNLLHalftoning` | `src/common/NonLinearLaplacianHalftoning.cpp:263` | not ported: nothing to do | not covered |
| `CNLLHalftoning::IsProcessLineSupported` | `src/common/NonLinearLaplacianHalftoning.cpp:268` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:300` | ported (tested in section 2, proved in section 3) |
| `CNLLHalftoning::ProcessLine` | `src/common/NonLinearLaplacianHalftoning.cpp:274` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:305` | ported (tested in section 2, proved in section 3) |
| `CNLLHalftoning::ProcessImage` | `src/common/NonLinearLaplacianHalftoning.cpp:280` | not ported: the void*-image overload (:279-284) does nothing ("TODO: non-implemented yet"); the image-buffer overload (:288-324) is ported | not covered |
| `CNLLHalftoning::ProcessImage` | `src/common/NonLinearLaplacianHalftoning.cpp:289` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:308` | ported (tested in section 2, proved in section 3) |
| `CNLLHalftoning::ProcessDiagonal` | `src/common/NonLinearLaplacianHalftoning.cpp:355` | not ported: dead code: only the commented-out ProcessImage calls it (:326-351) | not covered |
| `CNLLHalftoning::GetThreshold` | `src/common/NonLinearLaplacianHalftoning.cpp:430` | ported as `NllHalftoning::get_threshold` | ported |
| `CNLLBlock::CNLLBlock` | `src/common/NonLinearLaplacianHalftoning.cpp:439` | ported as `NllBlock::new` | ported |
| `CNLLBlock::FillBlock` | `src/common/NonLinearLaplacianHalftoning.cpp:448` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:97` | ported (tested in section 2, proved in section 3) |
| `CNLLBlock::ReduceClasses` | `src/common/NonLinearLaplacianHalftoning.cpp:458` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:109` | ported (tested in section 2, proved in section 3) |
| `CNLLBlock::ReduceClasses` | `src/common/NonLinearLaplacianHalftoning.cpp:465` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:116` | ported (tested in section 2, proved in section 3) |
| `CNLLBlock::FillPixel` | `src/common/NonLinearLaplacianHalftoning.cpp:481` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:131` | ported (tested in section 2, proved in section 3) |
| `CNLLBlock::GetPixelGray` | `src/common/NonLinearLaplacianHalftoning.cpp:509` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:157` | ported (tested in section 2, proved in section 3) |
| `CNLLBlock::GetNLL` | `src/common/NonLinearLaplacianHalftoning.cpp:522` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:167` | ported (tested in section 2, proved in section 3) |
| `CNLLBlock::GetBlockIntenseValue` | `src/common/NonLinearLaplacianHalftoning.cpp:549` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:194` | ported (tested in section 2, proved in section 3) |
| `CNLLBlock::OutputBlock` | `src/common/NonLinearLaplacianHalftoning.cpp:561` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:204` | ported (tested in section 2, proved in section 3) |
| `CNLLBlock::OutputClass` | `src/common/NonLinearLaplacianHalftoning.cpp:580` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:221` | ported (tested in section 2, proved in section 3) |
| `CNLLBlock::OutputPixel` | `src/common/NonLinearLaplacianHalftoning.cpp:613` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:254` | ported (tested in section 2, proved in section 3) |
| `CNLLBlock::OutputPixel` | `src/common/NonLinearLaplacianHalftoning.cpp:619` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:263` | ported (tested in section 2, proved in section 3) |
| `CNLLBlock::IsInImage` | `src/common/NonLinearLaplacianHalftoning.cpp:626` | not ported: dead code: only ProcessDiagonal calls the block test | not covered |
| `CNLLBlock::IsInImage` | `src/common/NonLinearLaplacianHalftoning.cpp:636` | `crates/engine/src/ports/dymo_cups/nll_halftoning.rs:272` | ported (tested in section 2, proved in section 3) |

123 functions ported, 23 listed as not ported (of 146 in the 10 files).
<!-- generated functions: end -->

## 2. The driver's own tests, ported

`src/lw/tests/` (`TestLabelWriterDriver.cpp`, `TestLabelWriterFilter.cpp`,
`TestLabelWriterLanguageMonitor.cpp`, with `MOCK_LWLMPrintEnvironment`) and `src/common/tests/`
(`TestErrorDiffusionHalftoning.cpp`, `TestNLLHalftoning.cpp`, with `MOCK_PrintEnvironment`), each case
ported with the same input and the same expected bytes. Differences: `LabelWriterFilterTest` opens
`lwtt.ppd` and builds a Twin Turbo driver, the port's `testDensity` and `testQuality` use the plain
450's PPD and `CLabelWriterDriver400` (the options and the initializer are the same code); the monitor
tests run on the real clock (`time(NULL)`, one second of status timeout), as the C++ ones do.

<!-- generated tests: begin -->
| Driver test | Line | The port's test (`crates/engine/src/ports/dymo_cups/tests.rs`) | Result |
|---|---|---|---|
| `LabelWriterDriverTest::testPrologEpilogCommand` | `src/lw/tests/TestLabelWriterDriver.cpp:50` | `driver_test_prolog_epilog_command` (:157) | **passes** |
| `LabelWriterDriverTest::testCompression` | `src/lw/tests/TestLabelWriterDriver.cpp:76` | `driver_test_compression` (:178) | **passes** |
| `LabelWriterDriverTest::testEmptyLines` | `src/lw/tests/TestLabelWriterDriver.cpp:106` | `driver_test_empty_lines` (:199) | **passes** |
| `LabelWriterDriverTest::testDotTab` | `src/lw/tests/TestLabelWriterDriver.cpp:134` | `driver_test_dot_tab` (:222) | **passes** |
| `LabelWriterDriverTest::testContinuousPaper` | `src/lw/tests/TestLabelWriterDriver.cpp:164` | `driver_test_continuous_paper` (:245) | **passes** |
| `LabelWriterDriverTest::test400EndPageEndDoc` | `src/lw/tests/TestLabelWriterDriver.cpp:195` | `driver_test_400_end_page_end_doc` (:265) | **passes** |
| `LabelWriterDriverTest::testTwinTurboRoll` | `src/lw/tests/TestLabelWriterDriver.cpp:224` | not ported: Twin Turbo (not a 450): `CLabelWriterDriverTwinTurbo` is not ported | not covered |
| `LabelWriterFilterTest::testDensity` | `src/lw/tests/TestLabelWriterFilter.cpp:57` | `filter_test_density` (:337) | **passes** |
| `LabelWriterFilterTest::testQuality` | `src/lw/tests/TestLabelWriterFilter.cpp:77` | `filter_test_quality` (:358) | **passes** |
| `LabelWriterFilterTest::testRoll` | `src/lw/tests/TestLabelWriterFilter.cpp:89` | not ported: Twin Turbo (not a 450): `InputSlot` and the Twin Turbo initializer are not ported | not covered |
| `LabelWriterLMTest::testGoodJob` | `src/lw/tests/TestLabelWriterLanguageMonitor.cpp:51` | `lm_test_good_job` (:378) | **passes** |
| `LabelWriterLMTest::testPaperOut` | `src/lw/tests/TestLabelWriterLanguageMonitor.cpp:79` | `lm_test_paper_out` (:398) | **passes** |
| `LabelWriterLMTest::testPaperOutAfterReprint` | `src/lw/tests/TestLabelWriterLanguageMonitor.cpp:112` | `lm_test_paper_out_after_reprint` (:420) | **passes** |
| `LabelWriterLMTest::testPollForPaperIn` | `src/lw/tests/TestLabelWriterLanguageMonitor.cpp:147` | `lm_test_poll_for_paper_in` (:444) | **passes** |
| `LabelWriterLMTest::testOneGoodPageOneNot` | `src/lw/tests/TestLabelWriterLanguageMonitor.cpp:180` | `lm_test_one_good_page_one_not` (:466) | **passes** |
| `LabelWriterLMTest::testRollSynchronize` | `src/lw/tests/TestLabelWriterLanguageMonitor.cpp:220` | `lm_test_roll_synchronize` (:489) | **passes** |
| `LabelWriterLMTest::testResetPrinter` | `src/lw/tests/TestLabelWriterLanguageMonitor.cpp:243` | `lm_test_reset_printer` (:508) | **passes** |
| `LabelWriterLMTest::testStatusReadFailed` | `src/lw/tests/TestLabelWriterLanguageMonitor.cpp:257` | `lm_test_status_read_failed` (:523) | **passes** |
| `LabelWriterLMTest::testContinuousPaper` | `src/lw/tests/TestLabelWriterLanguageMonitor.cpp:289` | `lm_test_continuous_paper` (:546) | **passes** |
| `LabelWriterLMTest::testRollChangedBit` | `src/lw/tests/TestLabelWriterLanguageMonitor.cpp:322` | `lm_test_roll_changed_bit` (:570) | **passes** |
| `ErrorDiffusionTest::testBlack` | `src/common/tests/TestErrorDiffusionHalftoning.cpp:44` | `halftone_test_black` (:613) | **passes** |
| `ErrorDiffusionTest::testWhite` | `src/common/tests/TestErrorDiffusionHalftoning.cpp:73` | `halftone_test_white` (:619) | **passes** |
| `ErrorDiffusionTest::testRed` | `src/common/tests/TestErrorDiffusionHalftoning.cpp:102` | `halftone_test_red` (:625) | **passes** |
| `ErrorDiffusionTest::testGreen` | `src/common/tests/TestErrorDiffusionHalftoning.cpp:133` | `halftone_test_green` (:634) | **passes** |
| `ErrorDiffusionTest::testBlue` | `src/common/tests/TestErrorDiffusionHalftoning.cpp:164` | `halftone_test_blue` (:643) | **passes** |
| `NonLinearLaplacianTest::testBlock` | `src/common/tests/TestNLLHalftoning.cpp:44` | `nll_test_block` (:652) | **passes** |

24 of 26 test cases ported (2 not covered).
<!-- generated tests: end -->


## 3. The real filter against the port, on the same pages

`scripts/dymo-labelwriter-450-check.py proof` builds DYMO's `raster2dymolw` (clang++, the driver's
sources untouched) and `scripts/dymo-labelwriter-450/mkras.c` (a helper on the system's libcups). For
each case: a PNG of the label's size is made; the port makes its page from it (header by the ported
`cupsRasterInterpretPPD`, pixels as W3 says) and runs the port's filter on a recording backend
(`crates/engine/examples/dymo_cups_proof.rs`); `mkras` writes a **CUPS raster of the very same pixels with
the header CUPS's own `cupsRasterInterpretPPD` makes from the real `lw450.ppd`**; DYMO's filter reads
that raster with `PPD`, `DEVICE_URI` and the back channel (fd 3, a pipe with the status bytes) as the
scheduler gives them. Compared: what each writes to stdout, its stderr lines (the `ReadStatus` timestamps
aside), its exit status.

| Case | Options (`argv[5]`) | Result |
|---|---|---|
| default size (`w167h288`, 665 x 1112), defaults, status TOF on the back channel | none | **identical** (40,167 bytes, 3,444 stderr lines) |
| default size, `DEVICE_URI` not `usb://` (no status asked) | none | **identical** (40,163 bytes) |
| 1 in x 1 in (`w72h72`) | none | **identical** (6,628 bytes) |
| jewelry (`w154h64`), NLL | `DymoHalftoning=NLL` | **identical** (10,319 bytes) |
| shipping (`w167h288`), Default (1 bit) | `DymoHalftoning=Default` | **identical** (27,679 bytes) |
| address (`w79h252`), Dark and Graphics (300 x 600) | `DymoPrintDensity=Dark DymoPrintQuality=Graphics` | **identical** (32,364 bytes) |
| `w72h154`, light density, `300x600dpi` (the resolution trap: `SetResolution` is not called) | `DymoPrintDensity=Light Resolution=300x600dpi` | **identical** (11,546 bytes) |
| continuous, wide (`w154h792`, `cupsMediaType 1`: `ESC L ff ff`) | none | **identical** (82,463 bytes) |
| `w54h144`, medium density, `DEVICE_URI` not set | `DymoPrintDensity=Medium` | **identical** (8,407 bytes) |
| `w72h72`, no printer: the back channel never answers (the filter's 10 s status limits, twice) | none | **identical** in the label's bytes and the exit status (6,640 bytes); **not compared**: the stderr lines and the number of `ESC A` requests: macOS's libcups `cupsBackChannelRead` ignores its 2.5 s timeout (measured: it blocks until the pipe has data or ends), so the real filter was given a pipe at its end and spins to the 10 s limit, while the port's reads wait 2.5 s as `backchannel.c:41-76` says |
| shipping, NLL | `DymoHalftoning=NLL` | **identical** (42,451 bytes) |
| noise picture, ErrorDiffusion (`w79h252`) | none | **identical** (41,193 bytes) |
| noise picture, NLL (`w72h154`) | `DymoHalftoning=NLL` | **identical** (22,897 bytes) |
| noise picture, Default (`w162h90`) | `DymoHalftoning=Default` | **identical** (29,687 bytes) |

The page header of **every one of the 61 `PageSize` choices** of the PPD: CUPS's (libcups on the real
PPD) against the port's, every field the interpreter sets (`cupsWidth`, `cupsHeight`, bytes per line,
bits, colour space, `PageSize`, `HWResolution`, `cupsInteger0`, margins, imaging box): **61 of 61
identical**; 19 of them are choices that repeat a page under another name (`w72h72.1`, ...) and the
port takes the name of the first (`cupsPageSizeName` is not read by the filter).

What the filter's status flow shows when the answer is a paper-out, a roll change or a failed read is
section 2 (the monitor's own mock tests): the real filter cannot be given a timed sequence of answers
through a pipe.

## 4. The runtime: CUPS 2.4.20, ported where the driver uses it

Runtime, to approve (proxy). Each function is named with its file and lines in the port's doc comments
(`crates/engine/src/ports/dymo_cups/cups/`).

| CUPS function | File | Port |
|---|---|---|
| `cupsParseOptions` | `cups/options.c:260-465` | `options.rs`; test `cups_parse_options_as_cups` |
| `ppdMarkDefaults`, `ppd_defaults`, `ppdMarkOption`, `ppd_mark_option`, `ppdFindMarkedChoice`, `ppdFindChoice`, `ppdFindOption`, `cupsMarkOptions` | `cups/ppd-mark.c` | `ppd.rs`; test `ppd_defaults_are_marked_as_cups_marks_them` and the proof (the options of every case) |
| `ppdPageSize` | `cups/ppd-page.c:28-171` | `ppd.rs` |
| `ppdEmitString`, `ppd_handle_media`, `ppdCollect2` | `cups/ppd-emit.c:80-220, 612-, 1132-1225` | `ppd.rs` (`emit_codes_any`) |
| `cupsRasterInterpretPPD`, `_cupsRasterExecPS` (the `setpagedevice` dictionaries) | `cups/raster-interpret.c:125-475, 1420-1650` | `raster.rs`; the 61 headers |
| `cupsRasterReadHeader2`, `_cupsRasterReadHeader`, `cups_raster_update`, `cupsRasterReadPixels` | `cups/raster-stream.c:593-745, 759-, 1573-1757`, `raster-stubs.c:192-230` | `raster.rs` |
| `cupsBackChannelRead` | `cups/backchannel.c:41-76` | `usb_backend.rs` (`BackChannel::read`) |
| `print_device`, `find_device`, `open_device`, `print_cb`, `read_thread`, `get_device_id`, `close_device` | `backend/usb-libusb.c:169-687, 820-1043, 1073-1165, 1521-1722, 1820-1873` | `usb_backend.rs` (over `nusb`, W5) |
| the plain 450's PPD | `ppd/lw450.ppd` | `ppd_lw450.rs`, generated and checked |

## 5. Ours (not the driver's, not CUPS's): `docs/archive/plans/agent-engine.md`, Step 3c

W1 the job and its end; W2 the page geometry (the PPD's imageable area at its resolution, the size by the
job's label length, a picture of another size fails) and W3 the page's colour (RGB 8 bits for
ErrorDiffusion and NLL, 1 bit for Default: the PNG decoded with the `png` crate, alpha over white, 16 bits
cut to 8, black below the driver's own `RGBToGrayScale` 128 for 1 bit); W4 argv and environment; W5 the
`usb` backend over `nusb` on a thread beside the filter, started with it, waiting for the printer inside
`print_device` as the reference does, joined by an in-process 64 KiB pipe it reads 8192 bytes at a time; W6 Stop as CUPS's cancel (the filter
is ended at its next call of its environment; `PollUntilPaperIn`'s `jsDeleted`), the controls that fail
loudly; W7 telemetry from the monitor's own lines; W8 `Gone`/`Back` at print time; W9 stderr as a
thread-local list; W10 an in-memory raster instead of a stream. The status of this section is "to
approve (proxy)".

## 6. Not covered

- **Windows.** The IEEE-1284 device-ID read (`get_device_id`, `usb_backend.rs`): a control transfer goes on an interface, as libusb's WinUSB backend sends it
  (`reference/libusb-1.0.29/windows_winusb.c`: `winusbx_submit_control_transfer` :2998-3004, `auto_claim` :613,
  `auto_release` :658); the request on the wire is the same. When no interface can be claimed, libusb gives `LIBUSB_ERROR_NOT_FOUND`; nusb has no such error, so ours gives `InvalidArgument` (differs, ours). Two more differences, harmless at these lengths: we try the active configuration's interfaces, not indexes 0..`USB_MAXINTERFACES` filtered by API type, and there is no `MAX_CTRL_BUFFER_LENGTH` (4096) check. Built for `x86_64-pc-windows-gnu`, never run on Windows
  or tried on a printer there (WinUSB must be the device's driver).

- The Turbo, DUO and Twin Turbo variants (`lw450t`, `lw450dl`, `lw450dt`, `lw450tt`), the LabelManager
  driver (`src/lm/`, `lm450.ppd`), the models other than the 450 in `ProcessPPDOptions` and
  `raster2dymolw.cpp`; the filter without a PPD or without the back channel; `CLabelWriterDriverTwinTurbo`
  (only its `GetRollSelectCommand`, which the monitor calls, is ported).
- The status flow of a paper-out, a roll change or a failed read against the real filter (only the monitor's
  own mock tests, section 2).
- CUPS: the PPD parser (the 450's PPD is data), `ppdConflicts`, the PPD cache and the IPP options of
  `cupsMarkOptions`, custom page sizes, the PostScript machine of `_cupsRasterExecPS` beyond the PPD's
  `setpagedevice` dictionaries and the `[{ %%BeginFeature ... } stopped cleartomark` wrapper of
  `ppdEmitString`, the raster stream's file format (headers, PackBits, byte swapping), several pages in
  one job, `DYMO_PRN_DIR`, the side channel, the backend's quirks file, `soft_reset`, `usb-unidir` and
  `usb-no-reattach`, the kernel driver's re-attachment on non-Linux, the backend's SIGTERM handling
  (replaced by W6), the printer's serial number and interface in the device URI.
- A real printer, real USB, real timing (the 250 ms and 60 s of the read thread are the backend's; the
  100 ms slices of `nusb` reads are ours).
- The hub's side: its picture's orientation and size, its `dots_for_mm` and `HEAD_DOTS`.

## 7. The old code (B8)

`crates/label/src/labelwriter.rs` kept only what the hub uses: `DPI`, `HEAD_DOTS`, `dots_for_mm`
(`hub/src/profiles.rs:229-230`). Gone: the encoder (`encode`, `run_length`), the status byte, the
vendor and product table, `STATUS_REQUEST`, `RESET_ESCAPES`, `TOP_AT_DOT_ZERO` and the whole
`usb` module (its 16 KiB chunks, status timing and orientation guess) with their tests. Left, nothing
uses them any more: the `usb` feature and the `nusb` dependency in `crates/label/Cargo.toml` (outside
the two files B8 allowed).

## PPD 3.1 and DYMO Connect's catalogue (HS2b, 2026-11-17)

The PPD is DYMO's current one (`*FileVersion "3.1"`), from DYMO Connect for Desktop 1.6.4.18's macOS driver
(`reference/dymo-connect-1.6.4.18/ppd/lw450.ppd`, taken out by `scripts/dymo-connect-extract.py`; owner, 2026-11-17:
"gebruik ... de nieuwe ppd"). The filter stays DYMO's CUPS driver 1.4.0: **filter and PPD come from two DYMO
packages**, paired by the owner's choice; DYMO never shipped them together. They are proven together here:
`python3 -I scripts/dymo-labelwriter-450-check.py proof` (the real `raster2dymolw` 1.4.0 built here and libcups
2.4.20 on the new PPD vs the port): **61 of 61 `PageSize` headers identical, 14 of 14 cases identical**. The
`cupsFilter` line of the new PPD (the macOS filter path) is not used: the agent runs the ported filter.

Every line that differs between the two PPDs (`<` 1.4.0's, `>` 3.1's):

    < *% $Id: lw450.ppd 16401 2011-10-31 18:51:16Z pineichen $
    > *% $Id: lw450.ppd 18293 2012-05-28 16:46:08Z aleksandr $
    < *FileVersion:   "3.0"
    > *FileVersion:   "3.1"
    < *cupsFilter:    "application/vnd.cups-raster 0 raster2dymolw"
    > *cupsFilter:    "application/vnd.cups-raster 0 /Library/Printers/DYMO/Filters/raster2dymolw"
    < *APPrinterIconPath: "/Library/Printers/DYMO/CUPS/Resources/LW450.icns"
    > *APPrinterIconPath: "/Library/Printers/DYMO/Icons/LW450.icns"
    < *PageSize w102h252/30321 Large Address: "<</PageSize[102 252]/ImagingBBox null/cupsInteger0 0>>setpagedevice"
    > *PageSize w102h252/30321 Large Address: "<</PageSize[102 251]/ImagingBBox null/cupsInteger0 0>>setpagedevice"
    < *PageRegion w102h252/30321 Large Address: "<</PageSize[102 252]/ImagingBBox null/cupsInteger0 0>>setpagedevice"
    > *PageRegion w102h252/30321 Large Address: "<</PageSize[102 251]/ImagingBBox null/cupsInteger0 0>>setpagedevice"
    < *ImageableArea w102h252/30321 Large Address: "4.32 4.32 98.40 234.96"
    > *ImageableArea w102h252/30321 Large Address: "4.32 3.84 98.40 234.48"
    < *PaperDimension w79h252/30252 Address: "78.96 252.00"
    > *PaperDimension w79h252/30252 Address: "78.48 252.00"
    < *PaperDimension w102h252/30321 Large Address: "101.28 251.04"
    > *PaperDimension w102h252/30321 Large Address: "101.52 250.56"
    > 
    > *OpenUI *DymoContinuousPaper/Continuous Paper: PickOne
    > *OrderDependency: 110 AnySetup *DymoContinuousPaper
    > *DefaultDymoContinuousPaper: 0
    > *DymoContinuousPaper 0/Disabled: ""
    > *DymoContinuousPaper 1/Enabled: "/cupsMediaType 1>>setpagedevice"
    > 
    > *de.Translation DymoContinuousPaper/Endlospapier: ""
    > *de.DymoContinuousPaper 0/Deaktiviert: ""
    > *de.DymoContinuousPaper 1/Aktiviert: ""
    > 
    > *es.Translation DymoContinuousPaper/Papel continuo: ""
    > *es.DymoContinuousPaper 0/Inhabilitado: ""
    > *es.DymoContinuousPaper 1/Habilitado: ""
    > 
    > *es_CO.Translation DymoContinuousPaper/Papel continuo: ""
    > *es_CO.DymoContinuousPaper 0/Inhabilitado: ""
    > *es_CO.DymoContinuousPaper 1/Habilitado: ""
    > *fr.Translation DymoContinuousPaper/Papier continu: ""
    > *fr.DymoContinuousPaper 0/Désactivé: ""
    > *fr.DymoContinuousPaper 1/Activé: ""
    > 
    > *fr_CA.Translation DymoContinuousPaper/Papier continu: ""
    > *fr_CA.DymoContinuousPaper 0/Désactivé: ""
    > *fr_CA.DymoContinuousPaper 1/Activé: ""
    > 
    > *it.Translation DymoContinuousPaper/Carta continua: ""
    > *it.DymoContinuousPaper 0/Disabilitato: ""
    > *it.DymoContinuousPaper 1/Abilitato: ""
    > 
    > *nl.Translation DymoContinuousPaper/Kettingformulieren: ""
    > *nl.DymoContinuousPaper 0/Uitgeschakeld: ""
    > *nl.DymoContinuousPaper 1/Ingeschakeld: ""
    > 
    > *pt.Translation DymoContinuousPaper/Papel contínuo: ""
    > *pt.DymoContinuousPaper 0/Desativado: ""
    > *pt.DymoContinuousPaper 1/Ativado: ""
    > 
    > *pt_BR.Translation DymoContinuousPaper/Papel contínuo: ""
    > *pt_BR.DymoContinuousPaper 0/Desativado: ""
    > *pt_BR.DymoContinuousPaper 1/Ativado: ""
    > 
    > *CloseUI: *DymoContinuousPaper
    > 
    < *%  End of "$Id: lw450.ppd 16401 2011-10-31 18:51:16Z pineichen $"
    > *%  End of "$Id: lw450.ppd 18293 2012-05-28 16:46:08Z aleksandr $"

W2b (proxy, 2026-11-17): in the new PPD 30252 Address and 30320/99010 Address are both 89 mm and 299 × 963 dots but
different pages, so the length cannot choose. A job may name its `PageSize` choice (`JobOrder.page_size`), as CUPS's
`-o PageSize=<name>`: exactly that choice is marked, an unknown name fails, the length rule is not used; jobs without it
keep the length rule. The proof names the page for those three (and only where the length is not unique).

The hub's rolls and the labels on each page come from DYMO Connect's own catalogue (`DYMO.LabelAPI.dll`:
`SKUs`, `Labels`, `SkuLinks`), generated into `crates/engine/src/ports/dymo_connect_lw450.rs` and checked by
`python3 -I scripts/dymo-connect-catalog.py` (57 rolls for the 450; the 49 die-cut SKUs not covered are listed in
the file with why). This is the hub's layout only: nothing of it reaches the printer. Where DYMO's twips origin
lies on the page (taken as the page's left and top edge) is **not yet verified** on the printer.
Ours in the catalogue's reading (`scripts/dymo-connect-catalog.py`): the region order for a SKU's name (NL, BE, GB,
US, DE, FR, then the others alphabetically); in a `Path`, a rectangle inside another is a hole, not a label (a badge's
slot), and a `Path` of lines only is one label, the box around its points. These decide only how the hub draws the
labels; approved by the owner, 2026-11-17 ("akkoord met alle drie").

