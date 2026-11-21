//! About: what this server is (its release and build), what it runs on (the system, its
//! processor and memory), its licence, and where the project lives: the code, the releases,
//! the manual on the wiki, and Losse-Eindj.es. For anyone signed in; who runs the server
//! also updates it here (crate::updates).

use crate::access::Access;
use crate::{STYLE, Store, escape, failed, header};
use axum::{
    Router,
    extract::State,
    response::{Html, IntoResponse, Response},
    routing::get,
};
use std::sync::OnceLock;

const REPO: &str = "https://github.com/CuriousTimo/Maker-Connect";

/// When this server started (Unix seconds).
static STARTED: OnceLock<i64> = OnceLock::new();

pub fn routes() -> Router<Store> {
    STARTED.get_or_init(crate::store::now);
    Router::new().route("/about", get(page))
}

use maker_connect_system::{bytes, duration};

/// A row of a card's list (`dl.kv`), when there is something to say.
fn row(label: &str, value: Option<String>) -> String {
    value.map_or(String::new(), |v| {
        format!("<dt>{label}</dt><dd>{}</dd>", escape(&v))
    })
}

async fn page(access: Access, State(store): State<Store>) -> Response {
    let space = match store.space_name(access.space).await {
        Ok(name) => name,
        Err(error) => return failed(error),
    };
    // The database's size as Postgres counts it (no table read: the app pool).
    let database_size = sqlx::query_scalar::<_, i64>("SELECT pg_database_size(current_database())")
        .fetch_one(&store.pg().app)
        .await
        .ok()
        .map(|n| bytes(n as u64));
    let (computer, load) = tokio::task::spawn_blocking(|| {
        (
            maker_connect_system::computer(),
            maker_connect_system::load(),
        )
    })
    .await
    .unwrap_or_default();
    let commit = env!("MC_COMMIT");
    let commit_link = if commit.ends_with("-dirty") || commit == "unknown" {
        escape(commit)
    } else {
        format!(r#"<a href="{REPO}/commit/{commit}">{commit}</a>"#)
    };
    let release = crate::store::BUILD;
    let release_link = if release.starts_with('v') {
        format!(
            r#"<a href="{REPO}/releases/tag/hub-{0}">{0}</a>"#,
            escape(release)
        )
    } else {
        format!("{} (built from the source)", escape(release))
    };
    let started = STARTED.get().copied().unwrap_or_else(crate::store::now);
    let server_up = (crate::store::now() - started).max(0) as u64;
    // The cards of the agent's About (crates/agent `about_page`): this hub, its load, the computer.
    let hub_rows = [
        row("Hub up", Some(duration(server_up))),
        row("Database", database_size),
    ]
    .concat();
    let mut computer_rows: String = maker_connect_system::computer_rows(&computer)
        .into_iter()
        .map(|(label, value)| row(label, Some(value)))
        .collect();
    computer_rows += &row("Computer up", load.up.map(duration));
    let load_rows: String = maker_connect_system::load_rows(&load)
        .into_iter()
        .map(|(label, value)| row(label, Some(value)))
        .collect();
    let load_card = if load_rows.is_empty() {
        String::new()
    } else {
        format!(r#"<div class="card"><h2>Load</h2><dl class="kv">{load_rows}</dl></div>"#)
    };
    Html(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>About · Maker Connect</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{STYLE}
</style>
</head>
<body>
{header}
<main id="main" tabindex="-1" class="about">
<div class="head"><h1>About Maker Connect</h1></div>
<p class="muted">One self-hosted place for a maker's machines: 3D printers, lasers and label printers, with their materials.</p>
<div class="cols-even">
<div class="card"><h2>This hub</h2><dl class="kv"><dt>Release</dt><dd>{release_link}</dd><dt>Build</dt><dd>{commit_link}, {built}</dd>{hub_rows}</dl></div>
{load_card}
<div class="card"><h2>Computer</h2><dl class="kv">{computer_rows}</dl></div>
{updates}
<div class="card"><h2>Links</h2><ul class="list">
<li><div class="grow"><a href="{REPO}/wiki">Manual</a><br><small>The wiki: getting started, the agent, every machine</small></div></li>
<li><div class="grow"><a href="{REPO}">Source code</a><br><small>GitHub: CuriousTimo/Maker-Connect</small></div></li>
<li><div class="grow"><a href="{REPO}/releases">Releases</a><br><small>The server and the agent</small></div></li>
<li><div class="grow"><a href="{REPO}/issues">Report a problem</a><br><small>GitHub issues</small></div></li>
<li><div class="grow"><a href="https://losse-eindj.es">Losse-Eindj.es</a><br><small>Who makes it</small></div></li>
</ul></div>
<div class="card"><h2>Licence</h2>
<p>Maker Connect is free software under the <a href="https://www.gnu.org/licenses/agpl-3.0.html">GNU Affero General Public License, version 3</a> (AGPL-3.0). You may use, study, change and share it. If you change it and let others use it over a network, offer them your source as well.</p>
<p>It comes with no warranty. A laser and a heated printer can start a fire: stay with a running machine.</p>
<p>With SpoolmanDB's filaments (MIT) and the printer profiles of PrusaSlicer's bundles.</p>
</div>
</div>
</main>
<script>
  fetch("api/machines").then((r) => r.json()).then(summarise).catch(() => {{}});
  // Updating the hub: what is newer, and the button (who runs the server only).
  (async () => {{
    const box = document.querySelector("[data-updates]");
    if (!box) return;
    const say = box.querySelector("[data-say]");
    const button = box.querySelector("[data-go]");
    let u;
    try {{ u = await (await fetch("api/update")).json(); }} catch {{ say.textContent = "Could not look for updates."; return; }}
    if (u.last) {{
      const when = new Date(u.last.at * 1000).toLocaleString();
      const last = box.querySelector("[data-last]");
      last.hidden = false;
      last.textContent = u.last.ok ? `Last update: ${{u.last.from}} to ${{u.last.to}}, ${{when}}: it works.` : `The update to ${{u.last.to}} did not work (${{when}}): ${{u.last.note}}. It went back to ${{u.last.from}}.`;
      last.classList.toggle("bad", !u.last.ok);
    }}
    if (u.none) {{ say.textContent = "There is no hub release on GitHub yet."; return; }}
    if (u.error) {{ say.textContent = `Could not look for updates: ${{u.error}}`; return; }}
    if (!u.newer) {{ say.textContent = `This is the newest release (${{u.newest}}).`; return; }}
    say.textContent = `${{u.newest}} is out (this hub is ${{u.current}}).`;
    const notes = box.querySelector("[data-notes]");
    notes.textContent = u.notes || "";
    notes.hidden = !u.notes;
    if (u.windows) {{ box.querySelector("[data-windows]").hidden = false; return; }}
    if (u.printing) {{
      const warn = box.querySelector("[data-printing]");
      warn.hidden = false;
      warn.textContent = `${{u.printing}} job(s) running. Printers on Prusa Connect and agents keep going and catch up after the restart, which takes a few seconds.`;
    }}
    button.textContent = `Update to ${{u.newest}}`;
    button.hidden = false;
    button.addEventListener("click", async () => {{
      if (!(await ask(`Update the hub to ${{u.newest}}? It restarts.`, {{ ok: "Update" }}))) return;
      button.disabled = true;
      say.textContent = "Fetching and checking the release, backing up the database, trying the new build…";
      const response = await fetch("about/update", {{ method: "POST", headers: {{ "x-requested-with": "fetch" }} }});
      if (!response.ok) {{ say.textContent = `Not updated: ${{await response.text()}}`; button.disabled = false; return; }}
      say.textContent = `Restarting as ${{u.newest}}…`;
      await new Promise((r) => setTimeout(r, 3000));
      for (let i = 0; i < 60; i++) {{
        try {{ if ((await fetch("about", {{ cache: "no-store" }})).ok) {{ location.reload(); return; }} }} catch {{}}
        await new Promise((r) => setTimeout(r, 2000));
      }}
      say.textContent = "The hub does not answer yet: look at its log.";
    }});
  }})();
</script>
</body>
</html>
"##,
        built = escape(env!("MC_BUILT")),
        updates = if access.instance_admin {
            UPDATES
        } else {
            ""
        },
        header = header(&space, "", "about"),
    ))
    .into_response()
}

/// The Updates section, for who runs the server; the page's script fills it in.
const UPDATES: &str = r#"<div class="card" data-updates>
    <h2>Updates</h2>
    <p data-say>Looking for a newer release…</p>
    <p data-last hidden></p>
    <div class="notes" data-notes hidden></div>
    <p class="bad" data-printing hidden></p>
    <p data-windows hidden>On Windows, update by hand: stop the hub, unpack the new release over this one, and start it.</p>
    <button type="button" data-go hidden></button>
    <p class="muted">Every release is checked against Maker Connect's signature before it is used. The database is backed up, and the new build is tried on a copy of it first. If the new build does not stay up, the old build and the backup come back by themselves.</p>
  </div>"#;

#[cfg(test)]
mod tests {
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_about_page_says_what_runs(pool: sqlx::PgPool) {
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;

        use tower::ServiceExt;
        let store = crate::store::Legacy::open(":memory:").unwrap();
        let app =
            crate::auth::signed_in_app(crate::store::Store::with_legacy(&pool, store).await).await;
        let response = app
            .oneshot(Request::get("/about").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let page = String::from_utf8_lossy(&body);
        assert!(
            page.contains("AGPL-3.0") && page.contains("/wiki") && page.contains("losse-eindj.es")
        );
        assert!(page.contains(env!("MC_COMMIT")));
        assert!(page.contains("<h2>Load</h2>") && page.contains("Memory in use"));
        assert!(page.contains(r#"<div class="card"><h2>This hub</h2>"#));
    }
}
