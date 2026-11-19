"""A printer on the Prusa Connect SDK, linked to a hub over /p/ws, as PrusaLink drives the SDK.

The SDK is the reference (Prusa-Connect-SDK-Printer branch feat/ws-connect-local @ 04c5bd59); this
script only drives it. Usage: python link.py HOST PORT. It prints one line per step:
CODE <code>, TOKEN, LINKED, GOT <gcode>. Exits 0 after the first command, 1 on a timeout.
"""
import sys
import threading
import time
import types

# The SDK watches its local file tree with Linux's inotify; off Linux the watcher is a stub. It
# touches nothing on the wire.
if not sys.platform.startswith("linux"):
    stub = types.ModuleType("inotify_simple")

    class INotify:  # pylint: disable=too-few-public-methods
        def add_watch(self, *_):
            return 0

        def rm_watch(self, *_):
            pass

        def read(self, *_, **__):
            return []

        def fileno(self):
            return -1

    class flags:  # pylint: disable=invalid-name,too-few-public-methods
        CREATE = DELETE = MODIFY = MOVED_FROM = MOVED_TO = DELETE_SELF = MOVE_SELF = 0
        CLOSE_WRITE = ISDIR = ATTRIB = 0

        @staticmethod
        def from_mask(_):
            return []

    stub.INotify, stub.flags = INotify, flags
    sys.modules["inotify_simple"] = stub

from prusa.connect.printer import Printer, const

import logging
logging.basicConfig(level=logging.DEBUG if "-v" in sys.argv else logging.WARNING, stream=sys.stderr)
host, port = sys.argv[1], int(sys.argv[2])
printer = Printer(const.PrinterType.I3MK3S, "CZPX0000X000XK00001", "f" * 32)
printer.set_connection(Printer.connect_url(host, False, port), "")
got = threading.Event()


def gcode(caller):
    print("GOT", caller.kwargs["gcode"], flush=True)
    got.set()
    return {"source": const.Source.MARLIN}


printer.set_handler(const.Command.GCODE, gcode)
print("CODE", printer.register(), flush=True)
threading.Thread(target=printer.loop, daemon=True).start()

deadline = time.monotonic() + 60
while not printer.token and time.monotonic() < deadline:
    time.sleep(0.2)
if not printer.token:
    sys.exit(1)
print("TOKEN", flush=True)

printer.set_state(const.State.IDLE, const.Source.MARLIN, ready=True)
# As the SDK's README drives it: the loop in a thread, the command handler every 100 ms.
linked = False
last_report = 0.0
while not got.is_set() and time.monotonic() < deadline:
    if time.monotonic() - last_report >= 1:
        printer.telemetry(temp_nozzle=24.0, temp_bed=23.0)
        last_report = time.monotonic()
    if printer.ws.connected and not linked:
        print("LINKED", flush=True)
        linked = True
    printer.command()
    time.sleep(0.1)
sys.exit(0 if got.is_set() else 1)
