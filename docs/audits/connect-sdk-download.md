# The SDK's download.py: our port against the reference

**The reference:** the Connect SDK's `prusa/connect/printer/download.py` at `c0d7d233` (branch
`websockets`, the SDK the agent follows: `docs/design/wire.md`; `reference/Prusa-Connect-SDK-Printer`;
"Freeware" in its `setup.py`, ported on the owner's decision of 2026-11-12), on CPython 3.13.12.

**Ours:** `crates/connect-sdk/src/download.rs` (Unix only, as `crate::pyos`), on our ports of
CPython's os calls ([connect-sdk-pyos.md](connect-sdk-pyos.md)) and of the SDK's `util.py`
(`Session::get_stream`, HTTP through `ureq`).

| Part of `download.py` | Ours |
|---|---|
| `DOWNLOAD_TYPES` | `DOWNLOAD_TYPES` |
| `TransferRunningError`, `TransferAbortedError`, `TransferStoppedError`, `ForbiddenCharactersError`, `FilenameTooLongError`, `FoldernameTooLongError` | `Exception`, with the `repr` and `str` Python gives them |
| `forbidden_characters`, `filename_too_long`, `foldername_too_long`, `generate_transfer_id` | the same functions |
| `Transfer`: `transferred`, `in_progress`, `start`, `stop`, `reset`, `progress`, `time_transferring`, `time_remaining`, `to_dict` | `Transfer`, the same methods |
| `Transfer.get_speed` | not ported: nothing in the SDK calls it |
| `DownloadMgr`: `start`, `to_os_path`, `loop`, `stop_loop`, `info`, `download`, `tmp_filename` | `DownloadMgr`, the same (`loop` is `run_loop`) |
| `files.Filesystem` (what `to_os_path` reads of it: `sep`, each storage's `path_storage`) | `Filesystem`, `Storage`: just those (`files.py` is not ported) |

## Results

`crates/connect-sdk/tests/download_reference.rs` is the checker: `tests/download_scenario.py
run` runs the reference's steps; the test runs ours on the same steps and compares them one by
one. Both download from one HTTP server (`download_scenario.py serve`, stdlib only), which logs
the headers of each request. A step's record: how `DownloadMgr.start` ended (its dict, or the
exception's `repr`), the events the loop sent, the files left in the folder and the headers the
server got; the random `transfer_id` and the clock's fields are masked. It wants CPython 3.13.12
with the SDK at `c0d7d233` (`DOWNLOAD_PYTHON`, else `reference/.venv-connect-sdk-313`); without it
it says "skipped" and passes. CI's job `store` runs it on Linux (owner, 2026-11-20: the SDK is
installed in CI only).

| # | What | Result |
|---|---|---|
| 1 | `start` without the loop, 14 paths: no leading `/`, a storage alone, an unknown storage, a storage with no path, `..`, `#` and `?` in the name, a name of 256 bytes (`é` × 125 + `.gcode`), one of 249 bytes with no extension, a folder of 256 bytes (ASCII and `é`), a storage path ending in `/`, `//` and `/./` | not yet run |
| 2 | Downloads, 26 steps: from Connect with its token; another server (`User-Agent` only); no server; the server in upper case; `text/plain`; a 404; `text/html`; a charset; an upper-case type; no content type; an empty file; chunked with no length; gzip; deflate; into new folders, an existing folder and a file where a folder is wanted; stopped at the first chunk; onto the file being printed; from the web; a transfer type the loop leaves alone; a second `start` while one runs; an unreachable server; an invalid storage in the loop, after a good one, and again | not yet run |
| 3 | Rows 1 and 2 on Linux, CPython 3.13.12 | **not yet run**: CI only (job `store`) |

On macOS, the scenario was only run with stand-ins for the SDK's imports (not installed; owner,
2026-11-20): all 40 steps the same, with gzip and "unreachable" taken as under "Not covered". That run does not count for
this audit.

## Not covered

- Known difference, checked to be exactly that: "gzip". The reference's `TRANSFER_INFO` event has
  `size`, the compressed length from `Content-Length`; ours has none, because `ureq` drops
  `Content-Length` from a response it decompresses.
- Not compared: the `reason` of "unreachable", `str()` of a `requests` exception (urllib3's
  words); both must give one.
- `Transfer.get_speed`: not ported (no caller in the SDK).
- `files.Filesystem` beyond `sep` and `path_storage`: with the port of `files.py`.
- Windows: `download.rs` is Unix only, as `pyos.rs`.
