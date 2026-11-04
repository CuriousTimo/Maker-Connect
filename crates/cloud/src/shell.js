  // The app's shell, on every page: the sidebar (pages, machines live, counts), the dialog
  // to add a machine, toasts, forms saved in the background, and the ⌘K palette.
  const ROOT = document.querySelector(".sidebar").dataset.root;
  const escapeHtml = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

  // The dialog to add a machine: from any [data-add-open], and at #add.
  {
    const dialog = document.getElementById("add-dialog");
    const open = () => { if (!dialog.open) dialog.showModal(); };
    document.addEventListener("click", (event) => {
      if (event.target.closest("[data-add-open]")) { event.preventDefault(); open(); }
      if (event.target.closest("[data-add-close]") || event.target === dialog) dialog.close();
    });
    if (location.search.includes("unknown-code")) {
      document.querySelector("[data-add-note]").textContent = "No machine waits with that code.";
      open();
    }
    if (location.hash === "#add") open();
  }

  // How a machine's state is shown, the same on every page. A machine that waits for
  // someone (a label printer without a cassette) shows the reason, amber: not an error.
  function shownState(m) {
    const t = m.online ? m.telemetry : null;
    if (!t) return { text: "Offline", cls: "offline" };
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
      attention: machines.filter((m) => m.online && m.telemetry && (m.telemetry.state === "attention" || m.telemetry.state === "error")).length,
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
