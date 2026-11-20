# The SDK's fileinfo.py: our port against the reference

**The reference:** the Connect SDK's `prusa/connect/printer/fileinfo.py` at `04c5bd5`
(`reference/Prusa-Connect-SDK-Printer`; "Freeware" in its `setup.py`, ported on the owner's decision of 2026-11-12), with gcode-metadata at `7d8f25d` (what it
calls for a file's details), on CPython 3.13.12.

**Ours:** `crates/connect-sdk/src/fileinfo.rs` (Unix only, as `crate::pyos`), on our ports of
CPython's os calls ([connect-sdk-pyos.md](connect-sdk-pyos.md)) and of gcode-metadata
([gcode-metadata.md](gcode-metadata.md)).

| Function of `fileinfo.py` | Ours |
|---|---|
| `MAX_FOLDER_ITEMS`, `REASON_NOT_FOUND`, `REASON_OVERSIZED`, `REASON_FORBIDDEN` | the same constants |
| `get_type_by_name` | `get_type_by_name` |
| `is_inside` | `is_inside` |
| `folder_info` | `folder_info` |
| `file_info` | `file_info` |
| `node_info` | not yet: it takes `files.File`, ported with `files.py` |
| `resolve` | not yet: it takes `files.Storage`, ported with `files.py` |

## Results

`crates/connect-sdk/tests/fileinfo_reference.rs` is the checker: `tests/fileinfo_scenario.py`
builds the trees and runs the reference's steps; the test runs ours on the same steps and
compares them one by one. An info is compared as `json.dumps(info)`, the text that goes to
Connect; a `CommandRejected` by its reason, another exception by its `repr`. Each side reads a
storage of its own for `file_info`, so the caches the reference writes are not read by ours.
It wants CPython 3.13.12 with the SDK and gcode-metadata (`FILEINFO_PYTHON`, else
`reference/.venv-connect-sdk-313`) and gcode-metadata's test files (`FILEINFO_GCODES`); without
them it says "skipped" and passes. CI's job `store` runs it on Linux (owner, 2026-11-20: the
SDK is installed in CI only).

| # | What | Result |
|---|---|---|
| 1 | `get_type_by_name` on 18 names (cases, `.hex`, no extension, `.gcode` alone, `.bgcode`, `.sl1`, `""`, `İ`, `K` (Kelvin sign), `é`), as file and as folder | not yet run |
| 2 | `is_inside` of 14 paths (each entry of a storage with symlinks inside, outside, dangling and back in; a missing path; `..` out; through a link) against 4 storages (the storage, with `/`, by a symlink, another folder) | not yet run |
| 3 | `folder_info` of 6 folders (the root, nested, empty, a link inside, a link back), the storage named by its symlink, a folder of 20,001 items (`Oversized command`), one of 20,000 plus a hidden file, a file, a missing path (`File not found`) | not yet run |
| 4 | `file_info` of 17 files: gcode-metadata's 8 FDM test files, a small, an empty, a binary and an upper-case `.GCO` G-code, `.hex`, `.txt`, a read-only file, a file without permissions, a `.g` in a subfolder; each twice (the second from the cache written by the first) | not yet run |
| 5 | `file_info` of a removed file (another file's stat), and the cache files left in the folder | not yet run |
| 6 | Rows 1 to 5 on Linux, CPython 3.13.12 | **not yet run**: CI only (job `store`) |

On macOS, the scenario was only run with stand-ins for the SDK's imports `requests` and
`mypy_extensions` (not installed; owner, 2026-11-20): all 49 steps the same. That run does not
count for this audit.

## Not covered

- `node_info` and `resolve`: with the port of `files.py`.
- Known differences, none in the tests: a name that is not UTF-8 (Python's `json.dumps` writes
  its lone surrogates as `\udcXX`; ours writes U+FFFD per bad byte); a metadata `int` beyond 64
  bits (ours: a float); a NaN or infinite `float` (Python writes `NaN`/`Infinity`; ours `null`);
  a file path that is not UTF-8 (ours reads no metadata: gcode-metadata takes a `&str`).
- SL1 files (`.sl1`, `.sl1s`): gcode-metadata's SL1 part is left out (owner, 2026-11-20), so
  `file_info` reads no metadata for them; only their type is checked (row 1).
- `.bgcode` files: none is in this scenario.
