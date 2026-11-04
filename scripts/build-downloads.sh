#!/bin/sh
# Builds the agent for the platforms this computer can build for, into the cloud's downloads
# folder (beside its database), named so /downloads gives each a card.
#   scripts/build-downloads.sh [folder]      default: maker-connect-downloads
# Windows (x86_64) and Linux (ARM64: Orange Pi Zero3, Raspberry Pi 4/5; and x86_64) are
# cross-built with cargo-zigbuild when it is installed (`cargo install cargo-zigbuild`, and
# Zig: `pip install ziglang`); Linux for glibc 2.31 and later (Debian 11, Ubuntu 20.04,
# Armbian of those years on).
set -e
cd "$(dirname "$0")/.."
out="${1:-maker-connect-downloads}"
mkdir -p "$out"

# The build these are, as the agent knows itself (crates/agent/build.rs): the cloud compares
# it with what agents report, to say an update is there.
commit=$(git rev-parse --short=7 HEAD)
[ -n "$(git status --porcelain --untracked-files=no)" ] && commit="$commit-dirty"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
printf '%s\n%s\n%s\n' "$commit" "$(date -u '+%Y-%m-%d %H:%M UTC')" "$version" > "$out/.build"

os=$(uname -s); arch=$(uname -m)
case "$os" in Darwin) os=macos ;; Linux) os=linux ;; *) os="" ;; esac
case "$arch" in arm64) arch=aarch64 ;; esac
if [ -n "$os" ]; then
    cargo build --release -p maker-connect-agent
    cp target/release/maker-connect-agent "$out/maker-connect-agent-$os-$arch"
    echo "built $out/maker-connect-agent-$os-$arch"
fi

if command -v cargo-zigbuild >/dev/null 2>&1; then
    cargo zigbuild --release -p maker-connect-agent --target x86_64-pc-windows-gnu
    cp target/x86_64-pc-windows-gnu/release/maker-connect-agent.exe \
        "$out/maker-connect-agent-windows-x86_64.exe"
    echo "built $out/maker-connect-agent-windows-x86_64.exe"
    # 32-bit ARMv6: a Raspberry Pi Zero W or Pi 1 (on an MK3S's RPi port), glibc 2.28 on.
    cargo zigbuild --release -p maker-connect-agent --target arm-unknown-linux-gnueabihf.2.28
    cp target/arm-unknown-linux-gnueabihf/release/maker-connect-agent \
        "$out/maker-connect-agent-linux-armv6"
    echo "built $out/maker-connect-agent-linux-armv6"
    for arch in aarch64 x86_64; do
        cargo zigbuild --release -p maker-connect-agent --target "$arch-unknown-linux-gnu.2.31"
        cp "target/$arch-unknown-linux-gnu/release/maker-connect-agent" \
            "$out/maker-connect-agent-linux-$arch"
        echo "built $out/maker-connect-agent-linux-$arch"
    done
fi
