use axum::response::{IntoResponse, Response};

/// The bar at the top of every page: the name (back to the machines), the space, and a
/// live count of its machines, with whether the page is getting live updates; and the
/// button that opens the dialog to add a machine (by its code, or a virtual one). `root`
/// leads from the page to the site's root ("" or "../"); `here` is the page's place in the
/// navigation ("machines", "files").
pub(crate) fn header(space: &str, root: &str, here: &str) -> String {
    // An empty link is the page itself: from the root, the machines are at "./".
    let home = if root.is_empty() { "./" } else { root };
    let link = |path: &str, name: &str, place: &str, icon: &str| {
        let current = if place == here {
            r#" aria-current="page""#
        } else {
            ""
        };
        format!(
            r#"<a href="{home}{path}"{current}><svg viewBox="0 0 20 20" aria-hidden="true">{icon}</svg><span>{name}</span></a>"#
        )
    };
    format!(
        r##"<script>try {{ const t = localStorage.getItem("theme"); if (t === "light" || t === "dark") document.documentElement.dataset.theme = t; }} catch {{}}</script>
<header class="topbar">
  <a class="brand" href="{home}"><img src="{root}logo.svg" alt="" width="28" height="28"><span><b>Maker Connect</b><small>{space}</small></span></a>
  <nav class="side-nav" aria-label="Pages">{now}{machines}{projects}{files}{prepare}{stock}{cameras}{space_link}</nav>
  <div class="top-right" data-top-right>
    <div class="summary">
      <span class="count" title="Machines in this space"><svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="4" width="14" height="10" rx="1.5"/><path d="M7 17h6M10 14v3"/></svg><b data-sum="machines">–</b><span class="sr">machines</span></span>
      <span class="count" title="Online: reporting now"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3.5 8a9.5 9.5 0 0 1 13 0M6 11a6 6 0 0 1 8 0"/><circle cx="10" cy="14.5" r="1.2"/></svg><b data-sum="online">–</b><span class="sr">online</span></span>
      <span class="count busy-count" title="Busy: printing, cutting, running a job"><svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="6.5"/><path d="M10 6.5V10l2.5 2"/></svg><b data-sum="busy">–</b><span class="sr">busy</span></span>
      <span class="count attention-count" data-attention-count hidden title="Need someone: a plate to confirm, a print to take, an error"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 3.5 17 16H3z"/><path d="M10 8.5v3.5M10 14v.01"/></svg><b data-sum="attention">–</b><span class="sr">need someone</span></span>
      <span class="live" data-live-dot title="Live updates"><i></i></span>
    </div>
    <button type="button" class="search-open" data-palette-open title="Go to a machine, a file or a page"><svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="9" cy="9" r="5.5"/><path d="m13.2 13.2 3.8 3.8"/></svg><span>Go to…</span><kbd>⌘K</kbd></button>
  </div>
</header>
<aside class="sidebar" data-root="{home}">
  <div class="side-section">
    <div class="side-title"><span>Machines</span><button type="button" class="side-add" data-add-open title="Add a machine" aria-label="Add a machine"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 4.5v11M4.5 10h11"/></svg></button></div>
    <nav class="side-machines" data-side-machines aria-label="Machines"></nav>
  </div>
</aside>
<div class="toasts" data-toasts aria-live="polite"></div>
<dialog class="palette" data-palette aria-label="Go to">
  <input type="search" data-palette-input placeholder="A machine, a file, a page…" autocomplete="off" aria-label="Search">
  <ul class="palette-results" data-palette-results role="listbox"></ul>
  <p class="palette-hint"><kbd>↑</kbd><kbd>↓</kbd> choose <kbd>↵</kbd> go <kbd>esc</kbd> close</p>
</dialog>
<dialog id="add-dialog" aria-labelledby="add-title">
  <div class="dialog-head">
    <button type="button" class="back" data-wizard-back hidden aria-label="Back">←</button>
    <h2 id="add-title">Add a machine</h2>
    <button type="button" class="close" data-add-close aria-label="Close">✕</button>
  </div>
  <section class="step" data-step="start">
    <p class="dialog-sub">How does it connect?</p>
    <div class="choices">
      <button type="button" class="choice" data-go="klipper"><b>A printer on Klipper</b><small>Voron, Sovol SV08, Neptune 4… with Mainsail or Fluidd: Moonraker connects here by itself</small></button>
      <button type="button" class="choice" data-go="prusa"><b>A Prusa with built-in Connect</b><small>MINI, MK4, XL: its own firmware connects here, no agent</small></button>
      <button type="button" class="choice" data-go="usb"><b>A machine on USB</b><small>A Marlin printer, an MK3S, a GRBL laser, a DYMO: through the agent on a computer beside it</small></button>
      <button type="button" class="choice" data-go="virtual"><b>A virtual one</b><small>Simulated in the hub, to try everything without a machine</small></button>
    </div>
    <p class="dialog-foot">It already shows a code? <button type="button" class="link" data-go="code">Enter the code</button></p>
  </section>
  <section class="step" data-step="klipper" hidden>
    <h3>A printer on Klipper</h3>
    <p class="dialog-sub">Its name here; then two lines for its <code>moonraker.conf</code>.</p>
    <form data-klipper-form action="{home}klipper">
      <input name="name" maxlength="60" required aria-label="Name" placeholder="Voron 2.4">
      <button>Next</button>
    </form>
  </section>
  <section class="step" data-step="klipper-conf" hidden>
    <h3>On the printer</h3>
    <ol class="steps">
      <li>Open its Mainsail or Fluidd, then <b>Machine</b> (or Configuration) and <b>moonraker.conf</b>.</li>
      <li>Add these two lines, save, and restart Moonraker:<pre class="conf" data-klipper-conf></pre></li>
    </ol>
    <p class="hint" data-local-warning hidden>This page was opened as <b>localhost</b>, which the printer cannot reach: open it by this computer's address on the network, or put that address in the line.</p>
    <p class="waiting" data-klipper-wait><i></i>Waiting for it to connect…</p>
    <p class="dialog-foot"><a data-klipper-page href="#">Open its page</a> · the lines are on its Settings too.</p>
  </section>
  <section class="step" data-step="prusa" hidden>
    <h3>A Prusa with built-in Connect</h3>
    <ol class="steps">
      <li>Put a file <code>prusa_printer_settings.ini</code> on its USB stick:<pre class="conf" data-prusa-ini></pre></li>
      <li>On the printer: <b>Settings → Network → Prusa Connect → Load Settings</b>, then <b>Add printer to Connect</b>.</li>
      <li>It shows a code: enter it here.</li>
    </ol>
    <form method="post" action="{home}pair" data-pair-form>
      <input name="code" maxlength="6" autocomplete="off" required aria-label="Pairing code" placeholder="ABC234">
      <button>Add</button>
    </form>
    <p class="note" data-pair-note></p>
  </section>
  <section class="step" data-step="usb" hidden>
    <h3>A machine on USB</h3>
    <ol class="steps">
      <li><a href="{home}downloads">Download the agent</a> for the computer the machine is plugged into: a Pi, a PC, a Mac.</li>
      <li>Start it with this hub's address and the machine's port: the Downloads page gives the command.</li>
      <li>It shows a code: enter it here.</li>
    </ol>
    <form method="post" action="{home}pair" data-pair-form>
      <input name="code" maxlength="6" autocomplete="off" required aria-label="Pairing code" placeholder="ABC234">
      <button>Add</button>
    </form>
    <p class="note" data-pair-note></p>
  </section>
  <section class="step" data-step="code" hidden>
    <h3>With its code</h3>
    <p class="dialog-sub">An agent shows a code when it starts, and a Prusa when its Connect points here.</p>
    <form method="post" action="{home}pair" data-pair-form>
      <input name="code" maxlength="6" autocomplete="off" required aria-label="Pairing code" placeholder="ABC234">
      <button>Add</button>
    </form>
    <p class="note" data-pair-note data-add-note></p>
  </section>
  <section class="step" data-step="virtual" hidden>
    <h3>A virtual one</h3>
    <p class="dialog-sub">Simulated in the hub: it moves, heats and measures its bed. Pick its machine.</p>
    <div class="picker" data-picker data-action="{home}virtual"></div>
  </section>
</dialog>
<script>
{SHELL_SCRIPT}</script>"##,
        space = escape(space),
        now = link(
            "",
            "Now",
            "now",
            r#"<circle cx="10" cy="10" r="7"/><path d="M10 6v4l2.5 2.5"/>"#
        ),
        machines = link(
            "machines",
            "Machines",
            "machines",
            r#"<rect x="2.5" y="4" width="15" height="10" rx="2"/><path d="M7 17h6M10 14v3"/>"#
        ),
        projects = link(
            "projects",
            "Projects",
            "projects",
            r#"<rect x="3" y="4" width="14" height="13" rx="2"/><path d="M7 8h6M7 11h6M7 14h3"/>"#
        ),
        files = link(
            "files",
            "Files",
            "files",
            r#"<path d="M5 2.5h6.5l3.5 3.5v11.5H5z"/><path d="M11.5 2.5V6H15"/>"#
        ),
        // Prepare: shown when this build has a tool in it (today the laser design page).
        prepare = if crate::laser_design::ASSETS.is_empty() {
            String::new()
        } else {
            link(
                "prepare",
                "Prepare",
                "prepare",
                r#"<path d="M4 16 13.5 6.5l2 2L6 18H4z"/><path d="m12 8 2-2 2 2-2 2"/><path d="M3 3.5h5M5.5 1v5"/>"#,
            )
        },
        stock = link(
            "stock",
            "Materials",
            "stock",
            r#"<path d="M3 6.5 10 3l7 3.5v7L10 17l-7-3.5z"/><path d="M3 6.5 10 10l7-3.5M10 10v7"/>"#
        ),
        cameras = if cfg!(feature = "cameras") {
            link(
                "cameras",
                "Cameras",
                "cameras",
                r#"<rect x="2.5" y="5.5" width="11" height="9" rx="1.5"/><path d="m13.5 9 4-2.5v7l-4-2.5"/>"#,
            )
        } else {
            String::new()
        },
        space_link = link(
            "space",
            "Space",
            "space",
            r#"<circle cx="7.5" cy="7" r="2.5"/><circle cx="13.5" cy="8" r="2"/><path d="M3 16c.6-3 2.3-4.5 4.5-4.5S11.4 13 12 16M12.5 12.2c2.2-.4 3.9.9 4.5 3.8"/>"#
        ),
    )
}

/// The logo mark, for the page's header and its icon.
pub(crate) async fn logo() -> Response {
    (
        [("content-type", "image/svg+xml")],
        include_str!("../../../../assets/logo-mark.svg"),
    )
        .into_response()
}

pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The machine list: drawn by the page from `/api/machines` every two seconds.
/// The styling both pages share.
/// The app's shell on every page: the add dialog, the sidebar's machines, the counts, toasts,
/// forms saved in the background, and the ⌘K palette.
pub(crate) const SHELL_SCRIPT: &str = include_str!("../shell.js");

pub(crate) const STYLE: &str = r#"  :root { color-scheme: light dark; --bg: #f6f5f2; --card: #fff; --ink: #1d1d1b; --muted: #6b6b66; --line: #e2e0da; --accent: #2f7d6d; --off: #9a9893; --bad: #b3412e; --warn: #b7791f; --busy: #2f6fd1; }
  @media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { --bg: #111214; --card: #1b1c1f; --ink: #f2f1ee; --muted: #a3a29d; --line: #2e2f33; --accent: #5fbfa9; --off: #77767a; --bad: #e07a63; --warn: #d9a441; --busy: #6ea8ff; } }
  :root[data-theme="dark"] { --bg: #111214; --card: #1b1c1f; --ink: #f2f1ee; --muted: #a3a29d; --line: #2e2f33; --accent: #5fbfa9; --off: #77767a; --bad: #e07a63; --warn: #d9a441; --busy: #6ea8ff; }
  * { box-sizing: border-box; }
  /* Hidden is hidden, whatever display a class gives (a notice is flex). */
  [hidden] { display: none !important; }
  body { margin: 0; background: var(--bg); color: var(--ink); font: 15px/1.45 system-ui, sans-serif; }
  /* The app's shell: a sidebar beside every page, the page beside it at one width. */
  :root[data-theme="light"] { color-scheme: light; } :root[data-theme="dark"] { color-scheme: dark; }
  .theme-pick { display: flex; gap: 2px; padding: 4px 4px 2px; }
  .theme-pick button { flex: 1; text-align: center !important; font-size: 13px !important; padding: 5px 4px !important; }
  .theme-pick button[aria-pressed="true"] { background: var(--line); }
  :root { --side: 248px; --fdm: #2f7d6d; --laser: #d9692b; --label: #7d5bd6; }
  @media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { --fdm: #5fbfa9; --laser: #f08a4b; --label: #a98bf5; } }
  :root[data-theme="dark"] { --fdm: #5fbfa9; --laser: #f08a4b; --label: #a98bf5; }
  @view-transition { navigation: auto; }
  ::view-transition-old(root), ::view-transition-new(root) { animation-duration: .18s; }
  .sidebar { view-transition-name: sidebar; position: fixed; inset: var(--top) auto 0 0; width: var(--side); z-index: 10; display: flex; flex-direction: column; gap: 6px; padding: 16px 12px; background: color-mix(in srgb, var(--card) 70%, var(--bg)); border-right: 1px solid var(--line); overflow: hidden; }
  /* The pages in a bar at the top; the sidebar holds only the machines. */
  :root { --top: 56px; }
  body { padding-left: var(--side); padding-top: var(--top); }
  .brand { display: flex; align-items: center; gap: 10px; padding: 0 8px 0 4px; text-decoration: none; width: calc(var(--side) - 28px); flex: none; }
  .brand span { display: grid; line-height: 1.2; min-width: 0; }
  .brand b { font-size: 16px; letter-spacing: -0.2px; }
  .brand small { color: var(--muted); font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sidebar svg, .topbar svg { width: 18px; height: 18px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; flex: none; }
  .search-open { display: flex; align-items: center; gap: 8px; width: 100%; padding: 7px 10px; margin-bottom: 6px; background: var(--bg); color: var(--muted); border: 1px solid var(--line); font-weight: 500; font-size: 14px; }
  .search-open span { flex: 1; text-align: left; }
  kbd { font: 11px ui-monospace, Menlo, monospace; padding: 1px 5px; border: 1px solid var(--line); border-radius: 4px; color: var(--muted); }
  .side-nav { display: grid; gap: 2px; }
  .side-nav a { display: flex; align-items: center; gap: 10px; padding: 8px 10px; border-radius: 8px; color: var(--muted); text-decoration: none; font-weight: 600; font-size: 14px; }
  .side-nav a:hover { color: var(--ink); background: color-mix(in srgb, var(--line) 60%, transparent); }
  .side-nav a[aria-current=page] { color: var(--ink); background: var(--line); }
  /* Only the machine list scrolls: the pages above and the user below stay put. */
  .side-section { margin-top: 14px; min-height: 0; flex: 1 1 auto; display: flex; flex-direction: column; }
  .side-title { display: flex; align-items: center; justify-content: space-between; padding: 0 6px 6px 10px; color: var(--muted); font-size: 11px; font-weight: 700; letter-spacing: 1px; text-transform: uppercase; }
  .side-add { background: transparent; color: var(--muted); padding: 3px; border-radius: 6px; display: grid; }
  .side-add:hover { color: var(--ink); background: var(--line); }
  .side-add svg { width: 16px; height: 16px; }
  .side-machines { display: grid; gap: 1px; align-content: start; min-height: 0; overflow-y: auto; overscroll-behavior: contain; }
  .side-machines a { position: relative; display: grid; grid-template-columns: auto 1fr auto; align-items: center; gap: 9px; padding: 7px 10px; border-radius: 8px; color: var(--ink); text-decoration: none; font-size: 14px; }
  .side-machines a:hover { background: color-mix(in srgb, var(--line) 60%, transparent); }
  .side-machines a[aria-current=page] { background: var(--line); }
  .side-machines a::before { content: ""; position: absolute; left: 0; top: 9px; bottom: 9px; width: 3px; border-radius: 2px; background: var(--k, transparent); }
  .side-machines a[data-kind=fdm] { --k: var(--fdm); } .side-machines a[data-kind=laser] { --k: var(--laser); } .side-machines a[data-kind=label] { --k: var(--label); }
  .side-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .side-machines small { color: var(--muted); font-size: 12px; max-width: 90px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .side-machines small.attention { color: var(--warn); } .side-machines small.error { color: var(--bad); }
  /* A file dragged from Files: the machines take it. */
  body.dragging-file .side-machines a { box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 35%, transparent); }
  .side-machines a.drop { background: color-mix(in srgb, var(--accent) 22%, transparent); box-shadow: inset 0 0 0 2px var(--accent); }
  .side-progress { grid-column: 2 / -1; height: 3px; border-radius: 2px; background: color-mix(in srgb, var(--busy) 28%, transparent); overflow: hidden; }
  .side-progress i { display: block; height: 100%; background: var(--busy); transition: width .5s; }
  .side-empty { margin: 4px 10px; color: var(--muted); font-size: 13px; }
  /* States, one list for badges and dots: quiet unless someone should look. Free is a ring
     (on, nothing to do), busy is filled, attention amber, an error red. */
  .dot { width: 9px; height: 9px; border-radius: 50%; background: var(--off); flex: none; }
  .dot.idle { background: transparent; box-shadow: inset 0 0 0 2px var(--accent); }
  /* Green: available. Blue: busy. */
  .dot.busy { background: var(--busy); box-shadow: 0 0 0 3px color-mix(in srgb, var(--busy) 25%, transparent); }
  .dot.paused, .dot.attention { background: var(--warn); } .dot.error { background: var(--bad); }
  .dot.cooling { background: var(--busy); } .dot.loaded { background: var(--accent); } .dot.out { background: transparent; box-shadow: inset 0 0 0 1.5px var(--off); }
  /* The counts, at the top of every page. */
  .topbar { view-transition-name: topbar; position: fixed; inset: 0 0 auto 0; height: var(--top); z-index: 11; display: flex; align-items: center; gap: 14px; padding: 0 16px; background: color-mix(in srgb, var(--card) 70%, var(--bg)); border-bottom: 1px solid var(--line); box-sizing: border-box; }
  .top-right { margin-left: auto; display: flex; align-items: center; gap: 14px; min-width: 0; }
  .topbar .side-nav { display: flex; gap: 2px; min-width: 0; overflow-x: auto; scrollbar-width: none; }
  .topbar .side-nav::-webkit-scrollbar { display: none; }
  .topbar .side-nav a { padding: 7px 10px; gap: 7px; white-space: nowrap; }
  .topbar .side-nav svg { width: 17px; height: 17px; }
  .topbar .search-open { width: auto; margin: 0; padding: 6px 10px; }
  .topbar .search-open span { flex: none; }
  .topbar .side-user { margin: 0; padding: 0; border: 0; }
  .topbar .side-user summary { padding: 4px 6px 4px 10px; }
  .topbar .side-user .who { max-width: 160px; }
  .topbar .side-menu { top: calc(100% + 8px); bottom: auto; left: auto; right: 0; width: 260px; max-height: calc(100vh - var(--top) - 24px); }
  @media (max-width: 1180px) { .brand { width: auto; } .brand span, .topbar .side-user .who small, .topbar .search-open span, .topbar .search-open kbd { display: none; } }
  @media (max-width: 980px) { .topbar .side-nav a span { display: none; } .topbar .side-user .who { display: none; } }
  .summary { display: flex; align-items: center; gap: 18px; color: var(--muted); font-size: 14px; white-space: nowrap; }
  .summary b { color: var(--ink); font-variant-numeric: tabular-nums; }
  /* A count is an icon and a number; the word is in its hover (and for screen readers). */
  .summary .count { display: inline-flex; align-items: center; gap: 5px; cursor: default; }
  .summary .count svg { width: 17px; height: 17px; }
  .summary .busy-count svg { color: var(--busy); } .summary .attention-count svg, .summary .attention-count b { color: var(--warn); }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .live { display: inline-flex; align-items: center; gap: 8px; }
  .live i { width: 8px; height: 8px; border-radius: 50%; background: var(--off); }
  .live.on i { background: var(--accent); box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 25%, transparent); }
  .state.pulse { animation: pulse .9s ease-out; }
  @keyframes pulse { 0% { box-shadow: 0 0 0 0 currentColor; } 100% { box-shadow: 0 0 0 8px transparent; } }
  /* Toasts: what a save did, for a moment. */
  .toasts { position: fixed; right: 20px; bottom: 20px; z-index: 50; display: grid; gap: 8px; }
  .toast { padding: 10px 16px; border-radius: 10px; background: var(--ink); color: var(--bg); font-weight: 600; font-size: 14px; box-shadow: 0 8px 30px rgb(0 0 0 / .3); animation: toast-in .2s ease-out; transition: opacity .4s, transform .4s; }
  .toast.bad { background: var(--bad); color: #fff; }
  .toast.gone { opacity: 0; transform: translateY(6px); }
  @keyframes toast-in { from { opacity: 0; transform: translateY(8px); } }
  /* The machine picker: search, kinds, a list with drawings. */
  .picker { display: grid; gap: 10px; }
  .picker-search { width: 100%; }
  .picker-kinds { display: flex; gap: 6px; flex-wrap: wrap; }
  .picker-kinds button { background: transparent; color: var(--muted); border: 1px solid var(--line); padding: 4px 10px; font-size: 13px; }
  .picker-kinds button[aria-pressed=true] { color: var(--ink); border-color: var(--accent); background: color-mix(in srgb, var(--accent) 12%, transparent); }
  .picker-list { list-style: none; margin: 0; padding: 4px; max-height: 340px; overflow-y: auto; border: 1px solid var(--line); border-radius: 10px; }
  .picker.in-card .picker-list { max-height: 300px; }
  .picker-list li { display: flex; align-items: center; gap: 12px; padding: 6px 8px; border-radius: 8px; cursor: pointer; }
  .picker-list li[aria-selected=true] { background: var(--line); }
  .picker-list li:hover { background: color-mix(in srgb, var(--line) 70%, transparent); }
  .picker-list img, .picker-auto { width: 40px; height: 40px; border-radius: 8px; flex: none; }
  .picker-auto { display: grid; place-items: center; background: var(--line); color: var(--muted); font-size: 20px; }
  .picker-text { display: grid; min-width: 0; flex: 1; }
  .picker-text b { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .picker-text small { color: var(--muted); font-size: 12px; }
  .picker-tag { font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: .5px; color: var(--accent); }
  .picker-list li.chosen b { color: var(--accent); }
  .picker-none { color: var(--muted); cursor: default !important; background: none !important; font-size: 13px; padding: 10px !important; }
  /* The palette. */
  dialog.palette { width: min(560px, calc(100vw - 32px)); margin-top: 12vh; padding: 0; }
  .palette input { width: 100%; border: 0; border-bottom: 1px solid var(--line); border-radius: 0; padding: 16px 18px; font-size: 17px; background: transparent; outline: none; }
  .palette-results { list-style: none; margin: 0; padding: 6px; max-height: 50vh; overflow-y: auto; }
  .palette-results li { display: flex; align-items: center; gap: 10px; padding: 9px 12px; border-radius: 8px; cursor: pointer; }
  .palette-results li span { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .palette-results li small { color: var(--muted); }
  .palette-results li[aria-selected=true] { background: var(--line); }
  .palette-results li.none { color: var(--muted); cursor: default; }
  .palette-hint { margin: 0; padding: 8px 16px 12px; color: var(--muted); font-size: 12px; display: flex; gap: 6px; align-items: center; }
  /* On a phone: the pages as a tab bar at the bottom. */
  .side-user { flex: none; margin-top: auto; border-top: 1px solid var(--line); padding-top: 8px; position: relative; }
  .side-user summary { display: flex; align-items: center; gap: 8px; padding: 6px 6px 6px 10px; border-radius: 8px; cursor: pointer; list-style: none; }
  .side-user summary::-webkit-details-marker { display: none; }
  .side-user summary:hover { background: color-mix(in srgb, var(--line) 60%, transparent); }
  .side-user summary svg { color: var(--muted); }
  .side-user[open] summary svg { transform: rotate(180deg); }
  .side-user .who { display: flex; flex-direction: column; min-width: 0; flex: 1; }
  .side-user .who b, .side-user .who small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .side-user .who b { font-size: 13px; }
  .side-user .who small { color: var(--muted); font-size: 12px; }
  .side-menu { position: absolute; bottom: calc(100% + 4px); left: 0; right: 0; display: grid; gap: 2px; padding: 6px; background: var(--card); border: 1px solid var(--line); border-radius: 10px; box-shadow: 0 10px 30px rgb(0 0 0 / .2); z-index: 20; max-height: 60vh; overflow-y: auto; }
  .side-menu > div { display: grid; gap: 2px; }
  .side-menu small { color: var(--muted); font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: .5px; padding: 6px 8px 2px; }
  .side-menu button, .side-menu a { display: block; width: 100%; text-align: left; padding: 7px 8px; border: 0; border-radius: 6px; background: none; color: var(--ink); font: inherit; font-size: 14px; font-weight: 500; text-decoration: none; cursor: pointer; }
  .side-menu button:hover, .side-menu a:hover { background: var(--line); }
  .side-menu button[aria-current] { color: var(--accent); font-weight: 700; }
  .side-menu form { margin: 0; border-top: 1px solid var(--line); padding-top: 2px; display: block; }
  @media (max-width: 820px) {
    /* A phone: no machine list at the side, the pages in a bar at the bottom. */
    body { padding-left: 0; padding-bottom: 72px; }
    .sidebar { display: none; }
    .topbar { padding: 0 10px; gap: 8px; }
    .topbar .side-nav { position: fixed; inset: auto 0 0 0; height: 64px; justify-content: space-around; padding: 6px 8px; background: color-mix(in srgb, var(--card) 70%, var(--bg)); border-top: 1px solid var(--line); overflow: visible; }
    .topbar .side-nav { gap: 0; padding: 6px 2px; }
    .topbar .side-nav a { flex: 1; min-width: 0; flex-direction: column; gap: 2px; font-size: 10.5px; padding: 6px 2px; }
    .topbar .side-nav a span { overflow: hidden; text-overflow: ellipsis; max-width: 100%; }
    .topbar .side-nav a span { display: block; }
    .summary { gap: 12px; font-size: 13px; }
    html, body { overflow-x: hidden; }
    main > * { max-width: 100%; }
  }

  dialog { width: min(600px, calc(100vw - 32px)); padding: 0; border: 1px solid var(--line); border-radius: 16px; background: var(--card); color: var(--ink); box-shadow: 0 20px 60px rgb(0 0 0 / 0.35); }
  dialog::backdrop { background: rgb(0 0 0 / 0.45); backdrop-filter: blur(2px); }
  /* A question before something is done (shell.js ask): in the middle, short. */
  dialog.ask { width: min(440px, calc(100vw - 32px)); padding: 22px 22px 18px; }
  dialog.ask p { margin: 0; font-size: 16px; line-height: 1.45; overflow-wrap: anywhere; }
  .ask-buttons { display: flex; justify-content: flex-end; gap: 10px; margin-top: 20px; }
  .ask-buttons .ghost { background: transparent; color: var(--ink); border: 1px solid var(--line); }
  .ask-buttons .danger { background: var(--bad); }
  .dialog-head { display: flex; align-items: center; justify-content: space-between; padding: 18px 22px 0; }
  .dialog-head h2 { margin: 0; font-size: 20px; }
  dialog .close { background: transparent; color: var(--muted); font-size: 18px; padding: 4px 8px; }
  dialog .back { background: transparent; color: var(--muted); font-size: 18px; padding: 4px 8px; margin-right: 4px; }
  .dialog-head h2 { flex: 1; }
  /* The add wizard: a choice, then its steps. */
  .choices { display: grid; gap: 8px; }
  .choice { display: grid; gap: 2px; text-align: left; padding: 12px 14px; background: var(--bg); color: var(--ink); border: 1px solid var(--line); border-radius: 12px; font-weight: 400; }
  .choice:hover, .choice:focus-visible { border-color: var(--accent); }
  .choice b { font-size: 15px; }
  .choice small { color: var(--muted); font-size: 13px; }
  .dialog-foot { margin: 14px 0 0; color: var(--muted); font-size: 14px; }
  button.link { background: none; border: 0; padding: 0; color: var(--accent); font: inherit; font-weight: 600; text-decoration: underline; cursor: pointer; }
  ol.steps { margin: 8px 0 14px; padding-left: 20px; display: grid; gap: 8px; font-size: 14px; }
  ol.steps li { min-width: 0; }
  pre.conf { margin: 6px 0 0; padding: 10px 12px; border-radius: 8px; background: var(--bg); border: 1px solid var(--line); font-size: 13px; white-space: pre-wrap; overflow-wrap: anywhere; user-select: all; }
  .waiting { display: flex; align-items: center; gap: 8px; color: var(--muted); font-size: 14px; }
  .waiting i { width: 10px; height: 10px; border-radius: 50%; background: var(--off); animation: wait 1.2s ease-in-out infinite; }
  .waiting.done { color: var(--ink); font-weight: 600; }
  .waiting.done i { background: var(--accent); animation: none; }
  @keyframes wait { 50% { opacity: .3; } }
  dialog section { padding: 16px 22px 18px; border-top: 1px solid var(--line); margin-top: 16px; }
  dialog section + section { margin-top: 0; }
  dialog h3 { margin: 0; font-size: 16px; }
  .dialog-sub { margin: 4px 0 12px; color: var(--muted); font-size: 14px; }
  dialog form { margin: 0; }
  dialog select { flex: 1; }
  dialog .note:empty { display: none; }
  dialog .note { margin: 10px 0 0; }
  main, main.wide { max-width: 1320px; margin: 0 auto; padding: 28px 36px 40px; }
  @media (max-width: 820px) { main, main.wide { padding: 20px 16px 28px; } }
  form { display: flex; gap: 8px; margin-bottom: 20px; flex-wrap: wrap; align-items: center; }
  input, select { font: inherit; padding: 8px 10px; border: 1px solid var(--line); border-radius: 8px; background: var(--card); color: var(--ink); max-width: 100%; }
  input[name=code] { width: 9em; text-transform: uppercase; letter-spacing: 2px; }
  button { font: inherit; font-weight: 600; padding: 8px 14px; border: 0; border-radius: 8px; background: var(--accent); color: #fff; cursor: pointer; }
  .note { color: var(--bad); }
  /* A machine in the list: the same columns for every kind, so they line up down the page:
     picture, name and model, what it does now, how it connects, last seen, its state. */
  .machine { display: grid; grid-template-columns: 56px minmax(0, 1.6fr) minmax(0, 1.5fr) minmax(0, 1fr) 112px; align-items: center; gap: 4px 20px; padding: 14px 18px; margin-bottom: 10px; background: var(--card); border: 1px solid var(--line); border-radius: 14px; }
  .m-col { display: grid; gap: 2px; min-width: 0; }
  .m-col > * { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .m-col b { font-weight: 500; font-variant-numeric: tabular-nums; }
  .m-col small { color: var(--muted); font-size: 13px; }
  .name { font-weight: 600 !important; font-size: 17px; }
  .machine .state { justify-self: stretch; text-align: center; }
  .m-spool { display: flex; align-items: center; gap: 6px; }
  /* The machine list's tools: chips with counts, a search, grouping, the view. */
  .list-tools { display: flex; flex-wrap: wrap; gap: 10px 16px; align-items: center; justify-content: space-between; margin-bottom: 14px; }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; }
  .chip { display: inline-flex; gap: 6px; align-items: baseline; padding: 5px 12px; border-radius: 999px; border: 1px solid var(--line); background: transparent; color: var(--muted); font-size: 13px; font-weight: 600; }
  .chip b { color: var(--ink); font-variant-numeric: tabular-nums; }
  .chip[aria-pressed=true] { background: var(--line); color: var(--ink); border-color: var(--muted); }
  .chip.busy b { color: var(--accent); } .chip.needs b { color: var(--warn); } .chip.error b { color: var(--bad); } .chip.offline b, .chip.out b { color: var(--off); }
  .list-controls { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .list-controls input[type=search] { width: 220px; }
  .view-switch { display: inline-flex; border: 1px solid var(--line); border-radius: 8px; overflow: hidden; }
  .view-switch button { background: transparent; color: var(--muted); padding: 6px 8px; border-radius: 0; display: grid; }
  .view-switch button[aria-pressed=true] { background: var(--line); color: var(--ink); }
  .view-switch svg { width: 18px; height: 18px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; }
  /* A plate that ended and waits for someone (the farm), on every page that shows it. */
  .notice.confirm { display: flex; align-items: center; justify-content: space-between; gap: 10px 16px; flex-wrap: wrap; padding: 10px 14px; margin: -4px 0 12px; border-radius: 10px; border: 1px solid var(--warn); background: color-mix(in srgb, var(--warn) 10%, var(--card)); }
  .notice.confirm span { flex: 1 1 300px; }
  .notice.confirm form { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; margin: 0; }
  .notice.confirm input[name=good] { width: 5em; padding: 5px 8px; }
  .notice.confirm .danger { background: transparent; color: var(--bad); border: 1px solid color-mix(in srgb, var(--bad) 50%, transparent); }
  /* One banner per kind of trouble; none when all is well. */
  .banners { display: grid; gap: 8px; margin-bottom: 14px; }
  .banners:empty { display: none; }
  .banner { padding: 10px 14px; border-radius: 10px; border: 1px solid; font-size: 14px; line-height: 1.6; }
  .banner b { margin-right: 6px; }
  .banner a { font-weight: 600; }
  .banner small { color: var(--muted); }
  .banner.needs { border-color: color-mix(in srgb, var(--warn) 50%, transparent); background: color-mix(in srgb, var(--warn) 10%, transparent); }
  .banner.error { border-color: color-mix(in srgb, var(--bad) 50%, transparent); background: color-mix(in srgb, var(--bad) 10%, transparent); }
  .banner.offline { border-color: var(--line); color: var(--muted); }
  /* Labels on machines. */
  .tags { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 4px; }
  .tag { display: inline-block; padding: 1px 7px; border-radius: 5px; background: color-mix(in srgb, var(--accent) 14%, transparent); color: var(--accent); font-size: 11px; font-weight: 700; letter-spacing: .3px; }
  .tag.induction { background: color-mix(in srgb, var(--warn) 14%, transparent); color: var(--warn); }
  /* The table view. */
  .table-wrap { overflow-x: auto; margin-bottom: 10px; border: 1px solid var(--line); border-radius: 12px; background: var(--card); }
  .machine-table { width: 100%; border-collapse: collapse; font-size: 14px; }
  .machine-table th { text-align: left; color: var(--muted); font-size: 12px; font-weight: 600; text-transform: uppercase; letter-spacing: .5px; padding: 8px 12px; border-bottom: 1px solid var(--line); white-space: nowrap; }
  .machine-table td { padding: 9px 12px; border-bottom: 1px solid var(--line); vertical-align: middle; }
  .machine-table tr:last-child td { border-bottom: 0; }
  .machine-table tbody tr { cursor: pointer; }
  .machine-table tbody tr:hover { background: color-mix(in srgb, var(--line) 50%, transparent); }
  .machine-table a { text-decoration: none; }
  .machine-table .state { font-size: 11px; padding: 2px 8px; }
  .machine-table small { color: var(--muted); display: block; max-width: 260px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .t-progress { width: 120px; height: 5px; border-radius: 3px; background: color-mix(in srgb, var(--busy) 28%, transparent); overflow: hidden; margin-bottom: 3px; }
  .t-progress i { display: block; height: 100%; background: var(--busy); }
  .muted { color: var(--muted); }
  .m-swatch { display: inline-block; width: 10px; height: 10px; border-radius: 50%; border: 1px solid var(--line); flex: none; }
  @media (max-width: 900px) { .machine { grid-template-columns: 56px minmax(0, 1fr) 104px; } .m-conn { display: none; } .m-now { grid-column: 2; } }
  .state { justify-self: end; font-weight: 700; letter-spacing: 1px; text-transform: uppercase; font-size: 12px; padding: 4px 10px; border-radius: 6px; background: transparent; color: var(--muted); box-shadow: inset 0 0 0 1.5px var(--line); }
  .state.busy { background: var(--busy); color: #fff; box-shadow: none; }
  /* Busy with a known progress: dark blue as far as it got, light blue for what is left. */
  .state.busy.pct { background: linear-gradient(to right, var(--busy) var(--pct), color-mix(in srgb, var(--busy) 38%, var(--card)) var(--pct)); letter-spacing: .3px; text-transform: none; font-variant-numeric: tabular-nums; white-space: nowrap; }
  .state.idle { color: var(--accent); box-shadow: inset 0 0 0 1.5px color-mix(in srgb, var(--accent) 60%, transparent); }
  .state.paused { background: var(--warn); color: #fff; box-shadow: none; }
  .state.offline, .state.disconnected { color: var(--off); box-shadow: inset 0 0 0 1.5px color-mix(in srgb, var(--off) 45%, transparent); }
  .state.error { background: var(--bad); color: #fff; box-shadow: none; }
  .state.attention { background: var(--warn); color: #fff; box-shadow: none; }
  .state.cooling { color: var(--busy); box-shadow: inset 0 0 0 1.5px var(--busy); }
  .state.loaded { color: var(--accent); box-shadow: inset 0 0 0 1.5px var(--accent); }
  .state.out { background: transparent; color: var(--muted); box-shadow: inset 0 0 0 1.5px var(--off); }
  .attention-count b { color: var(--warn) !important; }
  .thumb { width: 56px; height: 56px; border-radius: 10px; background: var(--line); }
  .empty { color: var(--muted); }
  .card-progress { grid-column: 2 / -1; height: 6px; margin-top: 8px; border-radius: 3px; background: color-mix(in srgb, var(--busy) 28%, transparent); overflow: hidden; }
  .card-progress i { display: block; height: 100%; background: var(--busy); transition: width 0.5s; }
  h2.group { margin: 26px 0 12px; font-size: 15px; color: var(--muted); text-transform: uppercase; letter-spacing: 1px; }
  h2.group span { margin-left: 6px; font-weight: 500; }
  a { color: inherit; }
  a.machine { text-decoration: none; }
  a.machine:hover { border-color: var(--accent); }
  .back { color: var(--muted); text-decoration: none; }
  h2 { margin: 6px 0 4px; font-size: 24px; letter-spacing: -0.3px; }
  dl { display: grid; grid-template-columns: auto 1fr; gap: 8px 18px; margin: 16px 0; }
  dt { color: var(--muted); }
  dd { margin: 0; overflow-wrap: anywhere; }
"#;

pub(crate) const PAGE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Maker Connect</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{style}</style>
</head>
<body>
{header}
<main>
  <div class="list-tools" data-tools hidden>
    <div class="chips" data-chips role="toolbar" aria-label="Show"></div>
    <div class="list-controls">
      <input type="search" data-search placeholder="Name, model, label, file…" aria-label="Find a machine">
      <select data-group aria-label="Group by"><option value="kind">By kind</option><option value="model">By model</option><option value="label">By label</option><option value="none">Not grouped</option></select>
      <div class="view-switch" role="group" aria-label="View"><button type="button" data-view="cards" title="Cards" aria-label="Cards"><svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="4" width="14" height="5" rx="1.5"/><rect x="3" y="11" width="14" height="5" rx="1.5"/></svg></button><button type="button" data-view="table" title="Table" aria-label="Table"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3 5h14M3 10h14M3 15h14"/></svg></button></div>
    </div>
  </div>
  <div class="banners" data-banners></div>
  <div id="machines"><p class="empty">Loading…</p></div>
</main>
<script>
  const esc = (t) => String(t).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const KIND = { fdm: "3D printer", laser: "Laser", cnc: "CNC", cutter: "Cutting plotter", label: "Label printer", paper: "Paper printer", scale: "Scale", other: "Machine" };
  let profiles = null;
  // A column of a machine's row: a value, and what it is under it.
  const col = (cls, value, caption) => `<div class="m-col ${cls}"><b>${esc(value ?? "–")}</b><small>${esc(caption ?? "")}</small></div>`;
  const LINK = { virtual: "Virtual", "prusa-connect": "Prusa Connect", agent: "Agent", simplyprint: "Moonraker" };
  // What it does now: its job, else what matters for its kind.
  function now(m, t) {
    if (t.job) {
      const percent = Math.round((t.job.done * 100) / Math.max(1, t.job.total));
      return [`${percent} % · ${t.job.file}`, timeLeft(t.job.remaining_s) || "printing"];
    }
    if (t.nozzle_c != null) return [`${t.nozzle_c.toFixed(0)} °C · ${t.bed_c != null ? `${t.bed_c.toFixed(0)} °C` : "–"}`, "nozzle · bed"];
    if (m.kind === "label") {
      const tape = t.cassette === false ? "no cassette" : m.media ? `tape ${m.media}` : "tape not set";
      const pct = t.battery_percent ?? null;
      return [`${tape}${pct != null ? ` · ${pct} %` : ""}`, pct != null ? "tape · battery" : "tape"];
    }
    if (t.position_mm) return [`X ${t.position_mm[0].toFixed(1)} · Y ${t.position_mm[1].toFixed(1)}`, "head, mm"];
    return [null, "nothing reported"];
  }
  // Said only when something is off: offline (since when), or late to report.
  function trouble(m) {
    if (m.out_of_use) return `<div class="m-col m-conn"></div>`;
    if (!m.online) return col("m-conn", "Offline", m.seen_seconds_ago == null ? "never reported" : `last seen ${seen(m.seen_seconds_ago)}`);
    if (m.seen_seconds_ago != null && m.seen_seconds_ago > 30) return col("m-conn", "Late", `last report ${seen(m.seen_seconds_ago)}`);
    return `<div class="m-col m-conn"></div>`;
  }
  function seen(seconds) {
    if (seconds == null) return "never";
    if (seconds < 60) return `${seconds} s ago`;
    if (seconds < 3600) return `${Math.round(seconds / 60)} min ago`;
    return `${Math.round(seconds / 3600)} h ago`;
  }
  // What someone chose here (filter, search, grouping, view), kept in this browser only.
  const ui = (() => { try { return { filter: "all", q: "", group: "kind", view: "cards", ...JSON.parse(localStorage.getItem("machine-list") || "{}") }; } catch { return { filter: "all", q: "", group: "kind", view: "cards" }; } })();
  const keep = () => { try { localStorage.setItem("machine-list", JSON.stringify({ filter: ui.filter, group: ui.group, view: ui.view })); } catch { /* not kept */ } };
  // Each machine in one bucket, for the chips: what someone would look for.
  const BUCKETS = [["all", "All"], ["busy", "Busy"], ["free", "Free"], ["needs", "Needs someone"], ["error", "Error"], ["offline", "Offline"], ["out", "Out of use"]];
  function bucket(m) {
    const cls = shownState(m).cls;
    return { busy: "busy", cooling: "busy", idle: "free", paused: "needs", attention: "needs", error: "error", offline: "offline", disconnected: "offline", out: "out" }[cls] || "free";
  }
  const modelOf = (m) => {
    const p = profiles[m.profile];
    return p ? (p.model.includes(p.brand) ? p.model : `${p.brand} ${p.model}`) : KIND[m.kind] || m.kind;
  };
  let all = [];
  function render(machines) {
      all = machines;
      const list = document.getElementById("machines");
      document.querySelector("[data-tools]").hidden = !machines.length;
      if (!machines.length) { document.querySelector("[data-banners]").innerHTML = ""; list.innerHTML = '<p class="empty">No machines yet. <a href="#add" data-add-open>Add one</a>: with the code its agent shows, or a virtual one to try everything.</p>'; return; }
      // The chips, with their counts; one with nothing in it does not show (All always does).
      const counts = Object.fromEntries(BUCKETS.map(([k]) => [k, 0]));
      for (const m of machines) { counts.all++; counts[bucket(m)]++; }
      if (ui.filter !== "all" && !counts[ui.filter]) ui.filter = "all";
      document.querySelector("[data-chips]").innerHTML = BUCKETS.filter(([k]) => k === "all" || counts[k])
        .map(([k, name]) => `<button type="button" class="chip ${k}" data-filter="${k}" aria-pressed="${ui.filter === k}">${name} <b>${counts[k]}</b></button>`).join("");
      // One banner per kind of trouble, its machines named; nothing when all is well.
      const named = (ms) => ms.map((m) => `<a href="machines/${encodeURIComponent(m.id)}">${esc(m.name)}</a>${shownState(m).text && bucket(m) === "needs" ? ` <small>(${esc(shownState(m).text.toLowerCase())})</small>` : ""}`).join(", ");
      const by = (b) => machines.filter((m) => bucket(m) === b);
      const banners = [];
      const needs = by("needs"), errors = by("error"), gone = by("offline");
      if (needs.length) banners.push(`<div class="banner needs"><b>${needs.length === 1 ? "1 machine needs" : `${needs.length} machines need`} someone</b> ${named(needs)}</div>`);
      if (errors.length) banners.push(`<div class="banner error"><b>${errors.length === 1 ? "1 machine reports" : `${errors.length} machines report`} an error</b> ${errors.map((m) => `<a href="machines/${encodeURIComponent(m.id)}">${esc(m.name)}</a>${m.telemetry?.problem ? ` <small>(${esc(m.telemetry.problem)})</small>` : ""}`).join(", ")}</div>`);
      if (gone.length) banners.push(`<div class="banner offline"><b>${gone.length} offline</b> ${gone.map((m) => `<a href="machines/${encodeURIComponent(m.id)}">${esc(m.name)}</a> <small>(${m.seen_seconds_ago == null ? "never reported" : `seen ${seen(m.seen_seconds_ago)}`})</small>`).join(", ")}</div>`);
      document.querySelector("[data-banners]").innerHTML = banners.join("");
      // What is shown: the chip, then the search (name, model, labels, the file it runs).
      const q = ui.q.trim().toLowerCase();
      const shown = machines.filter((m) => (ui.filter === "all" || bucket(m) === ui.filter)
        && (!q || [m.name, modelOf(m), ...(m.labels || []), m.telemetry?.job?.file || ""].some((t) => t.toLowerCase().includes(q))));
      if (!shown.length) { list.innerHTML = '<p class="empty">No machine like that.</p>'; return; }
      const chips = (m) => [...(m.labels || []).map((l) => `<span class="tag">${esc(l)}</span>`), m.queue?.length ? `<span class="tag" title="Waiting to print here, one after another">${m.queue.length} in queue</span>` : "", m.induction_required ? (m.training ? '<span class="tag induction" title="Only who had the training may use it">training</span>' : '<span class="tag induction" title="Only who has permission may use it">permission</span>') : ""].join("");
      const card = (m) => {
        const t = m.telemetry || {};
        const shownS = shownState(m);
        const percent = t.job ? Math.round((t.job.done * 100) / Math.max(1, t.job.total)) : null;
        const bar = t.job ? `<div class="card-progress"><i style="width: ${percent}%"></i></div>` : "";
        const [doing, what] = now(m, t);
        const profile = profiles[m.profile];
        const thumb = profile && profile.image ? `<img class="thumb" src="profiles/${esc(m.profile)}.svg" alt="">`
          : ["fdm", "laser", "cnc"].includes(m.kind) ? `<img class="thumb" src="profiles/_generic/${esc(m.kind)}.svg" alt="">` : `<span class="thumb"></span>`;
        // The model under the name, unless the name says it already; how it connects with it.
        const model = modelOf(m);
        const under = [m.name.toLowerCase().includes(model.toLowerCase()) ? null : model, LINK[m.link] || m.link].filter(Boolean).join(" · ");
        const tags = chips(m);
        return `<a class="machine" href="machines/${encodeURIComponent(m.id)}">
          ${thumb}
          <div class="m-col"><b class="name">${esc(m.name)}</b><small>${esc(under)}</small>${m.loaded ? `<small class="m-spool"><i class="m-swatch" style="background:${esc(m.loaded.color || "transparent")}"></i>${esc(loadedText(m.loaded))}</small>` : ""}${tags ? `<span class="tags">${tags}</span>` : ""}</div>
          ${col("m-now", doing, what)}
          ${trouble(m)}
          ${stateBadge(m)}
          ${bar}
        </a>`;
      };
      // The table, for many machines: one line each, the same columns for every kind.
      const row = (m) => {
        const t = m.telemetry || {};
        const st = shownState(m);
        const percent = t.job ? Math.round((t.job.done * 100) / Math.max(1, t.job.total)) : null;
        const go = `machines/${encodeURIComponent(m.id)}`;
        return `<tr data-go="${esc(go)}"><td><a href="${esc(go)}"><b>${esc(m.name)}</b></a></td><td>${stateBadge(m, "span")}</td><td>${esc(modelOf(m))}</td>
          <td>${m.loaded ? `<span class="m-spool"><i class="m-swatch" style="background:${esc(m.loaded.color || "transparent")}"></i>${esc(m.loaded.material || m.loaded.name)}</span>` : '<span class="muted">–</span>'}</td>
          <td>${t.job ? `<div class="t-progress"><i style="width:${percent}%"></i></div><small>${percent} % · ${esc(t.job.file)}</small>` : '<span class="muted">–</span>'}</td>
          <td>${t.job && t.job.remaining_s != null ? esc(timeLeft(t.job.remaining_s).split(" · ")[1] || "") : '<span class="muted">–</span>'}</td>
          <td>${chips(m) || '<span class="muted">–</span>'}</td></tr>`;
      };
      // Grouped: by kind (as it was), model or label (a machine with two labels is in both).
      const GROUPS = [["fdm", "3D printers"], ["laser", "Lasers"], ["cnc", "CNC machines"], ["cutter", "Cutting plotters"], ["label", "Label printers"], ["paper", "Paper printers"], ["scale", "Scales"], ["other", "Other machines"]];
      let groups;
      if (ui.group === "model") {
        const models = [...new Set(shown.map(modelOf))].sort();
        groups = models.map((g) => [g, shown.filter((m) => modelOf(m) === g)]);
      } else if (ui.group === "label") {
        const labels = [...new Set(shown.flatMap((m) => m.labels || []))].sort((a, b) => a.localeCompare(b));
        groups = [...labels.map((l) => [l, shown.filter((m) => (m.labels || []).includes(l))]), ["No label", shown.filter((m) => !(m.labels || []).length)]];
      } else if (ui.group === "none") {
        groups = [["", shown]];
      } else {
        groups = GROUPS.map(([kind, title]) => [title, shown.filter((m) => (GROUPS.some(([k]) => k === m.kind) ? m.kind : "other") === kind)]);
      }
      groups = groups.filter(([, ms]) => ms.length);
      for (const b of document.querySelectorAll("[data-view]")) b.setAttribute("aria-pressed", String(b.dataset.view === ui.view));
      list.innerHTML = groups.map(([title, ms]) => {
        const head = title ? `<h2 class="group">${esc(title)} <span>${ms.length}</span></h2>` : "";
        return ui.view === "table"
          ? `${head}<div class="table-wrap"><table class="machine-table"><thead><tr><th>Machine</th><th>State</th><th>Model</th><th>Material</th><th>Progress</th><th>Done at</th><th>Labels</th></tr></thead><tbody>${ms.map(row).join("")}</tbody></table></div>`
          : `${head}${ms.map((m) => card(m) + confirmHtml(m, "machines/")).join("")}`;
      }).join("");
  }
  // The tools: a chip, the search, the grouping, the view.
  document.querySelector("[data-chips]").addEventListener("click", (e) => {
    const chip = e.target.closest("[data-filter]");
    if (chip) { ui.filter = chip.dataset.filter; keep(); render(all); }
  });
  const search = document.querySelector("[data-search]");
  search.value = ui.q;
  search.addEventListener("input", () => { ui.q = search.value; render(all); });
  const group = document.querySelector("[data-group]");
  group.value = ui.group;
  group.addEventListener("change", () => { ui.group = group.value; keep(); render(all); });
  document.querySelector(".view-switch").addEventListener("click", (e) => {
    const b = e.target.closest("[data-view]");
    if (b) { ui.view = b.dataset.view; keep(); render(all); }
  });
  // A row of the table opens its machine, as a card does.
  document.getElementById("machines").addEventListener("click", (e) => {
    const tr = e.target.closest("tr[data-go]");
    if (tr && !e.target.closest("a")) location.href = tr.dataset.go;
  });
  // The list comes again on every change (server-sent events); the browser reconnects
  // by itself when the connection drops.
  (async () => {
    try { profiles = Object.fromEntries((await (await fetch("api/profiles")).json()).map((p) => [p.id, p])); } catch (error) { profiles = {}; }
    const stream = new EventSource("api/machines/stream");
    stream.onmessage = (event) => {
      const machines = JSON.parse(event.data);
      render(machines);
      summarise(machines);
    };
    stream.onopen = () => live(true);
    stream.onerror = () => live(false);
  })();
</script>
</body>
</html>
"##;
