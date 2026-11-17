# The hub side: devices that talk to the hub directly

Status: **draft, 2026-11-15**. Written while the owner delegated approvals to `owner-proxy`
("Ga door en maak af: … Hub kant"). Items marked *to decide (owner)* are reference choices the
proxy may not make.

## What it is

From `docs/agent-engine.md`: the MINI (Prusa Connect) and Klipper (Moonraker) connect to the hub
themselves. They get an **agent inside the hub**: the same engine (`crates/engine`) and the same
kind of ports, run in the hub's process. The hub then makes no G-code for them either (D9):
`jog_command`, `grbl_action` and `action()` in `crates/hub/src/devices/legacy_gcode.rs`, and the queued
`"G28"`-style strings in `devices/prusa_connect.rs` / `devices/printer_files.rs` / `devices/simplyprint.rs`, go.

Plus **Spoolman**: the hub answers Spoolman's API (`crates/hub/src/spoolman.rs`). The owner
decided it is API- and data-compatible with Spoolman, not a copy of it (memory
`maker-connect-spoolman-compatible`). It has no audit yet.

## The three parts

| # | Part | Wire | Reference | State |
|---|---|---|---|---|
| H1 | Engine inside the hub | none (in-process) | none: ours, same engine as the agent | to build |
| H2 | MINI port | Prusa Connect, the hub is the server | the printer's side: Buddy at the version the MINI runs (`reference/buddy-6.3.0`, `src/connect`), what it sends and accepts | protocol done in the hub; mapping of typed actions *to decide (owner)* |
| H3 | Klipper port | Moonraker's `[simplyprint]` component, the hub is the server | the printer's side: Moonraker `components/simplyprint.py` at `b559b9a` | protocol done in the hub; mapping of typed actions *to decide (owner)* |
| H4 | Spoolman API | HTTP + WebSocket, the hub is the server | Spoolman at `eccb05e` (`reference/Spoolman`, MIT) | built; **audit to make**: run real Spoolman and ours on the same requests, "identical" or where it differs |

## H2 and H3: the open question (to decide, owner)

**Owner, 2026-11-15, for the MINI:** "G-code voor de MINI zit in de mini firmware": the
reference is Buddy 6.3.0 itself, what its own UI sends for the same control. Klipper still open.

Both servers are closed (Prusa Connect, SimplyPrint), so no server code can be ported. What the
printer accepts is known from its own code; what the real server *sends* for a hub control
(jog, home, extrude, heat, fan, speed, flow) is not.

- **MINI.** Buddy 6.3 accepts typed Connect commands (`src/connect/command.cpp:154-192`:
  `START_PRINT`, `PAUSE_PRINT`, `DIALOG_ACTION`, …) and raw G-code as a command
  (`gcode_command`, :90). `SET_VALUE` (:326-415) covers only hostname, enclosure, nozzle
  properties and chamber settings: no nozzle/bed target, fan, speed or flow. So jog, home,
  extrude, heat, fan, speed and flow can only go as raw G-code, and the question is which
  G-code. **Decided by the owner (2026-11-15): Buddy 6.3.0's own menu G-code** (see the top of this
  section); the earlier candidates (PrusaLink's G-code, watching real Prusa Connect) are dropped:
  one reference.
- **Klipper.** Moonraker's `simplyprint.py` (`_process_demand`, :338-355) accepts
  `pause`/`resume`/`cancel`/`gcode`. Candidates for the G-code behind jog/home/extrude/heat:
  (a) what SimplyPrint's own panel sends (closed; needs watching); (b) Mainsail; (c) Fluidd.
  Mainsail and Fluidd are not in `reference/`; adding one is the owner's call.

Until the owner decides, H2/H3 keep the protocol code they have; no new mapping is written.

## H4: the Spoolman audit (can start)

`scripts/spoolman-audit-check.py`: start real Spoolman (`reference/Spoolman`, its own
`pyproject`, SQLite in a temp dir) and send it a fixed list of requests (create vendor,
filament, spool; lists with filters, sort, limit/offset and `x-total-count`; `use`, `measure`;
errors), recording status, every response header real Spoolman sets, and JSON bodies. A Rust test sends the same list
to the hub's `/api/v1` and compares. Every difference is listed with **Result: identical** or
exactly where it differs. What the owner decided is Maker Connect's own (extra fields, places)
is listed as "ours (owner)". The audit ends with an explicit "not covered" list (e.g. SpoolmanDB
import, extra-field definitions, WebSocket sequences if not tested). Real Spoolman runs from
`reference/Spoolman` on SQLite in a temp dir; nothing touches the owner's data.

**Decided (proxy, 2026-11-15):** H4 approved to start; H1 approved as a plan (its design to approve
when written); H2/H3 keep their protocol code, no new mapping until the owner picks the G-code
reference.

## H1: the engine inside the hub (after H2/H3 are decided)

The hub's Prusa Connect and SimplyPrint routes become the ports' transport: a route hands what
the printer sent to its port and takes what the port wants sent. The engine reports to the
hub's store as the agent's hub link does over the network. Written as its own design, for
approval, once H2/H3's mappings are known.

## For the hub side (owner, 2026-11-15, "ok, neem mee als de hub-kant aan de beurt is")

- **PnP tape count.** The hub still counts each PnP label as its picture length + 16 mm
  (`crates/hub/src/store/jobs.rs`, `count_off`, tape taken off the roll; `devices/virtual_machines.rs` ~157). The 16 mm
  is the old agent's own trailing blank, gone with the labelle port. Count the length with
  labelle's own margins instead, taken from the port's constants (`ports/labelle`,
  `labeler_margin_px`), so the count matches what the printer feeds.

## The hub-side plan (draft for review, 2026-11-15)

Everything left for the hub, collected from `docs/agent-engine.md` and the audits, in a proposed
order. Each item gets its own brief, vetted by the proxy and the watchdog before work starts.

| # | Item | Why now / source | Reference |
|---|---|---|---|
| HS1 | The ports' geometry for the hub: a small crate (or engine feature) with only the ported constants (labelle tape sizes and margins; the 450's PPD page sizes and imageable areas), so the hub can depend on it without the device crates | blocks HS2 and HS3; B8 "the hub takes each printer's geometry from its port" | the ports' ported constants |
| HS2 | LabelWriter 450 labels: the hub draws the CUPS page the port needs (imageable area at 300 dpi, `HEAD_DOTS` 672 vs 665) | every hub label to a 450 fails today | DYMO PPD `lw450.ppd` via HS1 |
| HS3 | PnP: tape counted with labelle's own margins; the hub's own 16 mm trailing blank (L9) | tape count wrong; L9 for the owner | labelle via HS1 |
| HS4 | The other agent ports fail pending commands on close (K18 open item), then the gate/120 s goes for them too | B3 completion for PnP, 450, laser | each port's reference |
| HS5 | Persistence: the hub's command queue in SQLite (AUTOINCREMENT ids; Queued cancelled after a hub restart; Done/JobEnd accepted once) and the engine's unreported reports on disk beside the agent's config | §10 weak spots (1b, 1c) | ours (architect's design) |
| HS6 | Emergency-stop button per kind: OctoPrint machines typed `M112`; lasers LaserFoundry's stop; label printers none | decided (point 2) | each reference's path |
| HS7 | A laser never starts unattended: `start_queued` skips lasers; Start on the machine page after framing, operate + PPE | owner's rule | — (hub safety) |
| HS8 | The hub's own G-code for virtual machines (`jog_command`, `grbl_action`, `action()`): virtual machines go through the same ports (virtual lasers already use LaserFoundry) | D9 debt | the ports |
| HS9 | H1: the engine inside the hub (bridge sync engine ↔ tokio, reports into the store) | needed for HS10/HS11 | ours (design first) |
| HS10 | MINI (H2): typed actions → Buddy 6.3.0's own menu G-code (owner's decision), via Prusa Connect; the hub's `M604`, load `M109`/`M701` with 215 °C, etc. go | D9 debt on the MINI | Buddy 6.3.0 |
| HS11 | Klipper (H3): G-code reference still open for the owner | — | owner to choose |
| HS12 | Spoolman audit (H4) | approved, waits for its turn | Spoolman eccb05e |
| HS13 | LaserFoundry web fork, served by the hub: "Send to [laser name]" (owner, 2026-11-17) makes it that laser's loaded job (HS7b: one loaded job, no laser queue) (Apps API v1 extension). Packed **gzip in the hub binary** (owner, 2026-11-17: "ja, gzip in de binary"; measured on LaserFoundry's own web build: wasm 11.8 MB, 3.8 MB gzip; the hub sends it with `Content-Encoding: gzip`) | after the laser port (done) | LaserFoundry v0.5.0 |
| HS14 | Docs pass: `docs/design-overview.html`, `docs/audits/README.md`; `scripts/reference-updates.sh` for every reference (pin read from `Cargo.lock`) | decided (points 3, 4) | — |

Proposed order: HS14 (docs, small, makes the plan readable) → HS1 → HS2 → HS3 → HS7 (safety) →
HS4 → HS6 → HS5 → HS8 → HS9 → HS10 → HS12 → HS13 → HS11 (when the owner picks).

### The hub-side plan, reviewed (architect + proxy + watchdog, 2026-11-15)

Order: **HS7a** (now: a laser job never starts by itself — `queue.rs` `start_queued`, `files.rs`
copies, `farm.rs` `dispatch` — guarded once in `store.add_job`; and the "Laser off" button removed
until LaserFoundry has a laser-off call; laser Stop offered also when no job runs) → HS14 (docs
only; the design page pass at the very end) → HS1 (engine Cargo features, the hub depends on the
engine with `default-features = false`, a small `ports::geometry` facade over ported code; test that
the hub has no `serialport`/`nusb`) → HS2 (the 450's raster size and orientation per PageSize,
`fit`/`media_dots`/`label.js`) → HS3 (PnP tape counted from labelle's own bitmap width; the hub draws
no 16 mm blank — the 16 mm exists only in the count, `store/jobs.rs` (`count_off`), `devices/virtual_machines.rs` ~157 —
so L9 is a count fix, not a question; head-to-cutter tape use "unknown") → HS4 (ours, K-row: PnP
nothing pending; 450 and laser fail pending on Close; the hub's gate rule becomes "an agent port that
fails pending on close") → HS6 (the button, our UI; agent OctoPrint already has typed `M112`) → HS7b
(Start on the machine page after framing, operate + PPE + a session; no "just framed" rule) → HS5a/b
(design first, approval: the hub's queue in SQLite; reports on disk with HS9's interface; running a
migration on the Pi is the owner's) → HS9 (design first, MINI scope) → HS8 (after HS9; which port a
virtual FDM machine uses is the owner's choice) → HS15 (new: make `buddy.md` and `klipper.md`
mechanical) → HS10 (Buddy 6.3.0's own menu G-code, a per-button table to approve; the hub's `M604`,
`G28`, 215 °C go; nothing of ours replaces them) → HS13 (the owner decides name, address, access and
the API extension first) → HS11 (the owner picks Klipper's reference). HS12 (Spoolman audit) in
parallel from the start.

**Owner, 2026-11-15: the hub is split up ("opsplitsen is een must").** The hub's own part is kept
and improved step by step; its device side is rebuilt bottom-up as engine ports inside the hub
(HS9/HS10/HS8); `crates/hub/src/main.rs` (~160 KB) and `store.rs` are split into modules. The split
(HS0) comes right after HS7a and HS12 finish (they edit hub files now), before the other hub items;
the architect designs the layout first (per module: keep / adjust / rebuild), for approval.
Owner, same day: "zo kan de hub ook lean en mean op zichzelf draaien" — the split also makes a lean
hub possible: a small core that runs on its own, with the optional parts (the in-hub engine and its
device ports, virtual machines, …) switched on only when wanted.
Owner: "juist aparte modules. en later meer modules voor andere zaken" — a lean core plus separate
modules with a clear boundary, so more modules can be added later without touching the core.

## The DYMO M2 scale (owner, 2026-11-16; later this week)

The owner's DYMO M2 USB scale (up to 2 kg): a general weighing function, and weighing a spool —
scan its QR (`S-0007`), put it on, the hub sends the gross weight to Spoolman's measure
(`PUT /spool/{id}/measure`). A new device kind in its own stack (agent port; read-only: nothing is
sent to the scale). Reference to be chosen by the agents when it arrives. Candidates: the USB-IF HID
Usage Tables for Point of Sale (usage page 0x8D, Scale Data Report) as the standard the scale follows;
`github.com/dunmatt/weighty` (MIT, Rust, 209 lines, one person, 5 commits on 2020-08-01, tested only
on a DYMO M10, uses `hidapi`) found by the owner, in `reference/weighty`, at least to understand DYMO's
reports. Proof on the real M2 with known weights.
Owner, same day: "zolang onze m2 driver werkt voor m1, m2, m5, m10 enz op linux, windows en macos
prima" — one scale port for DYMO's M-series (M1, M2, M5, M10, …) on Linux, Windows and macOS. They
share the HID Point of Sale Scale Data Report; range and units come from the report, not a table of
ours. On Windows and macOS the OS owns HID devices, so the port reads through the OS HID interface
(e.g. the `hidapi` crate, as `weighty` does, or a pure-Rust HID crate), not `nusb`; the crate choice
goes with the reference choice.
Owner, 2026-11-16: scope for now: "gewoon standaard gewicht meten en doorgeven live. verder niet het
hele spool system" — read the weight and report it live; the spool/Spoolman flow comes later, once
the scale works.

## USB webcams (owner, 2026-11-16; design, not started)

Owner: "camera verbindt met agent. meerdere cameras per agent mogelijk. pas in de hub wordt het aan een
device of ruimte/kamer gekoppeld." A camera can watch a machine or a room. Agents on Linux, Windows and
macOS; ffmpeg is too heavy.

Test on .67 (Orange Pi PC, Logitech C270 `046d:0825`, a throwaway program, not in the repo): nokhwa 0.10.11
(`default-features = false`, `input-native`) builds for armv6, 537 KB, 4.7 MB RSS; MJPEG up to 1280x960 at
5-30 fps; ~0.1 % CPU at any rate (frames passed through, not decoded); 3.5 s from open to the first frame;
the frame comes in the whole 816000-byte buffer (JPEG ~31 KB, cut at `bytesused`); UVC MJPEG has no Huffman
tables (macOS ImageIO cannot open it; Chrome/Firefox to be tried).

**Reference: nokhwa, on all three OSes (owner's exception, 2026-11-16):** "vergeet PrusaLink voor cameras. heel
slecht idee. -> nokhwa overal". nokhwa 0.10.11 (Apache-2.0, pinned; one main maintainer, so an exception to
CLAUDE.md as for LaserFoundry; fixes go upstream), `default-features = false`, `input-native` (V4L2 / Media
Foundation / AVFoundation). PrusaLink is not used for cameras.

The architect's design (2026-11-16), its model kept, the reference now nokhwa:
- A camera is not a machine: its own part of the agent, several per agent; `--mode cameras` for an agent with
  cameras only. Found and named through nokhwa's camera list; how a camera keeps its identity across replugs
  (name, serial, USB path) to be designed with nokhwa.
- The agent offers its cameras to the hub; the hub names them and links each to a machine or a space
  (hub module `cameras`). A machine camera is seen by whoever may see the machine; a room camera gets its own
  right, set only in Space.
- Snapshots: one JPEG per interval, MJPEG passed through, cut to the JPEG's length. To the browser: SSE "new frame" + the image reloaded.
- Steps: 1 proof (Linux, one camera, a snapshot every 30 s to a hub test page; the Huffman question in three
  browsers; do two C270s share a serial); 2 protocol + hub module + linking + the Space right + camera page;
  3 several cameras, replug, `--mode cameras`, audit; 4 live (below); 5 run and test on Windows/macOS.

**Maybe, later: live video via WebRTC on a fat hub or a fat agent (owner, 2026-11-16).** WebRTC needs
H.264/VP8, so MJPEG must be decoded and encoded again: too heavy for a Pi doing it in software. Only where
there is the power: a fat hub (the agent keeps passing MJPEG through; the hub encodes, only while someone
watches, and serves WebRTC, which also avoids a TURN relay) or a fat agent (a machine with a hardware
encoder, or a camera that gives H.264 itself). Cost: live MJPEG agent→hub ~1.2 Mbit/s per camera at 5 fps.
Reference to choose then.

**Owner, 2026-11-16:** "live beeld" — live video is wanted, not snapshots only (step 4 is in scope: live
while someone watches; how it reaches the browser goes to the architect; WebRTC stays the maybe above).
"later print snapshots icm layer hight … voor later een nice to have" — later, nice to have: a snapshot on
each layer change during a print (a timelapse by layer height). "Privacy negeren voor nu, op de lijst. vullen we later in als het technisch werkt" —
the privacy points below wait until it works technically.

Windows/macOS: nokhwa builds for both (870 KB / 1.4 MB), not yet run there. Later (privacy, on the list): a room camera's privacy (who may watch, whether views are logged,
whether frames are kept, a notice in the room (AVG, works council), a way to switch it off; the microphone
is never opened).

**Size and order (2026-11-16, written down at the owner's request, not started):**
- Step 1, the proof, a few hours: the agent takes the C270 on .67 through nokhwa and sends a JPEG every few
  seconds over the link; the hub keeps only the latest in memory and shows it on a test page refreshed by
  SSE. Needs one decision first: a reference for putting the missing Huffman tables into the raw MJPEG frame
  (nokhwa does not; candidate ustreamer). Can run beside other work only when the owner says so.
- Steps 2-5 (protocol camera list, several cameras, identity across replugs, hub module `cameras`, linking
  to a machine or a space, the right in Space, a camera page, live video through the hub, audit, Windows and
  macOS runs): 1 to 2 days. After the MK3 and Buddy work of this week.

## Network cameras (ONVIF/RTSP), straight to the hub (owner, 2026-11-16; step 1 the proof)

**Scope (owner):** "dus het is niet alleen deze camera, maar onvif cameras in het algemeen. we gaan nu niet de
hele onvif implementeren. alleen wat nu nodig is. de rest op de lijst (sommige zelfs als maybe, maybe not).
daarna de usb cameras via de agent. (later dus)" and "dus het software design is ook gebaseerd op dit. cameras
in het algemeen" — ONVIF/RTSP cameras in general, and the software is designed for cameras in general: nothing
LSC-specific in code; the LSC below is the first test device (given with `--camera-*` flags), not the target.
Where (owner): "zoals het nu werkt is onvif camera support er alleen binnen het LAN. in cloud mode zie ik het
niet als handig op dit moment omdat er geen manier is om op en onvif camera te komen zonder vpn of onveilige port
forwards. later kunnen we bekijken of een agent in hetzelfde lan als de camera als gw/proxy kan dienen?" — only
on the hub's own LAN; not in cloud mode for now; later, to look at: an agent on the camera's LAN as gateway/proxy.

**Order (owner):** "met deze RTSP camera kunnen we de camera support op de hub helemaal in orde maken. de usb
webcams via de agent volgen dan later" and "dit gaan we nu meteen doen" — this starts now, ahead of the
MK3/Buddy work (parked; its uncommitted files stay untouched). "nokhwa is alleen voor USB cameras via de
agent. compleet anders dan deze. RTSP kan direct met de hub praten." Why this camera: "veel goedkoper dan een
logitec. hij heeft wifi". Hub hardware: "beide. dus ga uit van een Pi" — no decoding or transcoding in the hub.

**First device (measured 2026-11-16):** LSC Smart Connect indoor camera (Mini 16S, firmware
`ppstrong-a3-tuya2_lsc-4.0.6.20210311`, rooted and patched for ONVIF/RTSP) at 192.168.60.72, `admin:admin`.
`rtsp://IP:8554/Streaming/Channels/101` 1920x1080 H.264 20 fps, `/102` 640x360, both with PCMU audio, played
over TCP. ONVIF :8000: `GetStreamUri` gives `/101` and `/102`; `GetSnapshotUri` gives an internet URL
(`www.gwelltimes.com/...jpg`), not the camera; no events pull-point.

**Reference: retina** (owner: "beantwoord vraag 1 voor me"), scottlamb/retina at `2b6b7fe` (2026-11-12),
crate 0.4.22, MIT OR Apache-2.0, in production in Moonfire NVR. Default transport TCP
(`src/client/mod.rs` `impl Default for Transport`). Camera→screen is retina's own example
`examples/client/src/webcodecs.rs` + `webcodecs/webcodecs.js` + `index.html`, ported function by function:
one RTSP session, frames broadcast to all viewers, a "parameters" message (only if the viewer has not seen
that `parameters_id`) and a "frame" message, binary, over an axum WebSocket with pings; the browser decodes
with WebCodecs. WebCodecs needs a secure context: the example serves HTTPS with a self-signed certificate
(`--tls`, rcgen + rustls/ring). Moonfire and fMP4 are not used.

**Changed by the owner (2026-11-16, later):** the live path is **MSE with fMP4** for the browser side (the reference first
named here, Moonfire NVR, was dropped the same evening: see go2rtc below) ("MSE"), instead of WebCodecs: WebCodecs needs a secure context (HTTPS or localhost), and HTTPS for a
LAN/demo hub is too much for test, demo and end users ("dat schiet niet op zo ... werkt ook niet makkelijk voor
eindgebruikers later"); MSE works over plain HTTP. retina stays the RTSP reference (Moonfire is built on it).
HTTPS is off the list for now. Not ported yet; the WebCodecs page below is what runs until then.
**Reference for the browser side: go2rtc** (decided 2026-11-16, owner delegated: "beantwoord de 4 vragen zelf voor me";
the owner noted Frigate uses it; Frigate itself is not used: "we gaan Frigate zelf niet gebruiken"): AlexxIT/go2rtc, MIT, live RTSP → fMP4 → MSE over a WebSocket without recording
(`internal/mp4/ws.go`, `pkg/mp4/muxer.go`, `pkg/iso/`, `www/video-rtc.js`); commit to pin when fetched. Moonfire NVR
dropped: its live path builds segments from its recording database (`server/src/web/live.rs`, `mp4::FileBuilder`
over `list_recordings_by_id`), so porting it means porting its recorder, and the Pi would record to SD all the time.
retina stays the RTSP side; go2rtc's own RTSP client is not used (one reference per stack).

**Port plan for MSE (go2rtc v1.9.15, `7c05066`, MIT; `reference/go2rtc`), H.264 video only, as now:**
- `pkg/iso/iso.go`, the atoms of `pkg/iso/atoms.go` that the muxer writes (`ftyp`, `moov`/`mvhd`/track/`avc1`/`avcC`,
  `mvex`/`trex`, `moof`/`mfhd`/`traf`/`tfhd`/`tfdt`/`trun`, `mdat`) → `cameras/iso.rs`, function by function.
- `pkg/mp4/muxer.go` (`GetInit`, `GetPayload`: one `moof`+`mdat` per frame, duration from the timestamp step, the
  key-frame flags, Safari's minimum duration), `consumer.go` (`WriteTo`: the init first; a viewer starts at a key
  frame), `mime.go` (`ContentType`) → `cameras/mp4.rs`; one muxer per viewer, as go2rtc's consumer per viewer.
- `internal/mp4/ws.go` `handlerWSMSE`: the viewer's first text message (its codecs), the answer
  `{"type":"mse","value":<content type>}`, then binary: init, fragments → the viewer's websocket in `page.rs`.
- `www/video-rtc.js`: only its MSE mode (MediaSource / ManagedMediaSource, its buffer handling) → `cameras/player.js`.
- From the camera side (retina, the camera's stack, not go2rtc's depacketizer): the frame in AVC form
  (length-prefixed NAL units, `FrameFormat::MP4`), its 90 kHz timestamp, key frame or not; SPS/PPS, size and the
  `avcC` record from retina's `VideoParameters`. Where go2rtc builds these itself (`h264.EncodeConfig`,
  `DecodeSPS`), that is listed as a difference.
- Not ported: audio (the microphone is never opened), H.265, rotate/scale patches, go2rtc's other modes.
- Where the two stacks meet (on-track, 2026-11-16), as go2rtc would see it: the muxer gets the frame's 32-bit RTP timestamp
  (retina's `timestamp()` as `u32`), so `GetPayload`'s own timeline and its first-frame duration (91) are go2rtc's;
  `cts` 0 (go2rtc's `ExtensionProfile` for RTP, so no CTS in `trun`); go2rtc's start-of-access-unit filter
  (`pkg/h264/rtp.go:52-75`: SPS+PPS before an I-frame that has none, leading SEI/AUD skipped, unknown NAL types dropped)
  is ported and run on retina's frame; SPS/PPS for it and for the init come from retina's `avcC` through go2rtc's
  `DecodeConfig`; the codec string is go2rtc's `GetProfileLevelID` on that SPS; width and height from retina (go2rtc
  decodes the SPS: a difference, caught by the byte compare); `GetInit`'s dummy SPS/PPS and 1920x1080 fallbacks are
  ported. Not used from go2rtc's depacketizer (retina's job): its packet handling, the 5 MB guard, the Annex B fix.
- The websocket as go2rtc's: the viewer's first text `{"type":"mse","value":"<its codecs>"}`, the answer
  `{"type":"mse","value":"video/mp4; codecs=\"avc1.…\""}`, then binary init and fragments; an error as
  `{"type":"error","value":"mse: <text>"}`; a 5 s write deadline per message; no pings (go2rtc has none: the 30 s
  keepalive and the "skipped" message of retina's example go); a viewer that falls behind loses frames and is not
  resynced (go2rtc: a full sender drops, `c.start` is set once).
- Ours (owner, 2026-11-16: "doe a maar nu"): the hub keeps a camera's frames since its last key frame (retina's
  random access point) and gives them to a new viewer first, so the picture comes at once instead of at the next key
  frame (the LSC sends one every 3.95 s on `/101` and `/102`); go2rtc's player catches up to live itself
  (`video-rtc.js:478`). Measured: the first fragment 20-40 ms after opening instead of up to 4 s. Memory: one GOP per
  camera (~100-150 kB for the LSC); a camera that never sends another key frame grows it (not bounded: noted).
- The player is `www/video-rtc.js` unchanged, used as go2rtc's `video-stream.js` uses it (a custom element,
  `mode = "mse"`, `src` = the camera's websocket).
- Proof: the bytes of our init and fragments compared with go2rtc's own muxer on the same frames (a Go test
  in `reference/go2rtc` feeding the frames we record from the LSC); then playing on plain `http://` on the Pi in
  Chrome and Safari (our goal, not go2rtc's); an audit `docs/audits/go2rtc-mse.md` with its checker script and a
  not-covered list (the JS player is not covered by the byte compare). The WebCodecs player stays until then.

**Decided by the owner (2026-11-16):**
1. Video bytes over a WebSocket as in retina's example; SSE stays for everything else ("Video via een
   WebSocket lijkt me goed tenzij je een beter alternatief hebt" — none: SSE is text only, base64 +33 %; a
   fetch stream would need framing of our own; WebRTC is heavier).
2. HTTPS (a separate port, self-signed as in the example): later ("later (1 stap tegelijk)"). Until then
   the camera page plays only on `localhost` (WebCodecs needs a secure context).

**Step 1, the proof:** the port of the example as one test route behind cargo feature `cameras`, the camera
given with `--camera-url/--camera-username/--camera-password` (the example's `--url/--username/--password`),
`/101`. Proven when the stream plays in a browser. Then, on approval: the other
browsers, a long run, other measurements.

**Step 1, result (2026-11-16):** proven. `crates/hub/src/cameras.rs` (+ `cameras/webcodecs.js`,
`cameras/index.html`), behind feature `cameras`; the differences from the example are listed in its module
doc (all on the hub's side; towards the camera it sends what the example sends). Against the LSC `/101`:
the owner saw it play on `http://localhost:8460/cameras/test` in Chrome and Safari (Firefox not tried);
a WebSocket probe got `avc1.4D0028`, 1920x1088, ~10 fps over 6.3 s, ~250 kbit/s. Seen once in the hub's
first run: the RTSP session ended after 120 s with retina's `RtspFramingError` ("Invalid RTSP message",
byte 8531222); retina's own example ran 3 minutes against the camera without it, and the hub's second run
did not repeat it: cause unknown (retina #80 names cameras truncating a packet when not read fast enough).
Route snapshot (`tests/routes.txt`) is of the build with all modules; without `cameras` that test is ignored.

**Step 2, the module `cameras` (owner: "ga door met stap 2"; design 2026-11-16, architect, vetted by the proxy
and on-track; not built):**
- Pattern of `farm`/`stock`: `cameras.rs` (the step-1 port) + `cameras/store.rs` (migrate, `impl Store`) +
  `cameras/page.rs`; migrate in `store/migrate.rs` after farm/queue behind the feature (A5: a module that is
  off has no tables); its routes merged in the router as now; its link in the top bar behind the feature.
  No HS0b work.
- Table `cameras`: id, space, name, url (`rtsp://`), username, password, machine (empty = a room camera). Ours,
  for approval: a `source` column (`rtsp`; `agent` later) and `created_at`. Every query by space.
- Pages (one subject per page): `GET /cameras` the list of the space's cameras, with add/edit for those who may;
  `GET /cameras/{id}` one camera with the step-1 player; `GET /cameras/{id}/websocket` its video (right checked
  before the upgrade); `POST /cameras`, `POST /cameras/{id}`, `POST /cameras/{id}/remove`, in the audit log; a
  "Camera" tab on the machine page when a camera is linked to it. `/cameras/test` and the `--camera-*` flags go.
  No SSE in this step: when a session ends the websocket closes and the player shows it.
- Sessions: as retina's example, a camera's session starts with the hub and runs; no reconnecting (a camera
  that drops stays down until it is saved again or the hub restarts). Approved: a session starts when a camera
  is added or saved and stops when it is removed.
- The password is never sent back in a page (an empty field keeps it) and never logged (the URL is logged
  without credentials).
- **Decided (owner, 2026-11-16, "akkoord op ... machinecamera/ruimtecamera"):** who may view: a machine camera
  whoever may see the machine (guests too: they have View); a room camera a minimum role per camera, default
  admin.
- **Decided (owner, 2026-11-16, "alles akkoord dus"):** (2) add/edit/remove: admins, in the audit log; (3) the
  password in plain text in the database, as the machine and Moonraker tokens; (4) the room-camera setting on
  the Cameras page, not the Space page — changed later the same day: "camera beheer hoort in
  http://192.168.60.67:8460/space thuis als tab": adding, changing and removing cameras (with the room-camera
  role) is a Cameras tab on the Space page; `/cameras` stays for watching ("is prima voor de live beelden"); (5) several cameras play at once now; (6) the `source` and
  `created_at` columns; (7) a camera's session starts when it is added or saved and stops when it is removed.
- Not in step 2: sub stream, ONVIF, reconnecting, sessions on demand, SSE status, HTTPS, snapshots, the
  agent source, privacy/logging who watches, audio, CPU/mem monitoring.
- Order and tests: a migrate + store (new store has the table, an older one gets it, another space sees
  nothing); b the rights (after the owner; a test per role x kind of camera); c routes + `RULES` (route
  snapshot regenerated and its diff read, the route-walking auth test); d live per camera (re-proven on the
  LSC at `/cameras/{id}`); e pages; fmt, clippy, tests with and without the feature; on-track before the
  report. No commit.

**Step 2, result (2026-11-16):** built, not yet looked at by the owner in a browser. `crates/hub/src/cameras.rs`
(sessions and video, differences from retina's example in its module doc), `cameras/store.rs` (table, who may
watch), `cameras/page.rs` (Cameras page with add/change/remove for admins, a camera's page, the player, the
video route, the machine page's Camera tab). Tests: the table on a new and an older store, cameras per space,
who may watch per role and kind of camera; 124/124 with `cameras`, 120 + 1 ignored without; the route snapshot
regenerated. Live on the LSC: added through the form, its session started, `/cameras/1/websocket` gave
`avc1.4D0028` 1920x1088. UI choices of ours, for the owner to see: the room camera's role list offers all four
roles; the pages' own small style.

**Later steps (one line each; the architect's proposals, ours, awaiting the owner):** 2 module `cameras`
(camera model with source RTSP now / agent later, link to machine or space, the Space right, camera page,
credentials storage); 3 several cameras, the sub stream, other ONVIF cameras and newer LSC firmware (URL
decided by measuring on the camera); 4 the ONVIF list below, item by item; 5 the USB source via the agent
(later); later, to look at (owner): cameras outside the hub's LAN (cloud mode) through an agent on the
camera's LAN as gateway/proxy; later, to look at (owner: "kunnen we later go2rtc ook voor de usb cams gebruiken?"):
the USB webcams' delivery to the browser by go2rtc's way too (MJPEG passed through, `pkg/mjpeg`; H.264 cameras
by the MSE path), capture staying nokhwa on the agent. Also proposed: sessions only while someone watches, reconnecting, status by SSE, video only, never
fetching camera-given URLs that point elsewhere.

**ONVIF, the list (nothing built; own reference to choose first; labels are proposals for the owner):**
- Now: nothing — the proof uses a fixed RTSP URL.
- Next, when other cameras come: `GetProfiles` + `GetStreamUri` (find the RTSP URLs of any ONVIF camera
  instead of a template per model). Needs WS-UsernameToken auth.
- Maybe: device info (`GetDeviceInformation`: maker, model, firmware), `GetSnapshotUri` (on the LSC it points
  to the internet, so only where it points to the camera), WS-Discovery (multicast; does not cross VLANs).
- Maybe not: events/motion, PTZ, imaging settings, recording/replay/search, audio backchannel (the microphone
  is never opened), changing camera settings (users, network, time).

## On the list (owner, 2026-11-16; not started)

- Hub CPU and memory monitoring (owner: "hub cpu en mem monitoring").
- Maybe: CPU and memory of an agent's host (owner: "agent host cpu en mem monitoring? <- niet zeker over
  deze").
- Idea, to look at later (owner, 2026-11-17): web slicing in the hub by integrating
  https://github.com/steenfeldt/mica-slicer ("a native 3D-printing slicer for macOS, Linux and Windows, written
  in Rust"; seen 2026-11-17: 3 stars, last push 2026-11-09, **no licence on GitHub** — so it may not be
  used until its author gives one; CLAUDE.md: ask the owner first; also a one-person project).
- **One agent for several devices, with a small web page to manage it (owner, 2026-11-17, plan for later:
  "1 agent voor meerder devices. de devices zijn module zoals we nu ook al hebben? kleine simpele web
  management page on de agent om modules en settings te beheren").** Today one agent process drives one
  machine: `mode` in `agent.toml` (`crates/agent/src/config.rs`) picks one port of the engine
  (`crates/engine/src/ports/`: octoprint, labelle, dymo_cups, laserfoundry, …), so .67 runs three agent
  services (PnP, LabelWriter 450, laser). Plan: one agent, a list of devices, each a module (the engine's
  ports as they are, one reference each, unchanged) with its own settings and its own machine at the hub;
  USB cameras join as modules too. A small web page on the agent lists the modules, adds/removes a device,
  edits its settings and shows its state. Design questions for then (architect/proxy): who may open that
  page (the agent's host only, or a password/pairing with the hub), config file vs page as the one source,
  one process with a thread per device vs a child per device (a crash in one port must not take the others).
