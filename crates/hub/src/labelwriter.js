(() => {
  // A DYMO LabelWriter's page (HS2; its own script, apart from the LabelManager's label.js). The
  // label is a page of exactly the loaded roll's raster as the printer's port gives it (width
  // across the head, `cupsWidth` × `cupsHeight` dots, the PPD's imageable area at 300 dpi): drawn
  // as SVG at that size, rendered at one pixel per dot, shown as the dots that will print, and sent
  // as a PNG (a file, then a job). Nothing after this rotates or scales it.
  const ID = document.querySelector("main").dataset.id;
  const TABS = ["overview", "label", "settings"];
  function show() {
    const id = TABS.includes(location.hash.slice(1)) ? location.hash.slice(1) : "overview";
    for (const tab of document.querySelectorAll(".tab")) tab.hidden = tab.id !== id;
    for (const a of document.querySelectorAll(".tabs a")) a.setAttribute("aria-selected", a.hash === "#" + id);
    scrollTo(0, 0);
  }
  for (const a of document.querySelectorAll(".tabs a")) a.addEventListener("click", (event) => {
    event.preventDefault();
    history.replaceState(null, "", a.hash);
    show();
  });
  addEventListener("hashchange", show);
  show();

  const $ = (selector) => document.querySelector(selector);
  const $$ = (selector) => [...document.querySelectorAll(selector)];
  let roll = JSON.parse($("#roll-data").textContent);
  const text = $("[data-text]"), text2 = $("[data-text2]");
  const codeKind = $("[data-code-kind]"), codeData = $("[data-code-data]"), codeNote = $("[data-code-note]");
  const preview = $("[data-preview]"), paper = $("[data-paper]");
  const printButton = $("[data-print]"), printStatus = $("[data-print-status]"), copies = $("[data-copies]");
  const settings = { both: "same", size: 0.75, align: "middle", turn: 0 };

  // Shift (owner, 2026-11-17): the content moved along the label in steps of 0.25 mm, kept per
  // printer and roll in this browser (ours, UI only; the page sent keeps its size).
  const STEP_MM = 0.25;
  // Also across the label (owner, 2026-11-17: "nog steeds niet 100% goed op het label").
  const shiftKey = (axis = "") => `lw-shift${axis}-${ID}-${roll?.id}`;
  function shiftSteps(axis = "") {
    try { return parseInt(localStorage.getItem(shiftKey(axis)) || "0", 10) || 0; } catch { return 0; }
  }
  function setShift(axis, value) {
    try { localStorage.setItem(shiftKey(axis), String(value)); } catch {}
  }
  for (const b of $$("[data-shift]")) b.addEventListener("click", () => {
    const step = Number(b.dataset.shift);
    if (step === 0) { setShift("", 0); setShift("x", 0); } else setShift("", shiftSteps() + step);
    render();
  });
  for (const b of $$("[data-shift-x]")) b.addEventListener("click", () => {
    setShift("x", shiftSteps("x") + Number(b.dataset.shiftX));
    render();
  });

  // Pills: one pressed in each group.
  function pills(attr, key, parse = (v) => v) {
    for (const b of $$(`[data-${attr}]`)) b.addEventListener("click", () => {
      settings[key] = parse(b.dataset[attr]);
      for (const o of $$(`[data-${attr}]`)) o.setAttribute("aria-pressed", o === b);
      render();
    });
  }
  pills("both", "both");
  pills("size", "size", Number);
  pills("align", "align");
  pills("turn", "turn", Number);

  // Turn (owner, 2026-11-25: "de dymos printen standaard eigenlijk altijd 90 graden. nu moeten we dat
  // steeds instellen"): it starts where DYMO Connect's catalogue puts the roll's paper
  // (`DefaultOrientation`: Landscape 90°, Portrait 0°), and a turn chosen is kept per printer and roll in
  // this browser, as the shift (ours, UI only).
  const turnKey = () => `lw-turn-${ID}-${roll?.id}`;
  function loadTurn() {
    let kept = null;
    try { kept = localStorage.getItem(turnKey()); } catch {}
    const turn = kept !== null && [0, 90, 180, 270].includes(Number(kept)) ? Number(kept) : roll?.orientation === "Landscape" ? 90 : 0;
    settings.turn = turn;
    for (const o of $$("[data-turn]")) o.setAttribute("aria-pressed", Number(o.dataset.turn) === turn);
  }
  for (const b of $$("[data-turn]")) b.addEventListener("click", () => {
    try { localStorage.setItem(turnKey(), b.dataset.turn); } catch {}
  });
  loadTurn();

  // Another roll chosen (saved in the background): the page is drawn for it at once.
  $("[data-roll-form]")?.addEventListener("saved", (event) => {
    const { data, submitter } = event.detail;
    for (const b of event.target.querySelectorAll("button[name=media]")) b.setAttribute("aria-pressed", b === submitter);
    if (data && typeof data === "object") {
      roll = data;
      loadTurn();
      render();
    }
  });
  // The roll list, filtered as one types.
  const search = $("[data-roll-search]"), count = $("[data-roll-count]");
  search?.addEventListener("input", () => {
    const q = search.value.trim().toLowerCase();
    const rolls = $$(".lw-rolls .roll");
    let shown = 0;
    for (const r of rolls) {
      const on = !q || r.dataset.search.includes(q);
      r.hidden = !on;
      shown += on;
    }
    count.textContent = `${shown} of ${rolls.length} rolls in DYMO Connect's catalogue for this printer`;
  });

  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const measure = document.createElement("canvas").getContext("2d");
  let dots = null; // the page as it will print: { w, h, bits }

  // The codes to draw, as the hub's /api/codes makes them: { size, rows } (QR) or { bars }, one per
  // label: what "What it encodes" says, else that label's own text (owner, 2026-11-17: each label
  // of a pair its own code).
  const codes = new Map(); // `${kind}\n${data}` -> { code, error }
  const codeFor = (words) => codeData.value.trim() || words.trim().replace(/\s*\n\s*/g, " ");
  async function fetchCodes(datas) {
    const kind = codeKind.value;
    if (kind === "none") return;
    for (const data of new Set(datas)) {
      const key = `${kind}\n${data}`;
      if (!data || codes.has(key)) continue;
      const entry = { code: null, error: "" };
      try {
        const response = await fetch(`../api/codes?kind=${kind}&data=${encodeURIComponent(data)}`);
        if (response.ok) entry.code = await response.json();
        else entry.error = await response.text();
      } catch (error) {
        entry.error = error.message;
      }
      codes.set(key, entry);
    }
  }
  const codeEntry = (words) => codes.get(`${codeKind.value}\n${codeFor(words)}`);

  // The code on whole dots, at the left of a box of cw × ch: its SVG and its width with its
  // quiet zones. A QR code gets as many dots a module as the box allows; a barcode 3 dots a
  // module (0.25 mm at 300 dpi) when it fits, else fewer.
  function codeSvg(entry, cw, ch, pad) {
    const code = entry?.code;
    if (!code) return { svg: "", width: 0, warning: entry?.error || "" };
    const inner = ch - 2 * pad;
    let path = "", width = 0, warning = "";
    if (code.rows) {
      const scale = Math.floor(Math.min(inner, cw * 0.5) / (code.size + 4));
      if (scale < 1) return { svg: "", width: 0, warning: "Too much for this label: shorten it" };
      if (scale === 1) warning = "One dot a module: it may not scan. Shorten it, or use a larger label";
      const quiet = 2 * scale, side = code.size * scale;
      const y0 = pad + Math.floor((inner - side) / 2);
      code.rows.forEach((row, r) => {
        for (let c = 0; c < code.size; c++) {
          if (row[c] === "1") path += `M${pad + quiet + c * scale},${y0 + r * scale}h${scale}v${scale}h-${scale}z`;
        }
      });
      width = side + 2 * quiet;
    } else {
      const bars = code.bars;
      const module = Math.min(3, Math.floor((cw * 0.6 - 2 * pad) / (bars.length + 20)));
      if (module < 1) return { svg: "", width: 0, warning: "Too long for this label: shorten it" };
      if (module === 1) warning = "One dot a module: it may not scan";
      const quiet = 10 * module;
      for (let i = 0; i < bars.length; ) {
        if (bars[i] !== "1") { i++; continue; }
        let run = 0;
        while (bars[i + run] === "1") run++;
        path += `M${pad + quiet + i * module},${pad}h${run * module}v${inner}h-${run * module}z`;
        i += run;
      }
      width = bars.length * module + 2 * quiet;
    }
    return { svg: `<path d="${path}" fill="#000" shape-rendering="crispEdges"/>`, width, warning };
  }

  // One label's content in its own box (cw × ch): the code at the left, the text in the rest.
  function contentSvg(cw, ch, words, entry) {
    const pad = Math.max(2, Math.round(Math.min(cw, ch) * 0.04));
    const c = codeKind.value === "none" ? { svg: "", width: 0, warning: "" } : codeSvg(entry, cw, ch, pad);
    const left = pad + c.width + (c.width ? pad : 0);
    const room = cw - left - pad;
    const lines = words.split("\n").map((l) => l.trim()).filter((l, i, all) => l || all.length === 1);
    let svg = c.svg;
    if (lines.length && room > 4 && lines.some((l) => l)) {
      let size = ((ch - 2 * pad) / lines.length) * 0.82 * settings.size;
      measure.font = `700 ${size}px system-ui, sans-serif`;
      const widest = Math.max(...lines.map((l) => measure.measureText(l).width), 1);
      if (widest > room) size *= room / widest;
      const lead = size * 1.12, block = lead * lines.length;
      const x = settings.align === "middle" ? left + room / 2 : left;
      lines.forEach((l, i) => {
        const y = (ch - block) / 2 + lead * i + size * 0.88;
        svg += `<text x="${x.toFixed(1)}" y="${y.toFixed(1)}" font-family="system-ui, sans-serif" font-weight="700" font-size="${size.toFixed(1)}" text-anchor="${settings.align}" fill="#000">${esc(l)}</text>`;
      });
    }
    return { svg, warning: c.warning };
  }

  // The page as SVG at its raster's size, its content centred on each label DYMO Connect's catalogue
  // puts on the page (HS2b; cells in the raster's dots); then the owner's shift. A label reaching past
  // the page keeps its centre: the content's box is the largest one around that centre on the page.
  function labelCells() {
    const [W, H] = roll.dots;
    return (roll.cells || [[0, 0, W, H]]).map(([x, y, w, h]) => {
      const cx = x + w / 2, cy = y + h / 2;
      const hw = Math.max(0, Math.min(cx, W - cx, w / 2)), hh = Math.max(0, Math.min(cy, H - cy, h / 2));
      return [Math.round(cx - hw), Math.round(cy - hh), Math.round(2 * hw), Math.round(2 * hh)];
    });
  }
  const wordsFor = (i) => (i === 0 || settings.both === "same" ? text.value : text2.value);
  function pageSvg() {
    const [W, H] = roll.dots;
    let body = "", warning = "";
    const dx = Math.round((shiftSteps("x") * STEP_MM * 300) / 25.4);
    const dy = Math.round((shiftSteps() * STEP_MM * 300) / 25.4);
    labelCells().forEach(([x, y, w, h], i) => {
      if (w < 4 || h < 4) return;
      const t = settings.turn;
      const [cw, ch] = t % 180 === 0 ? [w, h] : [h, w];
      const move = { 0: `translate(${x},${y})`, 90: `translate(${x + w},${y}) rotate(90)`, 180: `translate(${x + w},${y + h}) rotate(180)`, 270: `translate(${x},${y + h}) rotate(270)` }[t];
      const words = wordsFor(i);
      const content = contentSvg(cw, ch, words, codeEntry(words));
      warning ||= content.warning;
      body += `<g transform="${move}">${content.svg}</g>`;
    });
    return { svg: `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}"><rect width="${W}" height="${H}" fill="#fff"/><g transform="translate(${dx},${dy})">${body}</g></svg>`, warning };
  }

  // The page rendered at one pixel per dot, black where it is darker than half.
  async function render() {
    // Several labels on the page (DYMO Connect's catalogue): "same" or "different" (two at most).
    const many = (roll?.cells || []).length;
    const two = many > 1;
    $("[data-two-note]").hidden = !two;
    $("[data-two-legend]").hidden = !two;
    $("[data-two-field]").hidden = many !== 2;
    if (many !== 2) settings.both = "same";
    $("[data-second]").hidden = !(many === 2 && settings.both === "different");
    $("[data-text-label]").textContent = two && settings.both === "different" ? "First label" : "Text";
    $("[data-code-field]").hidden = codeKind.value === "none";
    $("[data-preview-empty]").hidden = !!roll;
    paper.hidden = !roll;
    $("[data-legend]").hidden = !roll;
    $("[data-out]").hidden = !roll;
    if (!roll) {
      $("[data-roll-line]").textContent = "Choose the roll loaded (Roll loaded, on the right)";
      printButton.disabled = true;
      dots = null;
      return;
    }
    $("[data-roll-line]").textContent = `${roll.id}${roll.name ? " " + roll.name : ""} · ${roll.paper_name}`;
    $("[data-fact-page]").textContent = `${roll.width_mm} × ${roll.length_mm} mm`;
    $("[data-fact-dots]").textContent = `${roll.dots[0]} × ${roll.dots[1]} dots at 300 dpi`;
    $("[data-fact-from]").textContent = `DYMO's PPD · ${roll.page_size}${many > 1 ? ` · ${many} labels` : ""}`;
    const along = shiftSteps(), across = shiftSteps("x");
    const parts = [];
    if (along) parts.push(`${along < 0 ? "up" : "down"} ${(Math.abs(along) * STEP_MM).toFixed(2)} mm`);
    if (across) parts.push(`${across < 0 ? "left" : "right"} ${(Math.abs(across) * STEP_MM).toFixed(2)} mm`);
    $("[data-shift-note]").textContent = `Centred on each label · ${parts.length ? parts.join(", ") : "not shifted"}`;
    await fetchCodes(labelCells().map((_, i) => codeFor(wordsFor(i))));
    const { svg, warning } = pageSvg();
    codeNote.textContent = warning;
    const [W, H] = roll.dots;
    const image = new Image();
    image.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
    try { await image.decode(); } catch { return; }
    const canvas = document.createElement("canvas");
    canvas.width = W; canvas.height = H;
    const ctx = canvas.getContext("2d");
    ctx.drawImage(image, 0, 0);
    const pixels = ctx.getImageData(0, 0, W, H).data;
    const bits = new Uint8Array(W * H);
    for (let i = 0; i < bits.length; i++) bits[i] = pixels[i * 4] < 128 ? 1 : 0;
    dots = { w: W, h: H, bits };
    paint();
    printButton.disabled = false;
  }

  // The preview: the PPD's label (PaperDimension) on scale, the page sent inside it at the
  // imageable area, the raster's first row at the PPD's top edge. The page leaves the printer
  // first row first, seen from the front as drawn here (the owner's print, 2026-11-17).
  function paint() {
    if (!dots || !roll) return;
    const [pw, pl] = roll.paper, [l, b, r, t] = roll.imageable;
    const s = Math.min(520 / pw, 420 / pl, 5);
    paper.style.width = `${(pw * s).toFixed(1)}px`;
    paper.style.height = `${(pl * s).toFixed(1)}px`;
    Object.assign(preview.style, { left: `${(l * s).toFixed(1)}px`, top: `${((pl - t) * s).toFixed(1)}px`, width: `${((r - l) * s).toFixed(1)}px`, height: `${((t - b) * s).toFixed(1)}px` });
    preview.width = dots.w; preview.height = dots.h;
    const ctx = preview.getContext("2d");
    const out = ctx.createImageData(dots.w, dots.h);
    for (let i = 0; i < dots.bits.length; i++) {
      const v = dots.bits[i] ? 0 : 255;
      out.data.set([v, v, v, 255], i * 4);
    }
    ctx.putImageData(out, 0, 0);
    // The labels on the page (DYMO Connect's catalogue): their edges, on the preview only, never sent.
    if ((roll.cells || []).length > 1) {
      ctx.fillStyle = getComputedStyle(document.documentElement).getPropertyValue("--warn") || "#b7791f";
      for (const [x, y, w, h] of labelCells()) {
        for (let i = 0; i < w; i += 6) { ctx.fillRect(x + i, y, 3, 1); ctx.fillRect(x + i, y + h - 1, 3, 1); }
        for (let i = 0; i < h; i += 6) { ctx.fillRect(x, y + i, 1, 3); ctx.fillRect(x + w - 1, y + i, 1, 3); }
      }
    }
  }

  // The dots as a black and white PNG, one pixel per dot.
  function png() {
    const canvas = document.createElement("canvas");
    canvas.width = dots.w; canvas.height = dots.h;
    const ctx = canvas.getContext("2d");
    const out = ctx.createImageData(dots.w, dots.h);
    for (let i = 0; i < dots.bits.length; i++) {
      const v = dots.bits[i] ? 0 : 255;
      out.data.set([v, v, v, 255], i * 4);
    }
    ctx.putImageData(out, 0, 0);
    return new Promise((resolve) => canvas.toBlob(resolve, "image/png"));
  }

  printButton.addEventListener("click", async () => {
    if (!dots || !roll) return;
    printButton.disabled = true;
    printStatus.textContent = "Sending…";
    try {
      const first = text.value.split("\n")[0].trim().toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 40);
      const name = `label-${first || "page"}.png`;
      const uploaded = await fetch(`../api/files?name=${encodeURIComponent(name)}`, { method: "POST", body: await png() });
      if (!uploaded.ok) throw new Error(await uploaded.text());
      const { id } = await uploaded.json();
      const n = Math.min(50, Math.max(1, parseInt(copies.value, 10) || 1));
      const printed = await fetch(`../files/${id}/print`, {
        method: "POST",
        body: new URLSearchParams({ machine: ID, copies: String(n) }),
        headers: { "x-requested-with": "fetch" },
      });
      if (!printed.ok) throw new Error(await printed.text());
      // 202: the printer is busy, or others wait: into its queue, at this place.
      const queued = printed.status === 202 ? (await printed.json()).queued : null;
      printStatus.textContent = queued
        ? `The printer is busy: in its queue, number ${queued}`
        : n > 1 ? `Sent to the printer (${n} copies, the rest queued)` : "Sent to the printer";
    } catch (error) {
      printStatus.textContent = `Not printed: ${error.message}`;
    } finally {
      printButton.disabled = !roll;
    }
  });

  // Sent here with a label to make (from Stock): its text and the QR code's link.
  {
    const wanted = new URLSearchParams(location.search);
    if (wanted.has("label")) {
      text.value = wanted.get("label");
      if (wanted.get("qr")) {
        codeKind.value = "qr";
        codeData.value = wanted.get("qr");
      }
      try { history.replaceState(null, "", location.pathname + location.hash); } catch {}
    }
  }

  let pending = 0;
  const later = () => { clearTimeout(pending); pending = setTimeout(render, 120); };
  for (const input of [text, text2, codeKind, codeData]) input.addEventListener("input", later);
  render();
  document.fonts?.ready.then(render);

  // The printer, live.
  const setLive = (key, value) => {
    for (const el of document.querySelectorAll(`[data-live="${key}"]`)) el.textContent = value;
  };
  const stream = new EventSource("../api/machines/stream");
  stream.onmessage = (event) => {
    const machines = JSON.parse(event.data);
    if (typeof summarise === "function") summarise(machines);
    const machine = machines.find((m) => m.id === ID);
    if (!machine) return;
    const t = machine.online ? machine.telemetry : null;
    if (typeof showState === "function") showState(machine);
    // A LabelWriter says only when its labels have run out.
    setLive("roll", !t ? "–" : (t.problem || "").startsWith("no labels") ? "Empty" : "Loaded");
    setLive("job", machine.job ? `${machine.job.file_name} (${machine.job.state})` : "–");
  };
})();
