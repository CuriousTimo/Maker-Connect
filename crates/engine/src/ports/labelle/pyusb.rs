//! **pyusb 1.3.1** (`usb/core.py`, `usb/util.py`, `usb/control.py`), the part labelle calls: its
//! runtime, as the owner approved it (the version installed in `reference/.venv-audit`).
//! pyusb: Copyright 2009-2017 Wander Lairson Costa, Copyright 2009-2021 PyUSB contributors;
//! BSD-3-Clause licence ("Redistribution and use in source and binary forms, with or without
//! modification, are permitted provided that the following conditions are met: ...").
//!
//! The classes keep their names' meaning: [`Device`] is `usb.core.Device` (with its
//! `_ResourceManager`: `core.py:118`), [`find`] is `usb.core.find`, `get_string`/`get_langids`
//! are `util.py`'s, `ctrl_transfer`/`get_descriptor` are `core.py:1057` and `control.py:152`.
//! What pyusb asks of a backend is the [`Backend`] trait (`usb/backend/__init__.py`'s
//! `IBackend`); the libusb1 backend's stand-in is `nusb_backend.rs` (ours).
//!
//! Python's garbage collector finalizes a `Device` (`_finalize_object`, `core.py:1157`:
//! `self._ctx.dispose(self)`); here it is `Drop`.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

/// `_DEFAULT_TIMEOUT` (`core.py:61`), in milliseconds.
pub const DEFAULT_TIMEOUT: u32 = 1000;

/// `util.ENDPOINT_TYPE_*`: `bmAttributes & 3`.
const ENDPOINT_TYPE_BULK: u8 = 2;
const ENDPOINT_TYPE_INTR: u8 = 3;
/// `util.CTRL_IN` and `util.CTRL_RECIPIENT_*`.
const CTRL_IN: u8 = 0x80;
const CTRL_RECIPIENT_INTERFACE: u8 = 1;
const CTRL_TYPE_VENDOR: u8 = 2 << 5;
/// `util.DESC_TYPE_STRING`.
const DESC_TYPE_STRING: u16 = 0x03;

/// `usb.core.USBError` (`core.py:268`): `IOError(errno, strerror)` with a `backend_error_code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbError {
    pub strerror: String,
    pub backend_error_code: Option<i32>,
    pub errno: Option<i32>,
    /// A `USBTimeoutError`.
    pub timeout: bool,
}

impl UsbError {
    /// `USBError(strerror)`.
    pub fn new(strerror: &str) -> UsbError {
        UsbError {
            strerror: strerror.to_owned(),
            backend_error_code: None,
            errno: None,
            timeout: false,
        }
    }
}

/// What a pyusb call raises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `USBError` and `USBTimeoutError`.
    Usb(UsbError),
    /// `ValueError`.
    Value(String),
    /// `NotImplementedError`.
    NotImplemented(String),
    /// `IndexError`.
    Index(String),
    /// `NoBackendError` (a `ValueError`).
    NoBackend(String),
}

impl std::fmt::Display for Error {
    /// `str(exception)`: an `OSError(errno, strerror)` reads `[Errno 110] Operation timed out`,
    /// and `[Errno None] ...` without an errno.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Usb(e) => match e.errno {
                Some(n) => write!(f, "[Errno {n}] {}", e.strerror),
                None => write!(f, "[Errno None] {}", e.strerror),
            },
            Error::Value(m) | Error::NotImplemented(m) | Error::Index(m) | Error::NoBackend(m) => {
                f.write_str(m)
            }
        }
    }
}

impl Error {
    /// Whether it is a `USBError`.
    pub fn is_usb(&self) -> bool {
        matches!(self, Error::Usb(_))
    }
}

/// A device as the backend knows it (pyusb's opaque `dev`).
pub type DevId = usize;
/// An open device (pyusb's opaque `dev_handle`).
pub type Handle = usize;

/// `backend.get_device_descriptor`: the fields labelle (and the repr texts) read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceDescriptor {
    pub id_vendor: u16,
    pub id_product: u16,
    pub i_manufacturer: u8,
    pub i_product: u8,
    pub i_serial_number: u8,
    pub b_num_configurations: u8,
    pub bus: u32,
    pub address: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigurationDescriptor {
    pub b_configuration_value: u8,
    pub b_num_interfaces: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InterfaceDescriptor {
    pub b_interface_number: u8,
    pub b_alternate_setting: u8,
    pub b_interface_class: u8,
    pub b_num_endpoints: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EndpointDescriptor {
    pub b_endpoint_address: u8,
    pub bm_attributes: u8,
}

/// pyusb's `IBackend` (`usb/backend/__init__.py`): what `usb.core` asks of a backend. The
/// descriptor getters raise `IndexError` ([`Error::Index`]) past the last one.
pub trait Backend {
    fn enumerate_devices(&self) -> Result<Vec<DevId>, Error>;
    fn get_device_descriptor(&self, dev: DevId) -> Result<DeviceDescriptor, Error>;
    fn get_configuration_descriptor(
        &self,
        dev: DevId,
        config: usize,
    ) -> Result<ConfigurationDescriptor, Error>;
    fn get_interface_descriptor(
        &self,
        dev: DevId,
        intf: usize,
        alt: usize,
        config: usize,
    ) -> Result<InterfaceDescriptor, Error>;
    fn get_endpoint_descriptor(
        &self,
        dev: DevId,
        ep: usize,
        intf: usize,
        alt: usize,
        config: usize,
    ) -> Result<EndpointDescriptor, Error>;
    fn open_device(&self, dev: DevId) -> Result<Handle, Error>;
    fn close_device(&self, handle: Handle);
    fn set_configuration(&self, handle: Handle, config_value: u8) -> Result<(), Error>;
    fn get_configuration(&self, handle: Handle) -> Result<u8, Error>;
    fn claim_interface(&self, handle: Handle, intf: u8) -> Result<(), Error>;
    fn release_interface(&self, handle: Handle, intf: u8) -> Result<(), Error>;
    fn is_kernel_driver_active(&self, handle: Handle, intf: u8) -> Result<bool, Error>;
    fn detach_kernel_driver(&self, handle: Handle, intf: u8) -> Result<(), Error>;
    fn bulk_write(
        &self,
        handle: Handle,
        ep: u8,
        intf: u8,
        data: &[u8],
        timeout: u32,
    ) -> Result<usize, Error>;
    fn bulk_read(
        &self,
        handle: Handle,
        ep: u8,
        intf: u8,
        buff: &mut [u8],
        timeout: u32,
    ) -> Result<usize, Error>;
    fn intr_write(
        &self,
        handle: Handle,
        ep: u8,
        intf: u8,
        data: &[u8],
        timeout: u32,
    ) -> Result<usize, Error>;
    fn intr_read(
        &self,
        handle: Handle,
        ep: u8,
        intf: u8,
        buff: &mut [u8],
        timeout: u32,
    ) -> Result<usize, Error>;
    #[allow(clippy::too_many_arguments)]
    fn ctrl_transfer(
        &self,
        handle: Handle,
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
        buff: &mut [u8],
        timeout: u32,
    ) -> Result<usize, Error>;
}

/// `usb.core.Configuration` (the parts used).
#[derive(Debug, Clone)]
pub struct Configuration {
    pub index: usize,
    pub desc: ConfigurationDescriptor,
}

/// `usb.core.Interface`.
#[derive(Debug, Clone)]
pub struct Interface {
    pub index: usize,
    pub alternate_index: usize,
    pub configuration: usize,
    pub desc: InterfaceDescriptor,
}

/// `usb.core.Endpoint`: its writes and reads go through its device.
#[derive(Clone)]
pub struct Endpoint {
    pub device: Rc<Device>,
    pub desc: EndpointDescriptor,
}

impl Endpoint {
    /// `Endpoint.write(data, timeout=None)` (`core.py:405`).
    pub fn write(&self, data: &[u8]) -> Result<usize, Error> {
        self.device.write(self.desc.b_endpoint_address, data, None)
    }

    /// `Endpoint.read(size, timeout=None)` (`core.py:418`).
    pub fn read(&self, size: usize) -> Result<Vec<u8>, Error> {
        self.device.read(self.desc.b_endpoint_address, size, None)
    }
}

/// `usb.core.Device` with its `_ResourceManager` (`core.py:118`).
pub struct Device {
    backend: Rc<dyn Backend>,
    dev: DevId,
    handle: Cell<Option<Handle>>,
    active_cfg_index: Cell<Option<usize>>,
    claimed_intf: RefCell<BTreeSet<u8>>,
    intf_setting: RefCell<BTreeMap<u8, u8>>,
    ep_info: RefCell<BTreeMap<u8, (InterfaceDescriptor, EndpointDescriptor)>>,
    default_timeout: Cell<u32>,
    serial_number: RefCell<Option<String>>,
    product: RefCell<Option<String>>,
    manufacturer: RefCell<Option<String>>,
    langids: RefCell<Option<Vec<u16>>>,
    pub desc: DeviceDescriptor,
}

impl Device {
    /// `Device(dev, backend)` (`core.py:786`).
    pub fn new(dev: DevId, backend: Rc<dyn Backend>) -> Result<Device, Error> {
        let desc = backend.get_device_descriptor(dev)?;
        Ok(Device {
            backend,
            dev,
            handle: Cell::new(None),
            active_cfg_index: Cell::new(None),
            claimed_intf: RefCell::new(BTreeSet::new()),
            intf_setting: RefCell::new(BTreeMap::new()),
            ep_info: RefCell::new(BTreeMap::new()),
            default_timeout: Cell::new(DEFAULT_TIMEOUT),
            serial_number: RefCell::new(None),
            product: RefCell::new(None),
            manufacturer: RefCell::new(None),
            langids: RefCell::new(None),
            desc,
        })
    }

    // ---- _ResourceManager (core.py:118) ----

    /// `managed_open`.
    fn managed_open(&self) -> Result<Handle, Error> {
        if self.handle.get().is_none() {
            self.handle.set(Some(self.backend.open_device(self.dev)?));
        }
        Ok(self.handle.get().unwrap())
    }

    /// `managed_close`.
    fn managed_close(&self) {
        if let Some(handle) = self.handle.take() {
            self.backend.close_device(handle);
        }
    }

    /// `managed_set_configuration(device, None)`: the device's first configuration is set.
    fn managed_set_configuration(&self) -> Result<(), Error> {
        let cfg = self.configuration(0)?;
        let handle = self.managed_open()?;
        self.backend
            .set_configuration(handle, cfg.desc.b_configuration_value)?;
        // cache the index
        self.active_cfg_index.set(Some(cfg.index));
        self.intf_setting.borrow_mut().clear();
        self.ep_info.borrow_mut().clear();
        Ok(())
    }

    /// `managed_claim_interface`.
    fn managed_claim_interface(&self, intf: u8) -> Result<(), Error> {
        let handle = self.managed_open()?;
        if !self.claimed_intf.borrow().contains(&intf) {
            self.backend.claim_interface(handle, intf)?;
            self.claimed_intf.borrow_mut().insert(intf);
        }
        Ok(())
    }

    /// `managed_release_interface`.
    fn managed_release_interface(&self, intf: u8) -> Result<(), Error> {
        if self.claimed_intf.borrow().contains(&intf) {
            let result = match self.handle.get() {
                Some(handle) => self.backend.release_interface(handle, intf),
                None => Ok(()),
            };
            self.claimed_intf.borrow_mut().remove(&intf);
            return result;
        }
        Ok(())
    }

    /// `setup_request`: the interface and endpoint of an endpoint address, the interface
    /// claimed.
    fn setup_request(
        &self,
        endpoint_address: u8,
    ) -> Result<(InterfaceDescriptor, EndpointDescriptor), Error> {
        let (intf, ep) = self.get_interface_and_endpoint(endpoint_address)?;
        self.managed_claim_interface(intf.b_interface_number)?;
        Ok((intf, ep))
    }

    /// `get_interface_and_endpoint`.
    fn get_interface_and_endpoint(
        &self,
        endpoint_address: u8,
    ) -> Result<(InterfaceDescriptor, EndpointDescriptor), Error> {
        if let Some(found) = self.ep_info.borrow().get(&endpoint_address) {
            return Ok(found.clone());
        }
        let cfg = self.get_active_configuration()?;
        let mut found = None;
        self.for_each_interface(&cfg, |intf| {
            let setting = self
                .intf_setting
                .borrow()
                .get(&intf.desc.b_interface_number)
                .copied()
                .unwrap_or(0);
            if intf.desc.b_alternate_setting != setting {
                return Ok(false);
            }
            if let Some(ep) =
                self.find_endpoint(intf, |e| e.b_endpoint_address == endpoint_address)?
            {
                found = Some((intf.desc.clone(), ep));
                return Ok(true);
            }
            Ok(false)
        })?;
        match found {
            Some(found) => {
                self.ep_info
                    .borrow_mut()
                    .insert(endpoint_address, found.clone());
                Ok(found)
            }
            None => Err(Error::Value(format!(
                "Invalid endpoint address {:#x}",
                endpoint_address
            ))),
        }
    }

    /// `get_active_configuration` of the resource manager (`core.py:254`).
    pub fn get_active_configuration(&self) -> Result<Configuration, Error> {
        match self.active_cfg_index.get() {
            None => {
                let handle = self.managed_open()?;
                let value = self.backend.get_configuration(handle)?;
                let cfg = self.find_configuration(|c| c.b_configuration_value == value)?;
                match cfg {
                    None => Err(Error::Usb(UsbError::new("Configuration not set"))),
                    Some(cfg) => {
                        self.active_cfg_index.set(Some(cfg.index));
                        Ok(cfg)
                    }
                }
            }
            Some(index) => self.configuration(index),
        }
    }

    /// `release_all_interfaces`: the errors of the releases are ignored.
    fn release_all_interfaces(&self) {
        let claimed: Vec<u8> = self.claimed_intf.borrow().iter().copied().collect();
        for i in claimed {
            let _ = self.managed_release_interface(i);
        }
    }

    /// `dispose` (`core.py:279`).
    fn dispose_inner(&self, close_handle: bool) {
        self.release_all_interfaces();
        if close_handle {
            self.managed_close();
        }
        self.intf_setting.borrow_mut().clear();
        self.ep_info.borrow_mut().clear();
        self.active_cfg_index.set(None);
    }

    // ---- descriptors ----

    /// `Configuration(device, index)`.
    fn configuration(&self, index: usize) -> Result<Configuration, Error> {
        let desc = self.backend.get_configuration_descriptor(self.dev, index)?;
        Ok(Configuration { index, desc })
    }

    /// `util.find_descriptor(device, ...)` over `Device.__iter__` (`core.py:1148`).
    fn find_configuration(
        &self,
        matches: impl Fn(&ConfigurationDescriptor) -> bool,
    ) -> Result<Option<Configuration>, Error> {
        for i in 0..self.desc.b_num_configurations as usize {
            let cfg = self.configuration(i)?;
            if matches(&cfg.desc) {
                return Ok(Some(cfg));
            }
        }
        Ok(None)
    }

    /// `Configuration.__iter__` (`core.py:651`): every alternate setting of every interface,
    /// until `f` says it is done. A `USBError` or `IndexError` ends one interface's settings.
    fn for_each_interface(
        &self,
        cfg: &Configuration,
        mut f: impl FnMut(&Interface) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        for i in 0..cfg.desc.b_num_interfaces as usize {
            let mut alt = 0;
            loop {
                let desc = match self
                    .backend
                    .get_interface_descriptor(self.dev, i, alt, cfg.index)
                {
                    Ok(desc) => desc,
                    Err(Error::Usb(_) | Error::Index(_)) => break,
                    Err(e) => return Err(e),
                };
                let intf = Interface {
                    index: i,
                    alternate_index: alt,
                    configuration: cfg.index,
                    desc,
                };
                if f(&intf)? {
                    return Ok(());
                }
                alt += 1;
            }
        }
        Ok(())
    }

    /// `util.find_descriptor(cfg, ...)` of an interface (`custom_match` or fields).
    pub fn find_interface(
        &self,
        cfg: &Configuration,
        matches: impl Fn(&InterfaceDescriptor) -> bool,
    ) -> Result<Option<Interface>, Error> {
        let mut found = None;
        self.for_each_interface(cfg, |intf| {
            if matches(&intf.desc) {
                found = Some(intf.clone());
                return Ok(true);
            }
            Ok(false)
        })?;
        Ok(found)
    }

    /// `util.find_descriptor(intf, ...)` of an endpoint (`Interface.__iter__`, `core.py:530`).
    pub fn find_endpoint(
        &self,
        intf: &Interface,
        matches: impl Fn(&EndpointDescriptor) -> bool,
    ) -> Result<Option<EndpointDescriptor>, Error> {
        for i in 0..intf.desc.b_num_endpoints as usize {
            let desc = self.backend.get_endpoint_descriptor(
                self.dev,
                i,
                intf.index,
                intf.alternate_index,
                intf.configuration,
            )?;
            if matches(&desc) {
                return Ok(Some(desc));
            }
        }
        Ok(None)
    }

    // ---- Device (core.py:720) ----

    /// `Device.set_configuration(configuration=None)`.
    pub fn set_configuration(&self) -> Result<(), Error> {
        self.managed_set_configuration()
    }

    /// `Device.is_kernel_driver_active(interface)`.
    pub fn is_kernel_driver_active(&self, interface: u8) -> Result<bool, Error> {
        let handle = self.managed_open()?;
        self.backend.is_kernel_driver_active(handle, interface)
    }

    /// `Device.detach_kernel_driver(interface)`.
    pub fn detach_kernel_driver(&self, interface: u8) -> Result<(), Error> {
        let handle = self.managed_open()?;
        self.backend.detach_kernel_driver(handle, interface)
    }

    fn timeout(&self, timeout: Option<u32>) -> u32 {
        timeout.unwrap_or(self.default_timeout.get())
    }

    /// `Device.write(endpoint, data, timeout=None)` (`core.py:978`).
    pub fn write(&self, endpoint: u8, data: &[u8], timeout: Option<u32>) -> Result<usize, Error> {
        let (intf, ep) = self.setup_request(endpoint)?;
        let handle = self.handle.get().expect("setup_request opened the device");
        let timeout = self.timeout(timeout);
        match ep.bm_attributes & 3 {
            ENDPOINT_TYPE_BULK => self.backend.bulk_write(
                handle,
                ep.b_endpoint_address,
                intf.b_interface_number,
                data,
                timeout,
            ),
            ENDPOINT_TYPE_INTR => self.backend.intr_write(
                handle,
                ep.b_endpoint_address,
                intf.b_interface_number,
                data,
                timeout,
            ),
            _ => Err(Error::NotImplemented("iso_write".to_owned())),
        }
    }

    /// `Device.read(endpoint, size, timeout=None)` (`core.py:1011`): the bytes read.
    pub fn read(&self, endpoint: u8, size: usize, timeout: Option<u32>) -> Result<Vec<u8>, Error> {
        let (intf, ep) = self.setup_request(endpoint)?;
        let handle = self.handle.get().expect("setup_request opened the device");
        let timeout = self.timeout(timeout);
        // `util.create_buffer(size)`
        let mut buff = vec![0u8; size];
        let ret = match ep.bm_attributes & 3 {
            ENDPOINT_TYPE_BULK => self.backend.bulk_read(
                handle,
                ep.b_endpoint_address,
                intf.b_interface_number,
                &mut buff,
                timeout,
            )?,
            ENDPOINT_TYPE_INTR => self.backend.intr_read(
                handle,
                ep.b_endpoint_address,
                intf.b_interface_number,
                &mut buff,
                timeout,
            )?,
            _ => return Err(Error::NotImplemented("iso_read".to_owned())),
        };
        buff.truncate(ret);
        Ok(buff)
    }

    /// `Device.ctrl_transfer` of a device-to-host request of `length` bytes (`core.py:1057`):
    /// the bytes read.
    pub fn ctrl_transfer_in(
        &self,
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
        length: usize,
    ) -> Result<Vec<u8>, Error> {
        let mut buff = vec![0u8; length];
        let handle = self.managed_open()?;
        // Thanks to Johannes Stezenbach to point me out that we need to claim the recipient
        // interface
        let recipient = bm_request_type & 3;
        let rqtype = bm_request_type & (3 << 5);
        if recipient == CTRL_RECIPIENT_INTERFACE && rqtype != CTRL_TYPE_VENDOR {
            self.managed_claim_interface((w_index & 0xff) as u8)?;
        }
        let ret = self.backend.ctrl_transfer(
            handle,
            bm_request_type,
            b_request,
            w_value,
            w_index,
            &mut buff,
            self.timeout(None),
        )?;
        buff.truncate(ret);
        Ok(buff)
    }

    /// `Device.langids` (`core.py:852`).
    pub fn langids(&self) -> Result<Vec<u16>, Error> {
        if self.langids.borrow().is_none() {
            let ids = match get_langids(self) {
                Ok(ids) => ids,
                Err(Error::Usb(_)) => Vec::new(),
                Err(e) => return Err(e),
            };
            *self.langids.borrow_mut() = Some(ids);
        }
        Ok(self.langids.borrow().clone().unwrap())
    }

    /// `Device.serial_number`, `.product`, `.manufacturer` (`core.py:871`ff).
    pub fn serial_number(&self) -> Result<Option<String>, Error> {
        self.cached_string(&self.serial_number, self.desc.i_serial_number)
    }

    pub fn product(&self) -> Result<Option<String>, Error> {
        self.cached_string(&self.product, self.desc.i_product)
    }

    pub fn manufacturer(&self) -> Result<Option<String>, Error> {
        self.cached_string(&self.manufacturer, self.desc.i_manufacturer)
    }

    fn cached_string(
        &self,
        cache: &RefCell<Option<String>>,
        index: u8,
    ) -> Result<Option<String>, Error> {
        if cache.borrow().is_none() {
            let value = get_string(self, index)?;
            *cache.borrow_mut() = value;
        }
        Ok(cache.borrow().clone())
    }

    /// `Device.configurations()`: the descriptors of all configurations, and for each its
    /// interfaces (`Configuration.interfaces()`): what `device_info` reads, for its text.
    pub fn read_all_descriptors(&self) -> Result<(), Error> {
        for i in 0..self.desc.b_num_configurations as usize {
            let cfg = self.configuration(i)?;
            self.for_each_interface(&cfg, |_| Ok(false))?;
        }
        Ok(())
    }

    /// `usb.util.dispose_resources(device)` (`util.py:217`).
    pub fn dispose(&self) {
        self.dispose_inner(true);
    }
}

impl Drop for Device {
    /// `_finalize_object` (`core.py:1157`): `self._ctx.dispose(self)`.
    fn drop(&mut self) {
        self.dispose_inner(true);
    }
}

/// `control.get_descriptor(dev, desc_size, desc_type, desc_index, wIndex)` (`control.py:152`).
fn get_descriptor(
    dev: &Device,
    desc_size: usize,
    desc_type: u16,
    desc_index: u16,
    w_index: u16,
) -> Result<Vec<u8>, Error> {
    let w_value = desc_index | (desc_type << 8);
    // `util.build_request_type(CTRL_IN, CTRL_TYPE_STANDARD, CTRL_RECIPIENT_DEVICE)`
    let bm_request_type = CTRL_IN;
    dev.ctrl_transfer_in(bm_request_type, 0x06, w_value, w_index, desc_size)
}

/// `util.get_langids` (`util.py:233`).
fn get_langids(dev: &Device) -> Result<Vec<u16>, Error> {
    let buf = get_descriptor(dev, 254, DESC_TYPE_STRING, 0, 0)?;
    // If the length of buf came back odd, something is wrong.
    if buf.len() < 4 || buf[0] < 4 || buf[0] & 1 != 0 {
        return Ok(Vec::new());
    }
    // `buf[2:buf[0]:2]` (the low bytes) and `buf[3:buf[0]:2]` (the high bytes)
    let end = (buf[0] as usize).min(buf.len());
    let lows = buf[2..end].iter().step_by(2);
    let highs = buf[3..end].iter().step_by(2);
    Ok(lows
        .zip(highs)
        .map(|(x, y)| *x as u16 + ((*y as u16) << 8))
        .collect())
}

/// `util.get_string(dev, index)` (`util.py:283`).
pub fn get_string(dev: &Device, index: u8) -> Result<Option<String>, Error> {
    if index == 0 {
        return Ok(None);
    }
    let langids = dev.langids()?;
    if langids.is_empty() {
        return Err(Error::Value(
            "The device has no langid (permission issue, no string descriptors supported or device error)".to_owned(),
        ));
    }
    let langid = langids[0];
    let buf = get_descriptor(dev, 254, DESC_TYPE_STRING, index as u16, langid)?;
    if buf.is_empty() {
        return Err(Error::Index("index out of range".to_owned()));
    }
    // should be even, ignore any trailing byte (see #154)
    let blen = (buf[0] & 0xfe) as usize;
    let bytes = buf.get(2..blen.min(buf.len())).unwrap_or(&[]);
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units)
        .map(Some)
        .map_err(|e| Error::Value(format!("'utf-16-le' codec can't decode: {e}")))
}

/// `usb.core.find(find_all=True, backend=..., custom_match=...)` (`core.py:1235`).
pub fn find(
    backend: &Rc<dyn Backend>,
    custom_match: impl Fn(&Device) -> bool,
) -> Result<Vec<Rc<Device>>, Error> {
    let mut found = Vec::new();
    for dev in backend.enumerate_devices()? {
        let d = Device::new(dev, backend.clone())?;
        if custom_match(&d) {
            found.push(Rc::new(d));
        }
    }
    Ok(found)
}

/// `usb.util.endpoint_direction(address) == ENDPOINT_OUT` (`util.py:106`).
pub fn endpoint_is_out(address: u8) -> bool {
    address & 0x80 == 0
}

/// `usb.util.endpoint_direction(address) == ENDPOINT_IN`.
pub fn endpoint_is_in(address: u8) -> bool {
    address & 0x80 != 0
}
