"""What the SDK's files.py (c0d7d233, with gcode-metadata 7d8f25d) gives, for
`tests/files_reference.rs` to compare with ours (`src/files.rs`).

    python -I files_scenario.py build D       the folder both read (D, empty)
    python -I files_scenario.py fs D          a Filesystem of D without inotify; prints JSON
    python -I files_scenario.py act D STEP    one of STEPS, done to D
    python -I files_scenario.py inotify D     every step with an InotifyHandler (Linux); JSON

Paths on the disk are shown with D as "<D>". A time (`m_timestamp`) is "T" and a size of the
disk (`free_space`, `total_space`) is "N", as they differ between two runs. An exception is
["raised", repr(err)]. A tree is listed node by node, the children of a folder by name.
"""

import json
import os
import shutil
import sys
import time
import types

from prusa.connect.printer import const
from prusa.connect.printer.files import (
    File,
    Filesystem,
    InotifyHandler,
    Storage,
    common_start,
    delete,
)

T = 1700000000  # every mtime of the built folder


def write(p, data):
    with open(p, "w", encoding="utf-8") as f:
        f.write(data)


def build(d):
    for sub in ["a/b", "a/c", "a/.hid", ".h", "empty"]:
        os.makedirs(os.path.join(d, sub))
    write(os.path.join(d, "a/1.gcode"), "G1 X1\n")
    write(os.path.join(d, "a/c/2.sl1"), "x" * 10)
    write(os.path.join(d, "a/c/3.txt"), "text")
    write(os.path.join(d, "a/.hid/x.gcode"), "G1\n")
    write(os.path.join(d, ".h/2.gcode"), "G1\n")
    write(os.path.join(d, ".h/.hidden.gcode"), "G1\n")
    write(os.path.join(d, "top.GCODE"), "G28\n")
    write(os.path.join(d, "fw.hex"), ":00000001FF\n")
    write(os.path.join(d, "é.gcode"), "G1\n")
    for top, dirs, files in os.walk(d, topdown=False):
        for n in dirs + files:
            os.utime(os.path.join(top, n), (T, T))
    os.utime(d, (T, T))


def norm(value, d):
    """The output as compared: see the module's doc."""
    if isinstance(value, dict):
        out = {}
        for k, v in value.items():
            if k == "m_timestamp":
                out[k] = "T"
            elif k in ("free_space", "total_space"):
                out[k] = "N"
            else:
                out[k] = norm(v, d)
        return out
    if isinstance(value, (list, tuple)):
        return [norm(v, d) for v in value]
    if isinstance(value, str):
        return value.replace(d.rstrip("/"), "<D>")
    return value


def call(d, f, *args):
    try:
        return norm(f(*args), d)
    except Exception as err:  # pylint: disable=broad-except
        return ["raised", norm(repr(err), d)]


def dump(fs):
    """Every node of every storage: [virtual path, to_dict(False)]."""
    out = []

    def walk(node, path):
        out.append([path, node.to_dict(include_children=False)])
        for name in sorted(node.children):
            walk(node.children[name], path + "/" + name)

    for name, storage in fs.storage_dict.items():
        walk(storage.tree, "/" + name)
    return out


def node_name(node):
    return None if node is None else node.name


def run_fs(d):
    fs = Filesystem()
    out = {}
    out["from_dir"] = call(d, fs.from_dir, d, "test")
    out["tree"] = norm(dump(fs), d)
    out["storages"] = call(d, fs.storages)
    out["storage_str"] = str(fs.storage_dict["test"]).replace(d.rstrip("/"), "<D>")
    gets = ["/test", "test", "/test/", "/test/a", "/test/a/1.gcode", "/test/a/c/3.txt",
            "/test/.h", "/test/a/.hid", "/test/a/.hid/x.gcode", "/test//a", "/nothing/a",
            "", "/", "/test/a/missing", "/test/é.gcode"]
    out["get"] = [[p, node_name(fs.get(p))] for p in gets]
    out["get_os_path"] = [[p, call(d, fs.get_os_path, p)] for p in gets]
    handler = types.SimpleNamespace(fs=fs)
    abs_os = ["/test/a/1.gcode", "/test", "/test/", "test/a", "/nothing/a", "//test/a",
              "/test/../a", "/test/./a/", "/", "."]
    out["get_abs_os_path"] = [
        [p, call(d, InotifyHandler.get_abs_os_path, handler, p)] for p in abs_os]
    infos = ["/test", "/test/a", "/test/a/1.gcode", "/test/top.GCODE", "/test/fw.hex",
             "/test/a/missing", "/test/../x", "/test/.h", "/nothing", "/", "/test/a/1.gcode/x",
             "/test/empty"]
    out["file_info"] = [[p, call(d, fs.file_info, p)] for p in infos]

    tree = File("sd", is_dir=True)
    tree.add("x.gcode", size=5, m_timestamp=7)
    sub = tree.add("sub", is_dir=True)
    sub.add("y.txt", size=3, read_only=True)
    out["attach_sd"] = call(d, fs.attach, "SD Card", tree, "", False,
                            const.StorageType.SDCARD, 123)
    out["sd_info"] = [[p, call(d, fs.file_info, p)]
                      for p in ["/SD Card", "/SD Card/sub", "/SD Card/x.gcode",
                                "/SD Card/none", "/SD Card/sub/y.txt"]]
    out["sd_storage"] = call(d, fs.storage_dict["SD Card"].to_dict)
    out["attach_errors"] = [
        call(d, fs.attach, "", File("e", is_dir=True)),
        call(d, fs.attach, "a/b", File("e", is_dir=True)),
        call(d, fs.attach, "test", File("e", is_dir=True)),
        call(d, fs.attach, "x", File("e", is_dir=True)),
        call(d, fs.attach, "y", File("e", is_dir=True), "", True),
    ]
    out["storages_after"] = call(d, fs.storages)
    out["detach"] = [call(d, fs.detach, "x"), call(d, fs.detach, "x"),
                     call(d, fs.detach, "SD Card")]
    out["storage_dict"] = list(fs.storage_dict)
    out["storage_none"] = str(Storage(File("n", is_dir=True), "n", None, False))

    out["common_start"] = [common_start(a, b) for a, b in
                           [("/a/b/c", "/a/b/d"), ("abc", "xyz"), ("", "x"), ("é1", "é2")]]
    out["relative"] = [
        InotifyHandler.get_relative_paths(a, b) for a, b in [
            ("/tmp/r", ["/tmp/r/folder"]),
            ("/tmp/r", ["/tmp/r", "/tmp/r/a", "/tmp/r/a/b", "/x/tmp/r/c", "/other"]),
            ("/tmp/r/", ["/tmp/r/", "/tmp/r/a"]),
        ]]
    node = File("n", is_dir=True)
    Filesystem.update(["/s", "/s/a", "/s/a/b", "/s/.h", "/t/x"], "/s", node)
    out["update"] = [[n, c.is_dir] for n, c in node.children.items()]
    out["delete_missing"] = call(d, delete, os.path.join(d, "missing"), False)
    out["delete_dir_full"] = call(d, delete, os.path.join(d, "a"), True)
    return out


def step_many(d):
    src = os.path.join(d, "src-many")
    os.mkdir(src)
    for i in range(25):
        write(os.path.join(src, f"f{i:02}.gcode"), "G1\n")
    shutil.move(src, os.path.join(d, "many"))


STEPS = {
    "create_file": lambda d: write(os.path.join(d, "x.gcode"), ""),
    "mkdir": lambda d: os.mkdir(os.path.join(d, "folder")),
    "file_in_folder": lambda d: write(os.path.join(d, "folder/f.gcode"), "G1 X2\n"),
    "append": lambda d: open(os.path.join(d, "a/1.gcode"), "a",
                             encoding="utf-8").write("Hello"),
    "move_file": lambda d: shutil.move(os.path.join(d, "a/1.gcode"),
                                       os.path.join(d, "a/c")),
    "rename": lambda d: os.rename(os.path.join(d, "a/c/3.txt"), os.path.join(d, "a/c/4.txt")),
    "move_dir": lambda d: os.rename(os.path.join(d, "a/c"), os.path.join(d, "c2")),
    "file_in_moved_dir": lambda d: write(os.path.join(d, "c2/new.gcode"), "G1\n"),
    "delete_file": lambda d: os.unlink(os.path.join(d, "x.gcode")),
    "rmdir": lambda d: os.rmdir(os.path.join(d, "a/b")),
    "hidden": lambda d: write(os.path.join(d, ".hidden.gcode"), "G1\n"),
    "many": step_many,
    "rmtree_many": lambda d: shutil.rmtree(os.path.join(d, "many")),
    "nested": lambda d: (os.makedirs(os.path.join(d, "deep/x/y")),
                         write(os.path.join(d, "deep/x/y/z.gcode"), "G1\n")),
    "rmtree_c2": lambda d: shutil.rmtree(os.path.join(d, "c2")),
    "delete_root": lambda d: shutil.rmtree(d),
}

# notify's thread passes the events on a little later (src/files.rs): both sides wait as long
WAIT = 0.3


def run_inotify(d):
    events = []

    def event_cb(event, source, timestamp=None, command_id=None, **kwargs):
        events.append([event.value, source.value, kwargs])

    fs = Filesystem(event_cb=event_cb)
    fs.from_dir(d, "test")
    handler = InotifyHandler(fs)
    out = [{"step": "init", "events": norm(events, d), "tree": norm(dump(fs), d),
            "wds": len(handler.wds)}]
    for name, step in STEPS.items():
        events.clear()
        step(d)
        time.sleep(WAIT)
        result = call(d, handler)
        out.append({"step": name, "result": result, "events": norm(events, d),
                    "tree": norm(dump(fs), d), "wds": len(handler.wds)})
    return out


def main():
    mode, d = sys.argv[1], sys.argv[2]
    if mode == "build":
        build(d)
    elif mode == "fs":
        print(json.dumps(run_fs(d), ensure_ascii=False))
    elif mode == "act":
        STEPS[sys.argv[3]](d)
    elif mode == "inotify":
        print(json.dumps(run_inotify(d), ensure_ascii=False))
    else:
        raise SystemExit(f"unknown mode {mode}")


if __name__ == "__main__":
    main()
