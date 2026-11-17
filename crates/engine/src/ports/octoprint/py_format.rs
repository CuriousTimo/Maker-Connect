//! Python's `str.format` for the arguments `comm.py:3562 _process_registered_message` gives it:
//! `template.format(*match.groups())` and `template.format(**match.groupdict())`, where every
//! argument is a `str` or `None` (a group that did not take part in the match).
//!
//! Port of CPython 3.13 `Objects/stringlib/unicode_format.h` (`do_string_format`, `build_string`,
//! `MarkupIterator_next`, `parse_field`, `output_markup`, `get_field_object`, `field_name_split`,
//! `FieldNameIterator_next`, `do_conversion`) and `Python/formatter_unicode.c`
//! (`_PyUnicode_FormatAdvancedWriter`, `parse_internal_render_format_spec`, `format_string_internal`),
//! as far as they apply to a `str` or `None` argument (fetched from
//! `https://raw.githubusercontent.com/python/cpython/3.13/`; the C files are not in
//! `reference/cpython-3.13`). Proven against real CPython 3.13 by `scripts/octoprint-format-check.py`
//! (fixture `py_format_expected.json`, test `matches_cpython_3_13_str_format`).
//!
//! OURS: an attribute of a `str` or `None` as a field (`{0.upper}`, `{0.__class__}`) is accepted by
//! Python (it prints a repr, often with a memory address, or the object's class or docs); it is
//! `unimplemented!`. A name that is no attribute (`dir()` in `py_str_attrs.txt`) is `AttributeError`.
//! `repr` / `ascii` of a `str` use the code points that are not printable (`py_nonprintable.txt`,
//! from `str.isprintable()`). The texts of the exceptions are ours (the classes are Python's, and the
//! fixture compares the class).

/// The exception `str.format` raises (the class matters: `comm.py` handles `KeyError` differently).
#[derive(Debug, Clone, PartialEq)]
pub enum FormatError {
    KeyError(String),
    IndexError(String),
    ValueError(String),
    TypeError(String),
    AttributeError(String),
}

type Arg = Option<String>;

/// Python's `str.isprintable` for one character: the code points that are not printable are the
/// ranges of `py_nonprintable.txt` (written from CPython 3.13's own `str.isprintable` by
/// `scripts/octoprint-format-check.py`).
fn isprintable(c: char) -> bool {
    use std::sync::LazyLock;
    static NON_PRINTABLE: LazyLock<Vec<(u32, u32)>> = LazyLock::new(|| {
        include_str!("py_nonprintable.txt")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| {
                let (a, b) = l.split_once('-').expect("first-last");
                (
                    u32::from_str_radix(a, 16).unwrap(),
                    u32::from_str_radix(b, 16).unwrap(),
                )
            })
            .collect()
    });
    let cp = c as u32;
    NON_PRINTABLE
        .binary_search_by(|&(a, b)| {
            if cp < a {
                std::cmp::Ordering::Greater
            } else if cp > b {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_err()
}

/// `Py_UNICODE_TODECIMAL`: the value of any Unicode decimal digit (`unicode_format.h get_integer`,
/// `formatter_unicode.c get_integer`).
fn decimal(c: char) -> Option<usize> {
    let mut buf = [0; 4];
    let s = super::comm::py_ascii_digits(c.encode_utf8(&mut buf));
    let d = s.chars().next().filter(char::is_ascii_digit)?;
    Some(d as usize - '0' as usize)
}

/// Python `repr(str)`; `ascii_only` is `ascii()`.
pub fn py_str_repr(s: &str, ascii_only: bool) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
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
            c if c.is_ascii() => out.push(c),
            c if !ascii_only && isprintable(c) => out.push(c),
            c if (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(quote);
    out
}

/// `str.format(*args, **kwargs)` with `str`/`None` arguments (`kwargs` in the order of the dict):
/// `do_string_format` (unicode_format.h:995 region) with `recursion_depth = 2`.
pub fn format(
    template: &str,
    args: &[Arg],
    kwargs: &[(String, Arg)],
) -> Result<String, FormatError> {
    let chars: Vec<char> = template.chars().collect();
    let mut auto = AutoNumber {
        state: AutoNumberState::Init,
        field_number: 0,
    };
    build_string(&chars, args, kwargs, 2, &mut auto)
}

/// `AutoNumberState` / `AutoNumber` (unicode_format.h:20-40)
#[derive(PartialEq, Clone, Copy)]
enum AutoNumberState {
    Init,
    Auto,
    Manual,
}

struct AutoNumber {
    state: AutoNumberState,
    field_number: i64,
}

fn value_error(text: &str) -> FormatError {
    FormatError::ValueError(text.to_string())
}

/// `build_string` (unicode_format.h:900)
fn build_string(
    input: &[char],
    args: &[Arg],
    kwargs: &[(String, Arg)],
    recursion_depth: i32,
    auto: &mut AutoNumber,
) -> Result<String, FormatError> {
    // check the recursion level
    if recursion_depth <= 0 {
        return Err(value_error("Max string recursion exceeded"));
    }
    do_markup(input, args, kwargs, recursion_depth, auto)
}

/// One field found by `MarkupIterator_next` / `parse_field`: ranges into the input.
struct Field {
    name: (usize, usize),
    spec: (usize, usize),
    spec_needs_expanding: bool,
    conversion: Option<char>,
}

/// `do_markup` (unicode_format.h:870) with `MarkupIterator_next` (:675) inlined: literal text up to
/// the next brace, `{{` / `}}` escapes, then a field.
fn do_markup(
    input: &[char],
    args: &[Arg],
    kwargs: &[(String, Arg)],
    recursion_depth: i32,
    auto: &mut AutoNumber,
) -> Result<String, FormatError> {
    let end = input.len();
    let mut pos = 0usize;
    let mut out = String::new();
    // No more input, end of iterator. This is the normal exit path.
    while pos < end {
        let start = pos;
        let mut markup_follows = false;
        let mut c = '\0';
        // First read any literal text. Read until the end of string, an escaped '{' or '}', or an
        // unescaped '{'.
        while pos < end {
            c = input[pos];
            pos += 1;
            if c == '{' || c == '}' {
                markup_follows = true;
                break;
            }
        }

        let at_end = pos >= end;
        let mut len = pos - start;

        if c == '}' && markup_follows && (at_end || c != input[pos]) {
            return Err(value_error("Single '}' encountered in format string"));
        }
        if at_end && c == '{' && markup_follows {
            return Err(value_error("Single '{' encountered in format string"));
        }
        if !at_end && markup_follows {
            if c == input[pos] {
                // escaped } or {, skip it in the input. there is no markup object following us,
                // just this literal text
                pos += 1;
                markup_follows = false;
            } else {
                len -= 1;
            }
        }

        // the literal text (with the first brace of an escape)
        out.extend(&input[start..start + len]);

        if !markup_follows {
            continue;
        }

        // this is markup; parse the field
        let field = parse_field(input, &mut pos, end)?;
        out.push_str(&output_markup(
            input,
            &field,
            args,
            kwargs,
            recursion_depth,
            auto,
        )?);
    }
    Ok(out)
}

/// `parse_field` (unicode_format.h:553)
fn parse_field(input: &[char], pos: &mut usize, end: usize) -> Result<Field, FormatError> {
    let mut c = '\0';
    let mut field = Field {
        name: (*pos, *pos),
        spec: (0, 0),
        spec_needs_expanding: false,
        conversion: None,
    };

    // Search for the field name. it's terminated by the end of the string, or a ':' or '!'
    while *pos < end {
        c = input[*pos];
        *pos += 1;
        match c {
            '{' => return Err(value_error("unexpected '{' in field name")),
            '[' => {
                while *pos < end && input[*pos] != ']' {
                    *pos += 1;
                }
                continue;
            }
            '}' | ':' | '!' => break,
            _ => continue,
        }
    }

    field.name.1 = *pos - 1;
    if c == '!' || c == ':' {
        // we have a format specifier and/or a conversion
        // see if there's a conversion specifier
        if c == '!' {
            // there must be another character present
            if *pos >= end {
                return Err(value_error(
                    "end of string while looking for conversion specifier",
                ));
            }
            field.conversion = Some(input[*pos]);
            *pos += 1;

            if *pos < end {
                c = input[*pos];
                *pos += 1;
                if c == '}' {
                    return Ok(field);
                }
                if c != ':' {
                    return Err(value_error("expected ':' after conversion specifier"));
                }
            }
        }
        field.spec.0 = *pos;
        let mut count = 1;
        while *pos < end {
            c = input[*pos];
            *pos += 1;
            match c {
                '{' => {
                    field.spec_needs_expanding = true;
                    count += 1;
                }
                '}' => {
                    count -= 1;
                    if count == 0 {
                        field.spec.1 = *pos - 1;
                        return Ok(field);
                    }
                }
                _ => {}
            }
        }

        return Err(value_error("unmatched '{' in format spec"));
    } else if c != '}' {
        return Err(value_error("expected '}' before end of string"));
    }

    Ok(field)
}

/// `output_markup` (unicode_format.h:800): the field object, its conversion, the expanded
/// format spec, `render_field`.
fn output_markup(
    input: &[char],
    field: &Field,
    args: &[Arg],
    kwargs: &[(String, Arg)],
    recursion_depth: i32,
    auto: &mut AutoNumber,
) -> Result<String, FormatError> {
    // convert field_name to an object
    let value = get_field_object(&input[field.name.0..field.name.1], args, kwargs, auto)?;

    // `do_conversion`
    let value: Arg = match field.conversion {
        None => value,
        Some('r') => Some(match value {
            Some(s) => py_str_repr(&s, false),
            None => "None".to_string(),
        }),
        Some('s') => Some(value.unwrap_or_else(|| "None".to_string())),
        Some('a') => Some(match value {
            Some(s) => py_str_repr(&s, true),
            None => "None".to_string(),
        }),
        Some(c) => {
            return Err(FormatError::ValueError(
                if (c as u32) > 32 && (c as u32) < 127 {
                    format!("Unknown conversion specifier {c}")
                } else {
                    format!("Unknown conversion specifier \\x{:x}", c as u32)
                },
            ));
        }
    };

    // if needed, recursively compute the format_spec
    let spec_chars: Vec<char> = if field.spec_needs_expanding {
        build_string(
            &input[field.spec.0..field.spec.1],
            args,
            kwargs,
            recursion_depth - 1,
            auto,
        )?
        .chars()
        .collect()
    } else {
        input[field.spec.0..field.spec.1].to_vec()
    };

    // render_field: `str` goes to `_PyUnicode_FormatAdvancedWriter`, `None` to `object.__format__`
    match value {
        Some(s) => format_str(&s, &spec_chars),
        None => {
            if spec_chars.is_empty() {
                Ok("None".to_string())
            } else {
                Err(FormatError::TypeError(
                    "unsupported format string passed to NoneType.__format__".to_string(),
                ))
            }
        }
    }
}

/// `get_integer` (unicode_format.h:106): `Ok(None)` is -1 (empty or not a decimal number)
fn get_integer(s: &[char]) -> Result<Option<i64>, FormatError> {
    // empty string is an error
    if s.is_empty() {
        return Ok(None);
    }
    let mut accumulator: i64 = 0;
    for &c in s {
        let Some(digit) = decimal(c) else {
            return Ok(None);
        };
        let digit = digit as i64;
        // Detect possible overflow before it happens
        if accumulator > (i64::MAX - digit) / 10 {
            return Err(value_error("Too many decimal digits in format string"));
        }
        accumulator = accumulator * 10 + digit;
    }
    Ok(Some(accumulator))
}

/// `get_field_object` (unicode_format.h:395) with `field_name_split` (:339) and `FieldNameIterator_next` (:290).
/// The names of what a `str` and `None` have as attributes are in `py_str_attrs.txt` (CPython 3.13's `dir`).
fn get_field_object(
    name: &[char],
    args: &[Arg],
    kwargs: &[(String, Arg)],
    auto: &mut AutoNumber,
) -> Result<Arg, FormatError> {
    // field_name_split: find the part up until the first '.' or '['
    let first_end = name
        .iter()
        .position(|c| *c == '[' || *c == '.')
        .unwrap_or(name.len());
    let first = &name[..first_end];
    let mut rest = first_end;

    // see if "first" is an integer, in which case it's used as an index
    let mut first_idx = get_integer(first)?;

    let field_name_is_empty = first.is_empty();

    // If the field name is omitted or if we have a numeric index specified, then we're doing
    // numeric indexing into args.
    let using_numeric_index = field_name_is_empty || first_idx.is_some();

    // Initialize our auto numbering state if this is the first time we're either auto-numbering or
    // manually numbering.
    if auto.state == AutoNumberState::Init && using_numeric_index {
        auto.state = if field_name_is_empty {
            AutoNumberState::Auto
        } else {
            AutoNumberState::Manual
        };
    }
    // Make sure our state is consistent with what we're doing this time through.
    if using_numeric_index {
        if auto.state == AutoNumberState::Manual {
            if field_name_is_empty {
                return Err(value_error(
                    "cannot switch from manual field specification to automatic field numbering",
                ));
            }
        } else if !field_name_is_empty {
            return Err(value_error(
                "cannot switch from automatic field numbering to manual field specification",
            ));
        }
    }
    // Zero length field means we want to do auto-numbering of the fields.
    if field_name_is_empty {
        first_idx = Some(auto.field_number);
        auto.field_number += 1;
    }

    let mut obj: Arg = match first_idx {
        None => {
            // look up in kwargs
            let key: String = first.iter().collect();
            match kwargs.iter().find(|(k, _)| *k == key) {
                Some((_, v)) => v.clone(),
                None => return Err(FormatError::KeyError(key)),
            }
        }
        Some(index) => match usize::try_from(index).ok().and_then(|i| args.get(i)) {
            Some(v) => v.clone(),
            None => {
                return Err(FormatError::IndexError(format!(
                    "Replacement index {index} out of range for positional args tuple"
                )));
            }
        },
    };

    // iterate over the rest of the field_name
    while rest < name.len() {
        let c = name[rest];
        rest += 1;
        let is_attribute;
        let item: &[char];
        let mut name_idx: Option<i64> = None;
        match c {
            '.' => {
                is_attribute = true;
                // return everything until '.' or '['
                let start = rest;
                while rest < name.len() && name[rest] != '[' && name[rest] != '.' {
                    rest += 1;
                }
                item = &name[start..rest];
            }
            '[' => {
                is_attribute = false;
                // return everything until ']'
                let start = rest;
                let mut bracket_seen = false;
                while rest < name.len() {
                    let c = name[rest];
                    rest += 1;
                    if c == ']' {
                        bracket_seen = true;
                        break;
                    }
                }
                // make sure we ended with a ']'
                if !bracket_seen {
                    return Err(value_error("Missing ']' in format string"));
                }
                // don't include the ']'
                item = &name[start..rest - 1];
                name_idx = get_integer(item)?;
            }
            _ => {
                // Invalid character follows ']'
                return Err(value_error(
                    "Only '.' or '[' may follow ']' in format field specifier",
                ));
            }
        }

        // empty string is an error
        if item.is_empty() {
            return Err(value_error("Empty attribute in format string"));
        }

        if is_attribute {
            // getattr lookup "."
            let attr: String = item.iter().collect();
            let known = if obj.is_some() {
                str_attrs("str").contains(&attr.as_str())
            } else {
                str_attrs("None").contains(&attr.as_str())
            };
            if known {
                unimplemented!(
                    "str.format: the attribute {attr:?} of a {} as a field",
                    if obj.is_some() { "str" } else { "None" }
                );
            }
            return Err(FormatError::AttributeError(format!(
                "'{}' object has no attribute '{attr}'",
                if obj.is_some() { "str" } else { "NoneType" }
            )));
        }
        // getitem lookup "[]"
        obj = match (&obj, name_idx) {
            (None, _) => {
                return Err(FormatError::TypeError(
                    "'NoneType' object is not subscriptable".to_string(),
                ));
            }
            (Some(_), None) => {
                return Err(FormatError::TypeError(
                    "string indices must be integers, not 'str'".to_string(),
                ));
            }
            (Some(s), Some(i)) => match usize::try_from(i).ok().and_then(|i| s.chars().nth(i)) {
                Some(c) => Some(c.to_string()),
                None => {
                    return Err(FormatError::IndexError(
                        "string index out of range".to_string(),
                    ));
                }
            },
        };
    }
    Ok(obj)
}

/// `dir("")` (`which` is `"str"`) or `dir(None)` (`"None"`) of CPython 3.13 (`py_str_attrs.txt`,
/// written by `scripts/octoprint-format-check.py`): the names `getattr` finds.
fn str_attrs(which: &str) -> &'static [&'static str] {
    use std::sync::LazyLock;
    static ATTRS: LazyLock<(Vec<&'static str>, Vec<&'static str>)> = LazyLock::new(|| {
        let (mut s, mut n) = (Vec::new(), Vec::new());
        for line in include_str!("py_str_attrs.txt")
            .lines()
            .filter(|l| !l.starts_with('#'))
        {
            let (kind, name) = line.split_once(' ').expect("kind name");
            match kind {
                "str" => s.push(name),
                "None" => n.push(name),
                _ => unreachable!("kind"),
            }
        }
        (s, n)
    });
    if which == "str" { &ATTRS.0 } else { &ATTRS.1 }
}

/// `_PyUnicode_FormatAdvancedWriter` for a `str` (formatter_unicode.c:1451): `parse_internal_render_format_spec`
/// (:149, default type `s`, default align `<`) and `format_string_internal` (:765).
fn format_str(s: &str, spec: &[char]) -> Result<String, FormatError> {
    // check for the special case of zero length format spec, make it equivalent to str(obj)
    if spec.is_empty() {
        return Ok(s.to_string());
    }
    let end = spec.len();
    let mut pos = 0usize;
    let (mut fill_char, mut align) = (' ', '<');
    let (mut alternate, mut no_neg_0, mut sign) = (false, false, '\0');
    let (mut align_specified, mut fill_char_specified) = (false, false);
    let is_align = |c: char| matches!(c, '<' | '>' | '=' | '^');

    // If the second char is an alignment token, then parse the fill char
    if end - pos >= 2 && is_align(spec[pos + 1]) {
        align = spec[pos + 1];
        fill_char = spec[pos];
        fill_char_specified = true;
        align_specified = true;
        pos += 2;
    } else if end - pos >= 1 && is_align(spec[pos]) {
        align = spec[pos];
        align_specified = true;
        pos += 1;
    }

    // Parse the various sign options
    if end - pos >= 1 && matches!(spec[pos], ' ' | '+' | '-') {
        sign = spec[pos];
        pos += 1;
    }

    // If the next character is z, request coercion of negative 0. Applies only to floats.
    if end - pos >= 1 && spec[pos] == 'z' {
        no_neg_0 = true;
        pos += 1;
    }

    // If the next character is #, we're in alternate mode. This only applies to integers.
    if end - pos >= 1 && spec[pos] == '#' {
        alternate = true;
        pos += 1;
    }

    // The special case for 0-padding (backwards compat)
    if !fill_char_specified && end - pos >= 1 && spec[pos] == '0' {
        fill_char = '0';
        // (`if !align_specified && default_align == '>'`: the default align of a str is '<')
        let _ = align_specified;
        pos += 1;
    }

    // `get_integer` of formatter_unicode.c:60: the number of digits, the value
    let get_integer = |pos: &mut usize| -> Result<(usize, i64), FormatError> {
        let (mut accumulator, mut numdigits) = (0i64, 0usize);
        while *pos < end {
            let Some(digit) = decimal(spec[*pos]) else {
                break;
            };
            let digit = digit as i64;
            if accumulator > (i64::MAX - digit) / 10 {
                return Err(value_error("Too many decimal digits in format string"));
            }
            accumulator = accumulator * 10 + digit;
            *pos += 1;
            numdigits += 1;
        }
        Ok((numdigits, accumulator))
    };

    let (consumed, width) = get_integer(&mut pos)?;
    // If consumed is 0, we didn't consume any characters for the width: -1 is "not specified"
    let width: i64 = if consumed == 0 { -1 } else { width };

    // Comma signifies add thousands separators; underscore too
    let mut thousands = '\0';
    if end - pos > 0 && spec[pos] == ',' {
        thousands = ',';
        pos += 1;
    }
    if end - pos > 0 && spec[pos] == '_' {
        if thousands != '\0' {
            return Err(value_error("Cannot specify both ',' and '_'."));
        }
        thousands = '_';
        pos += 1;
    }
    if end - pos > 0 && spec[pos] == ',' && thousands == '_' {
        return Err(value_error("Cannot specify both ',' and '_'."));
    }

    // Parse field precision
    let mut precision: i64 = -1;
    if end - pos > 0 && spec[pos] == '.' {
        pos += 1;
        let (consumed, p) = get_integer(&mut pos)?;
        // Not having a precision after a dot is an error.
        if consumed == 0 {
            return Err(value_error("Format specifier missing precision"));
        }
        precision = p;
    }

    // Finally, parse the type field.
    if end - pos > 1 {
        // More than one char remains, so this is an invalid format specifier.
        return Err(FormatError::ValueError(format!(
            "Invalid format specifier '{}' for object of type 'str'",
            spec.iter().collect::<String>()
        )));
    }

    let mut type_char = 's';
    if end - pos == 1 {
        type_char = spec[pos];
    }

    // Do as much validating as we can, just by looking at the format specifier.
    if thousands != '\0' {
        match type_char {
            'd' | 'e' | 'f' | 'g' | 'E' | 'G' | '%' | 'F' | '\0' => {}
            'b' | 'o' | 'x' | 'X' if thousands == '_' => {}
            _ => {
                return Err(FormatError::ValueError(format!(
                    "Cannot specify '{thousands}' with '{type_char}'."
                )));
            }
        }
    }

    // type conversion?
    if type_char != 's' {
        return Err(FormatError::ValueError(format!(
            "Unknown format code '{type_char}' for object of type 'str'"
        )));
    }

    // format_string_internal: sign is not allowed on strings
    if sign != '\0' {
        return Err(value_error(if sign == ' ' {
            "Space not allowed in string format specifier"
        } else {
            "Sign not allowed in string format specifier"
        }));
    }
    // negative 0 coercion is not allowed on strings
    if no_neg_0 {
        return Err(value_error(
            "Negative zero coercion (z) not allowed in string format specifier",
        ));
    }
    // alternate is not allowed on strings
    if alternate {
        return Err(value_error(
            "Alternate form (#) not allowed in string format specifier",
        ));
    }
    // '=' alignment not allowed on strings
    if align == '=' {
        return Err(value_error(
            "'=' alignment not allowed in string format specifier",
        ));
    }

    // if precision is specified, output no more that format.precision characters
    let mut text: Vec<char> = s.chars().collect();
    if precision >= 0 && text.len() as i64 >= precision {
        text.truncate(precision as usize);
    }

    // `calc_padding`
    let len = text.len() as i64;
    let total = if width >= 0 && width > len {
        width
    } else {
        len
    };
    let pad = total - len;
    let (lpad, rpad) = match align {
        '>' => (pad, 0),
        '^' => (pad / 2, pad - pad / 2),
        _ => (0, pad),
    };
    let mut out = String::new();
    out.extend(std::iter::repeat_n(fill_char, lpad as usize));
    out.extend(text);
    out.extend(std::iter::repeat_n(fill_char, rpad as usize));
    Ok(out)
}

#[cfg(test)]
mod tests {
    //! OURS: OctoPrint has no test of the templates (`comm.py:3562`); the expected values are what
    //! CPython 3.13's `str.format` prints for these calls.
    use super::*;

    fn a(items: &[Option<&str>]) -> Vec<Arg> {
        items.iter().map(|x| x.map(str::to_string)).collect()
    }
    fn kw(items: &[(&str, Option<&str>)]) -> Vec<(String, Arg)> {
        items
            .iter()
            .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
            .collect()
    }

    #[test]
    fn positional_automatic_and_manual() {
        assert_eq!(
            Ok("Temp 21.5C".to_string()),
            format("Temp {0}C", &a(&[Some("21.5")]), &[])
        );
        assert_eq!(
            Ok("a-b".to_string()),
            format("{}-{}", &a(&[Some("a"), Some("b")]), &[])
        );
        assert_eq!(
            Ok("b a b".to_string()),
            format("{1} {0} {1}", &a(&[Some("a"), Some("b")]), &[])
        );
        assert_eq!(Ok("{x}".to_string()), format("{{x}}", &[], &[]));
        assert_eq!(Ok("None".to_string()), format("{0}", &a(&[None]), &[]));
        assert!(matches!(
            format("{} {0}", &a(&[Some("a")]), &[]),
            Err(FormatError::ValueError(_))
        ));
        assert!(matches!(
            format("{0} {}", &a(&[Some("a")]), &[]),
            Err(FormatError::ValueError(_))
        ));
    }

    #[test]
    fn missing_arguments() {
        assert!(matches!(
            format("{1}", &a(&[Some("a")]), &[]),
            Err(FormatError::IndexError(_))
        ));
        assert!(matches!(
            format("{}", &[], &[]),
            Err(FormatError::IndexError(_))
        ));
        assert_eq!(
            Err(FormatError::KeyError("name".to_string())),
            format("{name}", &[], &[])
        );
        assert_eq!(
            Ok("v".to_string()),
            format("{name}", &[], &kw(&[("name", Some("v"))]))
        );
        assert_eq!(
            Ok("None".to_string()),
            format("{name}", &[], &kw(&[("name", None)]))
        );
    }

    #[test]
    fn conversions_and_specs() {
        let x = a(&[Some("ab")]);
        assert_eq!(Ok("'ab'".to_string()), format("{0!r}", &x, &[]));
        assert_eq!(Ok("ab".to_string()), format("{0!s}", &x, &[]));
        assert_eq!(Ok("ab   |".to_string()), format("{0:5}|", &x, &[]));
        assert_eq!(Ok("   ab|".to_string()), format("{0:>5}|", &x, &[]));
        assert_eq!(Ok(" ab  |".to_string()), format("{0:^5}|", &x, &[]));
        assert_eq!(Ok("*ab**".to_string()), format("{0:*^5}", &x, &[]));
        assert_eq!(Ok("a".to_string()), format("{0:.1}", &x, &[]));
        assert_eq!(Ok("ab000".to_string()), format("{0:05}", &x, &[]));
        assert_eq!(
            Ok("ab  ".to_string()),
            format("{0:{1}}", &a(&[Some("ab"), Some("4")]), &[])
        );
        assert!(matches!(
            format("{0:d}", &x, &[]),
            Err(FormatError::ValueError(_))
        ));
        assert!(matches!(
            format("{0:+}", &x, &[]),
            Err(FormatError::ValueError(_))
        ));
        assert!(matches!(
            format("{0:=5}", &x, &[]),
            Err(FormatError::ValueError(_))
        ));
        assert!(matches!(
            format("{0:5}", &a(&[None]), &[]),
            Err(FormatError::TypeError(_))
        ));
        assert_eq!(
            Ok("None  ".to_string()),
            format("{0!s:6}", &a(&[None]), &[])
        );
    }

    #[test]
    fn indexing_and_attributes() {
        let x = a(&[Some("abc")]);
        assert_eq!(Ok("b".to_string()), format("{0[1]}", &x, &[]));
        assert!(matches!(
            format("{0[5]}", &x, &[]),
            Err(FormatError::IndexError(_))
        ));
        assert!(matches!(
            format("{0[x]}", &x, &[]),
            Err(FormatError::TypeError(_))
        ));
        assert!(matches!(
            format("{0.nope}", &x, &[]),
            Err(FormatError::AttributeError(_))
        ));
    }

    #[test]
    fn syntax_errors() {
        for t in ["{", "}", "{0", "a } b", "{0!}", "{0!x}", "{0!rx}"] {
            assert!(
                matches!(
                    format(t, &a(&[Some("a")]), &[]),
                    Err(FormatError::ValueError(_))
                ),
                "{t}"
            );
        }
    }

    #[test]
    fn repr_of_str() {
        assert_eq!("'a'", py_str_repr("a", false));
        assert_eq!("\"it's\"", py_str_repr("it's", false));
        assert_eq!("'it\\'s \"q\"'", py_str_repr("it's \"q\"", false));
        assert_eq!("'a\\nb\\x00'", py_str_repr("a\nb\0", false));
        assert_eq!("'\\xe4'", py_str_repr("\u{e4}", true));
        assert_eq!("'\u{e4}'", py_str_repr("\u{e4}", false));
    }

    /// The calls of `scripts/octoprint-format-check.py` (every call of the tests above, and the
    /// shapes `comm.py:3562-3592` can produce: str and None arguments, positional and named fields,
    /// conversions, format specs, `{{` / `}}`, errors) run through real CPython 3.13 `str.format`:
    /// the text, or the exception class, byte for byte. A field that is an attribute of a `str`
    /// (Python accepts it) is `unimplemented!` here: the fixture marks those cases.
    #[test]
    fn matches_cpython_3_13_str_format() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("py_format_expected.json"))
                .expect("the fixture is JSON");
        let arg = |v: &serde_json::Value| -> Arg { v.as_str().map(str::to_string) };
        let class = |e: &FormatError| match e {
            FormatError::KeyError(_) => "KeyError",
            FormatError::IndexError(_) => "IndexError",
            FormatError::ValueError(_) => "ValueError",
            FormatError::TypeError(_) => "TypeError",
            FormatError::AttributeError(_) => "AttributeError",
        };
        let cases = fixture["cases"].as_array().expect("cases");
        assert!(cases.len() > 10_000);
        let (mut differs, mut unimplemented) = (Vec::new(), 0);
        for case in cases {
            let template = case["template"].as_str().unwrap();
            let args: Vec<Arg> = case["args"].as_array().unwrap().iter().map(arg).collect();
            let kwargs: Vec<(String, Arg)> = case["kwargs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|kv| (kv[0].as_str().unwrap().to_string(), arg(&kv[1])))
                .collect();
            let got = std::panic::catch_unwind(|| format(template, &args, &kwargs));
            let what = format!("{template:?} args={args:?} kwargs={kwargs:?}");
            if case.get("unimplemented").is_some() {
                unimplemented += 1;
                match got {
                    Err(p)
                        if p.downcast_ref::<String>()
                            .is_some_and(|m| m.starts_with("not implemented")) => {}
                    other => differs.push(format!(
                        "{what}: Python accepts it, expected `unimplemented!`, got {other:?}"
                    )),
                }
                continue;
            }
            let expected = &case["expected"];
            match (got, expected.get("ok"), expected.get("error")) {
                (Ok(Ok(text)), Some(ok), _) if ok.as_str() == Some(text.as_str()) => {}
                (Ok(Err(e)), _, Some(err)) if err.as_str() == Some(class(&e)) => {}
                (got, _, _) => differs.push(format!("{what}: Python {expected}, got {got:?}")),
            }
        }
        assert!(unimplemented > 0);
        assert!(
            differs.is_empty(),
            "{} of {} cases differ, first:\n{}",
            differs.len(),
            cases.len(),
            differs
                .iter()
                .take(25)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
