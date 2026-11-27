//! HS13: the LaserFoundry web fork, served by the hub (docs/archive/briefs/hs13-laserfoundry-web.md).
//!
//! The fork (`apps/laserfoundry-web` of our LaserFoundry fork, built without Web Serial by
//! `scripts/laserfoundry-web.sh`) is packed gzip into the binary (feature `laser-design`) and served from the hub's own
//! origin under `/laser-design/`, so its page talks to the hub with the person's session, no token. Ours (UI only):
//!
//! - who may open it: a member of the space (not a guest) who may use at least one laser of it;
//! - `Content-Encoding: gzip` on every file, the right content types, `Cache-Control: no-cache`, and
//!   `Permissions-Policy: serial=(), usb=(), hid=()` (the page must not reach a machine itself: only the agent does);
//! - `GET /api/laser-design/lasers`: the lasers the person may use, each as the apps API's `machines` gives it (Apps API v1
//!   ignores cookies on purpose, so the page uses this read-only route of the hub's own).
//!
//! What the page sends goes through the hub's existing routes (`POST /api/files`, `POST /files/{id}/print`): a laser's job is
//! only LOADED (HS7b); nothing here starts a burn.

use crate::access::{Access, Level, Role};
use crate::{Store, failed};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use maker_connect_protocol::MachineKind;

/// What the page must not do itself (ours).
pub const PERMISSIONS_POLICY: &str = "serial=(), usb=(), hid=()";

/// One packed file: its path under `/laser-design/`, its content type, and its gzip bytes.
pub struct Asset {
    pub path: &'static str,
    pub content_type: &'static str,
    pub gzip: &'static [u8],
}

/// The files built into this hub: none without the `laser-design` feature.
#[cfg(feature = "laser-design")]
pub const ASSETS: &[Asset] = &[
    Asset {
        path: "index.html",
        content_type: "text/html; charset=utf-8",
        gzip: include_bytes!("../laser-design/index.html.gz"),
    },
    Asset {
        path: "pkg/laserfoundry_web.js",
        content_type: "text/javascript; charset=utf-8",
        gzip: include_bytes!("../laser-design/pkg/laserfoundry_web.js.gz"),
    },
    Asset {
        path: "pkg/laserfoundry_web_bg.wasm",
        content_type: "application/wasm",
        gzip: include_bytes!("../laser-design/pkg/laserfoundry_web_bg.wasm.gz"),
    },
];
#[cfg(not(feature = "laser-design"))]
pub const ASSETS: &[Asset] = &[];

pub fn routes() -> Router<Store> {
    Router::new()
        .route(
            "/laser-design",
            get(|| async { Redirect::to("/laser-design/") }),
        )
        .route("/laser-design/", get(index))
        .route("/laser-design/{*file}", get(file))
        .route("/api/laser-design/lasers", get(lasers))
}

/// The lasers of the space the person may use (use or more), as the apps API's `machines` lists them.
/// `machines`: the space's ([`crate::Store::machines`]).
fn usable_lasers(
    store: &crate::store::Legacy,
    machines: &[crate::store::Machine],
    access: &Access,
    grants: &crate::access::Grants,
) -> Vec<serde_json::Value> {
    machines
        .iter()
        .filter(|m| m.kind == MachineKind::Laser)
        .filter_map(|m| {
            let level = store.machine_level(access, &m.id, grants)?;
            (level >= Level::Use).then(|| crate::apps::machine_json(m, Some(level)))
        })
        .collect()
}

async fn lasers(access: Access, State(store): State<Store>) -> Response {
    let grants = match store.grants_of(&access).await {
        Ok(grants) => grants,
        Err(error) => return failed(error),
    };
    let machines = match store.machines(access.space).await {
        Ok(machines) => machines,
        Err(error) => return failed(error),
    };
    Json(usable_lasers(
        &store.lock().unwrap(),
        &machines,
        &access,
        &grants,
    ))
    .into_response()
}

/// Whether the laser design tool is there for this person (Prepare, a laser's page): derived from
/// the space's lasers and the person's permissions on them (Space), nothing of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Availability {
    Open,
    /// This hub was built without the page (feature `laser-design`).
    NotBuilt,
    /// A guest: looks, does not send.
    Guest,
    /// The space has no laser.
    NoLaser,
    /// There are lasers, none the person may use.
    NotPermitted,
}

pub(crate) fn availability(
    store: &crate::store::Legacy,
    machines: &[crate::store::Machine],
    access: &Access,
    grants: &crate::access::Grants,
) -> Availability {
    availability_of(store, machines, access, grants, !ASSETS.is_empty())
}

fn availability_of(
    store: &crate::store::Legacy,
    machines: &[crate::store::Machine],
    access: &Access,
    grants: &crate::access::Grants,
    built: bool,
) -> Availability {
    if !built {
        return Availability::NotBuilt;
    }
    if access.role < Role::Member {
        return Availability::Guest;
    }
    if !machines.iter().any(|m| m.kind == MachineKind::Laser) {
        return Availability::NoLaser;
    }
    if usable_lasers(store, machines, access, grants).is_empty() {
        Availability::NotPermitted
    } else {
        Availability::Open
    }
}

/// Whether this person may open the page: a member who may use a laser.
fn may_open(
    store: &crate::store::Legacy,
    machines: &[crate::store::Machine],
    access: &Access,
    grants: &crate::access::Grants,
) -> Result<(), Box<Response>> {
    if access.role < Role::Member {
        return Err(Box::new(
            (
                StatusCode::FORBIDDEN,
                "a guest may look, not send: ask an admin for a member role",
            )
                .into_response(),
        ));
    }
    if usable_lasers(store, machines, access, grants).is_empty() {
        return Err(Box::new(
            (
                StatusCode::FORBIDDEN,
                "there is no laser in this space that you may use",
            )
                .into_response(),
        ));
    }
    Ok(())
}

/// `/laser-design/`: the tool inside the hub's own page (owner, 2026-11-27: not "een aparte pagina zonder
/// onze huisstijl"): the hub's sidebar and bar around it, the fork's page in a frame. The frame's page is the
/// fork's `index.html` as built, served by `file` with the same policy.
async fn index(access: Access, State(store): State<Store>) -> Response {
    if let Err(refused) = allowed(&access, &store, ASSETS).await {
        return *refused;
    }
    let space = match store.space_name(access.space).await {
        Ok(name) => name,
        Err(error) => return failed(error),
    };
    (
        [
            (header::CACHE_CONTROL, "no-cache"),
            (
                header::HeaderName::from_static("permissions-policy"),
                PERMISSIONS_POLICY,
            ),
        ],
        axum::response::Html(format!(
            r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Laser design · Maker Connect</title>
<link rel="icon" href="../logo.svg" type="image/svg+xml">
<style>
{style}
  .ld-frame {{ --cut: var(--cut-l); clip-path: polygon(0 0, calc(100% - var(--cut)) 0, 100% var(--cut), 100% 100%, 0 100%); display: block; width: 100%; height: calc(100dvh - 190px); min-height: 420px; border: 0; background: #1c1f24; }}
  @media (max-width: 760px) {{ .ld-frame {{ height: calc(100dvh - 250px); }} }}
</style>
</head>
<body>
{header}
<main id="main" tabindex="-1" class="wide">
  <div class="page-head">
    <div><h2>Laser design</h2><p class="subtitle">Lay out artwork, set each layer's cut, preview it. <i>Send to</i> a laser loads it there; you frame and start it on the laser's page.</p></div>
  </div>
  <iframe class="ld-frame" src="index.html" title="Laser design (LaserFoundry)" allow=""></iframe>
</main>
</body>
</html>"##,
            style = crate::STYLE,
            header = crate::header(&space, "../", "prepare"),
        )),
    )
        .into_response()
}

async fn file(access: Access, State(store): State<Store>, Path(file): Path<String>) -> Response {
    serve(&access, &store, ASSETS, &file).await
}

/// Whether this person may open the tool, and whether this hub has it built in.
async fn allowed(
    access: &Access,
    store: &Store,
    assets: &'static [Asset],
) -> Result<(), Box<Response>> {
    let grants = store
        .grants_of(access)
        .await
        .map_err(|error| Box::new(failed(error)))?;
    let machines = store
        .machines(access.space)
        .await
        .map_err(|error| Box::new(failed(error)))?;
    may_open(&store.lock().unwrap(), &machines, access, &grants)?;
    if assets.is_empty() {
        return Err(Box::new(
            (
                StatusCode::NOT_FOUND,
                "this hub was built without the laser design page (feature laser-design)",
            )
                .into_response(),
        ));
    }
    Ok(())
}

/// The packed file `name`, as the browser takes it: gzip, its type, never cached without asking, and the page's
/// Permissions-Policy.
async fn serve(access: &Access, store: &Store, assets: &'static [Asset], name: &str) -> Response {
    if let Err(refused) = allowed(access, store, assets).await {
        return *refused;
    }
    let Some(asset) = assets.iter().find(|a| a.path == name) else {
        return (StatusCode::NOT_FOUND, "no such file").into_response();
    };
    (
        [
            (header::CONTENT_TYPE, asset.content_type),
            (header::CONTENT_ENCODING, "gzip"),
            (header::CACHE_CONTROL, "no-cache"),
            (
                header::HeaderName::from_static("permissions-policy"),
                PERMISSIONS_POLICY,
            ),
        ],
        asset.gzip,
    )
        .into_response()
}

#[cfg(test)]
mod tests;
