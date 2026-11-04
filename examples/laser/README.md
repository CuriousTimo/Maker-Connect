# Laser test files

For a first run of a GRBL laser through Maker Connect (upload them on the Files page, then
"Print on" the laser). Written by hand for this, not by a laser program; each streams to the
end against LaserFoundry's simulated GRBL in the agent's tests.

| File | What | Time |
|---|---|---|
| `first-light.gcode` | Runs the outline with the laser off (to see where it goes), then one 20 mm square at 10 % power, 1000 mm/min, from X10 Y10 | seconds |
| `power-speed-grid.gcode` | 5 × 5 filled squares of 8 mm: power 10 to 50 % across, speed 600 to 3000 mm/min from front to back, from X10 Y10 to X58 Y58 | a few minutes |

They assume GRBL 1.1 in laser mode (`$32=1`), power from `S0` to `S1000` (`$30=1000`, the
Ortur's default), positions from the machine origin after homing (`$H`), and dynamic power
(`M4`: the beam follows the speed and is off when the head stands still).

Before the first run: home, check the head can reach X10 Y10 to X58 Y58, put scrap cardboard
or plywood down, wear the goggles, and stay at the machine. Stop switches the laser off.
