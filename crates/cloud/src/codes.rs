//! QR codes and barcodes for labels, as their modules: `GET /api/codes?kind=qr&data=...`.
//! The page draws them on whole dots of the label (a code drawn between dots blurs and does
//! not scan), so it gets the pattern, not a picture.
//!
//! - `qr`: a QR code at the lowest error correction (L), so it stays small enough for a
//!   12 mm tape: `{"size": 21, "rows": ["1111111010...", ...]}`, `1` dark.
//! - `code128`: `{"bars": "11010010000..."}`, `1` a dark module; set C for an even number of
//!   digits (half as long), else set B (printable ASCII).
//! - `ean13`: 12 or 13 digits (the check digit is computed when left out), as `bars`.
//!
//! By the `qrcode` crate and `barcoders` (both MIT OR Apache-2.0).

use crate::Shared;
use axum::{
    Json, Router,
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<Shared> {
    Router::new().route("/api/codes", get(code))
}

#[derive(Deserialize)]
struct CodeQuery {
    kind: String,
    data: String,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Code {
    /// A square of modules, row by row from the top.
    Matrix { size: usize, rows: Vec<String> },
    /// Bars from left to right, one character a module.
    Bars { bars: String },
}

pub fn make(kind: &str, data: &str) -> Result<Code, String> {
    if data.is_empty() {
        return Err("nothing to encode".to_owned());
    }
    if data.len() > 400 {
        return Err("too long for a label".to_owned());
    }
    let bars = |modules: Vec<u8>| Code::Bars {
        bars: modules
            .iter()
            .map(|&m| if m == 1 { '1' } else { '0' })
            .collect(),
    };
    match kind {
        "qr" => {
            let code =
                qrcode::QrCode::with_error_correction_level(data.as_bytes(), qrcode::EcLevel::L)
                    .map_err(|e| format!("no QR code: {e}"))?;
            let size = code.width();
            let colors = code.to_colors();
            let rows = colors
                .chunks(size)
                .map(|row| {
                    row.iter()
                        .map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' })
                        .collect()
                })
                .collect();
            Ok(Code::Matrix { size, rows })
        }
        "code128" => {
            let digits = data.bytes().all(|b| b.is_ascii_digit());
            let printable = data.bytes().all(|b| (0x20..0x7f).contains(&b));
            let set = if digits && data.len().is_multiple_of(2) {
                'Ć'
            } else if printable {
                'Ɓ'
            } else {
                return Err("Code 128 takes letters, digits and signs (ASCII)".to_owned());
            };
            let code = barcoders::sym::code128::Code128::new(format!("{set}{data}"))
                .map_err(|e| format!("no Code 128: {e}"))?;
            Ok(bars(code.encode()))
        }
        "ean13" => {
            if !(data.len() == 12 || data.len() == 13) || !data.bytes().all(|b| b.is_ascii_digit())
            {
                return Err("EAN-13 takes 12 or 13 digits".to_owned());
            }
            // barcoders computes the check digit from the first 12.
            let code = barcoders::sym::ean13::EAN13::new(&data[..12])
                .map_err(|e| format!("no EAN-13: {e}"))?;
            Ok(bars(code.encode()))
        }
        _ => Err("kind: qr, code128 or ean13".to_owned()),
    }
}

async fn code(Query(query): Query<CodeQuery>) -> Response {
    match make(&query.kind, &query.data) {
        Ok(code) => Json(code).into_response(),
        Err(why) => (StatusCode::BAD_REQUEST, why).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_come_as_modules() {
        let Code::Matrix { size, rows } = make("qr", "https://example.org/j/42").unwrap() else {
            panic!("a QR code is a matrix");
        };
        assert_eq!((size, rows.len()), (rows[0].len(), size));
        // A finder pattern in the top left corner: seven dark modules.
        assert!(rows[0].starts_with("1111111"));
        assert!(
            size <= 29,
            "small enough for 12 mm tape at two dots a module"
        );

        let Code::Bars { bars } = make("code128", "MC-42").unwrap() else {
            panic!("Code 128 is bars");
        };
        // Start B, and the stop pattern at the end.
        assert!(bars.starts_with("11010010000"));
        assert!(bars.ends_with("1100011101011"));
        // Digits in pairs (set C) are shorter than the same as letters would be.
        let Code::Bars { bars: digits } = make("code128", "12345678").unwrap() else {
            panic!()
        };
        assert!(digits.len() < bars.len() + 11);
        assert!(make("code128", "naïve").is_err());

        let Code::Bars { bars: ean } = make("ean13", "871234567890").unwrap() else {
            panic!()
        };
        assert_eq!(ean.len(), 95);
        assert!(make("ean13", "12345").is_err());
        assert!(make("pdf417", "x").is_err());
    }
}
