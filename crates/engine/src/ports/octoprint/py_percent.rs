//! Python's `str % args` for the arguments the JSON `parameters` of `printerCommand` can hold
//! (`server/api/printer.py:424-432`, `command % parameters`): `None`, `bool`, `int`, `float`, `str`,
//! `list` and `dict` (a `serde_json::Value`).
//!
//! Port of CPython 3.13.12 (`reference/cpython-3.13`, fetched from
//! `https://raw.githubusercontent.com/python/cpython/v3.13.12/`: the tag of the runtime the fixture is
//! made with; the `3.13` branch head has a later `precision too big` check that 3.13.12 does not
//! have), function by function:
//!
//! * `Objects/unicodeobject.c`: `PyUnicode_Format` (:14763), `unicode_format_arg` (:14719),
//!   `unicode_format_arg_parse` (:14274), `unicode_format_arg_format` (:14461),
//!   `unicode_format_arg_output` (:14565), `unicode_format_getnextarg` (:13931), `formatfloat` (:13954),
//!   `_PyUnicode_FormatLong` (:14009), `mainformatlong` (:14138), `formatchar` (:14232);
//! * `Objects/longobject.c`: `long_format_binary` (:2296, as `long_to_base`), `_PyLong_Format` (:2461),
//!   `_PyLong_FormatWriter` (:2475) and, for decimal, `long_to_decimal_string_internal` (:2059);
//! * `Python/pystrtod.c`: `PyOS_double_to_string` (:1221) and `format_float_short` (:969).
//!
//! The Python object model is the JSON one: no tuple is possible, so `PyTuple_Check(args)` is always
//! false (`arglen = -1`, `argidx = -2`); `PyMapping_Check(args)` is true for a dict and for a list
//! (`PyUnicode_Format` :14763, `ctx.dict`), false for the rest, and a `str` is excluded there.
//! `PyLong_CheckExact` is true for a JSON integer, `PyLong_Check` also for a `bool`.
//!
//! OURS (listed, each proven or `unimplemented!`):
//!
//! 1. `_Py_dg_dtoa` (`Python/dtoa.c`, modes 2 and 3) is not ported; Rust's `{:.*e}` / `{:.*}` give
//!    the same correctly rounded digits of the exact binary value (round half to even on an exact tie).
//!    The wrapper that turns the digits into dtoa's `(digits, decpt)` is [`dtoa`]; the fixture proves
//!    it on many floats (every `e f g` case of `scripts/octoprint-percent-check.py`).
//! 2. A float as `%d` (`int(float)`, `PyNumber_Long`) of any size is printed exactly
//!    ([`float_to_decimal_integer`], a small bignum, not `Objects/longobject.c` `PyLong_FromDouble`).
//! 3. Integers: a JSON integer is an `i64` or `u64` (`serde_json::Value`); an integer literal beyond
//!    `u64` is already a `f64` when it reaches here. That cannot be told from a float, so the top edge
//!    that parses the request MUST refuse an integer literal beyond `u64` (a loud error) before it
//!    makes the `Value`; nothing here rounds an integer. `serde_json` is built with `float_roundtrip`
//!    (`crates/engine/Cargo.toml`) so that a float literal gives the same `f64` as Python's `float()`.
//! 4. `%c` of a surrogate code point (`0xD800..=0xDFFF`) would make a `str` with a lone surrogate, which
//!    a Rust `String` cannot hold: `unimplemented!`.
//! 5. A width, a precision or a `*` above [`MEMORY_LIMIT`] (the result would be hundreds of MB; CPython
//!    would build it, or raise `MemoryError`): `unimplemented!`.
//! 6. The texts of the exceptions are CPython's (copied from the C source and compared by the fixture),
//!    except that a `KeyError` carries `repr(key)` as `str(KeyError)` does.
//!
//! Proven against CPython 3.13.12 by `scripts/octoprint-percent-check.py` (fixture
//! `py_percent_expected.json`, test `matches_cpython_3_13_percent`).

use super::py_format::py_str_repr;
use super::standard::{py_repr_value, py_str_value};
use serde_json::Value;
use std::fmt;

/// OURS (5): the largest width / precision this port builds.
pub const MEMORY_LIMIT: i64 = 1 << 26;

/// The exceptions `str % args` raises; the text is the exception's `str()`.
#[derive(Debug, Clone, PartialEq)]
pub enum PercentError {
    TypeError(String),
    ValueError(String),
    /// carries `repr(key)`, which is `str(KeyError(key))`
    KeyError(String),
    OverflowError(String),
}

impl PercentError {
    pub fn class(&self) -> &'static str {
        match self {
            PercentError::TypeError(_) => "TypeError",
            PercentError::ValueError(_) => "ValueError",
            PercentError::KeyError(_) => "KeyError",
            PercentError::OverflowError(_) => "OverflowError",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            PercentError::TypeError(m) | PercentError::ValueError(m) | PercentError::KeyError(m) | PercentError::OverflowError(m) => m,
        }
    }
}

impl fmt::Display for PercentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.class(), self.message())
    }
}

type R<T> = Result<T, PercentError>;

fn type_error<T>(m: impl Into<String>) -> R<T> {
    Err(PercentError::TypeError(m.into()))
}
fn value_error<T>(m: impl Into<String>) -> R<T> {
    Err(PercentError::ValueError(m.into()))
}

/// `Py_TYPE(v)->tp_name`
pub fn tp_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `PyLong_Check(v)`: the value of an `int` or `bool`
fn as_long(v: &Value) -> Option<i128> {
    match v {
        Value::Bool(b) => Some(*b as i128),
        Value::Number(n) => n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from)),
        _ => None,
    }
}

/// `PyLong_CheckExact(v)`
fn is_long_exact(v: &Value) -> bool {
    matches!(v, Value::Number(n) if !n.is_f64())
}

/// `PyFloat_AsDouble(v)` for a JSON value (`int`, `bool`, `float`; else `TypeError`)
fn as_double(v: &Value) -> R<f64> {
    match v {
        Value::Bool(b) => Ok(*b as i64 as f64),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i as f64)
            } else if let Some(u) = n.as_u64() {
                Ok(u as f64)
            } else {
                Ok(n.as_f64().expect("a JSON number is an f64"))
            }
        }
        other => type_error(format!("must be real number, not {}", tp_name(other))),
    }
}

// ---------------------------------------------------------------------------------------------
// Python/pystrtod.c
// ---------------------------------------------------------------------------------------------

/// OURS (1): `_Py_dg_dtoa(d, mode, ndigits)` for a finite `d` and mode 2 or 3: the digits without
/// trailing zeros, and `decpt` (the decimal point is after `decpt` digits). `d == 0` is `("0", 1)`; a
/// non-zero `d` that rounds to nothing in mode 3 is `("", -ndigits)` (dtoa.c `no_digits`).
fn dtoa(d: f64, mode: i32, ndigits: i64) -> (String, i64) {
    if d == 0.0 {
        return ("0".to_string(), 1);
    }
    let x = d.abs();
    if mode == 2 {
        // the exact expansion of a double has at most 767 significant digits, and `{:.*e}` takes a
        // precision up to 65535: more digits are zeros, which dtoa does not return
        let n = ndigits.clamp(1, 1100) as usize;
        let s = format!("{:.*e}", n - 1, x);
        let (mantissa, exp) = s.split_once('e').expect("{:e} has an exponent");
        let exp: i64 = exp.parse().expect("exponent");
        let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
        (digits.trim_end_matches('0').to_string(), exp + 1)
    } else {
        // at most 1074 digits after the point are not zeros
        let s = format!("{:.*}", ndigits.clamp(0, 1100) as usize, x);
        let (int_part, frac) = s.split_once('.').unwrap_or((&s, ""));
        let all = format!("{int_part}{frac}");
        let lz = all.len() - all.trim_start_matches('0').len();
        if lz == all.len() {
            return (String::new(), -ndigits);
        }
        let decpt = int_part.len() as i64 - lz as i64;
        (all.trim_start_matches('0').trim_end_matches('0').to_string(), decpt)
    }
}

/// `PyOS_double_to_string(val, format_code, precision, flags, NULL)` (pystrtod.c:1221) with `format_float_short`
/// (:969); `flags` is only `Py_DTSF_ALT` (`alt`), as `formatfloat` passes it. `format_code` is one of
/// `eEfFgG`.
pub fn double_to_string(val: f64, format_code: char, precision: i64, alt: bool) -> R<String> {
    let mut precision = precision;
    let upper = format_code.is_ascii_uppercase();
    let (code, mode) = match format_code {
        'E' | 'e' => {
            precision += 1;
            ('e', 2)
        }
        'F' | 'f' => ('f', 3),
        'G' | 'g' => {
            // precision 0 makes no sense for 'g' format; interpret as 1
            if precision == 0 {
                precision = 1;
            }
            ('g', 2)
        }
        other => unreachable!("PyOS_double_to_string: format code {other:?} (BadInternalCall)"),
    };
    if precision > MEMORY_LIMIT {
        unimplemented!("%-formatting of a float with a precision above {MEMORY_LIMIT} (ours, 5)");
    }
    Ok(format_float_short(val, code, mode, precision, upper, alt))
}

/// `format_float_short` (pystrtod.c:969) with `always_add_sign`, `add_dot_0_if_integer` and
/// `no_negative_zero` all 0 (what `formatfloat` passes) and the lower- or upper-case `float_strings`.
fn format_float_short(d: f64, format_code: char, mode: i32, precision: i64, upper: bool, use_alt_formatting: bool) -> String {
    let mut buf = String::new();
    let sign;
    let digits: String;
    let mut decpt: i64;
    if d.is_nan() || d.is_infinite() {
        // Infinities and nans here: ignore the actual sign of a nan
        let sign = d.is_infinite() && d < 0.0;
        if sign {
            buf.push('-');
        }
        let word = if d.is_nan() { "nan" } else { "inf" };
        buf.push_str(&if upper { word.to_uppercase() } else { word.to_string() });
        return buf;
    }
    let (dg, dp) = dtoa(d, mode, precision);
    sign = d.is_sign_negative();
    digits = dg;
    decpt = dp;
    let digits_len = digits.len() as i64;

    let mut use_exp = false;
    let mut vdigits_end = digits_len;
    match format_code {
        'e' => {
            use_exp = true;
            vdigits_end = precision;
        }
        'f' => vdigits_end = decpt + precision,
        'g' => {
            if decpt <= -4 || decpt > precision {
                use_exp = true;
            }
            if use_alt_formatting {
                vdigits_end = precision;
            }
        }
        _ => unreachable!(),
    }

    // if using an exponent, reset decimal point position to 1 and adjust exponent accordingly
    let mut exp = 0;
    if use_exp {
        exp = decpt - 1;
        decpt = 1;
    }
    // ensure vdigits_start < decpt <= vdigits_end
    let vdigits_start = if decpt <= 0 { decpt - 1 } else { 0 };
    vdigits_end = if vdigits_end > decpt { vdigits_end } else { decpt };

    if sign {
        buf.push('-');
    }

    // Zero padding on left of digit string
    if decpt <= 0 {
        buf.push_str(&"0".repeat((decpt - vdigits_start) as usize));
        buf.push('.');
        buf.push_str(&"0".repeat((0 - decpt) as usize));
    } else {
        buf.push_str(&"0".repeat((0 - vdigits_start) as usize));
    }

    // Digits, with included decimal point
    if 0 < decpt && decpt <= digits_len {
        buf.push_str(&digits[..decpt as usize]);
        buf.push('.');
        buf.push_str(&digits[decpt as usize..]);
    } else {
        buf.push_str(&digits);
    }

    // And zeros on the right
    if digits_len < decpt {
        buf.push_str(&"0".repeat((decpt - digits_len) as usize));
        buf.push('.');
        buf.push_str(&"0".repeat((vdigits_end - decpt) as usize));
    } else {
        buf.push_str(&"0".repeat((vdigits_end - digits_len) as usize));
    }

    // Delete a trailing decimal pt unless using alternative formatting.
    if buf.ends_with('.') && !use_alt_formatting {
        buf.pop();
    }

    // Now that we've done zero padding, add an exponent if needed.
    if use_exp {
        buf.push(if upper { 'E' } else { 'e' });
        // sprintf(p, "%+.02d", exp)
        buf.push_str(&format!("{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs()));
    }
    buf
}

// ---------------------------------------------------------------------------------------------
// Objects/longobject.c
// ---------------------------------------------------------------------------------------------

/// A Python `int` as `_PyUnicode_FormatLong` gets it: a JSON integer, or the integer a float gives
/// (`int(float)`, ours 2).
enum Long {
    Small(i128),
    /// the decimal digits of a (big) integer that came from a float, and its sign
    FromFloat { negative: bool, digits: String },
}

/// `PyNumber_ToBase(val, base)` (`_PyLong_Format`, longobject.c:2461): decimal, or `long_format_binary`
/// (:2296) with the prefix (`0x`, `0o`, `0b`) and the `-`.
fn long_to_base(v: &Long, base: u32) -> String {
    match v {
        Long::FromFloat { negative, digits } => {
            assert!(base == 10, "only %d and its kind take a float");
            format!("{}{digits}", if *negative { "-" } else { "" })
        }
        Long::Small(i) => {
            let negative = *i < 0;
            let mut a = i.unsigned_abs();
            let mut out: Vec<u8> = Vec::new();
            if a == 0 {
                out.push(b'0');
            }
            while a > 0 {
                let d = (a % base as u128) as u8;
                out.push(if d < 10 { b'0' + d } else { b'a' - 10 + d });
                a /= base as u128;
            }
            let mut s = String::new();
            if negative {
                s.push('-');
            }
            match base {
                16 => s.push_str("0x"),
                8 => s.push_str("0o"),
                2 => s.push_str("0b"),
                _ => {}
            }
            out.reverse();
            s.push_str(std::str::from_utf8(&out).expect("ASCII"));
            s
        }
    }
}

/// OURS (2): the decimal digits of `trunc(x)` for a finite `x`, exactly.
fn float_to_decimal_integer(x: f64) -> Long {
    let t = x.trunc();
    if t.abs() < 1e30 {
        return Long::Small(t as i128);
    }
    // x = m * 2**e with m < 2**53
    let bits = t.abs().to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i64 - 1075;
    let m = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    let mut limbs: Vec<u32> = vec![(m % 1_000_000_000) as u32, ((m / 1_000_000_000) % 1_000_000_000) as u32, (m / 1_000_000_000_000_000_000) as u32];
    for _ in 0..exp {
        let mut carry = 0u64;
        for l in limbs.iter_mut() {
            let v = *l as u64 * 2 + carry;
            *l = (v % 1_000_000_000) as u32;
            carry = v / 1_000_000_000;
        }
        if carry > 0 {
            limbs.push(carry as u32);
        }
    }
    while limbs.len() > 1 && *limbs.last().unwrap() == 0 {
        limbs.pop();
    }
    let mut digits = format!("{}", limbs.last().unwrap());
    for l in limbs.iter().rev().skip(1) {
        digits.push_str(&format!("{l:09}"));
    }
    Long::FromFloat { negative: t < 0.0, digits }
}

// ---------------------------------------------------------------------------------------------
// Objects/unicodeobject.c
// ---------------------------------------------------------------------------------------------

const F_LJUST: u32 = 1 << 0;
const F_SIGN: u32 = 1 << 1;
const F_BLANK: u32 = 1 << 2;
const F_ALT: u32 = 1 << 3;
const F_ZERO: u32 = 1 << 4;

const MAX_UNICODE: i128 = 0x10ffff;

/// `struct unicode_format_arg_t`
struct Arg {
    ch: u32,
    flags: u32,
    width: i64,
    prec: i64,
    sign: bool,
}

/// What `unicode_format_arg_format` returns: `1` (written into the writer) or `0` (a string)
enum Formatted {
    Written,
    Str(Vec<char>),
}

/// `struct unicode_formatter_t`
struct Ctx<'a> {
    args: &'a Value,
    arglen: i64,
    argidx: i64,
    dict: Option<&'a Value>,
    fmt: Vec<char>,
    fmtcnt: i64,
    fmtpos: i64,
    writer: String,
}

fn ch(c: char) -> u32 {
    c as u32
}

/// `PyObject_ASCII(v)`: `repr`, then `backslashreplace` of the non-ASCII characters
fn py_ascii(v: &Value) -> String {
    let repr = match v {
        Value::String(s) => py_str_repr(s, true),
        other => py_repr_value(other),
    };
    let mut out = String::new();
    for c in repr.chars() {
        if c.is_ascii() {
            out.push(c);
        } else if (c as u32) < 0x100 {
            out.push_str(&format!("\\x{:02x}", c as u32));
        } else if (c as u32) < 0x10000 {
            out.push_str(&format!("\\u{:04x}", c as u32));
        } else {
            out.push_str(&format!("\\U{:08x}", c as u32));
        }
    }
    out
}

impl<'a> Ctx<'a> {
    /// `PyUnicode_READ(fmtkind, fmtdata, fmtpos)`; the terminating NUL after the end
    fn read(&self) -> u32 {
        self.fmt.get(self.fmtpos as usize).map_or(0, |c| *c as u32)
    }

    /// `unicode_format_getnextarg` (:13931)
    fn getnextarg(&mut self) -> R<&'a Value> {
        let argidx = self.argidx;
        if argidx < self.arglen {
            self.argidx += 1;
            // `ctx->arglen < 0`: the single argument
            return Ok(self.args);
        }
        type_error("not enough arguments for format string")
    }

    /// `formatfloat` (:13954), the string form
    fn formatfloat(&self, v: &Value, arg: &Arg) -> R<String> {
        let x = as_double(v)?;
        let prec = if arg.prec < 0 { 6 } else { arg.prec };
        double_to_string(x, char::from_u32(arg.ch).expect("eEfFgG"), prec, arg.flags & F_ALT != 0)
    }

    /// `_PyUnicode_FormatLong` (:14009)
    fn format_long(val: &Long, alt: bool, prec: i64, type_: u32) -> R<Vec<char>> {
        // Avoid exceeding SSIZE_T_MAX
        if prec > i32::MAX as i64 - 3 {
            return Err(PercentError::OverflowError("precision too large".to_string()));
        }
        if prec > MEMORY_LIMIT {
            unimplemented!("%-formatting of an integer with a precision above {MEMORY_LIMIT} (ours, 5)");
        }
        let mut numnondigits: i64 = 0;
        let result = match type_ {
            t if t == ch('d') || t == ch('i') || t == ch('u') => long_to_base(val, 10),
            t if t == ch('o') => {
                numnondigits = 2;
                long_to_base(val, 8)
            }
            _ => {
                numnondigits = 2;
                long_to_base(val, 16)
            }
        };
        let mut buf: Vec<char> = result.chars().collect();
        let sign = buf[0] == '-';
        numnondigits += sign as i64;
        let numdigits = buf.len() as i64 - numnondigits;

        // Get rid of base marker unless F_ALT
        if !alt && (type_ == ch('o') || type_ == ch('x') || type_ == ch('X')) {
            numnondigits -= 2;
            let s = sign as usize;
            buf.drain(s..s + 2);
        }
        let _ = numnondigits;

        // Fill with leading zeroes to meet minimum width.
        if prec > numdigits {
            let nn = (buf.len() as i64 - numdigits) as usize;
            let mut b1: Vec<char> = buf[..nn].to_vec();
            b1.extend(std::iter::repeat('0').take((prec - numdigits) as usize));
            b1.extend_from_slice(&buf[nn..]);
            buf = b1;
        }

        // Fix up case for hex conversions.
        if type_ == ch('X') {
            for c in buf.iter_mut() {
                if *c >= 'a' && *c <= 'x' {
                    *c = ((*c as u8) - (b'a' - b'A')) as char;
                }
            }
        }
        Ok(buf)
    }

    /// `mainformatlong` (:14138)
    fn mainformatlong(&mut self, v: &Value, arg: &Arg) -> R<Formatted> {
        let type_ = arg.ch;
        let wrongtype = |v: &Value| -> R<Formatted> {
            let c = char::from_u32(type_).expect("conversion");
            if type_ == ch('o') || type_ == ch('x') || type_ == ch('X') {
                type_error(format!("%{c} format: an integer is required, not {}", tp_name(v)))
            } else {
                type_error(format!("%{c} format: a real number is required, not {}", tp_name(v)))
            }
        };
        // PyNumber_Check: int, bool, float
        if !matches!(v, Value::Bool(_) | Value::Number(_)) {
            return wrongtype(v);
        }

        let iobj: Long = match as_long(v) {
            Some(i) => Long::Small(i),
            None => {
                // a float: `_PyNumber_Index` (TypeError) for o, x, X; `PyNumber_Long` for the rest
                if type_ == ch('o') || type_ == ch('x') || type_ == ch('X') {
                    return wrongtype(v);
                }
                let x = as_double(v)?;
                if x.is_nan() {
                    return value_error("cannot convert float NaN to integer");
                }
                if x.is_infinite() {
                    return Err(PercentError::OverflowError("cannot convert float infinity to integer".to_string()));
                }
                float_to_decimal_integer(x)
            }
        };

        if is_long_exact(v) && arg.width == -1 && arg.prec == -1 && arg.flags & (F_SIGN | F_BLANK) == 0 && type_ != ch('X') {
            // Fast path: `_PyLong_FormatWriter(writer, v, base, alternate)`
            let alternate = arg.flags & F_ALT != 0;
            let base = if type_ == ch('o') {
                8
            } else if type_ == ch('x') || type_ == ch('X') {
                16
            } else {
                10
            };
            let mut s = long_to_base(&iobj, base);
            if base != 10 && !alternate {
                // long_format_binary with alternate = 0: no prefix
                let neg = s.starts_with('-');
                s = format!("{}{}", if neg { "-" } else { "" }, &s[if neg { 3 } else { 2 }..]);
            }
            self.writer.push_str(&s);
            return Ok(Formatted::Written);
        }

        Ok(Formatted::Str(Self::format_long(&iobj, arg.flags & F_ALT != 0, arg.prec, type_)?))
    }

    /// `formatchar` (:14232)
    fn formatchar(v: &Value) -> R<char> {
        let on_error = || type_error::<char>("%c requires int or char");
        if let Value::String(s) = v {
            let mut it = s.chars();
            return match (it.next(), it.next()) {
                (Some(c), None) => Ok(c),
                _ => on_error(),
            };
        }
        let Some(x) = as_long(v) else {
            // PyLong_AsLongAndOverflow: `_PyNumber_Index` raises TypeError
            return on_error();
        };
        // `x < 0 || x > MAX_UNICODE` (an overflow of C long is -1 there)
        if x < 0 || x > MAX_UNICODE {
            return Err(PercentError::OverflowError("%c arg not in range(0x110000)".to_string()));
        }
        match char::from_u32(x as u32) {
            Some(c) => Ok(c),
            None => unimplemented!("%c of the surrogate code point {x:#x}: a Python str with a lone surrogate (ours, 4)"),
        }
    }

    /// `unicode_format_arg_parse` (:14274)
    fn parse(&mut self, arg: &mut Arg) -> R<()> {
        if arg.ch == ch('(') {
            // Get argument value from a dictionary. Example: "%(name)s".
            let Some(dict) = self.dict else {
                return type_error("format requires a mapping");
            };
            self.fmtpos += 1;
            self.fmtcnt -= 1;
            let keystart = self.fmtpos;
            let mut pcount = 1;
            // Skip over balanced parentheses
            while pcount > 0 {
                self.fmtcnt -= 1;
                if self.fmtcnt < 0 {
                    break;
                }
                arg.ch = self.read();
                if arg.ch == ch(')') {
                    pcount -= 1;
                } else if arg.ch == ch('(') {
                    pcount += 1;
                }
                self.fmtpos += 1;
            }
            let keylen = self.fmtpos - keystart - 1;
            if self.fmtcnt < 0 || pcount > 0 {
                return value_error("incomplete format key");
            }
            let key: String = self.fmt[keystart as usize..(keystart + keylen) as usize].iter().collect();
            // `PyObject_GetItem(ctx->dict, key)`
            let found = match dict {
                Value::Object(o) => match o.get(&key) {
                    Some(v) => v,
                    None => return Err(PercentError::KeyError(py_str_repr(&key, false))),
                },
                // a list: `list indices must be integers or slices, not str`
                _ => return type_error("list indices must be integers or slices, not str"),
            };
            self.args = found;
            self.arglen = -1;
            self.argidx = -2;
        }

        // Parse flags. Example: "%+i" => flags=F_SIGN.
        loop {
            self.fmtcnt -= 1;
            if self.fmtcnt < 0 {
                break;
            }
            arg.ch = self.read();
            self.fmtpos += 1;
            match char::from_u32(arg.ch) {
                Some('-') => arg.flags |= F_LJUST,
                Some('+') => arg.flags |= F_SIGN,
                Some(' ') => arg.flags |= F_BLANK,
                Some('#') => arg.flags |= F_ALT,
                Some('0') => arg.flags |= F_ZERO,
                _ => break,
            }
        }

        // Parse width. Example: "%10s" => width=10
        if arg.ch == ch('*') {
            let v = self.getnextarg()?;
            let Some(n) = as_long(v) else {
                return type_error("* wants int");
            };
            // PyLong_AsSsize_t
            if n > i64::MAX as i128 || n < i64::MIN as i128 {
                return Err(PercentError::OverflowError("Python int too large to convert to C ssize_t".to_string()));
            }
            arg.width = n as i64;
            if arg.width < 0 {
                arg.flags |= F_LJUST;
                arg.width = arg.width.checked_neg().unwrap_or(i64::MAX);
            }
            self.fmtcnt -= 1;
            if self.fmtcnt >= 0 {
                arg.ch = self.read();
                self.fmtpos += 1;
            }
        } else if arg.ch >= ch('0') && arg.ch <= ch('9') {
            arg.width = (arg.ch - ch('0')) as i64;
            loop {
                self.fmtcnt -= 1;
                if self.fmtcnt < 0 {
                    break;
                }
                arg.ch = self.read();
                self.fmtpos += 1;
                if arg.ch < ch('0') || arg.ch > ch('9') {
                    break;
                }
                if arg.width > (i64::MAX - (arg.ch - ch('0')) as i64) / 10 {
                    return value_error("width too big");
                }
                arg.width = arg.width * 10 + (arg.ch - ch('0')) as i64;
            }
        }

        // Parse precision. Example: "%.3f" => prec=3
        if arg.ch == ch('.') {
            arg.prec = 0;
            self.fmtcnt -= 1;
            if self.fmtcnt >= 0 {
                arg.ch = self.read();
                self.fmtpos += 1;
            }
            if arg.ch == ch('*') {
                let v = self.getnextarg()?;
                let Some(n) = as_long(v) else {
                    return type_error("* wants int");
                };
                // PyLong_AsInt
                if n > i32::MAX as i128 || n < i32::MIN as i128 {
                    return Err(PercentError::OverflowError("Python int too large to convert to C int".to_string()));
                }
                arg.prec = n as i64;
                if arg.prec < 0 {
                    arg.prec = 0;
                }
                self.fmtcnt -= 1;
                if self.fmtcnt >= 0 {
                    arg.ch = self.read();
                    self.fmtpos += 1;
                }
            } else if arg.ch >= ch('0') && arg.ch <= ch('9') {
                arg.prec = (arg.ch - ch('0')) as i64;
                loop {
                    self.fmtcnt -= 1;
                    if self.fmtcnt < 0 {
                        break;
                    }
                    arg.ch = self.read();
                    self.fmtpos += 1;
                    if arg.ch < ch('0') || arg.ch > ch('9') {
                        break;
                    }
                    if arg.prec > (i32::MAX as i64 - (arg.ch - ch('0')) as i64) / 10 {
                        return value_error("precision too big");
                    }
                    arg.prec = arg.prec * 10 + (arg.ch - ch('0')) as i64;
                }
            }
        }

        // Ignore "h", "l" and "L" format prefix (ex: "%hi" or "%ls")
        if self.fmtcnt >= 0 && (arg.ch == ch('h') || arg.ch == ch('l') || arg.ch == ch('L')) {
            self.fmtcnt -= 1;
            if self.fmtcnt >= 0 {
                arg.ch = self.read();
                self.fmtpos += 1;
            }
        }
        if self.fmtcnt < 0 {
            return value_error("incomplete format");
        }
        Ok(())
    }

    /// `unicode_format_arg_format` (:14461)
    fn format_arg(&mut self, arg: &mut Arg) -> R<Formatted> {
        let v = self.getnextarg()?;
        let c = char::from_u32(arg.ch);
        match c {
            Some('s') | Some('r') | Some('a') => {
                if is_long_exact(v) && arg.width == -1 && arg.prec == -1 {
                    // Fast path
                    self.writer.push_str(&long_to_base(&Long::Small(as_long(v).expect("int")), 10));
                    return Ok(Formatted::Written);
                }
                let s = match (c, v) {
                    (Some('s'), Value::String(s)) => s.clone(),
                    (Some('s'), other) => py_str_value(other),
                    (Some('r'), other) => py_repr_value(other),
                    (_, other) => py_ascii(other),
                };
                Ok(Formatted::Str(s.chars().collect()))
            }
            Some('i') | Some('d') | Some('u') | Some('o') | Some('x') | Some('X') => match self.mainformatlong(v, arg)? {
                Formatted::Written => Ok(Formatted::Written),
                s => {
                    arg.sign = true;
                    Ok(s)
                }
            },
            Some('e') | Some('E') | Some('f') | Some('F') | Some('g') | Some('G') => {
                if arg.width == -1 && arg.prec == -1 && arg.flags & (F_SIGN | F_BLANK) == 0 {
                    // Fast path
                    let s = self.formatfloat(v, arg)?;
                    self.writer.push_str(&s);
                    return Ok(Formatted::Written);
                }
                arg.sign = true;
                Ok(Formatted::Str(self.formatfloat(v, arg)?.chars().collect()))
            }
            Some('c') => {
                let c = Self::formatchar(v)?;
                if arg.width == -1 && arg.prec == -1 {
                    // Fast path
                    self.writer.push(c);
                    return Ok(Formatted::Written);
                }
                Ok(Formatted::Str(vec![c]))
            }
            _ => value_error(format!(
                "unsupported format character '{}' (0x{:x}) at index {}",
                if (31..=126).contains(&arg.ch) { char::from_u32(arg.ch).expect("ASCII") } else { '?' },
                arg.ch,
                self.fmtpos - 1
            )),
        }
    }

    /// `unicode_format_arg_output` (:14565)
    fn output(&mut self, arg: &mut Arg, s: &[char]) {
        let mut fill = ' ';
        if arg.sign && arg.flags & F_ZERO != 0 {
            fill = '0';
        }

        let mut len = s.len() as i64;
        if (arg.width == -1 || arg.width <= len) && (arg.prec == -1 || arg.prec >= len) && arg.flags & (F_SIGN | F_BLANK) == 0 {
            // Fast path
            self.writer.extend(s.iter());
            return;
        }

        // Truncate the string for "s", "r" and "a" formats if the precision is set
        if arg.ch == ch('s') || arg.ch == ch('r') || arg.ch == ch('a') {
            if arg.prec >= 0 && len > arg.prec {
                len = arg.prec;
            }
        }

        // Adjust sign and width
        let mut pindex: usize = 0;
        let mut signchar = '\0';
        if arg.sign {
            let c = s[pindex];
            if c == '-' || c == '+' {
                signchar = c;
                len -= 1;
                pindex += 1;
            } else if arg.flags & F_SIGN != 0 {
                signchar = '+';
            } else if arg.flags & F_BLANK != 0 {
                signchar = ' ';
            } else {
                arg.sign = false;
            }
        }
        if arg.width < len {
            arg.width = len;
        }
        if arg.width > MEMORY_LIMIT {
            unimplemented!("%-formatting with a width above {MEMORY_LIMIT} (ours, 5)");
        }

        // Write the sign if needed
        if arg.sign {
            if fill != ' ' {
                self.writer.push(signchar);
            }
            if arg.width > len {
                arg.width -= 1;
            }
        }

        // Write the numeric prefix for "x", "X" and "o" formats if the alternate form is used.
        let alt_prefix = arg.flags & F_ALT != 0 && (arg.ch == ch('x') || arg.ch == ch('X') || arg.ch == ch('o'));
        if alt_prefix {
            if fill != ' ' {
                self.writer.push(s[pindex]);
                self.writer.push(s[pindex + 1]);
                pindex += 2;
            }
            arg.width -= 2;
            if arg.width < 0 {
                arg.width = 0;
            }
            len -= 2;
        }

        // Pad left with the fill character if needed
        if arg.width > len && arg.flags & F_LJUST == 0 {
            let sublen = arg.width - len;
            self.writer.extend(std::iter::repeat(fill).take(sublen as usize));
            arg.width = len;
        }

        // If padding with spaces: write sign if needed and/or numeric prefix if the alternate form is used
        if fill == ' ' {
            if arg.sign {
                self.writer.push(signchar);
            }
            if alt_prefix {
                self.writer.push(s[pindex]);
                self.writer.push(s[pindex + 1]);
                pindex += 2;
            }
        }

        // Write characters
        if len > 0 {
            self.writer.extend(s[pindex..pindex + len as usize].iter());
        }

        // Pad right with the fill character if needed
        if arg.width > len {
            let sublen = arg.width - len;
            self.writer.extend(std::iter::repeat(' ').take(sublen as usize));
        }
    }

    /// `unicode_format_arg` (:14719)
    fn format_one(&mut self) -> R<()> {
        let c = self.read();
        if c == ch('%') {
            self.fmtpos += 1;
            self.fmtcnt -= 1;
            self.writer.push('%');
            return Ok(());
        }
        let mut arg = Arg { ch: c, flags: 0, width: -1, prec: -1, sign: false };

        self.parse(&mut arg)?;

        match self.format_arg(&mut arg)? {
            Formatted::Written => {}
            Formatted::Str(s) => self.output(&mut arg, &s),
        }

        if self.dict.is_some() && self.argidx < self.arglen {
            return type_error("not all arguments converted during string formatting");
        }
        Ok(())
    }
}

/// `PyUnicode_Format(format, args)` (:14763), i.e. `format % args`, with `args` a JSON value
pub fn percent_format(format: &str, args: &Value) -> R<String> {
    let fmt: Vec<char> = format.chars().collect();
    let mut ctx = Ctx {
        args,
        // `PyTuple_Check(args)` is false for a JSON value
        arglen: -1,
        argidx: -2,
        // `PyMapping_Check(args) && !PyTuple_Check(args) && !PyUnicode_Check(args)`
        dict: if matches!(args, Value::Object(_) | Value::Array(_)) { Some(args) } else { None },
        fmtcnt: fmt.len() as i64,
        fmtpos: 0,
        fmt,
        writer: String::new(),
    };

    loop {
        ctx.fmtcnt -= 1;
        if ctx.fmtcnt < 0 {
            break;
        }
        if ctx.read() != ch('%') {
            let nonfmtpos = ctx.fmtpos;
            ctx.fmtpos += 1;
            while ctx.fmtcnt >= 0 && ctx.read() != ch('%') {
                ctx.fmtpos += 1;
                ctx.fmtcnt -= 1;
            }
            if ctx.fmtcnt < 0 {
                ctx.fmtpos -= 1;
            }
            ctx.writer.extend(ctx.fmt[nonfmtpos as usize..ctx.fmtpos as usize].iter());
        } else {
            ctx.fmtpos += 1;
            ctx.format_one()?;
        }
    }

    if ctx.argidx < ctx.arglen && ctx.dict.is_none() {
        return type_error("not all arguments converted during string formatting");
    }
    Ok(ctx.writer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// the cases and what CPython 3.13.12 gave (`scripts/octoprint-percent-check.py`): the text, or
    /// `Class: message` of the exception
    #[test]
    fn matches_cpython_3_13_percent() {
        let fixture: Value = serde_json::from_str(include_str!("py_percent_expected.json")).expect("fixture");
        let cases = fixture["cases"].as_array().expect("cases");
        assert!(cases.len() > 1000, "the fixture has {} cases", cases.len());
        let mut failures = Vec::new();
        for case in cases {
            let format = case["format"].as_str().expect("format");
            let expected = &case["expected"];
            let actual = match percent_format(format, &case["args"]) {
                Ok(s) => json!({"ok": s}),
                Err(e) => json!({"error": e.to_string()}),
            };
            if &actual != expected {
                failures.push(format!("{format:?} % {} -> {actual}, CPython: {expected}", case["args"]));
            }
        }
        assert!(failures.is_empty(), "{} of {} differ:\n{}", failures.len(), cases.len(), failures.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
    }

    #[test]
    fn known_bugs_of_the_hand_written_version_are_gone() {
        assert_eq!(Err(PercentError::TypeError("not enough arguments for format string".into())), percent_format("%s %s", &json!("a")));
        assert_eq!(Ok("5".to_string()), percent_format("%ld", &json!(5)));
        assert_eq!(Ok("1000000000000000019884624838656".to_string()), percent_format("%d", &json!(1e30)));
        assert_eq!(Ok("18446744073709551615".to_string()), percent_format("%d", &json!(u64::MAX)));
    }
}
