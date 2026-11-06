//! The filament catalogue, and moving to and from a Spoolman.
//!
//! - **SpoolmanDB** (github.com/Donkie/SpoolmanDB, MIT): thousands of filaments of their
//!   makers, with colours, weights, empty spools and temperatures, and the materials' facts.
//!   Fetched in the background and every hour after, as Spoolman does, and kept beside the
//!   database so it is there offline. Served as Spoolman serves it (`/api/v1/external/…`).
//!   One is taken into Materials with its maker as a vendor, once (by `external_id`).
//! - **From a Spoolman**: its vendors, filaments, spools, extra fields and tags, read from a
//!   running one (its API) or from its JSON export of spools. Ids are kept where they are
//!   free, so labels and tags that name a spool by its id still find it.
//! - **To a Spoolman**: the same, written to a running one through its API (it gives new ids).

use crate::Shared;
use crate::store::{Store, now};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::RwLock;

const SPOOLMANDB: &str = "https://donkie.github.io/SpoolmanDB";

/// The catalogue as last fetched: its filaments and its materials.
#[derive(Default)]
pub struct Catalogue {
    pub filaments: Vec<Value>,
    pub materials: Vec<Value>,
    /// When it was fetched, or read from the copy beside the database.
    pub fetched: Option<i64>,
}

pub fn catalogue() -> &'static RwLock<Catalogue> {
    static CATALOGUE: std::sync::OnceLock<RwLock<Catalogue>> = std::sync::OnceLock::new();
    CATALOGUE.get_or_init(RwLock::default)
}

/// Reads the copy kept in `dir`, then fetches SpoolmanDB now and every hour, on a thread of
/// its own. Without a network the copy (or nothing) stays.
pub fn start(dir: std::path::PathBuf) {
    let read = |name: &str| -> Option<Vec<Value>> {
        serde_json::from_slice(&std::fs::read(dir.join(name)).ok()?).ok()
    };
    if let (Some(filaments), Some(materials)) = (read("filaments.json"), read("materials.json")) {
        let mut c = catalogue().write().unwrap();
        c.filaments = filaments;
        c.materials = materials;
        c.fetched = std::fs::metadata(dir.join("filaments.json"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
    }
    std::thread::spawn(move || {
        loop {
            match fetch() {
                Ok((filaments, materials)) => {
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(
                        dir.join("filaments.json"),
                        Value::Array(filaments.clone()).to_string(),
                    );
                    let _ = std::fs::write(
                        dir.join("materials.json"),
                        Value::Array(materials.clone()).to_string(),
                    );
                    println!(
                        "SpoolmanDB: {} filaments, {} materials",
                        filaments.len(),
                        materials.len()
                    );
                    let mut c = catalogue().write().unwrap();
                    c.filaments = filaments;
                    c.materials = materials;
                    c.fetched = Some(now());
                }
                Err(error) => println!("SpoolmanDB not fetched: {error}"),
            }
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    });
}

fn fetch() -> Result<(Vec<Value>, Vec<Value>), String> {
    let get = |name: &str| -> Result<Vec<Value>, String> {
        ureq::get(&format!("{SPOOLMANDB}/{name}"))
            .timeout(std::time::Duration::from_secs(60))
            .call()
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())
    };
    Ok((get("filaments.json")?, get("materials.json")?))
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/v1/external/filament", get(all_filaments))
        .route("/api/v1/external/filament/search", get(search))
        .route("/api/v1/external/material", get(all_materials))
        .route("/api/stock/catalogue/{external_id}/take", post(take))
        .route("/api/stock/import/spoolman", post(import))
        .route("/api/stock/export/spoolman", post(export))
}

async fn all_filaments() -> Json<Value> {
    Json(Value::Array(catalogue().read().unwrap().filaments.clone()))
}

async fn all_materials() -> Json<Value> {
    Json(Value::Array(catalogue().read().unwrap().materials.clone()))
}

/// Filaments whose maker, name and material hold every word of the query.
pub fn find(query: &str) -> Vec<Value> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    catalogue()
        .read()
        .unwrap()
        .filaments
        .iter()
        .filter(|f| {
            let hay = format!(
                "{} {} {}",
                f["manufacturer"].as_str().unwrap_or(""),
                f["name"].as_str().unwrap_or(""),
                f["material"].as_str().unwrap_or("")
            )
            .to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .cloned()
        .collect()
}

async fn search(Query(q): Query<HashMap<String, String>>) -> Response {
    let found = find(q.get("q").map_or("", String::as_str));
    let limit = q
        .get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(30usize)
        .clamp(1, 100);
    let offset = q.get("offset").and_then(|o| o.parse().ok()).unwrap_or(0);
    let total = found.len();
    let page: Vec<Value> = found.into_iter().skip(offset).take(limit).collect();
    ([("x-total-count", total.to_string())], Json(page)).into_response()
}

/// A SpoolmanDB filament taken into Materials (its maker a vendor), or the one taken before:
/// its filament id.
pub fn take_into(store: &Store, space: i64, f: &Value) -> rusqlite::Result<i64> {
    let external = f["id"].as_str().unwrap_or_default();
    if let Some(id) = store
        .db
        .query_row(
            "SELECT id FROM filament WHERE space_id = ?1 AND external_id = ?2",
            params![space, external],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    let maker = f["manufacturer"].as_str().unwrap_or("Unknown");
    let vendor: i64 = match store
        .db
        .query_row(
            "SELECT id FROM vendor WHERE space_id = ?1 AND (external_id = ?2 OR name = ?2)",
            params![space, maker],
            |r| r.get(0),
        )
        .optional()?
    {
        Some(id) => id,
        None => {
            store.db.execute(
                "INSERT INTO vendor (space_id, registered, name, external_id, empty_spool_weight)
                 VALUES (?1, ?2, ?3, ?3, ?4)",
                params![space, now(), maker, f["spool_weight"].as_f64()],
            )?;
            let id = store.db.last_insert_rowid();
            store.spoolman_event("vendor", "added", id);
            id
        }
    };
    let hexes = f["color_hexes"].as_array().map(|h| {
        h.iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(",")
    });
    store.db.execute(
        "INSERT INTO filament (space_id, registered, name, vendor_id, material, density, diameter,
             weight, spool_weight, settings_extruder_temp, settings_bed_temp, color_hex,
             multi_color_hexes, multi_color_direction, external_id, minimum)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 1)",
        params![
            space,
            now(),
            f["name"].as_str(),
            vendor,
            f["material"].as_str(),
            f["density"].as_f64().unwrap_or(1.24),
            f["diameter"].as_f64().unwrap_or(1.75),
            f["weight"].as_f64(),
            f["spool_weight"].as_f64(),
            f["extruder_temp"].as_i64(),
            f["bed_temp"].as_i64(),
            f["color_hex"].as_str().map(str::to_lowercase),
            hexes,
            f["multi_color_direction"].as_str(),
            external,
        ],
    )?;
    let id = store.db.last_insert_rowid();
    store.spoolman_event("filament", "added", id);
    store.changed();
    Ok(id)
}

async fn take(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(external_id): Path<String>,
) -> Response {
    let found = catalogue()
        .read()
        .unwrap()
        .filaments
        .iter()
        .find(|f| f["id"] == external_id.as_str())
        .cloned();
    let Some(f) = found else {
        return (StatusCode::NOT_FOUND, "not in SpoolmanDB").into_response();
    };
    let store = store.lock().unwrap();
    let taken = take_into(&store, access.space, &f);
    match taken {
        Ok(id) => Json(json!({"id": id.to_string()})).into_response(),
        Err(e) => crate::failed(e),
    }
}

// ---- from and to a Spoolman -----------------------------------------------------------

/// What came over, by kind.
#[derive(Debug, Default, PartialEq, serde::Serialize)]
pub struct Moved {
    pub vendors: usize,
    pub filaments: usize,
    pub spools: usize,
    pub fields: usize,
    pub tags: usize,
}

/// A Spoolman's data, as its API gives it: vendors, filaments (with their vendor), spools
/// (with their filament), and the extra fields' definitions per kind.
#[derive(Default)]
pub struct Bundle {
    pub vendors: Vec<Value>,
    pub filaments: Vec<Value>,
    pub spools: Vec<Value>,
    pub fields: Vec<(String, Vec<Value>)>,
}

impl Bundle {
    /// From a running Spoolman at `url` (`http://host:7912`).
    pub fn fetch(url: &str) -> Result<Bundle, String> {
        let base = format!("{}/api/v1", url.trim_end_matches('/'));
        let get = |path: &str| -> Result<Vec<Value>, String> {
            ureq::get(&format!("{base}{path}"))
                .timeout(std::time::Duration::from_secs(30))
                .call()
                .map_err(|e| format!("{base}{path}: {e}"))?
                .into_json()
                .map_err(|e| e.to_string())
        };
        let mut fields = Vec::new();
        for entity in ["vendor", "filament", "spool"] {
            fields.push((
                entity.to_owned(),
                get(&format!("/field/{entity}")).unwrap_or_default(),
            ));
        }
        Ok(Bundle {
            vendors: get("/vendor")?,
            filaments: get("/filament")?,
            spools: get("/spool?allow_archived=true")?,
            fields,
        })
    }

    /// From Spoolman's JSON export of spools: each spool holds its filament, which holds its
    /// vendor.
    pub fn from_spools(spools: Vec<Value>) -> Bundle {
        let mut bundle = Bundle::default();
        for spool in &spools {
            let filament = &spool["filament"];
            if filament.is_object() && !bundle.filaments.iter().any(|f| f["id"] == filament["id"]) {
                bundle.filaments.push(filament.clone());
            }
            let vendor = &filament["vendor"];
            if vendor.is_object() && !bundle.vendors.iter().any(|v| v["id"] == vendor["id"]) {
                bundle.vendors.push(vendor.clone());
            }
        }
        bundle.spools = spools;
        bundle
    }

    /// Into Materials: each with its own id where that is free, else a new one.
    pub fn into_store(self, store: &Store, space: i64) -> rusqlite::Result<Moved> {
        let mut moved = Moved::default();
        let free = |table: &str, id: i64| -> rusqlite::Result<bool> {
            store.db.query_row(
                &format!("SELECT COUNT(*) = 0 FROM {table} WHERE id = ?1"),
                [id],
                |r| r.get(0),
            )
        };
        let when = |v: &Value| v.as_str().and_then(crate::spoolman::parse_iso);
        let extra = |v: &Value| {
            v["extra"]
                .as_object()
                .map_or("{}".to_owned(), |m| Value::Object(m.clone()).to_string())
        };
        let mut vendors = HashMap::new();
        for v in &self.vendors {
            let old = v["id"].as_i64().unwrap_or_default();
            let id = if free("vendor", old)? {
                Some(old)
            } else {
                None
            };
            store.db.execute(
                "INSERT INTO vendor (id, space_id, registered, name, empty_spool_weight, comment, external_id, extra)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    id,
                    space,
                    when(&v["registered"]).unwrap_or(now()),
                    v["name"].as_str().unwrap_or("Unknown"),
                    v["empty_spool_weight"].as_f64(),
                    v["comment"].as_str(),
                    v["external_id"].as_str(),
                    extra(v)
                ],
            )?;
            vendors.insert(old, store.db.last_insert_rowid());
            moved.vendors += 1;
        }
        let mut filaments = HashMap::new();
        for f in &self.filaments {
            let old = f["id"].as_i64().unwrap_or_default();
            let id = if free("filament", old)? {
                Some(old)
            } else {
                None
            };
            let vendor = f["vendor"]["id"]
                .as_i64()
                .and_then(|v| vendors.get(&v).copied());
            store.db.execute(
                "INSERT INTO filament (id, space_id, registered, name, vendor_id, material, price, density,
                     diameter, weight, spool_weight, article_number, comment, settings_extruder_temp,
                     settings_bed_temp, color_hex, multi_color_hexes, multi_color_direction, external_id, extra)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
                params![
                    id,
                    space,
                    when(&f["registered"]).unwrap_or(now()),
                    f["name"].as_str(),
                    vendor,
                    f["material"].as_str(),
                    f["price"].as_f64(),
                    f["density"].as_f64().unwrap_or(1.24),
                    f["diameter"].as_f64().unwrap_or(1.75),
                    f["weight"].as_f64(),
                    f["spool_weight"].as_f64(),
                    f["article_number"].as_str(),
                    f["comment"].as_str(),
                    f["settings_extruder_temp"].as_i64(),
                    f["settings_bed_temp"].as_i64(),
                    f["color_hex"].as_str(),
                    f["multi_color_hexes"].as_str(),
                    f["multi_color_direction"].as_str(),
                    f["external_id"].as_str(),
                    extra(f)
                ],
            )?;
            let new = store.db.last_insert_rowid();
            filaments.insert(old, new);
            moved.tags += tags_into(store, space, "filament", new, &f["tags"])?;
            moved.filaments += 1;
        }
        for s in &self.spools {
            let old = s["id"].as_i64().unwrap_or_default();
            let Some(filament) = s["filament"]["id"]
                .as_i64()
                .or(s["filament_id"].as_i64())
                .and_then(|f| filaments.get(&f).copied())
            else {
                continue;
            };
            let id = if free("spool", old)? { Some(old) } else { None };
            let place = match s["location"].as_str() {
                Some(name) => store.location_named(space, name)?,
                None => None,
            };
            store.db.execute(
                "INSERT INTO spool (id, space_id, registered, first_used, last_used, price, filament_id,
                     initial_weight, spool_weight, used_weight, location_id, lot_nr, comment, archived, extra)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    id,
                    space,
                    when(&s["registered"]).unwrap_or(now()),
                    when(&s["first_used"]),
                    when(&s["last_used"]),
                    s["price"].as_f64(),
                    filament,
                    s["initial_weight"].as_f64(),
                    s["spool_weight"].as_f64(),
                    s["used_weight"].as_f64().unwrap_or(0.0),
                    place,
                    s["lot_nr"].as_str(),
                    s["comment"].as_str(),
                    s["archived"].as_bool().unwrap_or(false),
                    extra(s)
                ],
            )?;
            let new = store.db.last_insert_rowid();
            store.movement(
                space,
                &crate::stock::spool_code(new),
                "received",
                s["remaining_weight"].as_f64(),
                None,
                Some("from Spoolman"),
            )?;
            moved.tags += tags_into(store, space, "spool", new, &s["tags"])?;
            moved.spools += 1;
        }
        for (entity, list) in &self.fields {
            for field in list {
                let Some(key) = field["key"].as_str() else {
                    continue;
                };
                let mut definition = field.clone();
                if let Some(m) = definition.as_object_mut() {
                    m.remove("key");
                    m.remove("entity_type");
                }
                store.db.execute(
                    "INSERT OR REPLACE INTO extra_field (space_id, entity_type, key, definition) VALUES (?1, ?2, ?3, ?4)",
                    params![space, entity, key, definition.to_string()],
                )?;
                moved.fields += 1;
            }
        }
        store.changed();
        Ok(moved)
    }
}

fn tags_into(
    store: &Store,
    space: i64,
    target: &str,
    id: i64,
    tags: &Value,
) -> rusqlite::Result<usize> {
    let mut n = 0;
    let column = if target == "spool" {
        "spool_id"
    } else {
        "filament_id"
    };
    for tag in tags.as_array().into_iter().flatten() {
        let Some(uid) = tag["uid"]
            .as_str()
            .and_then(|u| crate::spoolman::normalize_uid(u).ok())
        else {
            continue;
        };
        n += store.db.execute(
            &format!(
                "INSERT OR IGNORE INTO tag (space_id, uid, target_type, {column}, format, added) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
            ),
            params![space, uid, target, id, tag["format"].as_str(), now()],
        )?;
    }
    Ok(n)
}

/// `{"url": "http://host:7912"}` for a running Spoolman, or its JSON export of spools.
async fn import(
    access: crate::access::Access,
    State(store): State<Shared>,
    Json(body): Json<Value>,
) -> Response {
    let bundle = match (body["url"].as_str(), body.as_array()) {
        (Some(url), _) => {
            let url = url.to_owned();
            match tokio::task::spawn_blocking(move || Bundle::fetch(&url)).await {
                Ok(Ok(b)) => b,
                Ok(Err(why)) => return (StatusCode::BAD_GATEWAY, why).into_response(),
                Err(e) => return crate::failed(e),
            }
        }
        (None, Some(spools)) => Bundle::from_spools(spools.clone()),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "a Spoolman's address ({\"url\": …}) or its JSON export of spools",
            )
                .into_response();
        }
    };
    let store = store.lock().unwrap();
    let moved = bundle.into_store(&store, access.space);
    match moved {
        Ok(m) => Json(m).into_response(),
        Err(e) => crate::failed(e),
    }
}

/// Everything in Materials, written to a running Spoolman through its API.
async fn export(
    access: crate::access::Access,
    State(store): State<Shared>,
    Json(body): Json<Value>,
) -> Response {
    let Some(url) = body["url"]
        .as_str()
        .map(|u| format!("{}/api/v1", u.trim_end_matches('/')))
    else {
        return (
            StatusCode::BAD_REQUEST,
            "a Spoolman's address: {\"url\": …}",
        )
            .into_response();
    };
    let (vendors, filaments, spools) = {
        let store = store.lock().unwrap();
        let space = access.space;
        let all = |r: &str| -> Vec<Value> {
            let mut q = match store.db.prepare(&format!(
                "SELECT id FROM {r} WHERE space_id = ?1 ORDER BY id"
            )) {
                Ok(q) => q,
                Err(_) => return Vec::new(),
            };
            let ids: Vec<i64> = q
                .query_map([space], |row| row.get(0))
                .map(|r| r.flatten().collect())
                .unwrap_or_default();
            ids.into_iter()
                .filter_map(|id| store.one(r, space, id).ok().flatten())
                .collect()
        };
        (all("vendor"), all("filament"), all("spool"))
    };
    let result = tokio::task::spawn_blocking(move || -> Result<Moved, String> {
        let post = |path: &str, body: Value| -> Result<Value, String> {
            ureq::post(&format!("{url}{path}"))
                .timeout(std::time::Duration::from_secs(30))
                .send_json(body)
                .map_err(|e| format!("{path}: {e}"))?
                .into_json()
                .map_err(|e| e.to_string())
        };
        let mut moved = Moved::default();
        let mut vendor_ids = HashMap::new();
        for v in &vendors {
            let made = post("/vendor", json!({
                "name": v["name"], "comment": v["comment"], "empty_spool_weight": v["empty_spool_weight"],
                "external_id": v["external_id"], "extra": v["extra"],
            }))?;
            vendor_ids.insert(v["id"].as_i64(), made["id"].clone());
            moved.vendors += 1;
        }
        let mut filament_ids = HashMap::new();
        for f in &filaments {
            let mut body = f.clone();
            if let Some(m) = body.as_object_mut() {
                for key in ["id", "registered", "vendor", "tags"] {
                    m.remove(key);
                }
                m.insert("vendor_id".into(), vendor_ids.get(&f["vendor"]["id"].as_i64()).cloned().unwrap_or(Value::Null));
            }
            let made = post("/filament", body)?;
            filament_ids.insert(f["id"].as_i64(), made["id"].clone());
            moved.filaments += 1;
        }
        for s in &spools {
            let mut body = s.clone();
            if let Some(m) = body.as_object_mut() {
                for key in ["id", "registered", "filament", "tags", "remaining_weight", "used_length", "remaining_length"] {
                    m.remove(key);
                }
                m.insert("filament_id".into(), filament_ids.get(&s["filament"]["id"].as_i64()).cloned().unwrap_or(Value::Null));
            }
            post("/spool", body)?;
            moved.spools += 1;
        }
        Ok(moved)
    })
    .await;
    match result {
        Ok(Ok(m)) => Json(m).into_response(),
        Ok(Err(why)) => (StatusCode::BAD_GATEWAY, why).into_response(),
        Err(e) => crate::failed(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Spoolman's export comes over with its ids, its tags and its fields; a SpoolmanDB
    /// filament is taken once.
    #[test]
    fn a_spoolman_moves_over_and_spoolmandb_is_taken_once() {
        let store = Store::open(":memory:").unwrap();
        let space = store.default_space().unwrap().0;
        let export = json!([{
            "id": 12, "registered": "2025-03-01T10:00:00Z", "used_weight": 250.0,
            "initial_weight": 1000.0, "location": "Dry box 1", "archived": false,
            "extra": {"last_dried": "\"2025-04-01T00:00:00Z\""},
            "tags": [{"uid": "04A2B3C4", "format": "ntag"}],
            "filament": {"id": 3, "name": "PLA Basic Black", "material": "PLA", "density": 1.24,
                "diameter": 1.75, "weight": 1000.0, "color_hex": "000000",
                "vendor": {"id": 2, "name": "Bambu Lab", "empty_spool_weight": 250.0}}
        }]);
        let mut bundle = Bundle::from_spools(export.as_array().unwrap().clone());
        bundle.fields.push(("spool".into(), vec![json!({"key": "last_dried", "name": "Last dried", "field_type": "datetime", "order": 0})]));
        let moved = bundle.into_store(&store, space).unwrap();
        assert_eq!(
            moved,
            Moved {
                vendors: 1,
                filaments: 1,
                spools: 1,
                fields: 1,
                tags: 1
            }
        );
        let spool = store
            .sm_spool(space, 12)
            .unwrap()
            .expect("spool 12 kept its id");
        assert_eq!(spool["remaining_weight"], 750.0);
        assert_eq!(spool["location"], "Dry box 1");
        assert_eq!(spool["tags"][0]["uid"], "04A2B3C4");
        assert_eq!(spool["filament"]["vendor"]["name"], "Bambu Lab");
        assert_eq!(store.items(space).unwrap()[0].id, "S-0012");

        let f = json!({"id": "polymaker_pla_polysonicblack_1000_175", "manufacturer": "Polymaker",
            "name": "Polysonic Black", "material": "PLA", "density": 1.23, "weight": 1000,
            "spool_weight": 140, "diameter": 1.75, "color_hex": "1C1C1C", "extruder_temp": 210});
        let first = take_into(&store, space, &f).unwrap();
        assert_eq!(take_into(&store, space, &f).unwrap(), first, "taken once");
        let taken = store.sm_filament(space, first).unwrap().unwrap();
        assert_eq!(taken["vendor"]["name"], "Polymaker");
        assert_eq!(taken["color_hex"], "1c1c1c");
        assert_eq!(taken["settings_extruder_temp"], 210);
    }
}
