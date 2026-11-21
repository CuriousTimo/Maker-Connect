# BedLevelVisualizer port

**Reference:** OctoPrint-BedLevelVisualizer at 05ba418 (`reference/OctoPrint-BedLevelVisualizer`,
AGPL-3.0), `octoprint_bedlevelvisualizer/__init__.py`: the regexes of `__init__` (`:46-70`),
`enable_mesh_collection` (`:207-214`), `process_gcode` (`:228-445`), `unique_floats` (`:472-475`).

**Ours:** `crates/hub/src/devices/bed_level_visualizer.rs`. The hub runs each line of *Probe bed*'s
answer (`gcode_output` of its `FINISHED`, extension part 9) through it, in order, after
`enable_mesh_collection`; one state per printer while the hub runs (#107).

**Checked by:** `scripts/bed-level-visualizer-check.py` runs BLV's own `process_gcode` on every case of
`crates/hub/src/devices/bed_level_visualizer_cases.json` and holds its meshes to
`bed_level_visualizer_expected.json`; the hub test
`devices::bed_level_visualizer::tests::the_reference_cases` holds ours to the same file.

**Result: identical** on every case below (each point to 4 decimals; a point `float` cannot read is
none on both sides).

<!-- generated: begin -->
| Case | What | BLV's mesh |
|---|---|---|
| 0 | AtomX MK3 G28 G29 M420 V (Marlin bilinear, #107): the grid G29 prints, up to its ok | [[0.648, 0.151, 0.232, 0.08], [0.629, 0.21, 0.25, 0.131], [0.631, 0.184, 0.142, -0.049], [0.725, 0.085, -0.133, -0.336]] |
| 1 | Home XYZ first ends the collection | none |
| 2 | an ok without a grid | none |
| 3 | a correction matrix clears the grid | none |
| 4 | Mesh Bed Level data: is no mesh line, so it clears nothing | [[0.1, 0.2], [0.3, 0.4]] |
| 5 | old Marlin's Bed x/y/z points, rearranged | [[0.1, 0.2], [0.3, 0.4]] |
| 6 | old Marlin and Eqn coefficients: BLV raises there (no group 2 to substitute), the ok finishes | [[0.1, 0.2]] |
| 7 | Marlin bilinear, an unmeasured point (' =====') drops its row | [[0.4, 0.5, 0.6]] |
| 8 | a line of nan | [[0.1, 0.2, 0.3]] |
| 9 | a line of equal signs | [[0.1, 0.2]] |
| 10 | Prusa G81's measured points | [[0.12, 0.1, -0.05], [0.02, 0.0, -0.01]] |
| 11 | MakerGear's report: BLV raises at a[2], no mesh | none |
| 12 | CATMULL subdivision clears the grid | [[0.5, 0.6]] |
| 13 | a box that flips x and y | [[0.2, 0.1], [0.4, 0.3]] |
| 14 | Repetier G33 points, finished at T: | [[0.1, 0.2], [0.3, 0.4]] |
| 15 | UBL-like topography with unprobed dots | [[0.1, null], [null, 0.2]] |
| 16 | a stray dot point | [[0.1, null, 0.3]] |
<!-- generated: end -->

## Not covered

- BLV's settings other than its defaults (`ignore_correction_matrix`, `stripFirst`, `flipX`,
  `flipY`, `use_relative_offsets`, `use_center_origin`, `rotation`): our page has none of them.
- The `bed` BLV sends with the mesh (from OctoPrint's printer profile): the page draws the bed from
  the machine's own profile.
- A circular bed's mask (`create_circular_mask`): no profile of ours is circular.
- The plugin messages, the `mesh_data_collected` event, `BLV` lines and `echo:BEDLEVELVISUALIZER`
  while printing, the debug log.
- Python's `round(…, 4)` (half to even on the binary value) against ours (half away from zero),
  kept as `f32`: equal on every case here.
