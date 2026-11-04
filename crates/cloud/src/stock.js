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
  // Its brand and name, the brand once (Devil Design's names start with it).
  const fullName = (m) => m.brand && !m.name.startsWith(m.brand) ? `${m.brand} ${m.name}` : m.name;

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
          <button type="button" class="ghost small" data-receive-material="${esc(m.id)}">Receive</button></summary>
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

  // New material: from the catalogue (a printer's tapes; the filaments of PrusaSlicer's
  // profiles, hundreds: found by typing) or by hand.
  function newMaterial() {
    const c = data.catalogue;
    const named = new Map(c.map((m) => [fullName(m), m]));
    show(`${head("New material")}<section>
      <form data-new-material-form class="grid-form">
        <label class="wide">Start from <input name="from" type="search" list="stock-catalogue" autocomplete="off" placeholder="Type: Prusament PETG, Devil Design PLA, 45013… or leave empty"><small class="hint" data-facts>${c.length} in the catalogue: tapes of your label printers, filaments from PrusaSlicer's profiles</small></label>
        <datalist id="stock-catalogue">${c.map((m) => `<option value="${esc(fullName(m))}">${esc(m.kind === "tape" ? "label tape" : facts(m))}</option>`).join("")}</datalist>
        <label>Kind <select name="kind">${Object.entries(KINDS).map(([k, t]) => `<option value="${k}">${t}</option>`).join("")}</select></label>
        <label>Brand <input name="brand"></label>
        <label class="wide">Name <input name="name" required></label>
        <label>Colour <input name="color" type="color" value="#cccccc"></label>
        <label>Unit <select name="unit"><option value="g">grams</option><option value="m">metres</option><option value="pcs">pieces</option></select></label>
        <label>One comes with <input name="package" type="number" step="any" min="0" value="1000"></label>
        <label>Keep at least <input name="minimum" type="number" min="0" value="1"></label>
        <label>Empty spool (g) <input name="spool_g" type="number" step="any" min="0" placeholder="weighed, to take off"></label>
        <label class="wide">EAN (its barcode, to scan it) <input name="ean" inputmode="numeric"></label>
        <div class="wide"><button>Add material</button></div>
      </form></section>`);
    const f = body.querySelector("form");
    const factsLine = body.querySelector("[data-facts]");
    const fill = (m) => {
      if (!m) return;
      for (const k of ["kind", "brand", "name", "unit", "package", "minimum"]) if (m[k] != null) f[k].value = m[k];
      f.spool_g.value = m.spool_g ?? "";
      if (m.color && m.color.startsWith("#")) f.color.value = m.color.length === 4 ? "#" + [...m.color.slice(1)].map((x) => x + x).join("") : m.color;
      factsLine.textContent = m.kind === "tape" ? "A label tape: loading it tells the printer which tape it has" : [facts(m), m.source && `from ${m.source.replace(/^prusaslicer-settings-/, "PrusaSlicer's ").replace("@", " profiles, ")}`].filter(Boolean).join(" · ");
    };
    f.from.addEventListener("input", () => fill(named.get(f.from.value)));
    f.addEventListener("submit", async (e) => {
      e.preventDefault();
      const base = named.get(f.from.value) || {};
      const v = Object.fromEntries(new FormData(f));
      try {
        await post("api/stock/materials", { ...base, kind: v.kind, brand: v.brand, name: v.name, unit: v.unit, color: base.color === "clear" ? "clear" : v.color, package: Number(v.package) || null, minimum: v.minimum === "" ? null : Number(v.minimum), ean: v.ean || null, spool_g: v.spool_g === "" ? null : Number(v.spool_g) });
        toast("Material added");
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
