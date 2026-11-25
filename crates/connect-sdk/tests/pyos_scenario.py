"""What CPython gives for the os, os.path and shutil calls of files.py and fileinfo.py, for
`tests/pyos_reference.rs` to compare with ours (`src/pyos.rs`).

    python -I pyos_scenario.py pure            the string functions on generated paths
    python -I pyos_scenario.py build ROOT      makes the tree of the fs steps in ROOT
    python -I pyos_scenario.py fs ROOT         the fs steps on that tree (cwd: ROOT/r)
    python -I pyos_scenario.py build-rmtree D  makes the rmtree cases in D
    python -I pyos_scenario.py rmtree D        removes them, case by case

Each prints JSON. Paths are passed as `str` (as the SDK does) and shown as their
`os.fsencode` bytes in hex; a path under ROOT or D is shown relative to it ("<base>/...").
An exception is shown as [repr(err), its filename]: the repr is what command.py sends.
"""

import itertools
import json
import os
import shutil
import stat
import sys

# On Windows, pyos's model of CPython there (pyos_windows.py), before anything imports os.path.
if os.name == "nt":
    exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "pyos_windows.py")).read())

ALPHABET = "/.a"


def strings(max_len):
    for n in range(max_len + 1):
        for t in itertools.product(ALPHABET, repeat=n):
            yield "".join(t)


def h(s):
    return os.fsencode(s).hex()


BASE = None


def shown_path(p):
    if isinstance(p, int) or p is None:
        return p
    b = os.fsencode(p)
    if BASE is not None and b.startswith(BASE):
        b = b"<base>" + b[len(BASE):]
    return b.hex()


def outcome(call, ok=h):
    try:
        return ["ok", ok(call())]
    except OSError as err:
        return [repr(err), shown_path(err.filename)]
    except ValueError as err:
        return [repr(err), None]


def pure():
    s6 = list(strings(6))
    s3 = list(strings(3))
    s2 = list(strings(2))
    s4 = list(strings(4))
    return {
        "version": sys.version.split()[0],
        "normpath": [h(os.path.normpath(p)) for p in s6],
        "split": [[h(a), h(b)] for a, b in map(os.path.split, s6)],
        "join2": [h(os.path.join(a, b)) for a in s3 for b in s3],
        "join3": [h(os.path.join(a, b, c)) for a in s2 for b in s2 for c in s2],
        "join1": [h(os.path.join(a)) for a in s3],
        "commonpath1": [outcome(lambda a=a: os.path.commonpath([a])) for a in s4],
        "commonpath2": [outcome(lambda a=a, b=b: os.path.commonpath([a, b]))
                        for a in s4 for b in s4],
        "commonpath3": [outcome(lambda a=a, b=b, c=c: os.path.commonpath([a, b, c]))
                        for a in s2 for b in s2 for c in s2],
        "commonpath0": outcome(lambda: os.path.commonpath([])),
        # cwd: /.a-free names would not show what abspath adds; the Rust side sets the same cwd
        "abspath": [outcome(lambda a=a: os.path.abspath(a)) for a in s4],
        "relpath": [outcome(lambda a=a, b=b: os.path.relpath(a, b)) for a in s4 for b in s4],
        "relpath_nostart": [outcome(lambda a=a: os.path.relpath(a)) for a in s4],
        "nul": [outcome(lambda: os.path.normpath("a/\0/../b")),
                outcome(lambda: os.stat("a\0b"), ok=lambda st: None),
                [os.path.exists("a\0b"), os.path.lexists("a\0b"), os.path.isdir("a\0b"),
                 os.path.islink("a\0b")],
                outcome(lambda: os.path.realpath("/tmp/a\0b"))],
    }


def write(p, data=b"x"):
    with open(p, "wb") as f:
        f.write(data)


# The entries of ROOT/w and ROOT/r: the walk tree, and the links realpath resolves.
def build(root):
    w = os.path.join(root, "w")
    r = os.path.join(root, "r")
    os.makedirs(os.path.join(w, "sub", "deep"))
    os.makedirs(os.path.join(w, "sub", ".hid"))
    os.mkdir(os.path.join(w, "empty"))
    os.mkdir(os.path.join(w, "noperm"))
    os.mkdir(r)
    write(os.path.join(w, "a.gcode"), b"G28\n")
    write(os.path.join(w, ".hidden"))
    write(os.path.join(w, "sub", "b.gcode"), b"G1 X1\n")
    write(os.path.join(w, "sub", "deep", "c.txt"), b"")
    write(os.path.join(w, "sub", ".hid", "d.gcode"))
    write(os.path.join(w, "noperm", "x"))
    write(os.path.join(w, "ro.gcode"))
    os.symlink("sub", os.path.join(w, "link_sub"))
    os.symlink("missing", os.path.join(w, "dangling"))
    os.symlink("a.gcode", os.path.join(w, "file_link"))
    os.symlink(os.path.join(w, "sub", "deep"), os.path.join(r, "link_abs"))
    os.symlink("loop2", os.path.join(r, "loop1"))
    os.symlink("loop1", os.path.join(r, "loop2"))
    os.symlink("self", os.path.join(r, "self"))
    os.symlink("../w/link_sub/deep", os.path.join(r, "chain"))
    os.symlink("..", os.path.join(r, "up"))
    os.symlink("../w", os.path.join(r, "rel"))
    os.symlink("rel/link_sub/../a.gcode", os.path.join(r, "dotdot"))
    os.symlink("/", os.path.join(r, "root"))
    os.symlink("a.gcode/x", os.path.join(w, "through_file"))
    os.chmod(os.path.join(w, "ro.gcode"), 0o444)
    os.chmod(os.path.join(w, "noperm"), 0)


def st_shown(st):
    """What the SDK reads of a stat (`st_size`, `st_mtime`) and the file's type that os.path
    and DirEntry test (a directory, a symlink, a regular file, else 0)."""
    kind = stat.S_IFMT(st.st_mode)
    kind = kind if kind in (stat.S_IFDIR, stat.S_IFLNK, stat.S_IFREG) else 0
    return [kind, st.st_size, int(st.st_mtime)]


def space(s):
    """The free and total space, in bytes, as `shutil.disk_usage` counts them (`f_frsize`
    blocks). The SDK multiplies by `f_bsize` (`files.py:301-303`): on Linux the same, on macOS
    `f_iosize` (the audit's "Not covered")."""
    return [s.f_bavail * s.f_frsize, s.f_blocks * s.f_frsize]


def entry_shown(e):
    def call(f):
        try:
            return ["ok", f()]
        except OSError as err:
            return [repr(err), shown_path(err.filename)]
    return [h(e.name), shown_path(e.path), call(e.is_dir), call(lambda: e.is_dir(follow_symlinks=False)),
            call(e.is_symlink), call(lambda: st_shown(e.stat())),
            call(lambda: st_shown(e.stat(follow_symlinks=False)))]


def walk_shown(top, topdown, followlinks, with_onerror):
    errors = []
    onerror = (lambda err: errors.append([repr(err), shown_path(err.filename)])) if with_onerror else None
    out = [[shown_path(d), [h(n) for n in dirs], [h(n) for n in files]]
           for d, dirs, files in os.walk(top, topdown=topdown, onerror=onerror,
                                         followlinks=followlinks)]
    return [out, errors]


def fs(root):
    global BASE
    BASE = os.fsencode(root)
    w = os.path.join(root, "w")
    r = os.path.join(root, "r")
    names_w = sorted(os.listdir(w)) + ["missing", "sub/deep", "sub/.hid/d.gcode", "noperm/x"]
    names_r = sorted(os.listdir(r))
    suffixes = ["", "/", "/.", "/..", "/x", "/../sub", "/./c.txt", "//deep"]
    paths = [os.path.join(w, n) + s for n in names_w for s in suffixes]
    paths += [os.path.join(r, n) + s for n in names_r for s in suffixes]
    paths += [n + s for n in names_r for s in suffixes]  # relative to the cwd, ROOT/r
    paths += ["", ".", "..", "/", "//", "///", "/..", "../w/./sub", "up/up/r/self"]
    steps = []

    def step(name, value):
        steps.append([name, value])

    step("realpath", [outcome(lambda p=p: os.path.realpath(p), ok=shown_path) for p in paths])
    step("stat", [outcome(lambda p=p: st_shown(os.stat(p)), ok=lambda v: v) for p in paths])
    step("lstat", [outcome(lambda p=p: st_shown(os.lstat(p)), ok=lambda v: v) for p in paths])
    step("tests", [[os.path.exists(p), os.path.lexists(p), os.path.isdir(p), os.path.islink(p),
                    os.access(p, os.W_OK)] for p in paths])
    step("abspath", [outcome(lambda p=p: os.path.abspath(p), ok=shown_path) for p in paths])
    step("relpath", [outcome(lambda p=p: os.path.relpath(p, w), ok=shown_path) for p in paths])
    if hasattr(os, "statvfs"):
        step("statvfs /dev", outcome(lambda: os.statvfs("/dev"), ok=space))
        step("statvfs root", outcome(lambda: os.statvfs(root), ok=lambda s: space(s)[1:]))
        step("statvfs root, f_bsize is f_frsize",
             os.statvfs(root).f_bsize == os.statvfs(root).f_frsize)
        step("statvfs missing", outcome(lambda: os.statvfs(os.path.join(w, "missing")),
                                        ok=lambda s: None))
    else:
        # Windows: no os.statvfs; pyos's is shutil.disk_usage's sizes (audit, "Not covered")
        step("statvfs /dev", outcome(lambda: shutil.disk_usage("/dev"), ok=lambda u: [u.free, u.total]))
        step("statvfs root", outcome(lambda: shutil.disk_usage(root), ok=lambda u: [u.total]))
        step("statvfs root, f_bsize is f_frsize", None)
        step("statvfs missing", outcome(lambda: shutil.disk_usage(os.path.join(w, "missing")),
                                        ok=lambda s: None))
    step("scandir w", outcome(lambda: [entry_shown(e) for e in os.scandir(w)], ok=lambda v: v))
    step("scandir w/", outcome(lambda: [entry_shown(e) for e in os.scandir(w + "/")], ok=lambda v: v))
    step("scandir r", outcome(lambda: [entry_shown(e) for e in os.scandir(r)], ok=lambda v: v))
    for top in [w, w + "/", "../w", os.path.join(w, "sub"), os.path.join(w, "missing"),
                os.path.join(w, "a.gcode"), os.path.join(w, "noperm"), os.path.join(w, "link_sub")]:
        for topdown in [True, False]:
            for followlinks in [False, True]:
                for with_onerror in [False, True]:
                    step(f"walk {shown_path(top)} {topdown} {followlinks} {with_onerror}",
                         walk_shown(top, topdown, followlinks, with_onerror))
    for p in [os.path.join(w, "sub"), os.path.join(w, "missing"), os.path.join(w, "a.gcode"),
              os.path.join(w, "noperm", "x"), os.path.join(w, "link_sub")]:
        step(f"rmdir {shown_path(p)}", outcome(lambda p=p: os.rmdir(p), ok=lambda v: None))
    for p in [os.path.join(w, "sub"), os.path.join(w, "missing"), os.path.join(w, "noperm", "x"),
              os.path.join(w, "sub", "deep", "c.txt", "x")]:
        step(f"unlink {shown_path(p)}", outcome(lambda p=p: os.unlink(p), ok=lambda v: None))
    return {"version": sys.version.split()[0], "steps": steps}


RMTREE_CASES = ["tree", "link_to_dir", "missing", "file", "noperm_sub", "empty", "readonly"]


def build_rmtree(d):
    os.mkdir(d)
    t = os.path.join(d, "tree")
    os.makedirs(os.path.join(t, "a", "b", "c"))
    os.makedirs(os.path.join(t, "x"))
    write(os.path.join(t, "f1"))
    write(os.path.join(t, "a", "f2"))
    write(os.path.join(t, "a", "b", "c", "f3"))
    os.symlink(os.path.join(t, "x"), os.path.join(t, "a", "link_dir"))
    os.symlink("nowhere", os.path.join(t, "a", "dangling"))
    os.mkdir(os.path.join(d, "target"))
    write(os.path.join(d, "target", "keep"))
    os.symlink("target", os.path.join(d, "link_to_dir"))
    write(os.path.join(d, "file"))
    n = os.path.join(d, "noperm_sub")
    os.makedirs(os.path.join(n, "locked"))
    write(os.path.join(n, "locked", "inside"))
    write(os.path.join(n, "loose"))
    os.chmod(os.path.join(n, "locked"), 0)
    os.mkdir(os.path.join(d, "empty"))
    ro = os.path.join(d, "readonly")
    os.makedirs(os.path.join(ro, "inner"))
    write(os.path.join(ro, "inner", "f"))
    os.chmod(os.path.join(ro, "inner"), 0o555)


# Every path a case made, to show which are left after rmtree.
def made(d):
    out = []
    for top, dirs, files in os.walk(d):
        for n in dirs + files:
            out.append(os.path.join(top, n))
    return sorted(out)


def rmtree(d):
    # The branch without fd functions (`_rmtree_unsafe`), the one ours ports on every OS.
    shutil._use_fd_functions = False  # pylint: disable=protected-access
    global BASE
    BASE = os.fsencode(d)
    os.chmod(os.path.join(d, "noperm_sub", "locked"), 0o755)
    before = made(d)
    os.chmod(os.path.join(d, "noperm_sub", "locked"), 0)
    steps = []
    for case in RMTREE_CASES:
        steps.append([case, outcome(lambda case=case: shutil.rmtree(os.path.join(d, case)),
                                    ok=lambda v: None)])
    os.chmod(os.path.join(d, "noperm_sub", "locked"), 0o755)
    os.chmod(os.path.join(d, "readonly", "inner"), 0o755)
    steps.append(["left", [shown_path(p) for p in before if os.path.lexists(p)]])
    return {"version": sys.version.split()[0], "steps": steps}


if __name__ == "__main__":
    mode = sys.argv[1]
    if mode == "pure":
        print(json.dumps(pure()))
    elif mode == "build":
        build(sys.argv[2])
    elif mode == "fs":
        print(json.dumps(fs(sys.argv[2])))
    elif mode == "build-rmtree":
        build_rmtree(sys.argv[2])
    elif mode == "rmtree":
        print(json.dumps(rmtree(sys.argv[2])))
