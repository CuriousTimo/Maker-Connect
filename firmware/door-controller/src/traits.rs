//! The hardware and the network behind traits, so the ESP32 binding can be added later.

/// What a door's switch says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoorState {
    Open,
    Closed,
}

/// A relay or an input that does not answer (Part 7: the device's state `ERROR`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HwError;

/// The relays that drive the locks.
pub trait Relays {
    /// One pulse of `ms` milliseconds on the channel of `lock`; returns when the pulse has ended.
    fn pulse(&mut self, lock: u32, ms: u32) -> Result<(), HwError>;
}

/// The doors' switches.
pub trait Inputs {
    /// What the input reads. An input with no switch wired reads `Open`; the hub knows which locks have
    /// a switch (Part 7), the board does not.
    fn read_door(&mut self, lock: u32) -> Result<DoorState, HwError>;
}

/// The clock. Buddy's `now()`/`sleep_raw()` (`sleep.cpp:252-258`) are `now_ms`/`sleep_ms`.
pub trait Clock {
    /// Monotonic milliseconds, wrapping (Buddy's `ticks_ms()`).
    fn now_ms(&self) -> u32;
    /// Unix seconds, or `None` while the clock is not set (SNTP not done).
    fn unix_time(&self) -> Option<u64>;
    fn sleep_ms(&mut self, ms: u32);
}

/// NVS-like storage and the board's identity.
pub trait Store {
    fn token(&self) -> Option<String>;
    fn set_token(&mut self, token: &str);
    fn serial(&self) -> String;
    /// Buddy makes it from chip id, MAC and serial (`support_utils.cpp:22-60`); the binding does here.
    fn fingerprint(&self) -> String;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

pub struct HttpRequest {
    pub method: Method,
    pub path: &'static str,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

/// Buddy's `http::Error` (the ones `err_to_status` in `status.cpp:7-29` tells apart).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetError {
    Connect,
    Dns,
    Internal,
    Network,
    Timeout,
    UnexpectedEof,
    Parse,
    Tls,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsMessage {
    /// A whole text or binary message (Buddy does not tell them apart, `connect.cpp:427-430`).
    Data(Vec<u8>),
    Ping(Vec<u8>),
    Pong,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WsSend {
    Text,
    Ping,
    Pong,
}

/// The outcome of the `GET /p/ws` upgrade (`connect.cpp:586-632`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WsUpgrade {
    /// `101`, and the binding checked the accept key and the supported headers
    /// (`upgrade_hdrs.key_matched() && all_supported()`, `connect.cpp:594`).
    Switched,
    /// `101`, but the check failed.
    Unverified,
    Refused(u16),
}

/// HTTP and one WebSocket to the server (host, port, TLS are the binding's).
pub trait Net {
    /// One request/response exchange (the registration).
    fn http(&mut self, req: &HttpRequest) -> Result<HttpResponse, NetError>;
    /// Throw the connection away (Buddy's `conn_factory.invalidate()`).
    fn invalidate(&mut self);
    /// `GET /p/ws` with these headers (Buddy's `UpgradeRequest`, `connect.cpp:151-189`).
    fn ws_upgrade(&mut self, headers: &[(String, String)]) -> Result<WsUpgrade, NetError>;
    fn ws_send(&mut self, kind: WsSend, data: &[u8]) -> Result<(), NetError>;
    /// `wait_ms`: `Some(0)` does not wait, `None` waits for the rest of a message.
    fn ws_receive(&mut self, wait_ms: Option<u32>) -> Result<Option<WsMessage>, NetError>;
    /// Whether data waits on the socket, within `ms` (Buddy's `poll_readable`, `sleep.cpp:241`).
    fn ws_readable(&mut self, ms: u32) -> bool;
}
