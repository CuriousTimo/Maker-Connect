"""What the reference (inotify_simple 1.3.5) gives, for `tests/reference.rs` to compare with ours.

    python -I scenario.py pure          parse_events on crafted data, flags, masks (any OS)
    python -I scenario.py linux DIR     a session on a real inotify fd in DIR (Linux)

Both print JSON. `reference.rs` runs ours the same way, step by step, and wants the same.
Events are shown as [wd, flag names, cookie, name as hex bytes]; in the Linux session a cookie
is replaced by its order of first appearance (1, 2, ...; 0 stays 0), as the kernel's counter
differs per run. An OSError is shown by its errno: its text is the platform's.
"""

import fcntl
import json
import os
import struct
import sys
import threading

import inotify_simple
from inotify_simple import INotify, flags, masks, parse_events

# Prusa-Connect-SDK-Printer files.py: InotifyHandler.WATCH_FLAGS
SDK_MASK = (flags.CLOSE_WRITE | flags.CREATE | flags.DELETE | flags.MOVED_TO
            | flags.MOVED_FROM | flags.DELETE_SELF | flags.MOVE_SELF)


def outcome(call):
    try:
        return ["ok", call()]
    except OSError as err:
        return ["OSError", err.errno]
    except Exception as err:  # pylint: disable=broad-except
        return [type(err).__name__, str(err)]


def shown(events, cookies=None):
    out = []
    for e in events:
        cookie = e.cookie
        if cookies is not None and cookie:
            cookie = cookies.setdefault(cookie, len(cookies) + 1)
        out.append([e.wd, [f.name for f in flags.from_mask(e.mask)], cookie,
                    os.fsencode(e.name).hex()])
    return out


def event(wd, mask, cookie, name, namesize=None):
    namesize = len(name) if namesize is None else namesize
    return struct.pack("iIII", wd, mask, cookie, namesize) + name


def pure():
    one = event(1, 0x100, 0, b"a.gcode" + b"\0" * 9)
    datas = [
        b"",
        event(1, 0x100, 0, b""),
        one,
        event(2, 0x40000100, 7, b"no-nul"),
        event(3, 0x200, 0, b"\xff\xfe.gcode\0\0\0\0\0\0\0"),
        event(4, 0x8, 0, b"a\0b\0"),
        one + event(-1, 0xffffffff, 0xffffffff, b"x" * 16),
        b"\x01\x00\x00",
        one + b"\x01" * 5,
        event(5, 0x100, 0, b"abcd", namesize=100),
        event(6, 0x100, 0, b"abcd", namesize=0xffffffff),
    ]
    mask_values = [0, 0x100, 0x40000100, 0x1000, 0x10000, 0xffffffff, masks.ALL_EVENTS, SDK_MASK]
    return {
        "version": inotify_simple.__version__,
        "parse_events": [[d.hex(), outcome(lambda d=d: shown(parse_events(d)))] for d in datas],
        "from_mask": [[m, [f.name for f in flags.from_mask(m)]] for m in mask_values],
        "flags": [[f.name, int(f)] for f in flags.__members__.values()],
        "masks": [[m.name, int(m)] for m in masks.__members__.values()],
    }


def fd_flags(ino):
    return [bool(fcntl.fcntl(ino.fd, fcntl.F_GETFD) & fcntl.FD_CLOEXEC),
            bool(fcntl.fcntl(ino.fd, fcntl.F_GETFL) & os.O_NONBLOCK)]


def write(path, data):
    with open(path, "wb") as f:
        f.write(data)


def linux(root):
    d = os.fsencode(root)
    os.mkdir(d)
    cookies = {}
    steps = []

    def step(name, call):
        steps.append([name, outcome(call)])

    ino = INotify()
    step("fd flags", lambda: fd_flags(ino))
    step("read, nothing, timeout 0", lambda: shown(ino.read(timeout=0)))
    step("read, nothing, timeout 50", lambda: shown(ino.read(timeout=50)))
    step("read, nothing, timeout 20, delay 5",
         lambda: shown(ino.read(timeout=20, read_delay=5)))
    os.mkdir(d + b"/sub")
    wds = {}

    def watch(key, path):
        wds[key] = ino.add_watch(path, SDK_MASK)
        return wds[key]

    step("add_watch root", lambda: watch("root", d))
    step("add_watch sub", lambda: watch("sub", d + b"/sub"))
    step("add_watch root again", lambda: ino.add_watch(d, SDK_MASK))
    step("add_watch missing", lambda: ino.add_watch(d + b"/missing", SDK_MASK))
    step("add_watch, a NUL in the path", lambda: ino.add_watch(d + b"/sub\0ignored", SDK_MASK))

    write(d + b"/a.gcode", b"G28\n")
    os.rename(d + b"/a.gcode", d + b"/sub/b.gcode")
    os.mkdir(d + b"/new")
    write(d + b"/\xff.gcode", b"M84\n")
    os.remove(d + b"/sub/b.gcode")
    os.rmdir(d + b"/new")
    step("read the changes", lambda: shown(ino.read(timeout=0), cookies))

    step("rm_watch sub", lambda: ino.rm_watch(wds["sub"]))
    step("read, IGNORED", lambda: shown(ino.read(timeout=1000), cookies))
    step("rm_watch sub again", lambda: ino.rm_watch(wds["sub"]))

    timer = threading.Timer(0.05, write, (d + b"/late.gcode", b"G1\n"))
    timer.start()
    step("read, waiting, delay 100", lambda: shown(ino.read(timeout=5000, read_delay=100)))
    timer.join()

    os.remove(d + b"/\xff.gcode")
    os.remove(d + b"/late.gcode")
    os.rmdir(d + b"/sub")
    os.rmdir(d)
    step("read, the root removed", lambda: shown(ino.read(timeout=1000, read_delay=50), cookies))

    other = INotify(inheritable=True, nonblocking=True)
    step("fd flags, inheritable and nonblocking", lambda: fd_flags(other))
    step("read nonblocking, nothing", lambda: shown(other.read(timeout=0)))
    other.close()

    step("close", ino.close)
    step("read after close", lambda: shown(ino.read(timeout=0)))
    step("add_watch after close", lambda: ino.add_watch(b"/tmp", SDK_MASK))
    step("rm_watch after close", lambda: ino.rm_watch(1))
    step("close again", ino.close)
    return {"version": inotify_simple.__version__, "steps": steps}


if __name__ == "__main__":
    result = pure() if sys.argv[1] == "pure" else linux(sys.argv[2])
    print(json.dumps(result))
