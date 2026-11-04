# Printer protocols: notes

What we learn from reading other projects about how printers are talked to. **In our own
words only.** Prusa Link and the Prusa Connect SDK (`reference/`) are "Freeware", not open
source: read them to understand, never copy their code. OctoPrint (AGPL-3.0) may be
borrowed from. The firmware of the MINI, MK4 and XL (Prusa-Firmware-Buddy) is GPL-3.0 for
its code, which may be combined with AGPL code (what is taken stays GPL); its graphics and
design are CC BY-NC-SA: never used here.

## Marlin over USB (OctoPrint, `plugins/serial_connector/serial_comm.py`)

- Every line numbered and with a checksum: `N12 G1 X10*85`, the checksum the XOR of every
  byte before `*`. `M110 N0` sets the line number back to zero.
- `ok` acknowledges a line; `Resend: N` (or `rs N`) asks for line N again, after an
  `Error:` line and before an `ok` that belongs to the request.
- `busy: processing` / `echo:busy` while heating or homing: the printer is alive.
- Opening the port resets most boards: wait for `start`.

Built in `crates/agent/src/marlin.rs` with one line in flight at a time.

## Where Prusa's firmware differs (from Prusa Link, `prusa/link/serial` and `printer_adapter`)

- **Identifying the printer:** on connecting, Prusa Link sends `PRUSA Fir` (the firmware
  version) and `M862.2 Q` (the printer model code). Both are Prusa commands, not Marlin's.
- **Automatic reports:** `M155 S2 C7` asks for temperatures, positions and fans every 2 s;
  the `C` bit mask is Prusa's addition to Marlin's `M155 S<seconds>`.
- **More than one line in flight:** Prusa Link keeps the printer's planner fed
  (`is_planner_fed.py`) instead of waiting for each `ok`, so a resend can mean several
  lines back. To look into before a print with many short moves (curves, small features).

## Learned on a real MK3S (2026-11-04)

An Original Prusa MK3S (Einsy, Prusa-Firmware 3.x), driven by the agent on a Raspberry Pi
Zero 2 W on its RPi port, at 115200 baud:

- **The Pi:** Prusa's PrusaLink image is 32-bit Raspbian 11 (`armv7l`, glibc 2.31) even on
  a Zero 2 W, so it takes the `linux-armv6` agent. Its serial port is already set up:
  `/dev/serial0` is `ttyAMA0`, the full UART. `prusalink stop` frees the port, until the
  next start of the Pi.
- **`M105`** answers `ok T:24.5 /0.0 B:23.1 /0.0 T0:24.5 /0.0 @:0 B@:0 P:0.0 A:28.2`: each
  temperature, after the `/` what it heats to (0: off), and `@:` / `B@:` the heaters' power
  (0: none now). Read by `marlin::heat`. Near its target a heater switches on and off by
  itself, so "has power" flickers there.
- **`G28`** takes about a minute and says what the Trinamic drivers do while homing
  (`tmc2130_home_enter`, `tmc2130_goto_step`, `tmc2130_home_exit`) before its `ok`.
- **`G80`** measures a 7 × 7 mesh; **`G81`** prints it, the back row first.
- **The agent waits for each `ok`**, so a command that takes a minute holds it. It now
  reports *busy* every 2 s meanwhile (the cloud counts a machine offline after 10 s of
  silence), and the cloud gives a busy machine no next command and no job.

## Prusa Connect (from the Connect SDK)

- The printer registers with a code (`POST /p/register`) and gets a token.
- It then keeps sending its state (`POST /p/telemetry`); a command for it comes back in the
  reply. Events (`POST /p/events`) and camera snapshots (`/p/camera`) go the same way.
- Plain HTTPS polling started by the printer: no port to open where the printer stands.
  Maker Connect's agent protocol works the same way (`crates/protocol`).

## The MINI, MK4 and XL talk to Connect themselves (Prusa-Firmware-Buddy, `src/connect`)

- No Prusa Link on a Pi: the firmware is the Connect client.
- Its server is **not fixed**: host, port and TLS are settings (`connect_host`,
  `connect_port`, `connect_tls` in the printer's store; default `buddy-a.connect.prusa3d.com`
  on 443 with TLS). They can be set from a USB stick, `prusa_printer_settings.ini`, section
  `[service::connect]`: `hostname`, `port`, `tls`, `token`, `proxy_hostname`. With
  `tls = false` it speaks plain HTTP, so a Maker Connect cloud on the LAN could serve it
  directly, if the cloud speaks the printer side of Connect's protocol.
- The file is **not on a stick by default**: you write it yourself, and the printer only
  reads it when asked, from the menu (Prusa Connect: "Load Settings", or Settings: "Load
  Settings From File"). Keys: `hostname` (an IP address works), `port` (default 80 without
  TLS, 443 with), `tls` (`true`/`false` or `1`/`0`), `token`, `custom_cert` (then
  `connect.der` from the stick), `proxy_hostname`, `proxy_port`. **Loading it overwrites all
  of these**, the token included: a file without `token` leaves the printer without one, so
  it has to be registered again with Prusa Connect afterwards.
- Read in the newest firmware; whether firmware 6.3.0 (the user's MINI) has the same
  settings is still to be checked in an older version.
