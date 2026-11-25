#!/usr/bin/env python3
"""The LabelWriter rolls from DYMO Connect's own catalogue (docs/archive/briefs/hs2b-dymo-connect-catalog.md).

  gen    writes crates/engine/src/ports/dymo_connect_lw450.rs from
         reference/dymo-connect-1.6.4.18/catalog/{SKUs,Labels,SkuLinks}.xml and the 450's PPD
         (reference/dymo-connect-1.6.4.18/ppd/lw450.ppd); run scripts/dymo-connect-extract.py first
  check  regenerates it in memory and fails if the file on disk differs (the default)

The rules, all mechanical:
- a roll is a `DieCutSKU` (one per SKU number) whose `PaperName` is the text of a `PageSize` choice of
  the PPD; its PPD choice is the first choice with that text in PPD order (the others with the same
  text are the same page);
- its label shapes are the `DieCutLabel` whose `Id` is the SKU's `Name` (DYMO's own link); else the
  `DieCutLabel`s with its `PaperName`, if they all draw the same; else it is not covered;
- its name is the `Name` of its `SKULink`, by the region order below (ours, written here); a SKU linked
  only in other regions takes the first of those in alphabetical order (ours); without a link, none;
- its cells (the labels on the page, in twips): each top-level `RoundRectangle`/`Rectangle` of the
  shapes; in a `Path`, each `RoundRectangle`/`Rectangle` not inside another (one inside another is a
  hole: a badge's slot), or, for a `Path` of lines only, the box around its `LineTo` points (both
  ours, written here; approved by the owner, 2026-11-17);
- its orientation is its paper's `DefaultOrientation` in `Papers.xml`, paper group `LabelWriter400` (the
  group DYMO Connect's `PrinterGroups` give the 450 and the 400); the hub's turn starts there (owner,
  2026-11-25: "de dymos printen standaard eigenlijk altijd 90 graden");
- SKUs not covered are listed at the end of the file with why.

Run with `python3 -I`. Standard library only."""
import os, re, sys
import xml.etree.ElementTree as ET

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REF = os.path.join(ROOT, "reference", "dymo-connect-1.6.4.18")
CAT = os.path.join(REF, "catalog")
PPD = os.path.join(REF, "ppd", "lw450.ppd")
OUT = os.path.join(ROOT, "crates", "engine", "src", "ports", "dymo_connect_lw450.rs")
# The order a SKU's name is taken in, when DYMO links it in several regions (ours, written here).
REGIONS = ["NL", "BE", "GB", "US", "DE", "FR"]


def rust_str(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def num(v):
    return repr(float(v))


def ppd_choices():
    texts = {}
    for line in open(PPD, encoding="latin-1"):
        m = re.match(r"\*PageSize (\S+?)/([^:]*):", line)
        if m:
            texts.setdefault(m.group(2), []).append(m.group(1))
    return texts


def cells(draw):
    out = []
    for el in draw:
        if el.tag in ("RoundRectangle", "Rectangle"):
            out.append([float(el.get(k)) for k in ("X", "Y", "Width", "Height")])
        elif el.tag == "Path":
            # a Path's rectangles: each one not inside another is a label (30333: two side by side); one
            # inside another is a hole in it (a name badge's slot). A Path of lines only is one label.
            boxes = [[float(p.get(k)) for k in ("X", "Y", "Width", "Height")] for p in el.iter() if p.tag in ("RoundRectangle", "Rectangle")]

            def inside(a, b):
                return a is not b and b[0] <= a[0] and b[1] <= a[1] and a[0] + a[2] <= b[0] + b[2] and a[1] + a[3] <= b[1] + b[3]

            labels = [a for a in boxes if not any(inside(a, b) for b in boxes)]
            if labels:
                out.extend(labels)
                continue
            xs = [float(p.get("X")) for p in el.iter() if p.tag == "LineTo"]
            ys = [float(p.get("Y")) for p in el.iter() if p.tag == "LineTo"]
            if xs:
                out.append([min(xs), min(ys), max(xs) - min(xs), max(ys) - min(ys)])
    return out


def generate():
    texts = ppd_choices()
    labels = list(ET.parse(os.path.join(CAT, "Labels.xml")).getroot().iter("DieCutLabel"))
    by_id = {l.findtext("Id"): l for l in labels}
    by_paper = {}
    for l in labels:
        by_paper.setdefault(l.findtext("PaperName"), []).append(l)
    group = next(g for g in ET.parse(os.path.join(CAT, "Papers.xml")).getroot().iter("PaperGroup") if g.get("Id") == "LabelWriter400")
    orientation = {paper.get("Name"): paper.findtext("DefaultOrientation") for paper in group.iter("Paper")}
    names = {}
    for group in ET.parse(os.path.join(CAT, "SkuLinks.xml")).getroot():
        for link in group:
            names.setdefault(link.get("Sku"), {}).setdefault(link.get("Region"), link.get("Name"))
    rolls, not_covered, seen = [], [], set()
    for sku in ET.parse(os.path.join(CAT, "SKUs.xml")).getroot().iter("DieCutSKU"):
        number, paper = sku.get("SKU"), sku.get("PaperName")
        if number in seen:
            continue
        seen.add(number)
        if paper not in texts:
            not_covered.append((number, f"its paper {paper!r} is not a PageSize of the 450's PPD"))
            continue
        label = by_id.get(sku.get("Name"))
        if label is None:
            same = by_paper.get(paper, [])
            drawn = {ET.tostring(l.find("DrawCommands")) for l in same}
            if len(drawn) != 1:
                why = "no DieCutLabel draws its paper" if not same else "its paper's DieCutLabels draw differently"
                not_covered.append((number, why))
                continue
            label = same[0]
        by_region = names.get(number, {})
        name = next((by_region[r] for r in REGIONS if r in by_region), None)
        if name is None and by_region:
            name = by_region[sorted(by_region)[0]]
        rolls.append((number, name, paper, texts[paper][0], sku.get("SearchTerms") or "", label.findtext("Id"), cells(label.find("DrawCommands")), orientation[paper]))
    o = [
        "// GENERATED by scripts/dymo-connect-catalog.py gen from reference/dymo-connect-1.6.4.18/catalog (DYMO Connect",
        "// for Desktop 1.6.4.18's own label catalogue, DYMO.LabelAPI.dll) and the 450's PPD. Do not edit: `check` fails",
        "// if this file differs from what they give. DYMO's data; its use approved by the owner, 2026-11-17.",
        "",
        "//! The DYMO LabelWriter 450's rolls as DYMO Connect knows them (HS2b): each SKU (the number on the box), its",
        "//! name, its paper and the PPD's `PageSize` choice for it, and its labels on the page (cells in twips,",
        "//! 1/1440 in, from DYMO's `DrawCommands`). Pure data, for the hub.",
        "",
        "/// One roll.",
        "#[derive(Debug, Clone, Copy, PartialEq)]",
        "pub struct Roll {",
        "    /// The SKU, as on the box (`S0722530`, `30333`).",
        "    pub sku: &'static str,",
        "    /// Its name in DYMO's `SkuLinks` (by the region order NL, BE, GB, US, DE, FR); none without a link.",
        "    pub name: Option<&'static str>,",
        "    /// Its paper (`PaperName`), which is the text of a `PageSize` choice of the PPD.",
        "    pub paper: &'static str,",
        "    /// That `PageSize` choice (`w72h72.2`).",
        "    pub page_size: &'static str,",
        "    /// DYMO's search terms.",
        "    pub search: &'static str,",
        "    /// The `DieCutLabel` drawn for it (its `Id`).",
        "    pub label: &'static str,",
        "    /// The labels on its page: x, y, width, height in twips.",
        "    pub cells: &'static [[f32; 4]],",
        "    /// Its paper's `DefaultOrientation` in `Papers.xml` (paper group `LabelWriter400`): `Landscape` or `Portrait`.",
        "    pub orientation: &'static str,",
        "}",
        "",
        "/// The rolls, in the catalogue's order.",
        "pub static ROLLS: &[Roll] = &[",
    ]
    for number, name, paper, choice, search, label, cs, turn in rolls:
        o.append("    Roll {")
        o.append(f"        sku: {rust_str(number)},")
        o.append(f"        name: {'None' if name is None else 'Some(' + rust_str(name) + ')'},")
        o.append(f"        paper: {rust_str(paper)},")
        o.append(f"        page_size: {rust_str(choice)},")
        o.append(f"        search: {rust_str(search)},")
        o.append(f"        label: {rust_str(label)},")
        o.append("        cells: &[" + ", ".join("[" + ", ".join(num(v) for v in c) + "]" for c in cs) + "],")
        o.append(f"        orientation: {rust_str(turn)},")
        o.append("    },")
    o.append("];")
    o.append("")
    o.append("/// The catalogue's die-cut SKUs that are not rolls here, and why.")
    o.append("pub static NOT_COVERED: &[(&str, &str)] = &[")
    for number, why in not_covered:
        o.append(f"    ({rust_str(number)}, {rust_str(why)}),")
    o.append("];")
    o.append("")
    return "\n".join(o)


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "check"
    text = generate()
    if cmd == "gen":
        open(OUT, "w").write(text)
        print("wrote", OUT)
        return 0
    if not os.path.exists(OUT) or open(OUT).read() != text:
        print("FAIL: dymo_connect_lw450.rs differs from what the catalogue gives (run gen)")
        return 1
    print("ok: dymo_connect_lw450.rs is what the catalogue gives")
    return 0


if __name__ == "__main__":
    sys.exit(main())
