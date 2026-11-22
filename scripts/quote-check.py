#!/usr/bin/env python3
"""Every quote of a reference stands, letter for letter, where we say it stands (CLAUDE.md:
"NOTHING made up", *Lasers (GRBL)*). Checked by this script, never by eye.

A *unit* is one paragraph, list item or table row of a Markdown file, or one comment block of a
Rust file (`//`, `///`, `//!`; code is not looked at). A unit that names a reference page (see
SOURCES) is a quoting unit: each "double-quoted" text in it of two words or more must stand on one
of the pages it names. `…` or `...` inside a quote stands for left-out text: the parts must stand
in that order. Backticks, `*`, `_` and backslash escapes are ignored on both sides, and runs of
white space count as one space.

A quote with "owner" just before or after it is the owner's words; one after "(ours" in its unit is
our own words: neither is a reference's. A quote inside a code span (backticks) is code, not a quotation. A quote in a
unit that names no page of SOURCES but names another source (a file such as `command.py` or
`link.rs:54`, or PrusaLink or the SDK) is listed as NOT CHECKED: this script cannot check it, and
says so. Any other quote is UNSOURCED and fails the check: a claim carries its source. Quotation
marks are for quotations only, never for our own terms.

Usage: scripts/quote-check.py [FILE ...]
  With no FILE: the GRBL path (DEFAULT_FILES) and Part 4 of docs/design/prusa-sdk-extension.md.
  FILEs are checked instead (a PR text, a file of the LaserFoundry fork, ...).
Exit 1 when a quote is not found or is unsourced.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
REF = ROOT / "reference"

# The references and their pins (CLAUDE.md, *Lasers (GRBL)*). A page is named in a unit by one of
# its patterns.
WIKI = REF / "gnea-grbl-wiki"  # github.com/gnea/grbl.wiki 86e8ecd
GRBL = REF / "gnea-grbl"  # github.com/gnea/grbl bfb67f0
PINS = {WIKI: "86e8ecd", GRBL: "bfb67f0"}
SOURCES = {
    "Grbl-v1.1-Commands": (WIKI / "Grbl-v1.1-Commands.md", r"Grbl[- ]v1\.1[- ]Commands"),
    "Grbl-v1.1-Interface": (WIKI / "Grbl-v1.1-Interface.md", r"Grbl[- ]v1\.1[- ]Interface"),
    "Grbl-v1.1-Jogging": (WIKI / "Grbl-v1.1-Jogging.md", r"Grbl[- ]v1\.1[- ]Jogging"),
    "Grbl-v1.1-Laser-Mode": (WIKI / "Grbl-v1.1-Laser-Mode.md", r"Grbl[- ]v1\.1[- ]Laser[- ]Mode"),
    "Grbl-v1.1-Configuration": (WIKI / "Grbl-v1.1-Configuration.md", r"Grbl[- ]v1\.1[- ]Configuration"),
    "stream.py": (GRBL / "doc/script/stream.py", r"\bstream\.py\b"),
}

DEFAULT_FILES = [
    "crates/grbl/src/lib.rs",
    "crates/engine/src/ports/laserfoundry/port.rs",
    "crates/engine/src/ports/laserfoundry/mod.rs",
    "crates/engine/src/device/grbl.rs",
    "crates/agent/src/laser.rs",
]
PART4 = ("docs/design/prusa-sdk-extension.md", "## Part 4", "## Part 5")

QUOTE = re.compile(r"[\"“]([^\"”\n]+?)[\"”]")
OTHER = re.compile(r"\b[\w-]+\.(rs|py|md|toml)\b|PrusaLink|\bSDK\b|\bBuddy\b|LaserFoundry")


def norm(text):
    text = text.replace("\\", "").replace("’", "'").replace("‘", "'")
    text = re.sub(r"[`*_]", "", text)
    return re.sub(r"\s+", " ", text).strip()


def check_pins():
    bad = []
    for repo, pin in PINS.items():
        head = repo / ".git"
        if not head.exists():
            bad.append(f"{repo}: missing (clone it and check out {pin})")
            continue
        import subprocess

        rev = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD"], capture_output=True, text=True).stdout
        if not rev.startswith(pin):
            bad.append(f"{repo}: at {rev[:7]}, not the pin {pin}")
    return bad


def units_md(text):
    unit, start = [], 0
    for n, line in enumerate(text.splitlines(), 1):
        s = line.strip()
        new = not s or s.startswith(("- ", "* ", "|", "#")) or re.match(r"\d+\. ", s)
        if new and unit:
            yield start, " ".join(unit)
            unit = []
        if s:
            if not unit:
                start = n
            unit.append(s)
    if unit:
        yield start, " ".join(unit)


def units_rs(text):
    unit, start = [], 0
    for n, line in enumerate(text.splitlines(), 1):
        m = re.match(r"\s*//[/!]?\s?(.*)$", line)
        if not m:
            m = re.search(r"//[/!]?\s?(.*)$", line)  # a comment after code
            if m and unit:
                yield start, " ".join(unit)
                unit = []
            if m:
                yield n, m.group(1)
            elif unit:
                yield start, " ".join(unit)
                unit = []
            continue
        body = m.group(1).strip()
        if not body or body.startswith(("- ", "* ")):
            if unit:
                yield start, " ".join(unit)
                unit = []
        if body:
            if not unit:
                start = n
            unit.append(body)
    if unit:
        yield start, " ".join(unit)


def found(quote, page_text):
    pos = 0
    for part in re.split(r"…|\.\.\.", quote):
        part = norm(part)
        if not part:
            continue
        i = page_text.find(part, pos)
        if i < 0:
            return False
        pos = i + len(part)
    return True


def main(args):
    problems = check_pins()
    if problems:
        print("\n".join(problems))
        return 1
    pages = {name: norm(path.read_text(encoding="utf-8")) for name, (path, _) in SOURCES.items()}
    targets = []
    if args:
        targets = [(pathlib.Path(a), None) for a in args]
    else:
        targets = [(ROOT / f, None) for f in DEFAULT_FILES]
        targets.append((ROOT / PART4[0], PART4[1:]))
    checked, owners, failed, unchecked = 0, 0, [], []
    for path, section in targets:
        text = path.read_text(encoding="utf-8")
        offset = 0
        if section:
            a = text.index(section[0])
            b = text.index(section[1], a)
            offset = text[:a].count("\n")
            text = text[a:b]
        units = units_rs(text) if path.suffix == ".rs" else units_md(text)
        for line, unit in units:
            quotes = []
            spans = [(c.start(), c.end()) for c in re.finditer(r"`[^`]*`", unit)]
            for m in QUOTE.finditer(unit):
                if len(norm(m.group(1)).split()) < 2:
                    continue
                if any(a <= m.start() < b for a, b in spans):
                    continue
                around = unit[max(0, m.start() - 80) : m.end() + 80].lower()
                if "owner" in around or "(ours" in unit[: m.start()].lower():
                    owners += 1
                    continue
                quotes.append(m.group(1))
            if not quotes:
                continue
            named = [name for name, (_, pat) in SOURCES.items() if re.search(pat, unit)]
            where = f"{path.relative_to(ROOT) if path.is_relative_to(ROOT) else path}:{line + offset}"
            for q in quotes:
                if not named:
                    if OTHER.search(unit):
                        unchecked.append(f"NOT CHECKED {where}: \"{q}\"")
                    else:
                        failed.append(f"UNSOURCED {where}: \"{q}\"")
                    continue
                checked += 1
                if any(found(q, pages[n]) for n in named):
                    continue
                elsewhere = [n for n in pages if found(q, pages[n])]
                hint = f" (it stands on {', '.join(elsewhere)})" if elsewhere else ""
                failed.append(f"NOT FOUND {where}: \"{q}\" not on {', '.join(named)}{hint}")
    for f in unchecked + failed:
        print(f)
    print(f"{checked} quotes checked against the references, {owners} the owner's, {len(unchecked)} not checked, {len(failed)} problems")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
