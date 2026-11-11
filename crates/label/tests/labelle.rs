//! Our DYMO LabelManager session against labelle's, operation for operation:
//! `scripts/dymo-audit-check.py` runs labelle's own code (real pyusb, real Pillow) on the
//! examples in `labelle/cases.txt` against a fake PnP and records every USB operation in
//! `labelle/labelle.txt`; this test runs ours (`dymo::session`) on a recorder with the same
//! descriptors, writes `labelle/ours.txt`, and fails on the first difference.

use maker_connect_label::Label;
use maker_connect_label::dymo::{
    AltSetting, Configuration, Tape, Usb, commands, encode, session, status_session,
};
use std::path::Path;

/// What the fake device answers a read: as the script's ANSWER.
const ANSWER: [u8; 8] = [0x40, 0x00, 0x12, 0x4E, 0x0D, 0x6B, 0x00, 0x00];

struct Endpoint {
    address: u8,
    interrupt: bool,
}

/// The PnP as the script describes it, every operation written down as pyusb's backend sees
/// it.
struct Recorder {
    active: u8,
    interfaces: Vec<(AltSetting, bool, Vec<Endpoint>)>,
    log: Vec<String>,
}

impl Recorder {
    fn interrupt(&self, endpoint: u8) -> bool {
        self.interfaces
            .iter()
            .flat_map(|(_, _, eps)| eps)
            .any(|e| e.address == endpoint && e.interrupt)
    }
}

impl Usb for Recorder {
    fn open_device(&mut self) -> Result<(), String> {
        self.log.push("open_device".into());
        Ok(())
    }
    fn configurations(&self) -> Vec<Configuration> {
        vec![Configuration {
            value: 1,
            interfaces: self.interfaces.iter().map(|(a, _, _)| a.clone()).collect(),
        }]
    }
    fn get_configuration(&mut self) -> Option<u8> {
        self.log.push(format!("get_configuration -> {}", self.active));
        (self.active != 0).then_some(self.active)
    }
    fn set_configuration(&mut self, value: u8) -> Result<(), String> {
        self.log.push(format!("set_configuration {value}"));
        self.active = value;
        Ok(())
    }
    /// nusb's `detach_and_claim_interface` (Linux `USBDEVFS_DISCONNECT_CLAIM`): the kernel
    /// asks whether a driver is bound, detaches it if so, and claims, as labelle's three calls.
    fn detach_and_claim(&mut self, interface: u8) -> Result<(), String> {
        let bound = self
            .interfaces
            .iter()
            .find(|(a, _, _)| a.number == interface)
            .is_some_and(|(_, b, _)| *b);
        let python = if bound { "True" } else { "False" };
        self.log.push(format!("is_kernel_driver_active {interface} -> {python}"));
        if bound {
            self.log.push(format!("detach_kernel_driver {interface}"));
        }
        self.log.push(format!("claim_interface {interface}"));
        Ok(())
    }
    fn write(&mut self, endpoint: u8, bytes: &[u8], timeout_ms: u32) -> Result<(), String> {
        let kind = if self.interrupt(endpoint) { "intr" } else { "bulk" };
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        self.log.push(format!("{kind}_write {endpoint:02x} {hex} {timeout_ms}"));
        Ok(())
    }
    fn read(&mut self, endpoint: u8, size: usize, timeout_ms: u32) -> Result<Vec<u8>, String> {
        let kind = if self.interrupt(endpoint) { "intr" } else { "bulk" };
        self.log.push(format!("{kind}_read {endpoint:02x} {size} {timeout_ms}"));
        Ok(ANSWER.to_vec())
    }
    fn release_and_close(&mut self, interface: u8) {
        self.log.push(format!("release_interface {interface}"));
        self.log.push("close_device".into());
    }
}

/// `device active 1 iface 0 0 03 + 01i,81i iface 1 0 08 + 02b,82b`
fn device(line: &str) -> Recorder {
    let words: Vec<&str> = line.split_whitespace().collect();
    let active = words[2].parse().unwrap();
    let interfaces = words[3..]
        .chunks(6)
        .map(|w| {
            let eps: Vec<Endpoint> = w[5]
                .split(',')
                .map(|e| Endpoint {
                    address: u8::from_str_radix(&e[..2], 16).unwrap(),
                    interrupt: e.ends_with('i'),
                })
                .collect();
            let alt = AltSetting {
                number: w[1].parse().unwrap(),
                alt: w[2].parse().unwrap(),
                class: u8::from_str_radix(w[3], 16).unwrap(),
                endpoints: eps.iter().map(|e| e.address).collect(),
            };
            (alt, w[4] == "+", eps)
        })
        .collect();
    Recorder {
        active,
        interfaces,
        log: Vec::new(),
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// One example run through our session.
fn run(device_line: &str, kind: &str, mm: u32, rows: &[&str]) -> Vec<String> {
    let mut usb = device(device_line);
    let cmds = if kind == "lines" {
        commands(rows.iter().map(|r| unhex(r)).collect())
    } else {
        let (width, height) = (rows[0].len(), rows.len());
        let mut label = Label::blank(width, height);
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                label.set(x, y, c == '1');
            }
        }
        encode(&label, Tape::d1(mm).unwrap()).unwrap()
    };
    session(&mut usb, &cmds, |_| {}, |_, _| {}).unwrap();
    usb.log
}

#[test]
fn every_example_goes_as_labelle_sends_it() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/labelle");
    let cases = std::fs::read_to_string(dir.join("cases.txt")).expect("run scripts/dymo-audit-check.py");
    let mut lines = cases.lines();
    let device_line = lines.next().unwrap().to_owned();
    let mut out = String::new();
    let mut current: Option<(String, String, u32, Vec<&str>)> = None;
    let mut finish = |c: Option<(String, String, u32, Vec<&str>)>, out: &mut String| {
        if let Some((name, kind, mm, rows)) = c {
            out.push_str(&format!("case {name}\n"));
            for op in run(&device_line, &kind, mm, &rows) {
                out.push_str(&op);
                out.push('\n');
            }
        }
    };
    let mut expect_kind = false;
    for line in lines {
        if let Some(name) = line.strip_prefix("case ") {
            finish(current.take(), &mut out);
            current = Some((name.to_owned(), String::new(), 0, Vec::new()));
            expect_kind = true;
        } else if expect_kind {
            let (kind, mm) = line.split_once(' ').unwrap();
            let c = current.as_mut().unwrap();
            c.1 = kind.to_owned();
            c.2 = mm.parse().unwrap();
            expect_kind = false;
        } else if !line.is_empty() {
            current.as_mut().unwrap().3.push(line);
        }
    }
    finish(current.take(), &mut out);
    std::fs::write(dir.join("ours.txt"), &out).unwrap();
    let theirs = std::fs::read_to_string(dir.join("labelle.txt")).unwrap();
    for (i, (a, b)) in theirs.lines().zip(out.lines()).enumerate() {
        assert_eq!(a, b, "line {} of labelle.txt", i + 1);
    }
    assert_eq!(theirs.lines().count(), out.lines().count(), "as many operations");
}

/// Our idle status request (ours, not labelle's: the owner's decision 2026-11-11) opens and
/// lets go of the printer exactly as labelle's recorded sessions do; in between only `ESC A`
/// and one read.
#[test]
fn the_idle_status_request_opens_and_closes_as_labelle_does() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/labelle");
    let cases = std::fs::read_to_string(dir.join("cases.txt")).expect("run scripts/dymo-audit-check.py");
    let theirs = std::fs::read_to_string(dir.join("labelle.txt")).unwrap();
    let first: Vec<&str> = theirs.lines().skip(1).take_while(|l| !l.starts_with("case ")).collect();
    let opening: Vec<&str> = first.iter().take_while(|l| !l.contains("_write")).copied().collect();
    let closing: Vec<&str> = first[first.len() - 2..].to_vec();
    let mut usb = device(cases.lines().next().unwrap());
    status_session(&mut usb, |_| {}).unwrap();
    let ours: Vec<&str> = usb.log.iter().map(String::as_str).collect();
    let mut expected = opening.clone();
    expected.extend(["intr_write 01 1b41 1000", "intr_read 81 512 1000"]);
    expected.extend(closing);
    assert_eq!(ours, expected);
}
