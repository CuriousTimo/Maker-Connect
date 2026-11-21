//! Port of the reading side of OctoPrint 1.11.8 `src/octoprint/settings/__init__.py`, as
//! `util/comm.py` and `printer/standard.py` use it, with OctoPrint's own defaults and no user
//! configuration (no `config.yaml`, no overlays, no plugin settings).
//!
//! Original: Copyright (C) 2014 The OctoPrint Project, AGPL-3.0.
//!
//! The defaults are `default_settings` (`settings/__init__.py:94`), written by
//! `scripts/octoprint-settings.py` into `default_settings.json`. Values are `serde_json::Value`,
//! standing for the Python `None`, `bool`, `int`, `float`, `str`, `list` and `dict`.
//!
//! Not ported (not used by comm.py / standard.py, or outside "defaults only"): `get`'s `config`,
//! `defaults`, `preprocessors`, `incl_defaults`, `do_copy`, `validator`, `fallback` arguments;
//! `has`, `set*`, `save`, `add_overlay`, `getBaseFolder`, deprecated paths, and `loadScript(...,
//! source=True)` (only `server/api/settings.py:353` uses it).
//!
//! Script templating (`_init_script_templating` `:814`, `_get_script_template` `:944`,
//! `listScripts` `:2106`, `loadScript` `:2115`): Jinja2 is replaced by `minijinja` (owner's
//! approval, M5 in `docs/archive/plans/agent-engine.md`). `scripts_expected.json`, written by `scripts/octoprint-scripts-check.py` from the
//! real OctoPrint 1.11.8 `Settings`, is compared byte for byte in the tests. What differs:
//! - `FilteredFileSystemLoader(getBaseFolder("scripts"))` reads user script files from OctoPrint's
//!   basedir; we have defaults only and no basedir, so it finds nothing (no folder is invented).
//!   `file:name` is therefore never found, and `name` is found in the settings only.
//! - `SnippetExtension` (custom tag) is a rewrite of the tag into an `include` (see
//!   `snippet_extension` for each difference).
//! - `SandboxedEnvironment`: Jinja2's sandbox blocks unsafe attribute access (`__class__`, `mro`,
//!   frame/code attributes, `str.format`, mutating methods of `list`/`dict`/`set`) and
//!   limits `range()` to 100000 items (`MAX_RANGE`). minijinja has no Python objects to reach, so
//!   the first is moot; its `range()` has its own limit (100000 as well), and the context is data
//!   only (JSON values, or a minijinja map with int keys from `load_script_value`), so no callables
//!   can be passed in.
//! - Jinja2 vs minijinja in general: error messages differ (we only return `None`); filters, tests
//!   and global functions are minijinja's (most Jinja2 names exist, some behave differently, e.g.
//!   `|string`/`|tojson` formatting, float printing, `{{ 1/2 }}`); `{% include %}` of a missing
//!   template raises in both (`loadScript` gives `None`); minijinja loads a template once per
//!   environment, Jinja2 re-checks `uptodate` (moot without a config file); `jinja2`'s
//!   `{% extends %}`/`{% import %}` go through the same loader and `join_path` in both.
//! - Measured differences (the check's `KNOWN_DIFFER`; unreachable with the defaults, whose scripts
//!   use only `printer_profile` and the two snippets; they matter once user scripts exist):
//!   floats from 1e16 up and below 1e-4 (Python `1e+20`/`1e-07`, minijinja written out);
//!   `round` (Jinja2 uses Python's `round`: 1.25 -> 1.2, minijinja 1.3);
//!   a `{% snippet %}` inside `{% raw %}` is rewritten (Jinja2 leaves it).
//!   Int keys in the context (`as_script_dict`, comm.py:407: `last_temperature[0]`) are not a
//!   difference: `load_script_value` takes a minijinja map with int keys in insertion order (the
//!   `preserve_order` feature), which `_getGcodeScript` builds; `load_script` (JSON) has string keys
//!   only. The check script writes int keys as `"#int:<n>"` and the test turns them back.
//! - Python's `context` dict is updated in place; ours is not (see `load_script`).
//! - Not ported: `loadScript(source=True)` (only `server/api/settings.py:353` uses it);
//!   `_get_script_template(source=True)`.
//!
//! Where it differs from Python (none of it reachable with the defaults alone):
//! - `settings()` builds the singleton on first call; Python raises `ValueError` before `init=True`.
//! - `merged=True` changes nothing: with defaults only there is nothing to merge.
//! - The defaults file is dumped with `mode="json"`, so str-Enum defaults are plain strings; they
//!   compare equal to Python's Enum members. Objects keep Python's key order (serde_json
//!   `preserve_order`).
//! - A path through a list, string or number gives `NoSuchSettingsPath`; Python would test
//!   membership in it or raise `TypeError`.
//! - Python's uncaught `TypeError`/`OverflowError` in `getInt`/`getFloat` are panics; Python's
//!   unbounded `int` is `i64`.
//! - A path ending in a list of keys goes through `get_keys`; logger calls are comments.

use super::comm::{py_float, py_int};
use super::util::CaseInsensitiveSet;
use serde_json::{Map, Value, json};
use std::sync::{LazyLock, OnceLock};

/// `settings/__init__.py:102 NoSuchSettingsPath`
#[derive(Debug, Clone, PartialEq)]
pub struct NoSuchSettingsPath;

/// `settings/__init__.py:98 valid_boolean_trues`
///
/// Values that are considered to be equivalent to the boolean ``True`` value, used for type
/// conversion in various places.
#[allow(non_upper_case_globals)] // settings.py's own name, kept
pub static valid_boolean_trues: LazyLock<CaseInsensitiveSet> = LazyLock::new(|| {
    CaseInsensitiveSet::new(&[
        json!(true),
        json!("true"),
        json!("yes"),
        json!("y"),
        json!("1"),
        json!(1),
    ])
});

/// `settings/__init__.py:94 default_settings`: the file written by `scripts/octoprint-settings.py`.
const DEFAULT_SETTINGS_JSON: &str = include_str!("default_settings.json");

/// The keyword arguments of `Settings.get` that are ported: `error_on_path` (`:1963`),
/// `merged` and `asdict` (`_get_value`, `:1866`).
#[derive(Debug, Clone, Default)]
pub struct GetKwargs {
    pub error_on_path: bool,
    pub merged: bool,
    pub asdict: bool,
}

/// `settings/__init__.py:635 Settings`, defaults only.
pub struct Settings {
    /// `HierarchicalChainMap({}, default_settings)` (`:699`): with no config and no overlays the
    /// chain map holds only the (empty) top layer and the defaults, so a lookup is a lookup in the
    /// defaults.
    default_settings: Value,
}

static INSTANCE: OnceLock<Settings> = OnceLock::new();

/// `settings/__init__.py:54 settings`
///
/// The `Settings` singleton. Python's `settings(init=True)` constructs it and `settings()` raises
/// `ValueError("Settings not initialized yet")` before that; here the first call constructs it
/// (there is nothing to configure with defaults only).
pub fn settings() -> &'static Settings {
    INSTANCE.get_or_init(Settings::new)
}

impl Settings {
    /// `settings/__init__.py:94 default_settings` read from `default_settings.json`.
    pub fn new() -> Settings {
        #[allow(unused_mut)]
        let mut default_settings: Value = serde_json::from_str(DEFAULT_SETTINGS_JSON)
            .expect("default_settings.json is valid JSON");
        // OURS (test seam, `proof.rs`): the settings the proof against OctoPrint changes from the
        // defaults, the same on both sides (`scripts/octoprint-port-check.py`; OctoPrint's side reads
        // them from its `config.yaml`). Compiled only into the tests, and only read when a test sets it.
        #[cfg(test)]
        if let Ok(overlay) = std::env::var("MAKER_CONNECT_OCTOPRINT_PROOF_SETTINGS") {
            let overlay: Value =
                serde_json::from_str(&overlay).expect("the proof's settings are JSON");
            fn merge(into: &mut Value, from: &Value) {
                match (into, from) {
                    (Value::Object(a), Value::Object(b)) => {
                        for (k, v) in b {
                            merge(a.entry(k.clone()).or_insert(Value::Null), v);
                        }
                    }
                    (a, b) => *a = b.clone(),
                }
            }
            merge(&mut default_settings, &overlay);
        }
        Settings { default_settings }
    }

    /// Only for the tests: settings over other defaults.
    #[cfg(test)]
    pub(crate) fn from_defaults(default_settings: Value) -> Settings {
        Settings { default_settings }
    }

    /// Only for the tests: the defaults plus a script `scripts.gcode.<name>` (Python adds scripts
    /// with `add_overlay`, as `scripts/octoprint-scripts-check.py` does).
    #[cfg(test)]
    pub(crate) fn with_gcode_script(name: &str, source: &str) -> Settings {
        let mut settings = Settings::new();
        settings.default_settings["scripts"]["gcode"][name] = Value::String(source.to_string());
        settings
    }

    // ~~ internal getter

    /// `settings/__init__.py:1843 _get_by_path`, for the settings tree: `Err` is
    /// `NoSuchSettingsPath` (`HierarchicalChainMap.get_by_path`, `:355`, raises `KeyError`, which
    /// `_get_value` turns into `NoSuchSettingsPath`).
    fn _get_by_path(&self, path: &[&str]) -> Result<&Value, NoSuchSettingsPath> {
        let mut current = &self.default_settings;
        for key in path {
            match current {
                Value::Object(map) => match map.get(*key) {
                    Some(v) => current = v,
                    None => return Err(NoSuchSettingsPath),
                },
                _ => return Err(NoSuchSettingsPath),
            }
        }
        Ok(current)
    }

    /// `settings/__init__.py:949 _process_custom_controls`: the preprocessor of `["controls"]`
    /// (`:710`).
    fn _process_custom_controls(&self, controls: &Value) -> Value {
        fn process_control(c: &Value) -> Value {
            // shallow copy
            let mut result = match c {
                Value::Object(m) => m.clone(),
                _ => panic!("TypeError: cannot convert control to dict"),
            };

            fn truthy(v: Option<&Value>) -> bool {
                match v {
                    None | Some(Value::Null) => false,
                    Some(Value::Bool(b)) => *b,
                    Some(Value::Number(n)) => n.as_f64() != Some(0.0),
                    Some(Value::String(s)) => !s.is_empty(),
                    Some(Value::Array(a)) => !a.is_empty(),
                    Some(Value::Object(o)) => !o.is_empty(),
                }
            }
            fn md5_hex(v: &Value) -> String {
                use md5::{Digest, Md5};
                let s = v.as_str().expect("AttributeError: encode");
                let mut key_hash = Md5::new();
                key_hash.update(s.as_bytes());
                key_hash
                    .finalize()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect()
            }

            if truthy(result.get("regex")) && truthy(result.get("template")) {
                // if it's a template matcher, we need to add a key to associate with the matcher output
                let key = md5_hex(&result["regex"]);
                let template_key = md5_hex(&result["template"]);
                result.insert("key".to_string(), Value::String(key));
                result.insert("template_key".to_string(), Value::String(template_key));
            } else if result.contains_key("children") {
                // if it has children we need to process them recursively
                let children: Vec<Value> = result["children"]
                    .as_array()
                    .expect("TypeError: children is not iterable")
                    .iter()
                    .filter(|child| !child.is_null())
                    .map(process_control)
                    .collect();
                result.insert("children".to_string(), Value::Array(children));
            }

            Value::Object(result)
        }

        Value::Array(
            controls
                .as_array()
                .expect("TypeError: controls is not iterable")
                .iter()
                .map(process_control)
                .collect(),
        )
    }

    /// `settings/__init__.py:1866 _get_value` for `path` whose last element is a single key
    /// (`keys = [last]`) or a list of keys (`keys = last`). With `asdict` the result is an
    /// object keyed by the keys, otherwise an array (list-valued last element) or the single value.
    ///
    /// `merged` has no effect here: it merges the value with the defaults' value, and with defaults
    /// only the two are the same (`dict_merge(a, a) == a`).
    fn _get_value(
        &self,
        parent_path: &[&str],
        last: Result<&str, &[&str]>,
        kwargs: &GetKwargs,
    ) -> Result<Value, NoSuchSettingsPath> {
        // `if not path: raise NoSuchSettingsPath()` is in `get_with` (a path is never empty here)

        // is_deprecated = self._is_deprecated_path(path): there are no deprecated paths
        // (`self._logger.warning("DeprecationWarning: ...")` is never reached).

        // preprocessor = self._get_by_path(path, self._get_preprocessors), which is
        // {"controls": self._process_custom_controls}
        let use_controls_preprocessor = parent_path.is_empty() && last == Ok("controls");

        let keys: Vec<&str> = match last {
            Ok(k) => vec![k],
            Err(ks) => ks.to_vec(),
        };

        let mut results_dict: Map<String, Value> = Map::new();
        let mut results_list: Vec<Value> = Vec::new();

        for key in keys {
            let mut full: Vec<&str> = parent_path.to_vec();
            full.push(key);
            let value = self._get_by_path(&full)?;

            // value = self._preprocess_value(value, preprocessor)
            let value = if use_controls_preprocessor {
                self._process_custom_controls(value)
            } else {
                value.clone() // fast_deepcopy(value) (do_copy)
            };

            if kwargs.asdict {
                results_dict.insert(key.to_string(), value);
            } else {
                results_list.push(value);
            }
        }

        match last {
            Ok(_) => {
                if kwargs.asdict {
                    Ok(results_dict
                        .into_iter()
                        .next()
                        .map(|(_, v)| v)
                        .expect("one result"))
                } else {
                    Ok(results_list.pop().expect("one result"))
                }
            }
            Err(_) => {
                if kwargs.asdict {
                    Ok(Value::Object(results_dict))
                } else {
                    Ok(Value::Array(results_list))
                }
            }
        }
    }

    // ~~ getter

    /// `settings/__init__.py:1962 get`, for a path of keys: `None` is Python's `None` (a path that
    /// does not exist, or a stored `null`).
    pub fn get(&self, path: &[&str]) -> Option<Value> {
        self.get_with(path, &GetKwargs::default())
            .expect("error_on_path is False")
    }

    /// `settings/__init__.py:1962 get` with keyword arguments: `Err` is the raised
    /// `NoSuchSettingsPath` (only with `error_on_path`).
    pub fn get_with(
        &self,
        path: &[&str],
        kwargs: &GetKwargs,
    ) -> Result<Option<Value>, NoSuchSettingsPath> {
        let result = match path.split_last() {
            // `if not path: raise NoSuchSettingsPath()`
            None => Err(NoSuchSettingsPath),
            Some((last, parent)) => self._get_value(parent, Ok(last), kwargs),
        };
        Self::_finish_get(result, kwargs)
    }

    /// `settings/__init__.py:1962 get` for a path whose last element is a list of keys
    /// (`get(["serial", ["timeout", "additionalPorts"]])`): `parent` is the path before that list.
    /// The result is an array (an object with `asdict`).
    pub fn get_keys(
        &self,
        parent: &[&str],
        keys: &[&str],
        kwargs: &GetKwargs,
    ) -> Result<Option<Value>, NoSuchSettingsPath> {
        Self::_finish_get(self._get_value(parent, Err(keys), kwargs), kwargs)
    }

    /// The tail of `get` (`:1963-1978`): `process()` and the `error_on_path` handling.
    fn _finish_get(
        result: Result<Value, NoSuchSettingsPath>,
        kwargs: &GetKwargs,
    ) -> Result<Option<Value>, NoSuchSettingsPath> {
        match result {
            Ok(Value::Null) => Ok(None),
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                if kwargs.error_on_path {
                    Err(e)
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// `settings/__init__.py:1980 getInt`
    pub fn get_int(&self, path: &[&str]) -> Option<i64> {
        self.get_int_with(path, &GetKwargs::default(), None, None)
            .expect("error_on_path is False")
    }

    /// `settings/__init__.py:1980 getInt` with `min` and `max` (`minimum`, `maximum`) and the
    /// `get` keyword arguments.
    pub fn get_int_with(
        &self,
        path: &[&str],
        kwargs: &GetKwargs,
        minimum: Option<i64>,
        maximum: Option<i64>,
    ) -> Result<Option<i64>, NoSuchSettingsPath> {
        let value = self.get_with(path, kwargs)?;
        let value = match value {
            None => return Ok(None),
            Some(v) => v,
        };

        // intValue = int(value)
        let int_value: Option<i64> = match &value {
            Value::Bool(b) => Some(*b as i64),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Some(i)
                } else if n.is_u64() {
                    panic!("int {n} does not fit i64 (Python's int is unbounded)")
                } else {
                    let f = n.as_f64().expect("f64");
                    if f.is_nan() {
                        None // ValueError: cannot convert float NaN to integer
                    } else if f.is_infinite() {
                        panic!("OverflowError: cannot convert float infinity to integer")
                    } else {
                        let t = f.trunc();
                        if t >= -9223372036854775808.0 && t < 9223372036854775808.0 {
                            Some(t as i64)
                        } else {
                            panic!("int({f}) does not fit i64 (Python's int is unbounded)")
                        }
                    }
                }
            }
            Value::String(s) => py_int(s),
            other => {
                panic!("TypeError: int() argument must be a string or a real number, not {other}")
            }
        };

        match int_value {
            Some(i) => {
                if minimum.is_some_and(|m| i < m) {
                    Ok(minimum)
                } else if maximum.is_some_and(|m| i > m) {
                    Ok(maximum)
                } else {
                    Ok(Some(i))
                }
            }
            None => {
                // self._logger.warning("Could not convert %r to a valid integer when getting option %r" % (value, path))
                Ok(None)
            }
        }
    }

    /// `settings/__init__.py:2004 getFloat`
    pub fn get_float(&self, path: &[&str]) -> Option<f64> {
        self.get_float_with(path, &GetKwargs::default(), None, None)
            .expect("error_on_path is False")
    }

    /// `settings/__init__.py:2004 getFloat` with `min`, `max` and the `get` keyword arguments.
    pub fn get_float_with(
        &self,
        path: &[&str],
        kwargs: &GetKwargs,
        minimum: Option<f64>,
        maximum: Option<f64>,
    ) -> Result<Option<f64>, NoSuchSettingsPath> {
        let value = self.get_with(path, kwargs)?;
        let value = match value {
            None => return Ok(None),
            Some(v) => v,
        };

        // floatValue = float(value)
        let float_value: Option<f64> = match &value {
            Value::Bool(b) => Some(*b as i64 as f64),
            Value::Number(n) => Some(n.as_f64().expect("f64")),
            Value::String(s) => py_float(s),
            other => {
                panic!("TypeError: float() argument must be a string or a real number, not {other}")
            }
        };

        match float_value {
            Some(f) => {
                if minimum.is_some_and(|m| f < m) {
                    Ok(minimum)
                } else if maximum.is_some_and(|m| f > m) {
                    Ok(maximum)
                } else {
                    Ok(Some(f))
                }
            }
            None => {
                // self._logger.warning("Could not convert %r to a valid integer when getting option %r" % (value, path))
                Ok(None)
            }
        }
    }

    /// `settings/__init__.py:2028 getBoolean`
    pub fn get_boolean(&self, path: &[&str]) -> Option<bool> {
        self.get_boolean_with(path, &GetKwargs::default())
            .expect("error_on_path is False")
    }

    /// `settings/__init__.py:2028 getBoolean` with the `get` keyword arguments.
    pub fn get_boolean_with(
        &self,
        path: &[&str],
        kwargs: &GetKwargs,
    ) -> Result<Option<bool>, NoSuchSettingsPath> {
        let value = self.get_with(path, kwargs)?;
        Ok(Self::_to_boolean(value))
    }

    /// The body of `getBoolean` after `get` (`:2029-2038`).
    fn _to_boolean(value: Option<Value>) -> Option<bool> {
        match value {
            None => None,
            Some(Value::Bool(b)) => Some(b),
            // isinstance(value, (int, float)): value != 0
            Some(Value::Number(n)) => Some(n.as_f64() != Some(0.0)),
            Some(Value::String(s)) => {
                Some(valid_boolean_trues.contains(&Value::String(s.to_lowercase())))
            }
            // return value is not None
            Some(_) => Some(true),
        }
    }
}

// ~~ script templating

/// `settings/__init__.py:823 SnippetExtension`: `{% snippet 'name' %}` is `parse_include()` with
/// `"snippets/"` put in front of the template name unless it starts with `"/"`.
///
/// minijinja has no custom tags, so the tag is rewritten into the include statement that
/// `SnippetExtension.parse` returns, before the template is compiled (Jinja2 does the same
/// rewrite on the parse tree). Differences from Jinja2 (ours, none reachable with the defaults):
/// - only a string literal in `'...'` or `"..."` (without escapes) is recognised as the name; Jinja2
///   takes any expression from `parse_include` and then fails on `node.template.value` unless it is
///   a constant. A tag we do not recognise stays `snippet`, which minijinja rejects as an unknown
///   statement; either way the template does not compile and `loadScript` gives `None`.
/// - the tag is found by pattern, so a `{% snippet %}` inside `{% raw %}` or a `{# comment #}` is
///   rewritten too; Jinja2's lexer leaves it alone.
/// - `ignore missing` and `with/without context` after the name are kept as written and are
///   handled by minijinja's include, not by Jinja2's `parse_include`.
static SNIPPET_TAG: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(r#"\{%(?P<l>[-+]?)\s*snippet\s+(?P<q>['"])(?P<name>[^'"]*)['"](?P<rest>[^%]*?)(?P<r>[-+]?)%\}"#)
        .expect("valid regex")
});

fn snippet_extension(source: &str) -> String {
    SNIPPET_TAG
        .replace_all(source, |c: &fancy_regex::Captures| {
            let name = &c["name"];
            // `if not node.template.value.startswith("/"): value = "snippets/" + value`
            let name = if name.starts_with('/') {
                name.to_string()
            } else {
                format!("snippets/{name}")
            };
            let q = &c["q"];
            format!(
                "{{%{} include {q}{name}{q}{} {}%}}",
                &c["l"],
                c["rest"].trim_end(),
                &c["r"]
            )
        })
        .into_owned()
}

/// `settings/__init__.py:832 SettingsScriptLoader.get_source`: walks `get(["scripts"], merged=True)`
/// along the `/`-separated parts of the template name; `None` is `TemplateNotFound`.
///
/// Python returns `(source, None, uptodate)`; minijinja caches a loaded template by name, which the
/// `uptodate` callback (`mtime == last_modified`) only matters for with a config file on disk. The
/// source is also not checked for being a `str`: Python's Jinja2 would raise on a non-string there
/// and `_get_script_template` turns that into `None`; here it is `Err`, which gives `None` too.
fn settings_script_loader_get_source(
    scripts: &Value,
    template: &str,
) -> Result<Option<String>, minijinja::Error> {
    // `parts = template.split("/")`; `if not len(parts)` is never true for `str.split`.
    let mut script = scripts;
    for part in template.split('/') {
        match script {
            Value::Object(map) if map.contains_key(part) => script = &map[part],
            _ => return Ok(None),
        }
    }
    match script {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        other => Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("script source is not a string: {other}"),
        )),
    }
}

/// `settings/__init__.py:841 SettingsScriptLoader.list_templates` / `_get_templates`.
fn settings_script_loader_get_templates(scripts: &Value) -> Vec<String> {
    let mut templates = Vec::new();
    if let Value::Object(map) = scripts {
        for (key, value) in map {
            if value.is_object() {
                templates.extend(
                    settings_script_loader_get_templates(value)
                        .into_iter()
                        .map(|x| format!("{key}/{x}")),
                );
            } else if value.is_string() {
                templates.push(key.clone());
            }
        }
    }
    templates
}

/// `settings/__init__.py:867 SelectLoader.get_source` with `sep=":"` and the mapping
/// `{"bundled": settings_loader, "file": file_system_loader}`, the default being
/// `ChoiceLoader([file_system_loader, settings_loader])`.
///
/// The file system loader is `FilteredFileSystemLoader(getBaseFolder("scripts"), ...)` over
/// OctoPrint's basedir. We have defaults only and no basedir, so it finds nothing (ours: no folder
/// is invented); the choice loader therefore always ends at the settings loader.
fn select_loader_get_source(
    scripts: &Value,
    template: &str,
) -> Result<Option<String>, minijinja::Error> {
    if let Some((prefix, name)) = template.split_once(':') {
        return match prefix {
            "bundled" => settings_script_loader_get_source(scripts, name),
            // `file_system_loader.get_source`: nothing to find without a basedir
            "file" => Ok(None),
            _ => Ok(None),
        };
    }
    settings_script_loader_get_source(scripts, template)
}

/// `settings/__init__.py:885 RelEnvironment._split_prefix`
fn rel_split_prefix(template: &str) -> (&str, &str) {
    // `template.split(self._prefix_sep, 1)` if the separator is in it, else `"", template`
    template.split_once(':').unwrap_or(("", template))
}

/// `settings/__init__.py:885 RelEnvironment._join_prefix`
fn rel_join_prefix(prefix: &str, template: &str) -> String {
    if !prefix.is_empty() {
        format!("{prefix}:{template}")
    } else {
        template.to_string()
    }
}

/// `settings/__init__.py:885 RelEnvironment.join_path`
fn rel_join_path(template: &str, parent: &str) -> String {
    let (prefix, name) = rel_split_prefix(template);
    if let Some(rest) = name.strip_prefix('/') {
        rel_join_prefix(prefix, rest)
    } else {
        let (_, parent_name) = rel_split_prefix(parent);
        let mut parent_base: Vec<&str> = parent_name.split('/').collect();
        parent_base.pop(); // `[:-1]`
        rel_join_prefix(prefix, &format!("{}/{}", parent_base.join("/"), name))
    }
}

/// `settings/__init__.py:814 Settings._init_script_templating`: the `RelEnvironment` over the
/// `SelectLoader`, with the `SnippetExtension`.
///
/// Environment settings, each as in Jinja2's `Environment` defaults that `RelEnvironment` leaves
/// untouched (`jinja2/environment.py`, 3.1.6):
/// - `keep_trailing_newline=False` (`KEEP_TRAILING_NEWLINE = False`): one trailing newline of the
///   template is stripped; minijinja's default is the same, set explicitly.
/// - `trim_blocks=False`, `lstrip_blocks=False` (`TRIM_BLOCKS`, `LSTRIP_BLOCKS`): the newline after
///   `{% endif %}`/`{% endfor %}` is kept; minijinja's defaults, set explicitly.
/// - `autoescape=False`: no escaping (minijinja would decide by file extension, so it is switched
///   off for every name).
/// - undefined variables are lenient (`undefined=Undefined`): they print as nothing and are falsy;
///   minijinja's `UndefinedBehavior::Lenient`, set explicitly.
///
/// Ours: `scripts` is the value of `get(["scripts"], merged=True)` taken when the environment is
/// built (Python's loader asks `self._settings` at each lookup, and the environment is built once
/// in `__init__`); we build one per `load_script` call, so both see the same thing.
fn init_script_templating(scripts: Value) -> minijinja::Environment<'static> {
    let mut env = minijinja::Environment::new();
    env.set_keep_trailing_newline(false);
    env.set_trim_blocks(false);
    env.set_lstrip_blocks(false);
    env.set_auto_escape_callback(|_| minijinja::AutoEscape::None);
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Lenient);
    // `RelEnvironment.join_path`, called with the including template as `parent`
    env.set_path_join_callback(|name, parent| rel_join_path(name, parent).into());
    env.set_loader(move |name| {
        Ok(select_loader_get_source(&scripts, name)?.map(|source| snippet_extension(&source)))
    });
    env
}

impl Settings {
    /// `settings/__init__.py:2106 listScripts`, with `_get_scripts` (`:944`): the names under
    /// `script_type/` that `Environment.list_templates(filter_func=...)` finds, sorted. The file
    /// system loader lists nothing (no basedir), so these are the settings loader's.
    pub fn list_scripts(&self, script_type: &str) -> Vec<String> {
        let prefix = format!("{script_type}/");
        let scripts = self.scripts();
        let mut names: Vec<String> = settings_script_loader_get_templates(&scripts)
            .into_iter()
            .filter(|x| x.starts_with(&prefix))
            .collect();
        // `Environment.list_templates` returns `sorted(names)`; `ChoiceLoader` puts them in a set
        names.sort();
        names.dedup();
        names
            .into_iter()
            .map(|x| x[prefix.len()..].to_string())
            .collect()
    }

    /// `get(["scripts"], merged=True)`, as `SettingsScriptLoader` asks for it.
    fn scripts(&self) -> Value {
        let kwargs = GetKwargs {
            merged: true,
            ..Default::default()
        };
        self.get_with(&["scripts"], &kwargs)
            .expect("error_on_path is False")
            .unwrap_or(Value::Null)
    }

    /// `settings/__init__.py:2115 loadScript` (without `source=True`, which only
    /// `server/api/settings.py:353` uses): `None` is Python's `None` (unknown script, or an
    /// exception while loading or rendering it).
    ///
    /// `context` is a JSON object (`None` for Python's `None`), so every key is a string; see
    /// [`Settings::load_script_value`] for a context with int keys.
    pub fn load_script(
        &self,
        script_type: &str,
        name: &str,
        context: Option<&Value>,
    ) -> Option<String> {
        match context {
            Some(Value::Object(_)) => self.load_script_value(
                script_type,
                name,
                Some(minijinja::Value::from_serialize(context.unwrap())),
            ),
            Some(_) => None, // `context.update` / `**context` raise on a non-mapping
            None => self.load_script_value(script_type, name, None),
        }
    }

    /// `loadScript` with the context as a minijinja value, so that keys can be ints
    /// (`as_script_dict`, comm.py:407: `last_temperature[0]`) and the order of the keys is the
    /// caller's (insertion order, as Python's dict; minijinja's `preserve_order`).
    ///
    /// Like Python, `{"script": {"type": script_type, "name": name}}` is put into the context
    /// (replacing a `script` key in its place, else at the end). Python does that to the caller's
    /// dict; here the caller's value is not touched (ours; the caller, `comm.py:1393`, does not
    /// read it again). A context that is not a mapping, or has a key that is not a str, is `None`
    /// (`**context` raises `TypeError`).
    pub fn load_script_value(
        &self,
        script_type: &str,
        name: &str,
        context: Option<minijinja::Value>,
    ) -> Option<String> {
        let mut pairs: Vec<(minijinja::Value, minijinja::Value)> = Vec::new();
        if let Some(context) = context {
            if context.kind() != minijinja::value::ValueKind::Map {
                return None;
            }
            for key in context.try_iter().ok()? {
                key.as_str()?;
                let value = context.get_item(&key).ok()?;
                pairs.push((key, value));
            }
        }
        let script = minijinja::Value::from_serialize(&json!({"type": script_type, "name": name}));
        match pairs.iter_mut().find(|(k, _)| k.as_str() == Some("script")) {
            Some(pair) => pair.1 = script,
            None => pairs.push((minijinja::Value::from("script"), script)),
        }

        // `_get_script_template` (`:944`): any failure is `None`
        let env = init_script_templating(self.scripts());
        let template = env.get_template(&format!("{script_type}/{name}")).ok()?;
        // `template.render(**context)`
        template.render(minijinja::Value::from_iter(pairs)).ok()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings::new()
    }
}

#[cfg(test)]
mod tests {
    //! Ports of the getter tests of `tests/settings/test_settings.py` (`SettingsTest`, and
    //! `HelpersTest.test_valid_boolean_trues`). The reference fixture (`_files/config.yaml`,
    //! `overlay.yaml`, `defaults.yaml`) is replaced by OctoPrint's own defaults, so the expected
    //! values that came from the fixture are the defaults' values; each test names the value it
    //! had in the reference. Tests marked "ours" are not from the reference.
    use super::*;

    fn defaults_json() -> Value {
        serde_json::from_str(DEFAULT_SETTINGS_JSON).unwrap()
    }

    /// test_get (reference: `["api", "key"]` == "test", set by the fixture's config.yaml)
    #[test]
    fn test_get() {
        let settings = Settings::new();
        let value = settings.get(&["appearance", "color"]);
        assert!(value.is_some());
        assert_eq!(value, Some(json!("default")));
    }

    /// test_get_int (reference: `["server", "port"]` == 8080; the default is 5000)
    #[test]
    fn test_get_int() {
        let settings = Settings::new();
        let server_port = settings.get(&["server", "port"]);
        assert!(server_port.is_some());
        assert_eq!(server_port, Some(json!(5000)));
    }

    /// test_get_int_converted (reference: "5" -> 5; the default is the float 10.0 -> 10)
    #[test]
    fn test_get_int_converted() {
        let settings = Settings::new();
        assert_eq!(
            Some(10),
            settings.get_int(&["serial", "timeout", "connection"])
        );
    }

    /// test_get_int_invalid (reference: `["server", "host"]`, 0.0.0.0 in the fixture, the default
    /// is None)
    #[test]
    fn test_get_int_invalid() {
        let settings = Settings::new();
        assert_eq!(None, settings.get_int(&["server", "host"]));
    }

    /// test_get_float (reference: `["serial", "timeout", "detection"]` == 1.0; the defaults have
    /// `detectionFirst`)
    #[test]
    fn test_get_float() {
        let settings = Settings::new();
        let serial_timeout = settings.get(&["serial", "timeout", "detectionFirst"]);
        assert!(serial_timeout.is_some());
        assert_eq!(serial_timeout, Some(json!(10.0)));
    }

    /// test_get_float_converted (reference: "5" -> 5.0; the default is 10.0)
    #[test]
    fn test_get_float_converted() {
        let settings = Settings::new();
        assert_eq!(
            Some(10.0),
            settings.get_float(&["serial", "timeout", "connection"])
        );
    }

    /// test_get_float_invalid
    #[test]
    fn test_get_float_invalid() {
        let settings = Settings::new();
        assert_eq!(None, settings.get_float(&["server", "host"]));
    }

    /// test_get_boolean (reference: `["devel", "virtualPrinter", "enabled"]`, which the defaults do
    /// not have; `["devel", "useFrozenDictForPrinterState"]` is True)
    #[test]
    fn test_get_boolean() {
        let settings = Settings::new();
        let value = settings.get(&["devel", "useFrozenDictForPrinterState"]);
        assert_eq!(value, Some(json!(true)));
    }

    /// test_get_list (reference: two ports from the fixture; the default is empty)
    #[test]
    fn test_get_list() {
        let settings = Settings::new();
        let data = settings.get(&["serial", "additionalPorts"]).unwrap();
        assert_eq!(data.as_array().unwrap().len(), 0);
        assert_eq!(data, json!([]));
    }

    /// test_get_map (reference: the fixture's `devel.virtualPrinter`; here the defaults' `devel`)
    #[test]
    fn test_get_map() {
        let settings = Settings::new();
        let data = settings.get(&["devel"]).unwrap();
        assert_eq!(defaults_json()["devel"], data);
    }

    /// test_get_map_merged
    #[test]
    fn test_get_map_merged() {
        let settings = Settings::new();
        let kwargs = GetKwargs {
            merged: true,
            ..Default::default()
        };
        let data = settings.get_with(&["devel"], &kwargs).unwrap().unwrap();
        assert_eq!(defaults_json()["devel"], data);
    }

    /// test_get_multiple
    #[test]
    fn test_get_multiple() {
        let settings = Settings::new();
        let data = settings
            .get_keys(
                &["serial"],
                &["timeout", "additionalPorts"],
                &GetKwargs::default(),
            )
            .unwrap()
            .unwrap();
        let data = data.as_array().expect("list");
        assert_eq!(data.len(), 2);
        assert!(data[0].is_object());
        assert!(data[1].is_array());
    }

    /// test_get_multiple_asdict
    #[test]
    fn test_get_multiple_asdict() {
        let settings = Settings::new();
        let kwargs = GetKwargs {
            asdict: true,
            ..Default::default()
        };
        let data = settings
            .get_keys(&["serial"], &["timeout", "additionalPorts"], &kwargs)
            .unwrap()
            .unwrap();
        let data = data.as_object().expect("dict");
        assert_eq!(data.len(), 2);
        assert!(data.contains_key("timeout"));
        assert!(data.contains_key("additionalPorts"));
    }

    /// test_get_invalid
    #[test]
    fn test_get_invalid() {
        let settings = Settings::new();
        assert_eq!(None, settings.get(&["i", "do", "not", "exist"]));
    }

    /// test_get_invalid_error
    #[test]
    fn test_get_invalid_error() {
        let settings = Settings::new();
        let kwargs = GetKwargs {
            error_on_path: true,
            ..Default::default()
        };
        assert_eq!(
            Err(NoSuchSettingsPath),
            settings.get_with(&["i", "do", "not", "exist"], &kwargs)
        );
    }

    /// test_get_empty_path
    #[test]
    fn test_get_empty_path() {
        let settings = Settings::new();
        assert_eq!(None, settings.get(&[]));
        let kwargs = GetKwargs {
            error_on_path: true,
            ..Default::default()
        };
        assert_eq!(Err(NoSuchSettingsPath), settings.get_with(&[], &kwargs));
    }

    /// HelpersTest.test_valid_boolean_trues
    #[test]
    fn test_valid_boolean_trues() {
        let data = [
            (json!(true), true),
            (json!("true"), true),
            (json!("True"), true),
            (json!("tRuE"), true),
            (json!("yes"), true),
            (json!("YES"), true),
            (json!("y"), true),
            (json!("Y"), true),
            (json!("1"), true),
            (json!(1), true),
            (json!(false), false),
            (json!("Truuuuuuuuue"), false),
            (json!("Nope"), false),
            (json!(null), false),
        ];
        for (value, expected) in data {
            assert_eq!(expected, valid_boolean_trues.contains(&value), "{value}");
        }
    }

    // ---- ours ----

    /// ours: every `settings().get*` call in util/comm.py and printer/standard.py finds its path
    /// exactly where OctoPrint's defaults have it, or misses it where they miss it (comm.py:616
    /// `serial.capabilities.chamber_temp` is not in OctoPrint's schema, so OctoPrint gets None).
    /// The list is extracted with Python's parser by scripts/octoprint-settings-paths.py
    /// (78 calls, all literal paths; none left out).
    #[test]
    fn every_settings_path_comm_reads_exists() {
        let s = Settings::new();
        let list = include_str!("settings_call_paths.txt");
        let mut n = 0;
        for line in list.lines() {
            let mut cols = line.split('\t');
            let (at, _method, path, state) = (
                cols.next().unwrap(),
                cols.next().unwrap(),
                cols.next().unwrap(),
                cols.next().unwrap(),
            );
            assert_ne!(path, "NOT LITERAL", "{at}");
            let path: Vec<String> = serde_json::from_str(path).unwrap();
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            let kw = GetKwargs {
                error_on_path: true,
                ..Default::default()
            };
            match state {
                "exists" => assert!(s.get_with(&path, &kw).is_ok(), "{at} {path:?}"),
                "missing" => {
                    assert!(s.get_with(&path, &kw).is_err(), "{at} {path:?}");
                    assert_eq!(s.get(&path), None, "{at} {path:?}");
                }
                other => panic!("{at}: {other}"),
            }
            n += 1;
        }
        assert_eq!(n, 78);
    }

    /// ours: the call at comm.py:554, `get(["serial", "timeout"], merged=True, asdict=True)`.
    #[test]
    fn ours_serial_timeout_merged_asdict() {
        let s = Settings::new();
        let kw = GetKwargs {
            merged: true,
            asdict: true,
            ..Default::default()
        };
        let v = s.get_with(&["serial", "timeout"], &kw).unwrap().unwrap();
        assert_eq!(defaults_json()["serial"]["timeout"], v);
    }

    /// ours: the conversion rules of getInt/getFloat/getBoolean over other values.
    #[test]
    fn ours_conversions() {
        let s = Settings::from_defaults(json!({
            "i": 7, "f": 2.9, "neg": -2.9, "t": true, "s": " +4_2 ", "bad": "abc", "fs": "5.",
            "z": 0, "zf": 0.0, "n": null, "l": [1], "e": {}, "yes": "YeS", "no": "no", "one": "1",
            "empty": "", "d": {"a": 1}
        }));
        assert_eq!(Some(7), s.get_int(&["i"]));
        assert_eq!(Some(2), s.get_int(&["f"]));
        assert_eq!(Some(-2), s.get_int(&["neg"]));
        assert_eq!(Some(1), s.get_int(&["t"]));
        assert_eq!(Some(42), s.get_int(&["s"]));
        assert_eq!(None, s.get_int(&["bad"]));
        assert_eq!(None, s.get_int(&["n"]));
        assert_eq!(None, s.get_int(&["missing"]));
        assert_eq!(Some(5.0), s.get_float(&["fs"]));
        assert_eq!(Some(7.0), s.get_float(&["i"]));
        assert_eq!(Some(1.0), s.get_float(&["t"]));
        assert_eq!(None, s.get_float(&["bad"]));
        // min / max
        let kw = GetKwargs::default();
        assert_eq!(Ok(Some(10)), s.get_int_with(&["i"], &kw, Some(10), None));
        assert_eq!(Ok(Some(5)), s.get_int_with(&["i"], &kw, None, Some(5)));
        assert_eq!(Ok(Some(7)), s.get_int_with(&["i"], &kw, Some(1), Some(9)));
        assert_eq!(
            Ok(Some(3.0)),
            s.get_float_with(&["i"], &kw, None, Some(3.0))
        );
        // getBoolean
        assert_eq!(None, s.get_boolean(&["n"]));
        assert_eq!(None, s.get_boolean(&["missing"]));
        assert_eq!(Some(true), s.get_boolean(&["t"]));
        assert_eq!(Some(true), s.get_boolean(&["i"]));
        assert_eq!(Some(false), s.get_boolean(&["z"]));
        assert_eq!(Some(false), s.get_boolean(&["zf"]));
        assert_eq!(Some(true), s.get_boolean(&["yes"]));
        assert_eq!(Some(false), s.get_boolean(&["no"]));
        assert_eq!(Some(true), s.get_boolean(&["one"]));
        assert_eq!(Some(false), s.get_boolean(&["empty"]));
        assert_eq!(Some(true), s.get_boolean(&["l"]));
        assert_eq!(Some(true), s.get_boolean(&["e"]));
        assert_eq!(Some(true), s.get_boolean(&["d"]));
    }

    /// ours: `["controls"]` goes through `_process_custom_controls`; Python's md5 of "a"/"b".
    #[test]
    fn ours_process_custom_controls() {
        let s = Settings::from_defaults(json!({"controls": [
            {"regex": "a", "template": "b"},
            {"name": "g", "children": [null, {"regex": "a", "template": "b"}, {"x": 1}]},
        ]}));
        let v = s.get(&["controls"]).unwrap();
        let key = "0cc175b9c0f1b6a831c399e269772661"; // md5("a")
        let tkey = "92eb5ffee6ae2fec3ad71c777531578f"; // md5("b")
        assert_eq!(v[0]["key"], key);
        assert_eq!(v[0]["template_key"], tkey);
        assert_eq!(v[1]["children"][0]["key"], key);
        assert_eq!(v[1]["children"].as_array().unwrap().len(), 2);
    }

    // ~~ script templating

    const SCRIPTS_EXPECTED_JSON: &str = include_str!("scripts_expected.json");

    /// The settings the real OctoPrint ran with (`scripts/octoprint-scripts-check.py`): the defaults
    /// plus `extra_scripts` below `scripts.gcode` (Python adds them as an overlay; with defaults
    /// only they go straight into our defaults).
    fn settings_with_extra_scripts(expected: &Value) -> Settings {
        let mut settings = Settings::new();
        let gcode = settings.default_settings["scripts"]["gcode"]
            .as_object_mut()
            .unwrap();
        for (key, value) in expected["extra_scripts"].as_object().unwrap() {
            gcode.insert(key.clone(), value.clone());
        }
        settings
    }

    /// A fixture context (`scripts/octoprint-scripts-check.py`) as minijinja value: a key written as
    /// `"#int:<n>"` is the int key `n` (JSON has no int keys), key order is kept.
    fn tagged_context(value: &Value) -> minijinja::Value {
        match value {
            Value::Object(map) => minijinja::Value::from_iter(map.iter().map(|(k, v)| {
                let key = match k.strip_prefix("#int:") {
                    Some(n) => minijinja::Value::from(n.parse::<i64>().unwrap()),
                    None => minijinja::Value::from(k.as_str()),
                };
                (key, tagged_context(v))
            })),
            Value::Array(a) => {
                minijinja::Value::from(a.iter().map(tagged_context).collect::<Vec<_>>())
            }
            other => minijinja::Value::from_serialize(other),
        }
    }

    /// Mechanical proof (ours; the reference has no such test): every case the real OctoPrint 1.11.8
    /// `loadScript` was run on gives byte-identical output (or `None`) with our port.
    #[test]
    fn scripts_identical_to_octoprint() {
        let expected: Value = serde_json::from_str(SCRIPTS_EXPECTED_JSON).unwrap();
        let settings = settings_with_extra_scripts(&expected);
        // Known differences, not reachable with OctoPrint's defaults (the default scripts use only
        // `printer_profile` and the two snippets); they matter once user scripts exist:
        // - t_float_exp: Python prints 1e+20 / 1e-07, minijinja 100000000000000000000.0 / 0.0000001;
        // - t_round: Jinja2's `round` is Python's `round` (1.25 -> 1.2), minijinja's gives 1.3;
        // - t_raw_snip: the snippet rewrite also rewrites inside `{% raw %}`; Jinja2 does not.
        const KNOWN_DIFFER: [&str; 3] = ["t_float_exp", "t_round", "t_raw_snip"];
        let mut mismatches = Vec::new();
        let cases = expected["cases"].as_array().unwrap();
        assert!(cases.len() > 800);
        for case in cases {
            let context = if case["context"].is_null() {
                None
            } else {
                Some(&case["context"])
            };
            let got = settings.load_script_value(
                case["type"].as_str().unwrap(),
                case["name"].as_str().unwrap(),
                context.map(tagged_context),
            );
            let want = case["expected"].as_str().map(str::to_string);
            if KNOWN_DIFFER.contains(&case["name"].as_str().unwrap()) {
                if case["context"].get("last_position").is_some() || case["name"] == "t_raw_snip" {
                    assert_ne!(
                        got, want,
                        "{} no longer differs: take it off KNOWN_DIFFER",
                        case["name"]
                    );
                }
                continue;
            }
            if got != want {
                mismatches.push(format!(
                    "{} / {} / {}: want {:?}, got {:?}",
                    case["type"], case["name"], case["context"], want, got
                ));
            }
        }
        for (script_type, names) in expected["list_scripts"].as_object().unwrap() {
            let want: Vec<String> = names
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_string())
                .collect();
            assert_eq!(
                settings.list_scripts(script_type),
                want,
                "listScripts({script_type})"
            );
        }
        assert!(
            mismatches.is_empty(),
            "{} of {} differ:\n{}",
            mismatches.len(),
            cases.len(),
            mismatches.join("\n")
        );
    }

    /// `get(["scripts"], merged=True)` as the loader calls it (ours).
    #[test]
    fn scripts_loader_get_merged() {
        let settings = Settings::new();
        let scripts = settings.scripts();
        assert_eq!(scripts, defaults_json()["scripts"]);
        assert!(scripts["gcode"]["snippets"]["disable_bed"].is_string());
    }

    /// `loadScript` with the default script and a context (ours).
    #[test]
    fn load_script_default_after_print_cancelled() {
        let ctx = json!({"printer_profile": {"heatedBed": true, "extruder": {"count": 2, "sharedNozzle": false}}});
        assert_eq!(
            Settings::new()
                .load_script("gcode", "afterPrintCancelled", Some(&ctx))
                .as_deref(),
            Some(
                "; disable motors\nM84\n\n;disable all heaters\nM104 T0 S0\nM104 T1 S0\n\nM140 S0\n\n;disable fan\nM106 S0"
            )
        );
    }

    /// A script that is not there, or is `null` (the default of most events), is `None` (ours).
    #[test]
    fn load_script_missing_is_none() {
        let settings = Settings::new();
        assert_eq!(settings.load_script("gcode", "doesNotExist", None), None);
        assert_eq!(
            settings.load_script("gcode", "beforePrintStarted", None),
            None
        );
    }

    /// `RelEnvironment.join_path` (`:885`) (ours: values worked out from the source).
    #[test]
    fn rel_join_path_cases() {
        assert_eq!(rel_join_path("snippets/a", "gcode/x"), "gcode/snippets/a");
        assert_eq!(rel_join_path("/gcode/a", "gcode/x"), "gcode/a");
        assert_eq!(
            rel_join_path("bundled:/gcode/a", "gcode/x"),
            "bundled:gcode/a"
        );
        assert_eq!(rel_join_path("a", "file:gcode/x"), "gcode/a");
        assert_eq!(rel_join_path("a", "x"), "/a");
    }

    /// `SnippetExtension.parse` (`:823`) (ours).
    #[test]
    fn snippet_extension_rewrite() {
        assert_eq!(
            snippet_extension("{% snippet 'a' %}"),
            "{% include 'snippets/a' %}"
        );
        assert_eq!(
            snippet_extension("{%- snippet '/a/b' -%}"),
            "{%- include '/a/b' -%}"
        );
    }
}
