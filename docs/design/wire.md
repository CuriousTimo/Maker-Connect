# Agent link: the Prusa Connect printer protocol

Every device on an agent talks to the hub exactly as a Prusa Connect printer talks to Prusa Connect
(owner, 2026-11-19; [decision](decisions/20261120-agent-link-is-prusa-connect-protocol.md)). The agent's
client is a port of Prusa-Connect-SDK-Printer; what the hub answers is what Prusa's own server answers.
Nothing on this wire is ours, except the additions of the
[proposed SDK extension](prusa-sdk-extension.md), which lie on top of it and are kept on that page,
apart from this one. What is ours beside the wire (where files live, the settings page) is marked
**ours**. File and line references below are to these trees:

| Role | Reference | Commit | Cited as |
|---|---|---|---|
| Client, ported by the agent | `reference/Prusa-Connect-SDK-Printer`, branch `websockets` | `c0d7d233d4ce2081e6c020d2e98fb92ad9b51521` | `__init__.py`, `ws.py`, `models.py`, `command.py`, `const.py`, `conditions.py` (all under `prusa/connect/printer/`) |
| How a program uses the client | `reference/Prusa-Link`, branch `websockets` | `171148ce7dbc7f1fb0633ea519523f3258877e2e` | `prusa/link/…` |
| Server answers, read only (OCL) | `reference/prusa-connect` `internal/connector/xbuddy` | `2e8c62b` | `xbuddy.go`, `wire.go`, `inbound.go`, `link.go`, `session.go`, `registration.go` |
| The client's WebSocket library | websocket-client, `>=1.6.0` (SDK `requirements.txt:6`) | not pinned by the SDK | only through the SDK's calls (`ws.py`) |

**How it is read.** `reference/Prusa-Connect-SDK-Printer` is checked out at `04c5bd59`; the SDK is read
at `c0d7d233` with `git -C reference/Prusa-Connect-SDK-Printer show c0d7d233:<path>` (or `git archive
c0d7d233`). Every SDK line below is a line at `c0d7d233`.

**Why `c0d7d233`.** PrusaLink takes the SDK from the branch `websockets` (`requirements.txt:1`,
`.github/workflows/python-tests.yml:51`, `README.md:68`), whose head is `c0d7d233`
(`git ls-remote`, 2026-11-20). `feat/ws-connect-local` @ `04c5bd59`, which PR #40's proof ran
(`crates/hub/src/devices/prusa_connect.rs:17`), is not pinned by PrusaLink and is not the reference
([decision](decisions/20261119-mk3-reference-prusalink-websockets.md)). Its four commits change exactly
the link's edges, so a proof at `04c5bd59` does not prove `c0d7d233`:

| Behaviour | `c0d7d233` (the reference) | `04c5bd59` (not the reference) |
|---|---|---|
| Subprotocol on the upgrade | none (`ws.py:50-57`) | `prusa-connect` |
| `401`/`403` on the upgrade | not read; dialled again for the next item | the token is marked bad, no dial until it changes |
| Retry pace | none: every item dials again (`__init__.py:804-806`) | backoff with jitter, at most 60 s |
| Events while the socket is down | dropped (`__init__.py:807-812`) | kept and sent later |
| Files over the socket | `T` frames ignored (`__init__.py:665-667`) | `START_INLINE_DOWNLOAD`, binary `T` frames |

## The unit: one printer per device

- Each device is one SDK `Printer`: its own serial (`sn`), fingerprint, type, registration, token and
  WebSocket, as PrusaLink runs one instance with one settings file per printer
  (`prusa/link/multi_instance/config_component.py:145-175`, `prusa/link/multi_instance/const.py:30`).
- The agent has no identity of its own on the wire. An agent without drivers hosts no `Printer` and so
  registers nothing. The first real registration and pairing come with the first device, the DYMO
  LabelManager PnP (owner, [OQ-49](risks-and-open.md#oq-49-registering-in-an-agent-without-drivers);
  [below](#the-first-device-dymo-labelmanager-pnp)).
- Without serial, fingerprint and type the `Printer` is not initialised: its telemetry goes without
  fields and every command is answered `REJECTED`, reason "Printer has not been initialized properly"
  (`__init__.py:91`, `:264-271`, `:354-358`, `:669-674`). Where a device's serial, fingerprint and type
  come from is its port's, designed with each driver (PrusaLink: fingerprint = SHA-256 of the serial,
  `prusa/link/util.py:153-159`, set in `prusa/link/printer_adapter/prusa_link.py:906-916`). What a
  device that is not a Prusa sends as its type is the [extension](prusa-sdk-extension.md#part-1-device-types)
  ([OQ-48](risks-and-open.md#oq-48-what-a-device-that-is-not-a-prusa-says-it-is)).

## Shape

```mermaid
sequenceDiagram
  participant A as Agent: SDK Printer of one device
  participant H as Hub
  participant P as Person
  A->>H: POST /p/register, JSON sn, fingerprint, printer_type, firmware
  H-->>A: 200, header Code
  A->>P: settings page shows the code
  loop every 1 s, at most 30 min
    A->>H: GET /p/register, header Code
    H-->>A: 202
  end
  P->>H: enters the code in Space, the hub approves it
  A->>H: GET /p/register, header Code
  H-->>A: 200, header Token
  A->>A: token written to the device's settings file
  Note over A: the first telemetry or event item opens the socket
  A->>H: GET /p/ws, Upgrade, Fingerprint, Token, Timestamp, User-Agent headers
  H-->>A: 101 Switching Protocols
  H-->>A: after 100 ms, J frame SEND_INFO
  A->>H: event ACCEPTED, then event INFO with the command_id
  A->>H: telemetry, a JSON object per text frame
  Note over A,H: the agent pings every 30 s and drops the socket without a pong in 10 s; the hub pings every 54 s and drops it after 60 s without a frame
```

## Registration (HTTP)

| Step | The agent sends (SDK) | The hub answers (xbuddy) | The agent then (SDK) |
|---|---|---|---|
| Begin | `POST /p/register` (`__init__.py:739`), JSON `{"sn", "fingerprint", "printer_type": "<type as a.b.c>", "firmware"}` (`:733-738`, `const.py:47-60`), the common headers (below) without `Token`, timeout 10 s (`const.py:6`) | `200` with header `Code` (`xbuddy.go:653-654`): 10 characters of `ACDEFGHJKMNPQRSTUVWXYZ2345679` (`registration.go:80-86`), usable 24 h (`registration.go:266`); `400` when the body is unreadable or has no fingerprint (`xbuddy.go:613-624`); `503` with `Retry-After: 54` when the table is full (`xbuddy.go:634-640`, `:128`); `500` (`xbuddy.go:641-645`) | on `200` keeps the code in memory (`Printer.code`, `__init__.py:117`, `:756`) and queues the poll (`:757`); on anything else raises with the body as its message, marks API bad and, on `5xx`, HTTP bad (`:744-754`); `Retry-After` is not read |
| Poll | `GET /p/register` (`models.py:91-92`) with header `Code` (`models.py:100-103`) and the common headers stamped at the poll's creation (`__init__.py:824-826`) | `202` while nobody approved (`xbuddy.go:670-671`); `404` for an unknown code (`:672-673`); `400` without a code (`:661-664`); `200` with header `Token` (`:676-678`) | `200`: takes `Token`, marks the token good, calls the register handler with it, forgets the code (`__init__.py:844-851`); `202`: polls again after 1 s (`:852-854`) until 30 min after the code was made (`models.py:18`, `:98`); anything else: the poll is dropped and the state set from the status (`:863-882`), and the code is kept, so the page still shows it as in progress |
| Not used | `GET /p/auth` (`xbuddy.go:483-484`, `auth.go:43-72`) is never called by the SDK at `c0d7d233` | | |

PrusaLink starts the registration from its page: it resolves the host name (`prusa/link/web/connection.py:105-108`),
opens `<server>/info` and stops if that fails (`:112-116`,
[OQ-47](risks-and-open.md#oq-47-the-info-probe-before-registering)), stores host, port and TLS (`:118-122`),
then calls `register()` (`:18-21`, `:124-127`). A person who already has a token gets `409` (`:93-94`).
The agent's page does these steps as PrusaLink's does, `409` included: **ours, following PrusaLink** (the
SDK has no page).

### How the code reaches the person

PrusaLink's page reads, per printer, `connect.registration` (`NO_REGISTRATION`, `IN_PROGRESS` while a code
exists, `FINISHED` once a token exists) and `connect.code` (`prusa/link/web/connection.py:46-56`, `:75-81`;
`const.py:179-183`), and links to `<server>/add-printer/connect/<type>/<code>` with name and location
(`connection.py:18-34`). The agent's page shows the same status and code per device. The person types the
code into Space's pairing page on the hub ([flows.md](flows.md#1-pairing-a-device-on-an-agent)). The
`add-printer` link is not offered: the hub has no such page (**ours**, a departure from PrusaLink's
`connection.py:22`; a hub page for it is a later choice).

## What the agent keeps

The SDK keeps the token only in memory (`Printer.token`, `__init__.py:83`, `:381-387`) and hands it to its
register handler (`:850`); storing it is the program's. So this whole section is **ours, following
PrusaLink** (the SDK's own consumer), listed for the owner's approval:

| What | Where, as PrusaLink | Reference |
|---|---|---|
| Hub host, TLS, port, token | the device's `prusa_printer_settings.ini`, `[service::connect] hostname`, `tls`, `port`, `token`; the token in the clear | `prusa/link/config.py:298-307`; written when registered, `prusa/link/printer_adapter/prusa_link.py:918-928`; read back with `connection_from_settings`, `prusa/link/sdk_augmentation/printer.py:60-71` |
| Type, name, location | the same file, `[printer] type`, `name`, `location`. The type is the device profile the person picks on the agent's page when turning on the device's module (**ours**, owner 2026-11-20; PrusaLink reads its type from the printer, `prusa/link/web/lib/wizard.py:188-191`) | `prusa/link/config.py:280-289` |
| Serial and fingerprint | not kept: read from the device at every start | `prusa/link/printer_adapter/prusa_link.py:906-916` |
| The registration code | memory only; lost at a restart | `__init__.py:117`, `:756` |
| Reset of a registration | the token set to `""` and the file written | `prusa/link/web/connection.py:136-150` |

**Ours:** where the files lie (one folder per device under the agent's state folder, each with its
`prusa_printer_settings.ini`); one hub address entered once for all devices and written into each file
(PrusaLink has one per instance); the file's mode
([OQ-36](risks-and-open.md#oq-36-the-mode-of-the-agents-credentials-file)). PrusaLink's
`[service::local]` (its own page's login, `config.py:309-315`) is not kept per device; the agent page's
login is [OQ-44](risks-and-open.md#oq-44-the-agent-settings-page-open-points).

## The WebSocket

- **URL:** the server address with `http://` made `ws://` and `https://` made `wss://`, plus `/p/ws`
  (`__init__.py:625-629`). The hub answers `GET /p/ws` only (`xbuddy.go:485-486`), other methods `405`
  (`:487-489`).
- **Headers,** stamped when the socket is dialled (`__init__.py:637`, `make_headers` `:273-292`), empty
  ones left out (`ws.py:45-48`): `Fingerprint`; `Timestamp`; `User-Agent: Prusa-Connect-SDK-Printer/0.9.0dev0`
  (`:39`, `:280`); `User-Agent-Printer` (the type as `a.b.c`); `User-Agent-Version` (software, else
  firmware); `Token`; `Clock-Adjusted: 1` once after the clock jumped (`:287-290`). No
  `Sec-WebSocket-Protocol` (`ws.py:50-57`). The hub offers `prusa-connect` (`xbuddy.go:61`, `:388`).
  That the hub takes an upgrade naming none, and must not refuse one, is **ours**: xbuddy does not say
  what it does then, and the SDK at `c0d7d233` never names one.
- **When it opens:** only when a telemetry or event item is to be sent and no socket is open or the token
  changed (`ensure_ws_connected`, `__init__.py:631-638`, called at `:805-806`). The dial waits up to 10 s
  for the open (`ws.py:69-70`, `const.py:6`). Each socket is `WebSocketApp.run_forever(ping_interval=30,
  ping_timeout=10)` in a daemon thread (`ws.py:59-67`), called with no reconnect argument; the SDK
  dials again only through `ensure_ws_connected`.
- **The hub's checks, before the upgrade:** credentials refused `401` (`xbuddy.go:696-702`); the hub does
  not accept the printer `403` (`:704-712`). A printer it knows with the same identity is reattached to
  its session, which drains the commands queued while it was away (`:714-718`, `:793-804`); one that comes
  back as something else gets a new session and the old one's queue is dropped (`:719-731`). A second
  connection replaces the first, which is closed without a close frame (`link.go:126-130`, `:424-429`).
- **Agent to hub:** text frames, one JSON object each, no header (`wire.go:13-15`), nulls left out
  (`models.py:39-55`).
  - Telemetry: the telemetry fields with `state` (`models.py:171-195`; `__init__.py:334-359`, which adds
    `command_id` while a command runs, `job_id`, and transfer fields during a transfer).
  - Event: `{"event", "source", "data"}` plus `command_id`, `job_id`, `transfer_id`, `reason`,
    `dialog_id` when set and `state` (`models.py:148-163`; `__init__.py:312-332`).
  - Without a token neither is made (`__init__.py:319-321`, `:341-343`).
  - The hub reads a frame with an `event` key as an event, one with `"transfer": "inline"` as a block
    request, anything else as telemetry (`wire.go:237-271`); at most 1 MiB a frame (`session.go:63-64`,
    `link.go:70`); a frame it cannot read, or a binary one, is dropped and the connection kept
    (`inbound.go:189-205`, `:448-456`).
- **Hub to agent:** a type letter, the id as 8 hex digits, then the payload (`wire.go:13-19`, `:52-58`):
  `J` JSON `{"command": <name>, "kwargs": {…}}` (`wire.go:60-74`), `G` G-code, `F` forced G-code, `T` a
  file chunk (`wire.go:23-31`). The SDK reads (`__init__.py:640-724`): shorter than 9 characters, logged and
  dropped (`:649-651`); an id that is not hex, `REJECTED` "Invalid command_id" without an id
  (`:654-661`); `T` ignored (`:665-667`); not initialised, `REJECTED` (`:669-674`); `G` and `F` become
  `GCODE` with `force` for `F` (`:676-693`); `J` is parsed, `REJECTED` "Invalid JSON body" if it is not
  JSON (`:695-705`); then checked and accepted (`:707-715`); any other letter is logged (`:723-724`).
- **A command's life** (`command.py`): accepted, event `ACCEPTED` (`:99-117`); while another runs,
  `REJECTED` "Another command is running" (`:76-83`); the same id again, its current or last state again
  (`:84-96`). A runner thread calls it (PrusaLink `prusa/link/sdk_augmentation/command_handler.py:23-34`):
  the handler's answer ends it `FINISHED` or with the event the handler names (`:146-157`); a
  `CommandRejected`, `REJECTED` with its reason (`:190-191`); any other error, `FAILED` "Command error"
  (`:192-198`); an unknown name `REJECTED` "Unknown command", one without a handler "Not Implemented"
  (`:180-186`). The hub matches an event's `command_id` to the command in flight (`inbound.go:235-266`).
- **The SDK's own handlers,** in every `Printer` before any port adds one (`__init__.py:137-156`). In this
  step only `SEND_INFO` is in use: it answers event `INFO` with type, version, subversion, firmware, SDK
  version, network info, API key, storages, serial, fingerprint, MBL, sheets and MMU (`:393-434`). The
  hub sends `SEND_INFO` 100 ms after every attach (`link.go:134`, `:148-171`; `inbound.go:44-47`).

## Ping, silence and a dropped socket

- **The agent** pings every 30 s and closes the socket when no pong comes within 10 s (`ws.py:62-63`).
- **The hub** pings every 54 s, bounds each write to 10 s, and re-arms a 60 s read deadline on every
  frame, ping and pong (`inbound.go:32-42`; `link.go:72-87`, `:241-250`, `:301-325`).
- **A dropped socket:** the socket's thread ends and the `Printer` is no longer connected (`ws.py:30-34`).
  The next telemetry or event item dials again (`__init__.py:805-806`); there is no other retry and no
  backoff. An item that finds no socket is dropped with "WS not connected, dropping item" and the
  Internet condition goes bad (`:807-812`); a failed send marks HTTP bad (`:813-821`). The queue is in
  memory (`:135`): nothing is kept for later. The hub keeps the session and its queue (`link.go:327-353`)
  and drains it on the next attach (`xbuddy.go:793-804`).

## When the hub refuses

- **`401` or `403` on the upgrade:** websocket-client fails the handshake; the SDK logs it and stops
  waiting (`ws.py:107-109`). The socket is not connected, so the item is dropped and the Internet
  condition goes bad (`__init__.py:806-812`). The token is kept and the next item dials again. The SDK
  does not read the status of a refused upgrade; it reads `403` only on its HTTP path
  (`deduce_state_from_code`, `:869-882`), which at `c0d7d233` carries only the registration poll and
  `/p/camera` (`:823-867`).
- **The way out** is the person's: "reset registration" clears the token
  (`prusa/link/web/connection.py:136-150`); the device then registers again.
- **What the page shows** for the link is the worst of the SDK's conditions, each with its own message:
  Internet, HTTP (`5xx`), Token (no valid token), API (`4xx`) (`conditions.py:266-282`; PrusaLink
  `prusa/link/conditions.py:114-122`).

## The first device: DYMO LabelManager PnP

The first device registered and paired over this protocol (owner, 2026-11-20,
[decision](decisions/20261120-dymo-first-device-and-sdk-extension.md)). Its port is labelle, already ported
and audited (`crates/engine/src/ports/labelle`, [audit](../audits/dymo-labelmanager.md); adapter
`crates/engine/src/device/labelmanager.rs`). On the wire it is an SDK `Printer` like any other, plus the
additions of the [extension](prusa-sdk-extension.md#part-2-a-tape-label-printer-dymolabelmanager-pnp): its type `dymo.labelmanager-pnp`, three telemetry keys and the kwarg `tape_size_mm`. The
rest is choosing values the protocol already has, all **ours**:

- **Serial and fingerprint.** The port reads no serial (`labelmanager.rs:64-72`), but the SDK needs one
  (`__init__.py:264-271`) and the hub needs a fingerprint (`xbuddy.go:620-624`). So the agent draws a random
  serial once, keeps it in the device's settings file, and makes the fingerprint from it as PrusaLink does
  (SHA-256 of the serial, `prusa/link/util.py:153-159`). In this step an agent has at most one LabelManager
  PnP.
- **State:** the port's idle is `IDLE` and its busy is `PRINTING` (`port.rs:138`, `:179`;
  `const.py:24-34`).
- **A label print** comes as `START_CONNECT_DOWNLOAD` with `printing: true` and `tape_size_mm`. The download's end is
  the SDK's own `TRANSFER_FINISHED` (`download.py:363-368`); then the port prints the label. The file lands in the storage the device attaches with `Printer.attach`
  (`__init__.py:888-904`), which `path` must point into (`download.py:314-334`). A label that
  fails is reported as a `STATE_CHANGED` event with state `ERROR` and the port's reason in `reason`
  (`models.py:148-163`), and then `IDLE`.
- **Gone and back:** while the port says the printer is gone, the agent sends nothing for it, so its
  socket ends and the hub sees it as away, as it sees any printer that went away (`inbound.go:18-35`).
  When it is back, telemetry dials again.
- **Pause and resume:** there is no handler, so the SDK answers `REJECTED` "Not Implemented"
  (`command.py:184-186`).
- **Dialogs:** `DIALOG_ACTION` is the SDK's own handler (`__init__.py:156`, `:519-524`): `FINISHED`, or
  `FAILED` "Command error" without kwargs. The device raises no dialogs.

## Removed

Our own link, which the SDK does not have, is gone; none of it is carried over:

- the path `/api/agent/v2/ws?stream=<uuid>`, the subprotocol `maker-connect-agent.v2` and
  `Authorization: Bearer` with an agent token;
- the agent token and the agent as the link's identity; the stream uuid, `409 agent/STREAM_IN_USE`;
- `POST /api/agent/v2/register`, `GET /api/agent/v2/register/{id}`, the registration secret,
  `RegisterRequest`, `RegisterAnswer`, `CollectAnswer`;
- the `up` message and its items `hello`, `patch`, `result`, `job_end`, `offline`, `camera_frame`; the
  agent-wide `seq`, `ack`, the outbox, `agents.up_seq`, the taken command ids on disk;
- the down messages `ack`, `command`, `job`, `answer`, `update`;
- `GET /api/agent/v2/files/{id}` and `GET /api/agent/v2/update/{tag}/{file}`;
- the shared types `AgentInfo`, `Identity`, `CapabilitySet`, `Verb`, `Arg`, `StateSet`, `Patch` and the
  schema generated from them;
- the agent's ping after 15 s of silence (Buddy's, `reference/buddy-6.4.0` `src/connect/connect.cpp:49-50`, not the SDK's).

A result that arrives after its record was settled `unknown` stays a hub rule on `commands`
([database.md](database.md#devices-and-commands)); it is not part of the wire.

## Not in this step

Each comes with the driver that needs it, as the SDK has it: files beyond the LabelManager's
`START_CONNECT_DOWNLOAD` (`START_URL_DOWNLOAD`, `__init__.py:436-455`);
cameras (`/p/camera`, `models.py:198-224`; [links.md](links.md) L10, live view in the [extension](prusa-sdk-extension.md#part-5-a-usb-camera)); the commands each port handles; how often a device sends
telemetry (PrusaLink's pace, `prusa/link/const.py:67-71`). The agent's own update:
[OQ-46](risks-and-open.md#oq-46-how-the-agents-own-update-reaches-it).

## Not settled

- websocket-client's exact version: the SDK asks for `>=1.6.0` only (`requirements.txt:6`); what
  `run_forever` does underneath is not read from any other source here.
- The proof: PR #40's test ran `04c5bd59`; it is run again with `c0d7d233` in the build step, and the
  audit compares what the SDK and the agent's port send to the hub.
