#!/usr/bin/env python3
"""Writes the regex patterns, and the Unicode decimal digits Python accepts, of OctoPrint 1.11.8's comm.py exactly as Python builds them, one
per line (`name<TAB>pattern`), for the Marlin port's test (crates/engine/src/ports/octoprint/
comm_patterns.txt). Run from the repository root."""
import ast, json

SRC = "reference/OctoPrint-1.11.8/src/octoprint/util/comm.py"
OUT = "crates/engine/src/ports/octoprint/comm_patterns.txt"
tree = ast.parse(open(SRC).read())
env, out = {}, []
for node in tree.body:
    if not (isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name)):
        continue
    name, v = node.targets[0].id, node.value
    if name.startswith("regex_") and name.endswith("_pattern"):
        env[name] = eval(compile(ast.Expression(v), SRC, "eval"), {}, env)
    elif isinstance(v, ast.Call) and getattr(v.func, "attr", None) == "compile":
        out.append((name, eval(compile(ast.Expression(v.args[0]), SRC, "eval"), {}, env)))
    elif isinstance(v, ast.Dict) and name == "regexes_parameters":
        for k, call in zip(v.keys, v.values):
            out.append((f"regexes_parameters.{k.value}", eval(compile(ast.Expression(call.args[0]), SRC, "eval"), {}, env)))
with open(OUT, "w") as f:
    for name, pat in out:
        f.write(f"{name}\t{json.dumps(pat)}\n")
print(f"{len(out)} patterns written to {OUT}")

# Python's int() and float() take any Unicode decimal digit (unicodedata.decimal): the first
# code point of each block of ten, as this Python knows them, for the port's py_int/py_float.
import sys, unicodedata
zeros = [c for c in range(0x110000) if unicodedata.decimal(chr(c), None) == 0]
assert all(unicodedata.decimal(chr(z + i), None) == i for z in zeros for i in range(10))
with open("crates/engine/src/ports/octoprint/py_decimal_zeros.txt", "w") as f:
    f.write(f"# unicodedata.decimal, Python {sys.version.split()[0]}, Unicode {unicodedata.unidata_version}\n")
    f.write(" ".join(f"{z:X}" for z in zeros) + "\n")
print(f"{len(zeros)} decimal digit blocks written")
