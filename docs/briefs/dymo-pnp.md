# Brief: the DYMO LabelManager PnP port (labelle), in parallel with section I

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust workspace; not a git repo; never commit;
never deploy or touch hardware). PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`.
**Build in your own target directory:** `export CARGO_TARGET_DIR=target-dymo` on every cargo
command. Another agent is porting section I of OctoPrint in `crates/engine/src/ports/octoprint/`
at the same time: never touch that folder; in `crates/engine/Cargo.toml` and
`crates/engine/src/ports/mod.rs` only add lines. If the engine crate fails to compile because of
`octoprint/` (half-done edits there), wait a minute and retry; never fix it yourself.

Read `CLAUDE.md` first: PORT, don't paraphrase; invent nothing; ONE reference: **labelle at commit
5e8f1e5** in `reference/labelle` (Apache-2.0; keep its licence notice, name it as source). Its
runtime: pyusb (`reference/.venv-audit`, the version installed there) and Pillow only for what
labelle calls of them. Anything not traceable to a labelle (or that runtime) line is marked
"ours" and listed in the report. Read `docs/plans/agent-engine.md`: the engine (O1–O8), the hub link
(S1–S5), decisions B1–B9 (B2 download, B4 `--mode`, B8 label stacks), and the owner's note that
the PnP and the LabelWriter 450 are separate stacks with no shared code.

**Only the PnP.** Nothing of the LabelWriter 450 (`crates/label/src/labelwriter.rs` stays as it
is), nothing of the hub side, nothing of OctoPrint.

## 1. The port: `crates/engine/src/ports/labelle/`

One Rust file per labelle file, function by function: `src/labelle/lib/devices/dymo_labeler.py`,
`usb_device.py`, `device_manager.py` (the parts used to find and open the printer), the constants
they use (`lib/constants.py`), and `DymoLabeler.print` (`dymo_labeler.py:307-352`: ROTATE_270,
tobytes, row regrouping, `print_label`, `device.dispose()`), plus `DymoLabeler.__init__`,
`SUPPORTED_TAPE_SIZES_MM`, `height_px` and `labeler_margin_px` where used.

**The picture.** The hub sends the label as a PNG. Labelle's own path for a picture file is
`render_engines/picture.py` (`Image.open`, `convert("L")`, `ImageOps.invert`, `convert("1")`,
lines ~6-45): port that, with whatever render-engine parts labelle runs around it for a picture
label (margins). Pillow's `convert("L")` and `convert("1")` (its dithering) are Pillow's runtime:
port them from Pillow's source at the version in `reference/.venv-audit` (fetch its C source,
`libImaging/Convert.c`, into `reference/`), and prove them against real Pillow on the same PNGs
(RGB, RGBA, grey, 1-bit, palette, and the hub's own label PNGs). PNG decoding with the `png` crate
standing for `Image.open` (ours). The port adds only the margins labelle adds; the hub's own 16 mm
trailing blank is drawn by the hub into the PNG (layout, UI) and is not in the port.

Not ported: `online_device_manager.py`, anything GUI, any other project (no thermal-label), the old
`status_session`.
USB with `nusb` standing for pyusb (ours, the crate `crates/label` uses). Opening, claiming,
kernel-driver detach, the reads and writes, and release exactly as labelle does. **Nothing is sent
while idle** (labelle sends nothing; the old idle status poll is not ported).

## 2. The top edge (ours, including Pillow/pyusb as labelle's runtime, the `png`/`nusb` stand-ins and the texts; write it as "Step 3b: the DYMO PnP" in `docs/plans/agent-engine.md`, items L1, L2…, "to approve (proxy)")

- Tape width: from `JobOrder.tape_mm` (already in the protocol). As labelle's
  `DymoLabeler.__init__` does, none is `DEFAULT_TAPE_SIZE_MM` (12); then labelle accepts only
  `SUPPORTED_TAPE_SIZES_MM`, anything else fails the job with labelle's `ValueError` text.
  Never derived from the PNG (proxy, 2026-11-15).
- `Order::Job { order, file }` (a PNG): labelle's picture render, then its print; then `Event::JobEnd` done, or
  failed with labelle's error text.
- `Order::Command` with any `Action`: failed `CommandDone`, "labelle has no such control".
- `Control` (pause/resume/stop) and `Answer`: labelle has none; they fail loudly (a failed
  `CommandDone` / `JobEnd` as fits), never silently dropped. The texts are ours: list them.
- Telemetry: only what labelle itself reads (its status reads during a print); no polling.
- `Gone`/`Back`: labelle's `DeviceManager.scan` (`device_manager.py:31`) and
  `find_and_select_device` (:69) run once per print (`cli.py` ~577-579): the port looks at print
  time only. No scan loop.
- **Idle status poll:** the owner, 2026-11-15: "idle poll on the PnP -> follow the example". Labelle
  sends nothing while idle, so the 2026-11-11 idle poll goes: not ported, and `status_session`
  is removed (§5). The status reads of a print are reported as raw hex only, their meaning unknown (proxy, 2026-11-15).

## 3. Wiring and the download (B2, B4 for the PnP only)

- `crates/agent/src/ports.rs`: `open` returns this port for `--mode dymo-pnp`. Add `dymo-pnp` to
  `Mode` and the usage text; do not change the other modes or their default (B4's "required"
  comes with the OctoPrint top edge).
- `crates/agent/src/ports.rs`: only add lines (another agent may add the OctoPrint port there).
- B2 in `crates/agent/src/link.rs` (`fetch`), for ALL jobs (no old `String` path kept): the job file as bytes, streamed straight to
  `job-<id>`, never a `String`, never all in memory; at most 512 MB (O6); on its own thread, so
  the 2 s reports go on; the job goes to the engine only when the file is complete. Test it with
  a PNG.

## 4. Proof

`scripts/dymo-audit-check.py` runs real labelle against a fake pyusb backend (15 cases, venv
`reference/.venv-audit`). Make a copy `scripts/labelle-port-check.py` whose recorded transcripts
the NEW port must reproduce byte for byte in a Rust test with a fake USB backend (as the old
`crates/label/tests/labelle.rs`). All cases identical.

Audit: update `docs/audits/dymo-labelmanager.md` and its row in `docs/audits/README.md` (current
result only, no history): rows extracted mechanically from labelle's source (constants, command
bytes), one Rust test per row, an explicit "not covered" list, the result as "N of N identical"
with the cases named.

## 5. Old code (B8; the only hub/label change allowed)

Remove from `crates/label/src/dymo.rs` the device code the hub does not use: `status_session`, the
`usb`-feature printer code. Keep what the hub uses (check `crates/hub`). The hub's PnP geometry
(tape widths, dots) comes from the port's ported constants: make the hub read them from the
engine's `ports::labelle` constants only if the hub can depend on the engine without pulling in
device crates or the unfinished OctoPrint port; otherwise list it as open and decide nothing.
`crates/hub/src/profiles.rs:227-233` uses `dymo::Tape::d1`; `maker_connect_label::Label` is used
in printer_files.rs, virtual_machines.rs, files.rs, gcode.rs: those stay. The old
`crates/label/tests/labelle.rs` and its fixtures go or are replaced by the new test. No adapters.

## 6. Shared files

`crates/agent/src/ports.rs`, `docs/plans/agent-engine.md` and `Cargo.lock` are shared with the
OctoPrint agent: in `ports.rs` only add lines; append "Step 3b: the DYMO PnP" at the end of
`docs/plans/agent-engine.md` and change nothing else there; new dependencies change `Cargo.lock`, so
re-read it before writing. Run the `on-track` watchdog (`.claude/agents/on-track.md`, as a
general-purpose agent on model sonnet if you can spawn agents; else say so) before the report and
put its verdict in the report.

`cargo test --workspace` (with your target dir) must pass; no new warnings.

## Report (brief)

Files; labelle items ported with file:line; the top-edge items L1…; every "ours" item; proof
result (N/N identical); what was removed; anything left open.
