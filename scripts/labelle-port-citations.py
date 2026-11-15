#!/usr/bin/env python3
"""Checks the `file:line` citations in crates/engine/src/ports/labelle/ against the sources they
cite (labelle, pyusb, Pillow): the cited line must exist and, unless --list is given, must start a
definition (def, class, a C function's name line or its static/return-type line) or be a plain
statement the citation names. Prints every citation with the source line; exits 1 on a line that
is blank or out of range, and lists those that are not definitions for a look."""
import re, sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "crates/engine/src/ports/labelle"
LAB = ROOT / "reference/labelle/src/labelle"
PIL = ROOT / "reference/pillow-12.3.0/src"
USB = ROOT / "reference/.venv-audit/lib/python3.14/site-packages/usb"
def find(name):
    for base in (LAB, PIL, USB):
        hits = [p for p in base.rglob(name) if "_vendor" not in str(p) and " 2" not in p.name]
        if hits:
            return hits[0]
FIX = "--fix" in sys.argv
def def_lines(lines, name):
    pat = re.compile(r"^\s*(?:async def |def |class )%s\b|^(?:static |inline |\w+ \*?)*%s\(|^\s*%s\s*[:=]" % ((re.escape(name),) * 3))
    return [i + 1 for i, l in enumerate(lines) if pat.match(l)]
bad = notdef = total = 0
for rs in sorted(SRC.rglob("*.rs")):
    rs_lines = rs.read_text().splitlines()
    changed = False
    for n, line in enumerate(rs_lines, 1):
        if FIX:
            def fixer(m):
                global changed
                f = find(m.group(1))
                if not f:
                    return m.group(0)
                lines = f.read_text().splitlines()
                ln = int(m.group(2))
                names = set(re.findall(r"`([A-Za-z_][A-Za-z_0-9]*)(?:\(\))?`", line))
                near = []
                for nm in names:
                    near += [(abs(d - ln), d) for d in def_lines(lines, nm)]
                if not near or (ln in [d for _, d in near]):
                    return m.group(0)
                d = min(near)[1]
                if min(near)[0] > 25:
                    return m.group(0)
                rng = m.group(0)
                new = rng.replace(f":{ln}", f":{d}", 1)
                end = re.search(r"-(\d+)", new)
                if end:
                    new = new.replace(f"-{end.group(1)}", f"-{int(end.group(1)) + d - ln}", 1)
                return new
            new_line = re.sub(r"`?([A-Za-z_]+\.(?:py|c)):(\d+)(?:-\d+)?`?", fixer, line)
            if new_line != line:
                rs_lines[n - 1] = new_line
                changed = True
                line = new_line
        for m in re.finditer(r"`?([A-Za-z_]+\.(?:py|c)):(\d+)(?:-\d+)?`?", line):
            f = find(m.group(1)); ln = int(m.group(2)); total += 1
            if not f: print(f"{rs.name}:{n}: {m.group(0)}: source not found"); bad += 1; continue
            lines = f.read_text().splitlines()
            if ln > len(lines) or not lines[ln - 1].strip():
                print(f"{rs.name}:{n}: {m.group(0)}: line is blank or past the end"); bad += 1; continue
            text = lines[ln - 1].strip()
            if not re.match(r"(async def |def |class |@|static |Imaging|ImagingPalette|[a-z_0-9A-Z]+\(|void|int |UINT8)", text):
                notdef += 1
                print(f"{rs.name}:{n}: {m.group(0)} -> {text[:70]}")
    if FIX and changed:
        rs.write_text("\n".join(rs_lines) + "\n")
print(f"{total} citations, {bad} wrong, {notdef} not at a definition (listed above for a look)")
sys.exit(1 if bad else 0)
