#!/usr/bin/env bash
# The check before every commit (CLAUDE.md "Checks"): every step CI runs (.github/workflows/ci.yml),
# with the tests only of the crates changed since origin/main (committed or not) and of the crates
# that depend on them. The full gate with the audit checkers is scripts/check.sh (before a Pi build or
# a release).
#
#   scripts/quick-check.sh
#
# The hub's store tests need Postgres (scripts/dev-db.sh): started when the hub is among the crates to
# test, and stopped afterwards only if this run started it.
set -uo pipefail

cd "$(dirname "$0")/.."
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
export SQLX_OFFLINE=true

failed=()
step() { # name, command...
    local name="$1"; shift
    echo "== $name"
    if "$@"; then echo "   ok"; else echo "   FAILED: $name"; failed+=("$name"); fi
}
quiet() { "$@" >/tmp/maker-connect-quick.$$ 2>&1; local e=$?; [ $e -ne 0 ] && tail -20 /tmp/maker-connect-quick.$$; rm -f /tmp/maker-connect-quick.$$; return $e; }

# The crates changed: their folder under crates/, from the diff against origin/main and untracked files.
changed_dirs=$( { git diff --name-only origin/main; git ls-files --others --exclude-standard; } \
    | grep -E '^crates/[^/]+/' | cut -d/ -f2 | sort -u)
# Folder -> package name, and every workspace package that depends on one of them.
packages=()
for d in $changed_dirs; do
    [ -f "crates/$d/Cargo.toml" ] || continue
    name=$(sed -n 's/^name *= *"\(.*\)"/\1/p' "crates/$d/Cargo.toml" | head -1)
    packages+=("$name")
    while read -r dep; do packages+=("$dep"); done < <(
        cargo tree --workspace --locked -i "$name" -e normal,dev,build --prefix none --format '{p}' 2>/dev/null \
            | awk '{print $1}' | grep '^maker-connect' )
done
# (bash 3.2 on macOS: no mapfile)
unique=$(printf '%s\n' "${packages[@]+"${packages[@]}"}" | sort -u | grep .)
packages=()
for p in $unique; do packages+=("$p"); done
echo "crates to test: ${packages[*]+"${packages[*]}"}"

fmt_check() { local n; n=$(cargo fmt --all -- --check 2>/dev/null | grep -c '^Diff in'); [ "$n" -eq 0 ] || { echo "   $n places differ from cargo fmt (run: cargo fmt --all)"; return 1; }; }
step "fmt" fmt_check
step "clippy" quiet cargo clippy --workspace --all-targets --locked -- -D warnings
step "clippy (Windows)" quiet env CC_x86_64_pc_windows_gnu="$PWD/scripts/zigcc-windows.sh" AR_x86_64_pc_windows_gnu="$PWD/scripts/zig-ar.sh" \
    cargo clippy --workspace --all-targets --locked --target x86_64-pc-windows-gnu -- -D warnings
step "docs (rustdoc -D warnings)" quiet env RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --locked

hub=0
args=()
for p in ${packages[@]+"${packages[@]}"}; do
    [ "$p" = maker-connect-hub ] && hub=1 || args+=(-p "$p")
done
[ ${#args[@]} -gt 0 ] && step "tests ($((${#args[@]} / 2)) crates)" quiet cargo test --locked "${args[@]}"
if [ $hub -eq 1 ]; then
    # As CI's job `store`: with Postgres and the cameras (the .67 build), which also checks tests/routes.txt.
    DEV_DB_WAS_UP=$(docker ps -q --filter "name=^mc-dev-db$" 2>/dev/null)
    if DEV_DB=$(scripts/dev-db.sh up | grep '^DATABASE_URL='); then
        export "${DEV_DB?}"
        export "$(scripts/dev-db.sh command)"
        step "hub tests (Postgres, cameras)" quiet cargo test -p maker-connect-hub --locked --features pg-tests,cameras
        step ".sqlx cache = queries" quiet scripts/dev-db.sh prepare --check
    else
        step "hub tests (no dev database)" false
    fi
    [ -z "$DEV_DB_WAS_UP" ] && scripts/dev-db.sh down
fi

if [ ${#failed[@]} -eq 0 ]; then
    echo "== quick check: all passed"
else
    echo "== quick check: FAILED: ${failed[*]}"
    exit 1
fi
