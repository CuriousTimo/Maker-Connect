//! Configuration file parser (`configparser.py` of CPython 3.13.12), the part
//! `camera_configurator.py` and PrusaLink's `config.py` stand on.
//!
//! Reference: CPython 3.13.12, `Lib/configparser.py` (`reference/cpython-3.13/Lib/`), ported
//! function by function. Two parsers, with the other options at their defaults (delimiters
//! `=` and `:`, comment prefixes `#` and `;` on their own line only, `strict=True`,
//! `empty_lines_in_values=True`, `allow_no_value=False`, `default_section="DEFAULT"`):
//! - [`ConfigParser::new`]: `ConfigParser(interpolation=None)`, as PrusaLink's `Settings`
//!   (`prusa/link/config.py`, `Settings.__init__`: `super().__init__(interpolation=None)`) and the
//!   camera configurator it is given;
//! - [`ConfigParser::with_basic_interpolation`]: `ConfigParser()`, its `BasicInterpolation`
//!   (`%(name)s`, `%%`), as PrusaLink's `Config` (`Config.__init__`: `super().__init__()`).
//!
//! The mapping protocol only as far as `config.py` uses it: `key in parser`
//! ([`ConfigParser::contains`]) and `parser[section].items()` ([`ConfigParser::section_items`]).
//!
//! **Not covered:** the other options, the rest of the mapping protocol, `getint`, `getfloat`
//! and the other converters, and `vars=`. `fallback=` is the caller's: it matches the
//! `NoSectionError` and `NoOptionError` classes. Values are `str` only: `ConfigParser.set` raises
//! `TypeError` for anything else, and `read_dict` takes strings here. A file is read as UTF-8
//! (`io.text_encoding(None)` is the locale's encoding, UTF-8 on the agent's systems).

use std::path::Path;

use maker_connect_gcode_metadata::py::{isspace, repr_str, strip};

pub const DEFAULTSECT: &str = "DEFAULT";

/// `MAX_INTERPOLATION_DEPTH`
pub const MAX_INTERPOLATION_DEPTH: usize = 10;

/// `RawConfigParser.BOOLEAN_STATES`
pub const BOOLEAN_STATES: [(&str, bool); 8] = [
    ("1", true),
    ("yes", true),
    ("true", true),
    ("on", true),
    ("0", false),
    ("no", false),
    ("false", false),
    ("off", false),
];

/// The parser's interpolation: `Interpolation` (the dummy, `interpolation=None` gives it too) or
/// `BasicInterpolation` (`ConfigParser._DEFAULT_INTERPOLATION`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Interpolation {
    #[default]
    None,
    Basic,
}

/// A `configparser` exception (`NoSectionError`, `DuplicateSectionError`, ...) or another one
/// it raises (`ValueError`, `UnicodeDecodeError`, `OSError`): its class and `str()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub class: &'static str,
    pub message: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.class, self.message)
    }
}

impl std::error::Error for Error {}

fn no_section_error(section: &str) -> Error {
    Error {
        class: "NoSectionError",
        message: format!("No section: {}", repr_str(section)),
    }
}

fn duplicate_section_error(section: &str, source: Option<&str>, lineno: Option<usize>) -> Error {
    let msg = format!("{} already exists", repr_str(section));
    let message = match source {
        Some(source) => {
            let mut message = format!("While reading from {}", repr_str(source));
            if let Some(lineno) = lineno {
                message.push_str(&format!(" [line {lineno:2}]"));
            }
            format!("{message}: section {msg}")
        }
        None => format!("Section {msg}"),
    };
    Error {
        class: "DuplicateSectionError",
        message,
    }
}

fn duplicate_option_error(
    section: &str,
    option: &str,
    source: Option<&str>,
    lineno: Option<usize>,
) -> Error {
    let msg = format!(
        "{} in section {} already exists",
        repr_str(option),
        repr_str(section)
    );
    let message = match source {
        Some(source) => {
            let mut message = format!("While reading from {}", repr_str(source));
            if let Some(lineno) = lineno {
                message.push_str(&format!(" [line {lineno:2}]"));
            }
            format!("{message}: option {msg}")
        }
        None => format!("Option {msg}"),
    };
    Error {
        class: "DuplicateOptionError",
        message,
    }
}

fn no_option_error(option: &str, section: &str) -> Error {
    Error {
        class: "NoOptionError",
        message: format!(
            "No option {} in section: {}",
            repr_str(option),
            repr_str(section)
        ),
    }
}

/// `ParsingError(source)` with its `(lineno, line)` errors appended.
fn parsing_error(source: &str, errors: &[(usize, String)]) -> Error {
    let mut message = format!("Source contains parsing errors: {}", repr_str(source));
    for (lineno, line) in errors {
        message.push_str(&format!("\n\t[line {lineno:2}]: {}", repr_str(line)));
    }
    Error {
        class: "ParsingError",
        message,
    }
}

/// `InterpolationMissingOptionError(option, section, rawval, reference)`
fn interpolation_missing_option_error(
    option: &str,
    section: &str,
    rawval: &str,
    reference: &str,
) -> Error {
    Error {
        class: "InterpolationMissingOptionError",
        message: format!(
            "Bad value substitution: option {} in section {} contains an interpolation key {} \
             which is not a valid option name. Raw value: {}",
            repr_str(option),
            repr_str(section),
            repr_str(reference),
            repr_str(rawval)
        ),
    }
}

/// `InterpolationSyntaxError(option, section, msg)`
fn interpolation_syntax_error(message: String) -> Error {
    Error {
        class: "InterpolationSyntaxError",
        message,
    }
}

/// `InterpolationDepthError(option, section, rawval)`
fn interpolation_depth_error(option: &str, section: &str, rawval: &str) -> Error {
    Error {
        class: "InterpolationDepthError",
        message: format!(
            "Recursion limit exceeded in value substitution: option {} in section {} contains \
             an interpolation key which cannot be substituted in {MAX_INTERPOLATION_DEPTH} \
             steps. Raw value: {}",
            repr_str(option),
            repr_str(section),
            repr_str(rawval)
        ),
    }
}

/// `BasicInterpolation._KEYCRE.match(rest)`: `%\(([^)]+)\)s` at the start of `rest`; the name
/// and the length matched.
fn keycre_match(rest: &str) -> Option<(&str, usize)> {
    let inner = rest.strip_prefix("%(")?;
    let close = inner.find(')')?;
    if close == 0 || !inner[close + 1..].starts_with('s') {
        return None;
    }
    Some((&inner[..close], 2 + close + 2))
}

/// `BasicInterpolation._KEYCRE.sub('', value)`
fn keycre_sub_empty(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(c) = rest.chars().next() {
        if let Some((_, len)) = keycre_match(rest) {
            rest = &rest[len..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

fn missing_section_header_error(filename: &str, lineno: usize, line: &str) -> Error {
    Error {
        class: "MissingSectionHeaderError",
        message: format!(
            "File contains no section headers.\nfile: {}, line: {lineno}\n{}",
            repr_str(filename),
            repr_str(line)
        ),
    }
}

/// An option's value: a string, or the lines of one while a file is read.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    Text(String),
    Lines(Vec<String>),
}

impl Entry {
    fn text(&self) -> &str {
        match self {
            Entry::Text(text) => text,
            Entry::Lines(_) => unreachable!("lines are joined at the end of a read"),
        }
    }
}

/// A Python `dict` of options, in insertion order.
type Options = Vec<(String, Entry)>;

fn options_set(options: &mut Options, key: &str, value: Entry) {
    match options.iter_mut().find(|(k, _)| k == key) {
        Some(entry) => entry.1 = value,
        None => options.push((key.to_owned(), value)),
    }
}

fn options_get<'a>(options: &'a Options, key: &str) -> Option<&'a Entry> {
    options.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// Where the read's current section is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cursect {
    Defaults,
    Section(usize),
}

/// `_ReadState`
#[derive(Default)]
struct ReadState {
    /// `elements_added`: sections, and (section, option) pairs
    sections_added: Vec<String>,
    options_added: Vec<(String, String)>,
    cursect: Option<Cursect>,
    sectname: Option<String>,
    optname: Option<String>,
    lineno: usize,
    indent_level: usize,
    cur_indent_level: usize,
    errors: Vec<(usize, String)>,
}

/// `_Line.clean`: `''` for a comment line, else the stripped line.
fn clean(line: &str) -> &str {
    let stripped = strip(line);
    if stripped.starts_with('#') || stripped.starts_with(';') {
        ""
    } else {
        stripped
    }
}

/// `_Line.has_comments`
fn has_comments(line: &str) -> bool {
    strip(line) != clean(line)
}

/// `ConfigParser(interpolation=None)`, or `ConfigParser()` with its `BasicInterpolation`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigParser {
    sections: Vec<(String, Options)>,
    defaults: Options,
    interpolation: Interpolation,
}

impl ConfigParser {
    /// `ConfigParser(interpolation=None)`
    pub fn new() -> ConfigParser {
        ConfigParser::default()
    }

    /// `ConfigParser()`: `BasicInterpolation`
    pub fn with_basic_interpolation() -> ConfigParser {
        ConfigParser {
            interpolation: Interpolation::Basic,
            ..ConfigParser::default()
        }
    }

    fn section_index(&self, section: &str) -> Option<usize> {
        self.sections.iter().position(|(name, _)| name == section)
    }

    /// `optionxform`: `str.lower()`
    pub fn optionxform(optionstr: &str) -> String {
        optionstr.to_lowercase()
    }

    /// The defaults (the `[DEFAULT]` section)
    pub fn defaults(&self) -> Vec<(String, String)> {
        self.defaults
            .iter()
            .map(|(k, v)| (k.clone(), v.text().to_owned()))
            .collect()
    }

    /// Return a list of section names, excluding `[DEFAULT]`
    pub fn sections(&self) -> Vec<String> {
        self.sections.iter().map(|(name, _)| name.clone()).collect()
    }

    /// Create a new section in the configuration. Raise DuplicateSectionError if a section by
    /// the specified name already exists. Raise ValueError if name is DEFAULT.
    pub fn add_section(&mut self, section: &str) -> Result<(), Error> {
        if section == DEFAULTSECT {
            return Err(Error {
                class: "ValueError",
                message: format!("Invalid section name: {}", repr_str(section)),
            });
        }
        if self.section_index(section).is_some() {
            return Err(duplicate_section_error(section, None, None));
        }
        self.sections.push((section.to_owned(), Options::new()));
        Ok(())
    }

    /// Indicate whether the named section is present in the configuration. The DEFAULT
    /// section is not acknowledged.
    pub fn has_section(&self, section: &str) -> bool {
        self.section_index(section).is_some()
    }

    /// Return a list of option names for the given section name.
    pub fn options(&self, section: &str) -> Result<Vec<String>, Error> {
        let index = self
            .section_index(section)
            .ok_or_else(|| no_section_error(section))?;
        let mut opts: Vec<String> = self.sections[index]
            .1
            .iter()
            .map(|(k, _)| k.clone())
            .collect();
        for (key, _) in &self.defaults {
            if !opts.contains(key) {
                opts.push(key.clone());
            }
        }
        Ok(opts)
    }

    /// Read and parse a filename. A file that cannot be opened is silently ignored. Return
    /// whether it was read.
    pub fn read(&mut self, filename: &Path) -> Result<bool, Error> {
        let bytes = match std::fs::read(filename) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(false),
        };
        let text = String::from_utf8(bytes).map_err(|err| Error {
            class: "UnicodeDecodeError",
            message: format!(
                "'utf-8' codec can't decode byte 0x{:02x} in position {}: invalid start byte",
                err.as_bytes()[err.utf8_error().valid_up_to()],
                err.utf8_error().valid_up_to()
            ),
        })?;
        // universal newlines
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        self.read_lines(&text, &filename.to_string_lossy())?;
        Ok(true)
    }

    /// Read configuration from a given string (`io.StringIO`: lines end at `\n` only).
    pub fn read_string(&mut self, string: &str, source: &str) -> Result<(), Error> {
        self.read_lines(string, source)
    }

    /// `_read`
    fn read_lines(&mut self, text: &str, fpname: &str) -> Result<(), Error> {
        let result = self.read_inner(text, fpname);
        self.join_multiline_values();
        let errors = result?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(parsing_error(fpname, &errors))
        }
    }

    fn cursect_mut(&mut self, cursect: Cursect) -> &mut Options {
        match cursect {
            Cursect::Defaults => &mut self.defaults,
            Cursect::Section(index) => &mut self.sections[index].1,
        }
    }

    fn read_inner(&mut self, text: &str, fpname: &str) -> Result<Vec<(usize, String)>, Error> {
        let mut st = ReadState::default();

        for (index, line) in text.split_inclusive('\n').enumerate() {
            st.lineno = index + 1;
            if clean(line).is_empty() {
                // empty_lines_in_values: add empty line to the value, but only if there was no
                // comment on the line
                if !has_comments(line)
                    && let (Some(cursect), Some(optname)) = (st.cursect, st.optname.clone())
                    && !optname.is_empty()
                    && let Some(Entry::Lines(lines)) = self
                        .cursect_mut(cursect)
                        .iter_mut()
                        .find(|(k, _)| *k == optname)
                        .map(|(_, v)| v)
                {
                    lines.push(String::new()); // newlines added at join
                }
                continue;
            }

            st.cur_indent_level = line.chars().position(|c| !isspace(c)).unwrap_or(0);

            if self.handle_continuation_line(&mut st, line) {
                continue;
            }

            self.handle_rest(&mut st, line, fpname)?;
        }

        Ok(st.errors)
    }

    fn handle_continuation_line(&mut self, st: &mut ReadState, line: &str) -> bool {
        // continuation line?
        let (Some(cursect), Some(optname)) = (st.cursect, st.optname.clone()) else {
            return false;
        };
        let is_continue = !optname.is_empty() && st.cur_indent_level > st.indent_level;
        if is_continue {
            // allow_no_value=False: the value is never None, so no MultilineContinuationError
            if let Some(Entry::Lines(lines)) = self
                .cursect_mut(cursect)
                .iter_mut()
                .find(|(k, _)| *k == optname)
                .map(|(_, v)| v)
            {
                lines.push(clean(line).to_owned());
            }
        }
        is_continue
    }

    fn handle_rest(&mut self, st: &mut ReadState, line: &str, fpname: &str) -> Result<(), Error> {
        st.indent_level = st.cur_indent_level;
        // is it a section header?
        let header = section_header(clean(line));

        if header.is_none() && st.cursect.is_none() {
            return Err(missing_section_header_error(fpname, st.lineno, line));
        }

        match header {
            Some(sectname) => self.handle_header(st, sectname, fpname),
            None => self.handle_option(st, line, fpname),
        }
    }

    fn handle_header(
        &mut self,
        st: &mut ReadState,
        sectname: &str,
        fpname: &str,
    ) -> Result<(), Error> {
        st.sectname = Some(sectname.to_owned());
        if let Some(index) = self.section_index(sectname) {
            if st.sections_added.iter().any(|s| s == sectname) {
                return Err(duplicate_section_error(
                    sectname,
                    Some(fpname),
                    Some(st.lineno),
                ));
            }
            st.cursect = Some(Cursect::Section(index));
            st.sections_added.push(sectname.to_owned());
        } else if sectname == DEFAULTSECT {
            st.cursect = Some(Cursect::Defaults);
        } else {
            self.sections.push((sectname.to_owned(), Options::new()));
            st.cursect = Some(Cursect::Section(self.sections.len() - 1));
            st.sections_added.push(sectname.to_owned());
        }
        // So sections can't start with a continuation line
        st.optname = None;
        Ok(())
    }

    fn handle_option(&mut self, st: &mut ReadState, line: &str, fpname: &str) -> Result<(), Error> {
        // an option line?
        st.indent_level = st.cur_indent_level;

        let Some((option, value)) = option_line(clean(line)) else {
            // a non-fatal parsing error occurred. set up the exception but keep going. the
            // exception will be raised at the end of the file and will contain a list of all
            // bogus lines
            st.errors.push((st.lineno, line.to_owned()));
            return Ok(());
        };

        if option.is_empty() {
            st.errors.push((st.lineno, line.to_owned()));
        }
        let optname = ConfigParser::optionxform(option.trim_end_matches(isspace));
        let sectname = st.sectname.clone().unwrap_or_default();
        if st
            .options_added
            .iter()
            .any(|(s, o)| *s == sectname && *o == optname)
        {
            return Err(duplicate_option_error(
                &sectname,
                &optname,
                Some(fpname),
                Some(st.lineno),
            ));
        }
        st.options_added.push((sectname, optname.clone()));
        let cursect = st.cursect.expect("an option comes after a section header");
        options_set(
            self.cursect_mut(cursect),
            &optname,
            Entry::Lines(vec![strip(value).to_owned()]),
        );
        st.optname = Some(optname);
        Ok(())
    }

    /// `_join_multiline_values`
    fn join_multiline_values(&mut self) {
        let all_sections = std::iter::once(&mut self.defaults)
            .chain(self.sections.iter_mut().map(|(_, options)| options));
        for options in all_sections {
            for (_, value) in options.iter_mut() {
                if let Entry::Lines(lines) = value {
                    let joined = lines.join("\n");
                    *value = Entry::Text(joined.trim_end_matches(isspace).to_owned());
                }
            }
        }
    }

    /// Read configuration from a dictionary: sections and their keys are added in order.
    pub fn read_dict(
        &mut self,
        dictionary: &[(String, Vec<(String, String)>)],
        source: &str,
    ) -> Result<(), Error> {
        let mut sections_added: Vec<String> = Vec::new();
        let mut options_added: Vec<(String, String)> = Vec::new();
        for (section, keys) in dictionary {
            if let Err(err) = self.add_section(section)
                && sections_added.contains(section)
            {
                return Err(err);
            }
            sections_added.push(section.clone());
            for (key, value) in keys {
                let key = ConfigParser::optionxform(key);
                if options_added.iter().any(|(s, o)| s == section && *o == key) {
                    return Err(duplicate_option_error(section, &key, Some(source), None));
                }
                options_added.push((section.clone(), key.clone()));
                self.set(section, &key, value)?;
            }
        }
        Ok(())
    }

    /// Get an option value for a given section. The option is looked up in `section`, then in
    /// `DEFAULTSECT`. With `BasicInterpolation` its interpolations are expanded.
    pub fn get(&self, section: &str, option: &str) -> Result<String, Error> {
        let value = self.get_raw(section, option)?;
        self.before_get(section, &ConfigParser::optionxform(option), &value)
    }

    /// `getboolean(section, option)`: `_convert_to_boolean` of the value `get` gives.
    pub fn getboolean(&self, section: &str, option: &str) -> Result<bool, Error> {
        let value = self.get(section, option)?;
        let lower = value.to_lowercase();
        BOOLEAN_STATES
            .iter()
            .find(|(state, _)| *state == lower)
            .map(|(_, b)| *b)
            .ok_or_else(|| Error {
                class: "ValueError",
                message: format!("Not a boolean: {value}"),
            })
    }

    /// `key in parser` (`RawConfigParser.__contains__`): the default section, or a section.
    pub fn contains(&self, key: &str) -> bool {
        key == DEFAULTSECT || self.has_section(key)
    }

    /// `parser[section].items()`: the `SectionProxy`'s keys (`_options`: its options with the
    /// defaults', or the defaults' for `DEFAULT`), each with `parser.get(section, key)`. A section
    /// that is not there is `KeyError(section)` (`__getitem__`).
    pub fn section_items(&self, section: &str) -> Result<Vec<(String, String)>, Error> {
        if section != DEFAULTSECT && !self.has_section(section) {
            return Err(Error {
                class: "KeyError",
                message: repr_str(section),
            });
        }
        let keys = if section != DEFAULTSECT {
            self.options(section)?
        } else {
            self.defaults().into_iter().map(|(k, _)| k).collect()
        };
        keys.into_iter()
            .map(|key| {
                let value = self.get(section, &key)?;
                Ok((key, value))
            })
            .collect()
    }

    /// The value of `option` (already `optionxform`ed) in `section`'s `ChainMap` with the
    /// defaults (`_unify_values`): the `map` interpolation looks names up in.
    fn lookup(&self, section: &str, option: &str) -> Option<String> {
        self.section_index(section)
            .and_then(|index| options_get(&self.sections[index].1, option))
            .or_else(|| options_get(&self.defaults, option))
            .map(|value| value.text().to_owned())
    }

    /// `self._interpolation.before_get(self, section, option, value, d)`
    fn before_get(&self, section: &str, option: &str, value: &str) -> Result<String, Error> {
        match self.interpolation {
            Interpolation::None => Ok(value.to_owned()),
            Interpolation::Basic => {
                let mut accum = String::new();
                self.interpolate_some(option, &mut accum, value, section, 1)?;
                Ok(accum)
            }
        }
    }

    /// `BasicInterpolation._interpolate_some`
    fn interpolate_some(
        &self,
        option: &str,
        accum: &mut String,
        rest: &str,
        section: &str,
        depth: usize,
    ) -> Result<(), Error> {
        // `parser.get(section, option, raw=True, fallback=rest)`
        let rawval = self
            .get_raw(section, option)
            .unwrap_or_else(|_| rest.to_owned());
        if depth > MAX_INTERPOLATION_DEPTH {
            return Err(interpolation_depth_error(option, section, &rawval));
        }
        let mut rest = rest;
        while !rest.is_empty() {
            let Some(p) = rest.find('%') else {
                accum.push_str(rest);
                return Ok(());
            };
            if p > 0 {
                accum.push_str(&rest[..p]);
                rest = &rest[p..];
            }
            let c = rest[1..].chars().next();
            if c == Some('%') {
                accum.push('%');
                rest = &rest[2..];
            } else if c == Some('(') {
                let Some((name, len)) = keycre_match(rest) else {
                    return Err(interpolation_syntax_error(format!(
                        "bad interpolation variable reference {}",
                        repr_str(rest)
                    )));
                };
                let var = ConfigParser::optionxform(name);
                rest = &rest[len..];
                let Some(v) = self.lookup(section, &var) else {
                    return Err(interpolation_missing_option_error(
                        option, section, &rawval, &var,
                    ));
                };
                if v.contains('%') {
                    self.interpolate_some(option, accum, &v, section, depth + 1)?;
                } else {
                    accum.push_str(&v);
                }
            } else {
                return Err(interpolation_syntax_error(format!(
                    "'%' must be followed by '%' or '(', found: {}",
                    repr_str(rest)
                )));
            }
        }
        Ok(())
    }

    /// `self._interpolation.before_set(self, section, option, value)`
    fn before_set(&self, value: &str) -> Result<(), Error> {
        if self.interpolation == Interpolation::None {
            return Ok(());
        }
        let tmp_value = value.replace("%%", ""); // escaped percent signs
        let tmp_value = keycre_sub_empty(&tmp_value); // valid syntax
        if let Some(at) = tmp_value.find('%') {
            return Err(Error {
                class: "ValueError",
                message: format!(
                    "invalid interpolation syntax in {} at position {}",
                    repr_str(value),
                    tmp_value[..at].chars().count()
                ),
            });
        }
        Ok(())
    }

    /// `get(section, option, raw=True)`
    pub fn get_raw(&self, section: &str, option: &str) -> Result<String, Error> {
        let sectiondict = match self.section_index(section) {
            Some(index) => Some(&self.sections[index].1),
            None if section == DEFAULTSECT => None,
            None => return Err(no_section_error(section)),
        };
        let option = ConfigParser::optionxform(option);
        sectiondict
            .and_then(|options| options_get(options, &option))
            .or_else(|| options_get(&self.defaults, &option))
            .map(|value| value.text().to_owned())
            .ok_or_else(|| no_option_error(&option, section))
    }

    /// Return a list of (name, value) tuples for each option in a section.
    pub fn items(&self, section: &str) -> Result<Vec<(String, String)>, Error> {
        let mut d = self.defaults.clone();
        match self.section_index(section) {
            Some(index) => {
                for (key, value) in &self.sections[index].1 {
                    options_set(&mut d, key, value.clone());
                }
            }
            None if section == DEFAULTSECT => {}
            None => return Err(no_section_error(section)),
        }
        Ok(d.into_iter()
            .map(|(k, v)| (k, v.text().to_owned()))
            .collect())
    }

    /// Check for the existence of a given option in a given section. If the specified
    /// `section` is an empty string, DEFAULT is assumed. If the specified `section` does not
    /// exist, returns False.
    pub fn has_option(&self, section: &str, option: &str) -> bool {
        let option = ConfigParser::optionxform(option);
        if section.is_empty() || section == DEFAULTSECT {
            return options_get(&self.defaults, &option).is_some();
        }
        match self.section_index(section) {
            None => false,
            Some(index) => {
                options_get(&self.sections[index].1, &option).is_some()
                    || options_get(&self.defaults, &option).is_some()
            }
        }
    }

    /// Set an option (`ConfigParser.set`: its interpolation's `before_set` checks a value that is
    /// not empty).
    pub fn set(&mut self, section: &str, option: &str, value: &str) -> Result<(), Error> {
        if !value.is_empty() {
            self.before_set(value)?;
        }
        let sectdict = if section.is_empty() || section == DEFAULTSECT {
            &mut self.defaults
        } else {
            match self.section_index(section) {
                Some(index) => &mut self.sections[index].1,
                None => return Err(no_section_error(section)),
            }
        };
        options_set(
            sectdict,
            &ConfigParser::optionxform(option),
            Entry::Text(value.to_owned()),
        );
        Ok(())
    }

    /// Write an .ini-format representation of the configuration state, with spaces around the
    /// delimiter.
    pub fn write(&self) -> String {
        let mut fp = String::new();
        let d = " = ";
        if !self.defaults.is_empty() {
            write_section(&mut fp, DEFAULTSECT, &self.defaults, d);
        }
        for (section, options) in &self.sections {
            write_section(&mut fp, section, options, d);
        }
        fp
    }

    /// `with open(path, 'w', encoding='utf-8') as f: parser.write(f)`: text mode, so each
    /// `\n` is the platform's line separator.
    pub fn write_file(&self, path: &Path) -> Result<(), Error> {
        let mut text = self.write();
        if cfg!(windows) {
            text = text.replace('\n', "\r\n");
        }
        std::fs::write(path, text).map_err(|err| Error {
            class: "OSError",
            message: err.to_string(),
        })
    }

    /// Remove an option.
    pub fn remove_option(&mut self, section: &str, option: &str) -> Result<bool, Error> {
        let sectdict = if section.is_empty() || section == DEFAULTSECT {
            &mut self.defaults
        } else {
            match self.section_index(section) {
                Some(index) => &mut self.sections[index].1,
                None => return Err(no_section_error(section)),
            }
        };
        let option = ConfigParser::optionxform(option);
        let before = sectdict.len();
        sectdict.retain(|(k, _)| *k != option);
        Ok(sectdict.len() != before)
    }

    /// Remove a file section.
    pub fn remove_section(&mut self, section: &str) -> bool {
        match self.section_index(section) {
            Some(index) => {
                self.sections.remove(index);
                true
            }
            None => false,
        }
    }
}

/// Write a single section.
fn write_section(fp: &mut String, section_name: &str, section_items: &Options, delimiter: &str) {
    fp.push_str(&format!("[{section_name}]\n"));
    for (key, value) in section_items {
        let value = format!("{delimiter}{}", value.text().replace('\n', "\n\t"));
        fp.push_str(&format!("{key}{value}\n"));
    }
    fp.push('\n');
}

/// `SECTCRE`: `\[(?P<header>.+)\]` matched at the start: the header runs to the last `]`.
fn section_header(clean: &str) -> Option<&str> {
    let rest = clean.strip_prefix('[')?;
    let line = rest.split('\n').next().unwrap_or("");
    let end = line.rfind(']')?;
    if end == 0 {
        return None;
    }
    Some(&line[..end])
}

/// `OPTCRE`: `(?P<option>.*?)\s*(?P<vi>=|:)\s*(?P<value>.*)$`: the option before the first
/// delimiter, the value after it; `None` without a delimiter.
fn option_line(clean: &str) -> Option<(&str, &str)> {
    let at = clean.find(['=', ':'])?;
    let option = clean[..at].trim_end_matches(isspace);
    let value = clean[at + 1..].trim_start_matches(isspace);
    Some((option, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes() {
        let mut config = ConfigParser::new();
        config
            .read_string(
                "# comment\n[camera_order]\n1 = abc\n2: def\n\n[camera::abc]\nName = One\n  two\n\n  three\n[empty]\n",
                "<string>",
            )
            .unwrap();
        assert_eq!(
            config.sections(),
            vec!["camera_order", "camera::abc", "empty"]
        );
        assert_eq!(config.options("camera_order").unwrap(), vec!["1", "2"]);
        assert_eq!(
            config.get("camera::abc", "NAME").unwrap(),
            "One\ntwo\n\nthree"
        );
        assert_eq!(
            config.write(),
            "[camera_order]\n1 = abc\n2 = def\n\n[camera::abc]\nname = One\n\ttwo\n\t\n\tthree\n\n[empty]\n\n"
        );
    }

    #[test]
    fn errors_are_pythons() {
        let mut config = ConfigParser::new();
        assert_eq!(
            config
                .read_string("a = 1\n", "<string>")
                .unwrap_err()
                .message,
            "File contains no section headers.\nfile: '<string>', line: 1\n'a = 1\\n'"
        );
        let err = config
            .read_string("[s]\nbad\n= x\n", "<string>")
            .unwrap_err();
        assert_eq!(err.class, "ParsingError");
        assert_eq!(
            err.message,
            "Source contains parsing errors: '<string>'\n\t[line  2]: 'bad\\n'\n\t[line  3]: '= x\\n'"
        );
        let err = config.read_string("[t]\n[t]\n", "f.ini").unwrap_err();
        assert_eq!(
            err.message,
            "While reading from 'f.ini' [line  2]: section 't' already exists"
        );
        assert_eq!(
            config.add_section("t").unwrap_err().message,
            "Section 't' already exists"
        );
        assert_eq!(
            config.get("nope", "x").unwrap_err().message,
            "No section: 'nope'"
        );
    }
}
