# The SDK's files.py: our port against the reference

**The reference:** the Connect SDK's `prusa/connect/printer/files.py` at `c0d7d233` (branch
`websockets`, the SDK the agent follows: `docs/design/wire.md`; `reference/Prusa-Connect-SDK-Printer-c0d7d233`;
"Freeware" in its `setup.py`, ported on the owner's decision of 2026-11-12), with `inotify_simple`
1.3.5 (its `requirements.txt`), on CPython 3.13.12.

**Ours:** `crates/connect-sdk/src/files.rs` (Linux, macOS and Windows, as `crate::pyos`), on our ports
of CPython's os calls ([connect-sdk-pyos.md](connect-sdk-pyos.md)) and of the SDK's `fileinfo.py`
([connect-sdk-fileinfo.md](connect-sdk-fileinfo.md)). **The watching is ours:** the events come from
the crate `notify` on every OS (owner, 2026-11-25: "volledig gebruik te maken van die rust notify
voor alles. 1 codebase"), turned back into the inotify events the reference's `InotifyHandler`
reads; the table and the differences this brings are at the top of `files.rs`.

| Part of `files.py` | Ours |
|---|---|
| `common_start`, `delete`, `get_file_type` | the same functions |
| `File`: `add`, `get`, `__getitem__`, `__getattr__`, `parent`, `size`, `abs_parts`, `abs_path`, `delete`, `pprint`, `to_dict`, `__contains__`, `__str__`, `fill_attrs` | `File`, the same methods (`__getattr__` is `child`, `__getitem__` is `get`) |
| `Storage`: `get_space_info`, `to_dict`, `__str__` | `Storage`, the same (`__str__` is `str`) |
| `Filesystem`: `attach`, `detach`, `get`, `update`, `get_os_path`, `storages`, `resolve`, `file_info`, `from_dir`, `connect_event`, `wait_until_path` | `Filesystem`, the same |
| `InotifyHandler`: `create_cache`, `delete_cache`, `get_relative_paths`, `filter_delete_events`, `__call__`, `process_*`, `get_abs_os_path`, `attach_for`, `update_watch_dir` | `InotifyHandler`, the same (`__call__` is `call`); its `inotify_simple.INotify` is notify (`to_inotify`) |
| `inotify_simple.flags` | `files::flags` |

## Results

`crates/connect-sdk/tests/files_reference.rs` is the checker: `tests/files_scenario.py` builds the
folder and runs the reference's steps; the test runs ours on the same steps, on a copy of its own,
and compares them key by key. The folder is shown as `<D>`, a time as `T` and a size of the disk as
`N`; an exception by its `repr`. It wants CPython 3.13.12 with the SDK at `c0d7d233` (`FILES_PYTHON`,
else `reference/.venv-connect-sdk-313`); without it it says "skipped" and passes. CI's job `store`
runs it on Linux.

| # | What | Result |
|---|---|---|
| 1 | `from_dir` of a folder with subfolders, hidden folders and files, an empty folder, `.GCODE`, `.hex`, `é`; the tree node by node (`to_dict` without children); `storages()`; `str(storage)` | **identical** on macOS |
| 2 | `get` and `get_os_path` of 15 paths (the storage with and without `/`, nested, hidden, `//`, an unknown storage, `""`, `/`, missing, `é`) | **identical** on macOS |
| 3 | `get_abs_os_path` of 10 paths (`..`, `./`, `//`, no storage, `/`, `.`) | **identical** on macOS |
| 4 | `file_info` of 12 paths of the folder (storage, folder, files, missing, `..` out, hidden, an unknown storage, `/`, through a file, an empty folder): `resolve`, `folder_info` and `file_info` | **identical** on macOS |
| 5 | A storage attached without a path (an SD card's tree, `SDCARD`, system space): `file_info` of 5 paths (`node_info`), its `to_dict` | **identical** on macOS |
| 6 | `attach` refused (empty, with `/`, used, `use_inotify` without a path), `detach` twice and of the SD card, the storages left, `str` of a storage without a path | **identical** on macOS |
| 7 | `common_start` (4), `get_relative_paths` (3), `update`, `delete` of a missing file and of a full folder without `force` | **identical** on macOS |
| 8 | The watching, 16 file actions in a row (create, mkdir, a file in it, append, move a file, rename, move a folder, a file in the moved folder, delete, rmdir, a hidden file, 25 files moved in at once, `rmtree` of them, nested folders, `rmtree`, the storage itself removed): after each the handler's result, the events to Connect, the tree and the number of watches | Linux only: CI job `store` |
| 9 | Rows 1 to 7 on Linux, CPython 3.13.12 | CI job `store` |

The SDK's own tests: `tests/test_files.py` (42) and `tests/test_files_helpers.py` (4) are ported into
`files.rs`'s tests, 45 of 46: `test_get_str` cannot be written (`get` with a `str` is a `TypeError` in
Python; ours takes only a list of parts).

## Not covered

- The watching on macOS (FSEvents: no `CLOSE_WRITE`, other kinds of events) and on Windows: row 8 runs
  on Linux only.
- What notify does differently from `inotify_simple`, listed at the top of `files.rs`: events read a
  moment later, the `ISDIR` of a `MOVED_TO` read again from the disk, the watch of a moved or deleted
  folder ended by notify. Row 8 shows which of them a file action makes visible.
- A name that is not UTF-8: read with U+FFFD per bad byte, where Python keeps lone surrogates (as
  `fileinfo.py`'s audit).
- Windows: built, not run against the reference (its checker runs CPython on POSIX).
