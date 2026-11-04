  // The files page: upload by dropping or choosing, the files as cards, print on a machine.
  const esc = (t) => String(t).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const list = document.querySelector("[data-files]");
  const uploads = document.querySelector("[data-uploads]");

  function duration(s) {
    if (s == null) return null;
    if (s < 60) return `${s} s`;
    const h = Math.floor(s / 3600), m = Math.round((s % 3600) / 60);
    return h ? `${h} h ${m} min` : `${m} min`;
  }
  function bytes(n) {
    if (n < 1e6) return `${(n / 1e3).toFixed(0)} kB`;
    if (n < 1e9) return `${(n / 1e6).toFixed(1)} MB`;
    return `${(n / 1e9).toFixed(1)} GB`;
  }
  function ago(t) {
    const s = Date.now() / 1000 - t;
    if (s < 60) return "just now";
    if (s < 3600) return `${Math.floor(s / 60)} min ago`;
    if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
    return new Date(t * 1000).toLocaleDateString();
  }
  const fact = (value, label = "") => (value == null ? "" : `<span><b>${esc(value)}</b>${label}</span>`);

  const KINDS = { fdm: "3D printer", laser: "Laser", label: "Label", other: "Unknown" };
  const kindOf = (f) => (f.meta && f.meta.kind && KINDS[f.meta.kind] ? f.meta.kind : "other");
  let machines = []; // the space's machines, from the live list: id, name, kind
  const machineName = (id) => machines.find((m) => m.id === id)?.name || "a removed machine";

  function card(f) {
    const m = f.meta || {};
    const picture = m.thumbnail
      ? `<img src="files/${encodeURIComponent(f.id)}/thumbnail.png" alt="">`
      : "No picture in the file";
    const filament = m.filament_g != null ? `${m.filament_g.toFixed(1)} g` : m.filament_m != null ? `${m.filament_m.toFixed(2)} m` : null;
    const options = f.machines.map((t) =>
      `<option value="${esc(t.id)}"${t.ok ? "" : " disabled"}>${esc(t.name)}${t.note ? ` — ${esc(t.note)}` : ""}</option>`).join("");
    const ready = f.machines.some((t) => t.ok);
    const label = m.label_dots ? `${Math.round((m.label_dots[0] / 180) * 25.4)} mm label` : null;
    const last = f.printed[0];
    const printed = last
      ? `<div class="printed">${last.state === "finished" ? "Printed" : esc(last.state)} on <b>${esc(machineName(last.machine_id))}</b> ${ago(last.at)}${f.printed.length > 1 ? ` · ${f.printed.length}${f.printed.length === 5 ? "+" : ""} times` : ""}</div>`
      : `<div class="printed">Not printed yet</div>`;
    return `<article class="file${selected.has(f.id) ? " picked" : ""}" data-id="${esc(f.id)}">
      <input type="checkbox" class="pick" aria-label="Select ${esc(f.name)}"${selected.has(f.id) ? " checked" : ""}>
      <span class="kind">${KINDS[kindOf(f)]}</span>
      <div class="picture">${picture}</div>
      <div class="body">
        <h3 title="${esc(f.name)}">${esc(f.name)}</h3>
        <div class="facts">${fact(duration(m.estimated_s))}${fact(filament, m.filament_type ? ` ${esc(m.filament_type)}` : "")}${fact(m.layer_height_mm != null ? `${m.layer_height_mm} mm` : null, " layers")}${fact(m.printer_model, "")}${fact(label)}</div>
        <div class="when">${ago(f.uploaded_at)} · ${bytes(f.size)}${m.slicer ? ` · ${esc(m.slicer)}` : ""}</div>
        ${printed}
        <form data-print>
          <select name="machine" aria-label="Machine to print on"${f.machines.length ? "" : " disabled"}>
            <option value="">${f.machines.length ? (ready ? "Print on…" : "No machine free for it") : "No machines yet"}</option>${options}
          </select>
          ${m.kind === "laser" ? `<button type="button" class="ghost" data-frame${ready ? "" : " disabled"} title="Run the outline with the laser off">Frame</button>` : ""}
          <button${ready ? "" : " disabled"}>${m.kind === "laser" ? "Start" : "Print"}</button>
        </form>
        <p class="note" data-note role="status"></p>
        <div class="more"><a href="files/${encodeURIComponent(f.id)}/download">Download</a><button type="button" data-remove>Remove</button></div>
      </div>
    </article>`;
  }

  // A file as a row of the list: the same facts and actions as its card.
  function row(f) {
    const m = f.meta || {};
    const picture = m.thumbnail ? `<img src="files/${encodeURIComponent(f.id)}/thumbnail.png" alt="">` : "<span></span>";
    const filament = m.filament_g != null ? `${m.filament_g.toFixed(1)} g` : m.filament_m != null ? `${m.filament_m.toFixed(2)} m` : "";
    const options = f.machines.map((t) =>
      `<option value="${esc(t.id)}"${t.ok ? "" : " disabled"}>${esc(t.name)}${t.note ? ` — ${esc(t.note)}` : ""}</option>`).join("");
    const ready = f.machines.some((t) => t.ok);
    const label = m.label_dots ? `${Math.round((m.label_dots[0] / 180) * 25.4)} mm` : "";
    const last = f.printed[0];
    const cell = (v) => (v == null || v === "" ? '<span class="muted">–</span>' : esc(v));
    return `<tr class="file-row${selected.has(f.id) ? " picked" : ""}" data-id="${esc(f.id)}">
      <td><input type="checkbox" class="pick" aria-label="Select ${esc(f.name)}"${selected.has(f.id) ? " checked" : ""}></td>
      <td class="thumb-cell">${picture}</td>
      <td class="name-cell"><b title="${esc(f.name)}">${esc(f.name)}</b><small>${KINDS[kindOf(f)]}${m.slicer ? ` · ${esc(m.slicer)}` : ""}</small></td>
      <td>${cell(m.printer_model)}</td>
      <td class="num">${cell(duration(m.estimated_s) || label)}</td>
      <td class="num">${cell(m.filament_type ? `${m.filament_type} ${filament}`.trim() : filament)}</td>
      <td class="num">${cell(m.layer_height_mm != null ? `${m.layer_height_mm} mm` : null)}</td>
      <td class="num">${ago(f.uploaded_at)}<small class="muted"> · ${bytes(f.size)}</small></td>
      <td class="num">${last ? `${esc(machineName(last.machine_id))}, ${ago(last.at)}` : '<span class="muted">not yet</span>'}</td>
      <td class="row-actions"><a href="files/${encodeURIComponent(f.id)}/download" title="Download" aria-label="Download"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 1.5v8.5M4.5 6.5 8 10l3.5-3.5M2.5 13.5h11" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg></a><button type="button" data-remove title="Remove" aria-label="Remove"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 4h11M6 4V2.5h4V4M4 4l.7 9.5h6.6L12 4M6.6 6.5v5M9.4 6.5v5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg></button></td>
      <td><form data-print>
        <select name="machine" aria-label="Machine to print on"${f.machines.length ? "" : " disabled"}>
          <option value="">${f.machines.length ? (ready ? "Print on…" : "No machine free") : "No machines"}</option>${options}
        </select>
        ${m.kind === "laser" ? `<button type="button" class="ghost" data-frame${ready ? "" : " disabled"} title="Run the outline with the laser off">Frame</button>` : ""}
        <button${ready ? "" : " disabled"}>${m.kind === "laser" ? "Start" : "Print"}</button>
      </form><span class="note" data-note role="status"></span></td>
    </tr>`;
  }

  // What is shown: the filters and the sort, kept per browser.
  const search = document.querySelector("[data-search]");
  const sliced = document.querySelector("[data-sliced]");
  const material = document.querySelector("[data-material]");
  const layer = document.querySelector("[data-layer]");
  const kinds = document.querySelector("[data-kinds]");
  const machineFilter = document.querySelector("[data-machine]");
  const sort = document.querySelector("[data-sort]");
  const saved = (() => { try { return JSON.parse(localStorage.getItem("files-view")) || {}; } catch { return {}; } })();
  let kind = saved.kind || "";
  let view = saved.view === "list" ? "list" : "cards";
  sort.value = saved.sort || "new";
  const save = () => {
    try {
      localStorage.setItem("files-view", JSON.stringify({
        kind, view, sort: sort.value, machine: machineFilter.value,
        sliced: sliced.value, material: material.value, layer: layer.value,
      }));
    } catch {}
  };
  // A filter on what the files say: its values, the choice kept; hidden when there is none.
  function fillFilter(select, all, values) {
    const chosen = select.value || saved[select.dataset.key] || "";
    const unique = [...new Set(values.filter((v) => v != null && v !== ""))].sort((a, b) => String(a).localeCompare(String(b), undefined, { numeric: true }));
    select.hidden = !unique.length;
    select.innerHTML = `<option value="">${all}</option>` + unique.map((v) => `<option value="${esc(v)}">${esc(v)}</option>`).join("");
    select.value = unique.map(String).includes(chosen) ? chosen : "";
  }
  sliced.dataset.key = "sliced"; material.dataset.key = "material"; layer.dataset.key = "layer";
  let files = [];
  const selected = new Set();

  // A file is for a machine when it was printed on it, or is made for its kind.
  function forMachine(f, id) {
    if (f.printed.some((p) => p.machine_id === id)) return true;
    const machine = machines.find((m) => m.id === id);
    return machine && kindOf(f) === machine.kind;
  }
  function shownFiles() {
    const words = search.value.trim().toLowerCase();
    const machine = machineFilter.value;
    const lastAt = (f) => f.printed[0]?.at ?? -1;
    const order = {
      new: (a, b) => b.uploaded_at - a.uploaded_at,
      old: (a, b) => a.uploaded_at - b.uploaded_at,
      printed: (a, b) => lastAt(b) - lastAt(a) || b.uploaded_at - a.uploaded_at,
      name: (a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }),
      size: (a, b) => b.size - a.size,
    }[sort.value];
    return files
      .filter((f) => !kind || kindOf(f) === kind)
      .filter((f) => !machine || forMachine(f, machine))
      .filter((f) => !sliced.value || f.meta?.printer_model === sliced.value)
      .filter((f) => !material.value || f.meta?.filament_type === material.value)
      .filter((f) => !layer.value || String(f.meta?.layer_height_mm) === layer.value)
      .filter((f) => !words || f.name.toLowerCase().includes(words))
      .sort(order);
  }

  function render() {
    // The type buttons, with how many files each.
    const counts = {};
    for (const f of files) counts[kindOf(f)] = (counts[kindOf(f)] || 0) + 1;
    if (kind && !counts[kind]) kind = "";
    kinds.innerHTML = [["", "All", files.length], ...Object.keys(KINDS).filter((k) => counts[k]).map((k) => [k, KINDS[k], counts[k]])]
      .map(([k, name, n]) => `<button type="button" data-kind="${k}" aria-pressed="${k === kind}">${name}<small>${n}</small></button>`).join("");
    // The machines to filter on, keeping the choice.
    const chosen = machineFilter.value || saved.machine || "";
    machineFilter.innerHTML = `<option value="">All machines</option>` +
      machines.map((m) => `<option value="${esc(m.id)}">${esc(m.name)}</option>`).join("");
    machineFilter.value = machines.some((m) => m.id === chosen) ? chosen : "";
    // What the files say about themselves, to filter on.
    fillFilter(sliced, "Sliced for any", files.map((f) => f.meta?.printer_model));
    fillFilter(material, "Any material", files.map((f) => f.meta?.filament_type));
    fillFilter(layer, "Any layer height", files.map((f) => f.meta?.layer_height_mm));
    for (const b of document.querySelectorAll("[data-view]")) b.setAttribute("aria-pressed", b.dataset.view === view);
    // The files.
    for (const id of [...selected]) if (!files.some((f) => f.id === id)) selected.delete(id);
    const shown = shownFiles();
    list.classList.toggle("list", view === "list" && shown.length > 0);
    list.innerHTML = !files.length
      ? '<p class="empty">No files yet. Drop G-code here, or upload it with the button. Labels made on a label printer\'s page come here too.</p>'
      : !shown.length
        ? '<p class="empty">No files match.</p>'
        : view === "list"
          ? `<div class="table-wrap"><table class="file-table"><thead><tr><th></th><th></th><th>File</th><th>Sliced for</th><th>Time</th><th>Material</th><th>Layer</th><th>Uploaded</th><th>Last printed</th><th></th><th></th></tr></thead><tbody>${shown.map(row).join("")}</tbody></table></div>`
          : shown.map(card).join("");
    showSelection();
  }

  function showSelection() {
    const n = selected.size;
    document.querySelector("[data-count]").textContent =
      `${shownFiles().length} of ${files.length} files` + (n ? ` · ${n} selected` : "");
    document.querySelector("[data-select-none]").hidden = !n;
    const remove = document.querySelector("[data-remove-selected]");
    remove.hidden = !n;
    remove.textContent = `Remove ${n} selected`;
  }

  let fetched = "";
  async function refresh() {
    try {
      const fresh = await (await fetch("api/files")).json();
      const key = JSON.stringify(fresh);
      if (key === fetched) return;
      // Keep a choice someone is making.
      if (document.activeElement?.closest("[data-print]")) return;
      fetched = key;
      files = fresh;
      render();
      showStorage();
    } catch (error) { /* tried again shortly */ }
  }
  
  // What the files take, and what the disk has left.
  async function showStorage() {
    try {
      const s = await (await fetch("api/storage")).json();
      const box = document.querySelector("[data-storage]");
      box.hidden = false;
      const disk = s.disk_bytes || 0, free = s.free_bytes;
      const usedOnDisk = disk && free != null ? disk - free : 0;
      document.querySelector("[data-storage-fill]").style.width = disk ? `${Math.min(100, (usedOnDisk / disk) * 100).toFixed(1)}%` : "0";
      document.querySelector("[data-storage-text]").textContent =
        `${bytes(s.used_bytes)} in files` + (free != null ? ` · ${bytes(free)} free` : "");
    } catch { /* shown on the next refresh */ }
  }

  search.addEventListener("input", render);
  for (const select of [sliced, material, layer]) select.addEventListener("change", () => { save(); render(); });
  for (const b of document.querySelectorAll("[data-view]")) b.addEventListener("click", () => {
    view = b.dataset.view;
    save();
    render();
  });
  sort.addEventListener("change", () => { save(); render(); });
  machineFilter.addEventListener("change", () => { save(); render(); });
  kinds.addEventListener("click", (event) => {
    const button = event.target.closest("[data-kind]");
    if (!button) return;
    kind = button.dataset.kind;
    save();
    render();
  });
  list.addEventListener("change", (event) => {
    if (!event.target.matches(".pick")) return;
    const file = event.target.closest("[data-id]");
    if (event.target.checked) selected.add(file.dataset.id); else selected.delete(file.dataset.id);
    file.classList.toggle("picked", event.target.checked);
    showSelection();
  });
  document.querySelector("[data-select-shown]").addEventListener("click", () => {
    for (const f of shownFiles()) selected.add(f.id);
    render();
  });
  document.querySelector("[data-select-old]").addEventListener("click", () => {
    const month = Date.now() / 1000 - 30 * 86400;
    for (const f of shownFiles()) {
      if (f.uploaded_at < month && !(f.printed[0]?.at > month)) selected.add(f.id);
    }
    render();
  });
  document.querySelector("[data-select-none]").addEventListener("click", () => { selected.clear(); render(); });
  document.querySelector("[data-remove-selected]").addEventListener("click", async () => {
    const ids = [...selected];
    if (!ids.length || !confirm(`Remove ${ids.length} files? This cannot be undone.`)) return;
    const response = await fetch("api/files/remove", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ ids }),
    });
    const result = response.ok ? await response.json() : { removed: 0, kept: ids };
    selected.clear();
    for (const id of result.kept) selected.add(id);
    if (result.kept.length) alert(`${result.removed} removed; ${result.kept.length} kept: a machine runs or waits for them.`);
    fetched = "";
    refresh();
  });

  list.addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.target, file = form.closest("[data-id]"), note = file.querySelector("[data-note]");
    const machine = form.machine.value;
    if (!machine) { note.textContent = "Choose a machine first."; return; }
    const response = await fetch(`files/${encodeURIComponent(file.dataset.id)}/print`, {
      method: "POST",
      body: new URLSearchParams({ machine }),
      headers: { "x-requested-with": "fetch" },
    });
    if (response.ok) location.href = `machines/${encodeURIComponent(machine)}#control`;
    else note.textContent = `Not started: ${await response.text()}`;
  });
  // Frame: the outline of the job with the laser off, on the chosen machine.
  list.addEventListener("click", async (event) => {
    const button = event.target.closest("[data-frame]");
    if (!button) return;
    const file = button.closest("[data-id]"), note = file.querySelector("[data-note]");
    const machine = file.querySelector("select").value;
    if (!machine) { note.textContent = "Choose a machine first."; return; }
    const response = await fetch(`files/${encodeURIComponent(file.dataset.id)}/frame`, {
      method: "POST",
      body: new URLSearchParams({ machine }),
    });
    note.textContent = response.ok ? "Framing: the head runs the outline, laser off." : `Not framed: ${await response.text()}`;
  });
  list.addEventListener("click", async (event) => {
    const button = event.target.closest("[data-remove]");
    if (!button) return;
    const file = button.closest("[data-id]");
    const name = files.find((f) => f.id === file.dataset.id)?.name || "this file";
    if (!confirm(`Remove ${name}?`)) return;
    const response = await fetch(`files/${encodeURIComponent(file.dataset.id)}/remove`, { method: "POST" });
    if (!response.ok) alert(await response.text());
    fetched = "";
    refresh();
  });

  // Upload with progress (fetch has none for what it sends).
  function upload(file) {
    const row = document.createElement("div");
    row.className = "upload";
    row.innerHTML = `<span>${esc(file.name)}</span><span>0 %</span><div class="bar"><i></i></div>`;
    uploads.append(row);
    const xhr = new XMLHttpRequest();
    xhr.open("POST", `api/files?name=${encodeURIComponent(file.name)}`);
    xhr.upload.onprogress = (e) => {
      if (!e.lengthComputable) return;
      const pct = Math.round((e.loaded / e.total) * 100);
      row.querySelector("i").style.width = `${pct}%`;
      row.lastElementChild.previousElementSibling.textContent = `${pct} %`;
    };
    xhr.onload = () => {
      if (xhr.status < 300) {
        row.remove();
        fetched = "";
        refresh();
      } else {
        row.classList.add("failed");
        row.children[1].textContent = xhr.responseText || "not kept";
      }
    };
    xhr.onerror = () => { row.classList.add("failed"); row.children[1].textContent = "not sent"; };
    xhr.send(file);
  }
  document.querySelector("[data-pick]").addEventListener("change", (event) => {
    for (const file of event.target.files) upload(file);
    event.target.value = "";
  });
  const drop = document.querySelector("[data-drop]");
  let depth = 0;
  addEventListener("dragenter", (e) => { if (e.dataTransfer?.types.includes("Files")) { depth++; drop.hidden = false; } });
  addEventListener("dragleave", () => { depth = Math.max(0, depth - 1); if (!depth) drop.hidden = true; });
  addEventListener("dragover", (e) => e.preventDefault());
  addEventListener("drop", (e) => {
    e.preventDefault();
    depth = 0;
    drop.hidden = true;
    for (const file of e.dataTransfer.files) upload(file);
  });

  // The header's counts, and whether a machine is free, follow the live machine list.
  const stream = new EventSource("api/machines/stream");
  let last = 0;
  stream.onmessage = (event) => {
    const all = JSON.parse(event.data);
    summarise(all);
    const before = JSON.stringify(machines);
    machines = all.map((m) => ({ id: m.id, name: m.name, kind: m.kind }));
    if (before !== JSON.stringify(machines) && files.length) render();
    if (Date.now() - last > 5000) { last = Date.now(); refresh(); }
  };
  stream.onopen = () => live(true);
  stream.onerror = () => live(false);
  // The machines first, so the files show where they were printed by name.
  fetch("api/machines")
    .then((r) => r.json())
    .then((all) => { machines = all.map((m) => ({ id: m.id, name: m.name, kind: m.kind })); })
    .catch(() => {})
    .finally(refresh);
