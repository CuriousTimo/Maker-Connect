#!/usr/bin/env python3
"""Follows the hub's line citations in docs/audits/buddy.md after the hub's code moved.

Run it after a change in crates/hub/src/ and before `scripts/buddy-audit-check.py`, with the change
not yet committed: it maps each cited hub line from HEAD to the working tree.

1. Row by row: a row whose first two cells are the same at HEAD gets each hub citation it had at HEAD
   (`file.rs:N`, `file.rs:N-M`, `file.rs:N=anchor`) mapped through a diff of that file, HEAD to now.
2. What `scripts/buddy-audit-check.py` then still reports as "the line does not hold" is moved to
   the nearest line before the file's tests that holds what the row names.

It prints what it moved; check the result with `scripts/buddy-audit-check.py`. It changes only
docs/audits/buddy.md.
"""
import difflib
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
AUDIT = os.path.join(ROOT, "docs", "audits", "buddy.md")
HUB = "crates/hub/src/"
CITE = re.compile(r"(?<![\w/])((?:devices/|store/|pages/|web/)?[a-z_]+\.rs):(\d+)(?:-(\d+))?(?=[=`\s,;)]|$)")


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True)


maps = {}


def mapping(rel):
    """HEAD line index -> working-tree line index, for the lines that did not change."""
    if rel not in maps:
        old = git("show", f"HEAD:{rel}").stdout.split("\n")
        path = os.path.join(ROOT, rel)
        new = open(path).read().split("\n") if os.path.exists(path) else []
        m = {}
        for block in difflib.SequenceMatcher(None, old, new, autojunk=False).get_matching_blocks():
            for i in range(block.size):
                m[block.a + i] = block.b + i
        maps[rel] = m
    return maps[rel]


def is_hub_file(name):
    rel = HUB + name
    return os.path.exists(os.path.join(ROOT, rel)) or git("cat-file", "-e", f"HEAD:{rel}").returncode == 0


def key(line):
    cells = line.split("|")
    return "|".join(cells[:3]) if len(cells) > 3 else None


def follow_rows():
    head = git("show", "HEAD:docs/audits/buddy.md").stdout.split("\n")
    by_key = {}
    for line in head:
        k = key(line)
        if k:
            by_key.setdefault(k, []).append(line)
    lines = open(AUDIT).read().split("\n")
    moved = 0
    for i, line in enumerate(lines):
        k = key(line)
        if not k or len(by_key.get(k, [])) != 1:
            continue
        old = [m for m in CITE.finditer(by_key[k][0]) if is_hub_file(m.group(1))]
        new = [m for m in CITE.finditer(line) if is_hub_file(m.group(1))]
        if not new or [m.group(1) for m in old] != [m.group(1) for m in new]:
            continue
        out, last = "", 0
        for o, n in zip(old, new):
            m = mapping(HUB + o.group(1))
            nums = [o.group(2)] + ([o.group(3)] if o.group(3) else [])
            mapped = [m.get(int(x) - 1) for x in nums]
            if None in mapped:
                out = None
                break
            cite = f"{o.group(1)}:{mapped[0] + 1}" + (f"-{mapped[1] + 1}" if len(mapped) > 1 else "")
            out += line[last : n.start()] + cite
            last = n.end()
        if out is None:
            continue
        out += line[last:]
        if out != line:
            lines[i] = out
            moved += 1
    open(AUDIT, "w").write("\n".join(lines))
    return moved


def nearest():
    report = subprocess.run(
        [sys.executable, os.path.join(ROOT, "scripts", "buddy-audit-check.py")], capture_output=True, text=True
    ).stdout
    text = open(AUDIT).read()
    code = {}
    moved = []
    for m in re.finditer(r": ([a-z_/]+\.rs):(\d+)(?:=([^:]+))?: the line does not hold '([^']+)'", report):
        name, n, anchor, want = m.group(1), int(m.group(2)), m.group(3), m.group(4)
        path = os.path.join(ROOT, HUB, name)
        if not os.path.exists(path):
            continue
        lines = code.setdefault(name, open(path).read().split("\n"))
        end = next((i for i, l in enumerate(lines) if re.match(r"\s*mod (tests|flow)\b", l)), len(lines))
        found = [i for i in range(end) if f'"{want}"' in lines[i]] or [i for i in range(end) if want in lines[i]]
        if not found:
            continue
        best = min(found, key=lambda i: abs(i + 1 - n)) + 1
        old = f"`{name}:{n}={anchor}`" if anchor else f"`{name}:{n}`"
        new = f"`{name}:{best}={anchor}`" if anchor else f"`{name}:{best}`"
        if old in text:
            text = text.replace(old, new)
            moved.append(f"{old} -> {best}: {lines[best - 1].strip()[:60]}")
    open(AUDIT, "w").write(text)
    return moved


if __name__ == "__main__":
    print(f"rows followed from HEAD: {follow_rows()}")
    for line in sorted(set(nearest())):
        print(f"moved to the nearest line: {line}")
