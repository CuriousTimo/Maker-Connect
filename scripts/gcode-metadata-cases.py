#!/usr/bin/env python3
"""Record what gcode-metadata (prusa3d/gcode-metadata at 7d8f25d) gives on its own test files.

Reads every file in reference/gcode-metadata/tests/gcodes/ with the reference: `get_metadata(path,
False)` (the data as `json.dumps` writes it, the thumbnails by key, length and sha256), and the
same file fed to `load_from_chunk` in chunks of several sizes (as its tests' `chunk_read_file`).
Writes crates/gcode-metadata/tests/gcode-metadata-cases.json; `tests/metadata_cases.rs` runs ours
on the same files and wants the same. `--check` fails when the file is not what this Python
records. Caches are not written (`save_cache=False`); the reference's own `.fdn_filename.gcode.cache`
is read by both, as the reference reads it. SL files (`.sl1`, the reference's `SLA_EXTENSIONS`)
are left out: our port has no SL1 (owner, 2026-11-20).

    reference/.venv-connect-sdk-313/bin/python -I scripts/gcode-metadata-cases.py [--check]
"""

import hashlib
import json
import os
import sys
from pathlib import Path

from gcode_metadata import get_meta_class, get_metadata
from gcode_metadata.metadata import SLA_EXTENSIONS

ROOT = Path(__file__).resolve().parent.parent
GCODES = ROOT / "reference" / "gcode-metadata" / "tests" / "gcodes"
OUT = ROOT / "crates" / "gcode-metadata" / "tests" / "gcode-metadata-cases.json"
CHUNKS = [1024, 10 * 1024, 64 * 1024, 2 * 1024 * 1024, 20 * 1024 * 1024]


def outcome(call):
    """('ok', value) or (class name, message)"""
    try:
        return ["ok", call()]
    except Exception as err:  # pylint: disable=broad-except
        return [type(err).__name__, str(err)]


def shown(meta):
    return {
        "data": json.dumps(meta.data),
        "thumbnails": [[k, len(v), hashlib.sha256(v).hexdigest()]
                       for k, v in meta.thumbnails.items()],
    }


def chunked(path, size):
    def run():
        meta = get_meta_class(str(path))
        with open(path, "rb") as file:
            file_size = file.seek(0, os.SEEK_END)
            file.seek(0)
            while True:
                chunk = file.read(size)
                if not chunk:
                    break
                meta.load_from_chunk(chunk, file_size)
        return shown(meta)
    return outcome(run)


def main():
    cases = {}
    for path in sorted(GCODES.iterdir()):
        if path.name.endswith(".cache") or path.name.lower().endswith(SLA_EXTENSIONS):
            continue
        cases[path.name] = {
            "whole": outcome(lambda: shown(get_metadata(str(path), False))),
            "chunks": {str(n): chunked(path, n) for n in CHUNKS},
        }
    text = json.dumps({"python": sys.version.split()[0], "cases": cases}, indent=1,
                      ensure_ascii=False) + "\n"
    if "--check" in sys.argv:
        if OUT.read_text() != text:
            sys.exit("gcode-metadata-cases.json is not what this Python records")
        return
    OUT.write_text(text)


if __name__ == "__main__":
    main()
