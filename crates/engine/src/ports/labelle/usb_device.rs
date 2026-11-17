//! `src/labelle/lib/devices/usb_device.py` (labelle 5e8f1e5): `UsbDevice`, function by
//! function, over [`pyusb`](super::pyusb).
//!
//! labelle: Copyright (c) 2011 Sebastian J. Bronner <waschtl@sbronner.com>; the file's own notice:
//! "Copying and distribution of this file, with or without modification, are permitted in any
//! medium without royalty provided the copyright notice and this notice are preserved."
//!
//! Not ported: `hash` (a plain alias of `usb_id`: used as `usb_id`), `serial_number` and the
//! text of `device_info` (it only goes to `LOG.debug`; the reads that build it are made).

use super::constants::{
    DEV_VENDOR, HID_INTERFACE_CLASS, PRINTER_INTERFACE_CLASS, is_supported_product,
};
use super::pyusb::{self, Backend, Device, Endpoint, Error as UsbError};
use std::rc::Rc;

const GITHUB_ISSUE_MAC: &str = "<https://github.com/labelle-org/labelle/issues/5>";
const GITHUB_ISSUE_UDEV: &str = "<https://github.com/labelle-org/labelle/issues/6>";

thread_local! {
    /// The tests say which system they stand for (`platform.system()` of the audit's run).
    static PLATFORM: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// `platform.system()` (`usb_device.py:128`): "Linux", "Windows", "Darwin".
fn platform_system() -> String {
    if let Some(name) = PLATFORM.with(|p| p.get()) {
        return name.to_owned();
    }
    match std::env::consts::OS {
        "linux" => "Linux".to_owned(),
        "windows" => "Windows".to_owned(),
        "macos" => "Darwin".to_owned(),
        other => other.to_owned(),
    }
}

/// Ours, for the tests: the system `platform.system()` gives.
#[cfg(test)]
pub fn set_platform_for_test(name: Option<&'static str>) {
    PLATFORM.with(|p| p.set(name));
}

/// What `UsbDevice` raises: its own `UsbDeviceError(RuntimeError)`, or what pyusb raises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `UsbDeviceError`.
    UsbDevice(String),
    /// `usb.core.USBError`, `NoBackendError`, `ValueError`, ... from pyusb.
    Usb(UsbError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::UsbDevice(m) => f.write_str(m),
            Error::Usb(e) => write!(f, "{e}"),
        }
    }
}

impl From<UsbError> for Error {
    fn from(e: UsbError) -> Error {
        Error::Usb(e)
    }
}

/// `UsbDevice` (`usb_device.py:26`).
pub struct UsbDevice {
    pub dev: Rc<Device>,
    intf: Option<pyusb::Interface>,
    devin: Option<Endpoint>,
    devout: Option<Endpoint>,
}

impl UsbDevice {
    /// `__init__`.
    pub fn new(dev: Rc<Device>) -> UsbDevice {
        UsbDevice {
            dev,
            intf: None,
            devin: None,
            devout: None,
        }
    }

    /// `id_vendor` (`usb_device.py:61`).
    pub fn id_vendor(&self) -> u16 {
        self.dev.desc.id_vendor
    }

    /// `id_product` (`usb_device.py:68`).
    pub fn id_product(&self) -> u16 {
        self.dev.desc.id_product
    }

    /// `vendor_product_id` (`usb_device.py:75`).
    pub fn vendor_product_id(&self) -> String {
        format!("{:04x}:{:04x}", self.id_vendor(), self.id_product())
    }

    /// `usb_id` (`usb_device.py:79`), and so `hash`.
    pub fn usb_id(&self) -> String {
        format!(
            "Bus {:03} Device {:03}: ID {}",
            self.dev.desc.bus,
            self.dev.desc.address,
            self.vendor_product_id()
        )
    }

    /// `hash` (`usb_device.py:39`).
    pub fn hash(&self) -> String {
        self.usb_id()
    }

    /// `_is_supported_vendor` (`usb_device.py:85`).
    pub fn is_supported_vendor(dev: &Device) -> bool {
        dev.desc.id_vendor == DEV_VENDOR
    }

    /// `is_supported` (`usb_device.py:89`).
    pub fn is_supported(&self) -> bool {
        Self::is_supported_vendor(&self.dev) && is_supported_product(self.id_product())
    }

    /// `supported_devices` (`usb_device.py:96`): every device of labelle's vendor. (labelle
    /// returns a `set`; the order of its iteration is not defined, so callers sort.)
    pub fn supported_devices(backend: &Rc<dyn Backend>) -> Result<Vec<UsbDevice>, UsbError> {
        Ok(pyusb::find(backend, |d| UsbDevice::is_supported_vendor(d))?
            .into_iter()
            .map(UsbDevice::new)
            .collect())
    }

    /// `device_info` (`usb_device.py:105`): the reads it makes (the manufacturer, product and
    /// serial number strings, and the descriptors of all configurations); its text only goes to
    /// `LOG.debug`.
    pub fn device_info(&self) -> Result<(), Error> {
        match self.dev.manufacturer() {
            Ok(_) => {}
            Err(UsbError::Value(_)) => return Err(self.instruct_on_access_denied()),
            Err(e) => return Err(e.into()),
        }
        // `f"{self._dev!r}"`: no read.
        self.dev.manufacturer()?;
        self.dev.product()?;
        self.dev.serial_number()?;
        self.dev.read_all_descriptors()?;
        Ok(())
    }

    /// `_instruct_on_access_denied` (`usb_device.py:127`): the error it raises.
    fn instruct_on_access_denied(&self) -> Error {
        let system = platform_system();
        if system == "Linux" {
            self.instruct_on_access_denied_linux()
        } else if system == "Windows" {
            Error::UsbDevice(
                "Couldn't access the device. Please make sure that the device driver is set to WinUSB. This can be accomplished with Zadig <https://zadig.akeo.ie/>.".to_owned(),
            )
        } else if system == "Darwin" {
            // `{self._dev}` is pyusb's `str(device)`, the whole descriptor listing: not ported;
            // its first line stands for it (ours).
            Error::UsbDevice(format!(
                "Could not access DEVICE ID {} on Bus {:03} Address {:03}. Thanks for bravely trying this on a Mac. You are in uncharted territory. It would be appreciated if you share the results of your experimentation at {GITHUB_ISSUE_MAC}.",
                self.vendor_product_id(),
                self.dev.desc.bus,
                self.dev.desc.address
            ))
        } else {
            Error::UsbDevice(format!("Unknown platform {system}"))
        }
    }

    /// `_instruct_on_access_denied_linux` (`usb_device.py:146`), the message after `dedent`.
    fn instruct_on_access_denied_linux(&self) -> Error {
        let (vendor, product) = (self.id_vendor(), self.id_product());
        // `",'\\\n                     '".join([...])`, then `dedent` (12 spaces off).
        let udev_rule = [
            "ACTION==\"add\"".to_owned(),
            "SUBSYSTEMS==\"usb\"".to_owned(),
            format!("ATTRS{{idVendor}}==\"{vendor:04x}\""),
            format!("ATTRS{{idProduct}}==\"{product:04x}\""),
            "MODE=\"0666\"".to_owned(),
        ]
        .join(",'\\\n         '");
        Error::UsbDevice(format!(
            "\n\nYou do not have sufficient access to the device. You probably want to add the a udev\nrule in /etc/udev/rules.d with the following command:\n\n    echo '{udev_rule}' \\\n      | sudo tee /etc/udev/rules.d/91-labelle-{vendor:04x}-{product:04x}.rules\n\nNext, refresh udev with:\n\n    sudo udevadm control --reload-rules\n    sudo udevadm trigger --attr-match=idVendor=\"0922\"\n\nFinally, turn your device off and back on again to activate the new permissions.\n\nIf this still does not resolve the problem, you might need to reboot.\nIn case rebooting is necessary, please report this at {GITHUB_ISSUE_UDEV}.\nWe are still trying to figure out a simple procedure which works for everyone.\nIn case you still cannot connect, or if you have any information or ideas, please\npost them at that link.\n"
        ))
    }

    /// `_set_configuration` (`usb_device.py:201`).
    fn set_configuration(&self) -> Result<(), Error> {
        match self.dev.get_active_configuration() {
            Ok(_) => Ok(()), // Active device configuration already found.
            Err(UsbError::Usb(_)) => match self.dev.set_configuration() {
                Ok(()) => Ok(()), // Device configuration set.
                Err(UsbError::Usb(e)) => {
                    if e.errno == Some(13) {
                        Err(Error::UsbDevice("Access denied".to_owned()))
                    } else if e.errno == Some(16) {
                        Ok(()) // Device is busy, but this is okay.
                    } else {
                        Err(Error::Usb(UsbError::Usb(e)))
                    }
                }
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }

    /// `setup` (`usb_device.py:217`).
    pub fn setup(&mut self) -> Result<(), Error> {
        match self.setup_inner() {
            Err(Error::Usb(UsbError::Usb(e))) => Err(Error::UsbDevice(format!(
                "Failed setup USB device: {}",
                UsbError::Usb(e)
            ))),
            other => other,
        }
    }

    /// `_setup` (`usb_device.py:223`).
    fn setup_inner(&mut self) -> Result<(), Error> {
        self.set_configuration()?;
        let cfg = self.dev.get_active_configuration()?;
        let mut intf = self
            .dev
            .find_interface(&cfg, |i| i.b_interface_class == PRINTER_INTERFACE_CLASS)?;
        if intf.is_none() {
            let cfg = self.dev.get_active_configuration()?;
            intf = self
                .dev
                .find_interface(&cfg, |i| i.b_interface_class == HID_INTERFACE_CLASS)?;
            if intf.is_none() {
                return Err(Error::UsbDevice(
                    "Could not open a valid interface".to_owned(),
                ));
            }
        }
        let intf = intf.expect("an interface was found");
        let number = intf.desc.b_interface_number;
        match self.dev.is_kernel_driver_active(number) {
            Ok(true) => self.dev.detach_kernel_driver(number)?,
            Ok(false) => {}
            // "Kernel driver detaching not necessary on {platform}."
            Err(UsbError::NotImplemented(_)) => {}
            Err(e) => return Err(e.into()),
        }
        let devout = self
            .dev
            .find_endpoint(&intf, |e| pyusb::endpoint_is_out(e.b_endpoint_address))?;
        let devin = self
            .dev
            .find_endpoint(&intf, |e| pyusb::endpoint_is_in(e.b_endpoint_address))?;
        let (Some(devout), Some(devin)) = (devout, devin) else {
            self.intf = None;
            self.devin = None;
            self.devout = None;
            return Err(Error::UsbDevice(
                "The device endpoints not be found".to_owned(),
            ));
        };
        self.intf = Some(intf);
        self.devin = Some(Endpoint {
            device: self.dev.clone(),
            desc: devin,
        });
        self.devout = Some(Endpoint {
            device: self.dev.clone(),
            desc: devout,
        });
        Ok(())
    }

    /// `dispose` (`usb_device.py:278`).
    pub fn dispose(&self) {
        self.dev.dispose();
    }

    /// `devin` (`usb_device.py:295`).
    pub fn devin(&self) -> Option<&Endpoint> {
        self.devin.as_ref()
    }

    /// `devout` (`usb_device.py:299`).
    pub fn devout(&self) -> Option<&Endpoint> {
        self.devout.as_ref()
    }
}
