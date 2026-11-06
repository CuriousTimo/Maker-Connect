# Example files

`samples.zst` holds the example files the hub offers on the Files page ("Add example
files"). It is compiled into the binary and unpacked only when someone adds them.

What is in it: [3DBenchy](https://www.3dbenchy.com/) (CC0, public domain since February 2025),
sliced with PrusaSlicer 2.9.6:

| File | Printer | Material | Time |
|---|---|---|---|
| `3dbenchy_0.2mm_PLA_MK3S_1h24m.gcode` | Original Prusa MK3S | PLA | 1 h 24 min |
| `3dbenchy_0.2mm_PETG_MK3S_1h25m.gcode` | Original Prusa MK3S | PETG | 1 h 25 min |
| `3dbenchy_0.4n_0.2mm_PETG_MINIIS_1h3m.bgcode` | Original Prusa MINI+ (binary G-code) | PETG | 1 h 3 min |
| `3dbenchy_1h53m_0.20mm_215C_PLA_ENDER3.gcode` | Creality Ender 3 | PLA | 1 h 53 min |
| `3dbenchy_1h55m_0.20mm_215C_PLA_CR10.gcode` | Creality CR-10 | PLA | 1 h 55 min |

The files are 18 MB together and 3 MB in the pack. They go in as one zstd stream, so files
that are alike share what they have in common: the same boat for two printers is mostly the
same lines. Binary G-code is compressed already, so it barely shrinks.

To change the files or add one:

```
scripts/pack-samples.sh --unpack /tmp/samples   # the files back out, byte for byte
# change, add or remove files in /tmp/samples
scripts/pack-samples.sh /tmp/samples            # pack them again
```

Packing needs `zstd`. Building the hub does not: it unpacks with
[ruzstd](https://crates.io/crates/ruzstd), in Rust.
