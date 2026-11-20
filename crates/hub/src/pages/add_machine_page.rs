//! Add machine: pairing by the code the machine shows (docs/design/ui.md "Add machine",
//! mockup docs/design/ui/mockups/add-machine.html). Per kind: a printer with built-in Connect
//! gets the Buddy INI with this hub's address; a machine on USB goes through the agent.
//! The code goes to `POST /pair` (crate::link::pair); staff only (crate::access).

use crate::access::Access;
use crate::{STYLE, Store, escape, failed, header};
use axum::{
    Router,
    extract::State,
    http::HeaderMap,
    response::{Html, IntoResponse, Response},
    routing::get,
};

pub fn routes() -> Router<Store> {
    Router::new().route("/machines/add", get(page))
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
<li class="done"><h3>What are you adding?</h3><p>A machine on USB: a 3D printer, a laser, a tape or label printer, a scale or a USB camera, through the agent on a computer beside it.</p></li>
<li class="now"><h3>Start the agent beside it</h3>
<p>On the computer the machine is plugged into, download the agent for that computer from <a href="../downloads">Downloads</a> and start it against this hub. It then shows a code.</p>
</li>
<li><h3>Enter the code it shows</h3>
{pair_usb}
</li>
<li><h3>Then: who may use it</h3><p>Set who may use it in <a href="../space/permissions">Space → Permissions</a>.</p></li>
</ol>
</section>

<section id="k-cam" role="tabpanel" aria-labelledby="t-k-cam" hidden>
<div class="card"><p>A network camera (RTSP) has no code: add it in <a href="../space#cameras">Space → Cameras</a>. A USB camera comes through the agent: pick <a href="#k-usb">A machine on USB</a>.</p></div>
</section>
</main>
<script>
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
}
