#!/usr/bin/env python3
"""Extract from OctoPrint 1.11.8 comm.py (reference/OctoPrint-1.11.8) the class constants and the
attributes assigned in MachineCom.__init__ (lines 444-845), for the test in
crates/engine/src/ports/octoprint/comm.rs (`machinecom_attrs_exist`).

Output (machinecom_attrs.txt), one line per name:
  const NAME
  attr NAME [reassigned] [mutated]
`reassigned`: the attribute is assigned again (=, +=, ...) in another method of MachineCom (a
Python attribute read/write is atomic on its own, so these become `Attr<T>` / atomics).
`mutated`: a method is called on it, or it is subscripted/deleted, outside __init__ (in-place
change of the object it holds; one call is atomic under the GIL, so these become `Attr<T>` too).
"""
import ast, os, sys

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
src = os.path.join(root, "reference/OctoPrint-1.11.8/src/octoprint/util/comm.py")
out = os.path.join(root, "crates/engine/src/ports/octoprint/machinecom_attrs.txt")
tree = ast.parse(open(src).read())
cls = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "MachineCom")


def self_attr(n):
    return (
        isinstance(n, ast.Attribute)
        and isinstance(n.value, ast.Name)
        and n.value.id == "self"
    )


consts = []
for n in cls.body:
    if isinstance(n, ast.Assign) and n.lineno < 500:
        for t in n.targets:
            if isinstance(t, ast.Name):
                consts.append(t.id)
init = next(n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == "__init__")
attrs = []
for n in ast.walk(init):
    targets = []
    if isinstance(n, ast.Assign):
        targets = n.targets
    elif isinstance(n, (ast.AugAssign, ast.AnnAssign)):
        targets = [n.target]
    for t in targets:
        if self_attr(t) and t.attr not in attrs:
            attrs.append(t.attr)

reassigned, mutated = set(), set()
for m in cls.body:
    if not isinstance(m, ast.FunctionDef) or m is init:
        continue
    for n in ast.walk(m):
        targets = []
        if isinstance(n, ast.Assign):
            targets = n.targets
        elif isinstance(n, (ast.AugAssign, ast.AnnAssign)):
            targets = [n.target]
        for t in targets:
            if self_attr(t):
                reassigned.add(t.attr)
            elif isinstance(t, ast.Subscript) and self_attr(t.value):
                mutated.add(t.value.attr)
        if isinstance(n, ast.Delete):
            for t in n.targets:
                if isinstance(t, ast.Subscript) and self_attr(t.value):
                    mutated.add(t.value.attr)
        if (
            isinstance(n, ast.Call)
            and isinstance(n.func, ast.Attribute)
            and self_attr(n.func.value)
        ):
            mutated.add(n.func.value.attr)

with open(out, "w") as f:
    f.write("# written by scripts/octoprint-machinecom-attrs.py from comm.py (MachineCom, 444-845)\n")
    for c in consts:
        f.write(f"const {c}\n")
    for a in attrs:
        f.write(f"attr {a}" + (" reassigned" if a in reassigned else "") + (" mutated" if a in mutated else "") + "\n")
print(len(consts), "consts", len(attrs), "attrs ->", out)
