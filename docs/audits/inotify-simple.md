# inotify_simple: our port against the reference

**The reference:** inotify_simple 1.3.5 (github.com/chrisjbillington/inotify_simple at
`2a8f830`, BSD-2-Clause), the version the Connect SDK requires (`requirements.txt`:
`inotify_simple~=1.3.5`). It stands on CPython's `select.poll`, `fcntl.ioctl`, `os.read`,
`io.FileIO` and `struct`: ported from CPython 3.13.12's sources (`Modules/selectmodule.c`,
`Modules/fcntlmodule.c`, `Python/fileutils.c`, `Modules/_io/fileio.c`).

**Ours:** `crates/inotify-simple/src/lib.rs`.

## Results

`crates/inotify-simple/tests/reference.rs` is the checker: it runs `tests/scenario.py` with the
reference and ours on the same steps and compares the two. It is a test, not a
`scripts/*-audit-check.py` with a table extracted from the source; the reference is run, as for
gcode-metadata. Without a Python with the reference it says "skipped" and passes; CI's job
`store` always has one (`PRUSA_SDK_PYTHON`).

| # | What | Result |
|---|---|---|
| 1 | `parse_events` on 11 crafted buffers: none, one event with and without a name, a name without a NUL, non-UTF-8 bytes, two events, a cut header (at offset 0 and 32), a name size past the data (up to `0xffffffff`) | **identical** on CPython 3.13.12 (`reference/.venv-connect-sdk-313`) |
| 2 | `flags` (names, values, order), `flags.from_mask` on 8 masks, `masks` | **identical**, as row 1 |
| 3 | A session on a real inotify fd (Linux): the fd's `CLOEXEC`/`NONBLOCK` flags; `read` with nothing there (timeout 0, 50, 20 with delay 5); `add_watch` (new, again, missing path, a NUL in the path) with the SDK's `WATCH_FLAGS`; the events of creating, writing, moving, creating a directory, a non-UTF-8 name, deleting; `rm_watch` (and again: `EINVAL`); `read` waiting for an event with `read_delay`; the watched directory removed (`DELETE_SELF`, `IGNORED`); `INotify(inheritable=True, nonblocking=True)`; every call after `close` | **not yet run**: it runs in CI only (job `store`, Linux), with the reference as the SDK installs it, on the runner's `python3` (CPython 3.12 on ubuntu-24.04, not 3.13.12) |

## Not covered

- The rest of `FileIO` that `INotify` inherits (`readinto`, `readall`, `seek`, `with`, …): the
  SDK calls `INotify()`, `add_watch`, `rm_watch` and `read(timeout=...)` only.
- `timeout` and `read_delay` as a `float`: ours takes whole milliseconds as an `i64` (the SDK
  passes `int`s), so a value beyond that cannot be given.
- A `read_delay` too large to sleep: both raise `OverflowError`, with different messages.
- `FIONREAD` giving a negative count: Python's `os.read` would fail; ours reads nothing. The
  kernel does not give one.
- `poll()` interrupted by a signal (`EINTR` and the recomputed timeout), `os.read` retried on
  `EINTR`, `FileIO.__init__` refusing a directory: ported, not run.
- Python 2 (`PY2`): not ported.
