# Machine profiles

One file per machine model, by brand: what Maker Connect needs to know to show a machine
and plan its jobs (the build volume, the bed to draw) and how an agent recognises it.

```toml
brand = "Prusa"
model = "Original Prusa i3 MK3S/+"
kind = "fdm"                       # fdm, laser, cnc, cutter, label, paper, other
build_volume_mm = [250, 210, 210]  # x, y, z
bed_origin_mm = [0, 0]             # where X0 Y0 is, from the bed's front left corner
nozzle_mm = 0.4
firmware = "prusa"                 # prusa, marlin, klipper, grbl, ...
max_nozzle_c = 300
max_bed_c = 120

[detect]                           # optional: how an agent recognises it
model_code = 302                   # Prusa's M862.2 Q
machine_type = "Prusa i3 MK3S"     # M115's MACHINE_TYPE
```

Values marked `# check` are not yet confirmed on the machine itself. Later these become
community profiles: one pull request per machine.
