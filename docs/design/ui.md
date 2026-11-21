# Web UI

The target UI, seen from two sides: the **user** (a member, at the machine, often on a phone) and the
**admin** (a space admin or owner, at a desktop), with the instance admin as a small third view. The
study behind it, with today's findings and the comparable apps, is
[plans/ui-ux-study.md](../plans/ui-ux-study.md). Clickable mockups of every screen in both roles:
`docs/design/ui/mockups/index.html` (static HTML, one stylesheet, opens from disk).

Choices of the UI designer that no ruling or reference made are marked **proposal**. Roles and
actions are those of [authz.md](authz.md) (the action and preset names decided in
[decisions/20261119-actions-and-presets.md](decisions/20261119-actions-and-presets.md)); verbs those of the
[capability verbs registry](../registries/capability-verbs.md). Where an action has no name yet, it
is written as api.md writes it: `(OQ-4) <area>`. The UX test of the mockups (journal:UI-TEST:0004,
46 findings) is worked into this page and the mockups; its ids (UT-nn) are named where a rule comes
from it.

## On the public API

Server-rendered pages, no SPA ([decision](decisions/20261118-ui-server-rendered.md), plan §2.9). The
HTML shell is rendered by the hub from `askama` templates (compile-time checked); every live read a
page does and every change it makes goes through `/api/v1` and `/api/v1/events`. No page route changes
data. An SPA is reconsidered only for the multi-tenant cloud if page weight or offline use calls for it.

## Rules for every page

- **One subject per page or tab**; no long stacked pages.
- **Controls from the device:** a control is shown because the device declared the verb and the
  current state accepts it, never because the page knows the brand.
- **States** (UI study): every page shows its loading, empty, error, refused and partial states;
  unknown is shown as unknown, never as zero; a disabled control says why.
- **Permissions only in Space:** no page outside Space grants an action; asking for permission (#11)
  grants nothing.
- **Our own limits** (what a person may press, bounds on our side) live here and in host-side
  validation, never on the wire.
- **Laser:** Start burn only on the laser's own page, by a person, after placing, framing and PPE.
- **Plain words** (UT-24): the page's text names no verb id, action id or raw state enum
  (`CLOSED`, `OUT_OF_PAPER`); a reason says the state in words and when the control works again. The
  mockups carry the verb, action and call in a separate annotation (`.anno`, "spec: …") that is not
  the product's text.
- **A command answers on the spot** (UT-04, flows.md §3): a pressed control turns *pending*
  ("Sent 15:56:02. Waiting for Laser-A to answer.") and keeps the label, so a second press is not
  sent; on `command.settled` its status line (`aria-live`) reads *done*, *refused* (the hub's or the
  device's reason in words; the `domain/REASON` code stays in the record) or *outcome unknown* (the
  hub restarted before the answer; never sent again: "look at the machine"). Stop gets this first.
- **Unreachable** (UT-05): when the hub cannot be reached, every page shows one error banner with the
  time of the last answer; on a machine's page its controls are disabled with "Cannot reach the
  machine: use its own controls", and on a laser "use its own emergency stop or its power switch".
  The same holds when the hub answers but the machine does not (offline).
- **Ask first, or offer Undo** (UT-44): an action that cannot be taken back asks once, naming the
  thing and the machine (Stop a print, Delete, Retire, Remove a member, Revoke, Refuse a request,
  Withdraw a request, Bad print, take a laser job off, Start burn); one the API can take back acts at
  once and offers Undo (take a queue item out, archive a spool).

How a control decides what to show (**proposal**, from the study's U1, U2, U7):

| The person … | The device … | The control is |
|---|---|---|
| never holds the verb's action on this device | declared the verb | **hidden**; one banner on the page says what the person may not do and offers Ask for permission |
| holds the action | declared the verb, the current state does not accept it | **disabled, with the reason written under it** (the state's name and when it is accepted), never only a tooltip |
| holds the action | declared the verb, the state accepts it | enabled |
| any | did not declare the verb | absent: no "not supported" button |

Exception, **proposal**: Start burn (`mc.laser.start`), when the person lacks it, stays visible and
disabled with "granted by name" as its reason, so a person on a laser knows where Start burn is and
why it is grey (the refused state). A printer's terminal (`device.gcode`) follows the first row: it
is shown only to a person who holds it (UT-03; a laser has no console at all). Values: a value without a basis reads *unknown*, with
the last known value and its time beside it when there is one; a real zero reads `0`; a value that
does not apply is left out, not dashed.

## Roles and what they see

| Navigation item | Guest | Member | Host (the space's label) | Admin, owner | Instance admin |
|---|---|---|---|---|---|
| Now | yes | yes | yes, plus requests | yes, plus requests and every machine's trouble | as in a space they belong to |
| Machines, a machine's page | view only | yes; machines they may not use say so | as member | yes, plus Add machine | — |
| Jobs | yes | yes | yes | yes | — |
| Queue and farm | the queue and projects, read only | the queue (move and take out their own items); projects and parts (`farm.use`) | every queue item (`queue.manage`); projects and parts | every item; a part's count correction (`farm.manage`) | — |
| Files | view (`files.view`) | yes, their own | as member | yes, all | — |
| Materials | view | add, use, weigh, label, edit, archive; locations (`stock.use`) | as member (`stock.use`; never `stock.manage`, [OQ-41](risks-and-open.md#oq-41-the-host-role-open-points)) | plus import, export, extra fields, the Spoolman backup (`stock.manage`) | — |
| Cameras | the ones they may view | the ones they may view | as member | plus manage in Space → Cameras | — |
| Prepare | — | yes | yes | yes | — |
| My permissions (user menu, read only, **proposal**) | yes | yes | — (they see Space → Permissions, `grants.view`) | — (they see Space) | — |
| Space (tabs) | — | — | Members (read only), Permissions → Requests and Grants (no damaging switches) | yes | — |
| Instance (user menu) | — | — | — | — | yes |

The rows are the `makerspace` defaults; a space's [area rights](authz.md#area-rights-per-space) move
an area's `.view` and `.use` between roles (for example Materials' `stock.use` to host or admin), and
the navigation follows the person's actions from `GET /api/v1/me`, not the role's name.

- Lists, counts and the event stream show only devices in the person's `device_access` with
  `device.view`; nothing is filtered after fetching ([authz.md](authz.md#the-materialised-table)).
- "Guest" is the existing role that looks only; its columns follow from `device.view` alone.
- **Desktop** (≥ 900 px, **proposal**): a sidebar grouped as the wiki's *Design: The Website* has it
  (Now; Work: Machines, Jobs, Queue and farm; Stock: Files, Materials; Watch: Cameras; the machines,
  live; at the bottom the user menu with Space, Instance, sign out).
- **Phone** (**proposal**): a bottom bar of four (Now, Machines, Files, Materials) and **More** for
  the rest; the machines list is the Machines page.
- Admin-only entries are absent from a member's navigation; a member who opens such a URL gets the
  refused page ("This page is for the space's staff") and the hub refuses the calls too.
- The **role switch** "View as: Guest | Member | Host | Admin | Instance admin" exists only in the
  mockups (UT-26); Host (Mira) shows the space's label for the role ("Staff" in the mockups). Guest (Eli) looks only: no control that changes something is shown. Instance admin (Ada)
  is also the space's owner and the organisation's owner; the Instance entry shows for an instance
  admin only, never for every admin.

## Pages

| Page | Subject | Reads | Issue | Mockup |
|---|---|---|---|---|
| Sign in | name and password; the invite page; first-run setup | `POST /api/v1/session`, `/api/v1/invites/accept`, `/setup` | | `signin.html`, `invite.html`, `setup.html` |
| Now | the space at a glance; the wall screen | devices, jobs, queue, events | | `now.html`, `wall.html` |
| Machines | the device list | `GET /api/v1/devices`, events | | `machines.html` |
| Machine | one device: a head (picture, name, state, live chips, Set ready) and tabs per subject (Now, Queue, Controls, Files, Cameras, Machine, History, Advanced) | device, commands, history, transfers | | `printer.html` |
| Start a print | the "print on this machine" sheet | files, `transfers`, `queue` | | `print-start.html` |
| Laser | a laser's page: the loaded job, frame, place, PPE, Start burn | device, commands, `ppe`, `jobs` | HS7b | `laser.html` |
| Tape printer, label printer | one page per kind, never shared | device, commands | #6 | `label-tape.html`, `label-labels.html` |
| Add machine | pairing by code; per kind: the Buddy INI with the hub's address (warning when the address does not fit the dialect's hostname limit, 35 bytes for Buddy 6.4.0, `reference/buddy-6.4.0` `src/connect/hostname.hpp:16`), or the agent's download and start command | `POST /api/v1/devices/pair` (proposed path, [OQ-1](risks-and-open.md#oq-1-exact-paths-and-fields-of-our-own-endpoints)) | #22 | `add-machine.html` |
| Files | uploads, what a file says about itself, Print on, Send to a laser | files, devices | | `files.html`, `state-empty.html` |
| Queue | per device, order, take out | queue | | `farm.html` |
| Prepare | the tools that make the work: laser design (the LaserFoundry web fork; picks a hub laser, its settings from the hub), the label editor | `/api/v1/lasers`, devices | #6, #24 | not mocked |
| Jobs | job history with filters | `GET /api/v1/jobs` | #11 | `state-loading.html` |
| Farm, projects | projects, parts, dispatch | farm endpoints | | `farm.html#projects` |
| Materials and stock | Spoolman's data with our UI; tapes and filaments apart; laser materials | Spoolman tables, locations, items | #10 | `materials.html`, `spool.html` |
| Cameras | watching: the space's cameras, the player | cameras, `/ws` | #5 | `state-error.html` |
| Space | tabs: General, Area rights, Members (with invites), Permissions (grants, requests, machine groups), API tokens, Audit log, Cameras (manage), Access (doors) | space endpoints | | `space-members.html`, `space-permissions.html`, `space-settings.html` |
| The agent's page | on the agent's computer (:8470), no sign-in: Devices (each device, its link to the hub, registration by code; the hub's address) and About (the agent, the computer, its load); the hub's components only | the agent itself | #67 | `agent.html` |
| Instance | organisations and spaces; moving a device; the hub's update | `GET /api/v1/instance/spaces`, `/api/v1/update` | | `instance.html` |
| About | build, update, last update's outcome, dumps | `/api/v1/update` | | on `instance.html` |

## Per page: what the user sees, what the admin sees

Each control below names its verb or call and the action it needs. A verb's action comes from the
registry; a state that does not accept the verb disables it with the reason.

### Now

- **User:** four numbers (printing, free, waiting for someone, jobs done today; a laser with a job
  loaded is not free); *Needs someone* (waiting for a sign-off, attention such as out of labels,
  error, stopped, paused, offline; most urgent first; "All clear" when empty; UT-15); *Ends next* by
  the clock ("45 min left · done 16:35"); the fleet as cells, each with its state's letter and a
  name for a screen reader, with a legend (UT-17).
- **Admin:** the same, plus requests for permission as rows in *Needs someone* (linking to Space →
  Permissions → Requests, granting nothing here) and the projects' progress.
- **Controls:** Set Ready, Bad print (asks first) → `POST /api/v1/jobs/{id}/confirm` `{good}`,
  `job.control` on that device; Look → the machine's page.
- **Wall screen** (decided, journal:UI-DESIGN:0009 #4; UT-25): its own page with a large clock,
  **read only: no buttons and no acting links**, so a passer-by at the TV cannot act as anyone (study
  N3); person-only verbs need a person's own session, which a wall has not. Which session the screen
  keeps is open (UI-4).
- **States:** loading (frame at once, cells grey), empty ("No machines yet": staff get "Add machine",
  members get "Ask the space's staff"), error (banner with the time of the last answer), unknown
  (cells dashed).

### Machines

- **User:** filter chips with counts (All, Printing, Free, Job loaded, Needs someone, Attention,
  Error, Offline; the counts add up to All, UT-35), search, group by; one card per machine with its state, one primary action and More ⋯. A machine the person
  may not use reads "You don't have permission to use it" with **Ask for permission**.
- **Admin:** the same, plus **Add machine**, and one banner per kind of trouble ("2 requests for
  permission wait: Review in Space").
- **Primary action per card** (**proposal**, after prusa-connect-ui-study.md): printing → Pause
  (`core.command.pause_print`, `job.control`, a real command button, UT-13); waiting for a sign-off →
  Set Ready; free printer → Print (opens Start a print); laser → Go to the laser (never a start);
  tape or label printer → Print. A laser with a job loaded shows the tag *Job loaded*, not a state.
- **The card** (owner, 2026-11-20: "neem die van prusa als voorbeeld"; after Prusa Connect's printer
  card, in our own drawing and words): the machine's picture (its profile's drawing, else its kind's),
  its model small over its name, its state on the right; the job line (file and %, its bar, time left;
  else "No print running"); for a 3D printer three boxes: nozzle and bed (now / target, the target
  only while one is set) and the material (swatch and name, "–" without); a message with its tag when
  something needs someone or is wrong; then the actions.
- **States:** a card offline has a dashed outline (the state list's rule, UT-36) and shows its last
  report's time; values are unknown, not the last ones shown as current.

### A machine's page (3D printer)

Laid out after Prusa Connect's printer page, the online one's head and the local one's tabs, in our own
code and words (owner, 2026-11-20: "layout grotendeels van Prusa Connect local", then "Mengen";
journal:HUB-FINISH:0006). This replaces the six tabs of UI-2.

**Head**, above the tabs on every tab: the machine's picture; on one line its name, its state and, for a
printer, its live values in the state's own small
style (owner, 2026-11-20: "inline met idle in dezelfde stijl", "met icons"), each with its icon and its
name as label, a flame beside a heater while it heats; for who may operate it, nozzle and bed open a menu to set
them as Prusa Connect's head does (Off, the firmware's preheat presets PLA … PA, a value of one's own): nozzle and bed (temperature / target,
the target only while one is set), speed %, flow % and its material (the spool from Materials, else what
the printer says; a link to Materials until assigning is built there), and at its far right a pencil to
rename it (to the Machine tab); below one muted line: what it is
and how it is connected, its firmware. On the right **Set ready**: enabled while a finished print is on the bed (the print is taken),
then *Ready · undo* until a job starts; otherwise disabled with its reason (the control rule).

Tabs: Now, Queue, Controls, Files, Cameras, Machine, History, Advanced; a tab is shown only when the
device has its subject (Queue not for a label printer; Files only for a device with storage; Cameras
only with a linked camera the person may view). Old addresses (`#overview`, `#control`, `#jobs`,
`#camera`, `#settings`, `#telemetry`, `#terminal`, `#bedlevel`) lead to their new tab.

| Tab | User | Admin |
|---|---|---|
| Now | the printer's question and its buttons; the camera beside this print (file, by whom, progress, time left and the clock); the queue's next items beside the recent jobs | the same |
| Queue | the device's queue (move and take out own items), its past jobs | every queue item |
| Controls | the declared verbs they hold, enabled when the state accepts them, one card: moving and tools left, temperatures, speed, flow and nozzles right; below it the bed level (Probe bed and the mesh, as Prusa Connect's Utilities → Calibration; owner, 2026-11-20) | the same, with their own actions |
| Files | files on the device; Print… | plus Delete (`core.command.delete_file`, `job.control`) |
| Cameras | the linked camera (`device.view`) | the same; managing is in Space → Cameras |
| Machine | first **Details** in two columns, Specification (from its profile) and This machine (what it told); then read only: name, model, firmware label, serial, connection; "changed by the space's staff" | name and labels, induction, PPE list (`PATCH /api/v1/devices/{id}`, `device.manage`); Retire with a reason (`DELETE`, history stays) |
| History | temperatures and fans over the last 2 hours (Prusa Connect's span); the commands sent from the hub and how each went | the same |
| Advanced | the terminal (holder only, UT-03) | the same |

Control tab, a 3D printer (verbs of the `marlin` registry; the `prusa` kind uses the same
`core.command.*` names where its driver declares them):

| Control | Verb | Action | Accepted in (marlin) |
|---|---|---|---|
| Jog X, Y; Z; extrude | `core.command.move`, `move_z`, `move_e` | `device.move` | `OPERATIONAL`, `PAUSED`, `TRANSFERING_FILE` |
| Home | `core.command.home` | `device.move` | same |
| Motors off | `core.command.disable_steppers` | `device.move` | same |
| Nozzle, bed target | `core.command.set_nozzle_temperature`, `set_bed_temperature` | `device.heat` | operational and printing states |
| Fan on, off | `mc.printer.fan` | `device.heat` | same |
| Speed, flow | `core.command.set_speed`, `set_flow` | `job.control` | same |
| Pause, resume, stop | `core.command.pause_print`, `resume_print`, `stop_print` | `job.control` | the job states |
| Terminal | `mc.printer.gcode` | `device.gcode` (by name) | as declared |
| Refresh the SD list | `mc.printer.sd_list` | `device.view` | `OPERATIONAL`, `TRANSFERING_FILE` |

Bounds of a number come from the verb's declared argument (kind, unit, bounds); the page adds none
of its own (for a Prusa printer the firmware's limits rule, owner 2026-11-09).

**States:** printing disables movement with "Not while printing: moving works when it is ready or
paused"; offline shows every live value as unknown with its last time; a print whose end was not
observed reads "Outcome unknown", never good or failed. A member who lacks `device.move` and
`device.heat` sees no Move and Heat cards, only one banner with Ask for permission (UT-12). History
outcomes (done, stopped, failed) are tags, not state chips (UT-16).

### Start a print

A sheet from the machine and its card (**proposal**, after prusa-connect-ui-study.md §3). Two sources,
as tabs: the hub's files and the files on the printer. A file says what it is for (model, nozzle,
material, time); a file for another model is listed but cannot be picked, with its reason.

| Control | Call | Action |
|---|---|---|
| Print now on MINI-1 (hub file, Prusa) | `POST /api/v1/devices/{id}/transfers` `{file_id, path, print_after: true}` | `job.start` |
| Print now (hub file, agent device) | `POST /api/v1/devices/{id}/jobs` `{file_id}` | `job.start` |
| Print now (file on the printer) | `core.command.start_print` `{path, origin}` | `job.start` |
| Add to MINI-1's queue | `POST /api/v1/queue` `{device_id, file_id}` | `queue.manage` and `job.start` |
| Put it first (admin) | `PATCH /api/v1/queue/{id}` `{pos}` | `queue.manage` |

The button always names the machine. While the machine is busy, Print now is disabled with the reason
and *Add to the queue* becomes the primary button.

### The laser

A laser's page has a printer's layout (printer.html, laser.html): the head with X, Y and the laser's
power as chips, and the tabs Now, Queue, Controls, Cameras, Machine and History (no Files, no
Advanced: a laser has no console). **Now** carries the laser flow as numbered steps in the **Loaded
job** card, beside the camera, and below them what waits in its queue beside its last jobs; while it
burns, the card is the job's progress. **Controls** is one card in two columns: Coordinates (X and Y,
the work area, the pad with its step and speed, Home) and Origin and Machine (set origin, go to origin,
unlock, read settings); the Loaded job card below it. The flow is the owner's HS7b and the device
contract's laser rule; the order is the owner's "prepares, frames, PPE, presses Start" and flows.md §4
(origin, then frame), decided in journal:UI-DESIGN:0011 Q2 (UT-08):

1. **Loaded:** one job per laser, sent from Files or from Prepare's "Send to Laser-A"
   (`POST /api/v1/devices/{id}/jobs`; a laser job is loaded, not started). Take it off asks first;
   its path is open ([OQ-1](risks-and-open.md#oq-1-exact-paths-and-fields-of-our-own-endpoints), UI-6).
2. **Place it:** `mc.laser.jog`, `mc.laser.set_origin` (`device.move`). A Set origin undoes a frame
   done before it; a jog does not.
3. **Frame it, laser off:** `mc.laser.frame` with `power: 0` (`device.move`), so the frame shows the
   placed job. The page offers **no power field** (**proposal**): a frame above 0 is person-only in
   the contract and is not offered here.
4. **Protect yourself:** the laser's PPE list as boxes; Confirm → `POST /api/v1/devices/{id}/ppe`
   `{items[]}`, kept for the session. Shown when the space's `ppe_checks` is on. A missing tick is
   said in a live status line (UT-22).
5. **Start burn:** `mc.laser.start` (person-only, damaging, granted by name), from the person's own
   session, after a frame of the loaded job after the last Set origin and the PPE. It **asks once
   more** (UT-09): the dialog names the job, the laser, the origin and frame times and the PPE
   confirmed, and says to stay at the laser; its button reads "Start burn on Laser-A".

**Stop** (`mc.laser.stop`, `job.control`: a soft reset, the laser off) sits in a bar that sticks under
the top bar on every tab, so it never scrolls away (UT-01). The queue never starts a laser; a token
never does.

The Loaded job card's states (UT-02, from GRBL's states and the job flow; the device's state is said in
words):

| State | What the Loaded job card shows |
|---|---|
| Ready | the five steps, the current one marked |
| Burning (GRBL `RUN`) | progress, time left and the clock, who started it; Pause; "stay at the laser"; steps 1-4 closed: the running job cannot be taken off, placed again or framed |
| Lid open (`DOOR`) | Attention: the laser holds and is off; close the lid, then Resume or Stop |
| Alarm (`ALARM`) | Error: it stopped, the laser is off; Unlock or Home; Start burn disabled ("accepted when idle") |
| Ended | outcome *done*, how long it took; no job loaded any more: send it again to burn it again |
| Stopped | outcome *stopped* (or *failed* when it ended with a fault), by whom; no job loaded |
| Hub unreachable, laser offline | the error banner; Stop disabled with "use the laser's own emergency stop or its power switch" (UT-05) |

**Sending to a laser with a job loaded** (decided, journal:UI-DESIGN:0009 #3, UI-3; UT-10): Files and
Prepare ask "Laser-A has sign-oak loaded: replace it?", naming who loaded it and when; on Replace
the old job goes off the laser (it stays in Files) and the new one must be placed and framed. While
the laser burns, Send is disabled until the job has ended (the job flow has no replace from
running).

- **User:** with `mc.laser.start` granted, Start burn is enabled once steps 2-4 are done; until then it
  says which step is missing. Without the grant: the refused state (Start burn disabled, "granted by
  name after the laser course", the person's request for it and its state).
- **Admin:** the same rule. An admin without the grant sees the refused state with "Grant it in Space →
  Permissions, your own included" (being owner is not having had the laser course).
- **No console:** a laser offers no raw G-code (`mc.laser.gcode` is not declared, device contract;
  [decision](decisions/20261119-no-raw-gcode-on-a-laser.md)): nothing but Start burn
  switches the laser on (UT-03).

### Tape printer and label printer

Two kinds, two pages, nothing shared (the owner's ruling on the label stacks).

| | Tape printer (`labelmanager`) | Label printer (`labelwriter`) |
|---|---|---|
| Print | `mc.label.print` `{picture, tape_size_mm}`, `job.start` | `mc.label.print` `{picture, page_size}`, `job.start` |
| Accepted in | `IDLE` | `IDLE` |
| Disabled reasons (in words) | not connected (`DISCONNECTED`): "plug it in at the agent's computer"; printing (`BUSY`) | out of labels (`OUT_OF_PAPER`, the state list's **Attention**, listed in Now's *Needs someone*, UT-15): "load a roll"; printing; error |
| Sizes offered | the tape sizes the port declares | the rolls of the label catalogue (data) |

The picture is made by the label editor in Prepare (#6;
[OQ-14](risks-and-open.md#oq-14-tables-of-materials-items-and-labels)); the print page only picks a
picture made there and prints it, with no text editor of its own (UT-39). Users and admins see the
same print page; Settings is read only for a member.

### Materials and a spool

After Prusa Connect online's Materials (owner, journal:HUB-FINISH:0006; materials.html): the stock in
kg and spools on top; **Filament** is one card with search, sort and filter chips (all, on a printer,
low, a type, a brand, a location) over a table with **one row per material** (colour, name, brand,
type, its spools and how many are on a printer, what is left with a bar), each with **Assign…** (a
spool of it onto a printer, a dialog) and **+ Spool**; its spools open under the row.
**Low** means one thing: less than 15 % of a spool or a material left, the mark the bar already
had (ours, UI only; owner, 2026-11-20: "15 % is goed"). A material's own minimum ("Keep at least") reads *below its minimum*, never
"low". Assign offers the printers a spool fits, as moving a spool already does (filament to a 3D
printer, tape and labels to a label printer); laser materials are not assigned.
Tabs: Filament, Tapes, Label rolls, Laser materials, Locations. Tape cassettes and label
rolls are two tabs, as the two stacks are apart (decided, journal:UI-DESIGN:0011 Q4; UT-38). Data is
Spoolman's (`/spoolman/api/v1`).

| Control | Call | User | Admin |
|---|---|---|---|
| Subtract used grams (by hand) | `PUT /spool/{id}/use` | yes | yes |
| Use the scale's weight | `PUT /spool/{id}/measure`, enabled when the scale reports `STABLE` | yes | yes |
| Print a label | `mc.label.print` on the chosen printer | yes | yes |
| Add, edit, archive a spool; filaments; locations | Spoolman and `/api/v1/locations`, `stock.use` | yes | yes |
| Import, export, extra fields, the Spoolman backup | Spoolman, `stock.manage` | — | yes |

Adding a spool is a member's day-to-day work (`stock.use`, authz.md; UT-14: the design's action list
wins over this page's earlier "admin only"). A spool never weighed reads *unknown*; an empty one
reads `0 g`. A **Mine** filter needs an owner on a spool and is not shown until that is decided
(UI-5, [OQ-14](risks-and-open.md#oq-14-tables-of-materials-items-and-labels); UT-37).

### Ask for permission

- **User:** on a machine they may view but not use: one banner ("You don't have permission to use
  Printer-3", or "Printer-3 needs a training") with **Ask for permission** / **Ask for a training** →
  `POST /api/v1/access-requests` `{device_id}`. Then the **pending** state: "Asked on 19 Nov at
  14:20. The space's staff decide in Space. Asking grants nothing." with **Withdraw** (asks first;
  `DELETE /api/v1/access-requests/{id}` while open). The outcomes (UT-28): **refused** ("Not
  granted … you may ask again"), **granted** (the machine's controls appear), **withdrawn**. The
  member's own requests and their states are listed under My permissions
  (`GET /api/v1/access-requests/mine`, decided journal:UI-DESIGN:0009 #5). Control and Files stay
  hidden while not granted.
- **Operate:** on a machine they may use but not operate, Controls is that one banner ("You may not
  move, heat or send G-code to Printer-3") with **Ask for permission**, a request for `operate`
  (`{device_id, level: "operate"}`); its pending state the same. A request names its level (`use`
  or `operate`); the staff see "asks for permission" or "asks to operate", and **Give** gives that
  level on that machine.
- **Admin:** the machine's page shows "Ravi asked … Review in Space"; there is **no grant button on a
  machine's page**.

### Add machine (admin)

Numbered steps (**proposal**, after prusa-connect-ui-study.md): 1 what are you adding (a printer
with built-in Connect; a machine on USB through the agent; a network camera → Space → Cameras);
2, per kind (UT-07): for built-in Connect the INI's lines and the hostname length warning; for USB
the agent's download for the computer beside the machine and its start command; 3 the code it
shows, with the name here → `POST /api/v1/devices/pair` `{code, name}` (`devices.manage`); after Add: *pending* until the device collected its token
(flows.md §1), then *paired* with a link to the machine; 4 "who may use it": a link to Space →
Permissions. Only kinds of the [device contract](device-contract.md#kinds) are offered.

**A machine on USB:** the picker offers the agent builds in the hub's downloads folder (beside its
database, [storage-ops.md](storage-ops.md); named as `scripts/build-release.sh` names them), served at
`downloads/{name}` (`devices.manage`); the start command is a plain run; then the agent's own page
(:8470) takes this hub's address, from the page's `Host`, and shows the code. Without a build in the
folder, the step says so.

- **Errors** (shown only when they happen): "That code is not waiting here. It may have expired: the
  device shows a new one."; "The machine did not collect its token" (added, not reported yet).
- **User:** the refused page.

### Space (admin)

Tabs, one subject each: **General** (space name, organisation name, PPE checks, the host role's
label as free text, empty reads "host": `PATCH /api/v1/spaces/{id}`, `PATCH /api/v1/organisations/{id}`), **Area rights** (below),
**Members** (roles, remove, invites:
`space.members.manage`), **Permissions** (Grants, Requests, Machine groups), **API tokens**, **Audit
log**, **Cameras** (manage), **Access** (doors: their own module,
[OQ-10](risks-and-open.md#oq-10-door-access-link)).

Permissions → Grants: pick a person and a device or device group; a **preset** (View, Use, Operate,
Manage, with their contents as authz.md lists them, shown from the hub);
**damaging actions as separate switches by name** (`mc.laser.start`, `device.gcode` on a 3D printer
only, `device.firmware`), never in a preset; an optional expiry; an optional note (an induction is
recorded as a grant with a note, for example "laser course given"); who granted and when →
`POST /api/v1/grants`. The admin's own
row is set here like anyone's: no other route and no "give myself" button, and no extra rule that
locks the admin out of their own row.

Area rights ([authz.md](authz.md#area-rights-per-space),
[decision](decisions/20261119-host-role-and-area-rights.md),
[OQ-42](risks-and-open.md#oq-42-area-rights-open-points)): the space's kind (makerspace, office,
private) and per module area (Files, Queue, Farm, Materials) a row with its `.view` and `.use`, each a
select of the lowest role that holds it → `PUT /api/v1/spaces/{id}/area-rights` (`space.manage`, a
person's session only). A `.view` select offers guest, member, the host label, admin; a `.use`
select offers member, the host label, admin (a guest never uses) and none below that area's
`.view`. The page says what never moves: the `.manage` actions, every device action, and the
member list, grants and audit log views; and that a change is in the audit log. Save asks first,
listing each change in words ("Materials, use: member → host"). The kind is a select of its own with
Save kind (`PATCH /api/v1/spaces/{id}` `{kind}`); the page says that a new kind changes no right:
its defaults apply only when a space is made.

Permissions → Requests: each request with **Grant…** (opens the grant form for that person and
device; after saving, `PATCH /api/v1/access-requests/{id}` `{state: granted}`) and **Refuse…**
(asks first).

Members (UT-27, decided journal:UI-DESIGN:0011 Q1): the role select lists guest, member, host (shown
with the space's label, for example "Medewerker (host)"), admin, owner, and offers only roles up to
the caller's own effective role (`authz/ROLE_ABOVE_OWN`); "owner" is offered only by an owner; a member above the
caller and the last owner are greyed out with their reason (`authz/LAST_OWNER`;
[OQ-39](risks-and-open.md#oq-39-roles-above-ones-own-and-the-last-owner) is open). An invite for the
whole organisation gives at most the caller's organisation role (`authz/ORG_ROLE_NEEDED`). The invite
link is shown in full once, with Copy (UT-45); renaming the organisation needs an organisation role
(`organisation.manage`).

API tokens (UT-06): a name, an end date and **what it may do**, an action set picked in words
(see machines, start jobs, files, queue, stock …). Not offered, because a token can never use them:
Start burn and a frame with the laser on (person-only), and granting (`grants.manage`, a person's
session only). The page says that a token never does more than its person may at the time of each
call ([decision](decisions/20261119-token-effective-rights.md)). After Make token the secret
(`mct_…`) is shown once, in full, with Copy and "If it is lost, revoke it and make a new one".

Audit log (UT-45): filters (who, what, from, to) and **Older** for the next page (keyset paging,
`GET /api/v1/spaces/{id}/audit`). Cameras (UT-30): the list with each camera's state (online,
offline), Edit, Show live (the test after saving) and Remove…; Add a network camera
(`POST /api/v1/cameras`, `cameras.manage`).

- **Host:** Members read only; Permissions → Requests and Grants, where the grant form offers no
  Manage preset and no damaging switches (a host's grant holds neither `device.manage` nor a damaging
  action, "an admin grants these"), not the host's own row, and Remove only on a grant the host
  could have given; the other tabs are the refused page.
- **User:** the refused page; their own permissions are in **My permissions** (read only, from
  `GET /api/v1/me` `actions`, **proposal**).

### Queue and farm

- **User:** the timeline per machine and the queue in order; move and take out their own items (Take
  out offers Undo); why an item waits, in words ("nothing with PETG loaded"); a laser row reads "never
  from the queue"; **Projects** (projects, parts, their counts, why a part waits; `farm.use`).
- **Admin:** every item; a part's count correction (`farm.manage`).
- Calls: `GET`, `PATCH`, `DELETE /api/v1/queue/{id}` (`queue.manage`; own items for their person);
  `/api/v1/projects`, `/parts` (`farm.view`, `farm.use`).
- The timeline's rows shrink and wrap on a phone; the page never clips what overflows (UT-33).

### Instance (instance admin)

Organisations and their spaces (`GET /api/v1/instance/spaces`, `instance.admin`); every agent (its devices grouped by
`agent_id`, [extension part 6a](prusa-sdk-extension.md#6a-who-the-agent-is-and-what-each-side-knows)) with its release, computer, connection, last seen, devices and whether it fits
the hub, and its update ([decision](decisions/20261119-agent-update-by-instance-admin.md)); the hub's update
(`GET`, `POST /api/v1/update`) with a confirm that says a dump comes first and what the way back
loses, and its outcomes on About: in progress, done, rolled back, refused (UT-29, flows.md §8);
moving a device to another space ([decision](decisions/20261119-device-move.md)): a confirm that
says what stays and what is removed, refused with "its print has not ended" (`device/JOB_OPEN`);
its path is open (OQ-1). A new space is made here only by someone with an organisation role
(`POST /api/v1/spaces`, `organisation.manage`); a new organisation has no route (UI-7). A space admin
who is not an instance admin gets the refused page.

## The visual system

### House colour

The house colour is **`#93A682`** (owner, 2026-11-19, [decision](decisions/20261119-house-colour.md)).
Measured contrast (WCAG 2.x relative luminance):

| Pair | Ratio | Use |
|---|---|---|
| `#93A682` on white `#ffffff` | 2.62:1 | too low for text or for a control's only boundary |
| dark ink `#1d1d1b` on `#93A682` | 6.45:1 | text on an accent fill |
| `#93A682` on dark background `#111214` | 7.16:1 | accent as text in dark mode |
| `#93A682` on `#f6f7f5` (light page) | 2.44:1 | too low |

The text shade on light backgrounds ([OQ-16](risks-and-open.md#oq-16-the-house-colours-text-shade)) is
decided as `#5E6B52` (journal:UI-DESIGN:0009 #2): (same hue, HSV value lowered from 0.65 to 0.42): 5.28:1 on the light page
`#F6F7F5`, 5.67:1 on white. The lightest shade of that hue that passes 4.5:1 on the page is
`#68765C` (4.51:1), which leaves no margin. The earlier candidate `#697C58` reaches only 4.23:1 on
the page.

### The motif: one cut corner

The house style is the wiki page *Design: House Style* (owner, 2026-11-19); this section applies it.
**One 45° chamfered corner, top right; every other corner is square** (no rounded corners anywhere;
only a state's dot and a step's number stay round, as marks).

| Element | Cut | Token |
|---|---|---|
| Cards, panels (value tiles, steps, banners, the table frame, the laser's Stop bar, the phone's More menu, the empty state, the Undo toast), dialogs, camera tiles, the logo | 12 px | `--cut-l` |
| Buttons (and the theme switch) | 8 px | `--cut-m` |
| Chips: the state chip, the outcome tag, *unknown*, filter chips, label tags | 6 px | `--cut-s` |

- The shape is `clip-path: polygon(0 0, calc(100% - C) 0, 100% C, 100% 100%, 0 100%)`.
- **A shape with an outline is two layers**: an outer layer in the outline colour (border-box) and an
  inner layer in the fill (padding-box) whose cut is **C − 0.5858 × w** (w = outline width, 1 px). With
  the same cut on both, the diagonal comes out √2 times too thick. In `mc.css` the two layers are two
  backgrounds of the one element (the inner one a 225° gradient whose edge is the inner cut, blended
  over 1 px as anti-aliasing), so a scrolling frame (`.table-wrap`) keeps its shape. Measured in
  headless Chrome on a black-on-white test (sum of coverage across the edge): the diagonal is
  0.97 px at 1x, 0.99 px at 2x, 1.00 px at 3x and 4x against 1.00 px straight edges.
- A dashed outline (offline, unknown, the empty state, a pending command) is a 135° stripe on the
  outer layer, so the diagonal is dashed as well.
- A shape without an outline (filter chips, label tags, the logo, the toast, the primary button) is set
  apart by its fill alone.
- **The logo** (28 px square, 12 px cut): the letters *MC* (12 px, bold) sit bottom left with 3 px
  inset, clear of the cut; measured in headless Chrome, the text box's top right corner is 4.2 px clear
  of the cut's diagonal.
- **Focus ring:** `clip-path` cuts off an outline, so every cut control (button, chip, theme switch)
  sits in a wrapper `<span class="fw">` that draws the ring on `:focus-within`. Controls without a cut
  (links, tabs, inputs, selects, checkboxes, summaries) keep their own ring. A list that is itself a
  card keeps the card's side padding, so its cut never cuts a ring.

### Transparency

Transparency is used only where it shows something of what is behind:

- **Tints of the house colour at alpha** (`--tint-hover` 10 %, `--tint-sel` 18 %, `--tint-chip` 24 %,
  the wiki's values, decided): one token for light and dark. They replace the solid tints: the navigation's hover and current page,
  the fill of filter chips and label tags. A selected row also keeps a bar on its left, so selection
  never rests on colour alone.
- **The sticky header** is translucent (`--glass`, 88 %) with a 10 px backdrop blur. 88 % is decided:
  the wiki reference's 72 % fails contrast (it leaves `--line-strong` and `--muted` too low over a
  black picture).
- **The camera bar**: every camera tile carries a bar along its bottom edge over the picture
  (`.cam-bar`): the camera's name and **LIVE** (a `--st-bad` dot and the word), or for a stale picture
  *Last picture, 15:42* in words. Same `--glass` token and blur as the header. The header and this bar
  are the only two blurred layers (cost on the Pi and old phones).
- **Behind a dialog** a scrim of 50 % (`--scrim`; 60 % in dark).
- **The cut corner** shows the background behind it.

Never on text (a stale camera picture says *Last picture* in words; it is not dimmed), never to
disable a control (the muted text colour, contrast measured), never on status colours, warnings, Stop
or any laser control: those stay opaque.

Every translucent layer has an opaque version: where `backdrop-filter` is missing the header is
`--glass-solid`; under `prefers-reduced-transparency: reduce` every layer is replaced by its colour
over its usual background (the tints over `--surface`, the header and the camera bar `--glass-solid`, the scrim
`#848484` light, `#070708` dark) and the blur is off.

### Tokens

Every ratio below was computed with the WCAG 2.x formula (relative luminance, sRGB). Text needs
4.5:1, a control's boundary or a state's mark 3:1.

| Token | Light | Ratio (light) | Dark | Ratio (dark) | Use |
|---|---|---|---|---|---|
| `--bg` | `#F6F7F5` | — | `#111214` | — | the page |
| `--surface` | `#FFFFFF` | — | `#1B1C1F` | — | cards, bars |
| `--ink` | `#1D1D1B` | 15.71 on bg, 16.88 on surface | `#F2F1EE` | 16.59 on bg, 15.09 on surface | text |
| `--muted` | `#585C54` | 6.36 on bg, 6.83 on surface | `#A6A8A1` | 7.80 on bg, 7.09 on surface | secondary text |
| `--line-strong` | `#7D8278` | 3.66 on bg, 3.94 on surface | `#767A72` | 3.89 on surface | inputs' and buttons' boundary |
| `--accent` | `#93A682` | fill only (2.62 on white) | `#93A682` | 7.16 on bg, 6.51 on surface | buttons, the selected chip, the printing state |
| `--on-accent` | `#1D1D1B` | 6.45 on accent | `#1D1D1B` | 6.45 on accent | text on `--accent` |
| `--accent-text` | `#5E6B52` (proposal) | 5.28 on bg, 5.67 on surface | `#93A682` | 7.16 on bg | links, the selected tab's line, the focus ring |
| `--st-wait` (needs someone) | `#1E7B3B` | 4.95 on bg, 5.31 on surface | `#62C77E` | 8.92 on bg, 8.11 on surface | a waiting machine's mark and edge |
| `--st-warn` (attention) | `#8F5B00` | 5.33 on bg, 5.73 on surface | `#E3A948` | 8.96 on bg, 8.14 on surface | attention |
| `--st-bad` (error) | `#B3261E` | 6.08 on bg, 6.54 on surface | `#F2857A` | 7.51 on bg, 6.82 on surface | error; the danger button |
| `--st-info` | `#2F62B8` | 5.49 on bg, 5.90 on surface | `#86B3FF` | 8.84 on bg, 8.04 on surface | notes |
| `--st-off` (idle, offline) | `#6B6E66` | 5.19 on surface | `#9A9C95` | 6.14 on surface | free, offline |
| `--on-strong` | `#FFFFFF` | 6.54 on bad, 5.31 on wait | `#1D1D1B` | 6.76 on bad, 8.03 on wait | text on a state fill |
| `--tint-hover` | `rgb(147 166 130 / .10)` | see below | same | see below | hover in the navigation |
| `--tint-sel` | `rgb(147 166 130 / .18)` | see below | same | see below | the current page in the navigation (with its left bar) |
| `--tint-chip` | `rgb(147 166 130 / .24)` | see below | same | see below | filter chips, label tags |
| `--glass` | `rgb(255 255 255 / .88)` | see below | `rgb(27 28 31 / .88)` | see below | the sticky header, a bar over a camera image |
| `--glass-solid` | `#FFFFFF` | — | `#1B1C1F` | — | the opaque version of `--glass` |
| `--scrim` | `rgb(17 18 20 / .5)` | no text on it | `rgb(0 0 0 / .6)` | no text on it | behind a dialog |
| `--cut-l`, `--cut-m`, `--cut-s` | 12, 8, 6 px | — | same | — | the cut corner |

The focus ring is 2 px of `--accent-text` with a 2 px gap, so it is measured against the page
(5.28:1 light, 7.16:1 dark). No container clips it: tabs wrap instead of scrolling (UT-19, UT-46), and
on a cut control it sits on the wrapper (see the motif).

Pairs added by the house style. Text on a translucent layer is measured against the lightest and the
darkest background that can be behind it: for a tint `--bg`, `--surface` and `--surface-2` (the table
gives the worst, then the best); for `--glass` pure white and pure black, since any picture or page
can scroll under the header or show in a camera picture.

| Pair | Light | Dark | Use |
|---|---|---|---|
| `--ink` on `--tint-hover` | 13.73 – 15.53 | 11.62 – 14.38 | a hovered navigation link |
| `--muted` on `--tint-hover` | 5.55 – 6.28 | 5.46 – 6.76 | a hovered machine's state word in the navigation |
| `--ink` on `--tint-sel` | 12.94 – 14.54 | 10.08 – 12.43 | the current page's link |
| `--muted` on `--tint-sel` | 5.24 – 5.89 | 4.74 – 5.84 | the current machine's state word |
| `--ink` on `--tint-chip` | 12.31 – 13.82 | 8.95 – 10.98 | a filter chip's word and count, a label tag |
| `--ink` on `--glass` | 12.79 – 16.88 | 10.54 – 15.58 | the header's name |
| `--ink` on `--glass` (camera bar) | 12.79 – 16.88 | 10.54 – 15.58 | the camera's name and LIVE; rendered in headless Chrome over the mockup's white and black picture parts: 12.79 – 16.88 light, 10.39 – 15.58 dark (blur and saturate move dark by 0.15) |
| `--st-bad` on `--glass` | 4.95 – 6.54 | 4.77 – 7.05 | the LIVE dot |
| `--muted` on `--glass` | 5.18 – 6.83 | 4.95 – 7.32 | the header's space name and *View as*; also the edge of the header's select and theme switch (≥ 3:1 in the worst case) |
| `--line-strong` on `--glass` | 2.98 – 3.94 | 2.72 – 4.02 | too low in the worst case, so not used in the header: its select and theme switch take `--muted` as their edge |
| `--ink`, `--muted` on the opaque tints (reduced transparency) | 13.82 / 5.59 (chip), 14.54 / 5.89 (selected) | 9.83 / 4.62, 11.04 / 5.19 | the same, without transparency |

`--muted` on `--tint-chip` is not used: it falls to 4.21 on `--surface-2` in dark, so chips and tags
carry `--ink`.

Pairs added by the UX-test sweep (computed with the same formula; text needs 4.5:1, a boundary 3:1):

| Pair | Light | Dark | Use |
|---|---|---|---|
| `--ink` on `--surface-2` | 14.72 | 13.73 | outcome and job tags (`.out`), the shown-once secret |
| `--line-strong` on `--surface-2` | 3.43 | 3.54 | a tag's edge, a disabled button's edge |
| `--muted` on `--surface-2` | 5.96 | 6.45 | a disabled button's label, a closed step |
| `--on-strong` on `--st-warn` | 5.73 | 8.07 | the letter on an Attention cell |
| `--line-strong` on `--surface` | 3.94 | 3.89 | a free cell's edge (was `--line`, 1.15) |
| `--muted` on `--surface` | 6.83 | 7.09 | a free or offline cell's letter |
| `--ink` on `--surface` (as an edge) | 16.88 | 15.09 | the pressed chip's edge, beside its check mark (UT-18) |
| `--st-info` on `--surface`, `--bg` | 5.90, 5.49 | 8.04 | the mockups' spec annotation |
| `--st-wait`, `--st-bad` on `--surface` | 5.31, 6.54 | 8.11, 6.82 | a command's done and refused line |
| `--bg` on `--ink` | 15.71 | 16.59 | the Undo toast |
| `--st-bad` on `--surface` (as an edge) | 6.54 | 6.82 | the laser's Stop bar |

### States and colour

One list of states, one word and one colour each, everywhere (wiki *Design: The Website*, principles
1 and 6): **Free** grey and quiet; **Printing** (busy) the accent fill, read *Burning* on a laser;
**Needs someone** green with its button; **Attention** amber (a lid open, out of labels); **Error**
red; **Offline** a dashed outline; **Unknown** a dashed outline with "?". The state's word is always
written, so colour is never the only sign. The list is exclusive (UT-16): outcomes (done, stopped,
failed), a job's place (*Job loaded*, *On MINI-1*) and stock words (*Low*, *Weighing*) are square
tags without the dot, never a state's chip or colour. The kind of a machine is a word, not a colour
(**proposal**, study A3).

### Type, space, components

- **Type** (**proposal**): the system font; 12, 14, 16, 20, 26, 40 px, nothing smaller (the phone's
  bottom labels are 12 px, UT-41); tabular numerals for every number that changes; the clock 26 px
  (40 px on the wall screen).
- **Space:** 4, 8, 12, 16, 24, 32 px; a 16 px gutter on a phone; no horizontal page scroll (tables
  scroll inside their own frame).
- **Targets:** 44 px for every button, input, chip and tab; 32 px only for small buttons in dense
  admin lists on a desktop; on a phone everything is 44 px (UT-23).
- **Access** (UT-20, UT-21, UT-22, UT-40, UT-41): a "Skip to content" link comes first on every
  page; a disabled button is `aria-disabled`, stays focusable and points at its reason with
  `aria-describedby`; a status that changes (a command's answer, the PPE check, Start burn's reason)
  is `aria-live="polite"`; in-page tabs have the full tab pattern (`aria-controls`, arrow keys,
  Home, End); a navigation between pages (Space's tabs) is a list of links with `aria-current`; the
  theme switch is named "Theme: Auto"; the phone's More menu closes on Escape and on a click outside.
- **Components:** the state chip; the outcome tag; the card; the primary, secondary, danger and ghost button; a
  disabled button with its reason under it; the banner (info, attention, error, ok), one per kind of
  trouble; tabs; filter chips with counts; numbered steps; the value tile with *unknown*; the empty
  state (one sentence and one next step); the loading skeleton (no zeros while loading, one
  "Loading …" status for a screen reader, UT-42); the confirm dialog; the Undo toast; the
  shown-once secret.
- **Theme:** light and dark follow the system; a switch (Auto, Light, Dark) in the user menu.

## Open points of the UI

| # | Point | State |
|---|---|---|
| UI-1 | A member reading their own requests and withdrawing one | decided (journal:UI-DESIGN:0009 #5): `GET /api/v1/access-requests/mine`, `DELETE /api/v1/access-requests/{id}` in [api.md](api.md) |
| UI-2 | The machine's tabs: six against the wiki's five with **Data** | decided again by the owner (2026-11-20, journal:HUB-FINISH:0006): Prusa Connect's layout, eight tabs and a head with live chips ([A machine's page](#a-machines-page-3d-printer)) |
| UI-3 | The laser hand-off when a job is loaded | decided (0009 #3): sending asks to replace the loaded job |
| UI-4 | The wall screen | decided (0009 #4): read only, no buttons. Open: which session the screen keeps |
| UI-5 | "Mine" for spools needs an owner on a spool ([OQ-14](risks-and-open.md#oq-14-tables-of-materials-items-and-labels)) | open; the filter is not shown meanwhile |
| UI-6 | The path to take a loaded laser job off ([OQ-1](risks-and-open.md#oq-1-exact-paths-and-fields-of-our-own-endpoints)) | open |
| UI-7 | Making a new organisation: api.md has no route | open |
| UI-8 | How long a page waits before it says an answer did not come (loading), and the sign-in wait's numbers (flows.md §7) | open: not in the design |
| UI-9 | Host and area rights in Space: the host's exact tabs, the grant form's bounds, the kind's defaults ([OQ-41](risks-and-open.md#oq-41-the-host-role-open-points), [OQ-42](risks-and-open.md#oq-42-area-rights-open-points)) | decided (journal:DESIGN-HOST:0006): the bounds and defaults in [authz.md](authz.md#presets-roles-and-grants); the host's tabs follow its actions (Space (admin) above) |
