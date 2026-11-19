# API

The hub's HTTP surface by namespace (plan §2.8). The reference for every field is the generated
`docs/api/openapi.json` (utoipa from the Rust types and handler attributes, checked in, a test fails on
drift; `oasdiff` in CI labels each change additive, neutral or breaking, and a breaking change needs a
line in a `CHANGES.md` in `docs/api/` (made with the first breaking change)). This chapter is the map. Exact paths and field names of our own
endpoints are this design's proposal until the spec is generated in step 6
([OQ-1](risks-and-open.md#oq-1-exact-paths-and-fields-of-our-own-endpoints)). Actions and the gates
`public`, `session`, `space.member` and `instance.admin` are those of [authz.md](authz.md#actions);
every route names one or the other.

## Namespaces

| Namespace | Who calls it | Auth | In the public spec |
|---|---|---|---|
| `/api/v1` | the hub's own pages, external apps | session cookie or `Bearer mct_…` | yes |
| `/api/v1/events` | pages, external apps | session cookie (gate `space.member`) or `Bearer mct_…` in its own space | yes (SSE) |
| `/api/v1` 3D-printer REST | Prusa Connect clients, PrusaSlicer | as `/api/v1` | yes, mirroring Prusa Connect's ([OQ-2](risks-and-open.md#oq-2-the-3d-printer-rest-surface)) |
| `/spoolman/api/v1` | Spoolman clients | [OQ-7](risks-and-open.md#oq-7-how-spoolman-clients-authenticate) | no: Spoolman's own API, to the letter |
| `/api/agent/v2` | agents: registration and files over HTTP, the link over one WebSocket (`/api/agent/v2/ws`) | registration secret, then agent token | no: ours, [wire.md](wire.md) |
| `/p/*`, `/f/{iv}/raw` | Prusa printers | Buddy's own | no: Buddy's protocol |
| page routes | browsers | session cookie | no; GET only: no page route changes data (plan step 7) |

## Conventions (`/api/v1`)

- Nouns; `GET` reads, `POST` creates or acts, `PATCH` changes, `DELETE` removes.
- **Keyset paging** on every list: `?limit=` and a cursor; the answer carries the items and the
  next cursor.
- **Errors:** a stable body `{"code": "domain/REASON", "text": "…"}` ([decision](decisions/20261119-repeated-commands-and-error-body.md)) with the HTTP status of the
  [fault code](../registries/fault-codes.md); further fields per code in the spec.
- **Idempotency:** a command or transfer carries an idempotency key (plan §2.4); a repeat with the same key returns the first record ([decision](decisions/20261119-repeated-commands-and-error-body.md)).
- **Times** RFC 3339 UTC; **ids** uuid strings (Spoolman's: integers).
- A page uses exactly these endpoints; nothing a page can do is missing from the API (plan §2.9).
- **The space of a request** is the session's chosen space (`sessions.space_id`, set by
  `PUT /api/v1/session/space`) or the token's space (`api_tokens.space_id`); every tenant row it reads
  or writes is in that space's transaction. A path that names a space (`/spaces/{id}/…`) must name
  that space; another id gets `404 space/NOT_FOUND`, whatever the caller's role there. Likewise
  `/organisations/{id}` must name the organisation of the request's space, else
  `404 organisation/NOT_FOUND`. Only the
  `public` routes, the `session` routes (including `PUT /api/v1/session/space`, which chooses the
  space) and the `instance.admin` routes run outside a space.
- **Tokens** reach action routes in their own space only; the gates `session`, `space.member` and
  `instance.admin` and the `grants.manage` routes are a person's session ([authz.md](authz.md#the-decision-on-a-request)).

## `/api/v1`: ours

| Method | Path | Action | Request | Answer |
|---|---|---|---|---|
| POST | `/api/v1/session` | `public` (sign-in) | `{name, password}` | 204, `Set-Cookie` |
| DELETE | `/api/v1/session` | `session` | — | 204 |
| PUT | `/api/v1/session/space` | `space.member` of that space: a membership in it, or an organisation membership in its organisation | `{space_id}` | 204 |
| GET | `/api/v1/me` | `session` | — | `{user, spaces[], roles, actions}` |
| PUT | `/api/v1/me/password` | `session` | `{old, new}` | 204 |
| POST | `/api/v1/me/tokens/end` | `session` | — | 204; bumps the generation, every token of the person ends |
| GET | `/api/v1/spaces` | `session` | — | the person's spaces: their memberships and every space of the organisations they have a role in |
| POST | `/api/v1/spaces` | `organisation.manage` (an organisation role only, [decision](decisions/20261119-organisation-actions-by-organisation-role.md)) | `{name, kind}`; `kind` `makerspace`, `office` or `private` gives the space's first area rights | 201 space |
| PATCH | `/api/v1/spaces/{id}` | `space.manage` | `{name?, ppe_checks?, host_label?, kind?}`; `host_label` free text (empty: "host"); a new `kind` changes no existing right ([authz.md](authz.md#area-rights-per-space)) | space |
| GET | `/api/v1/spaces/{id}/area-rights` | `space.manage` | — | the space's kind, and per movable area action (`files.view`, `queue.view`, `farm.view`, `farm.use`, `stock.view`, `stock.use`) the lowest role that holds it ([authz.md](authz.md#area-rights-per-space)) |
| PUT | `/api/v1/spaces/{id}/area-rights` | `space.manage`, a person's session only (never a token, `authz/SESSION_ONLY`) | `{rights: {<action>: <role>}}`, `<role>` one of `guest`, `member`, `host`, `admin`; a `.use` never `guest` and never below its area's `.view` | area rights; refused for an action that does not move (a `.manage` action, `organisation.manage`, a device action, `space.members.view`, `grants.view`, `audit.view`), a role outside that list, a `.use` at guest or below its `.view` (`validation/ARGUMENT_NOT_SUPPORTED`, [OQ-42](risks-and-open.md#oq-42-area-rights-open-points)); one transaction writes the rows and an audit row per change and bumps `authz_epoch` ([decision](decisions/20261119-host-role-and-area-rights.md)) |
| PATCH | `/api/v1/organisations/{id}` | `organisation.manage` (an organisation role only) | `{name}` | organisation |
| GET | `/api/v1/spaces/{id}/members` | `space.members.view` | — | members with roles (`guest`, `member`, `host`, `admin`, `owner`) and the space's `host_label` |
| PATCH | `/api/v1/spaces/{id}/members/{user}` | `space.members.manage`; the role at most the caller's own ([decision](decisions/20261119-no-self-elevation.md)) | `{role}`: `guest`, `member`, `host`, `admin` or `owner` | member |
| DELETE | `/api/v1/spaces/{id}/members/{user}` | `space.members.manage` | — | 204 |
| GET, POST | `/api/v1/spaces/{id}/invites` | `space.members.manage`; the role at most the caller's own, `whole_organisation` only with an organisation role | `{role, whole_organisation, expires_at}`, `role` as in members | invites; on POST the link once |
| POST | `/api/v1/invites/accept` | `public`; the handler checks the invite token | `{token, name, display_name, password}` for a new account, `{token}` when signed in | 201 the membership, `Set-Cookie` for a new account; already a member: `authz/ALREADY_MEMBER` ([decision](decisions/20261119-invite-accept.md)) |
| DELETE | `/api/v1/spaces/{id}/invites/{invite}` | `space.members.manage` | — | 204 (revoked) |
| GET, POST | `/api/v1/tokens` | `tokens.manage`; POST from a session only | `{name, scopes, expires_at}` | tokens; on POST the secret once; a token's rights are its scopes intersected with its person's at each request |
| DELETE | `/api/v1/tokens/{id}` | `tokens.manage` | — | 204 (revoked) |
| GET | `/api/v1/spaces/{id}/audit` | `audit.view` | cursor | audit rows |
| GET | `/api/v1/grants` | `grants.view` | `?user=&device=&group=` | grants |
| POST | `/api/v1/grants` | `grants.manage`, in Space only, a person's session only (never a token); a host's grant holds neither `device.manage` nor a damaging action and is never to the host's own row, refused in the no-self-elevation family ([authz.md](authz.md#presets-roles-and-grants)) | `{user, device or device_group, actions, expires_at, note?}`; `note`: free text, for example an induction given | 201 grant |
| DELETE | `/api/v1/grants/{id}` | `grants.manage`, session only; a host only a grant they could have given (no `device.manage`, no damaging action) | — | 204 |
| GET, POST | `/api/v1/device-groups` | `grants.view`; POST `grants.manage`, session only | `{name}` | groups |
| PATCH, DELETE | `/api/v1/device-groups/{id}` | `grants.manage`, session only | `{name?, devices?}` | group |
| POST | `/api/v1/access-requests` | `space.member` | `{device_id}` | 201; grants nothing (#11) |
| GET | `/api/v1/access-requests/mine` | `space.member` | `?state=` | the caller's own requests in the space, with when they were asked and handled (UI study, journal:UI-DESIGN:0009 Q5: a member reads their own requests) |
| DELETE | `/api/v1/access-requests/{id}` | `space.member`, the request's own person, while `state` is `open` | — | 204; the row stays with `state` `withdrawn`, so staff see it was taken back (UI-DESIGN:0009 Q5: withdraw) |
| GET | `/api/v1/access-requests` | `grants.view` | `?state=` | requests |
| PATCH | `/api/v1/access-requests/{id}` | `grants.manage`, session only (a host's too: [authz.md](authz.md#presets-roles-and-grants)) | `{state: granted or refused}` | request; a grant is made only through `/grants` |
| GET | `/api/v1/devices` | `device.view` per row | `?kind=&group=` | devices with state name, capability epoch, current job |
| GET | `/api/v1/devices/{id}` | `device.view` | — | device with its state tree, capabilities, state set, settings |
| PATCH | `/api/v1/devices/{id}` | `device.manage` | `{name?, settings?}` (laser: origin, dialect, rotary, M4: #24) | device |
| DELETE | `/api/v1/devices/{id}` | `device.manage` | `{reason}` | 204 (retired) |
| POST | `/api/v1/devices/pair` | `devices.manage` | `{code, name}` | 201 device ([flows.md](flows.md#1-pairing-a-device-on-an-agent)) |
| POST | `/api/v1/devices/{id}/commands` | the verb's action ([registry](../registries/capability-verbs.md)) | `{verb, args, idempotency_key}` | 202 command record, or the fault |
| GET | `/api/v1/devices/{id}/commands` | `device.view` | cursor, `?open=` | command records |
| GET | `/api/v1/commands/{id}` | `device.view` on its device | — | command record |
| POST | `/api/v1/devices/{id}/transfers` | `job.start` | `{file_id, path, print_after, idempotency_key}` | 202 transfer record |
| GET | `/api/v1/devices/{id}/transfers` | `device.view` | cursor | transfer records |
| POST | `/api/v1/devices/{id}/jobs` | `job.start` | `{file_id}`; a laser job is loaded, not started | 201 job |
| POST | `/api/v1/devices/{id}/ppe` | `device.view` and a person's session | `{items[]}` | 204; kept in memory for the session |
| GET | `/api/v1/devices/{id}/history` | `device.view` | `?metric=&from=&to=&bucket=raw or 1m or 1h` | samples and gaps |
| GET | `/api/v1/lasers` | `device.view` | — | lasers with their settings, for the laser design page (#24) |
| GET, POST | `/api/v1/files` | `files.view`; POST `files.manage` | multipart upload | files; 201 file |
| GET | `/api/v1/files/{id}` | `files.view` | — | file with what it says about itself |
| GET | `/api/v1/files/{id}/content`, `/thumbnail` | `files.view` | — | bytes |
| DELETE | `/api/v1/files/{id}` | `files.manage` | — | 204 |
| GET | `/api/v1/jobs` | `device.view` per row | `?device=&person=&file=&from=&to=` cursor (#11) | jobs |
| GET | `/api/v1/jobs/{id}` | `device.view` | — | job with its command records |
| POST | `/api/v1/jobs/{id}/confirm` | `job.control` | `{good}` (farm: Set Ready, Bad print) | job |
| GET, POST | `/api/v1/queue` | `queue.view`; POST `queue.manage` and `job.start` on that device: the item is started on its person's behalf, and the dispatcher checks `job.start` of that person again before each start ([flows.md](flows.md#3-a-job-with-the-laser-rule)) | `{device_id, file_id}` | queue items |
| PATCH, DELETE | `/api/v1/queue/{id}` | `queue.manage`; own items for their person | `{pos}` | item |
| GET, POST | `/api/v1/projects` | `farm.view`; POST `farm.use` | `{name, description, priority}` | projects |
| PATCH, DELETE | `/api/v1/projects/{id}` | `farm.use` | fields | project |
| GET, POST | `/api/v1/projects/{id}/parts` | `farm.view`; POST `farm.use` | `{name, target}` | parts |
| PATCH, DELETE | `/api/v1/parts/{id}` | `farm.use`; `{count}` needs `farm.manage` | fields, `{count}` | part |
| GET, POST, DELETE | `/api/v1/parts/{id}/files[/{file}]` | `farm.view`; POST, DELETE `farm.use` | `{profile, file_id, per_plate, material, color, sheet}` | part files |
| GET | `/api/v1/parts/{id}/ledger` | `farm.view` | cursor | ledger rows |
| GET, POST | `/api/v1/locations` | `stock.view`; POST `stock.use` | `{id, name, parent, slots}` | locations |
| PATCH, DELETE | `/api/v1/locations/{id}` | `stock.use` | fields | location |
| GET, POST | `/api/v1/materials`, `/api/v1/items` | `stock.view`; POST `stock.use` | not designed (#10, [OQ-14](risks-and-open.md#oq-14-tables-of-materials-items-and-labels)) | — |
| GET | `/api/v1/items/{id}/movements` | `stock.view` | cursor | movements |
| GET, POST | `/api/v1/cameras` | per row: `device.view` on the device the camera watches, or for a room camera the role `room_view_role` names (today's rule, [authz.md](authz.md#actions)); POST `cameras.manage` | `{name, url, username, password, watches_device_id, room_view_role}` | cameras; the password never comes back |
| PATCH, DELETE | `/api/v1/cameras/{id}` | `cameras.manage` | fields; an empty password keeps it | camera |
| GET | `/api/v1/cameras/{id}/ws` | the same rule as the row, checked before the upgrade | WebSocket, go2rtc's MSE messages | binary init and fragments |
| GET | `/api/v1/events` | `space.member` of the session's space, or a token in its own space; filtered per event by `device_access`, for a token also by its scopes (an event is sent only when the token could read the row it is about) | `?types=`, `Last-Event-ID` | SSE, below |
| GET | `/api/v1/instance/spaces` | `instance.admin` | — | every organisation and space |
| GET | `/api/v1/instance/agents` | `instance.admin` | — | every agent: version, last seen, stream state, its devices with their spaces |
| POST | `/api/v1/instance/agents/{id}/update` | `instance.admin` ([decision](decisions/20261119-agent-update-by-instance-admin.md)) | `{tag}` | 202; the `update` message goes to the agent on its WebSocket ([wire.md](wire.md#hub-to-agent)) |
| GET | `/api/v1/update` | `instance.admin` | — | running build, newer signed release, last update's outcome |
| POST | `/api/v1/update` | `instance.admin` | `{tag}` | 202; [flows.md](flows.md#6-self-update-dump-and-the-way-back) |
| POST | `/setup` | `public`; the handler checks the one-time setup code | `{code, name, password, kind}`; `kind` is the first space's (`makerspace`, `office`, `private`), its area rights' defaults | the first account |

Labels (render and print from Prepare, #6) are [OQ-14](risks-and-open.md#oq-14-tables-of-materials-items-and-labels);
door access is [OQ-10](risks-and-open.md#oq-10-door-access-link).

## `/api/v1`: the 3D-printer REST

For 3D printers the hub serves Prusa Connect's REST paths and fields for printers, jobs, files and
storages, and commands (owner ruling R3, plan §2.8). The spec is written by us from running their
server and our tests, never from their `openapi.json` or docs text, pinned to a `reference/prusa-connect`
commit; a tracking test runs our hub and their server on the same requests where both can serve them
([decision](decisions/20261118-their-rest-surface-under-ocl.md)). The paths are not listed here
([OQ-2](risks-and-open.md#oq-2-the-3d-printer-rest-surface)).

## `/spoolman/api/v1`

Spoolman's API under the base path `/spoolman`, to the letter of Spoolman `eccb05e`: a client pointed
at `http://hub/spoolman` meets the same paths as a real Spoolman with that base path (plan §2.8). The
Spoolman audit (#7) runs real Spoolman with the same base path.

| Group | Paths (under `/spoolman/api/v1`) |
|---|---|
| info | `GET /info`, `GET /health`, `POST /backup` |
| vendor | `GET, POST /vendor`, `GET, PATCH, DELETE /vendor/{id}` |
| filament | `GET, POST /filament`, `GET, PATCH, DELETE /filament/{id}`, tags `POST /filament/{id}/tag`, `DELETE /filament/{id}/tag/{uid}` |
| spool | `GET, POST /spool`, `GET, PATCH, DELETE /spool/{id}`, `PUT /spool/{id}/use`, `PUT /spool/{id}/measure`, tags |
| fields | `GET /field/{entity}`, `POST, DELETE /field/{entity}/{key}` |
| settings | `GET /setting/`, `GET, POST /setting/{key}` |
| lookups | `GET /material`, `/article-number`, `/lot-number`, `/location`, `PATCH /location/{name}`, `GET /export/{what}`, `GET /external/filament`, `/external/material` |
| tags | `POST /tag/scan`, reader routes |
| WebSocket | `/spool`, `/spool/{id}` and the same per resource, `{type, resource, date, payload}` |

The WebSocket is fed from the event bus. Lists keep Spoolman's filters, sort, `limit`/`offset` and
`x-total-count`, not our keyset paging.

## `/api/agent/v2` and `/p/*`

See [wire.md](wire.md) and [links.md](links.md) (L6-L11).

## The SSE envelope

One endpoint, `/api/v1/events` (plan §2.4). Each event:

```text
id: 1201
event: command.settled
data: {"seq":1201,"type":"command.settled","time":"2026-11-19T17:40:00Z","space_id":"…","source":"<device or resource id>","origin":{"kind":"person","id":"…"},"data":{…}}
```

| Field | Type | Meaning |
|---|---|---|
| `seq` | integer | Global, increasing; the SSE `id`; resume point |
| `type` | string | From the [event types registry](../registries/event-types.md), which also says who may see it |
| `time` | string, RFC 3339 | When it happened |
| `space_id` | uuid | The space it belongs to |
| `source` | string | The device or resource it is about |
| `origin` | object | `kind` (person, token, agent, queue, device) and its id |
| `data` | object | The type's own schema (registry) |

- `?types=` narrows the stream; the subscriber sees only events of the session's space, and of those
  only the events it may see: the event types registry names per type the action a subscriber needs,
  `device.view` on the event's device (through `device_access`) for device events, an area action for
  the rest (`grant.changed` needs `grants.view`, a material moved `stock.view`). When the space's
  `authz_epoch` changes the filter is rebuilt; a subscriber who no longer has a membership there is
  sent `stream.reset` and the stream is closed.
- **Facts** (device online or offline, job started or ended, command settled, grant changed, material
  moved) are stored and replayable. **State patches** go out live, not stored; on resume the client
  gets a snapshot of each visible device's state tree instead.
- **Resume** with `Last-Event-ID`: the hub subscribes to the bus first and buffers, sends a snapshot of
  each visible device (with its `state_rev`), then every stored fact after that id, then the buffered
  facts above the last sent, then live ([decision](decisions/20261119-event-order.md)). A patch carries the
  device's `rev`; the client drops a patch whose `rev` is not above its snapshot's. An id older than the
  retention gets `stream.reset`, and the client reloads.
- Facts reach the bus from one **event relay** that reads committed rows in `seq` order; a request's
  transaction never publishes a fact itself, so no subscriber sees a fact before it is stored or out
  of order. A state patch is published only after the transaction that stored it and raised the
  device's `state_rev` has committed, so a snapshot's `rev` is never behind a patch already sent.
- The event type names besides `stream.reset` are [OQ-13](risks-and-open.md#oq-13-the-event-types).
