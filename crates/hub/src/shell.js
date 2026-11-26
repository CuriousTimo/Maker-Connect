  // The app's shell, on every page (docs/design/ui.md): the theme switch, the navigation by the
  // person's role, the phone's More menu, the confirm
  // dialog, the toast, and forms saved in the background.
  const ROOT = document.querySelector(".side").dataset.root;
  const escapeHtml = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

  // A request of this site that finds the session gone (it ran out, or was ended elsewhere)
  // sends the page to sign in.
  const plainFetch = window.fetch.bind(window);
  window.fetch = async (...args) => {
    const response = await plainFetch(...args);
    if (response.status === 401 && new URL(response.url, location.href).origin === location.origin) {
      location.href = `/login?next=${encodeURIComponent(location.pathname + location.search)}`;
    }
    return response;
  };

  // Who is signed in, in which space and role; the links the role may see; the spaces to go to.
  // The navigation follows the role from /api/me; the hub refuses the pages and calls anyway.
  plainFetch("/api/me").then((r) => (r.ok ? r.json() : null)).then((me) => {
    if (!me) return;
    const role = me.space ? me.space.role : null;
    const staff = role === "admin" || role === "owner";
    const allowed = { staff, member: !!role && !staff, instance: !!me.instance_admin };
    for (const el of document.querySelectorAll("[data-gate]")) el.hidden = !allowed[el.dataset.gate];
    const who = document.querySelector("[data-me-who]");
    who.textContent = `${me.display_name}${role ? ` · ${role}` : ""}`;
    const list = document.querySelector("[data-me-spaces]");
    const spaces = me.spaces || [];
    if (spaces.length < 2) return;
    let org = null;
    for (const s of spaces) {
      if (s.organisation !== org) {
        org = s.organisation;
        const g = document.createElement("div");
        g.className = "group";
        g.textContent = org;
        list.append(g);
      }
      const b = document.createElement("button");
      b.type = "button";
      b.className = "side-link";
      b.textContent = s.name;
      if (me.space && s.id === me.space.id) b.setAttribute("aria-current", "true");
      b.addEventListener("click", async () => {
        const r = await plainFetch("/spaces/switch", { method: "POST", body: new URLSearchParams({ space: s.id }), headers: { "x-requested-with": "fetch" } });
        if (r.ok) location.href = "/";
      });
      list.append(b);
    }
  });

  // An instant the server wrote (<time data-local>) in the viewer's own time zone (#159). This
  // script runs before the page's content, so once that is there.
  document.addEventListener("DOMContentLoaded", () => {
    for (const el of document.querySelectorAll("time[data-local]")) {
      const at = new Date(el.dateTime);
      if (isNaN(at)) continue;
      const date = at.toLocaleDateString();
      el.textContent = el.dataset.local === "minute" ? `${date} ${at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}` : date;
    }
  });

  // The phone's More menu closes on Escape and on a click outside.
  {
    const more = document.querySelector(".more-menu");
    document.addEventListener("click", (e) => { if (more.open && !more.contains(e.target)) more.open = false; });
    document.addEventListener("keydown", (e) => { if (e.key === "Escape" && more.open) { more.open = false; more.querySelector("summary").focus(); } });
  }

  // How a machine's state is shown, the same on every page. A machine that waits for
  // someone (a label printer without a cassette) shows the reason, amber: not an error.
  // A print on the bed can be taken at or below this (°C); as the hub's TAKE_BELOW_C.
  const TAKE_BELOW_C = 30;
  // What is loaded in a machine: "S-0007 · Galaxy Black · 412 g left".
  function loadedText(l) {
    const amount = l.unit === "g" ? Math.round(l.amount) : l.unit === "m" ? l.amount.toFixed(1) : l.amount;
    const from = l.place ? ` · from ${l.place}${l.slot ? `, slot ${l.slot}` : ""}` : "";
    return `${l.item} · ${l.name}${l.material ? ` · ${l.material}` : ""} · ${amount} ${l.unit} left${from}`;
  }
  // Time left with the clock: "45 min left · ETA 16:35" (with the day when not today).
  function timeLeft(seconds) {
    if (seconds == null || !isFinite(seconds)) return "";
    const s = Math.max(0, Math.round(seconds));
    const left = s < 60 ? "under a minute" : s < 3600 ? `${Math.round(s / 60)} min` : `${Math.floor(s / 3600)} h ${String(Math.round((s % 3600) / 60)).padStart(2, "0")} min`;
    const end = new Date(Date.now() + s * 1000);
    const today = end.toDateString() === new Date().toDateString();
    const clock = end.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    const day = today ? "" : `${end.toLocaleDateString([], { weekday: "short" })} `;
    return `${left} left · ETA ${day}${clock}`;
  }
  // A plate for a project that ended and waits for someone (the farm): how many came off
  // good, Set ready, or a bad print. `prefix` leads from the page to /machines/<id>.
  function confirmHtml(m, prefix) {
    const c = m.confirm;
    if (!c) return "";
    const ok = c.state === "finished";
    const how = ok ? "is done: how many came off good?" : `${c.state === "failed" ? "failed" : "was stopped"}: did any come off good?`;
    return `<div class="banner ok"><span><b>${escapeHtml(c.file_name)}</b>${c.part ? ` for <b>${escapeHtml(c.part)}</b>${c.project ? ` (${escapeHtml(c.project)})` : ""}` : ""} ${how}</span>
      <form class="row btns" data-confirm action="${prefix}${encodeURIComponent(m.id)}/ready"><input type="hidden" name="job" value="${c.job}">
        <label class="field">Good <input name="good" type="number" min="0" max="10000" value="${ok ? c.per_plate : 0}" aria-label="Parts that came off good"> / ${c.per_plate}</label>
        <span class="fw"><button class="btn primary">Set ready</button></span><span class="fw"><button class="btn danger" formaction="${prefix}${encodeURIComponent(m.id)}/bad">Bad print</button></span></form></div>`;
  }
  document.addEventListener("submit", async (event) => {
    const form = event.target;
    if (!form.matches("form[data-confirm]")) return;
    event.preventDefault();
    const action = event.submitter?.getAttribute("formaction") || form.getAttribute("action");
    const bad = action.endsWith("/bad");
    if (bad && !(await ask("A bad print: nothing of it counts, and the plate goes back in the queue. Sure?", { ok: "Bad print", danger: true }))) return;
    const r = await fetch(action, { method: "POST", body: new URLSearchParams(new FormData(form)), headers: { "x-requested-with": "fetch" } });
    if (r.ok) toast(bad ? "Bad print: the plate goes back in the queue" : `Counted: ${(await r.json()).done} made`);
    else toast(`Not done: ${await r.text()}`, "bad");
  });
  function shownState(m) {
    if (m.out_of_use) return { text: "Out of use", cls: "out" };
    if (m.confirm) return { text: m.confirm.state === "finished" ? "Confirm the plate" : "Check the plate", cls: "attention" };
    if (m.upkeep?.due && !(m.online && m.telemetry?.state === "busy")) return { text: "Service due", cls: "attention" };
    const t = m.online ? m.telemetry : null;
    if (!t) return { text: "Offline", cls: "offline" };
    // A finished print on the bed: cooling, then it waits for someone to take it.
    if (m.on_bed && t.state === "idle") {
      return t.bed_c != null && t.bed_c > TAKE_BELOW_C ? { text: "Cooling", cls: "cooling" } : { text: "Take the print", cls: "attention" };
    }
    // What the printer asks on its screen ("Sliced for another printer?"), when it says.
    if ((t.state === "attention" || t.state === "error") && m.asking) return { text: m.asking[0], cls: t.state, title: m.asking[1] };
    if (t.state === "attention" || (t.state === "idle" && t.problem)) return { text: t.problem || "Attention", cls: "attention" };
    // A failed or stopped job may have left something: nothing starts until the bed is clear.
    if (m.queue_held && t.state === "idle") return { text: "Check the bed", cls: "attention" };
    // HS7b: a laser with a job loaded, idle: it waits to be framed and started there.
    if (m.job?.state === "loaded" && t.state === "idle") return { text: "Loaded", cls: "loaded" };
    const names = { disconnected: "Disconnected", idle: "Idle", busy: "Busy", paused: "Paused", error: "Error" };
    return { text: names[t.state] || t.state, cls: t.state };
  }
  // A busy machine's progress: how far (%), when it ends ("10:35", a weekday if not today),
  // and for a badge: the text and its two blues (done, still to go) as --pct. None: not busy,
  // or it does not say.
  function progressOf(m) {
    const j = m.online && m.telemetry && m.telemetry.state === "busy" ? m.telemetry.job : null;
    if (!j || !j.total) return null;
    const pct = Math.max(0, Math.min(100, Math.round((j.done * 100) / j.total)));
    let eta = "";
    if (j.remaining_s != null && isFinite(j.remaining_s)) {
      const end = new Date(Date.now() + Math.max(0, j.remaining_s) * 1000);
      const today = end.toDateString() === new Date().toDateString();
      eta = `${today ? "" : `${end.toLocaleDateString([], { weekday: "short" })} `}${end.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
    }
    return { pct, eta, text: eta ? `${pct} % · ${eta}` : `${pct} %`, style: `--pct:${pct}%` };
  }
  // The state chip (ui.md "States and colour"): one list of states, the word always written.
  const ST = { idle: "idle", busy: "busy", paused: "warn", attention: "warn", cooling: "idle", loaded: "idle", error: "bad", offline: "off", disconnected: "off", out: "off" };
  const stClass = (cls) => `st ${ST[cls] || "unk"}`;
  function stateBadge(m, tag = "div") {
    const s = shownState(m), p = s.cls === "busy" ? progressOf(m) : null;
    const text = p ? p.text : s.text;
    return `<${tag} class="${stClass(s.cls)}"${s.title ? ` title="${escapeHtml(s.title)}"` : ""}>${escapeHtml(text)}</${tag}>`;
  }

  // Puts a machine's state in every [data-live="state"]; a state chip takes its state's class.
  function showState(m) {
    const shown = shownState(m);
    const p = shown.cls === "busy" ? progressOf(m) : null;
    for (const el of document.querySelectorAll('[data-live="state"]')) {
      el.textContent = p ? p.text : shown.text;
      el.title = shown.title || "";
      if (el.classList.contains("st") || el.classList.contains("state")) el.className = stClass(shown.cls);
    }
  }

  // A question before something is done that cannot be taken back (ui.md "Ask first"): resolves
  // true when its button is pressed, false on Cancel, Escape or a click beside it. `ok` names
  // the button ("Stop the print"); `danger` makes it the danger button.
  function ask(text, { ok = "OK", danger = false } = {}) {
    return new Promise((resolve) => {
      const dialog = document.createElement("dialog");
      dialog.setAttribute("aria-label", "Confirm");
      dialog.innerHTML = `<p></p><div class="acts"><span class="fw"><button type="button" class="btn ghost" value="no">Cancel</button></span><span class="fw"><button type="button" class="btn ${danger ? "danger" : "primary"}" value="yes"></button></span></div>`;
      dialog.querySelector("p").textContent = text;
      dialog.querySelector('[value="yes"]').textContent = ok;
      let answer = false;
      dialog.addEventListener("click", (event) => {
        const button = event.target.closest("button");
        if (button) { answer = button.value === "yes"; dialog.close(); }
        else if (event.target === dialog) dialog.close();
      });
      dialog.addEventListener("close", () => { dialog.remove(); resolve(answer); });
      document.body.append(dialog);
      dialog.showModal();
      dialog.querySelector('[value="yes"]').focus();
    });
  }

  // A short message (the toast), instead of a page that reloads; read out politely.
  let toastTimer = null;
  function toast(text, kind = "ok") {
    const box = document.querySelector("[data-toast]");
    box.textContent = text;
    box.dataset.kind = kind;
    box.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { box.hidden = true; }, kind === "bad" ? 6000 : 3000);
  }

  // Forms marked data-async are saved in the background: a toast says how it went; the page
  // is reloaded (smoothly) only where it says data-then="reload", or goes where data-then
  // names. A page can listen for "saved" on the form to update itself.
  document.addEventListener("submit", async (event) => {
    const form = event.target;
    if (!form.matches("form[data-async]")) return;
    event.preventDefault();
    if (form.dataset.confirm && !(await ask(form.dataset.confirm, { ok: form.dataset.confirmOk || "OK", danger: "confirmDanger" in form.dataset }))) return;
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
    const title = document.querySelector(".title h1, .title h2");
    if (title) title.textContent = name;
    document.title = `${name} · Maker Connect`;
  });

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
      // On a machine's settings only profiles of its own kind (owner, 2026-11-17: a laser was offered
      // every printer); the add dialog keeps every kind.
      const own = el.dataset.kind || "";
      const all = (await load()).filter((p) => !own || p.kind === own);
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
      // one kind only: no kind buttons
      kinds.hidden = !!own;
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
  // Tabs: the design's pattern (docs/design/ui/mockups/mc.js): role=tab, aria-controls, arrow
  // keys, roving tabindex; the hash picks the tab. Only a tablist with data-tabs.
  {
    const tabsOf = (list) => [...list.querySelectorAll('[role="tab"]')].filter((t) => t.getClientRects().length > 0);
    const select = (list, tab, focus) => {
      for (const t of list.querySelectorAll('[role="tab"]')) {
        const on = t === tab, p = document.getElementById(t.getAttribute("aria-controls"));
        t.setAttribute("aria-selected", String(on)); t.tabIndex = on ? 0 : -1; if (p) p.hidden = !on;
      }
      if (focus) tab.focus();
    };
    const tabsAll = () => {
      const h = location.hash.slice(1);
      for (const list of document.querySelectorAll('[role="tablist"][data-tabs]')) {
        const ts = tabsOf(list); if (!ts.length) continue;
        let pick = ts.find((t) => t.getAttribute("aria-controls") === h);
        if (!pick) { const p = h && document.getElementById(h); if (p) pick = ts.find((t) => document.getElementById(t.getAttribute("aria-controls"))?.contains(p)); }
        select(list, pick || ts.find((t) => t.getAttribute("aria-selected") === "true") || ts[0], false);
      }
    };
    document.addEventListener("click", (e) => {
      const t = e.target.closest('[data-tabs] [role="tab"]');
      if (!t) return;
      e.preventDefault(); select(t.closest('[role="tablist"]'), t, true); history.replaceState(null, "", "#" + t.getAttribute("aria-controls"));
    });
    document.addEventListener("keydown", (e) => {
      const t = e.target.closest && e.target.closest('[data-tabs] [role="tab"]');
      if (!t) return;
      const list = t.closest('[role="tablist"]'), ts = tabsOf(list), i = ts.indexOf(t);
      const n = { ArrowRight: ts[(i + 1) % ts.length], ArrowLeft: ts[(i - 1 + ts.length) % ts.length], Home: ts[0], End: ts[ts.length - 1] }[e.key];
      if (n) { e.preventDefault(); select(list, n, true); history.replaceState(null, "", "#" + n.getAttribute("aria-controls")); }
    });
    window.addEventListener("hashchange", tabsAll);
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", tabsAll);
    else tabsAll();
  }
