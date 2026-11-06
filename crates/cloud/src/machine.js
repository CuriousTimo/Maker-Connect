  // A machine's page: tabs, commands without leaving the page, the temperature chart (and
  // its table), and every [data-live] value kept live from the server-sent events.
  const ID = document.querySelector("main").dataset.id;
  // Tabs by purpose; a tab may show more than one section. Old addresses still lead there.
  const TABS = { overview: ["overview"], queue: ["queue"], telemetry: ["telemetry"], control: ["control"], terminal: ["terminal"], bedmesh: ["bedlevel"], usb: ["usb"], settings: ["settings"] };
  const OLD = { data: "telemetry", bedlevel: "bedmesh" };
  // Set below for a Prusa on its own Connect: opens its USB stick when its tab is shown.
  let usbOpen = null;
  function show() {
    const asked = location.hash.slice(1);
    const id = TABS[asked] ? asked : OLD[asked] || "overview";
    for (const tab of document.querySelectorAll(".tab")) tab.hidden = !TABS[id].includes(tab.id);
    for (const a of document.querySelectorAll(".tabs a")) a.setAttribute("aria-selected", a.hash === "#" + id);
    if (id === "telemetry") draw();
    if (id === "bedmesh") drawMesh();
    if (id === "usb") usbOpen?.();
    // A tab is a page of its own: from the top, not where the browser jumped to its id.
    scrollTo(0, 0);
  }
  for (const a of document.querySelectorAll(".tabs a")) a.addEventListener("click", (event) => {
    event.preventDefault();
    history.replaceState(null, "", a.hash);
    show();
  });

  // Each button sends its own action (without a script, the form posts it the plain way).
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  for (const form of document.querySelectorAll("form.command")) {
    const status = form.closest(".card")?.querySelector("[data-command-status]");
    // Not form.action: the buttons are named "action", and the form hands those back.
    const url = new URL(form.getAttribute("action"), location.href);
    for (const button of form.querySelectorAll("button[name=action]")) button.addEventListener("click", async (event) => {
      event.preventDefault();
      button.disabled = true;
      try {
        // The jog pad's step and speed go with its moves.
        const sent = new URLSearchParams({ action: button.value });
        const step = form.querySelector("[name=step]:checked"), feed = form.querySelector("[name=feed]");
        if (step) sent.set("step", step.value);
        if (feed) sent.set("feed", feed.value);
        // A typed temperature goes with its own button.
        const heat = button.closest(".heat-set")?.querySelector("input");
        if (heat) sent.set(heat.name, heat.value);
        const response = await fetch(url, {
          method: "POST",
          body: sent,
          headers: { "x-requested-with": "fetch" },
        });
        if (status) status.textContent = response.ok ? "" : `Not sent: ${await response.text()}`;
      } catch (error) {
        if (status) status.textContent = `Not sent: ${error.message}`;
      } finally {
        button.disabled = false;
      }
    });
  }
  // The lines for a Klipper printer's moonraker.conf, with this cloud's address.
  for (const pre of document.querySelectorAll("[data-moonraker-conf]")) {
    pre.textContent = `[simplyprint]\nurl: ws://${location.host}/simplyprint/${pre.dataset.token}`;
    const local = ["localhost", "127.0.0.1", "[::1]"].includes(location.hostname);
    pre.parentElement.querySelector("[data-moonraker-local]").hidden = !local;
  }
  // Enter in a temperature field sets that temperature (not the form's first button, Home).
  for (const input of document.querySelectorAll(".heat-set input")) input.addEventListener("keydown", (event) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    input.closest(".heat-set").querySelector("button").click();
  });
  // The arrow keys move the pad's head once it has focus: a move at a time (a key held down
  // does not pile them up), Home on the Home key.
  for (const pad of document.querySelectorAll("[data-jogpad]")) {
    const keys = { ArrowUp: "jog:0:1", ArrowDown: "jog:0:-1", ArrowLeft: "jog:-1:0", ArrowRight: "jog:1:0", Home: "home" };
    pad.addEventListener("keydown", (event) => {
      const action = keys[event.key];
      if (!action || event.target.matches("select")) return;
      event.preventDefault();
      const button = pad.querySelector(`button[value="${action}"]`);
      if (button && !button.disabled) button.click();
    });
  }
  const commandItem = (c) =>
    `<li><span>${esc(c.label)}</span><b class="badge ${c.state}">${c.state}</b>${c.reason ? ` <small>${esc(c.reason)}</small>` : ""}</li>`;

  // What someone should know first, as the server draws it (machine_page::notices).
  function noticesHtml(m, t) {
    let html = "";
    if (m.out_of_use) {
      const why = m.out_of_use.reason ? `: ${esc(m.out_of_use.reason)}` : "";
      html += `<div class="notice out"><span><b>Out of use</b>${why}. It takes no jobs until it is back.</span><form method="post" action="${esc(m.id)}/out-of-use" data-async data-done="Back in use" data-then="reload"><input type="hidden" name="action" value="back"><button class="ghost">Back in use</button></form></div>`;
    }
    html += confirmHtml(m, "");
    if (m.on_bed) {
      const warm = t.bed_c != null && t.bed_c > TAKE_BELOW_C;
      const how = warm ? `the bed cools (${t.bed_c.toFixed(0)} °C); take it at ${TAKE_BELOW_C} °C or below` : "the bed is cool: take it off";
      html += `<div class="notice bed"><span><b>${esc(m.on_bed.file_name)}</b> is done and on the bed: ${how}. The next job waits until it is taken.</span><form method="post" action="${esc(m.id)}/taken" data-async data-done="Print taken" data-then="reload"><button>Print taken</button></form></div>`;
    }
    return html;
  }

  // How a heater is doing, as the page says it on the server too.
  function heatNote(now, target) {
    if (now == null) return "not reported";
    if (target > 0) {
      const to = `${target.toFixed(0)} °C`;
      if (Math.abs(now - target) < 2) return `target ${to}`;
      return now < target ? `▲ heating to ${to}` : `▼ cooling to ${to}`;
    }
    const told = target != null ? "target off" : "no target told";
    return now > 40 ? `${told} · ▼ cooling` : told;
  }

  // The temperatures over the last half hour: nozzle and bed, their targets dashed.
  const SPAN = 30 * 60;
  const chart = document.getElementById("chart");
  const tip = document.getElementById("tip");
  let samples = [];
  const degrees = (v) => (v == null ? "–" : `${v.toFixed(1)} °C`);
  function draw() {
    if (!chart) return;
    const w = chart.clientWidth;
    if (!w) return;
    const h = 300, m = { l: 48, r: 110, t: 12, b: 30 };
    const now = Date.now() / 1000;
    const all = samples.flatMap((s) => [s.nozzle, s.nozzle_target, s.bed, s.bed_target]).filter((v) => v != null);
    const top = Math.max(50, Math.ceil((Math.max(0, ...all) + 5) / 50) * 50);
    const step = top <= 100 ? 25 : 50;
    const x = (t) => m.l + (w - m.l - m.r) * (1 - (now - t) / SPAN);
    const y = (v) => m.t + (h - m.t - m.b) * (1 - v / top);
    let svg = "";
    for (let v = 0; v <= top; v += step) {
      svg += `<line class="grid" x1="${m.l}" x2="${w - m.r}" y1="${y(v)}" y2="${y(v)}"/>`;
      svg += `<text class="axis" x="${m.l - 10}" y="${y(v) + 4}" text-anchor="end">${v}°</text>`;
    }
    for (const ago of [30, 25, 20, 15, 10, 5, 0]) {
      const anchor = ago === 30 ? "start" : ago ? "middle" : "end";
      svg += `<text class="axis" x="${x(now - ago * 60)}" y="${h - 6}" text-anchor="${anchor}">${ago ? `−${ago} min` : "now"}</text>`;
    }
    const path = (key) => {
      let d = "", pen = "M";
      for (const s of samples) {
        if (s[key] == null) { pen = "M"; continue; }
        d += `${pen}${x(s.t).toFixed(1)},${y(s[key]).toFixed(1)}`;
        pen = "L";
      }
      return d;
    };
    for (const [key, cls] of [["nozzle_target", "line nozzle target"], ["bed_target", "line bed target"], ["nozzle", "line nozzle"], ["bed", "line bed"]]) {
      svg += `<path class="${cls}" d="${path(key)}"/>`;
    }
    const last = samples[samples.length - 1];
    if (last) {
      // Direct labels at the line ends, kept apart when the lines are close.
      const ends = [["Nozzle", last.nozzle, "nozzle"], ["Bed", last.bed, "bed"]]
        .filter(([, v]) => v != null)
        .map(([n, v, c]) => ({ n, v, c, y: y(v) }));
      if (ends.length === 2 && Math.abs(ends[0].y - ends[1].y) < 16) {
        const mid = (ends[0].y + ends[1].y) / 2, upper = ends[0].y <= ends[1].y ? 0 : 1;
        ends[upper].y = mid - 9;
        ends[1 - upper].y = mid + 9;
      }
      for (const e of ends) {
        svg += `<circle class="dot ${e.c}" cx="${x(last.t)}" cy="${y(e.v)}" r="4.5"/>`;
        svg += `<text class="end" x="${w - m.r + 12}" y="${e.y + 4}">${e.n} ${e.v.toFixed(0)}°</text>`;
      }
    }
    svg += `<line id="cross" class="cross" y1="${m.t}" y2="${h - m.b}" visibility="hidden"/>`;
    chart.setAttribute("viewBox", `0 0 ${w} ${h}`);
    chart.setAttribute("height", h);
    chart.innerHTML = svg;
    chart.onpointermove = (event) => {
      if (!samples.length) return;
      const px = event.clientX - chart.getBoundingClientRect().left;
      const s = samples.reduce((a, b) => (Math.abs(x(b.t) - px) < Math.abs(x(a.t) - px) ? b : a));
      const cross = document.getElementById("cross");
      cross.setAttribute("x1", x(s.t));
      cross.setAttribute("x2", x(s.t));
      cross.setAttribute("visibility", "visible");
      tip.innerHTML = `<b>${new Date(s.t * 1000).toLocaleTimeString()}</b>`
        + `<span><i class="key key-nozzle"></i>Nozzle ${degrees(s.nozzle)} → ${degrees(s.nozzle_target)}</span>`
        + `<span><i class="key key-bed"></i>Bed ${degrees(s.bed)} → ${degrees(s.bed_target)}</span>`;
      tip.hidden = false;
      tip.style.left = `${Math.max(0, Math.min(x(s.t) + 14, w - tip.offsetWidth))}px`;
    };
    chart.onpointerleave = () => {
      tip.hidden = true;
      document.getElementById("cross")?.setAttribute("visibility", "hidden");
    };
  }

  // The same numbers as a table: one row a minute, newest first.
  const table = document.querySelector("[data-table]");
  const toggle = document.querySelector("[data-table-toggle]");
  function fillTable() {
    if (!table || table.hidden) return;
    const rows = [];
    let minute = null;
    for (const s of [...samples].reverse()) {
      const m = Math.floor(s.t / 60);
      if (m === minute) continue;
      minute = m;
      rows.push(`<tr><td>${new Date(s.t * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</td><td>${degrees(s.nozzle)}</td><td>${degrees(s.nozzle_target)}</td><td>${degrees(s.bed)}</td><td>${degrees(s.bed_target)}</td></tr>`);
    }
    table.innerHTML = `<table><thead><tr><th>Time</th><th>Nozzle</th><th>Target</th><th>Bed</th><th>Target</th></tr></thead><tbody>${rows.join("") || '<tr><td colspan="5">No readings yet</td></tr>'}</tbody></table>`;
  }
  toggle?.addEventListener("click", () => {
    const showTable = table.hidden;
    table.hidden = !showTable;
    document.querySelector(".plot").hidden = showTable;
    document.querySelector(".chart-card .legend").hidden = showTable;
    toggle.setAttribute("aria-pressed", String(showTable));
    toggle.textContent = showTable ? "Chart" : "Table";
    if (showTable) fillTable(); else draw();
  });

  // A Prusa's USB stick, through its own Connect: its folders, each file's picture (asked of
  // the printer one at a time), print and delete, a new folder, a file of the space sent to it.
  {
    const usb = document.querySelector("[data-usb]");
    if (usb) {
      const list = usb.querySelector("[data-usb-list]");
      const crumbs = usb.querySelector("[data-usb-crumbs]");
      const status = usb.querySelector("[data-usb-status]");
      const fileSelect = usb.querySelector("[data-usb-file]");
      const api = (path) => `../api/machines/${ID}/usb?path=${encodeURIComponent(path)}`;
      const post = (what, fields) => fetch(`../machines/${ID}/usb/${what}`, {
        method: "POST", body: new URLSearchParams(fields), headers: { "x-requested-with": "fetch" },
      });
      const size = (n) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1e3))} kB`);
      const date = (t) => (t ? new Date(t * 1000).toLocaleDateString() : "");
      let path = "/usb", timer = null, generation = 0;

      // One request a folder: its files with what is known of each (picture, time, material),
      // which the cloud reads ahead from the printer; asked again quickly while some are to come.
      async function load() {
        clearTimeout(timer);
        const mine = ++generation;
        let r;
        try { r = await (await fetch(api(path))).json(); } catch { status.textContent = "The cloud does not answer."; return; }
        if (mine !== generation) return;
        render(r);
        const busy = r.online && (r.asking || r.pending > 0);
        timer = setTimeout(load, busy ? 1500 : 15000);
      }
      function render(r) {
        const parts = path.split("/").filter(Boolean);
        crumbs.innerHTML = parts.map((p, i) => {
          const to = "/" + parts.slice(0, i + 1).join("/");
          return i === parts.length - 1 ? `<b>${esc(i ? p : "USB stick")}</b>` : `<button type="button" class="link" data-go="${esc(to)}">${esc(i ? p : "USB stick")}</button>`;
        }).join(" / ");
        const children = r.data?.children;
        const files = children ? children.filter((c) => c.type === "PRINT_FILE").length : 0;
        if (!r.online) status.textContent = "The printer is offline: this is what it said last.";
        else if (!children) status.textContent = r.asking ? "Asking the printer…" : "";
        else if (r.pending > 0) status.textContent = `Pictures and details: ${files - r.pending} of ${files}, the printer sends them one by one…`;
        else status.textContent = `${files} ${files === 1 ? "file" : "files"} to print.`;
        if (!children) { list.innerHTML = r.asking ? '<p class="empty">Waiting for the printer…</p>' : '<p class="empty">Nothing yet.</p>'; return; }
        const sorted = [...children].sort((a, b) => (a.type === "FOLDER") !== (b.type === "FOLDER") ? (a.type === "FOLDER" ? -1 : 1) : (a.display_name || a.name).localeCompare(b.display_name || b.name));
        list.innerHTML = sorted.length ? sorted.map((c) => {
          const full = `${path}/${c.name}`, name = c.display_name || c.name;
          if (c.type === "FOLDER") return `<div class="usb-item folder"><button type="button" class="usb-open" data-go="${esc(full)}"><span class="usb-icon">📁</span><b>${esc(name)}</b></button><button type="button" class="link danger-link" data-delete="${esc(full)}" data-folder="1" data-name="${esc(name)}">Delete</button></div>`;
          const printable = c.type === "PRINT_FILE";
          const d = c.details;
          const picture = d?.picture
            ? `<img src="../machines/${ID}/usb/preview?path=${encodeURIComponent(full)}&at=${d.at}" alt="" loading="lazy">`
            : `<span>${!printable ? esc(c.type.toLowerCase()) : d ? "no picture" : "…"}</span>`;
          const facts = d ? [d.time, d.material, d.grams && `${d.grams.split(",")[0].trim()} g`, d.layer_mm && `${d.layer_mm} mm`].filter(Boolean).join(" · ") : "";
          return `<div class="usb-item${printable ? "" : " other"}"><div class="usb-pic">${picture}</div><div class="usb-text"><b title="${esc(name)}">${esc(name)}</b><small>${facts ? esc(facts) : `${size(c.size)} · ${date(c.m_timestamp)}`}</small>${facts ? `<small>${size(c.size)} · ${date(c.m_timestamp)}</small>` : ""}</div><div class="usb-actions">${printable ? `<button type="button" data-print="${esc(full)}" data-name="${esc(name)}"${c.read_only ? " disabled title=\"Still arriving\"" : ""}>Print</button>` : ""}<button type="button" class="ghost" data-delete="${esc(full)}" data-name="${esc(name)}">Delete</button></div></div>`;
        }).join("") : '<p class="empty">Empty.</p>';
      }
      usb.addEventListener("click", async (event) => {
        const go = event.target.closest("[data-go]");
        if (go) { path = go.dataset.go; list.innerHTML = '<p class="empty">Waiting for the printer…</p>'; load(); return; }
        const print = event.target.closest("[data-print]");
        if (print) {
          if (!confirm(`Print ${print.dataset.name} on this printer now? Check the bed is clear.`)) return;
          const r = await post("print", { path: print.dataset.print });
          toast(r.ok ? `Asked to print ${print.dataset.name}` : `Not printed: ${await r.text()}`, r.ok ? "ok" : "bad");
          return;
        }
        const del = event.target.closest("[data-delete]");
        if (del) {
          if (!confirm(`Delete ${del.dataset.name}${del.dataset.folder ? " and what is in it" : ""} from the USB stick?`)) return;
          const fields = { path: del.dataset.delete };
          if (del.dataset.folder) fields.folder = "1";
          const r = await post("delete", fields);
          toast(r.ok ? `Deleting ${del.dataset.name}` : `Not deleted: ${await r.text()}`, r.ok ? "ok" : "bad");
          if (r.ok) setTimeout(load, 1500);
        }
      });
      usb.querySelector("[data-usb-folder]").addEventListener("submit", async (event) => {
        event.preventDefault();
        const name = event.target.elements.name.value.trim();
        const r = await post("folder", { path, name });
        toast(r.ok ? `Making ${name}` : `Not made: ${await r.text()}`, r.ok ? "ok" : "bad");
        if (r.ok) { event.target.reset(); setTimeout(load, 1500); }
      });
      usb.querySelector("[data-usb-send]").addEventListener("submit", async (event) => {
        event.preventDefault();
        if (!fileSelect.value) return;
        const r = await post("send", { file: fileSelect.value, folder: path });
        toast(r.ok ? `Sending to ${(await r.json()).path}: it shows here once on the stick` : `Not sent: ${await r.text()}`, r.ok ? "ok" : "bad");
        if (r.ok) setTimeout(load, 3000);
      });
      usbOpen = async () => {
        load();
        // The space's G-code, to send.
        try {
          const files = await (await fetch("../api/files")).json();
          const code = files.filter((f) => /\.(b?gcode|gco|g)$/i.test(f.name));
          fileSelect.innerHTML = code.length ? `<option value="">A file from Files…</option>${code.map((f) => `<option value="${esc(f.id)}">${esc(f.name)}</option>`).join("")}` : '<option value="">No G-code in Files yet</option>';
        } catch { /* stays as it is */ }
      };
    }
  }

  addEventListener("resize", () => { draw(); drawMesh(); });
  addEventListener("hashchange", show);
  // The browser scrolls to the tab's id once the page has loaded: back to the top.
  addEventListener("load", () => scrollTo(0, 0));
  fetch(`../api/machines/${ID}/history`).then((r) => r.json()).then((s) => { samples = s; draw(); });

  // Live: the machine's values again on every change (server-sent events).
  const fixed = (n, digits = 1) => (n == null ? null : n.toFixed(digits));
  const stream = new EventSource("../api/machines/stream");
  stream.onopen = () => live(true);
  stream.onerror = () => live(false);
  // The queue (crate::queue): from the live data, unless someone is dragging in it.
  const queueList = document.querySelector("[data-queue]");
  let dragged = null, shownQueue = "";
  // Staff move and take out any print; anyone else only their own (the server checks too).
  const ME = Number(document.querySelector("main").dataset.me);
  const STAFF = document.querySelector("main").dataset.staff === "true";
  const mayChange = (it) => STAFF || it.by_user === ME;
  const minutes = (s) => (s == null ? "" : s < 3600 ? `${Math.max(1, Math.round(s / 60))} min` : `${Math.floor(s / 3600)} h ${String(Math.round((s % 3600) / 60)).padStart(2, "0")} min`);
  function showQueue(m) {
    if (!queueList || dragged) return;
    const q = m.queue || [];
    const key = JSON.stringify([q, m.queue_held]);
    if (key === shownQueue) return;
    shownQueue = key;
    const count = document.querySelector("[data-queue-count]");
    if (count) count.textContent = q.length ? q.length : "";
    document.querySelector("[data-queue-empty]").hidden = q.length > 0;
    // After a failed or stopped job, it waits for someone to say the bed is clear.
    const held = document.querySelector("[data-queue-held]");
    held.hidden = !m.queue_held;
    if (m.queue_held) held.querySelector("[data-held-why]").textContent = m.queue_held;
    // Roughly when each starts: after what runs now, and what waits before it.
    const t = (m.online && m.telemetry) || {};
    let at = Date.now() + ((t.state === "busy" && t.job?.remaining_s) || 0) * 1000;
    queueList.innerHTML = q.map((it, i) => {
      const starts = new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
      at += (it.estimated_s || 0) * 1000;
      const facts = [minutes(it.estimated_s), it.by, i === 0 && t.state !== "busy" ? "next" : `starts around ${starts}`].filter(Boolean).join(" · ");
      const mine = it.by_user === ME, change = mayChange(it);
      return `<li${change ? ' draggable="true"' : ""} data-qid="${it.id}" data-mine="${mine}" class="${mine ? "mine" : ""}${change ? "" : " fixed"}"><span class="q-grip" aria-hidden="true">${change ? "⠿" : ""}</span><span class="q-n">${i + 1}</span>${it.thumbnail ? `<img src="../files/${encodeURIComponent(it.file_id)}/thumbnail.png" alt="">` : '<span class="q-img"></span>'}<span class="q-name"><b title="${escapeHtml(it.file_name)}">${escapeHtml(it.file_name)}</b><small>${mine ? '<span class="q-mine">yours</span> · ' : ""}${escapeHtml(facts)}${it.problem ? ` · <span class="q-problem">${escapeHtml(it.problem)}</span>` : ""}</small></span>${change ? `<button type="button" class="q-x" data-qremove="${it.id}" title="Take out of the queue" aria-label="Take out of the queue">×</button>` : "<span></span>"}</li>`;
    }).join("");
  }
  if (queueList) {
    queueList.addEventListener("dragstart", (e) => {
      dragged = e.target.closest("li");
      if (!dragged) return;
      dragged.classList.add("dragging");
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", dragged.dataset.qid);
    });
    queueList.addEventListener("dragover", (e) => {
      if (!dragged) return;
      const over = e.target.closest("li");
      // Not staff: only among one's own prints, swapping places, so the others stay put.
      if (!STAFF && over && over.dataset.mine !== "true") return;
      e.preventDefault();
      if (!over || over === dragged) return;
      if (STAFF) {
        const r = over.getBoundingClientRect();
        over.parentNode.insertBefore(dragged, e.clientY > r.top + r.height / 2 ? over.nextSibling : over);
      } else {
        const spot = document.createElement("li");
        dragged.replaceWith(spot);
        over.replaceWith(dragged);
        spot.replaceWith(over);
      }
    });
    queueList.addEventListener("dragend", async () => {
      if (!dragged) return;
      dragged.classList.remove("dragging");
      dragged = null;
      const ids = [...queueList.querySelectorAll("li")].map((li) => li.dataset.qid).join(",");
      [...queueList.querySelectorAll(".q-n")].forEach((n, i) => (n.textContent = i + 1));
      const r = await fetch(`${ID}/queue/order`, { method: "POST", body: new URLSearchParams({ ids }), headers: { "x-requested-with": "fetch" } });
      if (!r.ok) toast(`Not moved: ${await r.text()}`, "bad");
      shownQueue = "";
    });
    document.querySelector("[data-bed-clear]")?.addEventListener("click", async () => {
      const r = await fetch(`${ID}/taken`, { method: "POST", headers: { "x-requested-with": "fetch" } });
      toast(r.ok ? "Bed clear: the queue goes on" : `Not done: ${await r.text()}`, r.ok ? "ok" : "bad");
      shownQueue = "";
    });
    queueList.addEventListener("click", async (e) => {
      const x = e.target.closest("[data-qremove]");
      if (!x) return;
      const r = await fetch(`${ID}/queue/${x.dataset.qremove}/remove`, { method: "POST", headers: { "x-requested-with": "fetch" } });
      if (r.ok) { x.closest("li").remove(); toast("Taken out of the queue"); } else toast(`Not done: ${await r.text()}`, "bad");
    });
  }

  stream.onmessage = (event) => {
    const machines = JSON.parse(event.data);
    summarise(machines);
    const m = machines.find((m) => m.id === ID);
    if (!m) return;
    showQueue(m);
    const t = (m.online && m.telemetry) || {};
    const p = t.position_mm;
    const values = {
      nozzle_n: fixed(t.nozzle_c),
      nozzle_note: heatNote(t.nozzle_c, t.nozzle_target_c),
      bed_n: fixed(t.bed_c),
      bed_note: heatNote(t.bed_c, t.bed_target_c),
      print_fan_n: t.print_fan_rpm != null ? String(t.print_fan_rpm) : null,
      hotend_fan_n: t.hotend_fan_rpm != null ? String(t.hotend_fan_rpm) : null,
      pos_x: p && fixed(p[0]),
      pos_y: p && fixed(p[1]),
      pos_z: p && fixed(p[2]),
      position: p && p.map((n) => n.toFixed(1)).join(" · ") + " mm",
      speed: t.speed_percent != null ? `${t.speed_percent} %` : null,
      flow: t.flow_percent != null ? `${t.flow_percent} %` : null,
      material: t.material,
      seen_n: m.seen_seconds_ago != null ? String(m.seen_seconds_ago) : null,
    };
    for (const el of document.querySelectorAll("[data-live]")) {
      const key = el.dataset.live;
      if (!(key in values)) continue;
      el.textContent = values[key] ?? "–";
      if (key === "state") el.className = `state ${values[key]}`;
    }
    // Out of use, a print on the bed: drawn again when either changes.
    const notices = document.querySelector("[data-notices]");
    const noticeKey = JSON.stringify([m.out_of_use, m.confirm, m.on_bed, m.on_bed && t.bed_c != null && t.bed_c > TAKE_BELOW_C ? Math.round(t.bed_c) : null]);
    if (notices && notices.dataset.key !== noticeKey) {
      const first = notices.dataset.key == null;
      notices.dataset.key = noticeKey;
      if (!first) notices.innerHTML = noticesHtml(m, t);
    }
    // The heaters' flames: lit while they have power, outlined while not, gone untold.
    for (const el of document.querySelectorAll("[data-heater]")) {
      const on = t[`${el.dataset.heater}_heating`];
      el.dataset.on = on == null ? "" : String(on);
      const said = on == null ? "" : on ? "Heater on" : "Heater off";
      el.title = said;
      el.setAttribute("aria-label", said);
    }
    showJob(m, t);
    const gauge = document.querySelector("[data-z-fill]");
    if (gauge && p) {
      const max = +document.querySelector("[data-z-max]").dataset.zMax;
      gauge.style.height = `${Math.min(100, Math.max(0, (p[2] / max) * 100))}%`;
    }
    renderTerminal(m.commands);
    for (const list of document.querySelectorAll("[data-commands]")) {
      list.innerHTML = m.commands.length ? m.commands.map(commandItem).join("") : '<li class="empty">None yet</li>';
    }

    // The head on the bed, where the machine says it is.
    const bed = document.querySelector("svg.bed"), head = bed?.querySelector(".head");
    if (head) {
      head.setAttribute("visibility", p ? "visible" : "hidden");
      if (p) head.setAttribute("transform", `translate(${p[0]} ${bed.dataset.depth - p[1]})`);
    }

    if (m.online && t.nozzle_c != null) {
      const now = Date.now() / 1000;
      if (!samples.length || now - samples[samples.length - 1].t >= 2) {
        samples.push({ t: now, nozzle: t.nozzle_c, nozzle_target: t.nozzle_target_c, bed: t.bed_c, bed_target: t.bed_target_c });
      }
      samples = samples.filter((s) => now - s.t <= SPAN);
    }
    if (m.mesh && (!mesh || m.mesh.taken_at !== mesh.taken_at)) {
      mesh = m.mesh;
      drawMesh();
    }
    if (mesh) setLive("mesh_age", ago(Date.now() / 1000 - mesh.taken_at));
    if (!document.getElementById("telemetry").hidden) {
      draw();
      fillTable();
    }
  };

  // The bed mesh: front row first, each row from left to right, millimetres (null: not
  // measured). Drawn as a surface (drag to turn), a map, or a table.
  let mesh = JSON.parse(document.getElementById("mesh-data")?.textContent || "null");
  const canvas = document.getElementById("mesh3d");
  let view = "surface", azimuth = -0.55, elevation = 0.62;
  function setLive(key, value) {
    for (const el of document.querySelectorAll(`[data-live="${key}"]`)) el.textContent = value ?? "–";
  }
  function ago(s) {
    s = Math.max(0, Math.round(s));
    if (s < 60) return `${s} s ago`;
    if (s < 3600) return `${Math.floor(s / 60)} min ago`;
    if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
    return `${Math.floor(s / 86400)} days ago`;
  }
  const signed = (v) => (v == null ? "–" : `${v >= 0 ? "+" : "−"}${Math.abs(v).toFixed(3)}`);
  // A diverging scale: below the bed blue, on it neutral, above it orange.
  function scale() {
    const css = getComputedStyle(document.documentElement);
    const hex = (name) => css.getPropertyValue(name).trim().replace("#", "").match(/../g).map((h) => parseInt(h, 16));
    const low = hex("--low"), mid = hex("--mid"), high = hex("--high");
    return (t) => {
      const [a, b, f] = t < 0 ? [mid, low, -t] : [mid, high, t];
      const c = a.map((v, i) => Math.round(v + (b[i] - v) * Math.min(1, f)));
      return `rgb(${c[0]} ${c[1]} ${c[2]})`;
    };
  }
  function stats(rows) {
    const all = rows.flat().filter((v) => v != null);
    const low = Math.min(...all), high = Math.max(...all);
    return { low, high, reach: Math.max(Math.abs(low), Math.abs(high), 0.02) };
  }
  // Points not measured take the mean of their measured neighbours (for drawing only).
  function filled(rows) {
    return rows.map((row, j) => row.map((v, i) => {
      if (v != null) return v;
      const near = [[j - 1, i], [j + 1, i], [j, i - 1], [j, i + 1]].map(([a, b]) => rows[a]?.[b]).filter((n) => n != null);
      return near.length ? near.reduce((a, b) => a + b) / near.length : 0;
    }));
  }
  function drawMesh() {
    if (!canvas) return;
    const empty = !mesh || !mesh.rows.length;
    document.querySelector("[data-mesh-empty]").hidden = !empty;
    canvas.hidden = empty || view !== "surface";
    document.querySelector("[data-mesh-map]").hidden = empty || view !== "map";
    document.querySelector("[data-mesh-table]").hidden = empty || view !== "table";
    if (empty) return;
    const { low, high, reach } = stats(mesh.rows);
    setLive("mesh_range", (high - low).toFixed(3));
    setLive("mesh_low", signed(low));
    setLive("mesh_high", signed(high));
    setLive("mesh_points", `${mesh.rows[0].length} × ${mesh.rows.length} points`);
    document.querySelector("[data-scale-low]").textContent = `${signed(-reach)} mm`;
    document.querySelector("[data-scale-high]").textContent = `${signed(reach)} mm`;
    const color = scale();
    if (view === "surface") surface(color, reach);
    if (view === "map") map(color, reach);
    if (view === "table") meshTable();
  }
  function surface(color, reach) {
    const w = canvas.clientWidth, h = canvas.clientHeight;
    if (!w) return;
    const ratio = devicePixelRatio || 1;
    canvas.width = w * ratio;
    canvas.height = h * ratio;
    const g = canvas.getContext("2d");
    g.setTransform(ratio, 0, 0, ratio, 0, 0);
    g.clearRect(0, 0, w, h);
    const rows = filled(mesh.rows), ny = rows.length, nx = rows[0].length;
    // The bed in its proportions, the longer side 1; heights exaggerated to be seen.
    const bw = +canvas.dataset.bedWidth || 1, bd = +canvas.dataset.bedDepth || 1;
    const sx = bw / Math.max(bw, bd), sy = bd / Math.max(bw, bd);
    const lift = 0.22 / reach;
    document.querySelector("[data-exaggeration]").textContent = `heights × ${Math.round(lift * Math.max(bw, bd))}`;
    const f = 5, gx = (nx - 1) * f + 1, gy = (ny - 1) * f + 1;
    const at = (u, v) => {
      // Bilinear between the measured points.
      const i = Math.min(nx - 2, Math.floor(u)), j = Math.min(ny - 2, Math.floor(v));
      const a = u - i, b = v - j;
      return rows[j][i] * (1 - a) * (1 - b) + rows[j][i + 1] * a * (1 - b) + rows[j + 1][i] * (1 - a) * b + rows[j + 1][i + 1] * a * b;
    };
    const ca = Math.cos(azimuth), sa = Math.sin(azimuth), ce = Math.cos(elevation), se = Math.sin(elevation);
    const turn = (x, y, z) => {
      const X = x * ca - y * sa, Y = x * sa + y * ca;
      return { X, Y, z, depth: Y * ce - z * se };
    };
    const point = (p, q) => {
      const x = (p / (gx - 1) - 0.5) * sx, y = (q / (gy - 1) - 0.5) * sy;
      const z = at((p / (gx - 1)) * (nx - 1), (q / (gy - 1)) * (ny - 1));
      return { ...turn(x, y, z * lift), value: z };
    };
    const grid = [];
    for (let q = 0; q < gy; q++) { grid.push([]); for (let p = 0; p < gx; p++) grid[q].push(point(p, q)); }
    // Fit the turned bed in the canvas.
    const corners = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]].map(([x, y]) => turn(x * sx, y * sy, 0));
    const spanX = Math.max(...corners.map((c) => c.X)) - Math.min(...corners.map((c) => c.X));
    const spanY = (Math.max(...corners.map((c) => c.Y)) - Math.min(...corners.map((c) => c.Y))) * se + 0.5 * ce;
    const size = Math.min((w * 0.9) / spanX, (h * 0.95) / spanY);
    const screen = (c) => [w / 2 + c.X * size, h * 0.47 - (c.Y * se + c.z * ce) * size];
    // The bed plate under the surface.
    const plate = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]].map(([x, y]) => screen(turn(x * sx, y * sy, -reach * lift * 1.15)));
    const css = getComputedStyle(document.documentElement);
    g.beginPath();
    plate.forEach(([x, y], k) => (k ? g.lineTo(x, y) : g.moveTo(x, y)));
    g.closePath();
    g.fillStyle = css.getPropertyValue("--line");
    g.fill();
    // Quads from the back to the front.
    const quads = [];
    for (let q = 0; q < gy - 1; q++) for (let p = 0; p < gx - 1; p++) {
      const c = [grid[q][p], grid[q][p + 1], grid[q + 1][p + 1], grid[q + 1][p]];
      quads.push({ c, depth: c.reduce((s, v) => s + v.depth, 0) / 4, value: c.reduce((s, v) => s + v.value, 0) / 4 });
    }
    quads.sort((a, b) => b.depth - a.depth);
    for (const quad of quads) {
      g.beginPath();
      quad.c.forEach((v, k) => { const [x, y] = screen(v); k ? g.lineTo(x, y) : g.moveTo(x, y); });
      g.closePath();
      g.fillStyle = g.strokeStyle = color(quad.value / reach);
      g.lineWidth = 0.6;
      g.fill();
      g.stroke();
    }
    // The measured points, and which side is the front.
    g.fillStyle = css.getPropertyValue("--ink");
    for (let j = 0; j < ny; j++) for (let i = 0; i < nx; i++) {
      const [x, y] = screen(grid[j * f][i * f]);
      g.beginPath();
      g.arc(x, y, 2, 0, Math.PI * 2);
      g.fill();
    }
    g.fillStyle = css.getPropertyValue("--muted");
    g.font = "13px system-ui, sans-serif";
    g.textAlign = "center";
    const [fx, fy] = screen(turn(0, -0.5 * sy - 0.08, -reach * lift * 1.15));
    g.fillText("front", fx, fy + 6);
  }
  function map(color, reach) {
    const rows = mesh.rows, ny = rows.length, nx = rows[0].length, cell = 64;
    let svg = `<svg viewBox="0 0 ${nx * cell} ${ny * cell + 22}" role="img" aria-label="The bed mesh as a map, the front at the bottom">`;
    rows.forEach((row, j) => row.forEach((v, i) => {
      const y = (ny - 1 - j) * cell, t = v == null ? 0 : v / reach;
      svg += `<rect x="${i * cell + 1}" y="${y + 1}" width="${cell - 2}" height="${cell - 2}" rx="4" fill="${v == null ? "none" : color(t)}" stroke="var(--line)"/>`;
      svg += `<text x="${i * cell + cell / 2}" y="${y + cell / 2 + 4}" text-anchor="middle" fill="${Math.abs(t) > 0.6 ? "#fff" : "var(--ink)"}">${v == null ? "·" : signed(v)}</text>`;
    }));
    svg += `<text x="${(nx * cell) / 2}" y="${ny * cell + 16}" text-anchor="middle" fill="var(--muted)">front</text></svg>`;
    document.querySelector("[data-mesh-map]").innerHTML = svg;
  }
  function meshTable() {
    const rows = mesh.rows, nx = rows[0].length;
    const head = Array.from({ length: nx }, (_, i) => `<th>${i + 1}</th>`).join("");
    const body = [...rows].map((row, j) => [row, j]).reverse()
      .map(([row, j]) => `<tr><td>${j === 0 ? "1 (front)" : j === rows.length - 1 ? `${j + 1} (back)` : j + 1}</td>${row.map((v) => `<td>${signed(v)}</td>`).join("")}</tr>`).join("");
    document.querySelector("[data-mesh-table]").innerHTML = `<table><thead><tr><th>Row</th>${head}</tr></thead><tbody>${body}</tbody></table>`;
  }
  for (const button of document.querySelectorAll("[data-mesh-view]")) button.addEventListener("click", () => {
    view = button.dataset.meshView;
    for (const other of document.querySelectorAll("[data-mesh-view]")) other.setAttribute("aria-pressed", String(other === button));
    drawMesh();
  });
  if (canvas) {
    let drag = null;
    canvas.addEventListener("pointerdown", (e) => { drag = [e.clientX, e.clientY]; canvas.setPointerCapture(e.pointerId); });
    canvas.addEventListener("pointermove", (e) => {
      if (!drag) return;
      azimuth += (e.clientX - drag[0]) * 0.01;
      elevation = Math.min(1.45, Math.max(0.15, elevation + (e.clientY - drag[1]) * 0.01));
      drag = [e.clientX, e.clientY];
      drawMesh();
    });
    canvas.addEventListener("pointerup", () => { drag = null; });
  }

  // The terminal: each command as it was sent, what the machine answered, and how it went.
  const terminal = document.querySelector("[data-terminal]");
  let shown = "";
  function renderTerminal(commands) {
    if (!terminal) return;
    const key = JSON.stringify(commands);
    if (key === shown) return;
    shown = key;
    const atBottom = terminal.scrollHeight - terminal.scrollTop - terminal.clientHeight < 40;
    const lines = [];
    for (const c of [...commands].reverse()) {
      // A virtual machine echoes what it runs ("> G28"); otherwise show what was sent.
      if (!c.output.some((l) => l.startsWith("> "))) {
        for (const l of c.gcode.split("\n")) lines.push(`<span class="sent">&gt; ${esc(l)}</span>`);
      }
      for (const l of c.output) {
        lines.push(l.startsWith("> ") ? `<span class="sent">&gt; ${esc(l.slice(2))}</span>` : esc(l));
      }
      lines.push(`<span class="st st-${c.state}">· ${c.state}${c.reason ? `: ${esc(c.reason)}` : ""}</span>`);
    }
    terminal.innerHTML = lines.join("\n") || '<span class="st">Nothing sent yet. Type G-code below.</span>';
    if (atBottom || lines.length < 30) terminal.scrollTop = terminal.scrollHeight;
  }
  const gcodeForm = document.querySelector("[data-gcode]");
  if (gcodeForm) {
    const input = gcodeForm.querySelector("input");
    const note = document.querySelector("[data-gcode-status]");
    const url = new URL(gcodeForm.getAttribute("action"), location.href);
    const sent = [];
    let back = 0;
    async function send(text) {
      const response = await fetch(url, { method: "POST", body: new URLSearchParams({ gcode: text }), headers: { "x-requested-with": "fetch" } })
        .catch((error) => ({ ok: false, text: async () => error.message }));
      note.textContent = response.ok ? "" : `Not sent: ${await response.text()}`;
      return response.ok;
    }
    gcodeForm.addEventListener("submit", async (event) => {
      event.preventDefault();
      const text = input.value.trim();
      if (!text) return;
      if (await send(text)) {
        sent.push(text);
        back = 0;
        input.value = "";
        terminal.scrollTop = terminal.scrollHeight;
      }
    });
    // Up and down walk through what was sent, as in a shell.
    input.addEventListener("keydown", (event) => {
      if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
      event.preventDefault();
      back = Math.min(sent.length, Math.max(0, back + (event.key === "ArrowUp" ? 1 : -1)));
      input.value = back ? sent[sent.length - back] : "";
    });
    for (const button of document.querySelectorAll("[data-gcode-quick]")) {
      button.addEventListener("click", () => send(button.dataset.gcodeQuick));
    }
  }

  // The job: what is printing, how far, and pause, resume, stop.
  function duration(s) {
    if (s == null) return "–";
    if (s < 60) return `${s} s`;
    if (s < 3600) return `${Math.floor(s / 60)} min`;
    return `${Math.floor(s / 3600)} h ${String(Math.floor((s % 3600) / 60)).padStart(2, "0")} min`;
  }
  function showJob(m, t) {
    const job = m.job, progress = t.job;
    const percent = progress && progress.total ? (progress.done / progress.total) * 100 : null;
    const left = progress && progress.remaining_s != null ? ` · ${timeLeft(progress.remaining_s)}` : "";
    const values = {
      job_pct: percent == null ? null : percent.toFixed(0),
      job_note: job ? `${job.state} · ${job.file_name}${left}` : "no job",
      job_name: job ? job.file_name : "",
      job_done: percent == null ? null : `${percent.toFixed(0)} %`,
      job_elapsed: progress ? duration(progress.elapsed_s) : null,
      job_remaining: progress ? duration(progress.remaining_s) : null,
      job_ends: progress ? timeLeft(progress.remaining_s) : null,
    };
    for (const [key, value] of Object.entries(values)) setLive(key, value);
    showState(m);
    const bar = document.querySelector("[data-job-bar]");
    if (bar) bar.style.width = `${percent ?? 0}%`;
    for (const el of document.querySelectorAll("[data-job], [data-job-actions]")) el.hidden = !job;
    const none = document.querySelector("[data-no-job]");
    if (none) none.hidden = !!job;
    // An interrupted job that can start again where it stopped.
    const resume = document.querySelector("[data-resume]"), last = !job && m.resumable;
    if (resume) {
      resume.hidden = !last;
      if (last) {
        setLive("resume_name", last.file_name);
        setLive("resume_line", String(last.restart_line));
      }
    }
    const thumb = document.querySelector("[data-job-thumb]");
    if (job && thumb && !thumb.src.includes(job.file_id)) { thumb.hidden = false; thumb.src = `../files/${job.file_id}/thumbnail.png`; }
    for (const button of document.querySelectorAll("[data-job-actions] button")) {
      const can = { pause: job?.state === "printing", resume: job?.state === "paused", stop: !!job && job.state !== "stopping" };
      button.hidden = !can[button.value];
    }
  }
  for (const form of document.querySelectorAll("[data-restart]")) {
    const url = new URL(form.getAttribute("action"), location.href);
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      if (!confirm("Start the job again from that line? Home first if the machine lost its position.")) return;
      await fetch(url, { method: "POST", body: new URLSearchParams({ action: "restart" }) });
    });
  }
  for (const form of document.querySelectorAll("[data-job-actions]")) {
    const url = new URL(form.getAttribute("action"), location.href);
    for (const button of form.querySelectorAll("button")) button.addEventListener("click", async (event) => {
      event.preventDefault();
      if (button.value === "stop" && !confirm("Stop this job?")) return;
      await fetch(url, { method: "POST", body: new URLSearchParams({ action: button.value }) });
    });
  }
  show();

// Times the page gives as Unix seconds, shown in this browser's own time.
for (const t of document.querySelectorAll("[data-local-time]")) {
  t.textContent = new Date(Number(t.dataset.localTime) * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

