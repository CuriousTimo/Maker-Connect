# Capability verbs

<!-- Generated from crates/device/src/registry.rs. Do not edit: run `MC_WRITE_REGISTRIES=1 cargo test -p maker-connect-device --test registry_docs`. -->

| Verb | Arguments (kind, unit) | Action | Person only | Kinds | |
|---|---|---|---|---|---|
| `mc.printer.gcode` | `text` (string) | `device.gcode` | no | marlin | OctoPrint's `printer/command` with `commands`. |
| `core.command.home` | `axes` (string) | `device.move` | no | marlin | OctoPrint's `printer/printhead` `home`. |
| `core.command.move` | `x` (number, mm), `y` (number, mm), `feedrate` (number, mm/min) | `device.move` | no | marlin | OctoPrint's `printer/printhead` `jog`, X and Y. |
| `core.command.move_z` | `distance` (number, mm), `feedrate` (number, mm/min) | `device.move` | no | marlin | OctoPrint's `printer/printhead` `jog`, Z. |
| `core.command.move_e` | `length` (number, mm), `feedrate` (number, mm/min) | `device.move` | no | marlin | OctoPrint's `printer/tool` `extrude`. |
| `core.command.set_nozzle_temperature` | `nozzle_temperature` (number, celsius), `tool` (string) | `device.heat` | no | marlin | OctoPrint's `printer/tool` `target`. |
| `core.command.set_bed_temperature` | `bed_temperature` (number, celsius) | `device.heat` | no | marlin | OctoPrint's `printer/bed` `target`; refused without a heated bed (printer.py:176-177). |
| `core.command.set_speed` | `speed` (integer, percent) | `job.control` | no | marlin | OctoPrint's `printer/printhead` `feedrate`. |
| `core.command.set_flow` | `flow` (integer, percent) | `job.control` | no | marlin | OctoPrint's `printer/tool` `flowrate`. |
| `core.command.disable_steppers` | - | `device.move` | no | marlin | OctoPrint's control panel "Motors off" button: `M18`. |
| `mc.printer.fan` | `on` (boolean) | `device.heat` | no | marlin | OctoPrint's control panel fan buttons. |
| `core.command.start_print` | `path` (string), `origin` (string) | `job.start` | no | marlin | OctoPrint selects the file and prints it; from the card with `printAfterSelect` (files.py:156-172, K7). |
| `core.command.pause_print` | - | `job.control` | no | marlin | OctoPrint's `job` `pause`. |
| `core.command.resume_print` | - | `job.control` | no | marlin | OctoPrint's `job` `pause` with `resume`. |
| `core.command.stop_print` | - | `job.control` | no | marlin | OctoPrint's `job` `cancel`. |
| `mc.printer.sd_list` | - | `device.view` | no | marlin | OctoPrint's `printer/sd` `refresh`; the card's file names come back as the command's output. |
| `core.command.delete_file` | `path` (string), `origin` (string) | `job.control` | no | marlin | OctoPrint's `files/sdcard/<path>` DELETE; refused for the file in use (files.py:1329-1330). |
| `mc.label.print` | `picture` (string), `tape_size_mm` (integer, mm), `page_size` (string) | `job.start` | no | labelmanager, labelwriter | Prints one label as the reference's own print run does. |
| `mc.laser.jog` | `x` (number, mm), `y` (number, mm), `feed` (number, mm/min) | `device.move` | no | grbl | LaserFoundry's jog: `$J=G91 G21 X.. Y.. F..`. |
| `mc.laser.home` | - | `device.move` | no | grbl | LaserFoundry's Home: `$H`, one homing cycle. |
| `mc.laser.unlock` | - | `device.move` | no | grbl | LaserFoundry's Unlock: `$X`. |
| `mc.laser.set_origin` | - | `device.move` | no | grbl | LaserFoundry's Set origin: `G92 X0 Y0`. |
| `mc.laser.go_origin` | - | `device.move` | no | grbl | LaserFoundry's Go to origin: `G90 G0 X0 Y0`. |
| `mc.laser.read_settings` | - | `device.view` | no | grbl | LaserFoundry's settings read: `$$`. |
| `mc.laser.frame` | `power` (number, percent) | `device.move` | no | grbl | LaserFoundry's Frame: `gcode::frame` over the design's bounds, run as a job. |
| `mc.laser.start` | `program` (string), `from_line` (integer) | `mc.laser.start` | yes | grbl | LaserFoundry's Start. A person's own session only (hub-core-v2.md §2.1). |
| `mc.laser.pause` | - | `job.control` | no | grbl | LaserFoundry's Pause. |
| `mc.laser.resume` | - | `job.control` | no | grbl | LaserFoundry's Resume. |
| `mc.laser.stop` | - | `job.control` | no | grbl | LaserFoundry's Stop: a GRBL soft reset, the laser off. |
