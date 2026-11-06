#!/bin/sh
# Packs example files (G-code, binary G-code) into crates/hub/samples/samples.zst: one
# zstd stream, so files alike (the same model for two printers) share what they have in
# common. The hub unpacks it when someone asks for the example files (crate::samples).
#
#   scripts/pack-samples.sh ../temp/GCODE          pack a folder
#   scripts/pack-samples.sh --unpack some/folder   the files back out, byte for byte: to
#                                                  change one or add one, then pack again
#
# Format before compression: "MCSAMPLES1\n", then a line "<name>\t<bytes>\n" per file, an
# empty line, then the files one after another. Needs zstd.
set -e
root=$(cd "$(dirname "$0")/.." && pwd)
if [ "$1" = "--unpack" ]; then
  to=${2:?a folder to unpack into}
  mkdir -p "$to"
  python3 -I - "$root/crates/hub/samples/samples.zst" "$to" <<'PY'
import subprocess, sys, os
raw = subprocess.run(["zstd", "-dc", "--long=25", sys.argv[1]], capture_output=True, check=True).stdout
head, _, body = raw.partition(b"\n\n")
lines = head.decode().split("\n")
assert lines[0] == "MCSAMPLES1"
at = 0
for line in lines[1:]:
    name, size = line.split("\t")
    size = int(size)
    with open(os.path.join(sys.argv[2], name), "wb") as f:
        f.write(body[at:at + size])
    at += size
    print(name, size)
PY
  exit 0
fi
from=$(cd "${1:?a folder with the example files}" && pwd)
raw=$(mktemp)
# Text files first, sorted (alike next to each other), the binary ones last.
files=$(cd "$from" && { ls *.gcode *.gco 2>/dev/null | sort; ls *.bgcode 2>/dev/null | sort; })
{
  printf 'MCSAMPLES1\n'
  for f in $files; do printf '%s\t%s\n' "$f" "$(wc -c < "$from/$f" | tr -d ' ')"; done
  printf '\n'
} > "$raw"
for f in $files; do cat "$from/$f" >> "$raw"; done
mkdir -p "$root/crates/hub/samples"
# A window over everything (--long=25: 32 MB), so the second file finds the first.
zstd -q --ultra -22 --long=25 -f -o "$root/crates/hub/samples/samples.zst" "$raw"
rm -f "$raw"
chmod 644 "$root/crates/hub/samples/samples.zst"
ls -l "$root/crates/hub/samples/samples.zst"
