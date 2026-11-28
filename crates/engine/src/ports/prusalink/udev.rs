//! The part of pyudev 0.24.5 that PrusaLink's `get_usb_printers` (`util.py:252-286`) calls:
//! `pyudev.Context()`, `context.list_devices(subsystem='tty')` and `device.properties.get(...)`
//! (#246). pyudev is PrusaLink's requirement (`requirements.txt:17`, `pyudev~=0.24.0`); its source is
//! read from the sdist (`pyudev-0.24.5.tar.gz`, `src/pyudev/`).
//!
//! As pyudev, libudev is opened when it runs, not linked (`_ctypeslib/utils.py` `load_ctypes_library`:
//! `find_library(name)`, then `CDLL(library_name, use_errno=True)`; owner 2026-11-28 "ok a"), and
//! called in pyudev's order: `udev_new` (`core.py:59`); `udev_enumerate_new` (`core.py:182`),
//! `udev_enumerate_add_match_subsystem` (`core.py:251`, its error checker
//! `check_negative_errorcode`), `udev_enumerate_scan_devices` and `udev_enumerate_get_list_entry`
//! (`core.py:384-385`); per entry `udev_list_entry_get_name`, `udev_list_entry_get_value`,
//! `udev_list_entry_get_next` (`_util.py` `udev_list_iterate`) and `udev_device_new_from_syspath`
//! (`device/_device.py` `from_sys_path`), a device that is gone (NULL) skipped (`core.py:388-390`,
//! `except DeviceNotFoundAtPathError: continue`); `udev_device_get_property_value` for each
//! `properties.get` (`device/_device.py` `Properties.__getitem__`, NULL a `KeyError`, so the default).
//!
//! **Differs:** the library is `libudev.so.1` by name; pyudev asks `ctypes.util.find_library("udev")`
//! (on Linux `ldconfig -p`), which gives that name where libudev is installed.
//!
//! **Not covered:**
//! - `udev_new` returning NULL: pyudev has no error checker for it (`ERROR_CHECKERS["udev_new"]`
//!   is `None`) and crashes on the next call; here it is an `OSError`.
//! - A property name or value that is not UTF-8: pyudev's `ensure_unicode_string` raises
//!   `UnicodeDecodeError`; here a `ValueError` with another text.
//! - When the objects are freed (`__del__`, `udev_*_unref`): a device when the loop moves on, the
//!   enumerator and the context at the end.

use std::ffi::{CStr, CString, c_char, c_int, c_void};

use maker_connect_gcode_metadata::py::PyErr;

type Udev = *mut c_void;
type Enumerate = *mut c_void;
type Entry = *mut c_void;
type Device = *mut c_void;

/// `libudev.py` `SIGNATURES`, the ones called.
pub struct Lib {
    _lib: libloading::Library,
    udev_new: unsafe extern "C" fn() -> Udev,
    udev_unref: unsafe extern "C" fn(Udev) -> Udev,
    udev_enumerate_new: unsafe extern "C" fn(Udev) -> Enumerate,
    udev_enumerate_unref: unsafe extern "C" fn(Enumerate) -> Enumerate,
    udev_enumerate_add_match_subsystem: unsafe extern "C" fn(Enumerate, *const c_char) -> c_int,
    udev_enumerate_scan_devices: unsafe extern "C" fn(Enumerate) -> c_int,
    udev_enumerate_get_list_entry: unsafe extern "C" fn(Enumerate) -> Entry,
    udev_list_entry_get_name: unsafe extern "C" fn(Entry) -> *const c_char,
    udev_list_entry_get_value: unsafe extern "C" fn(Entry) -> *const c_char,
    udev_list_entry_get_next: unsafe extern "C" fn(Entry) -> Entry,
    udev_device_new_from_syspath: unsafe extern "C" fn(Udev, *const c_char) -> Device,
    udev_device_unref: unsafe extern "C" fn(Device) -> Device,
    udev_device_get_property_value: unsafe extern "C" fn(Device, *const c_char) -> *const c_char,
}

impl Lib {
    /// `load_ctypes_library("udev", ...)`: `raise ImportError(f"No library named {name}")` when
    /// there is none.
    fn load() -> Result<Lib, PyErr> {
        let missing = || PyErr::Other("ImportError", "No library named udev".to_owned());
        // SAFETY: libudev's initialisers have no preconditions; each symbol has the C type of its
        // pyudev signature.
        unsafe {
            let lib = libloading::Library::new("libudev.so.1").map_err(|_| missing())?;
            macro_rules! sym {
                ($name:ident) => {
                    *lib.get(concat!(stringify!($name), "\0").as_bytes())
                        .map_err(|_| missing())?
                };
            }
            Ok(Lib {
                udev_new: sym!(udev_new),
                udev_unref: sym!(udev_unref),
                udev_enumerate_new: sym!(udev_enumerate_new),
                udev_enumerate_unref: sym!(udev_enumerate_unref),
                udev_enumerate_add_match_subsystem: sym!(udev_enumerate_add_match_subsystem),
                udev_enumerate_scan_devices: sym!(udev_enumerate_scan_devices),
                udev_enumerate_get_list_entry: sym!(udev_enumerate_get_list_entry),
                udev_list_entry_get_name: sym!(udev_list_entry_get_name),
                udev_list_entry_get_value: sym!(udev_list_entry_get_value),
                udev_list_entry_get_next: sym!(udev_list_entry_get_next),
                udev_device_new_from_syspath: sym!(udev_device_new_from_syspath),
                udev_device_unref: sym!(udev_device_unref),
                udev_device_get_property_value: sym!(udev_device_get_property_value),
                _lib: lib,
            })
        }
    }
}

/// `_errorcheckers.py` `exception_from_errno` (`:35-47`), with `ERRNO_EXCEPTIONS` (`:28-32`):
/// `ENOMEM` `MemoryError`, `EOVERFLOW` `OverflowError`, `EINVAL` `ValueError`, else
/// `OSError(errnum, errorstr)`.
fn exception_from_errno(errnum: c_int) -> PyErr {
    let errorstr = maker_connect_sdk::pyos::OsError::from_errno(errnum, None).strerror;
    match errnum {
        libc::ENOMEM => PyErr::Other("MemoryError", errorstr),
        libc::EOVERFLOW => PyErr::OverflowError(errorstr),
        libc::EINVAL => PyErr::ValueError(errorstr),
        _ => PyErr::Other("OSError", format!("[Errno {errnum}] {errorstr}")),
    }
}

/// `ensure_unicode_string` (`_util.py`): bytes decoded with the file system's encoding (UTF-8).
///
/// # Safety
/// `text` is NULL or a NUL-terminated string.
unsafe fn unicode(text: *const c_char) -> Result<Option<String>, PyErr> {
    if text.is_null() {
        return Ok(None);
    }
    // SAFETY: the caller's.
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    String::from_utf8(bytes.to_vec())
        .map(Some)
        .map_err(|e| PyErr::ValueError(format!("'utf-8' codec can't decode bytes: {e}")))
}

/// A device's `properties` (`device/_device.py` `Properties`): each read asks libudev again.
pub struct Properties<'a> {
    lib: &'a Lib,
    device: Device,
}

impl Properties<'_> {
    /// `properties.get(prop, default)` (`Mapping.get` over `__getitem__`): libudev's value, or
    /// `default` where it has none.
    pub fn get(&self, prop: &str, default: Option<&str>) -> Result<Option<String>, PyErr> {
        let prop = CString::new(prop).expect("a property name without NUL");
        // SAFETY: `device` lives while `self` does.
        let value =
            unsafe { (self.lib.udev_device_get_property_value)(self.device, prop.as_ptr()) };
        // SAFETY: libudev's value, NULL or NUL-terminated.
        Ok(unsafe { unicode(value) }?.or(default.map(str::to_owned)))
    }
}

/// `for device in pyudev.Context().list_devices(subsystem='tty'): each(device.properties)`, in
/// pyudev's order of calls.
pub fn for_each_tty(
    each: &mut dyn FnMut(&dyn crate::ports::prusalink::util::TtyProperties) -> Result<(), PyErr>,
) -> Result<(), PyErr> {
    let lib = Lib::load()?;
    // SAFETY: each pointer is libudev's, used while its owner lives and released once.
    unsafe {
        let context = (lib.udev_new)();
        if context.is_null() {
            return Err(PyErr::Other("OSError", "udev_new failed".to_owned()));
        }
        let enumerate = (lib.udev_enumerate_new)(context);
        let result = (|| {
            let matched = (lib.udev_enumerate_add_match_subsystem)(enumerate, c"tty".as_ptr());
            if matched < 0 {
                return Err(exception_from_errno(-matched));
            }
            (lib.udev_enumerate_scan_devices)(enumerate);
            let mut entry = (lib.udev_enumerate_get_list_entry)(enumerate);
            // `udev_list_iterate`: name, value, yield; the next entry when the loop comes back
            while !entry.is_null() {
                let name = (lib.udev_list_entry_get_name)(entry);
                (lib.udev_list_entry_get_value)(entry);
                let device = (lib.udev_device_new_from_syspath)(context, name);
                if !device.is_null() {
                    let done = each(&Properties { lib: &lib, device });
                    (lib.udev_device_unref)(device);
                    done?;
                }
                entry = (lib.udev_list_entry_get_next)(entry);
            }
            Ok(())
        })();
        if !enumerate.is_null() {
            (lib.udev_enumerate_unref)(enumerate);
        }
        (lib.udev_unref)(context);
        result
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::ports::prusalink::python;
    use serde_json::json;

    /// pyudev 0.24.5 and ours on this machine's tty devices: the same devices in the same order,
    /// each property `get_usb_printers` reads the same (or missing in both).
    #[test]
    fn identical_to_pyudev() {
        let keys = [
            "ID_VENDOR_ID",
            "ID_USB_VENDOR_ID",
            "ID_MODEL_ID",
            "ID_USB_MODEL_ID",
            "DEVNAME",
            "ID_SERIAL_SHORT",
            "ID_USB_SERIAL_SHORT",
        ];
        let Some(theirs) = python::run(include_str!("driver_udev.py"), &json!({ "keys": keys }))
        else {
            return;
        };
        let mut ours = vec![];
        for_each_tty(&mut |device| {
            let mut one = vec![];
            for key in keys {
                one.push(json!(device.get(key, None)?));
            }
            ours.push(one);
            Ok(())
        })
        .unwrap();
        assert!(!ours.is_empty(), "no tty device to compare");
        assert_eq!(json!({ "devices": ours }), theirs);
    }
}
