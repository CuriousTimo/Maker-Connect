//! The DYMO M-series scale's port (`docs/agent-engine.md`, Step 3e): a port of **usbscale**
//! (github.com/erjiang/usbscale, commit `9ed1cb55a2dfd0daeb41548031a49ab756cb9114`, in
//! `reference/usbscale`; Copyright (C) 2011--2023 Eric Jiang; **GPL-3.0-or-later**, which GPLv3 section 13
//! lets this AGPL-3.0 program combine with). Its source, function by function:
//!
//! | usbscale | here |
//! |---|---|
//! | `usbscale.c` `UNITS[13]` | [`usbscale::UNITS`] |
//! | `scales.h:15` `WEIGH_COUNT` | [`usbscale::WEIGH_COUNT`] |
//! | `usbscale.c` `print_scale_data` (380-456) | [`usbscale::print_scale_data`] |
//! | `usbscale.c` `main` (the extra read, `weigh_count`, the read loop; 179, 270-345) | [`port`], `read_loop` |
//!
//! Checked against HID Usage Tables 1.6 ch. 33 and HID POS 1.02 section 6.2.4 (`reference/hid-usage-tables/`),
//! used only to verify (`scripts/usbscale-audit-check.py`, `docs/audits/usbscale.md`).
//!
//! Ours, decided E1 to E11 in the doc: hidapi instead of libusb ([`hid`], E1), the device by usage page
//! 0x8D ([`hid::select`], E2), reads on and on (E3), the telemetry (E4), looking again every 15 s (E5), a
//! read timeout of 1 s (E6), events on change (E7), the bytes as hidapi returns them (E8), report ID 4 as
//! usbscale parses it (E9). Nothing is ever sent to the scale: [`hid::HidDevice`] has no write.
//!
//! Not ported: usbscale's `find_nth_scale`/`scales.h` VID:PID table (E2), `--index`, `--human`, `zero`
//! (tare), argp, libusb (all of it).

pub mod hid;
pub mod port;
pub mod usbscale;

pub use port::UsbScale;

#[cfg(test)]
mod tests;
