#!/bin/sh
# HS13: builds our LaserFoundry fork's browser version for the hub: at the commit in scripts/laserfoundry-web.pin,
# without Web Serial (cargo feature machine-link off), runs wasm-bindgen, checks the bundle holds no Web Serial,
# gzips it, and writes it where the hub embeds it (crates/hub/laser-design/, generated, never committed):
#
#   scripts/laserfoundry-web.sh                 # the pinned commit
#   scripts/laserfoundry-web.sh --working-tree  # the fork's working tree as it is now (to try a change before it is committed)
#   cargo build --release -p maker-connect-hub --features laser-design
#
# Needs: rustup with the wasm32-unknown-unknown target, wasm-bindgen-cli at the version in the fork's Cargo.lock, git, gzip.
# The fork: $LASERFOUNDRY_FORK, default ../laserfoundry-fork (beside this repository).
set -eu
cd "$(dirname "$0")/.."
root="$(pwd)"
fork="${LASERFOUNDRY_FORK:-$root/../laserfoundry-fork}"
[ -e "$fork/.git" ] || { echo "The fork is not at $fork (set LASERFOUNDRY_FORK)." >&2; exit 1; }
out="$root/crates/hub/laser-design"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

if [ "${1:-}" = "--working-tree" ]; then
    src="$fork"
    pin="working tree of $(git -C "$fork" rev-parse --short HEAD)"
else
    pin="$(grep -v '^#' scripts/laserfoundry-web.pin | head -n 1 | tr -d ' ')"
    case "$pin" in
        ""|UNPINNED*) echo "scripts/laserfoundry-web.pin has no commit yet (use --working-tree to try the fork as it is)." >&2; exit 1 ;;
    esac
    git -C "$fork" cat-file -e "$pin^{commit}" 2>/dev/null || { echo "The fork has no commit $pin." >&2; exit 1; }
    src="$work/src"
    mkdir "$src"
    git -C "$fork" archive "$pin" | tar -x -C "$src"
fi

# The one wasm-bindgen the bundle's glue needs.
want="$(grep -A1 'name = "wasm-bindgen"$' "$src/Cargo.lock" | grep version | head -n 1 | cut -d'"' -f2)"
have="$(wasm-bindgen --version | cut -d' ' -f2)"
[ "$want" = "$have" ] || { echo "wasm-bindgen $want is needed (Cargo.lock of the fork), found $have." >&2; exit 1; }

target="${CARGO_TARGET_DIR:-$fork/target}"
(cd "$src" && CARGO_TARGET_DIR="$target" cargo build --release --target wasm32-unknown-unknown -p laserfoundry-web --no-default-features)
wasm-bindgen --target web --no-typescript --out-dir "$work/pkg" "$target/wasm32-unknown-unknown/release/laserfoundry_web.wasm"

# The page: the fork's index.html with our title (the hub's copy is not LaserFoundry's own product) and a policy that lets
# it talk to its own origin only. The inline start-up script is untouched, so its hash in the policy still holds.
sed -E \
    -e "s|<title>LaserFoundry</title>|<title>Laser design (LaserFoundry fork)</title>|" \
    -e "s|connect-src [^;]*;|connect-src 'self';|" \
    "$src/apps/laserfoundry-web/index.html" > "$work/index.html"
grep -q "connect-src 'self';" "$work/index.html" || { echo "the page's policy was not changed" >&2; exit 1; }
grep -q "__CLOUD_ORIGIN__\|makerfoundry" "$work/index.html" && { echo "the page still names another service" >&2; exit 1; }

# No Web Serial anywhere in what the browser gets.
for f in "$work/pkg/laserfoundry_web.js" "$work/pkg/laserfoundry_web_bg.wasm"; do
    if grep -a -q -e 'requestPort' -e 'navigator\.serial' -e 'getPorts' "$f"; then
        echo "$f holds Web Serial (requestPort / navigator.serial / getPorts)" >&2
        exit 1
    fi
done

rm -rf "$out"
mkdir -p "$out/pkg"
gzip -9 -n -c "$work/index.html" > "$out/index.html.gz"
for f in laserfoundry_web.js laserfoundry_web_bg.wasm; do
    gzip -9 -n -c "$work/pkg/$f" > "$out/pkg/$f.gz"
done
echo "$pin" > "$out/PINNED"
echo "built from $pin, no Web Serial; packed into $out:"
ls -l "$out/index.html.gz" "$out/pkg/"*.gz | awk '{print $5, $9}'
