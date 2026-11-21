//! Host fakes of the traits, for the tests. Handles are cheap clones sharing one state, so a test can
//! give a clone to the `Printer` and keep one to look at.

use crate::traits::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

#[derive(Default)]
struct ClockInner {
    ms: u32,
    unix: Option<u64>,
}

#[derive(Clone, Default)]
pub struct FakeClock(Rc<RefCell<ClockInner>>);

impl FakeClock {
    pub fn advance(&self, ms: u32) {
        let mut c = self.0.borrow_mut();
        c.ms = c.ms.wrapping_add(ms);
    }
    pub fn set_unix(&self, unix: Option<u64>) {
        self.0.borrow_mut().unix = unix;
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u32 {
        self.0.borrow().ms
    }
    fn unix_time(&self) -> Option<u64> {
        self.0.borrow().unix
    }
    fn sleep_ms(&mut self, ms: u32) {
        // A real sleep of 0 ms still lets a tick pass (`osDelay(0)`).
        self.advance(ms.max(1));
    }
}

#[derive(Default)]
struct RelaysInner {
    pulses: Vec<(u32, u32)>,
    fail: bool,
}

#[derive(Clone)]
pub struct FakeRelays {
    inner: Rc<RefCell<RelaysInner>>,
    clock: FakeClock,
}

impl FakeRelays {
    pub fn new(clock: &FakeClock) -> FakeRelays {
        FakeRelays {
            inner: Rc::default(),
            clock: clock.clone(),
        }
    }
    /// `(lock, ms)` of every pulse.
    pub fn pulses(&self) -> Vec<(u32, u32)> {
        self.inner.borrow().pulses.clone()
    }
    pub fn set_fail(&self, fail: bool) {
        self.inner.borrow_mut().fail = fail;
    }
}

impl Relays for FakeRelays {
    fn pulse(&mut self, lock: u32, ms: u32) -> Result<(), HwError> {
        if self.inner.borrow().fail {
            return Err(HwError);
        }
        self.inner.borrow_mut().pulses.push((lock, ms));
        self.clock.advance(ms);
        Ok(())
    }
}

#[derive(Clone)]
pub struct FakeInputs(Rc<RefCell<Vec<Option<DoorState>>>>);

impl FakeInputs {
    pub fn new(doors: Vec<Option<DoorState>>) -> FakeInputs {
        FakeInputs(Rc::new(RefCell::new(doors)))
    }
    pub fn set(&self, lock: usize, door: Option<DoorState>) {
        self.0.borrow_mut()[lock] = door;
    }
}

impl Inputs for FakeInputs {
    fn read_door(&mut self, lock: u32) -> Result<Option<DoorState>, HwError> {
        self.0.borrow().get(lock as usize).copied().ok_or(HwError)
    }
}

#[derive(Clone, Default)]
pub struct FakeStore(Rc<RefCell<Option<String>>>);

impl FakeStore {
    pub fn with_token(token: &str) -> FakeStore {
        FakeStore(Rc::new(RefCell::new(Some(token.to_string()))))
    }
    pub fn stored_token(&self) -> Option<String> {
        self.0.borrow().clone()
    }
}

impl Store for FakeStore {
    fn token(&self) -> Option<String> {
        self.0.borrow().clone()
    }
    fn set_token(&mut self, token: &str) {
        *self.0.borrow_mut() = Some(token.to_string());
    }
    fn serial(&self) -> String {
        "DOOR-0001".to_string()
    }
    fn fingerprint(&self) -> String {
        "fingerprint-of-door-0001".to_string()
    }
}

/// `(method, path, headers, body)` of one HTTP request.
pub type HttpLogEntry = (Method, &'static str, Vec<(String, String)>, String);

/// A scripted server: answers HTTP and the upgrade from queues, collects what the client sends.
pub struct FakeNet {
    pub clock: FakeClock,
    pub http_answers: VecDeque<Result<HttpResponse, NetError>>,
    pub http_log: Vec<HttpLogEntry>,
    pub upgrades: VecDeque<Result<WsUpgrade, NetError>>,
    pub upgrade_headers: Vec<Vec<(String, String)>>,
    pub sent: Vec<(WsSend, Vec<u8>)>,
    pub inbound: VecDeque<WsMessage>,
    pub invalidated: u32,
    /// While true every text send fails (the network is down).
    pub send_fails: bool,
}

impl FakeNet {
    pub fn new(clock: &FakeClock) -> FakeNet {
        FakeNet {
            clock: clock.clone(),
            http_answers: VecDeque::new(),
            http_log: vec![],
            upgrades: VecDeque::new(),
            upgrade_headers: vec![],
            sent: vec![],
            inbound: VecDeque::new(),
            invalidated: 0,
            send_fails: false,
        }
    }
    /// The server sends a `J` command with this id and JSON.
    pub fn push_json_command(&mut self, id: u32, json: &str) {
        let mut m = format!("J{id:08x}").into_bytes();
        m.extend_from_slice(json.as_bytes());
        self.inbound.push_back(WsMessage::Data(m));
    }
    /// What the client sent as text frames, as strings.
    pub fn texts(&self) -> Vec<String> {
        self.sent
            .iter()
            .filter(|(k, _)| *k == WsSend::Text)
            .map(|(_, d)| String::from_utf8(d.clone()).expect("utf-8"))
            .collect()
    }
}

impl Net for FakeNet {
    fn http(&mut self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        self.http_log.push((
            req.method,
            req.path,
            req.headers.clone(),
            String::from_utf8_lossy(&req.body).into_owned(),
        ));
        self.http_answers
            .pop_front()
            .unwrap_or(Err(NetError::Connect))
    }
    fn invalidate(&mut self) {
        self.invalidated += 1;
    }
    fn ws_upgrade(&mut self, headers: &[(String, String)]) -> Result<WsUpgrade, NetError> {
        self.upgrade_headers.push(headers.to_vec());
        self.upgrades.pop_front().unwrap_or(Ok(WsUpgrade::Switched))
    }
    fn ws_send(&mut self, kind: WsSend, data: &[u8]) -> Result<(), NetError> {
        if self.send_fails && kind == WsSend::Text {
            self.clock.advance(1000);
            return Err(NetError::Network);
        }
        self.sent.push((kind, data.to_vec()));
        Ok(())
    }
    fn ws_receive(&mut self, _wait_ms: Option<u32>) -> Result<Option<WsMessage>, NetError> {
        Ok(self.inbound.pop_front())
    }
    fn ws_readable(&mut self, ms: u32) -> bool {
        if !self.inbound.is_empty() {
            return true;
        }
        self.clock.advance(ms);
        false
    }
}
