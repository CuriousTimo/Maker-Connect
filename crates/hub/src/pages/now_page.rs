//! Now: the home page. What runs, what waits for someone, what ends next, the whole fleet at
//! a glance; and, full-screen with a clock, the wall screen for a TV in the room. From
//! print-farm-manager's dashboard (MIT), made calmer: colour only where someone should look.

use crate::access::Access;
use crate::{STYLE, Store, escape, failed, header};
use axum::{
    Json, Router,
    extract::{Query, State},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/", get(page))
        .route("/api/jobs/summary", get(summary))
}

#[derive(Deserialize)]
struct Since {
    /// Unix seconds: the page sends its own midnight.
    since: i64,
}

/// How the space's jobs ended since a moment: for "done today".
async fn summary(access: Access, State(store): State<Store>, Query(q): Query<Since>) -> Response {
    let counted = store.ended_since(access.space, q.since).await;
    match counted {
        Ok((finished, failed, stopped)) => {
            Json(serde_json::json!({ "finished": finished, "failed": failed, "stopped": stopped }))
                .into_response()
        }
        Err(error) => failed(error),
    }
}

async fn page(access: Access, State(store): State<Store>) -> Response {
    let name = match store.space_name(access.space).await {
        Ok(name) => name,
        Err(error) => return failed(error),
    };
    Html(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Now · {title}</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{STYLE}{NOW_STYLE}
</style>
</head>
<body>
{header}
<main id="main" tabindex="-1" class="now" data-now>
  <div class="now-head">
    <h1>Now</h1><b class="now-clock" data-clock>–</b>
    <p class="now-lead" data-lead>Loading…</p><small class="now-clock" data-date></small>
  </div>
  <div class="needs" data-needs></div>
  <div class="tiles" data-tiles><p class="muted">Loading…</p></div>
  <section class="now-card card projects-card" data-projects-card hidden>
    <h3>Active projects</h3>
    <div data-active-projects></div>
  </section>
  <div class="now-strip">
    <span><b data-n="done">–</b>done today</span>
    <span><b data-n="next">–</b>next one ends</span>
    <span class="now-space">{title}</span>
    <span class="fw"><button type="button" class="btn small" data-wall title="Full screen, for a screen on the wall">Wall screen</button></span>
  </div>
</main>
<script>
{NOW_SCRIPT}
</script>
</body>
</html>"##,
        title = escape(&name),
        header = header(&name, "", "now"),
    ))
    .into_response()
}

const NOW_STYLE: &str = r#"
  /* Now: one sentence, what needs someone, then the workshop itself: a tile per machine. */
  /* The time stands on the title's line and the date on the sentence's: two rows, on their baselines. */
  .now-head { display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: baseline; gap: 6px 24px; margin-bottom: 22px; }
  .now-head h1 { margin: 0; }
  .now-lead { margin: 0; font-size: 18px; color: var(--muted); }
  .now-lead b { color: var(--ink); }
  .now-clock { text-align: right; color: var(--muted); font-size: var(--fs-2); }
  b.now-clock { font-size: var(--fs-5); line-height: 1; color: var(--ink); font-variant-numeric: tabular-nums; }
  .needs { display: grid; gap: 8px; margin-bottom: 22px; }
  .needs:empty { display: none; }
  .need { --cut: var(--cut-l); clip-path: polygon(0 0, calc(100% - var(--cut)) 0, 100% var(--cut), 100% 100%, 0 100%); display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 14px; align-items: center; padding: 14px 18px; background: var(--surface); box-shadow: inset 5px 0 0 var(--c); --c: var(--st-warn); }
  .need.error { --c: var(--st-bad); }
  .need .why { display: block; font-size: 11px; font-weight: 800; letter-spacing: .08em; text-transform: uppercase; color: var(--c); }
  .need b { font-size: 16px; }
  .tiles { display: grid; grid-template-columns: repeat(auto-fill, minmax(250px, 1fr)); gap: 14px; margin-bottom: 22px; }
  .tiles .kind { grid-column: 1 / -1; display: flex; align-items: baseline; gap: 10px; margin: 14px 0 -4px; }
  .tiles .kind:first-child { margin-top: 0; }
  .tiles .kind h2 { margin: 0; }
  /* A tile: the outline in the state's colour (as every shape of the house), the machine, its state in large words, what it does. */
  .m { --cut: var(--cut-l); clip-path: polygon(0 0, calc(100% - var(--cut)) 0, 100% var(--cut), 100% 100%, 0 100%); display: grid; grid-template-rows: 1fr auto auto; min-height: 168px; background: var(--surface); color: inherit; text-decoration: none; --c: var(--line-strong); --ct: var(--ink); }
  .m .in { padding: 14px 16px 10px; display: grid; gap: 4px; align-content: start; min-width: 0; }
  .m .who { display: flex; align-items: center; gap: 10px; min-width: 0; }
  .m .who > span:last-child { min-width: 0; }
  .m .glyph { width: 40px; height: 40px; background: var(--surface-2); display: grid; place-items: center; flex: none; color: var(--muted); }
  .m .glyph svg { width: 24px; height: 24px; fill: none; stroke: currentColor; stroke-width: 1.8; }
  .m .name { display: block; font-weight: 700; font-size: 16px; line-height: 1.2; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .m .model { display: block; font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .m .state { margin-top: 10px; font-size: 22px; font-weight: 800; letter-spacing: -.01em; line-height: 1.15; color: var(--ct); font-variant-numeric: tabular-nums; }
  .m .sub { font-size: 13px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .m .bar { height: 8px; margin: 0 16px 10px; background: var(--surface-2); }
  .m .bar i { display: block; height: 100%; background: var(--st-info); }
  .m .foot { display: flex; justify-content: space-between; gap: 8px; padding: 0 16px 14px; font-size: 13px; color: var(--muted); }
  .m.free { --c: var(--accent); --ct: var(--accent-text); }
  .m.busy { --c: var(--st-info); }
  .m.needs { --c: var(--st-warn); --ct: var(--st-warn); }
  .m.error { --c: var(--st-bad); --ct: var(--st-bad); }
  /* Offline and out of use: no surface, an outline (dashed when out of use), quiet words. */
  /* The outline as every shape of the house (mc.css): two layers, the inner cut 0.5858 x the outline
     smaller, so the diagonal is as thick as the straight edges; dashed with --ring, the diagonal too. */
  .m, .need { --bw: 1px; border: var(--bw) solid transparent; background: linear-gradient(225deg, transparent calc((var(--cut) - var(--bw) * .5858) * .7071), var(--fill, var(--surface)) calc((var(--cut) - var(--bw) * .5858) * .7071 + 1px)) padding-box, var(--ring, var(--bc, var(--line))) border-box; }
  .m { --bc: var(--c); }
  .m.offline, .m.out { --ct: var(--muted); --bc: var(--line-strong); }
  .m.offline .state, .m.out .state { font-weight: 700; }

  .m.out { --ring: repeating-linear-gradient(135deg, var(--bc) 0 3px, transparent 3px 6px); }
  .m:hover, .m:focus-visible { --bw: 2px; --bc: var(--accent); --ring: none; }
  .now-strip { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 28px; padding-top: 16px; border-top: 1px solid var(--line); color: var(--muted); font-size: 14px; }
  .now-strip b { color: var(--ink); font-size: 18px; margin-right: 6px; font-variant-numeric: tabular-nums; }
  .now-strip .fw { margin-left: auto; }
  .now-card { padding: 16px 20px; }
  .now-card h3 { margin: 0 0 10px; font-size: 13px; color: var(--muted); text-transform: uppercase; letter-spacing: 1px; }
  .projects-card { margin-bottom: 22px; }
  .np { padding: 8px 0; border-top: 1px solid var(--line); }
  .np:first-child { border-top: 0; padding-top: 0; }
  .np-head { display: flex; align-items: baseline; gap: 8px; margin-bottom: 4px; }
  .np-head a { font-weight: 700; text-decoration: none; }
  .np-part { display: grid; grid-template-columns: minmax(0, 220px) minmax(0, 1fr) auto; gap: 12px; align-items: center; font-size: 14px; padding: 3px 0; }
  .np-part span:first-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .np-bar { display: flex; height: 8px; border-radius: 0; background: var(--line); overflow: hidden; }
  .np-bar i { display: block; height: 100%; }
  .np-bar .made { background: var(--accent); }
  .np-bar .under { background: color-mix(in srgb, var(--accent) 40%, transparent); }
  .np-count { font-variant-numeric: tabular-nums; color: var(--muted); white-space: nowrap; }
  .np-count b { color: var(--ink); } .np-count .under { color: var(--accent-text); }
  .np-done { color: var(--accent-text); font-weight: 700; font-size: 11px; letter-spacing: .5px; }
  /* The wall screen: the same page, full-screen, larger, without the rail. */
  .now:fullscreen { background: var(--bg); padding: 28px 40px; max-width: none; overflow-y: auto; }
  .now:fullscreen .now-lead { font-size: 28px; }
  .now:fullscreen b.now-clock { font-size: 56px; }
  .now:fullscreen .tiles { grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); }
  .now:fullscreen .m .state { font-size: 30px; }
  .now:fullscreen .fw { display: none; }
  /* A phone has its own clock. The wall screen keeps it. */
  @media (max-width: 600px) { .now:not(:fullscreen) .now-clock { display: none; } .now-head { grid-template-columns: 1fr; } .need { grid-template-columns: 1fr; } .tiles { grid-template-columns: 1fr; } .m { min-height: 0; } }
"#;

const NOW_SCRIPT: &str = r#"
  const esc = (t) => String(t).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  // Each machine in one bucket, as the machine list's chips have them.
  const bucket = (m) => ({ busy: "busy", cooling: "busy", idle: "free", paused: "needs", attention: "needs", error: "error", offline: "offline", disconnected: "offline", out: "out" })[shownState(m).cls] || "free";
  let profiles = {};
  const modelOf = (m) => { const p = profiles[m.profile]; return p ? (p.model.includes(p.brand) ? p.model : `${p.brand} ${p.model}`) : ONE[m.kind] || m.kind; };
  const ago = (s) => s == null ? "" : s < 60 ? "just now" : s < 3600 ? `${Math.round(s / 60)} min` : s < 36000 ? `${Math.floor(s / 3600)} h ${String(Math.floor((s % 3600) / 60)).padStart(2, "0")} min` : s < 172800 ? `${Math.round(s / 3600)} h` : `${Math.round(s / 86400)} days`;
  const TYPES = { fdm: "3D printers", laser: "Lasers", label: "Label printers", cnc: "CNC machines" };
  const ONE = { fdm: "3D printer", laser: "Laser", label: "Label printer", cnc: "CNC machine" };
  const GLYPH = {
    fdm: "M4 3h16v18H4zM4 17h16M9 3v6h6V3M12 9v3",
    laser: "M3 6h18M7 6v5h10V6M12 11v5M9 20l3-4 3 4",
    label: "M5 4h14v9H5zM8 13v7h8v-7M10 16h4",
  };
  const glyph = (kind) => `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="${GLYPH[kind] || "M4 4h16v16H4zM4 15h16M9 9h6"}"/></svg>`;
  const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;

  function render(machines) {
    const count = { busy: 0, free: 0, needs: 0, error: 0, offline: 0, out: 0 };
    for (const m of machines) count[bucket(m)]++;
    const href = (m) => `machines/${encodeURIComponent(m.id)}`;

    // One sentence: who needs someone first; else that nothing does.
    const call = count.needs + count.error;
    const rest = [count.busy && `${count.busy} busy`, count.free && `${count.free} free`, count.offline && `${count.offline} offline`, count.out && `${count.out} out of use`].filter(Boolean).join(", ");
    document.querySelector("[data-lead]").innerHTML = machines.length
      ? `<b>${call ? `${plural(call, "machine needs", "machines need")} attention.` : "Nothing needs attention."}</b> ${esc(rest)}${rest ? "." : ""}`
      : "No machines yet.";

    // Needs someone: an error first, then the one waiting longest; each with why and the way there.
    const urgent = machines.filter((m) => ["needs", "error"].includes(bucket(m)))
      .sort((a, b) => ((bucket(a) === "error" ? 0 : 1) - (bucket(b) === "error" ? 0 : 1)) || ((b.seen_seconds_ago || 0) - (a.seen_seconds_ago || 0)));
    document.querySelector("[data-needs]").innerHTML = urgent.map((m) => {
      const b = bucket(m);
      const why = b === "error" ? (m.telemetry?.problem || "Error") : shownState(m).text;
      return `<div class="need ${b}"><span><span class="why">${b === "error" ? "Error" : "Needs someone"}</span><b>${esc(m.name)}: ${esc(why)}</b></span><span class="fw"><a class="btn primary" href="${href(m)}">Go to it</a></span></div>`;
    }).join("");

    // The workshop: a tile per machine, by kind; its state in words, and what it is doing.
    const kinds = [...new Set(machines.map((m) => m.kind))].sort((a, b) => (Object.keys(TYPES).indexOf(a) + 1 || 99) - (Object.keys(TYPES).indexOf(b) + 1 || 99));
    document.querySelector("[data-tiles]").innerHTML = kinds.map((kind) => {
      const ms = machines.filter((m) => m.kind === kind).sort((a, b) => a.name.localeCompare(b.name));
      return `<div class="kind"><h2>${esc(TYPES[kind] || kind)}</h2><span class="muted">${ms.length}</span></div>` + ms.map((m) => {
        const b = bucket(m), st = shownState(m), p = b === "busy" ? progressOf(m) : null, j = m.online ? m.telemetry?.job : null;
        const state = p ? `${p.pct} %` : b === "error" ? (m.telemetry?.problem || st.text) : b === "free" ? "Free" : st.text;
        // Who started it: a person (or a token), the queue, someone on the website from the
        // USB stick; else it was started on the printer itself.
        const who = j ? (m.job?.by || m.usb_print_by || (m.job ? "" : "on the printer")) : "";
        const sub = j ? [j.file || m.job?.file_name || "", who].filter(Boolean).join(" · ")
          : b === "offline" ? (m.seen_seconds_ago == null ? "Never reported" : `Last seen ${ago(m.seen_seconds_ago)} ago`)
          : b === "free" ? "Ready" : "";
        const left = p && j ? timeLeft(j.remaining_s) : "";
        const foot = left ? left.replace(" · ETA ", ", ends ") : "";
        return `<a class="m ${b}" href="${href(m)}"><span class="in"><span class="who"><span class="glyph">${glyph(m.kind)}</span><span><span class="name">${esc(m.name)}</span><span class="model">${esc(modelOf(m))}</span></span></span><span class="state">${esc(state)}</span><span class="sub" title="${esc(sub)}">${esc(sub)}</span></span>${p ? `<span class="bar"><i style="width:${p.pct}%"></i></span>` : "<span></span>"}<span class="foot"><span>${esc(foot)}</span><span aria-hidden="true">Open →</span></span></a>`;
      }).join("");
    }).join("") || '<p class="muted">No machines yet.</p>';

    // Next one ends: the clock of the print that ends first.
    const next = machines.filter((m) => bucket(m) === "busy" && progressOf(m)?.eta)
      .sort((a, b) => a.telemetry.job.remaining_s - b.telemetry.job.remaining_s)[0];
    document.querySelector('[data-n="next"]').textContent = next ? progressOf(next).eta : "–";
  }

  // Active projects: each part as made, on a bed now, still to make; by priority.
  async function projects() {
    let data;
    try { data = await (await fetch("api/projects")).json(); } catch { return; }
    const active = data.projects.filter((p) => p.state === "active");
    document.querySelector("[data-projects-card]").hidden = !active.length;
    document.querySelector("[data-active-projects]").innerHTML = active.map((pr) => `<div class="np"><div class="np-head"><a href="projects">${esc(pr.name)}</a></div>${pr.parts.map((p) => {
      const pct = (n) => Math.min(100, (n * 100) / Math.max(1, p.target));
      const made = Math.min(p.done, p.target);
      return `<div class="np-part"><span title="${esc(p.name)}">${esc(p.name)}</span><div class="np-bar"><i class="made" style="width:${pct(made)}%"></i><i class="under" style="width:${pct(Math.min(p.parts_under_way, Math.max(0, p.target - made)))}%"></i></div><span class="np-count">${p.done >= p.target ? '<span class="np-done">DONE</span> ' : ""}<b>${p.done}</b>${p.parts_under_way ? ` <span class="under">+${p.parts_under_way}</span>` : ""} / ${p.target}</span></div>`;
    }).join("")}</div>`).join("");
  }
  let soon = null;

  // Done today: jobs that ended well since this browser's midnight, now and then.
  async function today() {
    const midnight = new Date(); midnight.setHours(0, 0, 0, 0);
    try {
      const r = await (await fetch(`api/jobs/summary?since=${Math.floor(midnight / 1000)}`)).json();
      document.querySelector('[data-n="done"]').textContent = r.finished;
    } catch { /* next time */ }
  }

  // The clock.
  function tick() {
    const now = new Date();
    document.querySelector("[data-clock]").textContent = now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
    document.querySelector("[data-date]").textContent = now.toLocaleDateString([], { weekday: "long", day: "numeric", month: "long" });
  }
  tick(); setInterval(tick, 1000);
  today(); setInterval(today, 60000);

  // The wall screen: this page, full-screen.
  document.querySelector("[data-wall]").addEventListener("click", () => document.querySelector("[data-now]").requestFullscreen?.());

  (async () => {
    try { profiles = Object.fromEntries((await (await fetch("api/profiles")).json()).map((p) => [p.id, p])); } catch { profiles = {}; }
    const stream = new EventSource("api/machines/stream");
    projects();
    stream.onmessage = (event) => {
      const machines = JSON.parse(event.data);
      render(machines);
      clearTimeout(soon);
      soon = setTimeout(projects, 1000);
    };
  })();
"#;
