r"""pyos on Windows (owner 2026-11-25, #241: "posixpath + drive"), for the reference scenarios
(`pyos_scenario.py` and the SDK's): CPython's os and shutil with posixpath as os.path, and a path
the OS gives (getcwd, readlink, a DirEntry's path) in pyos's form, `pyos::path_bytes`: `\` as `/`,
without `\\?\`. A scenario runs it before it imports the SDK (`from os import path` binds os.path
at import), on Windows only.
"""
import os
import posixpath
import sys


def _pyos_form(p):
    p = p.replace("\\", "/")
    return p[4:] if p.startswith("//?/") else p


class _Entry:
    def __init__(self, entry):
        self._entry = entry
        self.name = entry.name
        self.path = _pyos_form(entry.path)

    def __getattr__(self, name):
        return getattr(self._entry, name)


class _Scandir:
    def __init__(self, it):
        self._it = it

    def __iter__(self):
        return self

    def __next__(self):
        return _Entry(next(self._it))

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self._it.close()

    def close(self):
        self._it.close()


_getcwd, _readlink, _scandir = os.getcwd, os.readlink, os.scandir
os.path = sys.modules["os.path"] = posixpath
os.getcwd = lambda: _pyos_form(_getcwd())
os.readlink = lambda p, *, dir_fd=None: _pyos_form(_readlink(p))
os.scandir = lambda p=".": _Scandir(_scandir(p))
