(() => {
  // A label printer's page: the label drawn as SVG at the tape's size, rendered at one pixel
  // per dot, shown as the dots that will burn, and sent as a PNG (a file, then a job).
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

  let tape = JSON.parse(document.getElementById("tape-data").textContent);
  // Without a tape said, drawn for 12 mm (64 dots at 180 dpi).
  let H = tape ? tape.dots : 64;
  let DPI = tape ? tape.dpi : 180;
  // Another tape chosen (saved in the background): the label is drawn for it at once.
  document.querySelector("[data-tape-form]")?.addEventListener("saved", (event) => {
    const { data, submitter } = event.detail;
    for (const b of event.target.querySelectorAll("button[name=media]")) b.setAttribute("aria-pressed", b === submitter);
    if (data && typeof data === "object") {
      tape = data;
      H = tape.dots || 64;
      DPI = tape.dpi || 180;
      codeKey = "";
      render();
    }
  });
  const $ = (selector) => document.querySelector(selector);
  const text = $("[data-text]"), font = $("[data-font]"), weight = $("[data-weight]");
  const size = $("[data-size]"), frame = $("[data-frame]"), invert = $("[data-invert]");
  const drawingInput = $("[data-drawing]"), drawingRemove = $("[data-drawing-remove]");
  const preview = $("[data-preview]"), note = $("[data-preview-note]");
  const printButton = $("[data-print]"), printStatus = $("[data-print-status]");
  const svgSource = $("[data-svg]");
  const codeKind = $("[data-code-kind]"), codeData = $("[data-code-data]"), codeNote = $("[data-code-note]");

  // Dots kept blank above and below, before and after the content, and between its parts:
  // set for 180 dpi, scaled to the printer's. A die-cut label keeps 1.5 mm free at its
  // edges, as DYMO's own software does (the head cannot reach them reliably).
  const at = (dots) => Math.max(1, Math.round((dots * DPI) / 180));
  function spacing() {
    const edge = tape?.length_dots ? Math.round((1.5 * DPI) / 25.4) : 0;
    return { edge, margin: Math.max(at(3), edge), pad: Math.max(at(8), edge), gap: at(10), inset: frame.checked ? at(4) : 0, leading: at(2) };
  }
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const measure = document.createElement("canvas").getContext("2d");
  let drawing = null; // { href, w, h }
  let dots = null; // the label as it will print: { length, bits }
  // The code to draw, as the cloud made it: { size, rows } (QR) or { bars }.
  let code = null, codeKey = "", codeError = "";

  // What the code encodes: its own field, else the text on one line.
  async function fetchCode() {
    const kind = codeKind.value;
    const data = codeData.value.trim() || text.value.trim().replace(/\s*\n\s*/g, " ");
    const key = `${kind}\n${data}`;
    if (key === codeKey) return;
    codeKey = key; code = null; codeError = "";
    if (kind === "none" || !data) return;
    try {
      const response = await fetch(`../api/codes?kind=${kind}&data=${encodeURIComponent(data)}`);
      if (response.ok) code = await response.json();
      else codeError = await response.text();
    } catch (error) {
      codeError = error.message;
    }
  }

  // The code on whole dots at x: its SVG, its length with its quiet zones, and a note when
  // it may not scan. A QR code gets as many dots a module as the tape allows; a barcode two
  // dots a module (0.28 mm at 180 dpi) and its full height.
  function codeSvg(x, top, inner) {
    if (!code) return { svg: "", width: 0, warning: codeError };
    let path = "", width = 0, warning = "";
    if (code.rows) {
      const scale = Math.floor(inner / code.size);
      if (scale < 1) return { svg: "", width: 0, warning: "Too much for this tape: shorten it" };
      if (scale === 1) warning = "One dot a module: it may not scan. Shorten it, or use wider tape";
      const quiet = 2 * scale;
      const y0 = top + Math.floor((inner - code.size * scale) / 2);
      code.rows.forEach((row, r) => {
        for (let c = 0; c < code.size; c++) {
          if (row[c] === "1") path += `M${x + quiet + c * scale},${y0 + r * scale}h${scale}v${scale}h-${scale}z`;
        }
      });
      width = code.size * scale + 2 * quiet;
    } else {
      const module = at(2), quiet = 10 * module;
      const bars = code.bars;
      for (let i = 0; i < bars.length; ) {
        if (bars[i] !== "1") { i++; continue; }
        let run = 0;
        while (bars[i + run] === "1") run++;
        path += `M${x + quiet + i * module},${top}h${run * module}v${inner}h-${run * module}z`;
        i += run;
      }
      width = bars.length * module + 2 * quiet;
    }
    return { svg: `<path d="${path}" fill="#000" shape-rendering="crispEdges"/>`, width, warning };
  }

  // The label as SVG: white is tape, black burns.
  function labelSvg() {
    const { edge, margin, pad, gap, inset, leading: LEADING } = spacing();
    // A die-cut label is as long as it is; tape as long as its content.
    const fixed = tape?.length_dots || 0;
    const lines = text.value.split("\n").map((l) => l.trim()).filter(Boolean).slice(0, 3);
    const inner = H - 2 * margin - 2 * inset;
    const family = font.value, bold = weight.value;
    // The glyphs' real height, measured at 100 px, so the text fills the tape.
    measure.font = `${bold} 100px ${family}`;
    let ascent = 0, descent = 0;
    for (const line of lines) {
      const m = measure.measureText(line);
      ascent = Math.max(ascent, m.actualBoundingBoxAscent);
      descent = Math.max(descent, m.actualBoundingBoxDescent);
    }
    // A little between lines, so a descender does not touch the line under it.
    let lineH = ((inner * size.value) / 100 - LEADING * Math.max(lines.length - 1, 0)) / Math.max(lines.length, 1);
    let px = ascent + descent > 0 ? (lineH / (ascent + descent)) * 100 : lineH;
    measure.font = `${bold} ${px}px ${family}`;
    let textW = Math.ceil(Math.max(0, ...lines.map((l) => measure.measureText(l).width)));
    const drawH = inner, drawW = drawing ? Math.round((drawing.w / drawing.h) * drawH) : 0;
    let x = pad + inset;
    const parts = [];
    if (drawing) {
      parts.push(`<image href="${esc(drawing.href)}" x="${x}" y="${margin + inset}" width="${drawW}" height="${drawH}" preserveAspectRatio="xMidYMid meet"/>`);
      x += drawW + gap;
    }
    // The code, after the drawing; its quiet zone is the gap to the text.
    const made = codeSvg(x, margin + inset, inner);
    codeNote.textContent = made.warning;
    if (made.width) {
      parts.push(made.svg);
      x += made.width;
    }
    // On a die-cut label the text gets smaller until it fits.
    let warning = "";
    if (fixed) {
      const room = fixed - pad - inset - x;
      if (room < at(6)) warning = "Too much for this label: leave something out";
      else if (textW > room) {
        const k = room / textW;
        px *= k; lineH *= k; textW = room;
      }
    }
    const blockH = lineH * lines.length + LEADING * Math.max(lines.length - 1, 0);
    const top = margin + inset + (inner - blockH) / 2;
    lines.forEach((line, i) => {
      const baseline = top + i * (lineH + LEADING) + (lineH - (ascent + descent) * px / 100) / 2 + ascent * px / 100;
      parts.push(`<text x="${x}" y="${baseline.toFixed(2)}" font-family="${esc(family)}" font-weight="${bold}" font-size="${px.toFixed(2)}">${esc(line)}</text>`);
    });
    const natural = Math.max(x + textW + pad + inset, 2 * pad + 1);
    const length = fixed || natural;
    // The content in the middle of a die-cut label.
    const shift = fixed ? Math.max(0, Math.floor((fixed - natural) / 2)) : 0;
    const stroke = at(2), b = edge ? Math.round(edge * 0.6) : stroke * 0.75;
    const border = frame.checked
      ? `<rect x="${b}" y="${b}" width="${length - 2 * b}" height="${H - 2 * b}" rx="${at(5)}" fill="none" stroke="#000" stroke-width="${stroke}"/>`
      : "";
    return {
      length,
      warning,
      svg: `<svg xmlns="http://www.w3.org/2000/svg" width="${length}" height="${H}" viewBox="0 0 ${length} ${H}"><rect width="${length}" height="${H}" fill="#fff"/>${border}<g transform="translate(${shift} 0)">${parts.join("")}</g></svg>`,
    };
  }

  // The SVG at one pixel per dot, then each dot on or off.
  async function render() {
    await fetchCode();
    const { svg, length, warning } = labelSvg();
    svgSource.textContent = svg;
    const image = new Image();
    image.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
    try { await image.decode(); } catch { note.textContent = "The drawing does not render."; return; }
    const canvas = document.createElement("canvas");
    canvas.width = length; canvas.height = H;
    const ctx = canvas.getContext("2d");
    ctx.drawImage(image, 0, 0, length, H);
    const pixels = ctx.getImageData(0, 0, length, H).data;
    const bits = new Uint8Array(length * H);
    for (let i = 0; i < bits.length; i++) {
      const [r, g, b, a] = pixels.subarray(i * 4, i * 4 + 4);
      const dark = a >= 128 && (r * 3 + g * 6 + b) / 10 < 128;
      bits[i] = dark !== invert.checked ? 1 : 0;
    }
    dots = { length, bits, warning };
    paint();
  }

  // The dots, in the tape's own colours, large enough to see each.
  function paint() {
    const { length, bits } = dots;
    preview.width = length; preview.height = H;
    const ctx = preview.getContext("2d");
    const out = ctx.createImageData(length, H);
    const rgb = (hex) => {
      if (!hex || hex === "clear") return null;
      const h = hex.replace("#", "");
      const full = h.length === 3 ? [...h].map((c) => c + c).join("") : h;
      return [0, 2, 4].map((i) => parseInt(full.slice(i, i + 2), 16));
    };
    const ink = rgb(tape?.ink) || [0, 0, 0];
    const paper = rgb(tape?.background) || [236, 240, 243];
    for (let i = 0; i < bits.length; i++) {
      out.data.set([...(bits[i] ? ink : paper), 255], i * 4);
    }
    ctx.putImageData(out, 0, 0);
    const room = preview.parentElement.clientWidth - 2;
    // Whole dots when there is room; a long label at 300 dpi is shown smaller.
    const zoom = room >= length ? Math.min(6, Math.floor(room / length)) : room / length;
    preview.style.width = `${length * zoom}px`;
    preview.style.height = `${H * zoom}px`;
    const mm = (length / DPI) * 25.4;
    note.textContent = !tape
      ? `${mm.toFixed(0)} mm long, drawn for 12 mm tape: say which tape or labels are loaded to print`
      : tape.length_dots
        ? `${tape.name} (${H} × ${length} dots, ${DPI} dpi)${dots.warning ? `: ${dots.warning}` : ""}`
        : `${mm.toFixed(0)} mm long on ${tape.name} (${H} dots across, ${DPI} dpi), plus the tape the printer feeds before and after`;
    printButton.disabled = !tape;
  }

  // The dots as a black and white PNG, one pixel per dot.
  function png() {
    const { length, bits } = dots;
    const canvas = document.createElement("canvas");
    canvas.width = length; canvas.height = H;
    const ctx = canvas.getContext("2d");
    const out = ctx.createImageData(length, H);
    for (let i = 0; i < bits.length; i++) {
      const v = bits[i] ? 0 : 255;
      out.data.set([v, v, v, 255], i * 4);
    }
    ctx.putImageData(out, 0, 0);
    return new Promise((resolve) => canvas.toBlob(resolve, "image/png"));
  }

  printButton.addEventListener("click", async () => {
    if (!dots || !tape) return;
    printButton.disabled = true;
    printStatus.textContent = "Sending…";
    try {
      const first = text.value.split("\n")[0].trim().toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 40);
      const name = `label-${first || "drawing"}.png`;
      const uploaded = await fetch(`../api/files?name=${encodeURIComponent(name)}`, { method: "POST", body: await png() });
      if (!uploaded.ok) throw new Error(await uploaded.text());
      const { id } = await uploaded.json();
      const printed = await fetch(`../files/${id}/print`, {
        method: "POST",
        body: new URLSearchParams({ machine: ID }),
        headers: { "x-requested-with": "fetch" },
      });
      if (!printed.ok) throw new Error(await printed.text());
      printStatus.textContent = "Sent to the printer";
    } catch (error) {
      printStatus.textContent = `Not printed: ${error.message}`;
    } finally {
      printButton.disabled = !tape;
    }
  });

  // A drawing to place before the text: an SVG (or a PNG), at the tape's height.
  drawingInput.addEventListener("change", async () => {
    const file = drawingInput.files[0];
    if (!file) return;
    const href = await new Promise((resolve) => {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result);
      reader.readAsDataURL(file);
    });
    const image = new Image();
    image.src = href;
    try { await image.decode(); } catch { printStatus.textContent = "That drawing does not open."; return; }
    let w = image.naturalWidth, h = image.naturalHeight;
    // An SVG without a size of its own: its viewBox says its shape.
    if (!w || !h) {
      const box = atob(href.split(",")[1] || "").match(/viewBox="[\d.\-]+[ ,]+[\d.\-]+[ ,]+([\d.]+)[ ,]+([\d.]+)"/);
      [w, h] = box ? [+box[1], +box[2]] : [1, 1];
    }
    drawing = { href, w, h };
    drawingRemove.hidden = false;
    render();
  });
  drawingRemove.addEventListener("click", () => {
    drawing = null;
    drawingInput.value = "";
    drawingRemove.hidden = true;
    render();
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
      // The address without the label in it (a reload starts fresh); never in the way.
      try { history.replaceState(null, "", location.pathname + location.hash); } catch {}
    }
  }

  let pending = 0;
  const later = () => { clearTimeout(pending); pending = setTimeout(render, 120); };
  for (const input of [text, font, weight, size, frame, invert, codeKind, codeData]) input.addEventListener("input", later);
  size.addEventListener("input", () => {
    $("[data-size-note]").textContent = size.value == 100 ? `fills the ${tape?.length_dots ? "label" : "tape"}` : `${size.value} % of the ${tape?.length_dots ? "label" : "tape"}`;
  });
  addEventListener("resize", () => dots && paint());
  // System fonts may load after the page: render again once they have.
  render();
  document.fonts?.ready.then(render);

  // The printer, live.
  const setLive = (key, value) => {
    for (const el of document.querySelectorAll(`[data-live="${key}"]`)) el.textContent = value;
  };
  const stream = new EventSource("../api/machines/stream");
  stream.onopen = () => live(true);
  stream.onerror = () => live(false);
  stream.onmessage = (event) => {
    const machines = JSON.parse(event.data);
    summarise(machines);
    const machine = machines.find((m) => m.id === ID);
    if (!machine) return;
    const t = machine.online ? machine.telemetry : null;
    showState(machine);
    setLive("cassette", t?.cassette == null ? "–" : t.cassette ? "Loaded" : "Empty");
    // A LabelWriter's roll: it says only when the labels have run out.
    setLive("roll", !t ? "–" : (t.problem || "").startsWith("no labels") ? "Empty" : "Loaded");
    setLive("job", machine.job ? `${machine.job.file_name} (${machine.job.state})` : "–");
    setLive("problem", t?.problem || "");
    // The battery, as the page says it on the server too (dymo::battery_percent,
    // machine_page::battery_tile): how full, from its voltage.
    const v = t?.battery_v;
    const pct = v == null ? null : batteryPercent(v);
    setLive("battery_pct", pct == null ? "–" : String(pct));
    setLive("battery_note", v == null ? "not reported" : `${v.toFixed(2)} V · ${v < 7.0 ? "too low to print" : v < 7.2 ? "low" : "estimated"}`);
    const battery = document.querySelector("[data-battery]");
    if (battery) {
      battery.dataset.level = pct == null ? "none" : pct < 15 ? "low" : pct < 30 ? "mid" : "ok";
      battery.querySelector("[data-battery-fill]").setAttribute("width", ((pct || 0) * 19 / 100).toFixed(1));
    }
  };
})();
