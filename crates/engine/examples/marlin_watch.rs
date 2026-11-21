//! Watch a Marlin printer through the OctoPrint port: each second, its comm state and the identity
//! it reports. `marlin_watch <port> [baudrate] [seconds]`. Sends only what the port sends by itself.
use maker_connect_engine::Engine;
use maker_connect_engine::ports::octoprint::port::OctoPrintPort;
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let port = args.next().expect("a serial port");
    let baudrate = args.next().and_then(|b| b.parse().ok());
    let seconds = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);
    let mut engine = Engine::new();
    let id = engine.add(Box::new(OctoPrintPort::new(Some(port), baudrate)));
    let end = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_secs(1));
        if let Ok(r) = engine.report(id) {
            println!(
                "connected={} comm={:?} identity={:?}",
                r.connected, r.telemetry.comm_state, r.telemetry.identity
            );
        }
    }
}
