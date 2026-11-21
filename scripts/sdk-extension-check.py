#!/usr/bin/env python3
"""The SDK extension's check: nothing in docs/design/prusa-sdk-extension.md that the protocol has already.

Owner, 2026-11-21: parts were written for things the SDK already has (the bed mesh, layer triggers,
UPGRADE, a state's reason). This check makes that fail instead of relying on someone remembering to look.

It reads the page's table "Every addition" (one row per addition: name, kind, of, part, searched) and:

  1. Declared: every addition the page names is in that table. An upper-case name in backticks that is
     neither a reference's own name nor a declared addition, or a lower-case key in the first column of
     a part's table that is neither, fails ("undeclared").
  2. Not the protocol's: every declared command, telemetry key, INFO key, event field, endpoint, and
     kwarg of a command the protocol already has, is searched in the references, at their commits
     (wire.md "References"). A name a reference already uses fails ("the reference has it"), unless it
     is in REUSED below, with the reference's place and why it is the same thing.
     Not searched, by construction: a kwarg of a new command and a sub-key of a new key. They live
     only inside the addition, so they cannot clash with anything the protocol has.
  3. Searched: every row says what was looked at for the same *meaning* under another name, and each
     file it cites exists in its reference. A name check cannot see a meaning; this column is where
     the person writing the row shows they looked (the SDK's `reason` was the case it missed).

Not covered: a meaning the protocol has under another name, beyond what the "searched" column says.

Run with `python3 -I scripts/sdk-extension-check.py`. Standard library and git only."""
import os, re, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DOC = os.environ.get("SDK_EXTENSION_DOC") or os.path.join(ROOT, "docs", "design", "prusa-sdk-extension.md")


def ref_dir(name):
    """The reference checkouts live beside the worktrees, in the main checkout's reference/."""
    for base in (ROOT, os.path.join(ROOT, "..", "..", "..")):
        p = os.path.join(base, "reference", name)
        if os.path.isdir(p):
            return os.path.normpath(p)
    sys.exit(f"reference/{name} not found")


# The references, at the commits wire.md names; each with the paths that hold the printer protocol.
REFS = {
    "SDK": ("Prusa-Connect-SDK-Printer", "c0d7d233d4ce2081e6c020d2e98fb92ad9b51521", ["prusa/connect/printer/"]),
    "PrusaLink": ("Prusa-Link", "171148ce7dbc7f1fb0633ea519523f3258877e2e", ["prusa/link/"]),
    "Buddy": ("buddy-6.4.0", "b91eeda", ["src/connect/"]),
    "Connect": ("prusa-connect", "2e8c62b", ["internal/connector/xbuddy/"]),
}
SOURCE = re.compile(r"\.(py|cpp|hpp|h|go)$")

# A name a reference already has, used here with the reference's own meaning: (kind, name) -> why.
REUSED = {
    ("kwarg", "path"): "START_PRINT's own kwarg, read by PrusaLink (prusa_link.py:563-570)",
}

# Names in backticks that are no addition and no name of the printer protocol.
WORDS = {
    "raster2dymolw": "DYMO's CUPS filter (dymo-cups-drivers-1.4.0)",
    "w167h288": "a PageSize choice of DYMO's ppd/lw450.ppd",
    "SHA256SUMS": "the checksum list of our releases (crates/update)",
    "print_scale_data": "usbscale's function that reads a scale's report (usbscale.c:380-456)",
    "UNITS": "usbscale's table of unit names (usbscale.c:93-107)",
}

def device_words():
    """Names of a device's own reference that the page cites, which are no protocol names: OctoPrint
    1.11.8's comm states (`comm.py` `MachineCom.STATE_*`, part 9)."""
    comm = os.path.join(ref_dir("OctoPrint-1.11.8"), "src", "octoprint", "util", "comm.py")
    text = open(comm, encoding="utf-8").read()
    return {n for n in re.findall(r"^\s+STATE_([A-Z_]+) = \d+", text, re.M)}


KINDS = {"command", "telemetry", "info", "event-field", "endpoint", "kwarg", "sub-key", "value", "name"}


def load_refs():
    files = {}
    for label, (name, commit, prefixes) in REFS.items():
        d = ref_dir(name)
        listing = subprocess.run(["git", "-C", d, "ls-tree", "-r", "--name-only", commit],
                                 check=True, capture_output=True, text=True).stdout.split()
        for path in listing:
            if SOURCE.search(path) and any(path.startswith(p) for p in prefixes) and "test" not in path:
                text = subprocess.run(["git", "-C", d, "show", f"{commit}:{path}"],
                                      check=True, capture_output=True, text=True).stdout
                files[(label, path)] = text
    return files


def ref_names(files):
    """Every quoted string and enum member in the references: the protocol's own names."""
    names = set()
    for text in files.values():
        names.update(re.findall(r"[\"']([A-Za-z_/][A-Za-z0-9_/.\-]*)[\"']", text))
        names.update(re.findall(r"^\s*([A-Z][A-Z0-9_]+)\s*=", text, re.M))
        names.update(re.findall(r"\b(?:def|func|class)\s+(?:\([^)]*\)\s*)?(\w+)", text))
    return names


def hits(files, name):
    enum = r"|^\s*" + re.escape(name) + r"\s*=" if name.isupper() else ""  # an enum member is upper case
    pat = re.compile(r"[\"']" + re.escape(name) + r"[\"']" + enum, re.M)
    out = []
    for (label, path), text in files.items():
        for m in pat.finditer(text):
            line = text.count("\n", 0, m.start()) + 1
            out.append(f"{label} {path}:{line}")
    return out


def table_after(lines, heading):
    """Rows of the first table under a heading, as lists of cells."""
    try:
        i = next(n for n, l in enumerate(lines) if l.strip() == heading)
    except StopIteration:
        sys.exit(f"no section {heading!r} in {DOC}")
    rows, started = [], False
    for l in lines[i + 1:]:
        if l.startswith("|"):
            started = True
            cells = [c.strip() for c in l.strip().strip("|").split("|")]
            if not set("".join(cells)) <= set("-: "):
                rows.append(cells)
        elif started:
            break
    return rows[1:], i


def unquote(cell):
    m = re.fullmatch(r"`([^`]+)`", cell)
    return m.group(1) if m else cell


def main():
    lines = open(DOC, encoding="utf-8").read().splitlines()
    rows, at = table_after(lines, "## Every addition")
    files = load_refs()
    known = ref_names(files)
    words = set(WORDS) | device_words()
    errors = []

    declared = set()
    for r in rows:
        if len(r) != 5:
            errors.append(f"row {r}: wants 5 cells (name, kind, of, part, searched)")
            continue
        name, kind, of, part, searched = unquote(r[0]), r[1], unquote(r[2]), r[3], r[4]
        declared.add(name)
        if kind not in KINDS:
            errors.append(f"{name}: kind {kind!r} is not one of {sorted(KINDS)}")
            continue
        # 3. what was searched, and the files it cites.
        if not searched or searched == "-":
            errors.append(f"{name}: says nothing of what was searched for the same meaning")
        for cite in re.findall(r"`([\w./-]+\.(?:py|cpp|hpp|go))(?::[\d,-]+)?`", searched):
            if not any(p.endswith(cite) for (_, p) in files):
                errors.append(f"{name}: cites {cite}, which is in none of the references")
        # 2. not the protocol's own name.
        searchable = kind in {"command", "telemetry", "info", "event-field", "endpoint"} or (
            kind == "kwarg" and of in known)
        if searchable and (kind, name) not in REUSED:
            found = hits(files, name)
            if found:
                errors.append(f"{name} ({kind}): the reference has it: {', '.join(found[:4])}")

    # 1. every addition the page names is declared.
    body = lines[:at]
    for n, l in enumerate(body, 1):
        for name in re.findall(r"`([A-Z][A-Z0-9_]{3,})`", l):
            if name not in known and name not in declared and name not in words:
                errors.append(f"line {n}: `{name}` is no reference's name and not in Every addition")
        for name in re.findall(r"`([a-z][a-z0-9_]*)`", l):
            if re.fullmatch(r"[0-9a-f]{7,40}", name):
                continue  # a commit
            if name not in known and name not in declared and name not in words:
                errors.append(f"line {n}: `{name}` is no reference's name and not in Every addition")

    commits = ", ".join(f"{k} {v[1][:8]}" for k, v in REFS.items())
    print(f"sdk-extension-check: {len(rows)} additions, against {commits} ({len(files)} files)")
    for e in errors:
        print("  FAIL", e)
    print("sdk-extension-check:", "FAILED" if errors else "all passed")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
