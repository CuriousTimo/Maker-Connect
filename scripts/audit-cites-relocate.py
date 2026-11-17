#!/usr/bin/env python3
"""Moves the line numbers an audit cites in OUR code after that code moved (formatting, an edit above).

    scripts/audit-cites-relocate.py <base-commit> <audit.md> <src-root> [<src-root> ...]

For every `path.rs:N` in the audit whose file is ours (found under one of the src roots, e.g.
crates/hub/src) and changed since <base-commit>, N is mapped through `git diff <base-commit>` of that
file: a line outside the changed hunks moves by the hunks' offsets; a line inside a changed hunk is
looked up by its old text in the new file (the nearest occurrence after mapping). A cite it cannot map
is left as it is and listed. The audit's own checker then proves every cite (run it after this).
Reference citations (Buddy's .cpp, OctoPrint's .py, LaserFoundry's .rs outside our roots) are never
touched. Run with `python3 -I`. Standard library and git only."""
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CITE = re.compile(r"(?<![\w/.-])((?:[\w-]+/)*[\w-]+\.rs):(\d+)")
HUNK = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, check=True).stdout


def hunks(base, path):
    out = []
    for line in git("diff", "-U0", base, "--", path).split("\n"):
        m = HUNK.match(line)
        if m:
            old, oc, new, nc = int(m[1]), int(m[2] or 1), int(m[3]), int(m[4] or 1)
            out.append((old, oc, new, nc))
    return out


def mapper(base, path):
    hs = hunks(base, path)
    old_lines = git("show", f"{base}:{path}").split("\n")
    new_lines = open(os.path.join(ROOT, path), encoding="utf-8").read().split("\n")

    def map_line(n):
        shift = 0
        for old, oc, new, nc in hs:
            if oc == 0:  # pure insertion after line `old`
                if n > old:
                    shift = new + nc - 1 - old
                continue
            if n < old:
                break
            if n < old + oc:  # inside a changed hunk: find its old text near the mapped place
                text = old_lines[n - 1].strip() if 0 < n <= len(old_lines) else ""
                guess = n + shift
                if not text:
                    return None
                hits = [i + 1 for i, l in enumerate(new_lines) if l.strip() == text]
                if not hits:
                    hits = [i + 1 for i, l in enumerate(new_lines) if text in l]
                return min(hits, key=lambda h: abs(h - guess)) if hits else None
            shift = (new + nc) - (old + oc)
        return n + shift

    return map_line


def main():
    if len(sys.argv) < 4:
        print(__doc__)
        return 2
    base, audit, roots = sys.argv[1], sys.argv[2], sys.argv[3:]
    changed = set(git("diff", "--name-only", base).split())
    maps, unmapped, moved = {}, [], 0

    def resolve(cite):
        for r in roots:
            p = os.path.normpath(os.path.join(r, cite))
            if os.path.isfile(os.path.join(ROOT, p)):
                return p
        return cite if os.path.isfile(os.path.join(ROOT, cite)) else None

    def fix(m):
        nonlocal moved
        cite, n = m[1], int(m[2])
        path = resolve(cite)
        if path is None or path not in changed:
            return m[0]
        if path not in maps:
            maps[path] = mapper(base, path)
        new = maps[path](n)
        if new is None:
            unmapped.append(m[0])
            return m[0]
        if new != n:
            moved += 1
        return f"{cite}:{new}"

    text = open(os.path.join(ROOT, audit), encoding="utf-8").read()
    out = CITE.sub(fix, text)
    open(os.path.join(ROOT, audit), "w", encoding="utf-8").write(out)
    print(f"{audit}: {moved} cites moved")
    for u in unmapped:
        print("NOT MAPPED:", u)
    return 0


if __name__ == "__main__":
    sys.exit(main())
