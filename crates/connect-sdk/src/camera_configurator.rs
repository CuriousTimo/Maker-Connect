//! Implements the camera config management class (`camera_configurator.py`).
//!
//! Reference: the SDK at `c0d7d233` (branch `websockets`), `prusa/connect/printer/
//! camera_configurator.py`, ported function by function, with [`crate::configparser`] for its
//! `ConfigParser`.
//!
//! **Where this port differs:** `stored` and `detected` are Python sets of camera ids, iterated
//! in hash order, which changes per process: `_update_order` appends new cameras in that order.
//! Here the stored ones come first, in the order they were stored, then the detected ones in the
//! order they were detected. The bug of `_update_order` is kept: removing from the order while
//! iterating over it skips the camera after each one removed.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::ThreadId;

use maker_connect_gcode_metadata::py::{PyErr, repr_str};

use crate::camera::{Camera, Error, dict_get, dict_set};
use crate::camera_controller::CameraController;
use crate::camera_driver::{CameraDriver, DriverClass};
use crate::configparser::{self, ConfigParser};
use crate::consts::{CameraConfig, CameraConfigs, CameraError};

impl From<configparser::Error> for Error {
    fn from(err: configparser::Error) -> Error {
        Error::Py(match err.class {
            "ValueError" => PyErr::ValueError(err.message),
            class => PyErr::Other(class, err.message),
        })
    }
}

/// `multiprocessing.RLock()`: the thread that holds it may take it again.
struct RLock {
    owner: Mutex<(Option<ThreadId>, usize)>,
    released: Condvar,
}

struct RLockGuard<'a>(&'a RLock);

impl RLock {
    fn new() -> RLock {
        RLock {
            owner: Mutex::new((None, 0)),
            released: Condvar::new(),
        }
    }

    fn lock(&self) -> RLockGuard<'_> {
        let me = std::thread::current().id();
        let mut owner = self.owner.lock().unwrap();
        loop {
            match owner.0 {
                Some(holder) if holder == me => {
                    owner.1 += 1;
                    break;
                }
                None => {
                    *owner = (Some(me), 1);
                    break;
                }
                Some(_) => owner = self.released.wait(owner).unwrap(),
            }
        }
        RLockGuard(self)
    }
}

impl Drop for RLockGuard<'_> {
    fn drop(&mut self) {
        let mut owner = self.0.owner.lock().unwrap();
        owner.1 -= 1;
        if owner.1 == 0 {
            owner.0 = None;
            self.0.released.notify_one();
        }
    }
}

/// `repr(dict)` of a config
fn config_repr(config: &CameraConfig) -> String {
    let items: Vec<String> = config
        .iter()
        .map(|(k, v)| format!("{}: {}", repr_str(k), repr_str(v)))
        .collect();
    format!("{{{}}}", items.join(", "))
}

/// `set.add`
fn set_add(set: &mut Vec<String>, item: &str) {
    if !set.iter().any(|s| s == item) {
        set.push(item.to_owned());
    }
}

#[derive(Default)]
struct ConfiguratorState {
    /// Camera drivers that are loaded - even broken ones
    loaded: Vec<(String, Arc<CameraDriver>)>,
    /// A set of camera id's that are stored in the config. We know the order of these cameras
    stored: Vec<String>,
    detected: Vec<String>,
    /// Config hashes of detected cameras for comparison with stored ones
    hash_to_detected: Vec<(String, String)>,
    /// A list of camera IDs in descending order of importance
    order: Vec<String>,
    already_loaded_configs: bool,
}

impl ConfiguratorState {
    fn loaded(&self, camera_id: &str) -> Option<Arc<CameraDriver>> {
        self.loaded
            .iter()
            .find(|(id, _)| id == camera_id)
            .map(|(_, driver)| driver.clone())
    }
}

/// This component handles the configuration and instancing of cameras
///
/// Config handling explanation: the order of all loaded cameras + all stored cameras is saved.
/// If a user changes any setting on a camera its config gets saved. Order of unplugged cameras
/// that don't have a config shall be forgotten. Storing this order would accumulate all cameras
/// ever connected to PrusaLink, or we would need one order for saving and another for the
/// instance.
pub struct CameraConfigurator {
    pub camera_controller: Arc<CameraController>,
    pub config: Arc<Mutex<ConfigParser>>,
    pub config_file_path: PathBuf,
    lock: RLock,
    pub drivers: Vec<&'static DriverClass>,
    pub auto_detect: bool,
    state: Mutex<ConfiguratorState>,
    me: Weak<CameraConfigurator>,
}

impl CameraConfigurator {
    pub fn new(
        camera_controller: Arc<CameraController>,
        config: Arc<Mutex<ConfigParser>>,
        config_file_path: PathBuf,
        drivers: Vec<&'static DriverClass>,
        auto_detect: bool,
    ) -> Result<Arc<CameraConfigurator>, Error> {
        // `{driver.name: driver for driver in drivers}`: a later one of the same name wins
        let mut by_name: Vec<&'static DriverClass> = Vec::new();
        for driver in drivers {
            match by_name.iter_mut().find(|d| d.name == driver.name) {
                Some(entry) => *entry = driver,
                None => by_name.push(driver),
            }
        }
        let configurator = Arc::new_cyclic(|me| CameraConfigurator {
            camera_controller,
            config,
            config_file_path,
            lock: RLock::new(),
            drivers: by_name,
            auto_detect,
            state: Mutex::new(ConfiguratorState::default()),
            me: me.clone(),
        });
        configurator.load_cameras(true)?;
        Ok(configurator)
    }

    fn driver_class(&self, name: &str) -> Result<&'static DriverClass, PyErr> {
        self.drivers
            .iter()
            .copied()
            .find(|d| d.name == name)
            .ok_or_else(|| crate::camera::key_error(name))
    }

    /// The loaded drivers, in the order they were loaded (`self.loaded`).
    pub fn loaded(&self) -> Vec<(String, Arc<CameraDriver>)> {
        self.state.lock().unwrap().loaded.clone()
    }

    /// `self.order`
    pub fn order(&self) -> Vec<String> {
        self.state.lock().unwrap().order.clone()
    }

    /// `self.stored`
    pub fn stored(&self) -> Vec<String> {
        self.state.lock().unwrap().stored.clone()
    }

    /// `self.detected`
    pub fn detected(&self) -> Vec<String> {
        self.state.lock().unwrap().detected.clone()
    }

    // --- Public ----

    /// Is the camera already loaded and working?
    pub fn is_connected(&self, camera_id: &str) -> bool {
        // Don't answer until any ongoing operation is done
        let _lock = self.lock.lock();
        let driver = self.state.lock().unwrap().loaded(camera_id);
        driver.is_some_and(|driver| driver.is_connected())
    }

    /// Adds a camera into the configurator instance and if it works adds it to the SDK
    /// CameraController. Saves its config. Modifies the loaded config if it exists. raises
    /// ConfigError when a bad config is supplied
    pub fn add_camera(&self, camera_id: &str, config: &CameraConfig) -> Result<(), Error> {
        if self.is_connected(camera_id) {
            return Err(CameraError::CameraAlreadyConnected(format!(
                "A camera with id {camera_id} seems to be already working"
            ))
            .into());
        }
        let _lock = self.lock.lock();
        if !self.is_config_valid(config) {
            return Err(CameraError::ConfigError(format!(
                "Camera config is not valid {}",
                config_repr(config)
            ))
            .into());
        }
        // If valid, store the config
        self.store_config(camera_id, config)?;
        self.load_driver(camera_id, config)
    }

    /// Loads the cameras from config. Run with load_configs = True only once!
    pub fn load_cameras(&self, load_configs: bool) -> Result<(), Error> {
        let _lock = self.lock.lock();
        let config_dict = if load_configs {
            {
                let mut state = self.state.lock().unwrap();
                if state.already_loaded_configs {
                    return Err(PyErr::Other(
                        "RuntimeError",
                        "Loading configs more than once is not supported".into(),
                    )
                    .into());
                }
                state.already_loaded_configs = true;
            }
            let (order, config_dict) = self.get_configs()?;
            {
                let mut state = self.state.lock().unwrap();
                state.order = order;
                state.stored = Vec::new();
                for (camera_id, _) in &config_dict {
                    set_add(&mut state.stored, camera_id);
                }
            }
            self.update_order()?;
            config_dict
        } else {
            self.get_loaded_configs()
        };

        let mut detected_configs = CameraConfigs::new();
        if self.auto_detect {
            detected_configs = self.get_detected_cameras()?;
            let hash_to_detected = self.extract_hash_pairings(&detected_configs)?;
            {
                let mut state = self.state.lock().unwrap();
                state.detected = Vec::new();
                for (camera_id, _) in &detected_configs {
                    set_add(&mut state.detected, camera_id);
                }
                state.hash_to_detected = hash_to_detected;
            }
            self.update_order()?;
        }

        let updated_configs = self.get_updated_configs(&config_dict, &detected_configs)?;
        // Filters additional detected cameras while already running
        let new_configs = self.filter_new_configs(&updated_configs);

        for (camera_id, config) in &new_configs {
            self.load_driver(camera_id, config)?;
        }
        Ok(())
    }

    /// Resets any camera to default settings - removes its non-essential config values
    pub fn reset_to_defaults(&self, camera_id: &str) -> Result<(), Error> {
        let _lock = self.lock.lock();
        let Some(loaded_driver) = self.state.lock().unwrap().loaded(camera_id) else {
            return Err(CameraError::CameraNotFound(
                "Cannot factory reset non-loaded cameras".into(),
            )
            .into());
        };

        let config = loaded_driver.config();
        let required_settings = CameraDriver::get_required_settings(loaded_driver.class());
        let essential_config: CameraConfig = config
            .into_iter()
            .filter(|(key, _)| required_settings.contains(&key.as_str()))
            .collect();
        self.remove_camera_everywhere(camera_id)?;
        if let Err(err) = self.add_camera(camera_id, &essential_config) {
            log::error!(
                target: "camera_configurator",
                "Could not load the camera {camera_id} after config reset. New config: {}\n{err}",
                config_repr(&essential_config)
            );
        }
        Ok(())
    }

    /// If the camera is not detected, removes it from everywhere
    pub fn remove_camera(&self, camera_id: &str) -> Result<(), Error> {
        let _lock = self.lock.lock();
        if self
            .state
            .lock()
            .unwrap()
            .detected
            .iter()
            .any(|id| id == camera_id)
        {
            return Err(PyErr::Other(
                "RuntimeError",
                "Cannot remove an auto added camera. It would just re-add itself anyway.".into(),
            )
            .into());
        }
        self.remove_camera_everywhere(camera_id)
    }

    /// Stores the current camera order into the config
    pub fn store_order(&self) -> Result<(), Error> {
        let _lock = self.lock.lock();
        let order = self.state.lock().unwrap().order.clone();
        {
            let mut config = self.config.lock().unwrap();
            config.remove_section("camera_order");
            config.add_section("camera_order")?;
            for (i, camera_id) in order.iter().enumerate() {
                config.set("camera_order", &(i + 1).to_string(), camera_id)?;
            }
        }
        self.write_config()
    }

    /// Moves the specified ids to the front, does not add any
    pub fn set_order(&self, order: &[String]) -> Result<(), Error> {
        let _lock = self.lock.lock();
        let new_order = {
            let state = self.state.lock().unwrap();
            let known = |id: &String| state.stored.contains(id) || state.detected.contains(id);
            let mut new_order: Vec<String> = Vec::new();
            // Put new ones that are configured at the start
            for camera_id in order {
                if !known(camera_id) {
                    continue;
                }
                new_order.push(camera_id.clone());
            }

            // Copy over the rest of the list
            for camera_id in &state.order {
                if !new_order.contains(camera_id) {
                    new_order.push(camera_id.clone());
                }
            }
            new_order
        };
        self.state.lock().unwrap().order = new_order.clone();
        self.camera_controller.set_camera_order(new_order);
        self.store_order()
    }

    /// Adds the loaded camera to the config
    pub fn store(&self, camera_id: &str) -> Result<(), Error> {
        if self.state.lock().unwrap().loaded(camera_id).is_none() {
            return Err(
                CameraError::CameraNotFound("Cannot store an unknown camera".into()).into(),
            );
        }
        let _lock = self.lock.lock();
        let loaded_driver = self
            .state
            .lock()
            .unwrap()
            .loaded(camera_id)
            .ok_or_else(|| crate::camera::key_error(camera_id))?;
        let config = loaded_driver.config();

        self.store_config(camera_id, &config)
    }

    // --- Private ---

    /// Removes the camera from absolutely everywhere
    fn remove_camera_everywhere(&self, camera_id: &str) -> Result<(), Error> {
        if self.camera_controller.contains(camera_id) {
            let camera = self.camera_controller.get_camera(camera_id)?;
            camera.disconnect()?;
        }
        {
            let mut state = self.state.lock().unwrap();
            state.loaded.retain(|(id, _)| id != camera_id);
            state.stored.retain(|id| id != camera_id);
        }
        let section_name = format!("camera::{camera_id}");
        {
            let mut config = self.config.lock().unwrap();
            if config.has_section(&section_name) {
                config.remove_section(&section_name);
            }
        }
        self.write_config()?;
        self.update_order()
    }

    /// Asks all drivers to detect cameras, returns their configs
    fn get_detected_cameras(&self) -> Result<CameraConfigs, PyErr> {
        let mut scanned = CameraConfigs::new();
        for driver in &self.drivers {
            for (camera_id, config) in CameraDriver::scan(driver)? {
                dict_set(&mut scanned, &camera_id, config);
            }
        }
        Ok(scanned)
    }

    fn extract_hash_pairings(
        &self,
        config_dict: &CameraConfigs,
    ) -> Result<Vec<(String, String)>, PyErr> {
        let mut hash_pairings = Vec::new();
        for (camera_id, config) in config_dict {
            let driver_name =
                dict_get(config, "driver").ok_or_else(|| crate::camera::key_error("driver"))?;
            let config_hash =
                CameraDriver::get_config_hash(self.driver_class(driver_name)?, config)?;
            dict_set(&mut hash_pairings, &config_hash, camera_id.clone());
        }
        Ok(hash_pairings)
    }

    /// Returns configs of all loaded cameras
    fn get_loaded_configs(&self) -> CameraConfigs {
        self.state
            .lock()
            .unwrap()
            .loaded
            .iter()
            .map(|(camera_id, driver)| (camera_id.clone(), driver.config()))
            .collect()
    }

    /// Only allow configs for unknown or broken cameras
    fn filter_new_configs(&self, config_dict: &CameraConfigs) -> CameraConfigs {
        config_dict
            .iter()
            .filter(|(camera_id, _)| !self.is_connected(camera_id))
            .cloned()
            .collect()
    }

    /// Updates the supplied config using auto-detected config values. If we detect a camera
    /// with the same ID we update its config, for example path, to reflect this change
    fn get_updated_configs(
        &self,
        config_dict: &CameraConfigs,
        detected_configs: &CameraConfigs,
    ) -> Result<CameraConfigs, PyErr> {
        let mut config_dict = config_dict.clone();
        for (camera_id, config) in detected_configs {
            match config_dict.iter_mut().find(|(id, _)| id == camera_id) {
                None => config_dict.push((camera_id.clone(), config.clone())),
                Some((_, stored)) => {
                    let driver_name = dict_get(config, "driver")
                        .ok_or_else(|| crate::camera::key_error("driver"))?;
                    let driver = self.driver_class(driver_name)?;
                    for (setting_name, setting) in config {
                        if driver
                            .requires_settings
                            .iter()
                            .any(|(key, _)| key == setting_name)
                        {
                            dict_set(stored, setting_name, setting.clone());
                        }
                    }
                }
            }
        }
        // Return only configs, that were auto-detected or stored
        let stored = self.state.lock().unwrap().stored.clone();
        Ok(config_dict
            .into_iter()
            .filter(|(camera_id, _)| {
                dict_get(detected_configs, camera_id).is_some() || stored.contains(camera_id)
            })
            .collect())
    }

    /// Returns order and camera settings from the supplied config
    fn get_configs(&self) -> Result<(Vec<String>, CameraConfigs), Error> {
        let mut config = self.config.lock().unwrap();
        let mut order = Vec::new();
        if !config.has_section("camera_order") {
            config.add_section("camera_order")?;
        }
        let mut indexes = config.options("camera_order")?;
        indexes.sort();
        // Lose absolute numbering
        for index in indexes {
            let camera_id = config.get("camera_order", &index)?;
            order.push(camera_id);
        }

        // Load actual settings
        let mut config_dict = CameraConfigs::new();
        for name in config.sections() {
            let Some(camera_id) = name.split_once("::").map(|(_, id)| id.to_owned()) else {
                continue;
            };
            if !name.starts_with("camera::") {
                continue;
            }

            let camera_config: CameraConfig = config.items(&name)?;
            if !self.is_config_valid(&camera_config) {
                log::warn!(
                    target: "camera_configurator",
                    "Skipping loading config for camera ID: {camera_id}, because it's not valid"
                );
                continue;
            }
            dict_set(&mut config_dict, &camera_id, camera_config);
        }

        Ok((order, config_dict))
    }

    /// Returns True if the config is valid
    fn is_config_valid(&self, camera_config: &CameraConfig) -> bool {
        let Some(driver_name) = dict_get(camera_config, "driver") else {
            log::warn!(target: "camera_configurator", "Config does not contain which driver to use");
            return false;
        };
        let Ok(driver) = self.driver_class(driver_name) else {
            log::warn!(
                target: "camera_configurator",
                "Config specified an unknown driver {driver_name}"
            );
            return false;
        };
        if !(driver.is_config_valid)(driver, camera_config) {
            log::warn!(
                target: "camera_configurator",
                "<class '{}'> driver config validation failed",
                driver.class_name
            );
            return false;
        }
        true
    }

    /// Makes order reflect known stored and detected cameras, propagates any changes into the
    /// controller and the config
    fn update_order(&self) -> Result<(), Error> {
        let order = {
            let mut state = self.state.lock().unwrap();
            let known: Vec<String> = state
                .stored
                .iter()
                .chain(
                    state
                        .detected
                        .iter()
                        .filter(|id| !state.stored.contains(id)),
                )
                .cloned()
                .collect();
            for camera_id in &known {
                if !state.order.contains(camera_id) {
                    state.order.push(camera_id.clone());
                }
            }

            // `for camera_id in self.order: ... self.order.remove(camera_id)`: the list
            // iterator goes on at the next index, past the camera that moved into this one
            let mut index = 0;
            while index < state.order.len() {
                let camera_id = state.order[index].clone();
                if !known.contains(&camera_id) {
                    let first = state
                        .order
                        .iter()
                        .position(|id| *id == camera_id)
                        .expect("it is in the list");
                    state.order.remove(first);
                }
                index += 1;
            }
            state.order.clone()
        };

        self.camera_controller.set_camera_order(order);
        self.store_order()
    }

    /// Loads the camera's driver and if the camera's config is not conflicting with any
    /// detected one, tries connecting to it. If all goes well, passes the it to the
    /// CameraController as working
    fn load_driver(&self, camera_id: &str, config: &CameraConfig) -> Result<(), Error> {
        let driver_name =
            dict_get(config, "driver").ok_or_else(|| crate::camera::key_error("driver"))?;
        let driver = self.driver_class(driver_name)?;
        let config_hash = CameraDriver::get_config_hash(driver, config)?;

        let controller = self.camera_controller.clone();
        let loaded_driver = CameraDriver::new(
            driver,
            camera_id,
            config.clone(),
            Arc::new(move |loaded_driver: &CameraDriver| {
                // This camera is defunct, remove it from SDK cameras
                controller.remove_camera(&loaded_driver.camera_id())
            }),
        );
        let me = self.me.clone();
        loaded_driver.set_store_cb(Arc::new(move |camera_id: &str| match me.upgrade() {
            Some(configurator) => configurator.store(camera_id),
            None => Ok(()),
        }));
        {
            let mut state = self.state.lock().unwrap();
            match state.loaded.iter_mut().find(|(id, _)| id == camera_id) {
                Some(entry) => entry.1 = loaded_driver.clone(),
                None => state
                    .loaded
                    .push((camera_id.to_owned(), loaded_driver.clone())),
            }
        }

        // Proceed only if there's no detected camera with the same config, unless we are that
        // camera
        let detected =
            dict_get(&self.state.lock().unwrap().hash_to_detected, &config_hash).cloned();
        if detected.is_some_and(|id| id != camera_id) {
            return Ok(());
        }

        loaded_driver.connect()?;

        if loaded_driver.is_connected() {
            let camera = Camera::new(loaded_driver)?;
            self.camera_controller.add_camera(camera.clone())?;
            // Take the first photo right away
            camera.trigger_a_photo(None)?;
        }
        Ok(())
    }

    /// Stores the config given to it, doesn't validate
    fn store_config(&self, camera_id: &str, config: &CameraConfig) -> Result<(), Error> {
        let section_name = format!("camera::{camera_id}");

        {
            let mut parser = self.config.lock().unwrap();
            if !parser.has_section(&section_name) {
                parser.add_section(&section_name)?;
            }

            parser.read_dict(&[(section_name, config.clone())], "<dict>")?;
        }
        set_add(&mut self.state.lock().unwrap().stored, camera_id);
        self.write_config()?;

        self.update_order()
    }

    /// Writes the current ConfigParser config instance to the file
    fn write_config(&self) -> Result<(), Error> {
        self.config
            .lock()
            .unwrap()
            .write_file(&self.config_file_path)?;
        Ok(())
    }
}
