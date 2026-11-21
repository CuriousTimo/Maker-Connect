//! The serial ports a Marlin can be connected to, and which one a person chose (ours, the agent's
//! page only: nothing of it reaches the printer). OctoPrint keeps the chosen port's name
//! (`serial.port`), and names such as `/dev/ttyUSB4` or `COM5` can change at a restart. So the
//! agent keeps the USB device behind the name as well, and finds its name again by it:
//!
//! * `serialport` lists the ports by the name to open, each USB one with its VID, PID, serial
//!   number, interface and place (bus and port chain);
//! * `nusb` adds what it knows of the same device (by VID, PID and place): its release, and the
//!   strings `serialport` leaves out on some systems.
//!
//! A serial number is a device's own only when no other device of the same VID:PID has it (two
//! CP2102 both say `0001`); the place is the same while the cable stays in the same port.

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use nusb::MaybeFuture;

/// The USB device behind a serial port: what identifies it after a restart.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsbId {
    pub vid: u16,
    pub pid: u16,
    pub serial: String,
    pub interface: Option<u8>,
    /// bus and port chain, `8-1.3`; empty when the system does not tell it
    pub place: String,
}

impl UsbId {
    /// As the settings file keeps it: `1a86:7523 if=0 at=8-1.3 serial=...`.
    pub fn to_setting(&self) -> String {
        let mut s = format!("{:04x}:{:04x}", self.vid, self.pid);
        if let Some(i) = self.interface {
            s += &format!(" if={i}");
        }
        if !self.place.is_empty() {
            s += &format!(" at={}", self.place);
        }
        if !self.serial.is_empty() {
            // last: a serial number may hold spaces
            s += &format!(" serial={}", self.serial);
        }
        s
    }

    pub fn from_setting(text: &str) -> Option<UsbId> {
        let (head, serial) = match text.split_once(" serial=") {
            Some((h, s)) => (h, s.to_owned()),
            None => (text, String::new()),
        };
        let mut words = head.split_whitespace();
        let (vid, pid) = words.next()?.split_once(':')?;
        let mut id = UsbId {
            vid: u16::from_str_radix(vid, 16).ok()?,
            pid: u16::from_str_radix(pid, 16).ok()?,
            serial,
            ..UsbId::default()
        };
        for word in words {
            if let Some(i) = word.strip_prefix("if=") {
                id.interface = i.parse().ok();
            } else if let Some(p) = word.strip_prefix("at=") {
                id.place = p.to_owned();
            }
        }
        Some(id)
    }
}

/// One port to choose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// the name to open
    pub name: String,
    pub usb: Option<UsbId>,
    /// what the page shows
    pub label: String,
}

/// The serial ports there are now, USB ones with their device.
pub fn list() -> Vec<Listed> {
    let ports = serialport::available_ports().unwrap_or_default();
    let usb = usb_devices();
    let mut listed: Vec<Listed> = ports
        .into_iter()
        .map(|p| match p.port_type {
            serialport::SerialPortType::UsbPort(info) => {
                let place = info
                    .location
                    .as_ref()
                    .map(|l| place(l.bus_id(), l.port_chain()))
                    .unwrap_or_default();
                let id = UsbId {
                    vid: info.vid,
                    pid: info.pid,
                    serial: info.serial_number.clone().unwrap_or_default(),
                    interface: info.interface,
                    place,
                };
                let device = usb
                    .iter()
                    .find(|d| d.vid == id.vid && d.pid == id.pid && d.place == id.place);
                let product = info
                    .product
                    .clone()
                    .or_else(|| device.and_then(|d| d.product.clone()));
                let manufacturer = info
                    .manufacturer
                    .clone()
                    .or_else(|| device.and_then(|d| d.manufacturer.clone()));
                let label = label(
                    &p.port_name,
                    &id,
                    manufacturer.as_deref(),
                    product.as_deref(),
                    device.map(|d| d.release),
                );
                Listed {
                    name: p.port_name,
                    usb: Some(id),
                    label,
                }
            }
            _ => Listed {
                label: p.port_name.clone(),
                name: p.port_name,
                usb: None,
            },
        })
        .collect();
    listed.sort_by(|a, b| a.name.cmp(&b.name));
    listed
}

/// `8-1.3`: the bus without leading zeros (`serialport` says `8`, `nusb` `008`), then the chain.
fn place(bus: &str, chain: &[u8]) -> String {
    let bus = bus.trim_start_matches('0');
    let bus = if bus.is_empty() { "0" } else { bus };
    let chain: Vec<String> = chain.iter().map(u8::to_string).collect();
    format!("{bus}-{}", chain.join("."))
}

fn label(
    name: &str,
    id: &UsbId,
    manufacturer: Option<&str>,
    product: Option<&str>,
    release: Option<u16>,
) -> String {
    let mut parts = vec![name.to_owned()];
    let what = [manufacturer, product]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    if !what.is_empty() {
        parts.push(what);
    }
    let mut chip = format!("{:04x}:{:04x}", id.vid, id.pid);
    if let Some(r) = release {
        chip += &format!(" rev {:x}.{:02x}", r >> 8, r & 0xff);
    }
    parts.push(chip);
    if !id.place.is_empty() {
        parts.push(format!("USB {}", id.place));
    }
    if let Some(i) = id.interface {
        parts.push(format!("interface {i}"));
    }
    if !id.serial.is_empty() {
        parts.push(format!("serial {}", id.serial));
    }
    parts.join(" · ")
}

struct UsbDevice {
    vid: u16,
    pid: u16,
    place: String,
    release: u16,
    manufacturer: Option<String>,
    product: Option<String>,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn usb_devices() -> Vec<UsbDevice> {
    let Ok(devices) = nusb::list_devices().wait() else {
        return Vec::new();
    };
    devices
        .map(|d| UsbDevice {
            vid: d.vendor_id(),
            pid: d.product_id(),
            place: place(d.bus_id(), d.port_chain()),
            release: d.device_version(),
            manufacturer: d.manufacturer_string().map(str::to_owned),
            product: d.product_string().map(str::to_owned),
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn usb_devices() -> Vec<UsbDevice> {
    Vec::new()
}

/// The port of the chosen device now, or none when it is not there or cannot be told apart:
/// the same serial number at the same place; else its serial number, when no other device of
/// its kind has it; else its place; else the only device of its kind. Never a guess between
/// two.
pub fn find<'a>(listed: &'a [Listed], chosen: &UsbId) -> Option<&'a Listed> {
    let kind: Vec<(&Listed, &UsbId)> = listed
        .iter()
        .filter_map(|l| l.usb.as_ref().map(|u| (l, u)))
        .filter(|(_, u)| {
            u.vid == chosen.vid && u.pid == chosen.pid && u.interface == chosen.interface
        })
        .collect();
    let one = |m: Vec<&(&'a Listed, &UsbId)>| (m.len() == 1).then(|| m[0].0);
    if !chosen.serial.is_empty() {
        let exact = kind
            .iter()
            .filter(|(_, u)| u.serial == chosen.serial && u.place == chosen.place)
            .collect();
        if let Some(l) = one(exact) {
            return Some(l);
        }
        if let Some(l) = one(kind
            .iter()
            .filter(|(_, u)| u.serial == chosen.serial)
            .collect())
        {
            return Some(l);
        }
    }
    if !chosen.place.is_empty()
        && let Some(l) = one(kind
            .iter()
            .filter(|(_, u)| u.place == chosen.place)
            .collect())
    {
        return Some(l);
    }
    one(kind.iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str, vid: u16, pid: u16, serial: &str, interface: u8, place: &str) -> Listed {
        Listed {
            name: name.into(),
            label: name.into(),
            usb: Some(UsbId {
                vid,
                pid,
                serial: serial.into(),
                interface: Some(interface),
                place: place.into(),
            }),
        }
    }

    /// The ports on the Pi .67 (2026-11-21), as their names could be after a restart.
    fn pi() -> Vec<Listed> {
        vec![
            port("/dev/ttyUSB0", 0x0403, 0x6010, "", 0, "3-1.5"),
            port("/dev/ttyUSB1", 0x0403, 0x6010, "", 1, "3-1.5"),
            port("/dev/ttyUSB2", 0x10c4, 0xea60, "0001", 0, "2-1.1"),
            port("/dev/ttyUSB3", 0x10c4, 0xea60, "0001", 0, "3-1.6"),
            port("/dev/ttyUSB4", 0x1a86, 0x7523, "", 0, "8-1.3"),
        ]
    }

    fn id(listed: &[Listed], name: &str) -> UsbId {
        listed
            .iter()
            .find(|l| l.name == name)
            .unwrap()
            .usb
            .clone()
            .unwrap()
    }

    #[test]
    fn the_setting_reads_back() {
        let id = UsbId {
            vid: 0x1a86,
            pid: 0x7523,
            serial: "A B".into(),
            interface: Some(0),
            place: "8-1.3".into(),
        };
        assert_eq!(id.to_setting(), "1a86:7523 if=0 at=8-1.3 serial=A B");
        assert_eq!(UsbId::from_setting(&id.to_setting()), Some(id));
        assert_eq!(UsbId::from_setting("nope"), None);
    }

    /// The AtomX's CH340 has no serial number: found by its kind, under its new name.
    #[test]
    fn a_device_is_found_under_its_new_name() {
        let chosen = id(&pi(), "/dev/ttyUSB4");
        let mut after = pi();
        after[4].name = "/dev/ttyUSB2".into();
        after[2].name = "/dev/ttyUSB4".into();
        assert_eq!(find(&after, &chosen).unwrap().name, "/dev/ttyUSB2");
    }

    /// Two CP2102 with the same serial number `0001`: told apart by their place only.
    #[test]
    fn the_same_serial_number_is_told_apart_by_place() {
        let chosen = id(&pi(), "/dev/ttyUSB3");
        assert_eq!(find(&pi(), &chosen).unwrap().name, "/dev/ttyUSB3");
        // moved to another port: two of its kind, neither at its place: none, no guess
        let mut moved = pi();
        moved[3].usb.as_mut().unwrap().place = "3-1.7".into();
        assert_eq!(find(&moved, &chosen), None);
    }

    /// The FTDI's two ports share a place: the interface tells them apart.
    #[test]
    fn a_dual_port_is_told_apart_by_interface() {
        let chosen = id(&pi(), "/dev/ttyUSB1");
        assert_eq!(find(&pi(), &chosen).unwrap().name, "/dev/ttyUSB1");
    }

    /// The only CH340, in another port: still the one.
    #[test]
    fn the_only_one_of_its_kind_in_another_port() {
        let chosen = id(&pi(), "/dev/ttyUSB4");
        let mut moved = pi();
        moved[4].usb.as_mut().unwrap().place = "8-1.1.2".into();
        assert_eq!(find(&moved, &chosen).unwrap().name, "/dev/ttyUSB4");
        // a second CH340 comes: two of its kind, neither at its place: none
        moved.push(port("/dev/ttyUSB5", 0x1a86, 0x7523, "", 0, "8-1.2"));
        assert_eq!(find(&moved, &chosen), None);
    }

    /// Unplugged: none.
    #[test]
    fn an_unplugged_device_is_not_found() {
        let chosen = id(&pi(), "/dev/ttyUSB4");
        let mut gone = pi();
        gone.remove(4);
        assert_eq!(find(&gone, &chosen), None);
    }

    #[test]
    fn the_place_drops_leading_zeros() {
        assert_eq!(place("008", &[1, 3]), "8-1.3");
        assert_eq!(place("8", &[1, 3]), "8-1.3");
    }

    #[test]
    fn the_label_shows_the_device() {
        let id = UsbId {
            vid: 0x1a86,
            pid: 0x7523,
            serial: String::new(),
            interface: Some(0),
            place: "8-1.3".into(),
        };
        assert_eq!(
            label(
                "/dev/ttyUSB4",
                &id,
                None,
                Some("USB2.0-Serial"),
                Some(0x0254)
            ),
            "/dev/ttyUSB4 · USB2.0-Serial · 1a86:7523 rev 2.54 · USB 8-1.3 · interface 0"
        );
    }
}
