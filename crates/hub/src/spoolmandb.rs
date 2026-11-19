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

use crate::Store;
use crate::store::{Work, now};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
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

pub fn routes() -> Router<Store> {
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
pub async fn take_into(w: &mut Work, f: &Value) -> sqlx::Result<i64> {
    let external = f["id"].as_str().unwrap_or_default();
    if let Some(id) = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM filament WHERE space_id = $1 AND external_id = $2",
    )
    .bind(w.space)
    .bind(external)
    .fetch_optional(w.c())
    .await?
    {
        return Ok(id);
    }
    let maker = f["manufacturer"].as_str().unwrap_or("Unknown");
    let vendor: i64 = match sqlx::query_scalar::<_, i64>(
        "SELECT id FROM vendor WHERE space_id = $1 AND (external_id = $2 OR name = $2)",
    )
    .bind(w.space)
    .bind(maker)
    .fetch_optional(w.c())
    .await?
    {
        Some(id) => id,
        None => {
            let id = sqlx::query_scalar(
                "INSERT INTO vendor (space_id, registered, name, external_id, empty_spool_weight)
                 VALUES ($1, at_unix($2), $3, $3, $4) RETURNING id",
            )
            .bind(w.space)
            .bind(now())
            .bind(maker)
            .bind(f["spool_weight"].as_f64())
            .fetch_one(w.c())
            .await?;
            w.spoolman_event("vendor", "added", id);
            id
        }
    };
    let hexes = f["color_hexes"].as_array().map(|h| {
        h.iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(",")
    });
    let id = sqlx::query_scalar(
        "INSERT INTO filament (space_id, registered, name, vendor_id, material, density, diameter,
             weight, spool_weight, settings_extruder_temp, settings_bed_temp, color_hex,
             multi_color_hexes, multi_color_direction, external_id, minimum)
         VALUES ($1, at_unix($2), $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, 1)
         RETURNING id",
    )
    .bind(w.space)
    .bind(now())
    .bind(f["name"].as_str())
    .bind(vendor)
    .bind(f["material"].as_str())
    .bind(f["density"].as_f64().unwrap_or(1.24))
    .bind(f["diameter"].as_f64().unwrap_or(1.75))
    .bind(f["weight"].as_f64())
    .bind(f["spool_weight"].as_f64())
    .bind(f["extruder_temp"].as_i64())
    .bind(f["bed_temp"].as_i64())
    .bind(f["color_hex"].as_str().map(str::to_lowercase))
    .bind(hexes)
    .bind(f["multi_color_direction"].as_str())
    .bind(external)
    .fetch_one(w.c())
    .await?;
    w.spoolman_event("filament", "added", id);
    w.changed();
    Ok(id)
}

async fn take(
    access: crate::access::Access,
    State(store): State<Store>,
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
    let taken = crate::stock::in_space!(&store, access.space, |w| take_into(&mut w, &f)).await;
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

    /// Into Materials: each with its own id where that is free (in no space), else a new one.
    pub async fn into_store(self, w: &mut Work) -> sqlx::Result<Moved> {
        let space = w.space;
        let mut moved = Moved::default();
        let when = |v: &Value| v.as_str().and_then(crate::spoolman::parse_iso);
        let extra = |v: &Value| {
            v["extra"]
                .as_object()
                .map_or("{}".to_owned(), |m| Value::Object(m.clone()).to_string())
        };
        let mut vendors = HashMap::new();
        for v in &self.vendors {
            let old = v["id"].as_i64().unwrap_or_default();
            let insert = |id: Option<i64>| {
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO vendor (id, space_id, registered, name, empty_spool_weight, comment, external_id, extra)
                     VALUES (COALESCE($1, next_rowid('vendor')), $2, at_unix($3), $4, $5,
                         $6, $7, $8)
                     ON CONFLICT (id) DO NOTHING RETURNING id",
                )
                .bind(id)
                .bind(space)
                .bind(when(&v["registered"]).unwrap_or(now()))
                .bind(v["name"].as_str().unwrap_or("Unknown"))
                .bind(v["empty_spool_weight"].as_f64())
                .bind(v["comment"].as_str())
                .bind(v["external_id"].as_str())
                .bind(extra(v))
            };
            let new = match insert(Some(old)).fetch_optional(w.c()).await? {
                Some(id) => id,
                None => insert(None).fetch_one(w.c()).await?,
            };
            vendors.insert(old, new);
            moved.vendors += 1;
        }
        let mut filaments = HashMap::new();
        for f in &self.filaments {
            let old = f["id"].as_i64().unwrap_or_default();
            let vendor = f["vendor"]["id"]
                .as_i64()
                .and_then(|v| vendors.get(&v).copied());
            let insert = |id: Option<i64>| {
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO filament (id, space_id, registered, name, vendor_id, material, price, density,
                         diameter, weight, spool_weight, article_number, comment, settings_extruder_temp,
                         settings_bed_temp, color_hex, multi_color_hexes, multi_color_direction, external_id, extra)
                     VALUES (COALESCE($1, next_rowid('filament')), $2, at_unix($3), $4,
                         $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
                     ON CONFLICT (id) DO NOTHING RETURNING id",
                )
                .bind(id)
                .bind(space)
                .bind(when(&f["registered"]).unwrap_or(now()))
                .bind(f["name"].as_str())
                .bind(vendor)
                .bind(f["material"].as_str())
                .bind(f["price"].as_f64())
                .bind(f["density"].as_f64().unwrap_or(1.24))
                .bind(f["diameter"].as_f64().unwrap_or(1.75))
                .bind(f["weight"].as_f64())
                .bind(f["spool_weight"].as_f64())
                .bind(f["article_number"].as_str())
                .bind(f["comment"].as_str())
                .bind(f["settings_extruder_temp"].as_i64())
                .bind(f["settings_bed_temp"].as_i64())
                .bind(f["color_hex"].as_str())
                .bind(f["multi_color_hexes"].as_str())
                .bind(f["multi_color_direction"].as_str())
                .bind(f["external_id"].as_str())
                .bind(extra(f))
            };
            let new = match insert(Some(old)).fetch_optional(w.c()).await? {
                Some(id) => id,
                None => insert(None).fetch_one(w.c()).await?,
            };
            filaments.insert(old, new);
            moved.tags += tags_into(w, "filament", new, &f["tags"]).await?;
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
            let place = match s["location"].as_str() {
                Some(name) => w.location_named(name).await?,
                None => None,
            };
            let insert = |id: Option<i64>| {
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO spool (id, space_id, registered, first_used, last_used, price, filament_id,
                         initial_weight, spool_weight, used_weight, location_id, lot_nr, comment, archived, extra)
                     VALUES (COALESCE($1, next_rowid('spool')), $2, at_unix($3),
                         at_unix($4), at_unix($5), $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
                     ON CONFLICT (id) DO NOTHING RETURNING id",
                )
                .bind(id)
                .bind(space)
                .bind(when(&s["registered"]).unwrap_or(now()))
                .bind(when(&s["first_used"]))
                .bind(when(&s["last_used"]))
                .bind(s["price"].as_f64())
                .bind(filament)
                .bind(s["initial_weight"].as_f64())
                .bind(s["spool_weight"].as_f64())
                .bind(s["used_weight"].as_f64().unwrap_or(0.0))
                .bind(place.clone())
                .bind(s["lot_nr"].as_str())
                .bind(s["comment"].as_str())
                .bind(s["archived"].as_bool().unwrap_or(false))
                .bind(extra(s))
            };
            let new = match insert(Some(old)).fetch_optional(w.c()).await? {
                Some(id) => id,
                None => insert(None).fetch_one(w.c()).await?,
            };
            w.movement(
                &crate::stock::spool_code(new),
                "received",
                s["remaining_weight"].as_f64(),
                None,
                Some("from Spoolman"),
            )
            .await?;
            moved.tags += tags_into(w, "spool", new, &s["tags"]).await?;
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
                sqlx::query(
                    "INSERT INTO extra_field (space_id, entity_type, key, definition) VALUES ($1, $2, $3, $4)
                     ON CONFLICT (space_id, entity_type, key) DO UPDATE SET definition = excluded.definition",
                )
                .bind(space)
                .bind(entity)
                .bind(key)
                .bind(definition.to_string())
                .execute(w.c())
                .await?;
                moved.fields += 1;
            }
        }
        w.changed();
        Ok(moved)
    }
}

async fn tags_into(w: &mut Work, target: &str, id: i64, tags: &Value) -> sqlx::Result<usize> {
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
        n += sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO tag (space_id, uid, target_type, {column}, format, added)
             VALUES ($1, $2, $3, $4, $5, at_unix($6)) ON CONFLICT DO NOTHING"
        )))
        .bind(w.space)
        .bind(uid)
        .bind(target)
        .bind(id)
        .bind(tag["format"].as_str())
        .bind(now())
        .execute(w.c())
        .await?
        .rows_affected() as usize;
    }
    Ok(n)
}

/// `{"url": "http://host:7912"}` for a running Spoolman, or its JSON export of spools.
async fn import(
    access: crate::access::Access,
    State(store): State<Store>,
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
    let moved = crate::stock::in_space!(&store, access.space, |w| bundle.into_store(&mut w)).await;
    match moved {
        Ok(m) => Json(m).into_response(),
        Err(e) => crate::failed(e),
    }
}

/// Everything in Materials, written to a running Spoolman through its API.
async fn export(
    access: crate::access::Access,
    State(store): State<Store>,
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
    // What the spools' `loaded_in` says, read before the lock.
    let names = match store.spool_machine_names(access.space).await {
        Ok(names) => names,
        Err(error) => return crate::failed(error),
    };
    let (vendors, filaments, spools) = {
        let mut w = match store.work(access.space).await {
            Ok(w) => w,
            Err(error) => return crate::failed(error),
        };
        let mut out = Vec::new();
        for r in ["vendor", "filament", "spool"] {
            let ids = w.ids(r).await.unwrap_or_default();
            let mut all = Vec::new();
            for id in ids {
                if let Ok(Some(v)) = w.one(r, id, &names).await {
                    all.push(v);
                }
            }
            out.push(all);
        }
        let spools = out.pop().unwrap_or_default();
        let filaments = out.pop().unwrap_or_default();
        (out.pop().unwrap_or_default(), filaments, spools)
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
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_spoolman_moves_over_and_spoolmandb_is_taken_once(pool: sqlx::PgPool) {
        let pg = Store::for_test(&pool).await;
        let (space, _) = pg.default_space().await.unwrap();
        let mut store = pg.work(space).await.unwrap();
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
        let moved = bundle.into_store(&mut store).await.unwrap();
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
            .sm_spool(12, &crate::spoolman::MachineNames::new())
            .await
            .unwrap()
            .expect("spool 12 kept its id");
        assert_eq!(spool["remaining_weight"], 750.0);
        assert_eq!(spool["location"], "Dry box 1");
        assert_eq!(spool["tags"][0]["uid"], "04A2B3C4");
        assert_eq!(spool["filament"]["vendor"]["name"], "Bambu Lab");
        assert_eq!(store.items().await.unwrap()[0].id, "S-0012");

        let f = json!({"id": "polymaker_pla_polysonicblack_1000_175", "manufacturer": "Polymaker",
            "name": "Polysonic Black", "material": "PLA", "density": 1.23, "weight": 1000,
            "spool_weight": 140, "diameter": 1.75, "color_hex": "1C1C1C", "extruder_temp": 210});
        let first = take_into(&mut store, &f).await.unwrap();
        assert_eq!(
            take_into(&mut store, &f).await.unwrap(),
            first,
            "taken once"
        );
        let taken = store.sm_filament(first).await.unwrap().unwrap();
        assert_eq!(taken["vendor"]["name"], "Polymaker");
        assert_eq!(taken["color_hex"], "1c1c1c");
        assert_eq!(taken["settings_extruder_temp"], 210);
        store.commit(&pg).await.unwrap();
        // A spool made later does not meet the one that kept its id.
        let mut store = pg.work(space).await.unwrap();
        let more = store
            .receive(&first.to_string(), 12, None)
            .await
            .unwrap()
            .unwrap();
        assert!(!more.contains(&"S-0012".to_owned()), "{more:?}");
        store.commit(&pg).await.unwrap();
    }
}
