# Brief: HS15 — the Buddy audit (the MINI over Prusa Connect, firmware 6.3.0)

Goal (owner, 2026-11-17): "first get the normal one (Buddy 6.3, the MINI over Prusa Connect)
working properly". The WebSocket of 6.4+ is later and not part of this step. The hub is the
Connect server (closed upstream); the one reference is **Prusa-Firmware-Buddy 6.3.0**
(`reference/buddy-6.3.0`, `src/connect`, `src/common/http`), what the printer sends and what it
accepts. Master is not a reference here: rows only in master are marked "not in 6.3.0".

## What this step does

Makes `docs/audits/buddy.md` a real audit, one section at a time. Per row: the reference's
`file:line`, ours as `file:line`, a Result: `identical`, `differs` (exactly how, with a numbered
difference), `not used by the hub` (a command the hub never sends; not built), or `OPEN`.

Sections, in this order (stop where the work stops; the rest stays UNCHECKED and is counted):

1. Commands it takes from Connect (26).
2. Arguments it reads from a command (28).
3. Events it sends (14 event types of `planner.cpp` `to_str`), with their shape (`render.cpp`).
4. What it does with the server's answer (status codes, `Command-Id`, content type, 512 bytes,
   one command at a time) and its registration (`registrator.cpp`); endpoints.

## How each row is proven

- Reference side, by the checker (`scripts/buddy-audit-check.py`): every `file:line` in a row
  must contain the row's anchor text in the 6.3.0 tree; every item extracted from the source must
  be a row; every row must carry a Result; every test a row names must exist.
- Hub side, by the checker: every hub `file:line` must contain its anchor in `crates/hub/src`.
- Hub behaviour, by tests in `crates/hub/src/devices/buddy_audit.rs`:
  - a Rust port of `Command::parse_json_command` (`command.cpp:108-433`: depth, key, type and
    "expected arguments seen" rules, the 26-name table) is fed what the hub really sends (the
    hub's own routes and store drive it, the body is taken from the telemetry answer);
  - events are fed to the hub as `render.cpp` writes them (key order, nesting, types, taken
    line by line from `render_msg`) and the hub's state is checked;
  - the status and headers of the hub's answers are checked against what `connect.cpp`
    (`send_command`, `handle_server_resp`) and `registrator.cpp` accept.
- Buddy's own C++ is not built or run here (it needs the whole firmware and its HAL), so a row
  is "identical" only for what the port and fixtures show. Fixtures are written from the
  reference's serializer by reading it, not produced by it: this is said in the audit. A row
  whose claim rests only on reading is `OPEN`.

## Not touched

The hub's behaviour toward the printer is not changed: every difference found is listed with the
reference line, ours and a proposed fix, for the owner to decide. Only tests, the checker, the
audit and this brief are written.

## Not covered (stays UNCHECKED or OPEN unless the audit says otherwise)

- The WebSocket (`/p/ws`), `START_INLINE_DOWNLOAD`, `START_CONNECT_DOWNLOAD` (6.4+, not used by 6.3.0).
- The encrypted download itself (`/f/<iv>/raw`, `transfers/download.cpp`, the decryptor).
- Telemetry fields, the INFO event's data, file metadata, dialog button names: only what the
  hub reads is listed; row-by-row check not done in this step unless the audit says so.
- G-code the hub sends as `text/x.gcode` (the MINI mapping is HS10).
- TLS, timing and retry of the printer.
