//! The Projects page: projects of parts, made over many machines (crate::farm). Each part's
//! progress (made, on a bed now, still to make), its file per machine model, why it waits;
//! which machines take work from the queue.

use crate::access::Access;
use crate::{STYLE, Shared, escape, failed, header};
use axum::{
    Router,
    extract::State,
    response::{Html, IntoResponse, Response},
    routing::get,
};

pub fn routes() -> Router<Shared> {
    Router::new().route("/projects", get(page))
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
<title>Projects · {title}</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{STYLE}{PROJECTS_STYLE}
</style>
</head>
<body>
{header}
<main>
  <div class="p-head">
    <div><h2>Projects</h2><p class="intro">Parts to make, many machines making them: a free machine that suits a part takes its next plate; someone confirms what came off the bed.</p></div>
    <form class="p-new" data-new-project><input name="name" required maxlength="80" placeholder="Brackets for the hall" aria-label="Name of a new project"><button>New project</button></form>
  </div>
  <section class="p-schedule" data-schedule-box hidden>
    <h3 data-schedule-title>Ahead</h3>
    <p class="intro">What each machine would print, if nothing changes: the queue's own rule, worked out ahead. A plate waiting to be confirmed holds its machine.</p>
    <div class="tl" data-schedule></div>
  </section>
  <div data-projects><p class="empty">Loading…</p></div>
  <datalist id="sheet-kinds"><option value="Smooth PEI"><option value="Textured PEI"><option value="Satin"><option value="Glass"><option value="Garolite"><option value="PEO"></datalist>
  <section class="p-machines">
    <h3>Machines that take work</h3>
    <p class="intro">Only these get plates from the queue; the others are used by hand.</p>
    <div class="p-machine-list" data-machines></div>
  </section>
</main>
<dialog class="p-dialog" data-ledger-dialog><div class="dialog-head"><h2 data-ledger-title>Ledger</h2><button type="button" class="close" data-close aria-label="Close">✕</button></div><section data-ledger-body></section></dialog>
<script>
{PROJECTS_SCRIPT}
</script>
</body>
</html>"##,
        title = escape(&name),
        header = header(&name, "", "projects"),
    ))
    .into_response()
}

const PROJECTS_STYLE: &str = r#"
  .p-head { display: flex; gap: 20px; align-items: flex-start; justify-content: space-between; flex-wrap: wrap; margin-bottom: 16px; }
  .p-head h2 { margin: 0 0 4px; }
  .intro { color: var(--muted); margin: 0; max-width: 640px; font-size: 14px; }
  .p-new { margin: 0; }
  .project { background: var(--card); border: 1px solid var(--line); border-radius: 14px; padding: 16px 20px; margin-bottom: 14px; }
  .project.draft, .project.done { opacity: .85; }
  .project-head { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
  .project-head h3 { margin: 0; font-size: 18px; }
  .p-state { font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: .5px; padding: 2px 8px; border-radius: 5px; color: var(--muted); box-shadow: inset 0 0 0 1.5px var(--line); }
  .p-state.active { background: var(--accent); color: #fff; box-shadow: none; }
  .p-state.paused { color: var(--warn); box-shadow: inset 0 0 0 1.5px var(--warn); }
  .p-actions { margin-left: auto; display: flex; gap: 6px; flex-wrap: wrap; }
  .p-actions button, .part-tools button, .pf button { padding: 4px 10px; font-size: 13px; }
  .ghost { background: transparent; color: var(--ink); border: 1px solid var(--line); }
  .danger { background: transparent; color: var(--bad); border: 1px solid color-mix(in srgb, var(--bad) 40%, transparent); }
  .part { padding: 12px 0; border-top: 1px solid var(--line); }
  .part:first-of-type { margin-top: 12px; }
  .part-line { display: flex; align-items: baseline; gap: 10px; }
  .part-line b { font-size: 15px; }
  .part-count { margin-left: auto; font-variant-numeric: tabular-nums; color: var(--muted); }
  .part-count b { color: var(--ink); }
  .part-count .under { color: var(--accent); }
  .p-bar { display: flex; height: 8px; border-radius: 4px; background: var(--line); overflow: hidden; margin: 6px 0; }
  .p-bar i { display: block; height: 100%; }
  .p-bar .made { background: var(--accent); }
  .p-bar .under { background: repeating-linear-gradient(-45deg, color-mix(in srgb, var(--accent) 55%, transparent) 0 6px, color-mix(in srgb, var(--accent) 30%, transparent) 6px 12px); }
  .part-why { margin: 2px 0 6px; font-size: 13px; color: var(--warn); }
  .pfs { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
  .pf { display: inline-flex; align-items: center; gap: 6px; padding: 3px 4px 3px 10px; border: 1px solid var(--line); border-radius: 999px; font-size: 13px; }
  .pf small { color: var(--muted); }
  .pf button { background: none; color: var(--muted); padding: 0 6px; font-size: 15px; }
  .part-tools { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 8px; }
  .part-tools details summary { cursor: pointer; color: var(--muted); font-size: 13px; list-style: none; padding: 3px 8px; border: 1px dashed var(--line); border-radius: 6px; }
  .part-tools details summary::-webkit-details-marker { display: none; }
  .part-tools form { display: flex; gap: 6px; flex-wrap: wrap; align-items: center; margin: 8px 0 0; }
  .part-tools select, .part-tools input { font-size: 13px; padding: 5px 8px; }
  .add-part { display: flex; gap: 6px; margin: 12px 0 0; flex-wrap: wrap; }
  .p-schedule { margin-bottom: 22px; }
  .p-schedule h3 { margin: 0 0 4px; }
  .tl { margin-top: 12px; display: grid; grid-template-columns: minmax(90px, 160px) minmax(0, 1fr); gap: 6px 12px; align-items: center; }
  .tl-name { font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .tl-name small { display: block; color: var(--muted); font-size: 12px; }
  .tl-track { position: relative; height: 30px; background: color-mix(in srgb, var(--line) 45%, transparent); border-radius: 6px; overflow: hidden; }
  .tl-track.off { background: repeating-linear-gradient(-45deg, transparent 0 6px, color-mix(in srgb, var(--line) 60%, transparent) 6px 12px); }
  .tl-b { position: absolute; top: 3px; bottom: 3px; border-radius: 4px; font-size: 11px; line-height: 24px; padding: 0 6px; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; min-width: 3px; box-sizing: border-box; }
  .tl-b.running { background: var(--busy); color: #fff; }
  .tl-b.planned { background: color-mix(in srgb, var(--accent) 22%, var(--card)); color: var(--ink); box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 45%, transparent); }
  .tl-name small.tl-waits { color: var(--warn); }
  .tl-now { position: absolute; top: 0; bottom: 0; width: 2px; background: var(--ink); opacity: .35; }
  .tl-note { position: absolute; left: 8px; top: 0; line-height: 30px; font-size: 12px; color: var(--muted); }
  .tl-axis { grid-column: 2; position: relative; height: 16px; font-size: 11px; color: var(--muted); }
  .tl-axis span { position: absolute; transform: translateX(-50%); white-space: nowrap; }
  .p-machine small.fit { color: var(--muted); }
  .p-machines { margin-top: 26px; }
  .p-machines h3 { margin: 0 0 4px; }
  .p-machine-list { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 12px; }
  .p-machine { display: inline-flex; gap: 8px; align-items: center; padding: 6px 12px; border: 1px solid var(--line); border-radius: 10px; font-size: 14px; }
  .p-machine.on { border-color: var(--accent); }
  .p-dialog { width: min(560px, calc(100vw - 32px)); }
  .ledger { list-style: none; margin: 0; padding: 0; }
  .ledger li { display: grid; grid-template-columns: 60px minmax(0, 1fr); gap: 2px 10px; padding: 8px 0; border-top: 1px solid var(--line); font-size: 14px; }
  .ledger .d { font-weight: 700; font-variant-numeric: tabular-nums; }
  .ledger .d.minus { color: var(--bad); } .ledger .d.plus { color: var(--accent); }
  .ledger small { grid-column: 2; color: var(--muted); }
"#;

const PROJECTS_SCRIPT: &str = r#"
  const esc = (t) => String(t ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const api = async (path, body) => {
    const r = await fetch(path, body === undefined ? {} : { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
    if (!r.ok) throw new Error(await r.text());
    return r.headers.get("content-type")?.includes("json") ? r.json() : null;
  };
  let profiles = {}, files = [], data = null;
  const model = (id) => profiles[id] ? (profiles[id].model.includes(profiles[id].brand) ? profiles[id].model : `${profiles[id].brand} ${profiles[id].model}`) : id;
  const STATES = { draft: "Draft", active: "Active", paused: "Paused", done: "Done" };

  function part(p, project) {
    const pct = (n) => Math.min(100, (n * 100) / Math.max(1, p.target));
    const made = Math.min(p.done, p.target);
    const files_ = p.files.map((f) => `<span class="pf" title="${esc(f.file_name)}"><b>${esc(model(f.profile))}</b><small>${f.per_plate}/plate${f.material ? ` · ${esc(f.material)}` : ""}${f.color ? ` · ${esc(f.color)}` : ""}${f.sheet ? ` · ${esc(f.sheet)}` : ""}</small><button type="button" data-remove-file="${p.id}:${f.id}" title="Take this file from the part" aria-label="Take this file from the part">×</button></span>`).join("");
    // The machine models there are in this space, first; then every profile.
    const here = [...new Set(data.machines.map((m) => m.profile).filter(Boolean))];
    const others = Object.keys(profiles).filter((id) => !here.includes(id) && profiles[id].kind === "fdm").sort();
    const code = files.filter((f) => /\.(b?gcode|gco|g)$/i.test(f.name));
    return `<div class="part">
      <div class="part-line"><b>${esc(p.name)}</b><span class="part-count"><b>${p.done}</b>${p.parts_under_way ? ` <span class="under">+${p.parts_under_way}</span>` : ""} / ${p.target}${p.done >= p.target ? " · done" : ""}</span></div>
      <div class="p-bar" title="${p.done} made, ${p.parts_under_way} on a bed or waiting to be confirmed, of ${p.target}"><i class="made" style="width:${pct(made)}%"></i><i class="under" style="width:${pct(Math.min(p.parts_under_way, Math.max(0, p.target - made)))}%"></i></div>
      ${p.waiting ? `<p class="part-why">Waits: ${esc(p.waiting)}</p>` : ""}
      <div class="pfs">${files_ || '<small class="muted">No file yet: add one for each machine model that may make it.</small>'}</div>
      <div class="part-tools">
        <details><summary>Add a file</summary><form data-add-file="${p.id}">
          <select name="file" required aria-label="File"><option value="">A file from Files…</option>${code.map((f) => `<option value="${esc(f.id)}">${esc(f.name)}</option>`).join("")}</select>
          <select name="profile" required aria-label="For which machine model">${here.map((id) => `<option value="${esc(id)}">${esc(model(id))}</option>`).join("")}${others.length ? `<optgroup label="Other models">${others.map((id) => `<option value="${esc(id)}">${esc(model(id))}</option>`).join("")}</optgroup>` : ""}</select>
          <input name="per_plate" type="number" min="1" max="1000" placeholder="per plate" aria-label="Parts a plate holds" style="width:7em">
          <input name="material" maxlength="20" placeholder="material (any)" aria-label="Material it needs" style="width:9em">
          <input name="sheet" maxlength="40" list="sheet-kinds" placeholder="build plate (any)" aria-label="Build plate it needs" style="width:10em">
          <button>Add</button></form></details>
        <details><summary>Change</summary><form data-change-part="${p.id}">
          <input name="name" value="${esc(p.name)}" maxlength="80" aria-label="Name">
          <input name="target" type="number" min="1" value="${p.target}" aria-label="How many" style="width:7em">
          <button>Save</button><button type="button" class="danger" data-remove-part="${p.id}">Remove</button></form></details>
        ${data.admin ? `<details><summary>Correct the count</summary><form data-correct="${p.id}"><input name="done" type="number" min="0" value="${p.done}" style="width:7em" aria-label="Made"><input name="note" required maxlength="200" placeholder="why (it goes in the ledger)" aria-label="Why"><button>Correct</button></form></details>` : ""}
        <button type="button" class="ghost" data-ledger="${p.id}" data-name="${esc(p.name)}">Ledger</button>
      </div>
    </div>`;
  }

  function render() {
    const box = document.querySelector("[data-projects]");
    box.innerHTML = data.projects.length ? data.projects.map((pr) => {
      const next = pr.state === "active" ? [["paused", "Pause"], ["done", "Done"]] : pr.state === "paused" ? [["active", "Go on"], ["done", "Done"]] : pr.state === "draft" ? [["active", "Start"]] : [["active", "Open again"]];
      return `<article class="project ${esc(pr.state)}">
        <div class="project-head"><h3>${esc(pr.name)}</h3><span class="p-state ${esc(pr.state)}">${STATES[pr.state] || pr.state}</span>
          <div class="p-actions">${next.map(([s, label]) => `<button type="button" class="${s === "active" ? "" : "ghost"}" data-state="${pr.id}:${s}">${label}</button>`).join("")}
            <button type="button" class="ghost" data-priority="${pr.id}:${pr.priority - 1}" title="Before the others">↑</button><button type="button" class="ghost" data-priority="${pr.id}:${pr.priority + 1}" title="After the others">↓</button>
            <button type="button" class="danger" data-remove-project="${pr.id}" data-name="${esc(pr.name)}">Remove</button></div></div>
        ${pr.description ? `<p class="intro">${esc(pr.description)}</p>` : ""}
        ${pr.parts.map((p) => part(p, pr)).join("")}
        <form class="add-part" data-add-part="${pr.id}"><input name="name" required maxlength="80" placeholder="A part: Bracket" aria-label="Name of a new part"><input name="target" type="number" min="1" required placeholder="how many" aria-label="How many" style="width:8em"><button class="ghost">Add part</button></form>
      </article>`;
    }).join("") : '<p class="empty">No project yet. Make one, give it parts, and a file for each machine model that may make them.</p>';
    document.querySelector("[data-machines]").innerHTML = data.machines.filter((m) => m.kind === "fdm" || m.kind === "laser").map((m) => `<label class="p-machine${m.farm ? " on" : ""}"><input type="checkbox" data-farm="${esc(m.id)}"${m.farm ? " checked" : ""}${m.manage ? "" : " disabled title=\"Who manages it decides\""}> ${esc(m.name)} <small class="muted">${esc(model(m.profile))}${m.sheet ? ` · ${esc(m.sheet)}` : ""}</small>${m.compatible && m.compatible[1] ? ` <small class="fit" title="Open parts it could make now, of all open parts">${m.compatible[0]}/${m.compatible[1]} parts</small>` : ""}</label>`).join("") || '<p class="muted">No machines yet.</p>';
  }

  function timeline(s) {
    const box = document.querySelector("[data-schedule-box]");
    const lanes = s.lanes || [];
    box.hidden = !lanes.length;
    if (!lanes.length) return;
    // As far ahead as there is work: 6 to 48 hours, in whole hours.
    const last = Math.max(s.now, ...lanes.flatMap((l) => l.blocks.map((b) => b.end)));
    const hours = Math.min(48, Math.max(6, Math.ceil((last - s.now) / 3600)));
    document.querySelector("[data-schedule-title]").textContent = `Next ${hours} hours`;
    const from = s.now - 1800, span = hours * 3600 + 1800;
    const x = (t) => Math.max(0, Math.min(100, ((t - from) * 100) / span));
    const clock = (t) => new Date(t * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    const rows = lanes.map((l) => {
      const blocks = l.blocks.map((b) => {
        const left = x(b.start), width = Math.max(0.4, x(b.end) - left);
        const tip = `${b.part ? `${b.part}${b.project ? ` (${b.project})` : ""} · ` : ""}${b.label} · ${clock(b.start)}–${clock(b.end)}`;
        return b.kind === "waiting" ? "" : `<span class="tl-b ${esc(b.kind)}" style="left:${left}%;width:${width}%" title="${esc(tip)}">${esc(b.part || b.label)}</span>`;
      }).join("");
      const waits = l.blocks.find((b) => b.kind === "waiting");
      const note = l.note ? `<span class="tl-note">${esc(l.note)}</span>` : (!l.blocks.length ? '<span class="tl-note">nothing it suits</span>' : "");
      return `<div class="tl-name" title="${esc(l.name)}">${esc(l.name)}${waits ? `<small class="tl-waits" title="Its next plate starts once someone has confirmed the last">waits for someone</small>` : ""}</div><div class="tl-track${l.note ? " off" : ""}">${blocks}${note}<span class="tl-now" style="left:${x(s.now)}%"></span></div>`;
    }).join("");
    let axis = "";
    const step = (hours <= 12 ? 2 : hours <= 24 ? 4 : 6) * 3600;
    const first = Math.ceil(s.now / step) * step;
    for (let t = first; t < from + span - step / 3; t += step) axis += `<span style="left:${x(t)}%">${new Date(t * 1000).toLocaleString([], { weekday: "short", hour: "2-digit", minute: "2-digit" })}</span>`;
    document.querySelector("[data-schedule]").innerHTML = rows + `<div class="tl-axis">${axis}</div>`;
  }

  async function load() {
    api("api/schedule").then(timeline).catch(() => {});
    try { data = await api("api/projects"); render(); } catch (e) { document.querySelector("[data-projects]").innerHTML = `<p class="note">${esc(e.message)}</p>`; }
  }
  const act = async (what, ok) => {
    try { await what(); if (ok) toast(ok); await load(); } catch (e) { toast(`Not done: ${e.message}`, "bad"); }
  };

  document.querySelector("[data-new-project]").addEventListener("submit", (e) => {
    e.preventDefault();
    act(() => api("api/projects", { name: e.target.elements.name.value }), "Project made").then(() => e.target.reset());
  });
  document.addEventListener("submit", (e) => {
    const f = e.target, v = Object.fromEntries(new FormData(f));
    if (f.dataset.addPart) { e.preventDefault(); act(() => api(`api/projects/${f.dataset.addPart}/parts`, { name: v.name, target: Number(v.target) }), "Part added"); }
    if (f.dataset.addFile) { e.preventDefault(); act(() => api(`api/parts/${f.dataset.addFile}/files`, { file: v.file, profile: v.profile, per_plate: v.per_plate ? Number(v.per_plate) : null, material: v.material || null, sheet: v.sheet || null }), "File added"); }
    if (f.dataset.changePart) { e.preventDefault(); act(() => api(`api/parts/${f.dataset.changePart}`, { name: v.name, target: Number(v.target) }), "Saved"); }
    if (f.dataset.correct) { e.preventDefault(); act(() => api(`api/parts/${f.dataset.correct}/count`, { done: Number(v.done), note: v.note }), "Corrected"); }
  });
  document.addEventListener("click", async (e) => {
    const t = e.target.closest("button, input[type=checkbox]");
    if (!t) return;
    if (t.dataset.state) { const [id, state] = t.dataset.state.split(":"); act(() => api(`api/projects/${id}`, { state }), state === "active" ? "Plates go out to free machines" : "Saved"); }
    if (t.dataset.priority) { const [id, priority] = t.dataset.priority.split(":"); act(() => api(`api/projects/${id}`, { priority: Number(priority) })); }
    if (t.dataset.removeProject && confirm(`Remove ${t.dataset.name} and its parts?`)) act(() => api(`api/projects/${t.dataset.removeProject}/remove`, {}), "Removed");
    if (t.dataset.removePart && confirm("Remove this part?")) act(() => api(`api/parts/${t.dataset.removePart}/remove`, {}), "Removed");
    if (t.dataset.removeFile) { const [p, f] = t.dataset.removeFile.split(":"); act(() => api(`api/parts/${p}/files/${f}/remove`, {}), "File taken off"); }
    if (t.dataset.farm) {
      const r = await fetch(`machines/${t.dataset.farm}/farm`, { method: "POST", body: new URLSearchParams(t.checked ? { on: "1" } : {}), headers: { "x-requested-with": "fetch" } });
      toast(r.ok ? (t.checked ? "It takes work from the queue" : "It no longer takes work") : `Not done: ${await r.text()}`, r.ok ? "ok" : "bad");
      load();
    }
    if (t.dataset.ledger) {
      const dialog = document.querySelector("[data-ledger-dialog]");
      document.querySelector("[data-ledger-title]").textContent = `Ledger: ${t.dataset.name}`;
      const lines = await api(`api/parts/${t.dataset.ledger}/ledger`);
      document.querySelector("[data-ledger-body]").innerHTML = lines.length ? `<ol class="ledger">${lines.map((l) => `<li><span class="d ${l.delta < 0 ? "minus" : "plus"}">${l.delta >= 0 ? "+" : ""}${l.delta}</span><span>${esc(l.source)} by <b>${esc(l.actor)}</b> → ${l.done_after}</span><small>${new Date(l.at * 1000).toLocaleString()}${l.job_id ? ` · job ${l.job_id}` : ""}${l.note ? ` · ${esc(l.note)}` : ""}</small></li>`).join("")}</ol>` : '<p class="empty">Nothing counted yet.</p>';
      dialog.showModal();
    }
    if (t.hasAttribute("data-close")) t.closest("dialog").close();
  });

  (async () => {
    try { profiles = Object.fromEntries((await (await fetch("api/profiles")).json()).map((p) => [p.id, p])); } catch { profiles = {}; }
    try { files = await (await fetch("api/files")).json(); } catch { files = []; }
    await load();
    // Live: counts and plates change as machines work.
    const stream = new EventSource("api/machines/stream");
    let soon = null;
    stream.onmessage = (event) => { summarise(JSON.parse(event.data)); clearTimeout(soon); soon = setTimeout(load, 800); };
    stream.onopen = () => live(true);
    stream.onerror = () => live(false);
  })();
"#;
