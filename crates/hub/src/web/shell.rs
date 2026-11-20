use askama::Template;
use axum::response::{IntoResponse, Response};

/// One link of the navigation (docs/design/ui.md "Roles and what they see"). `gate` hides it
/// until `/api/me` shows the person may see it: "staff" (a space's admin or owner), "member"
/// (a guest or member: their own permissions), "instance" (an instance admin). The hub
/// refuses the pages and calls behind a hidden link too; hiding is only the navigation.
pub(crate) struct NavItem {
    pub group: &'static str,
    pub path: &'static str,
    pub name: &'static str,
    pub place: &'static str,
    pub gate: &'static str,
    pub icon: &'static str,
}

const fn item(
    group: &'static str,
    path: &'static str,
    name: &'static str,
    place: &'static str,
    gate: &'static str,
    icon: &'static str,
) -> NavItem {
    NavItem {
        group,
        path,
        name,
        place,
        gate,
        icon,
    }
}

/// The chrome of every page: the skip link, the sticky top bar with the theme switch, the
/// sidebar (desktop, >= 900 px) and the bottom bar with More (phone). `templates/chrome.html`.
#[derive(Template)]
#[template(path = "chrome.html")]
pub(crate) struct Chrome<'a> {
    pub space: &'a str,
    pub home: &'a str,
    pub here: &'a str,
    /// The page after which the sidebar lists the machines, live.
    pub live_after: &'a str,
    pub nav: Vec<NavItem>,
    pub bottom: Vec<NavItem>,
    pub more: Vec<NavItem>,
}

impl<'a> Chrome<'a> {
    /// The navigation of this build: Prepare only with a tool in it (the laser design),
    /// Cameras only with the network cameras built in.
    pub(crate) fn new(space: &'a str, root: &'a str, here: &'a str) -> Self {
        let prepare = !crate::laser_design::ASSETS.is_empty();
        Self::with(space, root, here, prepare, cfg!(feature = "cameras"))
    }

    /// The navigation with or without Prepare and Cameras.
    pub(crate) fn with(
        space: &'a str,
        root: &'a str,
        here: &'a str,
        prepare: bool,
        cameras: bool,
    ) -> Self {
        // From the root, the pages are at "/…".
        let home = if root.is_empty() { "/" } else { root };
        let mut nav = vec![
            item("", "", "Now", "now", "", "◷"),
            item("Work", "machines", "Machines", "machines", "", "▦"),
        ];
        if prepare {
            nav.push(item("", "prepare", "Prepare", "prepare", "", ""));
        }
        nav.push(item("", "projects", "Queue and farm", "projects", "", ""));
        nav.push(item("Stock", "files", "Files", "files", "", "▤"));
        nav.push(item("", "stock", "Materials", "stock", "", "◉"));
        if cameras {
            nav.push(item("Watch", "cameras", "Cameras", "cameras", "", ""));
        }
        let live_after = if cameras { "cameras" } else { "stock" };
        let manage = [
            item(
                "",
                "space/permissions",
                "My permissions",
                "space",
                "member",
                "",
            ),
            item(
                "Manage",
                "machines/add",
                "Add machine",
                "machines/add",
                "staff",
                "",
            ),
            item("", "space", "Space", "space", "staff", ""),
            item("", "downloads", "Downloads", "downloads", "staff", ""),
            item("", "about", "About", "about", "instance", ""),
        ];
        let bottom = nav
            .iter()
            .filter(|n| ["now", "machines", "files", "stock"].contains(&n.place))
            .map(|n| item("", n.path, n.name, n.place, "", n.icon))
            .collect();
        let more = nav
            .iter()
            .chain(manage.iter())
            .filter(|n| !["now", "machines", "files", "stock"].contains(&n.place))
            .map(|n| item("", n.path, n.name, n.place, n.gate, ""))
            .collect();
        nav.extend(manage);
        Chrome {
            space,
            home,
            here,
            live_after,
            nav,
            bottom,
            more,
        }
    }
}

/// The chrome at the top of every page's body, and the shell's script. `root` leads from the
/// page to the site's root ("" or "../"); `here` is the page's place in the navigation.
pub(crate) fn header(space: &str, root: &str, here: &str) -> String {
    let chrome = Chrome::new(space, root, here)
        .render()
        .expect("the chrome renders");
    format!("{THEME_SCRIPT}\n{chrome}\n<script>\n{THEME_SWITCH}{SHELL_SCRIPT}</script>")
}

/// The theme before the first paint: Light or Dark when chosen, else the system's.
pub(crate) const THEME_SCRIPT: &str = r#"<script>try{var t=localStorage.getItem("mc-theme");if(t==="light"||t==="dark")document.documentElement.dataset.theme=t;}catch(e){}</script>"#;

/// The logo mark (the house motif), for the page's icon.
pub(crate) async fn logo() -> Response {
    (
        [("content-type", "image/svg+xml")],
        include_str!("../../../../assets/logo-v2-mark.svg"),
    )
        .into_response()
}

/// The large logo (mark and name), for a light page.
pub(crate) async fn logo_large() -> Response {
    (
        [("content-type", "image/svg+xml")],
        include_str!("../../../../assets/logo-v2.svg"),
    )
        .into_response()
}

/// The large logo, for a dark page.
pub(crate) async fn logo_large_dark() -> Response {
    (
        [("content-type", "image/svg+xml")],
        include_str!("../../../../assets/logo-v2-dark.svg"),
    )
        .into_response()
}

pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The theme switch (Auto, Light, Dark) of the top bar, on every page and the bare ones.
pub(crate) const THEME_SWITCH: &str = include_str!("../theme.js");

/// The shell's script on every page: theme, navigation by role, the machines in the sidebar,
/// the More menu, toasts, the confirm dialog, forms saved in the background.
pub(crate) const SHELL_SCRIPT: &str = include_str!("../shell.js");

/// The one stylesheet (docs/design/ui.md "The visual system"), from the mockups' `mc.css`.
pub(crate) const STYLE: &str = include_str!("mc.css");

pub(crate) const PAGE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Maker Connect</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{style}</style>
</head>
<body>
{header}
<main id="main" tabindex="-1">
  <div class="head"><h1>Machines</h1><div class="actions"><span class="fw" data-gate="staff" hidden><a class="btn primary" href="machines/add">Add machine</a></span></div></div>
  <div class="banners" data-banners></div>
  <div data-tools hidden>
    <div class="chips" data-chips role="group" aria-label="Filter"></div>
    <div class="row" style="margin-bottom:var(--s-4)"><label class="field" style="margin:0">Search<input type="search" data-search placeholder="Name, model, label, file"></label><label class="field" style="margin:0;flex:0 1 200px">Group by<select data-group><option value="kind">Kind</option><option value="model">Model</option><option value="label">Label</option><option value="none">Not grouped</option></select></label></div>
  </div>
  <div id="machines"><p class="empty">Loading…</p></div>
</main>
<script>
  // The cards of docs/design/ui/mockups/machines.html, live over server-sent events.
  const esc = escapeHtml;
  const KIND = { fdm: "3D printer", laser: "Laser", cnc: "CNC", cutter: "Cutting plotter", label: "Label printer", paper: "Paper printer", scale: "Scale", other: "Machine" };
  const LINK = { virtual: "virtual", "prusa-connect": "built-in Connect", agent: "agent (USB)", simplyprint: "Moonraker" };
  let profiles = {};
  function seen(seconds) {
    if (seconds == null) return "never";
    if (seconds < 60) return `${seconds} s ago`;
    if (seconds < 3600) return `${Math.round(seconds / 60)} min ago`;
    return `${Math.round(seconds / 3600)} h ago`;
  }
  // What someone chose here (filter, search, grouping), kept in this browser only.
  const ui = (() => { try { return { filter: "all", q: "", group: "kind", ...JSON.parse(localStorage.getItem("machine-list") || "{}") }; } catch { return { filter: "all", q: "", group: "kind" }; } })();
  const keep = () => { try { localStorage.setItem("machine-list", JSON.stringify({ filter: ui.filter, group: ui.group })); } catch { /* not kept */ } };
  const BUCKETS = [["all", "All"], ["busy", "Printing"], ["free", "Free"], ["loaded", "Job loaded"], ["needs", "Needs someone"], ["error", "Error"], ["offline", "Offline"], ["out", "Out of use"]];
  function bucket(m) {
    const cls = shownState(m).cls;
    return { busy: "busy", cooling: "busy", idle: "free", loaded: "loaded", paused: "needs", attention: "needs", error: "error", offline: "offline", disconnected: "offline", out: "out" }[cls] || "free";
  }
  const modelOf = (m) => {
    const p = profiles[m.profile];
    return p ? (p.model.includes(p.brand) ? p.model : `${p.brand} ${p.model}`) : KIND[m.kind] || m.kind;
  };
  // What the card says under the kind: the job, else what matters for its kind; offline: since when.
  function says(m) {
    const t = m.telemetry || {};
    if (!m.online) return m.seen_seconds_ago == null ? "Never reported." : `No report since ${seen(m.seen_seconds_ago)}. Its last values are not shown as current.`;
    if (t.job) return [t.job.file, m.loaded?.material].filter(Boolean).join(" · ");
    const parts = [];
    if (t.nozzle_c != null) parts.push(`Nozzle ${t.nozzle_c.toFixed(1)} °C`);
    if (t.bed_c != null) parts.push(`bed ${t.bed_c.toFixed(1)} °C`);
    if (m.kind === "label") parts.push(t.cassette === false ? "no cassette" : m.media ? `Tape ${m.media}` : "tape not set");
    if (t.position_mm) parts.push(`X ${t.position_mm[0].toFixed(1)} · Y ${t.position_mm[1].toFixed(1)}`);
    if (m.loaded) parts.push(loadedText(m.loaded));
    return parts.join(" · ");
  }
  const EDGE = { needs: "edge-wait", error: "edge-bad" };
  // The card (ui.md "The card", after Prusa Connect's printer card in our own drawing): picture,
  // model over name, state; the job and its bar; nozzle, bed, material; a message; the actions.
  const ICON = {
    nozzle: '<svg class="ico" viewBox="0 0 20 20" aria-hidden="true"><path d="M5 3h10v6H5zM7 9h6l-2 5H9z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/><path d="M10 14v3" stroke="currentColor" stroke-width="1.6"/></svg>',
    bed: '<svg class="ico" viewBox="0 0 20 20" aria-hidden="true"><path d="M3 15h14" stroke="currentColor" stroke-width="1.8"/><path d="M6 12c-1-1.5 1-2.5 0-4M10 12c-1-1.5 1-2.5 0-4M14 12c-1-1.5 1-2.5 0-4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/></svg>',
  };
  // Its profile's drawing, else its kind's (crate::profiles), served by the hub.
  function picture(m) {
    const p = profiles[m.profile];
    const src = p?.image ? `profiles/${p.id}.svg` : ["fdm", "laser", "cnc"].includes(m.kind) ? `profiles/_generic/${m.kind}.svg` : null;
    return src ? `<img src="${esc(src)}" alt="" loading="lazy">` : "";
  }
  // "215° / 215°": now, and the target only while one is set.
  const deg = (now, target) => now == null ? "–" : `${Math.round(now)}°${target > 0 ? ` / ${Math.round(target)}°` : ""}`;
  const HEX = /^#[0-9a-fA-F]{3,8}$/;
  function card(m) {
    const t = (m.online && m.telemetry) || {};
    const st = shownState(m), b = bucket(m);
    const go = `machines/${encodeURIComponent(m.id)}`;
    const model = profiles[m.profile] ? modelOf(m) : [KIND[m.kind] || m.kind, LINK[m.link] || m.link].filter(Boolean).join(" · ");
    let job = "";
    if (m.online && t.job) {
      const pct = Math.round((t.job.done * 100) / Math.max(1, t.job.total));
      const left = timeLeft(t.job.remaining_s);
      job = `<div class="mc-job"><span title="${esc(t.job.file)}">${esc(t.job.file)}</span><b class="num">${pct} %</b></div><div class="bar" role="img" aria-label="${pct} percent"><i style="width:${pct}%"></i></div>${left ? `<small class="num">${esc(left)}</small>` : ""}`;
    } else if (m.online) {
      const line = m.kind === "fdm" ? "No print running" : says(m);
      job = line ? `<div class="mc-job"><span>${esc(line)}</span></div>` : "";
    }
    let vals = "";
    if (m.online && m.kind === "fdm") {
      const l = m.loaded;
      const swatch = l?.color && HEX.test(l.color) ? `<span class="swatch" style="background:${l.color}"></span>` : "";
      const material = l ? (l.material || l.name) : t.material;
      vals = `<div class="mc-vals"><span title="Nozzle">${ICON.nozzle}${deg(t.nozzle_c, t.nozzle_target_c)}</span><span title="Bed">${ICON.bed}${deg(t.bed_c, t.bed_target_c)}</span><span title="Material">${swatch}${esc(material || "–")}</span></div>`;
    }
    // What needs someone, in the printer's own words when it says them; offline: since when.
    let msg = "";
    if (!m.online) msg = `<p class="mc-msg"><small>${esc(says(m))}</small></p>`;
    else if (st.title) msg = `<p class="mc-msg"><span class="tag ${st.cls === "error" ? "bad" : "warn"}">${st.cls === "error" ? "Error" : "Attention"}</span><small>${esc(st.title)}</small></p>`;
    const cls = ["card", "mcard", EDGE[b] || "", m.online ? "" : "is-off"].filter(Boolean).join(" ");
    return `<article class="${cls}"><div class="mc-head"><div class="mc-pic">${picture(m)}</div><div class="mc-name"><small>${esc(model)}</small><a href="${go}">${esc(m.name)}</a></div>${stateBadge(m, "span")}</div>
      ${job}${vals}${msg}
      <div class="acts"><span class="fw"><a class="btn small${m.online ? "" : " ghost"}" href="${go}">${m.online ? "Open" : "Look"}</a></span></div></article>${confirmHtml(m, "machines/")}`;
  }
  let all = [];
  function render(machines) {
    all = machines;
    const list = document.getElementById("machines");
    document.querySelector("[data-tools]").hidden = !machines.length;
    if (!machines.length) { document.querySelector("[data-banners]").innerHTML = ""; list.innerHTML = '<div class="empty"><h2>No machines yet</h2><p>Add one with the code it shows.</p><p data-gate="staff"><span class="fw"><a class="btn primary" href="machines/add">Add machine</a></span></p></div>'; return; }
    const counts = Object.fromEntries(BUCKETS.map(([k]) => [k, 0]));
    for (const m of machines) { counts.all++; counts[bucket(m)]++; }
    if (ui.filter !== "all" && !counts[ui.filter]) ui.filter = "all";
    document.querySelector("[data-chips]").innerHTML = BUCKETS.filter(([k]) => k === "all" || counts[k])
      .map(([k, name]) => `<span class="fw"><button type="button" class="chip" data-filter="${k}" aria-pressed="${ui.filter === k}">${name} <b>${counts[k]}</b></button></span>`).join("");
    const shown = machines.filter((m) => (ui.filter === "all" || bucket(m) === ui.filter)
      && (!ui.q.trim() || [m.name, modelOf(m), ...(m.labels || []), m.telemetry?.job?.file || ""].some((t) => t.toLowerCase().includes(ui.q.trim().toLowerCase()))));
    if (!shown.length) { list.innerHTML = '<p class="empty">No machine like that.</p>'; return; }
    const GROUPS = [["fdm", "3D printers"], ["laser", "Lasers"], ["cnc", "CNC machines"], ["cutter", "Cutting plotters"], ["label", "Tape and label printers"], ["paper", "Paper printers"], ["scale", "Scales"], ["other", "Other machines"]];
    let groups;
    if (ui.group === "model") groups = [...new Set(shown.map(modelOf))].sort().map((g) => [g, shown.filter((m) => modelOf(m) === g)]);
    else if (ui.group === "label") {
      const labels = [...new Set(shown.flatMap((m) => m.labels || []))].sort((a, b) => a.localeCompare(b));
      groups = [...labels.map((l) => [l, shown.filter((m) => (m.labels || []).includes(l))]), ["No label", shown.filter((m) => !(m.labels || []).length)]];
    } else if (ui.group === "none") groups = [["", shown]];
    else groups = GROUPS.map(([kind, title]) => [title, shown.filter((m) => (GROUPS.some(([k]) => k === m.kind) ? m.kind : "other") === kind)]);
    list.innerHTML = groups.filter(([, ms]) => ms.length).map(([title, ms], i) =>
      `${title ? `<h2${i ? ' style="margin-top:var(--s-5)"' : ""}>${esc(title)}</h2>` : ""}<div class="grid">${ms.map(card).join("")}</div>`).join("");
  }
  document.querySelector("[data-chips]").addEventListener("click", (e) => {
    const chip = e.target.closest("[data-filter]");
    if (chip) { ui.filter = chip.dataset.filter; keep(); render(all); }
  });
  const search = document.querySelector("[data-search]");
  search.value = ui.q;
  search.addEventListener("input", () => { ui.q = search.value; render(all); });
  const group = document.querySelector("[data-group]");
  group.value = ui.group;
  group.addEventListener("change", () => { ui.group = group.value; keep(); render(all); });
  // The list comes again on every change (server-sent events); the browser reconnects
  // by itself when the connection drops.
  (async () => {
    try { profiles = Object.fromEntries((await (await fetch("api/profiles")).json()).map((p) => [p.id, p])); } catch { /* the kind instead */ }
    const stream = new EventSource("api/machines/stream");
    stream.onmessage = (event) => {
      const machines = JSON.parse(event.data);
      render(machines);
      summarise(machines);
    };
    stream.onopen = () => live(true);
    stream.onerror = () => live(false);
  })();
</script>
</body>
</html>
"##;

/// A page without the navigation (sign in, setup, an invite, welcome): the top bar with the
/// theme switch, and one narrow column. `templates/bare.html`.
#[derive(Template)]
#[template(path = "bare.html")]
pub(crate) struct Bare<'a> {
    pub title: &'a str,
    pub body: &'a str,
}

pub(crate) fn bare_page(title: &str, body: &str) -> String {
    Bare { title, body }
        .render()
        .expect("the bare page renders")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chrome as ui.md draws it, compared with a reviewed copy: `MC_UPDATE_SNAPSHOTS=1`
    /// writes the copy anew (then look at the diff before committing it).
    fn snapshot(name: &str, rendered: &str) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("templates/snapshots")
            .join(name);
        if std::env::var_os("MC_UPDATE_SNAPSHOTS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, rendered).unwrap();
        }
        let kept = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(rendered, kept, "{name} differs from its snapshot");
    }

    #[test]
    fn the_chrome_is_the_designs() {
        // The same in every build: without Prepare and Cameras (their entries are tested apart).
        let chrome = Chrome::with("Workshop <North>", "", "stock", false, false)
            .render()
            .unwrap();
        snapshot("chrome-stock.html", &chrome);
        // Skip to content comes first; the space's name is escaped.
        assert!(chrome.starts_with(r##"<a class="skip" href="#main">Skip to content</a>"##));
        assert!(chrome.contains("<small data-me-space>Workshop &#60;North&#62;</small>"));
        // The theme switch, named with its state.
        assert!(
            chrome
                .contains(r#"class="theme" data-theme-btn aria-label="Theme: Auto">Auto</button>"#)
        );
        // Desktop: the sidebar grouped as the wiki has it, the current page marked.
        let side = &chrome
            [chrome.find(r#"<aside class="side""#).unwrap()..chrome.find("</aside>").unwrap()];
        let order = [
            "Now",
            ">Work<",
            "Machines",
            "Queue and farm",
            ">Stock<",
            "Files",
            "Materials",
            ">Machines<",
            ">Manage<",
            "Space",
            "Your account",
            "Sign out",
        ];
        let mut at = 0;
        for word in order {
            let found = side[at..]
                .find(word)
                .unwrap_or_else(|| panic!("{word} after {at}"));
            at += found + word.len();
        }
        assert!(side.contains(r#"<a href="/stock" aria-current="page">Materials</a>"#));
        assert_eq!(side.matches("aria-current").count(), 1);
        // Staff-only entries are hidden until /api/me says so.
        assert!(side.contains(r#"<a href="/space" data-gate="staff" hidden>Space</a>"#));
        // Phone: four in the bottom bar and More with the rest.
        let bottom = &chrome[chrome.find(r#"<nav class="bottom""#).unwrap()..];
        let first: Vec<&str> = ["Now", "Machines", "Files", "Materials"]
            .into_iter()
            .filter(|w| {
                bottom[..bottom.find("<details").unwrap()].contains(&format!("</span>{w}</a>"))
            })
            .collect();
        assert_eq!(first.len(), 4);
        assert!(
            bottom.contains(
                "<summary><span class=\"ic\" aria-hidden=\"true\">≡</span>More</summary>"
            )
        );
        assert!(bottom[bottom.find("<details").unwrap()..].contains("Queue and farm"));
    }

    #[test]
    fn the_stylesheet_switches_between_desktop_and_phone_and_themes() {
        // Desktop >= 900 px: the sidebar; below: no sidebar, the bottom bar.
        let phone = &STYLE[STYLE.find("@media (max-width: 899px) {").unwrap()..];
        assert!(phone.contains(".side { display: none; }"));
        assert!(phone.contains(".bottom { display: flex;"));
        assert!(STYLE.contains(".bottom { display: none; }"));
        // The house colour, its text shade, the cut corners, light and dark.
        for token in [
            "--accent: #93A682;",
            "--accent-text: #5E6B52;",
            "--cut-l: 12px; --cut-m: 8px; --cut-s: 6px;",
        ] {
            assert!(STYLE.contains(token), "{token}");
        }
        assert!(STYLE.contains(
            "@media (prefers-color-scheme: dark) {\n  :root:not([data-theme=\"light\"]) {"
        ));
        assert!(STYLE.contains(":root[data-theme=\"dark\"] {"));
        // Nothing of the mockups' own: no annotation, no View-as.
        for mock in [".anno", "data-view", ".viewas", ".dialog-mock"] {
            assert!(!STYLE.contains(mock), "{mock}");
        }
    }

    #[test]
    fn a_bare_page_has_the_top_bar_and_no_navigation() {
        let page = bare_page("Sign in", "<p>x</p>");
        assert!(page.contains("<title>Sign in · Maker Connect</title>"));
        assert!(page.contains(r#"<main id="main" tabindex="-1" class="bare">"#));
        assert!(page.contains("data-theme-btn"));
        assert!(!page.contains(r#"class="side""#));
        assert!(!page.contains(r#"class="bottom""#));
    }
}
