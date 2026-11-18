# Actions

<!-- Generated from crates/device/src/registry.rs. Do not edit: run `MC_WRITE_REGISTRIES=1 cargo test -p maker-connect-device --test registry_docs`. -->

| Action | Presets | Damaging | |
|---|---|---|---|
| `device.view` | - | no | See a device and read from it. |
| `device.move` | - | no | Move a device's axes, home, motors off. |
| `device.heat` | - | no | Set heaters and fans. |
| `device.gcode` | - | yes | Send raw G-code (owner, 2026-11-18: granted by name). |
| `job.start` | - | no | Start a job. |
| `job.control` | - | no | Pause, resume or stop a job; change its speed or flow; delete a file. |
| `mc.laser.start` | - | yes | Start a laser job (owner, 2026-11-18: granted by name). |
