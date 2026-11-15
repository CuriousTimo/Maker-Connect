#!/usr/bin/env python3
"""Lists every `settings().get*(...)` call in the OctoPrint 1.11.8 files the Marlin port ports
(util/comm.py, printer/standard.py), extracted with Python's own parser, for the test
`every_settings_path_comm_reads_exists` (crates/engine/src/ports/octoprint/settings_call_paths.txt).
Each line: file:line<TAB>method<TAB>path as JSON<TAB>"exists" or "missing" in the defaults
(default_settings.json, OctoPrint's own dump; a missing path makes OctoPrint's get return None). A call whose path is not a literal is written
with path "NOT LITERAL" (not covered). Run from the repository root with python3 -I."""
import ast, json

SRC = "reference/OctoPrint-1.11.8/src/octoprint/"
FILES = ["util/comm.py", "printer/standard.py"]
OUT = "crates/engine/src/ports/octoprint/settings_call_paths.txt"


def literal(node):
    try:
        return ast.literal_eval(node)
    except Exception:
        return None


DEFAULTS = json.load(open("crates/engine/src/ports/octoprint/default_settings.json"))


def exists(path):
    node = DEFAULTS
    for key in path:
        if not isinstance(node, dict) or key not in node:
            return "missing"
        node = node[key]
    return "exists"


lines = []
for name in FILES:
    tree = ast.parse(open(SRC + name).read())
    for node in ast.walk(tree):
        if not (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)):
            continue
        recv = node.func.value
        if not (isinstance(recv, ast.Call) and isinstance(recv.func, ast.Name) and recv.func.id == "settings"):
            continue
        method = node.func.attr
        if not method.startswith("get") or method in ("getBaseFolder",):
            continue
        path = literal(node.args[0]) if node.args else None
        shown = json.dumps(path) if isinstance(path, list) else "NOT LITERAL"
        state = exists(path) if isinstance(path, list) else "unknown"
        lines.append(f"{name}:{node.lineno}\t{method}\t{shown}\t{state}")

lines.sort(key=lambda l: (l.split(":")[0], int(l.split(":")[1].split("\t")[0])))
with open(OUT, "w") as f:
    f.write("\n".join(lines) + "\n")
print(len(lines), "calls;", sum("NOT LITERAL" in l for l in lines), "not literal;",
      [l for l in lines if l.endswith("missing")])
