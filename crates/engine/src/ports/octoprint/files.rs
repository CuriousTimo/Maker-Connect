//! Port of the parts of OctoPrint 1.11.8 `src/octoprint/util/files.py` the port uses.
//!
//! Original: Copyright (C) Gina Häußge & others, AGPL-3.0.

use super::comm::{ValueError, py_strip};

/// `octoprint/util/files.py:353 m20_timestamp_to_unix_timestamp`
///
/// Converts "M20 T" timestamp to unix timestamp.
/// Upper 16 bit contain date, lower 16 bit contain time.
///
/// <https://reprap.org/wiki/G-code#M20:_List_SD_card>
///
/// Format derived from FAT filesystem timestamps:
/// <https://wiki.osdev.org/FAT>
///
/// Arguments:
///     timestamp (string): M20 T timestamp as hex string
/// Returns:
///     int: Unix timestamp in seconds
///
/// The naive `datetime.datetime.combine(d, t).timestamp()` is local time; here it is converted
/// with the C library's `mktime` (`tm_isdst = -1`).
pub fn m20_timestamp_to_unix_timestamp(timestamp: &str) -> Result<i64, ValueError> {
    // Only hex in 0xABC format is valid while int() accepts
    // hex without 0x prefix, too.
    if !timestamp.starts_with("0x") {
        return Err(ValueError("Invalid M20 T timestamp format"));
    }

    // int(timestamp, 16): hex digits, single underscores between digits (and right after the
    // prefix), surrounding whitespace. Only the low 32 bits are ever used below.
    let body = &py_strip(timestamp)[2..];
    let body = body.strip_prefix('_').unwrap_or(body);
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return Err(ValueError("invalid literal for int() with base 16"));
    }
    let digits: Vec<char> = body.chars().filter(|c| *c != '_').collect();
    if !digits.iter().all(|c| c.is_ascii_hexdigit()) {
        return Err(ValueError("invalid literal for int() with base 16"));
    }
    let low: String = digits[digits.len().saturating_sub(8)..].iter().collect();
    let timestamp: u64 = u64::from_str_radix(&low, 16).unwrap();

    let dt = timestamp >> 16;
    let day = dt & ((1 << 5) - 1);
    let month = (dt >> 5) & ((1 << 4) - 1);
    let year = ((dt >> 9) & ((1 << 7) - 1)) + 1980;
    // datetime.date(year, month, day)
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => return Err(ValueError("month must be in 1..12")),
    };
    if day < 1 || day > days_in_month {
        return Err(ValueError("day is out of range for month"));
    }

    let tm = timestamp & (2u64.pow(16) - 1);
    let second = (tm & ((1 << 5) - 1)) * 2;
    let minute = (tm >> 5) & ((1 << 6) - 1);
    let hour = (tm >> 11) & ((1 << 5) - 1);
    // datetime.time(hour, minute, second)
    if hour > 23 {
        return Err(ValueError("hour must be in 0..23"));
    }
    if minute > 59 {
        return Err(ValueError("minute must be in 0..59"));
    }
    if second > 59 {
        return Err(ValueError("second must be in 0..59"));
    }

    Ok(naive_local_to_timestamp(
        year as i32,
        month as i32,
        day as i32,
        hour as i32,
        minute as i32,
        second as i32,
    ))
}

#[cfg(unix)]
fn naive_local_to_timestamp(
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
) -> i64 {
    use std::ffi::{c_char, c_int, c_long};

    #[repr(C)]
    struct Tm {
        tm_sec: c_int,
        tm_min: c_int,
        tm_hour: c_int,
        tm_mday: c_int,
        tm_mon: c_int,
        tm_year: c_int,
        tm_wday: c_int,
        tm_yday: c_int,
        tm_isdst: c_int,
        tm_gmtoff: c_long,
        tm_zone: *const c_char,
    }
    unsafe extern "C" {
        fn mktime(tm: *mut Tm) -> c_long;
    }
    let mut tm = Tm {
        tm_sec: second,
        tm_min: minute,
        tm_hour: hour,
        tm_mday: day,
        tm_mon: month - 1,
        tm_year: year - 1900,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: -1,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null(),
    };
    // SAFETY: `tm` is a fully initialised struct with the C library's `struct tm` layout.
    unsafe { mktime(&mut tm) as i64 }
}

#[cfg(not(unix))]
fn naive_local_to_timestamp(_: i32, _: i32, _: i32, _: i32, _: i32, _: i32) -> i64 {
    todo!("local time to unix timestamp is only implemented for unix targets")
}

// ---------------------------------------------------------------------------------------------
// util/files.py:1-171 sanitize_filename, get_dos_filename, find_collision_free_name
// ---------------------------------------------------------------------------------------------

use super::text;

/// `util/files.py:12 _STRIPPED_FILE_NAME_CHARS`: `";|&?$*<>"` plus the ASCII characters that are
/// not in `string.printable`
fn _stripped_file_name_char(c: char) -> bool {
    if ";|&?$*<>".contains(c) {
        return true;
    }
    // _UNPRINTABLE_ASCII: chr(c) for c in range(128) if chr(c) not in string.printable
    (c as u32) < 128
        && !(c.is_ascii_graphic() || c == ' ' || matches!(c, '\t' | '\n' | '\r' | '\x0b' | '\x0c'))
}

/// `posixpath.splitext(p)` (`genericpath._splitext`, CPython `Lib/genericpath.py:157`).
/// Source: `reference/cpython-3.13/genericpath.py:157` (identical in 3.14, where it was first read).
pub fn splitext(p: &str) -> (&str, &str) {
    let sep_index = p.rfind('/').map(|i| i as isize).unwrap_or(-1);
    if let Some(dot_index) = p.rfind('.') {
        if dot_index as isize > sep_index {
            // skip all leading dots
            let mut filename_index = (sep_index + 1) as usize;
            while filename_index < dot_index {
                if p.as_bytes()[filename_index] != b'.' {
                    return (&p[..dot_index], &p[dot_index..]);
                }
                filename_index += 1;
            }
        }
    }
    (p, &p[..0])
}

/// `posixpath.basename(p)` (CPython `Lib/posixpath.py:166`, see [`splitext`] for the source)
pub fn basename(p: &str) -> &str {
    let i = p.rfind('/').map(|i| i + 1).unwrap_or(0);
    &p[i..]
}

/// `util/files.py:34 _sfn_really_universal`
fn _sfn_really_universal(name: &str) -> String {
    let result = text::sanitize(name, "-_.()[] ", true).replace(' ', "_");
    let (root, ext) = splitext(&result);
    let mut root = root.to_string();
    // _WINDOWS_RESERVED_FILE_NAMES + _MACOS_RESERVED_FILE_NAMES
    let upper = root.to_uppercase();
    let reserved = ["CON", "PRN", "AUX", "CLOCK$", "NUL"].contains(&upper.as_str())
        || ["COM", "LPT"].iter().any(|n| {
            upper
                .strip_prefix(n)
                .is_some_and(|d| d.len() == 1 && matches!(d.as_bytes()[0], b'1'..=b'9'))
        })
        || upper == ":";
    if reserved {
        root.push('_');
    }
    root + ext
}

/// `util/files.py:42 sanitize_filename(name, really_universal=False)`
///
/// Sanitizes the provided filename. Implementation differs between Python versions.
///
/// Under normal operation, ``pathvalidate.sanitize_filename`` will be used (NOT PORTED, third
/// party: `really_universal=False` is `unimplemented!`). With `really_universal=True` the name is
/// ASCII-fied, using `util::text::sanitize` with safe chars ``-_.()[] `` and all spaces replaced
/// by ``_``. In all cases, a single leading ``.`` will be removed.
///
/// `Err` is `ValueError("name must not contain / or \\")`.
pub fn sanitize_filename(name: &str, really_universal: bool) -> Result<String, ValueError> {
    if name.contains('/') || name.contains('\\') {
        return Err(ValueError("name must not contain / or \\"));
    }

    let stripped: String = name
        .chars()
        .filter(|c| !_stripped_file_name_char(*c))
        .collect();

    let result = if really_universal {
        _sfn_really_universal(&stripped)
    } else {
        unimplemented!(
            "pathvalidate.sanitize_filename (third-party `pathvalidate` package), called at util/files.py:75"
        );
    };

    // `result.lstrip(".")`
    Ok(result.trim_start_matches('.').to_string())
}

/// Python slice `s[:n]` for a code point count `n` (negative counts from the end)
fn py_prefix(s: &str, n: isize) -> String {
    let len = s.chars().count() as isize;
    let end = if n < 0 { (len + n).max(0) } else { n.min(len) };
    s.chars().take(end as usize).collect()
}

/// `util/files.py:80 get_dos_filename(input, existing_filenames=None, extension=None,
/// whitelisted_extensions=None, **kwargs)`
///
/// Converts the provided input filename to a 8.3 DOS compatible filename. If
/// ``existing_filenames`` is provided, the conversion result will be guaranteed not to collide
/// with any of the filenames provided thus. Uses [`find_collision_free_name`] internally
/// (`kwargs` is its `max_power`).
///
/// `Err` is the `ValueError` of `sanitize_filename` or `find_collision_free_name`.
pub fn get_dos_filename(
    input: &str,
    existing_filenames: Option<&[String]>,
    extension: Option<&str>,
    whitelisted_extensions: Option<&[&str]>,
    max_power: Option<u32>,
) -> Result<String, ValueError> {
    let input = sanitize_filename(input, true)?;

    let existing_filenames = existing_filenames.unwrap_or(&[]);

    let mut extension = extension.map(|e| e.to_lowercase());

    let whitelisted_extensions = whitelisted_extensions.unwrap_or(&[]);

    let (filename, ext) = splitext(&input);

    let ext = ext.to_lowercase();
    let ext = ext.strip_prefix('.').unwrap_or(&ext).to_string();
    if whitelisted_extensions.contains(&ext.as_str()) || extension.is_none() {
        extension = Some(ext);
    }

    find_collision_free_name(
        filename,
        &extension.expect("set above"),
        existing_filenames,
        max_power.unwrap_or(2),
    )
}

/// `util/files.py:134 find_collision_free_name(filename, extension, existing_filenames,
/// max_power=2)`
///
/// Tries to find a collision free translation of "`<filename>.<extension>`" to the 8.3 DOS
/// compatible format, preventing collisions with any of the ``existing_filenames``. See the
/// docstring of the original for the algorithm.
///
/// `Err` is `ValueError("Can't create a collision free filename")`.
pub fn find_collision_free_name(
    filename: &str,
    extension: &str,
    existing_filenames: &[String],
    max_power: u32,
) -> Result<String, ValueError> {
    let filename = filename.strip_prefix('/').unwrap_or(filename);
    let existing_filenames: Vec<String> = existing_filenames
        .iter()
        .map(|x| x.strip_prefix('/').unwrap_or(x).to_string())
        .collect();

    // make_valid(text): re.sub(r"\s+", "_", text.translate({ord(i): None for i in r'."/\[]:;=,'})).lower()
    let make_valid = |text: &str| -> String {
        let t: String = text
            .chars()
            .filter(|c| !".\"/\\[]:;=,".contains(*c))
            .collect();
        let mut out = String::new();
        let mut in_ws = false;
        for c in t.chars() {
            if c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c) {
                if !in_ws {
                    out.push('_');
                }
                in_ws = true;
            } else {
                in_ws = false;
                out.push(c);
            }
        }
        out.to_lowercase()
    };

    let filename = make_valid(filename);
    let extension = make_valid(extension);
    let extension = if extension.chars().count() > 3 {
        py_prefix(&extension, 3)
    } else {
        extension
    };

    let full_name = |filename: &str, extension: &str| -> String {
        if !extension.is_empty() {
            format!("{filename}.{extension}")
        } else {
            filename.to_string()
        }
    };

    let result = full_name(&filename, &extension);
    if filename.chars().count() <= 8 && !existing_filenames.contains(&result) {
        // early exit
        return Ok(result);
    }

    let mut counter: u64 = 1;
    let mut power: u32 = 1;
    while counter < 10u64.pow(max_power) {
        let prefix = format!(
            "{}~{}",
            py_prefix(&filename, 6 - power as isize + 1),
            counter
        );
        let result = full_name(&prefix, &extension);
        if !existing_filenames.contains(&result) {
            return Ok(result);
        }
        counter += 1;
        if counter >= 10u64.pow(power) {
            power += 1;
        }
    }

    Err(ValueError("Can't create a collision free filename"))
}

#[cfg(test)]
mod dos_filename_tests {
    //! Ports of `tests/util/test_files.py` (`test_sanitize_filename`,
    //! `test_sanitize_filename_invalid`; the cases that need `pathvalidate`, `unidecode` or `emoji`
    //! are not ported) and of the docstring examples of `find_collision_free_name` and
    //! `get_dos_filename` (the ones with non-ASCII input are not ported).
    use super::*;

    #[test]
    fn test_sanitize_filename() {
        // really_universal
        for (filename, expected) in [
            ("some_file.gcode", "some_file.gcode"),
            ("NUL.gcode", "NUL_.gcode"),
            ("LPT1", "LPT1_"),
            (".test.gcode", "test.gcode"),
            ("..test.gcode", "test.gcode"),
            ("file with space.gcode", "file_with_space.gcode"),
            ("file\u{1}file.gcode", "filefile.gcode"),
            ("file;|&?$*<>file.gcode", "filefile.gcode"),
        ] {
            assert_eq!(
                expected,
                sanitize_filename(filename, true).unwrap(),
                "{filename}"
            );
        }
    }

    #[test]
    fn test_sanitize_filename_invalid() {
        for filename in ["file/with/slash.gcode", "file\\with\\backslash.gcode"] {
            assert_eq!(
                Err(ValueError("name must not contain / or \\")),
                sanitize_filename(filename, true)
            );
        }
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_find_collision_free_name_examples() {
        assert_eq!(
            "test1234.gco",
            find_collision_free_name("test1234", "gco", &[], 2).unwrap()
        );
        assert_eq!(
            "test1234.gco",
            find_collision_free_name("test1234", "gcode", &[], 2).unwrap()
        );
        assert_eq!(
            "test12~1.gco",
            find_collision_free_name("test12345", "gco", &[], 2).unwrap()
        );
        assert_eq!(
            "test_123.gco",
            find_collision_free_name("test 123", "gco", &[], 2).unwrap()
        );
        assert_eq!(
            "test1234.g_o",
            find_collision_free_name("test1234", "g o", &[], 2).unwrap()
        );
        assert_eq!(
            "test12~2.gco",
            find_collision_free_name("test12345", "gco", &v(&["/test12~1.gco"]), 2).unwrap()
        );
        let mut many_files: Vec<String> = (1..10).map(|x| format!("/test12~{x}.gco")).collect();
        assert_eq!(
            "test1~10.gco",
            find_collision_free_name("test12345", "gco", &many_files, 2).unwrap()
        );
        many_files.extend((10..99).map(|x| format!("/test1~{x}.gco")));
        assert_eq!(
            "test1~99.gco",
            find_collision_free_name("test12345", "gco", &many_files, 2).unwrap()
        );
        many_files.push("/test1~99.gco".to_string());
        assert_eq!(
            Err(ValueError("Can't create a collision free filename")),
            find_collision_free_name("test12345", "gco", &many_files, 2)
        );
        assert_eq!(
            "test~100.gco",
            find_collision_free_name("test12345", "gco", &many_files, 3).unwrap()
        );
    }

    #[test]
    fn test_get_dos_filename_examples() {
        assert_eq!(
            "test1234.gco",
            get_dos_filename("test1234.gco", None, None, None, None).unwrap()
        );
        assert_eq!(
            "test1234.gco",
            get_dos_filename("test1234.gcode", None, None, None, None).unwrap()
        );
        assert_eq!(
            "test12~1.gco",
            get_dos_filename("test12345.gco", None, None, None, None).unwrap()
        );
        assert_eq!(
            "test1234.gco",
            get_dos_filename("test1234.fnord", None, Some("gco"), None, None).unwrap()
        );
        assert_eq!(
            "auto0.gco",
            get_dos_filename("auto0.g", None, Some("gco"), None, None).unwrap()
        );
        assert_eq!(
            "auto0.g",
            get_dos_filename("auto0.g", None, Some("gco"), Some(&["g"]), None).unwrap()
        );
        assert_eq!(
            "foo",
            get_dos_filename("foo", None, None, None, None).unwrap()
        );
    }

    /// ours: the real caller's arguments (comm.py:1575): whitelisted `gco`/`g`, existing names
    #[test]
    fn ours_get_dos_filename_as_comm_calls_it() {
        let existing = v(&["/test12~1.gco"]);
        assert_eq!(
            "test12~2.gco",
            get_dos_filename(
                "test12345.gcode",
                Some(&existing),
                Some("gco"),
                Some(&["gco", "g"]),
                None
            )
            .unwrap()
        );
    }

    /// ours: the pieces of `os.path`
    #[test]
    fn ours_splitext_basename() {
        assert_eq!(("a/b", ".c"), splitext("a/b.c"));
        assert_eq!((".bashrc", ""), splitext(".bashrc"));
        assert_eq!(("a.b/c", ""), splitext("a.b/c"));
        assert_eq!(("..x", ""), splitext("..x"));
        assert_eq!(("x.tar", ".gz"), splitext("x.tar.gz"));
        assert_eq!("c", basename("a/b/c"));
        assert_eq!("", basename("a/b/"));
    }
}
