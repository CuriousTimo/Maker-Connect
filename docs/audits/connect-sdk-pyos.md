# The SDK's os, os.path and shutil calls: our port against CPython

**The reference:** CPython 3.13.12 (`reference/cpython-3.13`, PSF licence), the functions the
Connect SDK's `files.py` and `fileinfo.py` call: `Lib/posixpath.py`, `Lib/genericpath.py`,
`Lib/os.py` (`walk`), `Lib/shutil.py` (`rmtree`, its `_rmtree_safe_fd` as on Linux and macOS),
`Modules/posixmodule.c` (with `Modules/clinic/posixmodule.c.h`), `Python/fileutils.c`
(`_Py_normpath_and_size`, `_Py_skiproot`) and `Objects/exceptions.c` (the `OSError` classes and
`repr`, which `command.py` sends as `error=repr(err)`).

**Ours:** `crates/connect-sdk/src/pyos.rs` (Unix only, as the reference: on Windows `os.path` is
`ntpath`, and `files.py` imports `inotify_simple`, which needs `fcntl`).

## Results

`crates/connect-sdk/tests/pyos_reference.rs` is the checker: it runs `tests/pyos_scenario.py`
with the reference and ours on the same steps and compares the two. Paths go to the reference as
`str`, as the SDK passes them; an exception is compared by its `repr` and its `filename`. It
wants CPython 3.13.12 (`PYOS_PYTHON`, else `reference/.venv-connect-sdk-313`); without one it says
"skipped" and passes. CI's job `store` runs it on Linux with CPython 3.13.12 from `setup-python`.

| # | What | Result |
|---|---|---|
| 1 | `normpath` and `split` on every path of up to 6 characters from `/`, `.`, `a` (1,093 each; every other character acts as `a` in them) | **identical** on macOS |
| 2 | `join` on every pair of up to 3 characters (1,600), every triple of up to 2 (2,197), one part (40) | **identical** on macOS |
| 3 | `commonpath` on one (121), two (14,641) and three (2,197) paths of up to 4 and 2 characters, and none (`ValueError`, and mixing absolute and relative) | **identical** on macOS |
| 4 | `abspath`, `relpath(path, start)` (14,641 pairs) and `relpath(path)` on paths of up to 4 characters, from the same cwd | **identical** on macOS |
| 5 | A NUL in a path: `normpath`, `stat` (`ValueError` naming the function), `exists`/`lexists`/`isdir`/`islink` (`False`), `realpath` | **identical** on macOS |
| 6 | On a tree with files, a hidden file, a read-only file, an empty directory, a directory without permissions, and symlinks (relative, absolute, to a file, dangling, through a file, a loop, to itself, chained, `..`, `/`), 265 paths (each entry with 8 suffixes such as `/..`, `/x`, `//deep`; relative ones from the cwd; `""`, `/`, `//`, `///`, ...): `realpath`, `stat`, `lstat`, `exists`, `lexists`, `isdir`, `islink`, `access(W_OK)`, `abspath`, `relpath` | **identical** on macOS |
| 7 | `statvfs` (`f_bsize`, `f_blocks`, `f_bavail` of `/dev`; `f_bsize`, `f_blocks` of the tree's file system; a missing path) | **identical** on macOS |
| 8 | `scandir` of three directories (with and without a trailing `/`): each entry's `name`, `path`, `is_dir()`, `is_dir(follow_symlinks=False)`, `is_symlink()`, `stat()`, `stat(follow_symlinks=False)` | **identical** on macOS |
| 9 | `walk` of 8 tops (the tree, with `/`, relative, a subdirectory, a missing path, a file, the directory without permissions, a symlink to a directory), each top-down and bottom-up, with and without `followlinks`, with and without `onerror` (64 walks) | **identical** on macOS |
| 10 | `rmdir` and `unlink` of a directory, a missing path, a file, a path in the directory without permissions, a symlink, a path through a file | **identical** on macOS |
| 11 | `rmtree` of a tree (files, a symlink to a directory and a dangling one inside), a symlink to a directory, a missing path, a file, a directory with a subdirectory without permissions, an empty directory, a directory with a read-only subdirectory; then what is left | **identical** on macOS |
| 12 | Rows 1 to 11 on Linux | **not yet run**: CI only (job `store`) |

## Not covered

- `realpath(strict=True)` and `ALLOW_MISSING`: the SDK calls `realpath(path)`; not ported.
- `walk`'s `followlinks=os._walk_symlinks_as_files` (only `_rmtree_unsafe` uses it): not ported.
  A change to `dirnames` by the caller during a top-down walk: ours yields copies (the SDK does
  not change them).
- `rmtree` with `ignore_errors`, `onerror`, `onexc` or `dir_fd`, and `_rmtree_unsafe` (platforms
  without the fd functions): not ported. `os.close` failing while `rmtree` closes its fds, and
  `EINTR`: ported, not run.
- The calls `download.py` added: `isabs`, `basename`, `dirname`, `remove`, `mkdir`, `makedirs`,
  `rename`, and `str(err)` of an `OSError` (`OSError_str`, with `filename2`, which `download.py`'s
  loop sends as `reason`). This checker does not run them; they are compared only through
  `download.py`'s steps ([connect-sdk-download.md](connect-sdk-download.md)).
- A `str` path with lone surrogates other than `surrogateescape`'s, and non-UTF-8 names in the
  tree: the tree has none (macOS refuses them); on bytes the functions act on `/` and `.` only.
- `getcwd` failing (the cwd removed): ported by `std::env::current_dir`, not run.
- `stat` fields the SDK does not read (`st_atime`, `st_ctime`, `st_nlink`, ...): not ported.
