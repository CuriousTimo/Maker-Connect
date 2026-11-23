//! Implementation of camera classes - Camera, CameraRegister, Snapshot and Resolution
//! (`camera.py`).
//!
//! Reference: the SDK at `c0d7d233` (branch `websockets`), `prusa/connect/printer/camera.py`,
//! ported function by function; `CameraRegister` is in [`crate::models`], as in `models.py`.
//!
//! **Python's values.** A camera setting holds whatever it was given, as in Python: [`Value`] is
//! a Python value ([`PyValue`]) or a `TriggerScheme` or a [`Resolution`]. Comparisons, `str()` and
//! the setters' checks follow Python's rules for them (`0 == 0.0`, `True == 1`, `str(1.0)` is
//! `"1.0"`).
//!
//! **Where this port differs:**
//! - A `frozenset` or `set` iterates in an order that depends on Python's hashes, which change
//!   per process (`PYTHONHASHSEED`): `capabilities`, `configurable_capabilities` (the order of
//!   the keys of `get_settings`, of the `config` of `/p/camera` and of the ini) and
//!   `available_resolutions`. Here capabilities are in their definition order and resolutions in
//!   the order the driver gave them.
//! - Python collects the cycle driver -> `photo_cb` -> camera -> driver; here it is kept (one
//!   camera per driver, until the program ends).
//!
//! **Not covered** (an error [`PyErr::Other`] `"NotCovered"`): a resolution whose width or
//! height is not an `int` that fits in 64 bits (`settings_from_json` with `{"width": "640"}`,
//! `settings_from_string` with a 20-digit width), and a `TriggerScheme` or `Resolution` under
//! another key in `json_from_settings`.

use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use maker_connect_gcode_metadata::py::{Dict, PyErr, PyValue, int_from_str, repr_str};

use crate::camera_driver::CameraDriver;
use crate::clock;
use crate::consts::CameraConfig;
use crate::consts::{
    CAMERA_BUSY_TIMEOUT, CAMERA_WAIT_TIMEOUT, CameraError, CapabilityType, DEFAULT_EXPOSURE,
    DEFAULT_ROTATION, DEFAULT_TRIGGER_SCHEME, TriggerScheme,
};
use crate::util::{RequestError, Response, Session, make_fingerprint, py_float_repr};

/// An exception of the camera modules: one of `const.py`'s, or a Python one.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Camera(CameraError),
    Py(PyErr),
}

impl Error {
    /// The exception's class name.
    pub fn name(&self) -> &'static str {
        match self {
            Error::Camera(err) => err.name(),
            Error::Py(err) => err.class_name(),
        }
    }

    /// `str(err)`
    pub fn message(&self) -> String {
        match self {
            Error::Camera(err) => err.to_string(),
            Error::Py(err) => err.message().to_owned(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name(), self.message())
    }
}

impl std::error::Error for Error {}

impl From<CameraError> for Error {
    fn from(err: CameraError) -> Error {
        Error::Camera(err)
    }
}

impl From<PyErr> for Error {
    fn from(err: PyErr) -> Error {
        Error::Py(err)
    }
}

/// The error for what this port does not cover (see the module doc).
pub(crate) fn not_covered(what: String) -> PyErr {
    PyErr::Other("NotCovered", what)
}

/// `KeyError(key)`: its `str` is the key's `repr`.
pub(crate) fn key_error(key: &str) -> PyErr {
    PyErr::KeyError(repr_str(key))
}

/// Snapshot from the camera
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub camera_token: Option<String>,
    pub camera_fingerprint: Option<String>,
    pub camera_id: Option<String>,
    pub printer_uuid: Option<String>,
    pub data: Option<Vec<u8>>,
    pub timestamp: Option<f64>,
}

/// A snapshot as Python shares it: the driver's `last_snapshot`, the one in the queue and the
/// one the camera fills in are the same object.
pub type SharedSnapshot = Arc<Mutex<Snapshot>>;

/// The `User-Agent` of `requests.utils.default_user_agent()`, which a snapshot goes with: it sets
/// no `User-Agent` of its own. requests 2.32.5, as `crates/engine/src/ports/prusalink/
/// reference-requirements.txt` pins it.
pub const REQUESTS_USER_AGENT: &str = "python-requests/2.32.5";

impl Snapshot {
    pub const ENDPOINT: &'static str = "/c/snapshot";
    pub const METHOD: &'static str = "PUT";

    pub fn new() -> Snapshot {
        Snapshot::default()
    }

    /// Is this snapshot complete and can it therefore be sent?
    pub fn is_sendable(&self) -> bool {
        self.camera_fingerprint.is_some()
            && self.camera_token.is_some()
            && self.camera_id.is_some()
            && self.timestamp.is_some()
            && self.data.is_some()
    }

    /// A snapshot send function. A header whose value is `None` is left out, as `requests`
    /// leaves it out.
    pub fn send(&self, conn: &Session, server: &str) -> Result<Response, RequestError> {
        if !self.is_sendable() {
            log::warn!(target: "camera", "Sending an incomplete snapshot");
        }
        let name = "Snapshot";
        log::debug!(target: "camera", "Sending {name}: {self:?}");

        let timestamp = match self.timestamp {
            Some(timestamp) => py_float_repr(timestamp),
            None => "None".to_owned(),
        };
        let mut headers = vec![("Timestamp".to_owned(), timestamp)];
        if let Some(fingerprint) = &self.camera_fingerprint {
            headers.push(("Fingerprint".to_owned(), fingerprint.clone()));
        }
        if let Some(token) = &self.camera_token {
            headers.push(("Token".to_owned(), token.clone()));
        }
        headers.push(("Content-Type".to_owned(), "image/jpg".to_owned()));
        let mut params = Vec::new();
        if let Some(printer_uuid) = &self.printer_uuid {
            params.push(("printer_uuid".to_owned(), printer_uuid.clone()));
        }
        let url = format!("{server}{}", Snapshot::ENDPOINT);
        let res = conn.request_data(
            Snapshot::METHOD,
            &url,
            &params,
            REQUESTS_USER_AGENT,
            &headers,
            self.data.as_deref(),
        )?;

        log::debug!(target: "camera", "{name} response: {}", res.text);
        Ok(res)
    }
}

/// A class to represent a camera resolution
///
/// `==` compares width and height; `<`, `>`, `<=` and `>=` compare the number of pixels, so
/// 640x480 and 480x640 are neither equal nor greater than one another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Resolution {
    pub width: i64,
    pub height: i64,
}

impl Resolution {
    pub fn new(width: i64, height: i64) -> Resolution {
        Resolution { width, height }
    }

    fn pixels(&self) -> i128 {
        i128::from(self.width) * i128::from(self.height)
    }

    /// Reverses the width and height - rotates to portrait or landscape (`__reversed__`)
    pub fn reversed(&self) -> Resolution {
        Resolution::new(self.height, self.width)
    }

    /// `__gt__`
    pub fn gt(&self, other: &Resolution) -> bool {
        self.pixels() > other.pixels()
    }

    /// `__ge__`
    pub fn ge(&self, other: &Resolution) -> bool {
        self.pixels() >= other.pixels()
    }

    /// `__lt__`
    pub fn lt(&self, other: &Resolution) -> bool {
        self.pixels() < other.pixels()
    }

    /// `__le__`
    pub fn le(&self, other: &Resolution) -> bool {
        self.pixels() <= other.pixels()
    }

    /// `dict(resolution)` (`__iter__`)
    pub fn to_dict(&self) -> Dict {
        let mut dict = Dict::new();
        dict.set("width", PyValue::int(self.width));
        dict.set("height", PyValue::int(self.height));
        dict
    }

    /// `sorted(resolutions)`: Python's sort is stable and uses `<` only.
    pub fn sorted(resolutions: &[Resolution]) -> Vec<Resolution> {
        let mut sorted = resolutions.to_vec();
        sorted.sort_by(|a, b| {
            if a.lt(b) {
                std::cmp::Ordering::Less
            } else if b.lt(a) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        sorted
    }
}

impl fmt::Display for Resolution {
    /// A simple `<width>x<height>` string representation
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

/// A camera setting's value, as Python holds it (see the module doc).
#[derive(Debug, Clone)]
pub enum Value {
    Py(PyValue),
    Scheme(TriggerScheme),
    Resolution(Resolution),
}

impl PartialEq for Value {
    /// Python's `==`: an enum member equals only itself, a `Resolution` only a `Resolution`.
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Py(a), Value::Py(b)) => a == b,
            (Value::Scheme(a), Value::Scheme(b)) => a == b,
            (Value::Resolution(a), Value::Resolution(b)) => a == b,
            _ => false,
        }
    }
}

impl Value {
    pub fn none() -> Value {
        Value::Py(PyValue::None)
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Value::Py(PyValue::None))
    }

    /// `str(value)`
    pub fn to_py_str(&self) -> String {
        match self {
            Value::Py(value) => value.to_py_str(),
            Value::Scheme(scheme) => format!("TriggerScheme.{}", scheme.name()),
            Value::Resolution(resolution) => resolution.to_string(),
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Value::Py(value) => value.type_name(),
            Value::Scheme(_) => "TriggerScheme",
            Value::Resolution(_) => "Resolution",
        }
    }

    /// `hash(value)` raises for a `list` or a `dict`.
    fn check_hashable(&self) -> Result<(), PyErr> {
        match self {
            Value::Py(PyValue::List(_)) | Value::Py(PyValue::Dict(_)) => Err(PyErr::TypeError(
                format!("unhashable type: '{}'", self.type_name()),
            )),
            _ => Ok(()),
        }
    }

    /// `lo <= value <= hi` for `int` bounds.
    fn in_range(&self, lo: i64, hi: i64) -> Result<bool, PyErr> {
        match self {
            Value::Py(PyValue::Bool(b)) => Ok(lo <= i64::from(*b) && i64::from(*b) <= hi),
            Value::Py(PyValue::Int(i)) => Ok(match i.to_i64() {
                Some(x) => lo <= x && x <= hi,
                // beyond 64 bits: below or above both bounds
                None => false,
            }),
            Value::Py(PyValue::Float(f)) => Ok(lo as f64 <= *f && *f <= hi as f64),
            // `int.__le__` gives NotImplemented, the reflected `Resolution.__ge__` reads `.width`
            Value::Resolution(_) => Err(PyErr::AttributeError(
                "'int' object has no attribute 'width'".into(),
            )),
            other => Err(PyErr::TypeError(format!(
                "'<=' not supported between instances of 'int' and '{}'",
                other.type_name()
            ))),
        }
    }
}

/// A Python `dict` of settings, in insertion order.
pub type Settings = Vec<(String, Value)>;

/// `d[key] = value`: an existing key keeps its place.
pub fn dict_set<V>(dict: &mut Vec<(String, V)>, key: &str, value: V) {
    match dict.iter_mut().find(|(k, _)| k == key) {
        Some(entry) => entry.1 = value,
        None => dict.push((key.to_owned(), value)),
    }
}

/// `d.get(key)`
pub fn dict_get<'a, V>(dict: &'a [(String, V)], key: &str) -> Option<&'a V> {
    dict.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// The default values for camera settings if the camera does not supply its own ones
/// (`DEFAULT_CAMERA_SETTINGS`, in its order).
pub fn default_camera_settings() -> Settings {
    vec![
        (
            CapabilityType::TriggerScheme.value().to_owned(),
            Value::Scheme(DEFAULT_TRIGGER_SCHEME),
        ),
        (
            CapabilityType::Exposure.value().to_owned(),
            Value::Py(PyValue::int(DEFAULT_EXPOSURE)),
        ),
        (
            CapabilityType::Rotation.value().to_owned(),
            Value::Py(PyValue::int(DEFAULT_ROTATION)),
        ),
    ]
}

/// Changed the trigger scheme of this camera - tell the manager: `(camera, old, new)`.
pub type SchemeCb = Arc<dyn Fn(&Arc<Camera>, &Value, &Value) -> Result<(), Error> + Send + Sync>;
/// A photo has been taken - give it to the manager
pub type PhotoCb = Arc<dyn Fn(SharedSnapshot) -> Result<(), Error> + Send + Sync>;

#[derive(Debug, Clone)]
struct CameraState {
    trigger_scheme: Value,
    resolution: Value,
    rotation: Value,
    exposure: Value,
    focus: Value,
    token: Option<String>,
}

/// The class for fully loaded cameras to be operated by the application
pub struct Camera {
    driver: Arc<CameraDriver>,
    capabilities: Vec<CapabilityType>,
    available_resolutions: Vec<Resolution>,
    state: Mutex<CameraState>,
    /// `_ready_event`: set when ready
    ready: Mutex<bool>,
    ready_cv: Condvar,
    busy_since: Mutex<Option<f64>>,
    scheme_cb: Mutex<SchemeCb>,
    photo_cb: Mutex<PhotoCb>,
    /// `wait_timeout`, `busy_timeout` (class attributes)
    timeouts: Mutex<(f64, f64)>,
}

impl fmt::Debug for Camera {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Camera")
            .field("camera_id", &self.camera_id())
            .finish_non_exhaustive()
    }
}

/// The capabilities in their definition order, each once.
fn in_definition_order(capabilities: &[CapabilityType]) -> Vec<CapabilityType> {
    CapabilityType::ALL
        .iter()
        .copied()
        .filter(|cap| capabilities.contains(cap))
        .collect()
}

/// `set(resolutions)`, in the order given.
fn unique(resolutions: &[Resolution]) -> Vec<Resolution> {
    let mut out: Vec<Resolution> = Vec::new();
    for resolution in resolutions {
        if !out.contains(resolution) {
            out.push(*resolution);
        }
    }
    out
}

impl Camera {
    pub fn new(driver: Arc<CameraDriver>) -> Result<Arc<Camera>, Error> {
        let capabilities = in_definition_order(&driver.capabilities());
        if !capabilities.contains(&CapabilityType::TriggerScheme) {
            return Err(PyErr::AttributeError(
                "Be sure to fill out driver supported capabilities. TRIGGER_SCHEME is the bare \
                 minimum"
                    .into(),
            )
            .into());
        }
        let available_resolutions = if capabilities.contains(&CapabilityType::Resolution) {
            unique(&driver.available_resolutions())
        } else {
            Vec::new()
        };

        let camera = Arc::new(Camera {
            driver: driver.clone(),
            capabilities,
            available_resolutions,
            state: Mutex::new(CameraState {
                trigger_scheme: Value::none(),
                resolution: Value::none(),
                rotation: Value::Py(PyValue::int(0)),
                exposure: Value::Py(PyValue::Float(0.0)),
                focus: Value::Py(PyValue::Float(0.0)),
                token: None,
            }),
            ready: Mutex::new(true),
            ready_cv: Condvar::new(),
            busy_since: Mutex::new(None),
            scheme_cb: Mutex::new(Arc::new(|_, _, _| Ok(()))),
            photo_cb: Mutex::new(Arc::new(|_| Ok(()))),
            timeouts: Mutex::new((CAMERA_WAIT_TIMEOUT as f64, CAMERA_BUSY_TIMEOUT as f64)),
        });
        let handler = camera.clone();
        driver.set_photo_cb(Arc::new(move |snapshot| handler.photo_handler(snapshot)));

        // - Initial settings -
        let mut initial_settings = default_camera_settings();

        let config = driver.config();
        let driver_settings = Camera::settings_from_string(&config)?;
        for (key, value) in driver_settings {
            dict_set(&mut initial_settings, &key, value);
        }

        // Check for missing configuration
        let missing: Vec<&str> = camera
            .configurable_capabilities()
            .iter()
            .map(|cap| cap.value())
            .filter(|needed| dict_get(&initial_settings, needed).is_none())
            .collect();
        if !missing.is_empty() {
            return Err(PyErr::AttributeError(format!(
                "Your driver is expected to define at least these additional settings: {}",
                missing.join(", ")
            ))
            .into());
        }

        camera.set_settings(&initial_settings, false)?;

        // - End initial settings -
        Ok(camera)
    }

    /// `Camera.wait_timeout`
    pub fn wait_timeout(&self) -> f64 {
        self.timeouts.lock().unwrap().0
    }

    /// `Camera.busy_timeout`
    pub fn busy_timeout(&self) -> f64 {
        self.timeouts.lock().unwrap().1
    }

    pub fn set_wait_timeout(&self, seconds: f64) {
        self.timeouts.lock().unwrap().0 = seconds;
    }

    pub fn set_busy_timeout(&self, seconds: f64) {
        self.timeouts.lock().unwrap().1 = seconds;
    }

    pub fn set_scheme_cb(&self, cb: SchemeCb) {
        *self.scheme_cb.lock().unwrap() = cb;
    }

    pub fn set_photo_cb(&self, cb: PhotoCb) {
        *self.photo_cb.lock().unwrap() = cb;
    }

    pub fn driver(&self) -> &Arc<CameraDriver> {
        &self.driver
    }

    // --- Camera settings ---

    /// Calls the getter for the value specified by CapabilityType
    pub fn get_value(&self, capability_type: CapabilityType) -> Result<Value, Error> {
        if capability_type == CapabilityType::Imaging {
            // getattr(self, "imaging")
            return Err(
                PyErr::AttributeError("'Camera' object has no attribute 'imaging'".into()).into(),
            );
        }
        // value_getter
        if !self.supports(capability_type) {
            return Err(CameraError::NotSupported(format!(
                "The camera {} does not support {}",
                self.name()?,
                capability_type.name()
            ))
            .into());
        }
        let state = self.state.lock().unwrap();
        Ok(match capability_type {
            CapabilityType::TriggerScheme => state.trigger_scheme.clone(),
            CapabilityType::Resolution => state.resolution.clone(),
            CapabilityType::Rotation => state.rotation.clone(),
            CapabilityType::Exposure => state.exposure.clone(),
            CapabilityType::Focus => state.focus.clone(),
            CapabilityType::Imaging => unreachable!("handled above"),
        })
    }

    /// Calls the setter for the value specified by CapabilityType, through `value_setter`:
    /// any exception of the setter disconnects the camera and becomes a `DriverError`.
    pub fn set_value(
        self: &Arc<Self>,
        capability_type: CapabilityType,
        value: Value,
    ) -> Result<(), Error> {
        if capability_type == CapabilityType::Imaging {
            // setattr(self, "imaging", value): a plain attribute, no setter
            return Ok(());
        }
        let old_value = self.get_value(capability_type)?;
        if !self.supports(capability_type) {
            return Err(CameraError::NotSupported(format!(
                "The camera {} does not support setting {}",
                self.name()?,
                capability_type.name()
            ))
            .into());
        }
        self.wait_ready(Some(self.wait_timeout()))?;
        self.become_busy();
        if let Err(exception) = self.setter(capability_type, &value) {
            log::error!(
                target: "camera",
                "Exception while setting {}\n{exception}",
                capability_type.name()
            );
            self.disconnect()?;
            return Err(CameraError::DriverError(format!(
                "The driver {} failed to set the {} from {} to {}.",
                self.driver.class().name,
                capability_type.value(),
                old_value.to_py_str(),
                value.to_py_str()
            ))
            .into());
        }
        self.become_ready();
        Ok(())
    }

    /// The setters: they call the driver first, so if it fails, the config change will get
    /// skipped.
    fn setter(
        self: &Arc<Self>,
        capability_type: CapabilityType,
        value: &Value,
    ) -> Result<(), Error> {
        match capability_type {
            CapabilityType::TriggerScheme => {
                let old = self.state.lock().unwrap().trigger_scheme.clone();
                let scheme_cb = self.scheme_cb.lock().unwrap().clone();
                scheme_cb(self, &old, value)?;
                self.state.lock().unwrap().trigger_scheme = value.clone();
            }
            CapabilityType::Resolution => {
                value.check_hashable()?;
                let resolution = match value {
                    Value::Resolution(r) if self.available_resolutions.contains(r) => *r,
                    _ => {
                        return Err(PyErr::ValueError(format!(
                            "Resolution {} is not available",
                            value.to_py_str()
                        ))
                        .into());
                    }
                };
                self.driver.set_resolution(&resolution)?;
                self.state.lock().unwrap().resolution = value.clone();
            }
            CapabilityType::Rotation => {
                value.check_hashable()?;
                let allowed = [0, 90, 180, 270]
                    .iter()
                    .any(|r| Value::Py(PyValue::int(*r)) == *value);
                if !allowed {
                    return Err(PyErr::ValueError(format!(
                        "Rotation of {}° is not allowed",
                        value.to_py_str()
                    ))
                    .into());
                }
                self.driver.set_rotation(value)?;
                self.state.lock().unwrap().rotation = value.clone();
            }
            CapabilityType::Exposure => {
                if !value.in_range(-2, 2)? {
                    return Err(PyErr::ValueError(format!(
                        "Exposure of {} is not allowed",
                        value.to_py_str()
                    ))
                    .into());
                }
                self.driver.set_exposure(value)?;
                self.state.lock().unwrap().exposure = value.clone();
            }
            CapabilityType::Focus => {
                if !value.in_range(0, 1)? {
                    return Err(PyErr::ValueError(format!(
                        "Focus value of {} is not allowed",
                        value.to_py_str()
                    ))
                    .into());
                }
                self.driver.set_focus(value)?;
                self.state.lock().unwrap().focus = value.clone();
            }
            CapabilityType::Imaging => unreachable!("set_value returns first"),
        }
        Ok(())
    }

    /// Getter for trigger scheme capability value
    pub fn trigger_scheme(&self) -> Result<Value, Error> {
        self.get_value(CapabilityType::TriggerScheme)
    }

    /// Getter for resolution capability value
    pub fn resolution(&self) -> Result<Value, Error> {
        self.get_value(CapabilityType::Resolution)
    }

    /// Getter for rotation capability value
    pub fn rotation(&self) -> Result<Value, Error> {
        self.get_value(CapabilityType::Rotation)
    }

    /// Getter for exposure capability value
    pub fn exposure(&self) -> Result<Value, Error> {
        self.get_value(CapabilityType::Exposure)
    }

    /// Getter for focus capability value
    pub fn focus(&self) -> Result<Value, Error> {
        self.get_value(CapabilityType::Focus)
    }

    // -----------------------------

    /// Gets the camera name
    pub fn name(&self) -> Result<String, PyErr> {
        self.driver
            .config_value("name")
            .ok_or_else(|| key_error("name"))
    }

    /// Sets the camera name
    pub fn set_name(&self, name: &str) {
        self.driver.set_name(name);
    }

    /// Gets the supported capabilities of this camera
    pub fn capabilities(&self) -> &[CapabilityType] {
        &self.capabilities
    }

    /// Returns capabilities with a configurable attribute
    pub fn configurable_capabilities(&self) -> Vec<CapabilityType> {
        self.capabilities
            .iter()
            .copied()
            .filter(|cap| *cap != CapabilityType::Imaging)
            .collect()
    }

    /// Is this camera busy? That's usually when taking a photo
    pub fn is_busy(&self) -> bool {
        !*self.ready.lock().unwrap()
    }

    /// Is this camera stuck in a busy state? Does it need help?
    pub fn is_stuck(&self) -> bool {
        let busy_since = *self.busy_since.lock().unwrap();
        busy_since.is_some_and(|since| clock::time() - since > self.busy_timeout())
    }

    /// Waits for the camera to become ready. raises TimeoutError if unsuccessful
    pub fn wait_ready(&self, timeout: Option<f64>) -> Result<(), Error> {
        let ready = self.ready.lock().unwrap();
        let ready = match timeout {
            None => self.ready_cv.wait_while(ready, |ready| !*ready).unwrap(),
            Some(timeout) => {
                let timeout = Duration::from_secs_f64(timeout.max(0.0));
                self.ready_cv
                    .wait_timeout_while(ready, timeout, |ready| !*ready)
                    .unwrap()
                    .0
            }
        };
        if !*ready {
            return Err(CameraError::ReadyTimeoutError(format!(
                "The camera {} did not become ready in time.",
                self.camera_id()
            ))
            .into());
        }
        Ok(())
    }

    /// Returns this camera's ID
    pub fn camera_id(&self) -> String {
        self.driver.camera_id().to_owned()
    }

    /// Returns the camera ID as a fingerprint for connect
    pub fn fingerprint(&self) -> String {
        make_fingerprint(&self.camera_id())
    }

    /// Is the camera registered to connect?
    pub fn is_registered(&self) -> bool {
        self.state.lock().unwrap().token.is_some()
    }

    /// Return the camera token
    pub fn token(&self) -> Option<String> {
        self.state.lock().unwrap().token.clone()
    }

    /// Sets or re-sets the camera token
    pub fn set_token(&self, token: Option<String>) -> Result<(), Error> {
        self.state.lock().unwrap().token = token;
        self.store()
    }

    /// Triggers a photo, waits for it to arrive and returns it. If a photo is already being
    /// taken, waits for that one
    pub fn take_a_photo(&self) -> Result<Option<SharedSnapshot>, Error> {
        if !self.supports(CapabilityType::Imaging) {
            return Err(CameraError::NotSupported(
                "This camera does not support returning images".into(),
            )
            .into());
        }
        if !self.is_busy() {
            self.trigger_a_photo(None)?;
        }
        self.wait_ready(Some(self.wait_timeout()))?;
        Ok(self.last_snapshot())
    }

    /// Triggers the camera, if we're expecting an image back, be busy until it arrives
    pub fn trigger_a_photo(&self, snapshot: Option<SharedSnapshot>) -> Result<(), Error> {
        if self.is_busy() {
            return Err(CameraError::CameraBusy(
                "The camera is far too busy to take more photos".into(),
            )
            .into());
        }
        if self.capabilities.contains(&CapabilityType::Imaging) {
            self.become_busy();
        }
        self.driver.trigger(snapshot);
        Ok(())
    }

    /// Gets the camera's last photo it has taken (can be None)
    pub fn last_snapshot(&self) -> Option<SharedSnapshot> {
        self.driver.last_snapshot()
    }

    /// Returns the expected resolution of the output image
    pub fn output_resolution(&self) -> Result<Option<Resolution>, Error> {
        if !self.supports(CapabilityType::Resolution) {
            return Ok(None);
        }
        let rotation = self.rotation()?;
        let resolution = match self.resolution()? {
            Value::Resolution(resolution) => resolution,
            other => {
                return Err(PyErr::TypeError(format!(
                    "'{}' object is not reversible",
                    other.type_name()
                ))
                .into());
            }
        };
        let turned = [90, 270]
            .iter()
            .any(|r| Value::Py(PyValue::int(*r)) == rotation);
        if turned {
            return Ok(Some(resolution.reversed()));
        }
        Ok(Some(resolution))
    }

    /// Gets the camera's available resolutions
    pub fn available_resolutions(&self) -> &[Resolution] {
        &self.available_resolutions
    }

    /// Returns whether the camera supports the given capability or not
    pub fn supports(&self, cap_type: CapabilityType) -> bool {
        self.capabilities.contains(&cap_type)
    }

    /// Sets the camera settings according to the given dict. The dictionary has to contain
    /// compatible values, convert them ahead of time using the string and json conversion
    /// methods
    pub fn set_settings(
        self: &Arc<Self>,
        new_settings: &Settings,
        store: bool,
    ) -> Result<(), Error> {
        for capability_type in self.configurable_capabilities() {
            let capability_name = capability_type.value();
            let Some(new_value) = dict_get(new_settings, capability_name) else {
                continue;
            };
            if *new_value != self.get_value(capability_type)? {
                self.set_value(capability_type, new_value.clone())?;
            }
        }
        if let Some(name) = dict_get(new_settings, "name") {
            self.set_name(&name.to_py_str());
        }
        if let Some(token) = dict_get(new_settings, "token") {
            self.state.lock().unwrap().token = match token {
                Value::Py(PyValue::None) => None,
                other => Some(other.to_py_str()),
            };
        }
        if store {
            self.store()?;
        }
        Ok(())
    }

    /// Converts settings from one format into a dictionary with Capability compatible values
    /// (from the strings of a config).
    pub fn settings_from_string(src_settings: &CameraConfig) -> Result<Settings, PyErr> {
        let mut settings = Settings::new();
        for (setting, src_value) in src_settings {
            let value = match setting.as_str() {
                "trigger_scheme" => Value::Scheme(
                    TriggerScheme::from_name(src_value).unwrap_or(DEFAULT_TRIGGER_SCHEME),
                ),
                "resolution" => Value::Resolution(resolution_from_str(src_value)?),
                "rotation" => Value::Py(PyValue::Int(int_from_str(src_value)?)),
                "exposure" | "focus" => Value::Py(PyValue::Float(
                    maker_connect_gcode_metadata::py::float_from_str(src_value)?,
                )),
                _ => Value::Py(PyValue::Str(src_value.clone())),
            };
            dict_set(&mut settings, setting, value);
        }
        Ok(settings)
    }

    /// Converts settings from one format into a dictionary with Capability compatible values
    /// (from JSON).
    pub fn settings_from_json(src_settings: &Dict) -> Result<Settings, PyErr> {
        let mut settings = Settings::new();
        for (setting, src_value) in src_settings.iter() {
            let value = match setting.as_str() {
                "trigger_scheme" => match src_value {
                    PyValue::List(_) | PyValue::Dict(_) => {
                        return Err(PyErr::TypeError(format!(
                            "unhashable type: '{}'",
                            src_value.type_name()
                        )));
                    }
                    PyValue::Str(name) => Value::Scheme(
                        TriggerScheme::from_name(name).unwrap_or(DEFAULT_TRIGGER_SCHEME),
                    ),
                    // KeyError
                    _ => Value::Scheme(DEFAULT_TRIGGER_SCHEME),
                },
                "resolution" => Value::Resolution(resolution_from_kwargs(src_value)?),
                _ => Value::Py(src_value.clone()),
            };
            dict_set(&mut settings, setting, value);
        }
        Ok(settings)
    }

    /// Converts settings into the strings of a config (`str(value)`, a trigger scheme's
    /// `.name`).
    pub fn string_from_settings(src_settings: &Settings) -> Result<CameraConfig, PyErr> {
        let mut settings = CameraConfig::new();
        for (setting, src_value) in src_settings {
            let value = if setting == CapabilityType::TriggerScheme.value() {
                match src_value {
                    Value::Scheme(scheme) => scheme.name().to_owned(),
                    other => return Err(no_name_attribute(other)),
                }
            } else {
                src_value.to_py_str()
            };
            dict_set(&mut settings, setting, value);
        }
        Ok(settings)
    }

    /// Converts settings into JSON values (a trigger scheme's `.name`, a resolution's
    /// `dict`).
    pub fn json_from_settings(src_settings: &Settings) -> Result<Dict, PyErr> {
        let mut settings = Dict::new();
        for (setting, src_value) in src_settings {
            let value = if setting == CapabilityType::TriggerScheme.value() {
                match src_value {
                    Value::Scheme(scheme) => PyValue::str(scheme.name()),
                    other => return Err(no_name_attribute(other)),
                }
            } else if setting == CapabilityType::Resolution.value() {
                match src_value {
                    Value::Resolution(resolution) => PyValue::Dict(resolution.to_dict()),
                    Value::Py(PyValue::Dict(dict)) => PyValue::Dict(dict.clone()),
                    other => {
                        return Err(PyErr::TypeError(format!(
                            "'{}' object is not iterable",
                            other.type_name()
                        )));
                    }
                }
            } else {
                match src_value {
                    Value::Py(value) => value.clone(),
                    other => {
                        return Err(not_covered(format!(
                            "a {} under {setting} in json_from_settings",
                            other.type_name()
                        )));
                    }
                }
            };
            settings.set(setting, value);
        }
        Ok(settings)
    }

    /// Gets the object representation of settings for conversion
    pub fn get_settings(&self) -> Result<Settings, Error> {
        let mut config = Settings::new();
        for capability_type in self.configurable_capabilities() {
            let value = self.get_value(capability_type)?;
            dict_set(&mut config, capability_type.value(), value);
        }
        dict_set(&mut config, "name", Value::Py(PyValue::Str(self.name()?)));
        if let Some(token) = self.token() {
            dict_set(&mut config, "token", Value::Py(PyValue::Str(token)));
        }
        Ok(config)
    }

    /// Tells the driver to update its config with new settings
    pub fn store(&self) -> Result<(), Error> {
        let settings = Camera::string_from_settings(&self.get_settings()?)?;
        self.driver.store_settings(&settings)
    }

    /// Asks the camera to disconnect
    pub fn disconnect(&self) -> Result<(), Error> {
        self.driver.disconnect()
    }

    // --- Private parts ---

    /// Notifies the SDK that a photo has been taken by this camera
    fn photo_handler(&self, snapshot: SharedSnapshot) -> Result<(), Error> {
        if !self.supports(CapabilityType::Imaging) {
            return Err(PyErr::Other(
                "RuntimeError",
                "Driver that does not support imaging has taken a photo".into(),
            )
            .into());
        }
        {
            let mut shot = snapshot.lock().unwrap();
            shot.camera_fingerprint = Some(self.fingerprint());
            shot.camera_token = self.token();
        }
        let photo_cb = self.photo_cb.lock().unwrap().clone();
        photo_cb(snapshot.clone())?;
        self.become_ready();
        let size = snapshot.lock().unwrap().data.as_ref().map_or(0, Vec::len);
        log::debug!(
            target: "camera",
            "A camera {} has taken a photo. ({size} bytes)",
            self.name().unwrap_or_default()
        );
        Ok(())
    }

    /// Makes the camera become busy
    pub(crate) fn become_busy(&self) {
        *self.ready.lock().unwrap() = false;
        *self.busy_since.lock().unwrap() = Some(clock::time());
    }

    /// Makes the camera become ready
    pub(crate) fn become_ready(&self) {
        *self.ready.lock().unwrap() = true;
        self.ready_cv.notify_all();
        *self.busy_since.lock().unwrap() = None;
    }
}

/// `AttributeError` of `value.name` on what is not an enum member.
fn no_name_attribute(value: &Value) -> PyErr {
    PyErr::AttributeError(format!(
        "'{}' object has no attribute 'name'",
        value.type_name()
    ))
}

/// `Resolution(*(int(val) for val in src_value.split("x")))`
pub fn resolution_from_str(src_value: &str) -> Result<Resolution, PyErr> {
    let mut values = Vec::new();
    for val in src_value.split('x') {
        values.push(int_from_str(val)?);
    }
    if values.len() == 1 {
        return Err(PyErr::TypeError(
            "Resolution.__init__() missing 1 required positional argument: 'height'".into(),
        ));
    }
    if values.len() > 2 {
        return Err(PyErr::TypeError(format!(
            "Resolution.__init__() takes 3 positional arguments but {} were given",
            values.len() + 1
        )));
    }
    let width = values[0].to_i64();
    let height = values[1].to_i64();
    match (width, height) {
        (Some(width), Some(height)) => Ok(Resolution::new(width, height)),
        _ => Err(not_covered(format!("the resolution {src_value}"))),
    }
}

/// `Resolution(**src_value)`
fn resolution_from_kwargs(src_value: &PyValue) -> Result<Resolution, PyErr> {
    let PyValue::Dict(kwargs) = src_value else {
        return Err(PyErr::TypeError(format!(
            "prusa.connect.printer.camera.Resolution() argument after ** must be a mapping, not \
             {}",
            src_value.type_name()
        )));
    };
    if let Some(unexpected) = kwargs.keys().find(|k| *k != "width" && *k != "height") {
        return Err(PyErr::TypeError(format!(
            "Resolution.__init__() got an unexpected keyword argument {}",
            repr_str(unexpected)
        )));
    }
    match (kwargs.get("width"), kwargs.get("height")) {
        (None, None) => Err(PyErr::TypeError(
            "Resolution.__init__() missing 2 required positional arguments: 'width' and 'height'"
                .into(),
        )),
        (Some(_), None) => Err(PyErr::TypeError(
            "Resolution.__init__() missing 1 required positional argument: 'height'".into(),
        )),
        (None, Some(_)) => Err(PyErr::TypeError(
            "Resolution.__init__() missing 1 required positional argument: 'width'".into(),
        )),
        (Some(PyValue::Int(width)), Some(PyValue::Int(height))) => {
            match (width.to_i64(), height.to_i64()) {
                (Some(width), Some(height)) => Ok(Resolution::new(width, height)),
                _ => Err(not_covered(format!("the resolution {}", src_value.repr()))),
            }
        }
        _ => Err(not_covered(format!("the resolution {}", src_value.repr()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_compares_as_python() {
        let a = Resolution::new(640, 480);
        let b = Resolution::new(480, 640);
        assert_ne!(a, b);
        assert!(!a.gt(&b) && !a.lt(&b) && a.ge(&b) && a.le(&b));
        assert!(Resolution::new(1280, 720).gt(&a));
        assert_eq!(a.reversed(), b);
        assert_eq!(a.to_string(), "640x480");
        // stable: ties keep their order
        let sorted = Resolution::sorted(&[Resolution::new(1280, 720), a, b]);
        assert_eq!(sorted, vec![a, b, Resolution::new(1280, 720)]);
    }

    #[test]
    fn values_compare_as_python() {
        assert_eq!(Value::Py(PyValue::int(0)), Value::Py(PyValue::Float(0.0)));
        assert_eq!(Value::Py(PyValue::Bool(true)), Value::Py(PyValue::int(1)));
        assert_ne!(
            Value::Scheme(TriggerScheme::Manual),
            Value::Py(PyValue::str("MANUAL"))
        );
        assert_eq!(Value::Py(PyValue::Float(1.0)).to_py_str(), "1.0");
        assert_eq!(
            Value::Scheme(TriggerScheme::TenSec).to_py_str(),
            "TriggerScheme.TEN_SEC"
        );
    }

    #[test]
    fn resolution_from_a_string() {
        assert_eq!(
            resolution_from_str("640x480"),
            Ok(Resolution::new(640, 480))
        );
        assert_eq!(
            resolution_from_str(" 640 x 480 "),
            Ok(Resolution::new(640, 480))
        );
        assert!(matches!(
            resolution_from_str("640"),
            Err(PyErr::TypeError(_))
        ));
        assert!(matches!(
            resolution_from_str("1x2x3"),
            Err(PyErr::TypeError(_))
        ));
        assert!(matches!(
            resolution_from_str("ax2"),
            Err(PyErr::ValueError(_))
        ));
    }

    #[test]
    fn range_checks() {
        assert_eq!(Value::Py(PyValue::Float(2.0)).in_range(-2, 2), Ok(true));
        assert_eq!(Value::Py(PyValue::Float(2.5)).in_range(-2, 2), Ok(false));
        assert_eq!(
            Value::Py(PyValue::Float(f64::NAN)).in_range(-2, 2),
            Ok(false)
        );
        assert_eq!(Value::Py(PyValue::Bool(true)).in_range(0, 1), Ok(true));
        assert!(Value::Py(PyValue::str("1")).in_range(0, 1).is_err());
    }
}
