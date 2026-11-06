# The apps API: sending work to Maker Connect from another program

For a program that makes the work: a laser program such as
[LaserFoundry](https://github.com/Stitchwright/laserfoundry), a slicer, a script. The program
sends a finished file to a space. The file then waits in that space's **Files**, with the
machine it was made for already chosen. **Nothing starts by itself.** A laser job starts
with someone at the laser, who frames it first.

Version 1. Every path starts with `/api/apps/v1/`; `GET /api/apps/v1/info` says which version
a server speaks.

## Signing in: an API token

Someone with the *admin* or *owner* role in the space makes a token on the **Space** page,
under *API tokens*. The token is shown once and starts with `mct_`.

- **Role:** choose *member*. A *guest* token can look but not send. A token never has a
  higher role than the person who made it.
- **Sending the token:** every request carries `Authorization: Bearer mct_…`.
- **Cookies are not used.** A browser session never counts here, so another site cannot act
  for someone who is signed in.
- **Revoking:** the token can be revoked on the same page. Every request with it then gets
  `401`.

The program asks for two things, and remembers both: the server's address (for example
`http://maker-connect.local:8460`, or an `https://` address behind a proxy) and the token.

**Browsers:** every answer allows any origin (`Access-Control-Allow-Origin: *`) and answers
the preflight (`OPTIONS`). A web version can call it directly. A page served over `https`
cannot reach an `http://` server on the LAN: the browser blocks that (mixed content). Over
plain `http` on the LAN, use the desktop program.

## The calls

### `GET /api/apps/v1/info` (no token)

Whether this address is a Maker Connect, and which API it speaks.

```json
{ "service": "maker-connect", "api": 1, "version": "<this build: a release tag, or the crate version>", "tokens": "…" }
```

### `GET /api/apps/v1/me`

The space and organisation the token belongs to, so the program can show where it sends.

```json
{
  "space": { "id": "d0d13688-d16a-480e-8349-ca50dad5a312", "name": "Werkplaats" },
  "organisation": { "name": "Stichting Fablab Noord" },
  "role": "member",
  "as": "token \"LaserFoundry\""
}
```

`space.id` stays the same when the space is renamed. Use it to remember which space a token
was for.

### `GET /api/apps/v1/machines[?kind=laser]`

The space's machines, or only one kind: `fdm`, `laser`, `cnc`, `cutter`, `label`, `paper`,
`other`.

```json
[
  {
    "id": "8c1f…",
    "name": "Ortur",
    "kind": "laser",
    "model": "Ortur Laser Master 2 Pro S2",
    "profile": "ortur/lm2-pro-s2",
    "online": true,
    "state": "idle",
    "in_use": true,
    "work_area_mm": [400.0, 430.0],
    "laser": { "power_w": 10.0, "max_s": 1000.0, "laser_mode": true },
    "may": "operate"
  }
]
```

- `work_area_mm`: what the controller says it travels (`$130`, `$131`). While the machine is
  offline it is the model's work area instead; `null` when neither is known.
- `laser.max_s`: the `S` value for full power (`$30`). Make the file's power range match it.
- `laser.laser_mode`: `$32`. When it is `false`, the beam stays on while the head stops
  between moves, and the server refuses laser files for that machine. Tell the user before
  they send.
- `max_s` and `laser_mode` are `null` while the machine is offline, or until its agent has
  read the settings.
- `may`: what this token may do with the machine: `view`, `use`, `operate` or `manage`.
  Sending work needs `use` or more. A machine can require permission, and then a member
  needs an admin to give it before using it.

### `POST /api/apps/v1/files?name=<name>[&machine=<id>]`

The file is the body: G-code (`.gcode`, `.gco`, `.g`), or a label as a PNG (`.png`, one
pixel per dot). Send a `name` without a path; it shows in Files. With `machine`, the file is
checked against that machine, and the page you get back has it chosen.

```json
{
  "id": "f3a9…",
  "name": "sign.gcode",
  "size": 18234,
  "work_area_mm": [10.0, 10.0, 30.0, 10.0],
  "max_s": 300.0,
  "check": { "fits": true, "warning": "…" },
  "page": "/files#file=f3a9…&machine=8c1f…"
}
```

- **`page`:** open it in the browser, relative to the server's address. It shows the file
  with the machine chosen. **Frame** runs the outline with the beam off, and **Start** runs
  the job. Both are done by someone at the machine.
- **`check`:** the same check as *Print on* on the Files page.
  - `fits: false` comes with a `problem`, for example laser mode off, outside the work area,
    or a file for another kind of machine.
  - A `warning` can come with `fits: true`, for example a power range that differs from
    `$30`.
  - The file is kept either way. It is `null` without a `machine`.
- **`work_area_mm`:** where the moves go, as `[x0, y0, x1, y1]` in mm. `max_s` is the highest
  `S` in the file.

**Errors:** the answer's text says why.

| Status | Why |
|---|---|
| `400` | not a file it reads (by its name) |
| `401` | no token, or the token is not valid |
| `403` | the token's role is too low, or it may not use that machine (no permission) |
| `404` | no such machine in this space |

**What the file should hold, so it shows well:**
- **Absolute positions in mm** (`G90`, `G21`). With `G92` (start from where the head is) the
  file's place on the bed is not known beforehand. The server reads its size, not where it
  lands.
- **A picture:** a PNG thumbnail in the header, as PrusaSlicer writes it, shows in Files.

  ```
  ; thumbnail begin 220x124 <length>
  ; <base64, in lines>
  ; thumbnail end
  ```

  Several sizes may come one after another; the largest is shown.
- **Comments with `;`.** Comments in parentheses are fine as well, but restarting a stopped
  job from a line is safer without them.

## What happens in the space

- The file is in **Files**, under the space's files, like an upload from the page.
- The space's **audit log** says "token "LaserFoundry" sent a file from a program", so
  admins see which program sent what.
- No job is made and nothing is sent to the machine. A later version may add a queue for
  machines where that is safe. Lasers are not among them.

## For LaserFoundry

Not built on LaserFoundry's side yet. It would fit next to their *Publish to MakerFoundry*,
under File:

1. **Settings:** the server's address and the token. *Test* calls `info`, then `me`, and
   shows the organisation and space.
2. **Send to Maker Connect…:**
   - choose a laser from `machines?kind=laser`
   - set the document's machine from `work_area_mm`, `laser.max_s` and `laser.laser_mode`
   - write G-code for that machine and send it with `machine`
3. Show `check`, then open `page` in the browser.

This can be done as a plugin, or as a merge request to their repository.
