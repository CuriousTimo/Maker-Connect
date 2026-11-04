#!/bin/sh
# Builds the agent for the platforms this computer can build for, into the cloud's downloads
# folder (beside its database), named so /downloads gives each a card.
#   scripts/build-downloads.sh [folder]      default: maker-connect-downloads
# Windows (x86_64) is cross-built with cargo-zigbuild when it is installed
# (`cargo install cargo-zigbuild`, and Zig: `pip install ziglang`). Linux builds come from CI.
set -e
cd "$(dirname "$0")/.."
out="${1:-maker-connect-downloads}"
mkdir -p "$out"

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
fi
