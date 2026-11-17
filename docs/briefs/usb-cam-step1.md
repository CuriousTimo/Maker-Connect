# Brief: USB webcams, step 1 (the proof)

Row: `docs/hub-side.md`, "USB webcams", step 1. One camera (nokhwa index) on an agent, a JPEG every few
seconds over the existing link, the hub keeps the latest in memory, an admin test page refreshed by SSE.
Nothing here is the design of steps 2-5; where step 2 will take over it is said.

## What

- Agent: `--camera <nokhwa index>` (config key `camera`), and `--mode cameras` for an agent with a camera and
  no machine (hub-side.md: "`--mode cameras` for an agent with cameras only"). Capture with nokhwa 0.10.11
  (`default-features = false`, `input-native`), owner's exception, all OSes.
- Wire: `POST /api/agent/v1/cameras/{index}/frame` (`protocol::CAMERA_FRAME`), body = one JPEG
  (`image/jpeg`), `Authorization: Bearer <the agent's token>`. An own route, not telemetry: telemetry is typed
  JSON every 2 s for the machine; a 30-300 kB binary frame does not belong in it.
- Hub: module `usbcam` (no cargo feature: it needs no retina). Keeps the latest frame per (agent machine id,
  camera index) in memory only, announces "new frame" on a broadcast channel. Test page `GET /usb-cameras/test`,
  frames `GET /usb-cameras/test/frame/{machine}/{index}`, SSE `GET /usb-cameras/test/events`; all three
  `Need::Role(Admin)`. The page has one `<img>` per camera, its `src` reloaded on each event.
- The frame the hub shows is a complete JPEG: the agent cuts it and inserts the Huffman tables (below).

## The open decision: the missing Huffman tables (DHT)

UVC MJPEG frames carry no DHT (measured: macOS ImageIO cannot open them). nokhwa does not add them.

| Candidate | Where | Verdict |
|---|---|---|
| **FFmpeg `mjpeg2jpeg` bitstream filter** | `libavcodec/bsf/mjpeg2jpeg.c:36-142` and `libavcodec/jpegtabs.h` (the K.3 tables), FFmpeg master `f1d4dd2bd0ace9263b590b1f3a500190b8687bef` (2026-11-17), LGPL-2.1-or-later (fits AGPL-3.0) | **Chosen.** Many maintainers, the most used; the filter's purpose is exactly "MJPEG/AVI1 to JPEG/JFIF"; installed locally (ffmpeg 8.1.2) so the port is compared byte for byte. |
| ustreamer | `src/ustreamer/encoders/hw/encoder.c:50-85`, `huffman.h:33-` | Same tables as FFmpeg's (checked: the 420 table bytes are identical), inserted before the first SOF0, skipped if a DHT exists. One maintainer; GPL-3.0. |
| go2rtc `pkg/mjpeg` | `helpers.go:12-53` `FixJPEG` (only acts on an `AVI1` APP header; a frame that starts `FFD8 FFDB` is returned unchanged), `InjectDHT` :81-100 (before SOS, 4 segments, 432 bytes) | One maintainer, and `FixJPEG` skips plain UVC frames. |

All three insert the same JPEG Annex K standard tables; they differ in where and in segment layout. One
reference only: FFmpeg's, ported as `crates/agent/src/camera.rs` `mjpeg2jpeg`: SOI, JFIF APP0 header
(`jpeg_header`), one 420-byte DHT segment (`append_dht_segment`), then the input after its SOI (or after its APP0
when it has one: `input_skip`). The output is what `ffmpeg -bsf:v mjpeg2jpeg` writes.

## Ours (named, owner to approve)

1. **Interval 5 s** between frames sent ("every few seconds"); the camera is read continuously and the latest
   frame is the one sent (a V4L2 queue left unread would hand out a frame seconds old).
2. **Capture format**: `Closest` 640x480 MJPEG 30 fps. Not MJPEG after opening: the agent stops with that
   message (step 1 sends MJPEG only; no decoding on a Pi).
3. **Cut to the JPEG's end**: nokhwa's V4L2 backend returns the whole mmap buffer and drops `bytesused`
   (`v4l-0.14.0/src/io/mmap/stream.rs:206-208`, `nokhwa-bindings-linux-0.1.4/src/lib.rs:899-902`). The frame
   is cut after its first EOI (`FFD9`) that follows the SOS, entropy-coded `FF00`/restart markers skipped. A
   frame with none is dropped.
4. **A frame that already has a DHT is sent as it is** (FFmpeg's filter assumes none and would add a second).
5. **Hub refuses a body that does not start `FFD8`** (400); axum's default body limit (2 MB) applies.
6. Latest frame in memory only, never on disk; the test page, its routes, `--mode cameras` registering as a
   machine of kind `other` (an agent registers as a machine today; step 2 offers cameras to the hub instead).
7. A failed POST is logged and the next frame goes at the next interval (no retries of ours).
8. The agent's token is shared with the camera thread (`Arc<Mutex<…>>`) and refreshed when the agent
   registers again, so frames keep going after a re-register.
9. The hub's live channel for frame events holds 64 events (`usbcam.rs`); a slow page skips to the newest.
10. `--mode cameras` needs `--kind other` (a camera is not a machine kind of its own yet; step 2).
11. The DHT code is a port of FFmpeg's LGPL-2.1-or-later `mjpeg2jpeg.c` + `jpegtabs.h` (f1d4dd2): its
    copyright line and licence are in the ported files' headers; LGPL-2.1+ code may be used in our
    AGPL-3.0 program (it can be taken under the GPL, LGPL-2.1 section 3).

## Proof

- `mjpeg2jpeg` against `ffmpeg -f image2 -c copy -bsf:v mjpeg2jpeg`: byte-identical on a sample JPEG with
  its DHT removed (a test builds the sample, runs ffmpeg when installed, compares; the case with an APP0 too).
- Hub: a frame POSTed with the right token is kept, the second replaces the first, an SSE event arrives, a bad
  token is 401, non-JPEG 400; the three pages refuse a member (403) and no session (401).
- nokhwa compiles for Linux armv7 and macOS.
- Hardware (2026-11-17, test build v0.3.0-test.9 on 192.168.60.67): the agent opened the Logitech C270
  (`046d:0825`, `/dev/video1`; `/dev/video0` there is the board's `cedrus` decoder) as "UVC Camera
  (046d:0825) at 640x480@30FPS, MJPEG Format, a frame every 5 s"; the owner saw the picture on the test page
  refresh in Chrome. Safari and Firefox not yet tried.

## Not covered

Several cameras, naming, identity across replugs, linking to a machine or a room, the Space right, live
video, Windows build and runs, a macOS run, permission prompts (macOS), the picture in Safari and Firefox
(Chrome shown by the owner), privacy, hub restart (frames are gone), stale cameras
(no "last seen" on the page). Step 2 moves `usbcam` into the module `cameras` (`source = agent`) and
replaces `--mode cameras` by cameras offered by the agent.
