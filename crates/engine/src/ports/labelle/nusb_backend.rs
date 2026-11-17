//! **Ours:** `nusb` (pure Rust, the crate `crates/label` used) standing for pyusb's libusb1
//! backend (`usb/backend/libusb1.py`): the operations of [`super::pyusb::Backend`]
//! that labelle's code reaches, on nusb. Errors are given the text and `errno` pyusb gives for
//! the libusb error they stand for (`libusb1.py:_strerror`, `_libusb_errno`).
//!
//! Differences from libusb that nusb brings (the owner to approve; not tried on the printer
//! yet):
//! - the descriptors of a device are read from an opened `nusb::Device`
//!   (libusb reads its cached copy without opening it): only for devices of labelle's vendor;
//! - `is_kernel_driver_active` reads sysfs (`<device>:<config>.<interface>/driver`) on Linux,
//!   where libusb asks the kernel (`USBDEVFS_GETDRIVER`); elsewhere it is not implemented, as in
//!   libusb's macOS and Windows backends;
//! - the device's `iManufacturer`, `iProduct` and `iSerialNumber` come from its device
//!   descriptor as libusb's do.

use super::constants::DEV_VENDOR;
use super::pyusb::{
    Backend, ConfigurationDescriptor, DevId, DeviceDescriptor, EndpointDescriptor, Error, Handle,
    InterfaceDescriptor, UsbError,
};
use nusb::MaybeFuture;
use nusb::transfer::{
    Buffer, Bulk, ControlIn, ControlType, In, Interrupt, Out, Recipient, TransferError,
};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Duration;

/// Linux's errno numbers, as pyusb's `_libusb_errno` gives them.
const EIO: i32 = 5;
const ENODEV: i32 = 19;
const EACCES: i32 = 13;
const ENOENT: i32 = 2;
const EBUSY: i32 = 16;
const ETIMEDOUT: i32 = 110;
const EPIPE: i32 = 32;
const ENOSYS: i32 = 38;

/// `_strerror(libusb error)` and its errno, as a pyusb error.
fn libusb_error(strerror: &str, code: i32, errno: i32, timeout: bool) -> Error {
    Error::Usb(UsbError {
        strerror: strerror.to_owned(),
        backend_error_code: Some(code),
        errno: Some(errno),
        timeout,
    })
}

fn io_error() -> Error {
    libusb_error("Input/Output Error", -1, EIO, false)
}

/// A nusb error as the libusb error pyusb would show.
fn from_nusb(e: nusb::Error) -> Error {
    match e.kind() {
        nusb::ErrorKind::Disconnected => libusb_error(
            "No such device (it may have been disconnected)",
            -4,
            ENODEV,
            false,
        ),
        nusb::ErrorKind::Busy => libusb_error("Resource busy", -6, EBUSY, false),
        nusb::ErrorKind::PermissionDenied => {
            libusb_error("Insufficient permissions", -3, EACCES, false)
        }
        nusb::ErrorKind::NotFound => libusb_error("Entity not found", -5, ENOENT, false),
        nusb::ErrorKind::Unsupported => libusb_error(
            "Operation not supported or unimplemented on this platform",
            -12,
            ENOSYS,
            false,
        ),
        _ => io_error(),
    }
}

/// A nusb transfer error as the libusb error pyusb would show (`Cancelled` is a timeout: the
/// transfer was cancelled because its time was up).
fn from_transfer(e: TransferError) -> Error {
    match e {
        TransferError::Cancelled => libusb_error("Operation timed out", -7, ETIMEDOUT, true),
        TransferError::Stall => libusb_error("Pipe error", -9, EPIPE, false),
        TransferError::Disconnected => libusb_error(
            "No such device (it may have been disconnected)",
            -4,
            ENODEV,
            false,
        ),
        _ => io_error(),
    }
}

/// A device-to-host control transfer as libusb sends it. Elsewhere on the device; on Windows
/// (WinUSB) libusb sends it on an interface (`reference/libusb-1.0.29/windows_winusb.c`,
/// `winusbx_submit_control_transfer` :2998-3004, `get_valid_interface` :2909, `check_valid_interface`
/// :2933, `auto_claim` :613, `auto_release` :658): with an interface recipient the claimed interface
/// `wIndex & 0xff`, otherwise the lowest claimed interface; with none, the first interface that can be
/// claimed is claimed for the transfer and released after it. The request on the wire is the same.
#[cfg(not(windows))]
fn control_in(
    device: &nusb::Device,
    _claimed: &BTreeMap<u8, nusb::Interface>,
    control: ControlIn,
    timeout: Duration,
) -> Result<Vec<u8>, TransferError> {
    device.control_in(control, timeout).wait()
}

#[cfg(windows)]
fn control_in(
    device: &nusb::Device,
    claimed: &BTreeMap<u8, nusb::Interface>,
    control: ControlIn,
    timeout: Duration,
) -> Result<Vec<u8>, TransferError> {
    let valid = if control.recipient == Recipient::Interface {
        claimed.get(&((control.index & 0xff) as u8))
    } else {
        claimed.values().next()
    };
    if let Some(interface) = valid {
        return interface.control_in(control, timeout).wait();
    }
    // auto_claim: "No serviceable interface was found"; released when the transfer ends (dropped).
    let mut numbers: Vec<u8> = device
        .active_configuration()
        .map(|c| c.interfaces().map(|i| i.interface_number()).collect())
        .unwrap_or_default();
    numbers.sort_unstable();
    numbers.dedup();
    for number in numbers {
        if let Ok(interface) = device.claim_interface(number).wait() {
            return interface.control_in(control, timeout).wait();
        }
    }
    // "could not auto-claim any interface" (LIBUSB_ERROR_NOT_FOUND)
    Err(TransferError::InvalidArgument)
}

struct Open {
    dev: DevId,
    device: nusb::Device,
    interfaces: BTreeMap<u8, nusb::Interface>,
}

/// The nusb stand-in for the libusb1 backend.
pub struct NusbBackend {
    devices: Vec<nusb::DeviceInfo>,
    open: RefCell<Vec<Option<Open>>>,
}

impl NusbBackend {
    /// `libusb1.get_backend()`: the devices of the system now (nothing is sent).
    pub fn new() -> Result<NusbBackend, Error> {
        let devices = nusb::list_devices()
            .wait()
            .map_err(|e| Error::NoBackend(format!("No backend available: {e}")))?
            .collect();
        Ok(NusbBackend {
            devices,
            open: RefCell::new(Vec::new()),
        })
    }

    /// An opened `nusb::Device` for a descriptor read: the one open for it, else one opened for
    /// the read.
    fn with_device<T>(&self, dev: DevId, f: impl FnOnce(&nusb::Device) -> T) -> Result<T, Error> {
        for open in self.open.borrow().iter().flatten() {
            if open.dev == dev {
                return Ok(f(&open.device));
            }
        }
        let device = self.devices[dev].open().wait().map_err(from_nusb)?;
        Ok(f(&device))
    }

    fn with_open<T>(
        &self,
        handle: Handle,
        f: impl FnOnce(&mut Open) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut open = self.open.borrow_mut();
        match open.get_mut(handle).and_then(Option::as_mut) {
            Some(open) => f(open),
            None => Err(io_error()),
        }
    }

    /// `transfer_blocking` on the endpoint of an interface (bulk or interrupt, as the
    /// descriptor says: `Device.write`/`read` pick the function from `bmAttributes`).
    fn transfer_out(
        &self,
        handle: Handle,
        ep: u8,
        interrupt: bool,
        data: &[u8],
        timeout: u32,
    ) -> Result<usize, Error> {
        self.with_open(handle, |open| {
            let interface = open.interfaces.values().next().ok_or_else(io_error)?;
            let (buf, timeout) = (
                Buffer::from(data.to_vec()),
                Duration::from_millis(timeout.into()),
            );
            let done = if interrupt {
                let mut ep = interface
                    .endpoint::<Interrupt, Out>(ep)
                    .map_err(from_nusb)?;
                ep.transfer_blocking(buf, timeout)
            } else {
                let mut ep = interface.endpoint::<Bulk, Out>(ep).map_err(from_nusb)?;
                ep.transfer_blocking(buf, timeout)
            };
            done.status.map_err(from_transfer)?;
            Ok(data.len())
        })
    }

    fn transfer_in(
        &self,
        handle: Handle,
        ep: u8,
        interrupt: bool,
        buff: &mut [u8],
        timeout: u32,
    ) -> Result<usize, Error> {
        self.with_open(handle, |open| {
            let interface = open.interfaces.values().next().ok_or_else(io_error)?;
            let (buf, timeout) = (
                Buffer::new(buff.len()),
                Duration::from_millis(timeout.into()),
            );
            let done = if interrupt {
                let mut ep = interface.endpoint::<Interrupt, In>(ep).map_err(from_nusb)?;
                ep.transfer_blocking(buf, timeout)
            } else {
                let mut ep = interface.endpoint::<Bulk, In>(ep).map_err(from_nusb)?;
                ep.transfer_blocking(buf, timeout)
            };
            done.status.map_err(from_transfer)?;
            let bytes = done.buffer.to_vec();
            let n = bytes.len().min(buff.len());
            buff[..n].copy_from_slice(&bytes[..n]);
            Ok(n)
        })
    }
}

impl Backend for NusbBackend {
    fn enumerate_devices(&self) -> Result<Vec<DevId>, Error> {
        Ok((0..self.devices.len()).collect())
    }

    fn get_device_descriptor(&self, dev: DevId) -> Result<DeviceDescriptor, Error> {
        let info = &self.devices[dev];
        #[cfg(target_os = "linux")]
        let bus = info.busnum() as u32;
        #[cfg(not(target_os = "linux"))]
        let bus = info.bus_id().parse().unwrap_or(0);
        let mut desc = DeviceDescriptor {
            id_vendor: info.vendor_id(),
            id_product: info.product_id(),
            bus,
            address: info.device_address() as u32,
            ..DeviceDescriptor::default()
        };
        if desc.id_vendor == DEV_VENDOR
            && let Ok(d) = self.with_device(dev, |device| device.device_descriptor())
        {
            desc.i_manufacturer = d.manufacturer_string_index().map_or(0, |i| i.get());
            desc.i_product = d.product_string_index().map_or(0, |i| i.get());
            desc.i_serial_number = d.serial_number_string_index().map_or(0, |i| i.get());
            desc.b_num_configurations = d.num_configurations();
        }
        Ok(desc)
    }

    fn get_configuration_descriptor(
        &self,
        dev: DevId,
        config: usize,
    ) -> Result<ConfigurationDescriptor, Error> {
        self.with_device(dev, |device| {
            device
                .configurations()
                .nth(config)
                .map(|c| ConfigurationDescriptor {
                    b_configuration_value: c.configuration_value(),
                    b_num_interfaces: c.num_interfaces(),
                })
        })?
        .ok_or_else(|| Error::Index(format!("Invalid configuration index {config}")))
    }

    fn get_interface_descriptor(
        &self,
        dev: DevId,
        intf: usize,
        alt: usize,
        config: usize,
    ) -> Result<InterfaceDescriptor, Error> {
        self.with_device(dev, |device| {
            let cfg = device.configurations().nth(config)?;
            let group = cfg.interfaces().nth(intf)?;
            let a = group.alt_settings().nth(alt)?;
            Some(InterfaceDescriptor {
                b_interface_number: a.interface_number(),
                b_alternate_setting: a.alternate_setting(),
                b_interface_class: a.class(),
                b_num_endpoints: a.num_endpoints(),
            })
        })?
        .ok_or_else(|| Error::Index(format!("Invalid interface index {intf}")))
    }

    fn get_endpoint_descriptor(
        &self,
        dev: DevId,
        ep: usize,
        intf: usize,
        alt: usize,
        config: usize,
    ) -> Result<EndpointDescriptor, Error> {
        self.with_device(dev, |device| {
            let cfg = device.configurations().nth(config)?;
            let group = cfg.interfaces().nth(intf)?;
            let a = group.alt_settings().nth(alt)?;
            let e = a.endpoints().nth(ep)?;
            Some(EndpointDescriptor {
                b_endpoint_address: e.address(),
                bm_attributes: e.attributes(),
            })
        })?
        .ok_or_else(|| Error::Index(format!("Invalid endpoint index {ep}")))
    }

    fn open_device(&self, dev: DevId) -> Result<Handle, Error> {
        let device = self.devices[dev].open().wait().map_err(from_nusb)?;
        let mut open = self.open.borrow_mut();
        open.push(Some(Open {
            dev,
            device,
            interfaces: BTreeMap::new(),
        }));
        Ok(open.len() - 1)
    }

    fn close_device(&self, handle: Handle) {
        if let Some(slot) = self.open.borrow_mut().get_mut(handle) {
            *slot = None;
        }
    }

    fn set_configuration(&self, handle: Handle, config_value: u8) -> Result<(), Error> {
        self.with_open(handle, |open| {
            open.device
                .set_configuration(config_value)
                .wait()
                .map_err(from_nusb)
        })
    }

    fn get_configuration(&self, handle: Handle) -> Result<u8, Error> {
        // libusb gives 0 for an unconfigured device.
        self.with_open(handle, |open| {
            Ok(open
                .device
                .active_configuration()
                .map_or(0, |c| c.configuration_value()))
        })
    }

    fn claim_interface(&self, handle: Handle, intf: u8) -> Result<(), Error> {
        self.with_open(handle, |open| {
            let interface = open
                .device
                .claim_interface(intf)
                .wait()
                .map_err(from_nusb)?;
            open.interfaces.insert(intf, interface);
            Ok(())
        })
    }

    fn release_interface(&self, handle: Handle, intf: u8) -> Result<(), Error> {
        self.with_open(handle, |open| {
            open.interfaces.remove(&intf);
            Ok(())
        })
    }

    fn is_kernel_driver_active(&self, handle: Handle, intf: u8) -> Result<bool, Error> {
        #[cfg(target_os = "linux")]
        {
            let dev = self.with_open(handle, |open| Ok(open.dev))?;
            let info = &self.devices[dev];
            let config = self.with_open(handle, |open| {
                Ok(open
                    .device
                    .active_configuration()
                    .map_or(0, |c| c.configuration_value()))
            })?;
            let name = info
                .sysfs_path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let driver = info
                .sysfs_path()
                .join(format!("{name}:{config}.{intf}"))
                .join("driver");
            Ok(driver.exists())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (handle, intf);
            Err(Error::NotImplemented("is_kernel_driver_active".to_owned()))
        }
    }

    fn detach_kernel_driver(&self, handle: Handle, intf: u8) -> Result<(), Error> {
        self.with_open(handle, |open| {
            open.device.detach_kernel_driver(intf).map_err(from_nusb)
        })
    }

    fn bulk_write(
        &self,
        handle: Handle,
        ep: u8,
        _intf: u8,
        data: &[u8],
        timeout: u32,
    ) -> Result<usize, Error> {
        self.transfer_out(handle, ep, false, data, timeout)
    }

    fn bulk_read(
        &self,
        handle: Handle,
        ep: u8,
        _intf: u8,
        buff: &mut [u8],
        timeout: u32,
    ) -> Result<usize, Error> {
        self.transfer_in(handle, ep, false, buff, timeout)
    }

    fn intr_write(
        &self,
        handle: Handle,
        ep: u8,
        _intf: u8,
        data: &[u8],
        timeout: u32,
    ) -> Result<usize, Error> {
        self.transfer_out(handle, ep, true, data, timeout)
    }

    fn intr_read(
        &self,
        handle: Handle,
        ep: u8,
        _intf: u8,
        buff: &mut [u8],
        timeout: u32,
    ) -> Result<usize, Error> {
        self.transfer_in(handle, ep, true, buff, timeout)
    }

    fn ctrl_transfer(
        &self,
        handle: Handle,
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
        buff: &mut [u8],
        timeout: u32,
    ) -> Result<usize, Error> {
        // Device-to-host only (`get_descriptor`): the string descriptors of `device_info`.
        if bm_request_type & 0x80 == 0 {
            return Err(Error::NotImplemented(
                "a host-to-device control transfer".to_owned(),
            ));
        }
        let control_type = match (bm_request_type >> 5) & 3 {
            0 => ControlType::Standard,
            1 => ControlType::Class,
            _ => ControlType::Vendor,
        };
        let recipient = match bm_request_type & 0x1f {
            0 => Recipient::Device,
            1 => Recipient::Interface,
            2 => Recipient::Endpoint,
            _ => Recipient::Other,
        };
        self.with_open(handle, |open| {
            let data = control_in(
                &open.device,
                &open.interfaces,
                ControlIn {
                    control_type,
                    recipient,
                    request: b_request,
                    value: w_value,
                    index: w_index,
                    length: buff.len() as u16,
                },
                Duration::from_millis(timeout.into()),
            )
            .map_err(from_transfer)?;
            let n = data.len().min(buff.len());
            buff[..n].copy_from_slice(&data[..n]);
            Ok(n)
        })
    }
}
