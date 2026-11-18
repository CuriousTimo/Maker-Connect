# What we can learn from the Prusa Connect web app

Reference: `reference/prusa-connect` at commit 2e8c62b (v0.1.0-beta.1), read only. Licence OCL v1.1 + Micro +
NoSaaS + SWAtt: no code, CSS, texts or pictures are copied; this page is our own writing about what we saw.
`pc/` = the reference. Companion to [prusa-connect-study.md](prusa-connect-study.md), which covers the server.

Sources read: the demo walkthrough with its ten screenshots (`pc/docs/src/content/docs/demo-walkthrough.md`,
`pc/docs/src/assets/screens/`), the UI rules (`pc/.agents/rules/ui-integration.md`, `writing-style.md`, the
headings of `frontend-css.md`, `frontend-i18n.md`, `svelte-data.md`), the screen list (`pc/frontend/src/lib/screens/`)
and the Storybook tour (`pc/frontend/src/tour/start-here.mdx`). The demo was **not** run. The screen list has
screens the walkthrough pictures do not show (home, dashboard, jobs, job detail, file detail, storage,
maintenance, diagnostics, app pairing, appearance, person access), so the pictures may be older than the code.

## 1. What their app looks like

- **Shell**: left sidebar (Fleet, Fractal, Tools, Extensions, Settings); at the bottom connection state, System,
  language select, Sign out; a footer with Documentation, Support, Repository. Light theme, one orange accent.
- **Fleet**: a header with the printer and connected counts and an add button; search, state filter, sort, order,
  and a switch to show only devices with an issue. Large cards, two per row. Each card answers *which printer, what it
  is doing, what you can do*: model picture, name, model and serial, connection dot, state badge, file and
  progress with time left, nozzle and bed with their targets, the newest camera frame, a **Print** button and a
  "⋯" menu with the other actions.
- **Add a device**: a dialog in three steps: *what are you adding* → *point it here* (the lines for the
  printer's settings file and the printer menu that loads them) → *let it in* (waiting printers, each with the
  code on its screen). Options that are off in this build stay visible, greyed, with the reason.
- **One printer**: name (editable), state, connection, model, firmware, serial, device id with copy; camera
  frame with Live view and full screen; the running print; Pause, Resume, Stop, Start, where a control that does
  not apply is disabled and **says why on hover** (when it becomes available); then telemetry, this print,
  recent prints, files per storage.
- **Print sheet**: from the card, a sheet slides in from the right: drop a `.gcode/.bgcode/.3mf`, or pick a file
  the printer already has (search, folders, sizes). The button names the target printer.
- **Tools** (for developers): pick a device; a G-code box (Enter sends, up arrow recalls); the answer with its
  outcome; a command log (time, kind, sent, outcome, answer); a **state tree**: every reported value with the
  time it last changed; the camera.
- **Settings**: Account; API credentials (label, minted once and shown once, expiry, last use); People with an
  empty state (a sentence that nobody else is on the server, and a button to add a person); Modules with a state per module and
  why it is not ready; Server health.
- **First run**: four steps (who gets in, administrator, modules, review); every other request answers 503
  until it is done.
- **Extensions**: installed and available, a search, a card per extension with version, kind, source link,
  one switch; the install dialog says what it installs and what it asks for.

## 2. How they keep it right (their process)

From `ui-integration.md` and the tour:

1. **Every state is designed**: loading, empty, error, refused by permission, partial. An empty panel must not stand for several meanings: could not find out, nothing there and not yet
   happened are each shown in their own words; the same for a value with no basis, one not fetched, and a real zero.
2. **Hierarchy before decoration**: the screen's question is answered at a glance; rank by consequence;
   colour = state, one variable per state, so an unstyled state shows up; density is a feature.
3. **Primitives only**: a lint rule refuses raw `<button>`, `<input>`, spinners etc. in pages; exceptions need a
   written reason. All values come from tokens; light and dark both checked.
4. **Filters behave the same everywhere**: a reset, a clear X and Escape on search, a clear-filters button while
   any filter is on, and the same offer on an empty result.
5. **Every text through the language catalogue** (English and Czech).
6. **Storybook shows every screen in every state** and with fleet sizes of 1, 2 and 12 printers.
7. **UX tests in a real Chromium** measure geometry and colours (overflow, clipping, focus off screen), not
   just "the CSS rule exists"; and they read the console.
8. **Review question**: does the change fit in with the screens around it, not only work on its own?
9. **Writing style**: for a B2 reader; short sentences; a heading is the first line of the answer.

## 3. Ours beside theirs

| Subject | Theirs | Ours | Lesson |
|---|---|---|---|
| Overview | Fleet cards with camera | Now page (busy/free/needs someone, ends next, wall screen) + Machines list by kind + side list with progress | Ours is stronger for a makerspace; keep |
| Card/row | Answers which / doing / **what you can do**: one primary button + ⋯ menu | Row answers which and doing; every action is behind a click into the machine | **Adopt**: one primary action per row (Print / Label / Burn → queue) and a ⋯ menu |
| Start a job | Sheet from the card: drop or pick a file on the machine, button names the target | Via Files / Prepare / queue | **Adopt**: a "print on this machine" sheet from the machine and its row |
| Disabled controls | Disabled + why on hover, everywhere | In places (laser "not yet: frame it first") | **Adopt** as a rule: no silent disabled button |
| Unknown vs zero | Rule: separate sentences | not checked page by page | **Adopt**: audit our pages for blank / "–" / 0 that hides "unknown" |
| Empty states | Sentence + the next action | Some ("No machines yet. Add one") | **Adopt** for every list (members, tokens, files, queue, materials) |
| Filters | Same toolbar everywhere, reset, X, Escape | Chips + search + group per page; reset behaviour not checked | **Adopt** one filter bar with reset |
| Add a device | 3-step dialog incl. what the device's screen shows | Add dialog with the agent's code; virtual machine | Compare step by step; adopt "options off in this build stay visible with the reason" |
| Debug | Tools: G-code console + **state tree with last-change time** + command log | Terminal tab per machine | **Adopt** the state tree with times (great for audits and #2/#3 Buddy work) |
| API tokens | Label, shown once, expiry, last use | Space → API tokens (expiry and last use not checked) | Check |
| First run | 4-step setup, server refuses all else until done | not checked (a grep found no first-run code) | **Consider** a first-run setup |
| Modules health | Per module ready / why not | About page | Idea for #12 (agent modules) |
| Language | en + cs catalogue | English strings in Rust | Owner decision (Dutch later?) |
| States in dev | Storybook, every state, 1/2/12 printers | Virtual machines in the hub | **Adopt** a fixtures page or test that renders each page empty / loading / error / refused / farm of 12 |
| UI tests | Real Chromium measurements | none of that kind | Later, after the points above |

## 4. Proposed order (ours to build, our own design)

A proposal only: it starts after the current step of [prusa-connect-study.md](prusa-connect-study.md) §7, one thing at a time.

1. A short UI rule file of our own (states, unknown vs zero, disabled-with-reason, empty state with action,
   filter bar behaviour, one primary action per row), written in our words, next to `CLAUDE.md`.
2. State tree with last-change time in the machine's Terminal tab (helps #2/#3).
3. One primary action + ⋯ menu per machine row; the "start a job on this machine" sheet.
4. Pass over every page for unknown/empty/refused states and disabled reasons.
5. Shared filter bar with reset.
6. First-run setup.
7. Fixtures of every page in every state; later browser measurements.

Open: run their demo (`make demo`, needs Go + pnpm) to see the newer screens (home, dashboard, jobs,
maintenance) before step 3.
