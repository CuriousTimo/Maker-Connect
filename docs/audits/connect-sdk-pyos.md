# The SDK's os, os.path and shutil calls: our port against CPython

**The reference:** CPython 3.13.12 (`reference/cpython-3.13`, PSF licence), the functions the
Connect SDK's `files.py` and `fileinfo.py` call: `Lib/posixpath.py`, `Lib/genericpath.py`,
`Lib/os.py` (`walk`), `Lib/shutil.py` (`rmtree`, its branch without fd functions:
`_rmtree_islink`, `_rmtree_unsafe`), `Modules/posixmodule.c` (with
`Modules/clinic/posixmodule.c.h`; `os_access_impl` on POSIX and on Windows), `Python/fileutils.c`
(`_Py_normpath_and_size`, `_Py_skiproot`, on Windows `attributes_to_mode`),
`Objects/exceptions.c` (the `OSError` classes, `repr` and `str`; `command.py` sends
`error=repr(err)`), and on Windows `PC/errmap.h` (`winerror_to_errno`) and `Python/errors.c`
(the message of a Windows error).

**Ours:** `crates/connect-sdk/src/pyos.rs`, one codebase for Linux, macOS and Windows (owner,
2026-11-25, #261: "focus nu op 1 codebase"): the calls through Rust's `std::fs` under CPython's
names, `statvfs` through the crate `fs4`. Per OS only where CPython itself is: the errno of an
error (`errmap.h` on Windows), `access` (`access(2)` on POSIX, the file attributes on Windows),
path bytes and the drive on Windows.

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
| 7 | `statvfs`: free and total bytes (`f_bavail`, `f_blocks` times the block size) of `/dev` and of the tree's file system, a missing path | **identical** on macOS. **Differs** in the block size: CPython's `f_bsize` is the file system's (on macOS `f_iosize`, not `f_frsize`), ours is 1 with the counts in bytes; see "Not covered" |
| 8 | `scandir` of three directories (with and without a trailing `/`): each entry's `name`, `path`, `is_dir()`, `is_dir(follow_symlinks=False)`, `is_symlink()`, `stat()`, `stat(follow_symlinks=False)` | **identical** on macOS |
| 9 | `walk` of 8 tops (the tree, with `/`, relative, a subdirectory, a missing path, a file, the directory without permissions, a symlink to a directory), each top-down and bottom-up, with and without `followlinks`, with and without `onerror` (64 walks) | **identical** on macOS |
| 10 | `rmdir` and `unlink` of a directory, a missing path, a file, a path in the directory without permissions, a symlink, a path through a file | **identical** on macOS |
| 11 | `rmtree` (the reference with `shutil._use_fd_functions = False`, the branch ours ports) of a tree (files, a symlink to a directory and a dangling one inside), a symlink to a directory, a missing path, a file, a directory with a subdirectory without permissions, an empty directory, a directory with a read-only subdirectory; then what is left | **identical** on macOS |
| 12 | Rows 1 to 11 on Linux | **not yet run**: CI only (job `store`) |
| 13 | Windows | **not run against the reference**: built and linted for `x86_64-pc-windows-gnu`; the crate's own tests run in CI's `windows-latest` job, not this checker (it runs CPython on POSIX) |

## Not covered

- `realpath(strict=True)` and `ALLOW_MISSING`: the SDK calls `realpath(path)`; not ported.
- `walk`'s `followlinks=os._walk_symlinks_as_files`: not ported (`_rmtree_unsafe` of 3.13.12
  does not walk).
  A change to `dirnames` by the caller during a top-down walk: ours yields copies (the SDK does
  not change them).
- `rmtree` with `ignore_errors`, `onerror`, `onexc` or `dir_fd`: not ported.
- **Differs**: `rmtree` on Linux and macOS. CPython uses `_rmtree_safe_fd` there (`openat`,
  `unlinkat`; safe against a symlink swapped in during the removal); ours uses `_rmtree_unsafe`, the
  branch CPython takes where the fd functions are missing. Row 11 shows the two differ in the
  `filename` of an error and in what is left after one, so ours is checked against
  `_rmtree_unsafe`, not against what CPython runs on POSIX.
- The calls `download.py` added: `isabs`, `basename`, `dirname`, `remove`, `mkdir`, `makedirs`,
  `rename`, and `str(err)` of an `OSError` (`OSError_str`, with `filename2`, which `download.py`'s
  loop sends as `reason`). This checker does not run them; they are compared only through
  `download.py`'s steps ([connect-sdk-download.md](connect-sdk-download.md)).
- A `str` path with lone surrogates other than `surrogateescape`'s, and non-UTF-8 names in the
  tree: the tree has none (macOS refuses them); on bytes the functions act on `/` and `.` only.
- `getcwd` failing (the cwd removed): ported by `std::env::current_dir`, not run.
- `stat` fields the SDK does not read (`st_atime`, `st_ctime`, `st_nlink`, `st_ino`, ...): not
  ported. `st_mode` holds only the file type (directory, symlink, regular file, else 0), no
  permission bits.
- **Differs**: `statvfs`'s `f_bsize` is 1 and `f_bavail`/`f_blocks` count bytes. The SDK
  multiplies by `f_bsize` (`files.py:301-303`): on Linux CPython gives the same bytes; on macOS
  CPython's `f_bsize` is `f_iosize`, so the SDK's numbers there are too large and ours are not.
- **Differs**: `mkdir(path)` has no `mode`; the SDK never passes one (`os.makedirs(path)`), and
  `std` uses CPython's default 0o777 less the umask.
- Windows, where ours differs from CPython there (none of it run against the reference):
  - paths are posixpath's with `/` and a drive (`C:/`), not `ntpath`'s: no `\`, no UNC paths
    (`//server/share`), a drive-relative path (`C:x`) is relative, `relpath` and `commonpath` of
    paths on two drives are not refused;
  - `rename` replaces an existing `dst` (`std`, as POSIX `rename(2)`); CPython's raises
    `FileExistsError`;
  - `statvfs`: CPython has no `os.statvfs` on Windows; ours gives `shutil.disk_usage`'s sizes in
    blocks of 1 byte;
  - `access` reads the attributes through `std`'s `symlink_metadata`, not `GetFileAttributesW`:
    `std` (`library/std/src/sys/fs/windows.rs`, `fn metadata`) opens the path with
    `FILE_FLAG_OPEN_REPARSE_POINT` and reads the handle's information, and on
    `ERROR_SHARING_VIOLATION` or `ERROR_ACCESS_DENIED` uses `FindFirstFileExW`; where the two
    disagree is not known;
  - a junction counts as a symlink (`std`), so `walk` and `rmtree` do not enter it and `islink`
    is true, where CPython's `islink` is false for a junction.
