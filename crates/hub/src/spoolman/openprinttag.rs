//! A scanned tag's payload, decoded as Spoolman decodes it.
//!
//! **The one reference:** Spoolman (github.com/Donkie/Spoolman, MIT) at `eccb05e`, version 0.27.1:
//! `spoolman/tags/decode.py` (`decode`, `decode_one_cbor_item` with its walk that refuses CBOR
//! semantic tags, `DecodedTag`) and `spoolman/tags/openprinttag.py` (`decode_nfcv_memory` and its
//! field casters), ported function by function. Spoolman hands the bytes the walk covered to cbor2
//! (5.9.0, its C decoder, `max_depth=8`); `Cbor` below reads them as that decoder does, and the values
//! are looked up with Python's rules, because Spoolman's code does (`dict.get`, `isinstance(x, int)`
//! takes a boolean, a key `0.0` or `False` is the key `0`, a later duplicate key replaces the value).
//!
//! Proof: `scripts/spoolman-openprinttag-corpus.py` runs Spoolman's own `decode` on a corpus of
//! tags and writes `openprinttag-fixture.json`; the test below decodes every case and compares
//! every field (`docs/audits/spoolman.md`).
//!
//! MIT License, Copyright (c) 2023 Daniel Hultgren (Spoolman), for the ported code.

use serde::Serialize;
use sha1::{Digest, Sha1};

/// The tag formats this decodes, as `normalize_format` spells them.
const OPENPRINTTAG: &str = "openprinttag";

/// What Spoolman's `DecodedTag` holds, in its field order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Decoded {
    pub material_class: Option<String>,
    pub material: Option<String>,
    pub name: Option<String>,
    pub vendor_name: Option<String>,
    pub article_number: Option<String>,
    pub color_hex: Option<String>,
    pub diameter: Option<f64>,
    pub density: Option<f64>,
    pub weight: Option<f64>,
    pub initial_weight: Option<f64>,
    pub spool_weight: Option<f64>,
    pub used_weight: Option<f64>,
    pub extruder_temp: Option<i64>,
    pub bed_temp: Option<i64>,
    pub instance_uuid: Option<String>,
}

/// `APPROXIMATE_DENSITY_BY_MATERIAL` (g/cm³), for a filament made from a tag without a density.
const APPROXIMATE_DENSITY_BY_MATERIAL: [(&str, f64); 13] = [
    ("PLA", 1.24),
    ("PETG", 1.27),
    ("ABS", 1.04),
    ("ASA", 1.07),
    ("TPU", 1.21),
    ("PA6", 1.14),
    ("PA11", 1.03),
    ("PA12", 1.01),
    ("PA66", 1.15),
    ("PC", 1.20),
    ("PCTG", 1.23),
    ("HIPS", 1.04),
    ("PVA", 1.23),
];
const FALLBACK_DENSITY: f64 = 1.24;

/// `density_or_fallback`: the material's approximate density, or PLA's.
pub fn density_or_fallback(material: Option<&str>) -> f64 {
    material
        .map(str::to_uppercase)
        .and_then(|m| {
            APPROXIMATE_DENSITY_BY_MATERIAL
                .iter()
                .find(|(k, _)| *k == m)
                .map(|(_, d)| *d)
        })
        .unwrap_or(FALLBACK_DENSITY)
}

/// `normalize_format`: trimmed and lower case; empty is none.
pub fn normalize_format(format: Option<&str>) -> Option<String> {
    let f = format?.trim().to_lowercase();
    (!f.is_empty()).then_some(f)
}

/// `decode.decode`: the fields of a payload in a format Spoolman reads, or `None` (an unknown
/// format, or a payload it could not decode; never an error).
pub fn decode(format: Option<&str>, payload: &[u8], uid: Option<&[u8]>) -> Option<Decoded> {
    if normalize_format(format).as_deref() != Some(OPENPRINTTAG) {
        return None;
    }
    let data = decode_nfcv_memory(payload, uid).ok()?;
    // Print and bed temperatures come as a range: the upper end (Python's `max or min`).
    let or = |max: Option<i64>, min: Option<i64>| match max {
        Some(m) if m != 0 => Some(m),
        _ => min,
    };
    let finite = |v: Option<f64>| v.filter(|v| v.is_finite());
    Some(Decoded {
        material_class: data.material_class,
        material: data.material_type,
        name: data.material_name,
        vendor_name: data.brand_name,
        article_number: data.gtin.map(|g| g.to_string()),
        color_hex: data.primary_color_hex,
        diameter: finite(Some(data.filament_diameter.unwrap_or(1.75))),
        density: finite(data.density),
        weight: finite(data.nominal_netto_full_weight),
        initial_weight: finite(data.actual_netto_full_weight),
        spool_weight: finite(data.empty_container_weight),
        used_weight: finite(data.consumed_weight),
        extruder_temp: or(data.max_print_temperature, data.min_print_temperature),
        bed_temp: or(data.max_bed_temperature, data.min_bed_temperature),
        instance_uuid: match (data.instance_uuid, data.nfc_tag_uid) {
            (Some(u), _) if !u.is_empty() => Some(u),
            (_, Some(uid)) if !uid.is_empty() => Some(uuid_text(&uuid5(&UUID_NS_INSTANCE, &uid))),
            _ => None,
        },
    })
}

/// A decode that did not work; Spoolman's ValueError, TypeError and CborRejectedError alike end in
/// "not decodable".
#[derive(Debug)]
struct Refused;

type R<T> = Result<T, Refused>;

// -- decode.py: the walk over CBOR heads that refuses semantic tags ---------------------------------

const MAJOR_UNSIGNED_INT: u8 = 0;
const MAJOR_NEGATIVE_INT: u8 = 1;
const MAJOR_BYTE_STRING: u8 = 2;
const MAJOR_TEXT_STRING: u8 = 3;
const MAJOR_ARRAY: u8 = 4;
const MAJOR_MAP: u8 = 5;
const MAJOR_TAG: u8 = 6;
const MAJOR_FLOAT_SIMPLE: u8 = 7;
const INDEFINITE_LENGTH: u8 = 31;
const BREAK_BYTE: u8 = 0xFF;
const MAX_SCAN_DEPTH: usize = 64;

fn argument_size(info: u8) -> Option<usize> {
    match info {
        24 => Some(1),
        25 => Some(2),
        26 => Some(4),
        27 => Some(8),
        _ => None,
    }
}

/// `decode_one_cbor_item`: the first CBOR item, refused when it holds a semantic tag.
fn decode_one_cbor_item(raw: &[u8], max_depth: usize) -> R<(Value, usize)> {
    let end = reject_semantic_tags(raw, 0, 0)?;
    let mut cbor = Cbor {
        buf: &raw[..end],
        pos: 0,
        depth: 0,
        max_depth,
    };
    let value = cbor.decode(false)?;
    Ok((value, cbor.pos))
}

fn reject_semantic_tags(buf: &[u8], pos: usize, depth: usize) -> R<usize> {
    if depth > MAX_SCAN_DEPTH || pos >= buf.len() {
        return Err(Refused);
    }
    let head = buf[pos];
    let major = head >> 5;
    let info = head & 0x1F;
    let pos = pos + 1;
    if major == MAJOR_TAG {
        return Err(Refused);
    }
    if major == MAJOR_FLOAT_SIMPLE {
        return skip_simple_argument(buf, pos, info);
    }
    if info == INDEFINITE_LENGTH {
        if !matches!(
            major,
            MAJOR_BYTE_STRING | MAJOR_TEXT_STRING | MAJOR_ARRAY | MAJOR_MAP
        ) {
            return Err(Refused);
        }
        return reject_in_indefinite_container(buf, pos, major, depth);
    }
    let (length, pos) = read_argument(buf, pos, info)?;
    skip_definite_item(buf, pos, major, length, depth)
}

fn skip_definite_item(buf: &[u8], pos: usize, major: u8, length: u64, depth: usize) -> R<usize> {
    match major {
        MAJOR_UNSIGNED_INT | MAJOR_NEGATIVE_INT => Ok(pos),
        MAJOR_BYTE_STRING | MAJOR_TEXT_STRING => {
            let end = (pos as u64).checked_add(length).ok_or(Refused)?;
            if end > buf.len() as u64 {
                return Err(Refused);
            }
            Ok(end as usize)
        }
        MAJOR_ARRAY => {
            let mut pos = pos;
            for _ in 0..length {
                pos = reject_semantic_tags(buf, pos, depth + 1)?;
            }
            Ok(pos)
        }
        _ => {
            let mut pos = pos;
            for _ in 0..length.saturating_mul(2) {
                pos = reject_semantic_tags(buf, pos, depth + 1)?;
            }
            Ok(pos)
        }
    }
}

fn read_argument(buf: &[u8], pos: usize, info: u8) -> R<(u64, usize)> {
    if info < 24 {
        return Ok((u64::from(info), pos));
    }
    let size = argument_size(info).ok_or(Refused)?;
    if pos + size > buf.len() {
        return Err(Refused);
    }
    let value = buf[pos..pos + size]
        .iter()
        .fold(0u64, |v, b| (v << 8) | u64::from(*b));
    Ok((value, pos + size))
}

fn skip_simple_argument(buf: &[u8], pos: usize, info: u8) -> R<usize> {
    if info == INDEFINITE_LENGTH {
        return Err(Refused);
    }
    if info < 24 {
        return Ok(pos);
    }
    let size = argument_size(info).ok_or(Refused)?;
    if pos + size > buf.len() {
        return Err(Refused);
    }
    Ok(pos + size)
}

fn reject_in_indefinite_container(buf: &[u8], pos: usize, major: u8, depth: usize) -> R<usize> {
    let mut pos = pos;
    loop {
        if pos >= buf.len() {
            return Err(Refused);
        }
        if buf[pos] == BREAK_BYTE {
            return Ok(pos + 1);
        }
        if major == MAJOR_MAP {
            pos = reject_semantic_tags(buf, pos, depth + 1)?;
            pos = reject_semantic_tags(buf, pos, depth + 1)?;
        } else if major == MAJOR_ARRAY {
            pos = reject_semantic_tags(buf, pos, depth + 1)?;
        } else {
            let chunk_head = buf[pos];
            if chunk_head >> 5 != major || chunk_head & 0x1F == INDEFINITE_LENGTH {
                return Err(Refused);
            }
            pos = reject_semantic_tags(buf, pos, depth + 1)?;
        }
    }
}

// -- cbor2's decoder, for the items the walk lets through ------------------------------------------

/// A decoded CBOR item as the Python object cbor2 makes of it.
#[derive(Debug, Clone)]
enum Value {
    Int(i128),
    Float(f64),
    Bool(bool),
    Null,
    Undefined,
    Simple(u8),
    Bytes(Vec<u8>),
    Text(String),
    /// A list, or a tuple in a key.
    Array(Vec<Value>),
    /// A dict (a FrozenDict in a key): Python's dict, keys unique by `==`, in insertion order.
    Map(Vec<(Value, Value)>),
    /// cbor2's break marker, only inside an indefinite container.
    Break,
}

impl Value {
    /// Python's `==` between two hashable values, as a dict compares its keys: `0 == 0.0 ==
    /// False`, NaN is equal to nothing (each decoded NaN is its own object), a CBOR simple value
    /// is its own key.
    fn key_eq(&self, other: &Value) -> bool {
        use Value::{Array, Bytes, Map, Null, Simple, Text, Undefined};
        match (self.number(), other.number()) {
            (Some(a), Some(b)) => return a.eq(&b),
            (Some(_), None) | (None, Some(_)) => return false,
            _ => {}
        }
        match (self, other) {
            (Null, Null) | (Undefined, Undefined) => true,
            (Simple(a), Simple(b)) => a == b,
            (Bytes(a), Bytes(b)) => a == b,
            (Text(a), Text(b)) => a == b,
            (Array(a), Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.key_eq(y)),
            (Map(a), Map(b)) => {
                a.len() == b.len()
                    && a.iter().all(|(k, v)| {
                        b.iter()
                            .find(|(k2, _)| k.key_eq(k2))
                            .is_some_and(|(_, v2)| v.key_eq(v2))
                    })
            }
            _ => false,
        }
    }

    fn number(&self) -> Option<Num> {
        match self {
            Value::Int(i) => Some(Num::Int(*i)),
            Value::Bool(b) => Some(Num::Int(i128::from(*b))),
            Value::Float(f) => Some(Num::Float(*f)),
            _ => None,
        }
    }

    /// Whether Python can hash it outside a key (a list and a dict cannot).
    fn hashable(&self) -> bool {
        !matches!(self, Value::Array(_) | Value::Map(_))
    }

    /// `isinstance(v, int)`: an integer or a boolean.
    fn py_int(&self) -> Option<i128> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Bool(b) => Some(i128::from(*b)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum Num {
    Int(i128),
    Float(f64),
}

impl Num {
    /// Python's exact comparison of an int and a float.
    fn eq(&self, other: &Num) -> bool {
        match (*self, *other) {
            (Num::Int(a), Num::Int(b)) => a == b,
            (Num::Float(a), Num::Float(b)) => a == b,
            (Num::Int(i), Num::Float(f)) | (Num::Float(f), Num::Int(i)) => {
                f.is_finite() && f.fract() == 0.0 && f.abs() < 1.8e38 && f as i128 == i
            }
        }
    }
}

/// A dict lookup: the value under the key equal to `key`.
fn get<'a>(map: &'a [(Value, Value)], key: &Value) -> Option<&'a Value> {
    map.iter().find(|(k, _)| k.key_eq(key)).map(|(_, v)| v)
}

struct Cbor<'a> {
    buf: &'a [u8],
    pos: usize,
    depth: usize,
    max_depth: usize,
}

impl Cbor<'_> {
    fn read(&mut self, n: u64) -> R<&[u8]> {
        let end = (self.pos as u64).checked_add(n).ok_or(Refused)?;
        if end > self.buf.len() as u64 {
            return Err(Refused);
        }
        let start = self.pos;
        self.pos = end as usize;
        Ok(&self.buf[start..self.pos])
    }

    fn length(&mut self, info: u8) -> R<Option<u64>> {
        Ok(Some(match info {
            0..=23 => u64::from(info),
            24..=27 => {
                let size = argument_size(info).unwrap_or(1) as u64;
                self.read(size)?
                    .iter()
                    .fold(0u64, |v, b| (v << 8) | u64::from(*b))
            }
            31 => return Ok(None),
            _ => return Err(Refused),
        }))
    }

    /// One item; the item at nesting level `max_depth` is refused (`maximum container nesting
    /// depth exceeded`), and a break marker counts as an item.
    fn decode(&mut self, key: bool) -> R<Value> {
        if self.depth >= self.max_depth {
            return Err(Refused);
        }
        self.depth += 1;
        let value = self.item(key);
        self.depth -= 1;
        value
    }

    fn item(&mut self, key: bool) -> R<Value> {
        let head = self.read(1)?[0];
        let major = head >> 5;
        let info = head & 0x1F;
        match major {
            MAJOR_UNSIGNED_INT => Ok(Value::Int(i128::from(self.length(info)?.ok_or(Refused)?))),
            MAJOR_NEGATIVE_INT => Ok(Value::Int(
                -1 - i128::from(self.length(info)?.ok_or(Refused)?),
            )),
            MAJOR_BYTE_STRING => Ok(Value::Bytes(self.string(info, MAJOR_BYTE_STRING)?)),
            MAJOR_TEXT_STRING => self.text(info).map(Value::Text),
            MAJOR_ARRAY => {
                let mut items = Vec::new();
                match self.length(info)? {
                    None => loop {
                        match self.decode(key)? {
                            Value::Break => break,
                            v => items.push(v),
                        }
                    },
                    Some(n) => {
                        for _ in 0..n {
                            items.push(self.decode(key)?);
                        }
                    }
                }
                Ok(Value::Array(items))
            }
            MAJOR_MAP => {
                let mut map: Vec<(Value, Value)> = Vec::new();
                let mut put =
                    |k: Value, v: Value| match map.iter_mut().find(|(k2, _)| k2.key_eq(&k)) {
                        Some(entry) => entry.1 = v,
                        None => map.push((k, v)),
                    };
                match self.length(info)? {
                    None => loop {
                        let k = self.decode(true)?;
                        if matches!(k, Value::Break) {
                            break;
                        }
                        let v = self.decode(key)?;
                        put(k, v);
                    },
                    Some(n) => {
                        for _ in 0..n {
                            let k = self.decode(true)?;
                            let v = self.decode(key)?;
                            put(k, v);
                        }
                    }
                }
                Ok(Value::Map(map))
            }
            MAJOR_TAG => Err(Refused),
            _ => Ok(match info {
                20 => Value::Bool(false),
                21 => Value::Bool(true),
                22 => Value::Null,
                23 => Value::Undefined,
                24 => Value::Simple(self.read(1)?[0]),
                25 => {
                    let b = self.read(2)?;
                    Value::Float(half(u16::from_be_bytes([b[0], b[1]])))
                }
                26 => {
                    let b = self.read(4)?;
                    Value::Float(f64::from(f32::from_be_bytes([b[0], b[1], b[2], b[3]])))
                }
                27 => {
                    let b = self.read(8)?;
                    Value::Float(f64::from_be_bytes(b.try_into().map_err(|_| Refused)?))
                }
                31 => Value::Break,
                0..=19 => Value::Simple(info),
                _ => return Err(Refused),
            }),
        }
    }

    fn string(&mut self, info: u8, major: u8) -> R<Vec<u8>> {
        match self.length(info)? {
            Some(n) => Ok(self.read(n)?.to_vec()),
            None => {
                let mut out = Vec::new();
                loop {
                    let head = self.read(1)?[0];
                    if head == 0xFF {
                        return Ok(out);
                    }
                    if head >> 5 != major {
                        return Err(Refused);
                    }
                    let n = self.length(head & 0x1F)?.ok_or(Refused)?;
                    let chunk = self.read(n)?;
                    if major == MAJOR_TEXT_STRING {
                        std::str::from_utf8(chunk).map_err(|_| Refused)?;
                    }
                    out.extend_from_slice(chunk);
                }
            }
        }
    }

    fn text(&mut self, info: u8) -> R<String> {
        let bytes = self.string(info, MAJOR_TEXT_STRING)?;
        String::from_utf8(bytes).map_err(|_| Refused)
    }
}

/// An IEEE 754 half-precision float, as `struct.unpack(">e")` reads it.
fn half(bits: u16) -> f64 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = (bits >> 10) & 0x1F;
    let frac = f64::from(bits & 0x3FF);
    sign * match exp {
        0 => frac * 2f64.powi(-24),
        31 if frac == 0.0 => f64::INFINITY,
        31 => f64::NAN,
        e => (1.0 + frac / 1024.0) * 2f64.powi(i32::from(e) - 15),
    }
}

// -- openprinttag.py --------------------------------------------------------------------------------

const MATERIAL_TYPES: [&str; 43] = [
    "PLA", "PETG", "TPU", "ABS", "ASA", "PC", "PCTG", "PP", "PA6", "PA11", "PA12", "PA66", "CPE",
    "TPE", "HIPS", "PHA", "PET", "PEI", "PBT", "PVB", "PVA", "PEKK", "PEEK", "BVOH", "TPC", "PPS",
    "PPSU", "PVC", "PEBA", "PVDF", "PPA", "PCL", "PES", "PMMA", "POM", "PPE", "PS", "PSU", "TPI",
    "SBS", "OBC", "EVA", "PA612",
];
const MATERIAL_CLASSES: [&str; 2] = ["FFF", "SLA"];

/// `UUID_NS_INSTANCE`: 31062f81-b5bd-4f86-a5f8-46367e841508.
const UUID_NS_INSTANCE: [u8; 16] = [
    0x31, 0x06, 0x2f, 0x81, 0xb5, 0xbd, 0x4f, 0x86, 0xa5, 0xf8, 0x46, 0x36, 0x7e, 0x84, 0x15, 0x08,
];
const MAX_CBOR_DEPTH: usize = 8;

const META_MAIN_REGION_OFFSET: i128 = 0;
const META_AUX_REGION_OFFSET: i128 = 2;
const MF_INSTANCE_UUID: i128 = 0;
const MF_GTIN: i128 = 4;
const MF_MATERIAL_CLASS: i128 = 8;
const MF_MATERIAL_TYPE: i128 = 9;
const MF_MATERIAL_NAME: i128 = 10;
const MF_BRAND_NAME: i128 = 11;
const MF_NOMINAL_NETTO_FULL_WEIGHT: i128 = 16;
const MF_ACTUAL_NETTO_FULL_WEIGHT: i128 = 17;
const MF_EMPTY_CONTAINER_WEIGHT: i128 = 18;
const MF_PRIMARY_COLOR: i128 = 19;
const MF_DENSITY: i128 = 29;
const MF_FILAMENT_DIAMETER: i128 = 30;
const MF_MIN_PRINT_TEMPERATURE: i128 = 34;
const MF_MAX_PRINT_TEMPERATURE: i128 = 35;
const MF_MIN_BED_TEMPERATURE: i128 = 37;
const MF_MAX_BED_TEMPERATURE: i128 = 38;
const AUX_CONSUMED_WEIGHT: i128 = 0;

const TLV_NDEF_MESSAGE: u8 = 0x03;
const TLV_TERMINATOR: u8 = 0xFE;
const TLV_THREE_BYTE_LENGTH: u8 = 0xFF;
const NDEF_TNF_MEDIA_TYPE: u8 = 0x02;
const OPENPRINTTAG_MIME: &[u8] = b"application/vnd.openprinttag";
const MAX_TEXT_LENGTH: usize = 1024;

/// `OpenPrintTagData`: the fields Spoolman reads.
#[derive(Default)]
struct Data {
    instance_uuid: Option<String>,
    gtin: Option<i128>,
    material_class: Option<String>,
    material_type: Option<String>,
    material_name: Option<String>,
    brand_name: Option<String>,
    density: Option<f64>,
    filament_diameter: Option<f64>,
    nominal_netto_full_weight: Option<f64>,
    actual_netto_full_weight: Option<f64>,
    empty_container_weight: Option<f64>,
    consumed_weight: Option<f64>,
    primary_color_hex: Option<String>,
    min_print_temperature: Option<i64>,
    max_print_temperature: Option<i64>,
    min_bed_temperature: Option<i64>,
    max_bed_temperature: Option<i64>,
    nfc_tag_uid: Option<Vec<u8>>,
}

/// `uuid5_bytes`: a version 5 UUID over a byte name.
fn uuid5(namespace: &[u8; 16], name: &[u8]) -> [u8; 16] {
    let mut sha = Sha1::new();
    sha.update(namespace);
    sha.update(name);
    let digest = sha.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&digest[..16]);
    b[6] = (b[6] & 0x0F) | 0x50;
    b[8] = (b[8] & 0x3F) | 0x80;
    b
}

fn uuid_text(b: &[u8; 16]) -> String {
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn parse_color_rgba(v: &Value) -> Option<String> {
    match v {
        Value::Bytes(b) if b.len() >= 3 => Some(format!("{:02x}{:02x}{:02x}", b[0], b[1], b[2])),
        _ => None,
    }
}

fn parse_uuid(v: &Value) -> Option<String> {
    match v {
        Value::Bytes(b) if b.len() == 16 => b.as_slice().try_into().ok().map(uuid_text),
        _ => None,
    }
}

fn decode_cbor_map(raw: &[u8]) -> R<(Vec<(Value, Value)>, usize)> {
    match decode_one_cbor_item(raw, MAX_CBOR_DEPTH)? {
        (Value::Map(m), consumed) => Ok((m, consumed)),
        _ => Err(Refused),
    }
}

fn find_ndef_payload(raw: &[u8]) -> Option<Vec<u8>> {
    if raw.len() < 4 || raw[0] != 0xE1 {
        return None;
    }
    let mut pos = 4;
    while pos < raw.len() {
        let tag = raw[pos];
        pos += 1;
        if tag == TLV_TERMINATOR {
            break;
        }
        if tag == 0x00 {
            continue;
        }
        if pos >= raw.len() {
            break;
        }
        let mut tlv_len = usize::from(raw[pos]);
        pos += 1;
        if tlv_len == usize::from(TLV_THREE_BYTE_LENGTH) {
            if pos + 2 > raw.len() {
                break;
            }
            tlv_len = (usize::from(raw[pos]) << 8) | usize::from(raw[pos + 1]);
            pos += 2;
        }
        if tag == TLV_NDEF_MESSAGE {
            return parse_ndef_records(slice(raw, pos, pos + tlv_len));
        }
        pos += tlv_len;
    }
    None
}

/// Python's `b[start:end]`: cut to what is there.
fn slice(b: &[u8], start: usize, end: usize) -> &[u8] {
    let end = end.min(b.len());
    &b[start.min(end)..end]
}

fn read_ndef_length(ndef: &[u8], pos: usize, short_record: bool) -> Option<(usize, usize)> {
    if short_record {
        if pos >= ndef.len() {
            return None;
        }
        return Some((usize::from(ndef[pos]), pos + 1));
    }
    if pos + 4 > ndef.len() {
        return None;
    }
    let n = u32::from_be_bytes([ndef[pos], ndef[pos + 1], ndef[pos + 2], ndef[pos + 3]]);
    Some((n as usize, pos + 4))
}

fn parse_ndef_records(ndef: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 0;
    while pos < ndef.len() {
        let header = ndef[pos];
        pos += 1;
        let message_end = header & 0x40 != 0;
        let short_record = header & 0x10 != 0;
        let id_present = header & 0x08 != 0;
        let tnf = header & 0x07;
        if pos >= ndef.len() {
            break;
        }
        let type_length = usize::from(ndef[pos]);
        pos += 1;
        let (payload_length, p) = read_ndef_length(ndef, pos, short_record)?;
        pos = p;
        let mut id_length = 0;
        if id_present {
            if pos >= ndef.len() {
                break;
            }
            id_length = usize::from(ndef[pos]);
            pos += 1;
        }
        let record_type = slice(ndef, pos, pos + type_length);
        pos += type_length;
        pos += id_length;
        let payload = slice(ndef, pos, pos.saturating_add(payload_length));
        pos = pos.saturating_add(payload_length);
        if tnf == NDEF_TNF_MEDIA_TYPE && record_type == OPENPRINTTAG_MIME {
            return Some(payload.to_vec());
        }
        if message_end {
            break;
        }
    }
    None
}

/// `_spec_order_uid`: an 8-byte NFC-V UID that ends in E0 is turned around.
fn spec_order_uid(uid: Option<&[u8]>) -> Option<Vec<u8>> {
    let uid = uid?;
    if uid.len() == 8 && uid[7] == 0xE0 && uid[0] != 0xE0 {
        return Some(uid.iter().rev().copied().collect());
    }
    Some(uid.to_vec())
}

fn decode_nfcv_memory(raw: &[u8], uid: Option<&[u8]>) -> R<Data> {
    let payload = find_ndef_payload(raw).ok_or(Refused)?;
    let mut data = Data {
        nfc_tag_uid: spec_order_uid(uid),
        ..Data::default()
    };
    let (meta, meta_size) = decode_cbor_map(&payload)?;
    let main_offset = match get(&meta, &Value::Int(META_MAIN_REGION_OFFSET)) {
        Some(v) => v.py_int().ok_or(Refused)?,
        None => meta_size as i128,
    };
    if main_offset < 0 {
        return Err(Refused);
    }
    let aux_offset = get(&meta, &Value::Int(META_AUX_REGION_OFFSET))
        .and_then(Value::py_int)
        .filter(|a| *a >= 0);
    if main_offset < payload.len() as i128 {
        let (main, _) = decode_cbor_map(&payload[main_offset as usize..])?;
        populate_main_fields(&mut data, &main)?;
    }
    if let Some(aux_offset) = aux_offset
        && aux_offset < payload.len() as i128
        && let Ok((aux, _)) = decode_cbor_map(&payload[aux_offset as usize..])
        && let Some(v) = get(&aux, &Value::Int(AUX_CONSUMED_WEIGHT))
    {
        data.consumed_weight = number(v).ok();
    }
    Ok(data)
}

/// `_number`: an integer up to 2**53 or a finite float; a boolean is refused.
fn number(v: &Value) -> R<f64> {
    match v {
        Value::Int(i) if i.unsigned_abs() <= 1 << 53 => Ok(*i as f64),
        Value::Float(f) if f.is_finite() => Ok(*f),
        _ => Err(Refused),
    }
}

/// `_text`: a text string of at most 1024 code points.
fn text(v: &Value) -> R<String> {
    match v {
        Value::Text(t) if t.chars().count() <= MAX_TEXT_LENGTH => Ok(t.clone()),
        _ => Err(Refused),
    }
}

/// `_int_between`: an integer (not a boolean) in range.
fn int_between(v: &Value, low: i128, high: i128) -> R<i128> {
    match v {
        Value::Int(i) if (low..=high).contains(i) => Ok(*i),
        _ => Err(Refused),
    }
}

fn int32(v: &Value) -> R<i64> {
    int_between(v, i128::from(i32::MIN), i128::from(i32::MAX)).map(|i| i as i64)
}

/// `MAP.get(value)`: the name under an integer key equal to `value`; a list or a dict cannot be
/// looked up (TypeError).
fn lookup(names: &[&str], v: &Value) -> R<Option<String>> {
    if !v.hashable() {
        return Err(Refused);
    }
    Ok(names
        .iter()
        .enumerate()
        .find(|(i, _)| Value::Int(*i as i128).key_eq(v))
        .map(|(_, n)| (*n).to_owned()))
}

fn populate_main_fields(data: &mut Data, main: &[(Value, Value)]) -> R<()> {
    let at = |key: i128| get(main, &Value::Int(key));
    // _SIMPLE_MAIN_FIELDS, in its order.
    if let Some(v) = at(MF_GTIN) {
        data.gtin = Some(int_between(v, 0, i128::from(i64::MAX))?);
    }
    if let Some(v) = at(MF_MATERIAL_NAME) {
        data.material_name = Some(text(v)?);
    }
    if let Some(v) = at(MF_BRAND_NAME) {
        data.brand_name = Some(text(v)?);
    }
    for (key, field) in [
        (MF_DENSITY, &mut data.density),
        (MF_FILAMENT_DIAMETER, &mut data.filament_diameter),
        (
            MF_NOMINAL_NETTO_FULL_WEIGHT,
            &mut data.nominal_netto_full_weight,
        ),
        (
            MF_ACTUAL_NETTO_FULL_WEIGHT,
            &mut data.actual_netto_full_weight,
        ),
        (MF_EMPTY_CONTAINER_WEIGHT, &mut data.empty_container_weight),
    ] {
        if let Some(v) = at(key) {
            *field = Some(number(v)?);
        }
    }
    for (key, field) in [
        (MF_MIN_PRINT_TEMPERATURE, &mut data.min_print_temperature),
        (MF_MAX_PRINT_TEMPERATURE, &mut data.max_print_temperature),
        (MF_MIN_BED_TEMPERATURE, &mut data.min_bed_temperature),
        (MF_MAX_BED_TEMPERATURE, &mut data.max_bed_temperature),
    ] {
        if let Some(v) = at(key) {
            *field = Some(int32(v)?);
        }
    }
    if let Some(v) = at(MF_INSTANCE_UUID) {
        data.instance_uuid = parse_uuid(v);
    }
    if let Some(v) = at(MF_MATERIAL_CLASS) {
        data.material_class = lookup(&MATERIAL_CLASSES, v)?;
    }
    if let Some(v) = at(MF_MATERIAL_TYPE) {
        data.material_type = lookup(&MATERIAL_TYPES, v)?;
    }
    if let Some(v) = at(MF_PRIMARY_COLOR) {
        data.primary_color_hex = parse_color_rgba(v);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as Json;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// The corpus script's `nfcv`: a payload in a short (or, from 256 bytes, a standard) NDEF
    /// record of the OpenPrintTag type, in NFC-V memory.
    fn nfcv(payload: &[u8]) -> Vec<u8> {
        let mime = OPENPRINTTAG_MIME;
        let record: Vec<u8> = if payload.len() < 256 {
            [
                &[0b1101_0010, mime.len() as u8, payload.len() as u8][..],
                mime,
                payload,
            ]
            .concat()
        } else {
            [
                &[0b1100_0010, mime.len() as u8][..],
                &(payload.len() as u32).to_be_bytes(),
                mime,
                payload,
            ]
            .concat()
        };
        if payload.len() < 256 {
            [
                &[0xE1, 0x40, 0x00, 0x01, 0x03, record.len() as u8][..],
                &record,
                &[0xFE],
            ]
            .concat()
        } else {
            [
                &[0xE1, 0x40, 0x00, 0x01, 0x03, 0xFF][..],
                &(record.len() as u16).to_be_bytes(),
                &record,
                &[0xFE],
            ]
            .concat()
        }
    }

    /// Every tag of the corpus decodes to what Spoolman decoded it to.
    #[test]
    fn it_decodes_every_tag_as_spoolman_does() {
        let fixture: Json =
            serde_json::from_str(include_str!("openprinttag-fixture.json")).unwrap();
        let bases: std::collections::HashMap<&str, Vec<u8>> = fixture["bases"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.as_str(), hex(v.as_str().unwrap())))
            .collect();
        let results = fixture["results"].as_array().unwrap();
        let mut differ = Vec::new();
        let cases = fixture["cases"].as_array().unwrap();
        for case in cases {
            let memory = if let Some(base) = case["base"].as_str() {
                let mut m = bases[base].clone();
                if let Some(cut) = case["cut"].as_u64() {
                    m.truncate(cut as usize);
                }
                if let Some(at) = case["at"].as_u64() {
                    m[at as usize] = case["byte"].as_u64().unwrap() as u8;
                }
                m
            } else if let Some(p) = case["payload"].as_str() {
                nfcv(&hex(p))
            } else {
                hex(case["memory"].as_str().unwrap())
            };
            let uid = case["uid"].as_str().map(hex);
            let format = match case.get("format") {
                None => Some(OPENPRINTTAG),
                Some(f) => f.as_str(),
            };
            let ours = serde_json::to_value(decode(format, &memory, uid.as_deref())).unwrap();
            let theirs = &results[case["result"].as_u64().unwrap() as usize];
            if &ours != theirs {
                differ.push(format!("{case}\n  Spoolman {theirs}\n  ours     {ours}"));
            }
        }
        assert!(
            differ.is_empty(),
            "{} of {} tags decode otherwise than in Spoolman:\n{}",
            differ.len(),
            cases.len(),
            differ[..differ.len().min(20)].join("\n")
        );
    }
}
