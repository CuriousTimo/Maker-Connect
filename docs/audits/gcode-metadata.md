# gcode-metadata: our port against Prusa's

**The reference:** prusa3d/gcode-metadata at `7d8f25d` (py-gcode-metadata 0.2.1, LGPLv2+), run
on CPython 3.13.12 (`reference/.venv-connect-sdk-313`). The Connect SDK reads a print file's
details and thumbnails with it (`files.py`, `fileinfo.py`).

**Ours:** `crates/gcode-metadata` (`src/metadata.rs`; the Python it stands on in `src/py.rs` and
`src/py_unicode.rs`).

## Results

| # | What | How it is compared | Result |
|---|---|---|---|
| 1 | The Python built-ins the port stands on (`float()`, `int()`, `round`, `json.loads`/`dumps`, `str` methods, …) | `scripts/gcode-metadata-py-cases.py` records CPython on the same inputs, `tests/py_cases.rs` runs ours | **identical** |
| 2 | `get_metadata(path, False)` on each of the reference's FDM test files (9, `reference/gcode-metadata/tests/gcodes/`): the data as `json.dumps` writes it, the thumbnails by key, length and sha256 | `scripts/gcode-metadata-cases.py` records the reference, `tests/metadata_cases.rs` runs ours | **identical** |
| 3 | The same files fed to `load_from_chunk` in chunks of 1 KiB, 10 KiB, 64 KiB, 2 MiB and 20 MiB | as row 2 | **identical** |
| 4 | The reference's own tests (`tests/test_metadata.py`) | ported test by test to `tests/test_metadata.rs` | **23 of 23 pass**; the 11 SL tests are left out (row 5) |
| 5 | SL1 (`SLMetaData`, `SLKeys`, `SLA_EXTENSIONS`: `.sl1`, `.sl1s`, `.m1`, `.sl2`, `.slx`) | – | **differs**: left out (owner, 2026-11-20: not an FDM printer). The reference reads these files as zip archives; ours gives `UnknownGcodeFileType`. The port with SL1, its zipfile and zlib, and its tests is on the local branch `sl1-parked` |

`scripts/check.sh` runs both recorders with `--check`. Rows 2-4 need `reference/`, which is not
in the repository: without it the tests say "skipped" and pass, so CI does not prove them.

## Not covered

- A cache file whose `metadata` is not a JSON object: Python takes any JSON value as `data`,
  ours refuses it as a `TypeError`, so `load` parses the file instead.
- `str.lower()` of characters whose lowercase differs between Unicode versions.
- `quick_parse` on a file that shrinks while it is read: the reference loops for ever, and so
  does ours (kept as it is, not run).
