# Brief: the PnP's idle status poll (owner exception, 2026-11-16)

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust; git repo, never commit unless told; no
hardware). PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`.

Read the last paragraph of `docs/archive/plans/agent-engine.md` ("the PnP's idle status poll comes back"): the
owner's decision, in the form of 2026-11-11. Port: `crates/engine/src/ports/labelle/` (labelle
5e8f1e5; read `port.rs`, `cli.rs`, `usb_device.rs`, `dymo_labeler.rs`, and Step 3b L1–L10 in
`docs/archive/plans/agent-engine.md`).

1. While no job runs, every 15 s: open the printer exactly as labelle's print path does
   (`DeviceManager.scan` + `find_and_select_device` + `UsbDevice.setup`, the ported functions, no new
   USB code), send one `ESC A` (labelle's `_status_request`, `dymo_labeler.py`), read its answer as
   `_get_status` does, release the device as `dispose` does. Nothing else is sent. A job always takes
   precedence: no poll starts while a job runs, and a job waits for a running poll to release.
2. The answer goes to telemetry as `raw_status` only (first 8 bytes in hex, meaning unknown), as
   during a print today (L6). No cassette/battery interpretation.
3. Gone/Back: a poll that finds no printer reports `Gone`, the first that finds it again `Back`
   (L7 extended; write down).
4. Write it as a new item in Step 3b (L11): "ours, the owner's exception 2026-11-16 to labelle's
   nothing-while-idle", with the interval, the sequence, and the quote. Update the audit
   (`docs/audits/dymo-labelmanager.md`): the idle poll as an owner exception, not covered by the
   labelle comparison, and state what bytes it sends.
5. Tests with the fake USB backend: the exact bytes of one poll (`ESC A`, then a read), the 15 s
   cadence with a test clock or a parameter set only in tests, no poll during a job, a job waiting
   for a poll's release, Gone/Back. The existing 52/52 proof must stay identical (prints are unchanged).

`cargo test --workspace` must pass; no new warnings. Report briefly.

## Additions from the proxy and the watchdog (binding)

1. **One `ESC A`:** use a `DymoLabelerFunctions` with `synwait = None` (labelle's own class default),
   so `get_status` sends exactly one `ESC A` and reads once (with `synwait = 64`, as `print` builds it,
   `send_command` would add a second `ESC A` and read, `dymo_labeler.rs` ~121-124). State the exact bytes
   in the audit.
2. **Timing (ours, within "every 15 s"):** the first poll 15 s after start-up; then 15 s after the
   previous poll ended or a job ended.
3. **A failed poll** other than "no printer" (a timeout, an open error): release as labelle's error
   path does, fail no job, send no Gone, no retry (the next poll comes on its time). Write it down.
4. **State:** a poll does not change the machine state; it stays Idle (L10: Busy only during a job);
   `raw_status` is reported while Idle.
5. **Amend, not delete:** L1, L7 ("no scan loop, no poll"), L8 (mark it "superseded by the owner,
   2026-11-16"), L10 ("nothing derived"), and the plan line that says the PnP sends nothing idle; the
   audit row "Sent while idle: nothing, identical" (`docs/audits/dymo-labelmanager.md` ~125) is
   rewritten as the owner's exception, present tense, no history.
6. **One Gone/Back flag** shared by the poll and the job: Gone/Back only on a change; a poll or a
   job that finds the printer after a Gone sends Back.
