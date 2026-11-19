/* Maker Connect v2 mockups: the one shared script. Mockup behaviour only (View as, theme, tabs,
   dialogs, the pending state of a command); the product's pages are server-rendered (ui.md). */
(function () {
  var b = document.body, views = ["guest", "user", "admin", "instance"];
  function store(k, v) { try { if (v === null) localStorage.removeItem(k); else localStorage.setItem(k, v); } catch (e) {} }
  function load(k) { try { return localStorage.getItem(k); } catch (e) { return null; } }

  /* View as: Guest (Eli), Member (Sam), Admin (Noor), Instance admin (Ada, also the space's owner) */
  var sel = document.querySelector("[data-view-select]");
  var q = new URLSearchParams(location.search).get("as");
  var v = views.indexOf(q) >= 0 ? q : load("mc-view");
  if (views.indexOf(v) < 0) v = "user";
  var guestNote = null;
  function setView(x) {
    b.dataset.view = x; if (sel) sel.value = x; store("mc-view", x);
    var main = document.getElementById("main");
    if (guestNote) { guestNote.remove(); guestNote = null; }
    if (x === "guest" && main && !main.querySelector('[data-only~="guest"].note-role') && !main.hasAttribute("data-no-guest-note")) {
      guestNote = document.createElement("p"); guestNote.className = "note-role";
      guestNote.innerHTML = "<b>As a guest (Eli):</b> you look only. Buttons that change something are not shown to a guest; the space's staff can make you a member.";
      main.insertBefore(guestNote, main.firstChild);
    }
    tabsAll();
  }
  if (sel) sel.addEventListener("change", function () { setView(sel.value); });

  /* Theme: Auto, Light, Dark */
  var th = document.querySelector("[data-theme-btn]"), order = ["system", "light", "dark"];
  function cur() { return load("mc-theme") || "system"; }
  function show() { var c = cur(), n = c === "system" ? "Auto" : c === "light" ? "Light" : "Dark"; th.textContent = n; th.setAttribute("aria-label", "Theme: " + n); th.title = "Theme: " + n + " (click to change)"; }
  if (th) { show(); th.addEventListener("click", function () { var n = order[(order.indexOf(cur()) + 1) % 3]; store("mc-theme", n === "system" ? null : n); if (n === "system") delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = n; show(); }); }

  /* Tabs: the full pattern (role=tab, aria-controls, arrow keys, roving tabindex); the hash picks the tab */
  function tabsOf(list) { return [].filter.call(list.querySelectorAll('[role="tab"]'), function (t) { return t.getClientRects().length > 0; }); }
  function select(list, tab, focus) {
    [].forEach.call(list.querySelectorAll('[role="tab"]'), function (t) {
      var on = t === tab, p = document.getElementById(t.getAttribute("aria-controls"));
      t.setAttribute("aria-selected", String(on)); t.tabIndex = on ? 0 : -1; if (p) p.hidden = !on;
    });
    if (focus) tab.focus();
  }
  function tabsAll() {
    var h = location.hash.slice(1);
    [].forEach.call(document.querySelectorAll('[role="tablist"]'), function (list) {
      var ts = tabsOf(list); if (!ts.length) return;
      var pick = ts.filter(function (t) { return t.getAttribute("aria-controls") === h; })[0];
      if (!pick) { var p = h && document.getElementById(h); if (p) pick = ts.filter(function (t) { var pan = document.getElementById(t.getAttribute("aria-controls")); return pan && pan.contains(p); })[0]; }
      if (!pick) pick = ts.filter(function (t) { return t.getAttribute("aria-selected") === "true"; })[0] || ts[0];
      select(list, pick, false);
    });
    /* Page sections behind a page navigation (Space): the link of the shown section carries aria-current */
    [].forEach.call(document.querySelectorAll("[data-pagenav]"), function (nav) {
      var secs = document.querySelectorAll("[data-section]"); if (!secs.length) return;
      var shown = h && document.getElementById(h); shown = shown && shown.closest("[data-section]"); if (!shown) shown = secs[0];
      [].forEach.call(secs, function (s) { s.hidden = s !== shown; });
      [].forEach.call(nav.querySelectorAll("a"), function (a) {
        var href = a.getAttribute("href"), here = href.split("#")[0] === location.pathname.split("/").pop() || href.charAt(0) === "#";
        if (here && href.split("#")[1] === shown.id) a.setAttribute("aria-current", "page"); else if (here) a.removeAttribute("aria-current");
      });
    });
  }
  document.addEventListener("click", function (e) {
    var t = e.target.closest('[role="tab"]'); if (!t) return;
    e.preventDefault(); select(t.closest('[role="tablist"]'), t, true); history.replaceState(null, "", "#" + t.getAttribute("aria-controls"));
  });
  document.addEventListener("keydown", function (e) {
    var t = e.target.closest && e.target.closest('[role="tab"]'); if (!t) return;
    var list = t.closest('[role="tablist"]'), ts = tabsOf(list), i = ts.indexOf(t), n = null;
    if (e.key === "ArrowRight") n = ts[(i + 1) % ts.length]; else if (e.key === "ArrowLeft") n = ts[(i - 1 + ts.length) % ts.length];
    else if (e.key === "Home") n = ts[0]; else if (e.key === "End") n = ts[ts.length - 1];
    if (n) { e.preventDefault(); select(list, n, true); history.replaceState(null, "", "#" + n.getAttribute("aria-controls")); }
  });
  window.addEventListener("hashchange", tabsAll);

  /* More (phone): closes on Escape and on a click outside */
  var more = document.querySelector(".more-menu");
  if (more) {
    document.addEventListener("keydown", function (e) { if (e.key === "Escape" && more.open) { more.open = false; more.querySelector("summary").focus(); } });
    document.addEventListener("click", function (e) { if (more.open && !more.contains(e.target)) more.open = false; });
  }

  /* A control with aria-disabled stays focusable, so its reason is read; it does nothing */
  document.addEventListener("click", function (e) { var d = e.target.closest('[aria-disabled="true"]'); if (d) { e.preventDefault(); e.stopImmediatePropagation(); } }, true);

  /* Dialogs: data-dialog="id" opens <dialog id>; a button with value closes it */
  document.addEventListener("click", function (e) { var o = e.target.closest("[data-dialog]"); if (!o) return; var d = document.getElementById(o.dataset.dialog); if (d && d.showModal) { e.preventDefault(); d.showModal(); } });

  /* data-goto="id": after a dialog's button, show that state (mockup navigation) */
  document.addEventListener("click", function (e) { var g = e.target.closest("[data-goto]"); if (g) location.hash = g.dataset.goto; });

  /* Undo: data-undo="what was done" shows a toast with Undo instead of asking first */
  var toast = document.getElementById("toast");
  document.addEventListener("click", function (e) {
    var u = e.target.closest("[data-undo]"); if (!u || !toast) return;
    toast.innerHTML = "<span></span><button class='btn small' type='button'>Undo</button><button class='btn small' type='button' aria-label='Close'>×</button>";
    toast.firstChild.textContent = u.dataset.undo; toast.hidden = false;
    var bs = toast.querySelectorAll("button");
    bs[0].onclick = function () { toast.firstChild.textContent = "Undone."; bs[0].remove(); };
    bs[1].onclick = function () { toast.hidden = true; };
  });

  /* A command's pending state: requested, then settled (done, refused or outcome unknown).
     data-cmd="ok|refused|unknown"; data-settled holds the settled text; the status line is aria-live. */
  document.addEventListener("click", function (e) {
    var c = e.target.closest("[data-cmd]"); if (!c || c.getAttribute("aria-busy") === "true") return;
    var st = document.getElementById(c.getAttribute("aria-describedby") || "") || c.parentNode.querySelector(".status");
    var label = c.textContent;
    c.setAttribute("aria-busy", "true"); c.textContent = label + "…";
    if (st) { st.className = "status"; st.textContent = c.dataset.pending || "Sent. Waiting for the machine to answer."; }
    setTimeout(function () {
      c.removeAttribute("aria-busy"); c.textContent = label;
      if (st) { var k = c.dataset.cmd; st.className = "status " + (k === "ok" ? "ok" : k === "refused" ? "bad" : "unk"); st.textContent = c.dataset.settled || "Done."; }
    }, 1200);
  });

  /* Laser: PPE confirmed enables Start burn (it still asks once more) */
  var f = document.querySelector("[data-ppe]");
  if (f) {
    var ok = f.querySelector("[data-ppe-ok]"), s = document.querySelector("[data-start]"), w = document.querySelector("[data-start-why]"), msg = f.querySelector("[data-ppe-msg]");
    ok.addEventListener("click", function () {
      var all = [].every.call(f.querySelectorAll("[data-ppe-item]"), function (c) { return c.checked; });
      if (!all) { msg.textContent = "Tick every item you have and wear, then Confirm."; return; }
      f.closest("li").className = "done";
      f.innerHTML = "<p role='status'><b>Confirmed 15:48:</b> you have and wear laser safety glasses; the lid is closed and the extraction runs.</p>";
      if (s) { s.setAttribute("aria-disabled", "false"); s.dataset.dialog = "confirm-burn"; w.textContent = "Ready: placed, framed after the last Set origin, PPE confirmed. It asks once more before it starts."; }
    });
  }

  setView(v);
})();
