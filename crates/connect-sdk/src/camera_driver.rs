//! Implementation of the base CameraDriver (`camera_driver.py`).
//!
//! Reference: the SDK at `c0d7d233` (branch `websockets`), `prusa/connect/printer/
//! camera_driver.py`, ported function by function.
//!
//! A Python driver is a subclass of `CameraDriver`. Here the base class is [`CameraDriver`], and a
//! subclass is a [`DriverClass`] (its class attributes and class methods) with a [`DriverImpl`]
//! (the methods it overrides). The subclass's `__init__` is [`DriverClass::init`]: it gets the
//! base's state, as `super().__init__()` left it, to fill in `_capabilities` and
//! `_available_resolutions`.
//!
//! **Where this port differs:** `get_required_settings` is a set in Python, iterated in hash
//! order (the order of its log line); here `ALWAYS_REQURIED` first, then `REQUIRES_SETTINGS`.
//! The class attribute `name` is required by [`DriverClass`], so the `ValueError` "Name your
//! driver" cannot happen.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use base64::Engine as _;
use blake2::digest::{Update, VariableOutput};
use maker_connect_gcode_metadata::py::PyErr;

use crate::camera::{
    Camera, Error, PhotoCb, Resolution, SharedSnapshot, Snapshot, Value, dict_get, dict_set,
};
use crate::consts::{ALWAYS_REQURIED, CameraConfig, CameraConfigs, CameraError, CapabilityType};
use crate::util::get_timestamp;

/// The default implementation for drivers, so any non-overriden methods that should have been
/// overriden raise right away (the SDK's message, with its arguments as they are).
pub fn not_implemented(driver: &CameraDriver, setting_name: &str) -> PyErr {
    PyErr::Other(
        "NotImplementedError",
        format!(
            "The driver {setting_name} told us it supports setting {}, but does not actually \
             implement it",
            driver.class().class_name
        ),
    )
}

/// The class side of a driver: its class attributes and the class methods it may override.
pub struct DriverClass {
    /// A driver name
    pub name: &'static str,
    /// The Python class name (`driver.__class__.__name__`)
    pub class_name: &'static str,
    /// Keys are the keys of the dictionary needed to instance the driver. Values are
    /// human-readable hints.
    pub requires_settings: &'static [(&'static str, &'static str)],
    /// `_scan`: return only valid configs that can be used to instance your driver as they are,
    /// keyed by the camera's plaintext id. No need to supply 'driver' in these configs, it will
    /// get filled by the base class
    pub scan: fn() -> Result<CameraConfigs, PyErr>,
    /// `is_config_valid`; [`CameraDriver::base_is_config_valid`] unless overridden.
    pub is_config_valid: fn(&'static DriverClass, &CameraConfig) -> bool,
    /// The subclass's `__init__` after `super().__init__()`.
    pub init: fn(&mut DriverState) -> Box<dyn DriverImpl>,
}

/// The `_scan` of the base class
pub fn no_scan() -> Result<CameraConfigs, PyErr> {
    Ok(CameraConfigs::new())
}

/// The methods a driver overrides. Each gets the driver, for its state.
pub trait DriverImpl: Send + Sync {
    /// Put all your connecting code over here
    fn connect(&self, _driver: &CameraDriver) -> Result<(), PyErr> {
        Err(PyErr::Other(
            "NotImplementedError",
            "Your driver is missing a _connect method".into(),
        ))
    }

    /// If your driver needs to handle a disconnect, override this
    fn disconnect(&self, _driver: &CameraDriver) -> Result<(), PyErr> {
        Ok(())
    }

    /// Override this, with your resolution setting method
    fn set_resolution(&self, driver: &CameraDriver, _resolution: &Resolution) -> Result<(), PyErr> {
        Err(not_implemented(driver, "resolution"))
    }

    /// Override this, with your rotation setting method
    fn set_rotation(&self, driver: &CameraDriver, _rotation: &Value) -> Result<(), PyErr> {
        Err(not_implemented(driver, "rotation"))
    }

    /// Override this, with your exposure setting method
    fn set_exposure(&self, driver: &CameraDriver, _exposure: &Value) -> Result<(), PyErr> {
        Err(not_implemented(driver, "exposure"))
    }

    /// Override this, with your focus setting method
    fn set_focus(&self, driver: &CameraDriver, _focus: &Value) -> Result<(), PyErr> {
        Err(not_implemented(driver, "focus"))
    }

    /// Takes a photo and returns it. Can block
    fn take_a_photo(&self, _driver: &CameraDriver) -> Result<Vec<u8>, PyErr> {
        Err(PyErr::Other("NotImplementedError", String::new()))
    }
}

/// The base class's attributes.
#[derive(Debug, Clone, Default)]
pub struct DriverState {
    pub camera_id: String,
    pub config: CameraConfig,
    pub connected: bool,
    pub capabilities: Vec<CapabilityType>,
    pub available_resolutions: Vec<Resolution>,
    /// For web to show a preview even if the camera does not work right now
    pub last_snapshot: Option<SharedSnapshot>,
}

/// `disconnected_cb`: called with the driver.
pub type DisconnectedCb = Arc<dyn Fn(&CameraDriver) -> Result<(), Error> + Send + Sync>;
/// `store_cb`: called with the camera id.
pub type StoreCb = Arc<dyn Fn(&str) -> Result<(), Error> + Send + Sync>;

/// The base class for a compatibility layer between the camera and the SDK
///
/// No class should directly interact with drivers if possible. These are expected to be managed
/// by the CameraConfigurator
pub struct CameraDriver {
    class: &'static DriverClass,
    state: Mutex<DriverState>,
    photo_cb: Mutex<PhotoCb>,
    disconnected_cb: DisconnectedCb,
    store_cb: Mutex<StoreCb>,
    photo_thread: Mutex<Option<JoinHandle<()>>>,
    imp: Box<dyn DriverImpl>,
}

impl CameraDriver {
    /// Instances the driver setting default values to everything. A cameraDriver is not
    /// supposed to raise on init, just don't set connected to True
    pub fn new(
        class: &'static DriverClass,
        camera_id: &str,
        config: CameraConfig,
        disconnected_cb: DisconnectedCb,
    ) -> Arc<CameraDriver> {
        let mut state = DriverState {
            camera_id: camera_id.to_owned(),
            config,
            ..DriverState::default()
        };
        let imp = (class.init)(&mut state);
        Arc::new(CameraDriver {
            class,
            state: Mutex::new(state),
            photo_cb: Mutex::new(Arc::new(|_| Ok(()))),
            disconnected_cb,
            store_cb: Mutex::new(Arc::new(|_| Ok(()))),
            photo_thread: Mutex::new(None),
            imp,
        })
    }

    pub fn class(&self) -> &'static DriverClass {
        self.class
    }

    pub fn set_photo_cb(&self, cb: PhotoCb) {
        *self.photo_cb.lock().unwrap() = cb;
    }

    pub fn set_store_cb(&self, cb: StoreCb) {
        *self.store_cb.lock().unwrap() = cb;
    }

    /// The base's state, for a driver's own methods.
    pub fn state(&self) -> std::sync::MutexGuard<'_, DriverState> {
        self.state.lock().unwrap()
    }

    /// Tells the driver to try connecting to its camera
    pub fn connect(&self) -> Result<(), Error> {
        if !(self.class.is_config_valid)(self.class, &self.config()) {
            return Err(CameraError::ConfigError(
                "Can't instance a driver because some essential config values are missing, or \
                 are incorrect"
                    .into(),
            )
            .into());
        }
        match self.imp.connect(self) {
            Err(err) => {
                log::error!(
                    target: "camera_driver",
                    "Initialization of camera {} has failed\n{err}",
                    self.config_value("name").unwrap_or_else(|| "unknown".to_owned())
                );
                self.disconnect()?;
            }
            Ok(()) => self.state().connected = true,
        }
        Ok(())
    }

    /// Gets the configured resolution and validates it, if invalid gives the highest possible
    /// one
    pub fn get_initial_resolution(
        available_resolutions: &[Resolution],
        config: &CameraConfig,
    ) -> Result<Resolution, PyErr> {
        let highest_resolution = *Resolution::sorted(available_resolutions)
            .last()
            .ok_or_else(|| PyErr::Other("IndexError", "list index out of range".into()))?;
        let mut configured_resolution = highest_resolution;
        if let Some(resolution) = dict_get(config, "resolution") {
            let setting = vec![("resolution".to_owned(), resolution.clone())];
            configured_resolution = match Camera::settings_from_string(&setting) {
                Ok(settings) => match dict_get(&settings, "resolution") {
                    Some(Value::Resolution(resolution)) => *resolution,
                    _ => highest_resolution,
                },
                Err(PyErr::TypeError(_) | PyErr::ValueError(_)) => highest_resolution,
                Err(err) => return Err(err),
            };
        }
        if !available_resolutions.contains(&configured_resolution) {
            configured_resolution = highest_resolution;
        }
        Ok(configured_resolution)
    }

    /// Hashes the camera ID: blake2b of 9 bytes of its latin-1, urlsafe base64.
    pub fn make_hash(plaintext_id: &str) -> Result<String, PyErr> {
        let mut latin1 = Vec::with_capacity(plaintext_id.len());
        for (position, c) in plaintext_id.chars().enumerate() {
            let code = u32::from(c);
            if code > 0xFF {
                return Err(PyErr::Other(
                    "UnicodeEncodeError",
                    format!(
                        "'latin-1' codec can't encode character {} in position {position}: \
                         ordinal not in range(256)",
                        unicode_escape(c)
                    ),
                ));
            }
            latin1.push(code as u8);
        }
        let mut hasher = blake2::Blake2bVar::new(9).expect("9 is a valid blake2b digest size");
        hasher.update(&latin1);
        let mut hashed_id = [0u8; 9];
        hasher
            .finalize_variable(&mut hashed_id)
            .expect("the buffer is the digest size");
        Ok(base64::engine::general_purpose::URL_SAFE.encode(hashed_id))
    }

    /// Returns available cameras as a dictionary, where the key is the camera's ID and the
    /// value contains a dictionary with config options needed to instance such a camera
    pub fn scan(class: &'static DriverClass) -> Result<CameraConfigs, PyErr> {
        let mut valid = CameraConfigs::new();
        match (class.scan)() {
            Err(err) => {
                log::error!(
                    target: "camera_driver",
                    "Error while scanning for {} cameras\n{err}",
                    class.name
                );
            }
            Ok(available) => {
                for (plaintext_id, mut config) in available {
                    let camera_id = CameraDriver::make_hash(&plaintext_id)?;

                    // Fill in this required config option for all drivers
                    if dict_get(&config, "driver").is_none() {
                        dict_set(&mut config, "driver", class.name.to_owned());
                    }

                    if dict_get(&config, "trigger_scheme").is_some() {
                        log::warn!(
                            target: "camera_driver",
                            "Camera drivers are not supposed to specify trigger scheme"
                        );
                    }

                    if !(class.is_config_valid)(class, &config) {
                        continue;
                    }
                    dict_set(&mut valid, &camera_id, config);
                }
            }
        }
        Ok(valid)
    }

    /// Returns the sum of always required and driver specific config options
    pub fn get_required_settings(class: &DriverClass) -> Vec<&'static str> {
        let mut required: Vec<&'static str> = Vec::new();
        for (key, _) in ALWAYS_REQURIED.iter().chain(class.requires_settings) {
            if !required.contains(key) {
                required.push(key);
            }
        }
        required
    }

    /// Returns a hash of this driver's specific config values
    pub fn get_config_hash(class: &DriverClass, config: &CameraConfig) -> Result<String, PyErr> {
        let mut config_values = vec![class.name.to_owned()];
        // sorted so the dict keys are in the same order every time
        let mut keys: Vec<&str> = class
            .requires_settings
            .iter()
            .map(|(key, _)| *key)
            .collect();
        keys.sort_unstable();
        keys.dedup();
        for key in keys {
            let Some(value) = dict_get(config, key) else {
                log::warn!(target: "camera_driver", "Use get_config_hash only on valid configs!");
                continue;
            };
            config_values.push(key.to_owned());
            config_values.push(value.clone());
        }
        let config_string = config_values.concat();
        CameraDriver::make_hash(&config_string)
    }

    /// Validates the supplied config, returns True if passed. A driver may override it
    /// ([`DriverClass::is_config_valid`]) and add specific checks.
    pub fn base_is_config_valid(class: &'static DriverClass, config: &CameraConfig) -> bool {
        let missing_settings: Vec<&str> = CameraDriver::get_required_settings(class)
            .into_iter()
            .filter(|key| dict_get(config, key).is_none())
            .collect();
        if !missing_settings.is_empty() {
            log::warn!(
                target: "camera_driver",
                "The camera driver {} is missing these settings {}",
                class.name,
                missing_settings.join(", ")
            );
        }
        missing_settings.is_empty()
    }

    /// Called when a camera breaks down or disconnects. Does not raise, unless its
    /// `disconnected_cb` does.
    pub fn disconnect(&self) -> Result<(), Error> {
        if let Err(err) = self.imp.disconnect(self) {
            log::error!(
                target: "camera_driver",
                "Driver {} for a camera {} threw an error while disconnecting\n{err}",
                self.class.name,
                self.camera_id()
            );
        }
        let was_connected = std::mem::replace(&mut self.state().connected, false);
        if was_connected {
            // If we got stuck taking a photo and are returning late, the driver is already
            // stopped, so we must not call the disconnected callback, otherwise we'd break
            // another working camera
            (self.disconnected_cb)(self)?;
        }
        Ok(())
    }

    // --- Setting change handlers ---
    // These get called when the camera object wants to change settings

    /// Handles a name change from the Camera object
    pub fn set_name(&self, name: &str) {
        dict_set(&mut self.state().config, "name", name.to_owned());
    }

    pub fn set_resolution(&self, resolution: &Resolution) -> Result<(), PyErr> {
        self.imp.set_resolution(self, resolution)
    }

    pub fn set_rotation(&self, rotation: &Value) -> Result<(), PyErr> {
        self.imp.set_rotation(self, rotation)
    }

    pub fn set_exposure(&self, exposure: &Value) -> Result<(), PyErr> {
        self.imp.set_exposure(self, exposure)
    }

    pub fn set_focus(&self, focus: &Value) -> Result<(), PyErr> {
        self.imp.set_focus(self, focus)
    }

    /// This method is not allowed to block, it just creates a new thread and runs it
    pub fn trigger(self: &Arc<Self>, snapshot: Option<SharedSnapshot>) {
        let snapshot = snapshot.unwrap_or_else(|| {
            let mut snapshot = Snapshot::new();
            snapshot.camera_id = Some(self.camera_id());
            Arc::new(Mutex::new(snapshot))
        });
        let driver = self.clone();
        let thread = std::thread::Builder::new()
            .name("Photographer".into())
            .spawn(move || driver.photo_taker(snapshot))
            .expect("the system can start a thread");
        *self.photo_thread.lock().unwrap() = Some(thread);
    }

    /// The thread target, calls the blocking photo taking method and catches errors. If a
    /// camera errors out while taking a photo it's considered disconnected. What it does not
    /// catch ends the thread, as Python's `threading.excepthook` prints it.
    fn photo_taker(&self, snapshot: SharedSnapshot) {
        let result = match self.imp.take_a_photo(self) {
            Err(err) => {
                log::error!(
                    target: "camera_driver",
                    "The driver {} broke while taking a photo. Disconnecting\n{err}",
                    self.class.name
                );
                self.disconnect()
            }
            Ok(data) => {
                {
                    let mut shot = snapshot.lock().unwrap();
                    shot.data = Some(data);
                    shot.timestamp = Some(get_timestamp(None));
                }
                self.state().last_snapshot = Some(snapshot.clone());
                let photo_cb = self.photo_cb.lock().unwrap().clone();
                photo_cb(snapshot)
            }
        };
        if let Err(err) = result {
            log::error!("Exception in thread Photographer:\n{err}");
        }
    }

    /// Stores only the camera settings, not its configuration
    pub fn store_settings(&self, config: &CameraConfig) -> Result<(), Error> {
        let forbidden: Vec<&str> = CameraDriver::get_required_settings(self.class)
            .into_iter()
            .filter(|key| *key != "name")
            .collect();
        if config
            .iter()
            .any(|(key, _)| forbidden.contains(&key.as_str()))
        {
            return Err(PyErr::Other(
                "RuntimeError",
                "Cannot set essential config values using SDK setting setter!".into(),
            )
            .into());
        }
        {
            let mut state = self.state();
            for (key, value) in config {
                dict_set(&mut state.config, key, value.clone());
            }
        }
        let store_cb = self.store_cb.lock().unwrap().clone();
        store_cb(&self.camera_id())
    }

    // --- Properties ----

    /// Returns whether the camera is connected. Return True if the camera driver connected to a
    /// real camera Or when you can't tell (on GPIO pins)
    pub fn is_connected(&self) -> bool {
        self.state().connected
    }

    /// Returns the last photo the camera has taken - None by default
    pub fn last_snapshot(&self) -> Option<SharedSnapshot> {
        self.state().last_snapshot.clone()
    }

    /// Returns the camera_id from settings
    pub fn camera_id(&self) -> String {
        self.state().camera_id.clone()
    }

    /// The capabilities supported by the device. The minimum is supporting TRIGGER_SCHEME
    /// (ability to trigger a camera)
    pub fn capabilities(&self) -> Vec<CapabilityType> {
        self.state().capabilities.clone()
    }

    /// Returns the available resolutions of the camera
    pub fn available_resolutions(&self) -> Vec<Resolution> {
        self.state().available_resolutions.clone()
    }

    /// A dictionary with all the supported camera setting defaults
    pub fn config(&self) -> CameraConfig {
        self.state().config.clone()
    }

    /// `self.config[key]`, or `None` where Python raises `KeyError`.
    pub fn config_value(&self, key: &str) -> Option<String> {
        dict_get(&self.state().config, key).cloned()
    }
}

/// How `repr()` writes a character a codec cannot encode: `'Ā'`, `'\U0001f600'`.
fn unicode_escape(c: char) -> String {
    let code = u32::from(c);
    if code <= 0xFFFF {
        format!("'\\u{code:04x}'")
    } else {
        format!("'\\U{code:08x}'")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `CameraDriver.make_hash(...)` printed by CPython 3.13.12.
    #[test]
    fn make_hash_is_blake2b_9() {
        assert_eq!(CameraDriver::make_hash("abc").unwrap(), "cGh9qmFXryfc");
        assert_eq!(
            CameraDriver::make_hash("/dev/video0").unwrap(),
            "3HbzT17wU6_C"
        );
        assert_eq!(CameraDriver::make_hash("").unwrap(), "1r1vyaMyTl8y");
        assert_eq!(
            CameraDriver::make_hash("aĀ").unwrap_err().message(),
            "'latin-1' codec can't encode character '\\u0100' in position 1: ordinal not in \
             range(256)"
        );
    }
}
