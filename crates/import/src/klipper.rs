//! Makes machine profiles from Klipper's printer configurations
//! (<https://github.com/Klipper3d/klipper/tree/master/config>, GPL-3.0): the `printer-*.cfg`
//! files only, one per printer as sold (the boards and examples beside them are not
//! printers). Only facts are taken (sizes, temperatures, how it moves).
//!
//! ```sh
//! git clone --depth 1 --filter=blob:none --sparse https://github.com/Klipper3d/klipper.git reference/klipper
//! git -C reference/klipper sparse-checkout set config
//! cargo run -p maker-connect-import -- klipper reference/klipper profiles
//! ```
//!
//! Only the printers in [`PRINTERS`] are made: the others in Klipper's folder have a profile
//! already (from Marlin's examples, or by hand), or are a variant of one (another board,
//! another tool head). Their file names do not give a model's name as its maker writes it,
//! so the table does. A profile made here says `source = "klipper-config@<commit>"` and is
//! made again on every run; one without that line is left alone.

use super::{Profile, slug};
use std::collections::BTreeMap;
use std::path::Path;

/// Klipper's file (without `printer-` and the year), and the brand and model it is.
const PRINTERS: [(&str, &str, &str); 37] = [
    ("alfawise-u30", "Alfawise", "U30"),
    ("anet-a4", "Anet", "A4"),
    ("anycubic-4max", "AnyCubic", "4Max"),
    ("anycubic-4maxpro-2.0", "AnyCubic", "4Max Pro 2.0"),
    ("anycubic-kobra-go", "AnyCubic", "Kobra Go"),
    ("anycubic-kobra-plus", "AnyCubic", "Kobra Plus"),
    ("anycubic-kossel", "AnyCubic", "Kossel"),
    ("anycubic-kossel-plus", "AnyCubic", "Kossel Plus"),
    (
        "artillery-sidewinder-x3-plus",
        "Artillery",
        "Sidewinder X3 Plus",
    ),
    ("creality-cr10-smart-pro", "Creality", "CR-10 Smart Pro"),
    ("creality-cr5pro-ht", "Creality", "CR-5 Pro HT"),
    ("eryone-er20", "Eryone", "ER-20"),
    ("flsun-q5", "FLSUN", "Q5"),
    ("flsun-qqs", "FLSUN", "QQ-S"),
    ("fokoos-odin5-f3", "Fokoos", "Odin-5 F3"),
    ("geeetech-301", "Geeetech", "301"),
    ("hiprecy-leo", "Hiprecy", "Leo"),
    ("longer-lk4-pro", "Longer", "LK4 Pro"),
    ("longer-lk4x", "Longer", "LK4 X"),
    ("lulzbot-mini1", "LulzBot", "Mini"),
    ("lulzbot-mini2", "LulzBot", "Mini 2"),
    ("lulzbot-taz6", "LulzBot", "TAZ 6"),
    ("micromake-d1", "Micromake", "D1"),
    ("monoprice-mini-delta", "Monoprice", "MP Mini Delta"),
    ("monoprice-select-mini-v1", "Monoprice", "MP Select Mini V1"),
    ("monoprice-select-mini-v2", "Monoprice", "MP Select Mini V2"),
    ("mtw-create", "MTW", "Create"),
    ("ratrig-v-minion", "RatRig", "V-Minion"),
    ("robo3d-r2", "Robo3D", "R2"),
    ("seemecnc-rostock-max-v2", "SeeMeCNC", "Rostock MAX v2"),
    ("tevo-flash", "Tevo", "Flash"),
    ("tronxy-crux1", "Tronxy", "Crux 1"),
    ("tronxy-p802e", "Tronxy", "P802E"),
    ("tronxy-p802m", "Tronxy", "P802M"),
    ("tronxy-x8", "Tronxy", "X8"),
    ("velleman-k8800", "Velleman", "K8800"),
    ("wanhao-duplicator-i3-v2.1", "Wanhao", "Duplicator i3 v2.1"),
];

/// A config's sections and their settings: `[stepper_x]` → `position_max` → `235`. Comments
/// (`#`, `;`) and indented continuation lines (G-code macros) are left out.
pub fn sections(text: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut found: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut section = String::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let line = line.split(['#', ';']).next().unwrap_or("").trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_owned();
            found.entry(section.clone()).or_default();
        } else if let Some((key, value)) = line.split_once([':', '='])
            && !section.is_empty()
        {
            found
                .entry(section.clone())
                .or_default()
                .entry(key.trim().to_owned())
                .or_insert_with(|| value.trim().to_owned());
        }
    }
    found
}

/// A printer's config as a profile, or None without a size.
pub fn profile(text: &str, brand: &str, model: &str, source: &str) -> Option<Profile> {
    let s = sections(text);
    let get = |section: &str, key: &str| -> Option<f64> { s.get(section)?.get(key)?.parse().ok() };
    let kinematics = s
        .get("printer")
        .and_then(|p| p.get("kinematics"))
        .map_or("cartesian", String::as_str);
    let (x, y, z) = if kinematics == "delta" {
        let radius = get("printer", "print_radius")
            .or_else(|| get("delta_calibrate", "radius"))
            .or_else(|| get("printer", "delta_radius"))?;
        (
            2.0 * radius,
            2.0 * radius,
            get("stepper_a", "position_endstop")?,
        )
    } else {
        (
            get("stepper_x", "position_max")?,
            get("stepper_y", "position_max")?,
            get("stepper_z", "position_max").unwrap_or(0.0),
        )
    };
    if x <= 0.0 || y <= 0.0 {
        return None;
    }
    // Klipper's max_temp is where it shuts the heater off: the usable top is under it, as
    // with Marlin's.
    let nozzle = get("extruder", "max_temp").map(|t| t - 15.0);
    let bed = get("heater_bed", "max_temp").map(|t| t - 10.0);
    let extruders = (1..8)
        .take_while(|n| s.contains_key(&format!("extruder{n}")))
        .count()
        + 1;
    Some(Profile {
        brand: brand.to_owned(),
        model: model.to_owned(),
        kind: "fdm",
        build_volume_mm: [x, y, z],
        bed_origin_mm: [0.0, 0.0],
        firmware: "klipper",
        max_nozzle_c: nozzle.filter(|t| *t > 0.0),
        max_bed_c: bed.filter(|t| *t > 0.0),
        extruders: (extruders > 1).then_some(extruders as u32),
        firmware_version: String::new(),
        motion: match kinematics {
            "delta" => "delta",
            "corexy" | "corexz" | "hybrid_corexy" | "hybrid_corexz" => "corexy",
            _ => "bedslinger",
        },
        source: source.to_owned(),
        detect: None,
    })
}

/// Klipper's file name without `printer-`, its year and `.cfg`:
/// `printer-creality-cr5pro-ht-2022.cfg` → `creality-cr5pro-ht`.
fn stem(file: &str) -> Option<&str> {
    let name = file.strip_prefix("printer-")?.strip_suffix(".cfg")?;
    Some(match name.rsplit_once('-') {
        Some((rest, year)) if year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()) => rest,
        _ => name,
    })
}

pub fn run(repo: &Path, out: &Path) -> Result<(), String> {
    let config = repo.join("config");
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .current_dir(repo)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let source = format!("klipper-config@{commit}");
    let mut files: Vec<String> = std::fs::read_dir(&config)
        .map_err(|e| format!("{}: {e}", config.display()))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("printer-") && n.ends_with(".cfg"))
        .collect();
    // The newest year last, so it wins when a printer has several.
    files.sort();
    let mut chosen: BTreeMap<&str, String> = BTreeMap::new();
    for file in &files {
        if let Some(stem) = stem(file)
            && PRINTERS.iter().any(|(s, _, _)| *s == stem)
        {
            chosen.insert(
                PRINTERS.iter().find(|(s, _, _)| *s == stem).unwrap().0,
                file.clone(),
            );
        }
    }
    let (mut written, mut kept, mut missing) = (0, 0, Vec::new());
    for (stem, brand, model) in PRINTERS {
        let Some(file) = chosen.get(stem) else {
            missing.push(stem);
            continue;
        };
        let text = std::fs::read_to_string(config.join(file)).map_err(|e| e.to_string())?;
        let Some(profile) = profile(&text, brand, model, &source) else {
            missing.push(stem);
            continue;
        };
        let path = out.join(slug(brand)).join(format!("{}.toml", slug(model)));
        if let Ok(existing) = std::fs::read_to_string(&path)
            && !existing.contains("source = \"klipper-config@")
        {
            kept += 1;
            continue;
        }
        let body = toml::to_string(&profile).map_err(|e| e.to_string())?;
        let text = format!(
            "# Made from Klipper's printer configuration ({file}) by\n\
             # `cargo run -p maker-connect-import`: made again on each run. Delete the source\n\
             # line to edit it by hand and keep it.\n{body}"
        );
        std::fs::create_dir_all(path.parent().unwrap_or(out)).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        written += 1;
    }
    println!(
        "{} printer configurations: {written} profiles written, {kept} made otherwise left alone ({source})",
        files.len()
    );
    if !missing.is_empty() {
        println!("not found or without a size: {}", missing.join(", "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_klipper_config_becomes_a_profile() {
        let text = "# Ender\n[stepper_x]\nposition_max: 235\nposition_min: -5\n[stepper_y]\nposition_max = 235 # bed\n[stepper_z]\nposition_max: 250\n[extruder]\nmax_temp: 250\n[heater_bed]\nmax_temp: 130\n[printer]\nkinematics: corexy\n[gcode_macro START]\ngcode:\n  G28\n  M104 S200\n";
        let p = profile(text, "Creality", "Ender", "klipper-config@test").unwrap();
        assert_eq!(p.build_volume_mm, [235.0, 235.0, 250.0]);
        assert_eq!((p.max_nozzle_c, p.max_bed_c), (Some(235.0), Some(120.0)));
        assert_eq!(p.motion, "corexy");
        assert_eq!(p.extruders, None);
        let delta = "[printer]\nkinematics: delta\nprint_radius: 125\n[stepper_a]\nposition_endstop: 370\n[extruder]\nmax_temp: 260\n";
        let p = profile(delta, "FLSUN", "QQ-S", "x").unwrap();
        assert_eq!(p.build_volume_mm, [250.0, 250.0, 370.0]);
        assert_eq!(p.motion, "delta");
        assert_eq!(
            stem("printer-creality-cr5pro-ht-2022.cfg"),
            Some("creality-cr5pro-ht")
        );
        assert_eq!(stem("printer-adimlab-2018.cfg"), Some("adimlab"));
    }
}
