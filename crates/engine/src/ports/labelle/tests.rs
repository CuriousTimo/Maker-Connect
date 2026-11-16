//! The port's proof (`docs/audits/dymo-labelmanager.md`): `scripts/labelle-port-check.py` runs
//! real labelle (real pyusb, real Pillow) on fake USB devices and records every operation;
//! these tests run the port on a recorder over the same devices and compare, and compare the
//! constants with what the script read from labelle's source.

use super::constants::{self, Direction};
use super::cli::{self, Printer};
use super::dymo_labeler::{self, COMMANDS};
use super::pillow::{Image, Mode};
use super::pyusb::{
    Backend, ConfigurationDescriptor, DevId, DeviceDescriptor, EndpointDescriptor, Error, Handle,
    InterfaceDescriptor, UsbError,
};
use super::usb_device;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

/// What the fake device answers a read.
const ANSWER: [u8; 8] = [0x40, 0x00, 0x12, 0x4E, 0x0D, 0x6B, 0x00, 0x00];

fn fixtures() -> PathBuf {
    match std::env::var("LABELLE_FIXTURES") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/labelle"),
    }
}

struct Iface {
    number: u8,
    alt: u8,
    class: u8,
    bound: bool,
    endpoints: Vec<(u8, bool)>,
}

struct FakeDev {
    bus: u32,
    addr: u32,
    vid: u16,
    pid: u16,
    strings: [u8; 3],
    active: Cell<u8>,
    flags: Vec<String>,
    ifaces: Vec<Iface>,
    drivers: RefCell<Vec<(u8, bool)>>,
}

impl FakeDev {
    fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }

    /// The interfaces grouped by number, in order of appearance.
    fn groups(&self) -> Vec<Vec<&Iface>> {
        let mut groups: Vec<Vec<&Iface>> = Vec::new();
        for i in &self.ifaces {
            match groups.iter_mut().find(|g| g[0].number == i.number) {
                Some(g) => g.push(i),
                None => groups.push(vec![i]),
            }
        }
        groups
    }
}

/// pyusb's backend for the devices of a scene, as `scripts/labelle-port-check.py`'s
/// `FakeBackend`: every operation besides reading descriptors recorded with the bus and address
/// of its device.
struct Fake {
    devices: Vec<FakeDev>,
    log: Rc<RefCell<Vec<String>>>,
    handles: RefCell<Vec<DevId>>,
    writes: Cell<usize>,
    reads: Cell<usize>,
}

fn timeout_error() -> Error {
    Error::Usb(UsbError { strerror: "Operation timed out".into(), backend_error_code: Some(-7), errno: Some(110), timeout: true })
}

fn usb_error(strerror: &str, code: i32, errno: i32) -> Error {
    Error::Usb(UsbError { strerror: strerror.into(), backend_error_code: Some(code), errno: Some(errno), timeout: false })
}

impl Fake {
    fn rec(&self, handle: Handle, text: String) {
        let d = &self.devices[self.handles.borrow()[handle]];
        self.log.borrow_mut().push(format!("{}.{} {text}", d.bus, d.addr));
    }

    fn dev_of(&self, handle: Handle) -> &FakeDev {
        &self.devices[self.handles.borrow()[handle]]
    }

    fn write(&self, kind: &str, handle: Handle, ep: u8, data: &[u8], timeout: u32) -> Result<usize, Error> {
        let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
        self.rec(handle, format!("{kind}_write {ep:02x} {hex} {timeout}"));
        self.writes.set(self.writes.get() + 1);
        for flag in &self.dev_of(handle).flags {
            if let Some(n) = flag.strip_prefix("failwrite=")
                && self.writes.get() == n.parse::<usize>().unwrap()
            {
                return Err(timeout_error());
            }
        }
        Ok(data.len())
    }

    fn read(&self, kind: &str, handle: Handle, ep: u8, buff: &mut [u8], timeout: u32) -> Result<usize, Error> {
        self.rec(handle, format!("{kind}_read {ep:02x} {} {timeout}", buff.len()));
        self.reads.set(self.reads.get() + 1);
        for flag in &self.dev_of(handle).flags {
            if let Some(n) = flag.strip_prefix("failread=")
                && self.reads.get() == n.parse::<usize>().unwrap()
            {
                return Err(timeout_error());
            }
        }
        buff[..ANSWER.len()].copy_from_slice(&ANSWER);
        Ok(ANSWER.len())
    }
}

impl Backend for Fake {
    fn enumerate_devices(&self) -> Result<Vec<DevId>, Error> {
        Ok((0..self.devices.len()).collect())
    }

    fn get_device_descriptor(&self, dev: DevId) -> Result<DeviceDescriptor, Error> {
        let d = &self.devices[dev];
        Ok(DeviceDescriptor {
            id_vendor: d.vid,
            id_product: d.pid,
            i_manufacturer: d.strings[0],
            i_product: d.strings[1],
            i_serial_number: d.strings[2],
            b_num_configurations: 1,
            bus: d.bus,
            address: d.addr,
        })
    }

    fn get_configuration_descriptor(&self, dev: DevId, config: usize) -> Result<ConfigurationDescriptor, Error> {
        if config != 0 {
            return Err(Error::Index("config".into()));
        }
        Ok(ConfigurationDescriptor { b_configuration_value: 1, b_num_interfaces: self.devices[dev].groups().len() as u8 })
    }

    fn get_interface_descriptor(&self, dev: DevId, intf: usize, alt: usize, _config: usize) -> Result<InterfaceDescriptor, Error> {
        let d = &self.devices[dev];
        let groups = d.groups();
        let i = groups.get(intf).and_then(|g| g.get(alt)).ok_or_else(|| Error::Index("interface".into()))?;
        Ok(InterfaceDescriptor {
            b_interface_number: i.number,
            b_alternate_setting: i.alt,
            b_interface_class: i.class,
            b_num_endpoints: i.endpoints.len() as u8,
        })
    }

    fn get_endpoint_descriptor(&self, dev: DevId, ep: usize, intf: usize, alt: usize, _config: usize) -> Result<EndpointDescriptor, Error> {
        let d = &self.devices[dev];
        let groups = d.groups();
        let (address, interrupt) = groups[intf][alt].endpoints[ep];
        Ok(EndpointDescriptor { b_endpoint_address: address, bm_attributes: if interrupt { 3 } else { 2 } })
    }

    fn open_device(&self, dev: DevId) -> Result<Handle, Error> {
        self.handles.borrow_mut().push(dev);
        let handle = self.handles.borrow().len() - 1;
        self.rec(handle, "open_device".into());
        Ok(handle)
    }

    fn close_device(&self, handle: Handle) {
        self.rec(handle, "close_device".into());
    }

    fn set_configuration(&self, handle: Handle, config_value: u8) -> Result<(), Error> {
        self.rec(handle, format!("set_configuration {config_value}"));
        let d = self.dev_of(handle);
        if d.has("busy_setconfig") {
            return Err(usb_error("Resource busy", -6, 16));
        }
        if d.has("denied_setconfig") {
            return Err(usb_error("Insufficient permissions", -3, 13));
        }
        d.active.set(config_value);
        Ok(())
    }

    fn get_configuration(&self, handle: Handle) -> Result<u8, Error> {
        let v = self.dev_of(handle).active.get();
        self.rec(handle, format!("get_configuration -> {v}"));
        Ok(v)
    }

    fn claim_interface(&self, handle: Handle, intf: u8) -> Result<(), Error> {
        self.rec(handle, format!("claim_interface {intf}"));
        Ok(())
    }

    fn release_interface(&self, handle: Handle, intf: u8) -> Result<(), Error> {
        self.rec(handle, format!("release_interface {intf}"));
        Ok(())
    }

    fn is_kernel_driver_active(&self, handle: Handle, intf: u8) -> Result<bool, Error> {
        let d = self.dev_of(handle);
        if d.has("nokernelapi") {
            self.rec(handle, format!("is_kernel_driver_active {intf} -> NotImplementedError"));
            return Err(Error::NotImplemented("is_kernel_driver_active".into()));
        }
        let bound = d.drivers.borrow().iter().find(|(n, _)| *n == intf).is_some_and(|(_, b)| *b);
        self.rec(handle, format!("is_kernel_driver_active {intf} -> {}", if bound { "True" } else { "False" }));
        Ok(bound)
    }

    fn detach_kernel_driver(&self, handle: Handle, intf: u8) -> Result<(), Error> {
        self.rec(handle, format!("detach_kernel_driver {intf}"));
        for (n, b) in self.dev_of(handle).drivers.borrow_mut().iter_mut() {
            if *n == intf {
                *b = false;
            }
        }
        Ok(())
    }

    fn bulk_write(&self, handle: Handle, ep: u8, _intf: u8, data: &[u8], timeout: u32) -> Result<usize, Error> {
        self.write("bulk", handle, ep, data, timeout)
    }

    fn bulk_read(&self, handle: Handle, ep: u8, _intf: u8, buff: &mut [u8], timeout: u32) -> Result<usize, Error> {
        self.read("bulk", handle, ep, buff, timeout)
    }

    fn intr_write(&self, handle: Handle, ep: u8, _intf: u8, data: &[u8], timeout: u32) -> Result<usize, Error> {
        self.write("intr", handle, ep, data, timeout)
    }

    fn intr_read(&self, handle: Handle, ep: u8, _intf: u8, buff: &mut [u8], timeout: u32) -> Result<usize, Error> {
        self.read("intr", handle, ep, buff, timeout)
    }

    fn ctrl_transfer(&self, handle: Handle, bm: u8, req: u8, value: u16, index: u16, buff: &mut [u8], timeout: u32) -> Result<usize, Error> {
        self.rec(handle, format!("ctrl_transfer {bm:02x} {req:02x} {value:04x} {index:04x} {} {timeout}", buff.len()));
        if bm & 0x80 != 0 && req == 6 && value >> 8 == 3 {
            let i = (value & 0xFF) as u8;
            let data: Vec<u8> = if i == 0 {
                if self.dev_of(handle).has("nolangid") {
                    return Err(usb_error("Pipe error", -9, 32));
                }
                vec![4, 3, 0x09, 0x04]
            } else {
                let text: Vec<u8> = format!("DYMO string {i}").encode_utf16().flat_map(u16::to_le_bytes).collect();
                let mut data = vec![2 + text.len() as u8, 3];
                data.extend(text);
                data
            };
            buff[..data.len()].copy_from_slice(&data);
            return Ok(data.len());
        }
        Err(usb_error("Pipe error", -9, 32))
    }
}

/// The scenes of `scenes.txt`: the devices of each.
fn scenes() -> Vec<(String, Vec<(u32, u32, u16, u16, [u8; 3], u8, Vec<String>, Vec<Iface>)>)> {
    let text = std::fs::read_to_string(fixtures().join("scenes.txt")).expect("run scripts/labelle-port-check.py");
    let mut scenes: Vec<(String, Vec<_>)> = Vec::new();
    for line in text.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w[0] {
            "scene" => scenes.push((w[1].to_owned(), Vec::new())),
            "dev" => {
                let flags = if w[9] == "-" { Vec::new() } else { w[9].split(',').map(str::to_owned).collect() };
                scenes.last_mut().unwrap().1.push((
                    w[1].parse().unwrap(),
                    w[2].parse().unwrap(),
                    u16::from_str_radix(w[3], 16).unwrap(),
                    u16::from_str_radix(w[4], 16).unwrap(),
                    [w[5].parse().unwrap(), w[6].parse().unwrap(), w[7].parse().unwrap()],
                    w[8].parse().unwrap(),
                    flags,
                    Vec::new(),
                ));
            }
            "iface" => {
                let endpoints = w[5]
                    .split(',')
                    .map(|e| (u8::from_str_radix(&e[..2], 16).unwrap(), e.ends_with('i')))
                    .collect();
                scenes.last_mut().unwrap().1.last_mut().unwrap().7.push(Iface {
                    number: w[1].parse().unwrap(),
                    alt: w[2].parse().unwrap(),
                    class: u8::from_str_radix(w[3], 16).unwrap(),
                    bound: w[4] == "+",
                    endpoints,
                });
            }
            other => panic!("scenes.txt: {other}"),
        }
    }
    scenes
}

/// A backend over the devices of a scene, and its log.
fn fake(scene: &str) -> (Rc<dyn Backend>, Rc<RefCell<Vec<String>>>) {
    fake_flagged(scene, None)
}

/// [`fake`] with one more flag on each device (`failread=N`, `failwrite=N`).
fn fake_flagged(scene: &str, extra: Option<&str>) -> (Rc<dyn Backend>, Rc<RefCell<Vec<String>>>) {
    let (_, devices) = scenes().into_iter().find(|(name, _)| name == scene).unwrap_or_else(|| panic!("scene {scene}"));
    let log = Rc::new(RefCell::new(Vec::new()));
    let devices = devices
        .into_iter()
        .map(|(bus, addr, vid, pid, strings, active, flags, ifaces)| FakeDev {
            bus,
            addr,
            vid,
            pid,
            strings,
            active: Cell::new(active),
            flags: flags.into_iter().chain(extra.map(str::to_owned)).collect(),
            drivers: RefCell::new(ifaces.iter().map(|i| (i.number, i.bound)).collect()),
            ifaces,
        })
        .collect();
    let backend = Fake { devices, log: log.clone(), handles: RefCell::new(Vec::new()), writes: Cell::new(0), reads: Cell::new(0) };
    (Rc::new(backend), log)
}

/// One example through the port, as `scripts/labelle-port-check.py` runs it through labelle:
/// every operation, then `done` or `error <message>`.
fn run_case(scene: &str, picture: &str, tape: Option<i64>) -> Vec<String> {
    usb_device::set_platform_for_test(Some("Linux"));
    let (backend, log) = fake(scene);
    let mut printer = None;
    let result = cli::print_picture(backend, picture, tape, &mut |_| {}, &mut printer);
    let last = match result {
        Ok(()) => "done".to_owned(),
        Err(e) => format!("error {e}"),
    };
    let mut ops: Vec<String> = log.borrow().clone();
    ops.push(last);
    let fix = fixtures().to_string_lossy().into_owned();
    ops.into_iter().map(|o| o.replace(&fix, "FIX")).collect()
}

#[test]
fn proof_every_example_goes_as_labelle_sends_it() {
    let cases = std::fs::read_to_string(fixtures().join("cases.txt")).expect("run scripts/labelle-port-check.py");
    let theirs = std::fs::read_to_string(fixtures().join("labelle.txt")).expect("run scripts/labelle-port-check.py");
    let mut out = String::new();
    for line in cases.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(w[0], "case");
        let tape = if w[3] == "none" { None } else { Some(w[3].parse().unwrap()) };
        let picture = fixtures().join("pngs").join(w[4]);
        out.push_str(&format!("case {}\n", w[1]));
        for op in run_case(w[2], &picture.to_string_lossy(), tape) {
            out.push_str(&op);
            out.push('\n');
        }
    }
    std::fs::write(fixtures().join("ours.txt"), &out).unwrap();
    // Constants and command bytes: what ours says, in the lines of the script's extraction.
    std::fs::write(fixtures().join("ours-constants.txt"), constants_rows().join("\n") + "\n").unwrap();
    let mut case = String::new();
    for (i, (a, b)) in theirs.lines().zip(out.lines()).enumerate() {
        if a.starts_with("case ") {
            case = a.to_owned();
        }
        assert_eq!(a, b, "line {} of labelle.txt, in {case}", i + 1);
    }
    assert_eq!(theirs.lines().count(), out.lines().count(), "as many operations");
}

/// The constants as the script reads them from labelle's source.
fn constants_rows() -> Vec<String> {
    let mut rows = vec![];
    for (id, name) in constants::supported_products() {
        rows.push(format!("SUPPORTED_PRODUCTS {id:#06x} {name}"));
    }
    rows.push(format!("UNCONFIRMED_MESSAGE {}", constants::UNCONFIRMED_MESSAGE));
    rows.push(format!("DEV_VENDOR {}", constants::DEV_VENDOR));
    rows.push(format!("PRINTER_INTERFACE_CLASS {}", constants::PRINTER_INTERFACE_CLASS));
    rows.push(format!("HID_INTERFACE_CLASS {}", constants::HID_INTERFACE_CLASS));
    rows.push(format!("ESC {}", constants::ESC));
    rows.push(format!("SYN {}", constants::SYN));
    rows.push(format!("DEFAULT_MARGIN_PX {}", constants::DEFAULT_MARGIN_PX));
    rows.push(format!("DPI {}", constants::DPI));
    rows.push(format!("MM_PER_INCH {}", constants::MM_PER_INCH));
    rows.push(format!(
        "SUPPORTED_TAPE_SIZES_MM ({})",
        dymo_labeler::DymoLabeler::SUPPORTED_TAPE_SIZES_MM.iter().map(i64::to_string).collect::<Vec<_>>().join(", ")
    ));
    rows.push(format!("DEFAULT_TAPE_SIZE_MM {}", dymo_labeler::DymoLabeler::DEFAULT_TAPE_SIZE_MM));
    rows.push(format!(
        "LABELER_DISTANCE_BETWEEN_PRINT_HEAD_AND_CUTTER_MM {}",
        dymo_labeler::DymoLabeler::LABELER_DISTANCE_BETWEEN_PRINT_HEAD_AND_CUTTER_MM
    ));
    rows.push(format!("LABELER_PRINT_HEAD_HEIGHT_MM {}", dymo_labeler::DymoLabeler::LABELER_PRINT_HEAD_HEIGHT_MM));
    for (method, letter) in COMMANDS {
        rows.push(format!("COMMAND {method} ESC {}", letter as char));
    }
    rows.push(format!("_maxLines {}", dymo_labeler::MAX_LINES));
    rows.push(format!("synwait {}", dymo_labeler::SYNWAIT));
    rows.sort();
    rows.dedup();
    rows
}

/// The rows of the script's extraction (labelle's source) that start with `name`.
fn their_rows(name: &str) -> Vec<String> {
    let text = std::fs::read_to_string(fixtures().join("constants.txt")).expect("run scripts/labelle-port-check.py");
    let rows: Vec<String> = text.lines().filter(|r| r.starts_with(name)).map(str::to_owned).collect();
    assert!(!rows.is_empty(), "labelle's source has no `{name}`");
    rows
}

/// A test for each row of constants, command bytes and sizes extracted from labelle's source:
/// ours says the same.
macro_rules! rows {
    ($($test:ident: $name:expr,)*) => {
        $(
            #[test]
            fn $test() {
                let ours: Vec<String> = constants_rows().into_iter().filter(|r| r.starts_with($name)).collect();
                assert_eq!(ours, their_rows($name));
            }
        )*
        /// Every row of labelle's source is one of those above, or not ported; ours has no more.
        #[test]
        fn rows_of_labelles_source_are_all_tested() {
            let names: &[&str] = &[$($name),*];
            // `_dot_tab` ('B'), `_cut` ('E'): not ported, nothing that prints calls them.
            let not_ported = ["COMMAND _dot_tab ", "COMMAND _cut "];
            let text = std::fs::read_to_string(fixtures().join("constants.txt")).expect("run scripts/labelle-port-check.py");
            for row in text.lines() {
                assert!(
                    names.iter().any(|n| row.starts_with(n)) || not_ported.iter().any(|n| row.starts_with(n)),
                    "no test for `{row}`"
                );
            }
            for row in constants_rows() {
                assert!(names.iter().any(|n| row.starts_with(n)), "no test for ours: `{row}`");
            }
        }
    };
}

rows! {
    row_supported_products: "SUPPORTED_PRODUCTS ",
    row_unconfirmed_message: "UNCONFIRMED_MESSAGE ",
    row_dev_vendor: "DEV_VENDOR ",
    row_printer_interface_class: "PRINTER_INTERFACE_CLASS ",
    row_hid_interface_class: "HID_INTERFACE_CLASS ",
    row_esc: "ESC ",
    row_syn: "SYN ",
    row_default_margin_px: "DEFAULT_MARGIN_PX ",
    row_dpi: "DPI ",
    row_mm_per_inch: "MM_PER_INCH ",
    row_supported_tape_sizes: "SUPPORTED_TAPE_SIZES_MM ",
    row_default_tape_size: "DEFAULT_TAPE_SIZE_MM ",
    row_distance_head_cutter: "LABELER_DISTANCE_BETWEEN_PRINT_HEAD_AND_CUTTER_MM ",
    row_print_head_height: "LABELER_PRINT_HEAD_HEIGHT_MM ",
    row_command_status_request: "COMMAND _status_request ",
    row_command_tape_color: "COMMAND _tape_color ",
    row_command_bytes_per_line: "COMMAND _bytes_per_line ",
    row_max_lines: "_maxLines ",
    row_synwait: "synwait ",
}

/// `ImageOps.invert` and the dithering of `convert("1")`, on values worked out by hand from
/// `Convert.c:1348`.
#[test]
fn pillow_dither_and_invert() {
    let mut l = Image::new(Mode::L, 4, 1);
    l.data = vec![0, 255, 128, 129];
    let inverted = l.invert().unwrap();
    assert_eq!(inverted.data, vec![255, 0, 127, 126]);
    // 255 white, 0 black; 128 is not above 128; the error of the 128 (-128) is spread on.
    let one = Image::new(Mode::L, 3, 1);
    assert_eq!(one.convert_one().unwrap().data, vec![0, 0, 0]);
    let mut grey = Image::new(Mode::L, 3, 1);
    grey.data = vec![255, 255, 255];
    assert_eq!(grey.convert_one().unwrap().data, vec![255, 255, 255]);
}

#[test]
fn pillow_paste_clips_and_rotates() {
    let mut payload = Image::new(Mode::One, 3, 2);
    payload.data = vec![255, 0, 255, 0, 255, 0];
    // Pasted one column to the left: the first column is lost.
    let mut bitmap = Image::new(Mode::One, 5, 2);
    bitmap.paste(&payload, -1, 0).unwrap();
    assert_eq!(bitmap.data, vec![0, 255, 0, 0, 0, 255, 0, 0, 0, 0]);
    // ROTATE_270: out[x][height - 1 - y] = in[y][x].
    let rotated = payload.rotate_270();
    assert_eq!((rotated.width, rotated.height), (2, 3));
    assert_eq!(rotated.data, vec![0, 255, 255, 0, 0, 255]);
    // Packed eight to a byte, the first pixel in bit 7, a row padded to a byte.
    let mut bits = Image::new(Mode::One, 10, 1);
    bits.data = vec![255, 0, 0, 0, 0, 0, 0, 255, 255, 0];
    assert_eq!(bits.tobytes_one().unwrap(), vec![0b1000_0001, 0b1000_0000]);
}

/// Python's `round` is to the even one, `math.ceil` up: the margins of a picture, worked out
/// from `margins.py` and `cli.py` for labelle's defaults (justify left, margin 56) on 12 mm tape.
#[test]
fn margins_of_a_picture_on_12mm_tape() {
    use super::render_engines::print_payload::PrintPayloadRenderEngine;
    let labeler = dymo_labeler::DymoLabeler::new(Some(12)).unwrap();
    assert_eq!(labeler.height_px(), 64);
    let render = PrintPayloadRenderEngine::new(Direction::Left, constants::DEFAULT_MARGIN_PX, labeler.labeler_margin_px(), None, Some(0.0));
    let mut payload = Image::new(Mode::One, 100, 64);
    payload.data[0] = 255;
    let (bitmap, offset) = render.render_with_meta(&payload).unwrap();
    // 56 - 8.1 mm in pixels (57.40...) = -1.40...: pasted at -1; the width is 100 + 2 * 56.
    assert!((offset - (56.0 - 8.1 * 180.0 / 25.4)).abs() < 1e-9);
    assert_eq!((bitmap.width, bitmap.height), (212, 64));
    assert_eq!(bitmap.data[0], 0, "the payload's first column is cut off by the paste at -1");
}

#[test]
fn the_tape_defaults_as_labelle_does() {
    assert_eq!(dymo_labeler::DymoLabeler::new(None).unwrap().tape_size_mm, 12);
    assert_eq!(
        dymo_labeler::DymoLabeler::new(Some(24)).unwrap_err().to_string(),
        "Unsupported tape size 24mm. Supported sizes: (19, 12, 9, 6)"
    );
    let _ = Printer::Absent;
}

// ---- the engine's port (port.rs) ----

mod idle_poll {
    use super::*;

    fn poll(scene: &str, extra: Option<&str>) -> (Result<(), cli::CliError>, Vec<String>, Vec<Vec<u8>>, Option<Printer>) {
        usb_device::set_platform_for_test(Some("Linux"));
        let (backend, log) = fake_flagged(scene, extra);
        let mut printer = None;
        let mut heard = Vec::new();
        let result = cli::poll_status(backend, &mut |a| heard.push(a.to_vec()), &mut printer);
        let ops = log.borrow().clone();
        (result, ops, heard, printer)
    }

    #[test]
    fn one_poll_is_exactly_one_esc_a_and_one_read_then_the_release() {
        let (result, ops, heard, printer) = poll("single", None);
        assert!(result.is_ok());
        assert_eq!(printer, Some(Printer::Present));
        let writes: Vec<&String> = ops.iter().filter(|o| o.contains("_write ")).collect();
        let reads: Vec<&String> = ops.iter().filter(|o| o.contains("_read ") && !o.contains("ctrl")).collect();
        assert_eq!(writes.len(), 1, "{ops:?}");
        assert!(writes[0].contains("_write ") && writes[0].split(' ').nth(3) == Some("1b41"), "{writes:?}");
        assert_eq!(reads.len(), 1, "{ops:?}");
        assert!(reads[0].contains(" 512 "), "{reads:?}");
        assert_eq!(heard, vec![ANSWER.to_vec()]);
        // the write comes before the read, and the device is released after it
        let w = ops.iter().position(|o| o.contains("_write ")).unwrap();
        let r = ops.iter().position(|o| o.contains("_read ") && !o.contains("ctrl")).unwrap();
        assert!(w < r);
        assert!(ops[r + 1..].iter().any(|o| o.contains("release_interface") || o.contains("close_device")), "{ops:?}");
    }

    #[test]
    fn a_poll_with_no_printer_says_absent() {
        let (result, ops, _, printer) = poll("no_dymo", None);
        assert!(result.is_err());
        assert_eq!(printer, Some(Printer::Absent));
        assert!(!ops.iter().any(|o| o.contains("_write ")));
    }

    #[test]
    fn a_poll_that_times_out_releases_and_is_not_absent() {
        let (result, ops, _, printer) = poll("single", Some("failread=1"));
        assert!(result.is_err());
        assert_eq!(printer, Some(Printer::Present));
        let r = ops.iter().rposition(|o| o.contains("_read ") && !o.contains("ctrl")).unwrap();
        assert!(ops[r + 1..].iter().any(|o| o.contains("release_interface") || o.contains("close_device")), "{ops:?}");
    }
}

mod port {
    use super::super::port::{DymoPnp, NO_JOB_CONTROL, NO_QUESTION, NO_SUCH_CONTROL};
    use super::*;
    use crate::{Event, Order, Port};
    use maker_connect_protocol::{Action, JobControl, JobOrder, MachineState};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::time::Duration;

    /// A port over the scene `scene`, with its orders and events.
    fn start(scene: &'static str) -> (Sender<Order>, Receiver<Event>, std::thread::JoinHandle<()>) {
        usb_device::set_platform_for_test(None);
        let (orders, port_orders) = channel();
        let (port_events, events) = channel();
        let port = DymoPnp::with_backend(Box::new(move || Ok(fake(scene).0)));
        let run = std::thread::spawn(move || Box::new(port).run(port_orders, port_events));
        (orders, events, run)
    }

    fn job(id: i64, tape_mm: Option<u32>) -> JobOrder {
        JobOrder { id, name: "label.png".into(), download: "/f".into(), size: 0, from_line: None, tape_mm, label_length_mm: None }
    }

    fn next(events: &Receiver<Event>) -> Event {
        events.recv_timeout(Duration::from_secs(10)).expect("an event")
    }

    #[test]
    fn a_command_a_control_and_an_answer_fail_loudly() {
        let (orders, events, run) = start("normal");
        assert!(matches!(next(&events), Event::Telemetry(t) if t.state == MachineState::Idle));
        orders.send(Order::Command { id: 7, action: Action::Home { axes: Vec::new() } }).unwrap();
        match next(&events) {
            Event::Done(d) => assert_eq!((d.id, d.ok, d.reason.as_deref()), (7, false, Some(NO_SUCH_CONTROL))),
            other => panic!("{other:?}"),
        }
        orders.send(Order::Control(JobControl::Stop)).unwrap();
        assert!(matches!(next(&events), Event::Telemetry(t) if t.problem.as_deref() == Some(NO_JOB_CONTROL)));
        orders.send(Order::Answer(1)).unwrap();
        assert!(matches!(next(&events), Event::Telemetry(t) if t.problem.as_deref() == Some(NO_QUESTION)));
        orders.send(Order::Close).unwrap();
        run.join().unwrap();
    }

    #[test]
    fn a_job_ends_done_with_the_printers_answers_or_failed_with_labelles_text() {
        let (orders, events, run) = start("single");
        let png = fixtures().join("pngs/hub_label.png");
        let _ = next(&events);
        orders.send(Order::Job { order: job(5, Some(12)), file: png.clone() }).unwrap();
        let mut ended = None;
        let mut cassette = None;
        while ended.is_none() {
            match next(&events) {
                Event::JobEnd(end) => ended = Some(end),
                Event::Telemetry(t) if t.state == MachineState::Busy && t.raw_status.is_some() => {
                    cassette = t.raw_status.clone();
                    assert_eq!((t.cassette, t.battery_percent), (Some(true), Some(78)));
                }
                _ => {}
            }
        }
        let end = ended.unwrap();
        assert!(end.ok && end.id == 5, "{end:?}");
        assert_eq!(cassette, Some("40 00 12 4e 0d 6b 00 00".to_owned()));
        // An unsupported tape size: failed with labelle's text.
        orders.send(Order::Job { order: job(6, Some(24)), file: png.clone() }).unwrap();
        loop {
            if let Event::JobEnd(end) = next(&events) {
                assert!(!end.ok && end.id == 6);
                assert_eq!(end.reason.as_deref(), Some("Unsupported tape size 24mm. Supported sizes: (19, 12, 9, 6)"));
                break;
            }
        }
        orders.send(Order::Close).unwrap();
        run.join().unwrap();
    }

    #[test]
    fn no_printer_is_gone_and_found_again_is_back() {
        let png = fixtures().join("pngs/hub_label.png");
        let (orders, events, run) = start("no_dymo");
        let _ = next(&events);
        orders.send(Order::Job { order: job(1, Some(12)), file: png }).unwrap();
        let mut seen = Vec::new();
        loop {
            match next(&events) {
                Event::Gone => seen.push("gone"),
                Event::Back => seen.push("back"),
                Event::JobEnd(end) => {
                    assert_eq!(end.reason.as_deref(), Some("No supported devices found"));
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(seen, ["gone"]);
        orders.send(Order::Close).unwrap();
        run.join().unwrap();
    }

    /// A port whose backend is made by `make`, polling every `interval`; the times of its backend
    /// makings (one per poll or job) come on `made`.
    fn start_polling(
        interval: Duration,
        make: impl Fn(usize) -> &'static str + Send + 'static,
    ) -> (Sender<Order>, Receiver<Event>, Receiver<std::time::Instant>, std::thread::JoinHandle<()>) {
        usb_device::set_platform_for_test(None);
        let (orders, port_orders) = channel();
        let (port_events, events) = channel();
        let (made_tx, made) = channel();
        let count = std::sync::atomic::AtomicUsize::new(0);
        let port = DymoPnp::with_poll_interval(
            Box::new(move || {
                let n = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = made_tx.send(std::time::Instant::now());
                Ok(fake(make(n)).0)
            }),
            interval,
        );
        let run = std::thread::spawn(move || Box::new(port).run(port_orders, port_events));
        (orders, events, made, run)
    }

    #[test]
    fn it_polls_on_its_time_reports_raw_status_and_stays_idle() {
        let interval = Duration::from_millis(60);
        let began = std::time::Instant::now();
        let (orders, events, made, run) = start_polling(interval, |_| "single");
        let mut times = Vec::new();
        for _ in 0..3 {
            times.push(made.recv_timeout(Duration::from_secs(5)).expect("a poll"));
        }
        // the first poll one interval after start-up; each next one an interval after the last
        assert!(times[0].duration_since(began) >= interval);
        assert!(times[1].duration_since(times[0]) >= interval);
        assert!(times[2].duration_since(times[1]) >= interval);
        assert!(times[2].duration_since(times[1]) < interval * 10);
        orders.send(Order::Close).unwrap();
        run.join().unwrap();
        let mut raw = 0;
        for e in events.try_iter() {
            if let Event::Telemetry(t) = e {
                assert_eq!(t.state, MachineState::Idle);
                if t.raw_status.as_deref() == Some("40 00 12 4e 0d 6b 00 00") {
                    assert_eq!((t.cassette, t.battery_percent), (Some(true), Some(78)));
                    raw += 1;
                }
            }
        }
        assert!(raw >= 2, "{raw}");
    }

    #[test]
    fn no_poll_during_a_job_and_the_next_one_an_interval_after_it() {
        let interval = Duration::from_millis(60);
        let png = fixtures().join("pngs/hub_label.png");
        let gate = std::sync::Arc::new(std::sync::Mutex::new(()));
        let held = gate.lock().unwrap();
        let g = gate.clone();
        let (orders, events, made, run) = start_polling(interval, move |n| {
            if n == 0 {
                // the job's backend is made while the gate is held
                drop(g.lock().unwrap());
            }
            "single"
        });
        orders.send(Order::Job { order: job(1, Some(12)), file: png }).unwrap();
        let job_made = made.recv_timeout(Duration::from_secs(5)).expect("the job");
        // the job is held for several intervals: no poll starts
        std::thread::sleep(interval * 5);
        assert!(made.try_recv().is_err(), "a poll started during a job");
        drop(held);
        let mut ended = false;
        while !ended {
            ended = matches!(next(&events), Event::JobEnd(e) if e.ok);
        }
        let ended_at = std::time::Instant::now();
        let poll = made.recv_timeout(Duration::from_secs(5)).expect("a poll after the job");
        assert!(poll.duration_since(job_made) >= interval * 5);
        assert!(poll >= ended_at - Duration::from_millis(20) + interval - Duration::from_millis(20));
        orders.send(Order::Close).unwrap();
        run.join().unwrap();
    }

    #[test]
    fn a_job_waits_for_a_running_poll_to_release() {
        let interval = Duration::from_millis(30);
        let png = fixtures().join("pngs/hub_label.png");
        let gate = std::sync::Arc::new(std::sync::Mutex::new(()));
        let held = gate.lock().unwrap();
        let g = gate.clone();
        let (orders, events, made, run) = start_polling(interval, move |n| {
            if n == 0 {
                // the poll's backend is made while the gate is held
                drop(g.lock().unwrap());
            }
            "single"
        });
        // the poll has started (its backend is being made) ...
        made.recv_timeout(Duration::from_secs(5)).expect("the poll");
        // ... a job comes: nothing of it happens while the poll holds the printer
        orders.send(Order::Job { order: job(2, Some(12)), file: png }).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(!events.try_iter().any(|e| matches!(e, Event::JobEnd(_)) || matches!(&e, Event::Telemetry(t) if t.state == MachineState::Busy)));
        assert!(made.try_recv().is_err());
        drop(held);
        // the poll ends (Idle, raw status), then the job runs
        let mut order_seen = Vec::new();
        loop {
            match next(&events) {
                Event::Telemetry(t) if t.state == MachineState::Busy => order_seen.push("busy"),
                Event::Telemetry(t) if t.raw_status.is_some() && order_seen.is_empty() => order_seen.push("poll"),
                Event::JobEnd(e) => {
                    assert!(e.ok && e.id == 2);
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(order_seen.first(), Some(&"poll"));
        assert!(order_seen.contains(&"busy"));
        orders.send(Order::Close).unwrap();
        run.join().unwrap();
    }

    #[test]
    fn a_poll_says_gone_once_and_back_once_shared_with_the_job() {
        let interval = Duration::from_millis(30);
        let png = fixtures().join("pngs/hub_label.png");
        // polls 0 and 1 find no printer, the job (3rd) and later ones find it
        let (orders, events, made, run) = start_polling(interval, |n| if n < 2 { "no_dymo" } else { "single" });
        made.recv_timeout(Duration::from_secs(5)).unwrap();
        made.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut seen = Vec::new();
        orders.send(Order::Job { order: job(3, Some(12)), file: png }).unwrap();
        loop {
            match next(&events) {
                Event::Gone => seen.push("gone"),
                Event::Back => seen.push("back"),
                Event::JobEnd(e) => {
                    assert!(e.ok);
                    break;
                }
                _ => {}
            }
        }
        // later polls find it: no further Back
        std::thread::sleep(interval * 4);
        orders.send(Order::Close).unwrap();
        run.join().unwrap();
        for e in events.try_iter() {
            match e {
                Event::Gone => seen.push("gone"),
                Event::Back => seen.push("back"),
                _ => {}
            }
        }
        assert_eq!(seen, ["gone", "back"]);
    }

    #[test]
    fn a_failed_poll_is_no_gone_fails_no_job_and_the_next_one_comes() {
        let interval = Duration::from_millis(30);
        let (orders, events, made, run) = start_polling(interval, |n| if n == 0 { "no_interface" } else { "single" });
        made.recv_timeout(Duration::from_secs(5)).unwrap();
        made.recv_timeout(Duration::from_secs(5)).unwrap();
        orders.send(Order::Close).unwrap();
        run.join().unwrap();
        assert!(!events.try_iter().any(|e| matches!(e, Event::Gone | Event::JobEnd(_))));
    }
}
