//! Now: the home page. What runs, what waits for someone, what ends next, the whole fleet at
//! a glance; and, full-screen with a clock, the wall screen for a TV in the room. From
//! print-farm-manager's dashboard (MIT), made calmer: colour only where someone should look.

use crate::access::Access;
use crate::{STYLE, Shared, escape, failed, header};
use axum::{
    Json, Router,
    extract::{Query, State},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use rusqlite::params;
use serde::Deserialize;

pub fn routes() -> Router<Shared> {
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
async fn summary(access: Access, State(store): State<Shared>, Query(q): Query<Since>) -> Response {
    let store = store.lock().unwrap();
    let counted = store.db.query_row(
        "SELECT COUNT(*) FILTER (WHERE state = 'finished'), COUNT(*) FILTER (WHERE state = 'failed'),
                COUNT(*) FILTER (WHERE state = 'stopped')
         FROM jobs WHERE space_id = ?1 AND ended_at >= ?2",
        params![access.space, q.since],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)),
    );
    match counted {
        Ok((finished, failed, stopped)) => {
            Json(serde_json::json!({ "finished": finished, "failed": failed, "stopped": stopped }))
                .into_response()
        }
        Err(error) => failed(error),
    }
}

async fn page(access: Access, State(store): State<Shared>) -> Response {
    let name = match store.lock().unwrap().space_name(access.space) {
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
<main class="now" data-now>
  <div class="now-head">
    <div><h2>Now</h2><p class="now-space">{title}</p></div>
    <div class="now-clock"><b data-clock>–</b><small data-date></small></div>
    <button type="button" data-wall title="Full screen, for a screen on the wall">Wall screen</button>
  </div>
  <div class="now-numbers">
    <div class="num busy"><b data-n="busy">–</b><span>busy</span></div>
    <div class="num free"><b data-n="free">–</b><span>free</span></div>
    <div class="num needs"><b data-n="needs">–</b><span>need someone</span></div>
    <div class="num done"><b data-n="done">–</b><span>done today</span></div>
  </div>
  <div class="now-grid">
    <section class="now-card">
      <h3>Needs someone</h3>
      <ol class="needs-list" data-needs><li class="clear">Loading…</li></ol>
    </section>
    <section class="now-card">
      <h3>Ends next</h3>
      <ol class="ends-list" data-ends><li class="muted">Loading…</li></ol>
    </section>
  </div>
  <section class="now-card projects-card" data-projects-card hidden>
    <h3>Active projects</h3>
    <div data-active-projects></div>
  </section>
  <section class="now-card fleet-card">
    <h3>The fleet</h3>
    <div class="fleet" data-fleet></div>
    <p class="legend"><i class="cell busy"></i>busy <i class="cell needs"></i>needs someone <i class="cell error"></i>error <i class="cell free"></i>free <i class="cell offline"></i>offline <i class="cell out"></i>out of use</p>
  </section>
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
  .now-head { display: flex; flex-wrap: wrap; align-items: center; gap: 12px 20px; margin-bottom: 18px; }
  .now-head h2 { margin: 0; }
  .now-space { margin: 2px 0 0; color: var(--muted); }
  .now-clock { margin-left: auto; text-align: right; display: grid; }
  .now-clock b { font-size: 34px; font-variant-numeric: tabular-nums; letter-spacing: 1px; }
  .now-clock small { color: var(--muted); }
  /* A quiet button: the page is for looking. */
  [data-wall] { background: transparent; color: var(--ink); border: 1px solid var(--line); }
  [data-wall]:hover { border-color: var(--muted); }
  .now-numbers { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 12px; margin-bottom: 14px; }
  .num { display: flex; align-items: baseline; gap: 12px; padding: 16px 20px; background: var(--card); border: 1px solid var(--line); border-radius: 14px; border-left-width: 4px; }
  .num b { font-size: 46px; line-height: 1; font-variant-numeric: tabular-nums; }
  .num span { color: var(--muted); font-size: 13px; font-weight: 700; text-transform: uppercase; letter-spacing: 1px; }
  .num.busy { border-left-color: var(--busy); } .num.busy b { color: var(--busy); }
  .num.free { border-left-color: var(--accent); } .num.free b { color: var(--accent); }
  .num.needs.some { border-left-color: var(--warn); } .num.needs.some b { color: var(--warn); }
  .num.done b { color: var(--ink); }
  .now-grid { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 12px; margin-bottom: 12px; }
  .now-card { background: var(--card); border: 1px solid var(--line); border-radius: 14px; padding: 16px 20px; }
  .now-card h3 { margin: 0 0 10px; font-size: 13px; color: var(--muted); text-transform: uppercase; letter-spacing: 1px; }
  .needs-list, .ends-list { list-style: none; margin: 0; padding: 0; }
  .needs-list li, .ends-list li { display: grid; grid-template-columns: auto minmax(0, 1fr) auto; gap: 10px; align-items: center; padding: 8px 0; border-top: 1px solid var(--line); }
  .needs-list li:first-child, .ends-list li:first-child { border-top: 0; }
  .needs-list a, .ends-list a { font-weight: 600; text-decoration: none; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .needs-list small, .ends-list small { color: var(--muted); }
  .needs-list .clear { grid-template-columns: 1fr; color: var(--accent); font-weight: 600; }
  .reason { font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: .5px; padding: 2px 8px; border-radius: 5px; color: #fff; background: var(--warn); white-space: nowrap; }
  .reason.error { background: var(--bad); } .reason.offline { background: var(--off); }
  .ends-list .who { grid-column: 2 / -1; margin-top: -6px; font-size: 13px; color: var(--muted); }
  .ends-list .who:empty { display: none; }
  .ends-list .bar { grid-column: 1 / -1; height: 4px; border-radius: 2px; background: color-mix(in srgb, var(--busy) 28%, transparent); overflow: hidden; }
  .ends-list .bar i { display: block; height: 100%; background: var(--busy); }
  .ends-list .clock { font-variant-numeric: tabular-nums; font-weight: 700; }
  .projects-card { margin-bottom: 12px; }
  .np { padding: 8px 0; border-top: 1px solid var(--line); }
  .np:first-child { border-top: 0; padding-top: 0; }
  .np-head { display: flex; align-items: baseline; gap: 8px; margin-bottom: 4px; }
  .np-head a { font-weight: 700; text-decoration: none; }
  .np-part { display: grid; grid-template-columns: minmax(0, 220px) minmax(0, 1fr) auto; gap: 12px; align-items: center; font-size: 14px; padding: 3px 0; }
  .np-part span:first-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .np-bar { display: flex; height: 8px; border-radius: 4px; background: var(--line); overflow: hidden; }
  .np-bar i { display: block; height: 100%; }
  .np-bar .made { background: var(--accent); }
  .np-bar .under { background: color-mix(in srgb, var(--accent) 40%, transparent); }
  .np-count { font-variant-numeric: tabular-nums; color: var(--muted); white-space: nowrap; }
  .np-count b { color: var(--ink); } .np-count .under { color: var(--accent); }
  .np-done { color: var(--accent); font-weight: 700; font-size: 11px; letter-spacing: .5px; }
  .fleet { display: grid; gap: 10px; }
  .fleet-row { display: grid; grid-template-columns: 180px minmax(0, 1fr) auto; gap: 12px; align-items: center; }
  .fleet-row > b { font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cells { display: flex; flex-wrap: wrap; gap: 6px; }
  /* A name too long ends in "…": a block, not a grid, lets it. */
  .cell { display: inline-block; box-sizing: border-box; line-height: 40px; text-align: center; vertical-align: top; width: 84px; height: 40px; border-radius: 7px; font-size: 10px; color: var(--muted); background: var(--line); text-decoration: none; overflow: hidden; padding: 0 4px; text-overflow: ellipsis; white-space: nowrap; }
  .legend .cell { width: 12px; height: 12px; line-height: 12px; border-radius: 3px; margin: 0 4px 0 12px; vertical-align: -1px; }
  .cell span, .cell small { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cell.pct { line-height: 15px; padding-top: 5px; background: linear-gradient(to right, color-mix(in srgb, var(--busy) 70%, var(--card)) var(--pct), color-mix(in srgb, var(--busy) 22%, var(--card)) var(--pct)) !important; }
  .cell.pct small { font-size: 10px; font-weight: 700; font-variant-numeric: tabular-nums; }
  .cell.busy { background: color-mix(in srgb, var(--busy) 30%, var(--card)); color: var(--ink); box-shadow: inset 0 0 0 1.5px var(--busy); }
  .cell.free { background: color-mix(in srgb, var(--accent) 16%, var(--card)); color: var(--ink); box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 55%, transparent); }
  .cell.needs { background: color-mix(in srgb, var(--warn) 30%, var(--card)); color: var(--ink); box-shadow: inset 0 0 0 1.5px var(--warn); }
  .cell.error { background: color-mix(in srgb, var(--bad) 30%, var(--card)); color: var(--ink); box-shadow: inset 0 0 0 1.5px var(--bad); }
  .cell.offline { background: transparent; box-shadow: inset 0 0 0 1px var(--line); opacity: .6; }
  .cell.out { background: transparent; border: 1px dashed var(--off); opacity: .6; }
  .counts { display: flex; gap: 6px; flex-wrap: wrap; justify-content: flex-end; }
  .counts span { font-size: 11px; font-weight: 700; padding: 2px 7px; border-radius: 5px; background: var(--line); color: var(--muted); white-space: nowrap; }
  .legend { margin: 12px 0 0; color: var(--muted); font-size: 13px; }
  .legend .cell:first-child { margin-left: 0; }
  /* The wall screen: the same page, full-screen, larger, without the sidebar. */
  .now:fullscreen { background: var(--bg); padding: 28px 40px; max-width: none; overflow-y: auto; }
  .now:fullscreen .num b { font-size: 72px; }
  .now:fullscreen .now-clock b { font-size: 56px; }
  .now:fullscreen [data-wall] { display: none; }
  @media (max-width: 900px) { .now-numbers { grid-template-columns: repeat(2, minmax(0, 1fr)); } .now-grid { grid-template-columns: 1fr; } .fleet-row { grid-template-columns: 1fr; } .counts { justify-content: flex-start; } }
"#;

const NOW_SCRIPT: &str = r#"
  const esc = (t) => String(t).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  // Each machine in one bucket, as the machine list's chips have them.
  const bucket = (m) => ({ busy: "busy", cooling: "busy", idle: "free", paused: "needs", attention: "needs", error: "error", offline: "offline", disconnected: "offline", out: "out" })[shownState(m).cls] || "free";
  // Who needs someone most: an error, then a print waiting to be taken or attention, paused,
  // offline; within each, the one waiting longest first.
  const URGENCY = { error: 0, needs: 1, offline: 3 };
  let profiles = {};
  const modelOf = (m) => { const p = profiles[m.profile]; return p ? (p.model.includes(p.brand) ? p.model : `${p.brand} ${p.model}`) : m.kind; };
  const ago = (s) => s == null ? "" : s < 60 ? "just now" : s < 3600 ? `${Math.round(s / 60)} min` : `${Math.floor(s / 3600)} h ${String(Math.round((s % 3600) / 60)).padStart(2, "0")} min`;

  function render(machines) {
    const count = { busy: 0, free: 0, needs: 0, error: 0, offline: 0, out: 0 };
    for (const m of machines) count[bucket(m)]++;
    document.querySelector('[data-n="busy"]').textContent = count.busy;
    document.querySelector('[data-n="free"]').textContent = count.free;
    document.querySelector('[data-n="needs"]').textContent = count.needs + count.error;
    document.querySelector(".num.needs").classList.toggle("some", count.needs + count.error > 0);

    // Needs someone: with why, and how long it waits (since its last report while offline).
    const waiting = machines.filter((m) => ["needs", "error", "offline"].includes(bucket(m)))
      .sort((a, b) => (URGENCY[bucket(a)] - URGENCY[bucket(b)]) || ((b.seen_seconds_ago || 0) - (a.seen_seconds_ago || 0)));
    document.querySelector("[data-needs]").innerHTML = waiting.length ? waiting.map((m) => {
      const b = bucket(m), st = shownState(m);
      const why = b === "offline" ? "offline" : b === "error" ? (m.telemetry?.problem || "error") : st.text;
      const since = b === "offline" ? (m.seen_seconds_ago == null ? "never reported" : `${ago(m.seen_seconds_ago)} ago`) : "";
      return `<li><span class="reason ${b}">${esc(why)}</span><a href="machines/${encodeURIComponent(m.id)}">${esc(m.name)}</a><small>${esc(since)}</small></li>`;
    }).join("") : '<li class="clear">✓ All clear</li>';

    // Ends next: what runs, by when it ends.
    const running = machines.filter((m) => m.online && m.telemetry?.job)
      .sort((a, b) => (a.telemetry.job.remaining_s ?? 1e12) - (b.telemetry.job.remaining_s ?? 1e12));
    document.querySelector("[data-ends]").innerHTML = running.length ? running.map((m) => {
      const j = m.telemetry.job, pct = Math.round((j.done * 100) / Math.max(1, j.total));
      // "52 min left · ETA 23:35": the clock in front, the rest under the file.
      const left = timeLeft(j.remaining_s);
      const [leftText, clock] = left ? left.split(" · ETA ") : ["", ""];
      // Who started it: a person (or a token), the queue, someone on the website from the
      // USB stick; else it was started on the printer itself.
      const who = m.job?.by || m.usb_print_by || (m.job ? "" : "on the printer");
      const file = j.file || m.job?.file_name || "";
      const how = [`${pct} %`, leftText, who].filter(Boolean).join(" · ");
      return `<li><span class="clock">${esc(clock || `${pct} %`)}</span><a href="machines/${encodeURIComponent(m.id)}">${esc(m.name)}</a><small>${esc(file)}</small><div class="who">${esc(how)}</div><div class="bar"><i style="width:${pct}%"></i></div></li>`;
    }).join("") : '<li class="muted">Nothing runs now.</li>';

    // The fleet: a row per model, a cell per machine, the counts at the end of the row.
    // A row per device type (3D printers, lasers, label printers…), a cell per machine.
    const TYPES = { fdm: "3D printers", laser: "Lasers", label: "Label printers", cnc: "CNC machines" };
    const kinds = [...new Set(machines.map((m) => m.kind))].sort((a, b) => (Object.keys(TYPES).indexOf(a) + 1 || 99) - (Object.keys(TYPES).indexOf(b) + 1 || 99));
    document.querySelector("[data-fleet]").innerHTML = kinds.map((kind) => {
      const model = TYPES[kind] || kind;
      const ms = machines.filter((m) => m.kind === kind).sort((a, b) => a.name.localeCompare(b.name));
      const tally = {};
      for (const m of ms) tally[bucket(m)] = (tally[bucket(m)] || 0) + 1;
      const names = { busy: "busy", free: "free", needs: "needs someone", error: "error", offline: "offline", out: "out of use" };
      return `<div class="fleet-row"><b title="${esc(model)}">${esc(model)} <small class="muted">×${ms.length}</small></b><div class="cells">${ms.map((m) => {
        // Busy: two blues as far as it got, and how far and when it ends under its name.
        const p = bucket(m) === "busy" ? progressOf(m) : null;
        return `<a class="cell ${bucket(m)}${p ? " pct" : ""}"${p ? ` style="${p.style}"` : ""} href="machines/${encodeURIComponent(m.id)}" title="${esc(m.name)} (${esc(modelOf(m))}): ${esc(p ? `${p.pct} % done${p.eta ? `, ends ${p.eta}` : ""}` : shownState(m).text)}"><span>${esc(m.name)}</span>${p ? `<small>${esc(p.text)}</small>` : ""}</a>`;
      }).join("")}</div><div class="counts">${Object.entries(tally).map(([k, n]) => `<span>${n} ${names[k]}</span>`).join("")}</div></div>`;
    }).join("") || '<p class="muted">No machines yet.</p>';
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
      summarise(machines);
      clearTimeout(soon);
      soon = setTimeout(projects, 1000);
    };
    stream.onopen = () => live(true);
    stream.onerror = () => live(false);
  })();
"#;
