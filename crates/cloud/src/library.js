  // Materials: the library (spools grouped beside an inspector) and the board of locations,
  // on Spoolman's API (/api/v1), live through its WebSockets. A USB barcode scanner types
  // into the search box (or anywhere): S-0007 opens a spool, L-0002 moves the open one there,
  // an EAN finds its filament, a tag's UID its spool.
  const $ = (s, root = document) => root.querySelector(s);
  const esc = (t) => String(t ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const dialog = $("[data-dialog]"), dialogBody = $("[data-dialog-body]");
  const inspector = $("[data-inspector]");
  let spools = [], filaments = [], vendors = [], places = [], tapes = [], fields = { spool: [], filament: [] };
  let materialsDb = [];
  let settings = { label_template: "{code}\n{vendor} {name}\n{material} {color}", board_field: "location" };
  let machines = [];
  let chosen = null; // { type: "spool" | "filament" | "vendor", id }
  const ui = { view: "library", group: "filament", sort: "name", search: "", archived: false, collapsed: new Set() };
  try { Object.assign(ui, JSON.parse(localStorage.getItem("library-ui") || "{}"), { collapsed: new Set(JSON.parse(localStorage.getItem("library-collapsed") || "[]")) }); } catch (e) { /* defaults */ }
  const remember = () => {
    try {
      localStorage.setItem("library-ui", JSON.stringify({ view: ui.view, group: ui.group, sort: ui.sort, archived: ui.archived }));
      localStorage.setItem("library-collapsed", JSON.stringify([...ui.collapsed]));
    } catch (e) { /* private window */ }
  };

  // ---- reading what the API gives -----------------------------------------------------
  // Spoolman's extra fields are JSON texts in strings; Maker Connect's own go out that way.
  const extra = (o, key) => { try { return o?.extra?.[key] != null ? JSON.parse(o.extra[key]) : undefined; } catch (e) { return o?.extra?.[key]; } };
  const unitOf = (f) => extra(f, "unit") || "g";
  const kindOf = (f) => extra(f, "kind") || "filament";
  const code = (id) => `S-${String(id).padStart(4, "0")}`;
  const numberOf = (c) => { const m = String(c).trim().toUpperCase().match(/^S-(\d+)$/); return m ? Number(m[1]) : null; };
  const amount = (v, unit = "g") => v == null ? "–" : unit === "g"
    ? (Math.abs(v) >= 1000 ? `${(v / 1000).toFixed(2)} kg` : `${Math.round(v)} g`)
    : unit === "m" ? `${Number(v).toFixed(1)} m` : `${Math.round(v)} ${unit}`;
  const vendorOf = (f) => f?.vendor?.name || "";
  // Its maker and name, the maker once, and its colour's name.
  const nameOf = (f) => {
    if (!f) return "?";
    const v = vendorOf(f), n = f.name || f.material || "Filament";
    return `${v && !n.toLowerCase().startsWith(v.toLowerCase()) ? `${v} ` : ""}${n}`;
  };
  const colourName = (f) => extra(f, "color_name");
  const hexes = (f) => (f?.multi_color_hexes ? f.multi_color_hexes.split(",") : f?.color_hex ? [f.color_hex] : []).map((h) => `#${h.replace("#", "").slice(0, 6)}`);
  function swatch(f, big = false) {
    if (kindOf(f) === "tape") {
      const tape = f.color_hex ? `#${f.color_hex}` : "#fff";
      return `<span class="swatch-tape" style="--tape:${esc(tape)};--ink:${esc(extra(f, "ink") || "#000")}">Abc</span>`;
    }
    const h = hexes(f);
    const fill = !h.length ? "#999" : h.length === 1 ? h[0]
      : `conic-gradient(${h.map((c, i) => `${c} ${(i * 100) / h.length}% ${((i + 1) * 100) / h.length}%`).join(", ")})`;
    return `<span class="swatch-spool${big ? " big" : ""}" style="--c:${esc(fill)}"></span>`;
  }
  const facts = (f) => [f.material, f.diameter && kindOf(f) === "filament" && `${f.diameter} mm`, colourName(f)].filter(Boolean).join(" · ");
  const leftOf = (s) => s.remaining_weight;
  const share = (s) => s.initial_weight ? Math.max(0, Math.min(1, (s.remaining_weight ?? 0) / s.initial_weight)) : null;
  const loadedIn = (s) => extra(s, "loaded_in");
  const whereOf = (s) => loadedIn(s) ? `in ${loadedIn(s)}` : s.location || "no place";
  const bar = (s) => { const p = share(s); return `<div class="bar${p != null && p < 0.15 ? " low" : ""}"><i style="width:${p == null ? 0 : Math.round(p * 100)}%"></i></div>`; };
  const ago = (iso) => {
    if (!iso) return "never";
    const s = (Date.now() - Date.parse(iso)) / 1000;
    if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min ago`;
    if (s < 86400) return `${Math.round(s / 3600)} h ago`;
    if (s < 86400 * 60) return `${Math.round(s / 86400)} days ago`;
    return new Date(iso).toLocaleDateString();
  };
  const filament = (id) => filaments.find((f) => f.id === id);
  const placeCode = (name) => places.find((p) => p.name === name)?.id;

  async function api(method, path, body) {
    const r = await fetch(path.startsWith("api/") ? path : `api/v1${path}`, {
      method, headers: body !== undefined ? { "content-type": "application/json" } : {},
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    if (!r.ok) {
      let why = await r.text();
      try { why = JSON.parse(why).message || why; } catch (e) { /* text */ }
      throw new Error(why || r.statusText);
    }
    return r.status === 204 ? null : r.json();
  }

  async function load() {
    const [s, f, v, stock, fs, ff] = await Promise.all([
      api("GET", "/spool?allow_archived=true"), api("GET", "/filament"), api("GET", "/vendor"),
      api("GET", "api/stock"), api("GET", "/field/spool"), api("GET", "/field/filament"),
    ]);
    spools = s; filaments = f; vendors = v; places = stock.locations; fields = { spool: fs, filament: ff };
    try {
      const all = await api("GET", "/setting/");
      for (const key of ["label_template", "board_field"]) if (all[key]) settings[key] = JSON.parse(all[key].value);
    } catch (e) { /* defaults */ }
    tapes = stock.catalogue.filter((m) => m.kind === "tape");
    render();
  }

  // ---- the list -------------------------------------------------------------------------
  function matching() {
    const words = ui.search.toLowerCase().split(/\s+/).filter(Boolean);
    return spools.filter((s) => {
      if (s.archived && !ui.archived) return false;
      if (!words.length) return true;
      const f = s.filament;
      const hay = [code(s.id), nameOf(f), f?.material, colourName(f), s.location, s.lot_nr, loadedIn(s), f?.article_number, ...(s.tags || []).map((t) => t.uid)].join(" ").toLowerCase();
      return words.every((w) => hay.includes(w));
    });
  }
  const sortKey = {
    name: (s) => nameOf(s.filament).toLowerCase(),
    remaining: (s) => -(leftOf(s) ?? -1),
    last_used: (s) => -(Date.parse(s.last_used || 0) || 0),
    id: (s) => -s.id,
  };
  const compare = (a, b) => { const x = sortKey[ui.sort](a), y = sortKey[ui.sort](b); return x < y ? -1 : x > y ? 1 : a.id - b.id; };

  function groupsOf(list) {
    const by = new Map();
    const add = (key, title, sub, swatchFor, kind, id) => { if (!by.has(key)) by.set(key, { key, title, sub, swatchFor, kind, id, spools: [] }); return by.get(key); };
    for (const s of list) {
      const f = s.filament;
      let g;
      if (ui.group === "filament") g = add(`f${f.id}`, nameOf(f), facts(f), f, "filament", f.id);
      else if (ui.group === "vendor") g = add(`v${f.vendor?.id ?? 0}`, vendorOf(f) || "No vendor", "", null, f.vendor ? "vendor" : null, f.vendor?.id);
      else if (ui.group === "location") g = add(`l${loadedIn(s) ? "m" + loadedIn(s) : s.location || ""}`, loadedIn(s) ? `In ${loadedIn(s)}` : s.location || "No place", "", null, null);
      else if (ui.group === "material") g = add(`m${f.material || ""}`, f.material || "No material", "", null, null);
      else g = add("all", "", "", null, null);
      g.spools.push(s);
    }
    // Filaments without a spool (yet) are there too, when nothing is searched.
    if (ui.group === "filament" && !ui.search) for (const f of filaments) add(`f${f.id}`, nameOf(f), facts(f), f, "filament", f.id);
    const groups = [...by.values()];
    for (const g of groups) g.spools.sort(compare);
    const total = (g) => g.spools.reduce((t, s) => t + (s.archived ? 0 : leftOf(s) ?? 0), 0);
    groups.sort((a, b) => ui.sort === "name" ? a.title.localeCompare(b.title)
      : ui.sort === "remaining" ? total(b) - total(a)
      : a.spools.length && b.spools.length ? compare(a.spools[0], b.spools[0]) : b.spools.length - a.spools.length);
    return groups;
  }

  function render() {
    renderTotals();
    if (ui.view === "locations") { renderBoard(); return; }
    const list = matching();
    const groups = groupsOf(list);
    const listEl = $("[data-list]");
    if (!spools.length && !filaments.length) {
      listEl.innerHTML = `<div class="group"><p class="empty" style="padding:24px">Nothing yet. <b>Add spools</b> finds a filament among thousands of SpoolmanDB, or your label printer's tapes; <b>Import from Spoolman</b> brings a Spoolman's spools over.</p></div>`;
    } else if (!groups.length) {
      listEl.innerHTML = `<p class="empty">Nothing matches “${esc(ui.search)}”.</p>`;
    } else {
      listEl.innerHTML = groups.map((g) => {
        const live = g.spools.filter((s) => !s.archived);
        const unit = g.spools[0] ? unitOf(g.spools[0].filament) : g.swatchFor ? unitOf(g.swatchFor) : "g";
        const total = live.reduce((t, s) => t + (leftOf(s) ?? 0), 0);
        const minimum = g.swatchFor ? extra(g.swatchFor, "minimum") : null;
        const low = minimum != null && live.length < minimum;
        const isChosen = chosen && g.kind === chosen.type && g.id === chosen.id;
        const open = !ui.collapsed.has(g.key);
        const head = ui.group === "none" ? "" : `<div class="group-head" data-group-key="${esc(g.key)}"${g.kind ? ` data-pick="${g.kind}:${g.id}"` : ""}>
            ${g.swatchFor ? swatch(g.swatchFor) : `<span class="badge">${live.length}</span>`}
            <span class="g-name"><b>${esc(g.title)}</b>${g.sub ? `<small>${esc(g.sub)}</small>` : ""}</span>
            <span class="g-total"><b>${amount(total, unit)}</b><small class="${low ? "low" : ""}">${live.length} ${live.length === 1 ? "spool" : "spools"}${low ? " · low" : ""}</small></span></div>`;
        const rows = open && g.spools.length ? `<ul class="rows">${g.spools.map((s) => `<li class="row${s.archived ? " archived" : ""}${chosen?.type === "spool" && chosen.id === s.id ? " chosen" : ""}" data-pick="spool:${s.id}">
            <code>${code(s.id)}</code>
            <div class="row-bar">${bar(s)}${ui.group !== "filament" ? `<span>${esc(nameOf(s.filament))}</span>` : `<span>${s.lot_nr ? `lot ${esc(s.lot_nr)} · ` : ""}used ${ago(s.last_used)}</span>`}</div>
            <span class="where${loadedIn(s) ? " loaded" : ""}">${esc(s.archived ? "used up" : whereOf(s))}</span>
            <span class="left">${amount(leftOf(s), unitOf(s.filament))}</span></li>`).join("")}</ul>` : "";
        return `<div class="group${isChosen ? " chosen" : ""}">${head}${rows}</div>`;
      }).join("");
    }
    renderInspector();
  }

  function renderTotals() {
    const live = spools.filter((s) => !s.archived);
    const grams = live.filter((s) => unitOf(s.filament) === "g");
    const cassettes = live.filter((s) => kindOf(s.filament) === "tape");
    const lows = filaments.filter((f) => { const m = extra(f, "minimum"); return m != null && live.filter((s) => s.filament.id === f.id).length < m; });
    $("[data-totals]").innerHTML = [
      ["Filament", amount(grams.reduce((t, s) => t + (leftOf(s) ?? 0), 0)), `${grams.length} spools`],
      ["Loaded", String(live.filter((s) => loadedIn(s)).length), "in machines"],
      ["Low", String(lows.length), lows.length ? lows.slice(0, 2).map(nameOf).join(", ") : "nothing to order", lows.length > 0],
      ["Label tape", String(cassettes.length), "cassettes"],
    ].map(([l, v, s, warn]) => `<div class="tile"><div class="label">${l}</div><div class="value">${esc(v)}</div><div class="sub${warn ? " low" : ""}">${esc(s)}</div></div>`).join("");
  }

  // ---- the inspector ---------------------------------------------------------------------
  const sheet = () => inspector.classList.toggle("open", !!chosen);
  function choose(type, id, scroll = false) {
    chosen = type ? { type, id: Number(id) } : null;
    if (type === "spool" && !spools.some((s) => s.id === Number(id))) { toast(`No spool ${code(id)}`, "bad"); chosen = null; }
    render();
    sheet();
    if (scroll) $(".row.chosen, .group.chosen")?.scrollIntoView({ block: "nearest" });
  }

  function renderInspector() {
    if (!chosen) { inspector.innerHTML = `<p class="empty">Choose a spool, a filament or a vendor.</p>`; return; }
    const close = `<button type="button" class="sheet-close" data-unpick aria-label="Close"></button>`;
    if (chosen.type === "spool") {
      const s = spools.find((x) => x.id === chosen.id);
      if (!s) { chosen = null; return renderInspector(); }
      inspector.innerHTML = close + spoolInspector(s);
      loadHistory(s);
    } else if (chosen.type === "filament") {
      const f = filament(chosen.id);
      if (!f) { chosen = null; return renderInspector(); }
      inspector.innerHTML = close + filamentInspector(f);
    } else {
      const v = vendors.find((x) => x.id === chosen.id);
      if (!v) { chosen = null; return renderInspector(); }
      inspector.innerHTML = close + vendorInspector(v);
    }
  }

  // An input for an extra field, by its type; its value as Spoolman keeps it (JSON text).
  function extraInput(field, owner) {
    const key = field.key, v = extra(owner, key);
    const name = `extra:${key}`;
    const unit = field.unit ? ` <small class="muted">${esc(field.unit)}</small>` : "";
    switch (field.field_type) {
      case "boolean": return `<input type="checkbox" name="${name}"${v ? " checked" : ""}>`;
      case "integer": case "float": return `<input type="number" step="${field.field_type === "integer" ? 1 : "any"}" name="${name}" value="${esc(v ?? "")}">${unit}`;
      case "datetime": return `<input type="datetime-local" name="${name}" value="${esc(v ? new Date(v).toISOString().slice(0, 16) : "")}">`;
      case "choice": return `<select name="${name}"${field.multi_choice ? " multiple" : ""}><option value=""></option>${(field.choices || []).map((c) => `<option${(Array.isArray(v) ? v.includes(c) : v === c) ? " selected" : ""}>${esc(c)}</option>`).join("")}</select>`;
      default: return `<input name="${name}" value="${esc(Array.isArray(v) ? v.join(" – ") : v ?? "")}">${unit}`;
    }
  }

  function spoolInspector(s) {
    const f = s.filament, unit = unitOf(f);
    const empty = s.spool_weight ?? f.spool_weight ?? f.vendor?.empty_spool_weight;
    const p = share(s);
    const fits = machines.filter((m) => kindOf(f) === "tape" ? m.kind === "label" : kindOf(f) === "filament" ? m.kind === "fdm" : true);
    return `<div class="i-head">${swatch(f, true)}<div><h3><code>${code(s.id)}</code> ${esc(nameOf(f))}</h3><small>${esc(facts(f))}${s.archived ? " · used up" : ""}</small></div></div>
      <div class="i-left"><b>${amount(leftOf(s), unit)}</b><span>left${s.initial_weight ? ` of ${amount(s.initial_weight, unit)}${p != null ? ` · ${Math.round(p * 100)} %` : ""}` : ""}</span></div>
      ${bar(s)}
      <p class="hint">${esc(whereOf(s))} · first used ${ago(s.first_used)} · last used ${ago(s.last_used)}</p>
      <div class="i-section"><h4>Weigh, use, move</h4>
        ${unit === "g" ? `<form class="act" data-act="measure"><input name="weight" type="number" step="any" min="0" placeholder="${empty != null && leftOf(s) != null ? Math.round(leftOf(s) + empty) : "gross g"}" aria-label="Weight on the scale, in g" required><button>Weigh</button><span class="hint">on the scale, with its spool${empty != null ? ` (${Math.round(empty)} g, taken off)` : ""}</span></form>` : ""}
        <form class="act" data-act="use"><input name="use" type="number" step="any" min="0" placeholder="${unit}" aria-label="Used, in ${unit}" required><button class="ghost">Used</button><span class="hint">taken off what is left</span></form>
        <form class="act" data-act="move"><select name="location" aria-label="Move to"><option value="">No place</option>${places.map((pl) => `<option${pl.name === s.location ? " selected" : ""}>${esc(pl.name)}</option>`).join("")}<option value="+">New place…</option></select><button class="ghost">Move</button></form>
        ${fits.length ? `<form class="act" data-act="load"><select name="machine" aria-label="Load into">${fits.map((m) => `<option value="${esc(m.id)}"${m.name === loadedIn(s) ? " selected" : ""}>${esc(m.name)}</option>`).join("")}</select><button class="ghost">Load</button></form>` : ""}
        <div class="act"><button type="button" class="ghost" data-print-label>Print its label</button><button type="button" class="ghost" data-archive>${s.archived ? "Not used up" : "Used up"}</button></div>
      </div>
      <form class="i-section" data-patch="spool:${s.id}"><h4>Details</h4><div class="fields">
        <label>Lot</label><input name="lot_nr" value="${esc(s.lot_nr ?? "")}">
        <label>Came with</label><span><input name="initial_weight" type="number" step="any" min="0" value="${esc(s.initial_weight ?? "")}"> <small class="muted">${unit}</small></span>
        ${unit === "g" ? `<label>Empty spool</label><span><input name="spool_weight" type="number" step="any" min="0" value="${esc(s.spool_weight ?? "")}" placeholder="${esc(f.spool_weight ?? f.vendor?.empty_spool_weight ?? "")}"> <small class="muted">g</small></span>` : ""}
        <label>Price</label><input name="price" type="number" step="any" min="0" value="${esc(s.price ?? "")}">
        <label>Note</label><textarea name="comment">${esc(s.comment ?? "")}</textarea>
        ${fields.spool.map((fl) => `<label>${esc(fl.name)}</label><span>${extraInput(fl, s)}</span>`).join("")}
      </div></form>
      <div class="i-section"><h4>Tags</h4>${tagsBlock("spool", s)}</div>
      <div class="i-section"><h4>History</h4><ul class="history" data-history><li class="muted">…</li></ul></div>
      <div class="i-section"><div class="act"><button type="button" class="link" data-pick="filament:${f.id}">Its filament</button>${!s.archived ? "" : `<button type="button" class="link" data-delete="spool:${s.id}">Remove for good</button>`}</div></div>`;
  }

  function tagsBlock(type, owner) {
    const tags = owner.tags || [];
    return `<div class="chips">${tags.map((t) => `<span class="chip"><code>${esc(t.uid)}</code>${t.format ? `<small class="muted">${esc(t.format)}</small>` : ""}<button type="button" data-untag="${type}:${owner.id}:${esc(t.uid)}" aria-label="Remove tag">✕</button></span>`).join("") || '<span class="muted">No NFC tag. A scale or reader links one, or add its UID:</span>'}</div>
      <form class="act" data-tag="${type}:${owner.id}" style="margin-top:8px"><input name="uid" placeholder="04:A2:B3:C4…" aria-label="Tag UID" required><select name="format" aria-label="Kind of tag"><option value="">kind</option>${["ntag", "openprinttag", "bambu", "tigertag", "creality", "prusa", "qidi"].map((x) => `<option>${x}</option>`).join("")}</select><button class="ghost">Add tag</button></form>`;
  }

  function filamentInspector(f) {
    const own = spools.filter((s) => s.filament.id === f.id);
    const live = own.filter((s) => !s.archived);
    const unit = unitOf(f);
    const db = f.external_id ? `<span class="badge db">SpoolmanDB</span>` : "";
    return `<div class="i-head">${swatch(f, true)}<div><h3>${esc(nameOf(f))} ${db}</h3><small>${esc(facts(f))}</small></div></div>
      <div class="i-left"><b>${amount(live.reduce((t, s) => t + (leftOf(s) ?? 0), 0), unit)}</b><span>in ${live.length} ${live.length === 1 ? "spool" : "spools"}</span></div>
      <div class="act"><button type="button" data-add-of="${f.id}">Add spools of it</button></div>
      <form class="i-section" data-patch="filament:${f.id}"><h4>Filament</h4><div class="fields">
        <label>Name</label><input name="name" value="${esc(f.name ?? "")}">
        <label>Vendor</label><select name="vendor_id"><option value="">None</option>${vendors.map((v) => `<option value="${v.id}"${v.id === f.vendor?.id ? " selected" : ""}>${esc(v.name)}</option>`).join("")}</select>
        <label>Material</label><input name="material" list="materials-list" value="${esc(f.material ?? "")}">
        <label>Colour</label><span class="act" style="margin:0"><input type="color" name="color_hex" value="#${esc((f.color_hex || "999999").slice(0, 6))}"><input name="extra:color_name" value="${esc(colourName(f) ?? "")}" placeholder="Galaxy Black"></span>
        ${kindOf(f) === "filament" ? `<label>Diameter</label><span><input name="diameter" type="number" step="any" value="${esc(f.diameter)}"> <small class="muted">mm</small></span>
        <label>Density</label><span><input name="density" type="number" step="any" value="${esc(f.density)}"> <small class="muted">g/cm³</small></span>` : ""}
        <label>One holds</label><span><input name="weight" type="number" step="any" value="${esc(f.weight ?? "")}"> <small class="muted">${unit}</small></span>
        ${unit === "g" ? `<label>Empty spool</label><span><input name="spool_weight" type="number" step="any" value="${esc(f.spool_weight ?? "")}"> <small class="muted">g</small></span>
        <label>Nozzle</label><span><input name="settings_extruder_temp" type="number" value="${esc(f.settings_extruder_temp ?? "")}"> <small class="muted">°C</small></span>
        <label>Bed</label><span><input name="settings_bed_temp" type="number" value="${esc(f.settings_bed_temp ?? "")}"> <small class="muted">°C</small></span>` : ""}
        <label>Keep at least</label><span><input name="extra:minimum" type="number" min="0" value="${esc(extra(f, "minimum") ?? "")}"> <small class="muted">spools</small></span>
        <label>Article / EAN</label><input name="article_number" value="${esc(f.article_number ?? "")}">
        <label>Price</label><input name="price" type="number" step="any" value="${esc(f.price ?? "")}">
        <label>Note</label><textarea name="comment">${esc(f.comment ?? "")}</textarea>
        ${fields.filament.map((fl) => `<label>${esc(fl.name)}</label><span>${extraInput(fl, f)}</span>`).join("")}
      </div></form>
      <div class="i-section"><h4>Tags</h4>${tagsBlock("filament", f)}</div>
      ${own.length ? "" : `<div class="i-section"><button type="button" class="link" data-delete="filament:${f.id}">Remove this filament</button></div>`}`;
  }

  function vendorInspector(v) {
    const of = filaments.filter((f) => f.vendor?.id === v.id);
    return `<div class="i-head"><div><h3>${esc(v.name)}</h3><small>${of.length} filaments${v.external_id ? " · SpoolmanDB" : ""}</small></div></div>
      <form class="i-section" data-patch="vendor:${v.id}"><h4>Vendor</h4><div class="fields">
        <label>Name</label><input name="name" value="${esc(v.name)}">
        <label>Empty spool</label><span><input name="empty_spool_weight" type="number" step="any" value="${esc(v.empty_spool_weight ?? "")}"> <small class="muted">g</small></span>
        <label>Note</label><textarea name="comment">${esc(v.comment ?? "")}</textarea>
      </div></form>
      <div class="i-section"><h4>Filaments</h4><ul class="history">${of.map((f) => `<li><button type="button" class="link" data-pick="filament:${f.id}">${esc(f.name || f.material)}</button> <span class="muted">${esc(facts(f))}</span></li>`).join("")}</ul></div>
      ${of.length ? "" : `<div class="i-section"><button type="button" class="link" data-delete="vendor:${v.id}">Remove this vendor</button></div>`}`;
  }

  async function loadHistory(s) {
    const list = $("[data-history]", inspector);
    if (!list) return;
    try {
      const history = await api("GET", `api/stock/items/${code(s.id)}/movements`);
      const unit = unitOf(s.filament);
      list.innerHTML = history.map((h) => `<li><b>${esc(h.kind)}</b>${h.amount != null && h.kind !== "received" ? ` ${h.amount > 0 ? "+" : ""}${amount(h.amount, unit)}` : ""}${h.place ? ` · ${esc(places.find((p) => p.id === h.place)?.name || machines.find((m) => m.id === h.place)?.name || h.place)}` : ""}${h.note ? ` · ${esc(h.note)}` : ""} <span class="muted">${new Date(h.at * 1000).toLocaleString()}</span></li>`).join("") || '<li class="muted">Nothing yet</li>';
    } catch (e) { list.innerHTML = ""; }
  }

  // Edits save as they are made: the field that changed, and nothing else.
  inspector.addEventListener("change", async (e) => {
    const form = e.target.closest("[data-patch]");
    if (!form) return;
    const [type, id] = form.dataset.patch.split(":");
    const owner = type === "spool" ? spools.find((s) => s.id === Number(id)) : type === "filament" ? filament(Number(id)) : vendors.find((v) => v.id === Number(id));
    const el = e.target, name = el.name;
    let body;
    if (name.startsWith("extra:")) {
      const key = name.slice(6);
      let value = el.type === "checkbox" ? el.checked : el.type === "number" ? (el.value === "" ? null : Number(el.value))
        : el.type === "datetime-local" ? (el.value ? new Date(el.value).toISOString() : null)
        : el.multiple ? [...el.selectedOptions].map((o) => o.value).filter(Boolean) : el.value.trim() || null;
      const all = { ...(owner.extra || {}) };
      delete all.loaded_in;
      if (value == null || value === "") delete all[key]; else all[key] = JSON.stringify(value);
      body = { extra: all };
    } else if (el.type === "number" || name === "vendor_id") {
      body = { [name]: el.value === "" ? null : Number(el.value) };
    } else if (name === "color_hex") {
      body = { color_hex: el.value.replace("#", "") };
    } else {
      body = { [name]: el.value.trim() || null };
    }
    try { await api("PATCH", `/${type}/${id}`, body); toast("Saved"); } catch (error) { toast(error.message, "bad"); }
  });

  inspector.addEventListener("submit", async (e) => {
    e.preventDefault();
    const form = e.target;
    const s = chosen?.type === "spool" ? spools.find((x) => x.id === chosen.id) : null;
    const v = Object.fromEntries(new FormData(form));
    try {
      if (form.dataset.act === "measure") { const r = await api("PUT", `/spool/${s.id}/measure`, { weight: Number(v.weight) }); toast(`${amount(r.remaining_weight)} left`); }
      if (form.dataset.act === "use") { const r = await api("PUT", `/spool/${s.id}/use`, { use_weight: Number(v.use) }); toast(`${amount(r.remaining_weight, unitOf(s.filament))} left`); }
      if (form.dataset.act === "move") {
        let place = v.location;
        if (place === "+") { place = prompt("The new place: a shelf, a drawer, a dry box"); if (!place) return; }
        await api("PATCH", `/spool/${s.id}`, { location: place || null });
        toast(place ? `Moved to ${place}` : "Moved out of its place");
      }
      if (form.dataset.act === "load") { await api("POST", `api/stock/items/${code(s.id)}/load`, { machine: v.machine }); toast(`Loaded into ${machines.find((m) => m.id === v.machine)?.name}`); }
      if (form.dataset.tag) {
        const [type, id] = form.dataset.tag.split(":");
        await api("POST", `/${type}/${id}/tag`, { uid: v.uid, format: v.format || undefined });
        toast("Tag added");
      }
    } catch (error) { toast(error.message, "bad"); }
  });

  // ---- clicks -------------------------------------------------------------------------------
  document.addEventListener("click", async (e) => {
    const t = e.target;
    const pick = t.closest("[data-pick]");
    if (pick && !t.closest("button:not([data-pick]), input, select, textarea, form")) {
      const [type, id] = pick.dataset.pick.split(":");
      const head = t.closest("[data-group-key]");
      if (head && chosen?.type === type && chosen.id === Number(id)) {
        const key = head.dataset.groupKey;
        if (ui.collapsed.has(key)) ui.collapsed.delete(key); else ui.collapsed.add(key);
        remember();
      }
      if (ui.view === "locations") switchView("library");
      return choose(type, id);
    }
    const head = t.closest("[data-group-key]");
    if (head) { const key = head.dataset.groupKey; if (ui.collapsed.has(key)) ui.collapsed.delete(key); else ui.collapsed.add(key); remember(); return render(); }
    if (t.closest("[data-unpick]")) return choose(null);
    if (t.closest("[data-add-spools]")) return addSpools();
    const of = t.closest("[data-add-of]");
    if (of) return addSpools(filament(Number(of.dataset.addOf)));
    if (t.closest("[data-import]")) return importDialog();
    if (t.closest("[data-settings]")) return settingsDialog();
    const view = t.closest("[data-view]");
    if (view) return switchView(view.dataset.view);
    const s = chosen?.type === "spool" ? spools.find((x) => x.id === chosen.id) : null;
    try {
      if (t.closest("[data-archive]") && s) { await api("PATCH", `/spool/${s.id}`, { archived: !s.archived }); toast(s.archived ? "Back in stock" : `${code(s.id)} used up`); }
      if (t.closest("[data-print-label]") && s) label(labelText(s), code(s.id));
      const untag = t.closest("[data-untag]");
      if (untag) { const [type, id, uid] = untag.dataset.untag.split(":"); await api("DELETE", `/${type}/${id}/tag/${encodeURIComponent(uid)}`); toast("Tag removed"); }
      const del = t.closest("[data-delete]");
      if (del) {
        const [type, id] = del.dataset.delete.split(":");
        if (!confirm(`Remove this ${type} for good?`)) return;
        await api("DELETE", `/${type}/${id}`);
        chosen = null;
        toast("Removed");
      }
      const labelPlace = t.closest("[data-label-place]");
      if (labelPlace) { const p = places.find((x) => x.id === labelPlace.dataset.labelPlace); if (p) label(`${p.id}\n${p.name}`, p.id); }
      if (t.closest("[data-new-place]")) {
        const name = prompt("A new place: a shelf, a drawer, a dry box");
        if (name) { await api("POST", "api/stock/locations", { name }); await load(); }
      }
    } catch (error) { toast(error.message, "bad"); }
  });

  // A spool's label text, from the template in Fields and labels.
  function labelText(s) {
    const f = s.filament;
    const values = {
      code: code(s.id), vendor: vendorOf(f), name: f.name || "", material: f.material || "",
      color: colourName(f) || "", lot: s.lot_nr || "", location: s.location || "",
      nozzle: f.settings_extruder_temp ? `${f.settings_extruder_temp} °C` : "", bed: f.settings_bed_temp ? `${f.settings_bed_temp} °C` : "",
      weight: amount(s.initial_weight ?? f.weight, unitOf(f)), diameter: f.diameter ? `${f.diameter} mm` : "",
    };
    return settings.label_template.replace(/\{(\w+)\}/g, (_, k) => values[k] ?? (extra(s, k) ?? extra(f, k) ?? "")).split("\n").map((l) => l.replace(/\s+/g, " ").trim()).filter(Boolean).join("\n");
  }

  // A label on the label printer's page: its text and a QR code leading back here.
  function label(text, c) {
    const printer = machines.find((m) => m.kind === "label");
    if (!printer) { toast("No label printer yet: add one to print labels", "bad"); return; }
    const link = `${location.origin}${location.pathname}#${c}`;
    location.href = `machines/${encodeURIComponent(printer.id)}?label=${encodeURIComponent(text)}&qr=${encodeURIComponent(link)}#label`;
  }

  // ---- toolbar ---------------------------------------------------------------------------
  const groupSel = $("[data-group]"), sortSel = $("[data-sort]"), archivedBox = $("[data-archived]"), searchBox = $("[data-search]");
  groupSel.value = ui.group; sortSel.value = ui.sort; archivedBox.checked = ui.archived;
  groupSel.addEventListener("change", () => { ui.group = groupSel.value; remember(); render(); });
  sortSel.addEventListener("change", () => { ui.sort = sortSel.value; remember(); render(); });
  archivedBox.addEventListener("change", () => { ui.archived = archivedBox.checked; remember(); render(); });
  searchBox.addEventListener("input", () => { ui.search = searchBox.value; render(); });
  searchBox.addEventListener("keydown", (e) => {
    if (e.key !== "Enter") return;
    e.preventDefault();
    if (scan(searchBox.value)) { searchBox.value = ""; ui.search = ""; }
  });
  function switchView(v) {
    ui.view = v; remember();
    for (const b of document.querySelectorAll("[data-view]")) b.setAttribute("aria-selected", String(b.dataset.view === v));
    $("[data-library-view]").hidden = v !== "library";
    $("[data-board]").hidden = v !== "locations";
    render();
  }

  // What a scan finds (true when it was one): a spool, a place (moving the open spool there),
  // a filament by its EAN, a spool by its tag. A link from a label is read for its code.
  function scan(raw) {
    let c = raw.trim();
    const linked = c.match(/[#/](S-\d+|L-\d+)$/i);
    if (linked) c = linked[1];
    // Spoolman's own labels: WEB+SPOOLMAN:S-7, or a link to /spool/show/7.
    const spoolman = c.match(/^web\+spoolman:s-(\d+)$/i) || c.match(/\/spool\/show\/(\d+)\/?$/i);
    if (spoolman) c = code(spoolman[1]);
    const n = numberOf(c);
    if (n != null) { if (ui.view !== "library") switchView("library"); choose("spool", n, true); return true; }
    if (/^L-\d+$/i.test(c)) {
      const place = places.find((p) => p.id === c.toUpperCase());
      if (!place) { toast(`No place ${c}`, "bad"); return true; }
      if (chosen?.type === "spool") {
        api("PATCH", `/spool/${chosen.id}`, { location: place.name }).then(() => toast(`Moved to ${place.name}`)).catch((e) => toast(e.message, "bad"));
      } else { searchBox.value = place.name; ui.search = place.name; render(); }
      return true;
    }
    const byEan = filaments.find((f) => f.article_number && f.article_number === c);
    if (byEan) { addSpools(byEan); return true; }
    const uid = c.replace(/[\s:_.-]/g, "").toUpperCase();
    if (/^[0-9A-F]{8,20}$/.test(uid)) {
      const tagged = spools.find((s) => (s.tags || []).some((t) => t.uid === uid));
      if (tagged) { choose("spool", tagged.id, true); return true; }
    }
    return false;
  }
  // The camera: a QR code (a label's link) or a barcode (an EAN, Code 128), read by the
  // browser's own BarcodeDetector. Browsers allow the camera on https or on localhost only.
  $("[data-camera]").addEventListener("click", async () => {
    show(`${head("Scan")}<section><video class="camera-view" data-video playsinline muted></video><p class="hint" data-camera-note>Hold a label's QR code or a barcode in view.</p></section>`);
    const note = $("[data-camera-note]", dialogBody), video = $("[data-video]", dialogBody);
    if (!("BarcodeDetector" in window)) {
      note.textContent = "This browser cannot read codes from the camera (Chrome and Edge on Android and desktop can). A phone's own camera app opens a label's QR code here directly.";
      video.hidden = true;
      return;
    }
    let stream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: "environment" } });
    } catch (e) {
      note.textContent = location.protocol === "https:" || location.hostname === "localhost"
        ? `No camera: ${e.message}` : "The browser allows the camera only on https or on localhost. A phone's own camera app opens a label's QR code here directly.";
      return;
    }
    video.srcObject = stream;
    await video.play();
    const detector = new BarcodeDetector({ formats: ["qr_code", "ean_13", "ean_8", "code_128", "upc_a", "upc_e", "data_matrix"] });
    const stop = () => stream.getTracks().forEach((t) => t.stop());
    dialog.addEventListener("close", stop, { once: true });
    const look = async () => {
      if (!dialog.open) return;
      try {
        const [hit] = await detector.detect(video);
        if (hit) {
          stop();
          dialog.close();
          if (!scan(hit.rawValue)) { searchBox.value = hit.rawValue; ui.search = hit.rawValue; render(); toast(`Read ${hit.rawValue}`); }
          return;
        }
      } catch (e) { /* the next frame */ }
      setTimeout(look, 200);
    };
    look();
  });

  // A scanner types fast and ends with Enter, wherever the cursor is.
  let typed = "", lastKey = 0;
  document.addEventListener("keydown", (e) => {
    if (e.target.closest("input, textarea, select")) return;
    const now = performance.now();
    if (now - lastKey > 80) typed = "";
    lastKey = now;
    if (e.key === "Enter" && typed.length >= 4) { e.preventDefault(); scan(typed); typed = ""; return; }
    if (e.key === "Escape" && chosen) choose(null);
    if (e.key.length === 1) typed += e.key;
  });

  // ---- the board of places -------------------------------------------------------------
  let picked = null;
  function renderBoard() {
    const live = spools.filter((s) => !s.archived);
    const card = (s) => `<div class="card-spool${picked === s.id ? " picked" : ""}" draggable="${loadedIn(s) ? "false" : "true"}" data-card="${s.id}">${swatch(s.filament)}<span><b>${code(s.id)}</b> ${esc(nameOf(s.filament))}<br><small class="muted">${amount(leftOf(s), unitOf(s.filament))}</small></span>${bar(s)}</div>`;
    // By place, or by one of the spool's own fields with one short value (text or a choice).
    const field = fields.spool.find((f) => f.key === settings.board_field && !f.multi_choice && ["text", "choice"].includes(f.field_type));
    const valueOf = (s) => field ? extra(s, field.key) ?? "" : s.location ?? "";
    const keys = field
      ? [...new Set([...(field.choices || []), ...live.map(valueOf).filter(Boolean)])].sort()
      : places.slice().sort((a, b) => a.name.localeCompare(b.name)).map((p) => p.name);
    const cols = [
      ...keys.map((k) => ({ key: k, title: k, code: field ? null : placeCode(k), spools: live.filter((s) => valueOf(s) === k && (field || !loadedIn(s))) })),
      { key: "", title: field ? `No ${field.name.toLowerCase()}` : "No place", spools: live.filter((s) => !valueOf(s) && (field || !loadedIn(s))) },
      ...(field ? [] : [{ key: null, title: "In machines", spools: live.filter((s) => loadedIn(s)) }]),
    ];
    const pickers = `<div class="col" style="place-content:start;gap:10px"><label class="pick">Board by <select data-board-field><option value="location">Place</option>${fields.spool.filter((f) => !f.multi_choice && ["text", "choice"].includes(f.field_type)).map((f) => `<option value="${esc(f.key)}"${f.key === settings.board_field ? " selected" : ""}>${esc(f.name)}</option>`).join("")}</select></label>${field ? "" : '<button type="button" class="ghost" data-new-place>New place</button>'}<p class="hint">Drag a spool to another column, or tap it, then a column.</p></div>`;
    $("[data-board]").innerHTML = cols.map((c) => `<div class="col${c.key !== null && picked ? " target" : ""}"${c.key !== null ? ` data-drop="${esc(c.key)}"` : ""}>
        <div class="col-head"><b>${esc(c.title)}</b><small>${c.code ? `<code>${esc(c.code)}</code> · ` : ""}${c.spools.length}${c.code ? ` · <button type="button" class="link" data-label-place="${esc(c.code)}">label</button>` : ""}</small></div>
        ${c.spools.map(card).join("") || '<p class="muted" style="margin:6px 4px;font-size:13px">Empty</p>'}</div>`).join("") + pickers;
  }
  const moveTo = async (id, place) => {
    try {
      const field = settings.board_field !== "location" && fields.spool.find((f) => f.key === settings.board_field);
      if (field) {
        const s = spools.find((x) => x.id === id);
        const all = { ...(s.extra || {}) };
        delete all.loaded_in;
        if (place) all[field.key] = JSON.stringify(place); else delete all[field.key];
        await api("PATCH", `/spool/${id}`, { extra: all });
      } else await api("PATCH", `/spool/${id}`, { location: place || null });
      toast(place ? `${code(id)} to ${place}` : `${code(id)}: none`);
    } catch (e) { toast(e.message, "bad"); }
  };
  const boardEl = $("[data-board]");
  boardEl.addEventListener("change", async (e) => {
    if (!e.target.matches("[data-board-field]")) return;
    settings.board_field = e.target.value;
    try { await api("POST", "/setting/board_field", JSON.stringify(settings.board_field)); } catch (err) { /* kept here */ }
    renderBoard();
  });
  const board = $("[data-board]");
  board.addEventListener("dragstart", (e) => { const c = e.target.closest("[data-card]"); if (c) e.dataTransfer.setData("text/plain", c.dataset.card); });
  board.addEventListener("dragover", (e) => { const col = e.target.closest("[data-drop]"); if (col) { e.preventDefault(); col.classList.add("over"); } });
  board.addEventListener("dragleave", (e) => e.target.closest("[data-drop]")?.classList.remove("over"));
  board.addEventListener("drop", (e) => {
    const col = e.target.closest("[data-drop]");
    if (!col) return;
    e.preventDefault();
    col.classList.remove("over");
    moveTo(Number(e.dataTransfer.getData("text/plain")), col.dataset.drop);
  });
  board.addEventListener("click", (e) => {
    const c = e.target.closest("[data-card]");
    if (c) { const id = Number(c.dataset.card); picked = picked === id ? null : id; return renderBoard(); }
    const col = e.target.closest("[data-drop]");
    if (col && picked && !e.target.closest("button")) { const id = picked; picked = null; moveTo(id, col.dataset.drop); }
  });

  // ---- adding spools ---------------------------------------------------------------------
  const head = (title) => `<div class="dialog-head"><h2>${title}</h2><button type="button" class="close" data-close aria-label="Close">✕</button></div>`;
  dialog.addEventListener("click", (e) => { if (e.target === dialog || e.target.closest("[data-close]")) dialog.close(); });
  const show = (html) => { dialogBody.innerHTML = html; if (!dialog.open) dialog.showModal(); };
  const fromDb = (x) => ({ name: x.name, material: x.material, vendor: { name: x.manufacturer }, color_hex: x.color_hex, multi_color_hexes: x.color_hexes?.join(","), diameter: x.diameter, weight: x.weight, spool_weight: x.spool_weight });
  const fromTape = (m) => ({ name: m.name, material: "TAPE", vendor: { name: m.brand }, color_hex: (m.color || "").replace("#", ""), extra: { kind: '"tape"', unit: '"m"', ink: JSON.stringify(m.ink || "#000") }, weight: m.package });

  async function addSpools(preset) {
    if (!materialsDb.length) api("GET", "/external/material").then((m) => { materialsDb = m; }).catch(() => {});
    show(`${head("Add spools")}<section>
      <div data-step-what>
        <label class="search" style="margin-bottom:4px"><svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="8.5" cy="8.5" r="5.5"/><path d="m13 13 4 4"/></svg><input data-find placeholder="Which filament? polymaker pla black, prusament petg, 45013…" autocomplete="off" aria-label="Find a filament"></label>
        <ul class="found" data-found role="listbox"></ul>
        <p class="hint">Your own filaments, your label printers' tapes, and SpoolmanDB's. Not there? <button type="button" class="link" data-new-filament>Make a new filament</button></p>
      </div>
      <form class="grid-form" data-new-form hidden style="margin-top:10px">
        <label>Vendor <input name="vendor" list="vendors-list" required placeholder="Prusament"></label>
        <label>Material <input name="material" list="materials-list" placeholder="PLA, PETG…"></label>
        <label class="wide">Name <input name="name" required placeholder="PETG Galaxy Black"></label>
        <label>Colour <input name="color" type="color" value="#888888"></label>
        <label>Colour name <input name="color_name" placeholder="Galaxy Black"></label>
        <label>Diameter (mm) <input name="diameter" type="number" step="any" value="1.75"></label>
        <label>Density (g/cm³) <input name="density" type="number" step="any" value="1.24"></label>
        <label>One spool holds (g) <input name="weight" type="number" step="any" value="1000"></label>
        <label>Empty spool (g) <input name="spool_weight" type="number" step="any"></label>
        <label>Nozzle (°C) <input name="extruder" type="number"></label>
        <label>Bed (°C) <input name="bed" type="number"></label>
      </form>
      <form class="grid-form" data-how hidden style="margin-top:12px">
        <div class="wide" data-chosen></div>
        <label>How many <input name="count" type="number" min="1" max="100" value="1"></label>
        <label>Where <input name="location" list="places-list" placeholder="Shelf B2, dry box 1"></label>
        <label>Each holds <input name="initial" type="number" step="any" min="0"></label>
        <label>Empty spool (g) <input name="spool" type="number" step="any" min="0"></label>
        <label>Lot <input name="lot"></label>
        <label>Price each <input name="price" type="number" step="any" min="0"></label>
        <div class="wide"><button data-add-button>Add</button></div>
      </form>
      <datalist id="vendors-list">${vendors.map((v) => `<option value="${esc(v.name)}">`).join("")}</datalist>
      <datalist id="places-list">${places.map((p) => `<option value="${esc(p.name)}">`).join("")}</datalist>
    </section>`);
    const find = $("[data-find]", dialogBody), found = $("[data-found]", dialogBody), how = $("[data-how]", dialogBody), newForm = $("[data-new-form]", dialogBody);
    let pickedOne = null, shown = [], at = 0, timer = null;
    const set = (o) => {
      pickedOne = o;
      const f = o.filament;
      $("[data-chosen]", dialogBody).innerHTML = `<div class="chosen-filament">${swatch(f)}<span class="f-text" style="display:grid"><b>${esc(nameOf(f))}</b><small class="muted">${esc(facts(f) || "")}${o.source === "db" ? " · from SpoolmanDB" : o.source === "tape" ? " · label tape" : ""}</small></span><button type="button" class="link" data-again>Change</button></div>`;
      how.hidden = false;
      how.initial.value = f.weight ?? "";
      how.spool.value = f.spool_weight ?? f.vendor?.empty_spool_weight ?? "";
      how.spool.closest("label").hidden = (f.extra && extra(f, "unit") && extra(f, "unit") !== "g") || o.source === "tape";
      $("[data-add-button]", dialogBody).textContent = "Add";
      $("[data-step-what]", dialogBody).hidden = true;
      newForm.hidden = true;
      how.count.focus();
    };
    const draw = () => {
      found.innerHTML = shown.map((o, i) => o.section ? `<li class="f-section">${esc(o.section)}</li>`
        : `<li role="option" data-i="${i}" aria-selected="${i === at}">${swatch(o.filament)}<span class="f-text"><b>${esc(nameOf(o.filament))}</b><small>${esc([facts(o.filament), o.filament.weight && `${amount(o.filament.weight, o.source === "tape" ? "m" : "g")}`].filter(Boolean).join(" · "))}</small></span>${o.source === "db" ? '<span class="badge db">SpoolmanDB</span>' : ""}</li>`).join("")
        || '<li class="muted" style="cursor:default">Nothing found: make a new filament below</li>';
    };
    const search = async () => {
      const q = find.value.trim().toLowerCase();
      const words = q.split(/\s+/).filter(Boolean);
      const hit = (text) => words.every((w) => text.toLowerCase().includes(w));
      const yours = filaments.filter((f) => !words.length || hit(`${nameOf(f)} ${f.material} ${colourName(f) || ""} ${f.article_number || ""}`)).slice(0, 12).map((f) => ({ filament: f, source: "own" }));
      const ownMedia = new Set(filaments.map((f) => extra(f, "media")).filter(Boolean));
      const tapeHits = words.length ? tapes.filter((m) => !ownMedia.has(m.media) && hit(`${m.brand} ${m.name} tape ${m.media}`)).slice(0, 6).map((m) => ({ filament: fromTape(m), tape: m, source: "tape" })) : [];
      let db = [];
      if (words.length) {
        try { db = (await api("GET", `/external/filament/search?q=${encodeURIComponent(q)}&limit=25`)).filter((x) => !filaments.some((f) => f.external_id === x.id)).map((x) => ({ filament: fromDb(x), db: x, source: "db" })); } catch (e) { /* offline */ }
      }
      shown = [
        ...(yours.length ? [{ section: "Yours" }, ...yours] : []),
        ...(tapeHits.length ? [{ section: "Label tapes" }, ...tapeHits] : []),
        ...(db.length ? [{ section: "SpoolmanDB" }, ...db] : []),
      ];
      at = shown.findIndex((o) => !o.section);
      draw();
    };
    find.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(search, 180); });
    find.addEventListener("keydown", (e) => {
      const step = (d) => { let i = at; do { i += d; } while (shown[i]?.section); if (shown[i]) { at = i; draw(); found.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" }); } };
      if (e.key === "ArrowDown") { e.preventDefault(); step(1); }
      if (e.key === "ArrowUp") { e.preventDefault(); step(-1); }
      if (e.key === "Enter") { e.preventDefault(); if (shown[at] && !shown[at].section) set(shown[at]); }
    });
    found.addEventListener("click", (e) => { const li = e.target.closest("[data-i]"); if (li) set(shown[Number(li.dataset.i)]); });
    dialogBody.addEventListener("click", (e) => {
      if (e.target.closest("[data-again]")) { $("[data-step-what]", dialogBody).hidden = false; how.hidden = true; newForm.hidden = true; find.focus(); }
      if (e.target.closest("[data-new-filament]")) {
        newForm.hidden = false; $("[data-step-what]", dialogBody).hidden = true; how.hidden = false;
        pickedOne = { source: "new" };
        $("[data-chosen]", dialogBody).innerHTML = "";
        $("[data-add-button]", dialogBody).textContent = "Make it and add";
        how.initial.value = 1000;
        newForm.vendor.focus();
      }
    });
    // A material from SpoolmanDB's list fills in its density and temperatures.
    newForm.material.addEventListener("change", () => {
      const m = materialsDb.find((x) => x.material.toLowerCase() === newForm.material.value.trim().toLowerCase());
      if (!m) return;
      if (m.density) newForm.density.value = m.density;
      if (m.extruder_temp && !newForm.extruder.value) newForm.extruder.value = m.extruder_temp;
      if (m.bed_temp && !newForm.bed.value) newForm.bed.value = m.bed_temp;
    });
    newForm.weight.addEventListener("input", () => { how.initial.value = newForm.weight.value; });
    how.addEventListener("submit", async (e) => {
      e.preventDefault();
      const v = Object.fromEntries(new FormData(how));
      const button = $("[data-add-button]", dialogBody);
      button.disabled = true;
      try {
        let id;
        if (pickedOne.source === "own") id = pickedOne.filament.id;
        else if (pickedOne.source === "db") id = Number((await api("POST", `api/stock/catalogue/${encodeURIComponent(pickedOne.db.id)}/take`)).id);
        else if (pickedOne.source === "tape") id = Number((await api("POST", "api/stock/materials", { ...pickedOne.tape, id: "" })).id);
        else {
          if (!newForm.reportValidity()) { button.disabled = false; return; }
          const n = Object.fromEntries(new FormData(newForm));
          let vendor = vendors.find((x) => x.name.toLowerCase() === n.vendor.trim().toLowerCase());
          if (!vendor) vendor = await api("POST", "/vendor", { name: n.vendor.trim(), empty_spool_weight: n.spool_weight ? Number(n.spool_weight) : null });
          const made = await api("POST", "/filament", {
            name: n.name.trim(), vendor_id: vendor.id, material: n.material.trim() || null, color_hex: n.color.replace("#", ""),
            diameter: Number(n.diameter) || 1.75, density: Number(n.density) || 1.24, weight: n.weight ? Number(n.weight) : null,
            spool_weight: n.spool_weight ? Number(n.spool_weight) : null,
            settings_extruder_temp: n.extruder ? Number(n.extruder) : null, settings_bed_temp: n.bed ? Number(n.bed) : null,
            extra: n.color_name.trim() ? { color_name: JSON.stringify(n.color_name.trim()) } : {},
          });
          id = made.id;
        }
        const made = [];
        for (let i = 0; i < Math.max(1, Math.min(100, Number(v.count) || 1)); i++) {
          made.push(await api("POST", "/spool", {
            filament_id: id, location: v.location.trim() || undefined,
            initial_weight: v.initial === "" ? undefined : Number(v.initial),
            spool_weight: v.spool === "" ? undefined : Number(v.spool),
            lot_nr: v.lot.trim() || undefined, price: v.price === "" ? undefined : Number(v.price),
          }));
        }
        dialog.close();
        toast(made.length === 1 ? `${code(made[0].id)} added` : `${code(made[0].id)} … ${code(made[made.length - 1].id)} added`);
        await load();
        choose("spool", made[0].id, true);
      } catch (error) { toast(error.message, "bad"); button.disabled = false; }
    });
    if (preset) set({ filament: preset, source: "own" });
    else { find.focus(); search(); }
  }

  // ---- fields and labels -------------------------------------------------------------------
  const TYPES = { text: "Text", integer: "Whole number", float: "Number", datetime: "Date and time", boolean: "Yes or no", choice: "Choice", integer_range: "Range of whole numbers", float_range: "Range of numbers" };
  function settingsDialog() {
    const list = (entity) => (fields[entity].map((f) => `<li><b>${esc(f.name)}</b> <code>${esc(f.key)}</code> <span class="muted">${esc(TYPES[f.field_type] || f.field_type)}${f.unit ? `, ${esc(f.unit)}` : ""}${f.choices?.length ? `: ${esc(f.choices.join(", "))}` : ""}</span> <button type="button" class="link" data-drop-field="${entity}:${esc(f.key)}">remove</button></li>`).join("") || '<li class="muted">None yet</li>');
    show(`${head("Fields and labels")}<section>
      <h3 style="margin:0 0 6px;font-size:15px">Your own fields</h3>
      <p class="hint">Asked of every spool or filament, beside the usual ones: a drying date, a hardness, a shop. Spoolman and its scales see them too.</p>
      <h4 class="muted" style="margin:12px 0 4px">Spools</h4><ul class="history">${list("spool")}</ul>
      <h4 class="muted" style="margin:12px 0 4px">Filaments</h4><ul class="history">${list("filament")}</ul>
      <form class="grid-form" data-new-field style="margin-top:12px">
        <label>For <select name="entity"><option value="spool">Spools</option><option value="filament">Filaments</option></select></label>
        <label>Kind <select name="type">${Object.entries(TYPES).map(([k, t]) => `<option value="${k}">${t}</option>`).join("")}</select></label>
        <label>Name <input name="name" required placeholder="Last dried"></label>
        <label>Unit <input name="unit" placeholder="°C, h, Shore"></label>
        <label class="wide">Choices (for a choice, comma-separated) <input name="choices" placeholder="open, sealed, in the dryer"></label>
        <div class="wide"><button>Add field</button></div>
      </form>
      <h3 style="margin:22px 0 6px;font-size:15px">A spool's label</h3>
      <form data-label-form><textarea name="template" rows="3" style="width:100%;font:13px ui-monospace,monospace">${esc(settings.label_template)}</textarea>
        <p class="hint">One line each. Fill in: {code} {vendor} {name} {material} {color} {diameter} {weight} {nozzle} {bed} {lot} {location}, and your fields by their key. The QR code beside it opens the spool here.</p>
        <button class="ghost">Save the label</button></form>
    </section>`);
    $("[data-new-field]", dialogBody).addEventListener("submit", async (e) => {
      e.preventDefault();
      const v = Object.fromEntries(new FormData(e.target));
      const key = v.name.trim().toLowerCase().normalize("NFD").replace(/[^a-z0-9]+/g, "_").replace(/^_|_$/g, "") || "field";
      try {
        await api("POST", `/field/${v.entity}/${key}`, {
          name: v.name.trim(), field_type: v.type, unit: v.unit.trim() || null, order: fields[v.entity].length,
          choices: v.type === "choice" ? v.choices.split(",").map((c) => c.trim()).filter(Boolean) : null,
          multi_choice: v.type === "choice" ? false : null,
        });
        await load();
        settingsDialog();
      } catch (error) { toast(error.message, "bad"); }
    });
    $("[data-label-form]", dialogBody).addEventListener("submit", async (e) => {
      e.preventDefault();
      settings.label_template = e.target.template.value;
      try { await api("POST", "/setting/label_template", JSON.stringify(settings.label_template)); toast("Label saved"); } catch (error) { toast(error.message, "bad"); }
    });
  }
  dialog.addEventListener("click", async (e) => {
    const drop = e.target.closest("[data-drop-field]");
    if (!drop) return;
    const [entity, key] = drop.dataset.dropField.split(":");
    if (!confirm(`Remove the field ${key}, and its values?`)) return;
    try { await api("DELETE", `/field/${entity}/${key}`); await load(); settingsDialog(); } catch (error) { toast(error.message, "bad"); }
  });

  // ---- moving to and from a Spoolman ------------------------------------------------------
  function importDialog() {
    show(`${head("Spoolman")}<section>
      <h3 style="margin:0 0 6px;font-size:15px">Bring a Spoolman's spools here</h3>
      <form class="act" data-from-url><input name="url" placeholder="http://192.168.1.20:7912" required style="flex:1" aria-label="Spoolman's address"><button>Import</button></form>
      <form class="act" data-from-file><input type="file" name="file" accept="application/json,.json" aria-label="Spoolman's JSON export of spools"><button class="ghost">Import its export</button></form>
      <p class="hint">Its vendors, filaments, spools, tags and extra fields. Spools keep their numbers where they are free, so their labels and tags still find them.</p>
      <h3 style="margin:18px 0 6px;font-size:15px">Take them along</h3>
      <div class="act"><a class="button ghost" href="api/v1/export/spools?fmt=csv" download>Spools as CSV</a><a class="button ghost" href="api/v1/export/spools?fmt=json" download>Spools as JSON</a></div>
      <form class="act" data-to-url><input name="url" placeholder="http://192.168.1.20:7912" required style="flex:1" aria-label="Spoolman to copy to"><button class="ghost">Copy to that Spoolman</button></form>
      <p class="hint">Moonraker, OctoPrint's Spoolman plugin and scales such as SpoolmanScale can use this cloud as their Spoolman: give them <code>${esc(location.origin)}</code>.</p>
      <p class="note" data-import-note></p>
    </section>`);
    const note = $("[data-import-note]", dialogBody);
    const done = (m) => { note.textContent = `${m.vendors} vendors, ${m.filaments} filaments, ${m.spools} spools${m.tags != null ? `, ${m.tags} tags, ${m.fields} fields` : ""}.`; load(); };
    $("[data-from-url]", dialogBody).addEventListener("submit", async (e) => {
      e.preventDefault(); note.textContent = "Importing…";
      try { done(await api("POST", "api/stock/import/spoolman", { url: e.target.url.value.trim() })); } catch (error) { note.textContent = error.message; }
    });
    $("[data-from-file]", dialogBody).addEventListener("submit", async (e) => {
      e.preventDefault();
      const file = e.target.file.files[0];
      if (!file) return;
      note.textContent = "Importing…";
      try { done(await api("POST", "api/stock/import/spoolman", JSON.parse(await file.text()))); } catch (error) { note.textContent = error.message; }
    });
    $("[data-to-url]", dialogBody).addEventListener("submit", async (e) => {
      e.preventDefault(); note.textContent = "Copying…";
      try { done(await api("POST", "api/stock/export/spoolman", { url: e.target.url.value.trim() })); } catch (error) { note.textContent = error.message; }
    });
  }

  // ---- live ----------------------------------------------------------------------------------
  // Spoolman's own WebSockets: anything added, changed or removed, from here or from a scale,
  // Moonraker, another browser.
  let reloading = null;
  const later = () => { clearTimeout(reloading); reloading = setTimeout(() => load().catch(() => {}), 250); };
  function listen(resource) {
    const ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}${location.pathname.replace(/stock$/, "")}api/v1/${resource}`);
    ws.onmessage = later;
    ws.onclose = () => setTimeout(() => listen(resource), 3000);
  }
  ["spool", "filament", "vendor"].forEach(listen);

  const stream = new EventSource("api/machines/stream");
  stream.onopen = () => live(true);
  stream.onerror = () => live(false);
  stream.onmessage = (e) => {
    const all = JSON.parse(e.data);
    summarise(all);
    const next = all.map((m) => ({ id: m.id, name: m.name, kind: m.kind }));
    if (JSON.stringify(next) !== JSON.stringify(machines)) { machines = next; if (chosen) renderInspector(); }
  };

  document.body.insertAdjacentHTML("beforeend", '<datalist id="materials-list"></datalist>');
  api("GET", "/external/material").then((m) => { materialsDb = m; $("#materials-list").innerHTML = m.map((x) => `<option value="${esc(x.material)}">`).join(""); }).catch(() => {});
  switchView(ui.view);
  load().then(() => {
    // Opened from a label's QR code: what it names.
    const c = decodeURIComponent(location.hash.slice(1));
    if (c) { history.replaceState(null, "", location.pathname); scan(c); }
  }).catch((e) => { $("[data-list]").innerHTML = `<p class="empty">Not loaded: ${esc(e.message)}</p>`; });
