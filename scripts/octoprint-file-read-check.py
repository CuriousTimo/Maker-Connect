#!/usr/bin/env python3
"""Prints what CPython 3.13 gives for the file reading of `PrintingGcodeFileInformation` (comm.py:5896-5982):
`open(f, encoding="utf-8-sig", errors="replace", newline="")`, `readline()`, `seek(pos)`, with `_pos` kept by hand
(`len(line.encode("utf-8"))`, plus the BOM length at start). The expected values of the Rust test
`ours_gcode_file_bom_read_then_resume_seek` (ports/octoprint/comm.rs); the file content there is CONTENT below.

    ~/.local/share/uv/python/cpython-3.13.12-macos-aarch64-none/bin/python3.13 scripts/octoprint-file-read-check.py
"""
import json
import sys
import tempfile

assert sys.version_info[:2] == (3, 13), sys.version
CONTENT = b"\xef\xbb\xbfG28\r\n; caf\xc3\xa9\nG1 X10\rM104 S200\r\nG1 X20\n\nM84"
BOM = b"\xef\xbb\xbf"


def read_all(handle, pos):
    out = []
    while True:
        line = handle.readline()
        pos += len(line.encode("utf-8"))
        out.append([line, pos])
        if not line:
            return out


with tempfile.NamedTemporaryFile(suffix=".gcode") as f:
    f.write(CONTENT)
    f.flush()
    result = {}

    # start(): BOM counted in _pos; read three lines
    h = open(f.name, encoding="utf-8-sig", errors="replace", newline="")
    pos = h.tell() + len(BOM)
    first = []
    for _ in range(3):
        line = h.readline()
        pos += len(line.encode("utf-8"))
        first.append([line, pos])
    result["first_three"] = first
    resume = pos
    h.close()

    # startPrint(pos=resume): a new handle (start()), then seek(resume); _pos = tell(); _read_lines = 0
    h = open(f.name, encoding="utf-8-sig", errors="replace", newline="")
    h.seek(resume)
    pos = h.tell()
    result["resume_pos"] = resume
    result["tell_after_seek"] = pos
    result["after_seek"] = read_all(h, pos)
    h.close()

    # seek(0): the BOM is dropped again, `_pos` is tell() = 0 (the BOM is not counted), then + len(line)
    h = open(f.name, encoding="utf-8-sig", errors="replace", newline="")
    h.readline()
    h.seek(0)
    pos = h.tell()
    line = h.readline()
    result["after_seek0"] = [line, pos + len(line.encode("utf-8"))]
    h.close()

print(json.dumps(result, ensure_ascii=False))
