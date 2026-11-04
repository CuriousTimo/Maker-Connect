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
  let dryboxes = [];
  let currency = "€";
  let settings = { label_template: "{code}\n{vendor} {name}\n{material} {color}", board_field: "location" };
  let machines = [];
  let chosen = null; // { type: "spool" | "filament" | "vendor", id }
  const ui = { view: "library", group: "filament", sort: "name", search: "", archived: false, collapsed: new Set(), filters: { material: "", vendor: "", location: "" } };
  try { Object.assign(ui, JSON.parse(localStorage.getItem("library-ui") || "{}"), { collapsed: new Set(JSON.parse(localStorage.getItem("library-collapsed") || "[]")) }); } catch (e) { /* defaults */ }
  const remember = () => {
    try {
      localStorage.setItem("library-ui", JSON.stringify({ view: ui.view, group: ui.group, sort: ui.sort, archived: ui.archived, filters: ui.filters }));
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
  // Where it is, and the machine it feeds from there: "RepBox v2.5 · 3 → MINI".
  const whereOf = (s) => {
    const place = s.location ? `${s.location}${extra(s, "slot") ? ` · ${extra(s, "slot")}` : ""}` : "";
    return loadedIn(s) ? `${place ? `${place} → ` : "in "}${loadedIn(s)}` : place || "no location";
  };
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
  const placeOf = (name) => places.find((p) => p.name === name);
  const slotOf = (s) => extra(s, "slot");
  // A spool to a place, and a slot in it (a dry box's), in one change.
  async function place(id, name, slot) {
    const s = spools.find((x) => x.id === id);
    const body = { location: name || null };
    if (slot) {
      const all = { ...(s?.extra || {}) };
      delete all.loaded_in;
      all.slot = JSON.stringify(Number(slot));
      body.extra = all;
    }
    await api("PATCH", `/spool/${id}`, body);
  }


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
    dryboxes = stock.dryboxes || [];
    try {
      const all = await api("GET", "/setting/");
      for (const key of ["label_template", "board_field"]) if (all[key]) settings[key] = JSON.parse(all[key].value);
      const c = all.currency ? JSON.parse(all.currency.value) : "EUR";
      currency = { EUR: "€", USD: "$", GBP: "£" }[c] || c;
    } catch (e) { /* defaults */ }
    tapes = stock.catalogue.filter((m) => m.kind === "tape");
    render();
  }

  // ---- the list -------------------------------------------------------------------------
  // A colour word ("red", "groen") matches filaments of that colour, by their hex.
  const COLOURS = {
    red: [345, 15], rood: [345, 15], orange: [15, 40], oranje: [15, 40], yellow: [40, 70], geel: [40, 70],
    green: [70, 170], groen: [70, 170], blue: [170, 255], blauw: [170, 255], purple: [255, 300], paars: [255, 300],
    pink: [300, 345], roze: [300, 345],
  };
  const NEUTRALS = { black: [0, 0.18], zwart: [0, 0.18], white: [0.82, 1], wit: [0.82, 1], gray: [0.18, 0.82], grey: [0.18, 0.82], grijs: [0.18, 0.82] };
  function hsl(hex) {
    const n = parseInt(hex.slice(0, 6), 16);
    const r = ((n >> 16) & 255) / 255, g = ((n >> 8) & 255) / 255, b = (n & 255) / 255;
    const max = Math.max(r, g, b), min = Math.min(r, g, b), l = (max + min) / 2, d = max - min;
    const sat = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
    let h = 0;
    if (d) h = max === r ? 60 * (((g - b) / d) % 6) : max === g ? 60 * ((b - r) / d + 2) : 60 * ((r - g) / d + 4);
    return [(h + 360) % 360, sat, l];
  }
  function colourWord(word, f) {
    const hexes = (f?.multi_color_hexes ? f.multi_color_hexes.split(",") : f?.color_hex ? [f.color_hex] : []).map((h) => h.replace("#", ""));
    if (!hexes.length) return false;
    return hexes.some((hex) => {
      const [h, sat, l] = hsl(hex);
      if (NEUTRALS[word]) { const [lo, hi] = NEUTRALS[word]; return sat < 0.25 && l >= lo && l <= hi || (word.startsWith("bl") || word === "zwart") && l < 0.12; }
      const [from, to] = COLOURS[word];
      const inHue = from > to ? h >= from || h < to : h >= from && h < to;
      return inHue && sat >= 0.25 && l > 0.12 && l < 0.9;
    });
  }
  function matching() {
    const words = ui.search.toLowerCase().split(/\s+/).filter(Boolean);
    const { material, vendor, location } = ui.filters;
    return spools.filter((s) => {
      if (s.archived && !ui.archived) return false;
      const f = s.filament;
      if (material && f?.material !== material) return false;
      if (vendor && vendorOf(f) !== vendor) return false;
      if (location && (location === "-" ? s.location || loadedIn(s) : location === "+" ? !loadedIn(s) : s.location !== location)) return false;
      if (!words.length) return true;
      const hay = [code(s.id), `#${s.id}`, nameOf(f), f?.material, colourName(f), s.location, s.lot_nr, loadedIn(s), f?.article_number, ...(s.tags || []).map((t) => t.uid)].join(" ").toLowerCase();
      return words.every((w) => hay.includes(w) || ((COLOURS[w] || NEUTRALS[w]) && colourWord(w, f)));
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
      if (ui.group === "filament") g = add(`f${f.id}`, f.name || f.material || "Filament", [vendorOf(f), kindOf(f) === "filament" && f.diameter && `${f.diameter} mm`, colourName(f)].filter(Boolean).join(" · "), f, "filament", f.id);
      else if (ui.group === "vendor") g = add(`v${f.vendor?.id ?? 0}`, vendorOf(f) || "No vendor", "", null, f.vendor ? "vendor" : null, f.vendor?.id);
      else if (ui.group === "location") g = add(`l${loadedIn(s) ? "m" + loadedIn(s) : s.location || ""}`, loadedIn(s) ? `In ${loadedIn(s)}` : s.location || "No location", "", null, null);
      else if (ui.group === "material") g = add(`m${f.material || ""}`, f.material || "No material", "", null, null);
      else g = add("all", "", "", null, null);
      g.spools.push(s);
    }
    // Filaments without a spool (yet) are there too, when nothing is searched.
    if (ui.group === "filament" && !ui.search && !Object.values(ui.filters).some(Boolean)) for (const f of filaments) add(`f${f.id}`, f.name || f.material || "Filament", [vendorOf(f), kindOf(f) === "filament" && f.diameter && `${f.diameter} mm`, colourName(f)].filter(Boolean).join(" · "), f, "filament", f.id);
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
        const chip = g.swatchFor?.material ? `<span class="chip-material">${esc(g.swatchFor.material)}</span>` : "";
        const head = ui.group === "none" ? "" : `<div class="group-head" data-group-key="${esc(g.key)}"${g.kind ? ` data-pick="${g.kind}:${g.id}"` : ""}>
            ${g.swatchFor ? swatch(g.swatchFor) : `<span class="badge">${live.length}</span>`}
            <span class="g-name"><b>${esc(g.title)} ${chip}</b>${g.sub ? `<small>${esc(g.sub)}</small>` : ""}</span>
            <span class="g-total"><b>${amount(total, unit)}</b><small class="${low ? "low" : ""}">${live.length} ${live.length === 1 ? "spool" : "spools"}${low ? " · low" : ""}</small></span>
            ${g.spools.length ? `<button type="button" class="fold${open ? " open" : ""}" data-fold="${esc(g.key)}" aria-label="${open ? "Fold" : "Unfold"}" aria-expanded="${open}"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="m6 8 4 4 4-4"/></svg></button>` : "<span></span>"}</div>`;
        const rows = (open || ui.group === "none") && g.spools.length ? `<ul class="rows">${g.spools.map((s) => {
            const p = share(s);
            return `<li class="row${s.archived ? " archived" : ""}${chosen?.type === "spool" && chosen.id === s.id ? " chosen" : ""}" data-pick="spool:${s.id}">
            <code>${code(s.id)}</code>
            <span class="used">${ui.group !== "filament" ? esc(nameOf(s.filament)) : s.archived ? "used up" : s.last_used ? `used ${ago(s.last_used)}` : "unused"}</span>
            ${bar(s)}
            <span class="left${p != null && p < 0.15 ? " low" : ""}">${amount(leftOf(s), unitOf(s.filament))}</span>
            <span class="where${loadedIn(s) ? " loaded" : ""}">${esc(s.archived ? "" : loadedIn(s) ? (s.location ? `${s.location} → ${loadedIn(s)}` : loadedIn(s)) : s.location || "")}</span></li>`;
          }).join("")}</ul>` : "";
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

  // An editable value in the text, saved as it changes: a dashed line, no box.
  const editable = (name, value, opts = {}) => {
    const type = opts.type || "text";
    const v = value ?? "";
    const input = type === "textarea"
      ? `<textarea class="inline" name="${name}" rows="1" placeholder="${esc(opts.placeholder || "–")}">${esc(v)}</textarea>`
      : `<input class="inline" name="${name}" type="${type}"${type === "number" ? ' step="any"' : ""} value="${esc(v)}" placeholder="${esc(opts.placeholder || "–")}">`;
    return opts.unit ? `<span class="with-unit">${input}<small>${esc(opts.unit)}</small></span>` : input;
  };
  const localTime = (iso) => iso ? new Date(Date.parse(iso) - new Date().getTimezoneOffset() * 60000).toISOString().slice(0, 16) : "";
  const fact = (label, value) => value == null || value === "" ? "" : `<label>${label}</label><span class="value">${value}</span>`;

  function spoolInspector(s) {
    const f = s.filament, unit = unitOf(f);
    const p = share(s);
    const used = s.used_weight;
    const metres = unit === "g" && s.remaining_length != null ? ` · ${Math.round(s.remaining_length / 1000)} m` : "";
    const fits = machines.filter((m) => kindOf(f) === "tape" ? m.kind === "label" : kindOf(f) === "filament" ? m.kind === "fdm" : true);
    const status = s.archived ? "used up" : esc(whereOf(s));
    const crumbs = `<nav class="crumbs">${f.vendor ? `<button type="button" class="link" data-pick="vendor:${f.vendor.id}">${esc(f.vendor.name)}</button> › ` : ""}<button type="button" class="link" data-pick="filament:${f.id}">${esc(f.name || f.material || "Filament")}</button> › <span>${code(s.id)}</span></nav>`;
    return `${crumbs}
      <div class="i-top">
        <div class="i-head">${swatch(f, true)}<div><h3>${esc(nameOf(f))} <small class="muted">${code(s.id)}</small></h3><small>${esc([f.material, kindOf(f) === "filament" && f.diameter && `${f.diameter} mm`].filter(Boolean).join(" · "))} · ${status}</small></div></div>
        <div class="i-actions"><button type="button" class="ghost" data-adjust>Adjust weight</button><button type="button" class="ghost" data-print-label>Print label</button><button type="button" class="ghost" data-clone title="A new, full spool like this one">Duplicate</button><button type="button" class="ghost" data-archive>${s.archived ? "Unarchive" : "Archive"}</button></div>
      </div>
      <div class="i-left"><b>${amount(leftOf(s), unit)}</b><span>${s.initial_weight ? `of ${amount(s.initial_weight, unit)} remaining` : "remaining"}${metres}${p != null ? ` · ${Math.round(p * 100)} %` : ""}</span><span class="i-used">used ${amount(used, unit)}</span></div>
      <div class="bar wide${p != null && p < 0.15 ? " low" : ""}"><i style="width:${p == null ? 0 : Math.round(p * 100)}%"></i></div>
      <div class="i-cols">
        <form data-patch="spool:${s.id}">
          <h4>Spool</h4>
          <div class="fields">
            <label>Location</label><select class="inline" name="location">${`<option value="">No location</option>` + places.map((pl) => `<option${pl.name === s.location ? " selected" : ""}>${esc(pl.name)}</option>`).join("")}<option value="+">New location…</option></select>
            ${placeOf(s.location)?.slots ? `<label>Slot</label><select class="inline" name="slot"><option value="">None</option>${Array.from({ length: placeOf(s.location).slots }, (_, i) => i + 1).map((n) => { const taken = spools.find((x) => x.id !== s.id && !x.archived && x.location === s.location && slotOf(x) === n); return `<option value="${n}"${slotOf(s) === n ? " selected" : ""}>${n}${taken ? ` (${code(taken.id)})` : ""}</option>`; }).join("")}</select>` : ""}
            ${fits.length ? `<label>Loaded in</label><select class="inline" name="machine" data-load><option value="">Not loaded</option>${fits.map((m) => `<option value="${esc(m.id)}"${m.name === loadedIn(s) ? " selected" : ""}>${esc(m.name)}</option>`).join("")}</select>` : ""}
            <label>Lot</label>${editable("lot_nr", s.lot_nr)}
            <label>Price</label>${editable("price", s.price, { type: "number", unit: currency })}
            <label>Came with</label>${editable("initial_weight", s.initial_weight, { type: "number", unit })}
            ${unit === "g" ? `<label>Empty spool</label>${editable("spool_weight", s.spool_weight, { type: "number", unit: "g", placeholder: String(f.spool_weight ?? f.vendor?.empty_spool_weight ?? "–") })}` : ""}
            <label>Registered</label><span class="value">${new Date(s.registered).toLocaleDateString()}</span>
            <label>First used</label><input class="inline" type="datetime-local" name="first_used" value="${localTime(s.first_used)}">
            <label>Last used</label><input class="inline" type="datetime-local" name="last_used" value="${localTime(s.last_used)}">
            <label>Comment</label>${editable("comment", s.comment, { type: "textarea" })}
          </div>
          <h4 class="h-row">Extra fields <button type="button" class="link" data-settings>Manage fields →</button></h4>
          <div class="fields">${fields.spool.map((fl) => `<label>${esc(fl.name)}</label><span>${extraInput(fl, s)}</span>`).join("") || '<span class="muted" style="grid-column:1/-1">None: add a drying date, a shop…</span>'}</div>
        </form>
        <div>
          <h4 class="h-row">Filament <button type="button" class="link" data-pick="filament:${f.id}">Open filament →</button></h4>
          <div class="fields">
            ${fact("Material", esc(f.material || ""))}
            ${fact("Colour", f.color_hex ? `<span class="dot-colour" style="background:#${esc(f.color_hex.slice(0, 6))}"></span> <code>#${esc(f.color_hex.toUpperCase())}</code>${colourName(f) ? ` ${esc(colourName(f))}` : ""}` : "")}
            ${kindOf(f) === "filament" ? fact("Diameter", `${esc(f.diameter)} mm`) + fact("Density", `${esc(f.density)} g/cm³`) : ""}
            ${fact("Nozzle", f.settings_extruder_temp != null ? `${f.settings_extruder_temp} °C` : "")}
            ${fact("Bed", f.settings_bed_temp != null ? `${f.settings_bed_temp} °C` : "")}
            ${fact("Holds", f.weight != null ? amount(f.weight, unit) : "")}
            ${fact("Empty spool", f.spool_weight != null ? `${Math.round(f.spool_weight)} g` : "")}
            ${fields.filament.map((fl) => fact(esc(fl.name), extra(f, fl.key) != null ? `${esc(extra(f, fl.key))}${fl.unit ? ` ${esc(fl.unit)}` : ""}` : "")).join("")}
          </div>
          <h4>Tags</h4>${tagsBlock("spool", s)}
          <h4>History</h4><ul class="history" data-history><li class="muted">…</li></ul>
          ${s.archived ? `<p><button type="button" class="link" data-delete="spool:${s.id}">Remove for good</button></p>` : ""}
        </div>
      </div>`;
  }

  // Adjust weight, as Spoolman has it: weigh it on its spool, take off what was used, or set
  // what is left.
  function adjustWeight(s) {
    const f = s.filament, unit = unitOf(f);
    const empty = s.spool_weight ?? f.spool_weight ?? f.vendor?.empty_spool_weight;
    show(`${head(`Adjust weight · ${code(s.id)}`)}<section>
      <nav class="views" role="tablist" style="margin-bottom:14px">${unit === "g" ? '<button type="button" role="tab" data-mode="measure" aria-selected="true">Weigh</button>' : ""}<button type="button" role="tab" data-mode="use" aria-selected="${unit !== "g"}">Used</button><button type="button" role="tab" data-mode="left" aria-selected="false">Left</button></nav>
      <form class="act" data-adjust-form><input name="value" type="number" step="any" min="0" required autofocus style="width:10em"><span class="muted" data-adjust-unit></span><button>Save</button></form>
      <p class="hint" data-adjust-hint></p>
    </section>`);
    let mode = unit === "g" ? "measure" : "use";
    const form = $("[data-adjust-form]", dialogBody);
    const hints = {
      measure: [`g on the scale`, `With its spool on the scale; the empty spool${empty != null ? ` (${Math.round(empty)} g)` : ""} is taken off. Now ${amount(leftOf(s), unit)} left${empty != null && leftOf(s) != null ? `, so about ${Math.round(leftOf(s) + empty)} g on the scale` : ""}.`],
      use: [unit, `Taken off what is left (${amount(leftOf(s), unit)}).`],
      left: [unit, `What is left, counted or weighed without its spool.`],
    };
    const setMode = (m) => {
      mode = m;
      for (const b of dialogBody.querySelectorAll("[data-mode]")) b.setAttribute("aria-selected", String(b.dataset.mode === m));
      $("[data-adjust-unit]", dialogBody).textContent = hints[m][0];
      $("[data-adjust-hint]", dialogBody).textContent = hints[m][1];
      form.value.value = m === "left" ? Math.round(leftOf(s) ?? 0) : "";
      form.value.focus();
    };
    dialogBody.querySelectorAll("[data-mode]").forEach((b) => b.addEventListener("click", () => setMode(b.dataset.mode)));
    setMode(mode);
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const value = Number(form.value.value);
      try {
        const r = mode === "measure" ? await api("PUT", `/spool/${s.id}/measure`, { weight: value })
          : mode === "use" ? await api("PUT", `/spool/${s.id}/use`, { use_weight: value })
          : await api("PATCH", `/spool/${s.id}`, { remaining_weight: value });
        dialog.close();
        toast(`${amount(r.remaining_weight, unit)} left`);
      } catch (error) { toast(error.message, "bad"); }
    });
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
    const crumbs = `<nav class="crumbs">${f.vendor ? `<button type="button" class="link" data-pick="vendor:${f.vendor.id}">${esc(f.vendor.name)}</button> › ` : ""}<span>${esc(f.name || f.material || "Filament")}</span></nav>`;
    return `${crumbs}
      <div class="i-top">
        <div class="i-head">${swatch(f, true)}<div><h3>${esc(nameOf(f))} ${db}</h3><small>${esc(facts(f))}</small></div></div>
        <div class="i-actions"><button type="button" data-add-of="${f.id}">Add spools of it</button><button type="button" class="ghost" data-clone-filament="${f.id}" title="A new filament with the same settings, to change the colour or name">Duplicate</button></div>
      </div>
      <div class="i-left"><b>${amount(live.reduce((t, s) => t + (leftOf(s) ?? 0), 0), unit)}</b><span>in ${live.length} ${live.length === 1 ? "spool" : "spools"}</span></div>
      <div class="i-cols">
        <form data-patch="filament:${f.id}">
          <h4>Filament</h4>
          <div class="fields">
            <label>Name</label>${editable("name", f.name)}
            <label>Vendor</label><select class="inline" name="vendor_id"><option value="">None</option>${vendors.map((v) => `<option value="${v.id}"${v.id === f.vendor?.id ? " selected" : ""}>${esc(v.name)}</option>`).join("")}</select>
            <label>Material</label><input class="inline" name="material" list="materials-list" value="${esc(f.material ?? "")}" placeholder="–">
            <label>Colour</label><span class="with-unit"><input type="color" name="color_hex" value="#${esc((f.color_hex || "999999").slice(0, 6))}"><input class="inline" name="extra:color_name" value="${esc(colourName(f) ?? "")}" placeholder="Colour name"></span>
            ${kindOf(f) === "filament" ? `<label>Diameter</label>${editable("diameter", f.diameter, { type: "number", unit: "mm" })}<label>Density</label>${editable("density", f.density, { type: "number", unit: "g/cm³" })}` : ""}
            <label>One holds</label>${editable("weight", f.weight, { type: "number", unit })}
            ${unit === "g" ? `<label>Empty spool</label>${editable("spool_weight", f.spool_weight, { type: "number", unit: "g" })}<label>Nozzle</label>${editable("settings_extruder_temp", f.settings_extruder_temp, { type: "number", unit: "°C" })}<label>Bed</label>${editable("settings_bed_temp", f.settings_bed_temp, { type: "number", unit: "°C" })}` : ""}
            <label>Keep at least</label>${editable("extra:minimum", extra(f, "minimum"), { type: "number", unit: "spools" })}
            <label>Article / EAN</label>${editable("article_number", f.article_number)}
            <label>Price</label>${editable("price", f.price, { type: "number", unit: currency })}
            <label>Comment</label>${editable("comment", f.comment, { type: "textarea" })}
          </div>
          <h4 class="h-row">Extra fields <button type="button" class="link" data-settings>Manage fields →</button></h4>
          <div class="fields">${fields.filament.map((fl) => `<label>${esc(fl.name)}</label><span>${extraInput(fl, f)}</span>`).join("") || '<span class="muted" style="grid-column:1/-1">None yet</span>'}</div>
        </form>
        <div>
          <h4>Spools</h4>
          <ul class="history">${own.map((s) => `<li><button type="button" class="link" data-pick="spool:${s.id}">${code(s.id)}</button> ${amount(leftOf(s), unit)} <span class="muted">${esc(s.archived ? "used up" : whereOf(s))}</span></li>`).join("") || '<li class="muted">None yet</li>'}</ul>
          <h4>Tags</h4>${tagsBlock("filament", f)}
          ${own.length ? "" : `<p><button type="button" class="link" data-delete="filament:${f.id}">Remove this filament</button></p>`}
        </div>
      </div>`;
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
    if (type === "spool" && name === "location") {
      if (el.value === "+") { newPlace((made) => place(Number(id), made).then(() => toast(`Moved to ${made}`))); return; }
      try { await place(Number(id), el.value); toast(el.value ? `Moved to ${el.value}` : "Moved out of its location"); } catch (error) { toast(error.message, "bad"); }
      return;
    }
    if (type === "spool" && name === "slot") {
      const s = spools.find((x) => x.id === Number(id));
      try { await place(Number(id), s.location, el.value || null); toast(el.value ? `Slot ${el.value}` : "Out of its slot"); } catch (error) { toast(error.message, "bad"); }
      return;
    }
    if (type === "spool" && name === "machine") {
      try {
        if (el.value) await api("POST", `api/stock/items/${code(Number(id))}/load`, { machine: el.value });
        else await api("POST", `api/stock/items/${code(Number(id))}/unload`);
        toast(el.value ? `Loaded into ${machines.find((m) => m.id === el.value)?.name}` : "Taken out");
      } catch (error) { toast(error.message, "bad"); }
      return;
    }
    if (name === "first_used" || name === "last_used") {
      try { await api("PATCH", `/${type}/${id}`, { [name]: el.value ? new Date(el.value).toISOString() : null }); toast("Saved"); } catch (error) { toast(error.message, "bad"); }
      return;
    }
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
        if (place === "+") { place = prompt("The new location: a shelf, a drawer, a dry box"); if (!place) return; }
        await api("PATCH", `/spool/${s.id}`, { location: place || null });
        toast(place ? `Moved to ${place}` : "Moved out of its location");
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
      if (ui.view === "locations") switchView("library");
      return choose(type, id);
    }
    const fold = t.closest("[data-fold]");
    if (fold) { const key = fold.dataset.fold; if (ui.collapsed.has(key)) ui.collapsed.delete(key); else ui.collapsed.add(key); remember(); return render(); }
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
      if (t.closest("[data-adjust]") && s) return adjustWeight(s);
      if (t.closest("[data-archive]") && s) { await api("PATCH", `/spool/${s.id}`, { archived: !s.archived }); toast(s.archived ? "Back in stock" : `${code(s.id)} used up`); }
      if (t.closest("[data-print-label]") && s) label(labelText(s), code(s.id));
      if (t.closest("[data-clone]") && s) return await cloneSpool(s);
      const cloneOf = t.closest("[data-clone-filament]");
      if (cloneOf) return await cloneFilament(filament(Number(cloneOf.dataset.cloneFilament)));
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
      if (t.closest("[data-new-place]")) newPlace();
      const editPlace = t.closest("[data-edit-place]");
      if (editPlace) newPlace(null, places.find((x) => x.id === editPlace.dataset.editPlace));
    } catch (error) { toast(error.message, "bad"); }
  });

  // Duplicate, as in Spoolman: a new spool like this one, full and unused. Not on a machine and
  // not in its slot: those belong to the original.
  // The order threshold stays with the original filament too.
  const copyExtra = (extra) => Object.fromEntries(Object.entries(extra || {}).filter(([k]) => !["loaded_in", "slot", "minimum"].includes(k)));
  async function cloneSpool(s) {
    const made = await api("POST", "/spool", {
      filament_id: s.filament.id, location: s.location || undefined,
      initial_weight: s.initial_weight ?? undefined, spool_weight: s.spool_weight ?? undefined,
      price: s.price ?? undefined, lot_nr: s.lot_nr || undefined, comment: s.comment || undefined,
      extra: copyExtra(s.extra),
    });
    toast(`${code(made.id)} added, like ${code(s.id)}`);
    await load();
    choose("spool", made.id, true);
  }
  // A new filament with the same settings, its own from here on (not tied to SpoolmanDB).
  async function cloneFilament(f) {
    if (!f) return;
    const keep = ["name", "material", "price", "density", "diameter", "weight", "spool_weight", "article_number", "comment",
      "settings_extruder_temp", "settings_bed_temp", "color_hex", "multi_color_hexes", "multi_color_direction"];
    const body = Object.fromEntries(keep.filter((k) => f[k] != null && f[k] !== "").map((k) => [k, f[k]]));
    const made = await api("POST", "/filament", { ...body, vendor_id: f.vendor?.id, extra: copyExtra(f.extra) });
    toast("Filament duplicated: change its name or colour");
    await load();
    choose("filament", made.id, true);
  }

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

  // ---- filters ---------------------------------------------------------------------------
  function renderFilters() {
    const names = { material: "Material", vendor: "Vendor", location: "Location" };
    const label = (k, v) => k === "location" && v === "-" ? "no location" : k === "location" && v === "+" ? "not loaded" : v;
    $("[data-filters]").innerHTML = Object.entries(ui.filters).filter(([, v]) => v).map(([k, v]) => `<span class="chip">${names[k]}: ${esc(label(k, v))}<button type="button" data-unfilter="${k}" aria-label="Remove filter">✕</button></span>`).join("");
  }
  function filterDialog() {
    const options = (values, chosenValue) => `<option value="">Any</option>` + [...new Set(values)].filter(Boolean).sort().map((v) => `<option${v === chosenValue ? " selected" : ""}>${esc(v)}</option>`).join("");
    show(`${head("Filter")}<section><form class="grid-form" data-filter-form>
      <label>Material <select name="material">${options(filaments.map((f) => f.material), ui.filters.material)}</select></label>
      <label>Vendor <select name="vendor">${options(vendors.map((v) => v.name), ui.filters.vendor)}</select></label>
      <label class="wide">Location <select name="location">${options(places.map((p) => p.name), ui.filters.location)}<option value="-"${ui.filters.location === "-" ? " selected" : ""}>No location</option><option value="+"${ui.filters.location === "+" ? " selected" : ""}>Not loaded</option></select></label>
      <div class="wide act"><button>Show</button><button type="button" class="ghost" data-clear-filters>Clear</button></div>
    </form></section>`);
    const form = $("[data-filter-form]", dialogBody);
    form.addEventListener("submit", (e) => { e.preventDefault(); ui.filters = Object.fromEntries(new FormData(form)); remember(); renderFilters(); dialog.close(); render(); });
    $("[data-clear-filters]", dialogBody).addEventListener("click", () => { ui.filters = { material: "", vendor: "", location: "" }; remember(); renderFilters(); dialog.close(); render(); });
  }
  document.addEventListener("click", (e) => {
    if (e.target.closest("[data-filter-open]")) filterDialog();
    const un = e.target.closest("[data-unfilter]");
    if (un) { ui.filters[un.dataset.unfilter] = ""; remember(); renderFilters(); render(); }
  });
  ui.filters = { material: "", vendor: "", location: "", ...(ui.filters || {}) };
  renderFilters();

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
      if (!place) { toast(`No location ${c}`, "bad"); return true; }
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
  // Where a spool may go: a label tape into a label printer, filament into a 3D printer; a
  // dry box (a place with slots) holds filament only. Why not, or "" when it may.
  function refusal(id, el) {
    const s = spools.find((x) => x.id === Number(id));
    if (!s || !el) return "";
    const kind = kindOf(s.filament);
    if (el.dataset.machine) {
      const m = machines.find((x) => x.id === el.dataset.machine);
      if (kind === "tape" && m?.kind !== "label") return "A label tape goes into a label printer";
      if (kind === "filament" && m?.kind !== "fdm") return "Filament goes into a 3D printer";
      return "";
    }
    if (kind === "tape" && el.dataset.drop && placeOf(el.dataset.drop)?.slots) return "A dry box holds filament, not a label tape";
    return "";
  }
  // While a spool is picked or dragged: where it fits lights up, the rest is greyed.
  function markTargets(id) {
    for (const el of document.querySelectorAll("[data-board] [data-machine], [data-board] [data-drop]")) {
      el.classList.toggle("no", id != null && !!refusal(id, el));
    }
  }
  function renderBoard() {
    const live = spools.filter((s) => !s.archived);
    const card = (s, inMachine = false) => `<div class="card-spool${picked === s.id ? " picked" : ""}" draggable="true" data-card="${s.id}">${swatch(s.filament)}<span><b>${code(s.id)}</b> ${esc(nameOf(s.filament))}<br><small class="muted">${amount(leftOf(s), unitOf(s.filament))}${!inMachine && loadedIn(s) ? ` · <span class="feeds">→ ${esc(loadedIn(s))}</span>` : inMachine && s.location ? ` · from ${esc(s.location)}${slotOf(s) ? ` ${slotOf(s)}` : ""}` : ""}</small></span>${bar(s)}</div>`;
    // By place, or by one of the spool's own fields with one short value (text or a choice).
    const field = fields.spool.find((f) => f.key === settings.board_field && !f.multi_choice && ["text", "choice"].includes(f.field_type));
    const valueOf = (s) => field ? extra(s, field.key) ?? "" : s.location ?? "";
    const keys = field
      ? [...new Set([...(field.choices || []), ...live.map(valueOf).filter(Boolean)])].sort()
      : places.slice().sort((a, b) => a.name.localeCompare(b.name)).map((p) => p.name);
    const cols = [
      ...keys.map((k) => ({ key: k, title: k, code: field ? null : placeCode(k), spools: live.filter((s) => valueOf(s) === k) })),
      // Without a place and on a printer, it is on the printer (shown there), not nowhere.
      { key: "", title: field ? `No ${field.name.toLowerCase()}` : "No location", spools: live.filter((s) => !valueOf(s) && (field || !loadedIn(s))) },
    ];
    // A column per printer and label printer: drop a spool on it to load it, where it is.
    const feeders = machines.filter((m) => ["fdm", "label"].includes(m.kind));
    const pickers = `<div class="board-top"><label class="pick">Board by <select data-board-field><option value="location">Location</option>${fields.spool.filter((f) => !f.multi_choice && ["text", "choice"].includes(f.field_type)).map((f) => `<option value="${esc(f.key)}"${f.key === settings.board_field ? " selected" : ""}>${esc(f.name)}</option>`).join("")}</select></label>${field ? "" : '<button type="button" class="ghost" data-new-place>New location</button>'}<span class="hint">Drag a spool to a location, a slot or a printer, or tap it, then where.</span></div>`;
    const body = (c) => {
      const p = !field && placeOf(c.key);
      if (!p?.slots) return c.spools.map((x) => card(x)).join("") || '<p class="muted" style="margin:6px 4px;font-size:13px">Empty</p>';
      // A dry box: its slots in order, each a spool or a place to drop one.
      const slots = Array.from({ length: p.slots }, (_, i) => i + 1).map((n) => {
        const in_ = c.spools.find((s) => slotOf(s) === n);
        return `<div class="slot${picked && !in_ ? " target" : ""}" data-drop="${esc(c.key)}" data-slot="${n}"><span class="slot-n">${n}</span>${in_ ? card(in_) : '<span class="muted">empty</span>'}</div>`;
      }).join("");
      const loose = c.spools.filter((s) => !slotOf(s) || slotOf(s) > p.slots);
      return slots + (loose.length ? `<p class="hint">Not in a slot</p>${loose.map((x) => card(x)).join("")}` : "");
    };
    const modelOf = (k) => { const p = !field && placeOf(k); const d = p?.model && dryboxes.find((x) => x.id === p.model); return d ? `${d.brand} ${d.model}` : p?.slots ? `${p.slots} slots` : ""; };
    // The printers in a strip that stays in view, each a place to drop a spool (it loads
    // there): no scrolling sideways to reach one.
    const drop = (m) => { const s = live.find((x) => loadedIn(x) === m.name); return `<div class="printer-drop${picked ? " target" : ""}" data-machine="${esc(m.id)}"><b>${esc(m.name)}</b>${s ? `<span class="loaded-line">${swatch(s.filament)} <span><b>${code(s.id)}</b> ${esc(nameOf(s.filament))}<br><small class="muted">${amount(leftOf(s), unitOf(s.filament))}${s.location ? ` · from ${esc(s.location)}${slotOf(s) ? ` ${slotOf(s)}` : ""}` : ""}</small></span></span>` : '<small class="muted">nothing loaded</small>'}</div>`; };
    // By kind: the 3D printers, then the label printers, each under its name.
    const KINDS_OF = [["fdm", "3D printers"], ["label", "Label printers"]];
    const strip = feeders.length ? `<div class="board-printers">${KINDS_OF.map(([k, title]) => {
      const of = feeders.filter((m) => m.kind === k).sort((a, b) => a.name.localeCompare(b.name));
      return of.length ? `<div class="printer-group"><h4>${title} <span class="muted">${of.length}</span></h4><div class="printer-row">${of.map(drop).join("")}</div></div>` : "";
    }).join("")}</div>` : "";
    $("[data-board]").innerHTML = pickers + strip + `<div class="board-grid">` + cols.map((c) => `<div class="col${c.key !== null && picked ? " target" : ""}"${c.key !== null ? ` data-drop="${esc(c.key)}"` : ""}>
        <div class="col-head"><b>${esc(c.title)}</b><small>${c.code ? `<code>${esc(c.code)}</code> · ` : ""}${c.spools.length}</small>${c.code ? `<span class="col-tools"><button type="button" class="icon-button" data-label-place="${esc(c.code)}" title="Print its label" aria-label="Print the label of ${esc(c.title)}"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3 6.5 9.5 3H17v7.5L10.5 17 3 10.5z"/><circle cx="13.5" cy="6.5" r="1.3"/></svg></button><button type="button" class="icon-button" data-edit-place="${esc(c.code)}" title="Edit: name, description, slots, remove" aria-label="Edit ${esc(c.title)}"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M4 16l1-4 8.5-8.5 3 3L8 15z"/><path d="m11.5 5.5 3 3"/></svg></button></span>` : ""}</div>
        ${modelOf(c.key) ? `<small class="muted" style="padding:0 4px">${esc(modelOf(c.key))}</small>` : ""}
        ${!field && placeOf(c.key)?.description ? `<small class="muted" style="padding:0 4px">${esc(placeOf(c.key).description)}</small>` : ""}
        ${body(c)}</div>`).join("") + `</div>`;
  }
  const moveTo = async (id, placeName, slot) => {
    try {
      if (slot) { await place(id, placeName, slot); toast(`${code(id)} to ${placeName}, slot ${slot}`); return; }
      const place_ = placeName;
      const field = settings.board_field !== "location" && fields.spool.find((f) => f.key === settings.board_field);
      if (field) {
        const s = spools.find((x) => x.id === id);
        const all = { ...(s.extra || {}) };
        delete all.loaded_in;
        if (place_) all[field.key] = JSON.stringify(place_); else delete all[field.key];
        await api("PATCH", `/spool/${id}`, { extra: all });
      } else await api("PATCH", `/spool/${id}`, { location: place_ || null });
      toast(place_ ? `${code(id)} to ${place_}` : `${code(id)}: none`);
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
  board.addEventListener("dragstart", (e) => { const c = e.target.closest("[data-card]"); if (c) { e.dataTransfer.setData("text/plain", c.dataset.card); markTargets(c.dataset.card); } });
  board.addEventListener("dragend", () => markTargets(null));
  const loadInto = async (id, machine) => {
    try {
      await api("POST", `api/stock/items/${code(id)}/load`, { machine });
      toast(`${code(id)} loaded into ${machines.find((m) => m.id === machine)?.name}, where it is`);
    } catch (e) { toast(e.message, "bad"); }
  };
  board.addEventListener("dragover", (e) => { const col = e.target.closest("[data-drop], [data-machine]"); if (col) { e.preventDefault(); col.classList.add("over"); } });
  board.addEventListener("dragleave", (e) => e.target.closest("[data-drop], [data-machine]")?.classList.remove("over"));
  board.addEventListener("drop", (e) => {
    const id = e.dataTransfer.getData("text/plain");
    markTargets(null);
    const target = e.target.closest("[data-machine]") || e.target.closest("[data-slot]") || e.target.closest("[data-drop]");
    const why = refusal(id, target);
    if (why) { e.preventDefault(); target.classList.remove("over"); toast(why, "bad"); return; }
    if (target?.dataset.machine) { e.preventDefault(); target.classList.remove("over"); loadInto(Number(id), target.dataset.machine); return; }
    const col = e.target.closest("[data-drop]");
    if (!col) return;
    e.preventDefault();
    col.classList.remove("over");
    moveTo(Number(e.dataTransfer.getData("text/plain")), col.dataset.drop, col.dataset.slot);
  });
  board.addEventListener("click", (e) => {
    const c = e.target.closest("[data-card]");
    if (c) { const id = Number(c.dataset.card); picked = picked === id ? null : id; renderBoard(); markTargets(picked); return; }
    const target = e.target.closest("[data-machine]") || e.target.closest("[data-slot]") || e.target.closest("[data-drop]");
    if (target && picked && !e.target.closest("button")) {
      const why = refusal(picked, target);
      if (why) { toast(why, "bad"); return; }
      const id = picked; picked = null;
      if (target.dataset.machine) loadInto(id, target.dataset.machine);
      else moveTo(id, target.dataset.drop, target.dataset.slot);
    }
  });

  // ---- a new place: a shelf, or a dry box with its slots -----------------------------------
  function newPlace(then, was) {
    show(`${head(was ? `Location <code>${esc(was.id)}</code>` : "New location")}<section><form class="grid-form" data-place-form>
      <label class="wide">What <select name="kind"><option value="shelf">A shelf, a drawer, a crate</option><optgroup label="A dry box">${dryboxes.map((d) => `<option value="${esc(d.id)}">${esc(d.brand)} ${esc(d.model)} · ${d.slots} ${d.slots === 1 ? "spool" : "spools"}${d.heats ? ", heats" : ""}${d.check ? " (check)" : ""}</option>`).join("")}<option value="own">Another dry box…</option></optgroup></select></label>
      <label>Name <input name="name" required placeholder="Shelf B2"></label>
      <label>Slots <input name="slots" type="number" min="0" max="64" placeholder="none"></label>
      <label class="wide">Description <input name="description" placeholder="Under the bench, left; the box with the blue lid"></label>
      <p class="hint wide" data-place-hint>A location with slots is drawn with them on the board; a spool goes in a slot.</p>
      <div class="wide act">${was ? `<button>Save</button><button type="button" class="danger" data-remove-place>Remove</button>` : "<button>Add location</button>"}</div>
    </form></section>`);
    const form = $("[data-place-form]", dialogBody);
    let named = !!was;
    if (was) {
      form.name.value = was.name;
      form.slots.value = was.slots ?? "";
      form.description.value = was.description ?? "";
      form.kind.value = was.model && dryboxes.some((d) => d.id === was.model) ? was.model : was.slots ? "own" : "shelf";
      $("[data-remove-place]", dialogBody).addEventListener("click", async () => {
        const inIt = spools.filter((x) => x.location === was.name && !x.archived).length;
        if (!confirm(inIt ? `Remove ${was.name}? Its ${inIt} ${inIt === 1 ? "spool goes" : "spools go"} to No location.` : `Remove ${was.name}?`)) return;
        try { await api("DELETE", `api/stock/locations/${encodeURIComponent(was.id)}`); dialog.close(); toast(`${was.name} removed`); await load(); } catch (error) { toast(error.message, "bad"); }
      });
    }
    form.name.addEventListener("input", () => { named = true; });
    form.kind.addEventListener("change", () => {
      const d = dryboxes.find((x) => x.id === form.kind.value);
      if (d) {
        form.slots.value = d.slots;
        const same = places.filter((p) => p.model === d.id).length;
        if (!named) form.name.value = `${d.model}${same ? ` ${same + 1}` : ""}`;
        $("[data-place-hint]", dialogBody).textContent = `${d.brand} ${d.model}: ${d.slots} slots for 1 kg spools${d.heats ? ", with a heater" : ", dry with desiccant"}${d.check ? ". The number of slots is not confirmed: set it as yours has." : "."}`;
      } else if (form.kind.value === "own") { form.slots.value = form.slots.value || 4; if (!named) form.name.value = "Dry box"; }
      else form.slots.value = "";
    });
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const d = dryboxes.find((x) => x.id === form.kind.value);
      const name = form.name.value.trim();
      const body = { name, slots: form.slots.value ? Number(form.slots.value) : null, model: d?.id || null, description: form.description.value.trim() || null };
      try {
        if (was) await api("POST", `api/stock/locations/${encodeURIComponent(was.id)}`, body);
        else await api("POST", "api/stock/locations", body);
        dialog.close();
        toast(was ? `${name} saved` : `${name} added`);
        await load();
        if (then) then(name);
      } catch (error) { toast(error.message, "bad"); }
    });
  }

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
        <label class="search" style="margin-bottom:4px"><svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="8.5" cy="8.5" r="5.5"/><path d="m13 13 4 4"/></svg><input data-find placeholder="Which filament or label tape? prusament petg, polymaker pla black, dymo 12 mm, 45013…" autocomplete="off" aria-label="Find a filament or a label tape"></label>
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
      // The tapes of the label printers you have (all of them when you have none yet),
      // found by their maker, name, number, width ("12 mm", "12mm") and "label tape cassette".
      const printers = new Set(machines.filter((m) => m.kind === "label").map((m) => m.profile).filter(Boolean));
      const ofYours = (m) => !printers.size || printers.has((m.media || "").split(":")[0]);
      const tapeText = (m) => `${m.brand} ${m.name} ${m.width_mm ?? ""} mm ${m.width_mm ?? ""}mm label tape cassette ${(m.media || "").split(":")[1] || ""}`;
      const tapeHits = tapes.filter((m) => !ownMedia.has(m.media) && (words.length ? hit(tapeText(m)) : ofYours(m))).slice(0, words.length ? 12 : 20).map((m) => ({ filament: fromTape(m), tape: m, source: "tape" }));
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
    const next = all.map((m) => ({ id: m.id, name: m.name, kind: m.kind, profile: m.profile }));
    if (JSON.stringify(next) !== JSON.stringify(machines)) { machines = next; if (ui.view === "locations") renderBoard(); else if (chosen) renderInspector(); }
  };

  document.body.insertAdjacentHTML("beforeend", '<datalist id="materials-list"></datalist>');
  api("GET", "/external/material").then((m) => { materialsDb = m; $("#materials-list").innerHTML = m.map((x) => `<option value="${esc(x.material)}">`).join(""); }).catch(() => {});
  switchView(ui.view);
  load().then(() => {
    // Opened from a label's QR code: what it names.
    const c = decodeURIComponent(location.hash.slice(1));
    if (c) { history.replaceState(null, "", location.pathname); scan(c); }
  }).catch((e) => { $("[data-list]").innerHTML = `<p class="empty">Not loaded: ${esc(e.message)}</p>`; });
