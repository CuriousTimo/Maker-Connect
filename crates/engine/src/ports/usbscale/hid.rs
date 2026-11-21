//! The HID source of the scale port (`docs/archive/plans/agent-engine.md`, Step 3e, E1, E2): a trait, so the tests
//! have a fake, and the real one on hidapi 2.6.7 (the OS's own HID: Linux hidraw, Windows' HID API, macOS
//! IOKit). **All of it is ours**: usbscale reads USB interrupt transfers with libusb.
//!
//! [`HidDevice`] has a read and nothing else: no write, no feature report, no GET_REPORT, so the port
//! cannot send anything to the scale (what the OS itself sends on an open is not ours).

/// The usage page of a scale (HID Usage Tables 1.6 ch. 33: "Weighing Devices", 0x8D).
pub const SCALE_USAGE_PAGE: u16 = 0x8D;

/// A HID device as the OS lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidInfo {
    /// `/dev/hidrawN`, the IOService path, `\\?\HID#...`.
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product: String,
    /// The top-level usage page of its report descriptor.
    pub usage_page: u16,
}

impl HidInfo {
    /// `path  0922:8003  product  usage page 0x8d`.
    pub fn describe(&self) -> String {
        format!(
            "{}  {:04x}:{:04x}  {}  usage page 0x{:02x}",
            self.path,
            self.vendor_id,
            self.product_id,
            if self.product.is_empty() {
                "(no product name)"
            } else {
                &self.product
            },
            self.usage_page
        )
    }
}

/// An open HID device: it can be read, nothing else.
pub trait HidDevice {
    /// One report into `buf`, waiting at most `timeout_ms`: its length, 0 when none came in time.
    fn read(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, String>;
}

/// The OS's HID devices.
pub trait HidSource {
    /// The HID devices now (enumerated again each time).
    fn list(&mut self) -> Result<Vec<HidInfo>, String>;
    fn open(&mut self, path: &str) -> Result<Box<dyn HidDevice>, String>;
}

/// E2: the device to read. `--port` as given; without it the one device with usage page 0x8D, never
/// guessed by VID:PID. None or several: refused with what was seen.
pub fn select(source: &mut dyn HidSource, port: Option<&str>) -> Result<String, String> {
    if let Some(path) = port {
        return Ok(path.to_owned());
    }
    let all = source.list()?;
    let scales: Vec<&HidInfo> = all
        .iter()
        .filter(|d| d.usage_page == SCALE_USAGE_PAGE)
        .collect();
    match scales.as_slice() {
        [one] => Ok(one.path.clone()),
        [] => {
            let seen: String = all
                .iter()
                .map(|d| format!("\n  {}", d.describe()))
                .collect();
            Err(format!(
                "no HID device with usage page 0x8d (a scale) found; name it with --port. HID devices seen: {}",
                if seen.is_empty() {
                    " none".to_owned()
                } else {
                    seen
                }
            ))
        }
        many => {
            let seen: String = many
                .iter()
                .map(|d| format!("\n  {}", d.describe()))
                .collect();
            Err(format!(
                "{} HID devices with usage page 0x8d (a scale); choose one with --port:{seen}",
                many.len()
            ))
        }
    }
}

#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
mod real {
    use super::{HidDevice, HidInfo, HidSource};
    use std::ffi::CString;

    /// hidapi 2.6.7. One `HidApi` at a time lives in a process: drop this before making another.
    pub struct Hidapi {
        api: hidapi::HidApi,
    }

    impl Hidapi {
        pub fn new() -> Result<Hidapi, String> {
            hidapi::HidApi::new()
                .map(|api| Hidapi { api })
                .map_err(|e| e.to_string())
        }
    }

    struct Device(hidapi::HidDevice);

    impl HidDevice for Device {
        fn read(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, String> {
            self.0
                .read_timeout(buf, timeout_ms)
                .map_err(|e| e.to_string())
        }
    }

    impl HidSource for Hidapi {
        fn list(&mut self) -> Result<Vec<HidInfo>, String> {
            self.api.refresh_devices().map_err(|e| e.to_string())?;
            Ok(self
                .api
                .device_list()
                .map(|d| HidInfo {
                    path: d.path().to_string_lossy().into_owned(),
                    vendor_id: d.vendor_id(),
                    product_id: d.product_id(),
                    product: d.product_string().unwrap_or_default().to_owned(),
                    usage_page: d.usage_page(),
                })
                .collect())
        }

        fn open(&mut self, path: &str) -> Result<Box<dyn HidDevice>, String> {
            let path = CString::new(path).map_err(|e| e.to_string())?;
            self.api
                .open_path(&path)
                .map(|d| Box::new(Device(d)) as Box<dyn HidDevice>)
                .map_err(|e| e.to_string())
        }
    }
}

#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
pub use real::Hidapi;

/// The real source: hidapi on Linux, Windows and macOS.
pub fn system_source() -> Result<Box<dyn HidSource>, String> {
    #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
    {
        Ok(Box::new(Hidapi::new()?))
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        Err("this platform has no HID support here (Linux, Windows and macOS have)".to_owned())
    }
}
