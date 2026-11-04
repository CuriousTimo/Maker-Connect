//! What a G-code file says about itself, read from the comments slicers write: PrusaSlicer
//! (`; estimated printing time (normal mode) = 1h 2m 3s`, `; printer_model = MINI`, and its
//! thumbnails, base64 between `; thumbnail begin` and `; thumbnail end`) and Cura (`;TIME:`,
//! `;Filament used:`). PrusaSlicer puts its settings at the end and the thumbnails at the
//! start, so the start and the end of a file are read, not all of it.

use base64::Engine;
use maker_connect_protocol::MachineKind;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// How much of the start and of the end of a file is read.
const HEAD_BYTES: u64 = 2 * 1024 * 1024;
const TAIL_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileMeta {
    /// What the file is for, from its commands: heating means a printer, a spindle or laser
    /// (`M3`/`M4`) without heating a laser.
    pub kind: Option<MachineKind>,
    /// The slicer's printer model (PrusaSlicer's `printer_model`: `MINI`, `MK3S`).
    pub printer_model: Option<String>,
    pub slicer: Option<String>,
    pub estimated_s: Option<u64>,
    pub filament_g: Option<f64>,
    pub filament_m: Option<f64>,
    pub filament_type: Option<String>,
    pub layer_height_mm: Option<f64>,
    pub height_mm: Option<f64>,
    pub nozzle_mm: Option<f64>,
    /// Whether a thumbnail was found (kept beside the file as PNG).
    #[serde(default)]
    pub thumbnail: bool,
    /// Where its moves go: X and Y from, X and Y to (absolute moves, the whole file).
    #[serde(default)]
    pub bounds_mm: Option<[f64; 4]>,
    /// The highest power it asks (`S` on a move or `M3`/`M4`): for a laser, set against the
    /// machine's full power (`$30`).
    #[serde(default)]
    pub max_power: Option<f64>,
}

/// Files up to this size are read whole for where their moves go.
const EXTENT_BYTES: u64 = 256 * 1024 * 1024;

/// Where a program's moves go, and the highest power it asks: one pass over all of it.
pub fn extent(lines: impl std::io::BufRead) -> (Option<[f64; 4]>, Option<f64>) {
    let mut relative = false;
    let (mut x, mut y) = (0.0f64, 0.0f64);
    let mut bounds: Option<[f64; 4]> = None;
    let mut power: Option<f64> = None;
    for line in lines.lines().map_while(Result::ok) {
        let code = line
            .split([';', '('])
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_uppercase();
        let mut words = code.split_whitespace();
        let Some(first) = words.next() else { continue };
        let value = |letter: char| {
            code.split_whitespace()
                .skip(1)
                .find_map(|w| w.strip_prefix(letter)?.parse::<f64>().ok())
                .filter(|v| v.is_finite())
        };
        match first {
            "G90" => relative = false,
            "G91" => relative = true,
            "G0" | "G1" | "G2" | "G3" | "G00" | "G01" | "G02" | "G03" => {
                let (nx, ny) = (value('X'), value('Y'));
                if relative {
                    x += nx.unwrap_or(0.0);
                    y += ny.unwrap_or(0.0);
                } else {
                    x = nx.unwrap_or(x);
                    y = ny.unwrap_or(y);
                }
                if nx.is_some() || ny.is_some() {
                    bounds = Some(match bounds {
                        None => [x, y, x, y],
                        Some([a, b, c, d]) => [a.min(x), b.min(y), c.max(x), d.max(y)],
                    });
                }
            }
            _ => {}
        }
        if matches!(first, "G1" | "G01" | "G2" | "G3" | "M3" | "M4")
            && let Some(s) = value('S')
        {
            power = Some(power.map_or(s, |p: f64| p.max(s)));
        }
    }
    (bounds, power)
}

/// Reads a file's metadata, and its largest PNG thumbnail.
pub fn read(path: &Path) -> io::Result<(FileMeta, Option<Vec<u8>>)> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let mut head = Vec::new();
    (&mut file).take(HEAD_BYTES).read_to_end(&mut head)?;
    let mut tail = Vec::new();
    if size > HEAD_BYTES {
        file.seek(SeekFrom::Start(
            size.saturating_sub(TAIL_BYTES).max(HEAD_BYTES),
        ))?;
        file.read_to_end(&mut tail)?;
    }
    let head = String::from_utf8_lossy(&head);
    let tail = String::from_utf8_lossy(&tail);
    let (mut meta, thumbnail) = parse(&head, &tail);
    if size <= EXTENT_BYTES {
        file.seek(SeekFrom::Start(0))?;
        (meta.bounds_mm, meta.max_power) = extent(std::io::BufReader::new(file));
    }
    Ok((meta, thumbnail))
}

/// The metadata in a file's start and end.
pub fn parse(head: &str, tail: &str) -> (FileMeta, Option<Vec<u8>>) {
    let mut meta = FileMeta::default();
    let mut heats = false;
    let mut laser = false;
    for line in head.lines().chain(tail.lines()) {
        let line = line.trim();
        if let Some(comment) = line.strip_prefix(';') {
            setting(&mut meta, comment.trim());
            continue;
        }
        let code = line.split([' ', ';']).next().unwrap_or("");
        match code {
            "M104" | "M109" | "M140" | "M190" => heats = true,
            "M3" | "M4" => laser = true,
            _ => {}
        }
    }
    meta.kind = if heats || meta.printer_model.is_some() {
        Some(MachineKind::Fdm)
    } else if laser {
        Some(MachineKind::Laser)
    } else {
        None
    };
    let thumbnail = largest_thumbnail(head);
    meta.thumbnail = thumbnail.is_some();
    (meta, thumbnail)
}

/// One `key = value` comment (PrusaSlicer) or `KEY:value` comment (Cura).
fn setting(meta: &mut FileMeta, comment: &str) {
    let first = |v: &str| v.split([',', ';']).next().unwrap_or("").trim().to_owned();
    let number = |v: &str| first(v).parse::<f64>().ok();
    if let Some(rest) = comment.strip_prefix("generated by ") {
        meta.slicer = Some(rest.split(" on ").next().unwrap_or(rest).trim().to_owned());
        return;
    }
    if let Some(rest) = comment.strip_prefix("Generated with ") {
        meta.slicer = Some(rest.trim().to_owned());
        return;
    }
    if let Some((key, value)) = comment.split_once(" = ") {
        let value = value.trim();
        match key.trim() {
            "estimated printing time (normal mode)" => meta.estimated_s = duration(value),
            "filament used [g]" => meta.filament_g = number(value),
            "filament used [mm]" => meta.filament_m = number(value).map(|mm| mm / 1000.0),
            "filament_type" => meta.filament_type = Some(first(value)).filter(|v| !v.is_empty()),
            "layer_height" => meta.layer_height_mm = number(value),
            "max_layer_z" => meta.height_mm = number(value),
            "nozzle_diameter" => meta.nozzle_mm = number(value),
            "printer_model" => meta.printer_model = Some(first(value)).filter(|v| !v.is_empty()),
            _ => {}
        }
        return;
    }
    if let Some((key, value)) = comment.split_once(':') {
        match key.trim() {
            "TIME" => meta.estimated_s = value.trim().parse().ok(),
            "Filament used" => meta.filament_m = number(value.trim_end_matches('m')),
            "Layer height" => meta.layer_height_mm = number(value),
            "MAXZ" => meta.height_mm = number(value),
            _ => {}
        }
    }
}

/// "1d 2h 3m 4s" (or any part of it) in seconds.
fn duration(text: &str) -> Option<u64> {
    let mut total = 0;
    for part in text.split_whitespace() {
        let (number, unit) = part.split_at(part.len().checked_sub(1)?);
        let n: u64 = number.parse().ok()?;
        total += n * match unit {
            "d" => 86_400,
            "h" => 3600,
            "m" => 60,
            "s" => 1,
            _ => return None,
        };
    }
    Some(total)
}

/// The largest PNG thumbnail in the file's start (`; thumbnail begin 220x124 12345`).
/// QOI and JPG thumbnails (`; thumbnail_QOI begin`) are not read yet.
fn largest_thumbnail(head: &str) -> Option<Vec<u8>> {
    let mut best: Option<(u64, Vec<u8>)> = None;
    let mut current: Option<(u64, String)> = None;
    for line in head.lines() {
        let line = line.trim();
        if let Some(rest) = line
            .strip_prefix("; thumbnail begin ")
            .or_else(|| line.strip_prefix("; thumbnail_PNG begin "))
        {
            let area = rest
                .split_whitespace()
                .next()
                .and_then(|size| size.split_once('x'))
                .and_then(|(w, h)| Some(w.parse::<u64>().ok()? * h.parse::<u64>().ok()?))
                .unwrap_or(0);
            current = Some((area, String::new()));
        } else if line.starts_with("; thumbnail end") || line.starts_with("; thumbnail_PNG end") {
            if let Some((area, data)) = current.take()
                && let Ok(png) = base64::engine::general_purpose::STANDARD.decode(data)
                && png.starts_with(b"\x89PNG")
                && best.as_ref().is_none_or(|(a, _)| area > *a)
            {
                best = Some((area, png));
            }
        } else if let Some((_, data)) = current.as_mut()
            && let Some(chunk) = line.strip_prefix(';')
        {
            data.push_str(chunk.trim());
        }
    }
    best.map(|(_, png)| png)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest PNG there is: one transparent pixel.
    const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGBgAAAABQABpfZFQAAAAABJRU5ErkJggg==";

    #[test]
    fn a_prusaslicer_file_tells_its_story() {
        let head = format!(
            "; generated by PrusaSlicer 2.8.1+MacOS-arm64 on 2026-11-04 at 10:00:00 UTC\n\n;\n; thumbnail begin 1x1 {}\n; {PIXEL}\n; thumbnail end\n;\nM73 P0 R62\nM104 S215\nG28\n",
            PIXEL.len()
        );
        let tail = "; filament used [mm] = 1234.56\n; filament used [g] = 3.68\n; estimated printing time (normal mode) = 1h 2m 3s\n; filament_type = PETG\n; layer_height = 0.2\n; max_layer_z = 20.2\n; nozzle_diameter = 0.4\n; printer_model = MINI\n";
        let (meta, thumbnail) = parse(&head, tail);
        assert_eq!(
            meta.slicer.as_deref(),
            Some("PrusaSlicer 2.8.1+MacOS-arm64")
        );
        assert_eq!(meta.estimated_s, Some(3723));
        assert_eq!(meta.filament_g, Some(3.68));
        assert!((meta.filament_m.unwrap() - 1.23456).abs() < 1e-9);
        assert_eq!(meta.filament_type.as_deref(), Some("PETG"));
        assert_eq!(meta.printer_model.as_deref(), Some("MINI"));
        assert_eq!(meta.kind, Some(MachineKind::Fdm));
        assert!(thumbnail.unwrap().starts_with(b"\x89PNG"));
        assert!(meta.thumbnail);
    }

    /// The example laser files say what they are and how long they take.
    #[test]
    fn the_laser_examples_are_for_a_laser() {
        let (grid, _) = parse(
            include_str!("../../../examples/laser/power-speed-grid.gcode"),
            "",
        );
        assert_eq!(grid.kind, Some(MachineKind::Laser));
        assert_eq!(
            grid.slicer.as_deref(),
            Some("Maker Connect example generator")
        );
        assert!(grid.estimated_s.is_some_and(|s| s > 60));
    }

    /// Where the moves go, relative ones too, and the highest power.
    #[test]
    fn the_extent_of_a_program() {
        let program = "G21\nG90\nM4 S0\nG0 X10 Y10\nG1 X30 Y10 S250 F1000\nG91\nG1 Y20 S400\nG90\nG0 X5 Y40 ; back\n";
        let (bounds, power) = extent(program.as_bytes());
        assert_eq!(bounds, Some([5.0, 10.0, 30.0, 40.0]));
        assert_eq!(power, Some(400.0));
        let (bounds, _) =
            extent(include_str!("../../../examples/laser/power-speed-grid.gcode").as_bytes());
        assert_eq!(bounds, Some([10.0, 10.0, 58.0, 58.0]));
    }

    #[test]
    fn cura_and_laser_files_are_read_too() {
        let (cura, _) = parse(
            ";FLAVOR:Marlin\n;TIME:5400\n;Filament used: 2.5m\n;Layer height: 0.15\nM190 S60\n",
            "",
        );
        assert_eq!(cura.estimated_s, Some(5400));
        assert_eq!(cura.filament_m, Some(2.5));
        assert_eq!(cura.kind, Some(MachineKind::Fdm));
        let (laser, thumbnail) = parse(
            "; LightBurn 1.7\nG90\nM4 S0\nG1 X10 Y10 S800 F3000\nM5\n",
            "",
        );
        assert_eq!(laser.kind, Some(MachineKind::Laser));
        assert!(thumbnail.is_none());
        assert_eq!(duration("2d 0h 5m 1s"), Some(173_101));
    }
}
