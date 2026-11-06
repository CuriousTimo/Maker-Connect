#!/bin/sh
# Builds a release into dist/<tag>/. The hub and the agent are released apart, each with its
# own version: tags `hub-v0.2.0-beta.2` and `agent-v0.2.0-beta.2`.
#
#   scripts/build-release.sh hub v0.2.0-beta.2
#   scripts/build-release.sh agent v0.2.0-beta.2
#
# hub: the hub for every platform, as an archive that is ready to start:
#
#   maker-connect-hub[.exe]  the hub; printer profiles and PrusaSlicer's filaments are
#                              inside it
#   spoolmandb/                SpoolmanDB as of the build (MIT), for Add spools before the
#                              hub has fetched it itself
#   README.md, LICENSE, SpoolmanDB-LICENSE
#
#   Its Downloads page offers the newest agent release from GitHub.
#   The archive holds no data: the database, its files, downloads and backups are made beside
#   the database when the hub starts, so unpacking a new release over an old one keeps them.
#   A hub that opens a database another build had backs it up first (store.rs).
#
# agent: maker-connect-agent-<os>-<arch>[.exe] for every platform.
#
# Both come with SHA256SUMS, signed with the release key (SHA256SUMS.sig, over the tag and
# the list; crates/update): the hub and the agent update themselves only to a signed release.
# The key: $MAKER_CONNECT_RELEASE_KEY, or ~/.config/maker-connect/release-key.pk8; never in
# the repository.
#
# Built on a Mac: macOS natively (Apple silicon and Intel),
# Windows and Linux with cargo-zigbuild (see build-downloads.sh). Only from a clean tree at
# the commit being tagged, so every build says the release it is.
set -e
cd "$(dirname "$0")/.."
part="${1:?give hub or agent, then the version, such as v0.2.0-beta.1}"
version="${2:?give the version, such as v0.2.0-beta.1}"
case "$part" in
    hub) crate=maker-connect-hub ;;
    agent) crate=maker-connect-agent ;;
    *) echo "hub or agent, not $part" >&2; exit 1 ;;
esac
tag="$part-$version"
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "Commit first: a release is built from a clean tree." >&2
    exit 1
fi
command -v cargo-zigbuild >/dev/null 2>&1 || { echo "cargo-zigbuild is needed" >&2; exit 1; }
key="${MAKER_CONNECT_RELEASE_KEY:-$HOME/.config/maker-connect/release-key.pk8}"
[ -f "$key" ] || { echo "The release key is needed: $key" >&2; exit 1; }

out="dist/$tag"
rm -rf "$out"
mkdir -p "$out"

# The release each build says it is (`v0.2.0-beta.1`): the hub writes it into the databases
# it opens, the agent tells the hub.
export MAKER_CONNECT_RELEASE="$version"

# os arch target(with a glibc for zig) how
targets="macos aarch64 aarch64-apple-darwin cargo
macos x86_64 x86_64-apple-darwin cargo
windows x86_64 x86_64-pc-windows-gnu zig
linux x86_64 x86_64-unknown-linux-gnu.2.31 zig
linux aarch64 aarch64-unknown-linux-gnu.2.31 zig
linux armv6 arm-unknown-linux-gnueabihf.2.28 zig"

echo "$targets" | while read -r os arch target how; do
    if [ "$how" = cargo ]; then
        cargo build --release --locked -p "$crate" --target "$target"
    else
        cargo zigbuild --release --locked -p "$crate" --target "$target"
    fi
    dir="target/${target%%.2.*}/release"
    exe=""; [ "$os" = windows ] && exe=".exe"
    if [ "$part" = agent ]; then
        cp "$dir/maker-connect-agent$exe" "$out/maker-connect-agent-$os-$arch$exe"
    else
        cp "$dir/maker-connect-hub$exe" "$out/maker-connect-hub-$os-$arch$exe.bin"
    fi
    echo "built $os-$arch"
done

if [ "$part" = agent ]; then
    (cd "$out" && shasum -a 256 maker-connect-* > SHA256SUMS)
    cargo run -q -p maker-connect-update --example release-key -- sign "$key" "$tag" "$out/SHA256SUMS"
    ls -la "$out"
    exit 0
fi

# SpoolmanDB, as the hub fetches it.
mkdir -p "$out/spoolmandb"
for f in filaments.json materials.json; do
    curl -fsSL "https://donkie.github.io/SpoolmanDB/$f" -o "$out/spoolmandb/$f"
done
curl -fsSL https://raw.githubusercontent.com/Donkie/SpoolmanDB/main/LICENSE -o "$out/SpoolmanDB-LICENSE"

# The hub's archives.
echo "$targets" | while read -r os arch target how; do
    exe=""; [ "$os" = windows ] && exe=".exe"
    name="maker-connect-hub-$os-$arch"
    stage="$out/stage/$name"
    mkdir -p "$stage/spoolmandb"
    cp "$out/$name$exe.bin" "$stage/maker-connect-hub$exe"
    cp "$out"/spoolmandb/*.json "$stage/spoolmandb/"
    cp README.md LICENSE "$out/SpoolmanDB-LICENSE" "$stage/"
    if [ "$os" = windows ]; then
        (cd "$out/stage" && zip -qr "../$name.zip" "$name")
    else
        tar -C "$out/stage" -czf "$out/$name.tar.gz" "$name"
    fi
    rm "$out/$name$exe.bin"
done
rm -rf "$out/stage" "$out/spoolmandb"
(cd "$out" && shasum -a 256 maker-connect-* > SHA256SUMS)
cargo run -q -p maker-connect-update --example release-key -- sign "$key" "$tag" "$out/SHA256SUMS"
ls -la "$out"
