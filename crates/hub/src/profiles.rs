//! Machine profiles: what Maker Connect knows of a machine model (its build volume, its
//! bed) and how it recognises one from what the machine says about itself. One TOML file
//! per model in the repository's `profiles/` folder, by brand, built into the binary so the
//! hub stays one file to install. Later: community profiles.

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
    /// X, Y, Z; none for a label printer.
    #[serde(default)]
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
    /// A label printer's resolution and the pins on its head.
    #[serde(default)]
    pub dpi: Option<f64>,
    #[serde(default)]
    pub head_dots: Option<u32>,
    /// Whether it runs on a battery (the LabelManager PnP): its charge is shown. A
    /// LabelWriter is on mains power only.
    #[serde(default)]
    pub battery: bool,
    /// The media it takes (a label printer's tapes), the owner choosing which is loaded.
    #[serde(default)]
    pub media: Vec<Media>,
    /// HS1: the media come from a port's geometry instead of this file (`"lw450"`: the
    /// LabelWriter 450's rolls, `maker_connect_engine::ports::geometry::lw450`; `"lw320"`: the catalogue's
    /// rolls on the 320's PPD, `geometry::lw320`; `"lw400"`: the catalogue's rolls on the 400's PPD, `geometry::lw400`).
    #[serde(default)]
    pub media_from_port: Option<String>,
    /// The firmware version its data is for (`2.1.3` for Marlin's examples).
    #[serde(default)]
    pub firmware_version: Option<String>,
    /// How it moves: `bedslinger`, `corexy`, `delta`; for a drawing when it has none.
    #[serde(default)]
    pub motion: Option<String>,
    /// Where it was made from (`marlin-configurations@ef42fa5`), when not by hand.
    #[serde(default)]
    pub source: Option<String>,
}

/// A tape or roll a label printer takes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Media {
    /// The maker's article number, which is on the cassette (`45013`).
    pub id: String,
    pub name: String,
    /// Across the head: a tape's width, a label's width.
    pub width_mm: u32,
    /// A die-cut label's length along the roll; none for tape or a continuous roll.
    #[serde(default)]
    pub length_mm: Option<u32>,
    /// Die-cut labels on one roll, for Materials (a roll counts down label by label).
    #[serde(default)]
    pub per_roll: Option<u32>,
    /// The text and the tape: how the label looks (`#000`, `#fff`).
    #[serde(default)]
    pub ink: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
}

/// A profile from what a machine's controller says of itself (a GRBL laser's `$130` to
/// `$132`), for one without a profile: its work area, to draw it and check files.
pub fn from_controller(kind: MachineKind, travel_mm: [f64; 3]) -> Profile {
    Profile {
        id: String::new(),
        brand: String::new(),
        model: "From its controller".to_owned(),
        kind,
        build_volume_mm: travel_mm,
        bed_origin_mm: [0.0, 0.0],
        nozzle_mm: None,
        firmware: Some("grbl".to_owned()),
        max_nozzle_c: None,
        max_bed_c: None,
        extruders: None,
        laser_w: None,
        max_speed_mm_min: None,
        printer_models: Vec::new(),
        detect: None,
        dpi: None,
        head_dots: None,
        battery: false,
        media: Vec::new(),
        media_from_port: None,
        firmware_version: None,
        motion: None,
        source: None,
    }
}

/// A profile from what a Marlin printer's firmware says of itself (its `M115` answer,
/// `firmware_info`, extension part 9), for one without a profile, as [`from_controller`] for a
/// laser: its name and its extruders. M115 says nothing of the bed, so no build volume.
pub fn from_firmware(info: &serde_json::Map<String, serde_json::Value>) -> Profile {
    let field = |key: &str| info.get(key).and_then(serde_json::Value::as_str);
    let mut profile = from_controller(MachineKind::Fdm, [0.0; 3]);
    profile.model = field("MACHINE_TYPE")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("From its firmware")
        .to_owned();
    profile.firmware = Some("marlin".to_owned());
    profile.extruders = field("EXTRUDER_COUNT").and_then(|n| n.trim().parse().ok());
    profile
}

/// The drawing of a kind of machine, for one without a profile yet: a 3D printer, a laser,
/// a CNC machine.
pub fn generic_image(kind: MachineKind) -> Option<&'static str> {
    let name = match kind {
        MachineKind::Fdm => "bedslinger",
        MachineKind::Laser => "laser",
        MachineKind::Cnc => "cnc",
        _ => return None,
    };
    FILES
        .get_file(format!("_generic/{name}.svg"))
        .and_then(|f| f.contents_utf8())
}

impl Profile {
    /// Its picture (`profiles/<brand>/<model>.svg`), when there is one.
    /// Its own drawing, or one of its kind and how it moves (`profiles/_generic/`).
    pub fn image(&self) -> Option<&'static str> {
        let generic = match self.kind {
            MachineKind::Laser => "laser",
            MachineKind::Cnc => "cnc",
            _ => self.motion.as_deref().unwrap_or("bedslinger"),
        };
        FILES
            .get_file(format!("{}.svg", self.id))
            .or_else(|| FILES.get_file(format!("_generic/{generic}.svg")))
            .and_then(|f| f.contents_utf8())
    }

    /// "250 × 210 × 210 mm", or for a machine without Z travel "400 × 430 mm"; a label
    /// printer's tape widths ("6, 9, 12 mm tape").
    pub fn size(&self) -> String {
        if self.kind == MachineKind::Label {
            let mut widths: Vec<u32> = self.media.iter().map(|m| m.width_mm).collect();
            widths.sort_unstable();
            widths.dedup();
            let widths: Vec<String> = widths.iter().map(u32::to_string).collect();
            return format!("{} mm tape", widths.join(", "));
        }
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
    /// More names it may give itself (a model's variants, each with its own board).
    #[serde(default)]
    pub machine_types: Vec<String>,
    /// A Buddy-firmware Prusa's printer type, as it registers with Prusa Connect.
    #[serde(default)]
    pub printer_type: Option<String>,
    /// More printer types that are this model (the XL Development Kit is an XL).
    #[serde(default)]
    pub printer_types: Vec<String>,
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
                match profile.media_from_port.as_deref() {
                    None => {}
                    Some(port) if port_rolls(port).is_some() => {
                        profile.media = labelwriter_media(port);
                    }
                    Some(other) => panic!("profiles/{path}: no port geometry {other}"),
                }
                profile
            })
            .collect();
        profiles.sort_by(|a, b| (&a.brand, &a.model).cmp(&(&b.brand, &b.model)));
        profiles
    })
}

/// A LabelWriter's rolls by its port geometry's name: DYMO Connect's catalogue (HS2b) on the 450's and the
/// 320's PPD (#25), and on the 400's less its `ExcludedPapers` (#230).
fn port_rolls(port: &str) -> Option<Vec<maker_connect_engine::ports::geometry::CatalogRoll>> {
    use maker_connect_engine::ports::geometry::{lw320, lw400, lw450};
    match port {
        "lw450" => Some(lw450::rolls()),
        "lw320" => Some(lw320::rolls()),
        "lw400" => Some(lw400::rolls()),
        _ => None,
    }
}

/// A LabelWriter's rolls as media: one per SKU of DYMO Connect's catalogue (the number on the box; several
/// SKUs share a page). The job names the page
/// (`loaded_page_size`). The name is DYMO's own, plus the page's size in mm by the port's rounding.
fn labelwriter_media(port: &str) -> Vec<Media> {
    port_rolls(port)
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            let (w, l) = (r.page.width_mm, r.page.length_mm);
            Media {
                id: r.roll.sku.to_owned(),
                name: match r.roll.name {
                    Some(name) => format!("{name} · {w} × {l} mm"),
                    None => format!("{w} × {l} mm"),
                },
                width_mm: w,
                length_mm: Some(l),
                per_roll: None,
                ink: Some("#000".to_owned()),
                background: Some("#fff".to_owned()),
            }
        })
        .collect()
}

/// HS2b: a LabelWriter roll (by its medium id) as the port gives it.
pub fn labelwriter_roll(
    profile: Option<&Profile>,
    media: &str,
) -> Option<maker_connect_engine::ports::geometry::CatalogRoll> {
    port_rolls(profile?.media_from_port.as_deref()?)?
        .into_iter()
        .find(|r| r.roll.sku == media)
}

/// HS2: a LabelWriter roll's page as its port gives it (by the medium's id).
pub fn labelwriter_page(
    profile: Option<&Profile>,
    media: &str,
) -> Option<maker_connect_engine::ports::geometry::PageGeometry> {
    labelwriter_roll(profile, media).map(|r| r.page)
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

/// One of a profile's media, by its id.
pub fn media<'a>(profile: &'a Profile, id: &str) -> Option<&'a Media> {
    profile.media.iter().find(|m| m.id == id)
}

/// A medium's size in dots on its printer: across the head, and along it for a die-cut
/// label (its length is then fixed).
pub fn media_dots(profile: &Profile, media: &Media) -> (usize, Option<usize>) {
    use maker_connect_label::{dymo, labelwriter};
    if profile.firmware.as_deref() == Some("dymo-lw") {
        let across = labelwriter::dots_for_mm(media.width_mm as f64).min(labelwriter::HEAD_DOTS);
        let along = media.length_mm.map(|l| labelwriter::dots_for_mm(l as f64));
        (across, along)
    } else {
        (dymo::Tape::d1(media.width_mm).map_or(0, |t| t.dots), None)
    }
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
        && let Some((profile, _)) = detects().find(|(_, d)| {
            d.printer_type.as_deref() == Some(printer_type)
                || d.printer_types.iter().any(|t| t == printer_type)
        })
    {
        return Some(profile);
    }
    let machine_type = identity.machine_type.as_deref()?.trim();
    // A profile made by hand first, then one made from other data.
    let named = |d: &Detect| {
        d.machine_type
            .iter()
            .chain(&d.machine_types)
            .any(|t| t.eq_ignore_ascii_case(machine_type))
    };
    // One profile by that name, or none: a name several models share ("Copymaster 3D" for its
    // 300, 400 and 500) does not say which, so the space chooses on the Machine tab.
    let only = |made: bool| {
        let mut found = detects().filter(|(p, d)| p.source.is_none() == made && named(d));
        let first = found.next();
        match (first, found.next()) {
            (Some((profile, _)), None) => Some(Some(profile)),
            (Some(_), Some(_)) => Some(None),
            (None, _) => None,
        }
    };
    only(true).or_else(|| only(false)).flatten()
}

/// A laser by the `MODEL` of its `$I` answer (extension part 4, "What the laser is"; ours, as
/// `MACHINE_TYPE` in part 9): the one laser profile whose brand and model, the part in brackets
/// left off, are that name, case aside. None, or several: the space picks one by hand.
pub fn detect_laser(model: &str) -> Option<&'static Profile> {
    let model = model.trim();
    let name = |p: &Profile| {
        let base = p.model.split(" (").next().unwrap_or(&p.model).trim();
        format!("{} {base}", p.brand.trim())
    };
    let mut found = all()
        .iter()
        .filter(|p| p.kind == MachineKind::Laser && name(p).eq_ignore_ascii_case(model));
    match (found.next(), found.next()) {
        (Some(profile), None) => Some(profile),
        _ => None,
    }
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
    /// Prusa's whole range, MK2 to CORE One and XL: each told by what it says (its Connect
    /// type, its model code, M115's machine type), no code twice, and a file sliced for one
    /// fits it and not the others.
    #[test]
    fn the_whole_prusa_range() {
        use maker_connect_protocol::Identity;
        let by = |code: Option<u32>, printer_type: Option<&str>, machine_type: Option<&str>| {
            detect(&Identity {
                model_code: code,
                printer_type: printer_type.map(str::to_owned),
                machine_type: machine_type.map(str::to_owned),
                ..Identity::default()
            })
            .map(|p| p.id.as_str())
        };
        // Buddy, as each registers with Connect.
        for (printer_type, id) in [
            ("2.1.0", "prusa/mini"),
            ("1.3.5", "prusa/mk3.5"),
            ("1.3.6", "prusa/mk3.5s"),
            ("1.3.9", "prusa/mk3.9"),
            ("1.3.10", "prusa/mk3.9s"),
            ("1.4.0", "prusa/mk4"),
            ("1.4.1", "prusa/mk4s"),
            ("3.1.0", "prusa/xl"),
            ("3.1.1", "prusa/xl-plus"),
            ("7.1.0", "prusa/coreone"),
            ("8.1.0", "prusa/coreone-l"),
            ("7.10.0", "prusa/coreone-indx"),
            ("8.10.0", "prusa/coreone-l-indx"),
            ("7.2.0", "prusa/signature-oak"),
            ("4.1.0", "prusa/ix"),
            ("5.1.0", "prusa/xl"),
            // Prusa's Connect SDK (PrusaLink) for the MK3 family.
            ("1.3.0", "prusa/i3-mk3"),
            ("1.3.1", "prusa/mk3s"),
            ("1.2.5", "prusa/mk2.5"),
            ("1.2.6", "prusa/mk2.5s"),
        ] {
            assert_eq!(
                by(None, Some(printer_type), None),
                Some(id),
                "{printer_type}"
            );
        }
        // Prusa-Firmware over USB: M862.2's code, or M115's name.
        for (code, id) in [
            (200, "prusa/mk2"),
            (202, "prusa/mk2s"),
            (250, "prusa/mk2.5"),
            (252, "prusa/mk2.5s"),
            (300, "prusa/i3-mk3"),
            (302, "prusa/mk3s"),
            // Buddy's codes, every model in printer_model_data.hpp.
            (230, "prusa/mk3.5"),
            (280, "prusa/mk3.5s"),
            (210, "prusa/mk3.9"),
            (270, "prusa/mk3.9s"),
            (130, "prusa/mk4"),
            (260, "prusa/mk4s"),
            (120, "prusa/mini"),
            (170, "prusa/xl"),
            (160, "prusa/ix"),
            (310, "prusa/coreone"),
            (350, "prusa/coreone-l"),
            (360, "prusa/coreone-indx"),
            (370, "prusa/coreone-l-indx"),
            (380, "prusa/signature-oak"),
            (410, "prusa/xl-plus"),
        ] {
            assert_eq!(by(Some(code), None, None), Some(id), "{code}");
        }
        assert_eq!(by(None, None, Some("Prusa i3 MK3")), Some("prusa/i3-mk3"));
        assert_eq!(by(None, None, Some("Prusa-COREONE")), Some("prusa/coreone"));
        // No code twice.
        let mut codes: Vec<u32> = all()
            .iter()
            .filter_map(|p| p.detect.as_ref()?.model_code)
            .collect();
        let n = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), n, "a model code twice");
        // Every Prusa profile is whole: size, limits, slicer names.
        for p in all()
            .iter()
            .filter(|p| p.brand == "Prusa" && p.source.is_none())
        {
            assert!(p.build_volume_mm.iter().all(|v| *v > 0.0), "{}", p.id);
            assert!(
                p.max_nozzle_c.is_some() && p.max_bed_c.is_some(),
                "{}",
                p.id
            );
            assert!(!p.printer_models.is_empty(), "{}", p.id);
        }
        // Sliced for one: that one (by PrusaSlicer's name or OrcaSlicer's), not another.
        let fits = |id: &str, sliced: &str| {
            find(id)
                .unwrap()
                .printer_models
                .iter()
                .any(|m| crate::files::same_model(sliced, m))
        };
        assert!(fits("prusa/mini", "MINIIS") && fits("prusa/mini", "Prusa MINI IS"));
        assert!(fits("prusa/mk4", "MK4IS") && !fits("prusa/mk4", "MK4S"));
        assert!(fits("prusa/coreone", "COREONE") && !fits("prusa/coreone", "COREONEL"));
        assert!(fits("prusa/xl", "XL5IS") && !fits("prusa/xl", "XLP5T"));
    }

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

    /// A laser by its `$I` `MODEL`: the Ortur's own answer (#165) finds its profile; a name no
    /// profile has finds none.
    #[test]
    fn a_laser_by_its_model() {
        let by = |name: &str| detect_laser(name).map(|p| p.id.as_str());
        assert_eq!(by("Ortur Laser Master 2 Pro S2"), Some("ortur/lm2-pro-s2"));
        assert_eq!(
            by(" ortur laser master 2 pro s2 "),
            Some("ortur/lm2-pro-s2")
        );
        for none in [
            "",
            "Ortur",
            "Ortur Laser Master 2 Pro S2 (10W module)",
            "Some Laser",
        ] {
            assert_eq!(by(none), None, "{none}");
        }
    }

    /// A Marlin printer by its M115 `MACHINE_TYPE`: a model's own name finds it; Marlin's
    /// default name, a name several models share and a custom printer's find none.
    #[test]
    fn a_marlin_printer_by_its_machine_type() {
        let by = |name: &str| {
            detect(&Identity {
                machine_type: Some(name.to_owned()),
                ..Identity::default()
            })
            .map(|p| p.id.as_str())
        };
        assert_eq!(by("Ender-5 Pro"), Some("creality/ender-5-pro"));
        assert_eq!(by("ender-5 pro"), Some("creality/ender-5-pro"));
        for none in [
            "3D Printer",
            "Copymaster 3D",
            // The owner's own builds (CuriousTimo/Marlin, MyBuilds---*): no profile, chosen by hand.
            "AtomX MK3",
            "Prutender MK3s",
            "Crearibo CR10",
        ] {
            assert_eq!(by(none), None, "{none}");
        }
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

#[cfg(test)]
mod hs1_tests {
    /// HS1: the hub takes the ports' geometry without their device code.
    #[test]
    fn the_hub_has_no_device_crates() {
        let out = std::process::Command::new(env!("CARGO"))
            .args([
                "tree",
                "-p",
                "maker-connect-hub",
                "-e",
                "normal",
                "--prefix",
                "none",
            ])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("cargo tree");
        let tree = String::from_utf8_lossy(&out.stdout);
        assert!(tree.contains("maker-connect-engine"), "{tree}");
        for name in ["serialport ", "nusb ", "hidapi "] {
            assert!(
                !tree.lines().any(|l| l.starts_with(name)),
                "the hub pulls in {name}"
            );
        }
    }

    /// HS2b: the 450's rolls are DYMO Connect's catalogue, each SKU once, S0722530 (11353) among them.
    #[test]
    fn the_450_takes_its_rolls_from_the_catalogue() {
        let profile = super::find("dymo/labelwriter-450").unwrap();
        assert_eq!(
            profile.media.len(),
            maker_connect_engine::ports::dymo_connect_lw450::ROLLS.len()
        );
        let m = super::media(profile, "S0722530").unwrap();
        assert_eq!(m.name, "LW LABEL MULTI-PURPOSE 13X25MM · 25 × 25 mm");
        assert_eq!(
            super::labelwriter_page(Some(profile), "S0722530")
                .unwrap()
                .name,
            "w72h72.2"
        );
        assert!(profile.media.iter().all(|m| m.per_roll.is_none()));
    }
}
