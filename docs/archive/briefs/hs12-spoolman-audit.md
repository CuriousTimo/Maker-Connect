# Brief: HS12 — the Spoolman API audit (in parallel with HS7a)

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust workspace; git repo, never commit; nothing
outward-facing). PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`;
`export CARGO_TARGET_DIR=target-spoolman`. Another agent edits `crates/hub` (store, queue, files,
farm, main, machine_page, access) and the protocol for HS7a: no other hub source file may be touched.
The hub is a binary crate with no public test entry points: put the test in
`crates/hub/src/spoolman/audit.rs` and add exactly one line, `#[cfg(test)] mod audit;`, to
`crates/hub/src/spoolman.rs`. Build the app as the existing test does (`Store::open(":memory:")`,
`crate::auth::signed_in_app`, a `TcpListener` on `127.0.0.1:0`, `ureq` in `spawn_blocking`; see
`spoolman.rs` ~2195-2215); the fixture beside it, loaded with `include_str!`. If a hub test failure
comes from a file outside `spoolman`, report it and do not fix it. Read `CLAUDE.md` (audits: run the reference's own
code and ours on the same examples; "Result: identical" or exactly where it differs; a "not
covered" list; no history), `docs/archive/plans/hub-side.md` (H4, approved), and `docs/audits/dymo-labelmanager.md`
+ `scripts/labelle-port-check.py` as the model. The owner decided ours is API- and data-compatible
with Spoolman, not a copy (memory `maker-connect-spoolman-compatible`).

Reference: **Spoolman at `eccb05e`** in `reference/Spoolman` (MIT). Ours: `crates/hub/src/spoolman.rs`.

1. `scripts/spoolman-audit-check.py`: a venv `reference/.venv-spoolman` (it may exist, empty) from
   Spoolman's own `pyproject.toml`/`uv.lock` (`uv` if available). Start real Spoolman on SQLite in a
   fresh temp dir on a free localhost port. Derive the request list mechanically from Spoolman's
   routers (`spoolman/api/v1/*.py`): every route × its main cases (create/get/list/update/delete
   vendor, filament, spool; filters, sort, limit/offset, `x-total-count`; spool use/measure; 404 and
   422; info/health/settings if present). Record per request: status, every response header
   Spoolman sets (except the volatile ones you list, with why), the JSON body with volatile values
   (timestamps, generated ids if they differ) normalised by a rule you write down. Write it to a
   fixture beside your test. Stop Spoolman afterwards.
2. A Rust test replays the same requests against the hub's `/api/v1` in a fresh test store and
   compares with the same normalisation. Do not change the hub to make it pass: the test asserts the
   measured differences equal a written list, so it stays honest. Spoolman has no sign-in and ours is wrapped by
   `signed_in_app`: auth headers or cookies ours adds are listed as ours with the reason, not as
   differences.
3. `docs/audits/spoolman.md` in the audit format (per request: reference, ours, Result: identical or
   exactly where it differs); Maker Connect's own additions (extra fields for kind/unit/colour
   name/ink/width/loaded machine, places for locations, spool codes `S-0007`, the count-off) as
   "ours (owner)"; an explicit "not covered" list (e.g. SpoolmanDB import, extra-field definitions,
   WebSocket sequences if not tested — say so explicitly); a row in `docs/audits/README.md` (only add the row). Present tense, no history ("fixed", "tried").

`cargo test -p maker-connect-hub` (your target dir) must pass. Report briefly: how Spoolman ran
(versions), N requests, N identical, the differences (short), not covered, files.
