#!/bin/sh
# Makes the machine profiles and the filament catalogue again from their sources, and the
# wiki's list of machines.
#   scripts/import-profiles.sh [wiki folder]      default: ../Maker-Connect.wiki
# Marlin's example configurations and Klipper's printer configurations are cloned into
# reference/ (not part of this repository) the first time, and updated after. Profiles made by hand are left alone.
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
# Klipper's printer configurations (GPL-3.0): only its config folder.
klipper=reference/klipper
if [ -d "$klipper/.git" ]; then
    git -C "$klipper" pull -q --depth 1
else
    git clone -q --depth 1 --filter=blob:none --sparse https://github.com/Klipper3d/klipper.git "$klipper"
    git -C "$klipper" sparse-checkout set config
fi
cargo run -q -p maker-connect-import -- klipper "$klipper" profiles
# PrusaSlicer's vendor profiles (AGPL-3.0): the newest bundle its index names.
bundles=https://raw.githubusercontent.com/prusa3d/PrusaSlicer-settings-prusa-fff/main/PrusaResearch
version=$(curl -fsSL "$bundles/index.idx" | grep -m1 -E '^[0-9]+\.[0-9]+\.[0-9]+ ' | cut -d' ' -f1)
mkdir -p reference/prusaslicer
curl -fsSL -o "reference/prusaslicer/prusa-fff-$version.ini" "$bundles/$version.ini"
cargo run -q -p maker-connect-import -- prusaslicer "reference/prusaslicer/prusa-fff-$version.ini" filaments/prusaslicer-prusa-fff.toml
if [ -d "$wiki" ]; then
    cargo run -q -p maker-connect-import -- list profiles "$wiki/Supported-Machines.md"
fi
