//! Makes machine profiles from Marlin's example configurations
//! (<https://github.com/MarlinFirmware/Configurations>, GPL-3.0): one per machine model, in
//! `profiles/<brand>/<model>.toml`. Only facts are taken (sizes, temperatures, the name the
//! printer gives itself); the profiles are our own files.
//!
//! ```sh
//! git clone --depth 1 --branch import-2.1.x \
//!     https://github.com/MarlinFirmware/Configurations.git reference/marlin-configurations
//! cargo run -p maker-connect-import -- marlin reference/marlin-configurations profiles
//! ```
//!
//! A profile made here says `source = "marlin-configurations@<commit>"`, and is made again on
//! every run. A profile without that line was made by hand (or taken over by deleting the
//! line): it is left alone. Variants of a machine (other boards, upgrades) become one
//! profile: the one at the shortest path, the machine as sold.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod prusaslicer;

/// Folders of examples that are not machines to offer: tests and demos.
const SKIP: [&str; 5] = [
    "AnimationExample",
    "linear_axes",
    "FoamCutter",
    "SCARA",
    "Simulator",
];
/// Folders that group machines by how they move, not by brand: the brand is the next one.
const GROUPS: [&str; 2] = ["delta", "Hangprinter"];

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct Profile {
    brand: String,
    model: String,
    kind: &'static str,
    build_volume_mm: [f64; 3],
    bed_origin_mm: [f64; 2],
    firmware: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_nozzle_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_bed_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extruders: Option<u32>,
    /// The firmware version its configuration is for: `2.1.3`.
    firmware_version: String,
    /// How it moves: `bedslinger`, `corexy`, `delta`: for its drawing.
    motion: &'static str,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detect: Option<Detect>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct Detect {
    /// What it says to M115 (MACHINE_TYPE): the CUSTOM_MACHINE_NAMEs of its variants.
    machine_types: Vec<String>,
}

/// A model's folder without the board it was set up for: `MK3S-BigTreeTech-BTT002` → `MK3S`.
fn without_board(name: &str) -> String {
    const BOARDS: [&str; 6] = ["BigTreeTech", "BTT", "SKR", "MKS", "Creality V4", "Melzi"];
    let mut cut = name.len();
    for board in BOARDS {
        for sep in [" ", "-", "_", " ("] {
            if let Some(at) = name.find(&format!("{sep}{board}")) {
                cut = cut.min(at);
            }
        }
    }
    name[..cut].trim().to_owned()
}

/// Marlin's configuration version as a version: `02010300` → `2.1.3`.
fn version(value: Option<&String>) -> Option<String> {
    let digits = value?.trim();
    if digits.len() != 8 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let part = |i: usize| digits[i..i + 2].parse::<u32>().ok();
    Some(format!("{}.{}.{}", part(0)?, part(2)?, part(4)?))
}

/// The `#define`s of a configuration file that are switched on, with their values (a
/// define with none has ""). The first of each wins: later ones sit in `#if` branches.
fn defines(text: &str) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("#define") else {
            continue;
        };
        let rest = rest.split("//").next().unwrap_or("").trim();
        let mut parts = rest.splitn(2, char::is_whitespace);
        let Some(name) = parts.next().filter(|n| !n.is_empty()) else {
            continue;
        };
        let value = parts.next().unwrap_or("").trim().to_owned();
        found.entry(name.to_owned()).or_insert(value);
    }
    found
}

/// A number from a define's value: `235`, `250.00`, `(235)`, `-20`.
fn number(value: Option<&String>) -> Option<f64> {
    value?
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim()
        .parse()
        .ok()
}

/// A name as a file name: `Ender-3 Neo` → `ender-3-neo`.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

/// Every `Configuration.h` under a folder.
fn configurations(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            configurations(&path, found);
        } else if path.file_name().is_some_and(|n| n == "Configuration.h") {
            found.push(path);
        }
    }
}

/// One example as a profile, or None when it is not one to offer (no bed size, a test).
fn profile(examples: &Path, path: &Path, source: &str) -> Option<Profile> {
    let relative = path.parent()?.strip_prefix(examples).ok()?;
    let parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.iter().any(|p| SKIP.contains(&p.as_str())) {
        return None;
    }
    let brand_at = usize::from(GROUPS.contains(&parts.first()?.as_str()));
    let brand = parts.get(brand_at)?.clone();
    let config = defines(&std::fs::read_to_string(path).ok()?);
    let adv = defines(
        &std::fs::read_to_string(path.with_file_name("Configuration_adv.h")).unwrap_or_default(),
    );
    let on = |name: &str| config.contains_key(name) || adv.contains_key(name);
    let named = config
        .get("CUSTOM_MACHINE_NAME")
        .map(|n| n.trim_matches('"').trim().to_owned())
        .filter(|n| !n.is_empty());
    // The model: its folder under the brand (deeper folders are its variants: boards,
    // upgrades), without a board in its name.
    let model = without_board(parts.get(brand_at + 1)?);
    if model.is_empty() {
        return None;
    }
    let delta = on("DELTA");
    let (x, y) = if delta {
        let r = number(config.get("DELTA_PRINTABLE_RADIUS"))?;
        (2.0 * r, 2.0 * r)
    } else {
        (
            number(config.get("X_BED_SIZE"))?,
            number(config.get("Y_BED_SIZE"))?,
        )
    };
    let z = number(config.get("Z_MAX_POS"))
        .or_else(|| number(config.get("DELTA_HEIGHT")))
        .unwrap_or(0.0);
    if x <= 0.0 || y <= 0.0 {
        return None;
    }
    let kind = if on("LASER_FEATURE") {
        "laser"
    } else if on("SPINDLE_FEATURE") {
        "cnc"
    } else {
        "fdm"
    };
    // Marlin's limits are where it shuts the heater off: the usable top is under it.
    let nozzle = number(config.get("HEATER_0_MAXTEMP")).map(|t| t - 15.0);
    let bed = number(config.get("BED_MAXTEMP")).map(|t| t - 10.0);
    Some(Profile {
        brand: brand.replace('_', " "),
        model,
        kind,
        build_volume_mm: [x, y, z],
        bed_origin_mm: [0.0, 0.0],
        firmware: "marlin",
        max_nozzle_c: nozzle.filter(|t| *t > 0.0),
        max_bed_c: bed.filter(|t| *t > 0.0),
        extruders: number(config.get("EXTRUDERS")).map(|e| e as u32),
        firmware_version: version(config.get("CONFIGURATION_H_VERSION")).unwrap_or_default(),
        motion: if delta {
            "delta"
        } else if on("COREXY") || on("COREXZ") || on("MARKFORGED_XY") {
            "corexy"
        } else {
            "bedslinger"
        },
        source: source.to_owned(),
        detect: named.map(|name| Detect {
            machine_types: vec![name],
        }),
    })
}

fn run(repo: &Path, out: &Path) -> Result<(), String> {
    let examples = repo.join("config/examples");
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .current_dir(repo)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let source = format!("marlin-configurations@{commit}");
    let mut paths = Vec::new();
    configurations(&examples, &mut paths);
    // The machine as sold first: the shortest path of a model wins over its variants.
    paths.sort_by_key(|p| (p.components().count(), p.clone()));
    let mut chosen: BTreeMap<(String, String), (Profile, usize)> = BTreeMap::new();
    for path in &paths {
        if let Some(p) = profile(&examples, path, &source) {
            let key = (slug(&p.brand), slug(&p.model));
            let names = p
                .detect
                .as_ref()
                .map(|d| d.machine_types.clone())
                .unwrap_or_default();
            let entry = chosen.entry(key).or_insert((p, 0));
            entry.1 += 1;
            // Every name its variants give themselves recognises it.
            for name in names {
                let detect = entry.0.detect.get_or_insert(Detect {
                    machine_types: Vec::new(),
                });
                if !detect.machine_types.contains(&name) {
                    detect.machine_types.push(name);
                }
            }
        }
    }
    let (mut written, mut kept) = (0, 0);
    for ((brand, model), (profile, variants)) in &chosen {
        let file = out.join(brand).join(format!("{model}.toml"));
        // Made by hand (no source line): left alone.
        if let Ok(existing) = std::fs::read_to_string(&file)
            && !existing.contains("source = \"marlin-configurations@")
        {
            kept += 1;
            continue;
        }
        let body = toml::to_string(profile).map_err(|e| e.to_string())?;
        let note = if *variants > 1 {
            format!(" ({variants} variants in Marlin's examples: other boards, upgrades)")
        } else {
            String::new()
        };
        let text = format!(
            "# Made from Marlin's example configurations{note} by\n\
             # `cargo run -p maker-connect-import`: made again on each run. Delete the source\n\
             # line to edit it by hand and keep it.\n{body}"
        );
        std::fs::create_dir_all(file.parent().unwrap_or(out)).map_err(|e| e.to_string())?;
        std::fs::write(&file, text).map_err(|e| e.to_string())?;
        written += 1;
    }
    println!(
        "{} configurations, {} machines: {written} profiles written, {kept} made by hand left alone ({source})",
        paths.len(),
        chosen.len()
    );
    Ok(())
}

/// Every profile in a folder, as a Markdown page of supported machines by brand (for the
/// wiki): model, kind, size, firmware and where it came from.
fn list(profiles: &Path, out: &Path) -> Result<(), String> {
    fn tomls(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                tomls(&path, found);
            } else if path.extension().is_some_and(|e| e == "toml") {
                found.push(path);
            }
        }
    }
    let mut files = Vec::new();
    tomls(profiles, &mut files);
    let mut rows: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut marlin = None;
    for file in &files {
        let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
        let Ok(v) = text.parse::<toml::Table>() else {
            continue;
        };
        let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_owned();
        let kind = match get("kind").as_str() {
            "fdm" => "3D printer",
            "laser" => "Laser",
            "label" => "Label printer",
            "cnc" => "CNC",
            other => other,
        }
        .to_owned();
        let size = if get("kind") == "label" {
            let mut widths: Vec<i64> = v
                .get("media")
                .and_then(|m| m.as_array())
                .map(|m| {
                    m.iter()
                        .filter_map(|x| x.get("width_mm")?.as_integer())
                        .collect()
                })
                .unwrap_or_default();
            widths.sort_unstable();
            widths.dedup();
            let widths: Vec<String> = widths.iter().map(i64::to_string).collect();
            format!("{} mm tape", widths.join(", "))
        } else {
            let n: Vec<f64> = v
                .get("build_volume_mm")
                .and_then(|b| b.as_array())
                .map(|b| {
                    b.iter()
                        .filter_map(|x| x.as_float().or(x.as_integer().map(|i| i as f64)))
                        .collect()
                })
                .unwrap_or_default();
            match n.as_slice() {
                [x, y, z] if *z > 0.0 => format!("{x:.0} × {y:.0} × {z:.0} mm"),
                [x, y, _] => format!("{x:.0} × {y:.0} mm"),
                _ => String::new(),
            }
        };
        let firmware = match (get("firmware").as_str(), get("firmware_version").as_str()) {
            ("", _) => String::new(),
            (f, "") => f.to_owned(),
            (f, version) => format!("{f} {version}"),
        };
        let source = get("source");
        let from = if source.starts_with("marlin-configurations@") {
            marlin = Some(source.clone());
            "Marlin's examples"
        } else {
            "by hand"
        };
        rows.entry(get("brand")).or_default().push((
            get("model"),
            format!(
                "| {} | {kind} | {size} | {firmware} | {from} |",
                get("model")
            ),
        ));
    }
    let count: usize = rows.values().map(Vec::len).sum();
    let mut page = format!(
        "## How they connect\n\n| Devices | How they connect | What works | On real hardware |\n|---|---|---|---|\n| **Prusa MINI** (Buddy firmware) | Its own firmware speaks Prusa Connect to the cloud: no agent | Registers with a code, reports, takes commands | Yes: a MINI on firmware 6.3.0 |\n| **3D printers on Marlin**: 245 models (Creality, Anycubic, Artillery, …) | Agent, on USB | Recognised from `M115`; live temperatures, moves, heating, bed mesh, terminal; **prints files** (pause, resume, stop) | Not yet: tested against simulated printers |\n| **Prusa on 8-bit firmware**: MK3S, MK2.5S | Agent, on USB or an MK3S's RPi port (a Pi Zero W) | As Marlin, recognised from `M862.2 Q` | Not yet: an MK3S with its Pi Zero W is the first to try |\n| **Lasers on GRBL**: Ortur Laser Master 2 Pro S2, other GRBL 1.1 | Agent, on USB, with LaserFoundry's device crate | Homing, jogs, settings checked, Frame, jobs streamed, pause, resume, stop, restart where it stopped | Not yet: tested against LaserFoundry's simulated GRBL |\n| **Label printers**: DYMO LabelManager PnP | Agent, on USB HID: no driver, no DYMO software | Labels from the editor (text, QR codes, barcodes, drawings), tapes, cassette and battery | Yes: from a Mac and a Windows laptop, 12 mm tape |\n| **A virtual one of every profile** | Simulated in the cloud | Printers home, heat, measure and print; lasers on simulated GRBL; label printers print | – |\n\n| The agent runs on | Build | On real hardware |\n|---|---|---|\n| macOS, Apple silicon | `macos-aarch64` | Yes |\n| Windows 10/11, x64 | `windows-x86_64.exe` | Yes |\n| Linux ARM64: Orange Pi Zero3, Raspberry Pi 4 and 5 | `linux-aarch64` | Not yet |\n| Linux ARM 32-bit: Raspberry Pi Zero W (an MK3S's RPi port), Pi 1 | `linux-armv6` | Not yet |\n| Linux x86-64: a PC, a NAS | `linux-x86_64` | Not yet |\n\n## The machines\n\nEvery machine Maker Connect has a profile for: {count} machines of {} brands. A machine is recognised from what it says when its agent connects (Marlin's `M115`, a Prusa's model code, a USB name), or its profile is chosen under **Settings → Profile**. A virtual one of each can be added to try it.\n\n\
         Most 3D printers come from [Marlin's example configurations](https://github.com/MarlinFirmware/Configurations) ({}); the others were made by hand. Made by `scripts/import-profiles.sh`.\n\n",
        rows.len(),
        marlin.as_deref().unwrap_or("not imported")
    );
    for (brand, mut machines) in rows {
        machines.sort_by(|a, b| a.0.cmp(&b.0));
        page += &format!(
            "### {brand}\n\n| Model | Kind | Size | Firmware | From |\n|---|---|---|---|---|\n"
        );
        for (_, row) in machines {
            page += &row;
            page += "\n";
        }
        page += "\n";
    }
    std::fs::write(out, page).map_err(|e| e.to_string())?;
    println!("{count} machines listed in {}", out.display());
    Ok(())
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = match args.as_slice() {
        [what, repo, out] if what == "marlin" => run(Path::new(repo), Path::new(out)),
        [what, profiles, out] if what == "list" => list(Path::new(profiles), Path::new(out)),
        [what, bundle, out] if what == "prusaslicer" => {
            prusaslicer::run(Path::new(bundle), Path::new(out))
        }
        _ => Err("usage: maker-connect-import marlin <Configurations repository> <profiles folder>\n       maker-connect-import list <profiles folder> <page.md>\n       maker-connect-import prusaslicer <vendor bundle .ini> <filaments/….toml>".to_owned()),
    };
    match done {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defines_are_read_as_marlin_has_them() {
        let text = "#define X_BED_SIZE 235  // the bed\n//#define COREXY\n  #define Z_MAX_POS 250\n#define CUSTOM_MACHINE_NAME \"Ender-3 Neo\"\n#if X\n#define X_BED_SIZE 300\n#endif\n#define DELTA";
        let d = defines(text);
        assert_eq!(d["X_BED_SIZE"], "235", "the first wins");
        assert_eq!(number(d.get("Z_MAX_POS")), Some(250.0));
        assert!(!d.contains_key("COREXY"), "commented out");
        assert_eq!(d["DELTA"], "");
        assert_eq!(slug("Ender-3 Neo"), "ender-3-neo");
        assert_eq!(slug("CR-10 V2 / BLTouch"), "cr-10-v2-bltouch");
        assert_eq!(without_board("MK3S-BigTreeTech-BTT002"), "MK3S");
        assert_eq!(without_board("Ender-3 S1"), "Ender-3 S1");
        assert_eq!(
            version(Some(&"02010300".to_owned())).as_deref(),
            Some("2.1.3")
        );
    }
}
