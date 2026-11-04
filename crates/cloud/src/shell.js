  // The app's shell, on every page: the sidebar (pages, machines live, counts), the dialog
  // to add a machine, toasts, forms saved in the background, and the ⌘K palette.
  const ROOT = document.querySelector(".sidebar").dataset.root;
  const escapeHtml = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

  // The dialog to add a machine, as a wizard: how it connects, then its steps. Opened from
  // any [data-add-open], and at #add.
  {
    const dialog = document.getElementById("add-dialog");
    const back = dialog.querySelector("[data-wizard-back]");
    let waiting = null;
    function step(name) {
      for (const s of dialog.querySelectorAll("[data-step]")) s.hidden = s.dataset.step !== name;
      back.hidden = name === "start";
      if (waiting && name !== "klipper-conf") { clearInterval(waiting); waiting = null; }
      dialog.querySelector(`[data-step="${name}"] input`)?.focus();
    }
    const open = (at = "start") => { step(at); if (!dialog.open) dialog.showModal(); };
    document.addEventListener("click", (event) => {
      if (event.target.closest("[data-add-open]")) { event.preventDefault(); open(); }
      if (event.target.closest("[data-add-close]") || event.target === dialog) dialog.close();
    });
    dialog.addEventListener("click", (event) => {
      const go = event.target.closest("[data-go]");
      if (go) step(go.dataset.go);
    });
    back.addEventListener("click", () => step("start"));
    dialog.addEventListener("close", () => { if (waiting) { clearInterval(waiting); waiting = null; } });
    // This cloud's address, as the machine must reach it.
    const local = ["localhost", "127.0.0.1", "[::1]"].includes(location.hostname);
    const port = location.port || (location.protocol === "https:" ? "443" : "80");
    dialog.querySelector("[data-prusa-ini]").textContent = `[service::connect]\nhostname = ${local ? "<this computer's address>" : location.hostname}\nport = ${port}\ntls = false`;
    // A code: added, then the list shows it.
    for (const form of dialog.querySelectorAll("[data-pair-form]")) form.addEventListener("submit", async (event) => {
      event.preventDefault();
      const note = form.parentElement.querySelector("[data-pair-note]");
      const response = await fetch(form.getAttribute("action"), { method: "POST", body: new URLSearchParams(new FormData(form)), headers: { "x-requested-with": "fetch" } });
      if (response.ok) { dialog.close(); toast("Machine added"); location.href = ROOT || "./"; }
      else note.textContent = response.status === 404 ? "No machine waits with that code." : `Not added: ${await response.text()}`;
    });
    // A printer on Klipper: made, its lines shown, then it is waited for.
    dialog.querySelector("[data-klipper-form]").addEventListener("submit", async (event) => {
      event.preventDefault();
      const form = event.target;
      const response = await fetch(form.getAttribute("action"), { method: "POST", body: new URLSearchParams(new FormData(form)), headers: { "x-requested-with": "fetch" } });
      if (!response.ok) { toast(`Not added: ${await response.text()}`, "bad"); return; }
      const { id, token } = await response.json();
      dialog.querySelector("[data-klipper-conf]").textContent = `[simplyprint]\nurl: ws://${location.host}/simplyprint/${token}`;
      dialog.querySelector("[data-local-warning]").hidden = !local;
      dialog.querySelector("[data-klipper-page]").href = `${ROOT}machines/${encodeURIComponent(id)}`;
      const wait = dialog.querySelector("[data-klipper-wait]");
      wait.classList.remove("done");
      wait.lastChild.textContent = "Waiting for it to connect…";
      step("klipper-conf");
      waiting = setInterval(async () => {
        try {
          const machines = await (await fetch(`${ROOT}api/machines`)).json();
          if (machines.find((m) => m.id === id)?.online) {
            clearInterval(waiting); waiting = null;
            wait.classList.add("done");
            wait.lastChild.textContent = "Connected.";
          }
        } catch (error) { /* tried again */ }
      }, 2000);
      form.reset();
    });
    if (location.search.includes("unknown-code")) {
      open("code");
      dialog.querySelector("[data-add-note]").textContent = "No machine waits with that code.";
    }
    if (location.hash === "#add") open();
  }

  // How full a DYMO's two-cell Li-ion pack is, from its voltage (as dymo::battery_percent).
  function batteryPercent(packVolts) {
    const curve = [[3.4, 0], [3.5, 5], [3.6, 15], [3.7, 30], [3.8, 50], [4.0, 80], [4.2, 100]];
    const cell = packVolts / 2;
    if (cell <= curve[0][0]) return 0;
    for (let i = 1; i < curve.length; i++) {
      const [v0, p0] = curve[i - 1], [v1, p1] = curve[i];
      if (cell <= v1) return Math.round(p0 + ((p1 - p0) * (cell - v0)) / (v1 - v0));
    }
    return 100;
  }
  // How a machine's state is shown, the same on every page. A machine that waits for
  // someone (a label printer without a cassette) shows the reason, amber: not an error.
  // A print on the bed can be taken at or below this (°C); as the cloud's TAKE_BELOW_C.
  const TAKE_BELOW_C = 30;
  // What is loaded in a machine: "S-0007 · Galaxy Black · 412 g left".
  function loadedText(l) {
    const amount = l.unit === "g" ? Math.round(l.amount) : l.unit === "m" ? l.amount.toFixed(1) : l.amount;
    const from = l.place ? ` · from ${l.place}${l.slot ? `, slot ${l.slot}` : ""}` : "";
    return `${l.item} · ${l.name}${l.material ? ` · ${l.material}` : ""} · ${amount} ${l.unit} left${from}`;
  }
  function shownState(m) {
    if (m.out_of_use) return { text: "Out of use", cls: "out" };
    const t = m.online ? m.telemetry : null;
    if (!t) return { text: "Offline", cls: "offline" };
    // A finished print on the bed: cooling, then it waits for someone to take it.
    if (m.on_bed && t.state === "idle") {
      return t.bed_c != null && t.bed_c > TAKE_BELOW_C ? { text: "Cooling", cls: "cooling" } : { text: "Take the print", cls: "attention" };
    }
    if (t.state === "attention" || (t.state === "idle" && t.problem)) return { text: t.problem || "Attention", cls: "attention" };
    const names = { disconnected: "Disconnected", idle: "Idle", busy: "Busy", paused: "Paused", error: "Error" };
    return { text: names[t.state] || t.state, cls: t.state };
  }
  // Puts a machine's state in every [data-live="state"], and its colour on the badges; a
  // badge that changes colour pulses once.
  function showState(m) {
    const shown = shownState(m);
    for (const el of document.querySelectorAll('[data-live="state"]')) {
      el.textContent = shown.text;
      if (el.classList.contains("state")) {
        const next = `state ${shown.cls}`;
        if (el.className.replace(" pulse", "") !== next) {
          el.className = next;
          void el.offsetWidth;
          el.classList.add("pulse");
        }
      }
    }
  }

  // The machines in the sidebar, live: each with its state as a dot, and what it does.
  let knownMachines = [];
  let fed = false;
  function summarise(machines) {
    fed = true;
    knownMachines = machines;
    const count = {
      machines: machines.length,
      online: machines.filter((m) => m.online).length,
      busy: machines.filter((m) => m.online && m.telemetry && m.telemetry.state === "busy").length,
      attention: machines.filter((m) => ["attention", "error"].includes(shownState(m).cls)).length,
    };
    for (const el of document.querySelectorAll("[data-sum]")) el.textContent = count[el.dataset.sum];
    const attention = document.querySelector("[data-attention-count]");
    if (attention) attention.hidden = !count.attention;
    const here = location.pathname.split("/").pop();
    const order = ["fdm", "laser", "cnc", "cutter", "label", "paper", "other"];
    const sorted = [...machines].sort((a, b) => order.indexOf(a.kind) - order.indexOf(b.kind) || a.name.localeCompare(b.name));
    const side = document.querySelector("[data-side-machines]");
    side.innerHTML = sorted.length ? sorted.map((m) => {
      const s = shownState(m);
      const t = (m.online && m.telemetry) || {};
      const pct = t.job && t.job.total ? Math.round((t.job.done * 100) / t.job.total) : null;
      const sub = pct != null ? `${pct} %` : s.cls === "idle" ? "" : s.text;
      return `<a href="${ROOT}machines/${encodeURIComponent(m.id)}" data-kind="${escapeHtml(m.kind)}"${m.id === here ? ' aria-current="page"' : ""} title="${escapeHtml(`${m.name}: ${s.text}`)}">
        <i class="dot ${s.cls}"></i><span class="side-name">${escapeHtml(m.name)}</span>${sub ? `<small class="${s.cls}">${escapeHtml(sub)}</small>` : ""}
        ${pct != null ? `<span class="side-progress"><i style="width:${pct}%"></i></span>` : ""}
      </a>`;
    }).join("") : `<p class="side-empty">None yet</p>`;
  }
  function live(on) {
    const dot = document.querySelector("[data-live-dot]");
    dot.classList.toggle("on", on);
    dot.title = on ? "Live updates" : "Reconnecting…";
  }
  // A page without a live list of its own (downloads) still keeps the sidebar current.
  setTimeout(() => {
    if (fed) return;
    const poll = () => fetch(`${ROOT}api/machines`).then((r) => r.json()).then((m) => { summarise(m); live(true); }).catch(() => live(false));
    poll();
    setInterval(poll, 10000);
  }, 1500);

  // A short message in the corner, instead of a page that reloads.
  function toast(text, kind = "ok") {
    const box = document.querySelector("[data-toasts]");
    const el = document.createElement("div");
    el.className = `toast ${kind}`;
    el.textContent = text;
    box.append(el);
    setTimeout(() => el.classList.add("gone"), 2600);
    setTimeout(() => el.remove(), 3000);
  }

  // Forms marked data-async are saved in the background: a toast says how it went; the page
  // is reloaded (smoothly) only where it says data-then="reload", or goes where data-then
  // names. A page can listen for "saved" on the form to update itself.
  document.addEventListener("submit", async (event) => {
    const form = event.target;
    if (!form.matches("form[data-async]")) return;
    event.preventDefault();
    if (form.dataset.confirm && !confirm(form.dataset.confirm)) return;
    const body = new URLSearchParams(new FormData(form, event.submitter));
    try {
      const response = await fetch(form.getAttribute("action"), { method: "POST", body, headers: { "x-requested-with": "fetch" } });
      if (!response.ok) throw new Error(await response.text());
      const text = response.headers.get("content-type")?.includes("json") ? await response.json() : null;
      toast(form.dataset.done || "Saved");
      form.dispatchEvent(new CustomEvent("saved", { bubbles: true, detail: { data: text, submitter: event.submitter } }));
      const then = form.dataset.then;
      if (then === "reload") setTimeout(() => location.reload(), 300);
      else if (then) setTimeout(() => (location.href = then), 300);
    } catch (error) {
      toast(`Not saved: ${error.message}`, "bad");
    }
  });

  // A machine renamed: its title, here and in the tab, at once.
  document.addEventListener("saved", (event) => {
    if (!event.target.matches("[data-rename]")) return;
    const name = event.target.querySelector("input[name=name]").value.trim();
    const title = document.querySelector(".title h2");
    if (title) title.textContent = name;
    document.title = `${name} · Maker Connect`;
  });

  // The palette: ⌘K (or Ctrl-K) to go to a machine, a file or a page by typing.
  {
    const palette = document.querySelector("[data-palette]");
    const input = document.querySelector("[data-palette-input]");
    const results = document.querySelector("[data-palette-results]");
    let files = null, items = [], chosen = 0;
    const pages = [
      { label: "Machines", hint: "page", href: ROOT },
      { label: "Files", hint: "page", href: `${ROOT}files` },
      { label: "Materials", hint: "page", href: `${ROOT}stock` },
      { label: "Downloads", hint: "page", href: `${ROOT}downloads` },
      { label: "Add a machine", hint: "action", run: () => document.querySelector("[data-add-open]").click() },
    ];
    function fill() {
      const words = input.value.trim().toLowerCase();
      const all = [
        ...knownMachines.map((m) => ({ label: m.name, hint: shownState(m).text, href: `${ROOT}machines/${encodeURIComponent(m.id)}`, dot: shownState(m).cls })),
        ...pages,
        ...(files || []).map((f) => ({ label: f.name, hint: "file", href: `${ROOT}files#file=${encodeURIComponent(f.id)}` })),
      ];
      items = all.filter((i) => !words || i.label.toLowerCase().includes(words)).slice(0, 12);
      chosen = Math.min(chosen, Math.max(items.length - 1, 0));
      results.innerHTML = items.length
        ? items.map((i, n) => `<li role="option" data-n="${n}" aria-selected="${n === chosen}">${i.dot ? `<i class="dot ${i.dot}"></i>` : ""}<span>${escapeHtml(i.label)}</span><small>${escapeHtml(i.hint)}</small></li>`).join("")
        : '<li class="none">Nothing found</li>';
    }
    function go(item) {
      if (!item) return;
      palette.close();
      if (item.run) item.run();
      else location.href = item.href;
    }
    function open() {
      input.value = "";
      chosen = 0;
      fill();
      palette.showModal();
      input.focus();
      if (!files) fetch(`${ROOT}api/files`).then((r) => r.json()).then((f) => { files = f; fill(); }).catch(() => {});
    }
    document.querySelector("[data-palette-open]").addEventListener("click", open);
    input.addEventListener("input", () => { chosen = 0; fill(); });
    input.addEventListener("keydown", (event) => {
      if (event.key === "ArrowDown") { chosen = Math.min(chosen + 1, items.length - 1); fill(); event.preventDefault(); }
      if (event.key === "ArrowUp") { chosen = Math.max(chosen - 1, 0); fill(); event.preventDefault(); }
      if (event.key === "Enter") { go(items[chosen]); event.preventDefault(); }
    });
    results.addEventListener("click", (event) => go(items[event.target.closest("[data-n]")?.dataset.n]));
    palette.addEventListener("click", (event) => { if (event.target === palette) palette.close(); });
    document.addEventListener("keydown", (event) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") { event.preventDefault(); palette.open ? palette.close() : open(); }
      // "/" puts the cursor in the page's search, where it has one.
      if (event.key === "/" && !event.target.closest("input, textarea, select")) {
        const search = document.querySelector("[data-search]");
        if (search) { event.preventDefault(); search.focus(); }
      }
    });
  }

  // The machine picker: every machine profile known (hundreds), found by typing a brand or
  // a model, narrowed by kind; each with its drawing and its size. In the add dialog it
  // makes a virtual machine; on a machine's settings it chooses its profile.
  {
    let profiles = null;
    const load = () => (profiles ??= fetch(`${ROOT}api/profiles`).then((r) => r.json()));
    const KINDS = { fdm: "3D printer", laser: "Laser", label: "Label printer", cnc: "CNC" };
    const FIRMWARE = { marlin: "Marlin", klipper: "Klipper", prusa: "Prusa", "prusa-buddy": "Prusa Buddy", grbl: "GRBL", "dymo-d1": "DYMO" };
    const size = (p) => {
      if (p.kind === "label") return [...new Set(p.media.map((m) => m.width_mm))].sort((a, b) => a - b).join(", ") + " mm tape";
      const [x, y, z] = p.build_volume_mm.map((v) => Math.round(v));
      return z > 0 ? `${x} × ${y} × ${z} mm` : `${x} × ${y} mm`;
    };
    async function mount(el) {
      const action = el.dataset.action, selected = el.dataset.selected || "", auto = el.dataset.auto;
      const inSettings = auto !== undefined;
      el.innerHTML = `<input type="search" class="picker-search" placeholder="Search machines: a brand, a model" aria-label="Search machines" autocomplete="off">
        <div class="picker-kinds" role="group" aria-label="Kind"></div>
        <div class="picker-kinds" data-firmwares role="group" aria-label="Firmware"></div>
        <ul class="picker-list" role="listbox"><li class="picker-none">Loading…</li></ul>`;
      const search = el.querySelector(".picker-search"), kinds = el.querySelector(".picker-kinds"), list = el.querySelector(".picker-list");
      const all = await load();
      let kind = "", firmware = "", items = [], at = 0;
      const counts = {}, firmwares = {};
      for (const p of all) {
        counts[p.kind] = (counts[p.kind] || 0) + 1;
        if (p.firmware) firmwares[p.firmware] = (firmwares[p.firmware] || 0) + 1;
      }
      const byFirmware = el.querySelector("[data-firmwares]");
      byFirmware.innerHTML = [["", "Any firmware"], ...Object.entries(firmwares).sort((a, b) => b[1] - a[1]).map(([f, n]) => [f, `${FIRMWARE[f] || f} ${n}`])]
        .map(([f, t]) => `<button type="button" data-firmware="${escapeHtml(f)}" aria-pressed="${f === firmware}">${escapeHtml(t)}</button>`).join("");
      byFirmware.addEventListener("click", (event) => {
        const b = event.target.closest("[data-firmware]");
        if (!b) return;
        firmware = b.dataset.firmware;
        for (const x of byFirmware.children) x.setAttribute("aria-pressed", x === b);
        at = 0;
        fill();
      });
      kinds.innerHTML = [["", `All ${all.length}`], ...Object.keys(KINDS).filter((k) => counts[k]).map((k) => [k, `${KINDS[k]} ${counts[k]}`])]
        .map(([k, t]) => `<button type="button" data-kind="${k}" aria-pressed="${k === kind}">${escapeHtml(t)}</button>`).join("");
      function fill() {
        const words = search.value.trim().toLowerCase().split(/\s+/).filter(Boolean);
        const found = all.filter((p) => (!kind || p.kind === kind) && (!firmware || p.firmware === firmware) && words.every((w) => `${p.brand} ${p.model}`.toLowerCase().includes(w)));
        // The chosen one first, then by brand and model.
        found.sort((a, b) => (b.id === selected) - (a.id === selected) || a.brand.localeCompare(b.brand) || a.model.localeCompare(b.model, undefined, { numeric: true }));
        items = found.slice(0, 60);
        if (inSettings && !words.length) items.unshift({ id: "", auto: true });
        at = Math.min(at, Math.max(items.length - 1, 0));
        list.innerHTML = items.length ? items.map((p, n) => p.auto
          ? `<li role="option" data-n="${n}" aria-selected="${n === at}" class="${selected === "" ? "chosen" : ""}"><span class="picker-auto">≈</span><span class="picker-text"><b>Recognise it from what it says</b><small>${auto ? `now: ${escapeHtml(all.find((x) => x.id === auto)?.model || auto)}` : "the profile follows what the machine reports"}</small></span></li>`
          : `<li role="option" data-n="${n}" aria-selected="${n === at}" class="${p.id === selected ? "chosen" : ""}"><img src="${ROOT}profiles/${p.id}.svg" alt="" loading="lazy"><span class="picker-text"><b>${escapeHtml(p.brand)} ${escapeHtml(p.model)}</b><small>${escapeHtml(size(p))}${p.firmware ? ` · ${escapeHtml(FIRMWARE[p.firmware] || p.firmware)}${p.firmware_version ? ` ${escapeHtml(p.firmware_version)}` : ""}` : ""}</small></span>${p.id === selected ? '<span class="picker-tag">chosen</span>' : ""}</li>`).join("")
          + (found.length > items.length ? `<li class="picker-none">${found.length - items.length} more: type to narrow</li>` : "")
          : '<li class="picker-none">No machine matches</li>';
        list.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
      }
      async function pick(item) {
        if (!item) return;
        const body = new URLSearchParams({ profile: item.id });
        if (!inSettings) {
          // A virtual machine: made, then its page.
          const form = Object.assign(document.createElement("form"), { method: "post", action });
          form.innerHTML = `<input type="hidden" name="profile" value="${escapeHtml(item.id)}">`;
          document.body.append(form);
          form.submit();
          return;
        }
        const response = await fetch(action, { method: "POST", body, headers: { "x-requested-with": "fetch" } });
        if (!response.ok) { toast(`Not saved: ${await response.text()}`, "bad"); return; }
        toast(item.auto ? "Recognised from what it says" : `Profile: ${item.brand} ${item.model}`);
        setTimeout(() => location.reload(), 300);
      }
      search.addEventListener("input", () => { at = 0; fill(); });
      search.addEventListener("keydown", (event) => {
        if (event.key === "ArrowDown") { at = Math.min(at + 1, items.length - 1); fill(); event.preventDefault(); }
        if (event.key === "ArrowUp") { at = Math.max(at - 1, 0); fill(); event.preventDefault(); }
        if (event.key === "Enter") { pick(items[at]); event.preventDefault(); }
      });
      list.addEventListener("click", (event) => pick(items[event.target.closest("[data-n]")?.dataset.n]));
      kinds.addEventListener("click", (event) => {
        const b = event.target.closest("[data-kind]");
        if (!b) return;
        kind = b.dataset.kind;
        for (const x of kinds.children) x.setAttribute("aria-pressed", x === b);
        at = 0;
        fill();
      });
      fill();
    }
    // The page's own pickers come after the shell: found once the page is read.
    const mountAll = () => { for (const el of document.querySelectorAll("[data-picker]:not([data-seen])")) {
      el.dataset.seen = "1";
      // In the add dialog: built when the dialog first opens.
      const dialog = el.closest("dialog");
      if (dialog) {
        const once = () => { if (dialog.open && !el.dataset.ready) { el.dataset.ready = "1"; mount(el); } };
        new MutationObserver(once).observe(dialog, { attributes: true, attributeFilter: ["open"] });
        once();
      } else {
        mount(el);
      }
    } };
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", mountAll);
    else mountAll();
  }
