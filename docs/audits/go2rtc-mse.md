# Audit: network cameras over MSE (go2rtc)

Reference: go2rtc v1.9.15 (`7c05066c90ab6ad1fccb506d5f0408aa31ebe60f`, AlexxIT/go2rtc, MIT), `reference/go2rtc`.
Ours: `crates/hub/src/cameras/iso.rs`, `cameras/mp4.rs`, `cameras.rs` (`serve_mse`), the player
`cameras/video-rtc.js` (go2rtc's `www/video-rtc.js`, unchanged: same SHA-256).
Checker: `scripts/go2rtc-mse-audit-check.py` (records frames of a real camera through retina with
`crates/hub/examples/mse_record.rs`, runs go2rtc's own `pkg/mp4.Muxer` and `ContentType` on the same frames with
`scripts/go2rtc-mse-check/main.go`, compares record by record).

| Item | Reference | Ours | Result |
|---|---|---|---|
| Content type (`mse` answer) | `pkg/mp4/mime.go` `ContentType`, `pkg/h264/h264.go` `GetProfileLevelID` | `mp4::content_type` | identical |
| Init segment (`ftyp`+`moov`) | `pkg/mp4/muxer.go` `GetInit`, `pkg/iso` | `Muxer::get_init`, `iso.rs` | identical |
| Fragments (`moof`+`mdat`), 200 frames | `pkg/mp4/muxer.go` `GetPayload` | `Muxer::get_payload` | identical (200/200) |
| Start-of-access-unit filter | `pkg/h264/rtp.go:52-75` | `mp4::start_of_access_unit` | unit tests per case (`cameras::mp4::tests`) |
| `EncodeConfig`/`DecodeConfig`, profile sanitising | `pkg/h264/mpeg4.go`, `h264.go` | `mp4.rs` | unit tests; the profile also by the content type above |

Run 2026-11-16 on the LSC Smart Connect indoor camera (`/Streaming/Channels/101`, H.264 Main 4.0, 1920x1080):
202/202 identical (content type, init segment, 200 fragments).

Not covered:
- Ours, not go2rtc's: a new viewer first gets the frames since the last key frame (go2rtc's viewer waits for the
  next one); the fragments are still made by the ported muxer, one muxer per viewer as go2rtc's.
- The websocket exchange (`internal/mp4/ws.go`, `internal/api/ws/ws.go`): ported by reading; not run against go2rtc.
- The player: go2rtc's file unchanged, so not compared; it was seen playing in Chrome and Safari over plain HTTP.
- The filter on real frames: the frames given to both muxers are already filtered (go2rtc's filter sits in its RTP
  depacketizer, which is retina's job here); the filter itself only by unit tests.
- Width and height: ours from retina, go2rtc's from its SPS decoder; equal for this camera (the init is identical),
  other cameras not yet.
- H.265, audio, go2rtc's other modes: not ported.
