#!/usr/bin/env python3
"""Buddy 8.0.2's own HTTP response automaton, as data for the hub's tests.

Runs Buddy's generator (`utils/gen-automata/http_client.py` with `common.py`, `parts.py` and `http.py`, read
from `reference/buddy-8.0.2` at `b1043d6` with `git show`, as its build does,
`src/common/http/CMakeLists.txt:18-30`) and writes the tables it compiles, unchanged, to
`crates/hub/src/devices/buddy_http_automaton.json`: the state names, the paths, the transitions
(target, label type, label, fallthrough) and the states (first transition, emit on enter, emit on
leave). The tests run them on Buddy's engine as ported (`buddy_http.rs`, from
`src/common/automata/core.cpp`).

    python3 -I scripts/buddy-http-automaton.py          write the file
    python3 -I scripts/buddy-http-automaton.py --check  fail if the file differs from what
                                                        Buddy's generator makes
"""
import json
import os
import runpy
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TREE = os.path.join(ROOT, "reference", "buddy-8.0.2")
OUT = os.path.join(ROOT, "crates", "hub", "src", "devices", "buddy_http_automaton.json")
FILES = ["common.py", "parts.py", "http.py", "http_client.py"]


def generate():
    with tempfile.TemporaryDirectory() as work:
        for name in FILES:
            text = subprocess.run(
                ["git", "show", f"HEAD:utils/gen-automata/{name}"],
                cwd=TREE, capture_output=True, text=True, check=True,
            ).stdout
            with open(os.path.join(work, name), "w") as f:
                f.write(text)
        # Buddy's `http.py` is imported as `http` (`http_client.py:1`), ahead of Python's own.
        sys.path.insert(0, work)
        for cached in ("http", "common", "parts"):
            sys.modules.pop(cached, None)
        import common  # noqa: E402
        from common import LabelType  # noqa: E402

        # `http_client.py` itself, as its build runs it (in a scratch folder, where it writes its
        # C++); its compiled tables are kept as they are handed to `cpp_header`.
        kept = []
        cpp_header = common.Compiled.cpp_header

        def keep(compiled):
            kept.append(compiled)
            return cpp_header(compiled)

        common.Compiled.cpp_header = keep
        here = os.getcwd()
        os.chdir(work)
        try:
            runpy.run_path(os.path.join(work, "http_client.py"), run_name="__main__")
        finally:
            os.chdir(here)
        (compiled,) = kept
        names = compiled._Compiled__names
        paths = compiled._Compiled__paths
        transitions = []
        for (target, ltype, label, fallthrough) in compiled._Compiled__transitions:
            kind = {LabelType.Char: "Char", LabelType.CharNoCase: "CharNoCase",
                    LabelType.Special: "Special", LabelType.Path: "Path"}[ltype]
            transitions.append([target, kind, label, fallthrough])
        states = [list(s) for s in compiled._Compiled__states]
        return {
            "source": "reference/buddy-8.0.2 b1043d6 utils/gen-automata/http_client.py",
            "names": names,
            "paths": paths,
            "transitions": transitions,
            "states": states,
        }


def main():
    text = json.dumps(generate(), indent=0) + "\n"
    if "--check" in sys.argv:
        have = open(OUT).read() if os.path.exists(OUT) else ""
        if have != text:
            print(f"{os.path.relpath(OUT, ROOT)} differs from Buddy's generator: run scripts/buddy-http-automaton.py")
            sys.exit(1)
        print("Buddy's HTTP response automaton: as its generator makes it")
        return
    with open(OUT, "w") as f:
        f.write(text)
    print(f"wrote {os.path.relpath(OUT, ROOT)}")


if __name__ == "__main__":
    main()
