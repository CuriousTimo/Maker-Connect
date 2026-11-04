//! Makes the filament catalogue from PrusaSlicer's vendor profiles: a bundle such as
//! <https://github.com/prusa3d/PrusaSlicer-settings-prusa-fff> (`PrusaResearch/<version>.ini`;
//! the profiles PrusaSlicer ships, AGPL-3.0). Only facts are taken: a filament's type,
//! diameter, density, the weight of its empty spool, its temperatures, its price. They go to
//! one file, `filaments/<bundle>.toml`, made again on each run.
//!
//! A bundle is an INI file of sections; a filament is `[filament:<name>]`, the ones starting
//! with `*` are templates the others inherit from (`inherits = *PET*; *common*`, later ones
//! overriding earlier ones, the section's own settings last). A filament comes in variants per
//! printer and nozzle (`Prusament PETG @MK4S 0.6`): one entry per filament is taken, the one
//! without a variant (the MK3S's), else its first variant.

use std::collections::BTreeMap;

type Section = BTreeMap<String, String>;

/// A filament as the catalogue has it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Filament {
    pub name: String,
    pub brand: String,
    /// PLA, PETG, ASA…
    #[serde(rename = "type")]
    pub kind: String,
    pub diameter_mm: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub density_g_cm3: Option<f64>,
    /// The empty spool: weighed with it, this is taken off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spool_g: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nozzle_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_layer_nozzle_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_layer_bed_c: Option<f64>,
    /// In the slicer's money, a kilogram.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_per_kg: Option<f64>,
    /// Its colour, when the profile has one of its own (not PrusaSlicer's default orange).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(serde::Serialize)]
struct Catalogue {
    source: String,
    filament: Vec<Filament>,
}

/// The sections of a bundle, by their name in brackets.
fn sections(text: &str) -> BTreeMap<String, Section> {
    let mut out: BTreeMap<String, Section> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim_end();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = Some(name.to_owned());
            out.entry(name.to_owned()).or_default();
        } else if let (Some(name), Some((key, value))) = (&current, line.split_once(" = "))
            && !key.starts_with('#')
        {
            out.entry(name.clone())
                .or_default()
                .insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    out
}

/// A filament's settings with what it inherits filled in.
fn resolve(all: &BTreeMap<String, Section>, name: &str, depth: u8) -> Section {
    let mut out = Section::new();
    let Some(own) = all.get(&format!("filament:{name}")) else {
        return out;
    };
    if depth < 16
        && let Some(parents) = own.get("inherits")
    {
        for parent in parents.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            out.extend(resolve(all, parent, depth + 1));
        }
    }
    out.extend(own.clone());
    out.remove("inherits");
    out
}

/// A number setting; for one per extruder (`210,210`), the first.
fn number(s: &Section, key: &str) -> Option<f64> {
    s.get(key)?
        .split(',')
        .next()?
        .trim()
        .parse()
        .ok()
        .filter(|n: &f64| n.is_finite() && *n > 0.0)
}

fn text(s: &Section, key: &str) -> Option<String> {
    let v = s.get(key)?.split(';').next()?.trim().trim_matches('"');
    (!v.is_empty()).then(|| v.to_owned())
}

/// Every filament of a bundle, one per name, by brand and name.
pub fn filaments(bundle: &str) -> Vec<Filament> {
    let all = sections(bundle);
    // Each filament's name without a variant, and the section to read it from.
    let mut chosen: BTreeMap<String, String> = BTreeMap::new();
    for section in all.keys() {
        let Some(name) = section.strip_prefix("filament:") else {
            continue;
        };
        if name.starts_with('*') {
            continue;
        }
        let base = name.split(" @").next().unwrap_or(name).trim().to_owned();
        let own = name == base;
        let entry = chosen.entry(base).or_insert_with(|| name.to_owned());
        if own {
            *entry = name.to_owned();
        }
    }
    let mut out: Vec<Filament> = chosen
        .into_iter()
        .filter_map(|(base, section)| {
            let s = resolve(&all, &section, 0);
            let color = text(&s, "filament_colour")
                .filter(|c| c.starts_with('#') && !c.eq_ignore_ascii_case("#FF8000"));
            Some(Filament {
                brand: text(&s, "filament_vendor").unwrap_or_default(),
                kind: text(&s, "filament_type")?,
                diameter_mm: number(&s, "filament_diameter").unwrap_or(1.75),
                density_g_cm3: number(&s, "filament_density"),
                spool_g: number(&s, "filament_spool_weight"),
                nozzle_c: number(&s, "temperature"),
                first_layer_nozzle_c: number(&s, "first_layer_temperature"),
                bed_c: number(&s, "bed_temperature"),
                first_layer_bed_c: number(&s, "first_layer_bed_temperature"),
                cost_per_kg: number(&s, "filament_cost"),
                color,
                name: base,
            })
        })
        .collect();
    out.sort_by(|a, b| (&a.brand, &a.name).cmp(&(&b.brand, &b.name)));
    out
}

/// The bundle's `config_version` and `repo_id` (`prusa-fff`, `2.5.14`).
fn identity(bundle: &str) -> (String, String) {
    let all = sections(bundle);
    let vendor = all.get("vendor");
    let get = |k: &str| vendor.and_then(|v| v.get(k)).cloned().unwrap_or_default();
    (get("repo_id"), get("config_version"))
}

/// Reads a bundle, writes the catalogue.
pub fn run(bundle: &std::path::Path, out: &std::path::Path) -> Result<(), String> {
    let text = std::fs::read_to_string(bundle).map_err(|e| format!("{}: {e}", bundle.display()))?;
    let (repo, version) = identity(&text);
    let filament = filaments(&text);
    if filament.is_empty() {
        return Err(format!("{}: no filaments in it", bundle.display()));
    }
    let source = format!("prusaslicer-settings-{repo}@{version}");
    let count = filament.len();
    let body = toml::to_string(&Catalogue {
        source: source.clone(),
        filament,
    })
    .map_err(|e| e.to_string())?;
    let page = format!(
        "# Made from PrusaSlicer's vendor profiles (AGPL-3.0) by `scripts/import-profiles.sh`:\n\
         # made again on each run. Only facts: type, diameter, density, empty spool, temperatures, price.\n{body}"
    );
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(out, page).map_err(|e| e.to_string())?;
    println!("{count} filaments written to {} ({source})", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE: &str = "\
[vendor]
repo_id = prusa-fff
config_version = 2.5.14

[filament:*common*]
filament_diameter = 1.75
filament_colour = #FF8000
temperature = 200
bed_temperature = 60

[filament:*PET*]
inherits = *common*
filament_type = PETG
temperature = 240
bed_temperature = 90

[filament:Prusament PETG]
inherits = *PET*
filament_vendor = Prusa Polymers
temperature = 250
filament_density = 1.27
filament_spool_weight = 278
filament_type = PETG

[filament:Prusament PETG @MK4S 0.6]
inherits = Prusament PETG
temperature = 255

[filament:Only Variants PLA @PG]
inherits = *common*
filament_vendor = Someone
filament_type = PLA
filament_colour = #AA845D
";

    /// Inherited settings are filled in, the filament's own win, its variants are one entry.
    #[test]
    fn a_bundle_becomes_filaments() {
        let f = filaments(BUNDLE);
        assert_eq!(f.len(), 2, "{f:?}");
        let petg = f.iter().find(|f| f.name == "Prusament PETG").unwrap();
        assert_eq!(petg.brand, "Prusa Polymers");
        assert_eq!(petg.kind, "PETG");
        assert_eq!(
            petg.nozzle_c,
            Some(250.0),
            "its own, not the variant's or template's"
        );
        assert_eq!(petg.bed_c, Some(90.0), "from *PET*");
        assert_eq!(petg.diameter_mm, 1.75, "from *common*");
        assert_eq!(petg.spool_g, Some(278.0));
        assert_eq!(petg.color, None, "the default orange is not its colour");
        let only = f.iter().find(|f| f.name == "Only Variants PLA").unwrap();
        assert_eq!(only.color.as_deref(), Some("#AA845D"));
        assert_eq!(identity(BUNDLE), ("prusa-fff".into(), "2.5.14".into()));
    }
}
