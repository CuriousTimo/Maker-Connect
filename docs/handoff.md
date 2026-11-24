# Handoff: where the work stands (2026-11-24)

For the next session, on either MacBook: read this before anything else, then `git pull`. It replaces
re-reading the day's issues and commits. The owner speaks Dutch: answer in Dutch.

## Where we are

- Deliverable **D-J (#87)**: Buddy printers straight to the hub, focus on the owner's **MINI on Buddy
  8.0.2**. Overview with every step: #87; the audit as a whole: #3.
- **#212** (files from the hub to the MINI over the WebSocket): built, `scripts/quick-check.sh` green
  with Postgres, CI green on `9d41837`. **Not yet tried on the MINI.** Its to-dos b and c stay unticked
  until that trial.
- **#200** and **#201**: closed. **#202**: every row closed but two, which go with #203. The HTTP
  rows are proved with Buddy's own parser: its generator's tables
  (`scripts/buddy-http-automaton.py`) on a test-only port of its engine
  (`crates/hub/src/devices/buddy_http.rs`), reading the hub's real answers.
- Next: **#203**.

## Next steps, in the owner's order

1. **#203** (the MINI's buttons, HS10, from Buddy 8.0.2's menu code), with #202's last two rows
   (a command body over 512 bytes, raw G-code as a command): first a table per button for the
   owner to approve. Open owner decision there: the G-code for mesh bed levelling.
2. The owner tries #212 on the MINI: send a file from the hub, see what it answers.

Later, decided: #213 (cancel objects), #211 (MMU, several tools), a material's colour, properties
and limits, a tool's type and `tool_count`. Also open: #153 (a job stuck in `stopping`), #214 (a
Spoolman audit test that failed once in CI), the 64 mentions of "Buddy 6.4" in code and docs (they
move to 8.0.2 only once that code is checked against 8.0.2).

## The owner's decisions of 2026-11-24 (binding; quoted in the code and the audit)

- Only Buddy 8.0.2 (`reference/buddy-8.0.2`, `b1043d6`). **Rule 1A**: no code of Prusa Connect
  Local's server in ours. **Rule 2**: Buddy decides what its data means; the server
  (`reference/prusa-connect`) is an explanation only. One reference per device: PrusaLink and its
  SDK for the agent's devices.
- #212: `START_CONNECT_DOWNLOAD`; the encrypted download is gone; no hub ping and no read deadline.
- Audit differences, `docs/audits/buddy.md`:
  - **D9**: an empty `T` frame on a refused segment request.
  - **D10** (B5, option A): a file already on the stick. The stick is looked at first; the hub's own unchanged copy is printed; a taken name waits for Replace; a queued job waits.
  - **D11**: a printer counts as online only while it was heard within 30 s.
  - **D12**:
    - a newer socket of a printer ends the older one;
    - no command goes into a printer's socket after 30 s of silence;
    - a command that was never answered goes again, with the same id, on the new socket.
- No "Laat maar" button for a file waiting on Replace.
- `.bgcode` only to a Buddy printer, never to a Marlin printer or a PrusaLink device.
- No Python in the product. The agent speaks the Prusa Connect protocol through our Rust port of
  Prusa's Connect SDK (`crates/connect-sdk`, about two thirds of it). The Python scripts in
  `scripts/` are dev checks only.

## How the owner wants it done

- Propose first and wait for an explicit "ja"; then do only that. No issues, edits or comments of
  your own on the side.
- Explain clearly and simply.
- Answer the owner's questions first.
- Give an overview when asked.
- Run a critical review agent, following `.claude/agents/on-track.md`, before reporting work. Report its verdict honestly.
- One commit per piece of work, on `main`. Push only when the owner says so. `git fetch` first: the owner works on two MacBooks.
- **Before each commit:**
  - Run `scripts/quick-check.sh`; its hub tests need Postgres, see below.
  - After any change in `crates/hub/src/` run `python3 scripts/buddy-audit-follow.py`. Such a change shifts the hub lines the audit cites, and the script moves those citations.
  - Then run `python3 scripts/buddy-audit-check.py` and fix by hand what it still reports.

## Tools on the MacBooks

- **Postgres for the hub tests**:
  - `scripts/dev-db.sh up` starts it, through Colima and Docker; `scripts/quick-check.sh` starts it on its own.
  - `scripts/dev-db.sh down` stops it afterwards. The owner wants nothing left running.
  - On the second MacBook these were installed on 2026-11-24: Homebrew, `brew install colima docker`, and `cargo install sqlx-cli --version 0.9.1 --locked --no-default-features --features rustls,postgres`. No `brew services`.
- Running the hub tests by hand: `SQLX_OFFLINE=true`, `DATABASE_URL` from `scripts/dev-db.sh url`,
  and `export "$(scripts/dev-db.sh command)"`.
- `tests/routes.txt` (routes and asset hashes): regenerate it with Postgres,
  `MAKER_CONNECT_REGENERATE_ROUTES=1 cargo test -p maker-connect-hub --features pg-tests,cameras -- the_routes_are_as_checked_in`.
