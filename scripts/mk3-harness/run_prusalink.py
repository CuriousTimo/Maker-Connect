#!/usr/bin/env python3
"""Runs the real PrusaLink (reference/Prusa-Link, installed in reference/.venv-prusalink) in
the foreground on a given serial port. Only modules that never talk to the printer are
stubbed, because they exist on Linux only: prctl (thread names), pyric (Wi-Fi address),
pyudev (finding USB printers: none on a pseudo-terminal), ipcqueue (several printers),
turbojpeg/numpy (cameras), magic (file types for the web pages), and inotify (watching the
G-code folder). websocket (websocket-client, the SDK's link to Connect, ws.py) is stubbed as the
PrusaLink tests' drivers stub it: the config here names no Connect, so it is never used.

PrusaLink and its Connect SDK are the pinned checkouts, put first on the path as the PrusaLink
tests' drivers do (crates/engine/src/ports/prusalink/driver_common.py): PrusaLink at
reference/Prusa-Link (branch websockets, 171148ce) and the SDK at
reference/Prusa-Connect-SDK-Printer-c0d7d233 (branch websockets, c0d7d233), not the copies in the
venv's site-packages.

Usage: run_prusalink.py <config.ini> <serial-port>
"""
import subprocess, sys, types
from pathlib import Path

import time

ROOT = Path(__file__).resolve().parents[2]
LINK = ROOT / "reference" / "Prusa-Link"
SDK = ROOT / "reference" / "Prusa-Connect-SDK-Printer-c0d7d233"
for checkout, pin in ((LINK, "171148ce"), (SDK, "c0d7d233")):
    head = subprocess.run(["git", "-C", str(checkout), "rev-parse", "HEAD"],
                          capture_output=True, text=True, check=True).stdout
    if not head.startswith(pin):
        sys.exit(f"{checkout} is at {head.strip()}, not the pin {pin}")
sys.path[:0] = [str(LINK), str(SDK)]

for name in ["prctl", "ipcqueue", "ipcqueue.posixmq", "turbojpeg", "numpy", "magic", "websocket"]:
    module = types.ModuleType(name)
    module.__getattr__ = lambda attr: (lambda *a, **k: None)
    sys.modules[name] = module

# pyric (Wi-Fi): no wireless interfaces, as on a printer on a cable.
pyric = types.ModuleType("pyric")
pyw = types.ModuleType("pyric.pyw")
pyw.interfaces = lambda: []
pyw.winterfaces = lambda: []
pyw.iswireless = lambda name: False
pyw.Card = object
pyric.error = OSError
pyric.pyw = pyw
sys.modules["pyric"], sys.modules["pyric.pyw"] = pyric, pyw


# pyudev: no device is a known USB printer (a pseudo-terminal has no udev entry), so
# get_usb_printers() finds none, as it would for a port without a USB serial number.
class _Context:
    def list_devices(self, **kwargs):
        return []


sys.modules["pyudev"] = types.ModuleType("pyudev")
sys.modules["pyudev"].Context = _Context

# inotify_simple: its flags are kept; only INotify (Linux's inotify) is replaced by one that
# never reports a change in the G-code folder (watching files, nothing to do with the printer).
import inotify_simple  # noqa: E402


class _INotify:
    def __init__(self):
        self._next = 0

    def add_watch(self, path, mask):
        self._next += 1
        return self._next

    def rm_watch(self, wd):
        pass

    def read(self, timeout=None, read_delay=None):
        time.sleep((timeout or 0) / 1000)
        return []

    def close(self):
        pass


inotify_simple.INotify = _INotify

# termios.IUCLC (Linux: map upper case to lower case on input) does not exist on macOS;
# PrusaLink only clears it (serial.py:59), so 0 does there what it does on Linux.
import termios  # noqa: E402

if not hasattr(termios, "IUCLC"):
    termios.IUCLC = 0

from prusa.link.__main__ import main  # noqa: E402

sys.argv = ["prusalink", "-f", "-c", sys.argv[1], "-s", sys.argv[2]]
sys.exit(main())
