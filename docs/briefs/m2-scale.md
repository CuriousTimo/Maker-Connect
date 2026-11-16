# Brief: the DYMO M-series scale (live weight only), 2026-11-16

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust; never commit; no hardware; no deploy).
PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`.

Read first: `CLAUDE.md`; Step 3e at the end of `docs/agent-engine.md` (the reference, E1-E11, the not-covered list:
binding); `reference/usbscale/usbscale.c` (the whole read path 170-345 and `print_scale_data` 380-456);
`reference/hid-usage-tables/hut1_6.txt` ch. 33 and `pos1_02.txt` §6.2.4 (to verify only). Look at how the labelle
port is built (`crates/engine/src/ports/labelle/`, `crates/agent/src/{link,ports,main}.rs`) and do the same; do not
touch the labelle or dymo_cups files (uncommitted PnP work sits in the tree).

Scope: read the weight and report it live. No commands to the scale, no tare, no range, no spool flow, no gram
conversion, no Spoolman.

1. **Protocol** (`crates/protocol`): `MachineKind::Scale` (label "Scale"); `Telemetry.scale: Option<ScaleReading>`
   with status (u8 + text), unit (u8 + name), exponent (i8), raw (u16), weight (f64, raw × 10^exp, the scale's own
   unit). Texts: usbscale's own strings and `UNITS` names; "unknown n" / "unknown unit n" (E4).
2. **Port** `crates/engine/src/ports/usbscale/`: header naming usbscale, the commit, GPL-3.0-or-later. Port
   `print_scale_data` and `UNITS` function by function. Port usbscale's discard of old data exactly as Step 3e
   describes it (`WEIGH_COUNT` is 2 in `reference/usbscale/scales.h:15`; the extra first read; `weigh_count`,
   usbscale.c:179, 322-327); state the exact count in the audit, once per open. Then E3, E5-E9 as written. A trait for the HID source so tests use a fake; the real one on `hidapi` 2.6.7
   with per-target features (Linux `linux-native-basic-udev`, Windows `windows-native`, macOS default) and
   `default-features = false` where needed.
3. **Agent**: `--mode dymo-scale` in `Mode`/`parse_mode`, usage text, `ports.rs::open`; `--port` = HID path; without
   it E2 (usage page 0x8D, exactly one, else refuse and list path, VID:PID, product, usage page). `--kind scale`.
4. **Hub**: accept the kind; one live line on the machine page with status, weight and unit (E11). Route snapshot
   (`crates/hub/tests/routes.txt`) changes only if a route changes (it should not).
5. **udev**: there is no `packaging/` dir. Look where the agent's systemd unit / install files live (e.g.
   `scripts/`, `crates/agent/`, `dist/`) and put `60-maker-connect-scale.rules` with E10's line there; if there is
   no fitting place, put it in `crates/agent/udev/` and say so. Plus a two-line install note in the agent docs. Do not install it anywhere.
6. **Proof**: `scripts/usbscale-audit-check.py` compiles usbscale's own `print_scale_data` and `UNITS` from
   `reference/usbscale/usbscale.c` (extract them mechanically, stub what it needs; reset its static `lastStatus` per
   vector; for unit > 12 use a bounds-safe copy of the table and record that as the one harness change; record every
   harness change) and runs it against our decoder (a small test binary or `cargo test` output) on the same vectors:
   statuses 1-8 and 0, 9, 255; units 0-12, 13, 255; exponents −1, 0, 0x80, 0x7F; weights 0, 1, 0x0100, 0xFFFF;
   report IDs 3, 4, 0, 5; a 6-byte and a 7-byte (leading 00) form. Where usbscale exits (report ID not 3/4, status 1,
   an unknown status) the row says "differs, ours (E3/E8)". It also checks `UNITS` and the status cases
   against HUT Table 33.1 (0x50+n, 0x70+n). `docs/audits/usbscale.md`: a row per item, Result identical or the exact
   difference, E-items marked ours, not-covered list as in Step 3e, no history. Add it to `docs/audits/README.md`.
   Port tests with the fake source: discard count, one event per change, nothing on no change, Gone on read error,
   Back on refind after 15 s (test clock or a test-only parameter), Close within the timeout, and that the fake
   records zero writes / feature requests.
7. Builds: `cargo test --workspace` passes, no new warnings; `cargo zigbuild --target
   arm-unknown-linux-gnueabihf.2.28 -p maker-connect-agent` and the other Linux targets of `scripts/build-release.sh`
   still build; the Windows target builds if build-release.sh builds it. Check first that hidapi 2.6.7 with these features builds
   under zigbuild for the ARM target; if it does not, stop and ask (do not switch crates or features on your own).

Report briefly: what was built, the checker's result line, the discard count, anything unclear (stop and ask
rather than guess).
