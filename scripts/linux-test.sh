#!/bin/sh
# The engine's tests on Linux, from the Mac: the PrusaLink port's comparisons run against PrusaLink on
# CPython 3.13, as CI and the Pi do (the local reference/.venv-prusalink is the Mac's Python), and the
# modules that exist on Linux only (serial.rs) run at all.
#
#   scripts/linux-test.sh setup        make the container's venv with PrusaLink's packages
#                                      (crates/engine/src/ports/prusalink/reference-requirements.txt)
#   scripts/linux-test.sh FILTER...    build the engine's lib tests for aarch64 Linux (cargo zigbuild)
#                                      and run them with FILTER... in a Debian container
#
# Needs Colima running (scripts/dev-db.sh up starts it). The container is small (2 CPUs, 2 GiB): run
# a filter such as ports::prusalink, not every test. Build output goes to
# ~/.cache/maker-connect-builds/linux-test.log.
set -eu

cd "$(dirname "$0")/.."
IMAGE="python:3.13-slim-bookworm"
VOLUME="mc-prusalink-venv"
TARGET="aarch64-unknown-linux-gnu.2.36"
LOG="$HOME/.cache/maker-connect-builds/linux-test.log"
mkdir -p "$(dirname "$LOG")"

if [ "${1:-}" = "setup" ]; then
  docker run --rm -v "$VOLUME:/v" \
    -v "$PWD/crates/engine/src/ports/prusalink/reference-requirements.txt:/req.txt:ro" "$IMAGE" \
    sh -c 'python -m venv /v/venv && /v/venv/bin/pip install -q -r /req.txt && /v/venv/bin/python -V'
  exit
fi

# rustup is Homebrew's: its stable toolchain, not ~/.cargo/bin
T="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin"
BIN=$(PATH="$T:$PATH" RUSTC="$T/rustc" SQLX_OFFLINE=true cargo zigbuild --tests --lib \
  -p maker-connect-engine --features devices --target "$TARGET" --message-format=json 2>>"$LOG" |
  python3 -c '
import json, sys
for line in sys.stdin:
    d = json.loads(line)
    if d.get("reason") == "compiler-message":
        print(d["message"]["rendered"], file=sys.stderr)
    if d.get("reason") == "compiler-artifact" and d.get("executable") \
            and d["target"]["name"] == "maker_connect_engine":
        print(d["executable"])
')
[ -n "$BIN" ] || { echo "build failed, see $LOG" >&2; exit 1; }
BIN=${BIN#"$PWD"/}

# the binary holds the Mac's paths: PrusaLink's and the SDK's checkouts as the container sees them
docker run --rm -v "$VOLUME:/v" -v "$PWD:/repo:ro" -w /repo \
  -e PRUSALINK_PYTHON=/v/venv/bin/python \
  -e PRUSALINK_SRC=/repo/reference/Prusa-Link \
  -e PRUSALINK_SDK_SRC=/repo/reference/Prusa-Connect-SDK-Printer-c0d7d233 \
  "$IMAGE" "/repo/$BIN" "$@"
