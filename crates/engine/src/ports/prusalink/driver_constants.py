import ast
import collections.abc
import enum
import os
import re

from packaging.version import Version

# Every name a module defines at its top level, read from its source (an assignment, an annotated
# assignment or a class), so no name is left out by eye; its value as Python builds it, in one
# canonical JSON form the Rust test makes too.
request = json.load(sys.stdin)


def canon(value):
    if isinstance(value, enum.Enum):
        return {"enum": type(value).__name__, "value": canon(value.value)}
    if value is None or isinstance(value, (bool, int, float, str)):
        return value
    if isinstance(value, re.Pattern):
        return {"pattern": value.pattern, "flags": int(value.flags)}
    if isinstance(value, Version):
        return {"version": str(value)}
    if isinstance(value, (set, frozenset)):
        return {"set": sorted((canon(item) for item in value), key=json.dumps)}
    if isinstance(value, collections.abc.Mapping):
        return {"dict": [[canon(k), canon(v)] for k, v in value.items()]}
    if isinstance(value, tuple):
        return {"tuple": [canon(item) for item in value]}
    if isinstance(value, list):
        return [canon(item) for item in value]
    if isinstance(value, type) and issubclass(value, enum.Enum):
        return {"class": value.__name__,
                "members": [[member.name, canon(member.value)] for member in value]}
    if isinstance(value, type):
        attrs = {name: canon(getattr(value, name)) for name in sorted(dir(value))
                 if not name.startswith("__")}
        return {"class": value.__name__, "attrs": attrs}
    return {"type": type(value).__name__}


modules = {}
for module_name in request["modules"]:
    module = __import__(module_name, fromlist=["_"])
    tree = ast.parse(open(module.__file__, encoding="utf-8").read())
    names = []
    for node in tree.body:
        if isinstance(node, ast.Assign):
            names += [t.id for t in node.targets if isinstance(t, ast.Name)]
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            names.append(node.target.id)
        elif isinstance(node, ast.ClassDef):
            names.append(node.name)
    modules[module_name] = {
        "names": names,
        "values": {name: canon(getattr(module, name)) for name in names},
        "file": os.path.relpath(module.__file__, LINK),
    }
print(json.dumps(modules))
