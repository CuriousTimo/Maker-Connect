//! Consumables and stock (the wiki's design, phase 1): **materials** (what is bought: a
//! filament, a label tape), **items** (what the space has: one spool, one cassette, each with
//! a code such as `S-0007` and a QR label), **locations** (`L-0003`: a shelf, a dry box), and
//! a ledger of **movements**: every change to an item (received, moved, loaded into a
//! machine, weighed, used up) is one, with when; amounts follow from them.

use crate::store::{Work, now};
use crate::{STYLE, Store, failed, header, profiles};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

const STOCK_STYLE: &str = include_str!("library.css");
const STOCK_SCRIPT: &str = include_str!("library.js");

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
async fn next_code(w: &mut Work, table: &str, prefix: &str) -> sqlx::Result<String> {
    let last: Option<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT id FROM {table} WHERE space_id = $1 AND id LIKE $2 ORDER BY id COLLATE \"C\" DESC LIMIT 1"
    )))
    .bind(w.space)
    .bind(format!("{prefix}-%"))
    .fetch_optional(w.c())
    .await?;
    let n = last
        .and_then(|l| l.split_once('-').and_then(|(_, n)| n.parse::<u32>().ok()))
        .unwrap_or(0);
    Ok(format!("{prefix}-{:04}", n + 1))
}

/// A material's row (`filament`), as [`Work::materials`] reads it.
fn material_from(r: &sqlx::postgres::PgRow) -> sqlx::Result<Material> {
    use sqlx::Row;
    let kind: String = r.try_get(1)?;
    let filament = kind == "filament";
    Ok(Material {
        id: r.try_get::<i64, _>(0)?.to_string(),
        kind,
        brand: r.try_get::<Option<String>, _>(2)?.unwrap_or_default(),
        name: r.try_get::<Option<String>, _>(3)?.unwrap_or_default(),
        color: r.try_get::<Option<String>, _>(4)?.map(|c| format!("#{c}")),
        color_name: r.try_get(5)?,
        ink: r.try_get(6)?,
        unit: r.try_get(7)?,
        package: r.try_get(8)?,
        ean: r.try_get(9)?,
        minimum: r.try_get::<Option<i64>, _>(10)?.map(|n| n as u32),
        media: r.try_get(11)?,
        width_mm: r.try_get::<Option<i64>, _>(12)?.map(|n| n as u32),
        diameter_mm: if filament { r.try_get(13)? } else { None },
        density_g_cm3: if filament { r.try_get(14)? } else { None },
        spool_g: r.try_get(15)?,
        nozzle_c: r.try_get::<Option<i64>, _>(16)?.map(|c| c as f64),
        bed_c: r.try_get::<Option<i64>, _>(17)?.map(|c| c as f64),
        source: r.try_get(18)?,
    })
}

/// Rows as read: a location; a spool as an item; a movement.
type LocationRow = (
    String,
    String,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
);
type ItemRow = (i64, i64, f64, Option<String>, Option<String>, bool, i64);
type MovementRow = (String, Option<f64>, Option<String>, Option<String>, i64);

impl Work {
    /// A vendor by its name in the space, made when missing.
    async fn vendor_id(&mut self, name: &str) -> sqlx::Result<Option<i64>> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(None);
        }
        if let Some(id) =
            sqlx::query_scalar::<_, i64>("SELECT id FROM vendor WHERE space_id = $1 AND name = $2")
                .bind(self.space)
                .bind(name)
                .fetch_optional(self.c())
                .await?
        {
            return Ok(Some(id));
        }
        let id = sqlx::query_scalar(
            "INSERT INTO vendor (space_id, registered, name) VALUES ($1, at_unix($2), $3) RETURNING id",
        )
        .bind(self.space)
        .bind(now())
        .bind(name)
        .fetch_one(self.c())
        .await?;
        Ok(Some(id))
    }

    async fn insert_filament(&mut self, m: &Material) -> sqlx::Result<i64> {
        let vendor = self.vendor_id(&m.brand).await?;
        sqlx::query_scalar(
            "INSERT INTO filament (space_id, registered, name, vendor_id, material, density, diameter,
                 weight, spool_weight, article_number, settings_extruder_temp, settings_bed_temp,
                 color_hex, external_id, kind, unit, color_name, ink, media, width_mm, minimum)
             VALUES ($1, at_unix($2), $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18,
                 $19, $20, $21)
             RETURNING id",
        )
        .bind(self.space)
        .bind(now())
        .bind(&m.name)
        .bind(vendor)
        .bind(material_of(m))
        .bind(m.density_g_cm3.unwrap_or(1.24))
        .bind(m.diameter_mm.unwrap_or(1.75))
        .bind(m.package)
        .bind(m.spool_g)
        .bind(&m.ean)
        .bind(m.nozzle_c.map(|c| c.round() as i64))
        .bind(m.bed_c.map(|c| c.round() as i64))
        .bind(hex(m.color.as_deref()))
        .bind(&m.source)
        .bind(&m.kind)
        .bind(&m.unit)
        .bind(&m.color_name)
        .bind(&m.ink)
        .bind(&m.media)
        .bind(m.width_mm.map(i64::from))
        .bind(m.minimum.map(i64::from))
        .fetch_one(self.c())
        .await
    }

    pub(crate) async fn movement(
        &mut self,
        item: &str,
        kind: &str,
        amount: Option<f64>,
        place: Option<&str>,
        note: Option<&str>,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "INSERT INTO movements (space_id, item, kind, amount, place, note, at)
             VALUES ($1, $2, $3, $4, $5, $6, at_unix($7))",
        )
        .bind(self.space)
        .bind(item)
        .bind(kind)
        .bind(amount)
        .bind(place)
        .bind(note)
        .bind(now())
        .execute(self.c())
        .await?;
        Ok(())
    }

    pub async fn materials(&mut self) -> sqlx::Result<Vec<Material>> {
        let rows = sqlx::query(
            "SELECT f.id, f.kind, v.name, f.name, f.color_hex, f.color_name, f.ink, f.unit, f.weight,
                    f.article_number, f.minimum, f.media, f.width_mm, f.diameter, f.density,
                    f.spool_weight, f.settings_extruder_temp, f.settings_bed_temp, f.external_id
             FROM filament f LEFT JOIN vendor v ON v.id = f.vendor_id WHERE f.space_id = $1",
        )
        .bind(self.space)
        .fetch_all(self.c())
        .await?;
        let mut out: Vec<Material> = rows
            .iter()
            .map(material_from)
            .collect::<sqlx::Result<_>>()?;
        out.sort_by(|a, b| (&a.kind, &a.brand, &a.name).cmp(&(&b.kind, &b.brand, &b.name)));
        Ok(out)
    }

    pub async fn add_material(&mut self, material: Material) -> sqlx::Result<String> {
        let id = self.insert_filament(&material).await?;
        self.changed();
        self.spoolman_event("filament", "added", id);
        Ok(id.to_string())
    }

    /// A material changed: false when there is no such one.
    pub async fn update_material(&mut self, id: &str, m: Material) -> sqlx::Result<bool> {
        let Ok(id) = id.parse::<i64>() else {
            return Ok(false);
        };
        let vendor = self.vendor_id(&m.brand).await?;
        let n = sqlx::query(
            "UPDATE filament SET name = $1, vendor_id = $2, material = $3, density = $4, diameter = $5,
                 weight = $6, spool_weight = $7, article_number = $8, settings_extruder_temp = $9,
                 settings_bed_temp = $10, color_hex = $11, kind = $12, unit = $13, color_name = $14,
                 ink = $15, media = $16, width_mm = $17, minimum = $18
             WHERE id = $19 AND space_id = $20",
        )
        .bind(&m.name)
        .bind(vendor)
        .bind(material_of(&m))
        .bind(m.density_g_cm3.unwrap_or(1.24))
        .bind(m.diameter_mm.unwrap_or(1.75))
        .bind(m.package)
        .bind(m.spool_g)
        .bind(&m.ean)
        .bind(m.nozzle_c.map(|c| c.round() as i64))
        .bind(m.bed_c.map(|c| c.round() as i64))
        .bind(hex(m.color.as_deref()))
        .bind(&m.kind)
        .bind(&m.unit)
        .bind(&m.color_name)
        .bind(&m.ink)
        .bind(&m.media)
        .bind(m.width_mm.map(i64::from))
        .bind(m.minimum.map(i64::from))
        .bind(id)
        .bind(self.space)
        .execute(self.c())
        .await?
        .rows_affected();
        if n > 0 {
            self.changed();
            self.spoolman_event("filament", "updated", id);
        }
        Ok(n > 0)
    }

    pub async fn locations(&mut self) -> sqlx::Result<Vec<Location>> {
        let rows: Vec<LocationRow> = sqlx::query_as(
            "SELECT id, name, parent, slots, model, description FROM locations WHERE space_id = $1
                 ORDER BY id COLLATE \"C\"",
        )
        .bind(self.space)
        .fetch_all(self.c())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, name, parent, slots, model, description)| Location {
                id,
                name,
                parent,
                slots: slots.map(|s| s as u32),
                model,
                description,
            })
            .collect())
    }

    pub async fn add_location(&mut self, name: &str, parent: Option<&str>) -> sqlx::Result<String> {
        self.add_place(name, parent, None, None).await
    }

    /// A place, perhaps a dry box with its slots (and which one it is): its code.
    pub async fn add_place(
        &mut self,
        name: &str,
        parent: Option<&str>,
        slots: Option<u32>,
        model: Option<&str>,
    ) -> sqlx::Result<String> {
        let id = next_code(self, "locations", "L").await?;
        sqlx::query(
            "INSERT INTO locations (id, space_id, name, parent, slots, model) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&id)
        .bind(self.space)
        .bind(name)
        .bind(parent)
        .bind(slots.filter(|s| *s > 0).map(i64::from))
        .bind(model)
        .execute(self.c())
        .await?;
        self.changed();
        Ok(id)
    }

    /// A location changed: its name, description, slots, which dry box. Spools in slots it no
    /// longer has come out of them. False when there is no such location.
    pub async fn update_location(
        &mut self,
        id: &str,
        name: &str,
        description: Option<&str>,
        slots: Option<u32>,
        model: Option<&str>,
    ) -> sqlx::Result<bool> {
        let n = sqlx::query(
            "UPDATE locations SET name = $1, description = $2, slots = $3, model = $4
             WHERE id = $5 AND space_id = $6",
        )
        .bind(name)
        .bind(description)
        .bind(slots.filter(|s| *s > 0).map(i64::from))
        .bind(model)
        .bind(id)
        .bind(self.space)
        .execute(self.c())
        .await?
        .rows_affected();
        if n > 0 {
            sqlx::query(
                "UPDATE spool SET slot = NULL WHERE location_id = $1 AND slot > COALESCE($2, 0)",
            )
            .bind(id)
            .bind(slots.map(i64::from))
            .execute(self.c())
            .await?;
            self.changed();
            for spool in self.spools_at(id).await? {
                self.spoolman_event("spool", "updated", spool);
            }
        }
        Ok(n > 0)
    }

    /// A location removed; what was in it has no location then (and stays loaded where it was).
    pub async fn remove_location(&mut self, id: &str) -> sqlx::Result<bool> {
        let spools = self.spools_at(id).await?;
        sqlx::query(
            "UPDATE spool SET location_id = NULL, slot = NULL WHERE location_id = $1 AND space_id = $2",
        )
        .bind(id)
        .bind(self.space)
        .execute(self.c())
        .await?;
        let n = sqlx::query("DELETE FROM locations WHERE id = $1 AND space_id = $2")
            .bind(id)
            .bind(self.space)
            .execute(self.c())
            .await?
            .rows_affected();
        if n > 0 {
            for spool in spools {
                self.movement(
                    &spool_code(spool),
                    "moved",
                    None,
                    None,
                    Some("its location was removed"),
                )
                .await?;
                self.spoolman_event("spool", "updated", spool);
            }
            self.changed();
        }
        Ok(n > 0)
    }

    async fn spools_at(&mut self, location: &str) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar("SELECT id FROM spool WHERE location_id = $1")
            .bind(location)
            .fetch_all(self.c())
            .await
    }

    /// A location by its name (Spoolman keeps locations as names), made when missing: its code.
    pub async fn location_named(&mut self, name: &str) -> sqlx::Result<Option<String>> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(None);
        }
        if let Some(id) = sqlx::query_scalar::<_, String>(
            "SELECT id FROM locations WHERE space_id = $1 AND name = $2",
        )
        .bind(self.space)
        .bind(name)
        .fetch_optional(self.c())
        .await?
        {
            return Ok(Some(id));
        }
        self.add_location(name, None).await.map(Some)
    }

    pub async fn items(&mut self) -> sqlx::Result<Vec<Item>> {
        let rows: Vec<ItemRow> = sqlx::query_as(
            "SELECT id, filament_id, COALESCE(initial_weight, 0) - used_weight, location_id, machine,
                    archived, unix_seconds(registered)
             FROM spool WHERE space_id = $1 ORDER BY id",
        )
        .bind(self.space)
        .fetch_all(self.c())
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, filament, amount, location, machine, archived, registered)| Item {
                    id: spool_code(id),
                    material: filament.to_string(),
                    amount: amount.max(0.0),
                    location,
                    state: if archived {
                        "used-up"
                    } else if machine.is_some() {
                        "loaded"
                    } else {
                        "stock"
                    }
                    .to_owned(),
                    machine,
                    created_at: registered,
                },
            )
            .collect())
    }

    async fn item(&mut self, id: &str) -> sqlx::Result<Option<Item>> {
        Ok(self.items().await?.into_iter().find(|i| i.id == id))
    }

    /// Items received: `count` of a material, each with its package's amount, at a place.
    pub async fn receive(
        &mut self,
        material: &str,
        count: u32,
        location: Option<&str>,
    ) -> sqlx::Result<Result<Vec<String>, &'static str>> {
        let Some(m) = self
            .materials()
            .await?
            .into_iter()
            .find(|m| m.id == material)
        else {
            return Ok(Err("no such material"));
        };
        let filament: i64 = m.id.parse().unwrap_or_default();
        let mut made = Vec::new();
        for _ in 0..count.clamp(1, 200) {
            let amount = m.package.unwrap_or(1.0);
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO spool (space_id, registered, filament_id, initial_weight, spool_weight,
                     used_weight, location_id)
                 VALUES ($1, at_unix($2), $3, $4, $5, 0, $6)
                 RETURNING id",
            )
            .bind(self.space)
            .bind(now())
            .bind(filament)
            .bind(amount)
            .bind(m.spool_g)
            .bind(location)
            .fetch_one(self.c())
            .await?;
            let code = spool_code(id);
            self.movement(&code, "received", Some(amount), location, None)
                .await?;
            self.spoolman_event("spool", "added", id);
            made.push(code);
        }
        self.changed();
        Ok(Ok(made))
    }

    /// Moves an item to a location (out of a machine, if it was loaded).
    pub async fn move_item(&mut self, id: &str, location: Option<&str>) -> sqlx::Result<bool> {
        let Some(number) = spool_number(id) else {
            return Ok(false);
        };
        let n = sqlx::query(
            "UPDATE spool SET location_id = $1, machine = NULL, slot = NULL WHERE id = $2 AND space_id = $3",
        )
        .bind(location)
        .bind(number)
        .bind(self.space)
        .execute(self.c())
        .await?
        .rows_affected();
        if n > 0 {
            self.movement(id, "moved", None, location, None).await?;
            self.changed();
            self.spoolman_event("spool", "updated", number);
        }
        Ok(n > 0)
    }

    /// Loads an item into a machine, where it is (a spool feeds a printer from its dry box):
    /// what was loaded there is no longer (one spool, one cassette at a time). A tape of the
    /// machine's profile tells the machine its tape: the media id returned, for the caller to
    /// set ([`Store::set_media`]). `target`: the machine of the space ([`Store::machine`], None: not
    /// in it), read before.
    pub async fn load_item(
        &mut self,
        id: &str,
        target: Option<&crate::store::Machine>,
    ) -> sqlx::Result<Result<Option<String>, &'static str>> {
        let Some(item) = self.item(id).await? else {
            return Ok(Err("no such item"));
        };
        let Some(target) = target else {
            return Ok(Err("no such machine"));
        };
        let machine = target.id.as_str();
        // A tape into a label printer, filament into a 3D printer; a tape or roll of labels
        // made for one printer model only into that model (a LabelManager's cassette does
        // not go into a LabelWriter). A printer without a profile takes any.
        let material = self
            .materials()
            .await?
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
            .items()
            .await?
            .into_iter()
            .filter(|i| i.machine.as_deref() == Some(machine) && i.id != id)
            .map(|i| i.id)
            .collect();
        for other in others {
            if let Some(number) = spool_number(&other) {
                sqlx::query("UPDATE spool SET machine = NULL WHERE id = $1")
                    .bind(number)
                    .execute(self.c())
                    .await?;
                self.spoolman_event("spool", "updated", number);
            }
            self.movement(&other, "unloaded", None, Some(machine), None)
                .await?;
        }
        let number = spool_number(id).unwrap_or_default();
        sqlx::query(
            // In a box (a place with slots) it stays: the box feeds the printer. From a shelf,
            // or from nowhere, it is on the printer now.
            "UPDATE spool SET machine = $1,
                 location_id = CASE WHEN (SELECT slots FROM locations WHERE id = spool.location_id) > 0
                     THEN location_id ELSE NULL END,
                 slot = CASE WHEN (SELECT slots FROM locations WHERE id = spool.location_id) > 0
                     THEN slot ELSE NULL END
             WHERE id = $2",
        )
        .bind(machine)
        .bind(number)
        .execute(self.c())
        .await?;
        self.movement(id, "loaded", None, Some(machine), None)
            .await?;
        self.spoolman_event("spool", "updated", number);
        // A tape of this printer's profile: the printer now knows which tape it has.
        if let Some(media) = self
            .materials()
            .await?
            .into_iter()
            .find(|m| m.id == item.material)
            .and_then(|m| m.media)
            && let Some((profile, media_id)) = media.split_once(':')
            && target.profile.as_deref() == Some(profile)
        {
            self.changed();
            return Ok(Ok(Some(media_id.to_owned())));
        }
        self.changed();
        Ok(Ok(None))
    }

    /// Takes an item out of its machine, leaving it where it is.
    pub async fn unload_item(&mut self, id: &str) -> sqlx::Result<bool> {
        let Some(number) = spool_number(id) else {
            return Ok(false);
        };
        let machine: Option<String> = sqlx::query_scalar::<_, Option<String>>(
            "SELECT machine FROM spool WHERE id = $1 AND space_id = $2",
        )
        .bind(number)
        .bind(self.space)
        .fetch_optional(self.c())
        .await?
        .flatten();
        let Some(machine) = machine else {
            return Ok(false);
        };
        sqlx::query("UPDATE spool SET machine = NULL WHERE id = $1")
            .bind(number)
            .execute(self.c())
            .await?;
        self.movement(id, "unloaded", None, Some(&machine), None)
            .await?;
        self.changed();
        self.spoolman_event("spool", "updated", number);
        Ok(true)
    }

    /// Sets what is left (weighed or counted), the difference recorded.
    pub async fn set_amount(&mut self, id: &str, amount: f64, why: &str) -> sqlx::Result<bool> {
        let (Some(item), Some(number)) = (self.item(id).await?, spool_number(id)) else {
            return Ok(false);
        };
        let amount = amount.max(0.0);
        // More than it came with: it came with more.
        sqlx::query(
            "UPDATE spool SET initial_weight = GREATEST(COALESCE(initial_weight, 0), $1),
                 used_weight = GREATEST(COALESCE(initial_weight, 0), $1) - $1
             WHERE id = $2",
        )
        .bind(amount)
        .bind(number)
        .execute(self.c())
        .await?;
        let kind = if why == "weighed" {
            "weighed"
        } else {
            "adjusted"
        };
        self.movement(id, kind, Some(amount - item.amount), None, None)
            .await?;
        self.changed();
        self.spoolman_event("spool", "updated", number);
        Ok(true)
    }

    pub async fn use_up(&mut self, id: &str) -> sqlx::Result<bool> {
        let Some(number) = spool_number(id) else {
            return Ok(false);
        };
        let n = sqlx::query(
            "UPDATE spool SET archived = true, used_weight = COALESCE(initial_weight, used_weight),
                 machine = NULL, location_id = NULL
             WHERE id = $1 AND space_id = $2",
        )
        .bind(number)
        .bind(self.space)
        .execute(self.c())
        .await?
        .rows_affected();
        if n > 0 {
            self.movement(id, "used-up", None, None, None).await?;
            self.changed();
            self.spoolman_event("spool", "updated", number);
        }
        Ok(n > 0)
    }

    pub async fn movements(&mut self, item: &str) -> sqlx::Result<Vec<Movement>> {
        let rows: Vec<MovementRow> = sqlx::query_as(
            "SELECT kind, amount, place, note, unix_seconds(at) FROM movements
             WHERE space_id = $1 AND item = $2 ORDER BY id DESC",
        )
        .bind(self.space)
        .bind(item)
        .fetch_all(self.c())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(kind, amount, place, note, at)| Movement {
                kind,
                amount,
                place,
                note,
                at,
            })
            .collect())
    }

    /// What is loaded in each machine of the space, as a machine shows it ([`Store::machines`]).
    pub(crate) async fn loaded(
        &mut self,
    ) -> sqlx::Result<std::collections::HashMap<String, crate::store::Loaded>> {
        let items = self.items().await?;
        let materials = self.materials().await?;
        let mut out = std::collections::HashMap::new();
        for item in items.iter().filter(|i| i.state == "loaded") {
            let Some(machine) = item.machine.clone() else {
                continue;
            };
            if out.contains_key(&machine) {
                continue;
            }
            let material = materials.iter().find(|m| m.id == item.material);
            let kind_material = match item.material.parse::<i64>() {
                Ok(id) => sqlx::query_scalar::<_, Option<String>>(
                    "SELECT material FROM filament WHERE id = $1 AND kind = 'filament'",
                )
                .bind(id)
                .fetch_optional(self.c())
                .await?
                .flatten(),
                Err(_) => None,
            };
            let place = match &item.location {
                Some(code) => {
                    sqlx::query_scalar::<_, String>(
                        "SELECT name FROM locations WHERE id = $1 AND space_id = $2",
                    )
                    .bind(code)
                    .bind(self.space)
                    .fetch_optional(self.c())
                    .await?
                }
                None => None,
            };
            let slot = match spool_number(&item.id) {
                Some(id) => {
                    sqlx::query_scalar::<_, Option<i64>>("SELECT slot FROM spool WHERE id = $1")
                        .bind(id)
                        .fetch_optional(self.c())
                        .await?
                        .flatten()
                }
                None => None,
            };
            out.insert(
                machine,
                crate::store::Loaded {
                    item: item.id.clone(),
                    name: material
                        .map(|m| m.color_name.clone().unwrap_or_else(|| m.name.clone()))
                        .unwrap_or_else(|| item.material.clone()),
                    color: material.and_then(|m| m.color.clone()),
                    nozzle_c: material.and_then(|m| m.nozzle_c),
                    bed_c: material.and_then(|m| m.bed_c),
                    material: kind_material,
                    place,
                    slot,
                    amount: item.amount,
                    unit: material.map_or("pcs".to_owned(), |m| m.unit.clone()),
                },
            );
        }
        Ok(out)
    }
}

/// One change to Materials in its own transaction: `stock!(store, space, |w| w.items())`.
macro_rules! in_space {
    ($store:expr, $space:expr, |$w:ident| $body:expr) => {
        async {
            let mut $w = $store.work($space).await?;
            let out = $body.await?;
            $w.commit($store).await?;
            Ok::<_, sqlx::Error>(out)
        }
    };
}
pub(crate) use in_space;

impl Store {
    pub async fn materials(&self, space: i64) -> sqlx::Result<Vec<Material>> {
        in_space!(self, space, |w| w.materials()).await
    }

    pub async fn add_material(&self, space: i64, material: Material) -> sqlx::Result<String> {
        in_space!(self, space, |w| w.add_material(material)).await
    }

    pub async fn locations(&self, space: i64) -> sqlx::Result<Vec<Location>> {
        in_space!(self, space, |w| w.locations()).await
    }

    pub async fn items(&self, space: i64) -> sqlx::Result<Vec<Item>> {
        in_space!(self, space, |w| w.items()).await
    }

    pub async fn movements(&self, space: i64, item: &str) -> sqlx::Result<Vec<Movement>> {
        in_space!(self, space, |w| w.movements(item)).await
    }
}

/// The filament catalogue: `filaments/*.toml`, made from PrusaSlicer's vendor profiles by the
/// importer, built into the binary.
static FILAMENTS: include_dir::Dir =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../filaments");

async fn change_material(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<String>,
    Json(material): Json<Material>,
) -> Response {
    if material.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "a material needs a name").into_response();
    }
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match in_space!(&store, space, |w| w.update_material(&id, material)).await {
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

pub fn routes() -> Router<Store> {
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

async fn overview(access: crate::access::Access, State(store): State<Store>) -> Response {
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let all = in_space!(&store, space, |w| async {
        Ok::<_, sqlx::Error>((w.materials().await?, w.items().await?, w.locations().await?))
    })
    .await;
    match all {
        Ok((materials, items, locations)) => Json(serde_json::json!({
            "materials": materials,
            "items": items,
            "locations": locations,
            "catalogue": catalogue(),
            "dryboxes": DRYBOXES,
        }))
        .into_response(),
        Err(e) => failed(e),
    }
}

async fn new_material(
    access: crate::access::Access,
    State(store): State<Store>,
    Json(material): Json<Material>,
) -> Response {
    if material.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "a material needs a name").into_response();
    }
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.add_material(space, material).await {
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
    State(store): State<Store>,
    Path(id): Path<String>,
    Json(c): Json<ChangedLocation>,
) -> Response {
    let name = c.name.trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "a location needs a name").into_response();
    }
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return failed(e),
    };
    if w.locations()
        .await
        .is_ok_and(|all| all.iter().any(|l| l.name == name && l.id != id))
    {
        return (StatusCode::CONFLICT, "another location has that name").into_response();
    }
    let changed = w
        .update_location(
            &id,
            name,
            c.description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty()),
            c.slots.map(|n| n.min(64)),
            c.model.as_deref().filter(|m| !m.is_empty()),
        )
        .await;
    let changed = match changed {
        Ok(c) => w.commit(&store).await.map(|()| c),
        Err(e) => Err(e),
    };
    match changed {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such location").into_response(),
        Err(e) => failed(e),
    }
}

async fn remove_location(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<String>,
) -> Response {
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match in_space!(&store, space, |w| w.remove_location(&id)).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such location").into_response(),
        Err(e) => failed(e),
    }
}

async fn new_location(
    access: crate::access::Access,
    State(store): State<Store>,
    Json(new): Json<NewLocation>,
) -> Response {
    if new.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "a location needs a name").into_response();
    }
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let made = in_space!(&store, space, |w| async {
        let id = w
            .add_place(
                new.name.trim(),
                new.parent.as_deref().filter(|p| !p.is_empty()),
                new.slots.map(|n| n.min(64)),
                new.model.as_deref().filter(|m| !m.is_empty()),
            )
            .await?;
        if let Some(d) = new
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            let _ = sqlx::query("UPDATE locations SET description = $1 WHERE id = $2")
                .bind(d)
                .bind(&id)
                .execute(w.c())
                .await;
        }
        Ok::<_, sqlx::Error>(id)
    })
    .await;
    match made {
        Ok(id) => Json(serde_json::json!({ "id": id })).into_response(),
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
    State(store): State<Store>,
    Json(r): Json<Receive>,
) -> Response {
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let location = r.location.as_deref().filter(|l| !l.is_empty());
    match in_space!(&store, space, |w| w.receive(&r.material, r.count, location)).await {
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
    State(store): State<Store>,
    Path(id): Path<String>,
    Json(p): Json<Place>,
) -> Response {
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let location = p.location.as_deref().filter(|l| !l.is_empty());
    match in_space!(&store, space, |w| w.move_item(&id, location)).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such item").into_response(),
        Err(e) => failed(e),
    }
}

async fn load_item(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<String>,
    Json(p): Json<Place>,
) -> Response {
    let grants = match store.grants_of(&access).await {
        Ok(grants) => grants,
        Err(error) => return failed(error),
    };
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let Some(machine) = p.machine else {
        return (StatusCode::BAD_REQUEST, "which machine?").into_response();
    };
    let target = match store.machine(space, &machine).await {
        Ok(target) => target,
        Err(e) => return failed(e),
    };
    {
        let legacy = store.lock().unwrap();
        // Loading a spool or a tape is using the machine.
        if let Err(refused) = crate::access::check_machine(
            &legacy,
            &access,
            &machine,
            crate::access::Level::Use,
            &grants,
        ) {
            return *refused;
        }
    }
    let loaded = in_space!(&store, space, |w| w.load_item(&id, target.as_ref())).await;
    match loaded {
        Ok(Ok(None)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(Some(media))) => match store.set_media(space, &machine, Some(&media)).await {
            Ok(_) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => failed(e),
        },
        Ok(Err(why)) => (StatusCode::BAD_REQUEST, why).into_response(),
        Err(e) => failed(e),
    }
}

async fn unload_item(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<String>,
) -> Response {
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match in_space!(&store, space, |w| w.unload_item(&id)).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "not loaded in a machine").into_response(),
        Err(e) => failed(e),
    }
}

async fn set_amount(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<String>,
    Json(p): Json<Place>,
) -> Response {
    let Some(amount) = p.amount.filter(|a| a.is_finite()) else {
        return (StatusCode::BAD_REQUEST, "how much?").into_response();
    };
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let why = p.why.as_deref().unwrap_or("adjusted");
    match in_space!(&store, space, |w| w.set_amount(&id, amount, why)).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such item").into_response(),
        Err(e) => failed(e),
    }
}

async fn use_up(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<String>,
) -> Response {
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match in_space!(&store, space, |w| w.use_up(&id)).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such item").into_response(),
        Err(e) => failed(e),
    }
}

async fn movements(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<String>,
) -> Response {
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.movements(space, &id).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => failed(e),
    }
}

async fn page(access: crate::access::Access, State(store): State<Store>) -> Response {
    let space = match store.space_name(access.space).await {
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
<main id="main" tabindex="-1" class="library-page">
  <div class="head">
    <div><h1>Materials</h1><p class="lede">Every spool and cassette: what is left, where it is, what is loaded. Spoolman's API speaks here too, for Moonraker, OctoPrint and scales.</p></div>
    <div class="actions">
      <span class="fw"><button type="button" class="btn ghost" data-settings>Fields and labels</button></span>
      <span class="fw"><button type="button" class="btn ghost" data-import>Import from Spoolman</button></span>
      <span class="fw"><button class="btn primary" type="button" data-add-spools>Add stock</button></span>
    </div>
  </div>
  <p class="stock-line" data-totals></p>
  <div class="toolbar">
    <nav class="views" role="tablist" aria-label="View">
      <button type="button" role="tab" data-view="library" aria-selected="true">Library</button>
      <button type="button" role="tab" data-view="locations" aria-selected="false">Locations</button>
    </nav>
    <label class="search"><svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="8.5" cy="8.5" r="5.5"/><path d="m13 13 4 4"/></svg><input type="search" data-search placeholder="Search, or scan: S-0007, L-0002, an EAN, a tag" aria-label="Search or scan" autocomplete="off"><button type="button" class="camera" data-camera title="Scan a QR code or barcode with the camera" aria-label="Scan with the camera"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3 7V4h3M14 4h3v3M17 13v3h-3M6 16H3v-3M6 10h8"/></svg></button></label>
    <span class="fw"><button type="button" class="btn small ghost" data-filter-open>+ Filter</button></span>
    <span class="filters" data-filters></span>
    <label class="pick">Group <select data-group><option value="filament">Filament</option><option value="vendor">Vendor</option><option value="location">Location</option><option value="material">Material</option><option value="none">None</option></select></label>
    <label class="pick">Sort <select data-sort><option value="name">Name</option><option value="remaining">Left</option><option value="last_used">Last used</option><option value="id">Newest</option></select></label>
    <label class="toggle"><input type="checkbox" data-archived> Used up</label>
  </div>
  <div class="chips mat-chips" data-chips role="group" aria-label="Show"></div>
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

    /// A location's code is its space's own (design decisions/20261119-composite-foreign-keys.md):
    /// two spaces each get L-0001, and one's names never block the other's.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn each_space_numbers_its_own_locations(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let (first, _) = store.default_space().await.unwrap();
        let second = store.create_space(first, "Lab").await.unwrap();
        for space in [first, second] {
            let mut w = store.work(space).await.unwrap();
            assert_eq!(
                w.add_place("Shelf", None, None, None).await.unwrap(),
                "L-0001"
            );
            assert_eq!(
                w.add_place("Box", Some("L-0001"), None, None)
                    .await
                    .unwrap(),
                "L-0002"
            );
            w.commit(&store).await.unwrap();
        }
        for space in [first, second] {
            let mut w = store.work(space).await.unwrap();
            let places = w.locations().await.unwrap();
            assert_eq!(places.len(), 2, "space {space}");
            w.commit(&store).await.unwrap();
        }
    }

    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn an_item_goes_through_its_life(pool: sqlx::PgPool) {
        let pg = crate::store::Store::for_test(&pool).await;
        let (space, _) = pg.default_space().await.unwrap();
        let dymo = pg
            .add_virtual(space, profiles::find("dymo/labelmanager-pnp").unwrap())
            .await
            .unwrap();
        let writer = pg
            .add_virtual(space, profiles::find("dymo/labelwriter-450").unwrap())
            .await
            .unwrap();
        let dymo_m = pg.machine(space, &dymo).await.unwrap();
        let writer_m = pg.machine(space, &writer).await.unwrap();
        let media = {
            let mut store = pg.work(space).await.unwrap();
            let tape = catalogue()
                .into_iter()
                .find(|m| m.media.as_deref() == Some("dymo/labelmanager-pnp:45018"))
                .unwrap();
            let material = store.add_material(tape.clone()).await.unwrap();
            let named = Material {
                color_name: Some("Black on Yellow".into()),
                ..tape
            };
            assert!(store.update_material(&material, named).await.unwrap());
            assert_eq!(
                store.materials().await.unwrap()[0].color_name.as_deref(),
                Some("Black on Yellow")
            );
            let shelf = store.add_location("Shelf B2", None).await.unwrap();
            assert_eq!(shelf, "L-0001");
            let items = store
                .receive(&material, 2, Some(&shelf))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(items, ["S-0001", "S-0002"]);
            assert_eq!(store.items().await.unwrap()[0].amount, 7.0);

            // Not into a LabelWriter: a LabelManager's cassette.
            assert_eq!(
                store.load_item("S-0001", writer_m.as_ref()).await.unwrap(),
                Err("this tape or roll is made for another label printer")
            );
            store
                .load_item("S-0001", dymo_m.as_ref())
                .await
                .unwrap()
                .unwrap();
            // From a shelf it is on the printer now; from a box (a place with slots) it stays.
            let loaded = store
                .items()
                .await
                .unwrap()
                .into_iter()
                .find(|i| i.id == "S-0001")
                .unwrap();
            assert_eq!(
                (loaded.state.as_str(), loaded.location.as_deref()),
                ("loaded", None)
            );
            let dry = store
                .add_place("RepBox", None, Some(6), Some("repkord-repbox-2.5"))
                .await
                .unwrap();
            store.move_item("S-0001", Some(&dry)).await.unwrap();
            let media = store
                .load_item("S-0001", dymo_m.as_ref())
                .await
                .unwrap()
                .unwrap();
            let fed = store
                .items()
                .await
                .unwrap()
                .into_iter()
                .find(|i| i.id == "S-0001")
                .unwrap();
            assert_eq!(
                (fed.state.as_str(), fed.location.as_deref()),
                ("loaded", Some(dry.as_str()))
            );
            store.commit(&pg).await.unwrap();
            media
        };
        // What the website's load does next (crate::stock::load_item): the tape on Postgres.
        pg.set_media(space, &dymo, media.as_deref()).await.unwrap();
        let printer = pg.machine(space, &dymo).await.unwrap().unwrap();
        let dymo_m = Some(printer.clone());
        let mut store = pg.work(space).await.unwrap();
        assert_eq!(
            printer.media.as_deref(),
            Some("45018"),
            "the printer knows its tape"
        );
        // The second loaded: the first goes back to stock.
        store
            .load_item("S-0002", dymo_m.as_ref())
            .await
            .unwrap()
            .unwrap();
        let first = store
            .items()
            .await
            .unwrap()
            .into_iter()
            .find(|i| i.id == "S-0001")
            .unwrap();
        assert_eq!((first.state.as_str(), first.machine), ("stock", None));

        store.set_amount("S-0002", 4.5, "weighed").await.unwrap();
        store.use_up("S-0001").await.unwrap();
        let kinds: Vec<String> = store
            .movements("S-0001")
            .await
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
        let weighed = &store.movements("S-0002").await.unwrap()[0];
        assert_eq!(
            (weighed.kind.as_str(), weighed.amount),
            ("weighed", Some(-2.5))
        );
    }
}
