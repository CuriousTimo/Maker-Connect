"""What the SDK's fileinfo.py (04c5bd5, with gcode-metadata 7d8f25d) gives, for
`tests/fileinfo_reference.rs` to compare with ours (`src/fileinfo.rs`).

    python -I fileinfo_scenario.py build-shared D GCODES   the tree read by both (D)
    python -I fileinfo_scenario.py build-copy D GCODES     a storage of its own (D)
    python -I fileinfo_scenario.py run SHARED COPY         the steps; prints JSON

GCODES is gcode-metadata's `tests/gcodes`. An info is shown as `json.dumps(info)`, as it goes to
Connect; `CommandRejected` as ["CommandRejected", reason], another exception as [repr(err)].
"""

import json
import os
import shutil
import sys

from prusa.connect.printer import fileinfo
from prusa.connect.printer.command import CommandRejected

T = 1700000000.75  # every mtime, so the copies are alike


def write(p, data):
    with open(p, "wb") as f:
        f.write(data)


def fix_times(d):
    for top, dirs, files in os.walk(d, topdown=False):
        for n in dirs + files:
            os.utime(os.path.join(top, n), (T, T), follow_symlinks=False)
    os.utime(d, (T, T))


NAMES = ["a.gcode", "a.GCODE", "a.Gc", "a.g", "a.gco", "a.gcode.txt", "fw.hex", "fw.HEX",
         "b.txt", "gcode", ".gcode", "a.bgcode", "a.sl1", "", "a.gİ", "a.Kgcode",
         "é.gcode", "a.gcodE"]


def build_shared(d, gcodes):
    os.mkdir(d)
    s = os.path.join(d, "storage")
    os.makedirs(os.path.join(s, "models", "sub"))
    os.mkdir(os.path.join(s, "empty"))
    os.mkdir(os.path.join(s, ".hidden"))
    write(os.path.join(s, "models", "a.gcode"), b"; temperature = 250\n")
    write(os.path.join(s, "models", "b.txt"), b"text")
    write(os.path.join(s, "models", "fw.hex"), b"fw")
    write(os.path.join(s, "models", ".a.gcode.cache"), b"")
    os.symlink(os.path.join(s, "models"), os.path.join(s, "link_in"))
    os.symlink("models/a.gcode", os.path.join(s, "link_file"))
    os.symlink(os.path.join(d, "outside"), os.path.join(s, "link_out"))
    os.symlink("../outside/x.gcode", os.path.join(s, "link_out_file"))
    os.symlink(os.path.join(s, "nowhere"), os.path.join(s, "broken"))
    os.symlink("../storage/models", os.path.join(s, "link_back"))
    os.mkdir(os.path.join(d, "outside"))
    write(os.path.join(d, "outside", "x.gcode"), b"G28\n")
    os.symlink("storage", os.path.join(d, "storage_link"))
    over = os.path.join(d, "over")
    limit = os.path.join(d, "limit")
    os.mkdir(over)
    os.mkdir(limit)
    for i in range(fileinfo.MAX_FOLDER_ITEMS + 1):
        write(os.path.join(over, f"{i}.gcode"), b"")
    for i in range(fileinfo.MAX_FOLDER_ITEMS):
        write(os.path.join(limit, f"{i}.gcode"), b"")
    write(os.path.join(limit, ".hidden.gcode"), b"")
    fix_times(d)


def build_copy(d, gcodes):
    os.makedirs(os.path.join(d, "models", "sub"))
    for n in sorted(os.listdir(gcodes)):
        if n.endswith(".gcode"):
            shutil.copy(os.path.join(gcodes, n), os.path.join(d, "models", n))
    write(os.path.join(d, "models", "a.gcode"), b"; temperature = 250\n")
    write(os.path.join(d, "models", "empty.gcode"), b"")
    write(os.path.join(d, "models", "binary.gcode"), bytes(range(256)) * 4)
    write(os.path.join(d, "models", "UPPER.GCO"), b"; temperature = 215\n; nozzle_diameter = 0.4\n")
    write(os.path.join(d, "models", "fw.hex"), b":00000001FF\n")
    write(os.path.join(d, "models", "b.txt"), b"text")
    write(os.path.join(d, "models", "ro.gcode"), b"; temperature = 200\n")
    write(os.path.join(d, "models", "locked.gcode"), b"; temperature = 200\n")
    write(os.path.join(d, "models", "sub", "c.g"), b"; layer_height = 0.15\n")
    fix_times(d)
    os.chmod(os.path.join(d, "models", "ro.gcode"), 0o444)
    os.chmod(os.path.join(d, "models", "locked.gcode"), 0)


def outcome(call):
    try:
        return ["ok", call()]
    except CommandRejected as err:
        return ["CommandRejected", err.reason]
    except Exception as err:  # pylint: disable=broad-except
        return [repr(err)]


def run(shared, copy):
    s = os.path.join(shared, "storage")
    steps = []

    def step(name, value):
        steps.append([name, value])

    step("get_type_by_name", [[fileinfo.get_type_by_name(n, d).value for d in (False, True)]
                              for n in NAMES])
    inside = [s, s + "/", os.path.join(shared, "storage_link"), os.path.join(shared, "outside")]
    entries = sorted(os.listdir(s)) + ["missing", "models/a.gcode", "models/../..",
                                       "link_in/sub", "link_back/../../outside"]
    step("is_inside", [[outcome(lambda p=p, st=st: fileinfo.is_inside(os.path.join(s, p), st))
                        for st in inside] for p in entries])

    def folder(virtual, storage, os_path):
        def call():
            return json.dumps(fileinfo.folder_info(virtual, storage, os_path, os.stat(os_path)))
        return outcome(call)

    for d in ["", "models", "models/sub", "empty", "link_in", "link_back"]:
        step(f"folder_info {d}", folder("/local/" + d, s, os.path.join(s, d)))
    step("folder_info, storage by its link",
         folder("/local", os.path.join(shared, "storage_link"), s))
    step("folder_info over", folder("/over", os.path.join(shared, "over"),
                                    os.path.join(shared, "over")))
    step("folder_info limit", folder("/limit", os.path.join(shared, "limit"),
                                     os.path.join(shared, "limit")))
    step("folder_info a file", folder("/local/models/a.gcode", s,
                                      os.path.join(s, "models", "a.gcode")))
    step("folder_info missing", outcome(
        lambda: json.dumps(fileinfo.folder_info("/local/missing", s, os.path.join(s, "missing"),
                                                os.stat(s)))))

    def file(virtual, os_path):
        def call():
            return json.dumps(fileinfo.file_info(virtual, os_path, os.stat(os_path)))
        return outcome(call)

    models = os.path.join(copy, "models")
    names = sorted(os.listdir(models)) + ["sub/c.g"]
    for n in names:
        if os.path.isdir(os.path.join(models, n)):
            continue
        step(f"file_info {n}", file("/local/models/" + n, os.path.join(models, n)))
    for n in names:  # again: from the caches written above
        if os.path.isdir(os.path.join(models, n)):
            continue
        step(f"file_info {n} again", file("/local/models/" + n, os.path.join(models, n)))
    step("file_info removed", outcome(lambda: json.dumps(fileinfo.file_info(
        "/local/models/gone.gcode", os.path.join(models, "gone.gcode"),
        os.stat(os.path.join(models, "b.txt"))))))
    step("caches", sorted(n for n in os.listdir(models) if n.startswith(".")))
    return {"steps": steps}


if __name__ == "__main__":
    mode = sys.argv[1]
    if mode == "build-shared":
        build_shared(sys.argv[2], sys.argv[3])
    elif mode == "build-copy":
        build_copy(sys.argv[2], sys.argv[3])
    elif mode == "run":
        print(json.dumps(run(sys.argv[2], sys.argv[3])))
