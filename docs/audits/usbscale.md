# DYMO M-series scale: our port against usbscale

**The one reference:** usbscale (github.com/erjiang/usbscale, GPL-3.0-or-later, which GPLv3 section 13 lets us
combine with our AGPL-3.0) at commit `9ed1cb55a2dfd0daeb41548031a49ab756cb9114`, in `reference/usbscale`
(`usbscale.c` sha256 `18360242a32d9f0555b4670477c6dc0df8c92fb4532d877e3144727d7682eb5f`). Ours:
`crates/engine/src/ports/usbscale/`, a port of `UNITS`, `print_scale_data` and the read path of `main`
(`docs/agent-engine.md`, Step 3e). Checked against HID Usage Tables 1.6 ch. 33 and HID POS 1.02 section 6.2.4
(`reference/hid-usage-tables/`), which are used only to verify, never as a second source.

Proof: `scripts/usbscale-audit-check.py` cuts usbscale's own `print_scale_data` and `UNITS` out of
`usbscale.c` as text, compiles them with a harness and runs them, one process per report, next to our decoder
on the same bytes; it checks `UNITS` and the status cases against HUT Table 33.1 (usages 0x50+n and 0x70+n) and
the report layout against POS Figure 27; it rewrites section 1 and exits 0 only when every row is identical or a
difference named below as ours. Run it with `python3 -I scripts/usbscale-audit-check.py` (needs `cc` and cargo).
The port's own tests (`cargo test -p maker-connect-engine usbscale`) run over a fake HID source.

**Harness changes to usbscale's code (all of them; the function's text is compiled as extracted):**
H1 `print_scale_data` and the `UNITS` initializer are cut out of `usbscale.c` by their text, the rest (libusb,
argp, `main`) is not compiled; H2 `UNITS` is a 256-entry array, usbscale's 13 entries then `(beyond UNITS)`, so the C
code can run on units 13 and 255 (`UNITS[unit]` outside its 13 entries is undefined in usbscale; this is the one
change for unit > 12); H3 the static `lastStatus` starts at 0 in each process: one process per report (and one
for the sequence); H4 stdout and stderr are told apart by markers printed around each call; H5 `human` is false
(`--human` is not ported).

## 1. The decoding: usbscale's code run against ours

<!-- generated: begin -->
usbscale `9ed1cb5`: `print_scale_data` (usbscale.c:368-456) and `UNITS`, compiled from `reference/usbscale/usbscale.c` by `scripts/usbscale-audit-check.py` and run (usbscale's return value, its stdout and stderr) beside the Rust decoder (`ports::usbscale::usbscale::print_scale_data`, `crates/engine/examples/usbscale_decode.rs`) on the same six or seven bytes. `WEIGH_COUNT` = 2 (`scales.h:15`), `WEIGH_REPORT_SIZE` = 6.

Checker result line: `usbscale audit: 48 reports: 39 identical, 9 differs (ours), 0 UNEXPECTED; UNITS and status cases against HUT 33.1: 21 of 21 identical; Figure 27 layout identical; status messages once per change confirmed; usbscale's own strings found yes`

### Reports (a fresh usbscale process each: its static `lastStatus` is 0)

| Report | Bytes | usbscale (said, returns) | Ours (status text / unit / weight) | Result |
|---|---|---|---|---|
| status 1 | `03,01,02,00,05,00` | Scale reports Fault, returns -1 | `Scale reports Fault` / `g` / 5 | differs, ours (E3): usbscale exits (-1) at status 1; ours reports the status, the same text, and reads on |
| status 2 | `03,02,02,00,05,00` | Scale is zero'd..., returns 1 | `Scale is zero'd...` / `g` / 5 | **identical** |
| status 3 | `03,03,02,00,05,00` | Weighing..., returns 1 | `Weighing...` / `g` / 5 | **identical** |
| status 4 | `03,04,02,00,05,00` | 5 g, returns 0 | `(no text)` / `g` / 5 | **identical** |
| status 5 | `03,05,02,00,05,00` | Scale reports Under Zero, returns 1 | `Scale reports Under Zero` / `g` / 5 | **identical** |
| status 6 | `03,06,02,00,05,00` | Scale reports Over Weight, returns 1 | `Scale reports Over Weight` / `g` / 5 | **identical** |
| status 7 | `03,07,02,00,05,00` | Scale reports Calibration Needed, returns 1 | `Scale reports Calibration Needed` / `g` / 5 | **identical** |
| status 8 | `03,08,02,00,05,00` | Scale reports Re-zeroing Needed!, returns 1 | `Scale reports Re-zeroing Needed!` / `g` / 5 | **identical** |
| status 0 | `03,00,02,00,05,00` | (nothing), returns -1 | `unknown 0` / `g` / 5 | differs, ours (E3, E4): usbscale prints nothing (a status equal to `lastStatus`, which starts at 0) and exits (-1); ours reports the status as `unknown 0` and reads on |
| status 9 | `03,09,02,00,05,00` | Unknown status code: 9, returns -1 | `unknown 9` / `g` / 5 | differs, ours (E3, E4): usbscale prints `Unknown status code: 9` and exits (-1); ours reports the status as `unknown 9` and reads on |
| status 255 | `03,ff,02,00,05,00` | Unknown status code: 255, returns -1 | `unknown 255` / `g` / 5 | differs, ours (E3, E4): usbscale prints `Unknown status code: 255` and exits (-1); ours reports the status as `unknown 255` and reads on |
| unit 0 | `03,04,00,00,05,00` | 5 units, returns 0 | `(no text)` / `units` / 5 | **identical** |
| unit 1 | `03,04,01,00,05,00` | 5 mg, returns 0 | `(no text)` / `mg` / 5 | **identical** |
| unit 2 | `03,04,02,00,05,00` | 5 g, returns 0 | `(no text)` / `g` / 5 | **identical** |
| unit 3 | `03,04,03,00,05,00` | 5 kg, returns 0 | `(no text)` / `kg` / 5 | **identical** |
| unit 4 | `03,04,04,00,05,00` | 5 cd, returns 0 | `(no text)` / `cd` / 5 | **identical** |
| unit 5 | `03,04,05,00,05,00` | 5 taels, returns 0 | `(no text)` / `taels` / 5 | **identical** |
| unit 6 | `03,04,06,00,05,00` | 5 gr, returns 0 | `(no text)` / `gr` / 5 | **identical** |
| unit 7 | `03,04,07,00,05,00` | 5 dwt, returns 0 | `(no text)` / `dwt` / 5 | **identical** |
| unit 8 | `03,04,08,00,05,00` | 5 tonnes, returns 0 | `(no text)` / `tonnes` / 5 | **identical** |
| unit 9 | `03,04,09,00,05,00` | 5 tons, returns 0 | `(no text)` / `tons` / 5 | **identical** |
| unit 10 | `03,04,0a,00,05,00` | 5 ozt, returns 0 | `(no text)` / `ozt` / 5 | **identical** |
| unit 11 | `03,04,0b,00,05,00` | 5 oz, returns 0 | `(no text)` / `oz` / 5 | **identical** |
| unit 12 | `03,04,0c,00,05,00` | 5 lbs, returns 0 | `(no text)` / `lbs` / 5 | **identical** |
| unit 13 | `03,04,0d,00,05,00` | 5 (beyond UNITS), returns 0 | `(no text)` / `unknown unit 13` / 5 | differs, ours (E4): the weight is identical (`5`); usbscale reads `UNITS[13]` outside its 13 entries (undefined; the harness pads the table), ours says `unknown unit 13` |
| unit 255 | `03,04,ff,00,05,00` | 5 (beyond UNITS), returns 0 | `(no text)` / `unknown unit 255` / 5 | differs, ours (E4): the weight is identical (`5`); usbscale reads `UNITS[255]` outside its 13 entries (undefined; the harness pads the table), ours says `unknown unit 255` |
| exponent -1, weight 0x0000 | `03,04,02,ff,00,00` | 0 g, returns 0 | `(no text)` / `g` / 0 | **identical** |
| exponent -1, weight 0x0001 | `03,04,02,ff,01,00` | 0.1 g, returns 0 | `(no text)` / `g` / 0.1 | **identical** |
| exponent -1, weight 0x0100 | `03,04,02,ff,00,01` | 25.6 g, returns 0 | `(no text)` / `g` / 25.6 | **identical** |
| exponent -1, weight 0xffff | `03,04,02,ff,ff,ff` | 6553.5 g, returns 0 | `(no text)` / `g` / 6553.5 | **identical** |
| exponent 0, weight 0x0000 | `03,04,02,00,00,00` | 0 g, returns 0 | `(no text)` / `g` / 0 | **identical** |
| exponent 0, weight 0x0001 | `03,04,02,00,01,00` | 1 g, returns 0 | `(no text)` / `g` / 1 | **identical** |
| exponent 0, weight 0x0100 | `03,04,02,00,00,01` | 256 g, returns 0 | `(no text)` / `g` / 256 | **identical** |
| exponent 0, weight 0xffff | `03,04,02,00,ff,ff` | 65535 g, returns 0 | `(no text)` / `g` / 65535 | **identical** |
| exponent -128, weight 0x0000 | `03,04,02,80,00,00` | 0 g, returns 0 | `(no text)` / `g` / 0 | **identical** |
| exponent -128, weight 0x0001 | `03,04,02,80,01,00` | 1e-128 g, returns 0 | `(no text)` / `g` / 1e-128 | **identical** |
| exponent -128, weight 0x0100 | `03,04,02,80,00,01` | 2.56e-126 g, returns 0 | `(no text)` / `g` / 2.56e-126 | **identical** |
| exponent -128, weight 0xffff | `03,04,02,80,ff,ff` | 6.5535e-124 g, returns 0 | `(no text)` / `g` / 6.5535e-124 | **identical** |
| exponent 127, weight 0x0000 | `03,04,02,7f,00,00` | 0 g, returns 0 | `(no text)` / `g` / 0 | **identical** |
| exponent 127, weight 0x0001 | `03,04,02,7f,01,00` | 1e+127 g, returns 0 | `(no text)` / `g` / 1e+127 | **identical** |
| exponent 127, weight 0x0100 | `03,04,02,7f,00,01` | 2.56e+129 g, returns 0 | `(no text)` / `g` / 2.56e+129 | **identical** |
| exponent 127, weight 0xffff | `03,04,02,7f,ff,ff` | 6.5535e+131 g, returns 0 | `(no text)` / `g` / 6.5535e+131 | **identical** |
| report ID 3 | `03,04,02,00,05,00` | 5 g, returns 0 | `(no text)` / `g` / 5 | **identical** |
| report ID 4 | `04,04,02,00,05,00` | 5 g, returns 0 | `(no text)` / `g` / 5 | **identical** |
| report ID 0 | `00,04,02,00,05,00` | Error reading scale data, returns -1 | refuses: report ID 0 | differs, ours (E3/E8): usbscale exits (-1), prints `Error reading scale data`; ours refuses the report, the same text as the problem, and reads on |
| report ID 5 | `05,04,02,00,05,00` | Error reading scale data, returns -1 | refuses: report ID 5 | differs, ours (E3/E8): usbscale exits (-1), prints `Error reading scale data`; ours refuses the report, the same text as the problem, and reads on |
| 6 bytes | `03,04,02,00,05,00` | 5 g, returns 0 | `(no text)` / `g` / 5 | **identical** |
| 7 bytes, leading 00 | `00,03,04,02,00,05,00` | Error reading scale data, returns -1 | refuses: report ID 0 | differs, ours (E3/E8): usbscale exits (-1), prints `Error reading scale data`; ours refuses the report, the same text as the problem, and reads on |

### Status messages once per change (one usbscale process, `lastStatus` kept)

Reports with status 2, 2, 3, 3, 2: usbscale printed a message for 3 of the five (10101). Ours: one event per change of the decoded reading, a repeat is nothing (port test `one_event_per_change_and_nothing_on_no_change`): **differs, ours (E7)**: usbscale prints text once per status change and (status 4) ends; ours reports every changed reading.

### `UNITS` and the status cases against HID Usage Tables 1.6, Table 33.1

| Item | Usage Tables | usbscale | Result |
|---|---|---|---|
| status 1 | 0x71 Scale Status Fault | `Scale reports Fault` | **identical** |
| status 2 | 0x72 Scale Status Stable at Center of Zero | `Scale is zero'd...` | **identical** |
| status 3 | 0x73 Scale Status In Motion | `Weighing...` | **identical** |
| status 4 | 0x74 Scale Status Weight Stable | `(prints the weight)` | **identical** |
| status 5 | 0x75 Scale Status Under Zero | `Scale reports Under Zero` | **identical** |
| status 6 | 0x76 Scale Status Over Weight Limit | `Scale reports Over Weight` | **identical** |
| status 7 | 0x77 Scale Status Requires Calibration | `Scale reports Calibration Needed` | **identical** |
| status 8 | 0x78 Scale Status Requires Rezeroing | `Scale reports Re-zeroing Needed!` | **identical** |
| unit 1 | 0x51 Weight Unit Milligram | `mg` | **identical** |
| unit 2 | 0x52 Weight Unit Gram | `g` | **identical** |
| unit 3 | 0x53 Weight Unit Kilogram | `kg` | **identical** |
| unit 4 | 0x54 Weight Unit Carats | `cd` | **identical** |
| unit 5 | 0x55 Weight Unit Taels | `taels` | **identical** |
| unit 6 | 0x56 Weight Unit Grains | `gr` | **identical** |
| unit 7 | 0x57 Weight Unit Pennyweights | `dwt` | **identical** |
| unit 8 | 0x58 Weight Unit Metric Ton | `tonnes` | **identical** |
| unit 9 | 0x59 Weight Unit Avoir Ton | `tons` | **identical** |
| unit 10 | 0x5A Weight Unit Troy Ounce | `ozt` | **identical** |
| unit 11 | 0x5B Weight Unit Ounce | `oz` | **identical** |
| unit 12 | 0x5C Weight Unit Pound | `lbs` | **identical** |
| unit 0 | 0x50 Weight Unit (the collection, no unit) | `units` (usbscale: unknown unit) | **identical** |

HID POS 1.02 section 6.2.4, Figure 27 (Scale Data Report): byte 0 Report ID = 3, 1 Scale Status, 2 Weight Unit, 3 Data Scaling, 4 Data Weight LSB, 5 Data Weight MSB: **identical** with usbscale's `dat[0]..dat[5]`.
<!-- generated: end -->

Weights are compared by their bits (ours against `raw * pow(10, expt)` evaluated by the C library through Python)
and printed with C's `%g`. usbscale prints a weight only for status 4; for the other statuses it is compared
by bits only.

## 2. The read path and the discard of old data

| # | usbscale | Ours | Result |
|---|---|---|---|
| R1 | `int weigh_count = WEIGH_COUNT - 1;` (usbscale.c:179), `WEIGH_COUNT` 2 (`scales.h:15`): 1 | `read_loop`: `weigh_count = WEIGH_COUNT - 1` after each open | **identical** (1) |
| R2 | one report read and thrown away before the loop: "we get old data the first time" (usbscale.c:277-289) | `read_loop`: the extra read, thrown away, after each open | **identical** |
| R3 | in the loop: a report is decoded only `if (weigh_count < 1)`, then `weigh_count--` (usbscale.c:322-327) | the same, in the same order | **identical** |
| R4 | resulting count | **2 reports are thrown away per open** (the extra read, then one of the loop, while `weigh_count` is 1); the 3rd report is the first decoded. `DISCARDED_PER_OPEN` = 2; test `the_old_data_is_discarded_after_each_open`, and again for each open: `the_discard_starts_again_with_each_open` | **identical** |
| R5 | a read that times out (10 s) is an error: `Error in USB transfer`, exit | a read waits 1 s, only to see `Order::Close` (E6); a timeout is not an error and not a read of usbscale's count | **differs, ours (E6)** |
| R6 | a read error: exit | the scale is Gone; looked for again every 15 s (E5); found: Back, and the discard starts again | **differs, ours (E5, E7)** |
| R7 | `print_scale_data` returns 0 (status 4) and the program ends; returns 1 and it reads on; -1 and it ends | every decoded report is a reading, whatever its status; reading goes on (E3) | **differs, ours (E3)**; rows in section 1 |
| R8 | report ID other than 3 or 4: `Error reading scale data`, exit | refused; shown once as the problem `Error reading scale data`, the last reading kept, reading goes on (E12); a report shorter than 6 bytes is refused the same (usbscale reads a 6-byte buffer and never looks at the length) | **differs, ours (E3/E12)** |
| R9 | prints on a change of status only (static `lastStatus`) | an event only when the decoded reading changes (E7); a repeat is nothing | **differs, ours (E7)**; test `one_event_per_change_and_nothing_on_no_change` |
| R10 | no bytes are sent by usbscale except `zero` (tare, `--zero`) | nothing is ever sent: the HID trait has a read and nothing else; the fake source records only list, open and read; `hid.rs` has no write, no feature report, no GET_REPORT (test `nothing_is_ever_sent_to_the_scale`) | **identical** for a read-only run; tare is not ported |

## 3. Ours (Step 3e, E1-E11; nothing of this is on the wire to the scale)

| # | Ours | Where | Result |
|---|---|---|---|
| E1 | hidapi 2.6.7 instead of libusb: Linux `linux-native-basic-udev` (hidraw), Windows `windows-native`, macOS IOKit; no libusb detach/claim | `hid.rs`, `crates/engine/Cargo.toml` | **ours** |
| E2 | the device: `--port` as given, else the one device with top-level usage page 0x8D; none or several: the agent refuses at start and lists path, VID:PID, product, usage page (none: every HID device seen; several: the matching ones); never a VID:PID guess (`scales.h`'s table is not used) | `hid::select`, `port::check`, `crates/agent/src/ports.rs`; test `the_device_is_chosen_by_usage_page_never_guessed` | **ours**; usbscale: first device of its VID:PID table |
| E3 | reads on and on, every status reported, no exit | `read_loop` | **ours**; section 1 |
| E4 | the reading: status (byte + usbscale's text, or `unknown n`), unit (byte + `UNITS` name, or `unknown unit n` above 12), exponent, raw, weight in the scale's own unit; no gram conversion. For status 4 usbscale prints the weight and no status text: the text is empty | `ScaleReading`, `usbscale::status_text`, `unit_name` | **ours**; section 1 |
| E5 | while the scale is gone, looked for every 15 s (enumerate, open; nothing sent) | `FIND_INTERVAL`; test `a_read_error_is_gone_and_a_refind_is_back_after_the_interval` (a test interval; the constant is asserted to be 15 s) | **ours** |
| E6 | a read timeout of 1 s, only to see `Order::Close` | `READ_TIMEOUT_MS`; test `close_is_seen_within_the_read_timeout` | **ours** |
| E7 | a telemetry event only when the decoded reading changes; Gone on a read error or a missing device, Back when found again | `read_loop`, `run` | **ours** |
| E8 | the bytes hidapi returns are usbscale's `dat`: byte 0 = report ID, no byte stripped or skipped; a report not starting with 3 or 4 is refused | `print_scale_data`; section 1 (`7 bytes, leading 00`: refused) | **ours** |
| E9 | report ID 4 is parsed exactly as usbscale parses it (six bytes); its meaning is **unknown**: POS calls it the 2-byte Status Report (Figure 28) | section 1 (`report ID 4`: identical) | **ours**, meaning unknown |
| E10 | the udev rule `SUBSYSTEM=="hidraw", ATTRS{idVendor}=="0922", MODE="0660", GROUP="plugdev"` as a file with an install note, not installed by the agent | `crates/agent/udev/60-maker-connect-scale.rules` | **ours** |
| E11 | the hub: one live line on the machine page: status, weight, unit | `machine_page.rs::scale_line`, `machine.js`; test `a_scale_shows_one_live_line` | **ours** |
| T1 | a command or a job sent to a scale is refused (`a scale takes no commands or jobs`), a pause or an answer is ignored | `port::handle`; test `nothing_is_ever_sent_to_the_scale` | **ours (E13)** (the engine's edge, as for the other ports) |

## Not covered (needs the real M2)

- the M2's VID:PID and its report descriptor (usage page 0x8D, report ID 3, Figure 27's six bytes);
- whether the first report is stale, and the report rate (the discard of 2 reports waits for them; a scale that
  sends reports only on a change shows its first reading late);
- the kg/lb button (units and exponents), known weights, over 2 kg (status 6), under zero after a tare (status 5
  and its weight bytes), auto-off, unplug and replug;
- the first byte per OS (E8: that hidapi returns the report ID first on Linux, Windows and macOS for this device);
- hidraw permissions and hidapi's `usage_page` on the Pi; macOS input-monitoring permission and hidapi's
  threading rule on macOS (the HID source is made in the port's thread);
- the scale's range (a Feature report, Weight Limit: needs a GET_REPORT, which usbscale does not send); the
  hub shows only the scale's own "over weight limit" status;
- ported nowhere: tare (`zero`), `--index`, `--human`, a VID:PID table, libusb; no spool flow, no gram
  conversion, no Spoolman;
- the C library's `pow` on the target: the comparison runs on the build machine (`pow(10, expt)`, the same
  call on both sides); other targets' libm are not compared;
- the hub's machine page for a scale still carries the other tabs (queue, control, terminal) of other kinds.
