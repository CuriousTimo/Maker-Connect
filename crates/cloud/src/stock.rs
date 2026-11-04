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

const STOCK_STYLE: &str = include_str!("stock.css");
const STOCK_SCRIPT: &str = include_str!("stock.js");

/// The tables, made when missing.
pub fn migrate(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS materials (
             id TEXT PRIMARY KEY,
             space_id INTEGER NOT NULL,
             data TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS locations (
             id TEXT PRIMARY KEY,
             space_id INTEGER NOT NULL,
             name TEXT NOT NULL,
             parent TEXT
         );
         CREATE TABLE IF NOT EXISTS items (
             id TEXT PRIMARY KEY,
             space_id INTEGER NOT NULL,
             material TEXT NOT NULL,
             amount REAL NOT NULL,
             location TEXT,
             machine TEXT,
             state TEXT NOT NULL,
             created_at INTEGER NOT NULL
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
         );",
    )
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
    /// Where it came from: `prusaslicer-settings-prusa-fff@2.5.14`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Location {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
}

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

/// The next code of a kind in a space: `S-0001`, `L-0001`.
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
    fn movement(
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
        let mut q = self
            .db
            .prepare("SELECT id, data FROM materials WHERE space_id = ?1")?;
        let rows = q.query_map([space], |r| {
            let id: String = r.get(0)?;
            let data: String = r.get(1)?;
            Ok((id, data))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, data) = row?;
            if let Ok(mut m) = serde_json::from_str::<Material>(&data) {
                m.id = id;
                out.push(m);
            }
        }
        out.sort_by(|a, b| (&a.kind, &a.brand, &a.name).cmp(&(&b.kind, &b.brand, &b.name)));
        Ok(out)
    }

    pub fn add_material(&self, space: i64, mut material: Material) -> rusqlite::Result<String> {
        material.id = format!("m-{}", &crate::store::random_id()[..8]);
        self.db.execute(
            "INSERT INTO materials (id, space_id, data) VALUES (?1, ?2, ?3)",
            params![
                material.id,
                space,
                serde_json::to_string(&material).unwrap_or_default()
            ],
        )?;
        self.changed();
        Ok(material.id)
    }

    /// A material changed: false when there is no such one.
    pub fn update_material(
        &self,
        space: i64,
        id: &str,
        mut material: Material,
    ) -> rusqlite::Result<bool> {
        material.id = id.to_owned();
        let n = self.db.execute(
            "UPDATE materials SET data = ?1 WHERE id = ?2 AND space_id = ?3",
            params![
                serde_json::to_string(&material).unwrap_or_default(),
                id,
                space
            ],
        )?;
        if n > 0 {
            self.changed();
        }
        Ok(n > 0)
    }

    pub fn locations(&self, space: i64) -> rusqlite::Result<Vec<Location>> {
        let mut q = self
            .db
            .prepare("SELECT id, name, parent FROM locations WHERE space_id = ?1 ORDER BY id")?;
        let rows = q.query_map([space], |r| {
            Ok(Location {
                id: r.get(0)?,
                name: r.get(1)?,
                parent: r.get(2)?,
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
        let id = next_code(&self.db, "locations", "L", space)?;
        self.db.execute(
            "INSERT INTO locations (id, space_id, name, parent) VALUES (?1, ?2, ?3, ?4)",
            params![id, space, name, parent],
        )?;
        self.changed();
        Ok(id)
    }

    pub fn items(&self, space: i64) -> rusqlite::Result<Vec<Item>> {
        let mut q = self.db.prepare(
            "SELECT id, material, amount, location, machine, state, created_at FROM items WHERE space_id = ?1 ORDER BY id",
        )?;
        let rows = q.query_map([space], |r| {
            Ok(Item {
                id: r.get(0)?,
                material: r.get(1)?,
                amount: r.get(2)?,
                location: r.get(3)?,
                machine: r.get(4)?,
                state: r.get(5)?,
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
            let id = next_code(&self.db, "items", "S", space)?;
            let amount = m.package.unwrap_or(1.0);
            self.db.execute(
                "INSERT INTO items (id, space_id, material, amount, location, machine, state, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'stock', ?6)",
                params![id, space, material, amount, location, now()],
            )?;
            self.movement(space, &id, "received", Some(amount), location, None)?;
            made.push(id);
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
        let n = self.db.execute(
            "UPDATE items SET location = ?1, machine = NULL, state = CASE WHEN state = 'loaded' THEN 'stock' ELSE state END
             WHERE id = ?2 AND space_id = ?3",
            params![location, id, space],
        )?;
        if n > 0 {
            self.movement(space, id, "moved", None, location, None)?;
            self.changed();
        }
        Ok(n > 0)
    }

    /// Loads an item into a machine: what was loaded there goes back to stock (one spool,
    /// one cassette at a time). A tape of the machine's profile tells the machine its tape.
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
        let others: Vec<String> = self
            .items(space)?
            .into_iter()
            .filter(|i| i.machine.as_deref() == Some(machine) && i.id != id)
            .map(|i| i.id)
            .collect();
        for other in others {
            self.db.execute(
                "UPDATE items SET machine = NULL, state = 'stock' WHERE id = ?1",
                [&other],
            )?;
            self.movement(space, &other, "unloaded", None, Some(machine), None)?;
        }
        self.db.execute(
            "UPDATE items SET machine = ?1, location = NULL, state = 'loaded' WHERE id = ?2",
            params![machine, id],
        )?;
        self.movement(space, id, "loaded", None, Some(machine), None)?;
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

    /// Sets what is left (weighed or counted), the difference recorded.
    pub fn set_amount(
        &self,
        space: i64,
        id: &str,
        amount: f64,
        why: &str,
    ) -> rusqlite::Result<bool> {
        let Some(item) = self.item(space, id)? else {
            return Ok(false);
        };
        self.db.execute(
            "UPDATE items SET amount = ?1 WHERE id = ?2",
            params![amount.max(0.0), id],
        )?;
        let kind = if why == "weighed" {
            "weighed"
        } else {
            "adjusted"
        };
        self.movement(space, id, kind, Some(amount - item.amount), None, None)?;
        self.changed();
        Ok(true)
    }

    pub fn use_up(&self, space: i64, id: &str) -> rusqlite::Result<bool> {
        let n = self.db.execute(
            "UPDATE items SET state = 'used-up', amount = 0, machine = NULL, location = NULL WHERE id = ?1 AND space_id = ?2",
            params![id, space],
        )?;
        if n > 0 {
            self.movement(space, id, "used-up", None, None, None)?;
            self.changed();
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
    State(store): State<Shared>,
    Path(id): Path<String>,
    Json(material): Json<Material>,
) -> Response {
    if material.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "a material needs a name").into_response();
    }
    let store = store.lock().unwrap();
    let space = match space(&store) {
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

/// Materials to start from: the tapes of the machine profiles (the DYMO's, drawn as they
/// look), and the filaments of the catalogue.
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
                unit: "m".into(),
                // A D1 cassette holds 7 m of tape.
                package: Some(7.0),
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
        .route("/api/stock", get(overview))
        .route("/api/stock/materials", post(new_material))
        .route("/api/stock/materials/{id}", post(change_material))
        .route("/api/stock/locations", post(new_location))
        .route("/api/stock/receive", post(receive))
        .route("/api/stock/items/{id}/move", post(move_item))
        .route("/api/stock/items/{id}/load", post(load_item))
        .route("/api/stock/items/{id}/amount", post(set_amount))
        .route("/api/stock/items/{id}/used-up", post(use_up))
        .route("/api/stock/items/{id}/movements", get(movements))
}

fn space(store: &Store) -> Result<i64, Box<Response>> {
    store
        .default_space()
        .map(|(s, _)| s)
        .map_err(|e| Box::new(failed(e)))
}

async fn overview(State(store): State<Shared>) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&store) {
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
        }))
        .into_response(),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => failed(e),
    }
}

async fn new_material(State(store): State<Shared>, Json(material): Json<Material>) -> Response {
    if material.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "a material needs a name").into_response();
    }
    let store = store.lock().unwrap();
    let space = match space(&store) {
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
}

async fn new_location(State(store): State<Shared>, Json(new): Json<NewLocation>) -> Response {
    if new.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "a location needs a name").into_response();
    }
    let store = store.lock().unwrap();
    let space = match space(&store) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.add_location(
        space,
        new.name.trim(),
        new.parent.as_deref().filter(|p| !p.is_empty()),
    ) {
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

async fn receive(State(store): State<Shared>, Json(r): Json<Receive>) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&store) {
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
    State(store): State<Shared>,
    Path(id): Path<String>,
    Json(p): Json<Place>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&store) {
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
    State(store): State<Shared>,
    Path(id): Path<String>,
    Json(p): Json<Place>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&store) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let Some(machine) = p.machine else {
        return (StatusCode::BAD_REQUEST, "which machine?").into_response();
    };
    match store.load_item(space, &id, &machine) {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(why)) => (StatusCode::BAD_REQUEST, why).into_response(),
        Err(e) => failed(e),
    }
}

async fn set_amount(
    State(store): State<Shared>,
    Path(id): Path<String>,
    Json(p): Json<Place>,
) -> Response {
    let Some(amount) = p.amount.filter(|a| a.is_finite()) else {
        return (StatusCode::BAD_REQUEST, "how much?").into_response();
    };
    let store = store.lock().unwrap();
    let space = match space(&store) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.set_amount(space, &id, amount, p.why.as_deref().unwrap_or("adjusted")) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such item").into_response(),
        Err(e) => failed(e),
    }
}

async fn use_up(State(store): State<Shared>, Path(id): Path<String>) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&store) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.use_up(space, &id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such item").into_response(),
        Err(e) => failed(e),
    }
}

async fn movements(State(store): State<Shared>, Path(id): Path<String>) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&store) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.movements(space, &id) {
        Ok(list) => Json(list).into_response(),
        Err(e) => failed(e),
    }
}

async fn page(State(store): State<Shared>) -> Response {
    let space = match store.lock().unwrap().default_space() {
        Ok((_, name)) => name,
        Err(error) => return failed(error),
    };
    Html(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Stock · Maker Connect</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{STYLE}{STOCK_STYLE}</style>
</head>
<body>
{header}
<main>
  <div class="page-head">
    <div><h2>Stock</h2><p class="subtitle">Filament, tapes and the rest: each spool and cassette with a code and a label, where it is, what is loaded.</p></div>
    <div class="page-actions">
      <button type="button" class="ghost" data-new-location>New location</button>
      <button type="button" class="ghost" data-new-material>New material</button>
      <button type="button" data-receive>Receive</button>
    </div>
  </div>
  <form class="scan" data-scan autocomplete="off">
    <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3 5v10M6 5v10M8.5 5v10M11 5v10M14 5v10M17 5v10"/></svg>
    <input data-scan-input placeholder="Scan a code, or type one: S-0007, L-0002, an EAN" aria-label="Scan or type a code">
  </form>
  <div class="tiles" data-totals></div>
  <section class="stock-section"><h3>Materials</h3><div data-materials><p class="empty">Loading…</p></div></section>
  <section class="stock-section"><h3>Locations</h3><div data-locations></div></section>
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

        store.load_item(space, "S-0001", &dymo).unwrap().unwrap();
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
        assert_eq!(kinds, ["used-up", "unloaded", "loaded", "received"]);
        let weighed = &store.movements(space, "S-0002").unwrap()[0];
        assert_eq!(
            (weighed.kind.as_str(), weighed.amount),
            ("weighed", Some(-2.5))
        );
    }
}
