#!/usr/bin/env python3
"""The DYMO LabelWriter 320 on the LabelWriter stack: mechanical checks (docs/audits/dymo-labelwriter-320.md).

The 320 takes DYMO's CUPS driver 1.4.0 (the filter `raster2dymolw`) and DYMO Connect 1.6.4.18's `lw320.ppd`
(FileVersion 3.1), as the 450 takes `lw450.ppd` from there. The code that does the work is the 450's
(scripts/dymo-labelwriter-450-check.py), run here with the 320's PPD:

  gen    writes crates/engine/src/ports/dymo_cups/cups/ppd_lw320.rs from reference/dymo-connect-1.6.4.18/ppd/lw320.ppd
  check  regenerates it in memory and fails if the file on disk differs, and checks in the driver's own
         source what makes the 320 take the base class `CLabelWriterDriver` (raster2dymolw.cpp:78-123)
  proof  builds DYMO's real raster2dymolw, makes the pages with libcups on lw320.ppd and compares what the
         real filter writes with what the port writes (needs `cargo build -p maker-connect-engine --example
         dymo_cups_proof`)

Run with `python3 -I`. Standard library only."""
import importlib.util, os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("lw450check", os.path.join(HERE, "dymo-labelwriter-450-check.py"))
lw = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lw)

lw.PPD = os.path.join(lw.ROOT, "reference", "dymo-connect-1.6.4.18", "ppd", "lw320.ppd")
lw.PPD_NAME = "reference/dymo-connect-1.6.4.18/ppd/lw320.ppd"
lw.OUT = os.path.join(lw.ROOT, "crates", "engine", "src", "ports", "dymo_cups", "cups", "ppd_lw320.rs")
lw.STEM = "lw320"
lw.GEN = "dymo-labelwriter-320"
lw.DESC = "LabelWriter 320"
lw.MODEL = "lw320"
lw.URI = "usb://DYMO/LabelWriter%20320"
# The 450's cases, for the 320's PPD (which has no 300x600 DPI choice): the same sizes and options.
lw.CASES = [
    ("default size, defaults, status TOF", "w167h288", "", "02" * 32, lw.URI),
    ("default size, defaults, not local (no status)", "w167h288", "", None, "ipp://printer/queue"),
    ("1 in x 1 in, defaults", "w72h72", "", "02" * 32, lw.URI),
    ("jewelry 154x64, NLL", "w154h64", "DymoHalftoning=NLL", "02" * 32, lw.URI),
    ("shipping, Default (1 bit)", "w167h288", "DymoHalftoning=Default", "02" * 32, lw.URI),
    ("address, Dark and Graphics", "w79h252", "DymoPrintDensity=Dark DymoPrintQuality=Graphics", "02" * 32, lw.URI),
    ("light density, 300 dpi", "w72h154", "DymoPrintDensity=Light Resolution=300dpi", "02" * 32, lw.URI),
    ("continuous, wide", "w154h792", "", "02" * 32, lw.URI),
    ("medium density, no URI set", "w54h144", "DymoPrintDensity=Medium", "02" * 32, None),
    ("no printer: the back channel never answers", "w72h72", "", None, lw.URI),
    ("shipping, NLL", "w167h288", "DymoHalftoning=NLL", "02" * 32, lw.URI),
    ("noise, ErrorDiffusion", "w79h252", "", "02" * 32, lw.URI),
    ("noise, NLL", "w72h154", "DymoHalftoning=NLL", "02" * 32, lw.URI),
    ("noise, Default (1 bit)", "w162h90", "DymoHalftoning=Default", "02" * 32, lw.URI),
]


def check_driver_source():
    """What makes the 320 take `CLabelWriterDriver`, found in the driver's own source and the PPD."""
    def src(p):
        return open(os.path.join(lw.REF, p), encoding="latin-1").read()
    fails = []
    main = src("src/lw/raster2dymolw.cpp")
    drv = src("src/lw/LabelWriterDriver.cpp")
    lwf = src("src/lw/CupsFilterLabelWriter.cpp")
    model, options, sizes, reasons = lw.parse_ppd()
    name = model[0]
    if name != "DYMO LabelWriter 320":
        fails.append(f"the PPD's ModelName is {name!r}")
    # the model names `main` matches, and the class each list takes
    names = re.findall(r'strcasecmp\(ppd->modelname, "([^"]+)"\)', main)
    if name in names:
        fails.append("the 320's model name is in a list of raster2dymolw.cpp")
    # the lists: the Twin Turbo (:78-79), the 400/450 (:92-98); after them the last `else` (:111-123)
    last_else = main[[m.start() for m in re.finditer(r"\n    else\n    \{\n      if \(IsBackchannelSupported\(\)\)", main)][-1]:]
    if "CCupsFilter<CLabelWriterDriver, CDriverInitializerLabelWriterWithLM, CLabelWriterLanguageMonitor> Filter;" not in last_else:
        fails.append("the last else of raster2dymolw.cpp does not take CLabelWriterDriver with the monitor")
    # the base class: width 84, EndDoc empty, EndPage the form feed
    if "MaxPrintWidth_(84)" not in drv:
        fails.append("MaxPrintWidth_(84) not in LabelWriterDriver.cpp")
    if not re.search(r"CLabelWriterDriver::EndDoc\(\)\s*\{\s*\}", drv):
        fails.append("CLabelWriterDriver::EndDoc is not empty")
    if not re.search(r"CLabelWriterDriver::EndPage\(\)\s*\{\s*SendFormFeed\(\);\s*\}", drv):
        fails.append("CLabelWriterDriver::EndPage is not SendFormFeed()")
    # no width of its own for the 320
    if "LabelWriter 320" in lwf or "LabelWriter 320" in drv:
        fails.append("the 320 has an entry of its own in the driver")
    # the resolution trap: the PPD has only 300dpi, which the initializer does not match
    res = [o for o in options if o[0] == "Resolution"][0]
    choices = [c[0] for c in res[4]]
    if choices != ["300dpi"]:
        fails.append(f"Resolution choices are {choices}")
    if re.findall(r'strcasecmp\(choice->choice, "([^"]+dpi)"\)', lwf) != ["203dpi", "203x138dpi"]:
        fails.append("resolution choices matched are not 203dpi/203x138dpi")
    return fails


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "check"
    if cmd == "proof":
        return lw.proof()
    text = lw.generate()
    if cmd == "gen":
        open(lw.OUT, "w").write(text)
        print("wrote", lw.OUT)
        return 0
    ok = True
    on_disk = open(lw.OUT).read() if os.path.exists(lw.OUT) else ""
    if on_disk != text:
        print("FAIL: ppd_lw320.rs differs from what lw320.ppd gives (run gen)")
        ok = False
    else:
        print("ok: ppd_lw320.rs is what lw320.ppd gives")
    fails = check_driver_source()
    for f in fails:
        print("FAIL:", f)
        ok = False
    if not fails:
        print("ok: the driver's source holds what makes the 320 take CLabelWriterDriver")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
