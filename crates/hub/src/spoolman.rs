//! Spoolman's API (`/spoolman/api/v1`), on Maker Connect's Materials: so what speaks to a Spoolman
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
//! - WebSockets (`/spoolman/api/v1/spool`, `/spoolman/api/v1/spool/{id}`, the same for `filament`, `vendor`)
//!   send `{"type": "added" | "updated" | "deleted", "resource", "date", "payload"}`.
//!
//! What Spoolman does not have stays Maker Connect's and goes out as `extra` fields: a
//! material's kind and unit (a label tape counts metres), its colour's name, a tape's ink and
//! width, and the machine a spool is loaded in. A spool's number is its code: spool 7 is the
//! label `S-0007`. Locations are places here (with a code and a label); Spoolman's are names,
//! so they go out as their names, and a new name makes a place.

use crate::Store;
use crate::store::{SpoolmanEvent, Work, now};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/spoolman/api/v1/info", get(info))
        .route(
            "/spoolman/api/v1/health",
            get(|| async { Json(json!({"status": "healthy"})) }),
        )
        .route(
            "/spoolman/api/v1/vendor",
            get(list_vendors).post(add_vendor),
        )
        .route(
            "/spoolman/api/v1/vendor/{id}",
            get(one_vendor).patch(change_vendor).delete(remove_vendor),
        )
        .route(
            "/spoolman/api/v1/filament",
            get(list_filaments).post(add_filament),
        )
        .route(
            "/spoolman/api/v1/filament/{id}",
            get(one_filament)
                .patch(change_filament)
                .delete(remove_filament),
        )
        .route("/spoolman/api/v1/spool", get(list_spools).post(add_spool))
        .route(
            "/spoolman/api/v1/spool/{id}",
            get(one_spool).patch(change_spool).delete(remove_spool),
        )
        .route(
            "/spoolman/api/v1/spool/field/{field}",
            axum::routing::patch(rename_field_value),
        )
        .route("/spoolman/api/v1/spool/{id}/use", put(use_spool))
        .route("/spoolman/api/v1/spool/{id}/measure", put(measure_spool))
        .route("/spoolman/api/v1/material", get(materials))
        .route("/spoolman/api/v1/location", get(locations))
        .route(
            "/spoolman/api/v1/location/{*name}",
            axum::routing::patch(rename_location),
        )
        .route("/spoolman/api/v1/lot-number", get(lot_numbers))
        .route("/spoolman/api/v1/article-number", get(article_numbers))
        .route("/spoolman/api/v1/setting/", get(settings))
        .route(
            "/spoolman/api/v1/setting/{key}",
            get(setting).post(set_setting),
        )
        .route("/spoolman/api/v1/field/{entity}", get(fields))
        .route(
            "/spoolman/api/v1/field/{entity}/{key}",
            post(set_field).delete(remove_field),
        )
        .route(
            "/spoolman/api/v1/field/{entity}/{key}/values",
            get(field_values),
        )
        .route("/spoolman/api/v1/spool/{id}/tag", post(tag_spool))
        .route(
            "/spoolman/api/v1/spool/{id}/tag/{uid}",
            axum::routing::delete(untag_spool),
        )
        .route("/spoolman/api/v1/filament/{id}/tag", post(tag_filament))
        .route(
            "/spoolman/api/v1/filament/{id}/tag/{uid}",
            axum::routing::delete(untag_filament),
        )
        .route("/spoolman/api/v1/tag/scan", post(scan).get(scan_live))
        .route("/spoolman/api/v1/tag/scan/{reader}", get(scan_live_reader))
        .route("/spoolman/api/v1/tag/reader", get(readers))
        .route("/spoolman/api/v1/export/{what}", get(export))
        .route(
            "/spoolman/api/v1/backup",
            post(|| async { StatusCode::NOT_IMPLEMENTED }),
        )
        // FastAPI's handling, inside the hub's sign-in and permission checks.
        .route_layer(axum::middleware::from_fn(routing::fastapi))
}

// ---- gzip ----------------------------------------------------------------------------

/// Starlette 1.10.0's `DEFAULT_EXCLUDED_CONTENT_TYPES` (`starlette/middleware/gzip.py`).
const GZIP_EXCLUDED_CONTENT_TYPES: [&str; 14] = [
    "application/grpc",
    "application/gzip",
    "application/x-gzip",
    "application/zip",
    "audio/*",
    "font/woff",
    "font/woff2",
    "image/avif",
    "image/gif",
    "image/jpeg",
    "image/png",
    "image/webp",
    "text/event-stream",
    "video/*",
];

/// Starlette's `GZipMiddleware` with its defaults (`minimum_size=500`, `compresslevel=9`),
/// as Spoolman's `main.py` adds it (`app.add_middleware(GZipMiddleware)`), ported from
/// `starlette/middleware/gzip.py` 1.10.0 (BSD-3-Clause), on what Spoolman's server answers:
/// everything under its base path `/spoolman`, the routing's 404 and 405 too. Every answer of
/// the hub is one body, Starlette's "Standard response". WebSockets pass by, as Starlette's
/// `scope["type"] != "http"` does.
pub async fn gzip(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if !req.uri().path().starts_with("/spoolman/") {
        return next.run(req).await;
    }
    let headers = req.headers();
    let websocket = headers
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if websocket {
        return next.run(req).await;
    }
    let gzip = headers
        .get("accept-encoding")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("gzip"));
    let (mut parts, body) = next.run(req).await.into_parts();
    let content_encoding_set = parts.headers.contains_key("content-encoding");
    let partial_response = parts.status == StatusCode::PARTIAL_CONTENT;
    let media_type = parts
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let mut media_types = vec![
        media_type.clone(),
        format!("{}/*", media_type.split('/').next().unwrap_or("")),
    ];
    if media_type.starts_with("application/grpc+") {
        media_types.push("application/grpc".into());
    }
    let content_type_is_excluded = media_types
        .iter()
        .any(|t| GZIP_EXCLUDED_CONTENT_TYPES.contains(&t.as_str()));
    if content_encoding_set || partial_response || content_type_is_excluded {
        return Response::from_parts(parts, body);
    }
    let body = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if body.len() < 500 {
        // "Don't apply compression to small outgoing responses."
        return Response::from_parts(parts, axum::body::Body::from(body));
    }
    // `MutableHeaders.add_vary_header`.
    let vary = match parts.headers.get("vary").and_then(|v| v.to_str().ok()) {
        Some(existing) => format!("{existing}, Accept-Encoding"),
        None => "Accept-Encoding".to_owned(),
    };
    if let Ok(v) = vary.parse() {
        parts.headers.insert("vary", v);
    }
    if !gzip {
        return Response::from_parts(parts, axum::body::Body::from(body));
    }
    // `zlib.compressobj(9, zlib.DEFLATED, 16 + zlib.MAX_WBITS)`: a gzip stream, level 9.
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(9));
    let compressed = match encoder.write_all(&body).and_then(|()| encoder.finish()) {
        Ok(c) => c,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    parts.headers.insert(
        "content-encoding",
        axum::http::HeaderValue::from_static("gzip"),
    );
    parts.headers.insert(
        "content-length",
        axum::http::HeaderValue::from(compressed.len()),
    );
    Response::from_parts(parts, axum::body::Body::from(compressed))
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

/// What a spool's `loaded_in` needs ([`Store::spool_machine_names`]), read before the lock; nothing
/// for a vendor or filament.
async fn names_for(
    store: &Store,
    space: i64,
    resource: &str,
) -> Result<MachineNames, crate::store::BoxError> {
    if resource == "spool" {
        store.spool_machine_names(space).await
    } else {
        Ok(MachineNames::new())
    }
}

fn not_found(what: &str, id: i64) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"message": format!("No {what} with ID {id} found.")})),
    )
        .into_response()
}

/// A 400 `{"message": ...}`, the `JSONResponse(status_code=400, ...)` Spoolman's routes return.
fn refused(why: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"message": why.into()})),
    )
        .into_response()
}

/// A 404 `{"message": ...}`: Spoolman's `ItemNotFoundError` handler (`api/v1/router.py`).
fn missing(why: impl Into<String>) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({"message": why.into()}))).into_response()
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

/// A vendor's row: id, registered, name, comment, empty spool, external id, extra.
type VendorRow = (
    i64,
    i64,
    String,
    Option<String>,
    Option<f64>,
    Option<String>,
    Option<String>,
);

impl Work {
    pub async fn sm_vendor(&mut self, id: i64) -> sqlx::Result<Option<Value>> {
        let row: Option<VendorRow> =
            sqlx::query_as(
                "SELECT id, unix_seconds(registered), name, comment, empty_spool_weight, external_id, extra
                 FROM vendor WHERE id = $1 AND space_id = $2",
            )
            .bind(id)
            .bind(self.space)
            .fetch_optional(self.c())
            .await?;
        Ok(row.map(
            |(id, registered, name, comment, empty_spool_weight, external_id, extra)| {
                json!({
                    "id": id,
                    "registered": iso(registered),
                    "name": name,
                    "comment": comment,
                    "empty_spool_weight": empty_spool_weight,
                    "external_id": external_id,
                    "extra": extra_map(extra),
                })
            },
        ))
    }

    pub async fn sm_filament(&mut self, id: i64) -> sqlx::Result<Option<Value>> {
        use sqlx::Row;
        let row = sqlx::query(
            "SELECT id, unix_seconds(registered), name, vendor_id, material, price, density, diameter, weight,
                    spool_weight, article_number, comment, settings_extruder_temp,
                    settings_bed_temp, color_hex, multi_color_hexes, multi_color_direction,
                    external_id, extra, kind, unit, color_name, ink, media, width_mm, minimum
             FROM filament WHERE id = $1 AND space_id = $2",
        )
        .bind(id)
        .bind(self.space)
        .fetch_optional(self.c())
        .await?;
        let Some(r) = row else {
            return Ok(None);
        };
        let mut extra = extra_map(r.try_get(18)?);
        // Maker Connect's own fields, beside the user's.
        let kind: String = r.try_get(19)?;
        let unit: String = r.try_get(20)?;
        if kind != "filament" {
            extra.insert("kind".into(), extra_value(json!(kind)));
        }
        if unit != "g" {
            extra.insert("unit".into(), extra_value(json!(unit)));
        }
        for (key, at) in [("color_name", 21), ("ink", 22), ("media", 23)] {
            if let Some(v) = r.try_get::<Option<String>, _>(at)? {
                extra.insert(key.into(), extra_value(json!(v)));
            }
        }
        for (key, at) in [("width_mm", 24), ("minimum", 25)] {
            if let Some(v) = r.try_get::<Option<i64>, _>(at)? {
                extra.insert(key.into(), extra_value(json!(v)));
            }
        }
        let vendor = match r.try_get::<Option<i64>, _>(3)? {
            Some(v) => self.sm_vendor(v).await?,
            None => None,
        };
        // In the order of Spoolman's `Filament` model.
        let mut filament = json!({
            "id": r.try_get::<i64, _>(0)?,
            "registered": iso(r.try_get(1)?),
            "name": r.try_get::<Option<String>, _>(2)?,
            "vendor": vendor,
            "material": r.try_get::<Option<String>, _>(4)?,
            "price": r.try_get::<Option<f64>, _>(5)?,
            "density": r.try_get::<f64, _>(6)?,
            "diameter": r.try_get::<f64, _>(7)?,
            "weight": r.try_get::<Option<f64>, _>(8)?,
            "spool_weight": r.try_get::<Option<f64>, _>(9)?,
            "article_number": r.try_get::<Option<String>, _>(10)?,
            "comment": r.try_get::<Option<String>, _>(11)?,
            "settings_extruder_temp": r.try_get::<Option<i64>, _>(12)?,
            "settings_bed_temp": r.try_get::<Option<i64>, _>(13)?,
            "color_hex": sanitize_color_hex(r.try_get::<Option<String>, _>(14)?.as_deref()),
            "multi_color_hexes": sanitize_multi_color_hexes(
                r.try_get::<Option<String>, _>(15)?.as_deref()
            ),
            "multi_color_direction": r.try_get::<Option<String>, _>(16)?,
            "external_id": r.try_get::<Option<String>, _>(17)?,
            "extra": extra,
            "tags": [],
        });
        filament["tags"] = self.tags_of("filament", id).await?;
        // No vendor: no `vendor` key, as Spoolman's `Filament` leaves out its `None`.
        if filament["vendor"].is_null()
            && let Some(m) = filament.as_object_mut()
        {
            m.shift_remove("vendor");
        }
        Ok(Some(filament))
    }

    /// A spool as Spoolman gives it; `names`: the machines' names ([`Store::spool_machine_names`]).
    pub async fn sm_spool(&mut self, id: i64, names: &MachineNames) -> sqlx::Result<Option<Value>> {
        use sqlx::Row;
        let row = sqlx::query(
            "SELECT s.id, unix_seconds(s.registered), unix_seconds(s.first_used), unix_seconds(s.last_used),
                    s.price, s.filament_id, s.initial_weight, s.spool_weight, s.used_weight, l.name, s.lot_nr,
                    s.comment, s.archived, s.extra, s.machine, s.slot
             FROM spool s
             LEFT JOIN locations l ON l.id = s.location_id AND l.space_id = s.space_id
             WHERE s.id = $1 AND s.space_id = $2",
        )
        .bind(id)
        .bind(self.space)
        .fetch_optional(self.c())
        .await?;
        let Some(r) = row else {
            return Ok(None);
        };
        let mut extra = extra_map(r.try_get(13)?);
        if let Some(machine) = r
            .try_get::<Option<String>, _>(14)?
            .and_then(|m| names.get(&m))
        {
            extra.insert("loaded_in".into(), extra_value(json!(machine)));
        }
        // Its slot in a dry box.
        if let Some(slot) = r.try_get::<Option<i64>, _>(15)? {
            extra.insert("slot".into(), extra_value(json!(slot)));
        }
        let filament_id: i64 = r.try_get(5)?;
        let initial_weight: Option<f64> = r.try_get(6)?;
        let used: f64 = r.try_get(8)?;
        let filament = self.sm_filament(filament_id).await?.unwrap_or(Value::Null);
        // What is left, and both as lengths: weight over density and the cross-section.
        let initial = initial_weight.or_else(|| filament["weight"].as_f64());
        let remaining = initial.map(|i| (i - used).max(0.0));
        let density = filament["density"].as_f64().unwrap_or(1.24);
        let diameter = filament["diameter"].as_f64().unwrap_or(1.75);
        let length = |weight: f64| length_from_weight(weight, diameter, density);
        // In the order of Spoolman's `Spool` model.
        let spool = json!({
            "id": r.try_get::<i64, _>(0)?,
            "registered": iso(r.try_get(1)?),
            "first_used": r.try_get::<Option<i64>, _>(2)?.map(iso),
            "last_used": r.try_get::<Option<i64>, _>(3)?.map(iso),
            "filament": filament,
            "price": r.try_get::<Option<f64>, _>(4)?,
            "remaining_weight": remaining,
            "initial_weight": initial_weight,
            "spool_weight": r.try_get::<Option<f64>, _>(7)?,
            "used_weight": used,
            "remaining_length": remaining.map(length),
            "used_length": length(used),
            "location": r.try_get::<Option<String>, _>(9)?,
            "lot_nr": r.try_get::<Option<String>, _>(10)?,
            "comment": r.try_get::<Option<String>, _>(11)?,
            "archived": r.try_get::<bool, _>(12)?,
            "extra": extra,
            "tags": self.tags_of("spool", id).await?,
        });
        Ok(Some(spool))
    }

    /// The tags on a spool or a filament: `[{uid, format, added}]`.
    async fn tags_of(&mut self, target: &str, id: i64) -> sqlx::Result<Value> {
        let column = if target == "spool" {
            "spool_id"
        } else {
            "filament_id"
        };
        let rows: Vec<(String, Option<String>, i64)> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT uid, format, unix_seconds(added) FROM tag
             WHERE space_id = $1 AND target_type = $2 AND {column} = $3 ORDER BY added, id"
            )))
            .bind(self.space)
            .bind(target)
            .bind(id)
            .fetch_all(self.c())
            .await?;
        Ok(Value::Array(
            rows.into_iter()
                .map(|(uid, format, added)| {
                    // In the order of Spoolman's `Tag` model.
                    let mut tag = json!({"uid": uid, "format": format, "added": iso(added)});
                    if format.is_none()
                        && let Some(m) = tag.as_object_mut()
                    {
                        m.shift_remove("format");
                    }
                    tag
                })
                .collect(),
        ))
    }

    pub async fn ids(&mut self, table: &str) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT id FROM {table} WHERE space_id = $1 ORDER BY id"
        )))
        .bind(self.space)
        .fetch_all(self.c())
        .await
    }

    pub async fn all(&mut self, resource: &str, names: &MachineNames) -> sqlx::Result<Vec<Value>> {
        let mut out = Vec::new();
        for id in self.ids(resource).await? {
            if let Some(v) = self.one(resource, id, names).await? {
                out.push(v);
            }
        }
        Ok(out)
    }

    pub async fn one(
        &mut self,
        resource: &str,
        id: i64,
        names: &MachineNames,
    ) -> sqlx::Result<Option<Value>> {
        match resource {
            "vendor" => self.sm_vendor(id).await,
            "filament" => self.sm_filament(id).await,
            "spool" => self.sm_spool(id, names).await,
            _ => Ok(None),
        }
    }

    /// The machines the space's spools are loaded in.
    async fn spool_machines(&mut self) -> sqlx::Result<Vec<String>> {
        sqlx::query_scalar(
            "SELECT DISTINCT machine FROM spool WHERE space_id = $1 AND machine IS NOT NULL",
        )
        .bind(self.space)
        .fetch_all(self.c())
        .await
    }
}

/// Machines' names by id: what a spool's `loaded_in` says.
pub type MachineNames = HashMap<String, String>;

impl Store {
    /// The names of the machines the space's spools are loaded in (the space's own: a machine moved
    /// away leaves its spools unloaded, Store::move_machine), for [`Work::one`], [`Work::all`],
    /// [`Work::sm_spool`].
    pub async fn spool_machine_names(
        &self,
        space: i64,
    ) -> Result<MachineNames, crate::store::BoxError> {
        let ids = crate::stock::in_space!(self, space, |w| w.spool_machines()).await?;
        if ids.is_empty() {
            return Ok(MachineNames::new());
        }
        let mut tx = self.pg().space(space).await?;
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, name FROM machines WHERE id = ANY($1)")
                .bind(&ids)
                .fetch_all(&mut *tx)
                .await?;
        tx.commit().await?;
        Ok(rows.into_iter().collect())
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
    query: &Map<String, Value>,
    filters: &[(&str, &str)],
) -> Response {
    for (param, path) in filters {
        if let Some(wanted) = query.get(*param).and_then(Value::as_str) {
            items.retain(|v| matches(at(v, path), wanted));
        }
    }
    if let Some(sort) = query.get("sort").and_then(Value::as_str) {
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
        .and_then(Value::as_u64)
        .map_or(0, |o| o as usize);
    let limit = query
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(usize::MAX, |l| l as usize);
    let page: Vec<Value> = items.into_iter().skip(offset).take(limit).collect();
    (
        [
            ("x-total-count", total.to_string()),
            ("access-control-expose-headers", "x-total-count".to_owned()),
        ],
        model(page),
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
    State(store): State<Store>,
    pydantic::Params(q): pydantic::Params,
) -> Response {
    // The same path is its WebSocket: what is added, changed, removed.
    if let Some(ws) = ws {
        return upgrade(ws, store, access.space, "vendor".to_owned(), None);
    }
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    match w.all("vendor", &MachineNames::new()).await {
        Ok(all) => listing(all, &q, &[("name", "name"), ("external_id", "external_id")]),
        Err(e) => crate::failed(e),
    }
}

async fn list_filaments(
    access: crate::access::Access,
    MaybeWs(ws): MaybeWs,
    State(store): State<Store>,
    pydantic::Params(q): pydantic::Params,
) -> Response {
    // The same path is its WebSocket: what is added, changed, removed.
    if let Some(ws) = ws {
        return upgrade(ws, store, access.space, "filament".to_owned(), None);
    }
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    match w.all("filament", &MachineNames::new()).await {
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
    State(store): State<Store>,
    pydantic::Params(q): pydantic::Params,
) -> Response {
    // The same path is its WebSocket: what is added, changed, removed.
    if let Some(ws) = ws {
        return upgrade(ws, store, access.space, "spool".to_owned(), None);
    }
    let names = match names_for(&store, access.space, "spool").await {
        Ok(names) => names,
        Err(e) => return crate::failed(e),
    };
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    let mut all = match w.all("spool", &names).await {
        Ok(all) => all,
        Err(e) => return crate::failed(e),
    };
    // Used-up spools only when asked for.
    if q.get("allow_archived") != Some(&Value::Bool(true)) {
        all.retain(|s| s["archived"] != true);
    }
    // By a tag's UID, in any spelling.
    if let Some(uid) = q
        .get("tag")
        .and_then(Value::as_str)
        .and_then(|t| normalize_uid(t).ok())
    {
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

async fn one_of(
    access: &crate::access::Access,
    store: &Store,
    resource: &str,
    id: i64,
) -> Response {
    let names = match names_for(store, access.space, resource).await {
        Ok(names) => names,
        Err(e) => return crate::failed(e),
    };
    let space = match space(access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    match w.one(resource, id, &names).await {
        Ok(Some(v)) => model(v).into_response(),
        Ok(None) => not_found(resource, id),
        Err(e) => crate::failed(e),
    }
}

async fn one_vendor(
    access: crate::access::Access,
    MaybeWs(ws): MaybeWs,
    State(store): State<Store>,
    Path(id): Path<i64>,
) -> Response {
    if let Some(ws) = ws {
        return upgrade(ws, store, access.space, "vendor".to_owned(), Some(id));
    }
    one_of(&access, &store, "vendor", id).await
}
async fn one_filament(
    access: crate::access::Access,
    MaybeWs(ws): MaybeWs,
    State(store): State<Store>,
    Path(id): Path<i64>,
) -> Response {
    if let Some(ws) = ws {
        return upgrade(ws, store, access.space, "filament".to_owned(), Some(id));
    }
    one_of(&access, &store, "filament", id).await
}
async fn one_spool(
    access: crate::access::Access,
    MaybeWs(ws): MaybeWs,
    State(store): State<Store>,
    Path(id): Path<i64>,
) -> Response {
    if let Some(ws) = ws {
        return upgrade(ws, store, access.space, "spool".to_owned(), Some(id));
    }
    one_of(&access, &store, "spool", id).await
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

/// A column's new value, as its column takes it.
enum Set {
    Real(Option<f64>),
    Text(Option<String>),
    Int(Option<i64>),
    Bool(bool),
    /// Unix seconds, into a timestamptz.
    Time(Option<i64>),
}

/// Sets the columns a body names (absent ones stay; `null` clears).
async fn update(w: &mut Work, table: &str, id: i64, changes: Vec<(&str, Set)>) -> sqlx::Result<()> {
    for (column, mut value) in changes {
        // `extra` merged per key, as Spoolman's `create` and `update` do: the keys in the
        // body replace the stored ones, and a null one is dropped, not stored.
        if let (true, Set::Text(Some(patch))) = (column == "extra", &value) {
            let sql = format!("SELECT extra FROM {table} WHERE id = $1");
            let stored: Option<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
                .bind(id)
                .fetch_optional(w.c())
                .await?
                .flatten();
            let mut merged = extra_map(stored);
            for (k, v) in extra_map(Some(patch.clone())) {
                merged.remove(&k);
                if !v.is_null() {
                    merged.insert(k, v);
                }
            }
            value = txt(Some(Value::Object(merged).to_string()));
        }
        let to = if matches!(value, Set::Time(_)) {
            "at_unix($1)"
        } else {
            "$1"
        };
        let sql = format!("UPDATE {table} SET {column} = {to} WHERE id = $2");
        let q = sqlx::query(sqlx::AssertSqlSafe(sql));
        let q = match value {
            Set::Real(v) => q.bind(v),
            Set::Text(v) => q.bind(v),
            Set::Int(v) | Set::Time(v) => q.bind(v),
            Set::Bool(v) => q.bind(v),
        };
        q.bind(id).execute(w.c()).await?;
    }
    Ok(())
}

fn real(v: Option<f64>) -> Set {
    Set::Real(v)
}
fn txt(v: Option<String>) -> Set {
    Set::Text(v)
}
fn int(v: Option<i64>) -> Set {
    Set::Int(v)
}

fn vendor_changes(body: &Value) -> Vec<(&'static str, Set)> {
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
    State(store): State<Store>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    let made = async {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO vendor (space_id, registered, name) VALUES ($1, at_unix($2), '') RETURNING id",
        )
        .bind(space)
        .bind(now())
        .fetch_one(w.c())
        .await?;
        update(&mut w, "vendor", id, vendor_changes(&body)).await?;
        Ok::<_, sqlx::Error>(id)
    }
    .await;
    respond_made(w, &store, "vendor", made, &MachineNames::new()).await
}

async fn change_vendor(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    if matches!(w.sm_vendor(id).await, Ok(None)) {
        return not_found("vendor", id);
    }
    let done = update(&mut w, "vendor", id, vendor_changes(&body))
        .await
        .map(|()| id);
    respond_changed(w, &store, "vendor", done, &MachineNames::new()).await
}

/// Err: the vendor that does not exist (Spoolman's `vendor.get_by_id` raises
/// `ItemNotFoundError`, a 404).
async fn filament_changes(
    w: &mut Work,
    body: &Value,
) -> sqlx::Result<Result<Vec<(&'static str, Set)>, i64>> {
    let mut c = Vec::new();
    for key in [
        "name",
        "material",
        "article_number",
        "comment",
        "multi_color_direction",
        "external_id",
    ] {
        if let Some(v) = text(body, key) {
            c.push((key, txt(v)));
        }
    }
    // Stored as Spoolman's validators return them: without the leading `#`, the case kept.
    if let Some(v) = text(body, "color_hex") {
        c.push(("color_hex", txt(v.and_then(|v| color_hex_given(&v)))));
    }
    if let Some(v) = text(body, "multi_color_hexes") {
        c.push((
            "multi_color_hexes",
            txt(v.and_then(|v| multi_color_hexes_given(&v))),
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
            && w.sm_vendor(vendor).await?.is_none()
        {
            return Ok(Err(vendor));
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
    Ok(Ok(c))
}

async fn add_filament(
    access: crate::access::Access,
    State(store): State<Store>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    let mut changes = match filament_changes(&mut w, &body).await {
        Ok(Ok(c)) => c,
        Ok(Err(vendor)) => return not_found("vendor", vendor),
        Err(e) => return crate::failed(e),
    };
    // Spoolman's `database.filament.create`: "default spool weight from vendor".
    if body["spool_weight"].is_null()
        && let Some(vendor) = body["vendor_id"].as_i64()
    {
        let empty = match w.sm_vendor(vendor).await {
            Ok(v) => v.and_then(|v| v["empty_spool_weight"].as_f64()),
            Err(e) => return crate::failed(e),
        };
        if empty.is_some() {
            changes.push(("spool_weight", real(empty)));
        }
    }
    let made = async {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO filament (space_id, registered) VALUES ($1, at_unix($2)) RETURNING id",
        )
        .bind(space)
        .bind(now())
        .fetch_one(w.c())
        .await?;
        update(&mut w, "filament", id, changes).await?;
        Ok::<_, sqlx::Error>(id)
    }
    .await;
    respond_made(w, &store, "filament", made, &MachineNames::new()).await
}

async fn change_filament(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    if matches!(w.sm_filament(id).await, Ok(None)) {
        return not_found("filament", id);
    }
    let changes = match filament_changes(&mut w, &body).await {
        Ok(Ok(c)) => c,
        Ok(Err(vendor)) => return not_found("vendor", vendor),
        Err(e) => return crate::failed(e),
    };
    let done = update(&mut w, "filament", id, changes).await.map(|()| id);
    respond_changed(w, &store, "filament", done, &MachineNames::new()).await
}

async fn spool_changes(
    w: &mut Work,
    body: &Value,
    spool: Option<i64>,
) -> Result<Vec<(&'static str, Set)>, Box<Response>> {
    let mut c = Vec::new();
    if let Some(v) = body.get("filament_id") {
        let Some(f) = v.as_i64() else {
            return Err(Box::new(invalid("filament_id must be a number")));
        };
        // Spoolman's `filament.get_by_id`: `ItemNotFoundError`, a 404.
        match w.sm_filament(f).await {
            Ok(Some(_)) => {}
            Ok(None) => return Err(Box::new(not_found("filament", f))),
            Err(e) => return Err(Box::new(crate::failed(e))),
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
            c.push((key, Set::Time(v.as_deref().and_then(parse_iso))));
        }
    }
    if let Some(v) = body.get("archived") {
        let archived = v.as_bool().unwrap_or(false);
        c.push(("archived", Set::Bool(archived)));
        if archived {
            c.push(("machine", txt(None)));
        }
    }
    if let Some(v) = text(body, "location") {
        let place = match v {
            Some(name) => w
                .location_named(&name)
                .await
                .map_err(|e| Box::new(crate::failed(e)))?,
            None => None,
        };
        c.push(("location_id", txt(place.clone())));
        // Another place: out of its slot (unless one comes with it). Into a box (a place
        // with slots) it stays loaded, the box feeding the printer; onto a shelf it is no
        // longer on the printer.
        let boxed = match place.as_deref() {
            Some(p) => sqlx::query_scalar::<_, bool>(
                "SELECT COALESCE(slots, 0) > 0 FROM locations WHERE id = $1",
            )
            .bind(p)
            .fetch_optional(w.c())
            .await
            .map_err(|e| Box::new(crate::failed(e)))?
            .unwrap_or(false),
            None => false,
        };
        c.push(("slot", int(None)));
        if place.is_some() && !boxed {
            c.push(("machine", txt(None)));
        }
        // A dry box holds filament, not a label tape.
        if boxed {
            let kind: Option<String> = match (spool, body["filament_id"].as_i64()) {
                (_, Some(f)) => sqlx::query_scalar("SELECT kind FROM filament WHERE id = $1")
                    .bind(f)
                    .fetch_optional(w.c())
                    .await
                    .map_err(|e| Box::new(crate::failed(e)))?,
                (Some(sp), None) => sqlx::query_scalar(
                    "SELECT f.kind FROM spool s JOIN filament f ON f.id = s.filament_id WHERE s.id = $1",
                )
                .bind(sp)
                .fetch_optional(w.c())
                .await
                .map_err(|e| Box::new(crate::failed(e)))?,
                _ => None,
            };
            if kind.as_deref() == Some("tape") {
                return Err(Box::new(invalid(
                    "a dry box holds filament, not a label tape",
                )));
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
    State(store): State<Store>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    // Spoolman's route, before anything else.
    if !body["remaining_weight"].is_null() && !body["used_weight"].is_null() {
        return refused("Only specify either remaining_weight or used_weight.");
    }
    let names = match names_for(&store, access.space, "spool").await {
        Ok(names) => names,
        Err(e) => return crate::failed(e),
    };
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    let mut changes = match spool_changes(&mut w, &body, None).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    // remaining_weight instead of used_weight: used is what the spool came with less it.
    let filament = w
        .sm_filament(body["filament_id"].as_i64().unwrap_or_default())
        .await
        .ok()
        .flatten()
        .unwrap_or(Value::Null);
    let initial = body["initial_weight"]
        .as_f64()
        .or(filament["weight"].as_f64());
    // Spoolman's `database.spool.create`: a weight not given (or null) is the filament's.
    if body["initial_weight"].is_null() {
        changes.push(("initial_weight", real(initial)));
    }
    if body["spool_weight"].is_null() {
        changes.push(("spool_weight", real(filament["spool_weight"].as_f64())));
    }
    // `database.spool.create`: remaining_weight needs an initial weight that is not 0, else
    // `ItemCreateError`, which the route turns into its own 400.
    if body["used_weight"].is_null()
        && let Some(remaining) = body["remaining_weight"].as_f64()
    {
        let Some(initial) = initial.filter(|i| *i != 0.0) else {
            return refused("Failed to create spool, see server logs for more information.");
        };
        changes.push(("used_weight", real(Some((initial - remaining).max(0.0)))));
    }
    let made = async {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO spool (space_id, registered, filament_id) VALUES ($1, at_unix($2), $3) RETURNING id",
        )
        .bind(space)
        .bind(now())
        .bind(body["filament_id"].as_i64())
        .fetch_one(w.c())
        .await?;
        update(&mut w, "spool", id, changes).await?;
        w.movement(&crate::stock::spool_code(id), "received", initial, None, None)
            .await?;
        Ok::<_, sqlx::Error>(id)
    }
    .await;
    respond_made(w, &store, "spool", made, &names).await
}

async fn change_spool(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    // Spoolman's route, before anything else.
    if !body["remaining_weight"].is_null() && !body["used_weight"].is_null() {
        return refused("Only specify either remaining_weight or used_weight.");
    }
    let names = match names_for(&store, access.space, "spool").await {
        Ok(names) => names,
        Err(e) => return crate::failed(e),
    };
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    let Ok(Some(before)) = w.sm_spool(id, &names).await else {
        return not_found("spool", id);
    };
    let mut changes = match spool_changes(&mut w, &body, Some(id)).await {
        Ok(c) => c,
        Err(r) => return *r,
    };
    // `database.spool.update` walks the fields in `SpoolUpdateParameters` order (filament_id,
    // initial_weight, remaining_weight): the spool's initial_weight at that point is the one
    // sent, else the stored one, else (when the filament changes and none is stored) the new
    // filament's weight. None: `ItemCreateError`, the route's own 400.
    if let Some(remaining) = body["remaining_weight"].as_f64() {
        let initial = if body.get("initial_weight").is_some() {
            body["initial_weight"].as_f64()
        } else {
            match (
                before["initial_weight"].as_f64(),
                body["filament_id"].as_i64(),
            ) {
                (Some(i), _) => Some(i),
                (None, Some(f)) => match w.sm_filament(f).await {
                    Ok(f) => f.and_then(|f| f["weight"].as_f64()),
                    Err(e) => return crate::failed(e),
                },
                (None, None) => None,
            }
        };
        let Some(initial) = initial else {
            return refused("Failed to update spool, see server logs for more information.");
        };
        changes.push(("used_weight", real(Some((initial - remaining).max(0.0)))));
    }
    let done = update(&mut w, "spool", id, changes).await.map(|()| id);
    w.changed();
    respond_changed(w, &store, "spool", done, &names).await
}

async fn use_spool(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    // Spoolman's route checks the body before it looks the spool up.
    let (use_weight, use_length) = (body["use_weight"].as_f64(), body["use_length"].as_f64());
    match (use_weight, use_length) {
        (Some(_), Some(_)) => return refused("Only specify either use_weight or use_length."),
        (None, None) => return refused("Either use_weight or use_length must be specified."),
        _ => {}
    }
    let names = match names_for(&store, access.space, "spool").await {
        Ok(names) => names,
        Err(e) => return crate::failed(e),
    };
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    let spool = match w.sm_spool(id, &names).await {
        Ok(Some(spool)) => spool,
        Ok(None) if use_length.is_some() => {
            // `database.spool.use_length` looks the filament up by the spool first.
            return missing("Filament not found for spool.");
        }
        Ok(None) => return not_found("spool", id),
        Err(e) => return crate::failed(e),
    };
    let grams = match (use_weight, use_length) {
        (Some(g), _) => g,
        (None, Some(mm)) => {
            let density = spool["filament"]["density"].as_f64().unwrap_or(1.24);
            let diameter = spool["filament"]["diameter"].as_f64().unwrap_or(1.75);
            weight_from_length(mm, diameter, density)
        }
        (None, None) => unreachable!("refused above"),
    };
    let done = async {
        let t = now();
        sqlx::query(
            "UPDATE spool SET used_weight = GREATEST(used_weight + $1, 0),
                 first_used = COALESCE(first_used, at_unix($2)), last_used = at_unix($2)
             WHERE id = $3",
        )
        .bind(grams)
        .bind(t)
        .bind(id)
        .execute(w.c())
        .await?;
        w.movement(
            &crate::stock::spool_code(id),
            "used",
            Some(-grams),
            None,
            None,
        )
        .await?;
        Ok::<_, sqlx::Error>(id)
    }
    .await;
    w.changed();
    respond_changed(w, &store, "spool", done, &names).await
}

async fn measure_spool(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    // `SpoolMeasureParameters.weight`, a `float` FastAPI checked.
    let gross = body["weight"].as_f64().unwrap_or_default();
    let names = match names_for(&store, access.space, "spool").await {
        Ok(names) => names,
        Err(e) => return crate::failed(e),
    };
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    // `database.spool.measure`: `SpoolMeasureError`, which the route turns into a 400.
    let spool = match w.sm_spool(id, &names).await {
        Ok(Some(spool)) => spool,
        Ok(None) => return refused("Spool not found."),
        Err(e) => return crate::failed(e),
    };
    // The empty spool: the spool's own, else its filament's, else its vendor's.
    let empty = spool["spool_weight"]
        .as_f64()
        .or(spool["filament"]["spool_weight"].as_f64())
        .or(spool["filament"]["vendor"]["empty_spool_weight"].as_f64())
        .unwrap_or(0.0);
    // An initial weight of None or 0 is not set: then the filament's, else 0.
    let initial = spool["initial_weight"]
        .as_f64()
        .filter(|i| *i != 0.0)
        .or(spool["filament"]["weight"].as_f64())
        .unwrap_or(0.0);
    if initial == 0.0 {
        return refused("Initial weight is not set.");
    }
    let net = (gross - empty).max(0.0);
    let done = async {
        let t = now();
        sqlx::query(
            "UPDATE spool SET used_weight = GREATEST($1 - $2, 0), last_used = at_unix($3) WHERE id = $4",
        )
        .bind(initial)
        .bind(net)
        .bind(t)
        .bind(id)
        .execute(w.c())
        .await?;
        let was = spool["remaining_weight"].as_f64().unwrap_or(net);
        w.movement(&crate::stock::spool_code(id), "weighed", Some(net - was), None, None)
            .await?;
        Ok::<_, sqlx::Error>(id)
    }
    .await;
    w.changed();
    respond_changed(w, &store, "spool", done, &names).await
}

async fn respond_made(
    w: Work,
    store: &Store,
    resource: &'static str,
    made: sqlx::Result<i64>,
    names: &MachineNames,
) -> Response {
    respond(w, store, resource, "added", made, names).await
}

async fn respond_changed(
    w: Work,
    store: &Store,
    resource: &'static str,
    done: sqlx::Result<i64>,
    names: &MachineNames,
) -> Response {
    respond(w, store, resource, "updated", done, names).await
}

/// What was made or changed, as it is now; committed, then told.
async fn respond(
    mut w: Work,
    store: &Store,
    resource: &'static str,
    kind: &'static str,
    done: sqlx::Result<i64>,
    names: &MachineNames,
) -> Response {
    let found = match done {
        Ok(id) => w.one(resource, id, names).await.map(|v| (id, v)),
        Err(e) => Err(e),
    };
    match found {
        Ok((id, Some(v))) => {
            w.changed();
            w.spoolman_event(resource, kind, id);
            match w.commit(store).await {
                Ok(()) => model(v).into_response(),
                Err(e) => crate::failed(e),
            }
        }
        Ok((id, None)) => not_found(resource, id),
        Err(e) => crate::failed(e),
    }
}

async fn remove(
    access: &crate::access::Access,
    store: &Store,
    resource: &'static str,
    id: i64,
) -> Response {
    let names = match names_for(store, access.space, resource).await {
        Ok(names) => names,
        Err(e) => return crate::failed(e),
    };

    let space = match space(access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    let Ok(Some(was)) = w.one(resource, id, &names).await else {
        return not_found(resource, id);
    };
    // A filament with spools: Spoolman's `filament.delete` lets the database refuse it
    // (`spool.filament_id` may not become NULL), the route's 403 (`api/v1/filament.py`).
    if resource == "filament" {
        let used: i64 = match sqlx::query_scalar(
            "SELECT COUNT(*) FROM spool WHERE space_id = $1 AND filament_id = $2",
        )
        .bind(space)
        .bind(id)
        .fetch_one(w.c())
        .await
        {
            Ok(n) => n,
            Err(e) => return crate::failed(e),
        };
        if used > 0 {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"message": "Failed to delete filament, see server logs for more information."})),
            )
                .into_response();
        }
    }
    // A vendor with filaments: "The vendor attribute of any filaments who refer to the deleted
    // vendor will be cleared." (`api/v1/vendor.py`, SQLAlchemy's `Vendor.filaments` without a
    // delete cascade).
    if resource == "vendor"
        && let Err(e) = sqlx::query(
            "UPDATE filament SET vendor_id = NULL WHERE space_id = $1 AND vendor_id = $2",
        )
        .bind(space)
        .bind(id)
        .execute(w.c())
        .await
    {
        return crate::failed(e);
    }
    let removed = sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM {resource} WHERE id = $1 AND space_id = $2"
    )))
    .bind(id)
    .bind(space)
    .execute(w.c())
    .await;
    match removed {
        Ok(_) => {
            // Its tags go with it.
            if resource != "vendor" {
                let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
                    "DELETE FROM tag WHERE space_id = $1 AND {resource}_id = $2"
                )))
                .bind(space)
                .bind(id)
                .execute(w.c())
                .await;
            }
            w.changed();
            w.spoolman_removed(resource, id, was);
            match w.commit(store).await {
                Ok(()) => Json(json!({"message": "Success!"})).into_response(),
                Err(e) => crate::failed(e),
            }
        }
        Err(e) => crate::failed(e),
    }
}

async fn remove_vendor(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
) -> Response {
    remove(&access, &store, "vendor", id).await
}
async fn remove_filament(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
) -> Response {
    remove(&access, &store, "filament", id).await
}
async fn remove_spool(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
) -> Response {
    remove(&access, &store, "spool", id).await
}

// ---- lists of values, settings, export ----------------------------------------------

async fn distinct(access: &crate::access::Access, store: &Store, sql: &'static str) -> Response {
    let space = match space(access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    let values: sqlx::Result<Vec<String>> =
        sqlx::query_scalar(sql).bind(space).fetch_all(w.c()).await;
    match values {
        Ok(v) => model(v).into_response(),
        Err(e) => crate::failed(e),
    }
}

async fn materials(access: crate::access::Access, State(store): State<Store>) -> Response {
    distinct(
        &access,
        &store,
        "SELECT DISTINCT material COLLATE \"C\" FROM filament WHERE space_id = $1 AND material IS NOT NULL ORDER BY 1",
    )
    .await
}
/// Spoolman's `find_locations`: the distinct place names its spools hold. A place of the hub
/// is more than a name (its parent, its slots), and two places with one name are one location
/// here; a place no spool is in is no location. In the order the reference gives them:
/// `SELECT DISTINCT` without `ORDER BY` on SQLite, a scan of the spool table, each name where a
/// spool first holds it.
async fn locations(access: crate::access::Access, State(store): State<Store>) -> Response {
    distinct(
        &access,
        &store,
        "SELECT l.name FROM spool s
         JOIN locations l ON l.space_id = s.space_id AND l.id = s.location_id
         WHERE s.space_id = $1 GROUP BY l.name ORDER BY MIN(s.id)",
    )
    .await
}

/// Spoolman's `rename_location`, `UPDATE spool SET location = new WHERE location = old`: every
/// spool in a place called `old` moves to the place called `new` (made when missing, merged
/// when it is there) the way a `PATCH /spool/{id}` with that location moves it. The place
/// called `old` itself stays. How many spools held the old name.
async fn move_spools(w: &mut Work, old: &str, new: &str) -> Result<i64, Box<Response>> {
    let spools: Vec<i64> = sqlx::query_scalar(
        "SELECT s.id FROM spool s
         JOIN locations l ON l.space_id = s.space_id AND l.id = s.location_id
         WHERE s.space_id = $1 AND l.name = $2 ORDER BY s.id",
    )
    .bind(w.space)
    .bind(old)
    .fetch_all(w.c())
    .await
    .map_err(|e| Box::new(crate::failed(e)))?;
    let to = json!({"location": new});
    for &id in &spools {
        let changes = spool_changes(w, &to, Some(id)).await?;
        update(w, "spool", id, changes)
            .await
            .map_err(|e| Box::new(crate::failed(e)))?;
    }
    if !spools.is_empty() {
        w.changed();
    }
    Ok(spools.len() as i64)
}

/// Spoolman's rename: `PATCH /location/Shelf A` with `{"name": "Shelf B"}`, the new name back
/// whether or not a spool was there.
async fn rename_location(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(name): Path<String>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    // `RenameLocationBody.name`, a `str` of at least 1 character FastAPI checked.
    let new = body["name"].as_str().unwrap_or_default();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    if let Err(r) = move_spools(&mut w, &name, new).await {
        return *r;
    }
    match w.commit(&store).await {
        Ok(()) => model(new).into_response(),
        Err(e) => crate::failed(e),
    }
}

/// Spoolman's `rename_field_value`: one value of `location` or of a spool's text or
/// single-choice extra field replaced wherever it occurs, archived spools included.
async fn rename_field_value(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(field): Path<String>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    // `RenameFieldValueParameters`, two `str`s of at least 1 character FastAPI checked.
    let value = body["value"].as_str().unwrap_or_default();
    let new_value = body["new_value"].as_str().unwrap_or_default();
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    let updated = if field == "location" {
        if new_value.chars().count() > LOCATION_MAX_LENGTH {
            return refused(format!(
                "A location can be at most {LOCATION_MAX_LENGTH} characters."
            ));
        }
        match move_spools(&mut w, value, new_value).await {
            Ok(n) => n,
            Err(r) => return *r,
        }
    } else if let Some(key) = field.strip_prefix(EXTRA_FIELD_PREFIX) {
        let fields = match get_extra_fields(&mut w, "spool").await {
            Ok(f) => f,
            Err(e) => return crate::failed(e),
        };
        if let Err(why) = extra_field_key(&fields, key) {
            return refused(why);
        }
        match rename_extra_value(&mut w, key, value, new_value).await {
            Ok(n) => n,
            Err(e) => return crate::failed(e),
        }
    } else {
        return refused(format!(
            "Cannot rename values of '{field}'. Only fields the spool itself owns can be renamed: \
             location, or '{EXTRA_FIELD_PREFIX}<spool extra field key>'."
        ));
    };
    match w.commit(&store).await {
        Ok(()) => model(json!({"spools_updated": updated})).into_response(),
        Err(e) => crate::failed(e),
    }
}

/// `EXTRA_FIELD_PREFIX` and `LOCATION_MAX_LENGTH` (`database/spool.py`).
const EXTRA_FIELD_PREFIX: &str = "extra.";
const LOCATION_MAX_LENGTH: usize = 64;

/// `_extra_field_key` (`database/spool.py`): the field exists and holds one plain string (text,
/// or choice without multi_choice).
fn extra_field_key(fields: &[Value], key: &str) -> Result<(), String> {
    let Some(field) = fields.iter().find(|f| f["key"] == key) else {
        return Err(format!("Unknown spool extra field '{key}'."));
    };
    let kind = field["field_type"].as_str().unwrap_or_default();
    let multi = kind == "choice" && field["multi_choice"].as_bool().unwrap_or(false);
    if !matches!(kind, "text" | "choice") || multi {
        return Err(format!(
            "Spool extra field '{key}' does not hold a single plain value. \
             Only text and single-choice fields do."
        ));
    }
    Ok(())
}

/// The extra-field half of `rename_field_value`: where the stored value decodes
/// (`extra_field_value_text`, Postgres' `#>> '{}'`) to `value`, `json.dumps(new_value,
/// ensure_ascii=False)` instead. How many spools held it.
async fn rename_extra_value(
    w: &mut Work,
    key: &str,
    value: &str,
    new_value: &str,
) -> sqlx::Result<i64> {
    let rows: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT id, extra FROM spool WHERE space_id = $1 ORDER BY id")
            .bind(w.space)
            .fetch_all(w.c())
            .await?;
    let mut n = 0;
    for (id, extra) in rows {
        let mut map = extra_map(extra);
        let Some(text) = map.get(key).and_then(Value::as_str) else {
            continue;
        };
        let decoded = match python_json::loads_str(text) {
            Ok(python_json::Py::Str(s)) => Some(s),
            Ok(python_json::Py::None) | Err(_) => None,
            Ok(other) => Some(pydantic::to_json(&other).to_string()),
        };
        if decoded.as_deref() != Some(value) {
            continue;
        }
        map.insert(
            key.to_owned(),
            extra_value(Value::String(new_value.to_owned())),
        );
        sqlx::query("UPDATE spool SET extra = $1 WHERE space_id = $2 AND id = $3")
            .bind(Value::Object(map).to_string())
            .bind(w.space)
            .bind(id)
            .execute(w.c())
            .await?;
        n += 1;
    }
    if n > 0 {
        w.changed();
    }
    Ok(n)
}

async fn lot_numbers(access: crate::access::Access, State(store): State<Store>) -> Response {
    distinct(
        &access,
        &store,
        "SELECT DISTINCT lot_nr COLLATE \"C\" FROM spool WHERE space_id = $1 AND lot_nr IS NOT NULL ORDER BY 1",
    )
    .await
}
async fn article_numbers(access: crate::access::Access, State(store): State<Store>) -> Response {
    distinct(
        &access,
        &store,
        "SELECT DISTINCT article_number COLLATE \"C\" FROM filament WHERE space_id = $1 AND article_number IS NOT NULL ORDER BY 1",
    )
    .await
}

/// Spoolman's settings, with their types and defaults, in the order `spoolman/settings.py`
/// registers them.
const SETTINGS: [(&str, &str, &str); 14] = [
    ("currency", "string", "\"EUR\""),
    ("round_prices", "boolean", "false"),
    ("print_presets", "array", "[]"),
    ("label_designs", "array", "[]"),
    ("extra_fields_vendor", "array", "[]"),
    ("extra_fields_filament", "array", "[]"),
    ("extra_fields_spool", "array", "[]"),
    ("base_url", "string", "\"\""),
    ("locations", "array", "[]"),
    ("locations_spoolorders", "object", "{}"),
    ("dashboard_groups", "object", "{}"),
    ("dashboard_spoolorders", "object", "{}"),
    // Maker Connect's own: what a spool's label says, and what its board groups by.
    (
        "label_template",
        "string",
        "\"{code}\\n{vendor} {name}\\n{material} {color}\"",
    ),
    ("board_field", "string", "\"location\""),
];

/// `SettingDefinition.validate_type` (`spoolman/settings.py`): `json.loads(value)` (its
/// `JSONDecodeError` is the message), then the type of what it made.
fn validate_type(key: &str, value: &str) -> Result<(), String> {
    use python_json::Py;
    let Some((_, kind, _)) = SETTINGS.iter().find(|(k, _, _)| *k == key) else {
        return Ok(());
    };
    let obj = python_json::loads_str(value).map_err(|e| e.message(value))?;
    let (ok, what) = match *kind {
        "boolean" => (matches!(obj, Py::Bool(_)), "a boolean"),
        // `isinstance(obj, (int, float))`: a Python bool is an int.
        "number" => (
            matches!(obj, Py::Bool(_) | Py::Int(_) | Py::BigInt(_) | Py::Float(_)),
            "a number",
        ),
        "string" => (matches!(obj, Py::Str(_)), "a string"),
        "array" => (matches!(obj, Py::List(_)), "an array"),
        "object" => (matches!(obj, Py::Dict(_)), "an object"),
        _ => (true, ""),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("Setting {key} must be {what}."))
    }
}

async fn setting_of(w: &mut Work, key: &str) -> Option<Value> {
    let (_, kind, default) = SETTINGS.iter().find(|(k, _, _)| *k == key)?;
    let set: Option<String> =
        sqlx::query_scalar("SELECT value FROM setting WHERE space_id = $1 AND key = $2")
            .bind(w.space)
            .bind(key)
            .fetch_optional(w.c())
            .await
            .ok()
            .flatten();
    Some(json!({
        "value": set.clone().unwrap_or_else(|| (*default).to_owned()),
        "is_set": set.is_some(),
        "type": kind,
    }))
}

async fn settings(access: crate::access::Access, State(store): State<Store>) -> Response {
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    // Spoolman's `find`: first the settings that have been set, in the order the database
    // holds them (a key it no longer knows is left out), then the others with their defaults.
    let set: Vec<(String, String)> =
        match sqlx::query_as("SELECT key, value FROM setting WHERE space_id = $1 ORDER BY row")
            .bind(space)
            .fetch_all(w.c())
            .await
        {
            Ok(rows) => rows,
            Err(e) => return crate::failed(e),
        };
    let mut all = Map::new();
    for (key, value) in set {
        if let Some((_, kind, _)) = SETTINGS.iter().find(|(k, _, _)| *k == key) {
            all.insert(key, json!({"value": value, "is_set": true, "type": kind}));
        }
    }
    for (key, kind, default) in SETTINGS {
        if !all.contains_key(key) {
            all.insert(
                key.to_owned(),
                json!({"value": default, "is_set": false, "type": kind}),
            );
        }
    }
    model(Value::Object(all)).into_response()
}

async fn setting(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(key): Path<String>,
) -> Response {
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    let _ = space;
    match setting_of(&mut w, &key).await {
        Some(v) => model(v).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"message": format!("Setting {key} does not exist.")})),
        )
            .into_response(),
    }
}

async fn set_setting(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(key): Path<String>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    if !SETTINGS.iter().any(|(k, _, _)| *k == key) {
        // `parse_setting`'s `ValueError`.
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"message": format!("Setting {key} does not exist.")})),
        )
            .into_response();
    }
    // The body is a `str` (FastAPI checked it): the value as JSON text, `"\"USD\""`.
    let value = body.as_str().unwrap_or_default().to_owned();
    // `if body and body != "null"`: set; otherwise the setting is unset.
    let set = !value.is_empty() && value != "null";
    if set {
        if let Err(why) =
            validate_type(&key, &value).and_then(|()| validate_extra_field_setting(&key, &value))
        {
            return refused(why);
        }
        // `database.setting.update`: `SETTING_MAX_LENGTH = 2**16 - 1`, on `len(value)`.
        if value.chars().count() > 65535 {
            return refused("Setting value is too big, max size is 65535 characters.");
        }
    }
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    let saved = if set {
        sqlx::query(
            "INSERT INTO setting (space_id, key, value, last_updated) VALUES ($1, $2, $3, at_unix($4))
             ON CONFLICT (space_id, key) DO UPDATE SET value = $3, last_updated = at_unix($4)",
        )
        .bind(space)
        .bind(&key)
        .bind(&value)
        .bind(now())
        .execute(w.c())
        .await
    } else {
        let gone = sqlx::query("DELETE FROM setting WHERE space_id = $1 AND key = $2")
            .bind(space)
            .bind(&key)
            .execute(w.c())
            .await;
        // `database.setting.delete` gets it first: `ItemNotFoundError`, a 404.
        if matches!(&gone, Ok(r) if r.rows_affected() == 0) {
            return missing(format!("Setting with key {key} has not been set."));
        }
        gone
    };
    match saved {
        Ok(_) => {
            let now = setting_of(&mut w, &key).await;
            match w.commit(&store).await {
                Ok(()) => model(now).into_response(),
                Err(e) => crate::failed(e),
            }
        }
        Err(e) => crate::failed(e),
    }
}

/// `/export/spools?fmt=csv` (or `json`): every row, nested fields flattened with dots.
async fn export(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(what): Path<String>,
    pydantic::Params(q): pydantic::Params,
) -> Response {
    let resource = match what.as_str() {
        "spools" => "spool",
        "filaments" => "filament",
        "vendors" => "vendor",
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let names = match names_for(&store, access.space, resource).await {
        Ok(names) => names,
        Err(e) => return crate::failed(e),
    };
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let all = match crate::stock::in_space!(&store, space, |w| w.all(resource, &names)).await {
        Ok(all) => all,
        Err(e) => return crate::failed(e),
    };
    if q.get("fmt").and_then(Value::as_str) == Some("json") {
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
    store: Store,
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
async fn live(mut socket: WebSocket, store: Store, space: i64, resource: String, id: Option<i64>) {
    let mut events = store.subscribe_spoolman();
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
                        let Ok(names) = names_for(&store, space, event.resource).await else {
                            continue;
                        };
                        let one = crate::stock::in_space!(&store, space, |w| {
                            w.one(event.resource, event.id, &names)
                        })
                        .await;
                        match one {
                            Ok(Some(v)) => v,
                            _ => continue,
                        }
                    }
                };
                let message = without_nulls(json!({
                    "type": event.kind,
                    "resource": event.resource,
                    "date": iso(now()),
                    "payload": payload,
                }));
                if socket.send(Message::Text(message.to_string().into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    // Spoolman's `notify`: a text that is not empty is answered.
                    Some(Ok(Message::Text(t))) if !t.is_empty() => {
                        let healthy = json!({"status": "healthy"}).to_string();
                        if socket.send(Message::Text(healthy.into())).await.is_err() { break; }
                    }
                    Some(Ok(_)) => {}
                    _ => break,
                }
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
    // `[\s:_.-]+` removed (Python's `\s` also takes U+001C..U+001F), trimmed, upper case, no `0X`.
    let cleaned: String = uid
        .chars()
        .filter(|c| {
            !c.is_whitespace()
                && !('\u{1c}'..='\u{1f}').contains(c)
                && !matches!(c, ':' | '_' | '.' | '-')
        })
        .collect::<String>()
        .to_uppercase();
    let cleaned = cleaned.strip_prefix("0X").unwrap_or(&cleaned).to_owned();
    if cleaned.is_empty() {
        return Err("A tag UID must not be empty.".into());
    }
    if !cleaned.chars().all(|c| matches!(c, '0'..='9' | 'A'..='F')) {
        return Err(format!(
            "\"{uid}\" is not a valid tag UID. A UID must be hexadecimal."
        ));
    }
    if cleaned.len() > UID_MAX_LENGTH {
        return Err(format!(
            "A tag UID can be at most {UID_MAX_LENGTH} hex characters."
        ));
    }
    if !cleaned.len().is_multiple_of(2) {
        return Err(
            "A tag UID must contain a whole number of bytes (an even number of hex characters)."
                .into(),
        );
    }
    Ok(cleaned)
}

/// Links a tag to a spool or a filament: 201 with the tag, 409 with who holds it.
async fn link(
    access: &crate::access::Access,
    store: &Store,
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
    let format = openprinttag::normalize_format(body["format"].as_str());
    let space = match space(access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    // Whether it is there (its `loaded_in` does not matter here).
    if !matches!(w.one(target, id, &MachineNames::new()).await, Ok(Some(_))) {
        return not_found(target, id);
    }
    let holder: Option<(String, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT target_type, spool_id, filament_id FROM tag WHERE space_id = $1 AND uid = $2",
    )
    .bind(space)
    .bind(&uid)
    .fetch_optional(w.c())
    .await
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
                let _ = sqlx::query("UPDATE tag SET format = $1 WHERE space_id = $2 AND uid = $3")
                    .bind(format)
                    .bind(space)
                    .bind(&uid)
                    .execute(w.c())
                    .await;
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
            if let Err(e) = sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO tag (space_id, uid, target_type, {column}, format, added)
                 VALUES ($1, $2, $3, $4, $5, at_unix($6))"
            )))
            .bind(space)
            .bind(&uid)
            .bind(target)
            .bind(id)
            .bind(&format)
            .bind(now())
            .execute(w.c())
            .await
            {
                return crate::failed(e);
            }
        }
    }
    w.changed();
    w.spoolman_event(target, "updated", id);
    let tags = w.tags_of(target, id).await.unwrap_or_default();
    let tag = tags
        .as_array()
        .and_then(|t| t.iter().find(|t| t["uid"] == uid).cloned())
        .unwrap_or(Value::Null);
    match w.commit(store).await {
        Ok(()) => (StatusCode::CREATED, model(tag)).into_response(),
        Err(e) => crate::failed(e),
    }
}

async fn unlink(
    access: &crate::access::Access,
    store: &Store,
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
    let space = match space(access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let mut w = match store.work(space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    let column = if target == "spool" {
        "spool_id"
    } else {
        "filament_id"
    };
    let removed = sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM tag WHERE space_id = $1 AND uid = $2 AND {column} = $3"
    )))
    .bind(space)
    .bind(&uid)
    .bind(id)
    .execute(w.c())
    .await
    .map(|r| r.rows_affected());
    match removed {
        Ok(0) => (
            StatusCode::NOT_FOUND,
            Json(json!({"message": format!("Tag {uid} is not on this {target}.")})),
        )
            .into_response(),
        Ok(_) => {
            w.changed();
            w.spoolman_event(target, "updated", id);
            match w.commit(store).await {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(e) => crate::failed(e),
            }
        }
        Err(e) => crate::failed(e),
    }
}

async fn tag_spool(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    link(&access, &store, "spool", id, &body).await
}
async fn untag_spool(
    access: crate::access::Access,
    State(store): State<Store>,
    Path((id, uid)): Path<(i64, String)>,
) -> Response {
    unlink(&access, &store, "spool", id, &uid).await
}
async fn tag_filament(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    link(&access, &store, "filament", id, &body).await
}
async fn untag_filament(
    access: crate::access::Access,
    State(store): State<Store>,
    Path((id, uid)): Path<(i64, String)>,
) -> Response {
    unlink(&access, &store, "filament", id, &uid).await
}

// ---- tag scans: Spoolman's `api/v1/tag.py`, `scanrelay.py` and `database/tag.py` ---------------
//
// Ported from Spoolman `eccb05e` (MIT): the scan's body model (`TagScanParameters`), the scan
// (`scan`, `_decode_payload`), the relay (`ScanRelay`: the reader registry and the debounce of
// the broadcast, `derive_reader_id`) and `create_spool_from_decoded_tag`. One relay per space:
// a space is one Spoolman.

/// `scanrelay.py`: how long a repeat of the same scan is not broadcast again.
const DEBOUNCE_WINDOW_SECS: i64 = 3;
/// How long a reader stays listed after its last scan.
const READER_TTL_SECS: i64 = 24 * 3600;
const MAX_READERS: usize = 256;
const MAX_DEBOUNCE_ENTRIES: usize = 1024;
const READER_ID_MAX_LENGTH: usize = 64;
/// `tags/__init__.py` and `api/v1/tag.py`.
const UID_MAX_LENGTH: usize = 64;

/// A reader that has reported a scan (`scanrelay.Reader`).
struct Reader {
    reader_id: String,
    name: Option<String>,
    last_seen: i64,
    last_uid: Option<String>,
}

/// What a scan resolved to, part of the debounce key: `("spool", 7)`, `("filament", 7)` or none.
type Matched = Option<(&'static str, i64)>;

#[derive(Default)]
struct Relay {
    /// Least- to most-recently seen, as Spoolman's dict.
    readers: Vec<Reader>,
    /// (uid, reader, match) and when it was last broadcast, oldest first.
    last_broadcast: Vec<((String, String, Matched), i64)>,
}

impl Relay {
    fn register(&mut self, reader_id: &str, name: Option<String>, uid: Option<String>, now: i64) {
        let existing = self
            .readers
            .iter()
            .position(|r| r.reader_id == reader_id)
            .map(|i| self.readers.remove(i));
        let (old_name, old_uid) = existing.map_or((None, None), |r| (r.name, r.last_uid));
        self.readers.push(Reader {
            reader_id: reader_id.to_owned(),
            name: name.or(old_name),
            last_seen: now,
            last_uid: uid.or(old_uid),
        });
        self.prune_readers(now);
        while self.readers.len() > MAX_READERS {
            self.readers.remove(0);
        }
    }

    fn should_broadcast(&mut self, uid: &str, reader_id: &str, matched: Matched, now: i64) -> bool {
        let key = (uid.to_owned(), reader_id.to_owned(), matched);
        self.last_broadcast
            .retain(|(_, seen)| *seen >= now - DEBOUNCE_WINDOW_SECS);
        if let Some(i) = self.last_broadcast.iter().position(|(k, _)| *k == key) {
            if now - self.last_broadcast[i].1 < DEBOUNCE_WINDOW_SECS {
                return false;
            }
            self.last_broadcast[i].1 = now;
        } else {
            self.last_broadcast.push((key, now));
        }
        while self.last_broadcast.len() > MAX_DEBOUNCE_ENTRIES {
            self.last_broadcast.remove(0);
        }
        true
    }

    /// Most recently seen first.
    fn readers(&mut self, now: i64) -> Vec<Value> {
        self.prune_readers(now);
        self.readers
            .iter()
            .rev()
            .map(|r| {
                without_nulls(json!({
                    "reader_id": r.reader_id,
                    "name": r.name,
                    "last_seen": iso(r.last_seen),
                    "last_uid": r.last_uid,
                }))
            })
            .collect()
    }

    fn prune_readers(&mut self, now: i64) {
        self.readers
            .retain(|r| r.last_seen >= now - READER_TTL_SECS);
    }
}

/// The readers that scanned lately and the scans as they come, per space (in memory, as
/// Spoolman's). One per hub (kept in its `Store`), so two hubs in one process, as tests run
/// them, never see each other's.
pub(crate) struct Scans {
    relays: std::sync::Mutex<HashMap<i64, Relay>>,
    sent: tokio::sync::broadcast::Sender<(i64, Value)>,
}

impl Default for Scans {
    fn default() -> Self {
        Scans {
            relays: std::sync::Mutex::new(HashMap::new()),
            sent: tokio::sync::broadcast::channel(64).0,
        }
    }
}

/// `derive_reader_id`: a reader id from the client's address.
fn derive_reader_id(host: &str) -> String {
    let mut out = String::new();
    for c in host.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let sanitized = out.trim_matches('-');
    if sanitized.is_empty() {
        return "unknown".into();
    }
    format!("ip-{sanitized}")
        .chars()
        .take(READER_ID_MAX_LENGTH)
        .collect()
}

/// Spoolman's `FilamentParameters.color_hex_validator`, what it returns (its refusals are not
/// ported yet): the code without its leading `#`, the case as given.
fn color_hex_given(v: &str) -> Option<String> {
    if v.is_empty() {
        return None;
    }
    Some(v.strip_prefix('#').unwrap_or(v).to_owned())
}

/// Spoolman's `FilamentParameters.multi_color_hexes_validator`, what it returns.
fn multi_color_hexes_given(v: &str) -> Option<String> {
    if v.is_empty() {
        return None;
    }
    let colors: Vec<&str> = v
        .split(',')
        .map(|c| c.strip_prefix('#').unwrap_or(c))
        .collect();
    Some(colors.join(","))
}

/// Spoolman's `_sanitize_color_hex` (`api/v1/models.py`), on a filament read from the
/// database: upper case, no leading `#`, and nothing when it is not a 6 or 8 digit code.
fn sanitize_color_hex(value: Option<&str>) -> Option<String> {
    let value = value.filter(|v| !v.is_empty())?;
    let upper = value.to_uppercase();
    let clr = upper.strip_prefix('#').unwrap_or(&upper);
    let len = clr.chars().count();
    if !(len == 6 || len == 8) || clr.chars().any(|c| !"0123456789ABCDEF".contains(c)) {
        return None;
    }
    Some(clr.to_owned())
}

/// Spoolman's `_sanitize_multi_color_hexes`: each code sanitised, the bad ones left out,
/// nothing when fewer than two are left.
fn sanitize_multi_color_hexes(value: Option<&str>) -> Option<String> {
    let value = value.filter(|v| !v.is_empty())?;
    let colors: Vec<String> = value
        .split(',')
        .filter_map(|part| sanitize_color_hex(Some(part)))
        .collect();
    if colors.len() < 2 {
        return None;
    }
    Some(colors.join(","))
}

/// Spoolman's `spoolman/math.py` `weight_from_length`, in its order of operations: the
/// weight in g of `length` mm of filament.
fn weight_from_length(length: f64, diameter: f64, density: f64) -> f64 {
    let volume_mm3 = length * std::f64::consts::PI * (diameter / 2.0).powi(2);
    let volume_cm3 = volume_mm3 / 1000.0;
    density * volume_cm3
}

/// Spoolman's `spoolman/math.py` `length_from_weight`, in its order of operations: the
/// length in mm of `weight` g of filament.
fn length_from_weight(weight: f64, diameter: f64, density: f64) -> f64 {
    let volume_cm3 = weight / density;
    let volume_mm3 = volume_cm3 * 1000.0;
    volume_mm3 / (std::f64::consts::PI * (diameter / 2.0).powi(2))
}

/// An answer of a route Spoolman declares with `response_model_exclude_none=True` (all of
/// its JSON routes but `/info`, `/health`, `/backup` and the exports): its model without
/// null fields. Spoolman's error answers (`JSONResponse`) keep theirs.
fn model(v: impl serde::Serialize) -> Json<Value> {
    Json(without_nulls(
        serde_json::to_value(v).unwrap_or(Value::Null),
    ))
}

/// Pydantic's `exclude_none`: every null field of every object left out.
fn without_nulls(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, without_nulls(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(without_nulls).collect()),
        v => v,
    }
}

/// `_decode_payload`: the payload decoded, when a format and a payload were given; `None` on
/// anything else (no field, bad base64, an unknown format, a payload that does not decode).
fn decode_payload(
    format: Option<&str>,
    payload_b64: Option<&str>,
    uid: &str,
) -> Option<openprinttag::Decoded> {
    use base64::Engine as _;
    let (format, payload_b64) = (format?, payload_b64?);
    // `base64.b64decode(validate=True)`: the standard alphabet, padded, extra bits ignored.
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::RequireCanonical),
    );
    let raw = engine.decode(payload_b64).ok()?;
    let uid_bytes: Option<Vec<u8>> = (0..uid.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(uid.get(i..i + 2)?, 16).ok())
        .collect();
    openprinttag::decode(Some(format), &raw, uid_bytes.as_deref())
}

/// `create_spool_from_decoded_tag`'s checks, before anything is written: the density and
/// diameter to store, or `None` when Spoolman refuses the values (`ItemCreateError`).
fn storable(d: &openprinttag::Decoded) -> Option<(f64, f64)> {
    let density = d
        .density
        .unwrap_or_else(|| openprinttag::density_or_fallback(d.material.as_deref()));
    let diameter = d.diameter.unwrap_or(1.75);
    let finite = [
        Some(density),
        Some(diameter),
        d.weight,
        d.initial_weight,
        d.spool_weight,
    ]
    .iter()
    .flatten()
    .all(|v| v.is_finite());
    let within = |v: Option<f64>, low: f64, high: f64| v.is_none_or(|v| (low..=high).contains(&v));
    let chars = |s: &Option<String>| s.as_ref().map_or(0, |s| s.chars().count());
    let ok = finite
        && within(Some(diameter), 0.5, 5.0)
        && within(Some(density), 0.3, 10.0)
        && within(d.weight, 1.0, 50000.0)
        && within(d.initial_weight, 1.0, 50000.0)
        && within(d.spool_weight, 1.0, 50000.0)
        // VendorParameters, FilamentParameters, SpoolParameters.
        && chars(&d.vendor_name) <= 64
        && density > 0.0
        && diameter > 0.0
        && chars(&d.material) <= 64
        && chars(&d.name) <= 64
        && d.weight.is_none_or(|w| w > 0.0)
        && d.spool_weight.is_none_or(|w| w >= 0.0)
        && chars(&d.article_number) <= 64
        && d.extruder_temp.is_none_or(|t| t >= 0)
        && d.bed_temp.is_none_or(|t| t >= 0)
        && d.initial_weight.is_none_or(|w| w >= 0.0)
        && d.color_hex.as_deref().is_none_or(|c| {
            let c = c.to_uppercase();
            let c = c.strip_prefix('#').unwrap_or(&c);
            c.is_empty()
                || (c.chars().all(|x| x.is_ascii_hexdigit()) && matches!(c.len(), 6 | 8))
        });
    ok.then_some((density, diameter))
}

/// `create_spool_from_decoded_tag`: a spool made from a decoded tag and linked to it, the
/// vendor and the filament reused when they are there; all in one transaction, told after it.
/// `Ok(None)`: the tag was linked by another scan first.
async fn create_from_tag(
    store: &Store,
    space: i64,
    uid: &str,
    format: Option<&str>,
    d: &openprinttag::Decoded,
    (density, diameter): (f64, f64),
) -> sqlx::Result<Option<i64>> {
    let mut w = store.work(space).await?;
    let mut vendor = None;
    if let Some(name) = d.vendor_name.as_deref().filter(|n| !n.is_empty()) {
        vendor = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM vendor WHERE space_id = $1 AND name = $2 ORDER BY id LIMIT 1",
        )
        .bind(space)
        .bind(name)
        .fetch_optional(w.c())
        .await?;
        if vendor.is_none() {
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO vendor (space_id, registered, name) VALUES ($1, at_unix($2), $3) RETURNING id",
            )
            .bind(space)
            .bind(now())
            .bind(name)
            .fetch_one(w.c())
            .await?;
            w.spoolman_event("vendor", "added", id);
            vendor = Some(id);
        }
    }
    // The filament of this product: vendor, name, material, colour, diameter and weight.
    let mut filament = None;
    if let (Some(name), Some(material)) = (&d.name, &d.material) {
        let candidates: Vec<(i64, Option<f64>, Option<f64>)> = sqlx::query_as(
            "SELECT id, diameter, weight FROM filament WHERE space_id = $1 AND name = $2
             AND material = $3 AND vendor_id IS NOT DISTINCT FROM $4
             AND color_hex IS NOT DISTINCT FROM $5 ORDER BY id",
        )
        .bind(space)
        .bind(name)
        .bind(material)
        .bind(vendor)
        .bind(&d.color_hex)
        .fetch_all(w.c())
        .await?;
        let close = |a: Option<f64>, b: Option<f64>, tol: f64| match (a, b) {
            (Some(a), Some(b)) => (a - b).abs() <= tol,
            (a, b) => a.is_none() && b.is_none(),
        };
        filament = candidates
            .into_iter()
            .find(|(_, dia, wt)| close(*dia, Some(diameter), 0.01) && close(*wt, d.weight, 1.0))
            .map(|(id, _, _)| id);
    }
    let filament = match filament {
        Some(f) => f,
        None => {
            let body = json!({
                "density": density, "diameter": diameter, "material": d.material, "name": d.name,
                "vendor_id": vendor, "weight": d.weight, "spool_weight": d.spool_weight,
                "article_number": d.article_number, "color_hex": d.color_hex,
                "settings_extruder_temp": d.extruder_temp, "settings_bed_temp": d.bed_temp,
            });
            let changes = filament_changes(&mut w, &body).await?.unwrap_or_default();
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO filament (space_id, registered) VALUES ($1, at_unix($2)) RETURNING id",
            )
            .bind(space)
            .bind(now())
            .fetch_one(w.c())
            .await?;
            update(&mut w, "filament", id, changes).await?;
            w.spoolman_event("filament", "added", id);
            id
        }
    };
    // spool.create: the weights the tag does not give come from the filament.
    let f = w.sm_filament(filament).await?.unwrap_or(Value::Null);
    let initial = d.initial_weight.or(f["weight"].as_f64());
    let spool_weight = d.spool_weight.or(f["spool_weight"].as_f64());
    let spool: i64 = sqlx::query_scalar(
        "INSERT INTO spool (space_id, registered, filament_id) VALUES ($1, at_unix($2), $3) RETURNING id",
    )
    .bind(space)
    .bind(now())
    .bind(filament)
    .fetch_one(w.c())
    .await?;
    update(
        &mut w,
        "spool",
        spool,
        vec![
            ("initial_weight", real(initial)),
            ("spool_weight", real(spool_weight)),
            ("used_weight", real(Some(0.0))),
        ],
    )
    .await?;
    w.movement(
        &crate::stock::spool_code(spool),
        "received",
        initial,
        None,
        None,
    )
    .await?;
    let linked = sqlx::query(
        "INSERT INTO tag (space_id, uid, target_type, spool_id, format, added)
         VALUES ($1, $2, 'spool', $3, $4, at_unix($5)) ON CONFLICT DO NOTHING",
    )
    .bind(space)
    .bind(uid)
    .bind(spool)
    .bind(openprinttag::normalize_format(format))
    .bind(now())
    .execute(w.c())
    .await?;
    if linked.rows_affected() == 0 {
        // Another scan linked this UID first: nothing of this one is kept.
        return Ok(None);
    }
    w.spoolman_event("spool", "added", spool);
    w.changed();
    w.commit(store).await?;
    Ok(Some(spool))
}

/// `POST /tag/scan`: a reader reports a tag it read. What it is linked to comes back, with the
/// payload decoded when its format is one Spoolman reads; `create` makes a spool of a decoded,
/// unlinked filament tag. The scan goes out on the scan WebSockets, a repeat within 3 s not.
async fn scan(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    // `TagScanParameters`, which FastAPI checked.
    let field = |k: &str| body.get(k).and_then(Value::as_str).map(str::to_owned);
    let raw_uid = field("uid");
    let reader_id = field("reader_id");
    let name = field("name");
    let format = field("format");
    let payload_b64 = field("payload_b64");
    let create = body.get("create").and_then(Value::as_bool).unwrap_or(false);
    let reader = reader_id.unwrap_or_else(|| derive_reader_id(&peer.ip().to_string()));
    let uid = match normalize_uid(&raw_uid.unwrap_or_default()) {
        Ok(uid) => uid,
        Err(why) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"message": why}))).into_response();
        }
    };
    let space = match space(&access) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let names = match names_for(&store, access.space, "spool").await {
        Ok(names) => names,
        Err(e) => return crate::failed(e),
    };
    let held = |store: Store, uid: String| async move {
        let mut w = store.work(space).await?;
        let held: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT spool_id, filament_id FROM tag WHERE space_id = $1 AND uid = $2",
        )
        .bind(space)
        .bind(&uid)
        .fetch_optional(w.c())
        .await?;
        Ok::<_, sqlx::Error>(held.unwrap_or((None, None)))
    };
    let (mut spool_id, filament_id) = match held(store.clone(), uid.clone()).await {
        Ok(h) => h,
        Err(e) => return crate::failed(e),
    };
    let mut decoded = decode_payload(format.as_deref(), payload_b64.as_deref(), &uid);
    let mut created = false;
    // Only a filament (FFF) tag creates; one linked to anything is never relinked.
    let creatable = decoded
        .as_ref()
        .is_some_and(|d| d.material_class.as_deref() == Some("FFF"));
    if let Some(d) = decoded
        .clone()
        .filter(|_| spool_id.is_none() && filament_id.is_none() && create && creatable)
    {
        match storable(&d) {
            None => decoded = None,
            Some(values) => {
                match create_from_tag(&store, space, &uid, format.as_deref(), &d, values).await {
                    Ok(Some(id)) => {
                        spool_id = Some(id);
                        created = true;
                    }
                    Ok(None) => match held(store.clone(), uid.clone()).await {
                        Ok((s, _)) => spool_id = s,
                        Err(e) => return crate::failed(e),
                    },
                    Err(e) => return crate::failed(e),
                }
            }
        }
    }
    let (spool, filament) = {
        let mut w = match store.work(space).await {
            Ok(w) => w,
            Err(e) => return crate::failed(e),
        };
        let spool = match spool_id {
            Some(s) => w.sm_spool(s, &names).await.ok().flatten(),
            None => None,
        };
        let filament = match filament_id {
            Some(f) => w.sm_filament(f).await.ok().flatten(),
            None => None,
        };
        (spool, filament)
    };
    let result = json!({
        "uid": uid,
        "reader_id": reader,
        "name": name,
        "format": format,
        "payload_b64": payload_b64,
        "matched_spool_id": spool.as_ref().map(|s| s["id"].clone()),
        "spool": spool,
        "matched_filament_id": filament.as_ref().map(|f| f["id"].clone()),
        "filament": filament,
        "decoded": decoded,
        "created": created,
    });
    let matched: Matched = match (&spool, &filament) {
        (Some(s), _) => s["id"].as_i64().map(|id| ("spool", id)),
        (None, Some(f)) => f["id"].as_i64().map(|id| ("filament", id)),
        _ => None,
    };
    let scans = store.spoolman_scans();
    let broadcast = {
        let mut relays = scans.relays.lock().unwrap();
        let relay = relays.entry(access.space).or_default();
        let t = now();
        relay.register(&reader, name.clone(), Some(uid.clone()), t);
        relay.should_broadcast(&uid, &reader, matched, t)
    };
    let content = without_nulls(result);
    if broadcast {
        let _ = scans.sent.send((
            access.space,
            json!({
                "type": "scanned",
                "resource": "tag_scan",
                "date": iso(now()),
                "payload": content,
            }),
        ));
    }
    // A null match is the answer to the device's question: it stays, last if it was null.
    let mut content = content;
    for key in ["matched_spool_id", "matched_filament_id"] {
        if content.get(key).is_none() {
            content[key] = Value::Null;
        }
    }
    Json(content).into_response()
}

async fn readers(access: crate::access::Access, State(store): State<Store>) -> Json<Value> {
    let scans = store.spoolman_scans();
    let mut relays = scans.relays.lock().unwrap();
    model(Value::Array(
        relays.entry(access.space).or_default().readers(now()),
    ))
}

async fn scan_live(
    access: crate::access::Access,
    State(store): State<Store>,
    MaybeWs(ws): MaybeWs,
) -> Response {
    let scans = store.spoolman_scans();
    match ws {
        Some(ws) => ws.on_upgrade(move |socket| scan_feed(socket, scans, access.space, None)),
        None => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

async fn scan_live_reader(
    access: crate::access::Access,
    State(store): State<Store>,
    MaybeWs(ws): MaybeWs,
    Path(reader): Path<String>,
) -> Response {
    let scans = store.spoolman_scans();
    match ws {
        Some(ws) => {
            ws.on_upgrade(move |socket| scan_feed(socket, scans, access.space, Some(reader)))
        }
        None => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

async fn scan_feed(
    mut socket: WebSocket,
    scans: std::sync::Arc<Scans>,
    space: i64,
    reader: Option<String>,
) {
    let mut feed = scans.sent.subscribe();
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
                match incoming {
                    // Spoolman's `notify`: a text that is not empty is answered.
                    Some(Ok(Message::Text(t))) if !t.is_empty() => {
                        let healthy = json!({"status": "healthy"}).to_string();
                        if socket.send(Message::Text(healthy.into())).await.is_err() { break; }
                    }
                    Some(Ok(_)) => {}
                    _ => break,
                }
            }
        }
    }
}

// ---- extra fields ---------------------------------------------------------------------
//
// Spoolman's `extra_field_registry.py`, `extra_fields.py` and `api/v1/field.py`: an entity's
// extra fields are the setting `extra_fields_<entity>`, a JSON list of `ExtraField`s as
// `json.dumps(jsonable_encoder(fields))` writes it (every field in the model's order, `null`
// included), in the order they were added; a changed field goes to the end. Not ported: the
// in-memory `extra_field_cache` (the hub reads the setting each time, so there is nothing to
// invalidate).

/// `EXTRA_FIELD_VALUE_MAX_LENGTH = 2**16 - 1`.
const EXTRA_FIELD_VALUE_MAX_LENGTH: usize = 65535;
/// `MAX_EXTRA_FIELDS_PER_ENTITY = 128`.
const MAX_EXTRA_FIELDS_PER_ENTITY: usize = 128;

fn entity(entity: &str) -> Option<&'static str> {
    ["vendor", "filament", "spool"]
        .into_iter()
        .find(|e| *e == entity)
}

/// An `ExtraField` as `jsonable_encoder` gives it: its fields in the model's order, the ones
/// not given at their defaults (`order` 0, the others `None`).
fn extra_field_json(given: &Value) -> Value {
    let mut out = Map::new();
    for key in [
        "name",
        "order",
        "unit",
        "field_type",
        "default_value",
        "choices",
        "multi_choice",
        "key",
        "entity_type",
    ] {
        let default = if key == "order" {
            json!(0)
        } else {
            Value::Null
        };
        out.insert(key.to_owned(), given.get(key).cloned().unwrap_or(default));
    }
    Value::Object(out)
}

/// `get_extra_fields`: the setting, or its default `[]`; not a list: the default (Spoolman logs
/// "Setting %s is not a list, using default."); each item `ExtraField.parse_obj`. Err: what
/// Spoolman raises there, a 500.
async fn get_extra_fields(w: &mut Work, entity: &str) -> Result<Vec<Value>, String> {
    let set: Option<String> =
        sqlx::query_scalar("SELECT value FROM setting WHERE space_id = $1 AND key = $2")
            .bind(w.space)
            .bind(format!("extra_fields_{entity}"))
            .fetch_optional(w.c())
            .await
            .map_err(|e| e.to_string())?;
    let text = set.unwrap_or_else(|| "[]".to_owned());
    let array = python_json::loads_str(&text).map_err(|e| e.message(&text))?;
    let python_json::Py::List(items) = array else {
        return Ok(Vec::new());
    };
    items
        .iter()
        .map(|obj| {
            pydantic::model_validate("ExtraField", obj)
                .map(|v| extra_field_json(&pydantic::to_json(&v)))
                .map_err(|es| pydantic::validation_error_str("ExtraField", &es))
        })
        .collect()
}

/// `db_setting.update` of `extra_fields_<entity>`: Ok(Err) is its `ValueError` (the value past
/// `SETTING_MAX_LENGTH`).
async fn save_extra_fields(
    w: &mut Work,
    entity: &str,
    fields: &[Value],
) -> sqlx::Result<Result<(), String>> {
    let value = python_json::dumps(&Value::Array(fields.to_vec()));
    if value.chars().count() > 65535 {
        return Ok(Err(
            "Setting value is too big, max size is 65535 characters.".to_owned(),
        ));
    }
    sqlx::query(
        "INSERT INTO setting (space_id, key, value, last_updated) VALUES ($1, $2, $3, at_unix($4))
         ON CONFLICT (space_id, key) DO UPDATE SET value = $3, last_updated = at_unix($4)",
    )
    .bind(w.space)
    .bind(format!("extra_fields_{entity}"))
    .bind(&value)
    .bind(now())
    .execute(w.c())
    .await?;
    Ok(Ok(()))
}

/// `validate_extra_field_value`.
fn validate_extra_field_value(field: &Value, value: &str) -> Result<(), String> {
    use python_json::Py;
    if value.chars().count() > EXTRA_FIELD_VALUE_MAX_LENGTH {
        return Err(format!(
            "Value is too big, max size is {EXTRA_FIELD_VALUE_MAX_LENGTH} characters."
        ));
    }
    let data = python_json::loads_str(value).map_err(|_| "Value is not valid JSON.".to_owned())?;
    // `isinstance(data, int)`; a JSON true/false is `Py::Bool`, never an int here.
    let int = |p: &Py| matches!(p, Py::Int(_) | Py::BigInt(_));
    let num = |p: &Py| int(p) || matches!(p, Py::Float(_));
    let fail = |m: &str| Err(m.to_owned());
    let range = |data: &Py, ok: &dyn Fn(&Py) -> bool, what: &str| -> Result<(), String> {
        let Py::List(items) = data else {
            return fail("Value is not a list.");
        };
        if items.len() != 2 {
            return fail("Value list must have exactly two values.");
        }
        if !items.iter().all(|i| ok(i) || matches!(i, Py::None)) {
            return Err(format!("Value list must contain only {what} or null."));
        }
        Ok(())
    };
    let in_choices = |s: &str| {
        field["choices"]
            .as_array()
            .is_none_or(|c| c.iter().any(|c| c.as_str() == Some(s)))
    };
    match field["field_type"].as_str().unwrap_or_default() {
        "text" | "datetime" if !matches!(data, Py::Str(_)) => fail("Value is not a string."),
        "integer" if !int(&data) => fail("Value is not an integer."),
        "integer_range" => range(&data, &int, "integers"),
        "float" if !num(&data) => fail("Value is not a float."),
        "float_range" => range(&data, &num, "floats"),
        "boolean" if !matches!(data, Py::Bool(_)) => fail("Value is not a boolean."),
        // `if field.multi_choice:`
        "choice" if field["multi_choice"] == json!(true) => {
            let Py::List(items) = &data else {
                return fail("Value is not a list.");
            };
            if !items.iter().all(|i| matches!(i, Py::Str(_))) {
                return fail("Value list must contain only strings.");
            }
            if !items
                .iter()
                .all(|i| matches!(i, Py::Str(s) if in_choices(s)))
            {
                return fail("Value list contains invalid choices.");
            }
            Ok(())
        }
        "choice" => match &data {
            Py::Str(s) if in_choices(s) => Ok(()),
            Py::Str(_) => fail("Value is not a valid choice."),
            _ => fail("Value is not a string."),
        },
        _ => Ok(()),
    }
}

/// `validate_extra_field`.
fn validate_extra_field(field: &Value) -> Result<(), String> {
    if field["field_type"] == "choice" {
        if field["choices"].is_null() {
            return Err("Choices must be set for field type choice.".to_owned());
        }
        if field["multi_choice"].is_null() {
            return Err("Multi choice must be set for field type choice.".to_owned());
        }
    } else {
        if !field["choices"].is_null() {
            return Err("Choices must not be set for field type other than choice.".to_owned());
        }
        if !field["multi_choice"].is_null() {
            return Err(
                "Multi choice must not be set for field type other than choice.".to_owned(),
            );
        }
    }
    if let Some(default) = field["default_value"].as_str() {
        validate_extra_field_value(field, default)
            .map_err(|e| format!("Default value is not valid: {e}"))?;
    }
    Ok(())
}

/// `validate_extra_field_setting`: what `POST /setting/extra_fields_<entity>` writes must be a
/// list of valid extra fields of that entity, without a key twice.
fn validate_extra_field_setting(key: &str, value: &str) -> Result<(), String> {
    use python_json::Py;
    let Some(entity) = key.strip_prefix("extra_fields_").and_then(entity) else {
        return Ok(());
    };
    let fields = python_json::loads_str(value).map_err(|e| e.message(value))?;
    let Py::List(items) = &fields else {
        return Err(format!("Setting {key} must be an array of extra fields."));
    };
    if items.len() > MAX_EXTRA_FIELDS_PER_ENTITY {
        return Err(format!(
            "Setting {key} has {} extra fields, the maximum is {MAX_EXTRA_FIELDS_PER_ENTITY}.",
            items.len()
        ));
    }
    for (index, obj) in items.iter().enumerate() {
        let field = match pydantic::model_validate("ExtraField", obj) {
            Ok(v) => extra_field_json(&pydantic::to_json(&v)),
            Err(es) => {
                return Err(format!(
                    "Extra field at index {index} is not valid: {} errors. {}",
                    es.len(),
                    pydantic::validation_error_str("ExtraField", &es)
                ));
            }
        };
        let theirs = field["entity_type"].as_str().unwrap_or_default();
        if theirs != entity {
            return Err(format!(
                "Extra field at index {index} has entity type {}, expected {}.",
                pydantic::ascii_repr(theirs),
                pydantic::ascii_repr(entity)
            ));
        }
        validate_extra_field(&field)
            .map_err(|e| format!("Extra field at index {index} is not valid: {e}"))?;
    }
    let keys: Vec<&Py> = items.iter().filter_map(|f| f.get("key")).collect();
    if keys.iter().enumerate().any(|(i, k)| keys[..i].contains(k)) {
        return Err(format!(
            "Setting {key} contains duplicate extra field keys."
        ));
    }
    Ok(())
}

/// `add_or_update_extra_field`. Err: a 500; Ok(Err): its `ValueError`, a 400.
async fn add_or_update_extra_field(
    w: &mut Work,
    entity: &str,
    field: Value,
) -> Result<Result<(), String>, String> {
    if let Err(e) = validate_extra_field(&field) {
        return Ok(Err(e));
    }
    let fields = get_extra_fields(w, entity).await?;
    if let Some(existing) = fields.iter().find(|f| f["key"] == field["key"]) {
        if existing["field_type"] != field["field_type"] {
            return Ok(Err("Field type cannot be changed.".to_owned()));
        }
        if field["field_type"] == "choice" {
            if existing["multi_choice"] != field["multi_choice"] {
                return Ok(Err("Multi choice cannot be changed.".to_owned()));
            }
            if let (Some(old), Some(new)) =
                (existing["choices"].as_array(), field["choices"].as_array())
                && !old.iter().all(|c| new.contains(c))
            {
                return Ok(Err("Cannot remove existing choices.".to_owned()));
            }
        }
    }
    let mut fields: Vec<Value> = fields
        .into_iter()
        .filter(|f| f["key"] != field["key"])
        .collect();
    if fields.len() >= MAX_EXTRA_FIELDS_PER_ENTITY {
        return Ok(Err(format!(
            "Cannot add another extra field for {entity}, the maximum is {MAX_EXTRA_FIELDS_PER_ENTITY}."
        )));
    }
    fields.push(field);
    save_extra_fields(w, entity, &fields)
        .await
        .map_err(|e| e.to_string())
}

/// `delete_extra_field`: the field out of the setting, then its values out of every row of the
/// entity (`clear_extra_field`). Ok(false): there is no such field (`ItemNotFoundError`).
async fn delete_extra_field(w: &mut Work, entity: &str, key: &str) -> Result<bool, String> {
    let fields = get_extra_fields(w, entity).await?;
    if !fields.iter().any(|f| f["key"] == key) {
        return Ok(false);
    }
    let fields: Vec<Value> = fields.into_iter().filter(|f| f["key"] != key).collect();
    save_extra_fields(w, entity, &fields)
        .await
        .map_err(|e| e.to_string())??;
    let rows: Vec<(i64, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT id, extra FROM {entity} WHERE space_id = $1"
    )))
    .bind(w.space)
    .fetch_all(w.c())
    .await
    .map_err(|e| e.to_string())?;
    for (id, extra) in rows {
        let mut map = extra_map(Some(extra));
        if map.shift_remove(key).is_some() {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {entity} SET extra = $1 WHERE id = $2"
            )))
            .bind(Value::Object(map).to_string())
            .bind(id)
            .execute(w.c())
            .await
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(true)
}

/// Moving from a Spoolman (`spoolmandb`): one of its fields as its `/field/{entity}` gave it,
/// added as `POST /field/{entity}/{key}` adds it. Ok(false): Spoolman's checks refuse it.
pub(crate) async fn import_extra_field(
    w: &mut Work,
    entity: &str,
    field: &Value,
) -> Result<bool, String> {
    let Some(entity) = self::entity(entity) else {
        return Err(format!("no entity {entity}"));
    };
    let mut field = extra_field_json(field);
    field["entity_type"] = json!(entity);
    add_or_update_extra_field(w, entity, field)
        .await
        .map(|r| r.is_ok())
}

async fn fields(
    access: crate::access::Access,
    State(store): State<Store>,
    Path(e): Path<String>,
) -> Response {
    let Some(e) = entity(&e) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut w = match store.work(access.space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    match get_extra_fields(&mut w, e).await {
        Ok(v) => model(v).into_response(),
        Err(err) => crate::failed(err),
    }
}

/// `POST /field/{entity_type}/{key}`: the body (`ExtraFieldParameters`, checked) with the key and
/// entity type is the `ExtraField`; answered with the entity's fields.
async fn set_field(
    access: crate::access::Access,
    State(store): State<Store>,
    Path((e, key)): Path<(String, String)>,
    pydantic::Body(body): pydantic::Body,
) -> Response {
    let Some(e) = entity(&e) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut field = extra_field_json(&body);
    field["key"] = json!(key);
    field["entity_type"] = json!(e);
    let mut w = match store.work(access.space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    match add_or_update_extra_field(&mut w, e, field).await {
        Ok(Ok(())) => {}
        Ok(Err(why)) => return refused(why),
        Err(err) => return crate::failed(err),
    }
    match get_extra_fields(&mut w, e).await {
        Ok(v) => match w.commit(&store).await {
            Ok(()) => model(v).into_response(),
            Err(err) => crate::failed(err),
        },
        Err(err) => crate::failed(err),
    }
}

async fn remove_field(
    access: crate::access::Access,
    State(store): State<Store>,
    Path((e, key)): Path<(String, String)>,
) -> Response {
    let Some(e) = entity(&e) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut w = match store.work(access.space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    match delete_extra_field(&mut w, e, &key).await {
        Ok(true) => {}
        Ok(false) => {
            return missing(format!(
                "Extra field with key {key} does not exist for entity type {e}"
            ));
        }
        Err(err) => return crate::failed(err),
    }
    match get_extra_fields(&mut w, e).await {
        Ok(v) => match w.commit(&store).await {
            Ok(()) => model(v).into_response(),
            Err(err) => crate::failed(err),
        },
        Err(err) => crate::failed(err),
    }
}

/// `GET /field/{entity_type}/{key}/values`: 404 for a key that is no field, else
/// `find_extra_field_values`, the distinct values decoded (`_JsonScalarText`), empty ones left
/// out, sorted. As on SQLite (`json_extract(value, '$')`), Spoolman's default database and the
/// one its audit runs on: a string unquoted, a list or object as minified JSON text. Not
/// covered: a number or boolean value (SQLite returns a number, which Spoolman's `list[str]`
/// response does not take), and in a list or object the spelling of numbers and `\u` escapes,
/// which SQLite keeps as written and serde writes its own way.
async fn field_values(
    access: crate::access::Access,
    State(store): State<Store>,
    Path((e, key)): Path<(String, String)>,
) -> Response {
    let Some(e) = entity(&e) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let names = match names_for(&store, access.space, e).await {
        Ok(names) => names,
        Err(e) => return crate::failed(e),
    };
    let mut w = match store.work(access.space).await {
        Ok(w) => w,
        Err(e) => return crate::failed(e),
    };
    match get_extra_fields(&mut w, e).await {
        Ok(fields) if fields.iter().any(|f| f["key"] == key) => {}
        Ok(_) => return missing(format!("No extra field with key '{key}'.")),
        Err(err) => return crate::failed(err),
    }
    let rows = match w.all(e, &names).await {
        Ok(rows) => rows,
        Err(err) => return crate::failed(err),
    };
    let mut values = std::collections::BTreeSet::new();
    for row in rows {
        let Some(text) = row["extra"].get(&key).and_then(Value::as_str) else {
            continue;
        };
        let decoded = match python_json::loads_str(text) {
            Ok(python_json::Py::Str(s)) => s,
            Ok(other @ (python_json::Py::List(_) | python_json::Py::Dict(_))) => {
                pydantic::to_json(&other).to_string()
            }
            _ => continue,
        };
        if !decoded.is_empty() {
            values.insert(decoded);
        }
    }
    model(values.into_iter().collect::<Vec<_>>()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures_util::StreamExt;

    /// What Moonraker, Mainsail and the OctoPrint plugin do with a Spoolman: make a vendor, a
    /// filament and a spool, list and filter them, count filament off by length and by
    /// weighing, and hear about it on the spool's WebSocket.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn it_answers_as_spoolman_does(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = crate::auth::signed_in_app(store.clone()).await;
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let base = format!("http://{addr}/spoolman/api/v1");
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
        assert_eq!(filament["color_hex"], "2B2B33");
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
            tokio_tungstenite::connect_async(format!("ws://{addr}/spoolman/api/v1/spool/{id}"))
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
        let event = tokio::time::timeout(std::time::Duration::from_secs(60), ws.next())
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
        let space = store.default_space().await.unwrap().0;
        let item = store
            .items(space)
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
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

#[cfg(test)]
mod audit;
pub mod openprinttag;
pub mod pydantic;
pub mod python_json;
pub mod routing;
