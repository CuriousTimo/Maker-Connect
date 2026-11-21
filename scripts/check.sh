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
CONNECT_SDK_PY="reference/.venv-connect-sdk-313/bin/python"

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
# The .67 build has the network cameras (feature `cameras`): it builds and passes clippy too.
step "clippy (cameras)" quiet cargo clippy -p maker-connect-hub --all-targets --locked --features cameras -- -D warnings
# Windows (CI runs it there): clippy for x86_64-pc-windows-gnu, with zig as the C compiler.
step "clippy (Windows)" quiet env CC_x86_64_pc_windows_gnu="$PWD/scripts/zigcc-windows.sh" AR_x86_64_pc_windows_gnu="$PWD/scripts/zig-ar.sh" \
    cargo clippy --workspace --all-targets --locked --target x86_64-pc-windows-gnu -- -D warnings
# The store's tests need Postgres (scripts/dev-db.sh, on demand): started here when it is not running, and
# stopped afterwards only if this run started it. Builds use the checked-in `.sqlx/` cache (SQLX_OFFLINE).
export SQLX_OFFLINE=true
DEV_DB_WAS_UP=$(docker ps -q --filter "name=^mc-dev-db$" 2>/dev/null)
if DEV_DB=$(scripts/dev-db.sh up | grep '^DATABASE_URL='); then
    export "${DEV_DB?}"
    export "$(scripts/dev-db.sh command)"
    step "tests" quiet cargo test --workspace --locked --features maker-connect-hub/pg-tests
    # With the cameras too (the .67 build), which also checks the routes' snapshot (tests/routes.txt).
    step "hub tests (cameras)" quiet cargo test -p maker-connect-hub --lib --locked --features pg-tests,cameras
    step ".sqlx cache = queries" quiet scripts/dev-db.sh prepare --check
else
    step "tests (no dev database)" false
fi
[ -z "$DEV_DB_WAS_UP" ] && scripts/dev-db.sh down
step "docs (rustdoc -D warnings)" quiet env RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --locked
step "docs links" python3 -I scripts/docs-links-check.py
# docs/registries/*.md are generated from crates/device/src/registry.rs (hub-core-v2.md §2.10).
step "registry docs = registry tables" quiet cargo test -p maker-connect-device --test registry_docs --locked

# Fixtures made by real CPython 3.13 / OctoPrint's own code / real Spoolman: regenerated, and they
# must not change in this run (compared before and after, so an uncommitted fixture is judged too).
FIXTURES="crates/engine/src/ports/octoprint/py_format_expected.json crates/engine/src/ports/octoprint/py_nonprintable.txt
          crates/engine/src/ports/octoprint/py_percent_expected.json crates/engine/src/ports/octoprint/py_str_attrs.txt
          crates/engine/src/ports/octoprint/scripts_expected.json crates/hub/src/spoolman/audit-fixture.json"
# shellcheck disable=SC2086
BEFORE=$(shasum $FIXTURES)

# Audits that must hold: every row the checker can prove, proven.
for c in buddy-audit-check klipper-audit-check labelle-port-check dymo-labelwriter-450-check \
         spoolman-audit-check usbscale-audit-check sdk-extension-check; do
    step "$c" quiet python3 -I "scripts/$c.py"
done
step "laser profiles = LaserFoundry's presets" quiet python3 -I scripts/laserfoundry-presets.py check

step "octoprint scripts (jinja2)" quiet "$OCTOPRINT_PY" scripts/octoprint-scripts-check.py
step "octoprint script context" quiet "$OCTOPRINT_PY" scripts/octoprint-script-context-check.py
step "octoprint file read (CPython 3.13)" quiet "$PY313" scripts/octoprint-file-read-check.py
step "octoprint str.format (CPython 3.13)" quiet "$PY313" scripts/octoprint-format-check.py
step "octoprint % (CPython 3.13)" quiet "$PY313" scripts/octoprint-percent-check.py
# gcode-metadata's recorded cases (crates/gcode-metadata/tests/*.json) = what the reference records now.
step "gcode-metadata py cases (CPython 3.13)" quiet "$CONNECT_SDK_PY" -I scripts/gcode-metadata-py-cases.py --check
step "gcode-metadata cases (the reference)" quiet "$CONNECT_SDK_PY" -I scripts/gcode-metadata-cases.py --check
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
