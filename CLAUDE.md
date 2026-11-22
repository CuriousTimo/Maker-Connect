# Maker Connect — rules for working on this project

## ⚠️ THE MOST IMPORTANT RULE: follow ONE working reference to the letter

Everything that talks to a machine or to another program (printers, lasers, label printers,
Moonraker, Spoolman clients, …) does **exactly** what one proven, working reference does:
the same bytes, the same order, the same timing, the same quirks. The machine must not be able
to tell it is not talking to that reference.

- **Pick ONE reference per device or protocol**: the best-maintained working one (the
  maker's own software first, e.g. PrusaLink/Buddy for Prusa; otherwise the established open
  project, e.g. labelle for the DYMO LabelManager, OctoPrint for plain Marlin). Write down
  which one, and at which commit, at the top of the module and in its audit.
- **NO fusing** of code or behaviour from different sources ("a bit from labelle, a bit
  from thermal-label, a bit of our own"). That is how the DYMO tape feeds, the invented
  battery voltage, and the Marlin/OctoPrint drift happened.
- **NOTHING made up, anywhere in this project** (owner, 2026-11-22: "voor dit hele project is het
  verboden om allerlei shit zelf te verzinnen. dit mag nooit meer gebeuren"). Also not in a library
  we use, a doc, an answer to the owner or an idea: every claim about how a
  device, program or protocol behaves carries the reference's verbatim quote and where it stands,
  checked by `scripts/quote-check.py`, or it says "not checked".
- **NO making things up**: no own timings, polling, padding, thresholds, retries, limits,
  "safety" checks or interpretations of unknown bytes in what goes to the device. If the
  reference does not do it, we do not do it. If we do not know what a byte means, we say
  "unknown", we do not guess.
- **NO bad sources**: no one-person experiments, unmaintained forks, forum guesses or
  half-understood notes as the reference. Other sources may only be read to *understand*
  the reference, never to change what we send.
- **Prove it mechanically**: an audit table (`docs/audits/*.md`, index in `docs/audits/README.md`) extracted from the
  reference's source by a checker script (`scripts/*-audit-check.py`), a test per row, and an
  explicit "not covered" list. "Checked by eye" counts as not checked.
- **Audit format**: per item the reference, ours, and a **Result: identical** or exactly
  where it differs, produced by running the reference's own code and ours on the same
  examples and comparing what both send and read (see `docs/audits/dymo-labelmanager.md`).
  No history in audits ("fixed", "tried", "test …"): only the current result.
- **Our own choices are allowed only in our UI** (what a user may press, limits on our
  side), never on the wire between agent/hub and the machine.
- **Port, don't paraphrase** (owner, 2026-11-12): the reference's code is translated file by file and function by function, keeping its licence and naming it as the source (OctoPrint AGPL-3.0 and labelle Apache-2.0 fit our AGPL-3.0). Where a reference has no licence that allows it, ask the owner first. PrusaLink and its Connect SDK (no licence on GitHub, "Freeware" in `setup.py`): the owner decided to port them (2026-11-12).
- When unsure: stop and ask, with the evidence. Don't guess and don't build on a guess.

## Lasers (GRBL): the wiki, and LaserFoundry where the wiki is silent

Owner, 2026-11-21/22: "gnea-wiki als referentie", "als de wiki zwijgt dan moeten wij dat niet zien als
hun 60s is fout", "we volgen laserfoundry".

- **The reference** is the gnea/grbl wiki at `86e8ecd` (github.com/gnea/grbl.wiki) and
  `doc/script/stream.py` at gnea/grbl `bfb67f0`. LaserFoundry (our fork, at the rev pinned in
  `crates/engine/Cargo.toml`) is the library that sends it. Where the Ortur (grblHAL) differs, only
  what was measured on it counts, recorded in #165.
- **Where the wiki is silent, LaserFoundry's own choice stays** (time-outs, jog and feed bounds, line
  limits, reconnecting). Only where its code contradicts a wiki text, quoted, is it a difference, and
  the owner decides. The one so far: Set origin `G92 X0 Y0`, kept ("we volgen laserfoundry", "dit
  hoeft niet in EEPROM").
- **Nothing of our own on top** ("volg de wiki en ga geen zaken zelf verzinnen of invullen"): no
  defaults, fallbacks, bounds or state checks of ours in what goes to the laser. The hub's UI may
  limit what a person can press; that is all.
- **Nothing goes to StitchFoundry/laserfoundry** (no PR, issue or comment) unless the owner asks for
  that one item. Fixes we need go into the fork only.
- **Quotes are checked by a script, never by eye**: `scripts/quote-check.py` (laser code and Part 4 of
  `docs/design/prusa-sdk-extension.md`), before every commit that touches them.

## The watchdog: `on-track`

The owner wants a cheap, critical agent that keeps the work on track (`.claude/agents/on-track.md`,
Sonnet, read only). Run it, and put its verdict in the report:
- before starting a phase or a step of `docs/archive/plans/agent-engine.md`;
- before reporting work to the owner;
- whenever the work changes direction.

Its verdict is DRIFTING or OFF TRACK: correct course first, or stop and ask the owner. Never hide
its findings.

## Other standing rules

- **No pull requests** (owner, 2026-11-20: one person works here, not a team). Commit straight on
  `main` and push, only when the owner asks for a commit.
- **Checks** (owner, 2026-11-20: the full gate every time costs too much time; 2026-11-23: no more
  red CI runs). Before a commit: `scripts/quick-check.sh` (every step CI runs: fmt, clippy on macOS and
  for Windows, rustdoc `-D warnings`, the tests of the crates changed and of those that depend on them,
  the hub's with Postgres and `tests/routes.txt` when the hub is among them), and the audit check that
  belongs to the change. When CI is red anyway, fix it in one go. The full `scripts/check.sh` only
  before a build for a Pi or a release.
- The release key (`~/.config/maker-connect/release-key.pk8`) is never in the repo; releases
  are signed. Test builds go only to the Pis, never as GitHub releases.
- Answer the user's questions first, before carrying on with other work.
