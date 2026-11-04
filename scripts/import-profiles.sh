#!/bin/sh
# Makes the machine profiles again from their sources, and the wiki's list of them.
#   scripts/import-profiles.sh [wiki folder]      default: ../Maker-Connect.wiki
# Marlin's example configurations are cloned into reference/ (not part of this repository)
# the first time, and updated after. Profiles made by hand are left alone.
set -e
cd "$(dirname "$0")/.."
wiki="${1:-../Maker-Connect.wiki}"
repo=reference/marlin-configurations
if [ -d "$repo/.git" ]; then
    git -C "$repo" fetch -q --depth 1 origin import-2.1.x && git -C "$repo" reset -q --hard FETCH_HEAD
else
    git clone -q --depth 1 --branch import-2.1.x https://github.com/MarlinFirmware/Configurations.git "$repo"
fi
cargo run -q -p maker-connect-import -- marlin "$repo" profiles
if [ -d "$wiki" ]; then
    cargo run -q -p maker-connect-import -- list profiles "$wiki/Supported-Machines.md"
fi
