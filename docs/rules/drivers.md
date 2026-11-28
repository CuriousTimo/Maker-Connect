# Driver-Specific References & Approved Exceptions

## Lasers (GRBL): the wiki, and LaserFoundry where the wiki is silent

Owner, 2026-11-21/22: "gnea-wiki als referentie", "als de wiki zwijgt dan moeten wij dat niet zien als hun 60s is fout", "we volgen laserfoundry".

- **The reference** is the gnea/grbl wiki at `86e8ecd` (://github.com) and `doc/script/stream.py` at gnea/grbl `bfb67f0`. LaserFoundry (our fork, at the rev pinned in `crates/engine/Cargo.toml`) is the library that sends it. Where the Ortur (grblHAL) differs, only what was measured on it counts, recorded in #165.
- **Where the wiki is silent, LaserFoundry's own choice stays** (time-outs, jog and feed bounds, line limits, reconnecting). Only where its code contradicts a wiki text, quoted, is it a difference, and the owner decides. The one so far: Set origin `G92 X0 Y0`, kept ("we volgen laserfoundry", "dit hoeft niet in EEPROM").
- **Nothing of our own on top** ("volg de wiki en ga geen zaken zelf verzinnen of invullen"): no defaults, fallbacks, bounds or state checks of ours in what goes to the laser. The hub's UI may limit what a person can press; that is all.
- **Nothing goes to StitchFoundry/laserfoundry** (no PR, issue or comment) unless the owner asks for that one item. Fixes we need go into the fork only.
- **Quotes are checked by a script, never by eye**: `scripts/quote-check.py` (laser code and Part 4 of `docs/design/prusa-sdk-extension.md`), before every commit that touches them.

---

## Stream Deck: our fork `deckhand`, an exception to "one reference"

Owner, 2026-11-23: "Ik wil 1 standaard implementatie in rust die ik keer op keer kan blijven gebruiken in allerlei projecten", and "Het kan zijn dat wij hem moeten tweaken en aanpassen voor ons gebruik" ([#190](https://github.com)).

- **The one reference** for the Elgato Stream Deck is **deckhand** (://github.com), our fork of the Rust crate `elgato-streamdeck` (://github.com, MPL-2.0; no file carries the "Incompatible With Secondary Licenses" notice, so it may be combined with our AGPL-3.0). It forked from upstream at `63770911` (v0.14.0). The same fork serves all the owner's projects.
- **The exception, decided by the owner:** upstream's README says it is "Heavily based on python-elgato-streamdeck ... and partially on rust-streamdeck", and it credits node-elgato-stream-deck for some models. That is fusing, which the most important rule forbids for a reference. For the Stream Deck the owner accepts it.
- **Used as a dependency, not ported**, pinned to a commit of the fork's `curioustimo` branch:
  `deckhand = { package = "elgato-streamdeck", git = "https://://github.com", rev = "<commit>" }`.
  No copy of its code here, and no second source (python-elgato-streamdeck or others) changes what we send; those may only be read to understand the crate.
- **The fork's branches** (its `FORK.md`): `main` is an exact copy of upstream, fast-forwarded daily by its `follow-upstream.yml`; `curioustimo` holds our changes. New upstream commits come as one pull request `main` -> `curioustimo` with a build check; the owner merges it, then the pinned `rev` here is updated.
- **Changes in the fork** only with the owner's approval, each listed with why in the fork's `FORK.md` ("Changes against upstream"). A change to what goes to the device counts as a change on the wire: it needs the owner's approval for that change, quoted in `FORK.md`. Offering one upstream is the owner's decision too. MPL-2.0: changed files stay MPL-2.0 and are published in the fork.
- **Nothing of our own on top / on the wire** outside the fork: what goes to the device is what deckhand sends. Our own choices (which keys, which images) are only in our UI, as everywhere else.
- **A key is not an emergency stop**: stopping stays the machine's own button.

---

## WireGuard inside the agent: onetun, with boringtun as a dependency

Owner, 2026-11-23 ([#194](https://github.com)): boringtun as a dependency ("cloudflare ontwikkelt daar actief aan"), the glue ported from onetun.

- **The one reference** is **onetun v0.3.10** (://github.com, `89c3b59`, MIT), ported file by file into `crates/wireguard` with its licence (`LICENSE-onetun`); every change against it is listed at the top of `crates/wireguard/src/lib.rs`.
- **The exception, decided by the owner:** the WireGuard protocol itself, **boringtun** (Cloudflare, BSD-3-Clause), is **used as a dependency, not ported**, pinned to `=0.7.1` in `crates/wireguard/Cargo.toml` (onetun has 0.6.0; that newer version is change 1 in `lib.rs`). smoltcp is a dependency as in onetun.

---

## Prusa MK3 family: PrusaLink (#79)

Owner, 2026-11-12: PrusaLink and its Connect SDK (no licence on GitHub, `"Freeware"` in `setup.py`) — the owner decided to port them.

- **The one reference** is **PrusaLink** (`prusa3d/Prusa-Link` at commit `<PIN_COMMIT_HASH>`), ported file by file and function by function in 19 planned steps (`#79`).
- **Scope & shared code:** Do not fuse PrusaLink behaviour with plain Marlin/OctoPrint behaviour. Only share low-level transport primitives where PrusaLink's own implementation is identical; any MK3/Einsy-specific serial quirk, baud rate, timeout, or state machine follows PrusaLink to the letter.
- **Mechanical proof:** Every step must have its side-by-side audit rows in `docs/audits/prusalink.md` verified by `scripts/*-audit-check.py` and `scripts/quote-check.py` before moving to the next step.
