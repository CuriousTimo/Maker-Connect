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
        // An empty link is the page itself: from the root, Now is at "./".
        let home = if root.is_empty() { "/" } else { root };
        let prepare = !crate::laser_design::ASSETS.is_empty();
        let cameras = cfg!(feature = "cameras");
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
            item("Manage", "space", "Space", "space", "staff", ""),
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
        include_str!("../../../../assets/logo-mark.svg"),
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
  <div class="list-tools" data-tools hidden>
    <div class="chips" data-chips role="toolbar" aria-label="Show"></div>
    <div class="list-controls">
      <input type="search" data-search placeholder="Name, model, label, file…" aria-label="Find a machine">
      <select data-group aria-label="Group by"><option value="kind">By kind</option><option value="model">By model</option><option value="label">By label</option><option value="none">Not grouped</option></select>
      <div class="view-switch" role="group" aria-label="View"><button type="button" data-view="cards" title="Cards" aria-label="Cards"><svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="4" width="14" height="5" rx="1.5"/><rect x="3" y="11" width="14" height="5" rx="1.5"/></svg></button><button type="button" data-view="table" title="Table" aria-label="Table"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3 5h14M3 10h14M3 15h14"/></svg></button></div>
    </div>
  </div>
  <div class="banners" data-banners></div>
  <div id="machines"><p class="empty">Loading…</p></div>
</main>
<script>
  const esc = (t) => String(t).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const KIND = { fdm: "3D printer", laser: "Laser", cnc: "CNC", cutter: "Cutting plotter", label: "Label printer", paper: "Paper printer", scale: "Scale", other: "Machine" };
  let profiles = null;
  // A column of a machine's row: a value, and what it is under it.
  const col = (cls, value, caption) => `<div class="m-col ${cls}"><b>${esc(value ?? "–")}</b><small>${esc(caption ?? "")}</small></div>`;
  const LINK = { virtual: "Virtual", "prusa-connect": "Prusa Connect", agent: "Agent", simplyprint: "Moonraker" };
  // What it does now: its job, else what matters for its kind.
  function now(m, t) {
    if (t.job) {
      const percent = Math.round((t.job.done * 100) / Math.max(1, t.job.total));
      return [`${percent} % · ${t.job.file}`, timeLeft(t.job.remaining_s) || "printing"];
    }
    if (t.nozzle_c != null) return [`${t.nozzle_c.toFixed(0)} °C · ${t.bed_c != null ? `${t.bed_c.toFixed(0)} °C` : "–"}`, "nozzle · bed"];
    if (m.kind === "label") {
      const tape = t.cassette === false ? "no cassette" : m.media ? `tape ${m.media}` : "tape not set";
      const pct = t.battery_percent ?? null;
      return [`${tape}${pct != null ? ` · ${pct} %` : ""}`, pct != null ? "tape · battery" : "tape"];
    }
    if (t.position_mm) return [`X ${t.position_mm[0].toFixed(1)} · Y ${t.position_mm[1].toFixed(1)}`, "head, mm"];
    return [null, "nothing reported"];
  }
  // Said only when something is off: offline (since when), or late to report.
  function trouble(m) {
    if (m.out_of_use) return `<div class="m-col m-conn"></div>`;
    if (!m.online) return col("m-conn", "Offline", m.seen_seconds_ago == null ? "never reported" : `last seen ${seen(m.seen_seconds_ago)}`);
    if (m.seen_seconds_ago != null && m.seen_seconds_ago > 30) return col("m-conn", "Late", `last report ${seen(m.seen_seconds_ago)}`);
    return `<div class="m-col m-conn"></div>`;
  }
  function seen(seconds) {
    if (seconds == null) return "never";
    if (seconds < 60) return `${seconds} s ago`;
    if (seconds < 3600) return `${Math.round(seconds / 60)} min ago`;
    return `${Math.round(seconds / 3600)} h ago`;
  }
  // What someone chose here (filter, search, grouping, view), kept in this browser only.
  const ui = (() => { try { return { filter: "all", q: "", group: "kind", view: "cards", ...JSON.parse(localStorage.getItem("machine-list") || "{}") }; } catch { return { filter: "all", q: "", group: "kind", view: "cards" }; } })();
  const keep = () => { try { localStorage.setItem("machine-list", JSON.stringify({ filter: ui.filter, group: ui.group, view: ui.view })); } catch { /* not kept */ } };
  // Each machine in one bucket, for the chips: what someone would look for.
  const BUCKETS = [["all", "All"], ["busy", "Busy"], ["free", "Free"], ["needs", "Needs someone"], ["error", "Error"], ["offline", "Offline"], ["out", "Out of use"]];
  function bucket(m) {
    const cls = shownState(m).cls;
    return { busy: "busy", cooling: "busy", idle: "free", paused: "needs", attention: "needs", error: "error", offline: "offline", disconnected: "offline", out: "out" }[cls] || "free";
  }
  const modelOf = (m) => {
    const p = profiles[m.profile];
    return p ? (p.model.includes(p.brand) ? p.model : `${p.brand} ${p.model}`) : KIND[m.kind] || m.kind;
  };
  let all = [];
  function render(machines) {
      all = machines;
      const list = document.getElementById("machines");
      document.querySelector("[data-tools]").hidden = !machines.length;
      if (!machines.length) { document.querySelector("[data-banners]").innerHTML = ""; list.innerHTML = '<p class="empty">No machines yet. <a href="#add" data-add-open>Add one</a>: with the code its agent shows, or a virtual one to try everything.</p>'; return; }
      // The chips, with their counts; one with nothing in it does not show (All always does).
      const counts = Object.fromEntries(BUCKETS.map(([k]) => [k, 0]));
      for (const m of machines) { counts.all++; counts[bucket(m)]++; }
      if (ui.filter !== "all" && !counts[ui.filter]) ui.filter = "all";
      document.querySelector("[data-chips]").innerHTML = BUCKETS.filter(([k]) => k === "all" || counts[k])
        .map(([k, name]) => `<button type="button" class="chip ${k}" data-filter="${k}" aria-pressed="${ui.filter === k}">${name} <b>${counts[k]}</b></button>`).join("");
      // One banner per kind of trouble, its machines named; nothing when all is well.
      const named = (ms) => ms.map((m) => `<a href="machines/${encodeURIComponent(m.id)}">${esc(m.name)}</a>${shownState(m).text && bucket(m) === "needs" ? ` <small>(${esc(shownState(m).text.toLowerCase())})</small>` : ""}`).join(", ");
      const by = (b) => machines.filter((m) => bucket(m) === b);
      const banners = [];
      const needs = by("needs"), errors = by("error"), gone = by("offline");
      if (needs.length) banners.push(`<div class="banner needs"><b>${needs.length === 1 ? "1 machine needs" : `${needs.length} machines need`} someone</b> ${named(needs)}</div>`);
      if (errors.length) banners.push(`<div class="banner error"><b>${errors.length === 1 ? "1 machine reports" : `${errors.length} machines report`} an error</b> ${errors.map((m) => `<a href="machines/${encodeURIComponent(m.id)}">${esc(m.name)}</a>${m.telemetry?.problem ? ` <small>(${esc(m.telemetry.problem)})</small>` : ""}`).join(", ")}</div>`);
      if (gone.length) banners.push(`<div class="banner offline"><b>${gone.length} offline</b> ${gone.map((m) => `<a href="machines/${encodeURIComponent(m.id)}">${esc(m.name)}</a> <small>(${m.seen_seconds_ago == null ? "never reported" : `seen ${seen(m.seen_seconds_ago)}`})</small>`).join(", ")}</div>`);
      document.querySelector("[data-banners]").innerHTML = banners.join("");
      // What is shown: the chip, then the search (name, model, labels, the file it runs).
      const q = ui.q.trim().toLowerCase();
      const shown = machines.filter((m) => (ui.filter === "all" || bucket(m) === ui.filter)
        && (!q || [m.name, modelOf(m), ...(m.labels || []), m.telemetry?.job?.file || ""].some((t) => t.toLowerCase().includes(q))));
      if (!shown.length) { list.innerHTML = '<p class="empty">No machine like that.</p>'; return; }
      const chips = (m) => [...(m.labels || []).map((l) => `<span class="tag">${esc(l)}</span>`), m.queue?.length ? `<span class="tag" title="Waiting to print here, one after another">${m.queue.length} in queue</span>` : "", m.induction_required ? (m.training ? '<span class="tag induction" title="Only who had the training may use it">training</span>' : '<span class="tag induction" title="Only who has permission may use it">permission</span>') : ""].join("");
      const card = (m) => {
        const t = m.telemetry || {};
        const shownS = shownState(m);
        const percent = t.job ? Math.round((t.job.done * 100) / Math.max(1, t.job.total)) : null;
        const bar = t.job ? `<div class="card-progress"><i style="width: ${percent}%"></i></div>` : "";
        const [doing, what] = now(m, t);
        const profile = profiles[m.profile];
        const thumb = profile && profile.image ? `<img class="thumb" src="profiles/${esc(m.profile)}.svg" alt="">`
          : ["fdm", "laser", "cnc"].includes(m.kind) ? `<img class="thumb" src="profiles/_generic/${esc(m.kind)}.svg" alt="">` : `<span class="thumb"></span>`;
        // The model under the name, unless the name says it already; how it connects with it.
        const model = modelOf(m);
        const under = [m.name.toLowerCase().includes(model.toLowerCase()) ? null : model, LINK[m.link] || m.link].filter(Boolean).join(" · ");
        const tags = chips(m);
        return `<a class="machine" href="machines/${encodeURIComponent(m.id)}">
          ${thumb}
          <div class="m-col"><b class="name">${esc(m.name)}</b><small>${esc(under)}</small>${m.loaded ? `<small class="m-spool"><i class="m-swatch" style="background:${esc(m.loaded.color || "transparent")}"></i>${esc(loadedText(m.loaded))}</small>` : ""}${tags ? `<span class="tags">${tags}</span>` : ""}</div>
          ${col("m-now", doing, what)}
          ${trouble(m)}
          ${stateBadge(m)}
          ${bar}
        </a>`;
      };
      // The table, for many machines: one line each, the same columns for every kind.
      const row = (m) => {
        const t = m.telemetry || {};
        const st = shownState(m);
        const percent = t.job ? Math.round((t.job.done * 100) / Math.max(1, t.job.total)) : null;
        const go = `machines/${encodeURIComponent(m.id)}`;
        return `<tr data-go="${esc(go)}"><td><a href="${esc(go)}"><b>${esc(m.name)}</b></a></td><td>${stateBadge(m, "span")}</td><td>${esc(modelOf(m))}</td>
          <td>${m.loaded ? `<span class="m-spool"><i class="m-swatch" style="background:${esc(m.loaded.color || "transparent")}"></i>${esc(m.loaded.material || m.loaded.name)}</span>` : '<span class="muted">–</span>'}</td>
          <td>${t.job ? `<div class="t-progress"><i style="width:${percent}%"></i></div><small>${percent} % · ${esc(t.job.file)}</small>` : '<span class="muted">–</span>'}</td>
          <td>${t.job && t.job.remaining_s != null ? esc(timeLeft(t.job.remaining_s).split(" · ")[1] || "") : '<span class="muted">–</span>'}</td>
          <td>${chips(m) || '<span class="muted">–</span>'}</td></tr>`;
      };
      // Grouped: by kind (as it was), model or label (a machine with two labels is in both).
      const GROUPS = [["fdm", "3D printers"], ["laser", "Lasers"], ["cnc", "CNC machines"], ["cutter", "Cutting plotters"], ["label", "Label printers"], ["paper", "Paper printers"], ["scale", "Scales"], ["other", "Other machines"]];
      let groups;
      if (ui.group === "model") {
        const models = [...new Set(shown.map(modelOf))].sort();
        groups = models.map((g) => [g, shown.filter((m) => modelOf(m) === g)]);
      } else if (ui.group === "label") {
        const labels = [...new Set(shown.flatMap((m) => m.labels || []))].sort((a, b) => a.localeCompare(b));
        groups = [...labels.map((l) => [l, shown.filter((m) => (m.labels || []).includes(l))]), ["No label", shown.filter((m) => !(m.labels || []).length)]];
      } else if (ui.group === "none") {
        groups = [["", shown]];
      } else {
        groups = GROUPS.map(([kind, title]) => [title, shown.filter((m) => (GROUPS.some(([k]) => k === m.kind) ? m.kind : "other") === kind)]);
      }
      groups = groups.filter(([, ms]) => ms.length);
      for (const b of document.querySelectorAll("[data-view]")) b.setAttribute("aria-pressed", String(b.dataset.view === ui.view));
      list.innerHTML = groups.map(([title, ms]) => {
        const head = title ? `<h2 class="group">${esc(title)} <span>${ms.length}</span></h2>` : "";
        return ui.view === "table"
          ? `${head}<div class="table-wrap"><table class="machine-table"><thead><tr><th>Machine</th><th>State</th><th>Model</th><th>Material</th><th>Progress</th><th>Done at</th><th>Labels</th></tr></thead><tbody>${ms.map(row).join("")}</tbody></table></div>`
          : `${head}${ms.map((m) => card(m) + confirmHtml(m, "machines/")).join("")}`;
      }).join("");
  }
  // The tools: a chip, the search, the grouping, the view.
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
  document.querySelector(".view-switch").addEventListener("click", (e) => {
    const b = e.target.closest("[data-view]");
    if (b) { ui.view = b.dataset.view; keep(); render(all); }
  });
  // A row of the table opens its machine, as a card does.
  document.getElementById("machines").addEventListener("click", (e) => {
    const tr = e.target.closest("tr[data-go]");
    if (tr && !e.target.closest("a")) location.href = tr.dataset.go;
  });
  // The list comes again on every change (server-sent events); the browser reconnects
  // by itself when the connection drops.
  (async () => {
    try { profiles = Object.fromEntries((await (await fetch("api/profiles")).json()).map((p) => [p.id, p])); } catch (error) { profiles = {}; }
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
