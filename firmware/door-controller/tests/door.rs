use door_controller::connect::Client;
use door_controller::device::Printer;
use door_controller::fakes::*;
use door_controller::status::ConnectionStatus;
use door_controller::traits::*;
use serde_json::{Value, json};

struct Rig {
    clock: FakeClock,
    relays: FakeRelays,
    inputs: FakeInputs,
    store: FakeStore,
    net: FakeNet,
    printer: Printer,
    client: Client,
}

fn rig_with(store: FakeStore) -> Rig {
    let clock = FakeClock::default();
    let relays = FakeRelays::new(&clock);
    let inputs = FakeInputs::new(vec![
        DoorState::Closed,
        DoorState::Closed,
        // No switch wired: the input reads open.
        DoorState::Open,
        DoorState::Open,
    ]);
    let net = FakeNet::new(&clock);
    let mut printer = Printer::new(
        Box::new(relays.clone()),
        Box::new(inputs.clone()),
        Box::new(clock.clone()),
        Box::new(store.clone()),
        4,
        "0.1.0",
    );
    let client = Client::new(&mut printer);
    Rig {
        clock,
        relays,
        inputs,
        store,
        net,
        printer,
        client,
    }
}

fn rig() -> Rig {
    rig_with(FakeStore::with_token("TOKEN"))
}

impl Rig {
    fn step(&mut self) {
        self.client.run_once(&mut self.printer, &mut self.net);
    }
    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }
    /// Run until the predicate holds on the sent frames; panic after many steps.
    fn until(&mut self, what: &str, pred: impl Fn(&[Value]) -> bool) {
        for _ in 0..2000 {
            if pred(&self.frames()) {
                return;
            }
            self.step();
        }
        panic!("never happened: {what}; frames: {:?}", self.frames());
    }
    fn frames(&self) -> Vec<Value> {
        self.net
            .texts()
            .iter()
            .map(|t| serde_json::from_str(t).expect("json"))
            .collect()
    }
    fn events(&self) -> Vec<Value> {
        self.frames()
            .into_iter()
            .filter(|f| f.get("event").is_some())
            .collect()
    }
    fn settle(&mut self) {
        self.until("first telemetry", |f| {
            f.iter().any(|v| v.get("event").is_none())
        });
    }
    fn command(&mut self, id: u32, body: Value) {
        self.net.push_json_command(id, &body.to_string());
    }
    fn answer_for(&mut self, id: u32, last_event: &str) -> Value {
        self.until("answer", |f| {
            f.iter()
                .any(|v| v["command_id"] == id && v["event"] == last_event)
        });
        self.events()
            .into_iter()
            .rev()
            .find(|v| v["command_id"] == id && v["event"] == last_event)
            .unwrap()
    }
    fn open_lock(&mut self, id: u32, lock: u32, expires_at: u64) {
        self.command(
            id,
            json!({"command": "OPEN_LOCK", "kwargs": {"lock": lock, "expires_at": expires_at}}),
        );
    }
}

fn pair(r: &mut Rig) {
    r.net.http_answers.push_back(Ok(HttpResponse {
        status: 200,
        headers: vec![("Code".into(), "ABCDEFGHJK".into())],
    }));
    r.net.http_answers.push_back(Ok(HttpResponse {
        status: 202,
        headers: vec![],
    }));
    r.net.http_answers.push_back(Ok(HttpResponse {
        status: 200,
        headers: vec![("Token".into(), "SECRETTOKEN".into())],
    }));
}

#[test]
fn registration_by_code() {
    let mut r = rig_with(FakeStore::default());
    pair(&mut r);
    r.client.request_registration();
    for _ in 0..400 {
        r.step();
        if r.client.last_known_status == ConnectionStatus::RegistrationDone {
            break;
        }
    }
    assert_eq!(
        r.client.last_known_status,
        ConnectionStatus::RegistrationDone
    );
    assert_eq!(r.client.registration_code(), Some("ABCDEFGHJK"));
    assert_eq!(r.store.stored_token().as_deref(), Some("SECRETTOKEN"));

    let (m, path, headers, body) = &r.net.http_log[0];
    assert_eq!((*m, *path), (Method::Post, "/p/register"));
    assert!(
        headers
            .iter()
            .all(|(n, _)| n != "Token" && n != "Fingerprint")
    );
    let body: Value = serde_json::from_str(body).unwrap();
    assert_eq!(
        body,
        json!({"sn": "DOOR-0001", "fingerprint": "fingerprint-of-door-0001",
               "printer_type": "maker-connect.door-controller", "firmware": "0.1.0"})
    );
    // The first poll is five seconds after the POST, with the code in the header Code.
    let polls: Vec<_> = r.net.http_log.iter().skip(1).collect();
    assert_eq!(polls.len(), 2);
    for (m, path, headers, body) in polls {
        assert_eq!((*m, *path), (Method::Get, "/p/register"));
        assert_eq!(
            headers,
            &vec![("Code".to_string(), "ABCDEFGHJK".to_string())]
        );
        assert!(body.is_empty());
    }
    assert!(r.clock.now_ms() >= 10_000);

    // Leaving the registration starts the session with the new token.
    r.client.leave_registration();
    r.settle();
    let h = &r.net.upgrade_headers[0];
    assert!(h.contains(&("Token".to_string(), "SECRETTOKEN".to_string())));
    assert!(h.contains(&("Fingerprint".to_string(), "fingerprint-of-d".to_string())));
    assert!(h.contains(&(
        "Sec-WebSocket-Protocol".to_string(),
        "prusa-connect".to_string()
    )));
}

#[test]
fn registration_stops_after_three_failures_and_starts_again_after_a_minute() {
    let mut r = rig_with(FakeStore::default());
    r.client.request_registration();
    for _ in 0..1000 {
        if r.net.http_log.len() == 3 {
            break;
        }
        r.step();
    }
    assert_eq!(r.net.http_log.len(), 3);
    // Quiet within the minute (the board sleeps 125 ms a step).
    r.steps(100);
    assert_eq!(
        r.client.last_known_status,
        ConnectionStatus::RegistrationError
    );
    assert_eq!(r.net.http_log.len(), 3);
    // After the minute: a new POST /p/register.
    r.steps(1000);
    assert!(r.net.http_log.len() > 3);
    assert_eq!(r.net.http_log[3].0, Method::Post);
}

#[test]
fn info_is_first_and_carries_part_7() {
    let mut r = rig();
    r.settle();
    let info = &r.frames()[0];
    assert_eq!(info["event"], "INFO");
    assert!(info.get("command_id").is_none());
    assert_eq!(info["state"], "IDLE");
    let d = &info["data"];
    assert_eq!(d["printer_type"], "maker-connect.door-controller");
    assert_eq!(d["lock_count"], 4);
    assert_eq!(d["extensions"], json!(["door.lock"]));
    assert_eq!(d["sn"], "DOOR-0001");
    assert!(d.get("agent_id").is_none());
    assert!(d.get("storages").is_none());
}

#[test]
fn send_info_answers_info_with_the_command_id() {
    let mut r = rig();
    r.settle();
    r.command(0x2a, json!({"command": "SEND_INFO"}));
    let ev = r.answer_for(0x2a, "INFO");
    assert_eq!(ev["data"]["lock_count"], 4);
}

#[test]
fn telemetry_has_locks_in_lock_order() {
    let mut r = rig();
    r.settle();
    let t = r
        .frames()
        .into_iter()
        .find(|f| f.get("event").is_none())
        .unwrap();
    assert_eq!(
        t["locks"],
        json!([
            {"lock": 0, "door": "closed", "resting_s": 0},
            {"lock": 1, "door": "closed", "resting_s": 0},
            {"lock": 2, "door": "open", "resting_s": 0},
            {"lock": 3, "door": "open", "resting_s": 0}
        ])
    );
    assert_eq!(t["state"], "IDLE");
}

#[test]
fn a_door_change_sends_telemetry_at_once() {
    let mut r = rig();
    r.settle();
    let before = r.frames().len();
    let t0 = r.clock.now_ms();
    r.inputs.set(1, DoorState::Open);
    r.until("telemetry with the open door", |f| {
        f.len() > before && f[before..].iter().any(|v| v["locks"][1]["door"] == "open")
    });
    // Buddy's TELEMETRY_INTERVAL_MIN (750 ms) plus its loop wake-up (500 ms) at the latest, not the 4 s pace.
    assert!(
        r.clock.now_ms() - t0 <= 1250,
        "took {} ms",
        r.clock.now_ms() - t0
    );
    // A change is no event of its own.
    assert!(r.events().iter().all(|e| e["event"] == "INFO"));
}

#[test]
fn open_lock_pulses_once_and_finishes() {
    let mut r = rig();
    r.clock.set_unix(Some(1000));
    r.settle();
    r.open_lock(7, 1, 1005);
    let acc = r.answer_for(7, "ACCEPTED");
    assert!(acc.get("reason").is_none());
    let fin = r.answer_for(7, "FINISHED");
    assert_eq!(fin["state"], "IDLE");
    assert_eq!(r.relays.pulses(), vec![(1, 500)]);
    // The rest is in the telemetry afterwards.
    r.until("resting", |f| {
        f.iter()
            .any(|v| v["locks"][1]["resting_s"].as_u64().unwrap_or(0) > 0)
    });
}

#[test]
fn open_lock_while_resting_is_rejected_then_allowed_after_the_rest() {
    let mut r = rig();
    r.clock.set_unix(Some(1000));
    r.settle();
    r.open_lock(1, 0, 1005);
    r.answer_for(1, "FINISHED");
    r.open_lock(2, 0, 1005);
    let rej = r.answer_for(2, "REJECTED");
    assert_eq!(rej["reason"], "Lock resting");
    assert_eq!(rej["machine_reason"], "LOCK_RESTING");
    assert_eq!(r.relays.pulses().len(), 1);
    // Another lock is not resting.
    r.open_lock(3, 1, 1005);
    r.answer_for(3, "FINISHED");
    // 10 s after the pulse ended the lock may be opened again.
    r.clock.advance(10_000);
    r.open_lock(4, 0, 1005);
    r.answer_for(4, "FINISHED");
    assert_eq!(r.relays.pulses(), vec![(0, 500), (1, 500), (0, 500)]);
}

#[test]
fn open_lock_expired_no_time_and_no_such_lock() {
    let mut r = rig();
    r.settle();
    // The clock is not set: "No time".
    r.open_lock(1, 0, 4_000_000_000);
    assert_eq!(r.answer_for(1, "REJECTED")["reason"], "No time");
    assert_eq!(r.answer_for(1, "REJECTED")["machine_reason"], "NO_TIME");
    r.clock.set_unix(Some(2000));
    // Past expires_at: "Expired".
    r.open_lock(2, 0, 1999);
    assert_eq!(r.answer_for(2, "REJECTED")["reason"], "Expired");
    assert_eq!(r.answer_for(2, "REJECTED")["machine_reason"], "EXPIRED");
    // Exactly at expires_at it still runs.
    r.open_lock(3, 0, 2000);
    r.answer_for(3, "FINISHED");
    // The lock must exist.
    r.open_lock(4, 4, 2005);
    assert_eq!(r.answer_for(4, "REJECTED")["reason"], "No such lock");
    assert_eq!(
        r.answer_for(4, "REJECTED")["machine_reason"],
        "NO_SUCH_LOCK"
    );
    assert_eq!(r.relays.pulses(), vec![(0, 500)]);
}

#[test]
fn open_lock_without_its_arguments_is_rejected() {
    let mut r = rig();
    r.clock.set_unix(Some(1000));
    r.settle();
    r.command(1, json!({"command": "OPEN_LOCK", "kwargs": {"lock": 0}}));
    assert_eq!(
        r.answer_for(1, "REJECTED")["reason"],
        "Missing or broken parameters"
    );
    r.command(
        2,
        json!({"command": "OPEN_LOCK", "kwargs": {"lock": "0", "expires_at": 1005}}),
    );
    assert_eq!(
        r.answer_for(2, "REJECTED")["reason"],
        "Missing or broken parameters"
    );
    assert!(r.relays.pulses().is_empty());
}

#[test]
fn unknown_command_is_rejected() {
    let mut r = rig();
    r.settle();
    r.command(
        9,
        json!({"command": "START_PRINT", "kwargs": {"path": "/usb/x.gcode"}}),
    );
    assert_eq!(r.answer_for(9, "REJECTED")["reason"], "Unknown command");
    r.command(10, json!({"command": "SET_SERVER_INFO", "kwargs": {}}));
    assert_eq!(r.answer_for(10, "REJECTED")["reason"], "Unknown command");
}

#[test]
fn the_same_command_id_does_not_run_twice() {
    let mut r = rig();
    r.clock.set_unix(Some(1000));
    r.settle();
    r.open_lock(5, 0, 1005);
    r.answer_for(5, "FINISHED");
    r.clock.advance(10_000);
    r.open_lock(5, 0, 1005);
    let rej = r.answer_for(5, "REJECTED");
    assert_eq!(
        rej["reason"],
        "Won't execute the same command multiple times"
    );
    assert_eq!(r.relays.pulses().len(), 1);
}

#[test]
fn nothing_opens_without_the_server() {
    // No socket: the board answers nothing and pulses nothing, however long it runs.
    let mut r = rig();
    r.net
        .upgrades
        .extend((0..50).map(|_| Ok(WsUpgrade::Refused(503))));
    r.steps(100);
    assert!(r.relays.pulses().is_empty());
    assert!(!r.client.websocket_open());
}

#[test]
fn a_relay_that_does_not_answer_is_an_error() {
    let mut r = rig();
    r.clock.set_unix(Some(1000));
    r.settle();
    r.relays.set_fail(true);
    r.open_lock(1, 0, 1005);
    let failed = r.answer_for(1, "FAILED");
    assert_eq!(failed["state"], "ERROR");
    // In the error state only SEND_INFO is taken (Buddy's whitelist).
    r.relays.set_fail(false);
    r.open_lock(2, 0, 1005);
    assert_eq!(
        r.answer_for(2, "REJECTED")["reason"],
        "Won't accept commands in error state"
    );
    assert!(r.relays.pulses().is_empty());
}

#[test]
fn a_command_from_a_wrong_message_is_rejected_not_run() {
    let mut r = rig();
    r.settle();
    r.net
        .inbound
        .push_back(WsMessage::Data(b"Gzzzzzzzz".to_vec()));
    r.steps(5);
    r.net
        .inbound
        .push_back(WsMessage::Data(b"G0000000bM17".to_vec()));
    assert_eq!(
        r.answer_for(0xb, "REJECTED")["reason"],
        "Unrecognized type of message"
    );
    r.net
        .inbound
        .push_back(WsMessage::Data(b"J0000000c{not json".to_vec()));
    assert_eq!(
        r.answer_for(0xc, "REJECTED")["reason"],
        "Error parsing JSON"
    );
}

#[test]
fn pings_are_sent_when_silent_and_answered() {
    let mut r = rig();
    r.settle();
    // Silence on our side for more than 15 s while the telemetry is held back by a failing socket is
    // not testable here; instead check the answer to the server's ping.
    r.net.inbound.push_back(WsMessage::Ping(b"hi".to_vec()));
    r.steps(5);
    assert!(r.net.sent.contains(&(WsSend::Pong, b"hi".to_vec())));
}

#[test]
fn a_refused_upgrade_is_dialled_again_with_a_cooldown() {
    let mut r = rig();
    r.net.upgrades.push_back(Ok(WsUpgrade::Refused(401)));
    r.net.upgrades.push_back(Ok(WsUpgrade::Switched));
    r.settle();
    assert_eq!(r.net.upgrade_headers.len(), 2);
    // Buddy drops the event a refused upgrade held (planner.cpp:636-640); the server asks again with
    // SEND_INFO after the attach.
    assert!(r.events().is_empty());
    r.command(3, json!({"command": "SEND_INFO"}));
    r.answer_for(3, "INFO");
}

#[test]
fn an_accepted_open_lock_does_not_pulse_after_it_expired() {
    // The command is taken, then the network drops for long before ACCEPTED is out (the board gives
    // the event up). When it is back the command has expired: the lock must not open late.
    let mut r = rig();
    r.clock.set_unix(Some(1000));
    r.settle();
    r.open_lock(9, 0, 1005);
    while !r.net.inbound.is_empty() {
        r.step();
    }
    r.net.send_fails = true;
    r.clock.set_unix(Some(2000));
    r.steps(200);
    r.net.send_fails = false;
    r.steps(200);
    assert!(
        r.relays.pulses().is_empty(),
        "pulsed late: {:?}",
        r.relays.pulses()
    );
    // The board is not stuck on it: a new command in time runs.
    r.open_lock(10, 0, 2005);
    r.answer_for(10, "FINISHED");
    assert_eq!(r.relays.pulses(), vec![(0, 500)]);
}

#[test]
fn an_open_lock_late_after_a_short_drop_fails_expired() {
    // A short drop: ACCEPTED goes out on a retry, but by then the command has expired.
    let mut r = rig();
    r.clock.set_unix(Some(1000));
    r.settle();
    r.open_lock(11, 0, 1005);
    while !r.net.inbound.is_empty() {
        r.step();
    }
    r.net.send_fails = true;
    r.clock.set_unix(Some(2000));
    r.steps(2);
    r.net.send_fails = false;
    r.answer_for(11, "ACCEPTED");
    let failed = r.answer_for(11, "FAILED");
    assert_eq!(failed["reason"], "Expired");
    assert_eq!(failed["machine_reason"], "EXPIRED");
    assert!(r.relays.pulses().is_empty());
}

#[test]
fn an_input_that_reads_again_clears_the_error_a_relay_does_not() {
    let mut r = rig();
    r.clock.set_unix(Some(1000));
    r.settle();
    r.inputs.set_fail(true);
    r.until("error", |f| f.iter().any(|v| v["state"] == "ERROR"));
    // An input that cannot be read is null, only then.
    r.until("null door", |f| {
        f.iter()
            .any(|v| v["state"] == "ERROR" && v["locks"][0]["door"].is_null())
    });
    r.inputs.set_fail(false);
    r.open_lock(1, 0, 1005);
    r.answer_for(1, "FINISHED");
    assert_eq!(r.relays.pulses(), vec![(0, 500)]);
    // A relay that failed keeps the board in ERROR.
    r.relays.set_fail(true);
    r.open_lock(2, 1, 1005);
    r.answer_for(2, "FAILED");
    r.relays.set_fail(false);
    r.open_lock(3, 1, 1005);
    assert_eq!(
        r.answer_for(3, "REJECTED")["reason"],
        "Won't accept commands in error state"
    );
}
