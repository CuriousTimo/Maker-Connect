#!/usr/bin/env python3
"""Takes the DYMO LabelWriter references out of DYMO Connect for Desktop's macOS installer, without
installing it (docs/archive/briefs/hs2b-dymo-connect-catalog.md; owner, 2026-11-17: "niet installeren").

  dymo-connect-extract.py PKG

PKG is DYMO's installer as downloaded from dymo.com (DCDMac1.6.4.18-Arm64.pkg). macOS's own
`pkgutil --expand-full` unpacks it into a temporary folder (nothing in it runs); then, into
reference/dymo-connect-1.6.4.18/ (not in git: reference/ is ignored):

  SOURCE.txt         the installer's name, size and SHA-256, and what was taken from where
  ppd/*.ppd          every Library/Printers/PPDs/Contents/Resources/*.ppd.gz, unpacked, unchanged
                     (each must carry its GPL-2-or-later notice)
  catalog/*.xml      the XML documents embedded in Contents/MonoBundle/DYMO.LabelAPI.dll, byte for
                     byte (each found by its XML declaration and its root element's end tag, and
                     checked to parse)

Run with `python3 -I`. Standard library only."""
import gzip, hashlib, os, re, shutil, subprocess, sys, tempfile
import xml.etree.ElementTree as ET

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
VERSION = "1.6.4.18"
OUT = os.path.join(ROOT, "reference", f"dymo-connect-{VERSION}")
APP = "dymo-connect.pkg/Payload/Applications/DYMO Connect.app/Contents"
PPDS = "dymo-connect.pkg/Payload/Library/Printers/PPDs/Contents/Resources"
WANTED = ["Labels", "SKUs", "SkuLinks", "Papers"]


def embedded_xml(blob):
    """Each XML document in the DLL: (root element, its bytes)."""
    for m in re.finditer(rb"<\?xml", blob):
        rest = blob[m.start():]
        root = re.search(rb"<([A-Za-z][\w:.-]*)[\s>]", rest[5:])
        if not root:
            continue
        end = rest.find(b"</" + root.group(1) + b">")
        if end < 0:
            continue
        yield root.group(1).decode(), rest[: end + len(root.group(1)) + 3]


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    pkg = sys.argv[1]
    data = open(pkg, "rb").read()
    if VERSION not in os.path.basename(pkg):
        sys.exit(f"not DYMO Connect {VERSION}: {pkg}")
    with tempfile.TemporaryDirectory() as tmp:
        expanded = os.path.join(tmp, "x")
        subprocess.run(["pkgutil", "--expand-full", pkg, expanded], check=True)
        os.makedirs(os.path.join(OUT, "ppd"), exist_ok=True)
        os.makedirs(os.path.join(OUT, "catalog"), exist_ok=True)
        ppds = sorted(n for n in os.listdir(os.path.join(expanded, PPDS)) if n.endswith(".ppd.gz"))
        for name in ppds:
            with gzip.open(os.path.join(expanded, PPDS, name), "rb") as f:
                text = f.read()
            if b"GNU General Public License" not in text or b"either version 2" not in text:
                sys.exit(f"no GPL-2-or-later notice in {name}")
            open(os.path.join(OUT, "ppd", name[: -len(".gz")]), "wb").write(text)
        dll = open(os.path.join(expanded, APP, "MonoBundle", "DYMO.LabelAPI.dll"), "rb").read()
        found = {}
        for root, xml in embedded_xml(dll):
            if root in WANTED and root not in found:
                ET.fromstring(xml)  # it parses
                found[root] = xml
        missing = [w for w in WANTED if w not in found]
        if missing:
            sys.exit(f"not found in DYMO.LabelAPI.dll: {missing}")
        for root, xml in found.items():
            open(os.path.join(OUT, "catalog", f"{root}.xml"), "wb").write(xml)
    with open(os.path.join(OUT, "SOURCE.txt"), "w") as f:
        f.write(
            f"DYMO Connect for Desktop {VERSION}, macOS Arm64, from dymo.com: {os.path.basename(pkg)}\n"
            f"size {len(data)} bytes, SHA-256 {hashlib.sha256(data).hexdigest()}\n"
            f"Expanded with pkgutil --expand-full (not installed) by scripts/dymo-connect-extract.py.\n"
            f"ppd/*.ppd: every {PPDS}/*.ppd.gz ({len(ppds)}), unpacked, unchanged\n"
            f"(each GPL-2 or later, the PPD's own notice).\n"
            f"catalog/*.xml: XML documents embedded in {APP}/MonoBundle/DYMO.LabelAPI.dll, byte for byte\n"
            f"(DYMO's; their use approved by the owner, 2026-11-17).\n"
        )
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
