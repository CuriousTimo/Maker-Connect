# Web UI

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
- **Laser:** Start burn only on the laser's own page, by a person, after framing and PPE.

## Pages

| Page | Subject | Reads | Issue |
|---|---|---|---|
| Now | the space at a glance, wall screen | devices, jobs, queue, events | |
| Machines | the device list | `GET /api/v1/devices`, events | |
| Machine | one device: tabs per subject (overview, control, files, jobs, camera, settings) | device, commands, history, transfers | |
| Add machine | pairing by code; the Buddy INI with the hub's address, warning when the address does not fit the dialect's hostname limit (35 bytes for Buddy 6.4.0, `reference/buddy-6.4.0` `src/connect/hostname.hpp:16`) | `POST /api/v1/devices/pair` (proposed path, [OQ-1](risks-and-open.md#oq-1-exact-paths-and-fields-of-our-own-endpoints)) | #22 |
| Files | uploads, what a file says about itself, Print on | files, devices | |
| Queue | per device, order, take out | queue | |
| Prepare | the tools that make the work: laser design (the LaserFoundry web fork; picks a hub laser, its settings from the hub), the label editor | `/api/v1/lasers`, devices | #6, #24 |
| Jobs | job history with filters | `GET /api/v1/jobs` | #11 |
| Farm, projects | projects, parts, dispatch | farm endpoints | |
| Materials and stock | Spoolman's data with our UI; tapes and filaments apart; laser materials | Spoolman tables, locations, items | #10 |
| Cameras | watching: the space's cameras, the player | cameras, `/ws` | #5 |
| Space | tabs: General, Members (with invites), Permissions, API tokens, Audit log, Cameras (manage), Access (doors) | space endpoints | |
| About | build, update, last update's outcome, dumps | `/api/v1/update` | |
| Instance (instance admin) | every organisation and space; every agent with its version, last seen and devices, and its update ([decision](decisions/20261119-agent-update-by-instance-admin.md)) | `/api/v1/instance/*` | |

## House colour

The house colour is **`#93A682`** (owner, 2026-11-19, [decision](decisions/20261119-house-colour.md)).
Measured contrast (WCAG 2.x relative luminance):

| Pair | Ratio | Use |
|---|---|---|
| `#93A682` on white `#ffffff` | 2.62:1 | too low for text or for a control's only boundary |
| dark ink `#1d1d1b` on `#93A682` | 6.45:1 | text on an accent fill |
| `#93A682` on dark background `#111214` | 7.16:1 | accent as text in dark mode |
| `#93A682` on `#f6f7f5` (light page) | 2.44:1 | too low |

Tokens:

| Token | Light | Dark | Use |
|---|---|---|---|
| `--accent` | `#93A682` | `#93A682` | fills: buttons, chips, the selected tab, with `--on-accent` text |
| `--on-accent` | `#1d1d1b` | `#1d1d1b` | text and icons on `--accent` |
| `--accent-text` | a darker shade, [OQ-16](risks-and-open.md#oq-16-the-house-colours-text-shade) | `#93A682` | links and accent text on a light background |

The derived candidate for `--accent-text`: `#697C58` (same hue and saturation, lighter value lowered)
reaches 4.54:1 on white but 4.23:1 on `#f6f7f5`, so the exact shade depends on the light page
background: open.
