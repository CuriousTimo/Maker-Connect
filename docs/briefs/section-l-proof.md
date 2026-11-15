# Brief: section L — proof of the Marlin port against real OctoPrint

Repo: /Users/timo/Documents/Coding/Maker-Connect (Rust workspace; git repo, never commit; never
deploy or touch hardware). PATH: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`.
Read `CLAUDE.md` (audits: run the reference's own code and ours on the same examples, compare what
both send and read, "Result: identical" or exactly where it differs, an explicit "not covered"
list), `docs/agent-engine.md` (Step 3a "Done when", D11, all decisions), and
`docs/audits/dymo-labelmanager.md` + `scripts/labelle-port-check.py` as the model.
This follows the existing CLAUDE.md audit rule; D8 is not needed and is not touched. Running
OctoPrint's own Python virtual printer unchanged meets the plan's "Done when" ("the ported virtual
printer prints a file through the port") better than porting it: do not port `virtual.py`.

**The test device is OctoPrint's own:** `plugins/virtual_printer/virtual.py` (`VirtualPrinter`,
`write` :2136, `readline` :2170) in `reference/OctoPrint-1.11.8`, run unchanged in Python
(`reference/.venv-octoprint`). Both sides talk to the *same* Python virtual printer; nothing of it
is ported to Rust.

## 1. The reference run: `scripts/octoprint-port-check.py`

Runs real OctoPrint 1.11.8's `MachineCom` + `Printer` (standard.py) headless against a fresh
`VirtualPrinter` (the virtual printer has no randomness; remove its wall-time dependence at the source, using only
its own settings — `sendWait: false`, the ambient-temperature and sleep options, autoreport off,
`simulateReset` as given — and OctoPrint's own serial timeout and temperature-poll settings, the
same on both sides; list every setting changed from the defaults and why; `virtual.py` stays
unchanged — if that is not possible, stop and report), for a
fixed list of scenarios, and records per scenario every byte written to and every line read from
the virtual printer, in order. Scenarios, through the same calls our top edge makes (M1): connect
(auto-detect and given port/baud), home XY, home Z, jog, extrude, set tool and bed temperature, fan
on/off, motors off, feed and flow rate, typed G-code (single, several lines), a print of a small
G-code file (select_file + start_print) to the end, pause and resume during it, cancel during it,
a resend request (the virtual printer's own option to force one), an emergency `M112`, an
`action:prompt` from the printer and an answer, SD list / select / print / delete (the virtual
printer's SD), disconnect. Write them to fixtures in `crates/engine/tests/octoprint/`. Freeze this scenario list
before our first run; no scenario is dropped or reworded after a failure. Run each scenario 5
times against the reference alone: one whose reference transcripts differ between its own runs
(after the rules below) is `not reproducible` and goes on "not covered", never "identical".

First state whether `tests/util/test_comm.py` / `test_comm_helpers.py` are ported and pass
(the plan's "Done when", first clause).

## 2. Our run

A Rust test drives our port (the engine `Port`, through the same `Order`s the hub link gives)
against the same Python `VirtualPrinter`, connected through a test-only transport (a pipe to a
small Python bridge process that wraps `VirtualPrinter.write`/`readline`; extend the existing
`cfg(test)` seam `TEST_TRANSPORTS` rather than adding another), and records the same way. Compare
byte for byte with the fixtures. Compare in two layers, the same rules for both sides, each rule in the audit with a count of
the lines it dropped: (a) the bytes sent to the printer, in order: identical, no normalisation at
all; (b) the lines read back, as an ordered sequence, after dropping only `wait` lines and the
numbers in `T:`/`B:` temperature fields while the simulated temperatures are still moving. No other
rule: if one seems needed, stop and report it. The bridge is ours: a pure byte pass-through, no
filtering, no retry, no timing of its own; name it in the audit. The normalisation also covers, and lists,
the approved ours-only items of Section K so they are not reported as wire differences: K6 (the
file arrives as `job-<id>.d/<name>`), K7 (SdPrint is `select_file(sd=True)`), K11 (the done rule),
K13 (the output lines), and any other K-item that changes a name or a path.

## 3. The audit

`docs/audits/octoprint-marlin.md` in the audit format: per scenario, reference, ours, **Result:
identical** or exactly where it differs; the normalisation rules; an explicit "not covered" list;
the new file takes over row 4 of `docs/audits/README.md` (current result only, no history); the
old `docs/audits/marlin.md` stays (deleting it is the owner's call). The M2 actions (load, unload,
probe, laser) go on "not covered": answered as no such control, nothing sent. Do NOT change
the port to make a scenario pass: list every difference; a fix is a later step.

`cargo test --workspace` must pass (if any old test still hangs, report it and change nothing) apart from the new comparison test, which reports the measured
differences (it may assert the known-difference list equals the measured one, so it stays honest).

## Report (brief)

How the reference ran (versions, the settings changed), the test_comm status, N scenarios, N identical, each difference (short),
not covered, files.
