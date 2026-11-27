#!/bin/sh
# Flash a KC868-A6 as an EMPTY door controller: the firmware, with NVS erased (no Wi-Fi, hub or
# token). The board then opens its setup hotspot `Maker-Door-XXXX`, password `makerdoor` (setup.rs).
#
# Usage: ./flash-empty.sh [serial port]     (build first: cargo build --release)
set -eu
cd "$(dirname "$0")"
PORT=${1:+--port "$1"}
BOOTLOADER=$(ls target/xtensa-esp32-espidf/release/build/esp-idf-sys-*/out/build/bootloader/bootloader.bin | head -1)
espflash flash $PORT --chip esp32 --bootloader "$BOOTLOADER" --partition-table partitions.csv \
  --erase-parts otadata target/xtensa-esp32-espidf/release/door-controller-esp32
espflash erase-region $PORT --chip esp32 0x9000 0x6000
