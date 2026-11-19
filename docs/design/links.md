# Links

Every link between two processes: what carries it, which way it goes, how the caller proves who it
is, and which reference it follows. Flows over these links are in [flows.md](flows.md); endpoints in
[api.md](api.md); the agent's messages in [wire.md](wire.md).

## The links

| # | From → to | Transport | Paths | Authentication | Reference | Source |
|---|---|---|---|---|---|---|
| L1 | Browser → hub | HTTP/1.1 | pages, `/api/v1/*` | session cookie (HttpOnly) set at sign-in (a further Origin check: [OQ-30](risks-and-open.md#oq-30-origin-check-on-state-changing-requests)) | ours | plan §2.6, §2.9 |
| L2 | Hub → browser | SSE over HTTP | `/api/v1/events` | session cookie, or an API token in its own space | ours (event envelope) | plan §2.4 |
| L3 | Hub → browser | WebSocket, binary fMP4 fragments, MSE player | `/api/v1/cameras/{id}/ws` | session cookie; `device.view` on the camera checked before the upgrade | go2rtc v1.9.15 `internal/mp4/ws.go`, `www/video-rtc.js` | cameras.md |
| L4 | External app → hub | HTTP | `/api/v1/*` | `Authorization: Bearer mct_…`: an API token, stored hashed, with scopes, expiry, last use and the person's generation | ours | plan §2.6, §2.8 |
| L5 | Spoolman client → hub | HTTP + WebSocket | `/spoolman/api/v1/*` | open: [OQ-7](risks-and-open.md#oq-7-how-spoolman-clients-authenticate) | Spoolman `eccb05e` with base path `/spoolman` | plan §2.8 |
| L6 | Agent ↔ hub | WebSocket, one per agent, subprotocol `maker-connect-agent.v2`, JSON text messages both ways: upstream items and acks, commands, jobs, answers, updates; ping and pong | `/api/agent/v2/ws?stream=<uuid>` | `Authorization: Bearer` agent token on the upgrade, checked before it: hashed, may only report and fetch | ours, shaped after Buddy 6.4.0 and prusa-connect's `/p/ws` ([decision](decisions/20261119-agent-link-one-websocket.md)) | plan §2.3 |
| L7 | (not used) | folded into L6: the hub's downstream travels on the agent's WebSocket | | | | |
| L8 | Agent → hub, pairing | HTTP | `/api/agent/v2/register` | none to ask; the registration secret to collect the token; a person enters the pairing code in Space | ours (registering as today, agent-engine.md O7) | plan §2.3, §2.7 `device_registrations` |
| L9 | Agent → hub, files | HTTP GET | `/api/agent/v2/files/{id}`, `/api/agent/v2/update/{tag}/{file}` | agent token | ours | agent-engine.md O6, O7 |
| L11 | Prusa printer, Buddy 6.4+ → hub | HTTP + WebSocket | `/p/register`, `/p/auth`, `/p/ws` | `Code` header while registering; then `Fingerprint` and `Token` headers on the upgrade, `Sec-WebSocket-Protocol: prusa-connect` (`reference/buddy-6.4.0` `src/connect/connect.cpp:151-189`); the only Buddy dialect ([decision](decisions/20261119-buddy-websocket-dialect-only.md)) | Buddy 6.4+; server side read in `reference/prusa-connect` `internal/connector/xbuddy` | plan §2.2 |
| L12 | Hub → network camera | RTSP over TCP | the camera's stream URL | the camera's user and password, kept by the hub | retina `2b6b7fe` (0.4.22) | cameras.md |
| L13 | Agent → device | USB HID, USB printer class, serial, CUPS | per port | none (local) | the port's one reference (device-contract.md) | `CLAUDE.md` |
| L14 | Hub → database | PostgreSQL wire, TLS off on `127.0.0.1` or a Unix socket | `DATABASE_URL` | the hub's login role, neither superuser nor table owner ([database.md](database.md#roles-and-pools)) | sqlx 0.9 | plan §2.6, §2.7 |
| L15 | Hub → dump tools | child process, stdin/stdout | `pg_dump -Fc`, `pg_restore`, `psql` | `PGPASSWORD` in the child's environment, never on the command line | PostgreSQL client of the server's major version | [decision](decisions/20261119-pg-client-no-docker-rights.md) |
| L16 | Hub → release host | HTTPS GET | the release list, `SHA256SUMS`, `SHA256SUMS.sig`, the build | none; the build is taken only when the Ed25519 signature over `<tag>\n<SHA256SUMS>` holds and the file's sum is listed | ours (`crates/update`) | storage-ops.md |
| L17 | Hub → SpoolmanDB | HTTPS GET | the public catalogue | none | SpoolmanDB | hub-modules.md A3 |
| L18 | Hub → event bus → subscribers | in-process broadcast | none | in-process | ours | plan §2.4 |
| L19 | Hub page → hub | same origin | `/laser-design/*` (LaserFoundry web fork, gzip in the binary) calling `/api/v1` | session cookie | LaserFoundry v0.5.0 fork | hub-side.md HS13 |

Not in v2 (parked by the plan, §3): the SimplyPrint/Moonraker route for Klipper, the MK3 PrusaLink
port (the MK3 joins over `/p/ws`), out-of-process drivers over Prusa's plugin wire. Open: how USB
camera frames reach the hub ([OQ-9](risks-and-open.md#oq-9-how-usb-camera-video-reaches-the-hub)),
door access hardware ([OQ-10](risks-and-open.md#oq-10-door-access-link)).

## Who talks to whom

```mermaid
flowchart LR
  browser["Browser"]
  app["External app"]
  spool["Spoolman client"]
  agent["Agent"]
  xl["Buddy 6.4+"]
  hub["Hub"]
  db[("Database")]
  cam["RTSP camera"]
  rel["Release host"]
  sdb["SpoolmanDB"]
  browser -->|"L1 cookie"| hub
  hub -->|"L2 SSE"| browser
  hub -->|"L3 WS video"| browser
  app -->|"L4 Bearer mct_"| hub
  spool -->|"L5"| hub
  agent <-->|"L6 WebSocket, L8 pair, L9 files"| hub
  xl -->|"L11 /p/ws"| hub
  hub -->|"L12 RTSP"| cam
  hub -->|"L14 login role"| db
  hub -->|"L16 HTTPS"| rel
  hub -->|"L17 HTTPS"| sdb
```

## Rules on every link

- What the request log redacts is [OQ-31](risks-and-open.md#oq-31-log-redaction-and-urls-a-device-hands-the-hub).
- A secret is stored hashed where the other side only presents it (sessions, API tokens, agent
  tokens, registration secrets, invites). Secrets the hub must present itself (a camera's password)
  are stored as the owner decided for cameras (plain, cameras.md, 2026-11-16).
- Every request that changes data runs in one transaction with `SET LOCAL app.space_id` for the
  caller's space ([authz.md](authz.md#tenancy)).
- Whether the hub fetches a URL a camera or a printer hands it: [OQ-31](risks-and-open.md#oq-31-log-redaction-and-urls-a-device-hands-the-hub).
