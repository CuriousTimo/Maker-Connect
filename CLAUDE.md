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
- **Checks** (owner, 2026-11-20: the full gate every time costs too much time). Before a commit: a
  quick check only: `cargo fmt --check`, `clippy -D warnings` and the tests of the crates changed, and
  the audit check that belongs to the change. CI runs the rest on the push to `main`; when it is red,
  fix it in one go. The full `scripts/check.sh` only before a build for a Pi or a release.
- The release key (`~/.config/maker-connect/release-key.pk8`) is never in the repo; releases
  are signed. Test builds go only to the Pis, never as GitHub releases.
- Answer the user's questions first, before carrying on with other work.
