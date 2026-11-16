#!/usr/bin/env python3
"""MSE audit (docs/audits/go2rtc-mse.md): our fMP4 muxer against go2rtc's own (v1.9.15, 7c05066).

Records frames of a real camera through retina (crates/hub/examples/mse_record.rs, which also
writes our content type, init segment and fragments), runs go2rtc's own muxer on the same
frames (scripts/go2rtc-mse-check/main.go against reference/go2rtc), and compares record by
record: the content type, the init segment, every fragment.

    scripts/go2rtc-mse-audit-check.py rtsp://IP:8554/Streaming/Channels/101 admin admin 200 OUTDIR
"""
import pathlib
import struct
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parent.parent


def records(b):
    out, i = [], 0
    while i + 4 <= len(b):
        (n,) = struct.unpack(">I", b[i : i + 4])
        out.append(b[i + 4 : i + 4 + n])
        i += 4 + n
    return out


def main():
    url, user, password, count, out = sys.argv[1:6]
    out = pathlib.Path(out)
    subprocess.run(
        ["cargo", "run", "-q", "-p", "maker-connect-hub", "--features", "cameras", "--example",
         "mse_record", "--", url, user, password, count, str(out)],
        cwd=root, check=True)
    subprocess.run(["go", "run", ".", str(out.resolve())], cwd=root / "scripts/go2rtc-mse-check", check=True)
    ours = records((out / "ours.bin").read_bytes())
    theirs = records((out / "go2rtc.bin").read_bytes())
    names = ["content type", "init segment"] + [f"fragment {i + 1}" for i in range(len(ours) - 2)]
    identical, differ = 0, []
    for name, a, b in zip(names, ours, theirs):
        if a == b:
            identical += 1
        else:
            first = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
            differ.append(f"{name}: differs at byte {first} (ours {len(a)} B, go2rtc {len(b)} B)")
    if len(ours) != len(theirs):
        differ.append(f"record count: ours {len(ours)}, go2rtc {len(theirs)}")
    print(f"{identical}/{max(len(ours), len(theirs))} identical")
    for line in differ[:20]:
        print("  " + line)
    sys.exit(0 if not differ else 1)


main()
