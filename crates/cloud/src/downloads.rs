//! Downloads: what is put in the folder beside the database (`maker-connect-downloads/` for
//! `maker-connect.db`), offered at `/downloads`. Meant for the agent, built for each
//! platform, so a computer next to a machine fetches it from the cloud it will report to.
//! Only files directly in that folder, by their plain name.
//!
//! A file named `maker-connect-agent-<os>-<arch>[.exe]` (`windows`, `macos`, `linux`;
//! `x86_64`, `aarch64`) gets a card of its own: the system's icon, and the command that
//! starts it against this cloud. Other files are listed under them.

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
    let arch = ["x86_64", "aarch64"].into_iter().find(|a| *a == arch)?;
    Some(Build { os, arch })
}

impl Build {
    fn title(&self) -> &'static str {
        match self.os {
            "windows" => "Windows",
            "macos" => "macOS",
            _ => "Linux",
        }
    }

    fn arch_title(&self) -> &'static str {
        match (self.os, self.arch) {
            ("macos", "aarch64") => "Apple silicon (M1 and later)",
            ("macos", _) => "Intel",
            ("linux", "aarch64") => "ARM 64-bit: Raspberry Pi 4 and 5",
            (_, "aarch64") => "ARM 64-bit",
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

    /// How to start it the first time, against this cloud; `--save` keeps the options.
    fn command(&self, name: &str, cloud: &str) -> String {
        let options = format!("run --cloud {cloud} --name \"My machine\" --save");
        match self.os {
            "windows" => format!(r".\{name} {options}"),
            _ => format!("chmod +x {name}\n./{name} {options}"),
        }
    }
}

fn megabytes(size: u64) -> String {
    format!("{:.1} MB", size as f64 / 1_000_000.0)
}

async fn page(State(store): State<Shared>, headers: HeaderMap) -> Response {
    let (space, dir, on_offer) = {
        let store = store.lock().unwrap();
        match store.default_space() {
            Ok((_, space)) => (space, store.downloads_dir.clone(), store.offered_build()),
            Err(error) => return failed(error),
        }
    };
    // This cloud's address as the visitor reached it.
    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .filter(|h| {
            h.chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c))
        })
        .unwrap_or("localhost:8460");
    let cloud = format!("http://{host}");
    let local = host.starts_with("localhost") || host.starts_with("127.");
    let files = offered(&dir);
    let mut cards = String::new();
    let mut others = String::new();
    for (name, size) in &files {
        let link = format!("downloads/{}", escape(name));
        match build(name) {
            Some(b) => {
                cards += &format!(
                    r#"<article class="build" data-os="{os}" data-arch="{arch}">
  <div class="build-head"><span class="os-icon {os}">{icon}</span><div><h3>{title}</h3><p>{arch_title}</p></div><span class="yours" hidden>Your system</span></div>
  {release}
  <a class="get" href="{link}" download>Download <small>{size}</small></a>
  <p class="how">Then, in {shell}, where you saved it:</p>
  <pre><code>{command}</code></pre>
  <button type="button" class="copy" data-copy>Copy</button>
</article>"#,
                    os = b.os,
                    arch = b.arch,
                    icon = b.icon(),
                    title = b.title(),
                    arch_title = b.arch_title(),
                    size = megabytes(*size),
                    release = on_offer.as_ref().map_or(String::new(), |b| format!(
                        r#"<p class="release"><b>Version {}</b> · build <code>{}</code><br>built {}</p>"#,
                        escape(b.version.as_deref().unwrap_or("?")),
                        escape(&b.commit),
                        escape(&b.built)
                    )),
                    shell = if b.os == "windows" {
                        "PowerShell"
                    } else {
                        "a terminal"
                    },
                    command = escape(&b.command(name, &cloud)),
                );
            }
            None => {
                others += &format!(
                    r#"<li><a href="{link}" download>{}</a> <small>{}</small></li>"#,
                    escape(name),
                    megabytes(*size)
                );
            }
        }
    }
    let body = if files.is_empty() {
        format!(
            r#"<p class="empty">Nothing yet. Put the agent in <code>{}</code>, named <code>maker-connect-agent-&lt;os&gt;-&lt;arch&gt;</code> (<code>windows</code>, <code>macos</code>, <code>linux</code>; <code>x86_64</code>, <code>aarch64</code>), for a card of its own here.</p>"#,
            escape(&dir.display().to_string())
        )
    } else {
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
  .note {{ color: var(--muted); font-size: 14px; }}
  .builds {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(300px, 1fr)); gap: 16px; margin: 20px 0; }}
  .build {{ position: relative; background: var(--card); border: 1px solid var(--line); border-radius: 14px; padding: 20px 22px; display: flex; flex-direction: column; gap: 12px; min-width: 0; }}
  .build pre {{ flex: 1; }}
  .build .copy {{ align-self: flex-start; }}
  .build.mine {{ border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }}
  /* The same height in every card, so the buttons under it line up. */
  .build-head {{ display: flex; align-items: center; gap: 14px; min-height: 56px; }}
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
  <p class="intro">The agent, for the computer next to a machine: it drives the machine and reports to this cloud. It shows a code; enter it under Add machine. With <code>--save</code> it keeps its options in <code>maker-connect-agent.toml</code>, so the next time a plain <code>run</code> (or a double-click on Windows) is enough.</p>
  {local_note}
  {build_note}
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
</script>
</body>
</html>
"##,
        header = header(&space, "", "downloads"),
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
                    r".\maker-connect-agent-windows-x86_64.exe run --cloud http://10.0.0.2:8460"
                )
        );
        assert_eq!(
            build("maker-connect-agent-linux-aarch64").unwrap().os,
            "linux"
        );
        assert!(build("maker-connect-agent-amiga-m68k").is_none());
        assert!(build("notes.txt").is_none());
    }

    #[tokio::test]
    async fn files_in_the_folder_are_offered_and_nothing_else() {
        let store = Store::open(":memory:").unwrap();
        let exe = "maker-connect-agent-windows-x86_64.exe";
        std::fs::write(store.downloads_dir.join(exe), b"MZ").unwrap();
        std::fs::write(store.downloads_dir.join("notes.txt"), b"hi").unwrap();
        std::fs::write(store.downloads_dir.join(".hidden"), b"x").unwrap();
        let app = crate::app(Arc::new(Mutex::new(store)));
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
            page.contains("--cloud http://192.168.1.20:8460"),
            "this cloud's address"
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
