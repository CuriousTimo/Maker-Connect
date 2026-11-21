//! OctoPrint-BedLevelVisualizer's mesh collection, ported: the reference is
//! `reference/OctoPrint-BedLevelVisualizer` at 05ba418 (AGPL-3.0, `setup.py:33`),
//! `octoprint_bedlevelvisualizer/__init__.py`: the regexes of `__init__` (`:46-70`),
//! `enable_mesh_collection` (`:207-214`), `process_gcode` (`:228-445`) and `unique_floats`
//! (`:472-475`).
//!
//! Where it runs (#107, owner 2026-11-21): BLV collects a mesh after its own *Update Mesh*, from
//! the lines the printer sends until an `ok`. The hub's *Probe bed* is that button: for a Marlin on
//! the agent its answer comes back as `gcode_output` (extension part 9), and each line goes through
//! [`Blv::process_gcode`] in order, as OctoPrint's `gcode.received` hook hands BLV each line.
//!
//! The settings are BLV's defaults (`get_settings_defaults`, `:74-111`): `ignore_correction_matrix`,
//! `stripFirst`, `flipX`, `flipY`, `use_relative_offsets`, `use_center_origin` false, `rotation` 0.
//!
//! Not covered:
//! - the `bed` it sends beside the mesh (from OctoPrint's printer profile, `:330-356`): the page
//!   draws the bed from the machine's own profile;
//! - a circular bed's mask (`:428-435`, `create_circular_mask`): no profile of ours is circular;
//! - the plugin messages and the event (`send_plugin_message`, `send_mesh_data_collected_event`),
//!   the `BLV` lines and `echo:BEDLEVELVISUALIZER` while printing (`:229-236`), the debug log.
//! - Python's `round(…, 4)` rounds half to even on the binary value; here half away from zero, the
//!   value then kept as an `f32`.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

/// Python's `re.match`: anchored at the start (the patterns end with their own `$`).
fn at_start(pattern: &str) -> Regex {
    Regex::new(&format!("^(?:{pattern})")).unwrap()
}

static MESH_DATA: LazyLock<Regex> = LazyLock::new(|| {
    at_start(
        r"^((G33.+)|(Bed.+)|(Llit.+)|(\d+\s)|(\|\s*)|(\s*\[\s+)|(\[?\s?\+?-?\d+?\.\d+\]?\s*,?)|(\s?\.\s*)|(NAN,?)|(nan\s?,?)|(=======\s?,?))+(\s+\],?)?$",
    )
});
static BED_LEVEL_CORRECTION: LazyLock<Regex> =
    LazyLock::new(|| at_start(r"^(Mesh )?Bed Level (Correction Matrix|data):.*$"));
static NANS: LazyLock<Regex> = LazyLock::new(|| at_start(r"^(nan\s?,?)+$"));
/// Used both to match (`re.match`) and to substitute (`re.sub`), as the reference does.
static EQUAL_SIGNS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(=======\s?,?)+$").unwrap());
static MESH_DATA_EXTRACTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\+?-?\d*\.\d*)").unwrap());
static OLD_MARLIN: LazyLock<Regex> = LazyLock::new(|| at_start(r"^(Bed x:.+)|(Llit x:.+)$"));
static MAKERGEAR: LazyLock<Regex> =
    LazyLock::new(|| at_start(r"^(\s=\s\[)(\s*,?\s*\[(\s?-?\d+.\d+,?)+\])+\];?$"));
static REPETIER: LazyLock<Regex> = LazyLock::new(|| at_start(r"^G33 X.+$"));
static NAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(nan)").unwrap());
static CATMULL: LazyLock<Regex> =
    LazyLock::new(|| at_start(r"^Subdivided with CATMULL ROM Leveling Grid:.*$"));
static EXTRACTED_BOX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(\s*(\d+),\s*(\d+)\)").unwrap());
static EQN_COEFFICIENTS: LazyLock<Regex> = LazyLock::new(|| at_start(r"^Eqn coefficients:.+$"));

/// A point as BLV holds it: the text its regex found, or a number (a MakerGear report's JSON, or
/// after the old-Marlin rearranging).
#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Text(String),
    Number(f64),
}

impl Cell {
    /// Python's `float(x)`, of the texts BLV's regex finds (digits, a dot, a sign).
    fn float(&self) -> Option<f64> {
        match self {
            Cell::Text(t) => t.parse().ok(),
            Cell::Number(n) => Some(*n),
        }
    }

    /// The key Python's `set` tells points apart by.
    fn key(&self) -> String {
        match self {
            Cell::Text(t) => format!("t{t}"),
            Cell::Number(n) => format!("n{}", if *n == 0.0 { 0.0 } else { *n }),
        }
    }
}

/// BLV's state (`__init__`, `:27-45`): one per printer, kept while the hub runs, as OctoPrint keeps
/// its plugin's (the flags it sets stay set across meshes, as there).
#[derive(Debug, Default)]
pub(crate) struct Blv {
    processing: bool,
    old_marlin: bool,
    makergear: bool,
    old_marlin_offset: f64,
    repetier_firmware: bool,
    mesh: Vec<Vec<Cell>>,
    box_: Vec<[f64; 2]>,
    flip_x: bool,
    flip_y: bool,
}

/// What a Python exception in the hook would have stopped (OctoPrint logs it and goes on).
struct Raised;

impl Blv {
    /// `enable_mesh_collection` (`:207-214`).
    pub(crate) fn enable_mesh_collection(&mut self) {
        self.mesh.clear();
        self.box_.clear();
        self.processing = true;
    }

    /// `process_gcode` (`:228-445`) for one received line: the mesh when this line completes it
    /// (front row first, each from left to right, as Marlin prints it), a point it cannot read
    /// `None`.
    pub(crate) fn process_gcode(&mut self, line: &str) -> Option<Vec<Vec<Option<f32>>>> {
        self.process(line).ok().flatten()
    }

    fn process(&mut self, line: &str) -> Result<Option<Vec<Vec<Option<f32>>>>, Raised> {
        if !self.processing {
            return Ok(None);
        }
        let mut line = line.to_owned();
        // `ignore_correction_matrix` is false (its default): the line stays.
        if !line.contains("ok") {
            let stripped = line.trim().to_owned();
            if MESH_DATA.is_match(&stripped) {
                if BED_LEVEL_CORRECTION.is_match(&stripped) {
                    // "resetting mesh to blank because of correction matrix"
                    self.mesh.clear();
                    return Ok(None);
                }
                if NANS.is_match(&stripped) {
                    line = NAN.replace_all(&line, "0.0").into_owned();
                }
                if EQUAL_SIGNS.is_match(line.trim()) {
                    line = EQUAL_SIGNS.replace_all(&line, "0.0").into_owned();
                }
                let new_line: Vec<Cell> = MESH_DATA_EXTRACTION
                    .captures_iter(&line)
                    .map(|c| Cell::Text(c[1].to_owned()))
                    .collect();
                if OLD_MARLIN.is_match(line.trim()) {
                    self.old_marlin = true;
                }
                if REPETIER.is_match(line.trim()) {
                    self.repetier_firmware = true;
                }
                // `stripFirst` is false (its default).
                if !new_line.is_empty() {
                    self.mesh.push(new_line);
                }
            } else if CATMULL.is_match(&stripped) {
                self.mesh.clear();
            } else if EXTRACTED_BOX.is_match(&stripped) {
                let found: Vec<[f64; 2]> = EXTRACTED_BOX
                    .captures_iter(&stripped)
                    .map(|c| [c[1].parse().unwrap_or(0.0), c[2].parse().unwrap_or(0.0)])
                    .collect();
                if found.len() == 2 {
                    self.box_.extend(found);
                }
                if self.box_.len() == 2 && self.box_[0][0] > self.box_[1][0] {
                    self.flip_x = true;
                }
                if self.box_.len() == 4 && self.box_[0][1] > self.box_[3][1] {
                    self.flip_y = true;
                }
            }
            if MAKERGEAR.is_match(&line) {
                let json = line.trim().replace("= ", "").replace(';', "");
                let rows: Vec<Vec<f64>> = serde_json::from_str(&json).map_err(|_| Raised)?;
                self.mesh = rows
                    .into_iter()
                    .map(|r| r.into_iter().map(Cell::Number).collect())
                    .collect();
                self.old_marlin = true;
                self.makergear = true;
                line = "ok".to_owned();
            }
            if self.old_marlin && EQN_COEFFICIENTS.is_match(line.trim()) {
                // `regex_eqn_coefficients.sub(r"\2", …)`: the pattern has no group 2, so Python
                // raises `re.error` ("invalid group reference 2") here.
                return Err(Raised);
            }
            if line.contains("Home XYZ first") {
                // BLV also tells its page the error.
                self.processing = false;
            }
        }
        let done = line.contains("ok") || (self.repetier_firmware && line.contains("T:"));
        if !done || self.mesh.is_empty() {
            return Ok(None);
        }
        if self.old_marlin || self.repetier_firmware {
            // rearrange matrix from point lists to coordinate lists
            let a: Vec<Vec<Cell>> = if self.makergear {
                self.mesh.clone()
            } else {
                zip(&self.mesh)
            };
            let x = unique_floats(a.first().ok_or(Raised)?)?;
            let y = unique_floats(a.get(1).ok_or(Raised)?)?;
            let values = a.get(2).ok_or(Raised)?;
            let (rows, cols) = (y.len(), x.len());
            let mut z = vec![vec![Cell::Number(0.0); cols]; rows];
            let mut k = 0;
            for row in z.iter_mut() {
                for cell in row.iter_mut() {
                    *cell = values.get(k).ok_or(Raised)?.clone();
                    k += 1;
                }
            }
            let offset = if self.old_marlin {
                self.old_marlin_offset
            } else {
                0.0
            };
            self.mesh = z
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|v| Ok(Cell::Number(round4(v.float().ok_or(Raised)? - offset))))
                        .collect::<Result<_, Raised>>()
                })
                .collect::<Result<_, Raised>>()?;
        }
        // "stopping mesh collection"
        if self.flip_x {
            for row in &mut self.mesh {
                row.reverse();
            }
        }
        if self.flip_y {
            self.mesh.reverse();
        }
        // `use_relative_offsets` false, `rotation` 0 (their defaults); a bed is not circular.
        self.processing = false;
        Ok(Some(
            self.mesh
                .iter()
                .map(|row| row.iter().map(|c| c.float().map(|v| v as f32)).collect())
                .collect(),
        ))
    }
}

/// Python's `list(zip(*mesh))`: column lists, as long as the shortest row.
fn zip(mesh: &[Vec<Cell>]) -> Vec<Vec<Cell>> {
    let width = mesh.iter().map(Vec::len).min().unwrap_or(0);
    (0..width)
        .map(|i| mesh.iter().map(|row| row[i].clone()).collect())
        .collect()
}

/// `unique_floats` (`:472-475`): the distinct values (by Python's `set`), each as `float`. Only how
/// many there are is used.
fn unique_floats(list: &[Cell]) -> Result<Vec<f64>, Raised> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for cell in list {
        if seen.insert(cell.key()) {
            out.push(cell.float().ok_or(Raised)?);
        }
    }
    Ok(out)
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(lines: &[&str]) -> Option<Vec<Vec<Option<f32>>>> {
        let mut blv = Blv::default();
        blv.enable_mesh_collection();
        let mut mesh = None;
        for line in lines {
            if let Some(found) = blv.process_gcode(line) {
                mesh = Some(found);
            }
        }
        mesh
    }

    /// The AtomX MK3's `G28`, `G29`, `M420 V` (Marlin bilinear, #107): the grid that `G29` prints,
    /// complete at its `ok`; what `M420 V` prints after it is not collected again.
    #[test]
    fn a_marlin_bilinear_grid() {
        let mesh = collect(&[
            "X:-4.00 Y:-26.00 Z:10.00 E:0.00 Count X:-320 Y:-2080 Z:4000",
            "ok",
            "Bilinear Leveling Grid:",
            "      0      1      2      3",
            " 0 +0.648 +0.151 +0.232 +0.080",
            " 1 +0.629 +0.210 +0.250 +0.131",
            " 2 +0.631 +0.184 +0.142 -0.049",
            " 3 +0.725 +0.085 -0.133 -0.336",
            "X:23.00 Y:5.00 Z:5.00 E:0.00 Count X:1840 Y:400 Z:2000",
            "ok",
            "Bilinear Leveling Grid:",
            "      0      1      2      3",
            " 0 +1.000 +1.000 +1.000 +1.000",
            "echo:Bed Leveling ON",
            "ok",
        ])
        .unwrap();
        assert_eq!(
            mesh,
            vec![
                vec![Some(0.648), Some(0.151), Some(0.232), Some(0.080)],
                vec![Some(0.629), Some(0.210), Some(0.250), Some(0.131)],
                vec![Some(0.631), Some(0.184), Some(0.142), Some(-0.049)],
                vec![Some(0.725), Some(0.085), Some(-0.133), Some(-0.336)],
            ]
        );
    }

    /// Every case of `bed_level_visualizer_cases.json` gives the mesh BLV's own code gives
    /// (`bed_level_visualizer_expected.json`, written by scripts/bed-level-visualizer-check.py;
    /// docs/audits/bed-level-visualizer.md), each point to 4 decimals.
    #[test]
    fn the_reference_cases() {
        #[derive(serde::Deserialize)]
        struct Case {
            what: String,
            lines: Vec<String>,
        }
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("bed_level_visualizer_cases.json")).unwrap();
        let expected: Vec<Option<Vec<Vec<Option<f64>>>>> =
            serde_json::from_str(include_str!("bed_level_visualizer_expected.json")).unwrap();
        assert_eq!(cases.len(), expected.len());
        for (case, expected) in cases.iter().zip(expected) {
            let lines: Vec<&str> = case.lines.iter().map(String::as_str).collect();
            let ours = collect(&lines).map(|mesh| {
                mesh.iter()
                    .map(|row| {
                        row.iter()
                            .map(|v| v.map(|v| round4(f64::from(v))))
                            .collect()
                    })
                    .collect::<Vec<Vec<Option<f64>>>>()
            });
            assert_eq!(ours, expected, "{}", case.what);
        }
    }

    /// `Home XYZ first` ends the collection; an `ok` without a grid gives none.
    #[test]
    fn no_grid_no_mesh() {
        assert_eq!(
            collect(&["echo:Home XYZ first", "ok", " 0 +0.1 +0.2"]),
            None
        );
        assert_eq!(collect(&["ok", "ok"]), None);
    }

    /// Before Update Mesh (or after the mesh), nothing is collected.
    #[test]
    fn only_while_collecting() {
        let mut blv = Blv::default();
        assert_eq!(blv.process_gcode(" 0 +0.1 +0.2"), None);
        assert_eq!(blv.process_gcode("ok"), None);
    }

    /// A correction matrix line clears what was gathered (`ignore_correction_matrix` false).
    #[test]
    fn a_correction_matrix_clears() {
        assert_eq!(
            collect(&[" 0 +0.1 +0.2", "Bed Level Correction Matrix:", "ok",]),
            None
        );
    }

    /// Old Marlin's points (`Bed x: … y: … z: …`): rearranged into rows by their x and y.
    #[test]
    fn old_marlin_points() {
        let mesh = collect(&[
            "Bed x: 10.000 y: 10.000 z: 0.100",
            "Bed x: 20.000 y: 10.000 z: 0.200",
            "Bed x: 10.000 y: 20.000 z: 0.300",
            "Bed x: 20.000 y: 20.000 z: 0.400",
            "ok",
        ])
        .unwrap();
        assert_eq!(
            mesh,
            vec![vec![Some(0.1), Some(0.2)], vec![Some(0.3), Some(0.4)]]
        );
    }
}
