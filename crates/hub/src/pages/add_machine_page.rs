//! Add machine: pairing by the code the machine shows (docs/design/ui.md "Add machine",
//! mockup docs/design/ui/mockups/add-machine.html). Per kind: a printer with built-in Connect
//! gets the Buddy INI with this hub's address; a machine on USB gets the agent's download, from
//! the hub's downloads folder (beside its database, `docs/design/storage-ops.md`), and how to
//! point the agent here; a network camera is added in Space → Cameras. The code goes to
//! `POST /pair` (crate::link::pair); staff only (crate::access).

use crate::access::Access;
use crate::{STYLE, Store, escape, failed, header};
use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/machines/add", get(page))
        .route("/downloads/{name}", get(download))
}

/// The downloads folder: beside the files folder, `maker-connect-downloads/` for
/// `maker-connect-files/` (`docs/design/storage-ops.md`).
fn downloads_dir(files_dir: &std::path::Path) -> std::path::PathBuf {
    let name = files_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    files_dir.with_file_name(match name.strip_suffix("-files") {
        Some(stem) => format!("{stem}-downloads"),
        None => format!("{name}-downloads"),
    })
}

/// The agent builds a release carries, by their names as `scripts/build-release.sh` names them:
/// `maker-connect-agent-<os>-<arch>[.exe]`. Only these are offered and served.
const RELEASED: [&str; 6] = [
    "maker-connect-agent-windows-x86_64.exe",
    "maker-connect-agent-macos-aarch64",
    "maker-connect-agent-macos-x86_64",
    "maker-connect-agent-linux-x86_64",
    "maker-connect-agent-linux-aarch64",
    "maker-connect-agent-linux-armv6",
];

/// The system an agent build is for, from its name (v1's `downloads.rs` at `35325c7^`, #192).
#[derive(Debug, PartialEq)]
struct Build {
    /// `windows`, `macos`, `linux`.
    os: &'static str,
    /// `x86_64`, `aarch64`, `armv6`.
    arch: &'static str,
}

/// An agent build by its exact name, one of [`RELEASED`].
fn agent_build(name: &str) -> Option<Build> {
    if !RELEASED.contains(&name) {
        return None;
    }
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

    /// Plain shapes standing for each system (v1's).
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

    /// Where it is started: PowerShell on Windows, else a terminal.
    fn shell(&self) -> &'static str {
        if self.os == "windows" {
            "PowerShell"
        } else {
            "a terminal"
        }
    }

    /// How to start it: a plain run (the v2 agent keeps its settings in its folder; the hub's
    /// address is entered on its page). Linux: v1's line for the serial ports.
    fn command(&self, name: &str) -> String {
        match self.os {
            "windows" => format!(r".\{name}"),
            "linux" => format!(
                "chmod +x {name}\nsudo usermod -aG dialout $USER   # once: serial ports, then log in again\n./{name}"
            ),
            _ => format!("chmod +x {name}\n./{name}"),
        }
    }
}

/// The agent builds in the downloads folder, with their sizes, by name.
fn agent_builds(dir: &std::path::Path) -> Vec<(String, Build, u64)> {
    let mut builds: Vec<(String, Build, u64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            let name = e.file_name().into_string().ok()?;
            agent_build(&name).map(|b| (name, b, meta.len()))
        })
        .collect();
    builds.sort_by(|a, b| a.0.cmp(&b.0));
    builds
}

fn megabytes(size: u64) -> String {
    format!("{:.1} MB", size as f64 / 1_000_000.0)
}

/// `GET /downloads/{name}`: an agent build from the downloads folder, by its exact name only.
async fn download(State(store): State<Store>, Path(name): Path<String>) -> Response {
    if agent_build(&name).is_none() {
        return (StatusCode::NOT_FOUND, "no such download").into_response();
    }
    let path = downloads_dir(store.files_dir()).join(&name);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{name}\""),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "no such download").into_response(),
    }
}

/// The USB tab's second step: a card per agent build, as v1's Downloads page had them (#192:
/// the system's icon and title, "Your system" first, the size, the start command with Copy), and
/// how to point the agent here.
fn usb_step(builds: &[(String, Build, u64)], host: &str, port: &str, local: bool) -> String {
    let point = format!(
        r#"<p>Then open the agent's page on that computer, <code>http://&lt;that computer&gt;:8470</code>: under <b>Hub</b>, enter this hub's address <code>{host}</code>, port <code>{port}</code>, and save; turn on the machine's device. It then shows a code.</p>"#,
        host = if local {
            "&lt;this computer's address&gt;".to_owned()
        } else {
            escape(host)
        },
        port = escape(port),
    );
    if builds.is_empty() {
        return format!(
            r#"<p>On the computer the machine is plugged into, start the Maker Connect agent.</p>
<div class="banner warn"><span><b>No agent build is on this hub yet.</b> Put the builds in the hub's downloads folder (beside its database), named as the releases name them (<code>maker-connect-agent-linux-aarch64</code>, …), or take them from the project's releases.</span></div>
{point}"#
        );
    }
    let cards: String = builds
        .iter()
        .map(|(name, b, size)| {
            format!(
                r#"<article class="card build" data-os="{os}" data-arch="{arch}">
<div class="build-head"><span class="os-icon {os}">{icon}</span><div><h3>{title}</h3><p class=muted>{arch_title}</p></div><span class="chip yours" hidden>Your system</span></div>
<a class="btn primary get" href="../downloads/{name}" download>Download <small>{size}</small></a>
<p class="muted how">Then, in {shell}, where you saved it:</p>
<pre><code>{command}</code></pre>
<span class="fw"><button type="button" class="btn small" data-copy>Copy</button></span>
</article>"#,
                os = b.os,
                arch = b.arch,
                icon = b.icon(),
                title = b.title(),
                arch_title = b.arch_title(),
                name = escape(name),
                size = megabytes(*size),
                shell = b.shell(),
                command = escape(&b.command(name)),
            )
        })
        .collect();
    format!(
        r#"<p>On the computer the machine is plugged into, download the agent for that computer and start it.</p>
<div class="grid builds">{cards}</div>
{point}"#
    )
}

/// Buddy keeps at most this many bytes of the hostname (`reference/buddy-6.4.0`
/// `src/connect/hostname.hpp:16`, `max_host_len`).
const BUDDY_MAX_HOST: usize = 35;

/// The hub's address as the browser reached it: host and port from the `Host` header.
fn address(headers: &HeaderMap) -> (String, String) {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    // An IPv6 address is in brackets: its port, if any, comes after the "]".
    let split = match host.rfind(']') {
        Some(end) => host[end..].find(':').map(|i| end + i),
        None => host.rfind(':'),
    };
    match split {
        Some(i) => (host[..i].to_owned(), host[i + 1..].to_owned()),
        None => (host.to_owned(), "80".to_owned()),
    }
}

async fn page(access: Access, State(store): State<Store>, headers: HeaderMap) -> Response {
    let name = match store.space_name(access.space).await {
        Ok(name) => name,
        Err(error) => return failed(error),
    };
    let (host, port) = address(&headers);
    let local = ["localhost", "127.0.0.1", "[::1]"].contains(&host.as_str());
    let warning = if local {
        format!(
            r#"<div class="banner warn"><span>You opened this hub as <code>{}</code>: the printer cannot reach that. Put this computer's network address in <code>hostname</code>.</span></div>"#,
            escape(&host)
        )
    } else if host.len() > BUDDY_MAX_HOST {
        format!(
            r#"<div class="banner warn"><span>The address must fit in the printer's hostname field: <b>{BUDDY_MAX_HOST} bytes</b>. <code>{}</code> is {} bytes: use this hub's IP address.</span></div>"#,
            escape(&host),
            host.len()
        )
    } else {
        String::new()
    };
    let ini = format!(
        "[service::connect]\nhostname = {}\nport = {port}\ntls = false",
        if local {
            "<this computer's address>"
        } else {
            &host
        }
    );
    let pair = |id: &str| {
        format!(
            r#"<form class="row" data-pair id="pair-{id}"><label class="field">Code<input name="code" required autocomplete="off" autocapitalize="characters" class="num" style="letter-spacing:.2em;text-transform:uppercase" placeholder="from the machine"></label><label class="field">Name here<input name="name" maxlength="60" placeholder="as the machine calls itself"></label><span class="fw"><button class="btn primary">Add</button></span></form>
<div data-pair-result aria-live="polite"></div>"#
        )
    };
    Html(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Add machine · {title}</title>
<link rel="icon" href="../logo.svg" type="image/svg+xml">
<style>
{STYLE}
/* #192: the agent's builds as v1's cards */
.grid.builds {{ row-gap: 0; }}
.builds > .build {{ margin-bottom: var(--s-4); }}
/* Every card in a row shares its rows (subgrid, as v1): head, button, how, command, copy;
   so the buttons and texts line up whatever the texts above them. */
.build {{ display: grid; grid-template-rows: subgrid; grid-row: span 5; gap: var(--s-3); align-content: start; position: relative; }}
.build > .fw {{ justify-self: start; align-self: start; }}
.build-head {{ display: flex; align-items: center; gap: var(--s-3); }}
.build-head h3 {{ margin: 0; }}
.build-head p {{ margin: 2px 0 0; }}
.os-icon {{ width: 48px; height: 48px; flex: none; display: grid; place-items: center; background: var(--bg); color: var(--ink); }}
.os-icon svg {{ width: 30px; height: 30px; }}
.os-icon.windows {{ color: #2f7fd1; }}
.yours {{ margin-left: auto; color: var(--accent-text); }}
.build.mine {{ --bc: var(--accent-text); }}
.get {{ justify-content: space-between; }}
.get small {{ font-weight: 400; opacity: .85; }}
.build .how {{ margin: 0; }}
.build pre {{ margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; }}
</style>
</head>
<body>
{header}
<main id="main" tabindex="-1">
<div class="head"><h1>Add machine</h1></div>
<div class="tabs" role="tablist" data-tabs aria-label="What are you adding?"><a href="#k-connect" role="tab" id="t-k-connect" aria-controls="k-connect">A printer with built-in Connect</a><a href="#k-usb" role="tab" id="t-k-usb" aria-controls="k-usb">A machine on USB</a><a href="#k-cam" role="tab" id="t-k-cam" aria-controls="k-cam">A network camera</a></div>

<section id="k-connect" role="tabpanel" aria-labelledby="t-k-connect">
<ol class="steps">
<li class="done"><h3>What are you adding?</h3><p>A printer with built-in Connect: its own firmware connects to the hub, no agent.</p></li>
<li class="now"><h3>Point it here</h3>
<p>Put this file on the printer's USB stick as <code>prusa_printer_settings.ini</code>; then, on the printer, load the settings and add it to Connect.</p>
<pre data-ini>{ini}</pre>
{warning}
</li>
<li><h3>Enter the code it shows</h3>
{pair_connect}
</li>
<li><h3>Then: who may use it</h3><p>Set who may use it in <a href="../space/permissions">Space → Permissions</a>.</p></li>
</ol>
</section>

<section id="k-usb" role="tabpanel" aria-labelledby="t-k-usb" hidden>
<ol class="steps">
<li class="done"><h3>What are you adding?</h3><p>A machine on USB, through the Maker Connect agent on a computer beside it.</p></li>
<li class="now"><h3>Start the agent beside it</h3>
{usb}
</li>
<li><h3>Enter the code it shows</h3>
{pair_usb}
</li>
<li><h3>Then: who may use it</h3><p>Set who may use it in <a href="../space/permissions">Space → Permissions</a>.</p></li>
</ol>
</section>

<section id="k-cam" role="tabpanel" aria-labelledby="t-k-cam" hidden>
<div class="card"><p>A network camera (RTSP) has no code: add it in <a href="../space#cameras">Space → Cameras</a>.</p></div>
</section>
</main>
<script>
// The USB tab (v1's Downloads page, #192): Copy for each start command.
for (const button of document.querySelectorAll("[data-copy]")) button.addEventListener("click", async () => {{
  const text = button.closest(".build").querySelector("code").textContent;
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
for (const form of document.querySelectorAll("form[data-pair]")) form.addEventListener("submit", async (event) => {{
  event.preventDefault();
  const out = form.nextElementSibling, button = form.querySelector("button");
  button.setAttribute("aria-busy", "true");
  const r = await fetch("../pair", {{ method: "POST", body: new URLSearchParams(new FormData(form)), headers: {{ "x-requested-with": "fetch" }} }});
  button.removeAttribute("aria-busy");
  if (r.ok) {{
    const m = await r.json();
    out.innerHTML = `<div class="banner ok"><span><b>${{escapeHtml(m.name)}} is added</b> and reports to the hub. Its first values come in now.</span><span class="fw"><a class="btn small" href="${{encodeURIComponent(m.id)}}">Open ${{escapeHtml(m.name)}}</a></span></div>`;
    form.reset();
  }} else if (r.status === 404) {{
    out.innerHTML = '<div class="banner bad"><span><b>That code is not waiting here.</b> It may have expired: the printer or the agent shows a new one; enter that one.</span></div>';
  }} else {{
    out.innerHTML = `<div class="banner bad"><span><b>Not added:</b> ${{escapeHtml(await r.text())}}</span></div>`;
  }}
}});
</script>
</body>
</html>"##,
        title = escape(&name),
        header = header(&name, "../", "machines/add"),
        pair_connect = pair("connect"),
        pair_usb = pair("usb"),
        usb = usb_step(&agent_builds(&downloads_dir(store.files_dir())), &host, &port, local),
        ini = escape(&ini),
    ))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_comes_from_the_host_header() {
        let mut h = HeaderMap::new();
        h.insert("host", "192.168.1.20:8460".parse().unwrap());
        assert_eq!(address(&h), ("192.168.1.20".into(), "8460".into()));
        h.insert("host", "hub.example".parse().unwrap());
        assert_eq!(address(&h), ("hub.example".into(), "80".into()));
        h.insert("host", "[::1]:8460".parse().unwrap());
        assert_eq!(address(&h), ("[::1]".into(), "8460".into()));
    }

    /// The downloads folder sits beside the files folder; only the release's agent names count.
    #[test]
    fn the_agent_builds_come_from_the_downloads_folder() {
        assert_eq!(
            downloads_dir(std::path::Path::new("/var/lib/mc/maker-connect-files")),
            std::path::PathBuf::from("/var/lib/mc/maker-connect-downloads")
        );
        let dir = std::env::temp_dir().join(format!("mc-downloads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "maker-connect-agent-linux-aarch64",
            "maker-connect-agent-windows-x86_64.exe",
            "maker-connect-agent-linux-sparc",
            "notes.txt",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let builds = agent_builds(&dir);
        let names: Vec<&str> = builds.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "maker-connect-agent-linux-aarch64",
                "maker-connect-agent-windows-x86_64.exe"
            ]
        );
        // #192: a card per build, as v1's Downloads page had them
        let step = usb_step(&builds, "192.168.1.20", "8460", false);
        assert!(
            step.contains(r#"<article class="card build" data-os="linux" data-arch="aarch64">"#),
            "{step}"
        );
        assert!(step.contains(
            "<h3>Linux ARM64</h3><p class=muted>Orange Pi Zero3, Raspberry Pi 4 and 5</p>"
        ));
        assert!(
            step.contains(r#"<a class="btn primary get" href="../downloads/maker-connect-agent-linux-aarch64" download>Download <small>0.0 MB</small></a>"#),
            "{step}"
        );
        assert!(step.contains("Then, in PowerShell, where you saved it:"));
        assert!(step.contains(
            "<pre><code>chmod +x maker-connect-agent-linux-aarch64\nsudo usermod -aG dialout $USER   # once: serial ports, then log in again\n./maker-connect-agent-linux-aarch64</code></pre>"
        ));
        assert!(step.contains(r"<pre><code>.\maker-connect-agent-windows-x86_64.exe</code></pre>"));
        assert_eq!(step.matches("data-copy>Copy</button>").count(), 2);
        assert!(step.contains("<code>192.168.1.20</code>, port <code>8460</code>"));
        assert!(
            usb_step(&builds, "localhost", "8460", true)
                .contains("&lt;this computer's address&gt;")
        );
        assert_eq!(
            agent_build("maker-connect-agent-macos-aarch64").map(|b| (b.title(), b.arch_title())),
            Some(("macOS", "Apple silicon (M1 and later)"))
        );
        assert!(agent_build("maker-connect-agent-windows-armv6.exe").is_none());
        assert!(usb_step(&[], "h", "1", false).contains("No agent build is on this hub yet"));
        assert!(agent_build("../etc/passwd").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
