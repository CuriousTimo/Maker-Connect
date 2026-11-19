//! The scale port's tests (Step 3e): the discard count, one event per change, Gone and Back, Close, and
//! that nothing is ever sent to the scale; over a fake HID source. The decoding against usbscale's own
//! code is `scripts/usbscale-audit-check.py`.

use super::hid::{self, HidDevice, HidInfo, HidSource, SCALE_USAGE_PAGE};
use super::port::{self, DISCARDED_PER_OPEN, FIND_INTERVAL, READ_TIMEOUT_MS, UsbScale};
use super::usbscale::{self, Rejected, WEIGH_COUNT};
use crate::{Event, Order, Port};
use maker_connect_protocol::{MachineState, ScaleReading};
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

enum Item {
    Report(Vec<u8>),
    Fail,
}

#[derive(Default)]
struct World {
    devices: Vec<HidInfo>,
    /// One script per open; a script that has run out reads timeouts.
    scripts: VecDeque<VecDeque<Item>>,
    /// Every call the port made, by name.
    log: Vec<String>,
}

type Shared = Arc<Mutex<World>>;

struct Fake(Shared);
struct FakeDevice(Shared, VecDeque<Item>);

impl HidSource for Fake {
    fn list(&mut self) -> Result<Vec<HidInfo>, String> {
        let mut world = self.0.lock().unwrap();
        world.log.push("list".into());
        Ok(world.devices.clone())
    }
    fn open(&mut self, path: &str) -> Result<Box<dyn HidDevice>, String> {
        let mut world = self.0.lock().unwrap();
        world.log.push("open".into());
        if !world.devices.iter().any(|d| d.path == path) {
            return Err("no such device".into());
        }
        let script = world.scripts.pop_front().unwrap_or_default();
        Ok(Box::new(FakeDevice(self.0.clone(), script)))
    }
}

impl HidDevice for FakeDevice {
    fn read(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, String> {
        self.0.lock().unwrap().log.push("read".into());
        match self.1.pop_front() {
            Some(Item::Report(bytes)) => {
                buf[..bytes.len()].copy_from_slice(&bytes);
                Ok(bytes.len())
            }
            Some(Item::Fail) => Err("device gone".into()),
            None => {
                std::thread::sleep(Duration::from_millis(timeout_ms as u64));
                Ok(0)
            }
        }
    }
}

fn info(path: &str, usage_page: u16) -> HidInfo {
    HidInfo {
        path: path.into(),
        vendor_id: 0x0922,
        product_id: 0x8003,
        product: "M2".into(),
        usage_page,
    }
}

/// A report: ID 3, status 4, unit 2 (g), exponent 0, the weight.
fn weight(grams: u16) -> Item {
    Item::Report(vec![3, 4, 2, 0, grams as u8, (grams >> 8) as u8])
}

fn start(
    world: &Shared,
    find: Duration,
    read_ms: i32,
) -> (Sender<Order>, Receiver<Event>, std::thread::JoinHandle<()>) {
    let (orders, port_orders) = channel();
    let (port_events, events) = channel();
    let world = world.clone();
    let port = UsbScale::with_times(
        Box::new(move || Ok(Box::new(Fake(world.clone())) as Box<dyn HidSource>)),
        None,
        find,
        read_ms,
    );
    let run = std::thread::spawn(move || Box::new(port).run(port_orders, port_events));
    (orders, events, run)
}

fn world_with(scripts: Vec<Vec<Item>>) -> Shared {
    let world = World {
        devices: vec![
            info("/dev/hidraw3", SCALE_USAGE_PAGE),
            info("/dev/hidraw0", 1),
        ],
        scripts: scripts.into_iter().map(VecDeque::from).collect(),
        log: Vec::new(),
    };
    Arc::new(Mutex::new(world))
}

fn next(events: &Receiver<Event>) -> Event {
    events
        .recv_timeout(Duration::from_secs(5))
        .expect("an event")
}

fn reading(event: &Event) -> Option<&ScaleReading> {
    match event {
        Event::Telemetry(t) => t.scale.as_ref(),
        _ => None,
    }
}

fn nothing_more(events: &Receiver<Event>) {
    assert!(
        events.recv_timeout(Duration::from_millis(150)).is_err(),
        "an event where none was due"
    );
}

#[test]
fn the_constants_are_usbscales_and_the_owners() {
    assert_eq!(WEIGH_COUNT, 2, "scales.h:15");
    assert_eq!(DISCARDED_PER_OPEN, 2);
    assert_eq!(FIND_INTERVAL, Duration::from_secs(15), "E5");
    assert_eq!(READ_TIMEOUT_MS, 1000, "E6");
}

/// usbscale.c:179, 322-327: the extra read and the loop's first are thrown away; the third report is the
/// first one decoded, after each open.
#[test]
fn the_old_data_is_discarded_after_each_open() {
    let world = world_with(vec![
        vec![weight(1), weight(2), weight(3), weight(4)],
        vec![weight(11), weight(12), weight(13)],
    ]);
    let (orders, events, run) = start(&world, Duration::from_millis(30), 20);
    let idle = next(&events);
    assert!(
        matches!(&idle, Event::Telemetry(t) if t.state == MachineState::Idle && t.scale.is_none())
    );
    assert_eq!(
        reading(&next(&events)).unwrap().raw,
        3,
        "the 1st and 2nd reports are discarded"
    );
    assert_eq!(reading(&next(&events)).unwrap().raw, 4);
    // the second open (after a failed read: the script ran out reads timeouts, so close here)
    orders.send(Order::Close).unwrap();
    run.join().unwrap();
}

#[test]
fn the_discard_starts_again_with_each_open() {
    let world = world_with(vec![
        vec![weight(1), weight(2), weight(3), Item::Fail],
        vec![weight(11), weight(12), weight(13)],
    ]);
    let (orders, events, run) = start(&world, Duration::from_millis(30), 20);
    let mut seen = Vec::new();
    while seen.len() < 2 {
        if let Some(r) = reading(&next(&events)) {
            seen.push(r.raw);
        }
    }
    assert_eq!(seen, [3, 13], "the third report of each open");
    orders.send(Order::Close).unwrap();
    run.join().unwrap();
}

#[test]
fn one_event_per_change_and_nothing_on_no_change() {
    let world = world_with(vec![vec![
        weight(0),
        weight(0),
        weight(10),
        weight(10),
        weight(10),
        weight(11),
        // an unknown unit and an unknown status are shown, not refused (E3, E4)
        Item::Report(vec![3, 9, 200, 0xFF, 1, 0]),
        Item::Report(vec![3, 9, 200, 0xFF, 1, 0]),
    ]]);
    let (orders, events, run) = start(&world, Duration::from_millis(30), 20);
    next(&events); // Idle, nothing read yet
    assert_eq!(reading(&next(&events)).unwrap().raw, 10);
    assert_eq!(reading(&next(&events)).unwrap().raw, 11);
    let odd = next(&events);
    let odd = reading(&odd).unwrap();
    assert_eq!(
        (
            odd.status_text.as_str(),
            odd.unit_name.as_str(),
            odd.exponent
        ),
        ("unknown 9", "unknown unit 200", -1)
    );
    assert_eq!(odd.weight, 0.1);
    nothing_more(&events);
    orders.send(Order::Close).unwrap();
    run.join().unwrap();
}

/// E8: a report that does not start with 3 or 4 is refused as usbscale refuses it, and reading goes on.
#[test]
fn a_report_usbscale_refuses_is_shown_once_and_reading_goes_on() {
    let world = world_with(vec![vec![
        weight(0),
        weight(0),
        // a 7-byte form with a leading 00: not stripped (E8), refused
        Item::Report(vec![0, 3, 4, 2, 0, 5, 0]),
        Item::Report(vec![0, 3, 4, 2, 0, 6, 0]),
        weight(7),
    ]]);
    let (orders, events, run) = start(&world, Duration::from_millis(30), 20);
    next(&events);
    let refused = next(&events);
    assert!(
        matches!(&refused, Event::Telemetry(t) if t.problem.as_deref() == Some("Error reading scale data") && t.scale.is_none())
    );
    let good = next(&events);
    assert!(
        matches!(&good, Event::Telemetry(t) if t.problem.is_none() && t.scale.as_ref().unwrap().raw == 7)
    );
    orders.send(Order::Close).unwrap();
    run.join().unwrap();
}

/// E7, E5: a read error is Gone; the scale is looked for again every 15 s (here a test interval), found: Back.
#[test]
fn a_read_error_is_gone_and_a_refind_is_back_after_the_interval() {
    let interval = Duration::from_millis(300);
    let world = world_with(vec![
        vec![weight(1), weight(2), weight(3), Item::Fail],
        vec![weight(8), weight(9), weight(10)],
    ]);
    // the device is unplugged: listed again only later
    let saved = std::mem::take(&mut world.lock().unwrap().devices);
    let (orders, events, run) = start(&world, interval, 20);
    assert_eq!(next(&events), Event::Gone, "not found at the start");
    let gone_at = Instant::now();
    std::thread::sleep(Duration::from_millis(100));
    world.lock().unwrap().devices = saved;
    // found on the next look, an interval after the last
    let back_at = loop {
        if next(&events) == Event::Back {
            break Instant::now();
        }
    };
    assert!(
        back_at - gone_at >= interval - Duration::from_millis(20),
        "looked for again only after the interval"
    );
    let mut raws = Vec::new();
    let mut gone_again = false;
    while raws.len() < 1 || !gone_again {
        match next(&events) {
            Event::Gone => gone_again = true,
            e => raws.extend(reading(&e).map(|r| r.raw)),
        }
    }
    assert_eq!(raws, [3], "after the open, the third report");
    // the read error is Gone; it is found again by the next look and the second script reads on
    let mut back = false;
    let mut last = 0;
    while !back || last != 10 {
        match next(&events) {
            Event::Back => back = true,
            e => last = reading(&e).map_or(last, |r| r.raw),
        }
    }
    orders.send(Order::Close).unwrap();
    run.join().unwrap();
}

/// E6: Close is seen within the read timeout (here 100 ms), reading or waiting to look again.
#[test]
fn close_is_seen_within_the_read_timeout() {
    let world = world_with(vec![vec![]]);
    let (orders, events, run) = start(&world, Duration::from_secs(60), 100);
    next(&events);
    let at = Instant::now();
    orders.send(Order::Close).unwrap();
    run.join().unwrap();
    // A slow CI runner can stall a thread for some hundreds of ms; the bound stays far
    // below the 60 s look-again interval this test tells it apart from.
    assert!(
        at.elapsed() < Duration::from_millis(100 + 900),
        "took {:?}",
        at.elapsed()
    );
    // while gone and waiting to look again, at once
    let world = world_with(vec![]);
    world.lock().unwrap().devices.clear();
    let (orders, events, run) = start(&world, Duration::from_secs(60), 100);
    assert_eq!(next(&events), Event::Gone);
    let at = Instant::now();
    orders.send(Order::Close).unwrap();
    run.join().unwrap();
    assert!(at.elapsed() < Duration::from_millis(1000));
}

/// Nothing is sent to the scale: the fake source and device are asked only to list, open and read, and the
/// real HID file has no write, no feature report and no GET_REPORT in it.
#[test]
fn nothing_is_ever_sent_to_the_scale() {
    let world = world_with(vec![vec![weight(1), weight(2), weight(3), weight(4)]]);
    let (orders, events, run) = start(&world, Duration::from_millis(30), 20);
    next(&events);
    next(&events);
    // a command and a job are refused, not passed on
    orders
        .send(Order::Command {
            id: 7,
            action: maker_connect_protocol::Action::Unlock,
        })
        .unwrap();
    let done = loop {
        if let Event::Done(d) = next(&events) {
            break d;
        }
    };
    assert!(!done.ok && done.reason.as_deref() == Some(port::NO_COMMANDS));
    orders.send(Order::Close).unwrap();
    run.join().unwrap();
    let log = world.lock().unwrap().log.clone();
    assert!(
        log.iter()
            .all(|c| ["list", "open", "read"].contains(&c.as_str())),
        "{log:?}"
    );
    assert!(log.iter().filter(|c| *c == "read").count() >= 4);
    // by construction: the traits have nothing to send with
    let source = include_str!("hid.rs");
    for call in [
        "write",
        "send_feature_report",
        "get_feature_report",
        "send_output_report",
        "get_input_report",
        "set_blocking_mode",
    ] {
        assert!(
            !source.contains(&format!(".{call}(")),
            "hid.rs calls {call}"
        );
    }
    assert!(
        !source.contains("fn write") && !source.contains("fn send") && !source.contains("fn get_")
    );
}

#[test]
fn the_device_is_chosen_by_usage_page_never_guessed() {
    let mut world = World::default();
    let shared: Shared = Arc::new(Mutex::new(std::mem::take(&mut world)));
    let mut source = Fake(shared.clone());
    // no scale: refused, what was seen is listed
    shared.lock().unwrap().devices = vec![info("/dev/hidraw0", 1), info("/dev/hidraw1", 0xFF00)];
    let error = hid::select(&mut source, None).unwrap_err();
    assert!(
        error.contains("/dev/hidraw0  0922:8003  M2  usage page 0x01")
            && error.contains("usage page 0xff00"),
        "{error}"
    );
    // one: it
    shared
        .lock()
        .unwrap()
        .devices
        .push(info("/dev/hidraw3", SCALE_USAGE_PAGE));
    assert_eq!(hid::select(&mut source, None).unwrap(), "/dev/hidraw3");
    // two: refused and both listed (not the others)
    shared
        .lock()
        .unwrap()
        .devices
        .push(info("/dev/hidraw4", SCALE_USAGE_PAGE));
    let error = hid::select(&mut source, None).unwrap_err();
    assert!(
        error.contains("2 HID devices")
            && error.contains("/dev/hidraw3")
            && error.contains("/dev/hidraw4")
            && !error.contains("/dev/hidraw0"),
        "{error}"
    );
    // --port is taken as given
    assert_eq!(
        hid::select(&mut source, Some("/dev/hidraw9")).unwrap(),
        "/dev/hidraw9"
    );
    // port::check is the same, over the real source: only that it exists
    let _: fn(Option<&str>) -> Result<(), String> = port::check;
}

#[test]
fn print_scale_data_reads_the_six_bytes() {
    let r = usbscale::print_scale_data(&[3, 4, 11, 0xFF, 0x34, 0x12]).unwrap();
    assert_eq!(
        (
            r.status,
            r.status_text.as_str(),
            r.unit,
            r.unit_name.as_str(),
            r.exponent,
            r.raw
        ),
        (4, "", 11, "oz", -1, 0x1234)
    );
    assert_eq!(r.weight, 4660.0 * 0.1);
    assert_eq!(
        usbscale::print_scale_data(&[4, 2, 2, 0, 0, 0])
            .unwrap()
            .status_text,
        "Scale is zero'd...",
        "report ID 4 as 3 (E9)"
    );
    assert_eq!(
        usbscale::print_scale_data(&[0, 3, 4, 2, 0, 5, 0]),
        Err(Rejected::NotAReport(0))
    );
    assert_eq!(
        usbscale::print_scale_data(&[3, 4, 2]),
        Err(Rejected::Short(3))
    );
    assert_eq!(usbscale::UNITS.len(), 13);
}
