# Maker Connect — rules for working on this project

## 1. Multi-Model Agent Workflow (Claude Max + 9Router Subagents)
- **Main Agent (Claude Max) writes ALL Rust code:** All architecture, traits, lifetimes, and wire-protocol implementations across all crates (`hub`, `agent`, `engine`, `connect-sdk`, `grbl`, `label`, `wireguard`, etc.) are written by Claude Max.
- **`ref-scout` (`haiku` -> Nemotron 3 Ultra) explores `reference/`:** Never load raw files from `reference/` (OctoPrint, PrusaLink, Connect SDK, Buddy, Moonraker, CUPS, labelle, go2rtc, etc.) into the main context. Delegate reference reading to `ref-scout` to extract file-by-file logic, exact byte sequences, and verbatim quotes from the single approved reference tree.
- **`audit-runner` (`sonnet` -> MiniMax-M3.1-Flash) runs `scripts/check.sh` & audits:** Delegate `scripts/check.sh`, `scripts/quote-check.py`, `scripts/*-audit-check.py`, `cargo check`, `cargo clippy`, and `cargo test` to `audit-runner`, returning only failing diffs or compiler errors.
- **`on-track` (`sonnet` -> MiniMax-M3.1-Flash) is the mandatory watchdog:** Run it before starting a phase or step, before reporting work to the owner, and whenever the work changes direction. It reads and reports only; it never changes anything.

---

## 2. ⚠️ THE MOST IMPORTANT RULE: follow ONE working reference to the letter

Everything that talks to a machine or to another program (printers, lasers, label printers, Moonraker, Spoolman clients, …) does **exactly** what one proven, working reference does: the same bytes, the same order, the same timing, the same quirks. The machine must not be able to tell it is not talking to that reference.

- **Pick ONE reference per device or protocol:** The best-maintained working one (the maker's own software first, e.g. PrusaLink/Buddy for Prusa; otherwise the established open project, e.g. `labelle` for the DYMO LabelManager, `OctoPrint-1.11.8` for plain Marlin). Write down which one, and at which commit, at the top of the module and in its audit.
- **NO fusing** of code or behaviour from different sources ("a bit from labelle, a bit from thermal-label, a bit of our own"). That is how the DYMO tape feeds, the invented battery voltage, and the Marlin/OctoPrint drift happened.
- **NOTHING made up, anywhere in this project** (owner, 2026-11-22: *"voor dit hele project is het verboden om allerlei shit zelf te verzinnen. dit mag nooit meer gebeuren"*). Also not in a library we use, a doc, an answer to the owner, or an idea: every claim about how a device, program, or protocol behaves carries the reference's verbatim quote and where it stands, checked by `scripts/quote-check.py`, or it says `"not checked"`.
- **NO making things up:** No own timings, polling, padding, thresholds, retries, limits, "safety" checks, or interpretations of unknown bytes in what goes to the device. If the reference does not do it, we do not do it. If we do not know what a byte means, we say `"unknown"`, we do not guess.
- **NO bad sources:** No one-person experiments, unmaintained forks, forum guesses, or half-understood notes as the reference. Other sources may only be read to *understand* the reference, never to change what we send.
- **Prove it mechanically:** An audit table (`docs/audits/*.md`, index in `docs/audits/README.md`) extracted from the reference's source by a checker script (`scripts/*-audit-check.py`), a test per row, and an explicit `"not covered"` list. **"Checked by eye" counts as not checked.**
- **Audit format:** Per item: the reference, ours, and a **Result: identical** or exactly where it differs, produced by running the reference's own code and ours on the same examples and comparing what both send and read (see `docs/audits/dymo-labelmanager.md`). No history in audits ("fixed", "tried", "test …"): only the current result.
- **Our own choices are allowed only in our UI** (what a user may press, limits on our side), never on the wire between agent/hub and the machine.
- **Port, don't paraphrase** (owner, 2026-11-12): The reference's code is translated file by file and function by function, keeping its licence and naming it as the source (OctoPrint AGPL-3.0 and labelle Apache-2.0 fit our AGPL-3.0). Where a reference has no licence that allows it, ask the owner first. **PrusaLink and its Connect SDK** (no licence on GitHub, `"Freeware"` in `setup.py`): the owner decided to port them (2026-11-12).
- **When unsure:** Stop and ask, with the evidence. Don't guess and don't build on a guess.

---

## 3. Driver-Specific References & Approved Exceptions
All approved driver exceptions, pinned commits, and hardware-specific rules live in `docs/rules/drivers.md` (covering **Lasers / GRBL & LaserFoundry**, **Stream Deck / `deckhand` #190**, **WireGuard / `onetun` & `boringtun` #194**, and all other device stacks). Read the relevant section in `docs/rules/drivers.md` before touching any of those drivers.

---

## 4. Parallel & Background Work (Read-Only Prep Only)
When subagents are idle while you are implementing the current step, keep them productive **strictly within `on-track` rule #6 (no coding ahead of approved design, one active implementation at a time)**:
- **Never** let idle subagents write or modify `.rs` production files on other parts of the codebase.
- **Background `ref-scout` (Nemotron):** May prepare read-only, quote-verified reference notes in `docs/notes/` for the *immediately next* sub-step of the active driver, or audit existing code comments for missing verbatim reference quotes.
- **Background `audit-runner` (MiniMax):** May run `scripts/check.sh`, `cargo clippy`, and audit-coverage checks (`docs/audits/`) in parallel while you code, reporting any drift immediately.
