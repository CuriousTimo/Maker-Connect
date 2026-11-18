//! Prepare: the tools that turn an idea into a job file for a machine (the architect's design,
//! 2026-11-17). Today the laser design tool (HS13); slicing and labels later. What a tool makes goes
//! to Files and from there to a machine; nothing here moves or starts a machine. Who sees what is
//! derived from the space's machines and the permissions set in Space, nothing of its own.

use crate::laser_design::{Availability, availability};
use crate::{STYLE, Store, failed, header};
use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
};

const PREPARE_STYLE: &str = "
  .tools { display: grid; grid-template-columns: repeat(auto-fill, minmax(280px, 1fr)); gap: 16px; }
  .tool { background: var(--card); border: 1px solid var(--line); border-radius: 12px; padding: 18px 20px; }
  .tool h3 { margin: 0 0 6px; }
  .tool p { margin: 0 0 12px; color: var(--muted); }
  .tool .tool-note { font-size: 13px; margin: 10px 0 0; }
  .tool a.button { display: inline-block; font-weight: 600; padding: 8px 14px; border-radius: 8px; background: var(--accent); color: #fff; text-decoration: none; }
";

pub async fn page(access: crate::access::Access, State(store): State<Store>) -> Response {
    let space = match store.space_name(access.space).await {
        Ok(name) => name,
        Err(error) => return failed(error),
    };
    let grants = match store.grants_of(&access).await {
        Ok(grants) => grants,
        Err(error) => return failed(error),
    };
    let laser = {
        let store = store.lock().unwrap();
        match availability(&store, &access, &grants) {
            Ok(a) => a,
            Err(error) => return failed(error),
        }
    };
    // Ours (UI only): the card's line for each state.
    let action = match laser {
        Availability::Open => {
            r#"<a class="button" href="laser-design/" target="_blank" rel="noopener">Open laser design</a><p class="tool-note">Opens in a new tab. <i>Send to</i> a laser loads it there; you frame and start it on the laser's page.</p>"#.to_owned()
        }
        Availability::NotBuilt => r#"<p class="tool-note">Not in this hub's build.</p>"#.to_owned(),
        Availability::Guest => r#"<p class="tool-note">A guest may look, not send: ask an admin for a member role.</p>"#.to_owned(),
        Availability::NoLaser => r#"<p class="tool-note">There is no laser in this space.</p>"#.to_owned(),
        Availability::NotPermitted => {
            r#"<p class="tool-note">You may not use a laser here yet: permission is given in Space.</p>"#.to_owned()
        }
    };
    Html(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Prepare · Maker Connect</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{STYLE}{PREPARE_STYLE}</style>
</head>
<body>
{header}
<main class="wide">
  <div class="page-head">
    <div><h2>Prepare</h2><p class="subtitle">Make the work for a machine. What you make goes to Files, and from there to a machine.</p></div>
  </div>
  <div class="tools">
    <section class="card tool">
      <h3>Laser design</h3>
      <p>Lay out artwork, set each layer's cut, preview it: LaserFoundry in the browser. Needs a laser you may use.</p>
      {action}
    </section>
  </div>
</main>
</body>
</html>"##,
        header = header(&space, "", "prepare"),
    ))
    .into_response()
}
