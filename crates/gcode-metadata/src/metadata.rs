//! `gcode_metadata/metadata.py` (prusa3d/gcode-metadata at 7d8f25d, LGPLv2+): metadata and
//! thumbnails of a print file, function by function.
//!
//! Python's classes `MetaData` and `FDMMetaData` are one [`MetaData`] with its [`Kind`]; a
//! method a subclass overrides matches on the kind.
//!
//! Left out (owner, 2026-11-20: SL1 is not an FDM printer, out for now): `SLMetaData`, `SLKeys`
//! and `SLA_EXTENSIONS`. An `.sl1` (`.sl1s`, `.m1`, `.sl2`, `.slx`) is therefore an
//! `UnknownGcodeFileType` here, where the reference reads it as a zip archive. The port with SL1
//! (and the zipfile and zlib it stands on) is parked on the local branch `sl1-parked`.
//!
//! Quirks of the reference kept as they are: `quick_parse` loops for ever when the file
//! shrinks while it reads (its `readline` returns nothing and `position` never reaches the
//! size); `load_from_chunk` cuts a chunk at an absolute file position. Not covered: a cache
//! file whose `metadata` is not an object (Python takes any JSON value as `data`; ours refuses
//! it as a `TypeError`, so `load` parses the file instead), and `str.lower()` of characters
//! whose lowercase differs between Unicode versions.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::sync::LazyLock;
use std::time::Instant;

use regex::Regex;

use crate::py::{self, Dict, PyErr, PyInt, PyValue};
use crate::py_unicode;

/// `importlib.metadata.version('py-gcode-metadata')` of the reference installed with pip
/// (pyproject.toml's version; measured 2026-11-20 on CPython 3.13.12).
pub const VERSION: &str = "0.2.1";

pub const GCODE_EXTENSIONS: [&str; 4] = [".gcode", ".gc", ".g", ".gco"];
pub const CHARS_TO_REMOVE: [&str; 8] = ["/", "\\", "\"", "(", ")", "[", "]", "'"];

const LOG: &str = "connect-printer";

/// The printer models, in the source's order.
const PRINTERS_SOURCE: [&str; 36] = [
    "COREONE",
    "HT90",
    "MK2.5",
    "MK2.5S",
    "MK2.5MMU2",
    "MK2.5SMMU2S",
    "MK3",
    "MK3S",
    "MK3MMU2",
    "MK3SMMU2S",
    "MK3MMU3",
    "MK3SMMU3",
    "MK3.9", // no IS in name as it shipped with IS FW
    "MK3.9MMU3",
    "MK3.9S",
    "MK3.9SMMU3",
    "MK3.5",
    "MK3.5MMU3",
    "MK3.5S",
    "MK3.5SMMU3",
    "MK4",
    // 'MK4MMU3',  # MMU3 for MK4 only for MK4IS
    "MK4ISMMU3",
    "MK4IS",
    "MK4S",
    "MK4SMMU3", // no IS in name as it shipped with IS FW
    "MINI",
    "MINIIS",
    "XL",
    "XL2",
    "XL5",
    "XLIS",
    "XL2IS",
    "XL5IS",
    "SL1",
    "SL1S",
    "M1",
];

/// `PRINTERS.sort(key=len, reverse=True)`: a stable sort, so equal lengths keep their order.
pub static PRINTERS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut printers: Vec<&'static str> = PRINTERS_SOURCE.to_vec();
    printers.push("iX");
    // `reverse=True` keeps equal elements in their original order, as a stable sort does
    printers.sort_by_key(|p| std::cmp::Reverse(p.len()));
    printers
});

pub const MATERIALS: [&str; 22] = [
    "PLA",
    "PETG",
    "ABS",
    "ASA",
    "FLEX",
    "HIPS",
    "EDGE",
    "NGEN",
    "PA",
    "PVA",
    "PCTG",
    "PP",
    "PC",
    "PEBA",
    "CPE",
    "PVB",
    "PET",
    "PLA Tough",
    "METAL",
    "TPU",
    "NYLON",
    "IGLIDUR",
];

pub const IMAGE_FORMATS: [&str; 2] = ["PNG", "JPG"];

// ---------------------------------------------------------------------------------------------
// Python's `re` classes on `str`, from CPython's own tables

fn class_of(ranges: &[(u32, u32)]) -> String {
    let mut out = String::from("[");
    for &(first, last) in ranges {
        let _ =
            std::fmt::Write::write_fmt(&mut out, format_args!("\\x{{{first:X}}}-\\x{{{last:X}}}"));
    }
    out.push(']');
    out
}

/// `\d` of `re` on `str`
static D: LazyLock<String> = LazyLock::new(|| {
    let ranges: Vec<(u32, u32)> = py_unicode::DECIMAL
        .iter()
        .map(|&(a, b, _)| (a, b))
        .collect();
    class_of(&ranges)
});
/// `\w` of `re` on `str`
static W: LazyLock<String> = LazyLock::new(|| class_of(py_unicode::WORD));
/// `\s` of `re` on `str`
static S: LazyLock<String> = LazyLock::new(|| class_of(py_unicode::ISSPACE));

/// A Python pattern in Rust's syntax: `\d`, `\w`, `\s` as CPython's classes. A `$` at the end
/// of a `re.match` pattern (end, or before a final newline) is written `\n?\z` by the caller.
fn py_re(pattern: &str) -> Regex {
    let pattern = pattern
        .replace("\\d", &D)
        .replace("\\w", &W)
        .replace("\\s", &S);
    Regex::new(&pattern).expect("a ported pattern compiles")
}

static RE_ESTIMATED: LazyLock<Regex> = LazyLock::new(|| {
    py_re(concat!(
        r"^((?P<days>[0-9]+)d\s*)?",
        r"((?P<hours>[0-9]+)h\s*)?",
        r"((?P<minutes>[0-9]+)m\s*)?",
        r"((?P<seconds>[0-9]+)s)?"
    ))
});

/// `str.lower()`
fn py_lower(s: &str) -> String {
    s.to_lowercase()
}

// ---------------------------------------------------------------------------------------------

/// The errors `metadata.py` lets through: Python's exceptions, `OSError`s with their kind, and
/// `UnknownGcodeFileType`.
#[derive(Debug)]
pub enum MetaError {
    UnknownGcodeFileType(String),
    Py(PyErr),
    Os(std::io::Error),
}

impl From<PyErr> for MetaError {
    fn from(err: PyErr) -> MetaError {
        MetaError::Py(err)
    }
}

impl From<std::io::Error> for MetaError {
    fn from(err: std::io::Error) -> MetaError {
        MetaError::Os(err)
    }
}

impl std::fmt::Display for MetaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetaError::UnknownGcodeFileType(path) => write!(f, "UnknownGcodeFileType: {path}"),
            MetaError::Py(err) => write!(f, "{err}"),
            MetaError::Os(err) => write!(f, "OSError: {err}"),
        }
    }
}

impl std::error::Error for MetaError {}

impl MetaError {
    /// `FileNotFoundError` (`ENOENT`)
    pub fn is_file_not_found(&self) -> bool {
        matches!(self, MetaError::Os(err) if err.kind() == std::io::ErrorKind::NotFound)
    }
}

/// A class to hold image info for thumbnail selection purposes
#[derive(Debug, Clone, PartialEq)]
pub struct ImageInfo {
    pub width: PyInt,
    pub height: PyInt,
    pub format: String,
}

impl ImageInfo {
    pub fn new(width: i64, height: i64, format: &str) -> ImageInfo {
        ImageInfo {
            width: PyInt::from_i64(width),
            height: PyInt::from_i64(height),
            format: format.to_owned(),
        }
    }

    fn w(&self) -> f64 {
        self.width.to_f64().unwrap_or(f64::INFINITY)
    }

    fn h(&self) -> f64 {
        self.height.to_f64().unwrap_or(f64::INFINITY)
    }

    /// Gets the image ratio
    pub fn ratio(&self) -> f64 {
        self.w() / self.h()
    }

    /// Returns a badness score for the size difference between the image and the target
    /// dimension. The score is higher when the dimension is smaller than the target dimension
    pub fn dimension_badness(width: f64, target_width: f64) -> f64 {
        let size_difference = width - target_width;
        if size_difference < 0.0 {
            return (size_difference.abs() + 2.0).powi(2);
        }
        size_difference
    }

    /// Returns a badness score for this image compared to the target
    pub fn badness(&self, target: &ImageInfo, aspect_ratio_weight: f64) -> f64 {
        // This gives a value between 1 and infinity
        let ar_badness = self.ratio().max(target.ratio()) / self.ratio().min(target.ratio());

        let width_badness = Self::dimension_badness(self.w(), target.w());
        let height_badness = Self::dimension_badness(self.h(), target.h());
        let size_badness = width_badness + height_badness;

        // The aspect ratio weight of 2 would square aspect ratio badness while using a square
        // root on the size badness. As the aspect ratio minimum is 1, let's make the lowest
        // value 0.01 by subtracting 0.99
        let weighted_ar_badness = ar_badness.powf(aspect_ratio_weight) - 0.99;
        let weighted_size_badness = size_badness.powf(1.0 / aspect_ratio_weight);

        weighted_ar_badness * weighted_size_badness
    }

    /// Parses thumbnail info from string thumbnail key format
    pub fn from_thumbnail_info(info: &str) -> Result<ImageInfo, PyErr> {
        let unpack = |parts: Vec<&str>| -> Result<(String, String), PyErr> {
            match parts.as_slice() {
                [a, b] => Ok(((*a).to_owned(), (*b).to_owned())),
                more if more.len() > 2 => Err(PyErr::ValueError(
                    "too many values to unpack (expected 2)".into(),
                )),
                fewer => Err(PyErr::ValueError(format!(
                    "not enough values to unpack (expected 2, got {})",
                    fewer.len()
                ))),
            }
        };
        let (string_resolution, format) = unpack(info.split('_').collect())?;
        let parts: Vec<&str> = string_resolution.split('x').collect();
        // `tuple(map(int, ...))` converts every part before the unpack
        let ints = parts
            .iter()
            .map(|p| py::int_from_str(p))
            .collect::<Result<Vec<PyInt>, PyErr>>()?;
        match ints.as_slice() {
            [w, h] => Ok(ImageInfo {
                width: w.clone(),
                height: h.clone(),
                format,
            }),
            more if more.len() > 2 => Err(PyErr::ValueError(
                "too many values to unpack (expected 2)".into(),
            )),
            fewer => Err(PyErr::ValueError(format!(
                "not enough values to unpack (expected 2, got {})",
                fewer.len()
            ))),
        }
    }

    /// Returns thumbnail info in string thumbnail key format
    pub fn to_thumbnail_info(&self) -> String {
        format!(
            "{}x{}_{}",
            self.width.as_str(),
            self.height.as_str(),
            self.format
        )
    }
}

impl std::fmt::Display for ImageInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_thumbnail_info())
    }
}

/// Returns a name for the new list value item
pub fn get_mmu_name(name: &str) -> String {
    format!("{name} per tool")
}

/// Check g-code integrity
pub fn check_gcode_completion(path: &str) {
    log::debug!(target: LOG, "{path}");
}

/// Convert data in bytes to string
pub fn from_bytes(data: &[u8]) -> Result<String, PyErr> {
    std::str::from_utf8(data)
        .map(str::to_owned)
        .map_err(|e| PyErr::ValueError(format!("'utf-8' codec can't decode bytes: {e}")))
}

/// Convert string to data in bytes
pub fn to_bytes(data: &str) -> Vec<u8> {
    data.as_bytes().to_vec()
}

/// Convert string value to seconds.
pub fn estimated_to_seconds(value: &str) -> Option<i64> {
    let lowered = py_lower(value);
    let m = RE_ESTIMATED.captures(&lowered)?;
    let part = |name: &str| -> i64 {
        m.name(name)
            .map(|g| {
                py::int_from_str(g.as_str())
                    .ok()
                    .and_then(|i| i.to_i64())
                    .expect("[0-9]+ is an int")
            })
            .unwrap_or(0)
    };
    let mut retval = part("days") * 60 * 60 * 24;
    retval += part("hours") * 60 * 60;
    retval += part("minutes") * 60;
    retval += part("seconds");
    if retval == 0 { None } else { Some(retval) }
}

/// Returns a value only if all the values in a list are the same
pub fn same_or_nothing(value_list: &[PyValue]) -> Result<PyValue, PyErr> {
    if value_list.iter().any(|x| x != &value_list[0]) {
        return Err(PyErr::ValueError("The values were not the same".into()));
    }
    Ok(value_list[0].clone())
}

/// Read `support_material` as 0/1 in both slicer generations.
///
/// PrusaSlicer 3.0 names the mode; any mode but `none` generates supports, which is what `1`
/// meant before 3.0.
pub fn support_material_flag(value: &PyValue) -> Result<PyValue, PyErr> {
    if let PyValue::Str(s) = value
        && (s == "none" || s == "enforcers_only" || s == "everywhere")
    {
        return Ok(PyValue::int((s != "none") as i64));
    }
    Ok(PyValue::Int(value.to_py_int()?))
}

/// A search's match: its start, its end, and the span of its group 1 if that took part.
type Found = Option<(usize, usize, Option<(usize, usize)>)>;

/// One of the patterns of `extract_data`, by its key.
struct DataPattern {
    key: &'static str,
    search: fn(&str) -> Found,
}

/// `(.*?)(?=LOOKAHEAD)` searched: the first start, then the shortest match not crossing a
/// newline, where `lookahead` matches at its end.
fn search_lazy_until(s: &str, lookahead: &Regex) -> Found {
    let starts: Vec<usize> = s.char_indices().map(|(i, _)| i).chain([s.len()]).collect();
    for &start in &starts {
        let mut end = start;
        loop {
            if lookahead.find_at(s, end).is_some_and(|m| m.start() == end) {
                return Some((start, end, Some((start, end))));
            }
            match s[end..].chars().next() {
                Some(c) if c != '\n' => end += c.len_utf8(),
                _ => break,
            }
        }
    }
    None
}

static NAME_LOOKAHEAD: LazyLock<Regex> = LazyLock::new(|| {
    py_re(&format!(
        r"(?:[0-9.]+n|mm|{}|{}|\d+[dhm]+)",
        MATERIALS.join("|"),
        PRINTERS.join("|")
    ))
});
static NOZZLE: LazyLock<Regex> = LazyLock::new(|| py_re(r"([0-9.]+)n"));
static HEIGHT: LazyLock<Regex> = LazyLock::new(|| py_re(r"([0-9.]+)mm"));
static MATERIAL: LazyLock<Regex> =
    LazyLock::new(|| py_re(&format!(r"(?:{})", MATERIALS.join("|"))));
static PRINTER: LazyLock<Regex> = LazyLock::new(|| py_re(&format!(r"(?:{})", PRINTERS.join("|"))));

fn search_group1(re: &Regex, s: &str) -> Found {
    let c = re.captures(s)?;
    let whole = c.get(0).unwrap();
    Some((
        whole.start(),
        whole.end(),
        c.get(1).map(|g| (g.start(), g.end())),
    ))
}

/// `(\d+[dhm]+(?:\d*[dhm]+)*)(?!\w)` searched, by backtracking in `re`'s order.
fn search_time(s: &str) -> Found {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let byte_at = |i: usize| chars.get(i).map(|c| c.0).unwrap_or(s.len());
    let is_d = |i: usize| chars.get(i).is_some_and(|c| py::decimal(c.1).is_some());
    let is_u = |i: usize| chars.get(i).is_some_and(|c| matches!(c.1, 'd' | 'h' | 'm'));
    let lookahead_ok = |i: usize| !chars.get(i).is_some_and(|c| py::isword(c.1));
    let run = |from: usize, pred: &dyn Fn(usize) -> bool| {
        let mut i = from;
        while pred(i) {
            i += 1;
        }
        i - from
    };

    // `(?:\d*[dhm]+)*` from `pos`, greedy: the ends in the order `re` tries them
    fn star(
        pos: usize,
        is_d: &dyn Fn(usize) -> bool,
        is_u: &dyn Fn(usize) -> bool,
        ok: &dyn Fn(usize) -> bool,
    ) -> Option<usize> {
        let mut digits = 0;
        while is_d(pos + digits) {
            digits += 1;
        }
        for d in (0..=digits).rev() {
            let p = pos + d;
            let mut units = 0;
            while is_u(p + units) {
                units += 1;
            }
            for u in (1..=units).rev() {
                if let Some(end) = star(p + u, is_d, is_u, ok) {
                    return Some(end);
                }
            }
        }
        // zero more iterations
        if ok(pos) { Some(pos) } else { None }
    }

    for start in 0..=chars.len() {
        let digits = run(start, &is_d);
        for d in (1..=digits).rev() {
            let p = start + d;
            let units = run(p, &is_u);
            for u in (1..=units).rev() {
                if let Some(end) = star(p + u, &is_d, &is_u, &lookahead_ok) {
                    let (a, b) = (byte_at(start), byte_at(end));
                    return Some((a, b, Some((a, b))));
                }
            }
        }
    }
    None
}

/// Extracts metadata from the filename
pub fn extract_data(input_string: &str) -> Result<Dict, PyErr> {
    let patterns = [
        DataPattern {
            key: "name",
            search: |s| search_lazy_until(s, &NAME_LOOKAHEAD),
        },
        DataPattern {
            key: "nozzle",
            search: |s| search_group1(&NOZZLE, s),
        },
        DataPattern {
            key: "height",
            search: |s| search_group1(&HEIGHT, s),
        },
        DataPattern {
            key: "material",
            search: |s| search_group1(&MATERIAL, s),
        },
        DataPattern {
            key: "printer",
            search: |s| search_group1(&PRINTER, s),
        },
        DataPattern {
            key: "time",
            search: search_time,
        },
    ];

    let mut data = Dict::new();
    for pattern in patterns {
        match (pattern.search)(input_string) {
            Some((start, end, group1)) => {
                if pattern.key == "nozzle" || pattern.key == "height" {
                    let (a, b) = group1.expect("the pattern has a group");
                    data.set(
                        pattern.key,
                        PyValue::Float(py::float_from_str(&input_string[a..b])?),
                    );
                } else {
                    data.set(pattern.key, PyValue::str(&input_string[start..end]));
                }
            }
            None => data.set(pattern.key, PyValue::None),
        }
    }
    Ok(data)
}

/// `os.path.split` (posixpath)
fn path_split(p: &str) -> (String, String) {
    let i = p.rfind('/').map(|i| i + 1).unwrap_or(0);
    let (mut head, tail) = (p[..i].to_owned(), p[i..].to_owned());
    if !head.is_empty() && head.chars().any(|c| c != '/') {
        head = head.trim_end_matches('/').to_owned();
    }
    (head, tail)
}

/// `os.path.basename` (posixpath)
fn basename(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

/// `os.path.getctime`: `st_ctime` as `sec + nsec * 1e-9` (on Windows, the creation time).
fn getctime(path: &str) -> std::io::Result<f64> {
    let meta = std::fs::metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(meta.ctime() as f64 + meta.ctime_nsec() as f64 * 1e-9)
    }
    #[cfg(not(unix))]
    {
        let created = meta.created()?;
        let since = created
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Ok(since.as_secs() as f64 + since.subsec_nanos() as f64 * 1e-9)
    }
}

/// `open(path, "r", encoding="utf-8").read()`: strict UTF-8, universal newlines.
fn read_text(path: &str) -> Result<String, MetaError> {
    let bytes = std::fs::read(path)?;
    let text = from_bytes(&bytes)?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// `open(path, "w", encoding="utf-8").write(text)`: `\n` written as `os.linesep`.
fn write_text(path: &str, text: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    let text = text.replace('\n', "\r\n");
    std::fs::write(path, text)
}

/// The conversion functions of `Attrs`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Conv {
    Str,
    Float,
    Int,
    Bool,
    SupportMaterialFlag,
    JsonLoads,
    List,
    /// `lambda x: int(float(x))`
    IntOfFloat,
    /// `lambda x: round(float(x), 2)`
    Round2,
}

impl Conv {
    pub fn apply(self, value: &PyValue) -> Result<PyValue, PyErr> {
        match self {
            Conv::Str => Ok(PyValue::Str(value.to_py_str())),
            Conv::Float => Ok(PyValue::Float(value.to_py_float()?)),
            Conv::Int => Ok(PyValue::Int(value.to_py_int()?)),
            Conv::Bool => Ok(PyValue::Bool(value.truthy())),
            Conv::SupportMaterialFlag => support_material_flag(value),
            Conv::JsonLoads => match value {
                PyValue::Str(s) => py::json_loads(s),
                other => Err(PyErr::TypeError(format!(
                    "the JSON object must be str, bytes or bytearray, not {}",
                    other.type_name()
                ))),
            },
            Conv::List => Ok(PyValue::List(value.to_py_list()?)),
            Conv::IntOfFloat => Ok(PyValue::Int(py::int_from_f64(value.to_py_float()?)?)),
            Conv::Round2 => Ok(PyValue::Float(py::round_f64(value.to_py_float()?, 2))),
        }
    }
}

/// How an MMU attribute's conversion makes one value of the list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Conversion {
    Sum,
    SameOrNothing,
}

/// A class describing how to parse an attribute that can have multiple values for an mmu print
#[derive(Debug, Clone, Copy)]
pub struct MMUAttribute {
    pub separator: &'static str,
    pub value_type: Conv,
    pub conversion: Conversion,
}

impl MMUAttribute {
    /// Parses the value from raw_value, returns the list value as well as a value for a single
    /// tool info compatibility
    pub fn parse_tools(
        &self,
        raw_value: &PyValue,
    ) -> Result<(Vec<PyValue>, Option<PyValue>), PyErr> {
        let values: Vec<PyValue> = match raw_value {
            PyValue::Str(s) => s.split(self.separator).map(PyValue::str).collect(),
            other => vec![other.clone()],
        };
        let mut parsed = Vec::new();
        for value in &values {
            match self.value_type.apply(value) {
                Ok(v) => parsed.push(v),
                Err(err) if err.is_value_error() => return Ok((Vec::new(), None)),
                Err(err) => return Err(err),
            }
        }
        let single = match self.conversion {
            Conversion::Sum => py::sum(&parsed),
            Conversion::SameOrNothing => same_or_nothing(&parsed),
        };
        let single = match single {
            Ok(v) => Some(v),
            Err(err) if err.is_value_error() => None,
            Err(err) => return Err(err),
        };
        Ok((parsed, single))
    }
}

/// `FDMMetaData.MMUAttrs`, in order.
pub const MMU_ATTRS: [(&str, MMUAttribute); 11] = [
    (
        "filament used [cm3]",
        MMUAttribute {
            separator: ", ",
            value_type: Conv::Float,
            conversion: Conversion::Sum,
        },
    ),
    (
        "filament used [mm]",
        MMUAttribute {
            separator: ", ",
            value_type: Conv::Float,
            conversion: Conversion::Sum,
        },
    ),
    (
        "filament used [g]",
        MMUAttribute {
            separator: ", ",
            value_type: Conv::Float,
            conversion: Conversion::Sum,
        },
    ),
    (
        "filament cost",
        MMUAttribute {
            separator: ", ",
            value_type: Conv::Float,
            conversion: Conversion::Sum,
        },
    ),
    (
        "filament_type",
        MMUAttribute {
            separator: ";",
            value_type: Conv::Str,
            conversion: Conversion::SameOrNothing,
        },
    ),
    (
        "temperature",
        MMUAttribute {
            separator: ",",
            value_type: Conv::Int,
            conversion: Conversion::SameOrNothing,
        },
    ),
    (
        "bed_temperature",
        MMUAttribute {
            separator: ",",
            value_type: Conv::Int,
            conversion: Conversion::SameOrNothing,
        },
    ),
    (
        "nozzle_diameter",
        MMUAttribute {
            separator: ",",
            value_type: Conv::Float,
            conversion: Conversion::SameOrNothing,
        },
    ),
    (
        "extruder_colour",
        MMUAttribute {
            separator: ";",
            value_type: Conv::Str,
            conversion: Conversion::SameOrNothing,
        },
    ),
    (
        "nozzle_high_flow",
        MMUAttribute {
            separator: ",",
            value_type: Conv::Int,
            conversion: Conversion::SameOrNothing,
        },
    ),
    (
        "filament_abrasive",
        MMUAttribute {
            separator: ",",
            value_type: Conv::Int,
            conversion: Conversion::SameOrNothing,
        },
    ),
];

fn mmu_attr(name: &str) -> Option<&'static MMUAttribute> {
    MMU_ATTRS.iter().find(|(n, _)| *n == name).map(|(_, a)| a)
}

/// `FDMMetaData.Attrs`: the keys primarily defined by PrusaSlicer, then per MMU attribute its
/// value type and a `list` for `"<name> per tool"`.
pub static FDM_ATTRS: LazyLock<Vec<(String, Conv)>> = LazyLock::new(|| {
    let mut attrs: Vec<(String, Conv)> = [
        ("estimated printing time (normal mode)", Conv::Str),
        ("printer_model", Conv::Str),
        ("layer_height", Conv::Float),
        ("fill_density", Conv::Str),
        ("brim_width", Conv::Int),
        ("support_material", Conv::SupportMaterialFlag),
        ("ironing", Conv::Int),
        ("quiet_percent_present", Conv::Bool),
        ("quiet_left_present", Conv::Bool),
        ("quiet_change_in_present", Conv::Bool),
        ("normal_percent_present", Conv::Bool),
        ("normal_left_present", Conv::Bool),
        ("normal_change_in_present", Conv::Bool),
        ("layer_info_present", Conv::Bool),
        ("max_layer_z", Conv::Float),
        ("objects_info", Conv::JsonLoads),
    ]
    .iter()
    .map(|(k, c)| ((*k).to_owned(), *c))
    .collect();
    for (name, mmu_attribute) in MMU_ATTRS {
        attrs.push((name.to_owned(), mmu_attribute.value_type));
        attrs.push((get_mmu_name(name), Conv::List));
    }
    attrs
});

/// The class of a [`MetaData`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// `MetaData`
    Base,
    /// `FDMMetaData`
    Fdm,
}

/// `dimensions: base64(data)`, in insertion order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Thumbnails(Vec<(String, Vec<u8>)>);

impl Thumbnails {
    pub fn set(&mut self, key: &str, data: Vec<u8>) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = data,
            None => self.0.push((key.to_owned(), data)),
        }
    }

    pub fn get(&self, key: &str) -> Option<&Vec<u8>> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &Vec<u8>> {
        self.0.iter().map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The M73 info group names and their attribute names (`M73_ATTRS`)
pub const M73_ATTRS: [(&str, &str); 6] = [
    ("quiet_percent", "quiet_percent_present"),
    ("quiet_left", "quiet_left_present"),
    ("quiet_change_in", "quiet_change_in_present"),
    ("normal_percent", "normal_percent_present"),
    ("normal_left", "normal_left_present"),
    ("normal_change_in", "normal_change_in_present"),
];

static KEY_VAL_PAT: LazyLock<Regex> =
    LazyLock::new(|| py_re(r"^; (?P<key>.*?) = (?P<value>.*)\n?\z"));
static THUMBNAIL_BEGIN_PAT: LazyLock<Regex> = LazyLock::new(|| {
    py_re(r"^; thumbnail_?(?P<format>QOI|JPG|) begin (?P<dim>[\w ]+) (?P<size>\d+)")
});
static THUMBNAIL_END_PAT: LazyLock<Regex> =
    LazyLock::new(|| py_re(r"^; thumbnail_?(QOI|JPG)? end"));
static M73_PAT: LazyLock<Regex> = LazyLock::new(|| {
    py_re(concat!(
        r"^[^;]*M73 ?",
        r"(?:Q(?P<quiet_percent>\d+))? ?",
        r"(?:S(?P<quiet_left>\d+))? ?",
        r"(?:C(?P<quiet_change_in>\d+))? ?",
        r"(?:P(?P<normal_percent>\d+))? ?",
        r"(?:R(?P<normal_left>\d+))? ?",
        r"(?:D(?P<normal_change_in>\d+))? ?.*",
        r"\n?\z"
    ))
});
static LAYER_CHANGE_PAT: LazyLock<Regex> = LazyLock::new(|| py_re(r"^;Z:\d+\.\d+\n?\z"));
/// Read 800KB from the start
pub const METADATA_START_OFFSET: u64 = 800000;
/// Read 256KB at the end of the file. PrusaSlicer 3.0 writes a JSON copy of the config
/// (~56KB for 4 tools, growing with the tool count) between max_layer_z and the INI block,
/// putting max_layer_z ~90KB from the end.
pub const METADATA_END_OFFSET: u64 = 256000;
/// Number of times the search for M73 is going to repeat if info is incomplete
pub const MAX_M73_SEARCH_BYTES: usize = 100000;
pub const TOLERATED_COUNT: usize = 2;

/// Base MetaData class, with the FDM subclass as its [`Kind`].
#[derive(Debug, Clone)]
pub struct MetaData {
    pub kind: Kind,
    pub path: String,
    pub thumbnails: Thumbnails,
    pub data: Dict,
    /// buffer which keeps last unfinished line from previous chunk
    pub chunk_buffer: Vec<u8>,
    /// current position in chunked read
    pub position: u64,
    // FDMMetaData
    pub last_filename: Option<String>,
    // When in the process of parsing an image, these won't be None
    // Parsed as in currently being parsed
    pub img_format: Option<String>,
    pub img_dimensions: Option<String>,
    pub img_size: Option<PyInt>,
    pub img: Option<Vec<String>>,
    pub m73_searched_bytes: usize,
}

impl MetaData {
    fn with_kind(kind: Kind, path: &str) -> MetaData {
        MetaData {
            kind,
            path: path.to_owned(),
            thumbnails: Thumbnails::default(),
            data: Dict::new(),
            chunk_buffer: Vec::new(),
            position: 0,
            last_filename: None,
            img_format: None,
            img_dimensions: None,
            img_size: None,
            img: None,
            m73_searched_bytes: 0,
        }
    }

    /// `MetaData(path)`
    pub fn new(path: &str) -> MetaData {
        Self::with_kind(Kind::Base, path)
    }

    /// `FDMMetaData(path)`
    pub fn fdm(path: &str) -> MetaData {
        Self::with_kind(Kind::Fdm, path)
    }

    /// The class's `Attrs`: the conversion of a name, if it is one.
    fn attr(&self, name: &str) -> Option<Conv> {
        match self.kind {
            Kind::Base => None,
            Kind::Fdm => FDM_ATTRS.iter().find(|(n, _)| n == name).map(|(_, c)| *c),
        }
    }

    /// The names of `Attrs`, in order.
    pub fn attrs_keys(&self) -> Vec<String> {
        match self.kind {
            Kind::Base => Vec::new(),
            Kind::Fdm => FDM_ATTRS.iter().map(|(n, _)| n.clone()).collect(),
        }
    }

    /// Create cache name in format `.<filename>.cache`
    pub fn cache_name(&self) -> String {
        let (head, tail) = path_split(&self.path);
        format!("{head}/.{tail}.cache")
    }

    /// Checks if we can use the current cache file
    pub fn is_cache_fresh(&self) -> Result<bool, MetaError> {
        Ok(self.is_cache_recent()? && self.is_cache_correct_version()?)
    }

    /// Checks if the cache file is newer than the source file
    pub fn is_cache_recent(&self) -> Result<bool, MetaError> {
        let not_found = |err: std::io::Error| -> Result<Option<f64>, MetaError> {
            if err.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(MetaError::Os(err))
            }
        };
        let file_time_created = match getctime(&self.path) {
            Ok(t) => Some(t),
            Err(err) => not_found(err)?,
        };
        let Some(file_time_created) = file_time_created else {
            return Ok(false);
        };
        let cache_time_created = match getctime(&self.cache_name()) {
            Ok(t) => Some(t),
            Err(err) => not_found(err)?,
        };
        let Some(cache_time_created) = cache_time_created else {
            return Ok(false);
        };
        Ok(file_time_created < cache_time_created)
    }

    /// Checks if the cache file was created with the same version of gcode-metadata
    pub fn is_cache_correct_version(&self) -> Result<bool, MetaError> {
        /// Filters out disallowed characters, used with str.filter
        fn isallowed(c: char) -> bool {
            !"\",\n} ".contains(c)
        }

        // This expects the first item in the json file to be the gcode-metadata version, with
        // which the cache was created. If it's not there, or the version is different, the
        // cache is deleted
        let text = read_text(&self.cache_name())?;
        let mut found = None;
        for line in text.split_inclusive('\n') {
            let alpha = py::alpha_only(line);
            if !alpha.is_empty() {
                if alpha.starts_with("version") {
                    found = Some(line);
                    break;
                }
                return Ok(false);
            }
        }
        let Some(line) = found else {
            // didn't reach break
            return Ok(false);
        };
        let first_pair = line.split(',').next().unwrap_or("");
        let Some((_, version_part)) = first_pair.split_once(':') else {
            return Err(PyErr::ValueError(
                "not enough values to unpack (expected 2, got 1)".into(),
            )
            .into());
        };
        let file_version: String = version_part.chars().filter(|c| isallowed(*c)).collect();
        Ok(file_version == VERSION)
    }

    /// Take metadata from source file and save them as JSON to `<file_name>.cache` file. Parse
    /// thumbnail from bytes to string format because of JSON serialization requirements
    pub fn save_cache(&self) -> Result<(), MetaError> {
        let get_cache_data = |info: &ImageInfo| -> Result<PyValue, MetaError> {
            let key = info.to_thumbnail_info();
            let data = self
                .thumbnails
                .get(&key)
                .ok_or_else(|| PyErr::KeyError(py::repr_str(&key)))?;
            let mut entry = Dict::new();
            entry.set(
                "resolution",
                PyValue::Str(format!("{}x{}", info.width.as_str(), info.height.as_str())),
            );
            entry.set("data", PyValue::Str(from_bytes(data)?));
            entry.set("format", PyValue::Str(info.format.clone()));
            Ok(PyValue::Dict(entry))
        };

        if !self.data.is_empty() {
            let mut cache = Dict::new();
            cache.set("version", PyValue::str(VERSION));
            cache.set("metadata", PyValue::Dict(self.data.clone()));

            if !self.thumbnails.is_empty() {
                if let Some(preview) = get_preview(&self.thumbnails)? {
                    cache.set("preview", get_cache_data(&preview)?);
                }
                if let Some(icon) = get_icon(&self.thumbnails)? {
                    cache.set("icon", get_cache_data(&icon)?);
                }
            }

            let text = py::json_dumps(&PyValue::Dict(cache), Some(2));
            if let Err(err) = write_text(&self.cache_name(), &text) {
                if err.kind() == std::io::ErrorKind::PermissionDenied {
                    log::warn!(target: LOG, "You don't have permission to save file here");
                } else {
                    return Err(err.into());
                }
            }
        }
        Ok(())
    }

    /// Load metadata values from `<file_name>.cache` file
    pub fn load_cache(&mut self) -> Result<(), MetaError> {
        let incorrect = || {
            MetaError::Py(PyErr::ValueError(
                "JSON data not found or in incorrect format".into(),
            ))
        };
        let text = match read_text(&self.cache_name()) {
            Ok(text) => text,
            Err(err) if err.is_file_not_found() => return Err(incorrect()),
            Err(err) => return Err(err),
        };
        let cache_data = match py::json_loads(&text) {
            Ok(value) => value,
            // `JSONDecodeError` is caught; other errors (`RecursionError`) are not
            Err(err) if err.is_value_error() => return Err(incorrect()),
            Err(err) => return Err(err.into()),
        };
        let PyValue::Dict(cache_data) = cache_data else {
            return Err(PyErr::AttributeError(format!(
                "'{}' object has no attribute 'get'",
                cache_data.type_name()
            ))
            .into());
        };
        let preview = cache_data.get("preview").cloned().unwrap_or(PyValue::None);
        let icon = cache_data.get("icon").cloned().unwrap_or(PyValue::None);

        self.thumbnails = Thumbnails::default();

        for image in [preview, icon] {
            if image.truthy() {
                let item = |name: &str| -> Result<PyValue, MetaError> {
                    match &image {
                        PyValue::Dict(d) => d.get(name).cloned().ok_or_else(incorrect),
                        PyValue::List(_) | PyValue::Str(_) => Err(PyErr::TypeError(format!(
                            "{} indices must be integers or slices, not str",
                            image.type_name()
                        ))
                        .into()),
                        other => Err(PyErr::TypeError(format!(
                            "'{}' object is not subscriptable",
                            other.type_name()
                        ))
                        .into()),
                    }
                };
                let key = format!(
                    "{}_{}",
                    item("resolution")?.to_py_str(),
                    item("format")?.to_py_str()
                );
                let data = match item("data")? {
                    PyValue::Str(s) => to_bytes(&s),
                    other => {
                        return Err(PyErr::TypeError(format!(
                            "encoding without a string argument (got {})",
                            other.type_name()
                        ))
                        .into());
                    }
                };
                self.thumbnails.set(&key, data);
            }
        }

        match cache_data.get("metadata") {
            Some(PyValue::Dict(d)) => self.data = d.clone(),
            Some(other) => {
                return Err(PyErr::TypeError(format!(
                    "a cache's metadata as {}: not covered by the port",
                    other.type_name()
                ))
                .into());
            }
            None => return Err(incorrect()),
        }
        Ok(())
    }

    /// Extract and set metadata from `self.path`. Any metadata obtained from the path will be
    /// overwritten by metadata from the file if the metadata is contained there as well
    pub fn load(&mut self, save_cache: bool) -> Result<(), MetaError> {
        let mut cache_loaded = false;
        if self.is_cache_fresh()? {
            match self.load_cache() {
                Ok(()) => cache_loaded = true,
                Err(_) => log::warn!(target: LOG, "Failed loading cache for: {}", self.path),
            }
        }
        if !cache_loaded {
            let path = self.path.clone();
            let loaded = self
                .load_from_path(&path)
                .and_then(|()| self.load_from_file(&path));
            match loaded {
                Err(err) => {
                    log::error!(target: LOG, "Failed loading metadata from: {}: {err}", self.path)
                }
                Ok(()) => {
                    if save_cache {
                        self.save_cache()?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Load metadata and thumbnails from given `path`
    pub fn load_from_file(&mut self, path: &str) -> Result<(), MetaError> {
        match self.kind {
            Kind::Base => Ok(()),
            Kind::Fdm => self.fdm_load_from_file(path),
        }
    }

    /// Load metadata from given path (path, not its content), if possible.
    pub fn load_from_path(&mut self, path: &str) -> Result<(), MetaError> {
        match self.kind {
            Kind::Base => Ok(()),
            Kind::Fdm => self.fdm_load_from_path(path),
        }
    }

    /// Process given chunk array of data. `data`: data of a chunk. `size`: size of the file.
    pub fn load_from_chunk(&mut self, data: &[u8], size: u64) -> Result<(), MetaError> {
        match self.kind {
            Kind::Base => Ok(()),
            Kind::Fdm => self.fdm_load_from_chunk(data, size),
        }
    }

    /// Helper function to save all items from `data` that match `self.Attr` in `self.data`.
    pub fn set_data(&mut self, data: &Dict) -> Result<(), PyErr> {
        for (key, value) in data.iter() {
            self.set_attr(key, value)?;
        }
        Ok(())
    }

    /// `MetaData.set_attr`: a helper function that saves attributes to `self.data`
    fn base_set_attr(&mut self, name: &str, value: &PyValue) -> Result<(), PyErr> {
        let Some(conv) = self.attr(name) else {
            return Ok(());
        };
        if matches!(value, PyValue::None) {
            return Ok(());
        }
        match conv.apply(value) {
            Ok(converted) => self.data.set(name, converted),
            Err(err) if err.is_value_error() => {
                log::warn!(target: LOG, "Could not convert using {conv:?}: {}", value.repr())
            }
            Err(err) => return Err(err),
        }
        Ok(())
    }

    /// `set_attr`, as the class overrides it
    pub fn set_attr(&mut self, name: &str, value: &PyValue) -> Result<(), PyErr> {
        match self.kind {
            Kind::Base => self.base_set_attr(name, value),
            Kind::Fdm => {
                // Set an attribute, but add support for mmu list attributes
                if value == &PyValue::str("\"\"") {
                    // e.g. when no extruder_colour
                    return Ok(());
                }
                if matches!(value, PyValue::None) {
                    return Ok(());
                }
                match mmu_attr(name) {
                    Some(mmu) => {
                        let (value_list, single_value) = mmu.parse_tools(value)?;
                        let mmu_name = get_mmu_name(name);
                        if value_list.len() > 1 {
                            self.base_set_attr(&mmu_name, &PyValue::List(value_list))?;
                        }
                        if let Some(single) = single_value {
                            self.base_set_attr(name, &single)?;
                        }
                        Ok(())
                    }
                    None => self.base_set_attr(name, value),
                }
            }
        }
    }

    // --- FDMMetaData -------------------------------------------------------------------------

    /// Try to obtain any usable metadata from the path itself
    fn fdm_load_from_path(&mut self, path: &str) -> Result<(), MetaError> {
        let filename = basename(path);
        let data = extract_data(filename)?;
        let get = |key: &str| data.get(key).cloned().unwrap_or(PyValue::None);

        let mut result = Dict::new();
        result.set("name", get("name"));
        result.set("nozzle_diameter", get("nozzle"));
        result.set("layer_height", get("height"));
        result.set("filament_type", get("material"));
        result.set("printer_model", get("printer"));
        result.set("estimated printing time (normal mode)", get("time"));

        self.set_data(&result)?;
        Ok(())
    }

    /// Parses data from a line in the comments
    pub fn from_comment_line(&mut self, line: &str) -> Result<(), PyErr> {
        // thumbnail handling
        if let Some(m) = THUMBNAIL_BEGIN_PAT.captures(line) {
            let img_format = m.name("format").map(|g| g.as_str()).unwrap_or("");
            // PNG is not explicitly described in thumbnails header
            self.img_format = Some(
                if img_format.is_empty() {
                    "PNG"
                } else {
                    img_format
                }
                .to_owned(),
            );
            self.img_dimensions = Some(m["dim"].to_owned());
            self.img_size = Some(py::int_from_str(&m["size"])?);
            self.img = Some(Vec::new());
            return Ok(());
        }

        if THUMBNAIL_END_PAT.is_match(line) {
            let Some(img) = &self.img else {
                return Err(PyErr::TypeError("can only join an iterable".into()));
            };
            let img_data: String = img.concat();
            let key = format!(
                "{}_{}",
                PyValue::from_option_str(&self.img_dimensions).to_py_str(),
                PyValue::from_option_str(&self.img_format).to_py_str()
            );
            self.thumbnails.set(&key, img_data.as_bytes().to_vec());
            let len = PyInt::from_i64(img_data.chars().count() as i64);
            if self.img_size.as_ref() != Some(&len) {
                return Err(PyErr::AssertionError(len.as_str().to_owned()));
            }

            self.img_format = None;
            self.img_dimensions = None;
            self.img_size = None;
            self.img = None;
            return Ok(());
        }

        // We store the image data only during parsing. If actively parsing:
        if let Some(img) = &mut self.img {
            img.push(py::strip(py::skip_chars(line, 2)).to_owned());
        }

        // For the bulk of metadata comments
        if let Some(m) = KEY_VAL_PAT.captures(line) {
            let (key, val) = (m["key"].to_owned(), m["value"].to_owned());
            self.set_attr(&key, &PyValue::Str(val))?;
        }

        if LAYER_CHANGE_PAT.is_match(line) {
            self.set_attr("layer_info_present", &PyValue::Bool(true))?;
        }
        Ok(())
    }

    /// Parses data from a line in the gcode section
    pub fn from_gcode_line(&mut self, line: &str) -> Result<(), PyErr> {
        if let Some(m) = M73_PAT.captures(line) {
            for (group_name, attribute_name) in M73_ATTRS {
                if m.name(group_name).is_some() {
                    self.set_attr(attribute_name, &PyValue::Bool(true))?;
                }
            }
        }
        Ok(())
    }

    /// Load metadata from file. Tries to use the quick_parse function.
    fn fdm_load_from_file(&mut self, path: &str) -> Result<(), MetaError> {
        let started_at = Instant::now();
        let file = File::open(path)?;
        self.quick_parse(file)?;
        log::debug!(target: LOG, "Caching took {}", started_at.elapsed().as_secs_f64());
        Ok(())
    }

    /// Evaluates if the parsed data is sufficient. Can log the result. Returns True if the
    /// data is sufficient
    pub fn evaluate_quick_parse(&self, to_log: bool) -> bool {
        let wanted: Vec<String> = self.attrs_keys();
        let missed: Vec<&String> = wanted.iter().filter(|k| !self.data.contains(k)).collect();
        log::debug!(target: LOG, "Wanted: {wanted:?}");
        log::debug!(target: LOG, "Parsed: {:?}", self.data.keys().collect::<Vec<_>>());
        log::debug!(target: LOG, "By not reading the whole file, we have managed to miss {missed:?}");

        // --- Was parsing successful? ---
        if self.data.len() < 10 {
            log::warn!(target: LOG, "Not enough info found, file not uploaded yet?");
            return false;
        }

        if !missed.is_empty() && to_log {
            if missed.len() == wanted.len() {
                log::warn!(target: LOG, "No metadata parsed!");
            } else {
                log::warn!(target: LOG, "Metadata missing {missed:?}");
            }
            if missed.len() <= TOLERATED_COUNT {
                log::warn!(target: LOG, "Missing meta tolerated, missing count < {TOLERATED_COUNT}");
            }
        }

        missed.len() <= TOLERATED_COUNT
    }

    /// Try to read info from given byte line
    pub fn process_line(&mut self, line: &[u8]) -> Result<(), PyErr> {
        if line.starts_with(b";") {
            self.from_comment_line(&decode_utf8(line)?)
        } else {
            if self.percent_of_m73_data() == 100.0 {
                return Ok(());
            }
            if self.m73_searched_bytes > MAX_M73_SEARCH_BYTES {
                return Ok(());
            }
            self.from_gcode_line(&decode_utf8(line)?)?;
            self.m73_searched_bytes += line.len();
            Ok(())
        }
    }

    /// Finds out if the current position contains metadata
    pub fn metadata_area(position: u64, size: u64) -> bool {
        let close_to_start = position < METADATA_START_OFFSET;
        let close_to_end = (position as i128) > size as i128 - METADATA_END_OFFSET as i128;
        close_to_start || close_to_end
    }

    /// Parse metadata on the start and end of the file
    pub fn quick_parse(&mut self, file: File) -> Result<(), MetaError> {
        let mut file = BufReader::new(file);
        let mut position: u64 = 0;
        let size = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;
        while position != size {
            if !Self::metadata_area(position, size) {
                // Skip the middle part of the file
                position = size - METADATA_END_OFFSET;
                file.seek(SeekFrom::Start(position))?;
            }
            let mut line = Vec::new();
            file.read_until(b'\n', &mut line)?;
            position += line.len() as u64;
            self.process_line(&line)?;
        }
        Ok(())
    }

    fn fdm_load_from_chunk(&mut self, data: &[u8], size: u64) -> Result<(), MetaError> {
        let metadata_area = Self::metadata_area(self.position, size);
        self.position += data.len() as u64;
        let mut data = data;
        if !metadata_area {
            let metadata_end_offset_position = size - METADATA_END_OFFSET;
            // end offset not in chunk data
            if metadata_end_offset_position > self.position {
                self.chunk_buffer = Vec::new();
                return Ok(());
            }
            // `data[:metadata_end_offset_position]`: an absolute position, as the reference
            let cut = (metadata_end_offset_position as usize).min(data.len());
            data = &data[..cut];
        }
        let mut joined = std::mem::take(&mut self.chunk_buffer);
        joined.extend_from_slice(data);
        let mut lines: Vec<&[u8]> = joined.split(|b| *b == b'\n').collect();
        // last line was cut in middle, save it to buffer to process it with next chunk of data
        let buffer = if !joined.ends_with(b"\n") {
            lines.pop().unwrap_or_default().to_vec()
        } else {
            Vec::new()
        };
        for line in lines {
            self.process_line(line)?;
        }
        self.chunk_buffer = buffer;
        Ok(())
    }

    /// Report what percentage of M73 attributes has been found
    pub fn percent_of_m73_data(&self) -> f64 {
        let count = M73_ATTRS.len();
        let present = M73_ATTRS
            .iter()
            .filter(|(_, attribute)| self.data.contains(attribute))
            .count();
        (present as f64 / count as f64) * 100.0
    }
}

impl std::fmt::Display for MetaData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Metadata: {}, {} items, {} thumbnails",
            self.path,
            self.data.len(),
            self.thumbnails.len()
        )
    }
}

impl PyValue {
    fn from_option_str(value: &Option<String>) -> PyValue {
        match value {
            Some(s) => PyValue::Str(s.clone()),
            None => PyValue::None,
        }
    }
}

/// `line.decode("UTF-8")`
fn decode_utf8(line: &[u8]) -> Result<String, PyErr> {
    std::str::from_utf8(line)
        .map(str::to_owned)
        .map_err(|e| PyErr::ValueError(format!("'utf-8' codec can't decode bytes: {e}")))
}

/// Returns the Metadata for given `path`. `filename`: in case of a temp file, as the path
/// differs from the name of the temp file and the meta class is decided by the extension.
pub fn get_metadata(
    path: &str,
    save_cache: bool,
    filename: Option<&str>,
) -> Result<MetaData, MetaError> {
    let mut metadata = get_meta_class(path, filename)?;
    metadata.load(save_cache)?;
    Ok(metadata)
}

/// Returns the Metadata class based on given filename or path.
pub fn get_meta_class(path: &str, filename: Option<&str>) -> Result<MetaData, MetaError> {
    let fnl = match filename {
        Some(name) if !name.is_empty() => py_lower(name),
        _ => py_lower(path),
    };
    if GCODE_EXTENSIONS.iter().any(|ext| fnl.ends_with(ext)) {
        Ok(MetaData::fdm(path))
    } else {
        Err(MetaError::UnknownGcodeFileType(path.to_owned()))
    }
}

/// Get the image with the closest resolution and aspect ratio to the target. The weight of
/// aspect ratio to resolution proximity can be tweaked
pub fn get_closest_image(
    thumbnails: &Thumbnails,
    target: &ImageInfo,
    aspect_ratio_weight: f64,
) -> Result<Option<ImageInfo>, PyErr> {
    let mut valid_thumbnails: Vec<ImageInfo> = Vec::new();
    let fifty = PyInt::from_i64(50);
    for thumbnail in thumbnails.keys() {
        let info = ImageInfo::from_thumbnail_info(thumbnail)?;
        if IMAGE_FORMATS.contains(&info.format.as_str())
            && int_ge(&info.width, &fifty)
            && int_ge(&info.height, &fifty)
        {
            valid_thumbnails.push(info);
        }
    }

    if valid_thumbnails.is_empty() {
        return Ok(None);
    }

    // `sorted` is stable
    let keyed: Vec<(f64, ImageInfo)> = valid_thumbnails
        .into_iter()
        .map(|info| (info.badness(target, aspect_ratio_weight), info))
        .collect();
    let mut sorted = keyed;
    sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(sorted.into_iter().next().map(|(_, info)| info))
}

/// `a >= b` of two `int`s
fn int_ge(a: &PyInt, b: &PyInt) -> bool {
    let (sa, sb) = (a.as_str(), b.as_str());
    let (na, nb) = (sa.starts_with('-'), sb.starts_with('-'));
    match (na, nb) {
        (false, true) => true,
        (true, false) => false,
        (false, false) => (sa.len(), sa) >= (sb.len(), sb),
        (true, true) => (sa.len(), sa) <= (sb.len(), sb),
    }
}

/// Get the preview with the biggest resolution from the list of thumbnails
pub fn get_preview(thumbnails: &Thumbnails) -> Result<Option<ImageInfo>, PyErr> {
    get_closest_image(thumbnails, &ImageInfo::new(640, 480, "PNG"), 1.0)
}

/// Get the icon which suits best according given parameters
pub fn get_icon(thumbnails: &Thumbnails) -> Result<Option<ImageInfo>, PyErr> {
    get_closest_image(thumbnails, &ImageInfo::new(100, 100, "PNG"), 1.0)
}
