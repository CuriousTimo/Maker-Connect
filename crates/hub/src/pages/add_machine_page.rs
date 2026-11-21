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

/// An agent build by its name, as `scripts/build-release.sh` names it:
/// `maker-connect-agent-<os>-<arch>[.exe]`; its title in the picker.
fn agent_build(name: &str) -> Option<&'static str> {
    let rest = name.strip_prefix("maker-connect-agent-")?;
    Some(match rest {
        "linux-aarch64" => "Linux, Raspberry Pi (aarch64)",
        "linux-x86_64" => "Linux (x86_64)",
        "linux-armv6" => "Linux, Raspberry Pi Zero (armv6)",
        "macos-aarch64" => "macOS (Apple silicon)",
        "macos-x86_64" => "macOS (Intel)",
        "windows-x86_64.exe" => "Windows (x86_64)",
        _ => return None,
    })
}

/// The agent builds in the downloads folder, in the picker's order.
fn agent_builds(dir: &std::path::Path) -> Vec<(String, &'static str)> {
    let mut builds: Vec<(String, &'static str)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.metadata().is_ok_and(|m| m.is_file()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            agent_build(&name).map(|title| (name, title))
        })
        .collect();
    builds.sort_by_key(|(_, title)| *title);
    builds
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

/// The USB tab's second step: the picker, the download, and how to point the agent here.
fn usb_step(builds: &[(String, &'static str)], host: &str, port: &str, local: bool) -> String {
    let point = format!(
        r#"<p>Then open the agent's page on that computer, <code>http://&lt;that computer&gt;:8470</code>: under <b>Hub</b>, enter this hub's address <code>{host}</code>, port <code>{port}</code>, and save; turn on the machine's device. It then shows a code.</p>"#,
        host = if local {
            "&lt;this computer's address&gt;".to_owned()
        } else {
            escape(host)
        },
        port = escape(port),
    );
    let Some((first, _)) = builds.first() else {
        return format!(
            r#"<p>On the computer the machine is plugged into, start the Maker Connect agent.</p>
<div class="banner warn"><span><b>No agent build is on this hub yet.</b> Put the builds in the hub's downloads folder (beside its database), named as the releases name them (<code>maker-connect-agent-linux-aarch64</code>, …), or take them from the project's releases.</span></div>
{point}"#
        );
    };
    let options: String = builds
        .iter()
        .map(|(name, title)| format!(r#"<option value="{0}">{1}</option>"#, escape(name), title))
        .collect();
    format!(
        r#"<p>On the computer the machine is plugged into, download the agent for that computer and start it.</p>
<div class="row"><label class="field">That computer<select data-build>{options}</select></label><span class="fw"><a class="btn" data-download href="../downloads/{first}" download>Download the agent</a></span></div>
<pre data-start>{start}</pre>
{point}"#,
        first = escape(first),
        start = escape(&start(first)),
    )
}

/// How to start a build: a plain run; it keeps its settings in its folder.
fn start(name: &str) -> String {
    if name.ends_with(".exe") {
        format!(r".\{name}")
    } else {
        format!("chmod +x {name}\n./{name}")
    }
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
// The USB tab: the download and the start command follow the computer picked.
const build = document.querySelector("[data-build]");
if (build) build.addEventListener("change", () => {{
  const name = build.value;
  document.querySelector("[data-download]").href = "../downloads/" + encodeURIComponent(name);
  document.querySelector("[data-start]").textContent = name.endsWith(".exe") ? `.\\${{name}}` : `chmod +x ${{name}}\n./${{name}}`;
}});
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
        let names: Vec<&str> = builds.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "maker-connect-agent-linux-aarch64",
                "maker-connect-agent-windows-x86_64.exe"
            ]
        );
        let step = usb_step(&builds, "192.168.1.20", "8460", false);
        assert!(
            step.contains(r#"href="../downloads/maker-connect-agent-linux-aarch64""#),
            "{step}"
        );
        assert!(step.contains(
            "chmod +x maker-connect-agent-linux-aarch64\n./maker-connect-agent-linux-aarch64"
        ));
        assert!(step.contains("<code>192.168.1.20</code>, port <code>8460</code>"));
        assert_eq!(
            start("maker-connect-agent-windows-x86_64.exe"),
            r".\maker-connect-agent-windows-x86_64.exe"
        );
        assert!(usb_step(&[], "h", "1", false).contains("No agent build is on this hub yet"));
        assert!(agent_build("../etc/passwd").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
