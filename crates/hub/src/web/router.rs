use crate::commands::command;
use crate::link::pair;
use crate::web::machines::{
    add_klipper, add_virtual, answer, bed_size, choose_media, choose_profile, history, laser_power,
    machine_page, machine_stream, machines, move_machine, not_ready, out_of_use, page,
    profile_image, profile_list, remove, rename, set_labels, taken,
};
use crate::web::shell::{logo, logo_large, logo_large_dark};
use crate::{
    STYLE, Store, about, access, apps, auth, codes, escape, farm, files, files_page, now_page,
    permissions_page, printer_files, projects_page, prusa_connect, queue, safety, samples,
    simplyprint, space_page, spoolman, spoolmandb, stock, updates,
};
use axum::{
    Router,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};

pub fn app(store: Store) -> Router {
    let app = Router::new()
        .route("/pair", post(pair))
        .route("/api/machines", get(machines))
        .route("/api/machines/stream", get(machine_stream))
        // The machine list; the home page is Now (now_page).
        .route("/machines", get(page))
        .route("/machines/", get(|| async { Redirect::to("/machines") }))
        .route("/logo.svg", get(logo))
        .route("/logo-large.svg", get(logo_large))
        .route("/logo-large-dark.svg", get(logo_large_dark))
        .route("/machines/{id}", get(machine_page))
        .route("/machines/{id}/profile", post(choose_profile))
        .route("/machines/{id}/media", post(choose_media))
        .route("/machines/{id}/name", post(rename))
        .route("/machines/{id}/labels", post(set_labels))
        .route("/machines/{id}/command", post(command))
        .route("/machines/{id}/remove", post(remove))
        .route("/machines/{id}/move", post(move_machine))
        .route("/machines/{id}/out-of-use", post(out_of_use))
        .route("/machines/{id}/taken", post(taken))
        .route("/machines/{id}/not-ready", post(not_ready))
        .route("/machines/{id}/answer", post(answer))
        .route("/machines/{id}/laser", post(laser_power))
        .route("/machines/{id}/bed", post(bed_size))
        .route("/api/machines/{id}/history", get(history))
        .route("/api/profiles", get(profile_list))
        .route("/profiles/{brand}/{file}", get(profile_image))
        .route("/virtual", post(add_virtual))
        .merge(crate::add_machine_page::routes())
        .merge(prusa_connect::routes())
        .merge(simplyprint::routes())
        .merge(spoolman::routes())
        .merge(spoolmandb::routes())
        .route("/klipper", post(add_klipper))
        .merge(files::routes())
        .merge(updates::routes())
        .merge(about::routes())
        .merge(crate::instance::routes())
        .merge(codes::routes())
        .merge(stock::routes())
        .route("/files", get(files_page::page))
        .route("/prepare", get(crate::pages::prepare_page::page))
        .merge(auth::routes())
        .merge(space_page::routes())
        .merge(permissions_page::routes())
        .merge(safety::routes())
        .merge(crate::pages::times_page::routes())
        .merge(samples::routes())
        .merge(queue::routes())
        .merge(apps::routes())
        .merge(crate::laser_design::routes())
        .merge(printer_files::routes())
        .merge(now_page::routes())
        .merge(farm::routes())
        .merge(projects_page::routes())
        .merge(crate::guides::routes())
        .merge(crate::jobs_page::routes())
        .merge(cameras_routes())
        // After a route is matched: what it needs, against who asks (access::RULES).
        .route_layer(axum::middleware::from_fn_with_state(
            store.clone(),
            access::permit,
        ))
        .fallback(not_found)
        .with_state(store.clone());
    // Spoolman's server (`/spoolman/…`): Starlette's routing of its API, inside its gzip. Around
    // the whole router, not as `Router::layer` (that runs after a route is chosen), so a path
    // parameter it writes back as its number is what the router matches.
    let app = Router::new()
        .fallback_service(tower::Layer::layer(
            &axum::middleware::from_fn(spoolman::routing::routing),
            app,
        ))
        .layer(axum::middleware::from_fn(spoolman::gzip))
        // Before: who asks (a session, an API token), and that they are signed in.
        .layer(axum::middleware::from_fn_with_state(
            store.clone(),
            auth::guard,
        ))
        .layer(axum::middleware::from_fn(prusa_connect::log));
    auth::token_paths(app)
}

/// The network cameras' routes, when the hub is built with them (feature `cameras`).
fn cameras_routes() -> Router<Store> {
    #[cfg(feature = "cameras")]
    return crate::cameras::page::routes();
    #[cfg(not(feature = "cameras"))]
    Router::new()
}

/// Nothing here. A browser asking for a page gets the "spaghetti" page; every other client
/// (agents, Moonraker, Spoolman clients, scripts: anything under `/api/`, `/p/`, `/t/`,
/// `/simplyprint/`, or not asking for HTML) gets the same plain text as always.
async fn not_found(uri: axum::http::Uri, headers: axum::http::HeaderMap) -> Response {
    let path = uri.path();
    let machine_path = ["/api/", "/p/", "/c/", "/t/", "/simplyprint/"]
        .iter()
        .any(|p| path.starts_with(p));
    let wants_html = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|a| a.to_str().ok())
        .is_some_and(|a| a.contains("text/html"));
    if machine_path || !wants_html {
        return (StatusCode::NOT_FOUND, format!("nothing at {uri}")).into_response();
    }
    let page = NOT_FOUND_PAGE
        .replace("{style}", STYLE)
        .replace("{theme}", crate::web::shell::THEME_SCRIPT)
        .replace("{path}", &escape(path));
    (StatusCode::NOT_FOUND, axum::response::Html(page)).into_response()
}

/// The 404 page for people: a print that came loose from the bed. Ours (UI only).
const NOT_FOUND_PAGE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Spaghetti · Maker Connect</title>
<link rel="icon" href="/logo.svg" type="image/svg+xml">
{theme}
<style>
{style}
  .lost { min-height: 100vh; display: grid; place-items: center; padding: 24px max(16px, env(safe-area-inset-left, 0px)); box-sizing: border-box; }
  .lost-card { max-width: 560px; width: 100%; display: grid; gap: 14px; text-align: center; }
  .lost svg { width: 100%; max-width: 420px; height: auto; margin: 0 auto; overflow: visible; }
  .lost h1 { margin: 0; font-size: 26px; letter-spacing: -0.01em; }
  .lost p { margin: 0; color: var(--muted); }
  .lost code { font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; background: var(--surface); border: 1px solid var(--line); border-radius: 0; padding: 2px 6px; color: var(--ink); word-break: break-all; }
  .gcode { text-align: left; margin: 4px auto 0; padding: 12px 14px; max-width: 420px; width: 100%; box-sizing: border-box; background: var(--surface); border: 1px solid var(--line); border-radius: 0; font: 13px/1.6 ui-monospace, SFMono-Regular, Menlo, monospace; color: var(--muted); }
  .gcode b { color: var(--accent-text); font-weight: 600; }
  .gcode i { color: var(--st-bad); font-style: normal; }
  .lost-actions { display: flex; gap: 10px; justify-content: center; flex-wrap: wrap; }
  .nozzle { animation: wander 3.2s ease-in-out infinite; transform-origin: 210px 40px; }
  .strand { stroke-dasharray: 900; stroke-dashoffset: 900; animation: ooze 3.2s ease-in-out infinite; }
  @keyframes wander { 0%, 100% { transform: translateX(-46px); } 50% { transform: translateX(46px); } }
  @keyframes ooze { 0% { stroke-dashoffset: 900; } 60%, 100% { stroke-dashoffset: 0; } }
  @media (prefers-reduced-motion: reduce) { .nozzle, .strand { animation: none; stroke-dashoffset: 0; } }
</style>
</head>
<body>
<main id="main" tabindex="-1" class="bare lost">
  <div class="lost-card">
    <svg viewBox="0 0 420 230" role="img" aria-label="A print nozzle drawing a tangle of filament in mid-air, spelling 404">
      <line x1="20" y1="22" x2="400" y2="22" stroke="var(--line)" stroke-width="6" stroke-linecap="round"/>
      <g class="nozzle">
        <rect x="186" y="6" width="48" height="30" rx="5" fill="var(--surface)" stroke="var(--ink)" stroke-width="2.5"/>
        <rect x="198" y="36" width="24" height="10" fill="var(--st-off)"/>
        <path d="M200 46 L220 46 L213 58 L207 58 Z" fill="var(--st-warn)"/>
      </g>
      <path class="strand" fill="none" stroke="var(--accent)" stroke-width="5" stroke-linecap="round" stroke-linejoin="round"
        d="M210 58 C 205 80, 150 70, 130 95 C 112 118, 96 150, 112 160 L 62 160 L 112 100 L 112 196
           M 178 128 C 150 128, 150 196, 178 196 C 206 196, 206 128, 178 128
           C 170 150, 196 170, 182 184
           M 290 160 L 240 160 L 290 100 L 290 196
           C 300 210, 330 186, 312 172 C 296 160, 330 140, 352 150"/>
      <line x1="40" y1="214" x2="380" y2="214" stroke="var(--line)" stroke-width="4" stroke-linecap="round" stroke-dasharray="2 10"/>
    </svg>
    <h1>This page came loose from the bed</h1>
    <p>The nozzle kept going, but there was nothing here to print on: <code>{path}</code></p>
    <div class="gcode" aria-label="What the printer would say">
      ; layer 404<br>
      <i>// warning: no adhesion at {path}</i><br>
      ; tip: level the bed, check the address<br>
      <b>G28</b> ; home
    </div>
    <div class="lost-actions">
      <span class="fw"><a class="btn primary" href="/">G28 · Home</a></span>
      <span class="fw"><button type="button" class="btn" onclick="history.back()">Undo last move</button></span>
    </div>
  </div>
</main>
</body>
</html>
"##;

/// The answer to a form: none for one saved in the background (`x-requested-with`), else
/// back to the page it belongs to.
pub(crate) fn done(headers: &HeaderMap, to: &str) -> Response {
    if headers.contains_key("x-requested-with") {
        StatusCode::NO_CONTENT.into_response()
    } else {
        Redirect::to(to).into_response()
    }
}

pub(crate) fn failed(error: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::tests::call;

    use axum::body::Body;
    use axum::http::Request;

    /// A person's browser gets the spaghetti page; machines and scripts keep the plain text.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn nothing_here_is_spaghetti_for_people_only(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store).await;
        let page = Request::get("/no-such-page")
            .header("accept", "text/html,application/xhtml+xml")
            .body(Body::empty())
            .unwrap();
        let (status, body) = call(&app, page).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("came loose from the bed"), "{body}");
        assert!(body.contains("/no-such-page"));
        let hostile = Request::get("/%3Cscript%3E")
            .header("accept", "text/html")
            .body(Body::empty())
            .unwrap();
        let (_, body) = call(&app, hostile).await;
        assert!(
            body.contains("<code>/%3Cscript%3E</code>"),
            "the path as asked"
        );
        assert!(!body.contains("<code>/<script>"), "the path is escaped");
        for (path, accept) in [
            ("/api/v1/nope", "text/html"),
            ("/no-such-page", "application/json"),
        ] {
            let request = Request::get(path)
                .header("accept", accept)
                .body(Body::empty())
                .unwrap();
            let (status, body) = call(&app, request).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert!(body.starts_with("nothing at "), "{path}: {body}");
        }
    }
}
