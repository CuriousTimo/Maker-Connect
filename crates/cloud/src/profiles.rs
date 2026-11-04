//! Machine profiles: what Maker Connect knows of a machine model (its build volume, its
//! bed) and how it recognises one from what the machine says about itself. One TOML file
//! per model in the repository's `profiles/` folder, by brand, built into the binary so the
//! cloud stays one file to install. Later: community profiles.

use include_dir::{Dir, include_dir};
use maker_connect_protocol::{Identity, MachineKind};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

static FILES: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../profiles");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// `brand/model`, from its file: `prusa/mk3s`.
    #[serde(default)]
    pub id: String,
    pub brand: String,
    pub model: String,
    pub kind: MachineKind,
    /// X, Y, Z.
    pub build_volume_mm: [f64; 3],
    /// Where X0 Y0 is, from the bed's front left corner.
    #[serde(default)]
    pub bed_origin_mm: [f64; 2],
    #[serde(default)]
    pub nozzle_mm: Option<f64>,
    #[serde(default)]
    pub firmware: Option<String>,
    #[serde(default)]
    pub max_nozzle_c: Option<f64>,
    #[serde(default)]
    pub max_bed_c: Option<f64>,
    /// How many nozzles it prints with.
    #[serde(default)]
    pub extruders: Option<u32>,
    /// A laser's optical power.
    #[serde(default)]
    pub laser_w: Option<f64>,
    #[serde(default)]
    pub max_speed_mm_min: Option<f64>,
    /// The slicer's names for this machine (PrusaSlicer's `printer_model`), to tell whether
    /// a file was sliced for it.
    #[serde(default)]
    pub printer_models: Vec<String>,
    #[serde(default)]
    pub detect: Option<Detect>,
}

impl Profile {
    /// Its picture (`profiles/<brand>/<model>.svg`), when there is one.
    pub fn image(&self) -> Option<&'static str> {
        FILES
            .get_file(format!("{}.svg", self.id))
            .and_then(|f| f.contents_utf8())
    }

    /// "250 × 210 × 210 mm", or for a machine without Z travel "400 × 430 mm".
    pub fn size(&self) -> String {
        match self.build_volume_mm {
            [x, y, z] if z > 0.0 => format!("{x} × {y} × {z} mm"),
            [x, y, _] => format!("{x} × {y} mm"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Detect {
    /// A Prusa's model code (`M862.2 Q`).
    #[serde(default)]
    pub model_code: Option<u32>,
    /// M115's `MACHINE_TYPE`.
    #[serde(default)]
    pub machine_type: Option<String>,
    /// A Buddy-firmware Prusa's printer type, as it registers with Prusa Connect.
    #[serde(default)]
    pub printer_type: Option<String>,
}

/// Every profile, by brand and model.
pub fn all() -> &'static [Profile] {
    static PROFILES: OnceLock<Vec<Profile>> = OnceLock::new();
    PROFILES.get_or_init(|| {
        let mut profiles: Vec<Profile> = toml_files(&FILES)
            .into_iter()
            .map(|file| {
                let path = file.path().to_string_lossy().replace('\\', "/");
                let text = file.contents_utf8().unwrap_or_default();
                let mut profile: Profile =
                    toml::from_str(text).unwrap_or_else(|error| panic!("profiles/{path}: {error}"));
                profile.id = path.trim_end_matches(".toml").to_owned();
                profile
            })
            .collect();
        profiles.sort_by(|a, b| (&a.brand, &a.model).cmp(&(&b.brand, &b.model)));
        profiles
    })
}

/// Every TOML file in a folder and the folders in it.
fn toml_files<'a>(dir: &'a Dir<'a>) -> Vec<&'a include_dir::File<'a>> {
    let mut files: Vec<_> = dir
        .files()
        .filter(|f| f.path().extension().is_some_and(|e| e == "toml"))
        .collect();
    for sub in dir.dirs() {
        files.extend(toml_files(sub));
    }
    files
}

pub fn find(id: &str) -> Option<&'static Profile> {
    all().iter().find(|p| p.id == id)
}

/// The profile a machine is, from what it says: its model code first (exact), then its
/// machine type. None when nothing matches: the owner chooses.
pub fn detect(identity: &Identity) -> Option<&'static Profile> {
    let detects = || all().iter().filter_map(|p| Some((p, p.detect.as_ref()?)));
    if let Some(code) = identity.model_code
        && let Some((profile, _)) = detects().find(|(_, d)| d.model_code == Some(code))
    {
        return Some(profile);
    }
    if let Some(printer_type) = identity.printer_type.as_deref()
        && let Some((profile, _)) =
            detects().find(|(_, d)| d.printer_type.as_deref() == Some(printer_type))
    {
        return Some(profile);
    }
    let machine_type = identity.machine_type.as_deref()?.trim();
    detects()
        .find(|(_, d)| {
            d.machine_type
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case(machine_type))
        })
        .map(|(profile, _)| profile)
}

/// The bed, drawn to scale: the build plate with a grid every 10 mm, a stronger line and
/// a label every 50 mm, and X0 Y0 marked. Millimetres are SVG units; Y runs up, as on the
/// machine, so the front of the bed is at the bottom.
pub fn bed_svg(profile: &Profile) -> String {
    let [width, depth, _] = profile.build_volume_mm;
    let flip = |y: f64| depth - y;
    let mut svg = format!(
        r#"<svg class="bed" xmlns="http://www.w3.org/2000/svg" viewBox="-16 -6 {} {}" role="img" aria-label="Bed of {}: {width} by {depth} mm" data-depth="{depth}">"#,
        width + 24.0,
        depth + 22.0,
        escape(&profile.model),
    );
    svg += &format!(r#"<rect class="plate" x="0" y="0" width="{width}" height="{depth}" rx="2"/>"#);
    let mut x = 10.0;
    while x < width {
        let class = if x % 50.0 == 0.0 { "major" } else { "minor" };
        svg += &format!(r#"<line class="{class}" x1="{x}" y1="0" x2="{x}" y2="{depth}"/>"#);
        x += 10.0;
    }
    let mut y = 10.0;
    while y < depth {
        let class = if y % 50.0 == 0.0 { "major" } else { "minor" };
        svg += &format!(
            r#"<line class="{class}" x1="0" y1="{}" x2="{width}" y2="{}"/>"#,
            flip(y),
            flip(y)
        );
        y += 10.0;
    }
    let mut x = 0.0;
    while x <= width {
        svg += &format!(
            r#"<text x="{x}" y="{}" text-anchor="middle">{x}</text>"#,
            depth + 9.0
        );
        x += 50.0;
    }
    let mut y = 50.0;
    while y <= depth {
        svg += &format!(
            r#"<text x="-4" y="{}" text-anchor="end">{y}</text>"#,
            flip(y) + 2.5
        );
        y += 50.0;
    }
    // X0 Y0, small; and the head where the machine says it is (the page moves it live).
    let [ox, oy] = profile.bed_origin_mm;
    let oy = flip(oy);
    svg += &format!(
        r#"<path class="origin" d="M{} {oy}h6M{ox} {}v6"/>"#,
        ox - 3.0,
        oy - 3.0
    );
    svg += r#"<g class="head" visibility="hidden"><circle r="7" class="halo"/><circle r="3.5"/><title>The head</title></g></svg>"#;
    svg
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profiles_load() {
        let ids: Vec<&str> = all().iter().map(|p| p.id.as_str()).collect();
        for id in ["prusa/mini", "prusa/mk2.5s", "prusa/mk3s", "zaribo/320"] {
            assert!(ids.contains(&id), "{ids:?}");
        }
        assert_eq!(
            find("prusa/mini").unwrap().build_volume_mm,
            [180.0, 180.0, 180.0]
        );
    }

    /// A MINI and an MK3S are recognised by their model code; a machine type alone is
    /// enough too; a Zaribo, which reports itself as an MK3S, is chosen by hand.
    #[test]
    fn a_machine_is_recognised() {
        let by_code = |code| {
            detect(&Identity {
                model_code: Some(code),
                ..Identity::default()
            })
            .map(|p| p.id.as_str())
        };
        assert_eq!(by_code(120), Some("prusa/mini"));
        assert_eq!(by_code(302), Some("prusa/mk3s"));
        assert_eq!(by_code(999), None);
        let by_type = detect(&Identity {
            machine_type: Some("Prusa-MINI".into()),
            ..Identity::default()
        });
        assert_eq!(by_type.map(|p| p.id.as_str()), Some("prusa/mini"));
        let by_connect_type = detect(&Identity {
            printer_type: Some("2.1.0".into()),
            ..Identity::default()
        });
        assert_eq!(by_connect_type.map(|p| p.id.as_str()), Some("prusa/mini"));
        assert!(find("zaribo/320").unwrap().detect.is_none());
    }

    #[test]
    fn the_bed_is_drawn_to_scale() {
        let svg = bed_svg(find("prusa/mini").unwrap());
        assert!(svg.contains(r#"width="180" height="180""#));
        assert!(svg.contains(">150</text>"), "labels every 50 mm");
        assert_eq!(
            svg.matches(r#"class="major""#).count(),
            6,
            "50, 100, 150 both ways"
        );
    }
}
