//! `src/labelle/lib/devices/device_manager.py` (labelle 5e8f1e5): `DeviceManager`, the parts
//! used to find the printer (`scan`, `devices`, `matching_devices`, `find_and_select_device`).
//!
//! labelle: Copyright (c) 2011 Sebastian J. Bronner <waschtl@sbronner.com> (see
//! `usb_device.rs`).
//!
//! Ours: labelle keeps its devices in a `dict` filled from a `set` of hashes, so the order
//! they are inserted in (and so freed in) is undefined; here it is the hash's order.

use super::pyusb::{Backend, Error as PyusbError};
use super::usb_device::{self, UsbDevice};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

/// What the manager raises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `DeviceManagerError`.
    Manager(String),
    /// `DeviceManagerNoDevices` (a `DeviceManagerError`).
    NoDevices(String),
    /// What `UsbDevice` raises (`device_info` in `find_and_select_device`).
    UsbDevice(usb_device::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Manager(m) | Error::NoDevices(m) => f.write_str(m),
            Error::UsbDevice(e) => write!(f, "{e}"),
        }
    }
}

/// A device the manager keeps, shared with whoever selected it.
pub type SharedDevice = Rc<RefCell<UsbDevice>>;

/// `DeviceManager` (`device_manager.py:25`).
pub struct DeviceManager {
    backend: Rc<dyn Backend>,
    devices: BTreeMap<String, SharedDevice>,
}

impl DeviceManager {
    /// `__init__`.
    pub fn new(backend: Rc<dyn Backend>) -> DeviceManager {
        DeviceManager {
            backend,
            devices: BTreeMap::new(),
        }
    }

    /// `scan` (`device_manager.py:31`): whether the devices changed.
    pub fn scan(&mut self) -> Result<bool, Error> {
        let prev: Vec<String> = self.devices.keys().cloned().collect();
        let cur: Result<BTreeMap<String, UsbDevice>, PyusbError> =
            UsbDevice::supported_devices(&self.backend).map(|devices| {
                devices
                    .into_iter()
                    .map(|d| (d.hash(), d))
                    .filter(|(h, _)| !h.is_empty())
                    .collect()
            });
        let mut cur = match cur {
            Ok(cur) => cur,
            // POSSIBLE_USB_ERRORS = (NoBackendError, USBError)
            Err(e @ (PyusbError::Usb(_) | PyusbError::NoBackend(_))) => {
                self.devices.clear();
                return Err(Error::Manager(format!("Failed scanning devices: {e}")));
            }
            Err(e) => return Err(Error::UsbDevice(e.into())),
        };
        if cur.is_empty() {
            self.devices.clear();
            return Err(Error::NoDevices("No supported devices found".to_owned()));
        }
        let cur_keys: Vec<String> = cur.keys().cloned().collect();
        for dev in prev.iter().filter(|h| !cur_keys.contains(h)) {
            self.devices.remove(dev);
        }
        for dev in cur_keys.iter().filter(|h| !prev.contains(h)) {
            self.devices.insert(
                dev.clone(),
                Rc::new(RefCell::new(cur.remove(dev).expect("it is there"))),
            );
        }
        Ok(prev != cur_keys)
    }

    /// `devices` (`device_manager.py:54`): sorted by hash.
    pub fn devices(&self) -> Vec<SharedDevice> {
        self.devices.values().cloned().collect()
    }

    /// `matching_devices(None)` (`device_manager.py:60`): all of them, sorted by hash.
    pub fn matching_devices(&self) -> Vec<SharedDevice> {
        self.devices()
    }

    /// `find_and_select_device(None)` (`device_manager.py:69`).
    pub fn find_and_select_device(&self) -> Result<SharedDevice, Error> {
        let devices: Vec<SharedDevice> = self
            .matching_devices()
            .into_iter()
            .filter(|d| d.borrow().is_supported())
            .collect();
        if devices.is_empty() {
            return Err(Error::Manager("No matching devices found".to_owned()));
        }
        // `LOG.debug(dev.device_info)` evaluates the property for each of them.
        for dev in &devices {
            dev.borrow().device_info().map_err(Error::UsbDevice)?;
        }
        Ok(devices[0].clone())
    }
}
