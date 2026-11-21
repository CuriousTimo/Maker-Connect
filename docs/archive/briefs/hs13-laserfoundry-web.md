# Brief: HS13 — the LaserFoundry web fork, served by the hub, "Send to [laser]"

Repos: Maker Connect (this worktree, branch `hs13`) and the fork `/Users/timo/Documents/Coding/laserfoundry-fork`
(branch `maker-connect`, upstream LaserFoundry v0.5.0 `61febdb`, MIT OR Apache-2.0). Never committed by the agent.
Reference for the laser stack stays LaserFoundry (owner exception); nothing here touches the wire to a machine.

## What

1. **The fork** (`laserfoundry-fork`): a "Send to a laser…" entry in the File menu of the web version, shown only when the
   page was served by a Maker Connect hub. It lists the lasers the signed-in person may use, one **Send to <laser name>**
   button each. Sending keeps LaserFoundry's own G-code (the text `job.gcode` makes, as Save G-code writes it) in the hub's
   Files and loads it as that laser's job through the hub's existing route. A 409 `{confirm}` (a job is already loaded) is shown;
   the request is repeated with `replace=true` only after the person presses "Replace it on <laser>".
   Built close to their MakerFoundry code: `crates/ai/src/maker_connect.rs` (requests and answers, no network),
   `crates/ui-egui/src/maker_connect_ui.rs` (window), `Platform::{hub_available,start_hub,take_hub}`, the browser's `fetch` in
   `apps/laserfoundry-web`.
2. **Web Serial out**: cargo feature `machine-link` (default ON: upstream unchanged). Off: `serial_pick/close/signals`,
   `serial_web.rs` (the moved Web Serial code) are not compiled, `machine_available()` is false (LaserFoundry's own "not
   available in this build" text), and the bundle has no `requestPort`/`navigator.serial`.
3. **The hub** serves the fork gzip-packed from its binary under `/laser-design/` (`laser_design.rs`), behind sign-in, with
   `Content-Encoding: gzip`, the right content types and `Permissions-Policy: serial=(), usb=(), hid=()`.
4. **`scripts/laserfoundry-web.sh`** builds the fork at the pinned commit (`scripts/laserfoundry-web.pin`), runs wasm-bindgen
   (0.2.129), checks the bundle has no serial, gzips, and writes `crates/hub/laser-design/` (gitignored, generated).
5. A hub route `GET /api/laser-design/lasers` (cookie): the lasers the person may use, in the `machines` shape of Apps API v1.

## Decisions (owner, 2026-11-17, and the architect's advice)

- Button "Send to [laser name]", never "Send to Maker Connect". What is sent is that laser's loaded job (HS7b): one loaded
  job, no queue; replace only after a confirm. Nothing here can start a burn: the fork never calls frame, start-burn, or the
  queue; `path_allowed` in the fork lists the only calls the page may make (info, lasers, upload, print).
- Gzip in the hub binary (measured below). Same-origin, cookie session (`credentials: same-origin`), no token in the page.
- The fork sends LaserFoundry's exporter output only. Nothing of ours in the G-code (test: upload body == `job.gcode` text).

## Why not the Apps API v1 route for the list and the file

Apps API v1 deliberately ignores cookies (`docs/api/apps-api.md`: "A browser session never counts here"; `auth.rs` strips the
cookie when an `Origin` header is present). It also never replaces a loaded job (a token is not a person at a session). So the
page uses the hub's own cookie routes that exist already: `POST /api/files?name=` (upload), `POST /files/{id}/print`
(`machine`, `replace`; 200 `{loaded}` / 409 `{confirm}`), and one new read-only route for the lasers' list (ours, below).
`docs/api/apps-api.md` gets a short section saying so; v1 itself is unchanged.

## How it is proven

- Fork: `cargo test -p laserfoundry-ai -p laserfoundry-ui-egui -p laserfoundry-fuzzkit` (new tests: path allow-list with
  hostile paths, request building, answer reading, the whole send flow on a fake platform: upload body equals the exporter's
  G-code, load, 409 confirm waits, replace only after the person, refusal shown), fuzz entry `maker_connect`, wasm builds
  with and without `machine-link` (warning free), `cargo xtask layers brands cleanroom assets messages`.
- Hub: tests of the served files (gzip, content types, Permissions-Policy, permission), the lasers' list (only lasers the
  person may use), and the send path end to end against the hub's real routes (upload, print 200 loaded, 409 confirm,
  replace), plus that nothing in the flow starts a job. The no-serial check is `scripts/laserfoundry-web.sh` (and a hub test
  on the embedded bundle when it is built in).

## Not covered

- Real browser run against a hub with a real laser; no laser was touched. The script builds the bundle; a person tries it.
- Upstream merge request: the hub-only route and `hub_available` are hub-specific; for LaserFoundry's own repo the MakerFoundry
  pattern with the Apps API token (their `service.rs` allow-list) would be needed instead.
- A file the person does not replace stays in Files (uploaded, not loaded).
- Translations of the new texts; the fork's PR size rule (under 300 lines) is exceeded: see the report.

## Ours (to approve by the owner)

1. Menu text "Send to a laser…" and the window texts; the title "Laser design (LaserFoundry fork)" in the hub (not presenting it as
   LaserFoundry's official product).
2. Who may open `/laser-design/`: a space member (not a guest) who may use at least one laser of the space (else 403).
3. `GET /api/laser-design/lasers` and its path `/laser-design/` (the name is ours); `Cache-Control: no-cache` on the files.
4. The hub's copy turns the page's AI/account/MakerFoundry features off at run time (their code is still in the
   bundle, e.g. the `makerfoundry.app` address; the page's CSP `connect-src 'self'` stops any request elsewhere) (`http_available` false when a hub serves it) and its
   CSP `connect-src 'self'` only.
5. The G-code file name: the project's name cleaned to letters, digits, space, `-`, `_` (as their publish name), else `design`.
6. `MAX_GCODE_BYTES` 64 MiB in the page (the hub's own limit is 1 GB).
7. The `machine-link` feature name and the `hub_*` Platform methods.
8. Limits in the page's allow-list (`path_allowed`): a file id of at most 64 characters, a name of at most
   `MAX_NAME` × 9 bytes once encoded; the G-code file name cut to 60 characters.
9. `Permissions-Policy: serial=(), usb=(), hid=()` on the page's files (the architect's advice named serial;
   usb and hid close the other device APIs a browser offers).

Approved by the owner (2026-11-17, "HS13 ok").
