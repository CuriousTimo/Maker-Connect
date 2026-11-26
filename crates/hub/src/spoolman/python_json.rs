//! Python's `json.loads` of a request body, as Starlette's `Request.json()` calls it under
//! Spoolman: CPython 3.12.13 (`Lib/json/__init__.py` `loads` and `detect_encoding`,
//! `Lib/json/decoder.py` `JSONDecoder.decode`/`raw_decode`, and the C scanner that runs,
//! `Modules/_json.c` `scanstring_unicode`, `_parse_object_unicode`, `_parse_array_unicode`,
//! `_match_number_unicode`, `scan_once_unicode`; PSF License,
//! `reference/cpython-3.12.13-json/LICENSE`). The result is the Python object, [`Py`], that
//! Pydantic then validates; an error is Python's `JSONDecodeError`, its `msg` and `pos` (in
//! characters), which FastAPI answers as `json_invalid`.
//!
//! Not as Python: a string with a lone surrogate (`"\ud800"`) holds U+FFFD, as a Rust string
//! cannot hold a surrogate; `surrogatepass` decoding of the bytes is not ported (such bytes are
//! not UTF-8 here, a decode error).

/// A Python object as `json.loads` makes it.
#[derive(Clone, Debug, PartialEq)]
pub enum Py {
    None,
    Bool(bool),
    /// A Python `int`; one past `i128` keeps its digits.
    Int(i128),
    BigInt(String),
    Float(f64),
    Str(String),
    List(Vec<Py>),
    /// A `dict`: insertion order, a repeated key keeping its first place and its last value.
    Dict(Vec<(String, Py)>),
    /// A body that is not JSON: `bytes` (FastAPI hands Pydantic the raw body).
    Bytes(Vec<u8>),
}

impl Py {
    pub fn get(&self, key: &str) -> Option<&Py> {
        match self {
            Py::Dict(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// `json.JSONDecodeError`: its `msg` and `pos`.
#[derive(Debug, PartialEq)]
pub struct DecodeError {
    pub msg: &'static str,
    pub pos: usize,
}

impl DecodeError {
    /// `str()` of the `JSONDecodeError`: `JSONDecodeError.__init__` (`Lib/json/decoder.py`),
    /// `'%s: line %d column %d (char %d)' % (msg, lineno, colno, pos)` with
    /// `lineno = doc.count('\n', 0, pos) + 1` and `colno = pos - doc.rfind('\n', 0, pos)`.
    pub fn message(&self, doc: &str) -> String {
        let before: Vec<char> = doc.chars().take(self.pos).collect();
        let lineno = before.iter().filter(|c| **c == '\n').count() + 1;
        let colno = match before.iter().rposition(|c| *c == '\n') {
            Some(i) => self.pos - i,
            None => self.pos + 1,
        };
        format!(
            "{}: line {lineno} column {colno} (char {})",
            self.msg, self.pos
        )
    }
}

/// `json.loads(s)` of a `str` (`Lib/json/__init__.py` `loads`): `s.startswith('﻿')` raises
/// `JSONDecodeError("Unexpected UTF-8 BOM (decode using utf-8-sig)", s, 0)`, else
/// `JSONDecoder.decode`.
pub fn loads_str(s: &str) -> Result<Py, DecodeError> {
    if s.starts_with('\u{feff}') {
        return Err(DecodeError {
            msg: "Unexpected UTF-8 BOM (decode using utf-8-sig)",
            pos: 0,
        });
    }
    let chars: Vec<char> = s.chars().collect();
    decode(&chars)
}

/// What `json.loads(body)` raises besides `JSONDecodeError`: a `UnicodeDecodeError` (FastAPI
/// answers 400 "There was an error parsing the body").
#[derive(Debug, PartialEq)]
pub enum LoadError {
    Decode(DecodeError),
    Unicode,
}

/// `json.loads(b)` of bytes: `b.decode(detect_encoding(b), 'surrogatepass')`, then
/// `JSONDecoder.decode`.
pub fn loads(b: &[u8]) -> Result<Py, LoadError> {
    let s = decode_bytes(b).ok_or(LoadError::Unicode)?;
    let chars: Vec<char> = s.chars().collect();
    decode(&chars).map_err(LoadError::Decode)
}

/// `detect_encoding` and the decode.
fn decode_bytes(b: &[u8]) -> Option<String> {
    const BOM_UTF8: &[u8] = b"\xef\xbb\xbf";
    let utf16 = |b: &[u8], le: bool| {
        if !b.len().is_multiple_of(2) {
            return None;
        }
        let units = b.chunks(2).map(|c| {
            if le {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        });
        char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .ok()
    };
    let utf32 = |b: &[u8], le: bool| {
        if !b.len().is_multiple_of(4) {
            return None;
        }
        b.chunks(4)
            .map(|c| {
                let a = [c[0], c[1], c[2], c[3]];
                char::from_u32(if le {
                    u32::from_le_bytes(a)
                } else {
                    u32::from_be_bytes(a)
                })
            })
            .collect::<Option<String>>()
    };
    if b.starts_with(b"\x00\x00\xfe\xff") {
        return utf32(&b[4..], false);
    }
    if b.starts_with(b"\xff\xfe\x00\x00") {
        return utf32(&b[4..], true);
    }
    if b.starts_with(b"\xfe\xff") {
        return utf16(&b[2..], false);
    }
    if b.starts_with(b"\xff\xfe") {
        return utf16(&b[2..], true);
    }
    if let Some(rest) = b.strip_prefix(BOM_UTF8) {
        return String::from_utf8(rest.to_vec()).ok();
    }
    if b.len() >= 4 {
        if b[0] == 0 {
            return if b[1] != 0 {
                utf16(b, false)
            } else {
                utf32(b, false)
            };
        }
        if b[1] == 0 {
            return if b[2] != 0 || b[3] != 0 {
                utf16(b, true)
            } else {
                utf32(b, true)
            };
        }
    } else if b.len() == 2 {
        if b[0] == 0 {
            return utf16(b, false);
        }
        if b[1] == 0 {
            return utf16(b, true);
        }
    }
    String::from_utf8(b.to_vec()).ok()
}

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn skip_ws(s: &[char], mut i: usize) -> usize {
    while i < s.len() && is_ws(s[i]) {
        i += 1;
    }
    i
}

/// A failed scan: `StopIteration(idx)` (no value here) or a `JSONDecodeError`.
enum Fail {
    Stop(usize),
    Error(DecodeError),
}

fn err(msg: &'static str, pos: usize) -> Fail {
    Fail::Error(DecodeError { msg, pos })
}

/// `JSONDecoder.decode`: whitespace, `raw_decode`, whitespace, "Extra data".
fn decode(s: &[char]) -> Result<Py, DecodeError> {
    let idx = skip_ws(s, 0);
    let (obj, end) = match scan_once(s, idx) {
        Ok(v) => v,
        Err(Fail::Stop(pos)) => {
            return Err(DecodeError {
                msg: "Expecting value",
                pos,
            });
        }
        Err(Fail::Error(e)) => return Err(e),
    };
    let end = skip_ws(s, end);
    if end != s.len() {
        return Err(DecodeError {
            msg: "Extra data",
            pos: end,
        });
    }
    Ok(obj)
}

fn at(s: &[char], i: usize, c: char) -> bool {
    s.get(i) == Some(&c)
}

/// `scan_once_unicode`.
fn scan_once(s: &[char], idx: usize) -> Result<(Py, usize), Fail> {
    let length = s.len();
    if idx >= length {
        return Err(Fail::Stop(idx));
    }
    let word = |w: &str| {
        let w: Vec<char> = w.chars().collect();
        idx + w.len() - 1 < length && s[idx..idx + w.len()] == w[..]
    };
    match s[idx] {
        '"' => return scanstring(s, idx + 1).map(|(v, e)| (Py::Str(v), e)),
        '{' => return parse_object(s, idx + 1),
        '[' => return parse_array(s, idx + 1),
        'n' if word("null") => return Ok((Py::None, idx + 4)),
        't' if word("true") => return Ok((Py::Bool(true), idx + 4)),
        'f' if word("false") => return Ok((Py::Bool(false), idx + 5)),
        'N' if word("NaN") => return Ok((Py::Float(f64::NAN), idx + 3)),
        'I' if word("Infinity") => return Ok((Py::Float(f64::INFINITY), idx + 8)),
        '-' if word("-Infinity") => return Ok((Py::Float(f64::NEG_INFINITY), idx + 9)),
        _ => {}
    }
    match_number(s, idx)
}

/// `_match_number_unicode`.
fn match_number(s: &[char], start: usize) -> Result<(Py, usize), Fail> {
    let end_idx = s.len() as isize - 1;
    let digit = |i: usize| s.get(i).is_some_and(|c| c.is_ascii_digit());
    let mut idx = start;
    let mut is_float = false;
    if s[idx] == '-' {
        idx += 1;
        if idx as isize > end_idx {
            return Err(Fail::Stop(start));
        }
    }
    if ('1'..='9').contains(&s[idx]) {
        idx += 1;
        while idx as isize <= end_idx && digit(idx) {
            idx += 1;
        }
    } else if s[idx] == '0' {
        idx += 1;
    } else {
        return Err(Fail::Stop(start));
    }
    if (idx as isize) < end_idx && s[idx] == '.' && digit(idx + 1) {
        is_float = true;
        idx += 2;
        while idx as isize <= end_idx && digit(idx) {
            idx += 1;
        }
    }
    if (idx as isize) < end_idx && (s[idx] == 'e' || s[idx] == 'E') {
        let e_start = idx;
        idx += 1;
        if (idx as isize) < end_idx && (s[idx] == '-' || s[idx] == '+') {
            idx += 1;
        }
        while idx as isize <= end_idx && digit(idx) {
            idx += 1;
        }
        if digit(idx - 1) {
            is_float = true;
        } else {
            idx = e_start;
        }
    }
    let text: String = s[start..idx].iter().collect();
    let value = if is_float {
        // `PyFloat_FromString`: a decimal string as the nearest double.
        Py::Float(text.parse().unwrap_or(f64::NAN))
    } else {
        match text.parse::<i128>() {
            Ok(i) => Py::Int(i),
            Err(_) => Py::BigInt(text),
        }
    };
    Ok((value, idx))
}

fn hex4(s: &[char], from: usize) -> Option<u32> {
    let mut c = 0u32;
    for i in from..from + 4 {
        c = (c << 4) | s.get(i)?.to_digit(16)?;
    }
    Some(c)
}

/// `scanstring_unicode` (strict): `end` is the index after the opening quote.
fn scanstring(s: &[char], mut end: usize) -> Result<(String, usize), Fail> {
    let len = s.len();
    let begin = end - 1;
    let mut out = String::new();
    loop {
        let mut next = end;
        let mut c = '\0';
        while next < len {
            c = s[next];
            if c == '"' || c == '\\' {
                break;
            }
            if c <= '\u{1f}' {
                return Err(err("Invalid control character at", next));
            }
            next += 1;
        }
        // The scan ran to the end: neither a quote nor a backslash.
        if next == len {
            return Err(err("Unterminated string starting at", begin));
        }
        out.extend(&s[end..next]);
        next += 1;
        if c == '"' {
            end = next;
            break;
        }
        if next == len {
            return Err(err("Unterminated string starting at", begin));
        }
        let e = s[next];
        #[allow(clippy::needless_late_init)] // as `scanstring_unicode`'s `c`
        let ch: u32;
        if e != 'u' {
            end = next + 1;
            ch = match e {
                '"' => '"' as u32,
                '\\' => '\\' as u32,
                '/' => '/' as u32,
                'b' => 8,
                'f' => 12,
                'n' => 10,
                'r' => 13,
                't' => 9,
                _ => return Err(err("Invalid \\escape", end - 2)),
            };
        } else {
            next += 1;
            end = next + 4;
            if end >= len {
                return Err(err("Invalid \\uXXXX escape", next - 1));
            }
            let Some(mut u) = hex4(s, next) else {
                return Err(err("Invalid \\uXXXX escape", end - 5));
            };
            next = end;
            // A surrogate pair.
            if (0xd800..=0xdbff).contains(&u) && end + 6 < len {
                let backslash = s[next] == '\\';
                next += 1;
                let letter_u = backslash && s[next] == 'u';
                if letter_u {
                    next += 1;
                    end += 6;
                    let Some(u2) = hex4(s, next) else {
                        return Err(err("Invalid \\uXXXX escape", end - 5));
                    };
                    if (0xdc00..=0xdfff).contains(&u2) {
                        u = 0x10000 + (((u - 0xd800) << 10) | (u2 - 0xdc00));
                    } else {
                        end -= 6;
                    }
                }
            }
            ch = u;
        }
        out.push(char::from_u32(ch).unwrap_or('\u{fffd}'));
    }
    Ok((out, end))
}

/// `_parse_object_unicode`: `idx` is the index after `{`.
fn parse_object(s: &[char], mut idx: usize) -> Result<(Py, usize), Fail> {
    let mut pairs: Vec<(String, Py)> = Vec::new();
    idx = skip_ws(s, idx);
    if !at(s, idx, '}') {
        loop {
            if !at(s, idx, '"') {
                return Err(err(
                    "Expecting property name enclosed in double quotes",
                    idx,
                ));
            }
            let (key, next) = scanstring(s, idx + 1)?;
            idx = skip_ws(s, next);
            if !at(s, idx, ':') {
                return Err(err("Expecting ':' delimiter", idx));
            }
            idx = skip_ws(s, idx + 1);
            let (val, next) = scan_once(s, idx)?;
            match pairs.iter_mut().find(|(k, _)| *k == key) {
                Some(pair) => pair.1 = val,
                None => pairs.push((key, val)),
            }
            idx = skip_ws(s, next);
            if at(s, idx, '}') {
                break;
            }
            if !at(s, idx, ',') {
                return Err(err("Expecting ',' delimiter", idx));
            }
            idx = skip_ws(s, idx + 1);
        }
    }
    Ok((Py::Dict(pairs), idx + 1))
}

/// `_parse_array_unicode`: `idx` is the index after `[`.
fn parse_array(s: &[char], mut idx: usize) -> Result<(Py, usize), Fail> {
    let mut values = Vec::new();
    let end_idx = s.len().saturating_sub(1);
    idx = skip_ws(s, idx);
    if !at(s, idx, ']') {
        loop {
            let (val, next) = scan_once(s, idx)?;
            values.push(val);
            idx = skip_ws(s, next);
            if at(s, idx, ']') {
                break;
            }
            if !at(s, idx, ',') {
                return Err(err("Expecting ',' delimiter", idx));
            }
            idx = skip_ws(s, idx + 1);
        }
    }
    if !at(s, idx, ']') {
        return Err(err("Expecting value", end_idx));
    }
    Ok((Py::List(values), idx + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(s: &str) -> (&'static str, usize) {
        match loads(s.as_bytes()) {
            Err(LoadError::Decode(d)) => (d.msg, d.pos),
            other => panic!("{s:?}: {other:?}"),
        }
    }

    #[test]
    fn errors_as_python_raises_them() {
        // Measured on Spoolman's venv (CPython 3.12.13): json.loads(s) -> (e.msg, e.pos).
        for (s, want) in [
            ("", ("Expecting value", 0)),
            ("not json", ("Expecting value", 0)),
            (
                "{",
                ("Expecting property name enclosed in double quotes", 1),
            ),
            ("{\"a\"", ("Expecting ':' delimiter", 4)),
            ("{\"a\":", ("Expecting value", 5)),
            ("{\"a\":1", ("Expecting ',' delimiter", 6)),
            (
                "{\"a\":1,}",
                ("Expecting property name enclosed in double quotes", 7),
            ),
            ("[", ("Expecting value", 1)),
            ("[1", ("Expecting ',' delimiter", 2)),
            ("[1,]", ("Expecting value", 3)),
            ("\"abc", ("Unterminated string starting at", 0)),
            ("\"a\\x\"", ("Invalid \\escape", 2)),
            ("\"a\\u12\"", ("Invalid \\uXXXX escape", 3)),
            ("\"a\nb\"", ("Invalid control character at", 2)),
            ("1 2", ("Extra data", 2)),
            ("-", ("Expecting value", 0)),
        ] {
            assert_eq!(e(s), want, "{s:?}");
        }
    }

    #[test]
    fn values_as_python_makes_them() {
        assert_eq!(
            loads(br#"{"a":1,"b":[true,null,1.5,"x"],"a":2}"#).unwrap(),
            Py::Dict(vec![
                ("a".into(), Py::Int(2)),
                (
                    "b".into(),
                    Py::List(vec![
                        Py::Bool(true),
                        Py::None,
                        Py::Float(1.5),
                        Py::Str("x".into())
                    ])
                ),
            ])
        );
        assert!(matches!(loads(b"NaN").unwrap(), Py::Float(f) if f.is_nan()));
        assert_eq!(loads(b"\"\\ud83d\\ude00\"").unwrap(), Py::Str("😀".into()));
        assert_eq!(loads(b"\xef\xbb\xbf1").unwrap(), Py::Int(1));
        assert_eq!(loads(b"\xff"), Err(LoadError::Unicode));
    }

    /// `str(json.JSONDecodeError)` of `json.loads(s)` for a `str`, as CPython 3.12.13 (Spoolman's
    /// runtime, `reference/.venv-spoolman`) printed it for each case.
    #[test]
    fn str_errors_read_as_cpythons() {
        let cases = [
            ("", "Expecting value: line 1 column 1 (char 0)"),
            ("USD", "Expecting value: line 1 column 1 (char 0)"),
            (
                "\u{feff}\"x\"",
                "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)",
            ),
            ("[1,", "Expecting value: line 1 column 4 (char 3)"),
            (
                "{\"a\" 1}",
                "Expecting ':' delimiter: line 1 column 6 (char 5)",
            ),
            (
                "\"abc",
                "Unterminated string starting at: line 1 column 1 (char 0)",
            ),
            ("1 2", "Extra data: line 1 column 3 (char 2)"),
            ("[\n1,\n]", "Expecting value: line 3 column 1 (char 5)"),
            ("\n\n  x", "Expecting value: line 3 column 3 (char 4)"),
            ("tru", "Expecting value: line 1 column 1 (char 0)"),
            (
                "{\n\"a\": [1,\n 2,,]}",
                "Expecting value: line 3 column 4 (char 14)",
            ),
            ("\"\\x\"", "Invalid \\escape: line 1 column 2 (char 1)"),
            (
                "\u{20ac}\u{e9} x",
                "Expecting value: line 1 column 1 (char 0)",
            ),
        ];
        for (doc, want) in cases {
            let e = loads_str(doc).expect_err(doc);
            assert_eq!(e.message(doc), want, "{doc:?}");
        }
    }
}
