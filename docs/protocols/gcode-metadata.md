# What a G-code file says about itself

How the hub (`crates/hub/src/gcode.rs`) reads a file's details and picture, set against
Prusa's own reader, gcode-metadata (`reference/gcode-metadata`, 2026-09-30, used by
PrusaLink and its Connect SDK). Read on 2026-11-08; nothing copied.

| | gcode-metadata | The hub |
|---|---|---|
| Where | the first 800 KB and the last 256 KB (PrusaSlicer 3.0's JSON config pushes `max_layer_z` ~90 KB from the end) | the first 2 MB and the last 256 KB |
| `.bgcode` | no (the Buddy firmware reads those itself) | yes: printer and print metadata blocks, thumbnails |
| Thumbnails | PNG, QOI, JPG (`; thumbnail[_QOI\|_JPG] begin WxH size`) | the same, and in `.bgcode` (format 0 PNG, 1 JPG, 2 QOI); the largest kept, as PNG |
| Time | `estimated printing time (normal mode)` | the same; Cura's `;TIME:` |
| Filament | `filament used [g]`, `[mm]`, `[cm3]`, `filament cost`: per tool, summed | `[g]` and `[mm]` summed; Cura's `;Filament used:` |
| Kind | `filament_type`, the same for every tool or nothing | every kind once (`PLA, PETG`) |
| Printer, layer, nozzle, height | `printer_model`, `layer_height`, `nozzle_diameter`, `max_layer_z` | the same; Cura's `;Layer height:`, `;MAXZ:` |
| Temperatures | `temperature`, `bed_temperature` | the same (the first tool's) |
| Colour | `extruder_colour` | the first tool's `#RRGGBB` |
| Nozzle needs | `nozzle_high_flow`, `filament_abrasive` | the same |
| Settings | `fill_density`, `support_material` (0/1, or PrusaSlicer 3.0's mode), `brim_width`, `ironing` | `fill_density`, supports (both forms) |
| Objects | `objects_info` (JSON: names and outlines, for cancel-object) | the names |
| Progress marks | whether `M73` gives normal/quiet percent, time left, change in | – (the printer reads them) |
| The file's name | `…_0.6n_0.32mm_PETG_MK4_8h55m`: name, nozzle, layer, material, printer, time | the same where the contents say nothing; also `0_4n_0_2mm` (dots turned into underscores, as on the MINI's stick: gcode-metadata would read a 4 mm nozzle there) |
| SL1 (`.sl1`, zip) | yes | – (no resin printers yet) |

Files kept before this are read again once at start (their new keys, a QOI or JPG
thumbnail they did not have).
