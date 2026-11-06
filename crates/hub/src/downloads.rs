//! Downloads: what is put in the folder beside the database (`maker-connect-downloads/` for
//! `maker-connect.db`), offered at `/downloads`. Meant for the agent, built for each
//! platform, so a computer next to a machine fetches it from the hub it will report to.
//! Only files directly in that folder, by their plain name.
//!
//! Without such files the agent comes from the project's GitHub releases: the newest agent
//! release (`agent-v…`, a beta too), which the page looks up. The agent is released apart
//! from the hub (`hub-v…`), so no hub knows the one agent that goes with it.
//!
//! A file named `maker-connect-agent-<os>-<arch>[.exe]` (`windows`, `macos`, `linux`;
//! `x86_64`, `aarch64`, `armv6`) gets a card of its own: the system's icon, and the command that
//! starts it against this hub. Other files are listed under them.

use crate::{STYLE, Shared, escape, failed, header};
use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
};

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/downloads", get(page))
        .route("/downloads/{name}", get(download))
}

/// The files offered, by name, with their sizes.
fn offered(dir: &std::path::Path) -> Vec<(String, u64)> {
    let mut files: Vec<(String, u64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok().filter(|m| m.is_file())?;
            let name = entry.file_name().into_string().ok()?;
            plain(&name).then_some((name, meta.len()))
        })
        .collect();
    files.sort();
    files
}

/// A name that stays in the folder: no path, not hidden.
fn plain(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

/// The system an agent build is for, from its name.
#[derive(Debug, PartialEq)]
struct Build {
    /// `windows`, `macos`, `linux`.
    os: &'static str,
    arch: &'static str,
}

fn build(name: &str) -> Option<Build> {
    let rest = name.strip_prefix("maker-connect-agent-")?;
    let rest = rest.strip_suffix(".exe").unwrap_or(rest);
    let (os, arch) = rest.split_once('-')?;
    let os = ["windows", "macos", "linux"]
        .into_iter()
        .find(|o| *o == os)?;
    let arch = ["x86_64", "aarch64", "armv6"]
        .into_iter()
        .find(|a| *a == arch)?;
    Some(Build { os, arch })
}

impl Build {
    fn title(&self) -> &'static str {
        match (self.os, self.arch) {
            ("windows", _) => "Windows",
            ("macos", _) => "macOS",
            ("linux", "aarch64") => "Linux ARM64",
            ("linux", "armv6") => "Linux ARM 32-bit",
            _ => "Linux x86-64",
        }
    }

    fn arch_title(&self) -> &'static str {
        match (self.os, self.arch) {
            ("macos", "aarch64") => "Apple silicon (M1 and later)",
            ("macos", _) => "Intel",
            ("linux", "aarch64") => "Orange Pi Zero3, Raspberry Pi 4 and 5",
            ("linux", "armv6") => "Raspberry Pi Zero W, Pi 1: on an MK3S's RPi port",
            (_, "aarch64") => "ARM 64-bit",
            ("linux", _) => "A PC, a NAS, a mini PC",
            _ => "64-bit (Intel, AMD)",
        }
    }

    /// Plain shapes standing for each system, drawn here.
    fn icon(&self) -> &'static str {
        match self.os {
            "windows" => {
                r#"<svg viewBox="0 0 48 48" aria-hidden="true"><path fill="currentColor" d="M6 9.5 21 7.4v15.1H6zm17-2.4L42 4.4v18.1H23zM6 25.5h15v15.1L6 38.5zm17 0h19v18.1l-19-2.7z"/></svg>"#
            }
            "macos" => {
                r#"<svg viewBox="0 0 48 48" aria-hidden="true"><path fill="currentColor" d="M24 15c-2-1.6-4.6-2.4-7.4-2-5.4.8-8.6 6-8.1 12.4.5 6.5 4.2 14 8.3 15.4 2 .7 3.6-.3 5-1 1.4-.6 2.9-.6 4.3 0 1.4.7 3 1.7 5 1 3.3-1.1 6.2-6.4 7.5-11.2-4.4-1.8-6.4-7.1-3-11.4.6-.8 1.3-1.4 2.1-1.9-1.9-2.5-4.8-3.8-7.9-3.4-2.1.3-3.8 1.1-5.8 2.1z"/><path fill="currentColor" d="M24.3 12.6c-.2-4.6 3.1-8.3 7.6-8.6.3 4.6-3.1 8.4-7.6 8.6z"/></svg>"#
            }
            _ => {
                r##"<svg viewBox="0 0 48 48" aria-hidden="true"><ellipse cx="24" cy="31" rx="13" ry="14" fill="#1d1d1b"/><circle cx="24" cy="14" r="9.5" fill="#1d1d1b"/><ellipse cx="24" cy="32.5" rx="8.5" ry="10.5" fill="#fbfaf7"/><circle cx="20.5" cy="13" r="2.6" fill="#fbfaf7"/><circle cx="27.5" cy="13" r="2.6" fill="#fbfaf7"/><circle cx="21" cy="13.5" r="1.2" fill="#1d1d1b"/><circle cx="27" cy="13.5" r="1.2" fill="#1d1d1b"/><path d="M19.5 18.2q4.5-2.6 9 0-4.5 3.6-9 0z" fill="#e8a33a"/><ellipse cx="16" cy="44.5" rx="6" ry="2.6" fill="#e8a33a"/><ellipse cx="32" cy="44.5" rx="6" ry="2.6" fill="#e8a33a"/></svg>"##
            }
        }
    }

    /// How to start it the first time, against this hub; `--save` keeps the options.
    fn command(&self, name: &str, hub: &str) -> String {
        let options = format!("run --hub {hub} --name \"My machine\" --save");
        match self.os {
            "windows" => format!(r".\{name} {options}"),
            "linux" => format!(
                "chmod +x {name}\nsudo usermod -aG dialout $USER   # once: serial ports, then log in again\n./{name} {options}"
            ),
            _ => format!("chmod +x {name}\n./{name} {options}"),
        }
    }
}

/// Where the releases are.
const RELEASES: &str = "https://github.com/CuriousTimo/Maker-Connect/releases";

/// The agent builds each release carries.
const RELEASED: [&str; 6] = [
    "maker-connect-agent-windows-x86_64.exe",
    "maker-connect-agent-macos-aarch64",
    "maker-connect-agent-macos-x86_64",
    "maker-connect-agent-linux-x86_64",
    "maker-connect-agent-linux-aarch64",
    "maker-connect-agent-linux-armv6",
];

fn megabytes(size: u64) -> String {
    format!("{:.1} MB", size as f64 / 1_000_000.0)
}

async fn page(
    access: crate::access::Access,
    State(store): State<Shared>,
    headers: HeaderMap,
) -> Response {
    let role = access.role;
    let (space, dir, on_offer) = {
        let store = store.lock().unwrap();
        match store.space_of(&access) {
            Ok((_, space)) => (space, store.downloads_dir.clone(), store.offered_build()),
            Err(error) => return failed(error),
        }
    };
    // This hub's address as the visitor reached it.
    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .filter(|h| {
            h.chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c))
        })
        .unwrap_or("localhost:8460");
    let hub = format!("http://{host}");
    let local = host.starts_with("localhost") || host.starts_with("127.");
    let files = offered(&dir);
    let mut cards = String::new();
    let mut others = String::new();
    // Agent builds of our own here, or else those of the releases on GitHub.
    let from_github = !files.iter().any(|(name, _)| build(name).is_some());
    let mut listed: Vec<(String, Option<u64>, String)> = files
        .iter()
        .map(|(name, size)| {
            (
                name.clone(),
                Some(*size),
                format!("downloads/{}", escape(name)),
            )
        })
        .collect();
    if from_github {
        listed.extend(
            RELEASED
                .iter()
                .map(|name| ((*name).to_owned(), None, RELEASES.to_owned())),
        );
    }
    for (name, size, link) in &listed {
        match build(name) {
            Some(b) => {
                cards += &format!(
                    r#"<article class="build" data-os="{os}" data-arch="{arch}">
  <div class="build-head"><span class="os-icon {os}">{icon}</span><div><h3>{title}</h3><p>{arch_title}</p></div><span class="yours" hidden>Your system</span></div>
  {release}
  <a class="get" href="{link}"{asset} download>Download <small>{size}</small></a>
  <p class="how">Then, in {shell}, where you saved it:</p>
  <pre><code>{command}</code></pre>
  <button type="button" class="copy" data-copy>Copy</button>
</article>"#,
                    os = b.os,
                    arch = b.arch,
                    icon = b.icon(),
                    title = b.title(),
                    arch_title = b.arch_title(),
                    size = size.map_or("GitHub".to_owned(), megabytes),
                    asset = if from_github {
                        format!(r#" data-asset="{}""#, escape(name))
                    } else {
                        String::new()
                    },
                    release = if from_github {
                        format!(
                            r#"<p class="release"><b>{}</b> · from <a href="{RELEASES}">GitHub</a></p>"#,
                            "The newest agent release"
                        )
                    } else {
                        on_offer.as_ref().map_or(String::new(), |b| format!(
                        r#"<p class="release"><b>Version {}</b> · build <code>{}</code><br>built {}</p>"#,
                        escape(b.version.as_deref().unwrap_or("?")),
                        escape(&b.commit),
                        escape(&b.built)
                    ))
                    },
                    shell = if b.os == "windows" {
                        "PowerShell"
                    } else {
                        "a terminal"
                    },
                    command = escape(&b.command(name, &hub)),
                );
            }
            None => {
                others += &format!(
                    r#"<li><a href="{link}" download>{}</a> <small>{}</small></li>"#,
                    escape(name),
                    size.map_or(String::new(), megabytes)
                );
            }
        }
    }
    let body = {
        let others = if others.is_empty() {
            String::new()
        } else {
            format!(r#"<h3 class="others-title">Other files</h3><ul class="others">{others}</ul>"#)
        };
        format!(r#"<div class="builds">{cards}</div>{others}"#)
    };
    let local_note = if local {
        r#"<p class="note">You opened this page as <code>localhost</code>: on another computer, use this one's network address instead (as you would open this page from there).</p>"#
    } else {
        ""
    };
    Html(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Downloads · Maker Connect</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{STYLE}
  .intro {{ color: var(--muted); max-width: 720px; }}
  .update-all {{ display: flex; flex-direction: column; align-items: flex-start; gap: 6px; max-width: 720px; margin: 12px 0; }}
  .update-all small {{ color: var(--muted); }}
  .note {{ color: var(--muted); font-size: 14px; }}
  .builds {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(300px, 1fr)); gap: 0 16px; margin: 20px 0; row-gap: 0; }}
  .builds > .build {{ margin-bottom: 16px; }}
  /* Every card in a row shares its rows (subgrid): head, version, button, how, command,
     copy; so the buttons line up whatever the texts above them. */
  .build {{ position: relative; background: var(--card); border: 1px solid var(--line); border-radius: 14px; padding: 20px 22px; display: grid; grid-template-rows: subgrid; grid-row: span 6; gap: 12px; align-content: start; min-width: 0; }}
  .build pre {{ align-self: stretch; }}
  .build .copy {{ justify-self: start; align-self: start; }}
  .build.mine {{ border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }}
  /* The same height in every card, so the buttons under it line up. */
  .build-head {{ display: flex; align-items: center; gap: 14px; }}
  .build-head h3 {{ margin: 0; font-size: 18px; }}
  .build-head p {{ margin: 2px 0 0; color: var(--muted); font-size: 14px; }}
  .os-icon {{ width: 48px; height: 48px; flex: none; display: grid; place-items: center; border-radius: 12px; background: var(--bg); color: var(--ink); }}
  .os-icon svg {{ width: 30px; height: 30px; }}
  .os-icon.windows {{ color: #2f7fd1; }}
  .yours {{ position: absolute; top: -11px; right: 16px; white-space: nowrap; background: var(--card) !important; border: 1px solid var(--accent); padding: 2px 8px; border-radius: 6px; background: color-mix(in srgb, var(--accent) 15%, transparent); font-size: 11px; font-weight: 700; letter-spacing: .5px; text-transform: uppercase; color: var(--accent); }}
  .get {{ display: flex; justify-content: space-between; align-items: baseline; padding: 10px 14px; border-radius: 8px; background: var(--accent); color: #fff; font-weight: 600; text-decoration: none; }}
  .get small {{ font-weight: 400; opacity: .85; }}
  .release {{ margin: 0; font-size: 14px; line-height: 1.5; color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }}
  .release b {{ color: var(--ink); }}
  .how {{ margin: 0; color: var(--muted); font-size: 14px; }}
  .build pre {{ margin: 0; padding: 10px 12px; background: var(--bg); border-radius: 8px; font-size: 12.5px; white-space: pre-wrap; overflow-wrap: anywhere; }}
  .copy {{ justify-self: start; padding: 5px 12px; font-size: 13px; background: transparent; color: var(--ink); border: 1px solid var(--line); }}
  .others-title {{ font-size: 15px; color: var(--muted); }}
  .others {{ list-style: none; padding: 0; }}
  .others li {{ padding: 6px 0; }}
  .others small {{ color: var(--muted); }}
</style>
</head>
<body>
{header}
<main>
  <h2>Downloads</h2>
  <p class="intro">The agent, for the computer next to a machine: it drives the machine and reports to this hub. It shows a code; enter it with the ＋ next to "Machines" in the sidebar. With <code>--save</code> it keeps its options in <code>maker-connect-agent.toml</code>, so the next time a plain <code>run</code> (or a double-click on Windows) is enough.</p>
  {local_note}
  {build_note}
  {update_all}
{body}
</main>
<script>
  // The counts in the header, once.
  fetch("api/machines").then((r) => r.json()).then(summarise).catch(() => {{}});
  for (const button of document.querySelectorAll("[data-copy]")) button.addEventListener("click", async () => {{
    const text = button.parentElement.querySelector("code").textContent;
    let copied = false;
    try {{ await navigator.clipboard.writeText(text); copied = true; }} catch {{}}
    if (!copied) {{
      // Over plain http on the network the clipboard API is off: the old way.
      const area = document.createElement("textarea");
      area.value = text; document.body.append(area); area.select();
      copied = document.execCommand("copy");
      area.remove();
    }}
    button.textContent = copied ? "Copied" : "Select and copy it";
    setTimeout(() => (button.textContent = "Copy"), 1500);
  }});
  // The card of the visitor's own system, first and marked.
  (() => {{
    const ua = navigator.userAgent;
    const os = /Windows/.test(ua) ? "windows" : /Mac/.test(ua) ? "macos" : /Linux|X11/.test(ua) ? "linux" : "";
    const cards = [...document.querySelectorAll(".build")].filter((c) => c.dataset.os === os);
    if (cards.length !== 1 && os !== "macos") return;
    const card = os === "macos" ? cards.find((c) => c.dataset.arch === "aarch64") || cards[0] : cards[0];
    if (!card) return;
    card.classList.add("mine");
    card.querySelector(".yours").hidden = false;
    card.parentElement.prepend(card);
  }})();
  // The agent of the newest agent release (a beta too) on GitHub.
  (async () => {{
    const links = document.querySelectorAll("[data-asset]");
    if (!links.length) return;
    try {{
      const releases = await (await fetch("https://api.github.com/repos/CuriousTimo/Maker-Connect/releases?per_page=50")).json();
      const newest = releases.find((r) => r.tag_name.startsWith("agent-"));
      if (!newest) return;
      for (const link of links) {{
        const asset = newest.assets.find((a) => a.name === link.dataset.asset);
        if (!asset) continue;
        link.href = asset.browser_download_url;
        link.querySelector("small").textContent = (asset.size / 1e6).toFixed(1) + " MB";
      }}
      for (const b of document.querySelectorAll(".release b")) b.textContent = "Agent " + newest.tag_name.replace(/^agent-/, "");
    }} catch {{}}
  }})();
</script>
</body>
</html>
"##,
        header = header(&space, "", "downloads"),
        update_all = if role >= crate::access::Role::Admin {
            r#"<form class="update-all" method="post" action="agents/update" data-async data-done="Asked: each agent updates once its machine is idle"><button class="ghost">Update every agent in this space to the newest release</button><small>Checked against Maker Connect's signature, here and by each agent. An agent updates once its machine is idle, and goes back by itself if the new one does not reach the hub. Agents from before 0.2.0-beta.2 are installed by hand once.</small></form>"#
        } else {
            ""
        },
        build_note = on_offer.as_ref().map_or(String::new(), |_| {
            r#"<p class="note">The Connection card of each machine says which build its agent runs, and when the one here is newer.</p>"#.to_owned()
        }),
    ))
    .into_response()
}

async fn download(State(store): State<Shared>, Path(name): Path<String>) -> Response {
    if !plain(&name) {
        return (StatusCode::NOT_FOUND, "no such download").into_response();
    }
    let path = store.lock().unwrap().downloads_dir.join(&name);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                ("content-type", "application/octet-stream".to_owned()),
                (
                    "content-disposition",
                    format!("attachment; filename=\"{}\"", name.replace('"', "")),
                ),
            ],
            Body::from(bytes),
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "no such download").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    #[test]
    fn a_build_is_known_by_its_name() {
        let windows = build("maker-connect-agent-windows-x86_64.exe").unwrap();
        assert_eq!(
            windows,
            Build {
                os: "windows",
                arch: "x86_64"
            }
        );
        assert!(
            windows
                .command(
                    "maker-connect-agent-windows-x86_64.exe",
                    "http://10.0.0.2:8460"
                )
                .starts_with(
                    r".\maker-connect-agent-windows-x86_64.exe run --hub http://10.0.0.2:8460"
                )
        );
        assert_eq!(
            build("maker-connect-agent-linux-aarch64").unwrap().os,
            "linux"
        );
        assert!(build("maker-connect-agent-amiga-m68k").is_none());
        assert!(build("notes.txt").is_none());
    }

    /// Nothing of our own in the folder: the agent of the releases on GitHub, a card each.
    #[tokio::test]
    async fn without_builds_here_the_agent_comes_from_github() {
        let store = Store::open(":memory:").unwrap();
        let app = crate::auth::signed_in_app(Arc::new(Mutex::new(store)));
        let response = app
            .oneshot(Request::get("/downloads").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let page = String::from_utf8_lossy(&body);
        assert_eq!(page.matches(r#"class="build""#).count(), RELEASED.len());
        assert!(page.contains("github.com/CuriousTimo/Maker-Connect/releases"));
        assert!(!page.contains("Nothing yet"));
    }

    #[tokio::test]
    async fn files_in_the_folder_are_offered_and_nothing_else() {
        let store = Store::open(":memory:").unwrap();
        let exe = "maker-connect-agent-windows-x86_64.exe";
        std::fs::write(store.downloads_dir.join(exe), b"MZ").unwrap();
        std::fs::write(store.downloads_dir.join("notes.txt"), b"hi").unwrap();
        std::fs::write(store.downloads_dir.join(".hidden"), b"x").unwrap();
        let app = crate::auth::signed_in_app(Arc::new(Mutex::new(store)));
        let get = |path: &str| {
            let app = app.clone();
            let request = Request::get(path)
                .header("host", "192.168.1.20:8460")
                .body(Body::empty())
                .unwrap();
            async move {
                let response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let body = response.into_body().collect().await.unwrap().to_bytes();
                (status, String::from_utf8_lossy(&body).into_owned())
            }
        };
        let (status, page) = get("/downloads").await;
        assert_eq!(status, StatusCode::OK);
        assert!(page.contains(r#"data-os="windows""#), "a card for Windows");
        assert!(
            page.contains("--hub http://192.168.1.20:8460"),
            "this hub's address"
        );
        assert!(page.contains("notes.txt") && !page.contains("downloads/.hidden"));
        assert_eq!(
            get(&format!("/downloads/{exe}")).await,
            (StatusCode::OK, "MZ".into())
        );
        assert_eq!(get("/downloads/.hidden").await.0, StatusCode::NOT_FOUND);
        assert_eq!(get("/downloads/..%2Fx").await.0, StatusCode::NOT_FOUND);
    }
}
