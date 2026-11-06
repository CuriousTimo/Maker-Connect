#!/bin/sh
# Builds a release into dist/<tag>/. The server (the cloud) and the agent are released apart,
# each with its own version: tags `server-v0.2.0-beta.1` and `agent-v0.2.0-beta.1`.
#
#   scripts/build-release.sh server v0.2.0-beta.1
#   scripts/build-release.sh agent v0.2.0-beta.1
#
# server: the cloud for every platform, as an archive that is ready to start:
#
#   maker-connect-cloud[.exe]  the cloud; printer profiles and PrusaSlicer's filaments are
#                              inside it
#   spoolmandb/                SpoolmanDB as of the build (MIT), for Add spools before the
#                              cloud has fetched it itself
#   README.md, LICENSE, SpoolmanDB-LICENSE
#
#   Its Downloads page offers the newest agent release from GitHub.
#   The archive holds no data: the database, its files, downloads and backups are made beside
#   the database when the cloud starts, so unpacking a new release over an old one keeps them.
#   A cloud that opens a database another build had backs it up first (store.rs).
#
# agent: maker-connect-agent-<os>-<arch>[.exe] for every platform.
#
# Both come with SHA256SUMS. Built on a Mac: macOS natively (Apple silicon and Intel),
# Windows and Linux with cargo-zigbuild (see build-downloads.sh). Only from a clean tree at
# the commit being tagged, so every build says the release it is.
set -e
cd "$(dirname "$0")/.."
part="${1:?give server or agent, then the version, such as v0.2.0-beta.1}"
version="${2:?give the version, such as v0.2.0-beta.1}"
case "$part" in
    server) crate=maker-connect-cloud ;;
    agent) crate=maker-connect-agent ;;
    *) echo "server or agent, not $part" >&2; exit 1 ;;
esac
tag="$part-$version"
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "Commit first: a release is built from a clean tree." >&2
    exit 1
fi
command -v cargo-zigbuild >/dev/null 2>&1 || { echo "cargo-zigbuild is needed" >&2; exit 1; }

out="dist/$tag"
rm -rf "$out"
mkdir -p "$out"

# The release each build says it is (`v0.2.0-beta.1`): the cloud writes it into the databases
# it opens, the agent tells the cloud.
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
        cp "$dir/maker-connect-cloud$exe" "$out/maker-connect-cloud-$os-$arch$exe.bin"
    fi
    echo "built $os-$arch"
done

if [ "$part" = agent ]; then
    (cd "$out" && shasum -a 256 maker-connect-* > SHA256SUMS)
    ls -la "$out"
    exit 0
fi

# SpoolmanDB, as the cloud fetches it.
mkdir -p "$out/spoolmandb"
for f in filaments.json materials.json; do
    curl -fsSL "https://donkie.github.io/SpoolmanDB/$f" -o "$out/spoolmandb/$f"
done
curl -fsSL https://raw.githubusercontent.com/Donkie/SpoolmanDB/main/LICENSE -o "$out/SpoolmanDB-LICENSE"

# The cloud's archives.
echo "$targets" | while read -r os arch target how; do
    exe=""; [ "$os" = windows ] && exe=".exe"
    name="maker-connect-cloud-$os-$arch"
    stage="$out/stage/$name"
    mkdir -p "$stage/spoolmandb"
    cp "$out/$name$exe.bin" "$stage/maker-connect-cloud$exe"
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
ls -la "$out"
