# Audit: network cameras found by ONVIF (go2rtc)

Reference: go2rtc v1.9.15 (`7c05066c90ab6ad1fccb506d5f0408aa31ebe60f`, AlexxIT/go2rtc, MIT), `reference/go2rtc`,
`pkg/onvif/client.go`, `envelope.go`, `helpers.go`.
Ours: `crates/hub/src/cameras/onvif.rs` (the port, function by function), `cameras.rs` (`resolved`, called by `open`).
Checker: `scripts/go2rtc-onvif-audit-check.py` (every go2rtc function named in the port, every cited line range
real, the test cases extracted from go2rtc's `onvif_test.go`, go2rtc's own tests run, go2rtc's own client run by
`crates/hub/src/cameras/onvif_fixtures/make-fixtures.sh` against a local server with the nonce and time fixed, and
our tests run against the same).

| Item | Reference | Ours | Result |
|---|---|---|---|
| Requests to the camera, with and without a user: `GetCapabilities`, `GetProfiles`, `GetStreamUri`, `GetSnapshotUri`, `GetVideoSourceConfiguration`, `GetProfile`, `GetServiceCapabilities`, `GetDeviceInformation`, `GetServices` (18) | `client.go` `NewClient`, `GetURI`, the `Get*` calls, `Request`; `envelope.go` | `Client`, `Envelope` | identical (18/18: method, path, content type, user agent, body with the WS-UsernameToken digest) |
| The RTSP address read from the answer, `TestGetStreamUri` (7 cases) | `FindTagValue`, `html.UnescapeString`, `url.Parse`/`String` | `find_tag_value`, `html_unescape`, `url::Url` | identical (7/7) |
| The media address read from `GetCapabilities`, `TestGetCapabilities` (2 cases) | `FindTagValue` | `find_tag_value` | identical (2/2) |
| The profile picked by `subtype` (empty, an index, a wrong index, not a number) | `GetURI`, `atoi` | `get_uri`, `atoi` | unit test `subtype_picks_the_profile` (cases read from the source) |
| `atoi`, `GetPath`, the created time, the nonce's symbols | `helpers.go`, `time.RFC3339Nano`, `core.RandString` | `atoi`, `get_path`, `rfc3339_nano`, `rand_string` | unit tests per case |
| The answer's RTSP address handed to retina | n/a (ours) | `cameras::resolved` | test `resolved_addresses` |

Not covered:
- Ours, not go2rtc's: when the `onvif://` address has no user, the camera form's user and password are put in it
  (go2rtc reads them from the address only); the user and password in the RTSP address that comes back are split
  off and given to retina as credentials (retina refuses them in the URL, `client/mod.rs:1436`).
- `html.UnescapeString`: numeric references and the Windows-1252 table ported; of the named entities only `amp`,
  `lt`, `gt`, `quot` (also upper case and without `;`) and `apos;`. Another named entity stays as written.
- Go's `url.Parse`/`String` is the `url` crate: the test cases are identical; spaces, non-ASCII and rare characters
  in a user or password may be written differently. Go's `Query()` drops pairs with `;`, the `url` crate does not.
- `GetPath` on a relative address such as `foo/bar`: go2rtc recurses without end, ours gives the default path.
- HTTP: `ureq` instead of Go's `net/http`. ureq also sends `Accept: */*`, not Go's `Accept-Encoding: gzip`;
  5 redirects instead of 10; transport errors read differently. Not in the compared requests (the harness records
  method, path, content type, user agent and body).
- Not ported: `server.go` (go2rtc as an ONVIF server), `Appendf`, `DiscoveryStreamingDevices`, `UUID`, `findScope`,
  `GetPosixTZ`.
- Not tried on a real ONVIF camera.
