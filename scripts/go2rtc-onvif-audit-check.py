#!/usr/bin/env python3
"""ONVIF audit (docs/audits/go2rtc-onvif.md): our client against go2rtc's own (v1.9.15, 7c05066).

Checks, all against reference/go2rtc:
  1. every func of pkg/onvif/client.go, envelope.go, helpers.go is named in our port (ported, or
     listed there as not ported);
  2. every `Name`, `file.go:A-B` in our port is that func's (or type's) real line range;
  3. our test cases (onvif_fixtures/uri*.xml/.url, caps*.xml) are onvif_test.go's, extracted here;
  4. go2rtc's own TestGetStreamUri and TestGetCapabilities pass;
  5. our 18 request fixtures are what go2rtc's own client sends (make-fixtures.sh, run here);
  6. our onvif tests pass (they send the same 18 requests and run the same test cases).

    scripts/go2rtc-onvif-audit-check.py
"""
import os
import pathlib
import re
import subprocess
import sys
import tempfile

root = pathlib.Path(__file__).resolve().parent.parent
go2rtc = root / "reference/go2rtc"
if not go2rtc.exists():
    go2rtc = root.parents[2] / "reference/go2rtc"  # from a worktree in .claude/worktrees.nosync
onvif = go2rtc / "pkg/onvif"
port = root / "crates/hub/src/cameras/onvif.rs"
fixtures = root / "crates/hub/src/cameras/onvif_fixtures"
FILES = ["client.go", "envelope.go", "helpers.go"]

failed = []


def check(name, ok, detail=""):
    print(f"{'ok  ' if ok else 'FAIL'} {name}" + (f": {detail}" if detail else ""))
    if not ok:
        failed.append(name)


def ranges(path):
    """Each top-level func/type of a Go file: name -> (first line, line of its closing brace)."""
    lines = path.read_text().split("\n")
    out = {}
    for i, line in enumerate(lines):
        m = re.match(r"^func (?:\([^)]*\) )?(\w+)\(|^type (\w+) ", line)
        if not m:
            continue
        name = m.group(1) or m.group(2)
        end = i
        if line.rstrip().endswith("{"):
            end = next(j for j in range(i + 1, len(lines)) if lines[j] == "}")
        out[name] = (i + 1, end + 1)
    return out


src = port.read_text()

# 1. Every func named in the port.
for f in FILES:
    for name in re.findall(r"^func (?:\([^)]*\) )?(\w+)\(", (onvif / f).read_text(), re.M):
        check(f"named in port: {f} {name}", f"`{name}`" in src)

# 2. Every `Name`, `file.go:A-B` citation is the real range.
cites = re.findall(r"`(\w+)`, `((?:client|envelope|helpers)\.go):(\d+)(?:-(\d+))?`", src)
for name, f, a, b in cites:
    real = ranges(onvif / f).get(name)
    want = f"{real[0]}-{real[1]}" if real else "not found"
    got = f"{a}-{b or a}"
    check(f"line range: {name} {f}:{got}", real is not None and got == want, f"go2rtc has {want}")

# 3. Test cases equal to onvif_test.go's.
test = (onvif / "onvif_test.go").read_text()
stream, caps = test.split("func TestGetCapabilities")
xmls = re.findall(r"xml:\s+`([^`]*)`", stream)
urls = re.findall(r'url:\s+"([^"]*)"', stream)
for i, (x, u) in enumerate(zip(xmls, urls), 1):
    check(f"test case uri{i}.xml", (fixtures / f"uri{i}.xml").read_text() == x)
    check(f"test case uri{i}.url", (fixtures / f"uri{i}.url").read_text() == u)
check("test case count TestGetStreamUri", len(xmls) == len(urls) == len(list(fixtures.glob("uri*.xml"))))
cxmls = re.findall(r"xml:\s+`([^`]*)`", caps)
for i, x in enumerate(cxmls, 1):
    check(f"test case caps{i}.xml", (fixtures / f"caps{i}.xml").read_text() == x)
check("test case count TestGetCapabilities", len(cxmls) == len(list(fixtures.glob("caps*.xml"))))

# 4. go2rtc's own tests.
r = subprocess.run(["go", "test", "./pkg/onvif", "-run", "TestGetStreamUri|TestGetCapabilities"],
                   cwd=go2rtc, capture_output=True, text=True)
check("go2rtc's own tests", r.returncode == 0, r.stdout.strip().splitlines()[-1] if r.stdout else r.stderr)

# 5. The request fixtures are go2rtc's.
with tempfile.TemporaryDirectory() as tmp:
    r = subprocess.run(["bash", str(fixtures / "make-fixtures.sh"), tmp], capture_output=True, text=True,
                       env={**os.environ, "GO2RTC": str(go2rtc)})
    check("go2rtc harness runs", r.returncode == 0, r.stderr.strip()[-200:])
    made = sorted((pathlib.Path(tmp) / "goref/out").glob("req*.txt"))
    check("request count", len(made) == len(list(fixtures.glob("req*.txt"))) and made, f"go2rtc sent {len(made)}")
    for p in made:
        check(f"request {p.name}", p.read_bytes() == (fixtures / p.name).read_bytes())

# 6. Our tests.
r = subprocess.run(["cargo", "test", "-q", "-p", "maker-connect-hub", "--features", "cameras", "--lib",
                    "cameras::"], cwd=root, capture_output=True, text=True)
summary = [line for line in r.stdout.splitlines() if line.startswith("test result")]
check("our onvif tests", r.returncode == 0, summary[-1] if summary else r.stderr.strip()[-300:])

print(f"\n{'all identical' if not failed else str(len(failed)) + ' failed'}")
sys.exit(1 if failed else 0)
