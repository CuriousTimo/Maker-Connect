//! About: what this server is (its release and build), what it runs on (the system, its
//! processor and memory), its licence, and where the project lives: the code, the releases,
//! the manual on the wiki, and Losse-Eindj.es. For anyone signed in; who runs the server
//! also updates it here (crate::updates).

use crate::access::Access;
use crate::{STYLE, Shared, escape, failed, header};
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

pub fn routes() -> Router<Shared> {
    STARTED.get_or_init(crate::store::now);
    Router::new().route("/about", get(page))
}

/// What a command prints, trimmed; nothing when it does not run.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !text.is_empty()).then_some(text)
}

/// A value from a `key: value` (or `key=value`) file such as /proc/cpuinfo or /etc/os-release.
fn field(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|line| {
            let (k, v) = line.split_once([':', '='])?;
            (k.trim() == key).then(|| v.trim().trim_matches('"').to_owned())
        })
        .filter(|v| !v.is_empty())
}

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// What the server runs on, as far as this system says.
#[derive(Debug, Default)]
struct System {
    /// The distribution or system: `Armbian 23.8.1 Bullseye`, `macOS 15.1`.
    system: Option<String>,
    kernel: Option<String>,
    /// The board or computer: `Xunlong Orange Pi PC`, `MacBookPro18,3`.
    board: Option<String>,
    cpu: Option<String>,
    cores: Option<usize>,
    /// Bytes.
    memory: Option<u64>,
    memory_free: Option<u64>,
    /// Seconds since the computer started.
    up: Option<u64>,
}

fn system() -> System {
    let mut s = System {
        cores: std::thread::available_parallelism().ok().map(|n| n.get()),
        ..System::default()
    };
    match std::env::consts::OS {
        "linux" => {
            s.system = read("/etc/os-release").and_then(|t| field(&t, "PRETTY_NAME"));
            s.kernel = read("/proc/sys/kernel/osrelease").map(|k| k.trim().to_owned());
            s.board = read("/proc/device-tree/model")
                .map(|m| m.trim_end_matches('\0').trim().to_owned())
                .filter(|m| !m.is_empty());
            let cpuinfo = read("/proc/cpuinfo").unwrap_or_default();
            s.cpu = field(&cpuinfo, "model name").or_else(|| field(&cpuinfo, "Hardware"));
            let meminfo = read("/proc/meminfo").unwrap_or_default();
            let kib = |key| {
                field(&meminfo, key)
                    .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
                    .map(|k| k * 1024)
            };
            s.memory = kib("MemTotal");
            s.memory_free = kib("MemAvailable");
            s.up = read("/proc/uptime")
                .and_then(|u| u.split_whitespace().next()?.parse::<f64>().ok())
                .map(|u| u as u64);
        }
        "macos" => {
            s.system = output("sw_vers", &["-productVersion"]).map(|v| format!("macOS {v}"));
            s.kernel = output("uname", &["-r"]).map(|k| format!("Darwin {k}"));
            s.board = output("sysctl", &["-n", "hw.model"]);
            s.cpu = output("sysctl", &["-n", "machdep.cpu.brand_string"]);
            s.memory = output("sysctl", &["-n", "hw.memsize"]).and_then(|m| m.parse().ok());
            s.up = output("sysctl", &["-n", "kern.boottime"])
                .and_then(|b| {
                    b.split("sec = ")
                        .nth(1)?
                        .split(',')
                        .next()?
                        .trim()
                        .parse::<i64>()
                        .ok()
                })
                .map(|boot| (crate::store::now() - boot).max(0) as u64);
        }
        "windows" => {
            s.system = Some("Windows".to_owned());
            s.cpu = std::env::var("PROCESSOR_IDENTIFIER").ok();
        }
        _ => {}
    }
    s
}

fn bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.0} MB", n as f64 / (1u64 << 20) as f64),
        n => format!("{:.0} kB", n as f64 / 1024.0),
    }
}

/// `3 d 4 h`, `5 h 12 min`, `40 min`.
fn duration(seconds: u64) -> String {
    let (d, h, m) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    match (d, h) {
        (0, 0) => format!("{m} min"),
        (0, _) => format!("{h} h {m} min"),
        _ => format!("{d} d {h} h"),
    }
}

/// A row of a table, when there is something to say.
fn row(label: &str, value: Option<String>) -> String {
    value.map_or(String::new(), |v| {
        format!("<tr><th>{label}</th><td>{}</td></tr>", escape(&v))
    })
}

async fn page(access: Access, State(store): State<Shared>) -> Response {
    let (space, database) = {
        let store = store.lock().unwrap();
        match store.space_name(access.space) {
            Ok(name) => (name, store.db.path().unwrap_or_default().to_owned()),
            Err(error) => return failed(error),
        }
    };
    let s = tokio::task::spawn_blocking(system)
        .await
        .unwrap_or_default();
    let database_size = std::fs::metadata(&database).ok().map(|m| bytes(m.len()));
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
    let memory = s.memory.map(|total| match s.memory_free {
        Some(free) => format!("{} ({} free)", bytes(total), bytes(free)),
        None => bytes(total),
    });
    let started = STARTED.get().copied().unwrap_or_else(crate::store::now);
    let server_up = (crate::store::now() - started).max(0) as u64;
    let rows = [
        row("System", s.system),
        row("Kernel", s.kernel),
        row("Computer", s.board),
        row(
            "Processor",
            match (s.cpu, s.cores) {
                (Some(cpu), Some(n)) => Some(format!("{cpu}, {n} cores")),
                (None, Some(n)) => Some(format!("{n} cores")),
                (cpu, None) => cpu,
            },
        ),
        row("Architecture", Some(std::env::consts::ARCH.to_owned())),
        row("Memory", memory),
        row("Computer up", s.up.map(duration)),
        row("Server up", Some(duration(server_up))),
        row("Database", database_size),
    ]
    .concat();
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
  .about {{ max-width: 760px; }}
  .about .lead {{ color: var(--muted); }}
  .about section {{ background: var(--card); border: 1px solid var(--line); border-radius: 14px; padding: 18px 22px; margin: 16px 0; }}
  .about h3 {{ margin: 0 0 10px; font-size: 16px; }}
  .about table {{ border-collapse: collapse; width: 100%; }}
  .about th {{ text-align: left; font-weight: 500; color: var(--muted); padding: 5px 16px 5px 0; width: 34%; vertical-align: top; }}
  .about td {{ padding: 5px 0; overflow-wrap: anywhere; }}
  .about .links {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 10px; }}
  .about .links a {{ display: block; padding: 10px 14px; border: 1px solid var(--line); border-radius: 10px; text-decoration: none; color: var(--ink); }}
  .about .links a small {{ display: block; color: var(--muted); }}
  .about p {{ margin: 0 0 8px; }}
  .about .notes {{ max-height: 220px; overflow: auto; background: var(--bg); border-radius: 8px; padding: 10px 12px; font-size: 13px; white-space: pre-wrap; }}
  .about .bad {{ color: var(--bad, #c0392b); }}
</style>
</head>
<body>
{header}
<main class="about">
  <h2>About Maker Connect</h2>
  <p class="lead">One self-hosted place for a maker's machines: 3D printers, lasers and label printers, with their materials.</p>
  <section>
    <h3>This server</h3>
    <table>
      <tr><th>Release</th><td>{release_link}</td></tr>
      <tr><th>Build</th><td>{commit_link}, {built}</td></tr>
      {rows}
    </table>
  </section>
  {updates}
  <section>
    <h3>Links</h3>
    <div class="links">
      <a href="{REPO}/wiki">Manual<small>The wiki: getting started, the agent, every machine</small></a>
      <a href="{REPO}">Source code<small>GitHub: CuriousTimo/Maker-Connect</small></a>
      <a href="{REPO}/releases">Releases<small>The server and the agent</small></a>
      <a href="{REPO}/issues">Report a problem<small>GitHub issues</small></a>
      <a href="https://losse-eindj.es">Losse-Eindj.es<small>Who makes it</small></a>
    </div>
  </section>
  <section>
    <h3>Licence</h3>
    <p>Maker Connect is free software under the <a href="https://www.gnu.org/licenses/agpl-3.0.html">GNU Affero General Public License, version 3</a> (AGPL-3.0). You may use, study, change and share it. If you change it and let others use it over a network, offer them your source as well.</p>
    <p>It comes with no warranty. A laser and a heated printer can start a fire: stay with a running machine.</p>
    <p>With SpoolmanDB's filaments (MIT) and the printer profiles of PrusaSlicer's bundles.</p>
  </section>
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
      if (!confirm(`Update the hub to ${{u.newest}}? It restarts.`)) return;
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
const UPDATES: &str = r#"<section data-updates>
    <h3>Updates</h3>
    <p data-say>Looking for a newer release…</p>
    <p data-last hidden></p>
    <div class="notes" data-notes hidden></div>
    <p class="bad" data-printing hidden></p>
    <p data-windows hidden>On Windows, update by hand: stop the hub, unpack the new release over this one, and start it.</p>
    <button type="button" data-go hidden></button>
    <p class="lead">Every release is checked against Maker Connect's signature before it is used. The database is backed up, and the new build is tried on a copy of it first. If the new build does not stay up, the old build and the backup come back by themselves.</p>
  </section>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_and_sizes_read_plainly() {
        let os = "NAME=\"Armbian\"\nPRETTY_NAME=\"Armbian 23.8.1 bullseye\"\n";
        assert_eq!(
            field(os, "PRETTY_NAME").as_deref(),
            Some("Armbian 23.8.1 bullseye")
        );
        let cpu = "processor\t: 0\nmodel name\t: ARMv7 Processor rev 5 (v7l)\nHardware\t: Allwinner sun8i Family\n";
        assert_eq!(
            field(cpu, "model name").as_deref(),
            Some("ARMv7 Processor rev 5 (v7l)")
        );
        assert_eq!(bytes(1 << 30), "1.0 GB");
        assert_eq!(duration(3 * 86_400 + 4 * 3600), "3 d 4 h");
        assert_eq!(duration(40 * 60), "40 min");
        assert!(system().cores.is_some());
    }

    #[tokio::test]
    async fn the_about_page_says_what_runs() {
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;
        use std::sync::{Arc, Mutex};
        use tower::ServiceExt;
        let store = crate::store::Store::open(":memory:").unwrap();
        let app = crate::auth::signed_in_app(Arc::new(Mutex::new(store)));
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
    }
}
