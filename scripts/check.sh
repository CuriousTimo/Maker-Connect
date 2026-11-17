#!/usr/bin/env bash
# The local quality gate, run before every commit: the CI checks plus the project's own checkers.
# Exit status 0 only when everything that must hold, holds. Checks with known open rows are reported
# with their numbers, never hidden (CLAUDE.md: "checked by eye" counts as not checked).
#
#   scripts/check.sh           the gate
#   scripts/check.sh --proof   also the Marlin port's proof against real OctoPrint (slow; docs/audits/octoprint-marlin.md)
#
# Not run here, by design: scripts/go2rtc-mse-audit-check.py needs a live RTSP camera (its usage line
# says how), and the hardware runs of every port are the owner's.
set -uo pipefail

cd "$(dirname "$0")/.."
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

PY313="${PY313:-$HOME/.local/share/uv/python/cpython-3.13.12-macos-aarch64-none/bin/python3.13}"
OCTOPRINT_PY="reference/.venv-octoprint/bin/python"
OCTOPRINT_313_PY="reference/.venv-octoprint-313/bin/python"

failed=()
step() { # name, command...
    local name="$1"; shift
    echo "== $name"
    if "$@"; then echo "   ok"; else echo "   FAILED: $name"; failed+=("$name"); fi
}
quiet() { "$@" >/tmp/maker-connect-check.$$ 2>&1; local e=$?; [ $e -ne 0 ] && tail -20 /tmp/maker-connect-check.$$; rm -f /tmp/maker-connect-check.$$; return $e; }

# The CI checks (.github/workflows/ci.yml).
fmt_check() { local n; n=$(cargo fmt --all -- --check 2>/dev/null | grep -c '^Diff in'); [ "$n" -eq 0 ] || { echo "   $n places differ from cargo fmt (run: cargo fmt --all)"; return 1; }; }
step "fmt" fmt_check
step "clippy" quiet cargo clippy --workspace --all-targets --locked -- -D warnings
step "tests" quiet cargo test --workspace --locked
step "docs (rustdoc -D warnings)" quiet env RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --locked
step "docs links" python3 -I scripts/docs-links-check.py

# Fixtures made by real CPython 3.13 / OctoPrint's own code / real Spoolman: regenerated, and they
# must not change in this run (compared before and after, so an uncommitted fixture is judged too).
FIXTURES="crates/engine/src/ports/octoprint/py_format_expected.json crates/engine/src/ports/octoprint/py_nonprintable.txt
          crates/engine/src/ports/octoprint/py_percent_expected.json crates/engine/src/ports/octoprint/py_str_attrs.txt
          crates/engine/src/ports/octoprint/scripts_expected.json crates/hub/src/spoolman/audit-fixture.json"
# shellcheck disable=SC2086
BEFORE=$(shasum $FIXTURES)

# Audits that must hold: every row the checker can prove, proven.
for c in buddy-audit-check klipper-audit-check labelle-port-check dymo-labelwriter-450-check \
         spoolman-audit-check usbscale-audit-check; do
    step "$c" quiet python3 -I "scripts/$c.py"
done
step "laser profiles = LaserFoundry's presets" quiet python3 -I scripts/laserfoundry-presets.py check

step "octoprint scripts (jinja2)" quiet "$OCTOPRINT_PY" scripts/octoprint-scripts-check.py
step "octoprint script context" quiet "$OCTOPRINT_PY" scripts/octoprint-script-context-check.py
step "octoprint file read (CPython 3.13)" quiet "$PY313" scripts/octoprint-file-read-check.py
step "octoprint str.format (CPython 3.13)" quiet "$PY313" scripts/octoprint-format-check.py
step "octoprint % (CPython 3.13)" quiet "$PY313" scripts/octoprint-percent-check.py
# shellcheck disable=SC2086
step "regenerated fixtures unchanged" test "$BEFORE" = "$(shasum $FIXTURES)"

if [ "${1:-}" = "--proof" ]; then
    step "Marlin port vs OctoPrint (proof)" "$OCTOPRINT_313_PY" scripts/octoprint-port-check.py compare
fi

# Known open, reported with their numbers (not failing the gate):
echo "== known open"
python3 -I scripts/marlin-audit-check.py 2>&1 | grep -E "named tests that do not check|not proven|OPEN" | sed 's/^/   marlin audit: /' | head -5
if [ -f crates/agent/src/lcd.rs ]; then
    python3 -I scripts/prusalink-audit-check.py 2>&1 | grep -E "not proven|OPEN" | sed 's/^/   prusalink audit: /' | head -5
else
    echo "   prusalink audit: checks the agent before the engine restart (2026-11-13); stale until the MK3 port (docs/plans/mk3-port-plan.md)"
fi

if [ ${#failed[@]} -eq 0 ]; then
    echo "== check: all passed"
else
    echo "== check: FAILED: ${failed[*]}"
    exit 1
fi
