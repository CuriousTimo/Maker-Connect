# UI/UX study: Maker Connect from the member's and the admin's side

Research, 2026-11-19 (lane UI-DESIGN, journal:UI-DESIGN:0004). It builds on
[prusa-connect-ui-study.md](prusa-connect-ui-study.md), which compared our UI with Prusa Connect's
app and set the states rules; that is not repeated here. This study takes **two viewpoints**, the
ordinary user (a member) and the admin, through every part. The target design that follows from it is
[design/ui.md](../design/ui.md); the clickable mockups are in `docs/design/ui/mockups/` (start at
`docs/design/ui/mockups/index.html`).

Evidence for today's UI is `main` at `149b8b4` (`crates/hub/src`). Branch `v2-store` (`569240b`) has
the same pages: its `crates/hub/src/web` differs from main only in the store plumbing (`Shared` →
`Store`, sqlx tests in `web/router.rs`), so every finding below holds for both.

Roles and actions come from [authz.md](../design/authz.md) and the
[actions registry](../registries/actions.md); verbs from the
[capability verbs registry](../registries/capability-verbs.md). Nothing here adds a role or an action.

## Method

- **Heuristics:** Nielsen's ten (H1 visibility of system status, H2 match with the real world, H3 user
  control and freedom, H4 consistency and standards, H5 error prevention, H6 recognition rather than
  recall, H7 flexibility and efficiency, H8 aesthetic and minimalist design, H9 help users recognise,
  diagnose and recover from errors, H10 help and documentation), plus **S**, the states rule of
  ui.md: loading, empty, error, refused and partial are each designed; unknown is shown as unknown,
  never as zero; a disabled control says why.
- **Severity:** 1 cosmetic; 2 slows people down or confuses; 3 a person fails a task, or is misled
  about the machine; 4 unsafe, or a security rule is broken.
- Read, not run: the pages were read in the source; the demo hub was not started for this study.

## 1. The two viewpoints

### The user (member)

| | |
|---|---|
| **Context** | At the machine, standing, often on a phone, sometimes with gloves or glasses on; short visits ("is my print done?"); also from home to check. Does not know the hub's words. |
| **Top tasks** | 1. See whether *my* job runs and when it ends (by the clock). 2. Start a print on a machine I may use, or queue it. 3. At the laser: load, frame, place, protect myself, Start burn. 4. Confirm a finished plate (Set Ready, Bad print). 5. Print a tape or a label for my box or spool. 6. Weigh and book my spool. 7. Ask for permission or a training on a machine I may not use, and see that it is pending. |
| **Pains today** | Controls shown that the hub then refuses; reasons that only show on hover (no hover on a phone); eight page links squeezed into a bottom bar; values that read "–" whether unknown, not applicable or not loaded; an admin-only "Add a machine" button that a member can press; silent failures when the hub is not reached. |

### The admin (space admin or owner)

| | |
|---|---|
| **Context** | At a desktop, in sessions of a few minutes to an hour; manages many machines and people; is often also a member who prints. Is not exempt from permissions: being owner is not having had the laser course. |
| **Top tasks** | 1. Add a machine by its pairing code. 2. Answer requests for permission and grant, in Space only, their own included. 3. Manage members, roles and invites. 4. Keep the queue and the farm moving (order, why a part waits). 5. See trouble across the space (offline, errors, waiting plates). 6. Set the space's rules (PPE checks), tokens, cameras. 7. Retire a machine; read the audit log. |
| **Pains today** | One-click "Give" on a request picks the level without showing what it holds; a machine's settings mix daily facts with rare, dangerous ones; moving a device to another space is offered to space staff while the design makes it an instance action; kinds that leave in v2 (Klipper, virtual) still sit in the Add dialog; colour by machine kind competes with colour by state. |

### The instance admin, and the wall screen

- **Instance admin:** rare, technical; sees every organisation and space
  (`GET /api/v1/instance/spaces`), updates the hub, moves a device between spaces
  ([decision](../design/decisions/20261119-device-move.md)). One small view.
- **The wall screen "Now"** is shared: a TV in the room, read from across it. It must work for both
  roles and leak nothing the signed-in session may not see.

## 2. Today's UI against the heuristics

### 2.1 Seen as a user (member)

| # | H | Finding | Evidence | Sev |
|---|---|---|---|---|
| U1 | H5, S | A machine's Control tab is drawn without the person's rights: `control()` gets no permission data, so a member without permission sees live jog, heat and print buttons; the refusal comes only after a press. One banner above the tabs says "You don't have permission" but the buttons below say otherwise. | `crates/hub/src/pages/machine_page.rs:1532` (`fn control(machine, profile, loaded)`), `:1209-1230` (`before_use`) | 3 |
| U2 | S, H1 | The laser's Start burn is disabled with its reason only in a `title` tooltip: on a phone at the laser the reason never shows. | `crates/hub/src/machine.js:959` | 3 |
| U3 | S | "–" stands for unknown, not applicable and not yet loaded alike; a battery whose level is unknown is drawn as an empty (zero) bar. | `machine_page.rs:461`, `:1005-1009` (`live_row`: "`–` while unknown"), `:475` (`percent.unwrap_or(0)`) | 3 |
| U4 | H9, S | Failed fetches are swallowed: the page keeps the old numbers with no error state and no time of the last answer. | `crates/hub/src/pages/now_page.rs:251`, `:268`; `crates/hub/src/files.js:541`; `crates/hub/src/shell.js:434` | 3 |
| U5 | H8, H7 | On a phone the side machine list is hidden and all eight page links go into one bottom bar at 10.5 px. | `crates/hub/src/web/shell.rs:350-359` | 3 |
| U6 | H4, H5 | "Add a machine" (the sidebar's +, and the ⌘K palette entry) shows for every member; the hub then refuses it (admin only). | `web/shell.rs:39`, `shell.js:407`; rule `crates/hub/src/access.rs:279-281` (`/pair`, `Need::Role(Role::Admin)`) | 2 |
| U7 | H8, H6 | A machine page has up to nine tabs (Overview, Queue, Telemetry, Control, Terminal, Bed mesh, USB or SD card, Camera, Settings), chosen partly by brand: USB only for `link == "prusa-connect"`. ui.md wants controls from the device's declared verbs. | `machine_page.rs:122-129`, `:190-203` | 2 |
| U8 | H3, H1 | Asking for permission works and shows "Asked on <date>" (good), but the date is cut to the UTC day, with no time, no way to withdraw, and no list of one's own requests. | `machine_page.rs:1218-1229` | 2 |
| U9 | H2 | Hub words in the person's face: "Telemetry", "Terminal", "Bed mesh" as tabs; the kind label from the link ("Maker Connect agent"). | `machine_page.rs:98-101`, `:125-127` | 1 |
| U10 | H1 | The header's counts are polled every 10 s from `/api/machines` while the pages listen to the SSE stream: two sources, so a count can disagree with the page for up to 10 s; before the first poll the counts read "–". | `shell.js:315`; `now_page.rs:285`, `machine.js:457`; `web/shell.rs:28` | 1 |
| U11 | H5 (good) | The laser card lists the steps (frame with the laser off, set origin, frame again, Start burn at the laser, glasses on), says "Not framed since the last Set origin", and asks before Start burn. Keep. | `machine_page.rs:689-712` | — |
| U12 | H5 (good) | PPE: "Before you use it: I have and wear …" with one box per item, kept until a time it shows. Keep. | `machine_page.rs:1236-1257` | — |

### 2.2 Seen as an admin

| # | H | Finding | Evidence | Sev |
|---|---|---|---|---|
| A1 | H1, H5 | A request for permission is answered with one-click **Give**, which grants "use" on that machine without showing what "use" holds; v2's API separates the two (the request's state; the grant only through `/grants`). | `crates/hub/src/pages/permissions_page.rs:210-234`; [api.md](../design/api.md) `PATCH /api/v1/access-requests/{id}` | 2 |
| A2 | H4 | A space admin can move a machine to another space from its Settings; the design makes moving an instance-admin action (allowlist #3). | `machine_page.rs:2176-2181`; [device-move decision](../design/decisions/20261119-device-move.md) | 2 |
| A3 | H4, H8 | Colour by kind (`--fdm`, `--laser`, `--label` edges in the sidebar) competes with colour by state; the wiki's principle is "colour means look here" and one colour per state. | `web/shell.rs:220-222`, `:252` | 2 |
| A4 | H2, H4 | The Add dialog offers Klipper and virtual machines, which are not kinds of the v2 device contract. | `web/shell.rs:58`, `:61`; [device-contract.md](../design/device-contract.md#kinds) | 2 |
| A5 | S | Add by code: a wrong or expired code shows a note, but the dialog does not say the code expires or where a new one comes from. | `web/shell.rs:90-117` (`data-pair-note`) | 2 |
| A6 | H8 | A machine's Settings tab carries daily and rare items together (profile, PPE list, upkeep, agent update, move). | `machine_page.rs:184-187`, `:2032-2255` | 2 |
| A7 | H4 | Today's accent is the old green (`#2f7d6d` / `#5fbfa9`), not the house colour `#93A682`. | `web/shell.rs:208-210` | 1 |
| A8 | — (good) | Space is already one subject per tab (Members, Permissions, Access, Cameras, API tokens, Audit log, General) and Access (doors) is apart from Permissions. Keep. | `crates/hub/src/pages/space_page.rs:455` | — |
| A9 | — (good) | The last owner cannot be demoted; staff move any queue item, members only their own. Keep. | `space_page.rs:722`; `machine_page.rs:1064`, `machine.js:465-507` | — |
| A10 | S | Grants table: an expiry is shown, but not who granted, and damaging actions are not apart from the levels (v2: by name, separate switches). | `permissions_page.rs:139-150` | 2 |

### 2.3 Shared: Now and the wall screen

| # | H | Finding | Evidence | Sev |
|---|---|---|---|---|
| N1 | H1 (good) | Now has the four numbers, "Needs someone" with "✓ All clear", ends next, and a wall-screen button: close to the wiki's design. Keep. | `now_page.rs:70-80`, `:204-212` | — |
| N2 | S | "No machines yet." on Now has no next step (Add for staff; nothing for a member). | `now_page.rs:245` | 1 |
| N3 | H5 | The wall screen is the signed-in person's page full screen, with its buttons: whoever walks past the TV acts as that person. | `now_page.rs:177-181`, `:281` | 2 |

## 3. Comparable apps, per task

How each tool splits admin from user, and what we take or deliberately leave.

| Task | Tool and source | How they do it | Admin vs user | We take | We do not take |
|---|---|---|---|---|---|
| Rights on a machine | OctoPrint 1.11.8 (`reference/OctoPrint-1.11.8`): `src/octoprint/access/permissions.py:272-443`, groups `access/groups.py:59-82`; Control tab `templates/tabs/control.jinja2:62-71` | Named permissions (CONTROL, PRINT, FILES_UPLOAD …) in groups (admins, users, read-only, guests). A panel without the permission is **hidden**; a button the state does not allow is **disabled** (`enable: isOperational() && !isPrinting()`), without a reason. | Groups; one printer per instance. | Hide what you may never do; disable what the state forbids. | A disabled button without its reason. |
| Rights per printer, many users | SimplyPrint, public docs: [user groups and permissions](https://help.simplyprint.io/en/article/user-groups-and-permissions-what-each-one-controls-1284ueh/), [workgroups](https://simplyprint.io/features/multiple-users/workgroups) | Groups (Administrator, Operator, custom) limited to printers, printer groups or models; the admin decides whether a user even sees printers they may not use; workgroups add time-limited access and can never grant user management. The shared "Hub" screen: signing in there grants nothing extra. | Strict: organisation authority only on user groups. | Grants with expiry on a device or a group; "a shared screen grants nothing". | Hiding machines a member may not use: our member sees them and can ask (#11). |
| Rights per person, at a glance | Prusa Connect (`reference/prusa-connect` `2e8c62b`): `frontend/src/lib/screens/person-access.svelte:1-52` | One person's page lists their access to every printer. | Settings → People. | The per-person view in Space → Permissions, and a read-only "my permissions" for the member. | — |
| A farm | OctoFarm (`reference/OctoFarm` `f22996b`): users `server/services/users.service.js:60` (only "Administrator" and "User"), live by SSE `server/services/server-side-events.service.js`; views in `screenshots/` (panel, list, camera, printer manager) | Panel, list and camera views of all printers; two roles. | Two groups, no per-printer rights. | Panel and list as cards or a table; SSE for live. | Two roles only. |
| A farm's dashboard and queue | print-farm-manager (`reference/print-farm-manager`), compared in the wiki page *Design: The Website* | Dashboard numbers, fleet cells, "needs someone"; colour per state. | Single user. | Already in the wiki design (Now, chips with counts). | Inline colours per page; a loud button per row. |
| Klipper web UIs | Moonraker (`b559b9a`): `reference/moonraker/docs/configuration.md:466-510` (`[authorization]`, `trusted_clients`, `force_logins`); Mainsail and Fluidd sit on it | Trusted networks or one login; everyone signed in may do everything. | None in practice. | — | Network trust as authorisation. |
| Spools | Spoolman (`reference/Spoolman` `eccb05e`): `client/src/pages/spools/show.tsx:44-137` (adjust, archive, archive at 0 g at once) | Spool page with remaining and used, adjust (use or measure), archive; QR label dialogs under `client/src/pages/printing/`. | No users at all. | Spool page layout, Use and Measure, "archive" for empty spools. | Single-user stock: our member's and admin's actions differ. |
| Laser: frame and start | LaserFoundry (`reference/laserfoundry` `3b9b9fa`): `crates/ui-egui/src/panels.rs:659-695` | Frame and Start side by side, both enabled when connected and idle; a "Frame power" field 0-100 %; Stop is a soft reset. | Single user. | Frame, jog, set origin, Stop as a soft reset, the jog steps. | Start without a prior frame; a frame power field (we frame at 0: person-only above 0, and the laser rule). |
| Laser: frame | LightBurn, public docs: [Framing](https://docs.lightburnsoftware.com/2.0/GetStarted/FramingBeginner/), [Laser window](https://docs.lightburnsoftware.com/2.0/Reference/LaserWindow/) | Rubber-band and box frame buttons; the Fire button is a separate low-power tool. | Single user. | Frame as an explicit step before Start. | A "Fire" button. |
| Makerspace members | Fabman, public docs: [Fabman FAQ of a fab lab](https://fablab-sandwell.notion.site/Fabman-FAQs-f520e6f0a1844e7d9e378c9aab1d4334), [fabman.io](https://fabman.io/) | Machine access control: training first, then the member activates a machine at its reader; staff record trainings. | Staff give trainings; members activate. | Training (induction) before a preset applies; a request that reaches staff. | Doors and machines in one model: our door access stays its own module. |
| Status colours, states | Prusa Connect (see prusa-connect-ui-study.md §2) | Every state designed; unknown apart from zero. | — | Already adopted as the S rule. | — |

Tools named in the brief but not read: Obico, 3DPrinterOS, gSender, LaserGRBL (no source in
`reference/`, and the tools above already cover their tasks); Mainsail and Fluidd themselves (their
rights come from Moonraker, read above).

## 4. Ranked improvements

Impact: how many people, how often, how bad. Effort: S (a page), M (several pages or one component
everywhere), L (a new page or API).

### 4.1 For the user (member)

| Rank | Improvement | Fixes | Impact | Effort |
|---|---|---|---|---|
| 1 | Draw every control from the declared verbs, the current state and the person's actions; a control the person may never use is hidden, one the state forbids is disabled **with its reason written under it**, not in a tooltip. | U1, U2, U7 | high | M |
| 2 | One value component that tells unknown (with "last at 13:12" where there is one), not applicable and a real zero apart; hatched bars for unknown progress. | U3 | high | S |
| 3 | An error state on every page: what failed, what is stale and since when, retry; SSE reconnect with `Last-Event-ID`. | U4, U10 | high | M |
| 4 | Phone navigation: four items (Now, Machines, Files, Materials) and More; the laser's job as numbered steps on one tab. | U5 | high | S |
| 5 | Ask for permission with a pending state (date and time), "asking grants nothing", withdraw, and a read-only "My permissions". | U8 | medium | M (needs an API read, see ui.md) |
| 6 | Six machine tabs by subject; people's words (Live, Control, Jobs). | U7, U9 | medium | M |

### 4.2 For the admin

| Rank | Improvement | Fixes | Impact | Effort |
|---|---|---|---|---|
| 1 | Requests: **Grant…** opens the grant form for that person and machine (preset with its actions shown, damaging actions as separate switches, expiry), then marks the request granted. | A1, A10 | high | M |
| 2 | Admin-only entries (Add machine, Space, Instance) only in an admin's navigation; a member who reaches the URL gets a refused page that says why. | U6 | high | S |
| 3 | Add machine as numbered steps: kind, point it here (with the hostname length warning), the code with its expiry, then "who may use it" linking to Space. Only v2 kinds. | A4, A5 | medium | S |
| 4 | One state colour list for everything; kind shown as a word, not a colour; house colour tokens. | A3, A7 | medium | S |
| 5 | Machine Settings split into what changes often (name, labels, PPE) and the rare (retire); moving a device on the instance view only. | A2, A6 | medium | S |
| 6 | Wall screen read-only (no buttons). | N3 | medium | S |

## 5. What goes into the design

All of section 4 is carried into [design/ui.md](../design/ui.md) as the target, with my own choices
marked "proposal". The open points this study found (a member reading their own requests, the
machine tabs against the wiki's Data tab, the laser hand-off when a job is already loaded, the wall
screen's session, "my spools") are listed in ui.md under *Open points of the UI*.
