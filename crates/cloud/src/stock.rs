//! Consumables and stock (the wiki's design, phase 1): **materials** (what is bought: a
//! filament, a label tape), **items** (what the space has: one spool, one cassette, each with
//! a code such as `S-0007` and a QR label), **locations** (`L-0003`: a shelf, a dry box), and
//! a ledger of **movements**: every change to an item (received, moved, loaded into a
//! machine, weighed, used up) is one, with when; amounts follow from them.

use crate::store::{Store, now};
use crate::{STYLE, Shared, failed, header, profiles};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

const STOCK_STYLE: &str = include_str!("library.css");
const STOCK_SCRIPT: &str = include_str!("library.js");

/// The tables, made when missing: Spoolman's model (vendor, filament, spool), so its API
/// and its data fit (see `spoolman.rs`), with what Spoolman does not have beside it (tapes
/// and other kinds, locations as places with codes, what a spool is loaded in). Materials
/// kept before (`materials`, `items`) are moved over once; those tables stay as they were.
pub fn migrate(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS locations (
             id TEXT PRIMARY KEY,
             space_id INTEGER NOT NULL,
             name TEXT NOT NULL,
             parent TEXT
         );
         CREATE TABLE IF NOT EXISTS movements (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL,
             item TEXT NOT NULL,
             kind TEXT NOT NULL,
             amount REAL,
             place TEXT,
             note TEXT,
             at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS setting (
             space_id INTEGER NOT NULL,
             key TEXT NOT NULL,
             value TEXT NOT NULL,
             last_updated INTEGER NOT NULL,
             PRIMARY KEY (space_id, key)
         );",
    )?;
    let fresh: bool = db.query_row(
        "SELECT COUNT(*) = 0 FROM sqlite_master WHERE type = 'table' AND name = 'spool'",
        [],
        |r| r.get(0),
    )?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS vendor (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL,
             registered INTEGER NOT NULL,
             name TEXT NOT NULL,
             empty_spool_weight REAL,
             comment TEXT,
             external_id TEXT,
             extra TEXT NOT NULL DEFAULT '{}'
         );
         CREATE TABLE IF NOT EXISTS filament (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL,
             registered INTEGER NOT NULL,
             name TEXT,
             vendor_id INTEGER REFERENCES vendor(id),
             material TEXT,
             price REAL,
             density REAL NOT NULL DEFAULT 1.24,
             diameter REAL NOT NULL DEFAULT 1.75,
             weight REAL,
             spool_weight REAL,
             article_number TEXT,
             comment TEXT,
             settings_extruder_temp INTEGER,
             settings_bed_temp INTEGER,
             color_hex TEXT,
             multi_color_hexes TEXT,
             multi_color_direction TEXT,
             external_id TEXT,
             extra TEXT NOT NULL DEFAULT '{}',
             -- Maker Connect's own, beside Spoolman's:
             kind TEXT NOT NULL DEFAULT 'filament',
             unit TEXT NOT NULL DEFAULT 'g',
             color_name TEXT,
             ink TEXT,
             media TEXT,
             width_mm INTEGER,
             minimum INTEGER
         );
         CREATE TABLE IF NOT EXISTS spool (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL,
             registered INTEGER NOT NULL,
             first_used INTEGER,
             last_used INTEGER,
             price REAL,
             filament_id INTEGER NOT NULL REFERENCES filament(id),
             initial_weight REAL,
             spool_weight REAL,
             used_weight REAL NOT NULL DEFAULT 0,
             location_id TEXT,
             lot_nr TEXT,
             comment TEXT,
             archived INTEGER NOT NULL DEFAULT 0,
             extra TEXT NOT NULL DEFAULT '{}',
             -- Maker Connect's own: the machine it is loaded in.
             machine TEXT
         );",
    )?;
    // NFC tags (Spoolman 0.27's): one physical tag means one spool or filament.
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS tag (
             id INTEGER PRIMARY KEY,
             space_id INTEGER NOT NULL,
             uid TEXT NOT NULL,
             target_type TEXT NOT NULL,
             spool_id INTEGER,
             filament_id INTEGER,
             target_value TEXT,
             format TEXT,
             added INTEGER NOT NULL,
             UNIQUE (space_id, uid)
         );
         CREATE TABLE IF NOT EXISTS extra_field (
             space_id INTEGER NOT NULL,
             entity_type TEXT NOT NULL,
             key TEXT NOT NULL,
             definition TEXT NOT NULL,
             PRIMARY KEY (space_id, entity_type, key)
         );",
    )?;
    // A place can be a dry box with numbered slots, a spool in one of them.
    for (table, column, definition) in [
        ("locations", "slots", "INTEGER"),
        ("locations", "model", "TEXT"),
        ("locations", "description", "TEXT"),
        ("spool", "slot", "INTEGER"),
    ] {
        let present: bool = db.query_row(
            &format!("SELECT COUNT(*) > 0 FROM pragma_table_info('{table}') WHERE name = ?1"),
            [column],
            |r| r.get(0),
        )?;
        if !present {
            db.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    if fresh {
        move_over(db)?;
    }
    Ok(())
}

/// Materials and items kept before the Spoolman model: each brand a vendor, each material a
/// filament, each item a spool with its number (`S-0007` is spool 7).
fn move_over(db: &Connection) -> rusqlite::Result<()> {
    let old: bool = db.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'materials'",
        [],
        |r| r.get(0),
    )?;
    if !old {
        return Ok(());
    }
    let mut materials = Vec::new();
    {
        let mut q = db.prepare("SELECT id, space_id, data FROM materials")?;
        let rows = q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (id, space, data) = row?;
            if let Ok(m) = serde_json::from_str::<Material>(&data) {
                materials.push((id, space, m));
            }
        }
    }
    let mut filament_of = std::collections::HashMap::new();
    for (old_id, space, m) in materials {
        let id = insert_filament(db, space, &m)?;
        filament_of.insert(old_id, id);
    }
    let mut q = db.prepare(
        "SELECT id, space_id, material, amount, location, machine, state, created_at FROM items",
    )?;
    let items = q.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, f64>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, i64>(7)?,
        ))
    })?;
    for item in items {
        let (code, space, material, amount, location, machine, state, created) = item?;
        let (Some(number), Some(filament)) = (spool_number(&code), filament_of.get(&material))
        else {
            continue;
        };
        let package: Option<f64> = db
            .query_row(
                "SELECT weight FROM filament WHERE id = ?1",
                [filament],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let initial = package.unwrap_or(amount).max(amount);
        let used_up = state == "used-up";
        db.execute(
            "INSERT INTO spool (id, space_id, registered, filament_id, initial_weight, used_weight,
                 location_id, archived, machine)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                number,
                space,
                created,
                filament,
                initial,
                if used_up { initial } else { initial - amount },
                location,
                used_up,
                machine
            ],
        )?;
    }
    Ok(())
}

/// A vendor by its name in a space, made when missing.
fn vendor_id(db: &Connection, space: i64, name: &str) -> rusqlite::Result<Option<i64>> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    if let Some(id) = db
        .query_row(
            "SELECT id FROM vendor WHERE space_id = ?1 AND name = ?2",
            params![space, name],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(Some(id));
    }
    db.execute(
        "INSERT INTO vendor (space_id, registered, name) VALUES (?1, ?2, ?3)",
        params![space, now(), name],
    )?;
    Ok(Some(db.last_insert_rowid()))
}

/// A colour as Spoolman keeps it: hex without `#` (`1d1d1b`); None for one that is not.
fn hex(color: Option<&str>) -> Option<String> {
    let c = color?.trim().trim_start_matches('#');
    let full = match c.len() {
        3 => c.chars().flat_map(|ch| [ch, ch]).collect(),
        6 | 8 => c.to_owned(),
        _ => return None,
    };
    full.chars()
        .all(|ch| ch.is_ascii_hexdigit())
        .then(|| full.to_ascii_lowercase())
}

fn insert_filament(db: &Connection, space: i64, m: &Material) -> rusqlite::Result<i64> {
    let vendor = vendor_id(db, space, &m.brand)?;
    db.execute(
        "INSERT INTO filament (space_id, registered, name, vendor_id, material, density, diameter,
             weight, spool_weight, article_number, settings_extruder_temp, settings_bed_temp,
             color_hex, external_id, kind, unit, color_name, ink, media, width_mm, minimum)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
             ?19, ?20, ?21)",
        params![
            space,
            now(),
            m.name,
            vendor,
            material_of(m),
            m.density_g_cm3.unwrap_or(1.24),
            m.diameter_mm.unwrap_or(1.75),
            m.package,
            m.spool_g,
            m.ean,
            m.nozzle_c.map(|c| c.round() as i64),
            m.bed_c.map(|c| c.round() as i64),
            hex(m.color.as_deref()),
            m.source,
            m.kind,
            m.unit,
            m.color_name,
            m.ink,
            m.media,
            m.width_mm,
            m.minimum
        ],
    )?;
    Ok(db.last_insert_rowid())
}

/// The material a name says (`Prusament PETG 1.75 mm` → `PETG`), for Spoolman's field.
fn material_of(m: &Material) -> Option<String> {
    if m.kind != "filament" {
        return Some(m.kind.to_uppercase());
    }
    const KNOWN: [&str; 14] = [
        "PETG", "PLA", "ABS", "ASA", "TPU", "PCTG", "PET", "PC", "PA", "HIPS", "PVA", "PP", "FLEX",
        "CPE",
    ];
    let upper = m.name.to_uppercase();
    KNOWN
        .iter()
        .find(|k| {
            upper
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|w| w == **k)
        })
        .map(|k| (*k).to_owned())
}

/// `S-0007` → 7.
pub fn spool_number(code: &str) -> Option<i64> {
    code.trim()
        .to_ascii_uppercase()
        .strip_prefix("S-")?
        .parse()
        .ok()
}

/// 7 → `S-0007`.
pub fn spool_code(id: i64) -> String {
    format!("S-{id:04}")
}

/// What can be bought.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Material {
    #[serde(default)]
    pub id: String,
    /// `filament`, `resin`, `tape`, `sheet`, `other`.
    pub kind: String,
    #[serde(default)]
    pub brand: String,
    pub name: String,
    /// Its colour (`#1d1d1b`): a tape's tape colour, a filament's colour.
    #[serde(default)]
    pub color: Option<String>,
    /// Its colour as the maker names it: `Galaxy Black`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_name: Option<String>,
    /// A tape's ink.
    #[serde(default)]
    pub ink: Option<String>,
    /// `g`, `m`, `pcs`.
    pub unit: String,
    /// How much one comes with (1000 g, 7 m).
    #[serde(default)]
    pub package: Option<f64>,
    #[serde(default)]
    pub ean: Option<String>,
    /// Below this many items in stock, it is low.
    #[serde(default)]
    pub minimum: Option<u32>,
    /// A tape of a machine profile: `<profile id>:<media id>` (`dymo/labelmanager-pnp:45013`),
    /// so loading it tells the printer which tape it has.
    #[serde(default)]
    pub media: Option<String>,
    #[serde(default)]
    pub width_mm: Option<u32>,
    /// A filament's facts, from its slicer profile: what turns metres into grams, what to
    /// take off when it is weighed on its spool, how hot it prints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diameter_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density_g_cm3: Option<f64>,
    /// The empty spool's weight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spool_g: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nozzle_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bed_c: Option<f64>,
    /// Where it came from: `prusaslicer-settings-prusa-fff@2.5.14`, `spoolmandb:<id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Location {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    /// A dry box's slots, numbered from 1; None for a shelf.
    pub slots: Option<u32>,
    /// The dry box it is, from [`DRYBOXES`].
    pub model: Option<String>,
    /// What it is or where, in words: "under the bench, left".
    pub description: Option<String>,
}

/// A dry box someone may have: its slots for 1 kg spools, whether it heats, whether it tells
/// its humidity. From the makers' pages; `check` where sources disagree or were not found.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Drybox {
    pub id: &'static str,
    pub brand: &'static str,
    pub model: &'static str,
    pub slots: u32,
    pub heats: bool,
    pub check: bool,
}

pub const DRYBOXES: [Drybox; 13] = [
    Drybox {
        id: "repkord-repbox-2.5",
        brand: "Repkord",
        model: "RepBox v2.5",
        slots: 6,
        heats: false,
        check: false,
    },
    Drybox {
        id: "sunlu-s4",
        brand: "SUNLU",
        model: "FilaDryer S4",
        slots: 4,
        heats: true,
        check: false,
    },
    Drybox {
        id: "sunlu-s2",
        brand: "SUNLU",
        model: "FilaDryer S2",
        slots: 1,
        heats: true,
        check: true,
    },
    Drybox {
        id: "sunlu-s1-plus",
        brand: "SUNLU",
        model: "FilaDryer S1 Plus",
        slots: 1,
        heats: true,
        check: true,
    },
    Drybox {
        id: "eibos-cyclopes",
        brand: "EIBOS",
        model: "Cyclopes",
        slots: 2,
        heats: true,
        check: false,
    },
    Drybox {
        id: "eibos-polyphemus",
        brand: "EIBOS",
        model: "Polyphemus",
        slots: 2,
        heats: true,
        check: false,
    },
    Drybox {
        id: "polymaker-polydryer",
        brand: "Polymaker",
        model: "PolyDryer box",
        slots: 1,
        heats: true,
        check: false,
    },
    Drybox {
        id: "creality-space-pi",
        brand: "Creality",
        model: "Space Pi",
        slots: 1,
        heats: true,
        check: true,
    },
    Drybox {
        id: "creality-space-pi-x4",
        brand: "Creality",
        model: "Space Pi X4",
        slots: 4,
        heats: true,
        check: true,
    },
    Drybox {
        id: "esun-ebox",
        brand: "eSUN",
        model: "eBOX",
        slots: 1,
        heats: true,
        check: true,
    },
    Drybox {
        id: "bambu-ams",
        brand: "Bambu Lab",
        model: "AMS",
        slots: 4,
        heats: false,
        check: true,
    },
    Drybox {
        id: "anycubic-ace-pro",
        brand: "Anycubic",
        model: "ACE Pro",
        slots: 4,
        heats: true,
        check: true,
    },
    Drybox {
        id: "bluerolls-drybox",
        brand: "BlueRolls",
        model: "Drybox",
        slots: 1,
        heats: false,
        check: true,
    },
];

/// One thing the space has.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    pub id: String,
    pub material: String,
    pub amount: f64,
    pub location: Option<String>,
    /// The machine it is loaded in.
    pub machine: Option<String>,
    /// `stock`, `loaded`, `used-up`.
    pub state: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Movement {
    pub kind: String,
    pub amount: Option<f64>,
    pub place: Option<String>,
    pub note: Option<String>,
    pub at: i64,
}

/// The next code of a kind in a space: `L-0001`.
fn next_code(db: &Connection, table: &str, prefix: &str, space: i64) -> rusqlite::Result<String> {
    let last: Option<String> = db
        .query_row(
            &format!(
                "SELECT id FROM {table} WHERE space_id = ?1 AND id LIKE ?2 ORDER BY id DESC LIMIT 1"
            ),
            params![space, format!("{prefix}-%")],
            |r| r.get(0),
        )
        .optional()?;
    let n = last
        .and_then(|l| l.split_once('-').and_then(|(_, n)| n.parse::<u32>().ok()))
        .unwrap_or(0);
    Ok(format!("{prefix}-{:04}", n + 1))
}

impl Store {
    pub(crate) fn movement(
        &self,
        space: i64,
        item: &str,
        kind: &str,
        amount: Option<f64>,
        place: Option<&str>,
        note: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO movements (space_id, item, kind, amount, place, note, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![space, item, kind, amount, place, note, now()],
        )?;
        Ok(())
    }

    pub fn materials(&self, space: i64) -> rusqlite::Result<Vec<Material>> {
        let mut q = self.db.prepare(
            "SELECT f.id, f.kind, v.name, f.name, f.color_hex, f.color_name, f.ink, f.unit, f.weight,
                    f.article_number, f.minimum, f.media, f.width_mm, f.diameter, f.density,
                    f.spool_weight, f.settings_extruder_temp, f.settings_bed_temp, f.external_id
             FROM filament f LEFT JOIN vendor v ON v.id = f.vendor_id WHERE f.space_id = ?1",
        )?;
        let rows = q.query_map([space], |r| {
            let filament = r.get::<_, String>(1)? == "filament";
            Ok(Material {
                id: r.get::<_, i64>(0)?.to_string(),
                kind: r.get(1)?,
                brand: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                name: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                color: r.get::<_, Option<String>>(4)?.map(|c| format!("#{c}")),
                color_name: r.get(5)?,
                ink: r.get(6)?,
                unit: r.get(7)?,
                package: r.get(8)?,
                ean: r.get(9)?,
                minimum: r.get(10)?,
                media: r.get(11)?,
                width_mm: r.get(12)?,
                diameter_mm: if filament { r.get(13)? } else { None },
                density_g_cm3: if filament { r.get(14)? } else { None },
                spool_g: r.get(15)?,
                nozzle_c: r.get::<_, Option<i64>>(16)?.map(|c| c as f64),
                bed_c: r.get::<_, Option<i64>>(17)?.map(|c| c as f64),
                source: r.get(18)?,
            })
        })?;
        let mut out: Vec<Material> = rows.collect::<rusqlite::Result<_>>()?;
        out.sort_by(|a, b| (&a.kind, &a.brand, &a.name).cmp(&(&b.kind, &b.brand, &b.name)));
        Ok(out)
    }

    pub fn add_material(&self, space: i64, material: Material) -> rusqlite::Result<String> {
        let id = insert_filament(&self.db, space, &material)?;
        self.changed();
        self.spoolman_event("filament", "added", id);
        Ok(id.to_string())
    }

    /// A material changed: false when there is no such one.
    pub fn update_material(&self, space: i64, id: &str, m: Material) -> rusqlite::Result<bool> {
        let Ok(id) = id.parse::<i64>() else {
            return Ok(false);
        };
        let vendor = vendor_id(&self.db, space, &m.brand)?;
        let n = self.db.execute(
            "UPDATE filament SET name = ?1, vendor_id = ?2, material = ?3, density = ?4, diameter = ?5,
                 weight = ?6, spool_weight = ?7, article_number = ?8, settings_extruder_temp = ?9,
                 settings_bed_temp = ?10, color_hex = ?11, kind = ?12, unit = ?13, color_name = ?14,
                 ink = ?15, media = ?16, width_mm = ?17, minimum = ?18
             WHERE id = ?19 AND space_id = ?20",
            params![
                m.name,
                vendor,
                material_of(&m),
                m.density_g_cm3.unwrap_or(1.24),
                m.diameter_mm.unwrap_or(1.75),
                m.package,
                m.spool_g,
                m.ean,
                m.nozzle_c.map(|c| c.round() as i64),
                m.bed_c.map(|c| c.round() as i64),
                hex(m.color.as_deref()),
                m.kind,
                m.unit,
                m.color_name,
                m.ink,
                m.media,
                m.width_mm,
                m.minimum,
                id,
                space
            ],
        )?;
        if n > 0 {
            self.changed();
            self.spoolman_event("filament", "updated", id);
        }
        Ok(n > 0)
    }

    pub fn locations(&self, space: i64) -> rusqlite::Result<Vec<Location>> {
        let mut q = self.db.prepare(
            "SELECT id, name, parent, slots, model, description FROM locations WHERE space_id = ?1 ORDER BY id",
        )?;
        let rows = q.query_map([space], |r| {
            Ok(Location {
                id: r.get(0)?,
                name: r.get(1)?,
                parent: r.get(2)?,
                slots: r.get(3)?,
                model: r.get(4)?,
                description: r.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn add_location(
        &self,
        space: i64,
        name: &str,
        parent: Option<&str>,
    ) -> rusqlite::Result<String> {
        self.add_place(space, name, parent, None, None)
    }

    /// A place, perhaps a dry box with its slots (and which one it is): its code.
    pub fn add_place(
        &self,
        space: i64,
        name: &str,
        parent: Option<&str>,
        slots: Option<u32>,
        model: Option<&str>,
    ) -> rusqlite::Result<String> {
        let id = next_code(&self.db, "locations", "L", space)?;
        self.db.execute(
            "INSERT INTO locations (id, space_id, name, parent, slots, model) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, space, name, parent, slots.filter(|s| *s > 0), model],
        )?;
        self.changed();
        Ok(id)
    }

    /// A location changed: its name, description, slots, which dry box. Spools in slots it no
    /// longer has come out of them. False when there is no such location.
    pub fn update_location(
        &self,
        space: i64,
        id: &str,
        name: &str,
        description: Option<&str>,
        slots: Option<u32>,
        model: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let n = self.db.execute(
            "UPDATE locations SET name = ?1, description = ?2, slots = ?3, model = ?4
             WHERE id = ?5 AND space_id = ?6",
            params![
                name,
                description,
                slots.filter(|s| *s > 0),
                model,
                id,
                space
            ],
        )?;
        if n > 0 {
            self.db.execute(
                "UPDATE spool SET slot = NULL WHERE location_id = ?1 AND slot > COALESCE(?2, 0)",
                params![id, slots],
            )?;
            self.changed();
            for spool in self.spools_at(id)? {
                self.spoolman_event("spool", "updated", spool);
            }
        }
        Ok(n > 0)
    }

    /// A location removed; what was in it has no location then (and stays loaded where it was).
    pub fn remove_location(&self, space: i64, id: &str) -> rusqlite::Result<bool> {
        let spools = self.spools_at(id)?;
        self.db.execute(
            "UPDATE spool SET location_id = NULL, slot = NULL WHERE location_id = ?1 AND space_id = ?2",
            params![id, space],
        )?;
        let n = self.db.execute(
            "DELETE FROM locations WHERE id = ?1 AND space_id = ?2",
            params![id, space],
        )?;
        if n > 0 {
            for spool in spools {
                self.movement(
                    space,
                    &spool_code(spool),
                    "moved",
                    None,
                    None,
                    Some("its location was removed"),
                )?;
                self.spoolman_event("spool", "updated", spool);
            }
            self.changed();
        }
        Ok(n > 0)
    }

    fn spools_at(&self, location: &str) -> rusqlite::Result<Vec<i64>> {
        let mut q = self
            .db
            .prepare("SELECT id FROM spool WHERE location_id = ?1")?;
        let ids = q.query_map([location], |r| r.get(0))?;
        ids.collect()
    }

    /// A location by its name (Spoolman keeps locations as names), made when missing: its code.
    pub fn location_named(&self, space: i64, name: &str) -> rusqlite::Result<Option<String>> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(None);
        }
        if let Some(id) = self
            .db
            .query_row(
                "SELECT id FROM locations WHERE space_id = ?1 AND name = ?2",
                params![space, name],
                |r| r.get(0),
            )
            .optional()?
        {
            return Ok(Some(id));
        }
        self.add_location(space, name, None).map(Some)
    }

    pub fn items(&self, space: i64) -> rusqlite::Result<Vec<Item>> {
        let mut q = self.db.prepare(
            "SELECT id, filament_id, COALESCE(initial_weight, 0) - used_weight, location_id, machine,
                    archived, registered
             FROM spool WHERE space_id = ?1 ORDER BY id",
        )?;
        let rows = q.query_map([space], |r| {
            let archived: bool = r.get(5)?;
            let machine: Option<String> = r.get(4)?;
            Ok(Item {
                id: spool_code(r.get(0)?),
                material: r.get::<_, i64>(1)?.to_string(),
                amount: r.get::<_, f64>(2)?.max(0.0),
                location: r.get(3)?,
                state: if archived {
                    "used-up"
                } else if machine.is_some() {
                    "loaded"
                } else {
                    "stock"
                }
                .to_owned(),
                machine,
                created_at: r.get(6)?,
            })
        })?;
        rows.collect()
    }

    fn item(&self, space: i64, id: &str) -> rusqlite::Result<Option<Item>> {
        Ok(self.items(space)?.into_iter().find(|i| i.id == id))
    }

    /// Items received: `count` of a material, each with its package's amount, at a place.
    pub fn receive(
        &self,
        space: i64,
        material: &str,
        count: u32,
        location: Option<&str>,
    ) -> rusqlite::Result<Result<Vec<String>, &'static str>> {
        let Some(m) = self
            .materials(space)?
            .into_iter()
            .find(|m| m.id == material)
        else {
            return Ok(Err("no such material"));
        };
        let mut made = Vec::new();
        for _ in 0..count.clamp(1, 200) {
            let amount = m.package.unwrap_or(1.0);
            self.db.execute(
                "INSERT INTO spool (space_id, registered, filament_id, initial_weight, spool_weight,
                     used_weight, location_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
                params![space, now(), m.id, amount, m.spool_g, location],
            )?;
            let id = self.db.last_insert_rowid();
            let code = spool_code(id);
            self.movement(space, &code, "received", Some(amount), location, None)?;
            self.spoolman_event("spool", "added", id);
            made.push(code);
        }
        self.changed();
        Ok(Ok(made))
    }

    /// Moves an item to a location (out of a machine, if it was loaded).
    pub fn move_item(
        &self,
        space: i64,
        id: &str,
        location: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let Some(number) = spool_number(id) else {
            return Ok(false);
        };
        let n = self.db.execute(
            "UPDATE spool SET location_id = ?1, machine = NULL, slot = NULL WHERE id = ?2 AND space_id = ?3",
            params![location, number, space],
        )?;
        if n > 0 {
            self.movement(space, id, "moved", None, location, None)?;
            self.changed();
            self.spoolman_event("spool", "updated", number);
        }
        Ok(n > 0)
    }

    /// Loads an item into a machine, where it is (a spool feeds a printer from its dry box):
    /// what was loaded there is no longer (one spool, one cassette at a time). A tape of the
    /// machine's profile tells the machine its tape.
    pub fn load_item(
        &self,
        space: i64,
        id: &str,
        machine: &str,
    ) -> rusqlite::Result<Result<(), &'static str>> {
        let Some(item) = self.item(space, id)? else {
            return Ok(Err("no such item"));
        };
        let Some(target) = self.machine(space, machine)? else {
            return Ok(Err("no such machine"));
        };
        // A tape into a label printer, filament into a 3D printer; a tape or roll of labels
        // made for one printer model only into that model (a LabelManager's cassette does
        // not go into a LabelWriter). A printer without a profile takes any.
        let material = self
            .materials(space)?
            .into_iter()
            .find(|m| m.id == item.material);
        let made_for = material
            .as_ref()
            .and_then(|m| m.media.as_deref())
            .and_then(|media| media.split_once(':'))
            .map(|(profile, _)| profile.to_owned());
        if let (Some(made_for), Some(profile)) = (&made_for, &target.profile)
            && made_for != profile
        {
            return Ok(Err("this tape or roll is made for another label printer"));
        }
        let kind = material.map(|m| m.kind);
        match (kind.as_deref(), target.kind) {
            (Some("tape"), k) if k != maker_connect_protocol::MachineKind::Label => {
                return Ok(Err("a label tape goes into a label printer"));
            }
            (Some("filament"), k) if k != maker_connect_protocol::MachineKind::Fdm => {
                return Ok(Err("filament goes into a 3D printer"));
            }
            _ => {}
        }
        let others: Vec<String> = self
            .items(space)?
            .into_iter()
            .filter(|i| i.machine.as_deref() == Some(machine) && i.id != id)
            .map(|i| i.id)
            .collect();
        for other in others {
            if let Some(number) = spool_number(&other) {
                self.db
                    .execute("UPDATE spool SET machine = NULL WHERE id = ?1", [number])?;
                self.spoolman_event("spool", "updated", number);
            }
            self.movement(space, &other, "unloaded", None, Some(machine), None)?;
        }
        let number = spool_number(id).unwrap_or_default();
        self.db.execute(
            // In a box (a place with slots) it stays: the box feeds the printer. From a shelf,
            // or from nowhere, it is on the printer now.
            "UPDATE spool SET machine = ?1,
                 location_id = CASE WHEN (SELECT slots FROM locations WHERE id = spool.location_id) > 0
                     THEN location_id ELSE NULL END,
                 slot = CASE WHEN (SELECT slots FROM locations WHERE id = spool.location_id) > 0
                     THEN slot ELSE NULL END
             WHERE id = ?2",
            params![machine, number],
        )?;
        self.movement(space, id, "loaded", None, Some(machine), None)?;
        self.spoolman_event("spool", "updated", number);
        // A tape of this printer's profile: the printer now knows which tape it has.
        if let Some(media) = self
            .materials(space)?
            .into_iter()
            .find(|m| m.id == item.material)
            .and_then(|m| m.media)
            && let Some((profile, media_id)) = media.split_once(':')
            && target.profile.as_deref() == Some(profile)
        {
            self.set_media(space, machine, Some(media_id))?;
        }
        self.changed();
        Ok(Ok(()))
    }

    /// Takes an item out of its machine, leaving it where it is.
    pub fn unload_item(&self, space: i64, id: &str) -> rusqlite::Result<bool> {
        let Some(number) = spool_number(id) else {
            return Ok(false);
        };
        let machine: Option<String> = self
            .db
            .query_row(
                "SELECT machine FROM spool WHERE id = ?1 AND space_id = ?2",
                params![number, space],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let Some(machine) = machine else {
            return Ok(false);
        };
        self.db
            .execute("UPDATE spool SET machine = NULL WHERE id = ?1", [number])?;
        self.movement(space, id, "unloaded", None, Some(&machine), None)?;
        self.changed();
        self.spoolman_event("spool", "updated", number);
        Ok(true)
    }

    /// Sets what is left (weighed or counted), the difference recorded.
    pub fn set_amount(
        &self,
        space: i64,
        id: &str,
        amount: f64,
        why: &str,
    ) -> rusqlite::Result<bool> {
        let (Some(item), Some(number)) = (self.item(space, id)?, spool_number(id)) else {
            return Ok(false);
        };
        let amount = amount.max(0.0);
        // More than it came with: it came with more.
        self.db.execute(
            "UPDATE spool SET initial_weight = MAX(COALESCE(initial_weight, 0), ?1),
                 used_weight = MAX(COALESCE(initial_weight, 0), ?1) - ?1
             WHERE id = ?2",
            params![amount, number],
        )?;
        let kind = if why == "weighed" {
            "weighed"
        } else {
            "adjusted"
        };
        self.movement(space, id, kind, Some(amount - item.amount), None, None)?;
        self.changed();
        self.spoolman_event("spool", "updated", number);
        Ok(true)
    }

    pub fn use_up(&self, space: i64, id: &str) -> rusqlite::Result<bool> {
        let Some(number) = spool_number(id) else {
            return Ok(false);
        };
        let n = self.db.execute(
            "UPDATE spool SET archived = 1, used_weight = COALESCE(initial_weight, used_weight),
                 machine = NULL, location_id = NULL
             WHERE id = ?1 AND space_id = ?2",
            params![number, space],
        )?;
        if n > 0 {
            self.movement(space, id, "used-up", None, None, None)?;
            self.changed();
            self.spoolman_event("spool", "updated", number);
        }
        Ok(n > 0)
    }

    pub fn movements(&self, space: i64, item: &str) -> rusqlite::Result<Vec<Movement>> {
        let mut q = self.db.prepare(
            "SELECT kind, amount, place, note, at FROM movements WHERE space_id = ?1 AND item = ?2 ORDER BY id DESC",
        )?;
        let rows = q.query_map(params![space, item], |r| {
            Ok(Movement {
                kind: r.get(0)?,
                amount: r.get(1)?,
                place: r.get(2)?,
                note: r.get(3)?,
                at: r.get(4)?,
            })
        })?;
        rows.collect()
    }
}

/// The filament catalogue: `filaments/*.toml`, made from PrusaSlicer's vendor profiles by the
/// importer, built into the binary.
static FILAMENTS: include_dir::Dir =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../filaments");

async fn change_material(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<String>,
    Json(material): Json<Material>,
) -> Response {
    if material.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "a material needs a name").into_response();
    }
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.update_material(space, &id, material) {
        Ok(true) => Json(serde_json::json!({ "id": id })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such material").into_response(),
        Err(e) => failed(e),
    }
}

#[derive(Deserialize)]
struct FilamentFile {
    source: String,
    filament: Vec<Filament>,
}

#[derive(Deserialize)]
struct Filament {
    name: String,
    brand: String,
    #[serde(rename = "type")]
    kind: String,
    diameter_mm: f64,
    density_g_cm3: Option<f64>,
    spool_g: Option<f64>,
    nozzle_c: Option<f64>,
    bed_c: Option<f64>,
    color: Option<String>,
}

/// The filaments of the catalogue, as materials of 1 kg spools.
fn filaments() -> &'static [Material] {
    static ALL: std::sync::OnceLock<Vec<Material>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let mut out = Vec::new();
        for file in FILAMENTS.files() {
            if file.path().extension().is_none_or(|e| e != "toml") {
                continue;
            }
            let parsed: FilamentFile = toml::from_str(file.contents_utf8().unwrap_or_default())
                .unwrap_or_else(|e| panic!("filaments/{}: {e}", file.path().display()));
            for f in parsed.filament {
                out.push(Material {
                    id: String::new(),
                    kind: "filament".into(),
                    brand: f.brand,
                    name: format!("{} {} mm", f.name, f.diameter_mm),
                    // Without a colour of its own, the colour of its type.
                    color: f.color.or_else(|| type_color(&f.kind).map(str::to_owned)),
                    color_name: None,
                    ink: None,
                    unit: "g".into(),
                    package: Some(1000.0),
                    ean: None,
                    minimum: Some(1),
                    media: None,
                    width_mm: None,
                    diameter_mm: Some(f.diameter_mm),
                    density_g_cm3: f.density_g_cm3,
                    spool_g: f.spool_g,
                    nozzle_c: f.nozzle_c,
                    bed_c: f.bed_c,
                    source: Some(parsed.source.clone()),
                });
            }
        }
        out
    })
}

/// A plain swatch for a type of filament.
fn type_color(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "PLA" => "#d9d9d6",
        "PETG" | "PET" | "PCTG" | "CPE" => "#9fc6d9",
        "ASA" => "#e0d7c3",
        "ABS" => "#e6e1d6",
        "FLEX" => "#7a7a7a",
        _ => return None,
    })
}

/// Materials to start from: the tapes and labels of the machine profiles (the DYMOs', drawn
/// as they look), and the filaments of the catalogue. A cassette of tape counts in metres, a
/// roll of die-cut labels in labels.
fn catalogue() -> Vec<Material> {
    let mut out = Vec::new();
    for profile in profiles::all() {
        for m in &profile.media {
            out.push(Material {
                id: String::new(),
                kind: "tape".into(),
                brand: profile.brand.clone(),
                name: format!("{} ({})", m.name, m.id),
                color: m.background.clone(),
                color_name: None,
                ink: m.ink.clone(),
                unit: if m.length_mm.is_some() { "labels" } else { "m" }.into(),
                // A D1 cassette holds 7 m of tape; a roll of labels as many as it says.
                package: match m.length_mm {
                    Some(_) => m.per_roll.map(f64::from),
                    None => Some(7.0),
                },
                ean: None,
                minimum: Some(1),
                media: Some(format!("{}:{}", profile.id, m.id)),
                width_mm: Some(m.width_mm),
                diameter_mm: None,
                density_g_cm3: None,
                spool_g: None,
                nozzle_c: None,
                bed_c: None,
                source: None,
            });
        }
    }
    out.extend(filaments().iter().cloned());
    out
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/stock", get(page))
        // Spoolman's label links, for labels printed before a move here.
        .route(
            "/spool/show/{id}",
            get(|Path(id): Path<i64>| async move {
                axum::response::Redirect::to(&format!("/stock#{}", spool_code(id)))
            }),
        )
        .route("/api/stock", get(overview))
        .route("/api/stock/materials", post(new_material))
        .route("/api/stock/materials/{id}", post(change_material))
        .route("/api/stock/locations", post(new_location))
        .route(
            "/api/stock/locations/{id}",
            post(change_location).delete(remove_location),
        )
        .route("/api/stock/receive", post(receive))
        .route("/api/stock/items/{id}/move", post(move_item))
        .route("/api/stock/items/{id}/load", post(load_item))
        .route("/api/stock/items/{id}/unload", post(unload_item))
        .route("/api/stock/items/{id}/amount", post(set_amount))
        .route("/api/stock/items/{id}/used-up", post(use_up))
        .route("/api/stock/items/{id}/movements", get(movements))
}

/// The space of the request: from who asks, never from anywhere else.
fn space(access: &crate::access::Access) -> Result<i64, Box<Response>> {
    Ok(access.space)
}

async fn overview(access: crate::access::Access, State(store): State<Shared>) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match (
        store.materials(space),
        store.items(space),
        store.locations(space),
    ) {
        (Ok(materials), Ok(items), Ok(locations)) => Json(serde_json::json!({
            "materials": materials,
            "items": items,
            "locations": locations,
            "catalogue": catalogue(),
            "dryboxes": DRYBOXES,
        }))
        .into_response(),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => failed(e),
    }
}

async fn new_material(
    access: crate::access::Access,
    State(store): State<Shared>,
    Json(material): Json<Material>,
) -> Response {
    if material.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "a material needs a name").into_response();
    }
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.add_material(space, material) {
        Ok(id) => Json(serde_json::json!({ "id": id })).into_response(),
        Err(e) => failed(e),
    }
}

#[derive(Deserialize)]
struct NewLocation {
    name: String,
    #[serde(default)]
    parent: Option<String>,
    /// A dry box: its slots, and which one it is.
    #[serde(default)]
    slots: Option<u32>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct ChangedLocation {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    slots: Option<u32>,
    #[serde(default)]
    model: Option<String>,
}

async fn change_location(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<String>,
    Json(c): Json<ChangedLocation>,
) -> Response {
    let name = c.name.trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "a location needs a name").into_response();
    }
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    if store
        .locations(space)
        .is_ok_and(|all| all.iter().any(|l| l.name == name && l.id != id))
    {
        return (StatusCode::CONFLICT, "another location has that name").into_response();
    }
    match store.update_location(
        space,
        &id,
        name,
        c.description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty()),
        c.slots.map(|n| n.min(64)),
        c.model.as_deref().filter(|m| !m.is_empty()),
    ) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such location").into_response(),
        Err(e) => failed(e),
    }
}

async fn remove_location(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<String>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.remove_location(space, &id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such location").into_response(),
        Err(e) => failed(e),
    }
}

async fn new_location(
    access: crate::access::Access,
    State(store): State<Shared>,
    Json(new): Json<NewLocation>,
) -> Response {
    if new.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "a location needs a name").into_response();
    }
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.add_place(
        space,
        new.name.trim(),
        new.parent.as_deref().filter(|p| !p.is_empty()),
        new.slots.map(|n| n.min(64)),
        new.model.as_deref().filter(|m| !m.is_empty()),
    ) {
        Ok(id) => {
            if let Some(d) = new
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                let _ = store.db.execute(
                    "UPDATE locations SET description = ?1 WHERE id = ?2",
                    params![d, id],
                );
            }
            Json(serde_json::json!({ "id": id })).into_response()
        }
        Err(e) => failed(e),
    }
}

#[derive(Deserialize)]
struct Receive {
    material: String,
    count: u32,
    #[serde(default)]
    location: Option<String>,
}

async fn receive(
    access: crate::access::Access,
    State(store): State<Shared>,
    Json(r): Json<Receive>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.receive(
        space,
        &r.material,
        r.count,
        r.location.as_deref().filter(|l| !l.is_empty()),
    ) {
        Ok(Ok(ids)) => Json(serde_json::json!({ "items": ids })).into_response(),
        Ok(Err(why)) => (StatusCode::BAD_REQUEST, why).into_response(),
        Err(e) => failed(e),
    }
}

#[derive(Deserialize)]
struct Place {
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    machine: Option<String>,
    #[serde(default)]
    amount: Option<f64>,
    #[serde(default)]
    why: Option<String>,
}

async fn move_item(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<String>,
    Json(p): Json<Place>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.move_item(space, &id, p.location.as_deref().filter(|l| !l.is_empty())) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such item").into_response(),
        Err(e) => failed(e),
    }
}

async fn load_item(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<String>,
    Json(p): Json<Place>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let Some(machine) = p.machine else {
        return (StatusCode::BAD_REQUEST, "which machine?").into_response();
    };
    // Loading a spool or a tape is using the machine.
    if let Err(refused) =
        crate::access::check_machine(&store, &access, &machine, crate::access::Level::Use)
    {
        return *refused;
    }
    match store.load_item(space, &id, &machine) {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(why)) => (StatusCode::BAD_REQUEST, why).into_response(),
        Err(e) => failed(e),
    }
}

async fn unload_item(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<String>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.unload_item(space, &id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "not loaded in a machine").into_response(),
        Err(e) => failed(e),
    }
}

async fn set_amount(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<String>,
    Json(p): Json<Place>,
) -> Response {
    let Some(amount) = p.amount.filter(|a| a.is_finite()) else {
        return (StatusCode::BAD_REQUEST, "how much?").into_response();
    };
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.set_amount(space, &id, amount, p.why.as_deref().unwrap_or("adjusted")) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such item").into_response(),
        Err(e) => failed(e),
    }
}

async fn use_up(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<String>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.use_up(space, &id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such item").into_response(),
        Err(e) => failed(e),
    }
}

async fn movements(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<String>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.movements(space, &id) {
        Ok(list) => Json(list).into_response(),
        Err(e) => failed(e),
    }
}

async fn page(access: crate::access::Access, State(store): State<Shared>) -> Response {
    let space = match store.lock().unwrap().space_name(access.space) {
        Ok(name) => name,
        Err(error) => return failed(error),
    };
    Html(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Materials · Maker Connect</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{STYLE}{STOCK_STYLE}</style>
</head>
<body>
{header}
<main class="library-page">
  <div class="page-head">
    <div><h2>Materials</h2><p class="subtitle">Every spool and cassette: what is left, where it is, what is loaded. Spoolman's API speaks here too, for Moonraker, OctoPrint and scales.</p></div>
    <div class="page-actions">
      <button type="button" class="ghost" data-settings>Fields and labels</button>
      <button type="button" class="ghost" data-import>Import from Spoolman</button>
      <button type="button" data-add-spools>Add stock</button>
    </div>
  </div>
  <div class="tiles" data-totals></div>
  <div class="toolbar">
    <nav class="views" role="tablist" aria-label="View">
      <button type="button" role="tab" data-view="library" aria-selected="true">Library</button>
      <button type="button" role="tab" data-view="locations" aria-selected="false">Locations</button>
    </nav>
    <label class="search"><svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="8.5" cy="8.5" r="5.5"/><path d="m13 13 4 4"/></svg><input type="search" data-search placeholder="Search, or scan: S-0007, L-0002, an EAN, a tag" aria-label="Search or scan" autocomplete="off"><button type="button" class="camera" data-camera title="Scan a QR code or barcode with the camera" aria-label="Scan with the camera"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3 7V4h3M14 4h3v3M17 13v3h-3M6 16H3v-3M6 10h8"/></svg></button></label>
    <button type="button" class="ghost filter-button" data-filter-open>+ Filter</button>
    <span class="filters" data-filters></span>
    <label class="pick">Group <select data-group><option value="filament">Filament</option><option value="vendor">Vendor</option><option value="location">Location</option><option value="material">Material</option><option value="none">None</option></select></label>
    <label class="pick">Sort <select data-sort><option value="name">Name</option><option value="remaining">Left</option><option value="last_used">Last used</option><option value="id">Newest</option></select></label>
    <label class="toggle"><input type="checkbox" data-archived> Used up</label>
  </div>
  <div class="library" data-library-view>
    <section class="list" data-list aria-label="Spools"><p class="empty">Loading…</p></section>
    <aside class="inspector" data-inspector aria-label="Details"><p class="empty">Choose a spool, a filament or a vendor.</p></aside>
  </div>
  <div class="board" data-board hidden></div>
</main>
<dialog class="stock-dialog" data-dialog><div data-dialog-body></div></dialog>
<script>
{STOCK_SCRIPT}</script>
</body>
</html>
"##,
        header = header(&space, "", "stock"),
    ))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Received, placed, loaded into a label printer (which then knows its tape), weighed,
    /// used up: every step in the ledger.
    #[test]
    fn the_catalogue_has_prusaslicers_filaments() {
        let all = catalogue();
        let petg = all
            .iter()
            .find(|m| m.brand == "Prusa Polymers" && m.name == "Prusament PETG 1.75 mm")
            .expect("Prusament PETG");
        assert_eq!(petg.spool_g, Some(278.0));
        assert_eq!(petg.density_g_cm3, Some(1.27));
        assert_eq!(petg.nozzle_c, Some(250.0));
        assert!(
            petg.source
                .as_deref()
                .unwrap()
                .starts_with("prusaslicer-settings-prusa-fff@")
        );
        assert!(
            all.iter()
                .any(|m| m.brand == "Devil Design" && m.name.starts_with("Devil Design PETG")),
        );
        assert!(all.iter().filter(|m| m.kind == "filament").count() > 200);
    }

    /// Materials kept before the Spoolman model move over: a brand a vendor, a material a
    /// filament, an item a spool with its number, a used-up one archived.
    #[test]
    fn materials_from_before_move_over() {
        let dir = std::env::temp_dir().join(format!("mc-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.db");
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch(
                "CREATE TABLE materials (id TEXT PRIMARY KEY, space_id INTEGER NOT NULL, data TEXT NOT NULL);
                 CREATE TABLE items (id TEXT PRIMARY KEY, space_id INTEGER NOT NULL, material TEXT NOT NULL,
                     amount REAL NOT NULL, location TEXT, machine TEXT, state TEXT NOT NULL, created_at INTEGER NOT NULL);",
            )
            .unwrap();
            let petg = serde_json::json!({"kind": "filament", "brand": "Prusa Polymers",
                "name": "Prusament PETG 1.75 mm", "color": "#2b2b33", "color_name": "Galaxy Black",
                "unit": "g", "package": 1000.0, "density_g_cm3": 1.27, "diameter_mm": 1.75, "spool_g": 278.0});
            db.execute(
                "INSERT INTO materials VALUES ('m-1', 1, ?1)",
                [petg.to_string()],
            )
            .unwrap();
            db.execute(
                "INSERT INTO items VALUES ('S-0007', 1, 'm-1', 412.0, 'L-0001', NULL, 'stock', 5)",
                [],
            )
            .unwrap();
            db.execute(
                "INSERT INTO items VALUES ('S-0008', 1, 'm-1', 0.0, NULL, NULL, 'used-up', 6)",
                [],
            )
            .unwrap();
        }
        let store = Store::open(path.to_str().unwrap()).unwrap();
        let space = store.default_space().unwrap().0;
        let material = &store.materials(space).unwrap()[0];
        assert_eq!(
            (
                material.brand.as_str(),
                material.color_name.as_deref(),
                material.spool_g
            ),
            ("Prusa Polymers", Some("Galaxy Black"), Some(278.0))
        );
        let items = store.items(space).unwrap();
        assert_eq!(
            items
                .iter()
                .map(|i| (i.id.as_str(), i.amount, i.state.as_str()))
                .collect::<Vec<_>>(),
            [("S-0007", 412.0, "stock"), ("S-0008", 0.0, "used-up")]
        );
        let spool = store.sm_spool(space, 7).unwrap().unwrap();
        assert_eq!(spool["filament"]["material"], "PETG");
        assert_eq!(spool["filament"]["vendor"]["name"], "Prusa Polymers");
        assert_eq!(spool["used_weight"], 588.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_item_goes_through_its_life() {
        let mut store = Store::open(":memory:").unwrap();
        let (space, _) = store.default_space().unwrap();
        let dymo = store
            .add_virtual(space, profiles::find("dymo/labelmanager-pnp").unwrap())
            .unwrap();
        let tape = catalogue()
            .into_iter()
            .find(|m| m.media.as_deref() == Some("dymo/labelmanager-pnp:45018"))
            .unwrap();
        let material = store.add_material(space, tape.clone()).unwrap();
        let named = Material {
            color_name: Some("Black on Yellow".into()),
            ..tape
        };
        assert!(store.update_material(space, &material, named).unwrap());
        assert_eq!(
            store.materials(space).unwrap()[0].color_name.as_deref(),
            Some("Black on Yellow")
        );
        let shelf = store.add_location(space, "Shelf B2", None).unwrap();
        assert_eq!(shelf, "L-0001");
        let items = store
            .receive(space, &material, 2, Some(&shelf))
            .unwrap()
            .unwrap();
        assert_eq!(items, ["S-0001", "S-0002"]);
        assert_eq!(store.items(space).unwrap()[0].amount, 7.0);

        // Not into a LabelWriter: a LabelManager's cassette.
        let writer = store
            .add_virtual(space, profiles::find("dymo/labelwriter-450").unwrap())
            .unwrap();
        assert_eq!(
            store.load_item(space, "S-0001", &writer).unwrap(),
            Err("this tape or roll is made for another label printer")
        );
        store.load_item(space, "S-0001", &dymo).unwrap().unwrap();
        // From a shelf it is on the printer now; from a box (a place with slots) it stays.
        let loaded = store
            .items(space)
            .unwrap()
            .into_iter()
            .find(|i| i.id == "S-0001")
            .unwrap();
        assert_eq!(
            (loaded.state.as_str(), loaded.location.as_deref()),
            ("loaded", None)
        );
        let dry = store
            .add_place(space, "RepBox", None, Some(6), Some("repkord-repbox-2.5"))
            .unwrap();
        store.move_item(space, "S-0001", Some(&dry)).unwrap();
        store.load_item(space, "S-0001", &dymo).unwrap().unwrap();
        let fed = store
            .items(space)
            .unwrap()
            .into_iter()
            .find(|i| i.id == "S-0001")
            .unwrap();
        assert_eq!(
            (fed.state.as_str(), fed.location.as_deref()),
            ("loaded", Some(dry.as_str()))
        );
        let printer = store.machine(space, &dymo).unwrap().unwrap();
        assert_eq!(
            printer.media.as_deref(),
            Some("45018"),
            "the printer knows its tape"
        );
        // The second loaded: the first goes back to stock.
        store.load_item(space, "S-0002", &dymo).unwrap().unwrap();
        let first = store
            .items(space)
            .unwrap()
            .into_iter()
            .find(|i| i.id == "S-0001")
            .unwrap();
        assert_eq!((first.state.as_str(), first.machine), ("stock", None));

        store.set_amount(space, "S-0002", 4.5, "weighed").unwrap();
        store.use_up(space, "S-0001").unwrap();
        let kinds: Vec<String> = store
            .movements(space, "S-0001")
            .unwrap()
            .into_iter()
            .map(|m| m.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                "used-up", "unloaded", "loaded", "moved", "loaded", "received"
            ]
        );
        let weighed = &store.movements(space, "S-0002").unwrap()[0];
        assert_eq!(
            (weighed.kind.as_str(), weighed.amount),
            ("weighed", Some(-2.5))
        );
    }
}
