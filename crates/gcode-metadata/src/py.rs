//! The Python built-ins gcode-metadata stands on, ported from CPython 3.13.12
//! (`reference/cpython-3.13`, PSF licence): the values (`int`, `float`, `str`, `bool`, `None`,
//! `list`, `dict`) as [`PyValue`], `int(str)` and `int(float)` (`Objects/longobject.c`,
//! `Objects/floatobject.c`), `float(str)` (`Objects/floatobject.c`, `Python/pystrtod.c`),
//! `round(float, n)`, `str()` and `repr()` (`Objects/unicodeobject.c` `unicode_repr`),
//! `str.strip()`, `str.isalpha()`, and `json.loads` / `json.dump` (`Lib/json/*.py`,
//! `Modules/_json.c`).
//!
//! The character predicates are CPython's own tables ([`crate::py_unicode`], generated).
//!
//! Not covered (Python would accept these, ours refuses them as a `ValueError`): strings with
//! lone surrogates (a Rust `String` cannot hold them; `json.loads` and the `surrogatepass`
//! decoding of `json.loads(bytes)` can make them), and the exact nesting depth at which
//! `json.loads` raises `RecursionError` (CPython's C stack budget at the call; ours is the
//! 9,998 levels measured at the top level of CPython 3.13.12).

use std::fmt::Write as _;

use crate::py_unicode;

/// A Python exception, by class. `JSONDecodeError` and `UnicodeDecodeError` are `ValueError`s;
/// `Other` names any other class (`BadZipFile`, `NotImplementedError`, ...).
#[derive(Debug, Clone, PartialEq)]
pub enum PyErr {
    ValueError(String),
    TypeError(String),
    OverflowError(String),
    RecursionError(String),
    KeyError(String),
    AttributeError(String),
    AssertionError(String),
    Other(&'static str, String),
}

impl PyErr {
    pub fn is_value_error(&self) -> bool {
        matches!(self, PyErr::ValueError(_))
    }

    pub fn class_name(&self) -> &'static str {
        match self {
            PyErr::ValueError(_) => "ValueError",
            PyErr::TypeError(_) => "TypeError",
            PyErr::OverflowError(_) => "OverflowError",
            PyErr::RecursionError(_) => "RecursionError",
            PyErr::KeyError(_) => "KeyError",
            PyErr::AttributeError(_) => "AttributeError",
            PyErr::AssertionError(_) => "AssertionError",
            PyErr::Other(class, _) => class,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            PyErr::ValueError(m)
            | PyErr::TypeError(m)
            | PyErr::OverflowError(m)
            | PyErr::RecursionError(m)
            | PyErr::KeyError(m)
            | PyErr::AttributeError(m)
            | PyErr::AssertionError(m)
            | PyErr::Other(_, m) => m,
        }
    }
}

impl std::fmt::Display for PyErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.class_name(), self.message())
    }
}

impl std::error::Error for PyErr {}

fn in_ranges(table: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    table
        .binary_search_by(|&(first, last)| {
            if last < c {
                std::cmp::Ordering::Less
            } else if first > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `str.isspace()` of one character.
pub fn isspace(c: char) -> bool {
    in_ranges(py_unicode::ISSPACE, c)
}

/// `str.isalpha()` of one character.
pub fn isalpha(c: char) -> bool {
    in_ranges(py_unicode::ISALPHA, c)
}

/// `str.isprintable()` of one character.
pub fn isprintable(c: char) -> bool {
    in_ranges(py_unicode::ISPRINTABLE, c)
}

/// `\w` of `re` on `str`.
pub fn isword(c: char) -> bool {
    in_ranges(py_unicode::WORD, c)
}

/// The digit value of a `str.isdecimal()` character.
pub fn decimal(c: char) -> Option<u8> {
    let code = c as u32;
    let table = py_unicode::DECIMAL;
    let index = table.partition_point(|&(_, last, _)| last < code);
    match table.get(index) {
        Some(&(first, _, value)) if first <= code => Some(value + (code - first) as u8),
        _ => None,
    }
}

/// `str.strip()` with no argument.
pub fn strip(s: &str) -> &str {
    s.trim_matches(isspace)
}

/// `"".join(filter(str.isalpha, s))`
pub fn alpha_only(s: &str) -> String {
    s.chars().filter(|c| isalpha(*c)).collect()
}

/// `s[n:]` on a `str`: by characters.
pub fn skip_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((index, _)) => &s[index..],
        None => "",
    }
}

/// `Py_ISSPACE`: the C-locale space of the ASCII parsers.
fn ascii_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

/// `_PyUnicode_TransformDecimalAndSpaceToASCII`: below 127 kept, spaces a space, decimal
/// digits `0`-`9`; the first other character becomes `?` and ends the text.
fn transform_decimal_and_space(s: &str) -> Vec<u8> {
    if s.is_ascii() {
        return s.as_bytes().to_vec();
    }
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        if (c as u32) < 127 {
            out.push(c as u8);
        } else if isspace(c) {
            out.push(b' ');
        } else if let Some(d) = decimal(c) {
            out.push(b'0' + d);
        } else {
            out.push(b'?');
            break;
        }
    }
    out
}

/// A Python `int` of any size, kept as its canonical decimal digits (`-` and no leading zeros).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PyInt(String);

/// `sys.int_info.default_max_str_digits`
pub const MAX_STR_DIGITS: usize = 4300;

impl PyInt {
    pub fn from_i64(value: i64) -> PyInt {
        PyInt(value.to_string())
    }

    /// From an optional `-` and ASCII digits (leading zeros allowed).
    fn from_digits(negative: bool, digits: &str) -> PyInt {
        let digits = digits.trim_start_matches('0');
        if digits.is_empty() {
            return PyInt("0".to_owned());
        }
        PyInt(if negative {
            format!("-{digits}")
        } else {
            digits.to_owned()
        })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_i64(&self) -> Option<i64> {
        self.0.parse().ok()
    }

    pub fn is_zero(&self) -> bool {
        self.0 == "0"
    }

    /// `float(int)`: correctly rounded, `OverflowError` past the float range.
    pub fn to_f64(&self) -> Result<f64, PyErr> {
        let x: f64 = self.0.parse().expect("canonical digits parse as a float");
        if x.is_infinite() {
            return Err(PyErr::OverflowError(
                "int too large to convert to float".into(),
            ));
        }
        Ok(x)
    }
}

/// `PyLong_FromString(s, &end, 10)` as `int(str)` calls it on the transformed text.
fn long_from_ascii(s: &[u8]) -> Option<Result<PyInt, PyErr>> {
    let mut p = 0;
    while p < s.len() && ascii_space(s[p]) {
        p += 1;
    }
    let mut negative = false;
    if p < s.len() && s[p] == b'+' {
        p += 1;
    } else if p < s.len() && s[p] == b'-' {
        p += 1;
        negative = true;
    }
    if p < s.len() && s[p] == b'_' {
        return None;
    }
    let start = p;
    let mut prev = 0u8;
    let mut digits = String::new();
    while p < s.len() && (s[p].is_ascii_digit() || s[p] == b'_') {
        if s[p] == b'_' {
            if prev == b'_' {
                return None;
            }
        } else {
            digits.push(s[p] as char);
        }
        prev = s[p];
        p += 1;
    }
    if prev == b'_' || p == start {
        return None;
    }
    while p < s.len() && ascii_space(s[p]) {
        p += 1;
    }
    if p != s.len() {
        return None;
    }
    if digits.len() > MAX_STR_DIGITS {
        return Some(Err(PyErr::ValueError(format!(
            "Exceeds the limit ({MAX_STR_DIGITS} digits) for integer string conversion: value \
             has {} digits; use sys.set_int_max_str_digits() to increase the limit",
            digits.len()
        ))));
    }
    Some(Ok(PyInt::from_digits(negative, &digits)))
}

/// `int(s)` for a `str`.
pub fn int_from_str(s: &str) -> Result<PyInt, PyErr> {
    // a NUL ends the C string the parser reads, so it is never "the end"
    let ascii = transform_decimal_and_space(s);
    match long_from_ascii(&ascii) {
        Some(result) => result,
        // `%.200R`: the repr cut to 200 characters
        None => Err(PyErr::ValueError(format!(
            "invalid literal for int() with base 10: {}",
            repr_str(s).chars().take(200).collect::<String>()
        ))),
    }
}

/// `int(x)` for a `float`: truncated toward zero.
pub fn int_from_f64(x: f64) -> Result<PyInt, PyErr> {
    if x.is_nan() {
        return Err(PyErr::ValueError(
            "cannot convert float NaN to integer".into(),
        ));
    }
    if x.is_infinite() {
        return Err(PyErr::OverflowError(
            "cannot convert float infinity to integer".into(),
        ));
    }
    let t = x.trunc();
    if t.abs() < 9.0e15 {
        return Ok(PyInt::from_i64(t as i64));
    }
    // exact: mantissa * 2^exp, in base 10^9 limbs
    let bits = t.abs().to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32 - 1075;
    let mantissa = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    let mut limbs: Vec<u64> = vec![mantissa % 1_000_000_000, mantissa / 1_000_000_000];
    for _ in 0..exp {
        let mut carry = 0u64;
        for limb in limbs.iter_mut() {
            let v = *limb * 2 + carry;
            *limb = v % 1_000_000_000;
            carry = v / 1_000_000_000;
        }
        if carry > 0 {
            limbs.push(carry);
        }
    }
    while limbs.len() > 1 && *limbs.last().unwrap() == 0 {
        limbs.pop();
    }
    let mut digits = limbs.last().unwrap().to_string();
    for limb in limbs.iter().rev().skip(1) {
        let _ = write!(digits, "{limb:09}");
    }
    Ok(PyInt::from_digits(x < 0.0, &digits))
}

/// `_Py_dg_strtod`'s syntax, then `_Py_parse_inf_or_nan`; the whole text must be used.
fn ascii_strtod(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut p = 0;
    if p < b.len() && (b[p] == b'+' || b[p] == b'-') {
        p += 1;
    }
    let int_start = p;
    while p < b.len() && b[p].is_ascii_digit() {
        p += 1;
    }
    let mut ndigits = p - int_start;
    if p < b.len() && b[p] == b'.' {
        p += 1;
        let frac_start = p;
        while p < b.len() && b[p].is_ascii_digit() {
            p += 1;
        }
        ndigits += p - frac_start;
    }
    if ndigits > 0 {
        if p < b.len() && (b[p] == b'e' || b[p] == b'E') {
            let mut q = p + 1;
            if q < b.len() && (b[q] == b'+' || b[q] == b'-') {
                q += 1;
            }
            let exp_start = q;
            while q < b.len() && b[q].is_ascii_digit() {
                q += 1;
            }
            if q > exp_start {
                p = q;
            }
        }
        if p != b.len() {
            return None;
        }
        // Rust's parse is correctly rounded, as `_Py_dg_strtod`; overflow gives inf, as
        // `PyOS_string_to_double` with no overflow exception
        return s.parse().ok();
    }
    // `_Py_parse_inf_or_nan`
    let (negate, rest) = match b.first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let lower = rest.to_ascii_lowercase();
    let value = if lower == "inf" || lower == "infinity" {
        f64::INFINITY
    } else if lower == "nan" {
        f64::NAN
    } else {
        return None;
    };
    Some(if negate { -value } else { value })
}

/// `float(s)` for a `str`.
pub fn float_from_str(s: &str) -> Result<f64, PyErr> {
    let fail = || {
        PyErr::ValueError(format!(
            "could not convert string to float: {}",
            repr_str(s)
        ))
    };
    let ascii = transform_decimal_and_space(s);
    // `_Py_string_to_number_with_underscores`: `strchr` looks for a `_` up to the first NUL;
    // with one, the copy stops at the NUL and an embedded NUL is refused
    let nul = ascii.iter().position(|b| *b == 0);
    let underscored = ascii[..nul.unwrap_or(ascii.len())].contains(&b'_');
    if underscored && nul.is_some() {
        return Err(fail());
    }
    let mut text = ascii.clone();
    if underscored {
        text.clear();
        let mut prev = 0u8;
        for &c in &ascii {
            if c == b'_' {
                if !prev.is_ascii_digit() {
                    return Err(fail());
                }
            } else {
                text.push(c);
                if prev == b'_' && !c.is_ascii_digit() {
                    return Err(fail());
                }
            }
            prev = c;
        }
        if prev == b'_' {
            return Err(fail());
        }
    }
    // `float_from_string_inner`
    let mut first = 0;
    let mut last = text.len();
    while first < last && ascii_space(text[first]) {
        first += 1;
    }
    if first == last {
        return Err(fail());
    }
    while first < last - 1 && ascii_space(text[last - 1]) {
        last -= 1;
    }
    let body = std::str::from_utf8(&text[first..last]).map_err(|_| fail())?;
    ascii_strtod(body).ok_or_else(fail)
}

/// `round(x, ndigits)` for a `float` and a small `ndigits` (`double_round`): the correctly
/// rounded decimal, ties to even on the exact binary value.
pub fn round_f64(x: f64, ndigits: usize) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let text = format!("{x:.ndigits$}");
    let rounded: f64 = text.parse().expect("formatted float parses");
    // `round` keeps the sign of a value rounded to zero
    if rounded == 0.0 {
        return 0.0f64.copysign(x);
    }
    rounded
}

/// `float.__repr__` (also `str(float)`): the shortest digits that round-trip, fixed point
/// with at least one decimal when the decimal exponent is in -4..16, otherwise `1e+16`.
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0" } else { "0.0" }.to_owned();
    }
    let sci = format!("{x:e}");
    let (sign, sci) = match sci.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", sci.as_str()),
    };
    let (mantissa, exp) = sci.split_once('e').expect("{:e} has an exponent");
    let exp: i32 = exp.parse().expect("{:e} exponent is an integer");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    if !(-4..16).contains(&exp) {
        let mut out = format!("{sign}{}", &digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let exp_sign = if exp < 0 { '-' } else { '+' };
        let _ = write!(out, "e{exp_sign}{:02}", exp.abs());
        return out;
    }
    if exp < 0 {
        let zeros = "0".repeat((-exp - 1) as usize);
        return format!("{sign}0.{zeros}{digits}");
    }
    let int_len = exp as usize + 1;
    if digits.len() <= int_len {
        let zeros = "0".repeat(int_len - digits.len());
        format!("{sign}{digits}{zeros}.0")
    } else {
        format!("{sign}{}.{}", &digits[..int_len], &digits[int_len..])
    }
}

/// `repr(s)` for a `str` (`unicode_repr`).
pub fn repr_str(s: &str) -> String {
    let squote = s.contains('\'');
    let dquote = s.contains('"');
    let quote = if squote && !dquote { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        let code = c as u32;
        if c == quote || c == '\\' {
            out.push('\\');
            out.push(c);
        } else if c == '\t' {
            out.push_str("\\t");
        } else if c == '\n' {
            out.push_str("\\n");
        } else if c == '\r' {
            out.push_str("\\r");
        } else if code < 0x20 || code == 0x7f {
            let _ = write!(out, "\\x{code:02x}");
        } else if code < 0x7f || isprintable(c) {
            out.push(c);
        } else if code <= 0xff {
            let _ = write!(out, "\\x{code:02x}");
        } else if code <= 0xffff {
            let _ = write!(out, "\\u{code:04x}");
        } else {
            let _ = write!(out, "\\U{code:08x}");
        }
    }
    out.push(quote);
    out
}

/// A Python value as JSON and gcode-metadata make them. A `dict` keeps insertion order and
/// has `str` keys only.
#[derive(Debug, Clone)]
pub enum PyValue {
    None,
    Bool(bool),
    Int(PyInt),
    Float(f64),
    Str(String),
    List(Vec<PyValue>),
    Dict(Dict),
}

/// A `dict` with `str` keys, in insertion order.
#[derive(Debug, Clone, Default)]
pub struct Dict(Vec<(String, PyValue)>);

impl Dict {
    pub fn new() -> Dict {
        Dict(Vec::new())
    }

    /// `d[key] = value`: a new key goes last, an existing one keeps its place.
    pub fn set(&mut self, key: &str, value: PyValue) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key.to_owned(), value)),
        }
    }

    pub fn get(&self, key: &str) -> Option<&PyValue> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn remove(&mut self, key: &str) -> Option<PyValue> {
        let index = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(index).1)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &PyValue)> {
        self.0.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.iter().map(|(k, _)| k)
    }

    /// `d.update(other)`
    pub fn update(&mut self, other: &Dict) {
        for (key, value) in other.iter() {
            self.set(key, value.clone());
        }
    }
}

/// `==` of two `dict`s: the same keys with equal values, in any order.
impl PartialEq for Dict {
    fn eq(&self, other: &Dict) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .all(|(k, v)| other.get(k).is_some_and(|o| o == v))
    }
}

/// The exact value of a finite `float` that is a whole number, as an `int`.
fn float_as_int(x: f64) -> Option<PyInt> {
    if x.is_finite() && x.trunc() == x {
        int_from_f64(x).ok()
    } else {
        None
    }
}

/// Python's `==` (`bool` is an `int`; `int` and `float` compare by their exact values).
impl PartialEq for PyValue {
    fn eq(&self, other: &PyValue) -> bool {
        use PyValue as V;
        let as_int = |v: &PyValue| -> Option<PyInt> {
            match v {
                V::Bool(b) => Some(PyInt::from_i64(*b as i64)),
                V::Int(i) => Some(i.clone()),
                _ => None,
            }
        };
        match (self, other) {
            (V::None, V::None) => true,
            (V::Str(a), V::Str(b)) => a == b,
            (V::List(a), V::List(b)) => a == b,
            (V::Dict(a), V::Dict(b)) => a == b,
            (V::Float(a), V::Float(b)) => a == b,
            (V::Float(f), v) | (v, V::Float(f)) => match as_int(v) {
                Some(i) => float_as_int(*f).is_some_and(|fi| fi == i),
                None => false,
            },
            (a, b) => match (as_int(a), as_int(b)) {
                (Some(x), Some(y)) => x == y,
                _ => false,
            },
        }
    }
}

impl PyValue {
    pub fn str(s: &str) -> PyValue {
        PyValue::Str(s.to_owned())
    }

    pub fn int(i: i64) -> PyValue {
        PyValue::Int(PyInt::from_i64(i))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            PyValue::None => "NoneType",
            PyValue::Bool(_) => "bool",
            PyValue::Int(_) => "int",
            PyValue::Float(_) => "float",
            PyValue::Str(_) => "str",
            PyValue::List(_) => "list",
            PyValue::Dict(_) => "dict",
        }
    }

    /// `bool(value)`
    pub fn truthy(&self) -> bool {
        match self {
            PyValue::None => false,
            PyValue::Bool(b) => *b,
            PyValue::Int(i) => !i.is_zero(),
            PyValue::Float(f) => *f != 0.0,
            PyValue::Str(s) => !s.is_empty(),
            PyValue::List(l) => !l.is_empty(),
            PyValue::Dict(d) => !d.is_empty(),
        }
    }

    /// `repr(value)`
    pub fn repr(&self) -> String {
        match self {
            PyValue::None => "None".to_owned(),
            PyValue::Bool(true) => "True".to_owned(),
            PyValue::Bool(false) => "False".to_owned(),
            PyValue::Int(i) => i.as_str().to_owned(),
            PyValue::Float(f) => float_repr(*f),
            PyValue::Str(s) => repr_str(s),
            PyValue::List(items) => {
                let parts: Vec<String> = items.iter().map(PyValue::repr).collect();
                format!("[{}]", parts.join(", "))
            }
            PyValue::Dict(d) => {
                let parts: Vec<String> = d
                    .iter()
                    .map(|(k, v)| format!("{}: {}", repr_str(k), v.repr()))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
        }
    }

    /// `str(value)`
    pub fn to_py_str(&self) -> String {
        match self {
            PyValue::Str(s) => s.clone(),
            other => other.repr(),
        }
    }

    /// `int(value)`
    pub fn to_py_int(&self) -> Result<PyInt, PyErr> {
        match self {
            PyValue::Bool(b) => Ok(PyInt::from_i64(*b as i64)),
            PyValue::Int(i) => Ok(i.clone()),
            PyValue::Float(f) => int_from_f64(*f),
            PyValue::Str(s) => int_from_str(s),
            other => Err(PyErr::TypeError(format!(
                "int() argument must be a string, a bytes-like object or a real number, not \
                 '{}'",
                other.type_name()
            ))),
        }
    }

    /// `float(value)`
    pub fn to_py_float(&self) -> Result<f64, PyErr> {
        match self {
            PyValue::Bool(b) => Ok(*b as i64 as f64),
            PyValue::Int(i) => i.to_f64(),
            PyValue::Float(f) => Ok(*f),
            PyValue::Str(s) => float_from_str(s),
            other => Err(PyErr::TypeError(format!(
                "float() argument must be a string or a real number, not '{}'",
                other.type_name()
            ))),
        }
    }

    /// `list(value)`: a `str` gives its characters, a `dict` its keys.
    pub fn to_py_list(&self) -> Result<Vec<PyValue>, PyErr> {
        match self {
            PyValue::List(items) => Ok(items.clone()),
            PyValue::Str(s) => Ok(s.chars().map(|c| PyValue::Str(c.to_string())).collect()),
            PyValue::Dict(d) => Ok(d.keys().map(|k| PyValue::Str(k.clone())).collect()),
            other => Err(PyErr::TypeError(format!(
                "'{}' object is not iterable",
                other.type_name()
            ))),
        }
    }
}

/// `sum(values)` of `float`s and `int`s, CPython 3.13's `builtin_sum_impl`: it starts from the
/// `int` 0; once a `float` is met it sums with Neumaier's compensation.
pub fn sum(values: &[PyValue]) -> Result<PyValue, PyErr> {
    let mut index = 0;
    // the int fast path: exact while the items are ints that fit in a C long
    let mut int_result: i64 = 0;
    while index < values.len() {
        let item = match &values[index] {
            PyValue::Int(i) => i.to_i64(),
            PyValue::Bool(b) => Some(*b as i64),
            _ => None,
        };
        match item.and_then(|v| int_result.checked_add(v)) {
            Some(total) => {
                int_result = total;
                index += 1;
            }
            None => break,
        }
    }
    if index == values.len() {
        return Ok(PyValue::int(int_result));
    }
    // the int path ends at a float: `result = int_result + item` (a plain `PyNumber_Add`),
    // then the float path goes on from that result
    let mut f_result = match &values[index] {
        PyValue::Float(x) => PyValue::Int(PyInt::from_i64(int_result)).to_py_float()? + *x,
        other => {
            return Err(PyErr::TypeError(format!(
                "sum of '{}' is not ported",
                other.type_name()
            )));
        }
    };
    index += 1;
    let mut c = 0.0f64;
    while index < values.len() {
        match &values[index] {
            PyValue::Float(x) => {
                let x = *x;
                // Avoid losing the sign on a negative result, and don't let adding the
                // compensation convert an infinite or overflowed sum to a NaN.
                let t = f_result + x;
                if f_result.abs() >= x.abs() {
                    c += (f_result - t) + x;
                } else {
                    c += (x - t) + f_result;
                }
                f_result = t;
            }
            // an int that fits a C long is added plainly, without the compensation
            PyValue::Int(i) if i.to_i64().is_some() => {
                f_result += i.to_i64().unwrap() as f64;
            }
            PyValue::Bool(b) => {
                f_result += *b as i64 as f64;
            }
            other => {
                return Err(PyErr::TypeError(format!(
                    "sum of a float and '{}' is not ported",
                    other.type_name()
                )));
            }
        }
        index += 1;
    }
    if c != 0.0 && c.is_finite() {
        f_result += c;
    }
    Ok(PyValue::Float(f_result))
}

// ---------------------------------------------------------------------------------------------
// json

/// The `JSONDecodeError` message: `"%s: line %d column %d (char %d)"`, on characters.
fn decode_error(msg: &str, doc: &[char], pos: usize) -> PyErr {
    let before = &doc[..pos.min(doc.len())];
    let lineno = before.iter().filter(|c| **c == '\n').count() + 1;
    let colno = match before.iter().rposition(|c| *c == '\n') {
        Some(nl) => pos - nl,
        None => pos + 1,
    };
    PyErr::ValueError(format!("{msg}: line {lineno} column {colno} (char {pos})"))
}

/// The nesting at which CPython 3.13.12 raises `RecursionError` from the top level.
pub const JSON_MAX_DEPTH: usize = 9998;

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

struct Scanner<'a> {
    s: &'a [char],
    depth: usize,
}

/// `StopIteration(idx)` of `scan_once`: `raw_decode` makes it "Expecting value".
enum ScanErr {
    Stop(usize),
    Err(PyErr),
}

impl From<PyErr> for ScanErr {
    fn from(err: PyErr) -> ScanErr {
        ScanErr::Err(err)
    }
}

impl Scanner<'_> {
    fn at(&self, i: usize) -> char {
        self.s[i]
    }

    fn hex4(&self, from: usize, err_at: usize) -> Result<u32, PyErr> {
        let mut c = 0u32;
        for i in from..from + 4 {
            let digit = self
                .at(i)
                .to_digit(16)
                .filter(|_| self.at(i).is_ascii_hexdigit());
            match digit {
                Some(d) => c = (c << 4) | d,
                None => return Err(decode_error("Invalid \\uXXXX escape", self.s, err_at)),
            }
        }
        Ok(c)
    }

    /// `scanstring_unicode(pystr, end, strict=True)`
    fn scanstring(&self, mut end: usize) -> Result<(String, usize), PyErr> {
        let len = self.s.len();
        let begin = end - 1;
        let mut out = String::new();
        loop {
            let mut next = end;
            let mut c = '\0';
            while next < len {
                c = self.at(next);
                if c == '"' || c == '\\' {
                    break;
                }
                if (c as u32) <= 0x1f {
                    return Err(decode_error("Invalid control character at", self.s, next));
                }
                next += 1;
            }
            // the scan ran off the end: the last character read is neither `"` nor `\`
            if next == len {
                return Err(decode_error(
                    "Unterminated string starting at",
                    self.s,
                    begin,
                ));
            }
            out.extend(&self.s[end..next]);
            next += 1;
            if c == '"' {
                return Ok((out, next));
            }
            if next == len {
                return Err(decode_error(
                    "Unterminated string starting at",
                    self.s,
                    begin,
                ));
            }
            let e = self.at(next);
            if e != 'u' {
                end = next + 1;
                let ch = match e {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    _ => return Err(decode_error("Invalid \\escape", self.s, end - 2)),
                };
                out.push(ch);
                continue;
            }
            next += 1;
            end = next + 4;
            if end >= len {
                return Err(decode_error("Invalid \\uXXXX escape", self.s, next - 1));
            }
            let mut code = self.hex4(next, end - 5)?;
            next = end;
            if (0xd800..=0xdbff).contains(&code)
                && end + 6 < len
                && self.at(next) == '\\'
                && self.at(next + 1) == 'u'
            {
                let low = self.hex4(next + 2, end + 6 - 5)?;
                if (0xdc00..=0xdfff).contains(&low) {
                    code = 0x10000 + (((code - 0xd800) << 10) | (low - 0xdc00));
                    end += 6;
                }
            }
            match char::from_u32(code) {
                Some(ch) => out.push(ch),
                None => {
                    return Err(PyErr::ValueError(format!(
                        "lone surrogate \\u{code:04x}: not covered by the port"
                    )));
                }
            }
        }
    }

    fn skip_ws(&self, mut idx: usize) -> usize {
        while idx < self.s.len() && is_ws(self.at(idx)) {
            idx += 1;
        }
        idx
    }

    /// `_match_number_unicode`
    fn match_number(&self, start: usize) -> Result<(PyValue, usize), ScanErr> {
        let s = self.s;
        let end_idx = s.len() as isize - 1;
        let digit = |i: usize| s[i].is_ascii_digit();
        let mut idx = start;
        if s[idx] == '-' {
            idx += 1;
            if idx as isize > end_idx {
                return Err(ScanErr::Stop(start));
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
            return Err(ScanErr::Stop(start));
        }
        let mut is_float = false;
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
        let numstr: String = s[start..idx].iter().collect();
        let value = if is_float {
            PyValue::Float(float_from_str(&numstr)?)
        } else {
            let negative = numstr.starts_with('-');
            let digits = numstr.trim_start_matches('-');
            if digits.len() > MAX_STR_DIGITS {
                return Err(ScanErr::Err(PyErr::ValueError(format!(
                    "Exceeds the limit ({MAX_STR_DIGITS} digits) for integer string \
                     conversion: value has {} digits; use sys.set_int_max_str_digits() to \
                     increase the limit",
                    digits.len()
                ))));
            }
            PyValue::Int(PyInt::from_digits(negative, digits))
        };
        Ok((value, idx))
    }

    fn matches(&self, idx: usize, word: &str) -> bool {
        let n = word.chars().count();
        idx + n <= self.s.len() && self.s[idx..idx + n].iter().copied().eq(word.chars())
    }

    /// `scan_once_unicode`
    fn scan_once(&mut self, idx: usize) -> Result<(PyValue, usize), ScanErr> {
        let s = self.s;
        if idx >= s.len() {
            return Err(ScanErr::Stop(idx));
        }
        match s[idx] {
            '"' => {
                let (text, next) = self.scanstring(idx + 1)?;
                return Ok((PyValue::Str(text), next));
            }
            '{' | '[' => {
                if self.depth >= JSON_MAX_DEPTH {
                    let what = if s[idx] == '{' { "object" } else { "array" };
                    return Err(ScanErr::Err(PyErr::RecursionError(format!(
                        "maximum recursion depth exceeded while decoding a JSON {what} from a \
                         unicode string"
                    ))));
                }
                self.depth += 1;
                let result = if s[idx] == '{' {
                    self.parse_object(idx + 1)
                } else {
                    self.parse_array(idx + 1)
                };
                self.depth -= 1;
                return result;
            }
            'n' if idx + 3 < s.len() && self.matches(idx, "null") => {
                return Ok((PyValue::None, idx + 4));
            }
            't' if idx + 3 < s.len() && self.matches(idx, "true") => {
                return Ok((PyValue::Bool(true), idx + 4));
            }
            'f' if idx + 4 < s.len() && self.matches(idx, "false") => {
                return Ok((PyValue::Bool(false), idx + 5));
            }
            'N' if idx + 2 < s.len() && self.matches(idx, "NaN") => {
                return Ok((PyValue::Float(f64::NAN), idx + 3));
            }
            'I' if idx + 7 < s.len() && self.matches(idx, "Infinity") => {
                return Ok((PyValue::Float(f64::INFINITY), idx + 8));
            }
            '-' if idx + 8 < s.len() && self.matches(idx, "-Infinity") => {
                return Ok((PyValue::Float(f64::NEG_INFINITY), idx + 9));
            }
            _ => {}
        }
        self.match_number(idx)
    }

    /// `_parse_object_unicode` (no hooks)
    fn parse_object(&mut self, idx: usize) -> Result<(PyValue, usize), ScanErr> {
        let s = self.s;
        let mut dict = Dict::new();
        let mut idx = self.skip_ws(idx);
        if idx >= s.len() || s[idx] != '}' {
            loop {
                if idx >= s.len() || s[idx] != '"' {
                    return Err(ScanErr::Err(decode_error(
                        "Expecting property name enclosed in double quotes",
                        s,
                        idx,
                    )));
                }
                let (key, next) = self.scanstring(idx + 1)?;
                idx = self.skip_ws(next);
                if idx >= s.len() || s[idx] != ':' {
                    return Err(ScanErr::Err(decode_error(
                        "Expecting ':' delimiter",
                        s,
                        idx,
                    )));
                }
                idx = self.skip_ws(idx + 1);
                let (value, next) = self.scan_once(idx)?;
                dict.set(&key, value);
                idx = self.skip_ws(next);
                if idx < s.len() && s[idx] == '}' {
                    break;
                }
                if idx >= s.len() || s[idx] != ',' {
                    return Err(ScanErr::Err(decode_error(
                        "Expecting ',' delimiter",
                        s,
                        idx,
                    )));
                }
                let comma_idx = idx;
                idx = self.skip_ws(idx + 1);
                if idx < s.len() && s[idx] == '}' {
                    return Err(ScanErr::Err(decode_error(
                        "Illegal trailing comma before end of object",
                        s,
                        comma_idx,
                    )));
                }
            }
        }
        Ok((PyValue::Dict(dict), idx + 1))
    }

    /// `_parse_array_unicode`
    fn parse_array(&mut self, idx: usize) -> Result<(PyValue, usize), ScanErr> {
        let s = self.s;
        let mut items = Vec::new();
        let mut idx = self.skip_ws(idx);
        if idx >= s.len() || s[idx] != ']' {
            loop {
                let (value, next) = self.scan_once(idx)?;
                items.push(value);
                idx = self.skip_ws(next);
                if idx < s.len() && s[idx] == ']' {
                    break;
                }
                if idx >= s.len() || s[idx] != ',' {
                    return Err(ScanErr::Err(decode_error(
                        "Expecting ',' delimiter",
                        s,
                        idx,
                    )));
                }
                let comma_idx = idx;
                idx = self.skip_ws(idx + 1);
                if idx < s.len() && s[idx] == ']' {
                    return Err(ScanErr::Err(decode_error(
                        "Illegal trailing comma before end of array",
                        s,
                        comma_idx,
                    )));
                }
            }
        }
        if idx >= s.len() || s[idx] != ']' {
            return Err(ScanErr::Err(decode_error(
                "Expecting value",
                s,
                s.len().saturating_sub(1),
            )));
        }
        Ok((PyValue::List(items), idx + 1))
    }
}

/// `json.loads(s)` for a `str`.
pub fn json_loads(s: &str) -> Result<PyValue, PyErr> {
    let chars: Vec<char> = s.chars().collect();
    if chars.first() == Some(&'\u{feff}') {
        return Err(decode_error(
            "Unexpected UTF-8 BOM (decode using utf-8-sig)",
            &chars,
            0,
        ));
    }
    let mut scanner = Scanner {
        s: &chars,
        depth: 0,
    };
    // `JSONDecoder.decode`: `raw_decode(s, idx=_w(s, 0).end())`
    let idx = scanner.skip_ws(0);
    let (value, end) = match scanner.scan_once(idx) {
        Ok(found) => found,
        Err(ScanErr::Stop(at)) => return Err(decode_error("Expecting value", &chars, at)),
        Err(ScanErr::Err(err)) => return Err(err),
    };
    let end = scanner.skip_ws(end);
    if end != chars.len() {
        return Err(decode_error("Extra data", &chars, end));
    }
    Ok(value)
}

/// `json.detect_encoding(b)`
pub fn detect_encoding(b: &[u8]) -> &'static str {
    if b.starts_with(&[0, 0, 0xfe, 0xff]) || b.starts_with(&[0xff, 0xfe, 0, 0]) {
        return "utf-32";
    }
    if b.starts_with(&[0xfe, 0xff]) || b.starts_with(&[0xff, 0xfe]) {
        return "utf-16";
    }
    if b.starts_with(&[0xef, 0xbb, 0xbf]) {
        return "utf-8-sig";
    }
    if b.len() >= 4 {
        if b[0] == 0 {
            return if b[1] != 0 { "utf-16-be" } else { "utf-32-be" };
        }
        if b[1] == 0 {
            return if b[2] != 0 || b[3] != 0 {
                "utf-16-le"
            } else {
                "utf-32-le"
            };
        }
    } else if b.len() == 2 {
        if b[0] == 0 {
            return "utf-16-be";
        }
        if b[1] == 0 {
            return "utf-16-le";
        }
    }
    "utf-8"
}

fn unicode_error(encoding: &str, reason: &str) -> PyErr {
    PyErr::ValueError(format!("'{encoding}' codec can't decode bytes: {reason}"))
}

/// `b.decode(encoding, 'surrogatepass')` for the encodings `detect_encoding` names.
pub fn decode(b: &[u8], encoding: &str) -> Result<String, PyErr> {
    let utf16 = |data: &[u8], big: bool| -> Result<String, PyErr> {
        if !data.len().is_multiple_of(2) {
            return Err(unicode_error(encoding, "truncated data"));
        }
        let units = data.chunks(2).map(|p| {
            if big {
                u16::from_be_bytes([p[0], p[1]])
            } else {
                u16::from_le_bytes([p[0], p[1]])
            }
        });
        char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .map_err(|_| unicode_error(encoding, "lone surrogate, not covered by the port"))
    };
    let utf32 = |data: &[u8], big: bool| -> Result<String, PyErr> {
        if !data.len().is_multiple_of(4) {
            return Err(unicode_error(encoding, "truncated data"));
        }
        data.chunks(4)
            .map(|q| {
                let code = if big {
                    u32::from_be_bytes([q[0], q[1], q[2], q[3]])
                } else {
                    u32::from_le_bytes([q[0], q[1], q[2], q[3]])
                };
                char::from_u32(code)
                    .ok_or_else(|| unicode_error(encoding, "code point not in range(0x110000)"))
            })
            .collect()
    };
    match encoding {
        "utf-8" => std::str::from_utf8(b)
            .map(str::to_owned)
            .map_err(|e| unicode_error(encoding, &e.to_string())),
        "utf-8-sig" => std::str::from_utf8(&b[3..])
            .map(str::to_owned)
            .map_err(|e| unicode_error(encoding, &e.to_string())),
        // the BOM picks the byte order and is dropped
        "utf-16" => {
            if b.starts_with(&[0xff, 0xfe]) {
                utf16(&b[2..], false)
            } else {
                utf16(&b[2..], true)
            }
        }
        "utf-16-le" => utf16(b, false),
        "utf-16-be" => utf16(b, true),
        "utf-32" => {
            if b.starts_with(&[0xff, 0xfe, 0, 0]) {
                utf32(&b[4..], false)
            } else {
                utf32(&b[4..], true)
            }
        }
        "utf-32-le" => utf32(b, false),
        "utf-32-be" => utf32(b, true),
        _ => unreachable!("not an encoding `detect_encoding` names"),
    }
}

/// `json.loads(b)` for `bytes`.
pub fn json_loads_bytes(b: &[u8]) -> Result<PyValue, PyErr> {
    json_loads(&decode(b, detect_encoding(b))?)
}

/// `py_encode_basestring_ascii`: `ensure_ascii=True`.
fn encode_str_ascii(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (' '..='~').contains(&c) => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

/// `float.__repr__` as `json` writes it, with `allow_nan=True`.
fn json_float(x: f64) -> String {
    if x.is_nan() {
        "NaN".to_owned()
    } else if x == f64::INFINITY {
        "Infinity".to_owned()
    } else if x == f64::NEG_INFINITY {
        "-Infinity".to_owned()
    } else {
        float_repr(x)
    }
}

fn dump(out: &mut String, value: &PyValue, indent: Option<usize>, level: usize) {
    let newline = |out: &mut String, level: usize| {
        if let Some(width) = indent {
            out.push('\n');
            out.push_str(&" ".repeat(width * level));
        }
    };
    let item_sep = if indent.is_some() { "," } else { ", " };
    match value {
        PyValue::None => out.push_str("null"),
        PyValue::Bool(true) => out.push_str("true"),
        PyValue::Bool(false) => out.push_str("false"),
        PyValue::Int(i) => out.push_str(i.as_str()),
        PyValue::Float(f) => out.push_str(&json_float(*f)),
        PyValue::Str(s) => encode_str_ascii(out, s),
        PyValue::List(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(item_sep);
                }
                newline(out, level + 1);
                dump(out, item, indent, level + 1);
            }
            newline(out, level);
            out.push(']');
        }
        PyValue::Dict(dict) => {
            if dict.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (key, item)) in dict.iter().enumerate() {
                if i > 0 {
                    out.push_str(item_sep);
                }
                newline(out, level + 1);
                encode_str_ascii(out, key);
                out.push_str(": ");
                dump(out, item, indent, level + 1);
            }
            newline(out, level);
            out.push('}');
        }
    }
}

/// `json.dumps(value)`, or `json.dump(value, file, indent=n)` for `Some(n)`: the defaults
/// otherwise (`ensure_ascii`, `allow_nan`, separators `", "` / `","` with an indent, `": "`).
pub fn json_dumps(value: &PyValue, indent: Option<usize>) -> String {
    let mut out = String::new();
    dump(&mut out, value, indent, 0);
    out
}

/// The CPython the character tables were generated from.
pub fn py_unicode_python_version() -> &'static str {
    py_unicode::PYTHON_VERSION
}
