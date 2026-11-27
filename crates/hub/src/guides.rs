//! Guides (wiki: Design: Guides, issue #210): step-by-step instructions per space. The model, its
//! checks and a step's HTML are `maker_connect_guides`; here are storage (`guides/migrations/`), the
//! photos, the pages (list, reader, editor) and their calls. Staff write and publish; everyone in the
//! space reads what is published.

use crate::access::{Access, Role};
use crate::store::{now, random_id};
use crate::{STYLE, Store, escape, failed, header};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{StatusCode, header as h},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use maker_connect_guides::{self as model, Guide};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fmt::Write as _;

/// The largest photo the hub takes (a phone's photo fits).
const MAX_PHOTO: usize = 25 * 1024 * 1024;

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/guides", get(list_page))
        .route("/guides/{id}", get(read_page))
        .route("/guides/{id}/edit", get(edit_page))
        .route("/guides/photos/{name}", get(photo))
        .route("/api/guides", get(list).post(new_guide))
        .route("/api/guides/{id}", get(one).post(save))
        .route("/api/guides/{id}/publish", post(publish))
        .route("/api/guides/{id}/remove", post(remove))
        .route(
            "/api/guides/photos",
            post(upload).layer(DefaultBodyLimit::max(MAX_PHOTO)),
        )
}

fn staff(access: &Access) -> bool {
    access.role >= Role::Admin
}

/// A guide's row: the draft, and what readers see once published.
#[derive(Debug, Clone, Serialize)]
struct Row {
    id: i64,
    draft: Guide,
    published: Option<Guide>,
    author: String,
    last_editor: String,
    updated_at: i64,
    published_at: Option<i64>,
}

impl Row {
    /// Whether the draft differs from what is published.
    fn changed(&self) -> bool {
        self.published.as_ref() != Some(&self.draft)
    }
}

type RawRow = (
    i64,
    String,
    Option<String>,
    String,
    String,
    i64,
    Option<i64>,
);

fn row(raw: RawRow) -> Option<Row> {
    let (id, draft, published, author, last_editor, updated_at, published_at) = raw;
    Some(Row {
        id,
        draft: serde_json::from_str(&draft).ok()?,
        published: match published {
            Some(p) => Some(serde_json::from_str(&p).ok()?),
            None => None,
        },
        author,
        last_editor,
        updated_at,
        published_at,
    })
}

impl Store {
    async fn guides(&self, space: i64) -> sqlx::Result<Vec<Row>> {
        let raw: Vec<RawRow> = crate::stock::in_space!(self, space, |w| async {
            sqlx::query_as::<_, RawRow>(
                "SELECT id, draft::text, published::text, author, last_editor, unix_seconds(updated_at), unix_seconds(published_at) FROM guides ORDER BY lower(draft->>'title'), id",
            )
                .fetch_all(w.c())
                .await
        })
        .await?;
        Ok(raw.into_iter().filter_map(row).collect())
    }

    async fn guide(&self, space: i64, id: i64) -> sqlx::Result<Option<Row>> {
        let raw: Option<RawRow> = crate::stock::in_space!(self, space, |w| async {
            sqlx::query_as::<_, RawRow>(
                "SELECT id, draft::text, published::text, author, last_editor, unix_seconds(updated_at), unix_seconds(published_at) FROM guides WHERE id = $1",
            )
                .bind(id)
                .fetch_optional(w.c())
                .await
        })
        .await?;
        Ok(raw.and_then(row))
    }

    /// The folder of a space's guide photos.
    fn guide_photos(&self, space: i64) -> std::path::PathBuf {
        self.files_dir().join("guides").join(space.to_string())
    }
}

fn bad_request(why: impl Into<String>) -> Response {
    (StatusCode::BAD_REQUEST, why.into()).into_response()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "no such guide").into_response()
}

fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, "only staff write guides").into_response()
}

/// A guide as the list shows it.
fn summary(row: &Row, guide: &Guide) -> serde_json::Value {
    json!({
        "id": row.id,
        "title": guide.title,
        "difficulty": guide.difficulty,
        "difficulty_name": model::difficulty_name(guide.difficulty),
        "steps": guide.steps.len(),
        "cover": guide.cover_photo(),
        "published": row.published.is_some(),
        "changed": row.changed(),
        "last_editor": row.last_editor,
        "updated_at": row.updated_at,
    })
}

async fn list(access: Access, State(store): State<Store>) -> Response {
    match store.guides(access.space).await {
        Ok(rows) => {
            let staff = staff(&access);
            let out: Vec<_> = rows
                .iter()
                .filter_map(|r| {
                    if staff {
                        Some(summary(r, &r.draft))
                    } else {
                        r.published.as_ref().map(|p| summary(r, p))
                    }
                })
                .collect();
            Json(json!({ "staff": staff, "guides": out })).into_response()
        }
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct NewGuide {
    title: String,
}

async fn new_guide(
    access: Access,
    State(store): State<Store>,
    Json(body): Json<NewGuide>,
) -> Response {
    if !staff(&access) {
        return forbidden();
    }
    let guide = match model::check(Guide {
        title: body.title,
        difficulty: 1,
        cover: None,
        steps: vec![model::Step {
            title: "Introduction".into(),
            lines: Vec::new(),
            photos: Vec::new(),
        }],
    }) {
        Ok(guide) => guide,
        Err(why) => return bad_request(why),
    };
    let draft = serde_json::to_string(&guide).expect("a guide is JSON");
    let made = crate::stock::in_space!(&store, access.space, |w| async {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO guides (space_id, draft, author, last_editor, created_at, updated_at)
             VALUES ($1, $2::jsonb, $3, $3, at_unix($4), at_unix($4)) RETURNING id",
        )
        .bind(w.space)
        .bind(&draft)
        .bind(&access.actor)
        .bind(now())
        .fetch_one(w.c())
        .await
    })
    .await;
    match made {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Err(error) => failed(error),
    }
}

async fn one(access: Access, State(store): State<Store>, Path(id): Path<i64>) -> Response {
    match store.guide(access.space, id).await {
        Ok(Some(row)) if staff(&access) => Json(row).into_response(),
        Ok(Some(row)) => match row.published {
            Some(published) => {
                Json(json!({ "id": row.id, "published": published })).into_response()
            }
            None => not_found(),
        },
        Ok(None) => not_found(),
        Err(error) => failed(error),
    }
}

async fn save(
    access: Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
    Json(guide): Json<Guide>,
) -> Response {
    if !staff(&access) {
        return forbidden();
    }
    let guide = match model::check(guide) {
        Ok(guide) => guide,
        Err(why) => return bad_request(why),
    };
    let draft = serde_json::to_string(&guide).expect("a guide is JSON");
    let saved = crate::stock::in_space!(&store, access.space, |w| async {
        sqlx::query(
            "UPDATE guides SET draft = $2::jsonb, last_editor = $3, updated_at = at_unix($4) WHERE id = $1",
        )
        .bind(id)
        .bind(&draft)
        .bind(&access.actor)
        .bind(now())
        .execute(w.c())
        .await
    })
    .await;
    match saved {
        Ok(done) if done.rows_affected() == 0 => not_found(),
        Ok(_) => Json(json!({ "saved": true, "guide": guide })).into_response(),
        Err(error) => failed(error),
    }
}

async fn publish(access: Access, State(store): State<Store>, Path(id): Path<i64>) -> Response {
    if !staff(&access) {
        return forbidden();
    }
    let done = crate::stock::in_space!(&store, access.space, |w| async {
        sqlx::query("UPDATE guides SET published = draft, published_at = at_unix($2) WHERE id = $1")
            .bind(id)
            .bind(now())
            .execute(w.c())
            .await
    })
    .await;
    match done {
        Ok(done) if done.rows_affected() == 0 => not_found(),
        Ok(_) => Json(json!({ "published": true })).into_response(),
        Err(error) => failed(error),
    }
}

async fn remove(access: Access, State(store): State<Store>, Path(id): Path<i64>) -> Response {
    if !staff(&access) {
        return forbidden();
    }
    let done = crate::stock::in_space!(&store, access.space, |w| async {
        sqlx::query("DELETE FROM guides WHERE id = $1")
            .bind(id)
            .execute(w.c())
            .await
    })
    .await;
    match done {
        Ok(done) if done.rows_affected() == 0 => not_found(),
        Ok(_) => Json(json!({ "removed": true })).into_response(),
        Err(error) => failed(error),
    }
}

/// The photo's type by its first bytes: JPEG, PNG or WebP, nothing else.
fn photo_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

async fn upload(access: Access, State(store): State<Store>, body: Bytes) -> Response {
    if !staff(&access) {
        return forbidden();
    }
    let Some(ext) = photo_type(&body) else {
        return bad_request("a photo: JPEG, PNG or WebP");
    };
    let name = format!("{}.{ext}", random_id());
    let dir = store.guide_photos(access.space);
    let written = async {
        tokio::fs::create_dir_all(&dir).await?;
        let partial = dir.join(format!("{name}.part"));
        tokio::fs::write(&partial, &body).await?;
        tokio::fs::rename(&partial, dir.join(&name)).await
    }
    .await;
    match written {
        Ok(()) => (StatusCode::CREATED, Json(json!({ "name": name }))).into_response(),
        Err(error) => failed(error),
    }
}

async fn photo(access: Access, State(store): State<Store>, Path(name): Path<String>) -> Response {
    if !model::photo_name_ok(&name) {
        return (StatusCode::NOT_FOUND, "no such photo").into_response();
    }
    let kind = match name.rsplit('.').next() {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    };
    match tokio::fs::read(store.guide_photos(access.space).join(&name)).await {
        Ok(bytes) => (
            [
                (h::CONTENT_TYPE, kind),
                (h::CACHE_CONTROL, "private, max-age=31536000, immutable"),
            ],
            Body::from(bytes),
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "no such photo").into_response(),
    }
}

fn page(title: &str, space: &str, root: &str, style: &str, body: &str, script: &str) -> Response {
    Html(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} · Maker Connect</title>
<link rel="icon" href="{root}logo.svg" type="image/svg+xml">
<style>
{STYLE}{LINES}{GUIDES_STYLE}{style}
</style>
</head>
<body>
{header}
<main id="main" tabindex="-1" class="wide">
{body}
</main>
<script>
{script}
</script>
</body>
</html>"##,
        title = escape(title),
        LINES = model::STYLE,
        header = header(space, root, "guides"),
    ))
    .into_response()
}

async fn list_page(access: Access, State(store): State<Store>) -> Response {
    let space = match store.space_name(access.space).await {
        Ok(name) => name,
        Err(error) => return failed(error),
    };
    let new = if staff(&access) {
        r#"<form class="g-new" data-new><input name="title" required maxlength="120" placeholder="Changing the tape on the DYMO" aria-label="Title of a new guide"><button class="btn primary">New guide</button></form>"#
    } else {
        ""
    };
    let body = format!(
        r#"<div class="page-head"><div><h2>Guides</h2><p class="subtitle">Step-by-step instructions for the machines in this space.</p></div>{new}</div>
<div class="g-list" data-list><p class="empty">Loading…</p></div>"#
    );
    page("Guides", &space, "", "", &body, LIST_SCRIPT)
}

#[derive(Deserialize)]
struct ReadQuery {
    #[serde(default)]
    draft: bool,
}

/// The reader: the front page and every step, one shown at a time by its script.
async fn read_page(
    access: Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
    Query(query): Query<ReadQuery>,
) -> Response {
    let space = match store.space_name(access.space).await {
        Ok(name) => name,
        Err(error) => return failed(error),
    };
    let row = match store.guide(access.space, id).await {
        Ok(Some(row)) => row,
        Ok(None) => return not_found(),
        Err(error) => return failed(error),
    };
    let staff = staff(&access);
    let (guide, draft) = match (
        &row.published,
        staff && (query.draft || row.published.is_none()),
    ) {
        (_, true) => (&row.draft, true),
        (Some(published), false) => (published, false),
        (None, false) => return not_found(),
    };
    let url = |p: &str| format!("photos/{p}");
    let total = guide.steps.len();
    let mut out = String::new();
    if draft {
        out.push_str(
            r#"<p class="g-banner">Draft: readers see this only once it is published.</p>"#,
        );
    }
    let cover = guide
        .cover_photo()
        .map(|p| format!(r#"<img src="{}" alt="">"#, escape(&url(p))))
        .unwrap_or_default();
    let mut bar = String::new();
    for i in 1..=5u8 {
        let _ = write!(
            bar,
            "<i{}></i>",
            if i <= guide.difficulty {
                " class=\"on\""
            } else {
                ""
            }
        );
    }
    let edit = if staff {
        format!(r#"<a class="btn" href="{id}/edit">Edit</a>"#)
    } else {
        String::new()
    };
    let _ = write!(
        out,
        r#"<section class="g-front" data-step="0">
<div class="g-cover">{cover}</div>
<div class="g-about"><h2>{title}</h2>
<dl class="g-facts"><dt>Difficulty</dt><dd><span class="g-diff">{bar}</span> {diff}</dd><dt>Steps</dt><dd>{total}</dd><dt>Last edited by</dt><dd>{editor}</dd></dl>
<div class="g-go">{begin}{edit}</div></div>
</section>"#,
        title = escape(&guide.title),
        diff = model::difficulty_name(guide.difficulty),
        editor = escape(&row.last_editor),
        begin = if total > 0 {
            r#"<button type="button" class="btn primary" data-go="1">Begin</button>"#
        } else {
            r#"<p class="muted">No steps yet.</p>"#
        },
    );
    let mut contents = String::new();
    for (i, step) in guide.steps.iter().enumerate() {
        let n = i + 1;
        let _ = write!(
            contents,
            r#"<li><button type="button" data-go="{n}">{n}. {}</button></li>"#,
            escape(&step.title)
        );
        let _ = write!(
            out,
            r#"<section class="g-step" data-step="{n}" hidden>
<div class="g-bar"><h2>{n}. {title}</h2><div class="g-tools"><button type="button" class="btn ghost small" data-contents>Contents</button><button type="button" class="btn ghost small" data-full title="Full screen (F)">Full screen</button></div></div>
<div class="g-body"><div class="g-text">{lines}</div>{photos}</div>
<nav class="g-nav" aria-label="Steps"><span class="g-count">Step {n} of {total}</span><button type="button" class="btn small" data-go="0" title="Front page" aria-label="Front page">⏮</button><button type="button" class="btn small" data-go="{prev}" aria-label="Previous step">←</button>{next}<button type="button" class="btn small" data-go="{total}" title="Last step" aria-label="Last step">⏭</button></nav>
</section>"#,
            title = escape(&step.title),
            lines = model::lines_html(step),
            photos = model::photos_html(step, url),
            prev = n - 1,
            next = if n < total {
                format!(
                    r#"<button type="button" class="btn primary" data-go="{}">Next →</button>"#,
                    n + 1
                )
            } else {
                r#"<button type="button" class="btn primary" data-go="0">Done</button>"#.to_owned()
            },
        );
    }
    let _ = write!(
        out,
        r#"<dialog class="g-contents" data-contents-dialog><div class="dialog-head"><h2>Contents</h2><button type="button" class="close" data-close aria-label="Close">✕</button></div><ol>{contents}</ol></dialog>"#
    );
    page(
        &guide.title,
        &space,
        "../",
        "",
        &format!(r#"<div class="g-read" data-read>{out}</div>"#),
        READ_SCRIPT,
    )
}

async fn edit_page(access: Access, State(store): State<Store>, Path(id): Path<i64>) -> Response {
    if !staff(&access) {
        return forbidden();
    }
    let space = match store.space_name(access.space).await {
        Ok(name) => name,
        Err(error) => return failed(error),
    };
    match store.guide(access.space, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found(),
        Err(error) => return failed(error),
    }
    let mut styles = String::new();
    for style in model::Style::ALL {
        let _ = write!(
            styles,
            r#"<option value="{0}">{1}</option>"#,
            style.name(),
            style.name().replace('_', " ")
        );
    }
    let mut levels = String::new();
    for (i, name) in model::DIFFICULTY.iter().enumerate() {
        let _ = write!(levels, r#"<option value="{}">{name}</option>"#, i + 1);
    }
    let body = format!(
        r#"<div class="e-head">
  <input class="e-title" data-title maxlength="120" aria-label="Title of the guide">
  <label>Difficulty <select data-difficulty>{levels}</select></label>
  <span class="e-state" data-state></span>
  <a class="btn" href="../{id}?draft=1">Preview</a>
  <button type="button" class="btn primary" data-publish>Publish</button>
  <button type="button" class="btn ghost" data-remove>Remove guide</button>
</div>
<div class="e-cols">
  <aside class="e-steps"><ol data-steps></ol><button type="button" class="btn small" data-add-step>Add step</button></aside>
  <section class="e-step card" data-step>
    <label class="e-step-title">Step title <input data-step-title maxlength="120"></label>
    <div class="e-lines" data-lines></div>
    <button type="button" class="btn small" data-add-line>Add line</button>
    <p class="muted e-help">One action per line. Pick the colour of the marking in the photo; caution for a warning, note for extra information. Bold: Ctrl+B, italic: Ctrl+I.</p>
    <h3>Photos</h3>
    <p class="muted e-help">The first is shown large. Mark them before uploading (arrows, circles in the line's colour).</p>
    <div class="e-photos" data-photos></div>
    <label class="btn small"><input type="file" accept="image/jpeg,image/png,image/webp" multiple hidden data-upload>Add photos</label>
    <div class="e-step-tools"><button type="button" class="btn small" data-step-up>Move step up</button><button type="button" class="btn small" data-step-down>Move step down</button><button type="button" class="btn small ghost" data-step-remove>Remove step</button></div>
  </section>
</div>
<template data-style-options>{styles}</template>"#
    );
    let script = format!("const GUIDE_ID = {id};\n{EDIT_SCRIPT}");
    page("Edit guide", &space, "../../", EDIT_STYLE, &body, &script)
}

const GUIDES_STYLE: &str = r#"
.page-head { display: flex; gap: 16px; align-items: flex-start; justify-content: space-between; flex-wrap: wrap; margin-bottom: 16px; }
.page-head h2 { margin: 0 0 4px; }
.subtitle, .muted { color: var(--muted); }
.g-new { display: flex; gap: 8px; flex-wrap: wrap; }
.g-new input { min-width: 260px; }
.g-list { display: grid; grid-template-columns: repeat(auto-fill, minmax(240px, 1fr)); gap: 16px; }
.g-list > .card { margin: 0; }
.g-card { display: block; text-decoration: none; color: inherit; padding: 0; overflow: hidden; }
.g-card .g-pic { aspect-ratio: 4 / 3; background: var(--surface-2); display: grid; place-items: center; }
.g-card .g-pic img { width: 100%; height: 100%; object-fit: cover; }
.g-card .g-info { padding: 12px 16px; }
.g-card h3 { margin: 0 0 6px; font-size: 16px; }
.g-tag { font-size: 12px; color: var(--muted); margin-right: 8px; }
.g-tag.draft { color: var(--st-warn); font-weight: 700; }
.g-banner { padding: 8px 12px; background: var(--surface-2); border-left: 4px solid var(--st-warn); margin: 0 0 16px; }
.g-front { display: grid; grid-template-columns: minmax(0, 320px) minmax(0, 1fr); gap: 24px; align-items: start; }
.g-cover img { width: 100%; display: block; }
.g-about h2 { margin: 0 0 12px; }
.g-facts { display: grid; grid-template-columns: max-content 1fr; gap: 8px 20px; margin: 0 0 20px; }
.g-facts dt { color: var(--muted); }
.g-facts dd { margin: 0; }
.g-diff { display: inline-flex; gap: 2px; vertical-align: middle; margin-right: 6px; }
.g-diff i { width: 14px; height: 8px; background: var(--line); }
.g-diff i.on { background: var(--accent); }
.g-go { display: flex; gap: 8px; flex-wrap: wrap; }
.g-bar { display: flex; align-items: center; gap: 12px; justify-content: space-between; background: var(--surface-2); padding: 10px 16px; margin-bottom: 16px; }
.g-bar h2 { margin: 0; font-size: 18px; }
.g-tools { display: flex; gap: 4px; }
.g-body { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 2fr); gap: 24px; align-items: start; }
.g-nav { display: flex; gap: 8px; justify-content: flex-end; align-items: center; margin-top: 20px; }
.g-count { margin-right: auto; color: var(--muted); }
.g-contents ol { padding-left: 0; list-style: none; margin: 0; }
.g-contents button { background: none; border: 0; padding: 8px 0; color: var(--ink); font: inherit; cursor: pointer; text-align: left; }
.g-read:fullscreen { background: var(--bg); padding: 24px; overflow: auto; }
@media (max-width: 760px) {
  .g-front, .g-body { grid-template-columns: 1fr; }
  .g-body .gl-photos { order: -1; }
  .g-nav { position: sticky; bottom: 0; background: var(--bg); padding: 8px 0; }
  .g-tools [data-full] { display: none; }
}
"#;

const EDIT_STYLE: &str = r#"
.e-head { display: flex; gap: 10px; align-items: center; flex-wrap: wrap; margin-bottom: 16px; }
.e-title { flex: 1 1 280px; font-size: 20px; font-weight: 700; }
.e-state { color: var(--muted); font-size: 13px; min-width: 70px; }
.e-cols { display: grid; grid-template-columns: 260px minmax(0, 1fr); gap: 20px; align-items: start; }
.e-steps ol { list-style: none; margin: 0 0 10px; padding: 0; }
.e-steps li button { width: 100%; text-align: left; background: none; border: 0; border-left: 4px solid transparent; padding: 8px 10px; font: inherit; color: var(--ink); cursor: pointer; }
.e-steps li button.on { border-left-color: var(--accent); background: color-mix(in srgb, var(--accent) 18%, transparent); }
.e-step-title { display: flex; flex-direction: column; gap: 4px; font-weight: 600; margin-bottom: 14px; }
.e-line { display: flex; gap: 8px; align-items: flex-start; margin-bottom: 8px; }
.e-line.lvl1 { margin-left: 30px; }
.e-line select { flex: none; width: 110px; }
.e-line .gl-mark { margin-top: 12px; }
.e-text { flex: 1; min-height: 38px; padding: 8px 10px; border: 1px solid var(--line-strong); background: var(--surface); line-height: 1.5; }
.e-text:focus { outline: 2px solid var(--focus); }
.e-line .btn { min-height: 38px; padding: 0 8px; }
.e-help { font-size: 13px; margin: 6px 0 12px; }
.e-photos { display: flex; gap: 10px; flex-wrap: wrap; margin-bottom: 10px; }
.e-photo { width: 150px; }
.e-photo img { width: 150px; aspect-ratio: 4 / 3; object-fit: contain; background: var(--surface-2); display: block; border: 2px solid var(--line); }
.e-photo:first-child img { border-color: var(--accent); }
.e-photo div { display: flex; gap: 4px; margin-top: 4px; }
.e-photo button { flex: 1; min-height: 32px; padding: 0; }
.e-step-tools { display: flex; gap: 8px; flex-wrap: wrap; margin-top: 20px; border-top: 1px solid var(--line); padding-top: 14px; }
@media (max-width: 760px) { .e-cols { grid-template-columns: 1fr; } }
"#;

const LIST_SCRIPT: &str = r#"
(() => {
  const list = document.querySelector('[data-list]');
  const esc = s => String(s).replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
  async function load() {
    const r = await fetch('api/guides');
    if (!r.ok) { list.innerHTML = '<p class="empty">Could not load the guides.</p>'; return; }
    const { staff, guides } = await r.json();
    if (!guides.length) { list.innerHTML = '<p class="empty">' + (staff ? 'No guides yet. Start one with New guide.' : 'No guides yet.') + '</p>'; return; }
    list.innerHTML = guides.map(g => {
      const state = !g.published ? '<span class="g-tag draft">Draft</span>' : (staff && g.changed ? '<span class="g-tag draft">Unpublished changes</span>' : '');
      const pic = g.cover ? '<img src="guides/photos/' + esc(g.cover) + '" alt="">' : '';
      return '<a class="card g-card" href="guides/' + g.id + (staff && !g.published ? '/edit' : '') + '"><div class="g-pic">' + pic + '</div><div class="g-info"><h3>' + esc(g.title) + '</h3>' + state + '<span class="g-tag">' + esc(g.difficulty_name) + '</span><span class="g-tag">' + g.steps + (g.steps === 1 ? ' step' : ' steps') + '</span></div></a>';
    }).join('');
  }
  const form = document.querySelector('[data-new]');
  if (form) form.addEventListener('submit', async e => {
    e.preventDefault();
    const r = await fetch('api/guides', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ title: form.title.value }) });
    if (r.ok) { const { id } = await r.json(); location.href = 'guides/' + id + '/edit'; }
    else alert(await r.text());
  });
  load();
})();
"#;

const READ_SCRIPT: &str = r#"
(() => {
  const read = document.querySelector('[data-read]');
  const sections = [...read.querySelectorAll('[data-step]')];
  const last = sections.length - 1;
  const dialog = read.querySelector('[data-contents-dialog]');
  function show(n, push) {
    n = Math.max(0, Math.min(last, n | 0));
    sections.forEach((s, i) => { s.hidden = i !== n; });
    if (push) history.replaceState(null, '', n ? '#step-' + n : location.pathname + location.search);
    if (dialog.open) dialog.close();
    window.scrollTo(0, 0);
  }
  read.addEventListener('click', e => {
    const go = e.target.closest('[data-go]');
    if (go) { show(+go.dataset.go, true); return; }
    const thumb = e.target.closest('[data-photo]');
    if (thumb) {
      const box = thumb.closest('.gl-photos');
      box.querySelector('[data-big]').src = thumb.dataset.photo;
      box.querySelectorAll('.gl-thumb').forEach(t => t.classList.toggle('on', t === thumb));
      return;
    }
    if (e.target.closest('[data-contents]')) { dialog.showModal(); return; }
    if (e.target.closest('[data-close]')) { dialog.close(); return; }
    if (e.target.closest('[data-full]')) { full(); }
  });
  function full() {
    if (document.fullscreenElement) document.exitFullscreen();
    else if (read.requestFullscreen) read.requestFullscreen();
  }
  // The button says what it does now: in full screen it leaves it (Esc does too).
  document.addEventListener('fullscreenchange', () => {
    const on = !!document.fullscreenElement;
    for (const b of document.querySelectorAll('[data-full]')) {
      b.textContent = on ? 'Exit full screen' : 'Full screen';
      b.title = on ? 'Exit full screen (F, Esc)' : 'Full screen (F)';
    }
  });
  function current() { return sections.findIndex(s => !s.hidden); }
  document.addEventListener('keydown', e => {
    if (e.target.closest('input, textarea, select, [contenteditable]')) return;
    if (e.key === 'ArrowRight') show(current() + 1, true);
    else if (e.key === 'ArrowLeft') show(current() - 1, true);
    else if (e.key === 'f' || e.key === 'F') full();
  });
  const m = /^#step-(\d+)$/.exec(location.hash);
  show(m ? +m[1] : 0, false);
})();
"#;

const EDIT_SCRIPT: &str = r#"
(() => {
  const $ = s => document.querySelector(s);
  const esc = s => String(s).replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
  const styleOptions = $('[data-style-options]').innerHTML;
  const state = $('[data-state]');
  let guide = null, at = 0, timer = null, saving = Promise.resolve(), published = false;
  const api = 'api/guides/' + GUIDE_ID;
  const base = '../../';

  async function load() {
    const r = await fetch(base + api);
    if (!r.ok) { state.textContent = 'Could not load'; return; }
    const row = await r.json();
    guide = row.draft;
    published = !!row.published;
    $('[data-title]').value = guide.title;
    $('[data-difficulty]').value = guide.difficulty;
    showState(row.published && JSON.stringify(row.published) === JSON.stringify(row.draft) ? 'Published' : (row.published ? 'Unpublished changes' : 'Draft'));
    drawSteps(); drawStep();
  }
  function showState(text) { state.textContent = text; }
  function changed() {
    showState('Saving…');
    clearTimeout(timer);
    timer = setTimeout(save, 800);
  }
  function save() {
    clearTimeout(timer);
    saving = saving.then(async () => {
      const r = await fetch(base + api, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(guide) });
      if (r.ok) showState(published ? 'Saved · unpublished changes' : 'Saved · draft');
      else showState('Not saved: ' + await r.text());
    });
    return saving;
  }
  function step() { return guide.steps[at]; }
  function drawSteps() {
    $('[data-steps]').innerHTML = guide.steps.map((s, i) => '<li><button type="button" data-pick="' + i + '"' + (i === at ? ' class="on"' : '') + '>' + (i + 1) + '. ' + esc(s.title || 'Untitled') + '</button></li>').join('');
  }
  function lineRow(line, i) {
    return '<div class="e-line' + (line.level ? ' lvl1' : '') + '" data-line="' + i + '">'
      + '<span class="gl-line gl-' + line.style + '"><span class="gl-mark"></span></span>'
      + '<select data-style aria-label="Style">' + styleOptions + '</select>'
      + '<div class="e-text" contenteditable="true" data-text aria-label="Line ' + (i + 1) + '">' + line.text + '</div>'
      + '<button type="button" class="btn small" data-indent title="Sub-line under the line above" aria-label="Indent">' + (line.level ? '⇤' : '⇥') + '</button>'
      + '<button type="button" class="btn small" data-link title="Link" aria-label="Link">🔗</button>'
      + '<button type="button" class="btn small" data-line-up aria-label="Move up">↑</button>'
      + '<button type="button" class="btn small" data-line-down aria-label="Move down">↓</button>'
      + '<button type="button" class="btn small ghost" data-line-remove aria-label="Remove line">✕</button></div>';
  }
  function drawStep() {
    const s = step();
    const box = $('[data-step]');
    box.hidden = !s;
    if (!s) return;
    $('[data-step-title]').value = s.title;
    const lines = $('[data-lines]');
    lines.innerHTML = s.lines.map(lineRow).join('');
    lines.querySelectorAll('[data-line]').forEach((row, i) => { row.querySelector('select').value = s.lines[i].style; });
    $('[data-photos]').innerHTML = s.photos.map((p, i) => '<div class="e-photo" data-photo="' + i + '"><img src="' + base + 'guides/photos/' + esc(p) + '" alt="Photo ' + (i + 1) + '"><div><button type="button" class="btn small" data-photo-left aria-label="Earlier">←</button><button type="button" class="btn small" data-photo-right aria-label="Later">→</button><button type="button" class="btn small ghost" data-photo-remove aria-label="Remove photo">✕</button></div></div>').join('');
  }
  function move(list, i, d) {
    const j = i + d;
    if (j < 0 || j >= list.length) return false;
    [list[i], list[j]] = [list[j], list[i]];
    return true;
  }
  function addLine(after, style) {
    const s = step();
    const prev = s.lines[after];
    s.lines.splice(after + 1, 0, { text: '', style: style || (prev ? prev.style : 'black'), level: prev ? prev.level : 0 });
    drawStep(); changed();
    const t = document.querySelectorAll('[data-text]')[after + 1];
    if (t) t.focus();
  }

  $('[data-title]').addEventListener('input', e => { guide.title = e.target.value; changed(); });
  $('[data-difficulty]').addEventListener('change', e => { guide.difficulty = +e.target.value; changed(); });
  $('[data-step-title]').addEventListener('input', e => { step().title = e.target.value; drawSteps(); changed(); });
  $('[data-steps]').addEventListener('click', e => { const b = e.target.closest('[data-pick]'); if (b) { at = +b.dataset.pick; drawSteps(); drawStep(); } });
  $('[data-add-step]').addEventListener('click', () => { guide.steps.splice(at + 1, 0, { title: 'New step', lines: [{ text: '', style: 'black', level: 0 }], photos: [] }); at += guide.steps.length > 1 ? 1 : 0; drawSteps(); drawStep(); changed(); $('[data-step-title]').select(); });
  $('[data-step-up]').addEventListener('click', () => { if (move(guide.steps, at, -1)) { at--; drawSteps(); changed(); } });
  $('[data-step-down]').addEventListener('click', () => { if (move(guide.steps, at, 1)) { at++; drawSteps(); changed(); } });
  $('[data-step-remove]').addEventListener('click', () => { if (!confirm('Remove this step?')) return; guide.steps.splice(at, 1); at = Math.max(0, at - 1); drawSteps(); drawStep(); changed(); });
  $('[data-add-line]').addEventListener('click', () => addLine(step().lines.length - 1));

  const lines = $('[data-lines]');
  lines.addEventListener('input', e => {
    const row = e.target.closest('[data-line]'); if (!row) return;
    const line = step().lines[+row.dataset.line];
    if (e.target.matches('[data-text]')) line.text = e.target.innerHTML;
    changed();
  });
  lines.addEventListener('change', e => {
    const row = e.target.closest('[data-line]'); if (!row || !e.target.matches('[data-style]')) return;
    step().lines[+row.dataset.line].style = e.target.value;
    row.querySelector('.gl-line').className = 'gl-line gl-' + e.target.value;
    changed();
  });
  lines.addEventListener('keydown', e => {
    if (e.key !== 'Enter' || e.shiftKey || !e.target.matches('[data-text]')) return;
    e.preventDefault();
    addLine(+e.target.closest('[data-line]').dataset.line);
  });
  lines.addEventListener('click', e => {
    const row = e.target.closest('[data-line]'); if (!row) return;
    const i = +row.dataset.line, s = step(), line = s.lines[i];
    if (e.target.closest('[data-indent]')) { line.level = line.level ? 0 : 1; }
    else if (e.target.closest('[data-line-up]')) { if (!move(s.lines, i, -1)) return; }
    else if (e.target.closest('[data-line-down]')) { if (!move(s.lines, i, 1)) return; }
    else if (e.target.closest('[data-line-remove]')) { s.lines.splice(i, 1); }
    else if (e.target.closest('[data-link]')) {
      const url = prompt('Link to (https://…)');
      if (!url) return;
      const text = row.querySelector('[data-text]');
      text.focus();
      document.execCommand('createLink', false, url);
      line.text = text.innerHTML;
    } else return;
    drawStep(); changed();
  });

  $('[data-photos]').addEventListener('click', e => {
    const box = e.target.closest('[data-photo]'); if (!box) return;
    const i = +box.dataset.photo, photos = step().photos;
    if (e.target.closest('[data-photo-left]')) { if (!move(photos, i, -1)) return; }
    else if (e.target.closest('[data-photo-right]')) { if (!move(photos, i, 1)) return; }
    else if (e.target.closest('[data-photo-remove]')) { photos.splice(i, 1); }
    else return;
    drawStep(); changed();
  });
  $('[data-upload]').addEventListener('change', async e => {
    const files = [...e.target.files];
    e.target.value = '';
    for (const file of files) {
      showState('Uploading ' + file.name + '…');
      const r = await fetch(base + 'api/guides/photos', { method: 'POST', body: file });
      if (!r.ok) { showState('Not uploaded: ' + await r.text()); return; }
      step().photos.push((await r.json()).name);
      drawStep();
    }
    changed();
  });
  $('[data-publish]').addEventListener('click', async () => {
    await save();
    const r = await fetch(base + api + '/publish', { method: 'POST' });
    if (r.ok) { published = true; showState('Published'); } else showState('Not published: ' + await r.text());
  });
  $('[data-remove]').addEventListener('click', async () => {
    if (!confirm('Remove this guide? This cannot be undone.')) return;
    const r = await fetch(base + api + '/remove', { method: 'POST' });
    if (r.ok) location.href = base + 'guides'; else alert(await r.text());
  });
  window.addEventListener('beforeunload', e => { if (state.textContent === 'Saving…') e.preventDefault(); });
  load();
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photo_types_by_their_bytes() {
        assert_eq!(photo_type(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(photo_type(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(photo_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(photo_type(b"<svg"), None);
    }
}
