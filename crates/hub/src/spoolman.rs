//! Spoolman's API (`/api/v1`), on Maker Connect's Materials: so what speaks to a Spoolman
//! (Moonraker's `[spoolman]`, OctoPrint's Spoolman plugin, Mainsail, Fluidd, KlipperScreen)
//! can speak to this hub, and a Spoolman's data moves here and back. Its model: vendors,
//! filaments of a vendor, spools of a filament, each with free `extra` fields.
//!
//! Learned from Spoolman (github.com/Donkie/Spoolman, MIT): its API and models, read to be
//! compatible with them. Its shapes, here in Rust:
//!
//! - Lists take filters (`?filament.vendor.name=Prusament`, comma-separated values; names
//!   match in part, ids exactly), `sort=field:asc,other:desc`, `limit`, `offset`, and say how
//!   many there are in `x-total-count`.
//! - `PUT /spool/{id}/use` (`use_weight` g or `use_length` mm) and `PUT /spool/{id}/measure`
//!   (the spool's gross weight) count filament off.
//! - WebSockets (`/api/v1/spool`, `/api/v1/spool/{id}`, the same for `filament`, `vendor`)
//!   send `{"type": "added" | "updated" | "deleted", "resource", "date", "payload"}`.
//!
//! What Spoolman does not have stays Maker Connect's and goes out as `extra` fields: a
//! material's kind and unit (a label tape counts metres), its colour's name, a tape's ink and
//! width, and the machine a spool is loaded in. A spool's number is its code: spool 7 is the
//! label `S-0007`. Locations are places here (with a code and a label); Spoolman's are names,
//! so they go out as their names, and a new name makes a place.

use crate::Shared;
use crate::store::{SpoolmanEvent, Store, now};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use rusqlite::{OptionalExtension, params};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/v1/info", get(info))
        .route(
            "/api/v1/health",
            get(|| async { Json(json!({"status": "healthy"})) }),
        )
        .route("/api/v1/vendor", get(list_vendors).post(add_vendor))
        .route(
            "/api/v1/vendor/{id}",
            get(one_vendor).patch(change_vendor).delete(remove_vendor),
        )
        .route("/api/v1/filament", get(list_filaments).post(add_filament))
        .route(
            "/api/v1/filament/{id}",
            get(one_filament)
                .patch(change_filament)
                .delete(remove_filament),
        )
        .route("/api/v1/spool", get(list_spools).post(add_spool))
        .route(
            "/api/v1/spool/{id}",
            get(one_spool).patch(change_spool).delete(remove_spool),
        )
        .route("/api/v1/spool/{id}/use", put(use_spool))
        .route("/api/v1/spool/{id}/measure", put(measure_spool))
        .route("/api/v1/material", get(materials))
        .route("/api/v1/location", get(locations))
        .route(
            "/api/v1/location/{*name}",
            axum::routing::patch(rename_location),
        )
        .route("/api/v1/lot-number", get(lot_numbers))
        .route("/api/v1/article-number", get(article_numbers))
        .route("/api/v1/setting/", get(settings))
        .route("/api/v1/setting/{key}", get(setting).post(set_setting))
        .route("/api/v1/field/{entity}", get(fields))
        .route(
            "/api/v1/field/{entity}/{key}",
            post(set_field).delete(remove_field),
        )
        .route("/api/v1/field/{entity}/{key}/values", get(field_values))
        .route("/api/v1/spool/{id}/tag", post(tag_spool))
        .route(
            "/api/v1/spool/{id}/tag/{uid}",
            axum::routing::delete(untag_spool),
        )
        .route("/api/v1/filament/{id}/tag", post(tag_filament))
        .route(
            "/api/v1/filament/{id}/tag/{uid}",
            axum::routing::delete(untag_filament),
        )
        .route("/api/v1/tag/scan", post(scan).get(scan_live))
        .route("/api/v1/tag/scan/{reader}", get(scan_live_reader))
        .route("/api/v1/tag/reader", get(readers))
        .route("/api/v1/export/{what}", get(export))
        .route(
            "/api/v1/backup",
            post(|| async { StatusCode::NOT_IMPLEMENTED }),
        )
}

/// A WebSocket upgrade when the request asks for one, else nothing: Spoolman's lists and
/// its live channels share their paths.
pub struct MaybeWs(Option<WebSocketUpgrade>);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for MaybeWs {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let upgrade = parts
            .headers
            .get("upgrade")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
        if !upgrade {
            return Ok(MaybeWs(None));
        }
        Ok(MaybeWs(
            WebSocketUpgrade::from_request_parts(parts, state)
                .await
                .ok(),
        ))
    }
}

// ---- time ----------------------------------------------------------------------------

/// Unix seconds as Spoolman writes dates: `2026-11-04T19:03:00Z`.
pub fn iso(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// A date as Spoolman takes it (`2026-11-04T19:03:00Z`, with or without the zone or the
/// seconds' fraction) as Unix seconds.
pub fn parse_iso(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches('Z');
    let s = s.split(['+']).next()?;
    let (date, time) = s.split_once('T').unwrap_or((s, "00:00:00"));
    let mut d = date.split('-').map(|p| p.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let mut t = time.split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let min: i64 = t.next().unwrap_or("0").parse().ok()?;
    let sec: f64 = t.next().unwrap_or("0").parse().ok()?;
    // days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + min * 60 + sec as i64)
}

// ---- reading -------------------------------------------------------------------------

/// The space of the request: from who asks, never from anywhere else.
fn space(access: &crate::access::Access) -> Result<i64, Box<Response>> {
    Ok(access.space)
}

fn not_found(what: &str, id: i64) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"message": format!("No {what} with ID {id} found.")})),
    )
        .into_response()
}

fn invalid(why: impl Into<String>) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({"message": why.into()})),
    )
        .into_response()
}

fn extra_map(text: Option<String>) -> Map<String, Value> {
    text.and_then(|t| serde_json::from_str::<Map<String, Value>>(&t).ok())
        .unwrap_or_default()
}

/// A value of ours as one of Spoolman's extra fields: a JSON text in a string.
fn extra_value(v: Value) -> Value {
    Value::String(v.to_string())
}

impl Store {
    pub fn sm_vendor(&self, space: i64, id: i64) -> rusqlite::Result<Option<Value>> {
        self.db
            .query_row(
                "SELECT id, registered, name, comment, empty_spool_weight, external_id, extra
                 FROM vendor WHERE id = ?1 AND space_id = ?2",
                params![id, space],
                |r| {
                    Ok(json!({
                        "id": r.get::<_, i64>(0)?,
                        "registered": iso(r.get(1)?),
                        "name": r.get::<_, String>(2)?,
                        "comment": r.get::<_, Option<String>>(3)?,
                        "empty_spool_weight": r.get::<_, Option<f64>>(4)?,
                        "external_id": r.get::<_, Option<String>>(5)?,
                        "extra": extra_map(r.get(6)?),
                    }))
                },
            )
            .optional()
    }

    pub fn sm_filament(&self, space: i64, id: i64) -> rusqlite::Result<Option<Value>> {
        let found = self
            .db
            .query_row(
                "SELECT id, registered, name, vendor_id, material, price, density, diameter, weight,
                        spool_weight, article_number, comment, settings_extruder_temp,
                        settings_bed_temp, color_hex, multi_color_hexes, multi_color_direction,
                        external_id, extra, kind, unit, color_name, ink, media, width_mm, minimum
                 FROM filament WHERE id = ?1 AND space_id = ?2",
                params![id, space],
                |r| {
                    let mut extra = extra_map(r.get(18)?);
                    // Maker Connect's own fields, beside the user's.
                    let kind: String = r.get(19)?;
                    let unit: String = r.get(20)?;
                    if kind != "filament" {
                        extra.insert("kind".into(), extra_value(json!(kind)));
                    }
                    if unit != "g" {
                        extra.insert("unit".into(), extra_value(json!(unit)));
                    }
                    for (key, at) in [("color_name", 21), ("ink", 22), ("media", 23)] {
                        if let Some(v) = r.get::<_, Option<String>>(at)? {
                            extra.insert(key.into(), extra_value(json!(v)));
                        }
                    }
                    for (key, at) in [("width_mm", 24), ("minimum", 25)] {
                        if let Some(v) = r.get::<_, Option<i64>>(at)? {
                            extra.insert(key.into(), extra_value(json!(v)));
                        }
                    }
                    Ok((
                        r.get::<_, Option<i64>>(3)?,
                        json!({
                            "id": r.get::<_, i64>(0)?,
                            "registered": iso(r.get(1)?),
                            "name": r.get::<_, Option<String>>(2)?,
                            "material": r.get::<_, Option<String>>(4)?,
                            "price": r.get::<_, Option<f64>>(5)?,
                            "density": r.get::<_, f64>(6)?,
                            "diameter": r.get::<_, f64>(7)?,
                            "weight": r.get::<_, Option<f64>>(8)?,
                            "spool_weight": r.get::<_, Option<f64>>(9)?,
                            "article_number": r.get::<_, Option<String>>(10)?,
                            "comment": r.get::<_, Option<String>>(11)?,
                            "settings_extruder_temp": r.get::<_, Option<i64>>(12)?,
                            "settings_bed_temp": r.get::<_, Option<i64>>(13)?,
                            "color_hex": r.get::<_, Option<String>>(14)?,
                            "multi_color_hexes": r.get::<_, Option<String>>(15)?,
                            "multi_color_direction": r.get::<_, Option<String>>(16)?,
                            "external_id": r.get::<_, Option<String>>(17)?,
                            "extra": extra,
                            "tags": [],
                        }),
                    ))
                },
            )
            .optional()?;
        let Some((vendor, mut filament)) = found else {
            return Ok(None);
        };
        filament["tags"] = self.tags_of(space, "filament", id)?;
        if let Some(v) = vendor
            .map(|v| self.sm_vendor(space, v))
            .transpose()?
            .flatten()
        {
            filament["vendor"] = v;
        }
        Ok(Some(filament))
    }

    pub fn sm_spool(&self, space: i64, id: i64) -> rusqlite::Result<Option<Value>> {
        let found = self
            .db
            .query_row(
                "SELECT s.id, s.registered, s.first_used, s.last_used, s.price, s.filament_id,
                        s.initial_weight, s.spool_weight, s.used_weight, l.name, s.lot_nr, s.comment,
                        s.archived, s.extra, s.machine, m.name, s.slot
                 FROM spool s
                 LEFT JOIN locations l ON l.id = s.location_id AND l.space_id = s.space_id
                 LEFT JOIN machines m ON m.id = s.machine
                 WHERE s.id = ?1 AND s.space_id = ?2",
                params![id, space],
                |r| {
                    let mut extra = extra_map(r.get(13)?);
                    if let Some(machine) = r.get::<_, Option<String>>(15)? {
                        extra.insert("loaded_in".into(), extra_value(json!(machine)));
                    }
                    // Its slot in a dry box.
                    if let Some(slot) = r.get::<_, Option<i64>>(16)? {
                        extra.insert("slot".into(), extra_value(json!(slot)));
                    }
                    Ok((
                        r.get::<_, i64>(5)?,
                        r.get::<_, Option<f64>>(6)?,
                        r.get::<_, f64>(8)?,
                        json!({
                            "id": r.get::<_, i64>(0)?,
                            "registered": iso(r.get(1)?),
                            "first_used": r.get::<_, Option<i64>>(2)?.map(iso),
                            "last_used": r.get::<_, Option<i64>>(3)?.map(iso),
                            "price": r.get::<_, Option<f64>>(4)?,
                            "initial_weight": r.get::<_, Option<f64>>(6)?,
                            "spool_weight": r.get::<_, Option<f64>>(7)?,
                            "used_weight": r.get::<_, f64>(8)?,
                            "location": r.get::<_, Option<String>>(9)?,
                            "lot_nr": r.get::<_, Option<String>>(10)?,
                            "comment": r.get::<_, Option<String>>(11)?,
                            "archived": r.get::<_, bool>(12)?,
                            "extra": extra,
                            "tags": [],
                        }),
                    ))
                },
            )
            .optional()?;
        let Some((filament_id, initial, used, mut spool)) = found else {
            return Ok(None);
        };
        spool["tags"] = self.tags_of(space, "spool", id)?;
        let filament = self.sm_filament(space, filament_id)?.unwrap_or(Value::Null);
        // What is left, and both as lengths: weight over density and the cross-section.
        let initial = initial.or_else(|| filament["weight"].as_f64());
        let remaining = initial.map(|i| (i - used).max(0.0));
        let density = filament["density"].as_f64().unwrap_or(1.24);
        let diameter = filament["diameter"].as_f64().unwrap_or(1.75);
        let mm_per_g = 1000.0 / (density * std::f64::consts::PI * (diameter / 2.0).powi(2));
        spool["remaining_weight"] = json!(remaining);
        spool["used_length"] = json!(used * mm_per_g);
        spool["remaining_length"] = json!(remaining.map(|r| r * mm_per_g));
        spool["filament"] = filament;
        Ok(Some(spool))
    }

    /// The tags on a spool or a filament: `[{uid, format, added}]`.
    fn tags_of(&self, space: i64, target: &str, id: i64) -> rusqlite::Result<Value> {
        let column = if target == "spool" {
            "spool_id"
        } else {
            "filament_id"
        };
        let mut q = self.db.prepare(&format!(
            "SELECT uid, format, added FROM tag WHERE space_id = ?1 AND target_type = ?2 AND {column} = ?3 ORDER BY added"
        ))?;
        let rows = q.query_map(params![space, target, id], |r| {
            let mut tag = json!({"uid": r.get::<_, String>(0)?, "added": iso(r.get(2)?)});
            if let Some(format) = r.get::<_, Option<String>>(1)? {
                tag["format"] = json!(format);
            }
            Ok(tag)
        })?;
        Ok(Value::Array(rows.collect::<rusqlite::Result<_>>()?))
    }

    fn ids(&self, table: &str, space: i64) -> rusqlite::Result<Vec<i64>> {
        let mut q = self.db.prepare(&format!(
            "SELECT id FROM {table} WHERE space_id = ?1 ORDER BY id"
        ))?;
        let ids = q.query_map([space], |r| r.get(0))?;
        ids.collect()
    }

    fn all(&self, resource: &str, space: i64) -> rusqlite::Result<Vec<Value>> {
        let mut out = Vec::new();
        for id in self.ids(resource, space)? {
            if let Some(v) = self.one(resource, space, id)? {
                out.push(v);
            }
        }
        Ok(out)
    }

    pub fn one(&self, resource: &str, space: i64, id: i64) -> rusqlite::Result<Option<Value>> {
        match resource {
            "vendor" => self.sm_vendor(space, id),
            "filament" => self.sm_filament(space, id),
            "spool" => self.sm_spool(space, id),
            _ => Ok(None),
        }
    }
}

/// A value at a dotted path: `filament.vendor.name`.
fn at<'a>(v: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(v, |v, key| &v[key])
}

/// Whether a value matches a filter: ids (and numbers) exactly, text in part, ignoring case;
/// `-1` or an empty value matches what has none.
fn matches(v: &Value, wanted: &str) -> bool {
    wanted.split(',').any(|w| {
        let w = w.trim();
        match v {
            Value::Null => w.is_empty() || w == "-1" || w.eq_ignore_ascii_case("null"),
            Value::Number(n) => n.to_string() == w,
            Value::String(s) => !w.is_empty() && s.to_lowercase().contains(&w.to_lowercase()),
            Value::Bool(b) => b.to_string() == w,
            _ => false,
        }
    })
}

/// A list as Spoolman gives it: filtered by the query's fields, sorted, a page of it, with
/// the count before paging in `x-total-count`.
fn listing(
    mut items: Vec<Value>,
    query: &HashMap<String, String>,
    filters: &[(&str, &str)],
) -> Response {
    for (param, path) in filters {
        if let Some(wanted) = query.get(*param) {
            items.retain(|v| matches(at(v, path), wanted));
        }
    }
    if let Some(sort) = query.get("sort") {
        let keys: Vec<(String, bool)> = sort
            .split(',')
            .filter(|k| !k.trim().is_empty())
            .map(|k| {
                let (field, order) = k.split_once(':').unwrap_or((k, "asc"));
                (
                    field.trim().to_owned(),
                    order.trim().eq_ignore_ascii_case("desc"),
                )
            })
            .collect();
        items.sort_by(|a, b| {
            for (field, desc) in &keys {
                let ordering = compare(at(a, field), at(b, field));
                let ordering = if *desc { ordering.reverse() } else { ordering };
                if ordering.is_ne() {
                    return ordering;
                }
            }
            std::cmp::Ordering::Equal
        });
    }
    let total = items.len();
    let offset = query
        .get("offset")
        .and_then(|o| o.parse().ok())
        .unwrap_or(0);
    let limit = query
        .get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(usize::MAX);
    let page: Vec<Value> = items.into_iter().skip(offset).take(limit).collect();
    (
        [
            ("x-total-count", total.to_string()),
            ("access-control-expose-headers", "x-total-count".to_owned()),
        ],
        Json(page),
    )
        .into_response()
}

fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    match (a, b) {
        (Value::Null, Value::Null) => Equal,
        (Value::Null, _) => Greater,
        (_, Value::Null) => Less,
        (Value::Number(x), Value::Number(y)) => {
            x.as_f64().partial_cmp(&y.as_f64()).unwrap_or(Equal)
        }
        (Value::String(x), Value::String(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        _ => Equal,
    }
}

async fn info() -> Json<Value> {
    Json(json!({
        "version": "0.23.0",
        "debug_mode": false,
        "automatic_backups": false,
        "data_dir": "",
        "logs_dir": "",
        "backups_dir": "",
        "db_type": "sqlite",
        "external_db_name": "SpoolmanDB",
        "git_commit": null,
        "build_date": null,
        "latest_version": null,
        "update_available": false,
        "server": "Maker Connect",
    }))
}

async fn list_vendors(
    access: crate::access::Access,
    MaybeWs(ws): MaybeWs,
    State(store): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    // The same path is its WebSocket: what is added, changed, removed.
    if let Some(ws) = ws {
        return upgrade(ws, store, access.space, "vendor".to_owned(), None);
    }
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.all("vendor", space) {
        Ok(all) => listing(all, &q, &[("name", "name"), ("external_id", "external_id")]),
        Err(e) => crate::failed(e),
    }
}

async fn list_filaments(
    access: crate::access::Access,
    MaybeWs(ws): MaybeWs,
    State(store): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    // The same path is its WebSocket: what is added, changed, removed.
    if let Some(ws) = ws {
        return upgrade(ws, store, access.space, "filament".to_owned(), None);
    }
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.all("filament", space) {
        Ok(all) => listing(
            all,
            &q,
            &[
                ("vendor.name", "vendor.name"),
                ("vendor.id", "vendor.id"),
                ("vendor_name", "vendor.name"),
                ("vendor_id", "vendor.id"),
                ("name", "name"),
                ("material", "material"),
                ("article_number", "article_number"),
                ("color_hex", "color_hex"),
                ("external_id", "external_id"),
            ],
        ),
        Err(e) => crate::failed(e),
    }
}

async fn list_spools(
    access: crate::access::Access,
    MaybeWs(ws): MaybeWs,
    State(store): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    // The same path is its WebSocket: what is added, changed, removed.
    if let Some(ws) = ws {
        return upgrade(ws, store, access.space, "spool".to_owned(), None);
    }
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut all = match store.all("spool", space) {
        Ok(all) => all,
        Err(e) => return crate::failed(e),
    };
    // Used-up spools only when asked for.
    if q.get("allow_archived").is_none_or(|a| a != "true") {
        all.retain(|s| s["archived"] != true);
    }
    // By a tag's UID, in any spelling.
    if let Some(uid) = q.get("tag").and_then(|t| normalize_uid(t).ok()) {
        all.retain(|s| {
            s["tags"]
                .as_array()
                .is_some_and(|tags| tags.iter().any(|t| t["uid"] == uid))
        });
    }
    listing(
        all,
        &q,
        &[
            ("filament.id", "filament.id"),
            ("filament_id", "filament.id"),
            ("filament.name", "filament.name"),
            ("filament_name", "filament.name"),
            ("filament.material", "filament.material"),
            ("filament_material", "filament.material"),
            ("filament.vendor.name", "filament.vendor.name"),
            ("vendor_name", "filament.vendor.name"),
            ("filament.vendor.id", "filament.vendor.id"),
            ("vendor_id", "filament.vendor.id"),
            ("location", "location"),
            ("lot_nr", "lot_nr"),
        ],
    )
}

fn one_of(access: &crate::access::Access, store: &Shared, resource: &str, id: i64) -> Response {
    let store = store.lock().unwrap();
    let space = match space(access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.one(resource, space, id) {
        Ok(Some(v)) => Json(v).into_response(),
        Ok(None) => not_found(resource, id),
        Err(e) => crate::failed(e),
    }
}

async fn one_vendor(
    access: crate::access::Access,
    MaybeWs(ws): MaybeWs,
    State(store): State<Shared>,
    Path(id): Path<i64>,
) -> Response {
    if let Some(ws) = ws {
        return upgrade(ws, store, access.space, "vendor".to_owned(), Some(id));
    }
    one_of(&access, &store, "vendor", id)
}
async fn one_filament(
    access: crate::access::Access,
    MaybeWs(ws): MaybeWs,
    State(store): State<Shared>,
    Path(id): Path<i64>,
) -> Response {
    if let Some(ws) = ws {
        return upgrade(ws, store, access.space, "filament".to_owned(), Some(id));
    }
    one_of(&access, &store, "filament", id)
}
async fn one_spool(
    access: crate::access::Access,
    MaybeWs(ws): MaybeWs,
    State(store): State<Shared>,
    Path(id): Path<i64>,
) -> Response {
    if let Some(ws) = ws {
        return upgrade(ws, store, access.space, "spool".to_owned(), Some(id));
    }
    one_of(&access, &store, "spool", id)
}

// ---- writing -------------------------------------------------------------------------

/// The `extra` of a body: Spoolman's values are JSON texts in strings.
fn extra_text(body: &Value) -> Option<String> {
    body.get("extra")
        .and_then(Value::as_object)
        .map(|m| Value::Object(m.clone()).to_string())
}

fn text(body: &Value, key: &str) -> Option<Option<String>> {
    body.get(key).map(|v| v.as_str().map(str::to_owned))
}

fn number(body: &Value, key: &str) -> Option<Option<f64>> {
    body.get(key).map(Value::as_f64)
}

/// Sets the columns a body names (absent ones stay; `null` clears).
fn update(
    db: &rusqlite::Connection,
    table: &str,
    id: i64,
    changes: Vec<(&str, rusqlite::types::Value)>,
) -> rusqlite::Result<()> {
    for (column, value) in changes {
        db.execute(
            &format!("UPDATE {table} SET {column} = ?1 WHERE id = ?2"),
            params![value, id],
        )?;
    }
    Ok(())
}

fn real(v: Option<f64>) -> rusqlite::types::Value {
    v.map_or(rusqlite::types::Value::Null, rusqlite::types::Value::Real)
}
fn txt(v: Option<String>) -> rusqlite::types::Value {
    v.map_or(rusqlite::types::Value::Null, rusqlite::types::Value::Text)
}
fn int(v: Option<i64>) -> rusqlite::types::Value {
    v.map_or(
        rusqlite::types::Value::Null,
        rusqlite::types::Value::Integer,
    )
}

fn vendor_changes(body: &Value) -> Vec<(&'static str, rusqlite::types::Value)> {
    let mut c = Vec::new();
    if let Some(v) = text(body, "name") {
        c.push(("name", txt(v)));
    }
    if let Some(v) = text(body, "comment") {
        c.push(("comment", txt(v)));
    }
    if let Some(v) = number(body, "empty_spool_weight") {
        c.push(("empty_spool_weight", real(v)));
    }
    if let Some(v) = text(body, "external_id") {
        c.push(("external_id", txt(v)));
    }
    if let Some(v) = extra_text(body) {
        c.push(("extra", txt(Some(v))));
    }
    c
}

async fn add_vendor(
    access: crate::access::Access,
    State(store): State<Shared>,
    Json(body): Json<Value>,
) -> Response {
    if body["name"].as_str().is_none_or(|n| n.trim().is_empty()) {
        return invalid("a vendor needs a name");
    }
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let made = (|| {
        store.db.execute(
            "INSERT INTO vendor (space_id, registered, name) VALUES (?1, ?2, '')",
            params![space, now()],
        )?;
        let id = store.db.last_insert_rowid();
        update(&store.db, "vendor", id, vendor_changes(&body))?;
        Ok::<_, rusqlite::Error>(id)
    })();
    respond_made(&store, space, "vendor", made)
}

async fn change_vendor(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    if matches!(store.sm_vendor(space, id), Ok(None)) {
        return not_found("vendor", id);
    }
    let done = update(&store.db, "vendor", id, vendor_changes(&body)).map(|()| id);
    respond_changed(&store, space, "vendor", done)
}

fn filament_changes(
    store: &Store,
    space: i64,
    body: &Value,
) -> rusqlite::Result<Vec<(&'static str, rusqlite::types::Value)>> {
    let mut c = Vec::new();
    for key in [
        "name",
        "material",
        "article_number",
        "comment",
        "multi_color_hexes",
        "multi_color_direction",
        "external_id",
    ] {
        if let Some(v) = text(body, key) {
            c.push((key, txt(v)));
        }
    }
    if let Some(v) = text(body, "color_hex") {
        c.push((
            "color_hex",
            txt(v.map(|h| h.trim_start_matches('#').to_lowercase())),
        ));
    }
    for key in ["price", "density", "diameter", "weight", "spool_weight"] {
        if let Some(v) = number(body, key) {
            c.push((key, real(v)));
        }
    }
    for key in ["settings_extruder_temp", "settings_bed_temp"] {
        if let Some(v) = body.get(key) {
            c.push((key, int(v.as_f64().map(|t| t.round() as i64))));
        }
    }
    if let Some(v) = body.get("vendor_id") {
        let vendor = v.as_i64();
        if let Some(vendor) = vendor
            && store.sm_vendor(space, vendor)?.is_none()
        {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        c.push(("vendor_id", int(vendor)));
    }
    // Maker Connect's own fields, when they come back in extra.
    if let Some(extra) = body.get("extra").and_then(Value::as_object) {
        let own = |key: &str| {
            extra
                .get(key)
                .and_then(Value::as_str)
                .and_then(|t| serde_json::from_str::<Value>(t).ok())
        };
        for key in ["kind", "unit", "color_name", "ink", "media"] {
            if let Some(v) = own(key) {
                c.push((key, txt(v.as_str().map(str::to_owned))));
            }
        }
        for key in ["width_mm", "minimum"] {
            if let Some(v) = own(key) {
                c.push((key, int(v.as_i64())));
            }
        }
        let rest: Map<String, Value> = extra
            .iter()
            .filter(|(k, _)| {
                ![
                    "kind",
                    "unit",
                    "color_name",
                    "ink",
                    "media",
                    "width_mm",
                    "minimum",
                ]
                .contains(&k.as_str())
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        c.push(("extra", txt(Some(Value::Object(rest).to_string()))));
    }
    Ok(c)
}

async fn add_filament(
    access: crate::access::Access,
    State(store): State<Shared>,
    Json(body): Json<Value>,
) -> Response {
    for key in ["density", "diameter"] {
        if body[key].as_f64().is_none_or(|v| v <= 0.0) {
            return invalid(format!("a filament needs its {key}"));
        }
    }
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let changes = match filament_changes(&store, space, &body) {
        Ok(c) => c,
        Err(_) => return invalid("no such vendor"),
    };
    let made = (|| {
        store.db.execute(
            "INSERT INTO filament (space_id, registered) VALUES (?1, ?2)",
            params![space, now()],
        )?;
        let id = store.db.last_insert_rowid();
        update(&store.db, "filament", id, changes)?;
        Ok::<_, rusqlite::Error>(id)
    })();
    respond_made(&store, space, "filament", made)
}

async fn change_filament(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    if matches!(store.sm_filament(space, id), Ok(None)) {
        return not_found("filament", id);
    }
    let changes = match filament_changes(&store, space, &body) {
        Ok(c) => c,
        Err(_) => return invalid("no such vendor"),
    };
    let done = update(&store.db, "filament", id, changes).map(|()| id);
    respond_changed(&store, space, "filament", done)
}

fn spool_changes(
    store: &Store,
    space: i64,
    body: &Value,
    spool: Option<i64>,
) -> Result<Vec<(&'static str, rusqlite::types::Value)>, String> {
    let mut c = Vec::new();
    if let Some(v) = body.get("filament_id") {
        let Some(f) = v.as_i64() else {
            return Err("filament_id must be a number".into());
        };
        if !matches!(store.sm_filament(space, f), Ok(Some(_))) {
            return Err(format!("no filament with ID {f}"));
        }
        c.push(("filament_id", int(Some(f))));
    }
    for key in ["price", "initial_weight", "spool_weight", "used_weight"] {
        if let Some(v) = number(body, key) {
            c.push((key, real(v)));
        }
    }
    for key in ["lot_nr", "comment"] {
        if let Some(v) = text(body, key) {
            c.push((key, txt(v)));
        }
    }
    for key in ["first_used", "last_used"] {
        if let Some(v) = text(body, key) {
            c.push((key, int(v.as_deref().and_then(parse_iso))));
        }
    }
    if let Some(v) = body.get("archived") {
        let archived = v.as_bool().unwrap_or(false);
        c.push(("archived", int(Some(archived as i64))));
        if archived {
            c.push(("machine", txt(None)));
        }
    }
    if let Some(v) = text(body, "location") {
        let place = match v {
            Some(name) => store
                .location_named(space, &name)
                .map_err(|e| e.to_string())?,
            None => None,
        };
        c.push(("location_id", txt(place.clone())));
        // Another place: out of its slot (unless one comes with it). Into a box (a place
        // with slots) it stays loaded, the box feeding the printer; onto a shelf it is no
        // longer on the printer.
        let boxed = place.as_deref().is_some_and(|p| {
            store
                .db
                .query_row(
                    "SELECT COALESCE(slots, 0) > 0 FROM locations WHERE id = ?1",
                    [p],
                    |r| r.get(0),
                )
                .unwrap_or(false)
        });
        c.push(("slot", int(None)));
        if place.is_some() && !boxed {
            c.push(("machine", txt(None)));
        }
        // A dry box holds filament, not a label tape.
        if boxed {
            let kind: Option<String> = match (spool, body["filament_id"].as_i64()) {
                (_, Some(f)) => store
                    .db
                    .query_row("SELECT kind FROM filament WHERE id = ?1", [f], |r| r.get(0))
                    .ok(),
                (Some(sp), None) => store
                    .db
                    .query_row(
                        "SELECT f.kind FROM spool s JOIN filament f ON f.id = s.filament_id WHERE s.id = ?1",
                        [sp],
                        |r| r.get(0),
                    )
                    .ok(),
                _ => None,
            };
            if kind.as_deref() == Some("tape") {
                return Err("a dry box holds filament, not a label tape".into());
            }
        }
    }
    if let Some(extra) = body.get("extra").and_then(Value::as_object) {
        if let Some(slot) = extra.get("slot") {
            // `"3"` as Spoolman's clients send it, also `"\"3\""` and a bare 3.
            let value = slot
                .as_str()
                .and_then(|t| serde_json::from_str::<Value>(t).ok())
                .unwrap_or_else(|| slot.clone());
            let n = value
                .as_i64()
                .or_else(|| value.as_str().and_then(|t| t.trim().parse().ok()));
            c.push(("slot", int(n.filter(|n| *n > 0))));
        }
        let rest: Map<String, Value> = extra
            .iter()
            .filter(|(k, _)| !["loaded_in", "slot"].contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        c.push(("extra", txt(Some(Value::Object(rest).to_string()))));
    }
    Ok(c)
}

async fn add_spool(
    access: crate::access::Access,
    State(store): State<Shared>,
    Json(body): Json<Value>,
) -> Response {
    if body["filament_id"].as_i64().is_none() {
        return invalid("a spool needs its filament_id");
    }
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut changes = match spool_changes(&store, space, &body, None) {
        Ok(c) => c,
        Err(why) => return invalid(why),
    };
    // remaining_weight instead of used_weight: used is what the spool came with less it.
    let filament = store
        .sm_filament(space, body["filament_id"].as_i64().unwrap_or_default())
        .ok()
        .flatten()
        .unwrap_or(Value::Null);
    let initial = body["initial_weight"]
        .as_f64()
        .or(filament["weight"].as_f64());
    if body.get("initial_weight").is_none() {
        changes.push(("initial_weight", real(initial)));
    }
    if body.get("spool_weight").is_none() {
        changes.push(("spool_weight", real(filament["spool_weight"].as_f64())));
    }
    if let (Some(remaining), Some(initial)) = (body["remaining_weight"].as_f64(), initial) {
        changes.push(("used_weight", real(Some((initial - remaining).max(0.0)))));
    }
    let made = (|| {
        store.db.execute(
            "INSERT INTO spool (space_id, registered, filament_id) VALUES (?1, ?2, ?3)",
            params![space, now(), body["filament_id"].as_i64()],
        )?;
        let id = store.db.last_insert_rowid();
        update(&store.db, "spool", id, changes)?;
        store.movement(
            space,
            &crate::stock::spool_code(id),
            "received",
            initial,
            None,
            None,
        )?;
        Ok::<_, rusqlite::Error>(id)
    })();
    respond_made(&store, space, "spool", made)
}

async fn change_spool(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let Ok(Some(before)) = store.sm_spool(space, id) else {
        return not_found("spool", id);
    };
    let mut changes = match spool_changes(&store, space, &body, Some(id)) {
        Ok(c) => c,
        Err(why) => return invalid(why),
    };
    if let Some(remaining) = body["remaining_weight"].as_f64() {
        let initial = body["initial_weight"]
            .as_f64()
            .or(before["initial_weight"].as_f64())
            .or(before["filament"]["weight"].as_f64());
        let Some(initial) = initial else {
            return invalid("remaining_weight needs the spool's initial_weight");
        };
        changes.push(("used_weight", real(Some((initial - remaining).max(0.0)))));
    }
    let done = update(&store.db, "spool", id, changes).map(|()| id);
    store.changed();
    respond_changed(&store, space, "spool", done)
}

async fn use_spool(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let Ok(Some(spool)) = store.sm_spool(space, id) else {
        return not_found("spool", id);
    };
    let grams = match (body["use_weight"].as_f64(), body["use_length"].as_f64()) {
        (Some(_), Some(_)) => return invalid("use_weight or use_length, not both"),
        (Some(g), None) => g,
        (None, Some(mm)) => {
            let density = spool["filament"]["density"].as_f64().unwrap_or(1.24);
            let diameter = spool["filament"]["diameter"].as_f64().unwrap_or(1.75);
            mm * std::f64::consts::PI * (diameter / 2.0).powi(2) * density / 1000.0
        }
        (None, None) => return invalid("use_weight or use_length"),
    };
    let done = (|| {
        let t = now();
        store.db.execute(
            "UPDATE spool SET used_weight = MAX(used_weight + ?1, 0),
                 first_used = COALESCE(first_used, ?2), last_used = ?2
             WHERE id = ?3",
            params![grams, t, id],
        )?;
        store.movement(
            space,
            &crate::stock::spool_code(id),
            "used",
            Some(-grams),
            None,
            None,
        )?;
        Ok::<_, rusqlite::Error>(id)
    })();
    store.changed();
    respond_changed(&store, space, "spool", done)
}

async fn measure_spool(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> Response {
    let Some(gross) = body["weight"].as_f64() else {
        return invalid("weight: the spool's gross weight, in g");
    };
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let Ok(Some(spool)) = store.sm_spool(space, id) else {
        return not_found("spool", id);
    };
    // The empty spool: the spool's own, else its filament's, else its vendor's.
    let empty = spool["spool_weight"]
        .as_f64()
        .or(spool["filament"]["spool_weight"].as_f64())
        .or(spool["filament"]["vendor"]["empty_spool_weight"].as_f64())
        .unwrap_or(0.0);
    let Some(initial) = spool["initial_weight"]
        .as_f64()
        .or(spool["filament"]["weight"].as_f64())
    else {
        return invalid("the spool's initial_weight is not known");
    };
    let net = (gross - empty).max(0.0);
    let done = (|| {
        let t = now();
        store.db.execute(
            "UPDATE spool SET used_weight = MAX(?1 - ?2, 0), last_used = ?3 WHERE id = ?4",
            params![initial, net, t, id],
        )?;
        let was = spool["remaining_weight"].as_f64().unwrap_or(net);
        store.movement(
            space,
            &crate::stock::spool_code(id),
            "weighed",
            Some(net - was),
            None,
            None,
        )?;
        Ok::<_, rusqlite::Error>(id)
    })();
    store.changed();
    respond_changed(&store, space, "spool", done)
}

fn respond_made(
    store: &Store,
    space: i64,
    resource: &'static str,
    made: rusqlite::Result<i64>,
) -> Response {
    match made.and_then(|id| Ok((id, store.one(resource, space, id)?))) {
        Ok((id, Some(v))) => {
            store.changed();
            store.spoolman_event(resource, "added", id);
            Json(v).into_response()
        }
        Ok((id, None)) => not_found(resource, id),
        Err(e) => crate::failed(e),
    }
}

fn respond_changed(
    store: &Store,
    space: i64,
    resource: &'static str,
    done: rusqlite::Result<i64>,
) -> Response {
    match done.and_then(|id| Ok((id, store.one(resource, space, id)?))) {
        Ok((id, Some(v))) => {
            store.changed();
            store.spoolman_event(resource, "updated", id);
            Json(v).into_response()
        }
        Ok((id, None)) => not_found(resource, id),
        Err(e) => crate::failed(e),
    }
}

fn remove(
    access: &crate::access::Access,
    store: &Shared,
    resource: &'static str,
    id: i64,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let Ok(Some(was)) = store.one(resource, space, id) else {
        return not_found(resource, id);
    };
    // A vendor with filaments, a filament with spools: Spoolman refuses those too.
    let used: i64 = match resource {
        "vendor" => store
            .db
            .query_row(
                "SELECT COUNT(*) FROM filament WHERE vendor_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap_or(0),
        "filament" => store
            .db
            .query_row(
                "SELECT COUNT(*) FROM spool WHERE filament_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap_or(0),
        _ => 0,
    };
    if used > 0 {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"message": format!("This {resource} still has {used} in it.")})),
        )
            .into_response();
    }
    match store.db.execute(
        &format!("DELETE FROM {resource} WHERE id = ?1 AND space_id = ?2"),
        params![id, space],
    ) {
        Ok(_) => {
            // Its tags go with it.
            if resource != "vendor" {
                let _ = store.db.execute(
                    &format!("DELETE FROM tag WHERE space_id = ?1 AND {resource}_id = ?2"),
                    params![space, id],
                );
            }
            store.changed();
            store.spoolman_removed(space, resource, id, was);
            Json(json!({"message": "Success!"})).into_response()
        }
        Err(e) => crate::failed(e),
    }
}

async fn remove_vendor(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<i64>,
) -> Response {
    remove(&access, &store, "vendor", id)
}
async fn remove_filament(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<i64>,
) -> Response {
    remove(&access, &store, "filament", id)
}
async fn remove_spool(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<i64>,
) -> Response {
    remove(&access, &store, "spool", id)
}

// ---- lists of values, settings, export ----------------------------------------------

fn distinct(access: &crate::access::Access, store: &Shared, sql: &str) -> Response {
    let store = store.lock().unwrap();
    let space = match space(access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let values = store.db.prepare(sql).and_then(|mut q| {
        q.query_map([space], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()
    });
    match values {
        Ok(v) => Json(v).into_response(),
        Err(e) => crate::failed(e),
    }
}

async fn materials(access: crate::access::Access, State(store): State<Shared>) -> Response {
    distinct(
        &access,
        &store,
        "SELECT DISTINCT material FROM filament WHERE space_id = ?1 AND material IS NOT NULL ORDER BY material",
    )
}
async fn locations(access: crate::access::Access, State(store): State<Shared>) -> Response {
    distinct(
        &access,
        &store,
        "SELECT name FROM locations WHERE space_id = ?1 ORDER BY name",
    )
}
/// Spoolman's rename: `PATCH /location/Shelf A` with `{"name": "Shelf B"}`.
async fn rename_location(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(name): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let Some(new) = body["name"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    else {
        return invalid("name");
    };
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let Some(place) = store
        .locations(space)
        .ok()
        .and_then(|all| all.into_iter().find(|l| l.name == name))
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"message": format!("No location {name}.")})),
        )
            .into_response();
    };
    match store.update_location(
        space,
        &place.id,
        new,
        place.description.as_deref(),
        place.slots,
        place.model.as_deref(),
    ) {
        Ok(_) => Json(json!(new)).into_response(),
        Err(e) => crate::failed(e),
    }
}

async fn lot_numbers(access: crate::access::Access, State(store): State<Shared>) -> Response {
    distinct(
        &access,
        &store,
        "SELECT DISTINCT lot_nr FROM spool WHERE space_id = ?1 AND lot_nr IS NOT NULL ORDER BY lot_nr",
    )
}
async fn article_numbers(access: crate::access::Access, State(store): State<Shared>) -> Response {
    distinct(
        &access,
        &store,
        "SELECT DISTINCT article_number FROM filament WHERE space_id = ?1 AND article_number IS NOT NULL ORDER BY article_number",
    )
}

/// Spoolman's settings, with their types and defaults.
const SETTINGS: [(&str, &str, &str); 5] = [
    ("currency", "string", "\"EUR\""),
    ("round_prices", "boolean", "false"),
    ("base_url", "string", "\"\""),
    // Maker Connect's own: what a spool's label says, and what its board groups by.
    (
        "label_template",
        "string",
        "\"{code}\\n{vendor} {name}\\n{material} {color}\"",
    ),
    ("board_field", "string", "\"location\""),
];

fn setting_of(store: &Store, space: i64, key: &str) -> Option<Value> {
    let (_, kind, default) = SETTINGS.iter().find(|(k, _, _)| *k == key)?;
    let set: Option<String> = store
        .db
        .query_row(
            "SELECT value FROM setting WHERE space_id = ?1 AND key = ?2",
            params![space, key],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    Some(json!({
        "value": set.clone().unwrap_or_else(|| (*default).to_owned()),
        "is_set": set.is_some(),
        "type": kind,
    }))
}

async fn settings(access: crate::access::Access, State(store): State<Shared>) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let all: Map<String, Value> = SETTINGS
        .iter()
        .filter_map(|(k, _, _)| Some(((*k).to_owned(), setting_of(&store, space, k)?)))
        .collect();
    Json(Value::Object(all)).into_response()
}

async fn setting(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(key): Path<String>,
) -> Response {
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match setting_of(&store, space, &key) {
        Some(v) => Json(v).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"message": format!("Setting {key} not found.")})),
        )
            .into_response(),
    }
}

async fn set_setting(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(key): Path<String>,
    body: String,
) -> Response {
    if !SETTINGS.iter().any(|(k, _, _)| *k == key) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"message": format!("Setting {key} not found.")})),
        )
            .into_response();
    }
    // The body is the value as JSON text, sent as a JSON string: `"\"USD\""`.
    let value = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or(body);
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match store.db.execute(
        "INSERT INTO setting (space_id, key, value, last_updated) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (space_id, key) DO UPDATE SET value = ?3, last_updated = ?4",
        params![space, key, value, now()],
    ) {
        Ok(_) => Json(setting_of(&store, space, &key)).into_response(),
        Err(e) => crate::failed(e),
    }
}

/// `/export/spools?fmt=csv` (or `json`): every row, nested fields flattened with dots.
async fn export(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(what): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let resource = match what.as_str() {
        "spools" => "spool",
        "filaments" => "filament",
        "vendors" => "vendor",
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let all = {
        let store = store.lock().unwrap();
        let space = match space(&access) {
            Ok(s) => s,
            Err(r) => return *r,
        };
        match store.all(resource, space) {
            Ok(all) => all,
            Err(e) => return crate::failed(e),
        }
    };
    if q.get("fmt").map(String::as_str) == Some("json") {
        return Json(all).into_response();
    }
    let rows: Vec<Vec<(String, String)>> = all.iter().map(|v| flatten("", v)).collect();
    let mut columns: Vec<String> = Vec::new();
    for row in &rows {
        for (k, _) in row {
            if !columns.contains(k) {
                columns.push(k.clone());
            }
        }
    }
    let cell = |s: &str| {
        if s.contains([',', '"', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_owned()
        }
    };
    let mut csv = columns
        .iter()
        .map(|c| cell(c))
        .collect::<Vec<_>>()
        .join(",")
        + "\n";
    for row in rows {
        let line: Vec<String> = columns
            .iter()
            .map(|c| {
                row.iter()
                    .find(|(k, _)| k == c)
                    .map_or(String::new(), |(_, v)| cell(v))
            })
            .collect();
        csv += &line.join(",");
        csv += "\n";
    }
    ([("content-type", "text/csv; charset=utf-8")], csv).into_response()
}

fn flatten(prefix: &str, v: &Value) -> Vec<(String, String)> {
    match v {
        Value::Object(m) => m
            .iter()
            .flat_map(|(k, v)| {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v)
            })
            .collect(),
        Value::Null => vec![(prefix.to_owned(), String::new())],
        Value::String(s) => vec![(prefix.to_owned(), s.clone())],
        other => vec![(prefix.to_owned(), other.to_string())],
    }
}

// ---- live ----------------------------------------------------------------------------

pub fn upgrade(
    ws: WebSocketUpgrade,
    store: Shared,
    space: i64,
    resource: String,
    id: Option<i64>,
) -> Response {
    if !["spool", "filament", "vendor", "setting"].contains(&resource.as_str()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    ws.on_upgrade(move |socket| live(socket, store, space, resource, id))
}

/// A space's changes, as Spoolman sends them; never another space's.
async fn live(mut socket: WebSocket, store: Shared, space: i64, resource: String, id: Option<i64>) {
    let mut events = store.lock().unwrap().subscribe_spoolman();
    // A ping every 30 s, the first after 30 s (not at once: events come first).
    let every = std::time::Duration::from_secs(30);
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    loop {
        tokio::select! {
            event = events.recv() => {
                let event: SpoolmanEvent = match event {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };
                if event.space != Some(space)
                    || event.resource != resource
                    || id.is_some_and(|id| id != event.id)
                {
                    continue;
                }
                let payload = match event.was {
                    Some(was) => was,
                    None => {
                        let store = store.lock().unwrap();
                        match store.one(event.resource, space, event.id) {
                            Ok(Some(v)) => v,
                            _ => continue,
                        }
                    }
                };
                let message = json!({
                    "type": event.kind,
                    "resource": event.resource,
                    "date": iso(now()),
                    "payload": payload,
                });
                if socket.send(Message::Text(message.to_string().into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                if !matches!(incoming, Some(Ok(_))) { break; }
            }
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
            }
        }
    }
}

// ---- tags ----------------------------------------------------------------------------

/// A tag's UID as Spoolman keeps it: uppercase hex, no separators (`04:a2:b3` → `04A2B3`).
pub fn normalize_uid(uid: &str) -> Result<String, String> {
    let cleaned: String = uid
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, ':' | '_' | '.' | '-'))
        .collect::<String>()
        .to_uppercase();
    let cleaned = cleaned.strip_prefix("0X").unwrap_or(&cleaned).to_owned();
    if cleaned.is_empty() {
        return Err("A tag UID cannot be empty.".into());
    }
    if !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("A tag UID is hexadecimal: {uid}"));
    }
    if cleaned.len() > 64 {
        return Err("A tag UID is at most 64 hexadecimal digits.".into());
    }
    Ok(cleaned)
}

/// Links a tag to a spool or a filament: 201 with the tag, 409 with who holds it.
fn link(
    access: &crate::access::Access,
    store: &Shared,
    target: &'static str,
    id: i64,
    body: &Value,
) -> Response {
    let uid = match body["uid"].as_str().map(normalize_uid) {
        Some(Ok(uid)) => uid,
        Some(Err(why)) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"message": why}))).into_response();
        }
        None => return invalid("uid"),
    };
    let format = body["format"].as_str().map(str::to_owned);
    let store = store.lock().unwrap();
    let space = match space(access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    if !matches!(store.one(target, space, id), Ok(Some(_))) {
        return not_found(target, id);
    }
    let holder: Option<(String, Option<i64>, Option<i64>)> = store
        .db
        .query_row(
            "SELECT target_type, spool_id, filament_id FROM tag WHERE space_id = ?1 AND uid = ?2",
            params![space, uid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .ok()
        .flatten();
    let column = if target == "spool" {
        "spool_id"
    } else {
        "filament_id"
    };
    match holder {
        // Already this one's: a format sent now refines the one recorded.
        Some((t, s, f)) if t == target && (if target == "spool" { s } else { f }) == Some(id) => {
            if let Some(format) = &format {
                let _ = store.db.execute(
                    "UPDATE tag SET format = ?1 WHERE space_id = ?2 AND uid = ?3",
                    params![format, space, uid],
                );
            }
        }
        Some((_, s, f)) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "message": format!("Tag {uid} is already linked to something else."),
                    "spool_id": s,
                    "filament_id": f,
                })),
            )
                .into_response();
        }
        None => {
            if let Err(e) = store.db.execute(
                &format!(
                    "INSERT INTO tag (space_id, uid, target_type, {column}, format, added) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
                ),
                params![space, uid, target, id, format, now()],
            ) {
                return crate::failed(e);
            }
        }
    }
    store.changed();
    store.spoolman_event(target, "updated", id);
    let tags = store.tags_of(space, target, id).unwrap_or_default();
    let tag = tags
        .as_array()
        .and_then(|t| t.iter().find(|t| t["uid"] == uid).cloned())
        .unwrap_or(Value::Null);
    (StatusCode::CREATED, Json(tag)).into_response()
}

fn unlink(
    access: &crate::access::Access,
    store: &Shared,
    target: &'static str,
    id: i64,
    uid: &str,
) -> Response {
    let uid = match normalize_uid(uid) {
        Ok(uid) => uid,
        Err(why) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"message": why}))).into_response();
        }
    };
    let store = store.lock().unwrap();
    let space = match space(access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let column = if target == "spool" {
        "spool_id"
    } else {
        "filament_id"
    };
    match store.db.execute(
        &format!("DELETE FROM tag WHERE space_id = ?1 AND uid = ?2 AND {column} = ?3"),
        params![space, uid, id],
    ) {
        Ok(0) => (
            StatusCode::NOT_FOUND,
            Json(json!({"message": format!("Tag {uid} is not on this {target}.")})),
        )
            .into_response(),
        Ok(_) => {
            store.changed();
            store.spoolman_event(target, "updated", id);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => crate::failed(e),
    }
}

async fn tag_spool(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> Response {
    link(&access, &store, "spool", id, &body)
}
async fn untag_spool(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path((id, uid)): Path<(i64, String)>,
) -> Response {
    unlink(&access, &store, "spool", id, &uid)
}
async fn tag_filament(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> Response {
    link(&access, &store, "filament", id, &body)
}
async fn untag_filament(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path((id, uid)): Path<(i64, String)>,
) -> Response {
    unlink(&access, &store, "filament", id, &uid)
}

/// The readers that scanned lately (in memory, as Spoolman's), and the scans as they come,
/// each with its space: a space sees only its own readers and scans.
struct Scans {
    readers: std::sync::Mutex<Vec<(i64, Value)>>,
    sent: tokio::sync::broadcast::Sender<(i64, Value)>,
}

fn scans() -> &'static Scans {
    static SCANS: std::sync::OnceLock<Scans> = std::sync::OnceLock::new();
    SCANS.get_or_init(|| Scans {
        readers: std::sync::Mutex::new(Vec::new()),
        sent: tokio::sync::broadcast::channel(64).0,
    })
}

/// A reader reports a tag it read: what it is linked to comes back, and the scan goes out on
/// the scan WebSockets (so a page paired with that reader can open the spool).
async fn scan(
    access: crate::access::Access,
    State(store): State<Shared>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    Json(body): Json<Value>,
) -> Response {
    let uid = match body["uid"].as_str().map(normalize_uid) {
        Some(Ok(uid)) => uid,
        Some(Err(why)) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"message": why}))).into_response();
        }
        None => return invalid("uid"),
    };
    let reader = body["reader_id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("ip-{}", peer.ip()).replace([':', '.'], "-"));
    let (spool, filament) = {
        let store = store.lock().unwrap();
        let space = match space(&access) {
            Ok(s) => s,
            Err(r) => return *r,
        };
        let held: Option<(Option<i64>, Option<i64>)> = store
            .db
            .query_row(
                "SELECT spool_id, filament_id FROM tag WHERE space_id = ?1 AND uid = ?2",
                params![space, uid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten();
        let (s, f) = held.unwrap_or((None, None));
        (
            s.and_then(|s| store.sm_spool(space, s).ok().flatten()),
            f.and_then(|f| store.sm_filament(space, f).ok().flatten()),
        )
    };
    let result = json!({
        "uid": uid,
        "reader_id": reader,
        "name": body["name"],
        "format": body["format"],
        "matched_spool_id": spool.as_ref().map(|s| s["id"].clone()),
        "spool": spool,
        "matched_filament_id": filament.as_ref().map(|f| f["id"].clone()),
        "filament": filament,
        "decoded": null,
        "created": false,
    });
    {
        let mut readers = scans().readers.lock().unwrap();
        readers.retain(|(s, r)| !(*s == access.space && r["reader_id"] == reader));
        readers.push((
            access.space,
            json!({
                "reader_id": reader,
                "name": body["name"],
                "last_seen": iso(now()),
                "last_uid": uid,
            }),
        ));
    }
    let _ = scans().sent.send((
        access.space,
        json!({
            "type": "added",
            "resource": "tag_scan",
            "date": iso(now()),
            "payload": result,
        }),
    ));
    Json(result).into_response()
}

async fn readers(access: crate::access::Access) -> Json<Value> {
    let readers = scans().readers.lock().unwrap();
    Json(Value::Array(
        readers
            .iter()
            .filter(|(s, _)| *s == access.space)
            .map(|(_, r)| r.clone())
            .collect(),
    ))
}

async fn scan_live(access: crate::access::Access, MaybeWs(ws): MaybeWs) -> Response {
    match ws {
        Some(ws) => ws.on_upgrade(move |socket| scan_feed(socket, access.space, None)),
        None => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

async fn scan_live_reader(
    access: crate::access::Access,
    MaybeWs(ws): MaybeWs,
    Path(reader): Path<String>,
) -> Response {
    match ws {
        Some(ws) => ws.on_upgrade(move |socket| scan_feed(socket, access.space, Some(reader))),
        None => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

async fn scan_feed(mut socket: WebSocket, space: i64, reader: Option<String>) {
    let mut feed = scans().sent.subscribe();
    loop {
        tokio::select! {
            scan = feed.recv() => {
                let Ok((from, scan)) = scan else { break };
                if from != space {
                    continue;
                }
                if reader.as_ref().is_some_and(|r| scan["payload"]["reader_id"] != *r) {
                    continue;
                }
                if socket.send(Message::Text(scan.to_string().into())).await.is_err() { break; }
            }
            incoming = socket.recv() => {
                if !matches!(incoming, Some(Ok(_))) { break; }
            }
        }
    }
}

// ---- extra fields ---------------------------------------------------------------------

const FIELD_TYPES: [&str; 8] = [
    "text",
    "integer",
    "integer_range",
    "float",
    "float_range",
    "datetime",
    "boolean",
    "choice",
];

fn entity(entity: &str) -> Option<&'static str> {
    ["vendor", "filament", "spool"]
        .into_iter()
        .find(|e| *e == entity)
}

fn field_list(store: &Store, space: i64, entity: &str) -> rusqlite::Result<Value> {
    let mut q = store.db.prepare(
        "SELECT key, definition FROM extra_field WHERE space_id = ?1 AND entity_type = ?2",
    )?;
    let rows = q.query_map(params![space, entity], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (key, definition) = row?;
        let mut field: Value = serde_json::from_str(&definition).unwrap_or(json!({}));
        field["key"] = json!(key);
        field["entity_type"] = json!(entity);
        out.push(field);
    }
    out.sort_by_key(|f| f["order"].as_i64().unwrap_or(0));
    Ok(Value::Array(out))
}

async fn fields(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path(e): Path<String>,
) -> Response {
    let Some(e) = entity(&e) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match field_list(&store, space, e) {
        Ok(v) => Json(v).into_response(),
        Err(err) => crate::failed(err),
    }
}

async fn set_field(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path((e, key)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Response {
    let Some(e) = entity(&e) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if key.is_empty()
        || key.len() > 64
        || !key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"message": "A key is lowercase letters, digits and _."})),
        )
            .into_response();
    }
    let Some(kind) = body["field_type"]
        .as_str()
        .filter(|t| FIELD_TYPES.contains(t))
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"message": format!("field_type is one of {}", FIELD_TYPES.join(", "))})),
        )
            .into_response();
    };
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    // Its type, and whether it takes several choices, stay as first defined.
    let before: Option<Value> = store
        .db
        .query_row(
            "SELECT definition FROM extra_field WHERE space_id = ?1 AND entity_type = ?2 AND key = ?3",
            params![space, e, key],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|d| serde_json::from_str(&d).ok());
    if let Some(before) = &before
        && (before["field_type"] != kind || before["multi_choice"] != body["multi_choice"])
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"message": "A field's type cannot change."})),
        )
            .into_response();
    }
    let definition = json!({
        "name": body["name"].as_str().unwrap_or(&key),
        "order": body["order"].as_i64().unwrap_or(0),
        "unit": body["unit"],
        "field_type": kind,
        "default_value": body["default_value"],
        "choices": body["choices"],
        "multi_choice": body["multi_choice"],
    });
    let saved = store.db.execute(
        "INSERT INTO extra_field (space_id, entity_type, key, definition) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (space_id, entity_type, key) DO UPDATE SET definition = ?4",
        params![space, e, key, definition.to_string()],
    );
    match saved.and_then(|_| field_list(&store, space, e)) {
        Ok(v) => Json(v).into_response(),
        Err(err) => crate::failed(err),
    }
}

async fn remove_field(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path((e, key)): Path<(String, String)>,
) -> Response {
    let Some(e) = entity(&e) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let removed = store.db.execute(
        "DELETE FROM extra_field WHERE space_id = ?1 AND entity_type = ?2 AND key = ?3",
        params![space, e, key],
    );
    // Its values go with it, as in Spoolman.
    let mut q = match store
        .db
        .prepare(&format!("SELECT id, extra FROM {e} WHERE space_id = ?1"))
    {
        Ok(q) => q,
        Err(err) => return crate::failed(err),
    };
    let rows: Vec<(i64, String)> = q
        .query_map([space], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    for (id, extra) in rows {
        let mut map = extra_map(Some(extra));
        if map.remove(&key).is_some() {
            let _ = store.db.execute(
                &format!("UPDATE {e} SET extra = ?1 WHERE id = ?2"),
                params![Value::Object(map).to_string(), id],
            );
        }
    }
    match removed.and_then(|_| field_list(&store, space, e)) {
        Ok(v) => Json(v).into_response(),
        Err(err) => crate::failed(err),
    }
}

/// The values a field has now, for suggestions.
async fn field_values(
    access: crate::access::Access,
    State(store): State<Shared>,
    Path((e, key)): Path<(String, String)>,
) -> Response {
    let Some(e) = entity(&e) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let store = store.lock().unwrap();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut values: Vec<Value> = store
        .all(e, space)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v["extra"].get(&key).cloned())
        .collect();
    values.sort_by_key(|v| v.to_string());
    values.dedup();
    Json(values).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::store::Store;
    use futures_util::StreamExt;
    use std::sync::{Arc, Mutex};

    /// What Moonraker, Mainsail and the OctoPrint plugin do with a Spoolman: make a vendor, a
    /// filament and a spool, list and filter them, count filament off by length and by
    /// weighing, and hear about it on the spool's WebSocket.
    #[tokio::test]
    async fn it_answers_as_spoolman_does() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = crate::auth::signed_in_app(store.clone());
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let base = format!("http://{addr}/api/v1");
        macro_rules! go {
            ($m:expr, $p:expr, $b:expr) => {{
                let (base, m, p, b) = (base.clone(), $m.to_owned(), $p.to_owned(), $b);
                tokio::task::spawn_blocking(move || call(&base, &m, &p, b))
                    .await
                    .unwrap()
            }};
        }

        let (status, _, vendor) = go!(
            "POST",
            "/vendor",
            Some(json!({"name": "Prusament", "empty_spool_weight": 193.0}))
        );
        assert_eq!(status, 200);
        let (_, _, filament) = go!(
            "POST",
            "/filament",
            Some(
                json!({"name": "PETG Galaxy Black", "vendor_id": vendor["id"], "material": "PETG",
                "density": 1.27, "diameter": 1.75, "weight": 1000.0, "color_hex": "#2B2B33"})
            )
        );
        assert_eq!(filament["vendor"]["name"], "Prusament");
        assert_eq!(filament["color_hex"], "2b2b33");
        let (status, _, spool) = go!(
            "POST",
            "/spool",
            Some(json!({"filament_id": filament["id"], "location": "Shelf B2", "lot_nr": "L123"}))
        );
        assert_eq!(status, 200);
        assert_eq!(spool["initial_weight"], 1000.0);
        assert_eq!(spool["remaining_weight"], 1000.0);
        assert_eq!(spool["location"], "Shelf B2");
        let id = spool["id"].as_i64().unwrap();

        // Its WebSocket, as Moonraker listens to it.
        let (mut ws, _) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/spool/{id}"))
                .await
                .unwrap();

        // 1 m of 1.75 mm PETG is 3.05 g.
        let (_, _, used) = go!(
            "PUT",
            format!("/spool/{id}/use"),
            Some(json!({"use_length": 1000.0}))
        );
        let grams = used["used_weight"].as_f64().unwrap();
        assert!((grams - 3.05).abs() < 0.01, "{grams}");
        assert!(used["first_used"].is_string());
        let event = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let event: Value = serde_json::from_str(event.to_text().unwrap()).unwrap();
        assert_eq!(
            (event["type"].as_str(), event["resource"].as_str()),
            (Some("updated"), Some("spool"))
        );
        assert_eq!(event["payload"]["id"], id);

        // Weighed at 693 g with its 193 g spool: 500 g left.
        let (_, _, weighed) = go!(
            "PUT",
            format!("/spool/{id}/measure"),
            Some(json!({"weight": 693.0}))
        );
        assert_eq!(weighed["remaining_weight"], 500.0);

        // A second spool, used up: lists leave it out unless asked.
        let (_, _, other) = go!(
            "POST",
            "/spool",
            Some(json!({"filament_id": filament["id"], "remaining_weight": 120.0}))
        );
        assert_eq!(other["used_weight"], 880.0);
        go!(
            "PATCH",
            format!("/spool/{}", other["id"]),
            Some(json!({"archived": true}))
        );
        let (_, total, list) = go!(
            "GET",
            "/spool?filament.vendor.name=prusa&sort=id:desc",
            None
        );
        assert_eq!(
            (total.as_deref(), list.as_array().unwrap().len()),
            (Some("1"), 1)
        );
        let (_, total, _) = go!("GET", "/spool?allow_archived=true&limit=1", None);
        assert_eq!(total.as_deref(), Some("2"));
        let (_, _, places) = go!("GET", "/location", None);
        assert_eq!(places, json!(["Shelf B2"]));

        // A filament with spools is not removed; a spool is.
        let (status, _, _) = go!("DELETE", format!("/filament/{}", filament["id"]), None);
        assert_eq!(status, 403);
        let (status, _, _) = go!("DELETE", format!("/spool/{}", other["id"]), None);
        assert_eq!(status, 200);
        let (status, _, _) = go!("GET", format!("/spool/{}", other["id"]), None);
        assert_eq!(status, 404);

        // Tags, as a scale with an NFC reader uses them: linked in any spelling, one holder
        // each, found by a scan and by the list.
        let (status, _, tag) = go!(
            "POST",
            format!("/spool/{id}/tag"),
            Some(json!({"uid": "04:a2:b3:c4", "format": "ntag"}))
        );
        assert_eq!((status, tag["uid"].as_str()), (201, Some("04A2B3C4")));
        let (status, _, again) = go!(
            "POST",
            format!("/spool/{id}/tag"),
            Some(json!({"uid": "04a2b3c4"}))
        );
        assert_eq!((status, again["format"].as_str()), (201, Some("ntag")));
        let (status, _, conflict) = go!(
            "POST",
            format!("/filament/{}/tag", filament["id"]),
            Some(json!({"uid": "04-A2-B3-C4"}))
        );
        assert_eq!((status, conflict["spool_id"].as_i64()), (409, Some(id)));
        let (_, _, scanned) = go!(
            "POST",
            "/tag/scan",
            Some(json!({"uid": "04 a2 b3 c4", "reader_id": "scale"}))
        );
        assert_eq!(scanned["matched_spool_id"], id);
        assert_eq!(scanned["spool"]["filament"]["name"], "PETG Galaxy Black");
        let (_, total, _) = go!("GET", "/spool?tag=04a2b3c4", None);
        assert_eq!(total.as_deref(), Some("1"));
        let (_, _, readers) = go!("GET", "/tag/reader", None);
        assert_eq!(readers[0]["reader_id"], "scale");
        let (status, _, _) = go!(
            "POST",
            format!("/spool/{id}/tag"),
            Some(json!({"uid": "not hex"}))
        );
        assert_eq!(status, 400);

        // An extra field, defined and set: when a spool was last dried.
        let (_, _, defined) = go!(
            "POST",
            "/field/spool/last_dried",
            Some(json!({"name": "Last dried", "field_type": "datetime"}))
        );
        assert_eq!(defined[0]["key"], "last_dried");
        let (status, _, _) = go!(
            "POST",
            "/field/spool/last_dried",
            Some(json!({"name": "Dried", "field_type": "text"}))
        );
        assert_eq!(status, 400, "a field's type stays");
        let (_, _, dried) = go!(
            "PATCH",
            format!("/spool/{id}"),
            Some(json!({"extra": {"last_dried": "\"2026-11-01T10:00:00Z\""}}))
        );
        assert_eq!(dried["extra"]["last_dried"], "\"2026-11-01T10:00:00Z\"");

        // The rest of the hub sees the same spool: S-0001 on the Materials page.
        let s = store.lock().unwrap();
        let space = s.default_space().unwrap().0;
        let item = s.items(space).unwrap().into_iter().next().unwrap();
        assert_eq!((item.id.as_str(), item.amount), ("S-0001", 500.0));
    }

    /// One request to the API: its status, its `x-total-count`, its JSON.
    fn call(
        base: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Option<String>, Value) {
        let request = ureq::request(method, &format!("{base}{path}"));
        let response = match body {
            Some(b) => request.send_json(b),
            None => request.call(),
        };
        match response {
            Ok(r) => {
                let total = r.header("x-total-count").map(str::to_owned);
                (
                    r.status(),
                    total,
                    r.into_json::<Value>().unwrap_or(Value::Null),
                )
            }
            Err(ureq::Error::Status(code, r)) => (code, None, r.into_json().unwrap_or(Value::Null)),
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn dates_go_both_ways() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_793_818_835), "2026-11-04T19:00:35Z");
        assert_eq!(parse_iso("2026-11-04T19:00:35Z"), Some(1_793_818_835));
        assert_eq!(parse_iso("2026-11-04T19:00:35.123"), Some(1_793_818_835));
        assert_eq!(parse_iso("2024-02-29"), Some(1_709_164_800));
    }
}
