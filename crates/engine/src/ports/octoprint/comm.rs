//! Port of OctoPrint 1.11.8 `src/octoprint/util/comm.py`.
//!
//! Original: Copyright (C) 2013 David Braam, Gina Häußge & others, AGPL-3.0
//! (`__author__ = "Gina Häußge <osd@foosel.net> based on work by David Braam"`).
//! Translated section by section; each item names its source lines.
#![allow(non_upper_case_globals)]

use super::filemanager::{FileDestinations, valid_file_type};
use super::files::{basename, get_dos_filename, m20_timestamp_to_unix_timestamp, splitext};
use super::py_queue::{
    Deque, Empty, GetError, PutError, Queue, QueueStorage, ValueError as PyValueError,
};
#[cfg(test)]
use super::py_threading::LockExt;
use super::py_threading::{Event, blocking_point, time_sleep};
use super::settings::settings;
use super::util::{
    PREPEND_DEFAULT_TIMEOUT, PrependableQueue, PrependableStorage, TypeAlreadyInQueue, TypedQueue,
    chunks, dict_merge, filter_non_ascii, filter_non_utf8, get_bom, get_exception_string,
    to_unicode,
};
use std::cell::RefCell;
use std::collections::HashSet;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;
use std::hash::Hash;
use std::ops::Deref;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

use fancy_regex::Regex;

// ---------------------------------------------------------------------------------------------
// Small helpers standing in for Python built-ins (str.strip, str.split, int(), float(), ...).
// They exist only so that the ported code below can behave as the Python built-in does.
// ---------------------------------------------------------------------------------------------

/// Python `str.isspace()` for one character (Rust's `char::is_whitespace` plus U+001C..U+001F).
fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python `str.strip()` without arguments.
pub(crate) fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

/// Python `str.rstrip()` without arguments.
pub(crate) fn py_rstrip(s: &str) -> &str {
    s.trim_end_matches(py_isspace)
}

/// Python `str.split(None, maxsplit)`.
pub(crate) fn py_split_none(s: &str, maxsplit: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    loop {
        let t = rest.trim_start_matches(py_isspace);
        if t.is_empty() {
            break;
        }
        if out.len() == maxsplit {
            out.push(t);
            break;
        }
        let end = t.find(py_isspace).unwrap_or(t.len());
        out.push(&t[..end]);
        rest = &t[end..];
    }
    out
}

/// Python `int(s)` (base 10): surrounding whitespace, one sign, digits with single underscores
/// between them. `None` stands for `ValueError`.
/// Python's `int()` and `float()` read any Unicode decimal digit (`unicodedata.decimal`): each
/// becomes its ASCII digit first. The blocks are Python's own (`py_decimal_zeros.txt`, written by
/// `scripts/octoprint-patterns.py`).
pub(crate) fn py_ascii_digits(s: &str) -> std::borrow::Cow<'_, str> {
    static ZEROS: LazyLock<Vec<u32>> = LazyLock::new(|| {
        include_str!("py_decimal_zeros.txt")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .flat_map(|l| {
                l.split_whitespace()
                    .map(|z| u32::from_str_radix(z, 16).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect()
    });
    if s.is_ascii() {
        return std::borrow::Cow::Borrowed(s);
    }
    std::borrow::Cow::Owned(
        s.chars()
            .map(|c| {
                let cp = c as u32;
                match ZEROS.iter().rev().find(|z| **z <= cp && cp - **z < 10) {
                    Some(z) => char::from(b'0' + (cp - z) as u8),
                    None => c,
                }
            })
            .collect(),
    )
}

pub(crate) fn py_int(s: &str) -> Option<i64> {
    let s = py_ascii_digits(s);
    let s = py_strip(&s);
    let (neg, digits) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let cleaned: String = digits.chars().filter(|c| *c != '_').collect();
    if !cleaned.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let v: i64 = cleaned.parse::<i64>().ok().or_else(|| {
        // i64::MIN has no positive counterpart
        if neg && cleaned == "9223372036854775808" {
            Some(i64::MIN)
        } else {
            None
        }
    })?;
    Some(if neg && v != i64::MIN { -v } else { v })
}

/// Python `float(s)`. `None` stands for `ValueError`.
pub(crate) fn py_float(s: &str) -> Option<f64> {
    let s = py_ascii_digits(s);
    let s = py_strip(&s);
    if s.contains('_') {
        // Python allows single underscores between digits
        if s.starts_with('_')
            || s.ends_with('_')
            || s.contains("__")
            || s.contains("_.")
            || s.contains("._")
        {
            return None;
        }
        return s.replace('_', "").parse::<f64>().ok();
    }
    s.parse::<f64>().ok()
}

/// Python `repr()` of a `bytes` object (needed because `str.format` of bytes yields its repr).
fn py_bytes_repr(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') {
        b'"'
    } else {
        b'\''
    };
    let mut out = String::new();
    out.push('b');
    out.push(quote as char);
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c as char);
            }
            0x20..=0x7e => out.push(c as char),
            c => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push(quote as char);
    out
}

/// A Python value that is either `str` or `bytes` (the reference's own test passes bytes where
/// the real caller passes str).
#[derive(Debug, Clone, PartialEq)]
pub enum PyText {
    Str(String),
    Bytes(Vec<u8>),
}

impl PyText {
    /// What `"{}".format(value)` gives in Python: the string itself, or the `repr` of bytes.
    pub fn format(&self) -> String {
        match self {
            PyText::Str(s) => s.clone(),
            PyText::Bytes(b) => py_bytes_repr(b),
        }
    }

    /// `re.compile(value)`. A bytes pattern must be valid UTF-8 here (fancy_regex has no
    /// bytes mode).
    fn compile(&self) -> Result<Regex, String> {
        match self {
            PyText::Str(s) => Regex::new(s).map_err(|e| e.to_string()),
            PyText::Bytes(b) => match std::str::from_utf8(b) {
                Ok(s) => Regex::new(s).map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            },
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Stand-ins for things outside comm.py that the ported functions need.
// ---------------------------------------------------------------------------------------------

/// Python's `ValueError`.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueError(pub &'static str);

// ---------------------------------------------------------------------------------------------
// comm.py:60-190 module level
// ---------------------------------------------------------------------------------------------

// a bunch of regexes we'll need for the communication parsing...

/// comm.py:64 regex_float_pattern
pub const regex_float_pattern: &str = r"[-+]?[0-9]*\.?[0-9]+";
/// comm.py:65 regex_positive_float_pattern
pub const regex_positive_float_pattern: &str = r"[+]?[0-9]*\.?[0-9]+";
/// comm.py:66 regex_int_pattern
pub const regex_int_pattern: &str = r"\d+";

fn compile(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|e| panic!("pattern {pattern:?} does not compile: {e}"))
}

/// comm.py:68 regex_command
///
/// Regex for a GCODE command.
pub static regex_command: LazyLock<Regex> = LazyLock::new(|| {
    compile(r"^\s*((?P<codeGM>[GM]\d+)(\.(?P<subcode>\d+))?|(?P<codeT>T)\d+|(?P<codeF>F)\d+)")
});

/// comm.py:73 regex_float
///
/// Regex for a float value.
pub static regex_float: LazyLock<Regex> = LazyLock::new(|| compile(regex_float_pattern));

/// comm.py:76 regexes_parameters
///
/// Regexes for parsing various GCODE command parameters.
pub static regexes_parameters: LazyLock<BTreeMap<&'static str, Regex>> = LazyLock::new(|| {
    let mut m = BTreeMap::new();
    m.insert(
        "floatE",
        compile(&format!(
            r"(^|[^A-Za-z])[Ee](?P<value>{regex_float_pattern})"
        )),
    );
    m.insert(
        "floatF",
        compile(&format!(
            r"(^|[^A-Za-z])[Ff](?P<value>{regex_float_pattern})"
        )),
    );
    m.insert(
        "floatP",
        compile(&format!(
            r"(^|[^A-Za-z])[Pp](?P<value>{regex_float_pattern})"
        )),
    );
    m.insert(
        "floatR",
        compile(&format!(
            r"(^|[^A-Za-z])[Rr](?P<value>{regex_float_pattern})"
        )),
    );
    m.insert(
        "floatS",
        compile(&format!(
            r"(^|[^A-Za-z])[Ss](?P<value>{regex_float_pattern})"
        )),
    );
    m.insert(
        "floatX",
        compile(&format!(
            r"(^|[^A-Za-z])[Xx](?P<value>{regex_float_pattern})"
        )),
    );
    m.insert(
        "floatY",
        compile(&format!(
            r"(^|[^A-Za-z])[Yy](?P<value>{regex_float_pattern})"
        )),
    );
    m.insert(
        "floatZ",
        compile(&format!(
            r"(^|[^A-Za-z])[Zz](?P<value>{regex_float_pattern})"
        )),
    );
    m.insert(
        "intN",
        compile(&format!(r"(^|[^A-Za-z])[Nn](?P<value>{regex_int_pattern})")),
    );
    m.insert(
        "intS",
        compile(&format!(r"(^|[^A-Za-z])[Ss](?P<value>{regex_int_pattern})")),
    );
    m.insert(
        "intT",
        compile(&format!(r"(^|[^A-Za-z])[Tt](?P<value>{regex_int_pattern})")),
    );
    m
});

/// comm.py:91 regex_minMaxError
///
/// Regex matching first line of min/max errors from the firmware.
pub static regex_minMaxError: LazyLock<Regex> = LazyLock::new(|| compile(r"Error:[0-9]\n"));

/// comm.py:94 regex_marlinKillError
///
/// Regex matching first line of kill causing errors from Marlin.
pub static regex_marlinKillError: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        r"Heating failed|Thermal Runaway|Thermal Malfunction|MAXTEMP triggered|MINTEMP triggered|Invalid extruder number|Watchdog barked|KILL caused",
    )
});

/// comm.py:99 regex_sdPrintingByte
///
/// Regex matching SD printing status reports.
///
/// Groups will be as follows:
///
///   * ``current``: current byte position in file being printed
///   * ``total``: total size of file being printed
pub static regex_sdPrintingByte: LazyLock<Regex> =
    LazyLock::new(|| compile(r"(?P<current>[0-9]+)/(?P<total>[0-9]+)"));

/// comm.py:108 regex_sdFileOpened
///
/// Regex matching "File opened" messages from the firmware.
///
/// Groups will be as follows:
///
///   * ``name``: name of the file reported as having been opened (str)
///   * ``size``: size of the file in bytes (int)
pub static regex_sdFileOpened: LazyLock<Regex> = LazyLock::new(|| {
    compile(&format!(
        r"File opened:\s*(?P<name>.*?)\s+Size:\s*(?P<size>{regex_int_pattern})"
    ))
});

/// comm.py:119 regex_temp
///
/// Regex matching temperature entries in line.
///
/// Groups will be as follows:
///
///   * ``sensor``: whole sensor designator, incl. optional ``toolnum``, e.g. "T1", "B",
///     "C" or anything custom (str)
///   * ``toolnum``: tool number, if provided, only for T0, T1, etc (int)
///   * ``actual``: actual temperature (float)
///   * ``target``: target temperature, if provided (float)
pub static regex_temp: LazyLock<Regex> = LazyLock::new(|| {
    compile(&format!(
        r"(^|\s)(?P<sensor>B|C|T(?P<toolnum>\d*)|([\w]+)):\s*(?P<actual>{0})(\s*\/?\s*(?P<target>{1}))?",
        regex_float_pattern, regex_float_pattern
    ))
});

/// comm.py:134 regex_repetierTempExtr
///
/// Regex for matching target temp reporting from Repetier.
///
/// Groups will be as follows:
///
///   * ``toolnum``: number of the extruder to which the target temperature
///     report belongs (int)
///   * ``target``: new target temperature (float)
pub static regex_repetierTempExtr: LazyLock<Regex> = LazyLock::new(|| {
    compile(&format!(
        r"TargetExtr(?P<toolnum>\d+):(?P<target>{regex_float_pattern})"
    ))
});

/// comm.py:146 regex_repetierTempBed
///
/// Regex for matching target temp reporting from Repetier for beds.
///
/// Groups will be as follows:
///
///   * ``target``: new target temperature (float)
pub static regex_repetierTempBed: LazyLock<Regex> =
    LazyLock::new(|| compile(&format!(r"TargetBed:(?P<target>{regex_float_pattern})")));

/// comm.py:154 regex_position
///
/// Regex for matching position reporting.
///
/// Groups will be as follows:
///
///   * ``x``: X coordinate
///   * ``y``: Y coordinate
///   * ``z``: Z coordinate
///   * ``e``: E coordinate if present, or
///   * ``es``: multiple E coordinates if present, to be parsed further with regex_e_positions
pub static regex_position: LazyLock<Regex> = LazyLock::new(|| {
    compile(&format!(
        r"X:\s*(?P<x>{float})\s*Y:\s*(?P<y>{float})\s*Z:\s*(?P<z>{float})\s*((E:\s*(?P<e>{float}))|(?P<es>(E\d+:\s*{float}\s*)+))",
        float = regex_float_pattern
    ))
});

/// comm.py:170 regex_e_positions
///
/// Regex for matching multiple E coordinates in a position report.
///
/// Groups will be as follows:
///
///   * ``id``: id of the extruder or which the position is reported
///   * ``value``: reported position value
pub static regex_e_positions: LazyLock<Regex> = LazyLock::new(|| {
    compile(&format!(
        r"E(?P<id>\d+):\s*(?P<value>{regex_float_pattern})"
    ))
});

/// comm.py:179 regex_firmware_splitter
///
/// Regex to use for splitting M115 responses.
pub static regex_firmware_splitter: LazyLock<Regex> =
    LazyLock::new(|| compile(r"(^|\s+)([A-Z][A-Z0-9_]*):"));

/// comm.py:182 regex_resend_linenumber
///
/// Regex to use for request line numbers in resend requests
pub static regex_resend_linenumber: LazyLock<Regex> =
    LazyLock::new(|| compile(&format!(r"(N|N:)?(?P<n>{regex_int_pattern})")));

/// comm.py:185 regex_serial_devices
///
/// Regex used to filter out valid tty devices
pub static regex_serial_devices: LazyLock<Regex> =
    LazyLock::new(|| compile(r"^(?:ttyUSB|ttyACM|tty\.usb|cu\.|cuaU|ttyS|rfcomm).*"));

/// comm.py:189 MINIMAL_SD_TIMESTAMP
///
/// May 29th 2008 = RepRap 1.0 "Darwin" achieves self-replication
pub const MINIMAL_SD_TIMESTAMP: i64 = 1212012000;

/// comm.py:6207 _temp_command_regex
pub static _temp_command_regex: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        r"^M(?P<command>104|109|140|190|141|191)(\s+T(?P<tool>\d+)|\s+[SR](?P<temperature>[-+]?\d*\.?\d*))+",
    )
});

fn re_captures<'t>(re: &Regex, text: &'t str) -> Option<fancy_regex::Captures<'t>> {
    re.captures(text).expect("regex runtime error")
}

// ---------------------------------------------------------------------------------------------
// comm.py:6212 ff. module level functions
// ---------------------------------------------------------------------------------------------

/// `offsets` of `apply_temperature_offsets`: a dict mapping `"tool0"`, `"tool1"`, ..., `"bed"`,
/// `"chamber"` to a number.
pub type TemperatureOffsets = BTreeMap<String, f64>;

/// comm.py:6212 apply_temperature_offsets
pub fn apply_temperature_offsets(
    line: &str,
    offsets: Option<&TemperatureOffsets>,
    current_tool: Option<i64>,
) -> String {
    let offsets = match offsets {
        None => return line.to_string(),
        Some(o) => o,
    };

    let captures = match re_captures(&_temp_command_regex, line) {
        None => return line.to_string(),
        Some(c) => c,
    };

    // groups = match.groupdict()
    let g_command = captures.name("command").map(|m| m.as_str());
    let g_tool = captures.name("tool").map(|m| m.as_str());
    let g_temperature = captures.name("temperature");
    match g_temperature {
        None => return line.to_string(),
        Some(m) if m.as_str().is_empty() => return line.to_string(),
        _ => {}
    }
    let g_temperature = g_temperature.unwrap();
    let command = g_command.unwrap();

    let mut offset: f64 = 0.0;
    if current_tool.is_some() && (command == "104" || command == "109") {
        // extruder temperature, determine which one and retrieve corresponding offset
        let mut tool_num = current_tool.unwrap();
        if let Some(tool) = g_tool {
            tool_num = py_int(tool).expect("int() of \\d+");
        }

        let tool_key = format!("tool{tool_num}");
        offset = match offsets.get(&tool_key) {
            Some(v) if *v != 0.0 => *v,
            _ => 0.0,
        };
    } else if command == "140" || command == "190" {
        // bed temperature
        offset = offsets.get("bed").copied().unwrap_or(0.0);
    } else if command == "141" || command == "191" {
        // chamber temperature
        offset = offsets.get("chamber").copied().unwrap_or(0.0);
    }

    if offset == 0.0 {
        return line.to_string();
    }

    let temperature = match py_float(g_temperature.as_str()) {
        Some(t) => t,
        None => {
            // _logger.warning(f"Could not parse target temperature, ignoring line for offset
            // application: {line}")  (no logging in this crate)
            return line.to_string();
        }
    };

    if temperature == 0.0 {
        return line.to_string();
    }

    format!(
        "{}{}{}",
        &line[..g_temperature.start()],
        format!("{:.6}", temperature + offset),
        &line[g_temperature.end()..]
    )
}

/// comm.py:6263 strip_comment
pub fn strip_comment(line: &str) -> String {
    if !line.contains(';') {
        // shortcut
        return line.to_string();
    }

    let mut escaped = false;
    let mut result = String::new();
    for c in line.chars() {
        if c == ';' && !escaped {
            break;
        }
        result.push(c);
        escaped = (c == '\\') && !escaped;
    }
    result
}

/// comm.py:6278 process_gcode_line
pub fn process_gcode_line(
    line: &str,
    offsets: Option<&TemperatureOffsets>,
    current_tool: Option<i64>,
) -> Option<String> {
    let stripped = strip_comment(line);
    let mut line = py_strip(&stripped).to_string();
    if line.is_empty() {
        return None;
    }

    if offsets.is_some() {
        line = apply_temperature_offsets(&line, offsets, current_tool);
    }

    Some(line)
}

/// comm.py:6289 convert_pause_triggers
///
/// Each configured trigger is a dict; here a map from key (`"regex"`, `"type"`) to string value.
/// Returns `Err` where Python raises `re.error` from the final `re.compile` of the joined
/// patterns. The result maps `"enable"`, `"disable"`, `"toggle"` to the compiled regex (Python
/// keeps insertion order, a `BTreeMap` sorts the keys).
pub fn convert_pause_triggers(
    configured_triggers: &[BTreeMap<String, String>],
) -> Result<BTreeMap<String, Regex>, fancy_regex::Error> {
    if configured_triggers.is_empty() {
        return Ok(BTreeMap::new());
    }

    let mut triggers: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    triggers.insert("enable", Vec::new());
    triggers.insert("disable", Vec::new());
    triggers.insert("toggle", Vec::new());
    for trigger in configured_triggers {
        if !trigger.contains_key("regex") || !trigger.contains_key("type") {
            continue;
        }

        let regex = &trigger["regex"];
        let t = &trigger["type"];
        if let Some(list) = triggers.get_mut(t.as_str()) {
            // make sure regex is valid
            match Regex::new(regex) {
                Ok(_) => {
                    // add to type list
                    list.push(regex.clone());
                }
                Err(_exc) => {
                    // invalid regex or something like this
                    // _logger.debug("Problem with trigger %r: %s", trigger, str(exc))
                    // (no logging in this crate)
                }
            }
        }
    }

    let mut result = BTreeMap::new();
    for t in ["enable", "disable", "toggle"] {
        if !triggers[t].is_empty() {
            let joined = triggers[t]
                .iter()
                .map(|p| format!("({p})"))
                .collect::<Vec<_>>()
                .join("|");
            result.insert(t.to_string(), Regex::new(&joined)?);
        }
    }
    Ok(result)
}

/// One entry of the `configured_controls` list given to `convert_feedback_controls`
/// (a Python dict; a key that is absent is `None`).
#[derive(Debug, Clone, Default)]
pub struct FeedbackControl {
    pub key: Option<String>,
    pub regex: Option<PyText>,
    pub template: Option<PyText>,
    pub template_key: Option<String>,
    pub children: Option<Vec<FeedbackControl>>,
}

/// `prepare_result_entry()` of `convert_feedback_controls`: `{"pattern": None, "matcher": None,
/// "templates": {}}`.
#[derive(Debug, Default)]
pub struct FeedbackControlEntry {
    pub pattern: Option<PyText>,
    pub matcher: Option<Regex>,
    pub templates: PyDict<PyText>,
}

/// The `defaultdict(prepare_result_entry)` of `convert_feedback_controls`, keeping Python's
/// insertion order.
#[derive(Debug, Default)]
pub struct FeedbackControls(pub Vec<(String, FeedbackControlEntry)>);

impl FeedbackControls {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }

    pub fn get(&self, key: &str) -> Option<&FeedbackControlEntry> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// `defaultdict.__getitem__`: creates the entry if missing.
    fn entry(&mut self, key: &str) -> &mut FeedbackControlEntry {
        let idx = match self.0.iter().position(|(k, _)| k == key) {
            Some(i) => i,
            None => {
                self.0
                    .push((key.to_string(), FeedbackControlEntry::default()));
                self.0.len() - 1
            }
        };
        &mut self.0[idx].1
    }
}

/// comm.py:6317 convert_feedback_controls
///
/// Returns `(feedback_controls, feedback_matcher)`; `Err` where Python raises `re.error` from
/// the final `re.compile`.
pub fn convert_feedback_controls(
    configured_controls: &[FeedbackControl],
) -> Result<(FeedbackControls, Option<Regex>), fancy_regex::Error> {
    if configured_controls.is_empty() {
        return Ok((FeedbackControls::default(), None));
    }

    /// comm.py:6321 preprocess_feedback_control (nested in convert_feedback_controls)
    fn preprocess_feedback_control(control: &FeedbackControl, result: &mut FeedbackControls) {
        if let (Some(key), Some(regex), Some(template)) =
            (&control.key, &control.regex, &control.template)
        {
            // key is always the md5sum of the regex
            let entry = result.entry(key);

            if entry.pattern.is_none() || entry.matcher.is_none() {
                // regex has not been registered
                match regex.compile() {
                    Ok(matcher) => {
                        entry.matcher = Some(matcher);
                        entry.pattern = Some(regex.clone());
                    }
                    Err(_exc) => {
                        // _logger.warning("Invalid regex {regex} for custom control: {exc}")
                        // (no logging in this crate)
                    }
                }
            }

            let template_key = control
                .template_key
                .as_ref()
                .expect("KeyError: 'template_key'");
            result
                .entry(key)
                .templates
                .insert(template_key, template.clone());
        } else if let Some(children) = &control.children {
            for c in children {
                preprocess_feedback_control(c, result);
            }
        }
    }

    let mut feedback_controls = FeedbackControls::default();

    for control in configured_controls {
        preprocess_feedback_control(control, &mut feedback_controls);
    }

    let mut feedback_pattern: Vec<String> = Vec::new();
    for (match_key, entry) in &feedback_controls.0 {
        let (matcher, pattern) = match (&entry.matcher, &entry.pattern) {
            (Some(m), Some(p)) => (m, p),
            _ => continue,
        };
        let _ = matcher;
        feedback_pattern.push(format!("(?P<group{}>{})", match_key, pattern.format()));
    }
    let feedback_matcher = Regex::new(&feedback_pattern.join("|"))?;

    Ok((feedback_controls, Some(feedback_matcher)))
}

/// `parsed`/`result` of `canonicalize_temperatures` and `parse_temperature_line`: sensor key to
/// `(actual, target)`, in Python's dict (insertion) order.
pub type Temperatures = PyDict<(f64, Option<f64>)>;

/// comm.py:6366 canonicalize_temperatures
///
/// Canonicalizes the temperatures provided in parsed.
///
/// Will make sure that returned result only contains extruder keys
/// like Tn, so always qualified with a tool number.
///
/// The algorithm for cleaning up the parsed keys is the following:
///
///   * If ``T`` is not included with the reported extruders, return
///   * If more than just ``T`` is reported:
///     * If both ``T`` and ``T0`` are reported, remove ``T`` from
///       the result.
///     * Else set ``T0`` to ``T`` and delete ``T`` (Smoothie extra).
///   * If only ``T`` is reported, set ``Tc`` to ``T`` and delete ``T``
///   * return
///
/// Arguments:
///     parsed (dict): the parsed temperatures (mapping tool => (actual, target))
///       to canonicalize
///     current (int): the current active extruder
/// Returns:
///     dict: the canonicalized version of ``parsed``
pub fn canonicalize_temperatures(parsed: &Temperatures, current: i64) -> Temperatures {
    let reported_extruders: Vec<&String> = parsed.keys().filter(|x| x.starts_with('T')).collect();
    if !reported_extruders.iter().any(|k| k.as_str() == "T") {
        // Our reported_extruders are either empty or consist purely
        // of Tn keys, no need for any action
        return parsed.clone();
    }

    let current_tool_key = format!("T{current}");
    let mut result = parsed.clone();

    if reported_extruders.len() > 1 {
        if reported_extruders.iter().any(|k| k.as_str() == "T0") {
            // Both T and T0 are present, let's check if Tc is too.
            // If it is, we just throw away T (it's redundant). It
            // it isn't, we first copy T to Tc, then throw T away.
            //
            // The easier construct would be to always overwrite Tc
            // with T and throw away T, but that assumes that if
            // both are present, T has the same value as Tc. That
            // might not necessarily be the case (weird firmware)
            // so we err on the side of caution here and trust Tc
            // over T.
            if !reported_extruders.iter().any(|k| **k == current_tool_key) {
                // T and T0 are present, but Tc is missing - copy
                // T to Tc
                let t = *result.get("T").expect("KeyError: 'T'");
                result.insert(&current_tool_key, t);
            }
            // throw away T, it's redundant (now)
            result.pop("T");
        } else {
            // So T is there, but T0 isn't. That looks like Smoothieware which
            // always reports the first extruder T0 as T:
            //
            //     T:<T0> T1:<T1> T2:<T2> ... B:<B>
            //
            // becomes
            //
            //     T0:<T0> T1:<T1> T2:<T2> ... B:<B>
            let t = *result.get("T").expect("KeyError: 'T'");
            result.insert("T0", t);
            result.pop("T");
        }
    } else {
        // We only have T. That can mean two things:
        //
        //   * we only have one extruder at all, or
        //   * we are currently parsing a response to M109/M190, which on
        //     some firmwares doesn't report the full M105 output while
        //     waiting for the target temperature to be reached but only
        //     reports the current tool and bed
        //
        // In both cases it is however safe to just move our T over
        // to Tc in the parsed data, current should always stay
        // 0 for single extruder printers. E.g. for current_tool == 1:
        //
        //     T:<T1>
        //
        // becomes
        //
        //     T1:<T1>

        let t = *result.get("T").expect("KeyError: 'T'");
        result.insert(&current_tool_key, t);
        result.pop("T");
    }

    result
}

/// comm.py:6455 _validate_m20_timestamp
pub fn _validate_m20_timestamp(timestamp: &str) -> bool {
    match m20_timestamp_to_unix_timestamp(timestamp) {
        Err(ValueError(_)) => false,
        Ok(_) => true,
    }
}

/// comm.py:6463 parse_file_list_line
///
/// Returns `(filename, size, timestamp, longname)`.
pub fn parse_file_list_line(line: &str) -> (String, Option<i64>, Option<i64>, Option<String>) {
    let mut longname: Option<String> = None;
    let mut size: Option<String> = None;
    let mut timestamp: Option<String> = None;
    let filename: String;
    let fileinfo = py_split_none(line, 3);
    if fileinfo.len() == 4 {
        // name, size, timestamp, long name or name, size, long name with spaces
        filename = fileinfo[0].to_string();
        size = Some(fileinfo[1].to_string());
        timestamp = Some(fileinfo[2].to_string());
        longname = Some(fileinfo[3].to_string());
        if !_validate_m20_timestamp(timestamp.as_deref().unwrap()) {
            // longname with spaces. Split line again with twice split limit to get longname right in case
            // of multiple whitespace characters in it.
            let again = py_split_none(line, 2);
            return parse_file_list_line_tail(
                again[0].to_string(),
                Some(again[1].to_string()),
                None,
                Some(again[2].to_string()),
                line,
            );
        }
    } else if fileinfo.len() == 3 {
        // name, size, long name or name, size, timestamp
        filename = fileinfo[0].to_string();
        size = Some(fileinfo[1].to_string());
        let third = fileinfo[2];
        if _validate_m20_timestamp(third) {
            timestamp = Some(third.to_string());
        } else {
            longname = Some(third.to_string());
        }
    } else if fileinfo.len() == 2 {
        // name, size
        filename = fileinfo[0].to_string();
        size = Some(fileinfo[1].to_string());
    } else {
        // name
        filename = line.to_string();
    }

    parse_file_list_line_tail(filename, size, timestamp, longname, line)
}

/// The second half of `parse_file_list_line` (comm.py:6491 `if size is not None:` to the
/// `return`), split out so the "longname with spaces" branch can continue with the re-split
/// values as Python does.
fn parse_file_list_line_tail(
    mut filename: String,
    size_str: Option<String>,
    timestamp_str: Option<String>,
    mut longname: Option<String>,
    line: &str,
) -> (String, Option<i64>, Option<i64>, Option<String>) {
    let mut size: Option<i64> = None;
    let mut timestamp: Option<i64> = None;

    if let Some(size_str) = size_str {
        match py_int(&size_str) {
            None => {
                // whatever that was, it was not an integer, so we'll just use the whole line as filename and set size/timestamp/longname to None
                filename = line.to_string();
                size = None;
                timestamp = None;
                longname = None;
            }
            Some(n) => {
                size = Some(n);
                if let Some(ts) = timestamp_str {
                    // size was valid and we have timestamp, so try to use it
                    timestamp = m20_timestamp_to_unix_timestamp(&ts).ok();
                }
            }
        }
    }

    if let Some(l) = &longname {
        if l.starts_with('"') && l.ends_with('"') {
            // apparently some firmwares enclose the long name in quotes...
            // (longname[1:-1])
            longname = Some(if l.len() >= 2 {
                l[1..l.len() - 1].to_string()
            } else {
                String::new()
            });
        }
    }

    (filename, size, timestamp, longname)
}

/// comm.py:6515 parse_temperature_line
///
/// Parses the provided temperature line.
///
/// The result will be a dictionary mapping from the extruder or bed key to
/// a tuple with current and target temperature. The result will be canonicalized
/// with :func:`canonicalize_temperatures` before returning.
///
/// Arguments:
///     line (str): the temperature line to parse
///     current (int): the current active extruder
///
/// Returns:
///     tuple: a 2-tuple with the maximum tool number and a dict mapping from
///       key to (actual, target) tuples, with key either matching ``Tn`` for ``n >= 0`` or ``B``
pub fn parse_temperature_line(line: &str, current: i64) -> (i64, Temperatures) {
    let mut result = Temperatures::new();
    let mut max_tool_num: i64 = 0;
    for captures in regex_temp.captures_iter(line) {
        let captures = captures.expect("regex runtime error");
        let sensor = captures.name("sensor").unwrap().as_str().to_string();
        if result.contains_key(&sensor) {
            // sensor already seen, let's not overwrite stuff
            continue;
        }

        let toolnum = captures.name("toolnum").map(|m| m.as_str());
        let tool_number = match toolnum {
            Some(t) if !t.is_empty() => Some(py_int(t).expect("int() of \\d*")),
            _ => None,
        };
        if let Some(tn) = tool_number {
            if tn != 0 && tn > max_tool_num {
                max_tool_num = tn;
            }
        }

        // try:
        let actual = py_float(captures.name("actual").unwrap().as_str());
        if let Some(actual) = actual {
            let mut target = None;
            let mut ok = true;
            if let Some(t) = captures.name("target") {
                if !t.as_str().is_empty() {
                    match py_float(t.as_str()) {
                        Some(v) => target = Some(v),
                        None => ok = false,
                    }
                }
            }
            if ok {
                result.insert(&sensor, (actual, target));
            }
        }
        // except ValueError: catch conversion issues, we'll rather just not get the temperature
        // update instead of killing the connection
    }

    (
        max_tool_num.max(current),
        canonicalize_temperatures(&result, current),
    )
}

/// comm.py:6560 parse_firmware_line
///
/// Parses the provided firmware info line.
///
/// The result will be a dictionary mapping from the contained keys to the contained
/// values.
///
/// Valid keys must only contain A-Z, 0-9 and _ and must start with a letter. See the
/// unit tests for valid and invalid examples. There sadly is no existing specification
/// of the key format, but this is the format extracted from real life logs.
///
/// Arguments:
///     line (str): the line to parse
///
/// Returns:
///     dict: a dictionary with the parsed data
pub fn parse_firmware_line(line: &str) -> PyDict<String> {
    let line: String = if line.starts_with("NAME.") {
        // Good job Malyan. Why use a : when you can also just use a ., right? Let's revert that.
        format!("NAME:{}", &line[5..])
    } else {
        line.to_string()
    };

    // a `dict`: the key order is the order of the firmware line (a repeated key keeps its first place)
    let mut result = PyDict::new();
    // regex_firmware_splitter.split(line.strip())[1:]: Python's re.split also returns the
    // captured groups, so every match contributes (group 1, group 2) after the text before it.
    let stripped = py_strip(&line);
    let mut split_line: Vec<String> = Vec::new();
    let mut last = 0;
    for captures in regex_firmware_splitter.captures_iter(stripped) {
        let captures = captures.expect("regex runtime error");
        let whole = captures.get(0).unwrap();
        split_line.push(stripped[last..whole.start()].to_string());
        split_line.push(
            captures
                .get(1)
                .map(|m| m.as_str())
                .unwrap_or("")
                .to_string(),
        );
        split_line.push(
            captures
                .get(2)
                .map(|m| m.as_str())
                .unwrap_or("")
                .to_string(),
        );
        last = whole.end();
    }
    split_line.push(stripped[last..].to_string());
    let split_line = &split_line[1..]; // first entry is empty start of trimmed string
    for chunk in chunks(split_line, 3) {
        let (_, key, value) = (&chunk[0], &chunk[1], &chunk[2]);
        result.insert(key, py_strip(value).to_string());
    }
    result
}

/// comm.py:6593 parse_capability_line
///
/// Parses the provided firmware capability line.
///
/// Lines are expected to be of the format
///
/// ```text
///     Cap:<capability name in caps>:<0 or 1>
/// ```
///
/// e.g.
///
/// ```text
///     Cap:AUTOREPORT_TEMP:1
///     Cap:TOGGLE_LIGHTS:0
/// ```
///
/// Args:
///         line (str): the line to parse
///
/// Returns:
///         tuple: a 2-tuple of the parsed capability name and whether it's on (true) or off (false), or None if the line
///             could not be parsed
pub fn parse_capability_line(line: &str) -> Option<(String, bool)> {
    let mut line = line.to_lowercase();
    if line.starts_with("cap:") {
        line = line["cap:".len()..].to_string();
    }

    let parts: Vec<&str> = line.split(':').collect();
    if parts.len() != 2 {
        // wrong format, can't parse this
        return None;
    }

    let (capability, flag) = (parts[0], parts[1]);
    if flag != "0" && flag != "1" {
        // wrong format, can't parse this
        return None;
    }

    Some((capability.to_uppercase(), flag == "1"))
}

/// comm.py:6631 parse_resend_line
///
/// Parses the provided resend line and returns requested line number.
///
/// Args:
///         line (str): the line to parse
///
/// Returns:
///         int or None: the extracted line number to resend, or None if no number could be extracted
pub fn parse_resend_line(line: &str) -> Option<i64> {
    let captures = re_captures(&regex_resend_linenumber, line);
    if let Some(captures) = captures {
        return Some(py_int(captures.name("n").unwrap().as_str()).expect("int() of \\d+"));
    }

    None
}

/// comm.py:6649 parse_position_line
///
/// Parses the provided M114 response line and returns the parsed coordinates.
///
/// Args:
///         line (str): the line to parse
///
/// Returns:
///         dict or None: the parsed coordinates, or None if no coordinates could be parsed
pub fn parse_position_line(line: &str) -> Option<BTreeMap<String, f64>> {
    let captures = re_captures(&regex_position, line);
    if let Some(captures) = captures {
        let f = |name: &str| {
            py_float(captures.name(name).unwrap().as_str()).expect("float() of float pattern")
        };
        let mut result = BTreeMap::new();
        result.insert("x".to_string(), f("x"));
        result.insert("y".to_string(), f("y"));
        result.insert("z".to_string(), f("z"));
        if captures.name("e").is_some() {
            // report contains only one E
            result.insert("e".to_string(), f("e"));
        } else if let Some(es) = captures.name("es") {
            // report contains individual entries for multiple extruders ("E0:... E1:... E2:...")
            let es = es.as_str();
            for m in regex_e_positions.captures_iter(es) {
                let m = m.expect("regex runtime error");
                result.insert(
                    format!("e{}", m.name("id").unwrap().as_str()),
                    py_float(m.name("value").unwrap().as_str()).expect("float() of float pattern"),
                );
            }
        } else {
            // apparently no E at all, should never happen but let's still handle this
            return None;
        }

        return Some(result);
    }

    None
}

/// comm.py:6686 gcode_command_for_cmd
///
/// Tries to parse the provided ``cmd`` and extract the GCODE command identifier from it (e.g. "G0" for "G0 X10.0").
///
/// Arguments:
///     cmd (str): The command to try to parse.
///
/// Returns:
///     str or None: The GCODE command identifier if it could be parsed, or None if not.
pub fn gcode_command_for_cmd(cmd: Option<&str>) -> Option<String> {
    let (gcode, _) = gcode_and_subcode_for_cmd(cmd);
    gcode
}

/// comm.py:6701 gcode_and_subcode_for_cmd
pub fn gcode_and_subcode_for_cmd(cmd: Option<&str>) -> (Option<String>, Option<String>) {
    let cmd = match cmd {
        None => return (None, None),
        Some("") => return (None, None),
        Some(c) => c,
    };

    let captures = match re_captures(&regex_command, cmd) {
        None => return (None, None),
        Some(c) => c,
    };

    // values = match.groupdict(); a group that did not match is None, and so is falsy
    let value = |name: &str| captures.name(name).map(|m| m.as_str().to_string());
    let gcode;
    if let Some(v) = value("codeGM").filter(|v| !v.is_empty()) {
        gcode = v;
    } else if let Some(v) = value("codeT").filter(|v| !v.is_empty()) {
        gcode = v;
    } else if settings().get_boolean(&["serial", "supportFAsCommand"]) == Some(true)
        && value("codeF").is_some_and(|v| !v.is_empty())
    {
        gcode = value("codeF").unwrap();
    } else {
        // this should never happen
        return (None, None);
    }

    (Some(gcode), value("subcode"))
}

/// A set of tags as `_normalize_command_handler_result` handles it. Python's sets are mutable
/// and shared by reference; `Rc<RefCell<..>>` keeps that (an empty set is falsy and therefore
/// not copied, then mutated in place).
pub type Tags = Rc<RefCell<BTreeSet<String>>>;

/// One normalized entry: `(command, command_type, gcode, subcode, tags)`.
pub type NormalizedCommand = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Tags>,
);

/// A tuple returned by a command handler (1, 2 or 3 entries, or any other length).
/// The elements are `command`, `command_type` and (third) a set of tags.
#[derive(Debug, Clone)]
pub enum HandlerTuple {
    One(Option<String>),
    Two(Option<String>, Option<String>),
    Three(Option<String>, Option<String>, Option<BTreeSet<String>>),
    /// a tuple of an unexpected length
    Other,
}

/// One handler result entry.
#[derive(Debug, Clone)]
pub enum HandlerResultEntry {
    None,
    Str(String),
    Tuple(HandlerTuple),
    /// neither `None`, `str` nor `tuple`
    Other,
}

/// What a handler returned: a single entry or a list of entries (`None` is `Option::None`).
#[derive(Debug, Clone)]
pub enum HandlerResults {
    Entry(HandlerResultEntry),
    List(Vec<HandlerResultEntry>),
}

/// comm.py:6727 _normalize_command_handler_result
///
/// Normalizes a command handler result.
///
/// Handler results can be either ``None``, a single result entry or a list of result
/// entries.
///
/// ``None`` results are ignored, the provided ``command``, ``command_type``,
/// ``gcode``, ``subcode`` and ``tags`` are returned in that case (as single-entry list with
/// one 5-tuple as entry).
///
/// Single result entries are either:
///
///   * a single string defining a replacement ``command``
///   * a 1-tuple defining a replacement ``command``
///   * a 2-tuple defining a replacement ``command`` and ``command_type``
///   * a 3-tuple defining a replacement ``command`` and ``command_type`` and additional ``tags`` to set
///
/// A ``command`` that is ``None`` will lead to the entry being ignored for
/// the normalized result.
///
/// The method returns a list of normalized result entries. Normalized result
/// entries always are a 4-tuple consisting of ``command``, ``command_type``,
/// ``gcode`` and ``subcode``, the latter three being allowed to be ``None``. The list may
/// be empty in which case the command is to be suppressed.
///
/// (The examples of the Python docstring are in the tests of this module.)
///
/// Arguments:
///     command (str or None): The command for which the handler result was
///         generated
///     command_type (str or None): The command type for which the handler
///         result was generated
///     gcode (str or None): The GCODE for which the handler result was
///         generated
///     subcode (str or None): The GCODE subcode for which the handler result
///         was generated
///     tags (set of str or None): The tags associated with the GCODE for which
///         the handler result was generated
///     handler_results: The handler result(s) to normalized. Can be either
///         a single result entry or a list of result entries.
///     tags_to_add (set of str or None): List of tags to add to expanded result
///         entries
///
/// Returns:
///     (list) - A list of normalized handler result entries, which are
///         5-tuples consisting of ``command``, ``command_type``, ``gcode``
///         ``subcode`` and ``tags``, the latter three of which may be ``None``.
#[allow(unused_assignments)] // `command = ...` mirrors Python's reassignments
pub fn _normalize_command_handler_result(
    command: Option<String>,
    command_type: Option<String>,
    gcode: Option<String>,
    subcode: Option<String>,
    tags: Option<Tags>,
    handler_results: Option<HandlerResults>,
    tags_to_add: Option<&BTreeSet<String>>,
) -> Vec<NormalizedCommand> {
    let original: NormalizedCommand = (
        command.clone(),
        command_type.clone(),
        gcode.clone(),
        subcode.clone(),
        tags.clone(),
    );
    let (mut command, mut command_type, mut gcode, mut subcode, mut tags) =
        (command, command_type, gcode, subcode, tags);

    let handler_results = match handler_results {
        // handler didn't return anything, we'll just continue
        None => return vec![original],
        Some(HandlerResults::Entry(e)) => vec![e],
        Some(HandlerResults::List(l)) => l,
    };

    let add_nonempty = |tags_to_add: Option<&BTreeSet<String>>| -> Option<BTreeSet<String>> {
        tags_to_add.filter(|t| !t.is_empty()).cloned()
    };

    let mut result: Vec<NormalizedCommand> = Vec::new();
    for handler_result in handler_results {
        // we iterate over all handler result entries and process each one
        // individually here

        if matches!(handler_result, HandlerResultEntry::None) {
            // entry is None, we'll ignore that entry and continue
            continue;
        }

        let copy = match &tags {
            Some(t) if !t.borrow().is_empty() => Some(t.borrow().clone()),
            _ => None,
        };
        if let Some(c) = copy {
            // copy the tags
            tags = Some(Rc::new(RefCell::new(c)));
        }

        match handler_result {
            HandlerResultEntry::Str(s) => {
                // entry is just a string, replace command with it
                command = Some(s);

                if command != original.0 {
                    // command changed, re-extract gcode and subcode and add tags if necessary
                    let (g, sc) = gcode_and_subcode_for_cmd(command.as_deref());
                    gcode = g;
                    subcode = sc;

                    if let Some(add) = add_nonempty(tags_to_add) {
                        if command != original.0 {
                            if tags.is_none() {
                                tags = Some(Rc::new(RefCell::new(BTreeSet::new())));
                            }
                            tags.as_ref().unwrap().borrow_mut().extend(add);
                        }
                    }
                }

                result.push((
                    command.clone(),
                    command_type.clone(),
                    gcode.clone(),
                    subcode.clone(),
                    tags.clone(),
                ));
            }
            HandlerResultEntry::Tuple(handler_result) => {
                // entry is a tuple, extract command and command_type
                let mut handler_tags: Option<BTreeSet<String>> = None;

                match handler_result {
                    HandlerTuple::One(c) => {
                        // handler returned just the command
                        command = c;
                    }
                    HandlerTuple::Two(c, ct) => {
                        // handler returned command and command_type
                        command = c;
                        command_type = ct;
                    }
                    HandlerTuple::Three(c, ct, ht) => {
                        // handler returned command, command type and additional tags
                        command = c;
                        command_type = ct;
                        handler_tags = ht;
                    }
                    HandlerTuple::Other => {
                        // handler returned a tuple of an unexpected length, ignore
                        // and continue
                        continue;
                    }
                }

                if command.is_none() {
                    // command is None, ignore it and continue
                    continue;
                }

                // (the reference compares command_type with original[2], the gcode; ported as is)
                if command != original.0 || command_type != original.2 {
                    // command or command_type changed, re-extract gcode and subcode and add tags if necessary
                    let (g, sc) = gcode_and_subcode_for_cmd(command.as_deref());
                    gcode = g;
                    subcode = sc;

                    if let Some(add) = add_nonempty(tags_to_add) {
                        if tags.is_none() {
                            tags = Some(Rc::new(RefCell::new(BTreeSet::new())));
                        }
                        tags.as_ref().unwrap().borrow_mut().extend(add);
                    }
                }

                if let Some(ht) = handler_tags {
                    if !ht.is_empty() {
                        // handler provided additional tags, add them
                        if tags.is_none() {
                            tags = Some(Rc::new(RefCell::new(BTreeSet::new())));
                        }
                        tags.as_ref().unwrap().borrow_mut().extend(ht);
                    }
                }

                result.push((
                    command.clone(),
                    command_type.clone(),
                    gcode.clone(),
                    subcode.clone(),
                    tags.clone(),
                ));
            }
            HandlerResultEntry::Other | HandlerResultEntry::None => {}
        }

        // reset to original
        command = original.0.clone();
        command_type = original.1.clone();
        gcode = original.2.clone();
        subcode = original.3.clone();
        tags = original.4.clone();
    }

    result
}

// ---------------------------------------------------------------------------------------------
// comm.py:317-441 PositionRecord, TemperatureRecord
// ---------------------------------------------------------------------------------------------

/// comm.py:317 PositionRecord
///
/// Python keeps the coordinates as dynamic attributes (`setattr`/`getattr`/`dir`/`delattr`); here
/// they are the entries of `attrs`, and `dir(self)` is the key set (the Python class has no
/// other attribute starting with `e`, so `dir()` filtered by `valid_e` is the same set).
/// Narrowed: values are `Option<f64>` (Python: any value, in practice a number or `None`).
#[derive(Debug, Clone, PartialEq)]
pub struct PositionRecord {
    attrs: BTreeMap<String, Option<f64>>,
    /// `valid`: an attribute `_monitor` sets (comm.py:2701) and nothing reads; `None` is "never
    /// set" (OURS, difference 20).
    pub valid: Option<bool>,
}

impl PositionRecord {
    const _standard_attrs: [&'static str; 6] = ["x", "y", "z", "e", "f", "t"];

    pub fn valid_e(attr: &str) -> bool {
        if !attr.starts_with('e') {
            return false;
        }

        // Narrowed: `py_int` is i64, Python's `int()` has no upper bound (so "e" followed by a
        // number beyond i64 is `valid_e` in Python and not here).
        if py_int(&attr[1..]).is_none() {
            return false;
        }

        true
    }

    /// `__init__(self, *args, **kwargs)`: `kwargs` as (name, value) pairs; `*args` is unused.
    pub fn new(kwargs: &[(&str, Option<f64>)]) -> Self {
        let mut rec = PositionRecord {
            attrs: BTreeMap::new(),
            valid: None,
        };
        let attrs = rec._attrs(kwargs.iter().map(|(k, _)| *k));
        for attr in attrs {
            // kwargs.get(attr)
            let v = kwargs
                .iter()
                .rev()
                .find(|(k, _)| *k == attr)
                .and_then(|(_, v)| *v);
            rec.setattr(&attr, v);
        }
        rec
    }

    /// `getattr(self, name)`: `None` is `AttributeError`.
    pub fn getattr(&self, name: &str) -> Option<Option<f64>> {
        self.attrs.get(name).copied()
    }

    /// `setattr(self, name, value)`
    pub fn setattr(&mut self, name: &str, value: Option<f64>) {
        self.attrs.insert(name.to_string(), value);
    }

    pub fn copy_from(&mut self, other: &PositionRecord) {
        // make sure all standard attrs and attrs from other are set
        let attrs = self._attrs(other.attrs.keys().map(|k| k.as_str()));
        for attr in &attrs {
            let v = other.getattr(attr).expect("getattr(other, attr)");
            self.setattr(attr, v);
        }

        // delete attrs other doesn't have
        let delete: Vec<String> = self
            .attrs
            .keys()
            .filter(|key| Self::valid_e(key))
            .filter(|key| !attrs.contains(*key))
            .cloned()
            .collect();
        for attr in delete {
            self.attrs.remove(&attr);
        }
    }

    pub fn as_dict(&self) -> BTreeMap<String, Option<f64>> {
        let attrs = self._attrs(self.attrs.keys().map(|k| k.as_str()));
        attrs
            .into_iter()
            .map(|attr| {
                let v = self.getattr(&attr).expect("getattr(self, attr)");
                (attr, v)
            })
            .collect()
    }

    fn _attrs<'a>(&self, args: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
        let mut r: BTreeSet<String> = Self::_standard_attrs
            .iter()
            .map(|s| s.to_string())
            .collect();
        r.extend(
            args.into_iter()
                .filter(|key| Self::valid_e(key))
                .map(|k| k.to_string()),
        );
        r
    }

    pub fn reset(&mut self) {
        let attrs = self._attrs(self.attrs.keys().map(|k| k.as_str()));
        for attr in attrs {
            self.setattr(&attr, None);
        }
    }
}

/// `(actual, target)`, Python tuple of numbers/`None`. Narrowed to `f64`.
pub type TempTuple = (Option<f64>, Option<f64>);

/// Key of `TemperatureRecord.as_script_dict()`: tool numbers (int) and the strings
/// `"b"`, `"c"` and the custom identifiers.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptKey {
    Tool(i64),
    Name(String),
}

/// A value of `as_script_dict()`: `{"actual": ..., "target": ...}`
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptTemp {
    pub actual: Option<f64>,
    pub target: Option<f64>,
}

/// Python `dict` assignment on an insertion-ordered `Vec`: replace the value, keep the position.
fn dict_set<K: PartialEq, V>(d: &mut Vec<(K, V)>, key: K, value: V) {
    match d.iter_mut().find(|(k, _)| *k == key) {
        Some(e) => e.1 = value,
        None => d.push((key, value)),
    }
}

/// comm.py:360 TemperatureRecord
///
/// Dicts are insertion-ordered `Vec`s (Python dict order). Narrowed: tool keys are `i64` (the
/// callers pass tool numbers), identifiers are `String`.
#[derive(Debug, Clone, PartialEq)]
pub struct TemperatureRecord {
    _tools: Vec<(i64, TempTuple)>,
    _bed: TempTuple,
    _chamber: TempTuple,
    _custom: Vec<(String, TempTuple)>,
}

/// comm.py:361 TemperatureRecord.RESERVED_IDENTIFIER_REGEX (used with `fullmatch`, hence the anchors)
pub static RESERVED_IDENTIFIER_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile(r"\A(?:B|C|T\d*)\z"));

impl TemperatureRecord {
    pub fn new() -> Self {
        TemperatureRecord {
            _tools: Vec::new(),
            _bed: (None, None),
            _chamber: (None, None),
            _custom: Vec::new(),
        }
    }

    pub fn copy_from(&mut self, other: &TemperatureRecord) {
        self._tools = other.tools();
        self._bed = other.bed();
    }

    pub fn set_tool(&mut self, tool: i64, actual: Option<f64>, target: Option<f64>) {
        let current = self
            ._tools
            .iter()
            .find(|(k, _)| *k == tool)
            .map(|(_, v)| *v)
            .unwrap_or((None, None));
        let new = Self::_to_new_tuple(Some(current), actual, target);
        dict_set(&mut self._tools, tool, new);
    }

    pub fn set_bed(&mut self, actual: Option<f64>, target: Option<f64>) {
        let current = self._bed;
        self._bed = Self::_to_new_tuple(Some(current), actual, target);
    }

    pub fn set_chamber(&mut self, actual: Option<f64>, target: Option<f64>) {
        let current = self._chamber;
        self._chamber = Self::_to_new_tuple(Some(current), actual, target);
    }

    /// Raises `ValueError` (a [`PyValueError`] with the formatted message) for a reserved identifier.
    pub fn set_custom(
        &mut self,
        identifier: &str,
        actual: Option<f64>,
        target: Option<f64>,
    ) -> Result<(), PyValueError> {
        if RESERVED_IDENTIFIER_REGEX
            .is_match(identifier)
            .unwrap_or(false)
        {
            return Err(PyValueError(format!(
                "{identifier} is a reserved identifier"
            )));
        }
        let current = self
            ._custom
            .iter()
            .find(|(k, _)| k == identifier)
            .map(|(_, v)| *v)
            .unwrap_or((None, None));
        let new = Self::_to_new_tuple(Some(current), actual, target);
        dict_set(&mut self._custom, identifier.to_string(), new);
        Ok(())
    }

    /// property `tools`: `dict(self._tools)`, a copy
    pub fn tools(&self) -> Vec<(i64, TempTuple)> {
        self._tools.clone()
    }

    /// property `bed`
    pub fn bed(&self) -> TempTuple {
        self._bed
    }

    /// property `chamber`
    pub fn chamber(&self) -> TempTuple {
        self._chamber
    }

    /// property `custom`: `dict(self._custom)`, a copy
    pub fn custom(&self) -> Vec<(String, TempTuple)> {
        self._custom.clone()
    }

    pub fn as_script_dict(&self) -> Vec<(ScriptKey, ScriptTemp)> {
        let mut result: Vec<(ScriptKey, ScriptTemp)> = Vec::new();

        let tools = self.tools();
        for (tool, data) in tools {
            dict_set(
                &mut result,
                ScriptKey::Tool(tool),
                ScriptTemp {
                    actual: data.0,
                    target: data.1,
                },
            );
        }

        let bed = self.bed();
        dict_set(
            &mut result,
            ScriptKey::Name("b".into()),
            ScriptTemp {
                actual: bed.0,
                target: bed.1,
            },
        );

        let chamber = self.chamber();
        dict_set(
            &mut result,
            ScriptKey::Name("c".into()),
            ScriptTemp {
                actual: chamber.0,
                target: chamber.1,
            },
        );

        let custom = self.custom();
        for (identifier, data) in custom {
            dict_set(
                &mut result,
                ScriptKey::Name(identifier),
                ScriptTemp {
                    actual: data.0,
                    target: data.1,
                },
            );
        }

        result
    }

    /// `_to_new_tuple(cls, current, actual, target)`. `current` is `Option` for Python's
    /// `current is None`; the `isinstance(current, tuple)` / `len(current) != 2` checks cannot fail
    /// for a [`TempTuple`].
    fn _to_new_tuple(
        current: Option<TempTuple>,
        actual: Option<f64>,
        target: Option<f64>,
    ) -> TempTuple {
        let current = current.unwrap_or((None, None));

        if actual.is_none() && target.is_none() {
            return current;
        }

        let (old_actual, old_target) = current;

        if actual.is_none() {
            (old_actual, target)
        } else if target.is_none() {
            (actual, old_target)
        } else {
            (actual, target)
        }
    }
}

impl Default for TemperatureRecord {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------------------------
// comm.py:6046-6211 JobQueue, CommandQueue, SendQueue
// ---------------------------------------------------------------------------------------------

/// `with self.blocked():`: the queue is blocked while the guard lives (`finally: self.unblock()`).
pub struct BlockedGuard<'a>(&'a Event);

impl Drop for BlockedGuard<'_> {
    fn drop(&mut self) {
        self.0.set();
    }
}

/// comm.py:6046 JobQueue
pub struct JobQueue<T>(PrependableQueue<Deque<T>>);

impl<T> JobQueue<T> {
    pub fn new(maxsize: i64) -> Self {
        JobQueue(PrependableQueue::new(maxsize))
    }
}

impl<T> Deref for JobQueue<T> {
    type Target = PrependableQueue<Deque<T>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// comm.py:6050 CommandQueue
pub struct CommandQueue<T, K: Hash + Eq + Clone + Display> {
    queue: TypedQueue<T, K>,
    _unblocked: Event,
}

impl<T, K: Hash + Eq + Clone + Display> Deref for CommandQueue<T, K> {
    type Target = TypedQueue<T, K>;
    fn deref(&self) -> &Self::Target {
        &self.queue
    }
}

impl<T, K: Hash + Eq + Clone + Display> CommandQueue<T, K> {
    pub fn new(maxsize: i64) -> Self {
        let q = CommandQueue {
            queue: TypedQueue::new(maxsize),
            _unblocked: Event::new(),
        };
        q._unblocked.set();
        q
    }

    pub fn block(&self) {
        self._unblocked.clear();
    }

    pub fn unblock(&self) {
        self._unblocked.set();
    }

    /// `blocked()` (a `contextlib.contextmanager`): blocks now, unblocks when the guard is dropped.
    pub fn blocked(&self) -> BlockedGuard<'_> {
        self.block();
        BlockedGuard(&self._unblocked)
    }

    /// `get(self, *args, **kwargs)`: `block, timeout` are `Queue.get`'s.
    pub fn get(&self, block: bool, timeout: Option<f64>) -> Result<T, GetError> {
        self._unblocked.wait(None);
        TypedQueue::get(&self.queue, block, timeout)
    }

    /// `put(self, *args, **kwargs)`: `item, item_type, block, timeout` are `TypedQueue.put`'s.
    pub fn put(
        &self,
        item: T,
        item_type: Option<K>,
        block: bool,
        timeout: Option<f64>,
    ) -> Result<(), PutError<TypeAlreadyInQueue<K>>> {
        self._unblocked.wait(None);
        TypedQueue::put(&self.queue, item, item_type, block, timeout)
    }

    pub fn clear(&self) -> Vec<T> {
        let mut cleared = Vec::new();
        loop {
            match TypedQueue::get(&self.queue, false, None) {
                Ok(item) => {
                    cleared.push(item);
                    Queue::task_done(&self.queue).expect("task_done after get");
                }
                Err(GetError::Empty) => break,
                Err(e) => panic!("TypedQueue.get raised {e:?}"),
            }
        }
        cleared
    }
}

/// `SendQueue` items: `(item, item_type, target)`
pub type SendItem<T, K> = (T, Option<K>, Option<String>);

/// The `_put`/`_get`/`_prepend`/`_qsize` overrides of `SendQueue`, with the `_resend_queue`,
/// `_send_queue`, `_lookup` and `_resend_active` it keeps. (`target` narrowed to `Option<String>`.)
pub struct SendStorage<T, K> {
    _resend_queue: PrependableQueue<Deque<SendItem<T, K>>>,
    _send_queue: PrependableQueue<Deque<SendItem<T, K>>>,
    _lookup: HashSet<K>,
    _resend_active: Arc<AtomicBool>,
}

impl<T, K: Hash + Eq + Clone + Display> SendStorage<T, K> {
    fn resend_active(&self) -> bool {
        self._resend_active.load(Ordering::SeqCst)
    }

    fn _check_type(&mut self, item_type: &Option<K>) -> Result<(), TypeAlreadyInQueue<K>> {
        if let Some(item_type) = item_type {
            if self._lookup.contains(item_type) {
                return Err(TypeAlreadyInQueue::new(
                    item_type.clone(),
                    format!("Type {item_type} is already in queue"),
                ));
            } else {
                self._lookup.insert(item_type.clone());
            }
        }
        Ok(())
    }
}

impl<T, K: Hash + Eq + Clone + Display> QueueStorage for SendStorage<T, K> {
    type Item = SendItem<T, K>;
    type Error = TypeAlreadyInQueue<K>;

    fn _put(&mut self, item: Self::Item) -> Result<(), Self::Error> {
        let (_, item_type, target) = &item;
        self._check_type(item_type)?;

        if target.as_deref() == Some("resend") {
            self._resend_queue
                .put(item, true, None)
                .expect("put on an unbounded queue");
        } else {
            self._send_queue
                .put(item, true, None)
                .expect("put on an unbounded queue");
        }

        Ok(())
    }

    fn _get(&mut self) -> Result<Self::Item, Empty> {
        let item = if self.resend_active() {
            self._resend_queue.get(false, None).map_err(|_| Empty)?
        } else {
            match self._resend_queue.get(false, None) {
                Ok(i) => i,
                Err(_) => self._send_queue.get(false, None).map_err(|_| Empty)?,
            }
        };

        let (_, item_type, _) = &item;
        if let Some(item_type) = item_type {
            if self._lookup.contains(item_type) {
                self._lookup.remove(item_type);
            }
        }

        Ok(item)
    }

    fn _qsize(&self) -> usize {
        if self.resend_active() {
            self._resend_queue.qsize()
        } else {
            self._resend_queue.qsize() + self._send_queue.qsize()
        }
    }
}

impl<T, K: Hash + Eq + Clone + Display> PrependableStorage for SendStorage<T, K> {
    fn _prepend(&mut self, item: Self::Item) -> Result<(), Self::Error> {
        let (_, item_type, target) = &item;
        self._check_type(item_type)?;

        if target.as_deref() == Some("resend") {
            self._resend_queue
                .prepend(item, true, PREPEND_DEFAULT_TIMEOUT)
                .expect("prepend on an unbounded queue");
        } else {
            self._send_queue
                .prepend(item, true, PREPEND_DEFAULT_TIMEOUT)
                .expect("prepend on an unbounded queue");
        }
        Ok(())
    }
}

/// comm.py:6089 SendQueue
pub struct SendQueue<T, K: Hash + Eq + Clone + Display> {
    queue: PrependableQueue<SendStorage<T, K>>,
    _unblocked: Event,
    _resend_active: Arc<AtomicBool>,
}

impl<T, K: Hash + Eq + Clone + Display> Deref for SendQueue<T, K> {
    type Target = PrependableQueue<SendStorage<T, K>>;
    fn deref(&self) -> &Self::Target {
        &self.queue
    }
}

impl<T, K: Hash + Eq + Clone + Display> SendQueue<T, K> {
    pub fn new(maxsize: i64) -> Self {
        let resend_active = Arc::new(AtomicBool::new(false));
        let q = SendQueue {
            queue: PrependableQueue::with_storage(
                maxsize,
                SendStorage {
                    _resend_queue: PrependableQueue::new(0),
                    _send_queue: PrependableQueue::new(0),
                    _lookup: HashSet::new(),
                    _resend_active: resend_active.clone(),
                },
            ),
            _unblocked: Event::new(),
            _resend_active: resend_active,
        };
        q._unblocked.set();
        q
    }

    /// property `resend_active` (read without the mutex, as Python does: an `AtomicBool`)
    pub fn resend_active(&self) -> bool {
        self._resend_active.load(Ordering::SeqCst)
    }

    /// `resend_active` setter: `with self.mutex:`
    pub fn set_resend_active(&self, resend_active: bool) {
        let _g = self.queue.lock();
        self._resend_active.store(resend_active, Ordering::SeqCst);
    }

    pub fn block(&self) {
        self._unblocked.clear();
    }

    pub fn unblock(&self) {
        self._unblocked.set();
    }

    /// `blocked()` (a `contextlib.contextmanager`): blocks now, unblocks when the guard is dropped.
    pub fn blocked(&self) -> BlockedGuard<'_> {
        self.block();
        BlockedGuard(&self._unblocked)
    }

    /// `prepend(self, item, item_type=None, target=None, block=True, timeout=None)`
    pub fn prepend(
        &self,
        item: T,
        item_type: Option<K>,
        target: Option<String>,
        block: bool,
        timeout: Option<f64>,
    ) -> Result<(), PutError<TypeAlreadyInQueue<K>>> {
        self._unblocked.wait(None);
        PrependableQueue::prepend(&self.queue, (item, item_type, target), block, timeout)
    }

    /// `put(self, item, item_type=None, target=None, block=True, timeout=None)`
    pub fn put(
        &self,
        item: T,
        item_type: Option<K>,
        target: Option<String>,
        block: bool,
        timeout: Option<f64>,
    ) -> Result<(), PutError<TypeAlreadyInQueue<K>>> {
        self._unblocked.wait(None);
        Queue::put(&self.queue, (item, item_type, target), block, timeout)
    }

    /// `get(self, block=True, timeout=None)`
    pub fn get(&self, block: bool, timeout: Option<f64>) -> Result<T, GetError> {
        self._unblocked.wait(None);
        let (item, _, _) = Queue::get(&self.queue, block, timeout)?;
        Ok(item)
    }

    /// `clear()`: the cleared items are the raw `(item, item_type, target)` tuples
    /// (`PrependableQueue.get`, not `SendQueue.get`).
    pub fn clear(&self) -> Vec<SendItem<T, K>> {
        let mut cleared = Vec::new();
        loop {
            match Queue::get(&self.queue, false, None) {
                Ok(item) => {
                    cleared.push(item);
                    Queue::task_done(&self.queue).expect("task_done after get");
                }
                Err(GetError::Empty) => break,
                Err(e) => panic!("PrependableQueue.get raised {e:?}"),
            }
        }
        cleared
    }
}

// ---------------------------------------------------------------------------------------------
// comm.py:6894-6909 QueueMarker, SendQueueMarker
// ---------------------------------------------------------------------------------------------

/// comm.py:6894 QueueMarker
///
/// `callback` is any callable (or `None`).
pub struct QueueMarker {
    pub callback: Option<Box<dyn Fn() + Send + Sync>>,
}

impl QueueMarker {
    pub fn new(callback: Option<Box<dyn Fn() + Send + Sync>>) -> Self {
        QueueMarker { callback }
    }

    /// `run()`: `except Exception` is a caught panic of the callback here.
    pub fn run(&self) {
        if let Some(callback) = &self.callback {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback())).is_err() {
                // _logger.exception("Error while running callback of QueueMarker")
            }
        }
    }
}

/// comm.py:6908 SendQueueMarker
pub struct SendQueueMarker(pub QueueMarker);

impl SendQueueMarker {
    pub fn new(callback: Option<Box<dyn Fn() + Send + Sync>>) -> Self {
        SendQueueMarker(QueueMarker::new(callback))
    }
}

impl Deref for SendQueueMarker {
    type Target = QueueMarker;
    fn deref(&self) -> &QueueMarker {
        &self.0
    }
}

// ---------------------------------------------------------------------------------------------
// comm.py:444-1272 MachineCom (class constants, __init__, state, getters, close);
// comm.py:5689-5767 MachineComPrintCallback
//
// Threads: Python shares one `MachineCom` between the caller, `comm._monitor` and
// `comm.sending_thread`. It is an `Arc<MachineCom>` here; every attribute that is assigned after
// `__init__` (or whose value is changed in place) is an `Attr<T>` (py_attr.rs: Python's
// attribute semantics, atomic read, atomic write, nothing more). Attributes that are only assigned
// in `__init__` and hold no state that changes in place are plain fields. Python's own locks
// (`RLock`, `Event`, `CountedEvent`, the queues) are the ports in py_threading.rs / util.rs /
// py_queue.rs. No lock is added.
//
// Differences from Python (numbered; the code refers to them as "difference N"):
//  1. Logging is not ported: `self._logger.*` and `self._serialLogger.*` calls are comments. What
//     feeds state is ported (`_terminal_log`, `_callback.on_comm_log`, `_lastLines`).
//  2. Python names kept: fields are the Python attribute names (hence `non_snake_case`), methods
//     are snake_case with the Python name in the doc comment. The typing is ours: Python
//     attributes are dynamic; each field has the narrowest Rust type the comm.py code in this
//     section and the uses we grepped allow. Where Python stores `False` for "none" the field is an
//     `Option` and `None` stands for `False` (`_dwelling_until`, `_sdFileLongName`).
//     Settings values that Python takes unchecked keep Python's `None` (`Option<..>`).
//  3. A Python exception that nothing catches (`TypeError` for `None / 100`, `min(20, None)`, a
//     negative `deque` maxlen, `AttributeError` on a `None` that another thread set) is a panic.
//  4. `__init__` creates the two `threading.Thread` objects; Python's thread name
//     (`comm._monitor`, `comm.sending_thread`) is not kept (py_threading.rs `Thread` has none).
//  5. `start()` takes `self: &Arc<Self>` (the thread closures hold a reference, as the bound
//     methods do in Python).
//  6. `__del__` is `Drop` (calls `close()`; skipped while panicking).
//  7. `job_put_on_hold` is a `contextlib.contextmanager`: here a guard value whose `Drop` is the
//     `finally:` part (it runs when the guard goes out of scope).
//  8. `close(*args, **kwargs)`: `kwargs.get("isError")` is the `legacy_is_error` argument of
//     `close_with_legacy`; `*args` is ignored by Python and has no counterpart.
//  9. `close()` reads `unfinished_tasks` of the command/send queue (a plain attribute in Python)
//     through the queue mutex (py_queue.rs keeps it in the mutex state), briefly.
// 10. `_serial` (a `serial.Serial`) is the `SerialTransport` trait; `hasattr(.., "port")` and
//     friends are the trait methods returning `Option`.
// 11. `_currentFile` (`PrintingFileInformation` objects, comm.py:5768-6045) is the
//     `PrintingFileInformation` trait: the base class's methods with its behaviour as defaults, four
//     structs for the classes (they embed `BasePrintingFileInformation`; `StreamingGcode...` wraps
//     `PrintingGcode...`, `SpecialStreaming...` wraps `Streaming...`); a method only a subclass
//     has is an `AttributeError` (panic) on the others; `isinstance` checks are two trait methods;
//     `x.pos = v` is `set_pos` (a no-op but for the SD class, where Python would create an unused
//     attribute). `open(..., encoding="utf-8-sig", errors="replace", newline="")` is `TextHandle`
//     (a `BufReader`; Python reads in chunks, the lines are the same); `handle.seek(n)` is a byte
//     seek (reset BOM state at 0, as `_pyio.py:2551` / `utf_8_sig.py:70-81`), `tell()` after it is
//     `n`; the constructors' and `start`'s `OSError` is `Err(OSError)`; `getNext`'s `except Exception:
//     close(); raise` is a `catch_unwind` that closes and re-raises. Callbacks
//     (`offsets_callback=self.getOffsets`, ...) are closures holding an `Arc<MachineCom>`, as the
//     bound methods hold `self` (a reference cycle, as in Python).
// 12. `threading.Timer` (`_resend_ok_timer`) is the CPython 3.13 port in py_threading.rs
//     (`threading.py:1321`); Python's inherited daemon flag is passed by the caller.
// 13. The queue item types (`CommandQueueItem`, `SendQueueEntry`) are ours: Python puts tuples of
//     differing shape; the shapes are those of comm.py:1312, 1303, 4630, 1214.
// 14. `to_unicode(message)` in `_log` is the identity here (`message` is already a `&str`).
//     (`to_unicode(cmd, errors="replace")` in `sendCommand` is `util::to_unicode` on the `str`.)
// 15. (was: stubs for comm.py:4509-5487) Section I (comm.py:4509-5688) is ported: nothing after comm.py:4508
//     is a stub any more except the ones named in differences 23, 26 and 32 and in plugin.rs.
// 16. Section F (comm.py:1273-2259): a method that Python lets raise on purpose returns `Err`
//     (`startPrint`'s `ValueError`, the file classes' `OSError` through `selectFile` and
//     `startFileTransfer`); every other uncaught exception is a panic (difference 3), also the
//     `ValueError` of `get_dos_filename`. `startPrint`'s `try/except Exception` catches the
//     file's `OSError`; its `except` calls `util::get_exception_string` (not ported, panics) before
//     the `_trigger_error` stub. Methods whose closures (markers, `on_sent`, timers) hold `self`
//     take `self: &Arc<Self>`. `sendCommand` returns `Option<bool>` (`None` is Python's `None`),
//     its `cmd` is a `QueuedCommand` (str or marker); `_hello_command` / `_sdCancelCommand` must be
//     a str (`None` is the `TypeError` of `process_gcode_line(None)`, any other type is
//     `unimplemented!` for `str()`); `_pauseWaitStartTime = 0` is `Some(0.0)`; a list `in` test on a
//     settings value is `py_in_list`; `"{}".format(float)` is `py_float_repr`.
// 17. `_getGcodeScript` (comm.py:1322): the context is a minijinja map (`Settings::load_script_value`):
//     `last_temperature`'s tool keys are ints as in `as_script_dict()`, in insertion order; the other
//     entries are JSON values (`printer_profile`, plugin variables), `last_position` is its
//     `as_dict()`; a hook is called with `("gcode", scriptName)` (the comm instance is not passed)
//     and cannot raise (a panic is the caught exception); `dict_merge`, `get_exception_string` are in util.rs, `valid_file_type`
//     and `FileDestinations` in filemanager.rs, `get_dos_filename` in files.rs, `sanitize` in
//     text.rs (non-ASCII text needs the third-party `unidecode`/`emoji`: `unimplemented!`).
// 18. `_processTemperatures` (comm.py:2179): hooks get the parsed temperatures as JSON
//     `{"T0": [actual, target]}` and not the comm instance. The parsed temperatures are
//     `Temperatures`, a `PyDict` (insertion-ordered, `==` ignores order like a Python dict), so the
//     loop over "other injected temperatures" and the hook's JSON object are in Python's order.
// 19. `MachineComPrintCallback::on_comm_file_selected` takes `Option<&str>` for the file (comm.py:1676
//     passes `None`), `on_comm_file_transfer_done/failed` take `Option<f64>` (`getPrintTime()`);
//     `_sdFiles` holds `SDFileData` (comm.py:193), `longname=False` is `None`.
// 20. `_monitor` (comm.py:2260-3307) is one method, in Python's order, its locals as Python's. OURS:
//     the `try:` body of the loop is a closure passed to `py_try`, `continue` is `return Flow::Next`,
//     `break` is `return Flow::Break`; `py_try` catches a panic as Python's `except Exception`
//     (difference 3) but lets the panic of an `unimplemented!` stub go on up. Every method it calls
//     after comm.py:3307 is in section H (differences 21-27); `system_command_manager().
//     perform_system_shutdown()` is a stub (needs `util/commandline.py`).
//     `convert_line(line)` is two lines with
//     `line` never `None` (a `None` from `_handle_errors` would raise `AttributeError` /
//     `TypeError` in Python on the first use, a panic here at once). The `controls` and
//     `pauseTriggers` settings are converted from JSON to `FeedbackControl` / string maps. Hooks
//     get `(line, action_command, name, params)` (action) and `(caps/info JSON)` without the comm
//     instance and without keyword names (plugin.rs). `"%s" % self._serial` is
//     `SerialTransport::str` (pyserial's `Serial<id=0x...>` repr holds a memory address, which cannot be
//     reproduced: ours prints the address of the Rust object), `self._serial.timeout = x` is
//     `SerialTransport::set_timeout`. `py_try` returns the text of the panic (`Err(String)`; Rust's text
//     differs from Python's `str(exception)`), which the shutdown error log (`except Exception as ex:
//     self._log(f"Error executing system shutdown: {ex}")`) carries. `controls` / `pauseTriggers`: a falsy
//     setting is "no controls" (`convert_*` return early), any other value that is no list is a panic.
//     `parse_firmware_line` returns a `PyDict` in the key order of the firmware line, and so do
//     `_firmware_info`, the FIRMWARE_DATA payload, the hook argument and `on_comm_firmware_info`.
//     `PositionRecord.valid` and `MachineCom._firmware_name` are fields (set, never read);
//     `_known_invalid_tools` holds the tool number as text; `_firmware_info` is a `PyDict` filled
//     from `parse_firmware_line`'s dict (the key order of the firmware line); `filter_non_utf8` is `false` for a `str`;
//     `last_position.t` is the tool number as `f64`; `int()` of a float setting is `as i64`;
//     `self.startPrint(...)`'s `ValueError` is a panic (an exception the loop catches).
// 21. Section H (comm.py:3308-4508). A method whose closures, timers or markers hold `self` takes
//     `self: &Arc<Self>`: `_handle_ok`, `_continue_sending`, `_send_from_job`, `_get_next_from_job`,
//     `_handle_errors`, `_handle_resend_request`, `_resend_simulate_ok`, `_on_connected`, and with them
//     `fake_ok`, `set_job_on_hold`, `job_put_on_hold` (the guard holds the `&Arc`). `_continue_sending`
//     returns `Option<bool>` (`None` is Python's `None`). The `try: ... finally:` of
//     `_send_from_command_queue` (`task_done`) and `_handle_resend_request` (the `ok` trigger) is
//     `catch_unwind` + `resume_unwind`. `with ...blocked():` / `with self._line_mutex:` are guards.
//     The `threading.Timer` of `_handle_resend_request` and the `RepeatedTimer`s of `_onConnected` are
//     daemon threads (the monitor thread is a daemon: the Timer inherits it).
// 22. Exceptions that Python catches with `except Exception`: a panic through `py_try` (difference 20);
//     `get_exception_string()` (util/__init__.py:447) reads `sys.exc_info()` and the first traceback frame,
//     which Rust has not: `util::get_exception_string_of` takes the class name, the message and the place
//     of the `try` (`comm.rs`, the Python method's name, `line!()`). A panic's class is the `XxxError:`
//     before its text (`TypeError: ...`) else `Exception`. An exception from the serial object is a
//     `SerialError` (class name, message, `isinstance(.., serial.SerialException)`).
// 23. The serial object (`_open_serial`, `_readline`, `_perform_detection_step`): `SerialTransport` has
//     `timeout()`, `set_baudrate()`, `readline()` and `set_low_latency_mode()` besides its former methods.
//     `serial.rs` is the transport: `serialport` for pyserial (M5; `serial.rs` lists the differences),
//     `serial.Timeout`, `Serial.read` with its abort pipe, and `BufferedReadlineWrapper` (comm.py:6910).
//     The `default` factory of `_open_serial` is `_open_serial_default` (not on Windows: `unimplemented!`);
//     `util/platform` `get_os` / `set_close_exec` are `platform.rs`. The serial factory hooks (empty,
//     plugin.rs) return a JSON value: one that is not `None` is `unimplemented!` (it would be a serial
//     object); hooks do not get the comm instance (difference 20). `serial.Serial(**args)`'s `ValueError`
//     (baud rate, timeout) is a `SerialError` that is no `SerialException`, so it fails the factory.
// 24. `_handle_errors` / `_readline` / `_trigger_error` / `_payload_for_error`: the payload dict is
//     `ErrorPayload` (`to_json` keeps Python's key order); `str + None` (`self._readline()` giving `None`
//     in `_handle_errors`) is a panic; the received line is decoded as UTF-8, else latin1;
//     `sanitize_ascii` is `util::sanitize_ascii` (a `str` is not changed by `to_unicode`); a received-
//     message hook that returns something other than `None` or a str is `unimplemented!`.
// 25. `_process_registered_message` (comm.py:3562): `template.format(...)` is `py_format.rs` (OURS: a
//     `str.format` for str/None arguments; an attribute of a `str` as a field is `unimplemented!`, it
//     prints a memory address); a `bytes` template has no `.format` (output `None`); the `templates` of a
//     `FeedbackControlEntry` are a `PyDict` (Python's insertion order, for the event's `outputs`).
// 26. `serialList` / `baudrateList` (comm.py:196, 258): `serial_list` / `baudrate_list`; the Windows
//     registry branch is `unimplemented!`; `glob.glob` and `fnmatch.fnmatch` are `py_glob.rs` (CPython
//     3.13, POSIX); the plugin hook `octoprint.comm.transport.serial.additional_port_names` gets the
//     candidates as a JSON array; `serial_list_in(dir)` is `serialList` with the scanned directory as an
//     argument (for tests: `serialList` passes `/dev`). `sorted(additionalBaudrates, reverse=True)` is
//     done for settings that are all numbers or all strings (a mix is the `TypeError` panic).
// 27. Queue items: the entries of the command queue and the job queue are always 4-tuples
//     (`isinstance(entry, tuple)` / `len(entry) == 4` hold, comm.py:3859-3866). A marker a method
//     creates (`_get_next_from_job`) is a `QueueMarker` (`SendQueueMarker.0`, difference 13).
//     `int(self._timeout_intervals.get(...))` is `as i64` for a finite float (`int()` of NaN / inf is
//     the exception that Python catches); `max(a, b)` of two floats is `py_max`; `bytes.decode(
//     self._serial_encoding)` (`_resendNextCommand`) is `decode_serial_bytes` (ascii, utf-8, latin-1).
// 28. Section I, `_sendCommand` / `_enqueue_for_sending` (comm.py:4509-4651). `_sendCommand` takes `self: &Arc<Self>`
//     (the handlers call `setPause`, `cancelPrint`, `_continue_sending`; difference 21), and so do all callers
//     (`sendCommand`, `_send_from_*`, `close`, ...). `isinstance(cmd, QueueMarker)`: every marker the port makes is a
//     `SendQueueMarker` (comm.py:1530 ff.), so the `else: return False` branch (a plain `QueueMarker`) is not
//     reachable. `tags` is a `Tags` (`Rc<RefCell<..>>`) inside the phases, as `_normalize_command_handler_result`
//     (comm.rs, section G) needs; `_enqueue_for_sending(tags=None)` stays `None` in the `SendQueueEntry` (an
//     `Option<BTreeSet>`) and reaches `_process_command_phase` / `_process_atcommand_phase` as `None`, as
//     comm.py:4631, 4717, 4780. The `tags = tags.unwrap_or_default()` in the public methods (`sendCommand`,
//     `sendGcodeScript`, `startPrint`, `startFileTransfer`, `_finishFileTransfer`, `selectFile`, `cancelPrint`,
//     `setPause`, `deleteSdFile`, `refreshSdFiles`, `initSdCard`, `releaseSdCard`, `sayHello`, `resetLineNumbers`,
//     `_set_autoreport_pos_interval`) are Python's own `if tags is None: tags = set()` (comm.py:1299, 1423, 1465,
//     1564, 1605, 1645, 1734, 1895, 2031, 2067, 2093, 2110, 2127, 2145, 3690), as is that of
//     `_command_phase_queuing` (comm.py:5621-5623).
// 29. `_send_loop` (comm.py:4653): one pass of the body is `_send_loop_entry`, run by `py_try` for the outer
//     `try: ... except Exception:`; `continue` / `break` / falling through to `_clear_to_send.wait()` are `LoopStep`;
//     the inner `try: ... finally: task_done()` is `catch_unwind` + `resume_unwind`. A bare `SendQueueMarker` in the
//     queue while the loop is still active is the `TypeError` (unpacking) that Python's `except Exception` catches.
//     `time.sleep(self._dwelling_until - now)` reads the attribute again, as Python does.
// 30. The gcode handlers (`_gcode_<G>_<phase>`, comm.py:5131-5578) are methods of the same names; `hasattr(self,
//     "_gcode_" + gcode + "_" + phase)` is the `match` of `_gcode_handler`, `_command_phase_<phase>` is a call in
//     `_process_command_phase`, `getattr(self, "_atcommand_" + ...)` the `match` of `_process_atcommand_phase`.
//     The aliases (`_gcode_G1_sent = _gcode_G0_sent`, `_gcode_M190_queuing = _gcode_M140_queuing`, ...) are match
//     arms; `_gcode_M104_sent(.., wait, support_r)` takes the two keyword arguments. A handler returns
//     `Option<HandlerResults>` (`None`, `(None,)`, or the list `_gcode_T_queuing` builds from the two scripts).
//     `last_fanspeed` is `i64`, as Python's `int` (comm.py:5556). `_gcode_M104_sent` checks the tool key exists where
//     Python checks `tools.get(toolNum) is not None`: the same, since a stored tool value is a tuple, never None. `int()` / `float()` of a regex group that cannot be converted is the
//     `ValueError` (a panic) where Python does not catch it; `_gcode_M104_sent` & co. catch it (`except ValueError`).
//     `_log_command_phase` only logs (`_phaseLogger`): its body is a comment (difference 1).
// 31. Plugin hooks of the phases (`_gcode_hooks[phase]`, `_atcommand_hooks[phase]`; none registered, plugin.rs)
//     get positional JSON without the comm instance (`phase, command, command_type, gcode, subcode, tags`; tags as
//     an array) and return JSON: `null` is `None`, a string a command, an array a list whose arrays are the tuples
//     (command, command_type, tags). JSON cannot tell a top-level tuple from a list: it is a list.
// 32. Serial writing (comm.py:5036): `SerialTransport::write` is pyserial 3.5 `Serial.write` (serialposix.py:612 ff.,
//     with `Timeout` :618 and `write_timeout`) in serial.rs, beside `read` (the Section H decision extended, same
//     reference, OURS: docs/archive/plans/agent-engine.md); it returns `Option<i64>` (`None`: an object that returns no int).
//     `self._serial.write` reads `_serial` each time: a `None` set by another thread is the `AttributeError`.
//     `str.encode(self._serial_encoding, errors="replace")` is `encode_serial_text` (ascii, utf-8, latin-1; any
//     other codec `unimplemented!`). `Drop` (`__del__`, difference 6) has no `Arc`, so `close()` there is
//     `close_impl(None, ..)`: the `beforePrinterDisconnected` script is not sent while the object is dropped (an
//     empty script, the default, sends nothing; a script with lines is `unimplemented!`).
// 33. Section J: `printerProfileManager` (comm.py:500) is the port `profile::PrinterProfileManager` (a struct, no longer a trait
//     object): `get_current_or_default()` returns the profile dict as JSON. What it cannot do is listed in profile.rs
//     (no profile folder: `exists` is false, `select` always takes the default). The tests build one with
//     `PrinterProfileManager::with_current(json)`.
// 34. Section J: a plugin hook handler (`plugin::HookFn`) is an `Arc` (was a `Box`), so `get_hooks` can hand out the handlers
//     `PluginManager::register_hook` registered (the prompt plugin, `action_command_prompt.rs`); a hook still gets positional
//     JSON and no comm instance (differences 20, 31). The plugin's `gcode.queuing` handler reaches the comm through the
//     printer it belongs to. `py_try`, `py_truthy` and `py_split_none` are crate-visible for `standard.rs`, `events.rs`,
//     `plugin.rs`, `profile.rs`, `api_printer.rs` and `action_command_prompt.rs`.
// ---------------------------------------------------------------------------------------------

#[allow(unused_imports)]
use super::events::{Events, event_manager};
use super::platform;
use super::plugin::{Hooks, PluginManager, plugin_manager};
use super::profile::PrinterProfileManager;
use super::py_attr::Attr;
use super::py_collections::{PyDeque, PyDict};
use super::py_threading::{RLock, RuntimeError, Thread, Timer};
use super::serial;
use super::settings::{GetKwargs, Settings};
use super::util::{
    CountedEvent, EXCEPTION_STRING_FMT, ExceptionInfo, Interval, RepeatedTimer, ResettableTimer,
    get_exception_string_of, sanitize_ascii,
};
use super::{py_format, py_glob};
use serde_json::Value;

/// OURS (difference 20): the `try: ... except Exception:` of Python. A Python exception is a panic
/// here (difference 3), so this catches it (`Err(text)`: the panic's text, `""` when the payload is
/// no text; Rust's text differs from Python's `str(exception)`); a panic of an `unimplemented!`
/// stub is never caught, it goes on up (a missing port must not look like an exception that was
/// handled).
pub(crate) fn py_try<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => Ok(r),
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied());
            if message.is_some_and(|m| m.starts_with("not implemented")) {
                std::panic::resume_unwind(payload);
            }
            Err(message.unwrap_or("").to_string())
        }
    }
}

/// What `_monitor`'s loop body does next (OURS, difference 20): the end of the body (also
/// `continue`), or `break`.
enum Flow {
    Next,
    Break,
}

/// `pattern.match(text)`: `re_captures` finds the leftmost match, which starts at 0 exactly when
/// `match` finds one.
fn re_match<'t>(re: &Regex, text: &'t str) -> Option<fancy_regex::Captures<'t>> {
    re_captures(re, text).filter(|c| c.get(0).is_some_and(|m| m.start() == 0))
}

/// `pattern.search(text) is not None`
fn re_search(re: &Regex, text: &str) -> bool {
    re_captures(re, text).is_some()
}

/// A settings value that Python reads as `str` (or `None`) in the entries of `controls` and
/// `printerParameters.pauseTriggers`; any other type is `unimplemented!` (OURS).
fn settings_str(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => unimplemented!(
            "a non-string value {other:?} in controls / printerParameters.pauseTriggers"
        ),
    }
}

/// OURS (difference 20): the `controls` setting (a list of dicts) as `FeedbackControl`s. A falsy
/// value (`None`, `[]`, `0`, `""`, ...) gives no controls, as `convert_feedback_controls` returns
/// early for it; any other value that is no list is a panic (the owner's decision, 2026-11-15).
fn feedback_controls_from_value(v: &Option<Value>) -> Vec<FeedbackControl> {
    fn control(v: &Value) -> FeedbackControl {
        let o = v.as_object().expect("a control is a dict");
        let text = |k: &str| o.get(k).and_then(settings_str);
        FeedbackControl {
            key: text("key"),
            regex: text("regex").map(PyText::Str),
            template: text("template").map(PyText::Str),
            template_key: text("template_key"),
            children: o
                .get("children")
                .and_then(|c| c.as_array())
                .map(|c| c.iter().map(control).collect()),
        }
    }
    match v {
        Some(Value::Array(a)) => a.iter().map(control).collect(),
        // `convert_feedback_controls`: `if not configured_controls: return {}, None` (comm.py:6318)
        v if !v.as_ref().is_some_and(py_truthy) => Vec::new(),
        // anything else is no list of dicts; Python would iterate it and fail on the first `in` / `[]`
        Some(other) => panic!("TypeError: the controls setting is not a list: {other:?}"),
        None => unreachable!("None is falsy"),
    }
}

/// OURS (difference 20): the `printerParameters.pauseTriggers` setting as `convert_pause_triggers`
/// takes it; falsy gives none, any other value that is no list is a panic (see above).
fn pause_triggers_from_value(v: &Option<Value>) -> Vec<BTreeMap<String, String>> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .map(|t| {
                t.as_object()
                    .expect("a trigger is a dict")
                    .iter()
                    .filter_map(|(k, v)| settings_str(v).map(|s| (k.clone(), s)))
                    .collect()
            })
            .collect(),
        // `convert_pause_triggers`: `if not configured_triggers: return {}` (comm.py:6290)
        v if !v.as_ref().is_some_and(py_truthy) => Vec::new(),
        Some(other) => panic!(
            "TypeError: the printerParameters.pauseTriggers setting is not a list: {other:?}"
        ),
        None => unreachable!("None is falsy"),
    }
}

/// `octoprint.systemcommands.system_command_manager().perform_system_shutdown()`
/// (`systemcommands/__init__.py:89`, which runs the configured command through
/// `util/commandline.py` `CommandlineCaller`, not ported): a stub, it panics.
fn system_command_manager_perform_system_shutdown() -> bool {
    unimplemented!(
        "systemcommands/__init__.py:89 perform_system_shutdown (util/commandline.py CommandlineCaller is not ported)"
    )
}

/// `time.monotonic()`: seconds from an arbitrary start (the first call), as in CPython.
pub fn time_monotonic() -> f64 {
    static START: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);
    START.elapsed().as_secs_f64()
}

/// `logging.ERROR`, `logging.INFO` (CPython `Lib/logging/__init__.py`): the `level` arguments.
pub const LOGGING_INFO: i64 = 20;
pub const LOGGING_ERROR: i64 = 40;
pub const LOGGING_WARNING: i64 = 30;

/// OURS (difference 13): `on_sent` callbacks.
pub type OnSent = Arc<dyn Fn() + Send + Sync>;

/// OURS (difference 13): a command in a queue is a gcode line or a `QueueMarker`
/// (`isinstance(cmd, QueueMarker)`, comm.py:1291).
pub enum QueuedCommand {
    Command(String),
    Marker(QueueMarker),
}

/// OURS (difference 13): `(cmd, cmd_type, on_sent, tags)` (comm.py:1303, 1312).
pub type CommandQueueItem = (
    QueuedCommand,
    Option<String>,
    Option<OnSent>,
    BTreeSet<String>,
);

/// OURS (difference 13): what `_send_queue` holds: a `SendQueueMarker` (comm.py:1214) or
/// `(command, linenumber, command_type, on_sent, False, tags)` (comm.py:4630).
pub enum SendQueueEntry {
    Marker(SendQueueMarker),
    Line {
        command: QueuedCommand,
        linenumber: Option<i64>,
        command_type: Option<String>,
        on_sent: Option<OnSent>,
        flag: bool,
        /// `tags=None` stays `None` (comm.py:4631, 4717, 4780); OURS: `Option` for Python's `None`.
        tags: Option<BTreeSet<String>>,
    },
}

/// OURS (difference 10): an exception raised by a call on the serial object (`serial.SerialException`
/// and its subclasses, `ValueError`, `OSError`, ...): the class name and the message;
/// `serial_exception` is `isinstance(ex, serial.SerialException)`.
#[derive(Debug, Clone, PartialEq)]
pub struct SerialError {
    pub type_name: String,
    pub message: String,
    pub serial_exception: bool,
}

impl SerialError {
    pub fn new(type_name: &str, message: impl Into<String>, serial_exception: bool) -> Self {
        SerialError {
            type_name: type_name.to_string(),
            message: message.into(),
            serial_exception,
        }
    }

    /// `serial.SerialException(message)`
    pub fn serial_exception(message: impl Into<String>) -> Self {
        Self::new("SerialException", message, true)
    }

    /// an exception that is no `serial.SerialException`
    pub fn other(type_name: &str, message: impl Into<String>) -> Self {
        Self::new(type_name, message, false)
    }
}

/// OURS (difference 10): the `serial.Serial`-like object in `_serial` (after the
/// `BufferedReadlineWrapper`: `serial.rs`). `None` from `port`/`baudrate` is "no such attribute"
/// (`hasattr` false); `Some(v)` is the attribute's value, which may itself be `None`.
/// `cancel_read`/`cancel_write`: `None` is "no such attribute or not callable", `Some(Err(msg))` is
/// an exception raised by the call. `close` may raise.
pub trait SerialTransport: Send + Sync {
    fn port(&self) -> Option<Option<String>> {
        None
    }
    fn baudrate(&self) -> Option<Option<i64>> {
        None
    }
    fn cancel_read(&self) -> Option<Result<(), String>> {
        None
    }
    fn cancel_write(&self) -> Option<Result<(), String>> {
        None
    }
    fn close(&self) -> Result<(), String>;
    /// OURS (difference 20): `"%s" % self._serial` (comm.py:2295), pyserial's `__repr__`; the
    /// transport that implements this trait says what the object prints as. Python's
    /// `Serial<id=0x...>` holds a memory address, which cannot be reproduced.
    fn str(&self) -> String;
    /// OURS (difference 20): `self._serial.timeout = value` (comm.py:2326, 3717, 3452), an attribute
    /// assignment on the `serial.Serial` object (a `ValueError` for a negative value is a panic).
    fn set_timeout(&self, timeout: f64);
    /// `self._serial.timeout` (comm.py:3451): `None` is Python's `None`.
    fn timeout(&self) -> Option<f64>;
    /// `self._serial.baudrate = baudrate` (comm.py:3499; a `ValueError` is a panic).
    fn set_baudrate(&self, baudrate: i64);
    /// `self._serial.readline()` (comm.py:4176): the bytes of one line (`b""` on a timeout), or the
    /// exception the call raised.
    fn readline(&self) -> Result<Vec<u8>, SerialError>;
    /// `self._serial.write(data)` (comm.py:5053): the number of bytes written, or `None` (an
    /// object that returns no int, `result is None or not isinstance(result, int)`), or the
    /// exception the call raised (`SerialTimeoutException` has the type name of that class).
    fn write(&self, data: &[u8]) -> Result<Option<i64>, SerialError>;
    /// `serial_obj.set_low_latency_mode(on)` (comm.py:3918): `None` is "no such attribute".
    fn set_low_latency_mode(&self, _on: bool) -> Option<Result<(), SerialError>> {
        None
    }
}

/// comm.py:5689 MachineComPrintCallback: every method is a no-op by default (`pass`). Argument
/// types are ours (Python is dynamic); a method that takes `user=None` takes `Option<&str>`.
pub trait MachineComPrintCallback: Send + Sync {
    /// comm.py:5690
    fn on_comm_log(&self, _message: &str) {}
    /// comm.py:5693 (`temp` = tools, `bedTemp`, `chamberTemp`, `customTemp`)
    fn on_comm_temperature_update(
        &self,
        _temp: &[(i64, TempTuple)],
        _bed_temp: TempTuple,
        _chamber_temp: TempTuple,
        _custom_temp: &[(String, TempTuple)],
    ) {
    }
    /// comm.py:5696
    fn on_comm_position_update(
        &self,
        _position: &BTreeMap<String, Option<f64>>,
        _reason: Option<&str>,
    ) {
    }
    /// comm.py:5699
    fn on_comm_state_change(&self, _state: i64) {}
    /// comm.py:5702
    fn on_comm_message(&self, _message: &str) {}
    /// comm.py:5705 (`logs` is a list of str)
    fn on_comm_error(
        &self,
        _error: &str,
        _reason: &str,
        _consequence: Option<&str>,
        _faq: Option<&str>,
        _logs: Option<&[String]>,
    ) {
    }
    /// comm.py:5708
    fn on_comm_progress(&self) {}
    /// comm.py:5711
    fn on_comm_print_job_started(&self, _suppress_script: bool, _user: Option<&str>) {}
    /// comm.py:5714
    fn on_comm_print_job_failed(&self, _reason: Option<&str>) {}
    /// comm.py:5717
    fn on_comm_print_job_done(&self, _suppress_script: bool) {}
    /// comm.py:5720
    fn on_comm_print_job_cancelling(&self, _firmware_error: Option<&str>, _user: Option<&str>) {}
    /// comm.py:5723
    fn on_comm_print_job_cancelled(&self, _suppress_script: bool, _user: Option<&str>) {}
    /// comm.py:5726
    fn on_comm_print_job_paused(&self, _suppress_script: bool, _user: Option<&str>) {}
    /// comm.py:5729
    fn on_comm_print_job_resumed(&self, _suppress_script: bool, _user: Option<&str>) {}
    /// comm.py:5732
    fn on_comm_z_change(&self, _new_z: Option<f64>) {}
    /// comm.py:5735
    fn on_comm_file_selected(
        &self,
        _filename: Option<&str>,
        _filesize: Option<i64>,
        _sd: bool,
        _user: Option<&str>,
        _data: Option<&Value>,
    ) {
    }
    /// comm.py:5738
    fn on_comm_sd_state_change(&self, _sd_ready: bool) {}
    /// comm.py:5741 (`files` is a list of file entries)
    fn on_comm_sd_files(&self, _files: &[Value]) {}
    /// comm.py:5744
    fn on_comm_file_transfer_started(
        &self,
        _local_filename: &str,
        _remote_filename: &str,
        _filesize: Option<i64>,
        _user: Option<&str>,
    ) {
    }
    /// comm.py:5749
    fn on_comm_file_transfer_done(
        &self,
        _local_filename: &str,
        _remote_filename: &str,
        _elapsed: Option<f64>,
    ) {
    }
    /// comm.py:5752
    fn on_comm_file_transfer_failed(
        &self,
        _local_filename: &str,
        _remote_filename: &str,
        _elapsed: Option<f64>,
    ) {
    }
    /// comm.py:5755
    fn on_comm_force_disconnect(&self) {}
    /// comm.py:5758
    fn on_comm_record_fileposition(&self, _origin: &str, _name: &str, _pos: Option<i64>) {}
    /// comm.py:5761
    fn on_comm_firmware_info(&self, _firmware_name: &str, _firmware_data: &PyDict<String>) {}
}

/// OURS: `MachineComPrintCallback()` instantiated as such (comm.py:515, `callbackObject is None`).
pub struct DefaultMachineComPrintCallback;

impl MachineComPrintCallback for DefaultMachineComPrintCallback {}

/// `base | {extra...}` (a Python set union with a literal)
fn with_tags(base: &BTreeSet<String>, extra: &[&str]) -> BTreeSet<String> {
    let mut r = base.clone();
    r.extend(extra.iter().map(|t| t.to_string()));
    r
}

/// `x in value` for a settings value that is a list (`gcode in self._emergency_commands`); a
/// string is a substring test, anything else a `TypeError` (a panic, difference 3)
fn py_in_list(haystack: &Option<Value>, needle: Option<&str>) -> bool {
    match haystack {
        Some(Value::Array(a)) => needle.is_some_and(|n| a.iter().any(|x| x.as_str() == Some(n))),
        Some(Value::String(s)) => match needle {
            Some(n) => s.contains(n),
            None => {
                panic!("TypeError: 'in <string>' requires string as left operand, not NoneType")
            }
        },
        other => panic!("TypeError: argument of type {other:?} is not iterable"),
    }
}

/// A settings value used as a command (`self._hello_command`, `self._sdCancelCommand`): a `str`;
/// `None` is the `TypeError` of `process_gcode_line(None)`; `to_unicode`'s `str(value)` of other
/// types is not ported.
fn value_to_cmd(v: &Option<Value>) -> QueuedCommand {
    match v {
        Some(Value::String(s)) => QueuedCommand::Command(s.clone()),
        None | Some(Value::Null) => {
            panic!("TypeError: argument of type 'NoneType' is not iterable")
        }
        Some(_) => unimplemented!(
            "util/__init__.py:94 to_unicode: str() of a settings value that is not a str"
        ),
    }
}

/// Python truthiness of a JSON value
pub(crate) fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// OURS: a `dict` of `str` as the JSON object a hook or an event payload gets, in the dict's order
/// (serde_json keeps the insertion order).
fn py_dict_to_json(d: &PyDict<String>) -> Value {
    Value::Object(
        d.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect(),
    )
}

/// What `_payload_for_error` (comm.py:4152) returns: the dict, as fields (`faq`, `logs` are
/// `payload.get("faq")`, `payload.get("logs")`).
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorPayload {
    pub error: String,
    pub reason: String,
    pub consequence: Option<String>,
    pub logs: Option<Vec<String>>,
    pub faq: Option<String>,
}

impl ErrorPayload {
    /// the dict as the event payload, keys in Python's insertion order
    pub fn to_json(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("error".to_string(), Value::String(self.error.clone()));
        m.insert("reason".to_string(), Value::String(self.reason.clone()));
        if let Some(c) = &self.consequence {
            m.insert("consequence".to_string(), Value::String(c.clone()));
        }
        if let Some(l) = &self.logs {
            m.insert(
                "logs".to_string(),
                Value::Array(l.iter().map(|x| Value::String(x.clone())).collect()),
            );
        }
        if let Some(f) = &self.faq {
            m.insert("faq".to_string(), Value::String(f.clone()));
        }
        Value::Object(m)
    }
}

/// `max(a, b)` of Python: the first of two equal values; with a NaN the first
fn py_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// OURS (difference 20): `get_exception_string()` for an exception that is a Rust panic caught by
/// `py_try`. The panic text is `"<Class>: <message>"` where the port raised it as a Python
/// exception (`TypeError: ...`); any other text has no class (`Exception`). `file`, `function`,
/// `line` are where the `try:` is (`ExceptionInfo`).
fn exception_string_of_panic(text: &str, fmt: &str, function: &str, line: u32) -> String {
    let (type_name, message) = match text.split_once(": ") {
        Some((t, m))
            if (t.ends_with("Error") || t.ends_with("Exception"))
                && t.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            (t, m)
        }
        _ => ("Exception", text),
    };
    get_exception_string_of(
        &ExceptionInfo {
            type_name,
            message,
            file: "comm.rs",
            function,
            line,
        },
        fmt,
    )
}

/// What one pass of `_send_loop`'s body does next (OURS, difference 29): `continue` (the next entry
/// without waiting for a clear), `break`, or falling through to `self._clear_to_send.wait()`.
enum LoopStep {
    Continue,
    Break,
    Wait,
}

/// comm.py:288 `gcodeToEvent[gcode]`
fn gcode_to_event(gcode: &str) -> Option<&'static str> {
    Some(match gcode {
        // pause for user input
        "M226" | "M0" | "M1" => Events::WAITING,
        // dwell command
        "G4" => Events::DWELL,
        // part cooler
        "M245" => Events::COOLING,
        // part conveyor
        "M240" => Events::CONVEYOR,
        // part ejector
        "M40" => Events::EJECT,
        // user alert
        "M300" => Events::ALERT,
        // home print head
        "G28" => Events::HOME,
        // emergency stop
        "M112" => Events::E_STOP,
        // motors on/off
        "M80" => Events::POWER_ON,
        "M81" => Events::POWER_OFF,
        // filament change
        "M600" | "M701" | "M702" => Events::FILAMENT_CHANGE,
        _ => return None,
    })
}

/// `(None,)`: what a handler returns to suppress the command
fn none_tuple() -> HandlerResults {
    HandlerResults::Entry(HandlerResultEntry::Tuple(HandlerTuple::One(None)))
}

/// OURS (difference 31): a `str` or `None` as the JSON a hook gets
fn json_opt_str(v: &Option<String>) -> Value {
    v.as_ref().map_or(Value::Null, |s| Value::String(s.clone()))
}

/// OURS (difference 31): a set of tags as the (sorted) JSON array a hook gets, `None` as `null`
fn json_tags(tags: &Option<Tags>) -> Value {
    tags.as_ref().map_or(Value::Null, |t| {
        Value::Array(
            t.borrow()
                .iter()
                .map(|s| Value::String(s.clone()))
                .collect(),
        )
    })
}

/// OURS (difference 31): what a gcode hook returned (JSON) as the handler results
/// `_normalize_command_handler_result` takes. JSON has no tuples: an array is a list, an array
/// inside that list is a tuple (command, command_type, tags); a top-level tuple cannot be told.
fn hook_value_to_results(v: &Value) -> Option<HandlerResults> {
    fn text(v: &Value) -> Option<String> {
        match v {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            other => {
                unimplemented!("a plugin hook result with {other:?} where a command is expected")
            }
        }
    }
    fn entry(v: &Value) -> HandlerResultEntry {
        match v {
            Value::Null => HandlerResultEntry::None,
            Value::String(s) => HandlerResultEntry::Str(s.clone()),
            Value::Array(a) => HandlerResultEntry::Tuple(match a.as_slice() {
                [c] => HandlerTuple::One(text(c)),
                [c, t] => HandlerTuple::Two(text(c), text(t)),
                [c, t, g] => HandlerTuple::Three(
                    text(c),
                    text(t),
                    match g {
                        Value::Null => None,
                        Value::Array(g) => Some(g.iter().filter_map(text).collect()),
                        other => unimplemented!(
                            "a plugin hook result with {other:?} where tags are expected"
                        ),
                    },
                ),
                _ => HandlerTuple::Other,
            }),
            _ => HandlerResultEntry::Other,
        }
    }
    match v {
        Value::Null => None,
        Value::Array(a) => Some(HandlerResults::List(a.iter().map(entry).collect())),
        other => Some(HandlerResults::Entry(entry(other))),
    }
}

/// `str.encode(self._serial_encoding, errors="replace")` (`replace` true) or strict: the codecs
/// the setting can name here are `ascii`, `utf-8` and `latin-1` (as `decode_serial_bytes`); a
/// character the codec cannot encode is `?` (replace) or the `UnicodeEncodeError` panic;
/// another codec is `unimplemented!`.
fn encode_serial_text(text: &str, encoding: &Option<Value>, replace: bool) -> Vec<u8> {
    let name = match encoding {
        Some(Value::String(s)) => s.to_lowercase().replace('_', "-"),
        other => panic!("TypeError: encode() argument 'encoding' must be str, not {other:?}"),
    };
    let limit: u32 = match name.as_str() {
        "utf-8" | "utf8" | "u8" => return text.as_bytes().to_vec(),
        "ascii" | "us-ascii" | "646" => 0x80,
        "latin-1" | "latin1" | "iso-8859-1" | "iso8859-1" | "l1" => 0x100,
        other => unimplemented!("str.encode with the codec {other:?}"),
    };
    text.chars()
        .map(|c| {
            if (c as u32) < limit {
                c as u8
            } else if replace {
                b'?'
            } else {
                panic!("UnicodeEncodeError: '{name}' codec can't encode character {c:?}")
            }
        })
        .collect()
}

/// `bytes.decode(self._serial_encoding)` (comm.py:4467): the codecs the setting can name here are
/// `ascii`, `utf-8` and `latin-1`; a failure is `UnicodeDecodeError` (a panic, difference 3);
/// another codec is `unimplemented!`.
fn decode_serial_bytes(bytes: &[u8], encoding: &Option<Value>) -> String {
    let name = match encoding {
        Some(Value::String(s)) => s.to_lowercase().replace('_', "-"),
        other => panic!("TypeError: decode() argument 'encoding' must be str, not {other:?}"),
    };
    match name.as_str() {
        "ascii" | "us-ascii" | "646" => {
            if bytes.is_ascii() {
                bytes.iter().map(|b| *b as char).collect()
            } else {
                panic!("UnicodeDecodeError: 'ascii' codec can't decode bytes")
            }
        }
        "utf-8" | "utf8" | "u8" => String::from_utf8(bytes.to_vec())
            .expect("UnicodeDecodeError: 'utf-8' codec can't decode bytes"),
        "latin-1" | "latin1" | "iso-8859-1" | "iso8859-1" | "l1" => {
            bytes.iter().map(|b| *b as char).collect()
        }
        other => unimplemented!("bytes.decode with the codec {other:?}"),
    }
}

/// Python `repr(float)` / `str(float)`: the shortest text that reads back as the number, with an
/// exponent below 1e-4 and from 1e16 up (`10.0`, `0.5`, `1e-05`, `1e+16`).
pub(crate) fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".to_string();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0" } else { "0.0" }.to_string();
    }
    let sci = format!("{:e}", x.abs());
    let (mantissa, exp) = sci.split_once('e').expect("{:e} has an exponent");
    let exp: i32 = exp.parse().expect("exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let decpt = exp + 1;
    let sign = if x < 0.0 { "-" } else { "" };
    let n = digits.len() as i32;
    if -4 < decpt && decpt <= 16 {
        if decpt <= 0 {
            format!("{sign}0.{}{digits}", "0".repeat((-decpt) as usize))
        } else if decpt >= n {
            format!("{sign}{digits}{}.0", "0".repeat((decpt - n) as usize))
        } else {
            format!(
                "{sign}{}.{}",
                &digits[..decpt as usize],
                &digits[decpt as usize..]
            )
        }
    } else {
        let rest = if n > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        let e = decpt - 1;
        format!(
            "{sign}{}{rest}e{}{:02}",
            &digits[..1],
            if e < 0 { "-" } else { "+" },
            e.abs()
        )
    }
}

// ---------------------------------------------------------------------------------------------
// comm.py:5768-6045 PrintingFileInformation and its subclasses
// ---------------------------------------------------------------------------------------------

/// Python's `OSError` (a message): raised by the file classes and by `open`/`os.stat`.
#[derive(Debug, Clone, PartialEq)]
pub struct OSError(pub String);

/// `offsets_callback` of `PrintingGcodeFileInformation` (`self.getOffsets`): the dict as a map
pub type OffsetsCallback = Arc<dyn Fn() -> TemperatureOffsets + Send + Sync>;

/// `current_tool_callback` of `PrintingGcodeFileInformation` (`self.getCurrentTool`)
pub type CurrentToolCallback = Arc<dyn Fn() -> i64 + Send + Sync>;

/// What `getNext` returns: `(line, pos, read_lines)`, all `None` for "no more lines".
pub type NextLine = (Option<String>, Option<i64>, Option<i64>);

/// comm.py:5768 PrintingFileInformation
///
/// Encapsulates information regarding the current file being printed: file name, current position,
/// total size and time the print started. Allows to reset the current file position to 0 and to
/// calculate the current progress as a floating point value between 0 and 1.
///
/// Python's class hierarchy is this trait (the methods of the base class, with its behaviour as
/// the default) and four structs that embed [`BasePrintingFileInformation`] (difference 11).
/// Methods that only subclasses have (`seek`, `getNext`, `getLocalFilename`, ...) are an
/// `AttributeError` (a panic, difference 3) on the others, as in Python.
pub trait PrintingFileInformation: Send + Sync {
    /// the attributes of the base class
    fn base(&self) -> &BasePrintingFileInformation;

    /// `checksum` (class attribute, `True`)
    fn checksum(&self) -> bool {
        true
    }

    /// comm.py:5787
    fn get_start_time(&self) -> Option<f64> {
        self.base()._start_time.get()
    }

    /// comm.py:5790
    fn get_filename(&self) -> String {
        self.base()._filename.clone()
    }

    /// comm.py:5793
    fn get_filesize(&self) -> Option<i64> {
        self.base()._size.get()
    }

    /// comm.py:5796
    fn get_filepos(&self) -> i64 {
        self.base()._pos.get()
    }

    /// comm.py:5799 (`FileDestinations.LOCAL`)
    fn get_file_location(&self) -> &'static str {
        FileDestinations::LOCAL
    }

    /// comm.py:5802
    fn get_user(&self) -> Option<String> {
        self.base()._user.clone()
    }

    /// comm.py:5805 `getProgress()`: the current progress of the file, calculated as relation
    /// between file position and absolute size. Returns -1 if file size is None or < 1.
    fn get_progress(&self) -> f64 {
        let size = self.base()._size.get();
        match size {
            Some(size) if size > 0 => self.base()._pos.get() as f64 / size as f64,
            _ => -1.0,
        }
    }

    /// comm.py:5814 `reset()`: resets the current file position to 0.
    fn reset(&self) {
        self.base()._pos.set(0);
    }

    /// comm.py:5820 `start()`: marks the print job as started and remembers the start time.
    /// `Err` is what the subclasses' `open`/`get_bom` raise.
    fn start(&self) -> Result<(), OSError> {
        self.base()._start_time.set(Some(time_monotonic()));
        self.base()._done.set(false);
        Ok(())
    }

    /// comm.py:5827 `close()`: closes the print job.
    fn close(&self) {}

    /// comm.py:5834 `done` (property)
    fn done(&self) -> bool {
        self.base()._done.get()
    }

    /// comm.py:5838 `done` (setter)
    fn set_done(&self, value: bool) {
        self.base()._done.set(value);
    }

    /// `isinstance(x, PrintingSdFileInformation)`
    fn is_printing_sd_file_information(&self) -> bool {
        false
    }

    /// `isinstance(x, StreamingGcodeFileInformation)`
    fn is_streaming_gcode_file_information(&self) -> bool {
        false
    }

    /// `x.pos = value` (the setter of `PrintingSdFileInformation`, comm.py:5869). On any other
    /// class Python would create an attribute `pos` that nothing reads; here that is nothing.
    fn set_pos(&self, _value: i64) {}

    /// `size` (property of `PrintingSdFileInformation`, comm.py:5857): an `AttributeError` on the
    /// other classes.
    fn size(&self) -> Option<i64> {
        panic!("AttributeError: object has no attribute 'size'")
    }

    /// `size` (setter, comm.py:5861)
    fn set_size(&self, _value: Option<i64>) {
        panic!("AttributeError: object has no attribute 'size'")
    }

    /// `seek(offset)` (`PrintingGcodeFileInformation`, comm.py:5896)
    fn seek(&self, _offset: i64) -> Result<(), OSError> {
        panic!("AttributeError: object has no attribute 'seek'")
    }

    /// `getNext()` (`PrintingGcodeFileInformation`, comm.py:5937): `Err` is the exception it
    /// re-raises.
    fn get_next(&self) -> Result<NextLine, OSError> {
        panic!("AttributeError: object has no attribute 'getNext'")
    }

    /// `getLocalFilename()` (`StreamingGcodeFileInformation`, comm.py:6003)
    fn get_local_filename(&self) -> String {
        panic!("AttributeError: object has no attribute 'getLocalFilename'")
    }

    /// `getRemoteFilename()` (`StreamingGcodeFileInformation`, comm.py:6006)
    fn get_remote_filename(&self) -> String {
        panic!("AttributeError: object has no attribute 'getRemoteFilename'")
    }
}

/// comm.py:5768 PrintingFileInformation (the class itself, and the attributes its subclasses
/// share): `__init__(self, filename, user=None)` (comm.py:5778). `_logger` is not ported.
pub struct BasePrintingFileInformation {
    pub _filename: String,
    pub _user: Option<String>,
    pub _pos: Attr<i64>,
    pub _size: Attr<Option<i64>>,
    pub _start_time: Attr<Option<f64>>,
    pub _done: Attr<bool>,
}

impl BasePrintingFileInformation {
    pub fn new(filename: &str, user: Option<&str>) -> Self {
        BasePrintingFileInformation {
            _filename: filename.to_string(),
            _user: user.map(str::to_string),
            _pos: Attr::new(0),
            _size: Attr::new(None),
            _start_time: Attr::new(None),
            _done: Attr::new(false),
        }
    }
}

impl PrintingFileInformation for BasePrintingFileInformation {
    fn base(&self) -> &BasePrintingFileInformation {
        self
    }
}

/// comm.py:5842 PrintingSdFileInformation: encapsulates information regarding an ongoing print
/// from SD.
pub struct PrintingSdFileInformation {
    base: BasePrintingFileInformation,
}

impl PrintingSdFileInformation {
    /// comm.py:5849 `__init__(self, filename, size, user=None)`
    pub fn new(filename: &str, size: Option<i64>, user: Option<&str>) -> Self {
        let base = BasePrintingFileInformation::new(filename, user);
        base._size.set(size);
        PrintingSdFileInformation { base }
    }

    /// comm.py:5857 `size` (property)
    pub fn size(&self) -> Option<i64> {
        self.base._size.get()
    }

    /// comm.py:5861 `size` (setter)
    pub fn set_size(&self, value: Option<i64>) {
        self.base._size.set(value);
    }
}

impl PrintingFileInformation for PrintingSdFileInformation {
    fn base(&self) -> &BasePrintingFileInformation {
        &self.base
    }

    /// `checksum = False`
    fn checksum(&self) -> bool {
        false
    }

    /// comm.py:5853 (`FileDestinations.SDCARD`)
    fn get_file_location(&self) -> &'static str {
        FileDestinations::SDCARD
    }

    fn is_printing_sd_file_information(&self) -> bool {
        true
    }

    /// comm.py:5869 `pos` (setter)
    fn set_pos(&self, value: i64) {
        self.base._pos.set(value);
    }

    fn size(&self) -> Option<i64> {
        self.base._size.get()
    }

    fn set_size(&self, value: Option<i64>) {
        self.base._size.set(value);
    }
}

/// The text handle `open(filename, encoding="utf-8-sig", errors="replace", newline="")` returns:
/// a file read line by line (`readline`), lines end in `\n`, `\r` or `\r\n` and are returned as
/// they are (`newline=""`), the stream is decoded as UTF-8 with undecodable bytes replaced by
/// U+FFFD, and a BOM at the start of the stream (`first`, the `utf_8_sig` decoder's flag) is
/// dropped. OURS: Python reads the file in chunks; the lines are the same.
struct TextHandle {
    reader: std::io::BufReader<std::fs::File>,
    first: bool,
}

impl TextHandle {
    /// `open(filename, encoding="utf-8-sig", errors="replace", newline="")`
    fn open(filename: &str) -> std::io::Result<TextHandle> {
        Ok(TextHandle {
            reader: std::io::BufReader::new(std::fs::File::open(filename)?),
            first: true,
        })
    }

    /// `readline()`: `""` at the end of the file
    fn readline(&mut self) -> std::io::Result<String> {
        use std::io::BufRead;
        let mut line: Vec<u8> = Vec::new();
        loop {
            let (done, used) = {
                let available = self.reader.fill_buf()?;
                if available.is_empty() {
                    (true, 0)
                } else {
                    match available.iter().position(|b| *b == b'\n' || *b == b'\r') {
                        Some(i) => {
                            line.extend_from_slice(&available[..=i]);
                            (true, i + 1)
                        }
                        None => {
                            line.extend_from_slice(available);
                            (false, available.len())
                        }
                    }
                }
            };
            self.reader.consume(used);
            if done {
                break;
            }
        }
        // `\r\n` is one line ending
        if line.last() == Some(&b'\r') {
            let next = self.reader.fill_buf()?;
            if next.first() == Some(&b'\n') {
                line.push(b'\n');
                self.reader.consume(1);
            }
        }
        if self.first && !line.is_empty() {
            self.first = false;
            if line.starts_with(&[0xef, 0xbb, 0xbf]) {
                line.drain(..3);
            }
        }
        Ok(String::from_utf8_lossy(&line).into_owned())
    }

    /// `seek(offset)` of the text file: a position in bytes; at 0 the decoder is reset (a BOM is
    /// dropped again), at any other position its state is "not first" (`_pyio.py:2551-2555`,
    /// `encodings/utf_8_sig.py:70-81`).
    fn seek(&mut self, offset: u64) -> std::io::Result<()> {
        use std::io::Seek;
        self.reader.seek(std::io::SeekFrom::Start(offset))?;
        self.first = offset == 0;
        Ok(())
    }
}

/// comm.py:5873 PrintingGcodeFileInformation
///
/// Encapsulates information regarding an ongoing direct print. Takes care of the needed file
/// handle and ensures that the file is closed in case of an error.
pub struct PrintingGcodeFileInformation {
    base: BasePrintingFileInformation,
    _handle: Attr<Option<TextHandle>>,
    _handle_mutex: RLock,
    _offsets_callback: Option<OffsetsCallback>,
    _current_tool_callback: Option<CurrentToolCallback>,
    _read_lines: Attr<i64>,
}

impl PrintingGcodeFileInformation {
    /// comm.py:5879 `__init__(self, filename, offsets_callback=None, current_tool_callback=None,
    /// user=None)`; `Err` is the `OSError` for a file that does not exist
    pub fn new(
        filename: &str,
        offsets_callback: Option<OffsetsCallback>,
        current_tool_callback: Option<CurrentToolCallback>,
        user: Option<&str>,
    ) -> Result<Self, OSError> {
        let base = BasePrintingFileInformation::new(filename, user);

        // os.path.exists / os.path.isfile / os.stat(...).st_size
        let metadata = match std::fs::metadata(filename) {
            Ok(m) if m.is_file() => m,
            _ => return Err(OSError(format!("File {filename} does not exist"))),
        };
        base._size.set(Some(metadata.len() as i64));
        base._pos.set(0);
        Ok(PrintingGcodeFileInformation {
            base,
            _handle: Attr::new(None),
            _handle_mutex: RLock::new(),
            _offsets_callback: offsets_callback,
            _current_tool_callback: current_tool_callback,
            _read_lines: Attr::new(0),
        })
    }

    /// comm.py:5896 `seek(offset)`
    pub fn seek(&self, offset: i64) -> Result<(), OSError> {
        let _g = self._handle_mutex.lock();
        if self._handle.with(|h| h.is_none()) {
            return Ok(());
        }

        let offset = u64::try_from(offset).expect("ValueError: negative seek position");
        self._handle
            .with_mut(|h| h.as_mut().expect("checked above").seek(offset))
            .map_err(|e| OSError(e.to_string()))?;
        self.base._pos.set(offset as i64); // `self._handle.tell()`
        self._read_lines.set(0);
        Ok(())
    }

    /// comm.py:5905 `start()`: opens the file for reading and determines the file size.
    pub fn start(&self) -> Result<(), OSError> {
        PrintingFileInformation::start(&self.base)?;
        let _g = self._handle_mutex.lock();
        let bom = get_bom(&self.base._filename, "utf-8-sig").map_err(|e| OSError(e.to_string()))?;
        let handle = TextHandle::open(&self.base._filename).map_err(|e| OSError(e.to_string()))?;
        self._handle.set(Some(handle));
        self.base._pos.set(0); // `self._handle.tell()`
        if let Some(bom) = bom {
            // Apparently we found an utf-8 bom in the file.
            // We need to add its length to our pos because it will
            // be stripped transparently and we'll have no chance
            // catching that.
            self.base._pos.set(self.base._pos.get() + bom.len() as i64);
        }
        self._read_lines.set(0);
        Ok(())
    }

    /// comm.py:5924 `close()`: closes the file if it's still open.
    pub fn close(&self) {
        let _g = self._handle_mutex.lock();
        // `self._handle.close()` cannot raise here; dropping the handle closes it
        self._handle.set(None);
    }

    /// comm.py:5937 `getNext()` with the `_process` (comm.py:5984) and `_report_stats`
    /// (comm.py:5987) of the class that calls it
    fn get_next_with(
        &self,
        process: &dyn Fn(&str, Option<&TemperatureOffsets>, Option<i64>) -> Option<String>,
        report_stats: &dyn Fn(),
    ) -> Result<NextLine, OSError> {
        let _g = self._handle_mutex.lock();
        if self._handle.with(|h| h.is_none()) {
            // self._logger.warning(f"File {self._filename} is not open for reading")
            return Ok((None, None, None));
        }

        let body = || -> Result<NextLine, OSError> {
            let offsets = self._offsets_callback.as_ref().map(|cb| cb());
            let current_tool = self._current_tool_callback.as_ref().map(|cb| cb());

            let mut processed: Option<String> = None;
            while processed.is_none() {
                if self._handle.with(|h| h.is_none()) {
                    // file got closed just now
                    self.base
                        ._pos
                        .set(self.base._size.get().expect("TypeError: _pos = None"));
                    self.base._done.set(true);
                    report_stats();
                    return Ok((None, None, None));
                }

                // we need to manually keep track of our pos here since
                // codecs' readline will make our handle's tell not
                // return the actual number of bytes read, but also the
                // already buffered bytes (for detecting the newlines)
                let line = self
                    ._handle
                    .with_mut(|h| h.as_mut().expect("checked above").readline())
                    .map_err(|e| OSError(e.to_string()))?;
                self.base._pos.set(self.base._pos.get() + line.len() as i64);

                if line.is_empty() {
                    self.close();
                }
                processed = process(&line, offsets.as_ref(), current_tool);
            }
            self._read_lines.set(self._read_lines.get() + 1);
            Ok((
                processed,
                Some(self.base._pos.get()),
                Some(self._read_lines.get()),
            ))
        };

        // except Exception as e: self.close(); self._logger.exception(...); raise e
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
            Ok(Ok(next)) => Ok(next),
            Ok(Err(e)) => {
                self.close();
                Err(e)
            }
            Err(panic) => {
                self.close();
                std::panic::resume_unwind(panic)
            }
        }
    }

    /// comm.py:5984 `_process`
    fn _process(
        line: &str,
        offsets: Option<&TemperatureOffsets>,
        current_tool: Option<i64>,
    ) -> Option<String> {
        process_gcode_line(line, offsets, current_tool)
    }

    /// comm.py:5987 `_report_stats`: only logs (`duration = time.monotonic() - self._start_time`,
    /// `self._logger.info(f"Finished in {duration:.3f} s.")`)
    fn _report_stats(&self) {}
}

impl PrintingFileInformation for PrintingGcodeFileInformation {
    fn base(&self) -> &BasePrintingFileInformation {
        &self.base
    }

    fn start(&self) -> Result<(), OSError> {
        PrintingGcodeFileInformation::start(self)
    }

    fn close(&self) {
        PrintingGcodeFileInformation::close(self)
    }

    fn seek(&self, offset: i64) -> Result<(), OSError> {
        PrintingGcodeFileInformation::seek(self, offset)
    }

    fn get_next(&self) -> Result<NextLine, OSError> {
        self.get_next_with(&Self::_process, &|| self._report_stats())
    }
}

/// comm.py:5993 StreamingGcodeFileInformation
#[allow(non_snake_case)]
pub struct StreamingGcodeFileInformation {
    inner: PrintingGcodeFileInformation,
    _localFilename: String,
    _remoteFilename: String,
}

impl StreamingGcodeFileInformation {
    /// comm.py:5994 `__init__(self, path, localFilename, remoteFilename, user=None)`
    pub fn new(
        path: &str,
        local_filename: &str,
        remote_filename: &str,
        user: Option<&str>,
    ) -> Result<Self, OSError> {
        Ok(StreamingGcodeFileInformation {
            inner: PrintingGcodeFileInformation::new(path, None, None, user)?,
            _localFilename: local_filename.to_string(),
            _remoteFilename: remote_filename.to_string(),
        })
    }

    /// comm.py:6009 `_process`
    fn _process(
        line: &str,
        _offsets: Option<&TemperatureOffsets>,
        _current_tool: Option<i64>,
    ) -> Option<String> {
        process_gcode_line(line, None, None)
    }

    /// comm.py:6012 `_report_stats`: only logs (the transfer rate)
    fn _report_stats(&self) {
        // duration = time.monotonic() - self._start_time; read_lines = self._read_lines; ...
        // self._logger.info("Finished in {duration:.3f} s. Approx. transfer rate of ...")
    }

    fn start_streaming(&self) -> Result<(), OSError> {
        self.inner.start()?;
        self.inner.base._start_time.set(Some(time_monotonic()));
        Ok(())
    }
}

impl PrintingFileInformation for StreamingGcodeFileInformation {
    fn base(&self) -> &BasePrintingFileInformation {
        &self.inner.base
    }

    /// comm.py:5999 `start()`
    fn start(&self) -> Result<(), OSError> {
        self.start_streaming()
    }

    fn close(&self) {
        self.inner.close()
    }

    fn seek(&self, offset: i64) -> Result<(), OSError> {
        self.inner.seek(offset)
    }

    fn get_next(&self) -> Result<NextLine, OSError> {
        self.inner
            .get_next_with(&Self::_process, &|| self._report_stats())
    }

    fn is_streaming_gcode_file_information(&self) -> bool {
        true
    }

    /// comm.py:6003
    fn get_local_filename(&self) -> String {
        self._localFilename.clone()
    }

    /// comm.py:6006
    fn get_remote_filename(&self) -> String {
        self._remoteFilename.clone()
    }
}

/// comm.py:6029 SpecialStreamingGcodeFileInformation
///
/// For streaming files to the printer that aren't GCODE.
///
/// Difference to regular StreamingGcodeFileInformation: no checksum requirement, only rudimentary
/// line processing (stripping of whitespace from the end and ignoring of empty lines)
pub struct SpecialStreamingGcodeFileInformation {
    inner: StreamingGcodeFileInformation,
}

impl SpecialStreamingGcodeFileInformation {
    /// `__init__` of `StreamingGcodeFileInformation` (comm.py:5994)
    pub fn new(
        path: &str,
        local_filename: &str,
        remote_filename: &str,
        user: Option<&str>,
    ) -> Result<Self, OSError> {
        Ok(SpecialStreamingGcodeFileInformation {
            inner: StreamingGcodeFileInformation::new(path, local_filename, remote_filename, user)?,
        })
    }

    /// comm.py:6039 `_process`
    fn _process(
        line: &str,
        _offsets: Option<&TemperatureOffsets>,
        _current_tool: Option<i64>,
    ) -> Option<String> {
        let line = line.trim_end_matches(py_isspace);
        if line.is_empty() {
            return None;
        }
        Some(line.to_string())
    }
}

impl PrintingFileInformation for SpecialStreamingGcodeFileInformation {
    fn base(&self) -> &BasePrintingFileInformation {
        self.inner.base()
    }

    /// `checksum = False`
    fn checksum(&self) -> bool {
        false
    }

    fn start(&self) -> Result<(), OSError> {
        self.inner.start_streaming()
    }

    fn close(&self) {
        self.inner.close()
    }

    fn seek(&self, offset: i64) -> Result<(), OSError> {
        self.inner.seek(offset)
    }

    fn get_next(&self) -> Result<NextLine, OSError> {
        self.inner
            .inner
            .get_next_with(&Self::_process, &|| self.inner._report_stats())
    }

    fn is_streaming_gcode_file_information(&self) -> bool {
        true
    }

    fn get_local_filename(&self) -> String {
        self.inner.get_local_filename()
    }

    fn get_remote_filename(&self) -> String {
        self.inner.get_remote_filename()
    }
}

/// comm.py:193 SDFileData `namedtuple("SDFileData", ["name", "size", "timestamp", "longname"])`
/// (`longname` is `False` for "none" in Python: `None` here, difference 2)
#[derive(Debug, Clone, PartialEq)]
pub struct SDFileData {
    pub name: String,
    pub size: Option<i64>,
    pub timestamp: Option<i64>,
    pub longname: Option<String>,
}

/// comm.py:196 `serialList()`: the serial ports to try, the last used one first.
///
/// On Windows (`os.name == "nt"`) the values of the registry key
/// `HKEY_LOCAL_MACHINE\HARDWARE\DEVICEMAP\SERIALCOMM` ([`winreg_serialcomm`]); elsewhere "/dev" is
/// scanned for `regex_serial_devices` ([`scan_dev`]).
pub fn serial_list() -> Vec<String> {
    // OURS (docs/archive/plans/agent-engine.md, Section H): under `cfg(test)` the scanned directory is a temp
    // directory, never the real `/dev`
    #[cfg(test)]
    return serial_list_in(&test_serial_dir());
    #[cfg(not(test))]
    serial_list_in("/dev")
}

/// OURS (test guard): the directory `serial_list()` scans under `cfg(test)`: an empty directory in
/// the temp directory.
#[cfg(test)]
pub(crate) fn test_serial_dir() -> String {
    let dir = std::env::temp_dir().join("maker-connect-test-serial-dev");
    std::fs::create_dir_all(&dir).expect("create the temp directory that stands in for /dev");
    dir.to_str()
        .expect("a temp directory with a text name")
        .to_string()
}

/// OURS (test seam, `port.rs`'s tests): under `cfg(test)` a port name that a test registered here opens as
/// that serial object, in place of a device (a fake printer in memory). Like the guard below, it is
/// compiled only into the tests.
#[cfg(test)]
pub(crate) static TEST_TRANSPORTS: std::sync::Mutex<Vec<(String, Arc<dyn SerialTransport>)>> =
    std::sync::Mutex::new(Vec::new());

/// OURS (test guard): under `cfg(test)` `_open_serial` panics on any port outside the temp directory,
/// except the nonexistent `/dev/maker-connect-test-does-not-exist`. No test may open a real device.
#[cfg(test)]
fn test_guard_open_serial(port: &str) {
    if port == "/dev/maker-connect-test-does-not-exist" {
        return;
    }
    let temp = std::env::temp_dir();
    let inside = |p: &std::path::Path, t: &std::path::Path| {
        // no `..` that could leave the directory, compared as written and as resolved
        !p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
            && (p.starts_with(t) || t.canonicalize().is_ok_and(|t| p.starts_with(t)))
    };
    if !inside(std::path::Path::new(port), &temp) {
        panic!(
            "test guard: _open_serial({port:?}) is outside the temp directory {temp:?}; no test may open a real device"
        );
    }
}

/// `serialList()` with the directory it scans (OURS: so that a test need not look at the real
/// `/dev`; `serialList` itself passes `"/dev"`).
fn serial_list_in(dev: &str) -> Vec<String> {
    #[cfg(windows)]
    let mut candidates: Vec<String> = {
        let _ = dev;
        // OURS (test guard, as `test_serial_dir` for "/dev"): under `cfg(test)` the real registry
        // is not read, so no test finds a real port; `winreg_serialcomm` has its own test.
        if cfg!(test) {
            Vec::new()
        } else {
            winreg_serialcomm()
        }
    };
    #[cfg(not(windows))]
    let mut candidates: Vec<String> = scan_dev(dev);

    // additional ports
    let additional_ports = settings().get(&["serial", "additionalPorts"]);
    if let Some(additional_ports) = additional_ports.as_ref().filter(|v| py_truthy(v)) {
        let Value::Array(additional_ports) = additional_ports else {
            panic!(
                "TypeError: the serial.additionalPorts setting is not a list: {additional_ports:?}"
            );
        };
        for additional in additional_ports {
            let pattern = additional
                .as_str()
                .expect("TypeError: expected str, bytes or os.PathLike object");
            candidates.extend(py_glob::glob(pattern));
        }
    }

    let hooks = plugin_manager().get_hooks("octoprint.comm.transport.serial.additional_port_names");
    for (_name, hook) in hooks.iter() {
        let args = [Value::Array(
            candidates
                .iter()
                .map(|c| Value::String(c.clone()))
                .collect(),
        )];
        match py_try(|| hook(&args)) {
            Err(_) => {
                // logging.getLogger(__name__).exception("Error while retrieving additional serial port names from hook {}")
            }
            Ok(Value::Array(names)) => {
                for n in names {
                    candidates.push(n.as_str().expect("a serial port name is a str").to_string());
                }
            }
            // `candidates += "abc"` extends by the characters
            Ok(Value::String(s)) => candidates.extend(s.chars().map(|c| c.to_string())),
            Ok(other) => {
                // `candidates += <not iterable>` is a TypeError, caught by `except Exception`
                let _ = other;
            }
        }
    }

    // blacklisted ports
    let blacklisted_ports = settings().get(&["serial", "blacklistedPorts"]);
    if blacklisted_ports.as_ref().is_some_and(py_truthy) {
        let Some(Value::Array(patterns)) = settings().get(&["serial", "blacklistedPorts"]) else {
            panic!("TypeError: the serial.blacklistedPorts setting is not a list");
        };
        for pattern in patterns {
            let pattern = pattern
                .as_str()
                .expect("TypeError: expected str, bytes or os.PathLike object, not non-str");
            candidates.retain(|x| !py_glob::fnmatch(x, pattern));
        }
    }

    // last used port = first to try, move to start
    let prev = settings().get(&["serial", "port"]);
    if let Some(prev) = prev.as_ref().and_then(Value::as_str)
        && let Some(i) = candidates.iter().position(|c| c == prev)
    {
        let prev = candidates.remove(i);
        candidates.insert(0, prev);
    }

    candidates
}

/// comm.py:211-220, the `else` branch of `serialList`: `os.scandir("/dev")`, the entries whose name
/// matches `regex_serial_devices`, as `entry.path`; any error ends the scan (`except Exception`, logged).
#[cfg_attr(windows, allow(dead_code))]
fn scan_dev(dev: &str) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    // try: with os.scandir("/dev") as it: ... except Exception: logger.exception(...)
    if let Ok(it) = std::fs::read_dir(dev) {
        for entry in it {
            let Ok(entry) = entry else { break };
            let name = entry.file_name().to_string_lossy().into_owned();
            if re_match(&regex_serial_devices, &name).is_some() {
                candidates.push(entry.path().to_string_lossy().into_owned());
            }
        }
    }
    candidates
}

/// comm.py:198-208, the `os.name == "nt"` branch of `serialList`:
///
/// ```python
/// key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, "HARDWARE\\DEVICEMAP\\SERIALCOMM")
/// i = 0
/// while True:
///     candidates += [winreg.EnumValue(key, i)[1]]
///     i += 1
/// except Exception: pass
/// ```
///
/// `winreg.OpenKey` opens with `KEY_READ`; `EnumValue(key, i)[1]` is the value's data, which for
/// `REG_SZ`/`REG_EXPAND_SZ` is a `str` up to its first NUL (CPython `PC/winreg.c` `Reg2Py`). The first
/// error, `ERROR_NO_MORE_ITEMS` at the end included, ends the list with what was read.
///
/// OURS: a value of another type would be an `int`, `bytes` or `list` in Python's list, which a list of
/// port names cannot hold: it ends the list here (the device map holds `REG_SZ` port names). The key is
/// closed at the end (Python leaves that to the garbage collector).
#[cfg(windows)]
pub fn winreg_serialcomm() -> Vec<String> {
    use windows_sys::Win32::Foundation::ERROR_MORE_DATA;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegEnumValueW,
        RegOpenKeyExW,
    };
    let mut candidates = Vec::new();
    let path: Vec<u16> = "HARDWARE\\DEVICEMAP\\SERIALCOMM"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: `path` is NUL-terminated; `key` receives the opened key.
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &mut key) }
        != ERROR_SUCCESS
    {
        return candidates;
    }
    let mut i: u32 = 0;
    // value names are at most 16383 characters (the registry's limit)
    let mut name = vec![0u16; 16384];
    let mut data: Vec<u8> = vec![0u8; 256];
    loop {
        let mut name_len = name.len() as u32;
        let mut data_len = data.len() as u32;
        let mut kind: u32 = 0;
        // SAFETY: the buffers and their lengths are what the call fills.
        let rc = unsafe {
            RegEnumValueW(
                key,
                i,
                name.as_mut_ptr(),
                &mut name_len,
                std::ptr::null(),
                &mut kind,
                data.as_mut_ptr(),
                &mut data_len,
            )
        };
        if rc == ERROR_MORE_DATA {
            // the data did not fit: grow to the size the call asked for and read this value again
            data = vec![0u8; (data_len as usize).max(data.len() * 2)];
            continue;
        }
        if rc != ERROR_SUCCESS || (kind != REG_SZ && kind != REG_EXPAND_SZ) {
            break;
        }
        let wide: Vec<u16> = data[..data_len as usize]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|c| *c != 0)
            .collect();
        candidates.push(String::from_utf16_lossy(&wide));
        i += 1;
    }
    // SAFETY: closing the key opened above.
    unsafe { RegCloseKey(key) };
    candidates
}

/// comm.py:258 `baudrateList(candidates=None)`: the baud rates to try, the last used one first.
pub fn baudrate_list(candidates: Option<Vec<i64>>) -> Vec<i64> {
    let mut candidates = candidates.unwrap_or_else(|| {
        // sorted by likelihood
        vec![115200, 250000, 230400, 57600, 38400, 19200, 9600]
    });

    // additional baudrates prepended, sorted descending
    let additional_baudrates = settings().get(&["serial", "additionalBaudrates"]);
    let Some(Value::Array(mut additional_baudrates)) = additional_baudrates else {
        panic!("TypeError: the serial.additionalBaudrates setting is not a list");
    };
    // `sorted(additionalBaudrates, reverse=True)`: numbers among themselves, strings among themselves
    if additional_baudrates.iter().all(|v| v.is_number()) {
        additional_baudrates.sort_by(|a, b| {
            b.as_f64()
                .partial_cmp(&a.as_f64())
                .expect("a number setting is no NaN")
        });
    } else if additional_baudrates.iter().all(|v| v.is_string()) {
        additional_baudrates.sort_by(|a, b| b.as_str().cmp(&a.as_str()));
    } else if additional_baudrates.len() > 1 {
        panic!("TypeError: '<' not supported between instances of 'str' and 'int'");
    }
    for additional in &additional_baudrates {
        // try: candidates.insert(0, int(additional)) except Exception: (warn and ignore)
        let parsed = match additional {
            Value::Number(_) | Value::Bool(_) | Value::String(_) => py_int_value(additional).ok(),
            _ => None,
        };
        match parsed {
            Some(b) => candidates.insert(0, b),
            None => {
                // _logger.warning(f"{additional} is not a valid additional baudrate, ignoring it")
            }
        }
    }

    // blacklisted baudrates
    let blacklisted_baudrates = settings().get(&["serial", "blacklistedBaudrates"]);
    if blacklisted_baudrates.as_ref().is_some_and(py_truthy) {
        let Some(Value::Array(blacklisted)) = blacklisted_baudrates else {
            panic!("TypeError: the serial.blacklistedBaudrates setting is not a list");
        };
        for baudrate in &blacklisted {
            let pos = candidates.iter().position(|c| match baudrate {
                Value::Number(n) => n.as_f64() == Some(*c as f64),
                Value::Bool(b) => *c == *b as i64,
                _ => false,
            });
            match pos {
                Some(i) => {
                    candidates.remove(i);
                }
                None => panic!("ValueError: list.remove(x): x not in list"),
            }
        }
    }

    // last used baudrate = first to try, move to start
    let prev = settings().get_int(&["serial", "baudrate"]);
    if let Some(prev) = prev
        && let Some(i) = candidates.iter().position(|c| *c == prev)
    {
        candidates.remove(i);
        candidates.insert(0, prev);
    }

    candidates
}

/// `getFilePosition()`'s dict (comm.py:2159)
#[derive(Debug, Clone, PartialEq)]
pub struct FilePosition {
    pub origin: &'static str,
    pub filename: String,
    pub pos: i64,
}

/// Python truthiness of a `getBoolean` result (`None` is falsy).
fn truthy(b: Option<bool>) -> bool {
    b == Some(true)
}

/// `float(value)` for a settings value (comm.py:547): `Err` is `ValueError`; a value of another
/// type is `TypeError` (uncaught, a panic: difference 3).
fn py_float_value(v: &Value) -> Result<f64, ()> {
    match v {
        Value::Number(n) => Ok(n.as_f64().expect("float(): number out of range")),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => py_float(s).ok_or(()),
        other => {
            panic!("TypeError: float() argument must be a string or a real number, not {other:?}")
        }
    }
}

/// `int(value)` for a settings value (comm.py:558); see [`py_float_value`].
fn py_int_value(v: &Value) -> Result<i64, ()> {
    match v {
        Value::Number(n) => match n.as_i64() {
            Some(i) => Ok(i),
            None => Ok(n.as_f64().expect("int(): number out of range").trunc() as i64),
        },
        Value::Bool(b) => Ok(*b as i64),
        Value::String(s) => py_int(s).ok_or(()),
        other => {
            panic!("TypeError: int() argument must be a string or a real number, not {other:?}")
        }
    }
}

/// comm.py:444 MachineCom
///
/// Fields keep the Python attribute names (difference 2); `Attr<T>` fields are the ones assigned
/// after `__init__` or changed in place (`scripts/octoprint-machinecom-attrs.py` lists them:
/// `reassigned` / `mutated`). The three loggers `_logger`, `_serialLogger`, `_phaseLogger` are not
/// ported (difference 1).
#[allow(non_snake_case)]
pub struct MachineCom {
    pub _port: Option<String>,
    pub _baudrate: i64,
    pub _callback: Arc<dyn MachineComPrintCallback>,
    pub _printerProfileManager: Option<Arc<PrinterProfileManager>>,
    pub _state: Attr<i64>,
    pub _serial: Attr<Option<Arc<dyn SerialTransport>>>,

    pub _detection_candidates: Attr<Vec<(String, i64)>>,
    pub _detection_retry: Attr<i64>,

    pub _temperatureTargetSetThreshold: i64,
    pub _tempOffsets: Attr<PyDict<f64>>,
    pub _command_queue: CommandQueue<CommandQueueItem, String>,
    pub _currentZ: Attr<Option<f64>>,
    pub _currentF: Attr<Option<f64>>,
    pub _heatupWaitStartTime: Attr<Option<f64>>,
    pub _heatupWaitTimeLost: Attr<f64>,
    pub _pauseWaitStartTime: Attr<Option<f64>>,
    pub _pauseWaitTimeLost: Attr<f64>,
    pub _currentTool: Attr<i64>,
    pub _toolBeforeChange: Attr<Option<i64>>,
    pub _toolBeforeHeatup: Attr<Option<i64>>,

    pub _known_invalid_tools: Attr<BTreeSet<String>>,
    pub _known_invalid_custom_temps: Attr<BTreeSet<String>>,
    pub _known_broken_temperature_hooks: Attr<BTreeSet<String>>,

    pub _long_running_command: Attr<bool>,
    pub _heating: Attr<bool>,
    /// `False` or a `time.monotonic()` value: `None` is `False` (difference 2)
    pub _dwelling_until: Attr<Option<f64>>,
    pub _connection_closing: Attr<bool>,

    pub _timeout: Attr<Option<f64>>,
    pub _ok_timeout: Attr<Option<f64>>,
    pub _timeout_intervals: Attr<PyDict<f64>>,

    pub _consecutive_timeouts: Attr<i64>,
    pub _consecutive_timeout_maximums: Attr<PyDict<i64>>,

    pub _max_write_passes: Option<i64>,

    pub _hello_command: Option<Value>,
    pub _hello_sent: Attr<i64>,
    pub _trigger_ok_for_m29: Option<bool>,

    pub _alwaysSendChecksum: Attr<Option<bool>>,
    pub _neverSendChecksum: Option<bool>,
    pub _sendChecksumWithUnknownCommands: Attr<Option<bool>>,
    pub _unknownCommandsNeedAck: Attr<Option<bool>>,
    pub _sdAlwaysAvailable: Attr<Option<bool>>,
    pub _sdRelativePath: Attr<Option<bool>>,
    pub _sdLowerCase: Attr<Option<bool>>,
    pub _sdCancelCommand: Option<Value>,
    pub _blockWhileDwelling: Attr<Option<bool>>,
    pub _send_m112_on_error: Option<bool>,
    pub _disable_sd_printing_detection: Attr<Option<bool>>,
    pub _current_line: Attr<i64>,
    pub _line_mutex: RLock,
    pub _resendDelta: Attr<Option<i64>>,

    /// capability name -> `getBoolean` result (`None` falsy)
    pub _capability_support: Attr<PyDict<Option<bool>>>,

    /// `deque` of what was sent (bytes: comm.py:4467 `.decode(self._serial_encoding)`)
    pub _lastLines: Attr<PyDeque<Vec<u8>>>,
    pub _lastCommError: Attr<Option<String>>,
    pub _lastResendNumber: Attr<Option<i64>>,
    pub _currentResendCount: Attr<i64>,

    pub _currentConsecutiveResendNumber: Attr<Option<i64>>,
    pub _currentConsecutiveResendCount: Attr<i64>,
    pub _maxConsecutiveResends: Option<i64>,

    pub _errorValue: Attr<String>,

    pub _firmware_detection: Option<bool>,
    pub _firmware_info_received: Attr<bool>,
    pub _firmware_capabilities_received: Attr<bool>,
    pub _firmware_info: Attr<PyDict<String>>,
    /// assigned in `_monitor` only (comm.py:2969), never read; `None` is "attribute not set"
    pub _firmware_name: Attr<Option<String>>,
    pub _firmware_capabilities: Attr<PyDict<bool>>,

    pub _defer_sd_refresh: Option<bool>,

    pub _temperature_autoreporting: Attr<bool>,
    pub _sdstatus_autoreporting: Attr<bool>,
    pub _pos_autoreporting: Attr<bool>,
    pub _busy_protocol_detected: Attr<bool>,
    pub _busy_protocol_support: Attr<bool>,

    pub _trigger_ok_after_resend: Option<Value>,
    pub _resend_ok_timer: Attr<Option<Arc<crate::ports::octoprint::py_threading::Timer>>>,

    pub _resendActive: Attr<bool>,

    pub _terminal_log: Attr<PyDeque<String>>,

    pub _disconnect_on_errors: Option<bool>,
    pub _ignore_errors: Option<bool>,

    pub _log_resends: Option<bool>,

    pub _log_resends_rate_start: Attr<Option<f64>>,
    pub _log_resends_rate_count: Attr<i64>,
    pub _log_resends_max: i64,
    pub _log_resends_rate_frame: i64,

    pub _long_running_commands: Option<Value>,
    pub _checksum_requiring_commands: Option<Value>,
    pub _blocked_commands: Option<Value>,
    pub _ignored_commands: Option<Value>,
    pub _pausing_commands: Option<Value>,
    pub _emergency_commands: Option<Value>,
    pub _sanity_check_tools: Option<bool>,

    pub _ack_max: Option<i64>,
    pub _clear_to_send: CountedEvent,
    pub _send_queue: SendQueue<SendQueueEntry, String>,
    pub _temperature_timer: Attr<Option<Arc<RepeatedTimer>>>,
    pub _sd_status_timer: Attr<Option<Arc<RepeatedTimer>>>,

    pub _consecutive_not_sd_printing: Attr<i64>,
    pub _consecutive_not_sd_printing_maximum: Option<i64>,

    pub _job_queue: JobQueue<CommandQueueItem>,

    pub _transmitted_lines: Attr<i64>,
    pub _received_resend_requests: Attr<i64>,
    pub _resend_ratio_start: Option<i64>,
    pub _resend_ratio_threshold: f64,
    pub _resend_ratio_reported: Attr<bool>,

    // hooks
    pub _pluginManager: &'static PluginManager,

    pub _gcode_hooks: PyDict<Hooks>,
    pub _received_message_hooks: Hooks,
    pub _error_message_hooks: Hooks,
    pub _atcommand_hooks: PyDict<Hooks>,
    pub _firmware_info_hooks: PyDict<Hooks>,

    pub _printer_action_hooks: Hooks,
    pub _gcodescript_hooks: Hooks,
    pub _serial_factory_hooks: Hooks,

    pub _temperature_hooks: Hooks,

    // SD status data
    pub _sdEnabled: Option<bool>,
    pub _sdAvailable: Attr<bool>,
    pub _sdFileList: Attr<bool>,
    /// `False` or a file name: `None` is `False` (difference 2)
    pub _sdFileLongName: Attr<Option<String>>,
    /// file name -> `SDFileData`
    pub _sdFiles: Attr<PyDict<SDFileData>>,
    pub _sdFilesAvailable: Event,
    pub _sdFileToSelect: Attr<Option<String>>,
    pub _sdFileToSelectUser: Attr<Option<String>>,
    pub _ignore_select: Attr<bool>,
    pub _manualStreaming: Attr<bool>,

    pub last_temperature: Attr<TemperatureRecord>,
    pub pause_temperature: Attr<TemperatureRecord>,
    pub cancel_temperature: Attr<TemperatureRecord>,

    pub last_position: Attr<PositionRecord>,
    pub pause_position: Attr<PositionRecord>,
    pub cancel_position: Attr<PositionRecord>,

    pub last_fanspeed: Attr<Option<i64>>,
    pub pause_fanspeed: Attr<Option<i64>>,
    pub cancel_fanspeed: Attr<Option<i64>>,

    pub _record_pause_data: Attr<bool>,
    pub _record_cancel_data: Attr<bool>,

    pub _suppress_scripts: BTreeSet<String>,
    pub _suppress_scripts_mutex: RLock,

    /// "cancel" / "pause" -> user
    pub _action_users: Attr<PyDict<Option<String>>>,
    pub _action_users_mutex: RLock,

    pub _pause_position_timer: Attr<Option<Arc<ResettableTimer>>>,
    pub _pause_mutex: RLock,
    pub _cancel_position_timer: Attr<Option<Arc<ResettableTimer>>>,
    pub _cancel_mutex: RLock,

    pub _log_position_on_pause: Option<bool>,
    pub _log_position_on_cancel: Option<bool>,
    pub _abort_heatup_on_cancel: Option<bool>,

    // serial encoding
    pub _serial_encoding: Option<Value>,

    // action commands
    pub _enable_shutdown_action_command: Option<bool>,

    // print job
    pub _currentFile: Attr<Option<Arc<dyn PrintingFileInformation>>>,
    pub _job_on_hold: CountedEvent,

    // multithreading locks
    pub _jobLock: RLock,
    pub _sendingLock: RLock,

    // monitoring thread
    pub _monitoring_active: Attr<bool>,
    pub monitoring_thread: Thread,

    // sending thread
    pub _send_queue_active: Attr<bool>,
    pub sending_thread: Thread,
}

/// `with self._job_on_hold`-style guard of `job_put_on_hold` (difference 7).
pub struct JobOnHold<'a> {
    comm: &'a Arc<MachineCom>,
}

impl Drop for JobOnHold<'_> {
    /// the `finally:` of comm.py:1181-1186
    fn drop(&mut self) {
        let c = self.comm;
        c._job_on_hold.clear(false);
        if c._job_on_hold.counter() == 0 {
            c._continue_sending();
        }
        c._job_on_hold
            .release()
            .expect("release of the job_on_hold lock");
    }
}

#[allow(non_snake_case)] // the Python method names are kept (`_gcode_M104_sent`)
impl MachineCom {
    pub const STATE_NONE: i64 = 0; // :445
    pub const STATE_OPEN_SERIAL: i64 = 1;
    pub const STATE_DETECT_SERIAL: i64 = 2;
    pub const STATE_CONNECTING: i64 = 3;
    pub const STATE_OPERATIONAL: i64 = 4;
    pub const STATE_STARTING: i64 = 5;
    pub const STATE_PRINTING: i64 = 6;
    pub const STATE_PAUSED: i64 = 7;
    pub const STATE_PAUSING: i64 = 8;
    pub const STATE_RESUMING: i64 = 9;
    pub const STATE_FINISHING: i64 = 10;
    pub const STATE_CLOSED: i64 = 11;
    pub const STATE_ERROR: i64 = 12;
    pub const STATE_CLOSED_WITH_ERROR: i64 = 13;
    pub const STATE_TRANSFERING_FILE: i64 = 14;
    pub const STATE_CANCELLING: i64 = 15; // :460

    /// `STATE_*` in the order they are defined (what `getStateId` finds through
    /// `self.__class__.__dict__`, comm.py:955)
    pub const STATE_NAMES: [(&'static str, i64); 16] = [
        ("STATE_NONE", Self::STATE_NONE),
        ("STATE_OPEN_SERIAL", Self::STATE_OPEN_SERIAL),
        ("STATE_DETECT_SERIAL", Self::STATE_DETECT_SERIAL),
        ("STATE_CONNECTING", Self::STATE_CONNECTING),
        ("STATE_OPERATIONAL", Self::STATE_OPERATIONAL),
        ("STATE_STARTING", Self::STATE_STARTING),
        ("STATE_PRINTING", Self::STATE_PRINTING),
        ("STATE_PAUSED", Self::STATE_PAUSED),
        ("STATE_PAUSING", Self::STATE_PAUSING),
        ("STATE_RESUMING", Self::STATE_RESUMING),
        ("STATE_FINISHING", Self::STATE_FINISHING),
        ("STATE_CLOSED", Self::STATE_CLOSED),
        ("STATE_ERROR", Self::STATE_ERROR),
        ("STATE_CLOSED_WITH_ERROR", Self::STATE_CLOSED_WITH_ERROR),
        ("STATE_TRANSFERING_FILE", Self::STATE_TRANSFERING_FILE),
        ("STATE_CANCELLING", Self::STATE_CANCELLING),
    ];

    /// :463 "be sure to add anything here that signifies an operational state"
    pub const OPERATIONAL_STATES: [i64; 9] = [
        Self::STATE_PRINTING,
        Self::STATE_STARTING,
        Self::STATE_OPERATIONAL,
        Self::STATE_PAUSED,
        Self::STATE_CANCELLING,
        Self::STATE_PAUSING,
        Self::STATE_RESUMING,
        Self::STATE_FINISHING,
        Self::STATE_TRANSFERING_FILE,
    ];

    /// :476 "be sure to add anything here that signifies a printing state"
    pub const PRINTING_STATES: [i64; 6] = [
        Self::STATE_STARTING,
        Self::STATE_PRINTING,
        Self::STATE_CANCELLING,
        Self::STATE_PAUSING,
        Self::STATE_RESUMING,
        Self::STATE_FINISHING,
    ];

    pub const CAPABILITY_AUTOREPORT_TEMP: &'static str = "AUTOREPORT_TEMP"; // :486
    pub const CAPABILITY_AUTOREPORT_SD_STATUS: &'static str = "AUTOREPORT_SD_STATUS";
    pub const CAPABILITY_AUTOREPORT_POS: &'static str = "AUTOREPORT_POS";
    pub const CAPABILITY_BUSY_PROTOCOL: &'static str = "BUSY_PROTOCOL";
    pub const CAPABILITY_EMERGENCY_PARSER: &'static str = "EMERGENCY_PARSER";
    pub const CAPABILITY_CHAMBER_TEMP: &'static str = "CHAMBER_TEMPERATURE";
    pub const CAPABILITY_EXTENDED_M20: &'static str = "EXTENDED_M20";
    pub const CAPABILITY_LFN_WRITE: &'static str = "LFN_WRITE";

    pub const CAPABILITY_SUPPORT_ENABLED: &'static str = "enabled";
    pub const CAPABILITY_SUPPORT_DETECTED: &'static str = "detected";
    pub const CAPABILITY_SUPPORT_DISABLED: &'static str = "disabled";

    pub const DETECTION_RETRIES: i64 = 3; // :499

    /// comm.py:500 `__init__(self, port=None, baudrate=None, callbackObject=None,
    /// printerProfileManager=None)`. The threads are created, not started (`start()`).
    pub fn new(
        port: Option<String>,
        baudrate: Option<i64>,
        callback_object: Option<Arc<dyn MachineComPrintCallback>>,
        printer_profile_manager: Option<Arc<PrinterProfileManager>>,
    ) -> Arc<MachineCom> {
        // self._logger = logging.getLogger(__name__)
        // self._serialLogger = logging.getLogger("SERIAL")
        // self._phaseLogger = logging.getLogger(__name__ + ".command_phases")

        let port = match port {
            None => settings()
                .get(&["serial", "port"])
                .and_then(|v| v.as_str().map(str::to_string)),
            some => some,
        };
        let baudrate = match baudrate {
            None => {
                let settings_baudrate = settings().get_int(&["serial", "baudrate"]);
                match settings_baudrate {
                    None => 0,
                    Some(b) => b,
                }
            }
            Some(b) => b,
        };
        let callback_object: Arc<dyn MachineComPrintCallback> = match callback_object {
            None => Arc::new(DefaultMachineComPrintCallback),
            Some(c) => c,
        };

        let mut timeout_intervals = PyDict::new();
        let kw = GetKwargs {
            merged: true,
            asdict: true,
            ..GetKwargs::default()
        };
        for (key, value) in settings()
            .get_with(&["serial", "timeout"], &kw)
            .expect("no error_on_path")
            .expect("AttributeError: 'NoneType' object has no attribute 'items'")
            .as_object()
            .expect("serial.timeout is a dict")
        {
            if let Ok(v) = py_float_value(value) {
                timeout_intervals.insert(key, v);
            }
        }

        let mut consecutive_timeout_maximums = PyDict::new();
        for (key, value) in settings()
            .get_with(&["serial", "maxCommunicationTimeouts"], &kw)
            .expect("no error_on_path")
            .expect("AttributeError: 'NoneType' object has no attribute 'items'")
            .as_object()
            .expect("serial.maxCommunicationTimeouts is a dict")
        {
            if let Ok(v) = py_int_value(value) {
                consecutive_timeout_maximums.insert(key, v);
            }
        }

        let cap = |name: &str| settings().get_boolean(&["serial", "capabilities", name]);
        let mut capability_support = PyDict::new();
        capability_support.insert(Self::CAPABILITY_AUTOREPORT_TEMP, cap("autoreport_temp"));
        capability_support.insert(
            Self::CAPABILITY_AUTOREPORT_SD_STATUS,
            cap("autoreport_sdstatus"),
        );
        capability_support.insert(Self::CAPABILITY_AUTOREPORT_POS, cap("autoreport_pos"));
        capability_support.insert(Self::CAPABILITY_BUSY_PROTOCOL, cap("busy_protocol"));
        capability_support.insert(Self::CAPABILITY_EMERGENCY_PARSER, cap("emergency_parser"));
        capability_support.insert(Self::CAPABILITY_CHAMBER_TEMP, cap("chamber_temp"));
        capability_support.insert(Self::CAPABILITY_EXTENDED_M20, cap("extended_m20"));
        capability_support.insert(Self::CAPABILITY_LFN_WRITE, cap("lfn_write"));

        // deque([], maxlen): None is unbounded, a negative maxlen is a ValueError (difference 3)
        let deque_maxlen = |n: Option<i64>| {
            n.map(|n| usize::try_from(n).expect("ValueError: maxlen must be non-negative"))
        };
        let last_line_count = settings().get_int(&["serial", "lastLineBufferSize"]);
        let terminal_log_size = settings()
            .get_int(&["serial", "terminalLogSize"])
            .expect("TypeError: '<' not supported between instances of 'int' and 'NoneType'");

        let ack_max = settings().get_int(&["serial", "ackMax"]);
        let pm = plugin_manager();
        let hooks = |name: &str| pm.get_hooks(name);
        let group = |pairs: &[(&str, &str)]| {
            let mut d = PyDict::new();
            for (k, h) in pairs {
                d.insert(k, hooks(h));
            }
            d
        };

        let comm = MachineCom {
            _port: port,
            _baudrate: baudrate,
            _callback: callback_object,
            _printerProfileManager: printer_profile_manager,
            _state: Attr::new(Self::STATE_NONE),
            _serial: Attr::new(None),

            _detection_candidates: Attr::new(Vec::new()),
            _detection_retry: Attr::new(Self::DETECTION_RETRIES),

            _temperatureTargetSetThreshold: 25,
            _tempOffsets: Attr::new(PyDict::new()),
            _command_queue: CommandQueue::new(0),
            _currentZ: Attr::new(None),
            _currentF: Attr::new(None),
            _heatupWaitStartTime: Attr::new(None),
            _heatupWaitTimeLost: Attr::new(0.0),
            _pauseWaitStartTime: Attr::new(None),
            _pauseWaitTimeLost: Attr::new(0.0),
            _currentTool: Attr::new(0),
            _toolBeforeChange: Attr::new(None),
            _toolBeforeHeatup: Attr::new(None),

            _known_invalid_tools: Attr::new(BTreeSet::new()),
            _known_invalid_custom_temps: Attr::new(BTreeSet::new()),
            _known_broken_temperature_hooks: Attr::new(BTreeSet::new()),

            _long_running_command: Attr::new(false),
            _heating: Attr::new(false),
            _dwelling_until: Attr::new(None),
            _connection_closing: Attr::new(false),

            _timeout: Attr::new(None),
            _ok_timeout: Attr::new(None),
            _timeout_intervals: Attr::new(timeout_intervals),

            _consecutive_timeouts: Attr::new(0),
            _consecutive_timeout_maximums: Attr::new(consecutive_timeout_maximums),

            _max_write_passes: settings().get_int(&["serial", "maxWritePasses"]),

            _hello_command: settings().get(&["serial", "helloCommand"]),
            _hello_sent: Attr::new(0),
            _trigger_ok_for_m29: settings().get_boolean(&["serial", "triggerOkForM29"]),

            _alwaysSendChecksum: Attr::new(
                settings().get_boolean(&["serial", "alwaysSendChecksum"]),
            ),
            _neverSendChecksum: settings().get_boolean(&["serial", "neverSendChecksum"]),
            _sendChecksumWithUnknownCommands: Attr::new(
                settings().get_boolean(&["serial", "sendChecksumWithUnknownCommands"]),
            ),
            _unknownCommandsNeedAck: Attr::new(
                settings().get_boolean(&["serial", "unknownCommandsNeedAck"]),
            ),
            _sdAlwaysAvailable: Attr::new(settings().get_boolean(&["serial", "sdAlwaysAvailable"])),
            _sdRelativePath: Attr::new(settings().get_boolean(&["serial", "sdRelativePath"])),
            _sdLowerCase: Attr::new(settings().get_boolean(&["serial", "sdLowerCase"])),
            _sdCancelCommand: settings().get(&["serial", "sdCancelCommand"]),
            _blockWhileDwelling: Attr::new(
                settings().get_boolean(&["serial", "blockWhileDwelling"]),
            ),
            _send_m112_on_error: settings().get_boolean(&["serial", "sendM112OnError"]),
            _disable_sd_printing_detection: Attr::new(
                settings().get_boolean(&["serial", "disableSdPrintingDetection"]),
            ),
            _current_line: Attr::new(1),
            _line_mutex: RLock::new(),
            _resendDelta: Attr::new(None),

            _capability_support: Attr::new(capability_support),

            _lastLines: Attr::new(PyDeque::new(deque_maxlen(last_line_count))),
            _lastCommError: Attr::new(None),
            _lastResendNumber: Attr::new(None),
            _currentResendCount: Attr::new(0),

            _currentConsecutiveResendNumber: Attr::new(None),
            _currentConsecutiveResendCount: Attr::new(0),
            _maxConsecutiveResends: settings().get_int(&["serial", "maxConsecutiveResends"]),

            _errorValue: Attr::new(String::new()),

            _firmware_detection: settings().get_boolean(&["serial", "firmwareDetection"]),
            _firmware_info_received: Attr::new(false),
            _firmware_capabilities_received: Attr::new(false),
            _firmware_info: Attr::new(PyDict::new()),
            _firmware_name: Attr::new(None),
            _firmware_capabilities: Attr::new(PyDict::new()),

            _defer_sd_refresh: settings().get_boolean(&["serial", "waitToLoadSdFileList"]),

            _temperature_autoreporting: Attr::new(false),
            _sdstatus_autoreporting: Attr::new(false),
            _pos_autoreporting: Attr::new(false),
            _busy_protocol_detected: Attr::new(false),
            _busy_protocol_support: Attr::new(false),

            _trigger_ok_after_resend: settings().get(&["serial", "supportResendsWithoutOk"]),
            _resend_ok_timer: Attr::new(None),

            _resendActive: Attr::new(false),

            _terminal_log: Attr::new(PyDeque::new(Some(std::cmp::min(
                20,
                usize::try_from(terminal_log_size)
                    .expect("ValueError: maxlen must be non-negative"),
            )))),

            _disconnect_on_errors: settings().get_boolean(&["serial", "disconnectOnErrors"]),
            _ignore_errors: settings().get_boolean(&["serial", "ignoreErrorsFromFirmware"]),

            _log_resends: settings().get_boolean(&["serial", "logResends"]),

            // don't log more resends than 5 / 60s
            _log_resends_rate_start: Attr::new(None),
            _log_resends_rate_count: Attr::new(0),
            _log_resends_max: 5,
            _log_resends_rate_frame: 60,

            _long_running_commands: settings().get(&["serial", "longRunningCommands"]),
            _checksum_requiring_commands: settings().get(&["serial", "checksumRequiringCommands"]),
            _blocked_commands: settings().get(&["serial", "blockedCommands"]),
            _ignored_commands: settings().get(&["serial", "ignoredCommands"]),
            _pausing_commands: settings().get(&["serial", "pausingCommands"]),
            _emergency_commands: settings().get(&["serial", "emergencyCommands"]),
            _sanity_check_tools: settings().get_boolean(&["serial", "sanityCheckTools"]),

            _ack_max: ack_max,
            // CountedEvent(name="comm.clear_to_send", minimum=None, maximum=self._ack_max):
            // `name` is swallowed by **kwargs (util/__init__.py:1420); `value` is the default 0.
            _clear_to_send: CountedEvent::new(0, None, ack_max, None),
            _send_queue: SendQueue::new(0),
            _temperature_timer: Attr::new(None),
            _sd_status_timer: Attr::new(None),

            _consecutive_not_sd_printing: Attr::new(0),
            _consecutive_not_sd_printing_maximum: settings()
                .get_int(&["serial", "maxNotSdPrinting"]),

            _job_queue: JobQueue::new(0),

            _transmitted_lines: Attr::new(0),
            _received_resend_requests: Attr::new(0),
            _resend_ratio_start: settings().get_int(&["serial", "resendRatioStart"]),
            _resend_ratio_threshold: settings()
                .get_int(&["serial", "resendRatioThreshold"])
                .expect("TypeError: unsupported operand type(s) for /: 'NoneType' and 'int'")
                as f64
                / 100.0,
            _resend_ratio_reported: Attr::new(false),

            // hooks
            _pluginManager: pm,

            _gcode_hooks: group(&[
                ("queuing", "octoprint.comm.protocol.gcode.queuing"),
                ("queued", "octoprint.comm.protocol.gcode.queued"),
                ("sending", "octoprint.comm.protocol.gcode.sending"),
                ("sent", "octoprint.comm.protocol.gcode.sent"),
            ]),
            _received_message_hooks: hooks("octoprint.comm.protocol.gcode.received"),
            _error_message_hooks: hooks("octoprint.comm.protocol.gcode.error"),
            _atcommand_hooks: group(&[
                ("queuing", "octoprint.comm.protocol.atcommand.queuing"),
                ("sending", "octoprint.comm.protocol.atcommand.sending"),
            ]),
            _firmware_info_hooks: group(&[
                ("info", "octoprint.comm.protocol.firmware.info"),
                (
                    "capabilities",
                    "octoprint.comm.protocol.firmware.capabilities",
                ),
                (
                    "capability_report",
                    "octoprint.comm.protocol.firmware.capability_report",
                ),
            ]),

            _printer_action_hooks: hooks("octoprint.comm.protocol.action"),
            _gcodescript_hooks: hooks("octoprint.comm.protocol.scripts"),
            _serial_factory_hooks: hooks("octoprint.comm.transport.serial.factory"),

            _temperature_hooks: hooks("octoprint.comm.protocol.temperatures.received"),

            // SD status data
            _sdEnabled: settings().get_boolean(&["feature", "sdSupport"]),
            _sdAvailable: Attr::new(false),
            _sdFileList: Attr::new(false),
            _sdFileLongName: Attr::new(None),
            _sdFiles: Attr::new(PyDict::new()),
            _sdFilesAvailable: Event::new(),
            _sdFileToSelect: Attr::new(None),
            _sdFileToSelectUser: Attr::new(None),
            _ignore_select: Attr::new(false),
            _manualStreaming: Attr::new(false),

            last_temperature: Attr::new(TemperatureRecord::new()),
            pause_temperature: Attr::new(TemperatureRecord::new()),
            cancel_temperature: Attr::new(TemperatureRecord::new()),

            last_position: Attr::new(PositionRecord::new(&[])),
            pause_position: Attr::new(PositionRecord::new(&[])),
            cancel_position: Attr::new(PositionRecord::new(&[])),

            last_fanspeed: Attr::new(None),
            pause_fanspeed: Attr::new(None),
            cancel_fanspeed: Attr::new(None),

            _record_pause_data: Attr::new(false),
            _record_cancel_data: Attr::new(false),

            _suppress_scripts: BTreeSet::new(),
            _suppress_scripts_mutex: RLock::new(),

            _action_users: Attr::new(PyDict::new()),
            _action_users_mutex: RLock::new(),

            _pause_position_timer: Attr::new(None),
            _pause_mutex: RLock::new(),
            _cancel_position_timer: Attr::new(None),
            _cancel_mutex: RLock::new(),

            _log_position_on_pause: settings().get_boolean(&["serial", "logPositionOnPause"]),
            _log_position_on_cancel: settings().get_boolean(&["serial", "logPositionOnCancel"]),
            _abort_heatup_on_cancel: settings().get_boolean(&["serial", "abortHeatupOnCancel"]),

            // serial encoding
            _serial_encoding: settings().get(&["serial", "encoding"]),

            // action commands
            _enable_shutdown_action_command: settings()
                .get_boolean(&["serial", "enableShutdownActionCommand"]),

            // print job
            _currentFile: Attr::new(None),
            _job_on_hold: CountedEvent::new(0, Some(0), None, None),

            // multithreading locks
            _jobLock: RLock::new(),
            _sendingLock: RLock::new(),

            // monitoring thread
            _monitoring_active: Attr::new(true),
            monitoring_thread: Thread::new(true), // name="comm._monitor" (difference 4)

            // sending thread
            _send_queue_active: Attr::new(true),
            sending_thread: Thread::new(true), // name="comm.sending_thread" (difference 4)
        };
        Arc::new(comm)
    }

    /// comm.py:847 `start()`: starts the two threads (`_monitor`, `_send_loop`).
    pub fn start(self: &Arc<Self>) {
        // doing this here instead of __init__ combats a race condition where
        // self._comm in the printer interface is still None on first pushs from
        // the comm layer during detection
        let me = self.clone();
        self.monitoring_thread
            .start(move || me._monitor())
            .expect("RuntimeError: threads can only be started once");
        let me = self.clone();
        self.sending_thread
            .start(move || me._send_loop())
            .expect("RuntimeError: threads can only be started once");
    }

    // `__del__` (comm.py:853) is `impl Drop for MachineCom` below (difference 6).

    /// comm.py:857 `_active` (property)
    pub fn _active(&self) -> bool {
        self._monitoring_active.get() && self._send_queue_active.get()
    }

    /// comm.py:861 `_capability_supported(cap)`
    pub fn _capability_supported(&self, cap: &str) -> bool {
        // `self._capability_support.get(cap, False) and self._firmware_capabilities.get(cap, False)`
        truthy(
            self._capability_support
                .with(|d| d.get(cap).copied())
                .unwrap_or(Some(false)),
        ) && self
            ._firmware_capabilities
            .with(|d| d.get(cap).copied())
            .unwrap_or(false)
    }

    /// comm.py:866 `received_resends` (property)
    pub fn received_resends(&self) -> i64 {
        self._received_resend_requests.get()
    }

    /// comm.py:870 `transmitted_lines` (property)
    pub fn transmitted_lines(&self) -> i64 {
        self._transmitted_lines.get()
    }

    /// comm.py:874 `resend_ratio` (property). Python reads `_transmitted_lines` for the test and
    /// again for the division, and `_received_resend_requests` once; so do we.
    pub fn resend_ratio(&self) -> f64 {
        if self._transmitted_lines.get() != 0 {
            self._received_resend_requests.get() as f64 / self._transmitted_lines.get() as f64
        } else {
            0.0
        }
    }

    /// comm.py:881 `_reevaluate_resend_ratio`
    pub fn _reevaluate_resend_ratio(&self) {
        let resend_ratio = self.resend_ratio();
        if resend_ratio >= self._resend_ratio_threshold
            && self._transmitted_lines.get()
                > self._resend_ratio_start.expect(
                    "TypeError: '>' not supported between instances of 'int' and 'NoneType'",
                )
            && !self._resend_ratio_reported.get()
        {
            let message = format!(
                "Over {:.0}% of transmitted lines have triggered resend requests \
                 ({:.2}%). The communication with the printer is unreliable. \
                 Please see https://faq.octoprint.org/communication-errors.",
                self._resend_ratio_threshold * 100.0,
                resend_ratio * 100.0
            );
            self._log(&message);
            // self._logger.warning(message)
            self._resend_ratio_reported.set(true);
        }
    }

    // ~~ internal state management

    /// comm.py:900 `_changeState(newState)`
    pub fn _change_state(&self, new_state: i64) {
        if self._state.get() == new_state {
            return;
        }

        if new_state == Self::STATE_CLOSED || new_state == Self::STATE_CLOSED_WITH_ERROR {
            if truthy(settings().get_boolean(&["feature", "sdSupport"])) {
                self._sdFileList.set(false);
                self._sdFiles.set(PyDict::new());
                self._callback.on_comm_sd_files(&[]);
            }

            if self._currentFile.get().is_some() {
                if self.is_busy() {
                    self._record_file_position();
                }
                self._currentFile
                    .get()
                    .expect("AttributeError: 'NoneType' object has no attribute 'close'")
                    .close();
            }
        }

        let old_state = self.get_state_string(None);
        self._state.set(new_state);

        let text = format!(
            "Changing monitoring state from \"{}\" to \"{}\"",
            old_state,
            self.get_state_string(None)
        );
        self._log(&text);
        // self._logger.info(text)
        self._callback.on_comm_state_change(new_state);
    }

    /// comm.py:925 `_dual_log(message, level=logging.ERROR, prefix="")`
    pub fn _dual_log(&self, message: &str, level: i64, prefix: &str) {
        // self._logger.log(level, message)
        let _ = level;
        self._log(&format!("{prefix}{message}"));
    }

    /// comm.py:929 `_log(message)`
    pub fn _log(&self, message: &str) {
        // message = to_unicode(message)  (difference 14)
        self._terminal_log
            .with_mut(|l| l.append(message.to_string()));
        self._callback.on_comm_log(message);
        // self._serialLogger.debug(message)
    }

    /// comm.py:936 `_to_logfile_with_terminal(message=None, level=logging.INFO)`: only logs
    /// (`self._logger.log(level, log)`), which is not ported; the text is built as in Python.
    pub fn _to_logfile_with_terminal(&self, message: Option<&str>, level: i64) {
        let mut log = format!(
            "Last lines in terminal:\n{}",
            self._terminal_log.with(|l| l
                .iter()
                .map(|x| format!("| {x}"))
                .collect::<Vec<_>>()
                .join("\n"))
        );
        if let Some(message) = message {
            log = format!("{message}\n| {log}");
        }
        // self._logger.log(level, log)
        let _ = (log, level);
    }

    /// comm.py:944 `_addToLastLines(cmd)`
    pub fn _add_to_last_lines(&self, cmd: Vec<u8>) {
        self._lastLines.with_mut(|l| l.append(cmd));
    }

    // ~~ getters

    /// comm.py:948 `getState`
    pub fn get_state(&self) -> i64 {
        self._state.get()
    }

    /// comm.py:951 `getStateId(state=None)`
    pub fn get_state_id(&self, state: Option<i64>) -> String {
        let state = state.unwrap_or_else(|| self._state.get());
        for (name, value) in Self::STATE_NAMES {
            if value == state {
                return name["STATE_".len()..].to_string();
            }
        }
        "UNKNOWN".to_string()
    }

    /// comm.py:962 `getStateString(state=None)`
    pub fn get_state_string(&self, state: Option<i64>) -> String {
        let state = state.unwrap_or_else(|| self._state.get());

        let s = match state {
            Self::STATE_NONE => "Offline",
            Self::STATE_OPEN_SERIAL => "Opening serial connection",
            Self::STATE_DETECT_SERIAL => "Detecting serial connection",
            Self::STATE_CONNECTING => "Connecting",
            Self::STATE_OPERATIONAL => "Operational",
            Self::STATE_STARTING => {
                if self.is_sd_file_selected() {
                    "Starting print from SD"
                } else if self.is_streaming() {
                    "Starting to send file to SD"
                } else {
                    "Starting"
                }
            }
            Self::STATE_PRINTING => {
                if self.is_sd_file_selected() {
                    "Printing from SD"
                } else if self.is_streaming() {
                    "Sending file to SD"
                } else {
                    "Printing"
                }
            }
            Self::STATE_CANCELLING => "Cancelling",
            Self::STATE_PAUSING => "Pausing",
            Self::STATE_PAUSED => "Paused",
            Self::STATE_RESUMING => "Resuming",
            Self::STATE_FINISHING => "Finishing",
            Self::STATE_CLOSED => "Offline",
            Self::STATE_ERROR => "Error",
            Self::STATE_CLOSED_WITH_ERROR => "Offline after error",
            Self::STATE_TRANSFERING_FILE => "Transferring file to SD",
            _ => return format!("Unknown State ({})", self._state.get()),
        };
        s.to_string()
    }

    /// comm.py:1016 `getErrorString`
    pub fn get_error_string(&self) -> String {
        self._errorValue.get()
    }

    /// comm.py:1019 `isClosedOrError`
    pub fn is_closed_or_error(&self) -> bool {
        [
            Self::STATE_ERROR,
            Self::STATE_CLOSED,
            Self::STATE_CLOSED_WITH_ERROR,
        ]
        .contains(&self._state.get())
    }

    /// comm.py:1026 `isError`
    pub fn is_error(&self) -> bool {
        [Self::STATE_ERROR, Self::STATE_CLOSED_WITH_ERROR].contains(&self._state.get())
    }

    /// comm.py:1029 `isOperational`
    pub fn is_operational(&self) -> bool {
        Self::OPERATIONAL_STATES.contains(&self._state.get())
    }

    /// comm.py:1032 `isPrinting`
    pub fn is_printing(&self) -> bool {
        Self::PRINTING_STATES.contains(&self._state.get())
    }

    /// comm.py:1035 `isCancelling`
    pub fn is_cancelling(&self) -> bool {
        self._state.get() == Self::STATE_CANCELLING
    }

    /// comm.py:1038 `isPausing`
    pub fn is_pausing(&self) -> bool {
        self._state.get() == Self::STATE_PAUSING
    }

    /// comm.py:1041 `isResuming`
    pub fn is_resuming(&self) -> bool {
        self._state.get() == Self::STATE_RESUMING
    }

    /// comm.py:1044 `isStarting`
    pub fn is_starting(&self) -> bool {
        self._state.get() == Self::STATE_STARTING
    }

    /// comm.py:1047 `isFinishing`
    pub fn is_finishing(&self) -> bool {
        self._state.get() == Self::STATE_FINISHING
    }

    /// comm.py:1050 `isSdPrinting`
    pub fn is_sd_printing(&self) -> bool {
        self.is_sd_file_selected() && self.is_printing()
    }

    /// comm.py:1053 `isSdFileSelected` (`self._currentFile` is read twice, as in Python)
    pub fn is_sd_file_selected(&self) -> bool {
        self._currentFile.get().is_some()
            && self
                ._currentFile
                .get()
                .is_some_and(|f| f.is_printing_sd_file_information())
    }

    /// comm.py:1058 `isStreaming`
    pub fn is_streaming(&self) -> bool {
        self._currentFile.get().is_some()
            && self
                ._currentFile
                .get()
                .is_some_and(|f| f.is_streaming_gcode_file_information())
            && !self
                ._currentFile
                .get()
                .expect("AttributeError: 'NoneType' object has no attribute 'done'")
                .done()
    }

    /// comm.py:1065 `isPaused`
    pub fn is_paused(&self) -> bool {
        self._state.get() == Self::STATE_PAUSED
    }

    /// comm.py:1068 `isBusy`
    pub fn is_busy(&self) -> bool {
        self.is_printing()
            || self.is_paused()
            || [Self::STATE_CANCELLING, Self::STATE_PAUSING].contains(&self._state.get())
    }

    /// comm.py:1075 `isSdReady`
    pub fn is_sd_ready(&self) -> bool {
        self._sdAvailable.get()
    }

    /// comm.py:1078 `getPrintProgress`
    pub fn get_print_progress(&self) -> Option<f64> {
        if self._currentFile.get().is_none() {
            return None;
        }
        Some(
            self._currentFile
                .get()
                .expect("AttributeError: 'NoneType' object has no attribute 'getProgress'")
                .get_progress(),
        )
    }

    /// comm.py:1083 `getPrintFilepos`
    pub fn get_print_filepos(&self) -> Option<i64> {
        if self._currentFile.get().is_none() {
            return None;
        }
        Some(
            self._currentFile
                .get()
                .expect("AttributeError: 'NoneType' object has no attribute 'getFilepos'")
                .get_filepos(),
        )
    }

    /// comm.py:1088 `getPrintTime`
    pub fn get_print_time(&self) -> Option<f64> {
        if self._currentFile.get().is_none()
            || self
                ._currentFile
                .get()
                .expect("AttributeError: 'NoneType' object has no attribute 'getStartTime'")
                .get_start_time()
                .is_none()
        {
            None
        } else {
            Some(
                time_monotonic()
                    - self
                        ._currentFile
                        .get()
                        .expect("AttributeError: 'NoneType' object has no attribute 'getStartTime'")
                        .get_start_time()
                        .expect(
                            "TypeError: unsupported operand type(s) for -: 'float' and 'NoneType'",
                        )
                    - self._pauseWaitTimeLost.get(),
            )
        }
    }

    /// comm.py:1098 `getCleanedPrintTime`
    pub fn get_cleaned_print_time(&self) -> Option<f64> {
        let print_time = self.get_print_time()?;

        let mut cleaned_print_time = print_time - self._heatupWaitTimeLost.get();
        if cleaned_print_time < 0.0 {
            cleaned_print_time = 0.0;
        }
        Some(cleaned_print_time)
    }

    /// comm.py:1107 `getTemp` (`self.last_temperature.tools`)
    pub fn get_temp(&self) -> Vec<(i64, TempTuple)> {
        self.last_temperature.with(|t| t.tools())
    }

    /// comm.py:1110 `getBedTemp`
    pub fn get_bed_temp(&self) -> TempTuple {
        self.last_temperature.with(|t| t.bed())
    }

    /// comm.py:1113 `getOffsets` (`dict(self._tempOffsets)`)
    pub fn get_offsets(&self) -> PyDict<f64> {
        self._tempOffsets.get()
    }

    /// comm.py:1116 `getCurrentTool`
    pub fn get_current_tool(&self) -> i64 {
        self._currentTool.get()
    }

    /// comm.py:1119 `getConnection`: `(port, baudrate)`
    pub fn get_connection(&self) -> (Option<String>, Option<i64>) {
        let mut port = self._port.clone();
        let mut baudrate = Some(self._baudrate);

        if let Some(serial) = self._serial.get() {
            if let Some(p) = serial.port() {
                port = p;
            }
            if let Some(b) = serial.baudrate() {
                baudrate = b;
            }
        }

        (port, baudrate)
    }

    /// comm.py:1130 `getTransport`
    pub fn get_transport(&self) -> Option<Arc<dyn SerialTransport>> {
        self._serial.get()
    }

    // ~~ external interface

    /// comm.py:1175 `job_put_on_hold(blocking=True)` (a context manager, difference 7): `Err` is
    /// `RuntimeError("Could not acquire job_on_hold lock")`.
    pub fn job_put_on_hold(
        self: &Arc<Self>,
        blocking: bool,
    ) -> Result<JobOnHold<'_>, RuntimeError> {
        if !self._job_on_hold.acquire(blocking) {
            return Err(RuntimeError("Could not acquire job_on_hold lock"));
        }

        self._job_on_hold.set();
        Ok(JobOnHold { comm: self })
    }

    /// comm.py:1190 `job_on_hold` (property)
    pub fn job_on_hold(&self) -> bool {
        self._job_on_hold.counter() > 0
    }

    /// comm.py:1194 `set_job_on_hold(value, blocking=True)`
    pub fn set_job_on_hold(self: &Arc<Self>, value: bool, blocking: bool) -> bool {
        let mut trigger = false;

        // don't run any locking code beyond this...
        if !self._job_on_hold.acquire(blocking) {
            return false;
        }

        // try: ... finally: release (a guard for the panic path as well)
        struct Release<'a>(&'a CountedEvent);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                self.0.release().expect("release of the job_on_hold lock");
            }
        }
        {
            let _release = Release(&self._job_on_hold);
            if value {
                self._job_on_hold.set();
            } else {
                self._job_on_hold.clear(false);
                if self._job_on_hold.counter() == 0 {
                    trigger = true;
                }
            }
        }

        // locking code is now safe to run again
        if trigger {
            self._continue_sending();
        }

        true
    }

    /// comm.py:1215 `close(is_error=False, wait=True, timeout=10.0, *args, **kwargs)`
    pub fn close(self: &Arc<Self>, is_error: bool, wait: bool, timeout: Option<f64>) {
        self.close_with_legacy(is_error, wait, timeout, None)
    }

    /// comm.py:1215 `close(...)` with `kwargs.get("isError")` as `legacy_is_error` (difference 8).
    ///
    /// Closes the connection to the printer. If `is_error` is False, will attempt to send the
    /// `beforePrinterDisconnected` gcode script. If `is_error` is False and `wait` is True, will
    /// wait until all messages in the send queue (including that script) have been sent.
    pub fn close_with_legacy(
        self: &Arc<Self>,
        is_error: bool,
        wait: bool,
        timeout: Option<f64>,
        legacy_is_error: Option<bool>,
    ) {
        self.close_impl(Some(self), is_error, wait, timeout, legacy_is_error)
    }

    /// The body of `close` (comm.py:1215). OURS (difference 32): `me` is the `Arc` of this object, which
    /// `sendGcodeScript` needs (the phase handlers hold it); `__del__` (`Drop`) has none.
    fn close_impl(
        &self,
        me: Option<&Arc<Self>>,
        is_error: bool,
        wait: bool,
        timeout: Option<f64>,
        legacy_is_error: Option<bool>,
    ) {
        // legacy parameters
        let mut is_error = legacy_is_error.unwrap_or(is_error);

        if self._connection_closing.get() {
            return;
        }
        self._connection_closing.set(true);

        // try: self._temperature_timer.cancel() except Exception: pass
        // (a None set by another thread in between is an AttributeError, caught as well)
        if let Some(t) = self._temperature_timer.get() {
            t.cancel();
        }

        if let Some(t) = self._sd_status_timer.get() {
            t.cancel();
        }

        let deactivate_monitoring_and_send_queue = || {
            self._monitoring_active.set(false);
            self._send_queue_active.set(false);
            self._clear_to_send.acquire(true);
            self._send_queue
                .put(
                    SendQueueEntry::Marker(SendQueueMarker::new(None)),
                    None,
                    None,
                    true,
                    None,
                )
                .unwrap_or_else(|_| panic!("put of a SendQueueMarker on an unbounded queue"));
        };

        if self._serial.get().is_some() {
            if !is_error && Self::OPERATIONAL_STATES.contains(&self._state.get()) {
                match me {
                    Some(me) => {
                        me.send_gcode_script("beforePrinterDisconnected", None, None, false);
                    }
                    None => {
                        // `__del__` (difference 32): an empty script sends nothing; a script with lines
                        // would need the `Arc`, which a value that is being dropped does not have
                        if !self
                            ._get_gcode_script("beforePrinterDisconnected", None)
                            .is_empty()
                        {
                            unimplemented!(
                                "comm.py:853 __del__ -> close(): sending the beforePrinterDisconnected script while the MachineCom is dropped"
                            );
                        }
                    }
                }
                if wait {
                    if let Some(timeout) = timeout {
                        let stop = time_monotonic() + timeout;
                        while (self._command_queue.lock().unfinished_tasks != 0
                            || self._send_queue.lock().unfinished_tasks != 0)
                            && time_monotonic() < stop
                        {
                            time_sleep(0.1);
                        }
                    } else {
                        self._command_queue.join();
                        self._send_queue.join();
                    }
                }
            }

            deactivate_monitoring_and_send_queue();

            // try: if hasattr(self._serial, "cancel_read") and callable(...): self._serial.cancel_read()
            // except Exception: self._logger.exception("Error while cancelling pending reads from the serial port")
            if let Some(serial) = self._serial.get() {
                if let Some(Err(_e)) = serial.cancel_read() {
                    // self._logger.exception(...)
                }
            }

            if let Some(serial) = self._serial.get() {
                if let Some(Err(_e)) = serial.cancel_write() {
                    // self._logger.exception("Error while cancelling pending writes to the serial port")
                }
            }

            // try: self._serial.close() except Exception: ...; is_error = True
            match self._serial.get() {
                Some(serial) => {
                    if serial.close().is_err() {
                        // self._logger.exception("Error while trying to close serial port")
                        is_error = true;
                    }
                }
                None => {
                    // AttributeError on None, caught by `except Exception`
                    is_error = true;
                }
            }
        } else {
            deactivate_monitoring_and_send_queue();
        }

        self._serial.set(None);

        // if we are printing, this will also make sure of firing PRINT_FAILED
        if is_error {
            self._change_state(Self::STATE_CLOSED_WITH_ERROR);
        } else {
            self._change_state(Self::STATE_CLOSED);
        }

        if truthy(settings().get_boolean(&["feature", "sdSupport"])) {
            self._sdFiles.set(PyDict::new());
        }
    }

    // ~~ external interface, continued (comm.py:1273-2259)

    /// comm.py:1273 `setTemperatureOffset(offsets)`
    pub fn set_temperature_offset(&self, offsets: impl IntoIterator<Item = (String, f64)>) {
        self._tempOffsets.with_mut(|d| d.update(offsets));
    }

    /// comm.py:1276 `fakeOk()`
    pub fn fake_ok(self: &Arc<Self>) {
        self._handle_ok();
    }

    /// comm.py:1279 `sendCommand(cmd, cmd_type=None, part_of_job=False, processed=False,
    /// force=False, on_sent=None, tags=None)`
    ///
    /// Returns Python's `True`/`False`, or `None` (nothing was done: not operational, not
    /// forced).
    pub fn send_command(
        self: &Arc<Self>,
        cmd: QueuedCommand,
        cmd_type: Option<&str>,
        part_of_job: bool,
        processed: bool,
        force: bool,
        on_sent: Option<OnSent>,
        tags: Option<BTreeSet<String>>,
    ) -> Option<bool> {
        let mut force = force;
        let cmd = match cmd {
            QueuedCommand::Marker(marker) => QueuedCommand::Marker(marker),
            QueuedCommand::Command(cmd) => {
                let mut cmd =
                    to_unicode(&PyText::Str(cmd), "utf-8", "replace").expect("errors=\"replace\"");
                if !processed {
                    match process_gcode_line(&cmd, None, None) {
                        None => return Some(false),
                        Some(line) => cmd = line,
                    }
                }

                let gcode = gcode_command_for_cmd(Some(&cmd));
                force = force || py_in_list(&self._emergency_commands, gcode.as_deref());
                QueuedCommand::Command(cmd)
            }
        };

        let tags = tags.unwrap_or_default();

        if part_of_job {
            self._job_queue
                .put(
                    (
                        cmd,
                        cmd_type.map(str::to_string),
                        on_sent,
                        with_tags(&tags, &["source:job"]),
                    ),
                    true,
                    None,
                )
                .unwrap_or_else(|_| panic!("put on the unbounded job queue"));
            Some(true)
        } else if self.is_printing() && !self.is_sd_file_selected() && !self.job_on_hold() && !force
        {
            match self._command_queue.put(
                (cmd, cmd_type.map(str::to_string), on_sent, tags),
                cmd_type.map(str::to_string),
                true,
                None,
            ) {
                Ok(()) => Some(true),
                Err(PutError::Raised(_e)) => {
                    // self._logger.debug("Type already in command queue: " + e.type)
                    Some(false)
                }
                Err(e) => panic!("put on the unbounded command queue: {e:?}"),
            }
        } else if self.is_operational() || force {
            Some(self._send_command(cmd, cmd_type, on_sent, Some(tags)))
        } else {
            None
        }
    }

    /// comm.py:1322 `_getGcodeScript(scriptName, replacements=None)`: the lines of the script
    /// with their tags.
    ///
    /// `replacements` is a dict (a JSON object; anything else is ignored, as Python does). The
    /// context is built as a minijinja value (`Settings::load_script_value`): the keys of
    /// `last_temperature` that are tool numbers are ints, as in `as_script_dict()`, in Python's
    /// insertion order; `last_position` (a `PositionRecord`) is its `as_dict()`.
    /// OURS: a hook is called with `("gcode", scriptName)` (the comm instance is not passed,
    /// plugin.rs).
    pub fn _get_gcode_script(
        &self,
        script_name: &str,
        replacements: Option<&Value>,
    ) -> Vec<(String, BTreeSet<String>)> {
        self.get_gcode_script_in(settings(), script_name, replacements)
    }

    /// `_getGcodeScript` with the settings it reads the script from (`settings()`; the tests pass
    /// settings that have a script of their own).
    fn get_gcode_script_in(
        &self,
        settings: &Settings,
        script_name: &str,
        replacements: Option<&Value>,
    ) -> Vec<(String, BTreeSet<String>)> {
        let mut context = Value::Object(Default::default());
        if let Some(Value::Object(r)) = replacements {
            for (k, v) in r {
                context[k] = v.clone();
            }
        }

        // `as_script_dict()` as a minijinja map: int tool keys, then "b", "c", the custom ones. The
        // JSON `context` holds a null at the place of each of these keys (which fixes the key
        // order); they are put in below, when the JSON is turned into the minijinja context.
        let mut temperature_dicts: Vec<(&str, minijinja::Value)> = Vec::new();
        let mut script_dict = |key: &'static str, t: &TemperatureRecord| -> Value {
            let d = t.as_script_dict().into_iter().map(|(k, data)| {
                let k = match k {
                    ScriptKey::Tool(n) => minijinja::Value::from(n),
                    ScriptKey::Name(n) => minijinja::Value::from(n),
                };
                (
                    k,
                    minijinja::Value::from_serialize(
                        &serde_json::json!({"actual": data.actual, "target": data.target}),
                    ),
                )
            });
            temperature_dicts.push((key, minijinja::Value::from_iter(d)));
            Value::Null
        };
        let position_dict = |p: &PositionRecord| -> Value {
            Value::Object(
                p.as_dict()
                    .into_iter()
                    .map(|(k, v)| (k, serde_json::json!(v)))
                    .collect(),
            )
        };

        context["printer_profile"] = self
            ._printerProfileManager
            .as_ref()
            .expect("AttributeError: 'NoneType' object has no attribute 'get_current_or_default'")
            .get_current_or_default();
        context["last_position"] = self.last_position.with(|p| position_dict(p));
        context["last_temperature"] = self
            .last_temperature
            .with(|t| script_dict("last_temperature", t));
        context["last_fanspeed"] = serde_json::json!(self.last_fanspeed.get());

        if script_name == "afterPrintPaused" || script_name == "beforePrintResumed" {
            context["pause_position"] = self.pause_position.with(|p| position_dict(p));
            context["pause_temperature"] = self
                .pause_temperature
                .with(|t| script_dict("pause_temperature", t));
            context["pause_fanspeed"] = serde_json::json!(self.pause_fanspeed.get());
        } else if script_name == "afterPrintCancelled" {
            context["cancel_position"] = self.cancel_position.with(|p| position_dict(p));
            context["cancel_temperature"] = self
                .cancel_temperature
                .with(|t| script_dict("cancel_temperature", t));
            context["cancel_fanspeed"] = serde_json::json!(self.cancel_fanspeed.get());
        }

        // scriptLinesPrefix / scriptLinesSuffix: (line, tags); a line is a JSON value (a hook may
        // return anything, only a str survives `process_gcode_line`)
        let mut script_lines_prefix: Vec<(Value, BTreeSet<String>)> = Vec::new();
        let mut script_lines_suffix: Vec<(Value, BTreeSet<String>)> = Vec::new();

        for (name, hook) in self._gcodescript_hooks.iter() {
            let retval = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                hook(&[
                    Value::String("gcode".to_string()),
                    Value::String(script_name.to_string()),
                ])
            })) {
                Ok(retval) => retval,
                Err(_) => {
                    // self._logger.exception(f"Error while processing hook {name}.", extra=...)
                    continue;
                }
            };
            if retval.is_null() {
                continue;
            }
            let Value::Array(retval) = retval else {
                continue;
            };
            if ![2, 3, 4].contains(&retval.len()) {
                continue;
            }

            let to_list =
                |data: &Value, t: &BTreeSet<String>| -> Option<Vec<(Value, BTreeSet<String>)>> {
                    let data: Vec<Value> = match data {
                        Value::String(s) => s
                            .split('\n')
                            .map(|x| Value::String(py_strip(x).to_string()))
                            .collect(),
                        Value::Array(a) => a.clone(),
                        _ => return None,
                    };
                    Some(data.into_iter().map(|x| (x, t.clone())).collect())
                };

            let mut additional_tags: BTreeSet<String> = BTreeSet::new();
            additional_tags.insert(format!("plugin:{name}"));
            if retval.len() == 4 {
                let Value::Array(extra) = &retval[3] else {
                    panic!("TypeError: set(retval[3])")
                };
                for t in extra {
                    additional_tags
                        .insert(t.as_str().expect("TypeError: tag is not a str").to_string());
                }
            }

            let prefix = to_list(&retval[0], &additional_tags);
            let suffix = to_list(&retval[1], &additional_tags);
            if let Some(prefix) = prefix.filter(|p| !p.is_empty()) {
                script_lines_prefix = prefix.into_iter().chain(script_lines_prefix).collect();
            }
            if let Some(suffix) = suffix.filter(|s| !s.is_empty()) {
                script_lines_suffix.extend(suffix);
            }

            if retval.len() == 3 {
                let variables = retval[2].clone();
                context = dict_merge(
                    &context,
                    &serde_json::json!({"plugins": {name.clone(): variables}}),
                    None,
                );
            }
        }

        let context = minijinja::Value::from_iter(context.as_object().expect("a dict").iter().map(
            |(k, v)| {
                let value = match temperature_dicts.iter().find(|(name, _)| name == k) {
                    Some((_, dict)) => dict.clone(),
                    None => minijinja::Value::from_serialize(v),
                };
                (minijinja::Value::from(k.as_str()), value)
            },
        ));
        let template = settings.load_script_value("gcode", script_name, Some(context));
        let script_lines: Vec<(Value, BTreeSet<String>)> = match template {
            None => Vec::new(),
            Some(t) => t
                .split('\n')
                .map(|l| (Value::String(l.to_string()), BTreeSet::new()))
                .collect(),
        };

        let script_lines = script_lines_prefix
            .into_iter()
            .chain(script_lines)
            .chain(script_lines_suffix);

        // process(line): `process_gcode_line(line), tags`; filter: line is not None and not blank
        script_lines
            .filter_map(|(line, tags)| {
                let Value::String(line) = line else {
                    panic!("TypeError: process_gcode_line of a value that is not a str")
                };
                process_gcode_line(&line, None, None).map(|l| (l, tags))
            })
            .filter(|(l, _)| !py_strip(l).is_empty())
            .collect()
    }

    /// comm.py:1420 `sendGcodeScript(scriptName, replacements=None, tags=None, part_of_job=False)`
    pub fn send_gcode_script(
        self: &Arc<Self>,
        script_name: &str,
        replacements: Option<&Value>,
        tags: Option<BTreeSet<String>>,
        part_of_job: bool,
    ) -> String {
        let tags = tags.unwrap_or_default();

        let script_lines = self._get_gcode_script(script_name, replacements);
        let tags_to_use = with_tags(
            &tags,
            &[
                "trigger:comm.send_gcode_script",
                "source:script",
                &format!("script:{script_name}"),
            ],
        );
        for (line, line_tags) in &script_lines {
            // 2-tuple: line + tags
            let ttu: BTreeSet<String> = tags_to_use.union(line_tags).cloned().collect();

            self.send_command(
                QueuedCommand::Command(line.clone()),
                None,
                part_of_job,
                false,
                false,
                None,
                Some(ttu),
            );
        }

        script_lines
            .iter()
            .map(|(l, _)| l.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// comm.py:1453 `startPrint(pos=None, tags=None, external_sd=False, user=None)`; `Err` is
    /// `ValueError("No file selected for printing")`
    pub fn start_print(
        self: &Arc<Self>,
        pos: Option<i64>,
        tags: Option<BTreeSet<String>>,
        external_sd: bool,
        user: Option<&str>,
    ) -> Result<(), ValueError> {
        if !self.is_operational() || self.is_printing() {
            return Ok(());
        }

        if self._currentFile.get().is_none() {
            return Err(ValueError("No file selected for printing"));
        }

        self._heatupWaitStartTime.set(if !self._heating.get() {
            None
        } else {
            Some(time_monotonic())
        });
        self._heatupWaitTimeLost.set(0.0);
        self._pauseWaitStartTime.set(Some(0.0)); // `= 0` (falsy)
        self._pauseWaitTimeLost.set(0.0);

        let tags = tags.unwrap_or_default();

        // if "source:plugin" in tags: ... self._logger.info(f"Starting job on behalf of plugin {tag[7:]}")
        // elif "source:api" in tags: self._logger.info(f"Starting job on behalf of user {user}")

        // try: ... except Exception: the `OSError`s of the file (`start`, `seek`) are caught,
        // every other exception of Python is a panic here (difference 3)
        let attempt = || -> Result<(), OSError> {
            let _job_lock = self._jobLock.lock();
            self._consecutive_not_sd_printing.set(0);

            self.current_file().start()?;
            self._change_state(Self::STATE_STARTING);

            if !self.is_sd_file_selected() {
                self.reset_line_numbers(
                    0,
                    true,
                    Some(with_tags(&BTreeSet::new(), &["trigger:comm.start_print"])),
                );
            }

            self._callback.on_comm_print_job_started(false, user);

            if self.is_sd_file_selected() {
                if !external_sd {
                    // make sure to ignore the "file selected" later on, otherwise we'll reset our progress data
                    self._ignore_select.set(true);

                    self.send_command(
                        QueuedCommand::Command(format!(
                            "M23 {}",
                            self.current_file().get_filename()
                        )),
                        None,
                        true,
                        false,
                        false,
                        None,
                        Some(with_tags(&tags, &["trigger:comm.start_print"])),
                    );
                    match pos {
                        Some(pos) if pos > 0 => {
                            self.current_file().set_pos(pos);
                            self.send_command(
                                QueuedCommand::Command(format!("M26 S{pos}")),
                                None,
                                true,
                                false,
                                false,
                                None,
                                Some(with_tags(&tags, &["trigger:comm.start_print"])),
                            );
                        }
                        _ => self.current_file().set_pos(0),
                    }

                    self.send_command(
                        QueuedCommand::Command("M24".to_string()),
                        None,
                        true,
                        false,
                        false,
                        None,
                        Some(with_tags(&tags, &["trigger:comm.start_print"])),
                    );
                }
            } else if let Some(pos) = pos.filter(|p| *p > 0) {
                self.current_file().seek(pos)?;
            }

            let me = self.clone();
            self.send_command(
                QueuedCommand::Marker(
                    SendQueueMarker::new(Some(Box::new(move || {
                        me._change_state(Self::STATE_PRINTING)
                    })))
                    .0,
                ),
                None,
                true,
                false,
                false,
                None,
                None,
            );

            // now make sure we actually do something, up until now we only filled up the queue
            self._continue_sending();
            Ok(())
        };
        if let Err(_e) = attempt() {
            // self._logger.exception("Error while trying to start printing")
            self._trigger_error(&get_exception_string(), "start_print", true);
        }
        Ok(())
    }

    /// comm.py:1540 `_get_free_remote_name(filename)`
    pub fn _get_free_remote_name(self: &Arc<Self>, filename: &str) -> String {
        if !self._capability_supported(Self::CAPABILITY_LFN_WRITE)
            && valid_file_type(filename, Some("gcode"), None) == Some(true)
        {
            // figure out remote filename
            self.refresh_sd_files(None, true, Some(10.0));
            let existing_sd_files: Vec<String> =
                self.get_sd_files().into_iter().map(|x| x.0).collect();
            // an uncaught ValueError is a panic (difference 3)
            get_dos_filename(
                filename,
                Some(&existing_sd_files),
                Some("gco"),
                Some(&["gco", "g"]),
                None,
            )
            .unwrap_or_else(|e| panic!("ValueError: {}", e.0))
        } else {
            // either LFN_WRITE is supported, or this is not a gcode file
            basename(filename).to_string()
        }
    }

    /// comm.py:1559 `startFileTransfer(path, filename, remote=None, special=False, tags=None)`
    ///
    /// Returns the remote name (`None`: not operational or busy); `Err` is an `OSError` of the
    /// file classes.
    pub fn start_file_transfer(
        self: &Arc<Self>,
        path: &str,
        filename: &str,
        remote: Option<&str>,
        special: bool,
        tags: Option<BTreeSet<String>>,
    ) -> Result<Option<String>, OSError> {
        if !self.is_operational() || self.is_busy() {
            // self._logger.info("Printer is not operational or busy")
            return Ok(None);
        }

        let tags = tags.unwrap_or_default();

        let remote = match remote {
            None => format!("/{}", self._get_free_remote_name(filename)),
            Some(r) => r.to_string(),
        };

        {
            let _job_lock = self._jobLock.lock();
            self.reset_line_numbers(
                0,
                false,
                Some(with_tags(
                    &BTreeSet::new(),
                    &["trigger:comm.start_file_transfer"],
                )),
            );

            if special {
                self._currentFile
                    .set(Some(Arc::new(SpecialStreamingGcodeFileInformation::new(
                        path, filename, &remote, None,
                    )?)));
            } else {
                self._currentFile
                    .set(Some(Arc::new(StreamingGcodeFileInformation::new(
                        path, filename, &remote, None,
                    )?)));
            }
            self.current_file().start()?;

            self.send_command(
                QueuedCommand::Command(format!("M28 {remote}")),
                None,
                false,
                false,
                false,
                None,
                Some(with_tags(&tags, &["trigger:comm.start_file_transfer"])),
            );
            let file = self.current_file();
            self._callback.on_comm_file_transfer_started(
                filename,
                &remote,
                file.get_filesize(),
                file.get_user().as_deref(),
            );
        }

        Ok(Some(remote))
    }

    /// comm.py:1597 `cancelFileTransfer(tags=None)`
    pub fn cancel_file_transfer(self: &Arc<Self>, tags: Option<BTreeSet<String>>) {
        if !self.is_operational() || !self.is_streaming() {
            // self._logger.info("Printer is not operational or not streaming")
            return;
        }

        self._finish_file_transfer(true, tags);
    }

    /// comm.py:1604 `_finishFileTransfer(failed=False, tags=None)`
    pub fn _finish_file_transfer(self: &Arc<Self>, failed: bool, tags: Option<BTreeSet<String>>) {
        let tags = tags.unwrap_or_default();

        let _job_lock = self._jobLock.lock();
        let remote = self.current_file().get_remote_filename();

        self._send_command(
            QueuedCommand::Command("M29".to_string()),
            None,
            None,
            Some(with_tags(&tags, &["trigger:comm.finish_file_transfer"])),
        );
        if failed {
            self.delete_sd_file(&remote, None);
        }

        let local = self.current_file().get_local_filename();
        let elapsed = self.get_print_time();

        let me = self.clone();
        let (local2, remote2) = (local.clone(), remote.clone());
        let finalize = move || {
            me._currentFile.set(None);
            me._change_state(Self::STATE_OPERATIONAL);

            if failed {
                me._callback
                    .on_comm_file_transfer_failed(&local2, &remote2, elapsed);
            } else {
                me._callback
                    .on_comm_file_transfer_done(&local2, &remote2, elapsed);
            }

            me.refresh_sd_files(
                Some(with_tags(
                    &BTreeSet::new(),
                    &["trigger:comm.finish_file_transfer"],
                )),
                false,
                Some(10.0),
            );
        };

        self._send_command(
            QueuedCommand::Marker(SendQueueMarker::new(Some(Box::new(finalize))).0),
            None,
            None,
            None,
        );
    }

    /// comm.py:1641 `selectFile(filename, sd, user=None, tags=None)`; `Err` is the `OSError` of
    /// `PrintingGcodeFileInformation` for a file that does not exist
    pub fn select_file(
        self: &Arc<Self>,
        filename: &str,
        sd: bool,
        user: Option<&str>,
        tags: Option<BTreeSet<String>>,
    ) -> Result<(), OSError> {
        if self.is_busy() {
            return Ok(());
        }

        let tags = tags.unwrap_or_default();

        if sd {
            if !self.is_operational() {
                // printer is not connected, can't use SD
                return Ok(());
            }

            let mut filename = filename;
            if filename.starts_with('/') && truthy(self._sdRelativePath.get()) {
                filename = &filename[1..];
            }

            self._sdFileToSelect.set(Some(filename.to_string()));
            self._sdFileToSelectUser.set(user.map(str::to_string));
            self.send_command(
                QueuedCommand::Command(format!("M23 {filename}")),
                None,
                false,
                false,
                false,
                None,
                Some(with_tags(&tags, &["trigger:comm.select_file"])),
            );
        } else {
            let (me1, me2) = (self.clone(), self.clone());
            self._currentFile
                .set(Some(Arc::new(PrintingGcodeFileInformation::new(
                    filename,
                    Some(Arc::new(move || {
                        me1.get_offsets()
                            .iter()
                            .map(|(k, v)| (k.clone(), *v))
                            .collect()
                    })),
                    Some(Arc::new(move || me2.get_current_tool())),
                    user,
                )?)));
            self._callback.on_comm_file_selected(
                Some(filename),
                self.current_file().get_filesize(),
                false,
                user,
                None,
            );
        }
        Ok(())
    }

    /// comm.py:1676 `unselectFile()`
    pub fn unselect_file(&self) {
        if self.is_busy() {
            return;
        }

        self._currentFile.set(None);
        self._callback
            .on_comm_file_selected(None, None, false, None, None);
    }

    /// comm.py:1683 `_cancel_preparation_failed()`
    pub fn _cancel_preparation_failed(self: &Arc<Self>) {
        let timeout = self
            ._timeout_intervals
            .with(|d| d.get("positionLogWait").copied())
            .unwrap_or(10.0);
        self._log(&format!(
            "Did not receive parseable position data from printer within {}s, continuing without it",
            py_float_repr(timeout)
        ));
        self._record_cancel_data.set(false);
        self._cancel_preparation_done(true, None);
    }

    /// comm.py:1693 `_cancel_preparation_done(check_timer=True, user=None)`
    pub fn _cancel_preparation_done(self: &Arc<Self>, check_timer: bool, user: Option<String>) {
        let mut user = user;
        if user.is_none() {
            let _g = self._action_users_mutex.lock();
            // try: user = self._action_users.pop("cancel") except KeyError: pass
            if let Some(u) = self._action_users.with_mut(|d| d.pop("cancel")) {
                user = u;
            }
        }

        let _g = self._cancel_mutex.lock();
        if let Some(timer) = self._cancel_position_timer.get() {
            timer.cancel();
            self._cancel_position_timer.set(None);
        } else if check_timer {
            return;
        }

        self.current_file().set_done(true);
        self._record_file_position();
        self._callback
            .on_comm_print_job_cancelled(false, user.as_deref());

        let me = self.clone();
        let finalize = move || me._change_state(Self::STATE_OPERATIONAL);

        self.send_command(
            QueuedCommand::Marker(SendQueueMarker::new(Some(Box::new(finalize))).0),
            None,
            true,
            false,
            false,
            None,
            None,
        );
        self._continue_sending();
    }

    /// comm.py:1718 `cancelPrint(firmware_error=None, disable_log_position=False, user=None,
    /// tags=None, external_sd=False)`
    pub fn cancel_print(
        self: &Arc<Self>,
        firmware_error: Option<&str>,
        disable_log_position: bool,
        user: Option<&str>,
        tags: Option<BTreeSet<String>>,
        external_sd: bool,
    ) {
        let cancel_tags = with_tags(&BTreeSet::new(), &["trigger:comm.cancel", "trigger:cancel"]);
        let abort_heatup_tags = with_tags(&cancel_tags, &["trigger:abort_heatup"]);
        let record_position_tags = with_tags(&cancel_tags, &["trigger:record_position"]);

        let tags = tags.unwrap_or_default();

        if !self.is_operational() {
            return;
        }

        if !self.is_busy() || self._currentFile.get().is_none() {
            // we aren't even printing, nothing to cancel...
            return;
        }

        if self.is_cancelling() {
            // we are already cancelling
            return;
        }

        // if "source:plugin" in tags: ... self._logger.info(f"Cancelling job on behalf of plugin {tag[7:]}")
        // elif "source:api" in tags: self._logger.info(f"Cancelling job on behalf of user {user}")

        if self.is_streaming() {
            // we are streaming, we handle cancelling that differently...
            self.cancel_file_transfer(None);
            return;
        }

        self._callback
            .on_comm_print_job_cancelling(firmware_error, user);

        let _job_lock = self._jobLock.lock();
        let pos_autoreporting = self._pos_autoreporting.get();

        self._change_state(Self::STATE_CANCELLING);

        let reenable_pos_autoreport: OnSent = {
            let me = self.clone();
            let record_position_tags = record_position_tags.clone();
            Arc::new(move || {
                if pos_autoreporting {
                    me._set_autoreport_pos_interval(
                        None,
                        false,
                        Some(record_position_tags.clone()),
                    );
                }
            })
        };

        let on_m400_sent: OnSent = {
            let me = self.clone();
            let tags = tags.clone();
            let record_position_tags = record_position_tags.clone();
            Arc::new(move || {
                // we don't call on_print_job_cancelled on our callback here
                // because we do this only after our M114 has been answered
                // by the firmware
                me.cancel_position.with_mut(|p| p.reset());
                me._record_cancel_data.set(true);

                {
                    let _g = me._cancel_mutex.lock();
                    if let Some(timer) = me._cancel_position_timer.get() {
                        timer.cancel();
                    }
                    let me2 = me.clone();
                    let timer = Arc::new(ResettableTimer::new(
                        me._timeout_intervals
                            .with(|d| d.get("positionLogWait").copied())
                            .unwrap_or(10.0),
                        Arc::new(move || me2._cancel_preparation_failed()),
                        None,
                        None,
                        true, // `.daemon = True`
                    ));
                    me._cancel_position_timer.set(Some(timer.clone()));
                    timer.start().expect("threads can only be started once");
                }
                me.send_command(
                    QueuedCommand::Command("M114".to_string()),
                    None,
                    true,
                    false,
                    false,
                    Some(reenable_pos_autoreport.clone()),
                    Some(tags.union(&record_position_tags).cloned().collect()),
                );
            })
        };

        if truthy(self._abort_heatup_on_cancel) {
            // abort any ongoing heatups immediately to get back control over the printer
            self.send_command(
                QueuedCommand::Command("M108".to_string()),
                None,
                false,
                false,
                true,
                None,
                Some(tags.union(&abort_heatup_tags).cloned().collect()),
            );
        }

        if self.is_sd_file_selected() && !external_sd {
            let t: BTreeSet<String> = tags.union(&cancel_tags).cloned().collect();
            self.send_command(
                value_to_cmd(&self._sdCancelCommand),
                None,
                true,
                false,
                false,
                None,
                Some(t.clone()),
            ); // pause print
            self.send_command(
                QueuedCommand::Command("M27".to_string()),
                None,
                true,
                false,
                false,
                None,
                Some(t.clone()),
            ); // get current byte position in file
            self.send_command(
                QueuedCommand::Command("M26 S0".to_string()),
                None,
                true,
                false,
                false,
                None,
                Some(t),
            ); // reset position in file to byte 0
        }

        if truthy(self._log_position_on_cancel) && !disable_log_position {
            {
                let _g = self._action_users_mutex.lock();
                self._action_users
                    .with_mut(|d| d.insert("cancel", user.map(str::to_string)));
            }

            // disable position autoreporting if enabled
            if pos_autoreporting {
                self._set_autoreport_pos_interval(
                    Some(0),
                    true,
                    Some(tags.union(&record_position_tags).cloned().collect()),
                );
            }
            self.send_command(
                QueuedCommand::Command("M400".to_string()),
                None,
                true,
                false,
                false,
                Some(on_m400_sent),
                Some(tags.union(&record_position_tags).cloned().collect()),
            );
            self._continue_sending();
        } else {
            self._cancel_preparation_done(false, user.map(str::to_string));
        }
    }

    /// comm.py:1844 `_pause_preparation_failed()`
    pub fn _pause_preparation_failed(self: &Arc<Self>) {
        let timeout = self
            ._timeout_intervals
            .with(|d| d.get("positionLogWait").copied())
            .unwrap_or(10.0);
        self._log(&format!(
            "Did not receive parseable position data from printer within {}s, continuing without it",
            py_float_repr(timeout)
        ));
        self._record_pause_data.set(false);
        self._pause_preparation_done(true, false, None);
    }

    /// comm.py:1854 `_pause_preparation_done(check_timer=True, suppress_script=False, user=None)`
    pub fn _pause_preparation_done(
        self: &Arc<Self>,
        check_timer: bool,
        suppress_script: bool,
        user: Option<String>,
    ) {
        let mut user = user;
        if user.is_none() {
            let _g = self._action_users_mutex.lock();
            // try: user = self._action_users.pop("pause") except KeyError: pass
            if let Some(u) = self._action_users.with_mut(|d| d.pop("pause")) {
                user = u;
            }
        }

        let _g = self._pause_mutex.lock();
        if let Some(timer) = self._pause_position_timer.get() {
            timer.cancel();
            self._pause_position_timer.set(None);
        } else if check_timer {
            return;
        }

        self._callback
            .on_comm_print_job_paused(suppress_script, user.as_deref());

        let me = self.clone();
        let finalize = move || {
            // only switch to PAUSED if we were still PAUSING, to avoid "swallowing" received resumes during
            // pausing
            if me._state.get() == Self::STATE_PAUSING {
                me._change_state(Self::STATE_PAUSED);
            }
        };

        self.send_command(
            QueuedCommand::Marker(SendQueueMarker::new(Some(Box::new(finalize))).0),
            None,
            true,
            false,
            false,
            None,
            None,
        );
        self._continue_sending();
    }

    /// comm.py:1882 `setPause(pause, local_handling=True, user=None, tags=None)`
    pub fn set_pause(
        self: &Arc<Self>,
        pause: bool,
        local_handling: bool,
        user: Option<&str>,
        tags: Option<BTreeSet<String>>,
    ) {
        if self.is_streaming() {
            return;
        }

        if self._currentFile.get().is_none() {
            return;
        }

        let pause_tags = with_tags(
            &BTreeSet::new(),
            &["trigger:comm.set_pause", "trigger:pause"],
        );
        let resume_tags = with_tags(
            &BTreeSet::new(),
            &["trigger:comm.set_pause", "trigger:resume"],
        );
        let record_position_tags = with_tags(&pause_tags, &["trigger:record_position"]);

        let tags = tags.unwrap_or_default();

        // if "source:plugin" in tags: ... self._logger.info(f"Pausing/resuming job on behalf of plugin {tag[7:]}")
        // elif user: self._logger.info(f"Pausing/resuming job on behalf of user {user}")

        let valid_paused_states = [Self::STATE_PAUSED, Self::STATE_PAUSING];
        let valid_running_states = [
            Self::STATE_PRINTING,
            Self::STATE_STARTING,
            Self::STATE_RESUMING,
        ];

        if !valid_paused_states.contains(&self._state.get())
            && !valid_running_states.contains(&self._state.get())
        {
            return;
        }

        let _job_lock = self._jobLock.lock();
        let pos_autoreporting = self._pos_autoreporting.get();
        if !pause && valid_paused_states.contains(&self._state.get()) {
            if let Some(start) = self._pauseWaitStartTime.get().filter(|s| *s != 0.0) {
                self._pauseWaitTimeLost
                    .set(self._pauseWaitTimeLost.get() + (time_monotonic() - start));
                self._pauseWaitStartTime.set(None);
            }

            self._change_state(Self::STATE_RESUMING);
            self._callback
                .on_comm_print_job_resumed(!local_handling, user);

            if self.is_sd_file_selected() {
                if local_handling {
                    self.send_command(
                        QueuedCommand::Command("M24".to_string()),
                        None,
                        true,
                        false,
                        false,
                        None,
                        Some(tags.union(&resume_tags).cloned().collect()),
                    );
                }
                self.send_command(
                    QueuedCommand::Command("M27".to_string()),
                    None,
                    true,
                    false,
                    false,
                    None,
                    Some(tags.union(&resume_tags).cloned().collect()),
                );
            }

            let me = self.clone();
            let finalize = move || {
                if me._state.get() == Self::STATE_RESUMING {
                    me._change_state(Self::STATE_PRINTING);
                }
            };

            self.send_command(
                QueuedCommand::Marker(SendQueueMarker::new(Some(Box::new(finalize))).0),
                None,
                true,
                false,
                false,
                None,
                None,
            );

            // now make sure we actually do something, up until now we only filled up the queue
            self._continue_sending();
        } else if pause && valid_running_states.contains(&self._state.get()) {
            if !self._pauseWaitStartTime.get().is_some_and(|s| s != 0.0) {
                self._pauseWaitStartTime.set(Some(time_monotonic()));
            }

            self._change_state(Self::STATE_PAUSING);
            if self.is_sd_file_selected() && local_handling {
                self.send_command(
                    QueuedCommand::Command("M25".to_string()),
                    None,
                    true,
                    false,
                    false,
                    None,
                    Some(tags.union(&pause_tags).cloned().collect()),
                ); // pause print
            }

            let reenable_pos_autoreport: OnSent = {
                let me = self.clone();
                let record_position_tags = record_position_tags.clone();
                Arc::new(move || {
                    if pos_autoreporting {
                        me._set_autoreport_pos_interval(
                            None,
                            false,
                            Some(record_position_tags.clone()),
                        );
                    }
                })
            };

            let on_m400_sent: OnSent = {
                let me = self.clone();
                let tags = tags.clone();
                let record_position_tags = record_position_tags.clone();
                Arc::new(move || {
                    // we don't call on_print_job_paused on our callback here
                    // because we do this only after our M114 has been answered
                    // by the firmware
                    me.pause_position.with_mut(|p| p.reset());
                    me._record_pause_data.set(true);

                    {
                        let _g = me._pause_mutex.lock();
                        if let Some(timer) = me._pause_position_timer.get() {
                            timer.cancel();
                            me._pause_position_timer.set(None);
                        }
                        let me2 = me.clone();
                        let timer = Arc::new(ResettableTimer::new(
                            me._timeout_intervals
                                .with(|d| d.get("positionLogWait").copied())
                                .unwrap_or(10.0),
                            Arc::new(move || me2._pause_preparation_failed()),
                            None,
                            None,
                            true, // `.daemon = True`
                        ));
                        me._pause_position_timer.set(Some(timer.clone()));
                        timer.start().expect("threads can only be started once");
                    }
                    me.send_command(
                        QueuedCommand::Command("M114".to_string()),
                        None,
                        true,
                        false,
                        false,
                        Some(reenable_pos_autoreport.clone()),
                        Some(tags.union(&record_position_tags).cloned().collect()),
                    );
                })
            };

            if truthy(self._log_position_on_pause) && local_handling {
                {
                    let _g = self._action_users_mutex.lock();
                    self._action_users
                        .with_mut(|d| d.insert("pause", user.map(str::to_string)));
                }

                // disable position autoreporting if enabled
                if pos_autoreporting {
                    self._set_autoreport_pos_interval(
                        Some(0),
                        true,
                        Some(tags.union(&record_position_tags).cloned().collect()),
                    );
                }
                self.send_command(
                    QueuedCommand::Command("M400".to_string()),
                    None,
                    true,
                    false,
                    false,
                    Some(on_m400_sent),
                    Some(tags.union(&record_position_tags).cloned().collect()),
                );
                self._continue_sending();
            } else {
                self._pause_preparation_done(false, !local_handling, user.map(str::to_string));
            }
        }
    }

    /// comm.py:2016 `getSdFiles()`: `(name, size, longname or name, timestamp)` of every file
    pub fn get_sd_files(&self) -> Vec<(String, Option<i64>, String, Option<i64>)> {
        self._sdFiles.with(|files| {
            files
                .iter()
                .map(|(_, x)| {
                    (
                        x.name.clone(),
                        x.size,
                        match &x.longname {
                            Some(l) if !l.is_empty() => l.clone(),
                            _ => x.name.clone(),
                        },
                        x.timestamp,
                    )
                })
                .collect()
        })
    }

    /// `getSdFiles()` as the list `on_comm_sd_files` takes
    fn sd_files_for_callback(&self) -> Vec<Value> {
        self.get_sd_files()
            .into_iter()
            .map(|(n, s, l, t)| serde_json::json!([n, s, l, t]))
            .collect()
    }

    /// comm.py:2027 `deleteSdFile(filename, tags=None)`
    pub fn delete_sd_file(self: &Arc<Self>, filename: &str, tags: Option<BTreeSet<String>>) {
        if !truthy(self._sdEnabled) {
            return;
        }

        let tags = tags.unwrap_or_default();

        if !self.is_operational()
            || (self.is_busy()
                && self._currentFile.get().is_some_and(|f| {
                    f.is_printing_sd_file_information() && f.get_filename() == filename
                }))
        {
            // do not delete a file from sd we are currently printing from
            return;
        }

        self.send_command(
            QueuedCommand::Command(format!("M30 {}", filename.to_lowercase())),
            None,
            false,
            false,
            false,
            None,
            Some(with_tags(&tags, &["trigger:comm.delete_sd_file"])),
        );
        self.refresh_sd_files(None, false, Some(10.0));
    }

    /// comm.py:2051 `refreshSdFiles(tags=None, blocking=False, timeout=10)`
    pub fn refresh_sd_files(
        self: &Arc<Self>,
        tags: Option<BTreeSet<String>>,
        blocking: bool,
        timeout: Option<f64>,
    ) {
        if !truthy(self._sdEnabled) {
            return;
        }

        if !self.is_operational() || self.is_busy() {
            return;
        }

        if truthy(self._defer_sd_refresh) && !self._firmware_capabilities_received.get() {
            // self._logger.debug("Deferring sd file refresh until capability report is processed")
            return;
        }

        if !self._sdAvailable.get() {
            return;
        }

        let tags = tags.unwrap_or_default();

        let command = if self._capability_supported(Self::CAPABILITY_EXTENDED_M20) {
            "M20 L T"
        } else {
            "M20"
        };

        self._sdFilesAvailable.clear();
        self.send_command(
            QueuedCommand::Command(command.to_string()),
            None,
            false,
            false,
            false,
            None,
            Some(with_tags(&tags, &["trigger:comm.refresh_sd_files"])),
        );
        if blocking {
            self._sdFilesAvailable.wait(timeout);
        }
    }

    /// comm.py:2086 `initSdCard(tags=None)`
    pub fn init_sd_card(self: &Arc<Self>, tags: Option<BTreeSet<String>>) {
        if !truthy(self._sdEnabled) {
            return;
        }

        if !self.is_operational() {
            return;
        }

        let tags = tags.unwrap_or_default();

        self.send_command(
            QueuedCommand::Command("M21".to_string()),
            None,
            false,
            false,
            false,
            None,
            Some(with_tags(&tags, &["trigger:comm.init_sd_card"])),
        );
        if truthy(self._sdAlwaysAvailable.get()) {
            self._sdAvailable.set(true);
            self.refresh_sd_files(None, false, Some(10.0));
            self._callback
                .on_comm_sd_state_change(self._sdAvailable.get());
        }
    }

    /// comm.py:2102 `releaseSdCard(tags=None)`
    pub fn release_sd_card(self: &Arc<Self>, tags: Option<BTreeSet<String>>) {
        if !truthy(self._sdEnabled) {
            return;
        }

        if !self.is_operational() || (self.is_busy() && self.is_sd_file_selected()) {
            // do not release the sd card if we are currently printing from it
            return;
        }

        let tags = tags.unwrap_or_default();

        self.send_command(
            QueuedCommand::Command("M22".to_string()),
            None,
            false,
            false,
            false,
            None,
            Some(with_tags(&tags, &["trigger:comm.release_sd_card"])),
        );
        self._sdAvailable.set(false);
        self._sdFiles.set(PyDict::new());

        self._callback
            .on_comm_sd_state_change(self._sdAvailable.get());
        self._callback
            .on_comm_sd_files(&self.sd_files_for_callback());
    }

    /// comm.py:2126 `sayHello(tags=None)`
    pub fn say_hello(self: &Arc<Self>, tags: Option<BTreeSet<String>>) {
        let tags = tags.unwrap_or_default();

        self.send_command(
            value_to_cmd(&self._hello_command),
            None,
            false,
            false,
            true,
            None,
            Some(with_tags(&tags, &["trigger:comm.say_hello"])),
        );
        self._clear_to_send.set();
        self._hello_sent.set(self._hello_sent.get() + 1);
    }

    /// comm.py:2141 `resetLineNumbers(number=0, part_of_job=False, tags=None)`
    pub fn reset_line_numbers(
        self: &Arc<Self>,
        number: i64,
        part_of_job: bool,
        tags: Option<BTreeSet<String>>,
    ) {
        if !self.is_operational() {
            return;
        }

        let tags = tags.unwrap_or_default();

        self.send_command(
            QueuedCommand::Command(format!("M110 N{number}")),
            None,
            part_of_job,
            false,
            false,
            None,
            Some(with_tags(&tags, &["trigger:comm.reset_line_numbers"])),
        );
    }

    // ~~ record aborted file positions

    /// comm.py:2159 `getFilePosition()`
    pub fn get_file_position(&self) -> Option<FilePosition> {
        let file = self._currentFile.get()?;

        let origin = file.get_file_location();
        let filename = file.get_filename();
        let pos = file.get_filepos();

        Some(FilePosition {
            origin,
            filename,
            pos,
        })
    }

    /// comm.py:2169 `_recordFilePosition()`
    pub fn _record_file_position(&self) {
        if self._currentFile.get().is_none() {
            return;
        }
        let data = self
            .get_file_position()
            .expect("AttributeError: 'NoneType' object is not subscriptable");
        self._callback
            .on_comm_record_fileposition(data.origin, &data.filename, Some(data.pos));
    }

    // ~~ communication monitoring and handling

    /// comm.py:2179 `_processTemperatures(line)`
    ///
    /// OURS: a hook gets the parsed temperatures as a JSON object `{"T0": [actual, target]}` and
    /// is not given the comm instance (plugin.rs).
    pub fn _process_temperatures(&self, line: &str) {
        let current_tool = self._currentTool.get();
        let current_tool_key = format!("T{current_tool}");
        let profile = || {
            self._printerProfileManager
                .as_ref()
                .expect(
                    "AttributeError: 'NoneType' object has no attribute 'get_current_or_default'",
                )
                .get_current_or_default()
        };
        let (max_tool_num, mut parsed_temps) = parse_temperature_line(line, current_tool);

        let extruder_count = profile()["extruder"]["count"]
            .as_i64()
            .expect("TypeError: extruder count");
        let max_tool_num = std::cmp::max(max_tool_num, extruder_count - 1);

        for (name, hook) in self._temperature_hooks.iter() {
            let temps_value: Value = Value::Object(
                parsed_temps
                    .iter()
                    .map(|(k, (a, t))| (k.clone(), serde_json::json!([a, t])))
                    .collect(),
            );
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(&[temps_value]))) {
                Ok(result) => {
                    self._known_broken_temperature_hooks
                        .with_mut(|s| s.remove(name));
                    match &result {
                        Value::Null => return,
                        Value::Object(o) if o.is_empty() => return,
                        Value::Object(o) => {
                            parsed_temps = o
                                .iter()
                                .map(|(k, v)| {
                                    let a = v[0].as_f64().expect("TypeError: actual");
                                    let t = v[1].as_f64();
                                    (k.clone(), (a, t))
                                })
                                .collect();
                        }
                        _ => panic!("TypeError: hook result is not a dict"),
                    }
                }
                Err(_) => {
                    // level = logging.DEBUG; if name not in known broken: level = logging.ERROR; add
                    let _newly_broken = self
                        ._known_broken_temperature_hooks
                        .with_mut(|s| s.insert(name.clone()));
                    // self._logger.log(level, f"Error while processing temperatures in {name}", ...)
                }
            }
        }

        if parsed_temps.contains_key(&current_tool_key) || parsed_temps.contains_key("T0") {
            let shared_nozzle = py_truthy(&profile()["extruder"]["sharedNozzle"]);
            let shared_temp = match parsed_temps.get(&current_tool_key) {
                Some(t) => *t,
                None => *parsed_temps.get("T0").expect("KeyError: 'T0'"),
            };

            for n in 0..(max_tool_num + 1) {
                let tool = format!("T{n}");
                let (actual, target);
                if !parsed_temps.contains_key(&tool) {
                    if shared_nozzle {
                        (actual, target) = shared_temp;
                    } else {
                        continue;
                    }
                } else {
                    (actual, target) = parsed_temps.pop(&tool).expect("checked above");
                }
                self.last_temperature
                    .with_mut(|t| t.set_tool(n, Some(actual), target));
            }
        }

        // bed temperature
        if let Some((actual, target)) = parsed_temps.pop("B") {
            self.last_temperature
                .with_mut(|t| t.set_bed(Some(actual), target));
        }

        // chamber temperature
        if parsed_temps.contains_key("C")
            && (self._capability_supported(Self::CAPABILITY_CHAMBER_TEMP)
                || py_truthy(&profile()["heatedChamber"]))
        {
            let (actual, target) = parsed_temps.pop("C").expect("checked above");
            self.last_temperature
                .with_mut(|t| t.set_chamber(Some(actual), target));
        }

        // all other injected temperatures or temperature-like entries
        for (key, (actual, target)) in parsed_temps.iter() {
            if self._known_invalid_custom_temps.with(|s| s.contains(key)) {
                continue;
            }
            if let Err(_ex) = self
                .last_temperature
                .with_mut(|t| t.set_custom(key, Some(*actual), *target))
            {
                if !self._known_invalid_custom_temps.with(|s| s.contains(key)) {
                    self._known_invalid_custom_temps
                        .with_mut(|s| s.insert(key.clone()));
                    // self._logger.warning(f"Could not add custom temperature record {key}, skipping it from now on: {ex}")
                }
            }
        }
    }

    /// `self._currentFile` where Python goes on to use it as an object (`None` is an
    /// `AttributeError`, difference 3)
    fn current_file(&self) -> Arc<dyn PrintingFileInformation> {
        self._currentFile
            .get()
            .expect("AttributeError: 'NoneType' object has no attribute (the current file)")
    }

    // ~~ comm.py:2260-3307 Serial monitor processing received messages

    /// comm.py:2260 `_monitor()` (target of `monitoring_thread`)
    ///
    /// One function, in Python's order (difference 20). The `try:` body of the loop is the closure
    /// passed to `py_try`; `continue` is `return Flow::Next`, `break` is `return Flow::Break`.
    pub fn _monitor(self: &Arc<Self>) {
        let controls = settings().get(&["controls"]);
        let (feedback_controls, feedback_matcher) =
            convert_feedback_controls(&feedback_controls_from_value(&controls))
                .expect("re.error: the final re.compile of convert_feedback_controls");
        let mut feedback_errors: Vec<String> = Vec::new();
        let pause_triggers = convert_pause_triggers(&pause_triggers_from_value(
            &settings().get(&["printerParameters", "pauseTriggers"]),
        ))
        .expect("re.error: the final re.compile of convert_pause_triggers");

        let mut disable_external_heatup_detection =
            !truthy(settings().get_boolean(&["serial", "externalHeatupDetection"]));

        let wait_for_start = truthy(settings().get_boolean(&["serial", "waitForStartOnConnect"]));

        let suppress_2nd_hello = truthy(settings().get_boolean(&["serial", "suppressSecondHello"]));

        self._consecutive_timeouts.set(0);

        // Open the serial port
        let needs_detection = !(self
            ._port
            .as_deref()
            .is_some_and(|p| !p.is_empty() && p != "AUTO")
            && self._baudrate != 0);
        let mut try_hello = false;

        if !needs_detection {
            self._change_state(Self::STATE_OPEN_SERIAL);
            if !self._open_serial(
                self._port.as_deref().expect("checked above"),
                self._baudrate,
                true,
            ) {
                return;
            }
            try_hello = !wait_for_start;
            self._change_state(Self::STATE_CONNECTING);
            let timeout = self._get_new_communication_timeout();
            self._timeout.set(Some(timeout));
            self._ok_timeout.set(Some(timeout));
        } else {
            self._change_state(Self::STATE_DETECT_SERIAL);
            self._perform_detection_step(true);
        }

        let state = self._state.get();
        if state != Self::STATE_CONNECTING && state != Self::STATE_DETECT_SERIAL {
            // we got cancelled during connection, bail
            return;
        }

        // Start monitoring the serial port
        self._log(&format!(
            "Connected to: {}, starting monitor",
            self._serial
                .get()
                .map_or_else(|| "None".to_string(), |s| s.str())
        ));

        let mut start_seen = false;
        let mut support_repetier_target_temp =
            truthy(settings().get_boolean(&["serial", "repetierTargetTemp"]));
        let support_wait = truthy(settings().get_boolean(&["serial", "supportWait"]));

        // enqueue the "hello command" first thing
        if try_hello {
            self.say_hello(None);

            if !suppress_2nd_hello {
                // we send a second one right away because sometimes there's garbage on the line on first connect
                // that can kill oks
                self.say_hello(None);
            }
        }

        // OURS (difference 20): `copy.copy(self._firmware_capabilities)` as the JSON object a hook
        // gets, and the call `self._callback.on_comm_temperature_update(self.last_temperature.tools,
        // .bed, .chamber, .custom)` that Python writes out four times (comm.py:2756, 2776, 2790,
        // 2806); closures inside `_monitor`, not methods.
        let capabilities_value = || {
            Value::Object(self._firmware_capabilities.with(|d| {
                d.iter()
                    .map(|(k, v)| (k.clone(), Value::Bool(*v)))
                    .collect()
            }))
        };
        let notify_temperature_update = || {
            let tools = self.last_temperature.with(|t| t.tools());
            let bed = self.last_temperature.with(|t| t.bed());
            let chamber = self.last_temperature.with(|t| t.chamber());
            let custom = self.last_temperature.with(|t| t.custom());
            self._callback
                .on_comm_temperature_update(&tools, bed, chamber, &custom);
        };

        while self._monitoring_active.get() {
            let flow = py_try(|| -> Flow {
                let line = self._readline();
                let Some(line) = line else {
                    return Flow::Break;
                };

                let now = time_monotonic();

                if py_strip(&line) != "" {
                    self._consecutive_timeouts.set(0);
                    self._timeout
                        .set(Some(self._get_new_communication_timeout()));

                    if let Some(dwelling_until) = self._dwelling_until.get().filter(|d| *d != 0.0) {
                        if now > dwelling_until {
                            self._dwelling_until.set(None);
                        }
                    }
                }

                if let Some(resend_ok_timer) = self._resend_ok_timer.get() {
                    if !line.is_empty() && !line.starts_with("ok") {
                        // we got anything but an ok after a resend request - this means the ok after the resend request
                        // was in fact missing and we now need to trigger the timer
                        resend_ok_timer.cancel();
                        self._resend_simulate_ok();
                    }
                }

                //~~ busy protocol handling
                if line.starts_with("echo:busy:") || line.starts_with("busy:") {
                    // reset the ok timeout, the regular comm timeout has already been reset
                    self._ok_timeout
                        .set(Some(self._get_new_communication_timeout()));

                    // make sure the printer sends busy in a small enough interval to match our timeout
                    if !self._busy_protocol_detected.get()
                        && truthy(
                            self._capability_support
                                .with(|d| d.get(Self::CAPABILITY_BUSY_PROTOCOL).copied())
                                .unwrap_or(Some(false)),
                        )
                    {
                        // to_log = "Printer seems to support the busy protocol, ..."
                        self._log("Printer seems to support the busy protocol, will adjust timeouts and set busy interval accordingly");
                        // self._logger.info(to_log)

                        self._busy_protocol_detected.set(true);
                        let communication_busy = self
                            ._timeout_intervals
                            .with(|d| d.get("communicationBusy").copied())
                            .unwrap_or(2.0);
                        let busy_interval = std::cmp::max(communication_busy as i64 - 1, 1);

                        // def busyIntervalSet(busy_interval): ... (the logger.info is a comment)
                        let me = self.clone();
                        let busy_interval_set: OnSent = Arc::new(move || {
                            // self._logger.info("Telling the printer to set the busy interval to our "
                            //                   '"communicationBusy" timeout - 1s = {}s'.format(busy_interval))
                            me._busy_protocol_support.set(true);
                            let timeout = me._get_communication_timeout_interval();
                            me._serial
                                .get()
                                .expect(
                                    "AttributeError: 'NoneType' object has no attribute 'timeout'",
                                )
                                .set_timeout(timeout);
                        });

                        self._set_busy_protocol_interval(
                            Some(busy_interval),
                            Some(busy_interval_set),
                        );
                    }

                    let state = self._state.get();
                    if state != Self::STATE_CONNECTING && state != Self::STATE_DETECT_SERIAL {
                        return Flow::Next;
                    }
                }
                //~~ debugging output handling
                else if line.starts_with("//") {
                    let debugging_output = py_strip(&line[2..]);
                    if debugging_output.starts_with("action:") {
                        let action_command = py_strip(&debugging_output["action:".len()..]);
                        let (action_name, action_params) =
                            if let Some((n, p)) = action_command.split_once(' ') {
                                (py_strip(n), p)
                            } else {
                                (action_command, "")
                            };

                        if action_name == "start" {
                            if self._currentFile.get().is_some() {
                                self._dual_log(
                                    "(Re)Starting current job on request of the printer...",
                                    LOGGING_INFO,
                                    "",
                                );
                                self.start_print(
                                    None,
                                    Some(with_tags(
                                        &BTreeSet::new(),
                                        &["trigger:serial.action_command.start"],
                                    )),
                                    false,
                                    None,
                                )
                                .expect("ValueError: No file selected for printing");
                            }
                        } else if action_name == "cancel" {
                            self._dual_log(
                                "Cancelling on request of the printer...",
                                LOGGING_INFO,
                                "",
                            );
                            self.cancel_print(
                                None,
                                false,
                                None,
                                Some(with_tags(
                                    &BTreeSet::new(),
                                    &["trigger:serial.action_command.cancel"],
                                )),
                                false,
                            );
                        } else if action_name == "pause" {
                            self._dual_log(
                                "Pausing on request of the printer...",
                                LOGGING_INFO,
                                "",
                            );
                            self.set_pause(
                                true,
                                true,
                                None,
                                Some(with_tags(
                                    &BTreeSet::new(),
                                    &["trigger:serial.action_command.pause"],
                                )),
                            );
                        } else if action_name == "paused" {
                            self._dual_log(
                                "Printer signalled that it paused, switching state...",
                                LOGGING_INFO,
                                "",
                            );
                            self.set_pause(
                                true,
                                false,
                                None,
                                Some(with_tags(
                                    &BTreeSet::new(),
                                    &["trigger:serial.action_command.paused"],
                                )),
                            );
                        } else if action_name == "resume" {
                            self._dual_log(
                                "Resuming on request of the printer...",
                                LOGGING_INFO,
                                "",
                            );
                            self.set_pause(
                                false,
                                true,
                                None,
                                Some(with_tags(
                                    &BTreeSet::new(),
                                    &["trigger:serial.action_command.resume"],
                                )),
                            );
                        } else if action_name == "resumed" {
                            self._dual_log(
                                "Printer signalled that it resumed, switching state...",
                                LOGGING_INFO,
                                "",
                            );
                            self.set_pause(
                                false,
                                false,
                                None,
                                Some(with_tags(
                                    &BTreeSet::new(),
                                    &["trigger:serial.action_command.resumed"],
                                )),
                            );
                        } else if action_name == "disconnect" {
                            self._dual_log(
                                "Disconnecting on request of the printer...",
                                LOGGING_INFO,
                                "",
                            );
                            self._callback.on_comm_force_disconnect();
                        } else if action_name == "shutdown" {
                            if truthy(self._enable_shutdown_action_command) {
                                self._dual_log(
                                    "Shutting down system on request of printer...",
                                    LOGGING_WARNING,
                                    "",
                                );
                                if let Err(ex) =
                                    py_try(system_command_manager_perform_system_shutdown)
                                {
                                    // except Exception as ex: self._log(f"Error executing system shutdown: {ex}")
                                    self._log(&format!("Error executing system shutdown: {ex}"));
                                }
                            } else {
                                self._dual_log(
                                    "Received a shutdown command from the printer, but processing of this command is disabled",
                                    LOGGING_WARNING,
                                    "",
                                );
                            }
                        } else if truthy(self._sdEnabled) && action_name == "sd_inserted" {
                            self._dual_log(
                                "Printer reported SD card as inserted",
                                LOGGING_INFO,
                                "",
                            );
                            self._sdAvailable.set(true);
                            self.refresh_sd_files(None, false, Some(10.0));
                            self._callback
                                .on_comm_sd_state_change(self._sdAvailable.get());
                        } else if truthy(self._sdEnabled) && action_name == "sd_ejected" {
                            self._dual_log("Printer reported SD card as ejected", LOGGING_INFO, "");
                            self._sdAvailable.set(false);
                            self._sdFiles.set(PyDict::new());
                            self._callback
                                .on_comm_sd_state_change(self._sdAvailable.get());
                        } else if truthy(self._sdEnabled) && action_name == "sd_updated" {
                            self._dual_log(
                                "Printer reported a change on the SD card",
                                LOGGING_INFO,
                                "",
                            );
                            self.refresh_sd_files(None, false, Some(10.0));
                        }

                        for (_name, hook) in self._printer_action_hooks.iter() {
                            // hook(self, line, action_command, name=action_name, params=action_params)
                            // (OURS: no comm instance, name and params positional, plugin.rs)
                            let args = [
                                Value::from(line.as_str()),
                                Value::from(action_command),
                                Value::from(action_name),
                                Value::from(action_params),
                            ];
                            if let Err(_) = py_try(|| hook(&args)) {
                                // self._logger.exception("Error while calling hook from plugin {} with action command {}")
                                continue;
                            }
                        }
                    }

                    let state = self._state.get();
                    if state != Self::STATE_CONNECTING && state != Self::STATE_DETECT_SERIAL {
                        return Flow::Next;
                    }
                }

                // def convert_line(line) is the two lines below (line is never None here, see difference 20)
                //~~ Error handling
                let line = self
                    ._handle_errors(Some(&line))
                    .expect("AttributeError: 'NoneType' object has no attribute 'startswith'");
                let line = py_strip(&line).trim_matches('\0').to_string();
                let lower_line = line.to_lowercase();

                //~~ SD file list
                // if we are currently receiving an sd file list, each line is just a filename, so just read it and abort processing
                if truthy(self._sdEnabled)
                    && self._sdFileList.get()
                    && !line.contains("End file list")
                {
                    let (mut filename, size, mut timestamp, longname) = parse_file_list_line(&line);

                    if valid_file_type(&filename, Some("machinecode"), None) == Some(true) {
                        if filter_non_ascii(&filename) {
                            // self._logger.warning("Got a file from printer's SD that has a non-ascii filename ({!r}), "
                            //                      "that shouldn't happen according to the protocol")
                        } else if longname
                            .as_deref()
                            .is_some_and(|l| !l.is_empty() && filter_non_utf8(l))
                        {
                            // self._logger.warning("Got a file from printer's SD that has a non-utf8 longname ({!r}), "
                            //                      "that shouldn't happen according to the protocol")
                        } else {
                            if !filename.starts_with('/') {
                                // file from the root of the sd -- we'll prepend a /
                                filename = format!("/{filename}");
                            }
                            if truthy(self._sdLowerCase.get()) {
                                filename = filename.to_lowercase();
                            }
                            if timestamp.is_some_and(|t| t < MINIMAL_SD_TIMESTAMP) {
                                // sanitize timestamp, when creating a file through serial the firmware usually doesn't know the date
                                // and sets something ridiculously low (e.g. 2000-01-01), so let's ignore timestamps like that here
                                timestamp = None;
                            }
                            let data = SDFileData {
                                name: filename.clone(),
                                size,
                                timestamp,
                                longname,
                            };
                            self._sdFiles.with_mut(|d| d.insert(&filename, data));
                        }
                        return Flow::Next;
                    }
                } else if let Some(sd_file_long_name) =
                    self._sdFileLongName.get().filter(|n| !n.is_empty())
                {
                    // Response to M33: just one line with the long name. Yay.
                    //
                    // As we have NO way to parse the request short name from that, and also no indicator
                    // on the line that this is in fact a reply to M33 and not just some arbitrary line injected
                    // by the firmware belonging to some other command and/or an out of band message/autoreport, we have
                    // to hope for the best here, set our requested filename as a flag and only use the potential mapping
                    // if it has a valid file extension.
                    //
                    // It would have been so much easier if M33's output was something sensible like
                    // "<short name>: <long name>" or such, but why make things easy...
                    let (_, ext) = splitext(&lower_line);
                    if !ext.is_empty()
                        && valid_file_type(&lower_line, Some("machinecode"), None) == Some(true)
                    {
                        let data = self._sdFiles.with(|d| d.get(&sd_file_long_name).cloned());
                        if let Some(data) = data {
                            let new = SDFileData {
                                name: data.name,
                                size: data.size,
                                timestamp: data.timestamp,
                                longname: Some(line.clone()),
                            };
                            self._sdFiles
                                .with_mut(|d| d.insert(&sd_file_long_name, new));
                        }
                        self._sdFileLongName.set(None);
                    }
                }

                let mut handled = false;

                // process oks
                if line.starts_with("ok") || (self.is_printing() && support_wait && line == "wait")
                {
                    // ok only considered handled if it's alone on the line, might be
                    // a response to an M105 or an M114
                    self._handle_ok();
                    self._sdFileLongName.set(None); // reset looking for M33 response
                    let needs_further_handling = line.contains("T:")
                        || line.contains("T0:")
                        || line.contains("B:")
                        || line.contains("C:")
                        || line.contains("X:")
                        || line.contains("NAME:");
                    handled = line == "wait" || line == "ok" || !needs_further_handling;
                }
                // process resends
                else if lower_line.starts_with("resend") || lower_line.starts_with("rs") {
                    self._handle_resend_request(&line);
                    handled = true;
                }
                // process timeouts
                else if ((line.is_empty()
                    && now
                        >= self._timeout.get().expect(
                            "TypeError: '>=' not supported between 'float' and 'NoneType'",
                        ))
                    || (self.is_printing()
                        && !self.is_sd_printing()
                        && (!self.job_on_hold() || self._resendActive.get())
                        && !self._long_running_command.get()
                        && !self._heating.get()
                        && now
                            >= self._ok_timeout.get().expect(
                                "TypeError: '>=' not supported between 'float' and 'NoneType'",
                            )))
                    && (!truthy(self._blockWhileDwelling.get())
                        || self._dwelling_until.get().filter(|d| *d != 0.0).is_none()
                        || now > self._dwelling_until.get().expect("checked above"))
                    && self._state.get() != Self::STATE_DETECT_SERIAL
                {
                    // We have two timeout variants:
                    //
                    // Variant 1: No line at all received within the communication timeout. This can always happen.
                    //
                    // Variant 2: No ok received while printing within the communication timeout. This can happen if
                    //            temperatures are auto reported, because then we'll continue to receive data from the
                    //            firmware in fairly regular intervals, even if an ok got lost somewhere and the firmware
                    //            is running dry but not sending a wait.
                    //
                    // Both variants can only happen if we are not currently blocked by a dwelling command

                    self._handle_timeout();
                    self._ok_timeout
                        .set(Some(self._get_new_communication_timeout()));

                    // timeout only considered handled if the printer is printing and it was a comm timeout, not an ok
                    // timeout
                    handled = self.is_printing() && line.is_empty();
                }

                // we don't have to process the rest if the line has already been handled fully
                let state = self._state.get();
                if handled && state != Self::STATE_CONNECTING && state != Self::STATE_DETECT_SERIAL
                {
                    return Flow::Next;
                }

                // wait for the end of the firmware capability report (M115) then notify plugins and refresh sd list if deferred
                if !self._firmware_capabilities.with(|d| d.is_empty())
                    && !self._firmware_capabilities_received.get()
                    && !lower_line.starts_with("cap:")
                {
                    self._firmware_capabilities_received.set(true);

                    if truthy(self._defer_sd_refresh) {
                        // sd list was deferred, refresh it now
                        // self._logger.debug("Performing deferred sd file refresh")
                        self.refresh_sd_files(None, false, Some(10.0));
                    }

                    // notify plugins
                    for (_name, hook) in self
                        ._firmware_info_hooks
                        .get("capability_report")
                        .expect("KeyError: 'capability_report'")
                        .iter()
                    {
                        // hook(self, copy.copy(self._firmware_capabilities))
                        let caps = capabilities_value();
                        if let Err(_) = py_try(|| hook(&[caps])) {
                            // self._logger.exception("Error processing firmware reported hook {}:")
                        }
                    }

                    // log firmware capabilities
                    // capability_list = "\n  ".join([f"{capability}: {'supported' if enabled else 'not supported'}" ...])
                    // self._logger.info("Firmware sent the following capability report:\n  " + capability_list)
                }

                //~~ position report processing
                if line.contains("X:") && line.contains("Y:") && line.contains("Z:") {
                    let parsed = parse_position_line(&line);
                    if let Some(parsed) = parsed.filter(|p| !p.is_empty()) {
                        // we don't know T or F when printing from SD since
                        // there's no way to query it from the firmware and
                        // no way to track it ourselves when not streaming
                        // the file - this all sucks sooo much
                        self.last_position.with_mut(|p| p.valid = Some(true));
                        self.last_position
                            .with_mut(|p| p.setattr("x", parsed.get("x").copied()));
                        self.last_position
                            .with_mut(|p| p.setattr("y", parsed.get("y").copied()));
                        self.last_position
                            .with_mut(|p| p.setattr("z", parsed.get("z").copied()));

                        if parsed.contains_key("e") {
                            self.last_position
                                .with_mut(|p| p.setattr("e", parsed.get("e").copied()));
                        } else {
                            // multiple extruder coordinates provided, find current one
                            let e = if !self.is_sd_file_selected() {
                                parsed
                                    .get(&format!("e{}", self._currentTool.get()))
                                    .copied()
                            } else {
                                None
                            };
                            self.last_position.with_mut(|p| p.setattr("e", e));
                        }

                        for key in parsed.keys().filter(|x| x.starts_with('e') && x.len() > 1) {
                            self.last_position
                                .with_mut(|p| p.setattr(key, parsed.get(key).copied()));
                        }

                        let t = if !self.is_sd_file_selected() {
                            Some(self._currentTool.get() as f64)
                        } else {
                            None
                        };
                        self.last_position.with_mut(|p| p.setattr("t", t));
                        let f = if !self.is_sd_file_selected() {
                            self._currentF.get()
                        } else {
                            None
                        };
                        self.last_position.with_mut(|p| p.setattr("f", f));

                        let mut reason: Option<&str> = None;

                        if self._record_pause_data.get() {
                            reason = Some("pause");
                            self._record_pause_data.set(false);
                            let last_position = self.last_position.get();
                            self.pause_position
                                .with_mut(|p| p.copy_from(&last_position));
                            let last_temperature = self.last_temperature.get();
                            self.pause_temperature
                                .with_mut(|t| t.copy_from(&last_temperature));
                            self.pause_fanspeed.set(self.last_fanspeed.get());
                            self._pause_preparation_done(true, false, None);
                        }

                        if self._record_cancel_data.get() {
                            reason = Some("cancel");
                            self._record_cancel_data.set(false);
                            let last_position = self.last_position.get();
                            self.cancel_position
                                .with_mut(|p| p.copy_from(&last_position));
                            let last_temperature = self.last_temperature.get();
                            self.cancel_temperature
                                .with_mut(|t| t.copy_from(&last_temperature));
                            self.cancel_fanspeed.set(self.last_fanspeed.get());
                            self._cancel_preparation_done(true, None);
                        }

                        self._callback
                            .on_comm_position_update(&self.last_position.get().as_dict(), reason);
                    }
                }
                //~~ temperature processing
                else if line.contains(" T:")
                    || line.starts_with("T:")
                    || line.contains(" T0:")
                    || line.starts_with("T0:")
                    || ((line.contains(" B:") || line.starts_with("B:")) && !line.contains("A:"))
                {
                    if !disable_external_heatup_detection
                        && !self._temperature_autoreporting.get()
                        && !py_strip(&line).starts_with("ok")
                        && !self._heating.get()
                        && self._firmware_info_received.get()
                    {
                        // self._logger.info("Externally triggered heatup detected")
                        self._heating.set(true);
                        self._heatupWaitStartTime.set(Some(time_monotonic()));
                    }

                    self._process_temperatures(&line);
                    notify_temperature_update();
                } else if support_repetier_target_temp
                    && (line.contains("TargetExtr") || line.contains("TargetBed"))
                {
                    let match_extr = re_match(&regex_repetierTempExtr, &line);
                    let match_bed = re_match(&regex_repetierTempBed, &line);

                    if let Some(match_extr) = match_extr {
                        let tool_num = py_int(match_extr.get(1).expect("group 1").as_str())
                            .expect("int() of \\d+");
                        // try: ... except ValueError: pass
                        if let Some(target) = py_float(match_extr.get(2).expect("group 2").as_str())
                        {
                            self.last_temperature
                                .with_mut(|t| t.set_tool(tool_num, None, Some(target)));
                            notify_temperature_update();
                        }
                    } else if let Some(match_bed) = match_bed {
                        if let Some(target) = py_float(match_bed.get(1).expect("group 1").as_str())
                        {
                            self.last_temperature
                                .with_mut(|t| t.set_bed(None, Some(target)));
                            notify_temperature_update();
                        }
                    }
                }
                //~~ Firmware capability report triggered by M115
                else if lower_line.starts_with("cap:") {
                    let parsed = parse_capability_line(&lower_line);
                    if let Some((capability, enabled)) = parsed {
                        self._firmware_capabilities
                            .with_mut(|d| d.insert(&capability, enabled));

                        if truthy(
                            self._capability_support
                                .with(|d| d.get(&capability).copied())
                                .unwrap_or(Some(false)),
                        ) {
                            if capability == Self::CAPABILITY_AUTOREPORT_TEMP && enabled {
                                // self._logger.info("Firmware states that it supports temperature autoreporting")
                                self._set_autoreport_temperature_interval(None);
                            } else if capability == Self::CAPABILITY_AUTOREPORT_SD_STATUS && enabled
                            {
                                // self._logger.info("Firmware states that it supports sd status autoreporting")
                                self._set_autoreport_sdstatus_interval(None);
                            } else if capability == Self::CAPABILITY_AUTOREPORT_POS && enabled {
                                // self._logger.info("Firmware states that it supports position autoreporting")
                                self._set_autoreport_pos_interval(None, false, None);
                            } else if capability == Self::CAPABILITY_EMERGENCY_PARSER && enabled {
                                // self._logger.info("Firmware states that it supports emergency GCODEs to be sent without waiting for an acknowledgement first")
                            } else if capability == Self::CAPABILITY_EXTENDED_M20 && enabled {
                                // self._logger.info("Firmware states that it supports long filenames")
                            } else if capability == Self::CAPABILITY_LFN_WRITE && enabled {
                                // self._logger.info("Firmware states that it supports writing long filenames")
                            }
                        }

                        // notify plugins
                        for (_name, hook) in self
                            ._firmware_info_hooks
                            .get("capabilities")
                            .expect("KeyError: 'capabilities'")
                            .iter()
                        {
                            // hook(self, capability, enabled, copy.copy(self._firmware_capabilities))
                            let args = [
                                Value::from(capability.as_str()),
                                Value::from(enabled),
                                capabilities_value(),
                            ];
                            if let Err(_) = py_try(|| hook(&args)) {
                                // self._logger.exception("Error processing firmware capability hook {}:")
                            }
                        }
                    }
                }
                //~~ firmware name & version
                else if line.contains("NAME:") || line.starts_with("NAME.") {
                    // looks like a response to M115
                    let data = parse_firmware_line(&line);
                    let mut firmware_name: Option<String> = data.get("FIRMWARE_NAME").cloned();

                    if firmware_name.is_none() {
                        // Malyan's "Marlin compatible firmware" isn't actually Marlin compatible and doesn't even
                        // report its firmware name properly in response to M115. Wonderful - why stick to established
                        // protocol when you can do your own thing, right?
                        //
                        // Examples:
                        //
                        //     NAME: Malyan VER: 2.9 MODEL: M200 HW: HA02
                        //     NAME. Malyan	VER: 3.8	MODEL: M100	HW: HB02
                        //     NAME. Malyan VER: 3.7 MODEL: M300 HW: HG01
                        //
                        // We do a bit of manual fiddling around here to circumvent that issue and get ourselves a
                        // reliable firmware name (NAME + VER) out of the Malyan M115 response.
                        let name = data.get("NAME");
                        let ver = data.get("VER");
                        if let (Some(name), Some(ver)) = (name, ver) {
                            if !name.is_empty()
                                && name.to_lowercase().contains("malyan")
                                && !ver.is_empty()
                            {
                                firmware_name =
                                    Some(format!("{} {}", py_strip(name), py_strip(ver)));
                            }
                        }
                    }

                    event_manager().fire(
                        Events::FIRMWARE_DATA,
                        Some(serde_json::json!({"name": firmware_name, "data": py_dict_to_json(&data)})),
                    );

                    if !self._firmware_info_received.get()
                        && firmware_name.as_deref().is_some_and(|n| !n.is_empty())
                    {
                        let firmware_name =
                            py_strip(firmware_name.as_deref().expect("checked above")).to_string();
                        // self._logger.info(f'Printer reports firmware name "{firmware_name}"')
                        // self._logger.info(f"Firmware info line: {line}")

                        if truthy(self._firmware_detection) {
                            let lower_name = firmware_name.to_lowercase();
                            if lower_name.contains("repetier") || lower_name.contains("anet_a8") {
                                // self._logger.info("Detected Repetier firmware, enabling relevant features for issue free communication")
                                self._alwaysSendChecksum.set(Some(true));
                                self._blockWhileDwelling.set(Some(true));
                                support_repetier_target_temp = true;
                                disable_external_heatup_detection = true;
                            } else if lower_name.contains("reprapfirmware") {
                                // self._logger.info("Detected RepRapFirmware, enabling relevant features for issue free communication")
                                self._sdRelativePath.set(Some(true));
                            } else if lower_name.contains("malyan") {
                                // self._logger.info("Detected Malyan firmware, enabling relevant features for issue free communication")
                                self._alwaysSendChecksum.set(Some(true));
                                self._blockWhileDwelling.set(Some(true));

                                let sd_always_available = self._sdAlwaysAvailable.get();
                                self._sdAlwaysAvailable.set(Some(true));
                                if !truthy(sd_always_available) && !self._sdAvailable.get() {
                                    self.init_sd_card(None);
                                }
                            } else if lower_name.contains("teacup") {
                                // self._logger.info("Detected Teacup firmware, enabling relevant features for issue free communication")
                                disable_external_heatup_detection = true; // see #2854
                            } else if lower_name.contains("klipper") {
                                // self._logger.info("Detected Klipper firmware, enabling relevant features for issue free communication")
                                self._unknownCommandsNeedAck.set(Some(true));
                            } else if lower_name.contains("ultimaker2") {
                                // self._logger.info("Detected Ultimaker2 firmware, enabling relevant features for issue free communication")
                                self._disable_sd_printing_detection.set(Some(true));
                            } else if lower_name.contains("prusa-firmware") {
                                // self._logger.info("Detected Prusa firmware, enabling relevant features for issue free communication")
                                self._sdLowerCase.set(Some(true));
                                self._sendChecksumWithUnknownCommands.set(Some(true));
                                self._unknownCommandsNeedAck.set(Some(true));
                            }
                        }

                        self._firmware_info_received.set(true);
                        self._firmware_info.set(data.clone());
                        self._firmware_name.set(Some(firmware_name.clone()));

                        // notify plugins
                        for (_name, hook) in self
                            ._firmware_info_hooks
                            .get("info")
                            .expect("KeyError: 'info'")
                            .iter()
                        {
                            // hook(self, firmware_name, copy.copy(data))
                            let args =
                                [Value::from(firmware_name.as_str()), py_dict_to_json(&data)];
                            if let Err(_) = py_try(|| hook(&args)) {
                                // self._logger.exception("Error processing firmware info hook {}:")
                            }
                        }

                        self._callback.on_comm_firmware_info(&firmware_name, &data);
                    }
                }
                //~~ invalid extruder
                else if lower_line.contains("invalid extruder") {
                    let mut tool: Option<i64> = None;

                    let m = re_captures(&regexes_parameters["intT"], &line);
                    if let Some(m) = m {
                        // try: tool = int(match.group("value")) except ValueError: pass  # should never happen
                        tool = py_int(m.name("value").expect("group value").as_str());
                    }

                    if tool.is_none() || tool == Some(self._currentTool.get()) {
                        let fallback_tool = match self._toolBeforeChange.get() {
                            Some(t) => t,
                            None => 0,
                        };

                        let invalid_tool = self._currentTool.get();

                        if truthy(self._sanity_check_tools) {
                            // log to terminal and remember as invalid
                            let message = format!(
                                "T{invalid_tool} reported as invalid, reverting to T{fallback_tool}"
                            );
                            self._log(&format!("Warn: {message}"));
                            // self._logger.warning(message)
                            event_manager().fire(
                                Events::INVALID_TOOL_REPORTED,
                                Some(serde_json::json!({"tool": invalid_tool, "fallback": fallback_tool})),
                            );

                            self._known_invalid_tools
                                .with_mut(|s| s.insert(invalid_tool.to_string()));

                            // we actually do send a T command here instead of just settings self._currentTool just in case
                            // we had any scripts or plugins modify stuff due to the prior tool change
                            self.send_command(
                                QueuedCommand::Command(format!("T{fallback_tool}")),
                                None,
                                false,
                                false,
                                false,
                                None,
                                Some(with_tags(
                                    &BTreeSet::new(),
                                    &["trigger:revert_invalid_tool"],
                                )),
                            );
                        } else {
                            // just log to terminal, user disabled sanity check
                            self._log(&format!(
                                "T{invalid_tool} reported as invalid by the firmware, but you've disabled tool sanity checking, ignoring"
                            ));
                        }
                    }
                }
                //~~ SD Card handling
                else if truthy(self._sdEnabled) {
                    if line.contains("SD init fail")
                        || line.contains("volume.init failed")
                        || line.contains("No media")
                        || line.contains("openRoot failed")
                        || line.contains("SD Card unmounted")
                        || line.contains("SD card released")
                    {
                        self._sdAvailable.set(false);
                        self._sdFiles.set(PyDict::new());
                        self._callback
                            .on_comm_sd_state_change(self._sdAvailable.get());
                    } else if (line.contains("SD card ok")
                        || line.contains("Card successfully initialized"))
                        && !self._sdAvailable.get()
                    {
                        self._sdAvailable.set(true);
                        self.refresh_sd_files(None, false, Some(10.0));
                        self._callback
                            .on_comm_sd_state_change(self._sdAvailable.get());
                    } else if line.contains("Begin file list") {
                        self._sdFiles.set(PyDict::new());
                        self._sdFileList.set(true);
                    } else if line.contains("End file list") {
                        self._sdFileList.set(false);
                        self._sdFilesAvailable.set();
                        self._callback
                            .on_comm_sd_files(&self.sd_files_for_callback());
                    } else if line.contains("SD printing byte") {
                        // answer to M27, at least on Marlin, Repetier and Sprinter: "SD printing byte %d/%d"
                        let m = re_captures(&regex_sdPrintingByte, &line);
                        if let Some(m) = m {
                            // try: ... except Exception: self._logger.exception("Error while parsing SD status report")
                            let current =
                                py_int(m.name("current").expect("group current").as_str());
                            let total = py_int(m.name("total").expect("group total").as_str());
                            if let (Some(current), Some(total)) = (current, total) {
                                if current == total
                                    && total == 0
                                    && self.is_sd_printing()
                                    && !self.is_starting()
                                    && !self.is_starting()
                                    && !self.is_finishing()
                                {
                                    // apparently not SD printing - some Marlin versions report it like that for some reason
                                    self._consecutive_not_sd_printing
                                        .set(self._consecutive_not_sd_printing.get() + 1);
                                    if self._consecutive_not_sd_printing.get()
                                        > self
                                            ._consecutive_not_sd_printing_maximum
                                            .expect("TypeError: '>' not supported between instances of 'int' and 'NoneType'")
                                    {
                                        self.cancel_print(None, false, None, None, true);
                                    }
                                } else {
                                    self._consecutive_not_sd_printing.set(0);
                                    if self.is_sd_file_selected() {
                                        // If we are not yet sd printing, the current does not equal the total, is larger
                                        // than zero and has increased since the last time we saw a position report, then
                                        // yes, this looks like we just started printing due to an external trigger.
                                        if !self.is_sd_printing()
                                            && current != total
                                            && current > 0
                                            && self._currentFile.get().is_some()
                                            && current > self.current_file().get_filepos()
                                        {
                                            self.start_print(None, None, true, None).expect(
                                                "ValueError: No file selected for printing",
                                            );
                                        }

                                        self.current_file().set_pos(current);
                                        if self.current_file().size() == Some(0) {
                                            self.current_file().set_size(Some(total));
                                        }

                                        if !self.current_file().done() {
                                            self._callback.on_comm_progress();
                                        }
                                    }
                                }
                            }
                        }
                    } else if line.contains("Not SD printing")
                        && self.is_sd_printing()
                        && !self.is_starting()
                        && !self.is_finishing()
                    {
                        self._consecutive_not_sd_printing
                            .set(self._consecutive_not_sd_printing.get() + 1);
                        if self._consecutive_not_sd_printing.get()
                            > self
                                ._consecutive_not_sd_printing_maximum
                                .expect("TypeError: '>' not supported between instances of 'int' and 'NoneType'")
                        {
                            // something went wrong, printer is reporting that we actually are not printing right now...
                            self.cancel_print(None, false, None, None, true);
                        }
                    } else if line.contains("File opened") && !self._ignore_select.get() {
                        // answer to M23, at least on Marlin, Repetier and Sprinter: "File opened:%s Size:%d"
                        let m = re_captures(&regex_sdFileOpened, &line);
                        let (mut name, size);
                        if let Some(m) = m {
                            name = m.name("name").expect("group name").as_str().to_string();
                            size = py_int(m.name("size").expect("group size").as_str())
                                .expect("int() of \\d+");
                        } else {
                            name = "Unknown".to_string();
                            size = 0;
                        }
                        let mut user: Option<String> = None;

                        if let Some(sd_file_to_select) =
                            self._sdFileToSelect.get().filter(|n| !n.is_empty())
                        {
                            name = sd_file_to_select;
                            user = self._sdFileToSelectUser.get();
                        }

                        self._sdFileToSelect.set(None);
                        self._sdFileToSelectUser.set(None);

                        self._currentFile
                            .set(Some(Arc::new(PrintingSdFileInformation::new(
                                &name,
                                Some(size),
                                user.as_deref(),
                            ))));
                    } else if line.contains("File selected") {
                        if self._ignore_select.get() {
                            self._ignore_select.set(false);
                        } else if self._currentFile.get().is_some() && self.is_sd_file_selected() {
                            // final answer to M23, at least on Marlin, Repetier and Sprinter: "File selected"
                            let file = self.current_file();
                            let data = self
                                ._sdFiles
                                .with(|d| d.get(&file.get_filename()).cloned())
                                .map(|d| {
                                    serde_json::json!([
                                        d.name,
                                        d.size,
                                        d.timestamp,
                                        d.longname.map_or(Value::Bool(false), Value::from)
                                    ])
                                });
                            self._callback.on_comm_file_selected(
                                Some(&file.get_filename()),
                                file.get_filesize(),
                                true,
                                file.get_user().as_deref(),
                                data.as_ref(),
                            );
                        }
                    } else if line.contains("Writing to file") && self.is_streaming() {
                        self._change_state(Self::STATE_PRINTING);
                    } else if line.contains("Done printing file") && self.is_sd_printing() {
                        // printer is reporting file finished printing
                        self._change_state(Self::STATE_FINISHING);

                        self.current_file().set_done(true);
                        self.current_file().set_pos(0);

                        self.send_command(
                            QueuedCommand::Command("M400".to_string()),
                            None,
                            true,
                            false,
                            false,
                            None,
                            None,
                        );
                        self._callback.on_comm_print_job_done(false);

                        // def finalize(): self._changeState(self.STATE_OPERATIONAL)
                        let me = self.clone();
                        let finalize = move || me._change_state(Self::STATE_OPERATIONAL);

                        self.send_command(
                            QueuedCommand::Marker(SendQueueMarker::new(Some(Box::new(finalize))).0),
                            None,
                            true,
                            false,
                            false,
                            None,
                            None,
                        );
                        self._continue_sending();
                    } else if line.contains("Done saving file") {
                        if truthy(self._trigger_ok_for_m29) {
                            // workaround for most versions of Marlin out in the wild
                            // not sending an ok after saving a file
                            self._handle_ok();
                        }
                    } else if line.contains("File deleted") && py_strip(&line).ends_with("ok") {
                        // buggy Marlin version that doesn't send a proper line break after the "File deleted" statement, fixed in
                        // current versions
                        self._handle_ok();
                    }
                }

                //~~ Message handling
                self._callback.on_comm_message(&line);

                //~~ Parsing for feedback commands
                if !feedback_controls.is_empty()
                    && feedback_matcher.is_some()
                    && !feedback_errors.iter().any(|e| e == "_all")
                {
                    let matcher = feedback_matcher.as_ref().expect("checked above");
                    if let Err(_) = py_try(|| {
                        self._process_registered_message(
                            &line,
                            matcher,
                            &feedback_controls,
                            &mut feedback_errors,
                        )
                    }) {
                        // something went wrong while feedback matching
                        // self._logger.exception("Error while trying to apply feedback control matching, disabling it")
                        feedback_errors.push("_all".to_string());
                    }
                }

                //~~ Parsing for pause triggers
                if !pause_triggers.is_empty() && !self.is_streaming() {
                    if pause_triggers
                        .get("enable")
                        .is_some_and(|r| re_search(r, &line))
                    {
                        self.set_pause(true, true, None, None);
                    } else if pause_triggers
                        .get("disable")
                        .is_some_and(|r| re_search(r, &line))
                    {
                        self.set_pause(false, true, None, None);
                    } else if pause_triggers
                        .get("toggle")
                        .is_some_and(|r| re_search(r, &line))
                    {
                        self.set_pause(!self.is_paused(), true, None, None);
                    }
                }

                //# Serial detection
                if self._state.get() == Self::STATE_DETECT_SERIAL {
                    if line.is_empty()
                        || time_monotonic()
                            > self._ok_timeout.get().expect(
                                "TypeError: '>' not supported between 'float' and 'NoneType'",
                            )
                    {
                        self._perform_detection_step(false);
                    } else if line.contains("start") || line.starts_with("ok") {
                        self._on_connected();
                        if line.contains("start") {
                            self._clear_to_send.set();
                        }
                    }
                }
                //# Connection attempt
                else if self._state.get() == Self::STATE_CONNECTING {
                    if line.contains("start") && !start_seen {
                        start_seen = true;
                        self.say_hello(None);
                    } else if line.starts_with("ok") || (support_wait && line == "wait") {
                        if line == "wait" {
                            // if it was a wait we probably missed an ok, so let's simulate that now
                            self._handle_ok();
                        }
                        self._on_connected();
                    } else if time_monotonic()
                        > self
                            ._timeout
                            .get()
                            .expect("TypeError: '>' not supported between 'float' and 'NoneType'")
                    {
                        if try_hello && self._hello_sent.get() < 3 {
                            self._log("No answer from the printer within the connection timeout, trying another hello");
                            self.say_hello(None);
                        } else {
                            self._log("There was a timeout while trying to connect to the printer");
                            self.close(false, false, Some(10.0));
                        }
                    }
                }
                //# Operational (idle or busy)
                else if [
                    Self::STATE_OPERATIONAL,
                    Self::STATE_STARTING,
                    Self::STATE_PRINTING,
                    Self::STATE_PAUSED,
                    Self::STATE_TRANSFERING_FILE,
                ]
                .contains(&self._state.get())
                {
                    if line == "start" {
                        // exact match, to be on the safe side
                        let _hold = self
                            .job_put_on_hold(true)
                            .expect("RuntimeError: job_put_on_hold");
                        let idle = self._state.get() == Self::STATE_OPERATIONAL;
                        if idle {
                            let message = "Printer sent 'start' while already operational. External reset? Resetting line numbers to be on the safe side";
                            self._log(message);
                            // self._logger.warning(message)

                            self._on_external_reset();
                        } else {
                            let verb = if self.is_streaming() {
                                "streaming to SD"
                            } else {
                                "printing"
                            };
                            let message = format!(
                                "Printer sent 'start' while {verb}. External reset? Aborting job since printer lost state."
                            );
                            self._log(&message);
                            // self._logger.warning(message)

                            self._on_external_reset();
                            self.cancel_print(None, true, None, None, false);
                        }

                        event_manager().fire(
                            Events::PRINTER_RESET,
                            Some(serde_json::json!({"idle": idle})),
                        );
                    }
                }

                Flow::Next
            });

            match flow {
                Ok(Flow::Break) => break,
                Ok(Flow::Next) => {}
                Err(_) => {
                    // except Exception:
                    // self._logger.exception("Something crashed inside the serial connection loop, please report this in OctoPrint's bug tracker:")

                    let error_msg = "See octoprint.log for details";
                    self._log(error_msg);
                    self._errorValue.set(error_msg.to_string());
                    event_manager().fire(
                        Events::ERROR,
                        Some(serde_json::json!({"error": self.get_error_string(), "reason": "crash"})),
                    );
                    self.close(true, true, Some(10.0));
                }
            }
        }
        self._dual_log("Connection closed, closing down monitor", LOGGING_INFO, "");
    }

    // ~~ comm.py:3308-4508 (section H)

    /// comm.py:3308 `_handle_ok()`
    pub fn _handle_ok(self: &Arc<Self>) {
        if let Some(timer) = self._resend_ok_timer.get() {
            timer.cancel();
            self._resend_ok_timer.set(None);
        }

        self._ok_timeout
            .set(Some(self._get_new_communication_timeout()));
        self._clear_to_send.set();

        // reset long running commands, persisted current tools and heatup counters on ok

        self._long_running_command.set(false);

        if let Some(tool) = self._toolBeforeHeatup.get() {
            self._currentTool.set(tool);
            self._toolBeforeHeatup.set(None);
        }

        self._finish_heatup();

        if !Self::OPERATIONAL_STATES.contains(&self._state.get()) {
            return;
        }

        if self._resendDelta.get().is_some() && self._resend_next_command(false) {
            // we processed a resend request and are done here
            return;
        }

        // continue with our queues and the job
        self._resendActive.set(false);
        self._continue_sending();
    }

    /// comm.py:3337 `_handle_timeout()`
    pub fn _handle_timeout(self: &Arc<Self>) {
        if !Self::OPERATIONAL_STATES.contains(&self._state.get()) {
            return;
        }

        let general_message = "Configure long running commands or increase communication timeout if that happens regularly on specific commands or long moves.";

        // figure out which consecutive timeout maximum we have to use
        let maximum = |key: &str| {
            self._consecutive_timeout_maximums
                .with(|d| d.get(key).copied())
                .unwrap_or(0)
        };
        let consecutive_max = if self._long_running_command.get() {
            maximum("long")
        } else if Self::PRINTING_STATES.contains(&self._state.get()) {
            maximum("printing")
        } else {
            maximum("idle")
        };

        // now increment the timeout counter
        self._consecutive_timeouts
            .set(self._consecutive_timeouts.get() + 1);
        // self._logger.debug(f"Now at {self._consecutive_timeouts} consecutive timeouts")

        if 0 < consecutive_max && consecutive_max < self._consecutive_timeouts.get() {
            // too many consecutive timeouts, we give up
            let message = format!(
                "No response from printer after {} consecutive communication timeouts, considering it dead.",
                consecutive_max + 1
            );
            // self._logger.info(message)
            self._log(&format!("{message} {general_message}"));
            self._errorValue.set(
                "Too many consecutive timeouts, printer still connected and alive?".to_string(),
            );
            event_manager().fire(
                Events::ERROR,
                Some(serde_json::json!({"error": self._errorValue.get(), "reason": "timeout"})),
            );
            self.close(true, true, Some(10.0));
        } else if self._resendActive.get() {
            // resend active, resend same command instead of triggering a new one
            let message = "Communication timeout during an active resend, resending same line again to trigger response from printer.";
            // self._logger.info(message)
            self._log(&format!("{message} {general_message}"));
            if self._resend_same_command() {
                self._clear_to_send.set();
            }
        } else if self._heating.get() {
            // blocking heatup active, consider that finished
            // self._logger.info("Timeout while in an active heatup, considering heatup to be over.")
            self._finish_heatup();
        } else if self._long_running_command.get() {
            // long running command active, ignore timeout
            // self._logger.debug("Ran into a communication timeout, but a command known to be a long runner is currently active")
        } else if Self::PRINTING_STATES.contains(&self._state.get())
            || self._state.get() == Self::STATE_PAUSED
        {
            // printing, try to tickle the printer
            let message =
                "Communication timeout while printing, trying to trigger response from printer.";
            // self._logger.info(message)
            self._log(&format!("{message} {general_message}"));
            if self._send_command(
                QueuedCommand::Command("M105".to_string()),
                Some("temperature"),
                None,
                Some(with_tags(
                    &BTreeSet::new(),
                    &["trigger:comm.handle_timeout"],
                )),
            ) {
                self._clear_to_send.set();
            }
        } else if self._clear_to_send.blocked() {
            // timeout while idle and no oks left, let's try to tickle the printer
            let message =
                "Communication timeout while idle, trying to trigger response from printer.";
            // self._logger.info(message)
            self._log(&format!("{message} {general_message}"));
            self._clear_to_send.set();
        }
    }

    /// comm.py:3407 `_perform_detection_step(init=False)`
    pub fn _perform_detection_step(self: &Arc<Self>, init: bool) {
        let log = |message: &str| {
            self._log(message);
            // self._logger.info(f"Serial detection: {message}")
        };

        if init {
            let port = self._port.clone();
            let baudrate = self._baudrate;

            let port_candidates: Vec<String> = match &port {
                Some(p) if !p.is_empty() && p != "AUTO" => vec![p.clone()],
                _ => serial_list(),
            };

            let baudrate_candidates: Vec<i64> = if baudrate != 0 {
                vec![baudrate]
            } else if port_candidates.len() == 1 {
                baudrate_list(None)
            } else {
                // if we have no baudrate and more than one port we limit tested baudrates to
                // the two most common plus any additionally configured ones
                baudrate_list(Some(vec![115200, 250000]))
            };

            self._detection_candidates.set(
                port_candidates
                    .iter()
                    .flat_map(|p| baudrate_candidates.iter().map(move |b| (p.clone(), *b)))
                    .collect(),
            );
            self._detection_retry.set(Self::DETECTION_RETRIES);

            let candidates = self._detection_candidates.get();
            log(&format!(
                "Performing autodetection with {} port/baudrate candidates: {}",
                candidates.len(),
                candidates
                    .iter()
                    .map(|x| format!("{}@{}", x.0, x.1))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        let attempt_handshake = || {
            self._detection_retry.set(self._detection_retry.get() + 1);
            let timeout = self._get_communication_timeout_interval();

            log(&format!(
                "Handshake attempt #{} with timeout {}s",
                self._detection_retry.get(),
                py_float_repr(timeout)
            ));
            let result = py_try(|| {
                let serial = self
                    ._serial
                    .get()
                    .expect("AttributeError: 'NoneType' object has no attribute 'timeout'");
                if serial.timeout() != Some(timeout) {
                    serial.set_timeout(timeout);
                }
                self._timeout.set(Some(time_monotonic() + timeout));
                self._ok_timeout.set(Some(time_monotonic() + timeout));
            });
            match result {
                Err(text) => {
                    self._log(&format!(
                        "Unexpected error while setting timeout {}: {}",
                        py_float_repr(timeout),
                        exception_string_of_panic(
                            &text,
                            EXCEPTION_STRING_FMT,
                            "_perform_detection_step",
                            line!()
                        )
                    ));
                    // self._logger.exception(f"Unexpected error while setting timeout {timeout}")
                }
                Ok(()) => {
                    self._do_send_without_checksum(b"", false); // new line to reset things
                    self.say_hello(Some(with_tags(&BTreeSet::new(), &["trigger:detection"])));
                }
            }
        };

        while !self._detection_candidates.get().is_empty()
            || self._detection_retry.get() < Self::DETECTION_RETRIES
        {
            if self._state.get() != Self::STATE_DETECT_SERIAL {
                return;
            }

            if self._detection_retry.get() < Self::DETECTION_RETRIES {
                if self._serial.get().is_none() {
                    self._detection_retry.set(Self::DETECTION_RETRIES);
                    continue;
                }

                attempt_handshake();
                return;
            } else {
                let (p, b) = self._detection_candidates.with_mut(|c| c.remove(0));

                // `continue` is `Ok(false)`, `return` is `Ok(true)`
                let step = py_try(|| -> bool {
                    log(&format!("Trying port {p}, baudrate {b}"));
                    let serial = self._serial.get();
                    let other_port = match &serial {
                        None => true,
                        Some(s) => {
                            s.port()
                                .expect("AttributeError: the serial object has no attribute 'port'")
                                != Some(p.clone())
                        }
                    };
                    if other_port {
                        if !self._open_serial(&p, b, false) {
                            log(&format!("Could not open port {p}, baudrate {b}, skipping"));
                            return false;
                        }
                    } else {
                        serial.expect("checked above").set_baudrate(b);
                    }

                    self._detection_retry.set(0);

                    attempt_handshake();
                    true
                });
                match step {
                    Ok(true) => return,
                    Ok(false) => continue,
                    Err(text) => {
                        self._log(&format!(
                            "Unexpected error while setting baudrate {b}: {}",
                            exception_string_of_panic(
                                &text,
                                EXCEPTION_STRING_FMT,
                                "_perform_detection_step",
                                line!()
                            )
                        ));
                        // self._logger.exception(f"Unexpected error while setting baudrate {b}")
                    }
                }
            }
        }

        let error_text =
            "No more candidates to test, and no working port/baudrate combination detected.";
        self._trigger_error(error_text, "autodetect", true);
    }

    /// comm.py:3519 `_finish_heatup()`
    pub fn _finish_heatup(&self) {
        if self._heating.get() {
            if self._heatupWaitStartTime.get().is_some_and(|t| t != 0.0) {
                self._heatupWaitTimeLost.set(
                    self._heatupWaitTimeLost.get()
                        + (time_monotonic()
                            - self
                                ._heatupWaitStartTime
                                .get()
                                .expect("TypeError: unsupported operand type(s) for -: 'float' and 'NoneType'")),
                );
                self._heatupWaitStartTime.set(None);
            }
            self._heating.set(false);
        }
    }

    /// comm.py:3528 `_continue_sending()`: `Some(true)` / `Some(false)` is Python's `True` /
    /// `False`, `None` its `None` (the loop was not entered, or ran out).
    pub fn _continue_sending(self: &Arc<Self>) -> Option<bool> {
        // Ensure we have at least one line in the send queue, but don't spam it
        while self._active() && self._send_queue.qsize() == 0 {
            let job_active = (self._state.get() == Self::STATE_STARTING
                || self._state.get() == Self::STATE_PRINTING)
                && !(self._currentFile.get().is_none_or(|f| f.done()) || self.is_sd_printing());

            if self._send_from_command_queue() {
                // we found something in the command queue to send
                return Some(true);
            } else if self.job_on_hold() {
                // job is on hold, that means we must not send from either job queue or file
                return Some(false);
            } else if self._send_from_job_queue() {
                // we found something in the job queue to send
                return Some(true);
            } else if job_active && self._send_from_job() {
                // we sent the next line from the file
                return Some(true);
            } else if !job_active {
                // nothing sent but also no job active, so we can just return false
                return Some(false);
            }

            // self._logger.debug("No command enqueued on ok while printing, doing another iteration")
        }
        None
    }

    /// comm.py:3562 `_process_registered_message(line, feedback_matcher, feedback_controls, feedback_errors)`
    ///
    /// OURS: `template.format(...)` is `py_format::format` (py_format.rs); a template that is
    /// `bytes` has no `.format` (`AttributeError`, caught: `output = None`).
    pub fn _process_registered_message(
        &self,
        line: &str,
        feedback_matcher: &Regex,
        feedback_controls: &FeedbackControls,
        feedback_errors: &mut Vec<String>,
    ) {
        let Some(feedback_match) = re_captures(feedback_matcher, line) else {
            return;
        };

        // `feedback_match.groupdict()`: every named group, in the order of the pattern
        for match_key in feedback_matcher.capture_names().flatten() {
            let result = py_try(|| {
                let feedback_key: String = match_key.chars().skip("group".len()).collect();
                let group = feedback_match.name(match_key);
                let (Some(control), Some(group)) = (feedback_controls.get(&feedback_key), group)
                else {
                    return;
                };
                if feedback_errors.iter().any(|e| e == &feedback_key) {
                    return;
                }
                let matched_part = group.as_str();

                let Some(matcher) = &control.matcher else {
                    return;
                };

                let Some(m) = re_captures(matcher, matched_part) else {
                    return;
                };

                // `match.groups()` and `match.groupdict()`
                let groups: Vec<Option<String>> = (1..m.len())
                    .map(|i| m.get(i).map(|g| g.as_str().to_string()))
                    .collect();
                let groupdict: Vec<(String, Option<String>)> = matcher
                    .capture_names()
                    .flatten()
                    .map(|n| (n.to_string(), m.name(n).map(|g| g.as_str().to_string())))
                    .collect();

                let mut outputs = serde_json::Map::new();
                for (template_key, template) in control.templates.iter() {
                    let output: Option<String> = match template {
                        PyText::Bytes(_) => None, // AttributeError: 'bytes' object has no attribute 'format'
                        PyText::Str(template) => match py_format::format(template, &groups, &[]) {
                            Ok(o) => Some(o),
                            Err(py_format::FormatError::KeyError(_)) => {
                                match py_format::format(template, &[], &groupdict) {
                                    Ok(o) => Some(o),
                                    Err(_) => None, // an exception raised in the `except KeyError:` block is not caught there
                                }
                            }
                            Err(_) => {
                                // self._logger.debug("Could not process template {}: {}".format(template_key, template), exc_info=1)
                                None
                            }
                        },
                    };

                    if let Some(output) = output {
                        outputs.insert(template_key.clone(), Value::String(output));
                    }
                }
                event_manager().fire(
                    Events::REGISTERED_MESSAGE_RECEIVED,
                    Some(serde_json::json!({"key": feedback_key, "matched": matched_part, "outputs": Value::Object(outputs)})),
                );
            });
            if result.is_err() {
                // self._logger.exception("Error while trying to match feedback control output, disabling key {key}")
                feedback_errors.push(match_key.to_string());
            }
        }
    }

    /// comm.py:3618 `_poll_temperature()`
    ///
    /// Polls the temperature.
    ///
    /// If the printer is not operational, capable of auto-reporting temperatures, closing the connection, not printing
    /// from sd, busy with a long running command or heating, no poll will be done.
    pub fn _poll_temperature(self: &Arc<Self>) {
        if self.is_operational()
            && !self._temperature_autoreporting.get()
            && !self._connection_closing.get()
            && !self.is_streaming()
            && !self._long_running_command.get()
            && !self._heating.get()
            && !self._dwelling_until.get().is_some_and(|d| d != 0.0)
            && !self._manualStreaming.get()
        {
            self.send_command(
                QueuedCommand::Command("M105".to_string()),
                Some("temperature_poll"),
                false,
                false,
                false,
                None,
                Some(with_tags(
                    &BTreeSet::new(),
                    &["trigger:comm.poll_temperature"],
                )),
            );
        }
    }

    /// comm.py:3642 `_poll_sd_status()`
    ///
    /// Polls the sd printing status.
    ///
    /// If the printer is not operational, closing the connection, not printing from sd, busy with a long running
    /// command or heating, no poll will be done.
    pub fn _poll_sd_status(self: &Arc<Self>) {
        if self.is_operational()
            && !self._sdstatus_autoreporting.get()
            && !self._connection_closing.get()
            && ((self.is_sd_file_selected() && !truthy(self._disable_sd_printing_detection.get()))
                || self.is_sd_printing())
            && !self._long_running_command.get()
            && !self._dwelling_until.get().is_some_and(|d| d != 0.0)
            && !self._heating.get()
        {
            self.send_command(
                QueuedCommand::Command("M27".to_string()),
                Some("sd_status_poll"),
                false,
                false,
                false,
                None,
                Some(with_tags(
                    &BTreeSet::new(),
                    &["trigger:comm.poll_sd_status"],
                )),
            );
        }
    }

    /// `int(self._timeout_intervals.get(key, default))` inside a `try: ... except Exception:`:
    /// `None` is the exception (`int()` of a NaN or an infinity).
    fn timeout_interval_int(&self, key: &str, default: f64) -> Option<i64> {
        let v = self
            ._timeout_intervals
            .with(|d| d.get(key).copied())
            .unwrap_or(default);
        if v.is_finite() { Some(v as i64) } else { None }
    }

    /// comm.py:3667 `_set_autoreport_temperature_interval(interval=None)`
    pub fn _set_autoreport_temperature_interval(self: &Arc<Self>, interval: Option<i64>) {
        let interval = match interval {
            None => self
                .timeout_interval_int("temperatureAutoreport", 2.0)
                .unwrap_or(2),
            Some(i) => i,
        };
        self.send_command(
            QueuedCommand::Command(format!("M155 S{interval}")),
            None,
            false,
            false,
            false,
            None,
            Some(with_tags(
                &BTreeSet::new(),
                &["trigger:comm.set_autoreport_temperature_interval"],
            )),
        );
    }

    /// comm.py:3678 `_set_autoreport_sdstatus_interval(interval=None)`
    pub fn _set_autoreport_sdstatus_interval(self: &Arc<Self>, interval: Option<i64>) {
        let interval = match interval {
            None => self
                .timeout_interval_int("sdStatusAutoreport", 1.0)
                .unwrap_or(1),
            Some(i) => i,
        };
        self.send_command(
            QueuedCommand::Command(format!("M27 S{interval}")),
            None,
            false,
            false,
            false,
            None,
            Some(with_tags(
                &BTreeSet::new(),
                &["trigger:comm.set_autoreport_sdstatus_interval"],
            )),
        );
    }

    /// comm.py:3689 `_set_autoreport_pos_interval(interval=None, part_of_job=False, tags=None)`
    pub fn _set_autoreport_pos_interval(
        self: &Arc<Self>,
        interval: Option<i64>,
        part_of_job: bool,
        tags: Option<BTreeSet<String>>,
    ) {
        let tags = tags.unwrap_or_default();
        let interval = match interval {
            None => self.timeout_interval_int("posAutoreport", 5.0).unwrap_or(5),
            Some(i) => i,
        };
        self.send_command(
            QueuedCommand::Command(format!("M154 S{interval}")),
            None,
            part_of_job,
            false,
            false,
            None,
            Some(with_tags(
                &tags,
                &["trigger:comm.set_autoreport_pos_interval"],
            )),
        );
    }

    /// comm.py:3703 `_set_busy_protocol_interval(interval=None, callback=None)`
    pub fn _set_busy_protocol_interval(
        self: &Arc<Self>,
        interval: Option<i64>,
        callback: Option<OnSent>,
    ) {
        let interval = match interval {
            None => match self.timeout_interval_int("communicationBusy", 3.0) {
                Some(i) => (i - 1).max(1),
                None => 2,
            },
            Some(i) => i,
        };
        self.send_command(
            QueuedCommand::Command(format!("M113 S{interval}")),
            None,
            false,
            false,
            false,
            callback,
            Some(with_tags(
                &BTreeSet::new(),
                &["trigger:comm.set_busy_protocol_interval"],
            )),
        );
    }

    /// comm.py:3717 `_onConnected()`
    pub fn _on_connected(self: &Arc<Self>) {
        self._serial
            .get()
            .expect("AttributeError: 'NoneType' object has no attribute 'timeout'")
            .set_timeout(self._get_communication_timeout_interval());

        let (me, me2) = (self.clone(), self.clone());
        let timer = Arc::new(RepeatedTimer::new(
            Interval::Callable(Arc::new(move || me._get_temperature_timer_interval())),
            Arc::new(move || me2._poll_temperature()),
            true,
            None,
            None,
            None,
            None,
            true,
        ));
        self._temperature_timer.set(Some(timer.clone()));
        timer
            .start()
            .expect("RuntimeError: threads can only be started once");

        let (me, me2) = (self.clone(), self.clone());
        let timer = Arc::new(RepeatedTimer::new(
            Interval::Callable(Arc::new(move || me._get_sd_status_timer_interval())),
            Arc::new(move || me2._poll_sd_status()),
            true,
            None,
            None,
            None,
            None,
            true,
        ));
        self._sd_status_timer.set(Some(timer.clone()));
        timer
            .start()
            .expect("RuntimeError: threads can only be started once");

        self._change_state(Self::STATE_OPERATIONAL);

        self.reset_line_numbers(
            0,
            false,
            Some(with_tags(&BTreeSet::new(), &["trigger:comm.on_connected"])),
        );
        self.send_command(
            QueuedCommand::Command("M115".to_string()),
            None,
            false,
            false,
            false,
            None,
            Some(with_tags(&BTreeSet::new(), &["trigger:comm.on_connected"])),
        );

        if self._sdAvailable.get() {
            self.refresh_sd_files(
                Some(with_tags(&BTreeSet::new(), &["trigger:comm.on_connected"])),
                false,
                Some(10.0),
            );
        } else {
            self.init_sd_card(Some(with_tags(
                &BTreeSet::new(),
                &["trigger:comm.on_connected"],
            )));
        }

        let payload = serde_json::json!({"port": self._port, "baudrate": self._baudrate});
        event_manager().fire(Events::CONNECTED, Some(payload.clone()));
        self.send_gcode_script(
            "afterPrinterConnected",
            Some(&serde_json::json!({"event": payload})),
            None,
            false,
        );
    }

    /// comm.py:3752 `_on_external_reset()`
    pub fn _on_external_reset(self: &Arc<Self>) {
        // hold queue processing, clear queues and acknowledgements, reset line number and last lines
        {
            let _send_queue_blocked = self._send_queue.blocked();
            self._clear_to_send.reset();
            {
                let _command_queue_blocked = self._command_queue.blocked();
                self._command_queue.clear();
            }
            self._send_queue.clear();

            {
                let _line_mutex = self._line_mutex.lock();
                self._current_line.set(0);
                self._lastLines.with_mut(|l| l.clear());
            }
        }

        self.say_hello(Some(with_tags(
            &BTreeSet::new(),
            &["trigger:comm.on_external_reset"],
        )));
        self.reset_line_numbers(
            0,
            false,
            Some(with_tags(
                &BTreeSet::new(),
                &["trigger:comm.on_external_reset"],
            )),
        );

        if self._temperature_autoreporting.get() {
            self._set_autoreport_temperature_interval(None);
        }
        if self._sdstatus_autoreporting.get() {
            self._set_autoreport_sdstatus_interval(None);
        }
        if self._pos_autoreporting.get() {
            self._set_autoreport_pos_interval(None, false, None);
        }
        if self._busy_protocol_support.get() {
            self._set_busy_protocol_interval(None, None);
        }

        self._consecutive_not_sd_printing.set(0);
    }

    /// `self._timeout_intervals.get(key, default)`
    fn timeout_interval(&self, key: &str, default: f64) -> f64 {
        self._timeout_intervals
            .with(|d| d.get(key).copied())
            .unwrap_or(default)
    }

    /// comm.py:3778 `_get_temperature_timer_interval()`
    pub fn _get_temperature_timer_interval(&self) -> f64 {
        let busy_default = 4.0;
        let target_default = 2.0;

        let get = |key: &str, default: f64| {
            let mut interval = self.timeout_interval(key, default);
            if interval <= 0.0 {
                interval = 1.0;
            }
            interval
        };

        if self.is_busy() {
            return get("temperature", busy_default);
        }

        let tools = self.last_temperature.with(|t| t.tools());
        for temp in tools.iter().map(|(_, t)| t.1) {
            if temp.is_some_and(|t| t != 0.0 && t > self._temperatureTargetSetThreshold as f64) {
                return get("temperatureTargetSet", target_default);
            }
        }

        let bed = self.last_temperature.with(|t| t.bed());
        if bed
            .1
            .is_some_and(|t| t > self._temperatureTargetSetThreshold as f64)
        {
            return get("temperatureTargetSet", target_default);
        }

        get("temperature", busy_default)
    }

    /// comm.py:3807 `_get_sd_status_timer_interval()`
    pub fn _get_sd_status_timer_interval(&self) -> f64 {
        let mut interval = self.timeout_interval("sdStatus", 1.0);
        if interval <= 0.0 {
            interval = 1.0;
        }
        interval
    }

    /// comm.py:3813 `_get_communication_timeout_interval()`
    pub fn _get_communication_timeout_interval(&self) -> f64 {
        // special rules during serial detection
        if self._state.get() == Self::STATE_DETECT_SERIAL {
            if self._detection_retry.get() == 0 {
                // first try
                return self.timeout_interval("detectionFirst", 10.0);
            } else {
                // consecutive tries
                return self.timeout_interval("detectionConsecutive", 2.0);
            }
        }

        // communication timeout
        let comm_timeout = if self._busy_protocol_support.get() {
            self.timeout_interval("communicationBusy", 2.0)
        } else {
            self.timeout_interval("communication", 30.0)
        };

        // temperature interval
        let temperature_timeout = if self._temperature_autoreporting.get() {
            self.timeout_interval("temperatureAutoreport", 2.0)
        } else {
            self._get_temperature_timer_interval()
        };

        // use the max of both, add a second to temperature to avoid race conditions
        py_max(comm_timeout, temperature_timeout + 1.0)
    }

    /// comm.py:3840 `_get_new_communication_timeout()`
    pub fn _get_new_communication_timeout(&self) -> f64 {
        time_monotonic() + self._get_communication_timeout_interval()
    }

    /// comm.py:3843 `_send_from_command_queue()`
    pub fn _send_from_command_queue(self: &Arc<Self>) -> bool {
        // We loop here to make sure that if we do NOT send the first command
        // from the queue, we'll send the second (if there is one). We do not
        // want to get stuck here by throwing away commands.
        loop {
            if self.is_streaming() {
                // command queue irrelevant
                return false;
            }

            let entry = match self._command_queue.get(false, None) {
                Ok(entry) => entry,
                Err(GetError::Empty) => {
                    // nothing in command queue
                    return false;
                }
                Err(e) => panic!("CommandQueue.get raised {e:?}"),
            };

            // try: ... finally: self._command_queue.task_done()
            let sent = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // `isinstance(entry, tuple)` and `len(entry) == 4` always hold (difference 13)
                let (cmd, cmd_type, callback, tags) = entry;
                self._send_command(cmd, cmd_type.as_deref(), callback, Some(tags))
            }));
            self._command_queue
                .task_done()
                .expect("ValueError: task_done() called too many times");
            match sent {
                Ok(true) => {
                    // we actually did add this cmd to the send queue, so let's
                    // return, we are done here
                    return true;
                }
                Ok(false) => {}
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
    }

    /// comm.py:3878 `_open_serial(port, baudrate, trigger_errors=True)`
    ///
    /// The serial factory hooks come first (`octoprint.comm.transport.serial.factory`; none are
    /// registered, plugin.rs), the `default` factory last. OURS: a hook that returns a serial
    /// object is `unimplemented!` (a hook returns a JSON value); one that returns `None` passes
    /// on to the next.
    pub fn _open_serial(self: &Arc<Self>, port: &str, baudrate: i64, trigger_errors: bool) -> bool {
        #[cfg(test)]
        test_guard_open_serial(port);
        let timeout = settings().get_float(&["serial", "timeout", "connection"]);

        // `exception_string` of the factory that failed (`get_exception_string()`), the name of
        // its hook
        let failed = |name: &str, exception_string: String| -> bool {
            if trigger_errors {
                self._trigger_error("Connection error, see Terminal tab", "connection", true);
            }

            let error_message = format!(
                "Unexpected error while connecting to serial port {port}, baudrate {baudrate} from hook {name}: {exception_string}"
            );
            self._log(&error_message);
            // self._logger.exception(error_message)
            false
        };

        let succeeded = |serial_obj: Arc<dyn SerialTransport>| -> bool {
            // first hook to succeed wins, but any can pass on to the next
            self._serial.set(Some(serial_obj));
            self._clear_to_send.reset();
            true
        };

        // OURS (test seam): a registered fake printer
        #[cfg(test)]
        {
            let fake = TEST_TRANSPORTS
                .plock()
                .iter()
                .find(|(p, _)| p == port)
                .map(|(_, t)| t.clone());
            if let Some(fake) = fake {
                return succeeded(fake);
            }
        }

        for (name, factory) in self._serial_factory_hooks.iter() {
            let args = [
                Value::from(port),
                Value::from(baudrate),
                serde_json::json!(timeout),
            ];
            match py_try(|| factory(&args)) {
                Err(text) => {
                    return failed(
                        name,
                        exception_string_of_panic(
                            &text,
                            EXCEPTION_STRING_FMT,
                            "_open_serial",
                            line!(),
                        ),
                    );
                }
                Ok(Value::Null) => {}
                Ok(_) => {
                    unimplemented!("plugin.rs: a serial factory hook that returns a serial object")
                }
            }
        }

        match self._open_serial_default(port, baudrate, timeout) {
            Err(e) => failed(
                "default",
                get_exception_string_of(
                    &ExceptionInfo {
                        type_name: &e.type_name,
                        message: &e.message,
                        file: "comm.rs",
                        function: "_open_serial",
                        line: line!(),
                    },
                    EXCEPTION_STRING_FMT,
                ),
            ),
            Ok(Some(serial_obj)) => succeeded(serial_obj),
            Ok(None) => false,
        }
    }

    /// comm.py:3879 `default(_, p, b, timeout)`, the factory nested in `_open_serial`: "connect to
    /// regular serial port". `Ok(None)` is Python's `return None`; `Err` an exception that is no
    /// `serial.SerialException`.
    fn _open_serial_default(
        &self,
        port: &str,
        baudrate: i64,
        timeout: Option<f64>,
    ) -> Result<Option<Arc<dyn SerialTransport>>, SerialError> {
        self._dual_log(
            &format!("Connecting to port {port}, baudrate {baudrate}"),
            LOGGING_INFO,
            "",
        );

        // `serial_port_args`: baudrate, timeout, write_timeout=0, and exclusive=True when set
        let exclusive = if truthy(settings().get_boolean(&["serial", "exclusive"])) {
            Some(true)
        } else {
            None
        };

        // try: ... except serial.SerialException: return None
        let opened = (|| -> Result<serial::Serial, SerialError> {
            let serial_obj = serial::Serial::new(baudrate, timeout, Some(0.0), exclusive)?;
            serial_obj.set_port(port);

            let use_parity_workaround = settings().get(&["serial", "useParityWorkaround"]);
            let needs_parity_workaround = platform::get_os() == "linux"
                && std::path::Path::new("/etc/debian_version").exists(); // See #673

            let always = use_parity_workaround.as_ref().and_then(Value::as_str) == Some("always");
            let detect = use_parity_workaround.as_ref().and_then(Value::as_str) == Some("detect");
            if always || (needs_parity_workaround && detect) {
                serial_obj.set_parity_odd(true);
                serial_obj.open()?;
                serial_obj.close();
                serial_obj.set_parity_odd(false);
            }

            serial_obj.open()?;
            Ok(serial_obj)
        })();
        let serial_obj = match opened {
            Ok(s) => s,
            Err(e) if e.serial_exception => {
                // self._logger.info(f"Failed to connect: Port {p} is busy or does not exist")
                return Ok(None);
            }
            Err(e) => return Err(e),
        };

        // Set close_exec flag on serial handle, see #3212
        #[cfg(unix)]
        if let Some(fd) = serial_obj.fd() {
            // posix
            platform::set_close_exec(fd)
                .map_err(|e| SerialError::other("OSError", e.to_string()))?;
        }
        #[cfg(windows)]
        if let Some(handle) = serial_obj.port_handle() {
            // win32: `raise ctypes.GetLastError()` raises an int, which is a `TypeError`
            platform::set_close_exec(handle as isize).map_err(|_| {
                SerialError::other("TypeError", "exceptions must derive from BaseException")
            })?;
        }

        let wrapper = Arc::new(serial::BufferedReadlineWrapper::new(serial_obj));

        if truthy(settings().get_boolean(&["serial", "lowLatency"])) {
            match wrapper.set_low_latency_mode(true) {
                Some(Err(_)) => {
                    // self._logger.exception("Could not set low latency mode on serial port, continuing without")
                }
                Some(Ok(())) => {}
                None => {
                    // self._logger.info("Platform doesn't support low latency mode on serial port")
                }
            }
        }

        Ok(Some(wrapper))
    }

    /// comm.py:3981 `_recoverable_communication_errors`
    pub const _recoverable_communication_errors: [&'static str; 2] =
        ["no line number with checksum", "missing linenumber"];
    /// comm.py:3985 `_resend_request_communication_errors`
    pub const _resend_request_communication_errors: [&'static str; 5] = [
        "line number", // since this error class gets checked after recoverable
        // communication errors, we can use this broad term here
        "linenumber", // since this error class gets checked after recoverable
        // communication errors, we can use this broad term here
        "checksum", // since this error class gets checked after recoverable
        // communication errors, we can use this broad term here
        "format error",
        "expected line",
    ];
    /// comm.py:3995 `_sd_card_errors`
    pub const _sd_card_errors: [&'static str; 7] = [
        "volume.init",
        "openroot",
        "workdir",
        "error writing to file",
        "cannot open",
        "open failed",
        "cannot enter",
    ];
    /// comm.py:4004 `_fatal_errors`
    pub const _fatal_errors: [&'static str; 2] = ["kill() called", "fatal:"];

    /// comm.py:4006 `_handle_errors(line)`: returns the line (`None` for a `None` line)
    pub fn _handle_errors(self: &Arc<Self>, line: Option<&str>) -> Option<String> {
        let line = line?;
        let mut line = line.to_string();

        let mut lower_line = line.to_lowercase();

        if lower_line.starts_with("fatal:") {
            // hello Repetier firmware -_-
            line = format!("Error:{line}");
            lower_line = line.to_lowercase();
        }

        if lower_line.starts_with("error:") || line.starts_with("!!") {
            if re_match(&regex_minMaxError, &line).is_some() {
                // special delivery for firmware that goes "Error:x\n: Extruder switched off. MAXTEMP triggered !\n"
                line = format!(
                    "{}{}",
                    py_rstrip(&line),
                    self._readline()
                        .expect("TypeError: can only concatenate str (not \"NoneType\") to str")
                );
                lower_line = line.to_lowercase();
            } else if re_search(&regex_marlinKillError, &line) {
                // special delivery for marlin kill errors that are split across multiple error lines
                let next_line = self
                    ._readline()
                    .expect("AttributeError: 'NoneType' object has no attribute 'strip'");
                let mut next_line = py_strip(&next_line).to_string();
                if next_line.to_lowercase().starts_with("error:") {
                    next_line =
                        py_strip(&next_line.chars().skip(6).collect::<String>()).to_string();
                }
                line = format!("{} - {}", py_rstrip(&line), next_line);
                lower_line = line.to_lowercase();
            }

            let stripped_error: String = py_strip(
                &(if lower_line.starts_with("error:") {
                    line.chars().skip(6).collect::<String>()
                } else {
                    line.chars().skip(2).collect::<String>()
                }),
            )
            .to_string();

            if Self::_recoverable_communication_errors
                .iter()
                .any(|x| lower_line.contains(x))
            {
                // manually trigger an ack for comm errors the printer doesn't send a resend request for but
                // from which we can recover from by just pushing on (because that then WILL trigger a fitting
                // resend request)
                self._handle_ok();
            } else if Self::_resend_request_communication_errors
                .iter()
                .any(|x| lower_line.contains(x))
            {
                // skip comm errors that the printer sends a resend request for anyhow
                self._lastCommError.set(Some(stripped_error.clone()));
            } else if Self::_sd_card_errors.iter().any(|x| lower_line.contains(x)) {
                // skip errors with the SD card
            } else if lower_line.contains("unknown command") {
                // ignore unknown command errors, it could be a typo or some missing feature
            } else if !self.is_error() {
                // handle everything else
                for (_name, hook) in self._error_message_hooks.iter() {
                    let args = [Value::from(stripped_error.as_str())];
                    match py_try(|| hook(&args)) {
                        Err(_) => {
                            // self._logger.exception(f"Error while processing hook {name}:", extra={"plugin": name})
                        }
                        Ok(ret) => {
                            if py_truthy(&ret) {
                                return Some(line);
                            }
                        }
                    }
                }

                self._to_logfile_with_terminal(
                    Some(&format!(
                        "Received an error from the printer's firmware: {stripped_error}"
                    )),
                    LOGGING_WARNING,
                );

                if !truthy(self._ignore_errors) {
                    if truthy(self._disconnect_on_errors)
                        || Self::_fatal_errors.iter().any(|x| lower_line.contains(x))
                    {
                        self._trigger_error(&stripped_error, "firmware", true);
                    } else if self.is_printing() {
                        let consequence = "cancel";
                        let payload = self._payload_for_error(
                            "firmware",
                            Some(consequence),
                            Some(&stripped_error),
                        );
                        self._callback.on_comm_error(
                            &stripped_error,
                            "firmware",
                            Some(consequence),
                            payload.faq.as_deref(),
                            payload.logs.as_deref(),
                        );

                        self.cancel_print(Some(&stripped_error), false, None, None, false);
                        self._clear_to_send.set();
                    }
                } else {
                    self._log(&format!(
                        "WARNING! Received an error from the printer's firmware, ignoring that as configured \
                         but you might want to investigate what happened here! Error: {stripped_error}"
                    ));
                    self._clear_to_send.set();
                }
            }
        }

        // finally return the line
        Some(line)
    }

    /// comm.py:4107 `_trigger_error(text, reason, close=True)`
    pub fn _trigger_error(self: &Arc<Self>, text: &str, reason: &str, close: bool) {
        self._errorValue.set(text.to_string());
        self._change_state(Self::STATE_ERROR);

        let trigger_m112 = truthy(self._send_m112_on_error)
            && !self.is_sd_printing()
            && reason != "connection"
            && reason != "autodetect";

        let consequence = if close && trigger_m112 {
            Some("emergency")
        } else if close {
            Some("disconnect")
        } else {
            None
        };

        let payload = self._payload_for_error(reason, consequence, None);
        self._callback.on_comm_error(
            text,
            reason,
            consequence,
            payload.faq.as_deref(),
            payload.logs.as_deref(),
        );
        event_manager().fire(Events::ERROR, Some(payload.to_json()));

        if close {
            if trigger_m112 {
                self._trigger_emergency_stop(false);
            }
            self.close(true, true, Some(10.0));
        }
    }

    /// comm.py:4139 `_error_faqs`: the FAQ entry (`"firmware-" + key`) and the words that select it
    pub const _error_faqs: [(&'static str, &'static [&'static str]); 5] = [
        ("mintemp", &["mintemp"]),
        ("maxtemp", &["maxtemp"]),
        ("thermal-runaway", &["runaway"]),
        ("heating-failed", &["heating failed"]),
        (
            "probing-failed",
            &[
                "probing failed",
                "bed leveling",
                "reference point",
                "bltouch",
            ],
        ),
    ];

    /// comm.py:4152 `_payload_for_error(reason, consequence=None, error=None)`
    pub fn _payload_for_error(
        &self,
        reason: &str,
        consequence: Option<&str>,
        error: Option<&str>,
    ) -> ErrorPayload {
        let error = match error {
            None => self.get_error_string(),
            Some(e) => e.to_string(),
        };

        let mut payload = ErrorPayload {
            error: error.clone(),
            reason: reason.to_string(),
            consequence: None,
            logs: None,
            faq: None,
        };

        if let Some(consequence) = consequence.filter(|c| !c.is_empty()) {
            payload.consequence = Some(consequence.to_string());
        }

        if reason == "firmware" {
            payload.logs = Some(self._terminal_log.with(|l| l.iter().cloned().collect()));

            let error_lower = error.to_lowercase();
            for (faq, triggers) in Self::_error_faqs {
                if triggers.iter().any(|trigger| error_lower.contains(trigger)) {
                    payload.faq = Some(format!("firmware-{faq}"));
                    break;
                }
            }
        }

        payload
    }

    /// comm.py:4172 `_readline()`: `None` ends the monitor loop, `""` is a timeout
    pub fn _readline(self: &Arc<Self>) -> Option<String> {
        let serial = self._serial.get()?;

        blocking_point("serial readline");
        let ret = match serial.readline() {
            Ok(ret) => ret,
            Err(ex) => {
                if !self._connection_closing.get() {
                    // self._logger.exception("Unexpected error while reading from serial port")
                    let info = ExceptionInfo {
                        type_name: &ex.type_name,
                        message: &ex.message,
                        file: "comm.rs",
                        function: "_readline",
                        line: line!(),
                    };
                    self._log(&format!(
                        "Unexpected error while reading serial port, please consult octoprint.log for details: {}",
                        get_exception_string_of(&info, EXCEPTION_STRING_FMT)
                    ));
                    if ex.serial_exception {
                        self._dual_log(
                            "Please see https://faq.octoprint.org/serialerror for possible reasons of this.",
                            LOGGING_ERROR,
                            "",
                        );
                    }
                    self._errorValue
                        .set(get_exception_string_of(&info, "{type}: {message}"));
                    self.close(true, true, Some(10.0));
                }
                return None;
            }
        };

        let null_pos = ret.iter().position(|b| *b == 0);

        let mut ret: String = match String::from_utf8(ret) {
            Ok(s) => s,
            // `ret.decode("latin1")`
            Err(e) => e.into_bytes().into_iter().map(|b| b as char).collect(),
        };

        if !ret.is_empty() {
            // try: self._log(f"Recv: {sanitize_ascii(ret)}") except ValueError: ... (`ret` is a str)
            self._log(&format!("Recv: {}", sanitize_ascii(&ret)));

            if let Some(null_pos) = null_pos {
                // self._logger.warning("Received line:")
                // self._logger.warning("| {}".format(ret.replace("\0", "\\x00").rstrip()))
                self._dual_log(
                    &format!(
                        "The received line contains at least one null byte character at position {null_pos}, \
                         this hints at some data corruption going on"
                    ),
                    LOGGING_WARNING,
                    "WARN",
                );
            }
        }

        for (_name, hook) in self._received_message_hooks.iter() {
            let args = [Value::from(ret.as_str())];
            match py_try(|| hook(&args)) {
                Err(_) => {
                    // self._logger.exception(f"Error while processing hook {name}:", extra={"plugin": name})
                }
                Ok(Value::Null) => return Some(String::new()),
                Ok(Value::String(s)) => ret = s,
                Ok(_) => unimplemented!(
                    "plugin.rs: a received-message hook that returns something else than a str or None"
                ),
            }
        }

        Some(ret)
    }

    /// comm.py:4232 `_get_next_from_job()`: `(line, pos, lineno)`; the line is a str or a marker
    pub fn _get_next_from_job(
        self: &Arc<Self>,
    ) -> (Option<QueuedCommand>, Option<i64>, Option<i64>) {
        let Some(current_file) = self._currentFile.get() else {
            return (None, None, None);
        };

        let (line, pos, lineno) = match current_file.get_next() {
            Ok(next) => next,
            Err(OSError(_)) => {
                self._log(
                    "There was an error reading from the file that's being printed, cancelling the print. Please \
                     consult octoprint.log for details on the error.",
                );
                self.cancel_print(None, false, None, None, false);
                return (None, None, None);
            }
        };

        let Some(line) = line else {
            if self
                ._currentFile
                .get()
                .is_some_and(|f| f.is_streaming_gcode_file_information())
            {
                self._finish_file_transfer(false, None);
                return (None, pos, lineno);
            } else {
                self._change_state(Self::STATE_FINISHING);
                self.send_command(
                    QueuedCommand::Command("M400".to_string()),
                    None,
                    true,
                    false,
                    false,
                    None,
                    None,
                );
                self._callback.on_comm_print_job_done(false);

                let me = self.clone();
                let finalize = move || {
                    me._change_state(Self::STATE_OPERATIONAL);
                };

                return (
                    Some(QueuedCommand::Marker(
                        SendQueueMarker::new(Some(Box::new(finalize))).0,
                    )),
                    None,
                    None,
                );
            }
        };
        (Some(QueuedCommand::Command(line)), pos, lineno)
    }

    /// comm.py:4260 `_send_from_job()`
    pub fn _send_from_job(self: &Arc<Self>) -> bool {
        let _job_lock = self._jobLock.lock();
        while self._active() {
            // we loop until we've actually enqueued a line for sending
            if self._state.get() != Self::STATE_STARTING
                && self._state.get() != Self::STATE_PRINTING
            {
                // we are no longer printing, return false
                return false;
            } else if self.job_on_hold() {
                // job is on hold, return false
                return false;
            }

            let (line, pos, lineno) = self._get_next_from_job();
            let line = match line {
                Some(QueuedCommand::Marker(marker)) => {
                    self.send_command(
                        QueuedCommand::Marker(marker),
                        None,
                        true,
                        false,
                        false,
                        None,
                        None,
                    );
                    self._callback.on_comm_progress();

                    // end of file, return false so that the next round in continue_sending will process
                    // what we just enqueued (any scripts + marker)
                    return false;
                }
                None => {
                    // end of file, return false
                    return false;
                }
                Some(QueuedCommand::Command(line)) => line,
            };

            let py_int_or_none =
                |v: Option<i64>| v.map_or_else(|| "None".to_string(), |v| v.to_string());
            let result = self._send_command(
                QueuedCommand::Command(line),
                None,
                None,
                Some(with_tags(
                    &BTreeSet::new(),
                    &[
                        "source:file",
                        &format!("filepos:{}", py_int_or_none(pos)),
                        &format!("fileline:{}", py_int_or_none(lineno)),
                    ],
                )),
            );
            self._callback.on_comm_progress();
            if result {
                // line from file sent, return true
                return true;
            }

            // self._logger.debug('Command "{}" from file not enqueued, doing another iteration')
        }
        false
    }

    /// comm.py:4304 `_send_from_job_queue()`
    pub fn _send_from_job_queue(self: &Arc<Self>) -> bool {
        match self._job_queue.get_nowait() {
            Ok((line, cmd_type, on_sent, tags)) => {
                let result = self._send_command(line, cmd_type.as_deref(), on_sent, Some(tags));
                if result {
                    // line from script sent, return true
                    return true;
                }
            }
            Err(GetError::Empty) => {}
            Err(e) => panic!("JobQueue.get_nowait raised {e:?}"),
        }

        false
    }

    /// comm.py:4318 `_handle_resend_request(line)`: Python's `True` / `False`
    pub fn _handle_resend_request(self: &Arc<Self>, line: &str) -> bool {
        self._received_resend_requests
            .set(self._received_resend_requests.get() + 1);
        self._reevaluate_resend_ratio();

        // try: ... finally: ...
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self._handle_resend_request_body(line)
        }));

        let trigger_ok_after_resend = self
            ._trigger_ok_after_resend
            .as_ref()
            .and_then(Value::as_str);
        if trigger_ok_after_resend == Some("always") {
            self._handle_ok();
        } else if trigger_ok_after_resend == Some("detect") {
            let me = self.clone();
            // the Timer is made on the monitoring thread, a daemon thread: it inherits that
            let timer = Arc::new(Timer::new(
                self.timeout_interval("resendOk", 1.0),
                move || me._resend_simulate_ok(),
                true,
            ));
            self._resend_ok_timer.set(Some(timer.clone()));
            timer
                .start()
                .expect("RuntimeError: threads can only be started once");
        }

        match result {
            Ok(r) => r,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    /// the `try:` body of `_handle_resend_request` (comm.py:4319-4426)
    fn _handle_resend_request_body(self: &Arc<Self>, line: &str) -> bool {
        let Some(line_to_resend) = parse_resend_line(line) else {
            return false;
        };

        if self._resendDelta.get().is_none()
            && line_to_resend == self._current_line.get()
            && self._current_line.get() == 1
        {
            // We probably just handled a resend and this request originates from lines sent before that
            // self._logger.info("Got a resend request for line 1 which is also our current line. It looks like we just handled a reset and this is a left over of this")
            return false;
        } else if self._resendDelta.get().is_none() && line_to_resend == self._current_line.get() {
            // We don't expect to have an active resend request and the printer is requesting a resend of
            // a line we haven't yet sent.
            //
            // This means the printer got a line from us with N = self._current_line - 1 but had already
            // acknowledged that. This can happen if the last line was resent due to a timeout during
            // an active (prior) resend request.
            //
            // We will ignore this resend request and just continue normally.
            // self._logger.info("Ignoring resend request for line %d == current line, we haven't sent that yet so the printer got N-1 twice from us, probably due to a timeout")
            return false;
        }

        let last_comm_error = self._lastCommError.get();
        self._lastCommError.set(None);

        let resend_delta = self._current_line.get() - line_to_resend;

        if let Some(last_comm_error) = &last_comm_error
            && (last_comm_error.to_lowercase().contains("line number")
                || last_comm_error.to_lowercase().contains("expected line"))
            && Some(line_to_resend) == self._lastResendNumber.get()
            && self._resendDelta.get().is_some()
            && self._currentResendCount.get() < resend_delta
        {
            // self._logger.info("Ignoring resend request for line %d, that still originates from lines we sent before we got the first resend request")
            self._currentResendCount
                .set(self._currentResendCount.get() + 1);
            return true;
        }

        if self._currentConsecutiveResendNumber.get() == Some(line_to_resend) {
            self._currentConsecutiveResendCount
                .set(self._currentConsecutiveResendCount.get() + 1);
            if self._currentConsecutiveResendCount.get()
                >= self._maxConsecutiveResends.expect(
                    "TypeError: '>=' not supported between instances of 'int' and 'NoneType'",
                )
            {
                // printer keeps requesting the same line again and again, something is severely broken here
                let error_text = format!(
                    "Printer keeps requesting line {line_to_resend} again and again, communication stuck"
                );
                self._log(&error_text);
                // self._logger.warning(error_text)
                self._trigger_error(&error_text, "resend_loop", true);
            }
        } else {
            self._currentConsecutiveResendNumber
                .set(Some(line_to_resend));
            self._currentConsecutiveResendCount.set(0);
        }

        self._resendActive.set(true);
        self._resendDelta.set(Some(resend_delta));
        self._lastResendNumber.set(Some(line_to_resend));
        self._currentResendCount.set(0);

        self._resend_check_possibility(line_to_resend);

        // if we log resends, make sure we don't log more resends than the set rate within a window
        //
        // this it to prevent the log from getting flooded for extremely bad communication issues
        if truthy(self._log_resends) {
            let now = time_monotonic();
            let new_rate_window = self
                ._log_resends_rate_start
                .get()
                .is_none_or(|start| start + (self._log_resends_rate_frame as f64) < now);
            let in_rate = self._log_resends_rate_count.get() < self._log_resends_max;

            if new_rate_window || in_rate {
                if new_rate_window {
                    self._log_resends_rate_start.set(Some(now));
                    self._log_resends_rate_count.set(0);
                }

                self._to_logfile_with_terminal(
                    Some(&format!(
                        "Got a resend request from the printer: requested line = {}, current line = {}",
                        line_to_resend,
                        self._current_line.get()
                    )),
                    LOGGING_INFO,
                );
                self._log_resends_rate_count
                    .set(self._log_resends_rate_count.get() + 1);
            }
        }

        self._send_queue.set_resend_active(true);

        true
    }

    /// comm.py:4428 `_resendSimulateOk()`
    pub fn _resend_simulate_ok(self: &Arc<Self>) {
        self._resend_ok_timer.set(None);
        self._handle_ok();
        // self._logger.info("Firmware didn't send an 'ok' with their resend request. That's a known bug with some firmware variants out there. Simulating an ok to continue...")
    }

    /// comm.py:4436 `_resendSameCommand()`
    pub fn _resend_same_command(self: &Arc<Self>) -> bool {
        self._resend_next_command(true)
    }

    /// comm.py:4439 `_resendNextCommand(again=False)`
    pub fn _resend_next_command(self: &Arc<Self>, again: bool) -> bool {
        self._lastCommError.set(None);

        // Make sure we are only handling one sending job at a time
        let _sending_lock = self._sendingLock.lock();
        if again {
            // If we are about to send the last line from the active resend request
            // again, we first need to increment resend delta. It might already
            // be set to None if the last resend line was already sent, so
            // if that's the case we set it to 0. It will then be incremented,
            // the last line will be sent again, and then the delta will be
            // decremented and set to None again, completing the cycle.
            if self._resendDelta.get().is_none() {
                self._resendDelta.set(Some(0));
            }
            self._resendDelta.set(Some(
                self._resendDelta
                    .get()
                    .expect("TypeError: unsupported operand type(s) for +=")
                    + 1,
            ));
        } else if self._resendDelta.get().is_none() {
            // we might enter this twice in quick succession if we get triggered by the
            // resend_ok_timer, so make sure that resendDelta is actually still set (see #2632)
            return false;
        }

        let line_number = self._current_line.get()
            - self
                ._resendDelta
                .get()
                .expect("TypeError: unsupported operand type(s) for -: 'int' and 'NoneType'");

        if !self._resend_check_possibility(line_number) {
            // Something has gone wrong if we get here, but we already logged and
            // handled it.
            return false;
        }

        // `self._lastLines[-self._resendDelta]`
        let index = -(self
            ._resendDelta
            .get()
            .expect("TypeError: bad operand type for unary -: 'NoneType'")
            as isize);
        let last_line = self
            ._lastLines
            .with(|l| l.get(index).cloned())
            .expect("IndexError: deque index out of range");
        let cmd = decode_serial_bytes(&last_line, &self._serial_encoding);
        let result = self._enqueue_for_sending(
            QueuedCommand::Command(cmd),
            Some(line_number),
            None,
            None,
            true,
            None,
        );

        self._resendDelta.set(Some(
            self._resendDelta
                .get()
                .expect("TypeError: unsupported operand type(s) for -=")
                - 1,
        ));
        if self
            ._resendDelta
            .get()
            .expect("TypeError: '<=' not supported between instances of 'NoneType' and 'int'")
            <= 0
        {
            // reset everything BUT the resendActive flag - that will have to stay active until we receive
            // our next ok!
            self._resendDelta.set(None);
            self._lastResendNumber.set(None);
            self._currentResendCount.set(0);

            self._send_queue.set_resend_active(false);
        }

        result
    }

    /// comm.py:4482 `_resendCheckPossibility(lineno)`
    pub fn _resend_check_possibility(self: &Arc<Self>, lineno: i64) -> bool {
        let delta = || {
            self._resendDelta
                .get()
                .expect("TypeError: '>' not supported between instances of 'NoneType' and 'int'")
        };
        let history = self._lastLines.with(|l| l.len()) as i64;
        if delta() > history || self._lastLines.with(|l| l.len()) == 0 || delta() < 0 {
            let error_text = format!(
                "Should resend line {lineno} but no sufficient history is available, can't resend"
            );
            self._log(&error_text);
            // self._logger.warning(error_text + ". Line to resend is {}, current line is {}, line history has {} entries.")
            if self.is_printing() {
                // abort the print & disconnect, there's nothing we can do to rescue it
                self._trigger_error(&error_text, "resend", true);
            } else {
                // reset resend delta, we can't do anything about it
                self._resendDelta.set(None);
            }

            false
        } else {
            true
        }
    }

    // ~~ section I (comm.py:4509-5688): sending (differences 28-31)

    /// comm.py:4509 `_sendCommand(cmd, cmd_type=None, on_sent=None, tags=None)`
    ///
    /// Takes `self: &Arc<Self>` because the phase handlers call `setPause` / `cancelPrint` /
    /// `_continue_sending` (difference 21). `tags` is `None` or a set (Python shares the set
    /// object; it is a `Tags` from here on, as `_normalize_command_handler_result` expects).
    pub fn _send_command(
        self: &Arc<Self>,
        cmd: QueuedCommand,
        cmd_type: Option<&str>,
        on_sent: Option<OnSent>,
        tags: Option<BTreeSet<String>>,
    ) -> bool {
        // Make sure we are only handling one sending job at a time
        let _sending = self._sendingLock.lock();
        if self._serial.get().is_none() {
            return false;
        }

        let cmd = match cmd {
            QueuedCommand::Marker(marker) => {
                // `isinstance(cmd, QueueMarker)`: every marker in this port is a `SendQueueMarker`
                // (difference 28), so the `else: return False` branch cannot be reached
                self._enqueue_for_sending(
                    QueuedCommand::Marker(marker),
                    None,
                    None,
                    None,
                    false,
                    None,
                );
                return true;
            }
            QueuedCommand::Command(cmd) => cmd,
        };

        let tags: Option<Tags> = tags.map(|t| Rc::new(RefCell::new(t)));
        let (gcode, subcode) = gcode_and_subcode_for_cmd(Some(&cmd));

        let results: Vec<NormalizedCommand> = if !self.is_streaming() {
            // trigger the "queuing" phase only if we are not streaming to sd right now
            let results = self._process_command_phase(
                "queuing",
                &cmd,
                cmd_type.map(str::to_string),
                gcode.clone(),
                subcode.clone(),
                tags.clone(),
            );

            if results.is_empty() {
                // command is no more, return
                return false;
            }
            results
        } else {
            vec![(
                Some(cmd.clone()),
                cmd_type.map(str::to_string),
                gcode,
                subcode,
                tags,
            )]
        };

        // process helper
        let process = |cmd: Option<String>,
                       cmd_type: Option<String>,
                       gcode: Option<String>,
                       subcode: Option<String>,
                       on_sent: Option<OnSent>,
                       tags: Option<Tags>|
         -> bool {
            let Some(cmd) = cmd else {
                // no command, next entry
                return false;
            };

            if let Some(event) = gcode
                .as_deref()
                .filter(|g| !g.is_empty())
                .and_then(gcode_to_event)
            {
                // if this is a gcode bound to an event, trigger that now
                event_manager().fire(event, None);
            }

            // process @ commands
            if gcode.is_none() && cmd.starts_with('@') {
                self._process_atcommand_phase("queuing", &cmd, tags.clone());
            }

            // actually enqueue the command for sending
            let tags_set = tags.as_ref().map(|t| t.borrow().clone());
            if self._enqueue_for_sending(
                QueuedCommand::Command(cmd.clone()),
                None,
                cmd_type.as_deref(),
                on_sent,
                false,
                tags_set,
            ) {
                if !self.is_streaming() {
                    // trigger the "queued" phase only if we are not streaming to sd right now
                    self._process_command_phase("queued", &cmd, cmd_type, gcode, subcode, tags);
                }
                true
            } else {
                false
            }
        };

        // split off the final command, because that needs special treatment
        let mut results = results;
        let last_command = results.pop().expect("results is not empty");

        // track if we enqueued anything at all
        let mut enqueued_something = false;

        // process all but the last ...
        for (cmd, cmd_type, gcode, subcode, tags) in results {
            enqueued_something =
                process(cmd, cmd_type, gcode, subcode, None, tags) || enqueued_something;
        }

        // ... and then process the last one with the on_sent callback attached
        let (cmd, cmd_type, gcode, subcode, tags) = last_command;
        process(cmd, cmd_type, gcode, subcode, on_sent, tags) || enqueued_something
    }

    // ~~ send loop handling

    /// comm.py:4602 `_enqueue_for_sending(command, linenumber=None, command_type=None, on_sent=None,
    /// resend=False, tags=None)`
    ///
    /// Enqueues a command and optional linenumber to use for it in the send queue. `tags` `None`
    /// is stored as the empty set (difference 28).
    pub fn _enqueue_for_sending(
        &self,
        command: QueuedCommand,
        linenumber: Option<i64>,
        command_type: Option<&str>,
        on_sent: Option<OnSent>,
        resend: bool,
        tags: Option<BTreeSet<String>>,
    ) -> bool {
        let target = if resend { "resend" } else { "send" };

        match self._send_queue.put(
            SendQueueEntry::Line {
                command,
                linenumber,
                command_type: command_type.map(str::to_string),
                on_sent,
                flag: false,
                tags,
            },
            command_type.map(str::to_string),
            Some(target.to_string()),
            true,
            None,
        ) {
            Ok(()) => true,
            Err(PutError::Raised(_e)) => {
                // self._logger.debug("Type already in send queue: " + e.type)
                false
            }
            Err(e) => panic!("put on the unbounded send queue: {e:?}"),
        }
    }

    /// comm.py:4640 `_use_up_clear(gcode)`
    pub fn _use_up_clear(&self, gcode: Option<&str>) -> bool {
        // we only need to use up a clear if the command we just sent was either a gcode command or if we also
        // require ack's for unknown commands
        let mut eats_clear = truthy(self._unknownCommandsNeedAck.get());
        if gcode.is_some() {
            eats_clear = true;
        }

        if eats_clear {
            // if we need to use up a clear, do that now
            self._clear_to_send.clear(false);
        }

        eats_clear
    }

    /// comm.py:4653 `_send_loop()`
    ///
    /// The send loop is responsible of sending commands in ``self._send_queue`` over the line, if it is cleared for
    /// sending (through received ``ok`` responses from the printer's firmware.
    ///
    /// OURS (difference 29): the loop body is a closure for `py_try`; `continue` / `break` are
    /// `LoopStep`; the inner `try: ... finally: task_done()` is `catch_unwind` + `resume_unwind`.
    pub fn _send_loop(self: &Arc<Self>) {
        self._clear_to_send.wait(None);

        while self._send_queue_active.get() {
            // try: ... except Exception: self._logger.exception("Caught an exception in the send loop")
            let step = py_try(|| -> LoopStep {
                // wait until we have something in the queue
                let entry = match self._send_queue.get(true, None) {
                    Ok(entry) => entry,
                    Err(GetError::Empty) => {
                        // I haven't yet been able to figure out *why* this can happen but according to #3096 and SERVER-2H
                        // an Empty exception can fly here due to resend_active being True but nothing being in the resend
                        // queue of the send queue. So we protect against this possibility...
                        return LoopStep::Continue;
                    }
                    Err(e) => panic!("SendQueue.get raised {e:?}"),
                };

                // try: ... finally: self._send_queue.task_done()
                let inner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    self._send_loop_entry(entry)
                }));
                // no matter _how_ we exit this block, we signal that we
                // are done processing the last fetched queue entry
                self._send_queue
                    .task_done()
                    .expect("ValueError: task_done() called too many times");
                let step = match inner {
                    Ok(step) => step,
                    Err(payload) => std::panic::resume_unwind(payload),
                };
                if !matches!(step, LoopStep::Wait) {
                    return step;
                }

                // now we just wait for the next clear and then start again
                self._clear_to_send.wait(None);
                LoopStep::Wait
            });
            if let Ok(LoopStep::Break) = step {
                break;
            }
        }
        self._dual_log("Closing down send loop", LOGGING_INFO, "");
    }

    /// The `try:` body of `_send_loop` (comm.py:4669-4776) for one fetched entry; `LoopStep::Continue` is its
    /// `continue`, `Break` its `break`, `Wait` is falling through to `self._clear_to_send.wait()`.
    fn _send_loop_entry(self: &Arc<Self>, entry: SendQueueEntry) -> LoopStep {
        // make sure we are still active
        if !self._send_queue_active.get() {
            return LoopStep::Break;
        }

        // sleep if we are dwelling
        let now = time_monotonic();
        if truthy(self._blockWhileDwelling.get())
            && self._dwelling_until.get().is_some_and(|d| d != 0.0)
            && now
                < self
                    ._dwelling_until
                    .get()
                    .expect("TypeError: '<' not supported between instances of 'float' and 'bool'")
        {
            let until = self
                ._dwelling_until
                .get()
                .expect("TypeError: unsupported operand type(s) for -");
            std::thread::sleep(std::time::Duration::from_secs_f64((until - now).max(0.0)));
            self._dwelling_until.set(None); // `= False`
        }

        // fetch command, command type and optional linenumber and sent callback from queue
        let (command, linenumber, command_type, on_sent, processed, tags) = match entry {
            SendQueueEntry::Line {
                command,
                linenumber,
                command_type,
                on_sent,
                flag,
                tags,
            } => (command, linenumber, command_type, on_sent, flag, tags),
            // `command, linenumber, ... = entry` of a bare marker (comm.py:1214 puts one while
            // `_send_queue_active` is already False, so the check above breaks first)
            SendQueueEntry::Marker(_) => {
                panic!("TypeError: cannot unpack non-iterable SendQueueMarker object")
            }
        };

        let mut command = match command {
            QueuedCommand::Marker(marker) => {
                marker.run();
                self._continue_sending();
                return LoopStep::Continue;
            }
            QueuedCommand::Command(command) => command,
        };

        // some firmwares (e.g. Smoothie) might support additional in-band communication that will not
        // stick to the acknowledgement behaviour of GCODE, so we check here if we have a GCODE command
        // at hand here and only clear our clear_to_send flag later if that's the case
        let (mut gcode, mut subcode) = gcode_and_subcode_for_cmd(Some(&command));
        let mut tags: Option<Tags> = tags.map(|t| Rc::new(RefCell::new(t)));

        if let Some(linenumber) = linenumber {
            // line number predetermined - this only happens for resends, so we'll use the number and
            // send directly without any processing (since that already took place on the first sending!)
            self._use_up_clear(gcode.as_deref());
            let encoded = encode_serial_text(&command, &self._serial_encoding, false);
            self._do_send_with_checksum(&encoded, linenumber);
        } else {
            if !processed {
                // trigger "sending" phase if we didn't so far
                let results = self._process_command_phase(
                    "sending",
                    &command,
                    command_type.clone(),
                    gcode.clone(),
                    subcode.clone(),
                    tags.clone(),
                );

                if results.is_empty() {
                    // No, we are not going to send this, that was a last-minute bail.
                    // However, since we already are in the send queue, our _monitor
                    // loop won't be triggered with the reply from this unsent command
                    // now, so we try to tickle the processing of any active
                    // command queues manually
                    self._continue_sending();

                    // and now let's fetch the next item from the queue
                    return LoopStep::Continue;
                }

                // we explicitly throw away plugin hook results that try
                // to perform command expansion in the sending/sent phase,
                // so "results" really should only have more than one entry
                // at this point if our core code contains a bug
                assert!(results.len() == 1);

                // we only use the first (and only!) entry here
                let (c, _, g, sc, t) = results.into_iter().next().expect("one result");
                command = c.expect("a command result is never None");
                gcode = g;
                subcode = sc;
                tags = t;
            }

            if py_strip(&command).is_empty() {
                // self._logger.info("Refusing to send an empty line to the printer")

                // same here, tickle the queues manually
                self._continue_sending();

                // and fetch the next item
                return LoopStep::Continue;
            }

            // handle @ commands
            if gcode.is_none() && command.starts_with('@') {
                self._process_atcommand_phase("sending", &command, tags.clone());

                // tickle...
                self._continue_sending();

                // ... and fetch the next item
                return LoopStep::Continue;
            }

            // now comes the part where we increase line numbers and send stuff - no turning back now
            let used_up_clear = self._use_up_clear(gcode.as_deref());
            self._do_send(&command, gcode.as_deref());
            if !used_up_clear {
                // If we didn't use up a clear we need to tickle the read queue - there might
                // not be a reply to this command, so our _monitor loop will stay waiting until
                // timeout. We definitely do not want that, so we tickle the queue manually here
                self._continue_sending();
            }
        }

        // trigger "sent" phase and use up one "ok"
        if let Some(on_sent) = on_sent {
            // we have a sent callback for this specific command, let's execute it now
            on_sent();
        }
        self._process_command_phase("sent", &command, command_type, gcode, subcode, tags);

        LoopStep::Wait
    }

    /// comm.py:4794 `_log_command_phase(phase, command, *args, **kwargs)`: only logs (to
    /// `_phaseLogger`, not ported: difference 1).
    pub fn _log_command_phase(
        &self,
        _phase: &str,
        _command: &str,
        _command_type: Option<&str>,
        _gcode: Option<&str>,
        _subcode: Option<&str>,
        _tags: &Option<Tags>,
    ) {
        // if self._phaseLogger.isEnabledFor(logging.DEBUG):
        //     "phase: {phase} | command: {command} | command_type | gcode | subcode | tags: [ sorted ]"
    }

    /// comm.py:4814 `_process_command_phase(phase, command, command_type=None, gcode=None, subcode=None, tags=None)`
    pub fn _process_command_phase(
        self: &Arc<Self>,
        phase: &str,
        command: &str,
        command_type: Option<String>,
        gcode: Option<String>,
        subcode: Option<String>,
        tags: Option<Tags>,
    ) -> Vec<NormalizedCommand> {
        let (gcode, subcode) = if gcode.is_none() {
            gcode_and_subcode_for_cmd(Some(command))
        } else {
            (gcode, subcode)
        };
        let mut results: Vec<NormalizedCommand> = vec![(
            Some(command.to_string()),
            command_type.clone(),
            gcode.clone(),
            subcode.clone(),
            tags.clone(),
        )];

        self._log_command_phase(
            phase,
            command,
            command_type.as_deref(),
            gcode.as_deref(),
            subcode.as_deref(),
            &tags,
        );

        if (self.is_streaming() && self.is_printing())
            || !["queuing", "queued", "sending", "sent"].contains(&phase)
        {
            return results;
        }

        // send it through the phase specific handlers provided by plugins
        let hooks = self
            ._gcode_hooks
            .get(phase)
            .expect("KeyError: the phase has hooks");
        for (name, hook) in hooks.iter() {
            let hook_run = py_try(|| -> Vec<NormalizedCommand> {
                let mut new_results: Vec<NormalizedCommand> = Vec::new();
                for (command, command_type, gcode, subcode, tags) in results.iter() {
                    // OURS (difference 31): positional JSON, without the comm instance
                    let hook_results = hook(&[
                        Value::String(phase.to_string()),
                        json_opt_str(command),
                        json_opt_str(command_type),
                        json_opt_str(gcode),
                        json_opt_str(subcode),
                        json_tags(tags),
                    ]);

                    let tags_to_add: BTreeSet<String> = [
                        "source:rewrite".to_string(),
                        format!("phase:{phase}"),
                        format!("plugin:{name}"),
                    ]
                    .into_iter()
                    .collect();
                    let normalized = _normalize_command_handler_result(
                        command.clone(),
                        command_type.clone(),
                        gcode.clone(),
                        subcode.clone(),
                        tags.clone(),
                        hook_value_to_results(&hook_results),
                        Some(&tags_to_add),
                    );

                    // make sure we don't allow multi entry results in anything but the queuing phase
                    if phase != "queuing" && normalized.len() > 1 {
                        // self._logger.error("Error while processing hook {name} for phase {phase} and command
                        // {command}: Hook returned multi-entry result ... Ignoring hook result and sending command as-is.")
                        new_results.push((
                            command.clone(),
                            command_type.clone(),
                            gcode.clone(),
                            subcode.clone(),
                            tags.clone(),
                        ));
                    } else {
                        new_results.extend(normalized);
                    }
                }
                new_results
            });

            match hook_run {
                Err(_) => {
                    // self._logger.exception("Error while processing hook {name} for phase {phase}:")
                }
                Ok(new_results) => {
                    if new_results.is_empty() {
                        // hook handler returned None or empty list for all commands, so
                        // we'll stop here and return a full out empty result
                        return Vec::new();
                    }
                    results = new_results;
                }
            }
        }

        // if it's a gcode command send it through the specific handler if it exists
        let mut new_results: Vec<NormalizedCommand> = Vec::new();
        let mut modified = false;
        for (command, command_type, gcode, subcode, tags) in results.iter() {
            if let Some(g) = gcode {
                // `hasattr(self, "_gcode_" + gcode + "_" + phase)`
                let cmd = command
                    .as_deref()
                    .expect("a command in the results is never None");
                if let Some(handler_results) = self._gcode_handler(g, phase, cmd) {
                    new_results.extend(_normalize_command_handler_result(
                        command.clone(),
                        command_type.clone(),
                        gcode.clone(),
                        subcode.clone(),
                        tags.clone(),
                        handler_results,
                        None,
                    ));
                    modified = true;
                } else {
                    new_results.push((
                        command.clone(),
                        command_type.clone(),
                        gcode.clone(),
                        subcode.clone(),
                        tags.clone(),
                    ));
                }
            } else {
                new_results.push((
                    command.clone(),
                    command_type.clone(),
                    gcode.clone(),
                    subcode.clone(),
                    tags.clone(),
                ));
            }
        }

        if modified {
            if new_results.is_empty() {
                // gcode handler returned None or empty list for all commands, so we'll stop here and return a full out empty result
                return Vec::new();
            } else {
                results = new_results;
            }
        }

        // send it through the phase specific command handler if it exists
        if phase == "queuing" || phase == "sending" {
            let mut new_results: Vec<NormalizedCommand> = Vec::new();
            for (command, command_type, gcode, subcode, tags) in results.iter() {
                let cmd = command
                    .as_deref()
                    .expect("a command in the results is never None");
                let handler_results = if phase == "queuing" {
                    self._command_phase_queuing(cmd, gcode.as_deref(), tags)
                } else {
                    self._command_phase_sending(cmd, gcode.as_deref());
                    None
                };
                new_results.extend(_normalize_command_handler_result(
                    command.clone(),
                    command_type.clone(),
                    gcode.clone(),
                    subcode.clone(),
                    tags.clone(),
                    handler_results,
                    None,
                ));
            }
            results = new_results;
        }

        // finally return whatever we resulted on
        results
    }

    /// comm.py:4947 `_process_atcommand_phase(phase, command, tags=None)`
    pub fn _process_atcommand_phase(
        self: &Arc<Self>,
        phase: &str,
        command: &str,
        tags: Option<Tags>,
    ) {
        if (self.is_streaming() && self.is_printing()) || !["queuing", "sending"].contains(&phase) {
            return;
        }

        let split = py_split_none(command, 1);
        let (atcommand, parameters) = if split.len() == 2 {
            (split[0], split[1])
        } else {
            (
                split
                    .first()
                    .copied()
                    .expect("IndexError: list index out of range"),
                "",
            )
        };

        let atcommand: String = atcommand.chars().skip(1).collect();

        // send it through the phase specific handlers provided by plugins
        let hooks = self
            ._atcommand_hooks
            .get(phase)
            .expect("KeyError: the phase has hooks");
        for (_name, hook) in hooks.iter() {
            // try: hook(self, phase, atcommand, parameters, tags=tags) except Exception: (log)
            let _ = py_try(|| {
                hook(&[
                    Value::String(phase.to_string()),
                    Value::String(atcommand.clone()),
                    Value::String(parameters.to_string()),
                    json_tags(&tags),
                ])
            });
        }

        // trigger built-in handler if available: `getattr(self, f"_atcommand_{atcommand.lower()}_{phase}", None)`
        let lower = atcommand.to_lowercase();
        let handled = match (lower.as_str(), phase) {
            ("pause", "queuing") => Some(0),
            ("cancel", "queuing") | ("abort", "queuing") => Some(1),
            ("resume", "queuing") => Some(2),
            _ => None,
        };
        if let Some(handler) = handled {
            // try: handler(atcommand, parameters, tags=tags) except Exception: (log)
            let _ = py_try(|| match handler {
                0 => self._atcommand_pause_queuing(&atcommand, parameters, &tags),
                1 => self._atcommand_cancel_queuing(&atcommand, parameters, &tags),
                _ => self._atcommand_resume_queuing(&atcommand, parameters, &tags),
            });
        }
    }

    // ~~ actual sending via serial

    /// comm.py:4990 `_needs_checksum(gcode=None)`
    pub fn _needs_checksum(&self, gcode: Option<&str>) -> bool {
        let command_requiring_checksum =
            gcode.is_some() && py_in_list(&self._checksum_requiring_commands, gcode);
        let command_allowing_checksum =
            gcode.is_some() || truthy(self._sendChecksumWithUnknownCommands.get());
        command_requiring_checksum || (command_allowing_checksum && self._checksum_enabled())
    }

    /// comm.py:5002 `_checksum_enabled` (property)
    pub fn _checksum_enabled(&self) -> bool {
        !truthy(self._neverSendChecksum)
            && ((self.is_printing()
                && self._currentFile.get().is_some()
                && self
                    ._currentFile
                    .get()
                    .expect("AttributeError: 'NoneType' object has no attribute 'checksum'")
                    .checksum())
                || self._manualStreaming.get()
                || truthy(self._alwaysSendChecksum.get())
                || !self._firmware_info_received.get())
    }

    /// comm.py:5010 `_do_send(command, gcode=None)`
    pub fn _do_send(self: &Arc<Self>, command: &str, gcode: Option<&str>) {
        let command_to_send = encode_serial_text(command, &self._serial_encoding, true);
        if self._needs_checksum(gcode) {
            self._do_increment_and_send_with_checksum(&command_to_send);
        } else {
            self._do_send_without_checksum(&command_to_send, true);
        }
    }

    /// comm.py:5017 `_do_increment_and_send_with_checksum(cmd)`
    pub fn _do_increment_and_send_with_checksum(self: &Arc<Self>, cmd: &[u8]) {
        let _line = self._line_mutex.lock();
        let linenumber = self._current_line.get();
        self._add_to_last_lines(cmd.to_vec());
        self._current_line.set(self._current_line.get() + 1);
        self._do_send_with_checksum(cmd, linenumber);
    }

    /// comm.py:5024 `_do_send_with_checksum(command, linenumber)`
    pub fn _do_send_with_checksum(self: &Arc<Self>, command: &[u8], linenumber: i64) {
        let mut command_to_send: Vec<u8> = b"N".to_vec();
        command_to_send.extend(encode_serial_text(
            &linenumber.to_string(),
            &self._serial_encoding,
            false,
        ));
        command_to_send.push(b' ');
        command_to_send.extend_from_slice(command);
        let mut checksum: u8 = 0;
        for c in &command_to_send {
            checksum ^= c;
        }
        command_to_send.push(b'*');
        command_to_send.extend(encode_serial_text(
            &checksum.to_string(),
            &self._serial_encoding,
            false,
        ));
        self._do_send_without_checksum(&command_to_send, true);
    }

    /// comm.py:5036 `_do_send_without_checksum(cmd, log=True)`
    ///
    /// `self._serial.write(..)` reads `self._serial` each time, as Python does: a `None` set by
    /// another thread in between is the `AttributeError` that the `except Exception` catches.
    pub fn _do_send_without_checksum(self: &Arc<Self>, cmd: &[u8], log: bool) {
        if self._serial.get().is_none() {
            return;
        }

        if log {
            self._log(&format!(
                "Send: {}",
                decode_serial_bytes(cmd, &self._serial_encoding)
            ));
        }

        let mut cmd: Vec<u8> = cmd.to_vec();
        cmd.push(b'\n');
        let mut written: i64 = 0;
        let mut passes: i64 = 0;

        let write = |to_send: &[u8]| -> Result<Option<i64>, SerialError> {
            match self._serial.get() {
                Some(serial) => {
                    blocking_point("serial write");
                    serial.write(to_send)
                }
                None => Err(SerialError::other(
                    "AttributeError",
                    "'NoneType' object has no attribute 'write'",
                )),
            }
        };
        let total = cmd.len() as i64;
        // `written += len(cmd)` when the result is no int, else `written += result`
        let counted = |result: Option<i64>| -> i64 {
            match result {
                // probably some plugin not returning the written bytes, assuming all of them
                None => total,
                Some(result) => result,
            }
        };
        // the `except Exception as ex:` body, which both handlers share
        let unexpected_write_error = |ex: &SerialError| {
            if !self._connection_closing.get() {
                // self._logger.exception("Unexpected error while writing to serial port")
                let info = ExceptionInfo {
                    type_name: &ex.type_name,
                    message: &ex.message,
                    file: "comm.rs",
                    function: "_do_send_without_checksum",
                    line: line!(),
                };
                self._log(&format!(
                    "Unexpected error while writing to serial port: {}",
                    get_exception_string_of(&info, EXCEPTION_STRING_FMT)
                ));
                if ex.serial_exception {
                    self._dual_log(
                        "Please see https://faq.octoprint.org/serialerror for possible reasons of this.",
                        LOGGING_INFO,
                        "",
                    );
                }
                self._errorValue
                    .set(get_exception_string_of(&info, "{type}: {message}"));
                self.close(true, true, Some(10.0));
            }
        };

        while written < total {
            let to_send: Vec<u8> = cmd[(written.max(0) as usize).min(cmd.len())..].to_vec();
            let old_written = written;

            match write(&to_send) {
                Ok(result) => written += counted(result),
                Err(e) if e.type_name == "SerialTimeoutException" => {
                    self._log("Serial timeout while writing to serial port, trying again.");
                    match write(&to_send) {
                        Ok(result) => written += counted(result),
                        Err(ex) => {
                            unexpected_write_error(&ex);
                            break;
                        }
                    }
                }
                Err(ex) => {
                    unexpected_write_error(&ex);
                    break;
                }
            }

            if old_written == written {
                // nothing written this pass
                passes += 1;
                let max_write_passes = self._max_write_passes.expect(
                    "TypeError: '>' not supported between instances of 'int' and 'NoneType'",
                );
                if passes > max_write_passes {
                    // nothing written in max consecutive passes, we give up
                    let message = format!(
                        "Could not write anything to the serial port in {max_write_passes} tries, something appears to be wrong with the printer communication"
                    );
                    self._dual_log(&message, LOGGING_ERROR, "");
                    self._errorValue
                        .set("Could not write to serial port".to_string());
                    self.close(true, true, Some(10.0));
                    break;
                }
                // if we have failed to write data after an initial retry then the printer/system
                // may be busy, so give things a little time before we try again. Extend this
                // period each time we fail until either we write the data or run out of retry attempts.
                if passes > 1 {
                    std::thread::sleep(std::time::Duration::from_secs_f64(
                        (passes - 1) as f64 / 10.0,
                    ));
                }
            }
        }

        self._transmitted_lines
            .set(self._transmitted_lines.get() + 1);
    }

    // ~~ command handlers

    // ~~ gcode

    /// comm.py:5127 `NOT_SENDING_T.format(action=.., tool=..)`
    fn not_sending_t(action: &str, tool: i64) -> String {
        format!(
            "Not {action} T{tool}, that tool doesn't exist according to the printer profile or \
             was reported as invalid by the firmware. Make sure your printer profile is set up \
             correctly in OctoPrint, with the number of extruders set to all available extruders."
        )
    }

    /// `self._printerProfileManager.get_current_or_default()` (`None` is the `AttributeError`)
    fn _current_profile(&self) -> Value {
        self._printerProfileManager
            .as_ref()
            .expect("AttributeError: 'NoneType' object has no attribute 'get_current_or_default'")
            .get_current_or_default()
    }

    /// `profile[key]` of the profile dict (`KeyError` is a panic)
    fn _profile_key(profile: &Value, key: &str) -> Value {
        profile
            .get(key)
            .cloned()
            .unwrap_or_else(|| panic!("KeyError: '{key}'"))
    }

    /// `self._callback.on_comm_temperature_update(self.last_temperature.tools, .bed, .chamber, .custom)`,
    /// which Python writes out in each handler (OURS: one method, as the closure of `_monitor`).
    fn _notify_temperature_update(&self) {
        let tools = self.last_temperature.with(|t| t.tools());
        let bed = self.last_temperature.with(|t| t.bed());
        let chamber = self.last_temperature.with(|t| t.chamber());
        let custom = self.last_temperature.with(|t| t.custom());
        self._callback
            .on_comm_temperature_update(&tools, bed, chamber, &custom);
    }

    /// `int(match.group("value"))` (a `ValueError` that nothing catches is a panic, difference 3)
    fn _int_group(m: &fancy_regex::Captures) -> i64 {
        py_int(m.name("value").expect("group value").as_str())
            .expect("ValueError: invalid literal for int() with base 10")
    }

    /// `float(match.group("value"))`: `None` is `ValueError`
    fn _float_group(m: &fancy_regex::Captures) -> Option<f64> {
        py_float(m.name("value").expect("group value").as_str())
    }

    /// OURS (difference 30): `hasattr(self, "_gcode_" + gcode + "_" + phase)` and the call. `None`
    /// is "no such handler"; `Some(result)` is what the handler returned (`None` / `Some(..)`).
    fn _gcode_handler(
        self: &Arc<Self>,
        gcode: &str,
        phase: &str,
        cmd: &str,
    ) -> Option<Option<HandlerResults>> {
        Some(match (gcode, phase) {
            ("T", "queuing") => self._gcode_T_queuing(cmd),
            ("T", "sending") => self._gcode_T_sending(cmd),
            ("T", "sent") => self._gcode_T_sent(cmd),
            ("G0", "sent") | ("G1", "sent") => self._gcode_G0_sent(cmd),
            ("G28", "sent") => self._gcode_G28_sent(cmd),
            ("M28", "sent") => self._gcode_M28_sent(cmd),
            ("M29", "sent") => self._gcode_M29_sent(cmd),
            ("M140", "queuing") | ("M190", "queuing") => self._gcode_M140_queuing(cmd),
            ("M141", "queuing") | ("M191", "queuing") => self._gcode_M141_queuing(cmd),
            ("M104", "sent") => self._gcode_M104_sent(cmd, false, false),
            ("M140", "sent") => self._gcode_M140_sent(cmd, false, false),
            ("M141", "sent") => self._gcode_M141_sent(cmd, false, false),
            ("M109", "sent") => self._gcode_M109_sent(cmd),
            ("M190", "sent") => self._gcode_M190_sent(cmd),
            ("M191", "sent") => self._gcode_M191_sent(cmd),
            ("M116", "sent") => self._gcode_M116_sent(cmd),
            ("M155", "sending") => self._gcode_M155_sending(cmd),
            ("M27", "sending") => self._gcode_M27_sending(cmd),
            ("M154", "sending") => self._gcode_M154_sending(cmd),
            ("M33", "sending") => self._gcode_M33_sending(cmd),
            ("M110", "sending") => self._gcode_M110_sending(cmd),
            ("M112", "queuing") => self._gcode_M112_queuing(),
            ("M114", "queued") | ("M114", "sent") => self._gcode_M114_queued(),
            ("G4", "sent") => self._gcode_G4_sent(cmd),
            ("M106", "sent") => self._gcode_M106_sent(cmd),
            ("M107", "sent") => self._gcode_M107_sent(cmd),
            _ => return None,
        })
    }

    /// comm.py:5131 `_gcode_T_queuing`
    pub fn _gcode_T_queuing(&self, cmd: &str) -> Option<HandlerResults> {
        let tool_match = re_captures(&regexes_parameters["intT"], cmd);
        if let Some(tool_match) = tool_match {
            let current_tool = self._currentTool.get();
            let new_tool = Self::_int_group(&tool_match);

            if !self._validate_tool(new_tool) {
                let message = Self::not_sending_t("queuing", new_tool);
                self._log(&format!("Warn: {message}"));
                event_manager().fire(
                    Events::COMMAND_SUPPRESSED,
                    Some(
                        serde_json::json!({"command": cmd, "message": message, "severity": "warn"}),
                    ),
                );
                return Some(none_tuple());
            }

            let replacements = serde_json::json!({"tool": {"old": current_tool, "new": new_tool}});
            let before = self._get_gcode_script("beforeToolChange", Some(&replacements));
            let after = self._get_gcode_script("afterToolChange", Some(&replacements));

            // `convert(data)`: a 2-tuple `(line, tags)` becomes `(line, None, tags)`
            let convert = |data: Vec<(String, BTreeSet<String>)>| -> Vec<HandlerResultEntry> {
                data.into_iter()
                    .map(|(line, tags)| {
                        HandlerResultEntry::Tuple(HandlerTuple::Three(Some(line), None, Some(tags)))
                    })
                    .collect()
            };

            let mut result = convert(before);
            result.push(HandlerResultEntry::Str(cmd.to_string()));
            result.extend(convert(after));
            return Some(HandlerResults::List(result));
        }
        None
    }

    /// comm.py:5170 `_gcode_T_sending`
    pub fn _gcode_T_sending(&self, cmd: &str) -> Option<HandlerResults> {
        let tool_match = re_captures(&regexes_parameters["intT"], cmd);
        if let Some(tool_match) = tool_match {
            let new_tool = Self::_int_group(&tool_match);
            if !self._validate_tool(new_tool) {
                let message = Self::not_sending_t("sending", new_tool);
                self._log(&format!("Warn: {message}"));
                // self._logger.warning(message)
                event_manager().fire(
                    Events::COMMAND_SUPPRESSED,
                    Some(
                        serde_json::json!({"command": cmd, "message": message, "severity": "warn"}),
                    ),
                );
                return Some(none_tuple());
            }
        }
        None
    }

    /// comm.py:5189 `_gcode_T_sent`
    pub fn _gcode_T_sent(&self, cmd: &str) -> Option<HandlerResults> {
        let tool_match = re_captures(&regexes_parameters["intT"], cmd);
        if let Some(tool_match) = tool_match {
            let new_tool = Self::_int_group(&tool_match);
            self._toolBeforeChange.set(Some(self._currentTool.get()));
            self._currentTool.set(new_tool);
            event_manager().fire(
                Events::TOOL_CHANGE,
                Some(serde_json::json!({"old": self._toolBeforeChange.get(), "new": self._currentTool.get()})),
            );
        }
        None
    }

    /// comm.py:5200 `_gcode_G0_sent` (also `_gcode_G1_sent`, :5222)
    pub fn _gcode_G0_sent(&self, cmd: &str) -> Option<HandlerResults> {
        if cmd.contains('Z') || cmd.contains('F') {
            // track Z
            if let Some(m) = re_captures(&regexes_parameters["floatZ"], cmd) {
                // try: ... except ValueError: pass
                if let Some(z) = Self::_float_group(&m) {
                    if self._currentZ.get() != Some(z) {
                        self._currentZ.set(Some(z));
                        self._callback.on_comm_z_change(Some(z));
                    }
                }
            }

            // track F
            if let Some(m) = re_captures(&regexes_parameters["floatF"], cmd) {
                if let Some(f) = Self::_float_group(&m) {
                    self._currentF.set(Some(f));
                }
            }
        }
        None
    }

    /// comm.py:5224 `_gcode_G28_sent`
    pub fn _gcode_G28_sent(&self, cmd: &str) -> Option<HandlerResults> {
        if cmd.contains('F') {
            if let Some(m) = re_captures(&regexes_parameters["floatF"], cmd) {
                if let Some(f) = Self::_float_group(&m) {
                    self._currentF.set(Some(f));
                }
            }
        }
        None
    }

    /// comm.py:5236 `_gcode_M28_sent`
    pub fn _gcode_M28_sent(&self, _cmd: &str) -> Option<HandlerResults> {
        if !self.is_streaming() {
            self._log(
                "Detected manual streaming. Disabling temperature polling. Finish writing with M29. \
                 Do NOT attempt to print while manually streaming!",
            );
            self._manualStreaming.set(true);
        }
        None
    }

    /// comm.py:5246 `_gcode_M29_sent`
    pub fn _gcode_M29_sent(&self, _cmd: &str) -> Option<HandlerResults> {
        if self._manualStreaming.get() {
            self._log("Manual streaming done. Re-enabling temperature polling. All is well.");
            self._manualStreaming.set(false);
        }
        None
    }

    /// comm.py:5254 `_gcode_M140_queuing` (also `_gcode_M190_queuing`, :5273)
    pub fn _gcode_M140_queuing(&self, cmd: &str) -> Option<HandlerResults> {
        let profile = self._current_profile();
        if !py_truthy(&Self::_profile_key(&profile, "heatedBed")) {
            let message = format!(
                "Not sending \"{cmd}\", printer profile has no heated bed. Either \
                 configure a heated bed or remove bed commands from your \
                 GCODE."
            );
            self._log(&format!("Warn: {message}"));
            // self._logger.warning(message)
            event_manager().fire(
                Events::COMMAND_SUPPRESSED,
                Some(serde_json::json!({"command": cmd, "message": message, "severity": "warn"})),
            );
            return Some(none_tuple()); // Don't send bed commands if we don't have a heated bed
        }
        None
    }

    /// comm.py:5275 `_gcode_M141_queuing` (also `_gcode_M191_queuing`, :5294)
    pub fn _gcode_M141_queuing(&self, cmd: &str) -> Option<HandlerResults> {
        let profile = self._current_profile();
        if !py_truthy(&Self::_profile_key(&profile, "heatedChamber")) {
            let message = format!(
                "Not sending \"{cmd}\", printer profile has no heated chamber. Either \
                 configure a heated chamber or remove chamber commands from your \
                 GCODE."
            );
            self._log(&format!("Warn: {message}"));
            // self._logger.warning(message)
            event_manager().fire(
                Events::COMMAND_SUPPRESSED,
                Some(serde_json::json!({"command": cmd, "message": message, "severity": "warn"})),
            );
            return Some(none_tuple()); // Don't send chamber commands if we don't have a heated chamber
        }
        None
    }

    /// comm.py:5296 `_gcode_M104_sent(cmd, cmd_type=None, gcode=None, subcode=None, wait=False, support_r=False)`
    pub fn _gcode_M104_sent(
        &self,
        cmd: &str,
        wait: bool,
        support_r: bool,
    ) -> Option<HandlerResults> {
        let mut tool_num = self._currentTool.get();

        if let Some(tool_match) = re_captures(&regexes_parameters["intT"], cmd) {
            tool_num = Self::_int_group(&tool_match);

            if wait {
                self._toolBeforeHeatup.set(Some(self._currentTool.get()));
                self._currentTool.set(tool_num);
            }
        }

        let mut m = re_captures(&regexes_parameters["floatS"], cmd);
        if m.is_none() && support_r {
            m = re_captures(&regexes_parameters["floatR"], cmd);
        }

        if let Some(m) = m {
            if self
                .last_temperature
                .with(|t| t.tools().iter().any(|(k, _)| *k == tool_num))
            {
                // try: ... except ValueError: pass
                if let Some(target) = Self::_float_group(&m) {
                    self.last_temperature
                        .with_mut(|t| t.set_tool(tool_num, None, Some(target)));
                    self._notify_temperature_update();
                }
            }
        }
        None
    }

    /// comm.py:5326 `_gcode_M140_sent(cmd, cmd_type=None, gcode=None, subcode=None, wait=False, support_r=False)`
    pub fn _gcode_M140_sent(
        &self,
        cmd: &str,
        _wait: bool,
        support_r: bool,
    ) -> Option<HandlerResults> {
        let mut m = re_captures(&regexes_parameters["floatS"], cmd);
        if m.is_none() && support_r {
            m = re_captures(&regexes_parameters["floatR"], cmd);
        }

        if let Some(m) = m {
            if let Some(target) = Self::_float_group(&m) {
                self.last_temperature
                    .with_mut(|t| t.set_bed(None, Some(target)));
                self._notify_temperature_update();
            }
        }
        None
    }

    /// comm.py:5352 `_gcode_M141_sent(cmd, cmd_type=None, gcode=None, subcode=None, wait=False, support_r=False)`
    pub fn _gcode_M141_sent(
        &self,
        cmd: &str,
        _wait: bool,
        support_r: bool,
    ) -> Option<HandlerResults> {
        let mut m = re_captures(&regexes_parameters["floatS"], cmd);
        if m.is_none() && support_r {
            m = re_captures(&regexes_parameters["floatR"], cmd);
        }

        if let Some(m) = m {
            if let Some(target) = Self::_float_group(&m) {
                self.last_temperature
                    .with_mut(|t| t.set_chamber(None, Some(target)));
                self._notify_temperature_update();
            }
        }
        None
    }

    /// comm.py:5378 `_gcode_M109_sent`
    pub fn _gcode_M109_sent(&self, cmd: &str) -> Option<HandlerResults> {
        self._heatupWaitStartTime.set(Some(time_monotonic()));
        self._long_running_command.set(true);
        self._heating.set(true);
        self._gcode_M104_sent(cmd, true, true)
    }

    /// comm.py:5386 `_gcode_M190_sent`
    pub fn _gcode_M190_sent(&self, cmd: &str) -> Option<HandlerResults> {
        self._heatupWaitStartTime.set(Some(time_monotonic()));
        self._long_running_command.set(true);
        self._heating.set(true);
        self._gcode_M140_sent(cmd, true, true)
    }

    /// comm.py:5394 `_gcode_M191_sent`
    pub fn _gcode_M191_sent(&self, cmd: &str) -> Option<HandlerResults> {
        self._heatupWaitStartTime.set(Some(time_monotonic()));
        self._long_running_command.set(true);
        self._heating.set(true);
        self._gcode_M141_sent(cmd, true, true)
    }

    /// comm.py:5420 `_gcode_M116_sent`
    pub fn _gcode_M116_sent(&self, _cmd: &str) -> Option<HandlerResults> {
        self._heatupWaitStartTime.set(Some(time_monotonic()));
        self._long_running_command.set(true);
        self._heating.set(true);
        None
    }

    /// comm.py:5427 `_gcode_M155_sending`
    pub fn _gcode_M155_sending(&self, cmd: &str) -> Option<HandlerResults> {
        if let Some(m) = re_captures(&regexes_parameters["intS"], cmd) {
            let interval = Self::_int_group(&m);
            self._temperature_autoreporting.set(
                self._firmware_capabilities
                    .with(|d| d.get(Self::CAPABILITY_AUTOREPORT_TEMP).copied())
                    .unwrap_or(false)
                    && (interval > 0),
            );
        }
        None
    }

    /// comm.py:5437 `_gcode_M27_sending`
    pub fn _gcode_M27_sending(&self, cmd: &str) -> Option<HandlerResults> {
        if let Some(m) = re_captures(&regexes_parameters["intS"], cmd) {
            let interval = Self::_int_group(&m);
            self._sdstatus_autoreporting.set(
                self._firmware_capabilities
                    .with(|d| d.get(Self::CAPABILITY_AUTOREPORT_SD_STATUS).copied())
                    .unwrap_or(false)
                    && (interval > 0),
            );
        }
        None
    }

    /// comm.py:5447 `_gcode_M154_sending`
    pub fn _gcode_M154_sending(&self, cmd: &str) -> Option<HandlerResults> {
        if let Some(m) = re_captures(&regexes_parameters["intS"], cmd) {
            let interval = Self::_int_group(&m);
            self._pos_autoreporting.set(
                self._firmware_capabilities
                    .with(|d| d.get(Self::CAPABILITY_AUTOREPORT_POS).copied())
                    .unwrap_or(false)
                    && (interval > 0),
            );
        }
        None
    }

    /// comm.py:5457 `_gcode_M33_sending`
    pub fn _gcode_M33_sending(&self, cmd: &str) -> Option<HandlerResults> {
        let parts = py_split_none(cmd, 1);
        if parts.len() > 1 {
            let mut filename = py_strip(parts[1]).to_string();
            if !filename.starts_with('/') {
                filename = format!("/{filename}");
            }
            self._sdFileLongName.set(Some(filename));
        }
        None
    }

    /// comm.py:5467 `_gcode_M110_sending`
    pub fn _gcode_M110_sending(&self, cmd: &str) -> Option<HandlerResults> {
        let mut new_line_number: i64 = 0;
        if let Some(m) = re_captures(&regexes_parameters["intN"], cmd) {
            new_line_number = Self::_int_group(&m);
        }

        {
            let _line = self._line_mutex.lock();
            // self._logger.info(f"M110 detected, setting current line number to {newLineNumber}")

            // send M110 command with new line number
            self._current_line.set(new_line_number);

            // after a reset of the line number we have no way to determine what line exactly the printer now wants
            self._lastLines.with_mut(|l| l.clear());
        }
        self._resendDelta.set(None);
        None
    }

    /// comm.py:5487 `_trigger_emergency_stop(close=True)`
    pub fn _trigger_emergency_stop(self: &Arc<Self>, close: bool) {
        // self._logger.info("Force-sending M112 to the printer")

        // emergency stop, jump the queue with the M112, regardless of whether the EMERGENCY_PARSER capability is
        // available or not
        //
        // send the M112 once without and with checksum
        self._do_send_without_checksum(b"M112", true);
        self._do_increment_and_send_with_checksum(b"M112");

        // No idea if the printer is still listening or if M112 won. Just in case
        // we'll now try to also manually make sure all heaters are shut off - better
        // safe than sorry. We do this ignoring the queue since at this point it
        // is irrelevant whether the printer has sent enough ack's or not, we
        // are going to shutdown the connection in a second anyhow.
        let profile = self._current_profile();
        let count = Self::_profile_key(&Self::_profile_key(&profile, "extruder"), "count")
            .as_i64()
            .expect("TypeError: 'range' object cannot be interpreted as an integer");
        for tool in 0..count {
            let command = format!("M104 T{tool} S0");
            self._do_increment_and_send_with_checksum(&encode_serial_text(
                &command,
                &self._serial_encoding,
                false,
            ));
        }
        if py_truthy(&Self::_profile_key(&self._current_profile(), "heatedBed")) {
            self._do_increment_and_send_with_checksum(b"M140 S0");
        }

        if close {
            // close to reset host state
            let error_text = "Closing serial port due to emergency stop M112.";
            self._log(error_text);

            self._errorValue.set(error_text.to_string());
            self.close(true, true, Some(10.0));
        }

        // fire the M112 event since we sent it and we're going to prevent the caller from seeing it
        if let Some(event) = gcode_to_event("M112") {
            event_manager().fire(event, None);
        }
    }

    /// comm.py:5524 `_gcode_M112_queuing`
    pub fn _gcode_M112_queuing(self: &Arc<Self>) -> Option<HandlerResults> {
        self._trigger_emergency_stop(true);
        Some(none_tuple())
    }

    /// comm.py:5528 `_gcode_M114_queued` (also `_gcode_M114_sent`, :5531)
    pub fn _gcode_M114_queued(&self) -> Option<HandlerResults> {
        self._reset_position_timers();
        None
    }

    /// comm.py:5533 `_gcode_G4_sent`
    pub fn _gcode_G4_sent(&self, cmd: &str) -> Option<HandlerResults> {
        // we are intending to dwell for a period of time, increase the timeout to match
        let p_match = re_captures(&regexes_parameters["floatP"], cmd);
        let s_match = re_captures(&regexes_parameters["floatS"], cmd);

        let mut timeout = 0.0;
        if let Some(p) = p_match {
            timeout = Self::_float_group(&p)
                .expect("ValueError: could not convert string to float")
                / 1000.0;
        } else if let Some(s) = s_match {
            timeout =
                Self::_float_group(&s).expect("ValueError: could not convert string to float");
        }

        self._timeout
            .set(Some(self._get_new_communication_timeout() + timeout));
        self._dwelling_until.set(Some(time_monotonic() + timeout));
        None
    }

    /// comm.py:5548 `_gcode_M106_sent`
    pub fn _gcode_M106_sent(&self, cmd: &str) -> Option<HandlerResults> {
        let mut fanspeed = 0;
        if let Some(s) = re_captures(&regexes_parameters["intS"], cmd) {
            fanspeed = Self::_int_group(&s);
        }

        self.last_fanspeed.set(Some(fanspeed));
        None
    }

    /// comm.py:5560 `_gcode_M107_sent`
    pub fn _gcode_M107_sent(&self, _cmd: &str) -> Option<HandlerResults> {
        self.last_fanspeed.set(Some(0));
        None
    }

    /// comm.py:5565 `_emergency_force_send(cmd, message, gcode=None)`
    pub fn _emergency_force_send(
        self: &Arc<Self>,
        cmd: &str,
        _message: &str,
        gcode: Option<&str>,
    ) -> Option<HandlerResults> {
        // only jump the queue with our command if the EMERGENCY_PARSER capability is available
        if !self._capability_supported(Self::CAPABILITY_EMERGENCY_PARSER) {
            return None;
        }

        // self._logger.info(message)

        // use up an ok since we will get one back for this command and don't want to get out of sync
        let used_up_clear = self._use_up_clear(gcode);
        self._do_send(cmd, gcode);
        if !used_up_clear {
            self._continue_sending();
        }

        Some(none_tuple())
    }

    /// comm.py:5580 `_validate_tool(tool)`
    pub fn _validate_tool(&self, tool: i64) -> bool {
        !truthy(self._sanity_check_tools)
            || (tool
                < Self::_profile_key(
                    &Self::_profile_key(&self._current_profile(), "extruder"),
                    "count",
                )
                .as_i64()
                .expect("TypeError: '<' not supported between instances of 'int' and a non-int")
                && !self
                    ._known_invalid_tools
                    .with(|s| s.contains(&tool.to_string())))
    }

    /// comm.py:5587 `_reset_position_timers()`
    pub fn _reset_position_timers(&self) {
        if let Some(timer) = self._cancel_position_timer.get() {
            timer.reset(None);
        }
        if let Some(timer) = self._pause_position_timer.get() {
            timer.reset(None);
        }
    }

    // ~~ atcommands

    /// comm.py:5595 `_atcommand_pause_queuing(command, parameters, tags=None)`
    pub fn _atcommand_pause_queuing(
        self: &Arc<Self>,
        _command: &str,
        _parameters: &str,
        tags: &Option<Tags>,
    ) {
        let not_in = tags
            .as_ref()
            .is_none_or(|t| !t.borrow().contains("script:afterPrintPaused"));
        if not_in {
            self.set_pause(
                true,
                true,
                None,
                Some(with_tags(&BTreeSet::new(), &["trigger:atcommand_pause"])),
            );
        }
    }

    /// comm.py:5602 `_atcommand_cancel_queuing(command, parameters, tags=None)` (also `_atcommand_abort_queuing`, :5608)
    pub fn _atcommand_cancel_queuing(
        self: &Arc<Self>,
        _command: &str,
        _parameters: &str,
        tags: &Option<Tags>,
    ) {
        let not_in = tags
            .as_ref()
            .is_none_or(|t| !t.borrow().contains("script:afterPrintCancelled"));
        if not_in {
            self.cancel_print(
                None,
                false,
                None,
                Some(with_tags(&BTreeSet::new(), &["trigger:atcommand_cancel"])),
                false,
            );
        }
    }

    /// comm.py:5610 `_atcommand_resume_queuing(command, parameters, tags=None)`
    pub fn _atcommand_resume_queuing(
        self: &Arc<Self>,
        _command: &str,
        _parameters: &str,
        tags: &Option<Tags>,
    ) {
        let not_in = tags
            .as_ref()
            .is_none_or(|t| !t.borrow().contains("script:beforePrintResumed"));
        if not_in {
            self.set_pause(
                false,
                true,
                None,
                Some(with_tags(&BTreeSet::new(), &["trigger:atcommand_resume"])),
            );
        }
    }

    // ~~ command phase handlers

    /// comm.py:5617 `_command_phase_queuing(cmd, cmd_type=None, gcode=None, subcode=None, tags=...)`
    pub fn _command_phase_queuing(
        self: &Arc<Self>,
        cmd: &str,
        gcode: Option<&str>,
        tags: &Option<Tags>,
    ) -> Option<HandlerResults> {
        if let Some(gcode) = gcode {
            let tags: BTreeSet<String> = tags
                .as_ref()
                .map(|t| t.borrow().clone())
                .unwrap_or_default();

            if py_in_list(&self._emergency_commands, Some(gcode)) && gcode != "M112" {
                return self._emergency_force_send(
                    cmd,
                    &format!("Force-sending {gcode} to the printer"),
                    Some(gcode),
                );
            }

            if self.is_printing()
                && py_in_list(&self._pausing_commands, Some(gcode))
                && !tags.contains("trigger:cancel")
                && !tags.contains("trigger:pause")
            {
                // self._logger.info(f"Pausing print job due to command {gcode}")
                self.set_pause(true, true, None, None);
            }

            if py_in_list(&self._blocked_commands, Some(gcode)) {
                let message = format!(
                    "Not sending {gcode} to printer as it's configured as a blocked command"
                );
                self._log(&format!("Info: {message}"));
                // self._logger.info(message)
                event_manager().fire(
                    Events::COMMAND_SUPPRESSED,
                    Some(
                        serde_json::json!({"command": cmd, "message": message, "severity": "warn"}),
                    ),
                );
                return Some(none_tuple());
            }

            if py_in_list(&self._ignored_commands, Some(gcode)) {
                let message = format!(
                    "Not sending {gcode} to printer as it's configured as an ignored command"
                );
                self._log(&format!("Info: {message}"));
                // self._logger.info(message)
                event_manager().fire(
                    Events::COMMAND_SUPPRESSED,
                    Some(
                        serde_json::json!({"command": cmd, "message": message, "severity": "info"}),
                    ),
                );
                return Some(none_tuple());
            }
        }
        None
    }

    /// comm.py:5675 `_command_phase_sending(cmd, cmd_type=None, gcode=None, subcode=None)`
    pub fn _command_phase_sending(&self, cmd: &str, gcode: Option<&str>) {
        if (gcode.is_some() && py_in_list(&self._long_running_commands, gcode))
            || py_in_list(&self._long_running_commands, Some(cmd))
        {
            self._long_running_command.set(true);
        }
    }
}

impl Drop for MachineCom {
    /// comm.py:853 `__del__`: `self.close()` (difference 6)
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.close_impl(None, false, true, Some(10.0), None);
        }
    }
}

#[cfg(test)]
mod tests {
    //! Ports of `tests/util/test_comm_helpers.py` (`TestCommHelpers`) for the functions above.
    //! Each `@data` table is one test looping over the same cases.
    use super::*;

    fn s(x: &str) -> String {
        x.to_string()
    }

    fn offs(items: &[(&str, f64)]) -> TemperatureOffsets {
        items.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn test_strip_comment() {
        let cases = [
            ("M117 Test", "M117 Test"),
            ("M117 Test ; foo", "M117 Test "),
            ("M117 Test \\; foo", "M117 Test \\; foo"),
            ("M117 Test \\\\; foo", "M117 Test \\\\"),
            ("M117 Test \\\\\\; foo", "M117 Test \\\\\\; foo"),
            ("; foo", ""),
        ];
        for (input, expected) in cases {
            assert_eq!(expected, strip_comment(input), "{input:?}");
        }
    }

    #[test]
    fn test_process_gcode_line() {
        let empty = offs(&[]);
        let cases: [(&str, Option<&TemperatureOffsets>, Option<i64>, Option<&str>); 4] = [
            ("M117 Test", None, None, Some("M117 Test")),
            ("", None, None, None),
            ("  \t \r    \n", None, None, None),
            ("M117 Test", Some(&empty), Some(0), Some("M117 Test")),
        ];
        for (input, offsets, current_tool, expected) in cases {
            assert_eq!(
                expected.map(s),
                process_gcode_line(input, offsets, current_tool),
                "{input:?}"
            );
        }
    }

    #[test]
    fn test_apply_temperature_offsets() {
        let t = offs(&[("tool0", 10.0), ("tool1", 20.0), ("bed", 30.0)]);
        let tc = offs(&[
            ("tool0", 10.0),
            ("tool1", 20.0),
            ("bed", 30.0),
            ("chamber", 40.0),
        ]);
        let empty = offs(&[]);
        let t0 = offs(&[("tool0", 0.0)]);
        let t20 = offs(&[("tool0", 20.0)]);
        let cases: Vec<(&str, Option<&TemperatureOffsets>, Option<i64>, Option<f64>)> = vec![
            ("M104 S200", None, None, None),
            ("M117 Test", Some(&empty), None, None),
            ("M104 T0", Some(&empty), None, None),
            ("M104 S220", Some(&t), Some(0), Some(230.0)),
            ("M104 T1 S220", Some(&t), Some(0), Some(240.0)),
            ("M104 S220", Some(&t), Some(1), Some(240.0)),
            ("M140 S100", Some(&t), Some(1), Some(130.0)),
            ("M190 S100", Some(&t), Some(1), Some(130.0)),
            ("M109 S220", Some(&t), Some(0), Some(230.0)),
            ("M141 S100", Some(&tc), Some(0), Some(140.0)),
            ("M191 S100", Some(&tc), Some(0), Some(140.0)),
            ("M109 S220", Some(&empty), Some(0), None),
            ("M104 S220", Some(&t0), Some(0), None),
            ("M104 S220", Some(&t20), None, None),
            ("M104 S0", Some(&t20), Some(0), None),
            ("M140 S100", Some(&empty), Some(0), None),
            ("M141 S100", Some(&empty), Some(0), None),
        ];
        let search = Regex::new(r"S(\d+(\.\d+)?)").unwrap();
        for (input, offsets, current_tool, expected) in cases {
            let actual = apply_temperature_offsets(input, offsets, current_tool);

            match expected {
                None => assert_eq!(input, actual, "{input:?}"),
                Some(expected) => {
                    let m = search
                        .captures(&actual)
                        .unwrap()
                        .expect("No temperature found");
                    let g1 = m.get(1).unwrap();
                    let temperature: f64 = g1.as_str().parse().unwrap();
                    assert_eq!(expected, temperature, "{input:?}");
                    assert_eq!(
                        input.get(..g1.start()).unwrap_or(input),
                        &actual[..g1.start()]
                    );
                    // (Python slicing past the end gives "")
                    assert_eq!(input.get(g1.end()..).unwrap_or(""), &actual[g1.end()..]);
                }
            }
        }
    }

    // Not in the Python tests: the case of a repeated group (`S220 T1`), where Python keeps
    // the value of `temperature` from an earlier iteration.
    #[test]
    fn apply_temperature_offsets_repeated_group_keeps_earlier_value() {
        let t = offs(&[("tool0", 10.0), ("tool1", 20.0)]);
        assert_eq!(
            "M104 S240.000000 T1",
            apply_temperature_offsets("M104 S220 T1", Some(&t), Some(0))
        );
    }

    fn trigger(items: &[(&str, &str)]) -> BTreeMap<String, String> {
        items
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn test_convert_pause_triggers() {
        let configured_triggers = vec![
            trigger(&[("regex", "pause1"), ("type", "enable")]),
            trigger(&[("regex", "pause2"), ("type", "enable")]),
            trigger(&[("regex", "resume"), ("type", "disable")]),
            trigger(&[("regex", "toggle"), ("type", "toggle")]),
            trigger(&[("type", "enable")]),
            trigger(&[("regex", "regex")]),
            trigger(&[("regex", "regex"), ("type", "unknown")]),
        ];

        let trigger_matchers = convert_pause_triggers(&configured_triggers).unwrap();

        assert!(trigger_matchers.contains_key("enable"));
        assert_eq!("(pause1)|(pause2)", trigger_matchers["enable"].as_str());

        assert!(trigger_matchers.contains_key("disable"));
        assert_eq!("(resume)", trigger_matchers["disable"].as_str());

        assert!(trigger_matchers.contains_key("toggle"));
        assert_eq!("(toggle)", trigger_matchers["toggle"].as_str());

        assert!(!trigger_matchers.contains_key("unknown"));
    }

    /// `hashlib.md5(input).hexdigest()`, for the test below (the `md-5` crate).
    fn md5sum(input: &[u8]) -> String {
        use md5::{Digest, Md5};
        Md5::digest(input)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn md5sum_known_vectors() {
        assert_eq!("d41d8cd98f00b204e9800998ecf8427e", md5sum(b""));
        assert_eq!("900150983cd24fb0d6963f7d28e17f72", md5sum(b"abc"));
    }

    #[test]
    fn test_convert_feedback_controls() {
        // rb'' doesn't exist in Python2
        let temp_regex: &[u8] = br"T:((\d*\.)\d+)";
        let temp_template: &[u8] = b"Temp: {}";
        let temp2_template: &[u8] = b"Temperature: {}";
        let temp_key = md5sum(temp_regex);
        let temp_template_key = md5sum(temp_template);
        let temp2_template_key = md5sum(temp2_template);

        let x_regex: &[u8] = br"X:(?P<x>\d+)";
        let x_template: &[u8] = b"X: {x}";
        let x_key = md5sum(x_regex);
        let x_template_key = md5sum(x_template);

        let bytes = |b: &[u8]| Some(PyText::Bytes(b.to_vec()));

        let configured_controls = vec![
            FeedbackControl {
                key: Some(temp_key.clone()),
                regex: bytes(temp_regex),
                template: bytes(temp_template),
                template_key: Some(temp_template_key.clone()),
                children: None,
            },
            // {"command": "M117 Hello World", "name": "Test"}
            FeedbackControl::default(),
            FeedbackControl {
                children: Some(vec![
                    FeedbackControl {
                        key: Some(x_key.clone()),
                        regex: bytes(x_regex),
                        template: bytes(x_template),
                        template_key: Some(x_template_key.clone()),
                        children: None,
                    },
                    FeedbackControl {
                        key: Some(temp_key.clone()),
                        regex: bytes(temp_regex),
                        template: bytes(temp2_template),
                        template_key: Some(temp2_template_key.clone()),
                        children: None,
                    },
                ]),
                ..Default::default()
            },
        ];

        let (controls, matcher) = convert_feedback_controls(&configured_controls).unwrap();
        let matcher = matcher.unwrap();

        assert_eq!(2, controls.len());

        // temp_regex is used twice, so we should have two templates for it
        assert!(controls.contains_key(&temp_key));
        let temp = controls.get(&temp_key).unwrap();

        assert!(temp.matcher.is_some());
        assert_eq!(
            temp_regex,
            temp.matcher.as_ref().unwrap().as_str().as_bytes()
        );
        assert_eq!(Some(PyText::Bytes(temp_regex.to_vec())), temp.pattern);

        assert_eq!(2, temp.templates.len());
        assert!(temp.templates.contains_key(&temp_template_key));
        assert_eq!(
            PyText::Bytes(temp_template.to_vec()),
            *temp.templates.get(&temp_template_key).unwrap()
        );
        assert!(temp.templates.contains_key(&temp2_template_key));
        assert_eq!(
            PyText::Bytes(temp2_template.to_vec()),
            *temp.templates.get(&temp2_template_key).unwrap()
        );

        // x_regex is used once, so we should have only one template for it
        assert!(controls.contains_key(&x_key));
        let x = controls.get(&x_key).unwrap();

        assert!(x.matcher.is_some());
        assert_eq!(x_regex, x.matcher.as_ref().unwrap().as_str().as_bytes());
        assert_eq!(Some(PyText::Bytes(x_regex.to_vec())), x.pattern);

        assert_eq!(1, x.templates.len());
        assert!(x.templates.contains_key(&x_template_key));
        assert_eq!(
            PyText::Bytes(x_template.to_vec()),
            *x.templates.get(&x_template_key).unwrap()
        );

        // f"(?P<group{temp_key}>{temp_regex})|(?P<group{x_key}>{x_regex})": in Python an
        // f-string puts the repr of a bytes object into the text, spelled out here.
        assert_eq!(
            format!(
                r"(?P<group{temp_key}>b'T:((\\d*\\.)\\d+)')|(?P<group{x_key}>b'X:(?P<x>\\d+)')"
            ),
            matcher.as_str()
        );
    }

    #[test]
    fn test_parameter_regexes() {
        let cases: [(&str, &str, bool, Option<&str>); 10] = [
            ("G4 P2.0", "floatP", true, Some("2.0")),
            ("M109 S220.0", "floatS", true, Some("220.0")),
            ("G1 X10.0 Y10.0 Z0.2", "floatZ", true, Some("0.2")),
            ("G1X10.0Y10.0Z0.2", "floatZ", true, Some("0.2")),
            ("g1x10.0y10.0z0.2", "floatZ", true, Some("0.2")),
            ("M110 N0", "intN", true, Some("0")),
            ("M104 S220.0 T1", "intT", true, Some("1")),
            ("M104 T1 S220.0", "intT", true, Some("1")),
            ("N100 M110", "intN", true, Some("100")),
            ("NP100", "floatP", false, None),
        ];
        for (line, parameter, should_match, expected_value) in cases {
            let regex = &regexes_parameters[parameter];
            let m = regex.captures(line).unwrap();

            if should_match {
                let m = m.unwrap_or_else(|| panic!("no match for {line:?}"));
                assert_eq!(
                    expected_value,
                    Some(m.name("value").unwrap().as_str()),
                    "{line:?}"
                );
            } else {
                assert!(m.is_none(), "{line:?}");
            }
        }
    }

    #[test]
    fn test_gcode_command_for_cmd() {
        let cases: [(Option<&str>, Option<&str>); 8] = [
            (Some("G0 X0"), Some("G0")),
            (Some("G28 X0 Y0"), Some("G28")),
            (Some("M109 S220.0 T1"), Some("M109")),
            (Some("M117 Hello World"), Some("M117")),
            (Some("T0"), Some("T")),
            (Some("T3"), Some("T")),
            (None, None),
            (Some("No match"), None),
        ];
        for (cmd, expected) in cases {
            assert_eq!(expected.map(s), gcode_command_for_cmd(cmd), "{cmd:?}");
        }
    }

    #[test]
    fn test_gcode_and_subcode_for_cmd() {
        let cases: [(Option<&str>, Option<&str>, Option<&str>); 9] = [
            (Some("G0 X0"), Some("G0"), None),
            (Some("M105"), Some("M105"), None),
            (Some("T2"), Some("T"), None),
            (Some("M80.1"), Some("M80"), Some("1")),
            (Some("G28.2"), Some("G28"), Some("2")),
            (Some("T0.3"), Some("T"), None),
            (Some("M80.nosubcode"), Some("M80"), None),
            (None, None, None),
            (Some("No match"), None, None),
        ];
        for (cmd, expected_gcode, expected_subcode) in cases {
            let (actual_gcode, actual_subcode) = gcode_and_subcode_for_cmd(cmd);
            assert_eq!(expected_gcode.map(s), actual_gcode, "{cmd:?}");
            assert_eq!(expected_subcode.map(s), actual_subcode, "{cmd:?}");
        }
    }

    fn temps(items: &[(&str, f64, Option<f64>)]) -> Temperatures {
        items
            .iter()
            .map(|(k, a, t)| (k.to_string(), (*a, *t)))
            .collect()
    }

    #[test]
    fn test_process_temperature_line() {
        let cases: Vec<(&str, i64, Temperatures, i64)> = vec![
            (
                "T:23.0 B:60.0",
                0,
                temps(&[("T0", 23.0, None), ("B", 60.0, None)]),
                0,
            ),
            (
                "T:23.0 B:60.0",
                1,
                temps(&[("T1", 23.0, None), ("B", 60.0, None)]),
                1,
            ),
            (
                "T:23.0/220.0 B:60.0/70.0",
                0,
                temps(&[("T0", 23.0, Some(220.0)), ("B", 60.0, Some(70.0))]),
                0,
            ),
            (
                "ok T:23.0/220.0 T0:23.0/220.0 T1:50.2/210.0 T2:39.4/220.0 B:60.0",
                0,
                temps(&[
                    ("T0", 23.0, Some(220.0)),
                    ("T1", 50.2, Some(210.0)),
                    ("T2", 39.4, Some(220.0)),
                    ("B", 60.0, None),
                ]),
                2,
            ),
            (
                "ok T:50.2/210.0 T0:23.0/220.0 T1:50.2/210.0 T2:39.4/220.0 B:60.0",
                1,
                temps(&[
                    ("T0", 23.0, Some(220.0)),
                    ("T1", 50.2, Some(210.0)),
                    ("T2", 39.4, Some(220.0)),
                    ("B", 60.0, None),
                ]),
                2,
            ),
            (
                "ok T:-55.7/0 T0:-55.7/0 T1:150.0/210.0",
                0,
                temps(&[("T0", -55.7, Some(0.0)), ("T1", 150.0, Some(210.0))]),
                1,
            ),
            (
                "ok T:150.0/210.0 T0:-55.7/0 T1:150.0/210.0",
                1,
                temps(&[("T0", -55.7, Some(0.0)), ("T1", 150.0, Some(210.0))]),
                1,
            ),
            (
                "T:210.04 /210.00 B:52.00 /52.00 @:85 B@:31pS_XYZ:5",
                0,
                temps(&[("T0", 210.04, Some(210.0)), ("B", 52.00, Some(52.0))]),
                0,
            ),
            (
                "T:210.04 /210.00 B:52.00 /52.00 @:85 31pS_XYZ:5",
                0,
                temps(&[
                    ("T0", 210.04, Some(210.0)),
                    ("B", 52.00, Some(52.0)),
                    ("31pS_XYZ", 5.0, None),
                ]),
                0,
            ),
            (
                "T:210.04 /210.00 B:52.00 /52.00 @:85 F0:255.0 /255.0",
                0,
                temps(&[
                    ("T0", 210.04, Some(210.0)),
                    ("B", 52.00, Some(52.0)),
                    ("F0", 255.0, Some(255.0)),
                ]),
                0,
            ),
            (
                "T:210.04 /210.00 @B:52.00 /52.00",
                0,
                temps(&[("T0", 210.04, Some(210.0))]),
                0,
            ),
            (
                "T:210.04 /210.00 @B:52.00 /52.00 TXYZ:2",
                0,
                temps(&[("T0", 210.04, Some(210.0)), ("TXYZ", 2.0, None)]),
                0,
            ),
            (
                // Only first occurrence of a sensor should be used, second B gets ignored
                "T:210.04 /210.00 B:52.00 /52.00 @:85 B:1234.0 /1234.0",
                0,
                temps(&[("T0", 210.04, Some(210.0)), ("B", 52.00, Some(52.0))]),
                0,
            ),
        ];
        for (line, current, expected_result, expected_max) in cases {
            let (maxtool, result) = parse_temperature_line(line, current);
            assert_eq!(expected_result, result, "{line:?}");
            assert_eq!(expected_max, maxtool, "{line:?}");
        }
    }

    #[test]
    fn test_canonicalize_temperatures() {
        let cases: Vec<(Temperatures, i64, Temperatures)> = vec![
            // T => T0
            (temps(&[("T", 23.0, None)]), 0, temps(&[("T0", 23.0, None)])),
            // T => T1
            (temps(&[("T", 23.0, None)]), 1, temps(&[("T1", 23.0, None)])),
            // T and Tn present => Tn wins
            (
                temps(&[("T", 23.0, None), ("T0", 23.0, None), ("T1", 42.0, None)]),
                0,
                temps(&[("T0", 23.0, None), ("T1", 42.0, None)]),
            ),
            (
                temps(&[("T", 42.0, None), ("T0", 23.0, None), ("T1", 42.0, None)]),
                1,
                temps(&[("T0", 23.0, None), ("T1", 42.0, None)]),
            ),
            (
                temps(&[("T", 21.0, None), ("T0", 23.0, None), ("T1", 42.0, None)]),
                0,
                temps(&[("T0", 23.0, None), ("T1", 42.0, None)]),
            ),
            (
                temps(&[("T", 41.0, None), ("T0", 23.0, None), ("T1", 42.0, None)]),
                1,
                temps(&[("T0", 23.0, None), ("T1", 42.0, None)]),
            ),
            // T and no T0 => Smoothieware, T = T0
            (
                temps(&[("T", 23.0, None), ("T1", 42.0, None)]),
                1,
                temps(&[("T0", 23.0, None), ("T1", 42.0, None)]),
            ),
            // no T => as-is
            (
                temps(&[("T0", 23.0, None), ("T1", 42.0, None)]),
                1,
                temps(&[("T0", 23.0, None), ("T1", 42.0, None)]),
            ),
        ];
        for (parsed, current, expected) in cases {
            let result = canonicalize_temperatures(&parsed, current);
            assert_eq!(expected, result, "{parsed:?}");
        }
    }

    #[test]
    fn test_parse_firmware_line() {
        let d = |items: &[(&str, &str)]| -> PyDict<String> {
            items
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let cases: Vec<(&str, PyDict<String>)> = vec![
            (
                "KEY1:Value 1 FIRMWARE_NAME:Some Firmware With Spaces KEY2:Value 2",
                d(&[
                    ("KEY1", "Value 1"),
                    ("KEY2", "Value 2"),
                    ("FIRMWARE_NAME", "Some Firmware With Spaces"),
                ]),
            ),
            (
                "NAME: Malyan VER: 2.9 MODEL: M200 HW: HA02",
                d(&[
                    ("NAME", "Malyan"),
                    ("VER", "2.9"),
                    ("MODEL", "M200"),
                    ("HW", "HA02"),
                ]),
            ),
            (
                "NAME. Malyan\tVER: 3.8\tMODEL: M100\tHW: HB02",
                d(&[
                    ("NAME", "Malyan"),
                    ("VER", "3.8"),
                    ("MODEL", "M100"),
                    ("HW", "HB02"),
                ]),
            ),
            (
                "NAME. Malyan VER: 3.7 MODEL: M300 HW: HG01",
                d(&[
                    ("NAME", "Malyan"),
                    ("VER", "3.7"),
                    ("MODEL", "M300"),
                    ("HW", "HG01"),
                ]),
            ),
            (
                "FIRMWARE_NAME:Marlin 1.1.0 From Archive SOURCE_CODE_URL:http:// ... PROTOCOL_VERSION:1.0 MACHINE_TYPE:www.cxsw3d.com EXTRUDER_COUNT:1 UUID:00000000-0000-0000-0000-000000000000",
                d(&[
                    ("FIRMWARE_NAME", "Marlin 1.1.0 From Archive"),
                    ("SOURCE_CODE_URL", "http:// ..."),
                    ("PROTOCOL_VERSION", "1.0"),
                    ("MACHINE_TYPE", "www.cxsw3d.com"),
                    ("EXTRUDER_COUNT", "1"),
                    ("UUID", "00000000-0000-0000-0000-000000000000"),
                ]),
            ),
            // Test firmware name with time created
            (
                "FIRMWARE_NAME:Marlin 2.0.7.2 (Nov 27 2020 14:30:11) SOURCE_CODE_URL:https://github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0 MACHINE_TYPE:Ender 5 Pro EXTRUDER_COUNT:1 UUID:cede2a2f-41a2-4748-9b12-c55c62f367ff",
                d(&[
                    ("FIRMWARE_NAME", "Marlin 2.0.7.2 (Nov 27 2020 14:30:11)"),
                    (
                        "SOURCE_CODE_URL",
                        "https://github.com/MarlinFirmware/Marlin",
                    ),
                    ("PROTOCOL_VERSION", "1.0"),
                    ("MACHINE_TYPE", "Ender 5 Pro"),
                    ("EXTRUDER_COUNT", "1"),
                    ("UUID", "cede2a2f-41a2-4748-9b12-c55c62f367ff"),
                ]),
            ),
            // Test that keys beginning with _ or number are ignored
            (
                "KEY1:VALUE1 _KEY2:INVALID 123:INVALID 1KEY:INVALID KEY2:VALUE2",
                d(&[
                    ("KEY1", "VALUE1 _KEY2:INVALID 123:INVALID 1KEY:INVALID"),
                    ("KEY2", "VALUE2"),
                ]),
            ),
        ];
        for (line, expected) in cases {
            assert_eq!(expected, parse_firmware_line(line), "{line:?}");
        }
    }

    #[test]
    fn test_parse_capability_line() {
        let cases: [(&str, Option<(&str, bool)>); 8] = [
            ("Cap:EEPROM:1", Some(("EEPROM", true))),
            ("Cap:EEPROM:0", Some(("EEPROM", false))),
            ("AUTOREPORT_TEMP:1", Some(("AUTOREPORT_TEMP", true))),
            ("AUTOREPORT_TEMP:0", Some(("AUTOREPORT_TEMP", false))),
            ("TOO:MANY:FIELDS", None),
            ("Cap:", None),
            ("TOOLITTLEFIELDS", None),
            ("WRONG:FLAG", None),
        ];
        for (line, expected) in cases {
            assert_eq!(
                expected.map(|(a, b)| (s(a), b)),
                parse_capability_line(line),
                "{line:?}"
            );
        }
    }

    #[test]
    fn test_parse_resend_line() {
        let cases = [
            ("Resend:23", 23),
            ("Resend: N23", 23),
            ("Resend: N:23", 23),
            ("rs 23", 23),
            ("rs N23", 23),
            ("rs N:23", 23),
            ("rs N23 expected checksum 109", 23), // teacup, see #300
        ];
        for (line, expected) in cases {
            assert_eq!(Some(expected), parse_resend_line(line), "{line:?}");
        }
    }

    #[test]
    fn test_parse_position_line() {
        let p = |items: &[(&str, f64)]| -> Option<BTreeMap<String, f64>> {
            Some(items.iter().map(|(k, v)| (k.to_string(), *v)).collect())
        };
        let cases: Vec<(&str, Option<BTreeMap<String, f64>>)> = vec![
            // Marlin
            (
                "ok X:62.417 Y:64.781 Z:0.2 E:2.72328 Count: A:6241 B:6478 C:20",
                p(&[("x", 62.417), ("y", 64.781), ("z", 0.2), ("e", 2.72328)]),
            ),
            (
                "X:62.417 Y:64.781 Z:0.2 E:2.72328 Count: A:6241 B:6478 C:20",
                p(&[("x", 62.417), ("y", 64.781), ("z", 0.2), ("e", 2.72328)]),
            ),
            // RepRapFirmware
            (
                "X:96.99 Y:88.31 Z:0.30 E0:0.0 E1:0.0 E2:0.0 E3:0.0 E4:0.0 E5:0.0",
                p(&[
                    ("x", 96.99),
                    ("y", 88.31),
                    ("z", 0.3),
                    ("e0", 0.0),
                    ("e1", 0.0),
                    ("e2", 0.0),
                    ("e3", 0.0),
                    ("e4", 0.0),
                    ("e5", 0.0),
                ]),
            ),
            // whitespace after the :, e.g. AlfaWise U20, see #2839
            (
                "X:150.0 Y:150.0 Z:  0.7 E:  0.0",
                p(&[("x", 150.0), ("y", 150.0), ("z", 0.7), ("e", 0.0)]),
            ),
            // invalid
            ("", None),
            ("X:62.417 Y:64.781 Z:0.2", None),
        ];
        for (line, expected) in cases {
            assert_eq!(expected, parse_position_line(line), "{line:?}");
        }
    }

    // --- Not in test_comm_helpers.py: the examples from the docstring of
    // --- _normalize_command_handler_result (comm.py:6745-6770), which are Python doctests.

    type Plain = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<Vec<String>>,
    );

    fn plain(r: Vec<NormalizedCommand>) -> Vec<Plain> {
        r.into_iter()
            .map(|(c, ct, g, sc, t)| {
                (
                    c,
                    ct,
                    g,
                    sc,
                    t.map(|t| t.borrow().iter().cloned().collect()),
                )
            })
            .collect()
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|x| x.to_string()).collect()
    }

    fn tags_of(items: &[&str]) -> Option<Tags> {
        Some(Rc::new(RefCell::new(set(items))))
    }

    fn pl(c: &str, ct: Option<&str>, g: &str, t: Option<&[&str]>) -> Plain {
        (
            Some(s(c)),
            ct.map(s),
            Some(s(g)),
            None,
            t.map(|t| {
                let mut v: Vec<String> = t.iter().map(|x| x.to_string()).collect();
                v.sort();
                v
            }),
        )
    }

    fn st(x: &str) -> HandlerResultEntry {
        HandlerResultEntry::Str(x.to_string())
    }

    fn t1(x: &str) -> HandlerResultEntry {
        HandlerResultEntry::Tuple(HandlerTuple::One(Some(s(x))))
    }

    fn t2(x: Option<&str>, y: &str) -> HandlerResultEntry {
        HandlerResultEntry::Tuple(HandlerTuple::Two(x.map(s), Some(s(y))))
    }

    fn norm(
        tags: Option<Tags>,
        results: Option<HandlerResults>,
        add: Option<&BTreeSet<String>>,
    ) -> Vec<Plain> {
        plain(_normalize_command_handler_result(
            Some(s("M105")),
            None,
            Some(s("M105")),
            None,
            tags,
            results,
            add,
        ))
    }

    fn list(l: Vec<HandlerResultEntry>) -> Option<HandlerResults> {
        Some(HandlerResults::List(l))
    }

    #[test]
    fn normalize_command_handler_result_docstring_examples() {
        use HandlerResultEntry as E;
        let m105 = || pl("M105", None, "M105", None);
        assert_eq!(vec![m105()], norm(None, None, None));
        assert_eq!(
            vec![pl("M110", None, "M110", None)],
            norm(None, Some(HandlerResults::Entry(st("M110"))), None)
        );
        assert_eq!(
            vec![pl("M110", None, "M110", None)],
            norm(None, list(vec![st("M110")]), None)
        );
        assert_eq!(
            vec![
                pl("M110", None, "M110", None),
                pl("M117 Foobar", None, "M117", None)
            ],
            norm(None, list(vec![st("M110"), st("M117 Foobar")]), None)
        );
        assert_eq!(
            vec![
                pl("M110", None, "M110", None),
                pl("M117 Foobar", None, "M117", None)
            ],
            norm(None, list(vec![t1("M110"), st("M117 Foobar")]), None)
        );
        assert_eq!(
            vec![
                pl("M110", Some("lineno_reset"), "M110", None),
                pl("M117 Foobar", None, "M117", None)
            ],
            norm(
                None,
                list(vec![t2(Some("M110"), "lineno_reset"), st("M117 Foobar")]),
                None
            )
        );
        assert_eq!(Vec::<Plain>::new(), norm(None, list(vec![]), None));
        assert_eq!(
            vec![pl("M110", None, "M110", None)],
            norm(None, list(vec![st("M110"), E::None]), None)
        );
        assert_eq!(
            vec![pl("M110", None, "M110", None)],
            norm(None, list(vec![t1("M110"), t2(None, "ignored")]), None)
        );
        assert_eq!(
            vec![
                pl("M110", None, "M110", None),
                pl("M117 Foobar", Some("display_message"), "M117", None),
                pl("M110", Some("lineno_reset"), "M110", None)
            ],
            norm(
                None,
                list(vec![
                    t1("M110"),
                    t2(Some("M117 Foobar"), "display_message"),
                    E::Tuple(HandlerTuple::Other),
                    t2(Some("M110"), "lineno_reset"),
                ]),
                None
            )
        );
        let tg = Some(&["tag1", "tag2"][..]);
        assert_eq!(
            vec![
                pl("M110", None, "M110", tg),
                pl("M117 Foobar", None, "M117", tg)
            ],
            norm(
                tags_of(&["tag1", "tag2"]),
                list(vec![st("M110"), st("M117 Foobar")]),
                None
            )
        );
        let tg3 = Some(&["tag1", "tag2", "tag3"][..]);
        let add = set(&["tag3"]);
        assert_eq!(
            vec![
                pl("M110", None, "M110", tg3),
                pl("M105", None, "M105", tg),
                pl("M117 Foobar", None, "M117", tg3)
            ],
            norm(
                tags_of(&["tag1", "tag2"]),
                list(vec![st("M110"), st("M105"), st("M117 Foobar")]),
                Some(&add)
            )
        );
        assert_eq!(
            vec![
                pl("M110", None, "M110", tg3),
                pl("M105", Some("temperature_poll"), "M105", tg3),
                pl("M117 Foobar", None, "M117", tg3)
            ],
            norm(
                tags_of(&["tag1", "tag2"]),
                list(vec![
                    st("M110"),
                    t2(Some("M105"), "temperature_poll"),
                    st("M117 Foobar")
                ]),
                Some(&add)
            )
        );
    }

    // --- Checks of the helpers standing in for Python built-ins and of the m20 conversion.

    #[test]
    fn py_helpers() {
        assert_eq!(vec!["a", "b", "c  d  "], py_split_none("  a b  c  d  ", 2));
        assert_eq!(vec!["a"], py_split_none("a ", 1));
        assert_eq!(Vec::<&str>::new(), py_split_none("   ", 3));
        assert_eq!(Some(42), py_int(" +4_2 "));
        assert_eq!(None, py_int("4__2"));
        assert_eq!(None, py_int("-"));
        assert_eq!(Some(5.0), py_float("5."));
        assert_eq!(None, py_float("-"));
        assert_eq!("b\"a'b\"", py_bytes_repr(b"a'b"));
        assert_eq!("b'a\\\\b\\x00'", py_bytes_repr(b"a\\b\0"));
    }

    /// `tests/util/test_comm.py:test__validate_m20_timestamp` (pytest, parametrized), all 5 cases
    #[test]
    #[allow(non_snake_case)] // the reference's name
    fn test__validate_m20_timestamp() {
        for (val, expected) in [
            ("aaa", false),
            ("1234", false),
            ("0x21bf7d", true),
            ("0xghijk", false),
            ("0x28210800", true),
        ] {
            assert_eq!(expected, _validate_m20_timestamp(val), "{val}");
        }
    }

    /// `tests/util/test_comm.py:test_parse_file_list_line` (pytest, parametrized), all 13 cases
    #[test]
    fn test_parse_file_list_line() {
        use super::super::files::m20_timestamp_to_unix_timestamp;
        let ts = || Some(m20_timestamp_to_unix_timestamp("0x21bf7d").expect("a valid timestamp"));
        let s = |x: &str| x.to_string();
        let cases: Vec<(&str, (String, Option<i64>, Option<i64>, Option<String>))> = vec![
            (
                "line that makes little sense",
                (s("line that makes little sense"), None, None, None),
            ),
            ("name.gco", (s("name.gco"), None, None, None)),
            (
                "name.gco invalid-size",
                (s("name.gco invalid-size"), None, None, None),
            ),
            (
                "name.gco 3424324",
                (s("name.gco"), Some(3424324), None, None),
            ),
            (
                "name.gco 3424324 longname.gcode",
                (
                    s("name.gco"),
                    Some(3424324),
                    None,
                    Some(s("longname.gcode")),
                ),
            ),
            (
                "name.gco 3424324 0x21bf7d",
                (s("name.gco"), Some(3424324), ts(), None),
            ),
            (
                "name.gco 3424324 0xinvalid_timestamp_as_longname",
                (
                    s("name.gco"),
                    Some(3424324),
                    None,
                    Some(s("0xinvalid_timestamp_as_longname")),
                ),
            ),
            (
                "longname.gcode 3424324 0x21bf7d",
                (s("longname.gcode"), Some(3424324), ts(), None),
            ),
            (
                "longname.gcode 3424324 longname.gcode",
                (
                    s("longname.gcode"),
                    Some(3424324),
                    None,
                    Some(s("longname.gcode")),
                ),
            ),
            (
                "name.gco 32424 0x21bf7d long name without quoting",
                (
                    s("name.gco"),
                    Some(32424),
                    ts(),
                    Some(s("long name without quoting")),
                ),
            ),
            (
                "name.gco 32424 0x21bf7d long   name   without   quoting",
                (
                    s("name.gco"),
                    Some(32424),
                    ts(),
                    Some(s("long   name   without   quoting")),
                ),
            ),
            (
                "name.gco 32424 0x21bf7d \"long name with quoting\"",
                (
                    s("name.gco"),
                    Some(32424),
                    ts(),
                    Some(s("long name with quoting")),
                ),
            ),
            (
                "name.gco 32424 0x21bf7d \"long   name   with   quoting\"",
                (
                    s("name.gco"),
                    Some(32424),
                    ts(),
                    Some(s("long   name   with   quoting")),
                ),
            ),
        ];
        for (val, expected) in cases {
            assert_eq!(expected, parse_file_list_line(val), "{val}");
        }
    }

    #[test]
    fn m20_timestamp_validation() {
        assert!(!_validate_m20_timestamp("12345678"));
        assert!(!_validate_m20_timestamp("0x"));
        // month 0
        assert!(!_validate_m20_timestamp("0x00000000"));
        // 2021-01-01 00:00:00 = 0x52210000
        assert!(_validate_m20_timestamp("0x52210000"));
        // hour 31
        assert!(!_validate_m20_timestamp("0x5221F800"));
    }

    #[test]
    fn parse_file_list_line_shapes() {
        assert_eq!(
            ("a.gco".to_string(), None, None, None),
            parse_file_list_line("a.gco")
        );
        assert_eq!(
            ("a.gco".to_string(), Some(123), None, None),
            parse_file_list_line("a.gco 123")
        );
        assert_eq!(
            (
                "a.gco".to_string(),
                Some(123),
                None,
                Some("Long  Name.gcode".to_string())
            ),
            parse_file_list_line("a.gco 123 Long  Name.gcode")
        );
        assert_eq!(
            ("a.gco 12x".to_string(), None, None, None),
            parse_file_list_line("a.gco 12x")
        );
        assert_eq!(
            (
                "a.gco".to_string(),
                Some(5),
                None,
                Some("Quoted".to_string())
            ),
            parse_file_list_line("a.gco 5 \"Quoted\"")
        );
    }

    /// Every regex of comm.py as Python itself builds it (`scripts/octoprint-patterns.py`
    /// writes `comm_patterns.txt` from the source): the same pattern, character for character.
    #[test]
    fn every_pattern_is_comm_pys() {
        let lookup = |name: &str| -> Option<&'static Regex> {
            Some(match name {
                "regex_command" => &regex_command,
                "regex_float" => &regex_float,
                "regex_minMaxError" => &regex_minMaxError,
                "regex_marlinKillError" => &regex_marlinKillError,
                "regex_sdPrintingByte" => &regex_sdPrintingByte,
                "regex_sdFileOpened" => &regex_sdFileOpened,
                "regex_temp" => &regex_temp,
                "regex_repetierTempExtr" => &regex_repetierTempExtr,
                "regex_repetierTempBed" => &regex_repetierTempBed,
                "regex_position" => &regex_position,
                "regex_e_positions" => &regex_e_positions,
                "regex_firmware_splitter" => &regex_firmware_splitter,
                "regex_resend_linenumber" => &regex_resend_linenumber,
                "regex_serial_devices" => &regex_serial_devices,
                "_temp_command_regex" => &_temp_command_regex,
                other => {
                    return other
                        .strip_prefix("regexes_parameters.")
                        .map(|k| &regexes_parameters[k]);
                }
            })
        };
        let mut checked = 0;
        for line in include_str!("comm_patterns.txt").lines() {
            let (name, pattern) = line.split_once('\t').unwrap();
            let pattern: String = serde_json::from_str(pattern).unwrap();
            let regex = lookup(name).unwrap_or_else(|| panic!("{name}: not ported"));
            assert_eq!(regex.as_str(), pattern, "{name}");
            checked += 1;
        }
        assert_eq!(checked, 26);
    }

    /// Python reads Unicode decimal digits: `int('٣') == 3`, `float('٣.٥') == 3.5`; a resend
    /// request `Resend: N٣` is line 3, not a crash.
    #[test]
    fn unicode_digits_read_as_python_reads_them() {
        assert_eq!(py_int("٣"), Some(3));
        assert_eq!(py_float("٣.٥"), Some(3.5));
        assert_eq!(parse_resend_line("Resend: N٣"), Some(3));
    }
}

#[cfg(test)]
mod tests_records_and_queues {
    //! Ports of `tests/util/test_comm_helpers.py` `TestPositionRecord` and `TestTemperatureRecord`,
    //! then tests marked "ours" for the queue classes (the reference has none for them).
    use super::*;

    fn coords(items: &[(&str, f64)]) -> Vec<(&'static str, Option<f64>)> {
        items
            .iter()
            .map(|(k, v)| {
                (
                    Box::leak(k.to_string().into_boxed_str()) as &'static str,
                    Some(*v),
                )
            })
            .collect()
    }

    fn expected(items: &[(&str, f64)], extra_none: &[&str]) -> BTreeMap<String, Option<f64>> {
        let mut e: BTreeMap<String, Option<f64>> = items
            .iter()
            .map(|(k, v)| (k.to_string(), Some(*v)))
            .collect();
        for k in extra_none {
            e.insert(k.to_string(), None);
        }
        e
    }

    // TestPositionRecord.test_as_dict_regular
    #[test]
    fn test_as_dict_regular() {
        let c = [("x", 1.0), ("y", 2.0), ("z", 3.0), ("e", 4.0)];
        let position = PositionRecord::new(&coords(&c));
        assert_eq!(position.as_dict(), expected(&c, &["f", "t"]));
    }

    // TestPositionRecord.test_as_dict_extra_e
    #[test]
    fn test_as_dict_extra_e() {
        let c = [("x", 1.0), ("y", 2.0), ("z", 3.0), ("e0", 4.0), ("e1", 5.0)];
        let position = PositionRecord::new(&coords(&c));
        assert_eq!(position.as_dict(), expected(&c, &["e", "f", "t"]));
    }

    // TestPositionRecord.test_copy_from_regular
    #[test]
    fn test_copy_from_regular() {
        let c = [("x", 1.0), ("y", 2.0), ("z", 3.0), ("e", 4.0)];
        let position1 = PositionRecord::new(&coords(&c));
        let mut position2 = PositionRecord::new(&[]);

        position2.copy_from(&position1);

        assert_eq!(position2.as_dict(), expected(&c, &["f", "t"]));
    }

    // TestPositionRecord.test_copy_from_extra_e
    #[test]
    fn test_copy_from_extra_e() {
        let c = [("x", 1.0), ("y", 2.0), ("z", 3.0), ("e0", 4.0), ("e1", 5.0)];
        let position1 = PositionRecord::new(&coords(&c));
        let mut position2 = PositionRecord::new(&[]);

        position2.copy_from(&position1);

        assert_eq!(position2.as_dict(), expected(&c, &["e", "f", "t"]));
    }

    // TestPositionRecord.test_copy_from_extra_e_changed
    #[test]
    fn test_copy_from_extra_e_changed() {
        let c1 = [("x", 1.0), ("y", 2.0), ("z", 3.0), ("e0", 4.0), ("e1", 5.0)];
        let position1 = PositionRecord::new(&coords(&c1));

        let c2 = [
            ("x", 2.0),
            ("y", 4.0),
            ("z", 6.0),
            ("e0", 8.0),
            ("e1", 10.0),
            ("e2", 12.0),
        ];
        let mut position2 = PositionRecord::new(&coords(&c2));

        let expected_before = expected(&c2, &["e", "f", "t"]);
        assert_eq!(position2.as_dict(), expected_before);

        position2.copy_from(&position1);

        let expected_after = expected(&c1, &["e", "f", "t"]);
        assert_eq!(position2.as_dict(), expected_after);
    }

    // TestTemperatureRecord.test_set_custom (@data)
    #[test]
    fn test_set_custom() {
        for identifier in [
            "TX",
            "B2",
            "BX",
            "SOMETHING_CUSTOM",
            "1234B456",
            "blub",
            "fnord",
            "C1",
            "CX",
        ] {
            let mut temperature = TemperatureRecord::new();

            temperature
                .set_custom(identifier, Some(1.0), Some(2.0))
                .unwrap();

            assert!(
                temperature.custom().iter().any(|(k, _)| k == identifier),
                "{identifier}"
            );
        }
    }

    // TestTemperatureRecord.test_set_custom_reserved (@data)
    #[test]
    fn test_set_custom_reserved() {
        for identifier in ["T", "T1", "T42", "B", "C"] {
            let mut temperature = TemperatureRecord::new();

            match temperature.set_custom(identifier, Some(1.0), Some(2.0)) {
                Ok(()) => panic!("Expected ValueError for reserved identifier {identifier}"),
                Err(ex) => assert!(ex.0.contains("is a reserved identifier")),
            }
        }
    }

    // ---- ours: not in the reference's tests --------------------------------------------------

    /// ours: `TemperatureRecord.set_tool/_to_new_tuple/as_script_dict` as the Python code reads.
    #[test]
    fn ours_temperature_record_to_new_tuple() {
        let mut t = TemperatureRecord::new();
        t.set_tool(0, Some(20.0), None);
        t.set_tool(0, None, Some(200.0));
        t.set_bed(Some(1.0), Some(2.0));
        t.set_bed(None, None);
        assert_eq!(t.tools(), vec![(0, (Some(20.0), Some(200.0)))]);
        assert_eq!(t.bed(), (Some(1.0), Some(2.0)));
        let d = t.as_script_dict();
        assert_eq!(
            d[0],
            (
                ScriptKey::Tool(0),
                ScriptTemp {
                    actual: Some(20.0),
                    target: Some(200.0)
                }
            )
        );
    }

    /// ours: CommandQueue: type uniqueness, order, blocked() and clear().
    #[test]
    fn ours_command_queue() {
        let q: CommandQueue<&str, &str> = CommandQueue::new(0);
        q.put("a", Some("T"), true, None).unwrap();
        assert!(matches!(
            q.put("b", Some("T"), true, None),
            Err(PutError::Raised(_))
        ));
        q.put("c", None, true, None).unwrap();
        q.prepend("p", None, true, None).unwrap();
        assert_eq!(q.get(true, None).unwrap(), "p");
        {
            let _g = q.blocked();
            assert!(!q._unblocked.is_set());
        }
        assert!(q._unblocked.is_set());
        assert_eq!(q.clear(), vec!["a", "c"]);
        assert_eq!(q.get(false, None), Err(GetError::Empty));
    }

    /// ours: SendQueue: resend queue goes first, resend_active hides the send queue.
    #[test]
    fn ours_send_queue() {
        let q: SendQueue<&str, &str> = SendQueue::new(0);
        q.put("s1", None, None, true, None).unwrap();
        q.put("r1", None, Some("resend".into()), true, None)
            .unwrap();
        assert_eq!(q.qsize(), 2);
        q.set_resend_active(true);
        assert_eq!(q.qsize(), 1);
        assert_eq!(q.get(true, None).unwrap(), "r1");
        assert_eq!(q.get(false, None), Err(GetError::Empty));
        q.set_resend_active(false);
        assert_eq!(q.get(true, None).unwrap(), "s1");
    }

    /// ours: QueueMarker runs its callback and survives a panicking one.
    #[test]
    fn ours_queue_marker() {
        use std::sync::atomic::AtomicUsize;
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        SendQueueMarker::new(Some(Box::new(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        })))
        .run();
        assert_eq!(n.load(Ordering::SeqCst), 1);
        QueueMarker::new(Some(Box::new(|| panic!("boom")))).run();
        QueueMarker::new(None).run();
    }
}

#[cfg(test)]
mod machinecom_tests {
    //! OURS. No test of `reference/OctoPrint-1.11.8/tests/util/test_comm.py` covers this section:
    //! `TestCommErrorHandling` (:12-224) tests `_handle_errors` / `_trigger_error` on a
    //! `mock.create_autospec(MachineCom)` (not the class state, and not in section E), and the
    //! rest of that file tests `m20_timestamp_to_unix_timestamp`. So every test here is ours.
    //! None of them calls `start()` (it would run the stubbed threads).
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Rec {
        logs: Mutex<Vec<String>>,
        states: Mutex<Vec<i64>>,
        sd_files: Mutex<usize>,
        positions: Mutex<Vec<(String, String, Option<i64>)>>,
    }

    impl MachineComPrintCallback for Rec {
        fn on_comm_log(&self, message: &str) {
            self.logs.lock().unwrap().push(message.to_string());
        }
        fn on_comm_state_change(&self, state: i64) {
            self.states.lock().unwrap().push(state);
        }
        fn on_comm_sd_files(&self, _files: &[Value]) {
            *self.sd_files.lock().unwrap() += 1;
        }
        fn on_comm_record_fileposition(&self, origin: &str, name: &str, pos: Option<i64>) {
            self.positions
                .lock()
                .unwrap()
                .push((origin.to_string(), name.to_string(), pos));
        }
    }

    /// A file in the temp dir with `content` (a unique name per call)
    fn temp_file(content: &[u8]) -> String {
        use std::sync::atomic::AtomicUsize;
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "maker-connect-comm-test-{}-{}.gcode",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::write(&path, content).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn sd_file() -> Arc<PrintingSdFileInformation> {
        let f = PrintingSdFileInformation::new("x.gco", Some(84), None);
        f.set_pos(42);
        Arc::new(f)
    }

    fn gcode_file() -> Arc<PrintingGcodeFileInformation> {
        Arc::new(
            PrintingGcodeFileInformation::new(&temp_file(b"G1 X1\n"), None, None, None).unwrap(),
        )
    }

    fn streaming_file() -> Arc<StreamingGcodeFileInformation> {
        Arc::new(
            StreamingGcodeFileInformation::new(
                &temp_file(b"G1 X1\n"),
                "local.gcode",
                "/remote.gco",
                None,
            )
            .unwrap(),
        )
    }

    struct FakeSerial {
        fail_close: bool,
        closed: Mutex<bool>,
    }

    impl SerialTransport for FakeSerial {
        fn port(&self) -> Option<Option<String>> {
            Some(Some("/dev/ttyFAKE".to_string()))
        }
        fn close(&self) -> Result<(), String> {
            *self.closed.lock().unwrap() = true;
            if self.fail_close {
                Err("boom".to_string())
            } else {
                Ok(())
            }
        }
        fn str(&self) -> String {
            "FakeSerial".to_string()
        }
        fn set_timeout(&self, _timeout: f64) {}
        fn timeout(&self) -> Option<f64> {
            None
        }
        fn set_baudrate(&self, _baudrate: i64) {}
        fn readline(&self) -> Result<Vec<u8>, SerialError> {
            Ok(Vec::new())
        }
        fn write(&self, data: &[u8]) -> Result<Option<i64>, SerialError> {
            Ok(Some(data.len() as i64))
        }
    }

    fn comm() -> (Arc<MachineCom>, Arc<Rec>) {
        let rec = Arc::new(Rec::default());
        let c = MachineCom::new(None, None, Some(rec.clone()), None);
        (c, rec)
    }

    #[test]
    fn machinecom_is_shared_between_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MachineCom>();
    }

    /// Every `STATE_*`/`CAPABILITY_*` constant and every attribute assigned in `__init__`
    /// (`machinecom_attrs.txt`, written by `scripts/octoprint-machinecom-attrs.py` from comm.py)
    /// exists in this file. Mapping rule: the Rust name is the Python name, exactly
    /// (`const NAME` for a constant, a `pub NAME:` field of `struct MachineCom` for an
    /// attribute); `reassigned` attributes must be `Attr<..>` fields. Not ported (logging,
    /// difference 1): `_logger`, `_serialLogger`, `_phaseLogger`.
    #[test]
    fn machinecom_attrs_exist() {
        let src = include_str!("comm.rs");
        let st = src
            .find("pub struct MachineCom {")
            .expect("struct MachineCom");
        let body = &src[st..st + src[st..].find("\n}\n").unwrap()];
        let not_ported = ["_logger", "_serialLogger", "_phaseLogger"];
        let mut n = (0, 0);
        for line in include_str!("machinecom_attrs.txt").lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            let name = parts[1];
            match parts[0] {
                "const" => {
                    assert!(
                        src.contains(&format!("pub const {name}:")),
                        "constant {name} is missing"
                    );
                    n.0 += 1;
                }
                "attr" => {
                    if not_ported.contains(&name) {
                        continue;
                    }
                    let field = body
                        .lines()
                        .find(|l| l.trim_start().starts_with(&format!("pub {name}:")))
                        .unwrap_or_else(|| panic!("attribute {name} is missing"));
                    if parts.contains(&"reassigned") {
                        assert!(
                            field.contains("Attr<"),
                            "{name} is reassigned in Python, so it must be an Attr"
                        );
                    }
                    n.1 += 1;
                }
                other => panic!("unknown kind {other}"),
            }
        }
        assert_eq!((30, 153), n);
    }

    #[test]
    fn init_defaults() {
        let (c, _) = comm();
        assert_eq!(MachineCom::STATE_NONE, c.get_state());
        assert_eq!(0, c._baudrate);
        assert_eq!(None, c._port);
        assert_eq!(3, c._detection_retry.get());
        assert_eq!(1, c._current_line.get());
        assert_eq!(Some(1), c._ack_max);
        assert_eq!(Some(1), c._clear_to_send.max());
        assert_eq!(None, c._clear_to_send.min());
        assert_eq!(0.1, c._resend_ratio_threshold);
        assert_eq!(Some(100), c._resend_ratio_start);
        assert_eq!(
            Some(50),
            c._lastLines.with(|l| l.maxlen()).map(|m| m as i64)
        );
        assert_eq!(
            Some(20),
            c._terminal_log.with(|l| l.maxlen()).map(|m| m as i64)
        );
        assert_eq!(
            Some(10.0),
            c._timeout_intervals
                .with(|d| d.get("detectionFirst").copied())
        );
        assert_eq!(
            Some(2),
            c._consecutive_timeout_maximums
                .with(|d| d.get("idle").copied())
        );
        // serial.capabilities has no "chamber_temp" in the defaults: None (falsy)
        assert_eq!(
            Some(None),
            c._capability_support
                .with(|d| d.get("CHAMBER_TEMPERATURE").copied())
        );
        assert_eq!(
            Some(Some(true)),
            c._capability_support
                .with(|d| d.get("AUTOREPORT_TEMP").copied())
        );
        assert!(c._active());
        assert!(!c.monitoring_thread.is_alive() && !c.sending_thread.is_alive());
        assert!(c._gcode_hooks.get("queuing").unwrap().is_empty());
        assert_eq!(4, c._gcode_hooks.len());
        assert!(!c.job_on_hold());
    }

    #[test]
    fn state_strings() {
        let (c, _) = comm();
        let expected = [
            (0, "Offline"),
            (1, "Opening serial connection"),
            (2, "Detecting serial connection"),
            (3, "Connecting"),
            (4, "Operational"),
            (5, "Starting"),
            (6, "Printing"),
            (7, "Paused"),
            (8, "Pausing"),
            (9, "Resuming"),
            (10, "Finishing"),
            (11, "Offline"),
            (12, "Error"),
            (13, "Offline after error"),
            (14, "Transferring file to SD"),
            (15, "Cancelling"),
        ];
        for (s, text) in expected {
            assert_eq!(text, c.get_state_string(Some(s)), "{s}");
        }
        assert_eq!("Offline", c.get_state_string(None));
        c._state.set(99);
        assert_eq!("Unknown State (99)", c.get_state_string(None));
        assert_eq!("Unknown State (99)", c.get_state_string(Some(98)));
    }

    #[test]
    fn state_strings_with_files() {
        let (c, _) = comm();
        c._currentFile.set(Some(sd_file()));
        assert_eq!("Starting print from SD", c.get_state_string(Some(5)));
        assert_eq!("Printing from SD", c.get_state_string(Some(6)));
        c._currentFile.set(Some(streaming_file()));
        assert_eq!("Starting to send file to SD", c.get_state_string(Some(5)));
        assert_eq!("Sending file to SD", c.get_state_string(Some(6)));
        let f = streaming_file();
        f.set_done(true);
        c._currentFile.set(Some(f));
        assert_eq!("Printing", c.get_state_string(Some(6)));
    }

    #[test]
    fn state_ids() {
        let (c, _) = comm();
        assert_eq!("NONE", c.get_state_id(None));
        assert_eq!("CLOSED_WITH_ERROR", c.get_state_id(Some(13)));
        assert_eq!("CANCELLING", c.get_state_id(Some(15)));
        assert_eq!("UNKNOWN", c.get_state_id(Some(16)));
    }

    #[test]
    fn state_predicates() {
        let (c, _) = comm();
        for s in 0..=15 {
            c._state.set(s);
            assert_eq!(
                MachineCom::OPERATIONAL_STATES.contains(&s),
                c.is_operational()
            );
            assert_eq!(MachineCom::PRINTING_STATES.contains(&s), c.is_printing());
            assert_eq!([11, 12, 13].contains(&s), c.is_closed_or_error());
            assert_eq!([12, 13].contains(&s), c.is_error());
            assert_eq!(s == 15, c.is_cancelling());
            assert_eq!(s == 8, c.is_pausing());
            assert_eq!(s == 9, c.is_resuming());
            assert_eq!(s == 5, c.is_starting());
            assert_eq!(s == 10, c.is_finishing());
            assert_eq!(s == 7, c.is_paused());
            assert_eq!([5, 6, 15, 8, 9, 10, 7].contains(&s), c.is_busy());
        }
        c._state.set(6);
        assert!(!c.is_sd_printing());
        c._currentFile.set(Some(sd_file()));
        assert!(c.is_sd_printing() && c.is_sd_file_selected() && !c.is_streaming());
        c._currentFile.set(Some(streaming_file()));
        assert!(c.is_streaming() && !c.is_sd_file_selected());
        c._currentFile.set(None); // dropping a busy comm records the file position
    }

    #[test]
    fn capability_supported() {
        let (c, _) = comm();
        let t = MachineCom::CAPABILITY_AUTOREPORT_TEMP;
        // supported in the settings, not reported by the firmware
        assert!(!c._capability_supported(t));
        c._firmware_capabilities.with_mut(|d| d.insert(t, true));
        assert!(c._capability_supported(t));
        c._firmware_capabilities.with_mut(|d| d.insert(t, false));
        assert!(!c._capability_supported(t));
        // reported by the firmware, but the setting is None
        let ch = MachineCom::CAPABILITY_CHAMBER_TEMP;
        c._firmware_capabilities.with_mut(|d| d.insert(ch, true));
        assert!(!c._capability_supported(ch));
        // unknown capability
        assert!(!c._capability_supported("FOO"));
    }

    #[test]
    fn resend_ratio_and_report() {
        let (c, rec) = comm();
        assert_eq!(0.0, c.resend_ratio());
        c._transmitted_lines.set(200);
        c._received_resend_requests.set(10);
        assert_eq!((200, 10), (c.transmitted_lines(), c.received_resends()));
        assert_eq!(0.05, c.resend_ratio());
        c._reevaluate_resend_ratio();
        assert!(!c._resend_ratio_reported.get());
        assert!(rec.logs.lock().unwrap().is_empty());

        c._received_resend_requests.set(40);
        c._reevaluate_resend_ratio();
        assert!(c._resend_ratio_reported.get());
        assert_eq!(
            vec!["Over 10% of transmitted lines have triggered resend requests (20.00%). The communication with the printer is unreliable. Please see https://faq.octoprint.org/communication-errors.".to_string()],
            *rec.logs.lock().unwrap()
        );
        // reported only once
        c._reevaluate_resend_ratio();
        assert_eq!(1, rec.logs.lock().unwrap().len());
    }

    #[test]
    fn resend_ratio_needs_more_than_start_lines() {
        let (c, rec) = comm();
        c._transmitted_lines.set(100); // resendRatioStart is 100: must be exceeded
        c._received_resend_requests.set(100);
        c._reevaluate_resend_ratio();
        assert!(!c._resend_ratio_reported.get() && rec.logs.lock().unwrap().is_empty());
    }

    #[test]
    fn log_feeds_terminal_log_and_callback() {
        let (c, rec) = comm();
        for i in 0..25 {
            c._log(&format!("line {i}"));
        }
        assert_eq!(25, rec.logs.lock().unwrap().len());
        let kept: Vec<String> = c._terminal_log.with(|l| l.iter().cloned().collect());
        assert_eq!(20, kept.len());
        assert_eq!("line 5", kept[0]);
        assert_eq!("line 24", kept[19]);
        c._dual_log("msg", LOGGING_ERROR, "Prefix: ");
        assert_eq!("Prefix: msg", rec.logs.lock().unwrap().last().unwrap());
        c._to_logfile_with_terminal(Some("x"), LOGGING_INFO);
    }

    #[test]
    fn last_lines_keep_fifty() {
        let (c, _) = comm();
        for i in 0..60u8 {
            c._add_to_last_lines(vec![i]);
        }
        assert_eq!(50, c._lastLines.with(|l| l.len()));
        assert_eq!(Some(vec![10u8]), c._lastLines.with(|l| l.get(0).cloned()));
        assert_eq!(Some(vec![59u8]), c._lastLines.with(|l| l.get(-1).cloned()));
    }

    #[test]
    fn change_state() {
        let (c, rec) = comm();
        c._change_state(MachineCom::STATE_NONE);
        assert!(rec.states.lock().unwrap().is_empty());
        c._change_state(MachineCom::STATE_OPERATIONAL);
        assert_eq!(MachineCom::STATE_OPERATIONAL, c.get_state());
        assert_eq!(vec![4], *rec.states.lock().unwrap());
        assert_eq!(
            "Changing monitoring state from \"Offline\" to \"Operational\"",
            rec.logs.lock().unwrap()[0]
        );
        assert_eq!(0, *rec.sd_files.lock().unwrap());
    }

    #[test]
    fn change_state_closed_clears_sd_files_and_closes_file() {
        let (c, rec) = comm();
        c._sdFiles.with_mut(|d| {
            d.insert(
                "a.gco",
                SDFileData {
                    name: "a.gco".into(),
                    size: None,
                    timestamp: None,
                    longname: None,
                },
            )
        });
        c._sdFileList.set(true);
        let f = gcode_file();
        f.start().unwrap();
        assert!(f._handle.with(|h| h.is_some()));
        c._currentFile.set(Some(f.clone()));
        c._state.set(MachineCom::STATE_OPERATIONAL);
        c._change_state(MachineCom::STATE_CLOSED);
        // feature.sdSupport is true in the defaults
        assert_eq!(1, *rec.sd_files.lock().unwrap());
        assert!(c._sdFiles.with(|d| d.is_empty()));
        assert!(!c._sdFileList.get());
        assert!(f._handle.with(|h| h.is_none()));
        assert_eq!(vec![11], *rec.states.lock().unwrap());
    }

    #[test]
    fn change_state_closed_while_busy_records_file_position() {
        let (c, rec) = comm();
        c._currentFile.set(Some(sd_file()));
        c._state.set(MachineCom::STATE_PRINTING);
        c._change_state(MachineCom::STATE_CLOSED);
        assert_eq!(
            vec![("sdcard".to_string(), "x.gco".to_string(), Some(42))],
            *rec.positions.lock().unwrap()
        );
    }

    #[test]
    fn getters() {
        let (c, _) = comm();
        assert_eq!(None, c.get_print_progress());
        assert_eq!(None, c.get_print_filepos());
        assert_eq!(None, c.get_print_time());
        assert_eq!(None, c.get_cleaned_print_time());
        c._currentFile.set(Some(sd_file()));
        assert_eq!(Some(0.5), c.get_print_progress());
        assert_eq!(Some(42), c.get_print_filepos());
        assert_eq!(None, c.get_print_time());
        let f = sd_file();
        f.base()._start_time.set(Some(time_monotonic() - 100.0));
        c._currentFile.set(Some(f));
        c._pauseWaitTimeLost.set(10.0);
        c._heatupWaitTimeLost.set(20.0);
        let t = c.get_print_time().unwrap();
        assert!((t - 90.0).abs() < 1.0, "{t}");
        let ct = c.get_cleaned_print_time().unwrap();
        assert!((ct - 70.0).abs() < 1.0, "{ct}");
        c._heatupWaitTimeLost.set(500.0);
        assert_eq!(Some(0.0), c.get_cleaned_print_time());

        c._tempOffsets.with_mut(|d| d.insert("tool0", 5.0));
        assert_eq!(Some(&5.0), c.get_offsets().get("tool0"));
        c._errorValue.set("oops".into());
        assert_eq!("oops", c.get_error_string());
        assert_eq!(0, c.get_current_tool());
        assert_eq!(Vec::<(i64, TempTuple)>::new(), c.get_temp());
        assert_eq!((None, None), c.get_bed_temp());
        assert!(c.is_sd_ready() == false);
    }

    #[test]
    fn connection_and_transport() {
        let (c, _) = comm();
        assert_eq!((None, Some(0)), c.get_connection());
        assert!(c.get_transport().is_none());
        c._serial.set(Some(Arc::new(FakeSerial {
            fail_close: false,
            closed: Mutex::new(false),
        })));
        // the transport has a port, no baudrate attribute
        assert_eq!(
            (Some("/dev/ttyFAKE".to_string()), Some(0)),
            c.get_connection()
        );
        assert!(c.get_transport().is_some());
    }

    #[test]
    fn job_on_hold() {
        let (c, _) = comm();
        assert!(c.set_job_on_hold(true, true));
        assert!(c.job_on_hold());
        {
            // counter 2 inside, back to 1 afterwards: no _continue_sending
            let _g = c.job_put_on_hold(true).unwrap();
            assert_eq!(2, c._job_on_hold.counter());
        }
        assert_eq!(1, c._job_on_hold.counter());
        assert!(c.job_on_hold());
    }

    #[test]
    fn job_on_hold_release_continues_sending() {
        let (c, _) = comm();
        c.set_job_on_hold(true, true);
        assert!(c.job_on_hold());
        // counter 0: `_continue_sending` runs (nothing is queued, the state is NONE: nothing is sent)
        assert!(c.set_job_on_hold(false, true));
        assert!(!c.job_on_hold());
    }

    #[test]
    fn close_without_serial() {
        let (c, rec) = comm();
        c.close(false, true, Some(10.0));
        assert!(!c._active());
        assert!(c._connection_closing.get());
        assert_eq!(MachineCom::STATE_CLOSED, c.get_state());
        assert_eq!(vec![11], *rec.states.lock().unwrap());
        assert_eq!(1, c._send_queue.qsize());
        // a second close does nothing
        c.close(false, true, Some(10.0));
        assert_eq!(1, c._send_queue.qsize());
        assert_eq!(vec![11], *rec.states.lock().unwrap());
    }

    #[test]
    fn close_with_serial_and_error() {
        let (c, _) = comm();
        let s = Arc::new(FakeSerial {
            fail_close: false,
            closed: Mutex::new(false),
        });
        c._serial.set(Some(s.clone()));
        c.close(true, true, Some(10.0));
        assert!(*s.closed.lock().unwrap());
        assert!(c._serial.get().is_none());
        assert_eq!(MachineCom::STATE_CLOSED_WITH_ERROR, c.get_state());

        let (c, _) = comm();
        let s = Arc::new(FakeSerial {
            fail_close: true,
            closed: Mutex::new(false),
        });
        c._serial.set(Some(s));
        c.close(false, true, Some(10.0)); // state NONE is not operational: no script
        assert_eq!(MachineCom::STATE_CLOSED_WITH_ERROR, c.get_state());

        let (c, _) = comm();
        c.close_with_legacy(false, true, None, Some(true)); // kwargs["isError"] wins over is_error
        assert_eq!(MachineCom::STATE_CLOSED_WITH_ERROR, c.get_state());
    }

    #[test]
    fn drop_closes() {
        let (c, rec) = comm();
        drop(c);
        assert_eq!(vec![11], *rec.states.lock().unwrap());
    }

    #[test]
    fn callback_defaults_do_nothing() {
        let cb = DefaultMachineComPrintCallback;
        cb.on_comm_log("x");
        cb.on_comm_state_change(1);
        cb.on_comm_error("e", "r", None, None, None);
        cb.on_comm_file_selected(Some("f"), None, false, None, None);
        cb.on_comm_print_job_started(false, None);
    }
}

#[cfg(test)]
mod section_f_tests {
    //! `tests/` of OctoPrint 1.11.8 has no test of `PrintingGcodeFileInformation` and the like, of
    //! `sendCommand` or of the methods of comm.py:1273-2259 (`tests/util/test_comm.py` mocks
    //! `MachineCom`), so every test here is ours. None of them calls `start()` (it would run the
    //! stubbed threads) or reaches a stub: states and arguments are chosen so that every command
    //! lands in a queue.
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Rec {
        selected: Mutex<Vec<(Option<String>, Option<i64>, bool, Option<String>)>>,
        sd_files: Mutex<Vec<Vec<Value>>>,
        sd_state: Mutex<Vec<bool>>,
        positions: Mutex<Vec<(String, String, Option<i64>)>>,
        logs: Mutex<Vec<String>>,
    }

    impl MachineComPrintCallback for Rec {
        fn on_comm_file_selected(
            &self,
            f: Option<&str>,
            size: Option<i64>,
            sd: bool,
            user: Option<&str>,
            _data: Option<&Value>,
        ) {
            self.selected.lock().unwrap().push((
                f.map(str::to_string),
                size,
                sd,
                user.map(str::to_string),
            ));
        }
        fn on_comm_sd_files(&self, files: &[Value]) {
            self.sd_files.lock().unwrap().push(files.to_vec());
        }
        fn on_comm_sd_state_change(&self, ready: bool) {
            self.sd_state.lock().unwrap().push(ready);
        }
        fn on_comm_record_fileposition(&self, origin: &str, name: &str, pos: Option<i64>) {
            self.positions
                .lock()
                .unwrap()
                .push((origin.to_string(), name.to_string(), pos));
        }
        fn on_comm_log(&self, message: &str) {
            self.logs.lock().unwrap().push(message.to_string());
        }
    }

    /// OURS: the profile of these tests (a `PrinterProfileManager` whose current profile it is)
    #[allow(non_snake_case)]
    fn Profile() -> PrinterProfileManager {
        PrinterProfileManager::with_current(serde_json::json!({
            "extruder": {"count": 2, "sharedNozzle": false},
            "heatedBed": true,
            "heatedChamber": true
        }))
    }

    fn comm() -> (Arc<MachineCom>, Arc<Rec>) {
        let rec = Arc::new(Rec::default());
        let c = MachineCom::new(None, None, Some(rec.clone()), Some(Arc::new(Profile())));
        (c, rec)
    }

    fn temp_file(content: &[u8]) -> String {
        use std::sync::atomic::AtomicUsize;
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "maker-connect-comm-f-test-{}-{}.gcode",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::write(&path, content).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn tags(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn cmd_text(item: &CommandQueueItem) -> String {
        match &item.0 {
            QueuedCommand::Command(c) => c.clone(),
            QueuedCommand::Marker(_) => "<marker>".to_string(),
        }
    }

    fn drain_job(c: &MachineCom) -> Vec<CommandQueueItem> {
        let mut r = Vec::new();
        while let Ok(i) = c._job_queue.get(false, None) {
            r.push(i);
        }
        r
    }

    // ---- file information classes

    fn all_lines(f: &dyn PrintingFileInformation) -> Vec<(String, i64, i64)> {
        let mut r = Vec::new();
        loop {
            match f.get_next().unwrap() {
                (Some(l), Some(p), Some(n)) => r.push((l, p, n)),
                other => {
                    assert_eq!((None, None, None), other);
                    return r;
                }
            }
        }
    }

    #[test]
    fn ours_gcode_file_reads_lines() {
        let path = temp_file(b"G1 X1 ; move\r\n; comment\r\n\r\nM104 S200\nG28");
        let f = PrintingGcodeFileInformation::new(&path, None, None, Some("alice")).unwrap();
        assert_eq!(Some(40), f.get_filesize());
        assert_eq!(0, f.get_filepos());
        assert_eq!(0.0, f.get_progress());
        assert_eq!(None, f.get_start_time());
        assert_eq!("local", f.get_file_location());
        assert_eq!(Some("alice".to_string()), f.get_user());
        assert!(
            f.checksum()
                && !f.is_printing_sd_file_information()
                && !f.is_streaming_gcode_file_information()
        );
        // not open: nothing
        assert_eq!((None, None, None), f.get_next().unwrap());
        f.start().unwrap();
        assert!(f.get_start_time().is_some() && !f.done());
        let lines = all_lines(&f);
        assert_eq!(
            vec![
                ("G1 X1".to_string(), 14, 1),
                ("M104 S200".to_string(), 37, 2),
                ("G28".to_string(), 40, 3)
            ],
            lines
        );
        assert!(f.done());
        assert_eq!(1.0, f.get_progress());
        f.reset();
        assert_eq!(0, f.get_filepos());
    }

    #[test]
    fn ours_gcode_file_bom_and_cr_and_invalid_utf8() {
        let mut content = vec![0xef, 0xbb, 0xbf];
        content.extend_from_slice(b"G1\rG2\n\xffG3\n");
        let path = temp_file(&content);
        let f = PrintingGcodeFileInformation::new(&path, None, None, None).unwrap();
        f.start().unwrap();
        assert_eq!(3, f.get_filepos()); // the BOM is counted
        let lines = all_lines(&f);
        // \xff is U+FFFD (3 bytes) after the replacement
        assert_eq!(
            vec![
                ("G1".to_string(), 6, 1),
                ("G2".to_string(), 9, 2),
                ("\u{fffd}G3".to_string(), 15, 3)
            ],
            lines
        );
    }

    /// The bytes of the file `scripts/octoprint-file-read-check.py` reads with CPython 3.13.
    const BOM_FILE: &[u8] = b"\xef\xbb\xbfG28\r\n; caf\xc3\xa9\nG1 X10\rM104 S200\r\nG1 X20\n\nM84";

    /// OURS (the reference has no such test): a file with a UTF-8 BOM is opened and read, then a
    /// new handle seeks to the resume position (`startPrint` with `pos`). The expected lines and
    /// positions are what CPython 3.13.12's `open(f, encoding="utf-8-sig", errors="replace",
    /// newline="")`, `readline()` and `seek()` give for the same file
    /// (`scripts/octoprint-file-read-check.py`, output copied below).
    #[test]
    fn ours_gcode_file_bom_read_then_resume_seek() {
        let path = temp_file(BOM_FILE);

        // Python: first_three [["G28\r\n", 8], ["; café\n", 16], ["G1 X10\r", 23]], with `_pos` = 3 (the
        // BOM) + `len(line.encode("utf-8"))` per line, as getNext keeps it
        let mut h = TextHandle::open(&path).unwrap();
        let mut pos = 3;
        for (line, want_pos) in [("G28\r\n", 8), ("; caf\u{e9}\n", 16), ("G1 X10\r", 23)] {
            let got = h.readline().unwrap();
            pos += got.len() as i64;
            assert_eq!((line, want_pos), (got.as_str(), pos));
        }

        // Python: resume_pos 23, tell_after_seek 23, after_seek [["M104 S200\r\n", 34], ["G1 X20\n", 41],
        // ["\n", 42], ["M84", 45], ["", 45]] (a new handle, `seek(23)`, `_pos = tell()`)
        let mut h = TextHandle::open(&path).unwrap();
        h.seek(23).unwrap();
        let mut pos = 23;
        for (line, want_pos) in [
            ("M104 S200\r\n", 34),
            ("G1 X20\n", 41),
            ("\n", 42),
            ("M84", 45),
            ("", 45),
        ] {
            let got = h.readline().unwrap();
            pos += got.len() as i64;
            assert_eq!((line, want_pos), (got.as_str(), pos));
        }

        // Python: after_seek0 ["G28\r\n", 5] (the BOM is dropped again, `_pos` = 0 + 5)
        let mut h = TextHandle::open(&path).unwrap();
        h.readline().unwrap();
        h.seek(0).unwrap();
        assert_eq!("G28\r\n", h.readline().unwrap());
    }

    /// OURS: the same file through `PrintingGcodeFileInformation` (`start`, `getNext`, then a new
    /// object with `seek(resume)` as `startPrint` does); positions are those of the Python values
    /// in `ours_gcode_file_bom_read_then_resume_seek`. Comment and blank lines give no gcode, so
    /// `getNext` goes on to the next line.
    #[test]
    fn ours_gcode_file_bom_start_print_with_pos() {
        let path = temp_file(BOM_FILE);
        let f = PrintingGcodeFileInformation::new(&path, None, None, None).unwrap();
        f.start().unwrap();
        assert_eq!(3, f.get_filepos());
        assert_eq!(
            (Some("G28".to_string()), Some(8), Some(1)),
            f.get_next().unwrap()
        );
        // "; café\n" ends at 16 and is skipped, "G1 X10\r" ends at 23
        assert_eq!(
            (Some("G1 X10".to_string()), Some(23), Some(2)),
            f.get_next().unwrap()
        );
        f.close();

        let f = PrintingGcodeFileInformation::new(&path, None, None, None).unwrap();
        f.start().unwrap();
        f.seek(23).unwrap();
        assert_eq!(23, f.get_filepos());
        // "\n" (42) is skipped
        assert_eq!(
            vec![
                ("M104 S200".to_string(), 34, 1),
                ("G1 X20".to_string(), 41, 2),
                ("M84".to_string(), 45, 3)
            ],
            all_lines(&f)
        );
        assert_eq!(45, f.get_filepos());
    }

    #[test]
    fn ours_gcode_file_offsets_and_tool() {
        let path = temp_file(b"M104 S200\nM140 S60\n");
        let offsets: OffsetsCallback = Arc::new(|| {
            [("tool0".to_string(), 5.0), ("bed".to_string(), -10.0)]
                .into_iter()
                .collect()
        });
        let tool: CurrentToolCallback = Arc::new(|| 0);
        let f = PrintingGcodeFileInformation::new(&path, Some(offsets), Some(tool), None).unwrap();
        f.start().unwrap();
        let lines: Vec<String> = all_lines(&f).into_iter().map(|l| l.0).collect();
        assert_eq!(
            vec![
                "M104 S205.000000".to_string(),
                "M140 S50.000000".to_string()
            ],
            lines
        );
    }

    #[test]
    fn ours_gcode_file_seek_close_missing() {
        let path = temp_file(b"G1\nG2\nG3\n");
        let f = PrintingGcodeFileInformation::new(&path, None, None, None).unwrap();
        f.seek(3).unwrap(); // not open: nothing
        assert_eq!(0, f.get_filepos());
        f.start().unwrap();
        f.seek(3).unwrap();
        assert_eq!(3, f.get_filepos());
        assert_eq!(
            vec![("G2".to_string(), 6, 1), ("G3".to_string(), 9, 2)],
            all_lines(&f)
        );
        f.start().unwrap();
        f.close();
        assert_eq!((None, None, None), f.get_next().unwrap());

        assert_eq!(
            Err(OSError(
                "File /nonexistent/x.gcode does not exist".to_string()
            )),
            PrintingGcodeFileInformation::new("/nonexistent/x.gcode", None, None, None).map(|_| ())
        );
        // a directory is not a file
        assert!(
            PrintingGcodeFileInformation::new(
                std::env::temp_dir().to_str().unwrap(),
                None,
                None,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn ours_streaming_and_special_files() {
        let path = temp_file(b"G1 X1 ; c\n\n  raw line  \n");
        let s =
            StreamingGcodeFileInformation::new(&path, "local.gcode", "/remote.gco", None).unwrap();
        assert!(s.is_streaming_gcode_file_information() && s.checksum());
        assert_eq!("local.gcode", s.get_local_filename());
        assert_eq!("/remote.gco", s.get_remote_filename());
        s.start().unwrap();
        let lines: Vec<String> = all_lines(&s).into_iter().map(|l| l.0).collect();
        assert_eq!(vec!["G1 X1".to_string(), "raw line".to_string()], lines);

        let sp = SpecialStreamingGcodeFileInformation::new(&path, "local.bin", "/remote.bin", None)
            .unwrap();
        assert!(sp.is_streaming_gcode_file_information() && !sp.checksum());
        assert_eq!("/remote.bin", sp.get_remote_filename());
        sp.start().unwrap();
        let lines: Vec<String> = all_lines(&sp).into_iter().map(|l| l.0).collect();
        assert_eq!(
            vec!["G1 X1 ; c".to_string(), "  raw line".to_string()],
            lines
        );
    }

    #[test]
    fn ours_sd_file_information() {
        let f = PrintingSdFileInformation::new("a.gco", Some(200), Some("bob"));
        assert!(!f.checksum() && f.is_printing_sd_file_information());
        assert_eq!("sdcard", f.get_file_location());
        assert_eq!(Some(200), f.size());
        f.set_pos(50);
        assert_eq!(50, f.get_filepos());
        assert_eq!(0.25, f.get_progress());
        f.set_size(Some(0));
        assert_eq!(-1.0, f.get_progress());
        f.set_size(None);
        assert_eq!(-1.0, f.get_progress());
        f.set_done(true);
        assert!(f.done());
        f.start().unwrap();
        assert!(!f.done());
    }

    #[test]
    #[should_panic(expected = "AttributeError")]
    fn ours_sd_file_has_no_get_next() {
        let _ = PrintingSdFileInformation::new("a.gco", None, None).get_next();
    }

    // ---- sendCommand and scripts

    fn cmd(s: &str) -> QueuedCommand {
        QueuedCommand::Command(s.to_string())
    }

    #[test]
    fn ours_send_command_to_job_queue() {
        let (c, _) = comm();
        assert_eq!(
            Some(true),
            c.send_command(
                cmd("G1 X1 ; c"),
                None,
                true,
                false,
                false,
                None,
                Some(tags(&["a"]))
            )
        );
        assert_eq!(
            Some(false),
            c.send_command(
                cmd("; only a comment"),
                None,
                true,
                false,
                false,
                None,
                None
            )
        );
        // processed: no stripping
        assert_eq!(
            Some(true),
            c.send_command(cmd("G1 ; c"), Some("t"), true, true, false, None, None)
        );
        let items = drain_job(&c);
        assert_eq!(2, items.len());
        assert_eq!("G1 X1", cmd_text(&items[0]));
        assert_eq!(tags(&["a", "source:job"]), items[0].3);
        assert_eq!("G1 ; c", cmd_text(&items[1]));
        assert_eq!(Some("t".to_string()), items[1].1);
    }

    #[test]
    fn ours_send_command_while_printing_goes_to_command_queue() {
        let (c, _) = comm();
        // not operational, not forced: nothing
        assert_eq!(
            None,
            c.send_command(cmd("G1"), None, false, false, false, None, None)
        );
        c._state.set(MachineCom::STATE_PRINTING);
        assert_eq!(
            Some(true),
            c.send_command(
                cmd("M105"),
                Some("temperature"),
                false,
                false,
                false,
                None,
                None
            )
        );
        // the same type twice: TypeAlreadyInQueue
        assert_eq!(
            Some(false),
            c.send_command(
                cmd("M105"),
                Some("temperature"),
                false,
                false,
                false,
                None,
                None
            )
        );
        assert_eq!(
            Some(true),
            c.send_command(
                cmd("G1"),
                None,
                false,
                false,
                false,
                None,
                Some(tags(&["x"]))
            )
        );
        let first = c._command_queue.get(false, None).unwrap();
        assert_eq!("M105", cmd_text(&first));
        let second = c._command_queue.get(false, None).unwrap();
        assert_eq!(tags(&["x"]), second.3);
        assert!(c._command_queue.get(false, None).is_err());
        c._state.set(MachineCom::STATE_NONE);
    }

    #[test]
    fn ours_send_command_marker_goes_to_queue_untouched() {
        let (c, _) = comm();
        let marker = QueuedCommand::Marker(SendQueueMarker::new(None).0);
        assert_eq!(
            Some(true),
            c.send_command(marker, None, true, false, false, None, None)
        );
        assert_eq!("<marker>", cmd_text(&drain_job(&c)[0]));
    }

    #[test]
    fn ours_get_gcode_script_cancel() {
        let (c, _) = comm();
        assert_eq!(
            Vec::<(String, BTreeSet<String>)>::new(),
            c._get_gcode_script("nonexistent", None)
        );
        let lines: Vec<String> = c
            ._get_gcode_script("afterPrintCancelled", None)
            .into_iter()
            .map(|l| l.0)
            .collect();
        assert_eq!(
            vec!["M84", "M104 T0 S0", "M104 T1 S0", "M140 S0", "M106 S0"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>(),
            lines
        );
    }

    #[test]
    fn ours_send_gcode_script_queues_with_tags() {
        let (c, _) = comm();
        let text = c.send_gcode_script("afterPrintCancelled", None, Some(tags(&["t"])), true);
        assert_eq!("M84\nM104 T0 S0\nM104 T1 S0\nM140 S0\nM106 S0", text);
        let items = drain_job(&c);
        assert_eq!(5, items.len());
        assert_eq!(
            tags(&[
                "t",
                "trigger:comm.send_gcode_script",
                "source:script",
                "script:afterPrintCancelled",
                "source:job"
            ]),
            items[0].3
        );
        assert_eq!("", c.send_gcode_script("nonexistent", None, None, true));
    }

    /// OURS (the reference has no such test): `_getGcodeScript` gives the template
    /// `last_temperature` with int tool keys, then "b", "c", then the custom ones in insertion
    /// order. The expected text is what the real OctoPrint 1.11.8 renders for the same script and
    /// the same `TemperatureRecord` (`scripts/octoprint-script-context-check.py`).
    #[test]
    fn ours_get_gcode_script_int_keys() {
        let (c, _) = comm();
        c.last_temperature.with_mut(|t| {
            t.set_tool(0, Some(21.5), Some(0.0));
            t.set_tool(1, Some(30.0), Some(210.0));
            t.set_bed(Some(22.0), Some(60.0));
            t.set_custom("zz", Some(1.0), None).unwrap();
            t.set_custom("aa", Some(2.0), None).unwrap();
        });
        let settings = Settings::with_gcode_script(
            "t_int_keys",
            "{{ last_temperature[0].actual }} {{ last_temperature[1].target }} {{ last_temperature.b.actual }}\n\
             {% for k in last_temperature %}{{ k }},{% endfor %}",
        );
        let lines: Vec<String> = c
            .get_gcode_script_in(&settings, "t_int_keys", None)
            .into_iter()
            .map(|l| l.0)
            .collect();
        // Python's output: "21.5 210.0 22.0\n0,1,b,c,zz,aa,"
        assert_eq!(vec!["21.5 210.0 22.0", "0,1,b,c,zz,aa,"], lines);
    }

    #[test]
    fn ours_script_context_replacements() {
        let (c, _) = comm();
        c._printerProfileManager.as_ref().unwrap(); // the profile is in the context
        // a replacement is visible to the template; the comm's own entries win over it
        let lines = c._get_gcode_script(
            "afterPrintCancelled",
            Some(&serde_json::json!({"x": 1, "printer_profile": 5})),
        );
        assert_eq!(5, lines.len());
    }

    // ---- selectFile & co

    #[test]
    fn ours_select_and_unselect_file() {
        let (c, rec) = comm();
        let path = temp_file(b"G1 X1\n");
        c.select_file(&path, false, Some("u"), None).unwrap();
        assert_eq!(
            vec![(Some(path.clone()), Some(6), false, Some("u".to_string()))],
            *rec.selected.lock().unwrap()
        );
        let f = c._currentFile.get().unwrap();
        assert_eq!(path, f.get_filename());
        c.unselect_file();
        assert!(c._currentFile.get().is_none());
        assert_eq!((None, None, false, None), rec.selected.lock().unwrap()[1]);
        assert!(
            c.select_file("/nonexistent.gcode", false, None, None)
                .is_err()
        );
        // not operational: SD is not used
        assert!(c.select_file("a.gco", true, None, None).is_ok());
        assert!(c._sdFileToSelect.get().is_none());
    }

    #[test]
    fn ours_select_file_callbacks_read_the_comm() {
        let (c, _) = comm();
        let path = temp_file(b"M104 S100\n");
        c._tempOffsets.with_mut(|d| d.insert("tool0", 10.0));
        c.select_file(&path, false, None, None).unwrap();
        let f = c._currentFile.get().unwrap();
        f.start().unwrap();
        assert_eq!("M104 S110.000000", f.get_next().unwrap().0.unwrap());
    }

    #[test]
    fn ours_select_sd_file() {
        let (c, _) = comm();
        c._sdRelativePath.set(Some(true));
        // operational and not busy: `_sendCommand` would run (a stub): only test the busy return
        c._state.set(MachineCom::STATE_PRINTING);
        c.select_file("/a.gco", true, Some("u"), None).unwrap();
        assert!(c._sdFileToSelect.get().is_none());
        c._state.set(MachineCom::STATE_NONE);
    }

    #[test]
    fn ours_unselect_while_busy_does_nothing() {
        let (c, rec) = comm();
        c._currentFile
            .set(Some(Arc::new(PrintingSdFileInformation::new(
                "a", None, None,
            ))));
        c._state.set(MachineCom::STATE_PRINTING);
        c.unselect_file();
        assert!(c._currentFile.get().is_some());
        assert!(rec.selected.lock().unwrap().is_empty());
        c._currentFile.set(None);
        c._state.set(MachineCom::STATE_NONE);
    }

    // ---- SD card

    #[test]
    fn ours_get_sd_files() {
        let (c, _) = comm();
        c._sdFiles.with_mut(|d| {
            d.insert(
                "a.gco",
                SDFileData {
                    name: "a.gco".into(),
                    size: Some(10),
                    timestamp: Some(5),
                    longname: None,
                },
            );
            d.insert(
                "B",
                SDFileData {
                    name: "B".into(),
                    size: None,
                    timestamp: None,
                    longname: Some("Long B.gcode".into()),
                },
            );
        });
        assert_eq!(
            vec![
                ("a.gco".to_string(), Some(10), "a.gco".to_string(), Some(5)),
                ("B".to_string(), None, "Long B.gcode".to_string(), None)
            ],
            c.get_sd_files()
        );
    }

    #[test]
    fn ours_sd_card_commands_while_printing() {
        let (c, rec) = comm();
        c._state.set(MachineCom::STATE_PRINTING);
        c.init_sd_card(Some(tags(&["t"])));
        c.release_sd_card(None);
        c._sdFiles.with_mut(|d| {
            d.insert(
                "a.gco",
                SDFileData {
                    name: "a.gco".into(),
                    size: None,
                    timestamp: None,
                    longname: None,
                },
            )
        });
        c.delete_sd_file("DIR/File.GCO", None);
        c.refresh_sd_files(None, false, Some(10.0)); // busy: nothing
        let mut texts = Vec::new();
        let mut all_tags = Vec::new();
        while let Ok(i) = c._command_queue.get(false, None) {
            texts.push(cmd_text(&i));
            all_tags.push(i.3);
        }
        assert_eq!(vec!["M21", "M22", "M30 dir/file.gco"], texts);
        assert_eq!(tags(&["t", "trigger:comm.init_sd_card"]), all_tags[0]);
        assert_eq!(tags(&["trigger:comm.delete_sd_file"]), all_tags[2]);
        assert_eq!(vec![false], *rec.sd_state.lock().unwrap());
        assert_eq!(vec![Vec::<Value>::new()], *rec.sd_files.lock().unwrap());
        c._state.set(MachineCom::STATE_NONE);
    }

    #[test]
    fn ours_sd_card_commands_do_nothing_when_not_operational() {
        let (c, rec) = comm();
        c.init_sd_card(None);
        c.release_sd_card(None);
        c.delete_sd_file("a.gco", None);
        c.refresh_sd_files(None, true, Some(0.0));
        assert!(rec.sd_state.lock().unwrap().is_empty());
        assert!(c._command_queue.get(false, None).is_err());
    }

    #[test]
    fn ours_reset_line_numbers_and_hello_tags() {
        let (c, _) = comm();
        c.reset_line_numbers(0, true, None); // not operational
        assert!(drain_job(&c).is_empty());
        c._state.set(MachineCom::STATE_PRINTING);
        c.reset_line_numbers(7, true, Some(tags(&["t"])));
        let items = drain_job(&c);
        assert_eq!("M110 N7", cmd_text(&items[0]));
        assert_eq!(
            tags(&["t", "trigger:comm.reset_line_numbers", "source:job"]),
            items[0].3
        );
        c._state.set(MachineCom::STATE_NONE);
    }

    // ---- print control

    #[test]
    fn ours_start_print_guards() {
        let (c, _) = comm();
        assert_eq!(Ok(()), c.start_print(None, None, false, None)); // not operational
        c._state.set(MachineCom::STATE_OPERATIONAL);
        assert_eq!(
            Err(ValueError("No file selected for printing")),
            c.start_print(None, None, false, None)
        );
        c._state.set(MachineCom::STATE_PRINTING);
        assert_eq!(Ok(()), c.start_print(None, None, false, None)); // already printing
        c._state.set(MachineCom::STATE_NONE);
    }

    #[test]
    fn ours_cancel_pause_and_transfer_guards() {
        let (c, _) = comm();
        c.cancel_print(None, false, None, None, false); // not operational
        c._state.set(MachineCom::STATE_OPERATIONAL);
        c.cancel_print(None, false, None, None, false); // not busy
        c.cancel_file_transfer(None); // not streaming
        c.set_pause(true, true, None, None); // no file
        c._currentFile
            .set(Some(Arc::new(PrintingSdFileInformation::new(
                "a", None, None,
            ))));
        c.set_pause(true, true, None, None); // not in a pausable state
        assert_eq!(MachineCom::STATE_OPERATIONAL, c.get_state());
        c._state.set(MachineCom::STATE_CANCELLING);
        c.cancel_print(None, false, None, None, false); // already cancelling
        c._state.set(MachineCom::STATE_PRINTING);
        c._currentFile.set(Some(Arc::new(
            StreamingGcodeFileInformation::new(&temp_file(b""), "l", "/r", None).unwrap(),
        )));
        c.set_pause(true, true, None, None); // streaming
        assert_eq!(MachineCom::STATE_PRINTING, c.get_state());
        c._currentFile.set(None);
        c._state.set(MachineCom::STATE_NONE);
    }

    #[test]
    fn ours_start_file_transfer_not_operational() {
        let (c, _) = comm();
        assert_eq!(
            Ok(None),
            c.start_file_transfer("p", "f.gcode", None, false, None)
        );
    }

    #[test]
    fn ours_preparation_done_without_timer_pops_the_user_and_returns() {
        let (c, _) = comm();
        c._action_users.with_mut(|d| {
            d.insert("cancel", Some("u".to_string()));
            d.insert("pause", Some("v".to_string()));
        });
        c._cancel_preparation_done(true, None);
        c._pause_preparation_done(true, false, None);
        assert!(c._action_users.with(|d| d.is_empty()));
    }

    #[test]
    fn ours_preparation_failed_logs_and_clears_the_record_flag() {
        let (c, rec) = comm();
        c._record_cancel_data.set(true);
        c._cancel_preparation_failed(); // no timer: returns after the log
        assert!(!c._record_cancel_data.get());
        c._record_pause_data.set(true);
        c._pause_preparation_failed();
        assert!(!c._record_pause_data.get());
        let logs = rec.logs.lock().unwrap();
        assert_eq!(
            "Did not receive parseable position data from printer within 10.0s, continuing without it",
            logs[0]
        );
        assert_eq!(2, logs.len());
    }

    #[test]
    fn ours_file_position() {
        let (c, rec) = comm();
        assert_eq!(None, c.get_file_position());
        c._record_file_position(); // no file: nothing
        assert!(rec.positions.lock().unwrap().is_empty());
        let f = PrintingSdFileInformation::new("a.gco", Some(10), None);
        f.set_pos(4);
        c._currentFile.set(Some(Arc::new(f)));
        assert_eq!(
            Some(FilePosition {
                origin: "sdcard",
                filename: "a.gco".to_string(),
                pos: 4
            }),
            c.get_file_position()
        );
        c._record_file_position();
        assert_eq!(
            vec![("sdcard".to_string(), "a.gco".to_string(), Some(4))],
            *rec.positions.lock().unwrap()
        );
    }

    #[test]
    fn ours_set_temperature_offset() {
        let (c, _) = comm();
        c.set_temperature_offset([("tool0".to_string(), 5.0), ("bed".to_string(), -2.0)]);
        c.set_temperature_offset([("tool0".to_string(), 6.0)]);
        assert_eq!(Some(&6.0), c.get_offsets().get("tool0"));
        assert_eq!(Some(&-2.0), c.get_offsets().get("bed"));
    }

    // ---- _processTemperatures

    #[test]
    fn ours_process_temperatures() {
        let (c, _) = comm();
        c._process_temperatures("ok T:21.3 /0.0 B:22.1 /60.0 C:30 /0 X:5");
        assert_eq!(vec![(0, (Some(21.3), Some(0.0)))], c.get_temp());
        assert_eq!((Some(22.1), Some(60.0)), c.get_bed_temp());
        assert_eq!(
            (Some(30.0), Some(0.0)),
            c.last_temperature.with(|t| t.chamber())
        );
    }

    /// The custom temperatures are recorded in the order the line gave them (Python's dict order).
    #[test]
    fn ours_process_temperatures_custom_in_line_order() {
        let (c, _) = comm();
        c._process_temperatures("T:20 /0 Z:1 /0 A:2 /0 M:3 /0");
        assert_eq!(
            vec!["Z", "A", "M"],
            c.last_temperature
                .with(|t| t.custom().into_iter().map(|(k, _)| k).collect::<Vec<_>>())
        );
        let (_, parsed) = parse_temperature_line("T:20 Z:1 A:2 B:3", 0);
        // `T` becomes `T0` at the end (`result["T0"] = result["T"]; del result["T"]`)
        assert_eq!(
            vec!["Z", "A", "B", "T0"],
            parsed.keys().map(|k| k.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ours_process_temperatures_tools_and_current_tool() {
        let (c, _) = comm();
        c._process_temperatures("T0:20 /0 T1:30 /210");
        assert_eq!(
            vec![(0, (Some(20.0), Some(0.0))), (1, (Some(30.0), Some(210.0)))],
            c.get_temp()
        );
        // current tool 1: a bare `T:` is tool 1's
        c._currentTool.set(1);
        c._process_temperatures("T:40 /220");
        assert_eq!(
            vec![(0, (Some(20.0), Some(0.0))), (1, (Some(40.0), Some(220.0)))],
            c.get_temp()
        );
    }

    #[test]
    fn ours_process_temperatures_chamber_needs_support_or_profile() {
        // Profile says heatedChamber is true; with a profile that says false and no capability the
        // chamber is not set: the same code with another manager
        let no_chamber = PrinterProfileManager::with_current(
            serde_json::json!({"extruder": {"count": 1, "sharedNozzle": true}, "heatedChamber": false}),
        );
        let c = MachineCom::new(None, None, None, Some(Arc::new(no_chamber)));
        c._process_temperatures("T0:20 /0 C:30 /0");
        assert_eq!((None, None), c.last_temperature.with(|t| t.chamber()));
        // shared nozzle: T0's value is every tool's up to the profile's count
        assert_eq!(vec![(0, (Some(20.0), Some(0.0)))], c.get_temp());
    }

    #[test]
    fn ours_py_float_repr() {
        for (x, s) in [
            (10.0, "10.0"),
            (0.5, "0.5"),
            (0.0, "0.0"),
            (-1.25, "-1.25"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (123456.789, "123456.789"),
            (1.5e-7, "1.5e-07"),
        ] {
            assert_eq!(s, py_float_repr(x));
        }
    }
}

#[cfg(test)]
mod section_g_tests {
    //! `tests/` of OctoPrint 1.11.8 has no test of `_monitor` (`tests/util/test_comm.py` tests
    //! `_handle_errors` and the module-level parsers, which are ported above). All tests here are
    //! OURS: `_monitor` reaches a stub (`_open_serial`, `_perform_detection_step`) before it reads
    //! a single line, so it cannot be driven by a fake transport yet; the pieces are tested.
    use super::*;

    #[test]
    fn ours_monitor_with_port_and_baudrate_opens_the_serial_port() {
        // a port that does not exist: `_open_serial` fails (the default factory returns `None`),
        // `_monitor` returns with the state of the opening still set
        let c = MachineCom::new(
            Some("/dev/maker-connect-test-does-not-exist".to_string()),
            Some(115200),
            None,
            None,
        );
        c._monitor();
        assert_eq!(c.get_state(), MachineCom::STATE_OPEN_SERIAL);
        assert!(c._serial.get().is_none());
    }

    #[test]
    fn ours_py_try_catches_exceptions_but_not_stubs() {
        assert_eq!(py_try(|| 1), Ok(1));
        assert_eq!(
            py_try(|| -> i32 { panic!("TypeError: boom") }),
            Err("TypeError: boom".to_string())
        );
        let r =
            std::panic::catch_unwind(|| py_try(|| -> i32 { unimplemented!("comm.py:1, test") }));
        assert!(r.is_err());
    }

    #[test]
    fn ours_re_match_is_anchored_at_the_start() {
        let m = re_match(&regex_repetierTempExtr, "TargetExtr1:200.5").unwrap();
        assert_eq!(m.get(1).unwrap().as_str(), "1");
        assert_eq!(m.get(2).unwrap().as_str(), "200.5");
        assert!(re_match(&regex_repetierTempExtr, " TargetExtr1:200.5").is_none());
        assert!(re_match(&regex_repetierTempBed, "TargetBed:60").is_some());
        assert!(re_search(&regex_repetierTempBed, "x TargetBed:60"));
    }

    #[test]
    fn ours_filter_non_ascii_and_utf8() {
        assert!(!filter_non_ascii("abc.gco"));
        assert!(filter_non_ascii("äbc.gco"));
        assert!(!filter_non_utf8("äbc.gco"));
    }

    #[test]
    fn ours_settings_to_controls_and_triggers() {
        let controls = serde_json::json!([
            {"name": "g", "children": [{"key": "k1", "regex": "a(b)", "template": "t", "template_key": "x"}]}
        ]);
        let c = feedback_controls_from_value(&Some(controls));
        assert_eq!(c.len(), 1);
        let child = &c[0].children.as_ref().unwrap()[0];
        assert_eq!(child.key.as_deref(), Some("k1"));
        assert_eq!(child.regex, Some(PyText::Str("a(b)".to_string())));
        assert!(feedback_controls_from_value(&None).is_empty());

        let t = pause_triggers_from_value(&Some(
            serde_json::json!([{"regex": "pause", "type": "enable"}]),
        ));
        assert_eq!(t[0]["regex"], "pause");
        assert!(pause_triggers_from_value(&None).is_empty());
    }

    #[test]
    fn ours_position_record_valid_is_set_never_copied() {
        let mut a = PositionRecord::new(&[]);
        assert_eq!(a.valid, None);
        a.valid = Some(true);
        let mut b = PositionRecord::new(&[]);
        b.copy_from(&a);
        assert_eq!(b.valid, None);
    }

    #[test]
    fn ours_sd_file_information_size() {
        let f = PrintingSdFileInformation::new("a.gco", Some(0), None);
        let f: &dyn PrintingFileInformation = &f;
        assert_eq!(f.size(), Some(0));
        f.set_size(Some(5));
        assert_eq!(f.size(), Some(5));
    }
}

#[cfg(test)]
mod section_h_tests {
    //! Section H (comm.py:3308-4508).
    //!
    //! `TestCommErrorHandling` (`tests/util/test_comm.py:12-224`) is ported first, case by case.
    //! Python tests `_handle_errors` / `_trigger_error` on `mock.create_autospec(MachineCom)` with
    //! only those two methods real. There are no autospec mocks here: the object is a real
    //! `MachineCom` (state PRINTING for `isPrinting`, no file for `isSdPrinting`, state ERROR for
    //! `isError`) and each mock assertion is read off what the real methods do:
    //!   * `_handle_ok` called  = `_ok_timeout` is set (only `_handle_ok` sets it here);
    //!   * `_clear_to_send.set` called = `_clear_to_send` is set (the real `_handle_ok` sets it too:
    //!     in `assert_recoverable` that is `_handle_ok`'s own, so it is checked to be set once);
    //!   * `_changeState(STATE_ERROR)` + `close(is_error=True)` = the states reported are
    //!     `[ERROR, CLOSED_WITH_ERROR]` (`close` is real) and `_errorValue` is set;
    //!   * `cancelPrint` called = the `on_comm_error` callback was called with consequence
    //!     `"cancel"` (`_handle_errors` calls it right before `cancelPrint`, which is real and
    //!     returns at once for there is no file);
    //!   * `_trigger_emergency_stop(close=False)` = the real method (comm.py:5487) writes M112 to a
    //!     recording serial object.
    //! Every other test here is ours (`ours_*`): OctoPrint has none for these methods. None of them
    //! opens a serial port, reaches a stub or starts a thread.
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Rec {
        states: Mutex<Vec<i64>>,
        errors: Mutex<
            Vec<(
                String,
                String,
                Option<String>,
                Option<String>,
                Option<Vec<String>>,
            )>,
        >,
        logs: Mutex<Vec<String>>,
        progress: Mutex<usize>,
        job_done: Mutex<usize>,
    }

    impl MachineComPrintCallback for Rec {
        fn on_comm_state_change(&self, state: i64) {
            self.states.lock().unwrap().push(state);
        }
        fn on_comm_error(
            &self,
            error: &str,
            reason: &str,
            consequence: Option<&str>,
            faq: Option<&str>,
            logs: Option<&[String]>,
        ) {
            self.errors.lock().unwrap().push((
                error.to_string(),
                reason.to_string(),
                consequence.map(str::to_string),
                faq.map(str::to_string),
                logs.map(<[String]>::to_vec),
            ));
        }
        fn on_comm_log(&self, message: &str) {
            self.logs.lock().unwrap().push(message.to_string());
        }
        fn on_comm_progress(&self) {
            *self.progress.lock().unwrap() += 1;
        }
        fn on_comm_print_job_done(&self, _suppress_script: bool) {
            *self.job_done.lock().unwrap() += 1;
        }
    }

    /// what `EventManager::fire` got, from every test (filter by what the test sent)
    static EVENTS: Mutex<Vec<(String, Option<Value>)>> = Mutex::new(Vec::new());

    struct Sink;

    impl super::super::events::EventSink for Sink {
        fn fire(&self, event: &str, payload: Option<Value>) {
            EVENTS.plock().push((event.to_string(), payload));
        }
    }

    fn install_sink() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| event_manager().set_sink(Arc::new(Sink)));
    }

    /// a scripted serial object: `readline` returns the queued results, then `b""`
    struct Scripted {
        lines: Mutex<std::collections::VecDeque<Result<Vec<u8>, SerialError>>>,
        closed: Mutex<bool>,
        written: Mutex<Vec<u8>>,
    }

    impl Scripted {
        fn new(lines: Vec<Result<&[u8], SerialError>>) -> Arc<Self> {
            Arc::new(Scripted {
                lines: Mutex::new(lines.into_iter().map(|l| l.map(<[u8]>::to_vec)).collect()),
                closed: Mutex::new(false),
                written: Mutex::new(Vec::new()),
            })
        }
    }

    impl SerialTransport for Scripted {
        fn port(&self) -> Option<Option<String>> {
            Some(Some("/dev/ttyFAKE".to_string()))
        }
        fn close(&self) -> Result<(), String> {
            *self.closed.lock().unwrap() = true;
            Ok(())
        }
        fn str(&self) -> String {
            "Scripted".to_string()
        }
        fn set_timeout(&self, _timeout: f64) {}
        fn timeout(&self) -> Option<f64> {
            None
        }
        fn set_baudrate(&self, _baudrate: i64) {}
        fn readline(&self) -> Result<Vec<u8>, SerialError> {
            self.lines
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(Vec::new()))
        }
        fn write(&self, data: &[u8]) -> Result<Option<i64>, SerialError> {
            self.written.lock().unwrap().extend_from_slice(data);
            Ok(Some(data.len() as i64))
        }
    }

    /// the printer profile `_trigger_emergency_stop` reads (OURS)
    #[allow(non_snake_case)]
    fn StaticProfile() -> PrinterProfileManager {
        PrinterProfileManager::with_current(
            serde_json::json!({"extruder": {"count": 1}, "heatedBed": true, "heatedChamber": false}),
        )
    }

    fn new_comm(configure: impl FnOnce(&mut MachineCom)) -> (Arc<MachineCom>, Arc<Rec>) {
        install_sink();
        let rec = Arc::new(Rec::default());
        let mut c = MachineCom::new(None, None, Some(rec.clone()), None);
        configure(Arc::get_mut(&mut c).expect("nothing else holds the comm yet"));
        (c, rec)
    }

    // ---- tests/util/test_comm.py TestCommErrorHandling -----------------------------------------

    /// `TestCommErrorHandling.setUp`
    fn set_up(configure: impl FnOnce(&mut MachineCom)) -> (Arc<MachineCom>, Arc<Rec>) {
        let (c, rec) = new_comm(|c| {
            c._ignore_errors = Some(false);
            c._disconnect_on_errors = Some(true);
            c._send_m112_on_error = Some(true);
            configure(c);
        });
        c._state.set(MachineCom::STATE_PRINTING); // isPrinting True (isSdPrinting False, isError False)
        (c, rec)
    }

    fn assert_handle_ok(c: &MachineCom) {
        assert!(c._ok_timeout.get().is_some());
        assert_eq!(1, c._clear_to_send.counter()); // the real `_handle_ok` sets it, once
    }

    fn assert_not_handle_ok(c: &MachineCom) {
        assert!(c._ok_timeout.get().is_none());
    }

    fn assert_last_comm_error(c: &MachineCom) {
        assert!(c._lastCommError.get().is_some());
    }

    fn assert_not_last_comm_error(c: &MachineCom) {
        assert!(c._lastCommError.get().is_none());
    }

    fn assert_disconnected(c: &MachineCom, rec: &Rec) {
        assert!(!c.get_error_string().is_empty());
        assert_eq!(
            vec![MachineCom::STATE_ERROR, MachineCom::STATE_CLOSED_WITH_ERROR],
            *rec.states.lock().unwrap()
        );
    }

    fn assert_not_disconnected(c: &MachineCom, rec: &Rec) {
        assert!(c.get_error_string().is_empty());
        assert!(rec.states.lock().unwrap().is_empty());
    }

    fn assert_print_cancelled(rec: &Rec) {
        let errors = rec.errors.lock().unwrap();
        assert_eq!(
            1,
            errors
                .iter()
                .filter(|e| e.2.as_deref() == Some("cancel"))
                .count()
        );
    }

    fn assert_not_print_cancelled(rec: &Rec) {
        assert_eq!(
            0,
            rec.errors
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e.2.as_deref() == Some("cancel"))
                .count()
        );
    }

    fn assert_cleared_to_send(c: &MachineCom) {
        assert_eq!(1, c._clear_to_send.counter());
    }

    fn assert_not_cleared_to_send(c: &MachineCom) {
        assert_eq!(0, c._clear_to_send.counter());
    }

    fn assert_nop(c: &MachineCom, rec: &Rec) {
        assert_not_handle_ok(c);
        assert_not_last_comm_error(c);
        assert_not_disconnected(c, rec);
        assert_not_print_cancelled(rec);
        assert_not_cleared_to_send(c);
    }

    fn assert_recoverable(c: &MachineCom, rec: &Rec) {
        assert_handle_ok(c);

        assert_not_last_comm_error(c);
        assert_not_disconnected(c, rec);
        assert_not_print_cancelled(rec);
    }

    fn assert_resend(c: &MachineCom, rec: &Rec) {
        assert_last_comm_error(c);

        assert_not_handle_ok(c);
        assert_not_disconnected(c, rec);
        assert_not_print_cancelled(rec);
        assert_not_cleared_to_send(c);
    }

    #[test]
    fn test_lineno_mismatch() {
        for line in [
            // Marlin
            "Error: Line Number is not Last Line Number+1, Last Line: 1",
            // Repetier
            "Error: Expected Line 1 got 2",
            // !! error type for good measure
            "!! expected line 1 got 2",
        ] {
            let (c, rec) = set_up(|_| {});
            let result = c._handle_errors(Some(line));
            assert_eq!(Some(line.to_string()), result);
            assert_resend(&c, &rec);
        }
    }

    #[test]
    fn test_lineno_missing() {
        // Should simulate OK to force resend request
        for line in [
            // Marlin
            "Error: No Line Number with checksum, Last Line: 1",
        ] {
            let (c, rec) = set_up(|_| {});
            let result = c._handle_errors(Some(line));
            assert_eq!(Some(line.to_string()), result);
            assert_recoverable(&c, &rec);
        }
    }

    #[test]
    fn test_checksum_mismatch() {
        // Should prepare receiving resend request
        for line in [
            // Marlin
            "Error: checksum mismatch",
            // Repetier
            "Error: Wrong checksum",
        ] {
            let (c, rec) = set_up(|_| {});
            let result = c._handle_errors(Some(line));
            assert_eq!(Some(line.to_string()), result);
            assert_resend(&c, &rec);
        }
    }

    #[test]
    fn test_checksum_missing() {
        // Should prepare receiving resend request
        for line in [
            // Marlin
            "Error: No Checksum with line number, Last Line: 1",
            // Repetier
            "Error: Missing checksum",
        ] {
            let (c, rec) = set_up(|_| {});
            let result = c._handle_errors(Some(line));
            assert_eq!(Some(line.to_string()), result);
            assert_resend(&c, &rec);
        }
    }

    #[test]
    fn test_sd_error() {
        // Should pass
        for line in [
            // Marlin
            "Error: volume.init failed",
            "Error: openRoot failed",
            "Error: workDir open failed",
            "Error: Cannot enter subdir: folder",
            // Repetier
            "Error: file.open failed",
            // Marlin & Repetier (halleluja!)
            "Error: error writing to file",
            "Error: open failed, File: foo.gco",
            // Legacy?
            "Error: Cannot open foo.gco",
        ] {
            let (c, rec) = set_up(|_| {});
            let result = c._handle_errors(Some(line));
            assert_eq!(Some(line.to_string()), result);
            assert_nop(&c, &rec);
        }
    }

    #[test]
    fn test_unknown_command() {
        // Should pass
        for line in [
            // Marlin
            "Error: Unknown command: \"ABC\"",
            // Repetier
            "Error: Unknown command:ABC",
        ] {
            let (c, rec) = set_up(|_| {});
            let result = c._handle_errors(Some(line));
            assert_eq!(Some(line.to_string()), result);
            assert_nop(&c, &rec);
        }
    }

    #[test]
    fn test_unknown_handled() {
        // Should pass
        for line in [
            "Error: This should get handled",
            "!! This should also get handled",
        ] {
            let (c, rec) = set_up(|c| {
                // def handler(comm, message, *args, **kwargs): return "handled" in message
                let handler: super::super::plugin::HookFn =
                    Arc::new(|args| Value::Bool(args[0].as_str().unwrap().contains("handled")));
                c._error_message_hooks.insert("test", handler);
            });
            let result = c._handle_errors(Some(line));
            assert_eq!(Some(line.to_string()), result);
            assert_nop(&c, &rec);
        }
    }

    #[test]
    fn test_other_error_disconnect() {
        // Should trigger escalation
        let (c, rec) = set_up(|c| {
            // `_trigger_emergency_stop` reads the printer profile (the Python test mocks the method)
            c._printerProfileManager = Some(Arc::new(StaticProfile()));
        });
        let serial = Scripted::new(vec![]);
        c._serial.set(Some(serial.clone()));
        let result = c._handle_errors(Some("Error: Printer on fire"));
        assert_eq!(Some("Error: Printer on fire".to_string()), result);

        // what should have happened
        // assert_m112_sent: `_trigger_emergency_stop(close=False)` sent M112 (without and with checksum)
        // and then switched the heaters off, without closing in `_trigger_emergency_stop` itself
        let written = String::from_utf8(serial.written.lock().unwrap().clone()).unwrap();
        assert!(written.starts_with("M112\nN1 M112*32\n"), "{written:?}");
        assert!(written.contains("M104 T0 S0*"), "{written:?}");
        assert!(written.contains("M140 S0*"), "{written:?}");
        assert_disconnected(&c, &rec);

        // what should not have happened
        assert_not_handle_ok(&c);
        assert_not_last_comm_error(&c);
        assert_not_print_cancelled(&rec);
        assert_not_cleared_to_send(&c);
    }

    #[test]
    fn test_other_error_no_m112() {
        // Should trigger escalation
        let (c, rec) = set_up(|c| c._send_m112_on_error = Some(false));
        let serial = Scripted::new(vec![]);
        c._serial.set(Some(serial.clone()));

        let result = c._handle_errors(Some("Error: Printer on fire"));
        assert_eq!(Some("Error: Printer on fire".to_string()), result);

        // what should have happened
        assert_disconnected(&c, &rec);

        // what should not have happened
        assert_not_handle_ok(&c);
        assert_not_last_comm_error(&c);
        assert_not_print_cancelled(&rec);
        assert_not_cleared_to_send(&c);
        // assert_not_m112_sent: nothing was written to the serial object
        assert!(serial.written.lock().unwrap().is_empty());
    }

    #[test]
    fn test_other_error_cancel() {
        // Should trigger print cancel
        let (c, rec) = set_up(|c| {
            c._disconnect_on_errors = Some(false);
            c._printerProfileManager = Some(Arc::new(StaticProfile()));
        });
        let serial = Scripted::new(vec![]);
        c._serial.set(Some(serial.clone()));

        let result = c._handle_errors(Some("Error: Printer on fire"));
        assert_eq!(Some("Error: Printer on fire".to_string()), result);

        // what should have happened
        assert_print_cancelled(&rec);
        assert_cleared_to_send(&c);

        // what should not have happened
        assert_not_handle_ok(&c);
        assert_not_last_comm_error(&c);
        // assert_not_m112_sent: nothing was written to the serial object
        assert!(serial.written.lock().unwrap().is_empty());
        assert_not_disconnected(&c, &rec);
    }

    #[test]
    fn test_other_error_ignored() {
        // Should only log
        let (c, rec) = set_up(|c| {
            c._ignore_errors = Some(true);
            c._printerProfileManager = Some(Arc::new(StaticProfile()));
        });
        let serial = Scripted::new(vec![]);
        c._serial.set(Some(serial.clone()));

        let result = c._handle_errors(Some("Error: Printer on fire"));
        assert_eq!(Some("Error: Printer on fire".to_string()), result);

        // what should have happened
        assert_cleared_to_send(&c);

        // what should not have happened
        assert_not_handle_ok(&c);
        assert_not_last_comm_error(&c);
        assert_not_print_cancelled(&rec);
        // assert_not_m112_sent: nothing was written to the serial object
        assert!(serial.written.lock().unwrap().is_empty());
        assert_not_disconnected(&c, &rec);
    }

    #[test]
    fn test_not_an_error() {
        // Should pass
        let (c, rec) = set_up(|_| {});
        let result = c._handle_errors(Some("Not an error"));
        assert_eq!(Some("Not an error".to_string()), result);
        assert_nop(&c, &rec);
    }

    #[test]
    fn test_already_error() {
        // Should pass
        let (c, rec) = set_up(|_| {});
        c._state.set(MachineCom::STATE_ERROR); // isError.return_value = True

        let result = c._handle_errors(Some("Error: Printer on fire"));
        assert_eq!(Some("Error: Printer on fire".to_string()), result);
        assert_nop(&c, &rec);
    }

    #[test]
    fn test_line_none() {
        // Should pass
        let (c, _) = set_up(|_| {});
        assert_eq!(None, c._handle_errors(None));
    }

    // ---- ours -------------------------------------------------------------------------------

    #[test]
    fn ours_handle_errors_fatal_is_made_an_error_line() {
        // "fatal:" (Repetier): handled as "Error:fatal:..."; the line is returned changed
        let (c, rec) = set_up(|c| {
            c._disconnect_on_errors = Some(false);
            c._send_m112_on_error = Some(false);
        });
        // "fatal:" is in `_fatal_errors`, so it disconnects although `disconnectOnErrors` is off
        let result = c._handle_errors(Some("fatal: out of cheese"));
        assert_eq!(Some("Error:fatal: out of cheese".to_string()), result);
        assert_eq!("fatal: out of cheese", c.get_error_string());
        assert_eq!(
            vec![MachineCom::STATE_ERROR, MachineCom::STATE_CLOSED_WITH_ERROR],
            *rec.states.lock().unwrap()
        );
        let errors = rec.errors.lock().unwrap();
        assert_eq!(
            (
                "fatal: out of cheese".to_string(),
                "firmware".to_string(),
                Some("disconnect".to_string())
            ),
            (
                errors[0].0.clone(),
                errors[0].1.clone(),
                errors[0].2.clone()
            )
        );
    }

    #[test]
    fn ours_handle_errors_joins_split_min_max_and_kill_errors() {
        // regex_minMaxError: "Error:x\n" and the next line is the rest
        let (c, _) = set_up(|c| c._send_m112_on_error = Some(false));
        c._serial.set(Some(Scripted::new(vec![Ok(
            b": Extruder switched off. MAXTEMP triggered !\n",
        )])));
        let result = c._handle_errors(Some("Error:0\n"));
        assert_eq!(
            Some("Error:0: Extruder switched off. MAXTEMP triggered !\n".to_string()),
            result
        );
        assert_eq!(
            "0: Extruder switched off. MAXTEMP triggered !",
            c.get_error_string()
        );

        // regex_marlinKillError: the next line, without its "Error:", is appended after " - "
        let (c, rec) = set_up(|c| c._send_m112_on_error = Some(false));
        c._serial.set(Some(Scripted::new(vec![Ok(
            b"Error:Printer halted. kill() called!\n",
        )])));
        let result = c._handle_errors(Some(
            "Error:Heating failed, system stopped! Heater_ID: bed\n",
        ));
        assert_eq!(
            Some("Error:Heating failed, system stopped! Heater_ID: bed - Printer halted. kill() called!".to_string()),
            result
        );
        let errors = rec.errors.lock().unwrap();
        assert_eq!(
            "Heating failed, system stopped! Heater_ID: bed - Printer halted. kill() called!",
            errors[0].0
        );
        assert_eq!(Some("firmware-heating-failed".to_string()), errors[0].3);
        assert!(errors[0].4.is_some());
    }

    #[test]
    fn ours_payload_for_error() {
        let (c, _) = set_up(|_| {});
        c._log("one");
        c._log("two");
        c._errorValue.set("boom".to_string());
        let p = c._payload_for_error("connection", None, None);
        assert_eq!(
            ErrorPayload {
                error: "boom".to_string(),
                reason: "connection".to_string(),
                consequence: None,
                logs: None,
                faq: None
            },
            p
        );
        assert_eq!(
            serde_json::json!({"error": "boom", "reason": "connection"}),
            p.to_json()
        );
        assert_eq!(
            vec!["error", "reason"],
            p.to_json().as_object().unwrap().keys().collect::<Vec<_>>()
        );

        let p = c._payload_for_error("firmware", Some("cancel"), Some("MINTEMP triggered"));
        assert_eq!(Some("firmware-mintemp".to_string()), p.faq);
        assert_eq!(Some(vec!["one".to_string(), "two".to_string()]), p.logs);
        assert_eq!(Some("cancel".to_string()), p.consequence);
        assert_eq!(
            vec!["error", "reason", "consequence", "logs", "faq"],
            p.to_json().as_object().unwrap().keys().collect::<Vec<_>>()
        );

        for (error, faq) in [
            ("MAXTEMP triggered", Some("firmware-maxtemp")),
            (
                "Thermal Runaway, system stopped",
                Some("firmware-thermal-runaway"),
            ),
            ("Probing failed", Some("firmware-probing-failed")),
            ("BLTouch error", Some("firmware-probing-failed")),
            ("Printer on fire", None),
        ] {
            assert_eq!(
                faq.map(str::to_string),
                c._payload_for_error("firmware", None, Some(error)).faq,
                "{error}"
            );
        }
        // an empty consequence is no consequence
        assert_eq!(
            None,
            c._payload_for_error("firmware", Some(""), Some("x"))
                .consequence
        );
    }

    #[test]
    fn ours_trigger_error_variants() {
        // "connection" and "autodetect" never send M112
        for reason in ["connection", "autodetect"] {
            let (c, rec) = new_comm(|c| c._send_m112_on_error = Some(true));
            c._trigger_error("no luck", reason, true);
            assert_eq!("no luck", c.get_error_string());
            assert_eq!(
                vec![MachineCom::STATE_ERROR, MachineCom::STATE_CLOSED_WITH_ERROR],
                *rec.states.lock().unwrap()
            );
            assert_eq!(
                Some("disconnect".to_string()),
                rec.errors.lock().unwrap()[0].2
            );
        }
        // close=False: no consequence, the connection stays
        let (c, rec) = new_comm(|c| c._send_m112_on_error = Some(true));
        c._trigger_error("minor", "firmware", false);
        assert_eq!(vec![MachineCom::STATE_ERROR], *rec.states.lock().unwrap());
        assert_eq!(None, rec.errors.lock().unwrap()[0].2);
        // the Error event
        super::super::events::wait_idle(); // the events are delivered by the worker thread
        let events = EVENTS.plock();
        assert!(events.iter().any(|(e, p)| e == "Error"
            && p.as_ref().is_some_and(|p| p["error"] == "minor"
                && p["reason"] == "firmware"
                && p.get("consequence").is_none())));
    }

    #[test]
    fn ours_handle_ok() {
        let (c, _) = new_comm(|_| {});
        c._state.set(MachineCom::STATE_OPERATIONAL);
        c._long_running_command.set(true);
        c._toolBeforeHeatup.set(Some(1));
        c._resendActive.set(true);
        c._heating.set(true);
        c._heatupWaitStartTime.set(Some(time_monotonic() - 5.0));
        let timer = Arc::new(Timer::new(100.0, || panic!("never"), true));
        c._resend_ok_timer.set(Some(timer.clone()));

        c._handle_ok();

        assert!(timer.finished.is_set()); // cancelled
        assert!(c._resend_ok_timer.get().is_none());
        assert!(c._ok_timeout.get().unwrap() > time_monotonic());
        assert!(c._clear_to_send.is_set());
        assert!(!c._long_running_command.get());
        assert_eq!(1, c._currentTool.get());
        assert_eq!(None, c._toolBeforeHeatup.get());
        assert!(!c._heating.get());
        assert!(c._heatupWaitTimeLost.get() >= 5.0);
        assert_eq!(None, c._heatupWaitStartTime.get());
        assert!(!c._resendActive.get());

        // not operational: returns after the heatup, `_resendActive` stays
        let (c, _) = new_comm(|_| {});
        c._resendActive.set(true);
        c._handle_ok();
        assert!(c._resendActive.get());
        assert!(c._clear_to_send.is_set());
    }

    #[test]
    fn ours_finish_heatup() {
        let (c, _) = new_comm(|_| {});
        c._finish_heatup(); // not heating: nothing
        assert_eq!(0.0, c._heatupWaitTimeLost.get());
        c._heating.set(true);
        c._heatupWaitStartTime.set(Some(0.0)); // falsy start time: only the flag is cleared
        c._finish_heatup();
        assert!(!c._heating.get());
        assert_eq!(0.0, c._heatupWaitTimeLost.get());
        assert_eq!(Some(0.0), c._heatupWaitStartTime.get());
    }

    #[test]
    fn ours_handle_timeout() {
        // not operational: nothing
        let (c, _) = new_comm(|_| {});
        c._handle_timeout();
        assert_eq!(0, c._consecutive_timeouts.get());

        // idle, nothing clear to send: tickle
        let (c, _) = new_comm(|_| {});
        c._state.set(MachineCom::STATE_OPERATIONAL);
        c._clear_to_send.clear(true);
        c._handle_timeout();
        assert_eq!(1, c._consecutive_timeouts.get());
        assert!(c._clear_to_send.is_set());

        // heating: heatup is over
        let (c, _) = new_comm(|_| {});
        c._state.set(MachineCom::STATE_OPERATIONAL);
        c._heating.set(true);
        c._handle_timeout();
        assert!(!c._heating.get());

        // a long running command: ignored (and 'long' counts)
        let (c, _) = new_comm(|_| {});
        c._state.set(MachineCom::STATE_OPERATIONAL);
        c._long_running_command.set(true);
        c._handle_timeout();
        assert!(c._long_running_command.get());
        assert_eq!(1, c._consecutive_timeouts.get());

        // too many consecutive timeouts (idle maximum 2): give up
        let (c, rec) = new_comm(|_| {});
        c._state.set(MachineCom::STATE_OPERATIONAL);
        c._consecutive_timeouts.set(2);
        c._handle_timeout();
        assert_eq!(
            "Too many consecutive timeouts, printer still connected and alive?",
            c.get_error_string()
        );
        assert_eq!(MachineCom::STATE_CLOSED_WITH_ERROR, c.get_state());
        assert!(rec.logs.lock().unwrap().iter().any(|l| l.starts_with(
            "No response from printer after 3 consecutive communication timeouts, considering it dead. Configure long running commands"
        )));
        super::super::events::wait_idle(); // the events are delivered by the worker thread
        let events = EVENTS.plock();
        assert!(events.iter().any(|(e, p)| e == "Error"
            && p.as_ref().is_some_and(|p| p["reason"] == "timeout"
                && p["error"]
                    == "Too many consecutive timeouts, printer still connected and alive?")));
    }

    #[test]
    fn ours_perform_detection_step() {
        let none = "/dev/maker-connect-test-does-not-exist";
        // init with a port and a baud rate: one candidate; not in DETECT_SERIAL, so no step
        let (c, rec) = new_comm(|_| {});
        c._detection_retry.set(0);
        let mut port_c = MachineCom::new(
            Some(none.to_string()),
            Some(250000),
            Some(rec.clone()),
            None,
        );
        let c2 = Arc::get_mut(&mut port_c).unwrap();
        c2._detection_retry.set(0);
        port_c._perform_detection_step(true);
        assert_eq!(
            vec![(none.to_string(), 250000)],
            port_c._detection_candidates.get()
        );
        assert_eq!(MachineCom::DETECTION_RETRIES, port_c._detection_retry.get());
        assert!(rec.logs.lock().unwrap().contains(&format!(
            "Performing autodetection with 1 port/baudrate candidates: {none}@250000"
        )));

        // a step with a candidate that cannot be opened is skipped, and with no candidate left it is an error
        let (c, rec) = new_comm(|_| {});
        c._state.set(MachineCom::STATE_DETECT_SERIAL);
        c._detection_retry.set(MachineCom::DETECTION_RETRIES);
        c._detection_candidates
            .set(vec![(none.to_string(), 115200)]);
        c._perform_detection_step(false);
        let logs = rec.logs.lock().unwrap();
        assert!(logs.contains(&format!("Trying port {none}, baudrate 115200")));
        assert!(logs.contains(&format!(
            "Could not open port {none}, baudrate 115200, skipping"
        )));
        assert_eq!(
            "No more candidates to test, and no working port/baudrate combination detected.",
            c.get_error_string()
        );
        assert_eq!(MachineCom::STATE_CLOSED_WITH_ERROR, c.get_state());
        assert_eq!(
            Some("disconnect".to_string()),
            rec.errors.lock().unwrap()[0].2
        );
        assert_eq!("autodetect", rec.errors.lock().unwrap()[0].1);

        // not in DETECT_SERIAL: the loop returns at once
        let (c, rec) = new_comm(|_| {});
        c._detection_candidates
            .set(vec![(none.to_string(), 115200)]);
        c._perform_detection_step(false);
        assert_eq!(1, c._detection_candidates.get().len());
        assert!(rec.states.lock().unwrap().is_empty());

        // a detection retry with no serial object ends the retries (then the candidates are tried)
        let (c, _) = new_comm(|_| {});
        c._state.set(MachineCom::STATE_DETECT_SERIAL);
        c._detection_retry.set(0);
        c._detection_candidates.set(vec![]);
        c._perform_detection_step(false);
        assert_eq!(MachineCom::DETECTION_RETRIES, c._detection_retry.get());
        assert_eq!(MachineCom::STATE_CLOSED_WITH_ERROR, c.get_state());
    }

    #[test]
    fn ours_continue_sending_and_the_queues() {
        let (c, _) = new_comm(|_| {});
        // nothing to send, not printing: `Some(false)`
        assert_eq!(Some(false), c._continue_sending());
        assert!(!c._send_from_command_queue());
        assert!(!c._send_from_job_queue());
        // not active: the loop is not entered
        c._monitoring_active.set(false);
        assert_eq!(None, c._continue_sending());
        // streaming: the command queue is irrelevant
        let (c, _) = new_comm(|_| {});
        let file = StreamingGcodeFileInformation::new(
            &temp_gcode(b"G1 X1\n"),
            "local.gcode",
            "/remote.gco",
            None,
        )
        .unwrap();
        c._currentFile.set(Some(Arc::new(file)));
        c._command_queue
            .put(
                (
                    QueuedCommand::Command("G28".to_string()),
                    None,
                    None,
                    BTreeSet::new(),
                ),
                None,
                true,
                None,
            )
            .unwrap_or_else(|_| panic!("put"));
        assert!(!c._send_from_command_queue());
        assert_eq!(1, c._command_queue.qsize());
        // a job on hold sends nothing
        let (c, _) = new_comm(|_| {});
        c.set_job_on_hold(true, true);
        assert_eq!(Some(false), c._continue_sending());
    }

    fn temp_gcode(content: &[u8]) -> String {
        use std::sync::atomic::AtomicUsize;
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "maker-connect-comm-h-test-{}-{}.gcode",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::write(&path, content).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn queued(c: &MachineCom) -> Vec<(String, Option<String>, BTreeSet<String>)> {
        let mut out = Vec::new();
        while let Ok((cmd, cmd_type, _, tags)) = c._command_queue.get(false, None) {
            let QueuedCommand::Command(cmd) = cmd else {
                panic!("a marker")
            };
            out.push((cmd, cmd_type, tags));
            c._command_queue.task_done().unwrap();
        }
        out
    }

    fn tagset(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ours_polls() {
        // printing: the commands land in the command queue
        let (c, _) = new_comm(|_| {});
        c._state.set(MachineCom::STATE_PRINTING);
        c._poll_temperature();
        assert_eq!(
            vec![(
                "M105".to_string(),
                Some("temperature_poll".to_string()),
                tagset(&["trigger:comm.poll_temperature"])
            )],
            queued(&c)
        );
        // each condition of the poll stops it
        for stop in [
            Box::new(|c: &MachineCom| c._temperature_autoreporting.set(true))
                as Box<dyn Fn(&MachineCom)>,
            Box::new(|c| c._connection_closing.set(true)),
            Box::new(|c| c._long_running_command.set(true)),
            Box::new(|c| c._heating.set(true)),
            Box::new(|c| c._dwelling_until.set(Some(time_monotonic() + 100.0))),
            Box::new(|c| c._manualStreaming.set(true)),
            Box::new(|c| c._state.set(MachineCom::STATE_NONE)),
        ] {
            let (c, _) = new_comm(|_| {});
            c._state.set(MachineCom::STATE_PRINTING);
            stop(&c);
            c._poll_temperature();
            assert!(queued(&c).is_empty());
        }

        // sd status: needs an SD file selected (or sd printing)
        let (c, _) = new_comm(|_| {});
        c._state.set(MachineCom::STATE_PRINTING);
        c._poll_sd_status();
        assert!(queued(&c).is_empty());
        c._currentFile
            .set(Some(Arc::new(PrintingSdFileInformation::new(
                "x.gco",
                Some(84),
                None,
            ))));
        // (with an SD file selected the poll would be sent with `_sendCommand`, comm.py:4509: not
        // reachable yet; operational and idle, `isSdPrinting` is false)
        c._state.set(MachineCom::STATE_OPERATIONAL);
        c._disable_sd_printing_detection.set(Some(true));
        c._poll_sd_status();
        assert!(queued(&c).is_empty());
        c._sdstatus_autoreporting.set(true);
        c._disable_sd_printing_detection.set(None);
        c._poll_sd_status();
        assert!(queued(&c).is_empty());
    }

    #[test]
    fn ours_set_autoreport_intervals() {
        let (c, _) = new_comm(|_| {});
        c._state.set(MachineCom::STATE_PRINTING);
        c._set_autoreport_temperature_interval(None);
        c._set_autoreport_temperature_interval(Some(7));
        c._set_autoreport_sdstatus_interval(None);
        c._set_autoreport_sdstatus_interval(Some(0));
        c._set_autoreport_pos_interval(None, false, Some(tagset(&["a"])));
        c._set_autoreport_pos_interval(Some(0), true, None);
        c._set_busy_protocol_interval(None, None);
        c._set_busy_protocol_interval(Some(4), None);
        // temperatureAutoreport 2, sdStatusAutoreport 1, posAutoreport 5, communicationBusy 3 - 1
        let got = queued(&c);
        let texts: Vec<&str> = got.iter().map(|g| g.0.as_str()).collect();
        // `part_of_job=True` goes to the job queue
        assert_eq!(
            vec![
                "M155 S2", "M155 S7", "M27 S1", "M27 S0", "M154 S5", "M113 S2", "M113 S4"
            ],
            texts
        );
        assert_eq!(
            tagset(&["a", "trigger:comm.set_autoreport_pos_interval"]),
            got[4].2
        );
        let (line, _, _, tags) = c._job_queue.get_nowait().unwrap();
        assert!(matches!(line, QueuedCommand::Command(l) if l == "M154 S0"));
        assert_eq!(
            tagset(&["source:job", "trigger:comm.set_autoreport_pos_interval"]),
            tags
        );

        // an interval that is no int falls back to the default
        let (c, _) = new_comm(|_| {});
        c._state.set(MachineCom::STATE_PRINTING);
        c._timeout_intervals.with_mut(|d| {
            d.insert("temperatureAutoreport", f64::NAN);
            d.insert("sdStatusAutoreport", f64::INFINITY);
            d.insert("posAutoreport", 9.9);
            d.insert("communicationBusy", f64::NAN);
        });
        c._set_autoreport_temperature_interval(None);
        c._set_autoreport_sdstatus_interval(None);
        c._set_autoreport_pos_interval(None, false, None);
        c._set_busy_protocol_interval(None, None);
        let texts: Vec<String> = queued(&c).into_iter().map(|g| g.0).collect();
        assert_eq!(vec!["M155 S2", "M27 S1", "M154 S9", "M113 S2"], texts);
    }

    #[test]
    fn ours_intervals() {
        let (c, _) = new_comm(|_| {});
        // sd status
        assert_eq!(1.0, c._get_sd_status_timer_interval());
        c._timeout_intervals
            .with_mut(|d| d.insert("sdStatus", -3.0));
        assert_eq!(1.0, c._get_sd_status_timer_interval());
        c._timeout_intervals.with_mut(|d| d.insert("sdStatus", 2.5));
        assert_eq!(2.5, c._get_sd_status_timer_interval());

        // temperature: temperature 5 (default settings), temperatureTargetSet 2
        let (c, _) = new_comm(|_| {});
        assert_eq!(5.0, c._get_temperature_timer_interval());
        c.last_temperature
            .with_mut(|t| t.set_tool(0, Some(20.0), Some(0.0)));
        assert_eq!(5.0, c._get_temperature_timer_interval());
        c.last_temperature
            .with_mut(|t| t.set_tool(0, Some(20.0), Some(200.0)));
        assert_eq!(2.0, c._get_temperature_timer_interval());
        c.last_temperature
            .with_mut(|t| t.set_tool(0, Some(20.0), Some(0.0)));
        c.last_temperature
            .with_mut(|t| t.set_bed(Some(20.0), Some(60.0)));
        assert_eq!(2.0, c._get_temperature_timer_interval());
        // busy: always `temperature`
        c._state.set(MachineCom::STATE_PRINTING);
        assert_eq!(5.0, c._get_temperature_timer_interval());
        // an interval <= 0 is 1.0
        c._timeout_intervals
            .with_mut(|d| d.insert("temperature", 0.0));
        assert_eq!(1.0, c._get_temperature_timer_interval());

        // communication timeout: max(communication 30, temperature + 1)
        let (c, _) = new_comm(|_| {});
        assert_eq!(30.0, c._get_communication_timeout_interval());
        c._timeout_intervals
            .with_mut(|d| d.insert("temperature", 40.0));
        assert_eq!(41.0, c._get_communication_timeout_interval());
        c._temperature_autoreporting.set(true); // temperatureAutoreport 2: 3
        assert_eq!(30.0, c._get_communication_timeout_interval());
        c._busy_protocol_support.set(true); // communicationBusy 3.0 (default settings)
        assert_eq!(3.0, c._get_communication_timeout_interval());
        c._state.set(MachineCom::STATE_DETECT_SERIAL);
        c._detection_retry.set(0);
        assert_eq!(10.0, c._get_communication_timeout_interval());
        c._detection_retry.set(1);
        assert_eq!(2.0, c._get_communication_timeout_interval());

        let before = time_monotonic();
        let t = c._get_new_communication_timeout();
        assert!(t >= before + 2.0 && t < before + 3.0);
    }

    #[test]
    fn ours_get_next_from_job_and_send_from_job() {
        // no file
        let (c, _) = new_comm(|_| {});
        let (line, pos, lineno) = c._get_next_from_job();
        assert!(line.is_none() && pos.is_none() && lineno.is_none());

        // a file: the line, its position and line number; then the end: M400, the job is done,
        // the marker finishes the print
        let (c, rec) = new_comm(|_| {});
        c._currentFile.set(Some(Arc::new(
            PrintingGcodeFileInformation::new(&temp_gcode(b"G1 X1\n"), None, None, None).unwrap(),
        )));
        c._currentFile.get().unwrap().start().unwrap();
        c._state.set(MachineCom::STATE_PRINTING);
        let (line, pos, lineno) = c._get_next_from_job();
        assert!(matches!(line, Some(QueuedCommand::Command(l)) if l == "G1 X1"));
        assert_eq!((Some(6), Some(1)), (pos, lineno));
        let (line, pos, lineno) = c._get_next_from_job();
        assert!(matches!(line, Some(QueuedCommand::Marker(_))));
        assert_eq!((None, None), (pos, lineno));
        assert_eq!(MachineCom::STATE_FINISHING, c.get_state());
        assert_eq!(1, *rec.job_done.lock().unwrap());
        let (m400, _, _, tags) = c._job_queue.get_nowait().unwrap();
        assert!(matches!(m400, QueuedCommand::Command(l) if l == "M400"));
        assert_eq!(tagset(&["source:job"]), tags);
        // the marker's callback: finalize
        let QueuedCommand::Marker(marker) = line.unwrap() else {
            unreachable!()
        };
        marker.run();
        assert_eq!(MachineCom::STATE_OPERATIONAL, c.get_state());

        // `_send_from_job`: not printing, on hold
        let (c, _) = new_comm(|_| {});
        assert!(!c._send_from_job());
        c._state.set(MachineCom::STATE_PRINTING);
        c.set_job_on_hold(true, true);
        assert!(!c._send_from_job());
        // at the end of the file: the marker is queued for the job and the loop returns false
        let (c, rec) = new_comm(|_| {});
        c._currentFile.set(Some(Arc::new(
            PrintingGcodeFileInformation::new(&temp_gcode(b""), None, None, None).unwrap(),
        )));
        c._currentFile.get().unwrap().start().unwrap();
        c._state.set(MachineCom::STATE_PRINTING);
        assert!(!c._send_from_job());
        assert_eq!(1, *rec.progress.lock().unwrap());
        let (a, _, _, _) = c._job_queue.get_nowait().unwrap();
        assert!(matches!(a, QueuedCommand::Command(l) if l == "M400"));
        let (b, _, _, _) = c._job_queue.get_nowait().unwrap();
        assert!(matches!(b, QueuedCommand::Marker(_)));
    }

    #[test]
    fn ours_open_serial_of_a_missing_port() {
        let (c, rec) = new_comm(|_| {});
        let none = "/dev/maker-connect-test-does-not-exist";
        assert!(!c._open_serial(none, 115200, true));
        assert!(c._serial.get().is_none());
        assert!(
            rec.logs
                .lock()
                .unwrap()
                .contains(&format!("Connecting to port {none}, baudrate 115200"))
        );
        // no error was triggered: the default factory returned None
        assert!(rec.errors.lock().unwrap().is_empty());
        assert_eq!(MachineCom::STATE_NONE, c.get_state());
    }

    #[test]
    fn ours_readline() {
        let (c, rec) = new_comm(|_| {});
        // no serial object
        assert_eq!(None, c._readline());

        // a line, a timeout, latin1 after invalid utf-8, a null byte
        c._serial.set(Some(Scripted::new(vec![
            Ok(b"ok T:20.0\n"),
            Ok(b""),
            Ok(b"caf\xe9\n"),
            Ok(b"a\x00b\n"),
        ])));
        assert_eq!(Some("ok T:20.0\n".to_string()), c._readline());
        assert_eq!(Some("".to_string()), c._readline());
        assert_eq!(Some("caf\u{e9}\n".to_string()), c._readline());
        assert_eq!(Some("a\0b\n".to_string()), c._readline());
        let logs = rec.logs.lock().unwrap();
        assert!(logs.contains(&"Recv: ok T:20.0".to_string()));
        assert!(logs.contains(&"Recv: caf\u{e9}".to_string()));
        assert!(logs.iter().any(|l| l == "WARNThe received line contains at least one null byte character at position 1, this hints at some data corruption going on"));
        drop(logs);

        // an exception: logged, the error value is "type: message", the connection is closed
        let (c, rec) = new_comm(|_| {});
        c._serial.set(Some(Scripted::new(vec![Err(
            SerialError::serial_exception("device reports readiness"),
        )])));
        assert_eq!(None, c._readline());
        assert_eq!(
            "SerialException: device reports readiness",
            c.get_error_string()
        );
        assert_eq!(MachineCom::STATE_CLOSED_WITH_ERROR, c.get_state());
        let logs = rec.logs.lock().unwrap();
        assert!(logs.iter().any(|l| l.starts_with(
            "Unexpected error while reading serial port, please consult octoprint.log for details: SerialException: 'device reports readiness' @ comm.rs:_readline:"
        )));
        assert!(
            logs.contains(
                &"Please see https://faq.octoprint.org/serialerror for possible reasons of this."
                    .to_string()
            )
        );
        drop(logs);

        // an exception that is no SerialException: no hint; while closing: nothing at all
        let (c, rec) = new_comm(|_| {});
        c._serial.set(Some(Scripted::new(vec![
            Err(SerialError::other("OSError", "x")),
            Err(SerialError::other("OSError", "y")),
        ])));
        assert_eq!(None, c._readline());
        assert!(
            !rec.logs
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.starts_with("Please see"))
        );
        let (c, rec) = new_comm(|_| {});
        c._connection_closing.set(true);
        c._serial
            .set(Some(Scripted::new(vec![Err(SerialError::other(
                "OSError", "y",
            ))])));
        assert_eq!(None, c._readline());
        assert!(rec.logs.lock().unwrap().is_empty());
        assert_eq!("", c.get_error_string());
    }

    fn resend_comm(configure: impl FnOnce(&mut MachineCom)) -> (Arc<MachineCom>, Arc<Rec>) {
        let (c, rec) = new_comm(|c| {
            c._send_m112_on_error = Some(false);
            // the timer of `detect` would run `_resendSimulateOk` after its interval
            c._trigger_ok_after_resend = Some(Value::Null);
            configure(c);
        });
        (c, rec)
    }

    #[test]
    fn ours_handle_resend_request() {
        // not a resend line
        let (c, _) = resend_comm(|_| {});
        assert!(!c._handle_resend_request("ok"));
        assert_eq!(1, c.received_resends());

        // line 1 while at line 1 (a left over of a reset), and a line we have not sent yet
        let (c, _) = resend_comm(|_| {});
        c._current_line.set(1);
        assert!(!c._handle_resend_request("Resend: 1"));
        let (c, _) = resend_comm(|_| {});
        c._current_line.set(5);
        assert!(!c._handle_resend_request("Resend: 5"));
        assert!(!c._resendActive.get());

        // a regular request
        let (c, _) = resend_comm(|_| {});
        c._current_line.set(10);
        for i in 5..10 {
            c._add_to_last_lines(format!("N{i} G1*1").into_bytes());
        }
        c._lastCommError.set(Some("checksum".to_string()));
        assert!(c._handle_resend_request("Resend: 8"));
        assert!(c._resendActive.get());
        assert_eq!(Some(2), c._resendDelta.get());
        assert_eq!(Some(8), c._lastResendNumber.get());
        assert_eq!(0, c._currentResendCount.get());
        assert_eq!(None, c._lastCommError.get());
        assert!(c._send_queue.resend_active());
        assert_eq!(Some(8), c._currentConsecutiveResendNumber.get());
        assert_eq!(1, c._log_resends_rate_count.get());
        assert!(c._log_resends_rate_start.get().is_some());

        // the same line again after a "line number" error, while the first request is handled:
        // ignored but true
        c._lastCommError
            .set(Some("Line Number is not Last Line Number+1".to_string()));
        assert!(c._handle_resend_request("Resend: 8"));
        assert_eq!(1, c._currentResendCount.get());

        // the printer keeps asking for the same line
        let (c, rec) = resend_comm(|_| {});
        c._current_line.set(10);
        for i in 5..10 {
            c._add_to_last_lines(format!("N{i} G1*1").into_bytes());
        }
        c._currentConsecutiveResendNumber.set(Some(8));
        c._currentConsecutiveResendCount.set(9);
        c._handle_resend_request("Resend: 8");
        assert_eq!(
            "Printer keeps requesting line 8 again and again, communication stuck",
            c.get_error_string()
        );
        assert_eq!(MachineCom::STATE_CLOSED_WITH_ERROR, c.get_state());
        assert_eq!("resend_loop", rec.errors.lock().unwrap()[0].1);

        // no history: not printing, the delta is reset; printing, it is an error
        let (c, _) = resend_comm(|_| {});
        c._current_line.set(10);
        assert!(c._handle_resend_request("Resend: 8"));
        assert_eq!(None, c._resendDelta.get());
        let (c, rec) = resend_comm(|_| {});
        c._current_line.set(10);
        c._state.set(MachineCom::STATE_PRINTING);
        c._handle_resend_request("Resend: 8");
        assert_eq!(
            "Should resend line 8 but no sufficient history is available, can't resend",
            c.get_error_string()
        );
        assert_eq!("resend", rec.errors.lock().unwrap()[0].1);
    }

    #[test]
    fn ours_handle_resend_request_timer_and_ok() {
        // supportResendsWithoutOk "always": an ok at once
        let (c, _) =
            resend_comm(|c| c._trigger_ok_after_resend = Some(Value::String("always".to_string())));
        assert!(!c._handle_resend_request("ok"));
        assert!(c._ok_timeout.get().is_some());

        // "detect": a timer that simulates the ok; cancelled here so that it never runs
        let (c, _) =
            resend_comm(|c| c._trigger_ok_after_resend = Some(Value::String("detect".to_string())));
        assert!(!c._handle_resend_request("ok"));
        let timer = c._resend_ok_timer.get().expect("the timer");
        timer.cancel();
        timer.join(None).unwrap();
        assert!(c._ok_timeout.get().is_none());

        // `_resendSimulateOk` clears the timer and handles the ok
        c._resend_simulate_ok();
        assert!(c._resend_ok_timer.get().is_none());
        assert!(c._ok_timeout.get().is_some());
    }

    #[test]
    fn ours_resend_next_command_and_possibility() {
        // no resend delta: nothing to do
        let (c, _) = resend_comm(|_| {});
        c._lastCommError.set(Some("x".to_string()));
        assert!(!c._resend_next_command(false));
        assert_eq!(None, c._lastCommError.get());

        // a delta that the history cannot satisfy: not possible, the delta is reset
        c._resendDelta.set(Some(3));
        assert!(!c._resend_next_command(false));
        assert_eq!(None, c._resendDelta.get());

        // `_resend_check_possibility`
        let (c, rec) = resend_comm(|_| {});
        c._add_to_last_lines(b"N1 G1*1".to_vec());
        c._resendDelta.set(Some(1));
        assert!(c._resend_check_possibility(1));
        c._resendDelta.set(Some(2));
        assert!(!c._resend_check_possibility(1));
        assert!(
            rec.logs.lock().unwrap().contains(
                &"Should resend line 1 but no sufficient history is available, can't resend"
                    .to_string()
            )
        );
        assert_eq!(None, c._resendDelta.get());
        c._resendDelta.set(Some(-1));
        assert!(!c._resend_check_possibility(1));
    }

    #[test]
    fn ours_on_resend_failure_a_timeout_during_active_resend_needs_the_stub() {
        // `_resend_same_command` runs `_resend_next_command(again=True)`, which ends in
        // `_enqueue_for_sending` (comm.py:4602): not reachable yet; what comes before it is tested here
        let (c, _) = resend_comm(|_| {});
        // again with no history: delta 0 -> 1, check fails (empty history), delta reset
        assert!(!c._resend_same_command());
        assert_eq!(None, c._resendDelta.get());
    }

    #[test]
    fn ours_process_registered_message() {
        install_sink();
        let controls = vec![
            FeedbackControl {
                key: Some("k1".to_string()),
                regex: Some(PyText::Str("Temp: (\\d+)/(?P<target>\\d+)".to_string())),
                template: Some(PyText::Str("now {0} of {1}".to_string())),
                template_key: Some("t1".to_string()),
                children: None,
            },
            FeedbackControl {
                key: Some("k1".to_string()),
                regex: Some(PyText::Str("Temp: (\\d+)/(?P<target>\\d+)".to_string())),
                template: Some(PyText::Str("goal {target}".to_string())),
                template_key: Some("t2".to_string()),
                children: None,
            },
            FeedbackControl {
                key: Some("k1".to_string()),
                regex: Some(PyText::Str("Temp: (\\d+)/(?P<target>\\d+)".to_string())),
                template: Some(PyText::Str("{9}".to_string())),
                template_key: Some("t3".to_string()),
                children: None,
            },
        ];
        let (feedback_controls, matcher) = convert_feedback_controls(&controls).unwrap();
        let matcher = matcher.unwrap();
        let (c, _) = new_comm(|_| {});
        let mut errors: Vec<String> = Vec::new();

        c._process_registered_message("no match here", &matcher, &feedback_controls, &mut errors);
        c._process_registered_message("Temp: 21/200", &matcher, &feedback_controls, &mut errors);
        assert!(errors.is_empty());
        super::super::events::wait_idle(); // the events are delivered by the worker thread
        let events = EVENTS.plock();
        let ev = events
            .iter()
            .rev()
            .find(|(e, p)| {
                e == "RegisteredMessageReceived" && p.as_ref().is_some_and(|p| p["key"] == "k1")
            })
            .expect("the event");
        let p = ev.1.as_ref().unwrap();
        assert_eq!("Temp: 21/200", p["matched"]);
        // `{0} {1}` positional, `{target}` is a KeyError and retried with the names, `{9}` is an
        // IndexError: no output for that key
        assert_eq!(
            serde_json::json!({"t1": "now 21 of 200", "t2": "goal 200"}),
            p["outputs"]
        );
        assert_eq!(
            vec!["t1", "t2"],
            p["outputs"].as_object().unwrap().keys().collect::<Vec<_>>()
        );
        drop(events);

        // a key that is in feedback_errors is skipped
        // (EVENTS is shared with the tests running in parallel: count only the events of this test's key)
        let count_k1 = || {
            super::super::events::wait_idle(); // the events are delivered by the worker thread
            EVENTS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter(|(e, p)| {
                    e == "RegisteredMessageReceived" && p.as_ref().is_some_and(|p| p["key"] == "k1")
                })
                .count()
        };
        let before = count_k1();
        let mut errors = vec!["k1".to_string()];
        c._process_registered_message("Temp: 21/200", &matcher, &feedback_controls, &mut errors);
        assert_eq!(before, count_k1());
    }

    #[test]
    fn ours_serial_list_and_baudrate_list() {
        let dir =
            std::env::temp_dir().join(format!("maker-connect-serial-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in [
            "ttyUSB0",
            "ttyACM1",
            "cu.usbserial-1",
            "tty.usbmodem2",
            "ttyS0",
            "rfcomm0",
            "null",
            "tty",
            "zero",
        ] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let d = dir.to_str().unwrap();
        // the "/dev" scan itself, on every platform; `serial_list_in` takes it only off Windows
        let mut got = scan_dev(d);
        got.sort();
        let mut expected: Vec<String> = [
            "ttyUSB0",
            "ttyACM1",
            "cu.usbserial-1",
            "tty.usbmodem2",
            "ttyS0",
            "rfcomm0",
        ]
        .iter()
        .map(|f| dir.join(f).to_string_lossy().into_owned())
        .collect();
        expected.sort();
        assert_eq!(expected, got);
        #[cfg(not(windows))]
        {
            let mut listed = serial_list_in(d);
            listed.sort();
            assert_eq!(expected, listed);
        }
        assert!(serial_list_in("/maker-connect-test-does-not-exist").is_empty());
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(
            vec![115200, 250000, 230400, 57600, 38400, 19200, 9600],
            baudrate_list(None)
        );
        assert_eq!(
            vec![115200, 250000],
            baudrate_list(Some(vec![115200, 250000]))
        );
    }

    /// comm.py:198-208 run by CPython itself (`winreg`, the code of `serialList`'s `nt` branch) and
    /// ours on the same machine's registry: the same port names in the same order.
    #[cfg(windows)]
    #[test]
    fn winreg_serialcomm_is_pythons() {
        let script = "import winreg\ncandidates = []\ntry:\n    key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, 'HARDWARE\\\\DEVICEMAP\\\\SERIALCOMM')\n    i = 0\n    while True:\n        candidates += [winreg.EnumValue(key, i)[1]]\n        i += 1\nexcept Exception:\n    pass\nprint('\\n'.join(candidates))\n";
        let out = std::process::Command::new("python")
            .args(["-I", "-c", script])
            .output()
            .expect("python (CPython with winreg) on the Windows test machine");
        assert!(out.status.success(), "{out:?}");
        let python: Vec<String> = String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(|l| l.trim_end_matches('\r').to_string())
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(python, winreg_serialcomm());
    }

    #[test]
    fn ours_get_exception_string_of() {
        let info = ExceptionInfo {
            type_name: "ValueError",
            message: "bad {file}",
            file: "comm.rs",
            function: "f",
            line: 12,
        };
        assert_eq!(
            "ValueError: 'bad {file}' @ comm.rs:f:12",
            get_exception_string_of(&info, EXCEPTION_STRING_FMT)
        );
        assert_eq!(
            "ValueError: bad {file}",
            get_exception_string_of(&info, "{type}: {message}")
        );
        assert_eq!(
            "Exception: 'plain' @ comm.rs:g:1",
            exception_string_of_panic("plain", EXCEPTION_STRING_FMT, "g", 1)
        );
        assert_eq!(
            "TypeError: 'x: y' @ comm.rs:g:1",
            exception_string_of_panic("TypeError: x: y", EXCEPTION_STRING_FMT, "g", 1)
        );
    }

    #[test]
    fn ours_decode_serial_bytes() {
        let enc = |s: &str| Some(Value::String(s.to_string()));
        assert_eq!("N3 G1*1", decode_serial_bytes(b"N3 G1*1", &enc("ascii")));
        assert_eq!("\u{e9}", decode_serial_bytes(b"\xe9", &enc("latin-1")));
        assert_eq!(
            "\u{e9}",
            decode_serial_bytes("\u{e9}".as_bytes(), &enc("UTF-8"))
        );
        assert!(
            std::panic::catch_unwind(|| decode_serial_bytes(
                b"\xe9",
                &Some(Value::String("ascii".to_string()))
            ))
            .is_err()
        );
    }

    // ---- the three fixes of "Section G, decided" -------------------------------------------------

    #[test]
    fn ours_non_list_controls_and_triggers_panic() {
        // falsy values: `convert_feedback_controls` / `convert_pause_triggers` return early
        for v in [
            None,
            Some(Value::Null),
            Some(serde_json::json!([])),
            Some(serde_json::json!({})),
            Some(serde_json::json!(0)),
            Some(serde_json::json!("")),
        ] {
            assert!(feedback_controls_from_value(&v).is_empty());
            assert!(pause_triggers_from_value(&v).is_empty());
        }
        // anything else that is no list is a panic, not an empty list
        for v in [
            serde_json::json!({"a": 1}),
            serde_json::json!("text"),
            serde_json::json!(3),
            serde_json::json!(true),
        ] {
            assert!(
                std::panic::catch_unwind(|| feedback_controls_from_value(&Some(v.clone())))
                    .is_err(),
                "{v}"
            );
            assert!(
                std::panic::catch_unwind(|| pause_triggers_from_value(&Some(v.clone()))).is_err(),
                "{v}"
            );
        }
    }

    #[test]
    fn ours_parse_firmware_line_keeps_the_order_of_the_line() {
        let d = parse_firmware_line("NAME: Malyan VER: 2.9 MODEL: M200 HW: HA02");
        assert_eq!(
            vec!["NAME", "VER", "MODEL", "HW"],
            d.keys().map(String::as_str).collect::<Vec<_>>()
        );
        let d = parse_firmware_line("ZZZ:1 AAA:2 FIRMWARE_NAME:Marlin MMM:3");
        assert_eq!(
            vec!["ZZZ", "AAA", "FIRMWARE_NAME", "MMM"],
            d.keys().map(String::as_str).collect::<Vec<_>>()
        );
        // a repeated key keeps its first place and takes the last value
        let d = parse_firmware_line("B:1 A:2 B:3");
        assert_eq!(
            vec!["B", "A"],
            d.keys().map(String::as_str).collect::<Vec<_>>()
        );
        assert_eq!(Some(&"3".to_string()), d.get("B"));
        // the FIRMWARE_DATA payload follows it
        assert_eq!(
            vec!["B", "A"],
            py_dict_to_json(&d)
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn ours_py_try_returns_the_panic_text() {
        assert_eq!(
            Err("TypeError: boom".to_string()),
            py_try(|| -> i32 { panic!("TypeError: boom") })
        );
        assert_eq!(
            Err("with 7".to_string()),
            py_try(|| -> i32 { panic!("with {}", 7) })
        );
        assert_eq!(
            Err(String::new()),
            py_try(|| -> i32 { std::panic::panic_any(5i32) })
        );
    }
}

#[cfg(test)]
mod section_i_tests {
    //! Section I (comm.py:4509-5688). OctoPrint's tests (`tests/util/test_comm.py`,
    //! `test_comm_helpers.py`) cover none of it but through `TestCommErrorHandling`, whose
    //! `test_other_error_disconnect` (it needs `_trigger_emergency_stop`) is un-ignored in
    //! `section_h_tests`. Every test here is OURS (`ours_*`). None opens a serial port or starts
    //! `start()`: the serial object is a recording fake.
    use super::*;
    use std::sync::Mutex;

    /// a recording serial object: every `write` is kept; the results are scripted, then "all written"
    struct Wire {
        written: Mutex<Vec<u8>>,
        calls: Mutex<Vec<Vec<u8>>>,
        results: Mutex<std::collections::VecDeque<Result<Option<i64>, SerialError>>>,
    }

    impl Wire {
        fn new(results: Vec<Result<Option<i64>, SerialError>>) -> Arc<Wire> {
            Arc::new(Wire {
                written: Mutex::new(Vec::new()),
                calls: Mutex::new(Vec::new()),
                results: Mutex::new(results.into()),
            })
        }
        fn text(&self) -> String {
            String::from_utf8(self.written.lock().unwrap().clone()).unwrap()
        }
    }

    impl SerialTransport for Wire {
        fn port(&self) -> Option<Option<String>> {
            Some(Some("/dev/ttyFAKE".to_string()))
        }
        fn close(&self) -> Result<(), String> {
            Ok(())
        }
        fn str(&self) -> String {
            "Wire".to_string()
        }
        fn set_timeout(&self, _timeout: f64) {}
        fn timeout(&self) -> Option<f64> {
            None
        }
        fn set_baudrate(&self, _baudrate: i64) {}
        fn readline(&self) -> Result<Vec<u8>, SerialError> {
            Ok(Vec::new())
        }
        fn write(&self, data: &[u8]) -> Result<Option<i64>, SerialError> {
            self.calls.lock().unwrap().push(data.to_vec());
            let result = self
                .results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(Some(data.len() as i64)));
            if let Ok(Some(n)) = &result {
                let n = (*n).clamp(0, data.len() as i64) as usize;
                self.written.lock().unwrap().extend_from_slice(&data[..n]);
            }
            if let Ok(None) = &result {
                self.written.lock().unwrap().extend_from_slice(data);
            }
            result
        }
    }

    #[allow(non_snake_case)]
    fn Profile(bed: bool, extruders: i64) -> PrinterProfileManager {
        PrinterProfileManager::with_current(
            serde_json::json!({"extruder": {"count": extruders}, "heatedBed": bed, "heatedChamber": false}),
        )
    }

    /// an operational comm that has its firmware info (no checksums unless asked) on a `Wire`
    fn comm_on(wire: &Arc<Wire>, configure: impl FnOnce(&mut MachineCom)) -> Arc<MachineCom> {
        let mut c = MachineCom::new(
            None,
            None,
            Some(Arc::new(DefaultMachineComPrintCallback)),
            Some(Arc::new(Profile(true, 1))),
        );
        configure(Arc::get_mut(&mut c).expect("nothing else holds the comm yet"));
        c._serial.set(Some(wire.clone()));
        c._state.set(MachineCom::STATE_OPERATIONAL);
        c._firmware_info_received.set(true);
        c
    }

    fn cmd(s: &str) -> QueuedCommand {
        QueuedCommand::Command(s.to_string())
    }

    fn send(c: &Arc<MachineCom>, s: &str) -> Option<bool> {
        c.send_command(cmd(s), None, false, false, false, None, None)
    }

    fn enable_emergency_parser(c: &MachineCom) {
        c._capability_support
            .with_mut(|d| d.insert(MachineCom::CAPABILITY_EMERGENCY_PARSER, Some(true)));
        c._firmware_capabilities
            .with_mut(|d| d.insert(MachineCom::CAPABILITY_EMERGENCY_PARSER, true));
    }

    fn queued_commands(c: &MachineCom) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(entry) = c._send_queue.get(false, None) {
            c._send_queue.task_done().unwrap();
            match entry {
                SendQueueEntry::Line {
                    command: QueuedCommand::Command(s),
                    ..
                } => out.push(s),
                _ => out.push("<marker>".to_string()),
            }
        }
        out
    }

    #[test]
    fn ours_checksum_line_is_n_number_text_star_xor() {
        let wire = Wire::new(vec![]);
        let c = comm_on(&wire, |_| {});
        // N0 M110 N0*125 is the line OctoPrint sends to reset the line number
        c._do_send_with_checksum(b"M110 N0", 0);
        assert_eq!("N0 M110 N0*125\n", wire.text());
    }

    #[test]
    fn ours_emergency_commands_jump_the_queue_and_use_up_an_ok_with_the_emergency_parser() {
        for command in ["M108", "M410"] {
            let wire = Wire::new(vec![]);
            let c = comm_on(&wire, |_| {});
            enable_emergency_parser(&c);
            c._clear_to_send.set();
            assert!(c._enqueue_for_sending(cmd("G1 X1"), None, None, None, false, None));

            // `sendCommand` forces an emergency command (comm.py:1297); `_gcode_..._queuing` does not
            // exist for it, `_command_phase_queuing` sends it at once (comm.py:5617, 5565)
            assert_eq!(
                Some(false),
                send(&c, command),
                "{command}: not enqueued, sent directly"
            );
            assert_eq!(
                format!("{command}\n"),
                wire.text(),
                "{command} written ahead of the queue"
            );
            assert_eq!(0, c._clear_to_send.counter(), "{command} uses up an ok");
            assert_eq!(
                vec!["G1 X1".to_string()],
                queued_commands(&c),
                "{command} is not in the send queue"
            );
        }
    }

    #[test]
    fn ours_emergency_commands_are_queued_in_order_without_the_emergency_parser() {
        for command in ["M108", "M410"] {
            let wire = Wire::new(vec![]);
            let c = comm_on(&wire, |_| {});
            c._clear_to_send.set();
            assert!(c._enqueue_for_sending(cmd("G1 X1"), None, None, None, false, None));

            assert_eq!(Some(true), send(&c, command));
            assert_eq!("", wire.text(), "{command}: nothing is written");
            assert_eq!(1, c._clear_to_send.counter(), "{command}: no ok is used up");
            assert_eq!(
                vec!["G1 X1".to_string(), command.to_string()],
                queued_commands(&c)
            );
        }
    }

    #[test]
    fn ours_m112_is_sent_plain_and_with_checksum_then_the_heaters_off_then_closes() {
        let wire = Wire::new(vec![]);
        let c = comm_on(&wire, |_| {});
        // M112 needs no EMERGENCY_PARSER (`_gcode_M112_queuing`, comm.py:5524)
        assert_eq!(Some(false), send(&c, "M112"));
        let text = wire.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!("M112", lines[0]);
        assert_eq!("N1 M112*32", lines[1]);
        assert!(lines[2].starts_with("N2 M104 T0 S0*"), "{lines:?}");
        assert!(lines[3].starts_with("N3 M140 S0*"), "{lines:?}");
        assert_eq!(4, lines.len());
        assert_eq!(
            "Closing serial port due to emergency stop M112.",
            c.get_error_string()
        );
        assert_eq!(MachineCom::STATE_CLOSED_WITH_ERROR, c.get_state());
    }

    #[test]
    fn ours_blocked_commands_are_not_sent() {
        let wire = Wire::new(vec![]);
        let c = comm_on(&wire, |_| {});
        // M0 is in `serial.blockedCommands`
        assert_eq!(Some(false), send(&c, "M0"));
        assert!(queued_commands(&c).is_empty());
    }

    #[test]
    fn ours_bed_commands_are_suppressed_without_a_heated_bed() {
        let wire = Wire::new(vec![]);
        let c = MachineCom::new(None, None, None, Some(Arc::new(Profile(false, 1))));
        c._serial.set(Some(wire.clone()));
        c._state.set(MachineCom::STATE_OPERATIONAL);
        assert_eq!(Some(false), send(&c, "M140 S60"));
        assert_eq!(Some(false), send(&c, "M190 S60"));
        assert!(queued_commands(&c).is_empty());
        assert!(c._terminal_log.with(|l| l.iter().any(|m| {
            m.starts_with("Warn: Not sending \"M140 S60\", printer profile has no heated bed")
        })));
        assert_eq!(Some(true), send(&c, "M104 S200"));
    }

    #[test]
    fn ours_a_tool_the_profile_does_not_have_is_not_queued_and_a_tool_change_is_tracked() {
        let wire = Wire::new(vec![]);
        let c = comm_on(&wire, |_| {});
        assert_eq!(Some(false), send(&c, "T3"));
        assert!(c._terminal_log.with(|l| {
            l.iter()
                .any(|m| m.starts_with("Warn: Not queuing T3, that tool doesn't exist"))
        }));
        assert_eq!(Some(true), send(&c, "T0"));
        assert_eq!(vec!["T0".to_string()], queued_commands(&c));

        c._process_command_phase("sent", "T0", None, None, None, None);
        assert_eq!(0, c._currentTool.get());
        assert_eq!(Some(0), c._toolBeforeChange.get());
    }

    #[test]
    fn ours_sent_phase_tracks_z_f_fan_and_targets() {
        let wire = Wire::new(vec![]);
        let c = comm_on(&wire, |_| {});
        c.last_temperature
            .with_mut(|t| t.set_tool(0, Some(20.0), None));

        c._process_command_phase("sent", "G1 Z5.5 F3000", None, None, None, None);
        assert_eq!(Some(5.5), c._currentZ.get());
        assert_eq!(Some(3000.0), c._currentF.get());

        c._process_command_phase("sent", "M106 S128", None, None, None, None);
        assert_eq!(Some(128), c.last_fanspeed.get());
        c._process_command_phase("sent", "M107", None, None, None, None);
        assert_eq!(Some(0), c.last_fanspeed.get());

        c._process_command_phase("sent", "M104 S200", None, None, None, None);
        assert_eq!(Some(200.0), c.last_temperature.with(|t| t.tools())[0].1.1);
        c._process_command_phase("sent", "M140 S60", None, None, None, None);
        assert_eq!(Some(60.0), c.last_temperature.with(|t| t.bed()).1);

        c._process_command_phase("sent", "M109 S210", None, None, None, None);
        assert!(
            c._heating.get()
                && c._long_running_command.get()
                && c._heatupWaitStartTime.get().is_some()
        );
        assert_eq!(Some(210.0), c.last_temperature.with(|t| t.tools())[0].1.1);
    }

    #[test]
    fn ours_g4_extends_the_timeout_and_m110_resets_the_line_number() {
        let wire = Wire::new(vec![]);
        let c = comm_on(&wire, |_| {});
        let before = time_monotonic();
        c._process_command_phase("sent", "G4 P2000", None, None, None, None);
        assert!(c._dwelling_until.get().unwrap() >= before + 2.0);
        assert!(c._timeout.get().unwrap() >= before + 2.0);

        c._current_line.set(9);
        c._lastLines.with_mut(|l| l.append(b"x".to_vec()));
        c._resendDelta.set(Some(1));
        c._process_command_phase("sending", "M110 N5", None, None, None, None);
        assert_eq!(5, c._current_line.get());
        assert!(c._lastLines.with(|l| l.is_empty()));
        assert_eq!(None, c._resendDelta.get());
    }

    #[test]
    fn ours_long_running_commands_set_the_flag_when_sending() {
        let wire = Wire::new(vec![]);
        let c = comm_on(&wire, |_| {});
        c._process_command_phase("sending", "G28", None, None, None, None);
        assert!(c._long_running_command.get());
    }

    #[test]
    fn ours_a_command_type_that_is_already_queued_is_not_enqueued_again() {
        let wire = Wire::new(vec![]);
        let c = comm_on(&wire, |_| {});
        assert!(c._enqueue_for_sending(cmd("M105"), None, Some("temperature"), None, false, None));
        assert!(!c._enqueue_for_sending(cmd("M105"), None, Some("temperature"), None, false, None));
        assert_eq!(1, c._send_queue.qsize());
    }

    #[test]
    fn ours_a_write_that_times_out_is_tried_once_more() {
        let wire = Wire::new(vec![Err(SerialError::new(
            "SerialTimeoutException",
            "Write timeout",
            true,
        ))]);
        let c = comm_on(&wire, |_| {});
        c._do_send_without_checksum(b"M105", true);
        assert_eq!(
            vec![b"M105\n".to_vec(), b"M105\n".to_vec()],
            *wire.calls.lock().unwrap()
        );
        assert_eq!("M105\n", wire.text());
        assert!(c._terminal_log.with(|l| {
            l.iter()
                .any(|m| m == "Serial timeout while writing to serial port, trying again.")
        }));
        assert_eq!(1, c._transmitted_lines.get());
    }

    #[test]
    fn ours_a_partial_write_sends_the_rest() {
        let wire = Wire::new(vec![Ok(Some(2))]);
        let c = comm_on(&wire, |_| {});
        c._do_send_without_checksum(b"M105", true);
        assert_eq!(
            vec![b"M105\n".to_vec(), b"05\n".to_vec()],
            *wire.calls.lock().unwrap()
        );
        assert_eq!("M105\n", wire.text());
    }

    #[test]
    fn ours_a_write_that_gives_no_count_counts_as_everything_written() {
        let wire = Wire::new(vec![Ok(None)]);
        let c = comm_on(&wire, |_| {});
        c._do_send_without_checksum(b"M105", true);
        assert_eq!(1, wire.calls.lock().unwrap().len());
    }

    #[test]
    fn ours_nothing_written_in_too_many_passes_closes_with_an_error() {
        let wire = Wire::new(vec![Ok(Some(0)), Ok(Some(0)), Ok(Some(0))]);
        let c = comm_on(&wire, |c| c._max_write_passes = Some(1));
        c._do_send_without_checksum(b"M105", true);
        assert_eq!(2, wire.calls.lock().unwrap().len());
        assert_eq!("Could not write to serial port", c.get_error_string());
        assert_eq!(MachineCom::STATE_CLOSED_WITH_ERROR, c.get_state());
        assert_eq!(1, c._transmitted_lines.get());
    }

    #[test]
    fn ours_any_other_write_error_closes_with_the_error_text() {
        let wire = Wire::new(vec![Err(SerialError::serial_exception(
            "write failed: boom",
        ))]);
        let c = comm_on(&wire, |_| {});
        c._do_send_without_checksum(b"M105", true);
        assert_eq!("SerialException: write failed: boom", c.get_error_string());
        assert_eq!(1, wire.calls.lock().unwrap().len());
    }

    #[test]
    fn ours_checksums_are_used_while_the_firmware_has_not_reported_and_for_m110() {
        let wire = Wire::new(vec![]);
        let c = comm_on(&wire, |_| {});
        assert!(!c._needs_checksum(Some("G1")));
        assert!(c._needs_checksum(Some("M110")), "checksumRequiringCommands");
        assert!(!c._needs_checksum(None));
        c._firmware_info_received.set(false);
        assert!(c._needs_checksum(Some("G1")));
        assert!(
            !c._needs_checksum(None),
            "unknown commands only with sendChecksumWithUnknownCommands"
        );
        c._do_send("G1 X1", Some("G1"));
        assert!(wire.text().starts_with("N1 G1 X1*"));
        assert_eq!(2, c._current_line.get());
    }

    #[test]
    fn ours_the_send_loop_sends_an_enqueued_command_and_stops_when_it_is_closed_down() {
        let wire = Wire::new(vec![]);
        let c = comm_on(&wire, |_| {});
        c._send_queue_active.set(true);
        c._clear_to_send.set();
        assert_eq!(Some(true), send(&c, "G28"));

        let runner = c.clone();
        let handle = std::thread::spawn(move || runner._send_loop());
        let started = std::time::Instant::now();
        while wire.text().is_empty() && started.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!("G28\n", wire.text());
        assert!(
            c._long_running_command.get(),
            "G28 is a long running command"
        );

        // the ok that the loop waits for, then the end: inactive and a marker
        c._send_queue_active.set(false);
        c._send_queue
            .put(
                SendQueueEntry::Marker(SendQueueMarker::new(None)),
                None,
                None,
                true,
                None,
            )
            .unwrap();
        c._clear_to_send.set();
        handle.join().expect("the send loop ends");
    }

    #[test]
    fn ours_an_empty_line_is_not_sent_and_a_marker_runs_its_callback() {
        let wire = Wire::new(vec![]);
        let c = comm_on(&wire, |_| {});
        c._send_queue_active.set(true);
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        assert!(c._enqueue_for_sending(cmd("   "), None, None, None, false, None));
        assert!(c._enqueue_for_sending(
            QueuedCommand::Marker(
                SendQueueMarker::new(Some(Box::new(move || flag.store(true, Ordering::SeqCst)))).0
            ),
            None,
            None,
            None,
            false,
            None
        ));
        c._clear_to_send.set();
        let runner = c.clone();
        let handle = std::thread::spawn(move || runner._send_loop());
        let started = std::time::Instant::now();
        while !ran.load(Ordering::SeqCst) && started.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ran.load(Ordering::SeqCst));
        assert_eq!("", wire.text());
        c._send_queue_active.set(false);
        c._send_queue
            .put(
                SendQueueEntry::Marker(SendQueueMarker::new(None)),
                None,
                None,
                true,
                None,
            )
            .unwrap();
        c._clear_to_send.set();
        handle.join().expect("the send loop ends");
    }

    #[test]
    fn ours_at_commands_are_split_and_dispatched() {
        let wire = Wire::new(vec![]);
        let c = comm_on(&wire, |_| {});
        // `@pause` with the tag of the script that paused already does nothing; an unknown one neither
        let tags = Some(Rc::new(RefCell::new(
            ["script:afterPrintPaused".to_string()]
                .into_iter()
                .collect::<BTreeSet<_>>(),
        )));
        c._process_atcommand_phase("queuing", "@pause", tags.clone());
        c._process_atcommand_phase("queuing", "@nosuchcommand with parameters", None);
        assert!(queued_commands(&c).is_empty());
        // an `@` command is enqueued like any command (`gcode is None`), not sent as g-code
        assert_eq!(Some(true), send(&c, "@nosuchcommand"));
        assert_eq!(vec!["@nosuchcommand".to_string()], queued_commands(&c));
    }

    #[test]
    fn ours_the_serial_text_is_encoded_with_the_serial_encoding() {
        let ascii = Some(Value::String("ascii".to_string()));
        assert_eq!(
            b"M117 caf?".to_vec(),
            encode_serial_text("M117 caf\u{e9}", &ascii, true)
        );
        assert_eq!(
            "M117 caf\u{e9}".as_bytes().to_vec(),
            encode_serial_text(
                "M117 caf\u{e9}",
                &Some(Value::String("utf-8".to_string())),
                true
            )
        );
        assert_eq!(
            b"M117 caf\xe9".to_vec(),
            encode_serial_text(
                "M117 caf\u{e9}",
                &Some(Value::String("latin-1".to_string())),
                true
            )
        );
        assert!(
            std::panic::catch_unwind(|| encode_serial_text(
                "caf\u{e9}",
                &Some(Value::String("ascii".to_string())),
                false
            ))
            .is_err()
        );
    }

    #[test]
    fn ours_the_test_guard_keeps_tests_off_the_real_devices() {
        // `serial_list()` scans a temp directory, not /dev
        assert!(serial_list().is_empty());
        // `_open_serial` panics on a path outside the temp directory ...
        for port in [
            "/dev/ttyUSB0",
            "/dev/tty.usbmodem1",
            "/tmp/../dev/ttyUSB0",
            "relative",
            "COM3",
            "\\\\.\\COM3",
        ] {
            let c = MachineCom::new(None, None, None, None);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                c._open_serial(port, 115200, false)
            }));
            let text = result
                .unwrap_err()
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_default();
            assert!(text.starts_with("test guard:"), "{port}: {text}");
        }
        // ... but not on the nonexistent path every test may use
        let c = MachineCom::new(None, None, None, None);
        assert!(!c._open_serial("/dev/maker-connect-test-does-not-exist", 115200, false));
    }
}
