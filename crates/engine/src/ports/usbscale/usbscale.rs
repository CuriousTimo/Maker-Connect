// Port of usbscale (github.com/erjiang/usbscale, commit 9ed1cb55a2dfd0daeb41548031a49ab756cb9114),
// `usbscale.c` and `scales.h`. Copyright (C) 2011--2023 Eric Jiang.
//
// This program is free software: you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version (SPDX: GPL-3.0-or-later).
//
//! `print_scale_data` and `UNITS`, ported. usbscale prints what a report says and tells its caller
//! whether to read on; here the report is decoded into a [`ScaleReading`] (E4) and the caller reads on
//! for every status (E3).

use maker_connect_protocol::ScaleReading;

/// `scales.h:15`: "the number of the weighing result to show, as the first result may be incorrect".
pub const WEIGH_COUNT: i32 = 2;

/// `usbscale.c`: `#define WEIGH_REPORT_SIZE 6`.
pub const WEIGH_REPORT_SIZE: usize = 6;

/// `UNITS[13]`: "all the unit abbreviations as set forth by HID Point of Sale Usage Tables, version
/// 1.02. The unit code returned by the scale is the index of its corresponding string."
pub const UNITS: [&str; 13] = [
    "units",  // unknown unit
    "mg",     // milligram
    "g",      // gram
    "kg",     // kilogram
    "cd",     // carat
    "taels",  // lian
    "gr",     // grain
    "dwt",    // pennyweight
    "tonnes", // metric tons
    "tons",   // avoir ton
    "ozt",    // troy ounce
    "oz",     // ounce
    "lbs",    // pound
];

/// A report `print_scale_data` refuses (E8): "Error reading scale data", where usbscale returns -1 and
/// the program ends; here the port goes on reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// "The scale's first byte, its 'report', is always 3" (or 4): it was not.
    NotAReport(u8),
    /// Fewer than `WEIGH_REPORT_SIZE` bytes: usbscale reads into a 6-byte buffer and never looks at
    /// the length, so a short report has no decoding there (ours, E8).
    Short(usize),
}

/// usbscale's own text when a report is refused.
pub const ERROR_READING: &str = "Error reading scale data";

/// The status text usbscale prints for a status (the `fprintf(stderr, ...)` of each `case`), "unknown n"
/// where it has none (E4); empty for 4, for which usbscale prints the weight line instead.
pub fn status_text(status: u8) -> String {
    match status {
        0x01 => "Scale reports Fault".to_owned(),
        0x02 => "Scale is zero'd...".to_owned(),
        0x03 => "Weighing...".to_owned(),
        // the only final, successful status: usbscale prints "%g %s" of the weight and the unit
        0x04 => String::new(),
        0x05 => "Scale reports Under Zero".to_owned(),
        0x06 => "Scale reports Over Weight".to_owned(),
        0x07 => "Scale reports Calibration Needed".to_owned(),
        0x08 => "Scale reports Re-zeroing Needed!".to_owned(),
        other => format!("unknown {other}"),
    }
}

/// `UNITS[unit]`, or "unknown unit n" above 12, where usbscale reads beyond its table (E4).
pub fn unit_name(unit: u8) -> String {
    match UNITS.get(unit as usize) {
        Some(name) => (*name).to_owned(),
        None => format!("unknown unit {unit}"),
    }
}

/// `print_scale_data`: "takes the 6 bytes of binary data sent by the scale and interprets" them. `dat` is
/// what hidapi returned, report ID first (E8).
///
/// E9: report ID 4 is parsed exactly as 3 (its meaning is unknown: POS calls it the 2-byte Status Report).
pub fn print_scale_data(dat: &[u8]) -> Result<ScaleReading, Rejected> {
    if dat.len() < WEIGH_REPORT_SIZE {
        return Err(Rejected::Short(dat.len()));
    }
    let report = dat[0];
    let status = dat[1];
    let unit = dat[2];
    // "scaling applied to the data as a base ten exponent"; signed
    let expt = dat[3] as i8;
    // "combine the little-endian weight bytes: dat[4] is the low byte and dat[5] is the high byte"
    let raw_weight = u16::from(dat[4]) | (u16::from(dat[5]) << 8);
    let weight = f64::from(raw_weight);
    // `weight * pow(10, expt)`: the C library's pow, as usbscale calls it
    let weight = weight * 10f64.powf(f64::from(expt));
    // "The scale's first byte, its 'report', is always 3."
    if report != 0x03 && report != 0x04 {
        return Err(Rejected::NotAReport(report));
    }
    Ok(ScaleReading {
        status,
        status_text: status_text(status),
        unit,
        unit_name: unit_name(unit),
        exponent: expt,
        raw: raw_weight,
        weight,
    })
}
