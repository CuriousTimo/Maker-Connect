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
    /// A label (a PNG, one pixel per dot): its length along the tape and its height across
    /// it, in dots.
    #[serde(default)]
    pub label_dots: Option<[u32; 2]>,
    /// What PrusaSlicer says besides (the keys Prusa's own gcode-metadata reads): the
    /// nozzle and bed temperatures, the filament's colour (`#RRGGBB`), whether it wants a
    /// high-flow nozzle or an abrasive-proof one, infill, supports, and the objects in it
    /// (by name, as cancel-object knows them). With several tools, the first's
    /// temperatures and colour.
    #[serde(default)]
    pub nozzle_temp_c: Option<f64>,
    #[serde(default)]
    pub bed_temp_c: Option<f64>,
    #[serde(default)]
    pub colour: Option<String>,
    #[serde(default)]
    pub high_flow: Option<bool>,
    #[serde(default)]
    pub abrasive: Option<bool>,
    #[serde(default)]
    pub fill_density: Option<String>,
    #[serde(default)]
    pub supports: Option<bool>,
    #[serde(default)]
    pub objects: Vec<String>,
}

/// Materials and printers as PrusaSlicer writes them into a file's name
/// (`…_0.4n_0.2mm_PETG_MINIIS_1h3m.gcode`): the names Prusa's gcode-metadata knows.
const NAME_MATERIALS: &[&str] = &[
    "PLA Tough", "PLA", "PETG", "ABS", "ASA", "FLEX", "HIPS", "EDGE", "NGEN", "PA", "PVA",
    "PCTG", "PP", "PC", "PEBA", "CPE", "PVB", "PET", "METAL", "TPU", "NYLON", "IGLIDUR",
];
const NAME_PRINTERS: &[&str] = &[
    "COREONE", "HT90", "MK2.5", "MK2.5S", "MK2.5MMU2", "MK2.5SMMU2S", "MK3", "MK3S", "MK3MMU2",
    "MK3SMMU2S", "MK3MMU3", "MK3SMMU3", "MK3.9", "MK3.9MMU3", "MK3.9S", "MK3.9SMMU3", "MK3.5",
    "MK3.5MMU3", "MK3.5S", "MK3.5SMMU3", "MK4", "MK4MMU3", "MK4IS", "MK4ISMMU3", "MK4S",
    "MK4SMMU3", "MINI", "MINIIS", "XL", "XL2", "XL5", "XLIS", "XL2IS", "XL5IS", "SL1", "SL1S",
    "M1", "iX",
];

/// Fills in what a file's name tells, where its contents did not: the nozzle (`0.6n`), the
/// layer height (`0.32mm`), the material, the printer and the printing time (`8h55m`), as
/// PrusaSlicer names its files.
pub fn from_name(meta: &mut FileMeta, name: &str) {
    let stem = name
        .rsplit_once('.')
        .filter(|(_, ext)| ext.len() <= 6)
        .map_or(name, |(stem, _)| stem);
    let words: Vec<&str> = stem.split(['_', ' ', ',']).filter(|w| !w.is_empty()).collect();
    // `0.4n`, and `0_4n` where the dot became an underscore (as on a USB stick: "0" then
    // "4n" is 0.4, not 4).
    let number = |i: usize, unit: &str| -> Option<f64> {
        let digits = words[i].strip_suffix(unit)?;
        match i.checked_sub(1).map(|p| words[p]) {
            Some(whole) if !digits.contains('.') && whole.bytes().all(|b| b.is_ascii_digit()) => {
                format!("{whole}.{digits}").parse().ok()
            }
            _ => digits.parse().ok(),
        }
    };
    if meta.nozzle_mm.is_none() {
        meta.nozzle_mm = (0..words.len()).find_map(|i| number(i, "n"));
    }
    if meta.layer_height_mm.is_none() {
        meta.layer_height_mm = (0..words.len()).find_map(|i| number(i, "mm"));
    }
    if meta.filament_type.is_none() {
        meta.filament_type = NAME_MATERIALS
            .iter()
            .find(|m| words.iter().any(|w| w == *m) || (m.contains(' ') && stem.contains(*m)))
            .map(|m| (*m).to_owned());
    }
    if meta.printer_model.is_none() {
        meta.printer_model = words
            .iter()
            .filter_map(|w| NAME_PRINTERS.iter().find(|p| *p == w))
            .max_by_key(|p| p.len())
            .map(|p| (*p).to_owned());
    }
    if meta.estimated_s.is_none() {
        meta.estimated_s = words.iter().rev().find_map(|w| compact_duration(w));
    }
}

/// "1d18h15m", "3h22m", "45m" in seconds; None for anything else.
fn compact_duration(word: &str) -> Option<u64> {
    let mut total = 0;
    let mut digits = String::new();
    let mut any = false;
    for c in word.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let n: u64 = std::mem::take(&mut digits).parse().ok()?;
        total += n * match c {
            'd' => 86_400,
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return None,
        };
        any = true;
    }
    (any && digits.is_empty()).then_some(total)
}

/// A thumbnail in any of the formats PrusaSlicer writes, as PNG: PNG as it is, QOI
/// (the MK4, XL, CORE One) and JPG converted.
fn to_png(format: &str, data: &[u8]) -> Option<Vec<u8>> {
    match format {
        "PNG" => data.starts_with(b"\x89PNG").then(|| data.to_vec()),
        "QOI" => {
            let (width, height, rgba) = decode_qoi(data)?;
            encode_png(width, height, png::ColorType::Rgba, &rgba)
        }
        "JPG" => {
            let mut decoder = jpeg_decoder::Decoder::new(data);
            let pixels = decoder.decode().ok()?;
            let info = decoder.info()?;
            let (colour, pixels) = match info.pixel_format {
                jpeg_decoder::PixelFormat::RGB24 => (png::ColorType::Rgb, pixels),
                jpeg_decoder::PixelFormat::L8 => (png::ColorType::Grayscale, pixels),
                _ => return None,
            };
            encode_png(info.width as u32, info.height as u32, colour, &pixels)
        }
        _ => None,
    }
}

fn encode_png(width: u32, height: u32, colour: png::ColorType, pixels: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(colour);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(pixels).ok()?;
    writer.finish().ok()?;
    Some(out)
}

/// A QOI image (qoiformat.org's specification, written here from it): width, height and
/// RGBA pixels.
fn decode_qoi(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if data.len() < 22 || &data[..4] != b"qoif" {
        return None;
    }
    let width = u32::from_be_bytes(data[4..8].try_into().ok()?);
    let height = u32::from_be_bytes(data[8..12].try_into().ok()?);
    let count = (width as usize).checked_mul(height as usize)?;
    if count == 0 || count > 4096 * 4096 {
        return None;
    }
    let mut index = [[0u8; 4]; 64];
    let mut px = [0u8, 0, 0, 255];
    let mut out = Vec::with_capacity(count * 4);
    let mut p = 14;
    let end = data.len() - 8;
    let mut run = 0u8;
    for _ in 0..count {
        if run > 0 {
            run -= 1;
        } else if p < end {
            let b1 = data[p];
            p += 1;
            match b1 {
                0xFE => {
                    px[..3].copy_from_slice(data.get(p..p + 3)?);
                    p += 3;
                }
                0xFF => {
                    px.copy_from_slice(data.get(p..p + 4)?);
                    p += 4;
                }
                _ => match b1 >> 6 {
                    0 => px = index[(b1 & 0x3f) as usize],
                    1 => {
                        px[0] = px[0].wrapping_add(((b1 >> 4) & 3).wrapping_sub(2));
                        px[1] = px[1].wrapping_add(((b1 >> 2) & 3).wrapping_sub(2));
                        px[2] = px[2].wrapping_add((b1 & 3).wrapping_sub(2));
                    }
                    2 => {
                        let b2 = *data.get(p)?;
                        p += 1;
                        let dg = (b1 & 0x3f).wrapping_sub(32);
                        px[0] = px[0].wrapping_add(dg.wrapping_sub(8).wrapping_add(b2 >> 4));
                        px[1] = px[1].wrapping_add(dg);
                        px[2] = px[2].wrapping_add(dg.wrapping_sub(8).wrapping_add(b2 & 0x0f));
                    }
                    _ => run = b1 & 0x3f,
                },
            }
            let hash = (px[0] as usize * 3 + px[1] as usize * 5 + px[2] as usize * 7 + px[3] as usize * 11) % 64;
            index[hash] = px;
        }
        out.extend_from_slice(&px);
    }
    Some((width, height, out))
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

/// Binary G-code (`.bgcode`, PrusaSlicer for the MINI, MK4, XL): its blocks up to the G-code,
/// as libbgcode lays them out (a file header `GCDE`, version, checksum type; then blocks:
/// type, compression, size, compressed size when compressed, parameters, data, a CRC32 when
/// the file has them). Kept: the largest PNG thumbnail, and metadata blocks that are not
/// compressed (`key=value` lines), read as a text file's `; key = value` comments are.
fn read_binary(path: &Path) -> io::Result<(FileMeta, Option<Vec<u8>>)> {
    let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut header = [0u8; 10];
    file.read_exact(&mut header)?;
    let checksum = u16::from_le_bytes([header[8], header[9]]) == 1;
    let mut comments = String::new();
    let mut thumbnail: Option<(u32, Vec<u8>)> = None;
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    for _ in 0..64 {
        let mut head = [0u8; 8];
        if file.read_exact(&mut head).is_err() {
            break;
        }
        let (kind, compression) = (u16_at(&head, 0), u16_at(&head, 2));
        let size = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as u64;
        let stored = if compression != 0 {
            let mut c = [0u8; 4];
            file.read_exact(&mut c)?;
            u32::from_le_bytes(c) as u64
        } else {
            size
        };
        // The G-code itself: what comes before it is all we read.
        if kind == 1 || stored > 64 * 1024 * 1024 {
            break;
        }
        let params = if kind == 5 { 6 } else { 2 };
        let mut p = [0u8; 6];
        file.read_exact(&mut p[..params])?;
        let mut data = vec![0u8; stored as usize];
        file.read_exact(&mut data)?;
        if checksum {
            let mut crc = [0u8; 4];
            file.read_exact(&mut crc)?;
        }
        match kind {
            // A thumbnail: format 0 PNG, 1 JPG, 2 QOI (libbgcode), kept as PNG.
            5 if compression == 0 => {
                let area = u16_at(&p, 2) as u32 * u16_at(&p, 4) as u32;
                let format = match u16_at(&p, 0) {
                    0 => "PNG",
                    1 => "JPG",
                    2 => "QOI",
                    _ => "",
                };
                if thumbnail.as_ref().is_none_or(|(a, _)| area > *a)
                    && let Some(png) = to_png(format, &data)
                {
                    thumbnail = Some((area, png));
                }
            }
            // File, printer, print and slicer metadata, as INI lines.
            0 | 2 | 3 | 4 if compression == 0 && u16_at(&p, 0) == 0 => {
                for line in String::from_utf8_lossy(&data).lines() {
                    if let Some((k, v)) = line.split_once('=') {
                        comments += &format!("; {} = {}\n", k.trim(), v.trim());
                    }
                }
            }
            _ => {}
        }
    }
    let (mut meta, _) = parse(&comments, "");
    // Only a printer's slicer writes binary G-code.
    meta.kind = Some(MachineKind::Fdm);
    meta.thumbnail = thumbnail.is_some();
    Ok((meta, thumbnail.map(|(_, png)| png)))
}

/// Reads a file's metadata, and its largest PNG thumbnail.
pub fn read(path: &Path) -> io::Result<(FileMeta, Option<Vec<u8>>)> {
    let mut file = std::fs::File::open(path)?;
    let mut magic = [0u8; 8];
    if file.read_exact(&mut magic).is_ok() && magic == *b"\x89PNG\r\n\x1a\n" {
        return read_label(path);
    }
    if magic.starts_with(b"GCDE") {
        return read_binary(path);
    }
    file.seek(SeekFrom::Start(0))?;
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

/// A label: a PNG, one pixel per dot. It is its own thumbnail.
fn read_label(path: &Path) -> io::Result<(FileMeta, Option<Vec<u8>>)> {
    let png = std::fs::read(path)?;
    let label = maker_connect_label::Label::from_png(&png).map_err(io::Error::other)?;
    let meta = FileMeta {
        kind: Some(MachineKind::Label),
        label_dots: Some([label.length as u32, label.height as u32]),
        thumbnail: true,
        ..FileMeta::default()
    };
    Ok((meta, Some(png)))
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
    // With several tools a value comes per tool ("1.2, 3.4"): used filament is their sum (as
    // gcode-metadata reads it); a kind of filament is each one once ("PLA, PETG").
    let sum = |v: &str| -> Option<f64> {
        let parts: Vec<f64> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        (!parts.is_empty()).then(|| parts.iter().sum())
    };
    let flag = |v: &str| first(v).parse::<u8>().ok().map(|n| n != 0);
    if let Some((key, value)) = comment.split_once(" = ") {
        let value = value.trim();
        match key.trim() {
            "estimated printing time (normal mode)" => meta.estimated_s = duration(value),
            "filament used [g]" => meta.filament_g = sum(value),
            "filament used [mm]" => meta.filament_m = sum(value).map(|mm| mm / 1000.0),
            "filament_type" => {
                let mut kinds: Vec<&str> = Vec::new();
                for kind in value.split(';').map(str::trim).filter(|k| !k.is_empty()) {
                    if !kinds.contains(&kind) {
                        kinds.push(kind);
                    }
                }
                meta.filament_type = (!kinds.is_empty()).then(|| kinds.join(", "));
            }
            "layer_height" => meta.layer_height_mm = number(value),
            "max_layer_z" => meta.height_mm = number(value),
            "nozzle_diameter" => meta.nozzle_mm = number(value),
            "printer_model" => meta.printer_model = Some(first(value)).filter(|v| !v.is_empty()),
            "temperature" => meta.nozzle_temp_c = number(value),
            "bed_temperature" => meta.bed_temp_c = number(value),
            "extruder_colour" => {
                meta.colour = Some(first(value)).filter(|c| c.starts_with('#') && c.len() == 7)
            }
            "nozzle_high_flow" => meta.high_flow = flag(value),
            "filament_abrasive" => meta.abrasive = flag(value),
            "fill_density" => meta.fill_density = Some(first(value)).filter(|v| !v.is_empty()),
            // 0/1 before PrusaSlicer 3.0; a mode after ("none" is none).
            "support_material" => {
                meta.supports = match first(value).as_str() {
                    "none" => Some(false),
                    "enforcers_only" | "everywhere" => Some(true),
                    other => other.parse::<u8>().ok().map(|n| n != 0),
                }
            }
            // `{"objects":[{"name":"Shape-Box","polygon":[...]}, ...]}`
            "objects_info" => {
                if let Ok(info) = serde_json::from_str::<serde_json::Value>(value) {
                    meta.objects = info["objects"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|o| o["name"].as_str().map(str::to_owned))
                        .collect();
                }
            }
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

/// The largest thumbnail in the file's start, as PNG: `; thumbnail begin 220x124 12345`
/// (PNG), `; thumbnail_PNG begin`, `; thumbnail_QOI begin`, `; thumbnail_JPG begin`, each up
/// to its `end`, base64 on comment lines.
fn largest_thumbnail(head: &str) -> Option<Vec<u8>> {
    let mut best: Option<(u64, Vec<u8>)> = None;
    let mut current: Option<(u64, &str, String)> = None;
    for line in head.lines() {
        let line = line.trim();
        let begin = [("PNG", "; thumbnail begin "), ("PNG", "; thumbnail_PNG begin "), ("QOI", "; thumbnail_QOI begin "), ("JPG", "; thumbnail_JPG begin ")]
            .iter()
            .find_map(|(format, prefix)| line.strip_prefix(prefix).map(|rest| (*format, rest)));
        if let Some((format, rest)) = begin {
            let area = rest
                .split_whitespace()
                .next()
                .and_then(|size| size.split_once('x'))
                .and_then(|(w, h)| Some(w.parse::<u64>().ok()? * h.parse::<u64>().ok()?))
                .unwrap_or(0);
            current = Some((area, format, String::new()));
        } else if line.starts_with("; thumbnail end") || line.starts_with("; thumbnail_") && line.ends_with(" end") {
            if let Some((area, format, data)) = current.take()
                && best.as_ref().is_none_or(|(a, _)| area > *a)
                && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data)
                && let Some(png) = to_png(format, &bytes)
            {
                best = Some((area, png));
            }
        } else if let Some((_, _, data)) = current.as_mut()
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
    fn pixels(png: &[u8]) -> Vec<u8> {
        let mut reader = png::Decoder::new(png).read_info().unwrap();
        let mut out = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut out).unwrap();
        out.truncate(info.buffer_size());
        out
    }

    /// A QOI image with every kind of chunk (RGB, DIFF, LUMA, INDEX, RGBA, RUN), decoded
    /// as the specification says.
    #[test]
    fn a_qoi_thumbnail_is_read() {
        let mut qoi = b"qoif".to_vec();
        qoi.extend(7u32.to_be_bytes());
        qoi.extend(1u32.to_be_bytes());
        qoi.extend([4, 0]);
        qoi.extend([0xFE, 10, 20, 30]); // (10,20,30)
        qoi.push(0x79); // DIFF +1, 0, -1: (11,20,29)
        qoi.extend([0xA5, 0xA5]); // LUMA dg +5, dr-dg +2, db-dg -3: (18,25,31)
        qoi.push(0x09); // INDEX 9: (10,20,30), whose hash is 9
        qoi.extend([0xFF, 1, 2, 3, 4]); // RGBA
        qoi.push(0xC1); // RUN of 2
        qoi.extend([0, 0, 0, 0, 0, 0, 0, 1]);
        let png = to_png("QOI", &qoi).unwrap();
        assert_eq!(
            pixels(&png),
            [
                10, 20, 30, 255, 11, 20, 29, 255, 18, 25, 31, 255, 10, 20, 30, 255, 1, 2, 3, 4,
                1, 2, 3, 4, 1, 2, 3, 4
            ]
        );
    }

    /// QOI and JPG thumbnails in a text file, the largest kept (as PNG); in a binary file
    /// too (format 2: QOI).
    #[test]
    fn every_thumbnail_format_is_read() {
        let mut jpg = Vec::new();
        jpeg_encoder::Encoder::new(&mut jpg, 100)
            .encode(&[200u8; 16 * 16 * 3], 16, 16, jpeg_encoder::ColorType::Rgb)
            .unwrap();
        let mut qoi = b"qoif".to_vec();
        qoi.extend(2u32.to_be_bytes());
        qoi.extend(2u32.to_be_bytes());
        qoi.extend([4, 0, 0xFE, 1, 2, 3, 0xC2, 0, 0, 0, 0, 0, 0, 0, 1]);
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let head = format!(
            "; thumbnail_QOI begin 2x2 {}\n; {}\n; thumbnail_QOI end\n; thumbnail_JPG begin 16x16 {}\n; {}\n; thumbnail_JPG end\nM104 S215\n",
            qoi.len(),
            b64(&qoi),
            jpg.len(),
            b64(&jpg)
        );
        let (meta, png) = parse(&head, "");
        assert!(meta.thumbnail);
        let px = pixels(&png.unwrap());
        assert_eq!(px.len(), 16 * 16 * 3, "the larger, the JPG");
        assert!(px.iter().all(|p| (195..=205).contains(p)));
        // Binary G-code: header, a QOI thumbnail block, then the G-code block.
        let mut bgcode = b"GCDE".to_vec();
        bgcode.extend(1u32.to_le_bytes());
        bgcode.extend(0u16.to_le_bytes());
        bgcode.extend(5u16.to_le_bytes());
        bgcode.extend(0u16.to_le_bytes());
        bgcode.extend((qoi.len() as u32).to_le_bytes());
        bgcode.extend([2, 0, 2, 0, 2, 0]);
        bgcode.extend(&qoi);
        bgcode.extend(1u16.to_le_bytes());
        bgcode.extend(0u16.to_le_bytes());
        bgcode.extend(0u32.to_le_bytes());
        let path = std::env::temp_dir().join(format!("mc-thumb-{}.bgcode", std::process::id()));
        std::fs::write(&path, &bgcode).unwrap();
        let (meta, png) = read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(meta.thumbnail);
        assert_eq!(pixels(&png.unwrap())[..4], [1, 2, 3, 255]);
    }

    /// The rest of what PrusaSlicer writes, as gcode-metadata reads it, and a file for
    /// several tools (MMU): used filament summed, each kind of filament once.
    #[test]
    fn the_rest_of_prusaslicers_keys() {
        let tail = "; filament used [g] = 1.20, 3.40, 0.00\n; filament used [mm] = 400.0, 1100.0, 0.0\n; filament_type = PLA;PETG;PLA\n; temperature = 215,240,215\n; bed_temperature = 60,80,60\n; extruder_colour = #FF8000;#000000;#FFFFFF\n; nozzle_high_flow = 0,0,0\n; filament_abrasive = 1,0,0\n; fill_density = 15%\n; support_material = none\n; objects_info = {\"objects\":[{\"name\":\"Shape-Box\",\"polygon\":[[1,2]]},{\"name\":\"benchy.stl\",\"polygon\":[]}]}\n";
        let (meta, _) = parse("M104 S215\n", tail);
        assert!((meta.filament_g.unwrap() - 4.6).abs() < 1e-9);
        assert!((meta.filament_m.unwrap() - 1.5).abs() < 1e-9);
        assert_eq!(meta.filament_type.as_deref(), Some("PLA, PETG"));
        assert_eq!((meta.nozzle_temp_c, meta.bed_temp_c), (Some(215.0), Some(60.0)));
        assert_eq!(meta.colour.as_deref(), Some("#FF8000"));
        assert_eq!((meta.high_flow, meta.abrasive), (Some(false), Some(true)));
        assert_eq!(meta.fill_density.as_deref(), Some("15%"));
        assert_eq!(meta.supports, Some(false));
        assert_eq!(meta.objects, ["Shape-Box", "benchy.stl"]);
        let (meta, _) = parse("", "; support_material = everywhere\n");
        assert_eq!(meta.supports, Some(true));
        let (meta, _) = parse("", "; support_material = 1\n");
        assert_eq!(meta.supports, Some(true));
    }

    /// What a file's name tells, as PrusaSlicer names files: also with the dots turned into
    /// underscores, as on the MINI's stick.
    #[test]
    fn a_name_tells_what_the_file_does_not() {
        let mut meta = FileMeta::default();
        from_name(&mut meta, "sh_bn_0.6n_0.32mm_PETG_MK4_8h55m.gcode");
        assert_eq!((meta.nozzle_mm, meta.layer_height_mm), (Some(0.6), Some(0.32)));
        assert_eq!(meta.filament_type.as_deref(), Some("PETG"));
        assert_eq!(meta.printer_model.as_deref(), Some("MK4"));
        assert_eq!(meta.estimated_s, Some(8 * 3600 + 55 * 60));
        let mut meta = FileMeta::default();
        from_name(&mut meta, "3dbenchy_0_4n_0_2mm_PETG_MINIIS_1h3m.bgcode");
        assert_eq!((meta.nozzle_mm, meta.layer_height_mm), (Some(0.4), Some(0.2)));
        assert_eq!(meta.printer_model.as_deref(), Some("MINIIS"));
        assert_eq!(meta.estimated_s, Some(3600 + 3 * 60));
        let mut meta = FileMeta::default();
        from_name(&mut meta, "HP_PLA,PLA_MK3SMMU3_1d1h42m.gcode");
        assert_eq!(meta.printer_model.as_deref(), Some("MK3SMMU3"));
        assert_eq!(meta.estimated_s, Some(86_400 + 3600 + 42 * 60));
        // What the contents said stays.
        let mut meta = FileMeta {
            filament_type: Some("ASA".into()),
            ..FileMeta::default()
        };
        from_name(&mut meta, "x_PLA_MK4_1h.gcode");
        assert_eq!(meta.filament_type.as_deref(), Some("ASA"));
        let mut meta = FileMeta::default();
        from_name(&mut meta, "42.gcode");
        assert_eq!(meta, FileMeta::default());
    }

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
