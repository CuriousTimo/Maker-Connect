# OctoPrint serial layer: behaviour spec for a plain Marlin printer (default settings)

Purpose: let another engineer reimplement what OctoPrint does, to the letter, when it talks to a
printer whose M115 answer has `FIRMWARE_NAME:Marlin ...`, with all settings at their defaults and a
fixed port and baud rate. Facts only; every statement carries a file:line. No code was changed.

## Citation key

| Prefix | File (under `/Users/timo/Documents/Coding/Maker-Connect/reference/OctoPrint/src/octoprint/`) |
|---|---|
| `sc:` | `plugins/serial_connector/serial_comm.py` |
| `cs:` | `plugins/serial_connector/config_schema.py` (all defaults; loaded as plugin settings via `__init__.py:62-65`) |
| `scr:` | `schema/config/scripts.py` (default G-code scripts) |
| `con:` | `plugins/serial_connector/connector.py` |
| `std:` | `printer/standard.py` (calls the scripts) |
| `set:` | `settings/__init__.py` (script lookup) |
| `acp:` | `plugins/action_command_prompt/__init__.py` |
| `util:` | `util/__init__.py` |
| `prof:` | `printer/profile.py` (default printer profile) |

Terms used below. "Send queue" = the queue the send thread writes from. "Credit" = the
`_clear_to_send` counter (`sc:743-745`). "Job queue" = lines enqueued with `part_of_job=True`
(scripts, M110 at start, M400, ...; `sc:1364-1366`). "Command queue" = lines sent by plugins/UI
while a host print is running (`sc:1367-1380`). "Default profile" = 1 extruder, `sharedNozzle` false,
heated bed true, heated chamber false (`prof:237-243`).

A "plain Marlin printer" matches none of the firmware-name rules of section 11 (`sc:3003-3064`).

---

## 1. Opening the port and the handshake

1.1 Baud rate and port are used as given; with a port and a non-zero baud rate there is no
detection (`needs_detection = not (port and port != "AUTO" and baudrate)`, `sc:2357`). The state goes
`OPEN_SERIAL` then `CONNECTING` (`sc:2361-2365`).

1.2 Port parameters: timeout = `timeout.connection` = 10.0 s, `write_timeout` 0, `exclusive` True
(`sc:4040-4047`; `cs:37`, `cs:112`). `lowLatency` False (`cs:115`). Parity workaround
`useParityWorkaround = "detect"` (`cs:259`): only on Linux with `/etc/debian_version` OctoPrint first opens
the port with odd parity, closes it, resets to no parity and opens again (`sc:4053-4066`). Otherwise a single
open. Line framing is 8N1 as the pyserial default (parity none after the workaround; not set explicitly
otherwise, `unclear`: stop bits/bytesize are pyserial defaults, `sc:4040-4066`).

1.3 Outgoing text is encoded `ascii` (`cs:288`, `sc:5155`, errors replaced), terminated with a single
`\n` (`sc:5187`). Incoming bytes are split on `\n` only (`sc:7195`), decoded as UTF-8 with fallback latin1
(`sc:4338-4341`). Leading/trailing whitespace and `\0` are stripped for parsing (`sc:2591`).

1.4 First thing sent (`try_hello` is `not waitForStartOnConnect`, default False, `cs:204`, `sc:2364`):
`sc:2383-2389` calls `sayHello()` and, unless `suppressSecondHello` (default False, `cs:178`), a second
`sayHello()` immediately after. Each hello is `helloCommand` = `M110 N0` (`cs:175`), sent forced
(`sc:2209-2218`). No bare newline is sent first in this path (the bare `\n` is only part of the
auto-detection path, `sc:3617`).

1.5 On the wire each hello is `N0 M110 N0*125` + `\n`. Reason: M110 is in `checksumRequiringCommands`
(`cs:172`, `sc:5134-5143`), the M110 handler sets the line counter to 0 before the write (`sc:5615-5643`),
so the line is numbered `N0`; checksum is the XOR of the bytes of `N0 M110 N0` = 125 (computed, algorithm
`sc:5168-5178`). After each hello the counter is 1 (`sc:5161-5166`).

1.6 Each `sayHello()` also raises the credit by one (`sc:2217`), so both hellos are written without waiting
for an `ok` (credit capped at `ackMax` = 1, `cs:271`, `sc:743-745`, `util:1481-1490`; the exact interleaving of
the two writes is thread-timed: `unclear`).

1.7 While in `CONNECTING` the port is read with a 10 s read timeout (the `connection` timeout, `sc:4042`).
The connection deadline is `now + 30 s` (`_timeout`, `sc:2366`; `communication` = 30.0 `cs:40`; temperature
term 5.0+1 is smaller, `sc:3967-3992`). It is pushed out by 30 s each time any non-empty line arrives (`sc:2399-2401`).

1.8 Handshake line rules in `CONNECTING` (`sc:3355-3374`), checked in this order on each received line:
 a. a line containing the substring `start` (and `start` not yet seen): remember it and send one more hello (`sc:3356-3358`);
 b. a line starting with `ok`, or exactly `wait` (supportWait, `cs:247`): connected; for `wait` a simulated ok is
    first handled (`sc:3359-3363`);
 c. else if now > deadline: if fewer than 3 hellos were sent (`_hello_sent < 3`) log "No answer from the printer
    within the connection timeout, trying another hello" and send one more hello; otherwise log "There was a
    timeout while trying to connect to the printer" and close (`sc:3364-3374`).
Consequence for a silent printer: two hellos at t=0, a third at the first read timeout past 30 s, close at the
next one (about 40 s total; derived from 1.7 and 1.8c, not a printed constant).
Any `ok` line also raises the credit before this logic runs (`sc:2677-2682`, `sc:3457-3463`).

1.9 On connect (`_onConnected`, `sc:3871-3904`), in this order:
 a. read timeout becomes the communication timeout (`sc:3872`);
 b. start the temperature poll timer (`run_first=True`) and the SD status poll timer (`run_first=True`) (`sc:3873-3881`);
 c. state `OPERATIONAL` (`sc:3883`);
 d. send `M110 N0` via `resetLineNumbers` (`sc:3885-3889`, `sc:2220-2234`), same wire form `N0 M110 N0*125`;
 e. send `M115` (`sc:3890-3895`). Wire form `N1 M115*39` (the firmware name is not known yet, see 2.2);
 f. SD: since `_sdAvailable` is False at start, send `M21` (`sc:3897-3900`, `sc:2165-2179`); `feature.sdSupport` default True
    (`schema/config/feature.py:19`, read at `sc:821`). Wire form `M21` bare once the firmware name is known (2.2);
 g. fire CONNECTED and run the gcode script `afterPrinterConnected` (default `None`, nothing sent; `scr:8`, `sc:3902-3904`).
The first temperature poll may run before step c and then be skipped, or after and be queued before M110 (thread race: `unclear`, `sc:3873-3883`, `sc:3780-3794`).

1.10 Firmware detection (`sc:2973-3068`): any received line containing `NAME:` or starting with `NAME.` (and not matched
earlier by the position, temperature or `cap:` branches, `sc:2797-2973`) is parsed with the splitter
`(^|\s+)([A-Z][A-Z0-9_]*):` into KEY:value pairs, values stripped (`sc:6814-6844`, regex `sc:175`). `FIRMWARE_NAME` is taken
(`sc:2976`). Only the first such line counts (`not self._firmware_info_received`, `sc:2996`). It is stored and
`_firmware_info_received` becomes True (`sc:3066-3068`). `firmwareDetection` default True (`cs:253`).

1.11 Capability lines (`sc:2913-2951`): a line whose lower-case form starts with `cap:` is parsed
(`sc:6847-6882`): lower-cased, `cap:` removed, split on `:` must give exactly two parts, flag must be `0` or `1`,
else ignored; the name is upper-cased. So `Cap:AUTOREPORT_TEMP:1`, `cap:autoreport_temp:1` and `CAP:AUTOREPORT_TEMP:1`
are equal. Every parsed capability is stored (`sc:2917`) and offered to plugin hooks (`sc:2953-2970`).

1.12 Reaction to each capability line whose name has an enabled host setting (all `capabilities.*` default True,
`cs:86-108`) and whose flag is 1 (`sc:2919-2951`):
 - `AUTOREPORT_TEMP` -> send `M155 S2` (`sc:3821-3830`; interval = int(`timeout.temperatureAutoreport` 2.0, `cs:52`));
 - `AUTOREPORT_SD_STATUS` -> send `M27 S1` (`sc:3832-3841`; `sdStatusAutoreport` 1.0, `cs:58`);
 - `AUTOREPORT_POS` -> send `M154 S5` (`sc:3843-3855`; `posAutoreport` 5.0, `cs:61`);
 - `EMERGENCY_PARSER`, `EXTENDED_M20`, `LFN_WRITE`: log only; used later (9.1, 7.1, `sc:1605`);
 - `CHAMBER_TEMPERATURE`, `BUSY_PROTOCOL`: nothing sent here (chamber: parsing, `sc:2321-2325`; busy: see 4.7);
 - `PROMPT_SUPPORT`: handled by the prompt plugin: send `M876 P1` (`acp:234-240`, section 9.4).
These commands are queued in the order the `Cap:` lines arrive. They go out only when credit is available, i.e.
after the `ok` of `M115` (Marlin ends the report with `ok`; the `ok` of M115 raises the credit, `sc:2677-2682`).
Wire form: bare (no number, no checksum), see 2.2. Autoreport flags are set when the line is about to be written
(`_gcode_M155_sending`, `_gcode_M27_sending`, `_gcode_M154_sending`, `sc:5575-5603`): true only if the firmware
reported the capability and interval > 0.

1.13 End of capability report (`sc:2745-2786`): the first received line that is not `cap:` and not an `ok` that was
fully handled (a bare `ok` is skipped by `continue`, `sc:2692`, `sc:2738-2742`) marks the report complete: deferred SD
file list refresh is allowed (`waitToLoadSdFileList` True, `cs:207`, `sc:2752-2755`), plugin hook runs, firmware info is
published (`sc:2771-2774`). If M115 produced a firmware line but no `Cap:` lines, the info is published on the next line
(`sc:2788-2794`).

1.14 Count of `M110 N0` lines: 2 hellos (1.4) + 1 after connect (1.9d) + 1 more hello if a line containing `start` is seen while connecting (1.8a). Later `M110 N0` is sent at the start of every host-file print (6.1) and twice on an external reset (10.4). `M21` is sent once at connect (1.9f) and again only on the SD events of section 7.

---

## 2. Line numbers and checksums

2.1 Setting `sendChecksum` default `"print"` (`cs:213`, values always/print/never `cs:12-15`);
`sendChecksumWithUnknownCommands` False (`cs:216`); `checksumRequiringCommands` = `["M110"]` (`cs:172`).

2.2 A line is sent as `N<n> <cmd>*<cs>\n` if `_needs_checksum` (`sc:5134-5143`) is true, i.e. if either:
 a. its G-code word is in `checksumRequiringCommands` (only `M110`), or
 b. it is a G/M/T command (word matched by `^\s*((?P<codeGM>[GM]\d+)(\.sub)?|T\d+|F\d+)`, `sc:62-64`; `F` only when
    `supportFAsCommand`, False `cs:250`, `sc:6968-6973`) and `_checksum_enabled`: `sendChecksum != "never"` AND one of
    (`sc:5146-5152`):
    - state is a printing state (STARTING, PRINTING, CANCELLING, PAUSING, RESUMING, FINISHING; `sc:518-525`) and the current
      file is a host file (`PrintingFileInformation.checksum = True`, `sc:5972`; SD files `checksum = False`, `sc:6043`);
    - manual streaming (`_manualStreaming`, M28 sent by hand, `sc:5391-5399`);
    - `sendChecksum == "always"`;
    - `not _firmware_info_received` (no `FIRMWARE_NAME` line parsed yet).
 Anything else is sent bare: the line text plus `\n`, and the line counter is NOT incremented (`sc:5154-5159`).
 Consequences with defaults: hellos, post-connect `M110 N0` and `M115` are numbered (no firmware info yet); everything idle
 after the `FIRMWARE_NAME` line (`M21`, `M155 S2`, `M27 S1`, `M154 S5`, `M20 L T`, `M105`) is bare; during a host-file
 print every G/M/T line (file lines, scripts, polls, M400, M114, M108 ...) is numbered; during an SD print everything is bare.

2.3 Numbering: counter `_current_line` starts at 1 (`sc:656`); a numbered line takes the current value, stores the bare
command in the history deque, then increments (`sc:5161-5166`). History size `lastLineBufferSize` = 50 (`cs:187`, `sc:687-688`).

2.4 `M110 N<k>` (any source): before it is written the counter becomes `k` (last `N` parameter if several; none -> 0), the history
is cleared, any resend state is dropped, and the command is rewritten to exactly `M110 N<k>` (`sc:5615-5643`). It is then sent as
`N<k> M110 N<k>*<cs>`; the counter becomes `k+1`. The history then contains that M110 line.

2.5 Checksum: XOR of every byte of `N<n> <cmd>` (the `N`, the decimal number, one space and the command; not the `*`), written in
decimal after `*` (`sc:5168-5178`). Example: `N0 M110 N0*125`, `N1 M115*39`, `N1 M105*38`.

2.6 Before sending, every command is stripped of comments (`;` unless escaped with `\`), surrounding whitespace, and any existing
`N<n>` prefix or `*<digits>` suffix (`sc:6517-6540`, `sc:6981-7004`, `sc:4664`). Empty lines are never sent (`sc:4884-4893`).

2.7 Line number wrap: none in code; the counter is an unbounded integer reset only by M110 handling (`sc:5637`, `sc:3915`; no other
assignments: grep of `_current_line`, lines 656, 3915, 4469-4496, 4602, 5163-5165, 5637). Firmware-side limits: `unclear` (not in these sources).

2.8 Resent lines are always written numbered with their original number and checksum, whatever the setting (`sc:4844-4850`).

---

## 3. Idle

3.1 Temperature poll: timer `RepeatedTimer(interval callable, run_first=True)` (`sc:3873-3876`, `util:1249-1307`); interval re-evaluated each cycle
(`sc:3932-3959`):
 - printing/paused/pausing/cancelling (`isBusy`, `sc:1112-1117`): `timeout.temperature` = 5.0 s (`cs:46`);
 - idle and any tool target > 25 or bed target > 25 (threshold `sc:588`): `timeout.temperatureTargetSet` = 2.0 s (`cs:49`);
 - otherwise 5.0 s. (If an interval setting is <= 0 it becomes 1.0.)
The poll sends `M105` with command type `temperature_poll` (only one such command may sit in a queue at a time, `sc:6406-6414`) (`sc:3790-3794`).

3.2 Poll is skipped when any of: not operational, `_temperature_autoreporting` (M155 accepted, 1.12), closing, streaming to SD, a
long-running command pending, heating, dwelling, manual streaming (`sc:3780-3789`).

3.3 SD status poll: timer every `timeout.sdStatus` = 1.0 s (`cs:55`, `sc:3961-3965`); sends `M27` (type `sd_status_poll`) only if an SD file is
selected (and `disableSdPrintingDetection` False, `cs:268`) or an SD print is running, and not autoreporting M27, closing, long-running,
dwelling or heating (`sc:3804-3819`). With no SD file selected nothing is sent.

3.4 No position polling exists. `M114` is sent only by pause (6.6) and optionally cancel (6.8) (`sc:2062-2067`, `sc:1865-1870`). Autoreport via `M154 S5` (1.12).

3.5 Idle read timeout: the serial read timeout equals the communication timeout: `max(timeout.communication 30.0, temperature term + 1)`
(`sc:3967-3992`). Temperature term = `temperatureAutoreport` 2.0 when M155 is active, else the poll interval of 3.1. Defaults give 30.0 s
(6.0 s or 3.0 s if busy protocol is active, 4.7).

3.6 A timeout is counted when a read returns empty and `now >= _timeout` (`sc:2700-2702`), or, only while printing from a host file (not SD, job not
on hold unless resending, no long-running command, not heating), when `now >= _ok_timeout` even though other lines arrived (`sc:2703-2711`). `_timeout`
is re-armed by any non-empty line (`sc:2399-2401`), `_ok_timeout` by `ok`, `echo:busy:`/`busy:` lines and after each timeout (`sc:3462`, `sc:2415`, `sc:2731`).
Timeouts are ignored while a dwell is being blocked only if `blockWhileDwelling` (False, `cs:256`; `sc:2712-2716`).

3.7 What a timeout does (`_handle_timeout`, `sc:3486-3557`), first match wins; counter `_consecutive_timeouts` is incremented first (`sc:3501`)
and reset by any non-empty line (`sc:2400`):
 a. limit exceeded: `0 < max < count` where max is `maxCommunicationTimeouts` long=5 (a long-running command is active), printing=5 (any printing state),
    idle=2 (`cs:74-82`, `sc:3493-3498`). Idle therefore gives up on the 3rd consecutive timeout, printing/long on the 6th: error text
    "Too many consecutive timeouts, printer still connected and alive?", reason `timeout`, port closed with error (`sc:3504-3518`). No M112 is sent in this path (it
    calls `close(is_error=True)` directly, not `_trigger_error`).
 b. resend active: resend the same line again (`sc:3520-3526`, 5.6);
 c. heating: consider heat-up over (clear `_heating`), nothing sent (`sc:3528-3532`);
 d. long-running command active: ignore (`sc:3534-3538`);
 e. printing state or paused: send `M105` (type `temperature`) directly and raise the credit (`sc:3540-3550`);
 f. idle with credit exhausted: only raise the credit (`sc:3552-3557`), so the next queued line (e.g. the pending `M105` poll) can go out. Nothing is written by the timeout itself.

3.8 `wait` line: counted as an `ok` only while printing (`self.isPrinting() and supportWait and line == "wait"`, `sc:2677-2678`; `supportWait` True `cs:247`), and
while connecting (`sc:3359-3363`). While idle `wait` is just a message.

3.9 External heat-up detection (`externalHeatupDetection` True, `cs:240`): a temperature line not starting with `ok`, while not autoreporting temperature, not already heating,
after the firmware info was received, sets `_heating = True` (`sc:2861-2871`). It is cleared by the next `ok` (`sc:3473`, `sc:3673-3680`) or a timeout (3.7c). While heating, 3.2 suppresses polling.

---

## 4. Sending and acknowledging

4.1 One line in flight. The send thread waits for credit (`_clear_to_send.wait()`, `sc:4803`, `sc:4933`), writes one line, and uses one credit when the
command is a G/M/T command (or any command if `unknownCommandsNeedAck`, False `cs:219`; Klipper rule sets it, 11) (`sc:4784-4795`, `sc:4906`). `ackMax` = 1 (`cs:271`): credit
is capped at 1 (`util:1481-1490`, max 1, min None, so it may go negative when more lines are written than acknowledged). There is no window; the next line is chosen on each `ok` (`sc:3457-3484`).
Exceptions that write without waiting for credit: hellos (1.4), emergency force-sends (9.1), the timeout `M105` (3.7e), the M112 sequence (9.2), resend lines still use credit (`sc:4847`).

4.2 Source priority when an `ok` arrives (`_continue_sending`, `sc:3682-3714`): (1) command queue, (2) if the job is on hold, stop, (3) job queue, (4) next file line. Only one item is placed in
the send queue at a time.

4.3 What counts as ok (`sc:2677-2692`): any line, after stripping, that starts with the two characters `ok` (`ok`, `ok T:..`, `ok N12 P15 B3`, `okay`...), exactly `wait` while printing (3.8).
Nothing in `ok N.. P.. B..` is parsed. An `ok` that carries `T:`, `T0:`, `B:`, `C:`, `X:` or `NAME:` is still parsed further as a temperature/position/firmware line; a bare `ok`/`wait` is fully consumed (`sc:2684-2692`).
On each `ok` (`sc:3457-3484`): cancel a pending "resend ok" timer, re-arm the ok timeout, raise credit, clear the long-running flag, restore the tool remembered before a heat-up wait, clear the heating state; then, if
a resend is pending, send the next resend line, else continue with the queues.

4.4 Other "ok" equivalents: the line `Done saving file` triggers an ok if `triggerOkForM29` (True, `cs:299`; `sc:3297-3301`); a `File deleted ... ok` line triggers one (`sc:3302-3305`); a recoverable error line (5.4) and
a timeout-simulated ok after a resend (5.5) also do.

4.5 Unknown command: nothing special. `echo:Unknown command: "..."` is just a message (no `Error:` prefix) and the `ok` that follows is consumed normally. An `Error:` line containing `unknown command` is ignored (`sc:4202-4204`).
Commands that are not G/M/T (regex above) do not consume credit by default and do not get a number/checksum (`sc:5134-5143`, `sc:4906-4912`).

4.6 Blocked and ignored commands: `blockedCommands` = `["M0","M1"]` (`cs:154`) are dropped (not sent, no ok expected, `sc:5842-5856`); `ignoredCommands` empty (`cs:160`). `pausingCommands` = `["M0","M1","M25"]`
(`cs:166`): while printing, such a command (from any source except pause/cancel handling) triggers a pause (`sc:5833-5840`); M25 itself is still sent. Commands for non-existent tools (`T<n>` with n >= extruder count 1, or one reported invalid) are dropped (`sc:5316-5334`, `sc:5780-5784`); `M140`/`M190` dropped if no heated bed; `M141`/`M191` dropped when `heatedChamber` False (default profile) (`sc:5410-5448`, `prof:237-238`).

4.7 Busy protocol: a received line starting with `echo:busy:` or `busy:` (raw line, `sc:2413`): re-arms the ok timeout (`sc:2415`); on the first such line, if `capabilities.busy_protocol` True (the setting; the firmware capability is not checked, `sc:2418-2420`, `cs:95`), send `M113 S2`
(interval = max(int(`timeout.communicationBusy` 3.0) - 1, 1) = 2, `cs:43`, `sc:2429-2432`, `sc:3857-3869`); when that line has been written the busy flag is set and the read timeout is recomputed (`sc:2434-2449`). From then on the communication timeout is
`max(communicationBusy 3.0, temperature term + 1)` = 3.0 s with M155 active (2.0+1) or 3.0 when a target is set, 6.0 s when idle and polling (5.0+1) (`sc:3977-3992`). It shortens the timeout (30 -> 3..6 s), it does not extend it.
The busy line is consumed: it is not processed further (`continue`, `sc:2451-2455`), also for `busy: paused for user`/`busy: processing` (no special handling of the text). After a firmware reset (10) `M113 S2` is repeated (`sc:3927-3928`).

4.8 Long-running commands: default list `G4, G28, G29, G30, G32, M400, M226, M600` (`cs:142-151`). When such a command (or the full text equal to a list entry) reaches the sending phase the flag is set before the write (`sc:5874-5882`); also set for
`M109`, `M190`, `M191`, `M116` once written (`sc:5544-5573`). The flag is cleared by the next `ok` (`sc:3467`). The effect is not a longer timeout: the timeout value stays the same (3.5/4.7), but while the flag is set the ok-timeout variant never fires (3.6), a timeout never writes anything (3.7d), polling is suppressed (3.2, 3.3), and the number of consecutive timeouts tolerated is 5 (the `long` maximum, 3.7a). `G4` additionally pushes `_timeout` out by P/1000 or S seconds and sets a dwell deadline (`sc:5689-5703`), which suppresses polls until the next line after the deadline (`sc:2403-2404`, `sc:3787`).

4.9 Plugins/hooks can rewrite or drop commands in the phases queuing/queued/sending/sent (`sc:4958-5089`); bundled plugins that use this: the prompt plugin (9.4). Nothing else bundled sends or rewrites G-code.

---

## 5. Resends and errors

5.1 Resend lines: a stripped line whose lower-case form starts with `resend` OR starts with `rs` (`sc:2695-2696`), tested after the `ok` test. Both prefixes are plain prefixes: `Resend:12`, `Resend: 12`, `resend N12`, `rs 12`, `rs N12`, `rs N:12` and any line starting with `rs` all enter the handler. The requested number is the first run of digits found anywhere in the line, with an optional preceding `N` or `N:` (`regex_resend_linenumber` `sc:178`, `sc:6885-6900`). A line with no digit returns None: the request is counted but no resend is set up.

5.2 Every resend line (even an ignored one) increments the resend counter and re-evaluates the ratio (`sc:4461-4462`; warning once when resends/transmitted >= 10 % (`cs:279`) and transmitted > 100 (`cs:282`), `sc:928-944`); it also always runs the `finally` block (5.5).

5.3 Ignored requests (`_handle_resend_request`, `sc:4469-4513`), no state change:
 a. no resend active and requested == current line == 1;
 b. no resend active and requested == current line (the printer got N-1 twice);
 c. last error text contained `line number` or `expected line`, requested == last requested, a resend is active and fewer than `delta` repeats were seen: counted and ignored.

5.4 Otherwise (`sc:4493-4560`): delta = current_line - requested; if the same number was requested consecutively (counter not reset by successful lines, only by a different number) the repeat counter increments, and at `>= maxConsecutiveResends` = 10 (`cs:262`), i.e. on the 11th request of the same line, the error "Printer keeps requesting line N again and again, communication stuck", reason `resend_loop` is raised (fatal, see 5.9). Then: `_resendActive = True`, remember delta and requested number, check history (5.8), set the send queue to resend mode. The resends themselves are performed one per `ok`.

5.5 `finally` of every resend request (`sc:4561-4568`): `supportResendsWithoutOk` default `"detect"` (`cs:192`): start a 0.5 s timer (`timeout.resendOk` 0.5, `cs:64`; the code fallback 1.0 is not used because the setting always exists) that simulates an `ok` if none arrived. Any non-empty line that does not start with `ok` arriving while that timer runs cancels it and simulates the `ok` immediately (`sc:2406-2410`). A real `ok` cancels the timer (`sc:3458-3460`). (`always` would simulate at once, `never` not at all.)

5.6 Performing a resend (`_resendNextCommand`, `sc:4581-4622`), triggered by the `ok` (real or simulated): line number = current_line - delta; the text is the stored bare command from the history; it is queued on the priority resend queue with its original number and sent numbered with checksum (4.1, 2.8); delta is decremented; each following `ok` sends the next, in ascending order, until delta reaches 0; then the resend state is cleared (the `_resendActive` flag stays until the next `ok`, `sc:4613-4620`, `sc:3483`). While a resend is active no new lines are taken from the queues (`sc:6438-6458`). A timeout during an active resend re-sends the last resent line again (delta + 1) (`sc:3520-3526`, `sc:4586-4595`).

5.7 After the resends the sequence continues with the next never-sent line; nothing is skipped, nothing rewritten.

5.8 History check (`sc:4624-4649`): if delta > history length (50) or history empty or delta < 0: while printing raise error "Should resend line N but no sufficient history is available, can't resend", reason `resend` (fatal, 5.9); while not printing only drop the resend state.

5.9 Error lines (`_handle_errors`, `sc:4160-4259`). A line is an error line if its lower-case form starts with `error:` or it starts with `!!`; a line starting with `fatal:` is treated as `Error:fatal:...` (`sc:4166-4171`). Multi-line merge: `Error:<digit>` first lines of min/max errors take the next line appended (`sc:4172-4175`); lines matching `Heating failed|Thermal Runaway|Thermal Malfunction|MAXTEMP triggered|MINTEMP triggered|Invalid extruder number|Watchdog barked|KILL caused` are joined with the next line (`sc:90-92`, `sc:4176-4182`). Classification of the lower-case text, first match wins:
 a. contains `no line number with checksum` or `missing linenumber`: recoverable; an `ok` is simulated (`sc:4136-4137`, `sc:4188-4192`). (Prusa MK3 text `No Line Number with checksum`; Marlin 2's `No Checksum with line number` does not match, it falls to b.)
 b. contains `line number`, `linenumber`, `checksum`, `format error` or `expected line`: stored as `_lastCommError` (used in 5.3c), no other action; the firmware is expected to send `Resend:` (`sc:4139-4148`, `sc:4194-4196`).
 c. contains `volume.init`, `openroot`, `workdir`, `error writing to file`, `cannot open`, `open failed`, `cannot enter`: SD error, ignored (`sc:4149-4157`, `sc:4198-4200`).
 d. contains `unknown command`: ignored (`sc:4202-4204`).
 e. anything else (and not already in an error state): with `errorHandling = "disconnect"` (default, `cs:181`) or a text containing `kill() called` / `fatal:` (`sc:4158`): `_trigger_error(text, "firmware")` (5.10). (With `cancel` the print is cancelled and the credit raised; with `ignore` a warning only; not default.)

5.10 `_trigger_error` (`sc:4261-4290`): state `ERROR`; `sendM112OnError` True (`cs:265`) and reason not `connection`/`autodetect` -> send the M112 sequence (9.2) without closing, then close the port with error (state `CLOSED_WITH_ERROR`). The guard `not isSdPrinting()` is evaluated after the state was already changed to `ERROR` (`sc:4263-4268`), so it is always true: M112 is also sent when an SD print was running (observed from code order; not a documented intent). An `Error:` line that Marlin sends together with a halt (`Printer halted. kill() called!`) therefore ends in M112 + disconnect.

5.11 No simulated `ok` is produced for fatal errors; the connection is closed. Recoverable (5.9a) and resend (5.5) cases are the only simulated oks besides timeouts of the credit (3.7f), `triggerOkForM29` (4.4) and the `wait` line (3.8).

5.12 `maxWritePasses` = 5 (`cs:127`): a write that makes no progress is retried; after more than 5 consecutive zero-byte passes the port is closed with error "Could not write to serial port" (with sleeps of (passes-1)/10 s from pass 2) (`sc:5190-5261`). A write timeout is retried once (`sc:5201-5226`).

---

## 6. Printing a file from the host

6.1 Start (`startPrint`, `sc:1517-1602`), all lines go through the job queue in this order:
 1. state `STARTING` (`sc:1544`);
 2. `M110 N0` (host file only; `sc:1546-1549`) -> `N0 M110 N0*125` (counter then 1);
 3. the `beforePrintStarted` script lines (default `None`: nothing, `scr:14`, called at `std:1391` with `part_of_job=True`);
 4. a marker that switches to `PRINTING` (`sc:1593-1596`);
 5. then file lines, one per `ok` (6.2).
 No `M105`, `M155`, `M110` variants or heat-up commands are added by OctoPrint beyond what the file contains.

6.2 Streaming: after each `ok`, priority command queue > job queue > file (4.2). A file line is: read with newline kept, comments (`;`, not `\;`) stripped, trimmed, temperature offsets applied (none by default, `sc:6466-6514`), skipped if empty (`sc:6186-6234`). Each is sent as `N<n> <line>*<cs>` (2.2) after the queuing/sending hooks (4.6, 4.8). Position and progress are taken from bytes read (`sc:6221`). `M73` lines in the file update progress (`trustM73` True `cs:302`, `sc:5741-5763`); they are still sent.

6.3 Polling during a print: `M105` every 5.0 s (3.1) unless M155 is active; it goes through the command queue (sent before the next file line, only one pending) (`sc:1367-1380`, `sc:3780-3794`); a long-running command, heating or dwell pauses the polling (3.2). `M27` is not polled for host prints (3.3).

6.4 Heat-up: M109, M190, M191, M116 set the long-running and heating flags when written (`sc:5544-5573`); OctoPrint adds no blocking of its own: the one-line-in-flight rule makes the file wait for the `ok`. On `M109 ... T<n>` the "current tool" is switched for the duration and restored at the `ok` (`sc:5467-5469`, `sc:3469-3471`). A timeout during heating ends the heating state only (3.7c).

6.5 End of file (`sc:4374-4400`): state `FINISHING`; `M400` (job queue, numbered); print-done callback -> `afterPrintDone` script (default `None`, `scr:22`, `std:1501`); marker -> `OPERATIONAL`. Wire form `N<n> M400*<cs>`.

6.6 Pause (`setPause(True)`, `sc:2029-2090`), host file, defaults (`logPositionOnPause` True `cs:195`):
 1. state `PAUSING` (file lines stop; `sc:3685-3690`);
 2. if position autoreport is active: `M154 S0` (job queue, `sc:2074-2079`);
 3. `M400` (job queue); when it has been written a 10 s timer (`positionLogWait` 10.0 `cs:70`) starts and `M114` is queued (job queue); once `M114` is written, `M154 S5` is queued again if it was active (command queue) (`sc:2041-2067`);
 4. when a parseable position line (`X: Y: Z: E:`, `sc:150-154`, `sc:2797-2851`) arrives, or the 10 s timer fires ("Did not receive parseable position data ... continuing without it", `sc:1920-1928`), the pause callback runs: script `afterPrintPaused` (see note 6.10; no default content), then a marker moves the state to `PAUSED` (`sc:1930-1956`).
 No `M25` for host files; nothing else is sent. SD files: see 7.5.

6.7 Resume (`setPause(False)`, `sc:1995-2027`): state `RESUMING`; script `beforePrintResumed` (see 6.10; no default content); marker -> `PRINTING`; file lines continue. Nothing is sent by default for a host print (no position restore).

6.8 Cancel (`cancelPrint`, `sc:1794-1918`), host file, defaults:
 1. callback "cancelling" (event only), state `CANCELLING` (`sc:1836-1843`); file lines stop;
 2. `abortHeatupOnCancel` True (`cs:201`): `M108` with `force=True`, not part of the job (`sc:1872-1879`). With `EMERGENCY_PARSER` supported (9.1) it is written at once, bypassing credit, wire form `N<n> M108*<cs>` (it uses up one credit); without the capability it is queued like any command and waits for credit (so it cannot interrupt a pending `M109`);
 3. `logPositionOnCancel` False (`cs:198`): no `M400`/`M114` by default; the cancel is finalized immediately: file marked done, callback cancelled -> script `afterPrintCancelled` (job queue, 6.9), marker -> `OPERATIONAL` (`sc:1899-1918`, `sc:1769-1792`). If enabled: `M154 S0` (if active), `M400`, `M114` with the same pattern as 6.6 (`sc:1899-1916`).
 No `M410` and no `M112` are sent by this path. The job queue and command queue are not emptied (anything already queued is still sent, `sc:1794-1918`; no `clear` on `_job_queue`, grep `sc:` 1365, 4448).

6.9 Default `afterPrintCancelled` script (`scr:17-18`), rendered with the default profile (1 extruder, not shared, heated bed) from the snippets `scr:37-40` (`set:840-851`, `set:2216-2236`, `std:1570`, `con:206-234`): after comment/blank filtering (`sc:1477-1482`) the lines sent are, in order, `M84`, `M104 T0 S0`, `M140 S0`, `M106 S0`; (`M104 T<k> S0` once per tool; `M140 S0` only with heated bed). They are sent numbered and checksummed for a host print (state still `CANCELLING`), bare for an SD print.

6.10 Script naming mismatch (checked): the code runs `afterPrintPaused` and `beforePrintResumed` (`std:1431`, `std:1456`, `sc:1400`), but the config schema defines `beforePrintPaused` and `afterPrintResumed` (`scr:25-29`). A script is looked up as `gcode/<name>` in the settings tree and in the scripts folder (`set:944-962`, `set:853-887`); with defaults `afterPrintPaused` and `beforePrintResumed` do not exist, so no script runs on pause/resume. All other scripts default to `None` except `afterPrintCancelled` (6.9) (`scr:8-35`).

6.11 Tool changes: `T<n>` runs `beforeToolChange`/`afterToolChange` (default `None`, `scr:31-35`, `sc:5296-5314`).

6.12 Streaming to SD (M28/M29 upload), only for completeness: `M110 N0`, `M28 <remote name>`, lines numbered, `M29`, then `M20` refresh (`sc:1623-1710`).

---

## 7. SD card prints

7.1 Listing: `M20 L T` if `EXTENDED_M20` is both enabled in the host settings (default True, `cs:104`) and reported by the firmware, else `M20` (`sc:2149-2152`). Sent only when operational, not busy, the capability report was processed (`waitToLoadSdFileList` True `cs:207`) and an SD card is known to be available (`sc:2130-2161`). Triggers: when `SD card ok`/`Card successfully initialized` is seen while not available (`sc:3158-3163`), on connect if already available (`sc:3897-3898`), `//action:sd_inserted`/`sd_updated` (8), after deleting a file (`sc:2128`). Response parsing: lines between `Begin file list` and `End file list`; each line `name [size [timestamp] [longname]]` (`sc:6717-6766`); only machine-code file types are kept; the first name starting with `/` selects a `/` prefix (`sc:2619-2625`); names are lower-cased only if `sdLowerCase` (False default `cs:228`; the Prusa rule sets it, 11); timestamps before 2008-05-29 are dropped (`sc:202`, `sc:2629-2632`).

7.2 `M21` (init): `M21` on connect (1.9f) and from the UI; `M22` (release): only from the UI, never automatically (`sc:2181-2203`). `sdAlwaysAvailable` False (`cs:225`). SD becomes available on `SD card ok`/`Card successfully initialized`, unavailable on any of `SD init fail`, `volume.init failed`, `No media`, `openRoot failed`, `SD Card unmounted`, `SD card released` (`sc:3147-3163`).

7.3 Select: `M23 <filename>` exactly as in the file list (no case change; the leading `/` is removed only if `sdRelativePath`, False `cs:222`) (`sc:1712-1735`). The answer `File opened:<name> Size:<n>` (regex `sc:104-106`) creates the SD job (name = the requested one) (`sc:3233-3279`). `File selected` is ignored.

7.4 Start (`startPrint` with an SD file, `sc:1553-1596`): state `STARTING`; no `M110`; script `beforePrintStarted` lines (default none); then, in the job queue and bare: `M23 <filename>` again (select is repeated and its `File opened` is ignored with `_ignore_select`), then `M26 S<pos>` only if a resume position > 0 is given, then `M24`; marker -> `PRINTING`. Progress from `M27` (7.7).

7.5 Pause (SD): `M25` (job queue) then, with `logPositionOnPause`, `M400` and `M114` as in 6.6; `afterPrintPaused` (none by default) (`sc:2029-2090`). Resume: `M24`, then `M27` (`sc:2007-2018`).

7.6 Cancel (SD, not external): `M108` (6.8 step 2, only effective with the emergency parser), then `sdCancelCommand` = `M25` (`cs:231`), `M27`, `M26 S0` (job queue), then 6.8 step 3 and the `afterPrintCancelled` script (`sc:1881-1897`). For an SD print started or detected externally (`external_sd=True`, e.g. after `Not SD printing`, 7.8) the three SD commands are not sent (`sc:1882`).

7.7 Status: `M27` every 1.0 s (3.3) or `M27 S1` autoreport (1.12). Response `SD printing byte <cur>/<total>` (regex `sc:95`): updates position and progress (`sc:3172-3219`). If not yet printing, 0 < cur != total and cur > last position, an SD print started externally is adopted (`startPrint(external_sd=True)`, no M23/M24) (`sc:3205-3212`).

7.8 `Not SD printing` (and `SD printing byte 0/0` while printing): while an SD print is running (not STARTING/FINISHING) each such message increments a counter; when it exceeds `maxNotSdPrinting` = 2 (`cs:234`), i.e. on the 3rd consecutive, the print is cancelled as external (`cancelPrint(external_sd=True)`) (`sc:3184-3200`, `sc:3220-3232`). The counter resets on any other `SD printing byte` report and on start/reset.

7.9 `Done printing file` while SD printing (`sc:3282-3296`): state `FINISHING`, `M400` (job queue), done callback (`afterPrintDone`, none), marker -> `OPERATIONAL`.

7.10 Delete: `M30 <name lower-cased>` always lower-cased (hard-coded `.lower()`, `sc:2121-2128`). `M33` mapping is only parsed if sent by a user (`sc:5605-5613`).

---

## 8. Action commands (`//action:...`)

8.1 Recognition: after the busy check, a line starting with `//` is a debug line; its text after `//` is stripped; if it then starts with `action:` the rest is stripped and split at the first space into name and params (`sc:2458-2467`). `// action:x` therefore works too. In every state except `CONNECTING`, any `//` line is consumed and not processed further (`sc:2582-2586`).

8.2 Actions:
 - `start`: if a file is selected, `startPrint()` (re-run; ignored unless operational and not printing) (`sc:2469-2477`, `sc:1518-1519`).
 - `cancel`: `cancelPrint()` (6.8/7.6) (`sc:2478-2485`).
 - `pause`: `setPause(True)` (6.6/7.5) (`sc:2486-2492`).
 - `paused`: `setPause(True, local_handling=False)`: state `PAUSING` -> pause callback with scripts suppressed -> `PAUSED`; nothing sent (no `M25`, `M400`, `M114`) (`sc:2493-2502`, `sc:2034-2039`, `sc:2069-2090`).
 - `resume`: `setPause(False)` (6.7/7.5) (`sc:2503-2510`).
 - `resumed`: `setPause(False, local_handling=False)`: state `RESUMING` -> `PRINTING`, scripts suppressed; SD: only `M27`; nothing else sent (`sc:2511-2520`, `sc:2007-2018`).
 - `disconnect`: calls the force-disconnect callback (`sc:2521-2526`); the connection is closed normally (non-error): the `beforePrinterDisconnected` script (none) is sent, up to 10 s waiting for queues, then the port is closed (`sc:1225-1333`).
 - `shutdown`: only if `enableShutdownActionCommand` (False `cs:295`), otherwise a warning is logged; nothing is sent (`sc:2527-2541`).
 - `sd_inserted`: SD available, `M20 [L T]` (7.1); `sd_ejected`: SD unavailable, list cleared; `sd_updated`: `M20 [L T]` (`sc:2542-2561`), only if SD support is on.
 - `prompt_begin <text>`, `prompt_choice <text>` or `prompt_button <text>`, `prompt_show`, `prompt_end`: plugin `action_command_prompt` (9.4) (`acp:160-197`).
 - `notification <text>`: plugin `action_command_notification` (`plugins/action_command_notification/__init__.py`, handler near line 130); nothing is written to the printer by it (grep for `commands(`/`sendCommand` in that file finds no match).
 - any other name: no host action (hooks only, `sc:2563-2580`).

8.3 `@pause`, `@cancel`, `@abort`, `@resume` in G-code are host commands (never sent to the printer): `setPause(True)`, `cancelPrint()`, `setPause(False)` (`sc:5794-5812`).

---

## 9. Emergency and other commands

9.1 Emergency commands list `["M112","M108","M410"]` (`cs:169`). Any of them from any source skips the command queue (`sc:1359`). In the queuing phase `M108` and `M410` are written at once, outside credit, only if `EMERGENCY_PARSER` is enabled in the host (True `cs:101`) and reported by the firmware (`_capability_supported`, `sc:908-911`, `sc:5765-5778`, `sc:5824-5831`); one credit is used up for it. If the capability is missing they are queued as ordinary commands (the helper returns None, `sc:5767-5768`).

9.2 `M112` (`sc:5645-5682`): always handled specially regardless of the capability. From the UI/API (`con:156-157`) or any `M112` command: write raw `M112` (bare), then `N<n> M112*<cs>`, then `N<n+1> M104 T0 S0*<cs>` (one per tool), then `N<m> M140 S0*<cs>` (if heated bed), then close the port with error "Closing serial port due to emergency stop M112." (state `CLOSED_WITH_ERROR`, no disconnect script). Same sequence (without the close at that point) from `_trigger_error` (5.10); there the port is closed straight after.

9.3 Other commands OctoPrint writes by itself (complete list in `sc:` grep): `M400` at end of print (6.5/7.9) and before position logging (6.6); `M114` on pause (6.6); `M105` (3.1, 3.7e); `M27` (3.3, 7.6, 7.5); `M20`, `M21`, `M22`, `M23`, `M24`, `M25`, `M26`, `M30` (7); `M28`/`M29` for uploads; `M108` (6.8); `M110`, `M115`, `M113`, `M154`, `M155` (1, 4.7). No `M410`, no `M999`, no `M117`, no `M73` writes, no `G28`.

9.4 `M876` (plugin `action_command_prompt`, bundled): setting `enable` = `"detected"`, `command` = `M876`, `enable_emergency_sending` True, `enable_signal_support` True (`acp:98-104`). With `detected`, the plugin is active only after `Cap:PROMPT_SUPPORT:1` (`acp:237`). Then it sends `M876 P1` once (normal queue, not forced) when that capability line arrives (`acp:234-240`). (With `enable` = `always` it sends `M876 P1` at connect, `acp:86-92`.) A prompt is built from `//action:prompt_begin <text>`, `prompt_button <text>`/`prompt_choice <text>`, shown on `prompt_show`, closed on `prompt_end` (`acp:160-197`). When the user answers choice `n` it sends `M876 S<n>` with `force=True` (index of the button, 0-based; `acp:269-282`), and any `M876 S..` queued by anyone is force-sent immediately (out of band) when the emergency parser is supported (`acp:201-230`); otherwise it is queued normally. The prompt is closed on disconnect (`acp:93-94`).

9.5 `M400` timeouts: long-running (4.8). `M114` is answered with a line matching `X:.. Y:.. Z:.. E:..` (`sc:150-154`).

---

## 10. Printer restart detected mid-session

10.1 In any operational state a received line that is exactly `start` (after stripping) is treated as a reset (`sc:3378`) (during `CONNECTING` a line containing `start` is the handshake case 1.8a). The job is put on hold while handling (`sc:3379`). Fires event PRINTER_RESET with `idle` (`sc:3407-3409`).

10.2 Idle (`STATE_OPERATIONAL`): logs "Printer sent 'start' while already operational. External reset? Resetting line numbers to be on the safe side" and runs `_on_external_reset` (`sc:3380-3389`).

10.3 Printing (any printing state, including streaming to SD): logs "... while printing. External reset? Aborting job since printer lost state.", runs `_on_external_reset`, then `cancelPrint(disable_log_position=True)` (6.8/7.6; the job is cancelled, `afterPrintCancelled` runs) (`sc:3391-3405`).

10.4 `_on_external_reset` (`sc:3906-3930`): with the queues blocked: reset credit to 0; empty the command queue and the send queue (the job queue is not emptied); line counter to 0; history cleared. Then, in this order: a hello `M110 N0` (forced, wire `N0 M110 N0*125`), `M110 N0` again via `resetLineNumbers` (same wire form), then `M155 S2` if temperature autoreport was active, `M27 S1` if SD autoreport was active, `M154 S5` if position autoreport was active, `M113 S2` if busy protocol was in use; consecutive-"not SD printing" counter to 0. `M115` and `M21` are not repeated and the firmware info is kept.

---

## 11. Firmware-name rules (only with `firmwareDetection` True, default `cs:253`; substring match on the lower-cased `FIRMWARE_NAME`, first match wins; `sc:3003-3064`)

| Rule (substring) | Settings changed | Cite |
|---|---|---|
| `repetier` or `anet_a8` | `sendChecksum` -> always; `blockWhileDwelling` -> True; Repetier target temperatures `TargetExtr<n>:`/`TargetBed:` parsed; external heat-up detection off | `sc:3004-3015` |
| `reprapfirmware` | `sdRelativePath` -> True | `sc:3017-3021` |
| `malyan` (name taken from `NAME`+`VER` if `FIRMWARE_NAME` is absent, also the `NAME.` form) | `sendChecksum` -> always; `blockWhileDwelling` -> True; `sdAlwaysAvailable` -> True and `M21` sent if SD was not yet available | `sc:2978-2994`, `sc:3023-3034`, `sc:6832-6836` |
| `teacup` | external heat-up detection off | `sc:3036-3041` |
| `klipper` | `unknownCommandsNeedAck` -> True | `sc:3043-3048` |
| `ultimaker2` | `disableSdPrintingDetection` -> True | `sc:3050-3055` |
| `prusa-firmware` (Prusa MK2.5/MK3 firmware: `FIRMWARE_NAME:Prusa-Firmware 3.8.1 based on Marlin ...`, `/Users/timo/Documents/Coding/Maker-Connect/reference/Prusa-Firmware/Firmware/Marlin_main.cpp:5794`) | `sdLowerCase` -> True; `sendChecksumWithUnknownCommands` -> True; `unknownCommandsNeedAck` -> True | `sc:3057-3064` |
| none of the above (plain `Marlin ...`) | nothing changes | `sc:3003-3064` |

The M115 line of upstream Marlin starts `FIRMWARE_NAME:Marlin` (`/Users/timo/Documents/Coding/Maker-Connect/reference/Marlin/Marlin/src/gcode/host/M115.cpp:92`). Whether Prusa's Buddy firmware reports a name matching the `prusa-firmware` rule: `unclear` (not checked; the Buddy source has `SHORT_BUILD_VERSION "bugfix-2.0.x"` in `Prusa-Firmware-Buddy/lib/Marlin/Marlin/src/inc/Version.h:28`, but its M115 string was not read).

---

## 12. Summary of defaults used (all `cs:` unless noted)

`sendChecksum` print (213); `helloCommand` `M110 N0` (175); `suppressSecondHello` False (178); `waitForStartOnConnect` False (204); `timeout.connection` 10.0 (37); `communication` 30.0 (40); `communicationBusy` 3.0 (43); `temperature` 5.0 (46); `temperatureTargetSet` 2.0 (49); `temperatureAutoreport` 2.0 (52); `sdStatus` 1.0 (55); `sdStatusAutoreport` 1.0 (58); `posAutoreport` 5.0 (61); `resendOk` 0.5 (64); `positionLogWait` 10.0 (70); `maxCommunicationTimeouts` idle 2 / printing 5 / long 5 (74-82); `maxWritePasses` 5 (127); `longRunningCommands` G4 G28 G29 G30 G32 M400 M226 M600 (142); `blockedCommands` M0 M1 (154); `pausingCommands` M0 M1 M25 (166); `emergencyCommands` M112 M108 M410 (169); `checksumRequiringCommands` M110 (172); `errorHandling` disconnect (181); `lastLineBufferSize` 50 (187); `supportResendsWithoutOk` detect (192); `logPositionOnPause` True (195); `logPositionOnCancel` False (198); `abortHeatupOnCancel` True (201); `waitToLoadSdFileList` True (207); `sdCancelCommand` M25 (231); `maxNotSdPrinting` 2 (234); `externalHeatupDetection` True (240); `supportWait` True (247); `blockWhileDwelling` False (256); `maxConsecutiveResends` 10 (262); `sendM112OnError` True (265); `ackMax` 1 (271); `sanityCheckTools` True (274); all `capabilities.*` True (86-108); `resendRatioThreshold` 10 and `resendRatioStart` 100 (279-282); `encoding` ascii (288); `enableShutdownActionCommand` False (295); `triggerOkForM29` True (299); `trustM73` True (302); `feature.sdSupport` True (`schema/config/feature.py:19`).

---

## Marked `unclear`

- Exact interleaving of the two start-up hellos and the resulting one-ok offset (1.6): the credit logic is thread-timed (`sc:2217`, `sc:4803`, `sc:4906`).
- Whether the first temperature poll is queued before or after the post-connect `M110`/`M115` (1.9).
- Pyserial framing defaults beyond parity (1.2).
- Line-number range limits of the firmware (2.7).
- Whether Prusa Buddy's `FIRMWARE_NAME` matches the `prusa-firmware` rule (11).
- The schema/code script-name mismatch (6.10) is observed in this checkout; whether a user-level script folder exists is outside the sources.
