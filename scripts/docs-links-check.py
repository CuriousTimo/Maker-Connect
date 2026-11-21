#!/usr/bin/env python3
"""Every document named in the repository exists.

Reads every tracked text file (and the untracked docs/archive/plans/mk3-port-plan.md) and checks:
  - each `docs/<path>.md` or `.html` named anywhere exists;
  - each relative Markdown link `](x.md)` in a file under docs/ resolves from that file's folder;
  - each `briefs/<name>.md` / `audits/<name>.md` / `plans/…` named inside docs/ exists under docs/.
Exit 1 and list the misses otherwise. Run with `python3 -I`. Standard library only."""
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NAMED = re.compile(r"(?<![\w./-])(docs/[\w./-]+?\.(?:md|html))")
DOCS_RELATIVE = re.compile(r"(?<![\w./-])((?:plans|protocols|api|archive|briefs|audits)/[\w.-]+?\.md)")
LINK = re.compile(r"\]\(([^)#:\s]+\.md)(?:#[^)]*)?\)")


def files():
    out = subprocess.run(["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.split("\n")
    extra = ["docs/archive/plans/mk3-port-plan.md"]
    return [f for f in out + extra if f and os.path.isfile(os.path.join(ROOT, f))]


def main():
    missing = []
    for f in files():
        if f.startswith(("reference/", "target/")):
            continue
        try:
            text = open(os.path.join(ROOT, f), encoding="utf-8").read()
        except (UnicodeDecodeError, OSError):
            continue
        for n, line in enumerate(text.split("\n"), 1):
            for m in NAMED.finditer(line):
                if not os.path.exists(os.path.join(ROOT, m.group(1))):
                    missing.append(f"{f}:{n}: {m.group(1)}")
            if f.startswith("docs/"):
                for m in DOCS_RELATIVE.finditer(line):
                    if not os.path.exists(os.path.join(ROOT, "docs", m.group(1))):
                        missing.append(f"{f}:{n}: docs/{m.group(1)}")
                for m in LINK.finditer(line):
                    target = os.path.normpath(os.path.join(ROOT, os.path.dirname(f), m.group(1)))
                    if not os.path.exists(target):
                        missing.append(f"{f}:{n}: link {m.group(1)}")
    for m in missing:
        print("MISSING:", m)
    if not missing:
        print("ok: every document named in the repository exists")
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
