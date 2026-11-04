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
    return `${(n / 1e6).toFixed(1)} MB`;
  }
  function ago(t) {
    const s = Date.now() / 1000 - t;
    if (s < 60) return "just now";
    if (s < 3600) return `${Math.floor(s / 60)} min ago`;
    if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
    return new Date(t * 1000).toLocaleDateString();
  }
  const fact = (value, label = "") => (value == null ? "" : `<span><b>${esc(value)}</b>${label}</span>`);

  function card(f) {
    const m = f.meta || {};
    const picture = m.thumbnail
      ? `<img src="files/${encodeURIComponent(f.id)}/thumbnail.png" alt="">`
      : "No picture in the file";
    const filament = m.filament_g != null ? `${m.filament_g.toFixed(1)} g` : m.filament_m != null ? `${m.filament_m.toFixed(2)} m` : null;
    const options = f.machines.map((t) =>
      `<option value="${esc(t.id)}"${t.ok ? "" : " disabled"}>${esc(t.name)}${t.note ? ` — ${esc(t.note)}` : ""}</option>`).join("");
    const ready = f.machines.some((t) => t.ok);
    return `<article class="file" data-id="${esc(f.id)}">
      <div class="picture">${picture}</div>
      <div class="body">
        <h3 title="${esc(f.name)}">${esc(f.name)}</h3>
        <div class="facts">${fact(duration(m.estimated_s))}${fact(filament, m.filament_type ? ` ${esc(m.filament_type)}` : "")}${fact(m.layer_height_mm != null ? `${m.layer_height_mm} mm` : null, " layers")}${fact(m.printer_model, "")}</div>
        <div class="when">${ago(f.uploaded_at)} · ${bytes(f.size)}${m.slicer ? ` · ${esc(m.slicer)}` : ""}</div>
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

  let shown = "";
  async function refresh() {
    try {
      const files = await (await fetch("api/files")).json();
      const key = JSON.stringify(files);
      if (key === shown) return;
      // Keep a choice someone is making.
      if (document.activeElement?.closest("[data-print]")) return;
      shown = key;
      list.innerHTML = files.length
        ? files.map(card).join("")
        : '<p class="empty">No files yet. Drop G-code here, or upload it with the button.</p>';
    } catch (error) { /* tried again shortly */ }
  }

  list.addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.target, file = form.closest(".file"), note = file.querySelector("[data-note]");
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
    const file = button.closest(".file"), note = file.querySelector("[data-note]");
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
    const file = button.closest(".file");
    if (!confirm(`Remove ${file.querySelector("h3").textContent}?`)) return;
    await fetch(`files/${encodeURIComponent(file.dataset.id)}/remove`, { method: "POST" });
    shown = "";
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
        shown = "";
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
    summarise(JSON.parse(event.data));
    if (Date.now() - last > 5000) { last = Date.now(); refresh(); }
  };
  stream.onopen = () => live(true);
  stream.onerror = () => live(false);
  refresh();
