#!/usr/bin/env python3
"""The DYMO LabelWriter 400 on the LabelWriter stack: mechanical checks (docs/audits/dymo-labelwriter-400.md).

The 400 takes DYMO's CUPS driver 1.4.0 (the filter `raster2dymolw`) and DYMO Connect 1.6.4.18's `lw400.ppd`
as the 450 takes `lw450.ppd` from there (#230). The code that does the work is the 450's
(scripts/dymo-labelwriter-450-check.py), run here with the 400's PPD:

  gen    writes crates/engine/src/ports/dymo_cups/cups/ppd_lw400.rs from reference/dymo-connect-1.6.4.18/ppd/lw400.ppd
  check  regenerates it in memory and fails if the file on disk differs, and checks in the driver's own
         source what makes the 400 take `CLabelWriterDriver400`, as the 450 (raster2dymolw.cpp:78-123)
  proof  builds DYMO's real raster2dymolw, makes the pages with libcups on lw400.ppd and compares what the
         real filter writes with what the port writes (needs `cargo build -p maker-connect-engine --example
         dymo_cups_proof`)

Run with `python3 -I`. Standard library only."""
import importlib.util, os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("lw450check", os.path.join(HERE, "dymo-labelwriter-450-check.py"))
lw = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lw)

lw.PPD = os.path.join(lw.ROOT, "reference", "dymo-connect-1.6.4.18", "ppd", "lw400.ppd")
lw.PPD_NAME = "reference/dymo-connect-1.6.4.18/ppd/lw400.ppd"
lw.OUT = os.path.join(lw.ROOT, "crates", "engine", "src", "ports", "dymo_cups", "cups", "ppd_lw400.rs")
lw.STEM = "lw400"
lw.GEN = "dymo-labelwriter-400"
lw.DESC = "LabelWriter 400"
lw.MODEL = "lw400"
lw.URI = "usb://DYMO/LabelWriter%20400"
# The 320's cases (the 450's, without its 300x600 DPI choice, which lw400.ppd has not either).
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
    """What makes the 400 take `CLabelWriterDriver400`, found in the driver's own source and the PPD."""
    def src(p):
        return open(os.path.join(lw.REF, p), encoding="latin-1").read()
    fails = []
    main = src("src/lw/raster2dymolw.cpp")
    lwf = src("src/lw/CupsFilterLabelWriter.cpp")
    drv = src("src/lw/LabelWriterDriver.cpp")
    model, options, sizes, reasons = lw.parse_ppd()
    name = model[0]
    if name != "DYMO LabelWriter 400":
        fails.append(f"the PPD's ModelName is {name!r}")
    # the 400/450 list (:92-98) takes CLabelWriterDriver400 with the monitor; the 400 is in it
    m = re.search(r'else if \((!strcasecmp\(ppd->modelname, "[^"]+"\)\s*(\|\|\s*)?)+\)\s*\{\s*if \(IsBackchannelSupported\(\)\)\s*\{\s*CCupsFilter<CLabelWriterDriver400, CDriverInitializerLabelWriterWithLM, CLabelWriterLanguageMonitor> Filter;', main)
    if not m:
        fails.append("no list in raster2dymolw.cpp takes CLabelWriterDriver400 with the monitor")
    elif f'"{name}"' not in m.group(0):
        fails.append("the 400's model name is not in the CLabelWriterDriver400 list")
    # no width of its own: the default 84 bytes
    if "MaxPrintWidth_(84)" not in drv:
        fails.append("MaxPrintWidth_(84) not in LabelWriterDriver.cpp")
    if "LabelWriter 400" in lwf:
        fails.append("the 400 has an entry of its own in CupsFilterLabelWriter.cpp")
    # the PPD has only 300dpi, which the initializer does not match
    res = [o for o in options if o[0] == "Resolution"][0]
    choices = [c[0] for c in res[4]]
    if choices != ["300dpi"]:
        fails.append(f"Resolution choices are {choices}")
    return fails


EXCLUDED_OUT = os.path.join(lw.ROOT, "crates", "engine", "src", "ports", "dymo_connect_lw400.rs")


def printer_groups():
    """Papers.xml's PrinterGroups (comments taken out): each driver name, its paper group and its excluded papers."""
    papers = open(os.path.join(lw.ROOT, "reference", "dymo-connect-1.6.4.18", "catalog", "Papers.xml"),
                  encoding="utf-8-sig", errors="replace").read()
    papers = re.sub(r"<!--.*?-->", "", papers, flags=re.S)
    out = {}
    for m in re.finditer(r'<PrinterGroup PaperGroupId="([^"]+)">(.*?)</PrinterGroup>', papers, re.S):
        excluded = re.findall(r"<PaperName>([^<]+)</PaperName>", m.group(2))
        for name in re.findall(r"<DriverName>([^<]+)</DriverName>", m.group(2)):
            out[name] = (m.group(1), excluded)
    return out


def generate_excluded():
    group, excluded = printer_groups()["DYMO LabelWriter 400"]
    lines = [
        "// GENERATED by scripts/dymo-labelwriter-400-check.py gen from reference/dymo-connect-1.6.4.18/catalog/Papers.xml",
        "// (DYMO Connect for Desktop 1.6.4.18's own catalogue). Do not edit: `check` fails if this file differs from",
        "// what it gives. DYMO's data; its use approved by the owner, 2026-11-17.",
        "",
        "//! The DYMO LabelWriter 400 in DYMO Connect's catalogue (#230): its `PrinterGroup` takes the paper group",
        f"//! `{group}`, as the 450's does, less the papers it excludes. Pure data, for the hub.",
        "",
        "/// The papers (`PaperName`) DYMO Connect does not offer on the 400 (`ExcludedPapers`), in its order.",
        "pub static EXCLUDED_PAPERS: &[&str] = &[",
    ] + [f"    {lw.rust_str(e)}," for e in excluded] + ["];", ""]
    return "\n".join(lines)


def check_catalogue():
    """DYMO Connect gives the 400 the 450's paper group (less its own excluded papers), so the 400 takes the
    catalogue's rolls on its own PPD."""
    fails = []
    groups = printer_groups()
    group = {k: v[0] for k, v in groups.items()}
    for name in ("DYMO LabelWriter 400", "DYMO LabelWriter 450"):
        if name not in group:
            fails.append(f"{name} is in no PrinterGroup of Papers.xml")
    if group.get("DYMO LabelWriter 400") != group.get("DYMO LabelWriter 450"):
        fails.append("the 400 and the 450 are in different paper groups")
    # every roll's page is a PageSize of the 400's PPD, with the same text as in the 450's
    rolls = open(os.path.join(lw.ROOT, "crates", "engine", "src", "ports", "dymo_connect_lw450.rs")).read()
    pages = set(re.findall(r'page_size: "([^"]+)"', rolls))
    def texts(path):
        return dict(re.findall(r'^\*PageSize ([^/:]+)/([^:]+):', open(path, encoding="latin-1").read(), re.M))
    t400 = texts(lw.PPD)
    t450 = texts(os.path.join(lw.ROOT, "reference", "dymo-connect-1.6.4.18", "ppd", "lw450.ppd"))
    for page in sorted(pages):
        if page not in t400:
            fails.append(f"the catalogue's page {page} is not a PageSize of lw400.ppd")
        elif t400[page] != t450.get(page):
            fails.append(f"{page}: lw400.ppd calls it {t400[page]!r}, lw450.ppd {t450.get(page)!r}")
    return fails


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "check"
    if cmd == "proof":
        return lw.proof()
    text = lw.generate()
    if cmd == "gen":
        open(lw.OUT, "w").write(text)
        print("wrote", lw.OUT)
        open(EXCLUDED_OUT, "w").write(generate_excluded())
        print("wrote", EXCLUDED_OUT)
        return 0
    ok = True
    on_disk = open(lw.OUT).read() if os.path.exists(lw.OUT) else ""
    if on_disk != text:
        print("FAIL: ppd_lw400.rs differs from what lw400.ppd gives (run gen)")
        ok = False
    else:
        print("ok: ppd_lw400.rs is what lw400.ppd gives")
    fails = check_driver_source()
    for f in fails:
        print("FAIL:", f)
        ok = False
    if not fails:
        print("ok: the driver's source holds what makes the 400 take CLabelWriterDriver400")
    if (open(EXCLUDED_OUT).read() if os.path.exists(EXCLUDED_OUT) else "") != generate_excluded():
        print("FAIL: dymo_connect_lw400.rs differs from what Papers.xml gives (run gen)")
        ok = False
    else:
        print("ok: dymo_connect_lw400.rs is what Papers.xml gives")
    cat = check_catalogue()
    for f in cat:
        print("FAIL:", f)
        ok = False
    if not cat:
        print("ok: DYMO Connect gives the 400 the 450's paper group; every roll's page is in lw400.ppd, with the 450's text")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
