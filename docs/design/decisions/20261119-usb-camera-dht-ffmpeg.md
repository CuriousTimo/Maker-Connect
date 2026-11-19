# USB camera frames: Huffman tables as FFmpeg's mjpeg2jpeg

Date: 2026-11-19  
Status: accepted  
Decided by: owner (2026-11-19, via the coordinator)

## Context

A UVC camera's MJPEG frame has no DHT; Safari and ImageIO show nothing (C270). PrusaLink's fix is an unmerged PR (prusa3d/Prusa-Link#1062), and PrusaLink is not used for cameras.

## Decision

The agent adds the tables as FFmpeg's `libavcodec/bsf/mjpeg2jpeg.c` does, with `jpegtabs.h` (T.81 annex K.3), FFmpeg master `f1d4dd2b`, LGPL-2.1-or-later (`crates/agent/src/camera.rs:1-9`, `89a8c4e`); capture stays nokhwa.

## Where it shows

[cameras.md](../cameras.md); #5.
