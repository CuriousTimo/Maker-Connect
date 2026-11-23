# USB camera live view: the hub reads the agent's stream while someone watches

Date: 2026-11-23  
Status: accepted  
Decided by: owner (2026-11-23)  
Replaces: the live-view part of [20261120-dymo-first-device-and-sdk-extension.md](20261120-dymo-first-device-and-sdk-extension.md) (`/c/stream`, `START_CAMERA_STREAM`/`STOP_CAMERA_STREAM`)

## Context

The SDK (Prusa-Connect-SDK-Printer `c0d7d233`) has snapshots only: the device pushes `PUT /c/snapshot` at
its trigger schemes. Part 5 had added a pushed upload `POST /c/stream` with two commands of our own.
Prusa Connect Local (`reference/prusa-connect` `2e8c62b`) already has live view for a camera that pushes
its stills: its generic camera, `adapters/camera/generic/generic.go:34-37`: "The still images are the
frames the device sends to PUT /api/v1/cameras/{camera}/snapshot, or, when snapshot_url is set, the image
this server fetches from it. The stream is optional: an rtsp:// address is read as H.264 and served to a
browser through the WebRTC relay, and an http:// address is relayed as MJPEG. It is read only while
somebody watches." Its MJPEG driver "fits the Prusa ESP32-CAM firmware and any IP camera with a snapshot
URL and a stream URL" (`adapters/camera/mjpeg/mjpeg.go:5-6`).

The agent's host is on the hub's LAN or behind WireGuard, so the hub can reach it.

## Decision

Asked "de hub haalt het beeld bij de agent op, alleen zolang iemand kijkt. Dat is een uitzondering op 'het
apparaat belt de hub'. Mag dat?", the owner answered "Ja, ophalen".

- **Snapshots** stay the SDK's, pushed by the agent (owner, 2026-11-23).
- **Live view** as Prusa Connect Local's generic camera: the agent serves each camera's MJPEG stream on
  its page, in the format of Prusa's ESP32-CAM firmware; the hub reads it only while somebody watches.
- **The address:** the agent names the stream `stream_url` (Prusa Connect Local's option) in the `config`
  of `POST /p/camera`, so a camera switched on in the agent's Cameras tab shows up in the hub by itself
  (owner, 2026-11-23: "het is toch gewoon een module in de agent die je aan/uit zet"; asked for the address, the owner chose "Met registratie mee").
- **Not through the agent's own WireGuard** (onetun forwards outwards only): there snapshots only (owner,
  2026-11-23: "Accepteren voor nu").
- `/c/stream`, `START_CAMERA_STREAM`, `STOP_CAMERA_STREAM` and their kwargs leave Part 5: the one
  addition for live view is `stream_url`.
- This is the one exception to the rule that the device dials the hub.

## Where it shows

[prusa-sdk-extension.md](../prusa-sdk-extension.md#part-5-a-usb-camera), [cameras.md](../cameras.md),
[links.md](../links.md) L10, [risks-and-open.md](../risks-and-open.md#oq-9-how-usb-camera-video-reaches-the-hub); #5, #96.
