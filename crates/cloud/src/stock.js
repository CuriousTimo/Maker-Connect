  // The stock page: totals, materials with their items, locations; a scan box that a USB
  // barcode scanner types into (it types the code and Enter); dialogs to receive, move,
  // load, weigh and label.
  const $ = (s) => document.querySelector(s);
  const esc = (t) => String(t ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const dialog = $("[data-dialog]"), body = $("[data-dialog-body]");
  let data = { materials: [], items: [], locations: [], catalogue: [] };
  let machines = [];
  let openItem = null;

  const amount = (value, unit) => unit === "g"
    ? (value >= 1000 ? `${(value / 1000).toFixed(2)} kg` : `${Math.round(value)} g`)
    : unit === "m" ? `${value.toFixed(1)} m` : `${Math.round(value)} ${unit}`;
  const KINDS = { filament: "Filament", resin: "Resin", tape: "Label tape", sheet: "Sheet", other: "Other" };
  const material = (id) => data.materials.find((m) => m.id === id);
  const placeName = (item) => item.machine
    ? `in ${machines.find((m) => m.id === item.machine)?.name || "a machine"}`
    : item.location ? (data.locations.find((l) => l.id === item.location)?.name || item.location) : "no place";
  // A material as it looks: a tape as a piece of tape with its ink, a filament as a spool.
  const swatch = (m) => m.kind === "tape"
    ? `<span class="swatch-tape${m.color === "clear" ? " clear" : ""}" style="--tape:${m.color === "clear" ? "transparent" : esc(m.color || "#fff")};--ink:${esc(m.ink || "#000")}">Abc</span>`
    : `<span class="swatch-spool" style="--c:${esc(m.color || "#999")}"></span>`;

  // A filament's facts from its slicer profile: 1.27 g/cm³ · spool 278 g · 250 °C, bed 90 °C.
  const facts = (m) => [
    m.density_g_cm3 && `${m.density_g_cm3} g/cm³`,
    m.spool_g && `empty spool ${Math.round(m.spool_g)} g`,
    m.nozzle_c && `${Math.round(m.nozzle_c)} °C${m.bed_c ? `, bed ${Math.round(m.bed_c)} °C` : ""}`,
  ].filter(Boolean).join(" · ");
  // Its brand and name, the brand once (Devil Design's names start with it), and its colour's
  // name: Prusa Polymers Prusament PETG 1.75 mm · Galaxy Black.
  const fullName = (m) => (m.brand && !m.name.startsWith(m.brand) ? `${m.brand} ${m.name}` : m.name)
    + (m.color_name ? ` · ${m.color_name}` : "");

  // Fuzzy search: every word typed must be found in the text, as a part of it (best at the
  // start of a word), as its letters in order inside one word with a gap or two ("prsa",
  // "devl"), or as a word with one letter wrong ("petq"; two in long words). Better matches
  // score higher.
  const words = (t) => String(t).toLowerCase().normalize("NFD").replace(/[\u0300-\u036f]/g, "").split(/[^a-z0-9.%+]+/).filter(Boolean);
  function close(a, b) {
    if (Math.abs(a.length - b.length) > 2) return 99;
    let row = Array.from({ length: b.length + 1 }, (_, i) => i);
    for (let i = 1; i <= a.length; i++) {
      const next = [i];
      for (let j = 1; j <= b.length; j++) next[j] = Math.min(row[j] + 1, next[j - 1] + 1, row[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
      row = next;
    }
    return row[b.length];
  }
  function fuzzy(query, hay, hayWords) {
    let score = 0;
    for (const q of words(query)) {
      const at = hay.indexOf(q);
      if (at >= 0) { score += at === 0 || !/[a-z0-9]/.test(hay[at - 1]) ? 4 : 3; continue; }
      // Its letters in order inside one word, from the word's start: "prsa" in "prusa".
      const within = (w) => {
        if (w[0] !== q[0]) return false;
        let last = 0, gaps = 0;
        for (const ch of q.slice(1)) {
          const n = w.indexOf(ch, last + 1);
          if (n < 0) return false;
          if (n > last + 1) gaps++;
          last = n;
        }
        return gaps <= 2;
      };
      if (q.length > 2 && hayWords.some(within)) { score += 2; continue; }
      if (q.length > 3 && hayWords.some((w) => close(q, w) <= (q.length > 6 ? 2 : 1))) { score += 1.5; continue; }
      return 0;
    }
    return score;
  }

  async function load() {
    data = await (await fetch("api/stock")).json();
    render();
  }

  function render() {
    const live = data.items.filter((i) => i.state !== "used-up");
    const filament = live.filter((i) => material(i.material)?.unit === "g");
    const tapes = live.filter((i) => material(i.material)?.kind === "tape");
    $("[data-totals]").innerHTML = [
      ["Filament", amount(filament.reduce((s, i) => s + i.amount, 0), "g"), `${filament.length} spools`],
      ["Label tape", `${tapes.length}`, "cassettes"],
      ["Loaded", `${live.filter((i) => i.state === "loaded").length}`, "in machines"],
      ["Locations", `${data.locations.length}`, "shelves, boxes"],
    ].map(([l, v, s]) => `<div class="tile"><div class="label">${l}</div><div class="value">${esc(v)}</div><div class="sub">${esc(s)}</div></div>`).join("");

    $("[data-materials]").innerHTML = data.materials.length ? data.materials.map((m) => {
      const items = live.filter((i) => i.material === m.id);
      const low = m.minimum != null && items.length < m.minimum;
      return `<details class="material"${items.length ? " open" : ""}>
        <summary>${swatch(m)}<span class="m-name"><b>${esc(fullName(m))}</b><small>${KINDS[m.kind] || esc(m.kind)}${facts(m) ? ` · ${esc(facts(m))}` : ""}${m.ean ? ` · EAN ${esc(m.ean)}` : ""}</small></span>
          <span class="m-count${low ? " low" : ""}">${items.length} in stock${low ? " · low" : ""}</span>
          <span class="m-amount">${amount(items.reduce((s, i) => s + i.amount, 0), m.unit)}</span>
          <span class="m-actions"><button type="button" class="ghost small" data-edit-material="${esc(m.id)}">Edit</button><button type="button" class="ghost small" data-receive-material="${esc(m.id)}">Receive</button></span></summary>
        ${items.length ? `<ul class="items">${items.map((i) => `<li data-item="${esc(i.id)}"><code>${esc(i.id)}</code><span>${amount(i.amount, m.unit)}</span><span class="${i.state === "loaded" ? "loaded" : "muted"}">${esc(placeName(i))}</span></li>`).join("")}</ul>` : ""}
      </details>`;
    }).join("") : '<p class="empty">No materials yet. <b>New material</b> adds one, from the tapes your label printers take or common filaments; <b>Receive</b> then adds what came in.</p>';

    $("[data-locations]").innerHTML = data.locations.length ? `<ul class="locations">${data.locations.map((l) => {
      const n = live.filter((i) => i.location === l.id).length;
      return `<li data-location="${esc(l.id)}"><code>${esc(l.id)}</code><b>${esc(l.name)}</b><span class="muted">${n} items</span><button type="button" class="ghost small" data-label-location="${esc(l.id)}">Label</button></li>`;
    }).join("")}</ul>` : '<p class="empty">No locations yet: a shelf, a drawer, a dry box. Each gets a code and a label to scan.</p>';
  }

  // A label for an item or a location, made on the label printer's page: its text and a QR
  // code linking here, filled in to print.
  function label(text, code) {
    const printer = machines.find((m) => m.kind === "label");
    if (!printer) { toast("No label printer yet: add one to print labels", "bad"); return; }
    const link = `${location.origin}${location.pathname.replace(/stock$/, "")}stock#${code}`;
    location.href = `machines/${encodeURIComponent(printer.id)}?label=${encodeURIComponent(text)}&qr=${encodeURIComponent(link)}#label`;
  }

  async function post(url, json) {
    const r = await fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(json) });
    if (!r.ok) throw new Error(await r.text());
    return r.status === 204 ? null : r.json();
  }

  function show(html) {
    body.innerHTML = html;
    if (!dialog.open) dialog.showModal();
  }
  dialog.addEventListener("click", (e) => { if (e.target === dialog || e.target.closest("[data-close]")) dialog.close(); });
  dialog.addEventListener("close", () => { openItem = null; });
  const head = (title) => `<div class="dialog-head"><h2>${title}</h2><button type="button" class="close" data-close aria-label="Close">✕</button></div>`;
  const locationOptions = (chosen) => `<option value="">No place</option>` + data.locations.map((l) => `<option value="${esc(l.id)}"${l.id === chosen ? " selected" : ""}>${esc(l.id)} · ${esc(l.name)}</option>`).join("");

  // An item: what it is, where, its history; move, load, weigh, used up, label.
  async function showItem(id) {
    const item = data.items.find((i) => i.id === id);
    if (!item) { toast(`No item ${id}`, "bad"); return; }
    openItem = id;
    const m = material(item.material) || { name: "?", unit: "pcs" };
    const history = await (await fetch(`api/stock/items/${encodeURIComponent(id)}/movements`)).json();
    const fits = machines.filter((x) => (m.kind === "tape" ? x.kind === "label" : m.kind === "filament" ? x.kind === "fdm" : true));
    show(`${head(`${swatch(m)} <code>${esc(id)}</code> ${esc(fullName(m))}`)}
      <section class="item-body">
        <dl><dt>Left</dt><dd>${amount(item.amount, m.unit)}</dd><dt>Where</dt><dd>${esc(placeName(item))}</dd><dt>State</dt><dd>${esc(item.state)}</dd>${facts(m) ? `<dt>Prints</dt><dd>${esc(facts(m))}</dd>` : ""}</dl>
        <form data-act="move"><label>Move to <select name="location">${locationOptions(item.location)}</select></label><button>Move</button></form>
        ${fits.length ? `<form data-act="load"><label>Load into <select name="machine">${fits.map((x) => `<option value="${esc(x.id)}"${x.id === item.machine ? " selected" : ""}>${esc(x.name)}</option>`).join("")}</select></label><button>Load</button></form>` : ""}
        ${m.spool_g ? `<form data-act="weigh"><label>On the scale (g) <input name="gross" type="number" step="any" min="0" placeholder="${Math.round(item.amount + m.spool_g)}" required></label><button>Weigh</button><span class="hint">its empty spool, ${Math.round(m.spool_g)} g, is taken off</span></form>` : ""}
        <form data-act="amount"><label>Left (${esc(m.unit)}) <input name="amount" type="number" step="any" min="0" value="${item.amount}"></label><select name="why"><option value="weighed">weighed</option><option value="adjusted">counted</option></select><button>Set</button></form>
        <div class="item-actions"><button type="button" class="ghost" data-act-label>Print its label</button><button type="button" class="danger" data-act-used>Used up</button></div>
        <h4>History</h4>
        <ul class="history">${history.map((h) => `<li><b>${esc(h.kind)}</b>${h.amount != null && h.kind !== "received" ? ` ${h.amount > 0 ? "+" : ""}${amount(h.amount, m.unit)}` : ""}${h.place ? ` · ${esc(data.locations.find((l) => l.id === h.place)?.name || machines.find((x) => x.id === h.place)?.name || h.place)}` : ""} <span class="muted">${new Date(h.at * 1000).toLocaleString()}</span></li>`).join("")}</ul>
        <p class="hint">Scan a location's label now to move it there.</p>
      </section>`);
    body.querySelectorAll("form[data-act]").forEach((f) => f.addEventListener("submit", async (e) => {
      e.preventDefault();
      const v = Object.fromEntries(new FormData(f));
      try {
        if (f.dataset.act === "move") await post(`api/stock/items/${id}/move`, { location: v.location });
        if (f.dataset.act === "load") await post(`api/stock/items/${id}/load`, { machine: v.machine });
        if (f.dataset.act === "amount") await post(`api/stock/items/${id}/amount`, { amount: Number(v.amount), why: v.why });
        if (f.dataset.act === "weigh") await post(`api/stock/items/${id}/amount`, { amount: Math.max(0, Number(v.gross) - m.spool_g), why: "weighed" });
        toast({ move: "Moved", load: "Loaded", amount: "Amount set", weigh: `${amount(Math.max(0, Number(v.gross) - (m.spool_g || 0)), "g")} left` }[f.dataset.act]);
        await load();
        showItem(id);
      } catch (error) { toast(error.message, "bad"); }
    }));
    body.querySelector("[data-act-label]").addEventListener("click", () => label(`${id}\n${fullName(m).slice(0, 28)}`, id));
    body.querySelector("[data-act-used]").addEventListener("click", async () => {
      if (!confirm(`${id} used up?`)) return;
      await post(`api/stock/items/${id}/used-up`, {});
      toast(`${id} used up`);
      dialog.close();
      load();
    });
  }

  // New material (or one changed): from the catalogue (a printer's tapes; the filaments of
  // PrusaSlicer's profiles, hundreds: found by typing, fuzzily) or by hand.
  function newMaterial(editing) {
    const c = data.catalogue.map((m) => {
      const text = `${fullName(m)} ${KINDS[m.kind] || m.kind} ${m.media || ""}`.toLowerCase();
      return { m, text, words: words(text) };
    });
    const was = editing ? material(editing) : null;
    show(`${head(was ? `Change ${esc(fullName(was))}` : "New material")}<section>
      <form data-new-material-form class="grid-form" autocomplete="off">
        <div class="wide catalogue-pick"><label>Start from <input name="from" type="search" placeholder="Type: prusament petg, devil pla, 45013… or leave empty" role="combobox" aria-expanded="false" aria-controls="stock-catalogue"></label>
          <ul class="picker-list" id="stock-catalogue" role="listbox" hidden></ul>
          <small class="hint" data-facts>${c.length} in the catalogue: tapes of your label printers, filaments from PrusaSlicer's profiles</small></div>
        <label>Kind <select name="kind">${Object.entries(KINDS).map(([k, t]) => `<option value="${k}">${t}</option>`).join("")}</select></label>
        <label>Brand <input name="brand"></label>
        <label class="wide">Name <input name="name" required></label>
        <label>Colour <input name="color" type="color" value="#cccccc"></label>
        <label>Colour name <input name="color_name" placeholder="Galaxy Black, Jet Black, Black on Yellow"></label>
        <label>Unit <select name="unit"><option value="g">grams</option><option value="m">metres</option><option value="pcs">pieces</option></select></label>
        <label>One comes with <input name="package" type="number" step="any" min="0" value="1000"></label>
        <label>Keep at least <input name="minimum" type="number" min="0" value="1"></label>
        <label>Empty spool (g) <input name="spool_g" type="number" step="any" min="0" placeholder="weighed, to take off"></label>
        <label class="wide">EAN (its barcode, to scan it) <input name="ean" inputmode="numeric"></label>
        <div class="wide"><button>${was ? "Save" : "Add material"}</button></div>
      </form></section>`);
    const f = body.querySelector("form");
    const factsLine = body.querySelector("[data-facts]");
    const list = body.querySelector("#stock-catalogue");
    let base = was ? { ...was } : {};
    let shown = [], at = 0;
    const fill = (m) => {
      base = { ...m, id: undefined };
      for (const k of ["kind", "brand", "name", "unit", "package", "minimum"]) if (m[k] != null) f[k].value = m[k];
      f.spool_g.value = m.spool_g ?? "";
      if (m.color && m.color.startsWith("#")) f.color.value = m.color.length === 4 ? "#" + [...m.color.slice(1)].map((x) => x + x).join("") : m.color;
      factsLine.textContent = m.kind === "tape" ? "A label tape: loading it tells the printer which tape it has" : [facts(m), m.source && `from ${m.source.replace(/^prusaslicer-settings-/, "PrusaSlicer's ").replace("@", " profiles, ")}`].filter(Boolean).join(" · ");
    };
    if (was) {
      fill(was);
      base = { ...was };
      f.color_name.value = was.color_name || "";
      f.ean.value = was.ean || "";
    }
    const draw = () => {
      list.innerHTML = shown.length
        ? shown.map(({ m }, n) => `<li role="option" data-n="${n}" aria-selected="${n === at}">${swatch(m)}<span class="picker-text"><b>${esc(fullName(m))}</b><small>${esc(m.kind === "tape" ? "Label tape" : facts(m) || KINDS[m.kind])}</small></span></li>`).join("")
        : '<li class="picker-none">Nothing matches: fill it in by hand below</li>';
      list.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
    };
    const open = (yes) => { list.hidden = !yes; f.from.setAttribute("aria-expanded", String(yes)); };
    const search = () => {
      const q = f.from.value.trim();
      shown = q
        ? c.map((e) => ({ ...e, score: fuzzy(q, e.text, e.words) })).filter((e) => e.score > 0)
          .sort((a, b) => b.score - a.score || a.text.length - b.text.length).slice(0, 40)
        : c.slice(0, 40);
      at = 0;
      draw();
      open(true);
    };
    const choose = (n) => {
      const e = shown[n];
      if (!e) return;
      fill(e.m);
      f.from.value = fullName(e.m);
      open(false);
      f.color_name.focus();
    };
    f.from.addEventListener("input", search);
    f.from.addEventListener("focus", search);
    f.from.addEventListener("keydown", (e) => {
      if (list.hidden) return;
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        at = Math.max(0, Math.min(shown.length - 1, at + (e.key === "ArrowDown" ? 1 : -1)));
        draw();
      } else if (e.key === "Enter") { e.preventDefault(); choose(at); }
      else if (e.key === "Escape") { e.preventDefault(); open(false); }
    });
    // Chosen with the mouse before the field loses focus.
    list.addEventListener("mousedown", (e) => { e.preventDefault(); const li = e.target.closest("[data-n]"); if (li) choose(Number(li.dataset.n)); });
    f.from.addEventListener("blur", () => open(false));
    f.addEventListener("submit", async (e) => {
      e.preventDefault();
      const v = Object.fromEntries(new FormData(f));
      const sent = { ...base, kind: v.kind, brand: v.brand, name: v.name, unit: v.unit, color: base.color === "clear" ? "clear" : v.color, color_name: v.color_name.trim() || null, package: Number(v.package) || null, minimum: v.minimum === "" ? null : Number(v.minimum), ean: v.ean || null, spool_g: v.spool_g === "" ? null : Number(v.spool_g) };
      delete sent.id;
      try {
        await post(was ? `api/stock/materials/${encodeURIComponent(was.id)}` : "api/stock/materials", sent);
        toast(was ? "Material saved" : "Material added");
        dialog.close();
        load();
      } catch (error) { toast(error.message, "bad"); }
    });
  }

  // Receive: how many of a material came in, and where they go; each gets a code.
  function receive(materialId) {
    if (!data.materials.length) { newMaterial(); return; }
    show(`${head("Receive")}<section><form class="grid-form">
      <label class="wide">Material <select name="material">${data.materials.map((m) => `<option value="${esc(m.id)}"${m.id === materialId ? " selected" : ""}>${esc(fullName(m))}</option>`).join("")}</select></label>
      <label>How many <input name="count" type="number" min="1" max="200" value="1"></label>
      <label>Where <select name="location">${locationOptions("")}</select></label>
      <div class="wide"><button>Receive</button></div></form></section>`);
    body.querySelector("form").addEventListener("submit", async (e) => {
      e.preventDefault();
      const v = Object.fromEntries(new FormData(e.target));
      try {
        const { items } = await post("api/stock/receive", { material: v.material, count: Number(v.count), location: v.location });
        toast(`${items.length} received: ${items[0]}${items.length > 1 ? ` … ${items[items.length - 1]}` : ""}`);
        dialog.close();
        await load();
        if (items.length === 1) showItem(items[0]);
      } catch (error) { toast(error.message, "bad"); }
    });
  }

  function newLocation() {
    show(`${head("New location")}<section><form class="grid-form">
      <label class="wide">Name <input name="name" required placeholder="Shelf B2, dry box 1, drawer D12"></label>
      <label class="wide">Inside <select name="parent">${locationOptions("")}</select></label>
      <div class="wide"><button>Add location</button></div></form></section>`);
    body.querySelector("form").addEventListener("submit", async (e) => {
      e.preventDefault();
      const v = Object.fromEntries(new FormData(e.target));
      try {
        const { id } = await post("api/stock/locations", { name: v.name, parent: v.parent });
        toast(`${id} added`);
        dialog.close();
        load();
      } catch (error) { toast(error.message, "bad"); }
    });
  }

  // What a scan finds: an item, a location (with an item open: move it there), a material by
  // its EAN; a link from a label is read for its code.
  async function scan(raw) {
    let code = raw.trim();
    const linked = code.match(/[#/](S-\d+|L-\d+)$/i);
    if (linked) code = linked[1];
    code = code.toUpperCase();
    if (/^S-\d+$/.test(code)) return showItem(code);
    if (/^L-\d+$/.test(code)) {
      const loc = data.locations.find((l) => l.id === code);
      if (!loc) return toast(`No location ${code}`, "bad");
      if (openItem) {
        await post(`api/stock/items/${openItem}/move`, { location: code });
        toast(`${openItem} moved to ${loc.name}`);
        const again = openItem;
        await load();
        return showItem(again);
      }
      const items = data.items.filter((i) => i.location === code && i.state !== "used-up");
      return show(`${head(`<code>${esc(code)}</code> ${esc(loc.name)}`)}<section><ul class="items">${items.map((i) => `<li data-item="${esc(i.id)}"><code>${esc(i.id)}</code><span>${esc(material(i.material)?.name || "")}</span><span>${amount(i.amount, material(i.material)?.unit || "pcs")}</span></li>`).join("") || '<li class="muted">Nothing here</li>'}</ul><div class="item-actions"><button type="button" class="ghost" data-label-location="${esc(code)}">Print its label</button></div></section>`);
    }
    const byEan = data.materials.find((m) => m.ean && m.ean === raw.trim());
    if (byEan) return receive(byEan.id);
    if (/^\d{8,14}$/.test(raw.trim())) {
      toast("An unknown barcode: make a material of it");
      newMaterial();
      body.querySelector("[name=ean]").value = raw.trim();
      return;
    }
    toast(`Nothing found for ${raw}`, "bad");
  }

  $("[data-scan]").addEventListener("submit", (e) => {
    e.preventDefault();
    const input = $("[data-scan-input]");
    const value = input.value;
    input.value = "";
    if (value.trim()) scan(value);
  });
  // A scanner types fast and ends with Enter, wherever the cursor is: catch it outside fields.
  let typed = "", lastKey = 0;
  document.addEventListener("keydown", (e) => {
    if (e.target.closest("input, textarea, select") && !e.target.matches("[data-scan-input]")) return;
    if (e.target.matches("[data-scan-input]")) return;
    const t = performance.now();
    if (t - lastKey > 80) typed = "";
    lastKey = t;
    if (e.key === "Enter" && typed.length >= 4) { e.preventDefault(); scan(typed); typed = ""; return; }
    if (e.key.length === 1) typed += e.key;
  });

  document.addEventListener("click", (e) => {
    const t = e.target;
    if (t.closest("[data-receive]")) receive();
    if (t.closest("[data-new-material]")) newMaterial();
    if (t.closest("[data-new-location]")) newLocation();
    const em = t.closest("[data-edit-material]");
    if (em) { e.preventDefault(); newMaterial(em.dataset.editMaterial); }
    const rm = t.closest("[data-receive-material]");
    if (rm) { e.preventDefault(); receive(rm.dataset.receiveMaterial); }
    const it = t.closest("[data-item]");
    if (it) showItem(it.dataset.item);
    const ll = t.closest("[data-label-location]");
    if (ll) { const l = data.locations.find((x) => x.id === ll.dataset.labelLocation); label(`${l.id}\n${l.name}`, l.id); }
  });

  // The machines (for loading, and the label printer), live in the sidebar too.
  const stream = new EventSource("api/machines/stream");
  stream.onopen = () => live(true);
  stream.onerror = () => live(false);
  stream.onmessage = (e) => {
    const all = JSON.parse(e.data);
    summarise(all);
    const before = JSON.stringify(machines);
    machines = all.map((m) => ({ id: m.id, name: m.name, kind: m.kind }));
    // Machines named where items are loaded, once known.
    if (before !== JSON.stringify(machines) && data.materials.length) render();
  };
  load().then(() => {
    // Opened from a label's QR code: what it names.
    const code = location.hash.slice(1);
    if (/^(S|L)-\d+$/i.test(code)) { history.replaceState(null, "", location.pathname); scan(code); }
  });
