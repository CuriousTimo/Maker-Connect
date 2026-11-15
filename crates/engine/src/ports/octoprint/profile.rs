//! Port of the parts of OctoPrint 1.11.8 `src/octoprint/printer/profile.py` that `util/comm.py` and
//! `printer/standard.py` use: the class `PrinterProfileManager` (:213) with `default` (:222-257),
//! `get_default` (:443), `get_current` (:473), `get_current_or_default` (:466), `select` (:368),
//! `deselect` (:384), `_ensure_valid_profile` (:630) and `_default_box_for_volume` (:812), and the
//! constants `BedFormFactor` (:120) and `BedOrigin` (:143).
//!
//! Original: Copyright (C) 2014 The OctoPrint Project, AGPL-3.0.
//!
//! A printer profile is a `dict`: here a JSON object (`serde_json` with `preserve_order`), in
//! Python's key order. What `comm.py` and `standard.py` read of it: `profile["id"]` (standard.py:1349),
//! `["axes"][axis]["speed"]` (standard.py:524, 604), `["extruder"]["count"]` and `["sharedNozzle"]`
//! (standard.py:640-641, comm.py:3684-3699), `["heatedBed"]`, `["heatedChamber"]` (comm.py:3689, 5171,
//! 5191, 5357) and the whole dict as the `printer_profile` of a script's context (comm.py:4941).
//!
//! Not ported (not wire): `__init__` (:213, the profile folder, `_migrate_old_default_profile`,
//! `_verify_default_available`), `_load_all`, `_load_all_identifiers`, `_load_from_path`, `_save_to_path`,
//! `_get_profile_path`, `_migrate_profile`, `save`, `remove`, `get_all`, `set_default`, `last_modified`,
//! `profile_count`, `is_default_unmodified`: file I/O and the settings file.
//!
//! OURS: because no profile file is read, `exists(identifier)` is false for every identifier and
//! `get(identifier)` is `None` (the profile folder is empty); `select(identifier)` therefore always takes
//! its first branch (`self._current = self.get_default()`, returns False) and `get_default()` is the
//! built-in `default`. `PrinterProfileManager::new()` is `self._current = None` only. A profile in the
//! `default` has `"offsets": [(0, 0)]` in Python (a tuple); JSON has no tuple, so it is `[[0, 0]]`, and
//! the `offsets` that `_ensure_valid_profile` builds (a list of float tuples) are arrays of two floats.
//! Python's warnings (`self._logger.warning`) are comments. A non-finite float that Python keeps in a
//! profile (`extruder.nozzleDiameter`, `extruder.offsets`: `float("inf")`) cannot be a JSON number and is
//! `unimplemented!`.

use super::comm::{py_float, py_int, py_truthy, py_try};
use super::py_attr::Attr;
use super::settings::settings;
use super::util::dict_contains_keys;
use serde_json::{json, Map, Value};

/// `profile.py:123 BedFormFactor`
pub struct BedFormFactor;

impl BedFormFactor {
    pub const RECTANGULAR: &'static str = "rectangular"; // :126
    pub const CIRCULAR: &'static str = "circular"; // :129

    /// `values()` (:132), in the order the attributes are defined
    pub fn values() -> [&'static str; 2] {
        [Self::RECTANGULAR, Self::CIRCULAR]
    }
}

/// `profile.py:143 BedOrigin`
pub struct BedOrigin;

impl BedOrigin {
    pub const LOWERLEFT: &'static str = "lowerleft"; // :146
    pub const CENTER: &'static str = "center"; // :149

    /// `values()` (:152)
    pub fn values() -> [&'static str; 2] {
        [Self::LOWERLEFT, Self::CENTER]
    }
}

/// `PrinterProfileManager` (profile.py:213), see the module doc for what is ported.
pub struct PrinterProfileManager {
    /// `self._current`
    _current: Attr<Option<Value>>,
}

/// Python `int(value)` of a JSON value; `Err` is the exception (`TypeError`, `ValueError`,
/// `OverflowError`) that `convert_value`'s caller catches (`except Exception`).
fn convert_int(v: &Value) -> Result<Value, String> {
    match v {
        Value::Bool(b) => Ok(Value::from(*b as i64)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::from(i))
            } else if let Some(u) = n.as_u64() {
                Ok(Value::from(u))
            } else {
                let f = n.as_f64().expect("a JSON number is an f64");
                if !f.is_finite() {
                    return Err("OverflowError: cannot convert float infinity to integer".to_string());
                }
                let t = f.trunc();
                if t.abs() >= 9.2e18 {
                    unimplemented!("profile.py:662 int() of a float beyond the 64-bit range")
                }
                Ok(Value::from(t as i64))
            }
        }
        Value::String(s) => py_int(s).map(Value::from).ok_or_else(|| format!("ValueError: invalid literal for int() with base 10: {s:?}")),
        other => Err(format!("TypeError: int() argument must be a string or a real number, not {other:?}")),
    }
}

/// Python `float(value)`; `Err` as for `convert_int`
fn convert_float(v: &Value) -> Result<f64, String> {
    match v {
        Value::Bool(b) => Ok(*b as i64 as f64),
        Value::Number(n) => Ok(n.as_f64().expect("a JSON number is an f64")),
        Value::String(s) => py_float(s).ok_or_else(|| format!("ValueError: could not convert string to float: {s:?}")),
        other => Err(format!("TypeError: float() argument must be a string or a real number, not {other:?}")),
    }
}

/// a finite float as a JSON number (a non-finite one is `unimplemented!`, see the module doc)
fn float_value(f: f64) -> Value {
    Value::from(f)
}

/// `value[path[0]][path[1]]...` as `convert_value` / `validate_value` walk it: `RuntimeError("... is not
/// contained in profile")` (a panic) when a part is missing or not a dict
fn walk<'a>(profile: &'a mut Value, path: &[&str]) -> &'a mut Value {
    let mut value = profile;
    for part in &path[..path.len() - 1] {
        match value {
            Value::Object(m) if m.contains_key(*part) => value = m.get_mut(*part).unwrap(),
            _ => panic!("RuntimeError: {} is not contained in profile", path.join(".")),
        }
    }
    match value {
        Value::Object(m) if m.contains_key(path[path.len() - 1]) => m.get_mut(path[path.len() - 1]).unwrap(),
        _ => panic!("RuntimeError: {} is not contained in profile", path.join(".")),
    }
}

/// Python `min(a, b)` / `max(a, b)` of two floats: the first unless the second is strictly less / greater
fn py_min(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

fn py_max(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

impl PrinterProfileManager {
    /// `PrinterProfileManager.default` (:222-257)
    pub fn default_profile() -> Value {
        json!({
            "id": "_default",
            "name": "Default",
            "model": "Generic RepRap Printer",
            "color": "default",
            "volume": {
                "width": 200,
                "depth": 200,
                "height": 200,
                "formFactor": BedFormFactor::RECTANGULAR,
                "origin": BedOrigin::LOWERLEFT,
                "custom_box": false,
            },
            "heatedBed": true,
            "heatedChamber": false,
            "extruder": {
                "count": 1,
                "offsets": [[0, 0]],
                "nozzleDiameter": 0.4,
                "sharedNozzle": false,
                "defaultExtrusionLength": 5,
            },
            "axes": {
                "x": {"speed": 6000, "inverted": false},
                "y": {"speed": 6000, "inverted": false},
                "z": {"speed": 200, "inverted": false},
                "e": {"speed": 300, "inverted": false},
            },
        })
    }

    /// OURS: `__init__` (:213) without the profile folder: `self._current = None`
    pub fn new() -> Self {
        PrinterProfileManager { _current: Attr::new(None) }
    }

    /// OURS (for the tests and for the top edge): a manager whose current profile is `profile`, as if
    /// `select` had loaded it from a file
    pub fn with_current(profile: Value) -> Self {
        PrinterProfileManager { _current: Attr::new(Some(profile)) }
    }

    /// `:368 select(identifier)`
    pub fn select(&self, identifier: Option<&str>) -> bool {
        if identifier.is_none() || !self.exists(identifier) {
            self._current.set(Some(self.get_default()));
            false
        } else {
            let current = self.get(identifier.expect("identifier is not None here"));
            self._current.set(current.clone());
            if current.is_none() {
                // self._logger.error("Profile {} is invalid, cannot select, falling back to default")
                self._current.set(Some(self.get_default()));
                false
            } else {
                true
            }
        }
    }

    /// `:384 deselect()`
    pub fn deselect(&self) {
        self._current.set(None);
    }

    /// `:392 get(identifier)`: OURS, `None` (no profile file is read, see the module doc)
    pub fn get(&self, _identifier: &str) -> Option<Value> {
        None
    }

    /// `:482 exists(identifier)`: OURS, always false (see the module doc)
    pub fn exists(&self, identifier: Option<&str>) -> bool {
        if identifier.is_none() {
            return false;
        }
        false
    }

    /// `:443 get_default()`
    pub fn get_default(&self) -> Value {
        let default = settings().get(&["printerProfiles", "default"]);
        if let Some(Value::String(default)) = default.filter(|d| !d.is_null()) {
            if self.exists(Some(&default)) {
                if let Some(profile) = self.get(&default) {
                    return profile;
                }
                // self._logger.warning("Default profile {} is invalid, falling back to built-in defaults")
            }
        }

        Self::default_profile()
    }

    /// `:466 get_current_or_default()`
    pub fn get_current_or_default(&self) -> Value {
        match self._current.get() {
            Some(current) => current,
            None => self.get_default(),
        }
    }

    /// `:473 get_current()`
    pub fn get_current(&self) -> Option<Value> {
        self._current.get()
    }

    /// `:630 _ensure_valid_profile(profile)`: the profile, converted and completed, or `None` for
    /// Python's `False`
    pub fn _ensure_valid_profile(&self, profile: Value) -> Option<Value> {
        let mut profile = profile;
        let default = Self::default_profile();

        // ensure all keys are present
        if !dict_contains_keys(&default, &profile) {
            // self._logger.warning("Profile invalid, missing keys. Expected: ... Actual: ...")
            return None;
        }

        // convert ints
        for path in [
            &["extruder", "count"][..],
            &["axes", "x", "speed"],
            &["axes", "y", "speed"],
            &["axes", "z", "speed"],
            &["axes", "e", "speed"],
        ] {
            // convert_value: a RuntimeError for a missing path is inside the `try` too
            let converted = py_try(|| {
                let slot = walk(&mut profile, path);
                match convert_int(slot) {
                    Ok(v) => {
                        *slot = v;
                        true
                    }
                    Err(_) => false,
                }
            });
            if !matches!(converted, Ok(true)) {
                // self._logger.warning("Profile has invalid value for path {path!r}: {msg}")
                return None;
            }
        }

        // convert floats
        let mut volume_not_finite = false;
        for path in [&["volume", "width"][..], &["volume", "depth"], &["volume", "height"], &["extruder", "nozzleDiameter"]] {
            let converted = py_try(|| {
                let slot = walk(&mut profile, path);
                match convert_float(slot) {
                    Ok(f) => {
                        if f.is_finite() {
                            *slot = float_value(f);
                            Some(true)
                        } else if path[0] == "volume" {
                            // `x > 0 and x < 10000` below is False for nan and inf
                            Some(false)
                        } else {
                            unimplemented!("profile.py:679 a non-finite extruder.nozzleDiameter")
                        }
                    }
                    Err(_) => None,
                }
            });
            match converted {
                Ok(Some(true)) => {}
                Ok(Some(false)) => volume_not_finite = true,
                _ => {
                    // self._logger.warning("Profile has invalid value for path ...")
                    return None;
                }
            }
        }

        // convert booleans
        for path in [
            &["axes", "x", "inverted"][..],
            &["axes", "y", "inverted"],
            &["axes", "z", "inverted"],
            &["extruder", "sharedNozzle"],
        ] {
            let converted = py_try(|| {
                let slot = walk(&mut profile, path);
                *slot = Value::Bool(py_truthy(slot));
            });
            if converted.is_err() {
                return None;
            }
        }

        // validate volume size range
        if volume_not_finite {
            return None;
        }
        for path in [&["volume", "width"][..], &["volume", "depth"], &["volume", "height"]] {
            let x = walk(&mut profile, path).as_f64().expect("converted to a float above");
            if !(x > 0.0 && x < 10000.0) {
                return None;
            }
        }

        // validate extruder count range
        let count = walk(&mut profile, &["extruder", "count"]).as_i64().expect("converted to an int above");
        if !(count > 0 && count < 100) {
            return None;
        }

        // validate form factor
        let form_factor = profile["volume"]["formFactor"].clone();
        if !BedFormFactor::values().iter().any(|v| form_factor == Value::String(v.to_string())) {
            // self._logger.warning("Profile has invalid value volume.formFactor: {formFactor}")
            return None;
        }

        // validate origin type
        let origin = profile["volume"]["origin"].clone();
        if !BedOrigin::values().iter().any(|v| origin == Value::String(v.to_string())) {
            // self._logger.warning("Profile has invalid value in volume.origin: {origin}")
            return None;
        }

        // ensure origin and form factor combination is legal
        if form_factor == BedFormFactor::CIRCULAR && origin != BedOrigin::CENTER {
            profile["volume"]["origin"] = Value::String(BedOrigin::CENTER.to_string());
        }

        // force width and depth of volume to be identical for circular beds, with width being the reference
        if form_factor == BedFormFactor::CIRCULAR {
            profile["volume"]["depth"] = profile["volume"]["width"].clone();
        }

        // if we have a custom bounding box, validate it
        let custom_box = profile["volume"]["custom_box"].clone();
        if py_truthy(&custom_box) && custom_box.is_object() {
            if custom_box.as_object().expect("an object").is_empty() {
                profile["volume"]["custom_box"] = Value::Bool(false);
            } else {
                let default_box = Self::_default_box_for_volume(&profile["volume"]);
                let mut cb: Map<String, Value> = custom_box.as_object().expect("an object").clone();
                for (prop, is_min) in [("x_min", true), ("y_min", true), ("z_min", true), ("x_max", false), ("y_max", false), ("z_max", false)] {
                    let limit = default_box.get(prop).and_then(Value::as_f64).expect("the default box has the property");
                    if cb.get(prop).is_none_or(Value::is_null) {
                        cb.insert(prop.to_string(), float_value(limit));
                    } else {
                        let value = &cb[prop];
                        match convert_float(value) {
                            Ok(f) => {
                                let limited = if is_min { py_min(f, limit) } else { py_max(f, limit) };
                                if !limited.is_finite() {
                                    unimplemented!("profile.py:773 a non-finite custom_box value")
                                }
                                cb.insert(prop.to_string(), float_value(limited));
                            }
                            Err(_) => {
                                // self._logger.warning("Profile has invalid value in volume.custom_box.{}: {!r}")
                                return None;
                            }
                        }
                    }
                }

                // make sure we actually do have a custom box and not just the same values as the
                // default box
                let mut differs = false;
                for (prop, value) in &cb {
                    let expected = default_box.get(prop).unwrap_or_else(|| panic!("KeyError: {prop:?}"));
                    if value.as_f64() != expected.as_f64() {
                        differs = true;
                        break;
                    }
                }
                profile["volume"]["custom_box"] = if differs { Value::Object(cb) } else { Value::Bool(false) };
            }
        }

        // validate offsets
        let mut offsets: Vec<Value> = Vec::new();
        let raw_offsets = profile["extruder"]["offsets"].clone();
        let entries: Vec<Value> = match &raw_offsets {
            Value::Array(a) => a.clone(),
            Value::String(s) => s.chars().map(|c| Value::String(c.to_string())).collect(),
            Value::Object(o) => o.keys().map(|k| Value::String(k.clone())).collect(),
            other => panic!("TypeError: {other:?} is not iterable"),
        };
        for offset in entries {
            let parts: Vec<Value> = match &offset {
                Value::Array(a) => a.clone(),
                Value::String(s) => s.chars().map(|c| Value::String(c.to_string())).collect(),
                Value::Object(o) => o.keys().map(|k| Value::String(k.clone())).collect(),
                other => panic!("TypeError: object of type {other:?} has no len()"),
            };
            if parts.len() != 2 {
                // self._logger.warning("Profile has an invalid extruder.offsets entry: {entry!r}")
                return None;
            }
            match (convert_float(&parts[0]), convert_float(&parts[1])) {
                (Ok(x), Ok(y)) => {
                    if !x.is_finite() || !y.is_finite() {
                        unimplemented!("profile.py:808 a non-finite extruder.offsets entry")
                    }
                    offsets.push(json!([x, y]));
                }
                _ => {
                    // self._logger.warning("Profile has an extruder.offsets entry with non-float values: {entry!r}")
                    return None;
                }
            }
        }
        profile["extruder"]["offsets"] = Value::Array(offsets);

        Some(profile)
    }

    /// `:812 _default_box_for_volume(volume)`
    pub fn _default_box_for_volume(volume: &Value) -> Map<String, Value> {
        let num = |k: &str| volume[k].as_f64().unwrap_or_else(|| panic!("TypeError: volume[{k:?}] is not a number"));
        let mut b = Map::new();
        if volume["origin"] == BedOrigin::CENTER {
            let half_width = num("width") / 2.0;
            let half_depth = num("depth") / 2.0;
            b.insert("x_min".into(), float_value(-half_width));
            b.insert("x_max".into(), float_value(half_width));
            b.insert("y_min".into(), float_value(-half_depth));
            b.insert("y_max".into(), float_value(half_depth));
            b.insert("z_min".into(), float_value(0.0));
            b.insert("z_max".into(), float_value(num("height")));
        } else {
            b.insert("x_min".into(), float_value(0.0));
            b.insert("x_max".into(), float_value(num("width")));
            b.insert("y_min".into(), float_value(0.0));
            b.insert("y_max".into(), float_value(num("depth")));
            b.insert("z_min".into(), float_value(0.0));
            b.insert("z_max".into(), float_value(num("height")));
        }
        b
    }
}

impl Default for PrinterProfileManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    //! All OURS: OctoPrint 1.11.8 has no test for `printer/profile.py`'s `PrinterProfileManager`
    //! (`tests/` holds none). Each is checked by hand against the source line named.
    use super::*;

    #[test]
    fn ours_default_is_the_class_default() {
        // profile.py:222-257
        let d = PrinterProfileManager::default_profile();
        assert_eq!("_default", d["id"]);
        assert_eq!(6000, d["axes"]["x"]["speed"]);
        assert_eq!(300, d["axes"]["e"]["speed"]);
        assert_eq!(true, d["heatedBed"]);
        assert_eq!(false, d["heatedChamber"]);
        assert_eq!(1, d["extruder"]["count"]);
        let keys: Vec<&String> = d.as_object().unwrap().keys().collect();
        assert_eq!(vec!["id", "name", "model", "color", "volume", "heatedBed", "heatedChamber", "extruder", "axes"], keys);
    }

    #[test]
    fn ours_current_or_default() {
        // profile.py:466 / :368 / :384: nothing selected is the default; select(None) selects the
        // default and returns False; deselect clears it
        let m = PrinterProfileManager::new();
        assert!(m.get_current().is_none());
        assert_eq!(PrinterProfileManager::default_profile(), m.get_current_or_default());
        assert!(!m.select(None));
        assert_eq!(Some(PrinterProfileManager::default_profile()), m.get_current());
        assert!(!m.select(Some("other"))); // ours: no profile file exists
        m.deselect();
        assert!(m.get_current().is_none());
        let m = PrinterProfileManager::with_current(json!({"id": "x"}));
        assert_eq!(json!({"id": "x"}), m.get_current_or_default());
    }

    #[test]
    fn ours_ensure_valid_profile_default_is_valid() {
        let m = PrinterProfileManager::new();
        let valid = m._ensure_valid_profile(PrinterProfileManager::default_profile()).expect("valid");
        assert_eq!(json!([[0.0, 0.0]]), valid["extruder"]["offsets"]);
        assert_eq!(json!(200.0), valid["volume"]["width"]);
        assert_eq!(json!(200), valid["axes"]["z"]["speed"]);
    }

    #[test]
    fn ours_ensure_valid_profile_refusals() {
        let m = PrinterProfileManager::new();
        // missing key (:632)
        let mut p = PrinterProfileManager::default_profile();
        p.as_object_mut().unwrap().remove("axes");
        assert!(m._ensure_valid_profile(p).is_none());
        // a value that cannot be converted (:650)
        let mut p = PrinterProfileManager::default_profile();
        p["extruder"]["count"] = json!("two");
        assert!(m._ensure_valid_profile(p).is_none());
        // volume size range (:719)
        let mut p = PrinterProfileManager::default_profile();
        p["volume"]["width"] = json!(10000);
        assert!(m._ensure_valid_profile(p).is_none());
        // extruder count range (:726)
        let mut p = PrinterProfileManager::default_profile();
        p["extruder"]["count"] = json!(100);
        assert!(m._ensure_valid_profile(p).is_none());
        // form factor and origin (:731, :739)
        let mut p = PrinterProfileManager::default_profile();
        p["volume"]["formFactor"] = json!("round");
        assert!(m._ensure_valid_profile(p).is_none());
        let mut p = PrinterProfileManager::default_profile();
        p["volume"]["origin"] = json!("corner");
        assert!(m._ensure_valid_profile(p).is_none());
        // an offsets entry of the wrong length or with a non-float (:796, :805)
        let mut p = PrinterProfileManager::default_profile();
        p["extruder"]["offsets"] = json!([[0, 0, 0]]);
        assert!(m._ensure_valid_profile(p).is_none());
        let mut p = PrinterProfileManager::default_profile();
        p["extruder"]["offsets"] = json!([[0, "x"]]);
        assert!(m._ensure_valid_profile(p).is_none());
    }

    #[test]
    fn ours_ensure_valid_profile_circular_and_custom_box() {
        let m = PrinterProfileManager::new();
        // a circular bed gets the centre origin and depth = width (:748-754)
        let mut p = PrinterProfileManager::default_profile();
        p["volume"]["formFactor"] = json!("circular");
        p["volume"]["width"] = json!(150);
        let v = m._ensure_valid_profile(p).unwrap();
        assert_eq!("center", v["volume"]["origin"]);
        assert_eq!(json!(150.0), v["volume"]["depth"]);
        // a custom box equal to the default box is removed (:790); a smaller one is kept and completed (:758-786)
        let mut p = PrinterProfileManager::default_profile();
        p["volume"]["custom_box"] = json!({"x_min": 0, "x_max": 200});
        assert_eq!(json!(false), m._ensure_valid_profile(p).unwrap()["volume"]["custom_box"]);
        let mut p = PrinterProfileManager::default_profile();
        p["volume"]["custom_box"] = json!({"x_min": 10, "x_max": 500});
        let b = m._ensure_valid_profile(p).unwrap()["volume"]["custom_box"].clone();
        assert_eq!(json!(0.0), b["x_min"]); // min(10.0, 0.0)
        assert_eq!(json!(500.0), b["x_max"]); // max(500.0, 200.0)
        assert_eq!(json!(200.0), b["z_max"]);
    }
}
