//! `src/labelle/lib/constants.py` (labelle 5e8f1e5): what the print path uses of it.
//!
//! labelle: Copyright (c) 2011 Sebastian J. Bronner <waschtl@sbronner.com>. "Copying and
//! distribution of this file, with or without modification, are permitted in any medium without
//! royalty provided the copyright notice and this notice are preserved."

pub const UNCONFIRMED_MESSAGE: &str = "WARNING: This device is not confirmed to work with this software. Please report your experiences in https://github.com/labelle-org/labelle/issues/4";

/// `SUPPORTED_PRODUCTS` (`constants.py:37`): product id and its name. The names of the
/// unconfirmed ones end in `UNCONFIRMED_MESSAGE` (the f-strings of the source).
pub fn supported_products() -> Vec<(u16, String)> {
    let unconfirmed = |name: &str| format!("{name} {UNCONFIRMED_MESSAGE}");
    vec![
        (0x0011, "DYMO LabelMANAGER PC".to_owned()),
        (0x001C, "DYMO LabelMANAGER PC II".to_owned()),
        (0x0015, "LabelPoint 350".to_owned()),
        (0x1001, "LabelManager PnP (no mode switch)".to_owned()),
        (0x1002, "LabelManager PnP (mode switch)".to_owned()),
        (0x1003, unconfirmed("LabelManager 420P (no mode switch)")),
        (0x1004, unconfirmed("LabelManager 420P (mode switch)")),
        (0x1005, "LabelManager 280 (no mode switch)".to_owned()),
        (0x1006, "LabelManager 280 (no mode switch)".to_owned()),
        (
            0x1007,
            unconfirmed("LabelManager Wireless PnP (no mode switch)"),
        ),
        (
            0x1008,
            unconfirmed("LabelManager Wireless PnP (mode switch)"),
        ),
        (0x1009, unconfirmed("MobileLabeler")),
    ]
}

/// Whether `SUPPORTED_PRODUCTS` has this product id (`id_product in SUPPORTED_PRODUCTS`).
pub fn is_supported_product(id_product: u16) -> bool {
    supported_products().iter().any(|(id, _)| *id == id_product)
}

pub const DEV_VENDOR: u16 = 0x0922;

pub const PRINTER_INTERFACE_CLASS: u8 = 0x07;
pub const HID_INTERFACE_CLASS: u8 = 0x03;

/// Escape character preceeding all commands
pub const ESC: u8 = 0x1B;

/// Synchronization character preceding uncompressed print data
pub const SYN: u8 = 0x16;

pub const DEFAULT_MARGIN_PX: f64 = 56.0;

pub const DPI: f64 = 180.0;
pub const MM_PER_INCH: f64 = 25.4;
/// `DPI / MM_PER_INCH`
pub const PIXELS_PER_MM: f64 = DPI / MM_PER_INCH;

/// `Direction` (`constants.py:102`): how a label is justified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Center,
    Right,
}
