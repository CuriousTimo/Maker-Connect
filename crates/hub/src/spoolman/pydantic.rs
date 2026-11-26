//! How Spoolman checks a request body before its handler runs: FastAPI 0.162.0 reads the body
//! (`fastapi/routing.py` `get_request_handler`: JSON when the `content-type` is
//! `application/json` or `application/*+json`, else the bytes; `request_body_to_args`,
//! `_validate_value_with_model_field`) and Pydantic 2.14.2 validates it with
//! `validate_python(body, from_attributes=True)` in lax mode, its errors answered 422
//! `{"detail": [...]}` (`fastapi/exception_handlers.py`).
//!
//! The validators are pydantic-core 2.50.2's (MIT, `reference/pydantic-2.14.2/pydantic-core`),
//! ported for the schema kinds Spoolman's body models use, from Python input
//! (`src/input/input_python.rs`, `src/input/shared.rs`, `src/validators/{model,model_fields,
//! with_default,nullable,function,string,int,float,bool,datetime,dict,list,enum_}.rs`), with the
//! messages of `src/errors/types.rs`. Its own crates are used as they are: jiter (`NumberInt`)
//! and speedate (datetimes), at pydantic-core's versions.
//!
//! The models are Spoolman's own: `models.json`, each route's body model as its
//! `__pydantic_core_schema__`, written by `scripts/spoolman-audit-check.py`. Spoolman's own
//! validators in them (`function-after`) are ported by name in `spoolman_function`.

// A line error carries its input, as pydantic-core's does; a 422 is the response itself.
#![allow(clippy::result_large_err)]

use super::python_json::{self, LoadError, Py};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use std::sync::LazyLock;
use strum::EnumMessage;

static MODELS: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("models.json")).expect("models.json is JSON")
});

/// One Pydantic line error: `type`, `loc` (inside the body), `msg`, `input`, `ctx`.
#[derive(Debug, Clone)]
pub struct LineError {
    kind: String,
    loc: Vec<Value>,
    msg: String,
    input: Py,
    ctx: Option<Value>,
}

impl LineError {
    fn new(kind: &str, msg: impl Into<String>, input: &Py) -> Self {
        LineError {
            kind: kind.into(),
            loc: Vec::new(),
            msg: msg.into(),
            input: input.clone(),
            ctx: None,
        }
    }

    fn ctx(mut self, ctx: Value) -> Self {
        self.ctx = Some(ctx);
        self
    }

    fn outer(mut self, item: Value) -> Self {
        self.loc.insert(0, item);
        self
    }
}

type Errors = Vec<LineError>;

/// A Python value as FastAPI's `jsonable_encoder` makes it, for an error's `input`: bytes
/// decoded. `None`: a float that is not finite, which Starlette's `JSONResponse`
/// (`allow_nan=False`) cannot render.
fn encode(v: &Py) -> Option<Value> {
    Some(match v {
        Py::None => Value::Null,
        Py::Bool(b) => Value::Bool(*b),
        Py::Int(i) => int_value(*i),
        Py::BigInt(s) => serde_json::from_str(s).ok()?,
        Py::Float(f) => Value::from(serde_json::Number::from_f64(*f)?),
        Py::Str(s) => Value::String(s.clone()),
        Py::Bytes(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
        Py::List(items) => Value::Array(items.iter().map(encode).collect::<Option<_>>()?),
        Py::Dict(pairs) => Value::Object(
            pairs
                .iter()
                .map(|(k, v)| Some((k.clone(), encode(v)?)))
                .collect::<Option<Map<_, _>>>()?,
        ),
    })
}

fn int_value(i: i128) -> Value {
    i64::try_from(i)
        .map(Value::from)
        .or_else(|_| u64::try_from(i).map(Value::from))
        .unwrap_or_else(|_| serde_json::from_str(&i.to_string()).unwrap_or(Value::Null))
}

/// A float as Python's `repr`, for the `ctx` of a bound (`0.0`) and for a message (Rust's
/// `Display`, as pydantic-core renders it: `0`).
fn float_value(f: f64) -> Value {
    serde_json::Number::from_f64(f).map_or(Value::Null, Value::from)
}

// --- Input (input_python.rs, shared.rs) ---

/// `maybe_as_string`: a `str`, or `bytes` as UTF-8 (`unicode_error` when not).
fn maybe_as_string<'a>(v: &'a Py, unicode_error: &str) -> Result<Option<&'a str>, LineError> {
    match v {
        Py::Str(s) => Ok(Some(s)),
        Py::Bytes(b) => match std::str::from_utf8(b) {
            Ok(s) => Ok(Some(s)),
            Err(_) => Err(unicode_error_of(unicode_error, v)),
        },
        _ => Ok(None),
    }
}

fn unicode_error_of(kind: &str, v: &Py) -> LineError {
    match kind {
        "int_parsing" => LineError::new(kind, INT_PARSING, v),
        "float_parsing" => LineError::new(kind, FLOAT_PARSING, v),
        "bool_parsing" => LineError::new(kind, BOOL_PARSING, v),
        _ => LineError::new(
            "string_unicode",
            "Input should be a valid string, unable to parse raw data as a unicode string",
            v,
        ),
    }
}

const INT_PARSING: &str = "Input should be a valid integer, unable to parse string as an integer";
const FLOAT_PARSING: &str = "Input should be a valid number, unable to parse string as a number";
const BOOL_PARSING: &str = "Input should be a valid boolean, unable to interpret input";

/// `validate_str` (lax, `coerce_numbers_to_str` off).
fn validate_str(v: &Py) -> Result<String, LineError> {
    match v {
        Py::Str(s) => Ok(s.clone()),
        Py::Bytes(b) => std::str::from_utf8(b)
            .map(str::to_owned)
            .map_err(|_| unicode_error_of("string_unicode", v)),
        _ => Err(LineError::new(
            "string_type",
            "Input should be a valid string",
            v,
        )),
    }
}

/// `str_as_bool`.
fn str_as_bool(v: &Py, s: &str) -> Result<bool, LineError> {
    let is = |w: &str| s.eq_ignore_ascii_case(w);
    if s == "0" || is("f") || is("n") || is("no") || is("off") || is("false") {
        Ok(false)
    } else if s == "1" || is("t") || is("y") || is("on") || is("yes") || is("true") {
        Ok(true)
    } else {
        Err(LineError::new("bool_parsing", BOOL_PARSING, v))
    }
}

/// `validate_bool` (lax).
fn validate_bool(v: &Py) -> Result<bool, LineError> {
    if let Py::Bool(b) = v {
        return Ok(*b);
    }
    if let Some(s) = maybe_as_string(v, "bool_parsing")? {
        return str_as_bool(v, s);
    }
    match v {
        // `int_as_bool` of an `i64`; past it, the float of it is no `i64` either: `bool_type`.
        Py::Int(i) if i64::try_from(*i).is_ok() => match i {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(LineError::new("bool_parsing", BOOL_PARSING, v)),
        },
        Py::Float(f) => match float_as_int(v, *f) {
            Ok(0) => Ok(false),
            Ok(1) => Ok(true),
            Ok(_) => Err(LineError::new("bool_parsing", BOOL_PARSING, v)),
            Err(_) => Err(LineError::new(
                "bool_type",
                "Input should be a valid boolean",
                v,
            )),
        },
        _ => Err(LineError::new(
            "bool_type",
            "Input should be a valid boolean",
            v,
        )),
    }
}

/// A Python `int` that passed: `i64` or past it (digits).
#[derive(Clone, Debug, PartialEq)]
enum Int {
    Small(i128),
    Big(String),
}

impl Int {
    fn as_f64(&self) -> f64 {
        match self {
            Int::Small(i) => *i as f64,
            Int::Big(s) => s.parse().unwrap_or(f64::NAN),
        }
    }
}

/// `str_as_int`: jiter's `NumberInt` of the string, else of `clean_int_str`.
fn str_as_int(v: &Py, s: &str) -> Result<Int, LineError> {
    let number = |b: &[u8]| jiter::NumberInt::try_from(b);
    match number(s.as_bytes()) {
        Ok(n) => return Ok(from_number_int(n)),
        Err(e) if e.error_type == jiter::JsonErrorType::NumberOutOfRange => {
            return Err(LineError::new(
                "int_parsing_size",
                "Unable to parse input string as an integer, exceeded maximum size",
                v,
            ));
        }
        Err(_) => {}
    }
    match clean_int_str(s) {
        Some(cleaned) => number(cleaned.as_bytes())
            .map(from_number_int)
            .map_err(|_| LineError::new("int_parsing", INT_PARSING, v)),
        None => Err(LineError::new("int_parsing", INT_PARSING, v)),
    }
}

fn from_number_int(n: jiter::NumberInt) -> Int {
    match n {
        jiter::NumberInt::Int(i) => Int::Small(i as i128),
        jiter::NumberInt::BigInt(b) => match b.to_string().parse::<i128>() {
            Ok(i) => Int::Small(i),
            Err(_) => Int::Big(b.to_string()),
        },
    }
}

/// `strip_underscores`.
fn strip_underscores(s: &str) -> Option<String> {
    if s.starts_with('_') || s.ends_with('_') || !s.contains('_') || s.contains("__") {
        None
    } else {
        Some(s.replace('_', ""))
    }
}

/// `clean_int_str`.
fn clean_int_str(s: &str) -> Option<String> {
    let len_before = s.len();
    let mut s = s.trim();
    if let Some(suffix) = s.strip_prefix('+') {
        if suffix.starts_with('-') {
            return None;
        }
        s = suffix;
    }
    let mut is_negative = false;
    if let Some(suffix) = s.strip_prefix('-') {
        if suffix.starts_with('-') | suffix.starts_with('+') {
            return None;
        }
        is_negative = true;
        s = suffix;
    }
    s = strip_leading_zeros(s)?;
    if let Some(i) = s.find('.') {
        let decimal_part = &s[i + 1..];
        if !decimal_part.is_empty() && decimal_part.chars().all(|c| c == '0') {
            s = &s[..i];
        }
    }
    if let Some(stripped) = strip_underscores(s) {
        return Some(if is_negative {
            format!("-{stripped}")
        } else {
            stripped
        });
    }
    if len_before == s.len() {
        return None;
    }
    Some(if is_negative {
        format!("-{s}")
    } else {
        s.to_owned()
    })
}

/// `strip_leading_zeros`.
fn strip_leading_zeros(s: &str) -> Option<&str> {
    let mut chars = s.char_indices();
    match chars.next() {
        Some((_, '0')) => {}
        Some((_, c)) if ('1'..='9').contains(&c) || c == '-' => return Some(s),
        _ => return None,
    }
    for (i, c) in chars {
        match c {
            '0' | '_' => {}
            '1'..='9' | '-' => return Some(&s[i..]),
            '.' => return Some(&s[(i - 1)..]),
            _ => return None,
        }
    }
    Some(&s[s.len() - 1..])
}

/// `float_as_int`.
fn float_as_int(v: &Py, f: f64) -> Result<i128, LineError> {
    if f.is_infinite() || f.is_nan() {
        Err(LineError::new(
            "finite_number",
            "Input should be a finite number",
            v,
        ))
    } else if f % 1.0 != 0.0 {
        Err(LineError::new(
            "int_from_float",
            "Input should be a valid integer, got a number with a fractional part",
            v,
        ))
    } else if (i64::MIN as f64) < f && f < (i64::MAX as f64) {
        Ok(f as i64 as i128)
    } else {
        Err(LineError::new(
            "int_parsing_size",
            "Unable to parse input string as an integer, exceeded maximum size",
            v,
        ))
    }
}

/// A path parameter's `int`, as Pydantic takes one from its string: `None` when it cannot.
/// Not covered: an int past `i64`, which Pydantic takes and the hub's ids cannot hold (`None`).
pub fn str_int(s: &str) -> Option<i64> {
    match validate_int(&Py::Str(s.into())) {
        Ok(Int::Small(i)) => i64::try_from(i).ok(),
        _ => None,
    }
}

/// `validate_int` (lax).
fn validate_int(v: &Py) -> Result<Int, LineError> {
    match v {
        Py::Int(i) => return Ok(Int::Small(*i)),
        Py::BigInt(s) => return Ok(Int::Big(s.clone())),
        Py::Bool(b) => return Ok(Int::Small(*b as i128)),
        _ => {}
    }
    if let Some(s) = maybe_as_string(v, "int_parsing")? {
        return str_as_int(v, s);
    }
    if let Py::Float(f) = v {
        return float_as_int(v, *f).map(Int::Small);
    }
    Err(LineError::new(
        "int_type",
        "Input should be a valid integer",
        v,
    ))
}

/// `str_as_float`: Rust's `f64` parse of the trimmed string, else with underscores stripped.
fn str_as_float(v: &Py, s: &str) -> Result<f64, LineError> {
    match s.trim().parse() {
        Ok(f) => Ok(f),
        Err(_) => strip_underscores(s)
            .and_then(|t| t.parse().ok())
            .ok_or_else(|| LineError::new("float_parsing", FLOAT_PARSING, v)),
    }
}

/// `validate_float` (lax).
fn validate_float(v: &Py) -> Result<f64, LineError> {
    match v {
        Py::Float(f) => return Ok(*f),
        Py::Int(i) => return Ok(*i as f64),
        Py::BigInt(s) => return Ok(s.parse().unwrap_or(f64::INFINITY)),
        Py::Bool(b) => return Ok(*b as u8 as f64),
        _ => {}
    }
    if let Some(s) = maybe_as_string(v, "float_parsing")? {
        return str_as_float(v, s);
    }
    Err(LineError::new(
        "float_type",
        "Input should be a valid number",
        v,
    ))
}

// --- Validators ---

fn plural(n: u64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// `StrConstrainedValidator`: length (characters), then `pattern` (a search).
fn validate_constrained_str(schema: &Value, v: &Py) -> Result<Py, LineError> {
    let s = validate_str(v)?;
    let n = s.chars().count() as u64;
    if let Some(min) = schema["min_length"].as_u64()
        && n < min
    {
        return Err(LineError::new(
            "string_too_short",
            format!("String should have at least {min} character{}", plural(min)),
            v,
        )
        .ctx(json!({"min_length": min})));
    }
    if let Some(max) = schema["max_length"].as_u64()
        && n > max
    {
        return Err(LineError::new(
            "string_too_long",
            format!("String should have at most {max} character{}", plural(max)),
            v,
        )
        .ctx(json!({"max_length": max})));
    }
    if let Some(p) = schema["pattern"].as_str()
        && !regex::Regex::new(p).is_ok_and(|re| re.is_match(&s))
    {
        return Err(LineError::new(
            "string_pattern_mismatch",
            format!("String should match pattern '{p}'"),
            v,
        )
        .ctx(json!({"pattern": p})));
    }
    Ok(Py::Str(s))
}

/// The bounds of an `int` or `float`, in pydantic-core's order: `le`, `lt`, `ge`, `gt`.
fn check_bounds(
    schema: &Value,
    x: f64,
    v: &Py,
    render: impl Fn(&Value) -> (String, Value),
) -> Result<(), LineError> {
    use std::cmp::Ordering::*;
    for (key, kind, words, ok) in [
        (
            "le",
            "less_than_equal",
            "less than or equal to",
            &[Less, Equal][..],
        ),
        ("lt", "less_than", "less than", &[Less][..]),
        (
            "ge",
            "greater_than_equal",
            "greater than or equal to",
            &[Greater, Equal][..],
        ),
        ("gt", "greater_than", "greater than", &[Greater][..]),
    ] {
        let bound = &schema[key];
        let Some(b) = bound.as_f64() else { continue };
        if !x.partial_cmp(&b).is_some_and(|o| ok.contains(&o)) {
            let (shown, ctx) = render(bound);
            return Err(
                LineError::new(kind, format!("Input should be {words} {shown}"), v)
                    .ctx(json!({ key: ctx })),
            );
        }
    }
    Ok(())
}

/// `ConstrainedIntValidator`.
fn validate_constrained_int(schema: &Value, v: &Py) -> Result<Py, LineError> {
    let i = validate_int(v)?;
    // An int bound stays an int: its message and `ctx` show it so.
    check_bounds(schema, i.as_f64(), v, |b| (b.to_string(), b.clone()))?;
    Ok(match i {
        Int::Small(i) => Py::Int(i),
        Int::Big(s) => Py::BigInt(s),
    })
}

/// `ConstrainedFloatValidator` (`allow_inf_nan` on).
fn validate_constrained_float(schema: &Value, v: &Py) -> Result<Py, LineError> {
    let f = validate_float(v)?;
    // A float bound: its message as Rust renders an f64 (`0`), its `ctx` as Python's float (`0.0`).
    check_bounds(schema, f, v, |b| {
        let f = b.as_f64().unwrap_or_default();
        (f.to_string(), float_value(f))
    })?;
    Ok(Py::Float(f))
}

/// `DateTimeValidator` (lax): a datetime from a string or a number, else a date at 00:00.
fn validate_datetime(v: &Py) -> Result<Py, LineError> {
    use speedate::{DateTime, DateTimeConfig, MicrosecondsPrecisionOverflowBehavior, TimeConfig};
    let config = || DateTimeConfig {
        time_config: TimeConfig {
            microseconds_precision_overflow_behavior:
                MicrosecondsPrecisionOverflowBehavior::Truncate,
            unix_timestamp_offset: Some(0),
        },
        timestamp_unit: speedate::TimestampUnit::Infer,
    };
    let parsing = |e: speedate::ParseError| {
        let doc = e.get_documentation().unwrap_or_default();
        LineError::new(
            "datetime_parsing",
            format!("Input should be a valid datetime, {doc}"),
            v,
        )
        .ctx(json!({"error": doc}))
    };
    let parsed = match v {
        Py::Str(s) => DateTime::parse_bytes_with_config(s.as_bytes(), &config()).map_err(parsing),
        Py::Bytes(b) => DateTime::parse_bytes_with_config(b, &config()).map_err(parsing),
        Py::Bool(_) => return Err(datetime_type(v)),
        Py::Int(i) => match i64::try_from(*i) {
            Ok(i) => DateTime::from_timestamp_with_config(i, 0, &config()).map_err(parsing),
            Err(_) => DateTime::from_float_with_config(*i as f64, &config()).map_err(parsing),
        },
        Py::Float(f) => {
            if f.is_nan() {
                return Err(LineError::new(
                    "datetime_parsing",
                    "Input should be a valid datetime, NaN values not permitted",
                    v,
                )
                .ctx(json!({"error": "NaN values not permitted"})));
            }
            DateTime::from_float_with_config(*f, &config()).map_err(parsing)
        }
        _ => return Err(datetime_type(v)),
    };
    match parsed {
        Ok(dt) => Ok(Py::Str(dt.to_string())),
        // A parsing error, in lax mode: a date and the time 00:00:00 (`datetime_from_date`).
        Err(parse_error) => {
            let bytes: &[u8] = match v {
                Py::Str(s) => s.as_bytes(),
                Py::Bytes(b) => b,
                _ => return Err(parse_error),
            };
            let config = speedate::DateConfig::builder()
                .timestamp_unit(speedate::TimestampUnit::Infer)
                .build();
            match speedate::Date::parse_bytes_with_config(bytes, &config) {
                Ok(date) => Ok(Py::Str(format!("{date}T00:00:00"))),
                Err(e) => {
                    let doc = e.get_documentation().unwrap_or_default();
                    Err(LineError::new(
                        "datetime_from_date_parsing",
                        format!("Input should be a valid datetime or date, {doc}"),
                        v,
                    )
                    .ctx(json!({"error": doc})))
                }
            }
        }
    }
}

fn datetime_type(v: &Py) -> LineError {
    LineError::new("datetime_type", "Input should be a valid datetime", v)
}

/// Python's `repr` of a `str` of ASCII (`unicode_repr`, `Objects/unicodeobject.c`): in `'`,
/// or in `"` when it holds a `'` and no `"`; `\\`, the quote, `\t`, `\n`, `\r` escaped, other
/// control characters as `\xhh`. Not covered: a string past ASCII (Python's printable test).
pub fn ascii_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `expected_repr`: `'a'`, `'a' or 'b'`, `'a', 'b' or 'c'`.
fn expected_repr(values: &[String]) -> String {
    let reprs: Vec<String> = values.iter().map(|v| ascii_repr(v)).collect();
    match reprs.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// `EnumValidator` of a plain `Enum` with `str` values: a value equal to a member's.
fn validate_enum(schema: &Value, v: &Py) -> Result<Py, LineError> {
    let values: Vec<String> = schema["values"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str().map(str::to_owned))
        .collect();
    if let Py::Str(s) = v
        && values.contains(s)
    {
        return Ok(Py::Str(s.clone()));
    }
    let expected = expected_repr(&values);
    Err(
        LineError::new("enum", format!("Input should be {expected}"), v)
            .ctx(json!({"expected": expected})),
    )
}

/// `DictValidator` (lax): every key and value; a key's errors at `[key, "[key]"]`.
fn validate_dict(schema: &Value, v: &Py) -> Result<Py, Errors> {
    let Py::Dict(pairs) = v else {
        return Err(vec![LineError::new(
            "dict_type",
            "Input should be a valid dictionary",
            v,
        )]);
    };
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (key, value) in pairs {
        let k = Py::Str(key.clone());
        let ok_key = match validate(&schema["keys_schema"], &k) {
            Ok(Py::Str(s)) => Some(s),
            Ok(_) => None,
            Err(es) => {
                errors.extend(
                    es.into_iter()
                        .map(|e| e.outer(json!("[key]")).outer(json!(key))),
                );
                None
            }
        };
        match validate(&schema["values_schema"], value) {
            Ok(val) => {
                if let Some(k) = ok_key {
                    out.push((k, val));
                }
            }
            Err(es) => errors.extend(es.into_iter().map(|e| e.outer(json!(key)))),
        }
    }
    if errors.is_empty() {
        Ok(Py::Dict(out))
    } else {
        Err(errors)
    }
}

/// `ListValidator` (a Python `list`): its length, then every item at its index.
fn validate_list(schema: &Value, v: &Py) -> Result<Py, Errors> {
    let Py::List(items) = v else {
        return Err(vec![LineError::new(
            "list_type",
            "Input should be a valid list",
            v,
        )]);
    };
    if let Some(min) = schema["min_length"].as_u64()
        && (items.len() as u64) < min
    {
        let n = items.len();
        return Err(vec![
            LineError::new(
                "too_short",
                format!(
                    "List should have at least {min} item{} after validation, not {n}",
                    plural(min)
                ),
                v,
            )
            .ctx(json!({"field_type": "List", "min_length": min, "actual_length": n})),
        ]);
    }
    if let Some(max) = schema["max_length"].as_u64()
        && (items.len() as u64) > max
    {
        let n = items.len();
        return Err(vec![
            LineError::new(
                "too_long",
                format!(
                    "List should have at most {max} item{} after validation, not {n}",
                    plural(max)
                ),
                v,
            )
            .ctx(json!({"field_type": "List", "max_length": max, "actual_length": n})),
        ]);
    }
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match validate(&schema["items_schema"], item) {
            Ok(x) => out.push(x),
            Err(es) => errors.extend(es.into_iter().map(|e| e.outer(json!(i)))),
        }
    }
    if errors.is_empty() {
        Ok(Py::List(out))
    } else {
        Err(errors)
    }
}

/// `ModelFieldsValidator`, from a dict (`from_attributes` on: a builtin that is not a dict is
/// `model_attributes_type`): every field in order; one left out takes its default or is
/// `missing` (its input the whole dict); keys that are no field are ignored. The result holds
/// the fields that were given (Pydantic's `model_fields_set`), in the model's order.
fn validate_model_fields(schema: &Value, v: &Py) -> Result<Py, Errors> {
    let Py::Dict(_) = v else {
        return Err(vec![LineError::new(
            "model_attributes_type",
            "Input should be a valid dictionary or object to extract fields from",
            v,
        )]);
    };
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (name, field) in schema["fields"].as_object().into_iter().flatten() {
        let field_schema = &field["schema"];
        match v.get(name) {
            None if field_schema["type"] == "default" => {}
            None => errors.push(LineError::new("missing", "Field required", v).outer(json!(name))),
            Some(value) => match validate(field_schema, value) {
                Ok(x) => out.push((name.clone(), x)),
                Err(es) => errors.extend(es.into_iter().map(|e| e.outer(json!(name)))),
            },
        }
    }
    if errors.is_empty() {
        Ok(Py::Dict(out))
    } else {
        Err(errors)
    }
}

/// One schema node on one input.
fn validate(schema: &Value, v: &Py) -> Result<Py, Errors> {
    let one = |r: Result<Py, LineError>| r.map_err(|e| vec![e]);
    match schema["type"].as_str().unwrap_or_default() {
        "model" => validate(&schema["schema"], v),
        "model-fields" => validate_model_fields(schema, v),
        "default" => validate(&schema["schema"], v),
        "nullable" => match v {
            Py::None => Ok(Py::None),
            _ => validate(&schema["schema"], v),
        },
        "function-after" => {
            let value = validate(&schema["schema"], v)?;
            let name = schema["function"]["name"].as_str().unwrap_or_default();
            spoolman_function(name, value).map_err(|msg| {
                vec![
                    LineError::new("value_error", format!("Value error, {msg}"), v)
                        .ctx(json!({"error": {}})),
                ]
            })
        }
        "str" => one(validate_constrained_str(schema, v)),
        "int" => one(validate_constrained_int(schema, v)),
        "float" => one(validate_constrained_float(schema, v)),
        "bool" => one(validate_bool(v).map(Py::Bool)),
        "datetime" => one(validate_datetime(v)),
        "enum" => one(validate_enum(schema, v)),
        "dict" => validate_dict(schema, v),
        "list" => validate_list(schema, v),
        other => unreachable!("models.json has a schema kind not ported: {other}"),
    }
}

// --- Spoolman's own validators (spoolman/api/v1/{filament,vendor,spool}.py) ---

/// The `color_hex` check: upper case, a `#` taken off, hex digits only, 6 or 8 of them.
fn color_code(clr_raw: &str) -> Result<(), &'static str> {
    let clr = clr_raw.to_uppercase();
    let clr = clr.strip_prefix('#').unwrap_or(&clr);
    if clr.chars().any(|c| !"0123456789ABCDEF".contains(c)) {
        return Err("Invalid character in color code.");
    }
    if !matches!(clr.chars().count(), 6 | 8) {
        return Err("Color code must be 6 or 8 characters long.");
    }
    Ok(())
}

/// Python's truthiness of a validated value (`if not v`).
fn truthy(v: &Py) -> bool {
    match v {
        Py::None => false,
        Py::Str(s) => !s.is_empty(),
        Py::Bytes(b) => !b.is_empty(),
        Py::Bool(b) => *b,
        Py::Int(i) => *i != 0,
        Py::BigInt(_) => true,
        Py::Float(f) => *f != 0.0,
        Py::List(a) => !a.is_empty(),
        Py::Dict(o) => !o.is_empty(),
    }
}

fn as_str(v: &Py) -> Option<&str> {
    match v {
        Py::Str(s) => Some(s),
        _ => None,
    }
}

/// A `function-after` of Spoolman's, by its qualified name: `Err` is its `ValueError`.
fn spoolman_function(name: &str, v: Py) -> Result<Py, String> {
    match name {
        "FilamentParameters.color_hex_validator" => {
            let Some(s) = as_str(&v).filter(|s| !s.is_empty()) else {
                return Ok(Py::None);
            };
            color_code(s)?;
            Ok(Py::Str(s.strip_prefix('#').unwrap_or(s).to_owned()))
        }
        "FilamentParameters.multi_color_hexes_validator" => {
            let Some(s) = as_str(&v).filter(|s| !s.is_empty()) else {
                return Ok(Py::None);
            };
            let mut colors = Vec::new();
            for clr_raw in s.split(',') {
                color_code(clr_raw)?;
                colors.push(clr_raw.strip_prefix('#').unwrap_or(clr_raw));
            }
            Ok(Py::Str(colors.join(",")))
        }
        "FilamentParameters.validate" => {
            let field = |k: &str| v.get(k).cloned().unwrap_or(Py::None);
            let (hex, hexes, direction) = (
                field("color_hex"),
                field("multi_color_hexes"),
                field("multi_color_direction"),
            );
            if truthy(&hex) && truthy(&hexes) {
                return Err("Cannot specify both color_hex and multi_color_hexes.".into());
            }
            if truthy(&hexes) && as_str(&hexes).is_some_and(|s| s.split(',').count() < 2) {
                return Err("Must specify at least two colors in multi_color_hexes.".into());
            }
            if truthy(&hexes) && !truthy(&direction) {
                return Err("Multi-color filament must have multi_color_direction set.".into());
            }
            if !truthy(&hexes) && truthy(&direction) {
                return Err(
                    "Single-color filament must not have multi_color_direction set.".into(),
                );
            }
            Ok(v)
        }
        "FilamentUpdateParameters.prevent_none"
        | "VendorUpdateParameters.prevent_none"
        | "SpoolUpdateParameters.prevent_none" => {
            if v == Py::None {
                return Err("Value must not be None.".into());
            }
            Ok(v)
        }
        other => unreachable!("a Spoolman validator not ported: {other}"),
    }
}

// --- FastAPI ---

fn refuse(errors: Errors) -> Response {
    let detail: Option<Vec<Value>> = errors
        .into_iter()
        .map(|e| {
            let mut loc = vec![json!("body")];
            loc.extend(e.loc);
            let mut out =
                json!({"type": e.kind, "loc": loc, "msg": e.msg, "input": encode(&e.input)?});
            if let Some(ctx) = e.ctx {
                out["ctx"] = ctx;
            }
            Some(out)
        })
        .collect();
    match detail {
        Some(detail) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(json!({"detail": detail})),
        )
            .into_response(),
        // Starlette's `JSONResponse` raises on a NaN: uvicorn's 500.
        None => (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response(),
    }
}

/// A route's path with its parameters' names left out: `/vendor/{}`.
fn shape(path: &str) -> String {
    path.split('/')
        .map(|seg| {
            if seg.starts_with('{') && seg.ends_with('}') {
                "{}"
            } else {
                seg
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The body model of a route (its path as Spoolman's or as the hub's, under
/// [`super::routing::MOUNT`] or not; parameter names do not matter): `models.json`'s `bodies`.
fn model_of(method: &str, path: &str) -> Option<&'static Value> {
    let path = shape(path.strip_prefix(super::routing::MOUNT).unwrap_or(path));
    let bodies = MODELS["bodies"].as_array()?;
    let body = bodies.iter().find(|b| {
        b["path"].as_str().map(shape).as_deref() == Some(path.as_str())
            && b["methods"]
                .as_array()
                .is_some_and(|m| m.iter().any(|x| x == method))
    })?;
    MODELS["models"].get(body["model"].as_str()?)
}

/// A Spoolman route's body, read and checked as FastAPI does ([`body`]): the extractor for the
/// hub's handlers, the route's model found from its matched path.
pub struct Body(pub Value);

impl<S: Send + Sync> axum::extract::FromRequest<S> for Body {
    type Rejection = Response;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Response> {
        let method = req.method().as_str().to_owned();
        let path = req
            .extensions()
            .get::<axum::extract::MatchedPath>()
            .map(|m| m.as_str().to_owned())
            .unwrap_or_default();
        let headers = req.headers().clone();
        let bytes = axum::body::Bytes::from_request(req, state)
            .await
            .map_err(IntoResponse::into_response)?;
        body(&method, &path, &headers, &bytes).map(Body)
    }
}

/// FastAPI's reading and checking of a route's body (`path` as Spoolman declares it, e.g.
/// `/vendor/{vendor_id}`): the validated body (the fields given, coerced), or its answer.
pub fn body(
    method: &str,
    path: &str,
    headers: &HeaderMap,
    bytes: &[u8],
) -> Result<Value, Response> {
    let input = read(headers, bytes)?;
    check(method, path, input)
}

/// FastAPI's reading of the body (`get_request_handler`), before the route's dependencies:
/// `None` for no body, the bytes when the `content-type` is not JSON.
pub fn read(headers: &HeaderMap, bytes: &[u8]) -> Result<Option<Py>, Response> {
    if bytes.is_empty() {
        return Ok(None);
    }
    let json = match headers.get("content-type").and_then(|v| v.to_str().ok()) {
        // `strict_content_type` on: no content type, no JSON.
        None => false,
        Some(ct) => is_json(ct),
    };
    if !json {
        return Ok(Some(Py::Bytes(bytes.to_vec())));
    }
    match python_json::loads(bytes) {
        Ok(py) => Ok(Some(py)),
        Err(LoadError::Decode(e)) => {
            let detail = json!([{
                "type": "json_invalid", "loc": ["body", e.pos], "msg": "JSON decode error",
                "input": {}, "ctx": {"error": e.msg},
            }]);
            Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(json!({"detail": detail})),
            )
                .into_response())
        }
        Err(LoadError::Unicode) => Err((
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"detail": "There was an error parsing the body"})),
        )
            .into_response()),
    }
}

/// `request_body_to_args`: the body read on the route's model.
pub fn check(method: &str, path: &str, input: Option<Py>) -> Result<Value, Response> {
    let schema =
        model_of(method, path).unwrap_or_else(|| unreachable!("no body model for {method} {path}"));
    // `_validate_value_with_model_field`: no body, a required model is `missing` at `body`.
    let Some(input) = input else {
        return Err(refuse(vec![LineError::new(
            "missing",
            "Field required",
            &Py::None,
        )]));
    };
    validate(schema, &input)
        .map(|py| to_value(&py))
        .map_err(refuse)
}

/// The validated body for the hub's handler: a float that is not finite as `null` (not
/// covered: what Spoolman's handler then does with one).
fn to_value(v: &Py) -> Value {
    match v {
        Py::Float(f) if !f.is_finite() => Value::Null,
        Py::List(items) => Value::Array(items.iter().map(to_value).collect()),
        Py::Dict(pairs) => Value::Object(
            pairs
                .iter()
                .map(|(k, v)| (k.clone(), to_value(v)))
                .collect(),
        ),
        v => encode(v).unwrap_or(Value::Null),
    }
}

/// `email.message.Message`'s content type: maintype `application`, subtype `json` or `*+json`.
fn is_json(content_type: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    // `get_content_type` falls back to `text/plain` when the value is not `type/subtype`.
    let Some((main, sub)) = mime.split_once('/') else {
        return false;
    };
    if sub.contains('/') {
        return false;
    }
    main == "application" && (sub == "json" || sub.ends_with("+json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_as_python_reprs_them() {
        // Measured on Spoolman's venv: repr(s).
        for (s, want) in [
            ("text/plain", "'text/plain'"),
            ("it's", "\"it's\""),
            ("a\"b'c", "'a\"b\\'c'"),
            ("a\\b", "'a\\\\b'"),
            ("tab\there", "'tab\\there'"),
            ("", "''"),
            ("x\u{1}y", "'x\\x01y'"),
        ] {
            assert_eq!(ascii_repr(s), want, "{s:?}");
        }
    }

    #[test]
    fn ints_as_pydantic_parses_them() {
        let int = |s: &str| validate_int(&Py::Str(s.into())).ok();
        for (s, want) in [
            ("1", Some(1)),
            ("+1", Some(1)),
            (" 1 ", Some(1)),
            ("1_000", Some(1000)),
            ("1.0", Some(1)),
            ("01", Some(1)),
            ("-0", Some(0)),
            ("1.5", None),
            ("0x10", None),
            ("1e3", None),
            ("", None),
            ("1__0", None),
        ] {
            assert_eq!(int(s), want.map(Int::Small), "{s:?}");
        }
    }

    /// Every case of `pydantic-corpus.json` (FastAPI and Pydantic on Spoolman's routes, run by
    /// `scripts/spoolman-audit-check.py`): the same answer, or the same body for the handler.
    #[tokio::test]
    async fn bodies_as_fastapi_checks_them() {
        let corpus: Value = serde_json::from_str(include_str!("pydantic-corpus.json")).unwrap();
        let cases = corpus["cases"].as_array().unwrap();
        let mut differ = Vec::new();
        for c in cases {
            let method = c["method"].as_str().unwrap();
            let path = c["path"].as_str().unwrap();
            let bytes: Vec<u8> = match c["bytes"].as_array() {
                Some(b) => b.iter().map(|x| x.as_u64().unwrap() as u8).collect(),
                None => c["request"].as_str().unwrap().as_bytes().to_vec(),
            };
            let mut headers = HeaderMap::new();
            if let Some(ct) = c["content_type"].as_str() {
                headers.insert("content-type", ct.parse().unwrap());
            }
            let got = body(method, path, &headers, &bytes);
            let ours = match got {
                Ok(v) => json!({"status": 200, "body": v}),
                Err(r) => {
                    let status = r.status().as_u16();
                    let b = axum::body::to_bytes(r.into_body(), usize::MAX)
                        .await
                        .unwrap();
                    match serde_json::from_slice::<Value>(&b) {
                        Ok(j) if status == 422 => json!({"status": 422, "detail": j["detail"]}),
                        _ => json!({"status": status}),
                    }
                }
            };
            let want = match c["status"].as_str() {
                // A value that is not finite goes on to Spoolman's handler: here, validated.
                Some("not finite") => {
                    if ours["status"] != 200 {
                        differ.push(format!("{method} {path} {:?}: ours {ours}", c["request"]));
                    }
                    continue;
                }
                _ => {
                    let mut w = json!({"status": c["status"]});
                    for k in ["detail", "body"] {
                        if c.get(k).is_some() {
                            w[k] = c[k].clone();
                        }
                    }
                    w
                }
            };
            if ours != want {
                differ.push(format!(
                    "{method} {path} {} {:?}:\n  fastapi {want}\n  ours    {ours}",
                    c["content_type"], c["request"]
                ));
            }
        }
        assert!(
            differ.is_empty(),
            "{} of {} cases differ:\n{}",
            differ.len(),
            cases.len(),
            differ
                .iter()
                .take(40)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
