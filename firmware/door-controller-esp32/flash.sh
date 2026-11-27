#!/bin/sh
# Flash a KC868-A6 over USB and KEEP its settings (Wi-Fi, hub, token in NVS): ESP-IDF's own bootloader
# (with the rollback of updates over the hub), the partition table with two app slots, the firmware
# into ota_0, and otadata erased so ota_0 boots. After this, updates go over the hub (part 7).
#
# Usage: ./flash.sh [serial port]     (build first: cargo build --release)
set -eu
cd "$(dirname "$0")"
PORT=${1:+--port "$1"}
BOOTLOADER=$(ls target/xtensa-esp32-espidf/release/build/esp-idf-sys-*/out/build/bootloader/bootloader.bin | head -1)
espflash flash $PORT --chip esp32 --bootloader "$BOOTLOADER" --partition-table partitions.csv \
  --erase-parts otadata target/xtensa-esp32-espidf/release/door-controller-esp32
